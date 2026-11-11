//! The write log on real files, in a temporary directory.
//!
//! The simulated disk is where the log's promises are tested against
//! failures. These tests show that the same code runs on a real file
//! system, and that a record acknowledged to another process survives
//! `kill -9` of the process that wrote it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use skyzfs_local::format::{Bad, Identity, Kind, Lsn};
use skyzfs_local::fs::FsDir;
use skyzfs_local::log::{Log, Options, Stop};

fn identity(align: u32, segment_size: u64) -> Identity {
    Identity {
        volume: *b"real-file-tests!",
        device_size: 1 << 30,
        block_size: 4096,
        align,
        segment_size,
        write_unit: Identity::system_write_unit(),
    }
}

/// What record `i` of these tests holds: a length and a fill byte that
/// depend on `i`, so that a record in the wrong place is noticed.
fn payload(i: u64) -> Vec<u8> {
    vec![(i % 251) as u8 + 1; 1 + (i as usize * 37) % 9000]
}

/// The records of the log, as (offset, payload).
fn records(log: &Log<FsDir>) -> Vec<(u64, Vec<u8>)> {
    let mut out = Vec::new();
    log.replay(log.floor(), log.tail(), &mut |r| {
        assert_eq!(r.kind, Kind::Write);
        out.push((r.offset, r.data.to_vec()));
    })
    .unwrap();
    out
}

/// Real files, round trip: records appended across several segments
/// come back after a reopen, at both alignments; reclaim frees segment
/// files for reuse; recovery says whether it could read from the device
/// (it can on ext4 and xfs; a tmpfs refuses the flag or reads from memory).
#[test]
fn records_survive_a_reopen_on_real_files() {
    for align in [64, 4096] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let mut log = Log::create(dir.clone(), identity(align, 64 << 10)).unwrap();
        let mut first_of_second_half = Lsn(0);
        for i in 0..200 {
            let at = log.append(Kind::Write, i * 4096, 0, &payload(i)).unwrap();
            if i == 100 {
                first_of_second_half = at.start;
            }
            if i % 17 == 0 {
                log.sync().unwrap();
            }
        }
        let tail = log.tail();
        assert_eq!(log.sync().unwrap(), tail);
        let files = log.segment_files();
        assert!(files > 4, "{files} segment files");
        drop(log);

        let (mut log, recovery) = Log::open(dir.clone(), Options::default()).unwrap();
        assert_eq!(recovery.end, tail);
        // A log that was closed cleanly ends with nothing behind it.
        assert_eq!(recovery.stopped, Stop::Header(Bad::Blank));
        if !recovery.checked_device {
            eprintln!(
                "{}: no reads from the device here; recovery read through the cache",
                tmp.path().display()
            );
        }
        let want: Vec<(u64, Vec<u8>)> = (0..200).map(|i| (i * 4096, payload(i))).collect();
        assert_eq!(records(&log), want);

        // Give up the first half and write the same amount again: the
        // files of the first half are reused.
        assert_eq!(
            log.reclaim(first_of_second_half).unwrap(),
            first_of_second_half
        );
        for i in 0..100 {
            log.append(Kind::Write, (200 + i) * 4096, 0, &payload(i))
                .unwrap();
        }
        log.sync().unwrap();
        assert!(
            log.segment_files() <= files + 1,
            "{} segment files after reuse, {files} before",
            log.segment_files()
        );
        drop(log);
        let (log, _) = Log::open(dir, Options::default()).unwrap();
        let mut want = want[100..].to_vec();
        want.extend((0..100).map(|i| ((200 + i) * 4096, payload(i))));
        assert_eq!(records(&log), want);
    }
}

const CHILD_DIR: &str = "SKYZFS_LOCAL_KILL_TEST_DIR";

/// Not a test of its own: the body of the child process of
/// `acknowledged_records_survive_kill_9`. Run without the environment
/// variable, as the test harness does, it does nothing.
///
/// It opens the log and appends records for ever, telling its parent
/// the number of each one once the append has returned.
#[test]
fn kill_test_child() {
    let Some(path) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let dir = FsDir::new(path).unwrap();
    let (mut log, _) = Log::open(dir, Options::default()).unwrap();
    let mut next = records(&log).len() as u64;
    let stdout = std::io::stdout();
    loop {
        log.append(Kind::Write, next * 4096, 0, &payload(next))
            .unwrap();
        // The acknowledgement: one whole line, written at once.
        let mut out = stdout.lock();
        out.write_all(format!("ack {next}\n").as_bytes()).unwrap();
        out.flush().unwrap();
        drop(out);
        if next % 64 == 63 {
            log.sync().unwrap();
        }
        next += 1;
    }
}

/// Process kill, for real: a child process appends records and reports
/// each acknowledgement over a pipe; the parent kills it with SIGKILL
/// at a moment of its choosing, reopens the log, and finds every
/// acknowledged record, in order, with at most one more behind them
/// (the one being appended when the child died). Several rounds, each
/// continuing the same log.
#[test]
fn acknowledged_records_survive_kill_9() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    drop(Log::create(dir.clone(), identity(64, 256 << 10)).unwrap());
    let exe = std::env::current_exe().unwrap();
    let mut expected: u64 = 0;
    // No random numbers: the clock decides when the child dies.
    let mut moment = Instant::now().elapsed().subsec_nanos() as u64 | 1;
    for round in 0..6 {
        let mut child = Command::new(&exe)
            .args([
                "--exact",
                "kill_test_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_DIR, tmp.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        moment = moment
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let kill_after = 1 + (moment >> 33) % 3000;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut acked = None;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut seen = 0;
        while seen < kill_after && Instant::now() < deadline {
            let Some(Ok(line)) = lines.next() else {
                break;
            };
            // The harness prints lines of its own.
            if let Some(number) = line.strip_prefix("ack ") {
                acked = Some(number.parse::<u64>().unwrap());
                seen += 1;
            }
        }
        child.kill().unwrap();
        // Acknowledgements that were in the pipe when it died count too.
        for line in lines.map_while(Result::ok) {
            if let Some(number) = line.strip_prefix("ack ") {
                if let Ok(number) = number.parse::<u64>() {
                    acked = Some(number);
                }
            }
        }
        child.wait().unwrap();
        let acked =
            acked.unwrap_or_else(|| panic!("round {round}: the child acknowledged nothing"));
        assert!(
            acked + 1 >= expected,
            "round {round}: the child started at {acked}"
        );

        let (log, _) = Log::open(dir.clone(), Options::default()).unwrap();
        let held = records(&log);
        let count = held.len() as u64;
        assert!(
            count == acked + 1 || count == acked + 2,
            "round {round}: {} records acknowledged, the log holds {count}",
            acked + 1
        );
        for (i, (offset, data)) in held.iter().enumerate() {
            assert_eq!(*offset, i as u64 * 4096, "round {round}: record {i}");
            assert_eq!(*data, payload(i as u64), "round {round}: record {i}");
        }
        expected = count;
    }
}

/// A sanity number, not a gate: appends per second and the latency of a
/// sync on this machine's temporary directory, to catch anything that
/// is accidentally quadratic. Run with
/// `cargo test -p skyzfs-local --release --test real_files -- --ignored --nocapture`.
#[test]
#[ignore = "prints numbers; run by hand"]
fn sanity_numbers() {
    for align in [64u32, 4096] {
        for size in [4096usize, 128 << 10] {
            let tmp = tempfile::tempdir().unwrap();
            let dir = FsDir::new(tmp.path()).unwrap();
            let created = Instant::now();
            let mut log = Log::create(dir, identity(align, 64 << 20)).unwrap();
            let creating = created.elapsed();
            let data = vec![0x5A; size];
            let count = (256 << 20) / size as u64;
            let started = Instant::now();
            let mut halves = [Duration::ZERO; 2];
            for i in 0..count {
                log.append(Kind::Write, i * 4096, 0, &data).unwrap();
                if i == count / 2 {
                    halves[0] = started.elapsed();
                }
            }
            let appending = started.elapsed();
            halves[1] = appending - halves[0];
            let syncing = Instant::now();
            log.sync().unwrap();
            let first_sync = syncing.elapsed();
            let mut latencies = Vec::new();
            for i in 0..200 {
                log.append(Kind::Write, i * 4096, 0, &data).unwrap();
                let t = Instant::now();
                log.sync().unwrap();
                latencies.push(t.elapsed());
            }
            latencies.sort();
            log.sync().unwrap();
            let reading = Instant::now();
            let mut seen = 0u64;
            log.replay(log.floor(), log.tail(), &mut |_| seen += 1)
                .unwrap();
            let replaying = reading.elapsed();
            println!(
                "align {align:>4}, {size:>6}-byte writes: create {creating:?}; {count} appends in \
                 {appending:?} ({:.0}/s, {:.0} MiB/s; first half {:?}, second half {:?}); sync of \
                 all of it {first_sync:?}; append+sync median {:?}, p99 {:?}; replay of {seen} \
                 records {replaying:?}; {} segment files",
                count as f64 / appending.as_secs_f64(),
                (count * size as u64) as f64 / appending.as_secs_f64() / (1 << 20) as f64,
                halves[0],
                halves[1],
                latencies[latencies.len() / 2],
                latencies[latencies.len() * 99 / 100],
                log.segment_files(),
            );
        }
    }
}

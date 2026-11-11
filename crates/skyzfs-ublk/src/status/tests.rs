use super::*;
use crate::lifecycle::{Failure, Held, Reason, Shape};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};

fn counters(reads: u64, held: u64) -> Counters {
    Counters {
        reads,
        in_flight: held,
        ..Counters::default()
    }
}

fn sample() -> Status {
    Status {
        pid: 0x0102_0304,
        queues: 0x1112,
        depth: 0x2122,
        boot: [7; 16],
        identity: [3; 16],
        written_ms: 1,
        counters: Counters {
            reads: 2,
            writes: 3,
            flushes: 4,
            discards: 5,
            write_zeroes: 6,
            refused: 7,
            in_flight: 8,
            unstored: 9,
        },
        failures: 10,
        first_write_copies: 11,
        recovered_unacknowledged: 12,
    }
}

#[test]
fn the_file_has_the_documented_layout() {
    let bytes = encode(&sample());
    let mut expected = Vec::new();
    expected.extend(b"SKYZSTA2");
    expected.extend([4, 3, 2, 1, 0x12, 0x11, 0x22, 0x21]);
    expected.extend([7; 16]);
    expected.extend([3; 16]);
    for field in 1..=12u64 {
        expected.extend(field.to_le_bytes());
    }
    assert_eq!(bytes.len(), 144);
    assert_eq!(bytes.to_vec(), expected);
    assert_eq!(decode(&bytes), Some(sample()));
}

/// A file of the first version (128 bytes, `SKYZSTA1`) is no status,
/// and not an error: nothing that wrote one ever served.
#[test]
fn a_file_of_the_first_version_reads_as_no_status() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let mut old = encode(&sample())[..128].to_vec();
    old[..8].copy_from_slice(b"SKYZSTA1");
    fs::write(&path, &old).expect("write");
    assert_eq!(read(&path).expect("read"), None);
    // The new magic at the old length is a part of a file: an error.
    old[..8].copy_from_slice(b"SKYZSTA2");
    fs::write(&path, &old).expect("write");
    assert!(read(&path).is_err());
}

#[test]
fn the_file_is_replaced_whole_and_a_part_of_one_is_an_error() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    assert_eq!(read(&path).expect("read"), None);
    write(&path, &sample()).expect("write");
    assert_eq!(read(&path).expect("read"), Some(sample()));

    // The old file, held open, keeps its bytes: the new one is another
    // file under the same name.
    use std::io::Read;
    use std::os::unix::fs::MetadataExt;
    let inode = fs::metadata(&path).expect("there").ino();
    let mut held = fs::File::open(&path).expect("open");
    let mut next = sample();
    next.counters.reads = 99;
    write(&path, &next).expect("write");
    assert_ne!(fs::metadata(&path).expect("there").ino(), inode);
    let mut old = Vec::new();
    held.read_to_end(&mut old).expect("read");
    assert_eq!(decode(&old), Some(sample()));
    assert_eq!(read(&path).expect("read"), Some(next));
    assert_eq!(fs::read_dir(dir.path()).expect("list").count(), 1);

    let whole = encode(&sample());
    for len in 0..whole.len() {
        fs::write(&path, &whole[..len]).expect("write");
        assert!(read(&path).is_err(), "{len} bytes were read as a status");
    }
}

#[test]
fn the_uptime_is_read_in_milliseconds() {
    assert_eq!(parse_uptime("12345.67 54321.00\n"), Some(12_345_670));
    assert_eq!(parse_uptime("0.05 1.00"), Some(50));
    assert_eq!(parse_uptime("7 1"), Some(7000));
    assert_eq!(parse_uptime("7.1234"), Some(7123));
    for wrong in ["", "x", "1.x", "-1.0", "+1.0", ".5"] {
        assert_eq!(parse_uptime(wrong), None, "{wrong:?}");
    }
    if cfg!(target_os = "linux") {
        let (a, b) = (uptime_ms().expect("uptime"), uptime_ms().expect("uptime"));
        assert!(a > 0 && b >= a);
    }
}

/// Drive a pace with a clock of our own. `changes` are (time, reads);
/// between them and up to `end` the pace is asked every millisecond,
/// as the publishing thread would be woken. Returns the writes as
/// (time, reads).
fn drive(mutants: u32, changes: &[(u64, u64)], end: u64, fails_at: &[u64]) -> Vec<(u64, u64)> {
    let mut pace = Pace::new(100).with(mutants);
    let mut current = Counters::default();
    let mut writes = Vec::new();
    // The thread sleeps until a change or until the time it was told.
    let mut wake_at: Option<u64> = Some(0);
    for now in 0..=end {
        let changed = changes.iter().find(|(at, _)| *at == now);
        if let Some((_, reads)) = changed {
            current = counters(*reads, 0);
        }
        if changed.is_none() && wake_at != Some(now) {
            continue;
        }
        wake_at = None;
        match pace.due(&current, now) {
            Due::Never => {}
            Due::At(at) => wake_at = Some(at),
            Due::Now if fails_at.contains(&now) => {
                pace.failed(now);
                writes.push((now, u64::MAX));
                wake_at = Some(now + 1);
            }
            Due::Now => {
                pace.wrote(current, now);
                writes.push((now, current.reads));
                wake_at = Some(now + 1);
            }
        }
    }
    writes
}

fn converges(mutants: u32) -> Result<(), String> {
    // A burst: a change every 10 ms for 250 ms, then nothing.
    let changes: Vec<(u64, u64)> = (1..=25).map(|i| (i * 10, i)).collect();
    let writes = drive(mutants, &changes, 2000, &[]);
    let last = writes.last().ok_or("nothing was written")?;
    if last.1 != 25 {
        return Err(format!("the last change was never written: {writes:?}"));
    }
    if last.0 > 250 + 100 {
        return Err(format!("the last change was written at {}", last.0));
    }
    Ok(())
}

fn paced(mutants: u32) -> Result<(), String> {
    let changes: Vec<(u64, u64)> = (1..=25).map(|i| (i * 10, i)).collect();
    let writes = drive(mutants, &changes, 2000, &[]);
    for pair in writes.windows(2) {
        if pair[1].0 - pair[0].0 < 100 {
            return Err(format!("two writes within the interval: {pair:?}"));
        }
    }
    Ok(())
}

fn quiet(mutants: u32) -> Result<(), String> {
    let writes = drive(mutants, &[(10, 1)], 2000, &[]);
    // The first counters (all zero) at the start, and the one change.
    if writes != [(0, 0), (100, 1)] {
        return Err(format!("{writes:?}"));
    }
    Ok(())
}

fn a_failure_waits(mutants: u32) -> Result<(), String> {
    let writes = drive(mutants, &[(150, 1)], 2000, &[150, 151, 152]);
    if writes != [(0, 0), (150, u64::MAX), (250, 1)] {
        return Err(format!("{writes:?}"));
    }
    Ok(())
}

#[test]
fn the_pace_converges_is_quiet_and_keeps_its_interval() {
    for (name, rule, defect) in [
        (
            "converges",
            converges as fn(u32) -> Result<(), String>,
            Mutants::NEEDS_ANOTHER_CHANGE,
        ),
        ("paced", paced, Mutants::NO_INTERVAL),
        ("quiet", quiet, Mutants::REWRITES_UNCHANGED),
        ("a failure waits", a_failure_waits, Mutants::RETRIES_AT_ONCE),
    ] {
        assert_eq!(rule(0), Ok(()), "{name}");
        assert!(
            rule(defect).is_err(),
            "the defect against `{name}` is not noticed"
        );
    }
}

/// The publishing thread, for real: a burst, then an idle queue.
#[test]
fn after_a_burst_the_file_shows_the_last_change_with_no_further_request() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join(STATUS);
    let board = Arc::new(Board::new());
    let written = Arc::new(AtomicU64::new(0));
    let thread = {
        let (board, path, written) = (board.clone(), path.clone(), written.clone());
        std::thread::spawn(move || {
            board.publish(Pace::new(50), |counters, failures| {
                written.fetch_add(1, Ordering::SeqCst);
                let mut status = sample();
                status.counters = *counters;
                status.failures = failures;
                write(&path, &status)
            })
        })
    };
    for i in 1..=200 {
        board.post(counters(i, 200 - i));
        if i % 20 == 0 {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    // The queue is idle now: held is 0 and nothing more is posted. The
    // bound is one interval and one write; the test allows a loaded
    // machine far more, and fails only if the change never comes.
    let shown = |path: &Path| read(path).ok().flatten().map(|s| s.counters);
    let start = Instant::now();
    while shown(&path) != Some(counters(200, 0)) {
        assert!(
            start.elapsed() < Duration::from_secs(20),
            "the last change was not written: {:?}",
            shown(&path)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    // And then nothing is rewritten while nothing changes.
    let writes = written.load(Ordering::SeqCst);
    assert!(writes < 200, "paced: {writes} writes for 200 changes");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(written.load(Ordering::SeqCst), writes);
    // Posting the same counters wakes nothing.
    board.post(counters(200, 0));
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(written.load(Ordering::SeqCst), writes);

    board.stop();
    assert_eq!(thread.join().expect("the thread"), 0);
}

#[test]
fn a_write_that_blocks_or_fails_holds_no_post_and_the_last_counters_are_written_at_the_stop() {
    let board = Arc::new(Board::new());
    let (entered_tx, entered) = mpsc::channel();
    let (release, released) = mpsc::channel::<()>();
    let (seen_tx, seen) = mpsc::channel();
    let thread = {
        let board = board.clone();
        std::thread::spawn(move || {
            let mut first = true;
            board.publish(Pace::new(10_000), move |counters, failures| {
                if first {
                    // The first write hangs, and then fails.
                    first = false;
                    let _ = entered_tx.send(());
                    let _ = released.recv();
                    return Err(io::Error::other("no space"));
                }
                let _ = seen_tx.send((*counters, failures));
                Ok(())
            })
        })
    };
    // The publishing thread is inside its write. Posts return at once.
    entered.recv().expect("the write began");
    let start = Instant::now();
    for i in 1..=1000 {
        board.post(counters(i, 0));
    }
    assert!(start.elapsed() < Duration::from_secs(5));
    // The stop writes the last counters though the interval (ten
    // seconds) is far from over, and the failure is counted.
    board.stop();
    release.send(()).expect("release");
    assert_eq!(thread.join().expect("the thread"), 1);
    assert_eq!(
        seen.try_iter().collect::<Vec<_>>(),
        [(counters(1000, 0), 1)]
    );
}

fn located(state: DeviceState, whose: Whose) -> Located {
    Located {
        number: 4,
        state,
        shape: Shape {
            size: 1 << 30,
            queues: 2,
            depth: 32,
            flags: 0,
        },
        whose,
    }
}

fn value<'a>(lines: &'a [(&'static str, String)], key: &str) -> Option<&'a str> {
    lines
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, v)| v.as_str())
}

#[test]
fn the_report_has_the_keys_the_rig_reads_and_says_whose_the_counters_are() {
    let status = sample();
    let lines = report(
        Some(&located(DeviceState::Live, Whose::Ours)),
        Some(&status),
        Noted::Nothing,
        &[7; 16],
        251,
    );
    let keys: Vec<&str> = lines.iter().map(|(k, _)| *k).collect();
    assert_eq!(
        keys,
        [
            "state",
            "device",
            "queues",
            "depth",
            "reads",
            "writes",
            "flushes",
            "discards",
            "write_zeroes",
            "refused",
            "held",
            "unstored",
            "first_write_copies",
            "recovered_unacknowledged",
            "status_write_failures",
            "age_ms",
            "pid",
            "counters"
        ]
    );
    // The process to signal: only while the kernel shows a live server.
    assert_eq!(value(&lines, "pid"), Some("16909060"));
    assert_eq!(value(&lines, "state"), Some("serving"));
    assert_eq!(value(&lines, "device"), Some("/dev/ublkb4"));
    // The device's own values, not the file's.
    assert_eq!(value(&lines, "queues"), Some("2"));
    assert_eq!(value(&lines, "depth"), Some("32"));
    assert_eq!(value(&lines, "reads"), Some("2"));
    assert_eq!(value(&lines, "held"), Some("8"));
    assert_eq!(value(&lines, "unstored"), Some("9"));
    assert_eq!(value(&lines, "age_ms"), Some("250"));
    assert_eq!(
        value(&lines, "counters"),
        Some("of the serving daemon, pid 16909060")
    );

    // No live server: the counters were left by a daemon that is gone.
    for (state, said) in [
        (DeviceState::Waiting, "waiting for a server"),
        (
            DeviceState::Dead,
            "dead: never started, or deleted and still held",
        ),
    ] {
        let lines = report(
            Some(&located(state, Whose::Ours)),
            Some(&status),
            Noted::Nothing,
            &[7; 16],
            251,
        );
        assert_eq!(value(&lines, "state"), Some(said));
        assert_eq!(value(&lines, "pid"), None);
        assert_eq!(
            value(&lines, "counters"),
            Some("left by a daemon that is gone, pid 16909060")
        );
    }
}

#[test]
fn the_report_shows_no_device_that_is_not_this_volumes_and_no_counters_of_another_boot() {
    let status = sample();
    let lines = report(None, None, Noted::Nothing, &[7; 16], 0);
    assert_eq!(lines, [("state", "detached".to_string())]);

    // Detached, with the file a daemon left: its counters, marked.
    let lines = report(None, Some(&status), Noted::Nothing, &[7; 16], 251);
    assert_eq!(value(&lines, "state"), Some("detached"));
    assert_eq!(value(&lines, "device"), None);
    assert_eq!(value(&lines, "queues"), Some("4370"));
    assert!(value(&lines, "counters").is_some_and(|c| c.starts_with("left by")));

    // A file of another boot says nothing about this one.
    let lines = report(None, Some(&status), Noted::Nothing, &[8; 16], 251);
    assert_eq!(lines, [("state", "detached".to_string())]);

    for (whose, begins) in [
        (Whose::Anothers, "detached: device 4"),
        (Whose::Unsure, "unknown: device 4"),
    ] {
        let lines = report(
            Some(&located(DeviceState::Live, whose)),
            Some(&status),
            Noted::Nothing,
            &[7; 16],
            251,
        );
        assert!(value(&lines, "state").is_some_and(|s| s.starts_with(begins)));
        assert_eq!(value(&lines, "device"), None, "not shown as this volume's");
        assert!(value(&lines, "counters").is_some_and(|c| c.starts_with("left by")));
    }
}

#[test]
fn the_report_shows_a_failure_of_this_boot_and_what_it_needs() {
    let stored = |held: bool| StoredFailure {
        boot: [7; 16],
        failure: Failure {
            held: held.into(),
            reason: Reason::LogFailed,
            identity: [3; 16],
        },
        written_ms: 1000,
    };
    let dead = located(DeviceState::Dead, Whose::Ours);
    for (found, noted, state) in [
        (
            Some(&dead),
            Noted::Whole(stored(true)),
            "failed, held by a client: not served again in this boot",
        ),
        (
            None,
            Noted::Whole(stored(false)),
            "failed: needs --acknowledge-failure",
        ),
        (
            None,
            Noted::Unreadable,
            "failed, held by a client: not served again in this boot",
        ),
    ] {
        let lines = report(found, None, noted, &[7; 16], 1250);
        assert_eq!(value(&lines, "state"), Some(state));
        assert_eq!(value(&lines, "device").is_some(), found.is_some());
        if noted == Noted::Unreadable {
            assert_eq!(
                value(&lines, "failure"),
                Some("the failure note cannot be read")
            );
            assert_eq!(value(&lines, "failure_held"), None);
            continue;
        }
        let held = matches!(noted, Noted::Whole(s) if s.failure.held == Held::Yes);
        assert_eq!(
            value(&lines, "failure"),
            Some("the log failed while the volume was served")
        );
        assert_eq!(
            value(&lines, "failure_held"),
            Some(if held { "yes" } else { "no" })
        );
        assert_eq!(
            value(&lines, "failure_boot"),
            Some("07070707070707070707070707070707")
        );
        assert_eq!(value(&lines, "failure_age_ms"), Some("250"));
    }

    // A note of another boot is not shown and decides nothing.
    let lines = report(None, None, Noted::Whole(stored(true)), &[8; 16], 1250);
    assert_eq!(value(&lines, "state"), Some("detached"));
    assert_eq!(value(&lines, "failure"), None);
    assert_eq!(value(&lines, "failure_device"), None);
    assert_eq!(value(&lines, "failure_pool"), None);

    // With a failure of this boot, and no daemon and no status file:
    // what became of the device, in each state it can be found in, and
    // what to do about a pool on it.
    let waiting = located(DeviceState::Waiting, Whose::Ours);
    let unsure = located(DeviceState::Waiting, Whose::Unsure);
    let anothers = located(DeviceState::Live, Whose::Anothers);
    for (found, says) in [
        (None, "none under the volume's recorded number"),
        (Some(&waiting), "waits for a server it will not get"),
        (Some(&dead), "deleted, and something still holds it"),
        (Some(&unsure), "this volume's values and no note"),
        (Some(&anothers), "is not this volume's"),
    ] {
        for noted in [Noted::Whole(stored(false)), Noted::Unreadable] {
            let lines = report(found, None, noted, &[7; 16], 1250);
            let device = value(&lines, "failure_device").unwrap_or_default();
            assert!(device.contains(says), "{found:?}: {device}");
            assert_eq!(value(&lines, "failure_pool"), Some(POOL_WARNING));
            assert_eq!(
                lines.last().map(|(key, _)| *key),
                Some("failure_pool"),
                "the last line"
            );
        }
    }
    // The step it names is a read, and it does not advise an export,
    // which is not known to work in that state.
    assert!(POOL_WARNING.contains("zpool scrub") && POOL_WARNING.contains("reboot"));
    assert!(!POOL_WARNING.contains("export"));
}

/// The order of a start: the publishing thread's first write is tried
/// before whoever waits for it goes on (the daemon then says `ready`),
/// also when that write fails; and nobody is told it was tried before
/// it was.
#[test]
fn the_first_write_is_tried_before_the_waiter_goes_on_also_when_it_fails() {
    for fails in [false, true] {
        let board = Arc::new(Board::new());
        assert!(
            !board.first_write_tried(Duration::from_millis(20)),
            "nothing publishes yet"
        );
        let tried = Arc::new(AtomicU64::new(0));
        let gate = Arc::new(std::sync::Barrier::new(2));
        let thread = {
            let (board, tried, gate) = (board.clone(), tried.clone(), gate.clone());
            std::thread::spawn(move || {
                board.publish(Pace::new(50), |_, _| {
                    if tried.load(Ordering::SeqCst) == 0 {
                        // The first write is held until the test lets
                        // it go.
                        gate.wait();
                    }
                    tried.fetch_add(1, Ordering::SeqCst);
                    if fails {
                        Err(std::io::Error::other("no"))
                    } else {
                        Ok(())
                    }
                })
            })
        };
        // The write is under way and not finished: not tried yet.
        assert!(!board.first_write_tried(Duration::from_millis(50)));
        assert_eq!(tried.load(Ordering::SeqCst), 0);
        gate.wait();
        assert!(board.first_write_tried(Duration::from_secs(20)));
        assert!(tried.load(Ordering::SeqCst) >= 1, "fails: {fails}");
        board.stop();
        let failures = thread.join().expect("the publishing thread");
        assert_eq!(failures > 0, fails);
    }
}

/// Waiting for the first write posts nothing: counters the queue's
/// thread has put up before the start is over are the ones that are
/// written, then and at the end, and are not replaced by zeros.
#[test]
fn waiting_for_the_first_write_leaves_the_posted_counters_as_they_are() {
    let board = Arc::new(Board::new());
    board.post(counters(7, 2));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let thread = {
        let (board, seen) = (board.clone(), seen.clone());
        std::thread::spawn(move || {
            board.publish(Pace::new(10), |counters, _| {
                seen.lock().expect("lock").push(*counters);
                Ok(())
            })
        })
    };
    assert!(board.first_write_tried(Duration::from_secs(20)));
    // No request follows: the queue's thread posts nothing more.
    std::thread::sleep(Duration::from_millis(100));
    board.stop();
    thread.join().expect("the publishing thread");
    let seen = seen.lock().expect("lock");
    assert!(!seen.is_empty());
    assert!(
        seen.iter().all(|written| *written == counters(7, 2)),
        "{seen:?}"
    );
}

//! The store on real files, behind its worker thread: a round trip with
//! a reopen, a volume above 4 GiB, a real `kill -9`, and sanity numbers.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use skyzfs_blk::{Base, BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId};
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::store::{OpenOptions, Served, Store, StoreConfig, StoreError};

const BLOCK: u64 = 4096;
const TIMEOUT: Duration = Duration::from_secs(60);

/// The seed of the kill test's moments, to repeat a run.
const KILL_SEED: &str = "SKYZFS_KILL_SEED";

fn config(size_mib: u64, align: u32) -> StoreConfig {
    StoreConfig {
        volume: *b"a volume on disk",
        size_mib,
        base: Base::Empty,
        align,
        segment_size: 8 << 20,
        write_unit: Identity::system_write_unit(),
    }
}

/// Submit one request and wait for it.
fn run(store: &mut Served, op: Op) -> Completion {
    store
        .submit(Request {
            id: RequestId(0),
            op,
        })
        .unwrap();
    let mut done = Vec::new();
    assert_eq!(store.wait(&mut done, TIMEOUT), 1, "no completion in time");
    done.pop().unwrap()
}

fn write(store: &mut Served, block: u64, data: Buf) {
    let done = run(
        store,
        Op::Write {
            offset: block * BLOCK,
            data,
        },
    );
    assert_eq!(done.result, Ok(()));
}

fn read(store: &mut Served, block: u64, count: usize) -> Buf {
    let done = run(
        store,
        Op::Read {
            offset: block * BLOCK,
            buf: Buf::filled(count, 0xEE),
        },
    );
    assert_eq!(done.result, Ok(()));
    done.buf.unwrap()
}

fn flush(store: &mut Served) {
    assert_eq!(run(store, Op::Flush).result, Ok(()));
}

/// Real files, round trip: writes, write-zeroes and discards are read
/// back from the log, from the home image after a checkpoint, and after
/// a reopen, at both alignments; the files are private to their owner.
#[test]
fn the_store_round_trips_on_real_files() {
    for align in [64, 4096] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let store = Store::create(dir.clone(), &config(16, align), OpenOptions::default()).unwrap();
        let mut store = store.serve().unwrap();
        write(&mut store, 3, Buf::filled(4, 1));
        write(&mut store, 5, Buf::filled(1, 2));
        write(&mut store, 300, Buf::filled(256, 3));
        let zeroes = Op::WriteZeroes {
            offset: 6 * BLOCK,
            len: BLOCK,
        };
        assert_eq!(run(&mut store, zeroes).result, Ok(()));
        let discard = Op::Discard {
            offset: 3 * BLOCK,
            len: BLOCK,
        };
        assert_eq!(run(&mut store, discard).result, Ok(()));
        let check = |store: &mut Served, what: &str| {
            let got = read(store, 2, 6);
            let firsts: Vec<u8> = got.chunks(BLOCK as usize).map(|b| b[0]).collect();
            assert_eq!(firsts, [0, 1, 1, 2, 0, 0], "{what}");
            assert!(read(store, 300, 256).iter().all(|b| *b == 3), "{what}");
            assert!(read(store, 556, 1).iter().all(|b| *b == 0), "{what}");
        };
        check(&mut store, "from the log");
        flush(&mut store);
        let start = store.checkpoint().unwrap();
        assert_eq!(Some(start), store.position());
        check(&mut store, "from the home image");
        write(&mut store, 4, Buf::filled(1, 9));
        drop(store);
        let (store, recovery) = Store::open(dir, OpenOptions::default()).unwrap();
        assert!(recovery.end > recovery.start);
        let mut store = store.serve().unwrap();
        assert_eq!(read(&mut store, 4, 1)[0], 9, "after a reopen");
        assert_eq!(read(&mut store, 5, 1)[0], 2, "after a reopen");
        let beyond = Op::Read {
            offset: 16 << 20,
            buf: Buf::zeroed(1),
        };
        assert!(matches!(
            run(&mut store, beyond).result,
            Err(BlkError::OutOfBounds { .. })
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in std::fs::read_dir(tmp.path()).unwrap() {
                let mode = entry.unwrap().metadata().unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600);
            }
        }
    }
}

/// Real files: a write-zeroes over data that is in the home image gives
/// the space back. 8 MiB are written and checkpointed, 6 MiB of them
/// are zeroed and checkpointed: the home image then holds about 2 MiB,
/// the range reads as zeros and the rest as written, also after a
/// reopen, and the store says that it deallocates. On a file system
/// that cannot, the store says that it writes zeros, and only the
/// content is checked.
#[cfg(target_os = "linux")]
#[test]
fn a_write_zeroes_gives_the_home_images_space_back_on_real_files() {
    use skyzfs_local::store::Zeroes;
    use std::os::unix::fs::MetadataExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let store = Store::create(dir.clone(), &config(16, 4096), OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    for mib in 0..8 {
        write(&mut store, mib * 256, Buf::filled(256, 7));
    }
    flush(&mut store);
    store.checkpoint().unwrap();
    let home = tmp.path().join("home.img");
    let before = std::fs::metadata(&home).unwrap().blocks() * 512;
    assert!(before >= 8 << 20, "the home image holds {before} bytes");
    for mib in 1..7 {
        let zeroes = Op::WriteZeroes {
            offset: mib << 20,
            len: 1 << 20,
        };
        assert_eq!(run(&mut store, zeroes).result, Ok(()));
    }
    flush(&mut store);
    store.checkpoint().unwrap();
    let after = std::fs::metadata(&home).unwrap().blocks() * 512;
    match store.log_state().zeroes {
        Zeroes::Deallocated => assert!(
            after <= before - (5 << 20),
            "the home image held {before} bytes and holds {after}"
        ),
        Zeroes::Written => eprintln!("this file system cannot deallocate: zeros were written"),
        Zeroes::Untried => {
            panic!("a checkpoint applied write-zeroes and the store does not say how")
        }
    }
    let check = |store: &mut Served, what: &str| {
        assert!(read(store, 0, 256).iter().all(|b| *b == 7), "{what}");
        for mib in 1..7 {
            assert!(
                read(store, mib * 256, 256).iter().all(|b| *b == 0),
                "{what}"
            );
        }
        assert!(read(store, 7 * 256, 256).iter().all(|b| *b == 7), "{what}");
    };
    check(&mut store, "after the checkpoint");
    drop(store);
    let (store, _) = Store::open(dir, OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    check(&mut store, "after a reopen");
}

/// Real files, a volume above 4 GiB: the home image is sparse, and
/// writes and reads beyond the 32-bit boundary land where they belong,
/// in the log and in the home image, without touching the same offsets
/// modulo 4 GiB.
#[test]
fn a_volume_above_four_gib_on_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let store = Store::create(dir.clone(), &config(5 * 1024, 4096), OpenOptions::default())
        .unwrap()
        .serve();
    let mut store = store.unwrap();
    let four_gib = (4u64 << 30) / BLOCK;
    let last = store.geometry().blocks() - 1;
    write(&mut store, four_gib + 7, Buf::filled(2, 1));
    write(&mut store, last, Buf::filled(1, 2));
    for round in 0..3 {
        assert_eq!(read(&mut store, four_gib + 7, 2)[0], 1, "round {round}");
        assert_eq!(read(&mut store, last, 1)[0], 2, "round {round}");
        assert_eq!(read(&mut store, 7, 2)[0], 0, "round {round}");
        assert_eq!(read(&mut store, last - four_gib, 1)[0], 0, "round {round}");
        if round == 0 {
            flush(&mut store);
            store.checkpoint().unwrap();
        } else if round == 1 {
            drop(store);
            let (reopened, _) = Store::open(dir.clone(), OpenOptions::default()).unwrap();
            store = reopened.serve().unwrap();
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let home = std::fs::metadata(tmp.path().join("home.img")).unwrap();
        assert_eq!(home.len(), 5 << 30);
        assert!(
            home.blocks() * 512 < 1 << 30,
            "the home image is not sparse"
        );
    }
}

const CHILD_DIR: &str = "SKYZFS_STORE_KILL_TEST_DIR";
const CHILD_FROM: &str = "SKYZFS_STORE_KILL_TEST_FROM";
/// The blocks the kill test writes to.
const KILL_BLOCKS: u64 = 64;

/// What write number `n` of the kill test does: where, how long, and
/// what every block of it holds (its number, over and over).
fn kill_write(n: u64) -> (u64, usize, Buf) {
    let z = n.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 20;
    let count = 1 + (z % 4) as usize;
    let block = (z >> 8) % (KILL_BLOCKS - count as u64 + 1);
    let mut data = Buf::zeroed(count);
    for word in data.chunks_mut(8) {
        word.copy_from_slice(&n.to_le_bytes());
    }
    (block, count, data)
}

/// Not a test of its own: the child of
/// `acknowledged_writes_survive_kill_9`. Serves a writer and reports
/// every acknowledgement and every completed flush, until it is killed.
#[test]
fn store_kill_test_child() {
    let Some(path) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let from: u64 = std::env::var(CHILD_FROM).unwrap().parse().unwrap();
    let dir = FsDir::new(path).unwrap();
    let (store, _) = Store::open(dir, OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    // The reports go to standard error, which the test harness does not
    // write to: its own lines on standard output would be counted as
    // moments, and its unfinished line would swallow the first report.
    let stderr = std::io::stderr();
    let say = |line: String| {
        // One whole line, written at once.
        let mut out = stderr.lock();
        out.write_all(line.as_bytes()).unwrap();
        out.flush().unwrap();
    };
    // The store is open: from here on the lock is held.
    say("open\n".to_string());
    // Until the parent kills this process.
    for n in from..u64::MAX {
        let (block, _, data) = kill_write(n);
        write(&mut store, block, data);
        say(format!("ack {n}\n"));
        if n % 37 == 0 {
            flush(&mut store);
            say(format!("flushed {n}\n"));
        }
        if n % 211 == 0 {
            store.checkpoint().unwrap();
        }
    }
}

/// A full log behind a hold, through the worker thread: a write that
/// finds no room stays with the worker, neither completed nor failed;
/// a read submitted after it is answered meanwhile (since M1.3j a read
/// does not wait for the worker, and rule R8 orders only mutations);
/// when the hold is released the write completes, and the data is
/// there.
#[test]
fn a_write_held_behind_a_hold_completes_when_the_hold_moves() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let mut options = OpenOptions::default();
    options.log_segments = Some(2);
    let store = Store::create(dir, &config(64, 4096), options).unwrap();
    let mut store = store.serve().unwrap();
    write(&mut store, 0, Buf::filled(1, 1));
    flush(&mut store);
    store.set_hold(store.position()).unwrap();
    // Two segments of 8 MiB take fewer than twenty writes of 1 MiB.
    let mut done = Vec::new();
    let mut held = None;
    for n in 1..=20u64 {
        let op = Op::Write {
            offset: n * (1 << 20),
            data: Buf::filled(256, n as u8),
        };
        store
            .submit(Request {
                id: RequestId(n),
                op,
            })
            .unwrap();
        if store.wait(&mut done, Duration::from_millis(1500)) == 0 {
            held = Some(n);
            break;
        }
        assert_eq!(done.pop().unwrap().result, Ok(()), "write {n}");
    }
    let held = held.expect("no write was held behind the hold");
    let behind = Op::Read {
        offset: 0,
        buf: Buf::zeroed(1),
    };
    store
        .submit(Request {
            id: RequestId(100),
            op: behind,
        })
        .unwrap();
    assert_eq!(store.wait(&mut done, TIMEOUT), 1);
    assert_eq!((done[0].id, done[0].result), (RequestId(100), Ok(())));
    assert_eq!(done[0].buf.as_ref().unwrap()[0], 1);
    // The write is still held.
    assert_eq!(store.wait(&mut done, Duration::from_millis(300)), 0);
    store.set_hold(None).unwrap();
    while done.len() < 2 {
        assert!(
            store.wait(&mut done, TIMEOUT) > 0,
            "the held write did not complete"
        );
    }
    assert_eq!((done[1].id, done[1].result), (RequestId(held), Ok(())));
    assert_eq!(read(&mut store, held * 256, 1)[0], held as u8);
    assert_eq!(read(&mut store, 0, 1)[0], 1);
}

/// Every file of a directory with its bytes, sorted by name.
fn snapshot(path: &std::path::Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut out: Vec<_> = std::fs::read_dir(path)
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect();
    out.sort();
    out
}

/// One instance per volume, on real files (design §4.2): while a store
/// is open a second open, and a creation over it, are refused with
/// their own error and leave every file as it was; once the first
/// instance is dropped the store opens.
#[test]
fn a_second_instance_is_refused_on_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let store = Store::create(dir.clone(), &config(16, 64), OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    write(&mut store, 3, Buf::filled(4, 1));
    flush(&mut store);
    let before = snapshot(tmp.path());
    assert!(matches!(
        Store::open(dir.clone(), OpenOptions::default()),
        Err(StoreError::Locked)
    ));
    assert!(matches!(
        Store::create(dir.clone(), &config(16, 64), OpenOptions::default()),
        Err(StoreError::Locked)
    ));
    assert_eq!(snapshot(tmp.path()), before);
    write(&mut store, 4, Buf::filled(1, 2));
    drop(store);
    let (store, _) = Store::open(dir, OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    assert_eq!(read(&mut store, 3, 1)[0], 1);
    assert_eq!(read(&mut store, 4, 1)[0], 2);
}

/// What the store does not write does not keep it from opening: the
/// open asks the block size of the files it writes and of the
/// directory, and leaves a subdirectory and a file nobody may open,
/// put there by hand, alone.
#[cfg(unix)]
#[test]
fn a_stranger_in_the_directory_does_not_refuse_an_open() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let store = Store::create(dir.clone(), &config(16, 64), OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    write(&mut store, 3, Buf::filled(4, 1));
    flush(&mut store);
    drop(store);
    std::fs::create_dir(tmp.path().join("notes")).unwrap();
    let closed = tmp.path().join("closed");
    std::fs::write(&closed, b"x").unwrap();
    std::fs::set_permissions(&closed, std::fs::Permissions::from_mode(0o000)).unwrap();
    let (store, _) = Store::open(dir, OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    assert_eq!(read(&mut store, 3, 1)[0], 1);
}

/// Process kill, for real (rule R9), and the lock with it: while the
/// child lives the parent cannot open the store, and after the kill it
/// can. A child process serves a writer
/// through the store's worker thread and reports each acknowledged
/// write over a pipe; the parent kills it with SIGKILL at a moment the
/// clock chooses, opens the store, and finds in every block the last
/// write that was acknowledged for it, or the one write that was in
/// flight when the child died. The child reports on standard error:
/// `open` once the store is open, then each acknowledgement and each
/// flush. A moment is a number of reports after `open`, from none to
/// 1999, drawn from a seed that is printed. Several rounds on the same store, with
/// checkpoints in between.
#[test]
fn acknowledged_writes_survive_kill_9() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let mut options = OpenOptions::default();
    options.log_segments = Some(2);
    drop(Store::create(dir.clone(), &config(16, 64), options.clone()).unwrap());
    let exe = std::env::current_exe().unwrap();
    // What each block must hold: the number of the last write to it.
    let mut held = vec![None::<u64>; KILL_BLOCKS as usize];
    let mut next = 1u64;
    // The moments of the kills come from the clock, or from the seed a
    // failed run printed.
    let seed = match std::env::var(KILL_SEED) {
        Ok(seed) if !seed.is_empty() => seed.parse().unwrap(),
        _ => std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64,
    };
    println!("kill moments from seed {seed}: set {KILL_SEED}={seed} to repeat them");
    let mut moment = seed | 1;
    for round in 0..6 {
        let mut child = Command::new(&exe)
            .args([
                "--exact",
                "store_kill_test_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(CHILD_DIR, tmp.path())
            .env(CHILD_FROM, next.to_string())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        moment = moment
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        // The moment of the kill: after this many of the child's
        // reports behind "open". None is a moment too: the child dies
        // before its first acknowledgement.
        let kill_after = (moment >> 33) % 2000;
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut acked = None;
        let mut note = |line: &str| {
            if let Some(Ok(n)) = line.strip_prefix("ack ").map(str::parse::<u64>) {
                acked = Some(n);
            }
        };
        let deadline = Instant::now() + TIMEOUT;
        // The child says when it has opened the store.
        loop {
            match lines.next() {
                // The child runs its own program by now.
                Some(Ok(line)) if line == "open" => break,
                Some(Ok(_)) => {}
                _ => panic!("round {round}: the child ended before it had opened the store"),
            }
            assert!(
                Instant::now() < deadline,
                "round {round}: the child did not open the store"
            );
        }
        // While the child lives the store is its own: one instance per
        // volume, across processes.
        assert!(
            matches!(
                Store::open(dir.clone(), options.clone()),
                Err(StoreError::Locked)
            ),
            "round {round}: the store opened under a live instance"
        );
        for seen in 0..kill_after {
            assert!(
                Instant::now() < deadline,
                "round {round}: the child reported {seen} times in {TIMEOUT:?}, not {kill_after}"
            );
            match lines.next() {
                Some(Ok(line)) => note(&line),
                _ => panic!(
                    "round {round}: the child's reports ended after {seen}, before the kill at {kill_after}"
                ),
            }
        }
        child.kill().unwrap();
        // Acknowledgements that were in the pipe when it died count too.
        for line in lines.map_while(Result::ok) {
            note(&line);
        }
        child.wait().unwrap();
        // Killed before its first acknowledgement, the child has added
        // nothing that must be held, and its first write may be there.
        let acked = acked.unwrap_or(next - 1);
        assert!(
            acked + 1 >= next,
            "round {round}: acknowledged {acked}, started at {next}"
        );
        for n in next..=acked {
            let (block, count, _) = kill_write(n);
            for b in block..block + count as u64 {
                held[b as usize] = Some(n);
            }
        }
        // The write that may have been in flight.
        let pending = acked + 1;
        let (pending_block, pending_count, _) = kill_write(pending);

        let (store, _) = Store::open(dir.clone(), options.clone()).unwrap();
        let mut store = store.serve().unwrap();
        let got = read(&mut store, 0, KILL_BLOCKS as usize);
        let mut pending_seen = 0;
        for (b, block) in got.chunks(BLOCK as usize).enumerate() {
            let mut first = [0u8; 8];
            first.copy_from_slice(&block[..8]);
            let n = u64::from_le_bytes(first);
            assert!(
                block.chunks(8).all(|word| word == first),
                "round {round}: block {b} is torn"
            );
            let in_pending =
                (pending_block..pending_block + pending_count as u64).contains(&(b as u64));
            if in_pending && n == pending {
                pending_seen += 1;
                held[b] = Some(pending);
            } else {
                assert_eq!(
                    n,
                    held[b].unwrap_or(0),
                    "round {round}: block {b} holds write {n}, acknowledged was {:?}",
                    held[b]
                );
            }
        }
        assert!(
            pending_seen == 0 || pending_seen == pending_count,
            "round {round}: the write in flight is there in part"
        );
        drop(store);
        next = pending + 1;
    }
}

/// Sanity numbers, not a gate: the store on this machine's temporary
/// directory, to catch anything that is accidentally quadratic. Run
/// with
/// `cargo test -p skyzfs-local --release --test store_real -- --ignored --nocapture`.
#[test]
#[ignore = "prints numbers; run by hand"]
fn store_sanity_numbers() {
    let median = |mut times: Vec<Duration>| {
        times.sort();
        (times[times.len() / 2], times[times.len() * 99 / 100])
    };
    for (size, count) in [(1usize, 2000u64), (32, 500)] {
        let tmp = tempfile::tempdir().unwrap();
        let dir = FsDir::new(tmp.path()).unwrap();
        let config = StoreConfig {
            segment_size: 64 << 20,
            ..config(1024, 64)
        };
        let store = Store::create(dir, &config, OpenOptions::default()).unwrap();
        let mut store = store.serve().unwrap();
        let bytes = size * BLOCK as usize;
        // A flush after each write.
        let mut each = Vec::new();
        for i in 0..count {
            let t = Instant::now();
            write(&mut store, i * size as u64, Buf::filled(size, 1));
            flush(&mut store);
            each.push(t.elapsed());
        }
        let (each_median, each_p99) = median(each);
        // A flush after every hundred.
        let t = Instant::now();
        for i in 0..count {
            write(&mut store, (count + i) * size as u64, Buf::filled(size, 2));
            if i % 100 == 99 {
                flush(&mut store);
            }
        }
        flush(&mut store);
        let batched = t.elapsed();
        // Reads from the log.
        let t = Instant::now();
        for i in 0..count {
            read(&mut store, i * size as u64, size);
        }
        let from_log = t.elapsed();
        let t = Instant::now();
        store.checkpoint().unwrap();
        let checkpointing = t.elapsed();
        // Reads from the home image.
        let t = Instant::now();
        for i in 0..count {
            read(&mut store, i * size as u64, size);
        }
        let from_home = t.elapsed();
        println!(
            "{bytes:>6}-byte writes: write+flush median {each_median:?}, p99 {each_p99:?}; \
             {count} writes with a flush every 100 in {batched:?} ({:.0}/s, {:.0} MiB/s); \
             {count} reads from the log in {from_log:?} ({:.0}/s); a checkpoint of {} MiB in \
             {checkpointing:?}; {count} reads from the home image in {from_home:?} ({:.0}/s)",
            count as f64 / batched.as_secs_f64(),
            (count * bytes as u64) as f64 / batched.as_secs_f64() / (1 << 20) as f64,
            count as f64 / from_log.as_secs_f64(),
            (2 * count * bytes as u64) >> 20,
            count as f64 / from_home.as_secs_f64(),
        );
    }
}

/// A share of the store's lock keeps the volume locked after the store
/// is gone, also when the store ends on another thread and behind its
/// worker: what an owner needs that has work to do under the lock when
/// the store has ended.
#[test]
fn a_share_of_the_stores_lock_outlives_the_store() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = FsDir::new(tmp.path()).unwrap();
    let store = Store::create(dir.clone(), &config(16, 64), OpenOptions::default()).unwrap();
    let share = store.share_lock().expect("the store holds the lock");
    let served = store.serve().unwrap();
    // The store ends on another thread, by a panic.
    let ended = std::thread::spawn(move || {
        let _served = served;
        panic!("the thread that owns the store panics, as the test asks");
    })
    .join();
    assert!(ended.is_err());
    assert!(matches!(Store::lock_volume(&dir), Err(StoreError::Locked)));
    assert!(matches!(
        Store::open(dir.clone(), OpenOptions::default()),
        Err(StoreError::Locked)
    ));
    drop(share);
    assert!(Store::open(dir, OpenOptions::default()).is_ok());
}

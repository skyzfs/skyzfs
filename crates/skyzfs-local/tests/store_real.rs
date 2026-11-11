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
    let stdout = std::io::stdout();
    let say = |line: String| {
        // One whole line, written at once.
        let mut out = stdout.lock();
        out.write_all(line.as_bytes()).unwrap();
        out.flush().unwrap();
    };
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
/// finds no room stays with the worker, and a read submitted after it
/// waits behind it, neither completed nor failed; when the hold is
/// released both complete, in order, and the data is there.
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
    assert_eq!(store.wait(&mut done, Duration::from_millis(300)), 0);
    store.set_hold(None).unwrap();
    while done.len() < 2 {
        assert!(
            store.wait(&mut done, TIMEOUT) > 0,
            "the held requests did not complete"
        );
    }
    assert_eq!((done[0].id, done[0].result), (RequestId(held), Ok(())));
    assert_eq!((done[1].id, done[1].result), (RequestId(100), Ok(())));
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

/// Process kill, for real (rule R9), and the lock with it: while the
/// child lives the parent cannot open the store, and after the kill it
/// can. A child process serves a writer
/// through the store's worker thread and reports each acknowledged
/// write over a pipe; the parent kills it with SIGKILL at a moment the
/// clock chooses, opens the store, and finds in every block the last
/// write that was acknowledged for it, or the one write that was in
/// flight when the child died. Several rounds on the same store, with
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
        Ok(seed) => seed.parse().unwrap(),
        Err(_) => std::time::SystemTime::now()
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
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        moment = moment
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let kill_after = 1 + (moment >> 33) % 2000;
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut acked = None;
        let mut note = |line: &str| {
            if let Some(Ok(n)) = line.strip_prefix("ack ").map(str::parse::<u64>) {
                acked = Some(n);
            }
        };
        let deadline = Instant::now() + TIMEOUT;
        let mut seen = 0;
        while seen < kill_after && Instant::now() < deadline {
            let Some(Ok(line)) = lines.next() else {
                break;
            };
            note(&line);
            seen += 1;
        }
        // While the child lives the store is its own: one instance per
        // volume, across processes.
        if seen > 0 {
            assert!(
                matches!(
                    Store::open(dir.clone(), options.clone()),
                    Err(StoreError::Locked)
                ),
                "round {round}: the store opened under a live instance"
            );
        }
        child.kill().unwrap();
        // Acknowledgements that were in the pipe when it died count too.
        for line in lines.map_while(Result::ok) {
            note(&line);
        }
        child.wait().unwrap();
        let acked =
            acked.unwrap_or_else(|| panic!("round {round}: the child acknowledged nothing"));
        assert!(
            acked >= next,
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

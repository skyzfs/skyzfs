//! The in-place store on real files: a round trip, and a real
//! `kill -9`.
//!
//! What the kill test shows rests on the device, and on nothing the
//! store does: the store reads and writes by direct I/O (decision D6),
//! so a write is acknowledged once the device has taken it (the
//! `pwrite` on the direct handle has returned), and a process that dies
//! takes nothing of it along. The test cuts no power, and the in-place
//! store promises nothing more than the volatile-cache disk at a power
//! cut (see `skyzfs_local::inplace`).
//!
//! **These tests need a temporary directory on a file system that
//! offers direct I/O with a unit of 4 KiB or less, and whose page cache
//! the kernel can drop and count for a file.** The in-place store's
//! start refuses a file system without such direct I/O, and it drops
//! the home image's cache and checks with `cachestat` that none of it
//! is left, and refuses otherwise, by design: on tmpfs the pages are
//! the file and cannot be dropped, and through an overlay the count
//! sees none. Where the temporary directory is on such a file system,
//! each test is skipped and says why on standard error (`--nocapture`
//! shows it); set `TMPDIR` to run it.
//! Where the variable `CI` is set, as GitHub sets it on every runner, a
//! skip is a failure instead: a run there is expected to test, and a
//! skip reads as a pass.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use skyzfs_blk::{Base, BlockTarget, Buf, Completion, Op, Request, RequestId};
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::inplace::{InPlace, IoMode};
use skyzfs_local::store::{OpenOptions, Store, StoreConfig, StoreError, HOME_NAME};

const BLOCK: u64 = 4096;
const TIMEOUT: Duration = Duration::from_secs(60);

/// The seed of the kill test's moments, to repeat a run.
const KILL_SEED: &str = "SKYZFS_KILL_SEED";

type OnFiles = InPlace<FsDir>;

fn config() -> StoreConfig {
    StoreConfig {
        volume: *b"in place on disk",
        size_mib: 16,
        base: Base::Empty,
        align: 64,
        segment_size: 8 << 20,
        write_unit: Identity::system_write_unit(),
    }
}

/// The directory at `path`, whose files count their cached pages as
/// the daemon's do: the in-place store's start refuses without it, and
/// without direct I/O.
fn on_disk(path: &std::path::Path) -> Option<FsDir> {
    let usable =
        the_cache_can_be_dropped_and_counted(path).and_then(|()| direct_io_is_offered(path));
    if let Err(why) = usable {
        let why = format!(
            "{}: {why}; the in-place store refuses to start there by design. Set TMPDIR to \
             a directory whose file system's cache can be dropped and counted",
            path.display()
        );
        assert!(!on_ci(), "not skipped where CI is set: {why}");
        eprintln!("skipped: {why}");
        return None;
    }
    Some(
        FsDir::new(path)
            .unwrap()
            .with_cache_count(skyzfs_ublk_sys::cached_pages),
    )
}

/// Whether this is a CI run (see the module documentation).
fn on_ci() -> bool {
    std::env::var_os("CI").is_some_and(|v| !v.is_empty() && v != "false")
}

/// Whether the directory's file system can do what the in-place store's
/// start needs (see the module documentation), and if not, why not. A
/// count that fails other than as unsupported fails the test.
fn the_cache_can_be_dropped_and_counted(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::FileExt;
    let probe = path.join("cache-probe");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .read(true)
        .write(true)
        .open(&probe)
        .unwrap();
    file.write_all_at(&[7u8; 64 << 10], 0).unwrap();
    file.sync_data().unwrap();
    let mut page = [0u8; 4096];
    file.read_exact_at(&mut page, 0).unwrap();
    let seen = skyzfs_ublk_sys::cached_pages(&file);
    rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::DontNeed).unwrap();
    let left = skyzfs_ublk_sys::cached_pages(&file);
    drop(file);
    std::fs::remove_file(&probe).unwrap();
    match (seen, left) {
        (Err(e), _) if e.kind() == std::io::ErrorKind::Unsupported => {
            Err("the kernel cannot count a file's cached pages (cachestat)".into())
        }
        (Err(e), _) | (Ok(_), Err(e)) => panic!("counting a file's cached pages failed: {e}"),
        (Ok(0), _) => Err("the count does not see a page just read (an overlay?)".into()),
        (Ok(_), Ok(0)) => Ok(()),
        (Ok(_), Ok(n)) => Err(format!("{n} pages stay cached after a drop (tmpfs?)")),
    }
}

/// Whether the directory's file system offers direct I/O with a unit of
/// 4 KiB or less, as the in-place store's start asks, and if not, why
/// not.
fn direct_io_is_offered(path: &std::path::Path) -> Result<(), String> {
    use skyzfs_local::io::{Dir, File};
    let dir = FsDir::new(path).unwrap();
    let probe = dir.create("direct-probe").unwrap();
    let align = probe.direct_align();
    drop(probe);
    std::fs::remove_file(path.join("direct-probe")).unwrap();
    match align {
        Ok(align) if align.offset > 0 && BLOCK.is_multiple_of(u64::from(align.offset)) => Ok(()),
        Ok(align) => Err(format!(
            "its unit for direct I/O is {} bytes, above 4 KiB",
            align.offset
        )),
        Err(e) => Err(format!("it offers no direct I/O ({e})")),
    }
}

/// The log store's open, then the in-place store, with `io`.
fn start_io(dir: &FsDir, io: IoMode) -> Result<OnFiles, StoreError> {
    let mut options = OpenOptions::default();
    options.in_place_io = io;
    let (store, _) = Store::open(dir.clone(), options)?;
    Ok(store.into_in_place(None).expect("the in-place store"))
}

/// The log store's open, then the in-place store.
fn start(dir: &FsDir) -> Result<OnFiles, StoreError> {
    let (store, _) = Store::open(dir.clone(), OpenOptions::default())?;
    Ok(store.into_in_place(None).expect("the in-place store"))
}

/// Submit one request and wait for it.
fn run(store: &mut OnFiles, op: Op) -> Completion {
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

fn write(store: &mut OnFiles, block: u64, data: Buf) {
    let done = run(
        store,
        Op::Write {
            offset: block * BLOCK,
            data,
        },
    );
    assert_eq!(done.result, Ok(()));
}

fn read(store: &mut OnFiles, block: u64, count: usize) -> Buf {
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

fn flush(store: &mut OnFiles) {
    assert_eq!(run(store, Op::Flush).result, Ok(()));
}

/// Real files, round trip: a write, a write-zeroes and a discard, a
/// flush on the file's helper thread, and a start again; and back to
/// the log store, which reads the same.
#[test]
fn the_in_place_store_round_trips_on_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        return;
    };
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    let mut store = start(&dir).unwrap();
    write(&mut store, 0, Buf::filled(4, 0x11));
    assert_eq!(
        run(
            &mut store,
            Op::WriteZeroes {
                offset: BLOCK,
                len: BLOCK,
            }
        )
        .result,
        Ok(())
    );
    assert_eq!(
        run(
            &mut store,
            Op::Discard {
                offset: 3 * BLOCK,
                len: BLOCK,
            }
        )
        .result,
        Ok(())
    );
    flush(&mut store);
    assert!(store.counts().syncs >= 2, "the flush synced");
    let expect = |got: &[u8]| {
        assert_eq!(&got[..BLOCK as usize], &[0x11; BLOCK as usize][..]);
        assert_eq!(
            &got[BLOCK as usize..2 * BLOCK as usize],
            &[0; BLOCK as usize][..]
        );
        assert_eq!(
            &got[2 * BLOCK as usize..3 * BLOCK as usize],
            &[0x11; BLOCK as usize][..]
        );
        // A discarded block keeps its content (rule R6 allows it).
        assert_eq!(&got[3 * BLOCK as usize..], &[0x11; BLOCK as usize][..]);
    };
    expect(&read(&mut store, 0, 4));
    drop(store);
    let mut store = start(&dir).unwrap();
    expect(&read(&mut store, 0, 4));
    drop(store);
    let (store, _) = Store::open(dir.clone(), OpenOptions::default()).unwrap();
    let mut store = store.serve().unwrap();
    store
        .submit(Request {
            id: RequestId(0),
            op: Op::Read {
                offset: 0,
                buf: Buf::filled(4, 0xEE),
            },
        })
        .unwrap();
    let mut done = Vec::new();
    assert_eq!(store.wait(&mut done, TIMEOUT), 1);
    assert_eq!(done[0].result, Ok(()));
    expect(done[0].buf.as_deref().unwrap());
}

/// Direct I/O on real files, on four threads and on the default eight,
/// and with the image written with zeros first (`fill_home`).
/// Sixty-four writes of two blocks each go out together, then a flush,
/// and every block reads back, through a start on four threads and
/// through one with the default.
#[test]
fn direct_io_on_threads_round_trips_on_real_files() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        return;
    };
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    skyzfs_local::store::fill_home(&dir).unwrap();
    let io = IoMode::with_threads(4).unwrap();
    let mut store = start_io(&dir, io).unwrap();
    let pattern = |n: u64| (n as u8).wrapping_mul(37) | 1;
    for n in 0..64u64 {
        store
            .submit(Request {
                id: RequestId(n),
                op: Op::Write {
                    offset: n * 2 * BLOCK,
                    data: Buf::filled(2, pattern(n)),
                },
            })
            .unwrap();
    }
    let mut done = Vec::new();
    while done.len() < 64 {
        store.wait(&mut done, TIMEOUT);
    }
    assert!(done.iter().all(|c| c.result == Ok(())), "{done:?}");
    flush(&mut store);
    let check = |store: &mut OnFiles| {
        for n in 0..64u64 {
            let got = read(store, n * 2, 2);
            assert!(got.iter().all(|b| *b == pattern(n)), "blocks {}..", n * 2);
        }
    };
    check(&mut store);
    drop(store);
    check(&mut start_io(&dir, io).unwrap());
    check(&mut start(&dir).unwrap());
}

/// Every start serves where the image begins with blocks never
/// written, as a pool's label does (8 KiB that nothing wrote, then the
/// boot block). A read of such a block is answered at once while the
/// kernel's read-ahead of the blocks after it is still under I/O, and a
/// drop leaves pages under I/O; a start that read a block just before
/// its drop could find them still there and refuse. The image's cache
/// is dropped before each start, as after a boot: otherwise what one
/// start leaves in the cache would answer the next one's reads without
/// a read-ahead, and a read before the drop would go unnoticed.
#[test]
fn every_start_serves_where_the_image_begins_with_a_hole() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        return;
    };
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    let mut store = start(&dir).unwrap();
    write(&mut store, 2, Buf::filled(2, 0x22));
    flush(&mut store);
    drop(store);
    let home = std::fs::File::open(tmp.path().join(HOME_NAME)).unwrap();
    for n in 0..200 {
        rustix::fs::fadvise(&home, 0, None, rustix::fs::Advice::DontNeed).unwrap();
        let (store, _) = Store::open(dir.clone(), OpenOptions::default()).unwrap();
        if let Err(e) = store.into_in_place(None) {
            panic!("start {n} refused: {e}");
        }
    }
}

const CHILD_DIR: &str = "SKYZFS_INPLACE_KILL_TEST_DIR";
const CHILD_FROM: &str = "SKYZFS_INPLACE_KILL_TEST_FROM";
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
/// `acknowledged_writes_survive_kill_9_in_place`. Serves a writer and
/// reports every acknowledgement and every completed flush, until it
/// is killed.
#[test]
fn inplace_kill_test_child() {
    let Some(path) = std::env::var_os(CHILD_DIR) else {
        return;
    };
    let from: u64 = std::env::var(CHILD_FROM).unwrap().parse().unwrap();
    let Some(dir) = on_disk(std::path::Path::new(&path)) else {
        return;
    };
    let mut store = start(&dir).unwrap();
    // The reports go to standard error, which the test harness does not
    // write to (as in `store_real.rs`).
    let stderr = std::io::stderr();
    let say = |line: String| {
        let mut out = stderr.lock();
        out.write_all(line.as_bytes()).unwrap();
        out.flush().unwrap();
    };
    say("open\n".to_string());
    for n in from..u64::MAX {
        let (block, _, data) = kill_write(n);
        write(&mut store, block, data);
        say(format!("ack {n}\n"));
        if n % 37 == 0 {
            flush(&mut store);
            say(format!("flushed {n}\n"));
        }
    }
}

/// Process kill, for real (rule R9), and the lock with it, as the log
/// store's test in `store_real.rs` does: a child process serves a
/// writer from the in-place store and reports each acknowledged write;
/// the parent kills it with SIGKILL at a moment drawn from a printed
/// seed, starts the store, and finds in every block the last write
/// that was acknowledged for it, or the one write that was in flight
/// when the child died.
///
/// This rests on the page cache: the kernel keeps what the child wrote
/// to the file when the child dies, synced or not. And unlike the log
/// store, the write in flight may be there in part: the in-place store
/// writes its blocks straight to the file, and a process killed in the
/// middle of a `pwrite` of several blocks leaves the blocks it had
/// copied. Each block is whole: the kernel copies a page at a time and
/// looks for a fatal signal only between pages.
#[test]
fn acknowledged_writes_survive_kill_9_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let Some(dir) = on_disk(tmp.path()) else {
        return;
    };
    drop(Store::create(dir.clone(), &config(), OpenOptions::default()).unwrap());
    let exe = std::env::current_exe().unwrap();
    let mut held = vec![None::<u64>; KILL_BLOCKS as usize];
    let mut next = 1u64;
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
                "inplace_kill_test_child",
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
        let kill_after = (moment >> 33) % 2000;
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut acked = None;
        let mut note = |line: &str| {
            if let Some(Ok(n)) = line.strip_prefix("ack ").map(str::parse::<u64>) {
                acked = Some(n);
            }
        };
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match lines.next() {
                Some(Ok(line)) if line == "open" => break,
                Some(Ok(_)) => {}
                _ => panic!("round {round}: the child ended before it had opened the store"),
            }
            assert!(
                Instant::now() < deadline,
                "round {round}: the child did not open the store"
            );
        }
        // The in-place store holds the volume's lock as the log store
        // does.
        assert!(
            matches!(
                Store::open(dir.clone(), OpenOptions::default()),
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
        for line in lines.map_while(Result::ok) {
            note(&line);
        }
        child.wait().unwrap();
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
        let pending = acked + 1;
        let (pending_block, pending_count, _) = kill_write(pending);

        let mut store = start(&dir).unwrap();
        let got = read(&mut store, 0, KILL_BLOCKS as usize);
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
        drop(store);
        next = pending + 1;
    }
}

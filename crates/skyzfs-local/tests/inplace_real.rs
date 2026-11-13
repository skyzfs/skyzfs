//! The in-place store on real files: a round trip, and a real
//! `kill -9`.
//!
//! What the kill test shows rests on the page cache, and on nothing
//! the store does: a write is acknowledged once it is in the file
//! (`pwrite` has returned), and the kernel keeps what a process wrote
//! to a file when the process dies. The test cuts no power, and the
//! in-place store promises nothing more than the volatile-cache disk
//! at a power cut (see `skyzfs_local::inplace`).
//!
//! **The temporary directory must be on a file system whose page cache
//! the kernel can drop and count for a file** (ext4, xfs). Every start
//! of the in-place store drops the home image's cache and checks with
//! `cachestat` that none of it is left, and refuses otherwise: on tmpfs
//! the pages are the file and cannot be dropped, and through an overlay
//! the count sees none. Each test says so first where that is the case;
//! set `TMPDIR` to a directory on such a file system.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use skyzfs_blk::{Base, BlockTarget, Buf, Completion, Op, Request, RequestId};
use skyzfs_local::format::Identity;
use skyzfs_local::fs::FsDir;
use skyzfs_local::inplace::InPlace;
use skyzfs_local::store::{OpenOptions, Store, StoreConfig, StoreError};

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
/// the daemon's do: the in-place store's start refuses without it.
fn on_disk(path: &std::path::Path) -> FsDir {
    the_cache_can_be_dropped_and_counted(path);
    FsDir::new(path)
        .unwrap()
        .with_cache_count(skyzfs_ublk_sys::cached_pages)
}

/// Fail with a sentence where the directory's file system cannot do
/// what the in-place store's start needs (see the module documentation),
/// so that the start's refusal is not taken for a defect of the store.
fn the_cache_can_be_dropped_and_counted(path: &std::path::Path) {
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
    let seen = skyzfs_ublk_sys::cached_pages(&file).unwrap_or(0);
    assert!(
        seen > 0,
        "{}: the kernel's count does not see a page just read (an overlay file system, or a \
         kernel before 6.5): the in-place store refuses to start here by design; set TMPDIR \
         to a directory on ext4 or xfs",
        path.display()
    );
    rustix::fs::fadvise(&file, 0, None, rustix::fs::Advice::DontNeed).unwrap();
    let left = skyzfs_ublk_sys::cached_pages(&file).unwrap_or(u64::MAX);
    assert_eq!(
        left,
        0,
        "{}: pages stay cached after a drop (tmpfs keeps a file in its pages): the in-place \
         store refuses to start here by design; set TMPDIR to a directory on ext4 or xfs",
        path.display()
    );
    drop(file);
    std::fs::remove_file(&probe).unwrap();
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
    let dir = on_disk(tmp.path());
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
    let dir = on_disk(std::path::Path::new(&path));
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
    let dir = on_disk(tmp.path());
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

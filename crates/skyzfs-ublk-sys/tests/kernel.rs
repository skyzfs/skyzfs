//! The binding's [`Control`] and queue on a real kernel: what the
//! lifecycle's tests assume of a control interface, asked of the
//! driver. Ignored by default: they need root and the ublk driver.
//!
//! ```text
//! sudo -E cargo test -p skyzfs-ublk-sys --features control-test-program --test kernel -- --ignored --test-threads 1 --nocapture
//! ```
//!
//! The feature builds the control test program and the test of it;
//! without it the other tests run and that one is not there.
//!
//! Each test prints one line `control_<property>\tPASS` or
//! `control_<property>\tFAIL: <why>` on standard output, for the rig,
//! and fails with the same words. They use device numbers from
//! `SKYZFS_KERNEL_TEST_BASE` (default 200) on, one per test, and delete
//! what they add and nothing else: a test whose number is not free
//! when it begins fails and leaves the device there. These are the cases that need no stranger; the ones
//! that need a device of another server, or one held open by another
//! process, are the rig's.
#![cfg(target_os = "linux")]

use std::fs::OpenOptions;
use std::os::unix::fs::{FileExt, OpenOptionsExt};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use skyzfs_blk::{Buf, BLOCK_SIZE};
use skyzfs_ublk::lifecycle::{AddError, Control, DeviceInfo, DeviceState, Shape};
use skyzfs_ublk::pump::{Fetched, Queue, Wake as Woken};
use skyzfs_ublk::translate::op;
use skyzfs_ublk_sys::control::{block_device, Kernel};
use skyzfs_ublk_sys::queue::{Device, Open};
use skyzfs_ublk_sys::{Wake, FLAGS};

/// The test allocator, from the crate's sources: in test builds only.
#[path = "../src/watch.rs"]
mod watch;

/// The allocator of this binary counts the deallocations of memory it
/// is told to watch.
#[global_allocator]
static WATCHING: watch::Watching = watch::Watching;

const SIZE: u64 = 64 << 20;
const DEPTH: u16 = 8;

fn number(offset: u32) -> u32 {
    let base = std::env::var("SKYZFS_KERNEL_TEST_BASE")
        .ok()
        .and_then(|b| b.parse().ok())
        .unwrap_or(200);
    base + offset
}

fn shape() -> Shape {
    Shape {
        size: SIZE,
        queues: 1,
        depth: DEPTH,
        flags: FLAGS,
    }
}

/// Run `check`, print its line, and fail with its words.
fn property(name: &str, check: impl FnOnce() -> Result<(), String>) {
    match check() {
        Ok(()) => println!("control_{name}\tPASS"),
        Err(why) => {
            println!("control_{name}\tFAIL: {why}");
            panic!("{name}: {why}");
        }
    }
}

fn kernel() -> Result<Kernel, String> {
    Kernel::new().map_err(|e| e.to_string())
}

fn info(kernel: &mut Kernel, n: u32) -> Result<Option<DeviceInfo>, String> {
    kernel.info(n).map_err(|e| format!("info {n}: {e}"))
}

/// Ask until `want` holds, for up to 2 s.
fn until(
    kernel: &mut Kernel,
    n: u32,
    what: &str,
    want: impl Fn(&Option<DeviceInfo>) -> bool,
) -> Result<Option<DeviceInfo>, String> {
    let began = Instant::now();
    loop {
        let now = info(kernel, n)?;
        if want(&now) {
            return Ok(now);
        }
        if began.elapsed() > Duration::from_secs(2) {
            return Err(format!("device {n}: not {what} within 2 s: {now:?}"));
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The number `n` for the test whose property is `name`: free, or the
/// test fails here and deletes nothing. The numbers from
/// `SKYZFS_KERNEL_TEST_BASE` on are the tests' only by the word of
/// whoever runs them, so a device found under one before the test has
/// added anything is taken to be somebody else's, also when it is the
/// leftover of a test run that was killed: it is for a person to
/// delete, not for a test.
fn claim(n: u32, name: &str) {
    let found = kernel().and_then(|mut kernel| info(&mut kernel, n));
    if !matches!(found, Ok(None)) {
        property(name, || {
            Err(format!(
                "device {n} is not free before the test ({found:?}): it is not this test's \
                 and was not deleted. Delete it, or set SKYZFS_KERNEL_TEST_BASE to free numbers"
            ))
        });
    }
}

/// Delete what the test added under `n`, which [`claim`] found free
/// before, and wait until it is gone.
fn clean(n: u32) {
    if let Ok(mut kernel) = Kernel::new() {
        let _ = kernel.delete(n);
        let _ = until(&mut kernel, n, "gone", Option::is_none);
    }
}

#[test]
#[ignore = "needs root and the ublk driver"]
fn the_driver_can_reissue() {
    property("can_reissue", || match kernel()?.can_reissue() {
        Ok(true) => Ok(()),
        other => Err(format!("{other:?}")),
    });
}

#[test]
#[ignore = "needs root and the ublk driver"]
fn a_free_number_has_nothing() {
    let n = number(0);
    claim(n, "lookup_free_is_none");
    property("lookup_free_is_none", || match info(&mut kernel()?, n)? {
        None => Ok(()),
        Some(found) => Err(format!("device {n} reported: {found:?}")),
    });
}

/// An added device is dead, has the asked queues, depth and flags (the
/// flags under the mask), and size zero until it is started; an add
/// under its number is "taken" and changes nothing; a delete returns,
/// and the number is free again.
#[test]
#[ignore = "needs root and the ublk driver"]
fn add_reports_what_was_asked_a_second_add_is_taken_and_delete_frees() {
    let n = number(1);
    claim(n, "add_taken_delete");
    let result = (|| {
        let mut kernel = kernel()?;
        kernel
            .add(n, &shape())
            .map_err(|e| format!("add {n}: {e:?}"))?;
        let added = info(&mut kernel, n)?.ok_or("nothing after the add")?;
        let want = DeviceInfo {
            state: DeviceState::Dead,
            shape: Shape { size: 0, ..shape() },
        };
        if added != want {
            return Err(format!("after the add: {added:?}, wanted {want:?}"));
        }
        let other = Shape {
            depth: DEPTH * 2,
            ..shape()
        };
        match kernel.add(n, &other) {
            Err(AddError::Taken) => {}
            other => return Err(format!("a second add under {n}: {other:?}")),
        }
        let after = info(&mut kernel, n)?;
        if after != Some(added) {
            return Err(format!("the second add changed the device: {after:?}"));
        }
        let began = Instant::now();
        kernel.delete(n).map_err(|e| format!("delete: {e}"))?;
        let took = began.elapsed();
        if took > Duration::from_secs(1) {
            return Err(format!("the delete took {took:?}"));
        }
        until(&mut kernel, n, "gone after the delete", Option::is_none)?;
        Ok(())
    })();
    clean(n);
    property("add_taken_delete", || result);
}

/// Answer what the kernel asks of a device of zeros until a stop:
/// enough for the partition scan of a start.
fn zeros(mut queue: impl Queue) -> u64 {
    let mut fetched: Vec<Fetched> = Vec::new();
    let mut answered = 0;
    loop {
        fetched.clear();
        let Ok(woken) = queue.wait(&mut fetched) else {
            return answered;
        };
        for f in fetched.drain(..) {
            let bytes = u64::from(f.desc.nr_sectors) * 512;
            let carries = matches!(f.desc.op(), op::READ | op::WRITE);
            let buf = carries.then(|| {
                let mut buf = queue
                    .take_buf(f.tag, (bytes / BLOCK_SIZE) as usize)
                    .expect("the tag's buffer");
                if f.desc.op() == op::READ {
                    buf.fill(0);
                }
                buf
            });
            queue.commit(f.tag, bytes as i32, buf);
            answered += 1;
        }
        if woken == Woken::Stop {
            let _ = queue.flush();
            return answered;
        }
    }
}

/// The library's control object made with no flag sets the parameters
/// of a device the binding added and starts it; the device is live with
/// its size; nothing holds it exclusively; an orderly end of the queue
/// leaves it waiting for a server, and dropping the library's objects
/// deletes nothing.
#[test]
#[ignore = "needs root and the ublk driver"]
fn a_device_added_by_the_binding_is_started_and_outlives_its_server() {
    let n = number(2);
    claim(n, "start_no_flag_and_keep");
    let result = (|| {
        let mut kernel = kernel()?;
        kernel.add(n, &shape()).map_err(|e| format!("add: {e:?}"))?;
        let device = Device::open(&Open {
            number: n,
            size: SIZE,
            take_over: false,
        })
        .map_err(|e| format!("open: {e}"))?;
        let wake = Wake::new().map_err(|e| e.to_string())?;
        let serving = device
            .serve(&wake, false, |queue| zeros(queue))
            .map_err(|e| format!("serve: {e}"))?;
        let stopper = serving.stopper();
        if let Err(e) = device.start() {
            stopper.stop();
            let _ = serving.join();
            return Err(format!("start through the no-flag object: {e}"));
        }
        let live = until(&mut kernel, n, "live", |i| {
            i.is_some_and(|i| i.state == DeviceState::Live)
        })?;
        let want = shape();
        if live.map(|i| i.shape) != Some(want) {
            return Err(format!("live with {live:?}, wanted {want:?}"));
        }
        let holds = kernel.client_holds(n);
        // Hold it exclusively ourselves: then it is held.
        let held = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_EXCL)
            .open(block_device(n))
            .map_err(|e| format!("an exclusive open: {e}"))?;
        let holds_while_held = kernel.client_holds(n);
        drop(held);
        stopper.stop();
        let answered = serving.join().map_err(|e| e.to_string())?;
        let copies = stopper.first_write_copies();
        drop(device);
        let after = until(&mut kernel, n, "waiting after its server ended", |i| {
            i.is_some_and(|i| i.state == DeviceState::Waiting)
        })?;
        if holds.is_some() || holds_while_held != Some(true) {
            return Err(format!(
                "client_holds: {holds:?} with no holder, {holds_while_held:?} while held"
            ));
        }
        if copies > u64::from(DEPTH) {
            return Err(format!("{copies} first-write copies at depth {DEPTH}"));
        }
        println!(
            "control_started_detail\t{answered} requests answered, {copies} first-write copies, then {after:?}"
        );
        if std::path::Path::new(&format!("/run/ublksrvd/{n:04}.json")).exists() {
            return Err("the library wrote its file under /run/ublksrvd".into());
        }
        Ok(())
    })();
    clean(n);
    property("start_no_flag_and_keep", || result);
}

/// What a [`memory_gathering`] server counted.
#[derive(Default)]
struct Seen {
    /// The write requests it was handed.
    handed: std::sync::atomic::AtomicU64,
    /// The tags whose first request of this life was a write: what the
    /// queue's count of first-write copies must equal.
    first_writes: std::sync::atomic::AtomicU64,
    /// The most writes a gathering server held at once before it
    /// answered them: the number it was asked to gather, if the test
    /// did send that many together.
    gathered: std::sync::atomic::AtomicU64,
}

/// A server that keeps the device's content in memory, shared between
/// the lives of a device: what one life was written, the next reads.
/// Returns the longest write it was handed, in bytes.
///
/// It counts in `seen`, and answers no write until `gather` writes are
/// in its hands at once: every one of them then has a tag of its own. Reads are answered as they come (something
/// else may read a device that appears, and a tag whose first request
/// was such a read has no first write). If a wait brings nothing while
/// it gathers (the queue's idle time-out, 20 s), it answers what it
/// has, so that a test that did not send enough does not hang; how
/// many it held is in `seen.gathered`, and it is for the test to fail
/// on fewer than it asked for.
fn memory_gathering(
    queue: &mut impl Queue,
    content: Arc<Mutex<Vec<u8>>>,
    seen: &Seen,
    gather: usize,
) -> u64 {
    use std::sync::atomic::Ordering;
    let mut fetched: Vec<Fetched> = Vec::new();
    let mut gathered: Vec<Fetched> = Vec::new();
    let mut gathering = gather > 0;
    let mut used = vec![false; usize::from(queue.depth())];
    let mut longest = 0;
    loop {
        fetched.clear();
        let Ok(woken) = queue.wait(&mut fetched) else {
            return longest;
        };
        if gathering {
            let brought = !fetched.is_empty();
            let (writes, others): (Vec<_>, Vec<_>) =
                fetched.drain(..).partition(|f| f.desc.op() == op::WRITE);
            gathered.extend(writes);
            fetched.extend(others);
            seen.gathered
                .fetch_max(gathered.len() as u64, Ordering::SeqCst);
            if gathered.len() >= gather || !brought || woken == Woken::Stop {
                gathering = false;
                fetched.append(&mut gathered);
            }
        }
        for f in fetched.drain(..) {
            let at = (f.desc.start_sector * 512) as usize;
            let bytes = f.desc.nr_sectors as usize * 512;
            let mut content = content.lock().expect("the content");
            let first = !std::mem::replace(&mut used[usize::from(f.tag)], true);
            let buf = match f.desc.op() {
                op::READ => {
                    let mut buf = queue
                        .take_buf(f.tag, bytes / BLOCK_SIZE as usize)
                        .expect("the tag's buffer");
                    buf.copy_from_slice(&content[at..at + bytes]);
                    Some(buf)
                }
                op::WRITE => {
                    let buf = queue
                        .take_buf(f.tag, bytes / BLOCK_SIZE as usize)
                        .expect("the tag's buffer");
                    content[at..at + bytes].copy_from_slice(&buf);
                    longest = longest.max(bytes as u64);
                    seen.handed.fetch_add(1, Ordering::SeqCst);
                    if first {
                        seen.first_writes.fetch_add(1, Ordering::SeqCst);
                    }
                    Some(buf)
                }
                _ => None,
            };
            queue.commit(f.tag, bytes as i32, buf);
        }
        if woken == Woken::Stop {
            // As the pump does at the end of a stop: the commits above
            // have no wait behind them that would send them.
            let _ = queue.flush();
            return longest;
        }
    }
}

/// One life of a device over `content`: opened, served, started.
struct Life {
    serving: skyzfs_ublk_sys::queue::Serving<u64>,
    seen: Arc<Seen>,
    _device: Device,
}

fn life(n: u32, take_over: bool, content: &Arc<Mutex<Vec<u8>>>) -> Result<Life, String> {
    life_gathering(n, take_over, content, 0)
}

/// [`life`] over a server that gathers its first `gather` writes: see
/// [`memory_gathering`].
fn life_gathering(
    n: u32,
    take_over: bool,
    content: &Arc<Mutex<Vec<u8>>>,
    gather: usize,
) -> Result<Life, String> {
    let device = Device::open(&Open {
        number: n,
        size: SIZE,
        take_over,
    })
    .map_err(|e| format!("open: {e}"))?;
    let wake = Wake::new().map_err(|e| e.to_string())?;
    let content = content.clone();
    let seen = Arc::new(Seen::default());
    let serving = device
        .serve(&wake, false, {
            let seen = seen.clone();
            move |mut queue| memory_gathering(&mut queue, content, &seen, gather)
        })
        .map_err(|e| format!("serve: {e}"))?;
    if let Err(e) = device.start() {
        serving.stopper().stop();
        let _ = serving.join();
        return Err(format!(
            "{}: {e}",
            if take_over {
                "end of recovery"
            } else {
                "start"
            }
        ));
    }
    Ok(Life {
        serving,
        seen,
        _device: device,
    })
}

impl Life {
    /// The most writes the server held at once while it gathered.
    fn gathered(&self) -> u64 {
        self.seen.gathered.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// End the life in order; the longest write, and the first-write
    /// copies. The queue's count of copies must be the number of tags
    /// whose first request the server saw to be a write, exactly.
    fn end(self) -> Result<(u64, u64), String> {
        let stopper = self.serving.stopper();
        stopper.stop();
        let longest = self.serving.join().map_err(|e| e.to_string())?;
        let copies = stopper.first_write_copies();
        let first_writes = self
            .seen
            .first_writes
            .load(std::sync::atomic::Ordering::SeqCst);
        if copies != first_writes {
            return Err(format!(
                "the queue counted {copies} first-write copies and the server saw \
                 {first_writes} tags begin with a write"
            ));
        }
        Ok((longest, copies))
    }
}

/// What the write number `round` puts at the device's byte `at`, for
/// `bytes` bytes: every eight bytes are a mix of their own place on
/// the device and of the round. Data that arrives at another place, in
/// another order within a request, or from an earlier round does not
/// compare equal.
fn pattern(bytes: usize, at: u64, round: u64) -> Buf {
    let mut buf = Buf::zeroed(bytes / BLOCK_SIZE as usize);
    for (i, word) in buf.as_chunks_mut::<8>().0.iter_mut().enumerate() {
        let mut x = (at + 8 * i as u64) ^ round.wrapping_mul(0x9e37_79b9_7f4a_7c15);
        x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        *word = (x ^ (x >> 31)).to_le_bytes();
    }
    buf
}

/// Write the round's pattern at `at` and read it back.
fn write_and_read_back(
    file: &std::fs::File,
    bytes: usize,
    at: u64,
    round: u64,
    what: &str,
) -> Result<(), String> {
    let data = pattern(bytes, at, round);
    file.write_all_at(&data, at)
        .map_err(|e| format!("{what}: writing {bytes} bytes at {at}: {e}"))?;
    written_reads_back(file, at, &data, what)
}

fn direct(n: u32) -> Result<std::fs::File, String> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_DIRECT)
        .open(block_device(n))
        .map_err(|e| format!("opening {} with direct I/O: {e}", block_device(n)))
}

fn written_reads_back(file: &std::fs::File, at: u64, want: &Buf, what: &str) -> Result<(), String> {
    let mut got = Buf::zeroed(want.blocks());
    file.read_exact_at(&mut got, at)
        .map_err(|e| format!("reading {what}: {e}"))?;
    if got != *want {
        let first = got.iter().zip(want.iter()).position(|(a, b)| a != b);
        return Err(format!("{what} read back differs, first at byte {first:?}"));
    }
    Ok(())
}

/// The buffers across a take-over, by name: writes and reads of 4 KiB
/// and of 1 MiB come back as written in the first life (where each
/// tag's first request arrives in the library's buffer and every later
/// one in the tag's own) and across a take-over. Every write's data
/// depends on its place on the device and on its round, so data that
/// lands elsewhere or is left from an earlier round is seen. A 1 MiB
/// write arrives as one request.
///
/// The first requests of the life after the take-over are `depth`
/// writes of 1 MiB sent together, which the server holds until all are
/// in its hands, so that all tags hold a first write at once. In every
/// life the queue's count of first-write copies must equal the number
/// of tags the server saw begin with a write: `depth` after the
/// take-over, unless something else read the device first (the detail
/// line gives the count).
///
/// The first life ends in order and not by a kill: the test is its own
/// server. The device waits for a server either way, and the take-over
/// is the same begin and end of a recovery.
#[test]
#[ignore = "needs root and the ublk driver"]
fn data_of_4k_and_1m_is_whole_in_one_life_and_across_a_take_over() {
    let n = number(3);
    claim(n, "buffers_4k_1m_across_take_over");
    const MIB: usize = 1 << 20;
    let result = (|| {
        let mut kernel = kernel()?;
        kernel.add(n, &shape()).map_err(|e| format!("add: {e:?}"))?;
        let content = Arc::new(Mutex::new(vec![0u8; SIZE as usize]));
        let first = life(n, false, &content)?;
        let mut round = 0u64;
        let mut left_small = pattern(4096, 0, 0);
        {
            let file = direct(n)?;
            // Three times round the tags, so that tags are used after
            // their first commit too, and the same places again with
            // another round's data.
            for turn in 0..3 * u64::from(DEPTH) {
                round += 1;
                let at = (turn % 8) * 8192;
                write_and_read_back(&file, 4096, at, round, "4 KiB")?;
                if at == 0 {
                    left_small = pattern(4096, 0, round);
                }
            }
            for at in [8 << 20, 16 << 20, 8 << 20] {
                round += 1;
                write_and_read_back(&file, MIB, at, round, "1 MiB")?;
            }
        }
        // What the first life leaves at the places the second reads.
        let (left_large_8, left_large_16) = (
            pattern(MIB, 8 << 20, round),
            pattern(MIB, 16 << 20, round - 1),
        );
        let (longest, copies) = first.end()?;
        if longest != MIB as u64 {
            return Err(format!("the longest write handed over was {longest} bytes"));
        }
        if copies == 0 || copies > u64::from(DEPTH) {
            return Err(format!("{copies} first-write copies in the first life"));
        }
        until(&mut kernel, n, "waiting", |i| {
            i.is_some_and(|i| i.state == DeviceState::Waiting)
        })?;

        kernel
            .begin_recovery(n)
            .map_err(|e| format!("begin of recovery: {e}"))?;
        let second = life_gathering(n, true, &content, usize::from(DEPTH))?;
        {
            // The first requests of this life: one write of 1 MiB per
            // tag, all in flight together.
            let together = Arc::new(std::sync::Barrier::new(usize::from(DEPTH)));
            let writers: Vec<_> = (0..u64::from(DEPTH))
                .map(|w| {
                    let together = together.clone();
                    std::thread::spawn(move || -> Result<(), String> {
                        let file = direct(n)?;
                        let at = (24 + w) << 20;
                        let data = pattern(MIB, at, 1000 + w);
                        together.wait();
                        file.write_all_at(&data, at)
                            .map_err(|e| format!("a first write at {at}: {e}"))
                    })
                })
                .collect();
            for writer in writers {
                writer
                    .join()
                    .map_err(|_| "a writer panicked".to_owned())??;
            }
            let file = direct(n)?;
            for w in 0..u64::from(DEPTH) {
                let at = (24 + w) << 20;
                written_reads_back(
                    &file,
                    at,
                    &pattern(MIB, at, 1000 + w),
                    "a first write of the life after the take-over",
                )?;
            }
            // What the first life was written.
            written_reads_back(&file, 8 << 20, &left_large_8, "1 MiB of the first life")?;
            written_reads_back(&file, 16 << 20, &left_large_16, "1 MiB of the first life")?;
            written_reads_back(&file, 0, &left_small, "4 KiB of the first life")?;
            for turn in 0..2 * u64::from(DEPTH) {
                round += 1;
                let at = (40 << 20) + (turn % 5) * 8192;
                write_and_read_back(&file, 4096, at, round, "4 KiB after the take-over")?;
            }
            round += 1;
            write_and_read_back(&file, MIB, 16 << 20, round, "1 MiB after the take-over")?;
        }
        // The server did hold `depth` writes at once: without that the
        // rest of this life shows nothing about first writes sent
        // together.
        let gathered = second.gathered();
        if gathered != u64::from(DEPTH) {
            return Err(format!(
                "the server held {gathered} writes at once after the take-over, not the \
                 {DEPTH} that were sent together"
            ));
        }
        // `end` has compared the copies with the tags the server saw
        // begin with a write. All of them did, unless something else
        // read the device first (udev, when a device appears): such a
        // tag has no first write, and the line below says how many.
        let (_, copies2) = second.end()?;
        if copies2 == 0 || copies2 > u64::from(DEPTH) {
            return Err(format!("{copies2} first-write copies after the take-over"));
        }
        println!(
            "control_buffers_detail\tfirst-write copies {copies} in the first life and {copies2} after the take-over, depth {DEPTH}; longest write {longest} bytes"
        );
        Ok(())
    })();
    clean(n);
    property("buffers_4k_1m_across_take_over", || result);
}

/// A take-over where a file of the library's name is already under
/// `/run/ublksrvd` for the number, left by some other server: one of
/// the shape such a server writes and one of garbage, under both ways
/// of writing the number. The take-over works, and the files are as
/// they were: this daemon neither believes, changes nor removes them.
/// Each life that was taken over reads what the life before wrote and
/// has a write of its own read back.
#[test]
#[ignore = "needs root and the ublk driver"]
fn a_take_over_works_beside_a_stale_file_of_another_server_and_leaves_it() {
    let n = number(4);
    claim(n, "take_over_beside_stale_file");
    let dir = std::path::Path::new("/run/ublksrvd");
    let made_dir = !dir.exists();
    let files = [
        (
            dir.join(format!("{n:04}.json")),
            format!(
                "{{\"dev_info\":{{\"dev_id\":{n},\"nr_hw_queues\":4,\"queue_depth\":128,\"state\":1}},\"queues\":{{\"0\":{{\"qid\":0,\"tid\":1}}}},\"target\":{{\"dev_size\":1,\"name\":\"loop\",\"type\":1}}}}"
            ),
        ),
        (dir.join(format!("{n}.json")), "not json at all {".to_string()),
        (dir.join(format!("{n}.pid")), "1\n".to_string()),
    ];
    let result = (|| {
        let mut kernel = kernel()?;
        kernel.add(n, &shape()).map_err(|e| format!("add: {e:?}"))?;
        let content = Arc::new(Mutex::new(vec![0u8; SIZE as usize]));
        let first = life(n, false, &content)?;
        write_and_read_back(&direct(n)?, 8192, 4096, 1, "in the first life")?;
        first.end()?;
        until(&mut kernel, n, "waiting", |i| {
            i.is_some_and(|i| i.state == DeviceState::Waiting)
        })?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for (path, text) in &files {
            std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))?;
        }
        for garbage_in_the_librarys_name in [false, true] {
            if garbage_in_the_librarys_name {
                std::fs::write(&files[0].0, &files[1].1).map_err(|e| e.to_string())?;
            }
            kernel
                .begin_recovery(n)
                .map_err(|e| format!("begin of recovery: {e}"))?;
            let taken = life(n, true, &content)?;
            let live = until(&mut kernel, n, "live after the take-over", |i| {
                i.is_some_and(|i| i.state == DeviceState::Live)
            })?;
            if live.map(|i| i.shape) != Some(shape()) {
                return Err(format!("taken over as {live:?}, not {:?}", shape()));
            }
            // The life that was taken over serves: what the life
            // before wrote is there, and a write of its own comes back.
            let round = 1 + u64::from(garbage_in_the_librarys_name);
            {
                let file = direct(n)?;
                written_reads_back(
                    &file,
                    4096,
                    &pattern(8192, 4096, round),
                    "what the life before wrote",
                )?;
                write_and_read_back(&file, 8192, 4096, round + 1, "after the take-over")?;
            }
            taken.end()?;
            until(&mut kernel, n, "waiting", |i| {
                i.is_some_and(|i| i.state == DeviceState::Waiting)
            })?;
            for (i, (path, text)) in files.iter().enumerate() {
                let want = if i == 0 && garbage_in_the_librarys_name {
                    &files[1].1
                } else {
                    text
                };
                match std::fs::read_to_string(path) {
                    Ok(now) if now == *want => {}
                    other => {
                        return Err(format!(
                            "{} was changed or removed: {other:?}",
                            path.display()
                        ))
                    }
                }
            }
        }
        kernel.delete(n).map_err(|e| format!("delete: {e}"))?;
        until(&mut kernel, n, "gone", Option::is_none)?;
        if let Some((path, _)) = files.iter().find(|(path, _)| !path.exists()) {
            return Err(format!("{} went with the delete", path.display()));
        }
        Ok(())
    })();
    for (path, _) in &files {
        let _ = std::fs::remove_file(path);
    }
    if made_dir {
        let _ = std::fs::remove_dir(dir);
    }
    clean(n);
    property("take_over_beside_stale_file", || result);
}

/// A device whose largest request is above what the tag buffers hold
/// is not opened for serving, and is left as it is: the driver would
/// copy a request of that size past a buffer's end.
#[test]
#[ignore = "needs root and the ublk driver"]
fn a_device_with_larger_requests_than_the_buffers_is_not_opened() {
    let n = number(8);
    claim(n, "larger_requests_than_the_buffers_refused");
    let result = (|| {
        let larger = 2 * skyzfs_ublk_sys::MAX_IO;
        skyzfs_ublk_sys::raw::add(n, DEPTH, larger, FLAGS)
            .map_err(|e| format!("adding a device with requests of {larger} bytes: {e}"))?;
        let before = info(&mut kernel()?, n)?;
        match Device::open(&Open {
            number: n,
            size: SIZE,
            take_over: false,
        }) {
            Ok(_) => return Err("the device was opened".into()),
            Err(e) if e.kind() == std::io::ErrorKind::InvalidInput => {}
            Err(e) => return Err(format!("refused, but with: {e}")),
        }
        let after = info(&mut kernel()?, n)?;
        if after != before || after.is_none() {
            return Err(format!("the device was {before:?} and is {after:?}"));
        }
        Ok(())
    })();
    clean(n);
    property("larger_requests_than_the_buffers_refused", || result);
}

/// How long the number of a device nobody holds takes to come free
/// after the driver's delete, which does not wait: over many deletes of
/// a device that was never started and of one whose server has ended.
/// The lifecycle calls a number held when its device is still there
/// 5 s after the delete; this is what that figure rests on. It fails
/// if a number took longer than that, and prints the longest and the
/// median time of each kind, and how often the device was still there
/// at the first look.
#[test]
#[ignore = "needs root and the ublk driver"]
fn a_delete_frees_the_number_of_an_unheld_device_within_the_time_the_lifecycle_gives() {
    const DELETES: usize = 40;
    const LIMIT: Duration = Duration::from_secs(5);
    let n = number(7);
    claim(n, "delete_frees_the_number_within");
    let result = (|| {
        let mut kernel = kernel()?;
        let mut lines = Vec::new();
        for served in [false, true] {
            let kind = if served {
                "after its server ended"
            } else {
                "never started"
            };
            let mut times = Vec::with_capacity(DELETES);
            let mut there_at_first_look = 0;
            for round in 0..DELETES {
                kernel
                    .add(n, &shape())
                    .map_err(|e| format!("{kind}, round {round}: add: {e:?}"))?;
                if served {
                    let content = Arc::new(Mutex::new(vec![0u8; SIZE as usize]));
                    let life = life(n, false, &content)?;
                    until(&mut kernel, n, "live", |i| {
                        i.is_some_and(|i| i.state == DeviceState::Live)
                    })?;
                    life.end()?;
                    until(&mut kernel, n, "waiting", |i| {
                        i.is_some_and(|i| i.state == DeviceState::Waiting)
                    })?;
                }
                let began = Instant::now();
                kernel
                    .delete(n)
                    .map_err(|e| format!("{kind}, round {round}: delete: {e}"))?;
                let mut looks = 0u32;
                loop {
                    let gone = info(&mut kernel, n)?.is_none();
                    looks += 1;
                    if gone {
                        break;
                    }
                    if began.elapsed() > LIMIT {
                        return Err(format!(
                            "{kind}, round {round}: device {n} is still there {:?} after its \
                             delete, and nothing holds it",
                            began.elapsed()
                        ));
                    }
                    std::thread::sleep(Duration::from_micros(200));
                }
                if looks > 1 {
                    there_at_first_look += 1;
                }
                times.push(began.elapsed());
            }
            times.sort();
            lines.push(format!(
                "{kind}: longest {:?}, median {:?}, still there at the first look in \
                 {there_at_first_look} of {DELETES}",
                times[DELETES - 1],
                times[DELETES / 2]
            ));
        }
        println!(
            "control_delete_frees_the_number_detail\t{DELETES} deletes of each kind, limit {LIMIT:?}; {}",
            lines.join("; ")
        );
        Ok(())
    })();
    clean(n);
    property("delete_frees_the_number_within", || result);
}

/// What a queue's thread saw at one stop: see [`after_the_stop`].
#[derive(Debug, Default)]
struct Stop {
    /// Deallocations that gave back memory the driver had been told.
    freed: u64,
    /// The ranges the driver had been told.
    told: usize,
    /// How many of them were no longer mapped after the drop.
    unmapped: usize,
    /// The blocks taken afterwards that overlap a told range.
    overlapping: usize,
    /// Bytes that changed in the blocks taken afterwards.
    changed: u64,
    /// The most writes the block layer showed in flight at the device
    /// while the thread lingered.
    in_flight: u64,
    /// The lowest told address and the lowest taken one.
    lowest: (usize, usize),
}

/// The writes in flight at the device, from the block layer.
fn in_flight_writes(n: u32) -> u64 {
    std::fs::read_to_string(format!("/sys/block/ublkb{n}/inflight"))
        .ok()
        .and_then(|text| text.split_whitespace().nth(1)?.parse().ok())
        .unwrap_or(0)
}

/// What a queue's thread does at a stop, in place of the store's
/// shutdown. It drops the queue and looks at the memory whose
/// addresses the driver had been told: whether any of it went back to
/// the allocator (the binary's allocator counts that: the check that
/// is certain), and whether it is still mapped. Then it takes memory
/// of the size of the request buffers, fills it, goes through the
/// kernel many times while the writers' requests keep arriving at the
/// device, and counts the bytes that changed under it and the blocks
/// that lie where a told buffer was.
fn after_the_stop(queue: skyzfs_ublk_sys::queue::KernelQueue<'_>, n: u32) -> Stop {
    const FILL: u8 = 0x5a;
    const LINGER: Duration = Duration::from_millis(150);
    let told = queue.ranges_told_for_tests();
    let watched = told
        .iter()
        .filter(|&&(start, len)| watch::watch(start, len))
        .count();
    let before = watch::freed();
    drop(queue);
    let mut stop = Stop {
        // A range that could not be watched counts as one given back:
        // the check must not pass for want of room.
        freed: watch::freed() - before + (told.len() - watched) as u64,
        told: told.len(),
        unmapped: told
            .iter()
            .filter(|&&(start, len)| !watch::mapped(start, len))
            .count(),
        ..Stop::default()
    };
    let mut held: Vec<Buf> = (0..4 * usize::from(DEPTH))
        .map(|_| {
            let mut buf = Buf::zeroed((1 << 20) / BLOCK_SIZE as usize);
            buf.fill(FILL);
            buf
        })
        .collect();
    stop.overlapping = held
        .iter()
        .filter(|buf| {
            let (at, len) = (buf.as_ptr() as usize, buf.len());
            told.iter()
                .any(|&(start, told_len)| start < at + len && at < start + told_len)
        })
        .count();
    stop.lowest = (
        told.iter().map(|r| r.0).min().unwrap_or(0),
        held.iter().map(|b| b.as_ptr() as usize).min().unwrap_or(0),
    );
    let began = Instant::now();
    while began.elapsed() < LINGER {
        // Each return from the kernel is a moment at which the driver
        // copies an arrived write to the address its fetch names.
        std::thread::sleep(Duration::from_millis(2));
        stop.in_flight = stop.in_flight.max(in_flight_writes(n));
    }
    stop.changed = held
        .iter()
        .map(|buf| buf.iter().filter(|&&b| b != FILL).count() as u64)
        .sum();
    held.clear();
    stop
}

/// A server stopped under writers, many times: after each stop the
/// queue's thread goes on working while the device is still live and
/// requests arrive for tags whose fetch commands are still with the
/// driver. The addresses those commands name must stay memory that
/// nothing else uses.
///
/// **What is certain**: at every stop the queue says which memory the
/// driver may have been told, the queue is dropped, and this binary's
/// allocator must not have seen a deallocation of any byte of it
/// ([`after_the_stop`]). That fails when the buffers are freed at the
/// queue's drop, whatever the allocator then does with the memory and
/// whether or not a request arrives.
///
/// **What is only observed**: the thread then takes memory of the
/// buffers' size and counts the bytes the kernel changed in it. With
/// the buffers kept this is zero. With the buffers freed it shows
/// damage only if the allocator hands the freed memory out again and
/// a write arrives for it in that time; the line printed for each stop
/// says whether the told ranges were still mapped, whether any block
/// taken afterwards lay where one was, and whether writes were in
/// flight at the device. A zero here is not evidence that the buffers
/// were kept.
///
/// Every write the writers were acknowledged reads back, and the next
/// life takes over.
///
/// And an orderly stop answers what it committed: every write is one
/// request (1 MiB, the largest), the server commits every request it
/// is handed, and the writers count the writes that returned. If a
/// commit made before a stop did not reach the driver, the kernel
/// issues that request again and the next life is handed it a second
/// time: so the requests handed over all lives equal the writes the
/// writers finished exactly when no committed request was issued
/// again. (A request that arrives during a stop and is never handed to
/// the server is issued to the next life and counted there once.)
#[test]
#[ignore = "needs root and the ublk driver"]
fn a_stop_under_writers_leaves_the_memory_taken_afterwards_alone() {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    const ROUNDS: u32 = 24;
    const WRITERS: u64 = 4;
    const MIB: u64 = 1 << 20;
    let n = number(6);
    claim(n, "stop_under_writers_leaves_later_memory_alone");
    let result = (|| {
        let mut kernel = kernel()?;
        kernel.add(n, &shape()).map_err(|e| format!("add: {e:?}"))?;
        let content = Arc::new(Mutex::new(vec![0u8; SIZE as usize]));
        let seen = Arc::new(Seen::default());
        let serve = |take_over: bool| -> Result<_, String> {
            let device = Device::open(&Open {
                number: n,
                size: SIZE,
                take_over,
            })
            .map_err(|e| format!("open: {e}"))?;
            let wake = Wake::new().map_err(|e| e.to_string())?;
            let (content, seen) = (content.clone(), seen.clone());
            let serving = device
                .serve(&wake, false, move |mut queue| {
                    memory_gathering(&mut queue, content, &seen, 0);
                    after_the_stop(queue, n)
                })
                .map_err(|e| format!("serve: {e}"))?;
            if let Err(e) = device.start() {
                serving.stopper().stop();
                let _ = serving.join();
                return Err(format!("start (take-over {take_over}): {e}"));
            }
            Ok((serving, device))
        };
        let mut life = serve(false)?;

        // Each writer has its own megabytes and writes them round and
        // round, each write filled with its round's number, and
        // remembers the last round it was acknowledged for each place.
        let done = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicU64::new(0));
        let writers: Vec<_> = (0..WRITERS)
            .map(|w| {
                let (done, finished) = (done.clone(), finished.clone());
                std::thread::spawn(move || -> Result<Vec<u8>, String> {
                    let file = direct(n)?;
                    let places = 4u64;
                    let mut last = vec![0u8; places as usize];
                    let mut buf = Buf::zeroed((MIB / BLOCK_SIZE) as usize);
                    let mut round = 0u64;
                    while !done.load(Ordering::SeqCst) {
                        round += 1;
                        let place = round % places;
                        let fill = (round % 251) as u8 + 1;
                        buf.fill(fill);
                        file.write_all_at(&buf, (w * places + place) * MIB)
                            .map_err(|e| format!("writer {w}, write {round}: {e}"))?;
                        last[place as usize] = fill;
                        finished.fetch_add(1, Ordering::SeqCst);
                    }
                    Ok(last)
                })
            })
            .collect();

        let mut stops: Vec<Stop> = Vec::new();
        let mut idle_rounds = 0u32;
        for round in 0..ROUNDS {
            let before = finished.load(Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(20 + u64::from(round % 5) * 7));
            if finished.load(Ordering::SeqCst) == before {
                idle_rounds += 1;
            }
            let (serving, device) = life;
            serving.stopper().stop();
            stops.push(serving.join().map_err(|e| format!("round {round}: {e}"))?);
            drop(device);
            until(&mut kernel, n, "waiting", |i| {
                i.is_some_and(|i| i.state == DeviceState::Waiting)
            })?;
            kernel
                .begin_recovery(n)
                .map_err(|e| format!("round {round}: begin of recovery: {e}"))?;
            life = serve(true)?;
        }
        done.store(true, Ordering::SeqCst);
        let mut lasts = Vec::new();
        for writer in writers {
            lasts.push(
                writer
                    .join()
                    .map_err(|_| "a writer panicked".to_owned())??,
            );
        }
        let total = finished.load(Ordering::SeqCst);
        // Every place holds the write that was acknowledged last there.
        {
            let file = direct(n)?;
            let mut got = Buf::zeroed((MIB / BLOCK_SIZE) as usize);
            for (w, last) in lasts.iter().enumerate() {
                for (place, &fill) in last.iter().enumerate() {
                    if fill == 0 {
                        continue;
                    }
                    let at = (w as u64 * 4 + place as u64) * MIB;
                    file.read_exact_at(&mut got, at)
                        .map_err(|e| format!("reading back at {at}: {e}"))?;
                    if let Some(bad) = got.iter().position(|&b| b != fill) {
                        return Err(format!(
                            "writer {w}, place {place}: byte {bad} is {} and the last \
                             acknowledged write there was filled with {fill}",
                            got[bad]
                        ));
                    }
                }
            }
        }
        let (serving, _device) = life;
        serving.stopper().stop();
        stops.push(serving.join().map_err(|e| e.to_string())?);
        // The reads of the check above are not writes: not counted.
        let handed = seen.handed.load(Ordering::SeqCst);
        // One line per stop, for whoever asks why a defect did or did
        // not show.
        for (round, stop) in stops.iter().enumerate() {
            println!(
                "control_stop_under_writers_stop\t{round}: {} ranges told (lowest {:#x}), {} \
                 deallocations gave told memory back, {} of the ranges no longer mapped; of \
                 the blocks taken afterwards (lowest {:#x}) {} lie where a told buffer was; \
                 up to {} writes in flight at the device meanwhile; {} bytes changed",
                stop.told,
                stop.lowest.0,
                stop.freed,
                stop.unmapped,
                stop.lowest.1,
                stop.overlapping,
                stop.in_flight,
                stop.changed
            );
        }
        let changed: u64 = stops.iter().map(|s| s.changed).sum();
        let freed: u64 = stops.iter().map(|s| s.freed).sum();
        let stops_freed = stops.iter().filter(|s| s.freed > 0).count();
        let with_arrivals = stops.iter().filter(|s| s.in_flight > 0).count();
        println!(
            "control_stop_under_writers_detail\t{ROUNDS} stops under {WRITERS} writers of 1 MiB, \
             {total} writes finished, {handed} write requests handed to the servers, \
             {idle_rounds} rounds in which none finished before the stop, \
             {changed} bytes changed in memory taken after a stop, \
             {freed} deallocations of memory the driver had been told, \
             writes in flight at the device after {with_arrivals} of the {} stops",
            stops.len()
        );
        if freed != 0 {
            return Err(format!(
                "{freed} deallocations gave memory back whose address the driver had been \
                 told, at {stops_freed} of {} stops",
                stops.len()
            ));
        }
        if handed != total {
            return Err(format!(
                "{handed} write requests were handed to the servers for {total} writes that \
                 returned: {} were issued again after a stop that had committed them",
                handed.abs_diff(total)
            ));
        }
        if changed != 0 {
            return Err(format!(
                "{changed} bytes of memory taken after a stop were written by the kernel"
            ));
        }
        if total < u64::from(ROUNDS) {
            return Err(format!(
                "only {total} writes finished over {ROUNDS} stops: the writers were not writing"
            ));
        }
        Ok(())
    })();
    clean(n);
    property("stop_under_writers_leaves_later_memory_alone", || result);
}

/// One step of the control test program: its answer, without the step
/// and the number.
#[cfg(feature = "control-test-program")]
fn step(n: u32, line: &str) -> Result<String, String> {
    let mut words = line.split_whitespace();
    let name = words.next().unwrap_or_default();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_skyzfs-ublk-control"))
        .arg(name)
        .arg(n.to_string())
        .args(words)
        .output()
        .map_err(|e| format!("{line}: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if !out.status.success() {
        return Err(format!("{line}: {:?}, said {text:?}", out.status));
    }
    text.strip_prefix(&format!("{name}\t{n}\t"))
        .and_then(|answer| answer.strip_suffix('\n'))
        .filter(|answer| !answer.contains('\n'))
        .map(str::to_string)
        .ok_or_else(|| format!("{line}: said {text:?}, which is not one line of the step"))
}

/// A server of the control test program: started, its first line read,
/// then ended by `SIGTERM` and its second line read.
#[cfg(feature = "control-test-program")]
fn served(n: u32, line: &str) -> Result<(String, String), String> {
    use std::io::{BufRead, BufReader};
    let mut words = line.split_whitespace();
    let name = words.next().unwrap_or_default();
    let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_skyzfs-ublk-control"))
        .arg(name)
        .arg(n.to_string())
        .args(words)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("{line}: {e}"))?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no pipe")?).lines();
    let mut next = |what: &str| match lines.next() {
        Some(Ok(text)) => Ok(text),
        other => Err(format!("{line}: no {what} line: {other:?}")),
    };
    let first = next("first")?;
    if !first.contains("\tlive after ") {
        let _ = child.wait();
        return Err(format!("{line}: {first}"));
    }
    // Something for the server to count: a write and a read through
    // the block device.
    let data = pattern(4096, 4096, 9);
    let io = direct(n).and_then(|file| {
        file.write_all_at(&data, 4096)
            .map_err(|e| format!("a write: {e}"))?;
        written_reads_back(&file, 4096, &data, "4 KiB through the test program")
    });
    let killed = std::process::Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .map_err(|e| format!("kill: {e}"))?;
    let second = next("second");
    let status = child.wait().map_err(|e| e.to_string())?;
    io?;
    if !killed.success() || !status.success() {
        return Err(format!("{line}: kill {killed:?}, exit {status:?}"));
    }
    Ok((first, second?))
}

/// The control test program, step by step over one device: each step
/// prints one line with the binding's answer and exits 0, also when
/// the driver refuses; its server serves a file, counts what it was
/// handed, ends on a signal and leaves the device waiting; its
/// take-over waits as told between the queue and the end of the
/// recovery and is handed nothing in that time.
#[cfg(feature = "control-test-program")]
#[test]
#[ignore = "needs root and the ublk driver"]
fn the_control_test_program_answers_each_step_in_one_line() {
    let n = number(5);
    claim(n, "test_program_steps");
    let file = std::env::temp_dir().join(format!("skyzfs-ublk-control-{n}.img"));
    let _ = std::fs::remove_file(&file);
    let path = file.display().to_string();
    let result = (|| {
        let expect = |line: &str, want: &dyn Fn(&str) -> bool| {
            let answer = step(n, line)?;
            if want(&answer) {
                Ok(answer)
            } else {
                Err(format!("{line}: answered {answer:?}"))
            }
        };
        expect("info", &|a| a == "nothing")?;
        expect("delete", &|a| a == "nothing")?;
        expect("begin-recovery", &|a| a == "nothing")?;
        expect("client-holds", &|a| a == "unknown")?;
        expect("add --depth 8", &|a| a == "added")?;
        expect("add --depth 8", &|a| a == "taken")?;
        expect("info", &|a| {
            a.starts_with("state=dead size=0 queues=1 depth=8 flags=0x18 raw_state=")
        })?;
        // Not waiting for a server: the driver's own refusal, exit 0.
        // On the kernels this ran on the driver answers a device that
        // was never started with `EBUSY`, the same as for one whose
        // server is still being torn down.
        expect("begin-recovery", &|a| a == "busy")?;
        let (first, second) = served(n, &format!("serve --file {path} --size {SIZE}"))?;
        if !first.contains("handed before the start: 0;") {
            return Err(format!("serve: {first}"));
        }
        if !second.starts_with(&format!("served\t{n}\tended by a signal; handed "))
            || second.contains(" writes 0 ")
        {
            return Err(format!("serve: {second}"));
        }
        // The driver reports the device as waiting a moment after its
        // server has gone.
        let mut kernel = kernel()?;
        until(&mut kernel, n, "waiting", |i| {
            i.is_some_and(|i| i.state == DeviceState::Waiting)
        })?;
        expect("info", &|a| {
            a.starts_with(&format!(
                "state=waiting size={SIZE} queues=1 depth=8 flags=0x18 "
            ))
        })?;
        // A device that was started takes no parameters: whatever the
        // driver answers, it is one line and the device is as it was.
        let answer = expect(&format!("set-params --size {}", SIZE / 2), &|a| a != "set")?;
        println!("control_set_params_on_a_started_device\t{answer}");
        expect("info", &|a| {
            a.starts_with(&format!("state=waiting size={SIZE} "))
        })?;
        let from = Instant::now();
        let (first, second) = served(
            n,
            &format!("take-over --file {path} --recovery-wait-ms 1500"),
        )?;
        if !first.contains("handed before the start: 0;")
            || from.elapsed() < Duration::from_millis(1500)
        {
            return Err(format!("take-over: {first}"));
        }
        if !second.contains("ended by a signal") {
            return Err(format!("take-over: {second}"));
        }
        expect("begin-recovery --retry", &|a| {
            a.starts_with("begun; busy answers: ")
        })?;
        let (_, second) = served(n, &format!("serve --file {path} --recovery-begun"))?;
        if !second.contains("ended by a signal") {
            return Err(format!("serve --recovery-begun: {second}"));
        }
        expect("delete", &|a| a.starts_with("returned after "))?;
        until(&mut kernel, n, "gone", Option::is_none)?;
        expect("info", &|a| a == "nothing")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&file);
    clean(n);
    property("test_program_steps", || result);
}

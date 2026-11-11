//! The binding's [`Control`] and queue on a real kernel: what the
//! lifecycle's tests assume of a control interface, asked of the
//! driver. Ignored by default: they need root and the ublk driver.
//!
//! ```text
//! sudo -E cargo test -p skyzfs-ublk-sys --test kernel -- --ignored --test-threads 1
//! ```
//!
//! Each test prints one line `control_<property>\tPASS` or
//! `control_<property>\tFAIL: <why>` on standard output, for the rig,
//! and fails with the same words. They use device numbers from
//! `SKYZFS_KERNEL_TEST_BASE` (default 200) on, one per test, and delete
//! what they add. These are the cases that need no stranger; the ones
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

/// Delete whatever the test left, and wait until it is gone.
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
    clean(n);
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
    clean(n);
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
                let mut buf = queue.take_buf(f.tag, (bytes / BLOCK_SIZE) as usize);
                if f.desc.op() == op::READ {
                    buf.fill(0);
                }
                buf
            });
            queue.commit(f.tag, bytes as i32, buf);
            answered += 1;
        }
        if woken == Woken::Stop {
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
    clean(n);
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

/// A server that keeps the device's content in memory, shared between
/// the lives of a device: what one life was written, the next reads.
/// Returns the longest write it was handed, in bytes.
fn memory(mut queue: impl Queue, content: Arc<Mutex<Vec<u8>>>) -> u64 {
    let mut fetched: Vec<Fetched> = Vec::new();
    let mut longest = 0;
    loop {
        fetched.clear();
        let Ok(woken) = queue.wait(&mut fetched) else {
            return longest;
        };
        for f in fetched.drain(..) {
            let at = (f.desc.start_sector * 512) as usize;
            let bytes = f.desc.nr_sectors as usize * 512;
            let mut content = content.lock().expect("the content");
            let buf = match f.desc.op() {
                op::READ => {
                    let mut buf = queue.take_buf(f.tag, bytes / BLOCK_SIZE as usize);
                    buf.copy_from_slice(&content[at..at + bytes]);
                    Some(buf)
                }
                op::WRITE => {
                    let buf = queue.take_buf(f.tag, bytes / BLOCK_SIZE as usize);
                    content[at..at + bytes].copy_from_slice(&buf);
                    longest = longest.max(bytes as u64);
                    Some(buf)
                }
                _ => None,
            };
            queue.commit(f.tag, bytes as i32, buf);
        }
        if woken == Woken::Stop {
            return longest;
        }
    }
}

/// One life of a device over `content`: opened, served, started.
struct Life {
    serving: skyzfs_ublk_sys::queue::Serving<u64>,
    _device: Device,
}

fn life(n: u32, take_over: bool, content: &Arc<Mutex<Vec<u8>>>) -> Result<Life, String> {
    let device = Device::open(&Open {
        number: n,
        size: SIZE,
        take_over,
    })
    .map_err(|e| format!("open: {e}"))?;
    let wake = Wake::new().map_err(|e| e.to_string())?;
    let content = content.clone();
    let serving = device
        .serve(&wake, false, move |queue| memory(queue, content))
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
        _device: device,
    })
}

impl Life {
    /// End the life in order; the longest write, and the first-write
    /// copies.
    fn end(self) -> Result<(u64, u64), String> {
        let stopper = self.serving.stopper();
        stopper.stop();
        let longest = self.serving.join().map_err(|e| e.to_string())?;
        Ok((longest, stopper.first_write_copies()))
    }
}

/// A buffer of `bytes` whose content depends on `seed` and the place.
fn pattern(bytes: usize, seed: u8) -> Buf {
    let mut buf = Buf::zeroed(bytes / BLOCK_SIZE as usize);
    for (i, b) in buf.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(31).wrapping_add(seed) ^ (i >> 12) as u8;
    }
    buf
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
/// one in the tag's own) and across a take-over, where the first
/// requests of the new life are writes of each size. A 1 MiB write
/// arrives as one request. The copies of first writes are counted and
/// are at most the depth, in each life.
///
/// The first life ends in order and not by a kill: the test is its own
/// server. The device waits for a server either way, and the take-over
/// is the same begin and end of a recovery.
#[test]
#[ignore = "needs root and the ublk driver"]
fn data_of_4k_and_1m_is_whole_in_one_life_and_across_a_take_over() {
    let n = number(3);
    clean(n);
    const MIB: usize = 1 << 20;
    let result = (|| {
        let mut kernel = kernel()?;
        kernel.add(n, &shape()).map_err(|e| format!("add: {e:?}"))?;
        let content = Arc::new(Mutex::new(vec![0u8; SIZE as usize]));
        let first = life(n, false, &content)?;
        let (small, large) = (pattern(4096, 1), pattern(MIB, 2));
        {
            let file = direct(n)?;
            // Twice round the tags and more, so that tags are used
            // after their first commit too.
            for round in 0..3 * u64::from(DEPTH) {
                let at = (round % 8) * 8192;
                file.write_all_at(&small, at)
                    .map_err(|e| format!("a 4 KiB write: {e}"))?;
                written_reads_back(&file, at, &small, "4 KiB")?;
            }
            for at in [8 << 20, 16 << 20] {
                file.write_all_at(&large, at)
                    .map_err(|e| format!("a 1 MiB write: {e}"))?;
                written_reads_back(&file, at, &large, "1 MiB")?;
            }
        }
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
        let second = life(n, true, &content)?;
        let (small2, large2) = (pattern(4096, 3), pattern(MIB, 4));
        {
            let file = direct(n)?;
            // The first requests of this life: a write of each size.
            file.write_all_at(&large2, 24 << 20)
                .map_err(|e| format!("a 1 MiB write after the take-over: {e}"))?;
            file.write_all_at(&small2, 4096)
                .map_err(|e| format!("a 4 KiB write after the take-over: {e}"))?;
            written_reads_back(&file, 24 << 20, &large2, "1 MiB after the take-over")?;
            written_reads_back(&file, 4096, &small2, "4 KiB after the take-over")?;
            // What the first life was written.
            written_reads_back(&file, 8 << 20, &large, "1 MiB of the first life")?;
            written_reads_back(&file, 16 << 20, &large, "1 MiB of the first life")?;
            written_reads_back(&file, 0, &small, "4 KiB of the first life")?;
            for round in 0..2 * u64::from(DEPTH) {
                let at = (32 << 20) + round * 8192;
                file.write_all_at(&small2, at)
                    .map_err(|e| format!("a 4 KiB write: {e}"))?;
                written_reads_back(&file, at, &small2, "4 KiB")?;
            }
        }
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
#[test]
#[ignore = "needs root and the ublk driver"]
fn a_take_over_works_beside_a_stale_file_of_another_server_and_leaves_it() {
    let n = number(4);
    clean(n);
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
        life(n, false, &content)?.end()?;
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

/// One step of the control test program: its answer, without the step
/// and the number.
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
    let data = pattern(4096, 9);
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
#[test]
#[ignore = "needs root and the ublk driver"]
fn the_control_test_program_answers_each_step_in_one_line() {
    let n = number(5);
    clean(n);
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

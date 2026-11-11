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
use std::os::unix::fs::OpenOptionsExt;
use std::time::{Duration, Instant};

use skyzfs_blk::BLOCK_SIZE;
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

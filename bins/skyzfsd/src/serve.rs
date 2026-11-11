//! `serve`: the lifecycle's start, then one queue pumped between the
//! kernel and the store until a stop, a failure, or the queue's end.
//!
//! # The order
//!
//! 1. The two deployment rules (design §4.1), each with its override
//!    for tests: the process's memory is locked, and neither the
//!    volume's directory nor the run directory is on ZFS.
//! 2. The signals are blocked, before any thread exists.
//! 3. The lifecycle's start ([`lifecycle::start`]): the lock, the
//!    failure note, the acknowledged position, the store, the device.
//! 4. The store goes behind its worker, the queue's thread is started
//!    and gives the driver its fetches, and the device is started (or
//!    its recovery ended). Then one line on standard output, flushed:
//!    `ready /dev/ublkbN`.
//! 5. The main thread waits: for `SIGTERM` or `SIGINT`, for the store's
//!    word that the volume failed, or for the queue's thread to end.
//!
//! # How it ends
//!
//! | What happened | Exit status |
//! |---|---|
//! | a stop was asked and everything accepted was completed | 0 |
//! | a stop was asked and the acknowledged position could not be stored | 1 |
//! | the volume failed while it was served | 3 |
//! | the queue went away, or the target did | 1 |
//!
//! After an orderly stop the device stays and waits for the next
//! server. When the volume fails, the failure is noted, the device's
//! note is removed and the device is deleted ([`lifecycle::fail_serving`]),
//! while the store still holds the volume's lock.
//!
//! # Readings where the design says no more
//!
//! - "Its own files off any pool it serves" is checked as: the volume's
//!   directory and the run directory are not on a file system of type
//!   `zfs`, by `/proc/self/mountinfo`. That refuses more than the rule
//!   asks (a pool the daemon does not serve) and nothing it allows.
//! - A start that fails after the lifecycle has added or begun to
//!   recover a device leaves that device as it is: the next start
//!   finds it by its record and its note.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use skyzfs_local::log::SUPER_NAME;
use skyzfs_local::store::{FailureObserver, OpenOptions, Waker};
use skyzfs_ublk::ack::{AckFile, SharedAck};
use skyzfs_ublk::lifecycle::{self, Stop, Wanted};
use skyzfs_ublk::pump::{Pump, Stopped};
use skyzfs_ublk::records::{self, Files};
use skyzfs_ublk::status::{self, Board, Pace, Status};
use skyzfs_ublk::volume::StoreVolume;
use skyzfs_ublk_sys::control::{block_device, Kernel};
use skyzfs_ublk_sys::queue::{Device, Open};
use skyzfs_ublk_sys::signals::{self, Signal};
use skyzfs_ublk_sys::{Wake, FLAGS};

use crate::cli::{Exit, Serve};
use crate::commands::{GONE_RETRIES, LOCK_PAUSE, LOCK_RETRIES};
use crate::delay::{Delayed, LateAck};
use crate::timing::{TimedControl, TimedVolume};

/// How often a start asks again about a device of its own that is
/// still shown as live: 50 times, 100 ms apart, 5 s.
pub const LIVE_RETRIES: u32 = 50;

/// How often the status file is written at most: every 100 ms.
pub const STATUS_INTERVAL_MS: u64 = 100;

/// How long `ready` waits at most for the status thread's first write
/// of this instance.
const READY_WAITS_FOR_STATUS: Duration = Duration::from_secs(5);

/// How long the main thread sleeps between two looks at the queue's
/// thread and the failure flag when no signal arrives.
const LOOK: Duration = Duration::from_millis(50);

/// A line on standard error. A write that fails is ignored: nothing
/// the daemon says may end it or change its exit status.
fn say(text: impl std::fmt::Display) {
    crate::to_stderr(&format!("skyzfsd: {text}\n"));
}

/// The magic number of a ZFS file system in `statfs`.
pub const ZFS_SUPER_MAGIC: u64 = 0x2fc1_2fc1;

/// The nearest directory that exists at or above `dir`, resolved to
/// its real path: where a directory that does not exist yet (the run
/// directory) will be made. A relative `dir` is taken under `cwd`, so
/// that `run`, or `a/b` with no `a`, ends at the current directory and
/// not at an empty path.
pub fn nearest_existing(dir: &Path, cwd: &Path) -> io::Result<std::path::PathBuf> {
    let absolute = cwd.join(dir);
    let mut existing: &Path = &absolute;
    loop {
        match existing.canonicalize() {
            Ok(real) => return Ok(real),
            Err(e) => match existing.parent() {
                Some(parent) => existing = parent,
                None => return Err(e),
            },
        }
    }
}

/// The rule that the daemon keeps its files off any pool (design
/// §4.1): refuse `dir` if the file system it is on, or will be made
/// on, is ZFS. `kind` is `statfs`'s file system type for a path.
///
/// The kernel is asked (`statfs` of the real path) and the mount table
/// is not parsed: the kernel cannot be wrong about which mount a path
/// is on (bind mounts, mount namespaces, a mount over a mount, a mount
/// point with a tab, a newline or a backslash in its name, which the
/// table escapes). An answer that cannot be had is a refusal.
///
/// What the check sees and what it does not: a directory on any file
/// system of type ZFS is refused, whatever pool it is. A file system
/// of another type that itself lies on ZFS (ext4 on a zvol, or in a
/// loop file on a dataset) is not seen, and neither is ZFS under a
/// network or stacked file system.
pub fn off_zfs(
    dir: &Path,
    what: &str,
    cwd: &Path,
    kind: impl Fn(&Path) -> io::Result<u64>,
) -> Result<(), String> {
    let real = nearest_existing(dir, cwd).map_err(|e| format!("{what} {}: {e}", dir.display()))?;
    match kind(&real) {
        Ok(ZFS_SUPER_MAGIC) => Err(format!(
            "{what} {} is on ZFS: the daemon keeps its files off any pool",
            dir.display()
        )),
        Ok(_) => Ok(()),
        Err(e) => Err(format!(
            "the file system of {what} {} cannot be told ({}: {e})",
            dir.display(),
            real.display()
        )),
    }
}

/// `statfs`'s file system type of `path`.
fn statfs_kind(path: &Path) -> io::Result<u64> {
    let stat = rustix::fs::statfs(path)?;
    // A signed word on most architectures; the magic numbers are below
    // 2^32, so its low 32 bits are the whole of it.
    Ok(u64::from(stat.f_type as u32))
}

fn boot(serve: &Serve) -> io::Result<[u8; 16]> {
    match &serve.hooks.boot_id_file {
        Some(path) => records::parse_boot_id(&std::fs::read_to_string(path)?)
            .ok_or_else(|| io::Error::other(format!("{}: not a boot id", path.display()))),
        None => records::boot_id(),
    }
}

/// What the main thread and the queue's thread share.
#[derive(Default)]
struct Flags {
    /// The store said that the volume failed.
    failed: AtomicBool,
    /// `SIGUSR1` arrived: the test build fails the log.
    user: AtomicBool,
}

/// What `--fail-log-after` counts: the requests completed so far that
/// append to the log, which are writes, write-zeroes and discards. A
/// read never touches the log and a flush appends no operation, so
/// neither counts: a check that writes until the volume fails is then
/// what trips the failure, and not the reads of the device's start.
pub fn appended(counters: &skyzfs_ublk::pump::Counters) -> u64 {
    counters.writes + counters.write_zeroes + counters.discards
}

/// The test build's faults, on the queue's thread after each turn. In
/// a default build this does nothing and the options that ask for it
/// are refused by the command line.
#[cfg(feature = "test-hooks")]
fn faults<Q: skyzfs_ublk::pump::Queue>(
    pump: &mut Pump<Q, skyzfs_local::store::Served, LateAck<SharedAck>>,
    flags: &Flags,
    fail_after: &mut Option<u64>,
) {
    let due = fail_after.is_some_and(|after| appended(&pump.counters()) >= after);
    if due {
        *fail_after = None;
    }
    if due || flags.user.swap(false, Ordering::SeqCst) {
        pump.target().fail_now();
    }
}

#[cfg(not(feature = "test-hooks"))]
fn faults<Q: skyzfs_ublk::pump::Queue>(
    _pump: &mut Pump<Q, skyzfs_local::store::Served, LateAck<SharedAck>>,
    _flags: &Flags,
    _fail_after: &mut Option<u64>,
) {
}

/// Put the pump's counters up for the status file. A result the test
/// build keeps back is a request the kernel still waits for: it counts
/// as in flight.
fn post<Q: skyzfs_ublk::pump::Queue>(
    board: &Board,
    pump: &mut Pump<Delayed<Q, SharedAck>, skyzfs_local::store::Served, LateAck<SharedAck>>,
) {
    let mut counters = pump.counters();
    counters.in_flight += pump.queue().waiting() as u64;
    board.post(counters);
}

/// Run `serve`. Prints for itself: the ready line has to be out when
/// the device serves, not when the daemon ends.
pub fn serve(serve: &Serve) -> Exit {
    if !serve.dir.join(SUPER_NAME).is_file() {
        say(format_args!("{} holds no volume", serve.dir.display()));
        return Exit::Error;
    }
    match skyzfs_ublk_sys::lock_all_memory() {
        Ok(()) => {}
        Err(e) if serve.allow_unlocked_memory => {
            say(format_args!("warning: memory is not locked ({e})"));
        }
        Err(e) => {
            say(format_args!("the daemon's memory cannot be locked: {e}"));
            return Exit::Error;
        }
    }
    if !serve.allow_any_filesystem {
        for (dir, what) in [
            (&serve.dir, "the volume's directory"),
            (&serve.run_dir, "the run directory"),
        ] {
            let checked = std::env::current_dir()
                .map_err(|e| format!("the current directory: {e}"))
                .and_then(|cwd| off_zfs(dir, what, &cwd, statfs_kind));
            if let Err(why) = checked {
                say(why);
                return Exit::Error;
            }
        }
    }
    if let Err(e) = signals::block() {
        say(format_args!("blocking signals: {e}"));
        return Exit::Error;
    }
    match run(serve) {
        Ok(exit) => exit,
        Err(why) => {
            say(why);
            Exit::Error
        }
    }
}

fn run(serve: &Serve) -> Result<Exit, String> {
    let boot = boot(serve).map_err(|e| format!("reading the boot id: {e}"))?;
    let mut records = Files::new(&serve.dir, &serve.run_dir, boot, LOCK_PAUSE)
        .map_err(|e| format!("{}: {e}", serve.dir.display()))?;
    records
        .create_run_dirs()
        .map_err(|e| format!("{}: {e}", serve.run_dir.display()))?;
    let wake = Wake::new().map_err(|e| format!("an event descriptor: {e}"))?;
    let flags = Arc::new(Flags::default());

    let mut options = OpenOptions::default();
    options.pretend_short_by = serve.hooks.recover_short;
    options.on_failure = Some(FailureObserver::new({
        let (flags, wake) = (flags.clone(), wake.clone());
        move || {
            flags.failed.store(true, Ordering::SeqCst);
            wake.wake();
        }
    }));
    let began = std::time::Instant::now();
    let mut volume = TimedVolume::new(StoreVolume::new(&serve.dir, options));
    let mut kernel = TimedControl::new(Kernel::new().map_err(|e| e.to_string())?);
    let wanted = Wanted {
        queues: serve.queues,
        depth: serve.depth,
        flags: FLAGS,
        lock_retries: LOCK_RETRIES,
        live_retries: LIVE_RETRIES,
        gone_retries: GONE_RETRIES,
        acknowledge_failure: serve.acknowledge_failure,
    };
    let ready = match lifecycle::start(&mut kernel, &mut records, &mut volume, &wanted) {
        Ok(ready) => ready,
        Err(Stop::CannotReissue) => {
            return Err(
                "the kernel's ublk driver cannot reissue requests after a recovery \
                        (UBLK_F_USER_RECOVERY_REISSUE): not served"
                    .into(),
            )
        }
        Err(Stop::Held) => {
            say("another instance or a live server holds the volume");
            return Ok(Exit::RefusedHeld);
        }
        Err(Stop::Failed(why)) => {
            say(why);
            return Ok(Exit::RefusedFailed);
        }
        Err(Stop::Error(why)) => return Err(why),
    };
    if let Some((number, why)) = ready.left {
        say(format_args!("device {number} was left as it is: {why}"));
    }
    if let Some(reason) = ready.acknowledged {
        say(format_args!("a failure was acknowledged: {reason}"));
    }
    let (number, shape) = (ready.number, ready.shape);
    let identity = ready.store.identity();
    // What the server before this one had applied and never answered:
    // the kernel issues those requests again, and they are applied a
    // second time (design §4.1). A lower bound.
    let recovered = ready.store.recovered_unacknowledged();
    let (log_bytes, log_records) = ready.store.replayed();
    let took_over = ready.took_over;
    let to_device = began.elapsed();
    let ack = AckFile::open(&records.acknowledged_path(), identity, boot)
        .map_err(|e| format!("the record of what was acknowledged: {e}"))?;
    let served = ready
        .store
        .serve_with(Some(Waker::new({
            let wake = wake.clone();
            move || wake.wake()
        })))
        .map_err(|e| format!("the store's worker: {e}"))?;

    let queue_began = std::time::Instant::now();
    let device = Device::open(&Open {
        number,
        size: shape.size,
        take_over: took_over,
    })
    .map_err(|e| format!("opening device {number}: {e}"))?;
    let board = Arc::new(Board::new());
    let mut fail_after = serve.hooks.fail_log_after;
    let delay = serve.hooks.commit_delay_ms.map(Duration::from_millis);
    let hold = serve.hooks.hold;
    let serving = device
        .serve(&wake, !serve.allow_unlocked_memory, {
            let (board, flags, wake) = (board.clone(), flags.clone(), wake.clone());
            move |queue| {
                let (queue, ack) =
                    Delayed::new(queue, SharedAck::new(ack), delay, hold, move || wake.wake());
                let mut pump = Pump::new(queue, served, ack);
                let stopped = loop {
                    if let Some(stopped) = pump.turn() {
                        break stopped;
                    }
                    post(&board, &mut pump);
                    faults(&mut pump, &flags, &mut fail_after);
                };
                post(&board, &mut pump);
                // The pump is dropped here, and with it the store: the
                // volume's lock is held until then.
                stopped
            }
        })
        .map_err(|e| format!("the queue of device {number}: {e}"))?;
    let stopper = serving.stopper();
    let publisher = std::thread::Builder::new()
        .name("skyzfsd-status".into())
        .spawn({
            let board = board.clone();
            let path = records.volume_run_dir().join(status::STATUS);
            let (queues, depth) = (shape.queues, shape.depth);
            let stopper = stopper.clone();
            move || {
                board.publish(Pace::new(STATUS_INTERVAL_MS), |counters, failures| {
                    let status = Status {
                        pid: std::process::id(),
                        queues,
                        depth,
                        boot,
                        identity,
                        written_ms: status::uptime_ms()?,
                        counters: *counters,
                        failures,
                        first_write_copies: stopper.first_write_copies(),
                        recovered_unacknowledged: recovered,
                    };
                    status::write(&path, &status)
                })
            }
        })
        .map_err(|e| format!("the status thread: {e}"))?;
    let counter = stopper.clone();
    let finish = |board: &Board| {
        board.stop();
        let _ = publisher.join();
        // For whoever reads the daemon's log: `status` shows the same
        // while the daemon lives.
        say(format_args!(
            "served device {number}: first_write_copies {}, recovered_unacknowledged {recovered}",
            counter.first_write_copies()
        ));
    };

    let start_began = std::time::Instant::now();
    if let Err(e) = device.start() {
        stopper.stop();
        let _ = serving.join();
        finish(&board);
        return Err(format!("starting device {number}: {e}"));
    }
    let started = std::time::Instant::now();
    // `ready` only after this instance's first status write was tried,
    // done or failed: until then the status file is the one the
    // instance before left, and `status` would name that instance's
    // pid and counters as the serving daemon's. The device is live
    // already and its clients are served meanwhile. The wait is
    // bounded, so that a status thread that never writes holds `ready`
    // for a stated time and not for good.
    if !board.first_write_tried(READY_WAITS_FOR_STATUS) {
        say(format_args!(
            "warning: the status file was not written within {} s of the start: `status` \
             may show the instance before",
            READY_WAITS_FOR_STATUS.as_secs()
        ));
    }
    // A `ready` that cannot be written (the starter is gone) does not
    // stop the serve: the device is live and its clients are served.
    crate::to_stdout(&format!("ready {}\n", block_device(number)));
    // Where the start's time went. `devices` is everything between the
    // store's open and the device being this instance's: the records,
    // the notes, the waiting while the device was still shown as live
    // (`live`, a part of it) and the begin of a recovery (`begin`, a
    // part of it, with the driver's busy answers). `queue` is the
    // device's open and the queue's fetches; `start` is the start, or
    // the end of the recovery.
    let ms = |d: Duration| d.as_millis();
    let devices = to_device.saturating_sub(volume.lock + volume.position + volume.recovery);
    say(format_args!(
        "serving device {number} ({}) after {} ms: lock {} ms, position {} ms, recovery {} ms \
         of a log of {log_bytes} bytes and {log_records} operations, devices {} ms (live {} ms, \
         begin {} ms with {} busy answers), queue {} ms, start {} ms; \
         recovered_unacknowledged {recovered}",
        if took_over { "taken over" } else { "added" },
        ms(started - began),
        ms(volume.lock),
        ms(volume.position),
        ms(volume.recovery),
        ms(devices),
        ms(kernel.live),
        ms(kernel.begin),
        kernel.inner.busy_answers(),
        ms(queue_began.elapsed().saturating_sub(start_began.elapsed())),
        ms(start_began.elapsed().saturating_sub(started.elapsed())),
    ));
    board.post(Default::default());

    #[derive(PartialEq)]
    enum Why {
        Stop,
        Failed,
        Ended,
    }
    let why = loop {
        if flags.failed.load(Ordering::SeqCst) {
            break Why::Failed;
        }
        if serving.is_finished() {
            break Why::Ended;
        }
        match signals::wait(LOOK) {
            Some(Signal::Stop) => break Why::Stop,
            Some(Signal::User) => {
                flags.user.store(true, Ordering::SeqCst);
                wake.wake();
            }
            None => {}
        }
    };
    if why == Why::Failed {
        // The store still holds the volume's lock: the queue's thread
        // has not ended, or has only just.
        let (failure, written) =
            lifecycle::fail_serving(&mut kernel, &mut records, number, identity);
        say(format_args!(
            "the volume failed while it was served ({}); client holds the device: {:?}{}",
            failure.reason,
            failure.held,
            if written {
                ""
            } else {
                "; the failure note could not be written"
            }
        ));
        // The delete takes the queue down. A stop as well, for a
        // device whose delete did not.
        stopper.stop();
        let _ = serving.join();
        finish(&board);
        return Ok(Exit::Failed);
    }
    if why == Why::Stop {
        stopper.stop();
    }
    let stopped = serving.join().map_err(|e| e.to_string());
    finish(&board);
    // A failure can arrive with the queue's end.
    if flags.failed.load(Ordering::SeqCst) {
        let (failure, _) = lifecycle::fail_serving(&mut kernel, &mut records, number, identity);
        say(format_args!(
            "the volume failed while it was served ({})",
            failure.reason
        ));
        return Ok(Exit::Failed);
    }
    match stopped? {
        Stopped::Orderly if why == Why::Stop => Ok(Exit::Done),
        Stopped::Unstored { left } => Err(format!(
            "stopped, and the acknowledged position could not be stored: {left} completed \
             requests were not answered and are issued again to the next instance"
        )),
        other => Err(format!("the queue of device {number} ended: {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--fail-log-after` counts what appends to the log: not reads,
    /// not flushes, not what was refused or is in flight.
    #[test]
    fn the_log_fault_counts_writes_zeroes_and_discards_only() {
        let counters = skyzfs_ublk::pump::Counters {
            reads: 100,
            flushes: 50,
            refused: 9,
            in_flight: 8,
            ..Default::default()
        };
        assert_eq!(appended(&counters), 0);
        let counters = skyzfs_ublk::pump::Counters {
            writes: 3,
            write_zeroes: 2,
            discards: 1,
            ..counters
        };
        assert_eq!(appended(&counters), 6);
    }

    /// A directory on ZFS is refused, one on anything else passes, and
    /// an answer that cannot be had is a refusal. The name of the
    /// directory plays no part: the kernel is asked about the real
    /// path, also for a name with the characters a mount table escapes.
    #[test]
    fn a_directory_on_zfs_is_refused_by_what_the_kernel_says() {
        let keep = tempfile::tempdir().expect("a directory");
        for name in [
            "plain",
            "with space",
            "tab\there",
            "new\nline",
            "back\\slash",
        ] {
            let dir = keep.path().join(name);
            std::fs::create_dir(&dir).expect("the directory");
            let real = dir.canonicalize().expect("real");
            let asked = std::cell::RefCell::new(Vec::new());
            let says = |magic: u64| {
                let asked = &asked;
                move |path: &Path| {
                    asked.borrow_mut().push(path.to_path_buf());
                    Ok(magic)
                }
            };
            let refused = off_zfs(
                &dir,
                "the volume's directory",
                keep.path(),
                says(ZFS_SUPER_MAGIC),
            )
            .expect_err("on ZFS");
            assert!(refused.contains("is on ZFS"), "{refused}");
            // ext4's magic.
            assert_eq!(off_zfs(&dir, "x", keep.path(), says(0xef53)), Ok(()));
            assert!(asked.borrow().iter().all(|path| *path == real), "{name}");
            let unknown = off_zfs(&dir, "x", keep.path(), |_| {
                Err(io::Error::other("no answer"))
            })
            .expect_err("no answer");
            assert!(unknown.contains("cannot be told"), "{unknown}");
        }
        // The real call answers for a real directory, and this one is
        // on some file system.
        assert!(statfs_kind(keep.path()).is_ok());
    }

    /// A directory that does not exist yet is judged by the nearest
    /// directory above it that does. A relative one is taken under the
    /// current directory: `run`, and `a/b` with no `a`, end there and
    /// are not refused for having no parent.
    #[test]
    fn a_missing_directory_is_judged_by_where_it_will_be_made_also_when_relative() {
        let keep = tempfile::tempdir().expect("a directory");
        let cwd = keep.path().canonicalize().expect("real");
        std::fs::create_dir(cwd.join("there")).expect("dir");
        for (dir, lands) in [
            ("run", cwd.clone()),
            ("a/b", cwd.clone()),
            ("there", cwd.join("there")),
            ("there/run/deeper", cwd.join("there")),
            (".", cwd.clone()),
        ] {
            assert_eq!(
                nearest_existing(Path::new(dir), &cwd).expect(dir),
                lands,
                "{dir}"
            );
            let asked = std::cell::Cell::new(None);
            let checked = off_zfs(Path::new(dir), "the run directory", &cwd, |path| {
                asked.set(Some(path.to_path_buf()));
                Ok(0xef53)
            });
            assert_eq!(checked, Ok(()), "{dir}");
            assert_eq!(asked.take(), Some(lands), "{dir}");
        }
        // An absolute one is not moved under the current directory.
        let absolute = cwd.join("there").join("missing");
        assert_eq!(
            nearest_existing(&absolute, Path::new("/nowhere")).expect("absolute"),
            cwd.join("there")
        );
    }
}

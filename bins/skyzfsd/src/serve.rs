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

use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use skyzfs_local::log::SUPER_NAME;
use skyzfs_local::store::{FailureObserver, Marks, OpenOptions, StepBound, Waker};
use skyzfs_ublk::ack::{AckFile, SharedAck};
use skyzfs_ublk::lifecycle::{self, Stop, Wanted};
use skyzfs_ublk::pump::{Pump, Stopped};
use skyzfs_ublk::records::{self, Files};
use skyzfs_ublk::status::{self, Board, LogShown, LogStatus, Pace, Status};
use skyzfs_ublk::volume::StoreVolume;
use skyzfs_ublk_sys::control::{block_device, Kernel};
use skyzfs_ublk_sys::queue::{Device, Open};
use skyzfs_ublk_sys::signals::{self, Signal};
use skyzfs_ublk_sys::{Wake, FLAGS};

use crate::cli::{Exit, Serve};
use crate::commands::{LOCK_PAUSE, LOCK_RETRIES, SEGMENT_SIZE};
use crate::delay::{Delayed, LateAck};
use crate::timing::{TimedControl, TimedVolume};

/// How often a start asks again about a device of its own that is
/// still shown as live: 50 times, 100 ms apart, 5 s.
pub const LIVE_RETRIES: u32 = 50;

/// How often the status file is written at most: every 100 ms.
pub const STATUS_INTERVAL_MS: u64 = 100;

/// How long the main thread sleeps between two looks at the queue's
/// thread and the failure flag when no signal arrives.
const LOOK: Duration = Duration::from_millis(50);

fn say(text: impl std::fmt::Display) {
    eprintln!("skyzfsd: {text}");
}

/// The type of the file system `path` is on, by the mount table in
/// `mounts` (the text of `/proc/self/mountinfo`): the mount whose
/// mount point is the longest prefix of `path`.
pub fn filesystem_of(mounts: &str, path: &Path) -> Option<String> {
    let mut best: Option<(usize, String)> = None;
    for line in mounts.lines() {
        // "36 35 98:0 /root /mount/point options... - type source ..."
        let (before, after) = line.split_once(" - ")?;
        let point = before.split(' ').nth(4)?;
        let kind = after.split(' ').next()?;
        // The table escapes a space as \040.
        let point = point.replace("\\040", " ");
        if path.starts_with(&point) {
            let len = Path::new(&point).components().count();
            if best.as_ref().is_none_or(|(most, _)| len >= *most) {
                best = Some((len, kind.to_string()));
            }
        }
    }
    best.map(|(_, kind)| kind)
}

fn off_zfs(dir: &Path, what: &str) -> Result<(), String> {
    // A run directory that does not exist yet is made under its
    // nearest existing parent.
    let mut existing = dir;
    let real = loop {
        match existing.canonicalize() {
            Ok(real) => break real,
            Err(e) => match existing.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => existing = parent,
                _ => return Err(format!("{what} {}: {e}", dir.display())),
            },
        }
    };
    let mounts = std::fs::read_to_string("/proc/self/mountinfo")
        .map_err(|e| format!("/proc/self/mountinfo: {e}"))?;
    match filesystem_of(&mounts, &real).as_deref() {
        Some("zfs") => Err(format!(
            "{what} {} is on ZFS: the daemon keeps its files off any pool",
            dir.display()
        )),
        Some(_) => Ok(()),
        None => Err(format!(
            "the file system of {what} {} cannot be told",
            dir.display()
        )),
    }
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
    shown: &mut LogShown,
) {
    let mut counters = pump.counters();
    counters.in_flight += pump.queue().waiting() as u64;
    board.post(counters);
    // The log, as the store's worker last showed it. The worker wakes
    // this thread when a checkpoint step begins and when it ends; those
    // two are written to the status file at once.
    let log = pump.target().log_state();
    let now = LogShown {
        live: log.live(),
        steps: log.steps,
        stepping: log.stepping,
    };
    let step_changed = (now.steps, now.stepping) != (shown.steps, shown.stepping);
    board.post_log(now, step_changed);
    *shown = now;
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
            if let Err(why) = off_zfs(dir, what) {
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
        acknowledge_failure: serve.acknowledge_failure,
    };
    let mut ready = match lifecycle::start(&mut kernel, &mut records, &mut volume, &wanted) {
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
    // From here the store checkpoints by itself (design §5.1): not
    // before the device is this instance's, so a long log that this
    // start found is worked off while the client is served.
    ready.store.set_marks(Some(Marks {
        high: u64::from(serve.log_high) * SEGMENT_SIZE,
        low: u64::from(serve.log_low) * SEGMENT_SIZE,
        step: StepBound::DAEMON,
    }));
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
                let mut shown = LogShown::default();
                let stopped = loop {
                    if let Some(stopped) = pump.turn() {
                        break stopped;
                    }
                    post(&board, &mut pump, &mut shown);
                    faults(&mut pump, &flags, &mut fail_after);
                };
                post(&board, &mut pump, &mut shown);
                // The pump is dropped here, and with it the store: the
                // volume's lock is held until then.
                stopped
            }
        })
        .map_err(|e| format!("the queue of device {number}: {e}"))?;
    let stopper = serving.stopper();
    // How long the start took: known once the device serves, 0 until then.
    let start_ms = Arc::new(AtomicU64::new(0));
    let publisher = std::thread::Builder::new()
        .name("skyzfsd-status".into())
        .spawn({
            let board = board.clone();
            let path = records.volume_run_dir().join(status::STATUS);
            let (queues, depth) = (shape.queues, shape.depth);
            let stopper = stopper.clone();
            let start_ms = start_ms.clone();
            move || {
                board.publish_with_log(Pace::new(STATUS_INTERVAL_MS), |counters, log, failures| {
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
                        log: Some(LogStatus {
                            shown: *log,
                            start_ms: start_ms.load(Ordering::SeqCst),
                            replayed_bytes: log_bytes,
                        }),
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
    start_ms.store(
        u64::try_from((started - began).as_millis()).unwrap_or(u64::MAX),
        Ordering::SeqCst,
    );
    println!("ready {}", block_device(number));
    let _ = io::stdout().flush();
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

    /// The mount that holds a path is the one with the longest mount
    /// point that is a prefix of it, by whole components.
    #[test]
    fn the_file_system_of_a_path_is_its_longest_mount() {
        let mounts = "\
22 1 8:1 / / rw,relatime shared:1 - ext4 /dev/sda1 rw
30 22 0:25 / /run rw,nosuid shared:5 - tmpfs tmpfs rw
41 22 0:40 / /tank rw shared:9 - zfs tank rw
42 41 0:41 / /tank/with\\040space rw shared:9 - zfs tank/x rw
43 22 8:2 / /tank2 rw - xfs /dev/sdb rw
";
        let kind = |path: &str| filesystem_of(mounts, Path::new(path));
        assert_eq!(kind("/var/lib/v").as_deref(), Some("ext4"));
        assert_eq!(kind("/run/skyzfsd").as_deref(), Some("tmpfs"));
        assert_eq!(kind("/tank/v").as_deref(), Some("zfs"));
        assert_eq!(kind("/tank/with space/v").as_deref(), Some("zfs"));
        // A prefix of the name is not a prefix of the path.
        assert_eq!(kind("/tank2/v").as_deref(), Some("xfs"));
        assert_eq!(kind("/tankard").as_deref(), Some("ext4"));
        assert_eq!(filesystem_of("", Path::new("/x")), None);
    }
}

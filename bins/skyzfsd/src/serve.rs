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
//!    and queues its fetch commands (they reach the driver at the
//!    queue's first wait), and the device is started (or its recovery
//!    ended). Then, once this instance's status file was written or
//!    the write was tried (waited for at most 5 s, then said with a
//!    warning), one line on standard output, flushed:
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
//! | the volume failed while it was served, or at the start of the in-place store (`--store in-place`) | 3 |
//! | the queue went away, or the target did | 1 |
//!
//! After an orderly stop the device stays and waits for the next
//! server. When the volume fails, the failure is noted, the device's
//! note is removed and the device is deleted ([`lifecycle::fail_serving`]),
//! under the volume's lock: the main thread has a hold of its own on
//! the lock the store took, from the start until it returns, so the
//! lock does not depend on what becomes of the store on the queue's
//! thread.
//!
//! A failed volume holds no request. The driver's delete of a device
//! that is served waits for every request its server holds, so before
//! the delete the queue's thread answers everything it holds with an
//! I/O error, and everything that still arrives, without waiting for
//! the store ([`Pump::fail`]). The main thread writes the failure note,
//! waits up to two seconds for the queue's thread to say so and then
//! deletes whatever happened, and its line says which. The device is
//! live until it is deleted: a client gets errors, not a wait. Not
//! covered, and marked here because the design does not say: a store
//! call that hangs and never fails sets no failure, and nothing here
//! bounds it; and a queue's thread that is itself stuck (in a write of
//! the record of what was acknowledged that began before the failure
//! was known; once it is known that record refuses, `UnlessFailed`)
//! answers nothing, so the delete may then wait in the driver, and the
//! daemon with it.
//!
//! A start that settles a failure of unknown holding deletes the
//! failed volume's device and then looks for up to 5 s whether it goes
//! (the driver's delete does not wait): a start on a volume whose
//! device a client still holds takes that much longer to refuse.
//!
//! # Readings where the design says no more
//!
//! - "Its own files off any pool it serves" is checked as: the volume's
//!   directory and the run directory are not on a file system of type
//!   ZFS, by `statfs` ([`off_zfs`]). That refuses more than the rule
//!   asks (a pool the daemon does not serve). It does not see a file
//!   system of another type that itself lies on ZFS (on a zvol, or in
//!   a loop file on a dataset).
//! - A start that fails after the lifecycle has added or begun to
//!   recover a device leaves that device as it is: the next start
//!   finds it by its record and its note. The one exception is a start
//!   of the in-place store that meets the failed disk: the volume has
//!   failed, and it goes the way of a failure while served (the notes,
//!   the delete, exit 3).

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use skyzfs_local::log::SUPER_NAME;
use skyzfs_local::store::{FailureObserver, Marks, OpenOptions, StepBound, Waker, Zeroes};
use skyzfs_ublk::ack::{AckFile, SharedAck};
use skyzfs_ublk::lifecycle::{self, Stop, Wanted};
use skyzfs_ublk::pump::{Pump, Stopped};
use skyzfs_ublk::records::{self, Files};
use skyzfs_ublk::status::{self, Board, LogShown, LogStatus, Pace, Status, ZeroesShown};
use skyzfs_ublk::volume::StoreVolume;
use skyzfs_ublk_sys::control::{block_device, Kernel};
use skyzfs_ublk_sys::queue::{Device, Open};
use skyzfs_ublk_sys::signals::{self, Signal};
use skyzfs_ublk_sys::{Wake, FLAGS};

use crate::cli::{Exit, Serve, StoreKind};
use crate::commands::{GONE_RETRIES, LOCK_PAUSE, LOCK_RETRIES, SEGMENT_SIZE};
use crate::delay::{Delayed, LateAck};
use crate::timing::{TimedControl, TimedVolume};

mod target;
pub use target::Target;

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

/// How long the main thread waits, after the volume failed, for the
/// queue's thread to have answered what it held, before it deletes the
/// device all the same: 200 looks 10 ms apart, two seconds.
const ANSWER_LOOKS: u32 = 200;
const ANSWER_LOOK: Duration = Duration::from_millis(10);

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
/// The rule guards against a deployment mistake. It is not a boundary
/// against someone who can write where the directories are: a
/// directory swapped for another between the check and its use is not
/// seen.
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

/// The rule for a directory that must exist already, the volume's:
/// the directory itself is resolved and judged, and nothing above it
/// in its place. A path that does not resolve is refused: judged by a
/// directory above it, `missing/../pool/vol` would pass while there is
/// no `missing`, and name a volume on a pool as soon as there is one
/// (the run directory, made later, can be what makes it).
pub fn existing_off_zfs(
    dir: &Path,
    what: &str,
    cwd: &Path,
    kind: impl Fn(&Path) -> io::Result<u64>,
) -> Result<(), String> {
    let real = cwd
        .join(dir)
        .canonicalize()
        .map_err(|e| format!("{what} {} does not exist ({e})", dir.display()))?;
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

/// Make a directory of the daemon's own under the same rule, checked
/// on both sides of the making: where it will be made, by
/// [`off_zfs`], so that the common refusal leaves nothing behind; and
/// then the directory that was made, by its own real path.
///
/// The second check is the one that counts. The first predicts where
/// the directory will land from the nearest directory that exists, and
/// a path can land elsewhere than predicted: `missing/../pool/run`
/// with no `missing` is judged by the directory above `missing`, and
/// the making then creates `missing`, walks through it and back, and
/// makes `run` in `pool`. A link or a mount that appears between the
/// check and the making does the same. What was made is asked of the
/// kernel as it is.
///
/// When the second check refuses, the directories just made stay,
/// empty: the daemon has written nothing into them, and it does not
/// remove directories it cannot be sure it made.
pub fn make_off_zfs(
    dir: &Path,
    what: &str,
    cwd: &Path,
    make: impl FnOnce() -> io::Result<()>,
    kind: impl Fn(&Path) -> io::Result<u64>,
) -> Result<(), String> {
    off_zfs(dir, what, cwd, &kind)?;
    make().map_err(|e| format!("{}: {e}", dir.display()))?;
    let real = cwd
        .join(dir)
        .canonicalize()
        .map_err(|e| format!("{what} {}: {e}", dir.display()))?;
    match kind(&real) {
        Ok(ZFS_SUPER_MAGIC) => Err(format!(
            "{what} {} was made on ZFS ({}): the daemon keeps its files off any pool. \
             Nothing was written there; the empty directory is left",
            dir.display(),
            real.display()
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

/// Run `pump` until it stops, calling `each` after every turn (with
/// `false`) and once after the last (with `true`), and give the pump's
/// target back with how it stopped: `None` if the pump panicked.
///
/// The target is the store, which holds the volume's lock. It is not
/// dropped on the queue's thread, where its end would free the lock
/// while the main thread still has the failure's cleanup to do: it
/// goes back as the thread's result, and the lock is held until the
/// main thread drops it. That holds when the pump panics too: the
/// panic is caught here, the pump is still whole enough to give its
/// target up (a panic is no reason to trust what it did, but the
/// target is moved, not used), and the main thread gets the store and
/// reports the panic.
pub fn pumped<Q, T, A>(
    mut pump: Pump<Q, T, A>,
    mut each: impl FnMut(&mut Pump<Q, T, A>, bool),
) -> (Option<Stopped>, T)
where
    Q: skyzfs_ublk::pump::Queue,
    T: skyzfs_ublk::pump::Positions,
    A: skyzfs_ublk::pump::AckRecord,
{
    let stopped = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let stopped = loop {
            if let Some(stopped) = pump.turn() {
                break stopped;
            }
            each(&mut pump, false);
        };
        each(&mut pump, true);
        stopped
    }))
    .ok();
    (stopped, pump.into_target())
}

/// A failed volume holds no request. Called on the queue's thread
/// after every turn: once the store has said that the volume failed,
/// everything the queue's wrapper and the pump hold is answered with an
/// I/O error ([`Delayed::fail`], [`Pump::fail`]), and the queue is
/// woken so that its next wait, which sends those answers, returns at
/// once. The call after that wait says so in `flags.answered`, for the
/// main thread, which deletes the device: the driver's delete of a
/// served device waits for every request its server holds.
fn answer_if_failed<Q, T, A>(
    pump: &mut Pump<Delayed<Q, A>, T, LateAck<A>>,
    flags: &Flags,
    wake: &dyn Fn(),
) where
    Q: skyzfs_ublk::pump::Queue,
    T: skyzfs_ublk::pump::Positions,
    A: skyzfs_ublk::pump::AckRecord + Clone,
{
    if !flags.failed.load(Ordering::SeqCst) {
        return;
    }
    if pump.is_failed() {
        let kept = flags.kept.load(Ordering::SeqCst);
        flags
            .answers
            .store(kept + pump.failed_answers(), Ordering::SeqCst);
        flags.answered.store(true, Ordering::SeqCst);
        return;
    }
    let kept = pump.queue().fail() as u64;
    flags.kept.store(kept, Ordering::SeqCst);
    let held = pump.fail();
    flags.answers.store(kept + held, Ordering::SeqCst);
    wake();
}

/// The record of what was acknowledged, which stores nothing once the
/// volume has failed. The turn in which the store fails can still have
/// completions to commit; storing their position is a write and a sync
/// in the run directory, and on a node whose disks are failing that is
/// one more place for the queue's thread to hang with requests held. A
/// refusal costs nothing: the completions stay back as for any
/// position that could not be stored, and the call that follows the
/// turn answers them with the I/O error ([`answer_if_failed`]). An
/// error promises nothing, so nothing acknowledged goes unrecorded.
/// This narrows the case of a stuck queue's thread and does not close
/// it: a store that hangs before the failure is known hangs as before.
#[derive(Clone)]
struct UnlessFailed<A> {
    record: A,
    flags: Arc<Flags>,
}

impl<A: skyzfs_ublk::pump::AckRecord> skyzfs_ublk::pump::AckRecord for UnlessFailed<A> {
    fn advance(&mut self, position: u64) -> io::Result<()> {
        if self.flags.failed.load(Ordering::SeqCst) {
            return Err(io::Error::other("the volume has failed"));
        }
        self.record.advance(position)
    }
}

/// The daemon's record of what was acknowledged.
type Ack = UnlessFailed<SharedAck>;

/// What the main thread and the queue's thread share.
#[derive(Default)]
struct Flags {
    /// The store said that the volume failed.
    failed: AtomicBool,
    /// The queue's thread has answered everything it held with an I/O
    /// error, and a wait of the queue has followed, which sent those
    /// answers to the driver.
    answered: AtomicBool,
    /// How many requests it answered so, so far.
    answers: AtomicU64,
    /// Of those, the ones the queue's wrapper had kept back (test
    /// hooks: a delayed or a held commit).
    kept: AtomicU64,
    /// `SIGUSR1` arrived: the test build fails the log.
    user: AtomicBool,
}

/// What the failure line says of the requests the daemon held, from
/// what the main thread knows. `answered`: the queue's thread said that
/// it answered `answers` requests and that a wait of the queue has
/// sent those answers. `ended`: the queue's thread ended without
/// saying so (the pump ended in the turn that saw the failure, or the
/// thread panicked); what it had answered by then may never have been
/// sent, so no count is given. Neither: it did not answer in time.
fn answers_said(answered: bool, ended: bool, answers: u64) -> String {
    if answered {
        format!(
            "{answers} requests the daemon held were answered with an I/O error before the \
             delete"
        )
    } else if ended {
        "the queue's thread ended before it said that it had answered what it held: the \
         driver fails what was left at the delete"
            .to_string()
    } else {
        "the delete has returned".to_string()
    }
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
    pump: &mut Pump<Q, Target, LateAck<Ack>>,
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
    _pump: &mut Pump<Q, Target, LateAck<Ack>>,
    _flags: &Flags,
    _fail_after: &mut Option<u64>,
) {
}

/// The store's times as the status file holds them: the file keeps no
/// count of the batches a step's parts were measured over, so those
/// are left out here as a reader of the file finds them.
fn shown_timing(mut timing: skyzfs_local::store::Timing) -> skyzfs_local::store::Timing {
    for part in [
        &mut timing.step_walk,
        &mut timing.step_save,
        &mut timing.step_apply,
        &mut timing.step_home_sync,
        &mut timing.step_start,
        &mut timing.step_end,
        &mut timing.step_own_sync,
    ] {
        part.count = 0;
    }
    timing
}

/// The store's log state and times as the status file shows them. The
/// in-place store has no log, no steps and no times of its own: the
/// file shows zeros for those, and how write-zeroes reach the image.
fn log_shown(target: &Target) -> LogShown {
    let served = match target {
        Target::Log(served) => served,
        Target::InPlace(store) => {
            return LogShown {
                timing: None,
                pump: None,
                live: 0,
                steps: 0,
                stepping: false,
                own_syncs: 0,
                zeroes: zeroes_shown(store.zeroes()),
                io_queue: Some(store.io_queue()),
            }
        }
    };
    // The state and the times of one moment.
    let (log, timing) = &served.shown();
    LogShown {
        timing: Some(shown_timing(*timing)),
        pump: None,
        live: log.live(),
        steps: log.steps,
        stepping: log.stepping,
        own_syncs: log.own_syncs,
        zeroes: zeroes_shown(log.zeroes),
        io_queue: None,
    }
}

fn zeroes_shown(zeroes: Zeroes) -> ZeroesShown {
    match zeroes {
        Zeroes::Untried => ZeroesShown::Untried,
        Zeroes::Deallocated => ZeroesShown::Deallocated,
        Zeroes::Written => ZeroesShown::Written,
    }
}

/// Put the pump's counters up for the status file. A result the test
/// build keeps back is a request the kernel still waits for: it counts
/// as in flight.
fn post<Q: skyzfs_ublk::pump::Queue>(
    board: &Board,
    pump: &mut Pump<Delayed<Q, Ack>, Target, LateAck<Ack>>,
    shown: &mut LogShown,
) {
    let mut counters = pump.counters();
    counters.in_flight += pump.queue().waiting() as u64;
    board.post(counters);
    // The log, as the store's worker last showed it. The worker wakes
    // this thread when a checkpoint step begins and when it ends; those
    // two are written to the status file at once.
    let now = LogShown {
        pump: Some(pump.timing()),
        ..log_shown(pump.target())
    };
    let step_changed = (now.steps, now.stepping) != (shown.steps, shown.stepping);
    say_writeback_off(shown, &now);
    board.post_log(now, step_changed);
    *shown = now;
}

/// One line when the hint that starts writeback early is switched off
/// for a kind of file: its file system said it has no such call.
fn say_writeback_off(before: &LogShown, now: &LogShown) {
    let off = |shown: &LogShown, at: usize| shown.timing.is_some_and(|t| t.writeback[at].off);
    for (at, files) in [(0, "the log's segment files"), (1, "the home image")] {
        if off(now, at) && !off(before, at) {
            say(format_args!(
                "the file system has no call to start writeback early: \
                 the hint is off for {files} (the syncs are as they were)"
            ));
        }
    }
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
        // The volume's directory exists and is judged as itself; the
        // run directory is judged here by where it will be made, and
        // again in `run` once it is made.
        let checked = std::env::current_dir()
            .map_err(|e| format!("the current directory: {e}"))
            .and_then(|cwd| {
                existing_off_zfs(&serve.dir, "the volume's directory", &cwd, statfs_kind)?;
                off_zfs(&serve.run_dir, "the run directory", &cwd, statfs_kind)
            });
        if let Err(why) = checked {
            say(why);
            return Exit::Error;
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
    if serve.allow_any_filesystem {
        records
            .create_run_dirs()
            .map_err(|e| format!("{}: {e}", serve.run_dir.display()))?;
    } else {
        // The run directory was judged before by where it would be
        // made; here it is made and judged again as what it is, before
        // the store is opened or a device is touched. The status file,
        // the failure note and the record of what was acknowledged are
        // all in this one directory. The volume's own directory needs
        // no second look: its check resolved the directory itself and
        // refused a path that did not resolve.
        let cwd = std::env::current_dir().map_err(|e| format!("the current directory: {e}"))?;
        make_off_zfs(
            records.volume_run_dir(),
            "the run directory",
            &cwd,
            || records.create_run_dirs(),
            statfs_kind,
        )?;
    }
    let wake = Wake::new().map_err(|e| format!("an event descriptor: {e}"))?;
    let flags = Arc::new(Flags::default());

    let mut options = OpenOptions::default();
    options.pretend_short_by = serve.hooks.recover_short;
    options.writeback_unit = serve.writeback_kib << 10;
    options.home_writeback_unit = serve.home_writeback_kib << 10;
    options.readers = usize::from(serve.readers);
    options.in_place_io = serve.in_place_io;
    options.on_failure = Some(FailureObserver::new({
        let (flags, wake) = (flags.clone(), wake.clone());
        move || {
            flags.failed.store(true, Ordering::SeqCst);
            wake.wake();
        }
    }));
    let began = std::time::Instant::now();
    // The in-place store's start counts the home image's cached pages
    // after it drops them (`cachestat`); the log store does not ask.
    let volume =
        StoreVolume::new(&serve.dir, options).with_cache_count(skyzfs_ublk_sys::cached_pages);
    // With both units 0 the store never asks, and the call is not even
    // given: the benchmark's row without the hint is the daemon as it
    // was before it.
    let volume = if serve.writeback_kib == 0 && serve.home_writeback_kib == 0 {
        volume
    } else {
        volume.with_writeback(skyzfs_ublk_sys::start_writeback)
    };
    let mut volume = TimedVolume::new(volume);
    let mut kernel = TimedControl::new(Kernel::new().map_err(|e| e.to_string())?);
    let wanted = Wanted {
        queues: serve.queues,
        depth: serve.depth,
        flags: FLAGS,
        lock_retries: LOCK_RETRIES,
        live_retries: LIVE_RETRIES,
        gone_retries: GONE_RETRIES,
        acknowledge_failure: serve.acknowledge_failure,
        async_delete_asserted: serve.async_delete_asserted,
    };
    let mut ready = match lifecycle::start(&mut kernel, &mut records, &mut volume, &wanted) {
        Ok(ready) => ready,
        Err(Stop::Unsupported(why)) => return Err(not_served(&why)),
        Err(Stop::Held) => {
            say("another instance or a live server holds the volume");
            return Ok(Exit::RefusedHeld);
        }
        Err(Stop::Failed(why)) => {
            // `status` says the same from the notes, after this
            // process has gone.
            say(format_args!("{why}. Note: {}", status::POOL_WARNING));
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
    // The main thread holds the volume's lock from here until this
    // function returns, by a hold of its own on the lock the store
    // took. Whatever becomes of the store (it goes to the queue's
    // thread, and a panic there, in the pump or outside it, drops it
    // there), the lock is not free before the main thread has done the
    // cleanup after a failure: the failure note, the device's note and
    // the delete of the device by its number. Without it a detach or
    // another serve could act on the attachment in between.
    let Some(_lock) = ready.store.share_lock() else {
        return Err("the store holds no lock on the volume".into());
    };
    let (number, shape) = (ready.number, ready.shape);
    // From here the store checkpoints by itself (design §5.1): not
    // before the device is this instance's. The steps begin when the
    // store's worker is started, a few lines down: that is before the
    // device is started and before the ready line, so a long log that
    // this start found is worked off without waiting for the first
    // request. The start itself does not wait for a step: it goes on
    // while the worker steps. The first requests can wait for those
    // steps: while the log is at or above the high mark the worker
    // takes steps before it executes a request.
    // (The in-place store has no log: none of this applies to it, and
    // the command line refuses its options.)
    if serve.store == StoreKind::Log {
        ready.store.set_marks(Some(Marks {
            high: u64::from(serve.log_high) * SEGMENT_SIZE,
            low: u64::from(serve.log_low) * SEGMENT_SIZE,
            step: StepBound::DAEMON,
        }));
    }
    #[cfg(feature = "test-hooks")]
    if serve.hooks.refuse_holes {
        ready.store.set_refuse_holes(true);
    }
    #[cfg(feature = "test-hooks")]
    if let Some(pause) = &serve.hooks.step_pause {
        use crate::cli::StepPoint as At;
        use skyzfs_local::store::{StepPause, StepPoint};
        let pause = match pause {
            crate::cli::StepPause::Ms(ms) => StepPause::For(Duration::from_millis(*ms)),
            crate::cli::StepPause::Until(path) => StepPause::Until(path.clone()),
        };
        let at = match serve.hooks.step_pause_at {
            At::Saved => StepPoint::Saved,
            At::Written => StepPoint::Written,
            At::Synced => StepPoint::Synced,
            At::StartWritten => StepPoint::StartWritten,
            At::StartDurable => StepPoint::StartDurable,
        };
        ready.store.set_step_pause_at(pause, at);
    }
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
    let waker = Waker::new({
        let wake = wake.clone();
        move || wake.wake()
    });
    let served = match serve.store {
        StoreKind::Log => Target::Log(
            ready
                .store
                .serve_with(Some(waker))
                .map_err(|e| format!("the store's worker: {e}"))?,
        ),
        // The log is applied and emptied, the home image synced, and
        // the queue's thread executes the requests itself; the sync of
        // a flush runs on the image's helper thread, which wakes the
        // queue's thread when it ends.
        StoreKind::InPlace => match ready.store.into_in_place(Some(waker)) {
            Ok(store) => Target::InPlace(Box::new(store)),
            // The disk failed under the start: the volume has failed,
            // and it goes the way of a volume that fails while it is
            // served: the failure note, the device's note, the delete of
            // the device, exit 3. The device is this instance's and is
            // not started yet; it holds no request of this instance's.
            // The store's failure marker comes last: it needs the disk
            // that has just failed and may never end, and the note must
            // not wait for it. (Where the log store's own checkpoint
            // failed, its marker was tried already, as at any start of
            // the log store.) The helper's end without a report is not a
            // failure of the volume, and ends with exit 1 as it does
            // while the log store serves (`Err(other)`).
            Err(skyzfs_local::inplace::IntoInPlaceError::Failed(start)) => {
                let (failure, written, deleted) =
                    lifecycle::fail_serving(&mut kernel, &mut records, number, identity);
                say(format_args!(
                    "the volume failed at the start of the in-place store ({}); client holds the \
                     device: {:?}{}{}. Note: {}",
                    failure.reason,
                    failure.held,
                    if written {
                        ""
                    } else {
                        "; the failure note could not be written"
                    },
                    match &deleted {
                        Ok(()) => String::new(),
                        Err(undeleted) => format!("; {}", undeleted.said(number)),
                    },
                    status::POOL_WARNING
                ));
                start.finish();
                return Ok(Exit::Failed);
            }
            // The start serves only what the disk holds (design §4.1):
            // where it cannot show that, it serves nothing. The volume
            // has not failed, and the device waits as after any start
            // that fails after the lifecycle added it.
            Err(
                refused @ (skyzfs_local::inplace::IntoInPlaceError::CacheKept(_)
                | skyzfs_local::inplace::IntoInPlaceError::Io(_)),
            ) => {
                return Err(format!(
                    "the in-place store: {refused}. Nothing was served and the volume has not \
                     failed"
                ))
            }
            Err(other) => return Err(format!("the in-place store: {other}")),
        },
    };

    let queue_began = std::time::Instant::now();
    let device = Device::open(&Open {
        number,
        size: shape.size,
        take_over: took_over,
    })
    .map_err(|e| format!("opening device {number}: {e}"))?;
    let board = Arc::new(Board::new());
    // The log as the start found it, on the board before the status
    // thread's first write: without it the file says a live size of 0
    // until the queue's thread has had its first turn, which on an
    // idle device is the first request (seen on a kernel: `log_live 0`
    // right after a start that had replayed 81 MB).
    let first_log = LogShown {
        pump: Some(skyzfs_ublk::pump::PumpTiming::default()),
        ..log_shown(&served)
    };
    board.post_log(first_log, false);
    let mut fail_after = serve.hooks.fail_log_after;
    let delay = serve.hooks.commit_delay_ms.map(Duration::from_millis);
    let hold = serve.hooks.hold;
    let serving = device
        .serve(&wake, !serve.allow_unlocked_memory, {
            let (board, flags, wake) = (board.clone(), flags.clone(), wake.clone());
            move |queue| {
                let again = wake.clone();
                let ack = UnlessFailed {
                    record: SharedAck::new(ack),
                    flags: flags.clone(),
                };
                let (queue, ack) = Delayed::new(queue, ack, delay, hold, move || wake.wake());
                // The store goes back to the main thread with the
                // outcome: the volume's lock is held until the main
                // thread lets go of it.
                let mut shown = first_log;
                pumped(Pump::new(queue, served, ack), |pump, ended| {
                    answer_if_failed(pump, &flags, &|| again.wake());
                    post(&board, pump, &mut shown);
                    if !ended {
                        faults(pump, &flags, &mut fail_after);
                    }
                })
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
                        version: status::VERSION,
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
    // The start's time is known only now, after the status thread's
    // first write: it is written without a post, which would replace
    // the counters the queue's thread has put up.
    start_ms.store(
        u64::try_from((started - began).as_millis()).unwrap_or(u64::MAX),
        Ordering::SeqCst,
    );
    board.write_again();
    // The order of a start: the status thread starts and writes the
    // file at once with what the board holds (zeros, or what the
    // queue's thread has posted by then); the device starts; the
    // queue's thread posts its counters after every turn, and each
    // post replaces the board's with newer ones of its own; `ready`
    // follows the first write. Only the queue's thread posts, but for
    // the log's state at the start, which the main thread put on the
    // board before either thread existed.
    //
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
    // Nothing is posted from here: the queue's thread is running and
    // the board holds its counters. (The publishing thread writes the
    // file once when it starts, whatever is posted then, and after
    // that whenever the queue's thread posts a change.)

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
        // The delete of a served device waits in the driver for every
        // request its server holds. The queue's thread answers what it
        // holds with an I/O error as soon as it sees the failure; this
        // waits for that, for a stated time, and then deletes whatever
        // happened: a queue's thread that is itself stuck (in a call
        // that began before the failure was known) answers nothing
        // however long this waits.
        //
        // The failure note is written before that wait and not after
        // it. A daemon killed during the wait then leaves the note,
        // and the next start reads from it whether a client held the
        // device. Without the note that start goes by the log's marker
        // alone and counts the volume as held for the whole boot: safe,
        // and a failure nobody held could then not be acknowledged.
        // Nothing speaks for the other order: the note is written in
        // the run directory either way, and what the next start finds
        // after a kill between the note and the delete is the state it
        // has always had to deal with (the note, and the device with
        // its own note, waiting).
        //
        // The volume's lock is held: by `_lock`, whatever the queue's
        // thread has done with the store.
        let (failure, written) =
            lifecycle::note_failure(&mut kernel, &mut records, number, identity);
        let (mut answered, mut ended) = (false, false);
        for _ in 0..ANSWER_LOOKS {
            // The thread's end first: a flag set just before the end
            // is then seen.
            ended = serving.is_finished();
            answered = flags.answered.load(Ordering::SeqCst);
            if answered || ended {
                break;
            }
            std::thread::sleep(ANSWER_LOOK);
        }
        if !answered && !ended {
            // Said before the delete and not with its outcome: the
            // delete may now wait in the driver for as long as the
            // queue's thread holds a request, and this line is then
            // all an operator has. "May": the thread may only be slow.
            say(format_args!(
                "the volume failed, and the queue's thread did not answer what it held within \
                 {} ms: device {number} is deleted all the same, and the delete may wait in \
                 the driver for as long as a request is held",
                ANSWER_LOOKS * ANSWER_LOOK.as_millis() as u32
            ));
        }
        let deleted = lifecycle::delete_failed(&mut kernel, &mut records, number);
        say(format_args!(
            "the volume failed while it was served ({}); client holds the device: {:?}{}; {}{}. \
             Note: {}",
            failure.reason,
            failure.held,
            if written {
                ""
            } else {
                "; the failure note could not be written"
            },
            answers_said(answered, ended, flags.answers.load(Ordering::SeqCst)),
            match &deleted {
                Ok(()) => String::new(),
                Err(undeleted) => format!("; {}", undeleted.said(number)),
            },
            status::POOL_WARNING
        ));
        // The delete takes the queue down. A stop as well, for a
        // device whose delete did not.
        stopper.stop();
        let _store = serving.join();
        finish(&board);
        return Ok(Exit::Failed);
    }
    if why == Why::Stop {
        stopper.stop();
    }
    // The store comes back with the outcome and is dropped when this
    // function returns. The volume's lock does not depend on it:
    // `_lock` holds it past the failure's cleanup below also when the
    // queue's thread panicked outside the pump or the queue was never
    // made, and the store was dropped there.
    let (stopped, _store) = match serving.join() {
        Ok((Some(stopped), store)) => (Ok(stopped), Some(store)),
        Ok((None, store)) => (
            Err(format!("the queue's thread of device {number} panicked")),
            Some(store),
        ),
        Err(e) => (Err(e.to_string()), None),
    };
    finish(&board);
    // A failure can arrive with the queue's end.
    if flags.failed.load(Ordering::SeqCst) {
        let (failure, written) =
            lifecycle::note_failure(&mut kernel, &mut records, number, identity);
        let deleted = lifecycle::delete_failed(&mut kernel, &mut records, number);
        say(format_args!(
            "the volume failed while it was served ({}){}{}. Note: {}",
            failure.reason,
            if written {
                ""
            } else {
                "; the failure note could not be written"
            },
            match &deleted {
                Ok(()) => String::new(),
                Err(undeleted) => format!("; {}", undeleted.said(number)),
            },
            status::POOL_WARNING
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

/// What a start refused for the kernel says. The check comes before
/// the attachment record is read, so a device of this volume may be
/// waiting for a server: the message says that it is left so. When the
/// release is all that lacks, the operator's option lets a start go on
/// to it; when other things lack too, the option alone does not, and
/// the message says that instead. By then the run directory has been
/// made, so the message says what was not done, not that nothing was.
fn not_served(why: &skyzfs_ublk::features::Unsupported) -> String {
    let option = skyzfs_ublk::features::ASYNC_DELETE_OPTION;
    let then = if why.only_by_release() {
        format!(", and a start with {option} goes on to it")
    } else if why.by_release() {
        format!(
            ". {option} alone does not let a start go on: this kernel lacks more than the \
             release"
        )
    } else {
        String::new()
    };
    format!(
        "not served: {why}. No device was added, taken over or deleted, and the volume's lock \
         and store were not opened: a device of this volume that waits for a server, if there \
         is one, still waits{then}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A start refused for the kernel says what was not done and that a
    /// waiting device of the volume is left as it is. The option is said
    /// to let a start go on only when the release is all that lacks;
    /// when a feature lacks too (Linux 6.1 to 6.4 lack the features
    /// command), it is said not to be enough.
    #[test]
    fn a_refused_start_says_a_waiting_device_is_left() {
        use skyzfs_ublk::features::{check, ASYNC_DELETE_OPTION};
        let not_done = "No device was added, taken over or deleted, and the volume's lock and \
                        store were not opened";
        let goes_on = format!("a start with {ASYNC_DELETE_OPTION} goes on to it");
        let alone = format!("{ASYNC_DELETE_OPTION} alone does not let a start go on");

        let by_release = not_served(&check(Some(u64::MAX), "6.10.3", false).unwrap_err());
        assert!(by_release.starts_with("not served: "), "{by_release}");
        assert!(by_release.contains(not_done), "{by_release}");
        assert!(by_release.contains("waits for a server, if there is one, still waits"));
        assert!(by_release.ends_with(&goes_on), "{by_release}");
        assert!(!by_release.contains(&alone), "{by_release}");
        assert!(!by_release.contains("Nothing was touched"), "{by_release}");

        // Linux 6.4: no features command, and a release below the floor.
        for lacking_too in [None, Some(0)] {
            let both = not_served(&check(lacking_too, "6.4.0", false).unwrap_err());
            assert!(both.contains(not_done), "{both}");
            assert!(both.contains(&alone), "{both}");
            assert!(!both.contains(&goes_on), "{both}");
        }

        let by_flag = not_served(&check(Some(0), "7.0.0", false).unwrap_err());
        assert!(by_flag.contains(not_done), "{by_flag}");
        assert!(by_flag.contains("still waits"), "{by_flag}");
        assert!(!by_flag.contains(&goes_on), "{by_flag}");
        assert!(!by_flag.contains(&alone), "{by_flag}");
    }

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

    /// A run directory whose path goes through a directory that does
    /// not exist and back (`missing/../pool/run`) is predicted to land
    /// where `missing` would be made, and lands in `pool`. The check
    /// before the making is fooled; the check of what was made refuses
    /// it. With the real `create_dir_all` in a temporary directory, and
    /// a file system type keyed on the real path: everything under
    /// `pool` is "ZFS".
    #[test]
    fn a_run_directory_that_lands_elsewhere_than_predicted_is_refused_once_made() {
        const EXT4: u64 = 0xef53;
        let keep = tempfile::tempdir().expect("a directory");
        let cwd = keep.path().canonicalize().expect("real");
        let pool = cwd.join("pool");
        std::fs::create_dir(&pool).expect("pool");
        let kind = |path: &Path| -> io::Result<u64> {
            Ok(if path.starts_with(&pool) {
                ZFS_SUPER_MAGIC
            } else {
                EXT4
            })
        };
        let dir = Path::new("missing/../pool/run/v-1");
        // The prediction alone passes: the old check, and why it was
        // not enough.
        assert_eq!(off_zfs(dir, "the run directory", &cwd, kind), Ok(()));
        let refused = make_off_zfs(
            dir,
            "the run directory",
            &cwd,
            || std::fs::create_dir_all(cwd.join(dir)),
            kind,
        )
        .expect_err("made on ZFS");
        assert!(refused.contains("was made on ZFS"), "{refused}");
        assert!(refused.contains("the empty directory is left"), "{refused}");
        // What it left: the directories, empty.
        assert!(pool.join("run/v-1").is_dir());
        assert_eq!(
            std::fs::read_dir(pool.join("run/v-1"))
                .expect("listing")
                .count(),
            0
        );
        // A directory predicted on ZFS is refused before anything is
        // made.
        let made = std::cell::Cell::new(false);
        let refused = make_off_zfs(
            Path::new("pool/other/v-1"),
            "the run directory",
            &cwd,
            || {
                made.set(true);
                Ok(())
            },
            kind,
        )
        .expect_err("predicted on ZFS");
        assert!(refused.contains("is on ZFS"), "{refused}");
        assert!(!made.get() && !pool.join("other").exists());
        // One that lands where it was predicted is made and passes.
        assert_eq!(
            make_off_zfs(
                Path::new("run/v-1"),
                "the run directory",
                &cwd,
                || std::fs::create_dir_all(cwd.join("run/v-1")),
                kind,
            ),
            Ok(())
        );
        // A link planted where the directory is to be made: the same.
        std::os::unix::fs::symlink(&pool, cwd.join("link")).expect("link");
        std::fs::remove_dir(pool.join("run/v-1")).expect("remove");
        let refused = make_off_zfs(
            Path::new("fresh/v-2"),
            "the run directory",
            &cwd,
            || {
                // Between the check and the making, `fresh` becomes a
                // link into the pool.
                std::os::unix::fs::symlink(&pool, cwd.join("fresh"))?;
                std::fs::create_dir_all(cwd.join("fresh/v-2"))
            },
            kind,
        )
        .expect_err("made behind a link");
        assert!(refused.contains("was made on ZFS"), "{refused}");
        // The volume's directory through a directory that does not
        // exist is refused as it is, and not judged by a directory
        // above it: `missing/../pool/vol` with a run directory under
        // `missing`, whose making would make the path resolve onto
        // the pool afterwards.
        std::fs::create_dir(pool.join("vol")).expect("vol");
        let volume = Path::new("absent/../pool/vol");
        assert_eq!(
            off_zfs(volume, "the volume's directory", &cwd, kind),
            Ok(()),
            "the old check, which judged the directory above"
        );
        let refused = existing_off_zfs(volume, "the volume's directory", &cwd, kind)
            .expect_err("it does not resolve");
        assert!(refused.contains("does not exist"), "{refused}");
        assert_eq!(
            make_off_zfs(
                Path::new("absent/run/v-1"),
                "the run directory",
                &cwd,
                || std::fs::create_dir_all(cwd.join("absent/run/v-1")),
                kind,
            ),
            Ok(())
        );
        // Now it resolves, onto the pool: refused as what it is.
        let refused =
            existing_off_zfs(volume, "the volume's directory", &cwd, kind).expect_err("on ZFS");
        assert!(refused.contains("is on ZFS"), "{refused}");
        assert_eq!(
            existing_off_zfs(Path::new("run"), "the volume's directory", &cwd, kind),
            Ok(())
        );
        // An answer that cannot be had for what was made is a refusal.
        let unknown = make_off_zfs(
            Path::new("run/v-3"),
            "the run directory",
            &cwd,
            || std::fs::create_dir_all(cwd.join("run/v-3")),
            |path: &Path| {
                if path.ends_with("v-3") {
                    Err(io::Error::other("no answer"))
                } else {
                    Ok(EXT4)
                }
            },
        )
        .expect_err("no answer");
        assert!(unknown.contains("cannot be told"), "{unknown}");
    }

    /// A queue that says stop at its first wait, or panics there.
    struct Ending {
        panics: bool,
    }

    impl skyzfs_ublk::pump::Queue for Ending {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(
            &mut self,
            _fetched: &mut Vec<skyzfs_ublk::pump::Fetched>,
        ) -> Result<skyzfs_ublk::pump::Wake, skyzfs_ublk::pump::QueueDown> {
            assert!(!self.panics, "the queue panics, as the test asks");
            Ok(skyzfs_ublk::pump::Wake::Stop)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<skyzfs_blk::Buf> {
            Some(skyzfs_blk::Buf::zeroed(blocks))
        }
        fn commit(&mut self, _tag: u16, _result: i32, _buf: Option<skyzfs_blk::Buf>) {}
        fn pause(&mut self) {}
        fn flush(&mut self) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    struct NoRecord;

    impl skyzfs_ublk::pump::AckRecord for NoRecord {
        fn advance(&mut self, _position: u64) -> io::Result<()> {
            Ok(())
        }
    }

    /// The volume's lock is still held when the queue's thread has
    /// ended, by a stop and by a panic alike: the store comes back to
    /// the thread that joins, which is where the failure's cleanup
    /// runs, and the lock is free only when that thread drops it.
    #[test]
    fn the_volumes_lock_outlives_the_queues_thread_until_the_joiner_lets_go() {
        use skyzfs_local::fs::FsDir;
        use skyzfs_local::store::{OpenOptions, Store, StoreConfig, StoreError};
        for panics in [false, true] {
            let keep = tempfile::tempdir().expect("a directory");
            let dir = || FsDir::new(keep.path()).expect("the directory");
            let config = StoreConfig {
                volume: *b"a volume on disk",
                size_mib: 8,
                base: skyzfs_blk::Base::Empty,
                align: 4096,
                segment_size: 8 << 20,
                write_unit: skyzfs_local::format::Identity::system_write_unit(),
            };
            let served = Store::create(dir(), &config, OpenOptions::default())
                .expect("create")
                .serve_with(None)
                .expect("the worker");
            let turns = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let thread = std::thread::spawn({
                let turns = turns.clone();
                move || {
                    pumped(
                        Pump::new(Ending { panics }, served, NoRecord),
                        |_, ended| {
                            if ended {
                                turns.fetch_add(1, Ordering::SeqCst);
                            }
                        },
                    )
                }
            });
            let (stopped, store) = thread.join().expect("the panic is caught on the thread");
            assert_eq!(stopped.is_none(), panics, "{stopped:?}");
            assert_eq!(turns.load(Ordering::SeqCst), u32::from(!panics));
            // The queue's thread is gone. This is the moment of the
            // failure's cleanup: nobody else gets the lock.
            assert!(
                matches!(Store::lock_volume(&dir()), Err(StoreError::Locked)),
                "panics: {panics}: the lock was free after the queue's thread ended"
            );
            drop(store);
            assert!(Store::lock_volume(&dir()).is_ok(), "panics: {panics}");
        }
    }
    /// A queue that hands over three requests at its first wait and
    /// notes what is committed.
    #[derive(Default)]
    struct Three {
        waits: u32,
        committed: Arc<std::sync::Mutex<Vec<(u16, i32)>>>,
    }

    impl skyzfs_ublk::pump::Queue for Three {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(
            &mut self,
            fetched: &mut Vec<skyzfs_ublk::pump::Fetched>,
        ) -> Result<skyzfs_ublk::pump::Wake, skyzfs_ublk::pump::QueueDown> {
            use skyzfs_ublk::translate::{op, Desc};
            self.waits += 1;
            if self.waits == 1 {
                for (tag, op) in [(0, op::WRITE), (1, op::FLUSH), (2, op::READ)] {
                    fetched.push(skyzfs_ublk::pump::Fetched {
                        tag,
                        desc: Desc {
                            op_flags: u32::from(op),
                            nr_sectors: if op == op::FLUSH { 0 } else { 8 },
                            start_sector: 0,
                        },
                    });
                }
            }
            Ok(skyzfs_ublk::pump::Wake::Work)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<skyzfs_blk::Buf> {
            Some(skyzfs_blk::Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, result: i32, _buf: Option<skyzfs_blk::Buf>) {
            self.committed.lock().expect("lock").push((tag, result));
        }
        fn pause(&mut self) {}
        fn flush(&mut self) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    /// A target that takes requests and never answers one: a store
    /// whose disk has gone.
    struct Silent(Vec<skyzfs_blk::Request>);

    impl skyzfs_blk::BlockTarget for Silent {
        fn geometry(&self) -> skyzfs_blk::Geometry {
            skyzfs_blk::Geometry::new(8, 4, false).expect("a geometry")
        }
        fn submit(&mut self, request: skyzfs_blk::Request) -> Result<(), skyzfs_blk::SubmitError> {
            self.0.push(request);
            Ok(())
        }
        fn poll(&mut self, _out: &mut Vec<skyzfs_blk::Completion>) -> usize {
            0
        }
        fn wait(&mut self, _out: &mut Vec<skyzfs_blk::Completion>, _: Duration) -> usize {
            0
        }
    }

    impl skyzfs_ublk::pump::Positions for Silent {
        fn acknowledged(&self) -> Option<u64> {
            None
        }
        fn gone(&self) -> bool {
            false
        }
    }

    #[derive(Clone)]
    struct Unused;

    impl skyzfs_ublk::pump::AckRecord for Unused {
        fn advance(&mut self, _position: u64) -> io::Result<()> {
            Ok(())
        }
    }

    /// A failed volume holds no request (M1.3d), on the queue's thread
    /// as `serve` runs it: with three requests in a store that never
    /// answers, nothing is committed while the volume has not failed;
    /// the turn after the store said so, all three are committed with
    /// the I/O error and the queue is woken; and the call after the
    /// next wait, which is what sends them, tells the main thread that
    /// they are answered, and how many.
    /// Once the volume has failed, the record of what was
    /// acknowledged stores nothing and is not asked: the turn that
    /// sees the failure does not go on to write a position, and what
    /// waited for it is answered with the error afterwards.
    #[test]
    fn the_record_of_what_was_acknowledged_refuses_once_the_volume_failed() {
        use skyzfs_ublk::pump::AckRecord;
        #[derive(Clone, Default)]
        struct Counted(Arc<AtomicU64>);
        impl AckRecord for Counted {
            fn advance(&mut self, _: u64) -> io::Result<()> {
                self.0.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
        }
        let asked = Arc::new(AtomicU64::new(0));
        let flags = Arc::new(Flags::default());
        let mut record = UnlessFailed {
            record: Counted(asked.clone()),
            flags: flags.clone(),
        };
        assert!(record.advance(10).is_ok());
        assert_eq!(asked.load(Ordering::SeqCst), 1);
        flags.failed.store(true, Ordering::SeqCst);
        assert!(record.advance(20).is_err());
        assert!(record.clone().advance(30).is_err());
        assert_eq!(asked.load(Ordering::SeqCst), 1, "the record was asked");
    }

    /// The failure line gives a count of answered requests only when
    /// the queue's thread said that it answered them and that a wait
    /// sent the answers. A thread that ended without saying so (the
    /// pump ended in the turn that saw the failure, or a panic) gets
    /// its own words and no count: before, it was reported as "N
    /// requests were answered before the delete", and "0 requests"
    /// after a panic.
    #[test]
    fn the_failure_line_counts_answers_only_when_the_queues_thread_said_so() {
        let said = answers_said(true, false, 7);
        assert!(said.starts_with("7 requests the daemon held were answered"));
        // The flag decides, also when the thread has ended since.
        assert_eq!(answers_said(true, true, 7), said);
        let ended = answers_said(false, true, 7);
        assert!(ended.contains("the queue's thread ended before it said"));
        assert!(ended.contains("the driver fails what was left at the delete"));
        assert!(!ended.contains('7') && !ended.contains("were answered"));
        assert_eq!(answers_said(false, false, 7), "the delete has returned");
    }

    #[test]
    fn the_queues_thread_answers_what_a_silent_store_holds_once_the_volume_failed() {
        let committed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue = Three {
            committed: committed.clone(),
            ..Three::default()
        };
        let (queue, ack) = Delayed::new(queue, Unused, None, None, || {});
        let mut pump = Pump::new(queue, Silent(Vec::new()), ack);
        let flags = Flags::default();
        let woken = std::cell::Cell::new(0);
        let wake = || woken.set(woken.get() + 1);
        for _ in 0..50 {
            assert_eq!(pump.turn(), None);
            answer_if_failed(&mut pump, &flags, &wake);
        }
        assert!(committed.lock().expect("lock").is_empty());
        assert_eq!(pump.target().0.len(), 3, "all three are in the store");
        assert!(!flags.answered.load(Ordering::SeqCst));

        flags.failed.store(true, Ordering::SeqCst);
        assert_eq!(pump.turn(), None);
        answer_if_failed(&mut pump, &flags, &wake);
        let mut answers = committed.lock().expect("lock").clone();
        answers.sort_unstable();
        assert_eq!(answers, [(0, -5), (1, -5), (2, -5)]);
        assert_eq!(
            woken.get(),
            1,
            "the queue is woken so that its wait sends them"
        );
        assert!(
            !flags.answered.load(Ordering::SeqCst),
            "not said before a wait has sent them"
        );

        assert_eq!(pump.turn(), None);
        answer_if_failed(&mut pump, &flags, &wake);
        assert!(flags.answered.load(Ordering::SeqCst));
        assert_eq!(flags.answers.load(Ordering::SeqCst), 3);
        assert_eq!(woken.get(), 1);
        assert_eq!(committed.lock().expect("lock").len(), 3);
        assert_eq!(
            pump.target().0.len(),
            3,
            "the store was handed nothing more"
        );
    }
}

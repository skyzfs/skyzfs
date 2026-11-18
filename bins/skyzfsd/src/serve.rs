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
//!    failure note, the store's open, the device.
//! 4. The store is started (its sync of the home image, the drop of
//!    the kernel's cache of it, its I/O threads), the queue's thread is started
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
//! | the volume failed while it was served, or at its start | 3 |
//! | the queue went away, or the target did | 1 |
//! | the kernel's driver broke its rule: it handed over a tag the queue does not have, or one whose request was still out ([`Stopped::BrokenRule`]) | 7 |
//! | the kernel's driver refused one of serve's answers, a commit ([`Stopped::CommitRefused`]) | 8 |
//! | serve gave the device's start up: the driver refused a first fetch of the queue ([`Stopped::FetchRefused`]), or the start, or the end of the recovery, had no answer within [`START_LIMIT`] | 9 |
//!
//! After a broken rule nothing more is answered on the queue, since an
//! answer could complete the wrong request. The breach is the
//! driver's, not the store's: the volume is not failed, no note is
//! written, and the device is neither stopped nor deleted, so that it
//! waits for the next server as after a kill, and the kernel issues
//! again what was not answered (M1.3ac).
//!
//! A refused commit ends serve the same way (M1.3ac part 3), with
//! status 8, so that a reader tells "the driver refused our answer"
//! from "the driver broke its rule". The refused answer took no
//! effect: its request is still the server's in the driver's eyes,
//! and the next server is handed exactly that request again and
//! completes it (the control rig: a read, a write and a flush; a
//! delete instead fails it at once). Nothing more is answered on the
//! queue, and the refusal says nothing against the store, so the
//! volume is not failed. Also after a signal, with 8 and not 0: the
//! stop was then not orderly, and a 0 would tell the next start that
//! it was clean while a request was left unanswered. A refused commit
//! while the volume has failed is only
//! counted, and serve ends as a failed volume (3): the device is
//! deleted, which answers the refused request.
//!
//! # A start that is given up (M1.3ah)
//!
//! The driver answers the device's start (START_DEV), and the end of
//! its recovery at a take-over (END_USER_RECOVERY), only once every
//! tag's first fetch has arrived, and it waits for them with no limit
//! of its own (testing's M1.T36, on every image: the command's io_uring
//! worker waits in the driver's `ublk_wait_dev_ready_and_lock`, run
//! 43591915443). A fetch it refused never arrives. So the main thread
//! does not send the start: a thread of its own does, and the main
//! thread waits for its answer for at most [`START_LIMIT`] (30 s),
//! looking every 50 ms whether the volume has failed and whether the
//! queue's thread has ended, in that order.
//!
//! - A volume that fails meanwhile goes the way of a failure (exit 3),
//!   as one that fails later: the failure comes first.
//! - A queue's thread that ended because the driver refused a first
//!   fetch ([`Stopped::FetchRefused`]) gives the start up at once: it
//!   can never be answered. Any other end of the queue ends serve as it
//!   would after the start (the table above).
//! - No answer within the limit gives the start up too. At a start
//!   the device is deleted first, while the queue's thread still
//!   answers: on a kernel whose START_DEV reads the partition table
//!   holding the device's mutex, a stopped queue would leave the
//!   delete waiting behind it for good. Then the queue's thread is told
//!   to stop and waited for two seconds at most. A failure flagged
//!   before the delete or by the end of that wait takes the failure's
//!   way (exit 3), as everywhere.
//! - Each command that a start given up for want of an answer sends
//!   about the device (the delete, the reading of its info, the open
//!   that asks whether a client holds it) is sent from a thread of its
//!   own, and all of them are waited for [`GIVE_UP_LIMIT`] (10 s) at
//!   most, all told. On such a kernel, with a disk under the store that
//!   stopped answering, the delete would wait behind the mutex for
//!   good. A command with no answer by then is left with the driver,
//!   which may still carry it out, and nothing more is sent: the line
//!   says that the device could not be deleted and names the command.
//!
//! When serve gives the start up, it says why on one line, and ends
//! with status 9:
//!
//! - at a start, the device holds no pool: serve added it and never
//!   started it. It is deleted, with its note, under the volume's lock,
//!   and the line says whether the delete was sent.
//! - at a take-over, the device holds the pool's requests. It is left
//!   as after a kill: it waits for the next server, which is handed
//!   again what it holds.
//! - the volume is not failed and no failure note is written: neither
//!   a refused fetch nor a start that does not answer says anything
//!   against the store.
//!
//! A supervisor should retry the serve once: what kept the start from
//! being answered may not be there the next time. A second 9 in a row
//! is for a person (`skyzfsd --help` says the same).
//!
//! A `SIGTERM` or `SIGINT` that arrives during the wait is acted on
//! when the wait ends: after a start that answered, it stops serve as
//! usual; after a start given up, serve ends with 9 all the same, since
//! nothing was served. So a stop is late by at most [`START_LIMIT`],
//! plus the two seconds for the queue's thread, plus [`GIVE_UP_LIMIT`]
//! for the kernel's commands: 42 s, under half of systemd's default
//! stop timeout (90 s). Whether the process's end then waits for the
//! driver to let go of a command it holds (the start's, or one with no
//! answer) is the kernel's, and not shown here. A supervisor whose stop
//! timeout is shorter may kill serve during a start: the device is then
//! as after any kill.
//!
//! After an orderly stop the device stays and waits for the next
//! server. When the volume fails, the failure is noted with the image's
//! reason (a write or a sync of it), the device's note is removed and the device is
//! deleted ([`lifecycle::fail_serving`]),
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
//! bounds it; and a queue's thread that is itself stuck answers
//! nothing, so the delete may then wait in the driver, and the daemon
//! with it.
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
//!   whose sync of the image meets the failed disk: the volume has
//!   failed, and it goes the way of a failure while served (the notes,
//!   the delete, exit 3).
//! - The design gives no figure for how long a start may take. 30 s
//!   ([`START_LIMIT`]) is this daemon's, from the rigs' times (its doc
//!   gives them), and stays under a supervisor's usual stop timeout.
//! - The start's thread is not waited for once the start is given up.
//!   Its command is in the kernel, and the process's end ends it. That
//!   a delete answers a START_DEV that waits, and that the process's
//!   end answers an END_USER_RECOVERY that waits, is read from the
//!   driver and not shown by these tests. Testing's early look at this
//!   change (run 43597163595, one pass on Ubuntu 26.04) saw both: each
//!   refused start ended with 9 in 82 ms. Its rig is to show it on
//!   every image.

use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use skyzfs_local::fs::FsDir;
use skyzfs_local::inplace::{
    CutFigures, CutHoles, CutRegion, CutsTakenUp, HoldCuts, InPlace, IntentAtStart,
    IntentFigures, OpenOptions, Opened, StartError,
};
use skyzfs_local::store::{FailureObserver, Waker, Zeroes};
use skyzfs_local::volume::{VolumeError, LEGACY_NAME, VOLUME_NAME};
use skyzfs_ublk::lifecycle::{self, Control, Reason, Records, Stop, Undeleted, Wanted};
use skyzfs_ublk::pump::{Pump, Stopped};
use skyzfs_ublk::records::{self, Files};
use skyzfs_ublk::status::{
    self, Board, ChannelShown, ChannelStateShown, CutHolesShown, CutStateShown, HeaderSyncShown,
    HeldShown, HolesShown, IntentShown, Pace, Status, StoreShown, ZeroesShown,
};
use skyzfs_ublk::volume::StoreVolume;
use skyzfs_ublk_sys::control::{block_device, Kernel};
use skyzfs_ublk_sys::queue::{Device, Open};
use skyzfs_ublk_sys::signals::{self, Signal};
use skyzfs_ublk_sys::{Wake, FLAGS};

use crate::cli::{Exit, Serve};
use crate::commands::{GONE_RETRIES, LOCK_PAUSE, LOCK_RETRIES};
use crate::control::{self, handler, Channel, ChannelFigures};
use crate::delay::Delayed;
use crate::timing::{TimedControl, TimedVolume};
use crate::upload::{
    AtStart, Ended, Leased, Publishes, Uploading, Uploads, END_LIMIT, RELEASE_LIMIT,
};
use skyzfs_upload::Desk;

/// The store that serves the volume: the in-place store, on the
/// queue's thread. (Until M1.3t a choice between it and the log store
/// behind its worker.)
pub type Target = InPlace<FsDir>;

/// The reason the failure note gives for a volume that failed while it
/// was served or at its start: a write or a sync of the image (code 7).
/// The log store's reasons (codes 1 to 3) are no longer given, and the
/// note still reads them.
const FAILURE: Reason = Reason::ImageFailed;

/// The held cuts' state, in the volume's run directory (`--hold-cuts`).
pub const CUTS_STATE: &str = "cuts.state";

/// How often a start asks again about a device of its own that is
/// still shown as live: 50 times, 100 ms apart, 5 s.
pub const LIVE_RETRIES: u32 = 50;

/// How often the status file is written at most: every 100 ms.
pub const STATUS_INTERVAL_MS: u64 = 100;

/// How long `ready` waits at most for the status thread's first write
/// of this instance.
const READY_WAITS_FOR_STATUS: Duration = Duration::from_secs(5);

/// How long serve waits for the device's start (START_DEV, or the end
/// of a recovery at a take-over) to answer before it gives the start up
/// with status 9 (M1.3ah). The driver answers a start only once every
/// tag's first fetch has arrived, with no limit of its own (testing's
/// M1.T36): without one here a fetch the driver lost would hold serve,
/// and its device, for good.
///
/// In two runs of the daemon rig (43581985630 and 43585843902, three
/// images each) the start took at most 12 ms (90 starts) and the end of
/// a recovery at most 1 ms (54). In the replay rig's daemon jobs
/// (43585843885) the whole of a serve, from its launch to `ready`, took
/// at most 0.2 s (243 serves). 30 s is more than two thousand times the
/// longest start seen, and leaves room for a start whose partition scan
/// reads from a slow disk.
/// It stays a third of systemd's default stop timeout (90 s): a SIGTERM
/// during the start is acted on when this wait ends. What serve does
/// after it is in the module doc.
pub const START_LIMIT: Duration = Duration::from_secs(30);

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

/// The line the start says when it could not read some of the labels'
/// rings (`InPlace::rings_unread`), or `None` when it read them all: why
/// no cut opens as itself for a while, which `status` alone does not
/// tell from a rewind import.
pub fn rings_unread_said(unread: &[(u64, String)]) -> Option<String> {
    let (first, why) = unread.first()?;
    Some(format!(
        "the start could not read {} of the labels' 4 uberblock rings (the first at byte {first}: \
         {why}): no cut opens as itself until every slot of each has been written again, which \
         takes ZFS up to one transaction group a slot",
        unread.len()
    ))
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
pub fn pumped<Q, T>(
    mut pump: Pump<Q, T>,
    mut each: impl FnMut(&mut Pump<Q, T>, bool),
) -> (Option<Stopped>, T)
where
    Q: skyzfs_ublk::pump::Queue,
    T: skyzfs_ublk::pump::PumpTarget,
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
fn answer_if_failed<Q, T>(pump: &mut Pump<Delayed<Q>, T>, flags: &Flags, wake: &dyn Fn())
where
    Q: skyzfs_ublk::pump::Queue,
    T: skyzfs_ublk::pump::PumpTarget,
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
    /// `SIGUSR2` arrived: the test build reads a command about held cuts
    /// (`cut_commands.rs`).
    command: AtomicBool,
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
    pump: &mut Pump<Q, Target>,
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
    _pump: &mut Pump<Q, Target>,
    _flags: &Flags,
    _fail_after: &mut Option<u64>,
) {
}

/// The store's state as the status file shows it, with the pump's
/// times: how write-zeroes reach the image, the I/O threads' queue and
/// the count of cuts.
fn store_shown(store: &Target, pump: skyzfs_ublk::pump::PumpTiming) -> StoreShown {
    StoreShown {
        zeroes: zeroes_shown(store.zeroes()),
        pump,
        io_queue: Some(store.io_queue()),
        cuts: Some(store.cuts()),
        held: Some(held_shown(store)),
        intent: Some(intent_shown(store)),
        // The uploader's thread posts its own (`Board::post_upload`),
        // and the channel its own (`Board::post_channel`), which the
        // board keeps.
        upload: None,
        channel: None,
    }
}

/// The intent map as the status file says it.
fn intent_shown(store: &Target) -> IntentShown {
    match store.intent_figures() {
        Some(figures) => intent_shown_of(figures, store.intent_at_start()),
        None => IntentShown::default(),
    }
}

/// The status's intent map from the store's figures and what its start
/// did with the map.
fn intent_shown_of(figures: IntentFigures, at_start: Option<&IntentAtStart>) -> IntentShown {
    let (state, named_at_start) = intent_at_start_shown(at_start, figures.regions);
    IntentShown {
        state,
        region_bytes: figures.region_bytes,
        regions: figures.regions,
        named: figures.named,
        named_at_start,
        syncs: figures.syncs,
        waits: figures.waits,
        header: Some(HeaderSyncShown {
            on: figures.header_sync,
            syncs: figures.header_syncs,
        }),
    }
}

/// What the start did with the intent map, as the status file says it,
/// and how many of the volume's `regions` it named then: a map taken up
/// names what it named, and a new map names every region.
fn intent_at_start_shown(at_start: Option<&IntentAtStart>, regions: u64) -> (CutStateShown, u64) {
    match at_start {
        Some(IntentAtStart::TakenUp { named, .. }) => (CutStateShown::TakenUp, *named),
        Some(IntentAtStart::Begun(_)) => (CutStateShown::Begun, regions),
        None => (CutStateShown::Off, 0),
    }
}

/// The held cuts' options of a start that holds cuts (`--hold-cuts`, or
/// a volume that `uploads`), with their state at `path` and the region of
/// the intent map that `--intent-region-mib` gives, if any.
fn hold_cuts_of(
    serve: &Serve,
    uploads: bool,
    path: std::path::PathBuf,
    boot: [u8; 16],
) -> Option<HoldCuts> {
    (serve.hold_cuts || uploads).then(|| HoldCuts {
        region: CutRegion::Named(path),
        boot,
        bound: None,
        intent_region: serve.intent_region_mib.map(|mib| mib << 20),
        intent_header_sync: serve.intent_header_sync,
    })
}

/// The held cuts as the status file says them.
fn held_shown(store: &Target) -> HeldShown {
    held_shown_of(store.cuts_at_start(), store.cut_figures())
}

/// The held cuts as the status file says them, from what the store's
/// start did with the cut state and the state's figures.
fn held_shown_of(at_start: Option<&CutsTakenUp>, figures: CutFigures) -> HeldShown {
    let state = match at_start {
        None => CutStateShown::Off,
        Some(CutsTakenUp::TakenUp) => CutStateShown::TakenUp,
        Some(CutsTakenUp::Begun(_)) => CutStateShown::Begun,
    };
    HeldShown {
        state,
        held: figures.held,
        copied: figures.copied,
        bound: figures.bound,
        abandoned: figures.abandoned,
        replaced: Some(figures.replaced),
        holes: Some(HolesShown {
            state: match figures.holes {
                CutHoles::None => CutHolesShown::None,
                CutHoles::Held => CutHolesShown::Held,
                CutHoles::Dropped => CutHolesShown::Dropped,
            },
            chunks: figures.hole_chunks,
        }),
    }
}

fn zeroes_shown(zeroes: Zeroes) -> ZeroesShown {
    match zeroes {
        Zeroes::Untried => ZeroesShown::Untried,
        Zeroes::Deallocated => ZeroesShown::Deallocated,
        Zeroes::Written => ZeroesShown::Written,
        Zeroes::WrittenDirect => ZeroesShown::WrittenDirect,
    }
}

/// Put the pump's counters up for the status file. A result the test
/// build keeps back is a request the kernel still waits for: it counts
/// as in flight.
fn post<Q: skyzfs_ublk::pump::Queue>(board: &Board, pump: &mut Pump<Delayed<Q>, Target>) {
    let mut counters = pump.counters();
    counters.in_flight += pump.queue().waiting() as u64;
    board.post(counters);
    let timing = pump.timing();
    board.post_store(store_shown(pump.target(), timing));
}

/// Run `serve`. Prints for itself: the ready line has to be out when
/// the device serves, not when the daemon ends.
pub fn serve(serve: &Serve) -> Exit {
    // A quick look before anything else; the open under the lock reads
    // and checks the volume file itself.
    if !serve.dir.join(VOLUME_NAME).is_file() {
        if serve.dir.join(LEGACY_NAME).is_file() {
            say(format_args!(
                "{}: {}",
                serve.dir.display(),
                VolumeError::Legacy
            ));
        } else {
            say(format_args!("{} holds no volume", serve.dir.display()));
        }
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

/// Where the host's name is read, for the lease's holder unless
/// `--holder` is given: the kernel's name of the host (of its UTS
/// namespace), what `hostname` prints. `$HOSTNAME` is a shell's, and not
/// always in a daemon's environment.
const HOSTNAME: &str = "/proc/sys/kernel/hostname";

/// Where the machine's id is read, for the lease's holder unless
/// `--holder` is given: machine-id(5), 32 hexadecimal digits written once
/// when the system is installed.
const MACHINE_ID: &str = "/etc/machine-id";

/// Whose the lease in object storage is: `--holder`, or
/// [`default_holder`].
fn holder_of(serve: &Serve) -> Result<String, String> {
    match &serve.holder {
        Some(holder) => Ok(holder.clone()),
        None => default_holder(Path::new(HOSTNAME), Path::new(MACHINE_ID)),
    }
}

/// The lease's holder unless `--holder` is given:
/// `<host's name>:<machine id>`, read from `hostname` and `machine_id`.
///
/// A holder's name must mean one daemon: the lease cannot tell two
/// daemons of one name apart, and both would upload (#117's review, N1).
/// A host's name alone is not unique to a node, so the machine's id is
/// added (the lead's decision in M2.4c-2). A start with no machine id,
/// or one that is not 32 hexadecimal digits ("uninitialized" while the
/// system first boots) or all zeros, is refused until `--holder` is
/// given: the conservative reading, since a name made up here could not
/// be the same at the next start. A machine id does not tell apart two
/// copies of one image (a cloned VM copies it): the usage and the
/// status say that such copies must not be served under one name.
fn default_holder(hostname: &Path, machine_id: &Path) -> Result<String, String> {
    let read = std::fs::read_to_string(hostname).map_err(|e| {
        format!(
            "the host's name ({}), part of the lease's holder unless --holder is given: {e}",
            hostname.display()
        )
    })?;
    let name = read.trim_end_matches('\n');
    let read = std::fs::read_to_string(machine_id).map_err(|e| {
        format!(
            "the machine's id ({}), part of the lease's holder unless --holder is given: {e}; \
             give --holder, a name that means this daemon alone",
            machine_id.display()
        )
    })?;
    let id = read.trim_end_matches('\n');
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) || id.bytes().all(|b| b == b'0')
    {
        return Err(format!(
            "the machine's id ({}) is {id:?}, not 32 hexadecimal digits that are not all zero: \
             the lease's holder cannot be made from it; give --holder, a name that means this \
             daemon alone",
            machine_id.display()
        ));
    }
    let holder = format!("{name}:{id}");
    crate::cli::check_holder(&holder).map_err(|why| {
        format!("{why}: the host's name and the machine's id cannot name the lease's holder; give --holder")
    })?;
    Ok(holder)
}

/// The uploads of the volume in `serve.dir`, if it has a place in object
/// storage (the file `remote`, M2.4c): read before anything is touched,
/// so that a file that cannot be read, or options of a lease for a
/// volume with no place, refuse the serve with nothing to undo.
fn uploads_of(serve: &Serve) -> Result<Option<Uploads>, String> {
    let Some(remote) = crate::remote::read(&serve.dir)? else {
        if serve.holder.is_some() || serve.lease_ttl_s.is_some() || serve.serve_without_upload {
            return Err(format!(
                "{} has no place in object storage (it was created without --endpoint): \
                 --holder, --lease-ttl-s and --serve-without-upload are for a volume that has",
                serve.dir.display()
            ));
        }
        if serve.hooks.kill_at.is_some() {
            return Err(format!(
                "{} has no place in object storage: --kill-at-upload would never be reached",
                serve.dir.display()
            ));
        }
        return Ok(None);
    };
    let ttl = serve
        .lease_ttl_s
        .unwrap_or(crate::upload::DEFAULT_LEASE_TTL_S);
    Uploads::new(&remote, holder_of(serve)?, ttl, serve.serve_without_upload)
        .map(Some)
        .map_err(|e| format!("the uploader's runtime: {e}"))
}

/// Why the channel answers `no-upload`: the volume has no place in
/// object storage, or another holder has its lease.
fn no_upload_why(uploads: Option<&Uploads>) -> String {
    match uploads.and_then(|u| u.started().found()) {
        _ if uploads.is_none() => {
            "the volume has no place in object storage (it was created without --endpoint)".into()
        }
        Some((AtStart::Another { holder, epoch }, _)) => format!(
            "another holder has the volume's lease ({holder} at epoch {epoch}): this daemon \
             serves it with no upload (--serve-without-upload)"
        ),
        _ => "this daemon has no uploader".into(),
    }
}

/// Where the channel's figures go: the board (`Board::post_channel`).
type ChannelPost = Arc<dyn Fn(ChannelShown) + Send + Sync>;

/// Make the channel at `path` (M2.5-2): answered by the uploader through
/// `requests` where `answers` holds the attachment it serves, and refused
/// `no-upload` with `answers`' reason otherwise. A channel that cannot be
/// made is said, and the volume is served all the same: serving it comes
/// before publishing it (this module's reading; the design leaves the
/// channel to M2.5).
///
/// `post` is given what the channel is before it is opened, `failed` if
/// it cannot be, and its figures after each connection it counts. The
/// handler is answered through `record` (the request log's, in a build
/// with the test hooks).
fn open_channel(
    path: &Path,
    answers: Result<[u8; 16], String>,
    requests: tokio::sync::mpsc::Sender<skyzfs_upload::Control>,
    post: ChannelPost,
    record: impl FnOnce(control::Handler) -> control::Handler,
) -> Option<Channel> {
    let no_upload = ChannelStateShown::NoUpload;
    let (handler, state) = match answers {
        Ok(attachment) => match handler::uploader(attachment, requests, control::ANSWER_LIMIT) {
            Ok(handler) => (handler, ChannelStateShown::Uploads),
            Err(e) => (
                handler::no_upload(format!("the channel's runtime: {e}")),
                no_upload,
            ),
        },
        Err(why) => (handler::no_upload(why), no_upload),
    };
    post(ChannelShown {
        state,
        ..ChannelShown::default()
    });
    let posting = {
        let post = Arc::clone(&post);
        Box::new(move |figures: ChannelFigures| {
            post(ChannelShown {
                state,
                answered: figures.answered,
                peer: figures.peer,
                malformed: figures.malformed,
            })
        })
    };
    match Channel::open_posting(path, record(handler), posting) {
        Ok(channel) => Some(channel),
        Err(e) => {
            say(format_args!(
                "warning: the channel {} could not be made ({e}): the volume is served, and \
                 nothing can be published while it is",
                path.display()
            ));
            post(ChannelShown {
                state: ChannelStateShown::Failed,
                ..ChannelShown::default()
            });
            None
        }
    }
}

/// What the end of the uploader's thread says.
fn upload_ended_said(ended: &Ended) -> String {
    match ended {
        Ended::Released => "lease: let go".into(),
        Ended::Kept => "lease: kept until its time runs out".into(),
        Ended::NotReleased(why) => {
            format!("lease: not let go ({why}): it is kept until its time runs out")
        }
        Ended::NoAnswer => format!(
            "lease: the uploader's thread did not end within {} ms, and the lease is kept \
             until its time runs out",
            (RELEASE_LIMIT + END_LIMIT).as_millis()
        ),
    }
}

fn run(serve: &Serve) -> Result<Exit, String> {
    let boot = boot(serve).map_err(|e| format!("reading the boot id: {e}"))?;
    let mut uploads = uploads_of(serve)?;
    // The test build's record of the requests (`--request-log`): opened
    // before the volume or a device is touched, so that a path that
    // cannot be written refuses the serve with nothing to undo.
    #[cfg(feature = "test-hooks")]
    let request_log = match &serve.hooks.request_log {
        Some(path) => Some(
            crate::record::open(path)
                .map_err(|e| format!("the request log {}: {e}", path.display()))?,
        ),
        None => None,
    };
    // Every write's bytes beside it (`--request-log-content whole`).
    #[cfg(feature = "test-hooks")]
    let contents = {
        let data = match &serve.hooks.request_log {
            Some(path) if serve.hooks.request_log_whole => {
                let mut data = path.clone().into_os_string();
                data.push(".data");
                let data = std::path::PathBuf::from(data);
                Some(
                    crate::record::open(&data)
                        .map_err(|e| format!("the request log's data {}: {e}", data.display()))?,
                )
            }
            _ => None,
        };
        crate::record::Contents::new(data)
    };
    let mut records = Files::new(&serve.dir, &serve.run_dir, boot, LOCK_PAUSE)
        .map_err(|e| format!("{}: {e}", serve.dir.display()))?;
    if serve.allow_any_filesystem {
        records
            .create_run_dirs()
            .map_err(|e| format!("{}: {e}", serve.run_dir.display()))?;
    } else {
        // The run directory was judged before by where it would be
        // made; here it is made and judged again as what it is, before
        // the store is opened or a device is touched. The status file
        // and the failure note are both in this one directory. The volume's own directory needs
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

    let mut options = OpenOptions {
        io: serve.in_place_io,
        ..OpenOptions::default()
    };
    // The held cuts' state, in the volume's run directory, which is
    // tmpfs where the run directory is `/run`: kept while the machine
    // is up, never written back, gone at a reboot (M2.2b). The store's
    // open maps it, allocated and locked, under the volume's lock and
    // before a device is touched, and is refused where it cannot.
    let cuts_path = records.volume_run_dir().join(CUTS_STATE);
    options.hold_cuts = hold_cuts_of(serve, uploads.is_some(), cuts_path.clone(), boot);
    options.on_failure = Some(FailureObserver::new({
        let (flags, wake) = (flags.clone(), wake.clone());
        move || {
            flags.failed.store(true, Ordering::SeqCst);
            wake.wake();
        }
    }));
    let began = std::time::Instant::now();
    // The store's start counts the home image's cached pages after it
    // drops them (`cachestat`).
    let volume =
        StoreVolume::new(&serve.dir, options).with_cache_count(skyzfs_ublk_sys::cached_pages);
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
    // The lease in object storage is taken under the volume's lock, after
    // the store's open and before a device is looked for (M2.4c).
    let started = {
        let mut leased = Leased::new(&mut volume, Opened::<FsDir>::share_lock, || {
            uploads
                .as_mut()
                .map_or(Ok(()), |uploads| uploads.start().take())
        });
        lifecycle::start(&mut kernel, &mut records, &mut leased, &wanted)
    };
    let mut ready = match started {
        Ok(ready) => ready,
        Err(Stop::Refused(why)) => {
            say(why);
            return Ok(Exit::LeaseHeld);
        }
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
    // What the start's take of the lease found, and how long it took: said
    // on a line of its own, and in no figure of the start's line.
    let mut lease_time = Duration::ZERO;
    if let Some((found, spent)) = uploads.as_ref().and_then(|u| u.started().found()) {
        say(found.said(spent));
        lease_time = spent;
    }
    // The main thread holds the volume's lock from here until this
    // function returns, by a hold of its own on the lock the store
    // took. Whatever becomes of the store (it goes to the queue's
    // thread, and a panic there, in the pump or outside it, drops it
    // there), the lock is not free before the main thread has done the
    // cleanup after a failure: the failure note, the device's note and
    // the delete of the device by its number. Without it a detach or
    // another serve could act on the attachment in between.
    let _lock = ready.store.share_lock();
    let (number, shape) = (ready.number, ready.shape);
    let identity = ready.store.identity();
    // The attachment this instance serves, for the channel's answers and
    // the uploader's publishes (M2.5-2): the record's nonce, which the
    // start has just written or, at a take-over, kept.
    let attachment = match records.attachment() {
        Ok(Some(record)) => Ok(record.nonce),
        Ok(None) => Err("the volume has no attachment record".to_string()),
        Err(e) => Err(format!("the volume's attachment record: {e}")),
    };
    // What the log store's start counted and the status file and the
    // start's line still carry: zeros, with no log (M1.3t). Their
    // fields and words stay until the status file's next version, so
    // that what reads them reads on.
    let recovered = 0u64;
    let (log_bytes, log_records) = (0u64, 0u64);
    let took_over = ready.took_over;
    if took_over {
        // The mark the held cuts' state holds is kept only on a device
        // taken over (`InPlace::take_up_cut_state`).
        ready.store.device_taken_over();
    }
    let to_device = began.elapsed();
    let waker = Waker::new({
        let wake = wake.clone();
        move || wake.wake()
    });
    // The home image is synced and its cache dropped, and the I/O
    // threads start; a flush's sync runs on the image's helper thread,
    // which wakes the queue's thread when it ends, as do the I/O threads
    // with their results.
    let served = match ready.store.start(Some(waker)) {
        Ok(store) => store,
        // The disk failed under the start: the volume has failed, and it
        // goes the way of a volume that fails while it is served: the
        // failure note, the device's note, the delete of the device,
        // exit 3. The device is this instance's and is not started yet;
        // it holds no request of this instance's. The store's failure
        // marker comes last: it needs the disk that has just failed and
        // may never end, and the note must not wait for it.
        Err(StartError::Failed(start)) => {
            let (failure, written, deleted) =
                lifecycle::fail_serving(&mut kernel, &mut records, number, identity, FAILURE);
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
        // The start serves only what the disk holds (design §4.1): where
        // it cannot show that, it serves nothing. The volume has not
        // failed, and the device waits as after any start that fails
        // after the lifecycle added it.
        Err(refused @ (StartError::CacheKept(_) | StartError::Io(_) | StartError::Intent(_))) => {
            return Err(format!(
                "the in-place store: {refused}. Nothing was served and the volume has not failed"
            ))
        }
    };
    if let Some(said) = rings_unread_said(served.rings_unread()) {
        say(said);
    }

    match served.cuts_at_start() {
        None => {}
        Some(CutsTakenUp::TakenUp) => say(format_args!(
            "held cuts: the state in {} was taken up",
            cuts_path.display()
        )),
        Some(CutsTakenUp::Begun(why)) => say(format_args!(
            "held cuts: a new state in {}: {why}",
            cuts_path.display()
        )),
    }
    match served.intent_at_start() {
        None => {}
        Some(IntentAtStart::TakenUp { named, regions }) => say(format_args!(
            "intent map: taken up, naming {named} of {regions} regions (a new cut state counts \
             only their blocks as changed)"
        )),
        Some(IntentAtStart::Begun(why)) => say(format_args!(
            "intent map: begun anew with every region named (a new cut state counts the whole \
             volume as changed): {why}"
        )),
    }
    // The cut state the uploader's publishes are judged against, from the
    // start (it reads the store's own before each request).
    let cut_state = served.cut_state_id();
    let queue_began = std::time::Instant::now();
    let device = Device::open(&Open {
        number,
        size: shape.size,
        take_over: took_over,
    })
    .map_err(|e| format!("opening device {number}: {e}"))?;
    // The test build's request log hears of the store's writes applied,
    // its flushes' barriers and its cuts.
    #[cfg(feature = "test-hooks")]
    let served = {
        let mut served = served;
        if let Some(file) = &request_log {
            served.watch(crate::record::watcher(
                crate::record::Lines::another(file),
                contents.clone(),
            ));
        }
        served
    };
    // Its `publish` lines: the channel's and the uploader's, each on a
    // handle of its own.
    #[cfg(feature = "test-hooks")]
    let (channel_lines, ended_lines) = (
        request_log.as_ref().map(crate::record::Lines::another),
        request_log.as_ref().map(crate::record::Lines::another),
    );
    // What the start did with the held cuts' state, for the request
    // log's `cuts` line.
    #[cfg(feature = "test-hooks")]
    let cuts_said = held_shown(&served).state.said();
    let board = Arc::new(Board::new());
    // The store as the start left it, on the board before the status
    // thread's first write, which would otherwise show nothing of it
    // until the queue's thread has had its first turn: on an idle
    // device, the first request.
    board.post_store(store_shown(&served, Default::default()));
    // The upload as the start's lease left it, before the uploader's
    // first turn; with another holder's lease, all there is to show.
    if let Some(shown) = uploads.as_ref().and_then(|u| u.started().shown()) {
        board.post_upload(shown);
    }
    let no_upload = no_upload_why(uploads.as_ref());
    let mut uploads = uploads.filter(|u| u.started().found().is_some_and(|(f, _)| f.uploads()));
    // The uploader's calls on the store, answered on the queue's thread
    // between its turns (M2.4c, decision 8).
    let desk = uploads.as_ref().map(|_| {
        let wake = wake.clone();
        Desk::new(move || wake.wake())
    });
    let mut fail_after = serve.hooks.fail_log_after;
    let delay = serve.hooks.commit_delay_ms.map(Duration::from_millis);
    let hold = serve.hooks.hold;
    #[cfg(feature = "test-hooks")]
    let refuse = serve.hooks.refuse;
    let locked = !serve.allow_unlocked_memory;
    #[cfg(feature = "test-hooks")]
    let size = shape.size;
    // The test build's commands about held cuts, on `SIGUSR2`.
    #[cfg(feature = "test-hooks")]
    let mut commands = crate::cut_commands::Commands::new(records.volume_run_dir().to_owned())
        .uploading(uploads.is_some());
    let body = {
        let (board, flags, wake) = (board.clone(), flags.clone(), wake.clone());
        let desk = desk.clone();
        queue_body(move |queue| {
            let again = wake.clone();
            // Next to the kernel: the one commit sent with no
            // buffer, for the driver to refuse.
            #[cfg(feature = "test-hooks")]
            let queue = crate::refuse::Refusing::new(queue, refuse);
            // Below the delays: the record says what the kernel
            // handed over and what it was given.
            #[cfg(feature = "test-hooks")]
            let queue =
                crate::record::Recorded::new(queue, request_log, number, size, cuts_said, contents);
            let queue = Delayed::new(queue, delay, hold, move || wake.wake());
            // The store goes back to the main thread with the
            // outcome: the volume's lock is held until the main
            // thread lets go of it.
            pumped(Pump::new(queue, served), |pump, ended| {
                answer_if_failed(pump, &flags, &|| again.wake());
                post(&board, pump);
                if !ended {
                    faults(pump, &flags, &mut fail_after);
                    // A write of a held cut goes on at the next turn,
                    // which an idle device would not give without the
                    // wake.
                    #[cfg(feature = "test-hooks")]
                    if commands.turn(flags.command.swap(false, Ordering::SeqCst), pump.target()) {
                        again.wake();
                    }
                }
                // The uploader's call, if one waits, answered from the
                // store; at the queue's end the desk is closed and a call
                // that waits is dropped, with the upload under way.
                if let Some(desk) = &desk {
                    if ended {
                        desk.close();
                    } else {
                        desk.answer(pump.target());
                    }
                }
            })
        })
    };
    // The test build's refused fetch (`--refuse-one-fetch`): tag 0's
    // first fetch is sent with no buffer, at the kind of start named.
    #[cfg(feature = "test-hooks")]
    let serving = {
        let (tag, said) =
            crate::refuse::fetch_to_refuse(serve.hooks.refuse_fetch, took_over, number);
        if let Some(said) = said {
            say(said);
        }
        match tag {
            Some(tag) => device.serve_with_fetch_refused_for_tests(&wake, locked, tag, body),
            None => device.serve(&wake, locked, body),
        }
    };
    #[cfg(not(feature = "test-hooks"))]
    let serving = device.serve(&wake, locked, body);
    let serving = serving.map_err(|e| format!("the queue of device {number}: {e}"))?;
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
                board.publish_with_store(
                    Pace::new(STATUS_INTERVAL_MS),
                    |counters, shown, failures| {
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
                            start_ms: start_ms.load(Ordering::SeqCst),
                            shown: *shown,
                        };
                        status::write(&path, &status)
                    },
                )
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

    // The start, from a thread of its own, waited for at most
    // START_LIMIT (M1.3ah): the driver answers it only once every tag's
    // first fetch has arrived, and a fetch it refused never arrives.
    // The start's thread is not waited for once the start is given up:
    // its command ends with the process.
    let start_began = Instant::now();
    let device = Arc::new(device);
    let (answered, answer) = mpsc::channel();
    std::thread::Builder::new()
        .name("skyzfsd-start".into())
        .spawn({
            let device = device.clone();
            move || {
                let _ = answered.send(device.start());
            }
        })
        .map_err(|e| format!("the start's thread: {e}"))?;
    let waited = wait_for_start(
        &answer,
        || serving.is_finished(),
        || flags.failed.load(Ordering::SeqCst),
        START_LIMIT,
    );
    // The uploader's thread, once the device serves. Dropped at any end
    // but exit 0, which lets the lease go: the drop keeps it.
    let mut uploading: Option<Uploading> = None;
    // The daemon's channel, once the device serves; closed before the
    // uploader's thread ends, and at any other end by its drop.
    let mut channel: Option<Channel> = None;
    let failed_at_start = match waited {
        StartWait::Answered(Ok(())) => false,
        StartWait::Answered(Err(e)) => {
            stopper.stop();
            let _ = serving.join();
            finish(&board);
            return Err(format!("starting device {number}: {e}"));
        }
        StartWait::Failed => true,
        StartWait::QueueEnded => {
            let (stopped, _store) = joined(serving, number);
            finish(&board);
            if flags.failed.load(Ordering::SeqCst) {
                return Ok(failed_at_end(&mut kernel, &mut records, number, identity));
            }
            return match stopped? {
                Stopped::FetchRefused { tag, errno } => {
                    let deleted = give_up_start(&mut kernel, &mut records, number, took_over);
                    say(gave_up_said(
                        number,
                        took_over,
                        &GaveUp::FetchRefused { tag, errno },
                        deleted.as_ref(),
                    ));
                    Ok(Exit::NotStarted)
                }
                other => ended(number, false, other),
            };
        }
        StartWait::NoAnswer => {
            // The queue's thread ends at its stop. It is waited for a
            // stated time and no longer, as everything after a start
            // that does not answer. What it hands back is kept to the
            // end, so that the store is dropped after the line.
            let (exit, _kept) = give_up_unanswered(
                &mut kernel,
                KernelCommands,
                GIVE_UP_LIMIT,
                &mut records,
                number,
                took_over,
                identity,
                || flags.failed.load(Ordering::SeqCst),
                || {
                    stopper.stop();
                    for _ in 0..ANSWER_LOOKS {
                        if serving.is_finished() {
                            break;
                        }
                        std::thread::sleep(ANSWER_LOOK);
                    }
                    let kept = serving.is_finished().then(|| joined(serving, number));
                    finish(&board);
                    kept
                },
            );
            return Ok(exit);
        }
    };
    if !failed_at_start {
        let started = Instant::now();
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
        // the store's state at the start, which the main thread put on the
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
        let devices =
            to_device.saturating_sub(volume.lock + volume.position + volume.recovery + lease_time);
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
        // that whenever the queue's thread posts a change.) The
        // uploader's thread, from here on, posts what the status shows of
        // the upload.
        let (requests, inbox) = tokio::sync::mpsc::channel(handler::INBOX);
        let mut answers = Err(no_upload);
        if let (Some(begun), Some(desk)) = (uploads.take(), desk) {
            let board = board.clone();
            let kill = serve.hooks.kill_at;
            #[cfg(feature = "test-hooks")]
            let told: crate::upload::Told = match ended_lines {
                Some(mut lines) => {
                    Box::new(move |name: &str, progress: &skyzfs_upload::Progress| {
                        lines.publish(
                            crate::record::PublishLine::Ended,
                            name,
                            &control::Reply::of(progress),
                        );
                    })
                }
                None => Box::new(|_, _| {}),
            };
            #[cfg(not(feature = "test-hooks"))]
            let told: crate::upload::Told = Box::new(|_, _| {});
            let publishes = Publishes {
                attachment: attachment.as_ref().ok().copied(),
                state: cut_state,
                inbox,
                told,
            };
            match begun.begin(desk, publishes, move |shown| board.post_upload(shown), kill) {
                Ok(thread) => {
                    uploading = Some(thread);
                    answers = attachment.clone();
                }
                Err(e) => {
                    say(format_args!(
                        "warning: the uploader's thread could not start ({e}): the volume is \
                         served with no upload, and its lease is kept until its time runs out"
                    ));
                    answers = Err(format!("the uploader's thread could not start: {e}"));
                }
            }
        }
        let to_board = board.clone();
        // The test build's request log hears of the channel's publishes.
        #[cfg(feature = "test-hooks")]
        let recorded = move |handler: control::Handler| match channel_lines {
            Some(lines) => crate::record::channel(handler, lines),
            None => handler,
        };
        #[cfg(not(feature = "test-hooks"))]
        let recorded = |handler: control::Handler| handler;
        channel = open_channel(
            &records.volume_run_dir().join(control::SOCKET),
            answers,
            requests,
            Arc::new(move |shown| to_board.post_channel(shown)),
            recorded,
        );
    }

    #[derive(PartialEq)]
    enum Why {
        Stop,
        Failed,
        Ended,
    }
    // A volume that failed during the start goes the failure's way, as
    // one that fails later: the failure comes first (M1.3ah).
    let why = if failed_at_start {
        Why::Failed
    } else {
        loop {
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
                Some(Signal::Command) => {
                    flags.command.store(true, Ordering::SeqCst);
                    wake.wake();
                }
                None => {}
            }
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
        // device. Without the note that start goes by the store's marker
        // (`volume.failed`) alone and counts the volume as held for the whole boot: safe,
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
            lifecycle::note_failure(&mut kernel, &mut records, number, identity, FAILURE);
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
    // No request is taken from here: the uploader's thread may be ending.
    if let Some(channel) = channel.take() {
        if !channel.close() {
            say("the channel's thread was in a request at the end, and answers nothing more");
        }
    }
    if why == Why::Stop {
        stopper.stop();
    }
    // The store comes back with the outcome and is dropped when this
    // function returns. The volume's lock does not depend on it:
    // `_lock` holds it past the failure's cleanup below also when the
    // queue's thread panicked outside the pump or the queue was never
    // made, and the store was dropped there.
    let (stopped, _store) = joined(serving, number);
    finish(&board);
    // A failure can arrive with the queue's end.
    if flags.failed.load(Ordering::SeqCst) {
        return Ok(failed_at_end(&mut kernel, &mut records, number, identity));
    }
    let exit = ended(number, why == Why::Stop, stopped?)?;
    // The lease is let go at exit 0 and kept at every other (decision 4).
    if let Some(thread) = uploading.take() {
        let release = exit == Exit::Done;
        say(upload_ended_said(
            &thread.end(release, RELEASE_LIMIT + END_LIMIT),
        ));
    }
    Ok(exit)
}

/// The body of the queue's thread, as the binding takes it: for every
/// lifetime of the queue it is given. A closure made apart from the
/// call that takes it is not inferred so without this.
fn queue_body<R, F>(body: F) -> F
where
    F: for<'q> FnOnce(skyzfs_ublk_sys::queue::KernelQueue<'q>) -> R + Send + 'static,
{
    body
}

/// The queue's thread of device `number` joined: how its pump ended,
/// or why that is not known, and the store if it came back.
fn joined<S>(
    serving: skyzfs_ublk_sys::queue::Serving<(Option<Stopped>, S)>,
    number: u32,
) -> (Result<Stopped, String>, Option<S>) {
    match serving.join() {
        Ok((Some(stopped), store)) => (Ok(stopped), Some(store)),
        Ok((None, store)) => (
            Err(format!("the queue's thread of device {number} panicked")),
            Some(store),
        ),
        Err(e) => (Err(e.to_string()), None),
    }
}

/// A failure that came with the queue's end: the failure note, the
/// device's note and the device, and the line; exit 3.
fn failed_at_end<C: Control, R: Records>(
    kernel: &mut C,
    records: &mut R,
    number: u32,
    identity: [u8; 16],
) -> Exit {
    let (failure, written) = lifecycle::note_failure(kernel, records, number, identity, FAILURE);
    let deleted = lifecycle::delete_failed(kernel, records, number);
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
    Exit::Failed
}

/// How serve ends once the queue's thread of device `number` has ended
/// with `stopped` and no failure came with it: 0 after an asked stop
/// that ended in order; 7, with its line, when the kernel's driver
/// broke its rule, and 8, with its line, when it refused a commit (in
/// both the device is left as it is, waiting for the next server); an
/// error otherwise.
fn ended(number: u32, asked_to_stop: bool, stopped: Stopped) -> Result<Exit, String> {
    match stopped {
        Stopped::Orderly if asked_to_stop => Ok(Exit::Done),
        Stopped::BrokenRule { tag } => {
            say(broken_rule_said(number, tag));
            Ok(Exit::BrokenRule)
        }
        Stopped::CommitRefused { tag, errno } => {
            say(commit_refused_said(number, tag, errno));
            Ok(Exit::CommitRefused)
        }
        Stopped::FetchRefused { tag, errno } => {
            say(fetch_refused_said(number, tag, errno));
            Ok(Exit::NotStarted)
        }
        other => Err(format!("the queue of device {number} ended: {other:?}")),
    }
}

/// What serve says when the kernel's driver broke its rule.
fn broken_rule_said(number: u32, tag: u16) -> String {
    format!(
        "the kernel handed over tag {tag} of device {number}, which the queue does not have or \
         whose request was still out, against the driver's rule: nothing more is answered on \
         this queue, the volume is not failed, and device {number} waits for the next server, \
         as after a kill"
    )
}

/// What serve says when the kernel's driver refused a commit.
fn commit_refused_said(number: u32, tag: u16, errno: i32) -> String {
    format!(
        "the kernel refused serve's answer on tag {tag} of device {number} ({}): nothing more is \
         answered on this queue, the volume is not failed, and device {number} waits for the \
         next server, which is handed again what was not answered",
        io::Error::from_raw_os_error(errno)
    )
}

/// What serve says to a supervisor at every exit 9.
const NOT_STARTED_ADVICE: &str =
    "exit 9: a supervisor should retry the serve once; a second 9 is for a person";

/// What serve says when the kernel's driver refused a fetch of a queue
/// that was serving: nothing does that once the queue's fetches have
/// all arrived, which the start waits for, so the device is left as it
/// is.
fn fetch_refused_said(number: u32, tag: u16, errno: i32) -> String {
    format!(
        "the kernel refused the fetch of tag {tag} on queue 0 of device {number} ({}): nothing \
         more is answered on this queue, the volume is not failed, and device {number} is left \
         as it is; {NOT_STARTED_ADVICE}",
        io::Error::from_raw_os_error(errno)
    )
}

/// What ended serve's wait for the device's start.
#[derive(Debug)]
enum StartWait {
    /// The start returned, with what it answered.
    Answered(io::Result<()>),
    /// The volume failed before the start answered. A failure is a
    /// failure: it goes the failure's way (exit 3) and not that of a
    /// start given up.
    Failed,
    /// The queue's thread ended before the start answered: a fetch the
    /// driver refused, or another end of the queue.
    QueueEnded,
    /// No answer within the limit.
    NoAnswer,
}

/// Wait for the start's `answer`, sent from the start's own thread,
/// for at most `limit`, looking every [`LOOK`] whether the volume has
/// `failed` and whether the queue has `ended`, in that order. An answer
/// that came is taken first, whatever else holds.
fn wait_for_start(
    answer: &mpsc::Receiver<io::Result<()>>,
    ended: impl Fn() -> bool,
    failed: impl Fn() -> bool,
    limit: Duration,
) -> StartWait {
    let until = Instant::now() + limit;
    loop {
        match answer.try_recv() {
            Ok(answered) => return StartWait::Answered(answered),
            Err(mpsc::TryRecvError::Disconnected) => {
                return StartWait::Answered(Err(io::Error::other(
                    "the start's thread ended with no answer",
                )))
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
        if failed() {
            return StartWait::Failed;
        }
        if ended() {
            return StartWait::QueueEnded;
        }
        let now = Instant::now();
        if now >= until {
            return StartWait::NoAnswer;
        }
        match answer.recv_timeout(LOOK.min(until - now)) {
            Ok(answered) => return StartWait::Answered(answered),
            Err(mpsc::RecvTimeoutError::Disconnected | mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Why serve gave the device's start up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GaveUp {
    /// The driver refused the first fetch of `tag`.
    FetchRefused { tag: u16, errno: i32 },
    /// The start had no answer within [`START_LIMIT`].
    NoAnswer,
}

/// What becomes of the device when serve gives its start up: a device
/// this serve added was never started and holds no pool, and is deleted,
/// its note first; one it took over holds the pool's waiting I/O, which
/// a delete would fail, and is left waiting for the next server, as
/// after a kill. `None` when nothing was deleted.
///
/// The start's thread may still be waiting in START_DEV, and libublk
/// holds the device's control object locked for the whole of it. The
/// delete does not wait behind that: `control` is the lifecycle's,
/// which sends the delete through the binding's own open of the
/// control device and its own ring (`raw::delete`), never through the
/// device's control object.
fn give_up_start<C: Control, R: Records>(
    control: &mut C,
    records: &mut R,
    number: u32,
    took_over: bool,
) -> Option<Result<(), Undeleted>> {
    (!took_over).then(|| lifecycle::delete_unstarted(control, records, number))
}

/// How long a start given up for want of an answer waits, all told,
/// for the kernel's commands that it sends about the device: the
/// delete, the reading of its info, and the open that asks whether a
/// client holds it (M1.3ah, after #87's review and the lead's word on
/// it). The design gives no figure; this is the daemon's reading.
///
/// Such a command can wait in the driver for as long as the start did.
/// On a kernel whose START_DEV reads the partition table holding the
/// device's mutex, and whose disk under the store stopped answering,
/// the read never ends, and the delete waits behind the mutex for good.
/// By the same reading GET_PARAMS waits for that mutex, and the open of
/// the block device for the disk's open mutex, which the scan holds.
/// None of this is shown on a kernel here.
///
/// An asynchronous delete returns once it is under way: testing's
/// early look saw the whole of a start given up, its delete included,
/// end 82 ms after the start began. 10 s is more than a hundred times
/// that, and keeps the whole of a start given up, 30 s and 2 s and
/// this, under half of systemd's default stop timeout (90 s).
pub const GIVE_UP_LIMIT: Duration = Duration::from_secs(10);

/// The kernel's commands that a start given up sends about the device,
/// as functions of its number alone, so that each can be sent from a
/// thread of its own and waited for within a limit ([`Bounded`]).
/// [`KernelCommands`] sends them to the driver.
trait Commands: Clone + Send + 'static {
    /// [`Control::delete`].
    fn delete(&self, number: u32) -> io::Result<()>;
    /// [`Control::info`].
    fn info(&self, number: u32) -> io::Result<Option<lifecycle::DeviceInfo>>;
    /// [`Control::client_holds`].
    fn client_holds(&self, number: u32) -> Option<bool>;
}

/// The driver's commands, as [`Kernel`] sends them.
#[derive(Debug, Clone, Copy)]
struct KernelCommands;

impl Commands for KernelCommands {
    fn delete(&self, number: u32) -> io::Result<()> {
        skyzfs_ublk_sys::raw::delete(number)
    }
    fn info(&self, number: u32) -> io::Result<Option<lifecycle::DeviceInfo>> {
        skyzfs_ublk_sys::control::info(number)
    }
    fn client_holds(&self, number: u32) -> Option<bool> {
        skyzfs_ublk_sys::control::client_holds(number)
    }
}

/// The lifecycle's control for a start given up: the delete, the
/// reading of the device's info and the open that asks whether a
/// client holds it are each sent from a thread of their own, and waited
/// for until a deadline, [`GIVE_UP_LIMIT`] after this was made.
///
/// - A command with no answer by the deadline answers an error that
///   names it (`TimedOut`). It stays with the driver, which may still
///   carry it out, and its thread is not waited for: it ends with the
///   process, as the start's thread does. Whether the process's end
///   waits for the driver to let go of such a command is the kernel's,
///   and not shown here.
/// - Once the deadline has passed, nothing more is sent: each command
///   answers at once with an error that says so. A command sent after
///   one that had no answer would wait behind the same mutex.
/// - A client's hold that cannot be asked is unknown (`None`), as when
///   the open does not tell.
///
/// The give-up sends no other command; the others are `inner`'s.
struct Bounded<'k, C, K> {
    inner: &'k mut C,
    commands: K,
    limit: Duration,
    deadline: Instant,
}

impl<'k, C, K: Commands> Bounded<'k, C, K> {
    fn new(inner: &'k mut C, commands: K, limit: Duration) -> Self {
        Self {
            inner,
            commands,
            limit,
            deadline: Instant::now() + limit,
        }
    }

    /// `command`, named `name`, sent from a thread of its own and
    /// waited for until the deadline.
    fn send<T: Send + 'static>(
        &self,
        number: u32,
        name: &str,
        command: impl FnOnce(&K) -> T + Send + 'static,
    ) -> io::Result<T> {
        let limit = said_limit(self.limit);
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{name} of device {number} was not sent: the {limit} that a start given up \
                     waits for the kernel's commands had run out"
                ),
            ));
        }
        let (answered, answer) = mpsc::channel();
        let commands = self.commands.clone();
        std::thread::Builder::new()
            .name("skyzfsd-command".into())
            .spawn(move || {
                let _ = answered.send(command(&commands));
            })?;
        match answer.recv_timeout(left) {
            Ok(answer) => Ok(answer),
            Err(mpsc::RecvTimeoutError::Timeout) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "{name} of device {number} had no answer within the {limit} that a start \
                     given up waits for the kernel's commands: it stays with the driver, which \
                     may still carry it out"
                ),
            )),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(io::Error::other(format!(
                "{name} of device {number}: its thread ended with no answer"
            ))),
        }
    }
}

impl<C: Control, K: Commands> Control for Bounded<'_, C, K> {
    fn features(&mut self) -> io::Result<Option<u64>> {
        self.inner.features()
    }
    fn release(&mut self) -> io::Result<String> {
        self.inner.release()
    }
    fn info(&mut self, number: u32) -> io::Result<Option<lifecycle::DeviceInfo>> {
        self.send(number, "GET_DEV_INFO and GET_PARAMS", move |k| {
            k.info(number)
        })?
    }
    fn add(&mut self, number: u32, shape: &lifecycle::Shape) -> Result<(), lifecycle::AddError> {
        self.inner.add(number, shape)
    }
    fn disown(&mut self, number: u32) {
        self.inner.disown(number);
    }
    fn delete(&mut self, number: u32) -> io::Result<()> {
        self.send(number, "DEL_DEV_ASYNC", move |k| k.delete(number))?
    }
    fn begin_recovery(&mut self, number: u32) -> io::Result<()> {
        self.inner.begin_recovery(number)
    }
    fn client_holds(&mut self, number: u32) -> Option<bool> {
        let name = format!("the exclusive open of {}", block_device(number));
        self.send(number, &name, move |k| k.client_holds(number))
            .ok()
            .flatten()
    }
}

/// A limit as the lines say it: whole seconds as such, anything else
/// in milliseconds.
fn said_limit(limit: Duration) -> String {
    if limit.subsec_nanos() == 0 {
        format!("{} s", limit.as_secs())
    } else {
        format!("{} ms", limit.as_millis())
    }
}

/// The start of device `number` had no answer within [`START_LIMIT`]:
/// serve gives it up, and says so, or takes the failure's way. `stop`
/// stops the queue's thread, waits for it a stated time, and hands back
/// what that thread handed back (the store among it) if it ended.
/// Every command sent to the kernel about the device goes through
/// [`Bounded`], with `commands` and `limit` ([`GIVE_UP_LIMIT`]), so that
/// none of them holds serve without a limit.
///
/// The order:
/// 1. A failure already flagged takes the failure's way: its note,
///    the device's note and the device ([`failed_at_end`]), exit 3.
/// 2. At a start, the device is deleted ([`give_up_start`]) while the
///    queue's thread still answers. On a kernel whose START_DEV reads
///    the partition table holding the device's mutex, a stopped queue
///    would leave that read unanswered, and the delete waiting behind
///    the mutex for good (#87's review). At a take-over nothing is
///    deleted. A delete with no answer within the limit is said as a
///    delete that failed: the device "could not be deleted", with the
///    command named, and exit 9.
/// 3. The queue's thread is stopped.
/// 4. A failure flagged by then also takes the failure's way, and not
///    9 with "the volume is not failed" (#87's review, Codex's P1). At
///    a start that way deletes the device a second time: the note is
///    gone by then, and a device that is gone reads as deleted. That
///    the driver answers a second delete of a device it is deleting
///    without an error is read from it, not shown here. Were it to
///    answer with one, the failure's line would say that the device
///    could not be deleted, which sends a person to look: the
///    conservative reading. After a delete with no answer, nothing more
///    is sent: the failure note is written, and the line says the
///    device could not be deleted.
/// 5. Otherwise the line, and exit 9.
///
/// What the queue's thread handed back is handed to the caller and not
/// dropped here: dropping the store waits for its I/O with no limit, so
/// it comes after the line, as on serve's other ends.
#[allow(clippy::too_many_arguments)]
fn give_up_unanswered<C: Control, R: Records, K: Commands, S>(
    kernel: &mut C,
    commands: K,
    limit: Duration,
    records: &mut R,
    number: u32,
    took_over: bool,
    identity: [u8; 16],
    failed: impl Fn() -> bool,
    stop: impl FnOnce() -> Option<S>,
) -> (Exit, Option<S>) {
    let kernel = &mut Bounded::new(kernel, commands, limit);
    if failed() {
        let exit = failed_at_end(kernel, records, number, identity);
        return (exit, stop());
    }
    let deleted = give_up_start(kernel, records, number, took_over);
    let kept = stop();
    if failed() {
        return (failed_at_end(kernel, records, number, identity), kept);
    }
    say(gave_up_said(
        number,
        took_over,
        &GaveUp::NoAnswer,
        deleted.as_ref(),
    ));
    (Exit::NotStarted, kept)
}

/// What serve says when it gives the start of device `number` up:
/// why, what became of the device (`deleted`, from [`give_up_start`]),
/// that the volume is not failed, and what a supervisor should do.
fn gave_up_said(
    number: u32,
    took_over: bool,
    why: &GaveUp,
    deleted: Option<&Result<(), Undeleted>>,
) -> String {
    let why = match why {
        GaveUp::FetchRefused { tag, errno } => format!(
            "the kernel refused the first fetch of tag {tag} on queue 0 of device {number} ({}), \
             and {} waits for every tag's fetch with no limit: serve gives it up",
            io::Error::from_raw_os_error(*errno),
            if took_over {
                "the end of its recovery"
            } else {
                "the device's start"
            }
        ),
        GaveUp::NoAnswer => format!(
            "{} of device {number} had no answer within {} s: serve gives it up",
            if took_over {
                "the end of the recovery"
            } else {
                "the start"
            },
            START_LIMIT.as_secs()
        ),
    };
    let device = match deleted {
        None => format!(
            "device {number} is left waiting for the next server, as after a kill, with the I/O \
             it holds"
        ),
        Some(Ok(())) => {
            format!("device {number}, which this serve added and never started, is deleted")
        }
        Some(Err(undeleted)) => format!(
            "device {number}, which this serve added and never started, is to be deleted: {}",
            undeleted.said(number)
        ),
    };
    format!("{why}; {device}; the volume is not failed; {NOT_STARTED_ADVICE}")
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

    fn serving(dir: &Path, extra: &str) -> Serve {
        let line = format!("serve --dir {} {extra}", dir.display());
        match crate::cli::parse(line.split_whitespace().map(String::from)) {
            Ok(crate::cli::Command::Serve(serve)) => serve,
            other => panic!("{line}: {other:?}"),
        }
    }

    /// The place is the file `remote` in the volume's directory (M2.4c):
    /// with none, nothing is uploaded and the options of a lease are
    /// refused; a file that cannot be read refuses the serve, naming it;
    /// with one, the uploads are made, and nothing asked of object
    /// storage until the lease is taken.
    #[test]
    fn the_uploads_come_from_the_file_remote_and_the_lease_options_need_one() {
        let keep = tempfile::tempdir().expect("a directory");
        let dir = keep.path();
        assert!(uploads_of(&serving(dir, "")).expect("no place").is_none());
        for extra in [
            "--holder node-a",
            "--lease-ttl-s 5",
            "--serve-without-upload",
        ] {
            let refused = uploads_of(&serving(dir, extra))
                .err()
                .expect("refused with no place");
            assert!(refused.contains("no place in object storage"), "{refused}");
        }
        #[cfg(feature = "test-hooks")]
        {
            let refused = uploads_of(&serving(dir, "--kill-at-upload put:1"))
                .err()
                .expect("a kill point never reached");
            assert!(refused.contains("--kill-at-upload"), "{refused}");
        }
        std::fs::write(dir.join(crate::remote::REMOTE), "skyzfs-remote 2\n").expect("written");
        let refused = uploads_of(&serving(dir, ""))
            .err()
            .expect("an unreadable place");
        assert!(refused.contains(crate::remote::REMOTE), "{refused}");
        assert!(refused.contains("version"), "{refused}");

        let remote =
            crate::remote::Remote::new("http://127.0.0.1:1", "volumes", "pg-1").expect("a place");
        crate::remote::write(dir, &remote).expect("written");
        let uploads = uploads_of(&serving(dir, "--holder node-a --lease-ttl-s 5"))
            .expect("read")
            .expect("uploads");
        assert!(uploads.started().found().is_none(), "nothing taken yet");
        assert!(uploads.started().shown().is_none());
        assert_eq!(uploads.started().ttl_ms(), 5_000, "--lease-ttl-s");
        let uploads = uploads_of(&serving(dir, "--holder node-a"))
            .expect("read")
            .expect("uploads");
        assert_eq!(
            uploads.started().ttl_ms(),
            crate::upload::DEFAULT_LEASE_TTL_S * 1000,
            "30 s unless --lease-ttl-s is given"
        );
        assert_eq!(crate::upload::DEFAULT_LEASE_TTL_S, 30);
    }

    /// The lease's holder is `--holder`, or else the host's name and the
    /// machine's id as the system has them.
    #[test]
    fn the_holder_is_the_option_or_the_hosts_name_and_machine_id() {
        let keep = tempfile::tempdir().expect("a directory");
        let given = serving(keep.path(), "--holder node-a");
        assert_eq!(holder_of(&given), Ok("node-a".to_owned()));
        let host = std::fs::read_to_string(HOSTNAME).expect("the host's name");
        match std::fs::read_to_string(MACHINE_ID) {
            Ok(id) => assert_eq!(
                holder_of(&serving(keep.path(), "")),
                Ok(format!(
                    "{}:{}",
                    host.trim_end_matches('\n'),
                    id.trim_end_matches('\n')
                ))
            ),
            Err(_) => assert!(holder_of(&serving(keep.path(), "")).is_err()),
        }
    }

    /// The default holder from given files: the host's name, a colon and
    /// the machine's id; refused, naming --holder, where the id is
    /// missing or is not one.
    #[test]
    fn the_default_holder_needs_a_machine_id() {
        let keep = tempfile::tempdir().expect("a directory");
        let (host, id) = (keep.path().join("hostname"), keep.path().join("machine-id"));
        std::fs::write(&host, "db-7\n").expect("written");
        let id_hex = "0123456789abcdef0123456789ABCDEF";
        std::fs::write(&id, format!("{id_hex}\n")).expect("written");
        assert_eq!(default_holder(&host, &id), Ok(format!("db-7:{id_hex}")));

        let refused = |content: Option<&str>| {
            match content {
                Some(c) => std::fs::write(&id, c).expect("written"),
                None => std::fs::remove_file(&id).expect("removed"),
            }
            let why = default_holder(&host, &id).expect_err("refused");
            assert!(why.contains("give --holder"), "{why}");
            assert!(why.contains("machine-id"), "{why}");
            why
        };
        refused(Some(""));
        refused(Some("uninitialized\n"));
        refused(Some(&"0".repeat(32)));
        refused(Some(&"0123456789abcdef".repeat(2)[..31]));
        refused(Some(&format!("{}g\n", &id_hex[..31])));
        refused(Some(&format!("{id_hex}0\n")));
        assert!(refused(None).contains("No such file"));

        // A host's name the holder's rule refuses is refused with it.
        std::fs::write(&id, format!("{id_hex}\n")).expect("written");
        std::fs::write(&host, "db 7\n").expect("written");
        let why = default_holder(&host, &id).expect_err("refused");
        assert!(why.contains("give --holder"), "{why}");
    }

    /// The held cuts' figures reach the status each in its own field,
    /// the cuts replaced and the image's holes among them, with what the
    /// start did.
    #[test]
    fn the_held_cuts_reach_the_status_each_in_its_field() {
        let figures = CutFigures {
            held: 2,
            copied: 3,
            bound: 4,
            abandoned: 5,
            replaced: 6,
            holes: CutHoles::Held,
            hole_chunks: 7,
        };
        assert_eq!(
            held_shown_of(Some(&CutsTakenUp::TakenUp), figures),
            HeldShown {
                state: CutStateShown::TakenUp,
                held: 2,
                copied: 3,
                bound: 4,
                abandoned: 5,
                replaced: Some(6),
                holes: Some(HolesShown {
                    state: CutHolesShown::Held,
                    chunks: 7,
                }),
            }
        );
        assert_eq!(
            held_shown_of(None, CutFigures::default()).state,
            CutStateShown::Off
        );
        for (holes, shown) in [
            (CutHoles::None, CutHolesShown::None),
            (CutHoles::Held, CutHolesShown::Held),
            (CutHoles::Dropped, CutHolesShown::Dropped),
        ] {
            let figures = CutFigures {
                holes,
                ..CutFigures::default()
            };
            let said = held_shown_of(None, figures).holes.map(|h| h.state);
            assert_eq!(said, Some(shown), "{holes:?}");
        }
    }

    /// What the end of the uploader's thread says, for each end.
    #[test]
    fn the_uploaders_end_is_said() {
        assert_eq!(upload_ended_said(&Ended::Released), "lease: let go");
        assert_eq!(
            upload_ended_said(&Ended::Kept),
            "lease: kept until its time runs out"
        );
        assert!(upload_ended_said(&Ended::NotReleased("refused".into())).contains("(refused)"));
        assert!(upload_ended_said(&Ended::NoAnswer).contains("6000 ms"));
    }

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

    /// `--intent-region-mib N` reaches the store as N MiB, only with
    /// `--hold-cuts`; without it the store's default holds (G4 of the
    /// review of #106).
    #[test]
    fn the_intent_region_reaches_the_store() {
        let parsed = |line: &str| match crate::cli::parse(line.split_whitespace().map(String::from))
        {
            Ok(crate::cli::Command::Serve(serve)) => serve,
            other => panic!("{line}: {other:?}"),
        };
        let path = std::path::PathBuf::from("/run/v/cuts");
        let boot = [7; 16];
        let given = parsed("serve --dir /v/a --hold-cuts --intent-region-mib 64");
        let hold = hold_cuts_of(&given, false, path.clone(), boot).expect("cuts are held");
        assert_eq!(hold.intent_region, Some(64 << 20));
        assert_eq!(hold.boot, boot);
        assert!(matches!(&hold.region, CutRegion::Named(at) if *at == path));
        let default = parsed("serve --dir /v/a --hold-cuts");
        let hold = hold_cuts_of(&default, false, path.clone(), boot).expect("cuts are held");
        assert_eq!(hold.intent_region, None, "the store's default");
        assert!(hold_cuts_of(&parsed("serve --dir /v/a"), false, path.clone(), boot).is_none());
        // A volume that uploads holds its cuts, given --hold-cuts or not
        // (M2.4c).
        let hold = hold_cuts_of(&parsed("serve --dir /v/a"), true, path.clone(), boot);
        assert_eq!(hold.map(|hold| hold.intent_region), Some(None));
        // `--intent-header-sync` reaches the store, and is off unless
        // given (M2.4d).
        let synced = parsed("serve --dir /v/a --hold-cuts --intent-header-sync");
        let hold = hold_cuts_of(&synced, false, path.clone(), boot).expect("cuts are held");
        assert!(hold.intent_header_sync);
        let hold = hold_cuts_of(&default, false, path, boot).expect("cuts are held");
        assert!(!hold.intent_header_sync);
    }

    /// The status carries the store's figures of the header's sync as the
    /// store gives them, apart from the write path's (G2 of #120's
    /// review): whether it is on, and the header's syncs, not the write
    /// path's.
    #[test]
    fn the_status_carries_the_stores_header_syncs_apart_from_the_write_paths() {
        use skyzfs_local::store::Spent as Shown;
        let spent = |count, sum_ns, max_ns| Shown {
            count,
            sum_ns,
            max_ns,
        };
        for on in [false, true] {
            let figures = IntentFigures {
                regions: 9,
                named: 4,
                region_bytes: 1 << 20,
                syncs: spent(5, 500, 200),
                waits: 3,
                header_sync: on,
                header_syncs: if on { spent(2, 70, 40) } else { spent(0, 0, 0) },
            };
            let shown = intent_shown_of(figures, None);
            assert_eq!(
                shown.syncs,
                Shown {
                    count: 5,
                    sum_ns: 500,
                    max_ns: 200
                },
                "on {on}"
            );
            let header = shown.header.expect("the header's sync is shown");
            assert_eq!(header.on, on);
            let expected = if on {
                Shown {
                    count: 2,
                    sum_ns: 70,
                    max_ns: 40,
                }
            } else {
                Shown::default()
            };
            assert_eq!(header.syncs, expected, "on {on}");
        }
    }

    /// The status says how many regions the map named at the start: a
    /// map taken up, what it named; a new map, every region (G4 of the
    /// review of #106).
    #[test]
    fn a_new_map_names_every_region_at_the_start() {
        assert_eq!(
            intent_at_start_shown(Some(&IntentAtStart::Begun("no map".into())), 9),
            (CutStateShown::Begun, 9)
        );
        assert_eq!(
            intent_at_start_shown(
                Some(&IntentAtStart::TakenUp {
                    named: 2,
                    regions: 9
                }),
                9
            ),
            (CutStateShown::TakenUp, 2)
        );
        assert_eq!(intent_at_start_shown(None, 9), (CutStateShown::Off, 0));
    }

    /// A failure is noted with the image's reason (code 7), while the
    /// store serves and when its own sync fails at its start; the log
    /// store's reasons are no longer given (M1.3t).
    #[test]
    fn a_failure_is_noted_as_the_images() {
        assert_eq!(FAILURE, Reason::ImageFailed);
        assert_eq!(
            Reason::ImageFailed.to_string(),
            "a write or sync of the volume's image failed"
        );
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
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<skyzfs_ublk::pump::Wake, skyzfs_ublk::pump::QueueDown> {
            assert!(!self.panics, "the queue panics, as the test asks");
            Ok(skyzfs_ublk::pump::Wake::Stop)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<skyzfs_blk::Buf> {
            Some(skyzfs_blk::Buf::zeroed(blocks))
        }
        fn commit(&mut self, _tag: u16, _result: i32, _buf: Option<skyzfs_blk::Buf>) {}
        fn flush(
            &mut self,
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    /// The volume's lock is still held when the queue's thread has
    /// ended, by a stop and by a panic alike: the store comes back to
    /// the thread that joins, which is where the failure's cleanup
    /// runs, and the lock is free only when that thread drops it.
    #[test]
    fn the_volumes_lock_outlives_the_queues_thread_until_the_joiner_lets_go() {
        use skyzfs_local::inplace::{create, lock, OpenError, OpenOptions, Opened, VolumeConfig};
        use skyzfs_local::sim::SimDisk;
        for panics in [false, true] {
            let disk = SimDisk::new(1);
            let config = VolumeConfig {
                volume: *b"a volume on disk",
                size_mib: 8,
                base: skyzfs_blk::Base::Empty,
                write_unit: 4096,
            };
            create(&disk, &config).expect("create");
            let served = Opened::open(
                disk.clone(),
                lock(&disk).expect("lock"),
                OpenOptions::default(),
            )
            .map_err(|(e, _)| e)
            .expect("open")
            .start(None)
            .expect("start");
            let turns = Arc::new(std::sync::atomic::AtomicU32::new(0));
            let thread = std::thread::spawn({
                let turns = turns.clone();
                move || {
                    pumped(Pump::new(Ending { panics }, served), |_, ended| {
                        if ended {
                            turns.fetch_add(1, Ordering::SeqCst);
                        }
                    })
                }
            });
            let (stopped, store) = thread.join().expect("the panic is caught on the thread");
            assert_eq!(stopped.is_none(), panics, "{stopped:?}");
            assert_eq!(turns.load(Ordering::SeqCst), u32::from(!panics));
            // The queue's thread is gone. This is the moment of the
            // failure's cleanup: nobody else gets the lock.
            assert!(
                matches!(lock(&disk), Err(OpenError::Locked)),
                "panics: {panics}: the lock was free after the queue's thread ended"
            );
            drop(store);
            assert!(lock(&disk).is_ok(), "panics: {panics}");
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
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
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
        fn flush(
            &mut self,
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    /// A queue that hands over tag 0 at its first wait and tag 0 again
    /// at its second, while the first request is still out: a driver
    /// that breaks its rule.
    #[derive(Default)]
    struct Rehanded {
        waits: u32,
        committed: Arc<std::sync::Mutex<Vec<(u16, i32)>>>,
    }

    impl skyzfs_ublk::pump::Queue for Rehanded {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(
            &mut self,
            fetched: &mut Vec<skyzfs_ublk::pump::Fetched>,
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<skyzfs_ublk::pump::Wake, skyzfs_ublk::pump::QueueDown> {
            use skyzfs_ublk::translate::{op, Desc};
            self.waits += 1;
            if self.waits <= 2 {
                fetched.push(skyzfs_ublk::pump::Fetched {
                    tag: 0,
                    desc: Desc {
                        op_flags: u32::from(op::WRITE),
                        nr_sectors: 8,
                        start_sector: 0,
                    },
                });
            }
            Ok(skyzfs_ublk::pump::Wake::Work)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<skyzfs_blk::Buf> {
            Some(skyzfs_blk::Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, result: i32, _buf: Option<skyzfs_blk::Buf>) {
            self.committed.lock().expect("lock").push((tag, result));
        }
        fn flush(
            &mut self,
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    /// A driver that breaks its rule ends serve with status 7, on the
    /// path that leaves the device for the next server (M1.3ac, the
    /// lead's two conditions). The queue's thread, run as serve runs it
    /// (`pumped`, with the check for a failure at every turn), ends
    /// with the rule broken and the tag named, has committed nothing,
    /// and has not marked the volume failed: serve's main thread takes
    /// the failure's path (its note and the device's delete) only on
    /// that mark, and stops the device only on a signal, so it does
    /// neither here. The end then gives status 7 and a line that names
    /// the cause and says that the device waits for the next server. A
    /// queue that went away stays an error (1), and an orderly stop 0.
    #[test]
    fn a_driver_that_breaks_its_rule_ends_serve_with_7_and_leaves_the_device() {
        let committed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue = Rehanded {
            committed: committed.clone(),
            ..Rehanded::default()
        };
        let queue = Delayed::new(queue, None, None, || {});
        let flags = Flags::default();
        let (stopped, store) = pumped(Pump::new(queue, Silent(Vec::new())), |pump, _| {
            answer_if_failed(pump, &flags, &|| {});
        });
        assert_eq!(stopped, Some(Stopped::BrokenRule { tag: 0 }));
        assert!(committed.lock().expect("lock").is_empty());
        assert_eq!(store.0.len(), 1, "only the first request reached the store");
        assert!(
            !flags.failed.load(Ordering::SeqCst),
            "the volume is not failed"
        );
        assert!(!flags.answered.load(Ordering::SeqCst));

        assert_eq!(
            ended(3, false, Stopped::BrokenRule { tag: 0 }),
            Ok(Exit::BrokenRule)
        );
        let said = broken_rule_said(3, 0);
        for words in [
            "the kernel handed over tag 0 of device 3",
            "which the queue does not have or whose request was still out",
            "nothing more is answered",
            "waits for the next server",
        ] {
            assert!(said.contains(words), "{said}");
        }
        assert!(ended(3, false, Stopped::QueueDown).is_err());
        assert_eq!(ended(3, true, Stopped::Orderly), Ok(Exit::Done));
    }

    /// A queue that hands over a write on tag 0 and a read on tag 1 at
    /// its first wait; at its second, the driver refuses the commit of
    /// tag 0 and hands over a write on tag 2. It notes its waits and
    /// what is committed.
    #[derive(Default)]
    struct Refusing {
        waits: u32,
        committed: Arc<std::sync::Mutex<Vec<(u16, i32)>>>,
    }

    impl skyzfs_ublk::pump::Queue for Refusing {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(
            &mut self,
            fetched: &mut Vec<skyzfs_ublk::pump::Fetched>,
            unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<skyzfs_ublk::pump::Wake, skyzfs_ublk::pump::QueueDown> {
            use skyzfs_ublk::pump::{Command, Fetched, Unfetched};
            use skyzfs_ublk::translate::{op, Desc};
            let request = |tag: u16, op: u8| Fetched {
                tag,
                desc: Desc {
                    op_flags: u32::from(op),
                    nr_sectors: 8,
                    start_sector: u64::from(tag) * 8,
                },
            };
            self.waits += 1;
            match self.waits {
                1 => fetched.extend([request(0, op::WRITE), request(1, op::READ)]),
                2 => {
                    unfetched.push(Unfetched {
                        tag: 0,
                        command: Command::Commit,
                        result: -22,
                    });
                    fetched.push(request(2, op::WRITE));
                }
                _ => {}
            }
            Ok(skyzfs_ublk::pump::Wake::Work)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<skyzfs_blk::Buf> {
            Some(skyzfs_blk::Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, result: i32, _buf: Option<skyzfs_blk::Buf>) {
            self.committed.lock().expect("lock").push((tag, result));
        }
        fn flush(
            &mut self,
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    /// A target that completes every request it is handed at its next
    /// poll, and keeps what it was handed.
    #[derive(Default)]
    struct Answering {
        handed: Vec<u64>,
        done: Vec<skyzfs_blk::Completion>,
    }

    impl skyzfs_blk::BlockTarget for Answering {
        fn geometry(&self) -> skyzfs_blk::Geometry {
            skyzfs_blk::Geometry::new(8, 4, false).expect("a geometry")
        }
        fn submit(&mut self, request: skyzfs_blk::Request) -> Result<(), skyzfs_blk::SubmitError> {
            self.handed.push(request.id.0);
            let buf = match request.op {
                skyzfs_blk::Op::Read { buf, .. } => Some(buf),
                skyzfs_blk::Op::Write { data, .. } => Some(data),
                _ => None,
            };
            self.done.push(skyzfs_blk::Completion {
                id: request.id,
                result: Ok(()),
                buf,
            });
            Ok(())
        }
        fn poll(&mut self, out: &mut Vec<skyzfs_blk::Completion>) -> usize {
            let n = self.done.len();
            out.append(&mut self.done);
            n
        }
        fn wait(&mut self, out: &mut Vec<skyzfs_blk::Completion>, _: Duration) -> usize {
            self.poll(out)
        }
    }

    impl skyzfs_ublk::pump::PumpTarget for Answering {
        fn gone(&self) -> bool {
            false
        }
    }

    /// A commit the driver refuses ends serve with status 8, on the
    /// path that leaves the device for the next server (M1.3ac part 3).
    /// The queue's thread, run as serve runs it (`pumped`, with the
    /// check for a failure at every turn), ends with the refusal, its
    /// tag and its error number, and has not marked the volume failed:
    /// serve's main thread takes the failure's path (its note and the
    /// device's delete) only on that mark, and stops the device only on
    /// a signal, so it does neither here. Nothing of the wait that
    /// brought the refusal reaches the store, nothing is committed after
    /// it, and the refused tag is not committed again. The end then
    /// gives status 8 and a line that names the cause and says that the
    /// device waits for the next server; status 7 stays the broken rule.
    #[test]
    fn a_refused_commit_ends_serve_with_8_and_leaves_the_device() {
        let committed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue = Refusing {
            committed: committed.clone(),
            ..Refusing::default()
        };
        let queue = Delayed::new(queue, None, None, || {});
        let flags = Flags::default();
        let (stopped, store) = pumped(Pump::new(queue, Answering::default()), |pump, _| {
            answer_if_failed(pump, &flags, &|| {});
        });
        assert_eq!(stopped, Some(Stopped::CommitRefused { tag: 0, errno: 22 }));
        assert_eq!(
            *committed.lock().expect("lock"),
            [(0, 4096), (1, 4096)],
            "the two answers before the refusal, and none after it"
        );
        assert_eq!(
            store.handed,
            [0, 1],
            "the write on tag 2 is the next server's"
        );
        assert!(
            !flags.failed.load(Ordering::SeqCst),
            "the volume is not failed"
        );
        assert!(!flags.answered.load(Ordering::SeqCst));

        let refused = Stopped::CommitRefused { tag: 0, errno: 22 };
        assert_eq!(ended(3, false, refused), Ok(Exit::CommitRefused));
        assert_eq!(ended(3, true, refused), Ok(Exit::CommitRefused));
        let said = commit_refused_said(3, 0, 22);
        for words in [
            "the kernel refused serve's answer on tag 0 of device 3",
            "Invalid argument",
            "nothing more is answered",
            "the volume is not failed",
            "waits for the next server",
        ] {
            assert!(said.contains(words), "{said}");
        }
        assert_eq!(
            ended(3, false, Stopped::BrokenRule { tag: 0 }),
            Ok(Exit::BrokenRule)
        );
    }

    /// A queue whose first wait brings the driver's refusal of tag 0's
    /// first fetch, with a write handed over on tag 1, and nothing after.
    #[derive(Default)]
    struct FetchRefusing {
        waits: u32,
        committed: Arc<std::sync::Mutex<Vec<(u16, i32)>>>,
    }

    impl skyzfs_ublk::pump::Queue for FetchRefusing {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(
            &mut self,
            fetched: &mut Vec<skyzfs_ublk::pump::Fetched>,
            unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<skyzfs_ublk::pump::Wake, skyzfs_ublk::pump::QueueDown> {
            use skyzfs_ublk::pump::{Command, Fetched, Unfetched};
            use skyzfs_ublk::translate::{op, Desc};
            self.waits += 1;
            if self.waits == 1 {
                unfetched.push(Unfetched {
                    tag: 0,
                    command: Command::Fetch,
                    result: -22,
                });
                fetched.push(Fetched {
                    tag: 1,
                    desc: Desc {
                        op_flags: u32::from(op::WRITE),
                        nr_sectors: 8,
                        start_sector: 8,
                    },
                });
            }
            Ok(skyzfs_ublk::pump::Wake::Work)
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<skyzfs_blk::Buf> {
            Some(skyzfs_blk::Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, result: i32, _buf: Option<skyzfs_blk::Buf>) {
            self.committed.lock().expect("lock").push((tag, result));
        }
        fn flush(
            &mut self,
            _unfetched: &mut Vec<skyzfs_ublk::pump::Unfetched>,
        ) -> Result<(), skyzfs_ublk::pump::QueueDown> {
            Ok(())
        }
    }

    /// A fetch the driver refuses ends the queue's thread, run as serve
    /// runs it, with the refusal, before anything reaches the store or
    /// is committed, and with the volume not failed (M1.3ah). Serve then
    /// gives the device's start up with status 9 and a line that names
    /// the queue, the tag and the error, says what becomes of the device
    /// (deleted at a start, left waiting at a take-over) and that the
    /// volume is not failed, and tells a supervisor what to do. A start
    /// that had no answer within the limit is given up with 9 too, with
    /// a line of its own.
    #[test]
    fn a_refused_fetch_ends_the_queue_and_serve_gives_the_start_up_with_9() {
        let committed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue = FetchRefusing {
            committed: committed.clone(),
            ..FetchRefusing::default()
        };
        let queue = Delayed::new(queue, None, None, || {});
        let flags = Flags::default();
        let (stopped, store) = pumped(Pump::new(queue, Answering::default()), |pump, _| {
            answer_if_failed(pump, &flags, &|| {});
        });
        let refused = Stopped::FetchRefused { tag: 0, errno: 22 };
        assert_eq!(stopped, Some(refused));
        assert!(
            committed.lock().expect("lock").is_empty(),
            "nothing committed"
        );
        assert!(store.handed.is_empty(), "the write on tag 1 is not taken");
        assert!(
            !flags.failed.load(Ordering::SeqCst),
            "the volume is not failed"
        );
        assert_eq!(ended(3, false, refused), Ok(Exit::NotStarted));
        assert_eq!(ended(3, true, refused), Ok(Exit::NotStarted));

        let fetch = GaveUp::FetchRefused { tag: 0, errno: 22 };
        let common = [
            "the volume is not failed",
            "exit 9: a supervisor should retry the serve once; a second 9 is for a person",
        ];
        let at_start = gave_up_said(3, false, &fetch, Some(&Ok(())));
        for words in [
            "the kernel refused the first fetch of tag 0 on queue 0 of device 3",
            "Invalid argument",
            "the device's start waits for every tag's fetch",
            "device 3, which this serve added and never started, is deleted",
        ]
        .iter()
        .chain(&common)
        {
            assert!(at_start.contains(words), "{at_start}");
        }
        let at_take_over = gave_up_said(3, true, &fetch, None);
        for words in [
            "the end of its recovery waits for every tag's fetch",
            "device 3 is left waiting for the next server, as after a kill",
        ]
        .iter()
        .chain(&common)
        {
            assert!(at_take_over.contains(words), "{at_take_over}");
        }
        assert!(!at_take_over.contains("deleted"), "{at_take_over}");
        // Whole lines, so that what joins the parts is held too.
        assert_eq!(
            at_start,
            "the kernel refused the first fetch of tag 0 on queue 0 of device 3 (Invalid \
             argument (os error 22)), and the device's start waits for every tag's fetch with \
             no limit: serve gives it up; device 3, which this serve added and never started, \
             is deleted; the volume is not failed; exit 9: a supervisor should retry the serve \
             once; a second 9 is for a person"
        );
        assert_eq!(
            at_take_over,
            "the kernel refused the first fetch of tag 0 on queue 0 of device 3 (Invalid \
             argument (os error 22)), and the end of its recovery waits for every tag's fetch \
             with no limit: serve gives it up; device 3 is left waiting for the next server, as \
             after a kill, with the I/O it holds; the volume is not failed; exit 9: a \
             supervisor should retry the serve once; a second 9 is for a person"
        );
        assert_eq!(
            fetch_refused_said(3, 5, 22),
            "the kernel refused the fetch of tag 5 on queue 0 of device 3 (Invalid argument (os \
             error 22)): nothing more is answered on this queue, the volume is not failed, and \
             device 3 is left as it is; exit 9: a supervisor should retry the serve once; a \
             second 9 is for a person"
        );
        let undeleted = Err(Undeleted::NotePutBack(io::Error::from_raw_os_error(16)));
        let kept = gave_up_said(3, false, &fetch, Some(&undeleted));
        assert_eq!(
            kept,
            format!(
                "the kernel refused the first fetch of tag 0 on queue 0 of device 3 (Invalid \
                 argument (os error 22)), and the device's start waits for every tag's fetch \
                 with no limit: serve gives it up; device 3, which this serve added and never \
                 started, is to be deleted: {}; the volume is not failed; exit 9: a supervisor \
                 should retry the serve once; a second 9 is for a person",
                Undeleted::NotePutBack(io::Error::from_raw_os_error(16)).said(3)
            )
        );
        let limit = START_LIMIT.as_secs();
        assert_eq!(
            gave_up_said(3, false, &GaveUp::NoAnswer, Some(&Ok(()))),
            format!(
                "the start of device 3 had no answer within {limit} s: serve gives it up; \
                 device 3, which this serve added and never started, is deleted; the volume is \
                 not failed; exit 9: a supervisor should retry the serve once; a second 9 is for \
                 a person"
            )
        );
        assert_eq!(
            gave_up_said(3, true, &GaveUp::NoAnswer, None),
            format!(
                "the end of the recovery of device 3 had no answer within {limit} s: serve gives \
                 it up; device 3 is left waiting for the next server, as after a kill, with the \
                 I/O it holds; the volume is not failed; exit 9: a supervisor should retry the \
                 serve once; a second 9 is for a person"
            )
        );
    }

    /// The wait for the device's start ends with its answer; with the
    /// volume's failure, which comes before the queue's end (a failure
    /// is a failure, exit 3); with the queue's end (a refused fetch);
    /// or with no answer once the limit has run out. An answer that
    /// came is taken whatever else holds: the failure is then the
    /// main loop's to see. The limit stays well under a supervisor's
    /// usual stop timeout (systemd's 90 s), since a SIGTERM during the
    /// start is acted on when the wait ends, and the help gives it.
    #[test]
    fn the_start_is_waited_for_within_its_limit() {
        let limit = Duration::from_millis(200);
        let (no, yes) = (|| false, || true);
        let answered = || {
            let (tx, rx) = std::sync::mpsc::channel();
            tx.send(Ok(())).expect("send");
            rx
        };
        assert!(matches!(
            wait_for_start(&answered(), no, no, limit),
            StartWait::Answered(Ok(()))
        ));
        assert!(matches!(
            wait_for_start(&answered(), yes, yes, limit),
            StartWait::Answered(Ok(()))
        ));
        let (_tx, rx) = std::sync::mpsc::channel::<io::Result<()>>();
        let began = Instant::now();
        assert!(matches!(
            wait_for_start(&rx, yes, no, limit),
            StartWait::QueueEnded
        ));
        assert!(began.elapsed() < limit, "the queue's end is seen at once");
        assert!(matches!(
            wait_for_start(&rx, yes, yes, limit),
            StartWait::Failed
        ));
        assert!(matches!(
            wait_for_start(&rx, no, yes, limit),
            StartWait::Failed
        ));
        // The wait with no answer runs aside, so that a limit not kept
        // fails the test instead of hanging it.
        let (waited, wait) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let began = Instant::now();
            let ended = wait_for_start(&rx, || false, || false, limit);
            let _ = waited.send((matches!(ended, StartWait::NoAnswer), began.elapsed()));
            drop(_tx);
        });
        let (no_answer, took) = wait
            .recv_timeout(limit * 20)
            .expect("the wait with no answer ends within its limit");
        assert!(no_answer, "the wait with no answer says so");
        assert!(took >= limit, "{took:?}");
        let (tx, rx) = std::sync::mpsc::channel::<io::Result<()>>();
        drop(tx);
        assert!(matches!(
            wait_for_start(&rx, no, no, limit),
            StartWait::Answered(Err(_))
        ));
        assert!(
            START_LIMIT * 3 <= Duration::from_secs(90),
            "{START_LIMIT:?}"
        );
        assert!(
            crate::cli::HELP.contains(&format!("within {} s", START_LIMIT.as_secs())),
            "the help gives the limit"
        );
    }

    /// A kernel that keeps what it was asked to delete.
    #[derive(Default)]
    struct Deletes(Vec<u32>);

    impl skyzfs_ublk::lifecycle::Control for Deletes {
        fn features(&mut self) -> io::Result<Option<u64>> {
            Ok(Some(u64::MAX))
        }
        fn release(&mut self) -> io::Result<String> {
            Ok("7.0.0".into())
        }
        fn info(&mut self, _number: u32) -> io::Result<Option<skyzfs_ublk::lifecycle::DeviceInfo>> {
            Ok(None)
        }
        fn add(
            &mut self,
            _number: u32,
            _shape: &skyzfs_ublk::lifecycle::Shape,
        ) -> Result<(), skyzfs_ublk::lifecycle::AddError> {
            Err(skyzfs_ublk::lifecycle::AddError::Other(io::Error::other(
                "not in this test",
            )))
        }
        fn disown(&mut self, _number: u32) {}
        fn delete(&mut self, number: u32) -> io::Result<()> {
            self.0.push(number);
            Ok(())
        }
        fn begin_recovery(&mut self, _number: u32) -> io::Result<()> {
            Err(io::Error::other("not in this test"))
        }
        fn client_holds(&mut self, _number: u32) -> Option<bool> {
            None
        }
    }

    /// A start given up deletes the device when this serve added it
    /// (it was never started and holds no pool), its note first, as
    /// every delete of the lifecycle; a device taken over holds a
    /// pool's waiting I/O, which a delete would fail, and is left with
    /// its note, as after a kill.
    #[test]
    fn a_start_given_up_deletes_a_device_this_serve_added_and_leaves_one_it_took_over() {
        use skyzfs_ublk::lifecycle::Records;
        let keep = tempfile::tempdir().expect("a directory");
        let (volume, run) = (keep.path().join("volume"), keep.path().join("run"));
        std::fs::create_dir(&volume).expect("the volume's directory");
        std::fs::create_dir(&run).expect("the run directory");
        let mut records = Files::new(&volume, &run, [1; 16], Duration::ZERO).expect("records");
        records.create_run_dirs().expect("the run directories");
        records.write_note(3, &[9; 16]).expect("a note");
        let mut kernel = Deletes::default();
        assert!(give_up_start(&mut kernel, &mut records, 3, true).is_none());
        assert!(kernel.0.is_empty(), "nothing deleted at a take-over");
        assert_eq!(records.note(3).ok().flatten(), Some([9; 16]));
        assert!(matches!(
            give_up_start(&mut kernel, &mut records, 3, false),
            Some(Ok(()))
        ));
        assert_eq!(kernel.0, [3], "deleted at a start");
        assert_eq!(records.note(3).ok().flatten(), None, "its note first");
    }

    /// What happened, in order, across a fake kernel, the queue's
    /// thread and a store.
    type Log = Arc<std::sync::Mutex<Vec<String>>>;

    fn logged(log: &Log, what: impl Into<String>) {
        log.lock().expect("the log").push(what.into());
    }

    fn read(log: &Log) -> Vec<String> {
        log.lock().expect("the log").clone()
    }

    /// A kernel that logs each command a start given up sends, and
    /// whether the device's note under `run` was still there when the
    /// delete came. As the lifecycle's control it logs "unbounded ..."
    /// for each of them: the give-up sends none that way. As the bounded
    /// commands, its delete never answers unless `answers`.
    #[derive(Clone)]
    struct Logged {
        log: Log,
        run: std::path::PathBuf,
        answers: bool,
    }

    impl skyzfs_ublk::lifecycle::Control for Logged {
        fn features(&mut self) -> io::Result<Option<u64>> {
            Ok(Some(u64::MAX))
        }
        fn release(&mut self) -> io::Result<String> {
            Ok("7.0.0".into())
        }
        fn info(&mut self, number: u32) -> io::Result<Option<skyzfs_ublk::lifecycle::DeviceInfo>> {
            logged(&self.log, format!("unbounded info {number}"));
            Ok(None)
        }
        fn add(
            &mut self,
            _number: u32,
            _shape: &skyzfs_ublk::lifecycle::Shape,
        ) -> Result<(), skyzfs_ublk::lifecycle::AddError> {
            Err(skyzfs_ublk::lifecycle::AddError::Other(io::Error::other(
                "not in this test",
            )))
        }
        fn disown(&mut self, _number: u32) {}
        fn delete(&mut self, number: u32) -> io::Result<()> {
            logged(&self.log, format!("unbounded delete {number}"));
            Ok(())
        }
        fn begin_recovery(&mut self, _number: u32) -> io::Result<()> {
            Err(io::Error::other("not in this test"))
        }
        fn client_holds(&mut self, number: u32) -> Option<bool> {
            logged(&self.log, format!("unbounded holds {number}"));
            None
        }
    }

    impl Commands for Logged {
        fn delete(&self, number: u32) -> io::Result<()> {
            let noted = records::note_path(&self.run, number).exists();
            logged(
                &self.log,
                format!(
                    "delete {number}{}",
                    if noted { " (note there)" } else { "" }
                ),
            );
            if !self.answers {
                // A delete that never answers: the thread waits for
                // good, as the driver's would behind the mutex.
                loop {
                    std::thread::park();
                }
            }
            Ok(())
        }
        fn info(&self, number: u32) -> io::Result<Option<skyzfs_ublk::lifecycle::DeviceInfo>> {
            logged(&self.log, format!("info {number}"));
            Ok(None)
        }
        fn client_holds(&self, number: u32) -> Option<bool> {
            logged(&self.log, format!("holds {number}"));
            None
        }
    }

    /// What the queue's thread hands back: its drop is logged.
    struct Kept(Log);

    impl Drop for Kept {
        fn drop(&mut self) {
            logged(&self.0, "store dropped");
        }
    }

    /// How a start given up in a test ended: its exit, the log before
    /// and after the caller's drop of what it was handed, whether the
    /// device's note is still there, whether a failure note is, and how
    /// long it took.
    type GivenUp = (Exit, Vec<String>, Vec<String>, bool, bool, Duration);

    /// A start with no answer given up with [`Logged`] for a device 3
    /// with its note. `fails_at` is the look at the failure flag that
    /// first sees it set, if any: the first is before the delete, the
    /// second after the stop. Run on a thread of its own, so that a
    /// limit not kept fails the test instead of hanging it.
    fn given_up(took_over: bool, fails_at: Option<u32>, answers: bool, limit: Duration) -> GivenUp {
        let (done, outcome) = mpsc::channel();
        std::thread::spawn(move || {
            use skyzfs_ublk::lifecycle::Records;
            let keep = tempfile::tempdir().expect("a directory");
            let (volume, run) = (keep.path().join("volume"), keep.path().join("run"));
            std::fs::create_dir(&volume).expect("the volume's directory");
            std::fs::create_dir(&run).expect("the run directory");
            let mut records = Files::new(&volume, &run, [1; 16], Duration::ZERO).expect("records");
            records.create_run_dirs().expect("the run directories");
            records.write_note(3, &[9; 16]).expect("a note");
            let log = Log::default();
            let mut kernel = Logged {
                log: log.clone(),
                run: run.clone(),
                answers,
            };
            let commands = kernel.clone();
            let looks = std::cell::Cell::new(0);
            let failed = || {
                looks.set(looks.get() + 1);
                fails_at.is_some_and(|at| looks.get() >= at)
            };
            let began = Instant::now();
            let (exit, kept) = give_up_unanswered(
                &mut kernel,
                commands,
                limit,
                &mut records,
                3,
                took_over,
                [1; 16],
                failed,
                || {
                    logged(&log, "stopped");
                    Some(Kept(log.clone()))
                },
            );
            let took = began.elapsed();
            let before_the_drop = read(&log);
            drop(kept);
            let after = read(&log);
            let noted = records.note(3).ok().flatten().is_some();
            let failure = records.failure().ok().flatten().is_some();
            let _ = done.send((exit, before_the_drop, after, noted, failure, took));
        });
        outcome
            .recv_timeout(limit * 20 + Duration::from_secs(5))
            .expect("a start given up ends within its limits")
    }

    /// A start with no answer within the limit is given up in this
    /// order (M1.3ah, after #87's review):
    /// - at a start, the device is deleted, its note first, while the
    ///   queue's thread still answers, and only then is that thread
    ///   stopped;
    /// - at a take-over, the thread is stopped and nothing deleted;
    /// - a failure comes first: one seen before the delete, or after
    ///   the stop, takes the failure's way (its note, the delete,
    ///   exit 3) and not 9;
    /// - what the queue's thread handed back, the store, is handed to
    ///   the caller and not dropped: its drop waits for its I/O with no
    ///   limit, and comes after the line;
    /// - every command goes to the kernel through the bounded commands,
    ///   none through the lifecycle's control itself.
    #[test]
    fn a_start_with_no_answer_is_given_up_in_order() {
        let strings = |all: &[&str]| all.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let limit = Duration::from_secs(10);

        let (exit, log, after, noted, failure, _) = given_up(false, None, true, limit);
        assert_eq!(exit, Exit::NotStarted, "at a start");
        assert_eq!(log, strings(&["delete 3", "stopped"]), "at a start");
        assert_eq!(after.last().map(String::as_str), Some("store dropped"));
        assert!(!noted && !failure, "at a start");

        let (exit, log, _, noted, failure, _) = given_up(true, None, true, limit);
        assert_eq!(exit, Exit::NotStarted, "at a take-over");
        assert_eq!(log, strings(&["stopped"]), "at a take-over");
        assert!(noted && !failure, "at a take-over");

        for took_over in [false, true] {
            let (exit, log, _, noted, failure, _) = given_up(took_over, Some(1), true, limit);
            assert_eq!(exit, Exit::Failed, "failed before the delete");
            assert_eq!(
                log,
                strings(&["holds 3", "delete 3", "stopped"]),
                "failed before the delete, at a take-over: {took_over}"
            );
            assert!(!noted && failure, "failed before the delete");

            let (exit, log, after, noted, failure, _) = given_up(took_over, Some(2), true, limit);
            assert_eq!(exit, Exit::Failed, "failed after the stop");
            let expected: &[&str] = if took_over {
                &["stopped", "holds 3", "delete 3"]
            } else {
                &["delete 3", "stopped", "holds 3", "delete 3"]
            };
            assert_eq!(
                log,
                strings(expected),
                "failed after the stop, at a take-over: {took_over}"
            );
            assert_eq!(after.last().map(String::as_str), Some("store dropped"));
            assert!(!noted && failure, "failed after the stop");
        }
    }

    /// A delete that has no answer does not hold a start given up past
    /// its limit (M1.3ah, the lead's word on #87's review): on a kernel
    /// whose START_DEV reads the partition table under the device's
    /// mutex from a disk that stopped answering, it would wait for
    /// good. Serve ends with 9 and a line that names the delete, or,
    /// with a failure, with 3; after the limit nothing more is sent;
    /// the device's note is not put back, since nothing more can be
    /// asked of the device.
    #[test]
    fn a_delete_with_no_answer_does_not_hold_a_start_given_up() {
        let strings = |all: &[&str]| all.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let limit = Duration::from_millis(200);

        let (exit, log, _, noted, failure, took) = given_up(false, None, false, limit);
        assert_eq!(exit, Exit::NotStarted);
        assert_eq!(
            log,
            strings(&["delete 3", "stopped"]),
            "nothing after the delete"
        );
        assert!(!noted && !failure);
        assert!(took >= limit && took < limit * 10, "{took:?}");

        let (exit, log, after, noted, failure, took) = given_up(false, Some(2), false, limit);
        assert_eq!(exit, Exit::Failed, "a failure after the stop is still one");
        assert_eq!(
            log,
            strings(&["delete 3", "stopped"]),
            "nothing after the delete"
        );
        assert_eq!(after.last().map(String::as_str), Some("store dropped"));
        assert!(!noted && failure, "the failure note is written");
        assert!(took < limit * 10, "{took:?}");

        // What the bounded control answers, and the line serve says.
        let keep = tempfile::tempdir().expect("a directory");
        let log = Log::default();
        let mut inner = Logged {
            log: log.clone(),
            run: keep.path().to_path_buf(),
            answers: false,
        };
        let commands = inner.clone();
        let mut bounded = Bounded::new(&mut inner, commands, limit);
        let unanswered = bounded.delete(3).expect_err("no answer");
        assert_eq!(unanswered.kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            unanswered.to_string(),
            "DEL_DEV_ASYNC of device 3 had no answer within the 200 ms that a start given up \
             waits for the kernel's commands: it stays with the driver, which may still carry it \
             out"
        );
        let unsent = bounded.info(3).expect_err("not sent");
        assert_eq!(
            unsent.to_string(),
            "GET_DEV_INFO and GET_PARAMS of device 3 was not sent: the 200 ms that a start given \
             up waits for the kernel's commands had run out"
        );
        assert_eq!(bounded.client_holds(3), None);
        assert_eq!(
            read(&log),
            strings(&["delete 3"]),
            "nothing sent after the limit"
        );
        assert_eq!(
            gave_up_said(
                3,
                false,
                &GaveUp::NoAnswer,
                Some(&Err(Undeleted::NoNote(unanswered)))
            ),
            format!(
                "the start of device 3 had no answer within {} s: serve gives it up; device 3, \
                 which this serve added and never started, is to be deleted: device 3 could not \
                 be deleted (DEL_DEV_ASYNC of device 3 had no answer within the 200 ms that a \
                 start given up waits for the kernel's commands: it stays with the driver, which \
                 may still carry it out) and is left without its note, which was not put back: \
                 deleting it by hand, or a reboot, resolves it; the volume is not failed; exit 9: \
                 a supervisor should retry the serve once; a second 9 is for a person",
                START_LIMIT.as_secs()
            )
        );
        assert_eq!(said_limit(GIVE_UP_LIMIT), "10 s");
        assert!(
            START_LIMIT + ANSWER_LOOK * ANSWER_LOOKS + GIVE_UP_LIMIT <= Duration::from_secs(45),
            "a start given up stays under half of systemd's stop timeout"
        );
    }

    /// How a fake kernel command answers (M1.3ak): after a while, with
    /// an error after a while (a command the driver refused), or never.
    #[derive(Debug, Clone, Copy)]
    enum Answer {
        After(Duration),
        FailsAfter(Duration),
        Never,
    }

    impl Answer {
        /// Waits as this answer does, and says whether it ends in an error.
        fn wait(self) -> bool {
            match self {
                Answer::After(time) => {
                    std::thread::sleep(time);
                    false
                }
                Answer::FailsAfter(time) => {
                    std::thread::sleep(time);
                    true
                }
                // The thread waits for good, as the driver's would
                // behind the device's mutex.
                Answer::Never => loop {
                    std::thread::park();
                },
            }
        }
    }

    /// Bounded commands that take their time (M1.3ak): each is logged
    /// when it is sent, then answers as set for it.
    #[derive(Clone)]
    struct Timed {
        log: Log,
        delete: Answer,
        info: Answer,
        holds: Answer,
    }

    impl Timed {
        /// The command named `kind` answers after `first`, the one named
        /// `second` never, and any other at once.
        fn first_then_never(log: &Log, first: (&str, Answer), second: &str) -> Self {
            let answer = |kind: &str| {
                if kind == first.0 {
                    first.1
                } else if kind == second {
                    Answer::Never
                } else {
                    Answer::After(Duration::ZERO)
                }
            };
            Self {
                log: log.clone(),
                delete: answer("delete"),
                info: answer("info"),
                holds: answer("holds"),
            }
        }
    }

    impl Commands for Timed {
        fn delete(&self, number: u32) -> io::Result<()> {
            logged(&self.log, format!("delete {number}"));
            if self.delete.wait() {
                Err(io::Error::other("the driver refused the delete"))
            } else {
                Ok(())
            }
        }
        fn info(&self, number: u32) -> io::Result<Option<skyzfs_ublk::lifecycle::DeviceInfo>> {
            logged(&self.log, format!("info {number}"));
            if self.info.wait() {
                Err(io::Error::other("the driver refused GET_PARAMS"))
            } else {
                Ok(None)
            }
        }
        fn client_holds(&self, number: u32) -> Option<bool> {
            logged(&self.log, format!("holds {number}"));
            if self.holds.wait() {
                None
            } else {
                Some(false)
            }
        }
    }

    /// Sends the command named `kind` through `bounded`, and says what it
    /// answered.
    fn sent(bounded: &mut Bounded<'_, Logged, Timed>, kind: &str) -> String {
        match kind {
            "delete" => match bounded.delete(3) {
                Ok(()) => "deleted".into(),
                Err(e) => e.to_string(),
            },
            "info" => match bounded.info(3) {
                Ok(_) => "info".into(),
                Err(e) => e.to_string(),
            },
            _ => format!("holds: {:?}", bounded.client_holds(3)),
        }
    }

    /// The give-up's commands share one limit (M1.3ak; S4 of #87's
    /// review): the deadline is the limit after the bounded control was
    /// made, not the limit after each command is sent. For each order of
    /// two of the three commands (the delete, the reading of the info,
    /// the open that asks whether a client holds the device), the first
    /// answers after half the limit and the second never: the second
    /// ends when the limit is up, not half a limit later, and the third
    /// is then not sent. With a deadline per command the second would end
    /// at one and a half limits, and the third would be sent.
    ///
    /// The orders run on threads of their own, at once. Each must end
    /// before one and a half limits, which leaves a right implementation
    /// half a limit for its threads to be woken. A deadline per command
    /// cannot: the first command's half limit and the second's whole one
    /// are each a sleep or a wait of at least that long, so the plant is
    /// caught whatever the machine's load.
    #[test]
    fn the_give_ups_commands_share_one_limit() {
        const KINDS: [&str; 3] = ["delete", "info", "holds"];
        let limit = Duration::from_secs(1);
        let mut orders = Vec::new();
        for first in KINDS {
            for second in KINDS.into_iter().filter(|&kind| kind != first) {
                let (done, order) = mpsc::channel();
                orders.push(order);
                std::thread::spawn(move || {
                    let log = Log::default();
                    let commands =
                        Timed::first_then_never(&log, (first, Answer::After(limit / 2)), second);
                    let mut inner = Logged {
                        log: log.clone(),
                        run: std::path::PathBuf::from("/nonexistent"),
                        answers: true,
                    };
                    let began = Instant::now();
                    let mut bounded = Bounded::new(&mut inner, commands, limit);
                    let answered = sent(&mut bounded, first);
                    let unanswered = sent(&mut bounded, second);
                    let took = began.elapsed();
                    let third = KINDS
                        .into_iter()
                        .find(|&kind| kind != first && kind != second)
                        .expect("a third command");
                    let unsent = sent(&mut bounded, third);
                    let _ = done.send((
                        first,
                        second,
                        answered,
                        unanswered,
                        took,
                        unsent,
                        read(&log),
                    ));
                });
            }
        }
        // Waited for with a limit, so that a command waited for with none
        // fails the test rather than hangs it.
        let given = Instant::now() + limit * 20 + Duration::from_secs(5);
        for order in orders {
            let (first, second, answered, unanswered, took, unsent, log) = order
                .recv_timeout(given.saturating_duration_since(Instant::now()))
                .expect("each order ends within its limits");
            let order = format!("{first} then {second}");
            assert!(
                took >= limit && took < limit * 3 / 2,
                "{order}: the second command ended {took:?} after the bounded control was made, \
                 with a limit of {limit:?}"
            );
            assert!(
                !answered.contains("no answer") && !answered.contains("not sent"),
                "{order}: {answered}"
            );
            match second {
                "holds" => assert_eq!(unanswered, "holds: None", "{order}"),
                _ => assert!(
                    unanswered.contains("had no answer within the 1 s"),
                    "{order}: {unanswered}"
                ),
            }
            match KINDS
                .into_iter()
                .find(|&kind| kind != first && kind != second)
            {
                Some("holds") => assert_eq!(unsent, "holds: None", "{order}"),
                _ => assert!(unsent.contains("was not sent"), "{order}: {unsent}"),
            }
            assert_eq!(
                log,
                [format!("{first} 3"), format!("{second} 3")],
                "{order}: nothing sent after the limit, and nothing unbounded"
            );
        }
    }

    /// A start given up, with [`Logged`] as the lifecycle's control and
    /// `commands` as the bounded ones, for a device 3 with its note.
    /// `took_over` and `fails_at` are as in [`given_up`]; the stop takes
    /// `stop_takes`. Returns the exit, the log, whether a failure note is
    /// there, and how long it took.
    fn given_up_with(
        log: &Log,
        commands: Timed,
        took_over: bool,
        fails_at: Option<u32>,
        stop_takes: Duration,
        limit: Duration,
    ) -> (Exit, Vec<String>, bool, Duration) {
        let (done, outcome) = mpsc::channel();
        let log = log.clone();
        std::thread::spawn(move || {
            use skyzfs_ublk::lifecycle::Records;
            let keep = tempfile::tempdir().expect("a directory");
            let (volume, run) = (keep.path().join("volume"), keep.path().join("run"));
            std::fs::create_dir(&volume).expect("the volume's directory");
            std::fs::create_dir(&run).expect("the run directory");
            let mut records = Files::new(&volume, &run, [1; 16], Duration::ZERO).expect("records");
            records.create_run_dirs().expect("the run directories");
            records.write_note(3, &[9; 16]).expect("a note");
            let mut kernel = Logged {
                log: log.clone(),
                run,
                answers: true,
            };
            let looks = std::cell::Cell::new(0);
            let failed = || {
                looks.set(looks.get() + 1);
                fails_at.is_some_and(|at| looks.get() >= at)
            };
            let began = Instant::now();
            let (exit, _kept) = give_up_unanswered(
                &mut kernel,
                commands,
                limit,
                &mut records,
                3,
                took_over,
                [1; 16],
                failed,
                || {
                    std::thread::sleep(stop_takes);
                    logged(&log, "stopped");
                    Some(())
                },
            );
            let took = began.elapsed();
            let failure = records.failure().ok().flatten().is_some();
            let _ = done.send((exit, read(&log), failure, took));
        });
        outcome
            .recv_timeout(limit * 20 + Duration::from_secs(5))
            .expect("a start given up ends within its limits")
    }

    /// A start given up ends within one limit, whichever of its commands
    /// takes its time and whichever has no answer (M1.3ak; S4 of #87's
    /// review). Five ways through it, between them every pair of
    /// commands the give-up can send one after the other. In each, the
    /// commands before the last take half the limit between them, and the
    /// last never answers:
    /// - a failure flagged before the delete: the open that asks whether
    ///   a client holds the device answers late, the delete never; exit
    ///   3, the failure note written;
    /// - at a start: the delete is refused late, the reading of the
    ///   device's info that follows a refused delete never answers;
    ///   exit 9;
    /// - a failure flagged after the stop: the delete answers late, and
    ///   the open never; exit 3. One bounded control serves the whole
    ///   give-up, the failure's way after the stop included;
    /// - a refused delete, then a failure flagged after the stop: the
    ///   info that follows the refusal answers late (the device is gone),
    ///   and the open never; exit 3;
    /// - a failure flagged after a take-over's stop, which takes half the
    ///   limit and comes before any command: the open never answers;
    ///   exit 3. The limit runs from when the give-up began, not from its
    ///   first command.
    ///
    /// The five run on threads of their own, at once. Each must end
    /// before one and a half limits, as in
    /// [`the_give_ups_commands_share_one_limit`]. Each of these takes at
    /// least one and a half limits: a deadline per command, a bounded
    /// control of its own for the failure's way, or a deadline counted
    /// from the first command sent.
    #[test]
    fn a_start_given_up_ends_within_one_limit_whatever_its_commands_wait_for() {
        use Answer::{After, FailsAfter, Never};
        let limit = Duration::from_secs(1);
        let (late, now) = (limit / 2, Duration::ZERO);
        let strings = |all: &[&str]| all.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // (way, (delete, info, open), took over, the look that fails, the
        // stop's time, exit, the commands sent and the stop, in order)
        let ways = [
            (
                "a failure before the delete",
                (Never, After(now), After(late)),
                false,
                Some(1),
                now,
                Exit::Failed,
                strings(&["holds 3", "delete 3", "stopped"]),
            ),
            (
                "at a start",
                (FailsAfter(late), Never, After(now)),
                false,
                None,
                now,
                Exit::NotStarted,
                strings(&["delete 3", "info 3", "stopped"]),
            ),
            (
                "a failure after the stop",
                (After(late), After(now), Never),
                false,
                Some(2),
                now,
                Exit::Failed,
                strings(&["delete 3", "stopped", "holds 3"]),
            ),
            (
                "a refused delete, then a failure after the stop",
                (FailsAfter(now), After(late), Never),
                false,
                Some(2),
                now,
                Exit::Failed,
                strings(&["delete 3", "info 3", "stopped", "holds 3"]),
            ),
            (
                "a failure after a take-over's stop",
                (After(now), After(now), Never),
                true,
                Some(2),
                late,
                Exit::Failed,
                strings(&["stopped", "holds 3"]),
            ),
        ];
        let ways = ways.map(
            |(
                way,
                (delete, info, holds),
                took_over,
                fails_at,
                stop_takes,
                wanted,
                sent_in_order,
            )| {
                let given_up = std::thread::spawn(move || {
                    let log = Log::default();
                    let commands = Timed {
                        log: log.clone(),
                        delete,
                        info,
                        holds,
                    };
                    given_up_with(&log, commands, took_over, fails_at, stop_takes, limit)
                });
                (way, fails_at, wanted, sent_in_order, given_up)
            },
        );
        for (way, fails_at, wanted, sent_in_order, given_up) in ways {
            // given_up_with waits for the give-up with a limit.
            let (exit, log, failure, took) = given_up.join().expect("a way's thread");
            assert_eq!(exit, wanted, "{way}");
            assert_eq!(log, sent_in_order, "{way}: nothing sent after the limit");
            assert_eq!(failure, fails_at.is_some(), "{way}: the failure note");
            assert!(
                took >= limit && took < limit * 3 / 2,
                "{way}: the give-up took {took:?} with a limit of {limit:?}"
            );
        }
    }

    /// A delete with no answer gives 9, and the line names the delete
    /// (M1.3ak): the give-up ends with 9, and what the lifecycle's delete
    /// made of the bounded delete's error, said as serve says it, names
    /// DEL_DEV_ASYNC and the limit. #87's test said the line from an
    /// error it made itself; this one says it from the give-up's own.
    #[test]
    fn a_delete_with_no_answer_gives_9_and_a_line_that_names_it() {
        use skyzfs_ublk::lifecycle::Records;
        let limit = Duration::from_millis(200);

        let log = Log::default();
        let commands =
            Timed::first_then_never(&log, ("info", Answer::After(Duration::ZERO)), "delete");
        let (exit, sent_in_order, failure, _) =
            given_up_with(&log, commands, false, None, Duration::ZERO, limit);
        assert_eq!(exit, Exit::NotStarted);
        assert_eq!(sent_in_order, ["delete 3", "stopped"]);
        assert!(!failure);

        let keep = tempfile::tempdir().expect("a directory");
        let (volume, run) = (keep.path().join("volume"), keep.path().join("run"));
        std::fs::create_dir(&volume).expect("the volume's directory");
        std::fs::create_dir(&run).expect("the run directory");
        let mut records = Files::new(&volume, &run, [1; 16], Duration::ZERO).expect("records");
        records.create_run_dirs().expect("the run directories");
        records.write_note(3, &[9; 16]).expect("a note");
        let log = Log::default();
        let mut inner = Logged {
            log: log.clone(),
            run,
            answers: true,
        };
        let commands =
            Timed::first_then_never(&log, ("info", Answer::After(Duration::ZERO)), "delete");
        let mut bounded = Bounded::new(&mut inner, commands, limit);
        let deleted = give_up_start(&mut bounded, &mut records, 3, false);
        assert_eq!(read(&log), ["delete 3"], "nothing sent after the delete");
        assert_eq!(
            records.note(3).ok().flatten(),
            None,
            "the note is not put back"
        );
        assert_eq!(
            gave_up_said(3, false, &GaveUp::NoAnswer, deleted.as_ref()),
            format!(
                "the start of device 3 had no answer within {} s: serve gives it up; device 3, \
                 which this serve added and never started, is to be deleted: device 3 could not \
                 be deleted (DEL_DEV_ASYNC of device 3 had no answer within the 200 ms that a \
                 start given up waits for the kernel's commands: it stays with the driver, which \
                 may still carry it out) and is left without its note, which was not put back: \
                 deleting it by hand, or a reboot, resolves it; the volume is not failed; exit 9: \
                 a supervisor should retry the serve once; a second 9 is for a person",
                START_LIMIT.as_secs()
            )
        );
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

    impl skyzfs_ublk::pump::PumpTarget for Silent {
        fn gone(&self) -> bool {
            false
        }
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

    /// A failed volume holds no request (M1.3d), on the queue's thread
    /// as `serve` runs it: with three requests in a store that never
    /// answers, nothing is committed while the volume has not failed;
    /// the turn after the store said so, all three are committed with
    /// the I/O error and the queue is woken; and the call after the
    /// next wait, which is what sends them, tells the main thread that
    /// they are answered, and how many.
    #[test]
    fn the_queues_thread_answers_what_a_silent_store_holds_once_the_volume_failed() {
        let committed = Arc::new(std::sync::Mutex::new(Vec::new()));
        let queue = Three {
            committed: committed.clone(),
            ..Three::default()
        };
        let queue = Delayed::new(queue, None, None, || {});
        let mut pump = Pump::new(queue, Silent(Vec::new()));
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

    /// The start says once which of the labels' rings it could not read,
    /// and nothing when it read them all (the review of #95, its S3).
    #[test]
    fn the_start_says_when_it_could_not_read_a_ring() {
        assert_eq!(rings_unread_said(&[]), None);
        let unread = [
            (393_216, "Input/output error (os error 5)".to_string()),
            (8_650_752, "Input/output error (os error 5)".to_string()),
        ];
        let Some(said) = rings_unread_said(&unread) else {
            panic!("nothing said of two rings not read");
        };
        assert!(said.contains("could not read 2 of the labels' 4"), "{said}");
        assert!(said.contains("byte 393216: Input/output error"), "{said}");
        assert!(said.contains("no cut opens as itself"), "{said}");
    }

    /// The channel answers `no-upload` with why there is no uploader; a
    /// channel that cannot be made is said and the serve goes on. Each
    /// says what it is to the board: `no_upload` and its figures,
    /// `uploads`, or `failed`.
    ///
    /// Mutant: a reason dropped, a failed bind that ends the serve, a
    /// state or a figure not posted.
    #[test]
    fn the_channel_says_why_it_publishes_nothing_and_a_failed_one_is_left() {
        use crate::control::{ask, Code, Reply, Request, IO_LIMIT, SOCKET};
        use std::sync::Mutex;
        let keep = tempfile::tempdir().expect("a directory");
        let path = keep.path().join(SOCKET);
        let (requests, _inbox) = tokio::sync::mpsc::channel(1);
        let posts = Arc::new(Mutex::new(Vec::new()));
        let post: ChannelPost = {
            let posts = Arc::clone(&posts);
            Arc::new(move |shown| posts.lock().expect("posts").push(shown))
        };
        let taken = || std::mem::take(&mut *posts.lock().expect("posts"));
        let why = no_upload_why(None);
        assert!(why.contains("no place in object storage"), "{why}");
        let channel = open_channel(
            &path,
            Err(why.clone()),
            requests.clone(),
            post.clone(),
            |h| h,
        )
        .expect("made");
        let reply = ask(&path, &Request::Cuts, &[], IO_LIMIT).expect("answered");
        assert_eq!(reply, Reply::Refused(Code::NoUpload, why));
        assert!(channel.close());
        let quiet = ChannelShown {
            state: ChannelStateShown::NoUpload,
            ..ChannelShown::default()
        };
        assert_eq!(
            taken(),
            [
                quiet,
                ChannelShown {
                    answered: 1,
                    ..quiet
                }
            ]
        );

        let channel =
            open_channel(&path, Ok([1; 16]), requests.clone(), post.clone(), |h| h).expect("made");
        assert!(channel.close());
        assert_eq!(
            taken(),
            [ChannelShown {
                state: ChannelStateShown::Uploads,
                ..ChannelShown::default()
            }]
        );

        let nowhere = keep.path().join("missing").join(SOCKET);
        assert!(open_channel(&nowhere, Ok([1; 16]), requests, post, |h| h).is_none());
        assert_eq!(
            taken(),
            [
                ChannelShown {
                    state: ChannelStateShown::Uploads,
                    ..ChannelShown::default()
                },
                ChannelShown {
                    state: ChannelStateShown::Failed,
                    ..ChannelShown::default()
                }
            ]
        );
    }
}

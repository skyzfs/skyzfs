//! What the in-place store ([`crate::inplace`]) shares with its owner:
//! the names of its files, who is told of a failure and woken for a
//! result, and how write-zeroes reach the home image.
//!
//! Until M1.3t this module was the log store: a block target on a
//! write log and the home image. The log store is gone. What stays
//! here beside the in-place store's own types are the log store's
//! timing types ([`Timing`] and the types in it), which the daemon's
//! status file still carries and reads in its older versions: their
//! fields stay, zeros, until the status file's next version.

use std::io;
use std::sync::Arc;
use std::time::Duration;

use crate::io::File;
use crate::writeback::Hinted;

/// The home image's file.
pub const HOME_NAME: &str = "home.img";
/// The home image while it is being created.
pub(crate) const HOME_NEW_NAME: &str = "home.new";

/// Every name a creation of a volume that was interrupted can leave in
/// the directory, and so every name [`crate::inplace::create`] accepts
/// in it: the lock file, the home image under its first and its final
/// name, and the volume file under its first name. A creation removes
/// these, by these exact names, and no other file. A directory that
/// holds any other name is refused
/// ([`OpenError::NotEmpty`](crate::inplace::OpenError::NotEmpty))
/// with nothing removed and nothing written: the store cannot know
/// whose `home.tar` or `volume.txt` it is. A file that somebody else
/// put under exactly one of these names, in a directory with no
/// volume, is removed as a leftover: the store cannot tell it from one.
pub const CREATE_LEFTOVERS: [&str; 4] = [
    crate::fs::LOCK_NAME,
    HOME_NEW_NAME,
    HOME_NAME,
    crate::volume::VOLUME_NEW_NAME,
];

/// The first name in `names` that is not one of [`CREATE_LEFTOVERS`]:
/// what makes [`crate::inplace::create`] refuse a directory that holds
/// no volume.
pub fn stranger(names: &[String]) -> Option<&str> {
    names
        .iter()
        .map(String::as_str)
        .find(|name| !CREATE_LEFTOVERS.contains(name))
}

/// Who is told that the volume has failed: see
/// [`OpenOptions::on_failure`](crate::inplace::OpenOptions::on_failure).
#[derive(Clone)]
pub struct FailureObserver(Arc<dyn Fn() + Send + Sync>);

impl FailureObserver {
    /// An observer that calls `f`.
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Tell it.
    pub(crate) fn call(&self) {
        (self.0)()
    }
}

impl std::fmt::Debug for FailureObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FailureObserver")
    }
}

/// What the in-place store calls when its owner has something to take:
/// a sync that a flush began has ended on the file's helper, or an I/O
/// thread has a result ([`Opened::start`](crate::inplace::Opened::start)).
/// A transport that sleeps on something else, such as an event
/// descriptor in its ring, is woken through it. It is called from those
/// threads and must return quickly.
#[derive(Clone)]
pub struct Waker(Arc<dyn Fn() + Send + Sync>);

impl Waker {
    /// A waker that calls `f`.
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// What it calls.
    pub(crate) fn into_call(self) -> Arc<dyn Fn() + Send + Sync> {
        self.0
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

/// A count of things that took time, their summed time and the
/// longest of them, in nanoseconds of the monotonic clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Spent {
    /// How many.
    pub count: u64,
    /// Their time together.
    pub sum_ns: u64,
    /// The longest.
    pub max_ns: u64,
}

impl Spent {
    /// Count one more thing that took `time`.
    pub fn add(&mut self, time: Duration) {
        let ns = u64::try_from(time.as_nanos()).unwrap_or(u64::MAX);
        self.count += 1;
        self.sum_ns = self.sum_ns.saturating_add(ns);
        self.max_ns = self.max_ns.max(ns);
    }
}

/// The kinds of request the timing tells apart, in the order of
/// [`Timing::requests`].
pub const TIMED_KINDS: [&str; 5] = ["read", "write", "flush", "discard", "write_zeroes"];

/// How many activities a worker has: see [`ACTIVITIES`].
pub const ACTIVITY_COUNT: usize = 16;

/// The names of what a worker can be doing, in the order of
/// [`Timing::worker`]: idle; the call of a read, a write, a flush (the
/// log's sync), a discard, a write-zeroes; the six parts of a step
/// and the rule's own sync of the log; anything else; a step at its
/// barriers; waiting for the helper's report.
pub const ACTIVITIES: [&str; ACTIVITY_COUNT] = [
    "idle",
    "read",
    "write",
    "flush",
    "discard",
    "zeroes",
    "step_walk",
    "step_save",
    "step_apply",
    "step_home_sync",
    "step_log_start",
    "step_end",
    "own_sync",
    "other",
    "step_barrier",
    "step_report",
];

/// What one kind of request waited and took.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RequestTiming {
    /// From the request's submission to the served store until the
    /// worker began it: the time in the worker's queue, behind other
    /// requests, behind checkpoint steps and at the high mark's gate.
    pub wait: Spent,
    /// The store's call for it: from when the worker began it until
    /// its completion was ready. The two counts are the same.
    pub call: Spent,
}

/// The log store's timing, which the daemon's status file still carries
/// (zeros since M1.3t removed the log store; older status files are read
/// into it). As it was:
///
/// Where a served store's time went since the open: what each kind of
/// request waited and took, what the checkpoint steps took by part,
/// and what requests waited for steps. It is measurement only: nothing
/// in the store reads it.
///
/// The cost is one read of the monotonic clock per boundary: three per
/// request (its submission, its begin, its end), eight per batch of a
/// checkpoint, two per sync of the rule's own, two per turn of the
/// worker at the gate, and two around each wait for a request. What
/// the worker did is kept as one entry per span in a queue that is
/// reused (no allocation once it has grown to the longest wait seen),
/// and a request's wait is laid over the spans since its submission:
/// a walk of as many entries as the worker did things meanwhile. No
/// lock but the one the worker already takes to show the log's state,
/// held for a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    /// By kind, in the order of [`TIMED_KINDS`]. Requests that
    /// completed, whatever their result. A served store only.
    pub requests: [RequestTiming; 5],
    /// The parts of a checkpoint's batch, which follow each other:
    /// every step is one batch (an explicit checkpoint's batches count
    /// too, and so does one a write makes inside its own call to make
    /// room: there too the parts are the step's in the worker's time
    /// and in the waits, and the write's activity is the rest of its
    /// call, the sync of the log before the checkpoint included; the
    /// write's `call` is its whole call). First: reading the log to choose the batch. For an
    /// explicit checkpoint this is one walk for all its batches.
    pub step_walk: Spent,
    /// Saving the neighbours of what the batch writes, with that
    /// file's sync.
    pub step_save: Spent,
    /// Writing and deallocating in the home image.
    pub step_apply: Spent,
    /// The worker's part in the home image's sync: the whole sync where
    /// it makes it itself (an explicit checkpoint's, one that makes room,
    /// every step of a store that does not hand it over), and handing
    /// it over where a step of the rule's gives it to the helper
    /// ([`Timing::helper`]). One per batch, as the other parts.
    pub step_home_sync: Spent,
    /// The log's new start, written and made durable.
    pub step_start: Spent,
    /// The index letting go of what is now in the home image.
    pub step_end: Spent,
    /// The rule's own syncs of the log, before a step that found too
    /// little of the log durable. Not part of the six above.
    pub step_own_sync: Spent,
    /// Turns of the worker in which a request was waiting and none
    /// was taken because the log was at the high mark: the time of
    /// the step taken in that turn (its own sync included). This
    /// overlaps the parts above: it is a share of them, not more time.
    pub gate: Spent,
    /// The worker's whole time since it started, by what it did, in
    /// nanoseconds and in the order of [`ACTIVITIES`], idle included:
    /// the spans follow each other with no gap, so the sum is the time
    /// served, and one entry over the sum is the share of the worker's
    /// time that went there.
    pub worker: [u64; ACTIVITY_COUNT],
    /// For each kind of request ([`TIMED_KINDS`]): the summed waits of
    /// [`RequestTiming::wait`] by what the worker did meanwhile
    /// ([`ACTIVITIES`]). Each request's wait is laid over the worker's
    /// spans, so a row's sum is that kind's summed wait. "Idle" here is
    /// the time the worker took to wake for a request.
    pub wait_by: [[u64; ACTIVITY_COUNT]; 5],
    /// For each kind: of its longest wait, the activity that took the
    /// largest part of it, as a number into [`ACTIVITIES`].
    pub longest_wait_cause: [u64; 5],
    /// And how long that part was.
    pub longest_wait_cause_ns: [u64; 5],
    /// How long steps waited at their first barrier, before the home
    /// image is written, for the readers under way: once per batch.
    /// This time is the activity "step_barrier" and in none of the
    /// step's parts above.
    pub barrier_home: Spent,
    /// The same at the second barrier, before a segment file the step
    /// freed can be used again.
    pub barrier_reuse: Spent,
    /// The readers' whole time since they started, in nanoseconds:
    /// idle, in the calls of reads, and anything else. For each reader
    /// the three follow each other with no gap, so their sum is the
    /// time the reader has served. With several readers
    /// (the log store's `readers`) each is the sum over the readers, in
    /// thread-nanoseconds: a share of their sum is the share of the
    /// readers' time on average, and the sum itself is as many times
    /// the time served as there are readers. A served store only.
    pub reader: [u64; 3],
    /// The reads that found a block in a record of the log that was
    /// not durable yet, whose payload the reader then read whole and
    /// checked against its checksum (a failed sync can take such a
    /// record's content back: see the design, §5.1): how many reads; the
    /// bytes they read beyond those asked for; how many records did not
    /// match, each of which failed the volume. A served store only.
    pub read_unflushed: [u64; 3],
    /// Up to when the worker's activities ([`Timing::worker`]) are
    /// complete, in nanoseconds since the store began to be served.
    /// A worker that waits for work at the look has the wait counted
    /// as idle up to the look, and this is the look's moment; a worker
    /// that is doing something has its sums up to the end of its last
    /// span and this is that end: what it is doing now is counted when
    /// it is done, under what it was, and never first under something
    /// else. So between two looks the worker's activities sum to the
    /// difference of this exactly, and no sum goes down. A served
    /// store only.
    pub worker_as_of_ns: u64,
    /// The same for the readers' activities ([`Timing::reader`]), summed
    /// over the readers: each reader's moment, as this says it for one
    /// thread, added up. So between two looks the readers' activities
    /// sum to the difference of this exactly, with any number of
    /// readers, and the difference is about the number of readers
    /// times the time between the looks.
    pub reader_as_of_ns: u64,
    /// The hint that hands writes to the device before their sync
    /// ([`crate::writeback`]): for the log's segment files, and for the
    /// home image. Its time lies inside the activity it is made from:
    /// the log's in the call of the write, the write-zeroes or the
    /// discard that appended (or in a step's own sync of a log that
    /// rolled there), the home image's in a step's apply.
    pub writeback: [Hinted; 2],
    /// Each kind of sync, in the order of [`SYNC_KINDS`]: how many,
    /// their time, the bytes written to the file since its sync before,
    /// and of those how many the hint had handed over. The superblock's
    /// are since the store was opened.
    pub syncs: [Synced; 6],
    /// The home image's syncs that steps handed to the helper: from the
    /// hand-over until the worker took the report. Not worker time: the
    /// worker served meanwhile. The syncs are also counted in
    /// [`Timing::syncs`] under `home`, with this time.
    pub helper: Spent,
    /// How long the worker waited for the helper's report (the activity
    /// `step_report`): at the high mark, before an explicit checkpoint,
    /// a change of the hold, room in a full log, and the end.
    pub step_report: Spent,
    /// Requests the worker completed while a sync was with the helper.
    pub helper_requests: u64,
}

/// One kind of sync: see [`Timing::syncs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Synced {
    /// How many.
    pub count: u64,
    /// Their time together, in nanoseconds.
    pub sum_ns: u64,
    /// The bytes written to the file since the sync before each.
    pub bytes: u64,
    /// Of those, the bytes the hint had handed over.
    pub handed: u64,
}

/// The kinds of sync [`Timing::syncs`] tells apart: a client's flush
/// (and the log store's `sync_log`); the checkpoint rule's own sync of the
/// log; a sync of the log that makes room in a full log for a write;
/// a step's sync of the home image; of the preimage file; of the log's
/// superblock.
pub const SYNC_KINDS: [&str; 6] = [
    "flush",
    "own_sync",
    "room",
    "home",
    "preimage",
    "superblock",
];

/// How the in-place store's write-zeroes reach the home image, as the
/// daemon's status file shows it ([`InPlace::zeroes`](crate::inplace::InPlace::zeroes)).
/// With direct I/O, the store's only served mode, the range is written
/// as zeros by direct I/O ([`Zeroes::WrittenDirect`]): a deallocated
/// range would make the next writes to it allocate again, which a sync
/// must then make durable as metadata. Through the cache (test builds
/// only) the range is deallocated, or written as zeros where the file
/// system cannot deallocate. The first write-zeroes after a start says
/// which. The values `Deallocated` and `Written` are also what the log
/// store said, before M1.3t, of its checkpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zeroes {
    /// No write-zeroes has been applied since the start.
    #[default]
    Untried,
    /// The ranges are deallocated.
    Deallocated,
    /// The file system cannot deallocate: zeros are written.
    Written,
    /// Zeros are written by direct I/O: the in-place store's way
    /// (decision D6).
    WrittenDirect,
}

/// Write `len` zeros at `offset`, a MiB at a time.
pub(crate) fn write_zeros<F: File>(file: &mut F, offset: u64, len: u64) -> io::Result<()> {
    const CHUNK: u64 = 1 << 20;
    let zeros = vec![0u8; len.min(CHUNK) as usize];
    let mut at = 0;
    while at < len {
        let n = (len - at).min(CHUNK);
        file.write_at(offset + at, &zeros[..n as usize])?;
        at += n;
    }
    Ok(())
}

//! The in-place store: the volume's home image, written in place by
//! direct I/O and synced at a flush, with no write log (work packages
//! M1.3r to M1.3t).
//!
//! A volume is a directory that holds the volume file
//! ([`crate::volume`]), which says what the volume is, and the home
//! image (`home.img`), which holds it. [`create`] makes one; a start
//! opens it in two phases, [`Opened::open`] under the volume's lock and
//! [`Opened::start`] once the daemon may serve (the `start` module).
//! Beside them the directory holds the lock file and, once the volume
//! has failed, the failure marker (`volume.failed`). Until M1.3t the
//! store was entered from the log store, whose open applied its log to
//! the home image first; the log store is gone, and a directory it made
//! is refused ([`crate::volume::VolumeError::Legacy`]).
//!
//! # What it promises: the volatile-cache disk
//!
//! What it promises is what a disk with a volatile write cache promises,
//! the disk ZFS is built for: **the volatile-cache disk**, which is rule
//! R10 of the block contract since decision D5 (design §4.1). The
//! contract's first version asked for a prefix of the daemon's order
//! with every operation whole; a write in place is whole only if a
//! journal makes it so, and the journal was the log, which D5 dropped.
//! The points, as this store keeps them:
//!
//! 1. **Process loss.** A write or write-zeroes is acknowledged once
//!    its direct write has returned: it has reached the device (its
//!    volatile write cache, where it has one), past the kernel's cache
//!    (decision D6, [`IoMode`]). It is there after the process is lost,
//!    and so is everything acknowledged before it (rules R9 and R8);
//!    this no longer rests on the page cache. A discard does nothing
//!    (see below). A write in flight when the process dies may be there
//!    in part, block by block; the kernel issues it again (rule R9),
//!    and the next instance makes it whole. Where the write went into a
//!    hole or an unwritten extent, a returned direct write is not
//!    durable on its own: it survives a power cut only once the file
//!    system has committed its record that the block is written, which
//!    the next sync does. Until then a power cut may leave the block
//!    reading as zeros, its content before the write, which point 3
//!    allows; after a completed flush it never does. The replay rig
//!    showed both on ext4 and XFS (design §10, the allocation row):
//!    preallocating does not change it, and an image written through
//!    first does not show it. So the image stays sparse, and `skyzfsd
//!    create --zero-fill` (which writes it whole) is an option. That the
//!    record is visible to the next instance at once, after a lost
//!    process, is my reading of the two file systems, not shown here.
//! 2. **A flush** completes only when every operation acknowledged
//!    before it was received is durable (rule R7).
//! 3. **Power loss.** Every block that a completed flush covered holds
//!    what it held at that flush, or what was written to it after. Of
//!    a block written since its last flush, each sector holds the
//!    block's durable content or one of the versions written since,
//!    independently of every other sector and of every other block: a
//!    later write can survive while an earlier one is lost, and a
//!    write can be torn at a sector. A start makes what it finds
//!    durable before it serves ([`Opened::start`]): its sync of the
//!    home image makes durable what the instance before it wrote by
//!    direct I/O (on the device, perhaps only in its cache). A test
//!    pins it (`a_power_cut_after_a_start_keeps_what_the_instance_before_wrote`).
//!    The start then serves only what the disk holds: it drops the
//!    kernel's cache of the home image and checks that no page of it is
//!    left (`cachestat`), or refuses the volume (design §4.1). A failed
//!    sync can leave pages marked clean that the disk never got, and no
//!    later sync writes them; served from the cache, they would be
//!    shown and then taken by a cut behind a completed flush. The drop
//!    covers a start that acknowledges a failure as well as one after
//!    an instance that died between a failed sync and its marker, whose
//!    own sync succeeds because the kernel reports such an error once.
//!    Under direct I/O the store's own reads and writes leave no page
//!    in the cache, so such pages come only from a writer through it: a
//!    file system that accepts a direct request, reports an alignment
//!    for it and still serves it through its cache. (Until M1.3t the log
//!    store's checkpoint at the entry into this store was one too.) The
//!    start's check of direct I/O (`STATX_DIOALIGN`) cannot tell such a
//!    file system apart, so the drop stays. The design's example, ext4 with `data=journal`, is
//!    not one: as read in ext4 and not shown, its `statx` reports no
//!    direct-I/O alignment for an inode whose data is journalled, so the
//!    start refuses that volume by reason instead of serving it. No file
//!    system is known here that passes the check and serves through the
//!    cache; the drop is for one that might. Its cost: the first reads
//!    after every start, a take-over included, come from the disk.
//! 4. **Failure is final** (rule R11's failure rule). Once a write or
//!    a sync of the home image has failed, the volume has failed: from
//!    then on every request fails, reads included, and a later start
//!    refuses the volume until the failure is acknowledged (the failure
//!    marker, `volume.failed`). A failed sync is never tried again: the kernel may have
//!    dropped the pages and the error with them. What a completed flush
//!    covered is intact after a failure; a failed write or sync may
//!    leave every other block as point 3 leaves it at a power cut,
//!    which is what rule R11 asks of a failed volume that is opened
//!    again.
//!
//! Where the disk writes a sector neither old nor new at a power cut
//! (the simulated disk's garbage), point 3 holds for no sector a power
//! cut could take, and nothing about such a sector is promised. A file
//! system can do the same: ext4 with `data=writeback` (the CI runners'
//! root is mounted so) may show a block that was allocated since the
//! last flush holding bytes this store never wrote, another file's
//! included. That too is a sector the cut could take, and the same
//! holds of it: nothing is promised.
//!
//! The store has no log, so it reports no position of what was
//! acknowledged, and the daemon stores none (`skyzfsd`'s `Target`). At
//! a start after a lost process nothing can show that the home image
//! holds less than the kernel was told: the store's promise rests on
//! the direct write having reached the device for that (point 1).
//!
//! Where the page or the file system's block is larger than a block
//! (16 KiB or 64 KiB pages on some arm64 kernels and on Apple silicon),
//! a direct write still writes only the blocks it is given: it does not
//! rewrite the other blocks of its page, as a write through the cache
//! does. The tests run at 16 KiB and 64 KiB pages on the simulated disk
//! (`tests.rs`).
//!
//! **A limit that direct I/O removes, on the simulated disk: a disk
//! that garbles.** On a disk that leaves a sector neither old nor new at
//! a power cut, a write through the cache on a large page rewrites the
//! page's other blocks and exposes them, so a cut can damage a block a
//! completed flush covered and nothing wrote since: in the store's
//! power-cut test, through the cache, at 120 of 200 seeds at 16 KiB and
//! 113 at 64 KiB (measured in review), and at 0 at 4 KiB.
//! `a_cut_with_garbage_may_reach_a_neighbour_on_large_pages` pins that
//! some seed does so through the cache, which only tests still use;
//! with direct I/O no seed of the 200 does at either large page
//! (`direct_io_keeps_a_cut_with_garbage_from_the_neighbours_on_large_pages`,
//! which says what that rests on on a real file system). That is the
//! reason for decision D6.
//!
//! The other rules (R1 to R9, R11, R12) hold, and the conformance
//! suite runs on this store with every capability (`harness.rs`).
//!
//! # How it executes
//!
//! Reads and writes go by direct I/O to a pool of I/O threads
//! ([`IoMode::threads`], eight unless asked otherwise), each with its
//! own handle on the home image and one request at a time; their
//! completions are taken back on the caller's thread. A request waits,
//! in order, behind one on the threads that it must not run beside: a
//! read behind an overlapping write, a write or a write-zeroes behind
//! an overlapping read or write, so that overlapping requests take
//! effect in the order they came; and a flush behind every write on the
//! threads (it covers what was acknowledged before it, and this store
//! also has it wait for what was submitted before it). Everything
//! received after a waiting request waits behind it. On the simulated
//! disk a write applies whole under the disk's lock, so a read beside
//! an overlapping write never sees part of it and the tests that run
//! whole programs say little of the clauses about reads; on a file
//! system a direct read beside an overlapping direct write may see part
//! of it, which is what those clauses are for, and only the kernel rigs
//! show them there. Each clause has a test of its own (`tests.rs`: a
//! write behind a read, a read behind a write, a flush behind a write
//! beside a read). A write-zeroes is written as zeros by direct I/O
//! on the caller's thread, 1 MiB at a time (a deallocated range would
//! make the next writes to it allocate again), and a discard does
//! nothing. A thread that panics dies, the
//! job it held comes back failed, and the volume fails as from a
//! failed write; the requests that waited for the threads fail with it
//! (`inplace/pool.rs`). The store's drop waits for every request on the
//! threads, up to one per thread, where on the caller's thread there
//! was at most one: a daemon's exit waits for the direct I/O it has in
//! flight (the daemon rig's check e watches a request held at the
//! server). The store's queue (on the threads, waiting, and the
//! highs) is in [`InPlace::io_queue`], which the daemon's status file
//! shows.
//!
//! Rule R6 allows a discarded block to keep its content, and that is
//! the least work: no call, no metadata to make durable, and nothing a
//! power cut can tear. It counts as a change all the same, so that the
//! flush behind it syncs: a flush that follows a client's change never
//! succeeds on a disk that has failed (R11), at the cost of one sync
//! where a discard alone came before it. A flush begins a sync of the
//! home image on the file's helper ([`File::sync_begin`]) and completes
//! when the sync ends; writes go on meanwhile. Flushes that arrive
//! while a sync is under way join it when nothing was written since it
//! began, and otherwise wait for the next one, which begins when this
//! one ends and covers them all.
//!
//! Test builds can also do the I/O on the caller's thread, in
//! [`BlockTarget::submit`], or through the kernel's cache, where a
//! write-zeroes deallocates the range (`IoMode::for_tests`). Neither is
//! a mode the store is served in.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;
use std::time::Duration;

use skyzfs_blk::{
    BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind,
};

use crate::io::{Dir, File, NotBegun};
use crate::store::{FailureObserver, Zeroes};
use pool::{Done, Job, Pool};

pub use start::{create, identity_of, lock, OpenError, OpenOptions, Opened, Under, VolumeConfig};

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// A flush that arrives while a sync is under way joins it even
    /// when something was written since the sync began.
    pub(crate) const FLUSH_JOINS_ANY_SYNC: u32 = 1 << 0;
    /// After a failed sync the volume is not failed: the next flush
    /// tries again.
    pub(crate) const SYNC_RETRIED: u32 = 1 << 2;
    /// A flush with no sync under way completes at once whatever was
    /// written since the last sync.
    pub(crate) const FLUSH_WITHOUT_SYNC: u32 = 1 << 3;
    /// A failed write does not fail the volume.
    pub(crate) const WRITE_ERROR_PASSES: u32 = 1 << 4;
    /// A discard is not counted as a change: a flush behind it alone
    /// does not sync.
    pub(crate) const DISCARD_UNCHANGED: u32 = 1 << 5;
    /// No thread is started for the failure marker: it is written on
    /// the caller's thread when the volume fails.
    pub(crate) const MARKER_INLINE: u32 = 1 << 6;
    /// The start does not drop the home image's cache.
    pub(crate) const CACHE_NOT_DROPPED: u32 = 1 << 7;
    /// The start does not check that the cache is gone.
    pub(crate) const CACHE_NOT_CHECKED: u32 = 1 << 8;
    /// The start does not show that the count sees the file.
    pub(crate) const CACHE_SIGHT_UNCHECKED: u32 = 1 << 9;
    /// The start shows that the count sees the file before the drop,
    /// not after it.
    pub(crate) const LOOK_BEFORE_DROP: u32 = 1 << 10;
    /// A thread's death is taken for an I/O error of the job it held:
    /// the volume goes on, and the jobs that wait for the threads are
    /// left to them.
    pub(crate) const DEATH_IGNORED: u32 = 1 << 11;
    /// A flush does not wait for the writes on the threads.
    pub(crate) const FLUSH_PASSES_WRITES: u32 = 1 << 12;
    /// A request does not wait for an overlapping one on the threads.
    pub(crate) const OVERLAP_UNORDERED: u32 = 1 << 13;
    /// The start does not sync the home image.
    pub(crate) const START_UNSYNCED: u32 = 1 << 14;
    /// The failure observer is told after the marker is begun, not
    /// before.
    pub(crate) const OBSERVE_AFTER_MARKER: u32 = 1 << 15;
    /// The failure marker is made without the directory's sync, so a
    /// power cut can take it back.
    pub(crate) const MARKER_UNSYNCED: u32 = 1 << 16;
    /// A failed read fails the volume (the log store's rule, which the
    /// in-place store does not have).
    pub(crate) const READ_ERROR_FAILS: u32 = 1 << 17;
    /// Create and open do not hold the file systems' blocks to the
    /// write unit.
    pub(crate) const ANY_BLOCKS: u32 = 1 << 18;
    /// Create and open do not hold the system's page to the write unit.
    pub(crate) const ANY_PAGES: u32 = 1 << 19;
    /// A job pushed to the I/O threads wakes every waiting thread, not
    /// one.
    pub(crate) const POOL_WAKES_ALL: u32 = 1 << 20;
    /// An I/O thread looks at the queue and waits under two holds of
    /// its lock: a job pushed between them wakes nobody.
    pub(crate) const POOL_WAKE_LOST: u32 = 1 << 21;
    /// At the end the I/O threads end without doing the jobs queued.
    pub(crate) const POOL_ENDS_QUEUED: u32 = 1 << 22;
    /// An I/O thread that ends a job waits to be woken before it looks
    /// at the queue again.
    pub(crate) const POOL_WAITS_FIRST: u32 = 1 << 23;
    /// Where a thread cannot be started, the threads started before it
    /// are left waiting.
    pub(crate) const POOL_LEAVES_STARTED: u32 = 1 << 24;
    /// A write handed to the threads is acknowledged at once, before a
    /// thread has written it: a kill can lose it.
    pub(crate) const ACK_BEFORE_WRITE: u32 = 1 << 25;
    /// A failure writes no marker: the next start serves the volume.
    pub(crate) const FAIL_UNMARKED: u32 = 1 << 26;
    /// The start's look reads with the kernel's read-ahead, which the
    /// next start's drop cannot take while it is under I/O.
    pub(crate) const LOOK_READS_AHEAD: u32 = 1 << 27;

    pub(crate) fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Why the second phase of a start ([`Opened::start`]) did not give
/// an in-place store.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The volume has failed: the home image could not be synced. The
    /// failure marker that says so to the next start is still to write:
    /// [`FailedStart`] writes it when it is finished or dropped, so
    /// that the caller can first say what happened elsewhere.
    #[error("the volume has failed")]
    Failed(FailedStart),
    /// The kernel's cache of the home image could not be dropped after
    /// the start's sync, or could not be shown to be gone: a start
    /// serves only what the disk holds (design §4.1), and refuses the
    /// volume. The volume has not failed.
    #[error("the start could not empty the kernel's cache of the home image: {0}")]
    CacheKept(String),
    /// The I/O the store was asked for ([`IoMode`]) cannot be had: no
    /// direct I/O, an alignment it cannot meet, or no threads. The
    /// volume has not failed.
    #[error("the in-place store's I/O: {0}")]
    Io(String),
}

/// How the in-place store reads and writes the home image: by direct
/// I/O (design, decision D6), past the kernel's cache, on I/O threads of
/// its own ([`IoMode::threads`], [`IoMode::DEFAULT_THREADS`] unless
/// asked otherwise). Each thread has a handle of its own on the image
/// and one request at the disk at a time, so that up to that many
/// requests reach the device at once. The start checks the file
/// system's alignment for direct I/O (`STATX_DIOALIGN`) and refuses by
/// reason where it has none or its unit is above a block (4 KiB).
///
/// Test builds can also have the I/O done on the caller's thread, or
/// through the kernel's cache (`IoMode::for_tests`): for a test that
/// needs the cache, or a file system without direct I/O. Neither is a
/// mode the store is served in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoMode {
    /// Direct I/O; without it, through the cache (test builds only).
    direct: bool,
    /// The I/O threads; 0 (test builds only) does the I/O on the
    /// caller's thread.
    threads: usize,
}

impl Default for IoMode {
    fn default() -> Self {
        Self {
            direct: true,
            threads: Self::DEFAULT_THREADS,
        }
    }
}

impl IoMode {
    /// The I/O threads unless asked otherwise: on machines whose syncs
    /// are slow, eight measured above one in every job that ran both
    /// (design §9). That rests on slow-sync machines only, and the
    /// default may change after a benchmark round on the merged daemon:
    /// this is the one constant to change (the daemon's `--io-threads`
    /// takes its default from it).
    pub const DEFAULT_THREADS: usize = 8;
    /// The most I/O threads the store takes.
    pub const MAX_THREADS: usize = 64;

    /// Direct I/O on `threads` I/O threads, 1 to [`IoMode::MAX_THREADS`];
    /// `None` outside that range.
    pub fn with_threads(threads: usize) -> Option<Self> {
        (1..=Self::MAX_THREADS).contains(&threads).then_some(Self {
            direct: true,
            threads,
        })
    }

    /// The I/O threads.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Whether the I/O is direct: always, outside test builds.
    pub fn direct(&self) -> bool {
        self.direct
    }

    /// For tests: through the kernel's cache unless `direct`, and on the
    /// caller's thread where `threads` is 0.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn for_tests(direct: bool, threads: usize) -> Self {
        Self { direct, threads }
    }
}

/// The store on the home image alone: see the
/// [module documentation](self).
pub struct InPlace<D: Dir> {
    /// The volume's directory: where the I/O threads open their handles
    /// on the home image.
    dir: D,
    /// Who is told of the failure first; taken when it is told.
    observer: Option<FailureObserver>,
    home: D::File,
    lock: Option<D::Lock>,
    geometry: Geometry,
    identity: [u8; 16],
    failed: bool,
    /// Submitted and not yet handed out.
    ids: BTreeSet<RequestId>,
    ready: Vec<Completion>,
    /// A mutation was executed since the sync under way began, or,
    /// with none under way, since the last sync.
    unsynced: bool,
    /// The flushes the sync under way covers, if one is.
    syncing: Option<Vec<RequestId>>,
    /// Flushes received while a sync was under way that it does not
    /// cover: the next sync does.
    waiting: Vec<RequestId>,
    /// Called when a sync begun on the file's helper ends.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    zeroes: Zeroes,
    counts: Counts,
    mutants: Mutants,
    /// How reads and writes are done.
    io: IoMode,
    /// The threads, where [`IoMode::threads`] asks for some.
    pool: Option<Pool>,
    /// The requests on the threads: their byte ranges, and whether each
    /// is a write.
    busy: BTreeMap<RequestId, (u64, u64, bool)>,
    /// Requests that wait, in their order, for one on the threads that
    /// they must not run beside ([`InPlace::must_wait`]). Everything
    /// received after one of them waits behind it.
    deferred: VecDeque<Request>,
    /// The queue's counts and highs; the current figures are those of
    /// `busy` and `deferred`.
    queue: IoQueue,
    /// Writes on the threads acknowledged before a thread wrote them,
    /// under ids of their own on the threads (the client may use theirs
    /// again at once), and how many there were: only under the defect
    /// [`Mutants::ACK_BEFORE_WRITE`]. Such a write's end completes
    /// nothing.
    acked_early: (BTreeSet<RequestId>, u64),
    /// The failure marker: not yet needed, being written on a thread
    /// of its own, or tried. Whether it was written is not kept: a start
    /// that does not find it goes by the daemon's failure note.
    marker: Marker,
}

/// Writes the failure marker, on whichever thread runs it.
type MarkerWriter = Box<dyn FnOnce() -> bool + Send>;

/// The failure marker's writer. A thread of its own is started for it
/// when the store is assembled, before the first request, and waits:
/// at the failure it is only told to write. Starting a thread at the
/// moment of failure could wait for memory on the thread that must go
/// on answering (the daemon locks its memory, and a failed disk often
/// leaves the cache full of pages nobody can clean), or fail and leave
/// the write to that thread. Where no thread can be started at
/// assembly, the marker is written on the caller's thread when the
/// volume fails.
enum Marker {
    /// The thread waits for the word to write (`go`); `go` dropped, it
    /// ends without writing.
    Parked {
        go: std::sync::mpsc::Sender<()>,
        thread: std::thread::JoinHandle<bool>,
    },
    /// No thread: written on the caller's thread when needed.
    Inline(MarkerWriter),
    /// The thread was told to write it.
    Writing(std::thread::JoinHandle<bool>),
    Done,
}

impl Marker {
    fn new(write: MarkerWriter, mutants: Mutants) -> Self {
        if mutants.has(Mutants::MARKER_INLINE) {
            return Marker::Inline(write);
        }
        // The writer goes to the thread through a slot, so that it is
        // still here when the thread cannot be started.
        let slot = Arc::new(std::sync::Mutex::new(Some(write)));
        let theirs = slot.clone();
        let (go, word) = std::sync::mpsc::channel::<()>();
        // A small stack: the daemon locks its memory, so a thread's stack
        // stays resident for its life, and the marker's writer needs
        // little (a create, a sync, a sync of the directory).
        let started = std::thread::Builder::new()
            .name("skyzfs-marker".into())
            .stack_size(MARKER_STACK)
            .spawn(move || {
                if word.recv().is_err() {
                    return false;
                }
                let write = theirs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                write.is_some_and(|write| write())
            });
        match started {
            Ok(thread) => Marker::Parked { go, thread },
            Err(_) => {
                let write = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                match write {
                    Some(write) => Marker::Inline(write),
                    None => Marker::Done,
                }
            }
        }
    }

    /// Begin to write the marker: on its thread, which is told to, or
    /// here where there is none.
    fn begin(&mut self) {
        match std::mem::replace(self, Marker::Done) {
            Marker::Parked { go, thread } => {
                // The thread only ends after the word or without `go`,
                // so it is there to hear it.
                let _ = go.send(());
                *self = Marker::Writing(thread);
            }
            Marker::Inline(write) => {
                write();
            }
            other => *self = other,
        }
    }

    /// Wait for the marker's write where it was begun, and end the
    /// thread without writing where it was not.
    fn finish(&mut self) {
        match std::mem::replace(self, Marker::Done) {
            Marker::Parked { go, thread } => {
                drop(go);
                let _ = thread.join();
            }
            Marker::Writing(thread) => {
                let _ = thread.join();
            }
            Marker::Inline(_) | Marker::Done => {}
        }
    }
}

/// The marker thread's stack, in bytes.
const MARKER_STACK: usize = 256 << 10;

/// A start of the in-place store that met the failed disk
/// ([`StartError::Failed`]): it holds the volume's lock and the
/// failure marker still to write. The marker needs the disk that has
/// just failed and may take minutes or never end, so the caller first
/// says what happened (the daemon's failure note, the delete of its
/// device), then [`FailedStart::finish`]es it. Dropped, it writes the
/// marker as well. The lock is released after the marker, so that no
/// other start can open the volume between the two.
pub struct FailedStart {
    marker: Marker,
    lock: Option<Box<dyn Send>>,
}

impl FailedStart {
    /// Write the failure marker, wait for it, and release the lock.
    pub fn finish(mut self) {
        self.write();
    }

    fn write(&mut self) {
        self.marker.begin();
        self.marker.finish();
        self.lock = None;
    }
}

impl Drop for FailedStart {
    fn drop(&mut self) {
        self.write();
    }
}

impl std::fmt::Debug for FailedStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FailedStart")
    }
}

/// The I/O threads' queue, as the status file shows it
/// ([`InPlace::io_queue`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoQueue {
    /// The I/O threads; 0 where the I/O is done on the caller's thread
    /// (test builds only).
    pub threads: u64,
    /// Reads and writes on the threads now: handed to a thread, and not
    /// yet taken back by the store.
    pub in_flight: u64,
    /// Requests that wait now, in their order, for one on the threads
    /// that they must not run beside (an overlapping request, or for a
    /// flush a write), and every
    /// request received after the first of them.
    pub waiting: u64,
    /// The requests that waited so since the store began: a count that
    /// only grows.
    pub waited: u64,
    /// The most requests on the threads at once since the store began.
    pub in_flight_max: u64,
    /// The most requests that waited at once since the store began.
    pub waiting_max: u64,
}

/// What the store has done, for its owner and for tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    /// Syncs of the home image begun for flushes, and the one of the
    /// start.
    pub syncs: u64,
    /// Flushes that joined a sync already under way.
    pub joined: u64,
    /// Flushes completed with nothing to make durable.
    pub clean: u64,
}

impl<D: Dir> std::fmt::Debug for InPlace<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InPlace")
            .field("geometry", &self.geometry)
            .field("failed", &self.failed)
            .field("in_flight", &self.ids.len())
            .field("counts", &self.counts)
            .finish_non_exhaustive()
    }
}

/// The store waits for its failure marker when it is dropped, before
/// the lock is released, so that a start after it finds the marker (or
/// the failure to write it); a marker never asked for ends its thread.
impl<D: Dir> InPlace<D> {
    /// What a kill of the process does to the store, for the tests: the
    /// requests no thread has begun, and those that wait in the store,
    /// are dropped undone; the jobs under way on the threads end as the
    /// kernel ends them, and their results are not taken; nothing
    /// completes, nothing fails, and no marker is written. A drop,
    /// instead, does everything submitted before the store goes.
    #[cfg(test)]
    pub(crate) fn abandon(mut self) {
        if let Some(pool) = self.pool.take() {
            drop(pool.take_back());
        }
        self.busy.clear();
        self.deferred.clear();
    }
}

impl<D: Dir> Drop for InPlace<D> {
    fn drop(&mut self) {
        // What was submitted is done before the store goes, as it is on
        // the caller's thread: what waits is executed once what it waits
        // for has ended. Then the threads end, before the lock is gone.
        while self.pool.is_some() && !(self.busy.is_empty() && self.deferred.is_empty()) {
            self.collect(Duration::from_millis(10));
        }
        self.pool = None;
        self.marker.finish();
    }
}

/// The parts an in-place store is made of ([`Opened::start`]).
pub(crate) struct Parts<D: Dir> {
    pub(crate) dir: D,
    pub(crate) home: D::File,
    pub(crate) lock: Option<D::Lock>,
    pub(crate) geometry: Geometry,
    pub(crate) identity: [u8; 16],
    pub(crate) zeroes: Zeroes,
    /// What writes the failure marker.
    pub(crate) marker: MarkerWriter,
    /// Who is told of the failure first ([`OpenOptions::on_failure`]).
    pub(crate) observer: Option<FailureObserver>,
    /// How reads and writes are done.
    pub(crate) io: IoMode,
}

impl<D: Dir> InPlace<D> {
    /// From the parts of an opened volume ([`Opened::start`]): sync the
    /// home image, so that what this start exposes is durable before
    /// the first request, drop the kernel's cache of it, and start the
    /// I/O threads. Where the sync fails, the volume has failed: the
    /// observer is told, and the marker is handed back unwritten with
    /// the lock ([`FailedStart`]).
    pub(crate) fn assemble(
        parts: Parts<D>,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        mutants: Mutants,
    ) -> Result<Self, StartError>
    where
        D::Lock: Send + 'static,
        D::File: Send + 'static,
    {
        let io = parts.io;
        let mut store = Self {
            dir: parts.dir,
            observer: parts.observer,
            home: parts.home,
            lock: parts.lock,
            geometry: parts.geometry,
            identity: parts.identity,
            failed: false,
            ids: BTreeSet::new(),
            ready: Vec::new(),
            unsynced: false,
            syncing: None,
            waiting: Vec::new(),
            wake,
            zeroes: parts.zeroes,
            counts: Counts::default(),
            mutants,
            io,
            pool: None,
            busy: BTreeMap::new(),
            deferred: VecDeque::new(),
            queue: IoQueue::default(),
            acked_early: (BTreeSet::new(), 0),
            marker: Marker::new(parts.marker, mutants),
        };
        // What the instance before this one wrote by direct I/O may be
        // in the device's volatile cache only: this sync makes it
        // durable before this start serves it (point 3 of the module
        // documentation), and costs nothing when nothing is dirty.
        store.counts.syncs += 1;
        if !store.mutants.has(Mutants::START_UNSYNCED) && store.home.sync().is_err() {
            store.failed = true;
            if let Some(observer) = store.observer.take() {
                observer.call();
            }
            let marker = std::mem::replace(&mut store.marker, Marker::Done);
            let lock = store
                .lock
                .take()
                .map(|lock| Box::new(lock) as Box<dyn Send>);
            return Err(StartError::Failed(FailedStart { marker, lock }));
        }
        store.empty_cache().map_err(StartError::CacheKept)?;
        store.set_up_io().map_err(StartError::Io)?;
        Ok(store)
    }

    /// Check what direct I/O asks, and start the threads.
    ///
    /// Direct I/O takes the request's own buffer, which is aligned to a
    /// block ([`Buf::ALIGN`]), at the request's offset, which is a whole
    /// number of blocks: the file system's alignment must divide both.
    fn set_up_io(&mut self) -> Result<(), String>
    where
        D::File: Send + 'static,
    {
        if self.io.direct {
            let align = self
                .home
                .direct_align()
                .map_err(|e| format!("the file system offers no direct I/O on it: {e}"))?;
            let block = skyzfs_blk::BLOCK_SIZE;
            let (memory, offset) = (u64::from(align.memory), u64::from(align.offset));
            if offset == 0
                || !block.is_multiple_of(offset)
                || (memory > 0 && !(Buf::ALIGN as u64).is_multiple_of(memory))
            {
                return Err(format!(
                    "direct I/O asks an alignment of {offset} bytes of offsets and {memory} of \
                     memory, which a block of {block} bytes does not meet"
                ));
            }
        }
        if self.io.threads > 0 {
            let files = (0..self.io.threads)
                .map(|_| self.dir.open(crate::store::HOME_NAME))
                .collect::<io::Result<Vec<_>>>()
                .map_err(|e| format!("opening the home image for the threads: {e}"))?;
            let pool = Pool::start(files, self.io.direct, self.wake.clone(), self.mutants)
                .map_err(|e| format!("starting a thread: {e}"))?;
            self.pool = Some(pool);
        }
        Ok(())
    }

    /// Drop the kernel's cache of the home image, and check that it is
    /// gone: from here on every read comes from the disk. A failed sync
    /// can leave pages marked clean that the disk never got, and no
    /// later sync writes them; a start that served them would show
    /// what a cut can still take, behind a flush that does not cover it
    /// (design §4.1, "every start serves only what the disk holds").
    /// That holds whether this start acknowledges a failure or follows
    /// an instance that died between a failed sync and its marker,
    /// whose own sync then succeeds: the kernel reports a writeback
    /// error once. The drop is advisory, so it is tried twice. Once the
    /// count reads 0, it is shown to see this file at all
    /// ([`InPlace::count_sees_the_file`]). Pages that stay, a drop that
    /// fails, a count that does not see the file, or a kernel that
    /// cannot count the file's cached pages refuse the start. Its cost:
    /// the first reads after a start, a take-over included, come from
    /// the disk.
    ///
    /// Under direct I/O the store's own reads and writes leave no page
    /// in the cache: what the drop still guards is what a writer through
    /// the cache left before this start: a file system that serves
    /// direct requests through its cache (see the module documentation,
    /// point 3).
    fn empty_cache(&mut self) -> Result<(), String> {
        let look = !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED);
        let look_first = self.mutants.has(Mutants::LOOK_BEFORE_DROP);
        if look && look_first {
            self.count_sees_the_file()?;
        }
        let mut left = 0;
        for _ in 0..2 {
            if !self.mutants.has(Mutants::CACHE_NOT_DROPPED) {
                self.home
                    .drop_cache()
                    .map_err(|e| format!("the drop failed: {e}"))?;
            }
            left = if self.mutants.has(Mutants::CACHE_NOT_CHECKED) {
                0
            } else {
                self.cached_pages()?
            };
            if left == 0 {
                break;
            }
        }
        if left > 0 {
            return Err(format!(
                "{left} pages of it stay in the cache after two drops; something else may \
                 map, lock or read the file"
            ));
        }
        if look && !look_first {
            self.count_sees_the_file()?;
        }
        Ok(())
    }

    /// Show that the count, which has just read 0, sees this file's
    /// pages at all: a block read through the cache must count. On an
    /// overlay file system `cachestat` counts the overlay file's own
    /// mapping, which holds no page (they are the file's below), and
    /// would read 0 whatever the cache holds; a file system without a
    /// page cache would read 0 too, and is refused the same way.
    ///
    /// The look reads block 0, which is a hole on every pool's volume
    /// (the first 8 KiB of the label are never written), so it rests on
    /// a read of a hole leaving a page in the cache. ext4 and XFS leave
    /// one (the rigs' first start of a pool is on an image of holes
    /// only); tmpfs leaves none, and a new volume there is refused here,
    /// as it would be by the drop once it holds data.
    ///
    /// The look comes after the drop, and what it reads comes from the
    /// disk and may stay. Before the drop it made starts refuse: the
    /// read starts the kernel's read-ahead of the blocks after it, and
    /// where the block read is a hole (as the first 8 KiB of a pool's
    /// label are) it is answered at once, while the read-ahead is still
    /// under I/O, and a drop leaves pages under I/O. In the review's
    /// run, 731 of 1500 starts refused on such an image
    /// (`every_start_serves_where_the_image_begins_with_a_hole` in
    /// `tests/inplace_real.rs`).
    ///
    /// After the drop it still left the read-ahead under I/O for the
    /// next start: a start that came right after one (as that test's
    /// do, or a daemon's quick restart) found the pages under I/O and
    /// refused, start 4 of that test's 200 in one CI run. So the look
    /// reads its block alone ([`File::read_alone_at`], with read-ahead
    /// off for the file on Linux), and leaves nothing behind but that
    /// block's page, which the next drop takes
    /// (`a_start_right_after_a_start_serves`).
    fn count_sees_the_file(&mut self) -> Result<(), String> {
        let mut block = vec![0u8; skyzfs_blk::BLOCK_USIZE];
        let read = if self.mutants.has(Mutants::LOOK_READS_AHEAD) {
            self.home.read_at(0, &mut block)
        } else {
            self.home.read_alone_at(0, &mut block)
        };
        read.map_err(|e| format!("reading a block of it failed: {e}"))?;
        if self.cached_pages()? == 0 {
            return Err(
                "the file system's cache cannot be observed through this file: a block \
                 just read through the cache counts 0 pages, so the start cannot show that \
                 the drop left none (through an overlay file system, for example, the count \
                 sees no page)"
                    .into(),
            );
        }
        Ok(())
    }

    /// The home image's pages in the kernel's cache.
    fn cached_pages(&self) -> Result<u64, String> {
        match self.home.cached_pages() {
            Ok(pages) => Ok(pages),
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Err(
                "the kernel cannot count a file's cached pages without mapping it \
                     (cachestat, Linux 6.5 and later)"
                    .into(),
            ),
            Err(e) => Err(format!("counting its cached pages failed: {e}")),
        }
    }

    /// The volume's identity ([`crate::volume::Volume::identity`]).
    pub fn identity(&self) -> [u8; 16] {
        self.identity
    }

    /// Whether the volume has failed.
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// What the store has done so far.
    pub fn counts(&self) -> Counts {
        self.counts
    }

    /// How write-zeroes reach the home image: written as zeros by direct
    /// I/O ([`Zeroes::WrittenDirect`]) once one has been applied, and
    /// [`Zeroes::Untried`] until then. Through the cache (test builds
    /// only): deallocated, or written as zeros where the file system
    /// cannot deallocate.
    pub fn zeroes(&self) -> Zeroes {
        self.zeroes
    }

    /// Whether a sync of the home image is under way.
    pub fn syncing(&self) -> bool {
        self.syncing.is_some()
    }

    /// Requests submitted and not yet handed out.
    pub fn in_flight(&self) -> usize {
        self.ids.len()
    }

    /// The I/O threads' queue: what is on them, what waits for them,
    /// and the counts since the store began.
    pub fn io_queue(&self) -> IoQueue {
        IoQueue {
            threads: self.io.threads as u64,
            in_flight: self.busy.len() as u64,
            waiting: self.deferred.len() as u64,
            ..self.queue
        }
    }

    /// For test rigs: fail the volume now, as if the next sync had
    /// failed. Every later request fails, flushes under way and waiting
    /// included, and a later start finds the volume marked as failed.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn fail_now(&mut self) {
        if !self.failed {
            self.fail();
            self.settle_failed();
        }
    }

    /// The volume fails: the observer hears of it at once, and the
    /// marker's thread is told to write the marker for the next start.
    /// The marker needs the disk that has just failed, and a sync on a
    /// sick disk can take minutes or never return; the caller is the
    /// queue's thread, which must go on answering what it holds with
    /// the failure. Where no thread could be started, it is written
    /// here.
    fn fail(&mut self) {
        self.failed = true;
        // Once: the observer is taken when it is told, and a marker
        // begun or written is not begun again.
        if self.mutants.has(Mutants::OBSERVE_AFTER_MARKER) {
            self.marker.begin();
        }
        if let Some(observer) = self.observer.take() {
            observer.call();
        }
        if !self.mutants.has(Mutants::FAIL_UNMARKED) {
            self.marker.begin();
        }
    }

    fn complete(&mut self, id: RequestId, result: Result<(), BlkError>, buf: Option<Buf>) {
        self.ready.push(Completion { id, result, buf });
    }

    /// Every flush under way or waiting fails with the volume.
    fn settle_failed(&mut self) {
        let mut flushes = self.syncing.take().unwrap_or_default();
        flushes.append(&mut self.waiting);
        for id in flushes {
            self.complete(id, Err(BlkError::Failed), None);
        }
    }

    fn execute(&mut self, id: RequestId, op: Op) {
        if let Err(rejected) = self.geometry.validate(&op) {
            let buf = buffer_of(op);
            return self.complete(id, Err(rejected), buf);
        }
        if self.failed {
            let buf = buffer_of(op);
            return self.complete(id, Err(BlkError::Failed), buf);
        }
        match op {
            Op::Read { offset, mut buf } => {
                if self.pool.is_some() {
                    let end = offset + buf.len() as u64;
                    return self.on_thread(id, (offset, end, false), Job::Read { id, offset, buf });
                }
                // A failed read changes nothing and does not fail the
                // volume: only its own request fails.
                let read = if self.io.direct {
                    self.home.read_direct_at(offset, &mut buf)
                } else {
                    self.home.read_at(offset, &mut buf)
                };
                let result = match read {
                    Err(_) if self.mutants.has(Mutants::READ_ERROR_FAILS) => {
                        self.fail();
                        self.settle_failed();
                        Err(BlkError::Failed)
                    }
                    read => read.map_err(|e| BlkError::Io(e.kind())),
                };
                self.complete(id, result, Some(buf));
            }
            Op::Flush => self.flush(id),
            op if self.geometry.read_only() => {
                let buf = buffer_of(op);
                self.complete(id, Err(BlkError::ReadOnly), buf);
            }
            Op::Write { offset, data } => {
                if self.pool.is_some() {
                    let end = offset + data.len() as u64;
                    return self.on_thread(
                        id,
                        (offset, end, true),
                        Job::Write { id, offset, data },
                    );
                }
                let written = if self.io.direct {
                    self.home.write_direct_at(offset, &data)
                } else {
                    self.home.write_at(offset, &data)
                };
                let result = self.mutated(written);
                self.complete(id, result, Some(data));
            }
            Op::WriteZeroes { offset, len } => {
                let zeroed = self.zero(offset, len);
                let result = self.mutated(zeroed);
                self.complete(id, result, None);
            }
            // Rule R6 lets a discarded block keep its content. The
            // discard still counts as a change for the flush behind
            // it: a flush after a client's change always syncs, so it
            // fails on a failed disk whatever the change was (rule R11).
            Op::Discard { .. } => {
                if !self.mutants.has(Mutants::DISCARD_UNCHANGED) {
                    self.unsynced = true;
                }
                self.complete(id, Ok(()), None)
            }
        }
    }

    /// Hand `job` to the threads; `range` is its bytes and whether it
    /// writes.
    fn on_thread(&mut self, id: RequestId, range: (u64, u64, bool), job: Job) {
        let (key, job) = match job {
            Job::Write { offset, data, .. } if self.mutants.has(Mutants::ACK_BEFORE_WRITE) => {
                let (early, count) = &mut self.acked_early;
                *count += 1;
                let key = RequestId(u64::MAX - *count);
                early.insert(key);
                self.complete(id, Ok(()), Some(data.clone()));
                (
                    key,
                    Job::Write {
                        id: key,
                        offset,
                        data,
                    },
                )
            }
            job => (id, job),
        };
        self.busy.insert(key, range);
        self.queue.in_flight_max = self.queue.in_flight_max.max(self.busy.len() as u64);
        if let Some(pool) = &self.pool {
            pool.push(job);
        }
    }

    /// A mutation was tried: the volume fails with it if it failed.
    fn mutated(&mut self, done: io::Result<()>) -> Result<(), BlkError> {
        self.unsynced = true;
        match done {
            Ok(()) => Ok(()),
            Err(e) if self.mutants.has(Mutants::WRITE_ERROR_PASSES) => Err(BlkError::Io(e.kind())),
            Err(_) => {
                self.fail();
                self.settle_failed();
                Err(BlkError::Failed)
            }
        }
    }

    /// Zero a range: deallocate it, or write zeros where the file
    /// system has no such call (from then on, without asking again).
    ///
    /// With direct I/O, zeros are written by direct I/O: a deallocated
    /// range would make the next writes to it allocate again. The state
    /// the status shows ([`InPlace::zeroes`]) says so from the first.
    fn zero(&mut self, offset: u64, len: u64) -> io::Result<()> {
        if self.io.direct {
            const CHUNK: u64 = 1 << 20;
            let zeros = Buf::filled((len.min(CHUNK) / skyzfs_blk::BLOCK_SIZE) as usize, 0);
            let mut at = 0;
            while at < len {
                let n = (len - at).min(CHUNK) as usize;
                self.home.write_direct_at(offset + at, &zeros[..n])?;
                at += n as u64;
            }
            self.zeroes = Zeroes::WrittenDirect;
            return Ok(());
        }
        if self.zeroes != Zeroes::Written {
            match self.home.deallocate(offset, len) {
                Ok(()) => {
                    self.zeroes = Zeroes::Deallocated;
                    return Ok(());
                }
                Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                    self.zeroes = Zeroes::Written;
                }
                Err(e) => return Err(e),
            }
        }
        crate::store::write_zeros(&mut self.home, offset, len)
    }

    fn flush(&mut self, id: RequestId) {
        match &mut self.syncing {
            Some(covered) => {
                // A sync under way covers what was written before it
                // began: everything this flush must cover, if nothing
                // was written since.
                if !self.unsynced || self.mutants.has(Mutants::FLUSH_JOINS_ANY_SYNC) {
                    covered.push(id);
                    self.counts.joined += 1;
                } else {
                    self.waiting.push(id);
                }
            }
            None if !self.unsynced || self.mutants.has(Mutants::FLUSH_WITHOUT_SYNC) => {
                // Nothing written since the last sync, which succeeded:
                // a failed one has failed the volume.
                self.counts.clean += 1;
                self.complete(id, Ok(()), None);
            }
            None => self.begin_sync(vec![id]),
        }
    }

    /// Begin a sync that covers `flushes`, on the file's helper. Where
    /// the file cannot begin one, the sync is made here, waiting.
    fn begin_sync(&mut self, flushes: Vec<RequestId>) {
        self.unsynced = false;
        self.counts.syncs += 1;
        let wake = self.wake.clone();
        let begun = self.home.sync_begin(Box::new(move || {
            if let Some(wake) = wake {
                wake();
            }
        }));
        match begun {
            Ok(()) => self.syncing = Some(flushes),
            Err(e) if NotBegun::is(&e) => {
                self.syncing = Some(flushes);
                let synced = self.home.sync();
                self.synced(synced);
            }
            Err(_) => {
                self.syncing = Some(flushes);
                self.synced(Err(io::ErrorKind::Other.into()));
            }
        }
    }

    /// The sync under way has ended with `result`.
    fn synced(&mut self, result: io::Result<()>) {
        match result {
            Ok(()) => {
                for id in self.syncing.take().unwrap_or_default() {
                    self.complete(id, Ok(()), None);
                }
                if self.waiting.is_empty() {
                    return;
                }
                let waiting = std::mem::take(&mut self.waiting);
                if self.unsynced {
                    self.begin_sync(waiting);
                } else {
                    for id in waiting {
                        self.complete(id, Ok(()), None);
                    }
                }
            }
            Err(e) if self.mutants.has(Mutants::SYNC_RETRIED) => {
                // The defect: the flushes fail and nothing else does.
                let mut flushes = self.syncing.take().unwrap_or_default();
                flushes.append(&mut self.waiting);
                for id in flushes {
                    self.complete(id, Err(BlkError::Io(e.kind())), None);
                }
                self.unsynced = true;
            }
            Err(_) => {
                self.fail();
                self.settle_failed();
            }
        }
    }

    /// Whether `op` must wait for a request on the threads: a read for
    /// a write it overlaps, a write or a write-zeroes for a read or a
    /// write it overlaps (so that overlapping requests keep the order
    /// they came in), and a flush for every write (a flush covers what
    /// was acknowledged before it; this one also waits for what was
    /// submitted before it, the stricter reading).
    fn must_wait(&self, op: &Op) -> bool {
        if self.busy.is_empty() {
            return false;
        }
        let (offset, len, write) = match op {
            Op::Read { offset, buf } => (*offset, buf.len() as u64, false),
            Op::Write { offset, data } => (*offset, data.len() as u64, true),
            Op::WriteZeroes { offset, len } => (*offset, *len, true),
            Op::Flush if self.mutants.has(Mutants::FLUSH_PASSES_WRITES) => return false,
            Op::Flush => return self.busy.values().any(|&(_, _, write)| write),
            Op::Discard { .. } => return false,
        };
        if self.mutants.has(Mutants::OVERLAP_UNORDERED) {
            return false;
        }
        let end = offset.saturating_add(len);
        self.busy
            .values()
            .any(|&(from, to, busy_write)| (write || busy_write) && from < end && offset < to)
    }

    /// Execute `request` now, or let it wait behind what waits already
    /// or behind a request on the threads it must not run beside.
    fn accept(&mut self, request: Request) {
        if !self.deferred.is_empty() || self.must_wait(&request.op) {
            self.deferred.push_back(request);
            self.queue.waited += 1;
            self.queue.waiting_max = self.queue.waiting_max.max(self.deferred.len() as u64);
        } else {
            self.execute(request.id, request.op);
        }
    }

    /// Take what the threads did (with `timeout`, wait up to it for
    /// something), then execute what no longer has to wait.
    ///
    /// A write's end is a mutation's, as on the caller's thread: it
    /// counts for the next flush, and a failed one fails the volume. A
    /// request that ends after the volume failed fails with it, which
    /// rule R11 allows of one in flight at the failure.
    ///
    /// A thread that died (of a panic) fails the volume as a failed
    /// write does, whatever its job was: what it did to the image is not
    /// known, and fewer threads are left, perhaps none. Once the volume
    /// has failed, the jobs no thread has begun are taken back and fail,
    /// so that none waits for a thread that is gone; the jobs under way
    /// on the threads still alive come back as they end, and fail.
    fn collect(&mut self, timeout: Duration) {
        let Some(pool) = &self.pool else {
            return;
        };
        let mut done = Vec::new();
        pool.take(&mut done, timeout);
        let died = pool.died() > 0 && !self.mutants.has(Mutants::DEATH_IGNORED);
        if died && !self.failed {
            self.fail();
            self.settle_failed();
        }
        if self.failed {
            for job in self.pool.as_ref().map(Pool::take_back).unwrap_or_default() {
                let (id, _, buf) = job.into_parts();
                self.busy.remove(&id);
                self.complete(id, Err(BlkError::Failed), Some(buf));
            }
        }
        for Done {
            id,
            write,
            result,
            buf,
        } in done
        {
            self.busy.remove(&id);
            if self.acked_early.0.remove(&id) {
                // Acknowledged already, under the defect.
                let _ = self.mutated(result);
                continue;
            }
            let result = if write {
                let result = self.mutated(result);
                if self.failed {
                    Err(BlkError::Failed)
                } else {
                    result
                }
            } else if self.failed {
                Err(BlkError::Failed)
            } else if result.is_err() && self.mutants.has(Mutants::READ_ERROR_FAILS) {
                self.fail();
                self.settle_failed();
                Err(BlkError::Failed)
            } else {
                result.map_err(|e| BlkError::Io(e.kind()))
            };
            self.complete(id, result, Some(buf));
        }
        while let Some(front) = self.deferred.front() {
            if self.must_wait(&front.op) {
                break;
            }
            let Some(Request { id, op }) = self.deferred.pop_front() else {
                break;
            };
            self.execute(id, op);
        }
    }

    /// See whether the sync under way has ended; with `wait`, wait for
    /// it.
    fn settle(&mut self, wait: bool) {
        if self.syncing.is_none() {
            return;
        }
        if let Some(result) = self.home.sync_end(wait) {
            self.synced(result);
        }
    }

    fn hand_out(&mut self, out: &mut Vec<Completion>) -> usize {
        let n = self.ready.len();
        for completion in &self.ready {
            self.ids.remove(&completion.id);
        }
        out.append(&mut self.ready);
        n
    }
}

fn buffer_of(op: Op) -> Option<Buf> {
    match op {
        Op::Read { buf, .. } => Some(buf),
        Op::Write { data, .. } => Some(data),
        _ => None,
    }
}

impl<D: Dir> BlockTarget for InPlace<D> {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// Executes the request at once, on the caller's thread: a read or
    /// a mutation is file I/O, a flush begins a sync and returns. The
    /// completion is handed out by the next [`BlockTarget::poll`].
    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        let kind = if self.ids.contains(&request.id) {
            Some(SubmitErrorKind::DuplicateId)
        } else if self.ids.len() >= self.geometry.queue_depth() as usize {
            Some(SubmitErrorKind::Busy)
        } else {
            None
        };
        if let Some(kind) = kind {
            return Err(SubmitError { kind, request });
        }
        self.ids.insert(request.id);
        self.accept(request);
        Ok(())
    }

    /// Hands out what is complete. Looks whether the sync under way has
    /// ended, without waiting for it.
    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.collect(Duration::ZERO);
        self.settle(false);
        self.hand_out(out)
    }

    /// As [`BlockTarget::poll`] with a zero `timeout`. Otherwise, with
    /// nothing complete and a sync under way, it waits for that sync to
    /// end, which can take longer than `timeout`: the file has no wait
    /// with a bound ([`File::sync_end`]). Everything else completes in
    /// [`BlockTarget::submit`], so a sync is all there is to wait for.
    /// The daemon's pump calls only `poll`, and is woken by the file's
    /// helper when a sync ends.
    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        self.collect(Duration::ZERO);
        self.settle(false);
        if self.ready.is_empty() && !timeout.is_zero() {
            if self.busy.is_empty() {
                self.settle(true);
            } else {
                self.collect(timeout);
            }
            // Every end of a job completes something, and a wait ends
            // with it, but under the defect, where a write's end
            // completes nothing: wait on, as for a request behind it.
            while self.mutants.has(Mutants::ACK_BEFORE_WRITE) && self.ready.is_empty() {
                if self.busy.is_empty() {
                    self.settle(true);
                    break;
                }
                self.collect(timeout);
            }
        }
        self.hand_out(out)
    }
}

/// The volatile-cache disk's point 3, as a check on what one block
/// reads after a power cut: see the [module documentation](self).
/// `durable` is what the block held at its last flush (or the start's
/// sync), `since` every version written to it after that, in order.
/// With `garbage`, a sector that a power cut could take (the block has
/// versions since) may hold anything. Returns the first sector that
/// none of them explains.
pub fn volatile_cache_explains(
    read: &[u8],
    durable: &[u8],
    since: &[Vec<u8>],
    garbage: bool,
) -> Result<(), usize> {
    const SECTOR: usize = crate::io::SECTOR as usize;
    for (index, sector) in read.chunks(SECTOR).enumerate() {
        let at = index * SECTOR..(index + 1) * SECTOR;
        if durable.get(at.clone()) == Some(sector) {
            continue;
        }
        if since
            .iter()
            .any(|version| version.get(at.clone()) == Some(sector))
        {
            continue;
        }
        if garbage && !since.is_empty() {
            continue;
        }
        return Err(index);
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod harness;
mod pool;
mod start;
#[cfg(test)]
mod tests;

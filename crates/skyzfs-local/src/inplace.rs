//! The in-place store: the volume's home image, written in place and
//! synced at a flush, with no write log (work package M1.3r, an
//! experiment: is the log needed?).
//!
//! It is entered from an opened log store ([`Store::into_in_place`](crate::store::Store::into_in_place)):
//! the log store's open recovers the log as always, the whole log is
//! applied to the home image, the home image is synced, and from then
//! on the log is empty and is never written. The volume's files stay
//! those of the log store. **A volume served in place must not be given
//! back to the log store**: the log store's open does not sync the home
//! image, and its flushes sync the log alone, so a write this store
//! acknowledged and no flush covered before its process ended (lost or
//! stopped: nothing syncs the image at a stop) is
//! covered by no flush of the log store after it, and a power cut can
//! take it (rule R7). That is owed (the review of M1.3r, finding A;
//! the ignored test
//! `a_flush_on_the_log_store_covers_what_the_in_place_store_wrote`
//! reproduces it). What the log store keeps for the daemon (the lock, the
//! identity, the failure marker and the observer that hears of a
//! failure first) is the log's and stays with it here.
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
//! 1. **Process loss.** A write, write-zeroes or discard is
//!    acknowledged once it is in the file system's cache. It is there
//!    after the process is lost, and so is everything acknowledged
//!    before it (rules R9 and R8). This rests on the page cache: it
//!    holds while the machine runs. A write in flight when the process
//!    dies may be there in part, block by block; the kernel issues it
//!    again (rule R9), and the next instance makes it whole.
//! 2. **A flush** completes only when every operation acknowledged
//!    before it was received is durable (rule R7).
//! 3. **Power loss.** Every block that a completed flush covered holds
//!    what it held at that flush, or what was written to it after. Of
//!    a block written since its last flush, each sector holds the
//!    block's durable content or one of the versions written since,
//!    independently of every other sector and of every other block: a
//!    later write can survive while an earlier one is lost, and a
//!    write can be torn at a sector. A start makes what it finds
//!    durable before it serves ([`Store::into_in_place`](crate::store::Store::into_in_place)),
//!    and then serves only what the disk holds: it drops the kernel's
//!    cache of the home image and checks that no page of it is left
//!    (`cachestat`), or refuses the volume (design §4.1). A failed sync
//!    can leave pages marked clean that the disk never got, and no
//!    later sync writes them; served from the cache, they would be
//!    shown and then taken by a cut behind a completed flush. The drop
//!    covers a start that acknowledges a failure as well as one after
//!    an instance that died between a failed sync and its marker, whose
//!    own sync succeeds because the kernel reports such an error once.
//!    Its cost: the first reads after every start, a take-over
//!    included, come from the disk.
//! 4. **Failure is final** (rule R11's failure rule). Once a write or
//!    a sync of the home image has failed, the volume has failed: from
//!    then on every request fails, reads included, and a later start
//!    refuses the volume until the failure is acknowledged (the log's
//!    marker). A failed sync is never tried again: the kernel may have
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
//! the page cache for that (point 1), where the log store checks its
//! log against the stored position.
//!
//! Where the page or the file system's block is larger than a block
//! (16 KiB or 64 KiB pages on some arm64 kernels and on Apple silicon),
//! a write rewrites the other blocks of its page with what the page
//! cache holds of them: their durable content or a version written
//! since, so point 3 still holds of them, and a block a flush covered
//! and nothing wrote since is rewritten with the bytes it holds. The
//! tests run at 16 KiB and 64 KiB pages on the simulated disk
//! (`tests.rs`).
//!
//! **A stated limit there (design §10): a disk that garbles.** On a
//! disk that leaves a sector neither old nor new at a power cut, the
//! rewrite of a page exposes its other blocks, so a cut can damage a
//! block a completed flush covered and nothing wrote since: in the
//! store's power-cut test with such a disk it did at 0 of 200 seeds at
//! 4 KiB pages, 120 at 16 KiB and 113 at 64 KiB (the review's count;
//! `a_cut_with_garbage_may_reach_a_neighbour_on_large_pages` pins only
//! that some seed of the 200 does at each large page).
//! At 4 KiB only a block being written is exposed. Nothing protects
//! against it here: the log store's open, which the start goes through,
//! refuses a page or a file-system block larger than the volume's write
//! unit, but a volume created on the large-page host has a unit as
//! large as its page, and the store saves no neighbours as the log
//! store did. Direct I/O, which writes exactly the blocks it is given,
//! is the candidate remedy (design §10).
//!
//! The other rules (R1 to R9, R11, R12) hold as for the log store, and
//! the conformance suite runs on this store with every capability
//! (`harness.rs`).
//!
//! # How it executes
//!
//! On the caller's thread, in [`BlockTarget::submit`], one request at a
//! time, in the order of submission: a read is a read of the home
//! image, a write a write at its offset, a write-zeroes deallocates the
//! range (or writes zeros where the file system cannot), and a discard
//! does nothing. Rule R6 allows a discarded block to keep its content,
//! and that is the least work: no call, no metadata to make durable,
//! and nothing a power cut can tear. It counts as a change all the
//! same, so that the flush behind it syncs: a flush that follows a
//! client's change never succeeds on a disk that has failed (R11), at
//! the cost of one sync where a discard alone came before it. A flush
//! begins a sync of the home image on the file's helper
//! ([`File::sync_begin`]) and completes when the sync ends; writes go
//! on meanwhile. Flushes that arrive while a
//! sync is under way join it when nothing was written since it began,
//! and otherwise wait for the next one, which begins when this one
//! ends and covers them all. No thread of the store's own, and no
//! hand-off for a read or a write: the pump's thread does the I/O.
//!
//! Not in it: an index, a checkpoint, a recovery beyond the log store's
//! open of an empty log, any count of what the log holds.

use std::collections::BTreeSet;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use skyzfs_blk::{
    BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind,
};

use crate::io::{Dir, File, NotBegun};
use crate::log::Log;
use crate::store::Zeroes;

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
    /// The start does not first show that the count sees the file.
    pub(crate) const CACHE_SIGHT_UNCHECKED: u32 = 1 << 9;

    pub(crate) fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Why [`Store::into_in_place`](crate::store::Store::into_in_place) did not give an in-place store.
#[derive(Debug, thiserror::Error)]
pub enum IntoInPlaceError {
    /// The log store has requests queued or in flight.
    #[error("the store has requests in flight")]
    InFlight,
    /// The volume has failed, or failed on the way: the log could not
    /// be applied or the home image could not be synced. The failure
    /// marker that says so to the next start may still be to write:
    /// [`FailedStart`] writes it when it is finished or dropped, so
    /// that the caller can first say what happened elsewhere.
    #[error("the volume has failed")]
    Failed(FailedStart),
    /// The log store's helper ended without a report: what it did is
    /// not known, and the volume is not marked as failed. The next
    /// open applies the log again, as after the same event while the
    /// log store serves.
    #[error("the store's helper ended without a report")]
    HelperEnded,
    /// The kernel's cache of the home image could not be dropped after
    /// the start's sync, or could not be shown to be gone: a start
    /// serves only what the disk holds (design §4.1), and refuses the
    /// volume. The volume has not failed.
    #[error("the start could not empty the kernel's cache of the home image: {0}")]
    CacheKept(String),
    /// The log still holds records after the checkpoint: a hold, or a
    /// part of it that is not durable.
    #[error("the log could not be emptied: {0}")]
    NotEmpty(String),
}

/// The store on the home image alone: see the
/// [module documentation](self).
pub struct InPlace<D: Dir> {
    /// The log, empty: for the failure marker and its observer.
    log: Log<D>,
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
    /// The failure marker: not yet needed, being written on a thread
    /// of its own, or tried. Whether it was written is not kept: as for
    /// the log store, a start that does not find it goes by the
    /// daemon's failure note.
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
/// ([`IntoInPlaceError::Failed`]): it holds the volume's lock and the
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
    /// A start that failed with the marker already tried, or with no
    /// marker of this store's to write.
    pub(crate) fn written() -> Self {
        Self {
            marker: Marker::Done,
            lock: None,
        }
    }

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
impl<D: Dir> Drop for InPlace<D> {
    fn drop(&mut self) {
        self.marker.finish();
    }
}

/// The parts of a log store an in-place store is made of.
pub(crate) struct Parts<D: Dir> {
    pub(crate) log: Log<D>,
    pub(crate) home: D::File,
    pub(crate) lock: Option<D::Lock>,
    pub(crate) geometry: Geometry,
    pub(crate) identity: [u8; 16],
    pub(crate) zeroes: Zeroes,
    /// What writes the failure marker ([`Log::marker_writer`]).
    pub(crate) marker: MarkerWriter,
}

impl<D: Dir> InPlace<D> {
    /// From the parts of a log store whose log is empty and whose home
    /// image holds everything: sync the home image, so that what this
    /// start exposes is durable before the first request. Where the
    /// sync fails, the volume has failed, and the marker is handed back
    /// unwritten with the lock ([`FailedStart`]).
    pub(crate) fn assemble(
        parts: Parts<D>,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        mutants: Mutants,
    ) -> Result<Self, IntoInPlaceError>
    where
        D::Lock: Send + 'static,
    {
        let mut store = Self {
            log: parts.log,
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
            marker: Marker::new(parts.marker, mutants),
        };
        // The checkpoint that entered this store synced the home image
        // as well; this sync makes the start's promise rest on nothing
        // but this store, and costs nothing when nothing is dirty.
        store.counts.syncs += 1;
        if store.home.sync().is_err() {
            store.failed = true;
            store.log.fail_without_marker();
            let marker = std::mem::replace(&mut store.marker, Marker::Done);
            let lock = store
                .lock
                .take()
                .map(|lock| Box::new(lock) as Box<dyn Send>);
            return Err(IntoInPlaceError::Failed(FailedStart { marker, lock }));
        }
        store.empty_cache().map_err(IntoInPlaceError::CacheKept)?;
        Ok(store)
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
    /// error once. The count is first shown to see this file
    /// ([`InPlace::count_sees_the_file`]). The drop is advisory, so it is
    /// tried twice; pages that stay, a drop that fails, a count that
    /// does not see the file, or a kernel that cannot count the file's
    /// cached pages refuse the start. Its cost: the first reads after a
    /// start, a take-over included, come from the disk.
    fn empty_cache(&mut self) -> Result<(), String> {
        if !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED) {
            self.count_sees_the_file()?;
        }
        let mut left = 0;
        for _ in 0..2 {
            if !self.mutants.has(Mutants::CACHE_NOT_DROPPED) {
                self.home
                    .drop_cache()
                    .map_err(|e| format!("the drop failed: {e}"))?;
            }
            if self.mutants.has(Mutants::CACHE_NOT_CHECKED) {
                return Ok(());
            }
            left = self.cached_pages()?;
            if left == 0 {
                return Ok(());
            }
        }
        Err(format!(
            "{left} pages of it stay in the cache after two drops; something else may map, \
             lock or read the file"
        ))
    }

    /// Show that the count sees this file's pages before it is trusted
    /// to see none: a block read through the cache must count. On an
    /// overlay file system `cachestat` counts the overlay file's own
    /// mapping, which holds no page (they are the file's below), and
    /// would read 0 whatever the cache holds; a file system without a
    /// page cache would read 0 too, and is refused the same way.
    fn count_sees_the_file(&mut self) -> Result<(), String> {
        let mut block = vec![0u8; skyzfs_blk::BLOCK_USIZE];
        self.home
            .read_at(0, &mut block)
            .map_err(|e| format!("reading a block of it failed: {e}"))?;
        if self.cached_pages()? == 0 {
            return Err(
                "the file system's cache cannot be observed through this file: a block \
                 just read through the cache counts 0 pages (as through an overlay file \
                 system), so the start cannot show that the drop left none"
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

    /// The store's identity, as the log store's
    /// ([`Store::identity`](crate::store::Store::identity)).
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

    /// How write-zeroes reach the home image: deallocated, or written
    /// as zeros where the file system cannot deallocate.
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
    /// the failure. (The log store writes its marker on its worker for
    /// the same reason.) Where no thread could be started, it is
    /// written here.
    fn fail(&mut self) {
        self.failed = true;
        if self.log.is_failed() {
            return;
        }
        self.log.fail_without_marker();
        self.marker.begin();
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
                // A failed read changes nothing and does not fail the
                // volume: only its own request fails.
                let result = self
                    .home
                    .read_at(offset, &mut buf)
                    .map_err(|e| BlkError::Io(e.kind()));
                self.complete(id, result, Some(buf));
            }
            Op::Flush => self.flush(id),
            op if self.geometry.read_only() => {
                let buf = buffer_of(op);
                self.complete(id, Err(BlkError::ReadOnly), buf);
            }
            Op::Write { offset, data } => {
                let written = self.home.write_at(offset, &data);
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
    fn zero(&mut self, offset: u64, len: u64) -> io::Result<()> {
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
        self.execute(request.id, request.op);
        Ok(())
    }

    /// Hands out what is complete. Looks whether the sync under way has
    /// ended, without waiting for it.
    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
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
        self.settle(false);
        if self.ready.is_empty() && !timeout.is_zero() {
            self.settle(true);
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
#[cfg(test)]
mod tests;

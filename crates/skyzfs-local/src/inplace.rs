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
//!    durable before it serves ([`Store::into_in_place`](crate::store::Store::into_in_place)).
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
//! and nothing wrote since is rewritten with the bytes it holds. Only
//! the disk's garbage reaches further there: such a neighbour's sector
//! is one a cut could take, and nothing about it is promised. The store
//! is entered through the log store's open, which refuses a page or a
//! file-system block larger than the volume's write unit (a volume
//! created where pages are 4 KiB is refused where they are 16 KiB);
//! the tests run at 16 KiB and 64 KiB pages on the simulated disk
//! (`tests.rs`).
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
    /// The failure marker is written on the caller's thread.
    pub(crate) const MARKER_INLINE: u32 = 1 << 6;

    pub(crate) fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Why [`Store::into_in_place`](crate::store::Store::into_in_place) did not give an in-place store.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IntoInPlaceError {
    /// The log store has requests queued or in flight.
    #[error("the store has requests in flight")]
    InFlight,
    /// The volume has failed, or failed on the way: the log could not
    /// be applied or the home image could not be synced. The log's
    /// marker says so to the next start.
    #[error("the volume has failed")]
    Failed,
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
    _lock: Option<D::Lock>,
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

enum Marker {
    Unwritten(MarkerWriter),
    Writing(std::thread::JoinHandle<bool>),
    Done,
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

/// The store waits for its failure marker when it is dropped, so that
/// a start after it finds the marker (or the failure to write it).
impl<D: Dir> Drop for InPlace<D> {
    fn drop(&mut self) {
        if let Marker::Writing(thread) = std::mem::replace(&mut self.marker, Marker::Done) {
            let _ = thread.join();
        }
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
    /// start exposes is durable before the first request.
    pub(crate) fn assemble(
        parts: Parts<D>,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        mutants: Mutants,
    ) -> Result<Self, IntoInPlaceError> {
        let mut store = Self {
            log: parts.log,
            home: parts.home,
            _lock: parts.lock,
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
            marker: Marker::Unwritten(parts.marker),
        };
        // The checkpoint that entered this store synced the home image
        // as well; this sync makes the start's promise rest on nothing
        // but this store, and costs nothing when nothing is dirty.
        store.counts.syncs += 1;
        if store.home.sync().is_err() {
            store.fail();
            return Err(IntoInPlaceError::Failed);
        }
        Ok(store)
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
    /// marker for the next start is written on a thread of its own. The
    /// marker needs the disk that has just failed, and a sync on a sick
    /// disk can take minutes or never return; the caller is the queue's
    /// thread, which must go on answering what it holds with the
    /// failure. (The log store writes its marker on its worker for the
    /// same reason.) Where no thread can be started, it is written here.
    fn fail(&mut self) {
        self.failed = true;
        if self.log.is_failed() {
            return;
        }
        self.log.fail_without_marker();
        let Marker::Unwritten(write) = std::mem::replace(&mut self.marker, Marker::Done) else {
            return;
        };
        if self.mutants.has(Mutants::MARKER_INLINE) {
            write();
            return;
        }
        // The writer goes to the thread through a slot, so that it is
        // still here when the thread cannot be started.
        let slot = Arc::new(std::sync::Mutex::new(Some(write)));
        let theirs = slot.clone();
        let started = std::thread::Builder::new()
            .name("skyzfs-marker".into())
            .spawn(move || {
                let write = theirs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                write.is_some_and(|write| write())
            });
        self.marker = match started {
            Ok(thread) => Marker::Writing(thread),
            Err(_) => {
                let write = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                if let Some(write) = write {
                    write();
                }
                Marker::Done
            }
        };
    }

    /// Whether the failure marker is being written, or was, on a thread
    /// of its own: for the test that shows it is not the caller's.
    #[cfg(test)]
    pub(crate) fn marker_on_its_own_thread(&self) -> bool {
        matches!(self.marker, Marker::Writing(_))
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

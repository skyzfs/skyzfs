//! The store: a block target on the write log and a home image.
//!
//! See the [crate documentation](crate#the-store) for what it promises
//! and why. In short: a write, a write-zeroes and a discard are each one
//! record of the [write log](crate::log); the **home image** is one file
//! of the volume's size that holds the volume as it was at the log's
//! start; an **index** in memory says, for every block that a record
//! above the log's start has written, which record holds its newest
//! version. A read takes each block from the log if the index has it and
//! from the home image otherwise. A **checkpoint** applies the durable
//! records to the home image, syncs it, and only then gives up that part
//! of the log.
//!
//! One request is executed at a time, in the order of submission.
//! [`Store`] executes in the caller's thread, inside
//! [`BlockTarget::wait`]; [`Served`] has a worker thread for it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;

use skyzfs_blk::{
    Base, BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind, BLOCK_SIZE, BLOCK_USIZE, MAX_IO_BYTES, SIZE_UNIT,
};

use crate::format::siphash24;
use crate::format::{Identity, Key, Kind, Lsn};
use crate::io::{Dir, File};
use crate::log::{Log, LogError, Observer, Options, Recovery};

/// The home image's file.
pub const HOME_NAME: &str = "home.img";
/// The home image while it is being created.
const HOME_NEW_NAME: &str = "home.new";
/// The preimages of a checkpoint in progress: see [the crate documentation](crate#the-preimage-file).
pub const PREIMAGE_NAME: &str = "home.pre";

/// Deliberate defects, for the tests that show each defence is needed.
/// Production code never sets one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

#[allow(dead_code)]
impl Mutants {
    /// A checkpoint applies records that are not durable yet.
    pub(crate) const CHECKPOINT_UNSYNCED: u32 = 1 << 0;
    /// A checkpoint goes beyond the hold.
    pub(crate) const CHECKPOINT_PAST_HOLD: u32 = 1 << 1;
    /// A checkpoint gives up the log before the home image is synced.
    pub(crate) const RECLAIM_BEFORE_HOME_SYNC: u32 = 1 << 2;
    /// A checkpoint switches to the home image every block whose record
    /// is durable, applied or not.
    pub(crate) const INDEX_SWITCH_EARLY: u32 = 1 << 3;
    /// An open does not rebuild the index from the log.
    pub(crate) const NO_INDEX_REBUILD: u32 = 1 << 4;
    /// A flush is acknowledged without a sync of the log.
    pub(crate) const FLUSH_NO_SYNC: u32 = 1 << 5;
    /// A write is acknowledged before it is appended: the append is
    /// left for the next request.
    pub(crate) const ACK_BEFORE_APPEND: u32 = 1 << 6;
    /// Requests are served after the volume has failed.
    pub(crate) const SERVE_AFTER_FAILURE: u32 = 1 << 7;
    /// A read takes every block from the home image.
    pub(crate) const READ_HOME_ONLY: u32 = 1 << 8;
    /// A checkpoint keeps no preimages of the blocks it rewrites
    /// without writing them.
    pub(crate) const NO_PREIMAGES: u32 = 1 << 9;
    /// A checkpoint's preimages are not made durable before the home
    /// image is written.
    pub(crate) const PREIMAGES_UNSYNCED: u32 = 1 << 10;
    /// An open does not take the directory's lock.
    pub(crate) const NO_LOCK: u32 = 1 << 11;
    /// An open takes a preimage file for the current one whatever
    /// start of the log it was written for.
    pub(crate) const STALE_PREIMAGES: u32 = 1 << 12;
    /// An open does not write the preimages back.
    pub(crate) const NO_RESTORE: u32 = 1 << 13;
    /// A checkpoint does not look at the result of its writes into the
    /// home image.
    pub(crate) const IGNORE_HOME_WRITE: u32 = 1 << 14;
    /// A volume that fails on its own files does not mark the log.
    pub(crate) const FAIL_UNMARKED: u32 = 1 << 15;
    /// A read that fails on the store's files fails alone: the volume
    /// goes on serving.
    pub(crate) const READ_ERROR_SERVED: u32 = 1 << 16;
    /// A hold is taken at any position.
    pub(crate) const HOLD_UNCHECKED: u32 = 1 << 18;
    /// A neighbour counts as held by the log when the index names a
    /// record for it, durable or not, and is not saved.
    pub(crate) const UNSYNCED_COVERS: u32 = 1 << 19;
    /// The acknowledged position is the end of the record, which is
    /// the log's end, and not where the record starts.
    pub(crate) const ACKNOWLEDGED_IS_END: u32 = 1 << 20;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// What a new store is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoreConfig {
    /// The volume the store holds.
    pub volume: [u8; 16],
    /// The volume's size in MiB.
    pub size_mib: u64,
    /// What the volume holds before anything is written.
    pub base: Base,
    /// The log's record alignment: 64 or 4096.
    pub align: u32,
    /// The size of a log segment. A segment must take a record with the
    /// largest write the contract allows, 1 MiB.
    pub segment_size: u64,
    /// The log's write unit: see [`Identity::write_unit`].
    pub write_unit: u32,
}

impl StoreConfig {
    fn identity(&self) -> Option<Identity> {
        let identity = Identity {
            volume: self.volume,
            device_size: self.size_mib.checked_mul(SIZE_UNIT)?,
            block_size: BLOCK_SIZE as u32,
            align: self.align,
            segment_size: self.segment_size,
            write_unit: self.write_unit,
        };
        (self.size_mib > 0 && identity.valid() && u64::from(identity.max_data()) >= MAX_IO_BYTES)
            .then_some(identity)
    }
}

/// Who is told that the volume has failed: see
/// [`OpenOptions::on_failure`].
#[derive(Clone)]
pub struct FailureObserver(Arc<dyn Fn() + Send + Sync>);

impl FailureObserver {
    /// An observer that calls `f`.
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for FailureObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FailureObserver")
    }
}

/// What the worker of a [`Served`] store calls when its owner has
/// something to take: a completion is ready, or the worker has ended.
/// A transport that sleeps on something else, such as an event
/// descriptor in its ring, is woken through it. It is called from the
/// worker's thread and must return quickly.
#[derive(Clone)]
pub struct Waker(Arc<dyn Fn() + Send + Sync>);

impl Waker {
    /// A waker that calls `f`.
    pub fn new(f: impl Fn() + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }
}

impl std::fmt::Debug for Waker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Waker")
    }
}

/// How a store is opened.
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// How many requests may be in flight.
    pub queue_depth: u32,
    /// Whether the volume is served read-only.
    pub read_only: bool,
    /// Open a store whose log is marked as failed. The caller knows that
    /// no client is attached that was told more than the log holds.
    pub acknowledge_failure: bool,
    /// The most segment files the log may have: at least two. When the
    /// log is full the store checkpoints to make room.
    pub log_segments: Option<usize>,
    /// Called once, at the moment the store decides that the volume
    /// has failed, and before the failure marker is attempted: the
    /// marker needs the disk that has just failed and may take minutes
    /// or never be written, and a transport has to give up its
    /// attachment at once (design §4.2). It is called from the thread
    /// that executes the store's requests, inside the request that
    /// failed, and is expected to return quickly.
    ///
    /// It is installed once the log has been opened, so a failure
    /// during the log's recovery is not announced. Nothing is lost by
    /// that: no client is attached before the open returns, and the
    /// open returns the error to the caller that would attach one.
    pub on_failure: Option<FailureObserver>,
    /// A position the log must still reach: what an earlier instance
    /// reported with [`Store::acknowledged`] and its transport stored
    /// before it told its client. If the log's valid prefix ends at or
    /// before it, the open fails with [`LogError::Short`] and has
    /// written nothing to the log: see [`Options::must_reach`], also
    /// for why the comparison is made inside the open. A position
    /// below the log's start, of a record a checkpoint has given up,
    /// is always reached.
    pub must_reach: Option<Lsn>,
    /// The store [`OpenOptions::must_reach`] was recorded for
    /// ([`Store::identity`]). If the store that is opened is another
    /// one, the position counts as none. A caller cannot make this
    /// comparison itself: the identity is known only once the log is
    /// read, and by then the open has to have compared. `None` applies
    /// the position whatever the store.
    pub must_reach_identity: Option<[u8; 16]>,
    /// For test rigs: see [`Options::pretend_short_by`]. Leave it 0.
    pub pretend_short_by: u32,
    pub(crate) mutants: Mutants,
    pub(crate) log_mutants: crate::log::Mutants,
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            queue_depth: 64,
            read_only: false,
            acknowledge_failure: false,
            log_segments: None,
            on_failure: None,
            must_reach: None,
            must_reach_identity: None,
            pretend_short_by: 0,
            mutants: Mutants::default(),
            log_mutants: crate::log::Mutants::default(),
        }
    }
}

/// A hold was asked for at a position inside the log where no record
/// starts or ends. Nothing changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("{0} is not a record boundary: the hold was not set")]
pub struct NotABoundary(pub Lsn);

/// Why a store could not be created or opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum StoreError {
    /// The parameters cannot describe a store.
    #[error("the store's parameters are not valid")]
    InvalidConfig,
    /// The log could not be created or opened.
    #[error("the write log: {0}")]
    Log(#[from] LogError),
    /// An operation on the home image failed.
    #[error("{during} failed ({kind:?})")]
    Io {
        /// What the store was doing.
        during: &'static str,
        /// The error's kind.
        kind: io::ErrorKind,
    },
    /// Another instance has the store open. Nothing was read or
    /// written.
    #[error("the store is open in another instance")]
    Locked,
    /// The store's files do not fit together.
    #[error("the store is damaged: {0}")]
    Damaged(&'static str),
}

fn io_error(during: &'static str) -> impl Fn(io::Error) -> StoreError {
    move |e| StoreError::Io {
        during,
        kind: e.kind(),
    }
}

/// Where the newest version of a run of blocks is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// In the payload of the write record at `record`, from its block
    /// `first` on.
    Data { record: Lsn, first: u64 },
    /// Zeros, by the write-zeroes record at `record`.
    Zero { record: Lsn },
}

impl Source {
    fn record(self) -> Lsn {
        match self {
            Source::Data { record, .. } | Source::Zero { record } => record,
        }
    }

    /// The same source, `blocks` further in.
    fn advanced(self, blocks: u64) -> Self {
        match self {
            Source::Data { record, first } => Source::Data {
                record,
                first: first + blocks,
            },
            zero => zero,
        }
    }
}

/// For every block whose newest version is in the log: where. Runs of
/// blocks, keyed by their first block, never overlapping.
#[derive(Debug, Default)]
struct Index {
    extents: BTreeMap<u64, (u64, Source)>,
}

impl Index {
    /// `len` blocks from `start` are now at `source`.
    fn insert(&mut self, start: u64, len: u64, source: Source) {
        let end = start + len;
        // A run that begins before `start` and reaches into the range
        // keeps its front, and its back if it reaches beyond the end.
        let before = self
            .extents
            .range(..start)
            .next_back()
            .map(|(s, (l, src))| (*s, *l, *src));
        if let Some((s, l, src)) = before {
            if s + l > start {
                self.extents.insert(s, (start - s, src));
                if s + l > end {
                    self.extents
                        .insert(end, (s + l - end, src.advanced(end - s)));
                }
            }
        }
        // Runs that begin inside the range go, but for what reaches
        // beyond the end.
        let inside: Vec<u64> = self.extents.range(start..end).map(|(s, _)| *s).collect();
        for s in inside {
            if let Some((l, src)) = self.extents.remove(&s) {
                if s + l > end {
                    self.extents
                        .insert(end, (s + l - end, src.advanced(end - s)));
                }
            }
        }
        self.extents.insert(start, (len, source));
    }

    /// The range cut into runs, in order: each with its source, or
    /// `None` where the log holds nothing.
    fn lookup(&self, start: u64, len: u64) -> Vec<(u64, u64, Option<Source>)> {
        let end = start + len;
        let first = self
            .extents
            .range(..=start)
            .next_back()
            .filter(|(s, (l, _))| **s + *l > start)
            .map_or(start, |(s, _)| *s);
        let mut out = Vec::new();
        let mut at = start;
        for (&s, &(l, src)) in self.extents.range(first..end) {
            if s > at {
                out.push((at, s - at, None));
                at = s;
            }
            let to = (s + l).min(end);
            out.push((at, to - at, Some(src.advanced(at - s))));
            at = to;
        }
        if at < end {
            out.push((at, end - at, None));
        }
        out
    }

    /// The log now starts at `floor`: what records below it held is in
    /// the home image.
    fn drop_below(&mut self, floor: Lsn) {
        self.extents.retain(|_, (_, src)| src.record() >= floor);
    }
}

/// The buffer an operation carries, for its completion.
fn buffer_of(op: Op) -> Option<Buf> {
    match op {
        Op::Read { buf, .. } => Some(buf),
        Op::Write { data, .. } => Some(data),
        _ => None,
    }
}

/// What a request came to: its result and its buffer.
type Done = (Result<(), BlkError>, Option<Buf>);

/// The serial core: the log, the home image and the index, and one
/// request at a time.
struct Core<D: Dir> {
    log: Log<D>,
    home: D::File,
    /// The preimage file, once a checkpoint has needed one.
    preimages: Option<D::File>,
    /// The directory's lock, held for as long as the store lives.
    _lock: Option<D::Lock>,
    index: Index,
    geometry: Geometry,
    /// The volume has failed: nothing is served any more.
    failed: bool,
    /// Records appended since the last sync, for tests that cut the
    /// power between them.
    unsynced: Vec<(Lsn, Lsn)>,
    /// With [`Mutants::ACK_BEFORE_APPEND`]: the write that was
    /// acknowledged and not appended.
    deferred: Option<(Kind, u64, u64, Vec<u8>)>,
    /// Where the record of the mutation that was just executed starts:
    /// taken by the driver when it hands out the completion.
    appended: Option<Lsn>,
    mutants: Mutants,
}

impl<D: Dir> Core<D> {
    /// Execute one request. `Err` hands the operation back: it cannot
    /// proceed now (the log is full behind a hold) and is to be tried
    /// again.
    fn execute(&mut self, op: Op) -> Result<Done, Op> {
        if let Err(rejected) = self.geometry.validate(&op) {
            return Ok((Err(rejected), buffer_of(op)));
        }
        if self.failed && !self.mutants.has(Mutants::SERVE_AFTER_FAILURE) {
            return Ok((Err(BlkError::Failed), buffer_of(op)));
        }
        self.settle_deferred();
        match op {
            Op::Read { offset, mut buf } => {
                let result = self.read(offset, &mut buf);
                Ok((result, Some(buf)))
            }
            Op::Flush => Ok((self.flush(), None)),
            op if self.geometry.read_only() => Ok((Err(BlkError::ReadOnly), buffer_of(op))),
            Op::Write { offset, data } => {
                match self.mutate(Kind::Write, offset, data.len() as u64, &data) {
                    Some(result) => Ok((result, Some(data))),
                    None => Err(Op::Write { offset, data }),
                }
            }
            Op::WriteZeroes { offset, len } => {
                match self.mutate(Kind::WriteZeroes, offset, len, &[]) {
                    Some(result) => Ok((result, None)),
                    None => Err(Op::WriteZeroes { offset, len }),
                }
            }
            Op::Discard { offset, len } => match self.mutate(Kind::Discard, offset, len, &[]) {
                Some(result) => Ok((result, None)),
                None => Err(Op::Discard { offset, len }),
            },
        }
    }

    /// The volume fails: something of the store's own could not be read
    /// or written. Nothing is served from now on, and the log's marker
    /// says so to the next instance.
    fn fail(&mut self) -> BlkError {
        self.failed = true;
        if !self.log.is_failed() && !self.mutants.has(Mutants::FAIL_UNMARKED) {
            self.log.mark_failed();
        }
        BlkError::Failed
    }

    /// See [`Store::identity`].
    fn identity(&self) -> [u8; 16] {
        crate::format::store_identity(self.log.key())
    }

    /// Set or release the hold. A position inside the log must be a
    /// record boundary: a checkpoint reclaims up to the hold, and a
    /// reclaim takes nothing else. One at or below the log's start
    /// holds the whole log and is taken as it is.
    fn set_hold(&mut self, hold: Option<Lsn>) -> Result<(), NotABoundary> {
        if let Some(position) = hold {
            let inside = position > self.log.floor();
            if inside && !self.mutants.has(Mutants::HOLD_UNCHECKED) {
                let tail = self.log.tail();
                let found = self
                    .log
                    .replay_within(position, position, tail, &mut |_| {});
                if matches!(found, Err(LogError::BadPosition(_))) {
                    return Err(NotABoundary(position));
                }
            }
        }
        self.log.set_hold(hold);
        Ok(())
    }

    /// Append a record and point the index at it. `None`: the log is
    /// full and a checkpoint could not make room.
    ///
    /// The operation is acknowledged, by the caller, only after the
    /// append has returned: from then on it survives the loss of the
    /// process, and so does every operation before it.
    fn mutate(
        &mut self,
        kind: Kind,
        offset: u64,
        len: u64,
        data: &[u8],
    ) -> Option<Result<(), BlkError>> {
        if self.mutants.has(Mutants::ACK_BEFORE_APPEND) {
            self.deferred = Some((kind, offset, len, data.to_vec()));
            return Some(Ok(()));
        }
        let mut made_room = false;
        let appended = loop {
            match self.log.append(kind, offset, len, data) {
                Ok(appended) => break appended,
                Err(LogError::Full) if !made_room => {
                    made_room = true;
                    if let Err(failed) = self.make_room() {
                        return Some(Err(failed));
                    }
                }
                // Full still: a hold keeps the log from being given up.
                Err(LogError::Full) => return None,
                Err(_) => return Some(Err(self.fail())),
            }
        };
        self.unsynced.push((appended.start, appended.end));
        self.appended = Some(if self.mutants.has(Mutants::ACKNOWLEDGED_IS_END) {
            appended.end
        } else {
            appended.start
        });
        let (start, blocks) = (offset / BLOCK_SIZE, len / BLOCK_SIZE);
        match kind {
            Kind::Write => self.index.insert(
                start,
                blocks,
                Source::Data {
                    record: appended.start,
                    first: 0,
                },
            ),
            Kind::WriteZeroes => self.index.insert(
                start,
                blocks,
                Source::Zero {
                    record: appended.start,
                },
            ),
            // A discard leaves the blocks as they were: it is in the log
            // for whoever uploads, and changes nothing here.
            _ => {}
        }
        Some(Ok(()))
    }

    /// With [`Mutants::ACK_BEFORE_APPEND`], append the write that was
    /// acknowledged earlier.
    fn settle_deferred(&mut self) {
        if let Some((kind, offset, len, data)) = self.deferred.take() {
            let mutants = self.mutants;
            self.mutants = Mutants(mutants.0 & !Mutants::ACK_BEFORE_APPEND);
            // A defect on purpose: the result goes nowhere.
            let _ = self.mutate(kind, offset, len, &data);
            self.mutants = mutants;
        }
    }

    /// A flush: every record appended so far is durable when it returns.
    fn flush(&mut self) -> Result<(), BlkError> {
        if self.mutants.has(Mutants::FLUSH_NO_SYNC) {
            return Ok(());
        }
        // With nothing to make durable the log's sync does not touch
        // the disk, and such a flush promises nothing new: a disk that
        // would refuse the next sync and has not been asked has not
        // failed yet (design §4.1).
        let synced = self.log.sync().map(|_| ());
        match synced {
            Ok(()) => {
                self.unsynced.clear();
                Ok(())
            }
            Err(_) => Err(self.fail()),
        }
    }

    /// A read: each block from the record the index names, or from the
    /// home image.
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), BlkError> {
        let start = offset / BLOCK_SIZE;
        let blocks = (buf.len() / BLOCK_USIZE) as u64;
        let runs = if self.mutants.has(Mutants::READ_HOME_ONLY) {
            vec![(start, blocks, None)]
        } else {
            self.index.lookup(start, blocks)
        };
        for (run, len, source) in runs {
            let from = ((run - start) * BLOCK_SIZE) as usize;
            let part = &mut buf[from..from + (len * BLOCK_SIZE) as usize];
            let read = match source {
                Some(Source::Data { record, first }) => self
                    .log
                    .read_payload(record, first * BLOCK_SIZE, part)
                    .is_ok(),
                Some(Source::Zero { .. }) => {
                    part.fill(0);
                    true
                }
                None => self.home.read_at(run * BLOCK_SIZE, part).is_ok(),
            };
            if !read {
                // A store that cannot read its own files does not go on
                // serving.
                if self.mutants.has(Mutants::READ_ERROR_SERVED) {
                    return Err(BlkError::Failed);
                }
                return Err(self.fail());
            }
        }
        Ok(())
    }

    /// Make room in a full log: make the log durable, so that all of it
    /// can be applied, and checkpoint.
    fn make_room(&mut self) -> Result<(), BlkError> {
        self.flush()?;
        self.checkpoint().map(|_| ())
    }

    /// Apply the log's durable records, up to the hold, to the home
    /// image, and give up that part of the log. Returns where the log
    /// starts now.
    ///
    /// The order is what makes a crash at any point harmless:
    ///
    /// 1. the records are written into the home image, which is synced;
    /// 2. only then is the log given up, so that until the home image
    ///    is durable the log still holds every record that was applied,
    ///    and an open rebuilds the index from them;
    /// 3. only then does the index let go of those records, so that no
    ///    read goes to the home image for a block before the home image
    ///    is known to hold it.
    fn checkpoint(&mut self) -> Result<Lsn, BlkError> {
        let floor = self.log.floor();
        let limit = if self.mutants.has(Mutants::CHECKPOINT_UNSYNCED) {
            self.log.tail()
        } else {
            self.log.durable()
        };
        let hold = self.log.hold();
        let target = match hold {
            Some(hold) if !self.mutants.has(Mutants::CHECKPOINT_PAST_HOLD) => limit.min(hold),
            _ => limit,
        };
        if target <= floor {
            return Ok(floor);
        }
        let per_unit = self.log.identity().unit() / BLOCK_SIZE;
        let ends = if per_unit <= 1 {
            vec![target]
        } else {
            match self.batches(floor, target, limit, per_unit) {
                Ok(ends) => ends,
                Err(()) => return Err(self.fail()),
            }
        };
        let mut start = floor;
        for end in ends {
            start = self.apply(start, end, limit, hold)?;
        }
        Ok(start)
    }

    /// Where the batches of a checkpoint from `floor` to `target` end.
    /// A batch is as many records as keep the blocks to save in the
    /// preimage file within [`PREIMAGE_BATCH`], and at least one.
    fn batches(
        &mut self,
        floor: Lsn,
        target: Lsn,
        limit: Lsn,
        per_unit: u64,
    ) -> Result<Vec<Lsn>, ()> {
        let mut ends = Vec::new();
        let (mut saved, mut records) = (0u64, 0usize);
        self.log
            .replay_within(floor, target, limit, &mut |record| {
                if !matches!(record.kind, Kind::Write | Kind::WriteZeroes) {
                    return;
                }
                // At most the rest of its first and of its last unit.
                let start = record.offset / BLOCK_SIZE;
                let end = start + record.length / BLOCK_SIZE;
                let own = start % per_unit + (end.div_ceil(per_unit) * per_unit - end);
                if records > 0 && (saved + own > PREIMAGE_BATCH as u64 || records >= BATCH_RECORDS)
                {
                    ends.push(record.start);
                    (saved, records) = (0, 0);
                }
                saved += own;
                records += 1;
            })
            .map_err(|_| ())?;
        ends.push(target);
        Ok(ends)
    }

    /// One batch of a checkpoint: the records from `floor`, where the
    /// log starts, to `target`.
    fn apply(
        &mut self,
        floor: Lsn,
        target: Lsn,
        limit: Lsn,
        hold: Option<Lsn>,
    ) -> Result<Lsn, BlkError> {
        if self.protect_neighbours(floor, target, limit).is_err() {
            return Err(self.fail());
        }
        let home = &mut self.home;
        let mut written = Ok(());
        let replayed = self.log.replay_within(floor, target, limit, &mut |record| {
            if written.is_err() {
                return;
            }
            written = match record.kind {
                Kind::Write => home.write_at(record.offset, record.data),
                Kind::WriteZeroes => write_zeros(home, record.offset, record.length),
                _ => Ok(()),
            };
        });
        let written = written.is_err() && !self.mutants.has(Mutants::IGNORE_HOME_WRITE);
        if replayed.is_err() || written {
            return Err(self.fail());
        }
        let early = self.mutants.has(Mutants::RECLAIM_BEFORE_HOME_SYNC);
        if !early && self.home.sync().is_err() {
            return Err(self.fail());
        }
        // The home image is durable. The reclaim moves the log's start,
        // and with that the preimages, which are bound to the start
        // they were written for, have done their work.
        if self.mutants.has(Mutants::CHECKPOINT_PAST_HOLD) {
            self.log.set_hold(None);
        }
        let reclaimed = self.log.reclaim(target);
        self.log.set_hold(hold);
        let Ok(start) = reclaimed else {
            return Err(self.fail());
        };
        if early && self.home.sync().is_err() {
            return Err(self.fail());
        }
        if self.mutants.has(Mutants::INDEX_SWITCH_EARLY) {
            self.index.drop_below(self.log.durable());
        } else {
            self.index.drop_below(start);
        }
        Ok(start)
    }
}

impl<D: Dir> Core<D> {
    /// Before a checkpoint writes into the home image: save the blocks
    /// that it will rewrite without writing them.
    ///
    /// A write into the home image rewrites the whole write unit around
    /// it, and a power cut can leave any rewritten sector as garbage.
    /// The blocks the checkpoint writes are safe: the log holds them
    /// until the home image is durable. Their neighbours in the same
    /// unit may be held by the home image alone. Those are copied into
    /// the preimage file, which is made durable first, and an open that
    /// finds it whole writes them back. Where the write unit is one
    /// block there are no neighbours and no file.
    fn protect_neighbours(&mut self, floor: Lsn, target: Lsn, limit: Lsn) -> Result<(), ()> {
        let per_unit = self.log.identity().unit() / BLOCK_SIZE;
        if per_unit <= 1 || self.mutants.has(Mutants::NO_PREIMAGES) {
            return Ok(());
        }
        // The blocks the checkpoint writes, as disjoint runs.
        let mut written: BTreeMap<u64, u64> = BTreeMap::new();
        self.log
            .replay_within(floor, target, limit, &mut |record| {
                if matches!(record.kind, Kind::Write | Kind::WriteZeroes) {
                    let start = record.offset / BLOCK_SIZE;
                    add_run(&mut written, start, start + record.length / BLOCK_SIZE);
                }
            })
            .map_err(|_| ())?;
        // Their neighbours: the rest of the first and of the last unit
        // of each run.
        let mut neighbours = Vec::new();
        for (&start, &end) in &written {
            let head = start / per_unit * per_unit;
            let tail = end.div_ceil(per_unit) * per_unit;
            for block in (head..start).chain(end..tail) {
                let covered = written
                    .range(..=block)
                    .next_back()
                    .is_some_and(|(_, e)| *e > block);
                // A block that a durable record of the log writes is
                // covered by the log, whatever becomes of its copy here.
                // A record that is not durable covers nothing: a power
                // cut can take it and the block's copy here together.
                let durable = self.log.durable();
                let logged = self
                    .index
                    .lookup(block, 1)
                    .first()
                    .and_then(|(_, _, source)| *source)
                    .is_some_and(|source| {
                        source.record() < durable || self.mutants.has(Mutants::UNSYNCED_COVERS)
                    });
                if !covered && !logged && neighbours.last() != Some(&block) {
                    neighbours.push(block);
                }
            }
        }
        neighbours.sort_unstable();
        neighbours.dedup();
        if neighbours.is_empty() {
            return Ok(());
        }
        let mut blocks = vec![0u8; neighbours.len() * BLOCK_USIZE];
        for (block, data) in neighbours.iter().zip(blocks.chunks_mut(BLOCK_USIZE)) {
            self.home
                .read_at(block * BLOCK_SIZE, data)
                .map_err(|_| ())?;
        }
        let file = match self.preimages.take() {
            Some(file) => file,
            None => {
                let dir = self.log.dir();
                let file = match dir.create(PREIMAGE_NAME) {
                    Ok(file) => file,
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        dir.open(PREIMAGE_NAME).map_err(|_| ())?
                    }
                    Err(_) => return Err(()),
                };
                dir.sync().map_err(|_| ())?;
                file
            }
        };
        let file = self.preimages.insert(file);
        let volume = self.log.identity().volume;
        let image = Preimages::encode(self.log.key(), &volume, floor, &neighbours, &blocks);
        file.write_at(0, &image).map_err(|_| ())?;
        if !self.mutants.has(Mutants::PREIMAGES_UNSYNCED) {
            file.sync().map_err(|_| ())?;
        }
        Ok(())
    }
}

/// Add the run `start..end` to disjoint runs keyed by their start,
/// merging what touches.
fn add_run(runs: &mut BTreeMap<u64, u64>, mut start: u64, mut end: u64) {
    if let Some((&s, &e)) = runs.range(..=start).next_back() {
        if e >= start {
            start = s;
            end = end.max(e);
        }
    }
    let inside: Vec<u64> = runs.range(start..=end).map(|(s, _)| *s).collect();
    for s in inside {
        if let Some(e) = runs.remove(&s) {
            end = end.max(e);
        }
    }
    runs.insert(start, end);
}

/// The preimage file: blocks of the home image as they were before a
/// batch of a checkpoint began to rewrite their write units.
///
/// | Offset | Bytes | Field |
/// |---|---|---|
/// | 0 | 8 | magic `SKYZPRE\0` |
/// | 8 | 4 | version, 1 |
/// | 12 | 4 | number of blocks, `n` |
/// | 16 | 16 | volume id |
/// | 32 | 8 | the log's start when the batch began |
/// | 40 | 8 | tag of the table |
/// | 48 | 456 | reserved, zero |
/// | 504 | 8 | tag of bytes 0..504 |
/// | 4096 | 16 `n` | the table: for each block its number (8) and its tag (8) |
/// | 4096 + the table, rounded up to 4096 | 4096 `n` | the blocks |
///
/// Every integer is little-endian. A tag is SipHash-2-4 under the
/// volume's key: of the header's first 504 bytes; of the log's start
/// followed by the table; and for a block, of the log's start, the
/// block's number and its content. The file is whole if every tag
/// matches, and it is the current one if its start is where the log
/// starts now. Anything else is a file that was being written, or one
/// whose batch is finished, and is ignored.
struct Preimages;

const PREIMAGE_MAGIC: &[u8; 8] = b"SKYZPRE\0";
const PREIMAGE_VERSION: u32 = 1;
const PREIMAGE_HEADER: usize = 512;
/// Where the header's own tag is.
const PREIMAGE_TAG: usize = PREIMAGE_HEADER - 8;
/// The most blocks one batch of a checkpoint saves: the file is never
/// larger than these, their table and the header.
pub(crate) const PREIMAGE_BATCH: usize = 256;
/// The most records in one batch.
const BATCH_RECORDS: usize = 1024;

/// Saved blocks: each with its number in the volume.
type Saved = Vec<(u64, Vec<u8>)>;

impl Preimages {
    fn table_len(count: usize) -> usize {
        (count * 16).div_ceil(BLOCK_USIZE) * BLOCK_USIZE
    }

    fn table_tag(key: &Key, start: Lsn, table: &[u8]) -> u64 {
        let mut input = Vec::with_capacity(8 + table.len());
        input.extend_from_slice(&start.0.to_le_bytes());
        input.extend_from_slice(table);
        siphash24(key, &input)
    }

    fn block_tag(key: &Key, start: Lsn, number: u64, block: &[u8]) -> u64 {
        let mut input = Vec::with_capacity(16 + block.len());
        input.extend_from_slice(&start.0.to_le_bytes());
        input.extend_from_slice(&number.to_le_bytes());
        input.extend_from_slice(block);
        siphash24(key, &input)
    }

    /// The file's bytes for `numbers` and their content in `blocks`, of
    /// a batch that began with the log starting at `start`.
    fn encode(key: &Key, volume: &[u8; 16], start: Lsn, numbers: &[u64], blocks: &[u8]) -> Vec<u8> {
        let table_len = Self::table_len(numbers.len());
        let mut out = vec![0u8; BLOCK_USIZE + table_len + blocks.len()];
        let (head, rest) = out.split_at_mut(BLOCK_USIZE);
        let (table, data) = rest.split_at_mut(table_len);
        data.copy_from_slice(blocks);
        for ((number, block), entry) in numbers
            .iter()
            .zip(blocks.chunks(BLOCK_USIZE))
            .zip(table.chunks_mut(16))
        {
            entry[..8].copy_from_slice(&number.to_le_bytes());
            let tag = Self::block_tag(key, start, *number, block);
            entry[8..].copy_from_slice(&tag.to_le_bytes());
        }
        head[..8].copy_from_slice(PREIMAGE_MAGIC);
        head[8..12].copy_from_slice(&PREIMAGE_VERSION.to_le_bytes());
        head[12..16].copy_from_slice(&(numbers.len() as u32).to_le_bytes());
        head[16..32].copy_from_slice(volume);
        head[32..40].copy_from_slice(&start.0.to_le_bytes());
        head[40..48].copy_from_slice(&Self::table_tag(key, start, table).to_le_bytes());
        let tag = siphash24(key, &head[..PREIMAGE_TAG]);
        head[PREIMAGE_TAG..PREIMAGE_HEADER].copy_from_slice(&tag.to_le_bytes());
        out
    }

    /// The blocks of a whole file written for the log's start `start`
    /// (for any start, if `start` is `None`), with their numbers;
    /// `None` for anything else.
    fn decode<F: File>(
        file: &F,
        key: &Key,
        volume: &[u8; 16],
        start: Option<Lsn>,
        device_blocks: u64,
    ) -> io::Result<Option<Saved>> {
        let long = |bytes: &[u8]| {
            let mut raw = [0u8; 8];
            raw.copy_from_slice(&bytes[..8]);
            u64::from_le_bytes(raw)
        };
        let word = |bytes: &[u8]| long(&[&bytes[..4], &[0u8; 4][..]].concat()) as u32;
        let len = file.len()?;
        if len < BLOCK_SIZE {
            return Ok(None);
        }
        let mut head = [0u8; PREIMAGE_HEADER];
        file.read_at(0, &mut head)?;
        let count = word(&head[12..16]) as usize;
        let table_len = Self::table_len(count);
        let whole = (BLOCK_USIZE + table_len + count * BLOCK_USIZE) as u64;
        let written_for = Lsn(long(&head[32..40]));
        if &head[..8] != PREIMAGE_MAGIC
            || long(&head[PREIMAGE_TAG..]) != siphash24(key, &head[..PREIMAGE_TAG])
            || word(&head[8..12]) != PREIMAGE_VERSION
            || &head[16..32] != volume
            || start.is_some_and(|start| start != written_for)
            || head[48..PREIMAGE_TAG].iter().any(|b| *b != 0)
            || count > PREIMAGE_BATCH
            || len < whole
        {
            return Ok(None);
        }
        let mut table = vec![0u8; table_len];
        file.read_at(BLOCK_SIZE, &mut table)?;
        if long(&head[40..48]) != Self::table_tag(key, written_for, &table) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(count);
        for (i, entry) in table.chunks(16).take(count).enumerate() {
            let number = long(&entry[..8]);
            let mut block = vec![0u8; BLOCK_USIZE];
            file.read_at(
                (BLOCK_USIZE + table_len + i * BLOCK_USIZE) as u64,
                &mut block,
            )?;
            if number >= device_blocks
                || long(&entry[8..]) != Self::block_tag(key, written_for, number, &block)
            {
                return Ok(None);
            }
            out.push((number, block));
        }
        Ok(Some(out))
    }

    /// At an open: if a batch of a checkpoint left its preimages whole
    /// and the log still starts where that batch began, write them back
    /// into the home image and make that durable. A batch that was cut
    /// off may have damaged these blocks; one that was not has left
    /// them as the file has them. The file stays as it is: a cut during
    /// the writing back is repaired by the next open, and the file
    /// stops counting when the log's start moves.
    fn restore<D: Dir>(
        log: &Log<D>,
        home: &mut D::File,
        mutants: Mutants,
    ) -> io::Result<Option<D::File>> {
        let file = match log.dir().open(PREIMAGE_NAME) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if mutants.has(Mutants::NO_RESTORE) {
            return Ok(Some(file));
        }
        let identity = log.identity();
        let start = (!mutants.has(Mutants::STALE_PREIMAGES)).then(|| log.floor());
        let blocks = identity.device_size / BLOCK_SIZE;
        if let Some(saved) = Self::decode(&file, log.key(), &identity.volume, start, blocks)? {
            for (number, block) in &saved {
                home.write_at(number * BLOCK_SIZE, block)?;
            }
            home.sync()?;
        }
        Ok(Some(file))
    }
}

/// Write `len` zeros at `offset`.
fn write_zeros<F: File>(file: &mut F, offset: u64, len: u64) -> io::Result<()> {
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

/// The store, executing in the caller's thread.
///
/// [`BlockTarget::submit`] queues a request and returns.
/// [`BlockTarget::wait`] executes what is queued, in the order of
/// submission, and returns the completions: in this driver it does the
/// store's I/O and so can take as long as a sync takes.
/// [`BlockTarget::poll`] never blocks, as the contract says, and so
/// does nothing here: a caller of this driver has to call `wait`. A
/// write that finds the log full behind a hold stays at
/// the head of the queue, with everything submitted after it behind it,
/// until the hold is released.
pub struct Store<D: Dir> {
    core: Core<D>,
    /// Where the newest record starts among the mutations whose
    /// completions have been handed out.
    acknowledged: Option<Lsn>,
    queue: VecDeque<Request>,
    /// The ids in flight: submitted and not yet returned.
    ids: BTreeSet<RequestId>,
}

impl<D: Dir> Store<D> {
    /// Make a new store in `dir` and open it.
    pub fn create(dir: D, config: &StoreConfig, options: OpenOptions) -> Result<Self, StoreError> {
        let key = Key::random().ok_or(LogError::NoRandomness)?;
        Self::create_keyed(dir, config, key, options)
    }

    /// The same with a given key for the log, for tests that must be
    /// reproducible to the bit.
    pub(crate) fn create_keyed(
        dir: D,
        config: &StoreConfig,
        key: Key,
        options: OpenOptions,
    ) -> Result<Self, StoreError> {
        let identity = config.identity().ok_or(StoreError::InvalidConfig)?;
        let lock = Self::lock(&dir, &options)?;
        let names = dir.list().map_err(io_error("listing the directory"))?;
        if names.iter().any(|name| name == crate::log::SUPER_NAME) {
            return Err(LogError::Exists.into());
        }
        // The log's superblock is what makes a store, and it is made
        // last. Whatever else is here is left from a creation that was
        // interrupted.
        for name in names.iter().filter(|name| name.starts_with("home.")) {
            dir.remove(name).map_err(io_error("removing leftovers"))?;
        }
        let creating = io_error("creating the home image");
        let mut home = dir.create(HOME_NEW_NAME).map_err(&creating)?;
        home.extend(identity.device_size).map_err(&creating)?;
        if config.base != Base::Empty {
            // The base, block by block, a megabyte at a time.
            let per_chunk = SIZE_UNIT / BLOCK_SIZE;
            let mut chunk = vec![0u8; SIZE_UNIT as usize];
            for mib in 0..config.size_mib {
                for (i, block) in chunk.chunks_mut(BLOCK_USIZE).enumerate() {
                    config.base.fill_block(mib * per_chunk + i as u64, block);
                }
                home.write_at(mib * SIZE_UNIT, &chunk).map_err(&creating)?;
            }
        }
        home.sync().map_err(&creating)?;
        dir.rename(HOME_NEW_NAME, HOME_NAME).map_err(&creating)?;
        dir.sync().map_err(&creating)?;
        drop(home);
        let log_options = Options {
            mutants: options.log_mutants,
            ..Options::default()
        };
        let log = Log::create_keyed(dir, identity, key, log_options)?;
        let home = log
            .dir()
            .open(HOME_NAME)
            .map_err(io_error("opening the home image"))?;
        Self::assemble(log, home, lock, options)
    }

    /// Take the directory's lock: one instance per volume (design
    /// §4.2). Opening a store is a recovery that writes, so this comes
    /// before anything is read or written.
    fn lock(dir: &D, options: &OpenOptions) -> Result<Option<D::Lock>, StoreError> {
        if options.mutants.has(Mutants::NO_LOCK) {
            return Ok(None);
        }
        match dir.try_lock() {
            Ok(lock) => Ok(Some(lock)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(StoreError::Locked),
            Err(e) => Err(io_error("locking the directory")(e)),
        }
    }

    /// Open the store in `dir`: take its lock, recover the log, and
    /// rebuild the index from it. While another instance has the store
    /// open this fails with [`StoreError::Locked`] and changes nothing.
    pub fn open(dir: D, options: OpenOptions) -> Result<(Self, Recovery), StoreError> {
        let lock = Self::lock(&dir, &options)?;
        let log_options = Options {
            acknowledge_failure: options.acknowledge_failure,
            must_reach: options.must_reach,
            must_reach_identity: options.must_reach_identity,
            pretend_short_by: options.pretend_short_by,
            mutants: options.log_mutants,
        };
        let (log, recovery) = Log::open(dir, log_options)?;
        let home = match log.dir().open(HOME_NAME) {
            Ok(home) => home,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(StoreError::Damaged("there is no home image"))
            }
            Err(e) => return Err(io_error("opening the home image")(e)),
        };
        Ok((Self::assemble(log, home, lock, options)?, recovery))
    }

    fn assemble(
        mut log: Log<D>,
        mut home: D::File,
        lock: Option<D::Lock>,
        options: OpenOptions,
    ) -> Result<Self, StoreError> {
        if let Some(FailureObserver(call)) = options.on_failure.clone() {
            log.set_observer(Observer::new(move || call()));
        }
        let identity = *log.identity();
        let size = home.len().map_err(io_error("reading the home image"))?;
        if size != identity.device_size {
            return Err(StoreError::Damaged(
                "the home image is not of the volume's size",
            ));
        }
        // What a checkpoint that was cut off may have damaged beside
        // its own blocks, before anything reads the home image.
        let preimages = Preimages::restore(&log, &mut home, options.mutants)
            .map_err(io_error("restoring the home image"))?;
        if options.log_segments.is_some_and(|limit| limit < 2) {
            return Err(StoreError::InvalidConfig);
        }
        let geometry = Geometry::new(
            identity.device_size / SIZE_UNIT,
            options.queue_depth,
            options.read_only,
        )
        .map_err(|_| StoreError::InvalidConfig)?;
        log.set_segment_limit(options.log_segments);
        // The index: every record of the log, in order. After an open
        // the whole log is durable.
        let mut index = Index::default();
        if !options.mutants.has(Mutants::NO_INDEX_REBUILD) {
            log.replay(log.floor(), log.tail(), &mut |record| {
                let start = record.offset / BLOCK_SIZE;
                let blocks = record.length / BLOCK_SIZE;
                let at = record.start;
                match record.kind {
                    Kind::Write => index.insert(
                        start,
                        blocks,
                        Source::Data {
                            record: at,
                            first: 0,
                        },
                    ),
                    Kind::WriteZeroes => index.insert(start, blocks, Source::Zero { record: at }),
                    _ => {}
                }
            })?;
        }
        Ok(Self {
            core: Core {
                log,
                home,
                preimages,
                _lock: lock,
                index,
                geometry,
                failed: false,
                unsynced: Vec::new(),
                deferred: None,
                appended: None,
                mutants: options.mutants,
            },
            acknowledged: None,
            queue: VecDeque::new(),
            ids: BTreeSet::new(),
        })
    }

    /// Apply the log's durable records, up to the hold, to the home
    /// image and give up that part of the log. Returns where the log
    /// starts now. On a failed volume, and if it fails the volume, an
    /// error.
    pub fn checkpoint(&mut self) -> Result<Lsn, BlkError> {
        if self.core.failed {
            return Err(BlkError::Failed);
        }
        self.core.checkpoint()
    }

    /// The end of the log: the position of the next operation in the
    /// volume's order. What a hold is set to.
    pub fn position(&self) -> Lsn {
        self.core.log.tail()
    }

    /// Where the log starts: everything before it is in the home image
    /// only.
    pub fn log_start(&self) -> Lsn {
        self.core.log.floor()
    }

    /// The hold: no checkpoint gives up the log from this position on.
    pub fn hold(&self) -> Option<Lsn> {
        self.core.log.hold()
    }

    /// Set or release the hold. A position is one [`Store::position`]
    /// returned: a position inside the log that is not a record
    /// boundary is refused, and the hold stays as it was. The hold is
    /// not stored: after an open it is set again before the first
    /// checkpoint.
    pub fn set_hold(&mut self, hold: Option<Lsn>) -> Result<(), NotABoundary> {
        self.core.set_hold(hold)
    }

    /// Whether the volume has failed.
    pub fn is_failed(&self) -> bool {
        self.core.failed
    }

    /// The log position at which the newest record starts among the
    /// mutations whose completions [`BlockTarget::wait`] has returned.
    /// `None` before the first. A transport stores it before it tells
    /// its client, and the next instance hands it to the open
    /// ([`OpenOptions::must_reach`]), which requires that the prefix it
    /// recovers ends beyond it: design §4.2, "what
    /// was acknowledged". It is where the record
    /// starts and not where it ends, so that the padding at a change of
    /// segment and after a recovery cannot make the two differ, and it
    /// stays valid when a checkpoint has given the record up: it is
    /// then below the log's start, and still below its end.
    pub fn acknowledged(&self) -> Option<Lsn> {
        self.acknowledged
    }

    /// An identity of this store, derived from the log's key: the same
    /// for every open of one store, and different for a store made
    /// again in the same directory or for the same volume. It is not
    /// the key and does not give it away. A position kept outside the
    /// store ([`Store::acknowledged`]) is kept with it, so that one
    /// recorded for another store counts as none.
    pub fn identity(&self) -> [u8; 16] {
        self.core.identity()
    }

    /// Execute up to `limit` queued requests, in order, and collect
    /// their completions. Stops at a request that cannot proceed.
    fn run(&mut self, limit: usize, out: &mut Vec<Completion>) -> usize {
        let mut done = 0;
        while done < limit {
            let Some(Request { id, op }) = self.queue.pop_front() else {
                break;
            };
            match self.core.execute(op) {
                Ok((result, buf)) => {
                    self.ids.remove(&id);
                    if let Some(start) = self.core.appended.take() {
                        self.acknowledged = Some(start);
                    }
                    out.push(Completion { id, result, buf });
                    done += 1;
                }
                Err(op) => {
                    self.queue.push_front(Request { id, op });
                    break;
                }
            }
        }
        done
    }
}

impl<D: Dir> std::fmt::Debug for Store<D> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("geometry", &self.core.geometry)
            .field("log_start", &self.core.log.floor())
            .field("position", &self.core.log.tail())
            .field("failed", &self.core.failed)
            .field("queued", &self.queue.len())
            .finish()
    }
}

impl<D: Dir> BlockTarget for Store<D> {
    fn geometry(&self) -> Geometry {
        self.core.geometry
    }

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        let kind = if self.ids.contains(&request.id) {
            Some(SubmitErrorKind::DuplicateId)
        } else if self.ids.len() >= self.core.geometry.queue_depth() as usize {
            Some(SubmitErrorKind::Busy)
        } else {
            None
        };
        if let Some(kind) = kind {
            return Err(SubmitError { kind, request });
        }
        self.ids.insert(request.id);
        self.queue.push_back(request);
        Ok(())
    }

    /// Does nothing and returns 0: this driver executes requests only
    /// in [`BlockTarget::wait`], which hands out every completion it
    /// produces, so there is never one waiting here. The contract says
    /// `poll` never blocks, and executing a request is file I/O.
    fn poll(&mut self, _out: &mut Vec<Completion>) -> usize {
        0
    }

    /// Execute what is queued, in order, and return the completions.
    /// This is where the store's I/O happens, so it can take as long as
    /// a sync takes; it never sleeps, and returns 0 only when nothing
    /// is queued or the first request is held behind a hold, which only
    /// this thread can release.
    fn wait(&mut self, out: &mut Vec<Completion>, _timeout: Duration) -> usize {
        self.run(usize::MAX, out)
    }
}

/// What the owner of a [`Served`] store asks of its worker.
enum Command {
    Request(Request),
    Hold(Option<Lsn>, mpsc::Sender<Result<(), NotABoundary>>),
    Checkpoint(mpsc::Sender<Result<Lsn, BlkError>>),
    Position(mpsc::Sender<Lsn>),
    /// For tests: the worker panics.
    #[cfg(test)]
    Panic,
    /// For test rigs: fail the log now.
    #[cfg(any(test, feature = "test-hooks"))]
    Fail(mpsc::Sender<()>),
}

/// The store behind a worker thread.
///
/// The worker owns the log, the home image and the index, and executes
/// requests one at a time in the order of submission.
/// [`BlockTarget::submit`] hands a request to the worker and returns;
/// [`BlockTarget::poll`] takes the completions that have arrived and
/// [`BlockTarget::wait`] waits for the first. Neither does I/O. A write
/// that finds the log full behind a hold stays with the worker, and
/// everything behind it, until the hold is released.
///
/// Dropping it ends the worker after the request it is executing;
/// requests that were queued behind are not executed, as when the
/// process is lost.
pub struct Served {
    geometry: Geometry,
    identity: [u8; 16],
    commands: Option<mpsc::Sender<Command>>,
    completions: mpsc::Receiver<(Completion, Option<Lsn>)>,
    ids: BTreeSet<RequestId>,
    acknowledged: Option<Lsn>,
    /// Set by the worker as it ends, however it ends.
    ended: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

/// Says that the worker has ended, when the worker's closure is left:
/// by its return or by a panic. The flag is set before the owner is
/// woken, so an owner that is woken and looks finds it.
struct Ending {
    ended: Arc<AtomicBool>,
    waker: Option<Waker>,
}

impl Drop for Ending {
    fn drop(&mut self) {
        self.ended.store(true, Ordering::SeqCst);
        if let Some(Waker(wake)) = &self.waker {
            wake();
        }
    }
}

impl<D> Store<D>
where
    D: Dir + Send + 'static,
    D::File: Send,
    D::Lock: Send,
{
    /// Put the store behind a worker thread. Requests that are queued
    /// are executed first.
    pub fn serve(self) -> io::Result<Served> {
        self.serve_with(None)
    }

    /// As [`Store::serve`], with a waker that the worker calls after
    /// every completion it makes ready and when it ends.
    pub fn serve_with(self, waker: Option<Waker>) -> io::Result<Served> {
        let Store {
            mut core,
            acknowledged,
            mut queue,
            ids,
        } = self;
        let geometry = core.geometry;
        let identity = core.identity();
        let (commands, inbox) = mpsc::channel::<Command>();
        let (outbox, completions) = mpsc::channel::<(Completion, Option<Lsn>)>();
        let ended = Arc::new(AtomicBool::new(false));
        let ending = Ending {
            ended: ended.clone(),
            waker: waker.clone(),
        };
        let worker = thread::Builder::new()
            .name("skyzfs-store".into())
            .spawn(move || {
                let _ending = ending;
                // Whether the request at the head was tried and cannot
                // proceed until something changes.
                let mut held = false;
                loop {
                    // Take what has arrived; wait if there is nothing
                    // to do.
                    let command = if queue.is_empty() || held {
                        match inbox.recv() {
                            Ok(command) => Some(command),
                            Err(_) => return,
                        }
                    } else {
                        match inbox.try_recv() {
                            Ok(command) => Some(command),
                            Err(mpsc::TryRecvError::Empty) => None,
                            Err(mpsc::TryRecvError::Disconnected) => return,
                        }
                    };
                    match command {
                        Some(Command::Request(request)) => {
                            queue.push_back(request);
                            continue;
                        }
                        Some(Command::Hold(hold, reply)) => {
                            let _ = reply.send(core.set_hold(hold));
                            held = false;
                            continue;
                        }
                        Some(Command::Checkpoint(reply)) => {
                            let result = if core.failed {
                                Err(BlkError::Failed)
                            } else {
                                core.checkpoint()
                            };
                            held = false;
                            let _ = reply.send(result);
                            continue;
                        }
                        Some(Command::Position(reply)) => {
                            let _ = reply.send(core.log.tail());
                            continue;
                        }
                        #[cfg(test)]
                        Some(Command::Panic) => panic!("a test asked the worker to panic"),
                        #[cfg(any(test, feature = "test-hooks"))]
                        Some(Command::Fail(reply)) => {
                            // As a failed sync does: the volume has
                            // failed and the log is marked. A request
                            // that was held gets its answer now.
                            core.fail();
                            held = false;
                            let _ = reply.send(());
                            if let Some(Waker(wake)) = &waker {
                                wake();
                            }
                            continue;
                        }
                        None => {}
                    }
                    let Some(Request { id, op }) = queue.pop_front() else {
                        continue;
                    };
                    match core.execute(op) {
                        Ok((result, buf)) => {
                            let start = core.appended.take();
                            let completion = Completion { id, result, buf };
                            if outbox.send((completion, start)).is_err() {
                                return;
                            }
                            if let Some(Waker(wake)) = &waker {
                                wake();
                            }
                        }
                        Err(op) => {
                            queue.push_front(Request { id, op });
                            held = true;
                        }
                    }
                }
            })?;
        Ok(Served {
            geometry,
            identity,
            commands: Some(commands),
            completions,
            ids,
            acknowledged,
            ended,
            worker: Some(worker),
        })
    }
}

impl Served {
    fn send(&self, command: Command) -> bool {
        self.commands
            .as_ref()
            .is_some_and(|commands| commands.send(command).is_ok())
    }

    /// Set or release the hold: see [`Store::set_hold`]. It takes
    /// effect behind the request the worker is executing, and this
    /// waits for that. With the worker gone there is nothing to hold.
    pub fn set_hold(&self, hold: Option<Lsn>) -> Result<(), NotABoundary> {
        let (reply, result) = mpsc::channel();
        if !self.send(Command::Hold(hold, reply)) {
            return Ok(());
        }
        result.recv().unwrap_or(Ok(()))
    }

    /// Checkpoint, behind the request the worker is executing: see
    /// [`Store::checkpoint`]. Waits for it.
    pub fn checkpoint(&self) -> Result<Lsn, BlkError> {
        let (reply, result) = mpsc::channel();
        if !self.send(Command::Checkpoint(reply)) {
            return Err(BlkError::Failed);
        }
        result.recv().unwrap_or(Err(BlkError::Failed))
    }

    /// The end of the log as it is when the worker gets to the
    /// question: see [`Store::position`]. The worker answers before it
    /// executes requests that are still queued, so a request submitted
    /// just before may not be in it yet. `None` if the worker is gone.
    pub fn position(&self) -> Option<Lsn> {
        let (reply, result) = mpsc::channel();
        if !self.send(Command::Position(reply)) {
            return None;
        }
        result.recv().ok()
    }

    /// See [`Store::acknowledged`]: exact for what [`BlockTarget::poll`]
    /// and [`BlockTarget::wait`] have handed out, and never ahead of
    /// it, although the worker may have appended more.
    pub fn acknowledged(&self) -> Option<Lsn> {
        self.acknowledged
    }

    /// See [`Store::identity`].
    pub fn identity(&self) -> [u8; 16] {
        self.identity
    }

    /// Whether the worker has ended, which only a panic in it does
    /// while the store is served. Nothing more will complete: requests
    /// in flight stay in flight, and [`BlockTarget::submit`] answers
    /// "busy" from then on. The owner is told through the [`Waker`].
    pub fn gone(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }

    /// For test rigs, under the cargo feature `test-hooks`: fail the
    /// log now, as if its next sync had returned an error. It takes
    /// effect behind the request the worker is executing, and this
    /// waits for that; then the owner's waker is called. Every later
    /// request fails, and a later open finds the volume marked as
    /// failed. Requests completed before stay completed. With the
    /// worker gone there is nothing to fail.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn fail_now(&self) {
        let (reply, done) = mpsc::channel();
        if self.send(Command::Fail(reply)) {
            let _ = done.recv();
        }
    }

    /// For tests: make the worker panic, and wait until it has.
    #[cfg(test)]
    pub(crate) fn panic_worker(&mut self) {
        self.send(Command::Panic);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }

    fn took(&mut self, taken: (Completion, Option<Lsn>), out: &mut Vec<Completion>) {
        let (completion, start) = taken;
        self.ids.remove(&completion.id);
        if let Some(start) = start {
            self.acknowledged = Some(start);
        }
        out.push(completion);
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        // The worker ends when the commands' sender is gone.
        self.commands = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl BlockTarget for Served {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

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
        let id = request.id;
        match self.commands.as_ref() {
            Some(commands) => match commands.send(Command::Request(request)) {
                Ok(()) => {
                    self.ids.insert(id);
                    Ok(())
                }
                // The worker is gone, which only a panic in it does.
                Err(mpsc::SendError(Command::Request(request))) => Err(SubmitError {
                    kind: SubmitErrorKind::Busy,
                    request,
                }),
                Err(_) => Ok(()),
            },
            None => Err(SubmitError {
                kind: SubmitErrorKind::Busy,
                request,
            }),
        }
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        let mut taken = 0;
        while let Ok(completion) = self.completions.try_recv() {
            self.took(completion, out);
            taken += 1;
        }
        taken
    }

    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        let taken = self.poll(out);
        if taken > 0 || self.ids.is_empty() {
            return taken;
        }
        match self.completions.recv_timeout(timeout) {
            Ok(completion) => {
                self.took(completion, out);
                1 + self.poll(out)
            }
            Err(_) => 0,
        }
    }
}

#[cfg(test)]
mod harness;
#[cfg(test)]
mod tests;

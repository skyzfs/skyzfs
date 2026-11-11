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
//! [`BlockTarget::poll`] and [`BlockTarget::wait`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use skyzfs_blk::{
    Base, BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind, BLOCK_SIZE, BLOCK_USIZE, MAX_IO_BYTES, SIZE_UNIT,
};

use crate::format::{Identity, Key, Kind, Lsn};
use crate::io::{Dir, File};
use crate::log::{Log, LogError, Options, Recovery};

/// The home image's file.
pub const HOME_NAME: &str = "home.img";
/// The home image while it is being created.
const HOME_NEW_NAME: &str = "home.new";
/// The preimages of a checkpoint in progress: see [`Preimages`].
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

/// How a store is opened.
#[derive(Debug, Clone, Copy)]
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
            mutants: Mutants::default(),
            log_mutants: crate::log::Mutants::default(),
        }
    }
}

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
        if !self.log.is_failed() {
            self.log.mark_failed();
        }
        BlkError::Failed
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
        // the disk, and would report success on a disk that has died.
        // The contract lets no flush succeed once the log has failed,
        // so the store asks the disk every time. (The conservative
        // reading: such a flush would promise nothing false.)
        let synced = if self.log.durable() == self.log.tail() {
            self.log.probe()
        } else {
            self.log.sync().map(|_| ())
        };
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
        if replayed.is_err() || written.is_err() {
            return Err(self.fail());
        }
        let early = self.mutants.has(Mutants::RECLAIM_BEFORE_HOME_SYNC);
        if !early && self.home.sync().is_err() {
            return Err(self.fail());
        }
        // The home image is durable: the preimages have done their work,
        // and must be gone before the home image is written again.
        if let Some(file) = self.preimages.as_mut() {
            if Preimages::invalidate(file).is_err() {
                return Err(self.fail());
            }
        }
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
                if !covered && neighbours.last() != Some(&block) {
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
        let image = Preimages::encode(&volume, &neighbours, &blocks);
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
/// checkpoint began to rewrite their write units.
///
/// | Offset | Bytes | Field |
/// |---|---|---|
/// | 0 | 8 | magic `SKYZPRE\0` |
/// | 8 | 4 | version, 1 |
/// | 12 | 4 | number of blocks, `n` |
/// | 16 | 16 | volume id |
/// | 32 | 4 | CRC32C of the table |
/// | 36 | 472 | reserved |
/// | 508 | 4 | CRC32C of bytes 0..508 |
/// | 4096 | 16 `n` | the table: for each block its number (8), the CRC32C of its content (4), reserved (4) |
/// | 4096 + the table, rounded up to 4096 | 4096 `n` | the blocks |
///
/// Every integer is little-endian. The file is whole if the header, the
/// table and every block match their checksums; anything else is a file
/// that was being written, or one that was invalidated (its first
/// sector zeroed), and is ignored: the home image had not been touched.
struct Preimages;

const PREIMAGE_MAGIC: &[u8; 8] = b"SKYZPRE\0";
const PREIMAGE_VERSION: u32 = 1;
const PREIMAGE_HEADER: usize = 512;

impl Preimages {
    fn table_len(count: usize) -> usize {
        (count * 16).div_ceil(BLOCK_USIZE) * BLOCK_USIZE
    }

    /// The file's bytes for `numbers` and their content in `blocks`.
    fn encode(volume: &[u8; 16], numbers: &[u64], blocks: &[u8]) -> Vec<u8> {
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
            entry[8..12].copy_from_slice(&crc32c::crc32c(block).to_le_bytes());
        }
        head[..8].copy_from_slice(PREIMAGE_MAGIC);
        head[8..12].copy_from_slice(&PREIMAGE_VERSION.to_le_bytes());
        head[12..16].copy_from_slice(&(numbers.len() as u32).to_le_bytes());
        head[16..32].copy_from_slice(volume);
        head[32..36].copy_from_slice(&crc32c::crc32c(table).to_le_bytes());
        let crc = crc32c::crc32c(&head[..PREIMAGE_HEADER - 4]);
        head[PREIMAGE_HEADER - 4..PREIMAGE_HEADER].copy_from_slice(&crc.to_le_bytes());
        out
    }

    /// The blocks of a whole file, with their numbers; `None` for
    /// anything else.
    fn decode<F: File>(
        file: &F,
        volume: &[u8; 16],
        device_blocks: u64,
    ) -> io::Result<Option<Vec<(u64, Vec<u8>)>>> {
        let word = |bytes: &[u8]| {
            let mut raw = [0u8; 4];
            raw.copy_from_slice(&bytes[..4]);
            u32::from_le_bytes(raw)
        };
        let len = file.len()?;
        if len < BLOCK_SIZE {
            return Ok(None);
        }
        let mut head = [0u8; PREIMAGE_HEADER];
        file.read_at(0, &mut head)?;
        let count = word(&head[12..16]) as usize;
        let table_len = Self::table_len(count);
        let whole = (BLOCK_USIZE + table_len + count * BLOCK_USIZE) as u64;
        if &head[..8] != PREIMAGE_MAGIC
            || word(&head[PREIMAGE_HEADER - 4..]) != crc32c::crc32c(&head[..PREIMAGE_HEADER - 4])
            || word(&head[8..12]) != PREIMAGE_VERSION
            || &head[16..32] != volume
            || head[36..PREIMAGE_HEADER - 4].iter().any(|b| *b != 0)
            || len < whole
        {
            return Ok(None);
        }
        let mut table = vec![0u8; table_len];
        file.read_at(BLOCK_SIZE, &mut table)?;
        if word(&head[32..36]) != crc32c::crc32c(&table) {
            return Ok(None);
        }
        let mut out = Vec::with_capacity(count);
        for (i, entry) in table.chunks(16).take(count).enumerate() {
            let mut number = [0u8; 8];
            number.copy_from_slice(&entry[..8]);
            let number = u64::from_le_bytes(number);
            let mut block = vec![0u8; BLOCK_USIZE];
            file.read_at(
                (BLOCK_USIZE + table_len + i * BLOCK_USIZE) as u64,
                &mut block,
            )?;
            if number >= device_blocks || word(&entry[8..12]) != crc32c::crc32c(&block) {
                return Ok(None);
            }
            out.push((number, block));
        }
        Ok(Some(out))
    }

    /// Make the file say nothing, durably.
    fn invalidate<F: File>(file: &mut F) -> io::Result<()> {
        file.write_at(0, &[0u8; PREIMAGE_HEADER])?;
        file.sync()
    }

    /// At an open: if a checkpoint left its preimages whole, write them
    /// back into the home image, make that durable, and invalidate the
    /// file. A checkpoint that was cut off may have damaged these
    /// blocks; one that was not has left them as the file has them.
    fn restore<D: Dir>(
        dir: &D,
        home: &mut D::File,
        volume: &[u8; 16],
        device_blocks: u64,
    ) -> io::Result<Option<D::File>> {
        let mut file = match dir.open(PREIMAGE_NAME) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        if let Some(blocks) = Self::decode(&file, volume, device_blocks)? {
            for (number, block) in &blocks {
                home.write_at(number * BLOCK_SIZE, block)?;
            }
            home.sync()?;
        }
        if file.len()? >= PREIMAGE_HEADER as u64 {
            Self::invalidate(&mut file)?;
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
/// [`BlockTarget::poll`] and [`BlockTarget::wait`] execute what is
/// queued, in the order of submission, and return the completions: in
/// this driver they do the store's I/O and so can take as long as a
/// sync takes. A write that finds the log full behind a hold stays at
/// the head of the queue, with everything submitted after it behind it,
/// until the hold is released.
pub struct Store<D: Dir> {
    core: Core<D>,
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
            acknowledge_failure: false,
            mutants: options.log_mutants,
        };
        let log = Log::create_keyed(dir, identity, key, log_options)?;
        let home = log
            .dir()
            .open(HOME_NAME)
            .map_err(io_error("opening the home image"))?;
        Self::assemble(log, home, options)
    }

    /// Open the store in `dir`: recover the log, and rebuild the index
    /// from it.
    pub fn open(dir: D, options: OpenOptions) -> Result<(Self, Recovery), StoreError> {
        let log_options = Options {
            acknowledge_failure: options.acknowledge_failure,
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
        Ok((Self::assemble(log, home, options)?, recovery))
    }

    fn assemble(
        mut log: Log<D>,
        mut home: D::File,
        options: OpenOptions,
    ) -> Result<Self, StoreError> {
        let identity = *log.identity();
        let size = home.len().map_err(io_error("reading the home image"))?;
        if size != identity.device_size {
            return Err(StoreError::Damaged(
                "the home image is not of the volume's size",
            ));
        }
        // What a checkpoint that was cut off may have damaged beside
        // its own blocks, before anything reads the home image.
        let preimages = Preimages::restore(
            log.dir(),
            &mut home,
            &identity.volume,
            identity.device_size / BLOCK_SIZE,
        )
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
                index,
                geometry,
                failed: false,
                unsynced: Vec::new(),
                deferred: None,
                mutants: options.mutants,
            },
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
    /// returned. The hold is not stored: after an open it is set again
    /// before the first checkpoint.
    pub fn set_hold(&mut self, hold: Option<Lsn>) {
        self.core.log.set_hold(hold);
    }

    /// Whether the volume has failed.
    pub fn is_failed(&self) -> bool {
        self.core.failed
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

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.run(usize::MAX, out)
    }

    fn wait(&mut self, out: &mut Vec<Completion>, _timeout: Duration) -> usize {
        // Everything that can complete does so in `run`. What is left is
        // held behind a hold, which only this thread can release.
        self.run(usize::MAX, out)
    }
}

/// What the owner of a [`Served`] store asks of its worker.
enum Command {
    Request(Request),
    Hold(Option<Lsn>),
    Checkpoint(mpsc::Sender<Result<Lsn, BlkError>>),
    Position(mpsc::Sender<Lsn>),
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
    commands: Option<mpsc::Sender<Command>>,
    completions: mpsc::Receiver<Completion>,
    ids: BTreeSet<RequestId>,
    worker: Option<thread::JoinHandle<()>>,
}

impl<D> Store<D>
where
    D: Dir + Send + 'static,
    D::File: Send,
{
    /// Put the store behind a worker thread. Requests that are queued
    /// are executed first.
    pub fn serve(self) -> io::Result<Served> {
        let Store {
            mut core,
            mut queue,
            ids,
        } = self;
        let geometry = core.geometry;
        let (commands, inbox) = mpsc::channel::<Command>();
        let (outbox, completions) = mpsc::channel::<Completion>();
        let worker = thread::Builder::new()
            .name("skyzfs-store".into())
            .spawn(move || {
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
                        Some(Command::Hold(hold)) => {
                            core.log.set_hold(hold);
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
                        None => {}
                    }
                    let Some(Request { id, op }) = queue.pop_front() else {
                        continue;
                    };
                    match core.execute(op) {
                        Ok((result, buf)) => {
                            if outbox.send(Completion { id, result, buf }).is_err() {
                                return;
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
            commands: Some(commands),
            completions,
            ids,
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
    /// effect behind the request the worker is executing.
    pub fn set_hold(&self, hold: Option<Lsn>) {
        self.send(Command::Hold(hold));
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

    /// The end of the log, behind the request the worker is executing:
    /// see [`Store::position`]. `None` if the worker is gone.
    pub fn position(&self) -> Option<Lsn> {
        let (reply, result) = mpsc::channel();
        if !self.send(Command::Position(reply)) {
            return None;
        }
        result.recv().ok()
    }

    fn took(&mut self, completion: Completion, out: &mut Vec<Completion>) {
        self.ids.remove(&completion.id);
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

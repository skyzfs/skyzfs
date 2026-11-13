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
use std::ops::ControlFlow;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::thread;
use std::time::{Duration, Instant};

use skyzfs_blk::{
    Base, BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind, BLOCK_SIZE, BLOCK_USIZE, MAX_IO_BYTES, SIZE_UNIT,
};

use crate::format::siphash24;
use crate::format::{Identity, Key, Kind, Lsn};
use crate::io::{Dir, File, HelperEnded, NotBegun};
use crate::log::{Log, LogError, Observer, Options, Recovery};
use crate::writeback::{Hinted, Writeback};

/// The home image's file.
pub const HOME_NAME: &str = "home.img";
/// The home image while it is being created.
const HOME_NEW_NAME: &str = "home.new";

/// Every name a creation of a store that was interrupted can leave in
/// the directory, and so every name [`Store::create`] accepts in it:
/// the lock file, the home image under its first and its final name,
/// and what the log's creation leaves ([`crate::log::CREATE_LEFTOVERS`]).
/// A creation removes these, by these exact names, and no other file.
/// A directory that holds any other name is refused
/// ([`StoreError::NotEmpty`]) with nothing removed and nothing written:
/// the store cannot know whose `home.tar` or `log.txt` it is. A file
/// that somebody else put under exactly one of these names, in a
/// directory with no store, is removed as a leftover: the store cannot
/// tell it from one.
pub const CREATE_LEFTOVERS: [&str; 5] = [
    crate::fs::LOCK_NAME,
    HOME_NEW_NAME,
    HOME_NAME,
    crate::log::CREATE_LEFTOVERS[0],
    crate::log::CREATE_LEFTOVERS[1],
];

/// The first name in `names` that is not one of [`CREATE_LEFTOVERS`]:
/// what makes [`Store::create`] refuse a directory that holds no store.
pub fn stranger(names: &[String]) -> Option<&str> {
    names
        .iter()
        .map(String::as_str)
        .find(|name| !CREATE_LEFTOVERS.contains(name))
}
/// The preimages of a checkpoint in progress: see [the crate documentation](crate#the-preimage-file).
pub const PREIMAGE_NAME: &str = "home.pre";

/// Deliberate defects, for the tests that show each defence is needed.
/// Production code never sets one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u64);

#[allow(dead_code)]
impl Mutants {
    /// A checkpoint applies records that are not durable yet.
    pub(crate) const CHECKPOINT_UNSYNCED: u64 = 1 << 0;
    /// A checkpoint goes beyond the hold.
    pub(crate) const CHECKPOINT_PAST_HOLD: u64 = 1 << 1;
    /// A checkpoint gives up the log before the home image is synced.
    pub(crate) const RECLAIM_BEFORE_HOME_SYNC: u64 = 1 << 2;
    /// A checkpoint switches to the home image every block whose record
    /// is durable, applied or not.
    pub(crate) const INDEX_SWITCH_EARLY: u64 = 1 << 3;
    /// An open does not rebuild the index from the log.
    pub(crate) const NO_INDEX_REBUILD: u64 = 1 << 4;
    /// A flush is acknowledged without a sync of the log.
    pub(crate) const FLUSH_NO_SYNC: u64 = 1 << 5;
    /// A write is acknowledged before it is appended: the append is
    /// left for the next request.
    pub(crate) const ACK_BEFORE_APPEND: u64 = 1 << 6;
    /// Requests are served after the volume has failed.
    pub(crate) const SERVE_AFTER_FAILURE: u64 = 1 << 7;
    /// A read takes every block from the home image.
    pub(crate) const READ_HOME_ONLY: u64 = 1 << 8;
    /// A checkpoint keeps no preimages of the blocks it rewrites
    /// without writing them.
    pub(crate) const NO_PREIMAGES: u64 = 1 << 9;
    /// A checkpoint's preimages are not made durable before the home
    /// image is written.
    pub(crate) const PREIMAGES_UNSYNCED: u64 = 1 << 10;
    /// An open does not take the directory's lock.
    pub(crate) const NO_LOCK: u64 = 1 << 11;
    /// An open takes a preimage file for the current one whatever
    /// start of the log it was written for.
    pub(crate) const STALE_PREIMAGES: u64 = 1 << 12;
    /// An open does not write the preimages back.
    pub(crate) const NO_RESTORE: u64 = 1 << 13;
    /// A checkpoint does not look at the result of its writes into the
    /// home image.
    pub(crate) const IGNORE_HOME_WRITE: u64 = 1 << 14;
    /// A volume that fails on its own files does not mark the log.
    pub(crate) const FAIL_UNMARKED: u64 = 1 << 15;
    /// A read that fails on the store's files fails alone: the volume
    /// goes on serving.
    pub(crate) const READ_ERROR_SERVED: u64 = 1 << 16;
    /// A hold is taken at any position.
    pub(crate) const HOLD_UNCHECKED: u64 = 1 << 18;
    /// A neighbour counts as held by the log when the index names a
    /// record for it, durable or not, and is not saved.
    pub(crate) const UNSYNCED_COVERS: u64 = 1 << 19;
    /// The acknowledged position is the end of the record, which is
    /// the log's end, and not where the record starts.
    pub(crate) const ACKNOWLEDGED_IS_END: u64 = 1 << 20;
    /// A write-zeroes record is applied by writing zeros, and counts
    /// its length in a step's bound: the store before M1.3f.
    pub(crate) const ZEROES_WRITTEN: u64 = 1 << 21;
    /// A file system that cannot deallocate fails the volume.
    pub(crate) const NO_HOLES_FAILS: u64 = 1 << 22;
    /// A write-zeroes record counts no bytes in a step's bound also
    /// where its zeros are written.
    pub(crate) const WRITTEN_ZEROES_FREE: u64 = 1 << 23;
    /// A step goes on past the record at which the file system first
    /// refused to deallocate.
    pub(crate) const REFUSAL_ENDS_NO_STEP: u64 = 1 << 25;
    /// A store is created whatever the block size of the directory's
    /// file system.
    pub(crate) const CREATE_ON_ANY_BLOCKS: u64 = 1 << 26;
    /// A store is opened whatever the block size of the file systems
    /// its files are on.
    pub(crate) const OPEN_ON_ANY_BLOCKS: u64 = 1 << 27;
    /// A store is created and opened whatever the page size of the
    /// system it runs on.
    pub(crate) const UNDER_ANY_PAGES: u64 = 1 << 28;
    /// A step writes into the home image without waiting for the
    /// readers that are under way.
    pub(crate) const HOME_WRITTEN_UNBARRED: u64 = 1 << 29;
    /// A step's end does not wait for the readers that are under way:
    /// a segment file it freed can be used again while one of them
    /// still reads a record in it.
    pub(crate) const SLOT_REUSED_UNBARRED: u64 = 1 << 30;
    /// The reader answers a read of a record that is not durable yet
    /// without asking whether a sync of the log failed meanwhile.
    pub(crate) const UNFLUSHED_READ_UNCHECKED: u64 = 1 << 17;
    /// A barrier waits for a moment with no reader under way, and not
    /// only for the readers that were under way when it began.
    pub(crate) const BARRIER_WAITS_FOR_ALL: u64 = 1 << 31;
    /// The reader is told that the volume has failed before the log's
    /// marker is attempted.
    pub(crate) const FAILED_SHOWN_BEFORE_MARK: u64 = 1 << 24;
    /// The reader takes its pin after its lookup, and not before it:
    /// after the read's pause, where a test can hold it between the
    /// two.
    pub(crate) const PIN_AFTER_LOOKUP: u64 = 1 << 32;
    /// A step that handed the home image's sync over moves the log's
    /// start at once, before the helper reports, and takes the report
    /// later.
    pub(crate) const START_BEFORE_REPORT: u64 = 1 << 33;
    /// The rule of the marks takes the next step while one waits for
    /// its report: it writes into the home image while that sync is
    /// under way.
    pub(crate) const HOME_WRITTEN_BEFORE_REPORT: u64 = 1 << 34;
    /// A barrier waits until one of the readers that were under way
    /// when it began has left, and not for all of them.
    pub(crate) const BARRIER_WAITS_FOR_ONE: u64 = 1 << 35;
    /// A read pushed to the readers' queue wakes every waiting reader,
    /// and not one.
    pub(crate) const READ_WAKES_ALL: u64 = 1 << 36;
    /// A reader that finds the readers' queue empty lets go of its lock
    /// before it waits: a read pushed between the two wakes nobody.
    pub(crate) const READ_WAKE_LOST: u64 = 1 << 37;
    /// A serve that cannot start a later thread leaves the readers it
    /// started as they are: nothing closes their queue.
    pub(crate) const STARTED_READERS_LEFT: u64 = 1 << 38;

    fn has(self, flag: u64) -> bool {
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
    /// Hand the log's writes to the device once this many bytes of them
    /// wait, before the sync that must wait for them (see
    /// [`crate::writeback`]). 0 is never. A hint: nothing that is
    /// durable, or when, changes.
    pub writeback_unit: u64,
    /// The same for the home image's writes in a step. 0, never, unless
    /// set: the step hands the home image's sync to a helper at once
    /// ([`OpenOptions::hand_over_home_sync`]), and the hint's calls
    /// would be time of the worker's that the helper's sync does not
    /// need.
    pub home_writeback_unit: u64,
    /// Whether a step of the rule of the marks hands the home image's
    /// sync over and goes on serving until it is reported (design
    /// §5.1). A served store always does; a store that is not served
    /// does if this is set (for tests: the simulated disk then ends the
    /// sync at a point its seed chooses).
    pub hand_over_home_sync: bool,
    /// How many reader threads a served store has (design §5.1 "Reads
    /// leave the worker"): at least one, and 0 counts as one. Each
    /// executes reads on handles of its own, beside the worker and
    /// beside each other, so that a read does not wait behind another
    /// read's file read. A store that is not served has none and is
    /// unchanged by this.
    pub readers: usize,
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
            writeback_unit: crate::writeback::DEFAULT_UNIT,
            home_writeback_unit: 0,
            hand_over_home_sync: false,
            readers: 1,
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
    /// `create`: the directory holds a file that no creation leaves
    /// ([`CREATE_LEFTOVERS`]). Nothing was removed or written, not even
    /// the lock file.
    #[error("the directory holds a file that is not a store's")]
    NotEmpty,
    /// The store's files do not fit together.
    #[error("the store is damaged: {0}")]
    Damaged(&'static str),
    /// `create` and every open: a file system under the store has
    /// blocks larger than the store's write unit. Nothing was written.
    ///
    /// The write unit is the store's one assumption about how far a
    /// write reaches beside what it writes: the log never writes again
    /// into a unit that holds a durable record, and a checkpoint saves
    /// the other blocks of each unit it writes in part. The files are
    /// written through the operating system's cache, and a file system
    /// writes back whole blocks: where its block is larger than the
    /// unit, a write rewrites sectors beyond its unit, which a power
    /// cut can then lose with nothing to restore them from.
    #[error(
        "{what} is on a file system with blocks of {block} bytes, larger than the volume's \
         write unit of {unit} bytes: a write there reaches beyond what the store protects"
    )]
    Blocks {
        /// What is there.
        what: Under,
        /// The file system's block size.
        block: u64,
        /// The store's write unit.
        unit: u64,
    },
    /// `create` and every open: the system's page is larger than the
    /// store's write unit. Nothing was written. A kernel writes back
    /// whole pages, so this is the same exposure by the other measure:
    /// a volume made where pages are 4 KiB and opened where they are
    /// 16 or 64 KiB.
    #[error(
        "this system's pages are {page} bytes, larger than the volume's write unit of {unit} \
         bytes: a write here reaches beyond what the store protects"
    )]
    Pages {
        /// The system's page size.
        page: u64,
        /// The store's write unit.
        unit: u64,
    },
    /// The same, where the block size could not be read: the
    /// assumption cannot be checked, and the store is refused.
    #[error(
        "the block size of the file system under {what} could not be read ({kind:?}): \
         whether a write there stays within the volume's write unit cannot be checked"
    )]
    BlocksUnknown {
        /// What is there.
        what: Under,
        /// The error's kind.
        kind: io::ErrorKind,
    },
}

/// What of a store a file system's block size was read for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Under {
    /// The volume's directory: where every file of the store is made.
    Directory,
    /// The home image.
    Home,
    /// A segment file of the write log, by its number: `log.000003`
    /// is 3.
    Segment(usize),
    /// Another file of the write log: its superblock, its marker.
    Log,
    /// The file of preimages.
    Preimages,
}

impl Under {
    /// What the store's file of this name is, or `None` for a name
    /// under which the store writes nothing: its lock, and whatever
    /// else is in the directory.
    fn of(name: &str) -> Option<Self> {
        if name == HOME_NAME || name == HOME_NEW_NAME {
            Some(Self::Home)
        } else if name == PREIMAGE_NAME {
            Some(Self::Preimages)
        } else if let Some(index) = crate::log::slot_index(name) {
            Some(Self::Segment(index))
        } else if name.starts_with("log.") {
            Some(Self::Log)
        } else {
            None
        }
    }
}

impl std::fmt::Display for Under {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Directory => "the volume's directory",
            Self::Home => "the volume's home image (home.img)",
            Self::Preimages => "the volume's file of preimages (home.pre)",
            Self::Segment(index) => return write!(f, "the volume's log file log.{index:06}"),
            Self::Log => "the superblock or the marker of the volume's write log",
        })
    }
}

/// Refuse a system whose page is larger than `unit`.
fn pages_within_unit(page: u64, unit: u64, mutants: Mutants) -> Result<(), StoreError> {
    if page <= unit || mutants.has(Mutants::UNDER_ANY_PAGES) {
        Ok(())
    } else {
        Err(StoreError::Pages { page, unit })
    }
}

/// Refuse a block size that is not at most `unit`.
fn within_unit(what: Under, block: io::Result<u64>, unit: u64) -> Result<(), StoreError> {
    match block {
        Ok(block) if block <= unit => Ok(()),
        Ok(block) => Err(StoreError::Blocks { what, block, unit }),
        Err(e) => Err(StoreError::BlocksUnknown {
            what,
            kind: e.kind(),
        }),
    }
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
    /// Whether no block's newest version is in the log: a checkpoint
    /// has nothing to apply.
    fn is_empty(&self) -> bool {
        self.extents.is_empty()
    }

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

/// What a read looks at before it reads a file: the index, and for
/// every segment of the log the number of the file that holds it.
#[derive(Debug, Default)]
struct Lookup {
    index: Index,
    /// The log's segments as the index needs them: by first position,
    /// the file's number ([`crate::log::slot_name`]). A segment the
    /// log has given up stays here until the index lets go of its
    /// records, in the same hold of the lock.
    segments: BTreeMap<u64, usize>,
    /// [`Log::segments_changed`] when `segments` was taken.
    changed: Option<u64>,
}

impl Lookup {
    /// Take the log's segments again if they have changed.
    fn follow<D: Dir>(&mut self, log: &Log<D>) {
        let changed = Some(log.segments_changed());
        if self.changed != changed {
            self.segments = log.segments();
            self.changed = changed;
        }
    }
}

/// What the store's worker shares with whoever reads beside it (the
/// reader threads of a served store, design §5.1 "Reads leave the
/// worker").
///
/// What follows holds for each reader, whatever their number
/// ([`OpenOptions::readers`]): each takes its own pin before its own
/// lookup, holds the lookup's lock for its lookup alone, and a barrier
/// waits for the pin of every reader that was under way when it began.
/// Readers never wait for each other but at the lock of the lookup,
/// which they share, and at the queue of reads, where an idle reader
/// takes the next.
///
/// - `lookup` is held by a reader for the lookup alone, never across
///   a read of a file, and by the worker for an insert and for the
///   drop at a step's end.
/// - `pin` is held shared by a reader from just before its lookup to
///   the end of its last read of a file. The worker takes it
///   exclusively for an instant, as a barrier that waits for the
///   readers under way, at two points of a step and nowhere else:
///   before the step's first write into the home image, and after the
///   index has let go of the batch, before a segment file the step
///   freed can be used again.
/// - `failed` says that the volume has failed, to a reader that
///   begins a read.
/// - `durable` is for a read of a record that is not durable yet.
///   After a failed sync the page cache may show a file as it was
///   before the writes the sync was to cover, so such a record's
///   payload can read as something else from the moment a sync of the
///   log begins to fail, which is before anybody knows that it failed.
///   A reader that finds a record at or beyond `durable` (as it is at
///   the lookup) reads the record's whole payload and checks it
///   against the header's checksum: a payload that matches is the one
///   that was written, and is served whatever the sync does
///   afterwards (rule R11 lets a request in flight at a failure
///   complete either way). One that does not match is a log that
///   cannot be read: the worker fails the volume and answers the read,
///   which is also what a sync that is failing comes to. Records below
///   `durable` and the home image are not exposed to this: a failed
///   sync takes back only what no sync had covered, and what a step
///   writes into the home image is read from the log until the step's
///   end.
///
/// A read's whole range is looked up under one hold of `lookup`, and a
/// mutation's whole range is entered under one: a read shows a
/// mutation whole or not at all, and for a block the index only moves
/// forward (to a newer record, or to the home image once that holds
/// the newest), so a write that a read has shown is shown by every
/// later read.
#[derive(Debug, Default)]
struct Shared {
    /// With several readers, the worker's insert and drop must not
    /// wait behind a stream of lookups: this rests on a waiting writer
    /// being let in before new readers, which the standard library's
    /// lock does on Linux (its futex lock holds new readers back while
    /// a writer waits).
    lookup: RwLock<Lookup>,
    pin: Pins,
    failed: AtomicBool,
    /// Up to where the log is durable, as the worker last knew.
    durable: AtomicU64,
}

/// The pin of design §5.1: readers enter it before they look up and
/// leave it after their last read of a file; a barrier waits for the
/// readers that were under way when it began.
///
/// Readers enter under a generation, and a barrier begins a new one
/// and waits until no reader of an older generation is under way. A
/// reader that enters while a barrier waits is of the new generation
/// and is not waited for, so reads that keep arriving, overlapping each
/// other, never hold a step: the barrier waits for as long as the
/// slowest read that was under way when it began, and no longer. This
/// rests on nothing a lock promises about who goes first.
#[derive(Debug, Default)]
struct Pins {
    state: Mutex<PinState>,
    left: std::sync::Condvar,
}

#[derive(Debug, Default)]
struct PinState {
    /// The generation a reader that enters now is of.
    generation: u64,
    /// The readers under way, by the generation they entered under.
    under_way: BTreeMap<u64, usize>,
    /// For tests: how many barriers found a reader to wait for, so
    /// that a test waits for a step to be at its barrier, not for a
    /// time.
    #[cfg(test)]
    waited: u64,
    /// For tests: how often a barrier woke from its wait and looked
    /// again, and how many barriers that waited have passed.
    #[cfg(test)]
    woke: u64,
    #[cfg(test)]
    passed: u64,
}

/// A reader's pin: it leaves when this is dropped.
struct Pin<'a> {
    pins: &'a Pins,
    generation: u64,
}

impl Pins {
    fn state(&self) -> std::sync::MutexGuard<'_, PinState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn enter(&self) -> Pin<'_> {
        let mut state = self.state();
        let generation = state.generation;
        *state.under_way.entry(generation).or_default() += 1;
        Pin {
            pins: self,
            generation,
        }
    }

    /// Wait for the readers that are under way now. With `all` (a
    /// defect, for tests), wait for a moment with no reader at all;
    /// with `one` (another), until one of them has left.
    fn barrier(&self, all: bool, one: bool) {
        let mut state = self.state();
        let before = state.generation;
        state.generation += 1;
        let older = |state: &PinState| -> usize {
            state
                .under_way
                .iter()
                .filter(|(generation, _)| all || **generation <= before)
                .map(|(_, readers)| *readers)
                .sum()
        };
        let at_first = older(&state);
        #[cfg(test)]
        let mut counted = false;
        while older(&state) > 0 && !(one && older(&state) < at_first) {
            #[cfg(test)]
            if !counted {
                state.waited += 1;
                counted = true;
            }
            state = self
                .left
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            #[cfg(test)]
            {
                state.woke += 1;
            }
        }
        #[cfg(test)]
        if counted {
            state.passed += 1;
        }
    }

    /// For tests: how many barriers have waited for a reader.
    #[cfg(test)]
    fn waited(&self) -> u64 {
        self.state().waited
    }

    /// For tests: how often a barrier woke and looked again, and how
    /// many barriers that waited have passed.
    #[cfg(test)]
    fn woke_and_passed(&self) -> (u64, u64) {
        let state = self.state();
        (state.woke, state.passed)
    }
}

impl Drop for Pin<'_> {
    fn drop(&mut self) {
        let mut state = self.pins.state();
        if let Some(readers) = state.under_way.get_mut(&self.generation) {
            *readers -= 1;
            if *readers == 0 {
                state.under_way.remove(&self.generation);
            }
        }
        drop(state);
        self.pins.left.notify_all();
    }
}

impl Shared {
    fn read(&self) -> RwLockReadGuard<'_, Lookup> {
        self.lookup.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write(&self) -> RwLockWriteGuard<'_, Lookup> {
        self.lookup.write().unwrap_or_else(PoisonError::into_inner)
    }

    /// What a reader holds while it looks up and reads.
    fn pinned(&self) -> Pin<'_> {
        self.pin.enter()
    }

    /// Wait for the readers that are under way. A reader that begins
    /// after this began looks up after it.
    fn barrier(&self, mutants: Mutants) {
        self.pin.barrier(
            mutants.has(Mutants::BARRIER_WAITS_FOR_ALL),
            mutants.has(Mutants::BARRIER_WAITS_FOR_ONE),
        );
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
    /// The home image's writes since its last sync, and the hint that
    /// hands them to the device early.
    home_writeback: Writeback,
    /// The preimage file, once a checkpoint has needed one.
    preimages: Option<D::File>,
    /// The directory's lock, held for as long as the store lives.
    _lock: Option<D::Lock>,
    /// The index, and what else a reader beside the worker needs.
    shared: Arc<Shared>,
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
    /// When the store checkpoints by itself, if it does: see
    /// [`Store::set_marks`].
    marks: Option<Marks>,
    /// The log was found at or above the high mark and is not yet
    /// below the low one: steps are due.
    working: bool,
    /// The steps the rule has taken.
    steps: u64,
    /// The times the rule made the log durable itself, with no flush
    /// of the client's, to have something to take.
    own_syncs: u64,
    /// Where the time went: see [`Timing`].
    timing: Timing,
    /// For a served store: the parts of the batches applied since the
    /// worker last took them, each with its begin and its end, so that
    /// the worker can say what it was doing when. `None` for a store
    /// that is not served, which has nobody to take them.
    parts: Option<Vec<(Instant, Instant, usize)>>,
    /// Whether the rule's steps hand the home image's sync over:
    /// [`OpenOptions::hand_over_home_sync`], and always when served.
    hands_over: bool,
    /// The step whose home-image sync is with the helper and has not
    /// been reported yet. Until it is, no step is begun and nothing is
    /// written to the home image: see [`Core::hand_over`].
    handed: Option<Handed>,
    /// For a served store: called when the helper has synced the home
    /// image, or has ended, so that a worker that waits for work wakes.
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// The helper ended without saying how its sync went. A served
    /// store's worker ends with it: the target's end.
    helper_ended: bool,
    /// How a write-zeroes record reaches the home image.
    zeroes: Zeroes,
    /// For test rigs: how and where every batch of a checkpoint waits.
    #[cfg(any(test, feature = "test-hooks"))]
    pause: Option<(StepPause, StepPoint)>,
    /// For test rigs: the store behaves as on a file system that
    /// cannot deallocate.
    #[cfg(any(test, feature = "test-hooks"))]
    refuse_holes: bool,
    /// For test rigs: how the reader of a served store waits after
    /// every lookup.
    #[cfg(any(test, feature = "test-hooks"))]
    read_pause: Option<StepPause>,
    /// For test rigs: how a read that begins at a given block waits
    /// after its lookup, as [`Core::read_pause`] for every read.
    #[cfg(any(test, feature = "test-hooks"))]
    read_pauses_at: Vec<(u64, StepPause)>,
    /// How many reader threads the store has when it is served.
    readers: usize,
    /// For tests: the thread that [`Store::serve_with`] cannot start,
    /// by its place in the order it starts them (the readers from 0,
    /// then the worker).
    #[cfg(test)]
    spawn_fails_at: Option<usize>,
    /// For tests: how many of this store's reader threads have ended.
    #[cfg(test)]
    readers_ended: Arc<std::sync::atomic::AtomicUsize>,
    /// For test rigs: how a sync of the log for a flush waits before
    /// it begins.
    #[cfg(any(test, feature = "test-hooks"))]
    sync_pause: Option<StepPause>,
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
            Op::Flush => Ok((self.flush(SyncKind::FLUSH), None)),
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
    ///
    /// The reader beside the worker is told only after the marker was
    /// attempted (design §5.1): it answers a read as failed on its own
    /// when it finds the flag, and no read may be answered as failed
    /// before the next instance can find the mark. A read that begins
    /// while the marker is written is served, or checked, as any read
    /// in flight at the failure (rule R11). The log's own failures mark
    /// before the log call returns, and [`Core::with_log`] tells the
    /// reader after it returns.
    fn fail(&mut self) -> BlkError {
        self.failed = true;
        let early = self.mutants.has(Mutants::FAILED_SHOWN_BEFORE_MARK);
        if early {
            self.shared.failed.store(true, Ordering::SeqCst);
        }
        if !self.log.is_failed() && !self.mutants.has(Mutants::FAIL_UNMARKED) {
            self.log.mark_failed();
        }
        self.shared.failed.store(true, Ordering::SeqCst);
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
        // A step whose sync is with the helper puts back the hold it was
        // chosen under when it is taken up: it ends first. A sync that
        // failed has failed the volume, and the hold changes all the
        // same.
        let _ = self.settle();
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
            match self.with_log(|log| log.append(kind, offset, len, data)) {
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
        // The index points at the record before the caller
        // acknowledges it: a read that is submitted after the
        // acknowledgement looks up after this.
        let mut lookup = self.shared.write();
        lookup.follow(&self.log);
        match kind {
            Kind::Write => lookup.index.insert(
                start,
                blocks,
                Source::Data {
                    record: appended.start,
                    first: 0,
                },
            ),
            Kind::WriteZeroes => lookup.index.insert(
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
        drop(lookup);
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
    /// `why` is the kind of sync it is ([`SYNC_KINDS`]), under which
    /// what it had to wait for is counted.
    fn flush(&mut self, why: usize) -> Result<(), BlkError> {
        if self.mutants.has(Mutants::FLUSH_NO_SYNC) {
            return Ok(());
        }
        // With nothing to make durable the log's sync does not touch
        // the disk, and such a flush promises nothing new: a disk that
        // would refuse the next sync and has not been asked has not
        // failed yet (design §4.1).
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some(pause) = &self.sync_pause {
            pause.wait();
        }
        let (before, began) = (self.log.backlog(), Instant::now());
        let synced = self.with_log(|log| log.sync().map(|_| ()));
        match synced {
            Ok(()) => {
                let after = self.log.backlog();
                self.timing.syncs[why].add(
                    began.elapsed(),
                    after.bytes - before.bytes,
                    after.handed - before.handed,
                );
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
            self.shared.read().index.lookup(start, blocks)
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
        self.flush(SyncKind::ROOM)?;
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
        // A step's sync that is with the helper ends first: the log's
        // start is that step's to move.
        self.settle()?;
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
        let walk_began = Instant::now();
        let ends = if per_unit <= 1 {
            vec![target]
        } else {
            match self.batches(floor, target, limit, per_unit) {
                Ok(ends) => ends,
                Err(()) => return Err(self.fail()),
            }
        };
        self.walked(walk_began);
        let mut start = floor;
        for end in ends {
            let (reached, cut) = self.apply(start, end, limit, hold)?;
            start = reached;
            // The batch ended where the file system first refused to
            // deallocate: its rest is a batch of its own, and that one
            // cannot end early, because the store now writes zeros.
            if cut.is_some() {
                start = self.apply(start, end, limit, hold)?.0;
            }
        }
        Ok(start)
    }

    /// One step of a checkpoint: the first records of what a checkpoint
    /// would take, as many as stay within `bound` and within one batch
    /// of preimages, and at least one. They are applied to the home
    /// image and that part of the log is given up, in the order of
    /// [`Core::checkpoint`]: a step is one of its batches, cut shorter.
    ///
    /// The step reads the log only as far as it takes, so its cost does
    /// not grow with the log behind it.
    fn checkpoint_step(&mut self, bound: StepBound) -> Result<Step, BlkError> {
        self.settle()?;
        self.step(bound, false)
    }

    /// A step as [`Core::checkpoint_step`] takes it, with its home
    /// image's sync handed over if `hand_over` and the store does
    /// ([`Core::apply_handing_over`]): the step is then taken up when
    /// the helper reports, and the start it returns is where the log
    /// will start.
    fn step(&mut self, bound: StepBound, hand_over: bool) -> Result<Step, BlkError> {
        let floor = self.log.floor();
        let limit = self.log.durable();
        let hold = self.log.hold();
        let target = hold.map_or(limit, |hold| limit.min(hold));
        let nothing = Step {
            start: floor,
            records: 0,
            payload: 0,
            more: false,
        };
        if target <= floor {
            return Ok(nothing);
        }
        let per_unit = self.log.identity().unit() / BLOCK_SIZE;
        // The step's first part: reading the log to choose the batch.
        let walk_began = Instant::now();
        let mut end = target;
        let (mut saved, mut records, mut payload) = (0u64, 0u64, 0u64);
        let free_zeroes = !self.mutants.has(Mutants::ZEROES_WRITTEN)
            && (self.zeroes == Zeroes::Deallocated
                || self.mutants.has(Mutants::WRITTEN_ZEROES_FREE));
        let scanned = self.log.replay_until(floor, target, limit, &mut |record| {
            // Every record counts as one, so that the step's walk is
            // bounded whatever the log holds. Only what the step
            // writes into the home image counts in bytes: a discard
            // writes nothing there, and neither does a write-zeroes
            // whose range is deallocated. Until the file system has
            // shown that it deallocates, and where it has refused, a
            // write-zeroes counts its whole length, which is what is
            // then written.
            let writes = matches!(record.kind, Kind::Write | Kind::WriteZeroes);
            let length = match record.kind {
                Kind::Write => record.length,
                Kind::WriteZeroes if !free_zeroes => record.length,
                _ => 0,
            };
            // The blocks to save beside it: at most the rest of its
            // first and of its last unit.
            let first = record.offset / BLOCK_SIZE;
            let last = first + record.length / BLOCK_SIZE;
            let own = if per_unit <= 1 || !writes {
                0
            } else {
                first % per_unit + (last.div_ceil(per_unit) * per_unit - last)
            };
            let full = payload + length > bound.payload
                || records >= bound.records
                || saved + own > PREIMAGE_BATCH as u64;
            if records > 0 && full {
                end = record.start;
                return ControlFlow::Break(());
            }
            saved += own;
            records += 1;
            payload += length;
            ControlFlow::Continue(())
        });
        if scanned.is_err() {
            return Err(self.fail());
        }
        self.walked(walk_began);
        let (start, cut) = if hand_over {
            self.apply_handing_over(floor, end, limit, hold)?
        } else {
            self.apply(floor, end, limit, hold)?
        };
        // A step that ended early, at the record where the file system
        // first refused to deallocate, says what it took.
        Ok(match cut {
            Some(cut) => Step {
                start,
                records: cut.records,
                payload: cut.payload,
                more: cut.end < target,
            },
            None => Step {
                start,
                records,
                payload,
                more: end < target,
            },
        })
    }

    /// The log's state, for whoever shows it.
    fn log_state(&self) -> LogState {
        LogState {
            start: self.log.floor().0,
            end: self.log.tail().0,
            durable: self.log.durable().0,
            steps: self.steps,
            own_syncs: self.own_syncs,
            stepping: false,
            zeroes: self.zeroes,
        }
    }

    /// For test rigs: wait here if the hook is set for `point`.
    #[cfg(any(test, feature = "test-hooks"))]
    fn pause_at(&self, point: StepPoint) {
        if let Some((pause, at)) = &self.pause {
            if *at == point {
                pause.wait();
            }
        }
    }

    /// Whether the log's live size is at or above the high mark of a
    /// volume that has marks and has not failed: where no request is
    /// executed while the rule of the marks still finds a step to take
    /// ([`Marks`]).
    fn at_the_high_mark(&self) -> bool {
        let live = self.log.tail().0 - self.log.floor().0;
        !self.failed && self.marks.is_some_and(|marks| live >= marks.high)
    }

    /// The walk of the log that chose a batch, or a checkpoint's
    /// batches, is over: it began at `began`.
    fn walked(&mut self, began: Instant) {
        let ended = Instant::now();
        self.timing.step_walk.add(ended - began);
        self.part(began, ended, Activity::STEP_WALK);
    }

    /// A barrier of a step has been passed, where the step waited for
    /// the readers under way since `since`: the one before a segment
    /// file can be used again if `reuse`, else the one before the home
    /// image is written. Returns when it was passed.
    fn barred(&mut self, since: Instant, reuse: bool) -> Instant {
        let passed = Instant::now();
        let barrier = if reuse {
            &mut self.timing.barrier_reuse
        } else {
            &mut self.timing.barrier_home
        };
        barrier.add(passed - since);
        self.part(since, passed, Activity::STEP_BARRIER);
        passed
    }

    /// Call the log where it can sync a segment file (an append, which
    /// seals a segment at a roll, and a sync), and tell the readers
    /// beside the worker up to where it is durable now: see [`Shared`].
    /// The durable position is told only after the sync that made it
    /// so has returned. A log that failed in the call has attempted its
    /// marker before it returned (`Log::fail`), so the reader is told
    /// of the failure after the marker, as in [`Core::fail`].
    fn with_log<T>(&mut self, call: impl FnOnce(&mut Log<D>) -> T) -> T {
        let done = call(&mut self.log);
        if self.log.is_failed() {
            self.shared.failed.store(true, Ordering::SeqCst);
        }
        self.shared
            .durable
            .store(self.log.durable().0, Ordering::SeqCst);
        self.note_log();
        done
    }

    /// What the log counts of the hint and of its superblock's syncs,
    /// into the times.
    fn note_log(&mut self) {
        self.timing.writeback[0] = self.log.writeback();
        self.timing.writeback[1] = self.home_writeback.done;
        let backlog = self.log.backlog();
        self.timing.syncs[SyncKind::SUPERBLOCK] = Synced {
            count: backlog.superblock_syncs,
            sum_ns: backlog.superblock_ns,
            bytes: backlog.superblock_bytes,
            handed: 0,
        };
    }

    /// The home image's sync, begun at `began`: what it had to wait
    /// for is counted.
    fn sync_home(&mut self, began: Instant) -> io::Result<()> {
        self.home.sync()?;
        let (bytes, handed) = self.home_writeback.synced();
        self.timing.syncs[SyncKind::HOME].add(began.elapsed(), bytes, handed);
        self.timing.writeback[1] = self.home_writeback.done;
        Ok(())
    }

    /// Note a part of a batch for the worker, if there is one.
    fn part(&mut self, began: Instant, ended: Instant, activity: usize) {
        if let Some(parts) = &mut self.parts {
            parts.push((began, ended, activity));
        }
    }

    /// The rule of the marks after a request, for a store that is not
    /// served: one step, and further steps for as long as the log is
    /// at or above the high mark and a step took something. The worker
    /// of a served store does the same between two requests.
    fn tend_after_a_request(&mut self) {
        // A step that fails has failed the volume, which the next
        // request is told.
        while self.tend(&mut || {}) == Ok(true) && self.at_the_high_mark() {}
    }

    /// The rule of the marks, once: if the log's live size has reached
    /// the high mark and is not yet below the low one, take one step,
    /// after making the log durable if less than a step of it is and
    /// more could be. `before` is called once, before the step and
    /// before that sync if there is one: the rule's own sync is the
    /// beginning of its step, and can take as long as any sync.
    /// Returns whether a step was taken: if so the rule wants to be
    /// asked again.
    ///
    /// The sync is the log's, as a flush's is, and is nobody's request:
    /// it has no place in the volume's order.
    ///
    /// A store that hands the home image's sync over begins a step and
    /// returns: until the helper reports, the rule takes no step, and
    /// at the high mark it waits for the report and takes the step up
    /// (which counts as a step taken: the rule wants to be asked
    /// again).
    fn tend(&mut self, before: &mut dyn FnMut()) -> Result<bool, BlkError> {
        let Some(marks) = self.marks else {
            return Ok(false);
        };
        if self.failed {
            return Ok(false);
        }
        // No step while one waits for its report, unless the log is at
        // the high mark: then the report is waited for, which is the
        // time the worker would have spent in the sync itself, and the
        // log's bound is what it was.
        if self.handed.is_some() && !self.mutants.has(Mutants::HOME_WRITTEN_BEFORE_REPORT) {
            if !self.at_the_high_mark() {
                return Ok(false);
            }
            self.settle()?;
            return Ok(true);
        }
        let floor = self.log.floor();
        let live = self.log.tail().0 - floor.0;
        if live >= marks.high {
            self.working = true;
        } else if live < marks.low {
            self.working = false;
        }
        if !self.working {
            return Ok(false);
        }
        let hold = self.log.hold();
        let reach = |durable: Lsn| hold.map_or(durable, |hold| durable.min(hold));
        let durable = self.log.durable();
        let takeable = reach(durable).0.saturating_sub(floor.0);
        // A sync helps only if the hold is not what stands in the way.
        let more_if_synced = reach(self.log.tail()) > reach(durable);
        let syncs = takeable < marks.step.payload && more_if_synced;
        if syncs {
            // More becomes durable below the hold, so a step follows
            // unless the sync fails the volume.
            before();
            let began = Instant::now();
            let synced = self.flush(SyncKind::OWN);
            let ended = Instant::now();
            self.timing.step_own_sync.add(ended - began);
            self.part(began, ended, Activity::OWN_SYNC);
            synced?;
            self.own_syncs += 1;
        }
        if reach(self.log.durable()) <= floor {
            return Ok(false);
        }
        if !syncs {
            before();
        }
        let step = self.step(marks.step, self.hands_over)?;
        self.steps += 1;
        // Below the low mark the rule rests until the high one.
        if self.log.tail().0 - step.start.0 < marks.low {
            self.working = false;
        }
        Ok(true)
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
    /// log starts, to `target`, with the home image's sync made here.
    fn apply(
        &mut self,
        floor: Lsn,
        target: Lsn,
        limit: Lsn,
        hold: Option<Lsn>,
    ) -> Result<(Lsn, Option<Cut>), BlkError> {
        let applied = self.apply_writes(floor, target, limit, hold)?;
        let written_at = applied.written_at;
        let early = self.mutants.has(Mutants::RECLAIM_BEFORE_HOME_SYNC);
        if !early && self.sync_home(written_at).is_err() {
            return Err(self.fail());
        }
        let synced_at = Instant::now();
        self.timing.step_home_sync.add(synced_at - written_at);
        self.part(written_at, synced_at, Activity::STEP_HOME_SYNC);
        self.finish(applied, synced_at, early)
    }

    /// A batch of a step of the rule's whose home-image sync goes to
    /// the helper: its writes as [`Core::apply`]'s, then the sync is
    /// begun and the worker goes on. Until the helper reports
    /// ([`Core::report`]) no step is begun and nothing is written to
    /// the home image: what the sync makes durable is what was written
    /// before it began, and the preimage file holds this batch's
    /// neighbours until the step is over. The log's start stays where
    /// it is until then, so the log still holds every record of the
    /// batch, and a start applies them again. Returns where the log
    /// will start, and whether the batch was cut short. A sync that has
    /// ended at once is taken up here.
    fn apply_handing_over(
        &mut self,
        floor: Lsn,
        target: Lsn,
        limit: Lsn,
        hold: Option<Lsn>,
    ) -> Result<(Lsn, Option<Cut>), BlkError> {
        let early = self.mutants.has(Mutants::RECLAIM_BEFORE_HOME_SYNC);
        if early || !self.hands_over || self.handed.is_some() {
            return self.apply(floor, target, limit, hold);
        }
        let applied = self.apply_writes(floor, target, limit, hold)?;
        let (target, cut, written_at) = (applied.target, applied.cut, applied.written_at);
        let wake = Waking(self.wake.clone());
        let begun = self.home.sync_begin(Box::new(move || drop(wake)));
        // Not begun for want of a thread or a descriptor, which is no
        // fault of the volume's: the worker makes this sync itself, as
        // [`Core::apply`] does, and the next step tries the helper
        // again. The whole sync is then the worker's part of it.
        if matches!(&begun, Err(e) if NotBegun::is(e)) {
            if self.sync_home(written_at).is_err() {
                return Err(self.fail());
            }
            let synced_at = Instant::now();
            self.timing.step_home_sync.add(synced_at - written_at);
            self.part(written_at, synced_at, Activity::STEP_HOME_SYNC);
            return self.finish(applied, synced_at, false);
        }
        // The worker's part of this sync: handing it over.
        let began = Instant::now();
        self.timing.step_home_sync.add(began - written_at);
        self.part(written_at, began, Activity::STEP_HOME_SYNC);
        if let Err(e) = begun {
            return Err(if HelperEnded::is(&e) {
                self.helper_end()
            } else {
                self.fail()
            });
        }
        let applied = if self.mutants.has(Mutants::START_BEFORE_REPORT) {
            self.finish(applied, Instant::now(), false)?;
            None
        } else {
            Some(applied)
        };
        self.handed = Some(Handed { applied, began });
        match self.report(false) {
            Some(Err(failed)) => Err(failed),
            _ => Ok((target, cut)),
        }
    }

    /// The helper's report of the home image's sync that a step handed
    /// over: `None` if no step did, or if the sync is under way (with
    /// `wait` it is waited for). Once it has ended the step is taken up
    /// again where [`Core::apply`] would be after its sync: the log's
    /// new start, the index's drop, the second barrier. A failed sync
    /// fails the volume, as one of the worker's own would. On a volume
    /// that failed meanwhile the step is not taken up.
    fn report(&mut self, wait: bool) -> Option<Result<(), BlkError>> {
        self.handed.as_ref()?;
        let waited = wait.then(Instant::now);
        let ended = self.home.sync_end(wait)?;
        let reported_at = Instant::now();
        if let Some(waited) = waited {
            self.timing.step_report.add(reported_at - waited);
            self.part(waited, reported_at, Activity::STEP_REPORT);
        }
        let handed = self.handed.take()?;
        Some(match ended {
            Err(e) if HelperEnded::is(&e) => Err(self.helper_end()),
            Err(_) => Err(self.fail()),
            Ok(()) => {
                let took = reported_at - handed.began;
                let (bytes, handed_bytes) = self.home_writeback.synced();
                self.timing.syncs[SyncKind::HOME].add(took, bytes, handed_bytes);
                self.timing.writeback[1] = self.home_writeback.done;
                self.timing.helper.add(took);
                match handed.applied {
                    _ if self.failed => Err(BlkError::Failed),
                    Some(applied) => self.finish(applied, reported_at, false).map(|_| ()),
                    None => Ok(()),
                }
            }
        })
    }

    /// Wait for the helper's report of a step that handed its sync over,
    /// and take the step up: before whatever may not overlap that sync
    /// (another step, an explicit checkpoint, room made in a full log,
    /// a change of the hold, the end of a served store).
    fn settle(&mut self) -> Result<(), BlkError> {
        self.report(true).unwrap_or(Ok(()))
    }

    /// The helper ended without a report: the volume serves nothing
    /// more, and a served store's worker ends (the target's end). The
    /// log is not marked: what the helper did is not known, and the
    /// log's start has not moved, so the next open applies the batch
    /// again.
    fn helper_end(&mut self) -> BlkError {
        self.helper_ended = true;
        self.failed = true;
        BlkError::Failed
    }

    /// The first half of a batch: the neighbours saved, the first
    /// barrier, and the batch written into the home image, which is
    /// then not durable.
    fn apply_writes(
        &mut self,
        floor: Lsn,
        target: Lsn,
        limit: Lsn,
        hold: Option<Lsn>,
    ) -> Result<Applied, BlkError> {
        // The batch's time, in parts that follow each other with no
        // gap: one clock read at each boundary. A batch that fails
        // leaves its last part uncounted: the volume has failed.
        let began = Instant::now();
        if self.protect_neighbours(floor, target, limit).is_err() {
            return Err(self.fail());
        }
        let saved_at = Instant::now();
        self.timing.step_save.add(saved_at - began);
        self.part(began, saved_at, Activity::STEP_SAVE);
        // The first barrier: the readers that are under way end before
        // the home image is written. One of them may have found a
        // block in the home image before the write that this batch
        // applies there was appended, and still be reading it: a
        // buffered read beside a buffered write of the same page may
        // return a block that is part old and part new (assumed of
        // the kernel, not shown by a test here), which rule R3
        // forbids. A reader that begins after the barrier finds every
        // block of the batch in the log: the batch's records were in
        // the index before the batch was chosen, and stay there until
        // the step's end. Deallocating is a write here.
        if !self.mutants.has(Mutants::HOME_WRITTEN_UNBARRED) {
            self.shared.barrier(self.mutants);
        }
        let saved_at = self.barred(saved_at, false);
        // (1) The neighbours are saved and durable; the home image is
        // as it was.
        #[cfg(any(test, feature = "test-hooks"))]
        self.pause_at(StepPoint::Saved);
        let home = &mut self.home;
        let hint = &mut self.home_writeback;
        let zeroes = &mut self.zeroes;
        #[cfg(any(test, feature = "test-hooks"))]
        let refused = self.refuse_holes;
        #[cfg(not(any(test, feature = "test-hooks")))]
        let refused = false;
        let (always_written, no_holes_fails) = (
            self.mutants.has(Mutants::ZEROES_WRITTEN),
            self.mutants.has(Mutants::NO_HOLES_FAILS),
        );
        let goes_on = self.mutants.has(Mutants::REFUSAL_ENDS_NO_STEP);
        let mut written = Ok(());
        // Where the batch ends early, and what it took until there.
        let mut cut: Option<Cut> = None;
        let (mut records, mut payload) = (0u64, 0u64);
        let replayed = self.log.replay_within(floor, target, limit, &mut |record| {
            if written.is_err() || cut.is_some() {
                return;
            }
            records += 1;
            written = match record.kind {
                Kind::Write => {
                    payload += record.length;
                    home.write_at(record.offset, record.data)
                        .and_then(|()| hint.wrote(home, record.offset, record.length))
                }
                Kind::WriteZeroes if always_written || *zeroes == Zeroes::Written => {
                    write_zeros(home, record.offset, record.length)
                        .and_then(|()| hint.wrote(home, record.offset, record.length))
                }
                // The range is deallocated: nothing is written for it.
                // Its first and last write unit may be covered only in
                // part, and the file system then zeroes that part by
                // a write: their other blocks were saved above, as for
                // a write there.
                Kind::WriteZeroes => {
                    let hole = if refused {
                        Err(io::ErrorKind::Unsupported.into())
                    } else {
                        home.deallocate(record.offset, record.length)
                    };
                    match hole {
                        Ok(()) => {
                            *zeroes = Zeroes::Deallocated;
                            Ok(())
                        }
                        // The file system has no such call and changed
                        // nothing: zeros are written, for this record
                        // and from now on. Not a failure of the volume.
                        //
                        // If it deallocated until now, the batch was
                        // sized with these records counting nothing,
                        // and writing the rest of it could be far
                        // more than a step's bound: the batch ends
                        // with this record, and the next is sized by
                        // what it writes.
                        Err(e) if e.kind() == io::ErrorKind::Unsupported && !no_holes_fails => {
                            if *zeroes == Zeroes::Deallocated && !goes_on {
                                cut = Some(Cut {
                                    end: record.end,
                                    records,
                                    payload: payload + record.length,
                                });
                            }
                            *zeroes = Zeroes::Written;
                            write_zeros(home, record.offset, record.length)
                                .and_then(|()| hint.wrote(home, record.offset, record.length))
                        }
                        // A failed write, as any other.
                        Err(e) => Err(e),
                    }
                }
                _ => Ok(()),
            };
        });
        let written = written.is_err() && !self.mutants.has(Mutants::IGNORE_HOME_WRITE);
        if replayed.is_err() || written {
            return Err(self.fail());
        }
        let target = cut.map_or(target, |cut| cut.end);
        let written_at = Instant::now();
        self.timing.step_apply.add(written_at - saved_at);
        self.part(saved_at, written_at, Activity::STEP_APPLY);
        // (2) The middle of a step: the home image holds the batch and
        // is not durable, and the log still holds it too.
        #[cfg(any(test, feature = "test-hooks"))]
        self.pause_at(StepPoint::Written);
        Ok(Applied {
            target,
            cut,
            hold,
            written_at,
        })
    }

    /// The second half of a batch, once the home image is durable
    /// (`early`: with [`Mutants::RECLAIM_BEFORE_HOME_SYNC`], before it
    /// is): the log's new start, the index's drop, the second barrier.
    /// `synced_at` is where its time begins.
    fn finish(
        &mut self,
        applied: Applied,
        synced_at: Instant,
        early: bool,
    ) -> Result<(Lsn, Option<Cut>), BlkError> {
        let Applied {
            target, cut, hold, ..
        } = applied;
        // (3) The home image is durable, and the log starts where it
        // did: a start now applies the batch again from the log.
        #[cfg(any(test, feature = "test-hooks"))]
        self.pause_at(StepPoint::Synced);
        // The home image is durable. The reclaim moves the log's start,
        // and with that the preimages, which are bound to the start
        // they were written for, have done their work.
        if self.mutants.has(Mutants::CHECKPOINT_PAST_HOLD) {
            self.log.set_hold(None);
        }
        // (4) The log's new start: written, then durable.
        #[cfg(any(test, feature = "test-hooks"))]
        if let Some((pause, StepPoint::StartWritten)) = &self.pause {
            let pause = pause.clone();
            self.log
                .set_superblock_pause(Some(Box::new(move || pause.wait())));
        }
        let reclaimed = self.log.reclaim(target);
        #[cfg(any(test, feature = "test-hooks"))]
        self.log.set_superblock_pause(None);
        self.log.set_hold(hold);
        let Ok(start) = reclaimed else {
            return Err(self.fail());
        };
        #[cfg(any(test, feature = "test-hooks"))]
        self.pause_at(StepPoint::StartDurable);
        if early && self.sync_home(Instant::now()).is_err() {
            return Err(self.fail());
        }
        self.note_log();
        let started_at = Instant::now();
        self.timing.step_start.add(started_at - synced_at);
        self.part(synced_at, started_at, Activity::STEP_LOG_START);
        {
            // In one hold of the lock: the index lets go of the batch,
            // and of the segment files the log gave up with it.
            let mut lookup = self.shared.write();
            if self.mutants.has(Mutants::INDEX_SWITCH_EARLY) {
                lookup.index.drop_below(self.log.durable());
            } else {
                lookup.index.drop_below(start);
            }
            lookup.follow(&self.log);
        }
        // The batch's last part: the index lets go of what is now in
        // the home image.
        let ended_at = Instant::now();
        self.timing.step_end.add(ended_at - started_at);
        self.part(started_at, ended_at, Activity::STEP_END);
        // The second barrier: the readers that are under way end
        // before a segment file this step freed can be used again.
        // The log reuses its files, so a reader that found a record
        // before the index let go of it would otherwise read that
        // place after another record was written there. A reader that
        // begins after the barrier cannot find the old position.
        if !self.mutants.has(Mutants::SLOT_REUSED_UNBARRED) {
            self.shared.barrier(self.mutants);
        }
        self.barred(ended_at, true);
        Ok((start, cut))
    }
}

/// A batch written into the home image and not yet durable there: see
/// [`Core::apply_writes`] and [`Core::finish`].
#[derive(Debug, Clone, Copy)]
struct Applied {
    /// Where the batch ends: where the log will start.
    target: Lsn,
    /// Where it was cut short, if it was.
    cut: Option<Cut>,
    /// The hold when the batch was chosen, which the log's new start
    /// puts back.
    hold: Option<Lsn>,
    /// When its writes were done.
    written_at: Instant,
}

/// A step whose home-image sync is with the helper: see
/// [`Core::apply_handing_over`].
#[derive(Debug)]
struct Handed {
    /// The batch to take up when the sync is reported. `None` with
    /// [`Mutants::START_BEFORE_REPORT`], which took it up at once.
    applied: Option<Applied>,
    /// When the sync was handed over.
    began: Instant,
}

/// What the helper calls when it has synced the home image: the
/// worker's wake. Called by being dropped, so that a helper that ends
/// with it uncalled still wakes the worker, which then finds the
/// helper gone.
struct Waking(Option<Arc<dyn Fn() + Send + Sync>>);

impl Drop for Waking {
    fn drop(&mut self) {
        if let Some(wake) = &self.0 {
            wake();
        }
    }
}

/// A batch that ended before its target: the file system, which had
/// deallocated until then, refused at a write-zeroes record.
#[derive(Debug, Clone, Copy)]
struct Cut {
    /// The end of that record, where the batch ended.
    end: Lsn,
    /// The records the batch took, that one included.
    records: u64,
    /// The bytes it wrote into the home image for them, as a step
    /// counts them: that record's zeros, and the writes before it.
    payload: u64,
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
                    .shared
                    .read()
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
        // Written whole and synced at once: the hint would hand over
        // nothing the sync does not. Only what the sync waited for is
        // counted.
        if !self.mutants.has(Mutants::PREIMAGES_UNSYNCED) {
            let began = Instant::now();
            file.sync().map_err(|_| ())?;
            self.timing.syncs[SyncKind::PREIMAGE].add(began.elapsed(), image.len() as u64, 0);
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

/// How much one step of a checkpoint may take ([`Store::checkpoint_step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepBound {
    /// The most bytes of writes, and of write-zeroes whose zeros are
    /// written, by the length each covers on the volume: what the step
    /// writes into the home image. A discard counts zero bytes, and so
    /// does a write-zeroes whose range is deallocated ([`Zeroes`]): a
    /// step writes nothing for them.
    pub payload: u64,
    /// The most records, of every kind: a discard is one record too,
    /// so a log of discards alone is worked off in bounded steps and a
    /// step never walks more than this many records and one.
    pub records: u64,
}

impl StepBound {
    /// The daemon's step (design §5.1): 8 MiB or 1024 records,
    /// whichever comes first. For writes that is more than the largest
    /// request adds to the log, so a step after every request shortens
    /// the log. For records that are short in the log it is not: eight
    /// write-zeroes of 1 MiB whose zeros are written
    /// ([`Zeroes::Written`]) are 32 KiB of log at a record alignment
    /// of 4096, and 1024 discards are 64 KiB at an alignment of 64.
    /// The log's bound then rests on the rule and not on the step
    /// ([`Marks`]): at the high mark it takes as many steps as it
    /// needs before the next request.
    pub const DAEMON: Self = Self {
        payload: 8 << 20,
        records: 1024,
    };
}

/// For test rigs: where in a batch of a checkpoint, and so in every
/// step, the hook waits ([`Store::set_step_pause_at`]). The points are
/// the cuts between a step's four parts; a kill at each must leave a
/// store that opens with everything acknowledged.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepPoint {
    /// After (1): the neighbours of what the step writes are saved and
    /// durable, and the home image is as it was.
    Saved,
    /// After (2): the home image holds the batch and is not durable.
    #[default]
    Written,
    /// After (3): the home image is durable, and the log still starts
    /// where it did.
    Synced,
    /// Inside (4): the log's new start is written and not durable.
    StartWritten,
    /// After (4): the log's new start is durable.
    StartDurable,
}

/// For test rigs: how long every batch of a checkpoint, and so every
/// step, waits at its [`StepPoint`]. A kill in that time lands inside
/// a step.
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepPause {
    /// For this long.
    For(std::time::Duration),
    /// Until a file of this path exists. When a step arrives at the
    /// point it first makes an empty file of this path with `.reached`
    /// appended ([`StepPause::reached`]), so that whoever kills the
    /// process there can wait for the step to be at the point, and not
    /// only under way.
    Until(std::path::PathBuf),
}

#[cfg(any(test, feature = "test-hooks"))]
impl StepPause {
    /// The file a step makes when it arrives at the point of a pause
    /// [`StepPause::Until`] this path.
    pub fn reached(path: &std::path::Path) -> std::path::PathBuf {
        let mut name = path.as_os_str().to_owned();
        name.push(".reached");
        name.into()
    }

    fn wait(&self) {
        match self {
            Self::For(time) => thread::sleep(*time),
            Self::Until(path) => {
                // For the rig only: if the file cannot be made, the
                // rig waits and says so; the step is not failed for it.
                let _ = std::fs::write(Self::reached(path), b"");
                while !path.exists() {
                    thread::sleep(std::time::Duration::from_millis(5));
                }
            }
        }
    }
}

/// When the store checkpoints by itself (design §5.1, "when the daemon
/// checkpoints"): two marks on the log's live size, the bytes from its
/// start to its end. At or above `high` the store takes steps of
/// `step`, one after every request it executes and whenever it has
/// none, until the live size is below `low`. And while the live size
/// is at or above `high`, no request is executed as long as a step
/// still takes something: the log is brought back below `high` first.
/// So before every request the log is below `high`, and it never
/// holds more than `high` and what one request adds, whatever its
/// records are. Two things end the steps with the log still at
/// `high`, as they end them anywhere: a hold in front of the log, and
/// a failed volume. The request is then executed, and behind a hold
/// the log grows until it is full and the request is held.
///
/// # What it costs a client
///
/// Between the marks every request waits for one step before the next
/// request is executed, and a step ends with a sync of the home image
/// and one of the log's superblock. A served store hands the home
/// image's sync to a helper and serves meanwhile; it takes no further
/// step until the helper reports, and at the high mark it waits for
/// the report (see the crate's documentation, "The home image's sync
/// on a helper"). Where the worker makes the sync itself, the worst
/// case is one home-image sync per request until the low mark: with a small step
/// bound, or with requests that each append little while the step
/// finds little durable, a burst that reaches the high mark is served
/// at the pace of the volume's syncs until the log is below the low
/// mark again. A step takes up to its bound, so a log that grows by
/// less per request than a step takes shrinks, and the phase ends;
/// with [`StepBound::DAEMON`] a step takes up to 8 MiB or 1024
/// records. One record larger than the bound is a step of its own.
/// Before a step the worker also syncs the log itself when less than
/// a step of it is durable and more could be ([`LogState::own_syncs`]
/// counts these).
///
/// At the high mark a request waits for more than one step when a
/// step takes less log than the request before it added. For writes
/// it does not: a step of the daemon's takes at least 7 MiB of them.
/// For records that are short in the log it does. With the daemon's
/// step, a record alignment of 4096 and requests of at most 1 MiB:
///
/// - behind discards, and behind write-zeroes whose ranges are
///   deallocated ([`Zeroes::Deallocated`]), a step takes 1024
///   records, 4 MiB of log and nothing to write: one step a request,
///   as for writes. At a record alignment of 64 it is 64 KiB of log a
///   step, and a write of 1 MiB waits for 17 steps, each with its
///   syncs and nothing written;
/// - behind write-zeroes of 1 MiB each on a file system that cannot
///   deallocate ([`Zeroes::Written`]) a step takes eight records,
///   32 KiB of log, and writes 8 MiB of zeros into the home image; a
///   write of 1 MiB adds 1 MiB and 4 KiB, so the next request waits
///   for 33 steps: 264 MiB of zeros written and 33 syncs of the home
///   image. A log filled to a high mark of 128 MiB with such records
///   holds 32 768 of them, 32 GiB of zeros to write in 4096 steps,
///   and writes are served at that pace until they are gone. That is
///   long, and it is the price of a file system without holes: the
///   bound of a step does not hide it, and the store says which of
///   the two a volume is on ([`LogState::zeroes`]).
///
/// This cost is the design's choice of a bounded log and a bounded
/// start over an even latency (design §5.1). It has not been measured
/// on a device yet. Do not soften it (steps less often, a larger
/// bound, no step while requests wait) without a measurement that
/// shows what the change costs the log's bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Marks {
    /// The live size at which steps begin, in bytes.
    pub high: u64,
    /// The live size below which they end, in bytes: below `high`.
    pub low: u64,
    /// What one step may take.
    pub step: StepBound,
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

    /// Count what `other` counted as well.
    fn merge(&mut self, other: &Spent) {
        self.count += other.count;
        self.sum_ns = self.sum_ns.saturating_add(other.sum_ns);
        self.max_ns = self.max_ns.max(other.max_ns);
    }
}

/// The kinds of request the timing tells apart, in the order of
/// [`Timing::requests`].
pub const TIMED_KINDS: [&str; 5] = ["read", "write", "flush", "discard", "write_zeroes"];

fn timed_kind(op: &Op) -> usize {
    match op {
        Op::Read { .. } => 0,
        Op::Write { .. } => 1,
        Op::Flush => 2,
        Op::Discard { .. } => 3,
        Op::WriteZeroes { .. } => 4,
    }
}

/// What a store's worker can be doing, as numbers into
/// [`ACTIVITIES`], [`Timing::worker`] and [`Timing::wait_by`].
struct Activity;

impl Activity {
    /// Waiting for a request with nothing to do.
    const IDLE: usize = 0;
    /// The call of a request of kind `kind` ([`TIMED_KINDS`]).
    const fn call(kind: usize) -> usize {
        1 + kind
    }
    const STEP_WALK: usize = 6;
    const STEP_SAVE: usize = 7;
    const STEP_APPLY: usize = 8;
    const STEP_HOME_SYNC: usize = 9;
    const STEP_LOG_START: usize = 10;
    const STEP_END: usize = 11;
    const OWN_SYNC: usize = 12;
    /// Everything else: the loop's own work between two of the above
    /// and commands that are no request.
    const OTHER: usize = 13;
    /// A step waiting at one of its two barriers for the readers
    /// that are under way.
    const STEP_BARRIER: usize = 14;
    /// Waiting for the helper to report the home image's sync: at the
    /// high mark, and before what may not overlap that sync.
    const STEP_REPORT: usize = 15;
}

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

/// What the worker did, span after span, for as far back as a request
/// that is still waiting can have been submitted: a request's wait is
/// attributed by laying it over these.
///
/// Exact, and without anything shared between threads: the request
/// carries the time of its submission, the worker knows what it did
/// since. The cost is one entry per span, in a queue that is reused,
/// and for a request a walk back over the spans of its wait.
struct Spans {
    /// Where the last span ended.
    last: Instant,
    /// The end of each span and what it was, oldest first. A span
    /// begins where the one before it ended.
    spans: VecDeque<(Instant, usize)>,
}

impl Spans {
    /// At most this many spans are kept. A wait that reaches further
    /// back has the rest counted under the oldest span that is kept.
    const KEPT: usize = 1 << 16;

    fn new() -> Self {
        Self {
            last: Instant::now(),
            spans: VecDeque::new(),
        }
    }

    /// The span that is open now, if the thread is doing `activity`.
    fn open(&self, activity: usize) -> Open {
        Some((self.last, activity))
    }

    /// The worker did `activity` from the last span's end until `now`.
    fn close(&mut self, activity: usize, now: Instant, timing: &mut Timing) {
        let Some(time) = now
            .checked_duration_since(self.last)
            .filter(|t| !t.is_zero())
        else {
            return;
        };
        let ns = u64::try_from(time.as_nanos()).unwrap_or(u64::MAX);
        timing.worker[activity] = timing.worker[activity].saturating_add(ns);
        if self.spans.len() == Self::KEPT {
            self.spans.pop_front();
        }
        self.spans.push_back((now, activity));
        self.last = now;
    }

    /// The parts of a batch that the core noted since the last span's
    /// end: what lies between them and before the first is `around`,
    /// which is "other" for the rule's steps and a request's kind for
    /// a batch that its call made room with.
    fn take(
        &mut self,
        parts: &mut Option<Vec<(Instant, Instant, usize)>>,
        around: usize,
        timing: &mut Timing,
    ) {
        for (began, ended, activity) in parts.iter_mut().flat_map(|parts| parts.drain(..)) {
            // Whatever notes parts is followed by a take before the
            // worker closes another span. Where one is missing, the
            // parts would lie before the last span's end and `close`
            // would drop them without a word: the time would be under
            // whatever span was closed over them. A debug build, which
            // the tests are, stops here.
            debug_assert!(
                began >= self.last,
                "a part of a batch (activity {activity}) began before the last span's end: \
                 parts were noted and not taken before the worker closed a span"
            );
            self.close(around, began, timing);
            self.close(activity, ended, timing);
        }
    }

    /// What the worker did between `from` and the last span's end, by
    /// activity, in nanoseconds. The oldest span that is kept is taken
    /// to reach back as far as `from`: its begin was forgotten because
    /// no request that waits was submitted before it.
    fn between(&self, from: Instant) -> [u64; ACTIVITY_COUNT] {
        let mut by = [0u64; ACTIVITY_COUNT];
        for at in (0..self.spans.len()).rev() {
            let (end, activity) = self.spans[at];
            if end <= from {
                break;
            }
            let began = match at.checked_sub(1) {
                Some(before) => self.spans[before].0.max(from),
                None => from,
            };
            let ns = u64::try_from(end.saturating_duration_since(began).as_nanos());
            by[activity] = by[activity].saturating_add(ns.unwrap_or(u64::MAX));
        }
        by
    }

    /// Forget the spans no wait can reach any more. Called when the
    /// worker has just found nothing more arrived, so every request
    /// that waits is in its queue and `oldest` is the submission of
    /// the first of them: spans that ended before it are forgotten.
    /// With none waiting, a request can still come whose submission
    /// was stamped a moment before now and handed over after: the
    /// spans of the last millisecond are kept for it, and the last
    /// span always.
    fn forget(&mut self, oldest: Option<Instant>) {
        const MARGIN: Duration = Duration::from_millis(1);
        let oldest = oldest.or_else(|| Instant::now().checked_sub(MARGIN));
        while self.spans.len() > 1
            && self
                .spans
                .front()
                .is_some_and(|(end, _)| oldest.is_none_or(|oldest| *end <= oldest))
        {
            self.spans.pop_front();
        }
    }
}

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
    /// ([`OpenOptions::readers`]) each is the sum over the readers, in
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

impl Synced {
    fn add(&mut self, time: Duration, bytes: u64, handed: u64) {
        self.count += 1;
        self.sum_ns = self
            .sum_ns
            .saturating_add(u64::try_from(time.as_nanos()).unwrap_or(u64::MAX));
        self.bytes += bytes;
        self.handed += handed;
    }
}

/// The kinds of sync [`Timing::syncs`] tells apart: a client's flush
/// (and [`Store::sync_log`]); the checkpoint rule's own sync of the
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

/// Numbers into [`SYNC_KINDS`].
struct SyncKind;

impl SyncKind {
    const FLUSH: usize = 0;
    const OWN: usize = 1;
    const ROOM: usize = 2;
    const HOME: usize = 3;
    const PREIMAGE: usize = 4;
    const SUPERBLOCK: usize = 5;
}

/// The log as a store shows it to its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogState {
    /// Where the log starts: everything before is in the home image.
    pub start: u64,
    /// Where it ends.
    pub end: u64,
    /// Up to where it is durable.
    pub durable: u64,
    /// The steps the rule of the marks has taken since the open.
    pub steps: u64,
    /// The times the rule made the log durable itself since the open,
    /// because too little of it was durable to take a step from and no
    /// flush of the client's had come: what a client that does not
    /// flush costs the store.
    pub own_syncs: u64,
    /// Whether such a step is under way now. A step begins with the
    /// rule's own sync of the log, when it needs one: that sync can
    /// take as long as any, and is shown as the step it is for. It is
    /// under way while its home image's sync is with the helper, until
    /// the worker has taken the report and the step up again.
    pub stepping: bool,
    /// How write-zeroes records reach the home image.
    pub zeroes: Zeroes,
}

/// How a write-zeroes record reaches the home image when a checkpoint
/// applies it (design §5.1).
///
/// The range is deallocated in the home image, which is a sparse file:
/// a hole reads as zeros, nothing is written, and the record counts no
/// bytes in a step's bound, as a discard counts none. Where the file
/// system has no call for that, zeros are written instead, the record
/// counts its length, and a log of such records is worked off at the
/// pace of that writing: a request can then wait for many steps
/// ([`Marks`], "what it costs a client").
///
/// Which of the two holds for a volume is found out by the first
/// write-zeroes record that a checkpoint applies after an open, and
/// holds until the store is closed. Until then such a record counts
/// its length, so that a step keeps to its bound whichever it turns
/// out to be.
///
/// - **The edges.** The volume's blocks are 4 KiB. Where the write
///   unit is larger, a range can cover its first and last unit only in
///   part, and the file system zeroes a part of a unit by writing it.
///   So the other blocks of those two units are saved as preimages
///   before the range is deallocated, exactly as for a write there. A
///   range that covers whole units, and every range where the unit is
///   one block, has no neighbours and saves nothing. This reaches as
///   far as the write unit and no further, which is enough because no
///   store is open on a file system whose block is larger than its
///   write unit ([`StoreError::Blocks`]).
/// - **Durability.** A hole is durable when the home image's sync has
///   returned, like a write, and the log keeps the record until then.
///   A power cut before that leaves each block of the range as it was
///   or zero; the open applies the record again.
/// - **Reads.** While the record is in the log, a read of its blocks
///   is answered from the index, with zeros and without the home
///   image. Afterwards the home image is read, and a hole reads as
///   zeros. The store keeps no other copy of a block that could bring
///   older content back.
/// - **A discard is unchanged**: it is recorded and changes nothing in
///   the home image. A read after a discard returns what was there,
///   which the contract allows and the store's tests assert; freeing
///   the space would be a change of what a discard does, not a part of
///   making write-zeroes cheap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zeroes {
    /// No checkpoint has applied a write-zeroes record since the open.
    #[default]
    Untried,
    /// The ranges are deallocated.
    Deallocated,
    /// The file system cannot deallocate: zeros are written.
    Written,
}

impl LogState {
    /// The live size: what a start replays. Zero for a state whose
    /// start is beyond its end, which no store shows.
    pub fn live(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// What one step of a checkpoint did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    /// Where the log starts now.
    pub start: Lsn,
    /// The records it took, of every kind.
    pub records: u64,
    /// The bytes of the volume its writes and write-zeroes cover: what
    /// it wrote into the home image.
    pub payload: u64,
    /// Whether a step right after this one would find something to
    /// take: durable records below the hold are left.
    pub more: bool,
}

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

/// Write `len` zeros at `offset`: for a write-zeroes record on a file
/// system that cannot deallocate ([`Zeroes::Written`]). The record's
/// whole length is written, for a record that is one header in the
/// log.
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
    /// See [`Store::recovered_unacknowledged`].
    unacknowledged: u64,
    /// See [`Store::replayed`].
    replayed: (u64, u64),
    queue: VecDeque<Request>,
    /// The ids in flight: submitted and not yet returned.
    ids: BTreeSet<RequestId>,
}

impl Store<crate::fs::FsDir> {
    /// Another hold on the volume's lock this store has
    /// ([`FsLock::share`](crate::fs::FsLock::share)): the directory
    /// stays locked until the store and every such hold are gone. For
    /// an owner that has work to do under the lock after the store has
    /// ended, on whatever thread it ended. `None` if the store was
    /// opened without the lock, which only a test's defect does.
    pub fn share_lock(&self) -> Option<crate::fs::FsLock> {
        self.core._lock.as_ref().map(crate::fs::FsLock::share)
    }
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
        // Before anything is made: the write unit must be at least the
        // block of the directory's file system. Every file of a store
        // is created in this directory, under a name that did not
        // exist, so it is on this file system.
        pages_within_unit(dir.page_size(), identity.unit(), options.mutants)?;
        if !options.mutants.has(Mutants::CREATE_ON_ANY_BLOCKS) {
            within_unit(Under::Directory, dir.block_size(), identity.unit())?;
        }
        // Before the lock, which makes a file: a directory with a file
        // of somebody else is refused and left exactly as it was. A
        // directory that holds a store has its lock file already, and
        // whether that store is in use is what the lock says first.
        // Then everything again under the lock, for what another
        // instance did in between.
        let found = dir.list().map_err(io_error("listing the directory"))?;
        if !found.iter().any(|name| name == crate::log::SUPER_NAME) && stranger(&found).is_some() {
            return Err(StoreError::NotEmpty);
        }
        let lock = Self::lock(&dir, &options)?;
        let names = Self::creatable(&dir)?;
        // The log's superblock is what makes a store, and it is made
        // last. What is here under one of a creation's own names is
        // left from a creation that was interrupted; nothing is removed
        // under any other name.
        for name in [HOME_NEW_NAME, HOME_NAME] {
            if names.iter().any(|found| found == name) {
                dir.remove(name).map_err(io_error("removing leftovers"))?;
            }
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
    /// The names in `dir`, if a store can be created there: no store
    /// yet, and no name but a creation's own.
    fn creatable(dir: &D) -> Result<Vec<String>, StoreError> {
        let names = dir.list().map_err(io_error("listing the directory"))?;
        if names.iter().any(|name| name == crate::log::SUPER_NAME) {
            return Err(LogError::Exists.into());
        }
        if stranger(&names).is_some() {
            return Err(StoreError::NotEmpty);
        }
        Ok(names)
    }

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

    /// Take the lock of the volume in `dir` and nothing else: nothing
    /// is read, recovered or written. For whoever must act on the
    /// volume's attachment without opening the store, or must read
    /// something under the lock before the recovery
    /// ([`Store::open_locked`]). While another instance has the lock
    /// this fails with [`StoreError::Locked`]. Dropping the lock
    /// releases it.
    pub fn lock_volume(dir: &D) -> Result<D::Lock, StoreError> {
        match dir.try_lock() {
            Ok(lock) => Ok(lock),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Err(StoreError::Locked),
            Err(e) => Err(io_error("locking the directory")(e)),
        }
    }

    /// Open the store in `dir` under a lock that is held already
    /// ([`Store::lock_volume`], on the same directory): recover the
    /// log and rebuild the index. The store takes the lock over and
    /// holds it for as long as it lives. If the open fails the lock
    /// comes back with the error, still held.
    pub fn open_locked(
        dir: D,
        lock: D::Lock,
        options: OpenOptions,
    ) -> Result<(Self, Recovery), (StoreError, D::Lock)> {
        match Self::recover(dir, options) {
            Ok((mut store, recovery)) => {
                store.core._lock = Some(lock);
                Ok((store, recovery))
            }
            Err(e) => Err((e, lock)),
        }
    }

    /// Open the store in `dir`: take its lock, recover the log, and
    /// rebuild the index from it. While another instance has the store
    /// open this fails with [`StoreError::Locked`] and changes nothing.
    pub fn open(dir: D, options: OpenOptions) -> Result<(Self, Recovery), StoreError> {
        let lock = Self::lock(&dir, &options)?;
        let (mut store, recovery) = Self::recover(dir, options)?;
        store.core._lock = lock;
        Ok((store, recovery))
    }

    /// Before an open reads, recovers or writes anything: the system's
    /// page is no larger than the store's write unit, and every file
    /// the store writes, and the directory itself, where files are
    /// made later, is on a file system whose block is no larger
    /// either. The directory may have been copied or moved since the
    /// store was created, to another file system or to a machine with
    /// larger pages, and a name in it may lead to another file system.
    /// Names under which the store writes nothing are not asked: the
    /// lock, and whatever else somebody put there. A directory with no
    /// readable superblock is left to the log's own open, which says
    /// what is wrong with it.
    fn writes_stay_within_the_unit(dir: &D, mutants: Mutants) -> Result<(), StoreError> {
        let reading = io_error("reading the superblock");
        let Some(unit) = crate::log::unit_of(dir).map_err(&reading)? else {
            return Ok(());
        };
        pages_within_unit(dir.page_size(), unit, mutants)?;
        if mutants.has(Mutants::OPEN_ON_ANY_BLOCKS) {
            return Ok(());
        }
        within_unit(Under::Directory, dir.block_size(), unit)?;
        for name in dir.list().map_err(io_error("listing the directory"))? {
            let Some(what) = Under::of(&name) else {
                continue;
            };
            let block = match dir.open(&name) {
                Ok(file) => file.block_size(),
                // Gone since the listing.
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => Err(e),
            };
            within_unit(what, block, unit)?;
        }
        Ok(())
    }

    /// The open, without the lock.
    fn recover(dir: D, options: OpenOptions) -> Result<(Self, Recovery), StoreError> {
        Self::writes_stay_within_the_unit(&dir, options.mutants)?;
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
        Ok((Self::assemble(log, home, None, options)?, recovery))
    }

    fn assemble(
        mut log: Log<D>,
        mut home: D::File,
        lock: Option<D::Lock>,
        options: OpenOptions,
    ) -> Result<Self, StoreError> {
        log.set_writeback_unit(options.writeback_unit);
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
        // A position recorded for another store counts as none, as in
        // the log's own comparison.
        let core_identity = crate::format::store_identity(log.key());
        let acknowledged = options.must_reach.filter(|_| {
            options
                .must_reach_identity
                .is_none_or(|recorded_for| recorded_for == core_identity)
        });
        let mut unacknowledged = 0u64;
        let mut replayed = 0u64;
        let replayed_bytes = log.tail().0.saturating_sub(log.floor().0);
        if !options.mutants.has(Mutants::NO_INDEX_REBUILD) {
            log.replay(log.floor(), log.tail(), &mut |record| {
                let start = record.offset / BLOCK_SIZE;
                let blocks = record.length / BLOCK_SIZE;
                let at = record.start;
                replayed += 1;
                if acknowledged.is_some_and(|position| at > position) {
                    unacknowledged += 1;
                }
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
        let mut shared = Shared::default();
        {
            let lookup = shared
                .lookup
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner);
            lookup.index = index;
            lookup.follow(&log);
        }
        shared.durable.store(log.durable().0, Ordering::SeqCst);
        Ok(Self {
            core: Core {
                log,
                home,
                home_writeback: Writeback::new(options.home_writeback_unit),
                preimages,
                _lock: lock,
                shared: Arc::new(shared),
                geometry,
                failed: false,
                unsynced: Vec::new(),
                deferred: None,
                appended: None,
                marks: None,
                working: false,
                steps: 0,
                own_syncs: 0,
                timing: Timing::default(),
                parts: None,
                hands_over: options.hand_over_home_sync,
                handed: None,
                wake: None,
                helper_ended: false,
                zeroes: Zeroes::Untried,
                #[cfg(any(test, feature = "test-hooks"))]
                pause: None,
                #[cfg(any(test, feature = "test-hooks"))]
                refuse_holes: false,
                #[cfg(any(test, feature = "test-hooks"))]
                read_pause: None,
                #[cfg(any(test, feature = "test-hooks"))]
                read_pauses_at: Vec::new(),
                readers: options.readers.max(1),
                #[cfg(test)]
                spawn_fails_at: None,
                #[cfg(test)]
                readers_ended: Arc::default(),
                #[cfg(any(test, feature = "test-hooks"))]
                sync_pause: None,
                mutants: options.mutants,
            },
            unacknowledged,
            replayed: (replayed_bytes, replayed),
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

    /// One bounded step of a checkpoint: the first of the log's durable
    /// records below the hold, at most `bound` of them and at least one
    /// if there is any, are applied to the home image, which is synced,
    /// and that part of the log is given up. A record that alone
    /// exceeds the bound is taken alone. With nothing to take it does
    /// nothing and touches no file. On a failed volume, and if it fails
    /// the volume, an error.
    ///
    /// It never makes anything durable itself: records appended since
    /// the last flush stay in the log until a flush or a
    /// [`Store::sync_log`].
    pub fn checkpoint_step(&mut self, bound: StepBound) -> Result<Step, BlkError> {
        if self.core.failed {
            return Err(BlkError::Failed);
        }
        self.core.checkpoint_step(bound)
    }

    /// Make every record appended so far durable, as a flush does, but
    /// not as an operation of the client: it has no place in the
    /// volume's order and answers nobody. For whoever checkpoints
    /// under a client that does not flush.
    pub fn sync_log(&mut self) -> Result<(), BlkError> {
        if self.core.failed {
            return Err(BlkError::Failed);
        }
        self.core.flush(SyncKind::FLUSH)
    }

    /// Give the log up for the in-place store
    /// ([`crate::inplace`]): make everything appended durable, apply
    /// the whole log to the home image, and hand the home image, the
    /// empty log (for its failure marker and observer) and the lock to
    /// an [`InPlace`](crate::inplace::InPlace) store, which syncs the
    /// home image before it is returned. `waker` is called when a sync
    /// that a flush began ends on the file's helper.
    ///
    /// Refused while requests are queued or in flight. On a failed
    /// volume, and where any step fails, an error, and the volume has
    /// failed (the log's marker). The log store itself is unchanged by
    /// this: it is a checkpoint of everything, and a volume that is
    /// later opened by the log store again finds an empty log and the
    /// home image as the in-place store left it. But a volume served in
    /// place must not be served by the log store again: see the
    /// warning in the [in-place store's documentation](crate::inplace).
    pub fn into_in_place(
        self,
        waker: Option<Waker>,
    ) -> Result<crate::inplace::InPlace<D>, crate::inplace::IntoInPlaceError>
    where
        D: Clone + Send + 'static,
        D::Lock: Send + 'static,
    {
        self.into_in_place_with(waker, crate::inplace::Mutants::default())
    }

    pub(crate) fn into_in_place_with(
        mut self,
        waker: Option<Waker>,
        mutants: crate::inplace::Mutants,
    ) -> Result<crate::inplace::InPlace<D>, crate::inplace::IntoInPlaceError>
    where
        D: Clone + Send + 'static,
        D::Lock: Send + 'static,
    {
        use crate::inplace::{FailedStart, IntoInPlaceError, StartFailed};
        if !self.queue.is_empty() || !self.ids.is_empty() {
            return Err(IntoInPlaceError::InFlight);
        }
        // A failure of the log store marks the log as its own failure
        // path does (here, on this thread), before this returns; only
        // the in-place store's own sync hands its marker back unwritten.
        let failed = |core: &Core<D>, what: StartFailed| {
            if core.helper_ended {
                IntoInPlaceError::HelperEnded
            } else {
                IntoInPlaceError::Failed(FailedStart::written(what))
            }
        };
        // A store that failed before the hand-over failed in the log
        // store's own open or service.
        if self.core.failed {
            return Err(failed(&self.core, StartFailed::Log));
        }
        // What a failure of the hand-over is reported as: the log store's
        // own where its log holds operations to apply into the home
        // image, and otherwise the image's ([`StartFailed`]).
        let what = if self.core.shared.read().index.is_empty() {
            StartFailed::Image
        } else {
            StartFailed::Log
        };
        self.core.marks = None;
        self.core.working = false;
        self.core.log.set_hold(None);
        let emptied = self
            .core
            .settle()
            .and_then(|()| self.core.flush(SyncKind::FLUSH))
            .and_then(|()| self.core.checkpoint())
            .and_then(|_| self.core.settle());
        if emptied.is_err() || self.core.failed {
            return Err(failed(&self.core, what));
        }
        let (start, end) = (self.core.log.floor(), self.core.log.tail());
        if start != end {
            return Err(IntoInPlaceError::NotEmpty(format!(
                "the log holds {} to {} after a checkpoint of all of it",
                start.0, end.0
            )));
        }
        let identity = self.core.identity();
        let marker = self.core.log.marker_writer();
        let Core {
            log,
            home,
            _lock,
            geometry,
            zeroes,
            ..
        } = self.core;
        crate::inplace::InPlace::assemble(
            crate::inplace::Parts {
                log,
                home,
                lock: _lock,
                geometry,
                identity,
                marker,
                zeroes,
            },
            waker.map(|Waker(call)| call),
            mutants,
        )
    }

    /// Let the store checkpoint by itself, by `marks`, or not at all.
    /// Not at all is how a store is opened. A served store applies the
    /// rule on its worker, after every request and when it has none; a
    /// store that is not served applies it after every request it
    /// executes and when [`Store::tend`] is called.
    pub fn set_marks(&mut self, marks: Option<Marks>) {
        self.core.marks = marks;
        self.core.working = false;
    }

    /// For test rigs: behave as on a file system that cannot
    /// deallocate ([`Zeroes::Written`]).
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_refuse_holes(&mut self, refuse: bool) {
        self.core.refuse_holes = refuse;
    }

    /// For test rigs: make the reader of the served store wait after
    /// every lookup, with the lookup's lock released, before it reads
    /// a file. It holds its pin while it waits, taken before the
    /// lookup, so a step waits for it at its next barrier.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_read_pause(&mut self, pause: StepPause) {
        self.core.read_pause = Some(pause);
    }

    /// For test rigs: as [`Store::set_read_pause`], for the reads that
    /// begin at block `block` only. With several readers, one read can
    /// be held so while others go on; with a pause for each of two
    /// blocks, two readers can be held, each with its pin.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_read_pause_at(&mut self, block: u64, pause: StepPause) {
        self.core.read_pauses_at.push((block, pause));
    }

    /// For test rigs: make every sync of the log for a flush (a
    /// client's, the rule's own, or one that makes room) wait before
    /// it begins.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_sync_pause(&mut self, pause: StepPause) {
        self.core.sync_pause = Some(pause);
    }

    /// For test rigs: make every step wait at `point`.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn set_step_pause_at(&mut self, pause: StepPause, point: StepPoint) {
        self.core.pause = Some((pause, point));
    }

    /// Apply the rule of the marks once: see [`Marks`]. Returns whether
    /// a step was taken. On a failed volume it does nothing; if it
    /// fails the volume, an error.
    pub fn tend(&mut self) -> Result<bool, BlkError> {
        self.core.tend(&mut || {})
    }

    /// Where the time of the checkpoint's batches went: see [`Timing`].
    /// A store that is not served times nothing else.
    pub fn timing(&self) -> Timing {
        self.core.timing
    }

    /// The log's start, end and durable position, and the steps taken.
    pub fn log_state(&self) -> LogState {
        self.core.log_state()
    }

    /// Up to where the log is durable: what a checkpoint can take, as
    /// far as the hold allows.
    pub fn log_durable(&self) -> Lsn {
        self.core.log.durable()
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

    /// The identity ([`Store::identity`]) of the store in `dir`, read
    /// from the log's superblock without opening the store: no lock is
    /// taken, nothing is recovered and nothing is written. For whoever
    /// must tell which store a directory holds while a daemon may be
    /// serving it. `None` if the directory holds no store or neither
    /// superblock copy can be read as one.
    ///
    /// A store's key never changes, so either valid copy gives the
    /// same answer, also while the serving instance rewrites one.
    pub fn identity_of(dir: &D) -> io::Result<Option<[u8; 16]>> {
        Ok(crate::log::key_of(dir)?.map(|key| crate::format::store_identity(&key)))
    }

    /// How many operations this open found in the log beyond the
    /// position it was told was acknowledged
    /// ([`OpenOptions::must_reach`]): writes, write-zeroes and discards
    /// that the life before appended whole and whose completion it
    /// never passed on. Zero when the open was told no position, or
    /// one recorded for another store.
    ///
    /// **It is a lower bound** on what that life applied without
    /// answering its client. An operation at or before the position
    /// may also have gone unanswered: the position is recorded before
    /// the answers of a batch are given, and a kill between the two
    /// leaves answers ungiven that this count does not see. So does an
    /// operation that a checkpoint has already taken out of the log:
    /// with the rule of the marks ([`Marks`]) that includes a record
    /// which a step took before its answer was stored, when the kill
    /// came between the two. A flush leaves no record and is never
    /// counted.
    pub fn recovered_unacknowledged(&self) -> u64 {
        self.unacknowledged
    }

    /// What this open read back to build its index: the bytes of the
    /// log from its start to its end (the log's own records and
    /// padding included, so not zero for a log without operations), and
    /// the operations in it. Both
    /// grow until a checkpoint gives up a part of the log, and the time
    /// of an open grows with them.
    pub fn replayed(&self) -> (u64, u64) {
        self.replayed
    }

    /// Execute up to `limit` queued requests, in order, and collect
    /// their completions. Stops at a request that cannot proceed.
    fn run(&mut self, limit: usize, out: &mut Vec<Completion>) -> usize {
        let mut done = 0;
        while done < limit {
            let Some(Request { id, op }) = self.queue.pop_front() else {
                break;
            };
            // A step's sync with the helper is asked after, before each
            // request: on the simulated disk it ends where its seed says.
            // A sync that failed has failed the volume, which the request
            // finds.
            let _ = self.core.report(false);
            let helping = self.core.handed.is_some();
            match self.core.execute(op) {
                Ok((result, buf)) => {
                    if helping {
                        self.core.timing.helper_requests += 1;
                    }
                    self.ids.remove(&id);
                    if let Some(start) = self.core.appended.take() {
                        self.acknowledged = Some(start);
                    }
                    out.push(Completion { id, result, buf });
                    done += 1;
                    // The rule of the marks, as the worker of a served
                    // store applies it: a step after every request,
                    // and at the high mark steps until the log is
                    // below it before the next request.
                    self.core.tend_after_a_request();
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
    /// A request, and when it was submitted.
    Request(Request, Instant),
    Hold(Option<Lsn>, mpsc::Sender<Result<(), NotABoundary>>),
    Checkpoint(mpsc::Sender<Result<Lsn, BlkError>>),
    Position(mpsc::Sender<Lsn>),
    /// The reader could not read one of the store's files for the
    /// read `RequestId`: the worker fails the volume and answers the
    /// read, in that order.
    ReadFailed(RequestId, Buf),
    /// For tests: the worker panics.
    #[cfg(test)]
    Panic,
    /// For test rigs: fail the log now.
    #[cfg(any(test, feature = "test-hooks"))]
    Fail(mpsc::Sender<()>),
    /// The helper has ended the home image's sync, or has ended: the
    /// worker takes the report on its next turn.
    Reported,
    /// The owner is gone: the worker waits for the helper's report of
    /// a step's sync, takes the step up, and ends. (The worker holds a
    /// sender itself, for the helper's wake, so the channel does not
    /// close when the owner's senders go.)
    Stop,
}

/// How long a worker that waits for work, with a step's sync under way
/// on a file that does not say when it ends, waits before it asks again.
const LOOK_AGAIN: Duration = Duration::from_millis(1);

/// What the owner of a [`Served`] store hands its reader.
enum ToReader {
    /// A read, and when it was submitted.
    Read(Request, Instant),
    /// For tests: the reader panics.
    #[cfg(test)]
    Panic,
}

/// The readers' queue of reads: one queue for all the readers of a
/// served store, from which an idle reader takes the next read.
///
/// A push wakes one waiting reader, and only one: each read costs one
/// wake. A push that finds no reader waiting wakes none: every reader is
/// busy then, and looks before it waits again. A reader that finds the queue empty waits on the condition
/// variable with the queue's lock let go only inside the wait, so a
/// push between its look and its wait cannot happen, and no wake is
/// lost. A reader that is busy when a read is pushed finds it when it
/// looks again, without a wake. At the end ([`ReadQueue::close`]) the
/// readers take what is queued and then end. (Before, the readers
/// shared a channel's receiver behind a mutex: the idle reader that
/// held it got the read and, by letting go of the mutex, woke a second
/// reader, which took the mutex to wait on the channel. Two wakes and
/// two context switches per read.)
struct ReadQueue<T> {
    state: Mutex<Queued<T>>,
    ready: std::sync::Condvar,
    mutants: Mutants,
    /// For tests: called by a reader that found the queue empty, after
    /// its look and before its wait, with whether it still holds the
    /// queue's lock (it does unless [`Mutants::READ_WAKE_LOST`]).
    #[cfg(test)]
    between: Mutex<Option<Between>>,
}

/// For tests: what a reader does between its look and its wait.
#[cfg(test)]
type Between = Arc<dyn Fn(bool) + Send + Sync>;

struct Queued<T> {
    reads: VecDeque<T>,
    closed: bool,
    /// The readers in a wait: counted under the same hold of the lock
    /// as their look and their wait, so a push that finds none waiting
    /// can leave out the wake, and one that finds a reader about to
    /// wait cannot happen.
    waiting: usize,
    /// For tests: how often a reader has woken from a wait.
    #[cfg(test)]
    woke: u64,
    /// For tests: the pushes that found no reader waiting, whose wake
    /// went to nobody.
    #[cfg(test)]
    unheard: u64,
    /// For tests: the wakes that found no read and the queue open.
    #[cfg(test)]
    futile: u64,
}

/// What a reader found in the queue without waiting.
enum Looked<T> {
    Read(T),
    Empty,
    Closed,
}

impl<T> ReadQueue<T> {
    fn new(mutants: Mutants) -> Self {
        Self {
            state: Mutex::new(Queued {
                reads: VecDeque::new(),
                closed: false,
                waiting: 0,
                #[cfg(test)]
                woke: 0,
                #[cfg(test)]
                unheard: 0,
                #[cfg(test)]
                futile: 0,
            }),
            ready: std::sync::Condvar::new(),
            mutants,
            #[cfg(test)]
            between: Mutex::new(None),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Queued<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queue a read and wake one waiting reader; the read back if the
    /// queue is closed.
    fn push(&self, read: T) -> Result<(), T> {
        let mut state = self.lock();
        if state.closed {
            return Err(read);
        }
        state.reads.push_back(read);
        // With no reader waiting, every reader is busy and looks before
        // it waits again: the wake is left out.
        let wake = state.waiting > 0;
        #[cfg(test)]
        if !wake {
            state.unheard += 1;
        }
        drop(state);
        if self.mutants.has(Mutants::READ_WAKES_ALL) {
            self.ready.notify_all();
        } else if wake {
            self.ready.notify_one();
        }
        Ok(())
    }

    /// No more reads: the readers take what is queued and then end.
    fn close(&self) {
        self.lock().closed = true;
        self.ready.notify_all();
    }

    /// The next read, if one is queued, without waiting.
    fn look(&self) -> Looked<T> {
        let mut state = self.lock();
        match state.reads.pop_front() {
            Some(read) => Looked::Read(read),
            None if state.closed => Looked::Closed,
            None => Looked::Empty,
        }
    }

    /// The next read, waiting for one; `None` once the queue is closed
    /// and empty.
    fn next(&self) -> Option<T> {
        let mut state = self.lock();
        loop {
            if let Some(read) = state.reads.pop_front() {
                return Some(read);
            }
            if state.closed {
                return None;
            }
            let lost = self.mutants.has(Mutants::READ_WAKE_LOST);
            if lost {
                // The defect: the look and the wait under two holds.
                drop(state);
                #[cfg(test)]
                self.between_look_and_wait(false);
                state = self.lock();
            } else {
                #[cfg(test)]
                self.between_look_and_wait(true);
            }
            state.waiting += 1;
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
            state.waiting -= 1;
            #[cfg(test)]
            {
                state.woke += 1;
                if state.reads.is_empty() && !state.closed {
                    state.futile += 1;
                }
            }
        }
    }

    #[cfg(test)]
    fn between_look_and_wait(&self, holding: bool) {
        let hook = self
            .between
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(hook) = hook {
            hook(holding);
        }
    }
}

/// The readers [`Store::serve_with`] has started, until a [`Served`]
/// holds them. On any other way out of it (a later reader or the worker
/// that cannot be started, a panic) the queue of reads is closed and the
/// readers are joined: nothing else would close it, and they would wait
/// for a read for ever, with their handles on the store's files. (With
/// the channel the readers had before, the sender's drop did that.)
struct Starting {
    reads: Arc<ReadQueue<ToReader>>,
    readers: Vec<thread::JoinHandle<()>>,
    handed: bool,
    mutants: Mutants,
}

impl Starting {
    /// The readers, for the [`Served`] that now closes their queue.
    fn hand_over(mut self) -> Vec<thread::JoinHandle<()>> {
        self.handed = true;
        std::mem::take(&mut self.readers)
    }
}

impl Drop for Starting {
    fn drop(&mut self) {
        if self.handed || self.mutants.has(Mutants::STARTED_READERS_LEFT) {
            return;
        }
        self.reads.close();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

/// For tests: counts a reader's thread as ended when it is dropped, at
/// the thread's end, however it ends.
#[cfg(test)]
struct Leaving(Arc<std::sync::atomic::AtomicUsize>);

#[cfg(test)]
impl Drop for Leaving {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// [`thread::Builder::spawn`], or, when `refused` (for tests), the
/// error of a thread that cannot be started.
trait SpawnUnless {
    fn spawn_unless<F, T>(self, refused: bool, f: F) -> io::Result<thread::JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static;
}

impl SpawnUnless for thread::Builder {
    fn spawn_unless<F, T>(self, refused: bool, f: F) -> io::Result<thread::JoinHandle<T>>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        if refused {
            return Err(io::Error::other("a test refused to start the thread"));
        }
        self.spawn(f)
    }
}

/// Where a run of a read's blocks is, as the reader found it under the
/// lock: resolved as far as a file and a place in it, so that nothing
/// is asked of the index or of the log again.
enum Found {
    /// In the payload of the write record at `record`, `skip` bytes
    /// in, in the segment file of number `file`; `unflushed` if the
    /// record was not durable at the lookup.
    Log {
        file: usize,
        record: Lsn,
        skip: u64,
        unflushed: bool,
    },
    Zero,
    Home,
}

/// Why the reader did not read.
enum Unread {
    /// The request is not valid, or the volume has failed: answered
    /// with this, and nothing was read.
    Refused(BlkError),
    /// One of the store's files could not be read.
    Fault,
}

/// A reader of a served store: it executes reads, on a thread of its
/// own, with handles of its own on the store's files (design §5.1
/// "Reads leave the worker"). It writes nothing. A served store has
/// [`OpenOptions::readers`] of them, which take the reads from one
/// queue: a read goes to whichever reader is idle, and reads complete
/// in any order. That is no change to what a read promises: a read
/// submitted after a write was acknowledged looks up after it,
/// whichever reader takes it.
struct Reader<D: Dir> {
    dir: D,
    home: D::File,
    /// The log's segment files it has opened, by number. A file keeps
    /// its name for as long as the store lives: the log reuses its
    /// files and removes none.
    segments: BTreeMap<usize, D::File>,
    identity: crate::format::Identity,
    geometry: Geometry,
    shared: Arc<Shared>,
    /// Reads of records that were not durable yet: see
    /// [`Timing::read_unflushed`].
    unflushed: [u64; 3],
    /// Where a record's whole payload is read to be checked.
    scratch: Vec<u8>,
    mutants: Mutants,
    #[cfg(any(test, feature = "test-hooks"))]
    pause: Option<StepPause>,
    #[cfg(any(test, feature = "test-hooks"))]
    pauses_at: Vec<(u64, StepPause)>,
}

impl<D: Dir> Reader<D> {
    /// A read: each block from the record the index names, or from
    /// the home image, as [`Core::read`] does on the worker.
    ///
    /// The read was submitted before this is called, so the lookup
    /// sees every operation that was acknowledged before the read was
    /// received: the worker points the index at a record before it
    /// acknowledges it. An operation in flight is seen or not, whole:
    /// the lookup is of one moment (rule R3 asks less, block by
    /// block).
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), Unread> {
        if self.shared.failed.load(Ordering::SeqCst)
            && !self.mutants.has(Mutants::SERVE_AFTER_FAILURE)
        {
            return Err(Unread::Refused(BlkError::Failed));
        }
        let start = offset / BLOCK_SIZE;
        let blocks = (buf.len() / BLOCK_USIZE) as u64;
        // The pin, from before the lookup to the end of the last read
        // of a file: a step waits for it before it writes into the
        // home image and before a segment file it freed is used again.
        // Taken after the lookup, a step could drop the index and pass
        // its barrier between the two, and the reader would read a
        // file the step had given up.
        let shared = self.shared.clone();
        let late = self.mutants.has(Mutants::PIN_AFTER_LOOKUP);
        let _pin = (!late).then(|| shared.pinned());
        let mut runs = Vec::new();
        if self.mutants.has(Mutants::READ_HOME_ONLY) {
            runs.push((start, blocks, Found::Home));
        } else {
            // The lock, for the lookup alone.
            let lookup = shared.read();
            let durable = Lsn(shared.durable.load(Ordering::SeqCst));
            for (run, len, source) in lookup.index.lookup(start, blocks) {
                let found = match source {
                    Some(Source::Data { record, first }) => {
                        let base = self.identity.base_of(record);
                        match lookup.segments.get(&base) {
                            Some(&file) => Found::Log {
                                file,
                                record,
                                skip: first * BLOCK_SIZE,
                                unflushed: record >= durable,
                            },
                            // The index names a record whose segment
                            // the log does not have: the store's own
                            // state is not what it must be.
                            None => return Err(Unread::Fault),
                        }
                    }
                    Some(Source::Zero { .. }) => Found::Zero,
                    None => Found::Home,
                };
                runs.push((run, len, found));
            }
        }
        #[cfg(any(test, feature = "test-hooks"))]
        {
            if let Some(pause) = &self.pause {
                pause.wait();
            }
            for (block, pause) in &self.pauses_at {
                if *block == start {
                    pause.wait();
                }
            }
        }
        let _late_pin = late.then(|| shared.pinned());
        let mut checked_any = false;
        for (run, len, found) in runs {
            let from = ((run - start) * BLOCK_SIZE) as usize;
            let part = &mut buf[from..from + (len * BLOCK_SIZE) as usize];
            let read = match found {
                Found::Log {
                    file,
                    record,
                    skip,
                    unflushed,
                } => {
                    if let std::collections::btree_map::Entry::Vacant(unopened) =
                        self.segments.entry(file)
                    {
                        match self.dir.open(&crate::log::slot_name(file)) {
                            Ok(opened) => {
                                unopened.insert(opened);
                            }
                            Err(_) => return Err(Unread::Fault),
                        }
                    }
                    let Some(file) = self.segments.get(&file) else {
                        return Err(Unread::Fault);
                    };
                    if unflushed && !self.mutants.has(Mutants::UNFLUSHED_READ_UNCHECKED) {
                        // Checked against the record's checksum: see
                        // [`Shared`].
                        if !checked_any {
                            self.unflushed[0] += 1;
                            checked_any = true;
                        }
                        let checked = crate::log::read_checked_payload_in(
                            file,
                            &self.identity,
                            record,
                            skip,
                            part,
                            &mut self.scratch,
                        );
                        match checked {
                            Ok(Some(beyond)) => {
                                self.unflushed[1] += beyond;
                                true
                            }
                            Ok(None) => {
                                self.unflushed[2] += 1;
                                false
                            }
                            Err(_) => false,
                        }
                    } else {
                        crate::log::read_payload_in(file, &self.identity, record, skip, part)
                            .is_ok()
                    }
                }
                Found::Zero => {
                    part.fill(0);
                    true
                }
                Found::Home => self.home.read_at(run * BLOCK_SIZE, part).is_ok(),
            };
            if !read {
                return Err(Unread::Fault);
            }
        }
        Ok(())
    }

    /// Execute a read request: its result and its buffer, or the
    /// buffer back if a file could not be read.
    fn execute(&mut self, op: Op) -> Result<Done, Buf> {
        if let Err(rejected) = self.geometry.validate(&op) {
            return Ok((Err(rejected), buffer_of(op)));
        }
        let Op::Read { offset, mut buf } = op else {
            // Not reached: only reads are handed to the reader.
            return Ok((Err(BlkError::Failed), buffer_of(op)));
        };
        match self.read(offset, &mut buf) {
            Ok(()) => Ok((Ok(()), Some(buf))),
            Err(Unread::Refused(why)) => Ok((Err(why), Some(buf))),
            // A defect on purpose: the error is served and the volume
            // goes on.
            Err(Unread::Fault) if self.mutants.has(Mutants::READ_ERROR_SERVED) => {
                Ok((Err(BlkError::Failed), Some(buf)))
            }
            Err(Unread::Fault) => Err(buf),
        }
    }
}

/// What the reader shows of its time: see [`Timing`].
#[derive(Debug, Clone, Copy, Default)]
struct ReaderShown {
    requests: RequestTiming,
    wait_by: [u64; ACTIVITY_COUNT],
    longest_wait_cause: u64,
    longest_wait_cause_ns: u64,
    time: [u64; 3],
    unflushed: [u64; 3],
    /// The span the reader has open.
    open: Open,
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
    /// To the readers: every read goes there, and nothing else.
    reads: Option<Arc<ReadQueue<ToReader>>>,
    completions: mpsc::Receiver<(Completion, Option<Lsn>, Instant)>,
    /// When the worker had each completion of the last
    /// [`BlockTarget::poll`] or [`BlockTarget::wait`] ready.
    ready: Vec<(RequestId, Instant)>,
    ids: BTreeSet<RequestId>,
    acknowledged: Option<Lsn>,
    /// Set by the worker as it ends, however it ends.
    ended: Arc<AtomicBool>,
    /// What the worker shows of the log.
    log: Arc<Shown>,
    worker: Option<thread::JoinHandle<()>>,
    readers: Vec<thread::JoinHandle<()>>,
}

/// The log's state as the worker last published it: read by the owner
/// without a round trip through the worker, which a step can keep busy.
///
/// It is one value behind a lock that is held for a copy and for
/// nothing else, so a reader gets the state of one moment: never a
/// start beyond the end (a step can move the start past the end that
/// was published before it), never a step that has ended as under way.
///
/// With the times, each thread shows the span it has open (since when,
/// and whether it is a wait for work, which a look closes): see
/// [`Timing::worker_as_of_ns`]. A thread that wakes from a wait closes
/// it and shows that in one hold of its lock, which is the lock a look
/// holds while it reads the clock: so a look never counts more of a
/// wait than the thread then does. The last field is the moment the
/// times are counted from.
#[derive(Debug)]
struct Shown(
    Mutex<(LogState, Timing, Open)>,
    Mutex<Vec<ReaderShown>>,
    Instant,
);

/// The span a thread has open: since when, and doing what (an
/// [`Activity`]).
type Open = Option<(Instant, usize)>;

impl Default for Shown {
    fn default() -> Self {
        Self(Mutex::default(), Mutex::default(), Instant::now())
    }
}

impl Shown {
    /// The worker's state and times, with the span it has open.
    fn publish<D: Dir>(&self, core: &Core<D>, stepping: bool, open: Open) {
        let state = LogState {
            stepping: stepping || core.handed.is_some(),
            ..core.log_state()
        };
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = (state, core.timing, open);
    }

    #[cfg(test)]
    fn set(&self, state: LogState) {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).0 = state;
    }

    /// A step begins: everything else stays, and it is under way.
    fn begin(&self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .stepping = true;
    }

    fn state(&self) -> LogState {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).0
    }

    /// Reader `at`'s part of the times, as it last showed it.
    fn read(&self, at: usize, reader: ReaderShown) {
        let mut shown = self.1.lock().unwrap_or_else(PoisonError::into_inner);
        if shown.len() <= at {
            shown.resize(at + 1, ReaderShown::default());
        }
        shown[at] = reader;
    }

    /// The worker woke from a wait for work: the wait is closed and
    /// shown in one hold of the lock (see [`Shown`]).
    fn worker_woke(&self, spans: &mut Spans, timing: &mut Timing) {
        let mut shown = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        spans.close(Activity::IDLE, Instant::now(), timing);
        shown.1 = *timing;
        shown.2 = spans.open(Activity::OTHER);
    }

    /// The reader woke from a wait for work: `close` closes the wait at
    /// the moment it is given and says what the reader shows then, in
    /// one hold of the lock (see [`Shown`]).
    fn reader_woke(&self, at: usize, close: impl FnOnce(Instant) -> ReaderShown) {
        let mut shown = self.1.lock().unwrap_or_else(PoisonError::into_inner);
        let now = close(Instant::now());
        if let Some(reader) = shown.get_mut(at) {
            *reader = now;
        }
    }

    fn timing(&self) -> Timing {
        self.both().1
    }

    /// The log's state and the worker's times of one moment, with the
    /// readers' times as they last showed them: reads are the readers'.
    /// A thread's wait for work is closed at the look, which reads the
    /// clock with both shown values held; anything else a thread has
    /// open is left to it.
    ///
    /// The readers' times are summed over the readers, each thread's
    /// complete up to its own moment (see [`Timing::reader_as_of_ns`]).
    fn both(&self) -> (LogState, Timing) {
        let worker = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        let readers = self.1.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let (state, mut timing, open) = *worker;
        let ns = |from: Instant, to: Instant| {
            u64::try_from(to.saturating_duration_since(from).as_nanos()).unwrap_or(u64::MAX)
        };
        // Up to when a thread's sums are complete, the wait it is in
        // counted.
        let complete = |open: Open, idle: &mut u64| match open {
            Some((since, Activity::IDLE)) => {
                *idle = idle.saturating_add(ns(since, now));
                ns(self.2, now)
            }
            Some((since, _)) => ns(self.2, since),
            None => 0,
        };
        timing.worker_as_of_ns = complete(open, &mut timing.worker[Activity::IDLE]);
        let (mut reads, mut wait_by) = (RequestTiming::default(), [0u64; ACTIVITY_COUNT]);
        let (mut time, mut unflushed, mut as_of) = ([0u64; 3], [0u64; 3], 0u64);
        let mut longest = (0u64, 0u64, 0u64);
        for reader in readers.iter() {
            let mut reader = *reader;
            as_of = as_of.saturating_add(complete(reader.open, &mut reader.time[0]));
            if reader.requests.wait.count > 0 && reader.requests.wait.max_ns >= longest.0 {
                longest = (
                    reader.requests.wait.max_ns,
                    reader.longest_wait_cause,
                    reader.longest_wait_cause_ns,
                );
            }
            reads.wait.merge(&reader.requests.wait);
            reads.call.merge(&reader.requests.call);
            for (sum, ns) in wait_by.iter_mut().zip(reader.wait_by) {
                *sum = sum.saturating_add(ns);
            }
            for (sum, ns) in time.iter_mut().zip(reader.time) {
                *sum = sum.saturating_add(ns);
            }
            for (sum, n) in unflushed.iter_mut().zip(reader.unflushed) {
                *sum = sum.saturating_add(n);
            }
        }
        timing.reader_as_of_ns = as_of;
        timing.requests[0] = reads;
        timing.wait_by[0] = wait_by;
        timing.longest_wait_cause[0] = longest.1;
        timing.longest_wait_cause_ns[0] = longest.2;
        timing.reader = time;
        timing.read_unflushed = unflushed;
        (state, timing)
    }
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
    D: Dir + Clone + Send + 'static,
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
            unacknowledged: _,
            replayed: _,
            mut queue,
            ids,
        } = self;
        let geometry = core.geometry;
        let identity = core.identity();
        let (commands, inbox) = mpsc::channel::<Command>();
        // A served store hands the home image's sync of its rule's steps
        // to the helper, which wakes the worker when it is done. A step
        // the store handed over before it was served ends first: nothing
        // would wake the worker for it.
        let _ = core.settle();
        core.hands_over = true;
        let reported = Mutex::new(commands.clone());
        core.wake = Some(Arc::new(move || {
            let _ = reported
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .send(Command::Reported);
        }));
        let (outbox, completions) = mpsc::channel::<(Completion, Option<Lsn>, Instant)>();
        let ended = Arc::new(AtomicBool::new(false));
        let shown = Arc::new(Shown::default());
        // Each thread's time is counted from here, and shown from here
        // with the span it has open, so that every look holds it whole.
        let spans = Spans::new();
        let read_spans: Vec<Spans> = (0..core.readers).map(|_| Spans::new()).collect();
        shown.publish(&core, false, spans.open(Activity::OTHER));
        for (at, read_spans) in read_spans.iter().enumerate() {
            shown.read(
                at,
                ReaderShown {
                    open: read_spans.open(Activity::OTHER),
                    ..ReaderShown::default()
                },
            );
        }
        let log = shown.clone();
        let ending = Ending {
            ended: ended.clone(),
            waker: waker.clone(),
        };
        // The readers: threads of their own for the reads, each with
        // handles of its own on the store's files. Any thread's end is
        // the store's end for its owner.
        // One queue of reads for all the readers: a reader that is idle
        // takes the next read, whichever reader is busy with another.
        let reads = Arc::new(ReadQueue::<ToReader>::new(core.mutants));
        let dir = core.log.dir().clone();
        #[cfg(any(test, feature = "test-hooks"))]
        let (pause, pauses_at) = (
            core.read_pause.take(),
            std::mem::take(&mut core.read_pauses_at),
        );
        let mut readers = Vec::new();
        for _ in 0..core.readers {
            readers.push(Reader {
                dir: dir.clone(),
                home: dir.open(HOME_NAME)?,
                segments: BTreeMap::new(),
                identity: *core.log.identity(),
                geometry,
                shared: core.shared.clone(),
                unflushed: [0; 3],
                scratch: Vec::new(),
                mutants: core.mutants,
                #[cfg(any(test, feature = "test-hooks"))]
                pause: pause.clone(),
                #[cfg(any(test, feature = "test-hooks"))]
                pauses_at: pauses_at.clone(),
            });
        }
        // What the open left queued counts from here: the reads go to
        // the readers, the rest to the worker, each in its order.
        let served_at = Instant::now();
        let mut waiting = VecDeque::new();
        for request in queue.drain(..) {
            if matches!(request.op, Op::Read { .. }) {
                let _ = reads.push(ToReader::Read(request, served_at));
            } else {
                waiting.push_back((request, served_at));
            }
        }
        #[cfg(test)]
        let refused = core.spawn_fails_at;
        #[cfg(not(test))]
        let refused: Option<usize> = None;
        let reader_count = core.readers;
        let mut starting = Starting {
            reads: reads.clone(),
            readers: Vec::new(),
            handed: false,
            mutants: core.mutants,
        };
        for (at_index, (mut reader, read_spans)) in readers.into_iter().zip(read_spans).enumerate()
        {
            let ending = Ending {
                ended: ended.clone(),
                waker: waker.clone(),
            };
            let (outbox, waker, shown) = (outbox.clone(), waker.clone(), shown.clone());
            let failures = commands.clone();
            let read_inbox = reads.clone();
            #[cfg(test)]
            let leaving = core.readers_ended.clone();
            let spawned = thread::Builder::new()
                .name(format!("skyzfs-reader-{at_index}"))
                .spawn_unless(refused == Some(at_index), move || {
                    let _ending = ending;
                    #[cfg(test)]
                    let _leaving = Leaving(leaving);
                    // What the reader did, for the waits of its reads:
                    // idle, the call of a read, anything else.
                    let mut spans = read_spans;
                    let mut timing = Timing::default();
                    // What the reader shows of its times, with the span
                    // it has open.
                    let show = |timing: &Timing, unflushed: [u64; 3], open: Open| ReaderShown {
                        requests: timing.requests[0],
                        wait_by: timing.wait_by[0],
                        longest_wait_cause: timing.longest_wait_cause[0],
                        longest_wait_cause_ns: timing.longest_wait_cause_ns[0],
                        time: [
                            timing.worker[Activity::IDLE],
                            timing.worker[Activity::call(0)],
                            timing.worker[Activity::OTHER],
                        ],
                        unflushed,
                        open,
                    };
                    loop {
                        // A read that is there already, taken without a
                        // wait; else the reader shows itself idle and
                        // waits for one.
                        let next = match read_inbox.look() {
                            Looked::Read(next) => next,
                            Looked::Closed => return,
                            Looked::Empty => {
                                spans.forget(None);
                                spans.close(Activity::OTHER, Instant::now(), &mut timing);
                                // Shown as waiting, so that a look counts
                                // the wait as idle.
                                shown.read(
                                    at_index,
                                    show(&timing, reader.unflushed, spans.open(Activity::IDLE)),
                                );
                                let next = read_inbox.next();
                                shown.reader_woke(at_index, |now| {
                                    spans.close(Activity::IDLE, now, &mut timing);
                                    show(&timing, reader.unflushed, spans.open(Activity::OTHER))
                                });
                                match next {
                                    Some(next) => next,
                                    None => return,
                                }
                            }
                        };
                        let (Request { id, op }, at) = match next {
                            ToReader::Read(request, at) => (request, at),
                            #[cfg(test)]
                            ToReader::Panic => panic!("a test asked the reader to panic"),
                        };
                        let began = Instant::now();
                        spans.close(Activity::OTHER, began, &mut timing);
                        let by = spans.between(at);
                        let executed = reader.execute(op);
                        let ended = Instant::now();
                        spans.close(Activity::call(0), ended, &mut timing);
                        let wait = began.saturating_duration_since(at);
                        let longest = wait.as_nanos() >= u128::from(timing.requests[0].wait.max_ns);
                        let mut most = 0;
                        for (activity, ns) in by.iter().enumerate() {
                            timing.wait_by[0][activity] += ns;
                            if *ns > by[most] {
                                most = activity;
                            }
                        }
                        if longest {
                            timing.longest_wait_cause[0] = most as u64;
                            timing.longest_wait_cause_ns[0] = by[most];
                        }
                        timing.requests[0].wait.add(wait);
                        timing.requests[0].call.add(ended - began);
                        // Shown before the completion can be taken, as
                        // the worker does.
                        shown.read(
                            at_index,
                            show(&timing, reader.unflushed, spans.open(Activity::OTHER)),
                        );
                        match executed {
                            Ok((result, buf)) => {
                                let completion = Completion { id, result, buf };
                                if outbox.send((completion, None, ended)).is_err() {
                                    return;
                                }
                                if let Some(Waker(wake)) = &waker {
                                    wake();
                                }
                            }
                            // A file of the store's could not be read:
                            // the worker fails the volume, marks the
                            // log and answers this read, in that
                            // order, so the mark is there before the
                            // first failed completion goes out.
                            Err(buf) => {
                                if failures.send(Command::ReadFailed(id, buf)).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            // On an early return, `starting` closes the queue and joins
            // the readers already started.
            starting.readers.push(spawned?);
        }
        let worker = thread::Builder::new()
            .name("skyzfs-store".into())
            .spawn_unless(refused == Some(reader_count), move || {
                let _ending = ending;
                // Whether the request at the head was tried and cannot
                // proceed until something changes.
                let mut held = false;
                // Whether the rule of the marks is to be asked, request
                // or no request: at the start, for a long log that the
                // open found, and after every step it took.
                let mut busy = true;
                // The requests that wait, each with the time of its
                // submission, in one queue: a request that is put back
                // goes back with its time.
                let mut queue: VecDeque<(Request, Instant)> = waiting;
                // What the worker did, for the waits of the requests.
                let mut spans = spans;
                core.parts = Some(Vec::new());
                let wake = || {
                    if let Some(Waker(wake)) = &waker {
                        wake();
                    }
                };
                loop {
                    // The helper's report of a step's sync, if it has
                    // come: the step is taken up (its new start, the
                    // index's drop, the second barrier), and the rule
                    // is asked again.
                    if let Some(reported) = core.report(false) {
                        spans.take(&mut core.parts, Activity::OTHER, &mut core.timing);
                        if core.helper_ended {
                            return;
                        }
                        // A failed sync has failed the volume: a held
                        // request gets its answer now.
                        held &= reported.is_ok();
                        busy = true;
                        shown.publish(&core, false, spans.open(Activity::OTHER));
                        wake();
                    }
                    // Take what has arrived; wait if there is nothing
                    // to do. What is shown says which, so that a look
                    // counts the wait as idle.
                    let waits = (queue.is_empty() || held) && !busy;
                    if waits {
                        spans.close(Activity::OTHER, Instant::now(), &mut core.timing);
                    }
                    let doing = if waits {
                        Activity::IDLE
                    } else {
                        Activity::OTHER
                    };
                    shown.publish(&core, false, spans.open(doing));
                    let command = if waits {
                        // Where nothing wakes the worker when the sync
                        // ends (the simulated disk), it looks again
                        // every millisecond while it waits.
                        let looks = core.handed.is_some() && !core.home.sync_wakes();
                        let command = if looks {
                            match inbox.recv_timeout(LOOK_AGAIN) {
                                Ok(command) => Some(command),
                                Err(mpsc::RecvTimeoutError::Timeout) => None,
                                Err(mpsc::RecvTimeoutError::Disconnected) => Some(Command::Stop),
                            }
                        } else {
                            inbox.recv().ok().or(Some(Command::Stop))
                        };
                        shown.worker_woke(&mut spans, &mut core.timing);
                        command
                    } else {
                        match inbox.try_recv() {
                            Ok(command) => Some(command),
                            Err(mpsc::TryRecvError::Empty) => None,
                            Err(mpsc::TryRecvError::Disconnected) => Some(Command::Stop),
                        }
                    };
                    match command {
                        Some(Command::Reported) => continue,
                        Some(Command::Stop) => {
                            // The end waits for a step's sync that is
                            // with the helper, and takes the step up.
                            let _ = core.settle();
                            spans.take(&mut core.parts, Activity::OTHER, &mut core.timing);
                            shown.publish(&core, false, spans.open(Activity::OTHER));
                            return;
                        }
                        Some(Command::Request(request, at)) => {
                            queue.push_back((request, at));
                            continue;
                        }
                        Some(Command::Hold(hold, reply)) => {
                            let changed = core.set_hold(hold);
                            // The wait for a step's report, if any.
                            spans.take(&mut core.parts, Activity::OTHER, &mut core.timing);
                            held = false;
                            // A hold that moved or went can leave the
                            // rule of the marks something to take with
                            // no request to follow: it is asked again.
                            // The other events that change what it
                            // finds: an explicit checkpoint (below; it
                            // can leave a log that is long and not
                            // durable), a request (asked after each),
                            // and a step (asked until one takes
                            // nothing). The position and the failure
                            // hook leave it nothing new.
                            busy |= changed.is_ok();
                            // As for a checkpoint: the change can have
                            // taken a step up, and whoever is answered
                            // finds the log as that left it.
                            shown.publish(&core, false, spans.open(Activity::OTHER));
                            let _ = reply.send(changed);
                            continue;
                        }
                        Some(Command::Checkpoint(reply)) => {
                            let result = if core.failed {
                                Err(BlkError::Failed)
                            } else {
                                core.checkpoint()
                            };
                            spans.take(&mut core.parts, Activity::OTHER, &mut core.timing);
                            held = false;
                            busy = true;
                            // As for a completion: whoever is answered
                            // finds the log as the checkpoint left it.
                            shown.publish(&core, false, spans.open(Activity::OTHER));
                            let _ = reply.send(result);
                            continue;
                        }
                        Some(Command::Position(reply)) => {
                            let _ = reply.send(core.log.tail());
                            continue;
                        }
                        Some(Command::ReadFailed(id, buf)) => {
                            // As a failed sync does: the volume has
                            // failed and the log is marked, and only
                            // then is the read answered. A request
                            // that was held gets its answer now.
                            core.fail();
                            held = false;
                            shown.publish(&core, false, spans.open(Activity::OTHER));
                            let completion = Completion {
                                id,
                                result: Err(BlkError::Failed),
                                buf: Some(buf),
                            };
                            if outbox.send((completion, None, Instant::now())).is_err() {
                                return;
                            }
                            if let Some(Waker(wake)) = &waker {
                                wake();
                            }
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
                            // The other wakes need nothing: this one
                            // changes nothing that is shown (shown
                            // here all the same, for the one order),
                            // a step shows its begin and its end
                            // before each of its wakes, and the
                            // worker's end changes nothing after the
                            // last state shown.
                            shown.publish(&core, false, spans.open(Activity::OTHER));
                            // The owner is woken before the caller is
                            // answered: when `fail_now` returns, the
                            // owner has been told.
                            if let Some(Waker(wake)) = &waker {
                                wake();
                            }
                            let _ = reply.send(());
                            continue;
                        }
                        // Nothing more has arrived: every request
                        // that waits is in the queue.
                        None => spans.forget(queue.front().map(|(_, at)| *at)),
                    }
                    // At or above the high mark no request is executed
                    // while the rule still takes steps: `busy` says
                    // that its last step took something, or that it is
                    // yet to be asked. A rule that finds nothing to
                    // take (a hold, or a failed volume) does not keep
                    // a request waiting: the request is executed, and
                    // is held or refused by what it finds.
                    let steps_first = busy && core.at_the_high_mark();
                    let gated = steps_first && !held && !queue.is_empty();
                    let next = if held || steps_first {
                        None
                    } else {
                        queue.pop_front()
                    };
                    if let Some((Request { id, op }, at)) = next {
                        let kind = timed_kind(&op);
                        let began = Instant::now();
                        spans.close(Activity::OTHER, began, &mut core.timing);
                        let helping = core.handed.is_some();
                        let executed = core.execute(op);
                        let ended = Instant::now();
                        // What it waited for, before its own call is
                        // among the spans.
                        let by = executed.is_ok().then(|| spans.between(at));
                        // A call that made room in the log took a
                        // whole checkpoint inside itself: those parts
                        // are the step's in the worker's time, and in
                        // the wait of a request behind this one, and
                        // the kind's is the time outside them.
                        spans.take(&mut core.parts, Activity::call(kind), &mut core.timing);
                        spans.close(Activity::call(kind), ended, &mut core.timing);
                        match executed {
                            Ok((result, buf)) => {
                                let wait = began.saturating_duration_since(at);
                                let timing = &mut core.timing;
                                if let Some(by) = by {
                                    let longest = wait.as_nanos()
                                        >= u128::from(timing.requests[kind].wait.max_ns);
                                    let mut most = 0;
                                    for (activity, ns) in by.iter().enumerate() {
                                        timing.wait_by[kind][activity] += ns;
                                        if *ns > by[most] {
                                            most = activity;
                                        }
                                    }
                                    if longest {
                                        timing.longest_wait_cause[kind] = most as u64;
                                        timing.longest_wait_cause_ns[kind] = by[most];
                                    }
                                }
                                timing.requests[kind].wait.add(wait);
                                timing.requests[kind].call.add(ended - began);
                                if helping {
                                    timing.helper_requests += 1;
                                }
                                let start = core.appended.take();
                                let completion = Completion { id, result, buf };
                                // Shown before the completion can be
                                // taken: an owner that is woken for a
                                // completion finds a log state that
                                // has it. Shown only at the loop's
                                // top, the state was one request
                                // behind for an owner that looked at
                                // once, and stayed so while nothing
                                // else woke it.
                                shown.publish(&core, false, spans.open(Activity::OTHER));
                                if outbox.send((completion, start, ended)).is_err() {
                                    return;
                                }
                                if let Some(Waker(wake)) = &waker {
                                    wake();
                                }
                            }
                            Err(op) => {
                                queue.push_front((Request { id, op }, at));
                                held = true;
                            }
                        }
                    }
                    // The rule of the marks: one step after every
                    // request and whenever there is none, and at the
                    // high mark steps until the log is below it before
                    // the next request (above). The owner is woken
                    // when a step begins, which is before the rule's
                    // own sync of the log if it needs one, and when it
                    // ends, so that it can show both.
                    let mut begun = false;
                    let turn = gated.then(Instant::now);
                    let stepped = core.tend(&mut || {
                        begun = true;
                        shown.begin();
                        wake();
                    });
                    spans.take(&mut core.parts, Activity::OTHER, &mut core.timing);
                    if core.helper_ended {
                        return;
                    }
                    if let Some(turn) = turn.filter(|_| stepped != Ok(false)) {
                        core.timing.gate.add(turn.elapsed());
                    }
                    busy = stepped == Ok(true);
                    // `begun` is a safeguard and cannot decide today: the
                    // rule tells the owner of a step before its own sync
                    // only when that sync makes more durable below the
                    // hold, so a step follows (`Ok(true)`) or the sync
                    // fails the volume (an error). Should the rule ever
                    // begin and then take nothing, the owner is still
                    // shown the end of what it was told had begun.
                    if stepped != Ok(false) || begun {
                        // Only a safeguard: a request is held when
                        // the log is full behind a hold, after the log
                        // was made durable and checkpointed up to the
                        // hold, so the rule finds nothing to take and
                        // nothing to sync while one is held, and never
                        // gets here. If that ever changes, the request
                        // is tried again and not left waiting.
                        held = false;
                        shown.publish(&core, false, spans.open(Activity::OTHER));
                        wake();
                    }
                }
            })?;
        Ok(Served {
            geometry,
            identity,
            commands: Some(commands),
            reads: Some(reads),
            completions,
            ready: Vec::new(),
            ids,
            acknowledged,
            ended,
            log,
            worker: Some(worker),
            readers: starting.hand_over(),
        })
    }
}

impl Served {
    fn send(&self, command: Command) -> bool {
        self.commands
            .as_ref()
            .is_some_and(|commands| commands.send(command).is_ok())
    }

    /// The log as the worker last showed it: its start, end and
    /// durable position, the steps the rule of the marks has taken
    /// ([`Store::set_marks`]), and whether one is under way. It asks
    /// the worker nothing and returns at once, also during a step. All
    /// of it is of one moment: the start is never beyond the end, a
    /// step that has ended is never shown under way, and one that
    /// waits in its hook is.
    pub fn log_state(&self) -> LogState {
        self.log.state()
    }

    /// Where the time went, as the worker last showed it with the
    /// log's state: see [`Timing`]. Like [`Served::log_state`] it asks
    /// the worker nothing and returns at once.
    pub fn timing(&self) -> Timing {
        self.log.timing()
    }

    /// When the worker had the completion of request `id` ready: the
    /// end of the store's call for it. Known for the completions that
    /// the last [`BlockTarget::poll`] or [`BlockTarget::wait`]
    /// returned, so that their taker can say how long each waited to
    /// be taken.
    pub fn ready_at(&self, id: RequestId) -> Option<Instant> {
        self.ready
            .iter()
            .find(|(ready, _)| *ready == id)
            .map(|(_, at)| *at)
    }

    /// The log's state and the times of one moment: what the worker
    /// showed together. Two calls, one for each, can be a request
    /// apart.
    pub fn shown(&self) -> (LogState, Timing) {
        self.log.both()
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

    /// Whether the worker or the reader has ended, which only a panic
    /// in it does while the store is served. Nothing more will complete: requests
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

    /// For tests: make a reader panic, and wait until it has: until
    /// the store shows that it has ended.
    #[cfg(test)]
    pub(crate) fn panic_reader(&mut self) {
        if let Some(reads) = &self.reads {
            let _ = reads.push(ToReader::Panic);
        }
        while !self.ended.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn took(&mut self, taken: (Completion, Option<Lsn>, Instant), out: &mut Vec<Completion>) {
        let (completion, start, ready) = taken;
        self.ready.push((completion.id, ready));
        self.ids.remove(&completion.id);
        if let Some(start) = start {
            self.acknowledged = Some(start);
        }
        out.push(completion);
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        // The readers end when the queue of reads is closed, once they
        // have taken what is queued, and the worker when it is told to
        // stop: it holds a sender of its commands itself, for the
        // helper's wake.
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Stop);
        }
        self.commands = None;
        if let Some(reads) = self.reads.take() {
            reads.close();
        }
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
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
        // Either thread's end is the store's end: with the worker or
        // the reader gone, nothing is taken any more, of either kind.
        if self.gone() {
            return Err(SubmitError {
                kind: SubmitErrorKind::Busy,
                request,
            });
        }
        let id = request.id;
        // A read goes to the reader and everything else to the worker.
        if matches!(request.op, Op::Read { .. }) {
            return match self.reads.as_ref() {
                Some(reads) => match reads.push(ToReader::Read(request, Instant::now())) {
                    Ok(()) => {
                        self.ids.insert(id);
                        Ok(())
                    }
                    // The queue is closed, which only the store's end
                    // does: a reader that is gone is found by `gone`.
                    Err(ToReader::Read(request, _)) => Err(SubmitError {
                        kind: SubmitErrorKind::Busy,
                        request,
                    }),
                    #[cfg(test)]
                    Err(_) => Ok(()),
                },
                None => Err(SubmitError {
                    kind: SubmitErrorKind::Busy,
                    request,
                }),
            };
        }
        match self.commands.as_ref() {
            Some(commands) => match commands.send(Command::Request(request, Instant::now())) {
                Ok(()) => {
                    self.ids.insert(id);
                    Ok(())
                }
                // The worker is gone, which only a panic in it does.
                Err(mpsc::SendError(Command::Request(request, _))) => Err(SubmitError {
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
        self.ready.clear();
        self.take_ready(out)
    }

    fn wait(&mut self, out: &mut Vec<Completion>, timeout: Duration) -> usize {
        // The stamps of the last call are forgotten once, here in
        // `poll`: what this call takes after its wait keeps its stamp.
        let taken = self.poll(out);
        if taken > 0 || self.ids.is_empty() {
            return taken;
        }
        match self.completions.recv_timeout(timeout) {
            Ok(completion) => {
                self.took(completion, out);
                1 + self.take_ready(out)
            }
            Err(_) => 0,
        }
    }
}

impl Served {
    /// Take every completion that is ready now.
    fn take_ready(&mut self, out: &mut Vec<Completion>) -> usize {
        let mut taken = 0;
        while let Ok(completion) = self.completions.try_recv() {
            self.took(completion, out);
            taken += 1;
        }
        taken
    }
}

#[cfg(test)]
mod harness;
#[cfg(test)]
mod tests;

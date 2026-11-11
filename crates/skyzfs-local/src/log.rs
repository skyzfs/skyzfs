//! The write log: append, sync, recover, replay, reclaim.
//!
//! The format, the argument for recovery and the limits are in the
//! [crate documentation](crate). This module is the log as an object that
//! one writer owns.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;

use crate::format::{
    chain_seed, Bad, Identity, Key, Kind, Lsn, RecordHeader, SegmentHeader, Superblock,
    RECORD_HEADER, SECTOR_LEN, SUPER_COPY_STRIDE,
};
use crate::io::{Dir, File};

/// The name of the file that holds the two superblock copies.
pub const SUPER_NAME: &str = "log.super";

/// The name the superblock file has while a log is being created. The
/// rename to [`SUPER_NAME`] is what makes the log exist.
const SUPER_NEW_NAME: &str = "log.super.new";

/// The name of the file whose existence says that the log has failed.
pub const MARKER_NAME: &str = "log.failed";

/// The name of the `index`-th segment file. The name says nothing about
/// which part of the log the file holds; its header does.
pub fn slot_name(index: usize) -> String {
    format!("log.{index:06}")
}

fn slot_index(name: &str) -> Option<usize> {
    let digits = name.strip_prefix("log.")?;
    if digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit()) {
        digits.parse().ok()
    } else {
        None
    }
}

/// Deliberate defects, for testing that the tests notice. Not part of
/// the API: the crate's own tests set them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u64);

impl Mutants {
    /// A record whose payload is damaged is skipped, and the records
    /// after it are accepted.
    pub(crate) const ACCEPT_AFTER_GAP: u64 = 1 << 0;
    /// Recovery does not bump the epoch.
    pub(crate) const NO_EPOCH_BUMP: u64 = 1 << 1;
    /// Recovery does not seal the end of the prefix: it writes nothing
    /// over what it found there.
    pub(crate) const NO_TAIL_ERASE: u64 = 1 << 2;
    /// Recovery does not erase segments beyond the prefix.
    pub(crate) const KEEP_STALE_SEGMENTS: u64 = 1 << 3;
    /// Records are not chained: every header is checked against a
    /// constant.
    pub(crate) const NO_CHAIN: u64 = 1 << 4;
    /// A record's epoch is not checked.
    pub(crate) const NO_EPOCH_CHECK: u64 = 1 << 5;
    /// A record's own position is not checked.
    pub(crate) const NO_POSITION_CHECK: u64 = 1 << 6;
    /// A record's payload is not checked: a torn record is accepted.
    pub(crate) const NO_DATA_CHECK: u64 = 1 << 7;
    /// Sync returns without syncing.
    pub(crate) const SYNC_EARLY: u64 = 1 << 8;
    /// Sync syncs only the segment being written.
    pub(crate) const SYNC_ONLY_TAIL: u64 = 1 << 9;
    /// Sync does not sync the directory after a segment file was created.
    pub(crate) const SYNC_NO_DIR: u64 = 1 << 10;
    /// A failed sync does not fail the log; the next one may succeed.
    pub(crate) const RETRY_SYNC: u64 = 1 << 11;
    /// Recovery validates through the cache.
    pub(crate) const VALIDATE_CACHED: u64 = 1 << 12;
    /// Recovery does not sync before it validates.
    pub(crate) const NO_SYNC_BEFORE_VALIDATE: u64 = 1 << 13;
    /// Reclaim frees segments before the new floor is durable.
    pub(crate) const REUSE_BEFORE_FLOOR_SYNC: u64 = 1 << 14;
    /// Reclaim ignores the hold.
    pub(crate) const IGNORE_HOLD: u64 = 1 << 15;
    /// No marker is written when the log fails.
    pub(crate) const NO_MARKER: u64 = 1 << 16;
    /// The epoch of a segment's header is not checked against the
    /// records around it.
    pub(crate) const NO_SEGMENT_EPOCH_CHECK: u64 = 1 << 17;
    /// The superblock is always written to the same copy.
    pub(crate) const ONE_SUPER_COPY: u64 = 1 << 18;
    /// A prefix that ends below the durable floor is accepted.
    pub(crate) const NO_DURABLE_FLOOR: u64 = 1 << 19;
    /// Of two files that claim the same part of the log, the one written
    /// in the earlier epoch is taken.
    pub(crate) const PREFER_OLDER_CLAIM: u64 = 1 << 20;
    /// A record appended after a sync or a recovery goes into the block
    /// that holds durable records, and so rewrites it.
    pub(crate) const REWRITE_DURABLE_BLOCK: u64 = 1 << 21;
    /// Headers are tagged with a key that everybody knows: a payload can
    /// be made to look like a record.
    pub(crate) const PUBLIC_KEY: u64 = 1 << 22;
    /// A read error during recovery is taken for the end of the log.
    pub(crate) const READ_ERROR_ENDS_LOG: u64 = 1 << 24;
    /// The failure marker's name is not synced.
    pub(crate) const MARKER_UNSYNCED: u64 = 1 << 25;
    /// A segment header of a later epoch than the superblock's is taken
    /// as it is.
    pub(crate) const NO_HEADER_FUTURE_CHECK: u64 = 1 << 26;
    /// A reclaim writes its superblock in the epoch it found.
    pub(crate) const RECLAIM_KEEPS_EPOCH: u64 = 1 << 27;
    /// A replay does not stop at a record that starts where the range
    /// ends.
    pub(crate) const REPLAY_PAST_START: u64 = 1 << 28;
    /// A reclaim takes any position for the start of the log.
    pub(crate) const RECLAIM_UNCHECKED: u64 = 1 << 29;
    /// A replay goes beyond the durable position.
    pub(crate) const REPLAY_UNSYNCED: u64 = 1 << 30;
    /// The failure observer is called after the marker was attempted.
    pub(crate) const OBSERVE_AFTER_MARKER: u64 = 1 << 31;
    /// A reclaim keeps the cursor it had for the log's start and calls
    /// it current.
    pub(crate) const STALE_CURSOR: u64 = 1 << 32;
    /// A position inside a segment's header passes for a boundary.
    pub(crate) const HEADER_IS_BOUNDARY: u64 = 1 << 33;
    /// The position the log must reach is compared after the recovery
    /// has sealed the prefix, and not before it writes anything.
    pub(crate) const COMPARE_AFTER_SEAL: u64 = 1 << 34;
    /// A failed log still replays what its cache shows.
    pub(crate) const REPLAY_WHEN_FAILED: u64 = 1 << 23;

    fn has(self, flag: u64) -> bool {
        self.0 & flag != 0
    }
}

/// Why the log is not what it should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Corruption {
    /// Neither superblock copy is valid.
    #[error("neither superblock copy is valid")]
    NoSuperblock,
    /// The segment the log starts in is not there.
    #[error("the segment at {base}, where the log starts, is missing")]
    MissingSegment {
        /// The first position of the missing segment.
        base: u64,
    },
    /// The log holds records, or a segment, of a later epoch than the
    /// superblock that was found: the newest superblock copy is lost, and
    /// the older one cannot say where the log starts or what is durable.
    #[error("the log holds epoch {found} at {position}, the superblock only {superblock}")]
    SuperblockBehind {
        /// Where the later epoch was found.
        position: Lsn,
        /// The epoch found there.
        found: u64,
        /// The epoch of the superblock.
        superblock: u64,
    },
    /// The valid prefix ends before a position that was durable.
    #[error("the log ends at {end}, below {durable_floor} which was durable ({stopped:?})")]
    BelowDurableFloor {
        /// Where the valid prefix ends.
        end: Lsn,
        /// The position the superblock says was durable.
        durable_floor: Lsn,
        /// Why the prefix ends there.
        stopped: Stop,
    },
    /// A record inside the log does not validate when it is read back.
    #[error("the record at {position} is damaged ({stopped:?})")]
    Record {
        /// The record's position.
        position: Lsn,
        /// What is wrong with it.
        stopped: Stop,
    },
}

/// What a log operation can fail with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    /// `create`: the directory already holds a log.
    #[error("the directory already holds a log")]
    Exists,
    /// `open`: the directory holds no log.
    #[error("the directory holds no log")]
    NotFound,
    /// `create`: the identity cannot describe a log.
    #[error("the log's parameters are not valid")]
    InvalidConfig,
    /// `create`: the operating system gave no random bytes for the
    /// log's key. Nothing was created.
    #[error("the operating system gave no random bytes for the log's key")]
    NoRandomness,
    /// `open`: the log failed in an earlier life and the caller did not
    /// acknowledge it.
    #[error("the log is marked as failed")]
    MarkedFailed,
    /// `open`: the valid prefix ends at or before the position it must
    /// reach ([`Options::must_reach`]). Something that was in the log is
    /// gone. Nothing was written.
    #[error("the log's valid prefix ends at {end}, at or before {must_reach} which it must reach")]
    Short {
        /// Where the valid prefix ends (as compared: see
        /// [`Options::pretend_short_by`]).
        end: Lsn,
        /// The position it had to end beyond.
        must_reach: Lsn,
    },
    /// `append`: the record needs another segment file, and the log has
    /// as many as it may have, none of them free. Nothing was written
    /// and the log has not failed: a reclaim makes room.
    #[error("the log is full")]
    Full,
    /// The log has failed: an earlier write or sync returned an error.
    /// Nothing succeeds any more.
    #[error("the log has failed")]
    Failed,
    /// A write or a sync failed during this call, and the log is failed
    /// from now on; or a read failed, which fails only the call.
    #[error("{during} failed ({kind:?}); failure marker written: {marker_written}")]
    Io {
        /// What the log was doing.
        during: &'static str,
        /// The error's kind.
        kind: io::ErrorKind,
        /// Whether the failure could be recorded in the marker file. If
        /// not, a later open cannot know that the log failed.
        marker_written: bool,
    },
    /// The log's content is damaged.
    #[error("the log is corrupt: {0}")]
    Corrupt(#[from] Corruption),
    /// The record does not fit the log: a payload above the largest the
    /// segment size allows, a payload on a record that carries none, or
    /// none on a write.
    #[error("a record cannot carry {len} bytes (at most {max}; none unless it is a write)")]
    BadRecord {
        /// The payload's length.
        len: usize,
        /// The largest payload of a write.
        max: u32,
    },
    /// A position is not in the log, or is not a record boundary.
    #[error("{0} is not a position this call can use")]
    BadPosition(Lsn),
}

/// Why recovery stopped where it did, or what is wrong with a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The header is not a valid header chained to the record before.
    /// `Bad::Blank` is how a log normally ends.
    Header(Bad),
    /// The header is valid but says it belongs elsewhere.
    Position,
    /// The header's epoch is below that of the record before it, or of
    /// its segment.
    Epoch,
    /// The header's epoch, given here, is above the superblock's.
    /// Records are only written in an epoch the superblock already
    /// holds, so the superblock that was read is not the newest that was
    /// written.
    FutureEpoch(u64),
    /// The header's lengths do not fit the segment.
    Length,
    /// The payload does not match its checksum: a torn record.
    Data,
    /// The segment was closed and the next one is not there.
    NoNextSegment,
}

/// What `open` found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Recovery {
    /// Where the log starts.
    pub start: Lsn,
    /// Where the valid prefix ends. The log continues at or after this
    /// position: [`Log::tail`] says where.
    pub end: Lsn,
    /// Why the prefix ends there.
    pub stopped: Stop,
    /// The first place after the break, in the same segment, that looks
    /// like a record. For diagnosis only: the log ends at the break
    /// whatever follows it, and nothing beyond is ever replayed.
    pub follows: Option<Lsn>,
    /// Whether the log was validated by reads from the device. If not
    /// (the file system has no such read), data that only the operating
    /// system's cache still held after a failed sync in an earlier life
    /// may have been accepted, and the prefix may be less durable than
    /// reported.
    pub checked_device: bool,
    /// Whether the failure marker of an earlier life was found (and,
    /// since the caller acknowledged it, removed).
    pub marker_found: bool,
    /// The epoch records are written with from now on.
    pub epoch: u64,
}

/// Where an appended record is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Appended {
    /// The record's position.
    pub start: Lsn,
    /// The position just after it: the end of the log once it is
    /// appended.
    pub end: Lsn,
}

/// A record read back from the log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// The record's position.
    pub start: Lsn,
    /// The position just after it.
    pub end: Lsn,
    /// What it is. Never [`Kind::Pad`].
    pub kind: Kind,
    /// The byte offset on the volume it applies to.
    pub offset: u64,
    /// The number of bytes of the volume it applies to.
    pub length: u64,
    /// The payload: the data of a write, empty otherwise.
    pub data: &'a [u8],
}

/// How to open a log.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// Open a log that is marked as failed. Recovery then runs as usual,
    /// the marker is removed once it has succeeded, and
    /// [`Recovery::marker_found`] says so. The caller takes over the duty
    /// the marker stood for: not to serve the log to a client that was
    /// told more than the log holds.
    pub acknowledge_failure: bool,
    /// A position the valid prefix must end beyond: where a record
    /// starts that someone was told is in the log. If the prefix the
    /// recovery finds ends at or before it, the open fails with
    /// [`LogError::Short`] before the recovery has written anything:
    /// no new epoch, no seal, nothing erased, the failure marker left.
    /// The log's files are then as they were, and every later open with
    /// the same position fails the same way.
    ///
    /// The comparison has to be made here and not by the caller with
    /// [`Recovery::end`]. A recovery seals the prefix with a mark, which
    /// is a record: the next open finds a prefix that ends behind that
    /// mark, beyond the place of a record that was lost, and a caller
    /// that died between the open and its own comparison would have
    /// destroyed the evidence.
    pub must_reach: Option<Lsn>,
    /// For test rigs, to stand for a log that came back shorter: the
    /// prefix end that is compared with [`Options::must_reach`] is
    /// taken as if the last so many writes, write-zeroes and discards
    /// of the prefix were not there. Nothing else changes. Leave it 0.
    pub pretend_short_by: u32,
    pub(crate) mutants: Mutants,
}

/// One segment file.
#[derive(Debug)]
struct Slot<F> {
    /// The number in the file's name.
    number: usize,
    file: F,
    /// Whether the file has its full length.
    sized: bool,
}

/// Reads for validation: from the device where the file system can.
#[derive(Debug, Clone, Copy)]
struct Reader {
    device: bool,
    /// Cleared when a device read was wanted and not available.
    checked: bool,
}

impl Reader {
    fn read<F: File>(&mut self, file: &F, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        if self.device {
            match file.read_device_at(offset, buf) {
                Err(e) if e.kind() == io::ErrorKind::Unsupported => {
                    self.device = false;
                    self.checked = false;
                }
                other => return other,
            }
        }
        file.read_at(offset, buf)
    }
}

/// A read-ahead window over one segment file.
#[derive(Debug, Default)]
struct Window {
    start: u64,
    bytes: Vec<u8>,
}

const WINDOW: u64 = 256 << 10;

impl Window {
    fn reset(&mut self) {
        self.bytes.clear();
    }

    /// The `len` bytes at `offset` of `file`, which has `limit` bytes.
    fn fetch<F: File>(
        &mut self,
        file: &F,
        reader: &mut Reader,
        offset: u64,
        len: usize,
        limit: u64,
    ) -> io::Result<&[u8]> {
        let covered =
            offset >= self.start && offset + len as u64 <= self.start + self.bytes.len() as u64;
        if !covered {
            let want = (len as u64).max(WINDOW).min(limit.saturating_sub(offset));
            if want < len as u64 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            self.bytes.resize(want as usize, 0);
            self.start = offset;
            if let Err(e) = reader.read(file, offset, &mut self.bytes) {
                self.bytes.clear();
                return Err(e);
            }
        }
        let at = (offset - self.start) as usize;
        Ok(&self.bytes[at..at + len])
    }
}

/// Where a walk of the log is.
#[derive(Debug, Clone, Copy)]
struct Cursor {
    /// Where the record before ends. The next record is here, or at the
    /// next unit boundary.
    position: Lsn,
    /// What its header must be chained to.
    chain: u64,
    /// The lowest epoch it may have.
    min_epoch: u64,
}

/// What a walk needs to know about the log.
#[derive(Debug, Clone, Copy)]
struct Rules {
    identity: Identity,
    key: Key,
    /// The highest epoch a record may have.
    max_epoch: u64,
    mutants: Mutants,
}

impl Rules {
    fn seed(&self, base: u64) -> u64 {
        if self.mutants.has(Mutants::NO_CHAIN) {
            0
        } else {
            chain_seed(&self.key, &self.identity.volume, base)
        }
    }

    /// Read and validate the record that follows the cursor in `file`.
    /// Its payload is left in `window`.
    ///
    /// The record is at the cursor, or, if it was appended after a sync
    /// or a recovery, at the next unit boundary. The boundary is tried
    /// first: what a recovery wrote there counts, whatever is still at
    /// the cursor.
    fn record<F: File>(
        &self,
        file: &F,
        reader: &mut Reader,
        window: &mut Window,
        cursor: &Cursor,
    ) -> io::Result<Result<Found, Stop>> {
        let here = cursor.position;
        let boundary = Lsn(self.identity.unit_up(here.0));
        if boundary != here {
            match self.record_at(file, reader, window, cursor, boundary)? {
                Ok(found) => return Ok(Ok(found)),
                Err(stop @ Stop::FutureEpoch(_)) => return Ok(Err(stop)),
                Err(_) => {}
            }
        }
        self.record_at(file, reader, window, cursor, here)
    }

    /// Read and validate a record at `start`, following the cursor.
    fn record_at<F: File>(
        &self,
        file: &F,
        reader: &mut Reader,
        window: &mut Window,
        cursor: &Cursor,
        start: Lsn,
    ) -> io::Result<Result<Found, Stop>> {
        let id = &self.identity;
        let size = id.segment_size;
        let align = u64::from(id.align);
        let at = start.0 % size;
        if id.base_of(start) != id.base_of(cursor.position) || at + align > size {
            return Ok(Err(Stop::Length));
        }
        let mut raw = [0u8; RECORD_HEADER];
        raw.copy_from_slice(window.fetch(file, reader, at, RECORD_HEADER, size)?);
        let chain = if self.mutants.has(Mutants::NO_CHAIN) {
            0
        } else {
            cursor.chain
        };
        let (header, crc) = match RecordHeader::decode(&raw, &self.key, chain) {
            Ok(decoded) => decoded,
            Err(bad) => return Ok(Err(Stop::Header(bad))),
        };
        if header.position != start && !self.mutants.has(Mutants::NO_POSITION_CHECK) {
            return Ok(Err(Stop::Position));
        }
        if header.epoch > self.max_epoch {
            return Ok(Err(Stop::FutureEpoch(header.epoch)));
        }
        if header.epoch < cursor.min_epoch && !self.mutants.has(Mutants::NO_EPOCH_CHECK) {
            return Ok(Err(Stop::Epoch));
        }
        let mut found = Found {
            header,
            crc,
            start,
            end: Lsn(start.0 + align),
            damaged: false,
        };
        if matches!(header.kind, Kind::Pad | Kind::Mark) {
            return Ok(Ok(found));
        }
        if header.data_len > id.max_data() || !id.fits(at, header.data_len) {
            return Ok(Err(Stop::Length));
        }
        found.end = Lsn(start.0 + id.record_size(header.data_len));
        let data = window.fetch(file, reader, at + align, header.data_len as usize, size)?;
        if crc32c::crc32c(data) != header.data_crc && !self.mutants.has(Mutants::NO_DATA_CHECK) {
            if !self.mutants.has(Mutants::ACCEPT_AFTER_GAP) {
                return Ok(Err(Stop::Data));
            }
            found.damaged = true;
        }
        Ok(Ok(found))
    }
}

/// A record a walk found.
#[derive(Debug, Clone, Copy)]
struct Found {
    header: RecordHeader,
    /// The header's tag: what the next record is chained to.
    crc: u64,
    start: Lsn,
    end: Lsn,
    /// Only with the defect that skips damaged records: this is one.
    damaged: bool,
}

/// Record the failure where a later open will find it. Returns whether
/// that worked.
fn write_marker<D: Dir>(dir: &D, mutants: Mutants) -> bool {
    if mutants.has(Mutants::NO_MARKER) {
        return false;
    }
    let file = match dir.create(MARKER_NAME) {
        Ok(file) => Some(file),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => dir.open(MARKER_NAME).ok(),
        Err(_) => None,
    };
    let Some(mut file) = file else {
        return false;
    };
    file.sync().is_ok() && (mutants.has(Mutants::MARKER_UNSYNCED) || dir.sync().is_ok())
}

fn read_error(during: &'static str, e: &io::Error) -> LogError {
    LogError::Io {
        during,
        kind: e.kind(),
        marker_written: false,
    }
}

/// Who is told that the log has failed: see [`Log::set_observer`].
pub struct Observer(Box<dyn FnMut() + Send>);

impl Observer {
    /// An observer that calls `f`.
    pub fn new(f: impl FnMut() + Send + 'static) -> Self {
        Self(Box::new(f))
    }
}

impl std::fmt::Debug for Observer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Observer")
    }
}

/// The write log. One writer owns it: appends are written in order, one
/// at a time, and that is what makes an acknowledged record imply every
/// earlier one (see the crate documentation, "Order of
/// acknowledgements").
#[derive(Debug)]
pub struct Log<D: Dir> {
    dir: D,
    superblock: D::File,
    /// The superblock as last written, and the copy that holds it.
    current: Superblock,
    /// The key headers are tagged with: the superblock's.
    key: Key,
    copy: u64,
    slots: Vec<Slot<D::File>>,
    /// The segments of the log, by first position.
    live: BTreeMap<u64, usize>,
    /// The end of the last record.
    tail: Lsn,
    chain: u64,
    durable: Lsn,
    /// The write unit the tail is in holds records that are durable, or
    /// that a sync has been issued for: the next record starts at the
    /// next unit boundary.
    fresh_block: bool,
    /// The current segment has its closing marker and the next one has
    /// not been started.
    closed: bool,
    hold: Option<Lsn>,
    failed: bool,
    /// Segment files with writes no sync has covered.
    dirty: BTreeSet<usize>,
    /// A segment file was created and the directory not synced since.
    dir_dirty: bool,
    scratch: Vec<u8>,
    /// The most segment files the log may have.
    limit: Option<usize>,
    /// Told once, when the log fails.
    observer: Option<Observer>,
    /// A validated cursor at or below the log's start, with the base
    /// of the segment it is in: where the last reclaim's walk to its
    /// target ended. A replay that begins in that segment begins here
    /// and not at the segment's first record.
    start: Option<(u64, Cursor)>,
    /// Records validated by replays, for tests of what a replay costs.
    #[cfg(test)]
    walked: std::cell::Cell<u64>,
    mutants: Mutants,
}

impl<D: Dir> Log<D> {
    /// Make a new, empty log in `dir`, with a new random key, and open
    /// it.
    pub fn create(dir: D, identity: Identity) -> Result<Self, LogError> {
        let key = Key::random().ok_or(LogError::NoRandomness)?;
        Self::create_keyed(dir, identity, key, Options::default())
    }

    /// Make a new log with a given key, and open it with `options`. The
    /// tests use it to be reproducible to the bit.
    pub(crate) fn create_keyed(
        dir: D,
        identity: Identity,
        key: Key,
        options: Options,
    ) -> Result<Self, LogError> {
        if !identity.valid() {
            return Err(LogError::InvalidConfig);
        }
        let io = |during| move |e: io::Error| read_error(during, &e);
        let names = dir.list().map_err(io("listing the directory"))?;
        if names.iter().any(|name| name == SUPER_NAME) {
            return Err(LogError::Exists);
        }
        // What an interrupted creation left behind. The superblock's
        // name is what makes a log, and it is given last.
        for name in names.iter().filter(|name| name.starts_with("log.")) {
            dir.remove(name).map_err(io("removing leftovers"))?;
        }
        let mut segment = dir
            .create(&slot_name(0))
            .map_err(io("creating a segment"))?;
        segment
            .preallocate(identity.segment_size)
            .map_err(io("creating a segment"))?;
        let header = SegmentHeader {
            volume: identity.volume,
            base: 0,
            epoch: 1,
        };
        let tag_key = if options.mutants.has(Mutants::PUBLIC_KEY) {
            Key::PUBLIC
        } else {
            key
        };
        segment
            .write_at(0, &header.encode(&tag_key))
            .map_err(io("creating a segment"))?;
        segment.sync().map_err(io("creating a segment"))?;
        // The segment's name is durable before the superblock has one.
        // A file system may keep a later name and lose an earlier one,
        // and a superblock without its first segment is not "no log"
        // but a damaged one.
        dir.sync().map_err(io("creating a segment"))?;
        let mut superblock = dir
            .create(SUPER_NEW_NAME)
            .map_err(io("creating the superblock"))?;
        superblock
            .preallocate(SUPER_COPY_STRIDE + SECTOR_LEN as u64)
            .map_err(io("creating the superblock"))?;
        let first = Superblock {
            identity,
            key,
            counter: 1,
            epoch: 1,
            floor: Lsn(identity.unit()),
            durable_floor: Lsn(identity.unit()),
        };
        superblock
            .write_at(0, &first.encode())
            .map_err(io("creating the superblock"))?;
        superblock.sync().map_err(io("creating the superblock"))?;
        // The superblock gets its name only when it is whole and
        // durable: a creation that is interrupted leaves no log, never
        // half of one.
        dir.rename(SUPER_NEW_NAME, SUPER_NAME)
            .map_err(io("creating the log"))?;
        dir.sync().map_err(io("creating the log"))?;
        drop((segment, superblock));
        Self::open(dir, options).map(|(log, _)| log)
    }

    /// Open the log in `dir` and recover it: find the longest valid
    /// prefix, make it durable, and make everything beyond it
    /// unrecoverable for good, all before returning.
    pub fn open(dir: D, options: Options) -> Result<(Self, Recovery), LogError> {
        let mutants = options.mutants;
        let names = dir
            .list()
            .map_err(|e| read_error("listing the directory", &e))?;
        if !names.iter().any(|name| name == SUPER_NAME) {
            return Err(LogError::NotFound);
        }
        let marker_found = names.iter().any(|name| name == MARKER_NAME);
        if marker_found && !options.acknowledge_failure {
            return Err(LogError::MarkedFailed);
        }
        let fatal = |dir: &D, during, e: io::Error| LogError::Io {
            during,
            kind: e.kind(),
            marker_written: write_marker(dir, mutants),
        };
        let mut superblock = dir
            .open(SUPER_NAME)
            .map_err(|e| read_error("opening the superblock", &e))?;
        let mut files: BTreeMap<usize, D::File> = BTreeMap::new();
        for name in &names {
            if let Some(index) = slot_index(name) {
                let file = dir
                    .open(name)
                    .map_err(|e| read_error("opening a segment", &e))?;
                files.insert(index, file);
            }
        }

        // First hand the device everything an earlier process wrote and
        // did not sync, so that the reads below see it, and so that what
        // they see is durable.
        if !mutants.has(Mutants::NO_SYNC_BEFORE_VALIDATE) {
            if let Err(e) = superblock.sync() {
                return Err(fatal(&dir, "syncing before recovery", e));
            }
            for file in files.values_mut() {
                if let Err(e) = file.sync() {
                    return Err(fatal(&dir, "syncing before recovery", e));
                }
            }
            if let Err(e) = dir.sync() {
                return Err(fatal(&dir, "syncing before recovery", e));
            }
        }

        let mut reader = Reader {
            device: !mutants.has(Mutants::VALIDATE_CACHED),
            checked: !mutants.has(Mutants::VALIDATE_CACHED),
        };

        // The superblock: the valid copy with the higher counter.
        let mut best: Option<(Superblock, u64)> = None;
        for copy in 0..2u64 {
            let mut raw = [0u8; SECTOR_LEN];
            match reader.read(&superblock, copy * SUPER_COPY_STRIDE, &mut raw) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => continue,
                Err(e) => return Err(read_error("reading the superblock", &e)),
            }
            if let Ok(block) = Superblock::decode(&raw) {
                if best.is_none_or(|(b, _)| block.counter > b.counter) {
                    best = Some((block, copy));
                }
            }
        }
        let Some((current, copy)) = best else {
            return Err(Corruption::NoSuperblock.into());
        };
        let identity = current.identity;
        let size = identity.segment_size;
        let key = if mutants.has(Mutants::PUBLIC_KEY) {
            Key::PUBLIC
        } else {
            current.key
        };

        // The segments: what each file says it holds. Of two that claim
        // the same part of the log, the one written in the later epoch.
        let mut slots: Vec<Slot<D::File>> = Vec::new();
        let mut claims: BTreeMap<u64, (u64, usize)> = BTreeMap::new();
        let mut claimants: Vec<(u64, usize)> = Vec::new();
        for (number, file) in files {
            let index = slots.len();
            let sized = file
                .len()
                .map_err(|e| read_error("reading a segment", &e))?
                >= size;
            if sized {
                let mut raw = [0u8; SECTOR_LEN];
                reader
                    .read(&file, 0, &mut raw)
                    .map_err(|e| read_error("reading a segment", &e))?;
                if let Ok(header) = SegmentHeader::decode(&raw, &key) {
                    let valid =
                        header.volume == identity.volume && header.base.is_multiple_of(size);
                    if valid
                        && header.epoch > current.epoch
                        && !mutants.has(Mutants::NO_HEADER_FUTURE_CHECK)
                    {
                        return Err(Corruption::SuperblockBehind {
                            position: Lsn(header.base),
                            found: header.epoch,
                            superblock: current.epoch,
                        }
                        .into());
                    }
                    let newer = claims.get(&header.base).is_none_or(|(epoch, _)| {
                        (header.epoch > *epoch) != mutants.has(Mutants::PREFER_OLDER_CLAIM)
                    });
                    if valid {
                        claimants.push((header.base, index));
                    }
                    if valid && newer {
                        claims.insert(header.base, (header.epoch, index));
                    }
                }
            }
            slots.push(Slot {
                number,
                file,
                sized,
            });
        }

        // Walk the records from the start of the segment the log starts
        // in.
        let rules = Rules {
            identity,
            key,
            max_epoch: current.epoch,
            mutants,
        };
        let mut base = identity.base_of(current.floor);
        let Some((mut segment_epoch, mut index)) = claims.get(&base).copied() else {
            return Err(Corruption::MissingSegment { base }.into());
        };
        let mut live = BTreeMap::from([(base, index)]);
        let mut window = Window::default();
        let mut cursor = Cursor {
            position: Lsn(base + identity.unit()),
            chain: rules.seed(base),
            min_epoch: segment_epoch,
        };
        if mutants.has(Mutants::NO_SEGMENT_EPOCH_CHECK) {
            cursor.min_epoch = 0;
        }
        // The starts of the last records, for a rig that pretends the
        // log came back shorter.
        let pretend = options.pretend_short_by as usize;
        let mut last_starts: VecDeque<Lsn> = VecDeque::new();
        let stopped = loop {
            let file = &slots[index].file;
            // A read error is an error, never the end of the log: what
            // follows from an end (a new epoch, a seal, erased segments)
            // would destroy records that are only unreadable right now.
            let step = match rules.record(file, &mut reader, &mut window, &cursor) {
                Ok(step) => step,
                Err(_) if mutants.has(Mutants::READ_ERROR_ENDS_LOG) => {
                    Err(Stop::Header(Bad::Blank))
                }
                Err(e) => return Err(read_error("reading the log", &e)),
            };
            let found = match step {
                Ok(found) => found,
                Err(stop) => break stop,
            };
            cursor.min_epoch = cursor.min_epoch.max(found.header.epoch);
            if pretend > 0 && !matches!(found.header.kind, Kind::Pad | Kind::Mark) {
                if last_starts.len() == pretend {
                    last_starts.pop_front();
                }
                last_starts.push_back(cursor.position);
            }
            if found.header.kind != Kind::Pad {
                cursor.position = found.end;
                cursor.chain = found.crc;
                continue;
            }
            // The segment is closed: the log continues in the file that
            // claims the next part, if its header is not older than the
            // marker that closed this one.
            let next = base + size;
            let claim = claims.get(&next).copied().filter(|(epoch, _)| {
                (*epoch >= cursor.min_epoch && *epoch <= current.epoch)
                    || mutants.has(Mutants::NO_SEGMENT_EPOCH_CHECK)
            });
            let Some((epoch, next_index)) = claim else {
                break Stop::NoNextSegment;
            };
            base = next;
            index = next_index;
            segment_epoch = epoch;
            live.insert(base, index);
            window.reset();
            cursor = Cursor {
                position: Lsn(base + identity.unit()),
                chain: rules.seed(base),
                min_epoch: cursor.min_epoch.max(segment_epoch),
            };
            if mutants.has(Mutants::NO_SEGMENT_EPOCH_CHECK) {
                cursor.min_epoch = 0;
            }
        };
        let end = cursor.position;
        if let Stop::FutureEpoch(found) = stopped {
            return Err(Corruption::SuperblockBehind {
                position: end,
                found,
                superblock: current.epoch,
            }
            .into());
        }
        if end < current.durable_floor && !mutants.has(Mutants::NO_DURABLE_FLOOR) {
            return Err(Corruption::BelowDurableFloor {
                end,
                durable_floor: current.durable_floor,
                stopped,
            }
            .into());
        }
        // What the caller was told is in the log must be in the prefix.
        // Compared before anything is written: a recovery that sealed a
        // short prefix would leave the next open nothing to notice.
        let short = options.must_reach.and_then(|must_reach| {
            let compared = match last_starts.front() {
                Some(start) if last_starts.len() == pretend => *start,
                // Fewer records than are to be pretended away.
                Some(_) => current.floor,
                None => end,
            };
            (compared <= must_reach).then_some(LogError::Short {
                end: compared,
                must_reach,
            })
        });
        if let Some(short) = short {
            if !mutants.has(Mutants::COMPARE_AFTER_SEAL) {
                return Err(short);
            }
        }
        let follows = Self::scan(&identity, &slots[index].file, &mut reader, &mut window, end)
            .map_err(|e| read_error("reading the log", &e))?;

        let mut log = Self {
            dir,
            superblock,
            current,
            key,
            copy,
            slots,
            live,
            tail: end,
            chain: cursor.chain,
            durable: end,
            // What recovery returns is durable, so the unit it ends in
            // is not written again.
            fresh_block: !mutants.has(Mutants::REWRITE_DURABLE_BLOCK),
            closed: stopped == Stop::NoNextSegment,
            hold: None,
            observer: None,
            start: None,
            #[cfg(test)]
            walked: std::cell::Cell::new(0),
            failed: false,
            dirty: BTreeSet::new(),
            dir_dirty: false,
            scratch: Vec::new(),
            limit: None,
            mutants,
        };

        // Make everything beyond the prefix unrecoverable, for good,
        // before anyone is told where the log ends.
        //
        // 1. A new epoch. Whatever is left beyond the prefix was written
        //    in an older one, and can never again follow a record
        //    written from now on.
        if !mutants.has(Mutants::NO_EPOCH_BUMP) {
            log.write_superblock(Superblock {
                epoch: current.epoch + 1,
                durable_floor: end.max(current.durable_floor),
                ..current
            })?;
        }
        // 2. Seal the end of the prefix: write a record of the new epoch
        //    where the next record is looked for first, so that nothing
        //    that is there now, or behind it, can follow the prefix
        //    again. Always: that the place reads as never written proves
        //    nothing, since a lost sector can read as it did before. The
        //    one exception is by structure: the prefix ends with the
        //    marker that closes a segment, and (after step 3) no file
        //    holds the next one, so there is no place where anything
        //    could follow.
        if !mutants.has(Mutants::NO_TAIL_ERASE) && !log.closed {
            log.seal()?;
        }
        // 3. Erase the header of every file that claims a part of the
        //    log from its start on and is not one of its segments: a part
        //    beyond the prefix, or one a newer file holds.
        if !mutants.has(Mutants::KEEP_STALE_SEGMENTS) {
            let first = identity.base_of(current.floor);
            let stale: Vec<usize> = claimants
                .iter()
                .filter(|(base, index)| *base >= first && log.live.get(base) != Some(index))
                .map(|(_, index)| *index)
                .collect();
            for stale in stale {
                let file = &mut log.slots[stale].file;
                let erased = file
                    .write_at(0, &[0u8; SECTOR_LEN])
                    .and_then(|()| file.sync());
                if let Err(e) = erased {
                    return Err(log.fail("erasing a stale segment", &e));
                }
            }
        }
        if let Some(short) = short {
            return Err(short);
        }
        if marker_found {
            let removed = log.dir.remove(MARKER_NAME).and_then(|()| log.dir.sync());
            if let Err(e) = removed {
                return Err(log.fail("removing the failure marker", &e));
            }
        }
        let recovery = Recovery {
            start: current.floor,
            end,
            stopped,
            follows,
            checked_device: reader.checked,
            marker_found,
            epoch: log.current.epoch,
        };
        Ok((log, recovery))
    }

    /// The first position after `from`, in the same segment, whose bytes
    /// look like a record header that names that position.
    fn scan(
        identity: &Identity,
        file: &D::File,
        reader: &mut Reader,
        window: &mut Window,
        from: Lsn,
    ) -> io::Result<Option<Lsn>> {
        let size = identity.segment_size;
        let align = u64::from(identity.align);
        let base = identity.base_of(from);
        let mut at = from.0 % size + align;
        while at + RECORD_HEADER as u64 <= size {
            let mut raw = [0u8; RECORD_HEADER];
            raw.copy_from_slice(window.fetch(file, reader, at, RECORD_HEADER, size)?);
            if RecordHeader::looks_like(&raw, Lsn(base + at)) {
                return Ok(Some(Lsn(base + at)));
            }
            at += align;
        }
        Ok(None)
    }

    /// The directory the log is in, for an owner that keeps files of
    /// its own beside it.
    pub fn dir(&self) -> &D {
        &self.dir
    }

    /// The log's identity.
    pub fn identity(&self) -> &Identity {
        &self.current.identity
    }

    /// The end of the log: where the next record goes.
    pub fn tail(&self) -> Lsn {
        self.tail
    }

    /// The position up to which the log is durable: every record that
    /// ends at or before it survives a power cut.
    pub fn durable(&self) -> Lsn {
        self.durable
    }

    /// Where the log starts.
    pub fn floor(&self) -> Lsn {
        self.current.floor
    }

    /// Whether the log has failed.
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// The number of segment files, in use or free.
    pub fn segment_files(&self) -> usize {
        self.slots.len()
    }

    /// Fail the log on behalf of its owner: something the owner keeps
    /// beside the log has failed, and what the log holds must not be
    /// served or extended any more. As after a failure of the log's own
    /// files, nothing succeeds from now on, and the marker is written;
    /// returns whether it was.
    pub fn mark_failed(&mut self) -> bool {
        self.failed = true;
        self.announce()
    }

    /// Call `observer` once, at the moment the log fails: after the
    /// decision and before the marker is attempted. The marker needs
    /// the disk that has just failed, and a sync on a sick disk can
    /// take minutes or never return; whoever must act on the failure
    /// at once (a transport giving up its attachment) cannot wait for
    /// it. The observer runs in the thread that owns the log and is to
    /// return quickly.
    pub fn set_observer(&mut self, observer: Observer) {
        self.observer = Some(observer);
    }

    /// Tell the observer, then attempt the marker. Returns whether the
    /// marker was written.
    fn announce(&mut self) -> bool {
        let late = self.mutants.has(Mutants::OBSERVE_AFTER_MARKER);
        let mut observer = self.observer.take();
        if !late {
            if let Some(Observer(call)) = observer.as_mut() {
                call();
            }
        }
        let written = write_marker(&self.dir, self.mutants);
        if late {
            if let Some(Observer(call)) = observer.as_mut() {
                call();
            }
        }
        written
    }

    /// The key the log's headers are tagged with, for what the store
    /// keeps beside the log. It stays inside this crate.
    pub(crate) fn key(&self) -> &Key {
        &self.key
    }

    /// The segment file that holds `position` and the offset in it, for
    /// tests that decide the fate of single records at a power cut.
    #[cfg(test)]
    pub(crate) fn locate(&self, position: Lsn) -> Option<(String, u64)> {
        let identity = self.current.identity;
        let index = *self.live.get(&identity.base_of(position))?;
        Some((
            slot_name(self.slots[index].number),
            position.0 % identity.segment_size,
        ))
    }

    /// The hold: a position [`Log::reclaim`] never passes.
    pub fn hold(&self) -> Option<Lsn> {
        self.hold
    }

    /// Set or lift the hold. It lives in memory: after an open, set it
    /// again before the first reclaim.
    pub fn set_hold(&mut self, hold: Option<Lsn>) {
        self.hold = hold;
    }

    /// What the next record's header is chained to.
    fn chain(&self) -> u64 {
        if self.mutants.has(Mutants::NO_CHAIN) {
            0
        } else {
            self.chain
        }
    }

    /// The log has failed: remember it, here and on disk.
    fn fail(&mut self, during: &'static str, e: &io::Error) -> LogError {
        self.failed = true;
        LogError::Io {
            during,
            kind: e.kind(),
            marker_written: self.announce(),
        }
    }

    /// Write `next` to the copy that does not hold the current
    /// superblock, and sync it.
    fn write_superblock(&mut self, next: Superblock) -> Result<(), LogError> {
        let next = Superblock {
            counter: self.current.counter + 1,
            ..next
        };
        let copy = if self.mutants.has(Mutants::ONE_SUPER_COPY) {
            self.copy
        } else {
            1 - self.copy
        };
        let written = self
            .superblock
            .write_at(copy * SUPER_COPY_STRIDE, &next.encode())
            .and_then(|()| self.superblock.sync());
        if let Err(e) = written {
            return Err(self.fail("writing the superblock", &e));
        }
        self.current = next;
        self.copy = copy;
        Ok(())
    }

    /// Where the next record starts.
    fn next_start(&self) -> Lsn {
        if self.fresh_block {
            Lsn(self.current.identity.unit_up(self.tail.0))
        } else {
            self.tail
        }
    }

    /// Write a record without payload at the place the next record
    /// starts.
    fn write_marker_record(
        &mut self,
        kind: Kind,
        during: &'static str,
    ) -> Result<(usize, Lsn, u64), LogError> {
        let identity = self.current.identity;
        let size = identity.segment_size;
        let base = identity.base_of(self.tail);
        let Some(&index) = self.live.get(&base) else {
            return Err(Corruption::MissingSegment { base }.into());
        };
        let start = self.next_start();
        let header = RecordHeader {
            kind,
            position: start,
            epoch: self.current.epoch,
            offset: 0,
            length: if kind == Kind::Pad {
                base + size - start.0
            } else {
                0
            },
            data_len: 0,
            data_crc: 0,
        };
        let (bytes, crc) = header.encode(&self.key, self.chain());
        if let Err(e) = self.slots[index].file.write_at(start.0 % size, &bytes) {
            return Err(self.fail(during, &e));
        }
        self.dirty.insert(index);
        Ok((index, start, crc))
    }

    /// Recovery: put a record of the current epoch where the next record
    /// is looked for first, and make it durable. A mark if the segment
    /// has room to go on, else the marker that closes the segment.
    fn seal(&mut self) -> Result<(), LogError> {
        let identity = self.current.identity;
        let align = u64::from(identity.align);
        let start = self.next_start();
        let room =
            start.0 % identity.segment_size + identity.unit() + align <= identity.segment_size;
        let kind = if room { Kind::Mark } else { Kind::Pad };
        let (index, start, crc) = self.write_marker_record(kind, "sealing the tail")?;
        if let Err(e) = self.slots[index].file.sync() {
            return Err(self.fail("sealing the tail", &e));
        }
        self.dirty.remove(&index);
        if room {
            self.tail = Lsn(start.0 + align);
            self.chain = crc;
            self.durable = self.tail;
        } else {
            self.closed = true;
        }
        Ok(())
    }

    /// Close the current segment and continue in the next.
    fn roll(&mut self) -> Result<(), LogError> {
        let identity = self.current.identity;
        let size = identity.segment_size;
        let base = identity.base_of(self.tail);
        if !self.closed {
            self.write_marker_record(Kind::Pad, "closing a segment")?;
            self.closed = true;
        }

        // A file for the next part: a free one, or a new one.
        let used: BTreeSet<usize> = self.live.values().copied().collect();
        let next_index = match (0..self.slots.len()).find(|i| !used.contains(i)) {
            Some(free) => free,
            None => {
                let number = self.slots.iter().map(|s| s.number + 1).max().unwrap_or(0);
                match self.dir.create(&slot_name(number)) {
                    Ok(file) => {
                        self.slots.push(Slot {
                            number,
                            file,
                            sized: false,
                        });
                        self.dir_dirty = true;
                        self.slots.len() - 1
                    }
                    Err(e) => return Err(self.fail("creating a segment", &e)),
                }
            }
        };
        let next = base + size;
        let header = SegmentHeader {
            volume: identity.volume,
            base: next,
            epoch: self.current.epoch,
        };
        let slot = &mut self.slots[next_index];
        let prepared = if slot.sized {
            Ok(())
        } else {
            slot.file.preallocate(size)
        }
        .and_then(|()| slot.file.write_at(0, &header.encode(&self.key)));
        if let Err(e) = prepared {
            return Err(self.fail("starting a segment", &e));
        }
        self.slots[next_index].sized = true;
        self.dirty.insert(next_index);
        self.live.insert(next, next_index);
        self.closed = false;
        self.fresh_block = false;
        self.tail = Lsn(next + identity.unit());
        self.chain = chain_seed(&self.key, &identity.volume, next);
        Ok(())
    }

    /// Append a record. When this returns `Ok`, the record's write has
    /// returned, and so has the write of every record before it: it
    /// survives the loss of the process. It survives a power cut once a
    /// [`Log::sync`] that started after this call has returned `Ok`.
    ///
    /// `data` is the payload of a [`Kind::Write`] and must be empty for
    /// the other kinds. [`Kind::Pad`] and [`Kind::Mark`] are the log's own
    /// and are refused.
    ///
    /// Records are not always contiguous: the first record after a sync
    /// or a recovery starts at the next multiple of the write unit, so
    /// that a unit that holds durable records is never written again.
    pub fn append(
        &mut self,
        kind: Kind,
        offset: u64,
        length: u64,
        data: &[u8],
    ) -> Result<Appended, LogError> {
        if self.failed {
            return Err(LogError::Failed);
        }
        let identity = self.current.identity;
        let max = identity.max_data();
        let fits = match kind {
            Kind::Write => !data.is_empty() && data.len() <= max as usize,
            Kind::WriteZeroes | Kind::Discard => data.is_empty(),
            Kind::Pad | Kind::Mark => false,
        };
        if !fits {
            return Err(LogError::BadRecord {
                len: data.len(),
                max,
            });
        }
        let data_len = data.len() as u32;
        let size = identity.segment_size;
        let align = u64::from(identity.align);
        let record = identity.record_size(data_len);
        if self.closed || !identity.fits(self.next_start().0 % size, data_len) {
            // The record goes into the next segment. If that needs one
            // file more than the log may have, say so before anything
            // is written, the closing pad included.
            let in_use = self.live.len();
            if in_use == self.slots.len() && self.limit.is_some_and(|limit| in_use >= limit) {
                return Err(LogError::Full);
            }
            self.roll()?;
        }
        let start = self.next_start();
        let base = identity.base_of(start);
        let Some(&index) = self.live.get(&base) else {
            return Err(Corruption::MissingSegment { base }.into());
        };
        let header = RecordHeader {
            kind,
            position: start,
            epoch: self.current.epoch,
            offset,
            length,
            data_len,
            data_crc: crc32c::crc32c(data),
        };
        let (bytes, crc) = header.encode(&self.key, self.chain());
        // Header and payload go out in one write, each padded to the
        // alignment.
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize(record as usize, 0);
        scratch[..RECORD_HEADER].copy_from_slice(&bytes);
        scratch[align as usize..align as usize + data.len()].copy_from_slice(data);
        let written = self.slots[index].file.write_at(start.0 % size, &scratch);
        self.scratch = scratch;
        if let Err(e) = written {
            return Err(self.fail("appending a record", &e));
        }
        self.dirty.insert(index);
        self.fresh_block = false;
        self.tail = Lsn(start.0 + record);
        self.chain = crc;
        Ok(Appended {
            start,
            end: self.tail,
        })
    }

    /// Make every record appended so far durable, and return the
    /// position that is now durable. One call covers every append before
    /// it; a call with nothing to do costs nothing.
    pub fn sync(&mut self) -> Result<Lsn, LogError> {
        if self.failed {
            return Err(LogError::Failed);
        }
        let target = self.tail;
        if self.mutants.has(Mutants::SYNC_EARLY) {
            self.durable = target;
            return Ok(target);
        }
        // From the moment a sync is issued, what it covers may be durable
        // at any time, so the unit it ends in is not written again: the
        // next record starts a new unit. This is set before the first
        // device sync and not after the last, so that it also holds for
        // an append made while a sync is still in flight.
        if !self.dirty.is_empty() && !self.mutants.has(Mutants::REWRITE_DURABLE_BLOCK) {
            self.fresh_block = true;
        }
        // Every segment that holds bytes no sync has covered, not only
        // the one being written; and the directory, if one of them is a
        // new file. Only then does the durable position move.
        let tail_index = self
            .live
            .get(&self.current.identity.base_of(self.tail))
            .copied();
        let dirty: Vec<usize> = self.dirty.iter().copied().collect();
        for index in dirty {
            if self.mutants.has(Mutants::SYNC_ONLY_TAIL) && Some(index) != tail_index {
                continue;
            }
            if let Err(e) = self.slots[index].file.sync() {
                if self.mutants.has(Mutants::RETRY_SYNC) {
                    return Err(read_error("syncing", &e));
                }
                return Err(self.fail("syncing", &e));
            }
        }
        self.dirty.clear();
        if self.dir_dirty && !self.mutants.has(Mutants::SYNC_NO_DIR) {
            if let Err(e) = self.dir.sync() {
                return Err(self.fail("syncing the directory", &e));
            }
        }
        self.dir_dirty = false;
        self.durable = target;
        Ok(target)
    }

    /// Call `visit` with every record that starts at or after `from` and
    /// before `to`, in order. Both must be record boundaries inside the
    /// log, where a record starts or ends: positions that
    /// [`Log::append`] (start or end), [`Log::tail`], [`Log::floor`],
    /// [`Log::durable`] or a recovery returned. A record that follows a
    /// sync or a recovery starts at the next unit boundary, so its start
    /// is not the end of the record before it; both are boundaries, and
    /// a range from the one to the other is empty. A position inside a
    /// record, or in the gap between two, is refused, for `from` as for
    /// `to`. Every record is checked again as it is read, and a damaged
    /// one is an error, never data.
    ///
    /// A failed log replays nothing: after a failed sync the cache can
    /// show records the disk does not hold, and what the log holds is
    /// known again only after a recovery.
    ///
    /// `to` may not lie beyond [`Log::durable`]. Records beyond the
    /// durable position are in the log and survive the loss of the
    /// process, but a power cut may still take them back, and nothing
    /// that a power cut can take back may be applied to anything that
    /// outlives the log's own records (a home image, an upload): the
    /// volume would then hold what the log never had, and would not be
    /// a prefix. So a replay does not hand them out. After an open the
    /// whole log is durable. The payload of a single record that is not
    /// durable yet, to serve a client its own write, is read with
    /// [`Log::read_payload`].
    pub fn replay(
        &self,
        from: Lsn,
        to: Lsn,
        visit: &mut dyn FnMut(&Record<'_>),
    ) -> Result<(), LogError> {
        let limit = if self.mutants.has(Mutants::REPLAY_UNSYNCED) {
            self.tail
        } else {
            self.durable
        };
        self.replay_within(from, to, limit, visit)
    }

    /// [`Log::replay`] with the tail as the limit: for the tests, which
    /// look at records before they are synced.
    #[cfg(test)]
    pub(crate) fn replay_unsynced(
        &self,
        from: Lsn,
        to: Lsn,
        visit: &mut dyn FnMut(&Record<'_>),
    ) -> Result<(), LogError> {
        self.replay_within(from, to, self.tail, visit)
    }

    pub(crate) fn replay_within(
        &self,
        from: Lsn,
        to: Lsn,
        limit: Lsn,
        visit: &mut dyn FnMut(&Record<'_>),
    ) -> Result<(), LogError> {
        self.walk(from, to, limit, visit).map(|_| ())
    }

    /// Records validated by replays so far.
    #[cfg(test)]
    pub(crate) fn walked(&self) -> u64 {
        self.walked.get()
    }

    /// The replay. Returns where the walk ended: a cursor at or below
    /// `to`, validated from the segment's beginning or from the cursor
    /// kept for the log's start, and the base of its segment.
    ///
    /// A walk has to begin where the chain's value is known. That is a
    /// segment's first record, or the cursor a reclaim left at the
    /// log's start: without it every replay would validate again what
    /// lies between the segment's beginning and the start, and a
    /// checkpoint in batches would cost the square of the records in a
    /// segment. A cursor that does not belong where it is used shows as
    /// a record that does not validate, never as a wrong record.
    fn walk(
        &self,
        from: Lsn,
        to: Lsn,
        limit: Lsn,
        visit: &mut dyn FnMut(&Record<'_>),
    ) -> Result<(u64, Cursor), LogError> {
        if self.failed && !self.mutants.has(Mutants::REPLAY_WHEN_FAILED) {
            return Err(LogError::Failed);
        }
        if from < self.current.floor || from > to {
            return Err(LogError::BadPosition(from));
        }
        if to > limit {
            return Err(LogError::BadPosition(to));
        }
        let identity = self.current.identity;
        let size = identity.segment_size;
        // A segment begins with its header, one unit long, and no
        // record starts or ends in it. The walk below begins behind the
        // header and would never meet such a position, so it is refused
        // here.
        if !self.mutants.has(Mutants::HEADER_IS_BOUNDARY) {
            for bound in [from, to] {
                if bound.0 - identity.base_of(bound) < identity.unit() {
                    return Err(LogError::BadPosition(bound));
                }
            }
        }
        let rules = Rules {
            identity,
            key: self.key,
            max_epoch: self.current.epoch,
            mutants: self.mutants,
        };
        let mut reader = Reader {
            device: false,
            checked: true,
        };
        let mut window = Window::default();
        let mut base = identity.base_of(from);
        let mut cursor = Cursor {
            position: Lsn(base + identity.unit()),
            chain: rules.seed(base),
            min_epoch: 0,
        };
        if let Some((kept_base, kept)) = self.start {
            if kept_base == base && kept.position <= from {
                cursor = kept;
            }
        }
        while cursor.position < to {
            let Some(&index) = self.live.get(&base) else {
                return Err(Corruption::MissingSegment { base }.into());
            };
            #[cfg(test)]
            self.walked.set(self.walked.get() + 1);
            let step = rules
                .record(&self.slots[index].file, &mut reader, &mut window, &cursor)
                .map_err(|e| read_error("reading the log", &e))?;
            let found = step.map_err(|stopped| Corruption::Record {
                position: cursor.position,
                stopped,
            })?;
            let header = found.header;
            let start = found.start;
            // Where the record after this one is looked for. A pad
            // reaches to the first record of the next segment.
            let end = if header.kind == Kind::Pad {
                Lsn(base + size + identity.unit())
            } else {
                found.end
            };
            // A boundary is where a record starts or ends. Neither the
            // gap in front of this record (it was appended after a sync
            // or a recovery, at the next unit boundary) nor its inside
            // is one.
            let before = cursor.position;
            for bound in [from, to] {
                if (before < bound && bound < start) || (start < bound && bound < end) {
                    return Err(LogError::BadPosition(bound));
                }
            }
            if start >= to && !self.mutants.has(Mutants::REPLAY_PAST_START) {
                // The range ends where this record starts.
                break;
            }
            if header.kind == Kind::Pad {
                base += size;
                window.reset();
                cursor = Cursor {
                    position: end,
                    chain: rules.seed(base),
                    min_epoch: header.epoch,
                };
                continue;
            }
            if start >= from && !found.damaged && header.kind != Kind::Mark {
                let at = start.0 % size + u64::from(identity.align);
                let data = window
                    .fetch(
                        &self.slots[index].file,
                        &mut reader,
                        at,
                        header.data_len as usize,
                        size,
                    )
                    .map_err(|e| read_error("reading the log", &e))?;
                visit(&Record {
                    start,
                    end,
                    kind: header.kind,
                    offset: header.offset,
                    length: header.length,
                    data,
                });
            }
            cursor = Cursor {
                position: end,
                chain: found.crc,
                min_epoch: header.epoch,
            };
        }
        Ok((base, cursor))
    }

    /// Read `buf.len()` bytes of the payload of the write record that
    /// starts at `record`, from `skip` bytes into it.
    ///
    /// `record` must be the start of a write record that is in the log
    /// now: one [`Log::append`] returned to this instance, or one a
    /// replay showed it. Every such record was validated by this
    /// process: it wrote the record itself, or recovery checked it
    /// against the device. The record need not be durable; this is how
    /// a client is served a write it was acknowledged before any sync.
    /// A position that is not the start of a write record of at least
    /// that length is refused; the header at the position is checked
    /// for being a record's at that position, not for its tag, which
    /// would need the record before it.
    pub fn read_payload(&self, record: Lsn, skip: u64, buf: &mut [u8]) -> Result<(), LogError> {
        if self.failed {
            return Err(LogError::Failed);
        }
        let identity = self.current.identity;
        let size = identity.segment_size;
        let align = u64::from(identity.align);
        let base = identity.base_of(record);
        let at = record.0 % size;
        if record < self.current.floor || record >= self.tail || at + align > size {
            return Err(LogError::BadPosition(record));
        }
        let Some(&index) = self.live.get(&base) else {
            return Err(Corruption::MissingSegment { base }.into());
        };
        let file = &self.slots[index].file;
        let mut raw = [0u8; RECORD_HEADER];
        file.read_at(at, &mut raw)
            .map_err(|e| read_error("reading the log", &e))?;
        let Some((kind, data_len)) = RecordHeader::peek(&raw, record) else {
            return Err(LogError::BadPosition(record));
        };
        let wanted = skip.checked_add(buf.len() as u64);
        if kind != Kind::Write
            || wanted.is_none_or(|end| end > u64::from(data_len))
            || !identity.fits(at, data_len)
        {
            return Err(LogError::BadPosition(record));
        }
        file.read_at(at + align + skip, buf)
            .map_err(|e| read_error("reading the log", &e))
    }

    /// The most segment files the log may have, or `None` for no limit
    /// (which it is after every open). When a record needs one more,
    /// [`Log::append`] returns [`LogError::Full`]. Files the log already
    /// has stay, whatever the limit.
    pub fn set_segment_limit(&mut self, limit: Option<usize>) {
        self.limit = limit;
    }

    /// Give up the log before `before`, as far as the hold and the
    /// durable position allow, and return where the log starts now.
    /// The new start must be a record boundary, where a record starts or
    /// ends; any other position is refused and nothing changes. Segments that lie wholly
    /// before the new start become free for reuse, and only after the
    /// new start is durable.
    pub fn reclaim(&mut self, before: Lsn) -> Result<Lsn, LogError> {
        if self.failed {
            return Err(LogError::Failed);
        }
        let mut target = before.min(self.durable);
        if let Some(hold) = self.hold {
            if !self.mutants.has(Mutants::IGNORE_HOLD) {
                target = target.min(hold);
            }
        }
        if target <= self.current.floor {
            return Ok(self.current.floor);
        }
        // The start of the log is durable state: a start that is not a
        // record boundary would outlive this process and make the log
        // unreadable from its start. An empty replay at the position
        // walks its segment and refuses a position inside a record or
        // in a gap.
        let walked = if self.mutants.has(Mutants::RECLAIM_UNCHECKED) {
            None
        } else {
            Some(self.walk(target, target, self.tail, &mut |_| {})?)
        };
        // A new epoch, as with every superblock that is written: what
        // is written from now on, a record or the header of a segment
        // file this reclaim frees, then carries an epoch the other copy
        // does not have. If this copy is ever lost, the other one is
        // known to be behind, and is not taken for the log.
        let keep = self
            .mutants
            .has(Mutants::RECLAIM_KEEPS_EPOCH | Mutants::REUSE_BEFORE_FLOOR_SYNC);
        let next = Superblock {
            floor: target,
            durable_floor: self.durable.max(self.current.durable_floor),
            epoch: self.current.epoch + u64::from(!keep),
            ..self.current
        };
        if self.mutants.has(Mutants::REUSE_BEFORE_FLOOR_SYNC) {
            self.current = next;
        } else {
            self.write_superblock(next)?;
        }
        let first = self.current.identity.base_of(target);
        self.live.retain(|base, _| *base >= first);
        // The walk to the new start is where the next replay begins.
        self.start = match (self.start, walked) {
            (Some((_, stale)), _) if self.mutants.has(Mutants::STALE_CURSOR) => {
                Some((first, stale))
            }
            (_, walked) => walked,
        };
        Ok(target)
    }
}

#[cfg(test)]
impl<D: Dir> Log<D> {
    /// For tests that play the part of someone who knows everything
    /// about the log but its key: the header of a discard at
    /// `position`, which must lie in the segment the log ends in,
    /// correct in every field and chained to the records that segment
    /// holds, tagged with the key everybody knows.
    pub(crate) fn lookalike_header(&self, position: Lsn, offset: u64) -> [u8; RECORD_HEADER] {
        let identity = self.current.identity;
        let rules = Rules {
            identity,
            key: self.key,
            max_epoch: self.current.epoch,
            mutants: self.mutants,
        };
        let base = identity.base_of(self.tail);
        let mut chain = chain_seed(&Key::PUBLIC, &identity.volume, base);
        if let Some(slot) = self.live.get(&base) {
            let file = &self.slots[*slot].file;
            let mut reader = Reader {
                device: false,
                checked: true,
            };
            let mut window = Window::default();
            let mut cursor = Cursor {
                position: Lsn(base + identity.unit()),
                chain: rules.seed(base),
                min_epoch: 0,
            };
            while let Ok(Ok(found)) = rules.record(file, &mut reader, &mut window, &cursor) {
                chain = found.header.encode(&Key::PUBLIC, chain).1;
                cursor.position = found.end;
                cursor.chain = found.crc;
            }
        }
        let lookalike = RecordHeader {
            kind: Kind::Discard,
            position,
            epoch: self.current.epoch,
            offset,
            length: 4096,
            data_len: 0,
            data_crc: 0,
        };
        lookalike.encode(&Key::PUBLIC, chain).0
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod prop;

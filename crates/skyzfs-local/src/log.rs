//! The write log: append, sync, recover, replay, reclaim.
//!
//! The format, the argument for recovery and the limits are in the
//! [crate documentation](crate). This module is the log as an object that
//! one writer owns.

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use crate::format::{
    chain_seed, Bad, Identity, Kind, Lsn, RecordHeader, SegmentHeader, Superblock, RECORD_HEADER,
    SECTOR_LEN, SEGMENT_HEADER, SUPER_COPY_STRIDE,
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
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// A record whose payload is damaged is skipped, and the records
    /// after it are accepted.
    pub(crate) const ACCEPT_AFTER_GAP: u32 = 1 << 0;
    /// Recovery does not bump the epoch.
    pub(crate) const NO_EPOCH_BUMP: u32 = 1 << 1;
    /// Recovery does not erase the header at the end of the prefix.
    pub(crate) const NO_TAIL_ERASE: u32 = 1 << 2;
    /// Recovery does not erase segments beyond the prefix.
    pub(crate) const KEEP_STALE_SEGMENTS: u32 = 1 << 3;
    /// Records are not chained: every header is checked against a
    /// constant.
    pub(crate) const NO_CHAIN: u32 = 1 << 4;
    /// A record's epoch is not checked.
    pub(crate) const NO_EPOCH_CHECK: u32 = 1 << 5;
    /// A record's own position is not checked.
    pub(crate) const NO_POSITION_CHECK: u32 = 1 << 6;
    /// A record's payload is not checked: a torn record is accepted.
    pub(crate) const NO_DATA_CHECK: u32 = 1 << 7;
    /// Sync returns without syncing.
    pub(crate) const SYNC_EARLY: u32 = 1 << 8;
    /// Sync syncs only the segment being written.
    pub(crate) const SYNC_ONLY_TAIL: u32 = 1 << 9;
    /// Sync does not sync the directory after a segment file was created.
    pub(crate) const SYNC_NO_DIR: u32 = 1 << 10;
    /// A failed sync does not fail the log; the next one may succeed.
    pub(crate) const RETRY_SYNC: u32 = 1 << 11;
    /// Recovery validates through the cache.
    pub(crate) const VALIDATE_CACHED: u32 = 1 << 12;
    /// Recovery does not sync before it validates.
    pub(crate) const NO_SYNC_BEFORE_VALIDATE: u32 = 1 << 13;
    /// Reclaim frees segments before the new floor is durable.
    pub(crate) const REUSE_BEFORE_FLOOR_SYNC: u32 = 1 << 14;
    /// Reclaim ignores the hold.
    pub(crate) const IGNORE_HOLD: u32 = 1 << 15;
    /// No marker is written when the log fails.
    pub(crate) const NO_MARKER: u32 = 1 << 16;
    /// The epoch of a segment's header is not checked against the
    /// records around it.
    pub(crate) const NO_SEGMENT_EPOCH_CHECK: u32 = 1 << 17;
    /// The superblock is always written to the same copy.
    pub(crate) const ONE_SUPER_COPY: u32 = 1 << 18;
    /// A prefix that ends below the durable floor is accepted.
    pub(crate) const NO_DURABLE_FLOOR: u32 = 1 << 19;
    /// Of two files that claim the same part of the log, the one written
    /// in the earlier epoch is taken.
    pub(crate) const PREFER_OLDER_CLAIM: u32 = 1 << 20;

    fn has(self, flag: u32) -> bool {
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
    /// `open`: the log failed in an earlier life and the caller did not
    /// acknowledge it.
    #[error("the log is marked as failed")]
    MarkedFailed,
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
    /// The header's epoch is above the superblock's. Records are only
    /// written in an epoch the superblock already holds, so the
    /// superblock that was read is not the newest that was written.
    FutureEpoch,
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
    /// Where the valid prefix ends. The log continues from here.
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
    /// The next record's position.
    position: Lsn,
    /// What its header must be chained to.
    chain: u32,
    /// The lowest epoch it may have.
    min_epoch: u64,
}

/// What a walk needs to know about the log.
#[derive(Debug, Clone, Copy)]
struct Rules {
    identity: Identity,
    /// The highest epoch a record may have.
    max_epoch: u64,
    mutants: Mutants,
}

impl Rules {
    fn seed(&self, base: u64) -> u32 {
        if self.mutants.has(Mutants::NO_CHAIN) {
            0
        } else {
            chain_seed(&self.identity.volume, base)
        }
    }

    /// Read and validate the record at the cursor in `file`. Its payload
    /// is left in `window`.
    fn record<F: File>(
        &self,
        file: &F,
        reader: &mut Reader,
        window: &mut Window,
        cursor: &Cursor,
    ) -> io::Result<Result<(RecordHeader, u32, bool), Stop>> {
        let id = &self.identity;
        let size = id.segment_size;
        let at = cursor.position.0 % size;
        if at + RECORD_HEADER as u64 > size {
            return Ok(Err(Stop::Length));
        }
        let mut raw = [0u8; RECORD_HEADER];
        raw.copy_from_slice(window.fetch(file, reader, at, RECORD_HEADER, size)?);
        let chain = if self.mutants.has(Mutants::NO_CHAIN) {
            0
        } else {
            cursor.chain
        };
        let (header, crc) = match RecordHeader::decode(&raw, chain) {
            Ok(decoded) => decoded,
            Err(bad) => return Ok(Err(Stop::Header(bad))),
        };
        if header.position != cursor.position && !self.mutants.has(Mutants::NO_POSITION_CHECK) {
            return Ok(Err(Stop::Position));
        }
        if header.epoch > self.max_epoch {
            return Ok(Err(Stop::FutureEpoch));
        }
        if header.epoch < cursor.min_epoch && !self.mutants.has(Mutants::NO_EPOCH_CHECK) {
            return Ok(Err(Stop::Epoch));
        }
        let align = u64::from(id.align);
        if header.kind == Kind::Pad {
            return Ok(if at + align <= size {
                Ok((header, crc, false))
            } else {
                Err(Stop::Length)
            });
        }
        if header.data_len > id.max_data() || at + id.record_size(header.data_len) + align > size {
            return Ok(Err(Stop::Length));
        }
        let data = window.fetch(file, reader, at + align, header.data_len as usize, size)?;
        let mut damaged = false;
        if crc32c::crc32c(data) != header.data_crc && !self.mutants.has(Mutants::NO_DATA_CHECK) {
            if !self.mutants.has(Mutants::ACCEPT_AFTER_GAP) {
                return Ok(Err(Stop::Data));
            }
            damaged = true;
        }
        Ok(Ok((header, crc, damaged)))
    }
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
    file.sync().is_ok() && dir.sync().is_ok()
}

fn read_error(during: &'static str, e: &io::Error) -> LogError {
    LogError::Io {
        during,
        kind: e.kind(),
        marker_written: false,
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
    copy: u64,
    slots: Vec<Slot<D::File>>,
    /// The segments of the log, by first position.
    live: BTreeMap<u64, usize>,
    tail: Lsn,
    chain: u32,
    durable: Lsn,
    hold: Option<Lsn>,
    failed: bool,
    /// Segment files with writes no sync has covered.
    dirty: BTreeSet<usize>,
    /// A segment file was created and the directory not synced since.
    dir_dirty: bool,
    scratch: Vec<u8>,
    mutants: Mutants,
}

impl<D: Dir> Log<D> {
    /// Make a new, empty log in `dir` and open it.
    pub fn create(dir: D, identity: Identity) -> Result<Self, LogError> {
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
        segment
            .write_at(0, &header.encode())
            .map_err(io("creating a segment"))?;
        segment.sync().map_err(io("creating a segment"))?;
        let mut superblock = dir
            .create(SUPER_NEW_NAME)
            .map_err(io("creating the superblock"))?;
        superblock
            .preallocate(SUPER_COPY_STRIDE + SECTOR_LEN as u64)
            .map_err(io("creating the superblock"))?;
        let first = Superblock {
            identity,
            counter: 1,
            epoch: 1,
            floor: Lsn(SEGMENT_HEADER),
            durable_floor: Lsn(SEGMENT_HEADER),
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
        Self::open(dir, Options::default()).map(|(log, _)| log)
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
                if let Ok(header) = SegmentHeader::decode(&raw) {
                    let valid =
                        header.volume == identity.volume && header.base.is_multiple_of(size);
                    if valid && header.epoch > current.epoch {
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
            position: Lsn(base + SEGMENT_HEADER),
            chain: rules.seed(base),
            min_epoch: segment_epoch,
        };
        if mutants.has(Mutants::NO_SEGMENT_EPOCH_CHECK) {
            cursor.min_epoch = 0;
        }
        let stopped = loop {
            let file = &slots[index].file;
            let step = rules
                .record(file, &mut reader, &mut window, &cursor)
                .map_err(|e| read_error("reading the log", &e))?;
            let (header, crc, _) = match step {
                Ok(found) => found,
                Err(stop) => break stop,
            };
            cursor.min_epoch = cursor.min_epoch.max(header.epoch);
            if header.kind != Kind::Pad {
                cursor.position = Lsn(cursor.position.0 + identity.record_size(header.data_len));
                cursor.chain = crc;
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
                position: Lsn(base + SEGMENT_HEADER),
                chain: rules.seed(base),
                min_epoch: cursor.min_epoch.max(segment_epoch),
            };
            if mutants.has(Mutants::NO_SEGMENT_EPOCH_CHECK) {
                cursor.min_epoch = 0;
            }
        };
        let end = cursor.position;
        if stopped == Stop::FutureEpoch {
            // The epoch is in the header that stopped the walk; say which.
            let raw = window
                .fetch(
                    &slots[index].file,
                    &mut reader,
                    end.0 % size,
                    RECORD_HEADER,
                    size,
                )
                .map_err(|e| read_error("reading the log", &e))?;
            let mut found = [0u8; 8];
            found.copy_from_slice(&raw[16..24]);
            return Err(Corruption::SuperblockBehind {
                position: end,
                found: u64::from_le_bytes(found),
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
        let follows = Self::scan(&identity, &slots[index].file, &mut reader, &mut window, end)
            .map_err(|e| read_error("reading the log", &e))?;

        let mut log = Self {
            dir,
            superblock,
            current,
            copy,
            slots,
            live,
            tail: end,
            chain: cursor.chain,
            durable: end,
            hold: None,
            failed: false,
            dirty: BTreeSet::new(),
            dir_dirty: false,
            scratch: Vec::new(),
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
        // 2. Erase the header at the end of the prefix, so that what was
        //    there cannot validate again under a record that is written
        //    over it and then partly lost.
        if !mutants.has(Mutants::NO_TAIL_ERASE) {
            let at = end.0 % size;
            let file = &mut log.slots[index].file;
            let erased = file
                .write_at(at, &[0u8; RECORD_HEADER])
                .and_then(|()| file.sync());
            if let Err(e) = erased {
                return Err(log.fail("erasing the tail", &e));
            }
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
    fn chain(&self) -> u32 {
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
            marker_written: write_marker(&self.dir, self.mutants),
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

    /// Close the current segment and continue in the next.
    fn roll(&mut self) -> Result<(), LogError> {
        let identity = self.current.identity;
        let size = identity.segment_size;
        let base = identity.base_of(self.tail);
        let Some(&index) = self.live.get(&base) else {
            return Err(Corruption::MissingSegment { base }.into());
        };
        let pad = RecordHeader {
            kind: Kind::Pad,
            position: self.tail,
            epoch: self.current.epoch,
            offset: 0,
            length: base + size - self.tail.0,
            data_len: 0,
            data_crc: 0,
        };
        let (bytes, _) = pad.encode(self.chain());
        if let Err(e) = self.slots[index].file.write_at(self.tail.0 % size, &bytes) {
            return Err(self.fail("closing a segment", &e));
        }
        self.dirty.insert(index);

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
        .and_then(|()| slot.file.write_at(0, &header.encode()));
        if let Err(e) = prepared {
            return Err(self.fail("starting a segment", &e));
        }
        self.slots[next_index].sized = true;
        self.dirty.insert(next_index);
        self.live.insert(next, next_index);
        self.tail = Lsn(next + SEGMENT_HEADER);
        self.chain = chain_seed(&identity.volume, next);
        Ok(())
    }

    /// Append a record. When this returns `Ok`, the record's write has
    /// returned, and so has the write of every record before it: it
    /// survives the loss of the process. It survives a power cut once a
    /// [`Log::sync`] that started after this call has returned `Ok`.
    ///
    /// `data` is the payload of a [`Kind::Write`] and must be empty for
    /// the other kinds. [`Kind::Pad`] is the log's own and is refused.
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
            Kind::Pad => false,
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
        // A segment always keeps room for the marker that closes it.
        if self.tail.0 % size + record + align > size {
            self.roll()?;
        }
        let base = identity.base_of(self.tail);
        let Some(&index) = self.live.get(&base) else {
            return Err(Corruption::MissingSegment { base }.into());
        };
        let header = RecordHeader {
            kind,
            position: self.tail,
            epoch: self.current.epoch,
            offset,
            length,
            data_len,
            data_crc: crc32c::crc32c(data),
        };
        let (bytes, crc) = header.encode(self.chain());
        // Header and payload go out in one write, each padded to the
        // alignment.
        let mut scratch = std::mem::take(&mut self.scratch);
        scratch.clear();
        scratch.resize(record as usize, 0);
        scratch[..RECORD_HEADER].copy_from_slice(&bytes);
        scratch[align as usize..align as usize + data.len()].copy_from_slice(data);
        let written = self.slots[index]
            .file
            .write_at(self.tail.0 % size, &scratch);
        self.scratch = scratch;
        if let Err(e) = written {
            return Err(self.fail("appending a record", &e));
        }
        self.dirty.insert(index);
        let start = self.tail;
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

    /// Call `visit` with every record from `from` up to `to`, in order.
    /// Both must be record boundaries inside the log: positions that
    /// [`Log::append`], [`Log::tail`], [`Log::floor`] or a recovery
    /// returned. Every record is checked again as it is read, and a
    /// damaged one is an error, never data.
    pub fn replay(
        &self,
        from: Lsn,
        to: Lsn,
        visit: &mut dyn FnMut(&Record<'_>),
    ) -> Result<(), LogError> {
        if from < self.current.floor || from > to {
            return Err(LogError::BadPosition(from));
        }
        if to > self.tail {
            return Err(LogError::BadPosition(to));
        }
        let identity = self.current.identity;
        let size = identity.segment_size;
        let rules = Rules {
            identity,
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
            position: Lsn(base + SEGMENT_HEADER),
            chain: rules.seed(base),
            min_epoch: 0,
        };
        while cursor.position < to {
            let Some(&index) = self.live.get(&base) else {
                return Err(Corruption::MissingSegment { base }.into());
            };
            let step = rules
                .record(&self.slots[index].file, &mut reader, &mut window, &cursor)
                .map_err(|e| read_error("reading the log", &e))?;
            let (header, crc, damaged) = step.map_err(|stopped| Corruption::Record {
                position: cursor.position,
                stopped,
            })?;
            if header.kind == Kind::Pad {
                base += size;
                window.reset();
                cursor = Cursor {
                    position: Lsn(base + SEGMENT_HEADER),
                    chain: rules.seed(base),
                    min_epoch: header.epoch,
                };
                continue;
            }
            let start = cursor.position;
            let end = Lsn(start.0 + identity.record_size(header.data_len));
            if start < from && end > from {
                return Err(LogError::BadPosition(from));
            }
            if end > to {
                return Err(LogError::BadPosition(to));
            }
            if start >= from && !damaged {
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
                chain: crc,
                min_epoch: header.epoch,
            };
        }
        Ok(())
    }

    /// Give up the log before `before`, as far as the hold and the
    /// durable position allow, and return where the log starts now.
    /// `before` must be a record boundary. Segments that lie wholly
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
        let next = Superblock {
            floor: target,
            durable_floor: self.durable.max(self.current.durable_floor),
            ..self.current
        };
        if self.mutants.has(Mutants::REUSE_BEFORE_FLOOR_SYNC) {
            self.current = next;
        } else {
            self.write_superblock(next)?;
        }
        let first = self.current.identity.base_of(target);
        self.live.retain(|base, _| *base >= first);
        Ok(target)
    }
}

#[cfg(test)]
mod tests;

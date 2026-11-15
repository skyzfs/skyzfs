//! What `skyzfsd status` reads: a small file the serving daemon keeps
//! under the volume's directory in the run directory, and the report
//! made from it, the attachment record, the device's note and the
//! kernel's word on the device. `status` opens nothing else: not the
//! store, and no channel to the daemon.
//!
//! # The file
//!
//! [`STATUS`] under the volume's run directory
//! ([`records::volume_run_dir`](crate::records::volume_run_dir)). 312
//! bytes, little endian. It is replaced whole (made under another name
//! and renamed) and never synced.
//!
//! The file has no checksum. It is replaced whole by a rename, lives
//! on a memory file system for one boot, and is read only if its
//! length and its magic are exactly a version's: a torn or foreign
//! file is read as no status, and a damaged counter would mislead a
//! reader of `status` and nothing else. The failure note
//! ([`records`](crate::records)) is kept the same way.
//!
//! This is the file's twelfth version (M2.1b). It adds whether the
//! newest cut opens as itself (design §5.2), after the eleventh's
//! fields (M2.1). It drops the eleventh's four fields of the pump's
//! time from a completion being ready in the store to the pump taking
//! it (the eleventh's offset 216, 32 bytes, printed as
//! `pump_taken_*`): only the log store said when a completion was
//! ready, and since it was removed they were 0. The in-place store
//! completes a request on the pump's own thread
//! ([`PumpTiming::read_to_commit`]). So the eleventh's fields after the
//! pump's times lie 32 bytes earlier here. The eleventh dropped
//! the fields that only the log store filled, which the tenth still
//! carried as zeros after the log store was removed (M1.3t): the log's
//! length, its checkpoint steps and syncs, what a start replayed and
//! recovered beyond what was acknowledged, the store's worker's and
//! reader's times, the hint that started writeback early, the syncs by
//! kind, and the record of the acknowledged position; and it added the
//! count of cuts and the newest cut's group.
//!
//! The eleventh (`SKYZST11`, 336 bytes) is still read, without whether
//! the newest cut opens as itself: up to the pump's times its fields
//! lie at this version's places, its four fields of the time to the
//! taking are not read, and its later fields lie 32 bytes later than
//! here. So are the tenth (`SKYZST10`, 2032 bytes), the ninth
//! (`SKYZSTA9`, 2008 bytes) and the eighth (`SKYZSTA8`, 1960 bytes):
//! what this version keeps lies at the same places in all three, the
//! ninth has nothing of the commands the driver ended otherwise than OK
//! (from offset 2008 of the tenth), and the eighth nothing of the I/O
//! threads' queue (from offset 1960). None of them has cuts, and their
//! time to the taking is not read either. A file of an older version,
//! from a daemon before M1.3m, is read as no status.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZST12` |
//! | 8 | 4 | the daemon's process id |
//! | 12 | 2 | queues |
//! | 14 | 2 | depth of each queue |
//! | 16 | 16 | the boot it was written in |
//! | 32 | 16 | the store's identity |
//! | 48 | 8 | when it was written: milliseconds since boot |
//! | 56 | 8 | reads committed |
//! | 64 | 8 | writes committed |
//! | 72 | 8 | flushes committed |
//! | 80 | 8 | discards committed |
//! | 88 | 8 | write-zeroes committed |
//! | 96 | 8 | requests refused by the translation |
//! | 104 | 8 | `held`: requests fetched and not committed yet |
//! | 112 | 8 | writes of this file that failed |
//! | 120 | 8 | `first_write_copies`: first writes of a tag that were copied from the transport's own buffer since this daemon started serving (at most the depth) |
//! | 128 | 8 | `start_ms`: how long this daemon's start took until the device served |
//! | 136 | 8 | `zeroes_applied`: how write-zeroes reach the home image. 0: none since this daemon started serving; 1: their ranges are deallocated; 2: zeros are written, because the file system cannot deallocate (both only in a test build's I/O through the kernel's cache); 4: zeros are written by direct I/O. Any other value (3 was the log store's) is read as "not said" |
//! | 144 | 72 | the pump, before the store has a request: how many requests it submitted, their summed time from the queue's wait returning with the request to the store having it, the longest; the same after the store is done, from the request being finished to its commit, for reads and then for every other request |
//! | 216 | 48 | the store's I/O threads: how many (0 for a test build that does the I/O on the caller's thread); reads and writes on them now; requests waiting now for a thread, in their order, because one on the threads overlaps them or a flush is behind a write still there, and everything received after the first of them; the requests that waited so since the store began (only grows); the most reads and writes on the threads at once; the most requests waiting at once |
//! | 264 | 8 | `commands_aborted`: the queues' commands the driver ended with its abort (`-ENODEV`): the queue is being stopped. Each tag's last command ends so |
//! | 272 | 8 | `commands_failed`: the queues' commands that ended with any other result than OK: a commit or a fetch the driver refused, or one io_uring could not hand to it. Nothing is done about them but this count and the request log's line (see [`Queue::wait`](crate::pump::Queue::wait)) |
//! | 280 | 8 | `last_command_failure`: the result of the last of those, a negated error number, in the first 4 bytes as a signed number; 0 before the first |
//! | 288 | 8 | `cuts`: the cuts the store has counted since this daemon started serving (design §5.2). Not kept across a start: each daemon counts from 0 |
//! | 296 | 8 | `cut_group`: the newest cut's transaction group; 0 before the first cut |
//! | 304 | 8 | `cut_opens`: whether the newest cut opens as itself (design §5.2): 1 if an import of the volume at its instant, with no rewind, opens its uberblock's pool state; 2 if it opens only with a rewind to its group; 0 before the first cut |
//!
//! All times are nanoseconds of the monotonic clock, since this daemon
//! started serving. Sums and counts only grow, so a reader takes the
//! difference of two looks for a window; a longest time is of the whole
//! time served and has no difference. What each number means exactly
//! is at [`PumpTiming`].
//!
//! # The rules
//!
//! - **It is off the path of requests, in both directions.** The queue
//!   thread only puts its counters on a [`Board`], which takes a lock
//!   for a copy and does no I/O. A thread of its own
//!   ([`Board::publish`]) writes the file. A write that fails holds no
//!   request and fails none: it is counted, and tried again no sooner
//!   than the next interval.
//! - **It converges.** After the last change, the file shows it within
//!   one interval plus the time of one write, with no further request
//!   arriving: the publishing thread is woken by the change itself and
//!   sleeps only until the interval since its last write is over. With
//!   the interval `serve` uses (100 ms) the stated bound is 200 ms.
//! - **Nothing is rewritten while nothing changes**, and the publishing
//!   thread does not wake while nothing changes: it waits for a change
//!   without a time limit.
//! - **At most one write per interval**, however fast the counters
//!   move, **except a write asked for again** ([`Board::write_again`]),
//!   which is made at once.
//! - When the daemon stops in order, the last counters are written
//!   before [`Board::publish`] returns, whatever the interval.
//!
//! # The time
//!
//! "Milliseconds since boot" is the first field of `/proc/uptime`,
//! which counts time in suspend and has a hundredth of a second.
//! (The crate has no `unsafe`, so it does not call `clock_gettime`.)
//! `status` prints the age of the counters from it.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::lifecycle::{DeviceState, Held, Located, Whose};
use crate::pump::{Counters, PumpTiming};
use crate::records::StoredFailure;
use skyzfs_local::inplace::{Cut, Cuts, IoQueue};
use skyzfs_local::store::Spent;

/// The file's name under the volume's run directory.
pub const STATUS: &str = "status";

const MAGIC: &[u8; 8] = b"SKYZST12";
/// The version this crate writes.
pub const VERSION: u32 = 12;
const LEN: usize = 312;

/// The eleventh version: the twelfth without whether the newest cut
/// opens as itself, and with four fields of the pump's time to the
/// taking after its other times (offset 216, 32 bytes), so that the
/// fields after them lie [`ELEVENTH_SHIFT`] bytes later.
const ELEVENTH_MAGIC: &[u8; 8] = b"SKYZST11";
const ELEVENTH_LEN: usize = 336;
/// How much later than here the eleventh version's fields after the
/// pump's times lie.
const ELEVENTH_SHIFT: usize = 32;

/// The tenth version, the last with the log store's fields. A file of
/// it is still read, without them and without cuts.
const TENTH_MAGIC: &[u8; 8] = b"SKYZST10";
const TENTH_LEN: usize = 2032;
/// The ninth version: the tenth without the commands the driver ended
/// otherwise than OK (offset 2008), which are read as zero.
const NINTH_MAGIC: &[u8; 8] = b"SKYZSTA9";
const NINTH_LEN: usize = 2008;
/// The eighth version: the ninth without the I/O threads' queue (offset
/// 1960), which is read as absent.
const EIGHTH_MAGIC: &[u8; 8] = b"SKYZSTA8";
const EIGHTH_LEN: usize = 1960;

/// What the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The version of the file it was read from: 8 to [`VERSION`]. A
    /// file is always written as [`VERSION`], whatever this says.
    /// `status` prints it (`status_version`), so that whoever reads
    /// it can tell a field an older file did not have from a zero the
    /// daemon counted.
    pub version: u32,
    /// The daemon's process id.
    pub pid: u32,
    /// Queues.
    pub queues: u16,
    /// Depth of each queue.
    pub depth: u16,
    /// The boot it was written in.
    pub boot: [u8; 16],
    /// The store's identity.
    pub identity: [u8; 16],
    /// When it was written, in milliseconds since boot.
    pub written_ms: u64,
    /// The pump's counters.
    pub counters: Counters,
    /// Writes of the file that failed.
    pub failures: u64,
    /// First writes of a tag that were copied once more, from the
    /// transport's own buffer, since this daemon started serving.
    pub first_write_copies: u64,
    /// How long the start took until the device served, in
    /// milliseconds.
    pub start_ms: u64,
    /// The store and the pump as the queue thread posts them.
    pub shown: StoreShown,
}

/// The store and the pump as the queue thread posts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StoreShown {
    /// How write-zeroes reach the home image.
    pub zeroes: ZeroesShown,
    /// Where the pump's time went.
    pub pump: PumpTiming,
    /// The store's I/O threads' queue. `None` in a file of the eighth
    /// version.
    pub io_queue: Option<IoQueue>,
    /// The count of cuts and the newest cut. `None` in a file of a
    /// version before the eleventh.
    pub cuts: Option<Cuts>,
}

/// How write-zeroes reach the home image, as the status file says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZeroesShown {
    /// The file does not say: it holds a value this version does not
    /// know (3 was the log store's, for blocks larger than its write
    /// unit).
    #[default]
    Unsaid,
    /// None has been applied since the daemon started serving.
    Untried,
    /// Their ranges are deallocated: nothing is written for them. Only
    /// a test build's I/O through the kernel's cache.
    Deallocated,
    /// Zeros are written through the kernel's cache: the file system
    /// cannot deallocate. Only a test build's I/O through the cache.
    Written,
    /// Zeros are written by direct I/O: the in-place store, where a
    /// deallocated range would make the next writes to it allocate
    /// again.
    WrittenDirect,
}

impl ZeroesShown {
    fn code(self) -> u64 {
        match self {
            Self::Untried => 0,
            Self::Deallocated => 1,
            Self::Written => 2,
            Self::WrittenDirect => 4,
            Self::Unsaid => u64::MAX,
        }
    }

    fn of(code: u64) -> Self {
        match code {
            0 => Self::Untried,
            1 => Self::Deallocated,
            2 => Self::Written,
            4 => Self::WrittenDirect,
            _ => Self::Unsaid,
        }
    }

    /// The report's words, or none where the file does not say.
    fn said(self) -> Option<&'static str> {
        match self {
            Self::Unsaid => None,
            Self::Untried => Some("not yet: none applied since this daemon started serving"),
            Self::Deallocated => Some(
                "deallocated in the home image: nothing is written for them (a test build's \
                 I/O through the kernel's cache)",
            ),
            Self::Written => Some(
                "written as zeros through the kernel's cache: the file system of the volume's \
                 directory cannot deallocate (a test build's I/O through the cache)",
            ),
            Self::WrittenDirect => Some(
                "written as zeros by direct I/O: the in-place store does not deallocate, since \
                 the next writes to a deallocated range would allocate it again, and a \
                 write-zeroes takes as long as writing its zeros",
            ),
        }
    }
}

/// The keys of the pump's times in `status`, in the file's order. All
/// are nanoseconds but the three counts. A key ending in `_max_ns` is
/// the longest since the daemon started serving; every other value only
/// grows, and a script takes the difference of two looks. Through
/// version 10 they were printed as `time_pump_*`, with their values for
/// the in-place store too, among the log store's `time_*` keys, which
/// that store left at zero. Version 11 drops the log store's keys and
/// prints the pump's as `pump_*`: the benchmark reads them under the
/// new names on a version 11 file (M2.T0b). Version 12 drops the four
/// `pump_taken_*` keys, whatever version the file is: they were 0 since
/// the log store was removed. A reader of version 11 that took the
/// difference of `pump_commit_*` and `pump_taken_*` gets the same
/// number from `pump_commit_*` alone.
pub const PUMP_KEYS: [&str; PUMP_FIELDS] = [
    "pump_submit_count",
    "pump_submit_ns",
    "pump_submit_max_ns",
    "pump_commit_read_count",
    "pump_commit_read_ns",
    "pump_commit_read_max_ns",
    "pump_commit_other_count",
    "pump_commit_other_ns",
    "pump_commit_other_max_ns",
];

/// The number of 8-byte fields the pump's times take.
const PUMP_FIELDS: usize = 9;

/// The pump's times in the file's order: see the table at the top.
fn pump_fields(pump: &PumpTiming) -> [u64; PUMP_FIELDS] {
    let mut fields = [0u64; PUMP_FIELDS];
    let mut at = 0;
    let mut put = |value: u64| {
        fields[at] = value;
        at += 1;
    };
    for spent in [pump.to_submit, pump.read_to_commit, pump.other_to_commit] {
        put(spent.count);
        put(spent.sum_ns);
        put(spent.max_ns);
    }
    fields
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut field = [0u8; 8];
    field.copy_from_slice(&bytes[at..at + 8]);
    u64::from_le_bytes(field)
}

fn encode(status: &Status) -> [u8; LEN] {
    let mut bytes = [0u8; LEN];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..12].copy_from_slice(&status.pid.to_le_bytes());
    bytes[12..14].copy_from_slice(&status.queues.to_le_bytes());
    bytes[14..16].copy_from_slice(&status.depth.to_le_bytes());
    bytes[16..32].copy_from_slice(&status.boot);
    bytes[32..48].copy_from_slice(&status.identity);
    let c = &status.counters;
    let shown = &status.shown;
    let q = shown.io_queue.unwrap_or_default();
    let cuts = shown.cuts.unwrap_or_default();
    let fields = [
        status.written_ms,
        c.reads,
        c.writes,
        c.flushes,
        c.discards,
        c.write_zeroes,
        c.refused,
        c.in_flight,
        status.failures,
        status.first_write_copies,
        status.start_ms,
        shown.zeroes.code(),
    ];
    let after = [
        q.threads,
        q.in_flight,
        q.waiting,
        q.waited,
        q.in_flight_max,
        q.waiting_max,
        c.commands_aborted,
        c.commands_failed,
        // The last command's failure, in the field's first 4 bytes.
        u64::from(u32::from_le_bytes(c.last_command_failure.to_le_bytes())),
        cuts.count,
        cuts.newest.map_or(0, |cut| cut.group),
        cuts.newest.map_or(0, |cut| if cut.itself { 1 } else { 2 }),
    ];
    let pump = pump_fields(&shown.pump);
    for (i, field) in fields.iter().chain(&pump).chain(&after).enumerate() {
        bytes[48 + 8 * i..56 + 8 * i].copy_from_slice(&field.to_le_bytes());
    }
    bytes
}

/// Where the fields this version keeps lie in a file of the eighth,
/// ninth or tenth version.
mod tenth {
    pub(super) const FAILURES: usize = 120;
    pub(super) const FIRST_WRITE_COPIES: usize = 128;
    pub(super) const START_MS: usize = 168;
    pub(super) const ZEROES: usize = 192;
    /// The pump's times: three times three fields. The time to the
    /// taking, at 1440, is not read.
    pub(super) const PUMP: usize = 1336;
    pub(super) const IO_QUEUE: usize = 1960;
    pub(super) const COMMANDS: usize = 2008;
}

/// The pump's times from `fields`, in the order of [`pump_fields`].
fn pump_of(fields: &[u64; PUMP_FIELDS]) -> PumpTiming {
    let spent = |at: usize| Spent {
        count: fields[at],
        sum_ns: fields[at + 1],
        max_ns: fields[at + 2],
    };
    PumpTiming {
        to_submit: spent(0),
        read_to_commit: spent(3),
        other_to_commit: spent(6),
    }
}

fn decode(bytes: &[u8]) -> Option<Status> {
    let is = |len: usize, magic: &[u8; 8]| bytes.len() == len && &bytes[..8] == magic;
    let version = if is(LEN, MAGIC) {
        12
    } else if is(ELEVENTH_LEN, ELEVENTH_MAGIC) {
        11
    } else if is(TENTH_LEN, TENTH_MAGIC) {
        10
    } else if is(NINTH_LEN, NINTH_MAGIC) {
        9
    } else if is(EIGHTH_LEN, EIGHTH_MAGIC) {
        8
    } else {
        return None;
    };
    let mut boot = [0u8; 16];
    boot.copy_from_slice(&bytes[16..32]);
    let mut identity = [0u8; 16];
    identity.copy_from_slice(&bytes[32..48]);
    // Up to the counters, every version has the same places.
    let mut counters = Counters {
        reads: u64_at(bytes, 56),
        writes: u64_at(bytes, 64),
        flushes: u64_at(bytes, 72),
        discards: u64_at(bytes, 80),
        write_zeroes: u64_at(bytes, 88),
        refused: u64_at(bytes, 96),
        in_flight: u64_at(bytes, 104),
        ..Counters::default()
    };
    let field = |at: usize| u64_at(bytes, at);
    let failure_at =
        |at: usize| i32::from_le_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
    let (failures, first_write_copies, start_ms, shown) = if version >= 11 {
        let pump: [u64; PUMP_FIELDS] = std::array::from_fn(|i| field(144 + 8 * i));
        // After the pump's times: the eleventh's fields lie later.
        let after = |at: usize| {
            if version == 11 {
                at + ELEVENTH_SHIFT
            } else {
                at
            }
        };
        counters.commands_aborted = field(after(264));
        counters.commands_failed = field(after(272));
        counters.last_command_failure = failure_at(after(280));
        let count = field(after(288));
        let cuts = Cuts {
            count,
            // The eleventh version does not say whether the newest cut
            // opens as itself: read as not, and `status` prints nothing
            // of it for that version.
            newest: (count > 0).then(|| Cut {
                number: count,
                group: field(after(296)),
                itself: version >= 12 && field(304) == 1,
            }),
        };
        let shown = StoreShown {
            zeroes: ZeroesShown::of(field(136)),
            pump: pump_of(&pump),
            io_queue: Some(io_queue_at(bytes, after(216))),
            cuts: Some(cuts),
        };
        (field(112), field(120), field(128), shown)
    } else {
        let pump: [u64; PUMP_FIELDS] = std::array::from_fn(|i| field(tenth::PUMP + 8 * i));
        if version == 10 {
            counters.commands_aborted = field(tenth::COMMANDS);
            counters.commands_failed = field(tenth::COMMANDS + 8);
            counters.last_command_failure = failure_at(tenth::COMMANDS + 16);
        }
        let shown = StoreShown {
            zeroes: ZeroesShown::of(field(tenth::ZEROES)),
            pump: pump_of(&pump),
            io_queue: (version >= 9).then(|| io_queue_at(bytes, tenth::IO_QUEUE)),
            cuts: None,
        };
        (
            field(tenth::FAILURES),
            field(tenth::FIRST_WRITE_COPIES),
            field(tenth::START_MS),
            shown,
        )
    };
    Some(Status {
        version,
        pid: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        queues: u16::from_le_bytes([bytes[12], bytes[13]]),
        depth: u16::from_le_bytes([bytes[14], bytes[15]]),
        boot,
        identity,
        written_ms: field(48),
        counters,
        failures,
        first_write_copies,
        start_ms,
        shown,
    })
}

/// The I/O threads' queue, six fields from `at`.
fn io_queue_at(bytes: &[u8], at: usize) -> IoQueue {
    IoQueue {
        threads: u64_at(bytes, at),
        in_flight: u64_at(bytes, at + 8),
        waiting: u64_at(bytes, at + 16),
        waited: u64_at(bytes, at + 24),
        in_flight_max: u64_at(bytes, at + 32),
        waiting_max: u64_at(bytes, at + 40),
    }
}

/// Whether `bytes` are a whole file of a version before the eighth,
/// which is read as no status: by its length and its magic.
fn is_older(bytes: &[u8]) -> bool {
    const OLDER: [(usize, &[u8; 8]); 7] = [
        (1856, b"SKYZSTA7"),
        (1584, b"SKYZSTA6"),
        (1424, b"SKYZSTA5"),
        (200, b"SKYZSTA4"),
        (192, b"SKYZSTA3"),
        (144, b"SKYZSTA2"),
        (128, b"SKYZSTA1"),
    ];
    OLDER
        .iter()
        .any(|(len, magic)| bytes.len() == *len && &bytes[..8] == *magic)
}

/// Read the file at `path`. `Ok(None)` if there is none, or one of a
/// version before the eighth. Any other file that is not a whole record
/// is an error.
pub fn read(path: &Path) -> io::Result<Option<Status>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if is_older(&bytes) {
        return Ok(None);
    }
    decode(&bytes)
        .map(Some)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "not a status file"))
}

/// Put `status` at `path`: whole under another name, then renamed. Not
/// synced.
pub fn write(path: &Path, status: &Status) -> io::Result<()> {
    let mut beside = PathBuf::from(path).into_os_string();
    beside.push(".new");
    let beside = PathBuf::from(beside);
    fs::File::create(&beside)?.write_all(&encode(status))?;
    fs::rename(&beside, path)
}

/// Parse the first field of `/proc/uptime` into milliseconds.
pub fn parse_uptime(text: &str) -> Option<u64> {
    let first = text.split_whitespace().next()?;
    let (seconds, fraction) = first.split_once('.').unwrap_or((first, ""));
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    if !digits(seconds) || !(fraction.is_empty() || digits(fraction)) {
        return None;
    }
    let mut millis = format!("{fraction:0<3}");
    millis.truncate(3);
    let whole = seconds.parse::<u64>().ok()?.checked_mul(1000)?;
    whole.checked_add(millis.parse::<u64>().ok()?)
}

/// Milliseconds since boot, as the kernel tells them.
pub fn uptime_ms() -> io::Result<u64> {
    let text = fs::read_to_string("/proc/uptime")?;
    parse_uptime(&text)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "/proc/uptime is not a time"))
}

/// When the file is due.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// Not until the counters change.
    Never,
    /// Now.
    Now,
    /// At this time, in the caller's milliseconds.
    At(u64),
}

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// A change that comes within the interval of the last write is
    /// written only when another change follows.
    pub(crate) const NEEDS_ANOTHER_CHANGE: u32 = 1 << 0;
    /// The file is written every interval, changed or not.
    pub(crate) const REWRITES_UNCHANGED: u32 = 1 << 1;
    /// Every change is written at once.
    pub(crate) const NO_INTERVAL: u32 = 1 << 2;
    /// A failed write is tried again at once.
    pub(crate) const RETRIES_AT_ONCE: u32 = 1 << 3;
    /// A write asked for with [`Board::write_again`] that fails is
    /// forgotten.
    pub(crate) const OWED_FORGOTTEN: u32 = 1 << 4;
    /// Every post wakes the publisher, also while it waits for the end
    /// of an interval.
    pub(crate) const WOKEN_BY_EVERY_POST: u32 = 1 << 5;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// The pace of the writes: at most one per interval, none while nothing
/// changes, and a change is never left unwritten.
#[derive(Debug, Clone)]
pub struct Pace {
    interval_ms: u64,
    written: Option<Counters>,
    tried_at: Option<u64>,
    mutants: Mutants,
}

impl Pace {
    /// A pace of at most one write in `interval_ms`.
    pub fn new(interval_ms: u64) -> Self {
        Self {
            interval_ms,
            written: None,
            tried_at: None,
            mutants: Mutants::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn with(mut self, mutants: u32) -> Self {
        self.mutants = Mutants(mutants);
        self
    }

    /// When `current` is due, at the time `now`.
    pub fn due(&self, current: &Counters, now: u64) -> Due {
        let free_at = self.tried_at.map(|at| at.saturating_add(self.interval_ms));
        let free = self.mutants.has(Mutants::NO_INTERVAL) || free_at.is_none_or(|at| now >= at);
        if self.written.as_ref() == Some(current) {
            return match free_at {
                Some(at) if self.mutants.has(Mutants::REWRITES_UNCHANGED) && !free => Due::At(at),
                _ if self.mutants.has(Mutants::REWRITES_UNCHANGED) => Due::Now,
                _ => Due::Never,
            };
        }
        match free_at {
            _ if free => Due::Now,
            _ if self.mutants.has(Mutants::NEEDS_ANOTHER_CHANGE) => Due::Never,
            Some(at) => Due::At(at),
            None => Due::Now,
        }
    }

    /// When something that is not in the counters is due, at the time
    /// `now`: as for counters that changed.
    pub fn due_changed(&self, now: u64) -> Due {
        let free_at = self.tried_at.map(|at| at.saturating_add(self.interval_ms));
        match free_at {
            Some(at) if now < at => Due::At(at),
            _ => Due::Now,
        }
    }

    /// `counters` were written at `now`.
    pub fn wrote(&mut self, counters: Counters, now: u64) {
        self.written = Some(counters);
        self.tried_at = Some(now);
    }

    /// A write at `now` failed: the next is tried no sooner than an
    /// interval later.
    pub fn failed(&mut self, now: u64) {
        if !self.mutants.has(Mutants::RETRIES_AT_ONCE) {
            self.tried_at = Some(now);
        }
    }
}

#[derive(Debug, Default)]
struct Posted {
    counters: Counters,
    store: StoreShown,
    /// What is posted is to be written without waiting for the
    /// interval ([`Board::write_again`]).
    at_once: bool,
    stop: bool,
    /// How often [`Board::publish`] has tried to write the file, done
    /// or failed.
    attempts: u64,
    /// Whether [`Board::publish`] waits for a change and for nothing
    /// else: only then does a post wake it. While it waits for the end
    /// of an interval, the write at that end takes whatever is posted
    /// by then, so a post has nothing to wake it for. Woken by every
    /// post, it took most of its time in the wakes alone: with a post
    /// every 20 µs, 95 ms a second against 9 ms for its writes.
    awaits_change: bool,
    /// How often [`Board::publish`] has woken from a wait.
    wakes: u64,
    /// A defect, for tests: every post wakes [`Board::publish`].
    every_post_wakes: bool,
}

/// Where the queue thread puts its counters for the thread that writes
/// the file.
#[derive(Debug, Default)]
pub struct Board {
    posted: Mutex<Posted>,
    changed: Condvar,
}

impl Board {
    /// An empty board.
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Posted> {
        // The lock is held for a copy only: a panic while it is held
        // leaves whole counters.
        self.posted
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Put the counters up. Does no I/O and waits for none: the lock is
    /// never held across a write of the file. Wakes the publishing
    /// thread only if they changed and it waits for a change: while it
    /// waits for the end of an interval, the write at that end takes
    /// whatever is posted by then.
    pub fn post(&self, counters: Counters) {
        let mut posted = self.lock();
        if posted.counters != counters {
            posted.counters = counters;
            if posted.awaits_change || posted.every_post_wakes {
                self.changed.notify_all();
            }
        }
    }

    /// Put the store's and the pump's state up, as [`Board::post`] does
    /// the counters.
    pub fn post_store(&self, shown: StoreShown) {
        let mut posted = self.lock();
        if posted.store != shown {
            posted.store = shown;
            if posted.awaits_change || posted.every_post_wakes {
                self.changed.notify_all();
            }
        }
    }

    /// How often [`Board::publish`] has woken from a wait, and how
    /// often it has tried to write: for tests of what its wakes cost.
    pub fn wakes_and_attempts(&self) -> (u64, u64) {
        let posted = self.lock();
        (posted.wakes, posted.attempts)
    }

    /// For tests: whether [`Board::publish`] waits for a change and for
    /// nothing else. Read under the board's lock, so a waiting thread
    /// has let go of it in its wait.
    #[cfg(test)]
    pub(crate) fn awaits_change(&self) -> bool {
        self.lock().awaits_change
    }

    /// Wait until [`Board::publish`] has tried to write the file at
    /// least once, done or failed, for at most `limit`; whether it
    /// has. The daemon says `ready` after this: until the new
    /// instance's first write the file is the one the instance before
    /// left, and `status` would show that instance's pid and counters
    /// as the serving daemon's. A write that failed counts: writing the
    /// status holds nothing up.
    pub fn first_write_tried(&self, limit: Duration) -> bool {
        let until = Instant::now() + limit;
        let mut posted = self.lock();
        while posted.attempts == 0 {
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            posted = self
                .changed
                .wait_timeout(posted, left)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
        true
    }

    /// Have [`Board::publish`] write the file once more, at once, with
    /// what is posted: for a value the writer reads elsewhere and that
    /// has changed. Nothing posted is replaced. The write is owed until
    /// one succeeds: if it fails it is tried again an interval later,
    /// as a failed write of changed counters is, and at the stop if it
    /// is still owed then.
    pub fn write_again(&self) {
        self.lock().at_once = true;
        self.changed.notify_all();
    }

    /// Tell [`Board::publish`] to write what is posted and return.
    pub fn stop(&self) {
        self.lock().stop = true;
        self.changed.notify_all();
    }

    /// Write the posted counters whenever `pace` says so, with `write`,
    /// until [`Board::stop`]; then write the last ones if they were not
    /// written, and return how many writes failed. `write` is given the
    /// counters and the number of failures so far.
    pub fn publish(
        &self,
        pace: Pace,
        mut write: impl FnMut(&Counters, u64) -> io::Result<()>,
    ) -> u64 {
        self.publish_with_store(pace, |counters, _, failures| write(counters, failures))
    }

    /// [`Board::publish`] for a `write` that is also given the store's
    /// state as posted ([`Board::post_store`]). A change of it is
    /// written as a change of the counters is.
    pub fn publish_with_store(
        &self,
        mut pace: Pace,
        mut write: impl FnMut(&Counters, &StoreShown, u64) -> io::Result<()>,
    ) -> u64 {
        let started = Instant::now();
        let now = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut failures = 0u64;
        let mut written_store = StoreShown::default();
        // A write that was asked for at once and has not succeeded
        // yet. Neither the counters nor the store's state say that it
        // is owed: both may equal what was written last.
        let mut owed = false;
        let mut posted = self.lock();
        posted.every_post_wakes = pace.mutants.has(Mutants::WOKEN_BY_EVERY_POST);
        loop {
            let mut due = pace.due(&posted.counters, now());
            if due == Due::Never && (posted.store != written_store || owed) {
                due = pace.due_changed(now());
            }
            let last = posted.stop;
            if posted.at_once || matches!(due, Due::Now) || (last && due != Due::Never) {
                let (counters, store) = (posted.counters, posted.store);
                owed |= posted.at_once && !pace.mutants.has(Mutants::OWED_FORGOTTEN);
                posted.at_once = false;
                drop(posted);
                let written = write(&counters, &store, failures);
                posted = self.lock();
                posted.attempts += 1;
                self.changed.notify_all();
                match written {
                    Ok(()) => {
                        owed = false;
                        written_store = store;
                        pace.wrote(counters, now());
                    }
                    Err(_) => {
                        failures += 1;
                        pace.failed(now());
                        if last {
                            return failures;
                        }
                    }
                }
                continue;
            }
            if last {
                return failures;
            }
            posted = match due {
                Due::At(at) => {
                    let wait = Duration::from_millis(at.saturating_sub(now()).max(1));
                    let (guard, _) = self
                        .changed
                        .wait_timeout(posted, wait)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard
                }
                _ => {
                    posted.awaits_change = true;
                    let mut guard = self
                        .changed
                        .wait(posted)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.awaits_change = false;
                    guard
                }
            };
            posted.wakes += 1;
        }
    }
}

/// What every message about a failed or refused volume says of a pool
/// that is imported on its device, and `status` with it. The daemon
/// does not act on a pool: it is below ZFS, the device is gone, and it
/// knows no pool. It says the state and the step; making the pool read
/// is the operator's, and later the agent's.
///
/// What it says was measured by the testing session on the daemon
/// (runs 43093721420, 43100887191, 43100211375) and read in OpenZFS
/// 2.4.1 (`zio_done`): after the device is gone the pool's writes fail
/// at once, and ZFS executes a failed allocating write again without
/// end instead of suspending, so the pool reads ONLINE with one
/// transaction group syncing for ever; with a log device, synchronous
/// writes were still acknowledged minutes later. The first read that
/// had to reach the device suspended the pool within a second or two
/// (a read of an uncached file, the start of a scrub). It does not
/// say "export the pool": what an export does in that state is not
/// measured. (That a pool is exported before its volume is detached
/// is said where `detach` is described: that is about before.)
pub const POOL_WARNING: &str = "a pool imported on this volume's device does not notice that \
     the device is gone while it only writes: it goes on reading ONLINE and, with a log \
     device, acknowledging synchronous writes that never reach the volume. A read that must \
     reach the device suspends it (for example `zpool scrub POOL`), and the pool cannot be \
     imported again before a reboot";

/// The volume's failure note, as `status` found it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Noted {
    /// There is none.
    Nothing,
    /// One is there and is not a whole record: it counts as a failure
    /// while a client held the device.
    Unreadable,
    /// One is there. It counts if it is of this boot.
    Whole(StoredFailure),
}

/// What `status` prints: keys and values, in order.
///
/// `pid` is there while the kernel shows a live server for this
/// volume's device: the process that wrote the file.
///
/// `cuts` is the count of cuts the serving daemon's store has made
/// (design §5.2) and `cut_group` the newest one's transaction group,
/// from a file of the eleventh version or later. `cut_opens`, from the
/// twelfth, says whether that cut opens as itself: `itself` if an import
/// of the volume at its instant, with no rewind, opens its pool state,
/// `rewind` if it opens only with a rewind to its group (after a rewind
/// import, until the pool's groups pass the abandoned ones). The count is the daemon's own:
/// it starts at 0 with each daemon. A pool made on a partition of the
/// volume, not on the whole device, makes no cuts: its uberblocks are
/// not where the store looks for them.
///
/// A failure note of this boot decides the state, and is printed after
/// everything else: its boot, whether a client held the device, why,
/// and how long ago; then what became of the volume's device
/// (`failure_device`: none under the recorded number, or which and in
/// what state, and what removes it) and [`POOL_WARNING`]
/// (`failure_pool`). All of it comes from the notes and the driver, so
/// it is the same with no daemon running: this is where an operator
/// reads a failure or a refused start after the daemon has exited. A
/// note of another boot is not shown.
///
/// `located` is the device under the volume's recorded number
/// ([`lifecycle::locate`](crate::lifecycle::locate)), `status` the
/// file's content, `boot` this boot and `uptime_ms` the time now. The
/// counters of a file from another boot are not shown. The counters are
/// marked as left by a daemon that is gone when the kernel shows no
/// live server for this volume's device.
pub fn report(
    located: Option<&Located>,
    status: Option<&Status>,
    noted: Noted,
    boot: &[u8; 16],
    uptime_ms: u64,
) -> Vec<(&'static str, String)> {
    let ours = located.filter(|l| l.whose == Whose::Ours);
    let failure = match noted {
        Noted::Whole(stored) if stored.boot == *boot => Some(Some(stored)),
        Noted::Unreadable => Some(None),
        _ => None,
    };
    let state = match (located, ours) {
        _ if failure.is_some_and(|f| f.is_none_or(|f| f.failure.held == Held::Yes)) => {
            "failed, held by a client: not served again in this boot".to_string()
        }
        _ if failure.is_some_and(|f| f.is_some_and(|f| f.failure.held == Held::Unknown)) => {
            "failed, not known whether a client holds it: the next serve finds out".to_string()
        }
        _ if failure.is_some() => "failed: needs --acknowledge-failure".to_string(),
        (None, _) => "detached".to_string(),
        (Some(l), None) if l.whose == Whose::Unsure => format!(
            "unknown: device {} has this volume's values and no note",
            l.number
        ),
        (Some(l), None) => format!(
            "detached: device {} under the recorded number is not this volume's",
            l.number
        ),
        (_, Some(l)) => match l.state {
            DeviceState::Live => "serving",
            DeviceState::Waiting => "waiting for a server",
            DeviceState::Dead => "dead: never started, or deleted and still held",
        }
        .to_string(),
    };
    let mut lines = vec![("state", state)];
    if let Some(l) = ours {
        lines.push(("device", format!("/dev/ublkb{}", l.number)));
    }
    let status = status.filter(|s| s.boot == *boot);
    let shape = ours
        .map(|l| (l.shape.queues, l.shape.depth))
        .or(status.map(|s| (s.queues, s.depth)));
    if let Some((queues, depth)) = shape {
        lines.push(("queues", queues.to_string()));
        lines.push(("depth", depth.to_string()));
    }
    if let Some(s) = status {
        let c = &s.counters;
        for (key, value) in [
            ("reads", c.reads),
            ("writes", c.writes),
            ("flushes", c.flushes),
            ("discards", c.discards),
            ("write_zeroes", c.write_zeroes),
            ("refused", c.refused),
            ("held", c.in_flight),
        ] {
            lines.push((key, value.to_string()));
        }
        // The commands the driver ended otherwise than OK: only from a
        // file that holds them, so that a file of an older version
        // shows no count it did not keep.
        if s.version >= 10 {
            lines.push(("commands_aborted", c.commands_aborted.to_string()));
            lines.push(("commands_failed", c.commands_failed.to_string()));
            if c.commands_failed > 0 {
                lines.push(("last_command_failure", c.last_command_failure.to_string()));
            }
        }
        for (key, value) in [
            ("first_write_copies", s.first_write_copies),
            ("status_write_failures", s.failures),
            ("age_ms", uptime_ms.saturating_sub(s.written_ms)),
            ("status_version", u64::from(s.version)),
            ("start_ms", s.start_ms),
        ] {
            lines.push((key, value.to_string()));
        }
        let shown = &s.shown;
        if let Some(said) = shown.zeroes.said() {
            lines.push(("zeroes_applied", said.to_string()));
        }
        // The count of cuts and the newest cut's group, from a file
        // that holds them; `cut_group` once there is a cut, and
        // `cut_opens` with it from a file that says it.
        if let Some(cuts) = shown.cuts {
            lines.push(("cuts", cuts.count.to_string()));
            if let Some(newest) = cuts.newest {
                lines.push(("cut_group", newest.group.to_string()));
                if s.version >= 12 {
                    let opens = if newest.itself { "itself" } else { "rewind" };
                    lines.push(("cut_opens", opens.to_string()));
                }
            }
        }
        for (key, value) in PUMP_KEYS.iter().zip(pump_fields(&shown.pump)) {
            lines.push((key, value.to_string()));
        }
        // The store's I/O threads' queue, where there are threads.
        if let Some(q) = shown.io_queue.filter(|q| q.threads > 0) {
            for (key, value) in [
                ("io_threads", q.threads),
                ("io_in_flight", q.in_flight),
                ("io_waiting", q.waiting),
                ("io_waited", q.waited),
                ("io_in_flight_max", q.in_flight_max),
                ("io_waiting_max", q.waiting_max),
            ] {
                lines.push((key, value.to_string()));
            }
        }
        let live = ours.is_some_and(|l| l.state == DeviceState::Live);
        if live {
            lines.push(("pid", s.pid.to_string()));
        }
        let counters = if live {
            format!("of the serving daemon, pid {}", s.pid)
        } else {
            format!("left by a daemon that is gone, pid {}", s.pid)
        };
        lines.push(("counters", counters));
    }
    match failure {
        None => {}
        Some(None) => lines.push(("failure", "the failure note cannot be read".to_string())),
        Some(Some(stored)) => {
            let boot: String = stored.boot.iter().map(|b| format!("{b:02x}")).collect();
            let held = match stored.failure.held {
                Held::Yes => "yes",
                Held::No => "no",
                Held::Unknown => "unknown",
            };
            lines.push(("failure", stored.failure.reason.to_string()));
            lines.push(("failure_held", held.to_string()));
            lines.push(("failure_boot", boot));
            lines.push((
                "failure_age_ms",
                uptime_ms.saturating_sub(stored.written_ms).to_string(),
            ));
        }
    }
    if failure.is_some() {
        let device = match located {
            None => "none under the volume's recorded number: deleted, or never there".to_string(),
            Some(l) if l.whose == Whose::Ours => format!(
                "/dev/ublkb{} is this volume's and {}",
                l.number,
                match l.state {
                    DeviceState::Live => "live: it has a server",
                    DeviceState::Waiting =>
                        "waits for a server it will not get: the next serve or a detach \
                         deletes it",
                    DeviceState::Dead =>
                        "deleted, and something still holds it: its number is free when \
                         that lets go",
                }
            ),
            // The words of a refused start, for the same device.
            Some(l) if l.whose == Whose::Unsure => crate::lifecycle::unnoted(l.number, l.state),
            Some(l) => format!(
                "device {} under the recorded number is not this volume's",
                l.number
            ),
        };
        lines.push(("failure_device", device));
        lines.push(("failure_pool", POOL_WARNING.to_string()));
    }
    lines
}

#[cfg(test)]
mod tests;

//! What `skyzfsd status` reads: a small file the serving daemon keeps
//! under the volume's directory in the run directory, and the report
//! made from it, the attachment record, the device's note and the
//! kernel's word on the device. `status` opens nothing else: not the
//! store, and no channel to the daemon.
//!
//! # The file
//!
//! [`STATUS`] under the volume's run directory
//! ([`records::volume_run_dir`](crate::records::volume_run_dir)). 536
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
//! This is the file's fifth version. The fourth (`SKYZSTA4`, 200
//! bytes) ended before offset 200, where the times begin; a file of
//! that version is still read, without them. The third (`SKYZSTA3`, 192
//! bytes) ended before offset 192; a file of that version is still
//! read, and says nothing of how write-zeroes are applied. The second
//! (`SKYZSTA2`, 144 bytes) ended before the log's fields, from offset
//! 144 on; a file of that version is still read, without them. The
//! first (`SKYZSTA1`, 128 bytes) is read as no status.
//!
//! | Offset | Bytes | Field |
//! |---|---|---|
//! | 0 | 8 | magic `SKYZSTA5` |
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
//! | 112 | 8 | `unstored`: completions waiting because the acknowledged position could not be stored |
//! | 120 | 8 | writes of this file that failed |
//! | 128 | 8 | `first_write_copies`: first writes of a tag that were copied from the transport's own buffer since this daemon started serving (at most the depth) |
//! | 136 | 8 | `recovered_unacknowledged`: operations this daemon's start found in the log beyond the acknowledged position (a lower bound: see `Store::recovered_unacknowledged`) |
//! | 144 | 8 | `log_live`: the bytes of the log from its start to its end, which the next start replays |
//! | 152 | 8 | `checkpoint_steps`: the steps the store's checkpoint rule has taken since this daemon started serving |
//! | 160 | 8 | 1 while such a step is under way, else 0; a step begins with the rule's own sync of the log when it needs one |
//! | 168 | 8 | `start_ms`: how long this daemon's start took until the device served |
//! | 176 | 8 | `replayed_bytes`: the bytes of log that start replayed |
//! | 184 | 8 | `checkpoint_syncs`: the syncs of the log the store's worker issued for its steps since this daemon started serving. They are nobody's request and are not among the flushes committed |
//! | 192 | 8 | `zeroes_applied`: how a checkpoint applies write-zeroes to the home image. 0: none applied since this daemon started serving; 1: their ranges are deallocated; 2: zeros are written, because the file system cannot deallocate; 3: zeros are written, because the file system's blocks are larger than the store's write unit, or it does not say. Any other value is read as "not said" |
//! | 200 | 200 | the requests' times, five fields of 8 bytes for each kind in the order read, write, flush, discard, write-zeroes: the requests of that kind the store completed; their summed wait, from the request's submission to the store until the store's worker began it; the longest such wait; the summed time of the store's call for them; the longest such call |
//! | 400 | 80 | the checkpoint steps' times, a sum and a longest for each part in the order: saving the neighbours of what a step writes; writing and deallocating in the home image; the home image's sync; the log's new start; the rule's own sync of the log before a step (`checkpoint_syncs` counts those) |
//! | 480 | 24 | the gate at the high mark: the turns of the worker in which a request waited and none was taken because the log was at the high mark; the summed time of the steps taken in those turns; the longest |
//! | 504 | 16 | the requests that began after a step that ended while they waited: how many, and their summed wait |
//! | 520 | 16 | the reads among them: how many, and their summed wait |
//!
//! All times are nanoseconds of the monotonic clock, since this daemon
//! started serving. Sums and counts only grow, so a reader takes the
//! difference of two looks for a window; a longest time is of the whole
//! time served and has no difference. What each number means exactly
//! is at [`skyzfs_local::store::Timing`].
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
//!   move, **except when a checkpoint step begins or ends**: that is
//!   written at once ([`Board::post_log`] with `at_once`), so that
//!   whoever watches the file sees a step while it is under way.
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
use crate::pump::Counters;
use crate::records::StoredFailure;
use skyzfs_local::store::{RequestTiming, Spent, Timing};

/// The file's name under the volume's run directory.
pub const STATUS: &str = "status";

const MAGIC: &[u8; 8] = b"SKYZSTA5";
const LEN: usize = 536;
/// The fourth version: without the times from offset 200 on.
const FOURTH_MAGIC: &[u8; 8] = b"SKYZSTA4";
const FOURTH_LEN: usize = 200;
/// The third version: without the field at offset 192.
const THIRD_MAGIC: &[u8; 8] = b"SKYZSTA3";
const THIRD_LEN: usize = 192;
/// The second version: without the fields from offset 144 on.
const SECOND_MAGIC: &[u8; 8] = b"SKYZSTA2";
const SECOND_LEN: usize = 144;
/// The first version: read as no status.
const OLD_MAGIC: &[u8; 8] = b"SKYZSTA1";
const OLD_LEN: usize = 128;

/// The keys of the times in `status`, in the file's order. All values
/// are nanoseconds but the counts. A key ending in `_max_ns` is the
/// longest since the daemon started serving; every other value only
/// grows, and a script takes the difference of two looks.
pub const TIME_KEYS: [&str; TIME_FIELDS] = [
    "time_read_count",
    "time_read_wait_ns",
    "time_read_wait_max_ns",
    "time_read_call_ns",
    "time_read_call_max_ns",
    "time_write_count",
    "time_write_wait_ns",
    "time_write_wait_max_ns",
    "time_write_call_ns",
    "time_write_call_max_ns",
    "time_flush_count",
    "time_flush_wait_ns",
    "time_flush_wait_max_ns",
    "time_flush_call_ns",
    "time_flush_call_max_ns",
    "time_discard_count",
    "time_discard_wait_ns",
    "time_discard_wait_max_ns",
    "time_discard_call_ns",
    "time_discard_call_max_ns",
    "time_zeroes_count",
    "time_zeroes_wait_ns",
    "time_zeroes_wait_max_ns",
    "time_zeroes_call_ns",
    "time_zeroes_call_max_ns",
    "time_step_save_ns",
    "time_step_save_max_ns",
    "time_step_apply_ns",
    "time_step_apply_max_ns",
    "time_step_home_sync_ns",
    "time_step_home_sync_max_ns",
    "time_step_log_start_ns",
    "time_step_log_start_max_ns",
    "time_step_own_sync_ns",
    "time_step_own_sync_max_ns",
    "time_gate_count",
    "time_gate_ns",
    "time_gate_max_ns",
    "time_behind_step_count",
    "time_behind_step_wait_ns",
    "time_reads_behind_step_count",
    "time_reads_behind_step_wait_ns",
];

/// What the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
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
    /// Operations this daemon's start found in the log beyond the
    /// acknowledged position: applied by the daemon before it and never
    /// answered. A lower bound.
    pub recovered_unacknowledged: u64,
    /// The log and the start, which the third version added. `None`
    /// in a file of the second version.
    pub log: Option<LogStatus>,
}

/// What the file says of the log and of the daemon's start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogStatus {
    /// The log as the store shows it.
    pub shown: LogShown,
    /// How long the start took until the device served, in
    /// milliseconds.
    pub start_ms: u64,
    /// The bytes of log the start replayed.
    pub replayed_bytes: u64,
}

/// The log as the queue thread posts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LogShown {
    /// The bytes from the log's start to its end.
    pub live: u64,
    /// The checkpoint steps taken since the daemon started serving.
    pub steps: u64,
    /// Whether a step is under way.
    pub stepping: bool,
    /// The syncs of the log the worker issued for its steps: its own,
    /// apart from the flushes clients asked for.
    pub own_syncs: u64,
    /// How a checkpoint applies write-zeroes to the home image.
    pub zeroes: ZeroesShown,
    /// Where the store's time went. `None` in a file of a version
    /// before the fifth.
    pub timing: Option<Timing>,
}

/// How a checkpoint applies write-zeroes to the home image, as the
/// status file says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZeroesShown {
    /// The file does not say: it is of a version before the fourth, or
    /// holds a value this version does not know.
    #[default]
    Unsaid,
    /// No checkpoint has applied one since the daemon started serving.
    Untried,
    /// Their ranges are deallocated: nothing is written for them.
    Deallocated,
    /// Zeros are written: the file system cannot deallocate.
    Written,
    /// Zeros are written: the file system's blocks are larger than the
    /// store's write unit, or their size could not be read. No store
    /// says this any more: one is not opened on such a file system
    /// (M1.3g). The value stays readable.
    WrittenForBlocks,
}

impl ZeroesShown {
    fn code(self) -> u64 {
        match self {
            Self::Untried => 0,
            Self::Deallocated => 1,
            Self::Written => 2,
            Self::WrittenForBlocks => 3,
            Self::Unsaid => u64::MAX,
        }
    }

    fn of(code: u64) -> Self {
        match code {
            0 => Self::Untried,
            1 => Self::Deallocated,
            2 => Self::Written,
            3 => Self::WrittenForBlocks,
            _ => Self::Unsaid,
        }
    }

    /// The report's words, or none where the file does not say.
    fn said(self) -> Option<&'static str> {
        match self {
            Self::Unsaid => None,
            Self::Untried => {
                Some("not yet: no checkpoint has applied one since this daemon started serving")
            }
            Self::Deallocated => Some("deallocated in the home image: nothing is written for them"),
            Self::Written => Some(
                "written as zeros: the file system of the volume's directory cannot deallocate, \
                 so a log of write-zeroes is worked off at the pace of that writing, and \
                 requests wait for it",
            ),
            Self::WrittenForBlocks => Some(
                "written as zeros: the blocks of the volume's file system are larger than the \
                 store's write unit, or their size could not be read, so a range is not \
                 deallocated there; a log of write-zeroes is worked off at the pace of that \
                 writing, and requests wait for it",
            ),
        }
    }
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
    let log = status.log.unwrap_or_default();
    let fields = [
        status.written_ms,
        c.reads,
        c.writes,
        c.flushes,
        c.discards,
        c.write_zeroes,
        c.refused,
        c.in_flight,
        c.unstored,
        status.failures,
        status.first_write_copies,
        status.recovered_unacknowledged,
        log.shown.live,
        log.shown.steps,
        u64::from(log.shown.stepping),
        log.start_ms,
        log.replayed_bytes,
        log.shown.own_syncs,
        log.shown.zeroes.code(),
    ];
    let times = time_fields(&log.shown.timing.unwrap_or_default());
    for (i, field) in fields.iter().chain(&times).enumerate() {
        bytes[48 + 8 * i..56 + 8 * i].copy_from_slice(&field.to_le_bytes());
    }
    bytes
}

/// The number of 8-byte fields the times take.
const TIME_FIELDS: usize = 42;

/// The times in the file's order: see the table at the top.
fn time_fields(timing: &Timing) -> [u64; TIME_FIELDS] {
    let mut fields = [0u64; TIME_FIELDS];
    let mut at = 0;
    let mut put = |value: u64| {
        fields[at] = value;
        at += 1;
    };
    for kind in &timing.requests {
        for value in [
            kind.wait.count,
            kind.wait.sum_ns,
            kind.wait.max_ns,
            kind.call.sum_ns,
            kind.call.max_ns,
        ] {
            put(value);
        }
    }
    for part in [
        timing.step_save,
        timing.step_apply,
        timing.step_home_sync,
        timing.step_start,
        timing.step_own_sync,
    ] {
        put(part.sum_ns);
        put(part.max_ns);
    }
    put(timing.gate.count);
    put(timing.gate.sum_ns);
    put(timing.gate.max_ns);
    put(timing.behind_step.count);
    put(timing.behind_step.sum_ns);
    put(timing.reads_behind_step.count);
    put(timing.reads_behind_step.sum_ns);
    fields
}

/// The times of a file of the fifth version. What the file does not
/// hold is filled in where another field says it (the count of a
/// request's calls is that of its waits) and left zero where none
/// does (how many batches a step's parts were measured over; the
/// longest wait behind a step).
fn timing_at(bytes: &[u8]) -> Timing {
    let mut at = 200;
    let mut take = || {
        let value = u64_at(bytes, at);
        at += 8;
        value
    };
    let mut timing = Timing::default();
    for kind in &mut timing.requests {
        let count = take();
        let wait = Spent {
            count,
            sum_ns: take(),
            max_ns: take(),
        };
        let call = Spent {
            count,
            sum_ns: take(),
            max_ns: take(),
        };
        *kind = RequestTiming { wait, call };
    }
    let mut part = || Spent {
        count: 0,
        sum_ns: take(),
        max_ns: take(),
    };
    timing.step_save = part();
    timing.step_apply = part();
    timing.step_home_sync = part();
    timing.step_start = part();
    timing.step_own_sync = part();
    timing.gate = Spent {
        count: take(),
        sum_ns: take(),
        max_ns: take(),
    };
    timing.behind_step = Spent {
        count: take(),
        sum_ns: take(),
        max_ns: 0,
    };
    timing.reads_behind_step = Spent {
        count: take(),
        sum_ns: take(),
        max_ns: 0,
    };
    timing
}

fn decode(bytes: &[u8]) -> Option<Status> {
    let fifth = bytes.len() == LEN && &bytes[..8] == MAGIC;
    let fourth = fifth || (bytes.len() == FOURTH_LEN && &bytes[..8] == FOURTH_MAGIC);
    let third = bytes.len() == THIRD_LEN && &bytes[..8] == THIRD_MAGIC;
    let second = bytes.len() == SECOND_LEN && &bytes[..8] == SECOND_MAGIC;
    if !fourth && !third && !second {
        return None;
    }
    let log = (fourth || third).then(|| LogStatus {
        shown: LogShown {
            live: u64_at(bytes, 144),
            steps: u64_at(bytes, 152),
            stepping: u64_at(bytes, 160) != 0,
            own_syncs: u64_at(bytes, 184),
            zeroes: if fourth {
                ZeroesShown::of(u64_at(bytes, 192))
            } else {
                ZeroesShown::Unsaid
            },
            timing: fifth.then(|| timing_at(bytes)),
        },
        start_ms: u64_at(bytes, 168),
        replayed_bytes: u64_at(bytes, 176),
    });
    let mut boot = [0u8; 16];
    boot.copy_from_slice(&bytes[16..32]);
    let mut identity = [0u8; 16];
    identity.copy_from_slice(&bytes[32..48]);
    Some(Status {
        pid: u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]),
        queues: u16::from_le_bytes([bytes[12], bytes[13]]),
        depth: u16::from_le_bytes([bytes[14], bytes[15]]),
        boot,
        identity,
        written_ms: u64_at(bytes, 48),
        counters: Counters {
            reads: u64_at(bytes, 56),
            writes: u64_at(bytes, 64),
            flushes: u64_at(bytes, 72),
            discards: u64_at(bytes, 80),
            write_zeroes: u64_at(bytes, 88),
            refused: u64_at(bytes, 96),
            in_flight: u64_at(bytes, 104),
            unstored: u64_at(bytes, 112),
        },
        failures: u64_at(bytes, 120),
        first_write_copies: u64_at(bytes, 128),
        recovered_unacknowledged: u64_at(bytes, 136),
        log,
    })
}

/// Read the file at `path`. `Ok(None)` if there is none, or one of the
/// first version. Any other file that is not a whole record is an
/// error.
pub fn read(path: &Path) -> io::Result<Option<Status>> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if bytes.len() == OLD_LEN && &bytes[..8] == OLD_MAGIC {
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
    log: LogShown,
    /// The log as posted is to be written without waiting for the
    /// interval.
    at_once: bool,
    stop: bool,
    /// How often [`Board::publish`] has tried to write the file, done
    /// or failed.
    attempts: u64,
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
    /// thread only if they changed.
    pub fn post(&self, counters: Counters) {
        let mut posted = self.lock();
        if posted.counters != counters {
            posted.counters = counters;
            self.changed.notify_all();
        }
    }

    /// Put the log's state up, as [`Board::post`] does the counters.
    /// With `at_once` it is written without waiting for the interval:
    /// for the begin and the end of a checkpoint step.
    pub fn post_log(&self, log: LogShown, at_once: bool) {
        let mut posted = self.lock();
        if posted.log != log {
            posted.log = log;
            posted.at_once |= at_once;
            self.changed.notify_all();
        }
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
        self.publish_with_log(pace, |counters, _, failures| write(counters, failures))
    }

    /// [`Board::publish`] for a `write` that is also given the log's
    /// state as posted ([`Board::post_log`]). A change of the log is
    /// written as a change of the counters is, and at once if it was
    /// posted so.
    pub fn publish_with_log(
        &self,
        mut pace: Pace,
        mut write: impl FnMut(&Counters, &LogShown, u64) -> io::Result<()>,
    ) -> u64 {
        let started = Instant::now();
        let now = || u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        let mut failures = 0u64;
        let mut written_log = LogShown::default();
        // A write that was asked for at once and has not succeeded
        // yet. Neither the counters nor the log say that it is owed:
        // both may equal what was written last.
        let mut owed = false;
        let mut posted = self.lock();
        loop {
            let mut due = pace.due(&posted.counters, now());
            if due == Due::Never && (posted.log != written_log || owed) {
                due = pace.due_changed(now());
            }
            let last = posted.stop;
            if posted.at_once || matches!(due, Due::Now) || (last && due != Due::Never) {
                let (counters, log) = (posted.counters, posted.log);
                owed |= posted.at_once && !pace.mutants.has(Mutants::OWED_FORGOTTEN);
                posted.at_once = false;
                drop(posted);
                let written = write(&counters, &log, failures);
                posted = self.lock();
                posted.attempts += 1;
                self.changed.notify_all();
                match written {
                    Ok(()) => {
                        owed = false;
                        written_log = log;
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
                _ => self
                    .changed
                    .wait(posted)
                    .unwrap_or_else(|poisoned| poisoned.into_inner()),
            };
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
            ("unstored", c.unstored),
            ("first_write_copies", s.first_write_copies),
            ("recovered_unacknowledged", s.recovered_unacknowledged),
            ("status_write_failures", s.failures),
            ("age_ms", uptime_ms.saturating_sub(s.written_ms)),
        ] {
            lines.push((key, value.to_string()));
        }
        if let Some(log) = &s.log {
            for (key, value) in [
                ("log_live", log.shown.live.to_string()),
                ("checkpoint_steps", log.shown.steps.to_string()),
                ("checkpoint_syncs", log.shown.own_syncs.to_string()),
                (
                    "checkpoint_step",
                    if log.shown.stepping {
                        "under way"
                    } else {
                        "none"
                    }
                    .to_string(),
                ),
                ("start_ms", log.start_ms.to_string()),
                ("replayed_bytes", log.replayed_bytes.to_string()),
            ] {
                lines.push((key, value));
            }
            if let Some(said) = log.shown.zeroes.said() {
                lines.push(("zeroes_applied", said.to_string()));
            }
            if let Some(timing) = &log.shown.timing {
                for (key, value) in TIME_KEYS.iter().zip(time_fields(timing)) {
                    lines.push((key, value.to_string()));
                }
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

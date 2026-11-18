//! What `skyzfsd status` reads: a small file the serving daemon keeps
//! under the volume's directory in the run directory, and the report
//! made from it, the attachment record, the device's note and the
//! kernel's word on the device. `status` opens nothing else: not the
//! store, and no channel to the daemon.
//!
//! # The file
//!
//! [`STATUS`] under the volume's run directory
//! ([`records::volume_run_dir`](crate::records::volume_run_dir)). 1008
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
//! This is the file's sixteenth version (M2.4d). It adds, after the
//! fifteenth's fields, whether the intent map's header is synced at each
//! release (`serve --intent-header-sync`) and the syncs begun for a
//! header with no write waiting, with their time. Nothing is dropped or
//! moved. The fifteenth (`SKYZST15`, 976 bytes) is still read, without
//! them.
//!
//! The fifteenth version (M2.4c) added the uploader
//! after the fourteenth's fields: what it is doing, the volume's lease,
//! the driver's figures, its last check and the rule of the last cut
//! uploaded, and three short texts (why it waits or has no lease, the
//! lease's holder, the last error on object storage). A daemon that does
//! not upload writes them as zeros (`upload_state` off). After the texts
//! it adds one figure of the held cuts, the cuts replaced before their
//! upload (`cuts_replaced`), which a reader of an older file does not
//! get. Nothing is dropped or moved. The fourteenth (`SKYZST14`, 424 bytes) is still
//! read, without the uploader.
//!
//! The fourteenth version (M2.3b) added the intent
//! map after the thirteenth's fields: what the start did with the map,
//! the size of a region, the regions, those named now and those the
//! start's map named, the map's syncs on the write path with their
//! time, and the requests that waited for one. Nothing is dropped or
//! moved. The thirteenth (`SKYZST13`, 352
//! bytes) is still read, without the intent map or the uploader.
//!
//! The thirteenth version (M2.2b) added the held cuts after the
//! twelfth's fields: whether the store holds cuts and what its start
//! did with their state, how many are held, the bytes their copies
//! take, the bound, and the cuts abandoned at it. The twelfth
//! (`SKYZST12`, 312 bytes) is still read, without the held cuts.
//!
//! The twelfth version (M2.1b) added whether the
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
//! | 0 | 8 | magic `SKYZST15` |
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
//! | 288 | 8 | `cuts`: the cuts the store has counted since this daemon started serving (design §5.2). Not kept across a start (each daemon counts from 0), unless the store holds cuts and its start took their state up: then it counts on from the state's count |
//! | 296 | 8 | `cut_group`: the newest cut's transaction group; 0 before the first cut |
//! | 304 | 8 | `cut_opens`: whether the newest cut opens as itself (design §5.2): 1 if an import of the volume at its instant, with no rewind, opens its uberblock's pool state; 2 if it opens only with a rewind to its group; 0 before the first cut |
//! | 312 | 8 | `cut_state`: whether the store holds cuts, and what its start did with the state it found (`serve --hold-cuts`, M2.2b): 0 it holds none; 1 it took the state up; 2 it began a new one, which counts the whole volume as changed. Any other value is read as "not said" |
//! | 320 | 8 | `cuts_held`: the cuts held now (at most three: the one being uploaded, the one that waits, the one whose flush is under way) |
//! | 328 | 8 | `cut_copied_bytes`: the bytes the copies of held cuts take now |
//! | 336 | 8 | `cut_bound_bytes`: the bound on them |
//! | 344 | 8 | `cuts_abandoned`: the cuts abandoned at the bound or let go at a copy that could not be made, since the state began (kept across a start that takes the state up) |
//! | 352 | 8 | `intent_state`: what the start did with the intent map (M2.3): 0 the store keeps none (it holds no cuts); 1 it took the map up, so that only the regions it named count as changed where the cut state was begun anew; 2 it began a new map, which names every region. Any other value is read as "not said" |
//! | 360 | 8 | `intent_region_bytes`: the size of a region of the map |
//! | 368 | 8 | `intent_regions`: the volume's regions |
//! | 376 | 8 | `intent_named`: the regions the map names now |
//! | 384 | 8 | `intent_named_at_start`: the regions the map named when the start took it up (every region where it began one) |
//! | 392 | 24 | the map's syncs on the write path since this daemon started serving: how many, their summed time from the first page's write to the sync's end, the longest. A sync runs on the map's file's helper, not on the queue's thread, and covers every bit set since the one before it began |
//! | 416 | 8 | `intent_waits`: the writes and write-zeroes that waited for a sync of the map since this daemon started serving, those into a region whose bit was not durable. Only they and the requests behind them (one that overlaps them, a flush) waited |
//! | 424 | 8 | `upload_state`: what the uploader is doing (M2.4c). 0 off: this daemon does not upload; 1 no lease yet; 2 the lease is lost, for the rest of this instance; 3 idle: no cut to upload; 4 checking a cut against head's row; 5 reading one; 6 syncing the copies it was read from; 7 committing it; 8 waiting after an error or a commit given up; 9 stopped: the store kept a committed cut. Any other value is read as "not said" |
//! | 432 | 8 | `upload_cut`: the cut under way, while checking, reading, syncing or committing; 0 otherwise |
//! | 440 | 8 | `upload_wait_ms`: while waiting (state 8), the wait as it began |
//! | 448 | 8 | `lease`: 0 none held; 1 held; 2 held, and the last renewal failed on object storage (the uploads go on under its epoch); 3 lost. Any other value is read as "not said" |
//! | 456 | 8 | `lease_epoch`: the epoch held, or, when lost, the epoch of the holder that has it (0 where not known) |
//! | 464 | 8 | `lease_expires_unix_ms`: while held, when the lease expires by the refs object's clock, in milliseconds since the Unix epoch |
//! | 472 | 96 | the uploader's figures since this daemon started serving: cuts uploaded, among them those of the whole volume, their blocks, cuts found committed already, cuts committed that had gone before their release, among them those the intent map names now, cuts committed that the store kept, mismatches, the cuts they abandoned, cuts lost before their commit, commits given up, steps that failed on object storage |
//! | 568 | 8 | `upload_checked`: what the last check found: 0 no check yet; 1 matched; a mismatch: 2 no row, 3 the row names another cut, 4 head's commit is not the row's, 5 the row cannot be read. Any other value is read as "not said" |
//! | 576 | 8 | `upload_rule`: how the last cut uploaded went up: 0 none yet; 1 a delta onto the cut the row named; 2 the whole volume. Any other value is read as "not said" |
//! | 584 | 128 | `upload_why`: UTF-8, padded with zeros: why there is no lease, or the error or commit given up the uploader waits after |
//! | 712 | 128 | `lease_holder`: the holder the lease row names: this daemon's while held or unsure (as last held), another's when lost. A holder's name means one daemon (`--holder`, or the host's name and its machine id): a copy of a volume's files, a cloned VM image among them, must not be served under the same name |
//! | 840 | 128 | `upload_last_error`: the last step that failed on object storage |
//! | 976 | 8 | `intent_header_sync`: whether the intent map's header is synced at each release of a committed cut and where a committed cut is named (`serve --intent-header-sync`, M2.4d): 1 on; any other value, off. Off, the header is written without a sync, and a power cut that loses it costs one whole upload. On, a sync of the map's file begins on its helper once the header is written, and nothing waits for it |
//! | 984 | 24 | the syncs of the map begun for a header with no write waiting, since this daemon started serving: how many, their summed time from begin to end, the longest. None where `intent_header_sync` is off. A header written while a sync for the writes that wait is under way is synced by the next, which is counted with the write path's (offset 392) when writes wait for it too |
//! | 968 | 8 | `cuts_replaced`: the cuts replaced by a newer cut before they were uploaded, since this daemon started serving (not kept across a start): the cut that waits when a newer cut's flush completes, and a cut whose flush is still under way at the next cut's instant; the newer cut takes the map either way. Over a window between two looks at one daemon whose flushes did not fail (a cut whose flush failed, which fails the volume, is in no count), the cuts the store counted balance: `cuts` = (`uploaded` + `upload_found_committed` − `upload_committed_gone` − `upload_committed_kept`) + `cuts_replaced` + `cuts_abandoned` + `cuts_held`, the first group being the cuts committed and let go. The counts come from the store and the uploader at slightly different times: compare them while the uploader is idle |
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

const MAGIC: &[u8; 8] = b"SKYZST16";
/// The version this crate writes.
pub const VERSION: u32 = 16;
const LEN: usize = 1008;

/// The fifteenth version, still read: up to the header's sync, at the
/// same places.
const FIFTEENTH_MAGIC: &[u8; 8] = b"SKYZST15";
const FIFTEENTH_LEN: usize = 976;

/// The fourteenth version, still read: up to the uploader, at the same
/// places.
const FOURTEENTH_MAGIC: &[u8; 8] = b"SKYZST14";
const FOURTEENTH_LEN: usize = 424;

/// The thirteenth version: the fourteenth without its intent map
/// (offset 352), which is read as absent.
const THIRTEENTH_MAGIC: &[u8; 8] = b"SKYZST13";
const THIRTEENTH_LEN: usize = 352;

/// The twelfth version: the thirteenth without its held cuts (offset
/// 312), which are read as absent.
const TWELFTH_MAGIC: &[u8; 8] = b"SKYZST12";
const TWELFTH_LEN: usize = 312;

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
    /// The held cuts. `None` in a file of a version before the
    /// thirteenth.
    pub held: Option<HeldShown>,
    /// The intent map. `None` in a file of a version before the
    /// fourteenth.
    pub intent: Option<IntentShown>,
    /// The uploader. `None` in a file of a version before the
    /// fifteenth. Posted by the uploader's thread
    /// ([`Board::post_upload`]), not with the rest.
    pub upload: Option<UploadShown>,
}

/// The longest of the status file's texts, in bytes.
pub const TEXT: usize = 128;

/// A short text in the status file: UTF-8, padded with zeros.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text([u8; TEXT]);

impl Default for Text {
    fn default() -> Self {
        Self([0; TEXT])
    }
}

impl Text {
    /// `text`, cut at a character's boundary to [`TEXT`] bytes, with each
    /// control character (a line's end among them) made a space: `status`
    /// prints one line per key.
    pub fn new(text: &str) -> Self {
        let mut bytes = [0; TEXT];
        let mut at = 0;
        for c in text.chars() {
            let c = if c.is_control() { ' ' } else { c };
            let len = c.len_utf8();
            if at + len > TEXT {
                break;
            }
            c.encode_utf8(&mut bytes[at..at + len]);
            at += len;
        }
        Self(bytes)
    }

    /// The text, up to its first zero; a byte that is not UTF-8 is read
    /// as the replacement character.
    pub fn said(&self) -> String {
        let end = self.0.iter().position(|b| *b == 0).unwrap_or(TEXT);
        String::from_utf8_lossy(&self.0[..end]).into_owned()
    }

    fn of(bytes: &[u8]) -> Self {
        let mut text = [0; TEXT];
        text.copy_from_slice(&bytes[..TEXT]);
        Self(text)
    }
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.said())
    }
}

/// An enum the status file holds as a code: what each value is and what
/// `status` prints of it, and a value of the file this version does not
/// know (`Unsaid`).
macro_rules! coded {
    (
        $(#[$meta:meta])*
        $name:ident {
            $($(#[$vmeta:meta])* $variant:ident = $code:literal, $word:literal;)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
        pub enum $name {
            $($(#[$vmeta])* $variant,)*
            /// The file holds a value this version does not know.
            Unsaid,
        }

        impl $name {
            fn code(self) -> u64 {
                match self {
                    $(Self::$variant => $code,)*
                    Self::Unsaid => u64::MAX,
                }
            }

            fn of(code: u64) -> Self {
                match code {
                    $($code => Self::$variant,)*
                    _ => Self::Unsaid,
                }
            }

            /// What `status` prints of it.
            pub fn said(self) -> &'static str {
                match self {
                    $(Self::$variant => $word,)*
                    Self::Unsaid => "unsaid",
                }
            }
        }
    };
}

coded! {
    /// What the uploader is doing (M2.4c).
    UploadStateShown {
        /// This daemon does not upload.
        #[default]
        Off = 0, "off";
        /// No lease is held yet: nothing is uploaded.
        NoLease = 1, "no_lease";
        /// The lease is lost: nothing more is uploaded by this instance.
        LeaseLost = 2, "lease_lost";
        /// No cut to upload.
        Idle = 3, "idle";
        /// Checking a cut against head's row.
        Checking = 4, "checking";
        /// Reading a cut.
        Reading = 5, "reading";
        /// Syncing the copies a cut was read from.
        Syncing = 6, "syncing";
        /// Committing a cut.
        Committing = 7, "committing";
        /// Waiting after a step that failed or a commit given up.
        Backoff = 8, "backoff";
        /// The store kept a committed cut: nothing more is uploaded by
        /// this instance.
        Stopped = 9, "stopped";
    }
}

coded! {
    /// The volume's lease, as the uploader holds it.
    LeaseShown {
        /// None held.
        #[default]
        None = 0, "none";
        /// Held.
        Held = 1, "held";
        /// Held, and the last renewal failed on object storage: the
        /// uploads go on under its epoch.
        Unsure = 2, "unsure";
        /// Lost, for the rest of this instance.
        Lost = 3, "lost";
    }
}

coded! {
    /// What the uploader's last check found.
    CheckedShown {
        /// No check yet.
        #[default]
        Unchecked = 0, "unchecked";
        /// The row and head held what the cut was made against.
        Matched = 1, "matched";
        /// A cut of part of the volume, and no row.
        NoRow = 2, "no_row";
        /// The row names another cut than the intent map's.
        NotTheBase = 3, "not_the_base";
        /// Head's commit is not the row's.
        HeadMoved = 4, "head_moved";
        /// The row cannot be read.
        Unreadable = 5, "unreadable";
    }
}

coded! {
    /// How the last cut uploaded went up.
    RuleShown {
        /// None yet.
        #[default]
        None = 0, "none";
        /// A delta onto the cut the row named.
        Delta = 1, "delta";
        /// The whole volume.
        Whole = 2, "whole";
    }
}

/// The uploader's figures since this daemon started serving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UploadFigures {
    /// Cuts committed.
    pub uploaded: u64,
    /// Among them, those that held the whole volume.
    pub uploaded_whole: u64,
    /// Their blocks.
    pub blocks: u64,
    /// Cuts the check found committed already.
    pub found_committed: u64,
    /// Cuts committed that had gone from the store before their release.
    pub not_held: u64,
    /// Among them, those the intent map names now.
    pub named: u64,
    /// Cuts committed that the store kept.
    pub kept: u64,
    /// Mismatches.
    pub mismatches: u64,
    /// The cuts they abandoned.
    pub abandoned_at_mismatch: u64,
    /// Cuts lost before their commit.
    pub lost: u64,
    /// Commits given up.
    pub gave_up: u64,
    /// Steps that failed on object storage.
    pub errors: u64,
}

impl UploadFigures {
    fn fields(&self) -> [u64; 12] {
        [
            self.uploaded,
            self.uploaded_whole,
            self.blocks,
            self.found_committed,
            self.not_held,
            self.named,
            self.kept,
            self.mismatches,
            self.abandoned_at_mismatch,
            self.lost,
            self.gave_up,
            self.errors,
        ]
    }

    fn of(f: [u64; 12]) -> Self {
        Self {
            uploaded: f[0],
            uploaded_whole: f[1],
            blocks: f[2],
            found_committed: f[3],
            not_held: f[4],
            named: f[5],
            kept: f[6],
            mismatches: f[7],
            abandoned_at_mismatch: f[8],
            lost: f[9],
            gave_up: f[10],
            errors: f[11],
        }
    }
}

/// The uploader, as the status file says it (M2.4c).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UploadShown {
    /// What it is doing.
    pub state: UploadStateShown,
    /// The cut under way; 0 where none is.
    pub cut: u64,
    /// While waiting, the wait as it began.
    pub wait_ms: u64,
    /// The lease.
    pub lease: LeaseShown,
    /// The epoch held, or the one of the holder that has the lease when
    /// it is lost (0 where not known).
    pub epoch: u64,
    /// While held, when the lease expires by the refs object's clock,
    /// in milliseconds since the Unix epoch.
    pub expires_unix_ms: u64,
    /// The figures.
    pub figures: UploadFigures,
    /// What the last check found.
    pub checked: CheckedShown,
    /// How the last cut uploaded went up.
    pub rule: RuleShown,
    /// Why there is no lease, or what the uploader waits after.
    pub why: Text,
    /// The holder the lease row names.
    pub holder: Text,
    /// The last step that failed on object storage.
    pub last_error: Text,
}

impl UploadShown {
    /// The state in words, as `status` prints it under `upload`: "no
    /// lease: why", "lease lost to H at epoch E", "why, retry in N s".
    pub fn said(&self) -> String {
        let cut = |doing: &str| {
            if self.cut == 0 {
                doing.to_string()
            } else {
                format!("{doing} cut {}", self.cut)
            }
        };
        match self.state {
            UploadStateShown::Off => "off: this daemon does not upload".to_string(),
            UploadStateShown::NoLease => format!("no lease: {}", self.why.said()),
            UploadStateShown::LeaseLost => {
                let holder = self.holder.said();
                if holder.is_empty() {
                    format!("lease lost at epoch {}", self.epoch)
                } else {
                    format!("lease lost to {holder} at epoch {}", self.epoch)
                }
            }
            UploadStateShown::Idle => "idle".to_string(),
            UploadStateShown::Checking => cut("checking"),
            UploadStateShown::Reading => cut("reading"),
            UploadStateShown::Syncing => cut("syncing the copies of"),
            UploadStateShown::Committing => cut("committing"),
            UploadStateShown::Backoff => format!(
                "{}, retry in {} s",
                self.why.said(),
                self.wait_ms.div_ceil(1000)
            ),
            UploadStateShown::Stopped => {
                "stopped: the store kept a committed cut, and nothing more is uploaded by this \
                 instance"
                    .to_string()
            }
            UploadStateShown::Unsaid => "unsaid".to_string(),
        }
    }
}

/// The intent map, as the status file says it (`serve --hold-cuts`,
/// M2.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct IntentShown {
    /// What the start did with the map: [`CutStateShown::Off`] where
    /// the store keeps none, [`CutStateShown::TakenUp`] where it took
    /// the map up, [`CutStateShown::Begun`] where it began a new one.
    pub state: CutStateShown,
    /// The size of a region.
    pub region_bytes: u64,
    /// The volume's regions.
    pub regions: u64,
    /// The regions named now.
    pub named: u64,
    /// The regions named when the start took the map up, or every
    /// region where it began one.
    pub named_at_start: u64,
    /// The map's syncs on the write path, with their time.
    pub syncs: Spent,
    /// The writes and write-zeroes that waited for a sync of the map.
    pub waits: u64,
    /// The header's sync at each release (M2.4d). `None` in a file of a
    /// version before the sixteenth.
    pub header: Option<HeaderSyncShown>,
}

/// Whether the intent map's header is synced at each release, and the
/// syncs begun for it (`serve --intent-header-sync`, M2.4d).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeaderSyncShown {
    /// Whether it is synced.
    pub on: bool,
    /// The syncs of the map begun for a header with no write waiting,
    /// with their time from begin to end.
    pub syncs: Spent,
}

/// The held cuts, as the status file says them (`serve --hold-cuts`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HeldShown {
    /// Whether the store holds cuts, and what its start did.
    pub state: CutStateShown,
    /// The cuts held now.
    pub held: u64,
    /// The bytes their copies take now.
    pub copied: u64,
    /// The bound on the copies.
    pub bound: u64,
    /// The cuts abandoned at the bound or let go at a copy that could
    /// not be made.
    pub abandoned: u64,
    /// The cuts replaced before they were uploaded, since this daemon
    /// started serving. `None` in a file of a version before the
    /// fifteenth.
    pub replaced: Option<u64>,
}

/// Whether the store holds cuts, and what its start did with the state
/// it found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CutStateShown {
    /// The store holds no cut.
    #[default]
    Off,
    /// The start took the state up: the cuts it held are held still.
    TakenUp,
    /// The start began a new state, which counts the whole volume as
    /// changed.
    Begun,
    /// The file holds a value this version does not know.
    Unsaid,
}

impl CutStateShown {
    fn code(self) -> u64 {
        match self {
            Self::Off => 0,
            Self::TakenUp => 1,
            Self::Begun => 2,
            Self::Unsaid => u64::MAX,
        }
    }

    fn of(code: u64) -> Self {
        match code {
            0 => Self::Off,
            1 => Self::TakenUp,
            2 => Self::Begun,
            _ => Self::Unsaid,
        }
    }

    /// What `status` prints of it.
    pub fn said(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::TakenUp => "taken_up",
            Self::Begun => "begun",
            Self::Unsaid => "unsaid",
        }
    }
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
    let held = shown.held.unwrap_or_default();
    let intent = shown.intent.unwrap_or_default();
    let upload = shown.upload.unwrap_or_default();
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
        held.state.code(),
        held.held,
        held.copied,
        held.bound,
        held.abandoned,
        intent.state.code(),
        intent.region_bytes,
        intent.regions,
        intent.named,
        intent.named_at_start,
        intent.syncs.count,
        intent.syncs.sum_ns,
        intent.syncs.max_ns,
        intent.waits,
        upload.state.code(),
        upload.cut,
        upload.wait_ms,
        upload.lease.code(),
        upload.epoch,
        upload.expires_unix_ms,
    ];
    let figures = upload.figures.fields();
    let last = [upload.checked.code(), upload.rule.code()];
    let pump = pump_fields(&shown.pump);
    for (i, field) in fields
        .iter()
        .chain(&pump)
        .chain(&after)
        .chain(&figures)
        .chain(&last)
        .enumerate()
    {
        bytes[48 + 8 * i..56 + 8 * i].copy_from_slice(&field.to_le_bytes());
    }
    for (at, text) in [
        (584, upload.why),
        (712, upload.holder),
        (840, upload.last_error),
    ] {
        bytes[at..at + TEXT].copy_from_slice(&text.0);
    }
    bytes[968..976].copy_from_slice(&held.replaced.unwrap_or(0).to_le_bytes());
    let header = intent.header.unwrap_or_default();
    for (i, field) in [
        u64::from(header.on),
        header.syncs.count,
        header.syncs.sum_ns,
        header.syncs.max_ns,
    ]
    .iter()
    .enumerate()
    {
        bytes[976 + 8 * i..984 + 8 * i].copy_from_slice(&field.to_le_bytes());
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
        16
    } else if is(FIFTEENTH_LEN, FIFTEENTH_MAGIC) {
        15
    } else if is(FOURTEENTH_LEN, FOURTEENTH_MAGIC) {
        14
    } else if is(THIRTEENTH_LEN, THIRTEENTH_MAGIC) {
        13
    } else if is(TWELFTH_LEN, TWELFTH_MAGIC) {
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
        let held = (version >= 13).then(|| HeldShown {
            state: CutStateShown::of(field(312)),
            held: field(320),
            copied: field(328),
            bound: field(336),
            abandoned: field(344),
            replaced: (version >= 15).then(|| field(968)),
        });
        let intent = (version >= 14).then(|| IntentShown {
            state: CutStateShown::of(field(352)),
            region_bytes: field(360),
            regions: field(368),
            named: field(376),
            named_at_start: field(384),
            syncs: Spent {
                count: field(392),
                sum_ns: field(400),
                max_ns: field(408),
            },
            waits: field(416),
            header: (version >= 16).then(|| HeaderSyncShown {
                on: field(976) == 1,
                syncs: Spent {
                    count: field(984),
                    sum_ns: field(992),
                    max_ns: field(1000),
                },
            }),
        });
        let upload = (version >= 15).then(|| UploadShown {
            state: UploadStateShown::of(field(424)),
            cut: field(432),
            wait_ms: field(440),
            lease: LeaseShown::of(field(448)),
            epoch: field(456),
            expires_unix_ms: field(464),
            figures: UploadFigures::of(std::array::from_fn(|i| field(472 + 8 * i))),
            checked: CheckedShown::of(field(568)),
            rule: RuleShown::of(field(576)),
            why: Text::of(&bytes[584..]),
            holder: Text::of(&bytes[712..]),
            last_error: Text::of(&bytes[840..]),
        });
        let shown = StoreShown {
            zeroes: ZeroesShown::of(field(136)),
            pump: pump_of(&pump),
            io_queue: Some(io_queue_at(bytes, after(216))),
            cuts: Some(cuts),
            held,
            intent,
            upload,
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
            held: None,
            intent: None,
            upload: None,
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
    /// the counters. The uploader's part is not the queue thread's: what
    /// [`Board::post_upload`] put up is kept, whatever `shown` holds.
    pub fn post_store(&self, mut shown: StoreShown) {
        let mut posted = self.lock();
        shown.upload = posted.store.upload;
        if posted.store != shown {
            posted.store = shown;
            if posted.awaits_change || posted.every_post_wakes {
                self.changed.notify_all();
            }
        }
    }

    /// Put the uploader's state up, from its own thread, as
    /// [`Board::post_store`] does the store's: a change of it is written
    /// as a change of the counters is.
    pub fn post_upload(&self, upload: UploadShown) {
        let mut posted = self.lock();
        if posted.store.upload != Some(upload) {
            posted.store.upload = Some(upload);
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
/// import, until the pool's groups pass the abandoned ones). The count
/// is the daemon's own: it starts at 0 with each daemon, unless the
/// store holds cuts and took their state up, when it counts on from the
/// state's. A pool made on a partition of the
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
        // The held cuts, from a file that has them, after the I/O
        // threads: every key, so that a reader finds them whether or not
        // the store holds cuts.
        if let Some(held) = shown.held {
            lines.push(("cut_state", held.state.said().to_string()));
            for (key, value) in [
                ("cuts_held", held.held),
                ("cut_copied_bytes", held.copied),
                ("cut_bound_bytes", held.bound),
                ("cuts_abandoned", held.abandoned),
            ] {
                lines.push((key, value.to_string()));
            }
            if let Some(replaced) = held.replaced {
                lines.push(("cuts_replaced", replaced.to_string()));
            }
        }
        // The intent map, from a file that has it, after the held cuts:
        // every key, as for them.
        if let Some(intent) = shown.intent {
            lines.push(("intent_state", intent.state.said().to_string()));
            for (key, value) in [
                ("intent_region_bytes", intent.region_bytes),
                ("intent_regions", intent.regions),
                ("intent_named", intent.named),
                ("intent_named_at_start", intent.named_at_start),
                ("intent_syncs", intent.syncs.count),
                ("intent_sync_ns", intent.syncs.sum_ns),
                ("intent_sync_max_ns", intent.syncs.max_ns),
                ("intent_waits", intent.waits),
            ] {
                lines.push((key, value.to_string()));
            }
            // From a file of the sixteenth version on (M2.4d).
            if let Some(header) = intent.header {
                let on = if header.on { "on" } else { "off" };
                lines.push(("intent_header_sync", on.to_string()));
                for (key, value) in [
                    ("intent_header_syncs", header.syncs.count),
                    ("intent_header_sync_ns", header.syncs.sum_ns),
                    ("intent_header_sync_max_ns", header.syncs.max_ns),
                ] {
                    lines.push((key, value.to_string()));
                }
            }
        }
        // The uploader, from a file that has it, after the intent map:
        // every key, whether or not the daemon uploads.
        if let Some(upload) = shown.upload {
            let text = |text: Text| {
                let said = text.said();
                if said.is_empty() {
                    "none".to_string()
                } else {
                    said
                }
            };
            lines.push(("upload", upload.said()));
            lines.push(("upload_state", upload.state.said().to_string()));
            lines.push(("upload_cut", upload.cut.to_string()));
            lines.push(("upload_wait_ms", upload.wait_ms.to_string()));
            lines.push(("upload_why", text(upload.why)));
            lines.push(("lease", upload.lease.said().to_string()));
            lines.push(("lease_holder", text(upload.holder)));
            lines.push(("lease_epoch", upload.epoch.to_string()));
            lines.push(("lease_expires_unix_ms", upload.expires_unix_ms.to_string()));
            let f = &upload.figures;
            for (key, value) in [
                ("uploaded", f.uploaded),
                ("uploaded_whole", f.uploaded_whole),
                ("upload_blocks", f.blocks),
                ("upload_found_committed", f.found_committed),
                ("upload_committed_gone", f.not_held),
                ("upload_committed_named", f.named),
                ("upload_committed_kept", f.kept),
                ("upload_mismatches", f.mismatches),
                ("upload_abandoned_at_mismatch", f.abandoned_at_mismatch),
                ("upload_cuts_lost", f.lost),
                ("upload_given_up", f.gave_up),
                ("upload_errors", f.errors),
            ] {
                lines.push((key, value.to_string()));
            }
            lines.push(("upload_checked", upload.checked.said().to_string()));
            lines.push(("upload_rule", upload.rule.said().to_string()));
            lines.push(("upload_last_error", text(upload.last_error)));
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

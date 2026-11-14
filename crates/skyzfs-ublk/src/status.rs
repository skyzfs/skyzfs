//! What `skyzfsd status` reads: a small file the serving daemon keeps
//! under the volume's directory in the run directory, and the report
//! made from it, the attachment record, the device's note and the
//! kernel's word on the device. `status` opens nothing else: not the
//! store, and no channel to the daemon.
//!
//! # The file
//!
//! [`STATUS`] under the volume's run directory
//! ([`records::volume_run_dir`](crate::records::volume_run_dir)). 2032
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
//! This is the file's tenth version. The ninth (`SKYZSTA9`, 2008
//! bytes) ended before the queues' commands that the driver ended
//! otherwise than OK, from offset 2008 on; a file of that version is
//! still read, with those at zero, and `status` shows none of them. The
//! eighth (`SKYZSTA8`, 1960
//! bytes) ended before the in-place store's I/O threads' queue, from
//! offset 1960 on; a file of that version is still read, with that
//! queue absent (the log store has none, and shows the same). The
//! seventh (`SKYZSTA7`, 1856
//! bytes) ended before the worker's sixteenth activity (waiting for
//! the helper's report of a step's home-image sync), the waits by it
//! and the helper's account, from offset 1856 on; a file of that
//! version is still read, with those at zero. The worker's first
//! fifteen activities, and the requests' waits by them, are where
//! they were: the sixteenth of each comes after the fields of the
//! seventh version. The sixth (`SKYZSTA6`, 1584
//! bytes) ended before the hint that starts writeback early and the
//! syncs' backlogs, from offset 1584 on; a file of that version is
//! still read, with those at zero. The fifth (`SKYZSTA5`, 1424
//! bytes) had fourteen activities of the store's worker where this
//! one has fifteen, so the fields from offset 536 on lie elsewhere,
//! and ended before the barriers', the reader's and the checked
//! reads' fields; a file of that version is still read, with those at
//! zero, and its reads' times are the worker's (before the reads left
//! the worker, design §5.1). The fourth (`SKYZSTA4`, 200
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
//! | 0 | 8 | magic `SKYZST10` |
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
//! | 112 | 8 | `unstored`: completions waiting because the acknowledged position could not be stored. Always 0 since M1.3t-b, which removed that record; the field stays for what reads it |
//! | 120 | 8 | writes of this file that failed |
//! | 128 | 8 | `first_write_copies`: first writes of a tag that were copied from the transport's own buffer since this daemon started serving (at most the depth) |
//! | 136 | 8 | `recovered_unacknowledged`: operations this daemon's start found in the log beyond the acknowledged position (a lower bound: see `Store::recovered_unacknowledged`) |
//! | 144 | 8 | `log_live`: the bytes of the log from its start to its end, which the next start replays |
//! | 152 | 8 | `checkpoint_steps`: the steps the store's checkpoint rule has taken since this daemon started serving |
//! | 160 | 8 | 1 while such a step is under way, else 0; a step begins with the rule's own sync of the log when it needs one, and is under way while its home image's sync is with the helper, until the worker has taken it up again |
//! | 168 | 8 | `start_ms`: how long this daemon's start took until the device served |
//! | 176 | 8 | `replayed_bytes`: the bytes of log that start replayed |
//! | 184 | 8 | `checkpoint_syncs`: the syncs of the log the store's worker issued for its steps since this daemon started serving. They are nobody's request and are not among the flushes committed |
//! | 192 | 8 | `zeroes_applied`: how write-zeroes reach the home image, by a checkpoint of the log store or by the in-place store. 0: none applied since this daemon started serving; 1: their ranges are deallocated; 2: zeros are written, because the file system cannot deallocate; 3: zeros are written, because the file system's blocks are larger than the store's write unit, or it does not say; 4: zeros are written by direct I/O (the in-place store). Any other value is read as "not said" |
//! | 200 | 200 | the requests' times, five fields of 8 bytes for each kind in the order read, write, flush, discard, write-zeroes: the requests of that kind the store completed; their summed wait, from the request's submission to the store until the store's worker began it (for reads, the store's reader); the longest such wait; the summed time of the store's call for them; the longest such call |
//! | 400 | 112 | the checkpoint steps' times, a sum and a longest for each part in the order: reading the log to choose the batch; saving the neighbours of what the step writes; writing and deallocating in the home image; the home image's sync (the worker's part in it: the whole sync where the worker makes it, handing it over where a step of the rule gives it to the helper); the log's new start; the index letting go; and, not a part of a step, the rule's own sync of the log before one (`checkpoint_syncs` counts those) |
//! | 512 | 24 | the gate at the high mark: the turns of the worker in which a request waited and none was taken because the log was at the high mark; the summed time of the steps taken in those turns, which is a share of the parts above and not more time; the longest |
//! | 536 | 120 | the worker's whole time by what it did, fifteen fields in the order: idle; the call of a read (none since the reads have a reader of their own), a write, a flush (the log's sync), a discard, a write-zeroes; a step's six parts in the order above; the rule's own sync; anything else; a step waiting at one of its two barriers for the readers under way. They follow each other with no gap, so their sum is the time served |
//! | 656 | 600 | for each kind of request, in the order above: fifteen fields, its summed wait by what was done meanwhile, in the order of the worker's time. A kind's fifteen add up to its summed wait. For reads it is what the reader did: idle (its wake-up), the calls of reads before it, anything else; the other twelve are zero |
//! | 1256 | 40 | for each kind: of its longest wait, which of the fifteen took the largest part, as a number from 0 |
//! | 1296 | 40 | for each kind: how long that part was |
//! | 1336 | 24 | the pump, before the store has a request: how many requests it submitted; their summed time from the queue's wait returning with the request to the store having it; the longest |
//! | 1360 | 24 | the pump, after the store is done with a read: how many reads it committed; their summed time from the store's completion being ready to the commit; the longest |
//! | 1384 | 24 | the same for every other request |
//! | 1408 | 24 | the acknowledged position: how often it was stored; the summed time of that; the longest. Always 0 since M1.3t-b, as `unstored` |
//! | 1432 | 8 | the completions that were committed behind those stores. Always 0 since M1.3t-b |
//! | 1440 | 16 | of the reads' time from ready to the commit, the first part: from the store's completion being ready to the pump taking it, summed, and the longest. As many as the reads committed |
//! | 1456 | 16 | the same for every other request |
//! | 1472 | 48 | the step's barriers: before the home image is written, how many (one per batch), their summed wait for the readers under way, the longest; the same before a segment file the step freed can be used again. This wait is the worker's activity `step_barrier` above and in none of the step's parts |
//! | 1520 | 24 | the reader's whole time: idle, in the calls of reads, anything else. They follow each other with no gap |
//! | 1544 | 24 | the reads that found a record of the log that was not durable yet, whose whole payload the reader read and checked against its checksum: how many; the bytes they read beyond those asked for; how many records did not match (each failed the volume) |
//! | 1568 | 16 | up to when the worker's activities are complete, and the same for the reader's, in nanoseconds since the store began to be served: the moment of the look for a thread that waits for work, whose wait is counted as idle up to then; the end of its last span for a thread that is doing something. Between two files the worker's sixteen activities (the fifteen here and the one at offset 1856) sum to the difference of the first exactly, the reader's three to that of the second, and no sum goes down |
//! | 1584 | 80 | the hint that hands a file's writes to the device before their sync (`skyzfs_local::writeback`): for the log's segment files, and then for the home image, how many calls handed writes over, the bytes of the writes they handed over, the time of all calls, 1 if the file system answered "unsupported" and the hint was switched off (else 0), and how many calls the system declined this time (short of memory, interrupted, busy) while the volume went on. The time lies inside the activity the hint was made from: the call of a write, a write-zeroes or a discard for the log's, a step's apply for the home image's |
//! | 1664 | 192 | the syncs, by kind: a client's flush, the checkpoint rule's own sync of the log, a sync of the log that makes room for a write, a step's sync of the home image, of the preimage file, of the log's superblock. For each: how many, their time, the bytes written to the file since the sync before each, and of those how many the hint had handed over. A step's sync of the home image that the helper made counts from its hand-over to the worker's taking the report |
//! | 1856 | 8 | the worker's sixteenth activity: waiting for the helper's report of a step's home-image sync (at the high mark, before an explicit checkpoint, a change of the hold, room in a full log, and the end). With it the worker's sixteen activities follow each other with no gap |
//! | 1864 | 40 | for each kind of request, in the order above: its summed wait while the worker waited for that report. A kind's sixteen add up to its summed wait |
//! | 1904 | 24 | the home image's syncs that steps handed to the helper: how many, their summed time from the hand-over to the worker taking the report, the longest. Not the worker's time: it served meanwhile |
//! | 1928 | 24 | the worker's waits for that report: how many, their summed time, the longest |
//! | 1952 | 8 | the requests the worker completed while a sync was with the helper |
//! | 1960 | 8 | the in-place store's I/O threads: how many (0 for the log store, which has none, and for a build that does the I/O on the caller's thread) |
//! | 1968 | 8 | reads and writes on those threads now: handed to a thread and not yet taken back |
//! | 1976 | 8 | requests waiting now for a thread, in their order, because one on the threads overlaps them or a flush is behind a write still there, and everything received after the first of them |
//! | 1984 | 8 | the requests that waited so since the store began (only grows) |
//! | 1992 | 8 | the most reads and writes on the threads at once since the store began |
//! | 2000 | 8 | the most requests waiting at once since the store began |
//! | 2008 | 8 | `commands_aborted`: the queues' commands the driver ended with its abort (`-ENODEV`): the queue is being stopped. Each tag's last command ends so |
//! | 2016 | 8 | `commands_failed`: the queues' commands that ended with any other result than OK: a commit or a fetch the driver refused, or one io_uring could not hand to it. Nothing is done about them but this count and the request log's line (see [`Queue::wait`](crate::pump::Queue::wait)) |
//! | 2024 | 8 | `last_command_failure`: the result of the last of those, a negated error number, in the first 4 bytes as a signed number; 0 before the first |
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
use crate::pump::{Counters, PumpTiming};
use crate::records::StoredFailure;
use skyzfs_local::inplace::IoQueue;
use skyzfs_local::store::{RequestTiming, Spent, Synced, Timing, ACTIVITIES};
use skyzfs_local::writeback::Hinted;

/// The file's name under the volume's run directory.
pub const STATUS: &str = "status";

const MAGIC: &[u8; 8] = b"SKYZST10";
/// The version this crate writes.
pub const VERSION: u32 = 10;
const LEN: usize = 2032;

/// The ninth version: nothing of the commands the driver ended
/// otherwise than OK (offset 2008). A file of it is still read, with
/// those at zero.
const NINTH_MAGIC: &[u8; 8] = b"SKYZSTA9";
const NINTH_LEN: usize = 2008;

/// The eighth version: nothing from the I/O threads' queue on (offset
/// 1960). A file of it is still read, with that queue absent.
const EIGHTH_MAGIC: &[u8; 8] = b"SKYZSTA8";
const EIGHTH_LEN: usize = 1960;
const SEVENTH_MAGIC: &[u8; 8] = b"SKYZSTA7";
const SEVENTH_LEN: usize = 1856;
/// The sixth version: nothing from the hint on.
const SIXTH_MAGIC: &[u8; 8] = b"SKYZSTA6";
const SIXTH_LEN: usize = 1584;
/// The fifth version: fourteen activities, and nothing from the
/// barriers on.
const FIFTH_MAGIC: &[u8; 8] = b"SKYZSTA5";
const FIFTH_LEN: usize = 1424;
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
/// are nanoseconds but the counts and the five `*_longest_wait_cause`,
/// which name what the worker did
/// ([`skyzfs_local::store::ACTIVITIES`]). A key ending in `_max_ns` is the
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
    "time_step_walk_ns",
    "time_step_walk_max_ns",
    "time_step_save_ns",
    "time_step_save_max_ns",
    "time_step_apply_ns",
    "time_step_apply_max_ns",
    "time_step_home_sync_ns",
    "time_step_home_sync_max_ns",
    "time_step_log_start_ns",
    "time_step_log_start_max_ns",
    "time_step_end_ns",
    "time_step_end_max_ns",
    "time_step_own_sync_ns",
    "time_step_own_sync_max_ns",
    "time_gate_count",
    "time_gate_ns",
    "time_gate_max_ns",
    "time_worker_idle_ns",
    "time_worker_read_ns",
    "time_worker_write_ns",
    "time_worker_flush_ns",
    "time_worker_discard_ns",
    "time_worker_zeroes_ns",
    "time_worker_step_walk_ns",
    "time_worker_step_save_ns",
    "time_worker_step_apply_ns",
    "time_worker_step_home_sync_ns",
    "time_worker_step_log_start_ns",
    "time_worker_step_end_ns",
    "time_worker_own_sync_ns",
    "time_worker_other_ns",
    "time_worker_step_barrier_ns",
    "time_read_wait_by_idle_ns",
    "time_read_wait_by_read_ns",
    "time_read_wait_by_write_ns",
    "time_read_wait_by_flush_ns",
    "time_read_wait_by_discard_ns",
    "time_read_wait_by_zeroes_ns",
    "time_read_wait_by_step_walk_ns",
    "time_read_wait_by_step_save_ns",
    "time_read_wait_by_step_apply_ns",
    "time_read_wait_by_step_home_sync_ns",
    "time_read_wait_by_step_log_start_ns",
    "time_read_wait_by_step_end_ns",
    "time_read_wait_by_own_sync_ns",
    "time_read_wait_by_other_ns",
    "time_read_wait_by_step_barrier_ns",
    "time_write_wait_by_idle_ns",
    "time_write_wait_by_read_ns",
    "time_write_wait_by_write_ns",
    "time_write_wait_by_flush_ns",
    "time_write_wait_by_discard_ns",
    "time_write_wait_by_zeroes_ns",
    "time_write_wait_by_step_walk_ns",
    "time_write_wait_by_step_save_ns",
    "time_write_wait_by_step_apply_ns",
    "time_write_wait_by_step_home_sync_ns",
    "time_write_wait_by_step_log_start_ns",
    "time_write_wait_by_step_end_ns",
    "time_write_wait_by_own_sync_ns",
    "time_write_wait_by_other_ns",
    "time_write_wait_by_step_barrier_ns",
    "time_flush_wait_by_idle_ns",
    "time_flush_wait_by_read_ns",
    "time_flush_wait_by_write_ns",
    "time_flush_wait_by_flush_ns",
    "time_flush_wait_by_discard_ns",
    "time_flush_wait_by_zeroes_ns",
    "time_flush_wait_by_step_walk_ns",
    "time_flush_wait_by_step_save_ns",
    "time_flush_wait_by_step_apply_ns",
    "time_flush_wait_by_step_home_sync_ns",
    "time_flush_wait_by_step_log_start_ns",
    "time_flush_wait_by_step_end_ns",
    "time_flush_wait_by_own_sync_ns",
    "time_flush_wait_by_other_ns",
    "time_flush_wait_by_step_barrier_ns",
    "time_discard_wait_by_idle_ns",
    "time_discard_wait_by_read_ns",
    "time_discard_wait_by_write_ns",
    "time_discard_wait_by_flush_ns",
    "time_discard_wait_by_discard_ns",
    "time_discard_wait_by_zeroes_ns",
    "time_discard_wait_by_step_walk_ns",
    "time_discard_wait_by_step_save_ns",
    "time_discard_wait_by_step_apply_ns",
    "time_discard_wait_by_step_home_sync_ns",
    "time_discard_wait_by_step_log_start_ns",
    "time_discard_wait_by_step_end_ns",
    "time_discard_wait_by_own_sync_ns",
    "time_discard_wait_by_other_ns",
    "time_discard_wait_by_step_barrier_ns",
    "time_zeroes_wait_by_idle_ns",
    "time_zeroes_wait_by_read_ns",
    "time_zeroes_wait_by_write_ns",
    "time_zeroes_wait_by_flush_ns",
    "time_zeroes_wait_by_discard_ns",
    "time_zeroes_wait_by_zeroes_ns",
    "time_zeroes_wait_by_step_walk_ns",
    "time_zeroes_wait_by_step_save_ns",
    "time_zeroes_wait_by_step_apply_ns",
    "time_zeroes_wait_by_step_home_sync_ns",
    "time_zeroes_wait_by_step_log_start_ns",
    "time_zeroes_wait_by_step_end_ns",
    "time_zeroes_wait_by_own_sync_ns",
    "time_zeroes_wait_by_other_ns",
    "time_zeroes_wait_by_step_barrier_ns",
    "time_read_longest_wait_cause",
    "time_write_longest_wait_cause",
    "time_flush_longest_wait_cause",
    "time_discard_longest_wait_cause",
    "time_zeroes_longest_wait_cause",
    "time_read_longest_wait_cause_ns",
    "time_write_longest_wait_cause_ns",
    "time_flush_longest_wait_cause_ns",
    "time_discard_longest_wait_cause_ns",
    "time_zeroes_longest_wait_cause_ns",
    "time_pump_submit_count",
    "time_pump_submit_ns",
    "time_pump_submit_max_ns",
    "time_pump_commit_read_count",
    "time_pump_commit_read_ns",
    "time_pump_commit_read_max_ns",
    "time_pump_commit_other_count",
    "time_pump_commit_other_ns",
    "time_pump_commit_other_max_ns",
    "time_pump_store_count",
    "time_pump_store_ns",
    "time_pump_store_max_ns",
    "time_pump_store_completions",
    "time_pump_taken_read_ns",
    "time_pump_taken_read_max_ns",
    "time_pump_taken_other_ns",
    "time_pump_taken_other_max_ns",
    "time_barrier_home_count",
    "time_barrier_home_ns",
    "time_barrier_home_max_ns",
    "time_barrier_reuse_count",
    "time_barrier_reuse_ns",
    "time_barrier_reuse_max_ns",
    "time_reader_idle_ns",
    "time_reader_read_ns",
    "time_reader_other_ns",
    "time_read_unflushed_count",
    "time_read_unflushed_extra_bytes",
    "time_read_unflushed_mismatched",
    "time_worker_as_of_ns",
    "time_reader_as_of_ns",
    "time_writeback_log_count",
    "time_writeback_log_bytes",
    "time_writeback_log_ns",
    "time_writeback_log_off",
    "time_writeback_log_refused",
    "time_writeback_home_count",
    "time_writeback_home_bytes",
    "time_writeback_home_ns",
    "time_writeback_home_off",
    "time_writeback_home_refused",
    "time_sync_flush_count",
    "time_sync_flush_ns",
    "time_sync_flush_bytes",
    "time_sync_flush_handed",
    "time_sync_own_sync_count",
    "time_sync_own_sync_ns",
    "time_sync_own_sync_bytes",
    "time_sync_own_sync_handed",
    "time_sync_room_count",
    "time_sync_room_ns",
    "time_sync_room_bytes",
    "time_sync_room_handed",
    "time_sync_home_count",
    "time_sync_home_ns",
    "time_sync_home_bytes",
    "time_sync_home_handed",
    "time_sync_preimage_count",
    "time_sync_preimage_ns",
    "time_sync_preimage_bytes",
    "time_sync_preimage_handed",
    "time_sync_superblock_count",
    "time_sync_superblock_ns",
    "time_sync_superblock_bytes",
    "time_sync_superblock_handed",
    "time_worker_step_report_ns",
    "time_read_wait_by_step_report_ns",
    "time_write_wait_by_step_report_ns",
    "time_flush_wait_by_step_report_ns",
    "time_discard_wait_by_step_report_ns",
    "time_zeroes_wait_by_step_report_ns",
    "time_helper_count",
    "time_helper_ns",
    "time_helper_max_ns",
    "time_step_report_count",
    "time_step_report_ns",
    "time_step_report_max_ns",
    "time_helper_requests",
];

/// What the file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Status {
    /// The version of the file it was read from: 2 to [`VERSION`]. A
    /// file is always written as [`VERSION`], whatever this says.
    /// `status` prints it (`status_version`), so that whoever reads
    /// the times can tell the zeros of a field an older file did not
    /// have from a zero the daemon counted.
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
    /// Where the pump's time went. `None` in a file of a version
    /// before the fifth.
    pub pump: Option<PumpTiming>,
    /// The in-place store's I/O threads' queue. `None` in a file of a
    /// version before the ninth, and from the log store (which has no
    /// threads); a reader tells them apart by nothing, and the report
    /// shows the queue only where there are threads.
    pub io_queue: Option<IoQueue>,
}

/// How write-zeroes reach the home image, as the status file says it:
/// by a checkpoint of the log store, or by the in-place store.
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
            Self::WrittenForBlocks => 3,
            Self::WrittenDirect => 4,
            Self::Unsaid => u64::MAX,
        }
    }

    fn of(code: u64) -> Self {
        match code {
            0 => Self::Untried,
            1 => Self::Deallocated,
            2 => Self::Written,
            3 => Self::WrittenForBlocks,
            4 => Self::WrittenDirect,
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
            Self::WrittenDirect => Some(
                "written as zeros by direct I/O: the in-place store does not deallocate, since \
                 the next writes to a deallocated range would allocate it again, and a \
                 write-zeroes takes as long as writing its zeros",
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
    let times = time_fields(
        &log.shown.timing.unwrap_or_default(),
        &log.shown.pump.unwrap_or_default(),
    );
    for (i, field) in fields.iter().chain(&times).enumerate() {
        bytes[48 + 8 * i..56 + 8 * i].copy_from_slice(&field.to_le_bytes());
    }
    // The ninth version's I/O threads' queue, from offset 1960.
    let q = log.shown.io_queue.unwrap_or_default();
    for (i, field) in [
        q.threads,
        q.in_flight,
        q.waiting,
        q.waited,
        q.in_flight_max,
        q.waiting_max,
    ]
    .iter()
    .enumerate()
    {
        bytes[EIGHTH_LEN + 8 * i..EIGHTH_LEN + 8 + 8 * i].copy_from_slice(&field.to_le_bytes());
    }
    // The tenth version's commands the driver ended otherwise than OK,
    // from offset 2008.
    bytes[NINTH_LEN..NINTH_LEN + 8].copy_from_slice(&c.commands_aborted.to_le_bytes());
    bytes[NINTH_LEN + 8..NINTH_LEN + 16].copy_from_slice(&c.commands_failed.to_le_bytes());
    bytes[NINTH_LEN + 16..NINTH_LEN + 20].copy_from_slice(&c.last_command_failure.to_le_bytes());
    bytes
}

/// The number of 8-byte fields the times take.
const TIME_FIELDS: usize = 220;

/// The activities of the worker whose fields lie where the seventh
/// version had them; the sixteenth comes after that version's fields.
const SEVENTH_ACTIVITIES: usize = 15;

/// Where the five fields begin that name an activity: the largest
/// cause of each kind's longest wait.
const CAUSE_FIELDS: std::ops::Range<usize> = 132..137;

/// The times in the file's order: see the table at the top.
fn time_fields(timing: &Timing, pump: &PumpTiming) -> [u64; TIME_FIELDS] {
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
        timing.step_walk,
        timing.step_save,
        timing.step_apply,
        timing.step_home_sync,
        timing.step_start,
        timing.step_end,
        timing.step_own_sync,
    ] {
        put(part.sum_ns);
        put(part.max_ns);
    }
    put(timing.gate.count);
    put(timing.gate.sum_ns);
    put(timing.gate.max_ns);
    for value in &timing.worker[..SEVENTH_ACTIVITIES] {
        put(*value);
    }
    for value in timing
        .wait_by
        .iter()
        .flat_map(|kind| &kind[..SEVENTH_ACTIVITIES])
    {
        put(*value);
    }
    for value in timing.longest_wait_cause {
        put(value);
    }
    for value in timing.longest_wait_cause_ns {
        put(value);
    }
    for spent in [
        pump.to_submit,
        pump.read_to_commit,
        pump.other_to_commit,
        pump.store,
    ] {
        put(spent.count);
        put(spent.sum_ns);
        put(spent.max_ns);
    }
    put(pump.behind_store);
    for spent in [pump.read_to_taken, pump.other_to_taken] {
        put(spent.sum_ns);
        put(spent.max_ns);
    }
    for spent in [timing.barrier_home, timing.barrier_reuse] {
        put(spent.count);
        put(spent.sum_ns);
        put(spent.max_ns);
    }
    for value in timing.reader.iter().chain(&timing.read_unflushed) {
        put(*value);
    }
    put(timing.worker_as_of_ns);
    put(timing.reader_as_of_ns);
    for hinted in timing.writeback {
        put(hinted.count);
        put(hinted.bytes);
        put(hinted.ns);
        put(u64::from(hinted.off));
        put(hinted.refused);
    }
    for synced in timing.syncs {
        put(synced.count);
        put(synced.sum_ns);
        put(synced.bytes);
        put(synced.handed);
    }
    for value in timing.worker[SEVENTH_ACTIVITIES..].iter().chain(
        timing
            .wait_by
            .iter()
            .flat_map(|kind| &kind[SEVENTH_ACTIVITIES..]),
    ) {
        put(*value);
    }
    for spent in [timing.helper, timing.step_report] {
        put(spent.count);
        put(spent.sum_ns);
        put(spent.max_ns);
    }
    put(timing.helper_requests);
    fields
}

/// The pump's times of a file of the sixth version, or of the fifth.
fn pump_at(bytes: &[u8], fifth: bool) -> PumpTiming {
    let base = if fifth { 1288 } else { 1336 };
    let mut at = base;
    let mut spent = || {
        let fields = [
            u64_at(bytes, at),
            u64_at(bytes, at + 8),
            u64_at(bytes, at + 16),
        ];
        at += 24;
        Spent {
            count: fields[0],
            sum_ns: fields[1],
            max_ns: fields[2],
        }
    };
    let (to_submit, read_to_commit, other_to_commit, store) = (spent(), spent(), spent(), spent());
    // The file has no count for these: each is counted with a commit.
    let taken = |at: usize, count: u64| Spent {
        count,
        sum_ns: u64_at(bytes, at),
        max_ns: u64_at(bytes, at + 8),
    };
    PumpTiming {
        to_submit,
        read_to_commit,
        other_to_commit,
        read_to_taken: taken(base + 104, read_to_commit.count),
        other_to_taken: taken(base + 120, other_to_commit.count),
        store,
        behind_store: u64_at(bytes, base + 96),
    }
}

/// The times of a file of the seventh version, the sixth or the fifth.
/// What the file does not hold is filled in where another field says
/// it (the count of a request's calls is that of its waits) and left
/// zero where none does (how many batches a step's parts were measured
/// over; in the fifth version, the fifteenth activity and everything
/// from the barriers on; in the sixth, the hint and the syncs).
fn timing_at(bytes: &[u8], fifth: bool, seventh: bool, eighth: bool) -> Timing {
    let activities = if fifth { 14 } else { SEVENTH_ACTIVITIES };
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
    timing.step_walk = part();
    timing.step_save = part();
    timing.step_apply = part();
    timing.step_home_sync = part();
    timing.step_start = part();
    timing.step_end = part();
    timing.step_own_sync = part();
    timing.gate = Spent {
        count: take(),
        sum_ns: take(),
        max_ns: take(),
    };
    for value in &mut timing.worker[..activities] {
        *value = take();
    }
    for kind in &mut timing.wait_by {
        for value in &mut kind[..activities] {
            *value = take();
        }
    }
    for value in &mut timing.longest_wait_cause {
        *value = take();
    }
    for value in &mut timing.longest_wait_cause_ns {
        *value = take();
    }
    if !fifth {
        let mut at = 1472;
        let mut take = || {
            let value = u64_at(bytes, at);
            at += 8;
            value
        };
        for barrier in [&mut timing.barrier_home, &mut timing.barrier_reuse] {
            *barrier = Spent {
                count: take(),
                sum_ns: take(),
                max_ns: take(),
            };
        }
        for value in timing
            .reader
            .iter_mut()
            .chain(timing.read_unflushed.iter_mut())
        {
            *value = take();
        }
        timing.worker_as_of_ns = take();
        timing.reader_as_of_ns = take();
    }
    if seventh {
        let mut at = 1584;
        let mut take = || {
            let value = u64_at(bytes, at);
            at += 8;
            value
        };
        for hinted in &mut timing.writeback {
            *hinted = Hinted {
                count: take(),
                bytes: take(),
                ns: take(),
                off: take() != 0,
                refused: take(),
            };
        }
        for synced in &mut timing.syncs {
            *synced = Synced {
                count: take(),
                sum_ns: take(),
                bytes: take(),
                handed: take(),
            };
        }
    }
    if eighth {
        let mut at = SEVENTH_LEN;
        let mut take = || {
            let value = u64_at(bytes, at);
            at += 8;
            value
        };
        for value in timing.worker[SEVENTH_ACTIVITIES..].iter_mut().chain(
            timing
                .wait_by
                .iter_mut()
                .flat_map(|kind| &mut kind[SEVENTH_ACTIVITIES..]),
        ) {
            *value = take();
        }
        for spent in [&mut timing.helper, &mut timing.step_report] {
            *spent = Spent {
                count: take(),
                sum_ns: take(),
                max_ns: take(),
            };
        }
        timing.helper_requests = take();
    }
    timing
}

fn decode(bytes: &[u8]) -> Option<Status> {
    let tenth = bytes.len() == LEN && &bytes[..8] == MAGIC;
    let ninth = tenth || (bytes.len() == NINTH_LEN && &bytes[..8] == NINTH_MAGIC);
    let eighth = ninth || (bytes.len() == EIGHTH_LEN && &bytes[..8] == EIGHTH_MAGIC);
    let seventh = eighth || (bytes.len() == SEVENTH_LEN && &bytes[..8] == SEVENTH_MAGIC);
    let sixth = seventh || (bytes.len() == SIXTH_LEN && &bytes[..8] == SIXTH_MAGIC);
    let only_fifth = bytes.len() == FIFTH_LEN && &bytes[..8] == FIFTH_MAGIC;
    let fifth = sixth || only_fifth;
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
            timing: fifth.then(|| timing_at(bytes, only_fifth, seventh, eighth)),
            pump: fifth.then(|| pump_at(bytes, only_fifth)),
            io_queue: ninth.then(|| IoQueue {
                threads: u64_at(bytes, 1960),
                in_flight: u64_at(bytes, 1968),
                waiting: u64_at(bytes, 1976),
                waited: u64_at(bytes, 1984),
                in_flight_max: u64_at(bytes, 1992),
                waiting_max: u64_at(bytes, 2000),
            }),
        },
        start_ms: u64_at(bytes, 168),
        replayed_bytes: u64_at(bytes, 176),
    });
    let mut boot = [0u8; 16];
    boot.copy_from_slice(&bytes[16..32]);
    let mut identity = [0u8; 16];
    identity.copy_from_slice(&bytes[32..48]);
    let version = if tenth {
        10
    } else if ninth {
        9
    } else if eighth {
        8
    } else if seventh {
        7
    } else if sixth {
        6
    } else if fifth {
        5
    } else if fourth {
        4
    } else if third {
        3
    } else {
        2
    };
    Some(Status {
        version,
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
            commands_aborted: if tenth { u64_at(bytes, 2008) } else { 0 },
            commands_failed: if tenth { u64_at(bytes, 2016) } else { 0 },
            last_command_failure: if tenth {
                i32::from_le_bytes([bytes[2024], bytes[2025], bytes[2026], bytes[2027]])
            } else {
                0
            },
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
    log: LogShown,
    /// The log as posted is to be written without waiting for the
    /// interval.
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

    /// Put the log's state up, as [`Board::post`] does the counters.
    /// With `at_once` it is written without waiting for the interval:
    /// for the begin and the end of a checkpoint step.
    pub fn post_log(&self, log: LogShown, at_once: bool) {
        let mut posted = self.lock();
        if posted.log != log {
            posted.log = log;
            posted.at_once |= at_once;
            if at_once || posted.awaits_change || posted.every_post_wakes {
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
        posted.every_post_wakes = pace.mutants.has(Mutants::WOKEN_BY_EVERY_POST);
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
            ("recovered_unacknowledged", s.recovered_unacknowledged),
            ("status_write_failures", s.failures),
            ("age_ms", uptime_ms.saturating_sub(s.written_ms)),
            ("status_version", u64::from(s.version)),
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
            if let (Some(timing), Some(pump)) = (&log.shown.timing, &log.shown.pump) {
                let fields = time_fields(timing, pump);
                for (at, (key, value)) in TIME_KEYS.iter().zip(fields).enumerate() {
                    // A cause is said by its name; a number this
                    // version has no name for is printed as it is.
                    let name = usize::try_from(value)
                        .ok()
                        .filter(|_| CAUSE_FIELDS.contains(&at))
                        .and_then(|cause| ACTIVITIES.get(cause));
                    let said = name.map_or_else(|| value.to_string(), |name| (*name).to_string());
                    lines.push((key, said));
                }
            }
            // The in-place store's I/O threads' queue, where there are
            // threads (the log store has none, and shows nothing).
            if let Some(q) = log.shown.io_queue.filter(|q| q.threads > 0) {
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

//! The in-place store: the volume's home image, written in place by
//! direct I/O and synced at a flush, with no write log (work packages
//! M1.3r to M1.3t).
//!
//! A volume is a directory that holds the volume file
//! ([`crate::volume`]), which says what the volume is, and the home
//! image (`home.img`), which holds it. [`create`] makes one; a start
//! opens it in two phases, [`Opened::open`] under the volume's lock and
//! [`Opened::start`] once the daemon may serve (the `start` module).
//! Beside them the directory holds the lock file and, once the volume
//! has failed, the failure marker (`volume.failed`). Until M1.3t the
//! store was entered from the log store, whose open applied its log to
//! the home image first; the log store is gone, and a directory it made
//! is refused ([`crate::volume::VolumeError::Legacy`]).
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
//! 1. **Process loss.** A write or write-zeroes is acknowledged once
//!    its direct write has returned: it has reached the device (its
//!    volatile write cache, where it has one), past the kernel's cache
//!    (decision D6, [`IoMode`]). It is there after the process is lost,
//!    and so is everything acknowledged before it (rules R9 and R8);
//!    this no longer rests on the page cache. A discard does nothing
//!    (see below). A write in flight when the process dies may be there
//!    in part, block by block; the kernel issues it again (rule R9),
//!    and the next instance makes it whole. Where the write went into a
//!    hole or an unwritten extent, a returned direct write is not
//!    durable on its own: it survives a power cut only once the file
//!    system has committed its record that the block is written, which
//!    the next sync does. Until then a power cut may leave the block
//!    reading as zeros, its content before the write, which point 3
//!    allows; after a completed flush it never does. The replay rig
//!    showed both on ext4 and XFS (design §10, the allocation row):
//!    preallocating does not change it, and an image written through
//!    first does not show it. So the image stays sparse, and `skyzfsd
//!    create --zero-fill` (which writes it whole) is an option. That the
//!    record is visible to the next instance at once, after a lost
//!    process, is my reading of the two file systems, not shown here.
//! 2. **A flush** completes only when every operation acknowledged
//!    before it was received is durable (rule R7).
//! 3. **Power loss.** Every block that a completed flush covered holds
//!    what it held at that flush, or what was written to it after. Of
//!    a block written since its last flush, each sector holds the
//!    block's durable content or one of the versions written since,
//!    independently of every other sector and of every other block: a
//!    later write can survive while an earlier one is lost, and a
//!    write can be torn at a sector. A start makes what it finds
//!    durable before it serves ([`Opened::start`]): its sync of the
//!    home image makes durable what the instance before it wrote by
//!    direct I/O (on the device, perhaps only in its cache). A test
//!    pins it (`a_power_cut_after_a_start_keeps_what_the_instance_before_wrote`).
//!    The start then serves only what the disk holds: it drops the
//!    kernel's cache of the home image and checks that no page of it is
//!    left (`cachestat`), or refuses the volume (design §4.1). A failed
//!    sync can leave pages marked clean that the disk never got, and no
//!    later sync writes them; served from the cache, they would be
//!    shown and then taken by a cut behind a completed flush. The drop
//!    covers a start that acknowledges a failure as well as one after
//!    an instance that died between a failed sync and its marker, whose
//!    own sync succeeds because the kernel reports such an error once.
//!    Under direct I/O the store's own reads and writes leave no page
//!    in the cache, so such pages come only from a writer through it: a
//!    file system that accepts a direct request, reports an alignment
//!    for it and still serves it through its cache. (Until M1.3t the log
//!    store's checkpoint at the entry into this store was one too.) The
//!    start's check of direct I/O (`STATX_DIOALIGN`) cannot tell such a
//!    file system apart, so the drop stays. The design's example, ext4 with `data=journal`, is
//!    not one: as read in ext4 and not shown, its `statx` reports no
//!    direct-I/O alignment for an inode whose data is journalled, so the
//!    start refuses that volume by reason instead of serving it. No file
//!    system is known here that passes the check and serves through the
//!    cache; the drop is for one that might. Its cost: the first reads
//!    after every start, a take-over included, come from the disk.
//! 4. **Failure is final** (rule R11's failure rule). Once a write or
//!    a sync of the home image has failed, the volume has failed: from
//!    then on every request fails, reads included, and a later start
//!    refuses the volume until the failure is acknowledged (the failure
//!    marker, `volume.failed`). A failed sync is never tried again: the kernel may have
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
//! The store has no log, so it has no position of what was
//! acknowledged, and the daemon keeps no record of one (since M1.3t-b).
//! At a start after a lost process nothing can show that the home image
//! holds less than the kernel was told: the store's promise rests on
//! the direct write having reached the device for that (point 1).
//!
//! Where the page or the file system's block is larger than a block
//! (16 KiB or 64 KiB pages on some arm64 kernels and on Apple silicon),
//! a direct write still writes only the blocks it is given: it does not
//! rewrite the other blocks of its page, as a write through the cache
//! does. The tests run at 16 KiB and 64 KiB pages on the simulated disk
//! (`tests.rs`).
//!
//! **A limit that direct I/O removes, on the simulated disk: a disk
//! that garbles.** On a disk that leaves a sector neither old nor new at
//! a power cut, a write through the cache on a large page rewrites the
//! page's other blocks and exposes them, so a cut can damage a block a
//! completed flush covered and nothing wrote since: in the store's
//! power-cut test, through the cache, at 120 of 200 seeds at 16 KiB and
//! 113 at 64 KiB (measured in review), and at 0 at 4 KiB.
//! `a_cut_with_garbage_may_reach_a_neighbour_on_large_pages` pins that
//! some seed does so through the cache, which only tests still use;
//! with direct I/O no seed of the 200 does at either large page
//! (`direct_io_keeps_a_cut_with_garbage_from_the_neighbours_on_large_pages`,
//! which says what that rests on on a real file system). That is the
//! reason for decision D6.
//!
//! The other rules (R1 to R9, R11, R12) hold, and the conformance
//! suite runs on this store with every capability (`harness.rs`).
//!
//! # How it executes
//!
//! Reads and writes go by direct I/O to a pool of I/O threads
//! ([`IoMode::threads`], eight unless asked otherwise), each with its
//! own handle on the home image and one request at a time; their
//! completions are taken back on the caller's thread. A request waits,
//! in order, behind one on the threads that it must not run beside: a
//! read behind an overlapping write, a write or a write-zeroes behind
//! an overlapping read or write, so that overlapping requests take
//! effect in the order they came; and a flush behind every write on the
//! threads (it covers what was acknowledged before it, and this store
//! also has it wait for what was submitted before it). Everything
//! received after a waiting request waits behind it. On the simulated
//! disk a write applies whole under the disk's lock, so a read beside
//! an overlapping write never sees part of it and the tests that run
//! whole programs say little of the clauses about reads; on a file
//! system a direct read beside an overlapping direct write may see part
//! of it, which is what those clauses are for, and only the kernel rigs
//! show them there. Each clause has a test of its own (`tests.rs`: a
//! write behind a read, a read behind a write, a flush behind a write
//! beside a read). A write-zeroes is written as zeros by direct I/O
//! on the caller's thread, 1 MiB at a time (a deallocated range would
//! make the next writes to it allocate again), and a discard does
//! nothing. A thread that panics dies, the
//! job it held comes back failed, and the volume fails as from a
//! failed write; the requests that waited for the threads fail with it
//! (`inplace/pool.rs`). The store's drop waits for every request on the
//! threads, up to one per thread, where on the caller's thread there
//! was at most one: a daemon's exit waits for the direct I/O it has in
//! flight (the daemon rig's check e watches a request held at the
//! server). The store's queue (on the threads, waiting, and the
//! highs) is in [`InPlace::io_queue`], which the daemon's status file
//! shows.
//!
//! Rule R6 allows a discarded block to keep its content, and that is
//! the least work: no call, no metadata to make durable, and nothing a
//! power cut can tear. It counts as a change all the same, so that the
//! flush behind it syncs: a flush that follows a client's change never
//! succeeds on a disk that has failed (R11), at the cost of one sync
//! where a discard alone came before it. A flush begins a sync of the
//! home image on the file's helper ([`File::sync_begin`]) and completes
//! when the sync ends; writes go on meanwhile. Flushes that arrive
//! while a sync is under way join it when nothing was written since it
//! began, and otherwise wait for the next one, which begins when this
//! one ends and covers them all.
//!
//! Test builds can also do the I/O on the caller's thread, in
//! [`BlockTarget::submit`], or through the kernel's cache, where a
//! write-zeroes deallocates the range (`IoMode::for_tests`). Neither is
//! a mode the store is served in.
//!
//! # Held cuts (M2.2)
//!
//! A store started with a region for the cut state
//! ([`OpenOptions::hold_cuts`]) keeps it (`inplace/state.rs`), taken
//! up once the start's sync has succeeded, which counts as the flush of
//! every cut whose flush a death left under way. The count of cuts
//! (`inplace/cuts.rs`) tells it what happened: the mark of an uberblock
//! write is recorded once the write is whole, before anything it saw is
//! completed, so before its completion goes to the transport; a cut is
//! frozen at its flush's barrier, and its flush's completion or failure
//! is the cut's. Before a write or a write-zeroes is begun, on the
//! queue's thread, its blocks pass through the cut state: a block a
//! held cut reads is read from the home image and copied into
//! `copies.img` there and then, by direct I/O as the image is written
//! (M2.2c), and the write begins only after the copy is written. The
//! copies of a write's blocks are made in runs: one read of the image
//! and one write of `copies.img` for each run of blocks that are
//! consecutive and go to consecutive slots, at most 1 MiB (M2.2d;
//! `state.rs`, `CutState::change`). A copy
//! that cannot be made (a full file system:
//! `copies.img` grows a slot at a time and is not allocated up to the
//! bound; or a failed write of it, or read of the image) lets go the
//! held cuts that need it, and the write goes on: a recovery point is
//! worth less than the volume (the owner's ruling, through the lead,
//! M2.2b; `state.rs`). A discard is not noted: it changes no content
//! here.
//!
//! The region ([`CutRegion`]) is a named one the kernel keeps while the
//! machine is up, which the open maps, allocated in full and locked
//! (`skyzfs-mapped`; the daemon's is a file in the volume's run
//! directory, on tmpfs), or one in memory, which outlives a death only
//! in the tests. An open that cannot have the region is refused
//! ([`OpenError::CutRegion`]) before anything of the volume is written.
//! The start takes the state up only where it can trust it, and
//! otherwise begins a new one, which counts the whole volume as changed;
//! it says which ([`InPlace::cuts_at_start`]), and the rules are at
//! `take_up_cut_state`. A store started with no region holds no cut, as
//! the daemon's does unless told to (`serve --hold-cuts`): before the
//! uploader (M2.4) nothing lets a held cut go but the rigs' commands.
//!
//! # The intent map (M2.3)
//!
//! Beside the cut state, a store that holds cuts keeps the intent map
//! (`inplace/intent.rs`): a bit for each region of the volume (4 MiB
//! by default, [`HoldCuts::intent_region`]), in `intent.map` in the
//! volume's directory, on the image's own durable file system. A region
//! that may differ from the newest committed cut has its bit set on the
//! disk. So a write or a write-zeroes into a region whose bit the disk
//! does not hold waits until the bit's page is written and the map
//! synced; a failure there fails the volume, as a failed write does.
//! The sync is on the map's file's helper, as the image's are, and one
//! sync covers every bit set since the one before it began. Only the
//! waiting request, and those that must not run beside it (one that
//! overlaps it, a flush), wait: the queue's thread goes on with the
//! rest (the lead's ruling on M2.3b). A bit is cleared only after its cut is committed
//! (the test-hooks build's release stands in for that until M2.4), and
//! only where nothing in the region changed after that cut's instant.
//! After a restart of the machine the cut state is gone, and after a
//! failure that is acknowledged it is not taken up; where the start
//! takes the map up, the new state counts as changed only the blocks of
//! the regions it names, and otherwise the whole volume. The map is
//! taken up after an acknowledged failure as after a power cut: no
//! write began before its bit was durable (the lead's question 1 on
//! M2.3). The start says which ([`InPlace::intent_at_start`]), keeps
//! the map under its own token, and is refused where it cannot write
//! and sync it ([`StartError::Intent`]). The map's syncs on the write
//! path, with their time, are in [`InPlace::intent_figures`] and the
//! status (version 14).

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::io;
use std::sync::Arc;
use std::time::Duration;

use skyzfs_blk::{
    BlkError, BlockTarget, Buf, Completion, Geometry, Op, Request, RequestId, SubmitError,
    SubmitErrorKind,
};

use crate::io::{Dir, File, NotBegun};
use crate::store::{FailureObserver, Spent, Zeroes};
use cuts::CutRule;
use pool::{Done, Job, Pool};
use skyzfs_uberblock::State;
use state::CutState;

pub use cuts::{Cut, Cuts, Seen, Watcher};
pub use intent::Base as CommittedCut;
pub use state::{
    records_for, words_for, Figures as CutFigures, HeldCut, Lost, MemRegion, Phase as CutPhase,
    DEFAULT_INTENT_REGION, MAX_INTENT_REGION, MIN_INTENT_REGION,
};

pub use start::{
    create, create_zero_filled, identity_of, lock, CutRegion, HoldCuts, OpenError, OpenOptions,
    Opened, Under, VolumeConfig,
};

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u128);

impl Mutants {
    /// A flush that arrives while a sync is under way joins it even
    /// when something was written since the sync began.
    pub(crate) const FLUSH_JOINS_ANY_SYNC: u128 = 1 << 0;
    /// After a failed sync the volume is not failed: the next flush
    /// tries again.
    pub(crate) const SYNC_RETRIED: u128 = 1 << 2;
    /// A flush with no sync under way completes at once whatever was
    /// written since the last sync.
    pub(crate) const FLUSH_WITHOUT_SYNC: u128 = 1 << 3;
    /// A failed write does not fail the volume.
    pub(crate) const WRITE_ERROR_PASSES: u128 = 1 << 4;
    /// A discard is not counted as a change: a flush behind it alone
    /// does not sync.
    pub(crate) const DISCARD_UNCHANGED: u128 = 1 << 5;
    /// No thread is started for the failure marker: it is written on
    /// the caller's thread when the volume fails.
    pub(crate) const MARKER_INLINE: u128 = 1 << 6;
    /// The start does not drop the home image's cache.
    pub(crate) const CACHE_NOT_DROPPED: u128 = 1 << 7;
    /// The start does not check that the cache is gone.
    pub(crate) const CACHE_NOT_CHECKED: u128 = 1 << 8;
    /// The start does not show that the count sees the file.
    pub(crate) const CACHE_SIGHT_UNCHECKED: u128 = 1 << 9;
    /// The start shows that the count sees the file before the drop,
    /// not after it.
    pub(crate) const LOOK_BEFORE_DROP: u128 = 1 << 10;
    /// A thread's death is taken for an I/O error of the job it held:
    /// the volume goes on, and the jobs that wait for the threads are
    /// left to them.
    pub(crate) const DEATH_IGNORED: u128 = 1 << 11;
    /// A flush does not wait for the writes on the threads.
    pub(crate) const FLUSH_PASSES_WRITES: u128 = 1 << 12;
    /// A request does not wait for an overlapping one on the threads.
    pub(crate) const OVERLAP_UNORDERED: u128 = 1 << 13;
    /// The start does not sync the home image.
    pub(crate) const START_UNSYNCED: u128 = 1 << 14;
    /// The failure observer is told after the marker is begun, not
    /// before.
    pub(crate) const OBSERVE_AFTER_MARKER: u128 = 1 << 15;
    /// The failure marker is made without the directory's sync, so a
    /// power cut can take it back.
    pub(crate) const MARKER_UNSYNCED: u128 = 1 << 16;
    /// A failed read fails the volume (the log store's rule, which the
    /// in-place store does not have).
    pub(crate) const READ_ERROR_FAILS: u128 = 1 << 17;
    /// Create and open do not hold the file systems' blocks to the
    /// write unit.
    pub(crate) const ANY_BLOCKS: u128 = 1 << 18;
    /// Create and open do not hold the system's page to the write unit.
    pub(crate) const ANY_PAGES: u128 = 1 << 19;
    /// A job pushed to the I/O threads wakes every waiting thread, not
    /// one.
    pub(crate) const POOL_WAKES_ALL: u128 = 1 << 20;
    /// An I/O thread looks at the queue and waits under two holds of
    /// its lock: a job pushed between them wakes nobody.
    pub(crate) const POOL_WAKE_LOST: u128 = 1 << 21;
    /// At the end the I/O threads end without doing the jobs queued.
    pub(crate) const POOL_ENDS_QUEUED: u128 = 1 << 22;
    /// An I/O thread that ends a job waits to be woken before it looks
    /// at the queue again.
    pub(crate) const POOL_WAITS_FIRST: u128 = 1 << 23;
    /// Where a thread cannot be started, the threads started before it
    /// are left waiting.
    pub(crate) const POOL_LEAVES_STARTED: u128 = 1 << 24;
    /// A write handed to the threads is acknowledged at once, before a
    /// thread has written it: a kill can lose it.
    pub(crate) const ACK_BEFORE_WRITE: u128 = 1 << 25;
    /// A failure writes no marker: the next start serves the volume.
    pub(crate) const FAIL_UNMARKED: u128 = 1 << 26;
    /// The start's look reads with the kernel's read-ahead, which the
    /// next start's drop cannot take while it is under I/O.
    pub(crate) const LOOK_READS_AHEAD: u128 = 1 << 27;
    /// The open holds the directory's file system to the write unit and
    /// not the home image's: an image on a file system of its own (a
    /// mount point, a link elsewhere) goes unchecked.
    pub(crate) const HOME_ANY_BLOCKS: u128 = 1 << 28;
    /// An uberblock write marks when the store receives it, not once
    /// it is applied: a flush received while it is being written
    /// carries a cut (design §5.2).
    pub(crate) const MARK_AT_RECEIPT: u128 = 1 << 29;
    /// A repeat marks: every applied uberblock is followed by a cut.
    pub(crate) const REPEAT_COUNTED: u128 = 1 << 30;
    /// An uberblock whose group is 0 (a new label's) marks.
    pub(crate) const GROUP_ZERO_COUNTED: u128 = 1 << 31;
    /// A cut is counted as its flush is received, not at the flush's
    /// barrier.
    pub(crate) const CUT_AT_RECEIPT: u128 = 1 << 32;
    /// A repeat is told by its group alone, not by its group and root
    /// block pointer, against the mark, a cut a flush carries and the
    /// newest cut: after a rewind import a new state under a group
    /// already used makes no cut.
    pub(crate) const REPEAT_BY_GROUP: u128 = 1 << 33;
    /// A repeat is compared with the uberblock applied before it alone,
    /// not with the mark and the newest cut: under multihost a stale
    /// copy of the group before, applied after the group's own
    /// uberblocks, moves the mark back (design §5.2).
    pub(crate) const STALE_MOVES_MARK: u128 = 1 << 34;
    /// A request received after a flush that waits begins before it
    /// when nothing on the threads holds the request back itself: the
    /// half of a flush's barrier that M1 left untested (design §5.2).
    pub(crate) const PASSES_WAITING_FLUSH: u128 = 1 << 35;
    /// A repeat is not compared with the state a flush carries to its
    /// barrier: another label's copy of the same uberblock, applied
    /// between that flush's receipt and its barrier, marks it again, and
    /// the next flush cuts the same state twice (the review of M2.1).
    pub(crate) const REPEAT_WHILE_CARRIED: u128 = 1 << 36;
    /// Whether a cut opens as itself is judged by the group alone, not
    /// by ZFS's order (the group, then the timestamp, then the
    /// multihost sequence).
    pub(crate) const ORDER_BY_GROUP: u128 = 1 << 37;
    /// The start does not read the rings: they are taken to hold no
    /// uberblock, and what was written before the store began is not
    /// seen.
    pub(crate) const RINGS_UNREAD: u128 = 1 << 38;
    /// A cut opens as itself only if no uberblock in view ranks above
    /// the one that marks it: uberblocks are compared, not states, so a
    /// multihost copy of the cut's own state with a later timestamp
    /// makes it a rewind (the lead's reading of M2.1b, point 4).
    pub(crate) const ITSELF_BY_RANK: u128 = 1 << 39;
    /// An uberblock's label checksum is not checked: bytes in a ring that
    /// carry the magic and a group but that ZFS would pass over mark, and
    /// are held in the view (work package M2.1c).
    pub(crate) const CHECKSUM_UNCHECKED: u128 = 1 << 40;
    /// The cut state's records of copies are kept in memory only: a
    /// copy is made and read, but a later instance does not know of it
    /// (M2.2, "no record of a copy").
    pub(crate) const COPY_UNRECORDED: u128 = 1 << 41;
    /// A block is copied after the write that changes it, not before.
    pub(crate) const COPY_AFTER_OVERWRITE: u128 = 1 << 42;
    /// At the bound, the abandoned cut's map is dropped, not given on.
    pub(crate) const ABANDONED_MAP_DROPPED: u128 = 1 << 43;
    /// A cut that replaces the one that waits does not take its map.
    pub(crate) const REPLACED_MAP_DROPPED: u128 = 1 << 44;
    /// At the bound, every copy the abandoned cut needed is freed, those
    /// the cut that waits needs too.
    pub(crate) const ABANDONED_COPIES_FREED: u128 = 1 << 45;
    /// Every copy a replaced cut needed is freed, those the cut that
    /// replaced it needs too.
    pub(crate) const REPLACED_COPIES_FREED: u128 = 1 << 46;
    /// At the bound, the abandoned map goes to a cut whose flush is
    /// under way, though another waits.
    pub(crate) const ABANDONED_TO_FLUSHING: u128 = 1 << 47;
    /// At the bound, with none waiting, the abandoned map goes to the
    /// open map though a cut's flush is under way.
    pub(crate) const ABANDONED_TO_OPEN: u128 = 1 << 48;
    /// The cut that waits is replaced at the newer cut's instant, not
    /// when the newer cut's flush completes.
    pub(crate) const REPLACED_AT_INSTANT: u128 = 1 << 49;
    /// A freeze that a death can split: the count moves on before the
    /// cut is recorded.
    pub(crate) const FREEZE_SPLIT: u128 = 1 << 50;
    /// The mark is recorded in the cut state when the uberblock write is
    /// received, before it is whole.
    pub(crate) const MARK_BEFORE_WHOLE: u128 = 1 << 51;
    /// The mark is recorded in the cut state only after the write's
    /// completion went to the transport.
    pub(crate) const MARK_AFTER_COMPLETION: u128 = 1 << 52;
    /// A map's summary is noted after the block's bit, so that a death
    /// between leaves a word that a pass misses.
    pub(crate) const SUMMARY_AFTER_BIT: u128 = 1 << 53;
    /// A map being taken is moved with each word cleared in the smaller
    /// part before it is set in the larger.
    pub(crate) const MOVE_CLEARS_FIRST: u128 = 1 << 54;
    /// A map let go is not cleared, at a release or at a start.
    pub(crate) const MAP_NOT_CLEARED: u128 = 1 << 55;
    /// A cut state begun in another boot is taken up.
    pub(crate) const BOOT_UNCHECKED: u128 = 1 << 56;
    /// A cut state kept under another start token is taken up: the
    /// volume was opened after the instance that kept it. A token has
    /// no order, so this is also the state trusted on the token of a
    /// start older than the last: every state of this volume names one.
    pub(crate) const START_UNCHECKED: u128 = 1 << 62;
    /// An open that holds no cuts leaves the start token as it found it.
    pub(crate) const START_KEPT_WITHOUT_CUTS: u128 = 1 << 1;
    /// An open writes its start token after it removes an acknowledged
    /// failure marker, where it is written before.
    pub(crate) const START_AFTER_ACKNOWLEDGING: u128 = 1 << 63;
    /// A cut state of a volume with another identity is taken up.
    pub(crate) const IDENTITY_UNCHECKED: u128 = 1 << 60;
    /// The start does not sync the copies file before it trusts the
    /// state, or trusts it though the sync failed.
    pub(crate) const COPIES_SYNC_UNCHECKED: u128 = 1 << 57;
    /// A start that acknowledges a failure trusts the cut state.
    pub(crate) const ACKNOWLEDGED_TRUSTED: u128 = 1 << 58;
    /// A start on a created device keeps the mark the state holds.
    pub(crate) const MARK_KEPT_ON_CREATED: u128 = 1 << 59;
    /// A copy that cannot be made fails the change, and so the volume,
    /// instead of letting go the cuts that need it.
    pub(crate) const COPY_FAILURE_FAILS: u128 = 1 << 61;
    /// A write into a region whose bit the disk does not hold begins at
    /// once (M2.3).
    pub(crate) const INTENT_WRITE_UNCOVERED: u128 = 1 << 64;
    /// A cut's bits are cleared before the cut is committed in object
    /// storage (the model test's: the store clears only at a release).
    #[cfg(test)]
    pub(crate) const INTENT_CLEARED_BEFORE_COMMIT: u128 = 1 << 65;
    /// A bit is cleared although its region changed after the
    /// committed cut's instant.
    pub(crate) const INTENT_CLEARED_CHANGED: u128 = 1 << 66;
    /// A start that does not trust the cut state but takes up the map
    /// begins the new state with none of the regions the map names.
    pub(crate) const INTENT_NOT_CARRIED: u128 = 1 << 67;
    /// A map is taken up whatever start token it names.
    pub(crate) const INTENT_TOKEN_UNCHECKED: u128 = 1 << 68;
    /// A bit page whose checksum fails is taken up as it reads.
    pub(crate) const INTENT_TORN_TRUSTED: u128 = 1 << 69;
    /// A page written while a sync of the map runs leaves that sync's
    /// record as it was: a bit the write clears and a later one sets
    /// again is made durable by the sync (D1 of the review of #105).
    pub(crate) const INTENT_SYNC_UNNARROWED: u128 = 1 << 70;
    /// When a sync of the map ends, every request that waits for the
    /// map begins, its bits durable or not.
    pub(crate) const INTENT_RELEASED_UNCHECKED: u128 = 1 << 71;
    /// Setting a bit does not ask for its page to be written: no sync
    /// makes it durable (the review's plant A9 on #105).
    pub(crate) const INTENT_SET_UNWRITTEN: u128 = 1 << 72;

    pub(crate) fn has(self, flag: u128) -> bool {
        self.0 & flag != 0
    }
}

/// Why the second phase of a start ([`Opened::start`]) did not give
/// an in-place store.
#[derive(Debug, thiserror::Error)]
pub enum StartError {
    /// The volume has failed: the home image could not be synced. The
    /// failure marker that says so to the next start is still to write:
    /// [`FailedStart`] writes it when it is finished or dropped, so
    /// that the caller can first say what happened elsewhere.
    #[error("the volume has failed")]
    Failed(FailedStart),
    /// The kernel's cache of the home image could not be dropped after
    /// the start's sync, or could not be shown to be gone: a start
    /// serves only what the disk holds (design §4.1), and refuses the
    /// volume. The volume has not failed.
    #[error("the start could not empty the kernel's cache of the home image: {0}")]
    CacheKept(String),
    /// The I/O the store was asked for ([`IoMode`]) cannot be had: no
    /// direct I/O, an alignment it cannot meet, or no threads. The
    /// volume has not failed.
    #[error("the in-place store's I/O: {0}")]
    Io(String),
    /// The intent map could not be written and synced before the start
    /// serves (M2.3). Nothing was written to the volume: the map the
    /// next start finds names what it named. The volume has not failed.
    #[error("the intent map: {0}")]
    Intent(String),
}

/// How the in-place store reads and writes the home image: by direct
/// I/O (design, decision D6), past the kernel's cache, on I/O threads of
/// its own ([`IoMode::threads`], [`IoMode::DEFAULT_THREADS`] unless
/// asked otherwise). Each thread has a handle of its own on the image
/// and one request at the disk at a time, so that up to that many
/// requests reach the device at once. The start checks the file
/// system's alignment for direct I/O (`STATX_DIOALIGN`) and refuses by
/// reason where it has none or its unit is above a block (4 KiB).
///
/// Test builds can also have the I/O done on the caller's thread, or
/// through the kernel's cache (`IoMode::for_tests`): for a test that
/// needs the cache, or a file system without direct I/O. Neither is a
/// mode the store is served in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IoMode {
    /// Direct I/O; without it, through the cache (test builds only).
    direct: bool,
    /// The I/O threads; 0 (test builds only) does the I/O on the
    /// caller's thread.
    threads: usize,
}

impl Default for IoMode {
    fn default() -> Self {
        Self {
            direct: true,
            threads: Self::DEFAULT_THREADS,
        }
    }
}

impl IoMode {
    /// The I/O threads unless asked otherwise: on machines whose syncs
    /// are slow, eight measured above one in every job that ran both
    /// (design §9). That rests on slow-sync machines only, and the
    /// default may change after a benchmark round on the merged daemon:
    /// this is the one constant to change (the daemon's `--io-threads`
    /// takes its default from it).
    pub const DEFAULT_THREADS: usize = 8;
    /// The most I/O threads the store takes.
    pub const MAX_THREADS: usize = 64;

    /// Direct I/O on `threads` I/O threads, 1 to [`IoMode::MAX_THREADS`];
    /// `None` outside that range.
    pub fn with_threads(threads: usize) -> Option<Self> {
        (1..=Self::MAX_THREADS).contains(&threads).then_some(Self {
            direct: true,
            threads,
        })
    }

    /// The I/O threads.
    pub fn threads(&self) -> usize {
        self.threads
    }

    /// Whether the I/O is direct: always, outside test builds.
    pub fn direct(&self) -> bool {
        self.direct
    }

    /// For tests: through the kernel's cache unless `direct`, and on the
    /// caller's thread where `threads` is 0.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn for_tests(direct: bool, threads: usize) -> Self {
        Self { direct, threads }
    }
}

/// The store on the home image alone: see the
/// [module documentation](self).
pub struct InPlace<D: Dir> {
    /// The volume's directory: where the I/O threads open their handles
    /// on the home image.
    dir: D,
    /// Who is told of the failure first; taken when it is told.
    observer: Option<FailureObserver>,
    home: D::File,
    lock: Option<D::Lock>,
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
    /// How reads and writes are done.
    io: IoMode,
    /// The threads, where [`IoMode::threads`] asks for some.
    pool: Option<Pool>,
    /// The requests on the threads: their byte ranges, and whether each
    /// is a write.
    busy: BTreeMap<RequestId, (u64, u64, bool)>,
    /// Requests that wait, in their order, for one on the threads that
    /// they must not run beside ([`InPlace::must_wait`]). Everything
    /// received after one of them waits behind it.
    deferred: VecDeque<Request>,
    /// The queue's counts and highs; the current figures are those of
    /// `busy` and `deferred`.
    queue: IoQueue,
    /// Writes on the threads acknowledged before a thread wrote them,
    /// under ids of their own on the threads (the client may use theirs
    /// again at once), and how many there were: only under the defect
    /// [`Mutants::ACK_BEFORE_WRITE`]. Such a write's end completes
    /// nothing.
    acked_early: (BTreeSet<RequestId>, u64),
    /// Those writes not yet handed to the threads, by the client's id:
    /// each goes at the first submit, poll or wait after its
    /// acknowledgement was handed out, so that a kill that comes right
    /// after the client took it finds the write not begun, however the
    /// threads are scheduled. Only under the defect.
    held_early: Vec<(RequestId, Job)>,
    /// The failure marker: not yet needed, being written on a thread
    /// of its own, or tried. Whether it was written is not kept: a start
    /// that does not find it goes by the daemon's failure note.
    marker: Marker,
    /// The count of cuts (`cuts.rs`).
    cuts: CutRule,
    /// The cut state (`state.rs`): taken up or begun once the start's
    /// sync has succeeded.
    state: Option<CutState<state::StoreRegion>>,
    /// The copies file, once opened.
    copies: Option<D::File>,
    /// The sync of the copies file for an upload, under way or ended
    /// ([`InPlace::sync_cut_copies`]).
    copies_sync: Option<CopiesSync>,
    /// The intent map and its file, kept beside the cut state (M2.3,
    /// `intent.rs`).
    intent: Option<Intent<D::File>>,
    /// What the start did with the intent map.
    intent_at_start: Option<IntentAtStart>,
    /// The flushes that carry a cut to its flush's completion, with the
    /// cut's number.
    flushing: BTreeMap<RequestId, u64>,
    /// Under [`Mutants::MARK_AFTER_COMPLETION`]: marks recorded only once
    /// the write's completion is handed out.
    marks_after: Vec<(RequestId, State)>,
    /// What the start did with the cut state the region held.
    cuts_at_start: Option<CutsTakenUp>,
    /// The rings the start could not read: each one's offset on the
    /// volume and the error ([`InPlace::rings_unread`]).
    rings_unread: Vec<(u64, String)>,
}

/// Writes the failure marker, on whichever thread runs it.
type MarkerWriter = Box<dyn FnOnce() -> bool + Send>;

/// The failure marker's writer. A thread of its own is started for it
/// when the store is assembled, before the first request, and waits:
/// at the failure it is only told to write. Starting a thread at the
/// moment of failure could wait for memory on the thread that must go
/// on answering (the daemon locks its memory, and a failed disk often
/// leaves the cache full of pages nobody can clean), or fail and leave
/// the write to that thread. Where no thread can be started at
/// assembly, the marker is written on the caller's thread when the
/// volume fails.
enum Marker {
    /// The thread waits for the word to write (`go`); `go` dropped, it
    /// ends without writing.
    Parked {
        go: std::sync::mpsc::Sender<()>,
        thread: std::thread::JoinHandle<bool>,
    },
    /// No thread: written on the caller's thread when needed.
    Inline(MarkerWriter),
    /// The thread was told to write it.
    Writing(std::thread::JoinHandle<bool>),
    Done,
}

impl Marker {
    fn new(write: MarkerWriter, mutants: Mutants) -> Self {
        if mutants.has(Mutants::MARKER_INLINE) {
            return Marker::Inline(write);
        }
        // The writer goes to the thread through a slot, so that it is
        // still here when the thread cannot be started.
        let slot = Arc::new(std::sync::Mutex::new(Some(write)));
        let theirs = slot.clone();
        let (go, word) = std::sync::mpsc::channel::<()>();
        // A small stack: the daemon locks its memory, so a thread's stack
        // stays resident for its life, and the marker's writer needs
        // little (a create, a sync, a sync of the directory).
        let started = std::thread::Builder::new()
            .name("skyzfs-marker".into())
            .stack_size(MARKER_STACK)
            .spawn(move || {
                if word.recv().is_err() {
                    return false;
                }
                let write = theirs
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                write.is_some_and(|write| write())
            });
        match started {
            Ok(thread) => Marker::Parked { go, thread },
            Err(_) => {
                let write = slot
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .take();
                match write {
                    Some(write) => Marker::Inline(write),
                    None => Marker::Done,
                }
            }
        }
    }

    /// Begin to write the marker: on its thread, which is told to, or
    /// here where there is none.
    fn begin(&mut self) {
        match std::mem::replace(self, Marker::Done) {
            Marker::Parked { go, thread } => {
                // The thread only ends after the word or without `go`,
                // so it is there to hear it.
                let _ = go.send(());
                *self = Marker::Writing(thread);
            }
            Marker::Inline(write) => {
                write();
            }
            other => *self = other,
        }
    }

    /// Wait for the marker's write where it was begun, and end the
    /// thread without writing where it was not.
    fn finish(&mut self) {
        match std::mem::replace(self, Marker::Done) {
            Marker::Parked { go, thread } => {
                drop(go);
                let _ = thread.join();
            }
            Marker::Writing(thread) => {
                let _ = thread.join();
            }
            Marker::Inline(_) | Marker::Done => {}
        }
    }
}

/// The marker thread's stack, in bytes.
const MARKER_STACK: usize = 256 << 10;

/// A start of the in-place store that met the failed disk
/// ([`StartError::Failed`]): it holds the volume's lock and the
/// failure marker still to write. The marker needs the disk that has
/// just failed and may take minutes or never end, so the caller first
/// says what happened (the daemon's failure note, the delete of its
/// device), then [`FailedStart::finish`]es it. Dropped, it writes the
/// marker as well. The lock is released after the marker, so that no
/// other start can open the volume between the two.
pub struct FailedStart {
    marker: Marker,
    lock: Option<Box<dyn Send>>,
}

impl FailedStart {
    /// Write the failure marker, wait for it, and release the lock.
    pub fn finish(mut self) {
        self.write();
    }

    fn write(&mut self) {
        self.marker.begin();
        self.marker.finish();
        self.lock = None;
    }
}

impl Drop for FailedStart {
    fn drop(&mut self) {
        self.write();
    }
}

impl std::fmt::Debug for FailedStart {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FailedStart")
    }
}

/// The I/O threads' queue, as the status file shows it
/// ([`InPlace::io_queue`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IoQueue {
    /// The I/O threads; 0 where the I/O is done on the caller's thread
    /// (test builds only).
    pub threads: u64,
    /// Reads and writes on the threads now: handed to a thread, and not
    /// yet taken back by the store.
    pub in_flight: u64,
    /// Requests that wait now, in their order, for one on the threads
    /// that they must not run beside (an overlapping request, or for a
    /// flush a write), and every
    /// request received after the first of them.
    pub waiting: u64,
    /// The requests that waited so since the store began: a count that
    /// only grows.
    pub waited: u64,
    /// The most requests on the threads at once since the store began.
    pub in_flight_max: u64,
    /// The most requests that waited at once since the store began.
    pub waiting_max: u64,
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

/// The store waits for its failure marker when it is dropped, before
/// the lock is released, so that a start after it finds the marker (or
/// the failure to write it); a marker never asked for ends its thread.
impl<D: Dir> InPlace<D> {
    /// What a kill of the process does to the store, for the tests: the
    /// requests no thread has begun, and those that wait in the store,
    /// are dropped undone; the jobs under way on the threads end as the
    /// kernel ends them, and their results are not taken; nothing
    /// completes, nothing fails, and no marker is written. A drop,
    /// instead, does everything submitted before the store goes.
    #[cfg(test)]
    pub(crate) fn abandon(mut self) {
        if let Some(pool) = self.pool.take() {
            drop(pool.take_back());
        }
        self.busy.clear();
        self.deferred.clear();
        self.held_early.clear();
    }
}

impl<D: Dir> Drop for InPlace<D> {
    fn drop(&mut self) {
        // What was submitted is done before the store goes, as it is on
        // the caller's thread: what waits is executed once what it waits
        // for has ended, and a request that waits for the intent map
        // once the map's sync has ended (D2 of the review of #106, and
        // Codex's P2 there). Then the threads end, before the lock is
        // gone.
        loop {
            let for_map = self
                .intent
                .as_ref()
                .is_some_and(|intent| !intent.waiting.is_empty());
            let on_threads = self.pool.is_some() && !self.busy.is_empty();
            if !(on_threads || for_map || !self.deferred.is_empty()) {
                break;
            }
            // Under the defect, a write that waited behind another is
            // held back when it is begun here: hand it over each time.
            self.release_early(true);
            if for_map {
                if self.intent_syncing() {
                    self.settle_intent(true);
                } else if self.failed {
                    self.settle_failed();
                } else {
                    self.begin_intent_sync();
                }
            }
            self.collect(Duration::from_millis(10));
            self.run_deferred();
        }
        self.pool = None;
        self.marker.finish();
    }
}

/// The parts an in-place store is made of ([`Opened::start`]).
pub(crate) struct Parts<D: Dir> {
    pub(crate) dir: D,
    pub(crate) home: D::File,
    pub(crate) lock: Option<D::Lock>,
    pub(crate) geometry: Geometry,
    pub(crate) identity: [u8; 16],
    pub(crate) zeroes: Zeroes,
    /// What writes the failure marker.
    pub(crate) marker: MarkerWriter,
    /// Who is told of the failure first ([`OpenOptions::on_failure`]).
    pub(crate) observer: Option<FailureObserver>,
    /// How reads and writes are done.
    pub(crate) io: IoMode,
    /// The held cuts' region and what the start knows of trusting it.
    pub(crate) cuts: Option<CutsAtStart>,
}

/// What a start did with the cut state its region held
/// ([`InPlace::cuts_at_start`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutsTakenUp {
    /// The state was taken up: the cuts it held are held still.
    TakenUp,
    /// A new state was begun, with no cut held and the whole volume
    /// counted as changed, for the reason given.
    Begun(String),
}

/// What a start did with the intent map ([`InPlace::intent_at_start`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentAtStart {
    /// The map was taken up. It named `named` of the volume's
    /// `regions`; where the cut state was begun anew, only their blocks
    /// count as changed.
    TakenUp {
        /// The regions it named.
        named: u64,
        /// The volume's regions.
        regions: u64,
    },
    /// A new map was begun, with every region named, for the reason
    /// given.
    Begun(String),
}

/// The intent map and its file (`intent.rs`).
struct Intent<F> {
    map: intent::IntentMap,
    file: F,
    /// The syncs of the file on the write path, each with its time from
    /// the first page's write to the sync's end.
    syncs: Spent,
    /// A sync of the file under way on its helper, begun on the write
    /// path: when its first page was written.
    syncing: Option<std::time::Instant>,
    /// The writes and write-zeroes that wait for their regions' bits to
    /// be durable, in the order they came.
    waiting: VecDeque<Waiting>,
    /// How many requests have waited for the map since the start.
    waits: u64,
    /// How many syncs of the file have begun on the write path since
    /// the start.
    begun: u64,
    /// How many commits have cleared bits since the start.
    commits: u64,
}

/// A request that waits for the intent map, with the syncs begun and
/// the commits made when its bits were last set. A sync that begins
/// after that makes them durable, unless a commit clears one first
/// ([`InPlace::intent_synced`]).
struct Waiting {
    request: Request,
    begun: u64,
    commits: u64,
}

impl<F> Intent<F> {
    /// The bytes of each request that waits for the map, as a request
    /// on the threads is shown to [`InPlace::must_wait`]: all of them
    /// write.
    fn waiting_ranges(&self) -> impl Iterator<Item = (u64, u64, bool)> + '_ {
        self.waiting.iter().filter_map(|waiting| {
            mutation_span(&waiting.request.op).map(|(offset, len)| (offset, offset + len, true))
        })
    }
}

/// The bytes a write or a write-zeroes changes, as `(offset, len)`.
fn mutation_span(op: &Op) -> Option<(u64, u64)> {
    match op {
        Op::Write { offset, data } => Some((*offset, data.len() as u64)),
        Op::WriteZeroes { offset, len } => Some((*offset, *len)),
        _ => None,
    }
}

/// The blocks that `len` bytes at `offset` touch, as `(first, count)`.
fn blocks_of(offset: u64, len: u64) -> (u64, u64) {
    let first = offset / skyzfs_blk::BLOCK_SIZE;
    let count = offset.saturating_add(len).div_ceil(skyzfs_blk::BLOCK_SIZE) - first;
    (first, count)
}

/// The intent map's figures, for the status.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct IntentFigures {
    /// The volume's regions.
    pub regions: u64,
    /// The regions named now.
    pub named: u64,
    /// The bytes of a region.
    pub region_bytes: u64,
    /// The syncs of the map on the write path since the start, with
    /// their time: from the first page's write to the sync's end, on
    /// the file's helper. One sync covers every bit set since the last
    /// began (M2.T4's figure).
    pub syncs: Spent,
    /// The writes and write-zeroes that waited for a sync of the map
    /// since the start: those into a region whose bit was not durable.
    pub waits: u64,
}

/// What [`InPlace::read_cut_from`] read of a held cut.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CutSlice {
    /// The blocks read, in order: the `i`-th is in the buffer's `i`-th
    /// block. Empty only where the map has no block from the slice's
    /// first on.
    pub blocks: Vec<u64>,
    /// The block of the cut's map to read from next; `None` once the map
    /// is read to its end.
    pub next: Option<u64>,
    /// Whether a block was read from a copy: the upload syncs the copies
    /// file before it commits a cut it read copies for (M2.4).
    pub copied: bool,
}

/// A sync of the copies file for the upload of a cut, by its number.
#[derive(Debug)]
enum CopiesSync {
    /// Under way on the file's helper.
    UnderWay(u64),
    /// Ended where it began: the file could not begin it on a helper,
    /// or failed at once.
    Ended(u64, io::Result<()>),
}

/// The copies file as the start found it.
enum CopiesAtStart {
    Synced,
    Missing,
    Failed(io::Error),
}

/// What a start that holds cuts takes up ([`InPlace::take_up_cut_state`]).
pub(crate) struct CutsAtStart {
    /// The region as the open had it.
    pub(crate) region: state::Taken,
    /// Whether the start acknowledges a failure.
    pub(crate) acknowledged: bool,
    /// Whether the device was taken over rather than created.
    pub(crate) taken_over: bool,
}

impl<D: Dir> InPlace<D> {
    /// From the parts of an opened volume ([`Opened::start`]): sync the
    /// home image, so that what this start exposes is durable before
    /// the first request, drop the kernel's cache of it, and start the
    /// I/O threads. Where the sync fails, the volume has failed: the
    /// observer is told, and the marker is handed back unwritten with
    /// the lock ([`FailedStart`]).
    pub(crate) fn assemble(
        parts: Parts<D>,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        mutants: Mutants,
    ) -> Result<Self, StartError>
    where
        D::Lock: Send + 'static,
        D::File: Send + 'static,
    {
        let io = parts.io;
        let mut store = Self {
            dir: parts.dir,
            observer: parts.observer,
            home: parts.home,
            lock: parts.lock,
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
            io,
            pool: None,
            busy: BTreeMap::new(),
            deferred: VecDeque::new(),
            queue: IoQueue::default(),
            acked_early: (BTreeSet::new(), 0),
            held_early: Vec::new(),
            marker: Marker::new(parts.marker, mutants),
            cuts: CutRule::new(parts.geometry.size_bytes(), mutants),
            state: None,
            copies: None,
            copies_sync: None,
            intent: None,
            intent_at_start: None,
            flushing: BTreeMap::new(),
            marks_after: Vec::new(),
            cuts_at_start: None,
            rings_unread: Vec::new(),
        };
        // What the instance before this one wrote by direct I/O may be
        // in the device's volatile cache only: this sync makes it
        // durable before this start serves it (point 3 of the module
        // documentation), and costs nothing when nothing is dirty.
        store.counts.syncs += 1;
        if !store.mutants.has(Mutants::START_UNSYNCED) && store.home.sync().is_err() {
            store.failed = true;
            if let Some(observer) = store.observer.take() {
                observer.call();
            }
            let marker = std::mem::replace(&mut store.marker, Marker::Done);
            let lock = store
                .lock
                .take()
                .map(|lock| Box::new(lock) as Box<dyn Send>);
            return Err(StartError::Failed(FailedStart { marker, lock }));
        }
        // Before the cut state and the intent map are touched, so that a
        // start refused for want of direct I/O creates no map (the
        // review of #106).
        store.check_direct_io().map_err(StartError::Io)?;
        if let Some(cuts) = parts.cuts {
            store.take_up_cut_state(cuts)?;
        }
        store.empty_cache().map_err(StartError::CacheKept)?;
        store.read_rings();
        store.set_up_io().map_err(StartError::Io)?;
        Ok(store)
    }

    /// Read the four labels' uberblock rings, so that the count of cuts
    /// knows what an import would choose among before the first request
    /// (design §5.2, "Whether a cut opens as itself"; `cuts.rs`). It is
    /// the store's one read of the volume's content on its own account:
    /// four reads of 128 KiB, 512 KiB in all, on the caller's thread
    /// before the I/O threads start. They come after the cache is
    /// emptied and checked, so they read the disk, and by direct I/O
    /// where the store's I/O is direct, so they leave nothing in the
    /// cache. On a volume that holds no pool they read holes. A ring
    /// that cannot be read stays unknown, and then no cut opens as
    /// itself until every position of that ring has been written again;
    /// the start goes on, as a failed read fails nothing else, and says
    /// which it could not read ([`InPlace::rings_unread`]).
    fn read_rings(&mut self) {
        for (offset, len) in self.cuts.rings() {
            let mut buf = Buf::zeroed((len / skyzfs_blk::BLOCK_SIZE) as usize);
            let read = if self.io.direct {
                self.home.read_direct_at(offset, &mut buf)
            } else {
                self.home.read_at(offset, &mut buf)
            };
            match read {
                Ok(()) => self.cuts.read_at_start(offset, &buf),
                Err(e) => self.rings_unread.push((offset, e.to_string())),
            }
        }
    }

    /// The labels' rings the start could not read, by their offset on
    /// the volume, with the error: until every position of each has
    /// been written again, no cut opens as itself (the start's
    /// `read_rings` says why), and the daemon says so once at the start.
    pub fn rings_unread(&self) -> &[(u64, String)] {
        &self.rings_unread
    }

    /// Check what direct I/O asks, where the store's I/O is direct.
    ///
    /// Direct I/O takes the request's own buffer, which is aligned to a
    /// block ([`Buf::ALIGN`]), at the request's offset, which is a whole
    /// number of blocks: the file system's alignment must divide both.
    fn check_direct_io(&self) -> Result<(), String> {
        if self.io.direct {
            let align = self
                .home
                .direct_align()
                .map_err(|e| format!("the file system offers no direct I/O on it: {e}"))?;
            let block = skyzfs_blk::BLOCK_SIZE;
            let (memory, offset) = (u64::from(align.memory), u64::from(align.offset));
            if offset == 0
                || !block.is_multiple_of(offset)
                || (memory > 0 && !(Buf::ALIGN as u64).is_multiple_of(memory))
            {
                return Err(format!(
                    "direct I/O asks an alignment of {offset} bytes of offsets and {memory} of \
                     memory, which a block of {block} bytes does not meet"
                ));
            }
        }
        Ok(())
    }

    /// Start the I/O threads, if the store has any.
    fn set_up_io(&mut self) -> Result<(), String>
    where
        D::File: Send + 'static,
    {
        if self.io.threads > 0 {
            let files = (0..self.io.threads)
                .map(|_| self.dir.open(crate::store::HOME_NAME))
                .collect::<io::Result<Vec<_>>>()
                .map_err(|e| format!("opening the home image for the threads: {e}"))?;
            let pool = Pool::start(files, self.io.direct, self.wake.clone(), self.mutants)
                .map_err(|e| format!("starting a thread: {e}"))?;
            self.pool = Some(pool);
        }
        Ok(())
    }

    /// Drop the kernel's cache of the home image, and check that it is
    /// gone: from here on every read comes from the disk. A failed sync
    /// can leave pages marked clean that the disk never got, and no
    /// later sync writes them; a start that served them would show
    /// what a cut can still take, behind a flush that does not cover it
    /// (design §4.1, "every start serves only what the disk holds").
    /// That holds whether this start acknowledges a failure or follows
    /// an instance that died between a failed sync and its marker,
    /// whose own sync then succeeds: the kernel reports a writeback
    /// error once. The drop is advisory, so it is tried twice. Once the
    /// count reads 0, it is shown to see this file at all
    /// ([`InPlace::count_sees_the_file`]). Pages that stay, a drop that
    /// fails, a count that does not see the file, or a kernel that
    /// cannot count the file's cached pages refuse the start. Its cost:
    /// the first reads after a start, a take-over included, come from
    /// the disk.
    ///
    /// Under direct I/O the store's own reads and writes leave no page
    /// in the cache: what the drop still guards is what a writer through
    /// the cache left before this start: a file system that serves
    /// direct requests through its cache (see the module documentation,
    /// point 3).
    fn empty_cache(&mut self) -> Result<(), String> {
        let look = !self.mutants.has(Mutants::CACHE_SIGHT_UNCHECKED);
        let look_first = self.mutants.has(Mutants::LOOK_BEFORE_DROP);
        if look && look_first {
            self.count_sees_the_file()?;
        }
        let mut left = 0;
        for _ in 0..2 {
            if !self.mutants.has(Mutants::CACHE_NOT_DROPPED) {
                self.home
                    .drop_cache()
                    .map_err(|e| format!("the drop failed: {e}"))?;
            }
            left = if self.mutants.has(Mutants::CACHE_NOT_CHECKED) {
                0
            } else {
                self.cached_pages()?
            };
            if left == 0 {
                break;
            }
        }
        if left > 0 {
            return Err(format!(
                "{left} pages of it stay in the cache after two drops; something else may \
                 map, lock or read the file"
            ));
        }
        if look && !look_first {
            self.count_sees_the_file()?;
        }
        Ok(())
    }

    /// Show that the count, which has just read 0, sees this file's
    /// pages at all: a block read through the cache must count. On an
    /// overlay file system `cachestat` counts the overlay file's own
    /// mapping, which holds no page (they are the file's below), and
    /// would read 0 whatever the cache holds; a file system without a
    /// page cache would read 0 too, and is refused the same way.
    ///
    /// The look reads block 0, which is a hole on every pool's volume
    /// (the first 8 KiB of the label are never written), so it rests on
    /// a read of a hole leaving a page in the cache. ext4 and XFS leave
    /// one (the rigs' first start of a pool is on an image of holes
    /// only); tmpfs leaves none, and a new volume there is refused here,
    /// as it would be by the drop once it holds data.
    ///
    /// The look comes after the drop, and what it reads comes from the
    /// disk and may stay. Before the drop it made starts refuse: the
    /// read starts the kernel's read-ahead of the blocks after it, and
    /// where the block read is a hole (as the first 8 KiB of a pool's
    /// label are) it is answered at once, while the read-ahead is still
    /// under I/O, and a drop leaves pages under I/O. In the review's
    /// run, 731 of 1500 starts refused on such an image
    /// (`every_start_serves_where_the_image_begins_with_a_hole` in
    /// `tests/inplace_real.rs`).
    ///
    /// After the drop it still left the read-ahead under I/O for the
    /// next start: a start that came right after one (as that test's
    /// do, or a daemon's quick restart) found the pages under I/O and
    /// refused, start 4 of that test's 200 in one CI run. So the look
    /// reads its block alone ([`File::read_alone_at`], with read-ahead
    /// off for the file on Linux), and leaves nothing behind but that
    /// block's page, which the next drop takes
    /// (`a_start_right_after_a_start_serves`).
    fn count_sees_the_file(&mut self) -> Result<(), String> {
        let mut block = vec![0u8; skyzfs_blk::BLOCK_USIZE];
        let read = if self.mutants.has(Mutants::LOOK_READS_AHEAD) {
            self.home.read_at(0, &mut block)
        } else {
            self.home.read_alone_at(0, &mut block)
        };
        read.map_err(|e| format!("reading a block of it failed: {e}"))?;
        if self.cached_pages()? == 0 {
            return Err(
                "the file system's cache cannot be observed through this file: a block \
                 just read through the cache counts 0 pages, so the start cannot show that \
                 the drop left none (through an overlay file system, for example, the count \
                 sees no page)"
                    .into(),
            );
        }
        Ok(())
    }

    /// The home image's pages in the kernel's cache.
    fn cached_pages(&self) -> Result<u64, String> {
        match self.home.cached_pages() {
            Ok(pages) => Ok(pages),
            Err(e) if e.kind() == std::io::ErrorKind::Unsupported => Err(
                "the kernel cannot count a file's cached pages without mapping it \
                     (cachestat, Linux 6.5 and later)"
                    .into(),
            ),
            Err(e) => Err(format!("counting its cached pages failed: {e}")),
        }
    }

    /// The volume's identity ([`crate::volume::Volume::identity`]).
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

    /// How write-zeroes reach the home image: written as zeros by direct
    /// I/O ([`Zeroes::WrittenDirect`]) once one has been applied, and
    /// [`Zeroes::Untried`] until then. Through the cache (test builds
    /// only): deallocated, or written as zeros where the file system
    /// cannot deallocate.
    pub fn zeroes(&self) -> Zeroes {
        self.zeroes
    }

    /// Whether a sync of the home image is under way.
    pub fn syncing(&self) -> bool {
        self.syncing.is_some()
    }

    /// Whether a sync of the intent map is under way on the write path.
    fn intent_syncing(&self) -> bool {
        self.intent
            .as_ref()
            .is_some_and(|intent| intent.syncing.is_some())
    }

    /// Requests submitted and not yet handed out.
    pub fn in_flight(&self) -> usize {
        self.ids.len()
    }

    /// Tell `watcher` of every write applied, every flush's barrier and
    /// every cut counted, from now on ([`Seen`]): for the daemon's
    /// request log.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn watch(&mut self, watcher: Watcher) {
        self.cuts.watch(watcher);
    }

    /// The count of cuts, and the newest cut (design §5.2; `cuts.rs`).
    pub fn cuts(&self) -> Cuts {
        self.cuts.cuts()
    }

    /// The I/O threads' queue: what is on them, what waits for them,
    /// and the counts since the store began.
    pub fn io_queue(&self) -> IoQueue {
        IoQueue {
            threads: self.io.threads as u64,
            in_flight: self.busy.len() as u64,
            waiting: self.deferred.len() as u64,
            ..self.queue
        }
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
    /// marker's thread is told to write the marker for the next start.
    /// The marker needs the disk that has just failed, and a sync on a
    /// sick disk can take minutes or never return; the caller is the
    /// queue's thread, which must go on answering what it holds with
    /// the failure. Where no thread could be started, it is written
    /// here.
    fn fail(&mut self) {
        self.failed = true;
        // Once: the observer is taken when it is told, and a marker
        // begun or written is not begun again.
        if self.mutants.has(Mutants::OBSERVE_AFTER_MARKER) {
            self.marker.begin();
        }
        if let Some(observer) = self.observer.take() {
            observer.call();
        }
        if !self.mutants.has(Mutants::FAIL_UNMARKED) {
            self.marker.begin();
        }
    }

    fn complete(&mut self, id: RequestId, result: Result<(), BlkError>, buf: Option<Buf>) {
        self.cuts.ended(id);
        if let Some(number) = self.flushing.remove(&id) {
            if let Some(state) = &mut self.state {
                if result.is_ok() {
                    state.flushed(number);
                } else {
                    state.flush_failed(number);
                }
            }
        }
        self.ready.push(Completion { id, result, buf });
    }

    /// Every flush under way or waiting fails with the volume, and so
    /// does every request that waits for the intent map.
    fn settle_failed(&mut self) {
        let mut flushes = self.syncing.take().unwrap_or_default();
        flushes.append(&mut self.waiting);
        for id in flushes {
            self.complete(id, Err(BlkError::Failed), None);
        }
        let waiting = self
            .intent
            .as_mut()
            .map(|intent| std::mem::take(&mut intent.waiting))
            .unwrap_or_default();
        for Waiting {
            request: Request { id, op },
            ..
        } in waiting
        {
            let buf = buffer_of(op);
            self.complete(id, Err(BlkError::Failed), buf);
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
                if self.pool.is_some() {
                    let end = offset + buf.len() as u64;
                    return self.on_thread(id, (offset, end, false), Job::Read { id, offset, buf });
                }
                // A failed read changes nothing and does not fail the
                // volume: only its own request fails.
                let read = if self.io.direct {
                    self.home.read_direct_at(offset, &mut buf)
                } else {
                    self.home.read_at(offset, &mut buf)
                };
                let result = match read {
                    Err(_) if self.mutants.has(Mutants::READ_ERROR_FAILS) => {
                        self.fail();
                        self.settle_failed();
                        Err(BlkError::Failed)
                    }
                    read => read.map_err(|e| BlkError::Io(e.kind())),
                };
                self.complete(id, result, Some(buf));
            }
            Op::Flush => self.flush(id),
            op if self.geometry.read_only() => {
                let buf = buffer_of(op);
                self.complete(id, Err(BlkError::ReadOnly), buf);
            }
            Op::Write { offset, data } => {
                if !self.name_in_intent(offset, data.len() as u64) {
                    return self.wait_for_intent(id, Op::Write { offset, data });
                }
                self.write(id, offset, data);
            }
            Op::WriteZeroes { offset, len } => {
                if !self.name_in_intent(offset, len) {
                    return self.wait_for_intent(id, Op::WriteZeroes { offset, len });
                }
                self.write_zeroes(id, offset, len);
            }
            // Rule R6 lets a discarded block keep its content. The
            // discard still counts as a change for the flush behind
            // it: a flush after a client's change always syncs, so it
            // fails on a failed disk whatever the change was (rule R11).
            Op::Discard { .. } => {
                if !self.mutants.has(Mutants::DISCARD_UNCHANGED) {
                    self.unsynced = true;
                }
                // A discard changes no content, and the cut state does
                // not note it (`state.rs`): a cut holds the content.
                self.complete(id, Ok(()), None)
            }
        }
    }

    /// A write whose regions the intent map holds durably (or that the
    /// store keeps no map for): its copies for the held cuts, then the
    /// write itself, on the threads or here.
    fn write(&mut self, id: RequestId, offset: u64, data: Buf) {
        let len = data.len() as u64;
        let after = self.mutants.has(Mutants::COPY_AFTER_OVERWRITE);
        if !after {
            if let Err(e) = self.note_change(offset, len) {
                let result = self.mutated(Err(e));
                return self.complete(id, result, Some(data));
            }
        }
        if self.pool.is_some() {
            let end = offset + len;
            self.on_thread(id, (offset, end, true), Job::Write { id, offset, data });
            if after {
                // The defect: the copy races the write.
                let _ = self.note_change(offset, len);
            }
            return;
        }
        let written = if self.io.direct {
            self.home.write_direct_at(offset, &data)
        } else {
            self.home.write_at(offset, &data)
        };
        if after {
            let _ = self.note_change(offset, len);
        }
        let result = self.mutated(written);
        if result.is_ok() {
            self.cuts.applied(id);
            self.record_cuts();
        }
        self.complete(id, result, Some(data));
    }

    /// A write-zeroes whose regions the intent map holds durably (or
    /// that the store keeps no map for): its copies for the held cuts,
    /// then the zeros, here.
    fn write_zeroes(&mut self, id: RequestId, offset: u64, len: u64) {
        if let Err(e) = self.note_change(offset, len) {
            let result = self.mutated(Err(e));
            return self.complete(id, result, None);
        }
        let zeroed = self.zero(offset, len);
        let result = self.mutated(zeroed);
        if result.is_ok() {
            self.cuts.zeroed(id);
            self.record_cuts();
        }
        self.complete(id, result, None);
    }

    /// Hand `job` to the threads; `range` is its bytes and whether it
    /// writes.
    fn on_thread(&mut self, id: RequestId, range: (u64, u64, bool), job: Job) {
        let mut held = false;
        let (key, job) = match job {
            Job::Write { offset, data, .. } if self.mutants.has(Mutants::ACK_BEFORE_WRITE) => {
                let (early, count) = &mut self.acked_early;
                *count += 1;
                let key = RequestId(u64::MAX - *count);
                early.insert(key);
                self.complete(id, Ok(()), Some(data.clone()));
                held = true;
                (
                    key,
                    Job::Write {
                        id: key,
                        offset,
                        data,
                    },
                )
            }
            job => (id, job),
        };
        self.busy.insert(key, range);
        self.queue.in_flight_max = self.queue.in_flight_max.max(self.busy.len() as u64);
        if held {
            self.held_early.push((id, job));
        } else if let Some(pool) = &self.pool {
            pool.push(job);
        }
    }

    /// Under [`Mutants::ACK_BEFORE_WRITE`], hand to the threads the
    /// early-acknowledged writes whose acknowledgement was handed out
    /// (or, with `all`, every one).
    fn release_early(&mut self, all: bool) {
        if self.held_early.is_empty() {
            return;
        }
        let ready: BTreeSet<RequestId> = self.ready.iter().map(|c| c.id).collect();
        for (client, job) in std::mem::take(&mut self.held_early) {
            if all || !ready.contains(&client) {
                if let Some(pool) = &self.pool {
                    pool.push(job);
                }
            } else {
                self.held_early.push((client, job));
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
    ///
    /// With direct I/O, zeros are written by direct I/O: a deallocated
    /// range would make the next writes to it allocate again. The state
    /// the status shows ([`InPlace::zeroes`]) says so from the first.
    fn zero(&mut self, offset: u64, len: u64) -> io::Result<()> {
        if self.io.direct {
            const CHUNK: u64 = 1 << 20;
            let zeros = Buf::filled((len.min(CHUNK) / skyzfs_blk::BLOCK_SIZE) as usize, 0);
            let mut at = 0;
            while at < len {
                let n = (len - at).min(CHUNK) as usize;
                self.home.write_direct_at(offset + at, &zeros[..n])?;
                at += n as u64;
            }
            self.zeroes = Zeroes::WrittenDirect;
            return Ok(());
        }
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

    /// Flush `id` at its barrier: every request received before it has
    /// taken effect, and none received after it has begun. A cut it
    /// carries is counted here (`cuts.rs`).
    fn flush(&mut self, id: RequestId) {
        self.cuts.barrier(id);
        self.record_cuts();
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

    /// Whether `op` must wait for a request on the threads: a read for
    /// a write it overlaps, a write or a write-zeroes for a read or a
    /// write it overlaps (so that overlapping requests keep the order
    /// they came in), and a flush for every write (a flush covers what
    /// was acknowledged before it; this one also waits for what was
    /// submitted before it, the stricter reading).
    ///
    /// A request that waits for the intent map counts here as a write on
    /// the threads.
    fn must_wait(&self, op: &Op) -> bool {
        let waiting = self
            .intent
            .as_ref()
            .map(|intent| intent.waiting_ranges())
            .into_iter()
            .flatten();
        let mut busy = self.busy.values().copied().chain(waiting).peekable();
        if busy.peek().is_none() {
            return false;
        }
        let (offset, len, write) = match op {
            Op::Read { offset, buf } => (*offset, buf.len() as u64, false),
            Op::Write { offset, data } => (*offset, data.len() as u64, true),
            Op::WriteZeroes { offset, len } => (*offset, *len, true),
            Op::Flush if self.mutants.has(Mutants::FLUSH_PASSES_WRITES) => return false,
            Op::Flush => return busy.any(|(_, _, write)| write),
            Op::Discard { .. } => return false,
        };
        if self.mutants.has(Mutants::OVERLAP_UNORDERED) {
            return false;
        }
        let end = offset.saturating_add(len);
        busy.any(|(from, to, busy_write)| (write || busy_write) && from < end && offset < to)
    }

    /// Execute `request` now, or let it wait behind what waits already
    /// or behind a request on the threads it must not run beside.
    fn accept(&mut self, request: Request) {
        self.cuts.received(request.id, &request.op);
        self.record_cuts();
        let mut behind = !self.deferred.is_empty();
        if self.mutants.has(Mutants::PASSES_WAITING_FLUSH) {
            // The defect: only flushes wait ahead, and they are passed.
            behind &= !self.deferred.iter().all(|r| matches!(r.op, Op::Flush));
        }
        if behind || self.must_wait(&request.op) {
            self.deferred.push_back(request);
            self.queue.waited += 1;
            self.queue.waiting_max = self.queue.waiting_max.max(self.deferred.len() as u64);
        } else {
            self.execute(request.id, request.op);
        }
    }

    /// Take what the threads did (with `timeout`, wait up to it for
    /// something), then execute what no longer has to wait.
    ///
    /// A write's end is a mutation's, as on the caller's thread: it
    /// counts for the next flush, and a failed one fails the volume. A
    /// request that ends after the volume failed fails with it, which
    /// rule R11 allows of one in flight at the failure.
    ///
    /// A thread that died (of a panic) fails the volume as a failed
    /// write does, whatever its job was: what it did to the image is not
    /// known, and fewer threads are left, perhaps none. Once the volume
    /// has failed, the jobs no thread has begun are taken back and fail,
    /// so that none waits for a thread that is gone; the jobs under way
    /// on the threads still alive come back as they end, and fail.
    fn collect(&mut self, timeout: Duration) {
        let Some(pool) = &self.pool else {
            // Nothing is on threads, but a request deferred behind one
            // that waited for the map may go on now: where the volume
            // failed, it fails at once, and not when the map's sync ends
            // (N1 of the next look at #106).
            return self.run_deferred();
        };
        let mut done = Vec::new();
        pool.take(&mut done, timeout);
        let died = pool.died() > 0 && !self.mutants.has(Mutants::DEATH_IGNORED);
        if died && !self.failed {
            self.fail();
            self.settle_failed();
        }
        if self.failed {
            for job in self.pool.as_ref().map(Pool::take_back).unwrap_or_default() {
                let (id, _, buf) = job.into_parts();
                self.busy.remove(&id);
                self.complete(id, Err(BlkError::Failed), Some(buf));
            }
            // Under the defect: the writes held back were answered
            // already, and no thread is to begin them now.
            for (_, job) in std::mem::take(&mut self.held_early) {
                let (key, _, _) = job.into_parts();
                self.busy.remove(&key);
                self.acked_early.0.remove(&key);
            }
        }
        for Done {
            id,
            write,
            result,
            buf,
        } in done
        {
            self.busy.remove(&id);
            if self.acked_early.0.remove(&id) {
                // Acknowledged already, under the defect.
                let _ = self.mutated(result);
                continue;
            }
            let result = if write {
                let result = self.mutated(result);
                if self.failed {
                    Err(BlkError::Failed)
                } else {
                    result
                }
            } else if self.failed {
                Err(BlkError::Failed)
            } else if result.is_err() && self.mutants.has(Mutants::READ_ERROR_FAILS) {
                self.fail();
                self.settle_failed();
                Err(BlkError::Failed)
            } else {
                result.map_err(|e| BlkError::Io(e.kind()))
            };
            if write && result.is_ok() {
                self.cuts.applied(id);
                self.record_cuts();
            }
            self.complete(id, result, Some(buf));
        }
        self.run_deferred();
    }

    /// Execute, in order, the requests that wait and no longer have to.
    fn run_deferred(&mut self) {
        while let Some(front) = self.deferred.front() {
            if self.must_wait(&front.op) {
                break;
            }
            let Some(Request { id, op }) = self.deferred.pop_front() else {
                break;
            };
            self.execute(id, op);
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
        let handed: BTreeSet<RequestId> = self.ready.iter().map(|c| c.id).collect();
        for completion in &self.ready {
            self.ids.remove(&completion.id);
        }
        out.append(&mut self.ready);
        if !self.marks_after.is_empty() {
            // Under the defect: the mark only now.
            for (id, mark) in std::mem::take(&mut self.marks_after) {
                if handed.contains(&id) {
                    if let Some(state) = &mut self.state {
                        state.set_mark(Some(&mark));
                    }
                } else {
                    self.marks_after.push((id, mark));
                }
            }
        }
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
        self.release_early(false);
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
        self.accept(request);
        Ok(())
    }

    /// Hands out what is complete. Looks whether the sync under way has
    /// ended, without waiting for it.
    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        self.release_early(false);
        self.settle_intent(false);
        self.collect(Duration::ZERO);
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
        self.release_early(false);
        self.settle_intent(false);
        self.collect(Duration::ZERO);
        self.settle(false);
        if self.ready.is_empty() && !timeout.is_zero() && self.intent_syncing() {
            // The requests that wait for the map begin when its sync
            // ends.
            self.settle_intent(true);
            self.collect(Duration::ZERO);
        }
        if self.ready.is_empty() && !timeout.is_zero() {
            if self.busy.is_empty() {
                self.settle(true);
            } else {
                self.collect(timeout);
            }
            // Every end of a job completes something, and a wait ends
            // with it, but under the defect, where a write's end
            // completes nothing: wait on, as for a request behind it.
            while self.mutants.has(Mutants::ACK_BEFORE_WRITE) && self.ready.is_empty() {
                // Nothing is ready, so every write held back has had
                // its acknowledgement taken: the threads may have it.
                self.release_early(false);
                if self.busy.is_empty() {
                    self.settle(true);
                    break;
                }
                self.collect(timeout);
            }
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
impl<D: Dir> InPlace<D> {
    /// For the conformance harness's failure (`harness::fail_log`): let
    /// every job on the I/O threads end, take its result, and start
    /// what waited for it, a flush's sync among them. Nothing is handed
    /// out and no sync is looked at, so a sync begun here is under way
    /// when this returns, and the completions wait for the next poll:
    /// as if the threads had been quick and the caller had not looked
    /// since. A write held back from the threads under the plant
    /// ACK_BEFORE_WRITE is handed to them on each turn, as the drop
    /// does: nothing here hands its acknowledgement out.
    ///
    /// # Panics
    ///
    /// When a job is still on the threads after a minute: a thread that
    /// never ends its job is a defect, said here rather than met later
    /// somewhere less telling.
    pub(crate) fn finish_threads(&mut self) {
        let began = std::time::Instant::now();
        while !self.busy.is_empty() {
            assert!(
                began.elapsed() < Duration::from_secs(60),
                "{} jobs did not end on the I/O threads within a minute",
                self.busy.len()
            );
            self.release_early(true);
            self.collect(Duration::from_millis(10));
        }
    }
}

mod cuts;
#[cfg(test)]
pub(crate) mod harness;
pub(crate) mod intent;
mod pool;
mod start;
pub(crate) mod state;
/// The copies file: what the cut state keeps of a block a held cut
/// reads before a write changes it (`state.rs`).
const COPIES_NAME: &str = "copies.img";

/// The copies file, opened or created at its first use.
fn open_copies<'a, D: Dir>(
    dir: &D,
    copies: &'a mut Option<D::File>,
) -> io::Result<&'a mut D::File> {
    if copies.is_none() {
        let file = match dir.open(COPIES_NAME) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => dir.create(COPIES_NAME)?,
            Err(e) => return Err(e),
        };
        *copies = Some(file);
    }
    copies
        .as_mut()
        .ok_or_else(|| io::Error::other("the copies file is not open"))
}

/// Where a block of a held cut is read: a block of the image, or a slot
/// of the copies file.
#[derive(Debug, Clone, Copy)]
enum Place {
    Image(u64),
    Copy(u64),
}

impl Place {
    /// Whether this place is the one after `before` in the same file, so
    /// that one read takes both.
    fn follows(self, before: Place) -> bool {
        match (before, self) {
            (Place::Image(a), Place::Image(b)) | (Place::Copy(a), Place::Copy(b)) => b == a + 1,
            _ => false,
        }
    }
}

/// The cut state's I/O: the home image read as the store reads it, and
/// the copies file written the same way, by direct I/O in the daemon
/// (M2.2c; `read_cut` reads the copies so too).
///
/// A copy written by direct I/O was taken by the device when its write
/// returns, and a failure of it is that write's own error, which lets
/// go the cuts that need the copy there and then (`state.rs`). Through
/// the kernel's cache a write returned before the device took it: a
/// writeback that failed later was reported to no one if the file's
/// inode was evicted while no daemon held the file, and the next read
/// of the copy returned what the disk held (the review of #100, Q1).
/// What the device took, a death of the daemon leaves to the next
/// instance; a reboot may lose it, and the state is trusted in no other
/// boot (M2.2b).
///
/// A copies file that takes no direct I/O, or not at a block's
/// alignment, fails every copy, and each lets go the cuts that need it:
/// no copy falls back to the kernel's cache (coding's reading, M2.2c).
/// The start checks direct I/O on the home image, in the same directory
/// and so on the same file system.
struct Copies<'a, D: Dir> {
    dir: &'a D,
    home: &'a D::File,
    copies: &'a mut Option<D::File>,
    direct: bool,
}

impl<D: Dir> state::Copier for Copies<'_, D> {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        let at = first * skyzfs_blk::BLOCK_SIZE;
        if self.direct {
            self.home.read_direct_at(at, buf)
        } else {
            self.home.read_at(at, buf)
        }
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        let at = u64::from(first) * skyzfs_blk::BLOCK_SIZE;
        let file = open_copies(self.dir, self.copies)?;
        if self.direct {
            file.write_direct_at(at, buf)
        } else {
            file.write_at(at, buf)
        }
    }
}

impl<D: Dir> InPlace<D> {
    /// Take up the cut state the region holds, or begin one where it
    /// holds none this start may trust: once the start's sync has
    /// succeeded, which counts as the flush of every cut whose flush a
    /// death left under way (design §5.2). A store with no region holds
    /// no cut: nothing lets a held cut go before the uploader (M2.4) or
    /// the test-hooks build's commands (M2.2b), and a cut held for ever
    /// would have every first write to a block after it copied, until
    /// the bound (coding's reading, M2.2a).
    ///
    /// The state is trusted only where all of these hold, and otherwise
    /// a new one begins, which counts the whole volume as changed (M2.2b,
    /// as agreed with the lead):
    ///
    /// - the region was there with the volume's length, and holds a
    ///   state of this format, of this volume (its identity: a volume
    ///   made again in the same directory has the same run directory),
    ///   begun in this boot: a reboot or a crash of the machine can lose
    ///   copies the state names, which no daemon death can (the
    ///   daemon's region is on tmpfs, which a reboot empties; the boot
    ///   id guards a region kept elsewhere);
    /// - the copies file was synced by this start, and the sync
    ///   succeeded. A copy is not synced while the volume is served (the
    ///   lead's decision 6). The daemon writes it by direct I/O
    ///   ([`Copies`]), so a copy the device refused failed at its write
    ///   and let its cuts go then, and no page of it waits for a
    ///   writeback. The sync stays for what that does not cover, as the
    ///   start's sync of the image does (design §5.1): a file system
    ///   that takes a direct write through its cache, and the tests'
    ///   I/O through the cache. There a writeback that failed after the
    ///   copy was written would have the next read of it return what the
    ///   disk holds. The kernel reports such a failure to the first sync
    ///   after it only while the file's inode stays in memory: one
    ///   evicted while no daemon held the file loses the error (the
    ///   review of #100, Q1). So this sync narrows that case and does
    ///   not close it; for the daemon's own copies the direct write
    ///   does. A copies file that is missing is as good as synced only
    ///   if the state names no copy;
    /// - the start does not acknowledge a failure: a failed volume's
    ///   state is not one this start can vouch for;
    /// - the volume was not opened since the instance that kept the
    ///   state (the review of #100, D1 to D3). The state is kept under
    ///   the start token of the open its instance served under, and
    ///   every open writes a new token before it changes anything else
    ///   ([`crate::volume::START_NAME`]). Without that rule a start that
    ///   held no cuts, or held them in another run directory, could
    ///   write blocks a held cut reads with no copy made; and a start
    ///   lost after its open took the failure marker away, or after its
    ///   sync heard of a failed writeback of the copies, would leave a
    ///   state the next start trusts. The new token is recorded in the
    ///   state only once it is trusted, before the start finishes what
    ///   a death interrupted, or when a new state begins.
    ///
    /// A state taken up on a device this instance created, not took
    /// over, keeps no mark (coding's reading, M2.2b, agreed with the
    /// lead): a mark is cut at the flush that follows it, and the
    /// client that wrote the marked uberblock was the old device's,
    /// which can send no flush to this one; whatever writes the pool
    /// next on a created device marks afresh.
    ///
    /// The intent map (M2.3, `intent.rs`) is read beside it and taken up
    /// by its own rules. Where the cut state is begun anew and the map
    /// is taken up, only the blocks of the regions the map names count
    /// as changed in the new state, not the whole volume. Either way the
    /// map is then kept under this start's token, or begun anew with
    /// every region named, and written and synced before the start
    /// serves; a start that cannot do that is refused
    /// ([`StartError::Intent`]).
    fn take_up_cut_state(&mut self, cuts: CutsAtStart) -> Result<(), StartError> {
        let CutsAtStart {
            region:
                state::Taken {
                    region,
                    kept,
                    found_version,
                    owner,
                    start,
                    bound,
                    intent_region_blocks,
                },
            acknowledged,
            taken_over,
        } = cuts;
        let size = self.geometry.size_bytes();
        let shape = intent::Shape {
            identity: owner.volume,
            region_blocks: intent_region_blocks,
            blocks: size / skyzfs_blk::BLOCK_SIZE,
        };
        let (found_map, map_file) = self.read_intent(shape);
        let found_map = found_map.and_then(|map| {
            map.trust(Some(owner.start))?;
            Ok(map)
        });
        let copies = self.sync_copies();
        let trusted = if acknowledged && !self.mutants.has(Mutants::ACKNOWLEDGED_TRUSTED) {
            Err(("the start acknowledges a failure".to_owned(), region))
        } else if !kept {
            let why = match found_version {
                Some(version) if version != state::VERSION => {
                    state::RegionError::Version(version).to_string()
                }
                _ => "the region was not there with the volume's length".to_owned(),
            };
            Err((why, region))
        } else {
            // Read as it is: nothing is stored until it is trusted.
            match CutState::view(region, size, bound, owner, self.mutants) {
                Err((e, region)) => Err((e.to_string(), region)),
                Ok(cut_state) => match copies {
                    CopiesAtStart::Synced => Ok(cut_state),
                    CopiesAtStart::Missing if cut_state.figures().copied == 0 => Ok(cut_state),
                    CopiesAtStart::Missing => Err((
                        "the copies file is missing, and the state names copies".to_owned(),
                        cut_state.into_region(),
                    )),
                    CopiesAtStart::Failed(e) => Err((
                        format!("the copies file could not be synced: {e}"),
                        cut_state.into_region(),
                    )),
                },
            }
        };
        let cut_state = match trusted {
            Ok(cut_state) => {
                let mut cut_state = cut_state.take_up(start);
                if !taken_over
                    && cut_state.mark().is_some()
                    && !self.mutants.has(Mutants::MARK_KEPT_ON_CREATED)
                {
                    cut_state.set_mark(None);
                }
                self.cuts_at_start = Some(CutsTakenUp::TakenUp);
                cut_state
            }
            Err((why, region)) => {
                self.cuts_at_start = Some(CutsTakenUp::Begun(why));
                // The regions the map names, and no others, may differ
                // from the newest committed cut (design §5.2).
                let changed: Option<Vec<(u64, u64)>> = found_map.as_ref().ok().map(|map| {
                    if self.mutants.has(Mutants::INTENT_NOT_CARRIED) {
                        return Vec::new();
                    }
                    map.named()
                        .into_iter()
                        .map(|region| shape.blocks_of(region))
                        .collect()
                });
                CutState::begin_with(
                    region,
                    size,
                    bound,
                    state::Owner { start, ..owner },
                    self.mutants,
                    changed.as_deref(),
                )
            }
        };
        self.cuts
            .restore(cut_state.count(), cut_state.newest_cut(), cut_state.mark());
        self.state = Some(cut_state);
        self.keep_intent(shape, start, found_map, map_file)
            .map_err(|e| StartError::Intent(e.to_string()))
    }

    /// The intent map this start finds, and its file: `None` where
    /// there is none to read.
    fn read_intent(
        &self,
        shape: intent::Shape,
    ) -> (Result<intent::IntentMap, intent::Distrust>, Option<D::File>) {
        let file = match self.dir.open(intent::INTENT_NAME) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return (Err(intent::Distrust::Missing), None);
            }
            Err(e) => return (Err(intent::Distrust::Unreadable(e.to_string())), None),
        };
        let read = || -> Result<intent::IntentMap, intent::Distrust> {
            let unreadable = |e: io::Error| intent::Distrust::Unreadable(e.to_string());
            if file.len().map_err(unreadable)? != shape.file_len() {
                return Err(intent::Distrust::Shape);
            }
            let pages = (shape.file_len() / intent::PAGE as u64) as usize;
            let mut bytes = skyzfs_blk::Buf::zeroed(pages);
            if self.io.direct {
                file.read_direct_at(0, &mut bytes)
            } else {
                file.read_at(0, &mut bytes)
            }
            .map_err(unreadable)?;
            intent::IntentMap::read(&bytes, shape, self.mutants)
        };
        (read(), Some(file))
    }

    /// Keep the intent map from this start on: the one found, under
    /// this start's token `start`, or a new one with every region named
    /// in a new file. Its pages are written and synced before the start
    /// serves.
    fn keep_intent(
        &mut self,
        shape: intent::Shape,
        start: [u8; 16],
        found: Result<intent::IntentMap, intent::Distrust>,
        file: Option<D::File>,
    ) -> io::Result<()> {
        let (map, file) = match (found, file) {
            (Ok(mut map), Some(file)) => {
                self.intent_at_start = Some(IntentAtStart::TakenUp {
                    named: map.named_count(),
                    regions: shape.regions(),
                });
                map.kept_by(start);
                (map, file)
            }
            (found, file) => {
                let why = match found {
                    Err(why) => why.to_string(),
                    Ok(_) => "the intent map could not be opened".to_owned(),
                };
                self.intent_at_start = Some(IntentAtStart::Begun(why));
                drop(file);
                match self.dir.remove(intent::INTENT_NAME) {
                    Ok(()) => {}
                    Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e),
                }
                let file = self.dir.create(intent::INTENT_NAME)?;
                // The new file's name is durable only once the directory
                // is synced (D1 of the review of #106): without it a
                // power cut can leave no map, and the next cut is the
                // whole volume.
                self.dir.sync()?;
                (intent::IntentMap::new(shape, start, self.mutants), file)
            }
        };
        self.intent = Some(Intent {
            map,
            file,
            syncs: Spent::default(),
            syncing: None,
            waiting: VecDeque::new(),
            begun: 0,
            commits: 0,
            waits: 0,
        });
        self.write_intent(true)
    }

    /// Write the intent map's pages that are to be written, by direct
    /// I/O where the store's I/O is direct, and with `sync` sync the
    /// file after them.
    fn write_intent(&mut self, sync: bool) -> io::Result<()> {
        let direct = self.io.direct;
        let Some(intent) = &mut self.intent else {
            return Ok(());
        };
        for (at, image) in intent.map.write_begin() {
            if direct {
                intent.file.write_direct_at(at, &image)?;
            } else {
                intent.file.write_at(at, &image)?;
            }
        }
        if sync {
            intent.map.sync_begin();
            let synced = intent.file.sync();
            intent.map.synced(synced.is_ok());
            synced?;
        }
        Ok(())
    }

    /// Whether a write or a write-zeroes of `len` bytes at `offset` may
    /// begin now: the store keeps no intent map, or the map holds the
    /// bits of every region it touches durably. Where it does not, the
    /// bits are set here, and the request is to wait for the sync of
    /// the map that covers them ([`InPlace::wait_for_intent`]).
    fn name_in_intent(&mut self, offset: u64, len: u64) -> bool {
        let Some(intent) = &mut self.intent else {
            return true;
        };
        let (first, count) = blocks_of(offset, len);
        if intent.map.covers(first, count) {
            return true;
        }
        intent.map.set(first, count);
        // True where a bit cleared and not yet written is set again.
        intent.map.covers(first, count)
    }

    /// Let a write or a write-zeroes whose regions' bits are not durable
    /// wait for the sync of the map that makes them so, and begin that
    /// sync if none is under way. Only the requests that must not run
    /// beside it wait with it ([`InPlace::must_wait`]): one that
    /// overlaps it, and a flush.
    ///
    /// The sync is on the map's file's helper, as the image's are, and
    /// not on the queue's thread (the lead's ruling on M2.3b): a sync
    /// there would hold every request of the device. One sync covers
    /// every bit set since the one before it began, so the requests
    /// that come into clean regions while a sync is under way wait for
    /// the next one, together. The pages are written here, by direct
    /// I/O: a page of 4 KiB at a time, as a copy for a held cut is.
    fn wait_for_intent(&mut self, id: RequestId, op: Op) {
        let Some(intent) = &mut self.intent else {
            return self.resume(id, op);
        };
        intent.waits += 1;
        intent.waiting.push_back(Waiting {
            request: Request { id, op },
            begun: intent.begun,
            commits: intent.commits,
        });
        if intent.syncing.is_none() {
            self.begin_intent_sync();
        }
    }

    /// Write the map's pages that are to be written and begin a sync of
    /// its file on the helper. A failure fails the volume, and with it
    /// every request that waits for the map: a write without its bit
    /// would leave a region the map does not name, and the bit's write
    /// cannot be known (the lead's answer (a) to question 1 on M2.3).
    /// Where the file cannot begin a sync on a helper, the sync is made
    /// here.
    fn begin_intent_sync(&mut self) {
        let direct = self.io.direct;
        let wake = self.wake.clone();
        let Some(intent) = &mut self.intent else {
            return;
        };
        intent.syncing = Some(std::time::Instant::now());
        intent.begun += 1;
        let written = intent
            .map
            .write_begin()
            .into_iter()
            .try_for_each(|(at, image)| {
                if direct {
                    intent.file.write_direct_at(at, &image)
                } else {
                    intent.file.write_at(at, &image)
                }
            });
        let begun = written.and_then(|()| {
            intent.map.sync_begin();
            intent.file.sync_begin(Box::new(move || {
                if let Some(wake) = wake {
                    wake();
                }
            }))
        });
        match begun {
            Ok(()) => {}
            Err(e) if NotBegun::is(&e) => {
                let synced = intent.file.sync();
                self.intent_synced(synced);
            }
            Err(e) => self.intent_synced(Err(e)),
        }
    }

    /// See whether the map's sync under way has ended; with `wait`,
    /// wait for it.
    fn settle_intent(&mut self, wait: bool) {
        let Some(intent) = &mut self.intent else {
            return;
        };
        if intent.syncing.is_none() {
            return;
        }
        if let Some(result) = intent.file.sync_end(wait) {
            self.intent_synced(result);
        }
    }

    /// The map's sync under way has ended with `result`. Where it
    /// succeeded, the requests whose regions it made durable begin, in
    /// the order they came. One whose bit is not durable yet waits for
    /// the next sync, which begins here: its bit was set after this
    /// sync began, or a clear after a commit took it away while it
    /// waited (`intent_committed`), and it is set again, for the write
    /// has not changed the region yet.
    ///
    /// A bit set before this sync began, and not cleared by a commit
    /// since, is durable when it ends: where one is not, the map is
    /// wrong, and the volume fails. Setting it again and syncing would
    /// go on for ever (the addendum to the review of #106).
    fn intent_synced(&mut self, result: io::Result<()>) {
        let unchecked = self.mutants.has(Mutants::INTENT_RELEASED_UNCHECKED);
        let Some(intent) = &mut self.intent else {
            return;
        };
        intent.map.synced(result.is_ok());
        if let Some(began) = intent.syncing.take() {
            intent.syncs.add(began.elapsed());
        }
        if result.is_err() {
            if !self.failed {
                self.fail();
            }
            self.settle_failed();
            return self.run_deferred();
        }
        let mut ready = Vec::new();
        let mut lost = false;
        for waiting in std::mem::take(&mut intent.waiting) {
            let Some((offset, len)) = mutation_span(&waiting.request.op) else {
                ready.push(waiting.request);
                continue;
            };
            let (first, count) = blocks_of(offset, len);
            if intent.map.covers(first, count) || unchecked {
                ready.push(waiting.request);
            } else {
                lost |= waiting.begun < intent.begun && waiting.commits == intent.commits;
                intent.map.set(first, count);
                intent.waiting.push_back(Waiting {
                    begun: intent.begun,
                    commits: intent.commits,
                    ..waiting
                });
            }
        }
        if lost {
            if !self.failed {
                self.fail();
            }
            self.settle_failed();
        }
        for Request { id, op } in ready {
            self.resume(id, op);
        }
        let next = self
            .intent
            .as_ref()
            .is_some_and(|intent| !intent.waiting.is_empty() && intent.syncing.is_none());
        if next && !self.failed {
            self.begin_intent_sync();
        }
        self.run_deferred();
    }

    /// A write or a write-zeroes that waited for the map goes on, unless
    /// the volume failed meanwhile.
    fn resume(&mut self, id: RequestId, op: Op) {
        if self.failed {
            let buf = buffer_of(op);
            return self.complete(id, Err(BlkError::Failed), buf);
        }
        match op {
            Op::Write { offset, data } => self.write(id, offset, data),
            Op::WriteZeroes { offset, len } => self.write_zeroes(id, offset, len),
            op => self.execute(id, op),
        }
    }

    /// What the start did with the intent map, if the store holds cuts.
    pub fn intent_at_start(&self) -> Option<&IntentAtStart> {
        self.intent_at_start.as_ref()
    }

    /// The intent map's figures, if the store holds cuts.
    pub fn intent_figures(&self) -> Option<IntentFigures> {
        self.intent.as_ref().map(|intent| {
            let shape = intent.map.shape();
            IntentFigures {
                regions: shape.regions(),
                named: intent.map.named_count(),
                region_bytes: shape.region_blocks * skyzfs_blk::BLOCK_SIZE,
                syncs: intent.syncs,
                waits: intent.waits,
            }
        })
    }

    /// Sync the copies file, if there is one, as the start does before
    /// it trusts the cut state.
    fn sync_copies(&mut self) -> CopiesAtStart {
        if self.mutants.has(Mutants::COPIES_SYNC_UNCHECKED) {
            return CopiesAtStart::Synced;
        }
        match self.dir.open(COPIES_NAME) {
            Ok(mut file) => {
                let synced = file.sync();
                self.copies = Some(file);
                match synced {
                    Ok(()) => CopiesAtStart::Synced,
                    Err(e) => CopiesAtStart::Failed(e),
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => CopiesAtStart::Missing,
            Err(e) => CopiesAtStart::Failed(e),
        }
    }

    /// What the start did with the cut state the region held, if the
    /// store holds cuts.
    pub fn cuts_at_start(&self) -> Option<&CutsTakenUp> {
        self.cuts_at_start.as_ref()
    }

    /// The conformance harness's mark: as if an uberblock write naming
    /// `state` had just been applied, so that the next flush received
    /// cuts it. Recorded in the cut state at once (`harness.rs`).
    #[cfg(test)]
    pub(crate) fn mark_as_applied(&mut self, state: State) {
        self.cuts.applied_state(RequestId(u64::MAX), state);
        self.record_cuts();
    }

    /// Record in the cut state what the rule has just done: a mark, or a
    /// cut at its flush's barrier. Called before anything the rule saw
    /// is completed.
    fn record_cuts(&mut self) {
        for event in self.cuts.take_events() {
            let Some(state) = &mut self.state else {
                continue;
            };
            match event {
                cuts::Event::Mark(id, mark) => {
                    if self.mutants.has(Mutants::MARK_AFTER_COMPLETION) {
                        self.marks_after.push((id, mark));
                    } else {
                        state.set_mark(Some(&mark));
                    }
                }
                cuts::Event::Cut(id, cut, marked) => {
                    let number = state.freeze(&marked, cut.itself);
                    self.flushing.insert(id, number);
                }
            }
        }
    }

    /// The bytes `offset` to `offset + len` are about to be written or
    /// zeroed: the cut state keeps what a held cut reads of them first.
    /// The change begins only after this returns.
    fn note_change(&mut self, offset: u64, len: u64) -> io::Result<()> {
        let InPlace {
            state,
            dir,
            home,
            copies,
            io,
            ..
        } = self;
        let Some(state) = state else {
            return Ok(());
        };
        let first = offset / skyzfs_blk::BLOCK_SIZE;
        let end = (offset + len).div_ceil(skyzfs_blk::BLOCK_SIZE);
        let mut files = Copies {
            dir: &*dir,
            home: &*home,
            copies,
            direct: io.direct,
        };
        state.change(first, end - first, &mut files)
    }

    /// The held cuts, oldest first.
    pub fn held_cuts(&self) -> Vec<HeldCut> {
        self.state.as_ref().map(CutState::held).unwrap_or_default()
    }

    /// The cut state's name (`CutState::id`), which the uploader puts in
    /// a cut's row with its number and group: the numbers of cuts begin
    /// again with each state. `None` for a store that holds no cut.
    pub fn cut_state_id(&self) -> Option<[u8; 16]> {
        self.state.as_ref().map(CutState::id)
    }

    /// The newest cut committed in object storage, as the intent map
    /// names it: the base the held cuts were made against (M2.4: the
    /// uploader's assumed base), which needs no object store to know.
    /// `None` for a map that names none (one begun anew, before its
    /// first commit) or a store that holds no cut.
    pub fn committed_cut(&self) -> Option<CommittedCut> {
        self.intent.as_ref().and_then(|intent| intent.map.base())
    }

    /// Whether held cut `number` holds the whole volume, so that its
    /// upload needs no base: the first cut of a state begun with
    /// nothing known, the first after [`InPlace::abandon_all_cuts`], or
    /// a cut that took such a cut's map.
    pub fn cut_is_whole(&self, number: u64) -> bool {
        self.state
            .as_ref()
            .is_some_and(|state| state.is_whole_cut(number))
    }

    /// The row in object storage is not what the held cuts were made
    /// against (M2.4, the uploader's check): every held cut is
    /// abandoned, each counted among the cuts abandoned, their copies
    /// let go, and the next cut holds the whole volume. The numbers of
    /// the cuts abandoned, oldest first. The intent map is left as it
    /// is: nothing new was committed, and a start that does not take up
    /// the cut state counts what the map names against a base the
    /// uploader's check will find again that the row does not match.
    pub fn abandon_all_cuts(&mut self) -> Vec<u64> {
        self.state
            .as_mut()
            .map(CutState::abandon_all)
            .unwrap_or_default()
    }

    /// The cut state's figures, for the status: all 0 for a store that
    /// holds no cut.
    pub fn cut_figures(&self) -> CutFigures {
        self.state
            .as_ref()
            .map(CutState::figures)
            .unwrap_or_default()
    }

    /// The blocks in held cut `number`'s map.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn cut_map(&self, number: u64) -> Vec<u64> {
        self.state
            .as_ref()
            .map(|state| state.map(number))
            .unwrap_or_default()
    }

    /// Read block `block` of held cut `number` as it was at the cut's
    /// instant into `buf`, one block: whether it is in the cut's map
    /// (with `false`, `buf` is untouched and the image the cut is
    /// uploaded onto holds the block). Beside the write path: a block a
    /// write has changed since the instant is read from its copy, which
    /// was whole before the write began. The image and the copies are
    /// read by direct I/O where the store's I/O is direct, as they were
    /// written, so that no page of the copies file is left in the
    /// kernel's cache (M2.2c). A block past the volume's end, or a
    /// buffer of another size than a block, is refused.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn read_cut(&mut self, number: u64, block: u64, buf: &mut [u8]) -> io::Result<bool> {
        if block >= self.geometry.size_bytes() / skyzfs_blk::BLOCK_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the block lies past the volume's end",
            ));
        }
        if buf.len() != skyzfs_blk::BLOCK_USIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a cut is read one block at a time",
            ));
        }
        let read = self
            .state
            .as_ref()
            .and_then(|state| state.read(number, block))
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such held cut"))?;
        match read {
            state::Read::Unchanged => Ok(false),
            state::Read::Image => {
                let at = block * skyzfs_blk::BLOCK_SIZE;
                if self.io.direct {
                    self.home.read_direct_at(at, buf)?;
                } else {
                    self.home.read_at(at, buf)?;
                }
                Ok(true)
            }
            state::Read::Copy(slot) => {
                let at = u64::from(slot) * skyzfs_blk::BLOCK_SIZE;
                let file = open_copies(&self.dir, &mut self.copies)?;
                if self.io.direct {
                    file.read_direct_at(at, buf)?;
                } else {
                    file.read_at(at, buf)?;
                }
                Ok(true)
            }
        }
    }

    /// Read the blocks of held cut `number`'s map from block `from` on,
    /// as they were at the cut's instant, as many as `buf` has whole
    /// blocks: what the upload reads, a slice a turn of the queue's
    /// thread (M2.4). Each block reads as [`InPlace::read_cut`] reads it,
    /// and each run of blocks the image holds, or of copies in adjacent
    /// slots, in one read. A cut that is not held, and a buffer of no
    /// block, are refused. The buffer is a [`Buf`], whole blocks aligned
    /// for direct I/O, so that a caller's buffer cannot make a read fail
    /// and abandon the cut being uploaded (review of M2.4b-1, N1).
    ///
    /// Slices of the cut being uploaded hold the cut whatever the queue
    /// does between them: that cut is the oldest held, so its map does
    /// not change while it is held, and each block is read from its copy
    /// once a write has changed it. A cut that goes between two slices
    /// (abandoned at the bound, or let go at a copy that could not be
    /// made) is refused at the next. A read that fails abandons the cut
    /// if it is the one being uploaded (M2.4): its map goes to the cut
    /// that waits, which reads those blocks again.
    pub fn read_cut_from(&mut self, number: u64, from: u64, buf: &mut Buf) -> io::Result<CutSlice> {
        let block_bytes = skyzfs_blk::BLOCK_USIZE;
        if buf.blocks() == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a cut is read into a buffer of at least one block",
            ));
        }
        let not_held = || io::Error::new(io::ErrorKind::NotFound, "no such held cut");
        let state = self.state.as_ref().ok_or_else(not_held)?;
        let (blocks, next) = state
            .map_from(number, from, buf.len() / block_bytes)
            .ok_or_else(not_held)?;
        let reads = blocks
            .iter()
            .map(|&block| match state.read(number, block) {
                Some(state::Read::Image) => Ok(Place::Image(block)),
                Some(state::Read::Copy(slot)) => Ok(Place::Copy(u64::from(slot))),
                // Not reached: the block is in the cut's map. An upload
                // that read the image the cut lies on would be wrong.
                Some(state::Read::Unchanged) | None => Err(io::Error::other(
                    "a block of the cut's map reads as not in it",
                )),
            })
            .collect::<io::Result<Vec<Place>>>()?;
        let copied = reads.iter().any(|read| matches!(read, Place::Copy(_)));
        if let Err(e) = self.read_places(&reads, buf) {
            if self.uploading(number) {
                if let Some(state) = self.state.as_mut() {
                    state.abandon_uploading(number);
                }
            }
            return Err(e);
        }
        Ok(CutSlice {
            blocks,
            next,
            copied,
        })
    }

    /// Read the blocks at `places` into `buf`, a block each, in order:
    /// each run of the image, or of adjacent slots of the copies file,
    /// in one read.
    fn read_places(&mut self, places: &[Place], buf: &mut [u8]) -> io::Result<()> {
        let block_bytes = skyzfs_blk::BLOCK_USIZE;
        let mut i = 0;
        while i < places.len() {
            let mut end = i + 1;
            while end < places.len() && places[end].follows(places[end - 1]) {
                end += 1;
            }
            let into = &mut buf[i * block_bytes..end * block_bytes];
            let (file, at) = match places[i] {
                Place::Image(block) => (&mut self.home, block),
                Place::Copy(slot) => (open_copies(&self.dir, &mut self.copies)?, slot),
            };
            let at = at * skyzfs_blk::BLOCK_SIZE;
            if self.io.direct {
                file.read_direct_at(at, into)?;
            } else {
                file.read_at(at, into)?;
            }
            i = end;
        }
        Ok(())
    }

    /// Whether held cut `number` is the one being uploaded.
    fn uploading(&self, number: u64) -> bool {
        self.state.as_ref().is_some_and(|state| {
            state
                .held()
                .first()
                .is_some_and(|cut| cut.number == number && cut.phase == state::Phase::Uploading)
        })
    }

    /// Begin a sync of the copies file before held cut `number`, the one
    /// being uploaded, is committed in object storage, where the upload
    /// read a copy for it ([`CutSlice::copied`]): the upload commits a
    /// cut it read copies for only once they are durable, and abandons
    /// it if the sync fails (M2.4, the review of M2.2b-1). A copy read
    /// through the kernel's cache, as test builds can, is the page in
    /// the cache, and a writeback that failed can leave a disk that does
    /// not hold it; the sync says so. On the file's helper, as the
    /// image's syncs are, so that the queue goes on meanwhile;
    /// [`InPlace::cut_copies_synced`] is its end. Refused where a sync
    /// for an upload is under way or has not been asked for its end,
    /// and where cut `number` is not the one being uploaded.
    pub fn sync_cut_copies(&mut self, number: u64) -> io::Result<()> {
        if self.copies_sync.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a sync of the copies for an upload is under way",
            ));
        }
        if !self.uploading(number) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the cut is not the one being uploaded",
            ));
        }
        let wake = self.wake.clone();
        let file = open_copies(&self.dir, &mut self.copies)?;
        let begun = file.sync_begin(Box::new(move || {
            if let Some(wake) = wake {
                wake();
            }
        }));
        self.copies_sync = Some(match begun {
            Ok(()) => CopiesSync::UnderWay(number),
            Err(e) if NotBegun::is(&e) => CopiesSync::Ended(number, file.sync()),
            Err(e) => CopiesSync::Ended(number, Err(e)),
        });
        Ok(())
    }

    /// The end of the sync that [`InPlace::sync_cut_copies`] began:
    /// `None` while it is under way, which only a call with `wait` false
    /// can answer, and where none was begun; else its outcome. Where it
    /// failed, its cut is abandoned if it is still the one being
    /// uploaded: its map goes to the cut that waits, which uploads its
    /// blocks. The file's helper wakes the store's caller when the sync
    /// ends, as for the image's syncs.
    pub fn cut_copies_synced(&mut self, wait: bool) -> Option<io::Result<()>> {
        let (number, result) = match self.copies_sync.take()? {
            CopiesSync::Ended(number, result) => (number, result),
            CopiesSync::UnderWay(number) => {
                let ended = match self.copies.as_mut() {
                    Some(file) => file.sync_end(wait),
                    None => Some(Err(io::Error::other("the copies file is not open"))),
                };
                let Some(result) = ended else {
                    self.copies_sync = Some(CopiesSync::UnderWay(number));
                    return None;
                };
                (number, result)
            }
        };
        if result.is_err() && self.uploading(number) {
            if let Some(state) = self.state.as_mut() {
                state.abandon_uploading(number);
            }
        }
        Some(result)
    }

    /// Held cut `number`, the one being uploaded, is committed in object
    /// storage: let it go. Whether it was that cut; false changes
    /// nothing. Called only after the commit (M2.4); the test-hooks
    /// build's `release` command calls it without one.
    ///
    /// After it the intent map names the cut as its base (the cut
    /// state's id, the cut's number and its group), and clears the bits
    /// of the regions in which nothing changed after the cut's instant
    /// (`intent.rs`). The pages are written without a sync; one that
    /// cannot be written fails the volume, as on the write path, and the
    /// cut stays held (review of M2.4b-1).
    ///
    /// The intent map names the cut before the cut state lets it go, so
    /// that a death between, or a failed write of the map's pages, leaves
    /// the cut held for the uploader's check to find in the row, and
    /// never a cut gone that the map does not name: the regions it clears
    /// are those in which nothing changed after the cut's instant, the
    /// same whether the cut is held or not.
    pub fn release_cut(&mut self, number: u64) -> bool {
        let Some(group) = self
            .held_cuts()
            .into_iter()
            .find(|cut| cut.number == number && cut.phase == CutPhase::Uploading)
            .map(|cut| cut.state.txg)
        else {
            return false;
        };
        if !self.intent_committed(number, group) {
            // The volume has failed, and the cut stays held for the next
            // start's check, which finds it in the row.
            return true;
        }
        self.state
            .as_mut()
            .is_some_and(|state| state.release(number))
    }

    /// Cut `number` of group `group` is committed in object storage:
    /// see [`InPlace::release_cut`]. False where the map's pages could
    /// not be written, and the volume has failed.
    fn intent_committed(&mut self, number: u64, group: u64) -> bool {
        let Some(state) = self.state.as_ref() else {
            return true;
        };
        let changed = state.changed_after(number);
        let id = state.id();
        let Some(intent) = &mut self.intent else {
            return true;
        };
        let region_blocks = intent.map.shape().region_blocks;
        let changed: Option<BTreeSet<u64>> = changed.map(|words| {
            words
                .iter()
                .map(|&word| word as u64 * 64 / region_blocks)
                .collect()
        });
        intent.map.commit(intent::Base {
            state: id,
            number,
            group,
        });
        intent.commits += 1;
        intent.map.clear(|first, _| {
            changed
                .as_ref()
                .is_none_or(|regions| regions.contains(&(first / region_blocks)))
        });
        if self.write_intent(false).is_err() {
            self.fail();
            self.settle_failed();
            return false;
        }
        true
    }
}

#[cfg(test)]
mod tests;

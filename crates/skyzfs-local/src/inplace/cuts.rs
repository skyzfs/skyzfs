//! The count of cuts (design §5.2; work package M2.1): the mark of an
//! applied uberblock write, and the cut at the barrier of the first
//! flush received after it.
//!
//! # The rule, as the store keeps it
//!
//! - **An uberblock write** is a write that carries, in an uberblock
//!   slot of one of the four labels of a vdev on the whole volume, the
//!   uberblock magic and a group above zero
//!   ([`skyzfs_uberblock::Labels::uberblocks_in`],
//!   [`skyzfs_uberblock::Uberblock::counts`]), in a slot whose label
//!   checksum holds ([`skyzfs_uberblock::Uberblock::verified`], work
//!   package M2.1c): what ZFS takes for an uberblock. Without the
//!   checksum, bytes in a ring that carry the magic and a high group but
//!   that ZFS would pass over would mark, the cut after them would take
//!   their group, which errs high, and in the view they would be the
//!   greatest and the cut's own state, so the cut would say it opens as
//!   itself: a publish could take that group. The store looks for one
//!   when it receives a write, and only in a write that reaches a ring;
//!   the cost is one SHA-256 of each slot size tried, 1 to 8 KiB, for
//!   each uberblock found. A valid uberblock that something other than
//!   ZFS writes, such as a label restored with `dd`, is beyond the
//!   check: it marks as ZFS's own does.
//! - **It marks once the store has applied it**: when its direct write
//!   has returned whole and the store has taken that result, which is
//!   before the store's completion is handed out and so before the
//!   daemon acknowledges it. A write that fails marks nothing (the
//!   volume fails with it). A write that holds several uberblocks (a
//!   ring written whole) is taken slot by slot, in their order on the
//!   volume.
//! - **A repeat marks nothing and changes no mark**: an uberblock whose
//!   group and root block pointer equal those of the mark, those of a
//!   cut a flush carries to its barrier, or those of the newest cut
//!   ([`skyzfs_uberblock::is_repeat`]). The newest cut is there for
//!   multihost: a multihost write that still carries the group before
//!   can be applied after the group's own uberblock writes, and compared
//!   with the write before it alone it would move the mark back (design
//!   §5.2, "What counts as a repeat", from this package's finding). The
//!   carried cut is there because a flush takes the mark when it is
//!   received and counts the cut at its barrier: ZFS issues a group's
//!   four label writes at once, and one of them can be applied between
//!   a stray flush's receipt and its barrier. Compared with the mark and
//!   the newest cut alone it would mark the same state again, and the
//!   next flush would cut it twice (the review of this package, and
//!   Codex's).
//! - **The mark is taken by the first flush received after it**, which
//!   then carries a cut, and the mark is gone. A flush received while
//!   the uberblock write was still being written carries none: it was
//!   received before the write was applied. Two uberblocks applied with
//!   no flush received between them leave one mark, the later one's.
//! - **The cut happens at that flush's barrier**: when the store
//!   executes the flush, which it does only once every write received
//!   before it has ended and before anything received after it begins
//!   (§5.1, the in-place store's order; the defects `FLUSH_PASSES_WRITES`
//!   and `OVERLAP_UNORDERED` show those clauses tested). There the
//!   count of cuts goes up by one and the cut takes its number, with
//!   the mark's group beside it. A flush that never reaches its barrier
//!   (the volume failed first) counts no cut.
//! - **Each cut says whether it opens as itself** (work package M2.1b;
//!   design §5.2, "Whether a cut opens as itself"): whether an import
//!   of the volume as it is at the cut's instant, with no rewind, would
//!   open the state of the uberblock that marks it. The rule keeps the
//!   four rings in view, each whole KiB a position
//!   ([`skyzfs_uberblock::View`]): the store's start reads them before
//!   the first request (`InPlace::read_rings`, 4 × 128 KiB), and every
//!   write and write-zeroes the store applies to them after changes the
//!   view as it changed the volume. A discard changes nothing, as the
//!   store keeps a discarded block's content. At the cut, the answer is
//!   yes when every position is known and every uberblock of the
//!   greatest rank in view carries the mark's state, the rank being the
//!   order an import chooses by (`vdev_uberblock_compare`: the group,
//!   then the time, then the multihost sequence;
//!   [`skyzfs_uberblock::Rank`]). States are compared, not uberblocks: a
//!   multihost write carries the cut's own group and root with a later
//!   time, and ranks above the uberblock that marks the cut, yet an
//!   import of it opens the cut's state (the lead's reading, M2.1b).
//!   Likewise the first group of a rewind import by `-F` has the
//!   abandoned best's group, another root and a later time, and is
//!   chosen at once. Otherwise the cut opens only with a
//!   rewind to its group, which a later abandoned history above it
//!   shows: after a rewind import, the abandoned groups stay in the
//!   rings until the new ones are written over them.
//!
//! Each rule has a defect among [`Mutants`] and a test that fails with
//! it (`tests/cuts.rs`). The recorded pools of M2.T1 hold the parser,
//! the count and the answer against a reference
//! (`tests/recorded.rs`).
//!
//! A test build can watch the rule's moments ([`Seen`],
//! `InPlace::watch`, with the `test-hooks` feature): every write applied,
//! every flush's barrier, and every cut counted, each as it happens on
//! the thread that drives the store. The daemon's request log writes
//! them down (`skyzfsd serve --request-log`).
//!
//! # Limits of this package
//!
//! - **The count is in memory.** It starts at 0 with each store, so a
//!   take-over counts from 0 again, and an uberblock write the kernel
//!   hands the next instance again counts as new. The mapped state of
//!   M2.2 is to keep the count and the mark.
//! - **A cut's group can err low.** Two uberblocks applied with no flush
//!   between them that are not repeats leave the later one's mark. A
//!   stale write the repeat does not catch (a multihost copy two groups
//!   behind, or the first after a start, with no newest cut in memory)
//!   makes a cut whose group is lower than the one the volume holds: the
//!   volume is still a complete pool, and the group only delays a
//!   publish.
//! - **It can err high in one case: a cut made during an import** (the
//!   review's read of the amended §5.2, in the implementation notes
//!   under M2, "The read of these rules"). A mark left from before the
//!   import, an uberblock applied with no flush received after it, is
//!   taken by the first flush the import sends, before the import's own
//!   first uberblock. After a rewind import, writes of the import may by
//!   then have overwritten blocks that group needs, and the volume at
//!   the cut's instant is not a pool at it. The cut is counted before
//!   the import returns, and so before an agent can read the count that
//!   it publishes after; that a writable import syncs a group before it
//!   returns is the review's reading of OpenZFS, which the recorded
//!   pools of M2.T1 are to confirm. In this package the mark is in
//!   memory, so it outlives an import only where one daemon served the
//!   pool before the import and through it. Outside an import a cut's
//!   group is that of an uberblock the store applied before the cut's
//!   flush was received, so at the cut's instant that uberblock is in
//!   the volume with every block of its group (§5.2's first reason), and
//!   nothing it reaches has been overwritten (its second).
//! - **That argument has a premise: the volume holds the whole pool**,
//!   as the one data device the pool was created on (§5.2, the premise
//!   before "Three details"). A volume that joined a pool by `zpool
//!   attach` or `zpool replace`, or a pool with a second data device,
//!   makes cuts whose group is right and whose volume is not a complete
//!   pool at it, and the uberblocks cannot show that. The store counts
//!   such cuts all the same: the agent checks the pool's layout before
//!   it publishes one (M2.6).
//! - **Whether a cut opens as itself errs only toward "no"** (the
//!   conservative readings of M2.1b, where the design is silent). The
//!   view holds every slot with the magic and a group above zero and
//!   checks no checksum, while an import passes over a slot whose
//!   checksum fails: such a slot above the cut makes it a rewind that
//!   would have opened as itself. A position the start could not read
//!   is unknown, and so is one a write covered only in part; while any
//!   is, no cut opens as itself. Two different states that tie at the
//!   greatest rank are taken as a rewind, though an import opens one of
//!   them. And the view says which state an import chooses, not that
//!   the state opens: that rests on its blocks, which the rings do not
//!   show.
//! - **"Itself" is defined on the volume alone, on purpose.** The
//!   product's pool has one more top-level vdev, the node's log device
//!   (design §6.1), and ZFS writes uberblocks to its labels too; the
//!   store sees only the volume's. A restore on another node has only
//!   the volume and imports it without the log device, so the answer is
//!   about an import of the volume alone.
//! - **The view is in memory**, like the count. Each start reads it
//!   again from the rings, so it does not depend on what an instance
//!   before kept.

use std::collections::BTreeMap;

use skyzfs_blk::{Op, RequestId};
use skyzfs_uberblock::{is_repeat, Labels, State, Uberblock, View, RING_SIZE};

use super::Mutants;

/// A cut, as the store counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// Its number in the count of cuts, from 1.
    pub number: u64,
    /// The transaction group of the uberblock whose mark it took.
    pub group: u64,
    /// Whether it opens as itself (design §5.2): whether an import of
    /// the volume as it is at the cut's instant, with no rewind, would
    /// open the pool state of the uberblock that marks it. One that does
    /// not opens only with a rewind to its group.
    pub itself: bool,
}

/// The store's count of cuts since it began, and the newest cut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cuts {
    /// The cuts counted since the store began.
    pub count: u64,
    /// The newest cut; `None` before the first.
    pub newest: Option<Cut>,
}

/// A moment of the rule, as a watcher is told of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Seen {
    /// Write `id` has been applied whole: its direct write returned
    /// whole and the store took the result. Every write, whether or not
    /// it carries an uberblock. A write that fails has none.
    Applied(RequestId),
    /// Flush `id` has reached its barrier: every write received before
    /// it has ended, and nothing received after it has begun.
    Barrier(RequestId),
    /// A cut was counted at flush `id`'s barrier, told after that
    /// flush's [`Seen::Barrier`].
    Cut(RequestId, Cut),
}

/// What is told of the rule's moments, on the thread that drives the
/// store. It must not block for long: the store waits for it.
pub type Watcher = Box<dyn FnMut(Seen) + Send>;

/// The watcher, if there is one.
#[derive(Default)]
struct Watching(Option<Watcher>);

impl Watching {
    fn tell(&mut self, seen: Seen) {
        if let Some(watcher) = &mut self.0 {
            watcher(seen);
        }
    }
}

impl std::fmt::Debug for Watching {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() {
            "Watching(Some)"
        } else {
            "Watching(None)"
        })
    }
}

/// What a write or a write-zeroes that reaches a ring changes there,
/// from its receipt until it is applied or ends.
#[derive(Debug)]
struct Change {
    offset: u64,
    len: u64,
    /// The uberblocks it carries; none for a write-zeroes.
    found: Vec<Uberblock>,
}

/// The rule's state: see the [module documentation](self).
#[derive(Debug)]
pub(crate) struct CutRule {
    labels: Labels,
    mutants: Mutants,
    /// What the writes and write-zeroes received and not yet ended
    /// change in the rings, by request.
    changes: BTreeMap<RequestId, Change>,
    /// The uberblocks in the rings, as the start read them and the
    /// writes applied since changed them.
    view: View,
    /// The mark: the latest uberblock applied that was not a repeat,
    /// with no flush received since.
    mark: Option<Uberblock>,
    /// The newest cut's uberblock, which a repeat may equal.
    newest: Option<State>,
    /// The uberblock applied last, a repeat included: only for the
    /// defect [`Mutants::STALE_MOVES_MARK`], whose repeat compares with
    /// it alone.
    previous: Option<State>,
    /// Flushes received after a mark, by request: each carries a cut,
    /// counted at its barrier.
    carrying: BTreeMap<RequestId, Uberblock>,
    cuts: Cuts,
    watching: Watching,
}

impl CutRule {
    /// The rule for a volume of `size` bytes, whose vdev is the whole
    /// volume. What its rings hold is not known until the start has
    /// read them ([`CutRule::read_at_start`]).
    pub(crate) fn new(size: u64, mutants: Mutants) -> Self {
        let labels = Labels::for_size(size);
        let view = if mutants.has(Mutants::RINGS_UNREAD) {
            // The defect: the rings are taken to hold nothing.
            View::empty(labels)
        } else {
            View::unknown(labels)
        };
        Self {
            labels,
            mutants,
            changes: BTreeMap::new(),
            view,
            mark: None,
            newest: None,
            previous: None,
            carrying: BTreeMap::new(),
            cuts: Cuts::default(),
            watching: Watching::default(),
        }
    }

    /// Tell `watcher` of the rule's moments from now on.
    #[cfg(any(test, feature = "test-hooks"))]
    pub(crate) fn watch(&mut self, watcher: Watcher) {
        self.watching = Watching(Some(watcher));
    }

    /// The count, and the newest cut.
    pub(crate) fn cuts(&self) -> Cuts {
        self.cuts
    }

    /// Where the rings lie, and how long each is: what the start reads.
    pub(crate) fn rings(&self) -> Vec<(u64, u64)> {
        self.labels
            .rings()
            .map(|ring| (ring.start, RING_SIZE))
            .collect()
    }

    /// The start read `data` at `offset`: a ring as the volume holds it
    /// before the first request.
    pub(crate) fn read_at_start(&mut self, offset: u64, data: &[u8]) {
        if !self.mutants.has(Mutants::RINGS_UNREAD) {
            let found = self.found(offset, data);
            self.view.changed(offset, data.len() as u64, &found);
        }
    }

    /// The uberblocks ZFS would take for one in `data` at `offset`: those
    /// whose label checksum holds ([`Uberblock::verified`]).
    fn found(&self, offset: u64, data: &[u8]) -> Vec<Uberblock> {
        let mut found = self.labels.uberblocks_in(offset, data);
        if !self.mutants.has(Mutants::CHECKSUM_UNCHECKED) {
            found.retain(Uberblock::verified);
        }
        found
    }

    /// The store has received request `id`, in the order it receives
    /// them.
    pub(crate) fn received(&mut self, id: RequestId, op: &Op) {
        let (offset, len, found) = match op {
            Op::Write { offset, data } => {
                if !self.labels.reaches_a_ring(*offset, data.len() as u64) {
                    return;
                }
                let found = self.found(*offset, data);
                (*offset, data.len() as u64, found)
            }
            Op::WriteZeroes { offset, len } => {
                if !self.labels.reaches_a_ring(*offset, *len) {
                    return;
                }
                (*offset, *len, Vec::new())
            }
            Op::Flush => {
                let Some(uberblock) = self.mark.take() else {
                    return;
                };
                if self.mutants.has(Mutants::CUT_AT_RECEIPT) {
                    // The defect: counted as the flush is received, not
                    // at its barrier.
                    return self.count(id, uberblock);
                }
                self.carrying.insert(id, uberblock);
                return;
            }
            _ => return,
        };
        let change = Change { offset, len, found };
        if self.mutants.has(Mutants::MARK_AT_RECEIPT) {
            // The defect: marked as received, not as applied.
            return self.apply(change);
        }
        self.changes.insert(id, change);
    }

    /// Write `id` has been applied whole.
    pub(crate) fn applied(&mut self, id: RequestId) {
        self.watching.tell(Seen::Applied(id));
        if let Some(change) = self.changes.remove(&id) {
            self.apply(change);
        }
    }

    /// Write-zeroes `id` has been applied whole.
    pub(crate) fn zeroed(&mut self, id: RequestId) {
        if let Some(change) = self.changes.remove(&id) {
            self.apply(change);
        }
    }

    /// Flush `id` has reached its barrier.
    pub(crate) fn barrier(&mut self, id: RequestId) {
        self.watching.tell(Seen::Barrier(id));
        if let Some(uberblock) = self.carrying.remove(&id) {
            self.count(id, uberblock);
        }
    }

    /// Request `id` has ended, however: what it carried and did not use
    /// is forgotten.
    pub(crate) fn ended(&mut self, id: RequestId) {
        self.changes.remove(&id);
        self.carrying.remove(&id);
    }

    fn apply(&mut self, change: Change) {
        self.view.changed(change.offset, change.len, &change.found);
        for uberblock in change.found {
            if !uberblock.counts() && !self.mutants.has(Mutants::GROUP_ZERO_COUNTED) {
                continue;
            }
            let state = uberblock.state;
            if self.marks(&state) {
                self.mark = Some(uberblock);
            }
            self.previous = Some(state);
        }
    }

    /// Whether an applied uberblock naming `state` marks: whether it is
    /// not a repeat.
    fn marks(&self, state: &State) -> bool {
        if self.mutants.has(Mutants::REPEAT_COUNTED) {
            return true;
        }
        if self.mutants.has(Mutants::STALE_MOVES_MARK) {
            return !is_repeat(self.previous.as_ref(), None, state);
        }
        let mark = self.mark.map(|uberblock| uberblock.state);
        if self.mutants.has(Mutants::REPEAT_BY_GROUP) {
            let same = |other: Option<State>| other.is_some_and(|o| o.txg == state.txg);
            let carried = self
                .carrying
                .values()
                .any(|carried| carried.state.txg == state.txg);
            return !(same(mark) || same(self.newest) || carried);
        }
        let carried = !self.mutants.has(Mutants::REPEAT_WHILE_CARRIED)
            && self
                .carrying
                .values()
                .any(|carried| is_repeat(Some(&carried.state), None, state));
        !carried && !is_repeat(mark.as_ref(), self.newest.as_ref(), state)
    }

    fn count(&mut self, id: RequestId, uberblock: Uberblock) {
        let state = uberblock.state;
        self.newest = Some(state);
        self.cuts.count += 1;
        let by_group = self.mutants.has(Mutants::ORDER_BY_GROUP);
        let mut itself = self.view.chosen(&state, by_group);
        if self.mutants.has(Mutants::ITSELF_BY_RANK) {
            // The defect: uberblocks compared, not states.
            itself = itself
                && self
                    .view
                    .uberblocks()
                    .all(|other| other.rank <= uberblock.rank);
        }
        let cut = Cut {
            number: self.cuts.count,
            group: state.txg,
            itself,
        };
        self.cuts.newest = Some(cut);
        self.watching.tell(Seen::Cut(id, cut));
    }
}

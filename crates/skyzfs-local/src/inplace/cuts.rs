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
//!   [`skyzfs_uberblock::Uberblock::counts`]). The store looks for one
//!   when it receives a write, and only in a write that reaches a ring.
//! - **It marks once the store has applied it**: when its direct write
//!   has returned whole and the store has taken that result, which is
//!   before the store's completion is handed out and so before the
//!   daemon acknowledges it. A write that fails marks nothing (the
//!   volume fails with it). A write that holds several uberblocks (a
//!   ring written whole) is taken slot by slot, in their order on the
//!   volume.
//! - **A repeat marks nothing and changes no mark**: an uberblock whose
//!   group and root block pointer equal those of the mark, or those of
//!   the newest cut ([`skyzfs_uberblock::is_repeat`]). The newest cut is
//!   there for multihost: a multihost write that still carries the group
//!   before can be applied after the group's own uberblock writes, and
//!   compared with the write before it alone it would move the mark back
//!   (design §5.2, "What counts as a repeat", from this package's
//!   finding).
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
//!
//! Each rule has a defect among [`Mutants`] and a test that fails with
//! it (`tests/cuts.rs`).
//!
//! # Limits of this package
//!
//! - **The count is in memory.** It starts at 0 with each store, so a
//!   take-over counts from 0 again, and an uberblock write the kernel
//!   hands the next instance again counts as new. The mapped state of
//!   M2.2 is to keep the count and the mark.
//! - **A cut's group can err low, never high.** Two uberblocks applied
//!   with no flush between them that are not repeats leave the later
//!   one's mark. A stale write the repeat does not catch (a multihost
//!   copy two groups behind, or the first after a start, with no newest
//!   cut in memory) makes a cut whose group is lower than the one the
//!   volume holds: the volume is still a complete pool, and the group
//!   only delays a publish. It cannot err high: a cut's group is that of
//!   an uberblock the store applied before the cut's flush was received,
//!   so at the cut's instant that uberblock is in the volume, and ZFS
//!   does not write an uberblock of a group before every block of that
//!   group is acknowledged (§5.2's first reason).

use std::collections::BTreeMap;

use skyzfs_blk::{Op, RequestId};
use skyzfs_uberblock::{is_repeat, Labels, State, Uberblock};

use super::Mutants;

/// A cut, as the store counts it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cut {
    /// Its number in the count of cuts, from 1.
    pub number: u64,
    /// The transaction group of the uberblock whose mark it took.
    pub group: u64,
}

/// The store's count of cuts since it began, and the newest cut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cuts {
    /// The cuts counted since the store began.
    pub count: u64,
    /// The newest cut; `None` before the first.
    pub newest: Option<Cut>,
}

/// The rule's state: see the [module documentation](self).
#[derive(Debug)]
pub(crate) struct CutRule {
    labels: Labels,
    mutants: Mutants,
    /// The uberblocks in writes received and not yet ended, by request.
    written: BTreeMap<RequestId, Vec<Uberblock>>,
    /// The mark: the latest uberblock applied that was not a repeat,
    /// with no flush received since.
    mark: Option<State>,
    /// The newest cut's uberblock, which a repeat may equal.
    newest: Option<State>,
    /// The uberblock applied last, a repeat included: only for the
    /// defect [`Mutants::STALE_MOVES_MARK`], whose repeat compares with
    /// it alone.
    previous: Option<State>,
    /// Flushes received after a mark, by request: each carries a cut,
    /// counted at its barrier.
    carrying: BTreeMap<RequestId, State>,
    cuts: Cuts,
}

impl CutRule {
    /// The rule for a volume of `size` bytes, whose vdev is the whole
    /// volume.
    pub(crate) fn new(size: u64, mutants: Mutants) -> Self {
        Self {
            labels: Labels::for_size(size),
            mutants,
            written: BTreeMap::new(),
            mark: None,
            newest: None,
            previous: None,
            carrying: BTreeMap::new(),
            cuts: Cuts::default(),
        }
    }

    /// The count, and the newest cut.
    pub(crate) fn cuts(&self) -> Cuts {
        self.cuts
    }

    /// The store has received request `id`, in the order it receives
    /// them.
    pub(crate) fn received(&mut self, id: RequestId, op: &Op) {
        match op {
            Op::Write { offset, data } => {
                if !self.labels.reaches_a_ring(*offset, data.len() as u64) {
                    return;
                }
                let found = self.labels.uberblocks_in(*offset, data);
                if found.is_empty() {
                    return;
                }
                if self.mutants.has(Mutants::MARK_AT_RECEIPT) {
                    // The defect: marked as received, not as applied.
                    return self.apply(found);
                }
                self.written.insert(id, found);
            }
            Op::Flush => {
                let Some(state) = self.mark.take() else {
                    return;
                };
                if self.mutants.has(Mutants::CUT_AT_RECEIPT) {
                    // The defect: counted as the flush is received, not
                    // at its barrier.
                    return self.count(state);
                }
                self.carrying.insert(id, state);
            }
            _ => {}
        }
    }

    /// Write `id` has been applied whole.
    pub(crate) fn applied(&mut self, id: RequestId) {
        if let Some(found) = self.written.remove(&id) {
            self.apply(found);
        }
    }

    /// Flush `id` has reached its barrier.
    pub(crate) fn barrier(&mut self, id: RequestId) {
        if let Some(state) = self.carrying.remove(&id) {
            self.count(state);
        }
    }

    /// Request `id` has ended, however: what it carried and did not use
    /// is forgotten.
    pub(crate) fn ended(&mut self, id: RequestId) {
        self.written.remove(&id);
        self.carrying.remove(&id);
    }

    fn apply(&mut self, found: Vec<Uberblock>) {
        for uberblock in found {
            if !uberblock.counts() && !self.mutants.has(Mutants::GROUP_ZERO_COUNTED) {
                continue;
            }
            let state = uberblock.state;
            if self.marks(&state) {
                self.mark = Some(state);
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
        if self.mutants.has(Mutants::REPEAT_BY_GROUP) {
            let same = |other: Option<State>| other.is_some_and(|o| o.txg == state.txg);
            return !(same(self.mark) || same(self.newest));
        }
        !is_repeat(self.mark.as_ref(), self.newest.as_ref(), state)
    }

    fn count(&mut self, state: State) {
        self.newest = Some(state);
        self.cuts.count += 1;
        self.cuts.newest = Some(Cut {
            number: self.cuts.count,
            group: state.txg,
        });
    }
}

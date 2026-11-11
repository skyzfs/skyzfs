//! A reference model of the contract's observable behaviour.
//!
//! The model sees what a client sees: requests as they are submitted,
//! completions as they are delivered, daemon losses and power cuts. For
//! every completion it answers one question: *is there a total order of
//! whole operations, as the contract describes the daemon's, under which a
//! correct target produces this?* It accepts exactly those histories.
//!
//! # How it works
//!
//! The daemon's order is hidden from the client, so the model keeps every
//! order that is still possible. Each candidate is a **world**:
//!
//! - `durable`: the content that no power cut can take back,
//! - `tail`: the operations after that, in the world's order, each with
//!   the window of time in which it can have been applied,
//! - `waiting`: operations in flight that the world has not placed yet.
//!
//! Two tails that differ only in the order of operations which neither
//! overlap nor are ordered by time are the same world: such operations
//! commute in every observation the contract defines. What remains is one
//! world per way of ordering *overlapping* operations that were in flight
//! together (rule R8).
//!
//! - An **acknowledgement** places the operation: after everything that
//!   was acknowledged before it was submitted, and either side of each
//!   overlapping operation it was in flight with.
//! - A **read** is checked block by block: it may see any state between
//!   "everything acknowledged before the read was submitted" and "now"
//!   (rule R3). Worlds in which it cannot be explained are dropped. If
//!   none is left, the target broke the contract.
//! - A **flush** makes durable what was acknowledged before it was
//!   submitted, and whatever precedes that in the world's order (rule R7).
//!   A flushed operation stays in the tail, marked, until nothing that
//!   was in flight with it is outstanding: a read may still return the
//!   state before it, and an overlapping operation may still turn out to
//!   precede it.
//! - A **daemon loss** makes the whole log durable, as recovery does, and
//!   returns the requests in flight to `waiting`. Each of them may already
//!   be in the log, so each may end up in the order twice (rule R9).
//! - A **power cut** leaves a prefix of the world's order. Which one is
//!   unknown until the client reads, so the model asks for a **read-back**
//!   of every block an unflushed operation touched before anything else
//!   is submitted, and then keeps the prefixes that explain it (rule R10).
//!
//! # Limits
//!
//! These are limits of the model as a test tool, not of the contract.
//!
//! - The number of worlds grows with the number of overlapping operations
//!   in flight together whose order no read has settled. A client should
//!   keep that small and read or flush now and then; [`Model::worlds`]
//!   says how many there are.
//! - After a power cut the client must read back what
//!   [`Model::readback`] lists before it submits anything but reads.
//! - Once the log has failed ([`Model::log_failed`]) and the first failure
//!   has been delivered, nothing can be observed any more; the model then
//!   only requires that everything fails, until the volume is opened
//!   again ([`Model::reopened`]). There it asks for a prefix, as rule
//!   R11 does.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;

use crate::{
    Base, BlkError, Completion, Geometry, Op, Request, RequestId, BLOCK_SIZE, BLOCK_USIZE,
};

/// An interned block content. `ZERO` is a block of zeros.
type Cid = u32;
const ZERO: Cid = 0;

/// What a block may hold: more than one content only after a discard.
/// Sorted and without repeats.
type Cell = Vec<Cid>;

/// Block contents that differ from the base.
type Image = BTreeMap<u64, Cell>;

#[derive(Debug, Clone)]
enum Effect {
    /// One content per block.
    Data(Vec<Cid>),
    Zeros,
    Discard,
}

/// A write, write-zeroes or discard.
#[derive(Debug, Clone)]
struct Mutation {
    first: u64,
    count: u64,
    effect: Effect,
}

impl Mutation {
    fn covers(&self, block: u64) -> bool {
        block >= self.first && block - self.first < self.count
    }

    fn overlaps(&self, other: &Mutation) -> bool {
        self.first < other.first + other.count && other.first < self.first + self.count
    }

    fn blocks(&self) -> std::ops::Range<u64> {
        self.first..self.first + self.count
    }
}

/// What every world shares.
#[derive(Debug, Clone)]
struct Context {
    base: Base,
    ids: HashMap<Arc<[u8]>, Cid>,
    /// Every mutation ever submitted, by index.
    ops: Vec<Mutation>,
    /// The base content of the blocks touched so far, for a non-empty base.
    base_cids: BTreeMap<u64, Cid>,
    /// Every content that was ever meant for a block, to tell a torn block
    /// from a foreign one.
    history: BTreeMap<u64, Vec<Arc<[u8]>>>,
}

impl Context {
    fn intern(&mut self, bytes: &[u8]) -> Cid {
        if let Some(id) = self.ids.get(bytes) {
            return *id;
        }
        let id = self.ids.len() as Cid;
        self.ids.insert(Arc::from(bytes), id);
        id
    }

    fn lookup(&self, bytes: &[u8]) -> Option<Cid> {
        self.ids.get(bytes).copied()
    }

    /// Learn the base content of `blocks`.
    fn touch(&mut self, blocks: std::ops::Range<u64>) {
        if self.base == Base::Empty {
            return;
        }
        let mut buf = vec![0u8; BLOCK_USIZE];
        for block in blocks {
            if !self.base_cids.contains_key(&block) {
                self.base.fill_block(block, &mut buf);
                let id = self.intern(&buf);
                self.base_cids.insert(block, id);
                self.history
                    .entry(block)
                    .or_default()
                    .push(Arc::from(&buf[..]));
            }
        }
    }

    fn base_cid(&self, block: u64) -> Cid {
        self.base_cids.get(&block).copied().unwrap_or(ZERO)
    }

    fn cell(&self, image: &Image, block: u64) -> Cell {
        image
            .get(&block)
            .cloned()
            .unwrap_or_else(|| vec![self.base_cid(block)])
    }

    /// What `op` leaves in `block`, given what was there.
    fn step(&self, op: &Mutation, block: u64, before: Cell) -> Cell {
        match &op.effect {
            Effect::Data(cids) => vec![cids[(block - op.first) as usize]],
            Effect::Zeros => vec![ZERO],
            Effect::Discard => {
                let mut cell = before;
                cell.push(ZERO);
                cell.push(self.base_cid(block));
                cell.sort_unstable();
                cell.dedup();
                cell
            }
        }
    }

    fn apply(&self, image: &mut Image, op: &Mutation) {
        for block in op.blocks() {
            let cell = self.step(op, block, self.cell(image, block));
            if cell.len() == 1 && cell[0] == self.base_cid(block) {
                image.remove(&block);
            } else {
                image.insert(block, cell);
            }
        }
    }

    /// Whether `a`, earlier in a tail than `b`, must stay before it.
    fn ordered(&self, a: &Placed, b: &Placed) -> bool {
        a.ub < b.lb || self.ops[a.op].overlaps(&self.ops[b.op])
    }

    /// The one representative of all tails that differ from `tail` only in
    /// the order of operations that commute.
    fn canonical(&self, tail: &[Placed]) -> Vec<Placed> {
        let n = tail.len();
        let mut blockers = vec![0usize; n];
        for j in 0..n {
            for i in 0..j {
                if self.ordered(&tail[i], &tail[j]) {
                    blockers[j] += 1;
                }
            }
        }
        let mut done = vec![false; n];
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            let next = (0..n)
                .filter(|i| !done[*i] && blockers[*i] == 0)
                .min_by_key(|i| tail[*i]);
            let Some(i) = next else { break };
            done[i] = true;
            out.push(tail[i]);
            for j in i + 1..n {
                if !done[j] && self.ordered(&tail[i], &tail[j]) {
                    blockers[j] -= 1;
                }
            }
        }
        out
    }
}

/// An operation with a place in a world's order. It was applied at some
/// moment in `lb..=ub`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Placed {
    op: usize,
    lb: u64,
    ub: u64,
    /// A completed flush covers it: every power cut keeps it. It stays in
    /// the tail for as long as a read in flight may have seen the state
    /// before it.
    flushed: bool,
}

/// An operation in flight that the world has not placed. It has not been
/// applied before `lb`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Waiting {
    op: usize,
    lb: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct World {
    durable: Image,
    tail: Vec<Placed>,
    waiting: Vec<Waiting>,
}

impl World {
    /// Every world that results from placing `inst`.
    ///
    /// What matters is which members of the tail precede it. Those that
    /// were certainly applied before it could have been must; of the
    /// others, only the ones that overlap it make a difference, and each
    /// of those may go either side. A choice is consistent if taking the
    /// chosen ones, and everything that must precede them, does not drag
    /// in an overlapping one that was not chosen.
    fn place(&self, cx: &Context, inst: Placed, out: &mut BTreeSet<World>) {
        let n = self.tail.len();
        let must = self.closure(cx, &|p| p.ub < inst.lb);
        let free: Vec<usize> = (0..n)
            .filter(|i| !must[*i] && cx.ops[self.tail[*i].op].overlaps(&cx.ops[inst.op]))
            .collect();
        // One bit of `choice` for each. The suite keeps this far below the
        // width of the mask; beyond it the enumeration would be incomplete
        // and the model would reject correct targets, so stop instead.
        assert!(
            free.len() < 64,
            "the model cannot place an operation among {} overlapping operations \
             whose order is still open",
            free.len()
        );
        for choice in 0..1u64 << free.len() {
            let chosen = |i: usize| {
                free.iter()
                    .position(|f| *f == i)
                    .is_some_and(|k| choice >> k & 1 == 1)
            };
            let mut before = vec![false; n];
            for i in (0..n).rev() {
                before[i] = must[i]
                    || chosen(i)
                    || (i + 1..n).any(|j| before[j] && cx.ordered(&self.tail[i], &self.tail[j]));
            }
            if free.iter().any(|i| before[*i] != chosen(*i)) {
                continue;
            }
            let mut tail = Vec::with_capacity(n + 1);
            tail.extend((0..n).filter(|i| before[*i]).map(|i| self.tail[i]));
            tail.push(inst);
            tail.extend((0..n).filter(|i| !before[*i]).map(|i| self.tail[i]));
            out.insert(World {
                durable: self.durable.clone(),
                tail: cx.canonical(&tail),
                waiting: self.waiting.clone(),
            });
        }
    }

    /// Settle every waiting operation that `involved` selects: either it
    /// has not been applied yet (`None` drops it, `Some(lb)` keeps it
    /// waiting with that bound), or it was applied by `now`.
    fn settle(
        &self,
        cx: &Context,
        now: u64,
        involved: &dyn Fn(&Mutation) -> bool,
        keep_from: Option<u64>,
    ) -> BTreeSet<World> {
        let mut worlds = BTreeSet::from([self.clone()]);
        for w in self.waiting.iter().filter(|w| involved(&cx.ops[w.op])) {
            let mut next = BTreeSet::new();
            for world in worlds {
                let mut without = world.clone();
                without.waiting.retain(|x| x.op != w.op);
                let inst = Placed {
                    op: w.op,
                    lb: w.lb,
                    ub: now,
                    flushed: false,
                };
                without.place(cx, inst, &mut next);
                if let Some(lb) = keep_from {
                    without.waiting.push(Waiting { op: w.op, lb });
                    without.waiting.sort_unstable();
                }
                next.insert(without);
            }
            worlds = next;
        }
        worlds
    }

    /// The members of the tail that are, or must precede, a member that
    /// `forced` selects.
    fn closure(&self, cx: &Context, forced: &dyn Fn(&Placed) -> bool) -> Vec<bool> {
        let n = self.tail.len();
        let mut inside = vec![false; n];
        for i in (0..n).rev() {
            inside[i] = forced(&self.tail[i])
                || (i + 1..n).any(|j| inside[j] && cx.ordered(&self.tail[i], &self.tail[j]));
        }
        inside
    }

    /// Move the selected members of the tail, which must be closed under
    /// "precedes", into the durable content.
    fn absorb(&mut self, cx: &Context, inside: &[bool]) {
        let tail = std::mem::take(&mut self.tail);
        for (placed, absorbed) in tail.into_iter().zip(inside) {
            if *absorbed {
                cx.apply(&mut self.durable, &cx.ops[placed.op]);
            } else {
                self.tail.push(placed);
            }
        }
    }

    /// Mark the selected members of the tail, which must be closed under
    /// "precedes", as covered by a flush.
    fn mark_flushed(&mut self, cx: &Context, inside: &[bool]) {
        for (placed, covered) in self.tail.iter_mut().zip(inside) {
            placed.flushed |= *covered;
        }
        self.tail = cx.canonical(&self.tail);
    }

    /// Fold flushed operations into the durable content, except those that
    /// something in flight may still need the state before: a read that
    /// may return it, or an operation that may yet be placed before them.
    /// `horizon` is when the oldest such request was submitted.
    fn compact(&mut self, cx: &Context, horizon: u64) {
        let n = self.tail.len();
        let mut fold = vec![false; n];
        for i in 0..n {
            fold[i] = self.tail[i].flushed
                && self.tail[i].ub < horizon
                && (0..i).all(|j| fold[j] || !cx.ordered(&self.tail[j], &self.tail[i]));
        }
        self.absorb(cx, &fold);
    }

    /// Whether a read submitted at `since` may have returned `seen` for
    /// `block` (rule R3).
    fn explains(&self, cx: &Context, block: u64, seen: Option<Cid>, certain: &[bool]) -> bool {
        let Some(seen) = seen else { return false };
        // The read saw the block after at least every operation that was
        // certainly applied before the read was submitted.
        let last_certain = (0..self.tail.len())
            .rev()
            .find(|i| certain[*i] && cx.ops[self.tail[*i].op].covers(block));
        let mut cell = cx.cell(&self.durable, block);
        if last_certain.is_none() && cell.contains(&seen) {
            return true;
        }
        for (i, placed) in self.tail.iter().enumerate() {
            let op = &cx.ops[placed.op];
            if op.covers(block) {
                cell = cx.step(op, block, cell);
                if last_certain.is_none_or(|c| i >= c) && cell.contains(&seen) {
                    return true;
                }
            }
        }
        false
    }

    /// Every prefix of this world's order that explains what was read back
    /// after a power cut, as a world of its own.
    fn prefixes(&self, cx: &Context, seen: &Readback, out: &mut BTreeSet<World>) {
        let n = self.tail.len();
        // For each member, the blocks of which it is the last writer.
        let mut last: Vec<Vec<u64>> = vec![Vec::new(); n];
        let mut owner: BTreeMap<u64, usize> = BTreeMap::new();
        for (i, placed) in self.tail.iter().enumerate() {
            for block in cx.ops[placed.op].blocks() {
                owner.insert(block, i);
            }
        }
        for (block, i) in owner {
            last[i].push(block);
        }
        let mut present = vec![false; n];
        self.choose(cx, seen, &last, 0, self.durable.clone(), &mut present, out);
    }

    #[allow(clippy::too_many_arguments)]
    fn choose(
        &self,
        cx: &Context,
        seen: &Readback,
        last: &[Vec<u64>],
        i: usize,
        image: Image,
        present: &mut Vec<bool>,
        out: &mut BTreeSet<World>,
    ) {
        let agrees = |image: &Image, block: u64| {
            let cell = cx.cell(image, block);
            seen.get(&block)
                .is_none_or(|all| all.iter().all(|s| s.is_some_and(|c| cell.contains(&c))))
        };
        if i == self.tail.len() {
            if seen.keys().all(|block| agrees(&image, *block)) {
                out.insert(World {
                    durable: image,
                    tail: Vec::new(),
                    waiting: Vec::new(),
                });
            }
            return;
        }
        let op = &cx.ops[self.tail[i].op];
        // Kept: only if everything that must precede it was kept.
        let allowed = (0..i).all(|j| present[j] || !cx.ordered(&self.tail[j], &self.tail[i]));
        if allowed {
            let mut with = image.clone();
            cx.apply(&mut with, op);
            if last[i].iter().all(|block| agrees(&with, *block)) {
                present[i] = true;
                self.choose(cx, seen, last, i + 1, with, present, out);
                present[i] = false;
            }
        }
        // Lost: then every later operation on its blocks is lost too, so
        // those blocks are final.
        // An operation a flush covers is never lost.
        if !self.tail[i].flushed && op.blocks().all(|block| agrees(&image, block)) {
            self.choose(cx, seen, last, i + 1, image, present, out);
        }
    }
}

/// What the client read back after a power cut: for each block, every
/// answer it got. `None` is content the model has never seen.
type Readback = BTreeMap<u64, Vec<Option<Cid>>>;

#[derive(Debug, Clone)]
enum Pending {
    /// A flush submitted after the log had failed, with nothing to make
    /// durable: every operation acknowledged before it was covered by a
    /// completed flush. It promises nothing new and may complete either
    /// way (rule R11).
    IdleFlush,
    /// The request must fail. Where it is wrong in more than one way the
    /// contract does not say which error wins, so each that applies is
    /// accepted.
    MustFail {
        /// Invalid: a rejection is right (rule R2).
        invalid: bool,
        /// A mutation on a read-only target: `ReadOnly` is right (R12).
        read_only: bool,
        /// The log had failed when the request was submitted: `Failed` is
        /// right (rule R11). It is also right if the log fails later,
        /// while the request is in flight; that is judged at completion.
        failed: bool,
    },
    Read {
        first: u64,
        count: u64,
        /// The read saw nothing older than the state at this time.
        since: u64,
    },
    Mutation {
        op: usize,
    },
    Flush {
        at: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Health {
    Healthy,
    /// The log has failed; no failure has been delivered yet.
    Failing,
    /// A failure has been delivered: everything fails from here on.
    Failed,
}

/// How a block that a read returned differs from everything allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mismatch {
    /// Every byte comes from some content that was once meant for this
    /// block, but no single one: the block is a mix of two writes.
    Torn,
    /// The block holds content that is not allowed there now: stale, lost,
    /// or foreign data.
    Unexpected,
    /// Each block is allowed on its own, but no single order of whole
    /// operations explains them together.
    Inconsistent,
}

/// A completion, or a step of the client, that the contract does not allow.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Violation {
    /// A completion for an id that is not in flight: a request completed
    /// twice, or never submitted (rule R8).
    #[error("completion for {id}, which is not in flight")]
    UnknownCompletion {
        /// The id in the completion.
        id: RequestId,
    },
    /// The client reused an id that is in flight. A fault of the caller of
    /// the model, not of the target.
    #[error("{id} submitted while already in flight")]
    DuplicateSubmit {
        /// The id.
        id: RequestId,
    },
    /// The client submitted something other than a read before it had read
    /// back what a power cut may have changed. A fault of the caller of
    /// the model, not of the target.
    #[error("{id} submitted before the read-back after a power cut was complete")]
    ReadbackRequired {
        /// The id.
        id: RequestId,
    },
    /// An invalid request was not rejected (rule R2).
    #[error("{id} is invalid and must be rejected, but completed with {got:?}")]
    NotRejected {
        /// The id.
        id: RequestId,
        /// What the target answered.
        got: Result<(), BlkError>,
    },
    /// A request that must fail completed otherwise: a mutation on a
    /// read-only target (rule R12) or any request on a failed volume
    /// (rule R11).
    #[error("{id} must fail with {expected}, but completed with {got:?}")]
    NotFailed {
        /// The id.
        id: RequestId,
        /// The error the contract requires.
        expected: BlkError,
        /// What the target answered.
        got: Result<(), BlkError>,
    },
    /// A valid request failed.
    #[error("{id} is valid but failed: {error}")]
    UnexpectedError {
        /// The id.
        id: RequestId,
        /// The error.
        error: BlkError,
    },
    /// A read completed without its buffer, or with one of another length.
    #[error("read {id} returned a buffer of {got:?} bytes, expected {expected}")]
    BadBuffer {
        /// The id.
        id: RequestId,
        /// The length asked for.
        expected: usize,
        /// The length returned, or `None` if no buffer came back.
        got: Option<usize>,
    },
    /// A read returned what no order of whole operations explains (rules
    /// R3 to R9).
    #[error("read {id}: block {block} is {kind:?}")]
    BadRead {
        /// The id.
        id: RequestId,
        /// The first block that cannot be explained.
        block: u64,
        /// In what way.
        kind: Mismatch,
    },
    /// What was read back after a power cut is not a prefix of any order
    /// the daemon may have had that includes everything flushed (rule
    /// R10).
    #[error("after the power cut, block {block} is {kind:?}")]
    BadRecovery {
        /// The first block that cannot be explained.
        block: u64,
        /// In what way.
        kind: Mismatch,
    },
}

/// The reference model. See the [module documentation](self).
#[derive(Debug, Clone)]
pub struct Model {
    geometry: Geometry,
    cx: Context,
    worlds: Vec<World>,
    pending: BTreeMap<RequestId, Pending>,
    /// When each mutation was acknowledged.
    acked: BTreeMap<usize, u64>,
    /// When the last operation was acknowledged that the volume, as it
    /// has been recovered, does not hold durably for certain.
    last_ack: Option<u64>,
    /// The latest moment a completed flush was submitted at.
    flush_cover: u64,
    /// A clock that ticks at every event.
    now: u64,
    health: Health,
    /// After a power cut: the blocks to read back, and what was read.
    recovering: Option<(BTreeSet<u64>, Readback)>,
}

impl Model {
    /// A model of a fresh device on an empty base.
    pub fn new(geometry: Geometry) -> Self {
        Self::with_base(geometry, Base::Empty)
    }

    /// A model of a fresh device on `base`.
    pub fn with_base(geometry: Geometry, base: Base) -> Self {
        let mut cx = Context {
            base,
            ids: HashMap::new(),
            ops: Vec::new(),
            base_cids: BTreeMap::new(),
            history: BTreeMap::new(),
        };
        cx.intern(&[0u8; BLOCK_USIZE]);
        Self {
            geometry,
            cx,
            worlds: vec![World {
                durable: Image::new(),
                tail: Vec::new(),
                waiting: Vec::new(),
            }],
            pending: BTreeMap::new(),
            acked: BTreeMap::new(),
            last_ack: None,
            flush_cover: 0,
            now: 0,
            health: Health::Healthy,
            recovering: None,
        }
    }

    /// The geometry the model was built for.
    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    /// How many requests are in flight.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// How many orders of the daemon are still possible. See the limits in
    /// the [module documentation](self).
    pub fn worlds(&self) -> usize {
        self.worlds.len()
    }

    /// After a power cut: the blocks the client must still read back, with
    /// nothing else in flight, before it submits anything but reads. Empty
    /// otherwise.
    pub fn readback(&self) -> Vec<u64> {
        match &self.recovering {
            Some((needed, seen)) => needed
                .iter()
                .filter(|b| !seen.contains_key(b))
                .copied()
                .collect(),
            None => Vec::new(),
        }
    }

    /// The write log has failed for good (rule R11): a write or a sync
    /// of it has failed. Every flush submitted from now on that has
    /// something to make durable must fail, and every request submitted
    /// after the first failure has been delivered. A flush that finds
    /// every acknowledged operation covered by a completed flush has
    /// nothing to make durable and may complete either way until then. A request in flight now may complete
    /// either way; if a flush among them succeeds, the model holds the
    /// target to it: what it covered counts as durable.
    pub fn log_failed(&mut self) {
        if self.health == Health::Healthy {
            self.health = Health::Failing;
        }
    }

    fn set_worlds(&mut self, worlds: BTreeSet<World>) {
        self.worlds = worlds.into_iter().collect();
    }

    /// The client submits `request`. Call this before handing the request
    /// to the target. A request reissued after [`Model::kill`] must not be
    /// submitted to the model again.
    pub fn submit(&mut self, request: &Request) -> Result<(), Violation> {
        let id = request.id;
        if self.pending.contains_key(&id) {
            return Err(Violation::DuplicateSubmit { id });
        }
        let is_read = matches!(request.op, Op::Read { .. });
        if self.recovering.is_some() && !is_read {
            return Err(Violation::ReadbackRequired { id });
        }
        self.now += 1;
        let invalid = self.geometry.validate(&request.op).is_err();
        let read_only = self.geometry.read_only() && !is_read && request.op != Op::Flush;
        // A flush on a failed log must fail if it has something to make
        // durable. One that has not need not find out.
        let idle = self.last_ack.is_none_or(|at| at < self.flush_cover);
        let failing_flush = self.health == Health::Failing && request.op == Op::Flush;
        let must_fail = self.health == Health::Failed || (failing_flush && !idle);
        let pending = if failing_flush && idle && !invalid {
            Pending::IdleFlush
        } else if invalid || read_only || must_fail {
            Pending::MustFail {
                invalid,
                read_only,
                failed: self.health != Health::Healthy,
            }
        } else {
            match &request.op {
                Op::Flush => Pending::Flush { at: self.now },
                Op::Read { offset, buf } => {
                    let first = offset / BLOCK_SIZE;
                    let count = buf.len() as u64 / BLOCK_SIZE;
                    self.cx.touch(first..first + count);
                    Pending::Read {
                        first,
                        count,
                        since: self.now,
                    }
                }
                Op::Write { offset, data } => {
                    let first = offset / BLOCK_SIZE;
                    let mut cids = Vec::new();
                    for (i, chunk) in data.chunks(BLOCK_USIZE).enumerate() {
                        cids.push(self.cx.intern(chunk));
                        self.cx
                            .history
                            .entry(first + i as u64)
                            .or_default()
                            .push(Arc::from(chunk));
                    }
                    self.mutation(first, cids.len() as u64, Effect::Data(cids))
                }
                Op::WriteZeroes { offset, len } => {
                    self.mutation(offset / BLOCK_SIZE, len / BLOCK_SIZE, Effect::Zeros)
                }
                Op::Discard { offset, len } => {
                    self.mutation(offset / BLOCK_SIZE, len / BLOCK_SIZE, Effect::Discard)
                }
            }
        };
        self.pending.insert(id, pending);
        Ok(())
    }

    fn mutation(&mut self, first: u64, count: u64, effect: Effect) -> Pending {
        self.cx.touch(first..first + count);
        let op = self.cx.ops.len();
        self.cx.ops.push(Mutation {
            first,
            count,
            effect,
        });
        for world in &mut self.worlds {
            world.waiting.push(Waiting { op, lb: self.now });
        }
        Pending::Mutation { op }
    }

    /// The target delivered `completion`. An error means the target broke
    /// the contract; the model is not meaningful afterwards.
    pub fn complete(&mut self, completion: &Completion) -> Result<(), Violation> {
        let id = completion.id;
        let got = completion.result;
        let pending = self
            .pending
            .remove(&id)
            .ok_or(Violation::UnknownCompletion { id })?;
        self.now += 1;
        if let Pending::MustFail {
            invalid,
            read_only,
            failed,
        } = pending
        {
            return match got {
                Err(e) if invalid && e.is_rejection() => Ok(()),
                Err(BlkError::ReadOnly) if read_only => Ok(()),
                // The log may have failed since the request was submitted.
                Err(BlkError::Failed) if failed || self.health != Health::Healthy => {
                    self.health = Health::Failed;
                    Ok(())
                }
                got if invalid => Err(Violation::NotRejected { id, got }),
                got => Err(Violation::NotFailed {
                    id,
                    expected: if read_only {
                        BlkError::ReadOnly
                    } else {
                        BlkError::Failed
                    },
                    got,
                }),
            };
        }
        if matches!(pending, Pending::IdleFlush) && got.is_ok() {
            return Ok(());
        }
        if let Err(error) = got {
            if error == BlkError::Failed && self.health != Health::Healthy {
                // A failure of a failed volume. If the request was a
                // mutation it may or may not have been applied, so it
                // stays among the waiting operations: a read that was
                // already in flight may still show it.
                self.health = Health::Failed;
                return Ok(());
            }
            return Err(Violation::UnexpectedError { id, error });
        }
        match pending {
            Pending::MustFail { .. } | Pending::IdleFlush => {}
            Pending::Mutation { op } => self.acknowledge(op),
            Pending::Flush { at } => self.flushed(at),
            Pending::Read {
                first,
                count,
                since,
            } => {
                let expected = count as usize * BLOCK_USIZE;
                let buf = match &completion.buf {
                    Some(buf) if buf.len() == expected => buf,
                    other => {
                        return Err(Violation::BadBuffer {
                            id,
                            expected,
                            got: other.as_ref().map(|b| b.len()),
                        })
                    }
                };
                if self.recovering.is_some() {
                    self.read_back(first, buf)?;
                } else {
                    self.read(id, first, since, buf)?;
                }
            }
        }
        if self.recovering.is_none() {
            self.compact();
        }
        Ok(())
    }

    fn acknowledge(&mut self, op: usize) {
        self.acked.insert(op, self.now);
        self.last_ack = Some(self.now);
        let mut next = BTreeSet::new();
        for world in &self.worlds {
            match world.waiting.iter().find(|w| w.op == op) {
                // Not placed yet: it was applied at some moment up to now.
                Some(w) => {
                    let inst = Placed {
                        op,
                        lb: w.lb,
                        ub: self.now,
                        flushed: false,
                    };
                    let mut without = world.clone();
                    without.waiting.retain(|x| x.op != op);
                    without.place(&self.cx, inst, &mut next);
                }
                // A read already showed it applied.
                None => {
                    next.insert(world.clone());
                }
            }
        }
        self.set_worlds(next);
    }

    fn flushed(&mut self, at: u64) {
        self.flush_cover = self.flush_cover.max(at);
        let mut next = BTreeSet::new();
        for world in &self.worlds {
            let acked = &self.acked;
            let inside = world.closure(&self.cx, &|p| acked.get(&p.op).is_some_and(|t| *t < at));
            let mut world = world.clone();
            world.mark_flushed(&self.cx, &inside);
            next.insert(world);
        }
        self.set_worlds(next);
    }

    /// Fold what is flushed into the durable content, as far as the reads
    /// still in flight allow.
    fn compact(&mut self) {
        let horizon = self
            .pending
            .values()
            .filter_map(|p| match p {
                Pending::Read { since, .. } => Some(*since),
                _ => None,
            })
            .min()
            .unwrap_or(u64::MAX);
        let mut next = BTreeSet::new();
        for world in &self.worlds {
            // An operation still waiting may yet be placed before anything
            // that was applied after it was submitted.
            let waiting = world.waiting.iter().map(|w| w.lb).min();
            let horizon = waiting.map_or(horizon, |lb| lb.min(horizon));
            let mut world = world.clone();
            world.compact(&self.cx, horizon);
            next.insert(world);
        }
        self.set_worlds(next);
    }

    fn read(&mut self, id: RequestId, first: u64, since: u64, buf: &[u8]) -> Result<(), Violation> {
        let seen: Vec<Option<Cid>> = buf.chunks(BLOCK_USIZE).map(|b| self.cx.lookup(b)).collect();
        let end = first + seen.len() as u64;
        let touches = |m: &Mutation| m.first < end && first < m.first + m.count;
        // Operations in flight over these blocks: applied by now, or not
        // before now.
        let mut candidates = BTreeSet::new();
        for world in &self.worlds {
            candidates.extend(world.settle(&self.cx, self.now, &touches, Some(self.now)));
        }
        let mut explained = vec![false; seen.len()];
        let mut next = BTreeSet::new();
        for world in candidates {
            let certain = world.closure(&self.cx, &|p| p.ub < since);
            let mut all = true;
            for (i, s) in seen.iter().enumerate() {
                let ok = world.explains(&self.cx, first + i as u64, *s, &certain);
                explained[i] |= ok;
                all &= ok;
            }
            if all {
                next.insert(world);
            }
        }
        if next.is_empty() {
            let (block, kind) = match explained.iter().position(|e| !e) {
                Some(i) => {
                    let block = first + i as u64;
                    let bytes = &buf[i * BLOCK_USIZE..(i + 1) * BLOCK_USIZE];
                    (block, self.classify(block, bytes))
                }
                None => (first, Mismatch::Inconsistent),
            };
            return Err(Violation::BadRead { id, block, kind });
        }
        self.set_worlds(next);
        Ok(())
    }

    fn classify(&self, block: u64, bytes: &[u8]) -> Mismatch {
        if self.cx.lookup(bytes).is_some() {
            return Mismatch::Unexpected;
        }
        let empty = Vec::new();
        let meant = self.cx.history.get(&block).unwrap_or(&empty);
        let torn = bytes
            .iter()
            .enumerate()
            .all(|(i, b)| *b == 0 || meant.iter().any(|m| m[i] == *b));
        if torn {
            Mismatch::Torn
        } else {
            Mismatch::Unexpected
        }
    }

    /// The daemon was lost and has returned (rule R9). Requests in flight
    /// stay in flight: the transport reissues them under the same ids.
    /// Nothing acknowledged is lost. Recovery makes the whole log durable,
    /// including whatever the lost daemon had applied of the requests in
    /// flight.
    ///
    /// Not so once the log has failed (rule R11), whether or not the
    /// client has been told yet: the rule that an acknowledged operation
    /// survives the daemon holds for a log that has not failed. The
    /// recovery of the daemon that returns cannot make its prefix
    /// durable, and of what was acknowledged the volume is held only to
    /// what a completed flush covered. The log that failed may have
    /// lost the rest, and a restart does not bring it back. This shows
    /// when the volume is opened again ([`Model::reopened`]).
    pub fn kill(&mut self) {
        if self.recovering.is_some() || self.health != Health::Healthy {
            return;
        }
        self.now += 1;
        let now = self.now;
        let live: Vec<usize> = self
            .pending
            .values()
            .filter_map(|p| match p {
                Pending::Mutation { op } => Some(*op),
                _ => None,
            })
            .collect();
        let mut next = BTreeSet::new();
        for world in &self.worlds {
            for mut world in world.settle(&self.cx, now, &|_| true, None) {
                let all = vec![true; world.tail.len()];
                world.absorb(&self.cx, &all);
                // Every request in flight is issued again and will be
                // applied, perhaps for the second time, from now on.
                world.waiting = live.iter().map(|op| Waiting { op: *op, lb: now }).collect();
                next.insert(world);
            }
        }
        self.set_worlds(next);
        for pending in self.pending.values_mut() {
            if let Pending::Read { since, .. } = pending {
                *since = now;
            }
        }
    }

    /// Power was cut and the device has returned (rule R10). Requests in
    /// flight are gone and will not complete. The device now holds a
    /// prefix of the daemon's order; the model learns which from the
    /// read-back that must follow (see [`Model::readback`]).
    pub fn power_cut(&mut self) {
        self.now += 1;
        self.pending.clear();
        let mut worlds = BTreeSet::new();
        for world in &self.worlds {
            worlds.extend(world.settle(&self.cx, self.now, &|_| true, None));
        }
        let mut needed = BTreeSet::new();
        for world in &worlds {
            for placed in &world.tail {
                needed.extend(self.cx.ops[placed.op].blocks());
            }
        }
        // Blocks on which the worlds disagree about what is durable.
        let blocks: BTreeSet<u64> = worlds
            .iter()
            .flat_map(|w| w.durable.keys().copied())
            .collect();
        for block in blocks {
            let mut cells = worlds.iter().map(|w| self.cx.cell(&w.durable, block));
            if let Some(first) = cells.next() {
                if cells.any(|c| c != first) {
                    needed.insert(block);
                }
            }
        }
        self.set_worlds(worlds);
        self.recovering = Some((needed, Readback::new()));
        // Nothing to read back means nothing to decide.
        let _ = self.finish_recovery();
    }

    /// A failed volume was repaired and opened again, for a new client
    /// (rule R11). Requests in flight are gone. The volume serves again,
    /// and holds at least what every completed flush covered, on either
    /// side of the failure.
    ///
    /// As after a power cut, the volume holds a prefix of the daemon's
    /// order that includes everything flushed, and the model learns
    /// which from a read-back (see [`Model::readback`]). Of the
    /// operations no flush covered, any may be missing, but not an
    /// earlier one without the later ones.
    pub fn reopened(&mut self) {
        self.health = Health::Healthy;
        self.power_cut();
    }

    fn read_back(&mut self, first: u64, buf: &[u8]) -> Result<(), Violation> {
        for (i, bytes) in buf.chunks(BLOCK_USIZE).enumerate() {
            let block = first + i as u64;
            let Some(cid) = self.cx.lookup(bytes) else {
                // Content the model has never seen fits no prefix.
                return Err(Violation::BadRecovery {
                    block,
                    kind: self.classify(block, bytes),
                });
            };
            let cid = Some(cid);
            let Some((needed, seen)) = &mut self.recovering else {
                return Ok(());
            };
            if needed.contains(&block) {
                seen.entry(block).or_default().push(cid);
                continue;
            }
            // No unflushed operation touched it: every world agrees.
            let cell = match self.worlds.first() {
                Some(world) => self.cx.cell(&world.durable, block),
                None => continue,
            };
            if !cid.is_some_and(|c| cell.contains(&c)) {
                return Err(Violation::BadRecovery {
                    block,
                    kind: self.classify(block, bytes),
                });
            }
        }
        self.finish_recovery()
    }

    fn finish_recovery(&mut self) -> Result<(), Violation> {
        let Some((needed, seen)) = &self.recovering else {
            return Ok(());
        };
        if needed.iter().any(|b| !seen.contains_key(b)) {
            return Ok(());
        }
        let mut next = BTreeSet::new();
        for world in &self.worlds {
            world.prefixes(&self.cx, seen, &mut next);
        }
        if next.is_empty() {
            // Name a block that no prefix of any world can hold.
            let bad = seen.iter().find(|(block, answers)| {
                !self.worlds.iter().any(|world| {
                    let nothing_certain = vec![false; world.tail.len()];
                    answers
                        .iter()
                        .all(|s| world.explains(&self.cx, **block, *s, &nothing_certain))
                })
            });
            let violation = match bad {
                Some((block, _)) => Violation::BadRecovery {
                    block: *block,
                    kind: Mismatch::Unexpected,
                },
                None => Violation::BadRecovery {
                    block: seen.keys().next().copied().unwrap_or(0),
                    kind: Mismatch::Inconsistent,
                },
            };
            return Err(violation);
        }
        self.set_worlds(next);
        self.recovering = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    //! The model is the yardstick of the suite, so its own judgement is
    //! pinned here with hand-written histories: for each rule, histories
    //! it must accept and histories it must reject.

    use super::*;
    use crate::Buf;

    const B: usize = BLOCK_USIZE;

    #[derive(Clone)]
    struct Client {
        model: Model,
        next: u64,
    }

    fn base_block(base: Base, block: u64) -> Vec<u8> {
        let mut out = vec![0; B];
        base.fill_block(block, &mut out);
        out
    }

    impl Client {
        fn on(base: Base, read_only: bool) -> Self {
            Self {
                model: Model::with_base(Geometry::new(1, 8, read_only).unwrap(), base),
                next: 0,
            }
        }

        fn new() -> Self {
            Self::on(Base::Empty, false)
        }

        fn submit(&mut self, op: Op) -> RequestId {
            self.next += 1;
            let id = RequestId(self.next);
            self.model.submit(&Request { id, op }).unwrap();
            id
        }

        fn done(&mut self, id: RequestId, result: Result<(), BlkError>) -> Result<(), Violation> {
            self.model.complete(&Completion {
                id,
                result,
                buf: None,
            })
        }

        fn ack(&mut self, id: RequestId) {
            self.done(id, Ok(())).unwrap();
        }

        /// Submit a write of `count` blocks, each full of `fill`.
        fn write(&mut self, block: u64, count: usize, fill: u8) -> RequestId {
            self.submit(Op::Write {
                offset: block * BLOCK_SIZE,
                data: Buf::filled(count, fill),
            })
        }

        fn wrote(&mut self, block: u64, count: usize, fill: u8) {
            let id = self.write(block, count, fill);
            self.ack(id);
        }

        fn zeroes(&mut self, block: u64, count: u64) -> RequestId {
            self.submit(Op::WriteZeroes {
                offset: block * BLOCK_SIZE,
                len: count * BLOCK_SIZE,
            })
        }

        fn discard(&mut self, block: u64, count: u64) -> RequestId {
            self.submit(Op::Discard {
                offset: block * BLOCK_SIZE,
                len: count * BLOCK_SIZE,
            })
        }

        fn read(&mut self, block: u64, count: usize) -> RequestId {
            self.submit(Op::Read {
                offset: block * BLOCK_SIZE,
                buf: Buf::zeroed(count),
            })
        }

        fn returns(&mut self, id: RequestId, bytes: Vec<u8>) -> Result<(), Violation> {
            self.model.complete(&Completion {
                id,
                result: Ok(()),
                buf: Buf::from_blocks(&bytes),
            })
        }

        /// Read blocks from `block` on with nothing else going on; the
        /// target answers one block full of each of `fills`.
        fn sees(&mut self, block: u64, fills: &[u8]) -> Result<(), Violation> {
            let id = self.read(block, fills.len());
            let bytes = fills.iter().flat_map(|f| vec![*f; B]).collect();
            self.returns(id, bytes)
        }

        /// As `sees`, on a copy: "would this answer be accepted?" without
        /// committing the model to it.
        fn would_see(&self, block: u64, fills: &[u8]) -> bool {
            self.clone().sees(block, fills).is_ok()
        }

        fn flush(&mut self) {
            let id = self.submit(Op::Flush);
            self.ack(id);
        }
    }

    /// R3: a block never written reads as the base and as nothing else,
    /// on an empty volume and on a clone.
    #[test]
    fn fresh_block_reads_as_the_base_only() {
        let mut c = Client::new();
        c.sees(0, &[0]).unwrap();
        assert!(matches!(c.sees(0, &[7]), Err(Violation::BadRead { .. })));

        let base = Base::Pattern(3);
        let mut c = Client::on(base, false);
        let r = c.read(5, 1);
        c.returns(r, base_block(base, 5)).unwrap();
        let r = c.read(5, 1);
        assert!(c.returns(r, vec![0; B]).is_err());
        let r = c.read(5, 1);
        assert!(c.returns(r, base_block(base, 6)).is_err());
    }

    /// R3, R8: a read received after an operation was acknowledged
    /// observes it, and an operation submitted after another was
    /// acknowledged follows it.
    #[test]
    fn read_sees_the_latest_acknowledged_write() {
        let mut c = Client::new();
        c.wrote(1, 1, 1);
        c.wrote(1, 1, 2);
        assert!(!c.would_see(1, &[1]));
        assert!(!c.would_see(1, &[0]));
        c.sees(1, &[2]).unwrap();
    }

    /// R8: two overlapping writes in flight together may land either way
    /// round, but whole: the same one wins on every block they share.
    #[test]
    fn overlapping_writes_in_flight_land_either_way_but_whole() {
        let mut c = Client::new();
        let first = c.write(0, 2, 1);
        let second = c.write(0, 2, 2);
        c.ack(second);
        c.ack(first);
        assert!(c.would_see(0, &[1, 1]));
        assert!(c.would_see(0, &[2, 2]));
        assert_eq!(
            c.sees(0, &[1, 2]),
            Err(Violation::BadRead {
                id: RequestId(3),
                block: 0,
                kind: Mismatch::Inconsistent
            })
        );
    }

    /// R8: once a read has shown which of two overlapping writes won, the
    /// answer cannot change.
    #[test]
    fn the_order_a_read_showed_is_settled() {
        let mut c = Client::new();
        let first = c.write(0, 2, 1);
        let second = c.write(1, 2, 2);
        c.ack(first);
        c.ack(second);
        assert_eq!(c.model.worlds(), 2);
        c.sees(1, &[1]).unwrap();
        assert_eq!(c.model.worlds(), 1);
        assert!(!c.would_see(1, &[2]));
        c.sees(0, &[1, 1, 2]).unwrap();
    }

    /// R3: a read that overlaps an operation in flight sees, for each
    /// block, the content before it or after it. This is the one place
    /// where blocks may differ.
    #[test]
    fn read_racing_a_write_sees_before_or_after_per_block() {
        let mut c = Client::new();
        c.wrote(0, 2, 1);
        for answer in [[1, 1], [2, 2], [1, 2], [2, 1]] {
            let mut c = c.clone();
            let r = c.read(0, 2);
            let _w = c.write(0, 2, 2);
            let bytes = answer.iter().flat_map(|f| vec![*f; B]).collect();
            c.returns(r, bytes).unwrap();
        }
        // But never a mix inside a block.
        let r = c.read(0, 1);
        let _w = c.write(0, 1, 2);
        let mut mixed = vec![1u8; B];
        mixed[B / 2..].fill(2);
        assert_eq!(
            c.returns(r, mixed),
            Err(Violation::BadRead {
                id: r,
                block: 0,
                kind: Mismatch::Torn
            })
        );
    }

    /// R3: a write that a read has shown is applied, although it is not
    /// acknowledged yet; a later read cannot go back.
    #[test]
    fn a_write_seen_by_a_read_stays_applied() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let _w = c.write(0, 1, 2);
        c.sees(0, &[2]).unwrap();
        assert!(!c.would_see(0, &[1]));
    }

    /// R2: an invalid request must complete with a rejection; `Ok` and
    /// any other error are violations.
    #[test]
    fn invalid_request_must_be_rejected() {
        let op = Op::Discard {
            offset: 1,
            len: BLOCK_SIZE,
        };
        let rejection = BlkError::Misaligned {
            offset: 1,
            len: BLOCK_SIZE,
        };
        let mut c = Client::new();
        let id = c.submit(op.clone());
        c.done(id, Err(rejection)).unwrap();
        let id = c.submit(op.clone());
        assert!(matches!(
            c.done(id, Ok(())),
            Err(Violation::NotRejected { .. })
        ));
        let id = c.submit(op);
        assert!(matches!(
            c.done(id, Err(BlkError::Failed)),
            Err(Violation::NotRejected { .. })
        ));
    }

    /// R8: every request completes exactly once; a second completion for
    /// the same id is a violation.
    #[test]
    fn double_completion_is_a_violation() {
        let mut c = Client::new();
        let w = c.write(0, 1, 1);
        c.ack(w);
        assert_eq!(
            c.done(w, Ok(())),
            Err(Violation::UnknownCompletion { id: w })
        );
    }

    /// R7, R10: what was acknowledged before a completed flush was
    /// submitted survives a power cut; the older content is not accepted.
    #[test]
    fn flushed_write_survives_power_cut() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.flush();
        c.wrote(0, 1, 2);
        c.model.power_cut();
        assert_eq!(c.model.readback(), vec![0]);
        assert!(!c.would_see(0, &[0]));
        assert!(c.would_see(0, &[1]));
        assert!(c.would_see(0, &[2]));
    }

    /// R7: a write still in flight when the flush is submitted is not
    /// covered by it, even if it is acknowledged before the flush
    /// completes.
    #[test]
    fn flush_does_not_cover_a_write_in_flight() {
        let mut c = Client::new();
        let w = c.write(0, 1, 1);
        let f = c.submit(Op::Flush);
        c.ack(w);
        c.ack(f);
        c.model.power_cut();
        assert!(c.would_see(0, &[0]));
        assert!(c.would_see(0, &[1]));
    }

    /// R10: every completed flush counts, not only the last. F1 is
    /// submitted after W1, F2 before it; F2 completes last and covers
    /// less. W1 must still survive.
    #[test]
    fn every_completed_flush_counts() {
        let mut c = Client::new();
        let early = c.submit(Op::Flush);
        c.wrote(0, 1, 1);
        let late = c.submit(Op::Flush);
        c.ack(late);
        c.ack(early);
        c.model.power_cut();
        // Nothing unflushed: nothing to read back, and only W1 is there.
        assert!(c.model.readback().is_empty());
        assert!(!c.would_see(0, &[0]));
        c.sees(0, &[1]).unwrap();
    }

    /// R10: a power cut leaves a prefix of the order. Of two writes, one
    /// acknowledged before the other was submitted, the later cannot
    /// survive without the earlier.
    #[test]
    fn power_cut_leaves_a_prefix() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.wrote(1, 1, 2);
        c.model.power_cut();
        assert_eq!(c.model.readback(), vec![0, 1]);
        assert!(c.would_see(0, &[0, 0]));
        assert!(c.would_see(0, &[1, 0]));
        assert!(c.would_see(0, &[1, 2]));
        assert_eq!(
            c.sees(0, &[0, 2]),
            Err(Violation::BadRecovery {
                block: 0,
                kind: Mismatch::Inconsistent
            })
        );
    }

    /// R10: two writes that were in flight together and do not overlap
    /// have no order between them, so either may survive alone.
    #[test]
    fn concurrent_writes_may_survive_in_any_combination() {
        let mut c = Client::new();
        let a = c.write(0, 1, 1);
        let b = c.write(1, 1, 2);
        c.ack(a);
        c.ack(b);
        c.model.power_cut();
        for answer in [[0, 0], [1, 0], [0, 2], [1, 2]] {
            assert!(c.would_see(0, &answer), "{answer:?}");
        }
    }

    /// R10: every operation in the recovered prefix is whole. Half of a
    /// two-block write is not a prefix, and half a block is torn.
    #[test]
    fn power_cut_keeps_operations_whole() {
        let mut c = Client::new();
        c.wrote(0, 2, 1);
        c.model.power_cut();
        assert!(c.would_see(0, &[1, 1]));
        assert!(c.would_see(0, &[0, 0]));
        assert!(!c.would_see(0, &[1, 0]));
        let r = c.read(0, 2);
        let mut torn = vec![1u8; 2 * B];
        torn[B / 2..B].fill(0);
        torn[B] = 1;
        assert_eq!(
            c.returns(r, [&torn[..B], &[0u8; B][..]].concat()),
            Err(Violation::BadRecovery {
                block: 0,
                kind: Mismatch::Torn
            })
        );
    }

    /// R10: a write in flight at the power cut may have landed, whole.
    #[test]
    fn in_flight_write_may_land_at_power_cut() {
        let mut c = Client::new();
        let _w = c.write(0, 2, 1);
        c.model.power_cut();
        assert_eq!(c.model.in_flight(), 0);
        assert!(c.would_see(0, &[0, 0]));
        assert!(c.would_see(0, &[1, 1]));
        assert!(!c.would_see(0, &[0, 1]));
    }

    /// R10: what is read after a power cut is stable, and a second power
    /// cut does not take it back, flush or no flush.
    #[test]
    fn recovered_state_is_stable_and_durable() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.model.power_cut();
        c.sees(0, &[1]).unwrap();
        assert!(!c.would_see(0, &[0]));
        c.model.power_cut();
        assert!(c.model.readback().is_empty());
        assert!(!c.would_see(0, &[0]));
        c.sees(0, &[1]).unwrap();
    }

    /// The model's own rule: after a power cut the client reads back what
    /// may have changed before it submits anything else.
    #[test]
    fn readback_comes_before_new_requests() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.model.power_cut();
        let request = Request {
            id: RequestId(99),
            op: Op::Flush,
        };
        assert_eq!(
            c.model.submit(&request),
            Err(Violation::ReadbackRequired { id: RequestId(99) })
        );
        c.sees(0, &[0]).unwrap();
        c.model.submit(&request).unwrap();
    }

    /// R6: after a discard a block reads as its content before the
    /// discard, zeros or the base, in any alternation, and as nothing
    /// else.
    #[test]
    fn discarded_block_reads_old_zeros_or_base() {
        let base = Base::Pattern(9);
        let mut c = Client::on(base, false);
        c.wrote(0, 1, 1);
        let d = c.discard(0, 1);
        c.ack(d);
        for _ in 0..2 {
            c.sees(0, &[0]).unwrap();
            c.sees(0, &[1]).unwrap();
            let r = c.read(0, 1);
            c.returns(r, base_block(base, 0)).unwrap();
        }
        assert!(c.sees(0, &[9]).is_err());
    }

    /// R6: a discard is never applied to data written after it, and a
    /// write ends the block's unspecified state.
    #[test]
    fn write_after_discard_wins() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let d = c.discard(0, 1);
        c.ack(d);
        c.wrote(0, 1, 2);
        assert!(!c.would_see(0, &[0]));
        assert!(!c.would_see(0, &[1]));
        c.sees(0, &[2]).unwrap();
    }

    /// R5: write-zeroes reads as zeros, hides the base, and with a flush
    /// survives a power cut like a write.
    #[test]
    fn write_zeroes_hides_the_base_and_is_durable() {
        let base = Base::Pattern(4);
        let mut c = Client::on(base, false);
        let z = c.zeroes(0, 1);
        c.ack(z);
        let r = c.read(0, 1);
        assert!(c.clone().returns(r, base_block(base, 0)).is_err());
        c.returns(r, vec![0; B]).unwrap();
        c.flush();
        c.model.power_cut();
        let r = c.read(0, 1);
        assert!(c.clone().returns(r, base_block(base, 0)).is_err());
        c.returns(r, vec![0; B]).unwrap();
    }

    /// R9: a daemon loss loses nothing acknowledged, keeps requests in
    /// flight, and makes the log durable: a power cut right after it
    /// cannot lose the acknowledged write either.
    #[test]
    fn kill_keeps_acknowledged_writes_and_in_flight_requests() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let w2 = c.write(1, 1, 2);
        c.model.kill();
        assert_eq!(c.model.in_flight(), 1);
        assert!(!c.would_see(0, &[0]));
        c.ack(w2);
        c.sees(0, &[1, 2]).unwrap();
        c.model.kill();
        c.model.power_cut();
        assert!(c.model.readback().is_empty());
        assert!(!c.would_see(0, &[0, 0]));
        c.sees(0, &[1, 2]).unwrap();
    }

    /// R7, R9: a flush received by a new instance covers what the old one
    /// acknowledged.
    #[test]
    fn flush_after_kill_covers_the_earlier_instance() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.model.kill();
        c.flush();
        c.model.power_cut();
        assert!(!c.would_see(0, &[0]));
        c.sees(0, &[1]).unwrap();
    }

    /// R9: an operation in flight
    /// at a daemon loss may be applied before the loss and again after
    /// the reissue. W1 covers blocks 0 and 1, W2 block 1. Without a
    /// daemon loss, "W1 won block 1, then a power cut left W1 on block 0
    /// and W2 on block 1" has no explanation. With one, it has: the log
    /// is W1, W2, W1 and the cut lost the last record.
    #[test]
    fn a_reissued_operation_may_be_in_the_order_twice() {
        let history = |kill: bool| {
            let mut c = Client::new();
            let w1 = c.write(0, 2, 1);
            let w2 = c.write(1, 1, 2);
            c.ack(w2);
            if kill {
                c.model.kill();
            }
            c.ack(w1);
            c.sees(1, &[1]).unwrap();
            c.model.power_cut();
            c.would_see(0, &[1, 2])
        };
        assert!(!history(false));
        assert!(history(true));
    }

    /// R3, R7: a read stays in flight across a write to its block and a
    /// completed flush, and then returns the content from before the
    /// write. It overlapped the write, so that is allowed; the flush must
    /// not make the model forget the earlier state. (Found in review.)
    #[test]
    fn read_held_across_a_flush_may_see_the_state_before() {
        for answer in [0u8, 1] {
            let mut c = Client::new();
            let r = c.read(0, 1);
            c.wrote(0, 1, 1);
            c.flush();
            c.returns(r, vec![answer; B]).unwrap();
            // The flush still counts: the write survives a power cut.
            c.model.power_cut();
            assert!(!c.would_see(0, &[0]));
            c.sees(0, &[1]).unwrap();
        }
        // A read submitted after the write was acknowledged cannot.
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let r = c.read(0, 1);
        c.flush();
        assert!(c.returns(r, vec![0; B]).is_err());
    }

    /// R3: the same across two flushes and a second write: every state
    /// the block had while the read was in flight stays allowed, and the
    /// model catches up once the read is done.
    #[test]
    fn read_held_across_flushes_sees_any_state_in_its_flight() {
        for answer in [0u8, 1, 2] {
            let mut c = Client::new();
            let r = c.read(0, 2);
            c.wrote(0, 2, 1);
            c.flush();
            c.wrote(0, 1, 2);
            c.flush();
            let mut bytes = vec![answer; B];
            bytes.extend(vec![1; B]);
            c.returns(r, bytes).unwrap();
            assert!(!c.would_see(0, &[1, 1]));
            c.sees(0, &[2, 1]).unwrap();
        }
    }

    /// R7, R8: X and Y overlap and are in flight together; the daemon's
    /// order is X, Y. Y is acknowledged, a flush completes, and only then
    /// is X acknowledged. Reads return Y. The flush must not fix Y's place
    /// before X has one. (Found in review; B1.)
    #[test]
    fn write_held_across_a_flush_may_precede_what_was_flushed() {
        for winner in [1u8, 2] {
            let mut c = Client::new();
            let x = c.write(0, 1, 1);
            let y = c.write(0, 1, 2);
            c.ack(y);
            c.flush();
            c.ack(x);
            c.sees(0, &[winner]).unwrap();
            assert!(!c.would_see(0, &[3 - winner]));
        }
        // And across a power cut: Y is flushed, so it is there; X, in
        // flight, may be under it or over it, or absent.
        let mut c = Client::new();
        let _x = c.write(0, 2, 1);
        let y = c.write(0, 1, 2);
        c.ack(y);
        c.flush();
        c.model.power_cut();
        assert!(c.would_see(0, &[2, 0]));
        assert!(c.would_see(0, &[2, 1]));
        assert!(c.would_see(0, &[1, 1]));
        assert!(!c.would_see(0, &[0, 0]));
        assert!(!c.would_see(0, &[1, 0]));
    }

    /// R2, R11, R12: where a request is wrong in two ways, either error
    /// is accepted, and nothing else is.
    #[test]
    fn either_error_is_accepted_when_two_apply() {
        let invalid = Op::Discard {
            offset: 1,
            len: BLOCK_SIZE,
        };
        let rejection = BlkError::Misaligned {
            offset: 1,
            len: BLOCK_SIZE,
        };
        // Invalid, on a failed volume.
        let mut c = Client::new();
        c.model.log_failed();
        let f = c.submit(Op::Flush);
        c.done(f, Err(BlkError::Failed)).unwrap();
        for answer in [rejection, BlkError::Failed] {
            let id = c.submit(invalid.clone());
            c.done(id, Err(answer)).unwrap();
        }
        let id = c.submit(invalid.clone());
        assert!(c.clone().done(id, Ok(())).is_err());
        assert!(c.done(id, Err(BlkError::ReadOnly)).is_err());
        // Invalid, on a read-only target.
        let mut c = Client::on(Base::Empty, true);
        for answer in [rejection, BlkError::ReadOnly] {
            let id = c.submit(invalid.clone());
            c.done(id, Err(answer)).unwrap();
        }
        let id = c.submit(invalid);
        assert!(c.done(id, Err(BlkError::Failed)).is_err());
    }

    /// R8: placing an operation must consider every order, not only the
    /// positions in one listing of the operations so far. A discard of
    /// block 3 (D) is in flight; a write to block 5 (Q) is submitted and
    /// acknowledged; a write to blocks 0 to 3 (W) is submitted; D is
    /// acknowledged, then W. D and Q do not overlap and have no order, W
    /// must follow Q, and W may still precede D: block 3 may then read as
    /// zeros. (Found when `MemTarget` learnt to append on receipt.)
    #[test]
    fn an_operation_may_precede_one_listed_before_its_predecessor() {
        let mut c = Client::new();
        let d = c.discard(3, 1);
        c.wrote(5, 1, 7);
        let w = c.write(0, 4, 1);
        c.ack(d);
        c.ack(w);
        assert!(c.would_see(0, &[1, 1, 1, 1, 0, 7]));
        assert!(c.would_see(0, &[1, 1, 1, 0, 0, 7]));
        assert!(!c.would_see(0, &[1, 1, 0, 1, 0, 7]));
    }

    /// R11: a failed volume that is opened again serves again, and
    /// holds what a completed flush covered; what no flush covered may
    /// be there or not, as a prefix.
    #[test]
    fn reopened_volume_serves_again_and_keeps_what_was_flushed() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let f = c.submit(Op::Flush);
        c.ack(f);
        c.wrote(1, 1, 2);
        c.model.log_failed();
        let r = c.read(0, 1);
        c.done(r, Err(BlkError::Failed)).unwrap();
        c.model.reopened();
        assert_eq!(c.model.readback(), vec![1]);
        // The flushed write is not open to question.
        assert!(!c.would_see(0, &[0, 0]));
        assert!(c.would_see(0, &[1, 0]));
        c.sees(0, &[1, 2]).unwrap();
        // And the volume takes requests again.
        c.wrote(0, 1, 3);
        c.sees(0, &[3, 2]).unwrap();
    }

    /// R11 over R9: a daemon restart on a failed volume recovers nothing.
    /// A write that was acknowledged and that no completed flush covered
    /// may be gone when the volume is opened again, restart or not: the
    /// flush that would have covered it is what failed.
    #[test]
    fn a_restart_does_not_make_a_failed_volume_keep_what_was_not_flushed() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let f = c.submit(Op::Flush);
        c.ack(f);
        c.wrote(0, 1, 2);
        c.model.log_failed();
        let f = c.submit(Op::Flush);
        c.done(f, Err(BlkError::Failed)).unwrap();
        c.model.kill();
        c.model.reopened();
        assert_eq!(c.model.readback(), vec![0]);
        assert!(c.would_see(0, &[2]));
        assert!(!c.would_see(0, &[0]));
        c.sees(0, &[1]).unwrap();
    }

    /// R11 over R9, before anyone was told: the log fails, no request
    /// completes, the daemon restarts. The rule is about the log having
    /// failed, not about a failure having been reported: the write that
    /// no flush covered may be gone when the volume is opened again.
    #[test]
    fn a_restart_after_the_log_failed_unreported_keeps_only_what_was_flushed() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let f = c.submit(Op::Flush);
        c.ack(f);
        c.wrote(0, 1, 2);
        c.model.log_failed();
        c.model.kill();
        c.model.reopened();
        assert_eq!(c.model.readback(), vec![0]);
        assert!(c.would_see(0, &[2]));
        c.sees(0, &[1]).unwrap();
    }

    /// R11: on a failed log a flush with nothing to make durable may
    /// complete either way, and promises nothing: here every
    /// acknowledged write is covered by a completed flush, or there is
    /// none. With an acknowledged write that no flush covers, it must
    /// fail. Once a failure has been delivered, every flush fails.
    #[test]
    fn a_flush_with_nothing_to_make_durable_need_not_notice_the_failure() {
        for covered in [false, true] {
            let mut c = Client::new();
            if covered {
                c.wrote(0, 1, 1);
                let f = c.submit(Op::Flush);
                c.ack(f);
            }
            c.model.log_failed();
            let f = c.submit(Op::Flush);
            c.clone().done(f, Err(BlkError::Failed)).unwrap();
            c.done(f, Ok(())).unwrap();
            // Still nothing to make durable: either way again.
            let f = c.submit(Op::Flush);
            c.done(f, Ok(())).unwrap();
            // A write that is acknowledged now is something to make
            // durable, and the flush behind it must fail.
            c.wrote(1, 1, 2);
            let f = c.submit(Op::Flush);
            assert!(matches!(
                c.clone().done(f, Ok(())),
                Err(Violation::NotFailed { .. })
            ));
            c.done(f, Err(BlkError::Failed)).unwrap();
            let f = c.submit(Op::Flush);
            assert!(c.clone().done(f, Ok(())).is_err());
            c.done(f, Err(BlkError::Failed)).unwrap();
        }
    }

    /// R11: once the log has failed, a flush must fail, and after the
    /// first failure every request must, reads included.
    #[test]
    fn failed_volume_must_fail_everything() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        c.model.log_failed();
        // Before any failure is delivered, a read may still succeed.
        c.sees(0, &[1]).unwrap();
        let f = c.submit(Op::Flush);
        assert!(matches!(
            c.clone().done(f, Ok(())),
            Err(Violation::NotFailed { .. })
        ));
        c.done(f, Err(BlkError::Failed)).unwrap();
        let r = c.read(0, 1);
        assert!(matches!(
            c.clone().returns(r, vec![1; B]),
            Err(Violation::NotFailed { .. })
        ));
        c.done(r, Err(BlkError::Failed)).unwrap();
        // Also after a daemon restart.
        c.model.kill();
        let w = c.write(0, 1, 2);
        assert!(c.clone().done(w, Ok(())).is_err());
        c.done(w, Err(BlkError::Failed)).unwrap();
    }

    /// R2, R11: a request that must fail for a reason known when it was
    /// submitted (invalid; a mutation on a read-only target) may be
    /// answered `Failed` if the log fails while it is in flight. The
    /// model must judge by the volume's health at completion, not at
    /// submission. (Found in review.)
    #[test]
    fn a_request_in_flight_when_the_log_fails_may_report_the_failure() {
        let invalid = Op::Discard {
            offset: 1,
            len: BLOCK_SIZE,
        };
        let mut c = Client::new();
        let id = c.submit(invalid);
        c.model.log_failed();
        c.done(id, Err(BlkError::Failed)).unwrap();

        let mut c = Client::on(Base::Empty, true);
        let id = c.write(0, 1, 1);
        c.model.log_failed();
        c.done(id, Err(BlkError::Failed)).unwrap();

        // Without a log failure, `Failed` is not an answer.
        let mut c = Client::on(Base::Empty, true);
        let id = c.write(0, 1, 1);
        assert!(c.done(id, Err(BlkError::Failed)).is_err());
    }

    /// R7, R11: a flush that was in flight when the log failed may still
    /// succeed, if its work was done before the failure: the client cannot
    /// tell a late completion from a late flush. What it covered is then
    /// durable. A flush submitted after the failure must fail.
    #[test]
    fn a_flush_in_flight_when_the_log_fails_may_succeed() {
        let mut c = Client::new();
        c.wrote(0, 1, 1);
        let early = c.submit(Op::Flush);
        c.model.log_failed();
        let late = c.submit(Op::Flush);
        assert!(matches!(
            c.clone().done(late, Ok(())),
            Err(Violation::NotFailed { .. })
        ));
        // It may also fail.
        c.clone().done(early, Err(BlkError::Failed)).unwrap();
        c.done(early, Ok(())).unwrap();
        // The model holds the target to that success.
        c.model.power_cut();
        assert!(c.model.readback().is_empty());
    }

    /// R11: a write that fails because the log failed may or may not have
    /// been applied in memory. A read that was already in flight, and so
    /// may still succeed, may return it.
    #[test]
    fn a_write_that_failed_with_the_log_may_still_be_seen() {
        for answer in [0u8, 1] {
            let mut c = Client::new();
            let w = c.write(0, 1, 1);
            let r = c.read(0, 1);
            c.model.log_failed();
            c.done(w, Err(BlkError::Failed)).unwrap();
            c.returns(r, vec![answer; B]).unwrap();
        }
    }

    /// R12: on a read-only target a mutation must fail with `ReadOnly`
    /// and a flush must succeed.
    #[test]
    fn read_only_target_must_refuse_mutations() {
        let mut c = Client::on(Base::Empty, true);
        for id in [c.write(0, 1, 1), c.zeroes(0, 1), c.discard(0, 1)] {
            assert!(matches!(
                c.clone().done(id, Ok(())),
                Err(Violation::NotFailed { .. })
            ));
            c.done(id, Err(BlkError::ReadOnly)).unwrap();
        }
        c.flush();
        c.sees(0, &[0]).unwrap();
    }

    /// A read must hand back a buffer of the length asked for.
    #[test]
    fn read_without_buffer_is_a_violation() {
        let mut c = Client::new();
        let r = c.read(0, 1);
        assert!(matches!(
            c.done(r, Ok(())),
            Err(Violation::BadBuffer { .. })
        ));
    }
}

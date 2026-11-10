//! The committed extent map: immutable, per shard, columnar
//! (design §7.2; `skynbd-multinode-design.md` D-M10).
//!
//! Everything a volume has committed is read-only until the next commit,
//! and a commit rewrites only the shards it touched. So the committed map
//! is not an interval map that happens not to be mutated — it is a sorted
//! index, and the mutation machinery a `BTreeMap` carries to serve
//! `insert()` is dead weight. Measured against the previous representation
//! on a 100 GiB volume with 644,619 extents (`tests/extent_index.rs`):
//! 167.6 → 21.0 B/extent resident, and a commit churns 2 MB instead of
//! 105 MB because untouched shards are shared rather than copied.
//!
//! Two things make the layout compact, and both depend on the unit being
//! the *shard*:
//!
//! - every field fits in `u32` — an offset inside a 1 GiB shard, a length
//!   bounded by the same, an index into the object table, and an offset
//!   inside an 8 MiB segment. [`ShardBuilder::push`] checks rather than
//!   assumes this, so an unsupported configuration fails loudly instead of
//!   truncating a target offset and returning the wrong bytes.
//! - the object table is per *volume*. A segment's ~128 extents scatter
//!   across shards, so a per-shard table would hold a near-complete copy
//!   of the segment list in every shard — measured at 20 MB of a 34 MB
//!   prototype, more than the columns themselves.
//!
//! The write path is untouched: the overlay stays a mutable `ExtentMap`,
//! and [`merge`] folds it into a shard at commit time. The split is
//! writable-head versus read-only-export, not raw versus thin.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use skynbd_format::{Codec, EmapObj, EmapShard, Node, EMAP_ZERO};
use skynbd_store::ObjectStore;
use skynbd_tree::Run;
use skynbd_types::{DataRef, Extent, NodeHash, ObjectHash, SegmentId, Target};

use crate::{EngineError, Result};

/// `target[i]` for an extent of explicit zeroes, which names no object.
const ZERO: u32 = u32::MAX;

/// A backing object's identity, without the per-extent offset — the unit
/// the object table interns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ObjRef {
    Seg(SegmentId),
    Cas(ObjectHash),
}

/// One shard's committed extents: sorted by start, non-overlapping,
/// maximally coalesced, immutable once built.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Shard {
    /// Offset within the shard.
    start: Vec<u32>,
    len: Vec<u32>,
    /// Index into the volume's object table, or [`ZERO`].
    target: Vec<u32>,
    /// Offset within the backing object.
    offset: Vec<u32>,
}

impl Shard {
    fn is_empty(&self) -> bool {
        self.start.is_empty()
    }

    fn count(&self) -> usize {
        self.start.len()
    }

    /// The extent at `i`, absolute.
    fn extent(&self, i: usize, base: u64, objects: &[ObjRef]) -> Extent {
        let target = match self.target[i] {
            ZERO => Target::Zero,
            t => {
                let offset = self.offset[i] as u64;
                match objects[t as usize] {
                    ObjRef::Seg(segment) => Target::Data(DataRef::Seg { segment, offset }),
                    ObjRef::Cas(hash) => Target::Data(DataRef::Cas { hash, offset }),
                }
            }
        };
        Extent {
            lba_start: base + self.start[i] as u64,
            len: self.len[i] as u64,
            target,
        }
    }

    /// Index of the last extent starting at or before `at`, if any.
    fn seek(&self, at: u32) -> usize {
        self.start.partition_point(|&s| s <= at).saturating_sub(1)
    }
}

/// Accumulates one shard from extents pushed in ascending order.
struct ShardBuilder {
    shard: Shard,
    base: u64,
}

impl ShardBuilder {
    /// Push the next extent. Coalesces with the previous one when they
    /// describe a single run, so the index is maximally merged however it
    /// was built — including from trees written before fold-time
    /// coalescing existed.
    fn push(&mut self, e: &Extent, objects: &mut ObjectTable) -> Result<()> {
        if let Some(prev) = self.last(objects) {
            if prev.continues_into(e) {
                let i = self.shard.count() - 1;
                let merged = prev.len + e.len;
                self.shard.len[i] = fit(merged, "extent length")?;
                return Ok(());
            }
        }
        let start = fit(e.lba_start - self.base, "shard offset")?;
        let len = fit(e.len, "extent length")?;
        let (target, offset) = match &e.target {
            Target::Zero => (ZERO, 0),
            Target::Data(d) => {
                let (obj, offset) = match d {
                    DataRef::Seg { segment, offset } => (ObjRef::Seg(*segment), *offset),
                    DataRef::Cas { hash, offset } => (ObjRef::Cas(*hash), *offset),
                };
                (objects.intern(obj), fit(offset, "object offset")?)
            }
        };
        self.shard.start.push(start);
        self.shard.len.push(len);
        self.shard.target.push(target);
        self.shard.offset.push(offset);
        Ok(())
    }

    fn last(&self, objects: &ObjectTable) -> Option<Extent> {
        let n = self.shard.count();
        (n > 0).then(|| self.shard.extent(n - 1, self.base, &objects.refs))
    }

    fn finish(self) -> Option<Arc<Shard>> {
        (!self.shard.is_empty()).then(|| Arc::new(self.shard))
    }
}

/// Where the stream stops being ascending and disjoint, if anywhere.
fn first_disorder(extents: &[Extent]) -> Option<u64> {
    extents
        .windows(2)
        .find(|w| w[1].lba_start < w[0].end())
        .map(|w| w[1].lba_start)
}

/// Last-writer-wins over a stream that may overlap or go backwards —
/// exactly what `ExtentMap` did when `load_commit_state` inserted every
/// extent in walk order. Only reached for a tree that is already
/// inconsistent, so it favours obvious correctness over speed.
fn repair(extents: &[Extent]) -> Vec<Extent> {
    let mut map: skynbd_tree::ExtentMap<Extent> = skynbd_tree::ExtentMap::new();
    for e in extents {
        map.insert(e.lba_start, e.len, e.clone());
    }
    let mut out: Vec<Extent> = Vec::with_capacity(extents.len());
    for (start, len, original) in map.iter() {
        // `ExtentMap` keeps the original value across splits, so a
        // truncated piece still has to be re-based.
        let piece = original.slice(start, start + len);
        match out.last_mut() {
            Some(prev) if prev.continues_into(&piece) => prev.absorb(&piece),
            _ => out.push(piece),
        }
    }
    out
}

/// One shard's columns in wire form: starts become gaps. Never fails on
/// a well-formed shard; a non-sorted one is caught rather than encoded.
fn encode_shard(s: &Shard) -> Result<EmapShard> {
    let mut out = EmapShard::default();
    let mut prev_end: u32 = 0;
    for i in 0..s.count() {
        let gap = s.start[i].checked_sub(prev_end).ok_or_else(|| {
            EngineError::Corrupt("committed shard is not sorted; refusing to write emap".into())
        })?;
        out.gap.push(gap);
        out.len.push(s.len[i]);
        out.target.push(if s.target[i] == ZERO {
            EMAP_ZERO
        } else {
            s.target[i]
        });
        out.offset.push(s.offset[i]);
        prev_end = s.start[i] + s.len[i];
    }
    Ok(out)
}

/// The packed layout is only sound while every field fits; a volume that
/// violates it must fail to open rather than silently read wrong bytes.
fn fit(v: u64, what: &str) -> Result<u32> {
    u32::try_from(v).map_err(|_| {
        EngineError::Corrupt(format!(
            "{what} {v} exceeds the {} the packed extent index supports",
            u32::MAX
        ))
    })
}

/// Distinct backing objects, shared by every shard of a volume. Indices
/// are stable under append, so a commit that learns new segments extends
/// the table and leaves every existing shard's indices valid.
#[derive(Debug, Default)]
struct ObjectTable {
    refs: Vec<ObjRef>,
    by_ref: HashMap<ObjRef, u32>,
}

impl ObjectTable {
    /// Intern each of `local`'s objects here, returning the index each
    /// one got: the only part of moving a shard built against a private
    /// table onto this one that needs this table, so the shared lock is
    /// held for the distinct objects alone, never across a decode or the
    /// per-extent rewrite ([`remap_targets`]).
    fn adopt(&mut self, local: &ObjectTable) -> Vec<u32> {
        local.refs.iter().map(|&o| self.intern(o)).collect()
    }

    fn intern(&mut self, obj: ObjRef) -> u32 {
        if let Some(&i) = self.by_ref.get(&obj) {
            return i;
        }
        let i = self.refs.len() as u32;
        self.refs.push(obj);
        self.by_ref.insert(obj, i);
        i
    }
}

/// Rewrite `shard`'s targets from its private table's indices to the
/// shared table's, by the `remap` [`ObjectTable::adopt`] returned. Runs
/// outside the shared table's lock: linear in the extents.
fn remap_targets(mut shard: Shard, remap: &[u32]) -> Shard {
    for t in &mut shard.target {
        if *t != ZERO {
            *t = remap[*t as usize];
        }
    }
    shard
}

/// A volume's committed extents: one slot per shard, each materialized
/// from its subtree on first touch (design D-M8).
///
/// Nothing about a committed shard requires it in memory before a read
/// lands in it: the closure was made durable before the commit's ref
/// moved (I1's write side), and `fsck` is where reachability is
/// *verified*. So opening a volume records the shard roots and stops —
/// three object reads regardless of size — and an export nobody reads
/// materializes nothing.
///
/// Slots are shared by `Arc` across index generations ([`Self::with_shards`]),
/// so a shard materialized once is materialized for every engine state
/// that shares it, and a commit's untouched shards carry their cells
/// forward. The object table is shared across the same lineage and is
/// append-only, which is what keeps every shard's indices valid as later
/// materializations extend it. It grows with the distinct objects ever
/// referenced while this engine is open; a reopen resets it.
#[derive(Debug, Clone)]
pub(crate) struct CommittedIndex {
    shard_bytes: u64,
    slots: Vec<Arc<Slot>>,
    objects: Arc<RwLock<ObjectTable>>,
    /// The commit's materialized map, when one was in the store at open
    /// (design D-M7): shards decode from this buffer instead of walking
    /// their subtree. Any per-shard problem falls back to the walk —
    /// the tree stays the source of truth, the emap is only a shortcut.
    emap: Option<Arc<EmapSource>>,
}

/// A fetched-and-verified emap object: the interned object table plus
/// each shard's blob range into `body`.
#[derive(Debug)]
pub(crate) struct EmapSource {
    objects: Vec<ObjRef>,
    ranges: Vec<(u32, u32)>,
    body: bytes::Bytes,
}

impl EmapSource {
    /// From the raw object bytes, already known to be for the right
    /// commit and shard grid (the caller checked the header).
    pub(crate) fn new(header: skynbd_format::EmapHeader, raw: bytes::Bytes) -> Result<Self> {
        let body_start = skynbd_format::emap_header_end(&raw)?;
        Ok(Self {
            objects: header
                .objects
                .into_iter()
                .map(|o| match o {
                    EmapObj::Seg(id) => ObjRef::Seg(id),
                    EmapObj::Cas(h) => ObjRef::Cas(h),
                })
                .collect(),
            ranges: header.shards,
            body: raw.slice(body_start..),
        })
    }

    /// Decode shard `no` into the columnar structure, interning its
    /// targets into `objects` — the caller's private table, which it then
    /// adopts into the live one (never the live table itself: this is the
    /// decode the adopt keeps out of the shared lock). `Ok(None)` = the emap has
    /// no entry (empty shard, or a shorter map than the volume implies).
    fn shard(
        &self,
        no: usize,
        shard_bytes: u64,
        objects: &mut ObjectTable,
    ) -> Result<Option<Shard>> {
        let Some(&(off, len)) = self.ranges.get(no) else {
            return Ok(None);
        };
        if len == 0 {
            return Ok(None);
        }
        let blob = self
            .body
            .get(off as usize..(off + len) as usize)
            .ok_or_else(|| EngineError::Corrupt("emap shard range past body".into()))?;
        let em = skynbd_format::decode_emap_shard(blob)?;
        let mut shard = Shard {
            start: Vec::with_capacity(em.gap.len()),
            len: Vec::with_capacity(em.gap.len()),
            target: Vec::with_capacity(em.gap.len()),
            offset: Vec::with_capacity(em.gap.len()),
        };
        // One linear pass: gaps to absolute starts, emap target indices
        // to indices in `objects`. The result *is* the D-M10 structure.
        let mut cursor: u64 = 0;
        for i in 0..em.gap.len() {
            let start = cursor + em.gap[i] as u64;
            let end = start + em.len[i] as u64;
            if end > shard_bytes {
                return Err(EngineError::Corrupt(format!(
                    "emap shard {no} extent ends at {end}, past the {shard_bytes} B shard"
                )));
            }
            let target = match em.target[i] {
                EMAP_ZERO => ZERO,
                t => {
                    let obj = *self.objects.get(t as usize).ok_or_else(|| {
                        EngineError::Corrupt(format!("emap target {t} past the object table"))
                    })?;
                    objects.intern(obj)
                }
            };
            shard.start.push(start as u32);
            shard.len.push(em.len[i]);
            shard.target.push(target);
            shard.offset.push(em.offset[i]);
            cursor = end;
        }
        Ok(Some(shard))
    }
}

/// One shard's state: where its subtree lives, and the materialized form
/// once something touched it. The cell is single-flight: readers that
/// touch an unmaterialized shard concurrently (issue #97: reads share the
/// engine) wait for one walk instead of each fetching the subtree; if
/// that walk fails, the next waiter tries again.
#[derive(Debug)]
struct Slot {
    /// Subtree root from the commit's shard index; `None` = empty shard.
    root: Option<NodeHash>,
    cell: tokio::sync::OnceCell<Arc<Shard>>,
    /// Failed materializations so far, with the latest one's error. The
    /// cell runs one initializer at a time and leaves itself empty when
    /// one fails, so without this every caller queued behind a failing
    /// walk would re-run it in turn, each holding its engine read guard
    /// (and with it any queued writer) for another full walk.
    failed: std::sync::Mutex<(u64, Option<EngineError>)>,
}

impl Slot {
    fn unloaded(root: Option<NodeHash>) -> Arc<Self> {
        Arc::new(Self {
            root,
            cell: tokio::sync::OnceCell::new(),
            failed: std::sync::Mutex::new((0, None)),
        })
    }

    /// How many materializations of this slot have failed so far.
    fn failures(&self) -> u64 {
        self.failed.lock().expect("lock").0
    }

    /// The latest failure, if one has happened since the caller saw
    /// `seen` failures: it was waiting on that attempt, and shares it.
    fn failed_since(&self, seen: u64) -> Option<EngineError> {
        let f = self.failed.lock().expect("lock");
        if f.0 > seen {
            f.1.as_ref().map(replay)
        } else {
            None
        }
    }

    fn record_failure(&self, e: &EngineError) {
        let mut f = self.failed.lock().expect("lock");
        f.0 += 1;
        f.1 = Some(replay(e));
    }
}

/// A copy of a materialization error for each caller that shares it. The
/// walk's errors are store, format and corruption errors, which copy
/// exactly; anything else keeps its message.
fn replay(e: &EngineError) -> EngineError {
    match e {
        EngineError::Store(e) => EngineError::Store(e.clone()),
        EngineError::Format(e) => EngineError::Format(e.clone()),
        EngineError::Corrupt(m) => EngineError::Corrupt(m.clone()),
        other => EngineError::Corrupt(other.to_string()),
    }
}

impl CommittedIndex {
    pub(crate) fn empty(size: u64, shard_bytes: u64) -> Self {
        Self::from_roots(size, shard_bytes, Vec::new()).expect("empty index")
    }

    /// From a commit's shard index, materializing nothing. A shorter
    /// array than the volume implies is legitimate — a grown clone starts
    /// from its base's smaller index — and extends with empty shards; a
    /// longer one can only mean the geometry and the tree disagree, which
    /// the superblock (D-M14) is supposed to make impossible.
    pub(crate) fn from_roots(
        size: u64,
        shard_bytes: u64,
        roots: Vec<Option<NodeHash>>,
    ) -> Result<Self> {
        let count = size.div_ceil(shard_bytes) as usize;
        if roots.len() > count {
            return Err(EngineError::Corrupt(format!(
                "shard index has {} roots but this volume's geometry implies \
                 {count}; refusing to reinterpret",
                roots.len()
            )));
        }
        let mut slots: Vec<Arc<Slot>> = roots.into_iter().map(Slot::unloaded).collect();
        slots.resize_with(count, || Slot::unloaded(None));
        Ok(Self {
            shard_bytes,
            slots,
            objects: Arc::new(RwLock::new(ObjectTable::default())),
            emap: None,
        })
    }

    /// Attach a fetched emap (design D-M7): unmaterialized shards decode
    /// from it instead of walking their subtrees.
    pub(crate) fn with_emap(mut self, src: EmapSource) -> Self {
        self.emap = Some(Arc::new(src));
        self
    }

    /// Build fully materialized from a flat extent stream in ascending
    /// order — the rig this module's tests drive `query` through, and the
    /// shape the D-M7 emap loader will take when it lands.
    ///
    /// Shard membership is derived from each extent's own LBA, and an
    /// extent straddling a boundary is split, so the result is correct
    /// even for input that disagrees with the shard grid.
    #[cfg(test)]
    pub(crate) fn build(size: u64, shard_bytes: u64, extents: Vec<Extent>) -> Result<Self> {
        // A well-formed stream is ascending and disjoint, and `seek`
        // binary-searches on that assumption — `partition_point` on an
        // unsorted array is unspecified rather than merely wrong. Check,
        // and repair by the rule the interval map applied implicitly:
        // whatever arrives later wins.
        let extents = match first_disorder(&extents) {
            None => extents,
            Some(at) => {
                tracing::warn!(
                    at,
                    "extent stream has overlapping or out-of-order extents; \
                     repairing (later entries win)"
                );
                repair(&extents)
            }
        };
        let index = Self::empty(size, shard_bytes);
        let mut objects = ObjectTable::default();
        let mut open: Option<(usize, ShardBuilder)> = None;
        let seal = |no: usize, b: ShardBuilder, index: &Self| {
            if let Some(shard) = b.finish() {
                index.slots[no].cell.set(shard).expect("slot built once");
            }
        };
        for e in &extents {
            let mut cursor = e.lba_start;
            while cursor < e.end() {
                let no = (cursor / shard_bytes) as usize;
                let stop = ((no as u64 + 1) * shard_bytes).min(e.end());
                if no >= index.slots.len() {
                    return Err(EngineError::Corrupt(format!(
                        "extent at {cursor} past the volume's {size} bytes"
                    )));
                }
                if let Some((n, _)) = &open {
                    if *n != no {
                        let (n, b) = open.take().expect("checked");
                        seal(n, b, &index);
                    }
                }
                let (_, b) = open.get_or_insert_with(|| {
                    (
                        no,
                        ShardBuilder {
                            shard: Shard::default(),
                            base: no as u64 * shard_bytes,
                        },
                    )
                });
                b.push(&e.slice(cursor, stop), &mut objects)?;
                cursor = stop;
            }
        }
        if let Some((n, b)) = open {
            seal(n, b, &index);
        }
        *index.objects.write().expect("lock") = objects;
        Ok(index)
    }

    pub(crate) fn shard_of(&self, lba: u64) -> usize {
        (lba / self.shard_bytes) as usize
    }

    /// The shard's materialized form, fetching its subtree on first
    /// touch: root node, then leaves, with the walk fan-out (§7.2).
    async fn shard<S: ObjectStore>(
        &self,
        no: usize,
        store: &S,
        resolver: &crate::Resolver,
    ) -> Result<Option<Arc<Shard>>> {
        let Some(slot) = self.slots.get(no) else {
            return Ok(None);
        };
        if let Some(s) = slot.cell.get() {
            return Ok(Some(Arc::clone(s)));
        }
        let Some(root) = slot.root else {
            return Ok(None);
        };
        // Single flight: concurrent first touches wait for one walk. A
        // walk that FAILS fails every caller already waiting on it — they
        // share its error rather than retrying one after another — and
        // only callers arriving after the failure try again. A cancelled
        // walk (its reader dropped) records nothing, so the next waiter
        // runs it.
        let seen = slot.failures();
        let shard = slot
            .cell
            .get_or_try_init(|| async {
                if let Some(e) = slot.failed_since(seen) {
                    return Err(e);
                }
                let r = self.materialize(no, root, store, resolver).await;
                if let Err(e) = &r {
                    slot.record_failure(e);
                }
                r
            })
            .await?;
        Ok(Some(Arc::clone(shard)))
    }

    /// Build shard `no` from its emap entry or its subtree. Decoding runs
    /// against a private object table; only the final adopt takes the
    /// shared table's lock (a large shard's decode must not park the
    /// other readers' worker threads on it).
    async fn materialize<S: ObjectStore>(
        &self,
        no: usize,
        root: NodeHash,
        store: &S,
        resolver: &crate::Resolver,
    ) -> Result<Arc<Shard>> {
        // The D-M7 shortcut: decode the shard from the commit's emap
        // instead of walking its subtree. Any problem falls back to the
        // walk — the tree is the source of truth, and a bad emap must
        // cost a warning, never an IO error.
        if let Some(src) = &self.emap {
            let mut local = ObjectTable::default();
            match src.shard(no, self.shard_bytes, &mut local) {
                Ok(Some(shard)) => {
                    let remap = self.objects.write().expect("lock").adopt(&local);
                    return Ok(Arc::new(remap_targets(shard, &remap)));
                }
                // The emap simply has nothing for a shard whose root
                // exists — inconsistent; walk the tree.
                Ok(None) => {
                    tracing::warn!(
                        shard = no,
                        "emap misses a non-empty shard; walking the tree"
                    )
                }
                Err(e) => {
                    tracing::warn!(shard = no, error = %e, "emap shard unusable; walking the tree")
                }
            }
        }

        let base = no as u64 * self.shard_bytes;
        let bytes = resolver
            .get(store, &move |v| crate::keys::node(v, &root), None)
            .await?;
        let mut extents: Vec<Extent> = Vec::new();
        match Node::decode(&bytes)? {
            node @ Node::Leaf(_) => extents.extend(node.leaf_extents()?),
            Node::Internal(children) => {
                let hashes: Vec<NodeHash> = children.into_iter().map(|(_, h)| h).collect();
                for bytes in crate::fetch_nodes(store, resolver, &hashes).await? {
                    extents.extend(Node::decode(&bytes)?.leaf_extents()?);
                }
            }
            Node::ShardIndex(_) => {
                return Err(EngineError::Corrupt("nested shard index".into()));
            }
        }
        // A shard's subtree claiming bytes outside its range means the
        // tree and the volume's recorded geometry disagree — impossible
        // to construct since D-M14, so it is corruption, not a case to
        // paper over. `fsck` is the tool that says how bad it is.
        if let Some(e) = extents
            .iter()
            .find(|e| e.lba_start < base || e.end() > base + self.shard_bytes)
        {
            return Err(EngineError::Corrupt(format!(
                "shard {no} subtree holds extent [{}, {}) outside its range; \
                 the tree disagrees with the volume's shard geometry — run fsck",
                e.lba_start,
                e.end()
            )));
        }
        // Within one subtree the gap encoding makes each leaf ascending
        // by construction; disorder can only come from a corrupt internal
        // node's child order. Repair as `build` does, loudly.
        let extents = match first_disorder(&extents) {
            None => extents,
            Some(at) => {
                tracing::warn!(at, shard = no, "shard subtree out of order; repairing");
                repair(&extents)
            }
        };
        let mut b = ShardBuilder {
            shard: Shard::default(),
            base,
        };
        let mut local = ObjectTable::default();
        for e in &extents {
            b.push(e, &mut local)?;
        }
        let remap = self.objects.write().expect("lock").adopt(&local);
        Ok(Arc::new(remap_targets(b.shard, &remap)))
    }

    /// One shard's extents, absolute — what a commit merges its overlay
    /// into. Materializes the shard if nothing touched it yet.
    pub(crate) async fn extents<S: ObjectStore>(
        &self,
        no: usize,
        store: &S,
        resolver: &crate::Resolver,
    ) -> Result<Vec<Extent>> {
        let Some(shard) = self.shard(no, store, resolver).await? else {
            return Ok(Vec::new());
        };
        let base = no as u64 * self.shard_bytes;
        let objects = self.objects.read().expect("lock");
        Ok((0..shard.count())
            .map(|i| shard.extent(i, base, &objects.refs))
            .collect())
    }

    /// Serialize the whole committed map (design D-M7), materializing
    /// every untouched shard first — the cost a clean unstage or a
    /// snapshot pays so the next open doesn't walk. The live object
    /// table is written verbatim: shard target indices already point
    /// into it, and entries no current shard references are dead weight
    /// the format tolerates.
    pub(crate) async fn to_emap<S: ObjectStore>(
        &self,
        store: &S,
        resolver: &crate::Resolver,
    ) -> Result<(Vec<EmapObj>, Vec<Option<EmapShard>>)> {
        let mut shards: Vec<Option<EmapShard>> = Vec::with_capacity(self.slots.len());
        for no in 0..self.slots.len() {
            shards.push(match self.shard(no, store, resolver).await? {
                None => None,
                Some(shard) => Some(encode_shard(&shard)?),
            });
        }
        let objects = self
            .objects
            .read()
            .expect("lock")
            .refs
            .iter()
            .map(|o| match o {
                ObjRef::Seg(id) => EmapObj::Seg(*id),
                ObjRef::Cas(h) => EmapObj::Cas(*h),
            })
            .collect();
        Ok((objects, shards))
    }

    /// Replace the given shards with pre-merged extents and their new
    /// subtree roots; every other slot is shared, cells and all. New
    /// objects extend the shared table, which keeps existing indices
    /// valid.
    pub(crate) fn with_shards(
        &self,
        replaced: Vec<(usize, Vec<Extent>, Option<NodeHash>)>,
    ) -> Result<Self> {
        let mut slots = self.slots.clone();
        let mut objects = self.objects.write().expect("lock");
        for (no, extents, root) in replaced {
            let mut b = ShardBuilder {
                shard: Shard::default(),
                base: no as u64 * self.shard_bytes,
            };
            for e in &extents {
                b.push(e, &mut objects)?;
            }
            let cell = tokio::sync::OnceCell::new();
            if let Some(shard) = b.finish() {
                cell.set(shard).expect("fresh cell");
            }
            slots[no] = Arc::new(Slot {
                root,
                cell,
                failed: std::sync::Mutex::new((0, None)),
            });
        }
        drop(objects);
        // The emap carries forward: untouched slots hold the same roots
        // (same content) it describes, and replaced slots were set above
        // and never consult it.
        Ok(Self {
            shard_bytes: self.shard_bytes,
            slots,
            objects: Arc::clone(&self.objects),
            emap: self.emap.clone(),
        })
    }

    /// A complete cover of `[start, start+len)`: `Some(target)` already
    /// rebased to the run's own start, `None` for a hole. Materializes
    /// exactly the shards the range touches.
    pub(crate) async fn query<S: ObjectStore>(
        &self,
        store: &S,
        resolver: &crate::Resolver,
        start: u64,
        len: u64,
    ) -> Result<Vec<Run<Target>>> {
        let mut out: Vec<Run<Target>> = Vec::new();
        let end = start + len;
        let mut cursor = start;
        while cursor < end {
            let no = self.shard_of(cursor);
            let base = no as u64 * self.shard_bytes;
            let stop = (base + self.shard_bytes).min(end);
            match self.shard(no, store, resolver).await? {
                None => push_hole(&mut out, cursor, stop - cursor),
                Some(shard) => {
                    let objects = self.objects.read().expect("lock");
                    query_shard(&shard, &objects.refs, base, cursor, stop, &mut out);
                }
            }
            cursor = stop;
        }
        Ok(out)
    }
}

fn query_shard(
    shard: &Shard,
    objects: &[ObjRef],
    base: u64,
    from: u64,
    to: u64,
    out: &mut Vec<Run<Target>>,
) {
    let mut cursor = from;
    let mut i = shard.seek((from - base) as u32);
    while cursor < to && i < shard.count() {
        let e = shard.extent(i, base, objects);
        if e.end() <= cursor {
            i += 1;
            continue;
        }
        if e.lba_start >= to {
            break;
        }
        if e.lba_start > cursor {
            push_hole(out, cursor, e.lba_start - cursor);
            cursor = e.lba_start;
        }
        let stop = e.end().min(to);
        let piece = e.slice(cursor, stop);
        out.push(Run {
            start: piece.lba_start,
            len: piece.len,
            value: Some(piece.target),
        });
        cursor = stop;
        i += 1;
    }
    if cursor < to {
        push_hole(out, cursor, to - cursor);
    }
}

/// Holes carry no information beyond their extent, so adjacent ones are
/// one hole — callers see the same cover however shards are laid out.
fn push_hole(out: &mut Vec<Run<Target>>, start: u64, len: u64) {
    match out.last_mut() {
        Some(last) if last.value.is_none() && last.start + last.len == start => last.len += len,
        _ => out.push(Run {
            start,
            len,
            value: None,
        }),
    }
}

/// Fold `overlay` over `base`, both ascending and non-overlapping within
/// themselves, with the overlay winning where they meet. Truncated base
/// pieces are re-based through [`Extent::slice`]; the result is maximally
/// coalesced.
///
/// This is `ExtentMap::insert`'s last-writer-wins semantics as a single
/// sweep — one pass over one shard, rather than cloning the whole map and
/// re-inserting. The proptest in this module holds it against `ExtentMap`
/// as the oracle.
pub(crate) fn merge(base: &[Extent], overlay: &[Extent]) -> Vec<Extent> {
    let mut out: Vec<Extent> = Vec::with_capacity(base.len() + overlay.len());
    let mut bi = 0;
    // A base extent whose tail survived the previous overlay entry.
    let mut carry: Option<Extent> = None;
    let next_base = |bi: &mut usize, carry: &mut Option<Extent>| -> Option<Extent> {
        carry.take().or_else(|| {
            let e = base.get(*bi).cloned();
            if e.is_some() {
                *bi += 1;
            }
            e
        })
    };

    for ov in overlay {
        while let Some(b) = next_base(&mut bi, &mut carry) {
            if b.end() <= ov.lba_start {
                push(&mut out, b);
                continue;
            }
            if b.lba_start >= ov.end() {
                carry = Some(b);
                break;
            }
            if b.lba_start < ov.lba_start {
                push(&mut out, b.slice(b.lba_start, ov.lba_start));
            }
            if b.end() > ov.end() {
                carry = Some(b.slice(ov.end(), b.end()));
                break;
            }
            // Fully covered: the overlay replaces it outright.
        }
        push(&mut out, ov.clone());
    }
    while let Some(b) = next_base(&mut bi, &mut carry) {
        push(&mut out, b);
    }
    out
}

fn push(out: &mut Vec<Extent>, e: Extent) {
    match out.last_mut() {
        Some(prev) if prev.continues_into(&e) => prev.absorb(&e),
        _ => out.push(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use skynbd_store::MemoryStore;
    use skynbd_tree::ExtentMap;
    use skynbd_types::VolumeId;

    const SHARD: u64 = 1 << 20;

    fn seg(n: u128, offset: u64) -> Target {
        Target::Data(DataRef::Seg {
            segment: SegmentId(ulid::Ulid::from(n)),
            offset,
        })
    }

    fn ext(start: u64, len: u64, target: Target) -> Extent {
        Extent::new(start, len, target).unwrap()
    }

    /// Query rig for fully-materialized indexes: the store and resolver
    /// are required by the signature but never touched, since `build`
    /// leaves nothing to fetch. Single-threaded `block_on` keeps the
    /// proptests synchronous.
    fn query_all(idx: &CommittedIndex, start: u64, len: u64) -> Vec<Run<Target>> {
        let store = MemoryStore::new();
        let resolver = crate::Resolver::new(VolumeId(ulid::Ulid::from(1u128)), Vec::new());
        tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("rt")
            .block_on(idx.query(&store, &resolver, start, len))
            .expect("materialized index queries never fetch")
    }

    /// The oracle: `ExtentMap` applied in the same order, read back.
    fn oracle(base: &[Extent], overlay: &[Extent]) -> Vec<Extent> {
        let mut m: ExtentMap<Extent> = ExtentMap::new();
        for e in base.iter().chain(overlay) {
            m.insert(e.lba_start, e.len, e.clone());
        }
        let mut out: Vec<Extent> = Vec::new();
        for (start, len, orig) in m.iter() {
            // `ExtentMap` keeps the original value across splits, so the
            // target has to be re-based the same way a reader would.
            push(&mut out, orig.slice(start, start + len));
        }
        out
    }

    #[test]
    fn merge_matches_the_oracle_on_the_awkward_shapes() {
        let base = vec![
            ext(0, 4096, seg(1, 0)),
            ext(8192, 8192, seg(1, 4096)),
            ext(65536, 4096, Target::Zero),
        ];
        for overlay in [
            vec![],
            vec![ext(0, 4096, seg(2, 0))],         // exact replace
            vec![ext(10240, 2048, seg(2, 0))],     // interior split
            vec![ext(4096, 4096, seg(2, 0))],      // fills the hole
            vec![ext(0, 65536 + 4096, seg(2, 0))], // swallows all
            vec![ext(12288, 8192, seg(2, 0))],     // straddles the tail
            vec![ext(2048, 2048, seg(2, 0)), ext(9216, 1024, seg(3, 0))],
        ] {
            assert_eq!(
                merge(&base, &overlay),
                oracle(&base, &overlay),
                "{overlay:?}"
            );
        }
    }

    #[test]
    fn merge_coalesces_across_the_seam() {
        // Overlay continues the base extent in the same segment: one run.
        let base = vec![ext(0, 4096, seg(1, 0))];
        let overlay = vec![ext(4096, 4096, seg(1, 4096))];
        let merged = merge(&base, &overlay);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].len, 8192);
    }

    fn index_of(extents: &[Extent], size: u64) -> CommittedIndex {
        CommittedIndex::build(size, SHARD, extents.to_vec()).unwrap()
    }

    /// A tree written under one `shard_bytes` and read under another is
    /// no longer constructible (D-M14), but a flat extent stream — the
    /// emap shape — can still straddle the grid, and must be split
    /// rather than filed wholesale under one shard.
    #[test]
    fn extents_straddling_a_shard_boundary_are_split() {
        let e = ext(SHARD - 4096, 8192, seg(1, 1024));
        let idx = index_of(std::slice::from_ref(&e), 4 * SHARD);
        // And it reads back as one contiguous run of the right bytes.
        let runs = query_all(&idx, SHARD - 4096, 8192);
        assert_eq!(runs[0].value, Some(seg(1, 1024)));
        assert_eq!(runs.iter().map(|r| r.len).sum::<u64>(), 8192);
    }

    #[test]
    fn query_covers_holes_shards_and_partial_extents() {
        let extents = vec![
            ext(0, 4096, seg(1, 0)),
            ext(SHARD - 4096, 4096, seg(1, 4096)),
            ext(SHARD + 8192, 4096, Target::Zero),
        ];
        let idx = index_of(&extents, 4 * SHARD);

        // Whole volume: every byte accounted for, holes merged.
        let runs = query_all(&idx, 0, 4 * SHARD);
        assert_eq!(runs.iter().map(|r| r.len).sum::<u64>(), 4 * SHARD);
        assert_eq!(runs[0].value, Some(seg(1, 0)));
        assert!(runs[1].value.is_none());

        // A read landing inside an extent gets a re-based target.
        let runs = query_all(&idx, 2048, 2048);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].value, Some(seg(1, 2048)));

        // Spanning the shard boundary: no run straddles it, and the
        // cover is still complete.
        let runs = query_all(&idx, SHARD - 8192, 16384);
        assert_eq!(runs.iter().map(|r| r.len).sum::<u64>(), 16384);
    }

    #[test]
    fn replacing_a_shard_shares_the_others() {
        let extents = vec![ext(0, 4096, seg(1, 0)), ext(2 * SHARD, 4096, seg(2, 0))];
        let idx = index_of(&extents, 4 * SHARD);
        let next = idx
            .with_shards(vec![(0, vec![ext(0, 8192, seg(3, 0))], None)])
            .unwrap();

        // The untouched slot is the same allocation, cell and all; the
        // replaced one is fresh.
        assert!(Arc::ptr_eq(&idx.slots[2], &next.slots[2]));
        assert!(!Arc::ptr_eq(&idx.slots[0], &next.slots[0]));
        // The old index still resolves its own view.
        assert_eq!(query_all(&idx, 0, 4096)[0].value, Some(seg(1, 0)));
        assert_eq!(query_all(&next, 0, 4096)[0].value, Some(seg(3, 0)));
    }

    /// A materialization racing a commit: the slot the commit replaced is
    /// a *new* slot, so a stale in-flight materialization of the old one
    /// cannot leak into the new index generation.
    #[test]
    fn a_replaced_slot_does_not_inherit_the_old_cell() {
        let idx = index_of(&[ext(0, 4096, seg(1, 0))], SHARD);
        let next = idx.with_shards(vec![(0, vec![], None)]).unwrap();
        // Old generation still sees its extent; new generation sees the
        // shard emptied, not the old cell.
        assert_eq!(query_all(&idx, 0, 4096)[0].value, Some(seg(1, 0)));
        assert!(query_all(&next, 0, 4096)[0].value.is_none());
    }

    #[test]
    fn oversized_fields_are_refused_not_truncated() {
        let huge = Extent {
            lba_start: 0,
            len: 4096,
            target: seg(1, u32::MAX as u64 + 4096),
        };
        let err = CommittedIndex::build(SHARD, SHARD, vec![huge]).unwrap_err();
        assert!(matches!(err, EngineError::Corrupt(m) if m.contains("object offset")));
    }

    /// A longer shard array than the volume's geometry implies is the
    /// truncation hazard; `from_roots` refuses it. Shorter is a grown
    /// clone and extends with empty shards.
    #[test]
    fn from_roots_extends_short_arrays_and_refuses_long_ones() {
        let idx = CommittedIndex::from_roots(4 * SHARD, SHARD, vec![None; 2]).unwrap();
        assert_eq!(idx.slots.len(), 4);
        assert!(CommittedIndex::from_roots(SHARD, SHARD, vec![None; 2]).is_err());
    }

    prop_compose! {
        fn extents_strategy()(raw in prop::collection::vec((0u64..64, 1u64..8, 0u128..3), 0..12))
            -> Vec<Extent> {
            let mut m: ExtentMap<Extent> = ExtentMap::new();
            for (start, len, s) in raw {
                let e = ext(start * 512, len * 512, seg(s, start * 512));
                m.insert(e.lba_start, e.len, e);
            }
            let mut out = Vec::new();
            for (start, len, orig) in m.iter() {
                out.push(orig.slice(start, start + len));
            }
            out
        }
    }

    proptest! {
        #[test]
        fn merge_agrees_with_extent_map(base in extents_strategy(), overlay in extents_strategy()) {
            prop_assert_eq!(merge(&base, &overlay), oracle(&base, &overlay));
        }

        #[test]
        fn query_agrees_with_extent_map(extents in extents_strategy()) {
            let size = 64 * 512;
            let idx = index_of(&extents, size.max(SHARD));
            let mut m: ExtentMap<Extent> = ExtentMap::new();
            for e in &extents {
                m.insert(e.lba_start, e.len, e.clone());
            }
            for run in query_all(&idx, 0, size) {
                for probe in (run.start..run.start + run.len).step_by(512) {
                    let want = m.query(probe, 512).into_iter().next().unwrap();
                    let got = run.value.as_ref().map(|t: &Target| {
                        Extent { lba_start: run.start, len: run.len, target: t.clone() }
                            .slice(probe, probe + 512)
                            .target
                    });
                    let want = want.value.map(|e| e.slice(probe, probe + 512).target);
                    prop_assert_eq!(got, want, "at {}", probe);
                }
            }
        }
    }
}

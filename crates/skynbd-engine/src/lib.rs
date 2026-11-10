//! Volume engine — the single durability path (design §6, §7).
//!
//! Everything that becomes durable goes through one [`Engine::commit`],
//! implementing the sequence of design §6.3:
//!
//! ```text
//! seal open segment → PUT segments → fold overlay into shard trees →
//! PUT changed nodes → PUT commit → CAS head ref
//! ```
//!
//! Invariants exercised by this crate's tests (design §6.2):
//! - **I1** reachability-before-visibility: a ref may only move to a
//!   commit whose closure is already durable — established by the commit
//!   sequence (the CAS is last), verified at publish and by `fsck`. Open
//!   does *not* re-walk the closure (design D-M8).
//! - **I2** linear head history: every commit CASes from the exact
//!   `(commit, generation)` it loaded.
//! - **I3** read-your-writes: acked writes are readable pre-commit (overlay).
//! - **I4/I7** flush durability & crash equivalence: the crash-point
//!   matrix kills the commit at every stage and asserts reopened content
//!   equals the last acked flush.
//!
//! Serving model (design §7.2, D-M8/D-M10): opening a volume records its
//! commit's shard roots and nothing else; each shard's extents
//! materialize into an immutable columnar index on first touch, and a
//! commit rewrites only the shards it folds into. The persistent COW
//! nodes are the durability format; the in-memory index is a cache over
//! them.

use std::collections::HashMap;
use std::ops::Range;

use bytes::{Bytes, BytesMut};
use skynbd_format::{
    CloneBase, Codec, Commit, CommitKind, Node, SnapKind, SnapshotManifest, Superblock,
};
use skynbd_refs::{CasOutcome, RefStore, RefValue};
use skynbd_store::ObjectStore;
use skynbd_tree::ExtentMap;
use skynbd_types::{
    CommitHash, DataRef, Extent, NodeHash, SegmentId, SnapId, Target, VolumeId, SECTOR,
};

pub mod index;
pub mod keys;
pub mod maintenance;
pub mod prepared;
pub use maintenance::{
    commit_extents, fsck, fsck_with, has_history, rebuild_refs, recover_head, recover_head_with,
    FsckReport, GcReport, RebuildReport,
};

/// `LeaseLost` (design D-M3) is the same I2 self-fence as a lost head
/// CAS — drop the overlay, `NBD_ESHUTDOWN` — whether the head moved
/// under us or the volume's ownership did. Mapped here so every write
/// path inherits it without knowing leases exist. When the refs layer
/// knows the actual owner, the mapping keeps it ([`EngineError::Fenced`])
/// so the API can answer with the structured 409 `lease_lost` instead
/// of an opaque conflict; an owner-less refusal (no lease row) stays
/// the plain [`EngineError::Conflict`].
impl From<skynbd_refs::RefError> for EngineError {
    fn from(e: skynbd_refs::RefError) -> Self {
        match e {
            skynbd_refs::RefError::LeaseLost { holder, epoch, .. } if !holder.is_empty() => {
                EngineError::Fenced { holder, epoch }
            }
            skynbd_refs::RefError::LeaseLost { .. } => EngineError::Conflict,
            other => EngineError::Refs(other),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("store: {0}")]
    Store(#[from] skynbd_store::StoreError),
    #[error("refs: {0}")]
    Refs(skynbd_refs::RefError),
    #[error("format: {0}")]
    Format(#[from] skynbd_format::FormatError),
    #[error("invalid io: {0}")]
    InvalidIo(String),
    #[error("head CAS conflict: lease lost (I2)")]
    Conflict,
    /// A fenced write (Invariant F, task 15h) refused with the actual
    /// owner known: the write's epoch (explicit request epoch, or the
    /// store's ambient identity) is superseded — the volume is owned by
    /// `holder` at `epoch` now. Carries the owner so the API can answer
    /// with the distinguishable 409 `lease_lost` body; a refusal whose
    /// owner is unknown stays [`Self::Conflict`]. To the NBD frontend
    /// both are the same I2 self-fence (`ESHUTDOWN`).
    #[error("fenced: volume is owned by {holder:?} at epoch {epoch} (Invariant F)")]
    Fenced { holder: String, epoch: u64 },
    #[error("snapshot already exists: {0}")]
    SnapshotExists(String),
    #[error("snapshot not found: {0}")]
    SnapshotNotFound(String),
    #[error("rollback refused: un-flushed writes present (flush or discard first)")]
    DirtyRollback,
    #[error("gc refused: un-flushed writes present (flush first)")]
    DirtyGc,
    #[error("emap refused: un-flushed writes present (flush first)")]
    DirtyEmap,
    #[error("snapshot is pinned: {0}")]
    Pinned(String),
    #[error("prepared snapshot not found: {0}")]
    PreparedNotFound(String),
    #[error("corrupt volume state: {0}")]
    Corrupt(String),
}

pub type Result<T> = std::result::Result<T, EngineError>;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    /// Seal threshold for the open segment (design D2).
    pub segment_max_bytes: u64,
    /// Logical shard size (design D3). Tests shrink this to force
    /// multi-shard commits.
    pub shard_bytes: u64,
    /// Fence epoch stamped into every commit this engine writes (design
    /// D-M5): the volume's lease epoch at acquisition. Fixed for the
    /// engine's lifetime — if the lease moves, this engine gets fenced
    /// and dies; the re-acquirer opens a fresh engine at the new epoch.
    /// 0 = unowned (single-node, tests).
    pub epoch: u64,
    /// Store GETs one `read` keeps in flight (issue #97 item 4). A read
    /// spanning N cold segments or CAS objects costs ~N / `read_fanout`
    /// round trips instead of N. 0 is treated as 1 (serial).
    pub read_fanout: usize,
    /// Node fetches and existence probes a tree walk keeps in flight
    /// (open, shard materialization, closure walks, `ensure`) — see
    /// [`WALK_CONCURRENCY`], the default. 0 is treated as 1.
    pub walk_concurrency: usize,
    /// Data GETs reads keep in flight together: the shared cap
    /// `read_fanout` alone is not, since requests on many connections (and
    /// many per connection, issue #97 item 3) each fan out. Default
    /// [`MAX_READ_GETS`]; 0 is treated as 1, and values above 65536 are
    /// clamped to it. Shared by every engine holding the same
    /// [`EngineConfig::read_gets`]; an engine without one gets its own.
    pub max_read_gets: usize,
    /// The data-GET permits this engine draws from. `None` (the default):
    /// its own `max_read_gets` of them. A served volume sets one with
    /// [`EngineConfig::share_read_gets`], and every engine opened from
    /// that config or its [`EngineConfig::tuning`] — the head, the
    /// snapshot/rpo/origin exports, snapshot-data and diff readers — draws
    /// from the same permits, so the cap holds for the volume's reads
    /// together, not per engine.
    pub read_gets: Option<ReadGets>,
}

/// A shared pool of data-GET permits ([`EngineConfig::read_gets`]).
#[derive(Debug, Clone)]
pub struct ReadGets(std::sync::Arc<tokio::sync::Semaphore>);

/// Two handles are equal when they are the same pool.
impl PartialEq for ReadGets {
    fn eq(&self, other: &Self) -> bool {
        std::sync::Arc::ptr_eq(&self.0, &other.0)
    }
}

impl ReadGets {
    /// `max` permits, clamped to 1..=65536.
    pub fn new(max: usize) -> Self {
        Self(std::sync::Arc::new(tokio::sync::Semaphore::new(
            max.clamp(1, 1 << 16),
        )))
    }
}

/// Upper bound for [`EngineConfig::read_fanout`] and
/// [`EngineConfig::walk_concurrency`]; larger values are clamped.
pub const MAX_FANOUT: usize = 256;

/// Default [`EngineConfig::max_read_gets`]: a volume's reads together
/// stay well under S3's ~5.5k GET/s per-prefix ceiling at typical
/// latencies, however many requests are in flight.
pub const MAX_READ_GETS: usize = 256;

impl EngineConfig {
    /// Only the fan-out knobs of `self` (and its shared GET permits, if
    /// any), on defaults: what an auxiliary reader of the volume — a
    /// snapshot-data or diff reader — takes from the volume's config
    /// without inheriting its geometry or fence epoch. (Prepared sessions
    /// read no data; they take only the walk width.)
    pub fn tuning(&self) -> EngineConfig {
        EngineConfig {
            read_fanout: self.read_fanout,
            walk_concurrency: self.walk_concurrency,
            max_read_gets: self.max_read_gets,
            read_gets: self.read_gets.clone(),
            ..EngineConfig::default()
        }
    }

    /// Give this config one pool of `max_read_gets` permits, to be shared
    /// by every engine opened from it (and its [`Self::tuning`]). Kept if
    /// it already has one.
    pub fn share_read_gets(mut self) -> Self {
        if self.read_gets.is_none() {
            self.read_gets = Some(ReadGets::new(self.max_read_gets));
        }
        self
    }
}

/// Default [`EngineConfig::read_fanout`]: with NBD servicing many
/// requests per connection, 16 per read keeps a busy volume well under
/// S3's per-prefix GET ceiling while flattening a large cold read.
pub const READ_FANOUT: usize = 16;

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            segment_max_bytes: 8 << 20,
            shard_bytes: 1 << 30,
            epoch: 0,
            read_fanout: READ_FANOUT,
            walk_concurrency: WALK_CONCURRENCY,
            max_read_gets: MAX_READ_GETS,
            read_gets: None,
        }
    }
}

/// Overlay/committed map value: a target whose `DataRef` offsets are valid
/// at `lba0`. When `ExtentMap` splits a run, the clone keeps `lba0`, so a
/// reader recovers the correct offset as `offset + (run.start - lba0)`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Placed {
    lba0: u64,
    target: Target,
}

impl Placed {
    /// The target re-based to `at` (adjusting data offsets by the split).
    fn rebased(&self, at: u64) -> Target {
        let delta = at - self.lba0;
        match &self.target {
            Target::Zero => Target::Zero,
            Target::Data(DataRef::Seg { segment, offset }) => Target::Data(DataRef::Seg {
                segment: *segment,
                offset: offset + delta,
            }),
            Target::Data(DataRef::Cas { hash, offset }) => Target::Data(DataRef::Cas {
                hash: *hash,
                offset: offset + delta,
            }),
        }
    }
}

/// Namespace fall-through for clone volumes (CSI design §4.1 option A):
/// object keys resolve in the volume's own namespace first, then along
/// the base chain (nearest base first). Every object kind resolved this
/// way is content-addressed or ULID-unique, so the first hit is *the*
/// object. A location cache remembers where each key was found, so the
/// hot read path pays the miss round-trips once per object, not per IO.
pub(crate) struct Resolver {
    /// `vols[0]` is the volume itself; the rest is its base chain.
    vols: Vec<VolumeId>,
    cache: std::sync::RwLock<HashMap<String, usize>>,
    /// Fan-out of the walks resolved through this resolver
    /// ([`EngineConfig::walk_concurrency`]; ≥ 1). Carried here because
    /// every walk already takes the resolver, engine-owned or not.
    pub(crate) walk: usize,
}

impl Resolver {
    pub(crate) fn new(vol: VolumeId, base_chain: Vec<VolumeId>) -> Self {
        let mut vols = vec![vol];
        vols.extend(base_chain);
        Self {
            vols,
            cache: std::sync::RwLock::new(HashMap::new()),
            walk: WALK_CONCURRENCY,
        }
    }

    /// Set the walk fan-out, clamped to `1..=`[`MAX_FANOUT`].
    pub(crate) fn walk(mut self, n: usize) -> Self {
        self.walk = n.clamp(1, MAX_FANOUT);
        self
    }

    /// Build a resolver for a volume by loading its base chain from the
    /// store (maintenance entry points that have no engine in hand).
    pub(crate) async fn for_volume<S: ObjectStore>(store: &S, vol: VolumeId) -> Result<Self> {
        Ok(Self::new(vol, load_base_chain(store, vol).await?))
    }

    fn is_clone(&self) -> bool {
        self.vols.len() > 1
    }

    pub(crate) fn own_vol(&self) -> VolumeId {
        self.vols[0]
    }

    /// GET with fall-through on `NotFound`. `make_key` builds the key for
    /// a given namespace; the cache is keyed by the own-namespace key.
    pub(crate) async fn get<S: ObjectStore>(
        &self,
        store: &S,
        make_key: &(dyn Fn(VolumeId) -> String + Sync),
        range: Option<Range<u64>>,
    ) -> Result<Bytes> {
        let own_key = make_key(self.vols[0]);
        let cached = self.cache.read().expect("lock").get(&own_key).copied();
        if let Some(i) = cached {
            return Ok(store.get(&make_key(self.vols[i]), range).await?);
        }
        for (i, vol) in self.vols.iter().enumerate() {
            match store.get(&make_key(*vol), range.clone()).await {
                Ok(bytes) => {
                    if self.is_clone() {
                        self.cache.write().expect("lock").insert(own_key, i);
                    }
                    return Ok(bytes);
                }
                Err(skynbd_store::StoreError::NotFound(_)) if i + 1 < self.vols.len() => {}
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!("loop returns on last namespace");
    }

    /// Like [`Resolver::get`], but also reports the key where the object
    /// was found (closure walks verify content addresses against it).
    pub(crate) async fn get_with_key<S: ObjectStore>(
        &self,
        store: &S,
        make_key: &(dyn Fn(VolumeId) -> String + Sync),
    ) -> Result<(String, Bytes)> {
        for (i, vol) in self.vols.iter().enumerate() {
            let k = make_key(*vol);
            match store.get(&k, None).await {
                Ok(bytes) => return Ok((k, bytes)),
                Err(skynbd_store::StoreError::NotFound(_)) if i + 1 < self.vols.len() => {}
                Err(e) => return Err(e.into()),
            }
        }
        unreachable!("loop returns on last namespace");
    }

    /// The first namespace's key under which the object exists, if any.
    pub(crate) async fn find<S: ObjectStore>(
        &self,
        store: &S,
        make_key: &(dyn Fn(VolumeId) -> String + Sync),
    ) -> Result<Option<String>> {
        for vol in &self.vols {
            let k = make_key(*vol);
            if store.exists(&k).await? {
                return Ok(Some(k));
            }
        }
        Ok(None)
    }
}

/// Fan-out for the tree walks. These walks are pure round-trip latency —
/// a 50 GiB volume's map is thousands of small nodes, and fetching them
/// one at a time costs tens of seconds against S3 while moving only a few
/// MiB. Ordered (`buffered`, not `buffer_unordered`) so a walk's output is
/// a function of the tree alone; well under S3's per-prefix request
/// ceiling even with several volumes opening at once. The default for
/// [`EngineConfig::walk_concurrency`].
pub const WALK_CONCURRENCY: usize = 32;

/// Fetch tree nodes concurrently, preserving `hashes` order. Resolution
/// still falls through the clone base chain per node.
pub(crate) async fn fetch_nodes<S: ObjectStore>(
    store: &S,
    resolver: &Resolver,
    hashes: &[NodeHash],
) -> Result<Vec<Bytes>> {
    use futures_util::stream::{StreamExt, TryStreamExt};
    futures_util::stream::iter(hashes.iter().copied().map(|h| async move {
        // `h` is copied into the future rather than borrowed: a closure
        // capturing a reference here makes the walk's `Send` bound
        // higher-ranked, which callers inside `tokio::spawn` cannot prove.
        resolver.get(store, &move |v| keys::node(v, &h), None).await
    }))
    .buffered(resolver.walk)
    .try_collect()
    .await
}

/// Load a volume's clone base chain (nearest base first) by following
/// the per-volume `base` objects. Cycle-guarded.
pub(crate) async fn load_base_chain<S: ObjectStore>(
    store: &S,
    vol: VolumeId,
) -> Result<Vec<VolumeId>> {
    let mut chain = Vec::new();
    let mut seen = std::collections::HashSet::from([vol]);
    let mut cur = vol;
    loop {
        match store.get(&keys::clone_base(cur), None).await {
            Ok(bytes) => {
                let cb = CloneBase::decode(&bytes)?;
                if !seen.insert(cb.base_vol) {
                    return Err(EngineError::Corrupt(format!(
                        "clone base cycle at {}",
                        cb.base_vol
                    )));
                }
                chain.push(cb.base_vol);
                cur = cb.base_vol;
            }
            Err(skynbd_store::StoreError::NotFound(_)) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(chain)
}

struct OpenSegment {
    id: SegmentId,
    buf: BytesMut,
}

impl OpenSegment {
    fn new() -> Self {
        Self {
            id: SegmentId(ulid::Ulid::new()),
            buf: BytesMut::new(),
        }
    }
}

/// The permits an engine's reads draw from: its config's shared pool,
/// or a pool of its own.
fn read_gets(cfg: &EngineConfig) -> std::sync::Arc<tokio::sync::Semaphore> {
    let pool = cfg
        .read_gets
        .clone()
        .unwrap_or_else(|| ReadGets::new(cfg.max_read_gets));
    pool.0
}

/// A store-resident piece of a read: `len` bytes at `offset` in `obj`,
/// destined for `out[dst..]` (see [`Engine::read`]).
struct ColdRange {
    obj: maintenance::DataKey,
    offset: u64,
    len: u64,
    dst: usize,
}

pub struct Engine<S, R> {
    store: S,
    refs: R,
    /// The engine-wide cap on in-flight data GETs
    /// ([`EngineConfig::max_read_gets`]), shared by every read.
    read_gets: std::sync::Arc<tokio::sync::Semaphore>,
    cfg: EngineConfig,
    vol_id: VolumeId,
    size: u64,
    /// The ref this engine commits to: "head" for volumes, a hidden
    /// "prepared/{id}" for prepared snapshots (design D12).
    target_ref: String,
    head: RefValue,
    head_commit: Commit,
    /// Committed state, fully materialized (M1 model).
    /// The volume's shard size, adopted from its superblock. Fixed for
    /// the volume's lifetime: the shard index is positional, so changing
    /// it would silently re-interpret every root already written.
    /// `EngineConfig::shard_bytes` only chooses it at create.
    shard_bytes: u64,
    committed: index::CommittedIndex,
    /// Un-committed writes since the last commit.
    overlay: ExtentMap<Placed>,
    open_seg: OpenSegment,
    /// Bytes of open + sealed-but-uncommitted segments, served for reads
    /// and uploaded at commit.
    pending_segments: HashMap<SegmentId, Bytes>,
    sealed_order: Vec<SegmentId>,
    /// Own-namespace-first key resolution; falls through the clone base
    /// chain (design §4.1). Trivial (single namespace) for non-clones.
    resolver: Resolver,
}

impl<S: ObjectStore, R: RefStore> Engine<S, R> {
    // ---------------------------------------------------------- lifecycle

    /// Create a volume: superblock + empty commit + head ref (design §12).
    pub async fn create_volume(
        store: S,
        refs: R,
        vol_id: VolumeId,
        size: u64,
        cfg: EngineConfig,
    ) -> Result<Self> {
        if size == 0 || !size.is_multiple_of(SECTOR) {
            return Err(EngineError::InvalidIo(format!("bad volume size {size}")));
        }
        let sb = Superblock {
            vol_id,
            size,
            format_major: 1,
            created_at_ms: now_ms(),
            shard_bytes: cfg.shard_bytes,
        };
        store
            .put(&keys::superblock(vol_id, 0), sb.encode()?.into())
            .await?;

        let shard_count = size.div_ceil(cfg.shard_bytes) as usize;
        let index = Node::ShardIndex(vec![None; shard_count]);
        let index_hash = index.content_hash()?;
        store
            .put(&keys::node(vol_id, &index_hash), index.encode()?.into())
            .await?;

        let commit = Commit {
            vol_id,
            parent: None,
            shard_index: index_hash,
            segments: vec![],
            flush_seq: 0,
            created_at_ms: now_ms(),
            kind: CommitKind::Checkpoint,
            group: None,
            epoch: cfg.epoch,
        };
        let commit_hash = commit.content_hash()?;
        store
            .put(&keys::commit(vol_id, &commit_hash), commit.encode()?.into())
            .await?;

        let head = match refs
            .compare_and_swap(vol_id, "head", None, Some(commit_hash))
            .await?
        {
            CasOutcome::Committed(Some(rv)) => rv,
            CasOutcome::Conflict(_) => return Err(EngineError::Conflict),
            CasOutcome::Committed(None) => unreachable!("created a value"),
        };

        let shard_bytes = cfg.shard_bytes;
        let committed = index::CommittedIndex::empty(size, shard_bytes);
        let walk = cfg.walk_concurrency;
        Ok(Self {
            store,
            refs,
            read_gets: read_gets(&cfg),
            cfg,
            vol_id,
            size,
            target_ref: "head".to_string(),
            head,
            head_commit: commit,
            shard_bytes,
            committed,
            overlay: ExtentMap::new(),
            open_seg: OpenSegment::new(),
            pending_segments: HashMap::new(),
            sealed_order: vec![],
            resolver: Resolver::new(vol_id, vec![]).walk(walk),
        })
    }

    /// Create a fast clone (CSI design §4.1 option A): a new volume whose
    /// initial commit points at `base_commit`'s shard index — O(1) in
    /// data, true sharing. Reads fall through to the base namespace via
    /// the clone-base marker; new writes land in the clone's own.
    ///
    /// The caller must pin the source snapshot ref (holder
    /// `clone:<name>`) *before* calling this, so the source's GC cannot
    /// sweep the shared closure, and release the pin when the clone is
    /// deleted.
    pub async fn clone_volume(
        store: S,
        refs: R,
        vol_id: VolumeId,
        size: u64,
        base_vol: VolumeId,
        base_commit_hash: CommitHash,
        cfg: EngineConfig,
    ) -> Result<Self> {
        let base_sb_bytes = store.get(&keys::superblock(base_vol, 0), None).await?;
        let base_sb = Superblock::decode(&base_sb_bytes)?;
        if size < base_sb.size {
            return Err(EngineError::InvalidIo(format!(
                "clone size {size} below source size {}; clones may only grow",
                base_sb.size
            )));
        }
        if !size.is_multiple_of(SECTOR) {
            return Err(EngineError::InvalidIo(format!("bad volume size {size}")));
        }

        // The base may itself be a clone: the new chain is base + its chain.
        let mut base_chain = vec![base_vol];
        base_chain.extend(load_base_chain(&store, base_vol).await?);
        let base_resolver =
            Resolver::new(base_vol, base_chain[1..].to_vec()).walk(cfg.walk_concurrency);
        let base_commit = Commit::decode(
            &base_resolver
                .get(&store, &|v| keys::commit(v, &base_commit_hash), None)
                .await?,
        )?;

        // A clone's first commit points at the base's shard index
        // verbatim, so it must read that array on the base's grid. Its
        // geometry is inherited, not configured.
        let shard_bytes = base_sb.shard_bytes;
        let sb = Superblock {
            vol_id,
            size,
            format_major: 1,
            created_at_ms: now_ms(),
            shard_bytes,
        };
        store
            .put(&keys::superblock(vol_id, 0), sb.encode()?.into())
            .await?;
        let cb = CloneBase {
            base_vol,
            base_commit: base_commit_hash,
            created_at_ms: now_ms(),
        };
        store
            .put(&keys::clone_base(vol_id), cb.encode()?.into())
            .await?;

        // Initial commit: the base's shard index, verbatim. History starts
        // fresh (`parent: None`) — provenance lives in the clone-base
        // object, not the commit chain.
        let commit = Commit {
            vol_id,
            parent: None,
            shard_index: base_commit.shard_index,
            segments: vec![],
            flush_seq: 0,
            created_at_ms: now_ms(),
            kind: CommitKind::Import,
            group: None,
            epoch: cfg.epoch,
        };
        let commit_hash = commit.content_hash()?;
        store
            .put(&keys::commit(vol_id, &commit_hash), commit.encode()?.into())
            .await?;

        let head = match refs
            .compare_and_swap(vol_id, "head", None, Some(commit_hash))
            .await?
        {
            CasOutcome::Committed(Some(rv)) => rv,
            CasOutcome::Conflict(_) => return Err(EngineError::Conflict),
            CasOutcome::Committed(None) => unreachable!("created a value"),
        };

        let resolver = Resolver::new(vol_id, base_chain).walk(cfg.walk_concurrency);
        let (head_commit, committed) =
            load_commit_state(&store, &resolver, &commit_hash, size, shard_bytes).await?;
        Ok(Self {
            store,
            refs,
            read_gets: read_gets(&cfg),
            cfg,
            vol_id,
            size,
            target_ref: "head".to_string(),
            head,
            head_commit,
            shard_bytes: sb.shard_bytes,
            committed,
            overlay: ExtentMap::new(),
            open_seg: OpenSegment::new(),
            pending_segments: HashMap::new(),
            sealed_order: vec![],
            resolver,
        })
    }

    /// Open from durable state: superblock, head commit, shard index —
    /// three reads, no closure walk (D-M8). Reachability is verified by
    /// `fsck`, not re-proven at every open; a broken closure surfaces at
    /// the read that touches it, as the store error it is.
    pub async fn open(store: S, refs: R, vol_id: VolumeId, cfg: EngineConfig) -> Result<Self> {
        let sb_bytes = store.get(&keys::superblock(vol_id, 0), None).await?;
        let sb = Superblock::decode(&sb_bytes)?;

        let head = refs
            .get(vol_id, "head")
            .await?
            .ok_or_else(|| EngineError::Corrupt("missing head ref".into()))?;
        let resolver = Resolver::new(vol_id, load_base_chain(&store, vol_id).await?)
            .walk(cfg.walk_concurrency);
        let (head_commit, committed) =
            load_commit_state(&store, &resolver, &head.commit, sb.size, sb.shard_bytes).await?;

        Ok(Self {
            store,
            refs,
            read_gets: read_gets(&cfg),
            cfg,
            vol_id,
            size: sb.size,
            target_ref: "head".to_string(),
            head,
            head_commit,
            shard_bytes: sb.shard_bytes,
            committed,
            overlay: ExtentMap::new(),
            open_seg: OpenSegment::new(),
            pending_segments: HashMap::new(),
            sealed_order: vec![],
            resolver,
        })
    }

    /// Open an engine bound to an arbitrary existing ref (prepared
    /// snapshots, design D12). Commits CAS that ref.
    pub async fn open_ref(
        store: S,
        refs: R,
        vol_id: VolumeId,
        ref_name: &str,
        cfg: EngineConfig,
    ) -> Result<Self> {
        let sb_bytes = store.get(&keys::superblock(vol_id, 0), None).await?;
        let sb = Superblock::decode(&sb_bytes)?;
        let head = refs
            .get(vol_id, ref_name)
            .await?
            .ok_or_else(|| EngineError::Corrupt(format!("missing ref {ref_name}")))?;
        let resolver = Resolver::new(vol_id, load_base_chain(&store, vol_id).await?)
            .walk(cfg.walk_concurrency);
        let (head_commit, committed) =
            load_commit_state(&store, &resolver, &head.commit, sb.size, sb.shard_bytes).await?;
        Ok(Self {
            store,
            refs,
            read_gets: read_gets(&cfg),
            cfg,
            vol_id,
            size: sb.size,
            target_ref: ref_name.to_string(),
            head,
            head_commit,
            shard_bytes: sb.shard_bytes,
            committed,
            overlay: ExtentMap::new(),
            open_seg: OpenSegment::new(),
            pending_segments: HashMap::new(),
            sealed_order: vec![],
            resolver,
        })
    }

    /// Open a read-only view of an arbitrary commit — the serving path
    /// for snapshot exports (design §7.3: snapshot reads walk immutable
    /// trees and never touch the writer). Any `commit()` on this engine
    /// fails with `Conflict` by construction (sentinel generation).
    pub async fn open_at_commit(
        store: S,
        refs: R,
        vol_id: VolumeId,
        commit_hash: CommitHash,
        cfg: EngineConfig,
    ) -> Result<Self> {
        let sb_bytes = store.get(&keys::superblock(vol_id, 0), None).await?;
        let sb = Superblock::decode(&sb_bytes)?;
        let resolver = Resolver::new(vol_id, load_base_chain(&store, vol_id).await?)
            .walk(cfg.walk_concurrency);
        let (commit, committed) =
            load_commit_state(&store, &resolver, &commit_hash, sb.size, sb.shard_bytes).await?;
        Ok(Self {
            store,
            refs,
            read_gets: read_gets(&cfg),
            cfg,
            vol_id,
            size: sb.size,
            target_ref: "head".to_string(),
            head: RefValue {
                commit: commit_hash,
                generation: u64::MAX,
            },
            head_commit: commit,
            shard_bytes: sb.shard_bytes,
            committed,
            overlay: ExtentMap::new(),
            open_seg: OpenSegment::new(),
            pending_segments: HashMap::new(),
            sealed_order: vec![],
            resolver,
        })
    }

    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn flush_seq(&self) -> u64 {
        self.head_commit.flush_seq
    }

    /// The commit this engine currently serves.
    pub fn head_commit_hash(&self) -> CommitHash {
        self.head.commit
    }

    pub fn vol_id(&self) -> VolumeId {
        self.vol_id
    }

    pub(crate) fn refs(&self) -> &R {
        &self.refs
    }

    /// This engine's walk fan-out ([`EngineConfig::walk_concurrency`]).
    pub(crate) fn walk_concurrency(&self) -> usize {
        self.resolver.walk
    }

    pub(crate) fn store(&self) -> &S {
        &self.store
    }

    // ---------------------------------------------------------- snapshots

    /// Create snapshot `name` (design §9.1, D10): force a commit, PUT the
    /// manifest, then CAS-create `snap/<name>`. Manifest-before-ref
    /// ordering guarantees a ref never dangles (§9.1). O(1) beyond the
    /// forced commit: on a clean volume this writes exactly one object.
    pub async fn snapshot_create(&mut self, name: &str) -> Result<(SnapId, CommitHash)> {
        validate_snapshot_name(name)?;
        let ref_name = format!("snap/{name}");
        if self.refs.get(self.vol_id, &ref_name).await?.is_some() {
            return Err(EngineError::SnapshotExists(name.to_string()));
        }

        let commit_hash = self.commit(CommitKind::SnapshotForced).await?;

        let snap_id = SnapId(ulid::Ulid::new());
        let manifest = SnapshotManifest {
            snap_id,
            vol_id: self.vol_id,
            ref_name: ref_name.clone(),
            commit: commit_hash,
            // Lineage arrives with diff/send (M5) and ingestion (M6).
            parent_snap: None,
            kind: SnapKind::Manual,
            created_at_ms: now_ms(),
        };
        self.store
            .put(
                &keys::snapshot(self.vol_id, snap_id),
                manifest.encode()?.into(),
            )
            .await?;

        match self
            .refs
            .compare_and_swap(self.vol_id, &ref_name, None, Some(commit_hash))
            .await?
        {
            CasOutcome::Committed(_) => {
                // The D-M7 cadence: each snapshot leaves its commit's
                // materialized map behind, so this snapshot's export —
                // and the head, until it moves — opens in one GET.
                // Best-effort: the snapshot is published either way.
                if let Err(e) = self.write_emap().await {
                    tracing::warn!(volume = %self.vol_id, error = %e, "emap write failed");
                }
                Ok((snap_id, commit_hash))
            }
            // Lost a race: the manifest is harmless garbage (GC, §10).
            CasOutcome::Conflict(_) => Err(EngineError::SnapshotExists(name.to_string())),
        }
    }

    /// List snapshots as `(name, commit)` from the ref store.
    pub async fn snapshot_list(&self) -> Result<Vec<(String, CommitHash)>> {
        let mut out: Vec<(String, CommitHash)> = self
            .refs
            .list(self.vol_id)
            .await?
            .into_iter()
            .filter_map(|(name, rv)| {
                name.strip_prefix("snap/")
                    .map(|n| (n.to_string(), rv.commit))
            })
            .collect();
        out.sort();
        Ok(out)
    }

    /// Delete snapshot `name`: the ref goes away now; space returns at GC
    /// (design D4, §9.6). TODO(M6): refuse while pinned (I8).
    pub async fn snapshot_delete(&mut self, name: &str) -> Result<()> {
        let ref_name = format!("snap/{name}");
        let holders = self.refs.pins(self.vol_id, &ref_name).await?;
        if !holders.is_empty() {
            return Err(EngineError::Pinned(format!(
                "{name} (holders: {holders:?})"
            )));
        }
        let Some(cur) = self.refs.get(self.vol_id, &ref_name).await? else {
            return Err(EngineError::SnapshotNotFound(name.to_string()));
        };
        match self
            .refs
            .compare_and_swap(self.vol_id, &ref_name, Some(cur), None)
            .await?
        {
            CasOutcome::Committed(_) => Ok(()),
            CasOutcome::Conflict(_) => Err(EngineError::Conflict),
        }
    }

    /// Alias snapshot `from` under a second name `to` (CSI 6h-2: a
    /// generation snapshot published by the agent cycle gains the
    /// CSI-requested name). The alias is a full snapshot ref to the
    /// *same commit* — it lists, serves, clones, and deletes like any
    /// snapshot, and either name keeps the commit's closure alive for
    /// GC. Manifest-before-ref ordering as in [`Self::snapshot_create`].
    /// Idempotent when `to` already points at `from`'s commit.
    pub async fn snapshot_alias(&mut self, from: &str, to: &str) -> Result<CommitHash> {
        validate_snapshot_name(to)?;
        let from_ref = format!("snap/{from}");
        let Some(src) = self.refs.get(self.vol_id, &from_ref).await? else {
            return Err(EngineError::SnapshotNotFound(from.to_string()));
        };
        let to_ref = format!("snap/{to}");
        if let Some(cur) = self.refs.get(self.vol_id, &to_ref).await? {
            if cur.commit == src.commit {
                return Ok(src.commit);
            }
            return Err(EngineError::SnapshotExists(to.to_string()));
        }
        let manifest = SnapshotManifest {
            snap_id: SnapId(ulid::Ulid::new()),
            vol_id: self.vol_id,
            ref_name: to_ref.clone(),
            commit: src.commit,
            parent_snap: None,
            kind: SnapKind::Manual,
            created_at_ms: now_ms(),
        };
        self.store
            .put(
                &keys::snapshot(self.vol_id, manifest.snap_id),
                manifest.encode()?.into(),
            )
            .await?;
        match self
            .refs
            .compare_and_swap(self.vol_id, &to_ref, None, Some(src.commit))
            .await?
        {
            CasOutcome::Committed(_) => Ok(src.commit),
            CasOutcome::Conflict(_) => Err(EngineError::SnapshotExists(to.to_string())),
        }
    }

    /// CAS the head to a snapshot's commit (design §9.6). Refuses with
    /// un-flushed writes (no silent data loss). Connection-level fencing
    /// of live writers is the API layer's job (design §17 open question).
    pub async fn rollback(&mut self, name: &str) -> Result<CommitHash> {
        if !self.overlay.is_empty() {
            return Err(EngineError::DirtyRollback);
        }
        let ref_name = format!("snap/{name}");
        let Some(snap) = self.refs.get(self.vol_id, &ref_name).await? else {
            return Err(EngineError::SnapshotNotFound(name.to_string()));
        };

        let (commit, committed) = load_commit_state(
            &self.store,
            &self.resolver,
            &snap.commit,
            self.size,
            self.shard_bytes,
        )
        .await?;
        let new_head = match self
            .refs
            .compare_and_swap(self.vol_id, "head", Some(self.head), Some(snap.commit))
            .await?
        {
            CasOutcome::Committed(Some(rv)) => rv,
            CasOutcome::Conflict(_) => return Err(EngineError::Conflict),
            CasOutcome::Committed(None) => unreachable!("swapped in a value"),
        };

        self.head = new_head;
        self.head_commit = commit;
        self.committed = committed;
        self.overlay = ExtentMap::new();
        self.pending_segments.clear();
        self.sealed_order.clear();
        self.open_seg = OpenSegment::new();
        Ok(snap.commit)
    }

    /// Whether [`Engine::rpo_advance`] would be a no-op: nothing
    /// un-committed and the `rpo` ref already at head. Needs only `&self`,
    /// so a periodic caller can check under a shared lock and take the
    /// exclusive one only when there is something to advance.
    pub async fn rpo_is_current(&self) -> Result<bool> {
        if !self.overlay.is_empty() {
            return Ok(false);
        }
        let cur = self.refs.get(self.vol_id, "rpo").await?;
        Ok(cur.is_some_and(|rv| rv.commit == self.head.commit))
    }

    /// Advance the `rpo` ref (design §9.3): same snapshot path with
    /// `ref_name = "rpo"`, replacing the previous manifest's ref. No-ops
    /// when nothing changed since the current rpo commit.
    pub async fn rpo_advance(&mut self) -> Result<Option<CommitHash>> {
        let cur = self.refs.get(self.vol_id, "rpo").await?;
        if self.overlay.is_empty() {
            if let Some(rv) = cur {
                if rv.commit == self.head.commit {
                    return Ok(None);
                }
            }
        }
        let commit_hash = self.commit(CommitKind::SnapshotForced).await?;
        if let Some(rv) = cur {
            if rv.commit == commit_hash {
                return Ok(None);
            }
        }

        let snap_id = SnapId(ulid::Ulid::new());
        let manifest = SnapshotManifest {
            snap_id,
            vol_id: self.vol_id,
            ref_name: "rpo".to_string(),
            commit: commit_hash,
            parent_snap: None,
            kind: SnapKind::Rpo,
            created_at_ms: now_ms(),
        };
        self.store
            .put(
                &keys::snapshot(self.vol_id, snap_id),
                manifest.encode()?.into(),
            )
            .await?;
        match self
            .refs
            .compare_and_swap(self.vol_id, "rpo", cur, Some(commit_hash))
            .await?
        {
            CasOutcome::Committed(_) => Ok(Some(commit_hash)),
            CasOutcome::Conflict(_) => Err(EngineError::Conflict),
        }
    }

    /// Drop every un-committed byte (overlay, pending segments). The
    /// transactional-apply error path (design §9.4: a torn stream changes
    /// nothing) and abort flows use this; acked-but-unflushed NBD writes
    /// are the caller's responsibility to fence first.
    pub fn discard_uncommitted(&mut self) {
        self.overlay = ExtentMap::new();
        self.pending_segments.clear();
        self.sealed_order.clear();
        self.open_seg = OpenSegment::new();
    }

    /// True when no un-committed writes exist.
    pub fn is_clean(&self) -> bool {
        self.overlay.is_empty()
    }

    // ---------------------------------------------------------------- emap

    /// Write the head commit's materialized extent map (design D-M7):
    /// `emap/{hex(head)}`, the object that turns the next open's
    /// thousands of node GETs into one. Called on clean unstage and on
    /// the snapshot cadence, never per commit — the object is the size
    /// of the whole map. Requires a clean overlay: the map must state
    /// exactly what the head commit states.
    pub async fn write_emap(&self) -> Result<()> {
        if !self.overlay.is_empty() {
            return Err(EngineError::DirtyEmap);
        }
        let (objects, shards) = self.committed.to_emap(&self.store, &self.resolver).await?;
        let bytes =
            skynbd_format::encode_emap(self.head.commit, self.shard_bytes, objects, shards)?;
        self.store
            .put(&keys::emap(self.vol_id, &self.head.commit), bytes.into())
            .await?;
        Ok(())
    }

    // ------------------------------------------------------------------ gc

    /// Mark-and-sweep GC (design §10). Runs with the engine held, so no
    /// commit can race the sweep (single-writer model; epochs arrive with
    /// M6 ingestion — see maintenance module docs). Requires a clean
    /// overlay so the head ref covers all acked state (I6).
    pub async fn gc(&mut self) -> Result<GcReport> {
        if !self.overlay.is_empty() {
            return Err(EngineError::DirtyGc);
        }
        let refs = self.refs.list(self.vol_id).await?;
        let ref_names: std::collections::HashSet<String> =
            refs.iter().map(|(n, _)| n.clone()).collect();

        let mut live: std::collections::HashSet<String> = std::collections::HashSet::new();
        live.insert(keys::superblock(self.vol_id, 0));
        // Clone provenance is permanent state, never garbage.
        live.insert(keys::clone_base(self.vol_id));
        // The walk doubles as the repack measurement (task 15f): every
        // live (segment, offset, len) slice, unioned ACROSS refs — two
        // snapshots referencing the same slice count it once — against
        // the totals the introducing commits recorded.
        let mut seg_ranges: std::collections::BTreeMap<skynbd_types::SegmentId, Vec<(u64, u64)>> =
            std::collections::BTreeMap::new();
        let mut seg_totals: std::collections::HashMap<skynbd_types::SegmentId, u64> =
            std::collections::HashMap::new();
        for (_, rv) in &refs {
            let c = maintenance::closure(&self.store, &self.resolver, &rv.commit, false).await?;
            for k in c.keys {
                live.insert(k);
            }
            for (seg, off, len) in c.seg_slices {
                seg_ranges.entry(seg).or_default().push((off, len));
            }
            for m in c.seg_meta {
                seg_totals.insert(m.id, m.len);
            }
        }

        // EnsureObject promises (design §10.2 resurrection rule): every
        // prepared session's pending CAS set is a root.
        for (_key, bytes) in self.refs.meta_list(self.vol_id, "prepared/").await? {
            if let Ok(pm) = postcard::from_bytes::<crate::prepared::PreparedMeta>(&bytes) {
                for h in pm.pending_cas {
                    live.insert(keys::cas_object(self.vol_id, &h));
                }
            }
        }

        // Manifests stay live while their ref exists (design §9.1: a ref
        // never dangles; a deleted ref tombstones its manifest).
        let snap_prefix = format!("{}snap/", keys::volume_prefix(self.vol_id));
        for key in self.store.list(&snap_prefix).await? {
            let bytes = self.store.get(&key, None).await?;
            if let Ok(m) = skynbd_format::SnapshotManifest::decode(&bytes) {
                if ref_names.contains(&m.ref_name) {
                    live.insert(key);
                }
            }
        }

        // An emap lives exactly as long as the commit that names it
        // (D-M7): derive the live emap keys from the live commit keys.
        let commit_prefix = format!("{}commit/", keys::volume_prefix(self.vol_id));
        let emap_prefix = format!("{}emap/", keys::volume_prefix(self.vol_id));
        let live_emaps: Vec<String> = live
            .iter()
            .filter_map(|k| k.strip_prefix(&commit_prefix))
            .map(|hex| format!("{emap_prefix}{hex}"))
            .collect();
        live.extend(live_emaps);

        let mut swept = 0usize;
        for sub in ["seg/", "node/", "commit/", "obj/", "snap/", "emap/"] {
            let prefix = format!("{}{}", keys::volume_prefix(self.vol_id), sub);
            for key in self.store.list(&prefix).await? {
                if !live.contains(&key) {
                    self.store.delete(&key).await?;
                    swept += 1;
                }
            }
        }
        // Merge each segment's slices into disjoint ranges; the sum is
        // its live bytes. `total_bytes` is None when the introducing
        // commit was thinned while slivers remain — the exact shape a
        // repack pass exists to collect.
        let segments = seg_ranges
            .into_iter()
            .map(|(seg, mut ranges)| {
                ranges.sort_unstable();
                let mut live_bytes = 0u64;
                let mut cur: Option<(u64, u64)> = None;
                for (start, len) in ranges {
                    let end = start + len;
                    match &mut cur {
                        Some((_, ce)) if start <= *ce => *ce = (*ce).max(end),
                        _ => {
                            if let Some((cs, ce)) = cur.take() {
                                live_bytes += ce - cs;
                            }
                            cur = Some((start, end));
                        }
                    }
                }
                if let Some((cs, ce)) = cur {
                    live_bytes += ce - cs;
                }
                maintenance::SegmentLiveness {
                    segment: seg.to_string(),
                    total_bytes: seg_totals.get(&seg).copied(),
                    live_bytes,
                }
            })
            .collect();
        Ok(GcReport {
            refs_scanned: refs.len(),
            live_objects: live.len(),
            swept_objects: swept,
            segments,
        })
    }

    // ----------------------------------------------------------------- io

    fn check_io(&self, off: u64, len: u64) -> Result<()> {
        if len == 0 {
            return Err(EngineError::InvalidIo("zero-length io".into()));
        }
        if !off.is_multiple_of(SECTOR) || !len.is_multiple_of(SECTOR) {
            return Err(EngineError::InvalidIo(format!(
                "unaligned io off={off} len={len}"
            )));
        }
        if off.checked_add(len).is_none_or(|end| end > self.size) {
            return Err(EngineError::InvalidIo(format!(
                "io out of bounds off={off} len={len} size={}",
                self.size
            )));
        }
        Ok(())
    }

    /// NBD WRITE (design §7.1): ack after overlay insert; durability at
    /// the next flush (D1).
    pub fn write(&mut self, off: u64, data: &[u8]) -> Result<()> {
        self.check_io(off, data.len() as u64)?;
        let seg_off = self.open_seg.buf.len() as u64;
        self.open_seg.buf.extend_from_slice(data);
        self.overlay.insert(
            off,
            data.len() as u64,
            Placed {
                lba0: off,
                target: Target::Data(DataRef::Seg {
                    segment: self.open_seg.id,
                    offset: seg_off,
                }),
            },
        );
        if self.open_seg.buf.len() as u64 >= self.cfg.segment_max_bytes {
            self.seal_open_segment();
        }
        Ok(())
    }

    /// Ingestion attach (design §9A.3): map a range onto a CAS object.
    /// Alignment/bounds validated; object existence is verified at
    /// publish (closure walk) rather than per-attach.
    pub fn attach_cas(
        &mut self,
        off: u64,
        len: u64,
        hash: [u8; 32],
        obj_offset: u64,
    ) -> Result<()> {
        self.check_io(off, len)?;
        self.overlay.insert(
            off,
            len,
            Placed {
                lba0: off,
                target: Target::Data(DataRef::Cas {
                    hash,
                    offset: obj_offset,
                }),
            },
        );
        Ok(())
    }

    /// NBD TRIM / WRITE_ZEROES (design §7.1): both map to `Zero` extents.
    pub fn write_zeroes(&mut self, off: u64, len: u64) -> Result<()> {
        self.check_io(off, len)?;
        self.overlay.insert(
            off,
            len,
            Placed {
                lba0: off,
                target: Target::Zero,
            },
        );
        Ok(())
    }

    /// NBD READ (design §7.2): overlay over committed; holes read zero.
    ///
    /// Two passes (issue #97 item 4): resolve every run first — copying
    /// open/pending-segment bytes in place and coalescing store-resident
    /// pieces that are contiguous both in their object and in `out` into
    /// one ranged GET — then fetch those up to
    /// [`EngineConfig::read_fanout`] at a time. A large cold read pays
    /// ~N / fan-out round trips, not N chained ones.
    pub async fn read(&self, off: u64, len: u64) -> Result<Vec<u8>> {
        use futures_util::stream::StreamExt;
        self.check_io(off, len)?;
        let mut out = vec![0u8; len as usize];
        let mut cold: Vec<ColdRange> = Vec::new();
        for run in self.overlay.query(off, len) {
            let dst = (run.start - off) as usize;
            match run.value {
                Some(placed) => {
                    self.place(placed.rebased(run.start), run.len, dst, &mut out, &mut cold);
                }
                None => {
                    // Not written since last commit: consult the committed
                    // index, which materializes the touched shards.
                    for crun in self
                        .committed
                        .query(&self.store, &self.resolver, run.start, run.len)
                        .await?
                    {
                        if let Some(target) = crun.value {
                            let dst = (crun.start - off) as usize;
                            self.place(target, crun.len, dst, &mut out, &mut cold);
                        } // None = hole = already zero
                    }
                }
            }
        }
        let (store, resolver) = (&self.store, &self.resolver);
        let gets = &self.read_gets;
        let mut fetches = futures_util::stream::iter(cold.into_iter().map(|c| async move {
            // `c` moves into the future and the key closure captures a
            // copy: borrowing it would make the `Send` bound higher-ranked
            // (see `fetch_nodes`), and the NBD server services reads
            // inside spawned tasks.
            let range = Some(c.offset..c.offset + c.len);
            let obj = c.obj;
            // The engine-wide cap: whatever this read's own fan-out, all
            // reads together hold at most `max_read_gets` GETs.
            let _permit = gets.acquire().await.expect("read_gets is never closed");
            let bytes = resolver.get(store, &move |v| obj.key(v), range).await?;
            if bytes.len() as u64 != c.len {
                return Err(EngineError::Corrupt(format!(
                    "short ranged read: {} of {} bytes",
                    bytes.len(),
                    c.len
                )));
            }
            Ok((c.dst, bytes))
        }))
        .buffer_unordered(self.cfg.read_fanout.clamp(1, MAX_FANOUT));
        while let Some(fetched) = fetches.next().await {
            let (dst, bytes) = fetched?;
            out[dst..dst + bytes.len()].copy_from_slice(&bytes);
        }
        Ok(out)
    }

    /// First pass of [`Engine::read`] for one already-rebased target:
    /// zero and in-memory data are settled now; store-resident data is
    /// queued in `cold`, merged into the previous entry when it simply
    /// continues it (runs arrive in ascending `dst` order).
    fn place(
        &self,
        target: Target,
        len: u64,
        dst: usize,
        out: &mut [u8],
        cold: &mut Vec<ColdRange>,
    ) {
        let (obj, offset) = match target {
            Target::Zero => return, // out pre-zeroed
            Target::Data(DataRef::Seg { segment, offset }) => {
                let src = if segment == self.open_seg.id {
                    Some(&self.open_seg.buf[..])
                } else {
                    self.pending_segments.get(&segment).map(|b| &b[..])
                };
                if let Some(src) = src {
                    out[dst..dst + len as usize]
                        .copy_from_slice(&src[offset as usize..(offset + len) as usize]);
                    return;
                }
                (maintenance::DataKey::Seg(segment), offset)
            }
            Target::Data(DataRef::Cas { hash, offset }) => {
                (maintenance::DataKey::Cas(hash), offset)
            }
        };
        if let Some(last) = cold.last_mut() {
            if last.obj == obj
                && last.offset + last.len == offset
                && last.dst + last.len as usize == dst
            {
                last.len += len;
                return;
            }
        }
        cold.push(ColdRange {
            obj,
            offset,
            len,
            dst,
        });
    }

    /// Allocation runs for `BLOCK_STATUS` (design §8.3): `(len, allocated)`
    /// covering `[off, off+len)` in order, adjacent same-status runs
    /// merged. Holes *and* explicit `Zero` extents report unallocated —
    /// both read as zeros, which is what `base:allocation` describes.
    pub async fn block_status(&self, off: u64, len: u64) -> Result<Vec<(u64, bool)>> {
        self.check_io(off, len)?;
        let mut raw: Vec<(u64, bool)> = Vec::new();
        for run in self.overlay.query(off, len) {
            match run.value {
                Some(p) => raw.push((run.len, matches!(p.target, Target::Data(_)))),
                None => {
                    for c in self
                        .committed
                        .query(&self.store, &self.resolver, run.start, run.len)
                        .await?
                    {
                        raw.push((c.len, matches!(c.value, Some(Target::Data(_)))));
                    }
                }
            }
        }
        let mut out: Vec<(u64, bool)> = Vec::new();
        for (l, a) in raw {
            match out.last_mut() {
                Some(last) if last.1 == a => last.0 += l,
                _ => out.push((l, a)),
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------- commit

    fn seal_open_segment(&mut self) {
        if self.open_seg.buf.is_empty() {
            return;
        }
        let seg = std::mem::replace(&mut self.open_seg, OpenSegment::new());
        self.pending_segments.insert(seg.id, seg.buf.freeze());
        self.sealed_order.push(seg.id);
    }

    /// The single durability path (design §6.3). Maps NBD FLUSH/FUA.
    /// No-ops (returning the current head) when nothing changed.
    pub async fn commit(&mut self, kind: CommitKind) -> Result<CommitHash> {
        if self.overlay.is_empty() {
            return Ok(self.head.commit);
        }
        self.seal_open_segment();

        // 1. PUT all pending segments, recording integrity metadata (D9).
        let mut seg_metas = Vec::with_capacity(self.sealed_order.len());
        for id in &self.sealed_order {
            let bytes = &self.pending_segments[id];
            self.store
                .put(&keys::segment(self.vol_id, *id), bytes.clone())
                .await?;
            seg_metas.push(skynbd_format::SegmentMeta {
                id: *id,
                len: bytes.len() as u64,
                blake3: *blake3::hash(bytes).as_bytes(),
            });
        }

        // 2. Group the overlay by shard, splitting runs that straddle a
        //    boundary — a shard is the unit the tree and the committed
        //    index are both rewritten in.
        let mut overlay_by_shard: std::collections::BTreeMap<usize, Vec<Extent>> =
            std::collections::BTreeMap::new();
        for (start, len, placed) in self.overlay.iter() {
            let mut cursor = start;
            while cursor < start + len {
                let no = (cursor / self.shard_bytes) as usize;
                let stop = ((no as u64 + 1) * self.shard_bytes).min(start + len);
                let e = Extent::new(cursor, stop - cursor, placed.rebased(cursor))
                    .map_err(|e| EngineError::Corrupt(e.to_string()))?;
                overlay_by_shard.entry(no).or_default().push(e);
                cursor = stop;
            }
        }

        // 3. Rewrite touched shards as content-addressed nodes.
        let shard_count = self.size.div_ceil(self.shard_bytes) as usize;
        // The previous index may live in the base namespace (fresh clone).
        let index_hash = self.head_commit.shard_index;
        let old_index_bytes = self
            .resolver
            .get(&self.store, &|v| keys::node(v, &index_hash), None)
            .await?;
        let Node::ShardIndex(mut shard_roots) = Node::decode(&old_index_bytes)? else {
            return Err(EngineError::Corrupt(
                "shard index is not a ShardIndex node".into(),
            ));
        };
        // Growth is legitimate — a clone may be larger than its base, and
        // it starts from the base's index. Shrinkage is not: `resize`
        // would *truncate*, dropping roots and the extents under them,
        // and GC would then reclaim what they referenced. That can only
        // mean the stored geometry and this engine's disagree, which the
        // superblock is supposed to make impossible.
        if shard_roots.len() > shard_count {
            return Err(EngineError::Corrupt(format!(
                "shard index has {} roots but this volume's geometry implies \
                 {shard_count}; refusing to truncate",
                shard_roots.len()
            )));
        }
        shard_roots.resize(shard_count, None);

        // Each touched shard is rewritten from its own extents merged with
        // the overlay's — one sweep over one shard, rather than rebuilding
        // the whole map. `merge` coalesces as it goes (design §7.2): the
        // overlay records one extent per IO, so a sequential writer would
        // otherwise leave one per IO forever, and extent count multiplies
        // leaf nodes, open-walk width and resident memory. The merge is
        // maximal and deterministic, which makes the tree a function of
        // content rather than write history — so equal content yields
        // equal nodes and dedupes across snapshots.
        let mut rebuilt: Vec<(usize, Vec<Extent>, Option<NodeHash>)> =
            Vec::with_capacity(overlay_by_shard.len());
        for (shard_no, overlay) in overlay_by_shard {
            let base = self
                .committed
                .extents(shard_no, &self.store, &self.resolver)
                .await?;
            let merged = index::merge(&base, &overlay);
            let root = self.write_shard_nodes(&merged).await?;
            shard_roots[shard_no] = root;
            rebuilt.push((shard_no, merged, root));
        }
        // Untouched shards are shared, not copied.
        let folded = self.committed.with_shards(rebuilt)?;

        // 4. PUT new shard index + commit object.
        let index = Node::ShardIndex(shard_roots);
        let index_hash = index.content_hash()?;
        self.store
            .put(
                &keys::node(self.vol_id, &index_hash),
                index.encode()?.into(),
            )
            .await?;

        let commit = Commit {
            vol_id: self.vol_id,
            parent: Some(self.head.commit),
            shard_index: index_hash,
            segments: seg_metas,
            flush_seq: self.head_commit.flush_seq + 1,
            created_at_ms: now_ms(),
            kind,
            group: None,
            epoch: self.cfg.epoch,
        };
        let commit_hash = commit.content_hash()?;
        self.store
            .put(
                &keys::commit(self.vol_id, &commit_hash),
                commit.encode()?.into(),
            )
            .await?;

        // 5. CAS the head ref — the visibility point (I1, I2).
        let new_head = match self
            .refs
            .compare_and_swap(
                self.vol_id,
                &self.target_ref,
                Some(self.head),
                Some(commit_hash),
            )
            .await?
        {
            CasOutcome::Committed(Some(rv)) => rv,
            CasOutcome::Conflict(actual) => {
                // The conflict names both sides. The error itself is one
                // opaque line ("head CAS conflict"), and when this fired
                // in CI the logs could not say whose commit had moved
                // the ref — the hashes identify the foreign writer.
                tracing::warn!(
                    vol = %self.vol_id.0,
                    target = %self.target_ref,
                    expected = ?(&self.head.commit[..4], self.head.generation),
                    actual = ?actual.as_ref().map(|a| (&a.commit[..4], a.generation)),
                    "head CAS conflict: the ref moved behind this engine handle"
                );
                return Err(EngineError::Conflict);
            }
            CasOutcome::Committed(None) => unreachable!("swapped in a value"),
        };

        // 6. Only now adopt the new state (a failure above leaves acked
        //    state untouched and the head unmoved — I7).
        self.head = new_head;
        self.head_commit = commit;
        self.committed = folded;
        self.overlay = ExtentMap::new();
        self.pending_segments.clear();
        self.sealed_order.clear();
        Ok(commit_hash)
    }

    /// Serialize one shard's extents into leaf/internal nodes; returns the
    /// shard root hash (None for an empty shard).
    async fn write_shard_nodes(&self, extents: &[Extent]) -> Result<Option<NodeHash>> {
        if extents.is_empty() {
            return Ok(None);
        }
        let mut leaf_refs: Vec<(u64, NodeHash)> = Vec::new();
        for chunk in extents.chunks(skynbd_format::MAX_NODE_ENTRIES) {
            let leaf = Node::leaf_from_extents(chunk)?;
            let hash = leaf.content_hash()?;
            self.store
                .put(&keys::node(self.vol_id, &hash), leaf.encode()?.into())
                .await?;
            leaf_refs.push((chunk[0].lba_start, hash));
        }
        if leaf_refs.len() == 1 {
            return Ok(Some(leaf_refs[0].1));
        }
        if leaf_refs.len() > skynbd_format::MAX_NODE_ENTRIES {
            // 256 × 256 extents per shard exceeded — deeper trees are M2+.
            return Err(EngineError::Corrupt(format!(
                "shard needs {} leaves; deeper trees not yet implemented",
                leaf_refs.len()
            )));
        }
        let internal = Node::Internal(leaf_refs);
        let hash = internal.content_hash()?;
        self.store
            .put(&keys::node(self.vol_id, &hash), internal.encode()?.into())
            .await?;
        Ok(Some(hash))
    }
}

/// Size recorded in a volume's superblock (provisioners default clone
/// sizes from the source without opening an engine).
pub async fn volume_size<S: ObjectStore>(store: &S, vol: VolumeId) -> Result<u64> {
    let sb = Superblock::decode(&store.get(&keys::superblock(vol, 0), None).await?)?;
    Ok(sb.size)
}

/// Load a commit and the *unmaterialized* index over its shard roots —
/// three object reads regardless of volume size (design D-M8). Shared by
/// `open`, `open_ref`, `open_at_commit`, `clone_volume`, and `rollback`.
///
/// Nothing walks the closure here: shards materialize on first touch, and
/// reachability is `fsck`'s job (I1 is *established* by the commit
/// sequence and *verified* at publish and by fsck — §6.2). Resolution
/// still falls through the clone base chain for every object.
async fn load_commit_state<S: ObjectStore>(
    store: &S,
    resolver: &Resolver,
    commit_hash: &CommitHash,
    size: u64,
    shard_bytes: u64,
) -> Result<(Commit, index::CommittedIndex)> {
    let commit_bytes = resolver
        .get(store, &|v| keys::commit(v, commit_hash), None)
        .await?;
    let commit = Commit::decode(&commit_bytes)?;

    let index_bytes = resolver
        .get(store, &|v| keys::node(v, &commit.shard_index), None)
        .await?;
    let Node::ShardIndex(shards) = Node::decode(&index_bytes)? else {
        return Err(EngineError::Corrupt(
            "shard index is not a ShardIndex node".into(),
        ));
    };
    let index = index::CommittedIndex::from_roots(size, shard_bytes, shards)?;
    // The D-M7 shortcut: one GET for the commit's materialized map,
    // where materializing from the tree costs a GET per node. Absent or
    // unusable → the walk, exactly as before; the emap can make an open
    // faster, never wrong and never failing.
    let index = match load_emap(store, resolver, commit_hash, shard_bytes).await {
        Ok(Some(src)) => index.with_emap(src),
        Ok(None) => index,
        Err(e) => {
            tracing::warn!(error = %e, "emap unusable; opens will walk the tree");
            index
        }
    };
    Ok((commit, index))
}

/// Fetch and verify a commit's emap object, if present.
async fn load_emap<S: ObjectStore>(
    store: &S,
    resolver: &Resolver,
    commit_hash: &CommitHash,
    shard_bytes: u64,
) -> Result<Option<index::EmapSource>> {
    let hash = *commit_hash;
    let raw = match resolver
        .get(store, &move |v| keys::emap(v, &hash), None)
        .await
    {
        Ok(bytes) => bytes,
        Err(EngineError::Store(skynbd_store::StoreError::NotFound(_))) => return Ok(None),
        Err(e) => return Err(e),
    };
    let header = skynbd_format::decode_emap_header(&raw)?;
    if header.commit != *commit_hash {
        return Err(EngineError::Corrupt(
            "emap names a different commit than its key".into(),
        ));
    }
    if header.shard_bytes != shard_bytes {
        return Err(EngineError::Corrupt(format!(
            "emap shard grid {} disagrees with the volume's {shard_bytes}",
            header.shard_bytes
        )));
    }
    Ok(Some(index::EmapSource::new(header, raw)?))
}

pub(crate) fn validate_snapshot_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 128
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'));
    if ok {
        Ok(())
    } else {
        Err(EngineError::InvalidIo(format!(
            "bad snapshot name {name:?}: 1-128 chars of [A-Za-z0-9._-]"
        )))
    }
}

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

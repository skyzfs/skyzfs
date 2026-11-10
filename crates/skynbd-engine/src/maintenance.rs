//! GC, fsck, and ref rebuilding (design §10, §6.5).
//!
//! GC model (design §10.1): mark-and-sweep over the immutable object
//! graph. In the single-writer MVP the collector runs while holding the
//! engine (`&mut self`), so no commit can be in flight and the epoch/
//! grace machinery of §10.2 is not yet needed; it activates with the
//! ingestion plane (M6), where `EnsureObject` can resurrect unreachable
//! objects. Roots: every ref of the volume; manifests stay live while
//! their ref exists.
//!
//! fsck (design §14.3 global postcondition): every ref's closure must be
//! complete and content-addresses must verify. `rebuild_refs` (§6.5)
//! reconstructs *missing* refs from S3 alone — existing refs are always
//! authoritative, which keeps post-rollback histories safe (the M3
//! caveat: `flush_seq` is not monotonic across rollbacks).

use std::collections::HashSet;

use futures_util::stream::{StreamExt, TryStreamExt};
use skynbd_format::{Codec, Commit, EmapObj, Node, SegmentMeta, SnapshotManifest, EMAP_ZERO};
use skynbd_refs::{CasOutcome, RefStore};
use skynbd_store::ObjectStore;
use skynbd_types::{CommitHash, NodeHash, VolumeId};

use crate::{keys, EngineError, Result};

/// A closure's worth of data-object key, as a value: which object a leaf
/// extent points at, independent of the namespace it is looked up in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DataKey {
    Seg(skynbd_types::SegmentId),
    Cas(skynbd_types::ObjectHash),
}

impl DataKey {
    pub(crate) fn key(&self, vol: VolumeId) -> String {
        match self {
            DataKey::Seg(s) => keys::segment(vol, *s),
            DataKey::Cas(h) => keys::cas_object(vol, h),
        }
    }
}

/// Verify a content-addressed key's bytes hash to its name.
fn check_hash(key: &str, bytes: &[u8]) -> Result<()> {
    let hex = key.rsplit('/').next().unwrap_or("");
    let actual: String = blake3::hash(bytes)
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if hex == actual {
        Ok(())
    } else {
        Err(EngineError::Corrupt(format!(
            "content hash mismatch at {key}"
        )))
    }
}

/// One commit's reachable closure, plus the segment-level accounting
/// GC's liveness report is built from: every live `(segment, offset,
/// len)` slice the tree references, and the `SegmentMeta` totals the
/// commit itself records. The slices are the raw material for the
/// per-segment live fractions (task 15f) that decide when a segment is
/// a repack candidate — mostly-dead segments are invisible to an
/// object-granularity sweep, so they must be measured where the walk
/// already is.
pub(crate) struct Closure {
    pub keys: Vec<String>,
    pub seg_slices: Vec<(skynbd_types::SegmentId, u64, u64)>,
    pub seg_meta: Vec<SegmentMeta>,
}

/// All S3 keys reachable from a commit, decoding every object on the way
/// (I1 as a checkable property). `verify` additionally checks content
/// addresses. Keys resolve through the clone base chain; the returned
/// keys are where each object was actually found, so a clone's closure
/// may span multiple volume namespaces (its own GC sweep only iterates
/// its own prefix, so foreign keys are inert there).
pub(crate) async fn closure_keys<S: ObjectStore>(
    store: &S,
    resolver: &crate::Resolver,
    commit_hash: &CommitHash,
    verify: bool,
) -> Result<Vec<String>> {
    Ok(closure(store, resolver, commit_hash, verify).await?.keys)
}

pub(crate) async fn closure<S: ObjectStore>(
    store: &S,
    resolver: &crate::Resolver,
    commit_hash: &CommitHash,
    verify: bool,
) -> Result<Closure> {
    let mut out = Vec::new();
    let mut seg_slices = Vec::new();

    let (ckey, cbytes) = resolver
        .get_with_key(store, &|v| keys::commit(v, commit_hash))
        .await?;
    if verify {
        check_hash(&ckey, &cbytes)?;
    }
    let commit = Commit::decode(&cbytes)?;
    let seg_meta = commit.segments.clone();
    out.push(ckey);

    let (ikey, ibytes) = resolver
        .get_with_key(store, &|v| keys::node(v, &commit.shard_index))
        .await?;
    if verify {
        check_hash(&ikey, &ibytes)?;
    }
    let Node::ShardIndex(shards) = Node::decode(&ibytes)? else {
        return Err(EngineError::Corrupt(
            "shard index is not a ShardIndex node".into(),
        ));
    };
    out.push(ikey);

    // Fetched two flat levels at a time (shard roots, then leaves) so the
    // walk's fan-out is bounded but not serialized — see `fetch_nodes`.
    let roots: Vec<NodeHash> = shards.iter().flatten().copied().collect();
    let mut leaves: Vec<Vec<skynbd_types::Extent>> = Vec::new();
    let mut children: Vec<NodeHash> = Vec::new();
    for (nkey, nbytes) in fetch_keyed_nodes(store, resolver, &roots).await? {
        if verify {
            check_hash(&nkey, &nbytes)?;
        }
        out.push(nkey);
        match Node::decode(&nbytes)? {
            leaf @ Node::Leaf(_) => leaves.push(leaf.leaf_extents()?),
            Node::Internal(kids) => children.extend(kids.into_iter().map(|(_, h)| h)),
            Node::ShardIndex(_) => {
                return Err(EngineError::Corrupt("nested shard index".into()));
            }
        }
    }
    for (lkey, lbytes) in fetch_keyed_nodes(store, resolver, &children).await? {
        if verify {
            check_hash(&lkey, &lbytes)?;
        }
        out.push(lkey);
        leaves.push(Node::decode(&lbytes)?.leaf_extents()?);
    }

    // Owned, `Copy` key descriptors rather than boxed closures: a future
    // that captures a reference to a trait object makes this walk's bounds
    // higher-ranked, which axum handlers and `tokio::spawn` callers cannot
    // prove.
    let mut data_keys: Vec<DataKey> = Vec::new();
    let mut seen = HashSet::new();
    for extents in leaves {
        for e in extents {
            let dk = match e.target {
                skynbd_types::Target::Zero => continue,
                skynbd_types::Target::Data(skynbd_types::DataRef::Seg { segment, offset }) => {
                    seg_slices.push((segment, offset, e.len));
                    DataKey::Seg(segment)
                }
                skynbd_types::Target::Data(skynbd_types::DataRef::Cas { hash, .. }) => {
                    DataKey::Cas(hash)
                }
            };
            if seen.insert(dk) {
                data_keys.push(dk);
            }
        }
    }
    // Existence probes dominate a data-heavy closure (one per segment or
    // CAS object), so they get the same fan-out as the node fetches.
    let found: Vec<Option<String>> = futures_util::stream::iter(
        data_keys
            .iter()
            .copied()
            .map(|dk| async move { resolver.find(store, &move |v| dk.key(v)).await }),
    )
    .buffered(resolver.walk)
    .try_collect()
    .await?;
    for (found_key, dk) in found.into_iter().zip(&data_keys) {
        match found_key {
            Some(k) => out.push(k),
            None => {
                return Err(EngineError::Corrupt(format!(
                    "missing data object {}",
                    dk.key(resolver.own_vol())
                )));
            }
        }
    }
    Ok(Closure {
        keys: out,
        seg_slices,
        seg_meta,
    })
}

/// [`crate::fetch_nodes`] for the closure walks, which also need the key
/// each node was found under (content addresses are verified against it).
async fn fetch_keyed_nodes<S: ObjectStore>(
    store: &S,
    resolver: &crate::Resolver,
    hashes: &[NodeHash],
) -> Result<Vec<(String, bytes::Bytes)>> {
    futures_util::stream::iter(hashes.iter().copied().map(|h| async move {
        resolver
            .get_with_key(store, &move |v| keys::node(v, &h))
            .await
    }))
    .buffered(resolver.walk)
    .try_collect()
    .await
}

/// Does the store hold history for this volume beyond a bare create?
///
/// `Engine::open` cannot distinguish "no such volume" from "volume exists
/// but its head ref is gone" by error alone — the first is a `NotFound` on
/// the superblock, the second a `Corrupt`. Provisioners need the
/// difference: creating over a volume whose refs were merely lost (§6.5,
/// or a peer's refs store that this process cannot see) republishes it as
/// an empty disk, which a CSI node plugin will then happily `mkfs`.
///
/// True for: any snapshot manifest, any commit past the initial one, any
/// clone. False for a volume that was created and never written — which is
/// content-identical to a fresh one, so re-creating it loses nothing.
pub async fn has_history<S: ObjectStore>(store: &S, vol: VolumeId) -> Result<bool> {
    if store.exists(&keys::clone_base(vol)).await? {
        return Ok(true);
    }
    let prefix = keys::volume_prefix(vol);
    if !store.list(&format!("{prefix}snap/")).await?.is_empty() {
        return Ok(true);
    }
    for key in store.list(&format!("{prefix}commit/")).await? {
        let bytes = store.get(&key, None).await?;
        match Commit::decode(&bytes) {
            Ok(c) if c.flush_seq > 0 || c.parent.is_some() => return Ok(true),
            // An undecodable commit object is not proof of nothing.
            Err(_) => return Ok(true),
            Ok(_) => {}
        }
    }
    Ok(false)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GcReport {
    pub refs_scanned: usize,
    pub live_objects: usize,
    pub swept_objects: usize,
    /// Live segments with their live-byte fractions (task 15f): the
    /// union of referenced slices across every live ref, against the
    /// total the introducing commit recorded. The repack decision's
    /// measurement — a segment mostly dead here is invisible to the
    /// object-granularity sweep above.
    pub segments: Vec<SegmentLiveness>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SegmentLiveness {
    pub segment: String,
    /// From the introducing commit's `SegmentMeta`; `None` when that
    /// commit is no longer live (thinned) while slivers of the segment
    /// still are — exactly the shape repack exists to collect.
    pub total_bytes: Option<u64>,
    pub live_bytes: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct FsckReport {
    /// (ref name, error) — empty error list means healthy.
    pub refs: Vec<(String, Option<String>)>,
}

impl FsckReport {
    pub fn ok(&self) -> bool {
        self.refs.iter().all(|(_, e)| e.is_none())
    }
}

/// Verify every ref's closure (existence + content addresses + decode).
/// Clone volumes verify across their base chain.
pub async fn fsck<S: ObjectStore, R: RefStore>(
    store: &S,
    refs: &R,
    vol: VolumeId,
) -> Result<FsckReport> {
    fsck_with(store, refs, vol, crate::WALK_CONCURRENCY).await
}

/// [`fsck`] with the walk fan-out chosen
/// ([`crate::EngineConfig::walk_concurrency`]).
pub async fn fsck_with<S: ObjectStore, R: RefStore>(
    store: &S,
    refs: &R,
    vol: VolumeId,
    walk: usize,
) -> Result<FsckReport> {
    let resolver = crate::Resolver::for_volume(store, vol).await?.walk(walk);
    let mut report = FsckReport { refs: Vec::new() };
    for (name, rv) in refs.list(vol).await? {
        let mut err = closure_keys(store, &resolver, &rv.commit, true)
            .await
            .err()
            .map(|e| e.to_string());
        // D-M7: the emap is a cache of the tree, and fsck is where the
        // two are compared — a disagreeing emap would serve wrong reads
        // on the next open.
        if err.is_none() {
            err = match emap_extents(store, vol, &rv.commit).await {
                Ok(None) => None,
                Ok(Some(from_emap)) => {
                    let walked = commit_extents(store, vol, &rv.commit).await?;
                    (from_emap != walked).then(|| {
                        format!(
                            "emap disagrees with the tree: {} vs {} extents — \
                             delete {} to fall back to the walk",
                            from_emap.len(),
                            walked.len(),
                            keys::emap(vol, &rv.commit)
                        )
                    })
                }
                Err(e) => Some(format!("emap unreadable: {e}")),
            };
        }
        report.refs.push((name, err));
    }
    Ok(report)
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RebuildReport {
    pub head_rebuilt: bool,
    pub refs_rebuilt: Vec<String>,
    pub refs_existing: usize,
}

/// Rebuild *missing* refs from S3 (design §6.5). Existing refs are never
/// touched: they are the source of truth (rollback caveat). Head is
/// recovered as the complete-closure commit with the highest
/// `(epoch, flush_seq, created_at_ms)`; snapshot/rpo refs come from
/// manifests.
///
/// Epoch first is the D-M5 fence filter: a fenced writer's discarded
/// commits are byte-for-byte plausible — same prefix, complete closure,
/// and possibly a *higher* `flush_seq` than the winner ever reached — so
/// ordering by `flush_seq` alone would resurrect exactly the writes the
/// fence discarded. Ownership epochs are monotonic (task 5), so the
/// highest epoch present is the last legitimate owner. When a lease row
/// survives, it also caps the scan: commits claiming an epoch the lease
/// history never granted are ignored outright.
pub async fn rebuild_refs<S: ObjectStore, R: RefStore>(
    store: &S,
    refs: &R,
    vol: VolumeId,
) -> Result<RebuildReport> {
    let resolver = crate::Resolver::for_volume(store, vol).await?;
    let existing: HashSet<String> = refs.list(vol).await?.into_iter().map(|(n, _)| n).collect();
    let mut report = RebuildReport {
        head_rebuilt: false,
        refs_rebuilt: Vec::new(),
        refs_existing: existing.len(),
    };

    if !existing.contains("head") {
        let epoch_cap = refs.lease_get(vol).await?.map(|l| l.epoch);
        let prefix = format!("{}commit/", keys::volume_prefix(vol));
        let mut candidates: Vec<(u64, u64, u64, CommitHash)> = Vec::new();
        for key in store.list(&prefix).await? {
            let bytes = store.get(&key, None).await?;
            if let Ok(c) = Commit::decode(&bytes) {
                if epoch_cap.is_some_and(|cap| c.epoch > cap) {
                    continue; // an epoch the lease history never granted
                }
                let hash = c.content_hash()?;
                candidates.push((c.epoch, c.flush_seq, c.created_at_ms, hash));
            }
        }
        candidates.sort_unstable_by(|a, b| b.cmp(a));
        for (_, _, _, hash) in candidates {
            if closure_keys(store, &resolver, &hash, false).await.is_ok() {
                if let CasOutcome::Committed(_) =
                    refs.compare_and_swap(vol, "head", None, Some(hash)).await?
                {
                    report.head_rebuilt = true;
                }
                break;
            }
        }
    }

    let snap_prefix = format!("{}snap/", keys::volume_prefix(vol));
    for key in store.list(&snap_prefix).await? {
        let bytes = store.get(&key, None).await?;
        let Ok(m) = SnapshotManifest::decode(&bytes) else {
            continue;
        };
        if existing.contains(&m.ref_name) || report.refs_rebuilt.contains(&m.ref_name) {
            continue;
        }
        if closure_keys(store, &resolver, &m.commit, false)
            .await
            .is_ok()
        {
            if let CasOutcome::Committed(_) = refs
                .compare_and_swap(vol, &m.ref_name, None, Some(m.commit))
                .await?
            {
                report.refs_rebuilt.push(m.ref_name);
            }
        }
    }
    Ok(report)
}

/// All extents of a commit, absolute and self-contained (targets valid at
/// `lba_start`). The diff walk (design §9.4) consumes this.
/// Close the cadence gap (design D-M4): under cadenced refs export the
/// head in the refs object may trail the commit stream by up to one
/// export interval. Forward-scan `commit/` for commits **of exactly
/// `owner_epoch`** — the lease row's owner of record, read before the
/// caller's own acquire — newer than the exported head, and CAS the
/// head to the newest complete one.
///
/// The epoch-equality filter is the D-M5 fence applied forward: a
/// fenced predecessor's leftovers carry an older epoch and are ignored
/// no matter how high their `flush_seq` counts; an impostor's higher
/// epoch was never granted by the lease history. It is airtight only
/// under the single-epoch-gap invariant — every owner exports at serve
/// (after running this) and at release — which is why the provisioner
/// calls `flush_volume` right after this returns.
///
/// The scan is bounded by the cadence, not by history: the listing's
/// mtimes (where the backend provides them) skip every commit at or
/// before the exported head's write time.
pub async fn recover_head<S: ObjectStore, R: RefStore>(
    store: &S,
    refs: &R,
    vol: VolumeId,
    owner_epoch: u64,
) -> Result<Option<CommitHash>> {
    recover_head_with(store, refs, vol, owner_epoch, crate::WALK_CONCURRENCY).await
}

/// [`recover_head`] with the closure walks' fan-out chosen
/// ([`crate::EngineConfig::walk_concurrency`]).
pub async fn recover_head_with<S: ObjectStore, R: RefStore>(
    store: &S,
    refs: &R,
    vol: VolumeId,
    owner_epoch: u64,
    walk: usize,
) -> Result<Option<CommitHash>> {
    let Some(head) = refs.get(vol, "head").await? else {
        // Nothing to roll forward from; total loss is rebuild_refs' job.
        return Ok(None);
    };
    let head_key = keys::commit(vol, &head.commit);
    let head_commit = Commit::decode(&store.get(&head_key, None).await?)?;

    let listing = store
        .list_with_mtime(&format!("{}commit/", keys::volume_prefix(vol)))
        .await?;
    // Commits written at or before the exported head cannot be newer
    // than it; unknown mtimes are read to stay correct on backends
    // without them.
    let head_mtime = listing
        .iter()
        .find(|(k, _)| *k == head_key)
        .and_then(|(_, m)| *m);
    let mut candidates: Vec<(u64, u64, CommitHash)> = Vec::new();
    for (key, mtime) in &listing {
        if *key == head_key {
            continue;
        }
        if let (Some(m), Some(h)) = (mtime, head_mtime) {
            if *m < h {
                continue;
            }
        }
        let Ok(c) = Commit::decode(&store.get(key, None).await?) else {
            continue;
        };
        if c.epoch == owner_epoch && c.flush_seq > head_commit.flush_seq {
            candidates.push((c.flush_seq, c.created_at_ms, c.content_hash()?));
        }
    }
    candidates.sort_unstable_by(|a, b| b.cmp(a));

    let resolver = crate::Resolver::for_volume(store, vol).await?.walk(walk);
    for (_, _, hash) in candidates {
        if closure_keys(store, &resolver, &hash, false).await.is_ok() {
            return match refs
                .compare_and_swap(vol, "head", Some(head), Some(hash))
                .await?
            {
                CasOutcome::Committed(_) => Ok(Some(hash)),
                // Someone else moved it under us: their answer stands.
                CasOutcome::Conflict(_) => Ok(None),
            };
        }
    }
    Ok(None)
}

/// A commit's extents as its emap object states them — `None` when no
/// emap exists. The fsck cross-check (D-M7: "fsck can re-walk and
/// compare"); readers use the engine's integrated path, not this.
pub async fn emap_extents<S: ObjectStore>(
    store: &S,
    vol: VolumeId,
    commit_hash: &CommitHash,
) -> Result<Option<Vec<skynbd_types::Extent>>> {
    use skynbd_types::{DataRef, Target};
    let raw = match store.get(&keys::emap(vol, commit_hash), None).await {
        Ok(b) => b,
        Err(skynbd_store::StoreError::NotFound(_)) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let header = skynbd_format::decode_emap_header(&raw)?;
    if header.commit != *commit_hash {
        return Err(EngineError::Corrupt(
            "emap names a different commit than its key".into(),
        ));
    }
    let body_start = skynbd_format::emap_header_end(&raw)?;
    let body = &raw[body_start..];
    let mut out = Vec::new();
    for (no, &(off, len)) in header.shards.iter().enumerate() {
        if len == 0 {
            continue;
        }
        let blob = body
            .get(off as usize..(off + len) as usize)
            .ok_or_else(|| EngineError::Corrupt("emap shard range past body".into()))?;
        let shard = skynbd_format::decode_emap_shard(blob)?;
        let base = no as u64 * header.shard_bytes;
        let mut cursor = base;
        for i in 0..shard.gap.len() {
            let lba_start = cursor + shard.gap[i] as u64;
            let target = match shard.target[i] {
                EMAP_ZERO => Target::Zero,
                t => {
                    let offset = shard.offset[i] as u64;
                    match *header.objects.get(t as usize).ok_or_else(|| {
                        EngineError::Corrupt(format!("emap target {t} past the object table"))
                    })? {
                        EmapObj::Seg(segment) => Target::Data(DataRef::Seg { segment, offset }),
                        EmapObj::Cas(hash) => Target::Data(DataRef::Cas { hash, offset }),
                    }
                }
            };
            out.push(skynbd_types::Extent {
                lba_start,
                len: shard.len[i] as u64,
                target,
            });
            cursor = lba_start + shard.len[i] as u64;
        }
    }
    Ok(Some(out))
}

pub async fn commit_extents<S: ObjectStore>(
    store: &S,
    vol: VolumeId,
    commit_hash: &CommitHash,
) -> Result<Vec<skynbd_types::Extent>> {
    let resolver = crate::Resolver::for_volume(store, vol).await?;
    let commit = Commit::decode(
        &resolver
            .get(store, &|v| keys::commit(v, commit_hash), None)
            .await?,
    )?;
    let ibytes = resolver
        .get(store, &|v| keys::node(v, &commit.shard_index), None)
        .await?;
    let Node::ShardIndex(shards) = Node::decode(&ibytes)? else {
        return Err(EngineError::Corrupt(
            "shard index is not a ShardIndex node".into(),
        ));
    };
    let mut out = Vec::new();
    for root in shards.iter().flatten() {
        let nbytes = resolver.get(store, &|v| keys::node(v, root), None).await?;
        match Node::decode(&nbytes)? {
            leaf @ Node::Leaf(_) => out.extend(leaf.leaf_extents()?),
            Node::Internal(children) => {
                for (_, child) in children {
                    let lbytes = resolver
                        .get(store, &|v| keys::node(v, &child), None)
                        .await?;
                    out.extend(Node::decode(&lbytes)?.leaf_extents()?);
                }
            }
            Node::ShardIndex(_) => {
                return Err(EngineError::Corrupt("nested shard index".into()));
            }
        }
    }
    out.sort_by_key(|e| e.lba_start);
    Ok(out)
}

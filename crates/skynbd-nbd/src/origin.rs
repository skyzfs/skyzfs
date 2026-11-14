//! The repointable origin export (multi-node design D-M15, task 15a).
//!
//! `<vol>/origin` is a read-only export serving the generation named by
//! the volume's `origin` refs meta row (Invariant D). Advancing it never
//! touches the registry entry or the node's dm table: [`repoint`] swaps
//! the engine *inside* the export's existing `Arc<RwLock<..>>`, and since
//! every attached connection cloned the `Export` at negotiation — sharing
//! that `Arc` — in-flight requests complete against the old generation
//! and the very next request resolves the new one (Invariant A).
//!
//! Meta writes emit no watch events, so live repoints are driven by the
//! API handler that wrote the row; [`reconcile`] covers everything else —
//! startup full sync, watch-lag resyncs, and generation refs appearing
//! or vanishing — which is what makes skynbdd restarts and
//! `nbd-client -persist` reconnects resolve through the pointer row.
//! The SERVE PATH calls it synchronously as well (fallibly, before
//! serve returns): the export dies with a release's teardown, and a
//! node whose stage attaches `<vol>/origin` the instant its lineage
//! serve returns must find it there — neither racing the sync task's
//! initial pass nor stranded by one transient failure inside it.
//!
//! **Order (M1.3ai).** Every swap of the export happens under its
//! [`ExportRegistry::swap_lock`], held from reading the row to the
//! swap. [`reconcile`] reads the row under it, so whoever converges
//! last swaps to what the row names then. Without it, a sync task's
//! reconcile that read the row before an API repoint could swap the
//! older generation back in after the API's swap: the export would
//! serve a generation the row no longer names, and whose `origin` pin
//! has moved (Invariants D and E), until the next generation event.
//! The API converges after its commit with [`reconcile`] too, not by
//! swapping to its own target, so two repoints whose swaps would land
//! out of order both swap to the row as it stands.

use std::sync::Arc;

use skynbd_engine::{Engine, EngineError};
use skynbd_refs::RefStore;
use skynbd_types::{CommitHash, VolumeId};
use tokio::sync::RwLock;

use crate::{Export, ExportRegistry, VolumeBinding};

/// The pointer row key (Invariant D): value is a `snap/g<N>` ref name.
pub const ORIGIN_META_KEY: &str = "origin";

/// Pin holder moved atomically with the pointer row, so the generation
/// the pointer names is never deletable — "the pointer pins whatever it
/// names" (Invariant D), enforced rather than assumed. The agent's own
/// §3 pin ordering is belt on top of this.
pub const ORIGIN_PIN_HOLDER: &str = "origin";

pub fn origin_export_name(vol_name: &str) -> String {
    format!("{vol_name}/origin")
}

/// The generation-ordering policy lives in `skynbd-types` so the agent's
/// in-process control-plane mirror shares the exact same monotone
/// decision (Invariant E) without depending on this crate.
pub use skynbd_types::origin::{generation_number, repoint_forward};

/// What the origin serves (or would serve) right now.
#[derive(Debug, Clone)]
pub struct ResolvedOrigin {
    /// `snap/g<N>` ref name.
    pub ref_name: String,
    pub commit: CommitHash,
    /// The pointer row named this. `false` = fallback: the row is absent
    /// or stale and the newest generation was substituted (the D-M15
    /// restage rule — anything older would resurrect old data).
    pub initialized: bool,
}

/// Resolve the pointer row against the live refs. `Ok(None)` = nothing
/// to serve: no row and no published generations.
pub async fn resolve(
    refs: &Arc<dyn RefStore>,
    vol: VolumeId,
) -> Result<Option<ResolvedOrigin>, skynbd_refs::RefError> {
    if let Some(bytes) = refs.meta_get(vol, ORIGIN_META_KEY).await? {
        let ref_name = String::from_utf8_lossy(&bytes).into_owned();
        if let Some(rv) = refs.get(vol, &ref_name).await? {
            return Ok(Some(ResolvedOrigin {
                ref_name,
                commit: rv.commit,
                initialized: true,
            }));
        }
        tracing::warn!(vol = %vol, pointer = %ref_name,
            "origin pointer names a missing ref; resolving to the newest generation");
    }
    let mut newest: Option<(u64, ResolvedOrigin)> = None;
    for (name, rv) in refs.list(vol).await? {
        if let Some(n) = generation_number(&name) {
            if newest.as_ref().is_none_or(|(m, _)| n > *m) {
                newest = Some((
                    n,
                    ResolvedOrigin {
                        ref_name: name,
                        commit: rv.commit,
                        initialized: false,
                    },
                ));
            }
        }
    }
    Ok(newest.map(|(_, r)| r))
}

/// Point `<vol>/origin` at `commit`, creating the export if absent,
/// under the export's [`ExportRegistry::swap_lock`]. A caller that wrote
/// the row converges with [`reconcile`] instead, which reads the row
/// under the same lock: a swap to a commit read before the lock was
/// taken can be older than the row by the time it lands.
pub async fn repoint(
    registry: &ExportRegistry,
    b: &VolumeBinding,
    commit: CommitHash,
) -> Result<(), EngineError> {
    let _order = registry.swap_lock(&origin_export_name(&b.vol_name)).await;
    swap(registry, b, commit).await
}

/// The swap itself, for a caller that holds the export's swap lock. The
/// new engine opens *outside* the export's `RwLock` so reads keep
/// flowing while the committed state loads; the swap itself is one
/// assignment under the exclusive lock every attached connection
/// already shares.
async fn swap(
    registry: &ExportRegistry,
    b: &VolumeBinding,
    commit: CommitHash,
) -> Result<(), EngineError> {
    let name = origin_export_name(&b.vol_name);
    if let Some(existing) = registry.get(&name).await {
        if existing.engine.read().await.head_commit_hash() == commit {
            return Ok(());
        }
        let fresh = Engine::open_at_commit(
            Arc::clone(&b.store),
            Arc::clone(&b.refs),
            b.vol_id,
            commit,
            b.cfg.clone(),
        )
        .await?;
        // NBD size is fixed at negotiation; a size change cannot be
        // swapped under attached clients.
        if fresh.size() != existing.size {
            return Err(EngineError::InvalidIo(format!(
                "origin repoint refused: export size {} != commit size {}",
                existing.size,
                fresh.size()
            )));
        }
        *existing.engine.write().await = fresh;
        tracing::info!(export = %name, "origin repointed");
        return Ok(());
    }
    let engine = Engine::open_at_commit(
        Arc::clone(&b.store),
        Arc::clone(&b.refs),
        b.vol_id,
        commit,
        b.cfg.clone(),
    )
    .await?;
    let size = engine.size();
    registry
        .register(
            &name,
            Export {
                engine: Arc::new(RwLock::new(engine)),
                read_only: true,
                size,
            },
        )
        .await;
    tracing::info!(export = %name, "origin export published");
    Ok(())
}

/// Converge the origin export onto the pointer row: serve what it
/// resolves to while the row exists, serve nothing while it doesn't.
/// The row-absent case deliberately publishes no export — an
/// uninitialized origin tracking "newest" would silently move on every
/// publish, and the whole point of the pointer is that the origin moves
/// only by explicit repoint.
pub async fn reconcile(registry: &ExportRegistry, b: &VolumeBinding) -> Result<(), EngineError> {
    reconcile_with(registry, b, || async {}).await
}

/// [`reconcile`], with `between` awaited after the row is resolved and
/// before the export is swapped: the window a test holds open.
async fn reconcile_with<F: std::future::Future<Output = ()>>(
    registry: &ExportRegistry,
    b: &VolumeBinding,
    between: impl FnOnce() -> F,
) -> Result<(), EngineError> {
    let export_name = origin_export_name(&b.vol_name);
    // Held from the row's reading to the swap (M1.3ai, module doc).
    let _order = registry.swap_lock(&export_name).await;
    let has_row = b
        .refs
        .meta_get(b.vol_id, ORIGIN_META_KEY)
        .await
        .map_err(EngineError::Refs)?
        .is_some();
    if !has_row {
        registry.unregister(&export_name).await;
        return Ok(());
    }
    match resolve(&b.refs, b.vol_id)
        .await
        .map_err(EngineError::Refs)?
    {
        Some(r) => {
            between().await;
            swap(registry, b, r.commit).await
        }
        // Stale row over a volume with no generations left at all.
        None => {
            registry.unregister(&export_name).await;
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use skynbd_engine::EngineConfig;
    use skynbd_refs::{MemoryRefStore, RefOp};
    use skynbd_store::{MemoryStore, ObjectStore};

    /// A volume with two published generations, `snap/g1` and
    /// `snap/g2`, and its binding as `lin`.
    async fn two_generations() -> (VolumeBinding, CommitHash, CommitHash) {
        let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
        let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
        let vol = VolumeId(ulid::Ulid::new());
        let mut engine: crate::DynEngine = Engine::create_volume(
            Arc::clone(&store),
            Arc::clone(&refs),
            vol,
            64 * 1024,
            EngineConfig::default(),
        )
        .await
        .expect("a volume");
        engine.write(0, &[0xA1; 4096]).expect("a write");
        let (_, g1) = engine.snapshot_create("g1").await.expect("g1");
        engine.write(0, &[0xB2; 4096]).expect("a write");
        let (_, g2) = engine.snapshot_create("g2").await.expect("g2");
        let binding = VolumeBinding {
            vol_id: vol,
            vol_name: "lin".into(),
            store,
            refs,
            cfg: EngineConfig::default(),
        };
        (binding, g1, g2)
    }

    async fn point_at(b: &VolumeBinding, ref_name: &str) {
        b.refs
            .transact(
                b.vol_id,
                vec![RefOp::MetaPut {
                    key: ORIGIN_META_KEY.into(),
                    value: ref_name.as_bytes().to_vec(),
                }],
            )
            .await
            .expect("the row");
    }

    async fn served(registry: &ExportRegistry) -> Option<CommitHash> {
        let export = registry.get("lin/origin").await?;
        let commit = export.engine.read().await.head_commit_hash();
        Some(commit)
    }

    /// A reconcile that read the row before a repoint does not undo it
    /// (M1.3ai; testing's flaky `origin_pointer_lifecycle`, the
    /// review's reading). The sync task's reconcile reads the row
    /// (g1) and is held there; the API then moves the row to g2 and
    /// converges the export as it does after its commit. Without an
    /// order between the two, the API's swap lands first and the held
    /// reconcile then swaps g1 back in: the export serves a generation
    /// the row no longer names, and whose pin has moved. With the
    /// order, the API's convergence waits for the held one and then
    /// reads the row itself.
    ///
    /// The test gives the API's convergence 500 ms to finish while the
    /// other is held: unordered, it does (all in memory); ordered, it
    /// cannot. Either way the verdict is the same: the export ends
    /// serving what the row names.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reconcile_that_read_the_row_before_a_repoint_does_not_undo_it() {
        let (binding, g1, g2) = two_generations().await;
        let registry = Arc::new(ExportRegistry::new());
        point_at(&binding, "snap/g1").await;
        reconcile(&registry, &binding).await.expect("g1 served");
        assert_eq!(served(&registry).await, Some(g1));

        let binding = Arc::new(binding);
        let (resolved, was_resolved) = tokio::sync::oneshot::channel();
        let (go, gone) = tokio::sync::oneshot::channel::<()>();
        let stale = tokio::spawn({
            let (registry, binding) = (Arc::clone(&registry), Arc::clone(&binding));
            async move {
                reconcile_with(&registry, &binding, || async move {
                    let _ = resolved.send(());
                    let _ = gone.await;
                })
                .await
            }
        });
        was_resolved.await.expect("the held reconcile read the row");

        point_at(&binding, "snap/g2").await;
        let mut api = tokio::spawn({
            let (registry, binding) = (Arc::clone(&registry), Arc::clone(&binding));
            async move { reconcile(&registry, &binding).await }
        });
        let api_first = tokio::time::timeout(std::time::Duration::from_millis(500), &mut api).await;
        let _ = go.send(());
        stale.await.expect("joined").expect("the held reconcile");
        match api_first {
            Ok(done) => done.expect("joined").expect("the API's convergence"),
            Err(_) => api.await.expect("joined").expect("the API's convergence"),
        }
        assert_eq!(
            served(&registry).await,
            Some(g2),
            "the export serves what the row names, not what a reconcile read before it moved"
        );
    }
}

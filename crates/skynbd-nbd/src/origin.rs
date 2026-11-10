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

/// Point `<vol>/origin` at `commit`, creating the export if absent. The
/// new engine opens *outside* the lock so reads keep flowing while the
/// committed state loads; the swap itself is one assignment under the
/// exclusive lock every attached connection already shares.
pub async fn repoint(
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
    let export_name = origin_export_name(&b.vol_name);
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
        Some(r) => repoint(registry, b, r.commit).await,
        // Stale row over a volume with no generations left at all.
        None => {
            registry.unregister(&export_name).await;
            Ok(())
        }
    }
}

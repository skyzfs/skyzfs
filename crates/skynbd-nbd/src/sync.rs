//! Registry derivation from ref-store watch events (design §3.1, §7.1):
//! snapshot refs *are* exports. One sync task per served volume publishes
//! `<vol>/snap/<name>` read-only exports the moment the ref lands —
//! that's the instant-publication property scenario S3 times.

use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig};
use skynbd_refs::RefStore;
use skynbd_store::ObjectStore;
use skynbd_types::{CommitHash, VolumeId};
use tokio::sync::RwLock;

use crate::{Export, ExportRegistry};

pub struct VolumeBinding {
    pub vol_id: VolumeId,
    pub vol_name: String,
    pub store: Arc<dyn ObjectStore>,
    pub refs: Arc<dyn RefStore>,
    pub cfg: EngineConfig,
}

/// Spawn the sync task: initial full sync from `list`, then incremental
/// from `watch`; a lagged watch stream triggers a full resync.
pub fn spawn_registry_sync(
    registry: Arc<ExportRegistry>,
    binding: VolumeBinding,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut rx = binding.refs.watch();
        if let Err(e) = full_sync(&registry, &binding).await {
            tracing::warn!(vol = %binding.vol_name, error = %e, "initial export sync failed");
        }
        loop {
            match rx.recv().await {
                Ok(ev) => {
                    if ev.vol != binding.vol_id {
                        continue;
                    }
                    if let Some(export_name) = export_name_for(&binding.vol_name, &ev.name) {
                        match ev.value {
                            Some(rv) => {
                                if let Err(e) =
                                    publish(&registry, &binding, &export_name, rv.commit).await
                                {
                                    tracing::warn!(export = %export_name, error = %e,
                                        "snapshot export publication failed");
                                }
                            }
                            None => registry.unregister(&export_name).await,
                        }
                    }
                    // The origin export follows the pointer meta row
                    // (design D-M15), and meta writes emit no events —
                    // the API repoints live state itself. Generation
                    // churn is the signal we do get, and re-resolving on
                    // it heals a stale pointer (its target force-deleted)
                    // without waiting for a full resync.
                    if ev.name.starts_with("snap/") {
                        if let Err(e) = crate::origin::reconcile(&registry, &binding).await {
                            tracing::warn!(vol = %binding.vol_name, error = %e,
                                "origin export reconcile failed");
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if let Err(e) = full_sync(&registry, &binding).await {
                        tracing::warn!(vol = %binding.vol_name, error = %e, "resync failed");
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    })
}

async fn full_sync(
    registry: &ExportRegistry,
    b: &VolumeBinding,
) -> Result<(), skynbd_engine::EngineError> {
    let refs = b.refs.list(b.vol_id).await?;
    let mut desired = Vec::new();
    for (name, rv) in &refs {
        if let Some(export_name) = export_name_for(&b.vol_name, name) {
            publish(registry, b, &export_name, rv.commit).await?;
            desired.push(export_name);
        }
    }
    // Drop exports whose refs vanished while we were lagging.
    let snap_prefix = format!("{}/snap/", b.vol_name);
    let rpo_name = format!("{}/rpo", b.vol_name);
    for existing in registry.names().await {
        let managed = existing.starts_with(&snap_prefix) || existing == rpo_name;
        if managed && !desired.contains(&existing) {
            registry.unregister(&existing).await;
        }
    }
    // Restart/resync resolution of the origin export through its pointer
    // row (design D-M15 Invariant D).
    crate::origin::reconcile(registry, b).await?;
    Ok(())
}

/// Which refs become exports (design D4): `snap/<x>` → `<vol>/snap/<x>`,
/// `rpo` → `<vol>/rpo`. Head is daemon-registered; prepared stays hidden.
fn export_name_for(vol_name: &str, ref_name: &str) -> Option<String> {
    if let Some(snap) = ref_name.strip_prefix("snap/") {
        Some(format!("{vol_name}/snap/{snap}"))
    } else if ref_name == "rpo" {
        Some(format!("{vol_name}/rpo"))
    } else {
        None
    }
}

async fn publish(
    registry: &ExportRegistry,
    b: &VolumeBinding,
    export_name: &str,
    commit: CommitHash,
) -> Result<(), skynbd_engine::EngineError> {
    // Idempotent: re-publishing the same commit is a cheap no-op.
    if let Some(existing) = registry.get(export_name).await {
        if existing.engine.read().await.head_commit_hash() == commit {
            return Ok(());
        }
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
            export_name,
            Export {
                engine: Arc::new(RwLock::new(engine)),
                read_only: true,
                size,
            },
        )
        .await;
    tracing::info!(export = %export_name, "snapshot export published");
    Ok(())
}

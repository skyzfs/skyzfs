//! The D-M6 lease handoff, node side: `NodeStage` asks the control
//! plane to serve the volume (acquiring its lease) before `nbd-client`
//! dials, and `NodeUnstage` releases it on the way out so the next
//! node's stage acquires instead of waiting out the TTL.
//!
//! Both calls are **best-effort by design**: a single-server deployment
//! without leases works exactly as before, a control plane briefly
//! unreachable at unstage must not wedge pod deletion (the TTL is the
//! backstop D-M6 names for every ungraceful path), and a serve refused
//! with 409 surfaces where it belongs — as the attach failure that
//! follows. The trait exists so [`crate::nodemech::NodeMechanics`] can
//! assert the choreography (serve before attach, release after detach)
//! against a recorder in plain `cargo test`.

use crate::cp;

#[async_trait::async_trait]
pub trait Handoff: Send + Sync {
    /// Ask the node's control plane to serve `volume` from the shared
    /// catalog. Called before the NBD attach of a writable stage.
    ///
    /// `node_holder = true` (Invariant F, task 15h — lineage serves)
    /// acquires the lease under THIS NODE's identity; the returned
    /// value is the granted lease epoch, the stage's fence token.
    /// `false` keeps the server-default holder, and the epoch (if any)
    /// is ignored by callers.
    async fn ensure_served(&self, volume: &str, node_holder: bool) -> Option<u64>;
    /// [`Self::ensure_served`] made fallible (task 15h): a `Held` or any
    /// other serve failure FAILS the caller instead of degrading to an
    /// epoch-less (unfenced) stage. Used for the LINEAGE serve of a thin
    /// stage — the granted epoch is the stage's fence token, and staging
    /// without it would let a superseded node's agent mutate the lineage
    /// unfenced. The default delegates to the best-effort path so
    /// recorders/tests and lease-less rigs keep working.
    async fn serve_required(&self, volume: &str, node_holder: bool) -> Result<Option<u64>, String> {
        Ok(self.ensure_served(volume, node_holder).await)
    }
    /// Commit + stop serving + release the lease. Called after the NBD
    /// detach of a writable unstage.
    async fn release(&self, volume: &str);
}

/// D-M15 origin provisioning, node side: a thin volume staging onto a
/// **fresh** cache file has none of the volume's structure (the
/// LBA→block mapping died with the previous node's pool), so the stage
/// attaches the lineage's repointable `<lineage>/origin` export —
/// initializing its pointer row when absent — as the thin device's
/// external origin: unprovisioned blocks read through, writes provision
/// locally, and each advance repoints the export server-side without
/// touching the attach.
///
/// Unlike [`Handoff`], these are NOT best-effort: staging a fresh pool
/// *without* its origin when published generations exist would present
/// a volume with history as a blank disk — the exact silent-loss shape
/// the multi-node design exists to prevent — so errors fail the stage.
#[async_trait::async_trait]
pub trait Origins: Send + Sync {
    /// Initialize `lineage`'s origin pointer (idempotent — an already
    /// initialized pointer is left where it is) and return the
    /// `snap/gN` ref it names; `None` when no generation is published
    /// yet (a brand-new volume).
    async fn init_origin(&self, lineage: &str) -> Result<Option<String>, String>;
    /// The `snap/gN` ref the pointer names, only when the pointer row
    /// is initialized. (The row outliving unstage is the durable marker
    /// that this volume's pool depends on an external origin.)
    async fn origin_ref(&self, lineage: &str) -> Result<Option<String>, String>;
}

/// Control-plane implementation over the driver's existing API client.
pub struct CpHandoff {
    client: cp::Client,
    /// This node's identity (`NODE_ID`) — the lease holder for lineage
    /// serves (Invariant F, task 15h).
    node_id: String,
}

impl CpHandoff {
    pub fn new(client: cp::Client, node_id: impl Into<String>) -> Self {
        Self {
            client,
            node_id: node_id.into(),
        }
    }
}

#[async_trait::async_trait]
impl Handoff for CpHandoff {
    async fn ensure_served(&self, volume: &str, node_holder: bool) -> Option<u64> {
        let holder = node_holder.then_some(self.node_id.as_str());
        match self.client.serve_volume(volume, holder).await {
            Ok(epoch) => {
                tracing::debug!(volume, ?epoch, "control plane serving volume");
                epoch
            }
            // NotCataloged (404) is normal for pre-existing/config
            // volumes; Held (409) will surface as the attach failure.
            Err(e) => {
                tracing::warn!(volume, error = %e, "serve request failed (continuing)");
                None
            }
        }
    }

    /// The fallible variant (task 15h): every failure — `Held`, 404,
    /// transport — propagates, so a thin stage's lineage serve either
    /// yields the real lease grant (epoch included) or fails the stage
    /// for kubelet to retry. Never a silent epoch-less fallback.
    async fn serve_required(&self, volume: &str, node_holder: bool) -> Result<Option<u64>, String> {
        let holder = node_holder.then_some(self.node_id.as_str());
        self.client
            .serve_volume(volume, holder)
            .await
            .map_err(|e| e.to_string())
    }

    async fn release(&self, volume: &str) {
        match self.client.release_volume(volume).await {
            Ok(()) => tracing::debug!(volume, "lease released"),
            Err(e) => {
                tracing::warn!(volume, error = %e,
                    "lease release failed; the next holder waits out the TTL")
            }
        }
    }
}

/// Control-plane implementation of [`Origins`].
pub struct CpOrigins {
    client: cp::Client,
}

impl CpOrigins {
    pub fn new(client: cp::Client) -> Self {
        Self { client }
    }
}

#[async_trait::async_trait]
impl Origins for CpOrigins {
    async fn init_origin(&self, lineage: &str) -> Result<Option<String>, String> {
        self.client
            .init_origin(lineage)
            .await
            .map_err(|e| format!("initializing {lineage} origin pointer: {e}"))
    }

    async fn origin_ref(&self, lineage: &str) -> Result<Option<String>, String> {
        self.client
            .origin_ref(lineage)
            .await
            .map_err(|e| format!("reading {lineage} origin pointer: {e}"))
    }
}

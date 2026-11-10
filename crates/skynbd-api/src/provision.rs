//! Dynamic volume lifecycle (design §12.1; the CSI driver's
//! CreateVolume/DeleteVolume dependency, auth design §9.3).
//!
//! The [`Provisioner`] owns the serving machinery a volume needs beyond
//! its engine — head export registration, the snapshot-export sync task,
//! the RPO ticker — and a durable catalog of dynamically-created volumes
//! so a daemon restart re-serves them. The catalog lives in the refs
//! store as meta rows under a reserved catalog volume id, so it inherits
//! the refs store's durability (redb in production) with no new state
//! file. Config-defined `[[volumes]]` are served through the same path
//! but stay config-owned: they are not cataloged and cannot be deleted
//! over the API.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_nbd::{spawn_registry_sync, DynEngine, Export, ExportRegistry, VolumeBinding};
use skynbd_refs::{LeaseConfig, LeaseOutcome, RefOp, RefStore, TxnOutcome};
use skynbd_store::{ObjectStore, StoreError};
use skynbd_types::VolumeId;
use tokio::sync::{Mutex, RwLock};

use crate::VolumeHandle;

/// Reserved catalog "volume": the NUL prefix is unreachable through
/// [`validate_name`], so it can never collide with a real volume.
const CATALOG_NAME: &str = "\u{0}skynbd-catalog";

/// Catalog row key for a dynamic volume.
fn catalog_key(name: &str) -> String {
    format!("vol/{name}")
}

/// Deterministic name → id derivation (the daemon has always used this;
/// it is what makes the catalog a pure name list — ids need no storage).
pub fn volume_id(name: &str) -> VolumeId {
    let h = blake3::hash(name.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h.as_bytes()[..16]);
    VolumeId(ulid::Ulid::from(u128::from_be_bytes(b)))
}

pub fn catalog_volume_id() -> VolumeId {
    volume_id(CATALOG_NAME)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VolumeSpec {
    pub name: String,
    /// For clones (`from` set), 0 = inherit the source volume's size;
    /// otherwise must be ≥ it (clones may only grow).
    #[serde(default)]
    pub size: u64,
    /// "attached" (writable NBD head) or "lineage" (design D14: no head;
    /// content arrives via the ingestion plane).
    #[serde(default = "default_kind")]
    pub kind: String,
    /// Fast clone source, `"<volume>/<snapshot>"` (CSI design §4.1): the
    /// new volume starts at the snapshot's content and diverges. The
    /// source snapshot is pinned (`clone:<name>`) for the clone's
    /// lifetime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

fn default_kind() -> String {
    "attached".into()
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    /// → 400.
    #[error("invalid volume spec: {0}")]
    BadSpec(String),
    /// Same name, different shape. → 409.
    #[error("volume exists with size {actual}, requested {requested}")]
    Mismatch { actual: u64, requested: u64 },
    /// Config-defined volumes are not deletable over the API. → 409.
    #[error("volume {0:?} is config-defined, not dynamically provisioned")]
    NotDynamic(String),
    /// Clone source volume/snapshot does not exist. → 404.
    #[error("clone source not found: {0}")]
    SourceMissing(String),
    /// Snapshots of this volume anchor live clones. → 409.
    #[error("volume {volume:?} has live clones: {holders:?}")]
    HasClones {
        volume: String,
        holders: Vec<String>,
    },
    /// The store holds this volume but the refs store does not know it.
    /// → 409: recoverable, but only by an operator decision. → §6.5.
    #[error(
        "volume {0:?} exists in the object store but has no head ref here; \
         its refs were lost or belong to another server — recover with \
         `skynbdctl fsck <config> {0} --rebuild-refs`, do not re-create"
    )]
    RefsLost(String),
    /// Another server holds the volume's lease (design D-M3). → 409:
    /// retryable — the holder may release, or its TTL may lapse.
    #[error("volume {volume:?} is owned by {holder:?} until its lease expires or is released")]
    Held { volume: String, holder: String },
    /// Serve-by-name of a volume the shared catalog does not know. → 404.
    #[error("volume {0:?} is not in the catalog")]
    NotCataloged(String),
    /// → 500.
    #[error("engine: {0}")]
    Engine(#[from] EngineError),
    /// → 500.
    #[error("catalog: {0}")]
    Catalog(String),
    /// The origin export could not be reconciled at serve time (D-M15
    /// Invariant D): the serve fails rather than returning with a dead
    /// export, so the caller's retry retries the reconcile. Its own
    /// variant so the failure is attributable at the serve boundary —
    /// serve newly carries this failure mode (fail-visible over
    /// succeed-broken), and a bare engine error there would read as
    /// the open path's. → 500, retryable.
    #[error("origin export reconcile for {volume:?}: {source}")]
    OriginReconcile { volume: String, source: EngineError },
}

/// `Existing` = idempotent replay of an identical create (CSI requires
/// CreateVolume to be repeatable).
pub enum Created {
    New(VolumeHandle),
    Existing(VolumeHandle),
}

/// The catalog-rebuild half of D-M9, callable without a [`Provisioner`]
/// (skynbdctl runs it against a stopped daemon's config): list the
/// name-index markers, restore every row the catalog is missing, skip
/// what it already has (existing rows are the source of truth, the
/// `rebuild_refs` rule). Marker bytes are the row bytes — unreadable
/// markers are reported by name in the error rather than skipped
/// silently becoming a half-restored namespace.
pub async fn rebuild_catalog(
    store: &Arc<dyn ObjectStore>,
    refs: &Arc<dyn RefStore>,
) -> Result<Vec<String>, ProvisionError> {
    let refs_err = |e: skynbd_refs::RefError| ProvisionError::Catalog(e.to_string());
    let prefix = skynbd_engine::keys::vol_index_prefix();
    let mut restored = Vec::new();
    for key in store
        .list(&prefix)
        .await
        .map_err(|e| ProvisionError::Catalog(e.to_string()))?
    {
        let Some(name) = key.strip_prefix(&prefix) else {
            continue;
        };
        if refs
            .meta_get(catalog_volume_id(), &catalog_key(name))
            .await
            .map_err(refs_err)?
            .is_some()
        {
            continue;
        }
        let row = store
            .get(&key, None)
            .await
            .map_err(|e| ProvisionError::Catalog(e.to_string()))?;
        if serde_json::from_slice::<VolumeSpec>(&row).is_err() {
            return Err(ProvisionError::Catalog(format!(
                "name-index marker for {name:?} is not a volume spec; \
                 refusing a half-restored catalog"
            )));
        }
        refs.transact(
            catalog_volume_id(),
            vec![RefOp::MetaPut {
                key: catalog_key(name),
                value: row.to_vec(),
            }],
        )
        .await
        .map_err(refs_err)?;
        restored.push(name.to_string());
    }
    restored.sort();
    Ok(restored)
}

/// DNS-label-ish, and structurally incapable of colliding with the
/// catalog name or escaping the `<vol>/<ref>` export namespace.
pub fn validate_name(name: &str) -> Result<(), ProvisionError> {
    let ok_len = !name.is_empty() && name.len() <= 63;
    let ok_chars = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    let ok_edges = name.starts_with(|c: char| c.is_ascii_alphanumeric());
    if ok_len && ok_chars && ok_edges {
        Ok(())
    } else {
        Err(ProvisionError::BadSpec(format!(
            "name {name:?} must be 1-63 chars of [A-Za-z0-9._-], starting alphanumeric"
        )))
    }
}

struct Serving {
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

#[derive(Default)]
struct Inner {
    serving: HashMap<String, Serving>,
    /// Names created over the API (catalog-backed) — the deletable set.
    dynamic: HashSet<String>,
}

pub struct Provisioner {
    store: Arc<dyn ObjectStore>,
    refs: Arc<dyn RefStore>,
    registry: Arc<ExportRegistry>,
    /// Seconds between RPO auto-advances; 0 disables (design §9.3).
    rpo_interval_s: u64,
    /// When set, every volume is leased before it is served and released
    /// when deleted (design D-M3). `None` = single-node trust mode.
    lease: Option<LeaseConfig>,
    /// Tuning applied to every engine this provisioner opens (issue #97
    /// items 4–5): only [`EngineConfig::tuning`]'s fields are taken from
    /// it; geometry and the fence epoch stay per volume.
    engine_tuning: EngineConfig,
    /// The live volume table, shared with [`crate::ApiState`].
    pub volumes: Arc<RwLock<HashMap<String, VolumeHandle>>>,
    inner: Mutex<Inner>,
}

impl Provisioner {
    pub fn new(
        store: Arc<dyn ObjectStore>,
        refs: Arc<dyn RefStore>,
        registry: Arc<ExportRegistry>,
        rpo_interval_s: u64,
    ) -> Self {
        Self {
            store,
            refs,
            registry,
            rpo_interval_s,
            lease: None,
            engine_tuning: EngineConfig::default(),
            volumes: Arc::new(RwLock::new(HashMap::new())),
            inner: Mutex::new(Inner::default()),
        }
    }

    pub fn with_lease(mut self, cfg: LeaseConfig) -> Self {
        self.lease = Some(cfg);
        self
    }

    /// Store-fetch tuning for served engines: `read_fanout`,
    /// `walk_concurrency` and `max_read_gets` are taken from `cfg` (see
    /// [`EngineConfig::tuning`]); its other fields are ignored, and each
    /// served volume gets a GET-permit pool of its own. Defaults to the
    /// engine's.
    pub fn with_engine_tuning(mut self, cfg: EngineConfig) -> Self {
        self.engine_tuning = EngineConfig {
            read_gets: None,
            ..cfg.tuning()
        };
        self
    }

    /// This server's own lease-holder identity, when leases are
    /// configured. What the fenced routes compare a volume's recorded
    /// `lease_holder` against to tell server-held from node-held serves.
    pub fn identity(&self) -> Option<&str> {
        self.lease.as_ref().map(|lc| lc.holder.as_str())
    }

    /// Renew every SERVER-held volume's lease (the idle-volume half of
    /// D-M3 — busy volumes renew for free on each commit). Volumes whose
    /// recorded holder is a NODE (task 15h lineage serves) are skipped
    /// on purpose: the node's own fenced writes are its heartbeat (the
    /// object store renews on every matching write, and an
    /// expired-but-matching lease still passes the fence, so an idle
    /// node-held volume re-renews on its next cycle) — if the server
    /// renewed on the node's behalf, a dead node's lease would never
    /// expire and TTL failover would be gone. Returns the names whose
    /// lease came back `Held`: those volumes are lost and their engines
    /// will self-fence on the next write.
    pub async fn renew_leases(&self) -> Vec<String> {
        let Some(lc) = &self.lease else {
            return Vec::new();
        };
        let volumes: Vec<(String, VolumeId, String)> = self
            .volumes
            .read()
            .await
            .iter()
            .filter(|(_, h)| match &h.lease_holder {
                Some(holder) => *holder == lc.holder,
                None => true,
            })
            .map(|(name, h)| {
                let holder = h.lease_holder.clone().unwrap_or_else(|| lc.holder.clone());
                (name.clone(), h.vol_id, holder)
            })
            .collect();
        let mut lost = Vec::new();
        for (name, vid, holder) in volumes {
            match self.refs.lease_acquire(vid, &holder, lc.ttl_ms).await {
                Ok(LeaseOutcome::Acquired(_)) => {}
                Ok(LeaseOutcome::Held(l)) => {
                    tracing::error!(volume = %name, holder = %l.holder, epoch = l.epoch,
                        "lease lost to another server; engine will self-fence on next write");
                    lost.push(name);
                }
                Err(e) => {
                    tracing::warn!(volume = %name, error = %e, "lease renewal failed");
                }
            }
        }
        lost
    }

    /// Serve a config-defined volume (startup): same machinery, but not
    /// cataloged and never deletable over the API.
    pub async fn serve_config_volume(
        &self,
        spec: &VolumeSpec,
    ) -> Result<VolumeHandle, ProvisionError> {
        let mut inner = self.inner.lock().await;
        self.serve(&mut inner, spec, false, None).await
    }

    /// Re-serve every cataloged volume (startup, after config volumes).
    /// Returns how many were loaded.
    pub async fn load_catalog(&self) -> Result<usize, ProvisionError> {
        let rows = self
            .refs
            .meta_list(catalog_volume_id(), "vol/")
            .await
            .map_err(|e| ProvisionError::Catalog(e.to_string()))?;
        let mut inner = self.inner.lock().await;
        let mut n = 0;
        for (key, value) in rows {
            let spec: VolumeSpec = match serde_json::from_slice(&value) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(row = %key, error = %e, "skipping unreadable catalog row");
                    continue;
                }
            };
            if inner.serving.contains_key(&spec.name) {
                // A config volume shadowed a cataloged one: config wins,
                // but keep the row — removing it silently would surprise.
                tracing::warn!(volume = %spec.name,
                    "cataloged volume shadowed by config-defined volume");
                continue;
            }
            self.serve(&mut inner, &spec, true, None).await?;
            n += 1;
        }
        Ok(n)
    }

    /// Create (or idempotently re-observe) a dynamic volume. With
    /// `from = "<vol>/<snapshot>"` set, the volume is a fast clone (CSI
    /// design §4.1): O(1) creation sharing the source's objects, with the
    /// source snapshot pinned for the clone's lifetime.
    pub async fn create(&self, spec: &VolumeSpec) -> Result<Created, ProvisionError> {
        validate_name(&spec.name)?;
        if !matches!(spec.kind.as_str(), "attached" | "lineage") {
            return Err(ProvisionError::BadSpec(format!(
                "kind {:?} must be \"attached\" or \"lineage\"",
                spec.kind
            )));
        }
        if let Some(from) = spec.from.clone() {
            if spec.kind != "attached" {
                return Err(ProvisionError::BadSpec(
                    "clones are attached volumes".into(),
                ));
            }
            return self.create_clone(spec, &from).await;
        }
        if spec.size == 0 {
            return Err(ProvisionError::BadSpec("size must be > 0".into()));
        }
        let mut inner = self.inner.lock().await;
        if inner.serving.contains_key(&spec.name) {
            let existing = self.volumes.read().await[&spec.name].clone();
            return if existing.size == spec.size {
                Ok(Created::Existing(existing))
            } else {
                Err(ProvisionError::Mismatch {
                    actual: existing.size,
                    requested: spec.size,
                })
            };
        }
        let handle = self.serve(&mut inner, spec, true, None).await?;
        self.write_catalog_row(&mut inner, spec).await?;
        Ok(Created::New(handle))
    }

    /// Fast-clone create: resolve the source snapshot, pin it (holder
    /// `clone:<name>`), then build the clone engine. The pin precedes the
    /// engine so the source's GC can never observe a clone without it;
    /// every failure path backs the pin out.
    async fn create_clone(&self, spec: &VolumeSpec, from: &str) -> Result<Created, ProvisionError> {
        let Some((src_name, snap_name)) = from.split_once('/') else {
            return Err(ProvisionError::BadSpec(format!(
                "from {from:?} must be \"<volume>/<snapshot>\""
            )));
        };
        validate_name(src_name)?;
        let src_vid = volume_id(src_name);
        let ref_name = format!("snap/{snap_name}");
        let refs_err = |e: skynbd_refs::RefError| ProvisionError::Catalog(e.to_string());

        let mut inner = self.inner.lock().await;
        let rv = self
            .refs
            .get(src_vid, &ref_name)
            .await
            .map_err(refs_err)?
            .ok_or_else(|| ProvisionError::SourceMissing(from.to_string()))?;
        let src_size = skynbd_engine::volume_size(&self.store, src_vid)
            .await
            .map_err(|_| ProvisionError::SourceMissing(from.to_string()))?;
        let size = if spec.size == 0 { src_size } else { spec.size };

        if inner.serving.contains_key(&spec.name) {
            let existing = self.volumes.read().await[&spec.name].clone();
            return if existing.size == size {
                Ok(Created::Existing(existing))
            } else {
                Err(ProvisionError::Mismatch {
                    actual: existing.size,
                    requested: size,
                })
            };
        }

        let holder = format!("clone:{}", spec.name);
        let pin = |op: fn(String, String) -> RefOp| vec![op(ref_name.clone(), holder.clone())];
        self.refs
            .transact(src_vid, pin(|name, holder| RefOp::Pin { name, holder }))
            .await
            .map_err(refs_err)?;
        // The snapshot could have been deleted between the get and the
        // pin; a pin on a deleted ref protects nothing — back out.
        if self
            .refs
            .get(src_vid, &ref_name)
            .await
            .map_err(refs_err)?
            .is_none()
        {
            let _ = self
                .refs
                .transact(src_vid, pin(|name, holder| RefOp::Unpin { name, holder }))
                .await;
            return Err(ProvisionError::SourceMissing(from.to_string()));
        }

        let mut effective = spec.clone();
        effective.size = size;
        let result = async {
            let handle = self
                .serve_new_clone(&mut inner, &effective, src_vid, rv.commit)
                .await?;
            self.write_catalog_row(&mut inner, &effective).await?;
            Ok(handle)
        }
        .await;
        match result {
            Ok(handle) => {
                tracing::info!(volume = %spec.name, from, size, "volume cloned");
                Ok(Created::New(handle))
            }
            Err(e) => {
                let _ = self
                    .refs
                    .transact(src_vid, pin(|name, holder| RefOp::Unpin { name, holder }))
                    .await;
                Err(e)
            }
        }
    }

    /// Catalog row last: a crash between engine creation and this write
    /// leaves an uncataloged (invisible) volume, which the next identical
    /// create simply re-opens.
    async fn write_catalog_row(
        &self,
        inner: &mut Inner,
        spec: &VolumeSpec,
    ) -> Result<(), ProvisionError> {
        let row = serde_json::to_vec(spec).expect("spec serializes");
        let outcome = self
            .refs
            .transact(
                catalog_volume_id(),
                vec![RefOp::MetaPut {
                    key: catalog_key(&spec.name),
                    value: row.clone(),
                }],
            )
            .await
            .map_err(|e| ProvisionError::Catalog(e.to_string()))?;
        if !matches!(outcome, TxnOutcome::Committed) {
            self.teardown(inner, &spec.name).await;
            return Err(ProvisionError::Catalog("catalog write conflicted".into()));
        }
        // The D-M9 name-index marker: same spec bytes, in the bucket
        // itself — what makes the catalog reconstructible after total
        // refs loss. Written after the row so a marker never names a
        // volume the catalog denies.
        if let Err(e) = self
            .store
            .put(&skynbd_engine::keys::vol_index(&spec.name), row.into())
            .await
        {
            self.teardown(inner, &spec.name).await;
            return Err(ProvisionError::Catalog(format!("name index write: {e}")));
        }
        Ok(())
    }

    /// Delete a dynamic volume: stop serving it and drop its catalog row.
    /// Idempotent (`Ok(false)` = was not present). The volume's objects
    /// and refs are left in the store — space reclamation for deleted
    /// volumes is a GC concern, not an API-latency one. A clone's pin on
    /// its source snapshot is released here; a volume whose snapshots
    /// anchor live clones refuses deletion.
    pub async fn delete(&self, name: &str) -> Result<bool, ProvisionError> {
        let mut inner = self.inner.lock().await;
        let vid = volume_id(name);
        let refs_err = |e: skynbd_refs::RefError| ProvisionError::Catalog(e.to_string());
        // Live clones fall through to this volume's namespace for reads;
        // deleting it out from under them is refused, mirroring the
        // engine's pinned-snapshot rule.
        let mut holders = Vec::new();
        for (ref_name, _) in self.refs.list(vid).await.map_err(refs_err)? {
            if ref_name.starts_with("snap/") {
                for h in self.refs.pins(vid, &ref_name).await.map_err(refs_err)? {
                    if h.starts_with("clone:") {
                        holders.push(h);
                    }
                }
            }
        }
        if !holders.is_empty() {
            holders.sort();
            return Err(ProvisionError::HasClones {
                volume: name.to_string(),
                holders,
            });
        }
        // If this volume was itself a clone, its catalog row records the
        // source; the pin is released after teardown.
        let from = self
            .refs
            .meta_get(catalog_volume_id(), &catalog_key(name))
            .await
            .map_err(refs_err)?
            .and_then(|v| serde_json::from_slice::<VolumeSpec>(&v).ok())
            .and_then(|s| s.from);

        if !inner.serving.contains_key(name) {
            // May be a stale catalog row (crash between teardown and row
            // removal): clear it so restarts do not resurrect the volume.
            self.remove_catalog_row(name).await?;
            self.release_clone_pin(name, from.as_deref()).await;
            return Ok(false);
        }
        if !inner.dynamic.contains(name) {
            return Err(ProvisionError::NotDynamic(name.to_string()));
        }
        let recorded = self
            .volumes
            .read()
            .await
            .get(name)
            .and_then(|h| h.lease_holder.clone());
        self.teardown(&mut inner, name).await;
        self.remove_catalog_row(name).await?;
        self.release_clone_pin(name, from.as_deref()).await;
        // Best-effort: a failed release only makes the next taker wait
        // out the TTL. (The refs object itself outlives deletion — space
        // is GC's concern — so the lease row has somewhere to live.)
        if let Some(lc) = &self.lease {
            let holder = recorded.unwrap_or_else(|| lc.holder.clone());
            if let Err(e) = self.refs.lease_release(vid, &holder).await {
                tracing::warn!(volume = %name, error = %e, "lease release failed");
            }
        }
        Ok(true)
    }

    /// Best-effort release of a deleted clone's pin on its source
    /// snapshot (unpin is idempotent; a failure only delays the source
    /// snapshot's deletability, never correctness).
    async fn release_clone_pin(&self, name: &str, from: Option<&str>) {
        let Some((src_name, snap_name)) = from.and_then(|f| f.split_once('/')) else {
            return;
        };
        let outcome = self
            .refs
            .transact(
                volume_id(src_name),
                vec![RefOp::Unpin {
                    name: format!("snap/{snap_name}"),
                    holder: format!("clone:{name}"),
                }],
            )
            .await;
        if let Err(e) = outcome {
            tracing::warn!(clone = %name, source = %src_name, error = %e,
                "failed to release clone pin");
        }
    }

    /// [`Provisioner::serve`] for a fresh clone: like create, but the
    /// absent-volume branch builds the engine via `Engine::clone_volume`.
    /// A crash-replayed clone (engine exists, catalog row lost) re-opens
    /// through the normal path — the clone-base marker is durable.
    async fn serve_new_clone(
        &self,
        inner: &mut Inner,
        spec: &VolumeSpec,
        src_vid: VolumeId,
        src_commit: skynbd_types::CommitHash,
    ) -> Result<VolumeHandle, ProvisionError> {
        self.serve_inner(inner, spec, true, Some((src_vid, src_commit)), None)
            .await
    }

    /// Engine open-or-create + full serving rig. Caller holds the lock.
    async fn serve(
        &self,
        inner: &mut Inner,
        spec: &VolumeSpec,
        dynamic: bool,
        holder_override: Option<&str>,
    ) -> Result<VolumeHandle, ProvisionError> {
        self.serve_inner(inner, spec, dynamic, None, holder_override)
            .await
    }

    async fn serve_inner(
        &self,
        inner: &mut Inner,
        spec: &VolumeSpec,
        dynamic: bool,
        clone_from: Option<(VolumeId, skynbd_types::CommitHash)>,
        holder_override: Option<&str>,
    ) -> Result<VolumeHandle, ProvisionError> {
        let vid = volume_id(&spec.name);
        // Lease before open (D-M3): the engine below will CAS the head —
        // create included — and must already be the owner when it does.
        // Acquire, not a bare get: a restarted server re-takes its own
        // lease, and a crashed peer's expired lease is stolen here. The
        // acquired epoch is stamped into every commit this engine writes
        // (D-M5), which is what lets recovery reject a fenced
        // predecessor's leftovers.
        // One GET-permit pool per served volume, shared by every engine
        // opened from this config: the head, its snapshot/rpo/origin
        // exports, and the API's snapshot-data and diff readers.
        let mut cfg = self.engine_tuning.clone().share_read_gets();
        let mut lease_holder: Option<String> = None;
        // The owner of record before our acquire, kept for the D-M4
        // gap-recovery scan below (which must run through the ACTING
        // refs handle, built once the granted epoch is known).
        let mut prior_owner: Option<skynbd_refs::Lease> = None;
        if let Some(lc) = &self.lease {
            // Task 15h: a NodeStage's lineage serve acquires under the
            // NODE's identity — the granted epoch is the stage's fence
            // token, carried by every agent transaction.
            let holder = holder_override.unwrap_or(&lc.holder);
            let refs_err = |e: skynbd_refs::RefError| ProvisionError::Catalog(e.to_string());
            // The owner of record *before* our acquire: whose un-exported
            // tail the recovery scan below may adopt (D-M4). Benign
            // race: if ownership moves between this read and the
            // acquire, the acquire comes back Held and nothing here ran.
            let prior = self.refs.lease_get(vid).await.map_err(refs_err)?;
            // The server hands its OWN lease to a requested node holder
            // (release → the acquire below bumps the epoch): the caller
            // is the current owner, so nothing is being revoked. A
            // foreign holder's lease is never touched here — that stays
            // Held until it expires.
            if let (Some(prior), true) = (&prior, holder_override.is_some()) {
                if prior.holder == lc.holder && prior.holder != holder {
                    self.refs
                        .lease_release(vid, &lc.holder)
                        .await
                        .map_err(refs_err)?;
                }
            }
            match self
                .refs
                .lease_acquire(vid, holder, lc.ttl_ms)
                .await
                .map_err(refs_err)?
            {
                LeaseOutcome::Acquired(l) => cfg.epoch = l.epoch,
                LeaseOutcome::Held(l) => {
                    return Err(ProvisionError::Held {
                        volume: spec.name.clone(),
                        holder: l.holder,
                    })
                }
            }
            lease_holder = Some(holder.to_string());
            prior_owner = prior;
        }
        // The ambient-write knot (task 15h): with a node-held lease,
        // every engine write for this volume must be fenced under the
        // node's `(holder, epoch)` — the server's ambient identity would
        // be refused. The acting wrapper makes that true by
        // construction for every write path that goes through this
        // volume's refs handle.
        //
        // Gated on the GRANTED lease, not on the requested holder: on a
        // lease-less deployment the lease block above never ran, no
        // lease row exists, and a fenced transaction would compare
        // against an absent row — refusing EVERY write on this handle
        // with `LeaseLost { holder: "" }`, forever. That was the k3s
        // smoke's restage wedge: the CSI stage always requests a node
        // holder, the first serve happened to reuse the creation-time
        // (unwrapped) handle, and the re-serve after a release built
        // the wrapped one. Without leases there is no Invariant F to
        // enforce — "a single-server deployment without leases works
        // exactly as before" is this match arm.
        let refs: Arc<dyn RefStore> = match (&lease_holder, holder_override) {
            (Some(h), Some(_)) => Arc::new(skynbd_refs::ActingRefStore::new(
                Arc::clone(&self.refs),
                h,
                cfg.epoch,
            )),
            _ => Arc::clone(&self.refs),
        };
        // Roll the head forward over the cadence gap, then export so
        // our successor's gap is single-epoch too. Under per-commit
        // refs the scan finds nothing and the flush is a no-op. Runs
        // through the ACTING handle: under a node-held serve the
        // recovery's head CAS must act as the granted principal, or the
        // ambient fence would silently disable D-M4 gap recovery.
        if self.lease.is_some() {
            if let Some(prior) = prior_owner {
                match skynbd_engine::recover_head_with(
                    &self.store,
                    &refs,
                    vid,
                    prior.epoch,
                    cfg.walk_concurrency,
                )
                .await
                {
                    Ok(Some(h)) => {
                        tracing::info!(volume = %spec.name, head = ?&h[..4],
                            "recovered head from the cadence gap")
                    }
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(volume = %spec.name, error = %e,
                            "cadence-gap recovery failed; serving the exported head")
                    }
                }
            }
        }
        let opened =
            Engine::open(Arc::clone(&self.store), Arc::clone(&refs), vid, cfg.clone()).await;
        // `Corrupt` from open means the superblock is present but the head
        // ref is not — refs lost (§6.5), or owned by a server this one
        // cannot see. Falling through to create would republish a live
        // volume as an empty disk, and a CSI node plugin would `mkfs` it.
        if matches!(&opened, Err(e) if matches!(e, EngineError::Corrupt(_)))
            && skynbd_engine::has_history(&self.store, vid).await?
        {
            return Err(ProvisionError::RefsLost(spec.name.clone()));
        }
        let engine = match opened {
            Ok(e) => {
                tracing::info!(volume = %spec.name, "opened existing volume");
                e
            }
            Err(e) if is_absent(&e) => match clone_from {
                Some((src_vid, src_commit)) => {
                    tracing::info!(volume = %spec.name, size = spec.size, "cloning volume");
                    Engine::clone_volume(
                        Arc::clone(&self.store),
                        Arc::clone(&refs),
                        vid,
                        spec.size,
                        src_vid,
                        src_commit,
                        cfg.clone(),
                    )
                    .await?
                }
                None => {
                    tracing::info!(volume = %spec.name, size = spec.size, "creating volume");
                    Engine::create_volume(
                        Arc::clone(&self.store),
                        Arc::clone(&refs),
                        vid,
                        spec.size,
                        cfg.clone(),
                    )
                    .await?
                }
            },
            Err(e) => return Err(e.into()),
        };
        // Serve-time export (D-M4): now that the engine exists — create
        // path included, whose head CAS is deferred under cadence — make
        // the refs object state current. This is the write half of the
        // single-epoch-gap invariant: after every serve, the only
        // un-exported tail a successor can face is ours.
        if self.lease.is_some() {
            refs.flush_volume(vid)
                .await
                .map_err(|e| ProvisionError::Catalog(e.to_string()))?;
        }
        let size = engine.size();
        let engine: Arc<tokio::sync::RwLock<DynEngine>> =
            Arc::new(tokio::sync::RwLock::new(engine));

        // The origin export, reconciled SYNCHRONOUSLY and fallibly —
        // "serve succeeded" must mean "the exports a client will dial
        // are live" (D-M15 Invariant D). The registry sync task spawned
        // below also reconciles in its initial full sync, but that is
        // asynchronous (a node whose stage attaches `<vol>/origin` the
        // instant this serve returns races it) and warn-only (one
        // transient failure there left an idle lineage export-less
        // until the next generation event — the restage-wedge class the
        // client-side POST /origin papered over). Here a failure fails
        // the serve, so the caller's retry retries the reconcile too.
        // No-op for volumes without a pointer row, so non-lineage
        // serves pay one meta read.
        skynbd_nbd::origin::reconcile(
            &self.registry,
            &VolumeBinding {
                vol_id: vid,
                vol_name: spec.name.clone(),
                store: Arc::clone(&self.store),
                refs: Arc::clone(&refs),
                cfg: cfg.clone(),
            },
        )
        .await
        .map_err(|source| ProvisionError::OriginReconcile {
            volume: spec.name.clone(),
            source,
        })?;

        // Export naming per design D4: `<volume>/head`, `<volume>/snap/*`.
        if spec.kind != "lineage" {
            self.registry
                .register(
                    &format!("{}/head", spec.name),
                    Export {
                        engine: Arc::clone(&engine),
                        read_only: false,
                        size,
                    },
                )
                .await;
        }
        let mut tasks = vec![spawn_registry_sync(
            Arc::clone(&self.registry),
            VolumeBinding {
                vol_id: vid,
                vol_name: spec.name.clone(),
                store: Arc::clone(&self.store),
                refs: Arc::clone(&refs),
                cfg: cfg.clone(),
            },
        )];
        if self.rpo_interval_s > 0 {
            let rpo_engine = Arc::clone(&engine);
            let name = spec.name.clone();
            let interval = self.rpo_interval_s;
            tasks.push(tokio::spawn(async move {
                let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    // Check under the shared lock: a clean volume's tick
                    // never takes the exclusive one (which would stall
                    // reads behind in-flight cold fetches for nothing).
                    // `rpo_advance` re-checks under `write()`.
                    if let Ok(true) = rpo_engine.read().await.rpo_is_current().await {
                        continue;
                    }
                    match rpo_engine.write().await.rpo_advance().await {
                        Ok(Some(_)) => tracing::debug!(volume = %name, "rpo advanced"),
                        Ok(None) => {}
                        Err(e) => {
                            tracing::warn!(volume = %name, error = %e, "rpo advance failed")
                        }
                    }
                }
            }));
        }

        let handle = VolumeHandle {
            vol_id: vid,
            size,
            engine,
            store: Arc::clone(&self.store),
            refs,
            cfg,
            lease_holder,
            prepared: Arc::new(tokio::sync::Mutex::new(Default::default())),
        };
        self.volumes
            .write()
            .await
            .insert(spec.name.clone(), handle.clone());
        inner.serving.insert(spec.name.clone(), Serving { tasks });
        if dynamic {
            inner.dynamic.insert(spec.name.clone());
        }
        Ok(handle)
    }

    /// The D-M6 handoff, server side: commit what is ours, stop serving,
    /// release the lease — after which another server's `serve` acquires
    /// at the next epoch instead of waiting out the TTL. Returns whether
    /// the volume was being served. Idempotent, and callable for any
    /// served volume (config-defined included: two hosts sharing a
    /// bucket via `[[volumes]]` is exactly the scenario D-M3 exists
    /// for). The catalog row stays — release is not delete.
    pub async fn release(&self, name: &str) -> Result<bool, ProvisionError> {
        let mut inner = self.inner.lock().await;
        let was_serving = inner.serving.contains_key(name);
        // The holder the serve recorded (task 15h): a node-held lineage
        // lease releases under the node's identity. Captured before the
        // teardown drops the handle. When nothing is serving (e.g. after
        // a server restart) the server's own holder is the only guess —
        // and `lease_release` no-ops on a mismatch, leaving the TTL as
        // the backstop, exactly the D-M6 crash shape.
        let recorded = self
            .volumes
            .read()
            .await
            .get(name)
            .and_then(|h| h.lease_holder.clone());
        if was_serving {
            // Commit-before-release (D-M6): the NBD client flushed at
            // unmount, so this is usually a no-op — but any acked write
            // still in the overlay must land under *our* epoch, because
            // after the release it can never land at all.
            let engine = self
                .volumes
                .read()
                .await
                .get(name)
                .map(|h| Arc::clone(&h.engine));
            if let Some(engine) = engine {
                let mut eng = engine.write().await;
                if !eng.is_clean() {
                    eng.commit(skynbd_format::CommitKind::Checkpoint).await?;
                }
                // Clean unstage writes the emap (design D-M7): whoever
                // serves this volume next opens it in one GET instead
                // of a tree walk. Best-effort — the handoff is correct
                // without it, just slower.
                if let Err(e) = eng.write_emap().await {
                    tracing::warn!(volume = %name, error = %e, "emap write failed at release");
                }
            }
            self.teardown(&mut inner, name).await;
        }
        if let Some(lc) = &self.lease {
            let holder = recorded.unwrap_or_else(|| lc.holder.clone());
            self.refs
                .lease_release(volume_id(name), &holder)
                .await
                .map_err(|e| ProvisionError::Catalog(e.to_string()))?;
        }
        Ok(was_serving)
    }

    /// The D-M6 handoff, receiving side: serve a volume this server did
    /// not create, from its row in the shared catalog (visible to every
    /// server in "object" refs mode). Idempotent when already serving.
    /// The serve path acquires the lease, so a volume still owned
    /// elsewhere is a clean `Held`, not a race.
    pub async fn serve_cataloged(&self, name: &str) -> Result<VolumeHandle, ProvisionError> {
        self.serve_cataloged_as(name, None).await
    }

    /// [`Self::serve_cataloged`] with an optional lease-holder override
    /// (task 15h): a NodeStage serves its lineage under the NODE's
    /// identity, and the granted epoch on the returned handle is the
    /// stage's fence token. When the volume is already being served
    /// under a *different* holder, the reschedule rules apply: the
    /// server hands over its own lease freely (it is the caller), but a
    /// foreign node's live lease refuses with `Held` — only an expired
    /// lease can be stolen, which is what keeps two live writers off one
    /// chain.
    pub async fn serve_cataloged_as(
        &self,
        name: &str,
        holder: Option<&str>,
    ) -> Result<VolumeHandle, ProvisionError> {
        validate_name(name)?;
        let mut inner = self.inner.lock().await;
        if inner.serving.contains_key(name) {
            let existing = self.volumes.read().await[name].clone();
            let same = match (holder, &self.lease) {
                // No override, or no leases at all: the replayed serve.
                (None, _) | (_, None) => true,
                (Some(h), Some(_)) => existing.lease_holder.as_deref() == Some(h),
            };
            if same {
                // The replayed serve re-converges the origin export too
                // (same guarantee as a fresh serve): a kubelet-retried
                // stage lands here, and before this the retry was
                // exactly the path that could NOT heal a missing export
                // — the first serve's async reconcile had failed once,
                // and the replay returned without looking.
                skynbd_nbd::origin::reconcile(
                    &self.registry,
                    &VolumeBinding {
                        vol_id: volume_id(name),
                        vol_name: name.to_string(),
                        store: Arc::clone(&existing.store),
                        refs: Arc::clone(&existing.refs),
                        cfg: existing.cfg.clone(),
                    },
                )
                .await
                .map_err(|source| ProvisionError::OriginReconcile {
                    volume: name.to_string(),
                    source,
                })?;
                return Ok(existing);
            }
            // A holder change through the same server: probe whether the
            // new holder can take the lease BEFORE tearing down the live
            // serving rig — a foreign unexpired lease must disturb
            // nothing.
            let (new_holder, lc) = (holder.expect("checked above"), self.lease.as_ref().unwrap());
            let vid = volume_id(name);
            let refs_err = |e: skynbd_refs::RefError| ProvisionError::Catalog(e.to_string());
            let row = self.refs.lease_get(vid).await.map_err(refs_err)?;
            match row {
                // Our own lease: hand it over (serve_inner releases and
                // re-acquires under the new holder, bumping the epoch).
                Some(l) if l.holder == lc.holder || l.holder == new_holder => {}
                _ => match self
                    .refs
                    .lease_acquire(vid, new_holder, lc.ttl_ms)
                    .await
                    .map_err(refs_err)?
                {
                    // Steal of an expired foreign lease succeeded; the
                    // serve below renews under the same holder/epoch.
                    LeaseOutcome::Acquired(_) => {}
                    LeaseOutcome::Held(l) => {
                        return Err(ProvisionError::Held {
                            volume: name.to_string(),
                            holder: l.holder,
                        })
                    }
                },
            }
            self.teardown(&mut inner, name).await;
        }
        let row = self
            .refs
            .meta_get(catalog_volume_id(), &catalog_key(name))
            .await
            .map_err(|e| ProvisionError::Catalog(e.to_string()))?
            .ok_or_else(|| ProvisionError::NotCataloged(name.to_string()))?;
        let spec: VolumeSpec = serde_json::from_slice(&row)
            .map_err(|e| ProvisionError::Catalog(format!("unreadable catalog row: {e}")))?;
        self.serve(&mut inner, &spec, true, holder).await
    }

    /// Stop serving: abort tasks, drop exports, remove the handle.
    /// Already-attached NBD clients keep their connection (CSI guarantees
    /// unpublish-before-delete); new attaches see an unknown export.
    async fn teardown(&self, inner: &mut Inner, name: &str) {
        if let Some(s) = inner.serving.remove(name) {
            for t in s.tasks {
                t.abort();
            }
        }
        inner.dynamic.remove(name);
        self.volumes.write().await.remove(name);
        let prefix = format!("{name}/");
        for export in self.registry.names().await {
            if export.starts_with(&prefix) {
                self.registry.unregister(&export).await;
            }
        }
    }

    async fn remove_catalog_row(&self, name: &str) -> Result<(), ProvisionError> {
        self.refs
            .transact(
                catalog_volume_id(),
                vec![RefOp::MetaDelete {
                    key: catalog_key(name),
                }],
            )
            .await
            .map(|_| ())
            .map_err(|e| ProvisionError::Catalog(e.to_string()))?;
        // Best-effort: a surviving marker only means a future catalog
        // rebuild restores a row for a deleted volume — annoying, not
        // dangerous (its serve finds no head and refuses, §6.5).
        if let Err(e) = self
            .store
            .delete(&skynbd_engine::keys::vol_index(name))
            .await
        {
            tracing::warn!(volume = %name, error = %e, "name-index marker delete failed");
        }
        Ok(())
    }

    /// Rebuild the catalog from the D-M9 name-index markers: restore the
    /// row of every indexed volume the catalog does not know. Returns the
    /// restored names. Pair with `rebuild_refs` per volume for full
    /// disaster recovery (refs store lost, bucket intact).
    pub async fn rebuild_catalog(&self) -> Result<Vec<String>, ProvisionError> {
        rebuild_catalog(&self.store, &self.refs).await
    }

    /// Checkpoint every served volume (graceful shutdown).
    pub async fn checkpoint_all(&self) {
        let volumes: Vec<_> = self
            .volumes
            .read()
            .await
            .iter()
            .map(|(n, h)| (n.clone(), Arc::clone(&h.engine)))
            .collect();
        for (name, engine) in volumes {
            if let Err(e) = engine
                .write()
                .await
                .commit(skynbd_format::CommitKind::Checkpoint)
                .await
            {
                tracing::warn!(volume = %name, error = %e, "shutdown checkpoint failed");
            }
        }
    }
}

fn is_absent(e: &EngineError) -> bool {
    matches!(
        e,
        EngineError::Store(StoreError::NotFound(_)) | EngineError::Corrupt(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_validation() {
        for ok in ["v", "pvc-2f9c", "db_1.x", "a".repeat(63).as_str()] {
            assert!(validate_name(ok).is_ok(), "{ok:?}");
        }
        for bad in [
            "",
            "-lead",
            ".lead",
            "has/slash",
            "has space",
            "a".repeat(64).as_str(),
        ] {
            assert!(validate_name(bad).is_err(), "{bad:?}");
        }
        // The reserved catalog name can never validate.
        assert!(validate_name(CATALOG_NAME).is_err());
    }

    #[test]
    fn ids_are_stable_and_catalog_is_disjoint() {
        assert_eq!(volume_id("vol1"), volume_id("vol1"));
        assert_ne!(volume_id("vol1"), volume_id("vol2"));
        assert_ne!(catalog_volume_id(), volume_id("skynbd-catalog"));
    }
}

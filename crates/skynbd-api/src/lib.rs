//! REST control plane (design §12.1).
//!
//! M3 surface: health, volumes, exports, snapshot create/list/delete,
//! rollback. The V3.2 ingestion endpoints (`prepared/*`, `objects/*`,
//! pins — design §9A) land in M6; diff/import (§12.1) in M5.
//!
//! Error mapping: unknown volume/snapshot → 404, duplicate snapshot or
//! dirty rollback → 409, bad names → 400, engine/store failures → 500.

use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use skynbd_engine::prepared::{PreparedSession, RangeOp};
use skynbd_engine::{EngineConfig, EngineError};
use skynbd_nbd::{DynEngine, ExportRegistry, VolumeBinding};
use skynbd_refs::RefStore;
use skynbd_store::ObjectStore;
use skynbd_types::SnapId;
use skynbd_types::VolumeId;
use tokio::sync::{Mutex, RwLock};

/// One served volume: its writable engine, shared with the NBD frontend.
/// Type-erased prepared session (design §9A).
pub type DynPrepared = PreparedSession<Arc<dyn ObjectStore>, Arc<dyn RefStore>>;

#[derive(Clone)]
pub struct VolumeHandle {
    pub vol_id: VolumeId,
    pub size: u64,
    pub engine: Arc<RwLock<DynEngine>>,
    /// Direct store/refs access for maintenance ops (gc report is via the
    /// engine; fsck walks read-only).
    pub store: Arc<dyn ObjectStore>,
    pub refs: Arc<dyn RefStore>,
    /// The config the volume was served under (lease epoch included):
    /// what read-only side engines — the origin export's, D-M15 — are
    /// opened with.
    pub cfg: EngineConfig,
    /// The identity the volume's lease was granted to at serve time
    /// (task 15h): the server's own node id, or a NodeStage's holder
    /// override for lineage serves. Release and renewal act under this
    /// holder; `None` = served without leases.
    pub lease_holder: Option<String>,
    /// Active prepared sessions (lazily reopened after restart, §9A.6).
    pub prepared: Arc<Mutex<std::collections::HashMap<String, Arc<Mutex<DynPrepared>>>>>,
}

pub mod auth;
pub mod enroll;
pub mod provision;
pub mod retention;
pub use auth::{ApiAuth, AuthzCtx, Binding, OidcAuth, TrustedIssuer};
pub use enroll::CertIssuer;
pub use provision::{ProvisionError, Provisioner, VolumeSpec};

pub struct ApiState {
    /// Live volume table. Shared with the [`Provisioner`] (when one is
    /// configured), which inserts/removes entries at runtime.
    pub volumes: Arc<tokio::sync::RwLock<HashMap<String, VolumeHandle>>>,
    pub registry: Arc<ExportRegistry>,
    /// Authentication mode (auth design §4): disabled (dev/tests), a
    /// static bearer token (break-glass), or OIDC multi-issuer with
    /// per-identity bindings. `/healthz` stays open for probes.
    pub auth: ApiAuth,
    /// mTLS client-cert enrollment (auth design §3.2 amendment); `None`
    /// disables `POST /v1/certs/enroll`.
    pub enroll: Option<CertIssuer>,
    /// Dynamic volume lifecycle (design §12.1); `None` makes
    /// POST/DELETE on volumes 404 (static, config-defined serving only).
    pub provisioner: Option<Arc<Provisioner>>,
    /// Pending on-demand snapshot requests per volume (CSI 6h-2): the
    /// CSI controller POSTs a name, the volume's embedded agent polls,
    /// runs a cycle, aliases the published generation to the requested
    /// name, and DELETEs the request. Each request carries the cycle's
    /// last reported status (multi-node design D-M12) so a stalled or
    /// failing cycle is observable, not a silent forever-pending.
    /// Deliberately in-memory: the requester retries until the named
    /// snapshot exists, so a lost request re-creates itself after a
    /// server restart (status restarts at "pending" with it), and the
    /// agent — which holds each request's capture binding durably —
    /// re-announces the capture time on its next poll.
    pub snap_requests: SnapRequests,
}

/// See [`ApiState::snap_requests`].
pub type SnapRequests =
    Arc<tokio::sync::Mutex<HashMap<String, std::collections::BTreeMap<String, SnapRequestStatus>>>>;

/// Most completed request statuses retained per volume for CSI replays.
/// Completion is the agent's last step and the CSI controller's next
/// poll reads the status once, so only the most recent few matter; the
/// cap bounds the map for long-lived servers with many snapshots.
pub const COMPLETED_SNAP_REQUESTS_RETAINED: usize = 256;

/// Cycle status projected onto a snapshot request (D-M12): the agent
/// reports volume-level cycle progress (`POST cycle-status`) and failures,
/// which the server stamps onto every pending request of that volume, and
/// per-request capture bindings, which it stamps onto exactly the named
/// requests. `GET snapshot-requests/{name}` answers "has my cut happened,
/// and if not, why not".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapRequestStatus {
    /// Agent cycle phase (`capturing`, `backfilling`, `visible`, …);
    /// `pending` until the agent's first report reaches the server.
    pub phase: String,
    /// Backfill op progress. `ops_total == 0` means "not counted yet"
    /// (the delta is only known once the backfill phase computes it).
    pub ops_done: u64,
    pub ops_total: u64,
    /// Consecutive failed cycle attempts since the last success.
    pub attempts: u32,
    /// Most recent cycle error reported for this request.
    pub last_error: Option<String>,
    /// Unix ms of the local thin-snapshot cut this request is bound to.
    /// First binding wins: a request's point in time never moves.
    pub capture_time_ms: Option<u64>,
    /// The agent has aliased and published this request. Completed
    /// statuses stay readable so CSI replays keep their capture time.
    pub completed: bool,
    /// The cut this request was bound to was discarded before it was
    /// published (or the agent tried to bind it to a different cut). It
    /// will never be served; the filer must delete and re-file.
    pub capture_lost: bool,
    /// Completion order, for [`COMPLETED_SNAP_REQUESTS_RETAINED`].
    #[serde(skip)]
    completed_seq: u64,
}

impl Default for SnapRequestStatus {
    fn default() -> Self {
        Self {
            phase: "pending".into(),
            ops_done: 0,
            ops_total: 0,
            attempts: 0,
            last_error: None,
            capture_time_ms: None,
            completed: false,
            capture_lost: false,
            completed_seq: 0,
        }
    }
}

/// Wrap a plain volume map for [`ApiState::volumes`] (fixtures/tests).
pub fn static_volumes(
    map: HashMap<String, VolumeHandle>,
) -> Arc<tokio::sync::RwLock<HashMap<String, VolumeHandle>>> {
    Arc::new(tokio::sync::RwLock::new(map))
}

pub fn router(state: Arc<ApiState>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok" }))
        .route("/v1/volumes", get(list_volumes).post(create_volume))
        .route("/v1/volumes/{vol}", axum::routing::delete(delete_volume))
        .route("/v1/volumes/{vol}/serve", post(serve_volume))
        .route("/v1/volumes/{vol}/release", post(release_volume))
        .route("/v1/catalog/rebuild", post(rebuild_catalog))
        .route("/v1/exports", get(list_exports))
        .route(
            "/v1/volumes/{vol}/snapshots",
            get(list_snapshots).post(create_snapshot),
        )
        .route(
            "/v1/volumes/{vol}/snapshots/{name}",
            axum::routing::delete(delete_snapshot),
        )
        .route(
            "/v1/volumes/{vol}/snapshots/{name}/alias",
            post(alias_snapshot),
        )
        .route(
            "/v1/volumes/{vol}/snapshot-requests",
            get(list_snapshot_requests).post(create_snapshot_request),
        )
        .route(
            "/v1/volumes/{vol}/snapshot-requests/{name}",
            get(get_snapshot_request).delete(delete_snapshot_request),
        )
        .route("/v1/volumes/{vol}/cycle-status", post(report_cycle_status))
        .route("/v1/volumes/{vol}/origin", get(get_origin).post(set_origin))
        .route("/v1/volumes/{vol}/rollback", post(rollback))
        .route(
            "/v1/volumes/{vol}/prepared",
            get(list_prepared).post(create_prepared),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}",
            get(prepared_status).delete(abort_prepared),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}/attach",
            post(prepared_attach),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}/clear",
            post(prepared_clear),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}/apply",
            post(prepared_apply),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}/ensure",
            post(prepared_ensure),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}/publish",
            post(prepared_publish),
        )
        .route(
            "/v1/volumes/{vol}/prepared/{id}/checkpoint",
            post(prepared_checkpoint),
        )
        .route(
            "/v1/volumes/{vol}/objects/{hash}",
            axum::routing::put(put_object),
        )
        .route(
            "/v1/volumes/{vol}/snapshots/{name}/pins",
            get(list_pins).post(pin_snapshot),
        )
        .route(
            "/v1/volumes/{vol}/snapshots/{name}/data",
            get(read_snapshot_data),
        )
        .route(
            "/v1/volumes/{vol}/snapshots/{name}/pins/{holder}",
            axum::routing::delete(unpin_snapshot),
        )
        .route("/v1/volumes/{vol}/gc", post(run_gc))
        .route("/v1/volumes/{vol}/thin", post(thin_volume))
        .route("/v1/volumes/{vol}/fsck", post(run_fsck))
        .route("/v1/volumes/{vol}/diff", get(diff))
        .route("/v1/volumes/{vol}/import", post(import))
        .route("/v1/volumes/{vol}/rpo", post(rpo_advance))
        .route("/v1/certs/enroll", post(enroll_cert))
        .layer(DefaultBodyLimit::max(1 << 30)) // streams can be large
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            require_bearer,
        ))
        // Outermost: every response, auth refusals and routing misses
        // included, says which optional calls this server has.
        .layer(axum::middleware::map_response(advertise_features))
        .with_state(state)
}

/// Response header naming the optional API features this server
/// supports, comma-separated. A client decides from it whether to use a
/// newer call, instead of inferring support from how an older server
/// fails (a status a proxy may rewrite, a body it may replace).
pub const FEATURES_HEADER: &str = "x-skynbd-features";

/// The features this server advertises. `prepared-apply`: the mixed
/// attach/clear batch, `POST …/prepared/{id}/apply`.
pub const FEATURES: &str = "prepared-apply";

async fn advertise_features(mut resp: Response) -> Response {
    resp.headers_mut().insert(
        FEATURES_HEADER,
        axum::http::HeaderValue::from_static(FEATURES),
    );
    resp
}

// ------------------------------------------------------------------ types

#[derive(Serialize)]
struct VolumeInfo {
    name: String,
    size: u64,
    flush_seq: u64,
    /// The granted lease epoch (task 15h) — the fence token every agent
    /// transaction for this volume must carry. Absent without leases.
    #[serde(skip_serializing_if = "Option::is_none")]
    epoch: Option<u64>,
    /// The lease's holder identity (server default, or the serve
    /// request's override).
    #[serde(skip_serializing_if = "Option::is_none")]
    holder: Option<String>,
}

#[derive(Deserialize)]
struct CreateSnapshotBody {
    name: String,
}

#[derive(Serialize)]
struct SnapshotInfo {
    name: String,
    commit: String,
}

#[derive(Deserialize)]
struct RollbackBody {
    snapshot: String,
}

pub struct ApiError(StatusCode, serde_json::Value);

impl ApiError {
    /// The HTTP status this error answers with — what out-of-band
    /// callers (the daemon's retention cadence) branch on: a 409 on a
    /// node-held volume is "not mine to thin", not a failure.
    pub fn status(&self) -> StatusCode {
        self.0
    }
}

impl ApiError {
    fn msg(code: StatusCode, m: impl std::fmt::Display) -> Self {
        Self(code, serde_json::json!({ "error": m.to_string() }))
    }

    /// The Invariant F refusal (task 15h): a 409 whose body is
    /// distinguishable from every other conflict —
    /// `{"error":"lease_lost","holder":…,"epoch":…}` — so the agent can
    /// map exactly this to its terminal `Fenced` error while ordinary
    /// 409s stay retryable.
    fn lease_lost(holder: &str, epoch: u64) -> Self {
        Self(
            StatusCode::CONFLICT,
            serde_json::json!({
                "error": "lease_lost",
                "holder": holder,
                "epoch": epoch,
            }),
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
}

impl From<EngineError> for ApiError {
    fn from(e: EngineError) -> Self {
        // Invariant F: the epoch-specific refusal keeps its structured
        // body so the agent can tell it from every other conflict.
        if let EngineError::Fenced { holder, epoch } = &e {
            return ApiError::lease_lost(holder, *epoch);
        }
        let code = match &e {
            EngineError::SnapshotExists(_) | EngineError::DirtyRollback | EngineError::DirtyGc => {
                StatusCode::CONFLICT
            }
            // The owner-less lease-lost / lost-CAS self-fence: still a
            // conflict (retryable against the new owner), never a 500.
            EngineError::Conflict => StatusCode::CONFLICT,
            EngineError::SnapshotNotFound(_) | EngineError::PreparedNotFound(_) => {
                StatusCode::NOT_FOUND
            }
            EngineError::Pinned(_) => StatusCode::CONFLICT,
            EngineError::InvalidIo(_) => StatusCode::BAD_REQUEST,
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError::msg(code, e.to_string())
    }
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

/// Owned handle lookup (the map is shared with the provisioner, so no
/// reference can outlive the read guard; handles are a bundle of Arcs).
async fn volume(state: &ApiState, name: &str) -> std::result::Result<VolumeHandle, ApiError> {
    if let Some(h) = state.volumes.read().await.get(name).cloned() {
        return Ok(h);
    }
    // Released volumes stay in the catalog (D-M6: release is not
    // delete), and an API operation is as good a reason to pick one
    // back up as a NodeStage — snapshots of an unstaged volume are the
    // canonical case. `serve_cataloged` acquires the lease on the way
    // in, so a volume owned by another server is a clean 409 here, not
    // a phantom 404.
    if let Some(prov) = &state.provisioner {
        match prov.serve_cataloged(name).await {
            Ok(h) => return Ok(h),
            // Not cataloged, or a name that could never be a volume:
            // the same "unknown volume" it always was.
            Err(provision::ProvisionError::NotCataloged(_))
            | Err(provision::ProvisionError::BadSpec(_)) => {}
            Err(e) => return Err(provision_error(e)),
        }
    }
    Err(ApiError::msg(
        StatusCode::NOT_FOUND,
        format!("unknown volume {name:?}"),
    ))
}

// --------------------------------------------------------------- handlers

async fn list_volumes(State(state): State<Arc<ApiState>>) -> Json<Vec<VolumeInfo>> {
    let snapshot: Vec<(String, VolumeHandle)> = state
        .volumes
        .read()
        .await
        .iter()
        .map(|(n, h)| (n.clone(), h.clone()))
        .collect();
    let mut out = Vec::new();
    for (name, h) in snapshot {
        let flush_seq = h.engine.read().await.flush_seq();
        out.push(VolumeInfo {
            name,
            size: h.size,
            flush_seq,
            epoch: (h.cfg.epoch != 0).then_some(h.cfg.epoch),
            holder: h.lease_holder.clone(),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Json(out)
}

/// POST /v1/volumes (design §12.1): dynamic provisioning — the CSI
/// controller's CreateVolume. Idempotent: an identical replay returns
/// 200 with the existing volume; a same-name different-size request is
/// 409. With `from` set the volume is a fast clone (design §4.1), which
/// additionally requires the `clone` op on the *source* volume — cloning
/// exfiltrates its full content, so `volume` alone must not grant it.
async fn create_volume(
    State(state): State<Arc<ApiState>>,
    axum::Extension(ctx): axum::Extension<AuthzCtx>,
    Json(spec): Json<VolumeSpec>,
) -> std::result::Result<(StatusCode, Json<VolumeInfo>), ApiError> {
    let Some(prov) = &state.provisioner else {
        return Err(ApiError::msg(
            StatusCode::NOT_FOUND,
            "dynamic volume provisioning is not configured",
        ));
    };
    if let Some(from) = &spec.from {
        // Body-carried source volume: route_perm cannot see it, so the
        // clone grant is enforced here.
        let src = from.split_once('/').map(|(v, _)| v).unwrap_or(from);
        if ctx.is_restricted() && !ctx.allows("clone", Some(src)) {
            return Err(ApiError::msg(
                StatusCode::FORBIDDEN,
                format!("cloning from volume {src:?} requires the \"clone\" op"),
            ));
        }
    }
    let (code, handle) = match prov.create(&spec).await {
        Ok(provision::Created::New(h)) => (StatusCode::CREATED, h),
        Ok(provision::Created::Existing(h)) => (StatusCode::OK, h),
        Err(e) => return Err(provision_error(e)),
    };
    tracing::info!(volume = %spec.name, size = handle.size, "volume provisioned");
    let flush_seq = handle.engine.read().await.flush_seq();
    Ok((
        code,
        Json(VolumeInfo {
            name: spec.name,
            size: handle.size,
            flush_seq,
            epoch: (handle.cfg.epoch != 0).then_some(handle.cfg.epoch),
            holder: handle.lease_holder.clone(),
        }),
    ))
}

/// DELETE /v1/volumes/{vol}: the CSI controller's DeleteVolume.
/// Idempotent (absent → 204); config-defined volumes are 409.
async fn delete_volume(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    let Some(prov) = &state.provisioner else {
        return Err(ApiError::msg(
            StatusCode::NOT_FOUND,
            "dynamic volume provisioning is not configured",
        ));
    };
    match prov.delete(&vol).await {
        Ok(deleted) => {
            if deleted {
                tracing::info!(volume = %vol, "volume deleted");
            }
            state.snap_requests.lock().await.remove(&vol);
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => Err(provision_error(e)),
    }
}

/// The D-M6 handoff endpoints. `release` is what `NodeUnstage` calls on
/// its way out: commit, stop serving, release the lease. `serve` is the
/// receiving side at the next `NodeStage`: pick the volume up from the
/// shared catalog (acquiring its lease) so the node's local server has
/// the export before `nbd-client` dials it.
async fn release_volume(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<StatusCode, ApiError> {
    let Some(prov) = &state.provisioner else {
        return Err(ApiError::msg(
            StatusCode::NOT_FOUND,
            "dynamic volume provisioning is not configured",
        ));
    };
    match prov.release(&vol).await {
        Ok(was_serving) => {
            if was_serving {
                tracing::info!(volume = %vol, "volume released");
            }
            Ok(StatusCode::NO_CONTENT)
        }
        Err(e) => Err(provision_error(e)),
    }
}

/// D-M9 disaster recovery, online: restore catalog rows from the
/// name-index markers. Rows the catalog already has are untouched;
/// restored volumes are servable immediately (serve/stage pulls them
/// from the catalog, `rebuild_refs` recovers their heads if needed).
async fn rebuild_catalog(
    State(state): State<Arc<ApiState>>,
) -> std::result::Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let Some(prov) = &state.provisioner else {
        return Err(ApiError::msg(
            StatusCode::NOT_FOUND,
            "dynamic volume provisioning is not configured",
        ));
    };
    match prov.rebuild_catalog().await {
        Ok(restored) => {
            if !restored.is_empty() {
                tracing::info!(
                    count = restored.len(),
                    "catalog rows restored from name index"
                );
            }
            Ok((
                StatusCode::OK,
                Json(serde_json::json!({ "restored": restored })),
            ))
        }
        Err(e) => Err(provision_error(e)),
    }
}

/// Optional serve body (task 15h): `{"holder": "<node-id>"}` acquires
/// the volume's lease under the NODE's identity instead of the server's
/// — the granted epoch (echoed in the response) becomes the fence token
/// for every agent transaction of that stage. Absent = back-compat
/// server-default holder.
#[derive(Deserialize, Default)]
struct ServeBody {
    holder: Option<String>,
}

async fn serve_volume(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    body: Option<Json<ServeBody>>,
) -> std::result::Result<(StatusCode, Json<VolumeInfo>), ApiError> {
    let Some(prov) = &state.provisioner else {
        return Err(ApiError::msg(
            StatusCode::NOT_FOUND,
            "dynamic volume provisioning is not configured",
        ));
    };
    let holder = body.and_then(|Json(b)| b.holder);
    match prov.serve_cataloged_as(&vol, holder.as_deref()).await {
        Ok(handle) => {
            tracing::info!(volume = %vol, holder = ?handle.lease_holder, "volume served");
            let flush_seq = handle.engine.read().await.flush_seq();
            Ok((
                StatusCode::OK,
                Json(VolumeInfo {
                    name: vol,
                    size: handle.size,
                    flush_seq,
                    epoch: (handle.cfg.epoch != 0).then_some(handle.cfg.epoch),
                    holder: handle.lease_holder.clone(),
                }),
            ))
        }
        Err(e) => Err(provision_error(e)),
    }
}

fn provision_error(e: provision::ProvisionError) -> ApiError {
    use provision::ProvisionError as P;
    let code = match &e {
        P::BadSpec(_) => StatusCode::BAD_REQUEST,
        P::Mismatch { .. }
        | P::NotDynamic(_)
        | P::HasClones { .. }
        | P::RefsLost(_)
        | P::Held { .. } => StatusCode::CONFLICT,
        P::SourceMissing(_) | P::NotCataloged(_) => StatusCode::NOT_FOUND,
        // e.g. a clone size below the source's.
        P::Engine(EngineError::InvalidIo(_)) => StatusCode::BAD_REQUEST,
        P::Engine(_) | P::Catalog(_) | P::OriginReconcile { .. } => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    ApiError::msg(code, e.to_string())
}

async fn list_exports(State(state): State<Arc<ApiState>>) -> Json<Vec<String>> {
    Json(state.registry.names().await)
}

async fn create_snapshot(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    Json(body): Json<CreateSnapshotBody>,
) -> std::result::Result<(StatusCode, Json<SnapshotInfo>), ApiError> {
    let h = volume(&state, &vol).await?;
    let (_, commit) = h.engine.write().await.snapshot_create(&body.name).await?;
    Ok((
        StatusCode::CREATED,
        Json(SnapshotInfo {
            name: body.name,
            commit: hex(&commit),
        }),
    ))
}

async fn list_snapshots(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<Vec<SnapshotInfo>>, ApiError> {
    let h = volume(&state, &vol).await?;
    let snaps = h.engine.read().await.snapshot_list().await?;
    Ok(Json(
        snaps
            .into_iter()
            .map(|(name, commit)| SnapshotInfo {
                name,
                commit: hex(&commit),
            })
            .collect(),
    ))
}

async fn delete_snapshot(
    State(state): State<Arc<ApiState>>,
    Path((vol, name)): Path<(String, String)>,
) -> std::result::Result<StatusCode, ApiError> {
    let h = volume(&state, &vol).await?;
    let deleted = h
        .engine
        .write()
        .await
        .snapshot_delete(&name)
        .await
        .map_err(ApiError::from);
    // The snapshot is gone (now or already): so is any request for it.
    // That includes one whose cut was lost and never aliased — deleting
    // its VolumeSnapshot must still release the request.
    if deleted
        .as_ref()
        .map_or_else(|e| e.status() == StatusCode::NOT_FOUND, |_| true)
    {
        if let Some(requests) = state.snap_requests.lock().await.get_mut(&vol) {
            requests.remove(&name);
        }
    }
    deleted?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct AliasBody {
    name: String,
    /// Stage-time lease epoch (Invariant F, task 15h). Validated as a
    /// precondition here — the alias transaction itself runs through the
    /// volume's engine, which under a node-held serve is already fenced
    /// by construction (the serve-time `ActingRefStore`).
    #[serde(default)]
    epoch: Option<u64>,
}

/// POST /v1/volumes/{vol}/snapshots/{name}/alias {"name"} — second ref
/// to the same commit (CSI 6h-2: the agent gives a published generation
/// the CSI-requested snapshot name). Idempotent; 409 if the alias name
/// exists pointing elsewhere.
async fn alias_snapshot(
    State(state): State<Arc<ApiState>>,
    Path((vol, from)): Path<(String, String)>,
    Json(body): Json<AliasBody>,
) -> std::result::Result<Json<SnapshotInfo>, ApiError> {
    let h = volume(&state, &vol).await?;
    check_epoch(&state, &h, body.epoch).await?;
    let commit = h
        .engine
        .write()
        .await
        .snapshot_alias(&from, &body.name)
        .await?;
    Ok(Json(SnapshotInfo {
        name: body.name,
        commit: hex(&commit),
    }))
}

#[derive(Deserialize)]
struct SnapshotRequestBody {
    name: String,
}

/// POST /v1/volumes/{vol}/snapshot-requests {"name"} — ask the volume's
/// embedded agent for an on-demand fs-consistent snapshot under `name`
/// (CSI 6h-2). Idempotent; readiness is "the named snapshot exists",
/// not request state. A replay keeps the accumulated status (D-M12) —
/// the external-snapshotter re-files while a cycle is mid-flight.
async fn create_snapshot_request(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    Json(body): Json<SnapshotRequestBody>,
) -> std::result::Result<StatusCode, ApiError> {
    let h = volume(&state, &vol).await?; // 404 for unknown volumes
    let completed = {
        let mut requests = state.snap_requests.lock().await;
        let status = requests
            .entry(vol.clone())
            .or_default()
            .entry(body.name.clone())
            .or_default();
        status.completed
    };
    // Re-filing is idempotent — the CSI controller re-POSTs on every
    // not-ready poll, and a completed status must survive a re-POST that
    // raced the agent's alias (it keeps the capture time replays need).
    // Only a completed request whose snapshot has since been deleted is a
    // genuinely new request.
    if completed {
        let exists = h
            .engine
            .read()
            .await
            .snapshot_list()
            .await?
            .iter()
            .any(|(name, _)| *name == body.name);
        if !exists {
            if let Some(status) = state
                .snap_requests
                .lock()
                .await
                .get_mut(&vol)
                .and_then(|requests| requests.get_mut(&body.name))
                .filter(|status| status.completed)
            {
                *status = SnapRequestStatus::default();
            }
        }
    }
    Ok(StatusCode::ACCEPTED)
}

/// GET /v1/volumes/{vol}/snapshot-requests — pending names (agent poll).
async fn list_snapshot_requests(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<Vec<String>>, ApiError> {
    volume(&state, &vol).await?;
    Ok(Json(
        state
            .snap_requests
            .lock()
            .await
            .get(&vol)
            .map(|s| {
                s.iter()
                    .filter(|(_, status)| !status.completed && !status.capture_lost)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default(),
    ))
}

/// GET /v1/volumes/{vol}/snapshot-requests/{name} — the request's cycle
/// status (D-M12). Completed and lost requests remain readable — replays
/// keep the original capture time, and a lost cut stays an error — while
/// the agent's list endpoint excludes both.
async fn get_snapshot_request(
    State(state): State<Arc<ApiState>>,
    Path((vol, name)): Path<(String, String)>,
) -> std::result::Result<Json<SnapRequestStatus>, ApiError> {
    volume(&state, &vol).await?;
    state
        .snap_requests
        .lock()
        .await
        .get(&vol)
        .and_then(|m| m.get(&name).cloned())
        .map(Json)
        .ok_or_else(|| {
            ApiError::msg(
                StatusCode::NOT_FOUND,
                format!("no pending snapshot request {name:?} on {vol:?}"),
            )
        })
}

/// Every field optional: a progress report carries phase/ops, a failure
/// report carries attempts/last_error, and absent fields keep their
/// previous value — so the two report kinds don't clobber each other.
#[derive(Deserialize)]
struct CycleStatusBody {
    phase: Option<String>,
    ops_done: Option<u64>,
    ops_total: Option<u64>,
    attempts: Option<u32>,
    last_error: Option<String>,
    /// Requests whose cut exists, each with that cut's time. Re-sent on
    /// every report: the agent holds these durably, this map does not.
    captures: Option<Vec<CaptureBindingBody>>,
    /// Requests whose cut was discarded before publication.
    lost: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct CaptureBindingBody {
    name: String,
    capture_time_ms: u64,
}

/// POST /v1/volumes/{vol}/cycle-status — the embedded agent reports its
/// cycle's progress or failure (D-M12). Progress and failure fields are
/// volume-level on purpose: every pending request of the volume waits on
/// the same cycle, so the server stamps them all. Capture bindings and
/// losses are per request: a request only ever receives the time of the
/// cut the agent bound it to, and that time never changes once set — a
/// different one means the original cut is gone, which fails the request
/// rather than silently moving its point in time. A volume with no
/// pending requests accepts the report as a no-op — the agent doesn't
/// track whether anyone is watching.
async fn report_cycle_status(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    Json(body): Json<CycleStatusBody>,
) -> std::result::Result<StatusCode, ApiError> {
    volume(&state, &vol).await?;
    if let Some(reqs) = state.snap_requests.lock().await.get_mut(&vol) {
        for st in reqs.values_mut().filter(|st| !st.completed) {
            if let Some(p) = &body.phase {
                st.phase = p.clone();
            }
            if let Some(d) = body.ops_done {
                st.ops_done = d;
            }
            if let Some(t) = body.ops_total {
                st.ops_total = t;
            }
            if let Some(a) = body.attempts {
                st.attempts = a;
            }
            if let Some(e) = &body.last_error {
                st.last_error = Some(e.clone());
            }
        }
        for capture in body.captures.iter().flatten() {
            let Some(st) = reqs.get_mut(&capture.name).filter(|st| !st.completed) else {
                continue;
            };
            match st.capture_time_ms {
                // The cut a failing earlier cycle never made now exists:
                // that cycle's error no longer describes this request.
                None if capture.capture_time_ms != 0 => {
                    st.capture_time_ms = Some(capture.capture_time_ms);
                    st.attempts = 0;
                    st.last_error = None;
                }
                Some(t) if t != capture.capture_time_ms => st.capture_lost = true,
                _ => {}
            }
        }
        for name in body.lost.iter().flatten() {
            if let Some(st) = reqs.get_mut(name).filter(|st| !st.completed) {
                st.capture_lost = true;
            }
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct OriginInfo {
    /// `snap/g<N>` ref name the origin serves.
    #[serde(rename = "ref")]
    ref_name: String,
    commit: String,
    /// The NBD export name (`<vol>/origin`).
    export: String,
    /// Whether the pointer row itself names this ref. `false` on GET
    /// means the row is absent or stale and this is the newest-generation
    /// fallback an initializing POST would adopt.
    initialized: bool,
    /// The generation the live export is SERVING right now — the
    /// Invariant E fence read (D-M15): resolved end-to-end from the
    /// export's engine, not the refs row. `None` while no origin export
    /// is registered.
    served: Option<String>,
}

#[derive(Deserialize, Default)]
struct SetOriginBody {
    /// Explicit repoint target (`snap/<name>`). Omitted = initialize:
    /// adopt the current pointer if live, else the newest generation
    /// (the D-M15 restage rule).
    #[serde(rename = "ref")]
    ref_name: Option<String>,
    /// Stage-time lease epoch (Invariant F, task 15h): present ⇒ the
    /// repoint transaction is fenced; a superseded epoch is a 409
    /// `lease_lost` and nothing moves.
    #[serde(default)]
    epoch: Option<u64>,
}

fn origin_info(
    vol: &str,
    r: skynbd_nbd::origin::ResolvedOrigin,
    served: Option<String>,
) -> OriginInfo {
    OriginInfo {
        commit: hex(&r.commit),
        export: skynbd_nbd::origin::origin_export_name(vol),
        ref_name: r.ref_name,
        initialized: r.initialized,
        served,
    }
}

/// The generation ref the origin export's engine is serving, resolved by
/// commit against the volume's generation refs (several refs can share a
/// commit; the highest generation is the fence's answer).
async fn origin_served(state: &ApiState, vol: &str, h: &VolumeHandle) -> Option<String> {
    let export = state
        .registry
        .get(&skynbd_nbd::origin::origin_export_name(vol))
        .await?;
    let commit = {
        let engine = export.engine.read().await;
        engine.head_commit_hash()
    };
    let refs = h.refs.list(h.vol_id).await.ok()?;
    refs.into_iter()
        .filter(|(n, rv)| rv.commit == commit && skynbd_nbd::origin::generation_number(n).is_some())
        .max_by_key(|(n, _)| skynbd_nbd::origin::generation_number(n))
        .map(|(n, _)| n)
}

fn refs_error(e: skynbd_refs::RefError) -> ApiError {
    match e {
        // A fenced/lease-lost refusal is a conflict with a structured
        // body, never a 500 — the caller can tell it from every other
        // failure and react (self-fence, re-stage) instead of retrying.
        skynbd_refs::RefError::LeaseLost { holder, epoch, .. } => {
            ApiError::lease_lost(&holder, epoch)
        }
        e => ApiError::msg(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

/// GET /v1/volumes/{vol}/origin — the resolved origin pointer (design
/// D-M15 Invariant D): what `<vol>/origin` serves, or — while
/// `initialized` is false — what an initializing POST would adopt. 404
/// when the volume has no pointer and no published generations.
async fn get_origin(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<OriginInfo>, ApiError> {
    let h = volume(&state, &vol).await?;
    let resolved = skynbd_nbd::origin::resolve(&h.refs, h.vol_id)
        .await
        .map_err(refs_error)?
        .ok_or_else(|| {
            ApiError::msg(
                StatusCode::NOT_FOUND,
                format!("volume {vol:?} has no published generations"),
            )
        })?;
    let served = origin_served(&state, &vol, &h).await;
    Ok(Json(origin_info(&vol, resolved, served)))
}

/// POST /v1/volumes/{vol}/origin — repoint the origin export (design
/// D-M15 §3 step 4). One transaction moves the pointer row and its
/// dedicated `origin` pin together, so the pointer always names a
/// generation GC cannot collect (Invariant D); then the export's backing
/// engine is swapped in place, so connections attached to
/// `<vol>/origin` serve the new generation from their next request
/// (Invariant A — no reload, no reconnect). Idempotent; an empty body
/// initializes the pointer instead of naming a target.
async fn set_origin(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    body: Option<Json<SetOriginBody>>,
) -> std::result::Result<Json<OriginInfo>, ApiError> {
    let h = volume(&state, &vol).await?;
    let body = body.map(|Json(b)| b).unwrap_or_default();
    let epoch = body.epoch;
    // Invariant F precondition (and the epoch-less fail-closed rule for
    // node-held volumes); the Some-epoch case is re-validated atomically
    // with the transaction below.
    check_epoch(&state, &h, epoch).await?;
    let target = match body.ref_name {
        Some(r) => {
            if !r.starts_with("snap/") {
                return Err(ApiError::msg(
                    StatusCode::BAD_REQUEST,
                    format!("origin target {r:?} must be a snapshot ref (snap/<name>)"),
                ));
            }
            let rv = h
                .refs
                .get(h.vol_id, &r)
                .await
                .map_err(refs_error)?
                .ok_or_else(|| {
                    ApiError::msg(
                        StatusCode::NOT_FOUND,
                        format!("unknown snapshot ref {r:?} on {vol:?}"),
                    )
                })?;
            skynbd_nbd::origin::ResolvedOrigin {
                ref_name: r,
                commit: rv.commit,
                initialized: true,
            }
        }
        None => skynbd_nbd::origin::resolve(&h.refs, h.vol_id)
            .await
            .map_err(refs_error)?
            .ok_or_else(|| {
                ApiError::msg(
                    StatusCode::NOT_FOUND,
                    format!("volume {vol:?} has no published generations to initialize from"),
                )
            })?,
    };
    let binding = VolumeBinding {
        vol_id: h.vol_id,
        vol_name: vol.clone(),
        store: Arc::clone(&h.store),
        refs: Arc::clone(&h.refs),
        cfg: h.cfg.clone(),
    };
    // Invariant E: the pointer moves only forward, and the compare and
    // the write are one atomic step (`MetaCas` on the row this attempt
    // read). A conflict means a concurrent writer moved the row — re-read
    // and re-decide; monotonicity converges, so a couple of retries
    // either win the CAS or land in the no-op branch.
    let holder = skynbd_nbd::origin::ORIGIN_PIN_HOLDER.to_string();
    for _ in 0..4 {
        let old_bytes = h
            .refs
            .meta_get(h.vol_id, skynbd_nbd::origin::ORIGIN_META_KEY)
            .await
            .map_err(refs_error)?;
        let old = old_bytes
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned());
        let forward = skynbd_nbd::origin::repoint_forward(old.as_deref(), &target.ref_name)
            .map_err(|reason| ApiError::msg(StatusCode::BAD_REQUEST, reason))?;
        if !forward {
            // Row already at the target's generation or a descendant: a
            // replayed or reordered repoint no-ops, and the response
            // reports what the pointer actually names.
            let resolved = skynbd_nbd::origin::resolve(&h.refs, h.vol_id)
                .await
                .map_err(refs_error)?
                .ok_or_else(|| {
                    ApiError::msg(
                        StatusCode::CONFLICT,
                        format!("origin pointer on {vol:?} names no resolvable generation"),
                    )
                })?;
            skynbd_nbd::origin::reconcile(&state.registry, &binding).await?;
            let served = origin_served(&state, &vol, &h).await;
            return Ok(Json(origin_info(&vol, resolved, served)));
        }
        let mut ops = vec![
            skynbd_refs::RefOp::Pin {
                name: target.ref_name.clone(),
                holder: holder.clone(),
            },
            skynbd_refs::RefOp::MetaCas {
                key: skynbd_nbd::origin::ORIGIN_META_KEY.into(),
                expected: old_bytes,
                value: target.ref_name.clone().into_bytes(),
            },
        ];
        if let Some(old) = old.filter(|o| *o != target.ref_name) {
            ops.push(skynbd_refs::RefOp::Unpin {
                name: old,
                holder: holder.clone(),
            });
        }
        // Invariant F (task 15h): a request carrying its stage-time
        // epoch runs the repoint through the fenced transact — a
        // superseded epoch is a 409 `lease_lost`, nothing moves.
        let outcome = match epoch {
            Some(e) => fenced_ops(&h, e, ops).await?,
            None => h.refs.transact(h.vol_id, ops).await.map_err(refs_error)?,
        };
        match outcome {
            skynbd_refs::TxnOutcome::Committed => {
                // Converge through the row, under the export's swap lock
                // (M1.3ai): a sync task's reconcile that read the row
                // before this commit, or a concurrent repoint whose
                // swap would land after this one, then cannot leave
                // the export on an older generation than the row names.
                skynbd_nbd::origin::reconcile(&state.registry, &binding).await?;
                let served = origin_served(&state, &vol, &h).await;
                return Ok(Json(origin_info(
                    &vol,
                    skynbd_nbd::origin::ResolvedOrigin {
                        initialized: true,
                        ..target
                    },
                    served,
                )));
            }
            skynbd_refs::TxnOutcome::Conflict { .. } => continue,
        }
    }
    Err(ApiError::msg(
        StatusCode::CONFLICT,
        format!("origin pointer on {vol:?} is contended; retry"),
    ))
}

/// DELETE /v1/volumes/{vol}/snapshot-requests/{name} — agent completed
/// the request. Keep its status for CSI's idempotent replay (bounded by
/// [`COMPLETED_SNAP_REQUESTS_RETAINED`]); the agent's list endpoint no
/// longer returns it, and snapshot deletion removes it. Idempotent.
async fn delete_snapshot_request(
    State(state): State<Arc<ApiState>>,
    Path((vol, name)): Path<(String, String)>,
) -> std::result::Result<StatusCode, ApiError> {
    volume(&state, &vol).await?;
    if let Some(requests) = state.snap_requests.lock().await.get_mut(&vol) {
        complete_snap_request(requests, &name);
    }
    Ok(StatusCode::NO_CONTENT)
}

fn complete_snap_request(
    requests: &mut std::collections::BTreeMap<String, SnapRequestStatus>,
    name: &str,
) {
    let next_seq = requests
        .values()
        .map(|st| st.completed_seq)
        .max()
        .unwrap_or(0)
        + 1;
    match requests.get_mut(name) {
        Some(st) if !st.completed => {
            st.completed = true;
            st.completed_seq = next_seq;
        }
        _ => return,
    }
    let completed = requests.values().filter(|st| st.completed).count();
    if completed > COMPLETED_SNAP_REQUESTS_RETAINED {
        let oldest = requests
            .iter()
            .filter(|(_, st)| st.completed)
            .min_by_key(|(_, st)| st.completed_seq)
            .map(|(name, _)| name.clone());
        if let Some(oldest) = oldest {
            requests.remove(&oldest);
        }
    }
}

async fn rollback(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    Json(body): Json<RollbackBody>,
) -> std::result::Result<Json<SnapshotInfo>, ApiError> {
    let h = volume(&state, &vol).await?;
    let commit = h.engine.write().await.rollback(&body.snapshot).await?;
    Ok(Json(SnapshotInfo {
        name: body.snapshot,
        commit: hex(&commit),
    }))
}

async fn run_gc(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<skynbd_engine::GcReport>, ApiError> {
    let h = volume(&state, &vol).await?;
    // Serialize with every active prepared session (design §10.2 note:
    // the single-node epoch substitute) — no mid-commit PUT can race the
    // sweep while all writers are held.
    let sessions: Vec<_> = h.prepared.lock().await.values().cloned().collect();
    let mut guards = Vec::with_capacity(sessions.len());
    for s in &sessions {
        guards.push(s.lock().await);
    }
    // Capture the marker BEFORE the sweep: debt recorded mid-sweep
    // (an agent thin isn't blocked by the session locks) was not seen
    // by this mark phase and must survive the clear.
    let owed_seen = owed_value(&h).await;
    let report = h.engine.write().await.gc().await?;
    // A completed sweep pays any outstanding debt (see thin_volume).
    clear_gc_owed(&h, owed_seen).await;
    Ok(Json(report))
}

#[derive(serde::Deserialize, Default)]
struct ThinBody {
    /// Node fence epoch (task 15h): present when the volume's staging
    /// agent drives the pass for a lineage it holds — the transaction
    /// runs `transact_as` under the lease row. Absent = the server's
    /// own authority (refused by the backend on node-held volumes).
    epoch: Option<u64>,
    /// Override of §6.2's `publishRetentionWindow` (seconds).
    window_s: Option<u64>,
    /// Override of the abandoned prepared-session TTL (seconds).
    prepared_ttl_s: Option<u64>,
}

#[derive(serde::Serialize)]
pub struct ThinReport {
    pub deleted_generations: Vec<String>,
    pub swept_sessions: Vec<String>,
    pub kept_configured: usize,
    pub kept_pinned: usize,
    pub kept_window: usize,
    /// "ran" | "owed" | "nothing" — whether the §10 sweep followed the
    /// thin, was recorded as owed (node-held lineage, or a busy
    /// engine), or had nothing to do.
    pub gc: &'static str,
}

/// §6.2 defaults; per-request overrides exist for the callers that own
/// a different dial (the agent's env, an operator's one-off).
const DEFAULT_PUBLISH_WINDOW_S: u64 = 24 * 60 * 60;
const DEFAULT_PREPARED_TTL_S: u64 = 7 * 24 * 60 * 60;

/// Retention thinning (design §6.2, task 15f): plan → ONE transaction →
/// GC. The GC step is what makes thinning mean anything at the store
/// level, and it is authority-gated: the §10 sweep requires the single
/// writer (it races in-flight, not-yet-referenced writes otherwise), so
/// an agent-driven pass on a node-held lineage records the sweep as
/// OWED inside the same transaction instead — paid by the next
/// server-side gc when the volume is back under server authority.
async fn thin_volume(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    body: Option<Json<ThinBody>>,
) -> std::result::Result<Json<ThinReport>, ApiError> {
    let h = volume(&state, &vol).await?;
    let b = body.map(|Json(b)| b).unwrap_or_default();
    let policy = retention::ThinPolicy {
        publish_window_ms: b.window_s.unwrap_or(DEFAULT_PUBLISH_WINDOW_S) * 1000,
        prepared_ttl_ms: b.prepared_ttl_s.unwrap_or(DEFAULT_PREPARED_TTL_S) * 1000,
    };
    Ok(Json(thin_pass(&h, policy, b.epoch).await?))
}

/// One retention pass over one volume — the route's body, shared with
/// the daemon's `[retention]` cadence so the two drivers cannot drift.
pub async fn thin_pass(
    h: &VolumeHandle,
    policy: retention::ThinPolicy,
    epoch: Option<u64>,
) -> std::result::Result<ThinReport, ApiError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let plan = retention::plan(&h.store, &h.refs, h.vol_id, policy, now).await?;
    let mut report = ThinReport {
        deleted_generations: plan.delete_generations.clone(),
        swept_sessions: plan.sweep_sessions.clone(),
        kept_configured: plan.kept_configured,
        kept_pinned: plan.kept_pinned,
        kept_window: plan.kept_window,
        gc: "nothing",
    };
    if plan.is_empty() {
        return Ok(report);
    }
    match epoch {
        Some(epoch) => {
            // Agent-driven: the fence carries the authority, the sweep
            // debt rides in the same atomic batch.
            let mut ops = plan.ops;
            ops.push(retention::owed_marker());
            match fenced_ops(h, epoch, ops).await? {
                skynbd_refs::TxnOutcome::Committed => report.gc = "owed",
                skynbd_refs::TxnOutcome::Conflict { .. } => {
                    return Err(EngineError::Conflict.into());
                }
            }
        }
        None => {
            // The thin transaction itself deliberately runs WITHOUT
            // the prepared-session locks the sweep below takes: refs
            // transactions are atomic and CAS-exact, so a session or
            // generation that moves aborts the batch — the locks
            // exist for the store-level sweep, which has no such guard.
            match h
                .refs
                .transact(h.vol_id, plan.ops)
                .await
                .map_err(refs_error)?
            {
                skynbd_refs::TxnOutcome::Committed => {}
                skynbd_refs::TxnOutcome::Conflict { .. } => {
                    return Err(EngineError::Conflict.into());
                }
            }
            // Same writer-serialization as run_gc: hold every prepared
            // session so no mid-commit PUT races the sweep.
            let sessions: Vec<_> = h.prepared.lock().await.values().cloned().collect();
            let mut guards = Vec::with_capacity(sessions.len());
            for s in &sessions {
                guards.push(s.lock().await);
            }
            let owed_seen = owed_value(h).await;
            match h.engine.write().await.gc().await {
                Ok(_) => {
                    drop(guards);
                    clear_gc_owed(h, owed_seen).await;
                    report.gc = "ran";
                }
                // A dirty overlay (live writer) defers, it doesn't fail:
                // the thin itself committed.
                Err(_) => {
                    drop(guards);
                    let _ = h
                        .refs
                        .transact(h.vol_id, vec![retention::owed_marker()])
                        .await;
                    report.gc = "owed";
                }
            }
        }
    }
    Ok(report)
}

async fn owed_value(h: &VolumeHandle) -> Option<Vec<u8>> {
    h.refs
        .meta_get(h.vol_id, retention::GC_OWED_KEY)
        .await
        .ok()
        .flatten()
}

/// Best-effort: the owed marker is a debt note, not an invariant — a
/// failed clear leaves a spurious note the next sweep clears again.
/// The clear is guarded on the exact nonce read before the sweep
/// began (`seen`): a marker written or replaced mid-sweep records
/// debt that sweep never saw, and the guard makes the clear conflict
/// instead of eating it.
async fn clear_gc_owed(h: &VolumeHandle, seen: Option<Vec<u8>>) {
    let Some(seen) = seen else { return };
    let _ = h
        .refs
        .transact(
            h.vol_id,
            vec![
                skynbd_refs::RefOp::MetaCas {
                    key: retention::GC_OWED_KEY.into(),
                    expected: Some(seen.clone()),
                    value: seen,
                },
                skynbd_refs::RefOp::MetaDelete {
                    key: retention::GC_OWED_KEY.into(),
                },
            ],
        )
        .await;
}

async fn run_fsck(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<skynbd_engine::FsckReport>, ApiError> {
    let h = volume(&state, &vol).await?;
    // Shared hold: GC (and commits) need the exclusive lock, so no sweep
    // can delete what the walk is about to read. NBD reads of this
    // engine keep flowing only until a writer queues: the lock is fair,
    // so a WRITE or a dirty volume's RPO tick waits for the whole walk,
    // and every later read of this export waits behind it. On a large
    // volume, run fsck when the export is idle. (Only GC needs
    // excluding; a GC-only gate would lift this — the fetch-outside-the-
    // lock follow-up to #97 noted on `Export::engine`.)
    let _guard = h.engine.read().await;
    let report =
        skynbd_engine::fsck_with(&h.store, &h.refs, h.vol_id, h.cfg.walk_concurrency).await?;
    Ok(Json(report))
}

#[derive(Deserialize)]
struct DiffParams {
    /// Source snapshot name; absent/empty = full baseline stream.
    #[serde(default)]
    from: String,
    /// Target snapshot name.
    to: String,
}

/// Bearer authentication + per-identity authorization (auth design §4).
/// `/healthz` is exempt so load balancers and probes need no credential.
/// On success the request carries an [`AuthzCtx`] extension (identity for
/// audit logging; future handlers may refine checks).
async fn require_bearer(
    State(state): State<Arc<ApiState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    if req.uri().path() == "/healthz" {
        return next.run(req).await;
    }
    let bearer = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "));
    let ctx = match &state.auth {
        ApiAuth::Disabled => AuthzCtx::full(),
        ApiAuth::Static(expected) => match bearer {
            Some(got) if auth::static_token_matches(expected, got) => AuthzCtx::full(),
            _ => {
                return (StatusCode::UNAUTHORIZED, "missing or invalid bearer token")
                    .into_response()
            }
        },
        ApiAuth::Oidc(oidc) => {
            let Some(token) = bearer else {
                return (StatusCode::UNAUTHORIZED, "missing bearer token").into_response();
            };
            match oidc.authenticate(token).await {
                Ok(ctx) => ctx,
                Err(auth::AuthError::Unauthenticated(msg)) => {
                    return (StatusCode::UNAUTHORIZED, msg).into_response()
                }
                Err(auth::AuthError::Forbidden) => {
                    return (StatusCode::FORBIDDEN, "no binding grants this identity")
                        .into_response()
                }
            }
        }
    };
    // Authorization: restricted identities must hold a grant for this
    // route's op group (and volume, where scoped); unknown routes fail
    // closed for them and 404 via the router for unrestricted callers.
    if ctx.is_restricted() {
        let allowed = auth::route_perm(req.method(), req.uri().path())
            .is_some_and(|(op, vol)| ctx.allows(op, vol.as_deref()));
        if !allowed {
            tracing::debug!(identity = ?ctx.identity, path = %req.uri().path(),
                "authorization denied");
            return (
                StatusCode::FORBIDDEN,
                "operation not permitted for this identity",
            )
                .into_response();
        }
    }
    req.extensions_mut().insert(ctx);
    next.run(req).await
}

#[derive(Deserialize)]
struct EnrollBody {
    /// PEM PKCS#10 request; only its (self-signature-verified) public
    /// key is used — subject/SANs/usages are server-assigned.
    csr_pem: String,
}

#[derive(Serialize)]
struct EnrollResponse {
    cert_pem: String,
    ca_pem: String,
    /// The URI SAN in the cert — what NBD ACLs match.
    identity: String,
    not_after: String,
}

/// POST /v1/certs/enroll (auth design §3.2 amendment): short-lived NBD
/// mTLS client cert for the caller's *token-verified* identity. Static
/// and disabled auth modes carry no verified identity, so they cannot
/// enroll — the SAN must never be caller-chosen.
async fn enroll_cert(
    State(state): State<Arc<ApiState>>,
    axum::Extension(ctx): axum::Extension<AuthzCtx>,
    Json(body): Json<EnrollBody>,
) -> std::result::Result<Json<EnrollResponse>, ApiError> {
    let Some(issuer) = &state.enroll else {
        return Err(ApiError::msg(
            StatusCode::NOT_FOUND,
            "cert enrollment is not configured",
        ));
    };
    let sub = ctx
        .sub
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            ApiError::msg(
                StatusCode::FORBIDDEN,
                "enrollment requires an identity-bearing (OIDC) token",
            )
        })?;
    match issuer.issue(&body.csr_pem, sub) {
        Ok(cert) => {
            tracing::info!(identity = %cert.identity, not_after = %cert.not_after,
                "issued NBD client cert");
            Ok(Json(EnrollResponse {
                cert_pem: cert.cert_pem,
                ca_pem: cert.ca_pem,
                identity: cert.identity,
                not_after: cert.not_after,
            }))
        }
        Err(e @ enroll::EnrollError::BadCsr(_)) => {
            Err(ApiError::msg(StatusCode::BAD_REQUEST, e.to_string()))
        }
        Err(enroll::EnrollError::Ca(e)) => {
            tracing::error!(error = %e, "cert enrollment CA unavailable");
            Err(ApiError::msg(
                StatusCode::INTERNAL_SERVER_ERROR,
                "enrollment CA unavailable",
            ))
        }
    }
}

/// GET /v1/volumes/{vol}/snapshots/{name}/pins — holders (I8 visibility;
/// the agent's `pinned_by` observation).
async fn list_pins(
    State(state): State<Arc<ApiState>>,
    Path((vol, name)): Path<(String, String)>,
) -> std::result::Result<Json<Vec<String>>, ApiError> {
    let h = volume(&state, &vol).await?;
    let holders = h
        .refs
        .pins(h.vol_id, &format!("snap/{name}"))
        .await
        .map_err(|e| ApiError::msg(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(holders))
}

#[derive(Deserialize)]
struct DataParams {
    off: u64,
    len: u64,
}

/// Read-capped range read of a published snapshot (agent verify path).
const MAX_DATA_READ: u64 = 32 * 1024 * 1024;

/// GET /v1/volumes/{vol}/snapshots/{name}/data?off=&len= — snapshot bytes.
async fn read_snapshot_data(
    State(state): State<Arc<ApiState>>,
    Path((vol, name)): Path<(String, String)>,
    axum::extract::Query(p): axum::extract::Query<DataParams>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    if p.len > MAX_DATA_READ {
        return Err(ApiError::msg(
            StatusCode::BAD_REQUEST,
            format!("len > {MAX_DATA_READ}"),
        ));
    }
    let h = volume(&state, &vol).await?;
    let commit = resolve_snapshot(&h, &name).await?;
    let ro = skynbd_engine::Engine::open_at_commit(
        Arc::clone(&h.store),
        Arc::clone(&h.refs),
        h.vol_id,
        commit,
        // The volume's fan-out knobs, not its geometry or fence epoch.
        h.cfg.tuning(),
    )
    .await?;
    let bytes = ro.read(p.off, p.len).await?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        bytes,
    ))
}

async fn resolve_snapshot(h: &VolumeHandle, name: &str) -> std::result::Result<[u8; 32], ApiError> {
    let snaps = h.engine.read().await.snapshot_list().await?;
    snaps
        .into_iter()
        .find(|(n, _)| n == name)
        .map(|(_, c)| c)
        .ok_or_else(|| ApiError::msg(StatusCode::NOT_FOUND, format!("unknown snapshot {name:?}")))
}

impl From<skynbd_stream::StreamError> for ApiError {
    fn from(e: skynbd_stream::StreamError) -> Self {
        use skynbd_stream::StreamError as E;
        let code = match &e {
            E::Torn | E::Checksum | E::Malformed(_) => StatusCode::BAD_REQUEST,
            E::TargetMismatch(_) => StatusCode::CONFLICT,
            E::Engine(_) | E::Format(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        ApiError::msg(code, e.to_string())
    }
}

/// GET /v1/volumes/{vol}/diff?from=&to= — SKYD1 stream (design §9.4).
async fn diff(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    Query(params): Query<DiffParams>,
) -> std::result::Result<impl IntoResponse, ApiError> {
    let h = volume(&state, &vol).await?;
    let from = if params.from.is_empty() {
        None
    } else {
        Some(resolve_snapshot(&h, &params.from).await?)
    };
    let to = resolve_snapshot(&h, &params.to).await?;
    let mut out = Vec::new();
    skynbd_stream::send(
        Arc::clone(&h.store),
        Arc::clone(&h.refs),
        h.vol_id,
        from,
        to,
        h.cfg.tuning(),
        &mut out,
    )
    .await?;
    Ok((
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        out,
    ))
}

/// POST /v1/volumes/{vol}/import — apply a SKYD1 stream, then advance the
/// rpo ref (design §9.5: import batches advance vol@rpo).
async fn import(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    body: axum::body::Bytes,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let mut engine = h.engine.write().await;
    let commit = skynbd_stream::apply(&mut engine, &body).await?;
    let rpo = engine.rpo_advance().await?;
    Ok(Json(serde_json::json!({
        "commit": hex(&commit),
        "rpo_advanced": rpo.is_some(),
    })))
}

/// POST /v1/volumes/{vol}/rpo — manual advance (design §9.3).
async fn rpo_advance(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let advanced = h.engine.write().await.rpo_advance().await?;
    Ok(Json(serde_json::json!({
        "advanced": advanced.is_some(),
        "commit": advanced.map(|c| hex(&c)),
    })))
}

// ------------------------------------------- V3.2 ingestion plane (§9A)

fn parse_hash(hexstr: &str) -> std::result::Result<[u8; 32], ApiError> {
    let bytes = (0..64)
        .step_by(2)
        .map(|i| u8::from_str_radix(hexstr.get(i..i + 2).unwrap_or("zz"), 16))
        .collect::<std::result::Result<Vec<u8>, _>>()
        .map_err(|_| ApiError::msg(StatusCode::BAD_REQUEST, "bad hash hex"))?;
    if hexstr.len() != 64 {
        return Err(ApiError::msg(
            StatusCode::BAD_REQUEST,
            "hash must be 64 hex chars",
        ));
    }
    Ok(bytes.try_into().unwrap())
}

async fn session(
    h: &VolumeHandle,
    id: &str,
) -> std::result::Result<Arc<Mutex<DynPrepared>>, ApiError> {
    let sid = SnapId::from_string(id)
        .map_err(|_| ApiError::msg(StatusCode::BAD_REQUEST, "bad prepared id"))?;
    let mut map = h.prepared.lock().await;
    if let Some(s) = map.get(id) {
        return Ok(Arc::clone(s));
    }
    // Lazy reopen after restart (§9A.6 resume).
    let s =
        PreparedSession::open_existing(Arc::clone(&h.store), Arc::clone(&h.refs), h.vol_id, sid)
            .await?
            .with_walk_concurrency(h.cfg.walk_concurrency);
    let s = Arc::new(Mutex::new(s));
    map.insert(id.to_string(), Arc::clone(&s));
    Ok(s)
}

#[derive(Deserialize)]
struct CreatePreparedBody {
    /// Base snapshot name; absent = bootstrap (empty mapping).
    base: Option<String>,
    /// Stage-time lease epoch (Invariant F, task 15h): validated as a
    /// precondition — prepare pins the base, so a superseded stage must
    /// not open new sessions (or take new pins) on the lineage.
    #[serde(default)]
    epoch: Option<u64>,
}

async fn create_prepared(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
    Json(body): Json<CreatePreparedBody>,
) -> std::result::Result<(StatusCode, Json<serde_json::Value>), ApiError> {
    let h = volume(&state, &vol).await?;
    check_epoch(&state, &h, body.epoch).await?;
    let base = match &body.base {
        None => None,
        Some(name) => {
            let commit = resolve_snapshot(&h, name).await?;
            Some((format!("snap/{name}"), commit))
        }
    };
    let sid = SnapId(ulid::Ulid::new());
    let sess = PreparedSession::create(
        Arc::clone(&h.store),
        Arc::clone(&h.refs),
        h.vol_id,
        sid,
        base,
    )
    .await?
    .with_walk_concurrency(h.cfg.walk_concurrency);
    let id = sid.to_string();
    h.prepared
        .lock()
        .await
        .insert(id.clone(), Arc::new(Mutex::new(sess)));
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "snap_id": id })),
    ))
}

async fn list_prepared(
    State(state): State<Arc<ApiState>>,
    Path(vol): Path<String>,
) -> std::result::Result<Json<Vec<String>>, ApiError> {
    let h = volume(&state, &vol).await?;
    let ids = skynbd_engine::prepared::list_prepared(&h.refs, h.vol_id).await?;
    Ok(Json(ids.into_iter().map(|s| s.to_string()).collect()))
}

async fn prepared_status(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
) -> std::result::Result<Json<skynbd_engine::prepared::PreparedStatus>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    let status = s.lock().await.status();
    Ok(Json(status))
}

#[derive(Deserialize)]
struct CheckpointBody {
    op_id: u64,
}

/// POST …/prepared/{id}/checkpoint — durable barrier at `op_id` (design
/// D12; the agent's pre-publish fence).
async fn prepared_checkpoint(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Json(body): Json<CheckpointBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    s.lock().await.checkpoint(body.op_id).await?;
    Ok(Json(serde_json::json!({ "op_watermark": body.op_id })))
}

#[derive(Deserialize)]
struct AttachBody {
    ranges: Vec<AttachRange>,
    op_id: u64,
}

#[derive(Deserialize)]
struct AttachRange {
    off: u64,
    len: u64,
    hash: String,
    #[serde(default)]
    obj_off: u64,
}

async fn prepared_attach(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Json(body): Json<AttachBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    let mut ranges = Vec::with_capacity(body.ranges.len());
    for r in &body.ranges {
        ranges.push((r.off, r.len, parse_hash(&r.hash)?, r.obj_off));
    }
    let applied = s.lock().await.attach(&ranges, body.op_id).await?;
    Ok(Json(serde_json::json!({ "applied": applied })))
}

#[derive(Deserialize)]
struct ClearBody {
    ranges: Vec<ClearRange>,
    op_id: u64,
}

#[derive(Deserialize)]
struct ClearRange {
    off: u64,
    len: u64,
}

async fn prepared_clear(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Json(body): Json<ClearBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    let ranges: Vec<(u64, u64)> = body.ranges.iter().map(|r| (r.off, r.len)).collect();
    let applied = s.lock().await.clear(&ranges, body.op_id).await?;
    Ok(Json(serde_json::json!({ "applied": applied })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyBody {
    ops: Vec<ApplyOp>,
    op_id: u64,
}

/// One op of a mixed batch: `{"op":"attach","off":..,"len":..,"hash":..,
/// "obj_off":..}` or `{"op":"clear","off":..,"len":..}`. An unknown `op`
/// or field is a 422, never skipped — a skipped op or modifier would be
/// checkpointed past. (This is why the shapes are not shared with
/// `/attach` and `/clear`, which keep their lenient historical parsing.)
/// A new op kind or modifier gets a new [`FEATURES`] entry, so clients
/// never send it to a server that would refuse it.
#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "snake_case", deny_unknown_fields)]
enum ApplyOp {
    Attach {
        off: u64,
        len: u64,
        hash: String,
        #[serde(default)]
        obj_off: u64,
    },
    Clear {
        off: u64,
        len: u64,
    },
}

/// Mixed attach/clear batch under one op id (issue #97 follow-up): the
/// agent's interleaved writes and discards in a single call, staged in
/// order. Same replay and checkpoint semantics as `attach` / `clear`.
async fn prepared_apply(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Json(body): Json<ApplyBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    let mut ops = Vec::with_capacity(body.ops.len());
    for op in &body.ops {
        ops.push(match op {
            ApplyOp::Attach {
                off,
                len,
                hash,
                obj_off,
            } => RangeOp::Attach {
                off: *off,
                len: *len,
                hash: parse_hash(hash)?,
                obj_off: *obj_off,
            },
            ApplyOp::Clear { off, len } => RangeOp::Clear {
                off: *off,
                len: *len,
            },
        });
    }
    let applied = s.lock().await.apply(&ops, body.op_id).await?;
    Ok(Json(serde_json::json!({ "applied": applied })))
}

#[derive(Deserialize)]
struct EnsureBody {
    hashes: Vec<String>,
}

async fn prepared_ensure(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Json(body): Json<EnsureBody>,
) -> std::result::Result<Json<serde_json::Value>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    let mut hashes = Vec::with_capacity(body.hashes.len());
    for hx in &body.hashes {
        hashes.push(parse_hash(hx)?);
    }
    let exists = s.lock().await.ensure(&hashes).await?;
    Ok(Json(serde_json::json!({ "exists": exists })))
}

#[derive(Deserialize)]
struct PublishBody {
    name: String,
    #[serde(default = "default_holder")]
    holder: String,
    /// The caller's stage-time lease epoch (Invariant F, task 15h).
    /// Present ⇒ the publish transaction is fenced: unless the volume's
    /// lease row still carries exactly this epoch, nothing lands and the
    /// answer is a 409 `lease_lost`. Absent ⇒ the legacy unfenced path.
    #[serde(default)]
    epoch: Option<u64>,
    /// §8.2 salvage (task 15h): when the epoch validation FAILS with a
    /// superseded epoch, re-execute the publish as a named fork outside
    /// the lineage (`salvage/<name>-e<epoch>`), re-stamped under the
    /// current lease. Requires `epoch`.
    #[serde(default)]
    salvage: bool,
}

fn default_holder() -> String {
    "publisher".into()
}

/// The task-15h fenced-transaction helper: validate the request's
/// stage-time `epoch` against the volume's lease row, then run the
/// transaction through `transact_as` under the row's holder + that epoch
/// — TOCTOU-safe because `transact_as` re-validates atomically with the
/// transaction. A mismatch anywhere is the distinguishable 409
/// `lease_lost`.
async fn fenced_ops(
    h: &VolumeHandle,
    epoch: u64,
    ops: Vec<skynbd_refs::RefOp>,
) -> std::result::Result<skynbd_refs::TxnOutcome, ApiError> {
    let (holder, cur) = lease_row(h).await?;
    if cur != epoch {
        return Err(ApiError::lease_lost(&holder, cur));
    }
    match h.refs.transact_as(h.vol_id, ops, &holder, epoch).await {
        Ok(out) => Ok(out),
        Err(skynbd_refs::RefError::LeaseLost { holder, epoch, .. }) => {
            Err(ApiError::lease_lost(&holder, epoch))
        }
        Err(e) => Err(refs_error(e)),
    }
}

/// The volume's lease row as `(holder, epoch)`; an absent row is
/// `("", 0)` — the same shape `transact_as` reports, so a caller-supplied
/// epoch can never match a volume nobody leases by accident (stage-time
/// epochs are ≥ 1).
async fn lease_row(h: &VolumeHandle) -> std::result::Result<(String, u64), ApiError> {
    Ok(h.refs
        .lease_get(h.vol_id)
        .await
        .map_err(refs_error)?
        .map(|l| (l.holder, l.epoch))
        .unwrap_or_default())
}

/// The fail-closed rule for epoch-less mutating requests (task 15h): a
/// volume served under a NODE's lease (its recorded `lease_holder` is
/// not this server's own identity) refuses requests that carry no
/// epoch, with the structured 409 `lease_lost` naming the actual owner
/// — otherwise any legacy caller could bypass Invariant F wholesale.
/// Server-held and lease-less volumes keep the unfenced legacy path
/// (config-volume back-compat).
async fn require_epoch_when_node_held(
    state: &ApiState,
    h: &VolumeHandle,
) -> std::result::Result<(), ApiError> {
    let Some(holder) = h.lease_holder.as_deref() else {
        return Ok(());
    };
    if state.provisioner.as_ref().and_then(|p| p.identity()) == Some(holder) {
        return Ok(());
    }
    let (row_holder, row_epoch) = lease_row(h).await?;
    Err(ApiError::lease_lost(&row_holder, row_epoch))
}

/// Shared precondition of every fenced route (task 15h): an epoch given
/// must match the volume's lease row (else the structured 409); an
/// epoch absent falls under the node-held fail-closed rule.
async fn check_epoch(
    state: &ApiState,
    h: &VolumeHandle,
    epoch: Option<u64>,
) -> std::result::Result<(), ApiError> {
    match epoch {
        Some(e) => {
            let (holder, cur) = lease_row(h).await?;
            if cur != e {
                return Err(ApiError::lease_lost(&holder, cur));
            }
            Ok(())
        }
        None => require_epoch_when_node_held(state, h).await,
    }
}

async fn prepared_publish(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Json(body): Json<PublishBody>,
) -> std::result::Result<Json<SnapshotInfo>, ApiError> {
    let h = volume(&state, &vol).await?;
    let s = session(&h, &id).await?;
    // Take ownership: publish consumes the session (§9A.2 idempotency:
    // once published, the snap ref exists and re-publish 409s by name).
    h.prepared.lock().await.remove(&id);
    let mut sess = Arc::try_unwrap(s)
        .map_err(|_| ApiError::msg(StatusCode::CONFLICT, "prepared session busy"))?
        .into_inner();
    let Some(epoch) = body.epoch else {
        // Legacy/unfenced callers (config-volume rigs, pre-15h agents).
        // Fail-closed on node-held volumes: an epoch-less publish there
        // would bypass Invariant F entirely.
        require_epoch_when_node_held(&state, &h).await?;
        let commit = sess.publish(&body.name, &body.holder).await?;
        return Ok(Json(SnapshotInfo {
            name: body.name,
            commit: hex(&commit),
        }));
    };
    // Invariant F: validate the stage-time epoch, then publish through
    // the fenced transact under the row's holder — re-validated
    // atomically with the I8 transaction itself.
    let (row_holder, row_epoch) = lease_row(&h).await?;
    let fenced = if row_epoch == epoch {
        match sess
            .publish_fenced(&body.name, &body.holder, Some((row_holder, epoch)))
            .await
        {
            Ok(commit) => {
                return Ok(Json(SnapshotInfo {
                    name: body.name,
                    commit: hex(&commit),
                }))
            }
            // Raced: the lease moved between our read and the transact.
            Err(EngineError::Fenced { holder, epoch }) => (holder, epoch),
            Err(e) => return Err(e.into()),
        }
    } else {
        (row_holder, row_epoch)
    };
    if !body.salvage {
        return Err(ApiError::lease_lost(&fenced.0, fenced.1));
    }
    // §8.2 salvage: the fenced delta enters as a write-once fork outside
    // the lineage, re-stamped under the CURRENT lease. The response's
    // `name` is the full salvage ref so the caller knows where it landed.
    let commit = sess.publish_salvage(&body.name, epoch, fenced).await?;
    Ok(Json(SnapshotInfo {
        name: format!("salvage/{}-e{epoch}", body.name),
        commit: hex(&commit),
    }))
}

async fn abort_prepared(
    State(state): State<Arc<ApiState>>,
    Path((vol, id)): Path<(String, String)>,
    Query(p): Query<EpochParam>,
) -> std::result::Result<StatusCode, ApiError> {
    let h = volume(&state, &vol).await?;
    // Invariant F precondition (abort unpins the base — a superseded
    // stage must not release the current owner's baseline anchor).
    check_epoch(&state, &h, p.epoch).await?;
    let s = session(&h, &id).await?;
    h.prepared.lock().await.remove(&id);
    let sess = Arc::try_unwrap(s)
        .map_err(|_| ApiError::msg(StatusCode::CONFLICT, "prepared session busy"))?
        .into_inner();
    sess.abort().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// PUT /objects/{hash}: body verified against its BLAKE3 name (D11).
async fn put_object(
    State(state): State<Arc<ApiState>>,
    Path((vol, hash)): Path<(String, String)>,
    body: axum::body::Bytes,
) -> std::result::Result<StatusCode, ApiError> {
    let h = volume(&state, &vol).await?;
    let expect = parse_hash(&hash)?;
    if *blake3::hash(&body).as_bytes() != expect {
        return Err(ApiError::msg(
            StatusCode::BAD_REQUEST,
            "body does not hash to the given name",
        ));
    }
    h.store
        .put(&skynbd_engine::keys::cas_object(h.vol_id, &expect), body)
        .await
        .map_err(|e| ApiError::msg(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(StatusCode::CREATED)
}

#[derive(Deserialize)]
struct PinBody {
    holder: String,
    /// Stage-time lease epoch (Invariant F, task 15h): present ⇒ fenced.
    #[serde(default)]
    epoch: Option<u64>,
}

/// Query-carried epoch for bodyless mutations (DELETE pins).
#[derive(Deserialize, Default)]
struct EpochParam {
    epoch: Option<u64>,
}

async fn pin_snapshot(
    State(state): State<Arc<ApiState>>,
    Path((vol, name)): Path<(String, String)>,
    Json(body): Json<PinBody>,
) -> std::result::Result<StatusCode, ApiError> {
    let h = volume(&state, &vol).await?;
    let ref_name = format!("snap/{name}");
    resolve_snapshot(&h, &name).await?; // 404 unless it exists
    let ops = vec![skynbd_refs::RefOp::Pin {
        name: ref_name,
        holder: body.holder,
    }];
    check_epoch(&state, &h, body.epoch).await?;
    match body.epoch {
        Some(e) => _ = fenced_ops(&h, e, ops).await?,
        None => {
            h.refs.transact(h.vol_id, ops).await.map_err(refs_error)?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn unpin_snapshot(
    State(state): State<Arc<ApiState>>,
    Path((vol, name, holder)): Path<(String, String, String)>,
    Query(p): Query<EpochParam>,
) -> std::result::Result<StatusCode, ApiError> {
    let h = volume(&state, &vol).await?;
    let ops = vec![skynbd_refs::RefOp::Unpin {
        name: format!("snap/{name}"),
        holder,
    }];
    check_epoch(&state, &h, p.epoch).await?;
    match p.epoch {
        Some(e) => _ = fenced_ops(&h, e, ops).await?,
        None => {
            h.refs.transact(h.vol_id, ops).await.map_err(refs_error)?;
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

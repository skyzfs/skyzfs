//! `HttpControlPlane`: the production `ControlPlane` over the server's
//! REST API (§9A / §12), with optional static bearer auth (agent design
//! doc §9). Exercised by the S15 crash matrix against a real in-process
//! axum server over real HTTP.

use serde::Deserialize;

use crate::control::{CaptureBinding, ControlPlane, CpError, CycleProgress, PreparedInfo, RangeOp};

/// Where the bearer token comes from. `File` is re-read on every request
/// (auth design §4): projected ServiceAccount tokens are rotated in place
/// by the kubelet, so a startup-time read would go stale within hours.
#[derive(Clone)]
enum TokenSource {
    None,
    Static(String),
    File(std::path::PathBuf),
}

#[derive(Clone)]
pub struct HttpControlPlane {
    client: reqwest::Client,
    /// e.g. `http://127.0.0.1:8080`
    base: String,
    /// Lineage volume name (server-side).
    volume: String,
    token: TokenSource,
    /// Stage-time lease epoch (Invariant F, task 15h): stamped into
    /// every mutating request body so the server fences superseded
    /// stages. `None` = legacy unfenced behavior (servers ignore an
    /// absent epoch).
    epoch: Option<u64>,
    /// What the server's `x-skynbd-features` header said on its latest
    /// response ([`FEATURES_UNKNOWN`] until one arrives). Shared by
    /// clones, and re-read on every response, so a rolling upgrade or a
    /// downgrade is noticed on the next call.
    features: std::sync::Arc<std::sync::atomic::AtomicU8>,
    /// Whether mixed batches go through `apply` ([`MixedApply`]).
    mixed_apply: crate::MixedApply,
}

/// Response header the server lists its optional features in.
const FEATURES_HEADER: &str = "x-skynbd-features";
/// The mixed attach/clear batch (`POST …/prepared/{id}/apply`).
const FEATURE_APPLY: &str = "prepared-apply";
const FEATURES_UNKNOWN: u8 = 0;
const FEATURES_APPLY: u8 = 1;
const FEATURES_NONE: u8 = 2;

/// How long a connect may take on its own. Note this is not additive
/// with [`READ_TIMEOUT`]: the read timer is armed before the connector
/// future is first polled, so connect time is charged against it too and
/// the worst case for a whole request is 60 s, not 70 s.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// The deadline from dispatch to response headers: connect, request
/// upload, and server think time, all under one clock.
///
/// **This is not the inter-byte stall timer the name suggests.** Read
/// from reqwest 0.12.28 and measured: `read_timeout` arms exactly one
/// `sleep` when the request is dispatched (`client.rs:2637`) and polls
/// it without ever resetting (`client.rs:3052`). Only the response
/// *body* gets the per-frame reset (`body.rs:357`). So a 64 MiB upload
/// that is still moving bytes at the deadline is killed anyway — the
/// timer does not care that the transfer is progressing.
///
/// Which is the dial §8 wants, so it stays: a control plane that cannot
/// answer within 60 s should make cycles fail, and failing is the whole
/// point. But it is denominated in **request size**, and the size is
/// operator-tunable: `put_object` carries one `chunk_bytes` object
/// (64 KiB default) and `ensure` walks one hash at a time, so nothing
/// binds today — but `preflight::check_chunk` only requires
/// `SKYNBD_CHUNK_BYTES` to equal the pool block size, and dm-thin allows
/// that to 1 GiB. At a large chunk over a degraded link this becomes a
/// cap on per-request upload size, and because each retry restarts the
/// object from zero, the cycle livelocks instead of failing. Raise it
/// with the chunk, or the ladder never gets walked.
///
/// Safe to add at all only because a timeout makes every mutating call
/// able to report a retryable failure for an operation the server
/// actually completed. That ambiguity already existed — a crash
/// mid-request is indistinguishable — and `op_id` plus `find_prepared`
/// are what already resolve it.
///
/// Without any of this the client had NO timeout of any kind (measured:
/// a server that accepts and never answers hangs the request
/// indefinitely), and that made §8's whole degradation ladder
/// unreachable in exactly the case §8.2 is named for. Rule 3's clock is
/// driven by cycles FAILING; a cycle that hangs never fails, so
/// `failing_since` is never set, the read-only watermark never fires,
/// and the loop's task is parked forever holding a volume that publishes
/// nothing. Connection-REFUSED (a dead port) always failed fast — it is
/// the accepted-and-silent partition, the ordinary Kubernetes one, that
/// hung.
const READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

fn client_with(connect: std::time::Duration, read: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(connect)
        .read_timeout(read)
        .build()
        // The builder only fails on TLS backend initialization, which
        // is the same failure `Client::new()` panics on; matching that
        // keeps the constructors infallible for every caller.
        .expect("reqwest client")
}

/// The control-plane HTTP client, timeouts and all.
///
/// Public because the CSI driver's own control-plane client
/// (`skynbd-csi-driver`'s `cp::Client`) needs timeouts for exactly these
/// reasons, and it is the shape where the accepted-and-silent partition
/// is the *ordinary* one. Two copies of `Client::new()` is how this bug
/// existed twice; one constructor is how it stops being possible to
/// reintroduce in either binary.
pub fn control_plane_client() -> reqwest::Client {
    client_with(CONNECT_TIMEOUT, READ_TIMEOUT)
}

/// The same client with caller-chosen dials, for a caller whose requests
/// are a different shape than this crate's.
///
/// [`READ_TIMEOUT`] is sized for the agent's upload path — it has to
/// cover a whole `chunk_bytes` object going out — which makes it the
/// wrong number for a caller that only makes small RPCs and has someone
/// waiting on the answer. Sharing the *constructor* is the point;
/// sharing the constants is not, and pretending otherwise is what put a
/// 60 s upload dial on a path that holds a node-wide lock.
pub fn control_plane_client_with(
    connect: std::time::Duration,
    read: std::time::Duration,
) -> reqwest::Client {
    client_with(connect, read)
}

fn client() -> reqwest::Client {
    control_plane_client()
}

impl HttpControlPlane {
    pub fn new(base: impl Into<String>, volume: impl Into<String>, token: Option<String>) -> Self {
        Self {
            client: client(),
            base: base.into(),
            volume: volume.into(),
            token: match token {
                Some(t) => TokenSource::Static(t),
                None => TokenSource::None,
            },
            epoch: None,
            features: std::sync::Arc::new(FEATURES_UNKNOWN.into()),
            mixed_apply: crate::MixedApply::Auto,
        }
    }

    /// Bearer token read from `path` on every request (rotated projected
    /// tokens; the file is tiny and the control plane is low-frequency).
    pub fn with_token_file(
        base: impl Into<String>,
        volume: impl Into<String>,
        path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            client: client(),
            base: base.into(),
            volume: volume.into(),
            token: TokenSource::File(path.into()),
            epoch: None,
            features: std::sync::Arc::new(FEATURES_UNKNOWN.into()),
            mixed_apply: crate::MixedApply::Auto,
        }
    }

    /// Stamp every mutating request with the stage's lease epoch
    /// (Invariant F): the fence token `NodeStage`'s serve granted.
    pub fn with_epoch(mut self, epoch: Option<u64>) -> Self {
        self.epoch = epoch;
        self
    }

    /// Override the features header's say on mixed batches
    /// ([`crate::MixedApply`]; `Auto` by default).
    pub fn with_mixed_apply(mut self, mode: crate::MixedApply) -> Self {
        self.mixed_apply = mode;
        self
    }

    fn url(&self, tail: &str) -> String {
        format!("{}/v1/volumes/{}/{tail}", self.base, self.volume)
    }

    fn auth(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token {
            TokenSource::Static(t) => r.bearer_auth(t),
            // Unreadable file → send no token; the server's 401 surfaces
            // as CpError::Denied at the call site (crate has no logger).
            TokenSource::File(p) => match std::fs::read_to_string(p) {
                Ok(t) => r.bearer_auth(t.trim()),
                Err(_) => r,
            },
            TokenSource::None => r,
        }
    }

    /// Record which optional features the server advertised on `resp`
    /// (a response without the header is a server that has none), and
    /// whether this response itself advertised `apply`.
    fn observe(&self, resp: &reqwest::Response) -> bool {
        let apply = resp
            .headers()
            .get(FEATURES_HEADER)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.split(',').any(|f| f.trim() == FEATURE_APPLY));
        let state = if apply { FEATURES_APPLY } else { FEATURES_NONE };
        self.features
            .store(state, std::sync::atomic::Ordering::Relaxed);
        apply
    }

    async fn check(&self, resp: reqwest::Response) -> Result<reqwest::Response, CpError> {
        self.observe(&resp);
        Self::check_status(resp).await
    }

    /// [`Self::check`] for a response already observed.
    async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, CpError> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        // The request PATH rides in every error. A 409 that says only
        // "conflict: head CAS conflict" cannot be attributed to a call
        // site from a log line — the cycle makes half a dozen POSTs
        // that can all answer it — and the k3s smoke's restage wedge
        // burned exactly that gap: two minutes of retries whose logs
        // never said WHICH call was refused.
        let path = resp.url().path().to_string();
        let msg = resp.text().await.unwrap_or_default();
        Err(match status.as_u16() {
            404 => CpError::NotFound(format!("{path}: {msg}")),
            // Only the structured, epoch-specific refusal becomes
            // `Fenced` (Invariant F is triggered by exactly this, never
            // by transport errors or ordinary conflicts). `lease_lost`
            // parses the RAW body, before the path prefix goes on.
            409 => match lease_lost(&msg) {
                Some(f) => f,
                None => CpError::Conflict(format!("{path}: {msg}")),
            },
            _ => CpError::Other(format!("{status} {path}: {msg}")),
        })
    }

    async fn post_json<T: serde::Serialize>(
        &self,
        tail: &str,
        body: &T,
    ) -> Result<reqwest::Response, CpError> {
        let r = self.auth(self.client.post(self.url(tail))).json(body);
        self.check(r.send().await.map_err(transport)?).await
    }

    async fn get(&self, tail: &str) -> Result<reqwest::Response, CpError> {
        let r = self.auth(self.client.get(self.url(tail)));
        self.check(r.send().await.map_err(transport)?).await
    }

    /// POST /v1/certs/enroll (auth design §3.2): exchange the bearer
    /// token's verified identity for a short-lived NBD mTLS client cert.
    /// `csr_pem` proves possession of the key; the private key never
    /// leaves this process. Not volume-scoped, unlike the rest of the
    /// control-plane surface.
    pub async fn enroll_cert(&self, csr_pem: &str) -> Result<EnrolledCert, CpError> {
        let r = self
            .auth(self.client.post(format!("{}/v1/certs/enroll", self.base)))
            .json(&serde_json::json!({ "csr_pem": csr_pem }));
        self.check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// Pending on-demand snapshot requests for this volume (CSI 6h-2):
    /// names a CSI CreateSnapshot caller wants published. The embedded
    /// agent polls this, runs a cycle, aliases, and completes.
    pub async fn snapshot_requests(&self) -> Result<Vec<String>, CpError> {
        self.get("snapshot-requests")
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// Mark an on-demand snapshot request served (idempotent).
    pub async fn complete_snapshot_request(&self, name: &str) -> Result<(), CpError> {
        let r = self.auth(
            self.client
                .delete(self.url(&format!("snapshot-requests/{name}"))),
        );
        self.check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    /// Report a failed cycle attempt (D-M12): stamps every pending
    /// snapshot request of this volume with the attempt count and error
    /// so the CSI driver can surface a reason instead of an eternal
    /// not-ready. Phase is left untouched — it still names where the
    /// cycle got to.
    pub async fn report_cycle_failure(&self, attempts: u32, error: &str) -> Result<(), CpError> {
        self.post_json(
            "cycle-status",
            &serde_json::json!({ "attempts": attempts, "last_error": error }),
        )
        .await?;
        Ok(())
    }

    /// Repoint the volume's origin export (multi-node design D-M15 §3
    /// step 4): `Some("snap/g7")` moves the pointer there; `None`
    /// initializes it (current row, else the newest generation — the
    /// restage rule). Returns the ref name the origin now serves. The
    /// advance sequence calls this between its pin-new and unpin-old
    /// steps; the server additionally moves its own `origin` pin
    /// atomically with the row, so ordering can't be lost to a crash.
    pub async fn repoint_origin(&self, target: Option<&str>) -> Result<String, CpError> {
        #[derive(Deserialize)]
        struct OriginResp {
            #[serde(rename = "ref")]
            ref_name: String,
        }
        let body = match target {
            Some(r) => serde_json::json!({ "ref": r, "epoch": self.epoch }),
            None => serde_json::json!({ "epoch": self.epoch }),
        };
        let resp = self.post_json("origin", &body).await?;
        Ok(resp.json::<OriginResp>().await.map_err(transport)?.ref_name)
    }

    /// Alias published snapshot `generation` (e.g. `g7`) under `name` —
    /// the CSI-requested snapshot name becomes a full snapshot ref to
    /// the same commit. Idempotent replay is OK; a name collision on a
    /// different commit surfaces as [`CpError::Conflict`].
    pub async fn alias_snapshot(&self, generation: &str, name: &str) -> Result<(), CpError> {
        self.post_json(
            &format!("snapshots/{generation}/alias"),
            &serde_json::json!({ "name": name, "epoch": self.epoch }),
        )
        .await?;
        Ok(())
    }
}

/// Issued client cert (`enroll_cert`). Pair `cert_pem` with the local
/// key; `ca_pem` is the issuing root; `identity` is the URI SAN that NBD
/// ACLs match; renew well before `not_after` (RFC 3339).
#[derive(Clone, Debug, Deserialize)]
pub struct EnrolledCert {
    pub cert_pem: String,
    pub ca_pem: String,
    pub identity: String,
    pub not_after: String,
}

/// One row of `GET /v1/volumes` (workstation CLI surface, CSI design §5).
#[derive(Debug, Clone, Deserialize)]
pub struct VolumeSummary {
    pub name: String,
    pub size: u64,
}

/// One row of `GET /v1/volumes/{vol}/snapshots`.
#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotSummary {
    pub name: String,
    pub commit: String,
}

impl HttpControlPlane {
    /// GET /v1/volumes — not volume-scoped.
    pub async fn list_volumes(&self) -> Result<Vec<VolumeSummary>, CpError> {
        let r = self.auth(self.client.get(format!("{}/v1/volumes", self.base)));
        self.check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// GET /v1/volumes/{volume}/snapshots for this instance's volume.
    pub async fn list_snapshots(&self) -> Result<Vec<SnapshotSummary>, CpError> {
        self.get("snapshots").await?.json().await.map_err(transport)
    }

    /// POST /v1/volumes {name, from} — server fast clone (design §4.1);
    /// size 0 inherits the source snapshot's volume size. Idempotent like
    /// plain create. Requires the `clone` op on the source volume.
    pub async fn clone_volume(&self, name: &str, from: &str) -> Result<VolumeSummary, CpError> {
        let r = self
            .auth(self.client.post(format!("{}/v1/volumes", self.base)))
            .json(&serde_json::json!({ "name": name, "size": 0, "from": from }));
        self.check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }
}

fn transport(e: reqwest::Error) -> CpError {
    CpError::Other(format!("transport: {e}"))
}

/// Parse a 409 body as the server's Invariant F refusal:
/// `{"error":"lease_lost","holder":…,"epoch":…}` → `CpError::Fenced`.
/// Anything else — including unparsable bodies — is not a fence.
fn lease_lost(body: &str) -> Option<CpError> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    if v.get("error")? != "lease_lost" {
        return None;
    }
    Some(CpError::Fenced {
        holder: v.get("holder")?.as_str()?.to_string(),
        epoch: v.get("epoch")?.as_u64()?,
    })
}

/// `snap/g3` → `g3` (API paths use bare snapshot names).
fn bare(ref_name: &str) -> &str {
    ref_name.strip_prefix("snap/").unwrap_or(ref_name)
}

fn hex(h: &[u8; 32]) -> String {
    h.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
struct StatusResp {
    base_ref: Option<String>,
    op_watermark: u64,
}

#[derive(Deserialize)]
struct SnapInfo {
    name: String,
}

impl HttpControlPlane {
    /// Whether the latest response advertised the mixed-batch call.
    /// Unknown (no response yet) counts as yes: the call is tried.
    fn supports_apply(&self) -> bool {
        self.features.load(std::sync::atomic::Ordering::Relaxed) != FEATURES_NONE
    }
}

impl ControlPlane for HttpControlPlane {
    async fn repoint_origin(&self, target: &str) -> Result<(), CpError> {
        HttpControlPlane::repoint_origin(self, Some(target)).await?;
        Ok(())
    }

    async fn thin(&self, window_s: Option<u64>) -> Result<(), CpError> {
        let body = match window_s {
            Some(w) => serde_json::json!({ "epoch": self.epoch, "window_s": w }),
            None => serde_json::json!({ "epoch": self.epoch }),
        };
        self.post_json("thin", &body).await?;
        Ok(())
    }

    async fn origin_served(&self) -> Result<Option<String>, CpError> {
        #[derive(Deserialize)]
        struct OriginResp {
            served: Option<String>,
        }
        match self.get("origin").await {
            Ok(resp) => Ok(resp.json::<OriginResp>().await.map_err(transport)?.served),
            // No pointer / no generations yet — nothing served.
            Err(CpError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError> {
        #[derive(Deserialize)]
        struct R {
            snap_id: String,
        }
        let body = serde_json::json!({ "base": base_ref.map(bare), "epoch": self.epoch });
        let r: R = self
            .post_json("prepared", &body)
            .await?
            .json()
            .await
            .map_err(transport)?;
        Ok(r.snap_id)
    }

    async fn find_prepared(&self) -> Result<Option<PreparedInfo>, CpError> {
        let ids: Vec<String> = self
            .get("prepared")
            .await?
            .json()
            .await
            .map_err(transport)?;
        let Some(id) = ids.first() else {
            return Ok(None);
        };
        let st: StatusResp = self
            .get(&format!("prepared/{id}"))
            .await?
            .json()
            .await
            .map_err(transport)?;
        Ok(Some(PreparedInfo {
            snap_id: id.clone(),
            base_ref: st.base_ref,
            op_watermark: st.op_watermark,
        }))
    }

    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        #[derive(Deserialize)]
        struct R {
            exists: Vec<bool>,
        }
        let hx: Vec<String> = hashes.iter().map(hex).collect();
        let r: R = self
            .post_json(
                &format!("prepared/{snap_id}/ensure"),
                &serde_json::json!({ "hashes": hx }),
            )
            .await?
            .json()
            .await
            .map_err(transport)?;
        Ok(r.exists)
    }

    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError> {
        let r = self
            .auth(
                self.client
                    .put(self.url(&format!("objects/{}", hex(&hash)))),
            )
            .body(data);
        self.check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        let ranges: Vec<_> = ranges
            .iter()
            .map(|(off, len, h, obj_off)| {
                serde_json::json!({ "off": off, "len": len, "hash": hex(h), "obj_off": obj_off })
            })
            .collect();
        self.post_json(
            &format!("prepared/{snap_id}/attach"),
            &serde_json::json!({ "op_id": op_id, "ranges": ranges }),
        )
        .await?;
        Ok(())
    }

    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError> {
        let ranges: Vec<_> = ranges
            .iter()
            .map(|(off, len)| serde_json::json!({ "off": off, "len": len }))
            .collect();
        self.post_json(
            &format!("prepared/{snap_id}/clear"),
            &serde_json::json!({ "op_id": op_id, "ranges": ranges }),
        )
        .await?;
        Ok(())
    }

    async fn apply(&self, snap_id: &str, op_id: u64, ops: &[RangeOp]) -> Result<(), CpError> {
        use crate::MixedApply;
        // Decided without a round trip where possible: by config, or by
        // the features the latest response advertised.
        match self.mixed_apply {
            MixedApply::Never => {
                return Err(CpError::Unsupported(
                    "mixed batches disabled by config".into(),
                ));
            }
            MixedApply::Auto if !self.supports_apply() => {
                return Err(CpError::Unsupported(format!(
                    "no {FEATURES_HEADER} header listing {FEATURE_APPLY} on the server's \
                     responses (an older server, or a proxy stripping the header)"
                )));
            }
            _ => {}
        }
        let ops: Vec<_> = ops
            .iter()
            .map(|op| match *op {
                RangeOp::Attach {
                    off,
                    len,
                    hash,
                    obj_off,
                } => serde_json::json!({
                    "op": "attach", "off": off, "len": len,
                    "hash": hex(&hash), "obj_off": obj_off,
                }),
                RangeOp::Clear { off, len } => {
                    serde_json::json!({ "op": "clear", "off": off, "len": len })
                }
            })
            .collect();
        let r = self
            .auth(
                self.client
                    .post(self.url(&format!("prepared/{snap_id}/apply"))),
            )
            .json(&serde_json::json!({ "op_id": op_id, "ops": ops }));
        let resp = r.send().await.map_err(transport)?;
        // Decided by THIS response alone (not the shared state another
        // clone's call may have just updated). A refusal without the
        // header did not come from a server that has the call: an older
        // replica, or a proxy answering for it (a 502/503/504 included,
        // which may follow a server that staged the batch — hence "not
        // known to be applied"; the caller's split re-applies the same
        // ops in order, idempotently).
        // This holds under `always` too: it skips only the check before
        // the send. Splitting is always safe, and a real refusal (a lost
        // session, a fence) comes back again from the split call, so
        // `always` costs at most one round trip against a server without
        // the call instead of failing every cycle with a mixed batch.
        let advertised = self.observe(&resp);
        let status = resp.status();
        if !status.is_success() && !advertised {
            return Err(CpError::Unsupported(format!(
                "{status} {} without the {FEATURES_HEADER} header (an older server or \
                 replica, or a proxy's own response)",
                resp.url().path()
            )));
        }
        Self::check_status(resp).await?;
        Ok(())
    }

    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError> {
        self.post_json(
            &format!("prepared/{snap_id}/checkpoint"),
            &serde_json::json!({ "op_id": op_id }),
        )
        .await?;
        Ok(())
    }

    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError> {
        self.post_json(
            &format!("prepared/{snap_id}/publish"),
            &serde_json::json!({ "name": name, "holder": publisher, "epoch": self.epoch }),
        )
        .await?;
        Ok(())
    }

    async fn abort(&self, snap_id: &str) -> Result<(), CpError> {
        // Bodyless DELETE: the stage epoch rides as a query parameter,
        // like the unpin route.
        let tail = match self.epoch {
            Some(e) => format!("prepared/{snap_id}?epoch={e}"),
            None => format!("prepared/{snap_id}"),
        };
        let r = self.auth(self.client.delete(self.url(&tail)));
        self.check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError> {
        let snaps: Vec<SnapInfo> = self
            .get("snapshots")
            .await?
            .json()
            .await
            .map_err(transport)?;
        Ok(snaps.iter().any(|s| s.name == bare(ref_name)))
    }

    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError> {
        let pins: Vec<String> = self
            .get(&format!("snapshots/{}/pins", bare(ref_name)))
            .await?
            .json()
            .await
            .map_err(transport)?;
        Ok(pins.iter().any(|h| h == holder))
    }

    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.post_json(
            &format!("snapshots/{}/pins", bare(ref_name)),
            &serde_json::json!({ "holder": holder, "epoch": self.epoch }),
        )
        .await?;
        Ok(())
    }

    async fn unpin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        let tail = match self.epoch {
            Some(e) => format!("snapshots/{}/pins/{holder}?epoch={e}", bare(ref_name)),
            None => format!("snapshots/{}/pins/{holder}", bare(ref_name)),
        };
        let r = self.auth(self.client.delete(self.url(&tail)));
        self.check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError> {
        let bytes = self
            .get(&format!(
                "snapshots/{}/data?off={off}&len={len}",
                bare(ref_name)
            ))
            .await?
            .bytes()
            .await
            .map_err(transport)?;
        Ok(bytes.to_vec())
    }

    /// POST /v1/volumes/{vol}/cycle-status — D-M12 progress fan-out to
    /// every pending snapshot request. The engine ignores errors (best
    /// effort), so a server without the route degrades to the old
    /// silence rather than failing cycles.
    async fn report_progress(&self, p: CycleProgress) -> Result<(), CpError> {
        self.post_json(
            "cycle-status",
            &serde_json::json!({
                "phase": p.phase,
                "ops_done": p.ops_done,
                "ops_total": p.ops_total,
                "captures": p.captures,
            }),
        )
        .await?;
        Ok(())
    }

    /// POST /v1/volumes/{vol}/cycle-status with bindings only — no phase
    /// or progress fields, so the requests' cycle status is untouched.
    async fn report_captures(
        &self,
        captures: &[CaptureBinding],
        lost: &[String],
    ) -> Result<(), CpError> {
        self.post_json(
            "cycle-status",
            &serde_json::json!({ "captures": captures, "lost": lost }),
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A server that ACCEPTS and never answers must not park the cycle
    /// forever. This is the §8.2 partition, and it is a different shape
    /// from the one every other test models: connection-*refused* (the
    /// dead port the e2e scenarios point at) fails instantly, so a
    /// client with no timeout at all still looked healthy there. Rule 3's
    /// clock is driven by cycles FAILING — a cycle that hangs never
    /// fails, `failing_since` is never set, and the read-only watermark
    /// can never fire.
    ///
    /// Runs with 250 ms dials rather than the production 60 s, so this
    /// asserts the timeouts are WIRED; the constants themselves are a
    /// separate judgement.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_silent_server_does_not_hang_the_cycle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Hold every accepted connection open and say nothing. Dropping
        // them would close the socket, and an EOF is not the failure
        // under test.
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((sock, _)) = listener.accept().await {
                held.push(sock);
            }
        });

        let short = std::time::Duration::from_millis(250);
        let mut cp = HttpControlPlane::new(format!("http://{addr}"), "lin", None);
        cp.client = client_with(short, short);

        let started = std::time::Instant::now();
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            ControlPlane::origin_served(&cp),
        )
        .await
        .expect("request never returned — the client has no read timeout");

        match outcome {
            // The whole point: a plain transport failure, which the
            // engine counts as a failing cycle. `Fenced` would be
            // terminal and would NOT start rule 3's clock.
            Err(CpError::Other(_)) => {}
            other => panic!("expected a transport error, got {other:?}"),
        }
        assert!(
            started.elapsed() < std::time::Duration::from_secs(5),
            "gave up, but far too late: {:?}",
            started.elapsed()
        );
    }

    /// Only the structured `lease_lost` body parses as the fence; every
    /// other 409 body — valid JSON or not — is an ordinary conflict.
    #[test]
    fn only_the_structured_lease_lost_body_is_a_fence() {
        match lease_lost(r#"{"error":"lease_lost","holder":"node-b","epoch":3}"#) {
            Some(CpError::Fenced { holder, epoch }) => {
                assert_eq!((holder.as_str(), epoch), ("node-b", 3))
            }
            other => panic!("expected Fenced, got {other:?}"),
        }
        assert!(lease_lost(r#"{"error":"snapshot already exists: x"}"#).is_none());
        assert!(
            lease_lost(r#"{"error":"lease_lost"}"#).is_none(),
            "no holder/epoch"
        );
        assert!(lease_lost("not json at all").is_none());
        assert!(lease_lost("").is_none());
    }

    /// End to end through `check`: a 409 whose body is the structured
    /// refusal maps to the terminal `CpError::Fenced`; a plain 409 stays
    /// the retryable `CpError::Conflict`.
    #[tokio::test(flavor = "multi_thread")]
    async fn http_409s_map_by_body_shape() {
        use axum::http::StatusCode;
        use axum::routing::post;
        let app = axum::Router::new()
            .route(
                "/v1/volumes/lin/snapshots/{name}/pins",
                post(|| async { (StatusCode::CONFLICT, "plain conflict body") }),
            )
            .route(
                "/v1/volumes/lin/snapshots/{from}/alias",
                post(|| async {
                    (
                        StatusCode::CONFLICT,
                        r#"{"error":"lease_lost","holder":"node-b","epoch":3}"#,
                    )
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let cp = HttpControlPlane::new(base, "lin", None).with_epoch(Some(2));
        match cp.pin("snap/g0", "holder").await {
            Err(CpError::Conflict(body)) => assert!(body.contains("plain conflict"), "{body}"),
            other => panic!("plain 409 must stay Conflict, got {other:?}"),
        }
        match cp.alias_snapshot("g0", "csi-snap").await {
            Err(CpError::Fenced { holder, epoch }) => {
                assert_eq!((holder.as_str(), epoch), ("node-b", 3))
            }
            other => panic!("structured 409 must be Fenced, got {other:?}"),
        }
    }
}

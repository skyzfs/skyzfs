//! SkyNBD control-plane client (design §12.1, auth design §9): volume
//! lifecycle for the controller service, cert enrollment for the node
//! service. The bearer token is re-read from its file on every request —
//! projected ServiceAccount tokens rotate in place (auth design §4).

use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum CpError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("unauthorized: {0}")]
    Denied(String),
    #[error("{0}")]
    Other(String),
}

impl CpError {
    fn from_status(status: reqwest::StatusCode, msg: String) -> Self {
        match status.as_u16() {
            400 => CpError::BadRequest(msg),
            404 => CpError::NotFound(msg),
            409 => CpError::Conflict(msg),
            401 | 403 => CpError::Denied(msg),
            _ => CpError::Other(format!("{status}: {msg}")),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct VolumeInfo {
    pub name: String,
    pub size: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotInfo {
    pub name: String,
    pub commit: String,
}

/// Cycle status of a snapshot request (D-M12). Field-for-field the
/// server's `SnapRequestStatus`; `ops_total == 0` means the backfill delta
/// isn't counted yet.
#[derive(Debug, Clone, Deserialize)]
pub struct RequestStatus {
    pub phase: String,
    pub ops_done: u64,
    pub ops_total: u64,
    pub attempts: u32,
    pub last_error: Option<String>,
    /// Unix ms of the capture this request is bound to; `None` before
    /// the agent has cut it.
    #[serde(default)]
    pub capture_time_ms: Option<u64>,
    /// Aliased and published; the status is retained for replays.
    #[serde(default)]
    pub completed: bool,
    /// The request's captured cut was discarded before publication. It
    /// will never be served under this name: the filer must delete the
    /// request (or the snapshot) and file again.
    #[serde(default)]
    pub capture_lost: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EnrolledCert {
    pub cert_pem: String,
    pub ca_pem: String,
    pub identity: String,
    pub not_after: String,
}

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token_path: Option<PathBuf>,
}

impl Client {
    pub fn new(base: impl Into<String>, token_path: Option<PathBuf>) -> Self {
        Self {
            // Same constructor as the agent's client, deliberately NOT
            // the same dials. This client had no timeout at all, which is
            // what made `await_final_publish`'s deadline unreachable and
            // left NodeStage holding the node-wide device lock across
            // `init_origin` forever. But the agent's 60 s read timeout is
            // sized for its upload path — a whole `chunk_bytes` object
            // going out — and nothing here uploads anything: every call
            // on this client is a small JSON RPC.
            //
            // So the number is chosen for THIS shape. It is what an
            // unrelated volume's stage waits behind when the control
            // plane goes silent, since `init_origin` runs under that
            // lock; 20 s tolerates a slow-but-alive server with room to
            // spare, and leaves the kubelet's own retry cadence — not
            // this constant — deciding how fast a stage is reattempted.
            http: crate::control_plane_client_with(
                std::time::Duration::from_secs(5),
                std::time::Duration::from_secs(20),
            ),
            base: base.into().trim_end_matches('/').to_string(),
            token_path,
        }
    }

    fn auth(&self, r: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.token_path {
            // Unreadable file → no token; the server's 401 tells the story.
            Some(p) => match std::fs::read_to_string(p) {
                Ok(t) => r.bearer_auth(t.trim()),
                Err(_) => r,
            },
            None => r,
        }
    }

    async fn check(resp: reqwest::Response) -> Result<reqwest::Response, CpError> {
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let msg = resp.text().await.unwrap_or_default();
        Err(CpError::from_status(status, msg))
    }

    /// POST /v1/volumes — idempotent on the server (identical replay →
    /// 200, size mismatch → 409). With `from = "<vol>/<snapshot>"` the
    /// volume is a server fast clone of that snapshot (design §4.1);
    /// size 0 then inherits the source size, and a size below it is 400.
    pub async fn create_volume(
        &self,
        name: &str,
        size: u64,
        from: Option<&str>,
    ) -> Result<VolumeInfo, CpError> {
        let mut body = serde_json::json!({ "name": name, "size": size });
        if let Some(from) = from {
            body["from"] = from.into();
        }
        let r = self
            .auth(self.http.post(format!("{}/v1/volumes", self.base)))
            .json(&body);
        Self::check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// POST /v1/volumes with `kind: "lineage"` (CSI 6h-2): a thin
    /// volume's ingestion target — no writable head, snapshots only.
    /// Idempotent like plain create.
    pub async fn create_lineage_volume(
        &self,
        name: &str,
        size: u64,
    ) -> Result<VolumeInfo, CpError> {
        let r = self
            .auth(self.http.post(format!("{}/v1/volumes", self.base)))
            .json(&serde_json::json!({ "name": name, "size": size, "kind": "lineage" }));
        Self::check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// A volume's size if it exists (GET /v1/volumes + search — the API
    /// has no per-volume GET). Doubles as the thin marker: a thin CSI
    /// volume always has its `<vol>-lin` lineage sibling.
    pub async fn volume_size(&self, name: &str) -> Result<Option<u64>, CpError> {
        let r = self.auth(self.http.get(format!("{}/v1/volumes", self.base)));
        let vols: Vec<VolumeInfo> = Self::check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)?;
        Ok(vols.iter().find(|v| v.name == name).map(|v| v.size))
    }

    /// GET /v1/volumes/{vol}/snapshots.
    pub async fn list_snapshots(&self, vol: &str) -> Result<Vec<SnapshotInfo>, CpError> {
        let r = self.auth(
            self.http
                .get(format!("{}/v1/volumes/{vol}/snapshots", self.base)),
        );
        Self::check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// POST /v1/volumes/{vol}/snapshot-requests — ask the volume's
    /// embedded agent for an on-demand fs-consistent snapshot (6h-2).
    /// Idempotent; readiness is "the named snapshot exists".
    pub async fn request_snapshot(&self, vol: &str, name: &str) -> Result<(), CpError> {
        let r = self
            .auth(
                self.http
                    .post(format!("{}/v1/volumes/{vol}/snapshot-requests", self.base)),
            )
            .json(&serde_json::json!({ "name": name }));
        Self::check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    /// GET /v1/volumes/{vol}/snapshot-requests/{name} — the request's
    /// cycle status (D-M12): whether this snapshot's capture has occurred.
    /// `None` covers both "no such request" and a server without the route;
    /// callers that need a capture proof must retry rather than expose a
    /// handle early.
    pub async fn snapshot_request_status(
        &self,
        vol: &str,
        name: &str,
    ) -> Result<Option<RequestStatus>, CpError> {
        let r = self.auth(self.http.get(format!(
            "{}/v1/volumes/{vol}/snapshot-requests/{name}",
            self.base
        )));
        match Self::check(r.send().await.map_err(transport)?).await {
            Ok(resp) => Ok(Some(resp.json().await.map_err(transport)?)),
            Err(CpError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// POST /v1/volumes/{vol}/serve — the D-M6 receiving side: the
    /// server picks the volume up from the shared catalog, acquiring
    /// its lease. 404 = not cataloged; 409 = leased elsewhere.
    ///
    /// With `holder = Some(node_id)` (Invariant F, task 15h) the lease
    /// is acquired under the NODE's identity, and the returned value is
    /// the granted epoch — the stage's fence token, stamped into every
    /// agent transaction. `None` epoch = a pre-15h server (or a
    /// lease-less deployment): unfenced legacy behavior.
    pub async fn serve_volume(
        &self,
        name: &str,
        holder: Option<&str>,
    ) -> Result<Option<u64>, CpError> {
        let r = self.auth(
            self.http
                .post(format!("{}/v1/volumes/{name}/serve", self.base)),
        );
        let r = match holder {
            Some(h) => r.json(&serde_json::json!({ "holder": h })),
            None => r,
        };
        let resp = Self::check(r.send().await.map_err(transport)?).await?;
        let v: serde_json::Value = resp.json().await.map_err(transport)?;
        Ok(v["epoch"].as_u64())
    }

    /// POST /v1/volumes/{vol}/release — the D-M6 handoff: commit, stop
    /// serving, release the lease. Idempotent.
    pub async fn release_volume(&self, name: &str) -> Result<(), CpError> {
        let r = self.auth(
            self.http
                .post(format!("{}/v1/volumes/{name}/release", self.base)),
        );
        Self::check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    /// POST /v1/volumes/{vol}/origin with an empty body — initialize
    /// the volume's origin pointer (D-M15 15a): adopt the current
    /// pointer row if live, else the newest published generation.
    /// Idempotent. Returns the `snap/gN` ref the pointer now names;
    /// `None` when the volume has no published generations yet (404).
    pub async fn init_origin(&self, vol: &str) -> Result<Option<String>, CpError> {
        let r = self
            .auth(
                self.http
                    .post(format!("{}/v1/volumes/{vol}/origin", self.base)),
            )
            .json(&serde_json::json!({}));
        match Self::check(r.send().await.map_err(transport)?).await {
            Ok(resp) => Ok(Some(origin_ref_field(resp).await?)),
            Err(CpError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// GET /v1/volumes/{vol}/origin — the `snap/gN` ref the origin
    /// pointer names, but only when the pointer row itself is
    /// initialized (`initialized: true`); an uninitialized answer is
    /// merely the fallback an initializing POST would adopt. 404 (no
    /// generations) and initialized=false both map to `None`.
    pub async fn origin_ref(&self, vol: &str) -> Result<Option<String>, CpError> {
        let r = self.auth(
            self.http
                .get(format!("{}/v1/volumes/{vol}/origin", self.base)),
        );
        match Self::check(r.send().await.map_err(transport)?).await {
            Ok(resp) => {
                let v: serde_json::Value = resp.json().await.map_err(transport)?;
                if !v["initialized"].as_bool().unwrap_or(false) {
                    return Ok(None);
                }
                match v["ref"].as_str() {
                    Some(r) => Ok(Some(r.to_string())),
                    None => Err(CpError::Other("origin response missing \"ref\"".into())),
                }
            }
            Err(CpError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// DELETE /v1/volumes/{name} — idempotent on the server (204 even if
    /// absent; config-defined volumes are 409).
    pub async fn delete_volume(&self, name: &str) -> Result<(), CpError> {
        let r = self.auth(self.http.delete(format!("{}/v1/volumes/{name}", self.base)));
        Self::check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    /// POST /v1/volumes/{vol}/snapshots — 201 with {name, commit};
    /// duplicate name → 409 (same name = same snapshot, so callers may
    /// treat that as idempotent success).
    pub async fn create_snapshot(&self, vol: &str, name: &str) -> Result<SnapshotInfo, CpError> {
        let r = self
            .auth(
                self.http
                    .post(format!("{}/v1/volumes/{vol}/snapshots", self.base)),
            )
            .json(&serde_json::json!({ "name": name }));
        Self::check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }

    /// DELETE /v1/volumes/{vol}/snapshots/{name} — 404 when absent
    /// (callers treat that as idempotent success).
    pub async fn delete_snapshot(&self, vol: &str, name: &str) -> Result<(), CpError> {
        let r = self.auth(
            self.http
                .delete(format!("{}/v1/volumes/{vol}/snapshots/{name}", self.base)),
        );
        Self::check(r.send().await.map_err(transport)?).await?;
        Ok(())
    }

    /// POST /v1/certs/enroll (auth design §3.2): CSR in, short-lived NBD
    /// mTLS client cert out.
    pub async fn enroll(&self, csr_pem: &str) -> Result<EnrolledCert, CpError> {
        let r = self
            .auth(self.http.post(format!("{}/v1/certs/enroll", self.base)))
            .json(&serde_json::json!({ "csr_pem": csr_pem }));
        Self::check(r.send().await.map_err(transport)?)
            .await?
            .json()
            .await
            .map_err(transport)
    }
}

fn transport(e: reqwest::Error) -> CpError {
    CpError::Other(format!("transport: {e}"))
}

/// The `"ref"` field of an origin-route response body.
async fn origin_ref_field(resp: reqwest::Response) -> Result<String, CpError> {
    let v: serde_json::Value = resp.json().await.map_err(transport)?;
    match v["ref"].as_str() {
        Some(r) => Ok(r.to_string()),
        None => Err(CpError::Other("origin response missing \"ref\"".into())),
    }
}

//! `ControlPlane`: the agent's view of the server's §9A ingestion API
//! (agent design doc §3.3). Implementations: [`crate::inproc`] (wired
//! straight to `skynbd-engine`'s `PreparedSession` — the fake IS the real
//! server logic) and, in a later M8 slice, an HTTP client.

pub use skynbd_engine::prepared::RangeOp;

#[derive(Debug, thiserror::Error)]
pub enum CpError {
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("not found: {0}")]
    NotFound(String),
    /// The Invariant F refusal (task 15h): the server's epoch-specific
    /// 409 `lease_lost` — this stage's epoch is superseded and the
    /// volume is owned by `holder` at `epoch` now. Terminal for the
    /// cycle: abandon, never retry. Only ever produced from the
    /// authenticated, structured refusal — transport errors, 5xx and
    /// ordinary conflicts stay their retryable selves.
    #[error("fenced: volume owned by {holder:?} at epoch {epoch} (Invariant F)")]
    Fenced { holder: String, epoch: u64 },
    #[error("transport/server: {0}")]
    Other(String),
    /// The server did not take this call: it does not advertise it, or
    /// refused it without the features header (an older server or
    /// replica, or a proxy's own error response), or it is disabled by
    /// config. Not known to be applied — a proxy's 5xx may follow a
    /// server that staged it — so the caller falls back to an
    /// equivalent that re-applies the same ops, in order, idempotently.
    #[error("not taken: {0}")]
    Unsupported(String),
}

/// Whether the agent sends a mixed attach/clear batch as one `apply`
/// call (config `mixedApply`, CSI `SKYNBD_MIXED_APPLY`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MixedApply {
    /// As the server's `x-skynbd-features` header says (the default).
    #[default]
    Auto,
    /// Whatever the header says: for a proxy that strips the header in
    /// front of a server that has the call. A refusal without the header
    /// still splits the rest of the cycle, as under `Auto`, so a server
    /// without the call (an old replica, a rollback) costs one round
    /// trip per cycle rather than failing it.
    Always,
    /// Never: every mixed batch is split into same-kind runs.
    Never,
}

impl std::str::FromStr for MixedApply {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!(
                "mixed apply mode {s:?}: expected auto, always or never"
            )),
        }
    }
}

/// A live prepared root on the server (one in-flight per volume, V3.2 §7.9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedInfo {
    pub snap_id: String,
    pub base_ref: Option<String>,
    /// Durable idempotency watermark (server design §9A.6).
    pub op_watermark: u64,
}

/// Async by nature (network in production, engine I/O in-process). Used
/// generically by the cycle engine — no `dyn`, so plain AFIT is fine.
#[allow(async_fn_in_trait)]
pub trait ControlPlane {
    /// `PrepareSnapshotFromBase`. `base_ref` is a visible ref name
    /// (`snap/gN`) or `None` for bootstrap. Returns the new snap id.
    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError>;
    /// The volume's single in-flight prepared root, if any.
    async fn find_prepared(&self) -> Result<Option<PreparedInfo>, CpError>;
    /// `EnsureObject`: which hashes already exist (recorded as GC promises).
    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError>;
    /// `PutObject` (CAS, idempotent).
    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError>;
    /// `AttachRange` batch: `(vol_off, len, hash, obj_off)`, idempotent by
    /// `op_id` against the durable watermark.
    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError>;
    /// `ClearRange` batch: `(vol_off, len)`.
    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError>;
    /// A mixed attach/clear batch under ONE op id, staged in order
    /// (issue #97 follow-up). [`CpError::Unsupported`] from a server
    /// without the call; the caller then splits the batch into
    /// same-kind runs instead.
    async fn apply(&self, snap_id: &str, op_id: u64, ops: &[RangeOp]) -> Result<(), CpError>;
    /// Force a durable checkpoint at `op_id` (pre-publish barrier).
    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError>;
    /// `PublishSnapshot` under `name` (ref becomes `snap/{name}`), pinned
    /// for `publisher`. Idempotent from the agent's viewpoint via
    /// [`ControlPlane::ref_exists`] before retry.
    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError>;
    /// `AbortSnapshot`.
    async fn abort(&self, snap_id: &str) -> Result<(), CpError>;
    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError>;
    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError>;
    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError>;
    async fn unpin(&self, ref_name: &str, holder: &str) -> Result<(), CpError>;
    /// Read published snapshot content (verification path).
    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError>;
    /// Report cycle progress for whoever is waiting on this volume's
    /// pending snapshot requests (multi-node design D-M12). Best-effort
    /// observability: callers ignore the result, and the default no-op
    /// keeps every non-HTTP implementation (and test double) unchanged.
    async fn report_progress(&self, _progress: CycleProgress) -> Result<(), CpError> {
        Ok(())
    }
    /// Report request bindings outside a cycle (D-M12): `captures` as in
    /// [`CycleProgress::captures`], and `lost` — requests whose captured
    /// candidate was discarded unpublished, which the control plane must
    /// fail rather than offer for a new capture. Unlike progress, the
    /// result matters: the agent forgets a lost request only once this
    /// succeeds.
    async fn report_captures(
        &self,
        _captures: &[CaptureBinding],
        _lost: &[String],
    ) -> Result<(), CpError> {
        Ok(())
    }
    /// D-M15 §3 step 4: move the origin pointer to `target` (a published
    /// `snap/gN` ref) — server-side row write + live export swap. The
    /// default fails closed so an advance against a plane that cannot
    /// repoint errors loudly instead of silently skipping the step.
    async fn repoint_origin(&self, target: &str) -> Result<(), CpError> {
        Err(CpError::Other(format!(
            "origin repoint unsupported by this control plane ({target})"
        )))
    }
    /// The generation the origin export is *serving* right now — the
    /// Invariant E fence read (end-to-end, not a refs re-read). `None` =
    /// no origin export; the fence fails on it.
    async fn origin_served(&self) -> Result<Option<String>, CpError> {
        Ok(None)
    }
    /// Retention thinning (design §6.2, task 15f): one server-side pass
    /// under this volume's fence epoch — the node-held driver, since
    /// the server's own cadence is refused on volumes this node leases.
    /// Best-effort like [`ControlPlane::report_progress`]: a missed
    /// pass costs bytes, not correctness, and the next advance retries.
    async fn thin(&self, _window_s: Option<u64>) -> Result<(), CpError> {
        Ok(())
    }
}

/// One cycle progress sample (D-M12). `ops_total == 0` means the backfill
/// delta isn't computed yet; phases mirror [`crate::state::Phase`] in
/// lowercase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleProgress {
    pub phase: &'static str,
    pub ops_done: u64,
    pub ops_total: u64,
    /// The agent's durable request bindings at the time of the report:
    /// every request whose capture has been cut, with that cut's time.
    /// Re-sent on every report so a control-plane restart re-learns them.
    pub captures: Vec<CaptureBinding>,
}

/// A snapshot request bound to a completed local capture (D-M12): the
/// handle-safe point in time a CSI caller is told about.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CaptureBinding {
    pub name: String,
    /// Unix ms of the capture this request is bound to.
    pub capture_time_ms: u64,
}

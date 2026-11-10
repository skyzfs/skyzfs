//! Node plugin mechanics (auth design §9.3): NodeStage attaches the
//! volume's `<name>/head` export on a free kernel NBD device via
//! nbd-client — over forced-TLS mTLS when a creds dir is configured
//! (nbd-client keeps a userspace GnuTLS proxy process alive; the kernel
//! never sees TLS), or over a node-local Unix socket when the server
//! runs beside us ([`NbdEndpoint`]) — then formats/mounts for Mount
//! volumes. NodePublish bind-mounts into the pod. Device↔volume
//! bindings are recorded as state files so unstage after a plugin
//! restart still finds its device.
//!
//! `ro:<vol>/snap/<name>` reference volumes (CSI design §4) attach the
//! snapshot's existing read-only export instead: never formatted, always
//! mounted read-only.
//!
//! `mode: thin` volumes (CSI design 6h, origin-advance §5) are
//! publish-only: NO head attach at all — the stage's only NBD client
//! is the RO origin attach (when the lineage has published history),
//! and durability is the embedded publish cycle. The pool is entirely
//! local via [`crate::thinstack::ThinStack`]: a loop (direct-IO when
//! supported) over a per-volume fallocate-preallocated file carved
//! `[tmeta][data]` — reservation up front turns backing-fs ENOSPC into
//! a clean stage-time failure. The filesystem lives on the linear shim
//! over the thin origin (D-M15 — the fs claim lands on the shim, so
//! the origin stays repointable and reclaimable). A pool file that
//! *survives* restage carries the volume's structure through reboots
//! (dm reads its own metadata back); a file that is *lost* re-stages
//! fresh from the origin with a loud warning — what is lost is exactly
//! the unpublished delta, bounded by the publish cadence.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::agentcycle::{CycleRunner, CycleSpec};
use crate::hostcmd::Runner;
use crate::provision::ThinStackSpec;
use crate::thinstack::ThinStack;

/// Where this node's NBD server is, and how the data path reaches it.
///
/// A **Unix** endpoint (CSI design §2.1, server design §15 M10a) is the
/// node-local topology: a skynbdd DaemonSet on the same host, reached
/// through a socket on a hostPath. It takes the TCP stack out of the
/// data path and replaces cert-based identity with the socket's file
/// mode — so mTLS enrollment does not apply, which is why the creds
/// live inside the TCP variant rather than beside both.
#[derive(Clone, Debug)]
pub enum NbdEndpoint {
    Tcp {
        host: String,
        port: u16,
        /// TLS hostname for cert verification; defaults to `host`.
        tls_hostname: Option<String>,
        /// Creds dir with client-cert/client-key/ca-cert PEMs (from
        /// enrollment). `None` = plaintext NBD (dev only).
        creds_dir: Option<PathBuf>,
    },
    Unix(PathBuf),
}

impl std::fmt::Display for NbdEndpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NbdEndpoint::Tcp { host, port, .. } => write!(f, "{host}:{port}"),
            NbdEndpoint::Unix(path) => write!(f, "unix:{}", path.display()),
        }
    }
}

#[derive(Clone)]
pub struct NodeConfig {
    pub endpoint: NbdEndpoint,
    /// `/sys/block` (overridable in tests).
    pub sysfs_block: PathBuf,
    /// Where `<volume>.json` device bindings live.
    pub state_dir: PathBuf,
    /// Filesystem when the volume capability doesn't name one.
    pub default_fstype: String,
    /// Where per-volume cache files live (thin mode). `None` = thin
    /// staging refused.
    pub cache_dir: Option<PathBuf>,
    /// Size of each volume's cache file.
    pub cache_bytes: u64,
    /// Thin chunk == cache block (V3.2 §8.2 preferred alignment).
    pub chunk_bytes: u64,
    /// §7.1: where per-volume READ-cache files live. `None` = no read
    /// cache — the filesystem mounts the shim, the pre-§7.1 shape. Its
    /// own directory (ideally its own filesystem/flash) rather than a
    /// carve of `cache_dir`'s budget, per the spec's own-device
    /// rationale; a per-volume file looped like the pool's, because
    /// `ThinStackSpec.read_cache_dev` needs a per-volume device and
    /// the loop is how this node turns files into those.
    pub read_cache_dir: Option<PathBuf>,
    /// Size of each volume's read-cache file (carved rcmeta+rcdata by
    /// `read_cache_layout`). Ignored without `read_cache_dir`.
    pub read_cache_bytes: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("no free NBD device (is the nbd module loaded with enough devices?)")]
    NoFreeDevice,
    #[error("{cmd} failed: {detail}")]
    Command { cmd: String, detail: String },
    #[error("io {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("volume {0} is not staged here")]
    NotStaged(String),
    #[error("thin mode is not configured on this node (set the cache dir)")]
    ThinUnavailable,
    #[error("thin stack: {0}")]
    Stack(String),
    #[error(
        "volume {0}: final publish cycle incomplete — {1}. The unstage \
         will be retried and the resumable cycle continues the upload; \
         teardown is refused until the delta is published (D-M13)"
    )]
    FinalCycle(String, String),
    #[error(
        "volume {0}: external-origin provisioning failed — {1}. Staging \
         a fresh pool without its origin would present a volume with \
         published history as a blank disk, so the stage is refused \
         (D-M11); kubelet retries"
    )]
    Origin(String, String),
    #[error(
        "volume {0}: lineage serve failed — {1}. The stage is refused \
         rather than degrading to an epoch-less (unfenced) stage \
         (Invariant F, task 15h); kubelet retries"
    )]
    LineageServe(String, String),
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StageState {
    /// The head NBD attach — raw and reference volumes only. Thin
    /// volumes have NO writable network path (origin-advance §5): the
    /// pool is local, publish is the durability step, and the only NBD
    /// surface is the read-only origin attach in [`ThinState`].
    #[serde(default)]
    device: Option<String>,
    /// Thin-mode extras; absent for raw and reference volumes (and for
    /// state files written before thin mode existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thin: Option<ThinState>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct ThinState {
    loop_dev: String,
    cache_file: PathBuf,
    size_bytes: u64,
    /// Lineage volume for the embedded agent cycle (6h-2); absent on
    /// state files from before it existed (their cycle starts on the
    /// next fresh stage) and on block volumes.
    #[serde(default)]
    lineage: Option<String>,
    /// D-M15 origin attach: the lineage volume whose repointable
    /// `<lineage>/origin` export backs the thin device's unprovisioned
    /// reads, and the NBD device it is attached on for this stage. The
    /// pointer row *on the server* is the durable marker (it outlives
    /// unstage and local state); these fields are per-stage bookkeeping
    /// for teardown.
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    origin_dev: Option<String>,
    /// The lineage lease epoch granted to this node at stage time
    /// (Invariant F, task 15h) — the stage's fence token, threaded into
    /// the embedded cycle so every agent transaction carries it. Absent
    /// on pre-15h state files and lease-less deployments.
    #[serde(default)]
    epoch: Option<u64>,
    /// Per-StorageClass §7 compaction-policy dials (task 15i). Absent
    /// on pre-15i state files — those volumes run the node defaults.
    #[serde(default)]
    reclaim: crate::agentcycle::ReclaimParams,
    /// §7.1: the loop device over this volume's read-cache file, when
    /// the node is configured with one. Per-stage bookkeeping like
    /// `loop_dev`; the warmth lives in the FILE (`rcache_file`), which
    /// outlives the loop. Absent on pre-§7.1 state files and cache-less
    /// nodes.
    #[serde(default)]
    rcache_loop: Option<String>,
    #[serde(default)]
    rcache_file: Option<PathBuf>,
}

/// Thin-mode staging parameters from the PV's volume context.
#[derive(Debug, Clone)]
pub struct ThinParams {
    pub size_bytes: u64,
    /// Lineage volume (publication target for the embedded cycle);
    /// `None` disables the cycle for this volume.
    pub lineage: Option<String>,
    /// §7 compaction-policy dials (task 15i), overriding the node
    /// defaults for this volume's cycle.
    pub reclaim: crate::agentcycle::ReclaimParams,
}

/// NBD export + read-only flag for a CSI volume id. Reference volumes
/// (`ro:` prefix, CSI design §4) carry their export name verbatim; real
/// volumes export `<id>/head`.
fn export_for(volume: &str) -> (String, bool) {
    match volume.strip_prefix("ro:") {
        Some(export) => (export.to_string(), true),
        None => (format!("{volume}/head"), false),
    }
}

/// Read-only mount options per filesystem. Crash-consistent snapshot
/// images can hold a dirty journal, and replay is impossible on a
/// read-only NBD device — ext4's `noload` / xfs's `norecovery` skip it.
fn ro_mount_opts(fstype: &str) -> &'static str {
    match fstype {
        "ext2" | "ext3" | "ext4" => "ro,noload",
        "xfs" => "ro,norecovery",
        _ => "ro",
    }
}

/// Invariant F: move a superseded pool's cache file aside
/// (`<name>.fenced-e<epoch>`, `-eunknown` when the epoch is unknown —
/// epoch 0 or none recorded). Rename, not unlink: a misclassification
/// stays recoverable until the file is swept. A failed rename must fail
/// the caller — leaving a superseded pool under its live name would
/// resurrect it on the next stage.
///
/// Two halves, both from #76's review. (a) A quarantine never
/// OVERWRITES an earlier one: `rename` replaces its destination on
/// Unix and the earlier file may be the only copy of that pool. (b) A
/// collision is expected, not exceptional — any SAGT version bump
/// makes every surviving record decode Corrupt, so #76's stage
/// validation quarantines every pool on the node as `-eunknown`, and
/// the previous bump's leftover holds that name. So collisions
/// uniquify (`.2`, `.3`, …) rather than refuse: a bump costs a suffix
/// instead of wedging every stage behind operator intervention. Only
/// an exhausted suffix space (100) fails the caller.
fn quarantine_cache(cache_file: &Path, epoch: Option<u64>) -> Result<PathBuf, NodeError> {
    const MAX_QUARANTINE_SUFFIX: u32 = 100;
    let tag = match epoch {
        Some(e) if e != 0 => format!("e{e}"),
        _ => "eunknown".to_string(),
    };
    let stem = cache_file
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("volume.cache");
    let base = format!("{stem}.fenced-{tag}");
    let mut quarantined = cache_file.with_file_name(&base);
    let mut nth = 2;
    // symlink_metadata, not `exists`: a dangling symlink squatting the
    // name is still a name that `rename` would consume.
    while quarantined.symlink_metadata().is_ok() {
        if nth > MAX_QUARANTINE_SUFFIX {
            return Err(NodeError::Io(
                quarantined,
                std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    format!(
                        "{MAX_QUARANTINE_SUFFIX} quarantines already hold this \
                         name — salvage or delete them before retrying"
                    ),
                ),
            ));
        }
        quarantined = cache_file.with_file_name(format!("{base}.{nth}"));
        nth += 1;
    }
    std::fs::rename(cache_file, &quarantined)
        .map_err(|err| NodeError::Io(cache_file.to_path_buf(), err))?;
    Ok(quarantined)
}

pub struct NodeMechanics {
    pub cfg: NodeConfig,
    pub runner: Arc<dyn Runner>,
    /// Thin-stack builder; `None` = raw-only node.
    pub thin: Option<Arc<dyn ThinStack>>,
    /// Embedded agent cycle per staged thin volume (6h-2); `None` = no
    /// continuous publication (raw-only nodes, tests).
    pub cycle: Option<Arc<dyn CycleRunner>>,
    /// D-M6 lease handoff: serve-before-attach, release-after-detach.
    /// `None` = no handoff calls (legacy single-server, tests).
    pub handoff: Option<Arc<dyn crate::handoff::Handoff>>,
    /// D-M11 external-origin provisioning; `None` disables it (fresh
    /// thin pools then start blank, the pre-D-M11 behavior).
    pub origins: Option<Arc<dyn crate::handoff::Origins>>,
    /// Serializes stage/unstage: device allocation via sysfs scan is
    /// only race-free under a single in-flight attach.
    pub lock: tokio::sync::Mutex<()>,
}

impl NodeMechanics {
    pub fn new(
        cfg: NodeConfig,
        runner: Arc<dyn Runner>,
        thin: Option<Arc<dyn ThinStack>>,
        cycle: Option<Arc<dyn CycleRunner>>,
    ) -> Self {
        Self {
            cfg,
            runner,
            thin,
            cycle,
            handoff: None,
            origins: None,
            lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn with_handoff(mut self, handoff: Arc<dyn crate::handoff::Handoff>) -> Self {
        self.handoff = Some(handoff);
        self
    }

    pub fn with_origins(mut self, origins: Arc<dyn crate::handoff::Origins>) -> Self {
        self.origins = Some(origins);
        self
    }

    /// Start the embedded cycle for a staged, mounted thin volume — a
    /// no-op without a runner, a lineage, or for block volumes (no
    /// filesystem to freeze; their snapshots stay crash-consistent).
    fn start_cycle(
        &self,
        volume: &str,
        staging_path: &str,
        t: &ThinState,
        block: bool,
        readonly: bool,
    ) -> Result<(), NodeError> {
        let (Some(cycle), Some(lineage), false) = (&self.cycle, &t.lineage, block) else {
            return Ok(());
        };
        cycle
            .start(&CycleSpec {
                volume: volume.to_string(),
                lineage: lineage.clone(),
                staging_path: staging_path.to_string(),
                size_bytes: t.size_bytes,
                // D-M15: the repointable origin export this stage
                // attached — advance cycles are only safe with one (see
                // the cycle loop's no-origin guard), and the §7 swing's
                // B pool names the same attach as its external origin.
                origin_dev: t.origin_dev.clone(),
                // Invariant F: the stage-time lease epoch every agent
                // transaction must carry.
                epoch: t.epoch,
                // §7 dials (task 15i): per-volume overrides over the
                // runner's node-level defaults.
                reclaim: t.reclaim,
                // §8.1 rule 3 (task 15j part B) is withheld from a
                // read-only stage — see `CycleSpec::readonly`.
                readonly,
                // §8.1 rule 2: the local pool file's filesystem is the
                // second exhaustion surface under the §5 layout.
                pool_file: Some(t.cache_file.clone()),
                // §7: the same `[tmeta][data]` carve the build computed
                // makes the pool file a slot — the two-pool swing's
                // precondition. Err is unreachable past a successful
                // build (same inputs), but degrading to "port-pinned"
                // beats failing a stage the build already accepted.
                pool_layout: self.thin_spec(volume, t).layout().ok(),
                // §7.1 / Invariant B: on a cached stack the per-cycle
                // mask re-assert must cover the rcache the guest mounts.
                read_cache: self.thin_spec(volume, t).read_cache(),
            })
            .map_err(NodeError::Stack)
    }

    fn state_path(&self, volume: &str) -> PathBuf {
        // Reference-volume ids contain '/'; flatten to one path segment.
        // Kubernetes-born ids (pvc-<uuid>, snapshot-<uuid>) never contain
        // '_', so the substitution cannot collide.
        self.cfg
            .state_dir
            .join(format!("{}.json", volume.replace('/', "_")))
    }

    fn state(&self, volume: &str) -> Option<StageState> {
        let raw = std::fs::read(self.state_path(volume)).ok()?;
        serde_json::from_slice(&raw).ok()
    }

    #[cfg(test)]
    pub fn staged_device(&self, volume: &str) -> Option<String> {
        self.state(volume).and_then(|s| s.device)
    }

    /// The device pods talk to: the linear shim over the thin origin
    /// for thin volumes (D-M15 — the claim must land on the shim, never
    /// the origin), the NBD device otherwise.
    fn published_device(&self, volume: &str) -> Option<String> {
        let s = self.state(volume)?;
        match s.thin {
            Some(t) => Some(self.thin_spec(volume, &t).mount_path()),
            None => s.device,
        }
    }

    /// The publish-only stack spec (origin-advance §5) for a staged
    /// (or staging) thin volume: the pool lives on the loop over the
    /// local file, and the only network reference is the raw RO origin
    /// attach.
    fn thin_spec(&self, volume: &str, t: &ThinState) -> ThinStackSpec {
        ThinStackSpec {
            volume: volume.to_string(),
            fast_dev: t.loop_dev.clone(),
            data_bytes: t.size_bytes,
            fast_bytes: self.cfg.cache_bytes,
            chunk_bytes: self.cfg.chunk_bytes,
            external_origin: t.origin_dev.clone(),
            // §7.1: the loop over this volume's read-cache file, when
            // the stage attached one (node `read_cache_dir` config).
            read_cache_dev: t.rcache_loop.clone(),
            read_cache_bytes: self.cfg.read_cache_bytes,
            // Fresh pool ⇒ cold cache (§7.1): stamped true only at the
            // fresh-restage build site, never here — every other use of
            // this spec (mount-path lookup, unwind teardown, replay)
            // must not re-zero a warm cache.
            cold_read_cache: false,
        }
    }

    /// D-M15: whether this thin stage attaches the lineage's
    /// repointable origin export, returning the LINEAGE volume name to
    /// attach (the export is always `<lineage>/origin`). Runs BEFORE
    /// the cache file is created on purpose — the pointer row and the
    /// seeded agent baseline become durable first, so a stage that
    /// crashes anywhere after this point retries into the reuse branch
    /// with a consistent (pointer, baseline) pair already in place.
    ///
    /// - Fresh cache + published generations → initialize the pointer
    ///   (idempotent server-side) and seed the agent's rebaselined
    ///   state on the same generation (origin content and baseline
    ///   must agree).
    /// - Surviving cache → attach when the pointer row is initialized
    ///   (its existence is the durable "this pool reads through an
    ///   origin" marker); no re-seed — local state is live.
    /// - No generations yet, no lineage, or no provider → none.
    async fn plan_external_origin(
        &self,
        volume: &str,
        params: &ThinParams,
        fresh_cache: bool,
        epoch: Option<u64>,
    ) -> Result<Option<String>, NodeError> {
        let Some(origins) = &self.origins else {
            return Ok(None);
        };
        let Some(lineage) = &params.lineage else {
            return Ok(None);
        };
        let err = |e: String| NodeError::Origin(volume.to_string(), e);
        if fresh_cache {
            match origins.init_origin(lineage).await.map_err(err)? {
                Some(gen) => {
                    if let Some(cycle) = &self.cycle {
                        // `gen` is already the full `snap/gN` ref name;
                        // the stage's lease epoch rides with the record.
                        cycle.seed_baseline(lineage, &gen, epoch).map_err(err)?;
                    }
                    tracing::info!(
                        volume,
                        lineage,
                        gen,
                        "fresh pool reads through the lineage origin export (D-M15)"
                    );
                    Ok(Some(lineage.clone()))
                }
                None => Ok(None),
            }
        } else if origins.origin_ref(lineage).await.map_err(err)?.is_some() {
            // Reconcile before the attach, not just read: the origin
            // EXPORT lives in the serving rig and dies with every
            // release, and historically only the POST rebuilt it (its
            // no-op arm, the pointer already being initialized, is a
            // pure export reconcile). Without this, a stage retry after
            // any attach failure looped forever against a registry
            // entry nothing repairs: the k3s smoke hit exactly that —
            // one transient negotiation failure, then ten identical
            // refusals over five minutes. The server's serve path now
            // reconciles synchronously before serve returns, which
            // retires that class structurally; this POST stays as the
            // compatibility belt for servers predating the change (it
            // is idempotent and one round trip).
            origins.init_origin(lineage).await.map_err(err)?;
            Ok(Some(lineage.clone()))
        } else {
            Ok(None)
        }
    }

    /// Invariant F, stage side: may an existing cache file be adopted,
    /// or must it be quarantined first? A persisted pool is adopted only
    /// when it VALIDATES against the authority the stage just acquired;
    /// three things fail it — a durable fenced marker, a missing or
    /// corrupt agent record (unvalidatable), and a record whose stage
    /// epoch differs from the fresh grant. Same-holder re-acquire
    /// preserves the lease epoch and any foreign hold bumps it, so
    /// epoch equality is exactly "no foreign writer has held this
    /// lineage since this pool was seeded" — and origin-pointer movement
    /// under an unbroken hold is safe (every generation between the
    /// pool's baseline and the pointer was published from this very
    /// pool, so its overlay still covers every block that changed).
    /// Without this check, a same-node restage after an intervening
    /// foreign owner would mount a stale hybrid of local mappings over
    /// the newer shared origin: canonically harmless (the epoch fence
    /// refuses its publishes) but wrong data served to the workload.
    /// Returns `Some((reason, epoch-for-the-quarantine-name))` when the
    /// pool must be quarantined; `None` = adopt.
    fn validate_persisted_pool(
        &self,
        volume: &str,
        lineage: &str,
        granted: Option<u64>,
    ) -> Option<(&'static str, Option<u64>)> {
        // Fail closed (#76 review): a lineage volume with a persisted
        // pool and no cycle runner to answer for it is the "no
        // trustworthy record" case, not a free adoption. (Production
        // wiring always pairs thin support with a cycle runner — both
        // are gated on the cache dir — so this arm guards odd rigs.)
        let Some(cycle) = self.cycle.as_ref() else {
            return Some(("no cycle runner to answer for the pool", None));
        };
        if let Some(fe) = cycle.fenced(volume) {
            return Some(("durable fenced marker", Some(fe)));
        }
        match cycle.record_epoch(lineage) {
            Some(rec) if rec == granted => None,
            Some(rec) => Some(("record epoch differs from the fresh lease grant", rec)),
            None => Some(("no trustworthy agent record", None)),
        }
    }

    /// Kernel-attached check: an NBD device with a server has a sysfs
    /// `pid` file. Distinguishes "state file from a live stage" from
    /// "state file surviving a reboot".
    fn device_attached(&self, device: &str) -> bool {
        let name = device.rsplit('/').next().unwrap_or(device);
        self.cfg.sysfs_block.join(name).join("pid").exists()
    }

    /// First `/dev/nbdX` whose sysfs node has no `pid` file (no server
    /// attached). Caller holds the stage lock.
    fn allocate_device(&self) -> Result<String, NodeError> {
        let mut idx: Vec<u64> = std::fs::read_dir(&self.cfg.sysfs_block)
            .map_err(|e| NodeError::Io(self.cfg.sysfs_block.clone(), e))?
            .filter_map(|d| {
                d.ok()?
                    .file_name()
                    .to_str()?
                    .strip_prefix("nbd")?
                    .parse()
                    .ok()
            })
            .collect();
        idx.sort_unstable();
        for i in idx {
            if !self.cfg.sysfs_block.join(format!("nbd{i}/pid")).exists() {
                return Ok(format!("/dev/nbd{i}"));
            }
        }
        Err(NodeError::NoFreeDevice)
    }

    async fn run_ok(
        &self,
        cmd: &str,
        args: Vec<String>,
    ) -> Result<crate::hostcmd::Output, NodeError> {
        let out = self
            .runner
            .run(cmd, &args)
            .await
            .map_err(|e| NodeError::Command {
                cmd: cmd.into(),
                detail: e.to_string(),
            })?;
        if out.success {
            Ok(out)
        } else {
            Err(NodeError::Command {
                cmd: format!("{cmd} {}", args.join(" ")),
                detail: format!("{}{}", out.stdout, out.stderr),
            })
        }
    }

    fn nbd_client_args(&self, export: &str, device: &str) -> Vec<String> {
        let mut args = vec!["-N".into(), export.to_string()];
        let mut creds = None;
        match &self.cfg.endpoint {
            NbdEndpoint::Tcp {
                host,
                port,
                tls_hostname,
                creds_dir,
            } => {
                args.extend([host.clone(), port.to_string()]);
                creds = creds_dir.as_ref().map(|d| {
                    (
                        d.clone(),
                        tls_hostname.clone().unwrap_or_else(|| host.clone()),
                    )
                });
            }
            // `-unix` is a flag: the socket path takes the host
            // positional's place, and the device follows it (nbd-client
            // reads a non-numeric second positional as the device).
            NbdEndpoint::Unix(path) => {
                args.extend(["-unix".to_string(), path.display().to_string()]);
            }
        }
        args.push(device.to_string());
        // Reconnect on transient server restarts instead of erroring the
        // filesystem.
        args.push("-persist".into());
        if let Some((dir, tls_hostname)) = creds {
            args.extend([
                "-enable-tls".into(),
                "-certfile".into(),
                dir.join("client-cert.pem").display().to_string(),
                "-keyfile".into(),
                dir.join("client-key.pem").display().to_string(),
                "-cacertfile".into(),
                dir.join("ca-cert.pem").display().to_string(),
                "-tlshostname".into(),
                tls_hostname,
            ]);
        }
        args
    }

    /// Attach + (for Mount volumes) format-if-blank + mount. Idempotent:
    /// an existing state file with a live attachment means a previous
    /// stage completed; a state file *without* one is the reboot path —
    /// re-attach and (thin) rebuild the stack from the surviving cache
    /// file. Reference volumes (`ro:` ids) attach the snapshot's RO
    /// export and are mounted read-only, never formatted. Thin volumes
    /// (`thin_size` set, CSI design 6h) put the head under
    /// dm-cache + thin pool and put the filesystem on the linear shim
    /// over the thin origin (D-M15).
    pub async fn stage(
        &self,
        volume: &str,
        staging_path: &str,
        fstype: Option<&str>,
        block: bool,
        thin: Option<ThinParams>,
    ) -> Result<(), NodeError> {
        let _g = self.lock.lock().await;
        if thin.is_some() && (self.thin.is_none() || self.cfg.cache_dir.is_none()) {
            return Err(NodeError::ThinUnavailable);
        }
        // Whether a prior stage's record said this volume had a pool —
        // what turns "no pool file" below from "first stage" into
        // "pool lost" once the slot resolution has had its say.
        let mut pool_was_recorded = false;
        if let Some(prior) = self.state(volume) {
            // "Still live" is judged per shape. A head-attached volume
            // probes its NBD device, as ever. A thin volume has no head
            // (origin-advance §5) — its liveness IS the dm stack, which
            // survives a driver restart and dies with a reboot, exactly
            // the line this branch needs to draw.
            let live = match (&prior.thin, &prior.device) {
                (Some(_), _) => {
                    let stack = self.thin.clone();
                    let vol = volume.to_string();
                    tokio::task::spawn_blocking(move || {
                        stack.map(|s| s.built(&vol)).and_then(Result::ok)
                    })
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(false)
                }
                (None, Some(d)) => self.device_attached(d),
                (None, None) => false,
            };
            if live {
                // Replayed stage on a live attachment — the one path a
                // driver restart takes, so make sure the embedded cycle
                // is running again (idempotent).
                if let Some(t) = &prior.thin {
                    // Invariant B: this branch skips `stack.build`, and
                    // `build` is where every stage re-derives the mask
                    // from observation. The shim it is skipping was
                    // loaded by a process that is gone — possibly one
                    // that died between a retarget and its re-assert —
                    // so re-assert here rather than trust it. NOT fatal,
                    // unlike in `build` — and the difference is `build`'s
                    // own reason: there the stage is the one moment
                    // nothing is mounted yet, so refusing costs a failed
                    // stage instead of a diverged image. Here the volume
                    // is already attached and mounted, so refusing
                    // un-exposes nothing; it only skips `start_cycle`,
                    // and a volume that never publishes strands its
                    // delta while kubelet retries forever. Log, and let
                    // the cycle's own per-cycle re-assert keep trying.
                    let spec = self.thin_spec(volume, t);
                    if let Some(stack) = &self.thin {
                        if let Err(e) = stack.assert_guest_discard_mask(&spec) {
                            tracing::error!(
                                volume,
                                error = %e,
                                "guest-discard mask not asserted on the replayed stage: \
                                 this shim can pass a guest fstrim into blocks the origin \
                                 serves (Invariant B). The cycle re-asserts every interval."
                            );
                        }
                    }
                    // `export_for` is pure and the real binding is
                    // made below, past this early return.
                    self.start_cycle(volume, staging_path, t, block, export_for(volume).1)?;
                }
                return Ok(());
            }
            // Reboot / crash: kernel state is gone, our records survive.
            // A thin volume whose pool file is ALSO gone lost exactly
            // its unpublished delta — but that verdict is NOT announced
            // here, and the placement is the point (§7 crash windows,
            // task 15e): a crash between a flipped swing's A-delete and
            // its rename leaves the live pool momentarily under the
            // `.b` name, so an existence probe taken before
            // `resolve_slot` restores it would report a loss the sweep
            // is about to prevent — and "pool file lost" is the exact
            // string the smoke and S15k grep as fresh-path evidence, so
            // an early warn is a vacuity trap as well as a false
            // incident. The loss is announced below, where
            // `fresh_cache` is actually established — post-resolve.
            pool_was_recorded = prior.thin.is_some();
            std::fs::remove_file(self.state_path(volume))
                .map_err(|e| NodeError::Io(self.state_path(volume), e))?;
        }
        let (export, readonly) = export_for(volume);
        // The head attach — raw and reference volumes only. Thin
        // volumes are publish-only (origin-advance §5): no writable
        // network path exists, the pool is local, and the volume-level
        // lease with it — the lineage serve below is the fence that
        // matters. D-M6, receiving side, for the volumes that DO
        // attach: have the node's control plane serve the volume
        // (acquiring its lease) before the attach dials the export.
        // Best-effort — if it could not serve, the attach is the
        // failure that reports it.
        let device = if thin.is_none() {
            if !readonly {
                if let Some(h) = &self.handoff {
                    h.ensure_served(volume, false).await;
                }
            }
            let d = self.allocate_device()?;
            self.run_ok("nbd-client", self.nbd_client_args(&export, &d))
                .await?;
            Some(d)
        } else {
            None
        };

        // Everything between the attach above and the durable state
        // write below is a window with live kernel objects that nothing
        // records. A failure inside it used to strand the /dev/nbdN —
        // and 15j made a permanent stage failure a real shape (an
        // unmasked shim refuses the build, Invariant B), so the
        // kubelet retry loop would allocate a fresh device per attempt
        // and walk the node to NoFreeDevice while the same stage kept
        // failing on the same cause.
        //
        // The loop device is the same hazard one layer down: `losetup
        // --find` allocates a NEW /dev/loopN on every call, so the
        // "idempotent" the earlier note claimed for the loop attach was
        // never true — `cachedev::prepare` now adopts an existing
        // association, and this rollback hands back the one this
        // attempt made. The dm build genuinely is idempotent, but it
        // still has to be torn down first: a built stack holds the loop
        // (and the head), so detaching under it answers EBUSY and the
        // rollback would leak everything it was trying to reclaim.
        // Hence the unwind order below — stack, loop, NBD — which is
        // the build order reversed, the same order `unstage` uses.
        let mut attached: Vec<String> = device.iter().cloned().collect();
        let mut looped: Option<String> = None;
        let mut rlooped: Option<String> = None;
        let mut built: Option<ThinStackSpec> = None;
        let staged: Result<(String, Option<ThinState>), NodeError> = async {
            // Thin mode: loop the pool file, build the publish-only
            // stack; the filesystem device becomes the mount device —
            // the linear shim over the thin origin (D-M15), or the
            // read cache over it when one is configured (§7.1).
            let mut thin_state = None;
            let fs_dev = match thin {
                None => device.clone().expect("non-thin stages attach a head"),
                Some(params) => {
                    let (stack, dir) = match (&self.thin, &self.cfg.cache_dir) {
                        (Some(s), Some(d)) => (s.clone(), d.clone()),
                        _ => return Err(NodeError::ThinUnavailable),
                    };
                    std::fs::create_dir_all(&dir).map_err(|e| NodeError::Io(dir.clone(), e))?;
                    let cache_file = dir.join(format!("{volume}.cache"));
                    // Invariant F (task 15h): the lineage's lease is held
                    // under THIS NODE's identity for the whole stage — the
                    // granted epoch is the fence token every agent
                    // transaction (publish, repoint, pin transfer) carries.
                    // Served before the origin pointer/baseline work so the
                    // seeded record can carry the epoch. FALLIBLE on
                    // purpose: a Held/error here fails the stage (kubelet
                    // retries) instead of degrading to an epoch-less,
                    // unfenced stage a zombie could exploit.
                    let mut epoch = None;
                    if let (Some(h), Some(lin)) = (&self.handoff, &params.lineage) {
                        epoch = h
                            .serve_required(lin, true)
                            .await
                            .map_err(|e| NodeError::LineageServe(volume.to_string(), e))?;
                    }
                    // §7 crash windows (task 15e): resolve any durably
                    // recorded in-flight swing BEFORE anything probes or
                    // attaches the pool file — after a flipped swing the
                    // live pool is the `.b` file, so the `fresh_cache`
                    // probe below, the persisted-pool validation, and
                    // the loop attach would all read the wrong world.
                    // The spec is provisional (no origin attach exists
                    // yet); the sweeps never touch the external origin,
                    // so its absence is fine here.
                    if let (Some(cycle), Some(lin)) = (&self.cycle, &params.lineage) {
                        cycle
                            .resolve_slot(&CycleSpec {
                                volume: volume.to_string(),
                                lineage: lin.clone(),
                                staging_path: staging_path.to_string(),
                                size_bytes: params.size_bytes,
                                origin_dev: None,
                                epoch,
                                reclaim: params.reclaim,
                                pool_file: Some(cache_file.clone()),
                                pool_layout: crate::provision::fast_layout(
                                    params.size_bytes,
                                    self.cfg.cache_bytes,
                                    self.cfg.chunk_bytes,
                                )
                                .ok(),
                                read_cache: self.cfg.read_cache_dir.is_some(),
                                readonly,
                            })
                            .map_err(NodeError::Stack)?;
                    }
                    // D-M15: no cache file = fresh pool — this node holds
                    // none of the volume's structure. Decide (and durably
                    // initialize) the origin pointer before creating it.
                    let mut fresh_cache = !cache_file.exists();
                    // The loss announcement, HERE and not at the record
                    // probe above: only past `resolve_slot` has "the
                    // pool file is gone" been established rather than
                    // sampled mid-rename, so this cannot fire on the
                    // flipped-crash window the sweep just recovered —
                    // and under publish-only (§5) the loss it does
                    // announce is bounded by the publish cadence, with
                    // a fresh pool over the lineage origin as the
                    // designed recovery (the old §15.7 "cache file
                    // lost" failure class died with the writeback
                    // head). The stage proceeds loudly, not refusing.
                    if fresh_cache && pool_was_recorded {
                        tracing::warn!(
                            volume,
                            pool_file = %cache_file.display(),
                            "local pool file lost — re-staging a fresh pool from the \
                             lineage origin; writes since the last publish are gone \
                             (bounded by the publish cadence, origin-advance §5)"
                        );
                    }
                    // Invariant F, stage side (task 15h follow-through): a
                    // persisted pool is adopted only after it validates
                    // against the authority just acquired — see
                    // `validate_persisted_pool`. On failure: quarantine
                    // (rename beside itself) and fall through to the fresh
                    // path, which re-seeds baseline + record from the
                    // pointer under the new epoch.
                    if !fresh_cache {
                        if let Some(lin) = &params.lineage {
                            if let Some((why, tag)) =
                                self.validate_persisted_pool(volume, lin, epoch)
                            {
                                let quarantined = quarantine_cache(&cache_file, tag)?;
                                // Marker only after the rename landed, as in
                                // the unstage branch.
                                if let Some(cycle) = &self.cycle {
                                    cycle.clear_fenced(volume);
                                }
                                tracing::warn!(
                                    volume,
                                    lineage = %lin,
                                    why,
                                    quarantined = %quarantined.display(),
                                    "persisted pool failed stage-time validation — \
                                     quarantined; fresh-seeding from the pointer \
                                     (Invariant F)"
                                );
                                fresh_cache = true;
                            }
                        }
                    }
                    let origin = self
                        .plan_external_origin(volume, &params, fresh_cache, epoch)
                        .await?;
                    // Shared pool-device rules (crate::cachedev):
                    // fallocate-preallocated — never sparse, so backing-fs
                    // ENOSPC is a clean stage-time failure instead of a
                    // mid-write pool wedge — and looped with direct IO
                    // where supported.
                    let cache = crate::cachedev::prepare(
                        self.runner.as_ref(),
                        &cache_file,
                        self.cfg.cache_bytes,
                    )
                    .await
                    .map_err(|e| NodeError::Stack(e.to_string()))?;
                    looped = Some(cache.loop_dev.clone());
                    if !cache.direct_io {
                        tracing::warn!(volume, "cache loop is buffered (no O_DIRECT support)");
                    }
                    // §7.1: the read-cache file, looped the same way.
                    // The FILE is the warmth (rcmeta persists across
                    // reboots inside it); a brand-new file reads as
                    // zeros, which is dm-cache's own "format me", so
                    // first-stage and lost-file both come up cold with
                    // no special casing. The build stamps
                    // `cold_read_cache` from `fresh_cache` below, so a
                    // surviving warm file over a REBUILT pool is
                    // zeroed rather than adopted (the §7.1 rule).
                    let mut rcache_loop = None;
                    let mut rcache_file = None;
                    if let Some(rdir) = &self.cfg.read_cache_dir {
                        std::fs::create_dir_all(rdir)
                            .map_err(|e| NodeError::Io(rdir.clone(), e))?;
                        let rfile = rdir.join(format!("{volume}.rcache"));
                        let rc = crate::cachedev::prepare(
                            self.runner.as_ref(),
                            &rfile,
                            self.cfg.read_cache_bytes,
                        )
                        .await
                        .map_err(|e| NodeError::Stack(e.to_string()))?;
                        rlooped = Some(rc.loop_dev.clone());
                        rcache_loop = Some(rc.loop_dev);
                        rcache_file = Some(rfile);
                    }
                    // Second, read-only NBD attach: the lineage's
                    // repointable `<lineage>/origin` export becomes the
                    // thin device's external origin (D-M15 15d — the
                    // single attach that replaces the two-clone swap; each
                    // advance repoints what the export serves server-side,
                    // the attach itself never changes). Its D-M6 serve
                    // already happened above, under the node's identity.
                    let mut origin_dev = None;
                    if let Some(lin) = &origin {
                        let dev = self.allocate_device()?;
                        self.run_ok(
                            "nbd-client",
                            self.nbd_client_args(&format!("{lin}/origin"), &dev),
                        )
                        .await?;
                        attached.push(dev.clone());
                        origin_dev = Some(dev);
                    }
                    let t = ThinState {
                        loop_dev: cache.loop_dev,
                        cache_file,
                        size_bytes: params.size_bytes,
                        lineage: params.lineage,
                        origin,
                        origin_dev,
                        epoch,
                        reclaim: params.reclaim,
                        rcache_loop,
                        rcache_file,
                    };
                    let spec = self.thin_spec(volume, &t);
                    // The filesystem lives on the MOUNT device, never
                    // the thin origin (D-M15): the fs claim lands on
                    // the shim (or the read cache over it), so the
                    // origin stays merely dm-claimed — repointable, and
                    // reclaimable through the discard port.
                    let mount_dev = spec.mount_path();
                    let mut for_build = spec.clone();
                    // Fresh pool ⇒ cold cache (§7.1's one invalidation
                    // event): this build replaces the content below any
                    // configured read cache without traversing it, so a
                    // surviving warm cache would serve the delta the
                    // pool loss just destroyed. The flag rides only on
                    // the build's copy — the retained `spec` (unwind
                    // teardown, mount-path lookups) must never re-zero.
                    for_build.cold_read_cache = fresh_cache;
                    tokio::task::spawn_blocking(move || stack.build(&for_build))
                        .await
                        .map_err(|e| NodeError::Stack(e.to_string()))?
                        .map_err(NodeError::Stack)?;
                    // From here a failure (mkfs, mount) has a live dm
                    // stack over the loop to unwind first.
                    built = Some(spec);
                    thin_state = Some(t);
                    mount_dev
                }
            };

            if !block {
                let fstype = fstype.unwrap_or(&self.cfg.default_fstype).to_string();
                if !readonly {
                    // Format only a blank device: blkid exits non-zero with
                    // no output when it finds no signature.
                    let probe = self
                        .runner
                        .run(
                            "blkid",
                            &[
                                "-o".into(),
                                "value".into(),
                                "-s".into(),
                                "TYPE".into(),
                                fs_dev.clone(),
                            ],
                        )
                        .await
                        .map_err(|e| NodeError::Command {
                            cmd: "blkid".into(),
                            detail: e.to_string(),
                        })?;
                    if probe.stdout.trim().is_empty() {
                        self.run_ok(&format!("mkfs.{fstype}"), vec![fs_dev.clone()])
                            .await?;
                    }
                }
                std::fs::create_dir_all(staging_path)
                    .map_err(|e| NodeError::Io(staging_path.into(), e))?;
                let mut mount_args = vec!["-t".into(), fstype.clone()];
                if readonly {
                    mount_args.extend(["-o".into(), ro_mount_opts(&fstype).into()]);
                }
                mount_args.extend([fs_dev.clone(), staging_path.into()]);
                self.run_ok("mount", mount_args).await?;
            }
            Ok((fs_dev, thin_state))
        }
        .await;
        let (fs_dev, thin_state) = match staged {
            Ok(v) => v,
            Err(e) => {
                // Build order reversed. Each step is best-effort and
                // logged: the stage has already failed, and the error
                // worth returning is the one that caused it.
                if let (Some(spec), Some(stack)) = (built, self.thin.clone()) {
                    let torn = tokio::task::spawn_blocking(move || stack.teardown(&spec))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r);
                    if let Err(d) = torn {
                        tracing::warn!(volume, error = %d,
                            "stage rollback: teardown failed; the dm stack stays up and the \
                             detaches below will fail against it");
                    }
                }
                for loop_dev in rlooped.iter().chain(looped.iter()) {
                    if let Err(d) = crate::cachedev::detach(self.runner.as_ref(), loop_dev).await {
                        tracing::warn!(volume, loop_dev, error = %d,
                            "stage rollback: loop detach failed; this device stays attached \
                             (the next attempt adopts it rather than adding another)");
                    }
                }
                for dev in attached.iter().rev() {
                    if let Err(d) = self
                        .run_ok("nbd-client", vec!["-d".into(), dev.clone()])
                        .await
                    {
                        tracing::warn!(volume, device = %dev, error = %d,
                            "stage rollback: detach failed; this device stays allocated");
                    }
                }
                return Err(e);
            }
        };

        std::fs::create_dir_all(&self.cfg.state_dir)
            .map_err(|e| NodeError::Io(self.cfg.state_dir.clone(), e))?;
        let state = serde_json::to_vec(&StageState {
            device: device.clone(),
            thin: thin_state.clone(),
        })
        .expect("state serializes");
        std::fs::write(self.state_path(volume), state)
            .map_err(|e| NodeError::Io(self.state_path(volume), e))?;
        // Continuous publication (6h-2): after the stack is up and the
        // filesystem mounted, the embedded agent starts snapshotting
        // into the lineage volume. Failing here fails the stage — a
        // thin volume that silently cannot snapshot violates the mode's
        // whole contract; the kubelet retries and the replay path above
        // re-attempts the start.
        if let Some(t) = &thin_state {
            self.start_cycle(volume, staging_path, t, block, readonly)?;
        }
        tracing::info!(
            volume,
            export,
            device = device.as_deref().unwrap_or("-"),
            fs_dev,
            readonly,
            endpoint = %self.cfg.endpoint,
            "staged"
        );
        Ok(())
    }

    /// Unmount (tolerating "not mounted") + tear down the stack +
    /// detach + drop state. Idempotent: no state file = already
    /// unstaged. For thin volumes the D-M13 final cycle runs FIRST and
    /// is the durability step (§5: there is no writeback to flush); a
    /// final-cycle failure leaves state in place so the unstage retries
    /// rather than stranding the unpublished delta.
    pub async fn unstage(&self, volume: &str, staging_path: &str) -> Result<(), NodeError> {
        // D-M13: the final publish cycle runs FIRST — before the node
        // lock and the umount. The origin must still be mounted for the
        // freeze (and NodeUnpublish already removed the pod's bind
        // mount, so the freeze is uncontended), and the wait is
        // control-plane-bound, so holding the device lock through it
        // would stall unrelated volumes' stage/unstage. `finish` is an
        // immediate Ok for volumes without a running cycle. On error we
        // refuse to tear down: kubelet retries the unstage and the
        // resumable cycle continues the upload — a planned reschedule
        // loses nothing (the whole point of D-M13).
        //
        // EXCEPT when the stage is FENCED (Invariant F, task 15h): its
        // epoch is superseded, the final cycle would publish exactly the
        // stale delta — and be refused forever. The fenced branch skips
        // the publish wait, quarantines the cache file (rename, delete
        // later — staged discard, survivable until soak proves the
        // trigger) and proceeds with teardown. Transient finish errors
        // keep the refuse-and-retry path: the fence fires only on the
        // epoch-specific refusal, never on a timeout or transport error.
        let fenced_epoch = self.cycle.as_ref().and_then(|c| c.fenced(volume));
        match (&self.cycle, fenced_epoch) {
            (Some(cycle), None) => {
                cycle
                    .finish(volume)
                    .await
                    .map_err(|e| NodeError::FinalCycle(volume.to_string(), e))?;
            }
            (Some(_), Some(epoch)) => {
                tracing::error!(
                    volume,
                    epoch,
                    "unstage: stage is fenced (epoch superseded) — skipping the final \
                     cycle, quarantining local state (Invariant F / D-M13)"
                );
            }
            (None, _) => {}
        }
        let _g = self.lock.lock().await;
        let Some(state) = self.state(volume) else {
            return Ok(());
        };
        let device = state.device;
        // The embedded cycle stops FIRST — before the umount, not just
        // before teardown: its freeze watchdog holds a pre-opened fd on
        // the mounted origin (V3.2 §7.11), so a plain umount under a
        // live cycle fails EBUSY against our own process, forever.
        // Stopping drops the engine (watchdog thread joined, fd closed)
        // and deactivates the agent's thin device nodes.
        if state.thin.is_some() {
            if let Some(cycle) = &self.cycle {
                cycle.stop(volume).await;
            }
        }
        let um = self
            .runner
            .run("umount", &[staging_path.to_string()])
            .await
            .map_err(|e| NodeError::Command {
                cmd: "umount".into(),
                detail: e.to_string(),
            })?;
        if !um.success
            && !um.stderr.contains("not mounted")
            && !um.stderr.contains("no mount point")
        {
            return Err(NodeError::Command {
                cmd: format!("umount {staging_path}"),
                detail: um.stderr,
            });
        }
        if let Some(t) = &state.thin {
            let stack = self.thin.clone().ok_or(NodeError::ThinUnavailable)?;
            let spec = self.thin_spec(volume, t);
            tokio::task::spawn_blocking(move || stack.teardown(&spec))
                .await
                .map_err(|e| NodeError::Stack(e.to_string()))?
                .map_err(NodeError::Stack)?;
            // The pool loop, resolved from the FILE, not the record: a
            // §7 swing replaces the loop mid-life (the B slot's loop
            // survives the canonicalize under the live file's name), so
            // `t.loop_dev` can be stale — and a stale loop number may by
            // now back some other volume. The recorded device is only
            // the fallback for a probe that cannot answer.
            let pool_loop =
                crate::cachedev::existing_association(self.runner.as_ref(), &t.cache_file)
                    .await
                    .map(|c| c.loop_dev)
                    .unwrap_or_else(|| t.loop_dev.clone());
            crate::cachedev::detach(self.runner.as_ref(), &pool_loop)
                .await
                .map_err(|e| NodeError::Stack(e.to_string()))?;
            // §7.1: the read-cache loop goes with the stack; the FILE
            // stays for the same reason the pool file does — its
            // rcmeta is the restage warmth. A fenced next stage is
            // covered without quarantining it: the fresh pool it
            // forces stamps `cold_read_cache`, which zeroes the carve.
            if let Some(rl) = &t.rcache_loop {
                crate::cachedev::detach(self.runner.as_ref(), rl)
                    .await
                    .map_err(|e| NodeError::Stack(e.to_string()))?;
            }
            // D-M15: the origin export attach goes with the stack.
            // Nothing needs reclaiming at DeleteVolume — the export
            // lives in the server's registry and its pointer row dies
            // with the lineage's refs object.
            if let Some(od) = &t.origin_dev {
                self.run_ok("nbd-client", vec!["-d".into(), od.clone()])
                    .await?;
            }
            // The cache file stays: its tmeta region IS the volume's
            // logical structure — the flushed head is pool-physical and
            // only interpretable through it (V3.2 §6.5). Thin volumes
            // are node-sticky (PV topology pins them here); the file is
            // reclaimed only with the volume itself.
            //
            // UNLESS the stage was fenced (Invariant F): the pool holds
            // a superseded epoch's delta — quarantine the file beside
            // itself (`<vol>.cache.fenced-e<epoch>`) so the next stage
            // starts a fresh pool from the lineage origin instead of
            // resuming a fenced one. Rename, not unlink: a
            // misclassification stays recoverable until the file is
            // swept. A failed rename fails the unstage — leaving the
            // fenced pool under its live name would resurrect it.
            if let Some(e) = fenced_epoch {
                let quarantined = quarantine_cache(&t.cache_file, Some(e))?;
                // Only now that the rename landed may the durable fenced
                // marker go: a crash or rename failure before this point
                // leaves the marker, so the RETRY still takes this
                // quarantine branch (the running map is long gone by
                // then — stop() already dropped it).
                if let Some(cycle) = &self.cycle {
                    cycle.clear_fenced(volume);
                }
                tracing::warn!(volume, quarantined = %quarantined.display(),
                    "fenced pool quarantined (delete after soak)");
            }
        }
        // The head detach — raw and reference volumes only; a thin
        // volume never attached one (origin-advance §5). Old-format
        // state files that recorded a head for a thin volume still get
        // it detached here.
        if let Some(d) = &device {
            self.run_ok("nbd-client", vec!["-d".into(), d.clone()])
                .await?;
        }
        // D-M6: the volume's writes are flushed (thin: published — the
        // final cycle above) and the client is gone — release the
        // lease(s) so the next node's stage acquires instead of waiting
        // out the TTL. Best-effort: on failure the TTL is the backstop,
        // exactly as on node death. Thin volumes hold only the
        // LINEAGE's lease (acquired by `serve_required` at stage, task
        // 15h) — there is no head, so no volume-level lease to return.
        if !export_for(volume).1 {
            if let Some(h) = &self.handoff {
                match &state.thin {
                    None => h.release(volume).await,
                    Some(t) => {
                        if let Some(lin) = t.lineage.as_ref().or(t.origin.as_ref()) {
                            h.release(lin).await;
                        }
                    }
                }
            }
        }
        std::fs::remove_file(self.state_path(volume))
            .map_err(|e| NodeError::Io(self.state_path(volume), e))?;
        tracing::info!(
            volume,
            device = device.as_deref().unwrap_or("-"),
            "unstaged"
        );
        Ok(())
    }

    /// Bind the staged volume into the pod: directory bind for Mount
    /// volumes, device-file bind for Block volumes.
    pub async fn publish(
        &self,
        volume: &str,
        staging_path: &str,
        target_path: &str,
        block: bool,
        readonly: bool,
    ) -> Result<(), NodeError> {
        let source = if block {
            self.published_device(volume)
                .ok_or_else(|| NodeError::NotStaged(volume.into()))?
        } else {
            staging_path.to_string()
        };
        if block {
            if let Some(parent) = Path::new(target_path).parent() {
                std::fs::create_dir_all(parent).map_err(|e| NodeError::Io(parent.into(), e))?;
            }
            if !Path::new(target_path).exists() {
                std::fs::File::create(target_path)
                    .map_err(|e| NodeError::Io(target_path.into(), e))?;
            }
        } else {
            std::fs::create_dir_all(target_path)
                .map_err(|e| NodeError::Io(target_path.into(), e))?;
        }
        self.run_ok(
            "mount",
            vec!["--bind".into(), source, target_path.to_string()],
        )
        .await?;
        if readonly {
            self.run_ok(
                "mount",
                vec![
                    "-o".into(),
                    "remount,bind,ro".into(),
                    target_path.to_string(),
                ],
            )
            .await?;
        }
        Ok(())
    }

    /// Unbind; tolerant of an already-gone mount (idempotent).
    pub async fn unpublish(&self, target_path: &str) -> Result<(), NodeError> {
        let um = self
            .runner
            .run("umount", &[target_path.to_string()])
            .await
            .map_err(|e| NodeError::Command {
                cmd: "umount".into(),
                detail: e.to_string(),
            })?;
        if !um.success
            && !um.stderr.contains("not mounted")
            && !um.stderr.contains("no mount point")
        {
            return Err(NodeError::Command {
                cmd: format!("umount {target_path}"),
                detail: um.stderr,
            });
        }
        // Best-effort cleanup of the bind target.
        let _ = std::fs::remove_file(target_path);
        let _ = std::fs::remove_dir(target_path);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hostcmd::mock::MockRunner;
    use crate::hostcmd::Output;
    use std::sync::atomic::Ordering;

    fn cfg(dir: &Path, creds: bool) -> NodeConfig {
        let sysfs = dir.join("sys-block");
        for i in 0..4 {
            std::fs::create_dir_all(sysfs.join(format!("nbd{i}"))).unwrap();
        }
        std::fs::create_dir_all(sysfs.join("sda")).unwrap(); // non-nbd noise
        NodeConfig {
            endpoint: NbdEndpoint::Tcp {
                host: "skynbd.example".into(),
                port: 10809,
                tls_hostname: None,
                creds_dir: creds.then(|| dir.join("creds")),
            },
            sysfs_block: sysfs,
            state_dir: dir.join("state"),
            default_fstype: "ext4".into(),
            cache_dir: Some(dir.join("cache")),
            cache_bytes: 256 << 20,
            chunk_bytes: 64 << 10,
            read_cache_dir: None,
            read_cache_bytes: 0,
        }
    }

    /// The kernel writes a sysfs pid file when nbd attaches; the mock
    /// runner doesn't, so tests that rely on the staged/attached check
    /// simulate it.
    fn mark_attached(m: &NodeMechanics, device: &str) {
        let name = device.rsplit('/').next().unwrap();
        std::fs::write(m.cfg.sysfs_block.join(name).join("pid"), "1").unwrap();
    }

    fn mechanics(dir: &Path, creds: bool) -> (NodeMechanics, Arc<MockRunner>) {
        let runner = Arc::new(MockRunner::default());
        (
            NodeMechanics::new(
                cfg(dir, creds),
                runner.clone() as Arc<dyn Runner>,
                None,
                None,
            ),
            runner,
        )
    }

    /// ThinParams for tests that don't exercise the cycle.
    fn tp(size: u64) -> ThinParams {
        ThinParams {
            size_bytes: size,
            lineage: None,
            reclaim: Default::default(),
        }
    }

    /// Records handoff calls into the runner's own ordered log (as
    /// `<handoff> serve|release <vol>` rows), so serve-before-attach and
    /// release-after-detach are assertable against one sequence.
    struct RecorderHandoff(Arc<MockRunner>);

    #[async_trait::async_trait]
    impl crate::handoff::Handoff for RecorderHandoff {
        async fn ensure_served(&self, volume: &str, node_holder: bool) -> Option<u64> {
            self.0.calls.lock().unwrap().push(vec![
                "<handoff>".into(),
                if node_holder {
                    "serve-as-node"
                } else {
                    "serve"
                }
                .into(),
                volume.into(),
            ]);
            node_holder.then_some(7)
        }
        async fn release(&self, volume: &str) {
            self.0.calls.lock().unwrap().push(vec![
                "<handoff>".into(),
                "release".into(),
                volume.into(),
            ]);
        }
    }

    /// D-M6 choreography: serve precedes the attach; release follows the
    /// detach; both are skipped for read-only stages.
    #[tokio::test]
    async fn handoff_brackets_the_attachment() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(false, ""); // blkid: blank device

        let staging = dir.path().join("stage/pvc-1").display().to_string();
        m.stage("pvc-1", &staging, None, false, None).await.unwrap();
        mark_attached(&m, "/dev/nbd0");
        m.unstage("pvc-1", &staging).await.unwrap();

        let seq: Vec<String> = runner.calls().iter().map(|c| c.join(" ")).collect();
        let pos = |needle: &str| {
            seq.iter()
                .position(|c| c.starts_with(needle))
                .unwrap_or_else(|| panic!("{needle:?} not in {seq:?}"))
        };
        assert!(
            pos("<handoff> serve pvc-1") < pos("nbd-client -N"),
            "serve must precede the attach: {seq:?}"
        );
        assert!(
            pos("<handoff> release pvc-1") > pos("nbd-client -d"),
            "release must follow the detach: {seq:?}"
        );
    }

    #[tokio::test]
    async fn handoff_skips_readonly_stages() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));

        let staging = dir.path().join("stage/ro").display().to_string();
        m.stage("ro:vol1/snap/nightly", &staging, None, false, None)
            .await
            .unwrap();
        mark_attached(&m, "/dev/nbd0");
        m.unstage("ro:vol1/snap/nightly", &staging).await.unwrap();

        assert!(
            !runner.calls().iter().any(|c| c[0] == "<handoff>"),
            "read-only stages own no lease: {:?}",
            runner.calls()
        );
    }

    #[tokio::test]
    async fn stage_plans_tls_attach_format_mount() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), true);
        // Replies: nbd-client ok, blkid finds nothing, mkfs ok, mount ok.
        runner.push_reply(true, "");
        runner.push_reply(false, "");
        runner.push_reply(true, "");
        runner.push_reply(true, "");

        // Staging paths must be writable by the test user (CI runs
        // unprivileged): stage() create_dir_all's them for real.
        let staging = dir.path().join("stage/pvc-1").display().to_string();
        m.stage("pvc-1", &staging, None, false, None).await.unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 4);
        let nbd = calls[0].join(" ");
        assert!(nbd.starts_with(
            "nbd-client -N pvc-1/head skynbd.example 10809 /dev/nbd0 -persist -enable-tls"
        ));
        for flag in ["-certfile", "-keyfile", "-cacertfile"] {
            assert!(nbd.contains(flag), "{nbd}");
        }
        assert!(nbd.ends_with("-tlshostname skynbd.example"), "{nbd}");
        assert_eq!(calls[2][0], "mkfs.ext4");
        assert_eq!(
            calls[3],
            vec![
                "mount".to_string(),
                "-t".into(),
                "ext4".into(),
                "/dev/nbd0".into(),
                staging.clone()
            ]
        );
        assert_eq!(m.staged_device("pvc-1").as_deref(), Some("/dev/nbd0"));

        // Idempotent replay (device still attached): no further commands.
        mark_attached(&m, "/dev/nbd0");
        m.stage("pvc-1", &staging, None, false, None).await.unwrap();
        assert_eq!(runner.calls().len(), 4);
    }

    /// Node-local Unix data path (CSI design §2.1): the socket path
    /// takes the host positional's place behind `-unix` (nbd-client
    /// reads the following non-numeric positional as the device), and
    /// no TLS arguments appear at all — the socket's file mode is the
    /// access control.
    #[tokio::test]
    async fn stage_over_unix_socket_attaches_without_tls() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = cfg(dir.path(), true);
        c.endpoint = NbdEndpoint::Unix("/run/skynbd/nbd.sock".into());
        let runner = Arc::new(MockRunner::default());
        let m = NodeMechanics::new(c, runner.clone() as Arc<dyn Runner>, None, None);
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, "ext4\n"); // blkid: already formatted
        runner.push_reply(true, ""); // mount

        let staging = dir.path().join("stage/pvc-1").display().to_string();
        m.stage("pvc-1", &staging, None, false, None).await.unwrap();

        assert_eq!(
            runner.calls()[0],
            vec![
                "nbd-client",
                "-N",
                "pvc-1/head",
                "-unix",
                "/run/skynbd/nbd.sock",
                "/dev/nbd0",
                "-persist",
            ]
        );
    }

    #[tokio::test]
    async fn restage_after_reboot_reattaches() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, "ext4\n"); // blkid: formatted
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/v").display().to_string();
        m.stage("v", &staging, None, false, None).await.unwrap();
        assert_eq!(runner.calls().len(), 3);

        // Reboot: state file survives, kernel attachment does not (no
        // sysfs pid file). A replayed stage re-attaches and re-mounts —
        // and must NOT re-format (blkid finds the filesystem).
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        m.stage("v", &staging, None, false, None).await.unwrap();
        let calls = runner.calls();
        assert_eq!(calls.len(), 6);
        assert_eq!(calls[3][0], "nbd-client");
        assert!(!calls.iter().any(|c| c[0].starts_with("mkfs")));
    }

    #[tokio::test]
    async fn stage_skips_mkfs_on_formatted_device_and_tls_when_plaintext() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        runner.push_reply(true, "");
        runner.push_reply(true, "ext4\n"); // blkid: already formatted
        runner.push_reply(true, "");

        let staging = dir.path().join("stage/v").display().to_string();
        m.stage("v", &staging, Some("xfs"), false, None)
            .await
            .unwrap();
        let calls = runner.calls();
        assert_eq!(calls.len(), 3); // no mkfs
        assert!(!calls[0].join(" ").contains("-enable-tls"));
        assert_eq!(calls[2][1], "-t");
        assert_eq!(calls[2][2], "xfs");
    }

    #[tokio::test]
    async fn block_stage_attaches_without_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        m.stage("bv", "/stage/bv", None, true, None).await.unwrap();
        assert_eq!(runner.calls().len(), 1); // nbd-client only
                                             // Publish binds the device node onto the target file.
        let target = dir.path().join("pod/dev-bv");
        m.publish("bv", "/stage/bv", target.to_str().unwrap(), true, false)
            .await
            .unwrap();
        let last = runner.calls().last().unwrap().clone();
        assert_eq!(last[0..2], ["mount".to_string(), "--bind".to_string()]);
        assert_eq!(last[2], "/dev/nbd0");
        assert!(target.exists());
    }

    #[tokio::test]
    async fn device_allocation_skips_attached_devices() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        // nbd0 and nbd1 busy.
        for i in 0..2 {
            std::fs::write(m.cfg.sysfs_block.join(format!("nbd{i}/pid")), "1").unwrap();
        }
        m.stage("v", "/stage/v", None, true, None).await.unwrap();
        assert!(runner.calls()[0].join(" ").contains("/dev/nbd2"));
    }

    #[tokio::test]
    async fn unstage_unmounts_detaches_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        m.stage("v", "/stage/v", None, true, None).await.unwrap();

        m.unstage("v", "/stage/v").await.unwrap();
        let calls = runner.calls();
        assert_eq!(calls[1][0], "umount");
        assert_eq!(calls[2], vec!["nbd-client", "-d", "/dev/nbd0"]);
        assert!(m.staged_device("v").is_none());

        // Replay: nothing left to do, no commands run.
        m.unstage("v", "/stage/v").await.unwrap();
        assert_eq!(runner.calls().len(), 3);
    }

    #[tokio::test]
    async fn ro_reference_stage_mounts_snapshot_export_readonly() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        // Replies: nbd-client ok, mount ok — no blkid, no mkfs.
        runner.push_reply(true, "");
        runner.push_reply(true, "");

        let staging = dir.path().join("stage/ro").display().to_string();
        m.stage("ro:vol1/snap/nightly", &staging, None, false, None)
            .await
            .unwrap();

        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[0]
            .join(" ")
            .starts_with("nbd-client -N vol1/snap/nightly "));
        assert_eq!(
            calls[1],
            vec![
                "mount".to_string(),
                "-t".into(),
                "ext4".into(),
                "-o".into(),
                "ro,noload".into(),
                "/dev/nbd0".into(),
                staging.clone()
            ]
        );
        // State lands under a flattened filename and unstage finds it.
        assert_eq!(
            m.staged_device("ro:vol1/snap/nightly").as_deref(),
            Some("/dev/nbd0")
        );
        m.unstage("ro:vol1/snap/nightly", &staging).await.unwrap();
        assert_eq!(
            runner.calls().last().unwrap(),
            &vec!["nbd-client".to_string(), "-d".into(), "/dev/nbd0".into()]
        );
        assert!(m.staged_device("ro:vol1/snap/nightly").is_none());
    }

    /// Records stack calls; the dm orchestration itself is covered by
    /// the agent crate's provision tests against the contract-enforcing
    /// mock.
    #[derive(Default)]
    struct RecStack(std::sync::Mutex<Vec<String>>);
    impl ThinStack for RecStack {
        fn build(&self, spec: &ThinStackSpec) -> Result<(), String> {
            // The rcache suffix appears only on cached specs so the
            // pre-§7.1 tests' exact-match assertions stay byte-stable.
            let rcache = match &spec.read_cache_dev {
                Some(d) => format!(
                    " rcache={d} rbytes={} cold={}",
                    spec.read_cache_bytes, spec.cold_read_cache
                ),
                None => String::new(),
            };
            self.0.lock().unwrap().push(format!(
                "build {} fast={} data={} pool={} chunk={} origin={}{rcache}",
                spec.volume,
                spec.fast_dev,
                spec.data_bytes,
                spec.fast_bytes,
                spec.chunk_bytes,
                spec.external_origin.as_deref().unwrap_or("-"),
            ));
            Ok(())
        }
        fn built(&self, volume: &str) -> Result<bool, String> {
            // The replay probe: "is the dm stack up?" — modelled over
            // the recorded calls as "the last relevant event is a
            // build". Tests simulate a reboot (kernel state gone, log
            // survives) by pushing a literal "reboot" event.
            let log = self.0.lock().unwrap();
            let last = log.iter().rev().find(|e| {
                e.as_str() == "reboot"
                    || e.starts_with(&format!("build {volume} "))
                    || e.as_str() == format!("teardown {volume}")
            });
            Ok(last.is_some_and(|e| e.starts_with("build")))
        }
        fn teardown(&self, spec: &ThinStackSpec) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("teardown {}", spec.volume));
            Ok(())
        }
        fn assert_guest_discard_mask(&self, spec: &ThinStackSpec) -> Result<(), String> {
            self.0
                .lock()
                .unwrap()
                .push(format!("mask {}", spec.names().fs));
            Ok(())
        }
    }

    /// Cycle-runner recorder writing into the shared RecStack event log,
    /// so tests can assert ordering (stop-before-teardown). Seeded
    /// baselines are remembered per lineage — the stand-in for the
    /// durable agent record `record_epoch` reads in production — and
    /// tests may pre-register one to model a pool that survives from an
    /// earlier stage.
    struct RecCycle {
        log: Arc<RecStack>,
        records: std::sync::Mutex<std::collections::HashMap<String, Option<u64>>>,
    }

    impl RecCycle {
        fn new(log: Arc<RecStack>) -> Self {
            Self {
                log,
                records: std::sync::Mutex::new(std::collections::HashMap::new()),
            }
        }

        /// Pre-register the agent record a previous stage would have
        /// left behind (lineage → its stage epoch).
        fn with_record(self, lineage: &str, epoch: Option<u64>) -> Self {
            self.records
                .lock()
                .unwrap()
                .insert(lineage.to_string(), epoch);
            self
        }
    }

    #[async_trait::async_trait]
    impl CycleRunner for RecCycle {
        fn start(&self, spec: &CycleSpec) -> Result<(), String> {
            let opt = |v: Option<String>| v.unwrap_or_else(|| "-".into());
            self.log.0.lock().unwrap().push(format!(
                "cycle-start {} lineage={} at={} size={} origin={} epoch={} \
                 strategy={} timing={} threshold={} swing_every={}",
                spec.volume,
                spec.lineage,
                spec.staging_path,
                spec.size_bytes,
                spec.origin_dev.is_some(),
                opt(spec.epoch.map(|e| e.to_string())),
                opt(spec
                    .reclaim
                    .strategy
                    .map(|s| crate::agentcycle::strategy_str(s).into())),
                opt(spec
                    .reclaim
                    .timing
                    .map(|t| crate::agentcycle::timing_str(t).into())),
                opt(spec.reclaim.range_threshold.map(|r| r.to_string())),
                opt(spec.reclaim.swing_every_n.map(|n| n.to_string())),
            ));
            Ok(())
        }
        fn resolve_slot(&self, spec: &CycleSpec) -> Result<(), String> {
            self.log.0.lock().unwrap().push(format!(
                "cycle-resolve-slot {} lineage={} pool={} layout={}",
                spec.volume,
                spec.lineage,
                spec.pool_file
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "-".into()),
                spec.pool_layout.is_some(),
            ));
            Ok(())
        }
        fn seed_baseline(
            &self,
            lineage: &str,
            base: &str,
            epoch: Option<u64>,
        ) -> Result<(), String> {
            self.records
                .lock()
                .unwrap()
                .insert(lineage.to_string(), epoch);
            self.log.0.lock().unwrap().push(format!(
                "cycle-seed {lineage} {base} epoch={}",
                epoch.map(|e| e.to_string()).unwrap_or_else(|| "-".into())
            ));
            Ok(())
        }

        fn record_epoch(&self, lineage: &str) -> Option<Option<u64>> {
            self.records.lock().unwrap().get(lineage).copied()
        }

        async fn finish(&self, volume: &str) -> Result<(), String> {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("cycle-finish {volume}"));
            Ok(())
        }

        async fn stop(&self, volume: &str) {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("cycle-stop {volume}"));
        }
    }

    /// A runner whose final cycle never publishes (D-M13 failure side).
    struct StuckCycle;

    #[async_trait::async_trait]
    impl CycleRunner for StuckCycle {
        fn start(&self, _spec: &CycleSpec) -> Result<(), String> {
            Ok(())
        }
        fn seed_baseline(
            &self,
            _lineage: &str,
            _base: &str,
            _epoch: Option<u64>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn finish(&self, _volume: &str) -> Result<(), String> {
            Err(
                "final cycle not published within 1s (phase=capturing … last_error=freeze: EBUSY)"
                    .into(),
            )
        }
        async fn stop(&self, _volume: &str) {
            panic!("stop must not run after a failed final cycle");
        }
    }

    fn thin_mechanics(dir: &Path) -> (NodeMechanics, Arc<MockRunner>, Arc<RecStack>) {
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        (
            NodeMechanics::new(
                cfg(dir, false),
                runner.clone() as Arc<dyn Runner>,
                Some(stack.clone() as Arc<dyn ThinStack>),
                None,
            ),
            runner,
            stack,
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn thin_stage_loops_cache_builds_stack_and_mounts_origin() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = thin_mechanics(dir.path());
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup --direct-io=on
        runner.push_reply(false, ""); // blkid: blank origin
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount

        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        // No head attach: thin volumes are publish-only (§5) — the
        // whole stage runs without a single nbd-client call.
        assert!(
            !runner.calls().iter().any(|c| c[0] == "nbd-client"),
            "{:?}",
            runner.calls()
        );
        // The pool file exists, preallocated, at the configured size.
        let cache = dir.path().join("cache/tv.cache");
        assert_eq!(cache.metadata().unwrap().len(), 256 << 20);
        // The stack was built over the loop alone.
        assert_eq!(
            stack.0.lock().unwrap().as_slice(),
            [format!(
                "build tv fast=/dev/loop5 data={} pool={} chunk={} origin=-",
                1u64 << 30,
                256u64 << 20,
                64u64 << 10
            )]
        );
        // The pool file is preallocated (ENOSPC at stage time, not
        // mid-write) and looped with direct IO.
        let calls = runner.calls();
        assert_eq!(
            calls[0],
            vec![
                "fallocate".to_string(),
                "-l".into(),
                (256u64 << 20).to_string(),
                cache.display().to_string()
            ]
        );
        assert!(
            calls[1].contains(&"--direct-io=on".to_string()),
            "{:?}",
            calls[1]
        );
        // mkfs and mount target the mount device — the linear shim
        // (D-M15) — never the thin origin, whose claim must stay
        // dm-only.
        assert_eq!(calls[3], vec!["mkfs.ext4", "/dev/mapper/sky-tv-fs"]);
        assert_eq!(calls[4][3], "/dev/mapper/sky-tv-fs");
        // Block publish binds the shim too.
        let target = dir.path().join("pod/dev-tv");
        m.publish("tv", &staging, target.to_str().unwrap(), true, false)
            .await
            .unwrap();
        let last = runner.calls().last().unwrap().clone();
        assert_eq!(last[2], "/dev/mapper/sky-tv-fs");
    }

    /// §7.1 read cache through the node: the stage loops a second
    /// per-volume file, the guest mounts the RCACHE (never the shim),
    /// the first stage is cold (no pool file = fresh pool ⇒ cold
    /// cache), a restage over the surviving pool file is warm, and
    /// unstage detaches both loops while both files survive (the
    /// rcache file's metadata IS the restage warmth).
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_stage_with_read_cache_mounts_the_rcache_cold_then_warm() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let mut c = cfg(dir.path(), false);
        c.read_cache_dir = Some(dir.path().join("rcache"));
        c.read_cache_bytes = 64 << 20;
        let m = NodeMechanics::new(
            c,
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            None,
        );
        runner.push_reply(true, ""); // fallocate (pool)
        runner.push_reply(true, "/dev/loop5\n"); // losetup (pool)
        runner.push_reply(true, ""); // fallocate (rcache)
        runner.push_reply(true, "/dev/loop6\n"); // losetup (rcache)
        runner.push_reply(false, ""); // blkid: blank
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();
        // The rcache file exists, preallocated at the configured size.
        let rfile = dir.path().join("rcache/tv.rcache");
        assert_eq!(rfile.metadata().unwrap().len(), 64 << 20);
        // First stage = no pool file = fresh pool ⇒ the build got the
        // cache device AND the cold flag.
        assert!(
            stack
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.contains("rcache=/dev/loop6") && e.contains("cold=true")),
            "{:?}",
            stack.0.lock().unwrap()
        );
        // mkfs and mount target the rcache — the guest-visible mount
        // device — never the shim it brackets.
        let calls = runner.calls();
        assert_eq!(calls[5], vec!["mkfs.ext4", "/dev/mapper/sky-tv-rcache"]);
        assert_eq!(calls[6][3], "/dev/mapper/sky-tv-rcache");

        m.unstage("tv", &staging).await.unwrap();
        let calls = runner.calls();
        let detached: Vec<_> = calls
            .iter()
            .filter(|c| c[0] == "losetup" && c[1] == "-d")
            .map(|c| c[2].clone())
            .collect();
        assert_eq!(
            detached,
            ["/dev/loop5", "/dev/loop6"],
            "both loops detach at unstage"
        );
        assert!(dir.path().join("cache/tv.cache").exists());
        assert!(rfile.exists(), "the rcache file is the warmth — it stays");

        // Restage over the SURVIVING pool file: warm — cold must not
        // be stamped (a warm resume must never zero the carve).
        runner.push_reply(true, ""); // losetup -j (pool file exists, detached)
        runner.push_reply(true, "/dev/loop7\n"); // losetup (pool)
        runner.push_reply(true, ""); // losetup -j (rcache file exists, detached)
        runner.push_reply(true, "/dev/loop8\n"); // losetup (rcache)
        runner.push_reply(true, "ext4\n"); // blkid: formatted
        runner.push_reply(true, ""); // mount
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();
        let log = stack.0.lock().unwrap();
        let last_build = log.iter().rev().find(|e| e.starts_with("build ")).unwrap();
        assert!(
            last_build.contains("rcache=/dev/loop8") && last_build.contains("cold=false"),
            "{last_build}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn thin_unstage_tears_down_and_keeps_the_pool_file() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = thin_mechanics(dir.path());
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        m.unstage("tv", &staging).await.unwrap();
        let calls = runner.calls();
        // umount → teardown → losetup -d. No nbd-client anywhere: no
        // head was ever attached (§5), and there is no flush — the
        // D-M13 final cycle is the durability step, and this rig has no
        // cycle runner.
        assert_eq!(calls[4][0], "umount");
        // The detach resolves the loop from the FILE first (a §7 swing
        // can have replaced the loop mid-life); with no live association
        // reported, the recorded device is the fallback.
        assert_eq!(calls[5][0..2], ["losetup", "--noheadings"]);
        assert_eq!(calls[6], vec!["losetup", "-d", "/dev/loop5"]);
        assert!(!calls.iter().any(|c| c[0] == "nbd-client"), "{calls:?}");
        assert!(stack.0.lock().unwrap().iter().any(|c| c == "teardown tv"));
        // State is gone, but the pool file SURVIVES: its tmeta region
        // is the volume's logical structure — the local delta is only
        // interpretable through it.
        assert!(dir.path().join("cache/tv.cache").exists());
        assert!(m.staged_device("tv").is_none());

        // Restage: the surviving file is reused — no recreation, no
        // re-fallocate; the volume's data comes back through its own
        // metadata.
        runner.push_reply(true, ""); // losetup -j: the unstage detached it
        runner.push_reply(true, "/dev/loop6\n"); // losetup (new loop)
        runner.push_reply(true, "ext4\n"); // blkid: fs is back
        runner.push_reply(true, ""); // mount
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();
        assert!(!runner.calls().iter().any(|c| c[0].starts_with("mkfs")));
        assert_eq!(
            runner
                .calls()
                .iter()
                .filter(|c| c[0] == "fallocate")
                .count(),
            1
        );
        assert_eq!(
            stack.0.lock().unwrap().last().unwrap(),
            &format!(
                "build tv fast=/dev/loop6 data={} pool={} chunk={} origin=-",
                1u64 << 30,
                256u64 << 20,
                64u64 << 10
            )
        );
    }

    /// A stage that fails after the attach gives the `/dev/nbdN` back.
    ///
    /// The window matters because 15j made PERMANENT stage failures a
    /// real shape: an unmasked shim refuses `build` (Invariant B), and
    /// the kubelet retries a failed NodeStageVolume forever. Without a
    /// rollback each retry ran `allocate_device` + `nbd-client` again
    /// and stranded another device, so a single volume that cannot be
    /// masked would walk the whole node to `NoFreeDevice` — turning one
    /// broken volume into a node that can stage nothing.
    #[tokio::test]
    async fn a_failed_stage_hands_back_the_device_it_attached() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        runner.push_reply(true, ""); // nbd-client attach
        runner.push_reply(false, ""); // blkid: blank device
        runner.push_reply(false, "mkfs refused"); // the failure
        runner.push_reply(true, ""); // nbd-client -d (the rollback)
        let staging = dir.path().join("stage/v1").display().to_string();
        m.stage("v1", &staging, None, false, None)
            .await
            .expect_err("mkfs failed, so the stage failed");

        let calls = runner.calls();
        assert_eq!(
            calls.last().unwrap(),
            &vec!["nbd-client".to_string(), "-d".into(), "/dev/nbd0".into()],
            "the last thing a failed stage does is detach: {calls:?}"
        );
        assert!(
            m.staged_device("v1").is_none(),
            "and it leaves no state record claiming otherwise"
        );
    }

    /// A ThinStack recording into the runner's own ordered log, so the
    /// unwind order across the two layers is assertable as one sequence.
    struct SharedLogStack(Arc<MockRunner>);

    impl ThinStack for SharedLogStack {
        fn build(&self, spec: &ThinStackSpec) -> Result<(), String> {
            self.0.calls.lock().unwrap().push(vec![
                "<stack>".into(),
                "build".into(),
                spec.volume.clone(),
            ]);
            Ok(())
        }
        fn built(&self, _: &str) -> Result<bool, String> {
            Ok(false)
        }
        fn teardown(&self, spec: &ThinStackSpec) -> Result<(), String> {
            self.0.calls.lock().unwrap().push(vec![
                "<stack>".into(),
                "teardown".into(),
                spec.volume.clone(),
            ]);
            Ok(())
        }
        fn assert_guest_discard_mask(&self, _: &ThinStackSpec) -> Result<(), String> {
            Ok(())
        }
    }

    /// The shape the rollback is for: a THIN stage that got all the way
    /// through `prepare` and `build` and then failed at the mount.
    ///
    /// The existing rollback test stages with `thin: None` and fails at
    /// `mkfs`, which never reaches either — so nothing pinned that the
    /// loop this stage attached comes back. It has to: `losetup --find`
    /// hands out a new device per call, so a permanently-failing thin
    /// stage walks the node out of loop devices exactly the way it used
    /// to walk it out of `/dev/nbdN`. And the order is load-bearing —
    /// the dm stack still holds the loop and the head, so a `losetup -d`
    /// (or `nbd-client -d`) before the teardown answers EBUSY and
    /// reclaims nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_thin_stage_tears_down_before_it_hands_back_the_loop() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(Arc::new(SharedLogStack(runner.clone())) as Arc<dyn ThinStack>),
            None,
        );
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup --find
        runner.push_reply(false, ""); // blkid: blank device
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(false, "mount refused"); // the failure

        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .expect_err("mount failed, so the stage failed");

        let seq: Vec<String> = runner.calls().iter().map(|c| c.join(" ")).collect();
        let pos = |needle: &str| {
            seq.iter()
                .position(|c| c.starts_with(needle))
                .unwrap_or_else(|| panic!("{needle:?} not in {seq:?}"))
        };
        assert!(
            pos("<stack> teardown tv") < pos("losetup -d"),
            "the stack holds the loop: tear it down first ({seq:?})"
        );
        // No head to hand back: the loop detach is the whole unwind.
        assert_eq!(seq.last().unwrap(), "losetup -d /dev/loop5", "{seq:?}");
        assert!(m.staged_device("tv").is_none());

        // The retry is what the rollback exists for: it re-attaches one
        // loop, not a second one. The pool file survives (it is the
        // volume's structure), so `prepare` probes for an association —
        // and finds none, because the rollback detached it.
        runner.push_reply(true, ""); // losetup -j: nothing attached
        runner.push_reply(true, "/dev/loop5\n"); // losetup --find: the same device back
        runner.push_reply(false, ""); // blkid
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .expect("the retry stages");
        let attaches = runner
            .calls()
            .iter()
            .filter(|c| c[0] == "losetup" && c.contains(&"--find".to_string()))
            .count();
        assert_eq!(attaches, 2, "one attach per attempt, never two");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn thin_cycle_starts_on_stage_and_stops_before_teardown() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::new(RecCycle::new(stack.clone())) as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop7\n"); // losetup
        runner.push_reply(false, ""); // blkid
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        let params = || {
            Some(ThinParams {
                size_bytes: 1 << 30,
                lineage: Some("tv-lin".into()),
                reclaim: Default::default(),
            })
        };
        m.stage("tv", &staging, None, false, params())
            .await
            .unwrap();
        {
            let log = stack.0.lock().unwrap();
            let start = log
                .iter()
                .find(|e| e.starts_with("cycle-start tv"))
                .expect("cycle started");
            assert!(start.contains("lineage=tv-lin"), "{start}");
            assert!(start.contains(&format!("at={staging}")), "{start}");
            // The stack was up before the cycle started.
            let build = log.iter().position(|e| e.starts_with("build tv")).unwrap();
            let cyc = log
                .iter()
                .position(|e| e.starts_with("cycle-start"))
                .unwrap();
            assert!(build < cyc, "{log:?}");
        }

        // Replayed stage on a live attachment (driver restart) restarts
        // the cycle idempotently — and re-asserts Invariant B's mask.
        // This branch returns before `stack.build`, which is where every
        // other stage re-derives the mask from observation, so the doc's
        // "a shim rebuilt outside `build` is re-masked from observation
        // rather than trusted" would otherwise hold only for cold builds
        // — leaving the driver restart (the one path a crashed
        // retarget's unmasked shim survives into) as the gap. Liveness
        // is judged from the dm stack now (there is no head to probe):
        // the RecStack's last event for tv is a build, so `built` says
        // live.
        m.stage("tv", &staging, None, false, params())
            .await
            .unwrap();
        {
            let log = stack.0.lock().unwrap();
            assert_eq!(
                log.iter().filter(|e| e.starts_with("cycle-start")).count(),
                2
            );
            let mask = log
                .iter()
                .rposition(|e| e.starts_with("mask "))
                .expect("the replay re-asserted the mask");
            let cyc = log
                .iter()
                .rposition(|e| e.starts_with("cycle-start"))
                .unwrap();
            assert!(
                mask < cyc,
                "the mask is asserted before the cycle resumes: {log:?}"
            );
        }

        // Unstage runs the final publish cycle FIRST (D-M13), then
        // stops the cycle BEFORE teardown.
        runner.push_reply(true, ""); // umount
        runner.push_reply(true, ""); // losetup -d
        m.unstage("tv", &staging).await.unwrap();
        let log = stack.0.lock().unwrap().clone();
        let finish = log.iter().position(|e| e == "cycle-finish tv").unwrap();
        let stop = log.iter().position(|e| e == "cycle-stop tv").unwrap();
        let teardown = log
            .iter()
            .position(|e| e.starts_with("teardown tv"))
            .unwrap();
        assert!(finish < stop && stop < teardown, "{log:?}");
    }

    /// Task 15e, §7 crash windows: the stage resolves a durably
    /// recorded in-flight swing BEFORE it touches the pool file — the
    /// resolve-slot call must precede the build (and it carries the
    /// pool file plus the carve layout, without which the runner could
    /// not construct the slot to sweep). After a flipped swing the live
    /// pool is the `.b` file, so anything that probes first reads the
    /// wrong world.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_stage_resolves_the_slot_before_touching_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::new(RecCycle::new(stack.clone())) as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop7\n"); // losetup
        runner.push_reply(false, ""); // blkid
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage(
            "tv",
            &staging,
            None,
            false,
            Some(ThinParams {
                size_bytes: 1 << 30,
                lineage: Some("tv-lin".into()),
                reclaim: Default::default(),
            }),
        )
        .await
        .unwrap();
        let log = stack.0.lock().unwrap();
        let resolve = log
            .iter()
            .position(|e| e.starts_with("cycle-resolve-slot tv"))
            .expect("the stage resolved the slot");
        let build = log.iter().position(|e| e.starts_with("build tv")).unwrap();
        assert!(resolve < build, "{log:?}");
        assert!(
            log[resolve].contains("lineage=tv-lin")
                && log[resolve].contains("tv.cache")
                && log[resolve].contains("layout=true"),
            "the resolver got the pool file and its carve: {}",
            log[resolve]
        );
    }

    /// The unstage detaches the loop CURRENTLY backing the pool file,
    /// not the one recorded at stage time: a §7 swing replaces the loop
    /// mid-life (the B slot's loop survives the canonicalize), and by
    /// unstage time the recorded number may back some other volume.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_unstage_detaches_the_loop_backing_the_file_not_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, _stack) = thin_mechanics(dir.path());
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup (recorded)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        runner.push_reply(true, ""); // umount
                                     // A swing replaced the loop: the file now answers with loop9.
        runner.push_reply(true, "/dev/loop9 1\n"); // losetup --associated
        m.unstage("tv", &staging).await.unwrap();
        let detached: Vec<_> = runner
            .calls()
            .iter()
            .filter(|c| c[0] == "losetup" && c[1] == "-d")
            .map(|c| c[2].clone())
            .collect();
        assert_eq!(
            detached,
            ["/dev/loop9"],
            "the file's live association wins over the stale record"
        );
    }

    /// Task 15i: the §7 dials parsed from the volume context reach the
    /// CycleSpec the runner starts with — the whole per-volume override
    /// path is just this plumbing (persisted in ThinState across the
    /// restage replay, so a driver restart keeps the dials too).
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_stage_threads_reclaim_dials_into_the_cycle_spec() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::new(RecCycle::new(stack.clone())) as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop7\n"); // losetup
        runner.push_reply(false, ""); // blkid
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        let params = || {
            Some(ThinParams {
                size_bytes: 1 << 30,
                lineage: Some("tv-lin".into()),
                reclaim: crate::agentcycle::ReclaimParams {
                    strategy: Some(crate::ReclaimStrategy::Swing),
                    timing: Some(crate::ReclaimTiming::Lazy),
                    range_threshold: Some(512),
                    swing_every_n: Some(4),
                },
            })
        };
        m.stage("tv", &staging, None, false, params())
            .await
            .unwrap();
        let dials = "strategy=swing timing=lazy threshold=512 swing_every=4";
        {
            let log = stack.0.lock().unwrap();
            let start = log
                .iter()
                .find(|e| e.starts_with("cycle-start tv"))
                .expect("cycle started");
            assert!(start.contains(dials), "{start}");
        }

        // The replayed stage (driver restart) restarts the cycle from
        // the PERSISTED ThinState — the dials must survive the round
        // trip through the state file. (Liveness comes from the dm
        // stack via `built`, not from any NBD device.)
        m.stage("tv", &staging, None, false, params())
            .await
            .unwrap();
        let log = stack.0.lock().unwrap();
        let replay = log
            .iter()
            .filter(|e| e.starts_with("cycle-start tv"))
            .nth(1)
            .expect("replayed cycle start");
        assert!(replay.contains(dials), "{replay}");
    }

    /// D-M13 failure side: an unpublished final cycle refuses the
    /// teardown entirely — no stop, no umount, state intact — so
    /// kubelet's retry resumes the upload instead of stranding it.
    #[tokio::test(flavor = "multi_thread")]
    async fn unstage_refuses_teardown_while_final_cycle_unpublished() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::new(StuckCycle) as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop9\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        let err = m.unstage("tv", &staging).await.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("final publish cycle") && msg.contains("freeze: EBUSY"),
            "{msg}"
        );
        // Nothing was torn down (StuckCycle::stop panics if called;
        // no umount/detach commands ran) and the state survives for
        // the retry.
        assert!(!stack
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("teardown")));
        assert!(m.state("tv").is_some());
    }

    /// Invariant F (task 15h): a FENCED stage's unstage skips the final
    /// publish cycle, quarantines the cache file beside itself and
    /// proceeds with teardown — the opposite fork from StuckCycle's
    /// transient refuse-and-retry. Mimics DmCycle's durable marker:
    /// `fenced` answers the carried epoch until `clear_fenced` — which
    /// the unstage path may only call after a successful quarantine
    /// rename.
    struct FencedCycle {
        epoch: u64,
        cleared: std::sync::atomic::AtomicBool,
        stops: std::sync::atomic::AtomicU32,
    }

    impl FencedCycle {
        fn new(epoch: u64) -> Self {
            Self {
                epoch,
                cleared: std::sync::atomic::AtomicBool::new(false),
                stops: std::sync::atomic::AtomicU32::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl CycleRunner for FencedCycle {
        fn start(&self, _spec: &CycleSpec) -> Result<(), String> {
            Ok(())
        }
        fn seed_baseline(
            &self,
            _lineage: &str,
            _base: &str,
            _epoch: Option<u64>,
        ) -> Result<(), String> {
            Ok(())
        }
        fn fenced(&self, _volume: &str) -> Option<u64> {
            // Durable-marker semantics: survives stop(), gone only after
            // the quarantine cleared it.
            (!self.cleared.load(std::sync::atomic::Ordering::SeqCst)).then_some(self.epoch)
        }
        fn clear_fenced(&self, _volume: &str) {
            self.cleared
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }
        async fn finish(&self, _volume: &str) -> Result<(), String> {
            panic!("a fenced unstage must never run the final publish cycle");
        }
        async fn stop(&self, _volume: &str) {
            self.stops.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fenced_unstage_quarantines_and_tears_down() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let fenced = Arc::new(FencedCycle::new(7));
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(fenced as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop9\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        // Unstage: no finish (FencedCycle::finish panics if reached),
        // full teardown, cache quarantined under the fenced name, state
        // gone — kubelet's retry finds a clean node.
        m.unstage("tv", &staging).await.unwrap();
        assert!(
            stack
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.starts_with("teardown tv")),
            "a fenced unstage still tears the stack down"
        );
        assert!(
            !dir.path().join("cache/tv.cache").exists(),
            "the fenced pool must not survive under its live name"
        );
        assert!(
            dir.path().join("cache/tv.cache.fenced-e7").exists(),
            "the fenced pool is quarantined (rename, delete later)"
        );
        assert!(m.state("tv").is_none(), "unstage completed");
    }

    /// B3 (task 15h): fencedness survives an unstage retry. The first
    /// attempt runs stop() and then FAILS the quarantine rename; the
    /// retry — running-map entry long gone — must still consult the
    /// durable fence, take the quarantine branch, and only then clear
    /// the marker.
    #[tokio::test(flavor = "multi_thread")]
    async fn fenced_unstage_retry_after_rename_failure_still_quarantines() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let fenced = Arc::new(FencedCycle::new(7));
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::clone(&fenced) as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop9\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        // Sabotage the quarantine: exhaust the suffix space, the one
        // collision the uniquifier gives up on. A single squatter is no
        // longer fatal — an expected repeat quarantine takes the next
        // free suffix instead of wedging the stage.
        let cache_dir = dir.path().join("cache");
        std::fs::create_dir_all(cache_dir.join("tv.cache.fenced-e7")).unwrap();
        for n in 2..=100 {
            std::fs::write(cache_dir.join(format!("tv.cache.fenced-e7.{n}")), b"old").unwrap();
        }
        let err = m.unstage("tv", &staging).await.unwrap_err();
        assert!(matches!(err, NodeError::Io(..)), "{err}");
        assert!(m.state("tv").is_some(), "state survives for the retry");
        assert_eq!(
            fenced.stops.load(Ordering::SeqCst),
            1,
            "stop() ran before the rename failed — the running map is gone"
        );
        assert!(
            !fenced.cleared.load(Ordering::SeqCst),
            "the marker must NOT clear on a failed rename"
        );

        // Retry: the durable fence still answers, the quarantine branch
        // runs again, and with one suffix freed the rename lands there —
        // never over the file already holding the name.
        std::fs::remove_file(cache_dir.join("tv.cache.fenced-e7.50")).unwrap();
        m.unstage("tv", &staging).await.unwrap();
        assert!(
            cache_dir.join("tv.cache.fenced-e7.50").exists(),
            "the retry still quarantined the fenced pool"
        );
        assert_eq!(
            std::fs::read(cache_dir.join("tv.cache.fenced-e7.2")).unwrap(),
            b"old",
            "an earlier quarantine is never overwritten"
        );
        assert!(!dir.path().join("cache/tv.cache").exists());
        assert!(
            fenced.cleared.load(Ordering::SeqCst),
            "the marker clears only after the rename succeeded"
        );
        assert!(m.state("tv").is_none());
    }

    /// Nit (task 15h): an epoch-unknown fence (0) quarantines under
    /// `fenced-eunknown`, not a fake `e0`.
    #[tokio::test(flavor = "multi_thread")]
    async fn unknown_epoch_quarantine_is_named_eunknown() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::new(FencedCycle::new(0)) as Arc<dyn CycleRunner>),
        );
        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop9\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();
        m.unstage("tv", &staging).await.unwrap();
        assert!(dir.path().join("cache/tv.cache.fenced-eunknown").exists());
    }

    /// B4a (task 15h): a Held/error on the LINEAGE serve fails the thin
    /// stage outright — no degradation to an epoch-less unfenced stage.
    struct HeldHandoff;

    #[async_trait::async_trait]
    impl crate::handoff::Handoff for HeldHandoff {
        async fn ensure_served(&self, _volume: &str, _node_holder: bool) -> Option<u64> {
            None
        }
        async fn serve_required(
            &self,
            volume: &str,
            node_holder: bool,
        ) -> Result<Option<u64>, String> {
            assert!(node_holder, "the lineage serve runs as the node");
            Err(format!("volume {volume:?} is owned by \"node-b\""))
        }
        async fn release(&self, _volume: &str) {}
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn held_lineage_serve_fails_the_thin_stage() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, _stack) = thin_mechanics(dir.path());
        let m = m.with_handoff(Arc::new(HeldHandoff));
        runner.push_reply(true, ""); // nbd-client (head attach)
        let staging = dir.path().join("stage/tv").display().to_string();
        let err = m
            .stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap_err();
        assert!(matches!(err, NodeError::LineageServe(..)), "{err}");
        assert!(
            err.to_string().contains("node-b"),
            "the refusal names the holder: {err}"
        );
        assert!(
            m.state("tv").is_none(),
            "a refused lineage serve must stage nothing"
        );
        assert!(
            !dir.path().join("cache/tv.cache").exists(),
            "no cache file for a refused stage"
        );
    }

    /// Origins recorder writing into the shared RecStack log so tests
    /// assert the D-M15 choreography (init+seed before the cache file
    /// exists, pointer-row reuse on surviving caches).
    struct RecOrigins {
        log: Arc<RecStack>,
        /// What an initializing POST resolves to (`snap/gN`); `None` =
        /// no published generations yet.
        init: Option<&'static str>,
        /// Whether the pointer row is already initialized (the reuse
        /// branch's durable marker).
        initialized: bool,
    }

    #[async_trait::async_trait]
    impl crate::handoff::Origins for RecOrigins {
        async fn init_origin(&self, lineage: &str) -> Result<Option<String>, String> {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("origins-init {lineage}"));
            Ok(self.init.map(str::to_string))
        }
        async fn origin_ref(&self, lineage: &str) -> Result<Option<String>, String> {
            self.log
                .0
                .lock()
                .unwrap()
                .push(format!("origins-ref {lineage}"));
            Ok(self
                .initialized
                .then(|| self.init.unwrap_or("snap/g0").to_string()))
        }
    }

    /// A real `nbd-client` attach creates the device's sysfs `pid` file
    /// before returning, which is what makes the SECOND allocation in a
    /// D-M11 stage pick a different device. The plain mock lacks that
    /// side effect, so this wrapper adds it.
    struct PidRunner {
        inner: Arc<MockRunner>,
        sysfs: PathBuf,
    }

    #[async_trait::async_trait]
    impl Runner for PidRunner {
        async fn run(&self, cmd: &str, args: &[String]) -> std::io::Result<Output> {
            let out = self.inner.run(cmd, args).await?;
            if cmd == "nbd-client" && !args.iter().any(|a| a == "-d") {
                if let Some(dev) = args.iter().find(|a| a.starts_with("/dev/nbd")) {
                    let name = dev.rsplit('/').next().unwrap();
                    let _ = std::fs::write(self.sysfs.join(name).join("pid"), "1");
                }
            }
            Ok(out)
        }
    }

    fn origin_mechanics(
        dir: &Path,
        init: Option<&'static str>,
        initialized: bool,
    ) -> (NodeMechanics, Arc<MockRunner>, Arc<RecStack>) {
        let (m, runner, stack, _) = origin_mechanics_with_cycle(dir, init, initialized, None);
        (m, runner, stack)
    }

    /// Like [`origin_mechanics`] but hands the RecCycle back (to
    /// pre-register a surviving agent record) and optionally pre-seeds
    /// one: `record` = the (lineage, epoch) a previous stage left.
    fn origin_mechanics_with_cycle(
        dir: &Path,
        init: Option<&'static str>,
        initialized: bool,
        record: Option<(&str, Option<u64>)>,
    ) -> (NodeMechanics, Arc<MockRunner>, Arc<RecStack>, Arc<RecCycle>) {
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let config = cfg(dir, false);
        let pid_runner = Arc::new(PidRunner {
            inner: runner.clone(),
            sysfs: config.sysfs_block.clone(),
        });
        let mut cycle = RecCycle::new(stack.clone());
        if let Some((lin, epoch)) = record {
            cycle = cycle.with_record(lin, epoch);
        }
        let cycle = Arc::new(cycle);
        let m = NodeMechanics::new(
            config,
            pid_runner as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(cycle.clone() as Arc<dyn CycleRunner>),
        )
        .with_origins(Arc::new(RecOrigins {
            log: stack.clone(),
            init,
            initialized,
        }));
        (m, runner, stack, cycle)
    }

    fn lin_tp(size: u64) -> ThinParams {
        ThinParams {
            size_bytes: size,
            lineage: Some("tv-lin".into()),
            reclaim: Default::default(),
        }
    }

    /// D-M15 fresh-pool restage: the origin pointer is initialized and
    /// the agent baseline seeded BEFORE the pool file exists, the
    /// lineage's `<lineage>/origin` export is attached — the stage's
    /// ONLY NBD attach (§5: no head) — the thin spec carries it as the
    /// external origin, and unstage detaches it.
    #[tokio::test(flavor = "multi_thread")]
    async fn fresh_thin_stage_wires_external_origin_and_seeds_baseline() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = origin_mechanics(dir.path(), Some("snap/g4"), false);
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin → /dev/nbd0)
        runner.push_reply(true, "ext4\n"); // blkid: fs visible via origin
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        let log = stack.0.lock().unwrap().clone();
        let init = log
            .iter()
            .position(|e| e == "origins-init tv-lin")
            .expect("pointer initialized on the fresh-cache stage");
        let seed = log
            .iter()
            .position(|e| e.starts_with("cycle-seed tv-lin snap/g4"))
            .expect("baseline seeded on the SAME generation the pointer names");
        let build = log.iter().position(|e| e.starts_with("build tv")).unwrap();
        assert!(init < seed && seed < build, "{log:?}");
        assert!(
            log.iter()
                .any(|e| e.starts_with("build tv") && e.ends_with("origin=/dev/nbd0")),
            "thin spec carries the origin attach: {log:?}"
        );
        assert!(
            log.iter()
                .any(|e| e.starts_with("cycle-start tv") && e.contains("origin=true")),
            "cycle told the origin is attached: {log:?}"
        );
        let cmds = runner.calls();
        assert!(
            cmds.iter()
                .any(|c| c[0] == "nbd-client" && c.contains(&"tv-lin/origin".to_string())),
            "lineage origin export attached: {cmds:?}"
        );

        // Unstage: teardown, then the origin attach — the only NBD
        // attachment there is — detaches.
        runner.push_reply(true, ""); // umount
        runner.push_reply(true, ""); // losetup -d
        runner.push_reply(true, ""); // nbd-client -d origin
        m.unstage("tv", &staging).await.unwrap();
        let cmds = runner.calls();
        let detached: Vec<&Vec<String>> = cmds
            .iter()
            .filter(|c| c[0] == "nbd-client" && c[1] == "-d")
            .collect();
        assert_eq!(
            detached.iter().map(|c| c[2].as_str()).collect::<Vec<_>>(),
            vec!["/dev/nbd0"],
            "the origin attach detaches with the stack; no head exists"
        );
    }

    /// Invariant F (task 15h): a thin stage with a lineage serves it
    /// under the NODE's identity, and the granted epoch threads into
    /// both the seeded baseline record and the cycle spec — the fence
    /// token every agent transaction will carry.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_stage_serves_lineage_as_node_and_threads_the_epoch() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = origin_mechanics(dir.path(), Some("snap/g4"), false);
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        // The lineage was served under the node's identity, before the
        // origin/baseline work.
        let seq: Vec<String> = runner.calls().iter().map(|c| c.join(" ")).collect();
        assert!(
            seq.iter().any(|c| c == "<handoff> serve-as-node tv-lin"),
            "lineage lease held under the node identity: {seq:?}"
        );
        // …and the granted epoch (RecorderHandoff grants 7) reached the
        // seeded record and the cycle spec.
        let log = stack.0.lock().unwrap().clone();
        assert!(
            log.iter()
                .any(|e| e.starts_with("cycle-seed tv-lin snap/g4") && e.ends_with("epoch=7")),
            "seeded baseline carries the stage epoch: {log:?}"
        );
        assert!(
            log.iter()
                .any(|e| e.starts_with("cycle-start tv") && e.contains("epoch=7")),
            "cycle spec carries the stage epoch: {log:?}"
        );

        // Unstage releases the LINEAGE lease (held since stage) — and
        // ONLY that lease: a thin volume never held a volume-level one,
        // there being no head export to serve (§5).
        runner.push_reply(true, ""); // umount
        runner.push_reply(true, ""); // losetup -d
        runner.push_reply(true, ""); // nbd-client -d origin
        m.unstage("tv", &staging).await.unwrap();
        let seq: Vec<String> = runner.calls().iter().map(|c| c.join(" ")).collect();
        assert!(
            seq.iter().any(|c| c == "<handoff> release tv-lin"),
            "unstage releases the lineage lease: {seq:?}"
        );
        assert!(
            !seq.iter().any(|c| c == "<handoff> release tv"),
            "no volume-level lease was ever held: {seq:?}"
        );
    }

    /// Surviving cache: the pool's structure is intact and its origin
    /// dependency is discovered from the pointer row being initialized
    /// — attached verbatim, no re-init, no re-seed (local state is
    /// live).
    #[tokio::test(flavor = "multi_thread")]
    async fn surviving_cache_restage_reuses_the_origin_pointer() {
        let dir = tempfile::tempdir().unwrap();
        // The pool survives from a previous stage on this node, WITH the
        // agent record that stage seeded (epoch None — this rig has no
        // handoff, so the fresh stage's grant is None too and the
        // Invariant F stage-time validation passes).
        let (m, runner, stack, _cycle) =
            origin_mechanics_with_cycle(dir.path(), Some("snap/g9"), true, Some(("tv-lin", None)));
        let cache_dir = m.cfg.cache_dir.clone().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("tv.cache"), b"x").unwrap();

        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        let log = stack.0.lock().unwrap().clone();
        assert!(log.contains(&"origins-ref tv-lin".to_string()), "{log:?}");
        // The POST fires here too — for an initialized pointer its
        // no-op arm is a pure export reconcile (the origin export dies
        // with every release and only the POST rebuilds it) — but it
        // must never RE-SEED: live local state is never overwritten.
        assert!(
            log.contains(&"origins-init tv-lin".to_string()),
            "the surviving-pool attach must reconcile the origin export: {log:?}"
        );
        assert!(
            !log.iter().any(|e| e.starts_with("cycle-seed")),
            "live local state is never overwritten: {log:?}"
        );
        assert!(
            log.iter()
                .any(|e| e.starts_with("build tv") && e.ends_with("origin=/dev/nbd0")),
            "{log:?}"
        );
    }

    /// Invariant F, stage side (PR #76 convergence): a surviving pool
    /// whose recorded stage epoch differs from the fresh lease grant was
    /// superseded by a foreign hold — the stage quarantines it (under
    /// its RECORDED epoch) and fresh-seeds from the pointer instead of
    /// mounting a stale hybrid of local mappings over the newer shared
    /// origin.
    #[tokio::test(flavor = "multi_thread")]
    async fn restage_with_stale_record_epoch_quarantines_and_reseeds() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack, _cycle) = origin_mechanics_with_cycle(
            dir.path(),
            Some("snap/g9"),
            true,
            Some(("tv-lin", Some(3))), // seeded at epoch 3…
        );
        // …but RecorderHandoff grants 7 now: a foreign hold intervened.
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));
        let cache_dir = m.cfg.cache_dir.clone().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("tv.cache"), b"x").unwrap();

        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        assert!(
            cache_dir.join("tv.cache.fenced-e3").exists(),
            "the superseded pool is quarantined under its recorded epoch"
        );
        let log = stack.0.lock().unwrap().clone();
        assert!(
            log.iter().any(|e| e == "origins-init tv-lin"),
            "the stage fell through to the fresh-pool path: {log:?}"
        );
        assert!(
            log.iter()
                .any(|e| e.starts_with("cycle-seed tv-lin snap/g9") && e.ends_with("epoch=7")),
            "re-seeded under the NEW grant: {log:?}"
        );
    }

    /// Invariant F, stage side: matching epochs (same-holder re-acquire
    /// preserves the epoch; any foreign hold bumps it) adopt the
    /// surviving pool in place — no quarantine, no re-init, no re-seed.
    #[tokio::test(flavor = "multi_thread")]
    async fn restage_with_matching_record_epoch_adopts_the_pool() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack, _cycle) = origin_mechanics_with_cycle(
            dir.path(),
            Some("snap/g9"),
            true,
            Some(("tv-lin", Some(7))),
        );
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));
        let cache_dir = m.cfg.cache_dir.clone().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("tv.cache"), b"x").unwrap();

        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        assert!(
            cache_dir.join("tv.cache").exists(),
            "the validated pool is adopted in place"
        );
        let log = stack.0.lock().unwrap().clone();
        assert!(log.contains(&"origins-ref tv-lin".to_string()), "{log:?}");
        // The POST reconciles the origin export (no-op on the pointer —
        // it is already initialized); what an adopted pool must never
        // do is RE-SEED over live local state.
        assert!(
            log.contains(&"origins-init tv-lin".to_string()),
            "the adopted-pool attach must reconcile the origin export: {log:?}"
        );
        assert!(
            !log.iter().any(|e| e.starts_with("cycle-seed")),
            "an adopted pool is never re-seeded: {log:?}"
        );
    }

    /// Invariant F, stage side: a surviving pool with NO trustworthy
    /// agent record is unvalidatable — fail closed: quarantine under
    /// `-eunknown` and fresh-seed from the pointer.
    #[tokio::test(flavor = "multi_thread")]
    async fn restage_without_agent_record_quarantines_unvalidatable_pool() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack, _cycle) =
            origin_mechanics_with_cycle(dir.path(), Some("snap/g9"), true, None);
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));
        let cache_dir = m.cfg.cache_dir.clone().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("tv.cache"), b"x").unwrap();

        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        assert!(cache_dir.join("tv.cache.fenced-eunknown").exists());
        let log = stack.0.lock().unwrap().clone();
        assert!(
            log.iter().any(|e| e == "origins-init tv-lin")
                && log
                    .iter()
                    .any(|e| e.starts_with("cycle-seed tv-lin snap/g9")),
            "unvalidatable pool falls through to the fresh path: {log:?}"
        );
    }

    /// #76 review: a quarantine must never overwrite an earlier one —
    /// the earlier file may be the only copy of that pool. A collision
    /// (expected after any SAGT version bump: every record decodes
    /// Corrupt, so every pool quarantines under `-eunknown`) uniquifies
    /// with a `.2` suffix and the stage proceeds, rather than wedging
    /// behind operator intervention.
    #[tokio::test(flavor = "multi_thread")]
    async fn quarantine_never_overwrites_an_earlier_quarantine() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack, _cycle) =
            origin_mechanics_with_cycle(dir.path(), Some("snap/g9"), true, None);
        let m = m.with_handoff(Arc::new(RecorderHandoff(runner.clone())));
        let cache_dir = m.cfg.cache_dir.clone().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("tv.cache"), b"new pool").unwrap();
        // An earlier unvalidatable pool already sits in quarantine.
        std::fs::write(cache_dir.join("tv.cache.fenced-eunknown"), b"old pool").unwrap();

        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // nbd-client (origin)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        assert_eq!(
            std::fs::read(cache_dir.join("tv.cache.fenced-eunknown")).unwrap(),
            b"old pool",
            "the earlier quarantine survives untouched"
        );
        assert_eq!(
            std::fs::read(cache_dir.join("tv.cache.fenced-eunknown.2")).unwrap(),
            b"new pool",
            "the colliding pool quarantines under a uniquified name"
        );
        let log = stack.0.lock().unwrap().clone();
        assert!(
            log.iter().any(|e| e == "origins-init tv-lin")
                && log
                    .iter()
                    .any(|e| e.starts_with("cycle-seed tv-lin snap/g9")),
            "the stage proceeds to the fresh path: {log:?}"
        );
    }

    /// Invariant F, stage side: a durable fenced marker left behind by a
    /// crash (the fenced unstage never completed) quarantines the pool
    /// at the NEXT stage — the marker's epoch names the quarantine, and
    /// the marker clears only after the rename landed.
    #[tokio::test(flavor = "multi_thread")]
    async fn restage_with_durable_fenced_marker_quarantines() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let fenced = Arc::new(FencedCycle::new(5));
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::clone(&fenced) as Arc<dyn CycleRunner>),
        );
        let cache_dir = m.cfg.cache_dir.clone().unwrap();
        std::fs::create_dir_all(&cache_dir).unwrap();
        std::fs::write(cache_dir.join("tv.cache"), b"x").unwrap();

        runner.push_reply(true, ""); // nbd-client
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop9\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();

        assert!(cache_dir.join("tv.cache.fenced-e5").exists());
        assert!(
            cache_dir.join("tv.cache").exists(),
            "a fresh pool takes over the live name"
        );
        assert!(
            fenced.cleared.load(Ordering::SeqCst),
            "the marker clears only after the quarantine rename landed"
        );
    }

    /// A brand-new volume (no published generations, no clone) stages
    /// with NO NBD attach at all (§5: no head; D-M11: no history, no
    /// origin) — a plain thin table over the local pool.
    #[tokio::test(flavor = "multi_thread")]
    async fn fresh_volume_without_history_stages_plain() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = origin_mechanics(dir.path(), None, false);
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, ""); // blkid: blank
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(lin_tp(1 << 30)))
            .await
            .unwrap();
        let log = stack.0.lock().unwrap().clone();
        assert!(
            log.iter()
                .any(|e| e.starts_with("build tv") && e.ends_with("origin=-")),
            "{log:?}"
        );
        let attaches = runner
            .calls()
            .iter()
            .filter(|c| c[0] == "nbd-client" && c[1] != "-d")
            .count();
        assert_eq!(attaches, 0, "no head, and no origin without history");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn thin_block_volumes_get_no_cycle() {
        let dir = tempfile::tempdir().unwrap();
        let runner = Arc::new(MockRunner::default());
        let stack = Arc::new(RecStack::default());
        let m = NodeMechanics::new(
            cfg(dir.path(), false),
            runner.clone() as Arc<dyn Runner>,
            Some(stack.clone() as Arc<dyn ThinStack>),
            Some(Arc::new(RecCycle::new(stack.clone())) as Arc<dyn CycleRunner>),
        );
        // fallocate, losetup — no head attach, no fs step for block.
        runner.push_reply(true, "");
        runner.push_reply(true, "/dev/loop8\n");
        m.stage(
            "bv",
            "/stage/bv",
            None,
            true,
            Some(ThinParams {
                size_bytes: 1 << 30,
                lineage: Some("bv-lin".into()),
                reclaim: Default::default(),
            }),
        )
        .await
        .unwrap();
        // No filesystem → nothing to freeze → no embedded cycle.
        assert!(!stack
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.starts_with("cycle-start")));
    }

    /// Total local loss: the node lost the pool file and rebooted.
    /// Under publish-only (§5) that is no longer a refusal — the old
    /// §15.7 "cache file lost" class died with the writeback head. The
    /// stage warns and rebuilds a fresh pool (new fallocate, mkfs of a
    /// blank pool); what is lost is exactly the unpublished delta,
    /// bounded by the publish cadence.
    #[tokio::test(flavor = "multi_thread")]
    async fn thin_pool_loss_restages_fresh_instead_of_refusing() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = thin_mechanics(dir.path());
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(true, "/dev/loop5\n"); // losetup
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount
        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();

        // Reboot (dm state gone) + the pool file gone with it.
        stack.0.lock().unwrap().push("reboot".into());
        std::fs::remove_file(dir.path().join("cache/tv.cache")).unwrap();
        runner.push_reply(true, ""); // fallocate: a FRESH pool file
        runner.push_reply(true, "/dev/loop6\n"); // losetup
        runner.push_reply(false, ""); // blkid: blank pool
        runner.push_reply(true, ""); // mkfs
        runner.push_reply(true, ""); // mount
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .expect("pool loss re-stages fresh; it does not refuse");
        assert_eq!(
            runner
                .calls()
                .iter()
                .filter(|c| c[0] == "fallocate")
                .count(),
            2,
            "the fresh pool file was recreated"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn thin_stage_falls_back_to_buffered_loop() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner, stack) = thin_mechanics(dir.path());
        runner.push_reply(true, ""); // fallocate
        runner.push_reply(false, ""); // losetup --direct-io=on: unsupported
        runner.push_reply(true, "/dev/loop9\n"); // losetup (buffered)
        runner.push_reply(true, "ext4\n"); // blkid
        runner.push_reply(true, ""); // mount

        let staging = dir.path().join("stage/tv").display().to_string();
        m.stage("tv", &staging, None, false, Some(tp(1 << 30)))
            .await
            .unwrap();
        let calls = runner.calls();
        assert!(calls[1].contains(&"--direct-io=on".to_string()));
        assert!(!calls[2].contains(&"--direct-io=on".to_string()));
        // The buffered loop is the one the stack is built over.
        assert!(stack.0.lock().unwrap()[0].contains("fast=/dev/loop9"));
    }

    #[tokio::test]
    async fn thin_stage_without_thin_support_is_refused_before_attaching() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        let err = m
            .stage("tv", "/stage/tv", None, false, Some(tp(1 << 30)))
            .await
            .unwrap_err();
        assert!(matches!(err, NodeError::ThinUnavailable), "{err}");
        assert!(runner.calls().is_empty());
    }

    #[test]
    fn ro_mount_opts_skip_journal_replay() {
        assert_eq!(ro_mount_opts("ext4"), "ro,noload");
        assert_eq!(ro_mount_opts("xfs"), "ro,norecovery");
        assert_eq!(ro_mount_opts("btrfs"), "ro");
        assert_eq!(export_for("pvc-1"), ("pvc-1/head".into(), false));
        assert_eq!(export_for("ro:v/snap/s"), ("v/snap/s".into(), true));
    }

    #[tokio::test]
    async fn readonly_publish_remounts_ro() {
        let dir = tempfile::tempdir().unwrap();
        let (m, runner) = mechanics(dir.path(), false);
        let target = dir.path().join("pod/vol");
        m.publish("v", "/stage/v", target.to_str().unwrap(), false, true)
            .await
            .unwrap();
        let calls = runner.calls();
        assert_eq!(calls.len(), 2);
        assert!(calls[1].join(" ").contains("remount,bind,ro"));
    }
}

//! Embedded agent cycle per staged thin volume (CSI design 6h-2,
//! V3.2 §9): while a thin volume is staged and mounted, a background
//! task runs the agent crate's `CycleEngine` against the staged stack —
//! fsfreeze → thin snapshot → objectize → publish into the volume's
//! **lineage volume** — on a cadence, plus immediately whenever a
//! CSI CreateSnapshot files a snapshot request. After a request-driven
//! cycle the published generation is *aliased* to the requested name,
//! which is what flips the controller's `ready_to_use`.
//!
//! The trait exists so [`crate::nodemech::NodeMechanics`] plans the
//! start/stop choreography against a recorder in plain `cargo test`;
//! the real implementation drives kernel ioctls and the control plane.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Everything the runner needs that is per-volume; node-level knobs
/// (API URL, cadence, chunk size) live in the runner itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CycleSpec {
    /// CSI volume id (provision names derive from it).
    pub volume: String,
    /// Lineage volume name (server-side publication target).
    pub lineage: String,
    /// Mounted filesystem on the shim over the thin origin (freeze
    /// target).
    pub staging_path: String,
    /// Origin size in bytes.
    pub size_bytes: u64,
    /// D-M15: the device path of the lineage's repointable origin
    /// export, when the stage attached it as the thin device's external
    /// origin (e.g. `/dev/nbd3`). `None` = brand-new volume (no
    /// generation published at stage time): the cycle loop then never
    /// runs an advance — a reclaim with no origin attach would unmap
    /// blocks nothing can re-serve. The path (not just the fact of it)
    /// rides here because the §7 swing's B pool names the same attach
    /// as ITS external origin: both slots share the one attach, and
    /// advances repoint what it serves server-side.
    pub origin_dev: Option<String>,
    /// The lineage lease epoch granted to THIS NODE at stage time
    /// (Invariant F, task 15h): stamped into every agent transaction so
    /// a superseded stage fails closed. `None` = unfenced (lease-less
    /// deployments, pre-15h servers).
    pub epoch: Option<u64>,
    /// Per-StorageClass §7 compaction-policy dials (task 15i), each
    /// overriding the node default when set.
    pub reclaim: ReclaimParams,
    /// The pool's backing file — §8.1 rule 2's second exhaustion
    /// surface: with the §5 local pool, the filesystem this file lives
    /// on can starve a pool write, so the engine must watch its free
    /// bytes alongside pool occupancy. `None` = the pool sits on a raw
    /// device (not the CSI shape).
    pub pool_file: Option<PathBuf>,
    /// The `[tmeta][data]` carve of the pool file, exactly as
    /// `provision::build` computed it — what makes the pool file a §7
    /// SLOT: the swing's B side must be carved at these offsets, and
    /// a mismatch reads pool metadata as garbage on the next resume.
    /// Callers derive it with [`crate::provision::fast_layout`] from
    /// the same inputs the build used, never by hand. `None` (with a
    /// `pool_file`) = layout unknown; the volume stays port-pinned.
    pub pool_layout: Option<crate::provision::FastLayout>,
    /// §7.1: whether the staged stack carries the top-of-stack read
    /// cache, i.e. the guest mounts the rcache rather than the shim.
    /// The engine's per-cycle Invariant B re-assert must then cover
    /// the rcache too — the cache advertises its own discard limits,
    /// so a mask verified on the shim alone is invisible to the guest.
    pub read_cache: bool,
    /// Whether this stage mounted the filesystem read-only (a `ro:`
    /// snapshot reference). §8.1 rule 3 is not armed for such a volume:
    /// there is nothing to protect from exhaustion, and — the part that
    /// matters — arming it would make "this mount is read-only"
    /// ambiguous, so a restarted agent could not tell rule 3's own
    /// remount from a deliberately read-only snapshot and would happily
    /// remount one read-write after an advance.
    pub readonly: bool,
}

/// The §7 compaction-policy dials as they ride the StorageClass
/// parameters → volume context → stage (task 15i). All optional: an
/// absent dial keeps the node-level default ([`DmCycle::policy`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ReclaimParams {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        with = "strategy_dial"
    )]
    pub strategy: Option<crate::ReclaimStrategy>,
    #[serde(default, skip_serializing_if = "Option::is_none", with = "timing_dial")]
    pub timing: Option<crate::ReclaimTiming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range_threshold: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing_every_n: Option<u32>,
}

/// The StorageClass spellings of the enum dials — one place, shared by
/// the serde mirrors and the test logs.
pub(crate) fn strategy_str(s: crate::ReclaimStrategy) -> &'static str {
    match s {
        crate::ReclaimStrategy::Auto => "auto",
        crate::ReclaimStrategy::Port => "port",
        crate::ReclaimStrategy::Swing => "swing",
    }
}

pub(crate) fn timing_str(t: crate::ReclaimTiming) -> &'static str {
    match t {
        crate::ReclaimTiming::Eager => "eager",
        crate::ReclaimTiming::Lazy => "lazy",
    }
}

/// serde mirror for the agent crate's serde-less policy enums, via
/// their strict StorageClass spellings.
macro_rules! dial_serde {
    ($mod:ident, $ty:ty, $to_str:path) => {
        mod $mod {
            use serde::{Deserialize, Deserializer, Serialize, Serializer};
            pub fn serialize<S: Serializer>(v: &Option<$ty>, s: S) -> Result<S::Ok, S::Error> {
                v.map($to_str).serialize(s)
            }
            pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<$ty>, D::Error> {
                Option::<String>::deserialize(d)?
                    .map(|raw| raw.parse().map_err(serde::de::Error::custom))
                    .transpose()
            }
        }
    };
}
dial_serde!(strategy_dial, crate::ReclaimStrategy, super::strategy_str);
dial_serde!(timing_dial, crate::ReclaimTiming, super::timing_str);

/// `swingEveryNAdvances` / `SKYNBD_RECLAIM_SWING_EVERY_N`, parsed once
/// for both surfaces. Unset means "never force a major compaction" (§7
/// aging is opt-in); 0 would read as "every advance is aged", which is
/// `reclaimStrategy: swing` spelled obscurely — and one keystroke from
/// the value that disables aging. The two surfaces configure the same
/// dial on the same engine, so they must refuse the same values with
/// the same words.
pub fn parse_swing_every_n(raw: &str) -> Result<u32, String> {
    match raw.parse::<u32>() {
        Ok(n) if n > 0 => Ok(n),
        Ok(_) => Err("swingEveryNAdvances must be >= 1 (leave it unset to \
                      never force a swing; use reclaimStrategy: swing to \
                      swing on every advance)"
            .to_string()),
        Err(_) => Err(format!("invalid swingEveryNAdvances {raw:?}")),
    }
}

/// `reclaimRangeThreshold` / `SKYNBD_RECLAIM_RANGE_THRESHOLD`, likewise
/// shared. 0 is meaningful here (any shared range at all prefers the
/// swing), so only the parse is a refusal.
pub fn parse_range_threshold(raw: &str) -> Result<usize, String> {
    raw.parse()
        .map_err(|_| format!("invalid reclaimRangeThreshold {raw:?}"))
}

impl ReclaimParams {
    pub const KEYS: [&'static str; 4] = [
        "reclaimStrategy",
        "reclaimTiming",
        "reclaimRangeThreshold",
        "swingEveryNAdvances",
    ];

    /// Parse the dials out of a StorageClass-parameter / volume-context
    /// map. Strict: a present-but-invalid value is an error naming the
    /// offending value; absent keys stay `None`.
    pub fn from_map(m: &HashMap<String, String>) -> Result<Self, String> {
        Ok(Self {
            strategy: m.get("reclaimStrategy").map(|s| s.parse()).transpose()?,
            timing: m.get("reclaimTiming").map(|s| s.parse()).transpose()?,
            range_threshold: m
                .get("reclaimRangeThreshold")
                .map(|s| parse_range_threshold(s))
                .transpose()?,
            swing_every_n: m
                .get("swingEveryNAdvances")
                .map(|s| parse_swing_every_n(s))
                .transpose()?,
        })
    }

    /// No dial set — the shape a raw volume must have (the dials are
    /// thin-mode-only) and the shape that emits nothing into the
    /// volume context.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Node-level defaults overridden per-volume: the §7 policy the
    /// engine actually runs.
    pub fn apply_to(&self, mut p: crate::ReclaimPolicy) -> crate::ReclaimPolicy {
        if let Some(s) = self.strategy {
            p.strategy = s;
        }
        if let Some(t) = self.timing {
            p.timing = t;
        }
        if let Some(r) = self.range_threshold {
            p.range_threshold = r;
        }
        if let Some(n) = self.swing_every_n {
            p.swing_every_n = Some(n);
        }
        p
    }
}

#[async_trait::async_trait]
pub trait CycleRunner: Send + Sync {
    /// Start the embedded cycle for a staged, mounted thin volume.
    /// Idempotent: a volume whose cycle is already running is a no-op
    /// (NodeStage replays after a driver restart hit this).
    fn start(&self, spec: &CycleSpec) -> Result<(), String>;
    /// §7 crash windows meet the stage: resolve a durably recorded
    /// in-flight swing for this lineage BEFORE anything probes or
    /// attaches the pool file. A crash mid-swing leaves the durable
    /// `swing_flipped` flag naming the live slot, and the stage cannot
    /// build ahead of it:
    ///
    ///   - flag = flipped: the live pool is the `.b` file. A stage that
    ///     probed first would either resume the condemned A pool
    ///     (serving stale content and dropping the post-capture writes
    ///     the swing moved to B) or — if the crash fell between the A
    ///     delete and the rename — read "pool file lost" and rebuild
    ///     fresh, destroying B and the acked writes inside it.
    ///   - flag = armed: slot B is garbage, and on a same-process crash
    ///     the shim may still be aimed at it (possibly under a freeze
    ///     that outlived its owner); the sweep is also that repair.
    ///
    /// Runs the idempotent sweep ONLY — the flag itself is cleared by
    /// the engine's own recovery (which re-sweeps a now-clean slot and
    /// does the `advances_since_swing` accounting), so the state
    /// transitions keep a single owner. A failed sweep fails the stage:
    /// kubelet retries into the same resolution rather than building
    /// on an unresolved slot. No-op for recorders, non-slot stacks,
    /// and lineages with no (or no trustworthy) record.
    fn resolve_slot(&self, spec: &CycleSpec) -> Result<(), String> {
        let _ = spec;
        Ok(())
    }
    /// D-M11: durably seed the agent's state for `lineage` with a
    /// rebaselined pair on `base` (a `snap/gN` ref name) — the shape the
    /// cycle engine already understands as "remote baseline, no local
    /// member", so the first cycle prepares from `base` and uploads only
    /// the locally provisioned delta. Called by a fresh-cache thin stage
    /// right after cloning that generation as the external origin, and
    /// BEFORE the cache file is created: a crashed stage then retries
    /// with clone + seed already durable, and any record a previous
    /// pool left is overwritten — a fresh pool voids all local state.
    /// `epoch` is the stage-time lease epoch (task 15h), persisted with
    /// the record.
    fn seed_baseline(&self, lineage: &str, base: &str, epoch: Option<u64>) -> Result<(), String>;
    /// Invariant F stage-time validation: the stage-time lease epoch the
    /// persisted agent record for `lineage` carries. `Some(epoch)` = a
    /// decodable record exists and this is its epoch field; `None` = no
    /// trustworthy record (absent or corrupt). NodeStage compares this
    /// against the freshly granted epoch before adopting a persisted
    /// pool: same-holder re-acquire preserves the epoch and any foreign
    /// hold bumps it, so equality is exactly "no foreign writer has held
    /// this lineage since this pool was seeded". A pool with no record
    /// to answer for it is unvalidatable and must not be adopted.
    fn record_epoch(&self, lineage: &str) -> Option<Option<u64>> {
        let _ = lineage;
        None
    }
    /// Invariant F (task 15h): whether `volume`'s cycle loop terminated
    /// FENCED — its stage epoch superseded, every further transaction
    /// refused. Returns the stage's own epoch (for quarantine naming; 0
    /// = epoch unknown); `None` = not fenced. A fenced volume's unstage
    /// SKIPS the final publish cycle and quarantines local state
    /// instead of refusing teardown. Implementations must answer from
    /// DURABLE state as well as the running map: a retry after
    /// `stop()` + a failed quarantine rename must still see the fence.
    fn fenced(&self, volume: &str) -> Option<u64> {
        let _ = volume;
        None
    }
    /// Drop the durable fenced marker for `volume`. Called by the
    /// unstage path ONLY after the quarantine rename succeeded — the
    /// marker must outlive every failure mode before that point.
    fn clear_fenced(&self, volume: &str) {
        let _ = volume;
    }
    /// D-M13 final cycle: drive the volume's unpublished delta to zero
    /// before teardown — file a snapshot request and wait (bounded) for
    /// the running cycle to publish it. MUST run before [`Self::stop`]
    /// and before the umount: the origin has to stay mounted for the
    /// freeze. No running cycle (raw/block/RO volumes) is an immediate
    /// `Ok`. An error means the delta is NOT known-published — the
    /// caller must fail the unstage so kubelet's retry *resumes* the
    /// upload (the cycle is resumable) instead of stranding it.
    async fn finish(&self, volume: &str) -> Result<(), String>;
    /// Stop the cycle and deactivate the agent's thin device *nodes*
    /// (their mappings stay in the pool metadata, which lives in the
    /// surviving cache file). MUST complete before the thin stack's
    /// flush/teardown — the pool cannot be removed under live thin
    /// devices.
    async fn stop(&self, volume: &str);
}

/// Kernel + control-plane implementation.
#[cfg(feature = "dm")]
pub struct DmCycle {
    api_url: String,
    token_file: Option<PathBuf>,
    /// Agent state (cycle state records + dev-id sidecars). Must not be
    /// on a freezable filesystem (agent design §6.3) — the node uses
    /// its host-path state dir.
    state_dir: PathBuf,
    chunk_bytes: u64,
    /// Cadence for unprompted cycles (continuous publication).
    pub interval: Duration,
    /// Snapshot-request poll (also the stop-signal latency bound).
    pub poll: Duration,
    /// How long [`CycleRunner::finish`] waits for the final publish
    /// before failing the unstage (D-M13). Generous on purpose: kubelet
    /// retries an errored unstage and the resumable cycle picks up
    /// where it left, so timing out early costs a retry, not data.
    pub finish_budget: Duration,
    pub freeze_budget_ms: u64,
    /// `advancePoolWatermark` (D-M15 §4): pool-occupancy fraction above
    /// which a publish cycle is promoted to an advance cycle. `None`
    /// disables the backstop.
    pub advance_watermark: Option<f64>,
    /// §6.2 `publishRetentionWindow` (task 15f): arm the post-advance
    /// retention pass with this window. `None` = no thinning from this
    /// node — same off-by-default as the daemon's absent `[retention]`.
    pub retention_window_s: Option<u64>,
    /// Backfill upload batch depth (issue #97). `None` = the engine's
    /// default of 1, one op at a time.
    pub publish_pipeline_depth: Option<usize>,
    /// Whether mixed attach/clear batches use the server's `apply` call
    /// ([`crate::MixedApply`]). `None` = `Auto`, the features header's say.
    pub mixed_apply: Option<crate::MixedApply>,
    /// Overlap consecutive backfill batches above depth 1. `None` = the
    /// engine's default (on).
    pub publish_overlap: Option<bool>,
    /// §8.1 rule 1 (task 15j): the span of the rescue advance the
    /// watermark keeps write-rate headroom for. `None` = the engine's
    /// default (30 s).
    pub advance_window: Option<Duration>,
    /// §8.1 rule 1: the flat reserve for the dirty data fsfreeze
    /// flushes into the pool. `None` = the engine's default (64 MiB).
    pub dirty_flush_reserve: Option<u64>,
    /// §8.1 rule 3 (`publishFailingReadOnlyWatermark`, task 15j part B):
    /// `(occupancy, duration)` — once publishes have been failing for
    /// the duration AND the pool is at or above the occupancy, the agent
    /// takes the filesystem read-only rather than let it exhaust into a
    /// dirty shutdown. `None` = off, and off is the default: a
    /// deliberate `EROFS` stops the workload, so it stays opt-in.
    pub ro_watermark: Option<(f64, Duration)>,
    /// Node-level §7 compaction-policy defaults (task 15i, the
    /// SKYNBD_RECLAIM_* env); per-volume dials override field-wise.
    pub policy: crate::ReclaimPolicy,
    /// std Mutex: taken only for map insert/remove (no awaits held),
    /// and `start` runs on the async gRPC path where a tokio
    /// blocking_lock would panic.
    running: std::sync::Mutex<HashMap<String, RunningCycle>>,
}

#[cfg(feature = "dm")]
struct RunningCycle {
    stop: tokio::sync::watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
    lineage: String,
    /// The stage's lease epoch (task 15h), for quarantine naming.
    epoch: Option<u64>,
    /// Set by the cycle loop when it terminates on the Invariant F
    /// fence: the volume's unstage must abandon, not retry.
    fenced: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(feature = "dm")]
impl DmCycle {
    pub fn new(
        api_url: String,
        token_file: Option<PathBuf>,
        state_dir: PathBuf,
        chunk_bytes: u64,
        interval: Duration,
        freeze_budget_ms: u64,
    ) -> Self {
        Self {
            api_url,
            token_file,
            state_dir,
            chunk_bytes,
            interval,
            poll: Duration::from_secs(5),
            finish_budget: Duration::from_secs(120),
            freeze_budget_ms,
            advance_watermark: None,
            retention_window_s: None,
            publish_pipeline_depth: None,
            mixed_apply: None,
            publish_overlap: None,
            advance_window: None,
            dirty_flush_reserve: None,
            ro_watermark: None,
            policy: crate::ReclaimPolicy::default(),
            running: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn dm_config(&self, spec: &CycleSpec) -> crate::DmConfig {
        let names = crate::provision::StackNames::for_volume(&spec.volume);
        // §7 slot config: the §5 local pool file IS the live slot, so a
        // provisioned stack that knows its carve supports the two-pool
        // swing. The carve names are provision's own — the swing's
        // teardown removes and its canonicalize re-creates exactly the
        // devices `provision::build`'s idempotent resume, `observe` and
        // teardown already own, so a post-swing stack is
        // indistinguishable from a freshly provisioned one (§7: "which
        // executor ran is not observable"). `pool_layout` absent means
        // the stage could not answer for the carve — the volume then
        // stays on the port, the pre-15e posture.
        let slot = match (&spec.pool_file, &spec.pool_layout) {
            (Some(f), Some(l)) => Some(crate::SlotConfig {
                live_file: f.clone(),
                data_bytes: l.data_bytes,
                meta_bytes: l.tmeta_bytes,
                shim_name: names.fs.clone(),
                external_origin: spec.origin_dev.clone(),
                tmeta_name: Some(names.tmeta.clone()),
                data_name: Some(names.data.clone()),
            }),
            _ => None,
        };
        crate::DmConfig {
            pool_name: names.pool,
            origin_name: names.origin,
            // provision::build always creates the origin as thin dev 0.
            origin_dev_id: 0,
            metadata_dev: format!("/dev/mapper/{}", names.tmeta).into(),
            mountpoint: spec.staging_path.clone().into(),
            volume: spec.lineage.clone(),
            size_bytes: spec.size_bytes,
            chunk_bytes: self.chunk_bytes,
            ids_path: self.state_dir.join(format!("{}.devids", spec.lineage)),
            freeze_budget_ms: self.freeze_budget_ms,
            // The discard port (D-M15 Invariant B): built into every
            // CSI thin stack by provision::build, so a mounted volume's
            // reclaim never targets a claimed node — the fail-closed
            // rule is structural here, not a config check. (Config-file
            // rigs may still opt out with `None`; they run unmounted.)
            discard_name: Some(names.rec),
            // §8.1 rule 2 (task 15j): with the §5 local pool the
            // second exhaustion surface is real — the pool's data
            // region lives in the local backing file, so the
            // filesystem that file sits on can starve a pool write.
            // The file is fallocate-preallocated, which covers its own
            // extents, but rule 2 watches the filesystem's free bytes
            // as the early-warning margin the design asks for.
            backing_file: spec.pool_file.clone(),
            // Invariant B (task 15j): the linear the guest filesystem
            // is mounted on, so the engine can re-derive the
            // guest-discard mask from the kernel at the head of every
            // cycle. CSI stacks run `slot: None`, so without this the
            // per-cycle re-assert would be dark on every CSI volume —
            // the deployments where a thawed, mounted, unmasked shim is
            // exactly the live hazard.
            shim_name: Some(names.fs),
            // §7.1: on a cached stack the guest mounts the rcache, so
            // the per-cycle re-assert must land there as well — the
            // shim's mask does not re-export through dm-cache.
            mount_name: spec.read_cache.then_some(names.rcache),
            slot,
        }
    }

    fn control_plane(&self, lineage: &str) -> crate::HttpControlPlane {
        match &self.token_file {
            Some(p) => crate::HttpControlPlane::with_token_file(
                self.api_url.clone(),
                lineage.to_string(),
                p.clone(),
            ),
            None => crate::HttpControlPlane::new(self.api_url.clone(), lineage.to_string(), None),
        }
        .with_mixed_apply(self.mixed_apply.unwrap_or_default())
    }

    /// Deactivate every dm device node the agent stack created for this
    /// lineage volume (`sky-<lineage>-{b|c|u}-…`). Device-node removal
    /// only: the thin mappings live in pool metadata (the cache file)
    /// and the dev-id sidecar survives, so a later restage can resume
    /// from the baseline (or the planner re-bootstraps).
    fn deactivate_agent_devices(lineage: &str) {
        let ops = match crate::LinuxDmOps::new() {
            Ok(o) => o,
            Err(e) => {
                tracing::warn!(lineage, error = %e, "dm unavailable; agent devices left active");
                return;
            }
        };
        let prefix = format!("sky-{lineage}-");
        match crate::dm::DmOps::list_devices(&ops) {
            Ok(devs) => {
                for d in devs {
                    let Some(rest) = d.name.strip_prefix(&prefix) else {
                        continue;
                    };
                    if matches!(rest.get(..2), Some("b-" | "c-" | "u-")) {
                        if let Err(e) = crate::dm::DmOps::remove_device(&ops, &d.name) {
                            tracing::warn!(device = %d.name, error = %e,
                                "failed to deactivate agent device");
                        }
                    }
                }
            }
            Err(e) => tracing::warn!(lineage, error = %e, "cannot list dm devices"),
        }
    }
}

/// The durable Invariant F marker (task 15h): `<lineage>.fenced` in the
/// DmCycle state dir, written the moment fencing is detected and again
/// (belt-and-braces) before the running-map entry is dropped, so
/// `fenced()` answers correctly across a driver restart or an unstage
/// retry after `stop()`. Content is JSON — the CSI volume the lineage
/// belongs to (the key `fenced()` is asked by) and the stage epoch
/// (`null` = unknown).
#[derive(serde::Serialize, serde::Deserialize)]
struct FencedMarker {
    volume: String,
    epoch: Option<u64>,
}

fn fenced_marker_path(state_dir: &Path, lineage: &str) -> PathBuf {
    state_dir.join(format!("{lineage}.fenced"))
}

/// Best-effort durable write of the marker (fence detection must never
/// fail on IO — the in-memory flag still covers the common path).
fn write_fenced_marker(state_dir: &Path, lineage: &str, volume: &str, epoch: Option<u64>) {
    let marker = FencedMarker {
        volume: volume.to_string(),
        epoch,
    };
    let write = std::fs::create_dir_all(state_dir).and_then(|()| {
        std::fs::write(
            fenced_marker_path(state_dir, lineage),
            serde_json::to_vec(&marker).expect("marker serializes"),
        )
    });
    if let Err(e) = write {
        tracing::warn!(lineage, volume, error = %e, "fenced marker write failed");
    }
}

/// The recorded fence for `volume`, if any marker in `state_dir` names
/// it. Returns the quarantine epoch (0 = unknown).
fn load_fenced_marker(state_dir: &Path, volume: &str) -> Option<u64> {
    for entry in std::fs::read_dir(state_dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("fenced") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(m) = serde_json::from_slice::<FencedMarker>(&bytes) {
            if m.volume == volume {
                return Some(m.epoch.unwrap_or(0));
            }
        }
    }
    None
}

fn clear_fenced_marker(state_dir: &Path, volume: &str) {
    let Ok(entries) = std::fs::read_dir(state_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("fenced") {
            continue;
        }
        let named = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice::<FencedMarker>(&b).ok())
            .is_some_and(|m| m.volume == volume);
        if named {
            if let Err(e) = std::fs::remove_file(&path) {
                tracing::warn!(volume, path = %path.display(), error = %e,
                    "fenced marker removal failed");
            }
        }
    }
}

/// Reserved alias name the final cycle publishes under. Transient: it
/// is deleted (best-effort) before filing — so the alias lands on the
/// fresh generation, not a Conflict against a previous unstage's — and
/// again after the wait, so it never pins an old commit against GC.
const FINAL_SNAP: &str = "skynbd-unstage";

/// The D-M13 wait: file a snapshot request under `name` and poll its
/// D-M12 status until the request completes (the cycle loop completes
/// requests only after a successful publish). Split from [`DmCycle`] so
/// the protocol is testable against a stub server.
pub(crate) async fn await_final_publish(
    client: &crate::cp::Client,
    lineage: &str,
    name: &str,
    budget: Duration,
    poll: Duration,
) -> Result<(), String> {
    // The budget bounds the whole function, not just the poll loop.
    // Two things used to escape it: the two calls below ran BEFORE the
    // clock started, and each request could take a full client read
    // timeout of its own — so a control plane that accepts and never
    // answers turned a 120 s budget into roughly 300 s of wall clock
    // (file, request, and two polls, each paying the timeout in full).
    // Finite, but the number read like a bound and was not one.
    //
    // `timeout_at` against a single deadline fixes both: every call is
    // clamped to whatever is left, and a call that outruns it is
    // indistinguishable from a call that failed — the deadline check at
    // the bottom of the loop decides either way.
    let deadline = std::time::Instant::now() + budget;
    let until = tokio::time::Instant::from_std(deadline);

    // Clear any stale alias from a previous unstage so this cycle's
    // alias attaches to the new generation. Best-effort: an alias that
    // survives only costs an extra pinned commit until the next pass.
    //
    // Best-effort about its RESULT, though — it can still spend the
    // whole budget, and a bare `let _ =` makes that invisible. Remember
    // whether it did, or the timeout below reports a failure to file the
    // request when filing never got a chance to start, and points an
    // operator at the wrong endpoint.
    let cleared_alias_in_time =
        tokio::time::timeout_at(until, client.delete_snapshot(lineage, name))
            .await
            .is_ok();
    match tokio::time::timeout_at(until, client.request_snapshot(lineage, name)).await {
        Ok(r) => r.map_err(|e| format!("filing final snapshot request: {e}"))?,
        Err(_) => {
            let why = if cleared_alias_in_time {
                "filing the request did not complete"
            } else {
                "the budget was spent clearing the stale alias, \
                 before the request could be filed"
            };
            return Err(format!(
                "final cycle not published within {budget:?} ({why})"
            ));
        }
    }
    let mut last: Option<crate::cp::RequestStatus> = None;
    loop {
        match tokio::time::timeout_at(until, client.snapshot_request_status(lineage, name)).await {
            // Request gone: an older server removes it only after the
            // cycle published (alias Conflict included — that too is
            // only reached post-publish). But requests are in-memory,
            // so a server restart mid-wait ALSO answers 404 — verify
            // the published alias exists before trusting it, and
            // re-file when it doesn't.
            Ok(Ok(None)) => {
                let published = tokio::time::timeout_at(until, client.list_snapshots(lineage))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .map(|s| s.iter().any(|s| s.name == name))
                    .unwrap_or(false);
                if published {
                    let _ =
                        tokio::time::timeout_at(until, client.delete_snapshot(lineage, name)).await;
                    return Ok(());
                }
                let _ =
                    tokio::time::timeout_at(until, client.request_snapshot(lineage, name)).await;
            }
            Ok(Ok(Some(st))) if st.completed => {
                let _ = tokio::time::timeout_at(until, client.delete_snapshot(lineage, name)).await;
                return Ok(());
            }
            // The agent discarded the cut this request was bound to: it
            // will never be served under the old request. Clearing the
            // name drops the request server-side; re-filing asks for a
            // fresh cut (no caller holds the lost one's time here).
            Ok(Ok(Some(st))) if st.capture_lost => {
                let _ = tokio::time::timeout_at(until, client.delete_snapshot(lineage, name)).await;
                let _ =
                    tokio::time::timeout_at(until, client.request_snapshot(lineage, name)).await;
            }
            Ok(Ok(Some(st))) => last = Some(st),
            // Transient fetch trouble: the request itself is durable
            // enough (the server re-answers), keep waiting. A call that
            // ran out the budget (the outer `Err`) lands here too — the
            // deadline check below is what ends it, and it has just
            // become true.
            Ok(Err(_)) | Err(_) => {}
        }
        if std::time::Instant::now() >= deadline {
            let why = match &last {
                Some(st) => format!(
                    "phase={} ops {}/{} attempts={} last_error={}",
                    st.phase,
                    st.ops_done,
                    st.ops_total,
                    st.attempts,
                    st.last_error.as_deref().unwrap_or("none"),
                ),
                None => "no status observed".to_string(),
            };
            return Err(format!(
                "final cycle not published within {budget:?} ({why})"
            ));
        }
        tokio::time::sleep(poll).await;
    }
}

/// §7.1: lazy timing runs the advance's §3 head and skips the reclaim,
/// leaving the published set pooled as read cache — evicted only under
/// `advancePoolWatermark` pressure. With no watermark configured there
/// is no pressure signal, so nothing is ever evicted and the pool grows
/// until it is full: the combination is a silent capacity leak, not a
/// policy. Fail the stage naming both knobs — they live on opposite
/// sides (StorageClass parameter vs node env), which is exactly why the
/// mismatch is easy to configure.
pub fn check_lazy_needs_watermark(
    policy: &crate::ReclaimPolicy,
    advance_watermark: Option<f64>,
) -> Result<(), String> {
    if policy.timing == crate::ReclaimTiming::Lazy && advance_watermark.is_none() {
        return Err(
            "reclaimTiming: lazy evicts only under pool-watermark pressure, \
                    and this node has no SKYNBD_ADVANCE_POOL_WATERMARK set — the pool \
                    would grow without bound. Set the watermark on the node DaemonSet \
                    or use reclaimTiming: eager"
                .to_string(),
        );
    }
    Ok(())
}

#[cfg(feature = "dm")]
#[async_trait::async_trait]
impl CycleRunner for DmCycle {
    fn start(&self, spec: &CycleSpec) -> Result<(), String> {
        let mut running = self.running.lock().unwrap();
        if running.contains_key(&spec.volume) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.state_dir).map_err(|e| e.to_string())?;
        // Construct the stack now so misconfiguration fails the stage
        // loudly instead of a background task silently never publishing.
        let stack =
            crate::LinuxDmStack::new_linux(self.dm_config(spec)).map_err(|e| e.to_string())?;
        // The stage's fence token (Invariant F): the spec's epoch, else
        // whatever the durable state record remembers — a driver
        // restart must not silently drop the fence.
        let store = crate::StateStore::new(&self.state_dir);
        let state_epoch = match store.load(&spec.lineage) {
            Ok(crate::LoadedState::Present(s)) => s.epoch,
            _ => None,
        };
        let epoch = spec.epoch.or(state_epoch);
        // §7 dials (task 15i): node defaults, overridden per-volume from
        // the StorageClass via the volume context. This is the first
        // point where the resolved timing and the node's watermark are
        // both in hand — CreateVolume sees only the StorageClass half.
        let policy = spec.reclaim.apply_to(self.policy.clone());
        check_lazy_needs_watermark(&policy, self.advance_watermark)?;
        let cp = self.control_plane(&spec.lineage).with_epoch(epoch);
        let mut engine = crate::CycleEngine::open(stack, cp.clone(), store, &spec.lineage)
            .map_err(|e| e.to_string())?
            .with_epoch(epoch)
            .with_policy(policy);
        if let Some(w) = self.advance_watermark {
            engine = engine.with_advance_watermark(w);
        }
        if let Some(w) = self.retention_window_s {
            engine = engine.with_retention_window(w);
        }
        if let Some(d) = self.publish_pipeline_depth {
            engine = engine.with_publish_pipeline_depth(d);
        }
        if self.mixed_apply == Some(crate::MixedApply::Never) {
            engine = engine.with_mixed_apply(false);
        }
        if let Some(o) = self.publish_overlap {
            engine = engine.with_publish_overlap(o);
        }
        // §8.1 rule 1's two dials. Both only ever matter under a
        // configured watermark — the margin term is gated on it, so a
        // node that set neither behaves exactly as it did before 15j.
        if let Some(w) = self.advance_window {
            engine = engine.with_advance_window(w);
        }
        if let Some(r) = self.dirty_flush_reserve {
            engine = engine.with_dirty_flush_reserve(r);
        }
        // §8.1 rule 3. Independent of the watermark above: rule 3's own
        // occupancy term is its gate, and a node can want the clean
        // degradation without wanting watermark-promoted advances.
        if let Some((f, after)) = self.ro_watermark.filter(|_| !spec.readonly) {
            engine = engine.with_ro_watermark(f, after);
        }

        let (stop, stop_rx) = tokio::sync::watch::channel(false);
        let (interval, poll) = (self.interval, self.poll);
        let volume = spec.volume.clone();
        let lineage = spec.lineage.clone();
        let fenced = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let task = tokio::spawn(cycle_loop(
            engine,
            cp,
            volume,
            lineage.clone(),
            interval,
            poll,
            spec.origin_dev.is_some(),
            stop_rx,
            std::sync::Arc::clone(&fenced),
            // Fence persistence (task 15h): the loop writes the durable
            // marker the moment the fence is detected, so an unstage
            // retry (or driver restart) still quarantines.
            Some((self.state_dir.clone(), epoch)),
        ));
        running.insert(
            spec.volume.clone(),
            RunningCycle {
                stop,
                task,
                lineage,
                epoch,
                fenced,
            },
        );
        Ok(())
    }

    fn resolve_slot(&self, spec: &CycleSpec) -> Result<(), String> {
        // Only a decodable record can name an in-flight swing. Absent
        // and Corrupt both resolve to "nothing to do" — same trust rule
        // as the engine's own recovery, which cannot discriminate the
        // §7 crash windows without the flag either.
        let flipped = match crate::StateStore::new(&self.state_dir).load(&spec.lineage) {
            Ok(crate::LoadedState::Present(s)) => s.swing_flipped,
            _ => None,
        };
        let Some(flipped) = flipped else {
            return Ok(());
        };
        // The stack construction needs the mountpoint to exist (the
        // freeze watchdog pre-opens it); on the reboot path the stage
        // has not created the staging dir yet.
        std::fs::create_dir_all(&spec.staging_path).map_err(|e| e.to_string())?;
        let stack =
            crate::LinuxDmStack::new_linux(self.dm_config(spec)).map_err(|e| e.to_string())?;
        tracing::warn!(
            volume = %spec.volume,
            lineage = %spec.lineage,
            flipped,
            "stage found an in-flight swing recorded for this volume — \
             resolving the slot before touching the pool file (§7 crash windows)"
        );
        stack
            .sweep_slot(flipped)
            .map(|_| ())
            .map_err(|e| format!("slot sweep (flipped={flipped}): {e}"))
    }

    fn seed_baseline(&self, lineage: &str, base: &str, epoch: Option<u64>) -> Result<(), String> {
        std::fs::create_dir_all(&self.state_dir).map_err(|e| e.to_string())?;
        let store = crate::StateStore::new(&self.state_dir);
        store
            .save(&crate::AgentVolumeState {
                volume: lineage.to_string(),
                phase: crate::Phase::Idle,
                baseline: Some(crate::BaselinePair {
                    // Empty local member: the rebaselined-pair shape —
                    // the next cycle's delta is full_mapped(candidate)
                    // (= locally provisioned blocks) prepared on `base`.
                    local: crate::LocalSnap {
                        lv_name: String::new(),
                        thin_dev_id: 0,
                        pool_tx_id: 0,
                        created_at_ms: 0,
                    },
                    remote: base.to_string(),
                    pin_holder: format!("agent-baseline:{lineage}"),
                }),
                candidate: None,
                prepared: None,
                cursor: None,
                advancing: false,
                epoch,
                advances_since_swing: 0,
                swing_flipped: None,
                candidate_requests: Vec::new(),
                served_requests: Vec::new(),
                lost_requests: Vec::new(),
                updated_at_ms: 0,
            })
            .map_err(|e| e.to_string())?;
        tracing::info!(
            lineage,
            base,
            ?epoch,
            "seeded rebaselined agent state (D-M11)"
        );
        Ok(())
    }

    fn record_epoch(&self, lineage: &str) -> Option<Option<u64>> {
        match crate::StateStore::new(&self.state_dir).load(lineage) {
            Ok(crate::LoadedState::Present(state)) => Some(state.epoch),
            // Absent, corrupt, or unreadable: no trustworthy record —
            // the caller must treat the pool as unvalidatable.
            _ => None,
        }
    }

    fn fenced(&self, volume: &str) -> Option<u64> {
        // The running map answers for a live loop…
        {
            let running = self.running.lock().unwrap();
            if let Some(rc) = running.get(volume) {
                if rc.fenced.load(std::sync::atomic::Ordering::SeqCst) {
                    return Some(rc.epoch.unwrap_or(0));
                }
            }
        }
        // …and the durable marker for everything else: an unstage retry
        // after stop() (the map entry is gone), or a driver restart.
        load_fenced_marker(&self.state_dir, volume)
    }

    fn clear_fenced(&self, volume: &str) {
        clear_fenced_marker(&self.state_dir, volume);
    }

    async fn finish(&self, volume: &str) -> Result<(), String> {
        // No running cycle → nothing unpublished this runner owns
        // (raw/block/RO volumes; also a restarted plugin whose stage
        // wasn't replayed — the recovery planner covers that on the
        // next stage, same exposure as before D-M13).
        let Some(lineage) = self
            .running
            .lock()
            .unwrap()
            .get(volume)
            .map(|rc| rc.lineage.clone())
        else {
            return Ok(());
        };
        let client = crate::cp::Client::new(self.api_url.clone(), self.token_file.clone());
        tracing::info!(volume, lineage, "unstage: awaiting final publish cycle");
        await_final_publish(
            &client,
            &lineage,
            FINAL_SNAP,
            self.finish_budget,
            Duration::from_secs(1),
        )
        .await
    }

    async fn stop(&self, volume: &str) {
        let Some(rc) = self.running.lock().unwrap().remove(volume) else {
            return;
        };
        // The map entry dies here — if the loop was fenced, make sure
        // the durable marker exists BEFORE the only in-memory record is
        // gone (the loop already wrote it at detection; this is the
        // belt-and-braces on the unstage path).
        if rc.fenced.load(std::sync::atomic::Ordering::SeqCst) {
            write_fenced_marker(&self.state_dir, &rc.lineage, volume, rc.epoch);
        }
        let _ = rc.stop.send(true);
        if let Err(e) = rc.task.await {
            tracing::warn!(volume, error = %e, "cycle task join");
        }
        let lineage = rc.lineage;
        // Blocking dm ioctls off the async path.
        let _ = tokio::task::spawn_blocking(move || Self::deactivate_agent_devices(&lineage)).await;
    }
}

/// The per-volume loop: poll for snapshot requests; run a cycle when one
/// is pending or the cadence is due; alias + complete requests after a
/// successful publication. Errors log and retry — the server may be
/// briefly unreachable, and the external-snapshotter keeps re-filing
/// requests it still wants.
#[allow(clippy::too_many_arguments)]
#[cfg(feature = "dm")]
async fn cycle_loop(
    mut engine: crate::CycleEngine<crate::LinuxDmStack, crate::HttpControlPlane>,
    cp: crate::HttpControlPlane,
    volume: String,
    lineage: String,
    interval: Duration,
    poll: Duration,
    has_origin: bool,
    mut stop: tokio::sync::watch::Receiver<bool>,
    fenced_flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    // Where (and with which stage epoch) to persist the durable fenced
    // marker at detection time; `None` = in-memory flag only (tests).
    marker: Option<(PathBuf, Option<u64>)>,
) {
    // Invariant F: the terminal exit. A fenced stage must never retry —
    // mark the volume fenced (unstage reads this to take the
    // quarantine-then-teardown branch), persist the marker so the fence
    // survives stop()/restart, and END the loop.
    let fence_exit = |holder: &str, epoch: u64| {
        tracing::error!(
            volume,
            lineage,
            holder,
            epoch,
            "stage epoch superseded (Invariant F): abandoning the cycle loop — \
             no retry; unstage will quarantine local state"
        );
        fenced_flag.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some((state_dir, stage_epoch)) = &marker {
            write_fenced_marker(state_dir, &lineage, &volume, *stage_epoch);
        }
    };
    // Recovery first (V3.2 §15.2): reconcile persisted state with local
    // devices + remote refs before the first capture. Retried because a
    // node often stages before the control plane is reachable.
    let mut recovered = false;
    let mut last_cycle: Option<std::time::Instant> = None;
    // Consecutive failures since the last success (D-M12): reported with
    // each error so a pending snapshot request shows *why* it's stuck,
    // not just that it is. Best-effort — a report that can't reach the
    // server changes nothing (the poll loop is already retrying).
    let mut attempts: u32 = 0;
    loop {
        tokio::select! {
            _ = stop.changed() => return,
            _ = tokio::time::sleep(poll) => {}
        }
        if !recovered {
            match engine.recover().await {
                Ok(()) => recovered = true,
                Err(crate::AgentError::Fenced { holder, epoch }) => {
                    let _ = cp
                        .report_cycle_failure(attempts + 1, "fenced: stage epoch superseded")
                        .await;
                    fence_exit(&holder, epoch);
                    return;
                }
                Err(e) => {
                    tracing::warn!(volume, lineage, error = %e, "agent recovery; will retry");
                    attempts += 1;
                    let _ = cp.report_cycle_failure(attempts, &e.to_string()).await;
                    continue;
                }
            }
        }
        // D-M12: a published capture owes its requests an alias to exactly
        // the generation holding their cut. That binding is the agent's own
        // durable state — never re-derived from generation names, which
        // repeat after a discarded candidate — and it is settled before
        // anything can start a new capture.
        if let Err((holder, epoch)) = settle_served(&mut engine, &cp, &volume).await {
            let _ = cp
                .report_cycle_failure(attempts + 1, "fenced: stage epoch superseded")
                .await;
            fence_exit(&holder, epoch);
            return;
        }
        let requests = match cp.snapshot_requests().await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(volume, lineage, error = %e, "snapshot-request poll");
                Vec::new()
            }
        };
        // A request already tied to a cut (the candidate's, a published one
        // awaiting its alias, or a discarded one) never joins a new capture:
        // its caller may hold that cut's time. The rest are fresh, sampled
        // before this cycle starts — a request filed mid-cycle waits for a
        // cut of its own instead of inheriting one taken before it existed.
        let fresh: Vec<String> = requests
            .iter()
            .filter(|name| !engine.is_request_bound(name))
            .cloned()
            .collect();
        // Re-announce bindings while a bound request is pending: the API
        // keeps requests in memory, so a restart forgets the capture times
        // its callers were given — and the loss of a discarded cut, whose
        // tombstone here outlives the report for exactly that reason.
        if fresh.len() != requests.len() {
            match engine.report_captures(&requests).await {
                // D-M13 re-files its reserved request itself and holds no
                // capture time, so once its loss is on record a re-filed
                // final request may take a fresh cut.
                Ok(()) if requests.iter().any(|r| r == FINAL_SNAP) => {
                    if let Err(e) = engine.forget_lost(FINAL_SNAP) {
                        tracing::warn!(volume, lineage, error = %e, "final-request tombstone");
                    }
                }
                Ok(()) => {}
                Err(e) => {
                    tracing::warn!(volume, lineage, error = %e, "snapshot-request capture report")
                }
            }
        }
        let serving = !fresh.is_empty() || engine.has_candidate_requests();
        let due = last_cycle.is_none_or(|t| t.elapsed() >= interval);
        if !serving && !due {
            continue;
        }
        engine.set_capture_requests(fresh);
        // D-M15 §4 (15d): request-serving cycles — including D-M13's
        // final unstage request (FINAL_SNAP) — are always ADVANCE
        // cycles: the stage attached the repointable `<lineage>/origin`
        // export, so the §3 tail can repoint, fence, and reclaim behind
        // it. Cadence-driven cycles stay publishes, with `run_cycle`
        // still promoting them at the pool watermark. EXCEPT with no
        // origin attach (a brand-new volume staged before any
        // generation existed): then every cycle is forced to a plain
        // publish regardless of requests and watermark — the advance
        // tail would initialize/move the pointer and pass the served
        // fence server-side, but nothing is attached locally to
        // re-serve the reclaimed blocks, so the reclaim would unmap
        // blocks that then read back as zeros.
        let outcome = if !has_origin {
            engine.run_cycle_as(crate::CycleKind::Publish).await
        } else if serving {
            engine.run_cycle_as(crate::CycleKind::Advance).await
        } else {
            engine.run_cycle().await
        };
        match outcome {
            Ok(target) => {
                attempts = 0;
                last_cycle = Some(std::time::Instant::now());
                let generation = target.strip_prefix("snap/").unwrap_or(&target).to_string();
                tracing::info!(volume, lineage, generation, "cycle published");
                if let Err((holder, epoch)) = settle_served(&mut engine, &cp, &volume).await {
                    let _ = cp
                        .report_cycle_failure(attempts + 1, "fenced: stage epoch superseded")
                        .await;
                    fence_exit(&holder, epoch);
                    return;
                }
            }
            Err(crate::AgentError::Fenced { holder, epoch }) => {
                let _ = cp
                    .report_cycle_failure(attempts + 1, "fenced: stage epoch superseded")
                    .await;
                fence_exit(&holder, epoch);
                return;
            }
            Err(e) => {
                tracing::warn!(volume, lineage, error = %e, "cycle failed; will retry");
                attempts += 1;
                let _ = cp.report_cycle_failure(attempts, &e.to_string()).await;
                // A failed cycle can leave a candidate (or a half-run
                // promotion) behind; re-running run_cycle blind would
                // re-capture into a dm name collision (EBUSY). Recover
                // first so the planner resumes from what actually exists.
                recovered = false;
            }
        }
    }
}

/// Alias + complete every request whose candidate has been published
/// (D-M12), each to the generation its own cut became. A request leaves
/// the agent's durable binding only once both steps succeeded; transient
/// failures stay for the next poll. `Err` = the alias was fenced
/// (Invariant F), which is terminal for the caller.
#[cfg(feature = "dm")]
async fn settle_served<L: crate::LocalStack, C: crate::ControlPlane>(
    engine: &mut crate::CycleEngine<L, C>,
    cp: &crate::HttpControlPlane,
    volume: &str,
) -> Result<(), (String, u64)> {
    for served in engine.served_requests() {
        let name = served.name.as_str();
        let generation = served
            .target
            .strip_prefix("snap/")
            .unwrap_or(&served.target);
        // Idempotent alias; Conflict means the name already exists — an
        // earlier attempt aliased it before its completion failed.
        match cp.alias_snapshot(generation, name).await {
            Ok(()) | Err(crate::CpError::Conflict(_)) => {}
            Err(crate::CpError::Fenced { holder, epoch }) => return Err((holder, epoch)),
            Err(e) => {
                tracing::warn!(volume, name, generation, error = %e, "alias failed; will retry");
                continue;
            }
        }
        if let Err(e) = cp.complete_snapshot_request(name).await {
            tracing::warn!(volume, name, error = %e, "request completion; will retry");
            continue;
        }
        if let Err(e) = engine.finish_served(name) {
            tracing::warn!(volume, name, error = %e, "served-request bookkeeping; will retry");
        }
    }
    Ok(())
}

#[cfg(all(test, feature = "dm"))]
mod slot_tests {
    use super::*;

    fn runner(dir: &std::path::Path) -> DmCycle {
        DmCycle::new(
            "http://127.0.0.1:1".into(),
            None,
            dir.to_path_buf(),
            64 << 10,
            Duration::from_secs(900),
            5_000,
        )
    }

    fn spec() -> CycleSpec {
        CycleSpec {
            volume: "pvc-1".into(),
            lineage: "lin-1".into(),
            staging_path: "/stage/pvc-1".into(),
            size_bytes: 1 << 30,
            origin_dev: Some("/dev/nbd7".into()),
            epoch: Some(3),
            reclaim: ReclaimParams::default(),
            pool_file: Some("/cache/pvc-1.cache".into()),
            pool_layout: crate::provision::fast_layout(1 << 30, 256 << 20, 64 << 10).ok(),
            read_cache: false,
            readonly: false,
        }
    }

    /// Task 15e: a provisioned stack that knows its pool file AND its
    /// carve is slot-managed — with provision's own carve names, the
    /// shared origin attach, and the exact layout the build computed —
    /// so `decide()` stops port-pinning it.
    #[test]
    fn dm_config_wires_the_slot_from_pool_file_and_layout() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = runner(dir.path()).dm_config(&spec());
        let slot = cfg.slot.expect("pool_file + pool_layout = a slot");
        assert_eq!(
            slot.live_file,
            std::path::PathBuf::from("/cache/pvc-1.cache")
        );
        let l = crate::provision::fast_layout(1 << 30, 256 << 20, 64 << 10).unwrap();
        assert_eq!(
            (slot.meta_bytes, slot.data_bytes),
            (l.tmeta_bytes, l.data_bytes)
        );
        assert_eq!(slot.shim_name, "sky-pvc-1-fs");
        assert_eq!(slot.external_origin.as_deref(), Some("/dev/nbd7"));
        // Provision's carve names, so the swing tears down and renames
        // exactly the devices provision::build owns.
        assert_eq!(slot.tmeta_name.as_deref(), Some("sky-pvc-1-tmeta"));
        assert_eq!(slot.data_name.as_deref(), Some("sky-pvc-1-data"));
    }

    /// No carve answer = no slot: the volume stays port-pinned rather
    /// than swinging against a guessed layout (a wrong carve reads pool
    /// metadata as garbage on the next resume).
    #[test]
    fn dm_config_stays_slotless_without_a_layout() {
        let dir = tempfile::tempdir().unwrap();
        let r = runner(dir.path());
        let mut s = spec();
        s.pool_layout = None;
        assert!(r.dm_config(&s).slot.is_none());
        // And without a pool file there is nothing to slot at all.
        let mut s = spec();
        s.pool_file = None;
        assert!(r.dm_config(&s).slot.is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};
    use std::sync::Arc;

    struct Stub {
        /// Pending status responses left before the request "completes"
        /// (GET starts returning 404). Negative = pending forever.
        pending_left: AtomicI32,
        status_body: serde_json::Value,
        deletes: AtomicU32,
        requests: AtomicU32,
        /// The published alias becomes visible in the snapshot listing
        /// only once at least this many requests were filed — 1 models
        /// a normal completion, 2 models a server that restarted after
        /// the first filing (request lost, nothing published).
        alias_when_requests: u32,
    }

    async fn spawn_stub(
        pending_left: i32,
        status_body: serde_json::Value,
        alias_when_requests: u32,
    ) -> (String, Arc<Stub>) {
        use axum::extract::Path;
        use axum::http::StatusCode;
        use axum::routing::{delete, get, post};
        let stub = Arc::new(Stub {
            pending_left: AtomicI32::new(pending_left),
            status_body,
            deletes: AtomicU32::new(0),
            requests: AtomicU32::new(0),
            alias_when_requests,
        });
        let (s1, s2, s3, s4) = (stub.clone(), stub.clone(), stub.clone(), stub.clone());
        let app = axum::Router::new()
            .route(
                "/v1/volumes/{vol}/snapshots/{name}",
                delete(move |Path((_, _)): Path<(String, String)>| {
                    let s = s1.clone();
                    async move {
                        s.deletes.fetch_add(1, Ordering::SeqCst);
                        StatusCode::NO_CONTENT
                    }
                }),
            )
            .route(
                "/v1/volumes/{vol}/snapshot-requests",
                post(
                    move |Path(_): Path<String>, _b: axum::Json<serde_json::Value>| {
                        let s = s2.clone();
                        async move {
                            s.requests.fetch_add(1, Ordering::SeqCst);
                            StatusCode::ACCEPTED
                        }
                    },
                ),
            )
            .route(
                "/v1/volumes/{vol}/snapshot-requests/{name}",
                get(move |Path((_, _)): Path<(String, String)>| {
                    let s = s3.clone();
                    async move {
                        let left = s.pending_left.load(Ordering::SeqCst);
                        if left == 0 {
                            return (StatusCode::NOT_FOUND, axum::Json(serde_json::json!({})));
                        }
                        if left > 0 {
                            s.pending_left.fetch_sub(1, Ordering::SeqCst);
                        }
                        (StatusCode::OK, axum::Json(s.status_body.clone()))
                    }
                }),
            )
            .route(
                "/v1/volumes/{vol}/snapshots",
                get(move |Path(_): Path<String>| {
                    let s = s4.clone();
                    async move {
                        let published = s.pending_left.load(Ordering::SeqCst) == 0
                            && s.requests.load(Ordering::SeqCst) >= s.alias_when_requests;
                        let list = if published {
                            serde_json::json!([{"name": FINAL_SNAP, "commit": "aa"}])
                        } else {
                            serde_json::json!([])
                        };
                        axum::Json(list)
                    }
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (base, stub)
    }

    /// B3 (task 15h): the durable fenced marker. `fenced()` must answer
    /// from the marker when the running map has no entry — the unstage
    /// retry after `stop()`, or a driver restart — and `clear_fenced`
    /// removes exactly the named volume's marker.
    #[cfg(feature = "dm")]
    #[test]
    fn fenced_marker_survives_the_running_map() {
        let dir = tempfile::tempdir().unwrap();
        let cycle = DmCycle::new(
            "http://127.0.0.1:1".into(),
            None,
            dir.path().to_path_buf(),
            64 << 10,
            Duration::from_secs(60),
            5_000,
        );
        assert_eq!(cycle.fenced("tv"), None);

        // The moment-of-detection write (what cycle_loop's fence_exit
        // and stop() both perform).
        write_fenced_marker(dir.path(), "tv-lin", "tv", Some(7));
        assert_eq!(
            cycle.fenced("tv"),
            Some(7),
            "the marker answers with no running-map entry"
        );
        assert!(dir.path().join("tv-lin.fenced").exists());

        // Unknown-epoch stage: quarantine epoch 0 ("eunknown" naming).
        write_fenced_marker(dir.path(), "tv2-lin", "tv2", None);
        assert_eq!(cycle.fenced("tv2"), Some(0));

        cycle.clear_fenced("tv");
        assert_eq!(cycle.fenced("tv"), None, "cleared after the quarantine");
        assert!(!dir.path().join("tv-lin.fenced").exists());
        assert_eq!(
            cycle.fenced("tv2"),
            Some(0),
            "other volumes' markers are untouched"
        );
    }

    /// Task 15i: the dial parse is strict (bad values name themselves),
    /// and the per-volume overrides layer field-wise over node defaults
    /// — the exact policy `start` hands to `CycleEngine::with_policy`.
    #[test]
    fn reclaim_params_parse_and_apply_over_node_defaults() {
        use crate::{ReclaimPolicy, ReclaimStrategy, ReclaimTiming};

        let empty = ReclaimParams::from_map(&HashMap::new()).unwrap();
        assert!(empty.is_empty());
        // No dials: the node default passes through untouched.
        assert_eq!(
            empty.apply_to(ReclaimPolicy::default()),
            ReclaimPolicy::default()
        );

        let m: HashMap<String, String> = [
            ("reclaimStrategy", "swing"),
            ("reclaimTiming", "lazy"),
            ("reclaimRangeThreshold", "512"),
            ("swingEveryNAdvances", "4"),
        ]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let full = ReclaimParams::from_map(&m).unwrap();
        assert_eq!(
            full.apply_to(ReclaimPolicy::default()),
            ReclaimPolicy {
                strategy: ReclaimStrategy::Swing,
                timing: ReclaimTiming::Lazy,
                range_threshold: 512,
                swing_every_n: Some(4),
            }
        );

        // Partial dials override only their own field.
        let node_default = ReclaimPolicy {
            strategy: ReclaimStrategy::Port,
            timing: ReclaimTiming::Lazy,
            range_threshold: 100,
            swing_every_n: Some(9),
        };
        let partial = ReclaimParams {
            strategy: Some(ReclaimStrategy::Auto),
            ..Default::default()
        };
        assert_eq!(
            partial.apply_to(node_default.clone()),
            ReclaimPolicy {
                strategy: ReclaimStrategy::Auto,
                ..node_default
            }
        );

        // Strict parse: the offending value is named.
        for (key, bad) in [
            ("reclaimStrategy", "Swing"),
            ("reclaimTiming", "never"),
            ("reclaimRangeThreshold", "-3"),
            ("swingEveryNAdvances", "many"),
        ] {
            let m = HashMap::from([(key.to_string(), bad.to_string())]);
            let err = ReclaimParams::from_map(&m).unwrap_err();
            assert!(err.contains(bad), "{key}: {err}");
        }

        // Aging is opt-in: 0 would mean "every advance is aged", which
        // is `reclaimStrategy: swing` under a name one keystroke from
        // the unset spelling that means "never".
        let m = HashMap::from([("swingEveryNAdvances".to_string(), "0".to_string())]);
        let err = ReclaimParams::from_map(&m).unwrap_err();
        assert!(err.contains("reclaimStrategy: swing"), "{err}");

        // Both surfaces resolve the dial through the same helper, so a
        // value the StorageClass refuses cannot slip in as a node-level
        // env default (`SKYNBD_RECLAIM_SWING_EVERY_N=0` reading as
        // "swing every advance" while the StorageClass spelling of the
        // same intent is rejected).
        assert_eq!(parse_swing_every_n("4"), Ok(4));
        assert!(parse_swing_every_n("0")
            .unwrap_err()
            .contains("reclaimStrategy: swing"));
        assert!(parse_swing_every_n("many").unwrap_err().contains("many"));
        // 0 is a legitimate range threshold — anything shared at all
        // prefers the swing — so only the parse refuses here.
        assert_eq!(parse_range_threshold("0"), Ok(0));
        assert!(parse_range_threshold("-3").unwrap_err().contains("-3"));

        // ThinState persistence round-trip (pre-15i state files load as
        // all-None via #[serde(default)] on the field).
        let json = serde_json::to_string(&full).unwrap();
        assert_eq!(serde_json::from_str::<ReclaimParams>(&json).unwrap(), full);
        assert_eq!(serde_json::to_string(&empty).unwrap(), "{}");
    }

    /// §7.1: lazy timing without a watermark evicts nothing, ever — the
    /// pool grows until it is full. The stage is the first point where
    /// the StorageClass dial and the node env meet, so it is where the
    /// pair is checked.
    #[test]
    fn lazy_timing_without_a_watermark_fails_the_stage() {
        use crate::{ReclaimPolicy, ReclaimTiming};

        let lazy = ReclaimPolicy {
            timing: ReclaimTiming::Lazy,
            ..ReclaimPolicy::default()
        };
        let err = check_lazy_needs_watermark(&lazy, None).unwrap_err();
        assert!(err.contains("SKYNBD_ADVANCE_POOL_WATERMARK"), "{err}");
        assert!(err.contains("reclaimTiming"), "{err}");
        // With the watermark configured, lazy is exactly the policy §7.1
        // describes.
        check_lazy_needs_watermark(&lazy, Some(0.8)).unwrap();
        // Eager never needs one.
        check_lazy_needs_watermark(&ReclaimPolicy::default(), None).unwrap();
    }

    fn healthy_status() -> serde_json::Value {
        serde_json::json!({
            "phase": "backfilling", "ops_done": 3, "ops_total": 10,
            "attempts": 0, "last_error": null
        })
    }

    /// Happy path: pending twice, then the (simulated) cycle completes
    /// the request — Ok, with the stale alias cleared before filing and
    /// the fresh one cleaned up after.
    #[tokio::test(flavor = "multi_thread")]
    async fn final_publish_waits_until_the_request_completes() {
        let (base, stub) = spawn_stub(2, healthy_status(), 1).await;
        let client = crate::cp::Client::new(&base, None);
        await_final_publish(
            &client,
            "tv-lin",
            FINAL_SNAP,
            Duration::from_secs(10),
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert_eq!(stub.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            stub.deletes.load(Ordering::SeqCst),
            2,
            "stale alias cleared before filing AND after publishing"
        );
    }

    /// D-M12 retains completed request status for CSI replay timestamps;
    /// D-M13 must recognize that form rather than waiting for a 404.
    #[tokio::test(flavor = "multi_thread")]
    async fn final_publish_accepts_a_retained_completed_status() {
        let (base, stub) = spawn_stub(
            -1,
            serde_json::json!({
                "phase": "visible", "ops_done": 10, "ops_total": 10,
                "attempts": 0, "last_error": null, "completed": true
            }),
            1,
        )
        .await;
        let client = crate::cp::Client::new(&base, None);
        await_final_publish(
            &client,
            "tv-lin",
            FINAL_SNAP,
            Duration::from_secs(10),
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert_eq!(stub.requests.load(Ordering::SeqCst), 1);
        assert_eq!(
            stub.deletes.load(Ordering::SeqCst),
            2,
            "stale alias cleared before filing AND after completed status"
        );
    }

    /// A 404 with no published alias is a lost in-memory request (the
    /// server restarted mid-wait), NOT completion: the wait re-files
    /// and only accepts once the alias is really visible.
    #[tokio::test(flavor = "multi_thread")]
    async fn final_publish_refiles_after_a_server_restart() {
        let (base, stub) = spawn_stub(0, healthy_status(), 2).await;
        let client = crate::cp::Client::new(&base, None);
        await_final_publish(
            &client,
            "tv-lin",
            FINAL_SNAP,
            Duration::from_secs(10),
            Duration::from_millis(20),
        )
        .await
        .unwrap();
        assert!(
            stub.requests.load(Ordering::SeqCst) >= 2,
            "the lost request was re-filed"
        );
    }

    /// Failure path: the cycle never publishes — the error carries the
    /// last observed D-M12 status so the unstage failure says WHY.
    #[tokio::test(flavor = "multi_thread")]
    async fn final_publish_times_out_with_the_cycle_status() {
        let (base, _stub) = spawn_stub(
            -1,
            serde_json::json!({
                "phase": "capturing", "ops_done": 0, "ops_total": 0,
                "attempts": 4, "last_error": "freeze: device busy"
            }),
            1,
        )
        .await;
        let client = crate::cp::Client::new(&base, None);
        let err = await_final_publish(
            &client,
            "tv-lin",
            FINAL_SNAP,
            Duration::from_millis(200),
            Duration::from_millis(30),
        )
        .await
        .unwrap_err();
        for needle in ["capturing", "attempts=4", "freeze: device busy"] {
            assert!(err.contains(needle), "missing {needle:?} in: {err}");
        }
    }
}

//! The cycle engine: capture → prepare → backfill → publish → promote
//! (agent design doc §8), plus execution of every [`RecoveryAction`].
//!
//! Every phase transition is `persist → act` or `act → persist` per the
//! V3.2 §15.4 ordering rules, and a crash seam ([`CrashPoints`]) sits at
//! each boundary so the §21.1 matrix can kill the engine everywhere a real
//! crash could land. Restart = build a fresh `CycleEngine` over the same
//! stack/plane and call [`CycleEngine::recover`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::control::{CaptureBinding, ControlPlane, CpError, CycleProgress, PreparedInfo, RangeOp};
use crate::planner::{plan, RecoveryAction, RemoteObservation, RemoteSnapStatus};
use crate::policy::{decide, ExecutorChoice, ReclaimPolicy, ReclaimTiming};
use crate::stack::{DeltaRange, LocalStack, Pressure, SnapRole, StackError, SwingHooks};
use crate::state::{
    AgentVolumeState, BaselinePair, LoadedState, LocalSnap, OpCursor, Phase, PreparedRef,
    ServedRequest, StateError, StateStore,
};

/// Shared ranges one eviction call may discard (one freeze window), and
/// the per-cycle bound on windows — enough for 8 × 1024 ranges of
/// pressure relief before the cycle proceeds (watermark promotion still
/// backstops a pool the eviction cannot drain).
const EVICT_BATCH_RANGES: usize = 1024;
const EVICT_MAX_BATCHES: u32 = 8;

/// §8.1 rule 1's window: how much of the emergency advance the
/// watermark must reserve write-rate headroom for — capture (freeze +
/// snap) plus publish plus repoint, before the port executor's
/// allocation-free drain begins.
///
/// A **floor, never a ceiling.** §8.1 rule 1 names a duration, not a
/// constant, and the duration it names is the one the rescue will
/// actually take on this deployment. Guessing it is worst exactly where
/// §8 exists: on a degraded link the publish dominates and runs for
/// minutes, so a fixed 30 s under-reserves by an order of magnitude at
/// the moment the rescue is slowest. So the window is self-tuning the
/// same way the rate is — [`CycleEngine::effective_window`] takes
/// `max(this, observed)` over the wall time of recent ADVANCE cycles
/// (bounded above by [`OBSERVED_WINDOW_CAP`] × this value), and a
/// deployment whose advances are quick keeps this value rather than
/// shrinking below it (a fast advance today is no promise about the
/// one that runs during an outage).
const DEFAULT_ADVANCE_WINDOW: Duration = Duration::from_secs(30);

/// How much of an observation survives each subsequent fold: a decayed
/// maximum, not a last-sample or a plain mean.
///
/// A plain maximum never forgets a one-off stall; a last-sample reading
/// lets one fast advance collapse the reserve the slow ones earned,
/// which is the wrong direction for a margin whose whole job is to
/// still be there when things go badly. Decaying a maximum keeps the
/// pessimistic observation and lets it age out over roughly a dozen
/// folds once the workload — or the link — really does calm down.
const OBSERVED_DECAY: f64 = 0.9;

/// The ceiling on the observed window, as a multiple of the configured
/// floor. The observation decays per ADVANCE, not per cycle, so on a
/// volume that advances rarely (a small workload under a generous
/// watermark) a single pathological measurement — one publish that ran
/// for an hour behind a dead link — would otherwise hold the margin at
/// that value for the life of the process, promoting every cycle on a
/// span nothing repeats.
const OBSERVED_WINDOW_CAP: u32 = 10;

/// Fold one sample into a decayed maximum: the sample if it is the new
/// worst, otherwise the previous value aged by [`OBSERVED_DECAY`].
/// Generic because §8.1 rule 1 measures both its terms this way and for
/// the same reason — the window in wall time, the write rate in
/// bytes/sec.
fn decay_observed<T: Decaying>(prev: T, sample: T) -> T {
    let aged = prev.aged(OBSERVED_DECAY);
    if sample > aged {
        sample
    } else {
        aged
    }
}

/// The carriers [`decay_observed`] spans: rule 1's two measured terms.
trait Decaying: Copy + PartialOrd {
    fn aged(self, factor: f64) -> Self;
}

impl Decaying for Duration {
    fn aged(self, factor: f64) -> Self {
        self.mul_f64(factor)
    }
}

impl Decaying for f64 {
    fn aged(self, factor: f64) -> Self {
        self * factor
    }
}

/// Rule 1's window as the margin uses it: the configured floor, raised
/// by observation but never past [`OBSERVED_WINDOW_CAP`] × the floor.
///
/// The ceiling is measured from the floor and never from one below the
/// default: a deployment that configures a very small (or zero) window
/// is asking the measurement to lead, and ten times nearly nothing
/// would silently disable the observation the cap exists only to bound.
fn bounded_window(floor: Duration, observed: Duration) -> Duration {
    let ceiling = floor.max(DEFAULT_ADVANCE_WINDOW) * OBSERVED_WINDOW_CAP;
    floor.max(observed.min(ceiling))
}

/// The other half of rule 1's margin: the dirty page cache fsfreeze
/// flushes at the head of the rescue, which lands in the pool *before*
/// anything is published. Rate-independent, so it is a flat reserve.
const DEFAULT_DIRTY_FLUSH_RESERVE: u64 = 64 << 20;

/// One sample pair's write rate in bytes/sec between two
/// `(pool_used, at)` readings — §8.1 rule 1's `write_rate`, measured
/// rather than configured. `None` when the samples are not separated in
/// time. The series the margin reads is the decayed maximum of these
/// (see [`CycleEngine::sample_and_margin`]).
///
/// Only growth counts: a reclaim or an eviction between cycles shrinks
/// `used`, and that is not a negative write rate — it is the pool
/// giving space back, which the margin must not read as headroom the
/// workload will keep producing.
fn derive_write_rate(prev_used: u64, now_used: u64, dt: Duration) -> Option<f64> {
    let secs = dt.as_secs_f64();
    if secs <= 0.0 {
        return None;
    }
    Some(now_used.saturating_sub(prev_used) as f64 / secs)
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("stack: {0}")]
    Stack(#[from] StackError),
    #[error("control plane: {0}")]
    Cp(#[from] CpError),
    #[error("state: {0}")]
    State(#[from] StateError),
    #[error("quarantine: {0}")]
    Quarantine(&'static str),
    /// Invariant F (task 15h): this stage's epoch is superseded — the
    /// volume is owned by `holder` at `epoch` now. TERMINAL: the cycle
    /// loop must abandon (no retry), and unstage takes the D-M13
    /// quarantine-then-teardown branch. Produced only from
    /// [`CpError::Fenced`] — the authenticated, epoch-specific refusal —
    /// never from transport errors or ordinary conflicts.
    #[error(
        "fenced: stage epoch superseded, volume owned by {holder:?} at epoch {epoch} (Invariant F)"
    )]
    Fenced { holder: String, epoch: u64 },
    #[error("injected crash at point {0}")]
    InjectedCrash(u32),
    #[error("verify mismatch at offset {0}")]
    VerifyMismatch(u64),
}

/// Crash seam: fails at the `at`-th boundary crossing (agent design doc
/// §13, the compiled-in test seam). `None` ⇒ never fires; the counter still
/// runs so tests can size the matrix.
#[derive(Debug, Default)]
pub struct CrashPoints {
    pub at: Option<u32>,
    pub counter: u32,
}

impl CrashPoints {
    fn hit(&mut self) -> Result<(), AgentError> {
        let n = self.counter;
        self.counter += 1;
        if self.at == Some(n) {
            return Err(AgentError::InjectedCrash(n));
        }
        Ok(())
    }
}

/// Which of the two D-M15 §4 cycle kinds to run. Every cycle publishes
/// (that is the RPO); only advance cycles run the §3 sequence — repoint,
/// fence, reclaim — and end in the rebaselined pair (Invariant C).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CycleKind {
    Publish,
    Advance,
}

pub struct CycleEngine<L, C> {
    pub stack: L,
    pub cp: C,
    pub store: StateStore,
    pub volume: String,
    pub crash: CrashPoints,
    /// `advancePoolWatermark` (§4): pool occupancy fraction above which a
    /// publish cycle is promoted to an advance cycle. `None` = never —
    /// for the occupancy term AND for §8.1's margin term. A deployment
    /// that never opted into watermark advances must not start getting
    /// them from the margin rule.
    advance_watermark: Option<f64>,
    /// §6.2 retention: run a server-side thinning pass (under this
    /// node's fence epoch) after each successful advance. `None` = off.
    retention_window_s: Option<u64>,
    /// Backfill ops per upload batch (issue #97): reads, hashes, one
    /// `ensure` and the batch's object uploads run together, then the
    /// batch attaches in op order. 1 = one op at a time, the historical
    /// behavior and the default.
    publish_pipeline_depth: usize,
    /// Above depth 1, stage batch k+1 while batch k attaches (default
    /// true; see [`CycleEngine::with_publish_overlap`]).
    publish_overlap: bool,
    /// Mixed attach/clear batches: whether they go through
    /// [`ControlPlane::apply`], and this cycle's fallback state.
    mixed_apply: MixedApplyState,
    /// §8.1 rule 1: the configured FLOOR on how long the rescue advance
    /// needs before its port executor can drain a full pool (capture +
    /// publish + repoint). The window actually used is
    /// [`Self::effective_window`].
    advance_window: Duration,
    /// The decayed maximum wall time of recent advance cycles — rule
    /// 1's window measured instead of assumed. `ZERO` until this engine
    /// has run one advance; the configured floor covers that gap.
    ///
    /// In memory on purpose: a restart falls back to the configured
    /// floor and re-measures within an advance or two, which is cheaper
    /// than a record-format bump (every one of those quarantines every
    /// pool on the node and abandons its unpublished delta).
    observed_advance: Duration,
    /// §8.1 rule 1: the flat reserve for the dirty data fsfreeze pushes
    /// into the pool at the head of that rescue.
    dirty_flush_reserve: u64,
    /// The previous cycle's `(pool_used_bytes, sampled_at)` — the other
    /// end of the rate measurement. Sampled inside `run_cycle` only, so
    /// explicit-kind cycles and recovery leave it alone.
    last_sample: Option<(u64, Instant)>,
    /// Bytes/sec the workload has been observed to produce — the
    /// decayed maximum over the sample pairs, not the last pair alone.
    /// `None` until a second sample exists, and then the margin term is
    /// simply absent (occupancy alone decides) rather than guessed.
    write_rate: Option<f64>,
    /// The §7/§7.1 reclaim dials: executor strategy, timing, and the
    /// compaction thresholds.
    policy: ReclaimPolicy,
    /// The stage-time lease epoch (Invariant F, task 15h), persisted
    /// into every state record this engine writes.
    epoch: Option<u64>,
    /// Set when any control-plane call answered with the epoch-specific
    /// `Fenced` refusal: the terminal state. Once set, planning
    /// short-circuits to `Abandon` and every public entry point returns
    /// [`AgentError::Fenced`].
    fenced: Option<(String, u64)>,
    /// A major compaction that recovery — not this run of the advance —
    /// completed (§7 flipped arm). The durable counter is zeroed the
    /// moment the flip is reconciled, but the advance's tail then
    /// re-runs and its executor choice is `NoOp` (R is already empty),
    /// which would age the counter to 1 and pull the next forced
    /// compaction an advance early. Nothing durable can carry this: the
    /// flip flag is cleared by the same persist that zeroes the
    /// counter, and re-deriving "a swing just landed" from observation
    /// is exactly what the flag existed to avoid.
    recovered_swing: bool,
    /// Invariant B (§8.1): the last mask re-assert failed. Engine-local
    /// on purpose — the condition is re-derived from the kernel at the
    /// head of every cycle, so persisting it would buy nothing a fresh
    /// process does not learn within one interval, at the price of a
    /// record-format bump that costs every volume on the node a
    /// quarantine and a re-seed. It exists only so the recovery message
    /// fires on the transition rather than every cycle.
    shim_unmasked: bool,
    /// Whether the "no shim configured, so this volume is unguarded"
    /// report has already been made for this volume. Same reasoning as
    /// [`Self::shim_unmasked`] and one step stronger: on a real stack
    /// the condition cannot change without a config change and the
    /// restage that reloads it, so it is worth saying exactly once.
    shim_unconfigured: bool,
    /// CSI request names the cycle loop selected for the next fresh
    /// capture (D-M12). Bound durably to the candidate only once its cut
    /// exists; a resumed candidate never picks them up.
    capture_requests: Vec<String>,
    /// §8.1 rule 3's dials: `(occupancy X, duration T)`. `None` = the
    /// rule is off, which is the default and the only safe one — a
    /// deliberate `EROFS` is still a destructive act, and a deployment
    /// that never asked for it must not have its workload stopped by an
    /// agent it only asked to publish. Same shape and same reasoning as
    /// [`Self::advance_watermark`].
    ro_watermark: Option<(f64, Duration)>,
    /// When the current run of failing cycles began — rule 3's T. Reset
    /// by any success.
    ///
    /// In memory on purpose, and the direction it errs is the safe one:
    /// a restart forgets that publishes were failing, so the clock
    /// starts over and the remount comes LATER than it might have. A
    /// fresh process has genuinely not observed a failing publish yet,
    /// and buying the memory would cost a record-format bump — which
    /// quarantines every pool on the node.
    failing_since: Option<Instant>,
    /// Whether rule 3 has already taken this volume read-only. Latches
    /// the remount to once per outage rather than once per cycle, and
    /// is what §8.2 step 5 reads to force the port executor.
    degraded: bool,
    /// Whether the cycle now running has actually handed pool blocks
    /// back — a port reclaim, a swing, or a lazy eviction.
    ///
    /// §8.2 step 6 is written against the eager path ("advance:
    /// repoint + PORT reclaim … must complete BEFORE any remount"),
    /// but the reason it gives is about space, not about which executor
    /// produced it: journal replay allocates, so a filesystem remounted
    /// read-write over a pool that is still full re-wedges within
    /// seconds. Under §7.1 lazy timing an advance deliberately frees
    /// NOTHING — the published set stays pooled as read cache — so
    /// keying the restore on "an advance completed" would do exactly
    /// what the step forbids. Keying it on space returned is the same
    /// rule, stated at the level its rationale actually lives.
    freed_this_cycle: bool,
    /// Whether the one-shot restart adoption has run. Re-deriving every
    /// cycle would fight `restore_after_reclaim`: a re-derivation racing
    /// the restore's own remount would re-latch `degraded` off a stale
    /// reading.
    ro_adoption_checked: bool,
    state: Option<AgentVolumeState>,
    tick: u64,
}

impl<L: LocalStack, C: ControlPlane> CycleEngine<L, C> {
    pub fn open(stack: L, cp: C, store: StateStore, volume: &str) -> Result<Self, AgentError> {
        let state = match store.load(volume)? {
            LoadedState::Present(s) => Some(*s),
            LoadedState::Absent | LoadedState::Corrupt => None,
        };
        let tick = state.as_ref().map(|s| s.updated_at_ms).unwrap_or(0);
        let epoch = state.as_ref().and_then(|s| s.epoch);
        Ok(Self {
            stack,
            cp,
            store,
            volume: volume.to_string(),
            crash: CrashPoints::default(),
            advance_watermark: None,
            retention_window_s: None,
            publish_pipeline_depth: 1,
            publish_overlap: true,
            mixed_apply: MixedApplyState {
                on: true,
                split_this_cycle: false,
                warned: false,
            },
            advance_window: DEFAULT_ADVANCE_WINDOW,
            observed_advance: Duration::ZERO,
            dirty_flush_reserve: DEFAULT_DIRTY_FLUSH_RESERVE,
            last_sample: None,
            write_rate: None,
            policy: ReclaimPolicy::default(),
            epoch,
            fenced: None,
            recovered_swing: false,
            shim_unmasked: false,
            shim_unconfigured: false,
            capture_requests: Vec::new(),
            ro_watermark: None,
            failing_since: None,
            degraded: false,
            freed_this_cycle: false,
            ro_adoption_checked: false,
            state,
            tick,
        })
    }

    /// Whether to send a mixed attach/clear batch as one
    /// [`ControlPlane::apply`] (default true). False forces one call per
    /// same-kind run — what a server without `apply` gets anyway; also an
    /// escape hatch, and how the crash matrix reaches the seams between
    /// a split batch's runs.
    pub fn with_mixed_apply(mut self, on: bool) -> Self {
        self.mixed_apply.on = on;
        self
    }

    /// Backfill upload batch depth (issue #97); 0 is treated as 1. See
    /// [`PIPELINE_BATCH_BYTES`] for the batch's memory bound.
    pub fn with_publish_pipeline_depth(mut self, depth: usize) -> Self {
        self.publish_pipeline_depth = depth.max(1);
        self
    }

    /// Overlap consecutive backfill batches above depth 1 (default true):
    /// batch k+1 is read, hashed, ensured and uploaded while batch k
    /// attaches. False stages each batch only after the previous one has
    /// attached — the pre-overlap sequence, kept as an escape hatch that
    /// keeps the batching (config `publishOverlap`, CSI
    /// `SKYNBD_PUBLISH_OVERLAP`) and as the baseline the overlap is
    /// measured against.
    pub fn with_publish_overlap(mut self, on: bool) -> Self {
        self.publish_overlap = on;
        self
    }

    /// Promote publish cycles to advance cycles when pool occupancy
    /// crosses `fraction` (0.0–1.0) — the §4 watermark backstop.
    /// Arm the post-advance retention pass (§6.2): the advance is the
    /// volume's natural "old generations just became thinnable" moment,
    /// and the node is the only authority that can act while it holds
    /// the lease.
    pub fn with_retention_window(mut self, secs: u64) -> Self {
        self.retention_window_s = Some(secs);
        self
    }

    pub fn with_advance_watermark(mut self, fraction: f64) -> Self {
        self.advance_watermark = Some(fraction);
        self
    }

    /// §8.1 rule 1's window: the span of the emergency advance the
    /// margin must keep free at the observed write rate (default 30 s).
    /// A **floor** — an advance observed to take longer raises it, and
    /// nothing lowers it below this value.
    pub fn with_advance_window(mut self, window: Duration) -> Self {
        self.advance_window = window;
        self
    }

    /// Rule 1's window as the margin actually uses it: the configured
    /// floor, or the decayed maximum of observed advance durations when
    /// this deployment's advances have proven slower than the guess —
    /// bounded by [`OBSERVED_WINDOW_CAP`] × the floor, since the decay
    /// runs per advance and a rarely-advancing volume would otherwise
    /// carry one outlier forever.
    pub fn effective_window(&self) -> Duration {
        bounded_window(self.advance_window, self.observed_advance)
    }

    /// §8.1 rule 3: `publishFailingReadOnlyWatermark`. Once publishes
    /// have been failing for `after` AND pool occupancy is at or above
    /// `fraction`, take the filesystem read-only rather than wait for
    /// the kernel to do it badly.
    ///
    /// BOTH terms are required, and the second is what makes the rule
    /// safe to arm. A failing publish on a quiet pool is an outage to
    /// ride out — the node has room, the delta is intact, and stopping
    /// the workload would be pure loss. It is the pool climbing *while*
    /// nothing can be published that has exactly one ending, and rule 3
    /// only exists to pick which one.
    ///
    /// Unset by default: see [`Self::ro_watermark`].
    pub fn with_ro_watermark(mut self, fraction: f64, after: Duration) -> Self {
        self.ro_watermark = Some((fraction, after));
        self
    }

    /// Whether §8.1 rule 3 currently holds this volume read-only — an
    /// observability seam, and the input §8.2 step 5 turns into a forced
    /// port executor.
    pub fn degraded(&self) -> bool {
        self.degraded
    }

    /// §8.1 rule 1's flat reserve for the dirty-data flush fsfreeze
    /// performs at the head of that advance (default 64 MiB).
    pub fn with_dirty_flush_reserve(mut self, bytes: u64) -> Self {
        self.dirty_flush_reserve = bytes;
        self
    }

    /// The §7 reclaim policy (per-StorageClass dials).
    pub fn with_policy(mut self, p: ReclaimPolicy) -> Self {
        self.policy = p;
        self
    }

    /// The stage-time lease epoch (Invariant F, task 15h), persisted
    /// with every state record. `None` keeps whatever the loaded record
    /// carried.
    pub fn with_epoch(mut self, epoch: Option<u64>) -> Self {
        if epoch.is_some() {
            self.epoch = epoch;
        }
        self
    }

    /// Whether this engine hit the terminal Invariant F fence, and whom
    /// the volume belongs to now.
    pub fn fenced(&self) -> Option<&(String, u64)> {
        self.fenced.as_ref()
    }

    /// Terminal-error seam (task 15h): every public entry point routes
    /// its result through here, so the epoch-specific `CpError::Fenced`
    /// — wherever in the cycle it surfaced — becomes the terminal
    /// [`AgentError::Fenced`] and is remembered for planning.
    fn seal<T>(&mut self, r: Result<T, AgentError>) -> Result<T, AgentError> {
        match r {
            Err(AgentError::Cp(CpError::Fenced { holder, epoch })) => {
                self.fenced = Some((holder.clone(), epoch));
                Err(AgentError::Fenced { holder, epoch })
            }
            other => other,
        }
    }

    pub fn state(&self) -> Option<&AgentVolumeState> {
        self.state.as_ref()
    }

    /// Select the pending CSI requests the next fresh capture binds to
    /// (D-M12). Requests already bound ([`Self::is_request_bound`]) must
    /// not be passed: their cut exists or is gone.
    pub fn set_capture_requests(&mut self, requests: Vec<String>) {
        self.capture_requests = requests;
    }

    /// Whether `name` is already tied to a capture — the in-flight
    /// candidate, a published generation awaiting its alias, or a
    /// discarded cut awaiting its loss report. Such a request must never
    /// join a new capture: its caller may already hold that cut's time.
    pub fn is_request_bound(&self, name: &str) -> bool {
        self.state.as_ref().is_some_and(|s| {
            s.candidate_requests.iter().any(|n| n == name)
                || s.served_requests.iter().any(|r| r.name == name)
                || s.lost_requests.iter().any(|n| n == name)
        })
    }

    /// The in-flight candidate carries request bindings, so publishing it
    /// is owed to callers rather than left to the interval cadence.
    pub fn has_candidate_requests(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| !s.candidate_requests.is_empty())
    }

    /// Requests whose candidate is published and still need alias +
    /// completion.
    pub fn served_requests(&self) -> Vec<ServedRequest> {
        self.state
            .as_ref()
            .map(|s| s.served_requests.clone())
            .unwrap_or_default()
    }

    /// Forget a served request once its alias exists and its request is
    /// completed.
    pub fn finish_served(&mut self, name: &str) -> Result<(), AgentError> {
        if !self
            .state
            .as_ref()
            .is_some_and(|s| s.served_requests.iter().any(|r| r.name == name))
        {
            return Ok(());
        }
        let name = name.to_string();
        self.persist(move |s| s.served_requests.retain(|r| r.name != name))
    }

    /// Every request whose cut exists, with that cut's time.
    pub fn capture_bindings(&self) -> Vec<CaptureBinding> {
        let Some(s) = self.state.as_ref() else {
            return Vec::new();
        };
        let candidate_time = s
            .candidate
            .as_ref()
            .map(|c| c.created_at_ms)
            .filter(|&t| t != 0);
        let candidate = candidate_time.into_iter().flat_map(|t| {
            s.candidate_requests.iter().map(move |name| CaptureBinding {
                name: name.clone(),
                capture_time_ms: t,
            })
        });
        let served = s
            .served_requests
            .iter()
            .filter(|r| r.capture_time_ms != 0)
            .map(|r| CaptureBinding {
                name: r.name.clone(),
                capture_time_ms: r.capture_time_ms,
            });
        candidate.chain(served).collect()
    }

    /// Re-announce bindings and report lost requests outside a cycle.
    ///
    /// A lost request is reported whenever it is `pending` on the control
    /// plane again, and is NOT forgotten when the report is accepted. The
    /// API keeps request status in memory: after a restart, the CSI
    /// controller's next re-POST recreates the request as fresh (and a
    /// report landing before that re-POST is accepted as a no-op). The
    /// tombstone here is the only record that the caller may already hold
    /// the discarded cut's time, so it must outlive both — see
    /// [`LOST_REQUESTS_RETAINED`] for its bound, and [`Self::forget_lost`]
    /// for names that are deliberately reused.
    pub async fn report_captures(&mut self, pending: &[String]) -> Result<(), AgentError> {
        let lost: Vec<String> = self
            .state
            .as_ref()
            .map(|s| {
                s.lost_requests
                    .iter()
                    .filter(|n| pending.contains(n))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        self.cp
            .report_captures(&self.capture_bindings(), &lost)
            .await?;
        Ok(())
    }

    /// Drop a lost-request tombstone. Only for a name whose filer re-files
    /// it on purpose and holds no capture time — D-M13's reserved final
    /// unstage request — after its loss has been reported, so the re-filed
    /// request gets a fresh cut instead of being refused forever.
    pub fn forget_lost(&mut self, name: &str) -> Result<(), AgentError> {
        if !self
            .state
            .as_ref()
            .is_some_and(|s| s.lost_requests.iter().any(|n| n == name))
        {
            return Ok(());
        }
        let name = name.to_string();
        self.persist(move |s| s.lost_requests.retain(|n| *n != name))
    }

    /// Whether any lost-request tombstones are held.
    pub fn has_lost_requests(&self) -> bool {
        self.state
            .as_ref()
            .is_some_and(|s| !s.lost_requests.is_empty())
    }

    /// Best-effort D-M12 progress report: observability must never fail
    /// or stall the cycle, so transport errors are swallowed here.
    async fn report(&self, phase: &'static str, ops_done: u64, ops_total: u64) {
        let _ = self
            .cp
            .report_progress(CycleProgress {
                phase,
                ops_done,
                ops_total,
                captures: self.capture_bindings(),
            })
            .await;
    }

    fn publisher(&self) -> String {
        format!("agent-publish:{}", self.volume)
    }

    fn baseline_holder(&self) -> String {
        format!("agent-baseline:{}", self.volume)
    }

    /// Best-effort unpin that still respects the fence (task 15h):
    /// unpin is idempotent cleanup, so ordinary failures are swallowed
    /// — but the epoch-specific `Fenced` refusal must propagate. A
    /// fenced stage terminates; shrugging the refusal off would let the
    /// cycle keep running (and keep retrying) against a superseded
    /// epoch.
    async fn unpin_best_effort(&self, ref_name: &str, holder: &str) -> Result<(), AgentError> {
        match self.cp.unpin(ref_name, holder).await {
            Err(CpError::Fenced { holder, epoch }) => {
                Err(AgentError::Cp(CpError::Fenced { holder, epoch }))
            }
            Ok(()) | Err(_) => Ok(()),
        }
    }

    fn persist(&mut self, mutate: impl FnOnce(&mut AgentVolumeState)) -> Result<(), AgentError> {
        let mut s = self.state.clone().unwrap_or(AgentVolumeState {
            volume: self.volume.clone(),
            phase: Phase::Idle,
            baseline: None,
            candidate: None,
            prepared: None,
            cursor: None,
            advancing: false,
            epoch: None,
            advances_since_swing: 0,
            swing_flipped: None,
            candidate_requests: Vec::new(),
            served_requests: Vec::new(),
            lost_requests: Vec::new(),
            updated_at_ms: 0,
        });
        let before = (
            s.candidate.clone(),
            s.prepared.as_ref().map(|p| p.target.clone()),
            s.candidate_requests.clone(),
        );
        mutate(&mut s);
        settle_request_bindings(&mut s, before);
        if self.epoch.is_some() {
            s.epoch = self.epoch; // the stage's fence token rides along
        }
        self.tick += 1;
        s.updated_at_ms = self.tick;
        self.store.save(&s)?;
        self.state = Some(s);
        self.crash.hit() // a crash lands *after* the durable write
    }

    /// Next generation ref name: `snap/g{n}` from the current baseline (or
    /// the in-flight target when resuming).
    fn next_target(&self) -> String {
        if let Some(p) = self.state.as_ref().and_then(|s| s.prepared.as_ref()) {
            return p.target.clone();
        }
        let n = self
            .state
            .as_ref()
            .and_then(|s| s.baseline.as_ref())
            .and_then(|b| b.remote.strip_prefix("snap/g"))
            .and_then(|g| g.parse::<u64>().ok())
            .map(|g| g + 1)
            .unwrap_or(0);
        format!("snap/g{n}")
    }

    fn candidate_name(&self, target: &str) -> String {
        format!("skynbd-{}-{}", self.volume, target.replace('/', "-"))
    }

    // -- observation --------------------------------------------------------

    async fn observe_remote(
        &self,
    ) -> Result<(RemoteObservation, Option<PreparedInfo>), AgentError> {
        let prepared = self.cp.find_prepared().await?;
        let target_ref = self
            .state
            .as_ref()
            .and_then(|s| s.prepared.as_ref())
            .map(|p| p.target.clone());
        let target = if let Some(p) = &prepared {
            RemoteSnapStatus::Prepared {
                watermark: p.op_watermark,
            }
        } else if let Some(t) = &target_ref {
            if self.cp.ref_exists(t).await? {
                RemoteSnapStatus::Visible {
                    pinned: self.cp.pinned_by(t, &self.publisher()).await?
                        || self.cp.pinned_by(t, &self.baseline_holder()).await?,
                }
            } else {
                RemoteSnapStatus::Absent
            }
        } else {
            RemoteSnapStatus::Absent
        };
        let (base_visible, base_pinned) =
            match self.state.as_ref().and_then(|s| s.baseline.as_ref()) {
                Some(b) => (
                    self.cp.ref_exists(&b.remote).await?,
                    self.cp.pinned_by(&b.remote, &b.pin_holder).await?,
                ),
                None => (false, false),
            };
        Ok((
            RemoteObservation {
                target,
                base_visible,
                base_pinned,
                // Sticky by design: once any transaction of this stage
                // was refused epoch-specifically, no later observation
                // can un-supersede the epoch.
                fenced: self.fenced.is_some(),
            },
            prepared,
        ))
    }

    // -- recovery -----------------------------------------------------------

    /// Observe, plan, execute — repeatedly, until the planner says the
    /// volume is quiescent. This is the startup entry point.
    pub async fn recover(&mut self) -> Result<(), AgentError> {
        let r = self.recover_inner().await;
        self.seal(r)
    }

    async fn recover_inner(&mut self) -> Result<(), AgentError> {
        // §7 crash windows: recovery cannot discriminate pre- from
        // post-retarget by looking at dm — a reboot loses every table —
        // so the discriminator IS the durable slot flag. Resolve it
        // before planning; both sweeps are idempotent.
        match self.state.as_ref().and_then(|s| s.swing_flipped) {
            Some(true) => {
                // Record names B: the flip is durable, slot A is
                // garbage. Finish the teardown and canonicalize; the
                // re-run §3 tail below then finds R (near-)empty and
                // completes step 7.
                self.stack.sweep_slot(true)?;
                self.persist(|s| {
                    s.swing_flipped = None;
                    s.advances_since_swing = 0;
                })?;
                // The compaction this advance asked for happened; the
                // tail below must not age the counter for re-running it.
                self.recovered_swing = true;
            }
            Some(false) => {
                // Record names A: slot B is garbage regardless of
                // whether the retarget executed — but the retarget may
                // have executed, so the sweep is also the repair (put
                // the shim back on A, thaw a freeze that outlived the
                // process, then discard B). It must therefore be
                // allowed to fail loudly: clearing the flag over a
                // failed sweep would strand a live B nothing revisits,
                // and — with the shim possibly still on it — hand the
                // next port reclaim a pool the record calls garbage.
                self.stack.sweep_slot(false)?;
                self.persist(|s| s.swing_flipped = None)?;
            }
            None => {}
        }
        // Invariant B on the restart path: both sweeps put a shim table
        // back, and a reboot brings the whole stack up from tables this
        // process never loaded. Re-derive the mask before any cycle work
        // — recovery is the one entry point a driver restart is
        // guaranteed to pass through.
        self.reassert_mask()?;
        for _ in 0..8 {
            let local = self.stack.observe();
            let (remote, prepared) = self.observe_remote().await?;
            match plan(self.state.as_ref(), &local, &remote) {
                RecoveryAction::Continue(Phase::Idle) => {
                    // Row 9: promotion observably complete, but if we died
                    // mid-cleanup the state record still says otherwise —
                    // rerun the idempotent promotion tail, then re-plan.
                    let unfinished = self
                        .state
                        .as_ref()
                        .map(|s| {
                            matches!(
                                s.phase,
                                Phase::Promoting | Phase::Visible | Phase::Advancing
                            )
                        })
                        .unwrap_or(false);
                    if !unfinished {
                        return self.normalize_idle();
                    }
                    let cand = self
                        .state
                        .as_ref()
                        .and_then(|s| s.candidate.as_ref())
                        .map(|c| c.lv_name.clone());
                    self.promote(cand.as_deref()).await?;
                }
                RecoveryAction::Bootstrap => return self.normalize_idle(),
                RecoveryAction::Continue(_) => {
                    // Candidate exists, nothing prepared: run from prepare.
                    let cand = local.candidates[0].clone();
                    self.resume_from_captured(&cand).await?;
                }
                RecoveryAction::ResumeBackfill { from_watermark } => {
                    let info = prepared.ok_or(AgentError::Quarantine(
                        "planner saw prepared but it vanished",
                    ))?;
                    let cand = local
                        .candidates
                        .first()
                        .cloned()
                        .ok_or(AgentError::Quarantine("resume without candidate"))?;
                    self.adopt_prepared(&info, &cand)?;
                    self.backfill_publish_promote(&cand, &info.snap_id, from_watermark)
                        .await?;
                }
                RecoveryAction::RepublishThenPromote => {
                    // Folded into ResumeBackfill by the planner; unreachable,
                    // but harmless if it ever fires:
                    continue;
                }
                RecoveryAction::PromoteOnly => {
                    let cand = local.candidates.first().cloned();
                    self.promote(cand.as_deref()).await?;
                }
                RecoveryAction::RebaselineFrom { remote_base } => {
                    self.rebaseline(&remote_base).await?;
                }
                RecoveryAction::Abort { delete_candidate } => {
                    self.finish_abort(delete_candidate, &local.candidates)
                        .await?;
                }
                // Invariant F: terminal — no retry, no further planning.
                RecoveryAction::Abandon { reason: _ } => {
                    let (holder, epoch) = self.fenced.clone().unwrap_or_default();
                    return Err(AgentError::Fenced { holder, epoch });
                }
                RecoveryAction::Quarantine(reason) => return Err(AgentError::Quarantine(reason)),
            }
        }
        Err(AgentError::Quarantine("recovery did not converge"))
    }

    /// Quiescent per the planner: scrub any stale in-flight fields left by
    /// a crash (e.g. phase `Capturing` persisted, then death before the
    /// snapshot existed — matrix row 1) so the record matches reality.
    fn normalize_idle(&mut self) -> Result<(), AgentError> {
        let dirty = self
            .state
            .as_ref()
            .map(|s| s.phase != Phase::Idle || s.candidate.is_some() || s.prepared.is_some())
            .unwrap_or(false);
        if dirty {
            self.persist(|s| {
                s.phase = Phase::Idle;
                s.advancing = false;
                s.candidate = None;
                s.prepared = None;
                s.cursor = None;
            })?;
        }
        Ok(())
    }

    /// Adopt a prepared root found on the server (e.g. crash between the
    /// server creating it and us persisting the id).
    fn adopt_prepared(&mut self, info: &PreparedInfo, _cand: &str) -> Result<(), AgentError> {
        let needs = self
            .state
            .as_ref()
            .and_then(|s| s.prepared.as_ref())
            .map(|p| p.snap_id != info.snap_id)
            .unwrap_or(true);
        if needs {
            let target = self.next_target();
            let snap_id = info.snap_id.clone();
            self.persist(move |s| {
                s.phase = Phase::Backfilling;
                s.prepared = Some(PreparedRef { snap_id, target });
            })?;
        }
        Ok(())
    }

    // -- the cycle ----------------------------------------------------------

    /// One full cycle: bootstrap (no baseline) or incremental. Runs as a
    /// publish cycle unless the pool crosses the advance watermark on
    /// EITHER §8.1 term — occupancy above `advancePoolWatermark`, or
    /// effective free space below the margin the rescue advance needs to
    /// survive its own freeze. Under lazy timing (§7.1), watermark
    /// pressure first evicts from the retained pair — the pool doubles
    /// as read cache, and eviction is its reclaim. Returns the published
    /// target ref name.
    pub async fn run_cycle(&mut self) -> Result<String, AgentError> {
        // §8.1 rule 3, before anything that can block. Everything above
        // the first control-plane call is local — the pool read, the
        // mask re-assert — which is what lets this fire during exactly
        // the failure it is for: a partition, where the publish that
        // set the clock running is also the call that will hang.
        self.degrade_if_publishes_are_failing();
        // The pool is read twice, on either side of the eviction,
        // because the two readings answer different questions.
        //
        // The RATE is sampled first. Sampling lives here and nowhere
        // else — an explicit-kind cycle (a snapshot request, D-M13's
        // final cycle) and recovery must leave the series alone rather
        // than seed it with the pool state of an unrelated moment — and
        // it has to precede any eviction: lazy timing frees pool blocks
        // on its way through, so sampling afterwards reads used ≤
        // previous and the growth the workload actually produced
        // vanishes, in precisely the mode that keeps the pool
        // deliberately full as a read cache.
        let sampled = self.stack.pressure();
        let margin = self.sample_and_margin(sampled.as_ref());
        if self.policy.timing == ReclaimTiming::Lazy {
            self.evict_under_pressure().await?;
        }
        // The KIND is decided from what the pool looks like AFTER it.
        // Eviction is reclaim (§7.1): a lazy volume that discarded its
        // way back under the watermark has already had the relief an
        // advance would buy it, and promoting anyway would pay a freeze
        // — plus the whole §3 tail — on every cycle for as long as the
        // pressure keeps recurring. Reading the pre-eviction pressure
        // here is what made that the behaviour.
        let pressure = self.stack.pressure();
        let kind = match (self.advance_watermark, pressure) {
            (Some(w), Some(p)) if p.total_bytes > 0 && p.occupancy() >= w => CycleKind::Advance,
            (Some(w), Some(p)) if p.total_bytes > 0 && p.effective_free < margin => {
                // The case operators need spelled out: nothing about
                // occupancy asked for this advance — the write rate did.
                eprintln!(
                    "skynbd-agent: {}: advancing on the §8.1 margin term — {} B free \
                     (the {} surface binds) is under the {margin} B the rescue needs \
                     at {:.0} B/s over {:?} plus a {} B dirty-flush reserve; occupancy \
                     {:.3} is below the {w:.3} watermark",
                    self.volume,
                    p.effective_free,
                    if p.fs_bound {
                        "backing filesystem"
                    } else {
                        "thin pool"
                    },
                    self.write_rate.unwrap_or(0.0),
                    self.effective_window(),
                    self.dirty_flush_reserve,
                    p.occupancy(),
                );
                CycleKind::Advance
            }
            _ => CycleKind::Publish,
        };
        self.run_cycle_as(kind).await
    }

    /// Fold this cycle's PRE-eviction `(pool_used, now)` into the rate
    /// series and return §8.1 rule 1's margin in bytes:
    /// `write_rate × effective_window + dirty_flush_reserve`.
    ///
    /// With no history there is no observed rate, and the margin is
    /// **zero** — not the bare reserve. A first cycle knows nothing
    /// about the workload, and the reserve alone would promote every
    /// freshly-staged nearly-full pool on no evidence at all.
    fn sample_and_margin(&mut self, pressure: Option<&Pressure>) -> u64 {
        let Some(p) = pressure else {
            return 0;
        };
        let now = Instant::now();
        if let Some((prev_used, at)) = self.last_sample {
            if let Some(rate) = derive_write_rate(prev_used, p.used_bytes, now.duration_since(at)) {
                // Decayed like the window, and for a reason one level
                // deeper than sampling order. `last_sample` records the
                // pre-eviction `used`, so the NEXT cycle measures its
                // growth against a baseline the eviction has since
                // undercut: an evicting cycle hands back more than the
                // writer produced, the delta saturates to zero, and
                // rule 1's margin collapses to the flat reserve one
                // cycle later — the blind spot moved, not closed.
                // Keeping the decayed maximum keeps the rate the
                // workload has proven it can produce, and ages it out
                // only over a run of genuinely quiet cycles.
                self.write_rate = Some(decay_observed(self.write_rate.unwrap_or(0.0), rate));
            }
        }
        self.last_sample = Some((p.used_bytes, now));
        let Some(rate) = self.write_rate else {
            return 0;
        };
        let ahead = rate * self.effective_window().as_secs_f64();
        (ahead.max(0.0) as u64).saturating_add(self.dirty_flush_reserve)
    }

    /// Adopt a read-only filesystem this process did not remount —
    /// §8.1 rule 3 surviving an agent restart.
    ///
    /// The remount outlives the process; `degraded` does not. Without
    /// this, an agent that restarts mid-outage — the ordinary path,
    /// since kubelet replays the stage onto the live attachment — comes
    /// back believing the volume is writable, `restore_after_reclaim`
    /// early-returns forever, and the network can return and the pool
    /// drain while the volume still sits at `EROFS` until a human runs
    /// `mount -o remount,rw`. `policy::decide`'s step-5 term goes quiet
    /// at the same moment, on the volume that most needs it.
    ///
    /// Re-derived rather than persisted: same posture as the
    /// guest-discard mask (Invariant B), and it costs no record bump.
    /// Note this is NOT symmetric with `failing_since`, which stays in
    /// memory deliberately — forgetting T errs *late*, and late is
    /// safe; forgetting `degraded` errs *permanent*.
    ///
    /// Safe to key on the mount alone because rule 3 is armed ONLY on
    /// rw-staged volumes (the CSI withholds the dials for a `ro:`
    /// stage), so a read-only mount under a configured rule 3 cannot be
    /// a deliberately read-only snapshot mount.
    ///
    /// Note this covers rule 3's remount and nothing else. A kernel
    /// error-flip does NOT produce a cleanly read-only super: ext4's
    /// own "Remounting filesystem read-only" lands in `emergency_ro`,
    /// which still reports `rw` to `statvfs` (measured via
    /// `trigger_fs_error`), so it is never adopted here. That volume
    /// needs an operator regardless, and §8.2 step 6's write-probe
    /// check is what keeps the agent from claiming otherwise.
    fn adopt_existing_degradation(&mut self) {
        if self.ro_watermark.is_none() || self.degraded || self.ro_adoption_checked {
            return;
        }
        // Spend the one-shot only on a DEFINITE answer. A stack that
        // errored has told us nothing, and burning the check on it
        // would silently restore the permanent-`EROFS` behaviour this
        // function exists to prevent. The race the one-shot guards
        // against needs a successful stale reading, which a
        // retry-on-error path cannot produce.
        let answer = self.stack.mount_is_read_only();
        if answer.is_some() {
            self.ro_adoption_checked = true;
        }
        if answer == Some(true) {
            self.degraded = true;
            eprintln!(
                "skynbd-agent: {}: adopting a read-only filesystem this process did not \
                 remount — §8.1 rule 3 fired before a restart. The volume returns to \
                 read-write once a cycle actually reclaims into the pool (§8.2 step 6).",
                self.volume,
            );
        }
    }

    /// §8.1 rule 3. Fires at most once per outage; never fails a cycle.
    fn degrade_if_publishes_are_failing(&mut self) {
        let Some((fraction, after)) = self.ro_watermark else {
            return;
        };
        if self.degraded {
            return;
        }
        let Some(since) = self.failing_since else {
            return;
        };
        if since.elapsed() < after {
            return;
        }
        // An unsized pool reads as empty (`Pressure::occupancy`), so
        // guarding on `total_bytes` is what keeps a stack that cannot
        // answer from reading as "0% full" and never firing — and, more
        // to the point, from reading as anything at all. Unknown
        // pressure means rule 3 has only one of its two terms, and one
        // term is not the rule.
        let Some(p) = self.stack.pressure().filter(|p| p.total_bytes > 0) else {
            return;
        };
        if p.occupancy() < fraction {
            return;
        }
        let elapsed = since.elapsed();
        eprintln!(
            "skynbd-agent: {}: taking the filesystem READ-ONLY — publishes have been failing \
             for {elapsed:?} and the pool is {:.1}% full (at or above the {:.1}% rule 3 \
             watermark). A clean EROFS now beats a dirty shutdown later: the writers stop, \
             nothing is damaged, and the volume resumes the moment an advance frees space.",
            self.volume,
            p.occupancy() * 100.0,
            fraction * 100.0,
        );
        if let Err(e) = self.stack.degrade_read_only() {
            // Logged, not propagated. Rule 3 is a mitigation; refusing
            // to attempt the publish because the mitigation failed
            // would give up the delta AND leave the filesystem exposed,
            // which is strictly worse than either alone.
            eprintln!(
                "skynbd-agent: {}: rule 3 remount failed ({e}) — the filesystem stays \
                 writable and will exhaust the way the kernel chooses",
                self.volume,
            );
            return;
        }
        self.degraded = true;
    }

    /// §8.2 step 6. Placement is the enforcement: this is called at the
    /// tail of a completed advance, after the reclaim has actually
    /// handed pool blocks back. Restoring earlier would re-wedge the
    /// filesystem within seconds — journal replay allocates, and a pool
    /// that has not been reclaimed yet has nothing to serve it with.
    ///
    /// Only an advance restores. A publish frees nothing, so a
    /// successful publish under degradation clears the failing clock
    /// (the outage is over) and still leaves the volume read-only until
    /// space actually comes back.
    fn restore_after_reclaim(&mut self) {
        if !self.degraded {
            return;
        }
        match self.stack.restore_read_write() {
            Ok(()) => {
                // The remount returning `Ok` is NOT evidence it took.
                // `MS_REMOUNT` succeeds on an aborted superblock and
                // changes nothing (measured: rc=0, `abort,emergency_ro`
                // still set, writes still `EROFS`), and neither
                // `statvfs` nor `access(W_OK)` can see the difference.
                // Trusting the return value here would clear the latch
                // and print "back READ-WRITE" about a volume that will
                // not accept a byte — a false all-clear, which is worse
                // than the stall it replaces: the operator log lies,
                // step 5's routing term goes quiet, and adoption cannot
                // recover it because an aborted super reports `rw`.
                if self.stack.mount_is_writable() == Some(false) {
                    eprintln!(
                        "skynbd-agent: {}: the read-write remount was accepted and did \
                         NOTHING — the superblock has aborted, so it still refuses every \
                         write. Staying degraded; this volume needs an operator (fsck / \
                         remount cycle), not another advance.",
                        self.volume,
                    );
                    return;
                }
                self.degraded = false;
                eprintln!(
                    "skynbd-agent: {}: filesystem back READ-WRITE — the advance reclaimed \
                     into the pool first (§8.2 step 6)",
                    self.volume,
                );
            }
            Err(e) => eprintln!(
                "skynbd-agent: {}: could not restore read-write after the advance ({e}) — \
                 the volume stays read-only and the next advance retries",
                self.volume,
            ),
        }
    }

    /// The observed write rate (§8.1 rule 1) in bytes/sec as of the last
    /// `run_cycle` sample — the decayed maximum, which is what the
    /// margin multiplies — or `None` before a second sample exists. An
    /// observability seam: it is the only rate-dependent input to the
    /// margin, so a test that pins it pins the margin's behaviour under
    /// eviction without reaching into the promotion decision.
    pub fn observed_write_rate(&self) -> Option<f64> {
        self.write_rate
    }

    /// Invariant B's per-cycle re-assert. Never fails the cycle: the
    /// alternative to publishing an unmasked volume's delta is not
    /// publishing it, which loses the delta as well as the mask. What it
    /// does instead is keep the lapse LOUD — on every cycle, until an
    /// assert takes.
    ///
    /// The stack's three answers get three treatments (see
    /// [`LocalStack::assert_guest_discard_mask`]). The middle one is
    /// the one worth naming here: a stack with no shim configured is
    /// not protected and never was, so reporting it every cycle would
    /// be noise about a condition only a config change can fix — but
    /// reporting it *nowhere* is how the mask came to have a silent
    /// opt-out that read exactly like a verified mask. Once per volume,
    /// per process.
    fn reassert_mask(&mut self) -> Result<(), AgentError> {
        let was_unmasked = self.shim_unmasked;
        match self.stack.assert_guest_discard_mask() {
            Ok(()) => {
                if was_unmasked {
                    eprintln!(
                        "skynbd-agent: {}: the guest-discard mask took again; the shim is \
                         masked (Invariant B)",
                        self.volume
                    );
                    self.shim_unmasked = false;
                }
            }
            Err(StackError::Unsupported(what)) => {
                if !std::mem::replace(&mut self.shim_unconfigured, true) {
                    eprintln!(
                        "skynbd-agent: {}: Invariant B's guest-discard mask is NOT asserted on \
                         this volume ({what}); nothing here refuses a guest fstrim, so a trim \
                         inside the guest can unmap blocks the origin serves and diverge the \
                         served image from the published generation. Every cycle publishes as \
                         normal — this is a configuration gap, not a lapse, and it is reported \
                         once per volume per process rather than every interval",
                        self.volume
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "skynbd-agent: {}: the guest-discard mask could NOT be asserted on this \
                     volume's shim ({e}); a mounted, thawed filesystem is passing guest \
                     fstrim through onto blocks the origin serves — the served image will \
                     diverge from the published generation in both directions (Invariant B). \
                     The cycle continues (an unpublished delta is the worse loss) and the \
                     assert is retried at the head of every cycle until one takes",
                    self.volume
                );
                self.shim_unmasked = true;
            }
        }
        Ok(())
    }

    /// §7.1 lazy eviction, bounded: while over the watermark, discard
    /// still-shared ranges of the retained pair in FIFO (LBA) order —
    /// heat is an optimization, not a dependency. Every eviction event
    /// is a reclaim and carries the Invariant E obligation, so the
    /// served-origin fence runs first against the PAIR's generation; a
    /// fence miss skips eviction entirely (e.g. an established pair
    /// rotated past the last repoint — its shared blocks are not
    /// origin-served yet, and the next advance unblocks them).
    async fn evict_under_pressure(&mut self) -> Result<(), AgentError> {
        let Some(w) = self.advance_watermark else {
            return Ok(());
        };
        let over = |usage: Option<(u64, u64)>| matches!(usage, Some((used, total)) if total > 0 && used as f64 / total as f64 >= w);
        if !over(self.stack.pool_usage()) {
            return Ok(());
        }
        let Some(pair) = self.state.as_ref().and_then(|s| s.baseline.clone()) else {
            return Ok(());
        };
        if pair.local.lv_name.is_empty() {
            return Ok(()); // rebaselined: nothing retained to evict
        }
        // Restage/reboot shape: the recorded LV is not activated, so
        // there is no retained pair member to evict from — same answer
        // as the rebaselined arm above, derived from observation.
        if !self.stack.observe().baselines.contains(&pair.local.lv_name) {
            return Ok(());
        }
        let served = self.cp.origin_served().await?;
        let ok = served
            .as_deref()
            .and_then(gen_number)
            .zip(gen_number(&pair.remote))
            .is_some_and(|(s, t)| s >= t);
        if !ok {
            return Ok(());
        }
        for _ in 0..EVICT_MAX_BATCHES {
            let freed = self.stack.evict(&pair.local.lv_name, EVICT_BATCH_RANGES)?;
            if freed > 0 {
                // §7.1: eviction IS reclaim. For §8.2 step 6 that is not
                // an analogy — it is the only thing a lazy volume ever
                // does that hands pool blocks back, so it is the only
                // thing that can license a read-write restore there.
                self.freed_this_cycle = true;
            }
            if freed == 0 || !over(self.stack.pool_usage()) {
                break;
            }
        }
        Ok(())
    }

    /// One full cycle of an explicit kind. The CSI cycle loop passes
    /// `Advance` when the cycle serves a configured snapshot (a pending
    /// snapshot request — including D-M13's final cycle, which is always
    /// an advance).
    pub async fn run_cycle_as(&mut self, kind: CycleKind) -> Result<String, AgentError> {
        let started = Instant::now();
        self.freed_this_cycle = false;
        // Before anything else, and on every path into a cycle: a
        // restart must not lose the fact that rule 3 already fired.
        self.adopt_existing_degradation();
        let r = self.run_cycle_inner(kind).await;
        // §8.1 rule 1's window, measured. An advance cycle CONTAINS the
        // rescue — capture, publish, repoint, in that order — so its
        // wall time bounds the span the margin has to reserve for,
        // whatever a config file guessed. The bracket is the whole of
        // `run_cycle_inner`, so it also covers the reclaim that follows
        // the repoint, which rule 1 explicitly excludes (the port
        // executor drains allocation-free). That is an over-measure in
        // the safe direction — a longer window reserves more — and it
        // is not worth an inner timing seam to shave.
        //
        // Only advances count: a publish cycle skips the repoint and
        // the reclaim, and timing it would read the window short. Only
        // successes count too: a cycle that failed halfway measures the
        // failure, not the span.
        if kind == CycleKind::Advance && r.is_ok() {
            self.observed_advance = decay_observed(self.observed_advance, started.elapsed());
            // §6.2: an advance is the moment intermediate generations
            // stop being structurally referenced — thin now, under this
            // node's epoch. Best-effort: the bytes wait, the data
            // doesn't, and the sweep debt is recorded server-side.
            if let Some(w) = self.retention_window_s {
                if let Err(e) = self.cp.thin(Some(w)).await {
                    tracing::warn!(volume = %self.volume, error = %e,
                        "post-advance retention pass failed (retried next advance)");
                }
            }
        }
        // §8.1 rule 3's T, folded on the SEALED result. `seal` is what
        // turns the plane's `Cp(Fenced)` into the terminal
        // `AgentError::Fenced`, so reading `r` directly would classify
        // every fenced cycle as an ordinary failing publish — the one
        // case that must not start this clock.
        let r = self.seal(r);
        match &r {
            Ok(_) => self.failing_since = None,
            // `Fenced` is terminal, not a failing publish: the node has
            // lost the volume to another holder, the loop is about to
            // exit, and remounting a filesystem this node no longer owns
            // helps nobody. Starting the clock here would also mean an
            // abandoned volume degrades itself on the way out.
            Err(AgentError::Fenced { .. }) => {}
            Err(_) => {
                self.failing_since.get_or_insert_with(Instant::now);
            }
        }
        // §8.2 step 6, and it runs here rather than inside the advance
        // for one reason: this is the first point at which the reclaim
        // is not merely done but DURABLE — `run_cycle_inner` has
        // persisted the rebaselined pair. A restore between the discard
        // and that persist would put the writers back on a pool whose
        // recovery record still describes the pre-advance shape.
        if r.is_ok() && self.freed_this_cycle {
            self.restore_after_reclaim();
        }
        r
    }

    async fn run_cycle_inner(&mut self, kind: CycleKind) -> Result<String, AgentError> {
        // Invariant B, once per cycle and before anything else: the mask
        // is re-derived from the kernel, never trusted. Every other
        // assert site is tied to an event (a stage, a retarget), and the
        // hazard this closes is precisely the one no event revisits — a
        // live, thawed, mounted shim left unmasked by a retarget whose
        // re-assert failed.
        self.reassert_mask()?;
        let target = self.next_target();

        // RESUME rather than re-capture, when the previous cycle left a
        // candidate it never managed to publish.
        //
        // That is not an edge case under §8.2, it is the ordinary shape
        // of an outage: `resume_from_captured` dies at `cp.prepare`, the
        // first control-plane call the cycle makes, leaving exactly
        // `phase = Captured, candidate = Some(..), prepared = None`. The
        // planner already has an arm for that state —
        // `RecoveryAction::Continue(Phase::Captured)`, "Row 2: S_n
        // exists, no P_n yet" — but `run` consults the planner once at
        // startup and then loops here, so the one state a partition
        // produces every interval was the one state this path could not
        // reach. Every cut cycle therefore re-froze a live guest
        // filesystem for a snapshot the next cycle would discard.
        //
        // Deliberately NOT by calling the planner: its decision needs a
        // remote observation, and that call is precisely what a
        // partition hangs — re-planning per cycle would add a
        // `READ_TIMEOUT` to every cut cycle to compute a plan whose
        // input it could not obtain. `prepared: None` is what makes the
        // local decision sound instead: no prepare was ever durable, so
        // there is no remote state to reconcile. The `RemoteSnapStatus`
        // disjunction the planner consults exists to tell "publish in
        // flight" from "absent", and with nothing prepared that question
        // does not arise.
        if let Some(cand) = self.resumable_candidate() {
            // The persisted intent is MONOTONE, not absolute: an
            // `Advance` asked for now upgrades it, and nothing
            // downgrades it.
            //
            // Absolute was wrong, and wrong in the direction §8.2
            // actually produces. The premise of the whole runbook is
            // that the OUTAGE is what fills the pool: the candidate is
            // stranded while things are still quiet — so `advancing` is
            // persisted false — and the pressure arrives afterwards.
            // Every later cycle then correctly computes
            // `CycleKind::Advance` from that pressure, and pinning the
            // intent would discard it. The heal would publish without
            // repointing, nothing would be reclaimed, and no pool blocks
            // would come back until the cycle after — a full backlog
            // upload that returns zero space, at the moment the pool is
            // fullest and (with both rule 3 dials unset by default) the
            // workload is still writing into it.
            //
            // Upgrade-only keeps the property that matters in the other
            // direction: an advance stranded by an outage stays an
            // advance even when the resuming cycle is called for a
            // publish, which is what a crash-restart would resume and
            // what the recovery path already does.
            //
            // Safe against the candidate's staleness by construction,
            // not by luck: `reclaim` unmaps only the blocks the origin
            // now serves IDENTICALLY — `FakeStack::is_shared` compares
            // content, and `DmStack::reclaim_window` recomputes the
            // shared set from thin mappings inside each freeze window.
            // Anything written after the strand differs, so it is not
            // shared and cannot be unmapped. A stale candidate therefore
            // costs reclaim YIELD, never correctness.
            if kind == CycleKind::Advance && !self.state.as_ref().is_some_and(|s| s.advancing) {
                self.persist(|s| s.advancing = true)?;
            }
            self.report("resuming", 0, 0).await;
            self.resume_from_captured(&cand).await?;
            return Ok(target);
        }

        let cand = self.candidate_name(&target);

        // Capture: persist intent (the cycle kind is durable from here so
        // a crash resumes the right promotion tail), freeze-snap-thaw,
        // persist result.
        let advancing = kind == CycleKind::Advance;
        self.persist(move |s| {
            s.phase = Phase::Capturing;
            s.advancing = advancing;
        })?;
        self.report("capturing", 0, 0).await;
        let mut snap = match self.stack.capture(&cand) {
            Ok(s) => s,
            Err(e) => {
                // V3.2 §9.3 capture failure: keep baseline, back to idle.
                self.persist(|s| s.phase = Phase::Idle)?;
                return Err(e.into());
            }
        };
        // The cut's wall-clock time (D-M12): what a CSI caller is told
        // its snapshot's point in time is, bound durably below together
        // with the requests selected before the freeze.
        snap.created_at_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or_default();
        self.stack.set_role(&cand, Some(SnapRole::Candidate))?;
        let requests = std::mem::take(&mut self.capture_requests);
        self.persist(move |s| {
            s.phase = Phase::Captured;
            s.candidate = Some(snap);
            s.candidate_requests = requests;
        })?;
        self.report("captured", 0, 0).await;

        self.resume_from_captured(&cand).await?;
        Ok(target)
    }

    /// The candidate a previous cycle captured and could not publish, if
    /// it is still there. Local state only — no control-plane call, by
    /// design; see the caller.
    fn resumable_candidate(&self) -> Option<String> {
        let s = self.state.as_ref()?;
        if s.phase != Phase::Captured || s.prepared.is_some() {
            return None;
        }
        let cand = s.candidate.as_ref()?.lv_name.clone();
        // The record is not the device. A sweep may have removed the
        // activation since, and resuming onto a name that no longer
        // resolves would trade a wasted freeze for a failed cycle — so
        // confirm against the same local observation the planner uses,
        // and fall through to a fresh capture when it is gone.
        self.stack
            .observe()
            .candidates
            .iter()
            .any(|c| c == &cand)
            .then_some(cand)
    }

    /// From an existing candidate: prepare, then backfill/publish/promote.
    async fn resume_from_captured(&mut self, cand: &str) -> Result<(), AgentError> {
        let base_ref = self
            .state
            .as_ref()
            .and_then(|s| s.baseline.as_ref())
            .map(|b| b.remote.clone());
        let snap_id = self.cp.prepare(base_ref.as_deref()).await?;
        self.crash.hit()?;
        let target = self.next_target();
        {
            let (snap_id, target) = (snap_id.clone(), target.clone());
            self.persist(move |s| {
                s.phase = Phase::Backfilling;
                s.prepared = Some(PreparedRef { snap_id, target });
                s.cursor = Some(OpCursor { next_op_id: 1 });
            })?;
        }
        self.report("backfilling", 0, 0).await;
        self.backfill_publish_promote(cand, &snap_id, 0).await
    }

    /// Deterministic op stream over the delta; idempotent above `watermark`.
    async fn backfill_publish_promote(
        &mut self,
        cand: &str,
        snap_id: &str,
        watermark: u64,
    ) -> Result<(), AgentError> {
        // Each cycle asks the server about mixed batches afresh.
        self.mixed_apply.split_this_cycle = false;
        let baseline_local = self
            .state
            .as_ref()
            .and_then(|s| s.baseline.as_ref())
            .map(|b| b.local.lv_name.clone())
            .filter(|n| !n.is_empty()) // rebaselined pair: no local member yet
            .filter(|n| {
                // A RECORDED local member whose LV is not activated is
                // the restage/reboot shape: teardown deactivated every
                // snapshot LV, and the rebuilt stack re-activates none
                // of them. The pair degrades to the rebaselined form —
                // the full-mapped delta below is correct against the
                // published base for exactly the reason the fresh-pool
                // mode's is (mapped blocks carry current content;
                // unmapped blocks agree with the base by Invariant B) —
                // and the promotion re-establishes a live local pair,
                // so the NEXT cycle is a cheap incremental again. This
                // is the planner's own "the next capture re-establishes
                // the local half", honored where the delta is computed.
                let present = self.stack.observe().baselines.iter().any(|b| b == n);
                if !present {
                    eprintln!(
                        "skynbd-agent: {}: recorded baseline LV {n} is not activated \
                         (restage/reboot); publishing a full-mapped delta against the \
                         published base and re-establishing the local pair",
                        self.volume
                    );
                }
                present
            });
        // Recompute the delta (V3.2 §15.3): snapshots are immutable, so the
        // op stream is identical on every resume.
        let delta = match &baseline_local {
            Some(b) => self.stack.thin_delta(b, cand)?,
            None => self.stack.full_mapped(cand)?,
        };

        // Now the op count is known: report it once up front, then every
        // 64 ops — cheap enough to disappear next to the uploads, fresh
        // enough that "uploading 40 GB" shows movement (D-M12).
        let total = delta.len() as u64;
        self.report("backfilling", watermark.min(total), total)
            .await;
        // Ops at or below the server's durable watermark were applied
        // before a crash; the op stream is identical on every resume.
        //
        // Batches are staged (read, hashed, ensured, uploaded) and then
        // attached, in op order. Above depth 1 the two overlap across
        // consecutive batches (issue #97 follow-up): batch k+1 stages while
        // batch k attaches, hiding a control-plane round trip per batch.
        // Attach order is untouched — k+1 attaches only after k's attach
        // has returned — and memory is still one batch's data, since an
        // attach needs only the hashes.
        //
        // An attach failure (an injected crash included) cancels k+1's
        // staging where it stands: staging is content-addressed and
        // attaches nothing, so a crash with k+1 part-ensured or
        // part-uploaded is just a resume that re-ensures it, and the
        // failure is reported without waiting for a batch of uploads. A
        // staging failure waits for k's attach, whose ops precede, and
        // surfaces after it. At depth 1 (or with overlap off) the staging
        // and attach calls follow the historical sequence, call for call;
        // only the best-effort progress hint is sent by a reporter beside
        // the backfill instead of between batches.
        let overlap = self.publish_overlap && self.publish_pipeline_depth > 1;
        let depth = self.publish_pipeline_depth;
        let batch_at = |at: usize| {
            (at < delta.len()).then(|| at..batch_end(&delta, at, depth, PIPELINE_BATCH_BYTES))
        };
        // Progress is a hint, kept off the critical path: the loop puts the
        // latest attached count on a watch channel, and a reporter running
        // beside the whole backfill sends it — one request at a time, always
        // the newest value. No attach, and not the final checkpoint, ever
        // waits for a report.
        let captures = self.capture_bindings();
        let progress = |ops_done| CycleProgress {
            phase: "backfilling",
            ops_done,
            ops_total: total,
            captures: captures.clone(),
        };
        let (progress_tx, mut progress_rx) = tokio::sync::watch::channel(None::<CycleProgress>);
        // Disjoint borrows for the whole backfill: the attach half owns the
        // crash seam and the mixed-apply state; staging and the reporter
        // only read.
        let (stack, cp, volume) = (&self.stack, &self.cp, self.volume.as_str());
        let (crash, apply) = (&mut self.crash, &mut self.mixed_apply);
        let stage_at = |r: std::ops::Range<usize>| {
            Self::stage_batch(
                stack,
                cp,
                cand,
                snap_id,
                &delta[r.clone()],
                r.start as u64 + 1,
            )
        };
        let reporter = async {
            while progress_rx.changed().await.is_ok() {
                let latest = progress_rx.borrow_and_update().clone();
                if let Some(p) = latest {
                    let _ = cp.report_progress(p).await;
                }
            }
            std::future::pending::<()>().await
        };
        let backfill = async {
            use futures_util::future::OptionFuture;
            let mut range = batch_at(watermark.min(total) as usize);
            let mut staged = OptionFuture::from(range.clone().map(stage_at)).await;
            while let Some(st) = staged.take() {
                // The one post-stage epilogue: a staging error surfaces
                // here, after the previous batch's attach; the post-upload
                // crash point follows a batch that had objects.
                let batch = st?;
                if batch.uploaded {
                    crash.hit()?;
                }
                range = range.and_then(|r| batch_at(r.end));
                let attach = Self::attach_staged(cp, apply, volume, crash, snap_id, &batch);
                let stage = range.clone().map(stage_at);
                staged = match stage {
                    Some(stage) if overlap => Some(beside(attach, stage).await?),
                    stage => {
                        attach.await?;
                        OptionFuture::from(stage).await
                    }
                };
                // Every 64 ops, once they are attached: fresh enough that
                // "uploading 40 GB" shows movement (D-M12), never ahead of
                // what has been attached. It is a hint, not the durable
                // watermark: a resume restarts from the server's last
                // checkpoint, which may lag this by up to 32 attach calls.
                let (first, last) = batch.ids();
                if let Some(at) = (first..=last).rev().find(|id| id.is_multiple_of(64)) {
                    progress_tx.send_replace(Some(progress(at)));
                }
            }
            Ok::<(), AgentError>(())
        };
        // The backfill decides; the reporter never finishes on its own. A
        // report still in flight when the backfill ends is dropped — the
        // "publishable" report below supersedes it.
        {
            tokio::pin!(backfill, reporter);
            tokio::select! {
                biased;
                r = &mut backfill => r?,
                _ = &mut reporter => unreachable!("the progress reporter never returns"),
            }
        }
        let op_id = total;

        // Durable barrier, then atomic publish.
        self.cp.checkpoint(snap_id, op_id).await?;
        self.persist(|s| s.phase = Phase::Publishable)?;
        self.report("publishable", total, total).await;
        let target = self.next_target();
        let name = target.strip_prefix("snap/").unwrap_or(&target).to_string();
        self.cp.publish(snap_id, &name, &self.publisher()).await?;
        // §3 step 2 (advance cycles): the durable intent that recovery
        // reads to resume the advance tail rather than plain promotion.
        let advancing = self.state.as_ref().map(|s| s.advancing).unwrap_or(false);
        self.persist(move |s| {
            s.phase = if advancing {
                Phase::Advancing
            } else {
                Phase::Visible
            }
        })?;
        self.report(
            if advancing { "advancing" } else { "visible" },
            total,
            total,
        )
        .await;

        self.promote(Some(cand)).await
    }

    /// Stage one batch of backfill ops, `first_op_id` being the id of
    /// `ops[0]` (issue #97): read, hash, check with ONE `ensure`, and
    /// upload the missing objects concurrently — content addressing makes
    /// those uploads order-free and idempotent. Nothing is attached.
    /// The post-upload crash point is the caller's.
    async fn stage_batch(
        stack: &L,
        cp: &C,
        cand: &str,
        snap_id: &str,
        ops: &[DeltaRange],
        first_op_id: u64,
    ) -> Result<Staged, AgentError> {
        // Read + hash: local, before any round trip. Each op yields after
        // its read so a 64 MiB batch does not hold a runtime worker for
        // the whole of it (lease renewal and reporting share the runtime).
        //
        // Each op is planned WITH its hash, so an op can never be attached
        // without (or with another op's) hash: there is no parallel array
        // to fall out of step.
        let mut planned: Vec<RangeOp> = Vec::with_capacity(ops.len());
        let mut objects: Vec<([u8; 32], Vec<u8>)> = Vec::new();
        let mut seen: std::collections::HashSet<[u8; 32]> = std::collections::HashSet::new();
        for r in ops {
            match *r {
                DeltaRange::Data { off, len } => {
                    let data = stack.read(cand, off, len)?;
                    let hash = *blake3::hash(&data).as_bytes();
                    planned.push(RangeOp::Attach {
                        off,
                        len,
                        hash,
                        obj_off: 0,
                    });
                    if seen.insert(hash) {
                        objects.push((hash, data));
                    }
                    if ops.len() > 1 {
                        tokio::task::yield_now().await;
                    }
                }
                DeltaRange::Clear { off, len } => planned.push(RangeOp::Clear { off, len }),
            }
        }
        let objects_empty = objects.is_empty();
        if !objects_empty {
            let wanted: Vec<[u8; 32]> = objects.iter().map(|(h, _)| *h).collect();
            let exists = cp.ensure(snap_id, &wanted).await?;
            // A short (or long) answer must never be zipped away: every
            // object past its end would go un-uploaded while its op still
            // attaches, publishing a reference to a missing object.
            if exists.len() != wanted.len() {
                return Err(AgentError::Cp(CpError::Other(format!(
                    "ensure answered {} results for {} hashes",
                    exists.len(),
                    wanted.len()
                ))));
            }
            use futures_util::stream::{StreamExt, TryStreamExt};
            futures_util::stream::iter(
                objects
                    .into_iter()
                    .zip(exists)
                    .filter(|(_, exists)| !exists)
                    .map(|((hash, data), _)| cp.put_object(hash, data)),
            )
            .buffer_unordered(PIPELINE_MAX_UPLOADS)
            .try_collect::<Vec<()>>()
            .await?;
        }
        Ok(Staged {
            uploaded: !objects_empty,
            planned,
            first_op_id,
        })
    }

    /// Attach one staged batch, in op order, in ONE call under the id of
    /// its LAST op: `attach` or `clear` when every op is one kind,
    /// [`ControlPlane::apply`] when they are mixed. Against a server
    /// without `apply`, a mixed batch falls back to one call per
    /// contiguous same-kind run, in op order, each under the id of the
    /// run's last op. A crash point follows every call, so a crash can
    /// land between a fallback batch's runs.
    ///
    /// That order is load-bearing: the server treats a checkpoint at op N
    /// as "every op ≤ N is durable" and a resume skips them all, so an op
    /// may never be attached ahead of an earlier one. Each call is atomic
    /// on the server, so its last id is exactly as safe a watermark as
    /// per-op calls would be.
    async fn attach_staged(
        cp: &C,
        apply: &mut MixedApplyState,
        volume: &str,
        crash: &mut CrashPoints,
        snap_id: &str,
        batch: &Staged,
    ) -> Result<(), AgentError> {
        let planned = &batch.planned;
        let (first_op_id, last) = batch.ids();
        let mixed = planned.iter().any(is_attach) && !planned.iter().all(is_attach);
        if mixed && apply.on && !apply.split_this_cycle {
            match cp.apply(snap_id, last, planned).await {
                Ok(()) => {
                    apply.warned = false;
                    return crash.hit();
                }
                // Not taken (not known to be applied): split this batch,
                // re-applying its ops in order, and the rest of the cycle's.
                Err(CpError::Unsupported(what)) => {
                    apply.split_this_cycle = true;
                    if !apply.warned {
                        apply.warned = true;
                        eprintln!(
                            "skynbd-agent: {volume}: mixed batch call not taken ({what}); \
                             attaching per same-kind run for the rest of this cycle"
                        );
                    }
                }
                Err(e) => return Err(e.into()),
            }
        }
        // One call per contiguous same-kind run, in op order (a single
        // run when the batch is one kind). Every op of a run goes into its
        // call: a run missing an op would still be checkpointed under
        // `run_last`, losing that extent silently.
        let mut op = first_op_id;
        for run in planned.chunk_by(|a, b| is_attach(a) == is_attach(b)) {
            let run_last = op + run.len() as u64 - 1;
            if is_attach(&run[0]) {
                let ranges: Vec<(u64, u64, [u8; 32], u64)> = run
                    .iter()
                    .map(|p| match *p {
                        RangeOp::Attach {
                            off,
                            len,
                            hash,
                            obj_off,
                        } => (off, len, hash, obj_off),
                        RangeOp::Clear { .. } => unreachable!("chunk_by groups same-kind ops"),
                    })
                    .collect();
                cp.attach(snap_id, run_last, &ranges).await?;
            } else {
                let ranges: Vec<(u64, u64)> = run
                    .iter()
                    .map(|p| match *p {
                        RangeOp::Clear { off, len } => (off, len),
                        RangeOp::Attach { .. } => unreachable!("chunk_by groups same-kind ops"),
                    })
                    .collect();
                cp.clear(snap_id, run_last, &ranges).await?;
            }
            crash.hit()?;
            op = run_last + 1;
        }
        debug_assert_eq!(op, last + 1);
        Ok(())
    }

    /// §15.4 promotion order, exactly.
    async fn promote(&mut self, cand: Option<&str>) -> Result<(), AgentError> {
        let Some(prepared) = self.state.as_ref().and_then(|s| s.prepared.clone()) else {
            return Err(AgentError::Quarantine("promote without prepared record"));
        };
        let target = prepared.target.clone();

        // 1. Verify G_n visible and pinned (publish implies pin).
        if !self.cp.ref_exists(&target).await? {
            return Err(AgentError::Quarantine("promote: target not visible"));
        }
        let Some(cand) = cand else {
            // Row 8: candidate lost after publish — rebaseline instead.
            // (For an advance this abandons the §3 tail, which the crash
            // table blesses: the pointer names a pinned generation either
            // way, and a later advance cycle retries.)
            return self.rebaseline(&target).await;
        };
        self.verify_sampled(cand, &target).await?;

        if self.state.as_ref().map(|s| s.advancing).unwrap_or(false) {
            return self.promote_advance(cand, &target).await;
        }

        // 2. Persist mapping + phase (S_n ↔ G_n).
        self.persist(|s| s.phase = Phase::Promoting)?;
        self.report("promoting", 0, 0).await;

        // 3. Mark S_n as baseline; transfer the pin to the baseline holder.
        //    Tag order: untag old B_k first so observation never shows two
        //    baselines (the old LV itself is deleted last, per §15.4 — a
        //    crash in between reruns this idempotent block).
        let holder = self.baseline_holder();
        let old = self.state.as_ref().and_then(|s| s.baseline.clone());
        self.cp.pin(&target, &holder).await?;
        self.crash.hit()?;
        if let Some(old) = &old {
            if !old.local.lv_name.is_empty() && old.local.lv_name != cand {
                self.stack.set_role(&old.local.lv_name, None).ok();
            }
        }
        self.stack.set_role(cand, Some(SnapRole::Baseline))?;
        self.crash.hit()?;
        self.unpin_best_effort(&target, &self.publisher()).await?;

        // 4. Unpin G_k for this holder; 5. delete old B_k.
        if let Some(old) = &old {
            if old.remote != target {
                self.unpin_best_effort(&old.remote, &old.pin_holder).await?;
            }
            if !old.local.lv_name.is_empty() && old.local.lv_name != cand {
                self.stack.snap_delete(&old.local.lv_name).ok();
            }
        }
        self.crash.hit()?;

        let new_local = self
            .state
            .as_ref()
            .and_then(|s| s.candidate.clone())
            .filter(|c| c.lv_name == cand);
        let (target2, holder2) = (target.clone(), holder.clone());
        self.persist(move |s| {
            s.phase = Phase::Idle;
            s.baseline = Some(BaselinePair {
                local: new_local.unwrap_or_else(|| crate::state::LocalSnap {
                    lv_name: cand.to_string(),
                    thin_dev_id: 0,
                    pool_tx_id: 0,
                    created_at_ms: 0,
                }),
                remote: target2,
                pin_holder: holder2,
            });
            s.candidate = None;
            s.prepared = None;
            s.cursor = None;
        })?;
        Ok(())
    }

    /// The §3 advance tail (D-M15), folded into promote: pin gₜ → repoint
    /// → unpin gₒ → fence → reclaim → delete S → rebaselined pair. Every
    /// step is idempotent, so recovery re-runs the whole tail from any
    /// crash inside it (the §3 crash table).
    async fn promote_advance(&mut self, cand: &str, target: &str) -> Result<(), AgentError> {
        self.persist(|s| s.phase = Phase::Advancing)?;
        self.report("advancing", 0, 0).await;
        let holder = self.baseline_holder();
        let old = self.state.as_ref().and_then(|s| s.baseline.clone());

        // §3 step 3: pin gₜ for the baseline holder (before the repoint,
        // so the pointer never names an unpinned generation).
        self.cp.pin(target, &holder).await?;
        self.crash.hit()?;

        // §3 step 4: server-side repoint — row + live export swap.
        self.cp.repoint_origin(target).await?;
        self.crash.hit()?;

        // §3 step 5: release the publish pin and the old baseline's.
        self.unpin_best_effort(target, &self.publisher()).await?;
        if let Some(old) = &old {
            if old.remote != target {
                self.unpin_best_effort(&old.remote, &old.pin_holder).await?;
            }
        }
        self.crash.hit()?;

        // §3 step 6: the Invariant E fence — the export must be SERVING
        // gₜ or a descendant (end-to-end read, not the refs row) before
        // one block is discarded. Failure = a stalled advance, never a
        // discard against a stale origin.
        let served = self.cp.origin_served().await?;
        let ok = served
            .as_deref()
            .and_then(gen_number)
            .zip(gen_number(target))
            .is_some_and(|(s, t)| s >= t);
        if !ok {
            return Err(AgentError::Quarantine(
                "advance fence: origin export is not serving the advance target",
            ));
        }
        // §7: both executors satisfy one contract, and the choice is one
        // comparison over data already in hand, made per advance after
        // the enumeration — never durable (a crashed swing re-decides).
        let sets = self.stack.enumerate(cand)?;
        let aged = self
            .state
            .as_ref()
            .map(|s| s.advances_since_swing)
            .unwrap_or(0);
        let choice = decide(
            sets.shared_count(),
            sets.dirty_bytes(),
            self.stack.headroom(),
            self.stack.swing_supported(),
            self.degraded,
            aged,
            &self.policy,
        );

        // §7.1 lazy timing: the scheduled advance ran the §3 head above
        // (pin, repoint, unpin, fence) but skips reclaim and delete-S —
        // the published set stays pooled as read cache, evicted only
        // under watermark pressure. A swing chosen under lazy is a major
        // compaction and runs normally (honest cost: the cache resets).
        if self.policy.timing == ReclaimTiming::Lazy && choice != ExecutorChoice::Swing {
            return self.finish_advance_lazy(cand, target, &holder);
        }

        let swung = match choice {
            ExecutorChoice::NoOp => false,
            ExecutorChoice::Port => {
                // Keyed on bytes, matching the eviction path: §8.2
                // step 6 is about space ACTUALLY returned, and one
                // predicate written twice is one predicate too many.
                let freed = self.stack.reclaim(cand)?;
                self.freed_this_cycle |= freed > 0;
                false
            }
            ExecutorChoice::Swing => {
                // A completed swing rebuilds the pool from the live set
                // by construction, so there is no byte count to gate
                // on. There IS a third outcome — `Ok(false)`, the
                // pre-flip abort that sweeps slot B and falls back to
                // the port inside `run_swing` — where a reclaim did run
                // and this flag stays unset. Inert today (§8.2 step 5
                // forces a degraded volume to Port, so it never reaches
                // this arm), and the conservative direction anyway: it
                // leaves the volume read-only for one more cycle.
                let done = self.run_swing(cand)?;
                self.freed_this_cycle |= done;
                done
            }
        };
        if swung {
            // The swing's retarget recomputed the shim queue's limits;
            // the executor re-asserts inside its own window and only
            // warns on failure, so confirm from here rather than wait a
            // whole cadence interval to find out (Invariant B).
            self.reassert_mask()?;
        }
        self.crash.hit()?;

        // §3 step 7: delete S (and the old baseline LV) and persist the
        // rebaselined pair — the memoryless post-advance shape
        // (Invariant C): identical to a fresh restage.
        if let Some(old) = &old {
            if !old.local.lv_name.is_empty() && old.local.lv_name != cand {
                self.stack.set_role(&old.local.lv_name, None).ok();
                self.stack.snap_delete(&old.local.lv_name).ok();
            }
        }
        self.stack.snap_delete(cand).ok();
        // A swing recovery already completed counts as this advance's
        // major compaction, whatever the re-run's own executor chose.
        let swung = swung || std::mem::take(&mut self.recovered_swing);
        let (target2, holder2) = (target.to_string(), holder.clone());
        self.persist(move |s| {
            s.phase = Phase::Idle;
            s.advancing = false;
            s.baseline = Some(BaselinePair {
                local: crate::state::LocalSnap {
                    lv_name: String::new(), // rebaselined: no local member
                    thin_dev_id: 0,
                    pool_tx_id: 0,
                    created_at_ms: 0,
                },
                remote: target2,
                pin_holder: holder2,
            });
            s.candidate = None;
            s.prepared = None;
            s.cursor = None;
            // The aging counter feeds the §7 forced-major cadence; only
            // a completed major compaction resets it.
            s.advances_since_swing = if swung { 0 } else { aged.saturating_add(1) };
            s.swing_flipped = None;
        })?;
        Ok(())
    }

    /// The swing executor with its durable slot bracket (§7 crash
    /// windows): persist `armed` (recovery: B is garbage) → run, with
    /// the flip record written by the executor INSIDE its freeze window
    /// through the store directly → on pre-flip failure, sweep B and
    /// fall back to the port in the same cycle; on post-flip failure,
    /// propagate (the flip is durable — recovery sweeps A and finishes).
    /// Returns whether the swing completed.
    fn run_swing(&mut self, cand: &str) -> Result<bool, AgentError> {
        self.persist(|s| s.swing_flipped = Some(false))?;
        let mut flip_state = self.state.clone().expect("state persisted above");
        flip_state.swing_flipped = Some(true);
        self.tick += 1;
        flip_state.updated_at_ms = self.tick;
        let flipped = AtomicBool::new(false);
        let store = &self.store;
        let (record, flag) = (flip_state.clone(), &flipped);
        let mut persist_flip = move || -> Result<(), StackError> {
            store
                .save(&record)
                .map_err(|_| StackError::Fault("slot flip persist failed"))?;
            flag.store(true, Ordering::SeqCst);
            Ok(())
        };
        let mut retargeted = false;
        let mut hooks = SwingHooks {
            persist_flip: &mut persist_flip,
            retargeted: &mut retargeted,
        };
        let result = self.stack.swing(cand, &mut hooks);
        match result {
            Ok(_stats) => {
                self.state = Some(flip_state); // reconcile with the store
                Ok(true)
            }
            Err(e) if flipped.load(Ordering::SeqCst) => {
                self.state = Some(flip_state);
                Err(e.into())
            }
            Err(e) if retargeted => {
                // The shim is on B with the record still naming A, and
                // the executor could not put it back. Neither a port
                // fallback (its discards would land on B) nor a sweep
                // from here is safe blind — leave the record armed and
                // stall: recovery's armed arm repairs the tables from
                // the record, thaws, and re-decides.
                Err(e.into())
            }
            Err(e) => {
                // Nothing irreversible happened (the flip gates it):
                // sweep B, record steady, run the port instead. The flag
                // clears only if the sweep actually succeeded — a
                // half-swept B left under a steady record is a slot no
                // recovery path would ever revisit, silently pinning
                // every future swing at its pre-clean.
                eprintln!("skynbd-agent: swing failed before flip ({e}); falling back to port");
                self.stack.sweep_slot(false)?;
                self.persist(|s| s.swing_flipped = None)?;
                self.stack.reclaim(cand)?;
                Ok(false)
            }
        }
    }

    /// §7.1 lazy advance tail: retain S as the local pair member — the
    /// ESTABLISHED pair, a shape the engine already rotates
    /// (`thin_delta(S, S′)` next cycle; the normal rotation deletes S at
    /// the next promote). The old baseline LV still goes, exactly as in
    /// the eager step 7. No reclaim ran, so Invariant C's "no delta
    /// spans a reclaim" holds trivially here; eviction keeps it holding
    /// by dual-discarding.
    fn finish_advance_lazy(
        &mut self,
        cand: &str,
        target: &str,
        holder: &str,
    ) -> Result<(), AgentError> {
        let old = self.state.as_ref().and_then(|s| s.baseline.clone());
        if let Some(old) = &old {
            if !old.local.lv_name.is_empty() && old.local.lv_name != cand {
                self.stack.set_role(&old.local.lv_name, None).ok();
                self.stack.snap_delete(&old.local.lv_name).ok();
            }
        }
        self.stack.set_role(cand, Some(SnapRole::Baseline))?;
        self.crash.hit()?;
        let swung = std::mem::take(&mut self.recovered_swing);
        let new_local = self
            .state
            .as_ref()
            .and_then(|s| s.candidate.clone())
            .filter(|c| c.lv_name == cand);
        let cand_name = cand.to_string();
        let (target2, holder2) = (target.to_string(), holder.to_string());
        self.persist(move |s| {
            s.phase = Phase::Idle;
            s.advancing = false;
            s.baseline = Some(BaselinePair {
                local: new_local.unwrap_or(crate::state::LocalSnap {
                    lv_name: cand_name,
                    thin_dev_id: 0,
                    pool_tx_id: 0,
                    created_at_ms: 0,
                }),
                remote: target2,
                pin_holder: holder2,
            });
            s.candidate = None;
            s.prepared = None;
            s.cursor = None;
            s.advances_since_swing = if swung {
                0
            } else {
                s.advances_since_swing.saturating_add(1)
            };
            s.swing_flipped = None;
        })?;
        Ok(())
    }

    /// Deterministic sampled verify (no clock, no rng): first, middle,
    /// and last chunk of the candidate's MAPPED set vs the published
    /// export. Probing fixed offsets was wrong the moment origins read
    /// through (D-M11/D-M15): an unmapped candidate chunk reads zero
    /// locally while the published generation serves base content there
    /// — base fidelity is the prepare-from-base transaction's guarantee,
    /// not this sample's. A candidate with nothing mapped verifies
    /// vacuously (the whole generation is the base).
    async fn verify_sampled(&mut self, cand: &str, target: &str) -> Result<(), AgentError> {
        let chunk = self.stack.chunk_size();
        let mapped = self.stack.full_mapped(cand)?;
        let spans: Vec<(u64, u64)> = mapped
            .iter()
            .filter_map(|r| match *r {
                DeltaRange::Data { off, len } => Some((off, len)),
                DeltaRange::Clear { .. } => None,
            })
            .collect();
        if spans.is_empty() {
            return Ok(());
        }
        let (first, _) = spans[0];
        let (moff, mlen) = spans[spans.len() / 2];
        let (loff, llen) = spans[spans.len() - 1];
        let probes = [
            first,
            (moff + (mlen / chunk / 2) * chunk).min(moff + mlen - chunk),
            loff + llen - chunk,
        ];
        for off in probes {
            let local = self.stack.read(cand, off, chunk)?;
            let remote = self.cp.read_snapshot(target, off, chunk).await?;
            if local != remote {
                return Err(AgentError::VerifyMismatch(off));
            }
        }
        Ok(())
    }

    /// Matrix row 8 / DR: re-establish the pair against `remote_base`
    /// without re-upload of shared content (CAS dedupe does the work).
    async fn rebaseline(&mut self, remote_base: &str) -> Result<(), AgentError> {
        let remote_base = if remote_base.is_empty() {
            // State was lost entirely: adopt the highest published gen.
            let mut n = 0u64;
            while self.cp.ref_exists(&format!("snap/g{n}")).await? {
                n += 1;
            }
            if n == 0 {
                return Err(AgentError::Quarantine("rebaseline with no published gen"));
            }
            format!("snap/g{}", n - 1)
        } else {
            remote_base.to_string()
        };
        // Hold the base while we rebuild.
        let holder = self.baseline_holder();
        self.cp.pin(&remote_base, &holder).await?;
        // Drop any stale local baseline record; the local pair member is
        // re-established by the next capture (published gens stay valid).
        let rb = remote_base.clone();
        self.persist(move |s| {
            s.phase = Phase::Idle;
            s.advancing = false;
            s.baseline = Some(BaselinePair {
                local: crate::state::LocalSnap {
                    lv_name: String::new(), // no local member yet
                    thin_dev_id: 0,
                    pool_tx_id: 0,
                    created_at_ms: 0,
                },
                remote: rb,
                pin_holder: holder,
            });
            s.candidate = None;
            s.prepared = None;
            s.cursor = None;
        })?;
        // Local baseline LV is gone ⇒ next cycle must full-objectize: clear
        // any stale baseline tags so run_cycle sees no local baseline.
        for b in self.stack.observe().baselines {
            self.stack.set_role(&b, None).ok();
        }
        Ok(())
    }

    async fn finish_abort(
        &mut self,
        delete_candidate: bool,
        candidates: &[String],
    ) -> Result<(), AgentError> {
        if let Some(info) = self.cp.find_prepared().await? {
            self.cp.abort(&info.snap_id).await?;
        }
        self.crash.hit()?;
        if delete_candidate {
            for c in candidates {
                self.stack.snap_delete(c).ok();
            }
        }
        self.persist(|s| {
            s.phase = Phase::Idle;
            s.advancing = false;
            s.candidate = None;
            s.prepared = None;
            s.cursor = None;
        })?;
        Ok(())
    }

    /// Policy abort of the in-flight cycle (pool pressure, operator).
    pub async fn abort_in_flight(&mut self) -> Result<(), AgentError> {
        self.persist(|s| s.phase = Phase::Aborting)?;
        let candidates = self.stack.observe().candidates;
        let r = self.finish_abort(true, &candidates).await;
        self.seal(r)
    }
}

/// D-M12 request bindings follow their candidate through every state
/// transition, so no individual transition has to remember them. When a
/// write replaces or clears a candidate that had requests bound:
///
/// * the candidate's prepared target is now the baseline — it was
///   published, so its requests are owed an alias to exactly that
///   generation (`served_requests`, with the original cut time);
/// * anything else discarded the cut (abort, recovery normalization, a
///   swept activation re-captured under the same deterministic name), so
///   the requests are `lost` — never re-captured, since a caller may
///   already hold the discarded cut's time.
fn settle_request_bindings(
    s: &mut AgentVolumeState,
    (old_candidate, old_target, old_requests): (Option<LocalSnap>, Option<String>, Vec<String>),
) {
    if old_requests.is_empty() || s.candidate == old_candidate {
        return;
    }
    if s.candidate_requests == old_requests {
        s.candidate_requests.clear();
    }
    let published = old_target
        .as_ref()
        .filter(|t| s.baseline.as_ref().is_some_and(|b| &b.remote == *t));
    match (published, old_candidate) {
        (Some(target), Some(candidate)) => {
            s.served_requests
                .extend(old_requests.into_iter().map(|name| ServedRequest {
                    name,
                    target: target.clone(),
                    capture_time_ms: candidate.created_at_ms,
                }));
        }
        _ => push_lost(&mut s.lost_requests, old_requests),
    }
}

/// Lost-request tombstones kept per volume. CSI snapshot names are unique
/// per `VolumeSnapshot` (`snapshot-<uid>`), so a tombstone only ever has to
/// refuse retries of its own request; the bound keeps the durable record
/// small, evicting the oldest loss first.
pub const LOST_REQUESTS_RETAINED: usize = 1024;

fn push_lost(lost: &mut Vec<String>, names: Vec<String>) {
    for name in names {
        if !lost.contains(&name) {
            lost.push(name);
        }
    }
    let excess = lost.len().saturating_sub(LOST_REQUESTS_RETAINED);
    lost.drain(..excess);
}

/// Byte budget of one backfill upload batch (issue #97): a batch closes
/// at `publish_pipeline_depth` ops or before an op that would take its
/// data past this, so a deep pipeline over large coalesced ranges cannot
/// hold depth × range bytes in memory. A single op larger than this still
/// uploads alone, exactly as it did unbatched.
pub const PIPELINE_BATCH_BYTES: u64 = 64 << 20;

/// How mixed attach/clear batches are sent ([`CycleEngine::with_mixed_apply`]).
/// A plane that answers [`CpError::Unsupported`] gets the batch split into
/// same-kind runs, and is not asked again until the next cycle: an
/// upgraded server (or a rollback undone) is used from the next cycle on,
/// and a refusal that costs a round trip each time (a proxy answering for
/// the path) costs one per cycle, not one per batch.
struct MixedApplyState {
    /// Send mixed batches as one `apply` (default true).
    on: bool,
    /// This cycle's backfill has had a mixed batch refused: split the
    /// rest without asking. Reset when a backfill starts.
    split_this_cycle: bool,
    /// The current switch to splitting has been logged. Cleared when an
    /// `apply` succeeds, so each later switch is logged again (a rollback
    /// after an upgrade is not hidden by a warning spent long ago).
    warned: bool,
}

/// Run `primary` beside `secondary`. A `primary` failure is returned at
/// once and drops `secondary` mid-flight (the backfill passes only
/// cancel-safe work there: staging, which is content-addressed and
/// attaches nothing, or a best-effort report); otherwise `secondary` is
/// awaited to completion, and a failure of its own is its output, for
/// the caller to surface after `primary`'s.
async fn beside<T, E>(
    primary: impl std::future::Future<Output = Result<(), E>>,
    secondary: impl std::future::Future<Output = T>,
) -> Result<T, E> {
    tokio::pin!(primary, secondary);
    let mut done = None;
    loop {
        tokio::select! {
            biased;
            r = &mut primary => {
                r?;
                return Ok(match done {
                    Some(t) => t,
                    None => secondary.await,
                });
            }
            t = &mut secondary, if done.is_none() => done = Some(t),
        }
    }
}

/// A backfill batch read, hashed and uploaded, waiting to be attached.
struct Staged {
    /// Its ops, each data op carrying its object's hash, in op order.
    planned: Vec<RangeOp>,
    /// The id of `planned[0]`.
    first_op_id: u64,
    /// Whether it had any objects (the historical crash point after the
    /// uploads exists only then).
    uploaded: bool,
}

impl Staged {
    /// `(first, last)` op ids.
    fn ids(&self) -> (u64, u64) {
        (
            self.first_op_id,
            self.first_op_id + self.planned.len() as u64 - 1,
        )
    }
}

fn is_attach(op: &RangeOp) -> bool {
    matches!(op, RangeOp::Attach { .. })
}

/// Most object uploads one batch keeps in flight, whatever its depth: a
/// deep batch of small ops must not open thousands of simultaneous PUTs.
pub const PIPELINE_MAX_UPLOADS: usize = 64;

/// End (exclusive) of the backfill batch starting at `next`: at most
/// `depth` ops (0 counts as 1), and no op admitted that would take the
/// batch's data past `budget` — except the first, which always fits.
fn batch_end(delta: &[DeltaRange], next: usize, depth: usize, budget: u64) -> usize {
    let mut end = next;
    let mut bytes = 0u64;
    while end < delta.len() && end - next < depth.max(1) {
        let len = match delta[end] {
            DeltaRange::Data { len, .. } => len,
            DeltaRange::Clear { .. } => 0,
        };
        if end > next && bytes + len > budget {
            break;
        }
        bytes += len;
        end += 1;
    }
    end
}

/// `snap/g<N>` → N — the total order the Invariant E fence compares in.
fn gen_number(ref_name: &str) -> Option<u64> {
    let digits = ref_name.strip_prefix("snap/g")?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use crate::stack::DeltaRange;

    /// The byte budget is checked BEFORE admitting an op: a batch never
    /// grows past it except for its first op, which uploads alone.
    #[test]
    fn batch_end_respects_depth_and_the_byte_budget() {
        let data = |len| DeltaRange::Data { off: 0, len };
        let clear = DeltaRange::Clear {
            off: 0,
            len: 1 << 30,
        };
        let delta = [data(48), data(48), data(10), clear, data(6), data(100)];
        // Budget 64: 48 admits; 48+48 would exceed it.
        assert_eq!(super::batch_end(&delta, 0, 16, 64), 1);
        // 48, then 10 + clear (no data bytes) + 6 = 64 exactly.
        assert_eq!(super::batch_end(&delta, 1, 16, 64), 5);
        // An op larger than the budget still forms a batch of one.
        assert_eq!(super::batch_end(&delta, 5, 16, 64), 6);
        // Depth caps the batch first; 0 counts as 1.
        assert_eq!(super::batch_end(&delta, 1, 2, 1 << 40), 3);
        assert_eq!(super::batch_end(&delta, 1, 0, 1 << 40), 2);
    }

    #[test]
    fn lost_tombstones_are_deduplicated_and_bounded() {
        let mut lost = vec!["a".to_string()];
        super::push_lost(&mut lost, vec!["a".into(), "b".into()]);
        assert_eq!(lost, ["a", "b"]);
        let many = (0..super::LOST_REQUESTS_RETAINED).map(|i| format!("n{i}"));
        super::push_lost(&mut lost, many.collect());
        assert_eq!(lost.len(), super::LOST_REQUESTS_RETAINED);
        assert_eq!(lost[0], "n0", "the oldest tombstones go first");
        assert!(!lost.contains(&"a".to_string()));
    }

    use super::*;

    /// §8.1 rule 1's rate is observed, and only growth is a rate: the
    /// pool giving space back (a reclaim, an eviction) is not headroom
    /// the workload will keep producing, and must not read as one.
    #[test]
    fn write_rate_from_two_samples() {
        let mib = 1 << 20;
        assert_eq!(
            derive_write_rate(0, 10 * mib, Duration::from_secs(2)),
            Some(5.0 * mib as f64)
        );
        // Half-second windows are the common case at short cadences.
        assert_eq!(
            derive_write_rate(mib, 3 * mib, Duration::from_millis(500)),
            Some(4.0 * mib as f64)
        );
        // A shrinking pool: zero, never negative.
        assert_eq!(
            derive_write_rate(10 * mib, mib, Duration::from_secs(1)),
            Some(0.0)
        );
        // Two samples at the same instant measure nothing.
        assert_eq!(derive_write_rate(0, mib, Duration::ZERO), None);
    }

    /// §8.1 rule 1's window is observed too, and the observation is a
    /// DECAYED MAXIMUM: a slow advance raises the reserve immediately,
    /// and no single fast advance can collapse it — which is the whole
    /// point on a degraded link, where the publish that dominates the
    /// window is exactly the thing that comes and goes.
    #[test]
    fn observed_window_is_a_decayed_maximum() {
        let s = Duration::from_secs(1);
        // A worse sample takes over immediately.
        assert_eq!(decay_observed(10 * s, 90 * s), 90 * s);
        // A better one does not: the previous worst only ages.
        assert_eq!(decay_observed(100 * s, s), 90 * s);
        // And it keeps ageing, geometrically, over a run of fast
        // advances — a minutes-long publish is forgotten in ~10 cycles,
        // not in one.
        let mut w = 100 * s;
        for _ in 0..10 {
            w = decay_observed(w, Duration::ZERO);
        }
        assert!(w < 36 * s && w > 34 * s, "{w:?}");
        // From nothing, the first sample is the observation.
        assert_eq!(decay_observed(Duration::ZERO, 7 * s), 7 * s);
    }

    /// The same shape carries the rate, where it closes the blind spot
    /// sampling-before-eviction only moved: the cycle AFTER an eviction
    /// measures its growth against a pre-eviction baseline, so its own
    /// sample pair reads zero however fast the writer is running.
    #[test]
    fn the_write_rate_decays_the_same_way() {
        let mib = (1 << 20) as f64;
        // A faster writer takes over immediately.
        assert_eq!(decay_observed(mib, 8.0 * mib), 8.0 * mib);
        // A sample flattened by the previous cycle's eviction does not
        // erase what the workload proved it can produce.
        assert_eq!(decay_observed(10.0 * mib, 0.0), 9.0 * mib);
        // Sustained quiet still ages it out geometrically.
        let mut r = 100.0 * mib;
        for _ in 0..10 {
            r = decay_observed(r, 0.0);
        }
        assert!(r < 36.0 * mib && r > 34.0 * mib, "{r}");
    }

    /// The observed window is bounded. It decays per ADVANCE, so on a
    /// volume that advances rarely one pathological publish would hold
    /// the margin at its span for the life of the process; the ceiling
    /// is a multiple of the floor, and a floor below the default does
    /// not shrink it (a deployment configuring ~0 wants the
    /// measurement to lead, not to be switched off).
    #[test]
    fn the_observed_window_is_capped_at_a_multiple_of_the_floor() {
        let s = Duration::from_secs(1);
        // Nothing observed: the floor stands alone.
        assert_eq!(bounded_window(30 * s, Duration::ZERO), 30 * s);
        // Observed above the floor and under the ceiling: it leads.
        assert_eq!(bounded_window(30 * s, 120 * s), 120 * s);
        // Observed above the ceiling: clamped to 10× the floor.
        assert_eq!(bounded_window(30 * s, 3 * 3600 * s), 300 * s);
        // A generous floor raises its own ceiling with it.
        assert_eq!(bounded_window(600 * s, 3 * 3600 * s), 6000 * s);
        // A zero floor keeps the default's ceiling rather than pinning
        // the observation at zero.
        assert_eq!(bounded_window(Duration::ZERO, 120 * s), 120 * s);
        assert_eq!(bounded_window(Duration::ZERO, 3 * 3600 * s), 300 * s);
    }
}

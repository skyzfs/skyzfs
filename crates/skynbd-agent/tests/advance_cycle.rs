//! D-M15 task 15c: the two cycle kinds and the §3 advance tail, engine
//! level (FakeStack + the in-process control plane — real server logic).
//!
//! Claims under test:
//! - an advance cycle ends in the rebaselined pair (Invariant C: the
//!   memoryless post-advance shape), with the pointer row moved, the
//!   `origin` and baseline pins on the target, and the pool reclaimed;
//! - the reclaim changes no observable byte (Invariant B at this level);
//! - the next cycle publishes bit-exact through the `full_mapped` path;
//! - the watermark backstop promotes a plain `run_cycle`;
//! - a failed Invariant E fence stalls the advance without discarding,
//!   and recovery retries the tail to completion;
//! - the publish path writes no emap objects (§4 cost case 1).

use skynbd_agent::{
    ControlPlane, CpError, CycleEngine, CycleKind, CycleProgress, FailPoint, FakeStack,
    InprocControlPlane, LocalStack, Phase, PreparedInfo, ReclaimPolicy, ReclaimStrategy,
    ReclaimTiming, StateStore,
};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;

type Cp = InprocControlPlane<MemoryStore, MemoryRefStore>;

struct Rig {
    stack: FakeStack,
    cp: Cp,
    store: MemoryStore,
    refs: MemoryRefStore,
    vol: VolumeId,
    dir: tempfile::TempDir,
}

impl Rig {
    async fn new() -> Self {
        let stack = FakeStack::new(VOL, CHUNK);
        let store = MemoryStore::new();
        let refs = MemoryRefStore::new();
        let vol = VolumeId(ulid::Ulid::new());
        let cp = InprocControlPlane::create_volume(store.clone(), refs.clone(), vol, VOL)
            .await
            .unwrap();
        Rig {
            stack,
            cp,
            store,
            refs,
            vol,
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn engine(&self) -> CycleEngine<FakeStack, Cp> {
        CycleEngine::open(
            self.stack.clone(),
            self.cp.clone(),
            StateStore::new(self.dir.path()),
            "vlin",
        )
        .unwrap()
    }

    fn pat(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
    }

    async fn origin_row(&self) -> Option<String> {
        self.refs
            .meta_get(self.vol, "origin")
            .await
            .unwrap()
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    }
}

/// The full advance arc: publish g0, advance to g1, then a publish cycle
/// proves the post-advance state is memoryless.
#[tokio::test]
async fn advance_cycle_ends_rebaselined_and_preserves_content() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();

    // Bootstrap publish cycle → g0, established pair.
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    let s = eng.state().unwrap();
    assert!(!s.baseline.as_ref().unwrap().local.lv_name.is_empty());

    // Advance cycle → g1: writes since g0 publish + reclaim + repoint.
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let before = rig.stack.origin_image();
    assert_eq!(
        eng.run_cycle_as(CycleKind::Advance).await.unwrap(),
        "snap/g1"
    );

    // Invariant C: the memoryless rebaselined pair, no local snapshots.
    let s = eng.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert!(!s.advancing);
    let b = s.baseline.as_ref().unwrap();
    assert_eq!(b.remote, "snap/g1");
    assert!(b.local.lv_name.is_empty(), "rebaselined: no local member");
    assert_eq!(rig.stack.snap_count(), 0, "S and old baseline deleted");

    // Invariant D/E surface: pointer row moved, pinned by `origin` and
    // the baseline holder.
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g1"));
    let pins = rig.refs.pins(rig.vol, "snap/g1").await.unwrap();
    assert!(pins.iter().any(|h| h == "origin"), "{pins:?}");
    assert!(pins.iter().any(|h| h == "agent-baseline:vlin"), "{pins:?}");

    // Invariant B: the reclaim moved not one observable byte, and the
    // pool actually shrank (everything published was unmapped).
    assert_eq!(rig.stack.origin_image(), before);
    let (used, _) = rig.stack.pool_usage().unwrap();
    assert_eq!(used, 0, "advance reclaims the whole published set");

    // Memoryless: the next publish cycle full-maps only fresh writes and
    // its generation is bit-exact against the origin.
    rig.stack
        .write_origin(3 * CHUNK, &Rig::pat(CHUNK as usize, 33));
    let img = rig.stack.origin_image();
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g2");
    assert_eq!(rig.cp.read_snapshot("snap/g2", 0, VOL).await.unwrap(), img);
}

/// §4 watermark backstop: a plain `run_cycle` above the watermark runs
/// as an advance.
#[tokio::test]
async fn watermark_promotes_a_publish_cycle() {
    let rig = Rig::new().await;
    let mut eng = rig.engine().with_advance_watermark(0.05);
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1)); // 2/16 mapped
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    let b = eng.state().unwrap().baseline.clone().unwrap();
    assert!(
        b.local.lv_name.is_empty(),
        "watermark-promoted cycle must end rebaselined"
    );
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g0"));
}

/// §8.1 rule 1: the watermark's OTHER term. Occupancy stays far below
/// `advancePoolWatermark`, but the observed write rate over the advance
/// window would eat every free byte before the rescue's capture,
/// publish and repoint could finish — so the cycle promotes anyway.
/// Firing this late is how the rescue advance ends up wedged inside its
/// own fsfreeze, flushing dirty data into queued no-space writes.
#[tokio::test]
async fn the_margin_term_promotes_a_cycle_occupancy_would_not() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_advance_watermark(0.9)
        // Zero reserve: whatever promotes here is the rate × window
        // term alone, not the flat dirty-flush allowance.
        .with_dirty_flush_reserve(0)
        // An hour-long window, deliberately: the rate is bytes-grown
        // over WALL time between two in-process cycles, so with the
        // 30 s default this assertion would silently become a bet on
        // the machine finishing the first cycle inside ~2 s. At 3600 s
        // the same 4 KiB of growth still exceeds the pool's 56 KiB of
        // free space unless the gap exceeds four minutes — a range no
        // CI host can miss, and the promotion is still the rate term.
        .with_advance_window(std::time::Duration::from_secs(3600));

    // First cycle: one sample, so there is no rate yet and no margin
    // term at all — a publish cycle, keeping its local pair member.
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    assert!(
        !eng.state()
            .unwrap()
            .baseline
            .as_ref()
            .unwrap()
            .local
            .lv_name
            .is_empty(),
        "the first cycle has no history to derive a rate from"
    );

    // Second cycle: the pool grew between the samples, so a rate exists.
    // Occupancy is 2/16 — nowhere near 0.9 — but the window's worth of
    // that rate is far more than the pool's remaining 56 KiB.
    rig.stack.write_origin(CHUNK, &Rig::pat(CHUNK as usize, 2));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert!(
        eng.observed_write_rate().is_some_and(|r| r > 0.0),
        "the promotion is the rate term: {:?}",
        eng.observed_write_rate()
    );
    let s = eng.state().unwrap();
    assert!(
        s.baseline.as_ref().unwrap().local.lv_name.is_empty(),
        "margin-promoted cycle must end rebaselined, exactly like an occupancy-promoted one"
    );
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g1"));
    assert_eq!(
        rig.stack.pool_usage().unwrap().0,
        0,
        "the promotion is a real advance: the published set is reclaimed"
    );
}

/// The first cycle has one sample and therefore no observed rate, and
/// its margin is ZERO — not the bare reserve. A freshly staged pool
/// knows nothing about its workload, and promoting on the reserve alone
/// would advance every nearly-full pool on no evidence at all.
#[tokio::test]
async fn the_first_cycle_carries_no_margin_term() {
    let rig = Rig::new().await;
    // A reserve larger than the whole pool: if the first cycle applied
    // it, every first cycle everywhere would be an advance.
    let mut eng = rig
        .engine()
        .with_advance_watermark(0.9)
        .with_dirty_flush_reserve(4 * VOL);
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    assert!(
        !eng.state()
            .unwrap()
            .baseline
            .as_ref()
            .unwrap()
            .local
            .lv_name
            .is_empty(),
        "no history, no margin: the first cycle is a publish"
    );
    assert_eq!(
        rig.origin_row().await,
        None,
        "no repoint on the first cycle"
    );

    // The second sample supplies the rate, and now the same reserve bites.
    rig.stack.write_origin(CHUNK, &Rig::pat(CHUNK as usize, 2));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert!(eng
        .state()
        .unwrap()
        .baseline
        .as_ref()
        .unwrap()
        .local
        .lv_name
        .is_empty());
}

/// §8.1 rule 1 names a DURATION, and the duration it names is the one
/// this deployment's rescue actually takes. So the configured window is
/// a floor the engine raises from observation, never a ceiling: an
/// advance cycle IS the rescue (capture → publish → repoint), and its
/// wall time is what the margin has to reserve for.
///
/// Configured at zero here so any nonzero effective window can only
/// have come from the measurement. That is the case §8 exists for
/// inverted into a test: on a degraded link the publish dominates and
/// runs for minutes, and a constant 30 s would under-reserve by an
/// order of magnitude at exactly that moment.
#[tokio::test]
async fn the_window_is_raised_by_observed_advance_durations() {
    let rig = Rig::new().await;
    let mut eng = rig.engine().with_advance_window(std::time::Duration::ZERO);
    assert_eq!(
        eng.effective_window(),
        std::time::Duration::ZERO,
        "nothing observed yet: the floor stands alone"
    );

    // A publish cycle is not the rescue — it skips the repoint and the
    // reclaim — so timing one would read the window short.
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    assert_eq!(
        eng.effective_window(),
        std::time::Duration::ZERO,
        "publish cycles do not measure rule 1's window"
    );

    rig.stack.write_origin(CHUNK, &Rig::pat(CHUNK as usize, 2));
    eng.run_cycle_as(CycleKind::Advance).await.unwrap();
    assert!(
        eng.effective_window() > std::time::Duration::ZERO,
        "the advance's own wall time became the window"
    );
}

/// Decision 4, the other half: `advance_watermark: None` means never,
/// for BOTH terms. A deployment that never opted into watermark
/// advances must not start getting them from the margin rule — an
/// advance reclaims, and reclaim is licensed by the repoint.
#[tokio::test]
async fn no_watermark_means_the_margin_term_never_fires() {
    let rig = Rig::new().await;
    let mut eng = rig.engine().with_dirty_flush_reserve(u64::MAX / 2);
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    rig.stack.write_origin(CHUNK, &Rig::pat(CHUNK as usize, 2));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    let s = eng.state().unwrap();
    assert!(
        !s.baseline.as_ref().unwrap().local.lv_name.is_empty(),
        "still a publish cycle: the established pair, not the rebaselined one"
    );
    assert_eq!(rig.origin_row().await, None, "no repoint ever happened");
    assert_eq!(
        rig.stack.pool_usage().unwrap().0,
        2,
        "nothing reclaimed without an advance"
    );
}

/// The filesystem surface reaches the engine, not just `Pressure`: a
/// sparse backing whose filesystem is nearly full promotes a cycle the
/// pool's own occupancy would have left alone (§8.1 rule 2 feeding rule
/// 1's margin).
///
/// Sized so the fs surface is genuinely what binds, and so nothing here
/// depends on wall clock. The workload writes NOTHING between the two
/// samples, which makes the observed rate exactly zero — a real rate,
/// derived from two samples, so the margin term applies — and collapses
/// the margin to the flat reserve, a constant. The pool's own free
/// space then clears that reserve with room to spare and only the
/// filesystem's number is under it. Both halves are asserted against
/// `pressure()` before the cycle runs, so the test cannot pass on the
/// rate × window term the way it would if the numbers were merely large.
#[tokio::test]
async fn the_filesystem_surface_drives_the_margin_too() {
    const RESERVE: u64 = 4 * CHUNK;
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_advance_watermark(0.9)
        .with_dirty_flush_reserve(RESERVE);
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");

    // One chunk left on the backing filesystem, fifteen still free in
    // the pool: the two surfaces now disagree across the reserve.
    rig.stack.set_fs_free(Some(CHUNK));
    let p = rig.stack.pressure().expect("pressure");
    assert!(p.fs_bound, "the injected surface is the tighter one");
    assert!(
        p.total_bytes - p.used_bytes > RESERVE,
        "pool free ({}) must clear the reserve on its own, or the pool \
         surface could promote this cycle by itself",
        p.total_bytes - p.used_bytes
    );
    assert!(
        p.effective_free < RESERVE,
        "and the filesystem's number must not: {}",
        p.effective_free
    );

    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert_eq!(
        eng.observed_write_rate(),
        Some(0.0),
        "no growth between the samples: the margin here is the flat \
         reserve, not rate × window"
    );
    assert!(
        eng.state()
            .unwrap()
            .baseline
            .as_ref()
            .unwrap()
            .local
            .lv_name
            .is_empty(),
        "the tighter surface is what the margin compares against"
    );
}

/// A control plane whose fence read reports a stale generation: the §3
/// step-6 fence must stall the advance — no discard — and a later
/// recovery against an honest plane completes the tail.
#[derive(Clone)]
struct StaleFence(Cp);

impl ControlPlane for StaleFence {
    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError> {
        self.0.prepare(base_ref).await
    }
    async fn find_prepared(&self) -> Result<Option<PreparedInfo>, CpError> {
        self.0.find_prepared().await
    }
    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        self.0.ensure(snap_id, hashes).await
    }
    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError> {
        self.0.put_object(hash, data).await
    }
    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        self.0.attach(snap_id, op_id, ranges).await
    }
    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError> {
        self.0.clear(snap_id, op_id, ranges).await
    }
    async fn apply(
        &self,
        snap_id: &str,
        op_id: u64,
        ops: &[skynbd_agent::RangeOp],
    ) -> Result<(), CpError> {
        self.0.apply(snap_id, op_id, ops).await
    }
    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError> {
        self.0.checkpoint(snap_id, op_id).await
    }
    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError> {
        self.0.publish(snap_id, name, publisher).await
    }
    async fn abort(&self, snap_id: &str) -> Result<(), CpError> {
        self.0.abort(snap_id).await
    }
    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError> {
        self.0.ref_exists(ref_name).await
    }
    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError> {
        self.0.pinned_by(ref_name, holder).await
    }
    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.0.pin(ref_name, holder).await
    }
    async fn unpin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.0.unpin(ref_name, holder).await
    }
    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError> {
        self.0.read_snapshot(ref_name, off, len).await
    }
    async fn report_progress(&self, p: CycleProgress) -> Result<(), CpError> {
        self.0.report_progress(p).await
    }
    async fn repoint_origin(&self, target: &str) -> Result<(), CpError> {
        self.0.repoint_origin(target).await
    }
    /// The stale view: always a generation older than any advance target.
    async fn origin_served(&self) -> Result<Option<String>, CpError> {
        Ok(Some("snap/g0".into()))
    }
}

#[tokio::test]
async fn fence_failure_stalls_the_advance_and_recovery_completes_it() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    eng.run_cycle().await.unwrap(); // g0

    // Advance to g1 against a stale-fence plane: repoint lands, the
    // fence refuses, nothing is discarded.
    let mut stale = CycleEngine::open(
        rig.stack.clone(),
        StaleFence(rig.cp.clone()),
        StateStore::new(rig.dir.path()),
        "vlin",
    )
    .unwrap();
    rig.stack
        .write_origin(4 * CHUNK, &Rig::pat(CHUNK as usize, 5));
    let (used_before, _) = rig.stack.pool_usage().unwrap();
    let err = stale.run_cycle_as(CycleKind::Advance).await.unwrap_err();
    assert!(err.to_string().contains("fence"), "{err}");
    assert_eq!(
        rig.stack.pool_usage().unwrap().0,
        used_before,
        "a failed fence must not discard"
    );
    // The stall is a state, not a wedge: an honest plane's recovery
    // resumes the durable Advancing tail and finishes the §3 sequence.
    let mut eng2 = rig.engine();
    eng2.recover().await.unwrap();
    let s = eng2.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.baseline.as_ref().unwrap().remote, "snap/g1");
    assert!(s.baseline.as_ref().unwrap().local.lv_name.is_empty());
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g1"));
    assert_eq!(rig.stack.pool_usage().unwrap().0, 0);
}

/// A plane whose unpin answers the epoch-specific fence: the promotion
/// tail's best-effort unpins must PROPAGATE it (a fenced stage
/// terminates), while every other unpin failure stays swallowed.
#[derive(Clone)]
struct FencedUnpin(Cp);

impl ControlPlane for FencedUnpin {
    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError> {
        self.0.prepare(base_ref).await
    }
    async fn find_prepared(&self) -> Result<Option<PreparedInfo>, CpError> {
        self.0.find_prepared().await
    }
    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        self.0.ensure(snap_id, hashes).await
    }
    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError> {
        self.0.put_object(hash, data).await
    }
    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        self.0.attach(snap_id, op_id, ranges).await
    }
    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError> {
        self.0.clear(snap_id, op_id, ranges).await
    }
    async fn apply(
        &self,
        snap_id: &str,
        op_id: u64,
        ops: &[skynbd_agent::RangeOp],
    ) -> Result<(), CpError> {
        self.0.apply(snap_id, op_id, ops).await
    }
    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError> {
        self.0.checkpoint(snap_id, op_id).await
    }
    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError> {
        self.0.publish(snap_id, name, publisher).await
    }
    async fn abort(&self, snap_id: &str) -> Result<(), CpError> {
        self.0.abort(snap_id).await
    }
    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError> {
        self.0.ref_exists(ref_name).await
    }
    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError> {
        self.0.pinned_by(ref_name, holder).await
    }
    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.0.pin(ref_name, holder).await
    }
    async fn unpin(&self, _ref_name: &str, _holder: &str) -> Result<(), CpError> {
        Err(CpError::Fenced {
            holder: "node-b".into(),
            epoch: 9,
        })
    }
    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError> {
        self.0.read_snapshot(ref_name, off, len).await
    }
    async fn repoint_origin(&self, target: &str) -> Result<(), CpError> {
        self.0.repoint_origin(target).await
    }
    async fn origin_served(&self) -> Result<Option<String>, CpError> {
        self.0.origin_served().await
    }
}

/// C4 (task 15h): the promote tail's `unpin` calls are best-effort for
/// ordinary failures but must NOT swallow `CpError::Fenced` — the cycle
/// terminates with the terminal `AgentError::Fenced` instead of
/// completing against a superseded epoch.
#[tokio::test]
async fn fenced_unpin_terminates_the_cycle() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    eng.run_cycle().await.unwrap(); // g0, honest plane
    drop(eng);

    // The next publish cycle promotes — its publisher unpin is refused
    // with the fence, and the refusal is terminal, not shrugged off.
    let mut fenced = CycleEngine::open(
        rig.stack.clone(),
        FencedUnpin(rig.cp.clone()),
        StateStore::new(rig.dir.path()),
        "vlin",
    )
    .unwrap();
    rig.stack
        .write_origin(2 * CHUNK, &Rig::pat(CHUNK as usize, 5));
    let err = fenced.run_cycle().await.unwrap_err();
    assert!(
        matches!(
            &err,
            skynbd_agent::AgentError::Fenced { holder, epoch: 9 } if holder == "node-b"
        ),
        "{err}"
    );
    assert!(fenced.fenced().is_some(), "the engine remembers the fence");
}

// ---------------------------------------------------- §7 hybrid reclaim

fn policy(strategy: ReclaimStrategy, timing: ReclaimTiming) -> ReclaimPolicy {
    ReclaimPolicy {
        strategy,
        timing,
        ..ReclaimPolicy::default()
    }
}

/// §7's core claim at the engine level: which executor ran is not
/// observable from outside the node. A forced swing ends in exactly the
/// port's post-state — rebaselined pair, unchanged bytes, empty pool —
/// and resets the aging counter.
#[tokio::test]
async fn swing_advance_ends_identical_to_port() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Swing, ReclaimTiming::Eager));
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");

    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let before = rig.stack.origin_image();
    assert_eq!(
        eng.run_cycle_as(CycleKind::Advance).await.unwrap(),
        "snap/g1"
    );

    let s = eng.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    let b = s.baseline.as_ref().unwrap();
    assert_eq!(b.remote, "snap/g1");
    assert!(b.local.lv_name.is_empty(), "rebaselined: no local member");
    assert_eq!(s.advances_since_swing, 0, "major compaction resets aging");
    assert_eq!(s.swing_flipped, None, "steady slot record");
    assert_eq!(rig.stack.snap_count(), 0);
    assert_eq!(rig.stack.origin_image(), before, "not one observable byte");
    assert_eq!(rig.stack.pool_usage().unwrap().0, 0, "pool fully compacted");
    assert_eq!(rig.stack.swings(), 1);
    assert_eq!(rig.stack.reclaims(), 0, "the port never ran");
}

/// Auto strategy with a stack that cannot swing: the port runs no
/// matter how large R is (threshold forced to 0).
#[tokio::test]
async fn auto_picks_port_when_swing_unsupported() {
    let rig = Rig::new().await;
    let mut p = policy(ReclaimStrategy::Auto, ReclaimTiming::Eager);
    p.range_threshold = 0;
    let mut eng = rig.engine().with_policy(p);
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    eng.run_cycle_as(CycleKind::Advance).await.unwrap();
    assert_eq!(rig.stack.reclaims(), 1);
    assert_eq!(rig.stack.swings(), 0);
    assert_eq!(eng.state().unwrap().advances_since_swing, 1, "port ages");
}

/// Auto strategy past the range threshold on a capable stack: swing.
#[tokio::test]
async fn auto_picks_swing_past_threshold() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    let mut p = policy(ReclaimStrategy::Auto, ReclaimTiming::Eager);
    p.range_threshold = 0;
    let mut eng = rig.engine().with_policy(p);
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    eng.run_cycle_as(CycleKind::Advance).await.unwrap();
    assert_eq!(rig.stack.swings(), 1);
    assert_eq!(rig.stack.reclaims(), 0);
}

/// §7 pre-flip crash arm, same-process shape: nothing irreversible
/// happened, so the engine sweeps B and completes the SAME advance
/// through the port — converged, steady record, no error surfaced.
#[tokio::test]
async fn swing_failure_before_flip_falls_back_to_port() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    rig.stack.fail_swing(FailPoint::BeforeFlip);
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Swing, ReclaimTiming::Eager));
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let before = rig.stack.origin_image();
    eng.run_cycle_as(CycleKind::Advance).await.unwrap();

    assert_eq!(rig.stack.reclaims(), 1, "port fallback ran");
    assert_eq!(rig.stack.swings(), 0);
    assert_eq!(rig.stack.sweeps(), vec![false], "slot B swept");
    let s = eng.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.swing_flipped, None);
    assert_eq!(s.advances_since_swing, 1, "no completed swing, no reset");
    assert!(s.baseline.as_ref().unwrap().local.lv_name.is_empty());
    assert_eq!(rig.stack.origin_image(), before);
    assert_eq!(rig.stack.pool_usage().unwrap().0, 0);
}

/// §7 C2: the pre-flip fallback is licensed by the sweep, not by the
/// failure. A sweep that itself fails leaves slot B in an unknown
/// partial state, and clearing the record over it would strand a live B
/// nothing revisits — while the port reclaim it would fall back to
/// discards through a pool the record has just called garbage. The
/// record must stay armed and the cycle must stall.
#[tokio::test]
async fn a_failed_sweep_keeps_the_record_armed_and_never_falls_back() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    rig.stack.fail_swing(FailPoint::BeforeFlip);
    rig.stack.fail_next_sweep();
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Swing, ReclaimTiming::Eager));
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let before = rig.stack.origin_image();

    let err = eng
        .run_cycle_as(CycleKind::Advance)
        .await
        .expect_err("a failed sweep stalls the advance");
    assert!(err.to_string().contains("sweep"), "{err}");
    assert_eq!(rig.stack.sweeps(), vec![false], "the armed sweep was tried");
    assert_eq!(
        rig.stack.reclaims(),
        0,
        "no port fallback over a slot that may still be live"
    );
    assert_eq!(rig.stack.swings(), 0);
    assert_eq!(
        eng.state().unwrap().swing_flipped,
        Some(false),
        "the record stays armed: recovery re-runs the sweep"
    );
    assert_eq!(rig.stack.origin_image(), before, "no byte moved");
}

/// §7 post-flip crash arm: the flip is durable, so the failure
/// propagates; a fresh engine's recovery sweeps slot A (`sweep_slot(true)`)
/// and finishes the §3 tail to the rebaselined pair.
#[tokio::test]
async fn swing_crash_after_flip_recovers_via_sweep() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Swing, ReclaimTiming::Eager));
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let before = rig.stack.origin_image();
    rig.stack.fail_swing(FailPoint::AfterFlip);
    let err = eng.run_cycle_as(CycleKind::Advance).await.unwrap_err();
    assert!(err.to_string().contains("after flip"), "{err}");
    assert_eq!(
        eng.state().unwrap().swing_flipped,
        Some(true),
        "the durable record names B"
    );
    drop(eng);

    let mut eng2 = rig.engine();
    eng2.recover().await.unwrap();
    assert_eq!(rig.stack.sweeps(), vec![true], "recovery swept slot A");
    let s = eng2.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.swing_flipped, None);
    assert_eq!(
        s.advances_since_swing, 0,
        "the compaction landed: the forced-major cadence starts from \
         zero, not from the re-run tail's NoOp"
    );
    let b = s.baseline.as_ref().unwrap();
    assert_eq!(b.remote, "snap/g1");
    assert!(b.local.lv_name.is_empty(), "rebaselined");
    assert_eq!(rig.stack.origin_image(), before);
    assert_eq!(rig.stack.pool_usage().unwrap().0, 0);
    assert_eq!(rig.stack.snap_count(), 0);
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g1"));
}

/// §7.1 lazy timing: the advance runs the §3 head (repoint + fence) but
/// retains the ESTABLISHED pair — S stays, the pool stays as read cache
/// — and the ordinary publish rotation continues from it bit-exactly.
#[tokio::test]
async fn lazy_advance_retains_established_pair() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Auto, ReclaimTiming::Lazy));
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap(); // g0
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let (used_before, _) = rig.stack.pool_usage().unwrap();
    assert_eq!(
        eng.run_cycle_as(CycleKind::Advance).await.unwrap(),
        "snap/g1"
    );

    let s = eng.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    let b = s.baseline.as_ref().unwrap();
    assert_eq!(b.remote, "snap/g1");
    assert_eq!(
        b.local.lv_name, "skynbd-vlin-snap-g1",
        "the capture snapshot IS the local pair member"
    );
    assert_eq!(rig.stack.snap_count(), 1, "S retained, old baseline gone");
    assert_eq!(
        rig.stack.pool_usage().unwrap().0,
        used_before,
        "no reclaim: the published set stays pooled as read cache"
    );
    assert_eq!(rig.stack.reclaims(), 0);
    assert_eq!(s.advances_since_swing, 1);
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g1"));

    // The next publish cycle rotates the established pair normally and
    // stays bit-exact.
    rig.stack
        .write_origin(3 * CHUNK, &Rig::pat(CHUNK as usize, 33));
    let img = rig.stack.origin_image();
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g2");
    assert_eq!(rig.cp.read_snapshot("snap/g2", 0, VOL).await.unwrap(), img);
    let s = eng.state().unwrap();
    assert_eq!(
        s.baseline.as_ref().unwrap().local.lv_name,
        "skynbd-vlin-snap-g2"
    );
    assert_eq!(rig.stack.snap_count(), 1, "rotation deleted the old S");
}

/// §7.1 lazy eviction: watermark pressure discards still-shared ranges
/// of the retained pair (dual-side, so no later delta misreports),
/// pool usage drops, and content stays bit-exact — and the relief is
/// what the cycle is then judged on, so the pressure that triggered the
/// eviction does not ALSO buy an advance.
#[tokio::test]
async fn lazy_watermark_pressure_evicts_instead_of_advancing() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Auto, ReclaimTiming::Lazy))
        .with_advance_watermark(0.2)
        // Rule 1's margin term switched off (zero reserve, zero window),
        // so occupancy is the only live term and the question under test
        // is which READING of it the kind comes from.
        .with_dirty_flush_reserve(0)
        .with_advance_window(std::time::Duration::ZERO);
    // 4/16 chunks mapped = over the watermark: the bootstrap cycle is
    // already promoted to a (lazy) advance and retains the pair.
    rig.stack.write_origin(0, &Rig::pat(4 * CHUNK as usize, 1));
    let img0 = rig.stack.origin_image();
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    let s = eng.state().unwrap();
    assert!(!s.baseline.as_ref().unwrap().local.lv_name.is_empty());
    assert_eq!(rig.stack.pool_usage().unwrap().0, 4, "nothing reclaimed");

    // Still over the watermark: the next cycle evicts (fenced against
    // the served origin) before publishing.
    let t = eng.run_cycle().await.unwrap();
    assert!(rig.stack.evicts() >= 1, "watermark pressure evicted");
    assert_eq!(rig.stack.pool_usage().unwrap().0, 0, "pressure relieved");
    assert_eq!(rig.stack.origin_image(), img0, "eviction moved no byte");
    assert_eq!(rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img0);
    // The kind is decided from the pool the eviction left behind, so
    // this cycle is a plain publish: the pointer stays where the
    // bootstrap advance put it. Deciding from the pre-eviction reading
    // would repoint here — and on every cycle the pressure recurred,
    // each with its own fsfreeze, for relief the eviction had already
    // delivered.
    assert_eq!(
        rig.origin_row().await.as_deref(),
        Some("snap/g0"),
        "an eviction that relieved the pressure must not also advance"
    );

    // Fresh writes after the eviction publish bit-exactly — the dual
    // discard kept the retained pair's delta honest.
    rig.stack
        .write_origin(9 * CHUNK, &Rig::pat(CHUNK as usize, 77));
    let img2 = rig.stack.origin_image();
    let t2 = eng.run_cycle().await.unwrap();
    assert_eq!(rig.cp.read_snapshot(&t2, 0, VOL).await.unwrap(), img2);
}

/// The other side of that decision: eviction is bounded (8 × 1024
/// ranges) and can only discard what the retained pair still shares, so
/// a pool full of DIRTY blocks stays over the watermark however hard it
/// evicts — and then the advance is exactly what the watermark is for.
/// The two tests together pin the reading the kind comes from: pass
/// either one with the pre-eviction pressure and the other fails.
#[tokio::test]
async fn an_eviction_that_cannot_relieve_the_pressure_still_advances() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Auto, ReclaimTiming::Lazy))
        .with_advance_watermark(0.2)
        // Same as above: occupancy alone, so neither half of this pair
        // of tests can pass on the margin term.
        .with_dirty_flush_reserve(0)
        .with_advance_window(std::time::Duration::ZERO);
    // 4/16 shared with the pair after the bootstrap advance — the only
    // blocks an eviction can reach.
    rig.stack.write_origin(0, &Rig::pat(4 * CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g0"));

    // Eight more chunks the pair has never seen: 12/16 now, and the
    // most an eviction can hand back is the 4 shared ones, leaving 8/16
    // — still over the watermark.
    rig.stack
        .write_origin(4 * CHUNK, &Rig::pat(8 * CHUNK as usize, 5));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert!(rig.stack.evicts() >= 1, "it still tried to evict");
    assert!(
        rig.stack.pool_usage().unwrap().0 >= 8,
        "the dirty set is unevictable: {}",
        rig.stack.pool_usage().unwrap().0
    );
    assert_eq!(
        rig.origin_row().await.as_deref(),
        Some("snap/g1"),
        "pressure the eviction could not relieve is what the watermark advances on"
    );
}

/// §8.1's margin arm guards on `total_bytes > 0` because a pool that
/// reports 0/0 has `effective_free == 0`, which is under every margin
/// there is. Without the guard a stack answering `pressure()` for a
/// pool it cannot size — or one sized before its geometry is known —
/// would advance on every cycle after the first, freeze included, on a
/// reading that says nothing at all.
#[tokio::test]
async fn a_zero_sized_pool_never_promotes_on_the_margin() {
    let rig = Rig::new().await;
    // A pool with no blocks: used 0, total 0, effective_free 0.
    let stack = FakeStack::new(0, CHUNK);
    let p = stack.pressure().expect("a 0/0 pool still answers");
    assert_eq!((p.used_bytes, p.total_bytes, p.effective_free), (0, 0, 0));
    let mut eng = CycleEngine::open(
        stack,
        rig.cp.clone(),
        StateStore::new(rig.dir.path()),
        "vlin",
    )
    .unwrap()
    .with_advance_watermark(0.9)
    // A reserve nothing could ever clear, so the margin arm is armed
    // from the second cycle on.
    .with_dirty_flush_reserve(u64::MAX / 2);

    for gen in ["snap/g0", "snap/g1", "snap/g2"] {
        assert_eq!(eng.run_cycle().await.unwrap(), gen);
    }
    assert_eq!(
        rig.origin_row().await,
        None,
        "0/0 is not a full pool: nothing here licenses an advance"
    );
    assert!(
        !eng.state()
            .unwrap()
            .baseline
            .as_ref()
            .unwrap()
            .local
            .lv_name
            .is_empty(),
        "every cycle stayed a publish cycle"
    );
}

/// §8.1 rule 1 under §7.1 LAZY timing — the mode with the least
/// headroom and the most need of the margin, because it keeps the
/// published set pooled as read cache on purpose.
///
/// Lazy timing frees pool blocks on its way through a cycle, so the
/// order of "sample the pool" against "evict from the pool" decides
/// what the rate measures. Sampling after the eviction reads
/// `used <= previous` on every evicting cycle, `derive_write_rate`
/// floors that at zero (a pool giving space back is not a workload
/// writing), the rate saturates to zero, and rule 1's margin silently
/// collapses to the flat reserve — for as long as the pressure lasts.
/// The workload's rate has to survive the eviction that shares its
/// cycle.
///
/// Asserted on the rate itself rather than on the promotion: at an
/// evicting cycle the occupancy term is over the watermark by
/// construction (that is what triggered the eviction), so the promotion
/// cannot distinguish the two orderings. The rate is the margin's only
/// rate-dependent input, and it is exactly what the ordering breaks.
#[tokio::test]
async fn lazy_evictions_do_not_erase_the_observed_write_rate() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Auto, ReclaimTiming::Lazy))
        .with_advance_watermark(0.25);

    // Cycle 1 (4/16, at the watermark): the bootstrap has no retained
    // pair to evict from, so it only samples — and, being a lazy
    // advance, it repoints, which is what licenses the next cycle's
    // eviction (Invariant E: the fence runs against the pair's
    // generation before one block is discarded).
    rig.stack.write_origin(0, &Rig::pat(4 * CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    assert_eq!(rig.stack.evicts(), 0, "nothing retained to evict yet");
    assert_eq!(
        eng.observed_write_rate(),
        None,
        "one sample is no rate at all"
    );

    // Cycle 2: three more chunks take the pool to 7/16, still over the
    // watermark. This cycle both samples and evicts.
    rig.stack
        .write_origin(4 * CHUNK, &Rig::pat(3 * CHUNK as usize, 7));
    let used_before = rig.stack.pool_usage().unwrap().0;
    assert_eq!(used_before, 7);
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert!(rig.stack.evicts() >= 1, "watermark pressure evicted");
    assert!(
        rig.stack.pool_usage().unwrap().0 < used_before,
        "the eviction really did shrink the pool inside this cycle"
    );

    // The claim: the rate reflects the 3 chunks the WORKLOAD wrote
    // between the samples, not the blocks the eviction handed back.
    let rate = eng
        .observed_write_rate()
        .expect("two samples exist, so a rate exists");
    assert!(
        rate > 0.0,
        "an evicting cycle must not read as a zero write rate — that is \
         rule 1's margin collapsing to the flat reserve in the one mode \
         that runs the pool deliberately full"
    );
}

/// The blind spot sampling-before-eviction MOVED rather than closed:
/// the sample recorded for the next comparison is the PRE-eviction
/// `used`, so the cycle AFTER an eviction measures its growth against a
/// baseline the eviction has since undercut. The writer can be running
/// flat out and the pair still saturates to zero — one cycle later,
/// with the same collapse to the flat reserve.
///
/// So the rate is a decayed maximum, exactly like the window: the rate
/// the workload proved it can produce survives the cycle whose sample
/// the eviction flattened, and only a run of genuinely quiet cycles
/// ages it out.
#[tokio::test]
async fn the_rate_survives_the_cycle_after_an_eviction() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Auto, ReclaimTiming::Lazy))
        .with_advance_watermark(0.25);

    // Cycle 1: 4/16, a lazy advance that retains the pair (and repoints,
    // which is what licenses the next cycle's eviction).
    rig.stack.write_origin(0, &Rig::pat(4 * CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");

    // Cycle 2: 7/16 at the sample, then the eviction hands the 4 shared
    // chunks back. This is the pair the rate is honest about.
    rig.stack
        .write_origin(4 * CHUNK, &Rig::pat(3 * CHUNK as usize, 7));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    let after_evicting_cycle = eng
        .observed_write_rate()
        .expect("two samples exist, so a rate exists");
    assert!(after_evicting_cycle > 0.0);

    // Cycle 3: the workload keeps writing, but `used` (3 + 1) is still
    // BELOW the pre-eviction sample of 7, so this cycle's own pair
    // measures exactly nothing.
    rig.stack
        .write_origin(7 * CHUNK, &Rig::pat(CHUNK as usize, 11));
    let used_now = rig.stack.pool_usage().unwrap().0;
    assert!(
        used_now < 7,
        "the premise: the eviction freed more than the writer produced ({used_now} vs 7)"
    );
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g2");
    let rate = eng.observed_write_rate().expect("still a rate");
    assert!(
        rate > 0.0,
        "the cycle after an eviction measures against a pre-eviction \
         baseline and reads zero growth; a plain replace would put rule \
         1's margin at the flat reserve here, on a pool that is filling"
    );
    assert!(
        rate <= after_evicting_cycle,
        "and it only ages: {rate} vs {after_evicting_cycle}"
    );
}

/// Invariant B, the hazard the mask exists to close: a shim that cannot
/// be masked must not stop the volume publishing, and must not stop
/// being re-tried. The engine re-derives the mask from the kernel at
/// the head of EVERY cycle, so a lapse cannot outlive one cadence
/// interval — before this, a mask failure after a retarget left a live,
/// thawed, mounted volume passing guest discards with nothing but an
/// `eprintln!` and no revisit until the next swing or a cold re-stage.
///
/// The re-derivation is what makes the condition observable, so nothing
/// about it is persisted: a fresh process learns the same fact from the
/// same kernel within one interval, and a record-format bump to carry
/// it would cost every volume on the node a quarantine and a re-seed.
#[tokio::test]
async fn an_unmaskable_shim_still_publishes_and_is_re_asserted_every_cycle() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    assert_eq!(rig.stack.mask_asserts(), 1, "asserted on a healthy cycle");

    // The shim stops taking the mask. The cycle must still publish —
    // withholding the delta would lose it as well as the mask.
    rig.stack.set_mask_fails(true);
    rig.stack.write_origin(CHUNK, &Rig::pat(CHUNK as usize, 2));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert_eq!(rig.stack.mask_asserts(), 2, "tried again despite failing");

    // Still failing: every cycle retries rather than giving up on it.
    rig.stack
        .write_origin(2 * CHUNK, &Rig::pat(CHUNK as usize, 3));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g2");
    assert_eq!(rig.stack.mask_asserts(), 3);

    // A fresh engine over the same store — the driver-restart shape —
    // re-derives it too, with no durable state to consult.
    rig.stack.set_mask_fails(false);
    let mut reopened = rig.engine();
    rig.stack
        .write_origin(3 * CHUNK, &Rig::pat(CHUNK as usize, 4));
    reopened.run_cycle().await.unwrap();
    assert_eq!(rig.stack.mask_asserts(), 4, "a restart re-asserts");
}

/// The mask's third answer: no shim is CONFIGURED, so nothing was ever
/// asserted. That is not a lapse — there is no queue to retry against —
/// but it is not protection either, and the engine used to receive it
/// as `Ok(())`, i.e. as a verified mask. It must not fail the cycle
/// (the volume is unguarded whether or not it publishes, and refusing
/// to publish only adds a stranded delta) and it must keep being
/// re-derived, because the fix is a config change this process will see
/// as a shim appearing.
#[tokio::test]
async fn a_volume_with_no_shim_configured_still_publishes_and_is_re_derived() {
    let rig = Rig::new().await;
    rig.stack.set_mask_unsupported(true);
    let mut eng = rig.engine();
    rig.stack.write_origin(0, &Rig::pat(CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    assert_eq!(rig.stack.mask_asserts(), 1);

    rig.stack.write_origin(CHUNK, &Rig::pat(CHUNK as usize, 2));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    assert_eq!(
        rig.stack.mask_asserts(),
        2,
        "the config gap is re-derived like every other mask state"
    );

    // A shim appears (the operator fixed the config and restaged): the
    // engine goes straight back to asserting it.
    rig.stack.set_mask_unsupported(false);
    rig.stack
        .write_origin(2 * CHUNK, &Rig::pat(CHUNK as usize, 3));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g2");
    assert_eq!(rig.stack.mask_asserts(), 3);
}

/// §4 cost case 1: the agent's publish path (prepared sessions + alias)
/// must not materialize emap objects — those belong to configured
/// snapshots via 15f's promotion, never to minute-cadence publishes.
#[tokio::test]
async fn publish_cycles_write_no_emap_objects() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    for seed in [1u8, 2, 3] {
        rig.stack
            .write_origin(u64::from(seed) * CHUNK, &Rig::pat(CHUNK as usize, seed));
        eng.run_cycle().await.unwrap();
    }
    let emaps = rig
        .store
        .list(&format!("skynbd/v1/{}/emap/", rig.vol))
        .await
        .unwrap();
    assert!(emaps.is_empty(), "publish path wrote emaps: {emaps:?}");
}

/// §7 crash windows, the arm the durable flag cannot describe: the
/// executor retargeted the shim onto slot B and then failed without
/// putting the tables back. The live filesystem is on B while the
/// record still names A, so the port fallback would discard through a
/// pool the record calls garbage. The cycle must stall with the record
/// still armed and let recovery — which repairs the tables from the
/// record — resolve it.
#[tokio::test]
async fn swing_failure_after_retarget_stalls_instead_of_falling_back() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    rig.stack.fail_swing(FailPoint::AfterRetarget);
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Swing, ReclaimTiming::Eager));
    rig.stack.write_origin(0, &Rig::pat(2 * CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();
    rig.stack
        .write_origin(5 * CHUNK, &Rig::pat(CHUNK as usize, 9));
    let before = rig.stack.origin_image();

    let err = eng
        .run_cycle_as(CycleKind::Advance)
        .await
        .expect_err("a retargeted-but-unflipped swing stalls the advance");
    assert!(format!("{err}").contains("after retarget"), "{err}");
    assert_eq!(rig.stack.reclaims(), 0, "no port fallback onto slot B");
    assert_eq!(
        eng.state().unwrap().swing_flipped,
        Some(false),
        "the record stays armed for recovery"
    );

    // Recovery: the armed sweep is the repair, and the re-run tail
    // completes the advance.
    let mut eng2 = rig.engine();
    eng2.recover().await.unwrap();
    assert!(rig.stack.sweeps().contains(&false), "armed sweep ran");
    let s = eng2.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.swing_flipped, None);
    assert_eq!(rig.stack.origin_image(), before, "no byte moved");
}

/// §7.1's load-bearing skip: eviction is a reclaim, so it carries
/// Invariant E's obligation against the PAIR's generation. Once publish
/// cycles roll the established pair past the last repoint, the pair's
/// shared blocks are not what the origin serves — evicting them would
/// resurrect the older generation's content at those LBAs. The fence
/// must refuse until the next advance moves the pointer forward.
#[tokio::test]
async fn lazy_eviction_is_skipped_while_the_pair_leads_the_origin() {
    let rig = Rig::new().await;
    let mut eng = rig
        .engine()
        .with_policy(policy(ReclaimStrategy::Auto, ReclaimTiming::Lazy))
        .with_advance_watermark(0.2);
    rig.stack.write_origin(0, &Rig::pat(4 * CHUNK as usize, 1));
    eng.run_cycle_as(CycleKind::Advance).await.unwrap(); // pair + origin on g0
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g0"));

    // A publish cycle rotates the pair to g1; the pointer stays at g0.
    rig.stack
        .write_origin(8 * CHUNK, &Rig::pat(CHUNK as usize, 5));
    eng.run_cycle_as(CycleKind::Publish).await.unwrap();
    assert_eq!(
        eng.state().unwrap().baseline.as_ref().unwrap().remote,
        "snap/g1"
    );
    assert_eq!(rig.origin_row().await.as_deref(), Some("snap/g0"));
    let img = rig.stack.origin_image();
    let (used, total) = rig.stack.pool_usage().unwrap();
    // The skip is only meaningful if the pressure is real: without this,
    // any change that stopped the pool crossing 0.2 would make the
    // assertions below pass for the wrong reason.
    assert!(
        used as f64 / total as f64 >= 0.2,
        "the scenario must be over the watermark to test the skip: {used}/{total}"
    );

    // Over the watermark, but the pair leads the origin: no eviction.
    eng.run_cycle().await.unwrap();
    assert_eq!(rig.stack.evicts(), 0, "fence refused the eviction");
    assert!(rig.stack.pool_usage().unwrap().0 >= used, "nothing evicted");
    assert_eq!(rig.stack.origin_image(), img, "content untouched");
}

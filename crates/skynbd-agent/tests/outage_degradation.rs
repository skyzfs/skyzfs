//! Task 15j part B: §8.1 rule 3 (`publishFailingReadOnlyWatermark`) and
//! the §8.2 recovery ordering, engine level.
//!
//! Claims under test:
//! - rule 3 needs BOTH terms — publishes failing for T *and* a pool at
//!   or above X. A failing publish on a quiet pool is an outage to ride
//!   out, and stopping the workload for it would be pure loss;
//! - the remount fires once per outage, not once per cycle;
//! - a fenced cycle does not start the failing clock: the node has lost
//!   the volume, and degrading a filesystem it no longer owns helps
//!   nobody;
//! - a failed remount does not fail the cycle it was trying to protect;
//! - §8.2 step 5: a degraded volume reclaims with the PORT executor,
//!   whatever the policy says;
//! - §8.2 step 6: read-write is restored only AFTER space has actually
//!   come back — asserted as a call ORDER, since that is what the rule
//!   is;
//! - a successful publish under degradation ends the outage (the clock
//!   clears) but does NOT return the volume to read-write: a publish
//!   frees nothing, and remounting over a still-full pool re-wedges on
//!   the first journal write.

use std::time::Duration;

use skynbd_agent::{
    AgentError, ControlPlane, CpError, CycleEngine, CycleKind, FakeStack, InprocControlPlane,
    LocalStack, PreparedInfo, ReclaimPolicy, ReclaimStrategy, StateStore,
};
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
/// 16 pool blocks, so occupancy moves in clean sixteenths.
const VOL: u64 = 16 * CHUNK;

type Cp = InprocControlPlane<MemoryStore, MemoryRefStore>;

/// How the wrapped plane should answer `prepare` — the first
/// control-plane call every cycle makes, so failing it fails the cycle
/// before anything else has happened.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Pass,
    /// An ordinary transport failure: what a partition looks like.
    Down,
    /// Invariant F's terminal refusal.
    Fenced,
}

#[derive(Clone)]
struct Flaky {
    inner: Cp,
    mode: std::sync::Arc<std::sync::Mutex<Mode>>,
}

impl Flaky {
    fn new(inner: Cp) -> Self {
        Self {
            inner,
            mode: std::sync::Arc::new(std::sync::Mutex::new(Mode::Pass)),
        }
    }
    fn set(&self, m: Mode) {
        *self.mode.lock().unwrap() = m;
    }
    fn gate(&self) -> Result<(), CpError> {
        match *self.mode.lock().unwrap() {
            Mode::Pass => Ok(()),
            Mode::Down => Err(CpError::Other("connection refused".into())),
            Mode::Fenced => Err(CpError::Fenced {
                holder: "node-b".into(),
                epoch: 9,
            }),
        }
    }
}

impl ControlPlane for Flaky {
    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError> {
        self.gate()?;
        self.inner.prepare(base_ref).await
    }
    async fn find_prepared(&self) -> Result<Option<PreparedInfo>, CpError> {
        self.gate()?;
        self.inner.find_prepared().await
    }
    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        self.inner.ensure(snap_id, hashes).await
    }
    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError> {
        self.inner.put_object(hash, data).await
    }
    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        self.inner.attach(snap_id, op_id, ranges).await
    }
    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError> {
        self.inner.clear(snap_id, op_id, ranges).await
    }
    async fn apply(
        &self,
        snap_id: &str,
        op_id: u64,
        ops: &[skynbd_agent::RangeOp],
    ) -> Result<(), CpError> {
        self.inner.apply(snap_id, op_id, ops).await
    }
    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError> {
        self.inner.checkpoint(snap_id, op_id).await
    }
    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError> {
        self.inner.publish(snap_id, name, publisher).await
    }
    async fn abort(&self, snap_id: &str) -> Result<(), CpError> {
        self.inner.abort(snap_id).await
    }
    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError> {
        self.inner.ref_exists(ref_name).await
    }
    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError> {
        self.inner.pinned_by(ref_name, holder).await
    }
    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.inner.pin(ref_name, holder).await
    }
    async fn unpin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.inner.unpin(ref_name, holder).await
    }
    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError> {
        self.inner.read_snapshot(ref_name, off, len).await
    }
    async fn repoint_origin(&self, target: &str) -> Result<(), CpError> {
        self.inner.repoint_origin(target).await
    }
    async fn origin_served(&self) -> Result<Option<String>, CpError> {
        self.inner.origin_served().await
    }
}

struct Rig {
    stack: FakeStack,
    cp: Flaky,
    dir: tempfile::TempDir,
}

impl Rig {
    async fn new() -> Self {
        let stack = FakeStack::new(VOL, CHUNK);
        let store = MemoryStore::new();
        let refs = MemoryRefStore::new();
        let vol = VolumeId(ulid::Ulid::new());
        let cp = InprocControlPlane::create_volume(store, refs, vol, VOL)
            .await
            .unwrap();
        Rig {
            stack,
            cp: Flaky::new(cp),
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn engine(&self) -> CycleEngine<FakeStack, Flaky> {
        CycleEngine::open(
            self.stack.clone(),
            self.cp.clone(),
            StateStore::new(self.dir.path()),
            "vlin",
        )
        .unwrap()
    }

    /// Provision `blocks` of the 16, so occupancy is a known fraction.
    fn fill(&self, blocks: u64) {
        for c in 0..blocks {
            self.stack
                .write_origin(c * CHUNK, &vec![c as u8 + 1; CHUNK as usize]);
        }
    }
}

/// T satisfied the instant a cycle has failed once.
const NOW: Duration = Duration::ZERO;
/// T that no test will ever wait out.
const NEVER: Duration = Duration::from_secs(86_400);

#[tokio::test]
async fn rule_3_needs_both_terms() {
    // (a) Failing, pool full, T not yet elapsed → nothing.
    {
        let rig = Rig::new().await;
        rig.fill(15); // 93.75%
        let mut eng = rig.engine().with_ro_watermark(0.9, NEVER);
        rig.cp.set(Mode::Down);
        eng.run_cycle().await.expect_err("cycle fails");
        eng.run_cycle().await.expect_err("cycle fails");
        assert!(!eng.degraded(), "T has not elapsed");
        assert!(!rig.stack.read_only());
    }
    // (b) Failing long enough, pool quiet → nothing. The volume has
    // room; the outage is one to ride out, not to stop writers for.
    {
        let rig = Rig::new().await;
        rig.fill(4); // 25%
        let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
        rig.cp.set(Mode::Down);
        eng.run_cycle().await.expect_err("cycle fails");
        eng.run_cycle().await.expect_err("cycle fails");
        assert!(!eng.degraded(), "the pool is nowhere near the watermark");
        assert!(!rig.stack.read_only());
    }
    // (c) Both → the remount.
    {
        let rig = Rig::new().await;
        rig.fill(15);
        let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
        rig.cp.set(Mode::Down);
        eng.run_cycle().await.expect_err("cycle fails");
        assert!(!eng.degraded(), "not before a failure has been observed");
        eng.run_cycle().await.expect_err("cycle fails");
        assert!(eng.degraded(), "both terms hold");
        assert!(rig.stack.read_only(), "the superblock is read-only");
    }
}

/// Unconfigured is off, however bad it gets. A deliberate `EROFS` stops
/// the workload, so it stays opt-in.
#[tokio::test]
async fn rule_3_is_off_unless_configured() {
    let rig = Rig::new().await;
    rig.fill(16); // 100%
    let mut eng = rig.engine();
    rig.cp.set(Mode::Down);
    for _ in 0..5 {
        eng.run_cycle().await.expect_err("cycle fails");
    }
    assert!(!eng.degraded());
    assert!(!rig.stack.read_only());
    assert!(rig.stack.calls().is_empty(), "{:?}", rig.stack.calls());
}

#[tokio::test]
async fn rule_3_remounts_once_per_outage() {
    let rig = Rig::new().await;
    rig.fill(15);
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
    rig.cp.set(Mode::Down);
    for _ in 0..6 {
        eng.run_cycle().await.expect_err("cycle fails");
    }
    assert!(eng.degraded());
    let remounts = rig
        .stack
        .calls()
        .iter()
        .filter(|c| **c == "degrade_ro")
        .count();
    assert_eq!(remounts, 1, "{:?}", rig.stack.calls());
}

/// Invariant F's fence is terminal, not a failing publish. Starting the
/// clock here would mean a volume this node has already lost degrades
/// its filesystem on the way out.
#[tokio::test]
async fn a_fenced_cycle_does_not_start_the_failing_clock() {
    let rig = Rig::new().await;
    rig.fill(15);
    // The most permissive rule-3 configuration there is: if the clock
    // had started, this would fire on the very next cycle.
    let mut eng = rig.engine().with_ro_watermark(0.0, NOW);
    rig.cp.set(Mode::Fenced);
    let e = eng.run_cycle().await.expect_err("fenced");
    assert!(matches!(e, AgentError::Fenced { .. }), "{e:?}");

    // The rule-3 check is the first thing the next cycle does, so this
    // second call evaluates the clock the fenced cycle would have
    // started — whatever it then goes on to fail with itself.
    let _ = eng.run_cycle().await;
    assert!(!eng.degraded(), "a fence is not a failing publish");
    assert!(!rig.stack.read_only());
    // Teeth: the identical two-cycle sequence with an ordinary
    // transport failure DOES degrade — `rule_3_needs_both_terms` case
    // (c), same watermark, same T.
}

/// Rule 3 is a mitigation. Refusing to attempt the publish because the
/// mitigation failed would give up the delta AND leave the filesystem
/// exposed — strictly worse than either alone.
#[tokio::test]
async fn a_failed_remount_does_not_fail_the_cycle() {
    let rig = Rig::new().await;
    rig.fill(15);
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
    rig.stack.set_remount_fails(true);
    rig.cp.set(Mode::Down);
    eng.run_cycle()
        .await
        .expect_err("cycle fails: the CP is down");

    // The rule fires now, fails, and the cycle behind it still runs and
    // still fails for its OWN reason. Then the plane comes back and the
    // publish succeeds — the failed mitigation blocked nothing.
    eng.run_cycle()
        .await
        .expect_err("cycle fails: the CP is down");
    assert!(!eng.degraded(), "the remount did not take");
    rig.cp.set(Mode::Pass);
    eng.recover()
        .await
        .expect("recovery reconciles the failed cycles");
    eng.run_cycle()
        .await
        .expect("publishes once the CP is back");
}

/// §8.2 step 5 at engine level: the executor a degraded volume reclaims
/// with is the port, even under a `Swing` override on a swing-capable
/// stack with ample headroom.
#[tokio::test]
async fn a_degraded_volume_reclaims_with_the_port() {
    let rig = Rig::new().await;
    rig.stack.set_swing_supported(true);
    rig.stack.set_headroom(Some((1 << 40, 1 << 20)));
    rig.fill(15);
    let mut eng = rig
        .engine()
        .with_ro_watermark(0.9, NOW)
        .with_policy(ReclaimPolicy {
            strategy: ReclaimStrategy::Swing,
            ..ReclaimPolicy::default()
        });

    // Seed a baseline so the advance has something to reclaim against.
    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance).await.expect("seed");
    // The seed advance RECLAIMED the pool empty, so refill it: rule 3's
    // occupancy term is about the pool as the outage finds it.
    rig.fill(15);

    // Now the outage, then rule 3, then the recovery advance.
    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("cycle fails");
    eng.run_cycle().await.expect_err("cycle fails");
    assert!(eng.degraded());
    let swings_before = rig.stack.swings();
    let reclaims_before = rig.stack.reclaims();

    rig.cp.set(Mode::Pass);
    eng.recover()
        .await
        .expect("recovery reconciles the failed cycles");
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect("recovery advance");
    assert_eq!(
        rig.stack.swings(),
        swings_before,
        "a swing cannot run on a read-only superblock"
    );
    assert!(
        rig.stack.reclaims() > reclaims_before,
        "the port drained instead"
    );
}

/// §8.2 step 6 as an ordering assertion — which is the only way to state
/// it. Journal replay allocates, so a read-write restore that lands
/// before the reclaim re-wedges the filesystem within seconds.
#[tokio::test]
async fn read_write_is_restored_only_after_the_reclaim() {
    let rig = Rig::new().await;
    rig.fill(15);
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);

    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance).await.expect("seed");
    rig.fill(15); // the seed advance reclaimed the pool empty

    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("cycle fails");
    eng.run_cycle().await.expect_err("cycle fails");
    assert!(eng.degraded() && rig.stack.read_only());

    rig.cp.set(Mode::Pass);
    eng.recover()
        .await
        .expect("recovery reconciles the failed cycles");
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect("recovery advance");
    assert!(!eng.degraded(), "the volume came back");
    assert!(!rig.stack.read_only());

    let calls = rig.stack.calls();
    let ro = calls.iter().position(|c| *c == "degrade_ro").expect("ro");
    let rw = calls.iter().position(|c| *c == "restore_rw").expect("rw");
    let reclaim = calls
        .iter()
        .enumerate()
        .filter(|(i, c)| **c == "reclaim" && *i > ro)
        .map(|(i, _)| i)
        .next()
        .expect("a reclaim after the degrade");
    assert!(ro < reclaim, "the degrade came first: {calls:?}");
    assert!(
        reclaim < rw,
        "space must come back BEFORE the writers do: {calls:?}"
    );
}

/// `MS_REMOUNT` SUCCEEDS on an aborted superblock and changes nothing.
/// Measured on 6.18.5:
///
/// ```text
///   clean ro  -> remount rc=0, rw,relatime                  write OK
///   ABORTED   -> remount rc=0, rw,...,abort,emergency_ro    write EROFS
/// ```
///
/// `statvfs` reports `ST_RDONLY=0` for the aborted super and
/// `access(W_OK)` returns OK, so neither can tell them apart — only a
/// real write can. A restore that trusted its own return value would
/// clear the latch and log "filesystem back READ-WRITE" about a volume
/// that will not take a byte. That is a FALSE ALL-CLEAR, and it is
/// worse than the stall it replaces: the operator log lies, §8.2
/// step 5's routing term goes quiet, and adoption cannot recover the
/// volume because an aborted super reports `rw`.
#[tokio::test]
async fn a_restore_onto_an_aborted_superblock_is_not_reported_as_success() {
    let rig = Rig::new().await;
    rig.fill(15);
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);

    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance).await.expect("seed");
    rig.fill(15);
    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("cycle fails");
    eng.run_cycle().await.expect_err("cycle fails");
    assert!(eng.degraded() && rig.stack.read_only());

    // The superblock aborts while the volume is degraded — the window
    // between rule 3's remount and the restore.
    rig.stack.abort_fs();

    rig.cp.set(Mode::Pass);
    eng.recover().await.expect("recovery reconciles");
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect("recovery advance");

    // The remount was attempted and returned Ok — and must not be
    // believed.
    assert!(
        rig.stack.calls().contains(&"restore_rw"),
        "the restore was attempted: {:?}",
        rig.stack.calls()
    );
    assert!(
        eng.degraded(),
        "an accepted-but-inert remount is not a restore"
    );
    assert!(
        !rig.stack.mount_is_writable().unwrap_or(true),
        "the volume genuinely still refuses writes"
    );
}

/// The remount outlives the process; the `degraded` latch does not.
///
/// A driver restart mid-outage is the ordinary case — kubelet replays
/// the stage onto the live attachment and the cycle starts again in a
/// fresh engine. Without re-derivation that engine believes the volume
/// is writable, `restore_after_reclaim` early-returns forever, and the
/// network can come back and the pool drain and the volume still sits
/// at `EROFS` until a human runs `mount -o remount,rw`. Rule 3's
/// mitigation would have become the wedge §8.2 exists to exit.
#[tokio::test]
async fn a_restarted_agent_adopts_a_read_only_volume_and_restores_it() {
    let rig = Rig::new().await;
    rig.fill(15);

    // Engine 1 seeds a baseline, then rule 3 fires and it goes away.
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance).await.expect("seed");
    rig.fill(15);
    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("cycle fails");
    eng.run_cycle().await.expect_err("cycle fails");
    assert!(eng.degraded() && rig.stack.read_only());
    drop(eng);

    // Engine 2 — a fresh process over the same (still read-only) stack.
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
    assert!(!eng.degraded(), "a fresh engine starts with no latch");
    rig.cp.set(Mode::Pass);
    eng.recover()
        .await
        .expect("recovery reconciles the failed cycles");
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect("recovery advance");

    assert!(
        !rig.stack.read_only(),
        "the restarted agent must hand the volume back: {:?}",
        rig.stack.calls()
    );
    assert!(!eng.degraded());
    // And the ordering rule still holds across the restart.
    let calls = rig.stack.calls();
    let rw = calls.iter().rposition(|c| *c == "restore_rw").expect("rw");
    let reclaim = calls
        .iter()
        .rposition(|c| *c == "reclaim")
        .expect("a reclaim");
    assert!(reclaim < rw, "space came back first: {calls:?}");
}

/// The adoption one-shot must be spent on a definite ANSWER, not on
/// having asked. A probe that errors has told the engine nothing; if
/// that burned the check, a single transient would silently restore the
/// permanent-`EROFS` behaviour adoption exists to prevent — and it
/// would do so invisibly, since nothing downstream ever asks again.
#[tokio::test]
async fn an_unanswerable_probe_does_not_burn_the_adoption_check() {
    let rig = Rig::new().await;
    rig.fill(15);
    rig.stack
        .degrade_read_only()
        .expect("rule 3 fired pre-restart");
    rig.stack.set_ro_probe_fails(true);

    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);
    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance).await.expect("seed");
    assert!(!eng.degraded(), "nothing was learned, so nothing adopted");

    // The probe recovers. The next cycle must still be able to adopt.
    rig.stack.set_ro_probe_fails(false);
    rig.fill(15);
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect("second advance");
    assert!(
        !rig.stack.read_only(),
        "the volume came back once the probe could answer: {:?}",
        rig.stack.calls()
    );
}

/// The adoption must not fire on a volume that is read-only because
/// someone MEANT it to be. Rule 3 is withheld from `ro:` stages by the
/// CSI, so an unconfigured engine is the shape that reaches here — and
/// it must leave the mount exactly as it found it.
#[tokio::test]
async fn adoption_is_confined_to_volumes_rule_3_is_armed_for() {
    let rig = Rig::new().await;
    rig.fill(15);
    rig.stack.degrade_read_only().expect("staged read-only");

    // No `with_ro_watermark`: this is how the CSI configures a `ro:`
    // stage.
    let mut eng = rig.engine();
    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance).await.expect("advance");

    assert!(!eng.degraded(), "nothing to adopt without rule 3 armed");
    assert!(
        rig.stack.read_only(),
        "a deliberately read-only volume must not be remounted rw: {:?}",
        rig.stack.calls()
    );
    assert!(!rig.stack.calls().contains(&"restore_rw"));
}

/// The asymmetry that keeps step 6 honest: a publish ends the outage but
/// frees nothing, so it clears the clock and leaves the volume
/// read-only. Only space coming back licenses the restore.
#[tokio::test]
async fn a_successful_publish_ends_the_outage_but_not_the_degradation() {
    let rig = Rig::new().await;
    rig.fill(15);
    let mut eng = rig.engine().with_ro_watermark(0.9, NOW);

    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("cycle fails");
    eng.run_cycle().await.expect_err("cycle fails");
    assert!(eng.degraded() && rig.stack.read_only());

    // The network returns and the backlog publishes. No advance, so no
    // reclaim, so no space.
    rig.cp.set(Mode::Pass);
    eng.recover()
        .await
        .expect("recovery reconciles the failed cycles");
    eng.run_cycle_as(CycleKind::Publish)
        .await
        .expect("backlog publishes");
    assert!(
        eng.degraded() && rig.stack.read_only(),
        "a publish returns no pool blocks, so the writers stay stopped"
    );
    assert!(
        !rig.stack.calls().contains(&"restore_rw"),
        "{:?}",
        rig.stack.calls()
    );

    // And the clock did clear: were it still running, this next failing
    // cycle would be enough to re-fire rule 3 — but rule 3 is latched,
    // so what the assertion really pins is that the outage is over.
    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("cycle fails");
    let remounts = rig
        .stack
        .calls()
        .iter()
        .filter(|c| **c == "degrade_ro")
        .count();
    assert_eq!(remounts, 1, "{:?}", rig.stack.calls());
}

// -- resuming a candidate the outage stranded ---------------------------
//
// `resume_from_captured` dies at `cp.prepare`, the first control-plane
// call a cycle makes, so the ordinary outage leaves
// `phase = Captured, candidate = Some(..), prepared = None`. The planner
// has an arm for exactly that (`Continue(Phase::Captured)`, "Row 2"),
// but `run` consults the planner once at startup and then loops on
// `run_cycle`, which used to start unconditionally at capture. So the
// one state a partition produces every interval was the one state the
// loop could not act on.

/// The heal must publish what the outage already captured.
///
/// This is the whole point: on the previous behaviour the recovery cycle
/// re-captured into a name the stranded candidate still held, and the
/// cycle failed on the collision instead of publishing — for as long as
/// the volume lived.
#[tokio::test]
async fn a_healed_outage_publishes_the_candidate_it_already_captured() {
    let rig = Rig::new().await;
    rig.fill(4);
    let mut eng = rig.engine();

    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("the cut cycle fails");
    let s = eng.state().expect("state after the cut cycle");
    assert_eq!(s.phase, skynbd_agent::Phase::Captured);
    assert!(s.candidate.is_some(), "the candidate is stranded, not lost");
    assert!(s.prepared.is_none(), "nothing was ever prepared");
    let stranded = s.candidate.as_ref().unwrap().thin_dev_id;

    rig.cp.set(Mode::Pass);
    let target = eng.run_cycle().await.expect("the heal publishes");
    assert!(
        rig.cp.ref_exists(&target).await.unwrap(),
        "the generation the outage captured is the one that got published"
    );
    assert_ne!(stranded, 0);
}

/// A second cut cycle must not take a second snapshot.
///
/// Asserted through the ERROR rather than a counter, because the error
/// is the part that misled: re-capturing collided with the stranded
/// candidate, so the cycle reported a local stack fault where the truth
/// was still a partition. An outage that reports itself as something
/// else is worse than the wasted freeze.
#[tokio::test]
async fn a_cut_cycle_does_not_re_capture_over_the_stranded_candidate() {
    let rig = Rig::new().await;
    rig.fill(4);
    let mut eng = rig.engine();

    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("the cut cycle fails");
    for _ in 0..3 {
        let e = eng.run_cycle().await.expect_err("still cut, still failing");
        let msg = e.to_string();
        assert!(
            msg.contains("connection refused"),
            "a cut cycle must keep failing as a PARTITION, not as a local \
             snapshot-name collision; got: {msg}"
        );
    }
    assert_eq!(
        rig.stack.observe().candidates.len(),
        1,
        "exactly one candidate across the whole outage"
    );
}

/// The durable intent wins over the kind this cycle would have chosen.
///
/// `s.advancing` is persisted at capture so a crash resumes the right
/// promotion tail. An in-process resume that re-derived the kind would
/// be a second, undocumented answer to the same question — so an advance
/// stranded by an outage stays an advance, even when the resuming cycle
/// is asked for a publish.
#[tokio::test]
async fn a_resumed_cycle_keeps_the_advance_intent_it_persisted() {
    let rig = Rig::new().await;
    rig.fill(4);
    let mut eng = rig.engine();

    rig.cp.set(Mode::Down);
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect_err("the cut advance fails");
    assert!(
        eng.state().unwrap().advancing,
        "the advance intent is durable from capture"
    );

    rig.cp.set(Mode::Pass);
    let target = eng
        .run_cycle_as(CycleKind::Publish)
        .await
        .expect("the heal publishes");
    assert_eq!(
        rig.cp.origin_served().await.unwrap().as_deref(),
        Some(target.as_str()),
        "the ADVANCE tail ran — the resumed cycle honoured the persisted \
         intent rather than the kind it was called with (Publish)"
    );
}

/// A record without a device is not resumable.
///
/// The state says `Captured`, but a sweep may have removed the
/// activation since. Resuming onto a name that no longer resolves would
/// trade a wasted freeze for a failed cycle, so the guard is the same
/// local observation the planner uses.
#[tokio::test]
async fn a_stranded_candidate_that_vanished_falls_back_to_a_fresh_capture() {
    let rig = Rig::new().await;
    rig.fill(4);
    let mut eng = rig.engine();

    rig.cp.set(Mode::Down);
    eng.run_cycle().await.expect_err("the cut cycle fails");
    let cand = eng
        .state()
        .unwrap()
        .candidate
        .as_ref()
        .unwrap()
        .lv_name
        .clone();
    rig.stack.snap_delete(&cand).expect("sweep removes it");

    rig.cp.set(Mode::Pass);
    eng.run_cycle()
        .await
        .expect("a fresh capture, since there is nothing to resume");
}

/// Pressure that arrived DURING the outage must still be able to
/// promote a stranded publish.
#[tokio::test]
async fn pressure_arriving_during_the_outage_promotes_a_stranded_publish() {
    let rig = Rig::new().await;
    rig.fill(4);
    let mut eng = rig.engine();

    rig.cp.set(Mode::Down);
    eng.run_cycle_as(CycleKind::Publish)
        .await
        .expect_err("the cut publish fails");
    assert!(
        !eng.state().unwrap().advancing,
        "stranded as a PUBLISH: the pool was quiet when the outage began"
    );

    rig.fill(12); // the pool crosses the watermark DURING the outage

    rig.cp.set(Mode::Pass);
    let target = eng
        .run_cycle_as(CycleKind::Advance)
        .await
        .expect("the heal publishes");
    assert_eq!(
        rig.cp.origin_served().await.unwrap().as_deref(),
        Some(target.as_str()),
        "the recovery cycle must ADVANCE"
    );
}

/// A resumed advance must not reclaim what was written after the strand.
///
/// The candidate the resume publishes is stale by an outage's worth of
/// writes, and the advance repoints the origin to it and then unmaps
/// what the origin now serves. If that set were derived from the
/// candidate's extent rather than from its CONTENT, the reclaim would
/// unmap blocks the origin cannot serve and the delta the runbook
/// exists to protect would be gone.
///
/// It is not: `is_shared` compares content, and `DmStack` recomputes the
/// shared set from thin mappings inside each freeze window. So the
/// staleness costs reclaim YIELD and nothing else. Asserted rather than
/// argued, because the fresh-capture path never had this gap.
#[tokio::test]
async fn a_resumed_advance_does_not_reclaim_what_was_written_after_the_strand() {
    let rig = Rig::new().await;
    rig.fill(4); // chunks 0..3, captured by the cycle the outage strands
    let mut eng = rig.engine();

    rig.cp.set(Mode::Down);
    eng.run_cycle_as(CycleKind::Publish)
        .await
        .expect_err("the cut publish fails");

    // Written DURING the outage: the stranded candidate does not hold
    // these, so the repointed origin cannot serve them.
    for c in 4..8u64 {
        rig.stack
            .write_origin(c * CHUNK, &vec![0xAA; CHUNK as usize]);
    }
    let before = rig.stack.pressure().expect("pool sized").used_bytes;

    rig.cp.set(Mode::Pass);
    eng.run_cycle_as(CycleKind::Advance)
        .await
        .expect("the heal advances");

    let after = rig.stack.pressure().expect("pool sized").used_bytes;
    assert_eq!(
        before - after,
        4 * CHUNK,
        "exactly the four pre-strand chunks come back; the four written \
         during the outage are not shared with the published generation \
         and must stay mapped"
    );
}

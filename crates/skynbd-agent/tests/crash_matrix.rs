//! V3.2 §21.1 crash-matrix fault injection as ordinary `cargo test`
//! (agent design doc §5, §13).
//!
//! For every crash point in bootstrap + two incremental cycles: kill the
//! engine at that point, "restart" (fresh `CycleEngine` over the surviving
//! pool/plane/state-dir), run recovery, and assert the §21.1 postconditions:
//!
//! - exactly one valid baseline pair survives or is reconstructed,
//! - no visible snapshot is corrupted or partially published,
//! - recovery converges without manual repair,
//! - no orphaned prepared roots, pins, or snapshot LVs.

use skynbd_agent::{
    AgentError, ControlPlane, CycleEngine, CycleKind, FakeStack, InprocControlPlane, LocalStack,
    Phase, ReclaimPolicy, ReclaimStrategy, StateStore,
};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;

type Cp = InprocControlPlane<MemoryStore, MemoryRefStore>;

struct Rig {
    stack: FakeStack,
    cp: Cp,
    refs: MemoryRefStore,
    vol: VolumeId,
    dir: tempfile::TempDir,
    /// Force every advance through the swing executor (the §7 crash
    /// arms — armed persist, flip, post-flip sweep — join the matrix).
    swing: bool,
    /// Backfill upload batch depth (issue #97). >1 puts a whole window
    /// of ops in flight at every backfill crash point.
    depth: usize,
    /// Send mixed batches as one `apply` (false: one call per same-kind
    /// run, as against a server without `apply`).
    mixed_apply: bool,
}

impl Rig {
    async fn new_full(swing: bool, depth: usize) -> Self {
        let stack = FakeStack::new(VOL, CHUNK);
        stack.set_swing_supported(swing);
        let refs = MemoryRefStore::new();
        let vol = VolumeId(ulid::Ulid::new());
        let cp = InprocControlPlane::create_volume(MemoryStore::new(), refs.clone(), vol, VOL)
            .await
            .unwrap();
        Rig {
            stack,
            cp,
            refs,
            vol,
            dir: tempfile::tempdir().unwrap(),
            swing,
            depth,
            mixed_apply: true,
        }
    }

    fn engine(&self) -> CycleEngine<FakeStack, Cp> {
        let eng = CycleEngine::open(
            self.stack.clone(),
            self.cp.clone(),
            StateStore::new(self.dir.path()),
            "vlin",
        )
        .unwrap()
        .with_publish_pipeline_depth(self.depth)
        .with_mixed_apply(self.mixed_apply);
        if self.swing {
            eng.with_policy(ReclaimPolicy {
                strategy: ReclaimStrategy::Swing,
                ..ReclaimPolicy::default()
            })
        } else {
            eng
        }
    }

    fn mutate(&self, phase: u8) {
        let p = |len: usize, seed: u8| -> Vec<u8> {
            (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
        };
        match phase {
            0 => {
                self.stack.write_origin(0, &p(3 * CHUNK as usize, 1));
                self.stack.write_origin(9 * CHUNK, &p(CHUNK as usize, 7));
            }
            1 => {
                // Delta: data 2, 3, clear 9, data 12 — at depth 3 a mixed
                // batch followed by another batch in the same cycle.
                self.stack
                    .write_origin(2 * CHUNK, &p(2 * CHUNK as usize, 21));
                self.stack.discard_origin(9 * CHUNK, CHUNK);
                self.stack.write_origin(12 * CHUNK, &p(CHUNK as usize, 33));
            }
            _ => {
                self.stack
                    .write_origin(5 * CHUNK, &p(3 * CHUNK as usize, 91));
            }
        }
    }

    /// §21.1 postconditions after convergence.
    async fn assert_converged(&self, eng: &CycleEngine<FakeStack, Cp>) {
        let s = eng.state().expect("state exists after convergence");
        assert_eq!(s.phase, Phase::Idle, "quiescent");
        assert!(s.candidate.is_none() && s.prepared.is_none() && s.cursor.is_none());

        match s.baseline.as_ref() {
            Some(b) => {
                // Remote pair member visible + pinned by the baseline holder.
                assert!(self.cp.ref_exists(&b.remote).await.unwrap());
                assert!(self
                    .cp
                    .pinned_by(&b.remote, "agent-baseline:vlin")
                    .await
                    .unwrap());
            }
            None => {
                // Legitimate only if the crash predated any publish: the
                // volume is exactly as it was pre-bootstrap (row 1).
                assert!(
                    !self.cp.ref_exists("snap/g0").await.unwrap(),
                    "published generation exists but no baseline pair"
                );
            }
        }

        // No orphaned prepared roots.
        assert!(self.cp.find_prepared().await.unwrap().is_none());

        // Local: at most the baseline LV (a rebaselined pair has none yet).
        let obs = self.stack.observe();
        assert!(obs.candidates.is_empty(), "no candidate LVs leak");
        assert!(obs.baselines.len() <= 1, "no duplicate baselines");
        assert!(
            self.stack.snap_count() <= 1,
            "no untagged snapshot LVs leak"
        );

        // Every published generation is bit-exact vs its recorded image.
    }
}

/// Run one scripted history — bootstrap, an ADVANCE cycle (the §3 tail
/// with its own seams: pin, repoint, unpin, fence, reclaim, rebaseline),
/// then a publish cycle — crashing at `at`; after the crash, recover
/// with a fresh engine and finish the remaining cycles crash-free.
/// Returns total crash points when `at` is None. With `swing` the
/// advance's reclaim runs the swing executor instead of the port.
async fn scripted_run_with(at: Option<u32>, swing: bool) -> (u32, bool) {
    scripted_run_full(at, swing, 1).await
}

async fn scripted_run_full(at: Option<u32>, swing: bool, depth: usize) -> (u32, bool) {
    scripted_run_apply(at, swing, depth, true).await
}

async fn scripted_run_apply(
    at: Option<u32>,
    swing: bool,
    depth: usize,
    mixed_apply: bool,
) -> (u32, bool) {
    let mut rig = Rig::new_full(swing, depth).await;
    rig.mixed_apply = mixed_apply;
    let rig = rig;
    let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
    let mut crashed = false;

    let mut eng = rig.engine();
    eng.crash.at = at;

    for phase in 0u8..3 {
        rig.mutate(phase);
        let img = rig.stack.origin_image();
        let kind = if phase == 1 {
            CycleKind::Advance
        } else {
            CycleKind::Publish
        };
        match eng.run_cycle_as(kind).await {
            Ok(target) => expected.push((target, img)),
            Err(AgentError::InjectedCrash(_)) => {
                crashed = true;
                break;
            }
            Err(e) => panic!("unexpected error mid-cycle: {e}"),
        }
    }
    let total = eng.crash.counter;

    if crashed {
        // Restart: fresh engine, no crash injection, recover then finish.
        let mut eng2 = rig.engine();
        eng2.recover().await.expect("recovery converges");
        rig.assert_converged(&eng2).await;

        // The interrupted generation either published (content == image at
        // capture time) or vanished without trace; either way the *next*
        // cycles must succeed and be exact.
        for phase in 0u8..2 {
            rig.mutate(phase + 10);
            let img = rig.stack.origin_image();
            let t = eng2.run_cycle().await.expect("post-recovery cycle");
            expected.push((t, img));
        }
        rig.assert_converged(&eng2).await;
    }

    // Bit-exact check for every generation we recorded an image for.
    for (target, img) in &expected {
        let got = rig.cp.read_snapshot(target, 0, VOL).await.unwrap();
        assert_eq!(&got, img, "generation {target} corrupted");
    }

    // Pin accounting: only the newest generation holds the baseline pin.
    // The advance target additionally keeps the `origin` pin for as long
    // as the pointer names it (Invariant D) — that one is not stale.
    if let Some((last, _)) = expected.last() {
        for (t, _) in &expected {
            let mut pins = rig.refs.pins(rig.vol, t).await.unwrap();
            pins.retain(|h| h != "origin");
            if t == last {
                assert_eq!(pins, vec!["agent-baseline:vlin".to_string()]);
            } else {
                assert!(pins.is_empty(), "stale pin on {t}: {pins:?}");
            }
        }
    }
    (total, crashed)
}

async fn scripted_run(at: Option<u32>) -> (u32, bool) {
    scripted_run_with(at, false).await
}

/// The full matrix: every crash point in the scripted history.
#[tokio::test(flavor = "multi_thread")]
async fn crash_at_every_point_converges() {
    let (total, crashed) = scripted_run(None).await;
    assert!(!crashed);
    assert!(total > 30, "matrix should be dense, got {total} points");

    for k in 0..total {
        let (_, crashed) = scripted_run(Some(k)).await;
        assert!(crashed, "point {k} never fired");
    }
}

/// The same matrix with the advance forced through the swing executor:
/// the §7 slot bracket (armed persist, in-window flip, post-flip
/// recovery sweep) adds crash arms, and every one must converge.
#[tokio::test(flavor = "multi_thread")]
async fn crash_at_every_point_converges_with_swing() {
    let (port_total, _) = scripted_run_with(None, false).await;
    let (total, crashed) = scripted_run_with(None, true).await;
    assert!(!crashed);
    assert!(
        total > port_total,
        "the swing arms must grow the matrix ({total} vs {port_total})"
    );

    for k in 0..total {
        let (_, crashed) = scripted_run_with(Some(k), true).await;
        assert!(crashed, "point {k} never fired");
    }
}

/// The matrix with the backfill pipelined (issue #97): every crash point
/// now lands with a batch in flight — objects uploaded but not attached.
/// A mixed batch is one `apply` call, so no point falls inside a batch;
/// the no-`apply` variant below covers the seams between split runs.
/// Resume must restart from the durable watermark, and the re-uploads
/// must be harmless (content addressing makes them `ensure` hits).
/// Batching removes per-op seams, so the matrix gets SMALLER, and must
/// still fire at every remaining point.
#[tokio::test(flavor = "multi_thread")]
async fn crash_at_every_point_converges_pipelined() {
    let (serial_total, _) = scripted_run_full(None, false, 1).await;
    for depth in [2usize, 4, 64] {
        let (total, crashed) = scripted_run_full(None, false, depth).await;
        assert!(!crashed);
        assert!(
            total < serial_total,
            "depth {depth}: batching must merge per-op seams ({total} vs {serial_total})"
        );
        for k in 0..total {
            let (_, crashed) = scripted_run_full(Some(k), false, depth).await;
            assert!(crashed, "depth {depth}: point {k} never fired");
        }
    }
}

/// The pipelined matrix against a server without `apply`: phase 1's
/// mixed batch (data chunks 2, 3 then a clear of chunk 9 — one batch at
/// depth 3, followed by a batch for chunk 12; all one batch at 64)
/// splits into a data run and a clear run, with a crash point between
/// them. At depth 3 the split batch attaches beside the overlapped
/// staging of the chunk-12 batch, so those seams are crossed too. They
/// must exist — the split matrix is LARGER than the `apply` one — and
/// recovery must converge from every point.
#[tokio::test(flavor = "multi_thread")]
async fn crash_at_every_point_converges_pipelined_without_apply() {
    for depth in [3usize, 64] {
        let (with_apply, _) = scripted_run_apply(None, false, depth, true).await;
        let (split, crashed) = scripted_run_apply(None, false, depth, false).await;
        assert!(!crashed);
        assert!(
            split > with_apply,
            "depth {depth}: no seam between a split batch's runs ({split} vs {with_apply})"
        );
        for k in 0..split {
            let (_, crashed) = scripted_run_apply(Some(k), false, depth, false).await;
            assert!(crashed, "depth {depth}: point {k} never fired");
        }
    }
}

/// Server-crash variant (S13 shape): kill the *server's* live sessions
/// mid-backfill — staged ops above the durable watermark vanish — then let
/// the agent resume from the watermark. Run with and without `apply`:
/// split, a batch's runs checkpoint separately, so the watermark can sit
/// between them.
#[tokio::test(flavor = "multi_thread")]
async fn server_crash_mid_backfill_resumes_from_watermark() {
    let cases = [(1usize, 8u32), (1, 12), (1, 16), (4, 4), (4, 5), (4, 6)];
    // Depth 1 never forms a mixed batch, so its no-apply runs would only
    // repeat the apply ones.
    for (mixed_apply, (depth, agent_crash_at)) in [true, false]
        .into_iter()
        .flat_map(|a| cases.into_iter().map(move |c| (a, c)))
        .filter(|&(a, (depth, _))| a || depth > 1)
    {
        let mut rig = Rig::new_full(false, depth).await;
        rig.mixed_apply = mixed_apply;
        let mut eng = rig.engine();
        rig.mutate(0);
        eng.run_cycle().await.unwrap();

        rig.mutate(1);
        let img = rig.stack.origin_image();
        let mut eng2 = rig.engine();
        eng2.crash.at = Some(agent_crash_at);
        match eng2.run_cycle().await {
            Err(AgentError::InjectedCrash(_)) => {
                // Both sides down: server loses non-durable staged ops.
                rig.cp.drop_sessions().await;
                let mut eng3 = rig.engine();
                eng3.recover().await.expect("recovery converges");
                rig.assert_converged(&eng3).await;
                if rig.cp.ref_exists("snap/g1").await.unwrap() {
                    assert_eq!(rig.cp.read_snapshot("snap/g1", 0, VOL).await.unwrap(), img);
                } else {
                    // Interrupted generation abandoned; next cycle covers it.
                    rig.mutate(2);
                    let img2 = rig.stack.origin_image();
                    let t = eng3.run_cycle().await.unwrap();
                    assert_eq!(rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img2);
                }
            }
            Ok(_) => {
                // Crash point landed past this cycle's ops — fine at depth
                // 1, whose matrix already covers those indices. The
                // pipelined cases exist only to crash mid-backfill, so for
                // them a clean finish means they stopped testing anything.
                assert_eq!(
                    depth, 1,
                    "depth {depth} apply={mixed_apply}: crash point {agent_crash_at} \
                     no longer lands in the backfill"
                );
            }
            Err(e) => panic!("unexpected: {e}"),
        }
    }
}

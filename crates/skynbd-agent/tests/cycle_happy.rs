//! Happy-path V3.2 loop over FakeStack + in-process control plane
//! (agent design doc §8): bootstrap `g0`, two incremental cycles with
//! writes and discards, full-content equality per generation, pin
//! lifecycle per §15.4.

use skynbd_agent::{ControlPlane, CycleEngine, FakeStack, LocalStack, Phase, StateStore};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 32 * CHUNK;

async fn setup() -> (
    FakeStack,
    skynbd_agent::InprocControlPlane<MemoryStore, MemoryRefStore>,
    MemoryRefStore,
    VolumeId,
) {
    let stack = FakeStack::new(VOL, CHUNK);
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let cp =
        skynbd_agent::InprocControlPlane::create_volume(MemoryStore::new(), refs.clone(), vol, VOL)
            .await
            .unwrap();
    (stack, cp, refs, vol)
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

#[tokio::test]
async fn bootstrap_and_two_cycles_bit_exact() {
    let (stack, cp, refs, vol) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let store = StateStore::new(dir.path());
    let mut eng = CycleEngine::open(stack.clone(), cp.clone(), store, "vlin").unwrap();

    // g0: bootstrap.
    stack.write_origin(0, &pattern(3 * CHUNK as usize, 1));
    stack.write_origin(10 * CHUNK, &pattern(CHUNK as usize, 2));
    let g0_expect = stack.origin_image();
    let t0 = eng.run_cycle().await.unwrap();
    assert_eq!(t0, "snap/g0");
    assert_eq!(
        cp.read_snapshot("snap/g0", 0, VOL).await.unwrap(),
        g0_expect
    );

    // State converged to a baseline pair.
    let s = eng.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.baseline.as_ref().unwrap().remote, "snap/g0");
    assert!(s.candidate.is_none() && s.prepared.is_none());
    assert_eq!(stack.snap_count(), 1, "only the baseline LV survives");

    // Publish pin transferred to the baseline holder (§15.4).
    let pins = refs.pins(vol, "snap/g0").await.unwrap();
    assert_eq!(pins, vec!["agent-baseline:vlin".to_string()]);

    // g1: mutate + discard.
    stack.write_origin(5 * CHUNK, &pattern(2 * CHUNK as usize, 9));
    stack.discard_origin(10 * CHUNK, CHUNK);
    let g1_expect = stack.origin_image();
    let t1 = eng.run_cycle().await.unwrap();
    assert_eq!(t1, "snap/g1");
    assert_eq!(
        cp.read_snapshot("snap/g1", 0, VOL).await.unwrap(),
        g1_expect
    );
    assert_eq!(
        cp.read_snapshot("snap/g0", 0, VOL).await.unwrap(),
        g0_expect
    );

    // Old baseline unpinned; new baseline pinned; old LV deleted.
    assert!(refs.pins(vol, "snap/g0").await.unwrap().is_empty());
    assert_eq!(
        refs.pins(vol, "snap/g1").await.unwrap(),
        vec!["agent-baseline:vlin".to_string()]
    );
    assert_eq!(stack.snap_count(), 1);

    // g2: heavy churn.
    stack.write_origin(0, &pattern(VOL as usize / 2, 33));
    let g2_expect = stack.origin_image();
    eng.run_cycle().await.unwrap();
    assert_eq!(
        cp.read_snapshot("snap/g2", 0, VOL).await.unwrap(),
        g2_expect
    );
    assert!(
        cp.find_prepared().await.unwrap().is_none(),
        "nothing in flight"
    );
}

/// Capture failure (freeze fault): baseline untouched, next cycle succeeds.
#[tokio::test]
async fn capture_failure_keeps_baseline() {
    let (stack, cp, _refs, _vol) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap();

    stack.write_origin(0, &pattern(CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();

    stack.fail_next_captures(1);
    stack.write_origin(CHUNK, &pattern(CHUNK as usize, 2));
    assert!(eng.run_cycle().await.is_err());
    assert_eq!(eng.state().unwrap().phase, Phase::Idle);
    assert_eq!(stack.snap_count(), 1, "no partial candidate leaked");

    let expect = stack.origin_image();
    eng.run_cycle().await.unwrap();
    assert_eq!(cp.read_snapshot("snap/g1", 0, VOL).await.unwrap(), expect);
}

/// Policy abort mid-flight: prepared root gone, candidate deleted,
/// baseline pair intact, next cycle publishes cleanly.
#[tokio::test]
async fn abort_in_flight_converges() {
    let (stack, cp, _refs, _vol) = setup().await;
    let dir = tempfile::tempdir().unwrap();
    let mut eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap();
    stack.write_origin(0, &pattern(CHUNK as usize, 1));
    eng.run_cycle().await.unwrap();

    // Fake an in-flight cycle: capture + prepare, then abort.
    stack.write_origin(CHUNK, &pattern(CHUNK as usize, 5));
    let _ = stack.capture("skynbd-vlin-snap-g1").unwrap();
    stack
        .set_role(
            "skynbd-vlin-snap-g1",
            Some(skynbd_agent::SnapRole::Candidate),
        )
        .unwrap();
    cp.prepare(Some("snap/g0")).await.unwrap();
    eng.abort_in_flight().await.unwrap();

    assert!(cp.find_prepared().await.unwrap().is_none());
    assert_eq!(stack.snap_count(), 1);
    assert_eq!(eng.state().unwrap().phase, Phase::Idle);

    let expect = stack.origin_image();
    eng.run_cycle().await.unwrap();
    assert_eq!(cp.read_snapshot("snap/g1", 0, VOL).await.unwrap(), expect);
}

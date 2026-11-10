//! D-M11 seeded rebaseline: a thin volume rescheduled onto a fresh pool
//! reads through an external origin (clone of the newest published
//! generation) and its agent state is pre-seeded with a rebaselined
//! pair on that same generation. This test proves the engine-side
//! contract: from the seeded shape — `baseline.remote = snap/gN`,
//! **empty** local member — the next cycle prepares on `gN` and uploads
//! only the locally provisioned delta, publishing a generation that
//! composes base content with local writes.
//!
//! The seeded record's shape must match what the CSI driver's
//! `DmCycle::seed_baseline` writes.

use skynbd_agent::{
    AgentVolumeState, BaselinePair, ControlPlane, CycleEngine, FakeStack, LocalSnap, Phase,
    StateStore,
};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 32 * CHUNK;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

#[tokio::test]
async fn fresh_pool_cycle_composes_base_and_local_delta() {
    // Shared server state: object store + refs + one lineage volume.
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let cp =
        skynbd_agent::InprocControlPlane::create_volume(MemoryStore::new(), refs.clone(), vol, VOL)
            .await
            .unwrap();

    // Node A: bootstrap g0 with X at chunk 5 — content that node B will
    // NEVER hold locally (its pool reads it through the origin clone).
    let x = pattern(CHUNK as usize, 0x55);
    {
        let stack_a = FakeStack::new(VOL, CHUNK);
        let dir_a = tempfile::tempdir().unwrap();
        let mut eng_a = CycleEngine::open(
            stack_a.clone(),
            cp.clone(),
            StateStore::new(dir_a.path()),
            "vlin",
        )
        .unwrap();
        stack_a.write_origin(5 * CHUNK, &x);
        assert_eq!(eng_a.run_cycle().await.unwrap(), "snap/g0");
    } // node A gone: its stack, state dir and baseline LV die with it.

    // Node B: fresh stack (fresh pool — locally everything reads as
    // unwritten), agent state seeded exactly as DmCycle::seed_baseline
    // writes it after cloning g0 as the external origin.
    let stack_b = FakeStack::new(VOL, CHUNK);
    let dir_b = tempfile::tempdir().unwrap();
    let store_b = StateStore::new(dir_b.path());
    store_b
        .save(&AgentVolumeState {
            volume: "vlin".into(),
            phase: Phase::Idle,
            baseline: Some(BaselinePair {
                local: LocalSnap {
                    lv_name: String::new(),
                    thin_dev_id: 0,
                    pool_tx_id: 0,
                    created_at_ms: 0,
                },
                remote: "snap/g0".into(),
                pin_holder: "agent-baseline:vlin".into(),
            }),
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
        })
        .unwrap();
    let mut eng_b = CycleEngine::open(stack_b.clone(), cp.clone(), store_b, "vlin").unwrap();

    // Recovery converges on the seeded baseline (g0 is visible and
    // still pinned by the baseline holder — node A's promote pinned it,
    // and the holder name is volume-scoped, not node-scoped).
    eng_b.recover().await.unwrap();
    let s = eng_b.state().unwrap();
    assert_eq!(s.phase, Phase::Idle);
    assert_eq!(s.baseline.as_ref().unwrap().remote, "snap/g0");

    // Local writes cover the verify-sampled chunks (first, middle,
    // last: the engine reads the candidate locally, and on a real thin
    // device unsampled unprovisioned chunks read through the origin).
    let y0 = pattern(CHUNK as usize, 0x01);
    let ym = pattern(CHUNK as usize, 0x02);
    let yl = pattern(CHUNK as usize, 0x03);
    stack_b.write_origin(0, &y0);
    stack_b.write_origin((VOL / CHUNK / 2) * CHUNK, &ym);
    stack_b.write_origin(VOL - CHUNK, &yl);

    // The seeded pair has no local member → the delta is
    // full_mapped(candidate) = exactly the locally provisioned blocks,
    // prepared on base g0 — so g1 = g0 ⊕ local writes.
    assert_eq!(eng_b.run_cycle().await.unwrap(), "snap/g1");

    let read = |off: u64| cp.read_snapshot("snap/g1", off, CHUNK);
    assert_eq!(read(0).await.unwrap(), y0, "local write at first chunk");
    assert_eq!(
        read(5 * CHUNK).await.unwrap(),
        x,
        "base content inherited from g0 — the block node B never wrote"
    );
    assert_eq!(
        read(VOL - CHUNK).await.unwrap(),
        yl,
        "local write at last chunk"
    );
    // g0 is untouched history.
    assert_eq!(
        cp.read_snapshot("snap/g0", 0, CHUNK).await.unwrap(),
        vec![0u8; CHUNK as usize],
        "g0's first chunk was never written by node A"
    );

    // Baseline advanced: g1 pinned by the baseline holder, g0 released.
    let s = eng_b.state().unwrap();
    assert_eq!(s.baseline.as_ref().unwrap().remote, "snap/g1");
    assert!(refs
        .pins(vol, "snap/g1")
        .await
        .unwrap()
        .contains(&"agent-baseline:vlin".to_string()));
    assert!(refs.pins(vol, "snap/g0").await.unwrap().is_empty());
}

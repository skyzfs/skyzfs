//! The full production pipeline, rootless: the REAL `DmStack`
//! orchestration over the contract-enforcing mock kernel, driving the
//! REAL cycle engine against the REAL server engine (in-proc control
//! plane). Before the M9 seam this composition only ran as the S14a
//! nightly; now every PR proves bootstrap + incremental cycles end
//! bit-exact and converged.

use skynbd_agent::dm::{DmConfig, DmStack};
use skynbd_agent::dm_mock::MockDmOps;
use skynbd_agent::{ControlPlane, CycleEngine, InprocControlPlane, LocalStack, Phase, StateStore};
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;
const POOL: &str = "skypool";
const ORIGIN: &str = "origin";

type Cp = InprocControlPlane<MemoryStore, MemoryRefStore>;

fn pat(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn dm_stack_bootstrap_and_cycles_bit_exact() {
    let dir = tempfile::tempdir().unwrap();
    let ops = MockDmOps::new(POOL, ORIGIN, 1, VOL, CHUNK);
    let stack = DmStack::new(
        DmConfig {
            pool_name: POOL.into(),
            origin_name: ORIGIN.into(),
            origin_dev_id: 1,
            metadata_dev: dir.path().join("tmeta"),
            mountpoint: dir.path().to_path_buf(),
            volume: "vlin".into(),
            size_bytes: VOL,
            chunk_bytes: CHUNK,
            ids_path: dir.path().join("vlin.devids"),
            freeze_budget_ms: 5_000,
            discard_name: None,
            backing_file: None,
            shim_name: None,
            mount_name: None,
            slot: None,
        },
        ops.clone(),
    )
    .unwrap();
    let cp = Cp::create_volume(
        MemoryStore::new(),
        MemoryRefStore::new(),
        VolumeId(ulid::Ulid::new()),
        VOL,
    )
    .await
    .unwrap();
    let mut eng =
        CycleEngine::open(stack, cp.clone(), StateStore::new(dir.path()), "vlin").unwrap();

    for phase in 0u8..3 {
        match phase {
            0 => {
                ops.write_origin(ORIGIN, 0, &pat(3 * CHUNK as usize, 1));
                ops.write_origin(ORIGIN, 9 * CHUNK, &pat(CHUNK as usize, 7));
            }
            1 => {
                ops.write_origin(ORIGIN, 2 * CHUNK, &pat(2 * CHUNK as usize, 21));
                ops.discard_origin(ORIGIN, 9 * CHUNK, CHUNK);
            }
            _ => {
                ops.write_origin(ORIGIN, 5 * CHUNK, &pat(3 * CHUNK as usize, 91));
            }
        }
        let img = ops.origin_image(ORIGIN);
        let target = eng.run_cycle().await.expect("cycle");
        assert_eq!(
            cp.read_snapshot(&target, 0, VOL).await.unwrap(),
            img,
            "generation {target} bit-exact"
        );
    }

    // Converged: idle state, one baseline pair, no leaks — the §21.1
    // postconditions, now with the production local stack.
    let s = eng.state().expect("state");
    assert_eq!(s.phase, Phase::Idle);
    assert!(s.candidate.is_none() && s.prepared.is_none());
    let b = s.baseline.as_ref().expect("baseline pair");
    assert!(cp.ref_exists(&b.remote).await.unwrap());
    assert!(cp
        .pinned_by(&b.remote, "agent-baseline:vlin")
        .await
        .unwrap());
    assert!(cp.find_prepared().await.unwrap().is_none());

    let obs = eng.stack.observe();
    assert!(obs.candidates.is_empty(), "no candidate devices leak");
    assert_eq!(obs.baselines.len(), 1, "exactly one local baseline");
    // Pool: origin + one baseline snapshot; dm table: pool + origin + baseline.
    assert_eq!(ops.thin_ids().len(), 2, "no leaked thin devs");
    assert_eq!(ops.device_names().len(), 3, "no leaked dm devices");
    assert!(!ops.frozen());
    assert!(!ops.meta_snap_reserved(), "no leaked reservation");
}

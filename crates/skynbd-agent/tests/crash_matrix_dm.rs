//! V3.2 §21.1 crash matrix with the PRODUCTION local stack: every crash
//! point in bootstrap + two cycles, recovered by a fresh `DmStack` over
//! the *surviving* mock kernel (pool, dm table, freeze state persist —
//! exactly what an agent restart sees on a real host).
//!
//! This is the composition the FakeStack matrix cannot check: recovery's
//! adopt/rename/delete decisions run against dm naming semantics and the
//! mock's kernel contracts (suspend ordering, EEXIST retry, reservation
//! pairing, activation-before-read).
//!
//! Two arms: a publish-only history over an unmanaged pool, and a
//! slot-managed one whose advance reclaims through the §7 swing — the
//! slot bracket's crash windows against those same contracts, with the
//! cycles after the swing running on the canonicalized pool.

use skynbd_agent::dm::{DmConfig, DmStack, SlotConfig};
use skynbd_agent::dm_mock::{MockDmOps, MockEvent};
use skynbd_agent::{
    AgentError, ControlPlane, CycleEngine, CycleKind, DmOps, InprocControlPlane, LocalStack, Phase,
    ReclaimPolicy, ReclaimStrategy, StateStore,
};
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;
const META: u64 = 4 * CHUNK;
const POOL: &str = "skypool";
const ORIGIN: &str = "origin";
const SHIM: &str = "shim";
const PORT: &str = "rec";
const EXT: &str = "/dev/nbd9";

type Cp = InprocControlPlane<MemoryStore, MemoryRefStore>;

struct Rig {
    ops: MockDmOps,
    cp: Cp,
    dir: tempfile::TempDir,
    /// Slot-managed stack + swing strategy: the advance's reclaim runs
    /// the §7 executor, so its crash arms join the matrix.
    swing: bool,
}

impl Rig {
    async fn new_with(swing: bool) -> Self {
        let ops = MockDmOps::new(POOL, ORIGIN, 1, VOL, CHUNK);
        let dir = tempfile::tempdir().unwrap();
        if swing {
            // The slot shape a slot-managed stage leaves behind: the shim
            // the filesystem claims and the unclaimed discard port over
            // the origin, plus the attached, preallocated pool file.
            let i = ops.device_info(ORIGIN).unwrap();
            let origin_mm = format!("{}:{}", i.major, i.minor);
            for (name, part) in [(SHIM, "shim"), (PORT, "rec")] {
                ops.create_device(
                    name,
                    &format!("SKYNBD-vlin-stack-{part}"),
                    &[(0, VOL / 512, "linear".into(), format!("{origin_mm} 0"))],
                )
                .unwrap();
            }
            ops.prepare_backing(&dir.path().join("pool.img"), META + VOL, true)
                .unwrap();
        }
        let cp = Cp::create_volume(
            MemoryStore::new(),
            MemoryRefStore::new(),
            VolumeId(ulid::Ulid::new()),
            VOL,
        )
        .await
        .unwrap();
        Rig {
            ops,
            cp,
            dir,
            swing,
        }
    }

    async fn new() -> Self {
        Self::new_with(false).await
    }

    /// An agent (re)start: a brand-new `DmStack` over the same kernel.
    fn engine(&self) -> CycleEngine<DmStack<MockDmOps>, Cp> {
        let slot = self.swing.then(|| SlotConfig {
            live_file: self.dir.path().join("pool.img"),
            data_bytes: VOL,
            meta_bytes: META,
            shim_name: SHIM.into(),
            external_origin: Some(EXT.into()),
            tmeta_name: None,
            data_name: None,
        });
        let stack = DmStack::new(
            DmConfig {
                pool_name: POOL.into(),
                origin_name: ORIGIN.into(),
                origin_dev_id: 1,
                metadata_dev: self.dir.path().join("tmeta"),
                mountpoint: self.dir.path().to_path_buf(),
                volume: "vlin".into(),
                size_bytes: VOL,
                chunk_bytes: CHUNK,
                ids_path: self.dir.path().join("vlin.devids"),
                freeze_budget_ms: 5_000,
                discard_name: self.swing.then(|| PORT.to_string()),
                backing_file: None,
                shim_name: None,
                mount_name: None,
                slot,
            },
            self.ops.clone(),
        )
        .unwrap();
        let eng = CycleEngine::open(
            stack,
            self.cp.clone(),
            StateStore::new(self.dir.path()),
            "vlin",
        )
        .unwrap();
        if self.swing {
            eng.with_policy(ReclaimPolicy {
                strategy: ReclaimStrategy::Swing,
                ..ReclaimPolicy::default()
            })
        } else {
            eng
        }
    }

    /// Model the repoint's origin re-attach, which is the node's job and
    /// not the stack's: after the advance repoints, the export serves the
    /// generation published from S, and both slots read unprovisioned
    /// blocks through it. Head has not drifted since the capture in this
    /// script, so binding the current logical image is exact — and it is
    /// a no-op for reads until reclaim unmaps the shared set.
    fn model_repoint(&self) {
        let img = self.ops.origin_image(ORIGIN);
        self.ops.set_external_origin(ORIGIN, img.clone());
        self.ops.set_path_image(EXT, img);
    }

    fn mutate(&self, phase: u8) {
        let p = |len: usize, seed: u8| -> Vec<u8> {
            (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
        };
        match phase {
            0 => {
                self.ops.write_origin(ORIGIN, 0, &p(3 * CHUNK as usize, 1));
                self.ops
                    .write_origin(ORIGIN, 9 * CHUNK, &p(CHUNK as usize, 7));
            }
            1 => {
                self.ops
                    .write_origin(ORIGIN, 2 * CHUNK, &p(2 * CHUNK as usize, 21));
                // A guest discard is only expressible on a stack with no
                // external origin. Over one, dm-thin serves the unmapped
                // block from the origin — so the block reads as the
                // published generation while `thin_delta` reports it
                // removed and the publish writes a hole. That divergence
                // is the stack's, not the swing's; the port matrix above
                // covers discards, this arm covers the slot bracket.
                if self.swing {
                    self.ops
                        .write_origin(ORIGIN, 9 * CHUNK, &p(CHUNK as usize, 77));
                } else {
                    self.ops.discard_origin(ORIGIN, 9 * CHUNK, CHUNK);
                }
            }
            _ => {
                self.ops
                    .write_origin(ORIGIN, 5 * CHUNK, &p(3 * CHUNK as usize, 91));
            }
        }
    }

    async fn assert_converged(&self, eng: &CycleEngine<DmStack<MockDmOps>, Cp>) {
        let s = eng.state().expect("state exists");
        assert_eq!(s.phase, Phase::Idle);
        assert!(s.candidate.is_none() && s.prepared.is_none());
        if let Some(b) = s.baseline.as_ref() {
            assert!(self.cp.ref_exists(&b.remote).await.unwrap());
            assert!(self
                .cp
                .pinned_by(&b.remote, "agent-baseline:vlin")
                .await
                .unwrap());
        }
        assert!(self.cp.find_prepared().await.unwrap().is_none());
        // Kernel-side hygiene, straight from the mock.
        let obs = eng.stack.observe();
        assert!(obs.candidates.is_empty(), "no candidate devices leak");
        assert!(obs.baselines.len() <= 1, "no duplicate baselines");
        // Across EVERY pool, not just the canonical one: a recovery that
        // failed to sweep slot B leaves a second pool whose thins hold
        // data blocks, and the fixture map cannot see it — so counting
        // there alone would let exactly the leak the sweeps exist to
        // prevent converge green.
        let pools = self.ops.all_thin_ids();
        let thins: usize = pools.iter().map(|(_, ids)| ids.len()).sum();
        assert!(thins <= 2, "no leaked thin devs: {pools:?}");
        assert_eq!(pools.len(), 1, "no leaked slot pool: {pools:?}");
        assert!(!self.ops.frozen(), "fs never left frozen");
        assert!(!self.ops.meta_snap_reserved(), "no leaked reservation");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn dm_crash_at_every_point_converges() {
    // Size the matrix crash-free.
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    for phase in 0u8..3 {
        rig.mutate(phase);
        eng.run_cycle().await.unwrap();
    }
    let total = eng.crash.counter;
    assert!(total > 30, "dense matrix expected, got {total}");
    drop(rig);

    for k in 0..total {
        let rig = Rig::new().await;
        let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
        let mut eng = rig.engine();
        eng.crash.at = Some(k);
        let mut crashed = false;
        for phase in 0u8..3 {
            rig.mutate(phase);
            let img = rig.ops.origin_image(ORIGIN);
            match eng.run_cycle().await {
                Ok(t) => expected.push((t, img)),
                Err(AgentError::InjectedCrash(_)) => {
                    crashed = true;
                    break;
                }
                Err(e) => panic!("point {k}: unexpected {e}"),
            }
        }
        assert!(crashed, "point {k} never fired");
        drop(eng); // the crashed agent process is gone

        let mut eng2 = rig.engine();
        eng2.recover()
            .await
            .unwrap_or_else(|e| panic!("point {k}: recovery failed: {e}"));
        rig.assert_converged(&eng2).await;
        for phase in 0u8..2 {
            rig.mutate(phase + 10);
            let img = rig.ops.origin_image(ORIGIN);
            let t = eng2
                .run_cycle()
                .await
                .unwrap_or_else(|e| panic!("point {k}: post-recovery cycle: {e}"));
            expected.push((t, img));
        }
        rig.assert_converged(&eng2).await;
        for (t, img) in &expected {
            let got = rig.cp.read_snapshot(t, 0, VOL).await.unwrap();
            assert_eq!(&got, img, "point {k}: generation {t} corrupted");
        }
    }
}

/// One scripted history over a SLOT-managed stack whose advance reclaims
/// through the swing executor, crashing at `at`.
async fn swing_scripted_run(at: Option<u32>) -> (u32, bool) {
    let rig = Rig::new_with(true).await;
    let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
    let mut crashed = false;
    let mut eng = rig.engine();
    eng.crash.at = at;

    for phase in 0u8..3 {
        rig.mutate(phase);
        rig.model_repoint();
        let img = rig.ops.origin_image(ORIGIN);
        let kind = if phase == 1 {
            CycleKind::Advance
        } else {
            CycleKind::Publish
        };
        match eng.run_cycle_as(kind).await {
            Ok(t) => expected.push((t, img)),
            Err(AgentError::InjectedCrash(_)) => {
                crashed = true;
                break;
            }
            Err(e) => panic!("point {at:?}: unexpected {e}"),
        }
    }
    let total = eng.crash.counter;
    drop(eng); // the crashed agent process is gone

    if at.is_none() {
        // The strategy really reached the executor: a completed swing is
        // the only thing that renames the slot file into place.
        assert!(
            rig.ops
                .events()
                .iter()
                .any(|e| matches!(e, MockEvent::RenameBacking(..))),
            "the advance never ran the swing"
        );
    }

    if crashed {
        let mut eng2 = rig.engine();
        eng2.recover()
            .await
            .unwrap_or_else(|e| panic!("point {at:?}: recovery failed: {e}"));
        rig.assert_converged(&eng2).await;
        // The cycles AFTER the swing run against the canonicalized pool —
        // the §7 claim that which executor ran is not observable from
        // outside the node, checked over the production stack.
        for phase in 0u8..2 {
            rig.mutate(phase + 10);
            rig.model_repoint();
            let img = rig.ops.origin_image(ORIGIN);
            let t = eng2
                .run_cycle()
                .await
                .unwrap_or_else(|e| panic!("point {at:?}: post-recovery cycle: {e}"));
            expected.push((t, img));
        }
        rig.assert_converged(&eng2).await;
    }
    for (t, img) in &expected {
        let got = rig.cp.read_snapshot(t, 0, VOL).await.unwrap();
        assert_eq!(&got, img, "point {at:?}: generation {t} corrupted");
    }
    (total, crashed)
}

/// The matrix with the advance forced through the swing executor over
/// the PRODUCTION stack: the §7 slot bracket (armed persist, in-window
/// flip, post-flip sweep, canonicalization) meets the mock kernel's
/// contracts, and every crash point converges to a pool that still
/// serves every published generation bit-exactly.
#[tokio::test(flavor = "multi_thread")]
async fn dm_crash_at_every_point_converges_with_swing() {
    let (total, crashed) = swing_scripted_run(None).await;
    assert!(!crashed);
    assert!(total > 30, "dense matrix expected, got {total}");

    for k in 0..total {
        let (_, crashed) = swing_scripted_run(Some(k)).await;
        assert!(crashed, "point {k} never fired");
    }
}

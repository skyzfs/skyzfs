//! Layer-3 tests for the engine commit path (design §14.2).
//!
//! Every test names the invariant(s) it verifies (design §6.2).

use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::{CasOutcome, MemoryRefStore, RedbRefStore, RefStore, RefValue};
use skynbd_store::{FaultOp, FaultStore, MemoryStore};
use skynbd_types::{CommitHash, VolumeId};

const KIB: u64 = 1024;

fn test_cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 4 * KIB, // force sealing mid-workload
        shard_bytes: 64 * KIB,      // force multi-shard commits
        epoch: 0,
        ..EngineConfig::default()
    }
}

fn vol() -> VolumeId {
    VolumeId(ulid::Ulid::new())
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

// -------------------------------------------------------------- basic path

/// I3: acked writes are readable before any commit.
#[tokio::test]
async fn read_your_writes_before_commit() {
    let mut eng = Engine::create_volume(
        MemoryStore::new(),
        MemoryRefStore::new(),
        vol(),
        256 * KIB,
        test_cfg(),
    )
    .await
    .unwrap();

    let data = pattern(2048, 7);
    eng.write(4096, &data).unwrap();
    assert_eq!(eng.read(4096, 2048).await.unwrap(), data);
    // Unwritten space reads zero.
    assert_eq!(eng.read(0, 512).await.unwrap(), vec![0u8; 512]);
    // Overwrite wins (last-writer).
    let data2 = pattern(512, 99);
    eng.write(4096, &data2).unwrap();
    assert_eq!(eng.read(4096, 512).await.unwrap(), data2);
}

/// I4: after flush + reopen, flushed content is fully recoverable.
/// I1: reopen walks the complete closure or fails.
#[tokio::test]
async fn flush_then_reopen_recovers_exact_content() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol();

    let data_a = pattern(8192, 1); // spans multiple sealed segments
    let data_b = pattern(1024, 2);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, test_cfg())
        .await
        .unwrap();
    eng.write(0, &data_a).unwrap();
    eng.write(100 * KIB, &data_b).unwrap(); // second shard
    eng.write_zeroes(512, 512).unwrap(); // punch a hole in data_a
    eng.commit(CommitKind::Flush).await.unwrap();
    drop(eng);

    let eng = Engine::open(store, refs, v, test_cfg()).await.unwrap();
    assert_eq!(eng.flush_seq(), 1);
    let mut expect_a = data_a.clone();
    expect_a[512..1024].fill(0);
    assert_eq!(eng.read(0, 8192).await.unwrap(), expect_a);
    assert_eq!(eng.read(100 * KIB, 1024).await.unwrap(), data_b);
    assert_eq!(eng.read(200 * KIB, 512).await.unwrap(), vec![0u8; 512]);
}

/// Un-flushed writes are lost on crash; flushed state survives (I4, I7, D1).
#[tokio::test]
async fn unflushed_writes_lost_flushed_state_intact() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol();

    let flushed = pattern(4096, 3);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, test_cfg())
        .await
        .unwrap();
    eng.write(0, &flushed).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    eng.write(0, &pattern(4096, 44)).unwrap(); // never flushed
    drop(eng); // crash

    let eng = Engine::open(store, refs, v, test_cfg()).await.unwrap();
    assert_eq!(eng.read(0, 4096).await.unwrap(), flushed);
}

/// Commits are cheap no-ops when nothing changed.
#[tokio::test]
async fn empty_commit_is_noop() {
    let mut eng = Engine::create_volume(
        MemoryStore::new(),
        MemoryRefStore::new(),
        vol(),
        64 * KIB,
        test_cfg(),
    )
    .await
    .unwrap();
    let h0 = eng.commit(CommitKind::Flush).await.unwrap();
    let h1 = eng.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(h0, h1);
    assert_eq!(eng.flush_seq(), 0);
}

/// Alignment and bounds are enforced (design §5.3).
#[tokio::test]
async fn io_validation() {
    let mut eng = Engine::create_volume(
        MemoryStore::new(),
        MemoryRefStore::new(),
        vol(),
        64 * KIB,
        test_cfg(),
    )
    .await
    .unwrap();
    assert!(matches!(
        eng.write(100, &[0u8; 512]),
        Err(EngineError::InvalidIo(_))
    ));
    assert!(matches!(
        eng.write(0, &[0u8; 100]),
        Err(EngineError::InvalidIo(_))
    ));
    assert!(matches!(
        eng.write(63 * KIB, &[0u8; 2048]),
        Err(EngineError::InvalidIo(_))
    ));
    assert!(matches!(
        eng.read(0, 0).await,
        Err(EngineError::InvalidIo(_))
    ));
}

/// Works identically over the redb ref store, surviving process restart
/// (redb file persists; D6).
#[tokio::test]
async fn redb_refs_end_to_end() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("refs.redb");
    let store = MemoryStore::new();
    let v = vol();
    let data = pattern(2048, 5);

    {
        let refs = RedbRefStore::open(&path).unwrap();
        let mut eng = Engine::create_volume(store.clone(), refs, v, 64 * KIB, test_cfg())
            .await
            .unwrap();
        eng.write(0, &data).unwrap();
        eng.commit(CommitKind::Flush).await.unwrap();
    } // engine + refs dropped ("restart")

    let refs = RedbRefStore::open(&path).unwrap();
    let eng = Engine::open(store, refs, v, test_cfg()).await.unwrap();
    assert_eq!(eng.read(0, 2048).await.unwrap(), data);
}

// ------------------------------------------------------ crash-point matrix

/// Kill the commit at every labeled stage of design §6.3 via fault
/// injection; after each "crash", reopen and assert I1/I4/I7: content
/// equals the last acked flush, closure walks completely, head unmoved.
#[tokio::test]
async fn crash_point_matrix() {
    // (label, key-class to kill, ops allowed through first)
    let points: &[(&str, &str, u64)] = &[
        ("first segment PUT", "/seg/", 0),
        ("mid segment PUTs", "/seg/", 1),
        ("first node PUT", "/node/", 0),
        ("mid node PUTs", "/node/", 2),
        ("commit PUT", "/commit/", 0),
    ];

    for &(label, class, allow) in points {
        let store = FaultStore::new(MemoryStore::new());
        let refs = MemoryRefStore::new();
        let v = vol();

        // Establish flushed baseline state.
        let baseline = pattern(8192, 10);
        let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, test_cfg())
            .await
            .unwrap();
        eng.write(0, &baseline).unwrap();
        eng.commit(CommitKind::Flush).await.unwrap();
        let head_before = refs.get(v, "head").await.unwrap().unwrap();

        // Doomed commit: spans two shards, several segments.
        eng.write(0, &pattern(8192, 20)).unwrap();
        eng.write(100 * KIB, &pattern(4096, 21)).unwrap();
        store.arm(FaultOp::Put, class, allow).await;
        let err = eng.commit(CommitKind::Flush).await;
        assert!(err.is_err(), "{label}: commit must fail");
        drop(eng); // crash
        store.disarm().await; // restarted process has a healthy store

        // I7/I4: reopen serves exactly the last acked flush.
        let eng = Engine::open(store.clone(), refs.clone(), v, test_cfg())
            .await
            .unwrap_or_else(|e| {
                panic!("{label}: reopen failed (I1 violated): {e}");
            });
        assert_eq!(
            eng.read(0, 8192).await.unwrap(),
            baseline,
            "{label}: content mismatch"
        );
        assert_eq!(
            eng.read(100 * KIB, 4096).await.unwrap(),
            vec![0u8; 4096],
            "{label}: torn commit visible"
        );
        // Head never moved (I2): orphan objects are GC's job, not
        // visibility's.
        assert_eq!(
            refs.get(v, "head").await.unwrap().unwrap(),
            head_before,
            "{label}"
        );

        // The volume is fully writable after recovery.
        let mut eng = eng;
        eng.write(0, &pattern(1024, 30)).unwrap();
        eng.commit(CommitKind::Flush).await.unwrap_or_else(|e| {
            panic!("{label}: post-recovery commit failed: {e}");
        });
        assert_eq!(
            eng.read(0, 1024).await.unwrap(),
            pattern(1024, 30),
            "{label}"
        );
    }
}

/// A lost CAS (lease stolen) must fail the commit and not adopt state (I2).
#[tokio::test]
async fn cas_conflict_fails_commit_without_adopting() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol();
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 64 * KIB, test_cfg())
        .await
        .unwrap();
    eng.write(0, &pattern(512, 1)).unwrap();

    // Another writer steals the head between our load and our CAS.
    let cur = refs.get(v, "head").await.unwrap().unwrap();
    let stolen: CommitHash = [0xEE; 32];
    match refs
        .compare_and_swap(v, "head", Some(cur), Some(stolen))
        .await
        .unwrap()
    {
        CasOutcome::Committed(Some(RefValue { .. })) => {}
        other => panic!("setup failed: {other:?}"),
    }

    assert!(matches!(
        eng.commit(CommitKind::Flush).await,
        Err(EngineError::Conflict)
    ));
}

// ------------------------------------------------- property test vs model

mod model {
    use super::*;
    use proptest::prelude::*;

    const VOL_BYTES: u64 = 128 * KIB;

    #[derive(Debug, Clone)]
    enum Op {
        Write { off_s: u64, len_s: u64, seed: u8 },
        Zero { off_s: u64, len_s: u64 },
        Flush,
        CrashReopen,
    }

    fn op_strategy() -> impl Strategy<Value = Op> {
        prop_oneof![
            4 => (0..VOL_BYTES / 512, 1..=16u64, any::<u8>()).prop_filter_map(
                "in bounds",
                |(o, l, s)| (o * 512 + l * 512 <= VOL_BYTES)
                    .then_some(Op::Write { off_s: o, len_s: l, seed: s })
            ),
            2 => (0..VOL_BYTES / 512, 1..=16u64).prop_filter_map(
                "in bounds",
                |(o, l)| (o * 512 + l * 512 <= VOL_BYTES).then_some(Op::Zero { off_s: o, len_s: l })
            ),
            1 => Just(Op::Flush),
            1 => Just(Op::CrashReopen),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]

        /// The engine agrees byte-for-byte with a flat model through
        /// arbitrary write/zero/flush/crash sequences (I3, I4, I7).
        #[test]
        fn engine_matches_model(ops in proptest::collection::vec(op_strategy(), 1..40)) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            rt.block_on(async move {
                let store = MemoryStore::new();
                let refs = MemoryRefStore::new();
                let v = VolumeId(ulid::Ulid::new());
                let mut eng =
                    Engine::create_volume(store.clone(), refs.clone(), v, VOL_BYTES, test_cfg())
                        .await
                        .unwrap();
                let mut current = vec![0u8; VOL_BYTES as usize];
                let mut flushed = current.clone();

                for op in ops {
                    match op {
                        Op::Write { off_s, len_s, seed } => {
                            let (off, len) = (off_s * 512, (len_s * 512) as usize);
                            let data = super::pattern(len, seed);
                            eng.write(off, &data).unwrap();
                            current[off as usize..off as usize + len].copy_from_slice(&data);
                        }
                        Op::Zero { off_s, len_s } => {
                            let (off, len) = (off_s * 512, len_s * 512);
                            eng.write_zeroes(off, len).unwrap();
                            current[off as usize..(off + len) as usize].fill(0);
                        }
                        Op::Flush => {
                            eng.commit(CommitKind::Flush).await.unwrap();
                            flushed = current.clone();
                        }
                        Op::CrashReopen => {
                            drop(eng);
                            eng = Engine::open(store.clone(), refs.clone(), v, test_cfg())
                                .await
                                .unwrap();
                            current = flushed.clone();
                        }
                    }
                    let got = eng.read(0, VOL_BYTES).await.unwrap();
                    prop_assert_eq!(&got, &current, "device diverged from model");
                }
                Ok(())
            })?;
        }
    }
}

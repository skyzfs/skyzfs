//! Snapshot subsystem tests (design §9, milestone M3).
//!
//! Each test names its invariant (design §6.2).

use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const KIB: u64 = 1024;

fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 4 * KIB,
        shard_bytes: 64 * KIB,
        epoch: 0,
        ..EngineConfig::default()
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

async fn fixture() -> (
    MemoryStore,
    MemoryRefStore,
    VolumeId,
    Engine<MemoryStore, MemoryRefStore>,
) {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let eng = Engine::create_volume(store.clone(), refs.clone(), vol, 256 * KIB, cfg())
        .await
        .unwrap();
    (store, refs, vol, eng)
}

/// Snapshot creation on a clean volume writes exactly one object (the
/// manifest) beyond a ref CAS — the O(1) claim of design §9.6.
#[tokio::test]
async fn clean_snapshot_create_writes_only_the_manifest() {
    let (store, _refs, _vol, mut eng) = fixture().await;
    eng.write(0, &pattern(4096, 1)).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();

    let objects_before = store.object_count().await;
    let t0 = std::time::Instant::now();
    eng.snapshot_create("s1").await.unwrap();
    let elapsed = t0.elapsed();
    assert_eq!(
        store.object_count().await,
        objects_before + 2,
        "clean snapshot writes exactly the manifest and the commit's \
         emap (design D-M7) — no data, no nodes"
    );
    assert!(
        elapsed.as_millis() < 100,
        "snapshot create took {elapsed:?}"
    );
}

/// A snapshot of a dirty volume forces a commit first (D10): the snapshot
/// never references volatile overlay state.
#[tokio::test]
async fn dirty_snapshot_forces_commit() {
    let (store, refs, vol, mut eng) = fixture().await;
    let data = pattern(4096, 2);
    eng.write(0, &data).unwrap(); // un-flushed
    let (_, snap_commit) = eng.snapshot_create("s1").await.unwrap();

    // The snapshot commit is durable and readable on its own (I1).
    let ro = Engine::open_at_commit(store, refs, vol, snap_commit, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 4096).await.unwrap(), data);
    assert_eq!(eng.flush_seq(), 1, "snapshot forced a flush");
}

/// I5: the commit behind a snapshot never changes — later writes to the
/// head are invisible through the snapshot.
#[tokio::test]
async fn snapshot_is_immutable_under_head_writes() {
    let (store, refs, vol, mut eng) = fixture().await;
    let a = pattern(8192, 0xA);
    let b = pattern(8192, 0xB);

    eng.write(0, &a).unwrap();
    let (_, s1) = eng.snapshot_create("s1").await.unwrap();
    eng.write(0, &b).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();

    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, s1, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 8192).await.unwrap(), a, "snapshot changed (I5)");
    assert_eq!(
        eng.read(0, 8192).await.unwrap(),
        b,
        "head must see new data"
    );

    // Still true after a full restart (I1 + I5).
    drop(ro);
    let ro = Engine::open_at_commit(store, refs, vol, s1, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 8192).await.unwrap(), a);
}

/// The read-only view cannot advance the volume: commits fail (I2 sentinel).
#[tokio::test]
async fn snapshot_view_cannot_commit() {
    let (store, refs, vol, mut eng) = fixture().await;
    eng.write(0, &pattern(512, 1)).unwrap();
    let (_, s1) = eng.snapshot_create("s1").await.unwrap();

    let mut ro = Engine::open_at_commit(store, refs, vol, s1, cfg())
        .await
        .unwrap();
    ro.write(0, &pattern(512, 9)).unwrap(); // NBD layer forbids this; engine must still be safe
    assert!(matches!(
        ro.commit(CommitKind::Flush).await,
        Err(EngineError::Conflict)
    ));
}

#[tokio::test]
async fn list_delete_and_duplicate_names() {
    let (_store, _refs, _vol, mut eng) = fixture().await;
    eng.write(0, &pattern(512, 1)).unwrap();
    let (_, c1) = eng.snapshot_create("nightly-1").await.unwrap();
    let (_, c2) = eng.snapshot_create("nightly-2").await.unwrap();
    assert_eq!(c1, c2, "clean second snapshot shares the same commit");

    assert_eq!(
        eng.snapshot_list().await.unwrap(),
        vec![("nightly-1".to_string(), c1), ("nightly-2".to_string(), c2)]
    );

    assert!(matches!(
        eng.snapshot_create("nightly-1").await,
        Err(EngineError::SnapshotExists(_))
    ));
    assert!(matches!(
        eng.snapshot_create("bad/name").await,
        Err(EngineError::InvalidIo(_))
    ));

    eng.snapshot_delete("nightly-1").await.unwrap();
    assert_eq!(eng.snapshot_list().await.unwrap().len(), 1);
    assert!(matches!(
        eng.snapshot_delete("nightly-1").await,
        Err(EngineError::SnapshotNotFound(_))
    ));
}

/// Rollback per design §9.6: head CASes to the snapshot's commit; dirty
/// overlays are refused; the volume writes on normally afterwards.
#[tokio::test]
async fn rollback_restores_snapshot_content() {
    let (store, refs, vol, mut eng) = fixture().await;
    let a = pattern(8192, 0xA);
    let b = pattern(8192, 0xB);
    let c = pattern(4096, 0xC);

    eng.write(0, &a).unwrap();
    eng.snapshot_create("s1").await.unwrap();
    eng.write(0, &b).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();

    // Dirty rollback refused.
    eng.write(0, &pattern(512, 0xE)).unwrap();
    assert!(matches!(
        eng.rollback("s1").await,
        Err(EngineError::DirtyRollback)
    ));
    eng.commit(CommitKind::Flush).await.unwrap();

    // Clean rollback restores A.
    eng.rollback("s1").await.unwrap();
    assert_eq!(eng.read(0, 8192).await.unwrap(), a);

    // Unknown snapshot refused.
    assert!(matches!(
        eng.rollback("nope").await,
        Err(EngineError::SnapshotNotFound(_))
    ));

    // The volume advances normally after rollback, and survives reopen.
    eng.write(0, &c).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    drop(eng);
    let eng = Engine::open(store, refs, vol, cfg()).await.unwrap();
    let mut expect = a.clone();
    expect[..4096].copy_from_slice(&c);
    assert_eq!(eng.read(0, 8192).await.unwrap(), expect);
}

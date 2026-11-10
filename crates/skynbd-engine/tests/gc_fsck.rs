//! GC and fsck/rebuild tests (design §10, §6.5 — milestone M4).

use skynbd_engine::{fsck, rebuild_refs, Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::MemoryRefStore;
use skynbd_store::{MemoryStore, ObjectStore};
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

/// I6: GC sweeps crash orphans, never anything reachable; idempotent.
#[tokio::test]
async fn gc_sweeps_orphans_preserves_live() {
    let (store, refs, vol, mut eng) = fixture().await;
    let a = pattern(8192, 1);
    eng.write(0, &a).unwrap();
    eng.snapshot_create("s1").await.unwrap();
    let b = pattern(8192, 2);
    eng.write(0, &b).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();

    // Baseline sweep first: the initial empty commit + its index node are
    // legitimately unreachable history now (design §10.1 prunes to refs).
    eng.gc().await.unwrap();

    // Orphans as a crashed commit leaves them (design §6.4): objects
    // under seg/ and node/ reachable from no ref.
    let prefix = skynbd_engine::keys::volume_prefix(vol);
    store
        .put(
            &format!("{prefix}seg/ORPHAN"),
            bytes::Bytes::from_static(b"junk"),
        )
        .await
        .unwrap();
    store
        .put(
            &format!("{prefix}node/{}", "ab".repeat(32)),
            bytes::Bytes::from_static(b"junk"),
        )
        .await
        .unwrap();

    let before = store.object_count().await;
    let report = eng.gc().await.unwrap();
    assert_eq!(report.swept_objects, 2, "exactly the two orphans");
    assert_eq!(store.object_count().await, before - 2);

    // Reachable state intact: head reads B, snapshot reads A (I5, I6).
    assert_eq!(eng.read(0, 8192).await.unwrap(), b);
    let (_, s1_commit) = eng
        .snapshot_list()
        .await
        .unwrap()
        .into_iter()
        .next()
        .unwrap();
    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, s1_commit, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 8192).await.unwrap(), a);

    // Idempotent: second run sweeps nothing.
    assert_eq!(eng.gc().await.unwrap().swept_objects, 0);

    // fsck clean after GC (S8 postcondition).
    assert!(fsck(&store, &refs, vol).await.unwrap().ok());
}

/// Deleting a snapshot releases exactly its private objects (S8 core).
#[tokio::test]
async fn gc_after_snapshot_delete_reclaims() {
    let (store, refs, vol, mut eng) = fixture().await;
    eng.write(0, &pattern(16384, 1)).unwrap();
    eng.snapshot_create("s1").await.unwrap();
    eng.write(0, &pattern(16384, 2)).unwrap(); // fully overwrites: s1 data becomes private
    eng.commit(CommitKind::Flush).await.unwrap();
    eng.gc().await.unwrap(); // baseline: nothing sweepable yet
    let before = store.object_count().await;

    eng.snapshot_delete("s1").await.unwrap();
    let report = eng.gc().await.unwrap();
    assert!(
        report.swept_objects > 0,
        "old segments + nodes + commit + manifest must go"
    );
    assert!(store.object_count().await < before);

    // Head unaffected; volume healthy.
    assert_eq!(eng.read(0, 16384).await.unwrap(), pattern(16384, 2));
    assert!(fsck(&store, &refs, vol).await.unwrap().ok());
}

#[tokio::test]
async fn gc_dirty_refused() {
    let (_s, _r, _v, mut eng) = fixture().await;
    eng.write(0, &pattern(512, 1)).unwrap();
    assert!(matches!(eng.gc().await, Err(EngineError::DirtyGc)));
}

/// fsck detects a broken closure (missing node) and names the refs.
#[tokio::test]
async fn fsck_detects_missing_objects() {
    let (store, refs, vol, mut eng) = fixture().await;
    eng.write(0, &pattern(4096, 1)).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    assert!(fsck(&store, &refs, vol).await.unwrap().ok());

    // Break the closure: delete one *live* node object (GC first so no
    // unreferenced historical nodes remain to pick by accident).
    eng.gc().await.unwrap();
    let prefix = skynbd_engine::keys::volume_prefix(vol);
    let mut nodes = store.list(&format!("{prefix}node/")).await.unwrap();
    nodes.sort();
    store.delete(&nodes[0]).await.unwrap();

    let report = fsck(&store, &refs, vol).await.unwrap();
    assert!(!report.ok());
    assert!(report.refs.iter().any(|(n, e)| n == "head" && e.is_some()));
}

/// §6.5: refs are reconstructible from S3 alone; existing refs win.
#[tokio::test]
async fn rebuild_refs_restores_missing_and_prefers_existing() {
    let (store, _refs, vol, mut eng) = fixture().await;
    let a = pattern(8192, 0xA);
    let b = pattern(8192, 0xB);
    eng.write(0, &a).unwrap();
    eng.snapshot_create("s1").await.unwrap();
    eng.write(0, &b).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    let head_before = eng.head_commit_hash();
    drop(eng);

    // Total ref-store loss (fresh redb in production; fresh memory here).
    let fresh = MemoryRefStore::new();
    let report = rebuild_refs(&store, &fresh, vol).await.unwrap();
    assert!(report.head_rebuilt);
    assert_eq!(report.refs_rebuilt, vec!["snap/s1".to_string()]);

    let eng = Engine::open(store.clone(), fresh.clone(), vol, cfg())
        .await
        .unwrap();
    assert_eq!(
        eng.head_commit_hash(),
        head_before,
        "head = highest complete flush_seq"
    );
    assert_eq!(eng.read(0, 8192).await.unwrap(), b);
    let snaps = eng.snapshot_list().await.unwrap();
    let ro = Engine::open_at_commit(store.clone(), fresh.clone(), vol, snaps[0].1, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 8192).await.unwrap(), a);

    // Existing refs are authoritative (rollback caveat): point head at the
    // snapshot commit (older flush_seq) and rebuild — head must not move.
    let mut eng = Engine::open(store.clone(), fresh.clone(), vol, cfg())
        .await
        .unwrap();
    eng.rollback("s1").await.unwrap();
    let rolled = eng.head_commit_hash();
    drop(eng);
    let report = rebuild_refs(&store, &fresh, vol).await.unwrap();
    assert!(!report.head_rebuilt);
    assert_eq!(report.refs_existing, 2);
    let eng = Engine::open(store, fresh, vol, cfg()).await.unwrap();
    assert_eq!(
        eng.head_commit_hash(),
        rolled,
        "rebuild must not undo rollback"
    );
}

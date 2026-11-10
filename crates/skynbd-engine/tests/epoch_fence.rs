//! `Commit.epoch` (design D-M5): recovery orders candidates by epoch
//! before `flush_seq`, because a fenced writer's discarded commits are
//! byte-for-byte plausible and can carry a *higher* `flush_seq` than the
//! winner ever reached. These tests build exactly that shape and assert
//! the zombie stays dead.

use skynbd_engine::{keys, rebuild_refs, Engine, EngineConfig};
use skynbd_format::{Codec, Commit, CommitKind};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

const KIB: u64 = 1024;

fn cfg(epoch: u64) -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 4 * KIB,
        shard_bytes: 64 * KIB,
        epoch,
        ..EngineConfig::default()
    }
}

/// A commit that mimics a fenced writer's leftover: complete closure
/// (it borrows a live commit's shard index, exactly like a zombie whose
/// data all landed), plausible kind — and a `flush_seq` far beyond the
/// winner's. Only the epoch gives it away.
fn zombie_from(winner: &Commit, epoch: u64, flush_seq: u64) -> Commit {
    Commit {
        vol_id: winner.vol_id,
        parent: winner.parent,
        shard_index: winner.shard_index,
        segments: vec![],
        flush_seq,
        created_at_ms: winner.created_at_ms + 1_000_000, // newer, too
        kind: CommitKind::Flush,
        group: None,
        epoch,
    }
}

async fn head_commit(store: &MemoryStore, refs: &dyn RefStore, vol: VolumeId) -> Commit {
    let head = refs.get(vol, "head").await.unwrap().unwrap();
    let bytes = store
        .get(&keys::commit(vol, &head.commit), None)
        .await
        .unwrap();
    Commit::decode(&bytes).unwrap()
}

/// The D-M5 scenario. Writer at epoch 1 is fenced; its discarded commit
/// (flush_seq 99) is still in the bucket. The epoch-2 winner's last
/// commit has flush_seq 2. Refs are lost; recovery must pick the winner.
/// Under the old `(flush_seq, created_at_ms)` order the zombie wins and
/// resurrects exactly the writes the fence discarded.
#[tokio::test]
async fn rebuild_prefers_the_highest_epoch_over_the_highest_flush_seq() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());

    // The winner's chain, epoch 2.
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), vol, 256 * KIB, cfg(2))
        .await
        .unwrap();
    let winner_data = vec![0xBBu8; 4096];
    eng.write(0, &winner_data).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    let winner_head = eng.head_commit_hash();
    let winner = head_commit(&store, &refs, vol).await;
    assert_eq!(winner.epoch, 2);
    drop(eng);

    // The fenced predecessor's leftover: epoch 1, flush_seq 99.
    let zombie = zombie_from(&winner, 1, 99);
    let zh = zombie.content_hash().unwrap();
    store
        .put(&keys::commit(vol, &zh), zombie.encode().unwrap().into())
        .await
        .unwrap();

    // Total refs loss; rebuild from the bucket alone.
    let fresh = MemoryRefStore::new();
    let report = rebuild_refs(&store, &fresh, vol).await.unwrap();
    assert!(report.head_rebuilt);
    let head = fresh.get(vol, "head").await.unwrap().unwrap();
    assert_eq!(
        head.commit, winner_head,
        "epoch orders before flush_seq: the zombie must stay dead"
    );

    let eng = Engine::open(store.clone(), fresh.clone(), vol, cfg(2))
        .await
        .unwrap();
    assert_eq!(eng.read(0, 4096).await.unwrap(), winner_data);
}

/// A surviving lease row caps the scan: a commit claiming an epoch the
/// lease history never granted (an impostor, or corruption) is ignored
/// outright, even though it sorts highest.
#[tokio::test]
async fn rebuild_ignores_epochs_the_lease_never_granted() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());

    // Legitimate history at epoch 1, with the lease row to prove it.
    refs.lease_acquire(vol, "node-a", 60_000).await.unwrap();
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), vol, 256 * KIB, cfg(1))
        .await
        .unwrap();
    eng.write(0, &[0xAAu8; 4096]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    let legit_head = eng.head_commit_hash();
    let legit = head_commit(&store, &refs, vol).await;
    drop(eng);

    // An impostor claiming epoch 99 — sorts above everything.
    let impostor = zombie_from(&legit, 99, 100);
    let ih = impostor.content_hash().unwrap();
    store
        .put(&keys::commit(vol, &ih), impostor.encode().unwrap().into())
        .await
        .unwrap();

    // Head lost, lease row intact (the object-mode shape: same row).
    let cur = refs.get(vol, "head").await.unwrap();
    refs.compare_and_swap(vol, "head", cur, None).await.unwrap();

    let report = rebuild_refs(&store, &refs, vol).await.unwrap();
    assert!(report.head_rebuilt);
    let head = refs.get(vol, "head").await.unwrap().unwrap();
    assert_eq!(head.commit, legit_head, "epoch 99 was never granted");
}

/// Pre-epoch volumes (every commit at 0) keep their exact old recovery
/// behavior: highest complete `flush_seq` wins.
#[tokio::test]
async fn rebuild_of_pre_epoch_history_is_unchanged() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), vol, 256 * KIB, cfg(0))
        .await
        .unwrap();
    eng.write(0, &[1u8; 4096]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    eng.write(0, &[2u8; 4096]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    let head_before = eng.head_commit_hash();
    drop(eng);

    let fresh = MemoryRefStore::new();
    rebuild_refs(&store, &fresh, vol).await.unwrap();
    assert_eq!(
        fresh.get(vol, "head").await.unwrap().unwrap().commit,
        head_before
    );
}

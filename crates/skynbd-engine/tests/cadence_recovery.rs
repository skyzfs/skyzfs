//! The D-M4 crash story: under cadenced refs export the owner dies with
//! its head N flushes ahead of the refs object. The successor steals the
//! lease, forward-scans `commit/` for the owner-of-record's epoch, rolls
//! the head to the newest complete commit — and a zombie's leftovers,
//! however plausible their `flush_seq`, stay dead because their epoch is
//! not the owner-of-record's.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use skynbd_engine::{keys, recover_head, Engine, EngineConfig};
use skynbd_format::{Codec, Commit, CommitKind};
use skynbd_refs::{CadencedRefStore, Clock, LeaseConfig, LeaseOutcome, ObjectRefStore, RefStore};
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

fn cadenced(
    shared: &Arc<MemoryStore>,
    holder: &str,
    clock: &Clock,
) -> Arc<CadencedRefStore<Arc<dyn ObjectStore>>> {
    Arc::new(CadencedRefStore::new(
        ObjectRefStore::new(Arc::clone(shared) as Arc<dyn ObjectStore>)
            .with_clock(Arc::clone(clock))
            .with_owner(LeaseConfig {
                holder: holder.to_string(),
                ttl_ms: 10_000,
            }),
    ))
}

fn test_clock() -> (Arc<AtomicU64>, Clock) {
    let now = Arc::new(AtomicU64::new(1_000_000));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    (now, clock)
}

/// A exports once at serve, then flushes twice more and dies. B steals,
/// recovers the tail by epoch, and reads everything A acked — while an
/// old-epoch zombie commit with a far higher flush_seq is ignored.
#[tokio::test]
async fn successor_recovers_the_cadence_gap_and_ignores_zombies() {
    let shared = Arc::new(MemoryStore::new());
    let (now, clock) = test_clock();
    let store: Arc<dyn ObjectStore> = Arc::clone(&shared) as Arc<dyn ObjectStore>;
    let vol = VolumeId(ulid::Ulid::new());

    // --- A's tenure (epoch 1) ---
    let refs_a = cadenced(&shared, "node-a", &clock);
    assert!(matches!(
        refs_a.lease_acquire(vol, "node-a", 10_000).await.unwrap(),
        LeaseOutcome::Acquired(_)
    ));
    let mut eng = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs_a) as Arc<dyn RefStore>,
        vol,
        256 * KIB,
        cfg(1),
    )
    .await
    .unwrap();
    eng.write(0, &[0x11u8; 4096]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    // The serve-time export: the last state the refs object will see.
    refs_a.flush_volume(vol).await.unwrap();

    // The gap: two more acked flushes, never exported.
    eng.write(0, &[0x22u8; 4096]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    eng.write(4096, &[0x33u8; 4096]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    let a_last_head = eng.head_commit_hash();
    drop(eng);
    drop(refs_a); // A dies without export or release.

    // A zombie leftover from a fenced pre-history: epoch 0, flush 99,
    // complete closure (it borrows the live head's shard index).
    let live = Commit::decode(
        &shared
            .get(&keys::commit(vol, &a_last_head), None)
            .await
            .unwrap(),
    )
    .unwrap();
    let zombie = Commit {
        flush_seq: 99,
        created_at_ms: live.created_at_ms + 1,
        epoch: 0,
        segments: vec![],
        ..live.clone()
    };
    let zh = zombie.content_hash().unwrap();
    shared
        .put(&keys::commit(vol, &zh), zombie.encode().unwrap().into())
        .await
        .unwrap();

    // --- B's takeover (epoch 2) ---
    let refs_b = cadenced(&shared, "node-b", &clock);
    // The owner of record, read before acquiring.
    let prior = refs_b.lease_get(vol).await.unwrap().unwrap();
    assert_eq!((prior.holder.as_str(), prior.epoch), ("node-a", 1));
    // The exported head trails A's real head.
    let exported = refs_b.get(vol, "head").await.unwrap().unwrap();
    assert_ne!(exported.commit, a_last_head, "the gap must exist");

    now.fetch_add(30_000, Ordering::SeqCst); // TTL lapses
    match refs_b.lease_acquire(vol, "node-b", 10_000).await.unwrap() {
        LeaseOutcome::Acquired(l) => assert_eq!(l.epoch, 2),
        other => panic!("steal: {other:?}"),
    }

    let recovered = recover_head(&store, &refs_b, vol, prior.epoch)
        .await
        .unwrap();
    assert_eq!(
        recovered,
        Some(a_last_head),
        "the scan must adopt A's newest commit, not the zombie's flush 99"
    );
    refs_b.flush_volume(vol).await.unwrap(); // single-epoch-gap invariant

    // B opens and reads everything A acked.
    let eng = Engine::open(
        Arc::clone(&store),
        Arc::clone(&refs_b) as Arc<dyn RefStore>,
        vol,
        cfg(2),
    )
    .await
    .unwrap();
    assert_eq!(eng.read(0, 4096).await.unwrap(), vec![0x22u8; 4096]);
    assert_eq!(eng.read(4096, 4096).await.unwrap(), vec![0x33u8; 4096]);
}

/// Under per-commit export there is no gap: the scan is a cheap no-op.
#[tokio::test]
async fn recovery_is_a_noop_without_a_gap() {
    let shared = Arc::new(MemoryStore::new());
    let store: Arc<dyn ObjectStore> = Arc::clone(&shared) as Arc<dyn ObjectStore>;
    let refs: Arc<dyn RefStore> = Arc::new(ObjectRefStore::new(Arc::clone(&store)));
    let vol = VolumeId(ulid::Ulid::new());

    let mut eng = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol,
        256 * KIB,
        cfg(1),
    )
    .await
    .unwrap();
    eng.write(0, &[1u8; 512]).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    let head = eng.head_commit_hash();
    drop(eng);

    assert_eq!(recover_head(&store, &refs, vol, 1).await.unwrap(), None);
    assert_eq!(
        refs.get(vol, "head").await.unwrap().unwrap().commit,
        head,
        "an up-to-date head must not move"
    );
}

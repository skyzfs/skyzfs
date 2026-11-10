//! The D-M3 self-fence, end to end through the engine: a server whose
//! volume was stolen gets `EngineError::Fenced` (carrying the thief's
//! identity) from its next commit — an error the NBD layer turns into
//! `ESHUTDOWN` exactly like `Conflict` — and the winner's history is
//! untouched by anything the loser acked but never landed.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::{Clock, LeaseConfig, LeaseOutcome, ObjectRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

const KIB: u64 = 1024;

fn fenced(
    shared: &Arc<MemoryStore>,
    holder: &str,
    clock: &Clock,
) -> Arc<ObjectRefStore<Arc<dyn ObjectStore>>> {
    Arc::new(
        ObjectRefStore::new(Arc::clone(shared) as Arc<dyn ObjectStore>)
            .with_clock(Arc::clone(clock))
            .with_owner(LeaseConfig {
                holder: holder.to_string(),
                ttl_ms: 10_000,
            }),
    )
}

/// A serves and commits; dies silently; B steals after the TTL and picks
/// the volume up at A's last flush; A comes back and tries to commit its
/// still-buffered writes — `Conflict`, by the lease fence, even though
/// A's CAS expectation for the head is still correct.
#[tokio::test]
async fn stale_server_commit_is_conflict_after_steal() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let refs_a = fenced(&shared, "node-a", &clock);
    let refs_b = fenced(&shared, "node-b", &clock);
    let v = VolumeId(ulid::Ulid::from_parts(0xD3, 0xE));
    let store: Arc<dyn ObjectStore> = Arc::clone(&shared) as Arc<dyn ObjectStore>;

    // A: lease, create, write, flush — the healthy owner's life.
    assert!(matches!(
        refs_a.lease_acquire(v, "node-a", 10_000).await.unwrap(),
        LeaseOutcome::Acquired(_)
    ));
    let mut eng_a = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs_a) as Arc<dyn RefStore>,
        v,
        256 * KIB,
        EngineConfig::default(),
    )
    .await
    .unwrap();
    eng_a.write(0, &[7u8; 4096]).unwrap();
    eng_a.commit(CommitKind::Flush).await.unwrap();

    // A buffers more writes but goes silent before committing them.
    eng_a.write(8192, &[8u8; 4096]).unwrap();

    // B waits out the TTL, steals, and opens at A's last flush.
    now.fetch_add(30_000, Ordering::SeqCst);
    match refs_b.lease_acquire(v, "node-b", 10_000).await.unwrap() {
        LeaseOutcome::Acquired(l) => assert_eq!(l.epoch, 2),
        other => panic!("steal: {other:?}"),
    }
    let eng_b = Engine::open(
        Arc::clone(&store),
        Arc::clone(&refs_b) as Arc<dyn RefStore>,
        v,
        EngineConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(eng_b.read(0, 4096).await.unwrap(), vec![7u8; 4096]);
    // A's un-flushed write never happened, as far as B can see.
    assert_eq!(eng_b.read(8192, 4096).await.unwrap(), vec![0u8; 4096]);

    // A comes back and tries to land its buffered writes: the fence
    // refuses with the thief's identity — the NBD server maps this to
    // ESHUTDOWN, and the API to the structured 409 `lease_lost`.
    let err = eng_a.commit(CommitKind::Flush).await.unwrap_err();
    assert!(
        matches!(&err, EngineError::Fenced { holder, epoch: 2 } if holder == "node-b"),
        "{err}"
    );
}

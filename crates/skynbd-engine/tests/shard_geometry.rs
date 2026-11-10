//! A volume's shard geometry is recorded in its superblock and adopted on
//! open, so the shard index's positional meaning cannot be reinterpreted
//! by a differently-configured reader (design §5.2).
//!
//! Before that, `EngineConfig::shard_bytes` decided how a tree was read.
//! Committing under a different value than the tree was written with
//! rewrote part of the shard array on a new grid and left the rest, so a
//! rewritten root and a stale one could both claim the same range — and
//! shrinking the geometry truncated the array outright, dropping roots.

use std::sync::Arc;

use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

const K: u64 = 1024;

fn cfg(shard_bytes: u64) -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 4 * K,
        shard_bytes,
        epoch: 0,
        ..EngineConfig::default()
    }
}

/// Opening with a different `shard_bytes` than the volume was created
/// with is not an error and not a reinterpretation — the volume's own
/// geometry wins, so reads and later commits stay on one grid.
#[tokio::test]
async fn geometry_is_adopted_from_the_volume_not_the_config() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(5u128));

    let mut e = Engine::create_volume(store.clone(), refs.clone(), vol, 256 * K, cfg(64 * K))
        .await
        .unwrap();
    e.write(0, &[0xaa; 4096]).unwrap();
    e.write(70 * K, &[0xbb; 4096]).unwrap();
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);

    // Reopened by an engine configured for 1 GiB shards. Under the old
    // behaviour this reinterpreted the array and the next commit rewrote
    // it on the wrong grid; now the superblock decides.
    let mut e = Engine::open(store.clone(), refs.clone(), vol, cfg(1 << 30))
        .await
        .unwrap();
    assert_eq!(e.read(0, 4096).await.unwrap(), vec![0xaa; 4096]);
    assert_eq!(e.read(70 * K, 4096).await.unwrap(), vec![0xbb; 4096]);
    e.write(80 * K, &[0xcc; 4096]).unwrap();
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);

    // Everything survives, under either configuration.
    for shard_bytes in [64 * K, 1 << 30] {
        let e = Engine::open(store.clone(), refs.clone(), vol, cfg(shard_bytes))
            .await
            .unwrap();
        assert_eq!(e.read(0, 4096).await.unwrap(), vec![0xaa; 4096]);
        assert_eq!(e.read(70 * K, 4096).await.unwrap(), vec![0xbb; 4096]);
        assert_eq!(e.read(80 * K, 4096).await.unwrap(), vec![0xcc; 4096]);
    }
}

/// A clone reads its base's shard index verbatim, so it must inherit the
/// base's grid rather than take the cloning engine's configuration.
#[tokio::test]
async fn a_clone_inherits_its_base_geometry() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let base = VolumeId(ulid::Ulid::from(6u128));
    let clone = VolumeId(ulid::Ulid::from(7u128));

    let mut e = Engine::create_volume(store.clone(), refs.clone(), base, 256 * K, cfg(64 * K))
        .await
        .unwrap();
    e.write(70 * K, &[0xdd; 4096]).unwrap();
    let src = e.commit(CommitKind::Flush).await.unwrap();
    drop(e);

    // Cloned by an engine configured for 1 GiB shards, and grown.
    let mut c = Engine::clone_volume(
        store.clone(),
        refs.clone(),
        clone,
        512 * K,
        base,
        src,
        cfg(1 << 30),
    )
    .await
    .unwrap();
    assert_eq!(c.read(70 * K, 4096).await.unwrap(), vec![0xdd; 4096]);
    // Growing the clone extends the shard array; it must not truncate.
    c.write(300 * K, &[0xee; 4096]).unwrap();
    c.commit(CommitKind::Flush).await.unwrap();
    drop(c);

    let c = Engine::open(store.clone(), refs.clone(), clone, cfg(1 << 30))
        .await
        .unwrap();
    assert_eq!(
        c.read(70 * K, 4096).await.unwrap(),
        vec![0xdd; 4096],
        "base content"
    );
    assert_eq!(
        c.read(300 * K, 4096).await.unwrap(),
        vec![0xee; 4096],
        "own write"
    );
}

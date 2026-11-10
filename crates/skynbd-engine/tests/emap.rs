//! The `emap` object (design D-M7): a commit's materialized extent map,
//! written on clean unstage and at every snapshot, loaded in one GET at
//! open. The tree stays the source of truth — the emap is a shortcut
//! that must never make a read wrong, so the proofs here are (a) reads
//! served from the emap *alone* are correct, (b) a broken emap degrades
//! to the walk, (c) GC and fsck treat it as what it is: a cache keyed by
//! its commit.

use skynbd_engine::maintenance::emap_extents;
use skynbd_engine::{fsck, keys, Engine, EngineConfig};
use skynbd_format::Codec;
use skynbd_format::CommitKind;
use skynbd_refs::MemoryRefStore;
use skynbd_refs::RefStore;
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

const KIB: u64 = 1024;

fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 4 * KIB, // seal mid-workload
        shard_bytes: 64 * KIB,      // multi-shard volume
        epoch: 0,
        ..EngineConfig::default()
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

async fn fixture() -> (MemoryStore, MemoryRefStore, VolumeId) {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), vol, 256 * KIB, cfg())
        .await
        .unwrap();
    // Content across multiple shards, with holes and zeroes.
    eng.write(0, &pattern(8192, 0xA)).unwrap();
    eng.write(100 * KIB, &pattern(4096, 0xB)).unwrap();
    eng.write_zeroes(512, 512).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    eng.write_emap().await.unwrap();
    (store, refs, vol)
}

/// Every subtree node except the shard index itself is deleted; opening
/// and reading must still return the exact content — only the emap can
/// be serving it. This is the D-M7 claim with the fallback amputated.
#[tokio::test]
async fn reads_are_served_from_the_emap_alone() {
    let (store, refs, vol) = fixture().await;

    // The shard index node must survive (open reads it to learn the
    // roots); every *subtree* node goes.
    let head = refs
        .get(vol, "head")
        .await
        .unwrap()
        .expect("head exists")
        .commit;
    let commit =
        skynbd_format::Commit::decode(&store.get(&keys::commit(vol, &head), None).await.unwrap())
            .unwrap();
    let index_key = keys::node(vol, &commit.shard_index);
    for key in store
        .list(&format!("{}node/", keys::volume_prefix(vol)))
        .await
        .unwrap()
    {
        if key != index_key {
            store.delete(&key).await.unwrap();
        }
    }

    let eng = Engine::open(store.clone(), refs.clone(), vol, cfg())
        .await
        .unwrap();
    let mut expect = pattern(8192, 0xA);
    expect[512..1024].fill(0);
    assert_eq!(eng.read(0, 8192).await.unwrap(), expect);
    assert_eq!(eng.read(100 * KIB, 4096).await.unwrap(), pattern(4096, 0xB));
    assert_eq!(eng.read(200 * KIB, 512).await.unwrap(), vec![0u8; 512]);
}

/// An emap that fails to decode costs a warning and a tree walk, never
/// an IO error — the fallback the whole design leans on.
#[tokio::test]
async fn a_corrupt_emap_degrades_to_the_walk() {
    let (store, refs, vol) = fixture().await;
    let head = refs.get(vol, "head").await.unwrap().unwrap().commit;
    store
        .put(
            &keys::emap(vol, &head),
            bytes::Bytes::from_static(b"SEMPgarbage"),
        )
        .await
        .unwrap();

    let eng = Engine::open(store.clone(), refs.clone(), vol, cfg())
        .await
        .unwrap();
    let mut expect = pattern(8192, 0xA);
    expect[512..1024].fill(0);
    assert_eq!(eng.read(0, 8192).await.unwrap(), expect);
}

/// Snapshots leave their commit's emap behind (the D-M7 cadence), and a
/// snapshot export opens through it.
#[tokio::test]
async fn snapshots_write_their_emap() {
    let (store, refs, vol) = fixture().await;
    let mut eng = Engine::open(store.clone(), refs.clone(), vol, cfg())
        .await
        .unwrap();
    eng.write(0, &pattern(1024, 0xC)).unwrap();
    let (_, snap_commit) = eng.snapshot_create("s1").await.unwrap();
    assert!(
        store.exists(&keys::emap(vol, &snap_commit)).await.unwrap(),
        "snapshot must leave its emap behind"
    );
    // The emap agrees with the tree, extent for extent.
    let from_emap = emap_extents(&store, vol, &snap_commit)
        .await
        .unwrap()
        .expect("emap present");
    let walked = skynbd_engine::commit_extents(&store, vol, &snap_commit)
        .await
        .unwrap();
    assert_eq!(from_emap, walked);
}

/// An emap lives exactly as long as its commit: GC sweeps the dead
/// snapshot's emap and keeps the head's.
#[tokio::test]
async fn gc_sweeps_the_emap_with_its_commit() {
    let (store, refs, vol) = fixture().await;
    let mut eng = Engine::open(store.clone(), refs.clone(), vol, cfg())
        .await
        .unwrap();
    let (_, snap_commit) = eng.snapshot_create("doomed").await.unwrap();

    // Move the head past the snapshot commit, then delete the snapshot.
    eng.write(0, &pattern(512, 0xD)).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    eng.write_emap().await.unwrap();
    let head_commit = refs.get(vol, "head").await.unwrap().unwrap().commit;
    eng.snapshot_delete("doomed").await.unwrap();

    eng.gc().await.unwrap();
    assert!(
        !store.exists(&keys::emap(vol, &snap_commit)).await.unwrap(),
        "dead commit's emap must be swept"
    );
    assert!(
        store.exists(&keys::emap(vol, &head_commit)).await.unwrap(),
        "live head's emap must survive"
    );
}

/// fsck compares the emap against the tree and reports a disagreement —
/// the check that keeps "cache" from quietly becoming "second truth".
#[tokio::test]
async fn fsck_reports_an_emap_that_disagrees() {
    let (store, refs, vol) = fixture().await;
    let report = fsck(&store, &refs, vol).await.unwrap();
    assert!(
        report.refs.iter().all(|(_, e)| e.is_none()),
        "matching emap must pass: {report:?}"
    );

    // Replace the head's emap with a well-formed map of the wrong
    // content: one bogus zero extent.
    let head = refs.get(vol, "head").await.unwrap().unwrap().commit;
    let wrong = skynbd_format::encode_emap(
        head,
        cfg().shard_bytes,
        vec![],
        vec![Some(skynbd_format::EmapShard {
            gap: vec![0],
            len: vec![512],
            target: vec![skynbd_format::EMAP_ZERO],
            offset: vec![0],
        })],
    )
    .unwrap();
    store
        .put(&keys::emap(vol, &head), wrong.into())
        .await
        .unwrap();

    let report = fsck(&store, &refs, vol).await.unwrap();
    let head_err = report
        .refs
        .iter()
        .find(|(n, _)| n == "head")
        .and_then(|(_, e)| e.clone());
    assert!(
        head_err.is_some_and(|e| e.contains("emap disagrees")),
        "{report:?}"
    );
}

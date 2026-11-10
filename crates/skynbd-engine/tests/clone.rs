//! Fast-clone tests (CSI design §4.1 option A): cross-volume base
//! references. The clone's initial commit points at the source snapshot's
//! shard index; reads fall through to the source namespace, new writes
//! land in the clone's own. The GC/fsck matrix here is the spike the
//! design called for: both sides collect and verify with shared objects,
//! before and after divergence, across chained clones and reopens.

use skynbd_engine::{fsck, Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefOp, RefStore};
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

const KIB: u64 = 1024;
const VOL: u64 = 256 * KIB;

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

struct Rig {
    store: MemoryStore,
    refs: MemoryRefStore,
    src: VolumeId,
    src_engine: Engine<MemoryStore, MemoryRefStore>,
}

/// Source volume with two committed 4k writes and snapshot `s1`.
async fn source_with_snapshot() -> (Rig, [u8; 32]) {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let src = VolumeId(ulid::Ulid::new());
    let mut e = Engine::create_volume(store.clone(), refs.clone(), src, VOL, cfg())
        .await
        .unwrap();
    e.write(0, &pattern(4096, 1)).unwrap();
    e.write(128 * KIB, &pattern(4096, 2)).unwrap();
    let (_, commit) = e.snapshot_create("s1").await.unwrap();
    (
        Rig {
            store,
            refs,
            src,
            src_engine: e,
        },
        commit,
    )
}

/// The API layer's pin, done directly against the refs store here.
async fn pin_clone(refs: &MemoryRefStore, src: VolumeId, snap: &str, clone_name: &str) {
    refs.transact(
        src,
        vec![RefOp::Pin {
            name: format!("snap/{snap}"),
            holder: format!("clone:{clone_name}"),
        }],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn clone_reads_base_content_then_diverges() {
    let (mut rig, commit) = source_with_snapshot().await;
    let clone_id = VolumeId(ulid::Ulid::new());
    pin_clone(&rig.refs, rig.src, "s1", "c1").await;
    let mut c = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        clone_id,
        VOL,
        rig.src,
        commit,
        cfg(),
    )
    .await
    .unwrap();

    // Clone sees the snapshot's content without owning any data objects.
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 1));
    assert_eq!(c.read(128 * KIB, 4096).await.unwrap(), pattern(4096, 2));
    assert_eq!(c.read(4096, 4096).await.unwrap(), vec![0u8; 4096]);

    // Divergence: the clone's write is invisible to the source, and the
    // source's post-snapshot write is invisible to the clone.
    c.write(0, &pattern(4096, 9)).unwrap();
    c.commit(CommitKind::Flush).await.unwrap();
    rig.src_engine.write(128 * KIB, &pattern(4096, 7)).unwrap();
    rig.src_engine.commit(CommitKind::Flush).await.unwrap();

    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 9));
    assert_eq!(c.read(128 * KIB, 4096).await.unwrap(), pattern(4096, 2));
    assert_eq!(
        rig.src_engine.read(0, 4096).await.unwrap(),
        pattern(4096, 1)
    );
    assert_eq!(
        rig.src_engine.read(128 * KIB, 4096).await.unwrap(),
        pattern(4096, 7)
    );

    // Reopen the clone from durable state: the base chain is rediscovered
    // from the clone-base marker.
    drop(c);
    let c = Engine::open(rig.store.clone(), rig.refs.clone(), clone_id, cfg())
        .await
        .unwrap();
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 9));
    assert_eq!(c.read(128 * KIB, 4096).await.unwrap(), pattern(4096, 2));
}

#[tokio::test]
async fn clone_size_may_only_grow_and_grown_tail_reads_zero() {
    let (rig, commit) = source_with_snapshot().await;
    let too_small = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        VolumeId(ulid::Ulid::new()),
        VOL - 4096,
        rig.src,
        commit,
        cfg(),
    )
    .await;
    assert!(matches!(too_small, Err(EngineError::InvalidIo(_))));

    let grown = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        VolumeId(ulid::Ulid::new()),
        VOL + 64 * KIB,
        rig.src,
        commit,
        cfg(),
    )
    .await
    .unwrap();
    assert_eq!(grown.size(), VOL + 64 * KIB);
    assert_eq!(grown.read(0, 4096).await.unwrap(), pattern(4096, 1));
    assert_eq!(grown.read(VOL, 4096).await.unwrap(), vec![0u8; 4096]);
}

/// The spike question: do GC and fsck stay correct with cross-volume
/// references? Source GC must keep shared objects live (the pinned snap
/// ref covers them); clone GC must not touch base-namespace objects; both
/// fscks verify the full (cross-namespace) closure.
#[tokio::test]
async fn gc_and_fsck_on_both_sides_preserve_shared_objects() {
    let (mut rig, commit) = source_with_snapshot().await;
    let clone_id = VolumeId(ulid::Ulid::new());
    pin_clone(&rig.refs, rig.src, "s1", "c1").await;
    let mut c = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        clone_id,
        VOL,
        rig.src,
        commit,
        cfg(),
    )
    .await
    .unwrap();
    c.write(64 * KIB, &pattern(4096, 5)).unwrap();
    c.commit(CommitKind::Flush).await.unwrap();
    rig.src_engine.write(0, &pattern(4096, 6)).unwrap();
    rig.src_engine.commit(CommitKind::Flush).await.unwrap();

    // GC both sides (source first, then clone, then source again for
    // good measure — order must not matter).
    rig.src_engine.gc().await.unwrap();
    c.gc().await.unwrap();
    rig.src_engine.gc().await.unwrap();

    // Everything still reads correctly.
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 1));
    assert_eq!(c.read(64 * KIB, 4096).await.unwrap(), pattern(4096, 5));
    assert_eq!(c.read(128 * KIB, 4096).await.unwrap(), pattern(4096, 2));
    assert_eq!(
        rig.src_engine.read(0, 4096).await.unwrap(),
        pattern(4096, 6)
    );

    // Both fscks verify their full closures, across namespaces.
    let src_report = fsck(&rig.store, &rig.refs, rig.src).await.unwrap();
    assert!(src_report.ok(), "{src_report:?}");
    let clone_report = fsck(&rig.store, &rig.refs, clone_id).await.unwrap();
    assert!(clone_report.ok(), "{clone_report:?}");

    // A reopened clone still reads correctly after both GCs.
    drop(c);
    let c = Engine::open(rig.store.clone(), rig.refs.clone(), clone_id, cfg())
        .await
        .unwrap();
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 1));
}

/// Without the pin, deleting the source snapshot and GCing the source
/// sweeps the shared closure — the clone must then FAIL loudly (fsck
/// reports it), proving the pin is load-bearing, not decorative.
#[tokio::test]
async fn unpinned_source_gc_breaks_the_clone_and_fsck_says_so() {
    let (mut rig, commit) = source_with_snapshot().await;
    let clone_id = VolumeId(ulid::Ulid::new());
    // Deliberately NO pin.
    let c = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        clone_id,
        VOL,
        rig.src,
        commit,
        cfg(),
    )
    .await
    .unwrap();
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 1));

    rig.src_engine.snapshot_delete("s1").await.unwrap();
    // Move the source head past the snapshot commit so its objects die.
    rig.src_engine.write(0, &pattern(4096, 8)).unwrap();
    rig.src_engine.commit(CommitKind::Flush).await.unwrap();
    rig.src_engine.gc().await.unwrap();

    let report = fsck(&rig.store, &rig.refs, clone_id).await.unwrap();
    assert!(!report.ok(), "clone fsck should report the swept base");
}

#[tokio::test]
async fn pinned_source_snapshot_refuses_delete() {
    let (mut rig, _commit) = source_with_snapshot().await;
    pin_clone(&rig.refs, rig.src, "s1", "c1").await;
    let err = rig.src_engine.snapshot_delete("s1").await.unwrap_err();
    assert!(matches!(err, EngineError::Pinned(_)), "{err}");
    // Released pin → deletable.
    rig.refs
        .transact(
            rig.src,
            vec![RefOp::Unpin {
                name: "snap/s1".into(),
                holder: "clone:c1".into(),
            }],
        )
        .await
        .unwrap();
    rig.src_engine.snapshot_delete("s1").await.unwrap();
}

/// Clone of a clone: resolution falls through the whole chain, and the
/// middle volume's snapshot pin keeps the grandparent alive transitively
/// (its closure references the grandparent's objects).
#[tokio::test]
async fn chained_clones_resolve_through_the_full_chain() {
    let (rig, commit) = source_with_snapshot().await;
    let b_id = VolumeId(ulid::Ulid::new());
    pin_clone(&rig.refs, rig.src, "s1", "b").await;
    let mut b = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        b_id,
        VOL,
        rig.src,
        commit,
        cfg(),
    )
    .await
    .unwrap();
    b.write(64 * KIB, &pattern(4096, 5)).unwrap();
    let (_, b_snap_commit) = b.snapshot_create("s2").await.unwrap();

    let c_id = VolumeId(ulid::Ulid::new());
    pin_clone(&rig.refs, b_id, "s2", "c").await;
    let mut c = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        c_id,
        VOL,
        b_id,
        b_snap_commit,
        cfg(),
    )
    .await
    .unwrap();

    // Grandparent data, parent data, and own writes all resolve.
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 1));
    assert_eq!(c.read(64 * KIB, 4096).await.unwrap(), pattern(4096, 5));
    c.write(128 * KIB, &pattern(4096, 3)).unwrap();
    c.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(c.read(128 * KIB, 4096).await.unwrap(), pattern(4096, 3));

    // All three fsck clean; GC on every level changes nothing for c.
    b.gc().await.unwrap();
    c.gc().await.unwrap();
    for vol in [rig.src, b_id, c_id] {
        let report = fsck(&rig.store, &rig.refs, vol).await.unwrap();
        assert!(report.ok(), "vol {vol}: {report:?}");
    }
    assert_eq!(c.read(0, 4096).await.unwrap(), pattern(4096, 1));
}

/// Snapshots and exports of a clone work like any volume's: the snapshot
/// commit's closure spans namespaces and `open_at_commit` (the snapshot
/// export path) resolves it.
#[tokio::test]
async fn clone_snapshots_serve_via_open_at_commit() {
    let (rig, commit) = source_with_snapshot().await;
    let clone_id = VolumeId(ulid::Ulid::new());
    pin_clone(&rig.refs, rig.src, "s1", "c1").await;
    let mut c = Engine::clone_volume(
        rig.store.clone(),
        rig.refs.clone(),
        clone_id,
        VOL,
        rig.src,
        commit,
        cfg(),
    )
    .await
    .unwrap();
    c.write(4096, &pattern(4096, 4)).unwrap();
    let (_, snap_commit) = c.snapshot_create("cs").await.unwrap();

    let ro = Engine::open_at_commit(
        rig.store.clone(),
        rig.refs.clone(),
        clone_id,
        snap_commit,
        cfg(),
    )
    .await
    .unwrap();
    assert_eq!(ro.read(0, 4096).await.unwrap(), pattern(4096, 1)); // base
    assert_eq!(ro.read(4096, 4096).await.unwrap(), pattern(4096, 4)); // own
}

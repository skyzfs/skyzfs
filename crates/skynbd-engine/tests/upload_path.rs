//! The engine side of SkyZFS's upload (M2.4a): sealed segments PUT
//! before their commit, the group and the extra ref ops a commit carries,
//! and the shard size chosen at create.
//!
//! The upload of a cut writes the cut's blocks into an engine opened at
//! the volume's head and commits them. The first upload of a volume is
//! the whole volume, so its segments cannot wait in memory for the
//! commit; the commit records the cut's group, and the cut's identity
//! lands in the same refs transaction as the head swap.

use std::sync::Arc;

use skynbd_engine::{fsck, keys, Engine, EngineConfig, EngineError};
use skynbd_format::{Codec, Commit, CommitKind};
use skynbd_refs::{MemoryRefStore, ObjectRefStore, RefOp, RefStore};
use skynbd_store::{FaultOp, FaultStore, FaultWhen, MemoryStore, ObjectStore};
use skynbd_types::{CommitHash, VolumeId};

const KIB: u64 = 1024;
const MIB: u64 = 1024 * KIB;
/// The block, and the seal threshold: every block written whole seals
/// the open segment.
const BLOCK: u64 = 4 * KIB;
const CUT_ROW: &str = "skyzfs/cut";

fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: BLOCK,
        shard_bytes: 64 * KIB,
        ..EngineConfig::default()
    }
}

fn vol(n: u128) -> VolumeId {
    VolumeId(ulid::Ulid::from(n))
}

fn block(seed: u8) -> Vec<u8> {
    (0..BLOCK as usize)
        .map(|i| seed.wrapping_add(i as u8))
        .collect()
}

async fn commit_of<S: ObjectStore>(store: &S, v: VolumeId, hash: &CommitHash) -> Commit {
    Commit::decode(&store.get(&keys::commit(v, hash), None).await.unwrap()).unwrap()
}

async fn segments_in<S: ObjectStore>(store: &S, v: VolumeId) -> usize {
    store
        .list(&format!("{}seg/", keys::volume_prefix(v)))
        .await
        .unwrap()
        .len()
}

fn row(value: &[u8]) -> RefOp {
    RefOp::MetaPut {
        key: CUT_ROW.into(),
        value: value.to_vec(),
    }
}

// ------------------------------------------- sealed segments PUT early

/// Sealed segments go to the store before their commit and leave memory,
/// so that the first upload of a whole volume is not held in memory until
/// its commit. Reads of them then come from the store, and the commit
/// lists each once without a second PUT.
#[tokio::test]
async fn sealed_segments_are_put_before_the_commit_and_leave_memory() {
    let store = FaultStore::new(MemoryStore::new());
    let refs = MemoryRefStore::new();
    let v = vol(1);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    let head = refs.get(v, "head").await.unwrap();

    for i in 0..3u8 {
        eng.write(u64::from(i) * 2 * BLOCK, &block(i)).unwrap();
    }
    // Part of a block: it stays in the open segment.
    eng.write(7 * BLOCK, &block(9)[..512]).unwrap();
    assert_eq!(eng.held_segment_bytes(), 3 * BLOCK + 512);

    eng.put_sealed().await.unwrap();
    assert_eq!(segments_in(store.inner(), v).await, 3);
    assert_eq!(eng.held_segment_bytes(), 512);
    assert_eq!(
        refs.get(v, "head").await.unwrap(),
        head,
        "nothing committed"
    );

    // Their reads come from the store now; the open segment's from memory.
    store.arm(FaultOp::Any, "/seg/", 0).await;
    assert!(eng.read(0, BLOCK).await.is_err());
    assert_eq!(eng.read(7 * BLOCK, 512).await.unwrap(), &block(9)[..512]);
    store.disarm().await;
    for i in 0..3u8 {
        assert_eq!(
            eng.read(u64::from(i) * 2 * BLOCK, BLOCK).await.unwrap(),
            block(i)
        );
    }

    // The commit PUTs the open segment and nothing else: a second PUT of
    // a segment would fail.
    store.arm(FaultOp::Put, "/seg/", 1).await;
    let hash = eng.commit(CommitKind::Flush).await.unwrap();
    store.disarm().await;
    let commit = commit_of(store.inner(), v, &hash).await;
    let lens: Vec<u64> = commit.segments.iter().map(|s| s.len).collect();
    assert_eq!(
        lens,
        vec![BLOCK, BLOCK, BLOCK, 512],
        "seal order, each once"
    );
    assert_eq!(eng.held_segment_bytes(), 0);
    assert!(fsck(store.inner(), &refs, v).await.unwrap().ok());

    drop(eng);
    let eng = Engine::open(store.clone(), refs.clone(), v, cfg())
        .await
        .unwrap();
    for i in 0..3u8 {
        assert_eq!(
            eng.read(u64::from(i) * 2 * BLOCK, BLOCK).await.unwrap(),
            block(i)
        );
    }
    assert_eq!(eng.read(7 * BLOCK, 512).await.unwrap(), &block(9)[..512]);
}

/// A sealed segment whose PUT fails keeps its bytes, and so do those
/// sealed after it. The next call PUTs them, and the commit lists each
/// segment once.
#[tokio::test]
async fn a_segment_whose_put_failed_stays_in_memory_and_is_put_by_the_next_call() {
    let store = FaultStore::new(MemoryStore::new());
    let refs = MemoryRefStore::new();
    let v = vol(2);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    for i in 0..3u8 {
        eng.write(u64::from(i) * BLOCK, &block(i)).unwrap();
    }

    store.arm(FaultOp::Put, "/seg/", 1).await;
    assert!(eng.put_sealed().await.is_err());
    store.disarm().await;
    assert_eq!(segments_in(store.inner(), v).await, 1);
    assert_eq!(eng.held_segment_bytes(), 2 * BLOCK);
    for i in 0..3u8 {
        assert_eq!(
            eng.read(u64::from(i) * BLOCK, BLOCK).await.unwrap(),
            block(i)
        );
    }

    eng.put_sealed().await.unwrap();
    assert_eq!(segments_in(store.inner(), v).await, 3);
    assert_eq!(eng.held_segment_bytes(), 0);
    let hash = eng.commit(CommitKind::Flush).await.unwrap();
    let commit = commit_of(store.inner(), v, &hash).await;
    let mut ids: Vec<_> = commit.segments.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), 3);
    ids.dedup();
    assert_eq!(ids.len(), 3, "each segment once");

    drop(eng);
    let eng = Engine::open(store.clone(), refs.clone(), v, cfg())
        .await
        .unwrap();
    for i in 0..3u8 {
        assert_eq!(
            eng.read(u64::from(i) * BLOCK, BLOCK).await.unwrap(),
            block(i)
        );
    }
}

/// Discarding the uncommitted writes forgets the segments already PUT:
/// the next commit does not list them (they are orphans for GC, as a
/// failed commit's segments are).
#[tokio::test]
async fn discarded_writes_take_their_put_segments_out_of_the_next_commit() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(3);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    eng.write(0, &block(1)).unwrap();
    eng.write(BLOCK, &block(2)).unwrap();
    eng.put_sealed().await.unwrap();
    eng.discard_uncommitted();
    assert_eq!(eng.held_segment_bytes(), 0);

    eng.write(4 * BLOCK, &block(3)).unwrap();
    let hash = eng.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(commit_of(&store, v, &hash).await.segments.len(), 1);
    drop(eng);
    let eng = Engine::open(store.clone(), refs.clone(), v, cfg())
        .await
        .unwrap();
    assert_eq!(
        eng.read(0, 2 * BLOCK).await.unwrap(),
        vec![0; 2 * BLOCK as usize]
    );
    assert_eq!(eng.read(4 * BLOCK, BLOCK).await.unwrap(), block(3));
}

/// One upload as the daemon makes it: blocks written, sealed segments
/// PUT, more blocks and a run of zeros, then the commit with the cut's
/// group and its row in the head swap's transaction.
async fn upload<S: ObjectStore, R: RefStore>(eng: &mut Engine<S, R>) -> Result<(), EngineError> {
    eng.write(0, &block(20))?;
    eng.write(2 * BLOCK, &block(21))?;
    eng.put_sealed().await?;
    eng.write_zeroes(BLOCK, BLOCK)?;
    eng.write(5 * BLOCK, &block(22))?;
    // The next shard.
    eng.write(80 * KIB, &block(23))?;
    eng.put_sealed().await?;
    eng.set_group(Some(41));
    eng.commit_with(CommitKind::Flush, vec![row(b"cut 1")])
        .await?;
    Ok(())
}

/// The volume before the upload: blocks 0 to 3.
fn before() -> Vec<u8> {
    let mut v = vec![0u8; 96 * KIB as usize];
    for i in 0..4u8 {
        let at = usize::from(i) * BLOCK as usize;
        v[at..at + BLOCK as usize].copy_from_slice(&block(10 + i));
    }
    v
}

/// The volume after it.
fn after() -> Vec<u8> {
    let mut v = before();
    let mut put = |at: u64, data: &[u8]| {
        v[at as usize..at as usize + data.len()].copy_from_slice(data);
    };
    put(0, &block(20));
    put(BLOCK, &vec![0; BLOCK as usize]);
    put(2 * BLOCK, &block(21));
    put(5 * BLOCK, &block(22));
    put(80 * KIB, &block(23));
    v
}

/// A death at every store operation of an upload, before the operation
/// and after it: a reopen finds the volume as it was or the upload whole,
/// with the cut's row exactly when the upload is there, and the upload
/// made again completes. The refs object is in the same store, so the
/// head swap is one of the operations: the after-mode on it is "the swap
/// landed, then the daemon died", which must show the upload although
/// the uploader heard an error.
#[tokio::test]
async fn a_death_at_any_operation_of_an_upload_leaves_the_old_head_or_the_new_whole() {
    for when in [FaultWhen::Before, FaultWhen::After] {
        let mut landed_unheard = 0;
        let mut points = 0;
        for k in 0.. {
            let store = FaultStore::new(MemoryStore::new());
            let v = vol(4);
            let refs = Arc::new(ObjectRefStore::new(store.clone()));
            let mut eng = Engine::create_volume(store.clone(), refs, v, 96 * KIB, cfg())
                .await
                .unwrap();
            for i in 0..4u8 {
                eng.write(u64::from(i) * BLOCK, &block(10 + i)).unwrap();
            }
            eng.commit(CommitKind::Flush).await.unwrap();

            store.arm_when(FaultOp::Any, "", k, when).await;
            let finished = upload(&mut eng).await.is_ok();
            drop(eng); // the daemon died
            store.disarm().await;

            let refs = Arc::new(ObjectRefStore::new(store.clone()));
            let mut eng = Engine::open(store.clone(), refs.clone(), v, cfg())
                .await
                .unwrap_or_else(|e| panic!("{when:?} at op {k}: reopen failed: {e}"));
            let seen = eng.read(0, 96 * KIB).await.unwrap();
            let row_seen = refs.meta_get(v, CUT_ROW).await.unwrap();
            let is_new = seen == after();
            assert!(
                is_new || seen == before(),
                "{when:?} at op {k}: a part of the upload is visible"
            );
            assert_eq!(
                row_seen.is_some(),
                is_new,
                "{when:?} at op {k}: the row and the head did not land together"
            );
            assert_eq!(eng.head_group(), is_new.then_some(41), "{when:?} at op {k}");
            if finished {
                assert!(is_new, "{when:?} at op {k}: a finished upload is not there");
                break;
            }
            points += 1;
            if is_new {
                landed_unheard += 1;
            } else {
                upload(&mut eng)
                    .await
                    .unwrap_or_else(|e| panic!("{when:?} at op {k}: the upload again: {e}"));
                assert_eq!(eng.read(0, 96 * KIB).await.unwrap(), after());
            }
            assert!(
                fsck(&store, &refs, v).await.unwrap().ok(),
                "{when:?} at op {k}"
            );
        }
        assert!(points > 10, "{when:?}: only {points} kill points");
        match when {
            // Before the swap, nothing of the upload is visible.
            FaultWhen::Before => assert_eq!(landed_unheard, 0),
            // The swap landed and its reply was lost: exactly that point.
            FaultWhen::After => assert_eq!(landed_unheard, 1),
        }
    }
}

// --------------------------------------------- the group and the ops

/// The group set before a commit goes into that commit and not the next:
/// SkyZFS records there the transaction group of the cut it uploads. A
/// commit that fails keeps it for its retry.
#[tokio::test]
async fn a_group_goes_into_the_next_commit_only() {
    let store = FaultStore::new(MemoryStore::new());
    let refs = MemoryRefStore::new();
    let v = vol(5);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    assert_eq!(eng.head_group(), None);

    eng.set_group(Some(77));
    eng.write(0, &block(1)).unwrap();
    let h1 = eng.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(commit_of(store.inner(), v, &h1).await.group, Some(77));
    assert_eq!(eng.head_group(), Some(77));

    eng.write(BLOCK, &block(2)).unwrap();
    let h2 = eng.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(commit_of(store.inner(), v, &h2).await.group, None);
    assert_eq!(eng.head_group(), None);

    eng.set_group(Some(78));
    eng.write(2 * BLOCK, &block(3)).unwrap();
    store.arm(FaultOp::Put, "/commit/", 0).await;
    assert!(eng.commit(CommitKind::Flush).await.is_err());
    store.disarm().await;
    let h3 = eng.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(commit_of(store.inner(), v, &h3).await.group, Some(78));

    drop(eng);
    let eng = Engine::open(store.clone(), refs.clone(), v, cfg())
        .await
        .unwrap();
    assert_eq!(eng.head_group(), Some(78), "read from the head's commit");
}

/// A group or ops make a commit even with nothing written since the last
/// one: they are what it records. With neither, a commit is still none.
#[tokio::test]
async fn a_group_or_ops_without_writes_still_make_a_commit() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(6);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    eng.write(0, &block(1)).unwrap();
    let h0 = eng.commit(CommitKind::Flush).await.unwrap();
    assert_eq!(eng.commit(CommitKind::Flush).await.unwrap(), h0);

    eng.set_group(Some(5));
    let h1 = eng.commit(CommitKind::Flush).await.unwrap();
    assert_ne!(h1, h0);
    let c1 = commit_of(&store, v, &h1).await;
    assert_eq!((c1.parent, c1.group), (Some(h0), Some(5)));
    assert!(c1.segments.is_empty());

    let h2 = eng
        .commit_with(CommitKind::Flush, vec![row(b"cut 5")])
        .await
        .unwrap();
    assert_ne!(h2, h1);
    assert_eq!(refs.get(v, "head").await.unwrap().unwrap().commit, h2);
    assert_eq!(
        refs.meta_get(v, CUT_ROW).await.unwrap().as_deref(),
        Some(&b"cut 5"[..])
    );
    assert_eq!(eng.read(0, BLOCK).await.unwrap(), block(1));
}

/// The ops given with a commit land in the head swap's transaction.
#[tokio::test]
async fn a_commit_with_ops_lands_them_with_the_head_swap() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(7);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    eng.write(0, &block(1)).unwrap();
    eng.set_group(Some(9));
    let other = RefOp::MetaPut {
        key: "skyzfs/other".into(),
        value: b"b".to_vec(),
    };
    let hash = eng
        .commit_with(CommitKind::Flush, vec![row(b"a"), other])
        .await
        .unwrap();
    let head = refs.get(v, "head").await.unwrap().unwrap();
    assert_eq!(head.commit, hash);
    assert_eq!(
        refs.meta_get(v, CUT_ROW).await.unwrap().as_deref(),
        Some(&b"a"[..])
    );
    assert_eq!(
        refs.meta_get(v, "skyzfs/other").await.unwrap().as_deref(),
        Some(&b"b"[..])
    );
    assert_eq!(commit_of(&store, v, &hash).await.group, Some(9));

    // The engine's idea of the head is the stored one: the next commit
    // swaps from it.
    eng.write(BLOCK, &block(2)).unwrap();
    let next = eng.commit(CommitKind::Flush).await.unwrap();
    let head2 = refs.get(v, "head").await.unwrap().unwrap();
    assert_eq!(
        (head2.commit, head2.generation),
        (next, head.generation + 1)
    );
}

/// An op that does not hold fails the whole commit: neither the head nor
/// any op lands, and the engine keeps its writes and its group for the
/// commit made again.
#[tokio::test]
async fn a_commit_whose_op_does_not_hold_lands_nothing_and_can_be_made_again() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(8);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    refs.transact(v, vec![row(b"old")]).await.unwrap();
    let head = refs.get(v, "head").await.unwrap();

    eng.write(0, &block(3)).unwrap();
    eng.set_group(Some(4));
    let stray = RefOp::MetaPut {
        key: "skyzfs/stray".into(),
        value: b"x".to_vec(),
    };
    let wrong = RefOp::MetaCas {
        key: CUT_ROW.into(),
        expected: Some(b"wrong".to_vec()),
        value: b"new".to_vec(),
    };
    let err = eng
        .commit_with(CommitKind::Flush, vec![stray, wrong])
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::OpConflict { op_index: 1 }),
        "{err}"
    );
    assert_eq!(refs.get(v, "head").await.unwrap(), head);
    assert_eq!(refs.meta_get(v, "skyzfs/stray").await.unwrap(), None);
    assert_eq!(
        refs.meta_get(v, CUT_ROW).await.unwrap().as_deref(),
        Some(&b"old"[..])
    );
    assert_eq!(eng.read(0, BLOCK).await.unwrap(), block(3));

    let right = RefOp::MetaCas {
        key: CUT_ROW.into(),
        expected: Some(b"old".to_vec()),
        value: b"new".to_vec(),
    };
    let hash = eng
        .commit_with(CommitKind::Flush, vec![right])
        .await
        .unwrap();
    assert_eq!(refs.get(v, "head").await.unwrap().unwrap().commit, hash);
    assert_eq!(commit_of(&store, v, &hash).await.group, Some(4));
    assert_eq!(
        refs.meta_get(v, CUT_ROW).await.unwrap().as_deref(),
        Some(&b"new"[..])
    );
}

/// A head moved by another writer fails the commit and lands none of its
/// ops.
#[tokio::test]
async fn a_commit_with_ops_after_the_head_moved_lands_nothing() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(9);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    let head = refs.get(v, "head").await.unwrap().unwrap();
    refs.compare_and_swap(v, "head", Some(head), Some(head.commit))
        .await
        .unwrap();
    eng.write(0, &block(1)).unwrap();
    let err = eng
        .commit_with(CommitKind::Flush, vec![row(b"a")])
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Conflict), "{err}");
    assert_eq!(refs.meta_get(v, CUT_ROW).await.unwrap(), None);
}

/// The ops may not swap the ref the engine commits to: the commit is
/// refused before anything is written.
#[tokio::test]
async fn ops_that_swap_the_engines_own_ref_are_refused() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(10);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    let head = refs.get(v, "head").await.unwrap().unwrap();
    eng.write(0, &block(1)).unwrap();
    let own = RefOp::CasRef {
        name: "head".into(),
        expected: Some(head),
        new_commit: Some(head.commit),
    };
    let err = eng
        .commit_with(CommitKind::Flush, vec![own])
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::InvalidIo(_)), "{err}");
    assert_eq!(refs.get(v, "head").await.unwrap(), Some(head));
    assert_eq!(segments_in(&store, v).await, 0, "nothing was written");
}

/// With the refs object in the store, the head and the cut's row are one
/// PUT: a death before it lands neither, a death after it both, although
/// the engine heard an error.
#[tokio::test]
async fn the_swap_and_its_row_land_together_or_not_at_all() {
    for when in [FaultWhen::Before, FaultWhen::After] {
        let store = FaultStore::new(MemoryStore::new());
        let v = vol(11);
        let refs = Arc::new(ObjectRefStore::new(store.clone()));
        let mut eng = Engine::create_volume(store.clone(), refs, v, 256 * KIB, cfg())
            .await
            .unwrap();
        eng.write(0, &block(1)).unwrap();
        eng.set_group(Some(12));
        store.arm_when(FaultOp::Put, "/refs", 0, when).await;
        assert!(eng
            .commit_with(CommitKind::Flush, vec![row(b"cut 12")])
            .await
            .is_err());
        drop(eng);
        store.disarm().await;

        let refs = Arc::new(ObjectRefStore::new(store.clone()));
        let eng = Engine::open(store.clone(), refs.clone(), v, cfg())
            .await
            .unwrap();
        let row_seen = refs.meta_get(v, CUT_ROW).await.unwrap();
        match when {
            FaultWhen::Before => {
                assert_eq!(row_seen, None);
                assert_eq!(eng.head_group(), None);
                assert_eq!(eng.read(0, BLOCK).await.unwrap(), vec![0; BLOCK as usize]);
            }
            FaultWhen::After => {
                assert_eq!(row_seen.as_deref(), Some(&b"cut 12"[..]));
                assert_eq!(eng.head_group(), Some(12));
                assert_eq!(eng.read(0, BLOCK).await.unwrap(), block(1));
            }
        }
    }
}

// ------------------------------------------------------- the shard size

/// An object of its own for block `b`: no two blocks attached with these
/// can merge into one extent.
fn object_of(b: u64) -> [u8; 32] {
    let mut h = [0u8; 32];
    h[..8].copy_from_slice(&b.to_le_bytes());
    h
}

/// SkyZFS creates its volumes with 128 MiB shards. A shard's tree holds
/// at most 256 × 256 extents today, and 128 MiB of 4 KiB blocks is
/// 32,768: a shard in which every block is an extent of its own still
/// commits. The size is the volume's, recorded at create, and a reader
/// configured for another adopts it.
#[tokio::test]
async fn a_shard_of_128_mib_holds_every_block_as_an_extent_of_its_own() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(12);
    let config = EngineConfig {
        shard_bytes: 128 * MIB,
        ..EngineConfig::default()
    };
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * MIB, config)
        .await
        .unwrap();
    for b in 0..128 * MIB / BLOCK {
        eng.attach_cas(b * BLOCK, BLOCK, object_of(b), 0).unwrap();
    }
    eng.write(128 * MIB, &block(1)).unwrap();
    eng.commit(CommitKind::Flush).await.unwrap();
    drop(eng);

    let eng = Engine::open(store.clone(), refs.clone(), v, EngineConfig::default())
        .await
        .unwrap();
    assert_eq!(eng.shard_bytes(), 128 * MIB);
    assert_eq!(
        eng.block_status(0, 128 * MIB).await.unwrap(),
        vec![(128 * MIB, true)]
    );
    assert_eq!(eng.read(128 * MIB, BLOCK).await.unwrap(), block(1));
}

/// Why SkyZFS's shards are smaller than the default: at 1 GiB, a shard in
/// which more than 65,536 blocks are extents of their own does not commit
/// (the engine builds no deeper trees).
#[tokio::test]
async fn at_one_gib_a_shard_of_scattered_blocks_does_not_commit() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(13);
    let mut eng = Engine::create_volume(
        store.clone(),
        refs.clone(),
        v,
        1 << 30,
        EngineConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(eng.shard_bytes(), 1 << 30);
    for b in 0..=(256 * 256) {
        eng.attach_cas(b * 2 * BLOCK, BLOCK, object_of(b), 0)
            .unwrap();
    }
    let err = eng.commit(CommitKind::Flush).await.unwrap_err();
    assert!(err.to_string().contains("deeper trees"), "{err}");
}

/// The shard size is checked at create: none, or one that is not whole
/// sectors, is refused before anything is written.
#[tokio::test]
async fn a_volume_is_not_created_with_a_shard_of_zero_or_of_part_of_a_sector() {
    for shard_bytes in [0, 100, 64 * KIB + 1] {
        let store = MemoryStore::new();
        let v = vol(14);
        let config = EngineConfig {
            shard_bytes,
            ..EngineConfig::default()
        };
        let made =
            Engine::create_volume(store.clone(), MemoryRefStore::new(), v, 256 * KIB, config).await;
        assert!(
            matches!(made, Err(EngineError::InvalidIo(_))),
            "{shard_bytes}: {:?}",
            made.err()
        );
        assert!(
            store
                .list(&keys::volume_prefix(v))
                .await
                .unwrap()
                .is_empty(),
            "{shard_bytes}: nothing written"
        );
    }
}

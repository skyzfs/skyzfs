//! Fold-time extent coalescing (design §7.2).
//!
//! The risk in merging is the offset arithmetic: a merged extent keeps
//! the first run's target offset, and every read inside it resolves as
//! `offset + (lba - lba_start)`. If the merge predicate were ever wrong
//! about contiguity, reads would silently return the wrong bytes rather
//! than fail — so these tests check content, not just extent counts.

use std::sync::Arc;

use skynbd_engine::{commit_extents, Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::{Target, VolumeId};

const IO: u64 = 64 << 10;
const VOL: u64 = 64 << 20;

/// Distinct bytes per IO, so a wrong offset inside a merged extent shows
/// up as content rather than as a plausible-looking zero.
fn payload(i: u64) -> Vec<u8> {
    let mut buf = vec![0u8; IO as usize];
    for (j, b) in buf.iter_mut().enumerate() {
        *b = (i as u8).wrapping_mul(31).wrapping_add(j as u8);
    }
    buf
}

type Rig = (
    Arc<MemoryStore>,
    Arc<dyn RefStore>,
    VolumeId,
    Engine<Arc<dyn ObjectStore>, Arc<dyn RefStore>>,
);

async fn engine() -> Rig {
    let store = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(3u128));
    let e = Engine::create_volume(
        Arc::clone(&store) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        VOL,
        EngineConfig::default(),
    )
    .await
    .expect("create");
    (store, refs, vol, e)
}

#[tokio::test]
async fn sequential_writes_fold_into_one_extent_and_read_back() {
    let (store, refs, vol, mut e) = engine().await;
    let n = 64;
    for i in 0..n {
        e.write(i * IO, &payload(i)).expect("write");
    }
    let hash = e.commit(CommitKind::Flush).await.expect("commit");

    // 64 IOs, one segment (4 MiB < the 8 MiB seal threshold), one extent.
    let extents = commit_extents(&store, vol, &hash).await.expect("extents");
    assert_eq!(extents.len(), 1, "expected one extent, got {extents:?}");
    assert_eq!(extents[0].lba_start, 0);
    assert_eq!(extents[0].len, n * IO);

    // Content survives the merge, through the in-memory map and after a
    // reopen (which rebuilds it from the merged nodes).
    for i in 0..n {
        assert_eq!(
            e.read(i * IO, IO).await.unwrap(),
            payload(i),
            "live read {i}"
        );
    }
    drop(e);
    let reopened = Engine::open(
        Arc::clone(&store) as Arc<dyn ObjectStore>,
        refs,
        vol,
        EngineConfig::default(),
    )
    .await
    .expect("reopen");
    for i in 0..n {
        assert_eq!(
            reopened.read(i * IO, IO).await.unwrap(),
            payload(i),
            "reopened read {i}"
        );
    }
    // Reads that straddle the merged extent's interior resolve too.
    assert_eq!(
        reopened.read(IO / 2, IO).await.unwrap(),
        [
            &payload(0)[(IO / 2) as usize..],
            &payload(1)[..(IO / 2) as usize]
        ]
        .concat(),
    );
}

/// LBA-adjacent but not segment-adjacent: writes arriving out of order
/// land at unrelated segment offsets, so they must stay separate.
#[tokio::test]
async fn lba_adjacent_but_discontiguous_writes_do_not_merge() {
    let (store, _refs, vol, mut e) = engine().await;
    e.write(IO, &payload(1)).expect("write");
    e.write(0, &payload(0)).expect("write");
    let hash = e.commit(CommitKind::Flush).await.expect("commit");

    let extents = commit_extents(&store, vol, &hash).await.expect("extents");
    assert_eq!(extents.len(), 2, "reversed writes are not one run");
    assert_eq!(e.read(0, IO).await.unwrap(), payload(0));
    assert_eq!(e.read(IO, IO).await.unwrap(), payload(1));
}

/// A hole between two runs blocks the merge; explicit zeroes on both
/// sides of one do not.
#[tokio::test]
async fn zero_runs_merge_and_holes_separate() {
    let (store, _refs, vol, mut e) = engine().await;
    e.write(0, &payload(0)).expect("write");
    e.write(3 * IO, &payload(3)).expect("write");
    e.write_zeroes(4 * IO, 2 * IO).expect("zeroes");
    let hash = e.commit(CommitKind::Flush).await.expect("commit");

    let extents = commit_extents(&store, vol, &hash).await.expect("extents");
    // [0], hole, [3] + the two zero runs folded into one.
    assert_eq!(extents.len(), 3, "{extents:?}");
    assert_eq!(extents[2].target, Target::Zero);
    assert_eq!(extents[2].len, 2 * IO);
    assert_eq!(
        e.read(4 * IO, 2 * IO).await.unwrap(),
        vec![0u8; 2 * IO as usize]
    );
}

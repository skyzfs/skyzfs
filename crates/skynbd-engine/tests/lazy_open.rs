//! Opening a volume is O(1) in its contents (design D-M8): the engine
//! records the commit's shard roots and materializes a shard only when a
//! read, a block-status query, or a commit touches it.
//!
//! These are the load-bearing claims of lazy materialization, asserted
//! against a counting store rather than stated: open costs a constant
//! number of reads however much data exists, a read fetches only its own
//! shard's subtree, and the second read in the same shard fetches no
//! nodes at all.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Result as StoreResult};
use skynbd_types::VolumeId;

const K: u64 = 1024;

struct CountingStore {
    inner: Arc<MemoryStore>,
    reads: AtomicU64,
}

#[async_trait::async_trait]
impl ObjectStore for CountingStore {
    async fn put(&self, key: &str, data: Bytes) -> StoreResult<()> {
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> StoreResult<Bytes> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> StoreResult<bool> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> StoreResult<Vec<String>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> StoreResult<()> {
        self.inner.delete(key).await
    }
}

fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 8 << 20,
        shard_bytes: 64 * K,
        epoch: 0,
        ..EngineConfig::default()
    }
}

/// Build a volume with data in many shards, then reopen it through a
/// counting store and watch what each operation actually fetches.
#[tokio::test]
async fn open_is_constant_and_reads_materialize_per_shard() {
    let mem = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(21u128));

    let mut e = Engine::create_volume(
        Arc::clone(&mem) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        64 * 64 * K, // 64 shards
        cfg(),
    )
    .await
    .unwrap();
    // One extent in every shard.
    for shard in 0..64u64 {
        e.write(shard * 64 * K, &[shard as u8 + 1; 4096]).unwrap();
    }
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);

    let counting = Arc::new(CountingStore {
        inner: Arc::clone(&mem),
        reads: AtomicU64::new(0),
    });
    let reads = |c: &CountingStore| c.reads.load(Ordering::Relaxed);

    // Open: superblock + clone-base probe + head commit + shard index —
    // a constant, not a function of the 64 populated shards.
    let opened = Engine::open(
        Arc::clone(&counting) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        cfg(),
    )
    .await
    .unwrap();
    let at_open = reads(&counting);
    assert!(
        at_open <= 6,
        "open touched {at_open} objects; it must not scale with content"
    );

    // First read in shard 3: its root node + the segment bytes, nothing
    // from the other 63 shards.
    assert_eq!(
        opened.read(3 * 64 * K, 4096).await.unwrap(),
        vec![4u8; 4096]
    );
    let first_read = reads(&counting) - at_open;
    assert!(
        (1..=3).contains(&first_read),
        "first read in a shard fetched {first_read} objects; expected its \
         root and its data, not the volume"
    );

    // Second read in the same shard: the index is materialized, so only
    // the segment bytes move.
    assert_eq!(
        opened.read(3 * 64 * K + 1024, 1024).await.unwrap(),
        vec![4u8; 1024]
    );
    let second_read = reads(&counting) - at_open - first_read;
    assert_eq!(
        second_read, 1,
        "a materialized shard's reads must fetch data only"
    );

    // Block status likewise touches only its shard.
    let before = reads(&counting);
    let runs = opened.block_status(5 * 64 * K, 64 * K).await.unwrap();
    assert_eq!(runs, vec![(4096, true), (64 * K - 4096, false)]);
    assert!(reads(&counting) - before <= 2);
}

/// A snapshot export that nobody reads costs nothing to publish beyond
/// the open itself — the startup cost that motivated D-M8: it was one
/// full tree walk per snapshot ref, at every daemon start.
#[tokio::test]
async fn an_unread_export_materializes_nothing() {
    let mem = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(22u128));

    let mut e = Engine::create_volume(
        Arc::clone(&mem) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        64 * 64 * K,
        cfg(),
    )
    .await
    .unwrap();
    for shard in 0..64u64 {
        e.write(shard * 64 * K, &[7u8; 4096]).unwrap();
    }
    e.commit(CommitKind::Flush).await.unwrap();
    let (_, snap_commit) = e.snapshot_create("g0").await.unwrap();
    drop(e);

    let counting = Arc::new(CountingStore {
        inner: Arc::clone(&mem),
        reads: AtomicU64::new(0),
    });
    let export = Engine::open_at_commit(
        Arc::clone(&counting) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        snap_commit,
        cfg(),
    )
    .await
    .unwrap();
    let at_open = counting.reads.load(Ordering::Relaxed);
    assert!(
        at_open <= 6,
        "publishing a snapshot export walked {at_open} objects"
    );
    // And it still serves correct bytes when finally read.
    assert_eq!(
        export.read(9 * 64 * K, 4096).await.unwrap(),
        vec![7u8; 4096]
    );
}

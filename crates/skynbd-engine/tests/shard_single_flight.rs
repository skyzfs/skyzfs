//! Shard materialization is single-flight (issue #97 item 2 review):
//! with reads sharing the engine, readers that cold-touch the same shard
//! at once wait for one subtree walk instead of each fetching it.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Result};
use skynbd_types::VolumeId;

/// Counts node GETs and delays each, so concurrent touches overlap.
/// `fail` makes node GETs fail (after the delay); `started` is notified
/// as each node GET begins.
#[derive(Default)]
struct NodeCounter {
    inner: MemoryStore,
    node_gets: AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
    started: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl ObjectStore for NodeCounter {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        if key.contains("/node/") {
            self.node_gets.fetch_add(1, Ordering::SeqCst);
            self.started.notify_waiters();
            tokio::time::sleep(Duration::from_millis(20)).await;
            if self.fail.load(Ordering::SeqCst) {
                return Err(skynbd_store::StoreError::Backend("injected".into()));
            }
        }
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> Result<bool> {
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_first_touches_of_a_shard_walk_it_once() {
    const KIB: u64 = 1024;
    let store = Arc::new(NodeCounter::default());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(21u128));
    let cfg = EngineConfig {
        segment_max_bytes: 4 * KIB,
        shard_bytes: 256 * KIB,
        ..EngineConfig::default()
    };
    let mut e = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol,
        1 << 20,
        cfg.clone(),
    )
    .await
    .unwrap();
    for i in 0..8u64 {
        e.write(i * 4 * KIB, &[i as u8 + 1; 4096]).unwrap();
    }
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);

    // One walk's cost: a fresh engine (open loads only the shard index),
    // touched by a single read.
    let single = {
        let e = Engine::open(Arc::clone(&store), Arc::clone(&refs), vol, cfg.clone())
            .await
            .unwrap();
        let before = store.node_gets.load(Ordering::SeqCst);
        e.read(0, 4 * KIB).await.unwrap();
        store.node_gets.load(Ordering::SeqCst) - before
    };
    assert!(single >= 1, "the shard was never walked");

    // Another fresh engine, touched by eight concurrent reads.
    let e = Engine::open(Arc::clone(&store), refs, vol, cfg)
        .await
        .unwrap();
    let before = store.node_gets.load(Ordering::SeqCst);
    let reads = (0..8u64).map(|i| e.read(i * 4 * KIB, 4 * KIB));
    let out = futures_util::future::join_all(reads).await;
    for (i, r) in out.into_iter().enumerate() {
        assert_eq!(r.unwrap(), vec![i as u8 + 1; 4096]);
    }
    let walked = store.node_gets.load(Ordering::SeqCst) - before;
    assert_eq!(
        walked, single,
        "8 concurrent first touches fetched {walked} nodes; one walk fetches {single}"
    );
}

/// A committed 8-block volume, as the first test builds it.
async fn volume(store: &Arc<NodeCounter>) -> (Arc<dyn RefStore>, VolumeId, EngineConfig) {
    const KIB: u64 = 1024;
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::new());
    let cfg = EngineConfig {
        segment_max_bytes: 4 * KIB,
        shard_bytes: 256 * KIB,
        ..EngineConfig::default()
    };
    let mut e = Engine::create_volume(
        Arc::clone(store),
        Arc::clone(&refs),
        vol,
        1 << 20,
        cfg.clone(),
    )
    .await
    .unwrap();
    for i in 0..8u64 {
        e.write(i * 4 * KIB, &[i as u8 + 1; 4096]).unwrap();
    }
    e.commit(CommitKind::Flush).await.unwrap();
    (refs, vol, cfg)
}

/// A failing walk fails every caller already waiting on it — one walk's
/// worth of GETs, not one per waiter in turn (each holding its read guard,
/// and any queued writer behind it) — and the error is not cached: a
/// caller arriving afterwards walks again and succeeds.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_walk_fails_its_waiters_once_and_a_later_read_retries() {
    const KIB: u64 = 1024;
    let store = Arc::new(NodeCounter::default());
    let (refs, vol, cfg) = volume(&store).await;
    let e = Engine::open(Arc::clone(&store), refs, vol, cfg)
        .await
        .unwrap();

    store.fail.store(true, Ordering::SeqCst);
    let before = store.node_gets.load(Ordering::SeqCst);
    let reads = (0..8u64).map(|i| e.read(i * 4 * KIB, 4 * KIB));
    for r in futures_util::future::join_all(reads).await {
        assert!(r.is_err(), "a read over a failing walk succeeded");
    }
    let walked = store.node_gets.load(Ordering::SeqCst) - before;
    assert_eq!(
        walked, 1,
        "8 waiters on one failing walk fetched {walked} roots"
    );

    store.fail.store(false, Ordering::SeqCst);
    for i in 0..8u64 {
        assert_eq!(
            e.read(i * 4 * KIB, 4 * KIB).await.unwrap(),
            vec![i as u8 + 1; 4096],
            "the failure was cached"
        );
    }
}

/// Cancelling the reader that runs the walk does not strand the readers
/// waiting on it: the next one walks, and completes.
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_the_walking_reader_hands_the_walk_to_a_waiter() {
    const KIB: u64 = 1024;
    let store = Arc::new(NodeCounter::default());
    let (refs, vol, cfg) = volume(&store).await;
    let e = Arc::new(
        Engine::open(Arc::clone(&store), refs, vol, cfg)
            .await
            .unwrap(),
    );

    let started = store.started.notified();
    let first = {
        let e = Arc::clone(&e);
        tokio::spawn(async move { e.read(0, 4 * KIB).await })
    };
    started.await; // the first reader's walk is mid-GET
    let second = {
        let e = Arc::clone(&e);
        tokio::spawn(async move { e.read(4 * KIB, 4 * KIB).await })
    };
    tokio::time::sleep(Duration::from_millis(5)).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());
    let got = tokio::time::timeout(Duration::from_secs(5), second)
        .await
        .expect("the waiter was stranded")
        .unwrap()
        .unwrap();
    assert_eq!(got, vec![2u8; 4096]);
}

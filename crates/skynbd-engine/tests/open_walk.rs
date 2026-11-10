//! What `Engine::open` costs when object GETs have real latency.
//!
//! Historically open materialized the whole extent map — thousands of
//! node fetches, pure round-trip time. With lazy materialization (D-M8)
//! open is a constant handful of reads; this now measures that constant
//! plus what the first read into a shard pays, against a store with
//! injected latency.
//!
//! Measurement only: run with
//! `cargo test -p skynbd-engine --test open_walk -- --ignored --nocapture`.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Result};
use skynbd_types::VolumeId;

/// Adds a fixed delay to every read, and counts them.
struct SlowStore {
    inner: Arc<MemoryStore>,
    latency: Duration,
    reads: AtomicU64,
}

#[async_trait::async_trait]
impl ObjectStore for SlowStore {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(self.latency).await;
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> Result<bool> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        tokio::time::sleep(self.latency).await;
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        tokio::time::sleep(self.latency).await;
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "measurement, not an assertion; run with --ignored --nocapture"]
async fn open_walk_latency() {
    const MIB: u64 = 1 << 20;
    let size = 4 << 30;
    let latency = Duration::from_millis(10);

    // A fragmented map: 4 KiB writes scattered across the address space,
    // which is the shape that makes the walk wide.
    let store = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(7u128));
    let mut e = Engine::create_volume(
        Arc::clone(&store) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        size,
        EngineConfig::default(),
    )
    .await
    .expect("create");
    let buf = [0x5au8; 4096];
    let mut seed = 0x243f_6a88_85a3_08d3u64;
    let mut since = 0;
    for _ in 0..65_536 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        e.write((seed % (size / 4096)) * 4096, &buf).expect("write");
        since += 4096;
        if since >= 8 * MIB {
            e.commit(CommitKind::Flush).await.expect("commit");
            since = 0;
        }
    }
    e.commit(CommitKind::Checkpoint).await.expect("commit");
    drop(e);

    let slow = Arc::new(SlowStore {
        inner: Arc::clone(&store),
        latency,
        reads: AtomicU64::new(0),
    });
    let t = std::time::Instant::now();
    let _opened = Engine::open(
        Arc::clone(&slow) as Arc<dyn ObjectStore>,
        Arc::clone(&refs),
        vol,
        EngineConfig::default(),
    )
    .await
    .expect("open");
    let elapsed = t.elapsed();
    let reads = slow.reads.load(Ordering::Relaxed);
    let serial = latency * reads as u32;

    println!(
        "open: reads={reads} latency={latency:?} => elapsed={elapsed:?} \
         (one-at-a-time would be {serial:?}, {:.1}x)",
        serial.as_secs_f64() / elapsed.as_secs_f64()
    );
}

//! The D-M4 economics, counted: under the cadenced store, head CASes —
//! the per-fsync movers that cost ~$26/volume/month as conditional PUTs
//! — touch the refs object zero times, while everything whose loss
//! would break a promise (snapshots, pins, meta) exports immediately.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use skynbd_refs::{CadencedRefStore, CasOutcome, ObjectRefStore, RefOp, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Precondition};
use skynbd_types::VolumeId;

/// Counts conditional PUTs on the way through.
struct Counting {
    inner: MemoryStore,
    put_ifs: AtomicU64,
}

#[async_trait::async_trait]
impl ObjectStore for Counting {
    async fn put(&self, k: &str, d: bytes::Bytes) -> skynbd_store::Result<()> {
        self.inner.put(k, d).await
    }
    async fn get(
        &self,
        k: &str,
        r: Option<std::ops::Range<u64>>,
    ) -> skynbd_store::Result<bytes::Bytes> {
        self.inner.get(k, r).await
    }
    async fn exists(&self, k: &str) -> skynbd_store::Result<bool> {
        self.inner.exists(k).await
    }
    async fn list(&self, p: &str) -> skynbd_store::Result<Vec<String>> {
        self.inner.list(p).await
    }
    async fn delete(&self, k: &str) -> skynbd_store::Result<()> {
        self.inner.delete(k).await
    }
    async fn get_versioned(&self, k: &str) -> skynbd_store::Result<(bytes::Bytes, String)> {
        self.inner.get_versioned(k).await
    }
    async fn put_if(
        &self,
        k: &str,
        d: bytes::Bytes,
        pre: Precondition,
    ) -> skynbd_store::Result<String> {
        self.put_ifs.fetch_add(1, Ordering::SeqCst);
        self.inner.put_if(k, d, pre).await
    }
}

#[tokio::test]
async fn head_cas_costs_no_conditional_puts_until_the_cadence() {
    let counting = Arc::new(Counting {
        inner: MemoryStore::new(),
        put_ifs: AtomicU64::new(0),
    });
    let s = CadencedRefStore::new(ObjectRefStore::new(
        Arc::clone(&counting) as Arc<dyn ObjectStore>
    ));
    let v = VolumeId(ulid::Ulid::from_parts(0xD4, 1));

    // A hundred head advances — a hundred fsyncs — through the cache.
    let mut cur = None;
    for i in 0..100u8 {
        let out = s
            .compare_and_swap(v, "head", cur, Some([i; 32]))
            .await
            .unwrap();
        match out {
            CasOutcome::Committed(rv) => cur = rv,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        counting.put_ifs.load(Ordering::SeqCst),
        0,
        "head CASes must not touch the refs object"
    );

    // A snapshot publish (ref + pin, I8) exports immediately: one PUT.
    s.transact(
        v,
        vec![
            RefOp::CasRef {
                name: "snap/s1".into(),
                expected: None,
                new_commit: Some([0xF0; 32]),
            },
            RefOp::Pin {
                name: "snap/s1".into(),
                holder: "publisher".into(),
            },
        ],
    )
    .await
    .unwrap();
    assert_eq!(counting.put_ifs.load(Ordering::SeqCst), 1);

    // The cadence tick exports the accumulated heads: one more PUT,
    // whatever the fsync count was.
    s.export(v).await.unwrap();
    assert_eq!(counting.put_ifs.load(Ordering::SeqCst), 2);

    // And the export carried the latest head: a fresh reader of the
    // object (a new server) sees CAS #100.
    let fresh = ObjectRefStore::new(Arc::clone(&counting) as Arc<dyn ObjectStore>);
    assert_eq!(
        fresh.get(v, "head").await.unwrap().unwrap().commit,
        [99u8; 32]
    );
    assert_eq!(
        fresh.pins(v, "snap/s1").await.unwrap(),
        vec!["publisher".to_string()]
    );
}

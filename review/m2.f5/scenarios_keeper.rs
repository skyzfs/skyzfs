//! Review scenarios for #131 (F5) at the keeper, copied into
//! crates/skyzfs-upload/tests/ as review_f5k.rs for a run and removed
//! after. Not part of the pull request.
//!
//! K1: L4 of the review of #117 again, on two real keepers: B's first
//!     take falls between A's renewal's read of the refs object and its
//!     write, and B then does not renew for twice the lease. Then B
//!     renews every third of the lease for ten leases' time.
//! K2: the same with A's clock ahead of B's by twice the lease.
//! K3: two daemons under one name (N1 of #117's review): records.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use skynbd_refs::{Clock, ObjectRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Precondition};
use skynbd_types::VolumeId;
use skyzfs_upload::lease::{Keeper, LeaseState};

const TTL: u64 = 30_000;

type Refs = ObjectRefStore<Arc<dyn ObjectStore>>;
type Hook = Arc<Mutex<Option<Box<dyn FnOnce() -> futures_util::future::BoxFuture<'static, ()> + Send>>>>;

/// After its next read of an object, runs the hook, then answers the read.
struct Between {
    inner: Arc<dyn ObjectStore>,
    hook: Hook,
}

#[async_trait::async_trait]
impl ObjectStore for Between {
    async fn put(&self, key: &str, data: Bytes) -> skynbd_store::Result<()> {
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> skynbd_store::Result<Bytes> {
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
        self.inner.delete(key).await
    }
    async fn get_versioned(&self, key: &str) -> skynbd_store::Result<(Bytes, String)> {
        let read = self.inner.get_versioned(key).await;
        let hook = self.hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook().await;
        }
        read
    }
    async fn put_if(
        &self,
        key: &str,
        data: Bytes,
        pre: Precondition,
    ) -> skynbd_store::Result<String> {
        self.inner.put_if(key, data, pre).await
    }
}

fn clock_of(now: &Arc<AtomicU64>) -> Clock {
    let n = Arc::clone(now);
    Arc::new(move || n.load(Ordering::SeqCst))
}

async fn row(objects: &Arc<dyn ObjectStore>, vol: VolumeId) -> Option<(String, u64, u64)> {
    let l = ObjectRefStore::new(Arc::clone(objects))
        .lease_get(vol)
        .await
        .unwrap()?;
    Some((l.holder, l.epoch, l.expires_at_ms))
}

/// K1 and K2. `skew`: how far A's clock is ahead of B's.
async fn l4(skew: u64) {
    let vol = VolumeId(ulid::Ulid::from_parts(0xF5, 2));
    let objects: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let b_now = Arc::new(AtomicU64::new(1_000_000));
    let a_now = Arc::new(AtomicU64::new(1_000_000 + skew));
    let hook: Hook = Arc::default();
    let a_refs: Refs = ObjectRefStore::new(Arc::new(Between {
        inner: Arc::clone(&objects),
        hook: Arc::clone(&hook),
    }) as Arc<dyn ObjectStore>)
    .with_clock(clock_of(&a_now));
    let b_refs: Refs = ObjectRefStore::new(Arc::clone(&objects)).with_clock(clock_of(&b_now));
    let pass = |ms: u64| {
        a_now.fetch_add(ms, Ordering::SeqCst);
        b_now.fetch_add(ms, Ordering::SeqCst);
    };
    let mut a = Keeper::new(a_refs, vol, "node-a", TTL);
    let b = Arc::new(tokio::sync::Mutex::new(Keeper::new(b_refs, vol, "node-b", TTL)));
    let a_first = a.keep().await.clone();
    // A's outage, longer than the lease (by B's clock as well).
    pass(2 * TTL + skew);
    let b_first: Arc<Mutex<Option<LeaseState>>> = Arc::default();
    {
        let (b, b_first, a_now, b_now) = (
            Arc::clone(&b),
            Arc::clone(&b_first),
            Arc::clone(&a_now),
            Arc::clone(&b_now),
        );
        *hook.lock().unwrap() = Some(Box::new(move || {
            Box::pin(async move {
                let state = b.lock().await.keep().await.clone();
                *b_first.lock().unwrap() = Some(state);
                // B does not renew for twice the lease.
                a_now.fetch_add(2 * TTL, Ordering::SeqCst);
                b_now.fetch_add(2 * TTL, Ordering::SeqCst);
            })
        }));
    }
    let a_state = a.keep().await.clone();
    let b_first = b_first.lock().unwrap().clone();
    let row_then = row(&objects, vol).await;
    let b_next = b.lock().await.keep().await.clone();
    println!("## L4 (skew {skew} ms): A's first take {a_first:?}");
    println!("## L4: B's first take, between A's read and A's write: {b_first:?}");
    println!("## L4: A after its renewal: {a_state:?}; the row {row_then:?}");
    println!("## L4: B at its next renewal: {b_next:?}");
    // Ten leases' time, B renewing every third of the lease, A turning too.
    let mut b_lost = 0;
    let mut a_held = 0;
    for _ in 0..30 {
        pass(TTL / 3);
        if !matches!(b.lock().await.keep().await, LeaseState::Held { epoch: 2, .. }) {
            b_lost += 1;
        }
        if a.keep().await.epoch().is_some() {
            a_held += 1;
        }
    }
    let row_end = row(&objects, vol).await;
    println!(
        "## L4: ten leases later: B not held at epoch 2 in {b_lost} of 30 renewals, A held in \
         {a_held} of 30; A {:?}; the row {row_end:?}",
        a.state()
    );
    assert!(matches!(b_first, Some(LeaseState::Held { epoch: 2, .. })));
    assert_eq!(
        a_state,
        LeaseState::Lost {
            holder: "node-b".into(),
            epoch: 2
        }
    );
    assert!(matches!(b_next, LeaseState::Held { epoch: 2, .. }));
    assert_eq!((b_lost, a_held), (0, 0));
    assert!(matches!(row_end, Some((h, 2, _)) if h == "node-b"));
}

#[tokio::test]
async fn k1_l4_on_two_keepers() {
    l4(0).await;
}

#[tokio::test]
async fn k2_l4_with_a_clock_ahead() {
    l4(2 * TTL).await;
}

/// Two daemons under one name: both hold, at one epoch (N1 of #117's
/// review; not changed by F5). Records.
#[tokio::test]
async fn k3_two_daemons_of_one_name() {
    let vol = VolumeId(ulid::Ulid::from_parts(0xF5, 3));
    let objects: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let refs = || ObjectRefStore::new(Arc::clone(&objects)).with_clock(clock_of(&now));
    let mut a1 = Keeper::new(refs(), vol, "node-a", TTL);
    let mut a2 = Keeper::new(refs(), vol, "node-a", TTL);
    let s1 = a1.keep().await.clone();
    let s2 = a2.keep().await.clone();
    now.fetch_add(TTL / 3, Ordering::SeqCst);
    let r1 = a1.keep().await.clone();
    let r2 = a2.keep().await.clone();
    // One of them stops cleanly and lets the lease go; the other renews.
    a2.release().await.unwrap();
    let released = row(&objects, vol).await;
    now.fetch_add(TTL / 3, Ordering::SeqCst);
    let r1b = a1.keep().await.clone();
    let after = row(&objects, vol).await;
    println!("## N1: takes {s1:?} / {s2:?}; renewals {r1:?} / {r2:?}");
    println!("## N1: a2 releases: row {released:?}; a1 renews: {r1b:?}; row {after:?}");
}

//! `ObjectRefStore` behaves like the in-memory store to a single caller —
//! `exercise_ref_store` is that contract — and unlike it, stays correct
//! when two *processes* share the bucket. The race tests are the point:
//! they are exactly what redb cannot do, and the reason D-M1 exists.

use std::sync::Arc;

use skynbd_refs::{conformance, CasOutcome, ObjectRefStore, RefOp, RefStore, RefValue, TxnOutcome};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

fn vol(n: u64) -> VolumeId {
    VolumeId(ulid::Ulid::from_parts(0xD0, n as u128))
}

#[tokio::test]
async fn passes_the_ref_store_conformance() {
    conformance::exercise_ref_store(&ObjectRefStore::new(MemoryStore::new())).await;
}

/// Two stores over one bucket — two skynbdd processes. A CAS race from
/// the same expected value must produce exactly one winner, and the
/// loser must observe the winner's value. This is I2 across processes,
/// which no node-local refs store can provide.
#[tokio::test]
async fn cas_race_across_instances_has_one_winner() {
    let shared = Arc::new(MemoryStore::new());
    let a = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn ObjectStore>);
    let b = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn ObjectStore>);
    let v = vol(1);

    // Both see the same current head...
    let base = match a
        .compare_and_swap(v, "head", None, Some([1u8; 32]))
        .await
        .unwrap()
    {
        CasOutcome::Committed(Some(rv)) => rv,
        other => panic!("create: {other:?}"),
    };
    assert_eq!(b.get(v, "head").await.unwrap(), Some(base));

    // ...and both try to advance from it concurrently.
    let (ra, rb) = tokio::join!(
        a.compare_and_swap(v, "head", Some(base), Some([2u8; 32])),
        b.compare_and_swap(v, "head", Some(base), Some([3u8; 32])),
    );
    let outcomes = [ra.unwrap(), rb.unwrap()];
    let winners = outcomes
        .iter()
        .filter(|o| matches!(o, CasOutcome::Committed(_)))
        .count();
    assert_eq!(winners, 1, "exactly one writer must win: {outcomes:?}");

    // The loser's Conflict carries the winner's value, and both stores
    // now agree on it.
    let winner_value = outcomes
        .iter()
        .find_map(|o| match o {
            CasOutcome::Committed(v) => *v,
            _ => None,
        })
        .expect("winner stored a value");
    let loser_saw = outcomes
        .iter()
        .find_map(|o| match o {
            CasOutcome::Conflict(v) => Some(*v),
            _ => None,
        })
        .expect("loser reported a conflict");
    assert_eq!(loser_saw, Some(winner_value));
    assert_eq!(a.get(v, "head").await.unwrap(), Some(winner_value));
    assert_eq!(b.get(v, "head").await.unwrap(), Some(winner_value));
    assert_eq!(winner_value.generation, base.generation + 1);
}

/// Publish-implies-pin (I8) across processes: the other instance never
/// observes the ref without its pin, whichever moment it reads.
#[tokio::test]
async fn transactions_are_atomic_across_instances() {
    let shared = Arc::new(MemoryStore::new());
    let a = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn ObjectStore>);
    let b = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn ObjectStore>);
    let v = vol(2);

    let publish = a.transact(
        v,
        vec![
            RefOp::CasRef {
                name: "snap/g0".into(),
                expected: None,
                new_commit: Some([7u8; 32]),
            },
            RefOp::Pin {
                name: "snap/g0".into(),
                holder: "publisher".into(),
            },
            RefOp::MetaPut {
                key: "manifest/g0".into(),
                value: b"m".to_vec(),
            },
        ],
    );
    assert!(matches!(publish.await.unwrap(), TxnOutcome::Committed));

    // Everything or nothing, seen from the second process.
    assert!(b.get(v, "snap/g0").await.unwrap().is_some());
    assert_eq!(b.pins(v, "snap/g0").await.unwrap(), vec!["publisher"]);
    assert_eq!(
        b.meta_get(v, "manifest/g0").await.unwrap(),
        Some(b"m".to_vec())
    );

    // And a transaction whose CAS loses leaves no partial state behind.
    let stale = RefValue {
        commit: [9u8; 32],
        generation: 41,
    };
    let lost = b
        .transact(
            v,
            vec![
                RefOp::MetaPut {
                    key: "manifest/g1".into(),
                    value: b"x".to_vec(),
                },
                RefOp::CasRef {
                    name: "snap/g0".into(),
                    expected: Some(stale),
                    new_commit: Some([8u8; 32]),
                },
            ],
        )
        .await
        .unwrap();
    assert!(matches!(lost, TxnOutcome::Conflict { op_index: 1, .. }));
    assert!(a.meta_get(v, "manifest/g1").await.unwrap().is_none());
}

/// Interleaved single-op writers converge: N concurrent pin transactions
/// through independent instances all land, none lost to the etag race.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_pins_are_not_lost() {
    let shared = Arc::new(MemoryStore::new());
    let v = vol(3);
    let mut tasks = tokio::task::JoinSet::new();
    for i in 0..16u32 {
        let store = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn ObjectStore>);
        tasks.spawn(async move {
            store
                .transact(
                    v,
                    vec![RefOp::Pin {
                        name: "snap/base".into(),
                        holder: format!("h{i}"),
                    }],
                )
                .await
                .unwrap()
        });
    }
    while let Some(r) = tasks.join_next().await {
        assert!(matches!(r.unwrap(), TxnOutcome::Committed));
    }
    let reader = ObjectRefStore::new(shared as Arc<dyn ObjectStore>);
    assert_eq!(reader.pins(v, "snap/base").await.unwrap().len(), 16);
}

/// A backend without conditional writes is refused at probe time with a
/// message naming the fix, not discovered mid-transaction.
#[tokio::test]
async fn probe_rejects_incapable_backends() {
    struct Plain(MemoryStore);
    #[async_trait::async_trait]
    impl ObjectStore for Plain {
        async fn put(&self, k: &str, d: bytes::Bytes) -> skynbd_store::Result<()> {
            self.0.put(k, d).await
        }
        async fn get(
            &self,
            k: &str,
            r: Option<std::ops::Range<u64>>,
        ) -> skynbd_store::Result<bytes::Bytes> {
            self.0.get(k, r).await
        }
        async fn exists(&self, k: &str) -> skynbd_store::Result<bool> {
            self.0.exists(k).await
        }
        async fn list(&self, p: &str) -> skynbd_store::Result<Vec<String>> {
            self.0.list(p).await
        }
        async fn delete(&self, k: &str) -> skynbd_store::Result<()> {
            self.0.delete(k).await
        }
        // get_versioned / put_if fall to the trait defaults: Unsupported.
    }

    let err = ObjectRefStore::probe(&Plain(MemoryStore::new()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("conditional writes"), "{err}");
}

// ---- Ownership (design D-M3) ----

use skynbd_refs::{Clock, Lease, LeaseConfig, LeaseOutcome, RefError};
use std::sync::atomic::{AtomicU64, Ordering};

/// A controllable clock shared by every store in a test: expiry is
/// exercised by advancing time, never by sleeping.
fn test_clock() -> (Arc<AtomicU64>, Clock) {
    let now = Arc::new(AtomicU64::new(1_000_000));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    (now, clock)
}

fn fenced(
    shared: &Arc<MemoryStore>,
    holder: &str,
    clock: &Clock,
) -> ObjectRefStore<Arc<dyn ObjectStore>> {
    ObjectRefStore::new(Arc::clone(shared) as Arc<dyn ObjectStore>)
        .with_clock(Arc::clone(clock))
        .with_owner(LeaseConfig {
            holder: holder.to_string(),
            ttl_ms: 10_000,
        })
}

#[tokio::test]
async fn passes_the_lease_conformance() {
    let (now, clock) = test_clock();
    let s = ObjectRefStore::new(MemoryStore::new()).with_clock(clock);
    let advance = move |ms: u64| _ = now.fetch_add(ms, Ordering::SeqCst);
    conformance::exercise_lease(&s, &advance).await;
}

#[tokio::test]
async fn passes_the_fenced_transact_conformance() {
    let (now, clock) = test_clock();
    let s = ObjectRefStore::new(MemoryStore::new()).with_clock(clock);
    let advance = move |ms: u64| _ = now.fetch_add(ms, Ordering::SeqCst);
    conformance::exercise_fenced_transact(&s, &advance).await;
}

/// The task-15h ambient-write knot: a store whose *ambient* identity is
/// the server's, serving a volume whose lease a NODE holds. Ambient
/// mutations are fenced out (`cfg.holder` is not the owner), while the
/// same store's `transact_as` under the node's `(holder, epoch)` — the
/// explicit principal a serve-time `ActingRefStore` supplies — commits.
#[tokio::test]
async fn explicit_principal_supersedes_the_ambient_fence() {
    let shared = Arc::new(MemoryStore::new());
    let (_now, clock) = test_clock();
    let server = Arc::new(fenced(&shared, "server-1", &clock));
    let v = VolumeId(ulid::Ulid::from_parts(0xAC, 7));

    // The node acquires the volume's lease through any path.
    match server.lease_acquire(v, "node-a", 10_000).await.unwrap() {
        LeaseOutcome::Acquired(l) => assert_eq!((l.holder.as_str(), l.epoch), ("node-a", 1)),
        other => panic!("expected acquire, got {other:?}"),
    }

    // Ambient write: fenced out — the server is not the owner.
    let op = |name: &str| {
        vec![skynbd_refs::RefOp::CasRef {
            name: name.into(),
            expected: None,
            new_commit: Some([1u8; 32]),
        }]
    };
    assert!(matches!(
        server.transact(v, op("snap/x")).await,
        Err(RefError::LeaseLost { .. })
    ));

    // Explicit principal: the same store commits as the node.
    let out = server
        .transact_as(v, op("snap/x"), "node-a", 1)
        .await
        .unwrap();
    assert_eq!(out, skynbd_refs::TxnOutcome::Committed);

    // The acting wrapper is that principal, packaged.
    let acting = skynbd_refs::ActingRefStore::new(Arc::clone(&server), "node-a", 1);
    let out = acting.transact(v, op("snap/y")).await.unwrap();
    assert_eq!(out, skynbd_refs::TxnOutcome::Committed);
}

/// The exclusivity D-M3 adds over bare CAS: a store fenced to holder B
/// cannot write A's volume even with a *correct* CAS expectation — the
/// two-servers-acking-writes window closes at the first write, not at
/// the first stale expectation.
#[tokio::test]
async fn fenced_write_by_non_owner_is_lease_lost() {
    let shared = Arc::new(MemoryStore::new());
    let (_, clock) = test_clock();
    let a = fenced(&shared, "node-a", &clock);
    let b = fenced(&shared, "node-b", &clock);
    let v = vol(20);

    assert!(matches!(
        a.lease_acquire(v, "node-a", 10_000).await.unwrap(),
        LeaseOutcome::Acquired(Lease { epoch: 1, .. })
    ));
    let head = match a
        .compare_and_swap(v, "head", None, Some([1u8; 32]))
        .await
        .unwrap()
    {
        CasOutcome::Committed(Some(rv)) => rv,
        other => panic!("owner create: {other:?}"),
    };

    // B's expectation is *correct* — the fence, not the CAS, refuses.
    let err = b
        .compare_and_swap(v, "head", Some(head), Some([2u8; 32]))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RefError::LeaseLost { holder, epoch: 1, .. } if holder == "node-a"),
        "{err}"
    );
    // Same for transactions and their side tables.
    let err = b
        .transact(
            v,
            vec![RefOp::Pin {
                name: "head".into(),
                holder: "b".into(),
            }],
        )
        .await
        .unwrap_err();
    assert!(matches!(err, RefError::LeaseLost { .. }), "{err}");
    // Nothing landed, and reads (which fencing never blocks) prove it.
    assert_eq!(b.get(v, "head").await.unwrap(), Some(head));
    assert!(b.pins(v, "head").await.unwrap().is_empty());
}

/// The D-M3 fence story end to end: A owns, dies silently; B waits out
/// the TTL and steals (epoch 2); A comes back and writes — and must get
/// `LeaseLost`, the signal the engine turns into ESHUTDOWN. Then the
/// mirror image: A cannot steal back while B renews.
#[tokio::test]
async fn stale_owner_is_fenced_after_steal() {
    let shared = Arc::new(MemoryStore::new());
    let (now, clock) = test_clock();
    let a = fenced(&shared, "node-a", &clock);
    let b = fenced(&shared, "node-b", &clock);
    let v = vol(21);

    a.lease_acquire(v, "node-a", 10_000).await.unwrap();
    let head = match a
        .compare_and_swap(v, "head", None, Some([1u8; 32]))
        .await
        .unwrap()
    {
        CasOutcome::Committed(Some(rv)) => rv,
        other => panic!("create: {other:?}"),
    };

    // A goes silent; its lease expires; B steals.
    now.fetch_add(30_000, Ordering::SeqCst);
    let stolen = match b.lease_acquire(v, "node-b", 10_000).await.unwrap() {
        LeaseOutcome::Acquired(l) => l,
        other => panic!("steal: {other:?}"),
    };
    assert_eq!(stolen.epoch, 2, "a steal must move the fence line");

    // A's comeback write: correct expectation, refused by the fence.
    let err = a
        .compare_and_swap(v, "head", Some(head), Some([9u8; 32]))
        .await
        .unwrap_err();
    assert!(
        matches!(&err, RefError::LeaseLost { holder, epoch: 2, .. } if holder == "node-b"),
        "{err}"
    );

    // B renews by writing (for free); A cannot steal while B is live.
    now.fetch_add(9_000, Ordering::SeqCst);
    b.compare_and_swap(v, "head", Some(head), Some([2u8; 32]))
        .await
        .unwrap();
    now.fetch_add(9_000, Ordering::SeqCst); // > original expiry, < renewed
    match a.lease_acquire(v, "node-a", 10_000).await.unwrap() {
        LeaseOutcome::Held(l) => assert_eq!((l.holder.as_str(), l.epoch), ("node-b", 2)),
        other => panic!("expected held: {other:?}"),
    }
}

/// "Every commit renews the lease for free": an owner's ordinary write
/// pushes its expiry out without any lease call.
#[tokio::test]
async fn owner_writes_renew_the_lease() {
    let shared = Arc::new(MemoryStore::new());
    let (now, clock) = test_clock();
    let a = fenced(&shared, "node-a", &clock);
    let v = vol(22);

    let l0 = match a.lease_acquire(v, "node-a", 10_000).await.unwrap() {
        LeaseOutcome::Acquired(l) => l,
        other => panic!("{other:?}"),
    };
    now.fetch_add(6_000, Ordering::SeqCst);
    a.compare_and_swap(v, "head", None, Some([1u8; 32]))
        .await
        .unwrap();
    let l1 = a.lease_get(v).await.unwrap().unwrap();
    assert_eq!(l1.epoch, l0.epoch);
    assert!(
        l1.expires_at_ms >= l0.expires_at_ms + 6_000,
        "write must extend expiry: {l0:?} -> {l1:?}"
    );
}

/// An unfenced store (no `with_owner`) ignores owner rows entirely —
/// single-node mode keeps working against a bucket that once had leases.
#[tokio::test]
async fn unfenced_store_ignores_owner_rows() {
    let shared = Arc::new(MemoryStore::new());
    let (_, clock) = test_clock();
    let a = fenced(&shared, "node-a", &clock);
    let v = vol(23);
    a.lease_acquire(v, "node-a", 10_000).await.unwrap();

    let plain = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn ObjectStore>);
    assert!(matches!(
        plain
            .compare_and_swap(v, "head", None, Some([1u8; 32]))
            .await
            .unwrap(),
        CasOutcome::Committed(_)
    ));
    // And it does not touch the lease row while writing.
    assert_eq!(plain.lease_get(v).await.unwrap().unwrap().holder, "node-a");
}

/// Two takers race an expired lease: conditional writes guarantee one
/// winner and one Held, never two owners at the same epoch.
#[tokio::test]
async fn steal_race_has_one_winner() {
    let shared = Arc::new(MemoryStore::new());
    let (now, clock) = test_clock();
    let a = fenced(&shared, "node-a", &clock);
    let b = fenced(&shared, "node-b", &clock);
    let c = fenced(&shared, "node-c", &clock);
    let v = vol(24);

    a.lease_acquire(v, "node-a", 10_000).await.unwrap();
    now.fetch_add(30_000, Ordering::SeqCst);

    let (rb, rc) = tokio::join!(
        b.lease_acquire(v, "node-b", 10_000),
        c.lease_acquire(v, "node-c", 10_000),
    );
    let outcomes = [rb.unwrap(), rc.unwrap()];
    let acquired: Vec<&Lease> = outcomes
        .iter()
        .filter_map(|o| match o {
            LeaseOutcome::Acquired(l) => Some(l),
            LeaseOutcome::Held(_) => None,
        })
        .collect();
    assert_eq!(acquired.len(), 1, "exactly one thief wins: {outcomes:?}");
    assert_eq!(acquired[0].epoch, 2);
    // The loser was told who won.
    let held = outcomes
        .iter()
        .find_map(|o| match o {
            LeaseOutcome::Held(l) => Some(l),
            _ => None,
        })
        .unwrap();
    assert_eq!(held.holder, acquired[0].holder);
}

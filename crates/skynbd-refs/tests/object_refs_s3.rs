//! `ObjectRefStore` over a live S3 endpoint — the deployment shape D-M1
//! describes, driven by the same conformance and race checks that run
//! over `MemoryStore`. Env-gated like the store's own S3 tests; a backend
//! that fails the conditional-write probe (SeaweedFS today) reports a
//! skip, mirroring skynbdd's startup behaviour.

use std::sync::Arc;

use skynbd_refs::{conformance, CasOutcome, ObjectRefStore, RefStore};
use skynbd_store::{ObjectStore, S3Store};
use skynbd_types::VolumeId;

#[tokio::test]
async fn object_refs_over_live_s3() {
    let Ok(endpoint) = std::env::var("SKYNBD_S3_TEST_ENDPOINT") else {
        eprintln!("skipped: SKYNBD_S3_TEST_ENDPOINT not set");
        return;
    };
    let access = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or("minioadmin".into());
    let secret = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or("minioadmin".into());
    let store = S3Store::new(
        &endpoint,
        "skynbd-object-refs",
        &access,
        &secret,
        "us-east-1",
        true,
    )
    .await;
    store.ensure_bucket().await.unwrap();
    let store: Arc<dyn ObjectStore> = Arc::new(store);

    if let Err(e) = ObjectRefStore::probe(&store).await {
        eprintln!("skipped: {e}");
        return;
    }

    conformance::exercise_ref_store(&ObjectRefStore::new(Arc::clone(&store))).await;

    // The cross-process race, against the real backend's etag semantics.
    let a = ObjectRefStore::new(Arc::clone(&store));
    let b = ObjectRefStore::new(Arc::clone(&store));
    let v = VolumeId(ulid::Ulid::new());
    let base = match a
        .compare_and_swap(v, "head", None, Some([1u8; 32]))
        .await
        .unwrap()
    {
        CasOutcome::Committed(Some(rv)) => rv,
        other => panic!("create: {other:?}"),
    };
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
}

/// The D-M3 fence over real etag semantics: acquire, foreign write
/// refused, steal after (client-clock) expiry, stale owner fenced.
#[tokio::test]
async fn lease_fence_over_live_s3() {
    let Ok(endpoint) = std::env::var("SKYNBD_S3_TEST_ENDPOINT") else {
        eprintln!("skipped: SKYNBD_S3_TEST_ENDPOINT not set");
        return;
    };
    let access = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or("minioadmin".into());
    let secret = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or("minioadmin".into());
    let store = S3Store::new(
        &endpoint,
        "skynbd-object-refs",
        &access,
        &secret,
        "us-east-1",
        true,
    )
    .await;
    store.ensure_bucket().await.unwrap();
    let store: Arc<dyn ObjectStore> = Arc::new(store);
    if let Err(e) = ObjectRefStore::probe(&store).await {
        eprintln!("skipped: {e}");
        return;
    }

    use skynbd_refs::{Clock, LeaseConfig, LeaseOutcome, RefError};
    use std::sync::atomic::{AtomicU64, Ordering};
    let now = Arc::new(AtomicU64::new(1_000_000));
    let clock: Clock = {
        let now = Arc::clone(&now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let fenced = |holder: &str| {
        ObjectRefStore::new(Arc::clone(&store))
            .with_clock(Arc::clone(&clock))
            .with_owner(LeaseConfig {
                holder: holder.to_string(),
                ttl_ms: 10_000,
            })
    };
    let a = fenced("node-a");
    let b = fenced("node-b");
    let v = VolumeId(ulid::Ulid::new());

    assert!(matches!(
        a.lease_acquire(v, "node-a", 10_000).await.unwrap(),
        LeaseOutcome::Acquired(_)
    ));
    let head = match a
        .compare_and_swap(v, "head", None, Some([1u8; 32]))
        .await
        .unwrap()
    {
        CasOutcome::Committed(Some(rv)) => rv,
        other => panic!("create: {other:?}"),
    };
    // Foreign write: refused despite a correct expectation.
    let err = b
        .compare_and_swap(v, "head", Some(head), Some([2u8; 32]))
        .await
        .unwrap_err();
    assert!(matches!(err, RefError::LeaseLost { .. }), "{err}");

    // Steal after expiry, then the stale owner is fenced.
    now.fetch_add(30_000, Ordering::SeqCst);
    match b.lease_acquire(v, "node-b", 10_000).await.unwrap() {
        LeaseOutcome::Acquired(l) => assert_eq!(l.epoch, 2),
        other => panic!("steal: {other:?}"),
    }
    let err = a
        .compare_and_swap(v, "head", Some(head), Some([9u8; 32]))
        .await
        .unwrap_err();
    assert!(matches!(err, RefError::LeaseLost { .. }), "{err}");
}

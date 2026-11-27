
// ---- review scratch tests (M2.7b-1), not part of the pull request ----

/// S1. The retire's transaction landed and its answer was lost (the
/// store took the PUT and then failed): asked again it is gone, and the
/// row went with the ref. Kills a retire made as two transactions (the
/// ref, then the row), which leaves the row.
#[tokio::test]
async fn review_s1_a_landed_retire_whose_answer_was_lost_took_the_row_with_the_ref() {
    let mut fx = Fixture::new(181).await;
    let commit = published(&mut fx).await;
    let mut remote = fx.remote();
    fx.store.arm_after(FaultOp::Put, "refs", 0).await;
    let first = remote.retire("t", commit).await;
    fx.store.disarm().await;
    assert!(matches!(first, Err(RemoteError::Store(_))), "{first:?}");
    assert_eq!(
        snap(&fx, "t").await,
        (None, None),
        "the ref and its row, together"
    );
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Gone));
}

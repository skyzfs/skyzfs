
/// Scratch S7 (the review of #175, its next look): a retire that object
/// storage fails leaves what the uploader kept of the publish (it may
/// not have landed), and the retire asked again that lands ends it.
/// Mutant: the outcome cleared on an error too (`cleared-on-error`).
#[tokio::test(start_paused = true)]
async fn review_s7_a_retire_that_failed_leaves_the_publish_done() {
    let mut fx = Fixture::new(99).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    assert_eq!(
        up.publishing().ask(asked(state, "t", 0, 7)),
        Answer::Accepted
    );
    until_idle(&mut up, 100).await;
    let head = head_of(&fx).await;
    assert_eq!(up.publishing().progress("t"), Progress::Done(head));

    fx.store.arm(FaultOp::Put, "refs", 0).await;
    let (taken, answer) = retire_of(&mut up, "t", head).await;
    fx.store.disarm().await;
    assert!(taken);
    assert!(matches!(answer, Err(RetireRefused::Store(_))), "{answer:?}");
    assert_eq!(up.publishing().progress("t"), Progress::Done(head));
    assert_eq!(
        retire_of(&mut up, "t", head).await,
        (true, Ok(Retired::Retired))
    );
    assert_eq!(up.publishing().progress("t"), Progress::Unknown);
}

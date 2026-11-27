
// ---- review scratch tests (M2.7b-1), not part of the pull request ----

/// S2. A retire whose read or transaction object storage fails is
/// answered `store` (ask again), not `stopped`, and the uploader goes on:
/// the next retire is made. Kills an error of the store answered as a
/// stop (which tells the agent that this daemon will retire nothing).
#[tokio::test(start_paused = true)]
async fn review_s2_a_retire_that_object_storage_fails_is_store_and_the_uploader_goes_on() {
    let mut fx = Fixture::new(191).await;
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
    let before = snap(&fx, "t").await;
    fx.store.arm(FaultOp::Any, "refs", 0).await;
    let (taken, answer) = retire_of(&mut up, "t", head).await;
    fx.store.disarm().await;
    assert!(taken);
    assert!(matches!(answer, Err(RetireRefused::Store(_))), "{answer:?}");
    assert_eq!(snap(&fx, "t").await, before, "nothing deleted");
    assert_eq!(
        retire_of(&mut up, "t", head).await,
        (true, Ok(Retired::Retired))
    );
}

/// S3. A retire asked before the lease is first held waits for it and
/// is made once it is (here: `gone`, no such snapshot). Kills retires
/// dropped or refused when the driver is made.
#[tokio::test(start_paused = true)]
async fn review_s3_a_retire_asked_before_the_first_take_is_made_after_it() {
    let mut fx = Fixture::new(192).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    let (reply, mut answer) = tokio::sync::oneshot::channel();
    assert!(up.control(Control::Retire {
        name: "nothing".into(),
        commit: [1; 32],
        reply,
    }));
    for _ in 0..3 {
        let wait = up.turn().await;
        assert!(
            matches!(up.shown().state, State::NoLease(_)),
            "{:?}",
            up.shown().state
        );
        assert!(
            answer.try_recv().is_err(),
            "no answer while no lease is held"
        );
        tokio::time::advance(wait).await;
    }
    fx.store.disarm().await;
    for _ in 0..20 {
        match answer.try_recv() {
            Ok(got) => {
                assert_eq!(got, Ok(Retired::Gone));
                return;
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                panic!("the retire asked before the first take was dropped unanswered")
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
        }
        let wait = up.turn().await;
        tokio::time::advance(wait).await;
    }
    panic!("the retire asked before the first take was not answered in 20 turns");
}

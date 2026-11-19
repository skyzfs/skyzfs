// ---------------------------------------------------------------------
// The review's scenarios for the next look at #129 (M2.4f, ef8b227),
// appended to crates/skyzfs-upload/tests/uploader.rs in a worktree of
// the head and never committed there. Each puts one step that must NOT
// end the whole waits between two whole cuts lost, and checks that the
// second waits two minutes, not one. They use the author's helpers of
// the same file (`until_reading`, `until_idle`, `until_doing`,
// `until_state`, `both_pass`, `over_the_bound`, `a_whole_cut_lost`,
// `give_ups`, `Refusals`, `Leasing`, `asked`, `Turned`).

const REVIEW_M24F_MINUTE: Duration = Duration::from_secs(60);

/// S1. A step that fails on object storage between two whole cuts lost
/// does not end the waits.
#[tokio::test(start_paused = true)]
async fn review_m24f_s1_an_error_does_not_end_the_whole_waits() {
    let mut fx = Fixture::new(211).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    // The lease apart, so that the fault fails the cut's step alone.
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 8);
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x08, 8);
    assert_eq!(a_whole_cut_lost(&mut up).await.0, REVIEW_M24F_MINUTE);
    // The heir's next step fails on object storage.
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut seen = None;
    for _ in 0..50 {
        let wait = up.turn().await;
        if let State::Backoff { .. } = up.shown().state {
            seen = Some(wait);
            break;
        }
        tokio::time::advance(wait).await;
    }
    fx.store.disarm().await;
    let wait = seen.unwrap_or_else(|| panic!("no error: {:?}", up.shown()));
    tokio::time::advance(wait).await;
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x09, 9);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, 0));
    assert_eq!(wait, 2 * REVIEW_M24F_MINUTE, "an error ended the whole waits");
}

/// S2. A mismatch at the check of a whole cut, between two whole cuts
/// lost, does not end the waits.
#[tokio::test(start_paused = true)]
async fn review_m24f_s2_a_mismatch_does_not_end_the_whole_waits() {
    let mut fx = Fixture::new(212).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    let other = |group| {
        row::encode(&CommittedCut {
            state: *b"another state...",
            number: 1,
            group,
        })
    };
    // Another writer's row: the next (delta) cut is a mismatch, and the
    // cut after it is whole.
    fx.put_row(other(7)).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 20);
    }
    until_idle(&mut up, 1000).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    // During the wait the row names a group that is not head's: the
    // heir, whole, is a mismatch at its check.
    fx.put_row(other(8)).await;
    let before = up.driver().expect("the driver").figures().clone();
    assert_eq!(a_whole_cut_lost(&mut up).await.0, REVIEW_M24F_MINUTE);
    // Every held cut abandoned, the uploader idles until the next.
    until_idle(&mut up, 100).await;
    let after = up.driver().expect("the driver").figures().clone();
    assert_eq!(after.mismatches, before.mismatches + 1, "the heir's check");
    // The row as head's again: the next whole cut goes on, and is lost.
    fx.put_row(other(7)).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 32, 0x32);
        fx.group(store, 23);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x41, 24);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!(
        (figures.lost_whole, figures.uploaded),
        (2, before.uploaded)
    );
    assert_eq!(wait, 2 * REVIEW_M24F_MINUTE, "a mismatch ended the whole waits");
}

/// S3. A commit refused and given up, between two whole cuts lost, does
/// not end the waits.
#[tokio::test(start_paused = true)]
async fn review_m24f_s3_a_give_up_does_not_end_the_whole_waits() {
    let mut fx = Fixture::new(213).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mode = Arc::new(AtomicU8::new(PASS));
    let make = Refusals::make(fenced(&fx, &clock), &mode);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), make, 8);
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x08, 8);
    assert_eq!(a_whole_cut_lost(&mut up).await.0, REVIEW_M24F_MINUTE);
    until_doing(&mut up, Doing::Committing, 100).await;
    mode.store(REFUSE, Ordering::SeqCst);
    assert_eq!(give_ups(&mut up, 1).await, [SECOND]);
    mode.store(PASS, Ordering::SeqCst);
    // The heir is checked anew and read again: lost this time.
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x09, 9);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, 0));
    assert_eq!(wait, 2 * REVIEW_M24F_MINUTE, "a give-up ended the whole waits");
}

/// S4. A publish made during a whole wait (the lifted R2) does not end
/// the waits: the next whole cut lost waits two minutes.
#[tokio::test(start_paused = true)]
async fn review_m24f_s4_a_publish_does_not_end_the_whole_waits() {
    let mut fx = Fixture::new(214).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    fx.put_row(row::encode(&CommittedCut {
        state: *b"another state...",
        number: 1,
        group: 7,
    }))
    .await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 20);
    }
    until_idle(&mut up, 1000).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    let mut sleep = until_state(&mut up, &clock, 20, |s| {
        matches!(s, State::WholeLost { .. })
    })
    .await;
    let (reply, answer) = tokio::sync::oneshot::channel();
    if up.control(Control::Ask(asked(state, "p", 0, 7), reply)) {
        sleep = Duration::ZERO;
    }
    assert_eq!(answer.await.expect("an answer"), Answer::Accepted);
    let mut turns = 0;
    while matches!(up.shown().state, State::WholeLost { .. }) {
        both_pass(&clock, sleep).await;
        sleep = up.turn().await;
        turns += 1;
        assert!(turns < 1000, "the wait does not end");
    }
    assert!(
        matches!(up.publishing().progress("p"), Progress::Done(_)),
        "{:?}",
        up.publishing().progress("p")
    );
    let uploaded = up.driver().expect("the driver").figures().uploaded;
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x41, 23);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, uploaded));
    assert_eq!(wait, 2 * REVIEW_M24F_MINUTE, "a publish ended the whole waits");
}

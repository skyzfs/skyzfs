
// ---------------------------------------------------------------------
// The review's scenarios for #129 (M2.4f), appended to
// crates/skyzfs-upload/tests/uploader.rs in a worktree of the head and
// never committed there. They use the author's helpers of the same file
// (`until_reading`, `over_the_bound`, `Turned`, `asked`, `snap`).

/// Move tokio's time and the refs object's clock together.
async fn review_m24f_advance(clock: &Clock, by: Duration) {
    tokio::time::advance(by).await;
    clock.pass(u64::try_from(by.as_millis()).expect("ms"));
}

/// The refs object's clock now, in ms.
fn review_m24f_now_ms(clock: &Clock) -> u64 {
    clock.0.load(Ordering::SeqCst)
}

/// Turn until the uploader's state is `want`, both clocks moving by each
/// sleep; at most `limit` turns.
async fn review_m24f_until(
    up: &mut Turned,
    clock: &Clock,
    limit: usize,
    want: impl Fn(&State) -> bool,
) -> Duration {
    let mut states = Vec::new();
    for _ in 0..limit {
        let wait = up.turn().await;
        let state = up.shown().state;
        if want(&state) {
            return wait;
        }
        states.push(state);
        review_m24f_advance(clock, wait).await;
    }
    panic!("not reached after {limit} turns: {states:?}");
}

/// R1. Through whole waits up to the longest, the lease is renewed BY THE
/// REFS OBJECT'S CLOCK: at every turn of each wait the row's expiry is
/// ahead of the refs clock (the author's test never moves the refs clock,
/// so a lease that lapsed would not show there).
#[tokio::test(start_paused = true)]
async fn review_m24f_r1_the_row_never_lapses_in_whole_waits() {
    let mut fx = Fixture::new(201).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    let mut waits = Vec::new();
    for group in 8..14u64 {
        review_m24f_until(&mut up, &clock, 100, |s| {
            matches!(s, State::Working(Doing::Reading, Some(_)))
        })
        .await;
        over_the_bound(&mut fx, &mut up, group as u8, group);
        let mut sleep = review_m24f_until(&mut up, &clock, 20, |s| {
            matches!(s, State::WholeLost { .. })
        })
        .await;
        let State::WholeLost { wait, .. } = up.shown().state else {
            unreachable!()
        };
        waits.push(wait.as_secs());
        let mut turns = 0;
        loop {
            // The row as last kept must still hold when the uploader
            // wakes: checked after the sleep and before the turn, which
            // would take a lapsed lease again.
            review_m24f_advance(&clock, sleep).await;
            let now = review_m24f_now_ms(&clock);
            match up.shown().lease {
                LeaseState::Held { expires_at_ms, .. } => assert!(
                    expires_at_ms > now,
                    "wait {wait:?}, turn {turns}: the row expired at {expires_at_ms}, now {now}"
                ),
                other => panic!("wait {wait:?}, turn {turns}: {other:?}"),
            }
            sleep = up.turn().await;
            if !matches!(up.shown().state, State::WholeLost { .. }) {
                break;
            }
            turns += 1;
            assert!(turns < 1000, "the wait does not end");
        }
    }
    assert_eq!(waits, [60, 120, 240, 480, 900, 900]);
}

/// R2. A publish asked during a whole wait, carried by a cut this
/// uploader committed before the wait, is made DURING the wait, not at
/// its end ("the publishes go on meanwhile").
#[tokio::test(start_paused = true)]
async fn review_m24f_r2_a_publish_is_made_during_a_whole_wait() {
    let mut fx = Fixture::new(202).await;
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
    review_m24f_until(&mut up, &clock, 1000, |s| *s == State::Idle).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    // Another writer names another state's cut of head's group: a
    // mismatch, and the next cut is whole.
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
    review_m24f_until(&mut up, &clock, 1000, |s| *s == State::Idle).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    review_m24f_until(&mut up, &clock, 100, |s| {
        matches!(s, State::Working(Doing::Reading, Some(_)))
    })
    .await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    let mut sleep = review_m24f_until(&mut up, &clock, 20, |s| {
        matches!(s, State::WholeLost { .. })
    })
    .await;
    let lost_at = tokio::time::Instant::now();
    let State::WholeLost { wait, .. } = up.shown().state else {
        unreachable!()
    };
    assert_eq!(wait, Duration::from_secs(60));
    // The agent asks for a publish carried by cut 1 (after 0, group 7).
    let (reply, answer) = tokio::sync::oneshot::channel();
    if up.control(Control::Ask(asked(state, "p", 0, 7), reply)) {
        sleep = Duration::ZERO;
    }
    assert_eq!(answer.await.expect("an answer"), Answer::Accepted);
    let mut turns = 0;
    loop {
        review_m24f_advance(&clock, sleep).await;
        sleep = up.turn().await;
        if let Progress::Done(_) = up.publishing().progress("p") {
            break;
        }
        assert!(
            matches!(up.shown().state, State::WholeLost { .. }),
            "the wait ended ({:?} after the loss, {turns} turns) before the publish was made: {:?}",
            lost_at.elapsed(),
            up.publishing().progress("p")
        );
        turns += 1;
        assert!(turns < 1000);
    }
    assert!(
        lost_at.elapsed() < wait,
        "made {:?} after the loss",
        lost_at.elapsed()
    );
    assert!(matches!(up.shown().state, State::WholeLost { .. }));
    let (found, _) = snap(&fx, "p").await;
    assert!(found.is_some(), "snap/p");
}

/// Turn until the driver is about to take `doing`, with no clock but
/// tokio's.
async fn review_m24f_until_doing(up: &mut Turned, doing: Doing, limit: usize) {
    for _ in 0..limit {
        let wait = up.turn().await;
        if let State::Working(d, Some(_)) = up.shown().state {
            if d == doing {
                return;
            }
        }
        tokio::time::advance(wait).await;
    }
    panic!("not {doing:?} after {limit} turns: {:?}", up.shown());
}

/// R3. A whole cut whose sync of the copies fails is lost before its
/// commit and is followed by the whole wait (the author's test loses a
/// whole cut only at a read).
#[tokio::test(start_paused = true)]
async fn review_m24f_r3_a_whole_cut_lost_at_its_sync_waits() {
    let mut fx = Fixture::new(203).await;
    let mut store = fx.start();
    fx.write(&mut store, 200, 0x20);
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    review_m24f_until_doing(&mut up, Doing::Reading, 100).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 200, 0x21);
        fx.group(store, 8);
    }
    review_m24f_until_doing(&mut up, Doing::Syncing, 200).await;
    fx.disk.fail_sync_of("copies.img");
    let wait = up.turn().await;
    let state = up.shown().state;
    let State::WholeLost { wait: began, .. } = state else {
        panic!("{state:?}, sleep {wait:?}")
    };
    assert_eq!(began, Duration::from_secs(60));
    assert_eq!(up.driver().expect("the driver").figures().lost_whole, 1);
}

/// R4. A whole cut abandoned at the bound after its last slice and before
/// its commit is lost at the commit and is followed by the whole wait.
#[tokio::test(start_paused = true)]
async fn review_m24f_r4_a_whole_cut_gone_at_its_commit_waits() {
    let mut fx = Fixture::new(204).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 64);
    review_m24f_until_doing(&mut up, Doing::Committing, 100).await;
    over_the_bound(&mut fx, &mut up, 0x09, 8);
    let wait = up.turn().await;
    let state = up.shown().state;
    let State::WholeLost { wait: began, why } = state else {
        panic!("{state:?}, sleep {wait:?}")
    };
    assert!(why.contains("it went before its commit"), "{why}");
    assert_eq!(began, Duration::from_secs(60));
    assert_eq!(fx.uploaded().await.group, None, "nothing was committed");
}

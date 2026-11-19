// Appended to crates/skyzfs-upload/tests/uploader.rs for a run (not part of the pull request).

// ---- review scenarios, next look (not part of the pull request) ----

/// U1. F1 again, harder: bursts of sixteen requests (asks, progress,
/// withdrawals) at three moments, 0.25 s before the first renewal, 1 ms
/// before the second and 5 ms before the third, while each check of a
/// cut takes 21 s. Every request is answered and the lease is renewed
/// every third of it, none late.
#[tokio::test(start_paused = true)]
async fn review_bursts_of_requests_do_not_hold_back_the_leases_renewal() {
    let fx = Fixture::new(97).await;
    let store = fx.start();
    let state = store.cut_state_id().expect("the cut state");
    let delay = Arc::new(AtomicU64::new(21_000));
    let slow = Slow {
        inner: store,
        delay: Arc::clone(&delay),
    };
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (inbox, requests) = tokio::sync::mpsc::channel(16);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        |_| {},
    );
    let begun = tokio::time::Instant::now();
    let third = Duration::from_millis(TTL / 3);
    let client = async move {
        let mut answered = 0usize;
        for (i, before) in [(1u32, IDLE), (2, Duration::from_millis(1)), (3, Duration::from_millis(5))] {
            tokio::time::sleep_until(begun + third * i - before).await;
            let mut burst = Vec::new();
            for j in 0..16u32 {
                let inbox = inbox.clone();
                let name = format!("b{i}x{j}");
                burst.push(async move {
                    match j % 3 {
                        0 => format!("{:?}", ask_of(&inbox, |r| Control::Ask(asked(state, &name, 0, 100), r)).await),
                        1 => format!("{:?}", ask_of(&inbox, |r| Control::Progress(name.clone(), r)).await),
                        _ => format!("{:?}", ask_of(&inbox, |r| Control::Withdraw(name.clone(), r)).await),
                    }
                });
            }
            let answers = futures_util::future::join_all(burst).await;
            answered += answers.len();
        }
        tokio::time::sleep_until(begun + Duration::from_secs(80)).await;
        let _ = stop.send(());
        answered
    };
    let (_keeper, answered) = tokio::join!(run, client);
    let takes = way.takes();
    eprintln!("review: bursts: {answered} answered; takes {takes:?}");
    assert_eq!(answered, 48);
    on_time(&takes, "bursts of requests");
    assert!(takes.len() >= 8, "{takes:?}");
}

/// U2. A store started with cuts held names its cut state before its
/// first cut (F5's premise).
#[tokio::test(start_paused = true)]
async fn review_the_cut_state_is_named_before_the_first_cut() {
    let fx = Fixture::new(92).await;
    let store = fx.start();
    let now = skyzfs_upload::Store::cuts(&mut { store });
    eprintln!("review: before the first cut: {now:?}");
    assert!(now.state.is_some());
    assert_eq!(now.count, 0);
}

/// U3. F1 for `cuts`, through the desk as the daemon asks it: the
/// uploader over a desk whose queue takes 21 s for each turn it is woken
/// for (a store answer of more than two thirds of the lease); `cuts`
/// asked at the desk 0.25 s before the first renewal, and a publish
/// through the inbox 0.25 s before the second. The renewals stay on
/// time, and both are answered.
#[tokio::test(start_paused = true)]
async fn review_cuts_at_a_slow_desk_does_not_hold_back_the_leases_renewal() {
    let fx = Fixture::new(98).await;
    let mut store = fx.start();
    let state = store.cut_state_id().expect("the cut state");
    let notify = Arc::new(tokio::sync::Notify::new());
    let waker = Arc::clone(&notify);
    let desk = skyzfs_upload::Desk::new(move || waker.notify_one());
    let queue = {
        let desk = Arc::clone(&desk);
        tokio::spawn(async move {
            loop {
                notify.notified().await;
                if desk.closed() {
                    return;
                }
                tokio::time::sleep(Duration::from_secs(21)).await;
                desk.answer(&mut store);
            }
        })
    };
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(
        skyzfs_upload::Queued::new(Arc::clone(&desk)),
        way.keeper(&fx),
        fenced(&fx, &clock),
        16,
    );
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (inbox, requests) = tokio::sync::mpsc::channel(16);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        |_| {},
    );
    let begun = tokio::time::Instant::now();
    let third = Duration::from_millis(TTL / 3);
    let asking = Arc::clone(&desk);
    let client = async move {
        tokio::time::sleep_until(begun + third - IDLE).await;
        let asked = tokio::time::Instant::now();
        let count = tokio::time::timeout(Duration::from_secs(30), asking.cuts_now()).await;
        let took = asked.elapsed();
        tokio::time::sleep_until(begun + third * 2 - IDLE).await;
        let published = ask_of(&inbox, |r| Control::Ask(asked_of(state), r)).await;
        tokio::time::sleep_until(begun + Duration::from_secs(80)).await;
        let _ = stop.send(());
        (count, took, published)
    };
    let (_keeper, (count, took, published)) = tokio::join!(run, client);
    desk.close();
    let _ = queue.await;
    let takes = way.takes();
    eprintln!("review: cuts at a slow desk: {count:?} after {took:?}; publish {published:?}; takes {takes:?}");
    assert!(matches!(count, Ok(Some(_))), "{count:?}");
    assert_eq!(published, Answer::Accepted);
    on_time(&takes, "cuts at a slow desk");
    assert!(takes.len() >= 8, "{takes:?}");
}

fn asked_of(state: [u8; 16]) -> Request {
    asked(state, "t", 0, 100)
}

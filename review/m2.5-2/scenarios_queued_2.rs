// Appended to crates/skyzfs-upload/tests/queued.rs for a run (not part of the pull request).

// ---- review scenarios, next look (not part of the pull request) ----

/// D1. Ten asks of the count at once are answered together, alike, and
/// are not counted as calls.
#[tokio::test]
async fn review_asks_of_the_count_at_once_are_answered_alike() {
    let mut fx = Fixture::new(61).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), store, woken);
    let asks: Vec<_> = (0..10)
        .map(|_| {
            let desk = Arc::clone(&desk);
            tokio::spawn(async move {
                tokio::time::timeout(Duration::from_secs(5), desk.cuts_now()).await
            })
        })
        .collect();
    let mut got = Vec::new();
    for ask in asks {
        got.push(ask.await.expect("joined").expect("within 5 s"));
    }
    eprintln!("review: ten asks: {:?}", got[0]);
    assert!(got.iter().all(|g| g.is_some() && *g == got[0]), "{got:?}");
    assert_eq!(got[0].map(|n| n.count), Some(1));
    assert_eq!(desk.answered(), 0, "counts are not calls");
    desk.close();
    let _ = queue.join();
}

/// D2. An ask left at a desk whose queue's thread takes no turn, then
/// closed: the ask ends with nothing at once (`ended`), and a later ask
/// too, without waiting.
#[tokio::test]
async fn review_an_ask_of_the_count_ends_at_the_close() {
    let desk = Desk::new(|| {});
    let asking = {
        let desk = Arc::clone(&desk);
        tokio::spawn(async move { desk.cuts_now().await })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    desk.close();
    let got = tokio::time::timeout(Duration::from_secs(2), asking)
        .await
        .expect("ended at the close")
        .expect("joined");
    assert_eq!(got, None);
    let later = tokio::time::timeout(Duration::from_secs(2), desk.cuts_now())
        .await
        .expect("at once");
    assert_eq!(later, None);
}

/// D3. The count is taken at the turn that answers it, not at the ask: a
/// cut counted on the queue's thread after the ask and before that turn
/// is in it (so no count is older than its ask).
#[tokio::test]
async fn review_the_count_is_taken_at_the_turn_after_the_ask() {
    let mut fx = Fixture::new(62).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let (wake, woken) = std::sync::mpsc::channel::<()>();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = {
        let desk = Arc::clone(&desk);
        std::thread::spawn(move || {
            // The first wake: a cut counted on this thread, then the turn.
            let _ = woken.recv();
            fx.write(&mut store, 11, 0x11);
            fx.group(&mut store, 8);
            desk.answer(&mut store);
            store
        })
    };
    let got = tokio::time::timeout(Duration::from_secs(5), desk.cuts_now())
        .await
        .expect("answered");
    eprintln!("review: counted after the ask: {got:?}");
    assert_eq!(got.map(|n| n.count), Some(2));
    let _ = queue.join();
}

/// D4. Asks whose askers gave up while the queue's thread took no turn
/// do not pile up or hold the next: a thousand asks of 1 ms each, then
/// one answered at the next turn.
#[tokio::test]
async fn review_asks_given_up_do_not_pile_up() {
    let mut fx = Fixture::new(63).await;
    let store = fx.start();
    let (wake, woken) = std::sync::mpsc::channel::<()>();
    let (go, gate) = std::sync::mpsc::channel::<()>();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = {
        let desk = Arc::clone(&desk);
        let mut store = store;
        std::thread::spawn(move || {
            let _ = gate.recv();
            loop {
                let _ = woken.recv_timeout(Duration::from_millis(5));
                if desk.closed() {
                    return store;
                }
                desk.answer(&mut store);
            }
        })
    };
    for _ in 0..1000 {
        let r = tokio::time::timeout(Duration::from_millis(1), desk.cuts_now()).await;
        assert!(r.is_err());
    }
    let _ = go.send(());
    let began = Instant::now();
    let got = tokio::time::timeout(Duration::from_secs(5), desk.cuts_now())
        .await
        .expect("answered");
    eprintln!("review: after 1000 given up: {got:?} in {:?}", began.elapsed());
    assert!(got.is_some());
    desk.close();
    let _ = queue.join();
    let _ = &mut fx;
}

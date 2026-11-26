    // Review scratch probe G1 for #149's fifth look (head 6ec5909): pasted
    // into `mod socket` of `bins/skyzfsd/src/control/tests.rs` in a scratch
    // worktree, never committed. Run with `cargo test -p skyzfsd --lib g1_
    // -- --nocapture`.
    //
    // G1: skyzfsd's channel answers one connection at a time. A publish
    // whose connection waits behind another client's request, and whose
    // client gives up (the agent's ask has no answer: its answer is lost),
    // is still read and handed to the handler (the uploader) once the
    // thread reaches it: its acceptance is bounded by the queue, not by the
    // client's ASK_LIMIT. The handler here holds the first request for 2 s
    // (an uploader in a long step), and the publish's client waits 0.5 s.
    #[test]
    fn g1_a_queued_publish_runs_after_its_client_gave_up() {
        use std::sync::Mutex;
        use std::time::Instant;
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let ran: Arc<Mutex<Vec<(String, Instant)>>> = Arc::new(Mutex::new(Vec::new()));
        let (entered, inside) = std::sync::mpsc::channel();
        let handler: Handler = {
            let ran = Arc::clone(&ran);
            let mut first = true;
            Box::new(move |request: &Request, _: Vec<u8>| {
                if let Ok(mut ran) = ran.lock() {
                    ran.push((request.line(), Instant::now()));
                }
                if first {
                    first = false;
                    let _ = entered.send(());
                    std::thread::sleep(Duration::from_secs(2));
                }
                Reply::Accepted
            })
        };
        let channel = Channel::answering(
            UnixListener::bind(&path).expect("bound"),
            &path,
            handler,
            server::same_user_or_root,
            quiet(),
            REQUEST_LIMIT,
        )
        .expect("open");
        let other_path = path.clone();
        let other = std::thread::spawn(move || {
            ask(&other_path, &Request::Cuts, &[], Duration::from_secs(10))
        });
        inside
            .recv_timeout(Duration::from_secs(10))
            .expect("the other request is in the handler");
        let sent = Instant::now();
        let reply = ask(&path, &publish(None), b"12345", Duration::from_millis(500));
        let gave_up = Instant::now();
        let _ = other.join();
        std::thread::sleep(Duration::from_millis(500));
        let ran = ran.lock().map(|r| r.clone()).unwrap_or_default();
        let publish_ran = ran.iter().find(|(line, _)| line.starts_with("publish"));
        eprintln!(
            "G1: the client's answer {reply:?} after {:?}; handler runs {:?}",
            gave_up - sent,
            ran.iter()
                .map(|(l, at)| (l.split(' ').next().unwrap_or("").to_string(), at.saturating_duration_since(sent)))
                .collect::<Vec<_>>()
        );
        assert!(reply.is_err(), "the client had no answer");
        let (_, at) = publish_ran.expect("the publish was handed to the handler");
        eprintln!(
            "G1: the publish was handed on {:?} after it was sent, {:?} after its client gave up",
            at.saturating_duration_since(sent),
            at.saturating_duration_since(gave_up)
        );
        assert!(*at > gave_up, "handed on after the client gave up");
        assert!(channel.close());
    }

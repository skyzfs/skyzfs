    /// The review's scenario for #133 (G2's bound): the tries of the
    /// connect against a full backlog, 20 times at each limit, and how
    /// soon a connect is made once room is made in the backlog 1.5 s into
    /// a wait of 5 s (what the pause's cap of 50 ms gives).
    #[test]
    fn review_q7_tries_on_a_full_backlog() {
        let dir = dir();
        let path = dir.path().join("full.sock");
        let listener = small_backlog(&path);
        let held = fill_backlog(&path);
        for limit_ms in [300u64, 1000] {
            let mut tries_seen = Vec::new();
            let mut took_max = Duration::ZERO;
            for _ in 0..20 {
                let mut tries = 0;
                let began = std::time::Instant::now();
                let _ = connect_counted(&path, Duration::from_millis(limit_ms), &mut tries);
                took_max = took_max.max(began.elapsed());
                tries_seen.push(tries);
            }
            eprintln!(
                "REVIEW limit {limit_ms} ms: tries min {} max {} all {:?}; longest {:?}",
                tries_seen.iter().min().unwrap(),
                tries_seen.iter().max().unwrap(),
                tries_seen,
                took_max
            );
        }
        drop(held);
        drop(listener);
        // Room made 1.5 s into a wait of 5 s.
        let path = dir.path().join("room.sock");
        let listener = small_backlog(&path);
        let held = fill_backlog(&path);
        let freer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            let mut held = held;
            let made = std::time::Instant::now();
            // Take the waiting connections, so the backlog has room.
            listener.set_nonblocking(true).unwrap();
            while let Ok((c, _)) = listener.accept() {
                drop(c);
            }
            held.clear();
            (made, listener)
        });
        let mut tries = 0;
        let began = std::time::Instant::now();
        let connected = connect_counted(&path, Duration::from_secs(5), &mut tries);
        let at = began.elapsed();
        let (made, listener) = freer.join().unwrap();
        eprintln!(
            "REVIEW room made 1.5 s in: {:?} after {} tries at {:?}, {:?} after the room was made",
            connected.as_ref().map(|_| "connected").map_err(|e| e.kind()),
            tries,
            at,
            std::time::Instant::now().saturating_duration_since(made)
        );
        drop(listener);
    }


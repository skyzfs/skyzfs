    /// The review's scenario for #136: the new test's steps, `ROOM_RUNS`
    /// times (default 30), printing how late each connect came after the
    /// room and at which try. Inserted into `mod socket` for a run.
    #[test]
    fn review_q7b_room_lateness() {
        let runs: usize = std::env::var("ROOM_RUNS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);
        let dir = dir();
        let mut lates = Vec::new();
        for i in 0..runs {
            let path = dir.path().join(format!("room{i}.sock"));
            let listener = small_backlog(&path);
            let held = fill_backlog(&path);
            let room = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(1500));
                let taken = listener.accept().expect("a connection waiting");
                (std::time::Instant::now(), taken, listener)
            });
            let mut tries = 0;
            let began = std::time::Instant::now();
            let stream = connect_counted(&path, Duration::from_secs(5), &mut tries)
                .expect("connected once the backlog had room");
            let connected = std::time::Instant::now();
            let (made, _taken, _listener) = room.join().expect("the thread that made room");
            let late = connected.saturating_duration_since(made);
            eprintln!(
                "REVIEW room run {i}: late {:.1} ms, try {tries}, room made at {:.1} ms",
                late.as_secs_f64() * 1e3,
                made.saturating_duration_since(began).as_secs_f64() * 1e3
            );
            lates.push(late);
            drop(stream);
            drop(held);
        }
        lates.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        eprintln!(
            "REVIEW room summary: {} runs, min {:.1} median {:.1} max {:.1} ms; at or over 200 ms: {}; over 100 ms: {}",
            lates.len(),
            ms(lates[0]),
            ms(lates[lates.len() / 2]),
            ms(lates[lates.len() - 1]),
            lates.iter().filter(|l| **l >= Duration::from_millis(200)).count(),
            lates.iter().filter(|l| **l > Duration::from_millis(100)).count()
        );
    }


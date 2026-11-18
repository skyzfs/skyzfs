// Appended to bins/skyzfsd/src/control/tests.rs for a run (not part of the pull request).

// ---- review scenarios (not part of the pull request) ----
#[cfg(target_os = "linux")]
mod review {
    use std::os::unix::net::UnixStream;
    use std::time::Instant;

    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a directory")
    }

    /// Many clients at once: each answered, one at a time.
    #[test]
    fn review_concurrent_clients_are_each_answered() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let handler: Handler = Box::new(|_: &Request, _| {
            std::thread::sleep(Duration::from_millis(50));
            Reply::Pending
        });
        let channel = Channel::open(&path, handler).expect("open");
        let began = Instant::now();
        let threads: Vec<_> = (0..20)
            .map(|_| {
                let path = path.clone();
                std::thread::spawn(move || ask(&path, &Request::Progress("t".into()), &[], IO_LIMIT))
            })
            .collect();
        for t in threads {
            assert_eq!(t.join().expect("thread").expect("answered"), Reply::Pending);
        }
        let took = began.elapsed();
        eprintln!("review: 20 clients answered in {took:?}");
        assert!(took >= Duration::from_millis(20 * 50), "serial: {took:?}");
        assert_eq!(channel.figures().answered, 20);
        assert!(channel.close());
    }

    /// Two requests on one connection: the first is answered, the second
    /// is not read, and the client still reads the answer.
    #[test]
    fn review_extra_bytes_after_the_request_are_ignored() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let mut raw = UnixStream::connect(&path).expect("connect");
        raw.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
        raw.write_all(format!("{HELLO}\nprogress name=a\nprogress name=b\n").as_bytes())
            .expect("write");
        let mut back = Vec::new();
        let read = raw.read_to_end(&mut back);
        let back = String::from_utf8_lossy(&back).into_owned();
        eprintln!("review: extra bytes: read {read:?}, back {back:?}");
        assert_eq!(back, format!("{HELLO}\npending\n"));
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// A line over the limit: refused, and the refusal reaches the client
    /// although the daemon closes with bytes unread.
    #[test]
    fn review_an_oversized_line_is_refused_and_the_refusal_reaches_the_client() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        for (sent, expected) in [
            (format!("{}\n", "x".repeat(100_000)), "refused version"),
            (format!("{HELLO}\nprogress name={}\n", "y".repeat(100_000)), "refused malformed"),
            (format!("{HELLO}\nprogress name={}\n", "y".repeat(LINE_MAX - 15)), "pending"),
            (format!("{HELLO}\nprogress name={}\n", "y".repeat(LINE_MAX - 14)), "refused malformed"),
        ] {
            let mut raw = UnixStream::connect(&path).expect("connect");
            raw.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
            raw.set_write_timeout(Some(Duration::from_secs(15))).expect("timeout");
            let written = raw.write_all(sent.as_bytes());
            let mut back = Vec::new();
            let read = raw.read_to_end(&mut back);
            let back = String::from_utf8_lossy(&back).into_owned();
            eprintln!("review: oversized {}: written {written:?}, read {read:?}, back {back:?}", sent.len());
            assert!(back.starts_with(&format!("{HELLO}\n{expected}")), "{back:?}");
        }
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// A partial request that stays open: closed after IO_LIMIT, and a
    /// client behind it waits that long.
    #[test]
    fn review_a_silent_client_holds_the_next_for_io_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let mut silent = UnixStream::connect(&path).expect("connect");
        silent.write_all(format!("{HELLO}\n{}\n12", publish(None).line()).as_bytes()).expect("write");
        std::thread::sleep(Duration::from_millis(100));
        let began = Instant::now();
        let reply = ask(&path, &Request::Progress("t".into()), &[], Duration::from_secs(30)).expect("answered");
        let took = began.elapsed();
        eprintln!("review: behind a silent client: {took:?}");
        assert_eq!(reply, Reply::Pending);
        assert!(took >= IO_LIMIT - Duration::from_millis(500), "{took:?}");
        let mut back = Vec::new();
        let _ = silent.read_to_end(&mut back);
        assert!(back.is_empty(), "the silent one got {back:?}");
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// A client that sends a byte every second holds the channel for as
    /// long as it goes on: no total limit on a connection.
    #[test]
    fn review_a_trickling_client_holds_the_channel_past_io_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let trickle_path = path.clone();
        let trickler = std::thread::spawn(move || {
            let mut s = UnixStream::connect(&trickle_path).expect("connect");
            let line = format!("{HELLO}\nprogress name={}\n", "z".repeat(40));
            for b in line.as_bytes() {
                if s.write_all(&[*b]).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(900));
            }
            let mut back = Vec::new();
            let _ = s.read_to_end(&mut back);
            String::from_utf8_lossy(&back).into_owned()
        });
        std::thread::sleep(Duration::from_millis(300));
        let began = Instant::now();
        let reply = ask(&path, &Request::Progress("t".into()), &[], Duration::from_secs(12));
        eprintln!("review: behind a trickler, limit 12 s: {reply:?} after {:?}", began.elapsed());
        assert!(reply.is_err(), "answered behind the trickler: {reply:?}");
        // The close is bounded all the same.
        let began = Instant::now();
        let ended = channel.close();
        eprintln!("review: close with a trickler in: ended {ended} after {:?}", began.elapsed());
        assert!(!ended);
        assert!(began.elapsed() < Duration::from_secs(7));
        let back = trickler.join().expect("trickler");
        eprintln!("review: the trickler got {back:?}");
    }

    /// Codex's P2: with the thread held by a trickling client, connections
    /// that fill the backlog make close() wait in its connect.
    #[test]
    #[ignore]
    fn review_close_with_a_full_backlog() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let trickle_path = path.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let trickling = Arc::clone(&stop);
        let trickler = std::thread::spawn(move || {
            let mut s = UnixStream::connect(&trickle_path).expect("connect");
            let _ = s.write_all(format!("{HELLO}\n").as_bytes());
            while !trickling.load(Ordering::SeqCst) {
                if s.write_all(b"z").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(500));
            }
            drop(s);
        });
        std::thread::sleep(Duration::from_millis(300));
        // Fill the backlog: connect and close until a connect would wait.
        let mut made = 0u64;
        loop {
            let p = path.clone();
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let r = UnixStream::connect(&p).map(drop);
                let _ = tx.send(r);
            });
            match rx.recv_timeout(Duration::from_secs(2)) {
                Ok(Ok(())) => made += 1,
                Ok(Err(e)) => panic!("connect {made}: {e}"),
                Err(_) => break,
            }
            if made > 100_000 {
                break;
            }
        }
        eprintln!("review: backlog full after {made} connections");
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let began = Instant::now();
            let ended = channel.close();
            let _ = tx.send((ended, began.elapsed()));
        });
        let closed = rx.recv_timeout(Duration::from_secs(20));
        eprintln!("review: close with a full backlog: {closed:?}");
        stop.store(true, Ordering::SeqCst);
        let _ = trickler.join();
        assert!(closed.is_ok(), "close() did not return within 20 s");
    }
}

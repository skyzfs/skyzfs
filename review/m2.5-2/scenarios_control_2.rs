// Appended to bins/skyzfsd/src/control/tests.rs for a run (not part of the pull request).

// ---- review scenarios, next look (not part of the pull request) ----
#[cfg(target_os = "linux")]
mod review {
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::time::Instant;

    use super::*;

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().expect("a directory")
    }

    fn quiet() -> super::super::server::Post {
        Box::new(|_| {})
    }

    /// Fill the listener's backlog at `path` with non-blocking connects
    /// until one would wait.
    fn fill(path: &std::path::Path) -> Vec<rustix::fd::OwnedFd> {
        use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
        let address = SocketAddrUnix::new(path).expect("an address");
        let mut waiting = Vec::new();
        loop {
            let socket = rustix::net::socket_with(
                AddressFamily::UNIX,
                SocketType::STREAM,
                SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
                None,
            )
            .expect("a socket");
            match rustix::net::connect(&socket, &address) {
                Ok(()) => waiting.push(socket),
                Err(rustix::io::Errno::AGAIN) => return waiting,
                Err(e) => panic!("connect {}: {e}", waiting.len()),
            }
            assert!(waiting.len() < 200_000);
        }
    }

    /// R1. Many clients at once: each answered, one at a time.
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
        assert_eq!(channel.figures().answered, 20);
        assert!(channel.close());
    }

    /// R2. Over-long lines: refused, and the refusal reaches the client.
    #[test]
    fn review_an_oversized_line_is_refused_and_the_refusal_reaches_the_client() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        for (sent, expected) in [
            (format!("{}\n", "x".repeat(100_000)), "refused version"),
            (format!("{HELLO}\nprogress name={}\n", "y".repeat(100_000)), "refused malformed"),
            (format!("{HELLO}\nprogress name={}\n", "y".repeat(128)), "pending"),
            (format!("{HELLO}\nprogress name={}\n", "y".repeat(129)), "refused name"),
            (format!("{HELLO}\nwithdraw name=a\tb\n"), "refused name"),
        ] {
            let mut raw = UnixStream::connect(&path).expect("connect");
            raw.set_read_timeout(Some(Duration::from_secs(15))).expect("timeout");
            raw.set_write_timeout(Some(Duration::from_secs(15))).expect("timeout");
            let written = raw.write_all(sent.as_bytes());
            let mut back = Vec::new();
            let read = raw.read_to_end(&mut back);
            let back = String::from_utf8_lossy(&back).into_owned();
            eprintln!("review: line of {}: written {written:?}, read {read:?}, back {back:?}", sent.len());
            assert!(back.starts_with(&format!("{HELLO}\n{expected}")), "{back:?}");
        }
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// R3. A partial request that stays open holds the next client for
    /// REQUEST_LIMIT at most.
    #[test]
    fn review_a_silent_client_holds_the_next_for_the_request_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let mut silent = UnixStream::connect(&path).expect("connect");
        silent
            .write_all(format!("{HELLO}\n{}\n12", publish(None).line()).as_bytes())
            .expect("write");
        std::thread::sleep(Duration::from_millis(100));
        let began = Instant::now();
        let reply = ask(&path, &Request::Progress("t".into()), &[], Duration::from_secs(30))
            .expect("answered");
        let took = began.elapsed();
        eprintln!("review: behind a silent client: {took:?}");
        assert_eq!(reply, Reply::Pending);
        assert!(took < REQUEST_LIMIT + Duration::from_millis(500), "{took:?}");
        let mut back = Vec::new();
        let _ = silent.read_to_end(&mut back);
        assert!(back.is_empty(), "the silent one got {back:?}");
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// R4. A client that sends a byte every 0.9 s is cut off at
    /// REQUEST_LIMIT, and the client behind it is answered.
    #[test]
    fn review_a_trickling_client_is_cut_off_at_the_request_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Pending)).expect("open");
        let trickle_path = path.clone();
        let trickler = std::thread::spawn(move || {
            let mut s = UnixStream::connect(&trickle_path).expect("connect");
            let line = format!("{HELLO}\nprogress name={}\n", "z".repeat(40));
            let began = Instant::now();
            for b in line.as_bytes() {
                if s.write_all(&[*b]).is_err() {
                    return (true, began.elapsed());
                }
                std::thread::sleep(Duration::from_millis(900));
            }
            (false, began.elapsed())
        });
        std::thread::sleep(Duration::from_millis(300));
        let began = Instant::now();
        let reply = ask(&path, &Request::Progress("t".into()), &[], Duration::from_secs(12));
        let took = began.elapsed();
        eprintln!("review: behind a trickler, limit 12 s: {reply:?} after {took:?}");
        assert_eq!(reply.ok(), Some(Reply::Pending));
        assert!(took < REQUEST_LIMIT + Duration::from_millis(500), "{took:?}");
        let (cut, after) = trickler.join().expect("trickler");
        eprintln!("review: the trickler cut off {cut} after {after:?}");
        assert!(cut);
        assert_eq!(asked.count.load(Ordering::SeqCst), 1);
        assert!(channel.close());
    }

    /// R5. Codex's P2 with the daemon's own socket: the thread held by a
    /// trickler, the backlog full (somaxconn), close() returns within
    /// CLOSE_LIMIT.
    #[test]
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
        });
        std::thread::sleep(Duration::from_millis(300));
        let waiting = fill(&path);
        eprintln!("review: backlog full after {} connections", waiting.len());
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
        drop(waiting);
        let (_, took) = closed.expect("close() returned within 20 s");
        assert!(took < server::CLOSE_LIMIT + Duration::from_secs(1), "{took:?}");
    }

    /// R6. A slow but reasonable client: a manifest of 1 MiB in 64 KiB
    /// pieces every 0.3 s (about 5 s) is taken whole.
    #[test]
    fn review_a_large_manifest_sent_in_pieces_within_the_limit_is_taken() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Accepted)).expect("open");
        let manifest: Vec<u8> = (0..MANIFEST_MAX).map(|i| (i % 251) as u8).collect();
        let request = Request::Publish(Publish {
            name: "big".into(),
            attachment: A,
            state: S,
            after: 1,
            group: 2,
            manifest: manifest.len(),
            expiry: None,
        });
        let mut s = UnixStream::connect(&path).expect("connect");
        s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
        s.write_all(format!("{HELLO}\n{}\n", request.line()).as_bytes()).expect("line");
        let began = Instant::now();
        for piece in manifest.chunks(64 * 1024) {
            s.write_all(piece).expect("a piece");
            std::thread::sleep(Duration::from_millis(300));
        }
        let mut back = String::new();
        s.read_to_string(&mut back).expect("read");
        eprintln!("review: 1 MiB in pieces over {:?}: {back:?}", began.elapsed());
        assert_eq!(back, format!("{HELLO}\naccepted\n"));
        assert_eq!(asked.manifest.lock().map(|m| m.clone()).ok(), Some(manifest));
        assert!(channel.close());
    }

    /// R7. A manifest whose second half comes after REQUEST_LIMIT: no
    /// answer, the handler not asked, the next client answered.
    #[test]
    fn review_a_manifest_late_past_the_limit_is_not_taken() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let asked = Arc::new(Asked::default());
        let channel = Channel::open(&path, recording(&asked, Reply::Accepted)).expect("open");
        let late_path = path.clone();
        let late = std::thread::spawn(move || {
            let mut s = UnixStream::connect(&late_path).expect("connect");
            s.set_read_timeout(Some(Duration::from_secs(30))).expect("timeout");
            let request = publish(None); // manifest=5
            s.write_all(format!("{HELLO}\n{}\n12", request.line()).as_bytes()).expect("first");
            std::thread::sleep(REQUEST_LIMIT + Duration::from_secs(1));
            let second = s.write_all(b"345");
            let mut back = Vec::new();
            let read = s.read_to_end(&mut back);
            (second.is_ok(), read.is_ok(), String::from_utf8_lossy(&back).into_owned())
        });
        std::thread::sleep(Duration::from_millis(200));
        let began = Instant::now();
        let reply = ask(&path, &Request::Progress("t".into()), &[], Duration::from_secs(30));
        eprintln!("review: behind a late manifest: {reply:?} after {:?}", began.elapsed());
        let (second, read, back) = late.join().expect("late");
        eprintln!("review: the late one: second write ok {second}, read ok {read}, back {back:?}");
        assert!(back.is_empty(), "{back:?}");
        assert_eq!(reply.ok(), Some(Reply::Accepted));
        assert_eq!(asked.count.load(Ordering::SeqCst), 1, "only the second asked");
        assert!(channel.close());
    }

    /// R8. A handler that takes longer than REQUEST_LIMIT: its answer is
    /// written all the same (the deadline is the request's, not the
    /// answer's).
    #[test]
    fn review_a_slow_handler_is_not_cut_by_the_request_limit() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let handler: Handler = Box::new(|_: &Request, _| {
            std::thread::sleep(REQUEST_LIMIT + Duration::from_secs(2));
            Reply::Pending
        });
        let channel = Channel::open(&path, handler).expect("open");
        let reply = ask(&path, &Request::Progress("t".into()), &[], ASK_LIMIT);
        eprintln!("review: a handler of 12 s: {reply:?}");
        assert_eq!(reply.ok(), Some(Reply::Pending));
        assert!(channel.close());
    }

    /// R9. The deadline runs from the accept: a client queued behind a
    /// slow handler (12 s) whose bytes have been waiting is answered.
    #[test]
    fn review_a_client_queued_behind_a_slow_handler_is_answered() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let handler: Handler = Box::new(|request: &Request, _| {
            if *request == Request::Withdraw("slow".into()) {
                std::thread::sleep(REQUEST_LIMIT + Duration::from_secs(2));
            }
            Reply::Pending
        });
        let channel = Channel::open(&path, handler).expect("open");
        let first_path = path.clone();
        let first = std::thread::spawn(move || {
            ask(&first_path, &Request::Withdraw("slow".into()), &[], ASK_LIMIT)
        });
        std::thread::sleep(Duration::from_millis(200));
        let reply = ask(&path, &Request::Progress("t".into()), &[], ASK_LIMIT);
        eprintln!("review: queued behind 12 s: {reply:?}");
        assert_eq!(reply.ok(), Some(Reply::Pending));
        assert_eq!(first.join().expect("first").ok(), Some(Reply::Pending));
        assert!(channel.close());
    }

    /// R10. The wake on a path with nothing behind it, or no file, does
    /// not wait or panic.
    #[test]
    fn review_the_wake_of_nothing_returns() {
        let dir = dir();
        let path = dir.path().join(SOCKET);
        let began = Instant::now();
        server::wake(&path);
        drop(UnixListener::bind(&path).expect("bound"));
        server::wake(&path);
        let long = dir.path().join("x".repeat(200));
        server::wake(&long);
        assert!(began.elapsed() < Duration::from_secs(1));
    }
}

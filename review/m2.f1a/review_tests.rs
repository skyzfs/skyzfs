
    /// The reviewer's tests of #166 (F1 (a)), put inside `mod socket` in a
    /// scratch worktree and taken out after. Not part of the pull request.
    mod review166 {
        use std::net::Shutdown;
        use std::sync::mpsc;

        use super::super::super::server::hung_up;
        use super::*;

        /// The bytes of a whole publish, as `ask` writes them.
        fn whole_publish() -> Vec<u8> {
            format!("{HELLO}\n{}\nhello", publish(None).line()).into_bytes()
        }

        /// A connection made to a fresh listener, with `act` done by the
        /// client either before the accept (the connection waits in the
        /// backlog) or after it; the server's end is read whole (as the
        /// channel does) unless `read` is false. `hung_up` on the server's
        /// end, asked twice.
        fn verdict(act: fn(&mut UnixStream), before_accept: bool, read: bool) -> (bool, bool) {
            let dir = dir();
            let path = dir.path().join("s");
            let listener = UnixListener::bind(&path).expect("bind");
            let mut client = UnixStream::connect(&path).expect("connect");
            let server = if before_accept {
                act(&mut client);
                listener.accept().expect("accept").0
            } else {
                let server = listener.accept().expect("accept").0;
                act(&mut client);
                server
            };
            // Let the client's act reach the server's end.
            std::thread::sleep(Duration::from_millis(20));
            if read {
                let mut got = vec![0; whole_publish().len()];
                (&server).read_exact(&mut got).expect("the request, whole");
                assert_eq!(got, whole_publish());
            }
            let first = hung_up(&server);
            let second = hung_up(&server);
            drop(client);
            (first, second)
        }

        /// `hung_up` on real Unix stream sockets, for each way a client
        /// can end: gone only once it has closed (or shut down both
        /// ways); a half-close, a client that only stopped reading, and
        /// one that waits are there. Deterministic: each case has acted
        /// before the server's end is polled. Kills: RDHUP taken as hung
        /// up; IN taken as hung up; HUP not taken.
        #[test]
        fn r1_hung_up_on_real_sockets() {
            let cases: [(&str, fn(&mut UnixStream), bool, bool, bool); 9] = [
                ("waits", |c| c.write_all(&whole_publish()).expect("w"), false, true, false),
                (
                    "shut write",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        c.shutdown(Shutdown::Write).expect("shutdown");
                    },
                    false,
                    true,
                    false,
                ),
                (
                    "shut read",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        c.shutdown(Shutdown::Read).expect("shutdown");
                    },
                    false,
                    true,
                    false,
                ),
                (
                    "shut both, open",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        c.shutdown(Shutdown::Both).expect("shutdown");
                    },
                    false,
                    true,
                    true,
                ),
                (
                    "shut both, polled unread",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        let _ = c.shutdown(Shutdown::Both);
                    },
                    false,
                    false,
                    true,
                ),
                ("queued, waits", |c| c.write_all(&whole_publish()).expect("w"), true, true, false),
                (
                    "queued, shut write",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        c.shutdown(Shutdown::Write).expect("shutdown");
                    },
                    true,
                    true,
                    false,
                ),
                (
                    "queued, shut both",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        c.shutdown(Shutdown::Both).expect("shutdown");
                    },
                    true,
                    true,
                    true,
                ),
                (
                    "trailing bytes, shut both",
                    |c| {
                        c.write_all(&whole_publish()).expect("w");
                        c.write_all(b"more than the request").expect("w");
                        c.shutdown(Shutdown::Both).expect("shutdown");
                    },
                    false,
                    true,
                    true,
                ),
            ];
            for (name, act, before, read, gone) in cases {
                let (first, second) = verdict(act, before, read);
                assert_eq!((first, second), (gone, gone), "{name}");
            }
        }

        /// A truly closed client (its last descriptor closed), queued and
        /// not: gone. Kills: HUP not taken.
        #[test]
        fn r1b_a_closed_client_is_gone() {
            for before_accept in [false, true] {
                let dir = dir();
                let path = dir.path().join("s");
                let listener = UnixListener::bind(&path).expect("bind");
                let mut client = UnixStream::connect(&path).expect("connect");
                let server = if before_accept {
                    client.write_all(&whole_publish()).expect("w");
                    drop(client);
                    listener.accept().expect("accept").0
                } else {
                    let server = listener.accept().expect("accept").0;
                    client.write_all(&whole_publish()).expect("w");
                    drop(client);
                    server
                };
                std::thread::sleep(Duration::from_millis(20));
                let mut got = vec![0; whole_publish().len()];
                (&server).read_exact(&mut got).expect("the request, whole");
                assert!(hung_up(&server), "queued: {before_accept}");
            }
        }

        /// A client that shuts its writing down while its connection
        /// waits in the queue is answered when its turn comes: the
        /// half-close has certainly happened before the request is read,
        /// unlike in `a_client_that_only_stops_writing_is_answered`.
        /// Kills: RDHUP taken as hung up, every time.
        #[test]
        fn r2_a_half_closed_client_queued_is_answered() {
            let dir = dir();
            let path = dir.path().join(SOCKET);
            let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (release, gate) = mpsc::channel();
            let channel = Channel::open(&path, holding(&asked, gate)).expect("open");
            let first = {
                let path = path.clone();
                std::thread::spawn(move || {
                    ask(&path, &Request::Progress("hold".into()), &[], IO_LIMIT)
                })
            };
            assert_eq!(asked_at_least(&asked, 1), vec!["hold".to_string()]);
            let mut half = UnixStream::connect(&path).expect("connect");
            half.write_all(&whole_publish()).expect("write");
            half.shutdown(Shutdown::Write).expect("shutdown");
            release.send(()).expect("release");
            assert_eq!(first.join().expect("first").ok(), Some(Reply::Pending));
            half.set_read_timeout(Some(IO_LIMIT)).expect("timeout");
            let mut back = String::new();
            half.read_to_string(&mut back).expect("read");
            assert_eq!(back, format!("{HELLO}\npending\n"));
            assert_eq!(
                asked_at_least(&asked, 2),
                vec!["hold".to_string(), "publish pool-7.b_c".to_string()]
            );
            assert!(channel.close());
        }

        /// A withdraw (the other request with an effect) whose client
        /// gave up in the queue is dropped as a publish is, and a
        /// malformed request whose client gave up is not counted (its
        /// refusal cannot be written). The figures are read from the
        /// posts, which come after the count. Kills: check removed;
        /// a dropped request counted.
        #[test]
        fn r3_a_withdraw_and_a_refusal_whose_clients_gave_up() {
            let dir = dir();
            let path = dir.path().join(SOCKET);
            let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (release, gate) = mpsc::channel();
            let (posted, posts) = mpsc::channel();
            let channel = Channel::open_posting(
                &path,
                holding(&asked, gate),
                Box::new(move |figures| {
                    let _ = posted.send(figures);
                }),
            )
            .expect("open");
            let first = {
                let path = path.clone();
                std::thread::spawn(move || {
                    ask(&path, &Request::Progress("hold".into()), &[], IO_LIMIT)
                })
            };
            assert_eq!(asked_at_least(&asked, 1), vec!["hold".to_string()]);
            for sent in [
                format!("{HELLO}\nwithdraw name=gone\n"),
                format!("{HELLO}\nnonsense\n"),
            ] {
                let mut gone = UnixStream::connect(&path).expect("connect");
                gone.write_all(sent.as_bytes()).expect("write");
                drop(gone);
            }
            release.send(()).expect("release");
            assert_eq!(first.join().expect("first").ok(), Some(Reply::Pending));
            let wait = Duration::from_secs(10);
            assert_eq!(
                posts.recv_timeout(wait).expect("a post").answered,
                1,
                "the held request"
            );
            let last = ask(&path, &Request::Progress("waits".into()), &[], IO_LIMIT);
            assert_eq!(last.ok(), Some(Reply::Pending));
            let post = posts.recv_timeout(wait).expect("a post");
            assert_eq!(
                post,
                ChannelFigures {
                    answered: 2,
                    peer: 0,
                    malformed: 0
                }
            );
            assert_eq!(
                asked_at_least(&asked, 2),
                vec!["hold".to_string(), "waits".to_string()]
            );
            assert!(channel.close());
        }

        /// The documented boundary (F1 (b)): a client that closes after
        /// its request was read and looked at, while the handler works,
        /// still has its request handled. This pins the residual the
        /// module's docs state; it is not a wanted behaviour.
        #[test]
        fn r4_a_client_that_gives_up_while_handled_is_still_handled() {
            let dir = dir();
            let path = dir.path().join(SOCKET);
            let (entered_tx, entered) = mpsc::channel();
            let (go_tx, go) = mpsc::channel::<()>();
            let go = std::sync::Mutex::new(go);
            let entered_tx = std::sync::Mutex::new(entered_tx);
            let handled = Arc::new(AtomicU64::new(0));
            let counted = Arc::clone(&handled);
            let channel = Channel::open(
                &path,
                Box::new(move |_: &Request, _| {
                    if let Ok(tx) = entered_tx.lock() {
                        let _ = tx.send(());
                    }
                    if let Ok(go) = go.lock() {
                        let _ = go.recv_timeout(Duration::from_secs(10));
                    }
                    counted.fetch_add(1, Ordering::SeqCst);
                    Reply::Accepted
                }),
            )
            .expect("open");
            let mut client = UnixStream::connect(&path).expect("connect");
            client.write_all(&whole_publish()).expect("write");
            entered
                .recv_timeout(Duration::from_secs(10))
                .expect("the handler is asked");
            drop(client);
            go_tx.send(()).expect("go");
            let until = std::time::Instant::now() + Duration::from_secs(10);
            while handled.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            assert_eq!(handled.load(Ordering::SeqCst), 1);
            assert!(channel.close());
        }

        /// The PR's `a_publish_whose_client_gave_up_in_the_queue_is_dropped`
        /// with the count read from the channel's posts, which come after
        /// it, and not from `figures()` right after the last client's
        /// answer, which the count follows (T1 of the review): two posts
        /// and no third. The fix suggested for T1.
        #[test]
        fn r5_the_queued_publish_test_with_the_posts_awaited() {
            let dir = dir();
            let path = dir.path().join(SOCKET);
            let asked = Arc::new(std::sync::Mutex::new(Vec::new()));
            let (release, gate) = mpsc::channel();
            let (posted, posts) = mpsc::channel();
            let channel = Channel::open_posting(
                &path,
                holding(&asked, gate),
                Box::new(move |figures| {
                    let _ = posted.send(figures);
                }),
            )
            .expect("open");
            let first = {
                let path = path.clone();
                std::thread::spawn(move || {
                    ask(&path, &Request::Progress("hold".into()), &[], IO_LIMIT)
                })
            };
            assert_eq!(asked_at_least(&asked, 1), vec!["hold".to_string()]);
            let mut gone = UnixStream::connect(&path).expect("connect");
            gone.write_all(&whole_publish()).expect("write");
            drop(gone);
            let waiting = {
                let path = path.clone();
                std::thread::spawn(move || {
                    ask(&path, &Request::Progress("waits".into()), &[], IO_LIMIT)
                })
            };
            std::thread::sleep(Duration::from_millis(200));
            release.send(()).expect("release");
            assert_eq!(first.join().expect("first").ok(), Some(Reply::Pending));
            assert_eq!(waiting.join().expect("waiting").ok(), Some(Reply::Pending));
            let wait = Duration::from_secs(10);
            assert_eq!(posts.recv_timeout(wait).expect("a post").answered, 1);
            assert_eq!(posts.recv_timeout(wait).expect("a post").answered, 2);
            assert!(posts.recv_timeout(Duration::from_millis(300)).is_err());
            assert_eq!(
                asked_at_least(&asked, 2),
                vec!["hold".to_string(), "waits".to_string()]
            );
            assert!(channel.close());
        }
    }

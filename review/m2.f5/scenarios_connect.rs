// Appended to bins/skyzfsd/src/control/tests.rs for a run (not part of the pull request).

// ---- review scenarios for #131's connect (not part of the pull request) ----
#[cfg(target_os = "linux")]
mod review_connect {
    use std::time::Instant;

    use super::*;
    use rustix::io::Errno;
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};

    fn socket(flags: SocketFlags) -> rustix::fd::OwnedFd {
        rustix::net::socket_with(AddressFamily::UNIX, SocketType::STREAM, flags, None)
            .expect("a socket")
    }

    /// A listener at `path` that never accepts, its backlog full.
    fn full(path: &std::path::Path) -> (rustix::fd::OwnedFd, Vec<rustix::fd::OwnedFd>) {
        let listener = socket(SocketFlags::CLOEXEC);
        let address = SocketAddrUnix::new(path).expect("an address");
        rustix::net::bind(&listener, &address).expect("bound");
        rustix::net::listen(&listener, 0).expect("listening");
        let mut held = Vec::new();
        loop {
            let peer = socket(SocketFlags::NONBLOCK | SocketFlags::CLOEXEC);
            match rustix::net::connect(&peer, &address) {
                Ok(()) => held.push(peer),
                Err(Errno::AGAIN) => break,
                Err(e) => panic!("{e}"),
            }
        }
        (listener, held)
    }

    fn cpu() -> Duration {
        let t = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
        Duration::new(t.tv_sec as u64, t.tv_nsec as u32)
    }

    /// C1. Room is made in the backlog after 400 ms: the connect is made
    /// within one pause of it.
    #[test]
    fn review_room_made_in_the_backlog_is_taken() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("c1.sock");
        let (listener, held) = full(&path);
        let began = Instant::now();
        let maker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let taken = rustix::net::accept(&listener).expect("accepted");
            (listener, taken)
        });
        let made = super::super::connect(&path, Duration::from_secs(5));
        let took = began.elapsed();
        println!("## C1: room made after 400 ms; connected {:?} after {took:?}", made.is_ok());
        assert!(made.is_ok());
        assert!(took < Duration::from_millis(400 + 120), "{took:?}");
        let _ = maker.join();
        drop(held);
    }

    /// C2, C3, C7. A socket nobody listens on, no file, a plain file: at
    /// once, not at the limit.
    #[test]
    fn review_no_daemon_is_said_at_once() {
        let dir = tempfile::tempdir().expect("a directory");
        let stale = dir.path().join("stale.sock");
        {
            let l = socket(SocketFlags::CLOEXEC);
            rustix::net::bind(&l, &SocketAddrUnix::new(&stale).expect("an address")).expect("bound");
            // Bound, never listening, closed: the file is left.
        }
        let plain = dir.path().join("plain");
        std::fs::write(&plain, b"x").expect("a file");
        let missing = dir.path().join("missing.sock");
        for (what, path) in [("stale socket", &stale), ("plain file", &plain), ("no file", &missing)] {
            let began = Instant::now();
            let e = super::super::connect(path, Duration::from_secs(5)).expect_err("no daemon");
            let took = began.elapsed();
            println!("## C2: {what}: {:?} ({e}) after {took:?}", e.kind());
            assert!(took < Duration::from_millis(50), "{what}: {took:?}");
        }
    }

    /// C4. The daemon goes while the client waits at a full backlog: the
    /// client is told within a pause, not at its limit.
    #[test]
    fn review_a_daemon_gone_during_the_wait_is_said_within_a_pause() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("c4.sock");
        let (listener, held) = full(&path);
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(listener);
            held
        });
        let began = Instant::now();
        let e = super::super::connect(&path, Duration::from_secs(5)).expect_err("gone");
        let took = began.elapsed();
        println!("## C4: listener closed at 300 ms: {:?} ({e}) after {took:?}", e.kind());
        assert!(took < Duration::from_millis(300 + 120), "{took:?}");
        let _ = closer.join();
    }

    /// C5, C6. A full backlog for 2 s: the ask ends at its limit, and the
    /// thread spends little CPU on the tries.
    #[test]
    fn review_a_full_backlog_for_two_seconds() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("c5.sock");
        let (_listener, _held) = full(&path);
        let limit = Duration::from_secs(2);
        let (began, cpu0) = (Instant::now(), cpu());
        let e = ask(&path, &Request::Cuts, &[], limit).expect_err("no connection");
        let (took, spent) = (began.elapsed(), cpu() - cpu0);
        println!("## C5: limit 2 s: {:?} after {took:?}, {spent:?} of CPU", e.kind());
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(took >= limit && took < limit + Duration::from_millis(100), "{took:?}");
        assert!(spent < Duration::from_millis(100), "{spent:?}");
    }
}

//! Review of #147, scratch tests of the client's one deadline (S4).
//! Planted as `skyzfs-control/src/scratch147.rs` with `#[cfg(test)] mod
//! scratch147;` in `lib.rs`; run:
//! `cargo test -p skyzfs-control --lib scratch147 -- --test-threads=1 --nocapture`.

use std::io::{self, BufRead, Read, Write};
use std::os::unix::net::UnixListener;
use std::thread::{sleep, spawn};
use std::time::{Duration, Instant};

use crate::{ask, Request, HELLO};

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().expect("a directory")
}

/// K1: a daemon that sends its greeting and part of the answer and then
/// holds the connection (slow-loris on a partial line): the ask ends at
/// its limit, `TimedOut`.
#[test]
fn k1_a_partial_line_held_ends_at_the_limit() {
    let dir = dir();
    let path = dir.path().join("c");
    let listener = UnixListener::bind(&path).expect("bound");
    let daemon = spawn(move || {
        let (stream, _) = listener.accept().expect("accepted");
        let mut reader = io::BufReader::new(stream.try_clone().expect("clone"));
        let mut line = String::new();
        let _ = reader.read_line(&mut line);
        let _ = reader.read_line(&mut line);
        let mut stream = stream;
        let _ = stream.write_all(format!("{HELLO}\nacc").as_bytes());
        sleep(Duration::from_secs(3));
    });
    let limit = Duration::from_millis(400);
    let began = Instant::now();
    let e = ask(&path, &Request::Cuts, &[], limit).expect_err("held");
    let took = began.elapsed();
    println!("k1: {:?} after {took:?}: {e}", e.kind());
    assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    assert!(took < limit + Duration::from_millis(200), "{took:?}");
    drop(daemon.join());
}

/// K2: a daemon that accepts and never reads, with a manifest larger
/// than the socket's buffer: the write ends at the limit.
#[test]
fn k2_a_daemon_that_never_reads_a_large_manifest() {
    let dir = dir();
    let path = dir.path().join("c");
    let listener = UnixListener::bind(&path).expect("bound");
    let daemon = spawn(move || {
        let (_stream, _) = listener.accept().expect("accepted");
        sleep(Duration::from_secs(3));
    });
    let manifest = vec![b'm'; 1 << 20];
    let request = Request::Publish(crate::Publish {
        name: "t".into(),
        attachment: [1; 16],
        state: [0; 16],
        after: 0,
        group: 1,
        manifest: manifest.len(),
        expiry: None,
    });
    let limit = Duration::from_millis(400);
    let began = Instant::now();
    let e = ask(&path, &request, &manifest, limit).expect_err("never read");
    let took = began.elapsed();
    println!("k2: {:?} after {took:?}: {e}", e.kind());
    assert!(took < limit + Duration::from_millis(200), "{took:?}");
    drop(daemon.join());
}

/// K3: a reply line over the bound, with no newline: refused at the
/// bound, not read on.
#[test]
fn k3_a_reply_over_the_line_bound() {
    let dir = dir();
    let path = dir.path().join("c");
    let listener = UnixListener::bind(&path).expect("bound");
    let daemon = spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepted");
        let _ = stream.write_all(format!("{HELLO}\n").as_bytes());
        let big = vec![b'a'; 1 << 20];
        let _ = stream.write_all(&big);
    });
    let e = ask(&path, &Request::Cuts, &[], Duration::from_secs(2)).expect_err("too long");
    println!("k3: {:?}: {e}", e.kind());
    assert_eq!(e.kind(), io::ErrorKind::InvalidData);
    drop(daemon.join());
}

/// K4: the socket's path is followed through a symbolic link, and the
/// peer is not asked who it is: whoever can place a socket (or a link)
/// at the path answers the agent. Recorded, not judged.
#[test]
fn k4_a_link_at_the_path_is_followed() {
    let dir = dir();
    let real = dir.path().join("elsewhere");
    let path = dir.path().join("control");
    let listener = UnixListener::bind(&real).expect("bound");
    std::os::unix::fs::symlink(&real, &path).expect("linked");
    let daemon = spawn(move || {
        let (mut stream, _) = listener.accept().expect("accepted");
        let mut buf = [0u8; 256];
        let _ = stream.read(&mut buf);
        let _ = stream.write_all(format!("{HELLO}\nwithdrawn\n").as_bytes());
    });
    let reply = ask(&path, &Request::Withdraw("t".into()), &[], Duration::from_secs(2));
    println!("k4: through a link: {reply:?}");
    assert!(reply.is_ok());
    drop(daemon.join());
}

/// K5: a full backlog for most of the limit, then a daemon that answers
/// a byte at a time: the whole ask still ends at about one limit, not
/// two (the connect's wait counts against the one deadline).
#[test]
fn k5_a_full_backlog_then_a_slow_answer_is_one_limit() {
    use rustix::net::{AddressFamily, SocketAddrUnix, SocketFlags, SocketType};
    let dir = dir();
    let path = dir.path().join("c");
    let socket = rustix::net::socket_with(
        AddressFamily::UNIX,
        SocketType::STREAM,
        SocketFlags::CLOEXEC,
        None,
    )
    .expect("a socket");
    let address = SocketAddrUnix::new(&path).expect("an address");
    rustix::net::bind(&socket, &address).expect("bound");
    rustix::net::listen(&socket, 1).expect("listening");
    let listener = UnixListener::from(socket);
    let mut waiting = Vec::new();
    for _ in 0..64 {
        let s = rustix::net::socket_with(
            AddressFamily::UNIX,
            SocketType::STREAM,
            SocketFlags::NONBLOCK | SocketFlags::CLOEXEC,
            None,
        )
        .expect("a socket");
        match rustix::net::connect(&s, &address) {
            Ok(()) => waiting.push(s),
            Err(rustix::io::Errno::AGAIN) => break,
            Err(e) => panic!("{e}"),
        }
    }
    let limit = Duration::from_millis(1000);
    let n = waiting.len();
    let daemon = spawn(move || {
        sleep(limit * 8 / 10);
        for _ in 0..n {
            let (s, _) = listener.accept().expect("accepted");
            drop(s);
        }
        drop(waiting);
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        for byte in format!("{HELLO}\nwithdrawn\n").bytes() {
            sleep(Duration::from_millis(100));
            if stream.write_all(&[byte]).is_err() {
                break;
            }
        }
    });
    let began = Instant::now();
    let reply = ask(&path, &Request::Withdraw("t".into()), &[], limit);
    let took = began.elapsed();
    println!("k5: {reply:?} after {took:?} (backlog filled with {n})");
    assert!(reply.is_err());
    assert!(took < limit + Duration::from_millis(300), "{took:?}");
    drop(daemon.join());
}

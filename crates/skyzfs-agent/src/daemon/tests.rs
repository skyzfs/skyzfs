use std::path::PathBuf;
use std::time::Duration;

use super::*;
use crate::host::fake::Fake;

fn daemon(node: Option<Node>) -> DaemonSocket<Fake> {
    let mut fake = Fake::default();
    if let Some(node) = node {
        fake.nodes.insert(PathBuf::from("/dev/ublkb3"), node);
    }
    DaemonSocket::new(
        PathBuf::from("/nowhere/control"),
        PathBuf::from("/dev/ublkb3"),
        Duration::from_millis(100),
        fake,
    )
}

/// The device number is the block device's, read at each call; a path
/// that is no block device, or none, is an error. Mutant: any node's
/// number taken.
#[test]
fn the_device_is_the_block_devices_number() {
    assert_eq!(
        daemon(Some(Node::Block(0x0103_0003))).device().ok(),
        Some(0x0103_0003)
    );
    for node in [Some(Node::File), Some(Node::Other), None] {
        assert!(daemon(node).device().is_err(), "{node:?}");
    }
}

/// An ask of a socket that is not there fails, and does not wait.
#[test]
fn an_ask_with_no_daemon_fails() {
    let e = daemon(None)
        .ask(&Request::Cuts, &[])
        .expect_err("no daemon");
    assert_eq!(e.kind(), io::ErrorKind::NotFound, "{e}");
}

/// An ask of a daemon that takes the connection and never answers ends
/// at the limit the socket was given (R9 of the review of #147). Mutant:
/// the limit not passed on (`ASK_LIMIT`, 40 s, in its place).
#[test]
fn an_ask_ends_at_the_limit_it_was_given() {
    let dir = tempfile::tempdir().expect("a directory");
    let socket = dir.path().join("control");
    let listener = std::os::unix::net::UnixListener::bind(&socket).expect("bound");
    let (stop, stopped) = std::sync::mpsc::channel::<()>();
    let silent = std::thread::spawn(move || {
        let taken = listener.accept();
        let _ = stopped.recv_timeout(Duration::from_secs(10));
        drop(taken);
    });
    let limit = Duration::from_millis(100);
    let mut daemon =
        DaemonSocket::new(socket, PathBuf::from("/dev/ublkb3"), limit, Fake::default());
    let began = std::time::Instant::now();
    let e = daemon.ask(&Request::Cuts, &[]).expect_err("no answer");
    let took = began.elapsed();
    assert_eq!(e.kind(), io::ErrorKind::TimedOut, "{e}");
    assert!(
        took >= limit && took < limit + Duration::from_millis(400),
        "{took:?}"
    );
    drop(stop);
    drop(silent.join());
}

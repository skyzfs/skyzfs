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

const BOOT: &str = "00010203-0405-0607-0809-0a0b0c0d0e0f\n";

fn noting(note: Option<Vec<u8>>, boot: Option<&str>) -> DaemonSocket<Fake> {
    let mut fake = Fake::default();
    if let Some(note) = note {
        fake.files.insert(PathBuf::from("/run/v-1-2/failed"), note);
    }
    if let Some(boot) = boot {
        fake.files
            .insert(PathBuf::from(BOOT_ID), boot.as_bytes().to_vec());
    }
    DaemonSocket::new(
        PathBuf::from("/run/v-1-2/control"),
        PathBuf::from("/dev/ublkb3"),
        Duration::from_millis(100),
        fake,
    )
}

fn note(boot: [u8; 16], held: skyzfs_control::failure::Held) -> Vec<u8> {
    use skyzfs_control::failure::{encode_failure, Failure, Reason, StoredFailure};
    encode_failure(&StoredFailure {
        boot,
        failure: Failure {
            held,
            reason: Reason::ImageFailed,
            identity: [9; 16],
        },
        written_ms: 5,
    })
    .to_vec()
}

/// The failure note is read beside the socket, and counts only in the
/// boot it was written in; a file there that is not a whole note is
/// `NotWhole`, as the daemon counts it (M2.7c). Mutants: the note read
/// anywhere else; the boot not compared; a torn note taken for none.
#[test]
fn the_failure_note_is_read_beside_the_socket_in_its_boot() {
    use skyzfs_control::failure::{Failure, Held, Reason};
    let this: [u8; 16] = std::array::from_fn(|i| i as u8);
    let mut daemon = noting(Some(note(this, Held::Yes)), Some(BOOT));
    assert_eq!(
        daemon.failure().expect("read"),
        Some(Noted::Failed(Failure {
            held: Held::Yes,
            reason: Reason::ImageFailed,
            identity: [9; 16],
        }))
    );
    let mut other = noting(Some(note([7; 16], Held::Yes)), Some(BOOT));
    assert_eq!(other.failure().expect("read"), None, "another boot's note");
    let mut none = noting(None, Some(BOOT));
    assert_eq!(none.failure().expect("read"), None, "no note");
    let mut torn = noting(Some(note(this, Held::Yes)[..63].to_vec()), Some(BOOT));
    assert_eq!(torn.failure().expect("read"), Some(Noted::NotWhole));
}

/// A boot id that cannot be read, or is not one, is an error, not "no
/// note": the note there may be this boot's.
#[test]
fn a_note_whose_boot_cannot_be_told_is_an_error() {
    use skyzfs_control::failure::Held;
    let this: [u8; 16] = std::array::from_fn(|i| i as u8);
    assert!(noting(Some(note(this, Held::No)), None).failure().is_err());
    assert!(noting(Some(note(this, Held::No)), Some("not a uuid"))
        .failure()
        .is_err());
}

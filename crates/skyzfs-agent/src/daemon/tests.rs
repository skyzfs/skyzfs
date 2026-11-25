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

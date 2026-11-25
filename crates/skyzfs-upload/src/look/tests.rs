use std::sync::Arc;

use skynbd_refs::{MemoryRefStore, ObjectRefStore, RefOp, RefStore, TxnOutcome};
use skynbd_store::{FaultOp, FaultStore, MemoryStore};
use skynbd_types::{SnapId, VolumeId};
use skyzfs_local::inplace::CommittedCut;

use super::*;
use crate::publish::{snap_ref, snap_row_key, SnapRow, SnapRowError, SNAP_ROW_LEN};

const VOL: VolumeId = VolumeId(ulid::Ulid(0x51));

fn row(commit: u8) -> SnapRow {
    SnapRow {
        cut: CommittedCut {
            state: [9; 16],
            number: 3,
            group: 42,
        },
        commit: [commit; 32],
        snap: SnapId(ulid::Ulid::from(7u128)),
        manifest_len: 19,
        manifest_hash: [5; 32],
    }
}

/// Make `snap/<name>` on `commit`, with `row` beside it where one is
/// given, as one transaction.
async fn make(refs: &impl RefStore, name: &str, commit: u8, row: Option<Vec<u8>>) {
    let mut ops = vec![RefOp::CasRef {
        name: snap_ref(name),
        expected: None,
        new_commit: Some([commit; 32]),
    }];
    if let Some(value) = row {
        ops.push(RefOp::MetaPut {
            key: snap_row_key(name),
            value,
        });
    }
    let outcome = refs.transact(VOL, ops).await.expect("made");
    assert_eq!(outcome, TxnOutcome::Committed);
}

#[tokio::test]
async fn a_snapshot_is_seen_as_its_ref_and_the_row_that_names_its_commit() {
    let refs = MemoryRefStore::new();
    let v2 = {
        let mut bytes = row(1).encode();
        bytes[..2].copy_from_slice(&2u16.to_le_bytes());
        bytes
    };
    make(&refs, "ours", 1, Some(row(1).encode())).await;
    make(&refs, "bare", 1, None).await;
    make(&refs, "v2", 1, Some(v2)).await;
    make(
        &refs,
        "short",
        1,
        Some(row(1).encode()[..SNAP_ROW_LEN - 1].to_vec()),
    )
    .await;
    make(&refs, "other", 2, Some(row(1).encode())).await;
    // A row left behind with no ref (SkyNBD deletes a snapshot's ref and
    // not its row) is not read.
    let outcome = refs
        .transact(
            VOL,
            vec![RefOp::MetaPut {
                key: snap_row_key("gone"),
                value: row(1).encode(),
            }],
        )
        .await
        .expect("row put");
    assert_eq!(outcome, TxnOutcome::Committed);

    let cases = [
        (
            "ours",
            Seen::Snap {
                commit: [1; 32],
                row: row(1),
            },
        ),
        ("never", Seen::Absent),
        ("gone", Seen::Absent),
        (
            "bare",
            Seen::Unreadable {
                commit: [1; 32],
                why: Unread::NoRow,
            },
        ),
        (
            "v2",
            Seen::Unreadable {
                commit: [1; 32],
                why: Unread::Row(SnapRowError::Version(2)),
            },
        ),
        (
            "short",
            Seen::Unreadable {
                commit: [1; 32],
                why: Unread::Row(SnapRowError::Length(SNAP_ROW_LEN - 1)),
            },
        ),
        (
            "other",
            Seen::Unreadable {
                commit: [2; 32],
                why: Unread::OtherCommit([1; 32]),
            },
        ),
    ];
    for (name, wanted) in cases {
        assert_eq!(
            look(&refs, VOL, name).await.expect("looked"),
            wanted,
            "{name}"
        );
    }
    // Of another volume, nothing is seen.
    let other = VolumeId(ulid::Ulid(0x52));
    assert_eq!(
        look(&refs, other, "ours").await.expect("looked"),
        Seen::Absent
    );
}

#[tokio::test]
async fn a_lookup_that_object_storage_fails_is_an_error_and_not_absent() {
    let store = Arc::new(FaultStore::new(MemoryStore::new()));
    let refs = ObjectRefStore::new(Arc::clone(&store));
    make(&refs, "t", 1, Some(row(1).encode())).await;
    assert_eq!(
        look(&refs, VOL, "t").await.expect("looked"),
        Seen::Snap {
            commit: [1; 32],
            row: row(1)
        }
    );
    // The refs object's read fails: no answer, and not `Absent`.
    store.arm(FaultOp::Any, "", 0).await;
    assert!(look(&refs, VOL, "t").await.is_err());
    assert!(look(&refs, VOL, "never").await.is_err());
}

#[test]
fn what_is_seen_follows_what_was_found() {
    let found = |commit: u8, row: Option<Vec<u8>>| Found {
        commit: [commit; 32],
        row,
    };
    assert_eq!(Seen::of(None), Seen::Absent);
    assert_eq!(
        Seen::of(Some(found(1, Some(row(1).encode())))),
        Seen::Snap {
            commit: [1; 32],
            row: row(1)
        }
    );
    assert_eq!(
        Seen::of(Some(found(1, None))),
        Seen::Unreadable {
            commit: [1; 32],
            why: Unread::NoRow
        }
    );
    assert_eq!(
        Seen::of(Some(found(3, Some(row(1).encode())))),
        Seen::Unreadable {
            commit: [3; 32],
            why: Unread::OtherCommit([1; 32])
        }
    );
    assert_eq!(
        Seen::of(Some(found(1, Some(Vec::new())))),
        Seen::Unreadable {
            commit: [1; 32],
            why: Unread::Row(SnapRowError::Length(0))
        }
    );
}

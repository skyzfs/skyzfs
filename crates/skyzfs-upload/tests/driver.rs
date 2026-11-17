//! The upload driver against the in-place store on the simulated disk
//! and the engine on a store in memory: each rule of the check, a cut
//! read while the write path overwrites it, and each way a cut or a
//! commit can fail without a death.

mod common;

use common::*;
use skynbd_store::{FaultOp, FaultWhen};
use skyzfs_local::inplace::CommittedCut;
use skyzfs_upload::{row, Checked, Doing, Mismatch, Rule, Step};

/// The steps of a run that are not reads.
fn told(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter(|step| !matches!(step, Step::Read { .. }))
        .cloned()
        .collect()
}

/// Step `up` until its next step does `doing`. A driver that comes to
/// rest first, or takes more than a hundred steps, fails the test rather
/// than leave it stepping for ever.
async fn step_until(up: &mut Up, doing: Doing) {
    for _ in 0..100 {
        if up.doing() == doing {
            return;
        }
        let step = up.step().await.expect("a step");
        assert_ne!(step, Step::Idle, "at rest before {doing:?}");
    }
    panic!("{doing:?} not reached in a hundred steps");
}

/// The first cut of a state begun with nothing known holds the whole
/// volume and goes up onto the empty commit; the next holds what changed
/// and goes up onto it. Each commit has its cut's group, and the row
/// names the cut.
#[tokio::test]
async fn the_first_cut_goes_up_whole_and_the_next_as_a_delta_onto_it() {
    let mut fx = Fixture::new(1).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.write(&mut store, 11, 0x00);
    fx.group(&mut store, 7);
    let state = store.cut_state_id().expect("a cut state");
    let mut up = fx.driver(store, 8);

    let steps = up.run().await.expect("the upload");
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 1,
                rule: Rule::Whole
            },
            Step::Uploaded {
                number: 1,
                rule: Rule::Whole,
                blocks: BLOCKS
            },
        ]
    );
    assert_eq!(
        steps.len() - 2,
        (BLOCKS / 8) as usize,
        "a whole cut reads every block, 8 a slice"
    );
    let uploaded = fx.assert_uploaded(7, "the first cut").await;
    assert_eq!(
        uploaded.row,
        Some(CommittedCut {
            state,
            number: 1,
            group: 7
        })
    );

    fx.write(up.local(), 12, 0x12);
    fx.write(up.local(), 10, 0x00);
    fx.group(up.local(), 8);
    let steps = up.run().await.expect("the second upload");
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 2,
                rule: Rule::Delta
            },
            Step::Uploaded {
                number: 2,
                rule: Rule::Delta,
                blocks: 2 + 4
            },
        ],
        "two blocks and the four uberblock slots"
    );
    fx.assert_uploaded(8, "the second cut").await;
    assert_eq!(up.local().committed_cut().map(|cut| cut.number), Some(2));
    assert!(up.local().held_cuts().is_empty());
    let figures = up.figures();
    assert_eq!(
        (figures.uploaded, figures.uploaded_whole, figures.blocks),
        (2, 1, BLOCKS + 6)
    );
    assert_eq!(figures.checked, Checked::Matched);
    assert_eq!(figures.last_rule, Some(Rule::Delta));
}

/// Nothing to upload while no cut is held, nor while the only one held
/// still waits for its flush.
#[tokio::test]
async fn no_cut_held_is_nothing_to_upload() {
    let fx = Fixture::new(2).await;
    let mut up = fx.driver(fx.start(), 8);
    assert_eq!(up.step().await, Ok(Step::Idle));
    assert_eq!(up.figures().checked, Checked::Unchecked);
}

/// The M2.4 row's test: a cut read while the write path overwrites its
/// blocks holds the content at its instant. Each overwrite lands after
/// the slice before it was read and before the block's own: the store
/// copied the block before the write, and the slice reads the copy.
/// The copies are synced before the commit.
#[tokio::test]
async fn a_cut_read_while_its_blocks_are_overwritten_goes_up_as_at_its_instant() {
    let mut fx = Fixture::new(3).await;
    let mut store = fx.start();
    for block in 20..30 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 1);
    assert!(matches!(up.step().await, Ok(Step::Began { number: 1, .. })));
    let mut steps = Vec::new();
    let mut overwritten = 0;
    loop {
        let step = up.step().await.expect("a step");
        let Step::Read { .. } = step else {
            steps.push(step);
            break;
        };
        // The block after the next one the driver reads, of those the
        // test wrote, overwritten now.
        if overwritten < 10 {
            fx.write(up.local(), 20 + overwritten, 0xEE);
            overwritten += 1;
        }
    }
    steps.extend(up.run().await.expect("the upload"));
    assert_eq!(
        steps,
        [
            Step::CopiesSynced { number: 1 },
            Step::Uploaded {
                number: 1,
                rule: Rule::Whole,
                blocks: BLOCKS
            }
        ]
    );
    fx.assert_uploaded(7, "overwritten while read").await;
    assert_ne!(fx.now, fx.at[&7], "the volume moved on");
}

/// A cut read from no copy needs no sync of the copies before its
/// commit.
#[tokio::test]
async fn a_cut_read_from_no_copy_is_committed_without_a_sync() {
    let mut fx = Fixture::new(4).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    let steps = up.run().await.expect("the upload");
    assert!(!steps.iter().any(|s| matches!(s, Step::CopiesSynced { .. })));
    fx.assert_uploaded(7, "no copy").await;
}

/// A sync of the copies that fails abandons the cut: nothing of it is
/// committed, and the cut that took its map goes up instead, whole as
/// the cut it took the map of.
#[tokio::test]
async fn a_failed_sync_of_the_copies_loses_the_cut_and_its_heir_goes_up() {
    let mut fx = Fixture::new(5).await;
    let mut store = fx.start();
    fx.write(&mut store, 20, 0x20);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    assert!(matches!(up.step().await, Ok(Step::Began { number: 1, .. })));
    fx.write(up.local(), 20, 0x21);
    fx.group(up.local(), 8);
    step_until(&mut up, Doing::Syncing).await;
    fx.disk.fail_sync_of("copies.img");
    let lost = up.step().await.expect("the sync step");
    assert!(matches!(lost, Step::Lost { number: 1, .. }), "{lost:?}");
    let empty = fx.uploaded().await;
    assert_eq!(empty.group, None, "nothing of cut 1 was committed");
    assert_eq!(empty.row, None);

    let steps = up.run().await.expect("the heir's upload");
    assert!(
        told(&steps).contains(&Step::Uploaded {
            number: 2,
            rule: Rule::Whole,
            blocks: BLOCKS
        }),
        "{:?}",
        told(&steps)
    );
    fx.assert_uploaded(8, "the heir").await;
    assert_eq!(up.figures().lost, 1);
}

/// A cut abandoned at the bound between two slices is refused at the
/// next: its writes are dropped, and its heir goes up.
#[tokio::test]
async fn a_cut_abandoned_at_the_bound_while_it_is_read_is_lost() {
    let mut fx = Fixture::new(6).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 8);
    assert!(matches!(up.step().await, Ok(Step::Began { number: 1, .. })));
    assert!(matches!(up.step().await, Ok(Step::Read { .. })));
    // More blocks of the whole cut overwritten than the bound has room
    // for copies of.
    for block in 0..(BOUND / BLOCK) + 8 {
        fx.write(up.local(), 100 + block, 0x55);
    }
    fx.group(up.local(), 8);
    let lost = up.step().await.expect("the next slice");
    assert!(matches!(lost, Step::Lost { number: 1, .. }), "{lost:?}");
    let steps = up.run().await.expect("the heir's upload");
    assert!(
        told(&steps).contains(&Step::Uploaded {
            number: 2,
            rule: Rule::Whole,
            blocks: BLOCKS
        }),
        "{:?}",
        told(&steps)
    );
    fx.assert_uploaded(8, "the heir").await;
}

/// A commit that failed before the swap is made again once the row and
/// head are read again and found as they were; one whose swap landed
/// and whose reply was lost is found by the row, and is not made again.
#[tokio::test]
async fn a_failed_commit_is_made_again_or_found_landed_by_the_row() {
    for when in [FaultWhen::Before, FaultWhen::After] {
        let case = format!("{when:?}");
        let mut fx = Fixture::new(7).await;
        let mut store = fx.start();
        fx.group(&mut store, 7);
        let mut up = fx.driver(store, 64);
        step_until(&mut up, Doing::Committing).await;
        // The refs object's PUT is the head swap.
        fx.store.arm_when(FaultOp::Put, "refs", 0, when).await;
        let failed = up.step().await;
        assert!(failed.is_err(), "{case}: {failed:?}");
        fx.store.disarm().await;
        let landed = fx.uploaded().await.group == Some(7);
        assert_eq!(landed, when == FaultWhen::After, "{case}");
        let next = up.step().await.expect("the commit again");
        assert_eq!(
            next,
            Step::Uploaded {
                number: 1,
                rule: Rule::Whole,
                blocks: BLOCKS
            },
            "{case}"
        );
        fx.assert_uploaded(7, &case).await;
        assert_eq!(up.figures().errors, 1, "{case}");
        assert_eq!(up.step().await, Ok(Step::Idle), "{case}");
    }
}

/// The row moved between the check and the commit: the commit fails on
/// the row, is given up when the row is read again, and the next check
/// is a mismatch. The row another writer put is not overwritten.
#[tokio::test]
async fn a_row_moved_before_the_commit_gives_the_commit_up() {
    let mut fx = Fixture::new(8).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    step_until(&mut up, Doing::Committing).await;
    let theirs = row::encode(&CommittedCut {
        state: *b"another writer's",
        number: 9,
        group: 0,
    });
    fx.put_row(theirs.clone()).await;
    assert!(up.step().await.is_err(), "the commit fails on the row");
    assert_eq!(up.step().await, Ok(Step::GaveUp { number: 1 }));
    let mismatch = up.step().await.expect("the check");
    assert!(
        matches!(
            mismatch,
            Step::Mismatch {
                why: Mismatch::HeadMoved,
                ..
            }
        ),
        "a whole cut, and head's commit has no group where the row says 0: {mismatch:?}"
    );
    let inner = fx.store.inner().clone();
    let refs = skynbd_refs::ObjectRefStore::new(inner);
    use skynbd_refs::RefStore;
    assert_eq!(
        refs.meta_get(fx.vol, skyzfs_upload::ROW_KEY).await.unwrap(),
        Some(theirs)
    );
    assert_eq!(fx.uploaded().await.group, None, "head never moved");
}

/// A row that names another cut than the base: every held cut is
/// abandoned, and the next cut goes up whole onto head, which is the
/// row's.
#[tokio::test]
async fn a_row_naming_another_cut_abandons_every_held_cut_and_the_next_goes_up_whole() {
    let mut fx = Fixture::new(9).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("the first upload");
    fx.write(up.local(), 30, 0x30);
    fx.group(up.local(), 8);
    fx.write(up.local(), 31, 0x31);
    fx.group(up.local(), 9);
    // Another cut, of head's group.
    fx.put_row(row::encode(&CommittedCut {
        state: *b"another state...",
        number: 1,
        group: 7,
    }))
    .await;
    let mismatch = up.step().await.expect("the check");
    assert_eq!(
        mismatch,
        Step::Mismatch {
            why: Mismatch::NotTheBase,
            abandoned: vec![2, 3]
        }
    );
    assert_eq!(
        up.figures().checked,
        Checked::Mismatched(Mismatch::NotTheBase)
    );
    assert_eq!(up.step().await, Ok(Step::Idle), "nothing held");
    fx.write(up.local(), 32, 0x32);
    fx.group(up.local(), 10);
    let steps = up.run().await.expect("the whole cut");
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 4,
                rule: Rule::Whole
            },
            Step::Uploaded {
                number: 4,
                rule: Rule::Whole,
                blocks: BLOCKS
            }
        ]
    );
    fx.assert_uploaded(10, "whole after a mismatch").await;
    let figures = up.figures();
    assert_eq!((figures.mismatches, figures.abandoned_at_mismatch), (1, 2));
    // The cut after it is a delta onto it again.
    fx.write(up.local(), 33, 0x33);
    fx.group(up.local(), 11);
    let steps = up.run().await.expect("a delta");
    assert!(told(&steps).contains(&Step::Began {
        number: 5,
        rule: Rule::Delta
    }));
    fx.assert_uploaded(11, "a delta after the whole cut").await;
}

/// Q5: head moved by another writer is never moved by the uploader. The
/// check finds a mismatch, and so does every check after it while head
/// is not the row's, whole cuts included.
#[tokio::test]
async fn a_head_moved_by_another_writer_stops_the_upload() {
    let mut fx = Fixture::new(10).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("the first upload");
    fx.move_head().await;
    let moved = fx.uploaded().await;
    for txg in [8, 9, 10] {
        fx.write(up.local(), 40, txg as u8);
        fx.group(up.local(), txg);
        let steps = up.run().await.expect("the check");
        assert_eq!(steps.len(), 1, "{steps:?}");
        assert!(matches!(
            steps[0],
            Step::Mismatch {
                why: Mismatch::HeadMoved,
                ..
            }
        ));
        let now = fx.uploaded().await;
        assert_eq!(now.content, moved.content, "the uploader moved head");
        assert_eq!(now.row, moved.row);
    }
    assert_eq!(up.figures().mismatches, 3);
}

/// A row this uploader cannot read is a mismatch, and never overwritten.
#[tokio::test]
async fn a_row_of_another_version_is_never_overwritten() {
    let mut fx = Fixture::new(11).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let newer = vec![2, 0, 1, 2, 3];
    fx.put_row(newer.clone()).await;
    let mut up = fx.driver(store, 64);
    for txg in [8, 9] {
        let steps = up.run().await.expect("the check");
        assert_eq!(
            steps,
            [Step::Mismatch {
                why: Mismatch::Unreadable,
                abandoned: vec![if txg == 8 { 1 } else { 2 }]
            }]
        );
        fx.group(up.local(), txg);
    }
    let inner = fx.store.inner().clone();
    let refs = skynbd_refs::ObjectRefStore::new(inner);
    use skynbd_refs::RefStore;
    assert_eq!(
        refs.meta_get(fx.vol, skyzfs_upload::ROW_KEY).await.unwrap(),
        Some(newer)
    );
}

/// No row while a cut of part of the volume is uploaded (the row was
/// deleted): a mismatch, and the whole cut after it finds head's commit
/// with a group and no row, a mismatch too.
#[tokio::test]
async fn no_row_under_a_cut_of_part_of_the_volume_is_a_mismatch() {
    let mut fx = Fixture::new(12).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("the first upload");
    let refs = skynbd_refs::ObjectRefStore::new(fx.store.inner().clone());
    use skynbd_refs::RefStore;
    refs.transact(
        fx.vol,
        vec![skynbd_refs::RefOp::MetaDelete {
            key: skyzfs_upload::ROW_KEY.into(),
        }],
    )
    .await
    .unwrap();
    fx.group(up.local(), 8);
    assert_eq!(
        up.run().await.expect("the check"),
        [Step::Mismatch {
            why: Mismatch::NoRow,
            abandoned: vec![2]
        }]
    );
    fx.group(up.local(), 9);
    assert_eq!(
        up.run().await.expect("the whole cut's check"),
        [Step::Mismatch {
            why: Mismatch::HeadMoved,
            abandoned: vec![3]
        }]
    );
}

/// An all-zero block goes up as a zero extent, and reads as zeros.
#[tokio::test]
async fn zero_blocks_go_up_as_zeros() {
    let mut fx = Fixture::new(13).await;
    let mut store = fx.start();
    fx.write(&mut store, 50, 0x50);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("the first upload");
    fx.write(up.local(), 50, 0x00);
    fx.group(up.local(), 8);
    up.run().await.expect("the delta");
    let uploaded = fx.assert_uploaded(8, "zeros").await;
    let at = (50 * BLOCK) as usize;
    assert!(uploaded.content[at..at + BLOCK as usize]
        .iter()
        .all(|&b| b == 0));
    // The whole cut's 250 blocks of zeros and the delta's one took no
    // segment's bytes: the segments hold the data block, the uberblocks'
    // slots, and nothing else.
    assert!(
        fx.segment_bytes().await <= 12 * BLOCK,
        "{} bytes of segments",
        fx.segment_bytes().await
    );
}

/// A cut abandoned at the bound after its last slice is not committed:
/// the check before the commit finds it gone, its writes are dropped,
/// and its heir goes up.
#[tokio::test]
async fn a_cut_abandoned_after_its_last_slice_is_not_committed() {
    let mut fx = Fixture::new(14).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    step_until(&mut up, Doing::Committing).await;
    for block in 0..(BOUND / BLOCK) + 8 {
        fx.write(up.local(), 100 + block, 0x66);
    }
    fx.group(up.local(), 8);
    let lost = up.step().await.expect("the commit step");
    assert!(matches!(lost, Step::Lost { number: 1, .. }), "{lost:?}");
    assert_eq!(fx.uploaded().await.group, None, "cut 1 was committed");
    let steps = up.run().await.expect("the heir");
    assert!(told(&steps).contains(&Step::Uploaded {
        number: 2,
        rule: Rule::Whole,
        blocks: BLOCKS
    }));
    fx.assert_uploaded(8, "the heir").await;
}

/// A cut lost before its commit leaves nothing in the engine, so the next
/// check opens the engine at head as head is now: a head that another
/// writer moved meanwhile is found at once, and not once the heir has
/// been read against the head that was.
#[tokio::test]
async fn a_lost_cut_leaves_the_next_check_to_find_head_as_it_is() {
    let mut fx = Fixture::new(15).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("the first upload");
    // Cut 2 holds more blocks than the bound, so that overwriting them
    // all abandons it.
    let over = BOUND / BLOCK + 8;
    for block in 0..over {
        fx.write(up.local(), 100 + block, 0x22);
    }
    fx.group(up.local(), 8);
    step_until(&mut up, Doing::Committing).await;
    for block in 0..over {
        fx.write(up.local(), 100 + block, 0x33);
    }
    fx.group(up.local(), 9);
    let lost = up.step().await.expect("the commit step");
    assert!(matches!(lost, Step::Lost { number: 2, .. }), "{lost:?}");
    fx.move_head().await;
    let next = up.step().await.expect("the heir's check");
    assert!(
        matches!(
            next,
            Step::Mismatch {
                why: Mismatch::HeadMoved,
                ..
            }
        ),
        "{next:?}"
    );
}

/// A PUT that fails while a cut is read fails its step and nothing else:
/// the slice is in the engine, and the steps after it go on from the
/// next slice once the store answers again, with no block read twice.
#[tokio::test]
async fn a_failed_put_goes_on_from_the_next_slice() {
    let mut fx = Fixture::new(15).await;
    let mut store = fx.start();
    for block in 0..BLOCKS {
        fx.write(&mut store, block, (block as u8) | 1);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 16);
    assert!(matches!(up.step().await, Ok(Step::Began { .. })));
    assert!(matches!(up.step().await, Ok(Step::Read { .. })));
    fx.store
        .arm_when(FaultOp::Put, "/seg/", 0, FaultWhen::Before)
        .await;
    assert!(up.step().await.is_err(), "the slice's PUT fails");
    assert!(up.step().await.is_err(), "and the PUT made again first");
    fx.store.disarm().await;
    assert_eq!(up.doing(), Doing::Reading);
    let steps = up.run().await.expect("the upload goes on");
    assert!(
        matches!(steps.first(), Some(Step::Read { .. })),
        "{steps:?}"
    );
    assert_eq!(
        steps.last(),
        Some(&Step::Uploaded {
            number: 1,
            rule: Rule::Whole,
            blocks: BLOCKS
        })
    );
    fx.assert_uploaded(7, "after a failed PUT").await;
    assert_eq!(up.figures().errors, 2);
}

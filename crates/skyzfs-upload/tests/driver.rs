//! The upload driver against the in-place store on the simulated disk
//! and the engine on a store in memory: each rule of the check, a cut
//! read while the write path overwrites it, and each way a cut or a
//! commit can fail without a death.

mod common;

use std::time::Duration;

use common::*;
use skynbd_store::{FaultOp, FaultWhen};
use skyzfs_blk::{BlockTarget, Buf, Op, Request, RequestId};
use skyzfs_local::inplace::{CommittedCut, CutPhase, InPlace};
use skyzfs_local::sim::SimDisk;
use skyzfs_local::store::HOME_NAME;
use skyzfs_upload::{row, Checked, Doing, Driver, Mismatch, Rule, Step};

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

/// The cuts `store` holds, by number, and the phase of each.
fn phases(store: &InPlace<SimDisk>) -> Vec<(u64, CutPhase)> {
    store
        .held_cuts()
        .iter()
        .map(|cut| (cut.number, cut.phase))
        .collect()
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
        uploaded.row.map(|row| row.cut),
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
    let theirs = row::encode(
        &CommittedCut {
            state: *b"another writer's",
            number: 9,
            group: 0,
        },
        &THEIR_COMMIT,
    );
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
        "a whole cut, and the row does not name head's commit: {mismatch:?}"
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
    // Another cut, of head's group, on head's commit.
    fx.put_row_on_head(*b"another state...", 1, 7).await;
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

/// An upgrade from a daemon that wrote the row's version 1, which names
/// no commit: the row names cut 1, and head is cut 1's commit. The row
/// does not stop the uploads, and vouches for no delta: the next cut's
/// check finds head not named by the row, every held cut is abandoned,
/// and the cut after it goes up whole onto head, whose group is the
/// row's, and writes the row's version 2. The cut after that is a delta
/// again (the lead's decision on F1).
#[tokio::test]
async fn after_a_row_of_version_1_the_first_commit_is_whole_and_writes_version_2() {
    let mut fx = Fixture::new(27).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    let cut1 = fx.assert_uploaded(7, "cut 1").await;
    let named = cut1.row.expect("the row");
    fx.put_row(row_v1(&named.cut)).await;
    assert_eq!(
        fx.uploaded().await.row,
        Some(skyzfs_upload::Named {
            cut: named.cut,
            commit: None
        }),
        "a row of version 1 reads as naming no commit"
    );

    fx.write(up.local(), 11, 0x11);
    fx.group(up.local(), 8);
    let steps = up.run().await.expect("cut 2");
    assert_eq!(
        told(&steps),
        [Step::Mismatch {
            why: Mismatch::HeadMoved,
            abandoned: vec![2]
        }],
        "no delta onto a row of version 1"
    );
    assert_eq!(fx.uploaded().await.head, cut1.head, "head did not move");

    fx.write(up.local(), 12, 0x12);
    fx.group(up.local(), 9);
    let steps = up.run().await.expect("cut 3");
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 3,
                rule: Rule::Whole
            },
            Step::Uploaded {
                number: 3,
                rule: Rule::Whole,
                blocks: BLOCKS
            }
        ]
    );
    let cut3 = fx.assert_uploaded(9, "cut 3, whole").await;
    assert_eq!(
        fx.uploaded().await.row.map(|row| row.commit),
        Some(Some(cut3.head)),
        "the row's version 2 again"
    );

    fx.write(up.local(), 13, 0x13);
    fx.group(up.local(), 10);
    let steps = up.run().await.expect("cut 4");
    assert!(
        told(&steps).contains(&Step::Began {
            number: 4,
            rule: Rule::Delta
        }),
        "{steps:?}"
    );
    fx.assert_uploaded(10, "cut 4, a delta").await;
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

/// A commit fails with nothing of it landed (the commit object's PUT),
/// and another writer then moves head, or the row, leaving the other as
/// it was. The engine holds cut 2's writes on a head and a row that are
/// no longer the volume's: the commit is given up, not made again, and
/// the check after it is a mismatch. What the other writer left stays.
/// Made again, the commit conflicts (the engine kept as it was), or,
/// where the engine is opened again at the moved head, carries the group
/// and the row onto the other writer's commit with none of the cut's
/// blocks: a cut in object storage that lacks its writes, and the next
/// delta onto it.
#[tokio::test]
async fn a_failed_commit_on_a_head_or_row_another_writer_moved_is_given_up() {
    let theirs = row::encode(
        &CommittedCut {
            state: *b"another writer's",
            number: 9,
            group: 7,
        },
        &THEIR_COMMIT,
    );
    for (seed, head_moved) in [(23, true), (26, false)] {
        let case = if head_moved {
            "head moved"
        } else {
            "row moved"
        };
        let mut fx = Fixture::new(seed).await;
        let mut store = fx.start();
        fx.write(&mut store, 10, 0x10);
        fx.group(&mut store, 7);
        let mut up = fx.driver(store, 64);
        up.run().await.expect("cut 1");
        fx.write(up.local(), 20, 0x20);
        fx.group(up.local(), 8);
        step_until(&mut up, Doing::Committing).await;
        fx.store
            .arm_when(FaultOp::Put, "/commit/", 0, FaultWhen::Before)
            .await;
        assert!(up.step().await.is_err(), "{case}: the commit fails");
        fx.store.disarm().await;
        assert_eq!(
            fx.uploaded().await.group,
            Some(7),
            "{case}: nothing of cut 2 landed"
        );
        if head_moved {
            fx.move_head().await;
        } else {
            fx.put_row(theirs.clone()).await;
        }
        let moved = fx.uploaded().await;
        assert_eq!(up.step().await, Ok(Step::GaveUp { number: 2 }), "{case}");
        let steps = up.run().await.expect("the check after");
        let why = if head_moved {
            Mismatch::HeadMoved
        } else {
            Mismatch::NotTheBase
        };
        assert_eq!(
            told(&steps),
            [Step::Mismatch {
                why,
                abandoned: vec![2]
            }],
            "{case}"
        );
        let now = fx.uploaded().await;
        assert!(
            now.content == moved.content,
            "{case}: the uploader moved head"
        );
        assert_eq!(now.group, moved.group, "{case}: head's group");
        assert_eq!(now.row, moved.row, "{case}: the row");
        assert_eq!(up.figures().gave_up, 1, "{case}");
    }
}

/// A row this uploader cannot read is a mismatch, and never overwritten.
#[tokio::test]
async fn a_row_of_another_version_is_never_overwritten() {
    let mut fx = Fixture::new(11).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let newer = vec![3, 0, 1, 2, 3];
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

/// The bound takes a cut after its commit has landed and before it is
/// let go: the driver names it in the intent map as the committed cut,
/// and the cut that took its map goes up as a delta onto it, with no
/// mismatch and no whole upload.
#[tokio::test]
async fn a_cut_the_bound_takes_after_its_commit_is_named_and_its_heir_goes_up_as_a_delta() {
    let mut fx = Fixture::new(16).await;
    let mut up = committed_then_taken(&mut fx).await;
    let state = up.local().cut_state_id().expect("a cut state");
    assert_eq!(
        up.step().await,
        Ok(Step::UploadedNotHeld {
            number: 2,
            named: true
        })
    );
    assert_eq!(
        up.local().committed_cut(),
        Some(CommittedCut {
            state,
            number: 2,
            group: 8
        })
    );
    fx.assert_uploaded(8, "cut 2").await;

    let steps = up.run().await.expect("cut 3");
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 3,
                rule: Rule::Delta
            },
            Step::Uploaded {
                number: 3,
                rule: Rule::Delta,
                blocks: OVER + 4
            },
        ],
        "the blocks from 100, which hold the slots of groups 8 and 9 in two \
         labels, and those slots in the other two"
    );
    fx.assert_uploaded(9, "cut 3").await;
    let figures = up.figures();
    assert_eq!(
        (
            figures.uploaded,
            figures.uploaded_whole,
            figures.not_held,
            figures.named,
            figures.mismatches
        ),
        (3, 1, 1, 1, 0)
    );
    assert_eq!(up.local().committed_cut().map(|cut| cut.number), Some(3));
}

/// The daemon's store answers the driver between turns of serving the
/// pool, so the bound can take a cut between the check that found it
/// committed and its release: here after a restart that followed a
/// commit whose reply was lost. The cut is named as committed all the
/// same, and the next cut goes up as a delta onto it.
#[tokio::test]
async fn a_cut_the_bound_takes_after_the_check_found_it_committed_is_named() {
    let mut fx = Fixture::new(17).await;
    let up = committed_unheard(&mut fx).await;
    fx.died(up);
    let at_release = (0..OVER).map(|block| (100 + block, 0x33)).collect();
    let racing = Racing {
        store: fx.start(),
        at_release,
    };
    let mut up = Driver::with_slices(racing, fx.remote(), 64);
    assert_eq!(
        up.step().await,
        Ok(Step::UploadedNotHeld {
            number: 2,
            named: true
        })
    );
    for block in 0..OVER {
        fx.wrote(100 + block, 0x33);
    }
    assert_eq!(
        up.local()
            .store
            .committed_cut()
            .map(|cut| (cut.number, cut.group)),
        Some((2, 8))
    );
    fx.group(&mut up.local().store, 9);
    let steps = up.run().await.expect("cut 3");
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 3,
                rule: Rule::Delta
            },
            Step::Uploaded {
                number: 3,
                rule: Rule::Delta,
                blocks: OVER + 4
            },
        ]
    );
    fx.assert_uploaded(9, "cut 3").await;
    let figures = up.figures();
    assert_eq!(
        (figures.found_committed, figures.not_held, figures.named),
        (0, 1, 1)
    );
}

/// The store's intent map, as its file is called in the volume's
/// directory (`skyzfs_local`'s `intent::INTENT_NAME`, which is the
/// crate's own).
const INTENT_NAME: &str = "intent.map";

/// The commit another writer's row names in these tests: not head's.
const THEIR_COMMIT: [u8; 32] = [0xab; 32];

/// A release whose write of the intent map fails after a commit that
/// landed (F2 of the review of M2.4b-1): the volume fails, the map may
/// name the cut from now on, and the cut stays held. The store would
/// offer it again, and the check would find it committed again, for
/// ever. The driver says the cut is kept and stops, for good: only the
/// cut being uploaded can be let go, so no later cut can leave from this
/// instance. Once at the driver's own commit, once at a check that found
/// the commit landed before a death. The defects, planted by hand: a
/// kept cut taken as let go, or as gone; the driver that goes on after
/// it; and a stop that holds for one step.
#[tokio::test]
async fn a_cut_kept_at_a_failed_write_of_the_map_stops_the_driver() {
    async fn stopped(up: &mut Up, number: u64, case: &str) {
        assert!(up.local().is_failed(), "{case}");
        assert_eq!(
            phases(up.local()),
            [(number, CutPhase::Uploading)],
            "{case}: kept"
        );
        assert_eq!(up.doing(), Doing::Stopped, "{case}");
        assert_eq!(up.under_way(), None, "{case}");
        for _ in 0..3 {
            assert_eq!(up.step().await, Ok(Step::Idle), "{case}");
        }
        assert_eq!(up.run().await, Ok(Vec::new()), "{case}");
        assert_eq!(up.figures().kept, 1, "{case}");
    }

    let case = "at its commit";
    let mut fx = Fixture::new(18).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    step_until(&mut up, Doing::Committing).await;
    fx.disk.fail_write_of(INTENT_NAME);
    assert_eq!(up.step().await, Ok(Step::Kept { number: 1 }), "{case}");
    fx.assert_uploaded(7, case).await;
    stopped(&mut up, 1, case).await;
    assert_eq!(
        (up.figures().uploaded, up.figures().found_committed),
        (1, 0),
        "{case}"
    );

    let case = "at a check that found it committed";
    let mut fx = Fixture::new(19).await;
    let up = committed_unheard(&mut fx).await;
    fx.died(up);
    let store = fx.start();
    fx.disk.fail_write_of(INTENT_NAME);
    let mut up = fx.driver(store, 64);
    assert_eq!(up.step().await, Ok(Step::Kept { number: 2 }), "{case}");
    stopped(&mut up, 2, case).await;
    assert_eq!(
        (up.figures().uploaded, up.figures().found_committed),
        (0, 0),
        "{case}"
    );
}

/// Design §5.2: a cut is uploaded only once its flush has completed.
/// While the flush that takes cut 1 waits for the image's sync, the cut
/// is held, flushing: the store offers nothing, and the driver rests
/// with no check made. Once the flush has ended, the upload begins.
#[tokio::test]
async fn a_cut_whose_flush_is_under_way_is_not_uploaded() {
    let mut fx = Fixture::new(24).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.labels(&mut store, 7);
    fx.at.insert(7, fx.now.clone());
    fx.disk.hold_syncs_of(HOME_NAME);
    store
        .submit(Request {
            id: RequestId(1 << 41),
            op: Op::Flush,
        })
        .expect("submitted");
    let mut done = Vec::new();
    for _ in 0..1000 {
        store.poll(&mut done);
        if !phases(&store).is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(phases(&store), [(1, CutPhase::Flushing)]);
    assert!(done.is_empty(), "the flush's sync is held");
    let mut up = fx.driver(store, 64);
    assert_eq!(up.step().await, Ok(Step::Idle));
    assert_eq!(up.figures().checked, Checked::Unchecked);
    assert_eq!(fx.uploaded().await.group, None);
    fx.disk.release_syncs();
    while done.is_empty() {
        up.local().wait(&mut done, Duration::from_secs(60));
    }
    assert!(done[0].result.is_ok(), "the flush: {:?}", done[0].result);
    let steps = up.run().await.expect("the upload");
    assert_eq!(
        told(&steps).first(),
        Some(&Step::Began {
            number: 1,
            rule: Rule::Whole
        }),
        "{steps:?}"
    );
    fx.assert_uploaded(7, "once the flush has ended").await;
}

/// Design §5.2: a cut whose flush had completed is still uploaded after
/// the volume fails, by the instance that saw the failure. The volume
/// fails at a write of the image, not at the intent map: cut 2 stays
/// held, goes up as a delta (block 20 and the four uberblocks) and is
/// let go, and the driver goes on to the next check. It stops only at a
/// cut the store kept (above).
#[tokio::test]
async fn a_cut_held_when_the_volume_fails_at_a_write_of_the_image_is_still_uploaded() {
    let mut fx = Fixture::new(25).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    fx.write(up.local(), 20, 0x20);
    fx.group(up.local(), 8);
    fx.disk.fail_write_of(HOME_NAME);
    let store = up.local();
    store
        .submit(Request {
            id: RequestId(1 << 41),
            op: Op::Write {
                offset: 30 * BLOCK,
                data: Buf::filled(1, 0x30),
            },
        })
        .expect("submitted");
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, Duration::from_secs(60));
    }
    assert!(done[0].result.is_err(), "the write fails");
    assert!(store.is_failed(), "and the volume with it");
    assert_eq!(phases(store), [(2, CutPhase::Uploading)]);
    let steps = up.run().await.expect("the upload on the failed volume");
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
                blocks: 5
            }
        ]
    );
    assert_eq!(up.doing(), Doing::Checking);
    assert!(phases(up.local()).is_empty(), "let go");
    assert_eq!(up.figures().kept, 0);
    fx.assert_uploaded(8, "on a failed volume").await;
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

/// Refs that let another writer swap head once, just after the
/// uploader's next read of head returns: the writer commits onto head a
/// block 0 full of 0xCD, of group `group` (`None`: a commit with no
/// group, an operator's write), through refs of its own, and leaves the
/// row. Every other call goes to `refs` as it is.
#[derive(Clone)]
struct Swapping {
    refs: Refs,
    store: Store,
    vol: skynbd_types::VolumeId,
    pending: std::sync::Arc<std::sync::Mutex<Option<Option<u64>>>>,
}

impl Swapping {
    fn new(fx: &Fixture) -> Self {
        Self {
            refs: std::sync::Arc::new(skynbd_refs::ObjectRefStore::new(fx.store.clone())),
            store: fx.store.clone(),
            vol: fx.vol,
            pending: Default::default(),
        }
    }

    /// The next read of head is followed by another writer's commit of
    /// group `group`.
    fn after_the_next_read(&self, group: Option<u64>) {
        *self.pending.lock().expect("pending") = Some(group);
    }

    /// Whether the swap is still to come.
    fn pending(&self) -> bool {
        self.pending.lock().expect("pending").is_some()
    }

    async fn other_writer(&self, group: Option<u64>) {
        let inner = self.store.inner().clone();
        let refs = skynbd_refs::ObjectRefStore::new(inner.clone());
        let mut engine = skynbd_engine::Engine::open(inner, refs, self.vol, cfg())
            .await
            .expect("the other writer's engine");
        engine.write(0, &[0xCD; BLOCK as usize]).expect("a write");
        engine.set_group(group);
        engine
            .commit(skynbd_format::CommitKind::Flush)
            .await
            .expect("the other writer's commit");
    }
}

#[async_trait::async_trait]
impl skynbd_refs::RefStore for Swapping {
    async fn get(
        &self,
        vol: skynbd_types::VolumeId,
        name: &str,
    ) -> skynbd_refs::Result<Option<skynbd_refs::RefValue>> {
        let got = self.refs.get(vol, name).await;
        let group = if name == "head" {
            self.pending.lock().expect("pending").take()
        } else {
            None
        };
        if let Some(group) = group {
            self.other_writer(group).await;
        }
        got
    }
    async fn compare_and_swap(
        &self,
        vol: skynbd_types::VolumeId,
        name: &str,
        expected: Option<skynbd_refs::RefValue>,
        new_commit: Option<skynbd_types::CommitHash>,
    ) -> skynbd_refs::Result<skynbd_refs::CasOutcome> {
        self.refs
            .compare_and_swap(vol, name, expected, new_commit)
            .await
    }
    async fn list(
        &self,
        vol: skynbd_types::VolumeId,
    ) -> skynbd_refs::Result<Vec<(String, skynbd_refs::RefValue)>> {
        self.refs.list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<skynbd_refs::RefEvent> {
        self.refs.watch()
    }
    async fn transact(
        &self,
        vol: skynbd_types::VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.refs.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: skynbd_types::VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.refs.transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(
        &self,
        vol: skynbd_types::VolumeId,
        name: &str,
    ) -> skynbd_refs::Result<Vec<String>> {
        self.refs.pins(vol, name).await
    }
    async fn meta_get(
        &self,
        vol: skynbd_types::VolumeId,
        key: &str,
    ) -> skynbd_refs::Result<Option<Vec<u8>>> {
        self.refs.meta_get(vol, key).await
    }
    async fn meta_list(
        &self,
        vol: skynbd_types::VolumeId,
        prefix: &str,
    ) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
        self.refs.meta_list(vol, prefix).await
    }
    async fn lease_acquire(
        &self,
        vol: skynbd_types::VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<skynbd_refs::LeaseOutcome> {
        self.refs.lease_acquire(vol, holder, ttl_ms).await
    }
    async fn lease_renew(
        &self,
        vol: skynbd_types::VolumeId,
        holder: &str,
        epoch: u64,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<skynbd_refs::RenewOutcome> {
        self.refs.lease_renew(vol, holder, epoch, ttl_ms).await
    }
    async fn lease_release(
        &self,
        vol: skynbd_types::VolumeId,
        holder: &str,
    ) -> skynbd_refs::Result<()> {
        self.refs.lease_release(vol, holder).await
    }
    async fn lease_get(
        &self,
        vol: skynbd_types::VolumeId,
    ) -> skynbd_refs::Result<Option<skynbd_refs::Lease>> {
        self.refs.lease_get(vol).await
    }
}

/// B1 of the review of #134: the check reads head, then the engine,
/// opened again after a restart, reads head a second time, and another
/// writer swaps head from cut 1's commit to a commit of its own between
/// the two reads. The engine sits on the other writer's commit, and its
/// head swap would expect it: the check must judge that commit, not the
/// one it read first, or cut 2 goes up as a delta onto another history
/// and moves head off the other writer's commit. Whatever the other
/// commit's group (none, cut 1's, another), cut 2's check finds head
/// moved, and head and the row stay where the other writer left them.
#[tokio::test]
async fn head_swapped_while_the_engine_is_opened_again_is_judged_as_the_engine_found_it() {
    for (seed, group) in [(90, None), (91, Some(7)), (92, Some(99))] {
        let case = format!("the other writer's group {group:?}");
        let mut fx = Fixture::new(seed).await;
        let mut store = fx.start();
        fx.write(&mut store, 10, 0x10);
        fx.group(&mut store, 7);
        let mut up = fx.driver(store, 64);
        up.run().await.expect("cut 1");
        let cut1 = fx.assert_uploaded(7, &case).await;
        fx.died(up);

        let mut store = fx.start();
        fx.write(&mut store, 11, 0x11);
        fx.group(&mut store, 8);
        let refs = Swapping::new(&fx);
        refs.after_the_next_read(group);
        let remote =
            skyzfs_upload::EngineRemote::new(fx.store.clone(), refs.clone(), fx.vol, cfg());
        let mut up = Driver::with_slices(store, remote, 64);
        let steps = up.run().await.expect("cut 2");
        assert!(!refs.pending(), "{case}: the other writer swapped head");
        assert_eq!(
            told(&steps),
            [Step::Mismatch {
                why: Mismatch::HeadMoved,
                abandoned: vec![2]
            }],
            "{case}: cut 2 is not uploaded onto another writer's commit"
        );
        let moved = fx.uploaded().await;
        assert_ne!(moved.head, cut1.head, "{case}: head moved");
        assert_eq!(moved.group, group, "{case}: head is the other writer's");
        assert_eq!(moved.row, cut1.row, "{case}: the row as cut 1 left it");
        assert_eq!(
            moved.content[..BLOCK as usize],
            [0xCD; BLOCK as usize],
            "{case}"
        );
    }
}

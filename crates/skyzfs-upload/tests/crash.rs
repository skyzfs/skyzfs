//! The crash matrix of the M2.4 row: the daemon lost at every operation
//! on object storage (before it, and after it with the reply lost, which
//! at the head swap is "the swap landed, then the daemon died") and at
//! every store into the cut state, while it uploads two cuts: the first
//! of the volume, whole, and the one after it, a delta, each read in part
//! from copies.
//!
//! After each death: head names a commit that is a whole cut, the row
//! names that cut, and the volume passes fsck; a start that takes the
//! cut state up uploads what is left with no mismatch, and the cut after
//! it goes up as a delta. Each uploaded commit reads as the volume at its
//! cut's instant, so no cut holds a block from after its instant.

mod common;

use std::panic::AssertUnwindSafe;

use common::*;
use futures_util::FutureExt;
use skynbd_store::{FaultOp, FaultWhen};
use skyzfs_local::inplace::{InPlace, Lost};
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::{Doing, Mismatch, Rule, Step};

/// Where the daemon dies.
#[derive(Debug, Clone, Copy)]
enum Kill {
    /// At the `k`th operation on object storage, from the run's first.
    Remote(FaultWhen, u64),
    /// At the `n`th store into the cut state's region, from the run's
    /// first.
    Region(u64),
}

/// Slices of 16 blocks, so that a whole cut takes sixteen reads and
/// seals segments between them.
const SLICE: usize = 16;

/// Cut 1 of group 7 (whole, the first) and cut 2 of group 8, each with a
/// block overwritten after its instant, so that each reads a copy. The
/// whole cut holds 40 blocks of data besides its uberblocks, in four
/// slices (three on a sparse volume): a segment sealed and PUT for each
/// before the commit.
fn two_cuts(fx: &mut Fixture) -> InPlace<SimDisk> {
    let mut store = fx.start();
    for block in 60..100 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    fx.write(&mut store, 61, 0xA1);
    fx.write(&mut store, 70, 0x70);
    fx.group(&mut store, 8);
    fx.write(&mut store, 70, 0xB0);
    store
}

/// What one kill point showed.
#[derive(Debug, Default)]
struct Seen {
    /// The kill came: the run did not end on its own.
    died: bool,
    /// The restart found a cut committed whose commit the daemon had not
    /// heard of.
    landed_unheard: u64,
}

/// The fixture, on a sparse volume or one with no holes.
async fn fixture(sparse: bool) -> Fixture {
    if sparse {
        Fixture::sparse(20).await
    } else {
        Fixture::new(20).await
    }
}

async fn kill_at(kill: Kill, sparse: bool) -> Seen {
    quiet_deaths();
    let case = format!("{kill:?}, sparse {sparse}");
    let mut fx = fixture(sparse).await;
    let store = two_cuts(&mut fx);
    let mut up = fx.driver(store, SLICE);
    match kill {
        Kill::Remote(when, k) => fx.store.arm_when(FaultOp::Any, "", k, when).await,
        Kill::Region(n) => fx.region.die_at(n),
    }
    let ran = AssertUnwindSafe(up.run()).catch_unwind().await;
    let died = match ran {
        Ok(Ok(steps)) => {
            assert!(
                matches!(steps.last(), Some(Step::Uploaded { number: 2, .. })),
                "{case}: {steps:?}"
            );
            false
        }
        Ok(Err(_)) => matches!(kill, Kill::Remote(..)),
        Err(payload) if payload.downcast_ref::<Lost>().is_some() => true,
        Err(payload) => std::panic::resume_unwind(payload),
    };
    fx.store.disarm().await;
    if !died {
        fx.assert_uploaded(8, &case).await;
        return Seen::default();
    }

    // Head as the death left it: the empty commit, or a whole cut.
    let left = fx.uploaded().await;
    match left.group {
        None => {
            assert_eq!(left.row, None, "{case}: a row with the empty commit");
            assert!(left.content.iter().all(|&b| b == 0), "{case}");
            assert!(left.fsck, "{case}: fsck");
        }
        Some(txg) => {
            fx.assert_uploaded(txg, &format!("{case}, as the death left it"))
                .await;
        }
    }

    fx.died(up);
    let mut up = fx.driver(fx.start(), SLICE);
    let steps = up
        .run()
        .await
        .unwrap_or_else(|e| panic!("{case}: the run after the restart: {e}"));
    assert_eq!(up.figures().mismatches, 0, "{case}: {steps:?}");
    assert!(up.local().held_cuts().is_empty(), "{case}: {steps:?}");
    fx.assert_uploaded(8, &format!("{case}, after the restart"))
        .await;
    let landed_unheard = up.figures().found_committed;

    // The next cut goes up as a delta onto the last.
    fx.write(up.local(), 71, 0x71);
    fx.group(up.local(), 9);
    let steps = up.run().await.expect("the next cut");
    assert!(
        steps.contains(&Step::Began {
            number: 3,
            rule: Rule::Delta
        }),
        "{case}: {steps:?}"
    );
    fx.assert_uploaded(9, &format!("{case}, the next cut"))
        .await;
    Seen {
        died: true,
        landed_unheard,
    }
}

/// The run with no death, step by step: its operations on object
/// storage, and how many of them it had made when it came to the first
/// cut's commit.
async fn operations_of_the_run(sparse: bool) -> (Vec<String>, usize) {
    let mut fx = fixture(sparse).await;
    let store = two_cuts(&mut fx);
    let mut up = fx.driver(store, SLICE);
    fx.store.inner().record(true);
    let mut at_commit = None;
    loop {
        if at_commit.is_none() && up.doing() == Doing::Committing {
            at_commit = Some(fx.store.inner().log().len());
        }
        if up.step().await.expect("a step") == Step::Idle {
            break;
        }
    }
    fx.store.inner().record(false);
    let at_commit = at_commit.expect("the run came to a commit");
    (fx.store.inner().log(), at_commit)
}

/// The run with no death, step by step: how many stores into the cut
/// state it made, and how many it had made when each cut's release
/// began (the step that commits it, which is the step that lets it go).
async fn stores_of_the_run(sparse: bool) -> (u64, Vec<u64>) {
    let mut fx = fixture(sparse).await;
    let store = two_cuts(&mut fx);
    let mut up = fx.driver(store, SLICE);
    let first = fx.region.stores();
    let mut releases = Vec::new();
    loop {
        let before = fx.region.stores() - first;
        let step = up.step().await.expect("a step");
        match step {
            Step::Idle => break,
            Step::Uploaded { .. } => releases.push(before),
            _ => assert_eq!(fx.region.stores() - first, before, "{step:?} stored"),
        }
    }
    (fx.region.stores() - first, releases)
}

/// The daemon lost at every operation on object storage, before it and
/// after it: a kill point at each operation of the run. Before, no
/// commit lands unheard; after, exactly the two head swaps do, and the
/// restart lets each cut go with nothing uploaded.
#[tokio::test]
async fn a_death_at_any_operation_on_object_storage_leaves_a_whole_cut_and_the_upload_goes_on() {
    deaths_at_operations(false).await;
}

/// [`a_death_at_any_operation_on_object_storage_leaves_a_whole_cut_and_the_upload_goes_on`]
/// on a sparse volume (M2.4e): the whole cut reads only what was written
/// since its state began, and the rest goes up as zeros.
#[tokio::test]
async fn a_death_at_any_operation_on_object_storage_of_a_sparse_volume() {
    deaths_at_operations(true).await;
}

async fn deaths_at_operations(sparse: bool) {
    let (operations, at_commit) = operations_of_the_run(sparse).await;
    let swaps: Vec<usize> = operations
        .iter()
        .enumerate()
        .filter(|(_, op)| *op == "put_if refs")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(swaps.len(), 2, "a head swap for each cut: {operations:?}");
    let commits: Vec<usize> = operations
        .iter()
        .enumerate()
        .filter(|(_, op)| *op == "put commit")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(commits.len(), 2, "{operations:?}");
    // Counted where the driver comes to the commit, not at the commit's
    // own PUTs: the engine's commit PUTs what is left sealed itself.
    let early = operations[..at_commit]
        .iter()
        .filter(|op| *op == "put seg")
        .count();
    // Where the volume has no holes, its data (blocks 60 to 99) runs
    // through four slices of the volume's blocks; where it is sparse, the
    // 44 blocks written since the state began (the data and the four
    // uberblock slots) are read 16 to a slice, in three.
    let slices = if sparse { 3 } else { 4 };
    assert!(
        early >= slices,
        "the whole cut's data runs through {slices} slices, and each \
         slice's segment is PUT before the commit: {operations:?}"
    );
    for when in [FaultWhen::Before, FaultWhen::After] {
        let mut points = 0;
        let mut landed_unheard = 0;
        for k in 0.. {
            let seen = kill_at(Kill::Remote(when, k), sparse).await;
            if !seen.died {
                break;
            }
            points += 1;
            landed_unheard += seen.landed_unheard;
        }
        assert_eq!(
            points,
            operations.len(),
            "{when:?}: a kill point at each operation of {operations:?}"
        );
        let expected = match when {
            FaultWhen::Before => 0,
            FaultWhen::After => 2,
        };
        assert_eq!(landed_unheard, expected, "{when:?}");
    }
}

/// The daemon lost at every store into the cut state while it uploads:
/// those of each release, after its commit. The restart finds the cut
/// held and the row naming it, and lets it go, or finds it gone and the
/// intent map naming it; never a mismatch.
///
/// A kill point at each store of the run with no death, counted first.
/// The cut is found held for the row to name exactly where the release
/// dies before the store that drops it: at cut 1's first store, and at
/// cut 2's first two, since cut 2 is the last held and its release
/// moves the open epoch's lower end before it drops the cut.
#[tokio::test]
async fn a_death_at_any_store_into_the_cut_state_leaves_a_whole_cut_and_no_mismatch() {
    deaths_at_stores(false).await;
}

/// [`a_death_at_any_store_into_the_cut_state_leaves_a_whole_cut_and_no_mismatch`]
/// on a sparse volume (M2.4e), whose state holds the image's holes until
/// the first cut is let go.
#[tokio::test]
async fn a_death_at_any_store_into_the_cut_state_of_a_sparse_volume() {
    deaths_at_stores(true).await;
}

async fn deaths_at_stores(sparse: bool) {
    let (stores, releases) = stores_of_the_run(sparse).await;
    assert_eq!(releases.len(), 2, "a release for each cut");
    let mut points = 0;
    let mut held_for_the_row = Vec::new();
    for n in 0.. {
        let seen = kill_at(Kill::Region(n), sparse).await;
        if !seen.died {
            break;
        }
        points += 1;
        if seen.landed_unheard > 0 {
            held_for_the_row.push(n);
        }
    }
    assert_eq!(points, stores, "a kill point at each store of the run");
    assert_eq!(
        held_for_the_row,
        [releases[0], releases[1], releases[1] + 1],
        "the stores at which the cut is found held, of {stores} \
         (releases at {releases:?})"
    );
}

/// The bound takes a cut after its commit has landed and before it is
/// let go, and the daemon dies there. Died once the driver had named it,
/// the restart finds the intent map naming it and uploads its heir as a
/// delta. Died before, the map names the cut before it, the check finds
/// a mismatch (Q8 stays strict), and the cut after it goes up whole:
/// one whole upload, and never a cut uploaded onto a base it was not
/// made against.
#[tokio::test]
async fn a_death_around_the_naming_of_a_cut_the_bound_took_costs_one_whole_upload_at_most() {
    let told = |steps: &[Step]| -> Vec<Step> {
        steps
            .iter()
            .filter(|step| !matches!(step, Step::Read { .. }))
            .cloned()
            .collect()
    };
    for named in [true, false] {
        let case = if named {
            "died once named"
        } else {
            "died before"
        };
        let mut fx = Fixture::new(22).await;
        let mut up = committed_then_taken(&mut fx).await;
        if named {
            assert_eq!(
                up.step().await,
                Ok(Step::UploadedNotHeld { number: 2, named }),
                "{case}"
            );
        }
        fx.died(up);
        let mut up = fx.driver(fx.start(), 64);
        let base = up
            .local()
            .committed_cut()
            .map(|cut| (cut.number, cut.group));
        assert_eq!(base, Some(if named { (2, 8) } else { (1, 7) }), "{case}");
        let steps = up.run().await.expect("the run after the restart");
        if named {
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
                "{case}"
            );
            fx.assert_uploaded(9, case).await;
            continue;
        }
        assert_eq!(
            told(&steps),
            [Step::Mismatch {
                why: Mismatch::NotTheBase,
                abandoned: vec![3]
            }],
            "{case}"
        );
        fx.assert_uploaded(8, case).await;
        fx.group(up.local(), 10);
        let steps = up.run().await.expect("the cut after the mismatch");
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
                },
            ],
            "{case}"
        );
        fx.assert_uploaded(10, case).await;
    }
}

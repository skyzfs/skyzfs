//! The upload of a sparse volume (M2.4e): the holes of the image when its
//! cut state began go up as zeros with nothing read, a write into one
//! under a held whole cut makes no copy, and the volume in object storage
//! is the volume at each cut's instant, also where it held other bytes
//! before.

mod common;

use common::*;
use skyzfs_local::inplace::{CutHoles, CutsTakenUp, HOLE_BLOCKS};
use skyzfs_upload::{Rule, Step};

/// The steps of a run that are not reads.
fn told(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter(|step| !matches!(step, Step::Read { .. }))
        .cloned()
        .collect()
}

/// The blocks of the four uberblock slots a group writes.
const SLOTS: u64 = 4;

/// The first cut of a sparse volume reads only the blocks written since
/// its state began and writes the rest as zeros; the hole set is dropped
/// once it is uploaded, and the next cut goes up as a delta.
#[tokio::test]
async fn the_first_cut_of_a_sparse_volume_reads_what_was_written_and_its_holes_go_up_as_zeros() {
    let mut fx = Fixture::sparse(30).await;
    let mut store = fx.start();
    let figures = store.cut_figures();
    assert_eq!(
        (figures.holes, figures.hole_chunks),
        (CutHoles::Held, BLOCKS / HOLE_BLOCKS),
        "a sparse image is all holes when its first state begins"
    );
    fx.write(&mut store, 10, 0x10);
    fx.write(&mut store, 11, 0x00);
    for block in 40..44 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 8);

    let steps = up.run().await.expect("the upload");
    let read = 2 + 4 + SLOTS;
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
                blocks: read
            },
        ]
    );
    let zeroes: u64 = steps
        .iter()
        .map(|step| match step {
            Step::Read { zeroes, .. } => *zeroes,
            _ => 0,
        })
        .sum();
    assert_eq!(zeroes, BLOCKS - read, "every other block goes up as zeros");
    fx.assert_uploaded(7, "the first cut").await;
    let figures = up.figures();
    assert_eq!((figures.blocks, figures.zero_blocks), (read, BLOCKS - read));
    let held = up.local().cut_figures();
    assert_eq!(
        (held.holes, held.hole_chunks),
        (CutHoles::Dropped, BLOCKS / HOLE_BLOCKS),
        "no cut reads by the set once the first is uploaded"
    );

    fx.write(up.local(), 12, 0x12);
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
                blocks: 1 + SLOTS
            },
        ]
    );
    fx.assert_uploaded(8, "the second cut").await;
    assert_eq!(up.figures().zero_blocks, BLOCKS - read, "a delta has none");
}

/// The first cut, whole, goes up onto a commit of another writer's
/// (no group, no row) that holds other bytes where the image has a hole
/// nothing wrote: the zeros are written over them. Mutant: the uploader
/// leaves a slice's runs of zeros out, and object storage keeps the old
/// bytes.
#[tokio::test]
async fn a_whole_cut_writes_its_holes_over_what_object_storage_held() {
    let mut fx = Fixture::sparse(31).await;
    // Block 0 holds 0xAB in object storage, and nothing on the image.
    fx.move_head().await;
    let mut store = fx.start();
    fx.write(&mut store, 20, 0x20);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 8);
    let steps = up.run().await.expect("the upload");
    assert!(
        steps.contains(&Step::Began {
            number: 1,
            rule: Rule::Whole
        }),
        "{steps:?}"
    );
    let uploaded = fx.assert_uploaded(7, "onto another writer's commit").await;
    assert!(
        uploaded.content[..BLOCK as usize].iter().all(|&b| b == 0),
        "the old bytes are written over"
    );
    let read = 1 + SLOTS;
    let figures = up.figures();
    assert_eq!((figures.blocks, figures.zero_blocks), (read, BLOCKS - read));
}

/// The first cut, whole, goes up onto a commit of another writer's
/// that holds other bytes in every block (the review of #125, G2). The
/// blocks written lie apart, so that each slice of eight blocks holds
/// several runs of zeros, and the last slice is a run alone, from after
/// the last uberblock slot to the volume's end. Every block of every
/// run is written over: the last block of a run, a slice's runs after
/// its first, and the runs of a slice that has no block. Mutants: each
/// of those left out, and object storage keeps old bytes there.
#[tokio::test]
async fn a_whole_cut_writes_every_run_of_its_holes_over_what_object_storage_held() {
    let mut fx = Fixture::sparse(34).await;
    fx.move_head_everywhere().await;
    let mut store = fx.start();
    // Twelve blocks, apart from one another and from the four slots of
    // group 7 (blocks 39, 103, 167 and 231), all below the last slot:
    // sixteen blocks to read, two slices of eight.
    let written = [2, 5, 9, 14, 20, 27, 50, 70, 120, 140, 180, 200];
    for (i, &block) in written.iter().enumerate() {
        fx.write(&mut store, block, 0x40 + i as u8);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 8);
    let steps = up.run().await.expect("the upload");
    let slices: Vec<(u64, u64)> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Read { blocks, zeroes, .. } => Some((*blocks, *zeroes)),
            _ => None,
        })
        .collect();
    assert_eq!(
        slices.iter().map(|&(blocks, _)| blocks).collect::<Vec<_>>(),
        [8, 8, 0],
        "the slices as (blocks, zeros): {slices:?}"
    );
    assert!(
        slices.iter().all(|&(_, zeroes)| zeroes > 0),
        "each slice has runs of zeros: {slices:?}"
    );
    let uploaded = fx.assert_uploaded(7, "onto old bytes in every block").await;
    for block in 0..BLOCKS {
        let at = (block * BLOCK) as usize;
        let old = old_byte(block);
        assert!(
            uploaded.content[at..at + BLOCK as usize]
                .iter()
                .any(|&byte| byte != old),
            "block {block} still holds the old bytes"
        );
    }
    let read = (written.len() as u64) + SLOTS;
    let figures = up.figures();
    assert_eq!((figures.blocks, figures.zero_blocks), (read, BLOCKS - read));
}

/// A reboot loses the cut state and the intent map: the next start
/// begins a state with nothing known and records the image's holes anew,
/// where nothing was written up to then; its first cut, whole, goes up
/// onto the last commit, reading only the chunks that hold data.
#[tokio::test]
async fn a_state_begun_after_a_reboot_records_the_holes_anew() {
    let mut fx = Fixture::sparse(33).await;
    let mut store = fx.start();
    fx.write(&mut store, 20, 0x20);
    fx.write(&mut store, 21, 0x21);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 8);
    up.run().await.expect("the first upload");
    fx.assert_uploaded(7, "the first cut").await;
    fx.rebooted(up);

    let mut store = fx.start();
    assert!(
        matches!(store.cuts_at_start(), Some(CutsTakenUp::Begun(_))),
        "{:?}",
        store.cuts_at_start()
    );
    // Blocks 20 and 21 make their chunk data, and so do the uberblock
    // slots, one chunk each.
    let data = 1 + SLOTS;
    let holes = BLOCKS / HOLE_BLOCKS - data;
    let figures = store.cut_figures();
    assert_eq!(
        (figures.holes, figures.hole_chunks),
        (CutHoles::Held, holes)
    );
    fx.write(&mut store, 130, 0x82);
    fx.group(&mut store, 9);
    let mut up = fx.driver(store, 8);
    let steps = up.run().await.expect("the whole upload");
    assert!(
        steps.contains(&Step::Began {
            number: 1,
            rule: Rule::Whole
        }),
        "{steps:?}"
    );
    fx.assert_uploaded(9, "the whole cut after the reboot")
        .await;
    // The data chunks are read whole, and block 130, written since the
    // state began, in its hole.
    let read = data * HOLE_BLOCKS + 1;
    let figures = up.figures();
    assert_eq!((figures.blocks, figures.zero_blocks), (read, BLOCKS - read));
}

/// A seed under load: while the first cut, whole, is held, a write into a
/// hole of the image makes no copy, and a write over data the cut holds
/// makes one; the upload is the volume at the cut's instant. On a volume
/// whose image has no holes the same writes copy both blocks, as before
/// M2.4e.
#[tokio::test]
async fn a_write_into_a_hole_under_the_first_cut_makes_no_copy_and_the_upload_is_the_instant() {
    for sparse in [true, false] {
        let case = if sparse { "sparse" } else { "no holes" };
        let mut fx = if sparse {
            Fixture::sparse(32).await
        } else {
            Fixture::new(32).await
        };
        let mut store = fx.start();
        for block in 64..96 {
            fx.write(&mut store, block, block as u8);
        }
        fx.group(&mut store, 7);
        assert_eq!(store.cut_figures().copied, 0, "{case}");
        fx.write(&mut store, 3, 0x03);
        fx.write(&mut store, 70, 0x77);
        let copied = if sparse { 1 } else { 2 };
        assert_eq!(store.cut_figures().copied, copied * BLOCK, "{case}");
        let mut up = fx.driver(store, 8);
        let steps = up.run().await.expect("the upload");
        assert!(
            matches!(
                steps.last(),
                Some(Step::Uploaded {
                    number: 1,
                    rule: Rule::Whole,
                    ..
                })
            ),
            "{case}: {steps:?}"
        );
        let uploaded = fx.assert_uploaded(7, case).await;
        assert_eq!(uploaded.content[(3 * BLOCK) as usize], 0, "{case}");
        assert_eq!(uploaded.content[(70 * BLOCK) as usize], 70, "{case}");
    }
}

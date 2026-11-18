//! Review scenarios for #112 at 4c887b5 (M2.4b-1, the next look), through the store on the
//! simulated disk. Not part of the product: `scenarios.py` puts this file
//! under the store's cut tests as `tests/cuts/review_m24b1.rs` in a checkout
//! of its own, runs it and takes it out again.

use super::*;

fn phases(store: &InPlace<SimDisk>) -> Vec<(u64, CutPhase)> {
    store
        .held_cuts()
        .iter()
        .map(|c| (c.number, c.phase))
        .collect()
}

/// T1. A mismatch while a cut's flush is under way at the store (the
/// image's sync held by the disk): both cuts go and are counted; the
/// flush then ends, well or with the volume's failure, and changes
/// nothing in the state; after a flush that ended well the pool's next
/// group makes a cut that is the whole volume, read in slices as the
/// volume was at its instant, under writes.
#[test]
fn review_t1_a_mismatch_while_a_cuts_flush_is_under_way() {
    for fails in [false, true] {
        let case = format!("the flush fails: {fails}");
        let disk = created(71, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, IoMode::default());
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        write(&mut store, 6, 0x67).unwrap();
        write(&mut store, 7, 0x77).unwrap();
        disk.hold_syncs_of(crate::store::HOME_NAME);
        let mut done = Vec::new();
        for label in 0..LABELS {
            let id = 300 + label as u64;
            submit(&mut store, id, uberblock_write(label, 9, 90));
            assert!(until_done(&mut store, &mut done, id), "{case}");
        }
        submit(&mut store, 310, Op::Flush);
        polled(&mut store, &mut done, usize::MAX, Duration::from_millis(50));
        assert_eq!(
            phases(&store),
            [(2, CutPhase::Uploading), (3, CutPhase::Flushing)],
            "{case}"
        );
        // A write received after the flush goes on while its sync runs,
        // and copies for cut 3.
        submit(&mut store, 311, write_op(7, 0x78));
        assert!(until_done(&mut store, &mut done, 311), "{case}");
        assert!(store.cut_figures().copied > 0, "{case}");
        let abandoned = store.cut_figures().abandoned;
        assert_eq!(store.abandon_all_cuts(), [2, 3], "{case}");
        assert!(store.held_cuts().is_empty(), "{case}");
        assert_eq!(store.cut_figures().abandoned, abandoned + 2, "{case}");
        assert_eq!(store.cut_figures().copied, 0, "{case}");
        if fails {
            disk.fail_sync_of(crate::store::HOME_NAME);
        }
        disk.release_syncs();
        assert!(until_done(&mut store, &mut done, 310), "{case}");
        let flush = done.iter().find(|c| c.id.0 == 310).expect("the flush");
        assert_eq!(flush.result.is_ok(), !fails, "{case}");
        assert!(store.held_cuts().is_empty(), "{case}: after the flush ended");
        assert_eq!(store.cut_figures().abandoned, abandoned + 2, "{case}");
        if fails {
            disk.clear_faults();
            println!("T1 ({case}): two cuts abandoned and counted, the failed flush changes nothing");
            continue;
        }
        write(&mut store, 9, 0x99).unwrap();
        group(&mut store, 400, 10, 100);
        let four = store.held_cuts().last().map(|c| (c.number, c.phase));
        assert_eq!(four, Some((4, CutPhase::Uploading)), "{case}");
        assert!(store.cut_is_whole(4), "{case}");
        let at_instant: Vec<Vec<u8>> = (0..VOLUME_BLOCKS)
            .map(|block| read(&mut store, block).unwrap())
            .collect();
        for block in [6, 7, 9, 0, 70, 255] {
            write(&mut store, block, 0xA0).unwrap();
        }
        for size in [1, 7, 64] {
            let (blocks, _) = sliced(&mut store, 4, size);
            assert_eq!(blocks.len() as u64, VOLUME_BLOCKS, "{case}");
            for (block, content) in blocks {
                assert!(
                    content == at_instant[block as usize],
                    "{case}: block {block} of cut 4 in slices of {size}"
                );
            }
        }
        println!("T1 ({case}): two cuts abandoned and counted; cut 4 is the whole volume as at its instant");
    }
}

/// T3. The buffer of a slice is a `Buf` now, so the unaligned buffer of
/// the first look is no call that compiles. What is left to a caller:
/// a buffer of no block (made so, or lent at that length), which is
/// refused and abandons nothing; and a buffer lent shorter than its
/// capacity, which is filled to its length and no further.
#[test]
fn review_t3_a_slice_into_a_buffer_of_no_block() {
    for (name, io) in [("direct", ON_CALLER), ("through the cache", BUFFERED)] {
        let (_disk, mut store) = two_cuts_with_a_copy(72, io);
        let before = (phases(&store), store.cut_figures().abandoned);
        let mut none = Buf::zeroed(0);
        let got = store
            .read_cut_from(1, 0, &mut none)
            .map(|slice| slice.blocks)
            .map_err(|e| e.kind());
        assert_eq!(got, Err(io::ErrorKind::InvalidInput), "{name}");
        let mut lent = Buf::with_capacity(4);
        assert!(lent.set_blocks(0).is_ok());
        let got = store
            .read_cut_from(1, 0, &mut lent)
            .map(|slice| slice.blocks)
            .map_err(|e| e.kind());
        assert_eq!(got, Err(io::ErrorKind::InvalidInput), "{name}");
        assert_eq!(
            (phases(&store), store.cut_figures().abandoned),
            before,
            "{name}: a refused buffer abandons nothing"
        );
        assert!(!store.is_failed(), "{name}");
        // Lent at two blocks of four: two blocks are read, and the
        // memory past the length is not written.
        let mut lent = Buf::filled(4, 0xEE);
        assert!(lent.set_blocks(2).is_ok());
        let slice = store.read_cut_from(1, 0, &mut lent).expect("reads");
        assert!(slice.blocks.len() <= 2, "{name}: {:?}", slice.blocks);
        assert!(lent.set_blocks(4).is_ok());
        assert!(
            lent[2 * BLOCK_USIZE..].iter().all(|&byte| byte == 0xEE),
            "{name}: the slice wrote past the buffer's length"
        );
        println!(
            "T3 ({name}): a buffer of no block is refused and abandons nothing; held {:?}; \
             a buffer lent at 2 blocks of 4 took {:?}",
            phases(&store),
            slice.blocks
        );
    }
}

/// T4. Runs where blocks and slots do not go together: two blocks apart
/// with copies in adjacent slots, two adjacent blocks with their copies
/// in slots the other way round, three adjacent blocks whose slots have
/// a gap, and a run made by one write. A slice holds each block as the
/// cut reads it alone, in every size of slice, and the reads are counted.
#[test]
fn review_t4_runs_where_blocks_and_slots_do_not_go_together() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(73, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 128..160 {
            write(&mut store, block, block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        let own = [130u64, 132, 134, 135, 140, 141, 142, 150, 151];
        let content = |block: u64| vec![0x30 + (block - 130) as u8; BLOCK_USIZE];
        for block in own {
            write(&mut store, block, 0x30 + (block - 130) as u8).unwrap();
        }
        group(&mut store, 200, 8, 80);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        assert!(!store.cut_is_whole(2), "{case}");
        // The copies, a slot each in this order.
        for block in [130u64, 132, 135, 134, 140, 142, 141] {
            write(&mut store, block, 0xEE).unwrap();
        }
        let done = run(
            &mut store,
            1,
            Op::Write {
                offset: 150 * BLOCK,
                data: Buf::filled(2, 0xEF),
            },
        );
        assert_eq!(done.result, Ok(()), "{case}");
        let alone: Vec<(u64, Vec<u8>)> = store
            .cut_map(2)
            .into_iter()
            .map(|block| {
                let mut buf = Buf::filled(1, 0xEE);
                assert_eq!(store.read_cut(2, block, &mut buf).ok(), Some(true));
                (block, buf.to_vec())
            })
            .collect();
        for block in own {
            let read = alone.iter().find(|(b, _)| *b == block).map(|(_, c)| c.clone());
            assert!(read == Some(content(block)), "{case}: block {block} alone");
        }
        for size in [1, 2, 3, 4, 5, 16, 64] {
            let (blocks, _) = sliced(&mut store, 2, size);
            for ((block, got), (_, want)) in blocks.iter().zip(&alone) {
                assert!(got == want, "{case}: block {block} in slices of {size}");
            }
            assert!(blocks == alone, "{case}: cut 2 in slices of {size}");
        }
        let image = disk.reads_of(crate::store::HOME_NAME);
        let copies = disk.reads_of(COPIES_NAME);
        let mut buf = Buf::filled(64, 0xEE);
        let slice = store.read_cut_from(2, 0, &mut buf).expect("reads");
        let image = disk.reads_of(crate::store::HOME_NAME) - image;
        let copies = disk.reads_of(COPIES_NAME) - copies;
        println!(
            "T4 ({case}): cut 2 holds {:?}; one slice read the image {image} times and the copies {copies} times",
            slice.blocks
        );
        // 130 and 132 in one read; 134, 135, 140, 141, 142 a read each;
        // 150 and 151 in one read.
        assert_eq!(copies, 7, "{case}: the reads of the copies");
    }
}

/// T5. The bound abandons the cut being uploaded after its commit landed
/// in object storage and before the release (no death: the commit is a
/// round trip, and writes go on meanwhile). What the store then offers
/// the uploader. Printed; the two facts the note rests on are judged.
#[test]
fn review_t5_the_bound_abandons_the_cut_between_its_commit_and_its_release() {
    let disk = created(74, 0);
    let mut store = start_on(&disk, 0, &region(), IoMode::default());
    write(&mut store, 5, 0x55).unwrap();
    group(&mut store, 100, 7, 70);
    assert!(store.release_cut(1));
    let base = store.committed_cut();
    // Cut 2 holds more blocks than the bound has room for copies of.
    let own: Vec<u64> = (64..96).chain(128..160).chain(192..200).collect();
    assert!(own.len() as u64 > TEST_BOUND / BLOCK);
    for &block in &own {
        write(&mut store, block, 0x22).unwrap();
    }
    group(&mut store, 200, 8, 80);
    assert_eq!(phases(&store), [(2, CutPhase::Uploading)]);
    assert!(!store.cut_is_whole(2));
    // "The commit of cut 2 lands here." Before the release, the pool
    // writes over cut 2's blocks.
    let abandoned = store.cut_figures().abandoned;
    for &block in &own {
        write(&mut store, block, 0x33).unwrap();
    }
    assert_eq!(store.cut_figures().abandoned, abandoned + 1, "the bound took cut 2");
    assert!(phases(&store).is_empty());
    let released = store.release_cut(2);
    assert!(!released, "the cut is not held: nothing to release");
    assert_eq!(store.committed_cut(), base, "the map still names cut 1");
    group(&mut store, 300, 9, 90);
    let next = store.held_cuts().first().map(|c| (c.number, c.phase, c.lo));
    println!(
        "T5: cut 2 was committed and then abandoned at the bound; release_cut(2) says {released}; \
         the intent map names {:?}; the next cut is {next:?}, whole: {}",
        store.committed_cut().map(|c| (c.number, c.group)),
        store.cut_is_whole(3)
    );
    assert_eq!(next, Some((3, CutPhase::Uploading, 1)));
}

/// T6. A block read from the image whose next block is read from the
/// copy in the slot of the next number, and the other way round: the
/// two are not one read, though their numbers follow.
#[test]
fn review_t6_a_copy_whose_slot_follows_a_blocks_number() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(75, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in [0u64, 1, 2, 200, 201] {
            write(&mut store, block, 0x10 + block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        let own = [0u64, 1, 2, 200, 201];
        for block in own {
            write(&mut store, block, 0x40 + (block % 64) as u8).unwrap();
        }
        group(&mut store, 200, 8, 80);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        // Copies: block 0 in slot 0, block 200 in slot 1, block 2 in
        // slot 2. Blocks 1 and 201 stay in the image. In the order of
        // the map: copy 0, image 1, copy 2, ..., copy 1, image 201.
        for block in [0u64, 200, 2] {
            write(&mut store, block, 0xEE).unwrap();
        }
        let map = store.cut_map(2);
        for size in [1, 2, 3, 64] {
            let (blocks, _) = sliced(&mut store, 2, size);
            assert_eq!(blocks.len(), map.len(), "{case}");
            for (block, content) in blocks {
                if own.contains(&block) {
                    assert!(
                        content == vec![0x40 + (block % 64) as u8; BLOCK_USIZE],
                        "{case}: block {block} of cut 2 in slices of {size} holds {:#x}",
                        content[0]
                    );
                }
            }
        }
        println!("T6 ({case}): cut 2 holds {map:?}, each block as at its instant");
    }
}

/// T7. Blocks of the image two apart: a cut that holds blocks 10, 12,
/// 14, 15 and 17, none changed since, reads each from its own place,
/// and only 14 and 15 in one read.
#[test]
fn review_t7_blocks_of_the_image_two_apart() {
    for io in [IoMode::default()].into_iter().chain(OTHER_IO) {
        let case = format!("{io:?}");
        let disk = created(76, 0);
        let mut store = start_on(&disk, 0, &region(), io);
        for block in 8u64..20 {
            write(&mut store, block, 0x60 + block as u8).unwrap();
        }
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        let own = [10u64, 12, 14, 15, 17];
        for block in own {
            write(&mut store, block, 0x20 + block as u8).unwrap();
        }
        group(&mut store, 200, 8, 80);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading)], "{case}");
        let map = store.cut_map(2);
        for size in [1, 2, 3, 64] {
            let (blocks, slices) = sliced(&mut store, 2, size);
            assert_eq!(blocks.len(), map.len(), "{case}");
            assert!(slices.iter().all(|(_, copied)| !copied), "{case}");
            for (block, content) in blocks {
                if own.contains(&block) {
                    assert!(
                        content == vec![0x20 + block as u8; BLOCK_USIZE],
                        "{case}: block {block} of cut 2 in slices of {size} holds {:#x}",
                        content[0]
                    );
                }
            }
        }
        let image = disk.reads_of(crate::store::HOME_NAME);
        let mut buf = Buf::filled(64, 0xEE);
        store.read_cut_from(2, 0, &mut buf).expect("reads");
        let image = disk.reads_of(crate::store::HOME_NAME) - image;
        println!("T7 ({case}): cut 2 holds {map:?}; one slice read the image {image} times");
        assert_eq!(image as usize, map.len() - 1, "{case}: only 14 and 15 are one read");
    }
}

/// A start as `coming` says, or why it was refused.
fn try_start(
    disk: &SimDisk,
    region: &state::MemRegion,
    coming: Coming,
) -> Result<InPlace<SimDisk>, String> {
    let options = OpenOptions {
        io: ON_CALLER,
        acknowledge_failure: coming.acknowledging,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(region.clone()),
            boot: coming.boot,
            bound: Some(TEST_BOUND),
            intent_region: Some(TEST_INTENT_REGION),
        }),
        ..OpenOptions::default()
    };
    let lock = lock(disk).map_err(|e| format!("the lock: {e:?}"))?;
    let mut opened = Opened::open(disk.clone(), lock, options)
        .map_err(|(e, _)| format!("the open is refused: {e:?}"))?;
    if coming.taken_over {
        opened.device_taken_over();
    }
    opened
        .start_with(None, Mutants(0))
        .map_err(|e| format!("the start is refused: {e:?}"))
}

/// T8. A release whose write of the map fails (the answer to the first
/// look keeps the cut held there and says true): what the store says
/// and holds then, what a second release does (the file still refusing,
/// or writable again), and what the starts after it find: one that
/// does not acknowledge the failure, and one that does. Printed; what
/// the review's note rests on is judged.
#[test]
fn review_t8_a_release_whose_map_cannot_be_written() {
    for again in ["no second release", "a second release, the file still refusing", "a second release, the file writable again"] {
        let disk = created(77, 0);
        let region = region();
        let mut store = start_on(&disk, 0, &region, ON_CALLER);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        write(&mut store, 7, 0x77).unwrap();
        group(&mut store, 300, 9, 90);
        assert_eq!(phases(&store), [(2, CutPhase::Uploading), (3, CutPhase::Waiting)]);
        let named = |store: &InPlace<SimDisk>| store.committed_cut().map(|c| (c.number, c.group));
        assert_eq!(named(&store), Some((1, 7)));
        disk.fail_write_of(intent::INTENT_NAME);
        let first = store.release_cut(2);
        println!(
            "T8 ({again}): release_cut(2) with the map's file refusing says {first}; failed {}; held {:?}; \
             the map in memory names {:?}",
            store.is_failed(),
            phases(&store),
            named(&store)
        );
        assert!(first);
        assert!(store.is_failed());
        assert_eq!(phases(&store), [(2, CutPhase::Uploading), (3, CutPhase::Waiting)]);
        if again != "no second release" {
            if again.ends_with("writable again") {
                disk.clear_faults();
            }
            let second = store.release_cut(2);
            println!(
                "T8 ({again}): release_cut(2) once more says {second}; held {:?}; the map in memory names {:?}",
                phases(&store),
                named(&store)
            );
        }
        disk.clear_faults();
        drop(store);
        disk.kill();
        let plain = try_start(&disk, &region, TAKING_OVER);
        match &plain {
            Ok(store) => println!(
                "T8 ({again}): the start that acknowledges nothing serves: cuts {:?}; held {:?}; the map names {:?}",
                store.cuts_at_start(),
                phases(store),
                named(store)
            ),
            Err(why) => println!("T8 ({again}): the start that acknowledges nothing: {why}"),
        }
        assert!(plain.is_err(), "a failed volume is refused until the failure is acknowledged");
        drop(plain);
        let acknowledging = Coming {
            acknowledging: true,
            ..TAKING_OVER
        };
        let store = try_start(&disk, &region, acknowledging).expect("the acknowledging start");
        println!(
            "T8 ({again}): the start that acknowledges the failure: cuts {:?}; intent {:?}; held {:?}; the map names {:?}",
            store.cuts_at_start(),
            store.intent_at_start(),
            phases(&store),
            named(&store)
        );
        assert!(
            matches!(store.cuts_at_start(), Some(CutsTakenUp::Begun(_))),
            "the cut state of a failed volume is not taken up"
        );
        assert!(phases(&store).is_empty(), "no cut is held for the uploader's check to find");
    }
}

/// T9. The same release where the failure is not marked on the disk
/// (the mutant that leaves the marker out stands for a daemon that
/// dies before the marker is written, or a disk that takes no marker):
/// the next start is not refused, takes the cut state up, and holds the
/// cut for the uploader's check. With a death of the daemon alone
/// (the page cache lives) and with the unsynced pages lost.
#[test]
fn review_t9_the_same_release_where_the_failure_is_not_marked() {
    for pages_lost in [false, true] {
        let disk = created(78, 0);
        let region = region();
        let mut store = start_on(&disk, Mutants::FAIL_UNMARKED, &region, ON_CALLER);
        write(&mut store, 5, 0x55).unwrap();
        group(&mut store, 100, 7, 70);
        assert!(store.release_cut(1));
        write(&mut store, 6, 0x66).unwrap();
        group(&mut store, 200, 8, 80);
        write(&mut store, 7, 0x77).unwrap();
        group(&mut store, 300, 9, 90);
        let named = |store: &InPlace<SimDisk>| store.committed_cut().map(|c| (c.number, c.group));
        disk.fail_write_of(intent::INTENT_NAME);
        assert!(store.release_cut(2));
        assert!(store.is_failed());
        assert_eq!(phases(&store), [(2, CutPhase::Uploading), (3, CutPhase::Waiting)]);
        disk.clear_faults();
        drop(store);
        if pages_lost {
            disk.kill();
        }
        let mut store = try_start(&disk, &region, TAKING_OVER).expect("no marker: the start serves");
        println!(
            "T9 (unsynced pages lost: {pages_lost}): the start that acknowledges nothing serves: cuts {:?}; intent {:?}; \
             held {:?}; the map names {:?}",
            store.cuts_at_start(),
            store.intent_at_start(),
            phases(&store),
            named(&store)
        );
        assert_eq!(phases(&store), [(2, CutPhase::Uploading), (3, CutPhase::Waiting)]);
        let released = store.release_cut(2);
        println!(
            "T9 (unsynced pages lost: {pages_lost}): release_cut(2) at that start says {released}; failed {}; held {:?}; \
             the map names {:?}",
            store.is_failed(),
            phases(&store),
            named(&store)
        );
        assert!(released);
        assert!(!store.is_failed());
        assert_eq!(phases(&store), [(3, CutPhase::Uploading)]);
        assert_eq!(named(&store), Some((2, 8)));
    }
}

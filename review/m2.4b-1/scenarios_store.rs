//! Review scenarios for #112 at 88e4edc (M2.4b-1), through the store on the
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

/// T3. What a slice into a buffer that is not aligned for direct I/O
/// does: the read fails, and the store takes that for a read that failed
/// and abandons the cut being uploaded. Printed, not judged.
#[test]
fn review_t3_a_slice_into_an_unaligned_buffer() {
    for (name, io) in [("direct", ON_CALLER), ("through the cache", BUFFERED)] {
        let (_disk, mut store) = two_cuts_with_a_copy(72, io);
        let mut raw = vec![0u8; 3 * BLOCK_USIZE];
        let to_aligned = (raw.as_ptr() as usize).wrapping_neg() % BLOCK_USIZE;
        let at = to_aligned + 1;
        let before = phases(&store);
        let got = store
            .read_cut_from(1, 0, &mut raw[at..at + BLOCK_USIZE])
            .map(|slice| slice.blocks)
            .map_err(|e| e.kind());
        println!(
            "T3 ({name}): a slice of cut 1 into a buffer one byte off a block's alignment: {got:?}; \
             held before {before:?}, after {:?}; abandoned {}",
            phases(&store),
            store.cut_figures().abandoned
        );
        // The one-block read of M2.2 for comparison.
        let (_disk, mut store) = two_cuts_with_a_copy(72, io);
        let got = store
            .read_cut(1, 0, &mut raw[at..at + BLOCK_USIZE])
            .map_err(|e| e.kind());
        println!(
            "T3 ({name}): read_cut into the same buffer: {got:?}; held after {:?}; abandoned {}",
            phases(&store),
            store.cut_figures().abandoned
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
            write(&mut store, block, 0x40 + block as u8).unwrap();
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
                        content == vec![0x40 + block as u8; BLOCK_USIZE],
                        "{case}: block {block} of cut 2 in slices of {size} holds {:#x}",
                        content[0]
                    );
                }
            }
        }
        println!("T6 ({case}): cut 2 holds {map:?}, each block as at its instant");
    }
}

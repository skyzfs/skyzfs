
// ---- review of #125 (M2.4e): scenarios the tests lack ----
//
// Appended to crates/skyzfs-local/src/inplace/state/tests.rs for a run.
// The world with holes: chunks 0, 2, 3, 5, 7, 8, 9 and 12 (blocks 0-15,
// 32-63, 80-95, 112-159, 192-199) are holes; data blocks read DATA +
// block; blocks 100-107 are zeros in a chunk of data.

/// What held cut `number` reads for `block`, as a value of the world's
/// disk: `None` for a block it does not hold.
fn review_m24e_content(state: &CutState<MemRegion>, disk: &Disk, number: u64, block: u64) -> Option<u64> {
    match state.read(number, block).expect("held") {
        Read::Unchanged => None,
        Read::Zero => Some(0),
        Read::Image => Some(disk.image[block as usize]),
        Read::Copy(slot) => Some(*disk.copies.get(&slot).expect("a copy written")),
    }
}

/// Write value `value` into block `block` as the store does: the change
/// (copies, records, bit), then the write.
fn review_m24e_write(state: &mut CutState<MemRegion>, disk: &mut Disk, block: u64, value: u64) {
    state.change(block, 1, disk).expect("the change");
    disk.image[block as usize] = value;
}

/// R1. A mismatch while the hole set holds (the first cut is held, not
/// yet committed: the uploader's check finds head moved, or the row
/// unreadable) lets every held cut go and counts the whole volume as
/// changed. The next cut has `lo` 0 again and reads holes as zeros by
/// its map, so its map must still hold every block written since the
/// state began: before M2.4e a whole cut's map was never read, now it is.
/// Writes into holes in epoch 1 (before cut 1), in epoch 2 (after cut 1,
/// before the mismatch) and in epoch 3 (after the mismatch) all read as
/// written in the next cut; holes nothing wrote read as zeros; data reads
/// as data.
#[test]
fn review_m24e_r1_a_mismatch_while_the_set_holds_keeps_every_write_into_a_hole() {
    let mut world = run_in(true, &[], 0, 64, None).expect("runs");
    let (state, disk) = (&mut world.state, &mut world.disk);
    review_m24e_write(state, disk, 1, 11);
    let c1 = state.freeze(&marked(1), true);
    state.flushed(c1);
    review_m24e_write(state, disk, 33, 33);
    review_m24e_write(state, disk, 17, 17); // data, copied for cut 1
    assert_eq!(state.abandon_all(), [c1]);
    assert_eq!(state.figures().holes, Holes::Held, "nothing was committed");
    review_m24e_write(state, disk, 34, 34);
    let c2 = state.freeze(&marked(2), true);
    state.flushed(c2);
    assert!(state.is_whole_cut(c2));
    assert_eq!(state.held().first().map(|c| c.lo), Some(0));
    for (block, want) in [(1, 11), (33, 33), (34, 34), (17, 17), (16, DATA + 16), (2, 0), (35, 0), (199, 0)] {
        assert_eq!(review_m24e_content(state, disk, c2, block), Some(want), "block {block}");
    }
    // After cut 2's instant: a write into a hole that nothing wrote makes
    // no copy; one over a hole block written before the instant does.
    let copied = state.figures().copied;
    review_m24e_write(state, disk, 36, 36);
    assert_eq!(state.figures().copied, copied, "a copy for a hole");
    review_m24e_write(state, disk, 33, 333);
    assert_eq!(state.figures().copied, copied + BLOCK_SIZE, "no copy for a written hole");
    assert_eq!(review_m24e_content(state, disk, c2, 33), Some(33));
    assert_eq!(review_m24e_content(state, disk, c2, 36), Some(0));
}

/// R2. The same mismatch with three cuts held (cut 1 uploading, `lo` 0;
/// cut 2 waiting, `lo` 1; cut 3 flushing, `lo` 2), each epoch writing a
/// hole of its own: every map goes to the open map with `lo` 0, and the
/// next cut reads each of those blocks as written.
#[test]
fn review_m24e_r2_a_mismatch_with_three_cuts_held_keeps_every_write_into_a_hole() {
    let mut world = run_in(true, &[], 0, 64, None).expect("runs");
    let (state, disk) = (&mut world.state, &mut world.disk);
    review_m24e_write(state, disk, 1, 11);
    let c1 = state.freeze(&marked(1), true);
    state.flushed(c1);
    review_m24e_write(state, disk, 33, 33);
    let c2 = state.freeze(&marked(2), true);
    state.flushed(c2);
    review_m24e_write(state, disk, 81, 81);
    let c3 = state.freeze(&marked(3), true);
    review_m24e_write(state, disk, 113, 113);
    assert_eq!(state.held().iter().map(|c| c.lo).collect::<Vec<_>>(), [0, 1, 2]);
    assert_eq!(state.abandon_all(), [c1, c2, c3]);
    let c4 = state.freeze(&marked(4), true);
    state.flushed(c4);
    assert!(state.is_whole_cut(c4));
    for (block, want) in [(1, 11), (33, 33), (81, 81), (113, 113), (2, 0), (114, 0)] {
        assert_eq!(review_m24e_content(state, disk, c4, block), Some(want), "block {block}");
    }
}

/// R3. A death at every store of a mismatch while the set holds: the
/// next instance takes the state up, the uploader's check finds the
/// mismatch again and lets every held cut go, and the next cut reads each
/// write into a hole as written.
#[test]
fn review_m24e_r3_a_death_at_any_store_of_a_mismatch_while_the_set_holds() {
    quiet_deaths();
    let mut deaths = 0;
    for death in 0.. {
        let mut world = run_in(true, &[], 0, 64, None).expect("runs");
        let bound = 64 * BLOCK_SIZE;
        {
            let (state, disk) = (&mut world.state, &mut world.disk);
            review_m24e_write(state, disk, 1, 11);
            let c1 = state.freeze(&marked(1), true);
            state.flushed(c1);
            review_m24e_write(state, disk, 33, 33);
            let c2 = state.freeze(&marked(2), true);
            state.flushed(c2);
            review_m24e_write(state, disk, 81, 81);
        }
        world.region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| world.state.abandon_all()));
        world.region.revive();
        if done.is_ok() {
            break;
        }
        deaths += 1;
        let mut state = CutState::restore(world.region.clone(), SIZE, bound, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("taken up");
        let _ = state.abandon_all();
        assert!(state.held().is_empty(), "death {death}");
        let n = state.freeze(&marked(9), true);
        state.flushed(n);
        assert!(state.is_whole_cut(n), "death {death}");
        for (block, want) in [(1, 11), (33, 33), (81, 81), (2, 0), (82, 0), (16, DATA + 16)] {
            assert_eq!(review_m24e_content(&state, &world.disk, n, block), Some(want), "death {death}, block {block}");
        }
    }
    assert!(deaths > 3, "{deaths}");
}

/// R4. The cut being uploaded cannot be read (abandon_uploading) while the
/// set holds and a cut waits: the waiting cut takes its map with `lo` 0
/// and is whole. A hole written between the two instants reads as
/// written in it; a hole written after its instant makes no copy and
/// reads as zeros; a hole written before cut 1 reads as written.
#[test]
fn review_m24e_r4_a_read_failure_of_the_first_cut_gives_its_holes_to_the_cut_that_waits() {
    let mut world = run_in(true, &[], 0, 64, None).expect("runs");
    let (state, disk) = (&mut world.state, &mut world.disk);
    review_m24e_write(state, disk, 1, 11);
    let c1 = state.freeze(&marked(1), true);
    state.flushed(c1);
    review_m24e_write(state, disk, 33, 33);
    let c2 = state.freeze(&marked(2), true);
    state.flushed(c2);
    assert!(state.abandon_uploading(c1));
    assert_eq!(state.held().first().map(|c| (c.number, c.lo)), Some((c2, 0)));
    assert!(state.is_whole_cut(c2));
    assert_eq!(state.figures().holes, Holes::Held);
    let copied = state.figures().copied;
    review_m24e_write(state, disk, 34, 34);
    assert_eq!(state.figures().copied, copied, "a copy for a hole nothing wrote");
    for (block, want) in [(1, 11), (33, 33), (34, 0), (2, 0)] {
        assert_eq!(review_m24e_content(state, disk, c2, block), Some(want), "block {block}");
    }
    // Its slices: every block once, the written holes read, the rest of
    // their chunks in runs of zeros.
    let mut from = 0;
    let (mut read, mut zeros) = (Vec::new(), Vec::new());
    loop {
        let slice = state.slice_from(c2, from, 5).expect("held");
        read.extend(slice.blocks.iter().copied());
        zeros.extend(slice.zeroes.iter().copied());
        match slice.next {
            Some(next) => from = next,
            None => break,
        }
    }
    assert!(read.contains(&1) && read.contains(&33) && !read.contains(&34), "{read:?}");
    let mut all: Vec<u64> = read.clone();
    for (first, n) in &zeros {
        all.extend(*first..first + n);
    }
    all.sort_unstable();
    assert_eq!(all, (0..BLOCKS).collect::<Vec<_>>());
}

/// R5. A slice begun in the middle of a run of holes (an upload that goes
/// on from where a slice ended) names the run from there, and one begun
/// on a block of the map inside a hole chunk reads it first.
#[test]
fn review_m24e_r5_a_slice_begun_inside_a_run_of_holes() {
    let mut world = run_in(true, &[], 0, 64, None).expect("runs");
    let (state, disk) = (&mut world.state, &mut world.disk);
    review_m24e_write(state, disk, 40, 40);
    let c1 = state.freeze(&marked(1), true);
    state.flushed(c1);
    let slice = state.slice_from(c1, 35, 64).expect("held");
    assert_eq!(slice.zeroes.first(), Some(&(35, 5)), "{slice:?}");
    assert_eq!(slice.blocks.first(), Some(&40), "{slice:?}");
    assert_eq!(slice.zeroes.get(1), Some(&(41, 23)), "{slice:?}");
    let slice = state.slice_from(c1, 40, 1).expect("held");
    assert_eq!(slice.blocks, [40], "{slice:?}");
    assert_eq!(slice.next, Some(41));
    let slice = state.slice_from(c1, 195, 64).expect("held");
    assert_eq!((slice.blocks.clone(), slice.zeroes.clone(), slice.next), (vec![], vec![(195, 5)], None));
}

/// R6. A mismatch while the set holds and no cut is held yet (the first
/// cut was abandoned at a copy that could not be made, its map in the
/// open map with `lo` 0): the open epoch is marked whole, and the next
/// cut reads the holes written before as written.
#[test]
fn review_m24e_r6_abandon_all_with_no_cut_held_keeps_the_open_map() {
    let mut world = run_in(true, &[], 0, 64, None).expect("runs");
    let (state, disk) = (&mut world.state, &mut world.disk);
    review_m24e_write(state, disk, 1, 11);
    assert!(state.abandon_all().is_empty());
    review_m24e_write(state, disk, 33, 33);
    let c1 = state.freeze(&marked(1), true);
    state.flushed(c1);
    assert!(state.is_whole_cut(c1));
    for (block, want) in [(1, 11), (33, 33), (2, 0)] {
        assert_eq!(review_m24e_content(state, disk, c1, block), Some(want), "block {block}");
    }
}

/// A copier for writes that make no copy (no cut held yet).
struct ReviewNoCopies;

impl Copier for ReviewNoCopies {
    fn read_image(&mut self, _: u64, _: &mut [u8]) -> io::Result<()> {
        Err(io::Error::other("no copy is expected"))
    }

    fn write_copies(&mut self, _: u32, _: &[u8]) -> io::Result<()> {
        Err(io::Error::other("no copy is expected"))
    }
}

/// R7 (Codex's P2 on slice_from, measured). A volume that was one hole
/// when the state began (a new image), 2048 blocks written at its start,
/// one a chunk, then the first cut: its slices of 256 blocks name a run
/// of 15 blocks between each two written blocks, and each run's end is
/// looked for to the end of the volume's one run of holes, twice. The
/// time grows with the volume's size for the same 2048 writes. Prints
/// the times; asserts only that the slices cover the volume.
#[test]
fn review_m24e_r7_slicing_a_cut_over_one_long_run_of_holes() {
    for gib in [16u64, 64, 256] {
        let blocks = (gib << 30) / BLOCK_SIZE;
        let size = blocks * BLOCK_SIZE;
        let bound = (size / 256).max(256 << 20);
        let region = MemRegion::new(words_for(blocks, records_for(bound)));
        let mut state = CutState::begin_with(region, size, bound, OWNER, Mutants(0), None, Some(&[(0, blocks)]));
        for i in 0..2048u64 {
            state.change(i * HOLE_BLOCKS, 1, &mut ReviewNoCopies).expect("no copy");
        }
        let number = state.freeze(&marked(1), true);
        let all = std::time::Instant::now();
        let (mut from, mut slices, mut worst, mut read, mut zeros) = (0, 0, std::time::Duration::ZERO, 0u64, 0u64);
        loop {
            let one = std::time::Instant::now();
            let slice = state.slice_from(number, from, 256).expect("held");
            worst = worst.max(one.elapsed());
            slices += 1;
            read += slice.blocks.len() as u64;
            zeros += slice.zeroes.iter().map(|&(_, n)| n).sum::<u64>();
            match slice.next {
                Some(next) => from = next,
                None => break,
            }
        }
        assert_eq!((read, read + zeros), (2048, blocks));
        eprintln!(
            "R7: {gib} GiB, 2048 blocks written at the start: {slices} slices in {:?}, the slowest {:?}",
            all.elapsed(),
            worst
        );
    }
}

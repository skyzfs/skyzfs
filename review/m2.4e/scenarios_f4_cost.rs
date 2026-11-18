
// ---- The review's scenarios for the third next look at #125 (F4), the
// cost of slicing. Appended to `state/tests.rs` for a run; they build at
// `af7b524` and at `d71869a` alike, so that the cost is measured with
// the same code before and after. Sizes come from REVIEW_SIZES (blocks,
// as powers of two, comma separated), so that a release run can go large.

fn review_sizes(default: &[u32]) -> Vec<u64> {
    std::env::var("REVIEW_SIZES")
        .ok()
        .map(|s| s.split(',').map(|p| 1u64 << p.trim().parse::<u32>().expect("a power")).collect())
        .unwrap_or_else(|| default.iter().map(|&p| 1u64 << p).collect())
}

/// All the slices of cut `number` from block 0, `limit` blocks a slice:
/// (loads, slices, runs, blocks read, the slowest slice, all of them).
fn review_slice_all(
    region: &MemRegion,
    state: &CutState<MemRegion>,
    number: u64,
    limit: usize,
) -> (u64, u64, u64, u64, std::time::Duration, std::time::Duration) {
    let before = region.loads();
    let (mut from, mut slices, mut runs, mut blocks, mut zeroes) = (Some(0), 0, 0, 0, 0);
    let mut slowest = std::time::Duration::ZERO;
    let started = std::time::Instant::now();
    while let Some(at) = from {
        let t = std::time::Instant::now();
        let slice = state.slice_from(number, at, limit).expect("held");
        slowest = slowest.max(t.elapsed());
        slices += 1;
        runs += slice.zeroes.len() as u64;
        zeroes += slice.zeroes.iter().map(|&(_, n)| n).sum::<u64>();
        blocks += slice.blocks.len() as u64;
        from = slice.next;
    }
    let all = started.elapsed();
    assert_eq!(zeroes + blocks, state.blocks);
    (region.loads() - before, slices, runs, blocks, slowest, all)
}

/// R9 (next look 3). Codex's scenario: a chunk of data in every MiB,
/// nothing written before the first cut, slices of 64 blocks. "Passes"
/// is the loads over one pass over the open map's summary and the hole
/// set.
#[test]
fn review_m24e_r9_sparse_slicing_cost() {
    const MIB: u64 = (1 << 20) / BLOCK_SIZE;
    for large in review_sizes(&[18, 20, 22, 24]) {
        let bound = 64 * BLOCK_SIZE;
        let holes: Vec<(u64, u64)> = (0..large / MIB)
            .map(|mib| (mib * MIB + HOLE_BLOCKS, MIB - HOLE_BLOCKS))
            .collect();
        let region = MemRegion::new(words_for(large, records_for(bound)));
        let mut state = CutState::begin_with(
            region.clone(),
            large * BLOCK_SIZE,
            bound,
            OWNER,
            Mutants(0),
            None,
            Some(&holes),
        );
        let number = state.freeze(&marked(1), true);
        state.flushed(number);
        let (loads, slices, runs, blocks, slowest, all) = review_slice_all(&region, &state, number, 64);
        let pass = large / 64 / 64 + hole_words(large) as u64;
        eprintln!(
            "R9 {} GiB: slices {slices} runs {runs} blocks {blocks} loads {loads} passes {:.1} all {:?} slowest {:?}",
            large * BLOCK_SIZE >> 30,
            loads as f64 / pass as f64,
            all,
            slowest
        );
    }
}

/// R13 (next look 3). One long run: a volume that is one hole with
/// nothing written (a new, empty volume), and the same with one block
/// written every `apart` blocks, `apart` a few stretches of 4096, so
/// that each run spans stretches. Slices of 64 blocks.
#[test]
fn review_m24e_r13_long_run_cost() {
    for large in review_sizes(&[20, 22, 24]) {
        for apart in [None, Some(3 * 4096 + 5), Some(64 * 4096 + 77)] {
            let bound = 64 * BLOCK_SIZE;
            let region = MemRegion::new(words_for(large, records_for(bound)));
            let mut state = CutState::begin_with(
                region.clone(),
                large * BLOCK_SIZE,
                bound,
                OWNER,
                Mutants(0),
                None,
                Some(&[(0, large)]),
            );
            if let Some(apart) = apart {
                for block in (apart / 2..large).step_by(apart as usize) {
                    state.change(block, 1, &mut NoCopies).expect("the change");
                }
            }
            let number = state.freeze(&marked(1), true);
            state.flushed(number);
            let (loads, slices, runs, blocks, slowest, all) = review_slice_all(&region, &state, number, 64);
            let pass = large / 64 / 64 + hole_words(large) as u64;
            eprintln!(
                "R13 {} GiB apart {apart:?}: slices {slices} runs {runs} blocks {blocks} loads {loads} passes {:.2} all {:?} slowest {:?}",
                large * BLOCK_SIZE >> 30,
                loads as f64 / pass as f64,
                all,
                slowest
            );
            if std::env::var("REVIEW_BOUND").is_ok() {
                assert!(loads <= 4 * pass + 16 * (runs + blocks), "R13: {loads} loads, a pass is {pass}");
            }
        }
    }
}

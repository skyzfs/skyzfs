
// ---- The review's scenarios for the third next look at #125 (F4), on
// the behaviour of the stretch look. Appended to `state/tests.rs` at
// `d71869a` (they use `Maps::first_in` and `RUN_LOOK`).

fn review_rng(mut seed: u64) -> impl FnMut() -> u64 {
    move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    }
}

/// `slice_from` as it was at `af7b524`, word for word but for the
/// receiver: a run ends at the first of the end of its hole run and the
/// next block of the cut's map, looked for with `blocks_from` to the
/// volume's end once a slice. The reference for R10 and R11.
fn review_old_slice_from(
    state: &CutState<MemRegion>,
    number: u64,
    from: u64,
    limit: usize,
) -> Option<Slice> {
    let cut = state.hold(number)?;
    if !state.hole_reader(&cut) {
        let (blocks, next) = state.map_from(number, from, limit)?;
        return Some(Slice {
            blocks,
            zeroes: Vec::new(),
            next,
        });
    }
    let maps: Vec<usize> = std::iter::once(cut.parts.first)
        .chain(cut.parts.also)
        .collect();
    let limit = limit.max(1);
    let mut block = from.min(state.blocks);
    let mut slice = Slice::default();
    let mut mapped: Option<Option<u64>> = None;
    while block < state.blocks && slice.blocks.len() < limit && slice.zeroes.len() < SLICE_ZERO_RUNS {
        if !state.hole_of(&cut.cut, block) || state.in_parts(cut.parts, block) {
            slice.blocks.push(block);
            block += 1;
            continue;
        }
        let next_mapped = match mapped {
            Some(Some(next)) if next > block => Some(next),
            Some(None) => None,
            _ => {
                let (found, _) = state.maps.blocks_from(&state.region, &maps, block, 1);
                let next = found.first().copied();
                mapped = Some(next);
                next
            }
        };
        let end = state.hole_run_end(block, next_mapped.unwrap_or(state.blocks));
        slice.zeroes.push((block, end - block));
        block = end;
    }
    slice.next = (block < state.blocks).then_some(block);
    Some(slice)
}

/// The slice a greedy reading of `zero` (a block reads `Zero`) gives.
fn review_greedy(zero: &[bool], from: u64, limit: usize) -> Slice {
    let blocks = zero.len() as u64;
    let limit = limit.max(1);
    let mut want = Slice::default();
    let mut block = from.min(blocks);
    while block < blocks && want.blocks.len() < limit && want.zeroes.len() < SLICE_ZERO_RUNS {
        if !zero[block as usize] {
            want.blocks.push(block);
            block += 1;
            continue;
        }
        let mut end = block;
        while end < blocks && zero[end as usize] {
            end += 1;
        }
        want.zeroes.push((block, end - block));
        block = end;
    }
    want.next = (block < blocks).then_some(block);
    want
}

/// Every slice of `number` from `from` on: new against old and greedy.
fn review_compare_all(state: &CutState<MemRegion>, number: u64, zero: &[bool], from: u64, limit: usize, what: &str) -> u64 {
    let mut at = from;
    let mut slices = 0;
    loop {
        let got = state.slice_from(number, at, limit).expect("held");
        let old = review_old_slice_from(state, number, at, limit).expect("held");
        let want = review_greedy(zero, at, limit);
        assert_eq!(got, old, "{what}: new against old, from {at}, limit {limit}");
        assert_eq!(got, want, "{what}: new against greedy, from {at}, limit {limit}");
        slices += 1;
        match got.next {
            Some(n) => at = n,
            None => return slices,
        }
    }
}

/// Build a whole first cut over `blocks` with `holes`, `writes` written
/// before the instant and `after` (blocks that read `Zero`) written
/// after it; give back the state, the cut and what reads `Zero`.
fn review_world(
    blocks: u64,
    holes: &[(u64, u64)],
    writes: &[(u64, u64)],
    after: &[u64],
) -> (CutState<MemRegion>, u64, Vec<bool>) {
    let bound = 64 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(blocks, records_for(bound)));
    let mut state = CutState::begin_with(region, blocks * BLOCK_SIZE, bound, OWNER, Mutants(0), None, Some(holes));
    for &(first, count) in writes {
        state.change(first, count, &mut NoCopies).expect("no copy before the cut");
    }
    let number = state.freeze(&marked(1), true);
    state.flushed(number);
    let zero: Vec<bool> = (0..blocks)
        .map(|b| state.read(number, b) == Some(Read::Zero))
        .collect();
    for &block in after {
        if zero[block as usize] {
            state.change(block, 1, &mut NoCopies).expect("a write into a hole after the instant makes no copy");
        }
    }
    let again: Vec<bool> = (0..blocks)
        .map(|b| state.read(number, b) == Some(Read::Zero))
        .collect();
    assert_eq!(zero, again, "a write after the instant changes what the cut reads");
    (state, number, zero)
}

/// R10 (next look 3). The differential: on random worlds of up to six
/// stretches of `RUN_LOOK` blocks, hole runs that span stretches, map
/// blocks far apart and close, writes into holes after the instant (in
/// the open map, which a cut's look must not see), every slice of the
/// cut from a random block on is the same with the stretch look as with
/// the code at `af7b524`, and as a greedy reading of `read`.
#[test]
fn review_m24e_r10_stretch_look_equals_the_old_slices() {
    let mut next = review_rng(0x2545_f491_4f6c_dd1d);
    let (mut slices, mut stretches_crossed) = (0u64, 0u64);
    for round in 0..400 {
        let blocks = match next() % 4 {
            0 => 1 + next() % RUN_LOOK,
            1 => RUN_LOOK * (1 + next() % 6),
            2 => RUN_LOOK * (1 + next() % 6) + next() % 40,
            _ => 1 + next() % (6 * RUN_LOOK + 200),
        };
        let mut holes = Vec::new();
        if next() % 5 == 0 {
            holes.push((0, blocks));
        } else {
            let mut at = 0;
            while at < blocks {
                let len = if next() % 2 == 0 { 1 + next() % 300 } else { 1 + next() % (3 * RUN_LOOK) };
                let len = len.min(blocks - at);
                if next() % 3 != 0 {
                    holes.push((at, len));
                }
                at += len;
            }
        }
        let mut writes = Vec::new();
        match next() % 4 {
            0 => {}
            1 => {
                for _ in 0..next() % 6 {
                    writes.push((next() % blocks, 1));
                }
            }
            2 => {
                for _ in 0..next() % 200 {
                    let first = next() % blocks;
                    writes.push((first, (1 + next() % 8).min(blocks - first)));
                }
            }
            _ => {
                // Next to the stretches' edges.
                for k in 1..=6 {
                    for d in [-1i64, 0, 1] {
                        let b = (k * RUN_LOOK) as i64 + d;
                        if b >= 0 && (b as u64) < blocks && next() % 3 == 0 {
                            writes.push((b as u64, 1));
                        }
                    }
                }
            }
        }
        let after: Vec<u64> = (0..next() % 20).map(|_| next() % blocks).collect();
        let (state, number, zero) = review_world(blocks, &holes, &writes, &after);
        for (b, &z) in zero.iter().enumerate() {
            if z && (b as u64 + 1) % RUN_LOOK == 0 && zero.get(b + 1) == Some(&true) {
                stretches_crossed += 1;
            }
        }
        for _ in 0..3 {
            let limit = match next() % 3 {
                0 => 1 + (next() % 70) as usize,
                1 => 64,
                _ => usize::MAX,
            };
            let from = if next() % 2 == 0 { 0 } else { next() % (blocks + 1) };
            slices += review_compare_all(&state, number, &zero, from, limit, &format!("round {round}, blocks {blocks}"));
        }
    }
    eprintln!("R10: {slices} slices equal, {stretches_crossed} runs of zeros that cross a stretch's edge");
    assert!(stretches_crossed > 200, "{stretches_crossed}");
}

/// R11 (next look 3). The edges, enumerated: volumes of one stretch or
/// less, of whole stretches, and ending inside a chunk; a hole run that
/// ends at a stretch's edge, that crosses a word of the hole set at a
/// stretch's edge, a block of the map at either side of an edge, two
/// beside it, and none; slices from each block near an edge.
#[test]
fn review_m24e_r11_stretch_edges() {
    let mut cases = 0;
    let volumes = [
        RUN_LOOK - 1,
        RUN_LOOK,
        RUN_LOOK + 1,
        RUN_LOOK + 16,
        2 * RUN_LOOK,
        2 * RUN_LOOK + 5,
        3 * RUN_LOOK - 3,
        3 * RUN_LOOK,
    ];
    for blocks in volumes {
        let mut edges: Vec<u64> = Vec::new();
        for k in 0..=3 {
            for d in [-17i64, -16, -2, -1, 0, 1, 15, 16] {
                let b = (k * RUN_LOOK) as i64 + d;
                if b >= 0 && (b as u64) < blocks {
                    edges.push(b as u64);
                }
            }
        }
        edges.push(blocks - 1);
        edges.sort_unstable();
        edges.dedup();
        // Hole layouts: all hole; holes up to an edge then data; data in
        // the chunk after each edge; a chunk of data at each word of the
        // hole set (every 1024 blocks) but not at the stretches.
        let mut layouts: Vec<Vec<(u64, u64)>> = vec![vec![(0, blocks)]];
        layouts.push(vec![(0, RUN_LOOK.min(blocks))]);
        let mut after_edges = Vec::new();
        let mut at = 0;
        for k in 1..=3 {
            let e = k * RUN_LOOK;
            if e >= blocks {
                break;
            }
            after_edges.push((at, e - at));
            at = e + HOLE_BLOCKS;
        }
        if at < blocks {
            after_edges.push((at, blocks - at));
        }
        layouts.push(after_edges);
        let mut words = Vec::new();
        let mut at = 0;
        while at < blocks {
            let end = ((at / 1024) + 1) * 1024;
            let hole_end = if end % RUN_LOOK == 0 { end } else { end - HOLE_BLOCKS };
            let hole_end = hole_end.min(blocks);
            if hole_end > at {
                words.push((at, hole_end - at));
            }
            at = end;
        }
        layouts.push(words);
        for holes in &layouts {
            let mut write_sets: Vec<Vec<(u64, u64)>> = vec![vec![]];
            for &e in &edges {
                write_sets.push(vec![(e, 1)]);
            }
            for k in 1..=3 {
                let e = k * RUN_LOOK;
                if e < blocks {
                    write_sets.push(vec![(e - 1, 2)]);
                }
            }
            for writes in &write_sets {
                let (state, number, zero) = review_world(blocks, holes, writes, &[]);
                let mut froms = edges.clone();
                froms.push(0);
                froms.push(blocks);
                for &from in &froms {
                    for limit in [1usize, 2, 64, usize::MAX] {
                        review_compare_all(&state, number, &zero, from, limit, &format!("blocks {blocks}, holes {holes:?}, writes {writes:?}"));
                        cases += 1;
                    }
                }
            }
        }
    }
    eprintln!("R11: {cases} cases");
}

/// R12 (next look 3). `Maps::first_in` against the blocks put in the
/// maps, over random maps (one to three of them asked at once), random
/// ranges (empty, inverted, past the volume, over many summary words),
/// and a summary that names words with no bit set (as a death between a
/// summary's bit and its word's leaves it).
#[test]
fn review_m24e_r12_first_in_against_the_set() {
    let mut next = review_rng(0x9e37_79b9_7f4a_7c15);
    let mut checked = 0u64;
    for round in 0..3000 {
        let blocks = match next() % 3 {
            0 => 1 + next() % 200,
            1 => 1 + next() % (6 * RUN_LOOK),
            _ => 64 * 64 * (1 + next() % 5) + next() % 3,
        };
        let mut region = MemRegion::new(Maps::words(blocks));
        let maps = Maps::new(0, blocks, Mutants(0));
        let mut sets: Vec<BTreeSet<u64>> = vec![BTreeSet::new(); 3];
        for (map, set) in sets.iter_mut().enumerate() {
            let n = match next() % 3 {
                0 => 0,
                1 => next() % 5,
                _ => next() % 60,
            };
            for _ in 0..n {
                let b = next() % blocks;
                maps.insert(&mut region, map, b);
                set.insert(b);
            }
        }
        // A summary that names empty words.
        let bits = blocks.div_ceil(64) as usize;
        let summary = bits.div_ceil(64);
        for _ in 0..next() % 8 {
            let map = (next() % 3) as usize;
            let word = (next() % bits as u64) as usize;
            let at = map * (summary + bits) + word / 64;
            let value = region.load(at) | (1 << (word % 64));
            region.store(at, value);
        }
        for _ in 0..40 {
            let named: Vec<usize> = match next() % 4 {
                0 => vec![0],
                1 => vec![1, 2],
                2 => vec![0, 2],
                _ => vec![2, 0, 1],
            };
            let from = match next() % 4 {
                0 => next() % (blocks + 3),
                1 => (next() % (blocks / 64 + 1)) * 64,
                2 => (next() % (blocks / 4096 + 1)) * 4096,
                _ => sets[(next() % 3) as usize].iter().next().copied().unwrap_or(0),
            };
            let until = match next() % 4 {
                0 => next() % (blocks + 10),
                1 => from + next() % 130,
                2 => from + next() % (3 * RUN_LOOK),
                _ => ((from / RUN_LOOK) + 1) * RUN_LOOK,
            };
            let want = named
                .iter()
                .filter_map(|&m| sets[m].range(from..until.min(blocks).max(from)).next().copied())
                .min();
            assert_eq!(
                maps.first_in(&region, &named, from, until),
                want,
                "round {round}, blocks {blocks}, maps {named:?}, {from}..{until}"
            );
            checked += 1;
        }
    }
    eprintln!("R12: {checked} looks equal");
}

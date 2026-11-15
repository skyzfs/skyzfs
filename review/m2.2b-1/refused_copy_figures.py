#!/usr/bin/env python3
"""#100 at 59d09bf: a refused copy in a write over several blocks of the cut that goes, and the
figures after it.

    refused_copy_figures.py CHECKOUT TARGET_DIR

Three plants of `plants.py` pass the pull request's tests and the three tests of
`refused_copy.py`: A7 (the copies of the cuts let go are not freed then), A9 (the held cuts are
not read again after the cuts went) and A11 (the slot of a copy that failed is not given back).

The test here holds three cuts as `refused_copy.py` does, with a copy made for cut 3 before
(block 31 is changed once more after cut 4's instant), and then writes blocks 30 to 33 with
copies refused: 30 and 32 are in cut 3's map and have no copy yet. It checks which cuts are
held, how many went, the next cut's map, and the bytes the copies take.

Run as the head has it and under each plant. The files are restored.
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402
import refused_copy as rc  # noqa: E402

TEST = r'''
#[test]
fn review_a_refused_copy_in_a_write_over_several_blocks_of_the_cut_that_goes() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REVIEW_BOUND)));
    let (mut state, mut io) = review_three_held(&region);
    // A copy for cut 3: block 31 changes after cut 4's instant.
    state.change(31, 1, &mut io).expect("the change");
    let before = state.figures();
    io.refuse = true;
    state.change(30, 4, &mut io).expect("the write goes on");
    io.refuse = false;
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    let after = state.figures();
    println!(
        "review: held {held:?}; abandoned {} then {}; copied {} then {} blocks",
        before.abandoned,
        after.abandoned,
        before.copied / BLOCK_SIZE,
        after.copied / BLOCK_SIZE
    );
    assert_eq!(held, [2]);
    assert_eq!(after.abandoned, before.abandoned + 2, "cuts 3 and 4 went, once each");
    assert_eq!(after.copied, 0, "cut 2 has no copy, and the cuts that had one are gone");
    let five = state.freeze(&marked(5), true);
    let map = state.map(five);
    println!("review: cut {five}'s map: {map:?}");
    for block in [30, 31, 32, 33, 40, 50, 100, 170] {
        assert!(map.contains(&block), "cut {five} lacks block {block}: {map:?}");
    }
    // The room under the bound is whole again: as many copies as it
    // has slots can be made for cut 5.
    state.flushed(five);
    let slots = records_for(REVIEW_BOUND) as u64;
    let mut made = 0;
    for block in [30, 31, 32, 33, 40, 50, 100, 170] {
        state.change(block, 1, &mut io).expect("the change");
        made += 1;
    }
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    println!(
        "review: {made} blocks of cut {five} changed, {slots} slots: held {held:?}, copied {} blocks",
        state.figures().copied / BLOCK_SIZE
    );
    assert_eq!(state.figures().copied, made * BLOCK_SIZE);
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'review_a_refused_copy_in_a_write_over',
       '--nocapture', '--test-threads=1']
NAMES = ['A7', 'A9', 'A11']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=rc.TARGET)
    rc.RUN = RUN
    state, tests = os.path.join(rc.ROOT, rc.STATE), os.path.join(rc.ROOT, rc.TESTS)
    orig_state, orig_tests = open(state).read(), open(tests).read()
    chosen = [('as the head has it', rc.STATE, None, None)]
    chosen += [p for p in plants.PLANTS if p[0].split()[0] in NAMES]
    assert len(chosen) == len(NAMES) + 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + rc.TEST + TEST)
        for name, path, old, new in chosen:
            assert path == rc.STATE, (name, path)
            print(name + ':', flush=True)
            if old:
                assert orig_state.count(old) == 1, name
                with open(state, 'w') as f:
                    f.write(orig_state.replace(old, new))
            rc.run(env)
            with open(state, 'w') as f:
                f.write(orig_state)
    finally:
        with open(state, 'w') as f:
            f.write(orig_state)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()

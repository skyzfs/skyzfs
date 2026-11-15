#!/usr/bin/env python3
"""#100 at 59d09bf: a copy that cannot be made, with three cuts held, and a death inside it.

    refused_copy.py CHECKOUT TARGET_DIR

The pull request's two tests of a refused copy hold two cuts, and the cut that reads the
block is the newest held each time; the model test in state/tests.rs never refuses a copy. So
nothing there shows that the cuts newer than the one that reads the block go too, that they go
newest first, or that the open map is read again afterwards.

The tests here drive the cut state directly. Cut 1 is let go as uploaded. Then cut 2 is being
uploaded (block 20), cut 3 waits (blocks 30 to 32, 100 and 170), cut 4's flush is under way (block 40), and
block 50 is in the open map.

1. Copies are refused and blocks 32 and 33 are changed in one write. Cut 3 reads block 32, so
   cuts 3 and 4 go and cut 2 stays; the next cut's map holds 30 to 33, 40, 50, 100 and 170.
2. The same with a death at each store of that change. After the start, cut 4 is never held
   without cut 3's blocks, and every block is in a cut after cut 2.
3. Copies are refused and block 20 is changed: every cut goes, and the next cut's `lo` is 1,
   the last cut let go as uploaded.

Run as the head has it, then under plants A1, A2 and A8 of plants.py, which pass the pull
request's tests. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
STATE = 'crates/skyzfs-local/src/inplace/state.rs'
TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'
TEST = r'''
/// Review: a disk that refuses copies while told to.
struct ReviewRefusing {
    disk: Disk,
    refuse: bool,
}

impl Copier for ReviewRefusing {
    fn read_image(&mut self, block: u64, buf: &mut [u8]) -> io::Result<()> {
        self.disk.read_image(block, buf)
    }

    fn write_copy(&mut self, slot: u32, buf: &[u8]) -> io::Result<()> {
        if self.refuse {
            return Err(io::Error::other("no space left"));
        }
        self.disk.write_copy(slot, buf)
    }
}

const REVIEW_BOUND: u64 = 64 * BLOCK_SIZE;

/// Review: cut 1 was let go as uploaded; cut 2 is being uploaded (block
/// 20), cut 3 waits (blocks 30 to 32), cut 4's flush is under way
/// (block 40), and block 50 is in the open map.
fn review_three_held(region: &MemRegion) -> (CutState<MemRegion>, ReviewRefusing) {
    let mut state = CutState::begin(region.clone(), SIZE, REVIEW_BOUND, OWNER, Mutants(0));
    let mut io = ReviewRefusing {
        disk: Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        },
        refuse: false,
    };
    state.change(10, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    assert!(state.release(1));
    state.change(20, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    state.change(30, 3, &mut io).expect("the change");
    // Two more blocks in other words of the map, so that cut 3's map
    // weighs more than the open map that takes it.
    state.change(100, 1, &mut io).expect("the change");
    state.change(170, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(3), true), 3);
    state.flushed(3);
    state.change(40, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(4), true), 4);
    state.change(50, 1, &mut io).expect("the change");
    let held: Vec<(u64, Phase)> = state.held().iter().map(|c| (c.number, c.phase)).collect();
    assert_eq!(
        held,
        [(2, Phase::Uploading), (3, Phase::Waiting), (4, Phase::Flushing)]
    );
    (state, io)
}

#[test]
fn review_a_refused_copy_lets_go_the_cut_that_reads_it_and_every_newer_one() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REVIEW_BOUND)));
    let (mut state, mut io) = review_three_held(&region);
    io.refuse = true;
    // Block 32 is in cut 3's map; block 33 has not changed before.
    state.change(32, 2, &mut io).expect("the change goes on");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    println!("review: 1: held after the refused copy: {held:?}");
    assert_eq!(held, [2], "cut 3 read block 32, and cut 4 would once it took cut 3's map");
    io.refuse = false;
    let five = state.freeze(&marked(5), true);
    let map = state.map(five);
    println!("review: 1: cut {five}'s map: {map:?}");
    for block in [30, 31, 32, 33, 40, 50, 100, 170] {
        assert!(map.contains(&block), "cut {five} lacks block {block}: {map:?}");
    }
}

#[test]
fn review_a_death_while_a_refused_copy_lets_cuts_go() {
    quiet_deaths();
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REVIEW_BOUND)));
        let (mut state, mut io) = review_three_held(&region);
        io.refuse = true;
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.change(32, 2, &mut io)));
        region.revive();
        match done {
            Ok(_) => break,
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let mut state = CutState::restore(region.clone(), SIZE, REVIEW_BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state");
        let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        // The map never skips a cut.
        if held.contains(&4) && !held.contains(&3) {
            let map = state.map(4);
            assert!(
                map.contains(&30),
                "a death at store {death}: cut 4 is held without cut 3 and lacks its blocks: held {held:?}, cut 4's map {map:?}"
            );
        }
        // The write comes again, with copies still refused.
        state.change(32, 2, &mut io).expect("the change goes on");
        let next = state.freeze(&marked(9), true);
        let mut all = BTreeSet::new();
        for cut in state.held() {
            if cut.number > 2 {
                all.extend(state.map(cut.number));
            }
        }
        for block in [30, 31, 32, 33, 40, 50, 100, 170] {
            assert!(
                all.contains(&block),
                "a death at store {death}: block {block} is in no cut after cut 2 (the next is {next}): {all:?}"
            );
        }
    }
    println!("review: 2: a death at each of {deaths} stores");
    assert!(deaths > 0);
}

#[test]
fn review_a_refused_copy_for_the_oldest_cut_leaves_the_next_cut_its_lo() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REVIEW_BOUND)));
    let (mut state, mut io) = review_three_held(&region);
    io.refuse = true;
    // Block 20 is in cut 2's map alone: cut 2 reads it, and every cut goes.
    state.change(20, 1, &mut io).expect("the change goes on");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert!(held.is_empty(), "{held:?}");
    io.refuse = false;
    let five = state.freeze(&marked(5), true);
    let cut = state.held()[0];
    let map = state.map(five);
    println!("review: 3: cut {five}: lo {}, map {map:?}", cut.lo);
    assert_eq!(
        cut.lo, 1,
        "cut {five} holds the changes since cut 1, the last let go as uploaded"
    );
    for block in [20, 30, 31, 32, 40, 50, 100, 170] {
        assert!(map.contains(&block), "cut {five} lacks block {block}: {map:?}");
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'review_', '--nocapture', '--test-threads=1']
PLANTS = [
    ('as the head has it', None, None),
    ('A1 only the cut that reads the content goes, not the newer ones',
     "        for held in holds[from..].iter().rev() {", "        for held in holds[from..=from].iter().rev() {"),
    ('A2 the cuts go oldest first',
     "        for held in holds[from..].iter().rev() {", "        for held in holds[from..].iter() {"),
    ('A8 the open map is not read again after the cuts went',
     "                        self.abandon_needing(&holds, block);\n                        holds = self.holds();\n"
     "                        open = self.open();",
     "                        self.abandon_needing(&holds, block);\n                        holds = self.holds();"),
]


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode)
    for ln in out.splitlines():
        s = ln.strip()
        if (s.startswith(('review:', 'error')) or 'FAILED' in s and s.startswith('test ')
                or s.startswith(('cut ', 'a death at store', 'left:', 'right:'))
                or (s.startswith('test result') and ' 0 passed; 0 failed' not in s)):
            print('   ', s[:330])


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    state, tests = os.path.join(ROOT, STATE), os.path.join(ROOT, TESTS)
    orig_state, orig_tests = open(state).read(), open(tests).read()
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        for name, old, new in PLANTS:
            print(name + ':', flush=True)
            if old:
                assert orig_state.count(old) == 1, name
                with open(state, 'w') as f:
                    f.write(orig_state.replace(old, new))
            run(env)
            with open(state, 'w') as f:
                f.write(orig_state)
    finally:
        with open(state, 'w') as f:
            f.write(orig_state)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()

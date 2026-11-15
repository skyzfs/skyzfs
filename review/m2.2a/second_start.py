#!/usr/bin/env python3
"""#98 at bf369ca: two starts in a row after a death in an abandoning.

    second_start.py CHECKOUT TARGET_DIR

The start counts a cut whose abandoning a death interrupted, and clears the entry, in one
store. Plant N10 of `plants_next2.py` leaves the entry in the word, and every test passes: no
test starts twice after such a death, and the second start would count the cut again.

The test here: cut 1 counts the whole volume, the bound is two blocks, and the third change
abandons it. The daemon is lost at each store of that change, and then starts twice. Both
starts must show the same count of abandoned cuts.

Run as the head has it and under N10. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
STATE = 'crates/skyzfs-local/src/inplace/state.rs'
TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'
TEST = r'''
#[test]
fn review_a_second_start_after_a_death_in_an_abandoning_counts_the_cut_once() {
    quiet_deaths();
    let bound = 2 * BLOCK_SIZE;
    let mut deaths = 0;
    let mut counted = BTreeSet::new();
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
        let mut state = CutState::begin(region.clone(), SIZE, bound, Mutants(0));
        let mut disk = Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        };
        state.change(0, 1, &mut disk).unwrap();
        assert_eq!(state.freeze(&marked(1), true), 1);
        state.flushed(1);
        // Cut 1 counts the whole volume: each change is copied.
        state.change(1, 1, &mut disk).unwrap();
        state.change(2, 1, &mut disk).unwrap();
        assert_eq!(state.figures().abandoned, 0);
        // The third copy passes the bound: cut 1 is abandoned.
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.change(3, 1, &mut disk)));
        region.revive();
        match done {
            Ok(result) => {
                result.unwrap();
                assert_eq!(state.figures().abandoned, 1);
                break;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let first = CutState::restore(region.clone(), SIZE, bound, Mutants(0)).unwrap();
        let (held, once) = (first.held().len(), first.figures().abandoned);
        drop(first);
        let second = CutState::restore(region.clone(), SIZE, bound, Mutants(0)).unwrap();
        let twice = second.figures().abandoned;
        counted.insert((held, once));
        assert_eq!(
            once, twice,
            "a death at store {death}: {held} held; the first start counts {once} abandoned, the second {twice}"
        );
        assert_eq!(once + held as u64, 1, "a death at store {death}: cut 1 is held or counted");
    }
    println!("review: {deaths} deaths; (held, abandoned) after them: {counted:?}");
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'review_a_second_start', '--nocapture']
PLANTS = [
    ('as the head has it', None, None),
    ('N10 settle: the entry is left when its cut is counted',
     "            self.store(at::ABANDONED, self.abandoned() + u64::from(gone));",
     "            let now = self.load(at::ABANDONED) + u64::from(gone);\n            self.store(at::ABANDONED, now);"),
]


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode)
    for ln in out.splitlines():
        s = ln.strip()
        if ('review: ' in s or s.startswith(('error', 'a death at store', 'left:', 'right:'))
                or (s.startswith('test ') and 'FAILED' in s)
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

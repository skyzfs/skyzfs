#!/usr/bin/env python3
"""#100 at 59d09bf: the daemon lost at every store of `CutState::begin`.

    begin_death.py CHECKOUT TARGET_DIR

`begin` stores the magic last, so a state half begun is not read. Plant T14 of `plants.py`
stores it first, and every test of the pull request passes: none loses the daemon inside
`begin`.

The test here loses it at each store of `begin`, over an empty region and over one that holds
another boot's state, and starts again as the same owner. The state is then either not read, or
its first cut counts the whole volume as changed, as a state begun does.

Run as the head has it and under T14. The files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
STATE = 'crates/skyzfs-local/src/inplace/state.rs'
TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'
TEST = r'''
const REVIEW_BEGIN_BOUND: u64 = 64 * BLOCK_SIZE;

#[test]
fn review_a_daemon_lost_inside_begin_leaves_no_state_with_nothing_whole() {
    quiet_deaths();
    let other = Owner {
        boot: [0x77; 16],
        ..OWNER
    };
    for held_before in [false, true] {
        let mut deaths = 0;
        let mut read = 0;
        for death in 0u64.. {
            let region = MemRegion::new(words_for(BLOCKS, records_for(REVIEW_BEGIN_BOUND)));
            if held_before {
                // Another boot's state, with a cut held.
                let mut old =
                    CutState::begin(region.clone(), SIZE, REVIEW_BEGIN_BOUND, other, Mutants(0));
                let n = old.freeze(&marked(1), true);
                old.flushed(n);
            }
            region.die_at(death);
            let done = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _ = CutState::begin(region.clone(), SIZE, REVIEW_BEGIN_BOUND, OWNER, Mutants(0));
            }));
            region.revive();
            match done {
                Ok(()) => break,
                Err(payload) => {
                    if payload.downcast_ref::<Lost>().is_none() {
                        std::panic::resume_unwind(payload);
                    }
                }
            }
            deaths += 1;
            // The next start, in the same boot, of the same volume.
            let Ok(mut state) =
                CutState::restore(region.clone(), SIZE, REVIEW_BEGIN_BOUND, OWNER, Mutants(0))
            else {
                continue;
            };
            read += 1;
            let first = state.freeze(&marked(2), true);
            let counted = state.map(first).len() as u64;
            assert_eq!(
                counted, BLOCKS,
                "a death at store {death} of begin (a state held before: {held_before}): the next \
                 start reads the state, and its first cut counts {counted} of {BLOCKS} blocks as changed"
            );
        }
        println!(
            "review: a state held before: {held_before}: {deaths} deaths inside begin, the state read after {read}"
        );
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'review_a_daemon_lost_inside_begin',
       '--nocapture', '--test-threads=1']
PLANTS = [
    ('as the head has it', None, None),
    ('T14 a new state begins with the magic first',
     "        region.store(at::VERSION, VERSION);\n        region.store(at::BLOCKS, blocks);",
     "        region.store(at::MAGIC, MAGIC);\n        region.store(at::VERSION, VERSION);\n"
     "        region.store(at::BLOCKS, blocks);"),
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

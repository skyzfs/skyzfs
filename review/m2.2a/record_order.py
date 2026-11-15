#!/usr/bin/env python3
"""#98 at e905c2e: the order of a copy record's two words, and a fixed program that pins it.

    record_order.py CHECKOUT TARGET_DIR

`CutState::copy` stores a record's epochs before its block, and `free_record` clears the block
alone, so a free slot keeps the epochs of its last use. With the two stores in the other order
(plant S17 of plants.py) a death between them leaves a record that names the new block with
the old epochs. No test of the pull request fails under that order.

The program below makes the case: a slot is freed with epochs [2,4) while cut 3 is still held
(its block was not in cut 3's map), and is taken again for block 1, which cut 3's map holds
and of which cut 3 already has its own copy in a lower slot. The death view and the next start
list a block's records from the highest slot down, so cut 3 reads the newer copy.

The test is appended to state/tests.rs, run as the head has it and with the order swapped, and
the files are restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ST = 'crates/skyzfs-local/src/inplace/state.rs'
TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'
OLD = ("        self.store_record(base + 1, lo | (hi << 32));\n"
       "        self.store_record(base, block + 1);")
NEW = ("        self.store_record(base, block + 1);\n"
       "        self.store_record(base + 1, lo | (hi << 32));")
TEST = r'''
/// Review: a free slot keeps the epochs of its last use, here [2,4), which hold cut 3.
#[test]
fn review_a_death_between_a_records_two_words() {
    let mut steps = UPLOADED_FIRST.to_vec();
    steps.extend([
        // Cut 2 (being uploaded) changes blocks 0 and 1.
        Step::Write(0, 1),
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        // Cut 3 (waits) changes block 1 alone: slot 0 = (1, [2,3)).
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        // Epoch 4: slot 1 = (1, [3,4)), cut 3's own copy; slot 2 = (0, [2,4)), cut 2's.
        Step::Write(1, 1),
        Step::Write(0, 1),
        // Cut 2 goes: slots 2 and 0 are freed, slot 2 with epochs that hold cut 3.
        Step::Release,
        // Cut 4 (waits) holds blocks 0 and 1.
        Step::Cut,
        Step::Flushed,
        // Epoch 5: block 0's copy takes slot 0, block 1's takes slot 2.
        Step::Write(0, 1),
        Step::Write(1, 1),
    ]);
    let stores = stores_of(&steps, 0, 64);
    assert!(stores > 0);
    for death in 0..stores {
        if let Err(e) = run(&steps, 0, 64, Some(death)) {
            panic!("death at store {death} of {stores}: {e}");
        }
    }
}
'''
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'inplace::state::tests::review_a_death_between_a_records_two_words']


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    for ln in out.splitlines():
        if ln.startswith('test result') or 'death at store' in ln or ln.startswith('error'):
            print('   ', ln[:600])
    return p.returncode


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    st, tests = os.path.join(ROOT, ST), os.path.join(ROOT, TESTS)
    orig_st, orig_tests = open(st).read(), open(tests).read()
    assert orig_st.count(OLD) == 1
    try:
        with open(tests, 'w') as f:
            f.write(orig_tests + TEST)
        print('as the head has it (epochs, then block):')
        print('    exit', run(env))
        with open(st, 'w') as f:
            f.write(orig_st.replace(OLD, NEW))
        print('with the order swapped (block, then epochs):')
        print('    exit', run(env))
    finally:
        with open(st, 'w') as f:
            f.write(orig_st)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()

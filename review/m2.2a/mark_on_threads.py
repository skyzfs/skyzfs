#!/usr/bin/env python3
"""#98 at e905c2e: the mark's death test on the I/O threads, which is how the daemon serves.

    mark_on_threads.py CHECKOUT TARGET_DIR

`a_death_at_any_store_of_a_mark_leaves_the_group_cut_whole` runs on the caller's thread
(`ON_CALLER`). The store records the mark in three places: after a write applied on the
caller's thread, after a write-zeroes, and in `collect`, after a write the I/O threads applied
(inplace.rs:1603). The daemon serves with I/O threads, so the third is the one it uses.

Here the test's mode is one I/O thread (`OTHER_IO[0]`): the region's stores are still made on
the queue's thread, in the same order. It is run as the head has it, and with the mark of a
write the threads applied recorded late, at the next request (plants2.py Q1). The files are
restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
INP = 'crates/skyzfs-local/src/inplace.rs'
TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
MODE_OLD = "const ON_CALLER: IoMode = OTHER_IO[1];"
MODE_NEW = "const ON_CALLER: IoMode = OTHER_IO[0];"
Q1_OLD = "            if write && result.is_ok() {\n                self.cuts.applied(id);\n                self.record_cuts();"
Q1_NEW = "            if write && result.is_ok() {\n                self.cuts.applied(id);"
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--',
       'a_death_at_any_store_of_a_mark_leaves_the_group_cut_whole']


def run(env):
    p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
    out = p.stdout + p.stderr
    print('    exit', p.returncode, flush=True)
    for ln in out.splitlines():
        if (ln.startswith('error') or 'panicked at' in ln or 'death at store' in ln or 'not noticed' in ln
                or (ln.startswith('test result') and ' 0 passed; 0 failed' not in ln)):
            print('   ', ln.strip()[:300], flush=True)


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    inp, tests = os.path.join(ROOT, INP), os.path.join(ROOT, TESTS)
    orig_inp, orig_tests = open(inp).read(), open(tests).read()
    assert orig_tests.count(MODE_OLD) == 1 and orig_inp.count(Q1_OLD) == 1
    try:
        print('on the caller\'s thread, with Q1 (the pull request\'s test as it is):', flush=True)
        with open(inp, 'w') as f:
            f.write(orig_inp.replace(Q1_OLD, Q1_NEW))
        run(env)
        with open(inp, 'w') as f:
            f.write(orig_inp)
        with open(tests, 'w') as f:
            f.write(orig_tests.replace(MODE_OLD, MODE_NEW))
        print('on one I/O thread, as the head has it:', flush=True)
        run(env)
        with open(inp, 'w') as f:
            f.write(orig_inp.replace(Q1_OLD, Q1_NEW))
        print('on one I/O thread, with Q1:', flush=True)
        run(env)
    finally:
        with open(inp, 'w') as f:
            f.write(orig_inp)
        with open(tests, 'w') as f:
            f.write(orig_tests)


if __name__ == '__main__':
    main()

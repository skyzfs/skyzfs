#!/usr/bin/env python3
"""#100 at 59d09bf: write reproducers.patch from the tests of holes.py and refused_copy.py.

    make_patch.py CHECKOUT TARGET_DIR

The tests are added to a clean checkout with the `review_` prefix taken off their names, run
once, and the diff is written to reproducers.patch. At the head the three tests of a refused
copy pass and the three of holes.py fail: they are the tests for D1, D2 and D3 of the review.
The checkout is restored.
"""
import os
import re
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
HERE = os.path.dirname(os.path.abspath(__file__))
STATE_TESTS = 'crates/skyzfs-local/src/inplace/state/tests.rs'
STORE_TESTS = 'crates/skyzfs-local/src/inplace/tests/cuts.rs'
EXPECT_FAIL = {'a_start_that_holds_no_cuts_between_two_that_do',
               'an_acknowledged_failure_whose_start_is_lost_before_its_second_phase',
               'a_failed_sync_of_the_copies_heard_by_a_start_that_is_lost'}


def test_of(script):
    src = open(os.path.join(HERE, script)).read()
    return re.search(r"\nTEST = r'''(.*?)'''\n", src, re.S).group(1)


def clean(test):
    test = re.sub(r'\n    println!\(\n        "review: [^;]*;\n', '\n', test)
    test = re.sub(r'\n    println!\("review: [^\n]*\n', '\n', test)
    test = test.replace('review_', '').replace('ReviewRefusing', 'Refusing').replace('REVIEW_BOUND', 'REFUSING_BOUND')
    return test.replace('/// Review: ', '/// ')


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    status = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True).stdout
    assert not status.strip(), f'the checkout is not clean:\n{status}'
    state_add, store_add = clean(test_of('refused_copy.py')), clean(test_of('holes.py'))
    left = [ln for ln in (state_add + store_add).splitlines() if 'review' in ln.lower()]
    assert not left, left
    st, so = os.path.join(ROOT, STATE_TESTS), os.path.join(ROOT, STORE_TESTS)
    orig_st, orig_so = open(st).read(), open(so).read()
    try:
        with open(st, 'w') as f:
            f.write(orig_st + state_add)
        with open(so, 'w') as f:
            f.write(orig_so + store_add)
        fmt = subprocess.run(['cargo', 'fmt', '-p', 'skyzfs-local'], cwd=ROOT, capture_output=True, text=True, env=env)
        print('cargo fmt: exit', fmt.returncode, fmt.stderr.strip()[:200])
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'inplace::state', 'inplace::tests::cuts'],
                           cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        failed = {ln.split()[1].split('::')[-1] for ln in out.splitlines()
                  if ln.startswith('test ') and ln.rstrip().endswith('FAILED')}
        for ln in out.splitlines():
            if ln.startswith(('test result', 'error[')):
                print(ln[:200])
        print('failed at the head:', sorted(failed))
        diff = subprocess.run(['git', 'diff'], cwd=ROOT, capture_output=True, text=True).stdout
        if failed == EXPECT_FAIL:
            with open(os.path.join(HERE, 'reproducers.patch'), 'w') as f:
                f.write(diff)
            print('reproducers.patch:', len(diff.splitlines()), 'lines')
    finally:
        with open(st, 'w') as f:
            f.write(orig_st)
        with open(so, 'w') as f:
            f.write(orig_so)


if __name__ == '__main__':
    main()

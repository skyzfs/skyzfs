#!/usr/bin/env python3
"""#98 at e905c2e: write reproducers.patch from the tests of the scripts beside this file.

    make_patch.py CHECKOUT TARGET_DIR

The tests are those of record_order.py, after_start.py, flush_death.py, repeat_after_start.py
and zeroes.py, with the `review_` prefix taken off their names. They are added to a clean
checkout, run once (all must pass at the head), and the diff is written to reproducers.patch.
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


def names(script):
    src = open(os.path.join(HERE, script)).read()
    ns = {'__file__': os.path.join(HERE, script)}
    exec(src.split('\ndef ')[0].replace('ROOT, TARGET = sys.argv[1], sys.argv[2]', 'ROOT = TARGET = ""'), ns)
    return ns


def clean(test):
    test = test.replace('review_', '')
    test = re.sub(r'\n    println!\("review: [^;]*;\n', '\n', test)
    test = re.sub(r'\n        println!\(\n            "review: [^;]*;\n', '\n', test)
    test = re.sub(r'\n    println!\("review: [^\n]*\n', '\n', test)
    return test.replace('/// Review: ', '/// ')


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    status = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True).stdout
    assert not status.strip(), f'the checkout is not clean:\n{status}'
    a = names('after_start.py')
    state_add = clean(names('record_order.py')['TEST']) + clean(a['SLOT_TEST'])
    check_new = a['CHECK_NEW'].replace('review: ', '')
    store_add = (clean(names('flush_death.py')['TEST']) + clean(names('repeat_after_start.py')['TEST'])
                 + clean(names('zeroes.py')['TEST']))
    assert 'review' not in (state_add + store_add + check_new).lower(), 'a review mark is left'
    st, so = os.path.join(ROOT, STATE_TESTS), os.path.join(ROOT, STORE_TESTS)
    orig_st, orig_so = open(st).read(), open(so).read()
    assert orig_st.count(a['CHECK_OLD']) == 1
    try:
        with open(st, 'w') as f:
            f.write(orig_st.replace(a['CHECK_OLD'], check_new) + state_add)
        with open(so, 'w') as f:
            f.write(orig_so + store_add)
        fmt = subprocess.run(['cargo', 'fmt', '-p', 'skyzfs-local'], cwd=ROOT, capture_output=True, text=True, env=env)
        print('cargo fmt: exit', fmt.returncode, fmt.stderr.strip()[:200])
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'inplace::state', 'inplace::tests::cuts'],
                           cwd=ROOT, capture_output=True, text=True, env=env)
        out = p.stdout + p.stderr
        for ln in out.splitlines():
            if ln.startswith(('test result', 'error')) or ln.rstrip().endswith('FAILED'):
                print(ln[:200])
        print('the tests with the additions: exit', p.returncode)
        diff = subprocess.run(['git', 'diff'], cwd=ROOT, capture_output=True, text=True).stdout
        if p.returncode == 0:
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

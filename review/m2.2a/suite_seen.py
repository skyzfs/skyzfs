#!/usr/bin/env python3
"""#98 at bf369ca: what the cuts did beneath the conformance suite, in each of its six runs.

    suite_seen.py CHECKOUT TARGET_DIR

The test of the suite while the store holds cuts asserts that each of four things happened
(cuts, flushes with copies held, cuts abandoned, starts that took up held cuts) and prints the
figures only where one is 0. This prints them in every run, to read beside the table in the
answer to the review. The file is restored.
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
TESTS = 'crates/skyzfs-local/src/inplace/tests.rs'
OLD = "            let seen = harness::seen();\n"
NEW = "            let seen = harness::seen();\n            println!(\"review: {harness:?}: {seen:?}\");\n"
RUN = ['cargo', 'test', '-p', 'skyzfs-local', '--lib', '--', 'while_it_holds_cuts', '--nocapture']


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    path = os.path.join(ROOT, TESTS)
    orig = open(path).read()
    assert orig.count(OLD) == 1
    try:
        with open(path, 'w') as f:
            f.write(orig.replace(OLD, NEW))
        p = subprocess.run(RUN, cwd=ROOT, capture_output=True, text=True, env=env)
        print('exit', p.returncode)
        for ln in (p.stdout + p.stderr).splitlines():
            if 'review: ' in ln or ln.startswith(('test result', 'error')) and ' 0 passed; 0 failed' not in ln:
                print(ln[ln.find('review: '):][:400] if 'review: ' in ln else ln[:200])
    finally:
        with open(path, 'w') as f:
            f.write(orig)


if __name__ == '__main__':
    main()

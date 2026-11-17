#!/usr/bin/env python3
"""#110 at dcf2df3 (M2.4a): run scenarios.rs as a test file of skynbd-engine in a
checkout of the head, and print what its scenarios say. The file is copied in as
tests/zz_review_scenarios.rs and removed afterwards; a run that does not end in 15
minutes is stopped and reported so.

    scenarios.py CHECKOUT TARGET_DIR [FILE, scenarios.rs if none]
"""
import os
import re
import shutil
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
HERE = os.path.dirname(os.path.abspath(__file__))
PLACE = os.path.join(ROOT, 'crates/skynbd-engine/tests/zz_review_scenarios.rs')


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    shutil.copyfile(os.path.join(HERE, sys.argv[3] if len(sys.argv) > 3 else 'scenarios.rs'), PLACE)
    try:
        cmd = ['cargo', 'test', '-p', 'skynbd-engine', '--test', 'zz_review_scenarios', '--',
               '--nocapture', '--test-threads=1']
        try:
            p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
        except subprocess.TimeoutExpired:
            print('(did not end in 15 minutes)')
            return 2
        out = (p.stdout + p.stderr).splitlines()
        for ln in out:
            # With --nocapture a test's first line follows "test NAME ... " on one line.
            at = re.search(r'\bS[1-9] ', ln)
            if at:
                print(ln[at.start():])
            elif ln.startswith(('test result', 'error', 'thread ')) or 'panicked' in ln \
                    or ln.rstrip().endswith('FAILED'):
                print(ln)
        if p.returncode != 0 and not any(ln.startswith('test result') for ln in out):
            print('\n'.join(out[-40:]))
        return p.returncode
    finally:
        os.remove(PLACE)
        st = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True)
        print(f'git status --short in the checkout: {st.stdout.strip() or "(clean)"}')


if __name__ == '__main__':
    sys.exit(main())

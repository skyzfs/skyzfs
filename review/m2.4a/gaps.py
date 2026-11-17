#!/usr/bin/env python3
"""#110 at dcf2df3 (M2.4a): the plants that pass every test of the pull request (B2
and B3 of plants.py), each against the reviewer's scenarios (scenarios.rs), to show
that a test of that shape would catch it. Both files are restored after each.

    gaps.py CHECKOUT TARGET_DIR
"""
import os
import shutil
import subprocess
import sys

import plants

ROOT, TARGET = sys.argv[1], sys.argv[2]
HERE = os.path.dirname(os.path.abspath(__file__))
PLACE = os.path.join(ROOT, 'crates/skynbd-engine/tests/zz_review_scenarios.rs')


def scenarios(env):
    cmd = ['cargo', 'test', '-p', 'skynbd-engine', '--test', 'zz_review_scenarios', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
    except subprocess.TimeoutExpired:
        return ['(did not end in 15 minutes)'], []
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    said = [ln.strip() for ln in out if 'assertion' in ln or ln.strip().startswith(('left:', 'right:'))]
    broken = [] if failed or p.returncode == 0 else [ln for ln in out if ln.startswith('error')][:3]
    return failed, said[:6] + broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    shutil.copyfile(os.path.join(HERE, 'scenarios.rs'), PLACE)
    try:
        print(f'the scenarios as the head has it: failed = {scenarios(env)[0]}', flush=True)
        for name, path, edits in plants.PLANTS:
            if not name.startswith(('B2', 'B3')):
                continue
            full = os.path.join(ROOT, path)
            orig = open(full).read()
            text = orig
            for old, new in edits:
                assert text.count(old) == 1, name
                text = text.replace(old, new)
            try:
                open(full, 'w').write(text)
                failed, said = scenarios(env)
            finally:
                open(full, 'w').write(orig)
            print(f'{name}: the scenarios that fail = {failed}', flush=True)
            for ln in said:
                print(f'    {ln}', flush=True)
    finally:
        os.remove(PLACE)
        st = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True)
        print(f'git status --short in the checkout: {st.stdout.strip() or "(clean)"}', flush=True)


if __name__ == '__main__':
    main()

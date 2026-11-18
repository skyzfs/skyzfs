#!/usr/bin/env python3
"""#112 at 4c887b5 (M2.4b-1, the next look): the review's scenarios, run in a checkout of the head.

The three files beside this one are put into the crate (two as test modules, one
appended to the tests on real files), run with the test hooks, and taken out again.
With plant prefixes, each named plant of plants.py is planted in turn and the
scenarios run against it, to show which of them a scenario catches.

    scenarios.py CHECKOUT TARGET_DIR [PLANT_PREFIX ...]
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import plants  # noqa: E402  (reads CHECKOUT and TARGET_DIR as this script does)

ROOT, TARGET = sys.argv[1], sys.argv[2]
WHICH = tuple(sys.argv[3:])
CRATE = 'crates/skyzfs-local'
STATE = f'{CRATE}/src/inplace/state.rs'
CUTS = f'{CRATE}/src/inplace/tests/cuts.rs'
REAL = f'{CRATE}/tests/inplace_real.rs'
ADDED = [
    ('scenarios_state.rs', f'{CRATE}/src/inplace/state/review_m24b1.rs'),
    ('scenarios_store.rs', f'{CRATE}/src/inplace/tests/cuts/review_m24b1.rs'),
]
CUTS_DIR = f'{CRATE}/src/inplace/tests/cuts'


def path(rel):
    return os.path.join(ROOT, rel)


def read(rel):
    with open(path(rel)) as f:
        return f.read()


def write(rel, text):
    with open(path(rel), 'w') as f:
        f.write(text)


def run(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast',
           'review_', '--', '--nocapture', '--test-threads=1']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=900)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(ROOT) + '/target/debug/deps/'], check=False)
        print('  (did not end in 15 minutes)', flush=True)
        return
    out = (p.stdout + p.stderr).splitlines()
    keep = ('S1', 'S2', 'S3', 'T1', 'T3', 'T4', 'T5', 'T6', 'T7', 'T8', 'T9', 'R1', 'test review_', 'test inplace::', 'test result',
            'error', 'thread ')
    shown = False
    for i, line in enumerate(out):
        if line.startswith(keep) and 'filtered out; finished' not in line or 'panicked at' in line:
            print('  ' + line, flush=True)
            shown = True
            if 'panicked at' in line and i + 1 < len(out):
                print('  ' + out[i + 1][:400], flush=True)
    if not shown:
        print('  (nothing to show)\n  ' + '\n  '.join(out[-15:]), flush=True)
    passed = sum(int(line.split()[3]) for line in out if line.startswith('test result'))
    failed = sum(int(line.split()[5]) for line in out if line.startswith('test result'))
    print(f'  in all: {passed} passed, {failed} failed', flush=True)


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    originals = {rel: read(rel) for rel in (STATE, CUTS, REAL)}
    try:
        os.makedirs(path(CUTS_DIR), exist_ok=True)
        for name, rel in ADDED:
            with open(os.path.join(HERE, name)) as f:
                write(rel, f.read())
        write(STATE, originals[STATE] + '\n#[cfg(test)]\nmod review_m24b1;\n')
        write(CUTS, originals[CUTS] + '\nmod review_m24b1;\n')
        with open(os.path.join(HERE, 'scenarios_real.rs')) as f:
            write(REAL, originals[REAL] + f.read())
        if not WHICH:
            print('as the head has it:', flush=True)
            run(env)
        for name, rel, edits in plants.PLANTS:
            if not WHICH or not name.startswith(WHICH):
                continue
            before = read(rel)
            text, ok = before, True
            for old, new in edits:
                if text.count(old) != 1:
                    print(f'{name}: NOT APPLIED ({text.count(old)} matches of {old[:60]!r})', flush=True)
                    ok = False
                    break
                text = text.replace(old, new)
            if not ok:
                continue
            try:
                write(rel, text)
                print(f'with the plant {name}:', flush=True)
                run(env)
            finally:
                write(rel, before)
    finally:
        for rel, text in originals.items():
            write(rel, text)
        os.remove(path(ADDED[0][1]))
        os.remove(path(ADDED[1][1]))
        os.rmdir(path(CUTS_DIR))
    status = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True)
    print('git status --short: ' + (status.stdout.strip() or 'clean'), flush=True)


if __name__ == '__main__':
    main()

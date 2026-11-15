#!/usr/bin/env python3
"""The base of #107 (zfs-topology at 1c6af2c, M2.2c): plant D1 of plants.py as the base
has the line, to see whether the gap is older than M2.2d. One
at a time, against the tests of skyzfs-local with its test hooks. The file is restored
after each; a run that does not end in 12 minutes is stopped and reported so.

    plants.py CHECKOUT TARGET_DIR [PREFIX ...]
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ONLY = tuple(sys.argv[3:])
ST = 'crates/skyzfs-local/src/inplace/state.rs'
PLANTS = [
    ('D1 a record a dead instance made for this change is not looked for', ST,
     [("            if !done && self.wanted(&holds, block).is_some() {\n",
       "            let _ = done;\n            if self.wanted(&holds, block).is_some() {\n")]),
]


def tests(env):
    cmd = ['cargo', 'test', '-p', 'skyzfs-local', '--features', 'test-hooks', '--no-fail-fast']
    try:
        p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=720)
    except subprocess.TimeoutExpired:
        subprocess.run(['pkill', '-f', os.path.basename(TARGET) + '/debug/deps/skyzfs_local-'], check=False)
        return ['(did not end in 12 minutes)'], []
    out = (p.stdout + p.stderr).splitlines()
    failed = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
    broken = [] if failed or p.returncode == 0 else [ln for ln in out if ln.startswith('error')][:3]
    return failed, broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    if not ONLY:
        print(f'as the head has it: {tests(env)}', flush=True)
    for name, path, edits in PLANTS:
        if ONLY and not name.startswith(ONLY):
            continue
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        text, ok = orig, True
        for old, new in edits:
            if text.count(old) != 1:
                print(f'{name}: NOT APPLIED ({text.count(old)} matches of {old[:50]!r})', flush=True)
                ok = False
                break
            text = text.replace(old, new)
        if not ok:
            continue
        try:
            with open(full, 'w') as f:
                f.write(text)
            failed, broken = tests(env)
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:8])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()

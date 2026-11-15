#!/usr/bin/env python3
"""#109 at 618f536 (the intent map's default region, 4 MiB): the tests of skyzfs-local
and skyzfsd with their test hooks as the head has them, then three plants, one at a
time. The file is restored after each.

    plants.py CHECKOUT TARGET_DIR
"""
import os
import subprocess
import sys

ROOT, TARGET = sys.argv[1], sys.argv[2]
ST = 'crates/skyzfs-local/src/inplace/state.rs'
CLI = 'bins/skyzfsd/src/cli.rs'
PLANTS = [
    ('P1 a store given no region takes 64 MiB, whatever the constant says', ST,
     "hold.intent_region.unwrap_or(DEFAULT_INTENT_REGION)", "hold.intent_region.unwrap_or(64 << 20)"),
    ('P2 the constant is 64 MiB again', ST,
     "pub const DEFAULT_INTENT_REGION: u64 = 4 << 20;", "pub const DEFAULT_INTENT_REGION: u64 = 64 << 20;"),
    ('P3 the help says 64 unless given', CLI,
     "(a power of two from 1 to 1024, 4\n", "(a power of two from 1 to 1024, 64\n"),
]


def tests(env):
    failed, broken = [], []
    for pkg in ('skyzfs-local', 'skyzfsd'):
        cmd = ['cargo', 'test', '-p', pkg, '--features', 'test-hooks', '--no-fail-fast']
        try:
            p = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, env=env, timeout=1200)
        except subprocess.TimeoutExpired:
            return [f'({pkg} did not end in 20 minutes)'], []
        out = (p.stdout + p.stderr).splitlines()
        f = sorted({ln.split()[1].split('::')[-1] for ln in out if ln.startswith('test ') and ln.rstrip().endswith('FAILED')})
        failed += f
        if not f and p.returncode != 0:
            broken += [ln for ln in out if ln.startswith('error')][:3]
        passed = sum(int(ln.split()[3]) for ln in out if ln.startswith('test result:'))
        print(f'    {pkg}: {passed} passed, {len(f)} failed', flush=True)
    return failed, broken


def main():
    env = dict(os.environ, CARGO_INCREMENTAL='0', CARGO_TARGET_DIR=TARGET)
    print('as the head has it:', flush=True)
    print(f'  {tests(env)}', flush=True)
    for name, path, old, new in PLANTS:
        full = os.path.join(ROOT, path)
        orig = open(full).read()
        if orig.count(old) != 1:
            print(f'{name}: NOT APPLIED ({orig.count(old)} matches)', flush=True)
            continue
        try:
            with open(full, 'w') as f:
                f.write(orig.replace(old, new))
            failed, broken = tests(env)
            print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(failed[:6])}' if failed else
                                 'DID NOT BUILD: ' + ' | '.join(broken)[:300] if broken else 'SURVIVED'), flush=True)
        finally:
            with open(full, 'w') as f:
                f.write(orig)
    print('finished', flush=True)


if __name__ == '__main__':
    main()

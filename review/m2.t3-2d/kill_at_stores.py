#!/usr/bin/env python3
"""#115 at afd217a: what a kill -9 leaves at each store of the cut state
after a cut's line, and after the `applied` line of a marking uberblock
write, on the store itself (no kernel).

    kill_at_stores.py CHECKOUT

It appends kill_at_stores.rs (two tests) to the checkout's
crates/skyzfs-local/src/inplace/tests/cuts.rs, runs them, and restores
the file (the checkout must be clean before). The tests use the store's
own death tooling (MemRegion.die_at, state::Lost) and its watcher, which
is what writes the request log's lines in the daemon."""
import os
import subprocess
import sys

ROOT = sys.argv[1]
HERE = os.path.dirname(os.path.abspath(__file__))
T = os.path.join(ROOT, 'crates/skyzfs-local/src/inplace/tests/cuts.rs')
dirty = subprocess.run(['git', 'status', '--short'], cwd=ROOT, capture_output=True, text=True, timeout=120)
if dirty.returncode != 0 or dirty.stdout.strip():
    sys.exit(f'{ROOT} is not a clean checkout: {dirty.stdout.strip() or dirty.stderr.strip()}')
with open(T) as f:
    orig = f.read()
with open(os.path.join(HERE, 'kill_at_stores.rs')) as f:
    extra = f.read()
try:
    with open(T, 'w') as f:
        f.write(orig + extra)
    env = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target')))
    try:
        p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', 'zz_review', '--', '--nocapture',
                            '--test-threads', '1'], cwd=ROOT, env=env, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, text=True, timeout=3000)
        for ln in p.stdout.splitlines():
            if ln.startswith(('REVIEW', 'test ', 'test result')) or 'error' in ln:
                print(ln)
        print('exit', p.returncode)
    except subprocess.TimeoutExpired:
        print('cargo test did not end in 3000 s')
finally:
    with open(T, 'w') as f:
        f.write(orig)

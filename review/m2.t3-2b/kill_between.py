#!/usr/bin/env python3
"""#111 at 8210332: where a kill -9 falls between the request log's lines
and the cut state's count, on the store itself (no kernel).

    kill_between.py CHECKOUT

It appends kill_between.rs (two tests) to the checkout's
crates/skyzfs-local/src/inplace/tests/cuts.rs, runs them, and restores
the file. The tests use the store's own death tooling (MemRegion.die_at,
state::Lost) and its watcher, which is what writes the request log's
`barrier` and `cut` lines in the daemon."""
import os
import subprocess
import sys

ROOT = sys.argv[1]
HERE = os.path.dirname(os.path.abspath(__file__))
T = os.path.join(ROOT, 'crates/skyzfs-local/src/inplace/tests/cuts.rs')
with open(T) as f:
    orig = f.read()
with open(os.path.join(HERE, 'kill_between.rs')) as f:
    extra = f.read()
try:
    with open(T, 'w') as f:
        f.write(orig + extra)
    env = dict(os.environ, CARGO_TARGET_DIR=os.environ.get('CARGO_TARGET_DIR', os.path.join(ROOT, 'target')))
    p = subprocess.run(['cargo', 'test', '-p', 'skyzfs-local', '--lib', 'zz_review', '--', '--nocapture',
                        '--test-threads', '1'], cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                       text=True, timeout=3000)
    for ln in p.stdout.splitlines():
        if ln.startswith('REVIEW') or ln.startswith('test ') or ln.startswith('test result') or 'error' in ln:
            print(ln)
    print('exit', p.returncode)
finally:
    with open(T, 'w') as f:
        f.write(orig)

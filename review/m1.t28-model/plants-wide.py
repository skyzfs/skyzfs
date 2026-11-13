#!/usr/bin/env python3
"""The plants of plants.py that no unit or replay test caught, against
the integration tests of skyzfs-blk.

    plants-wide.py WORKTREE TEST[,TEST...] name...
"""
import importlib.util
import os
import re
import subprocess
import sys
import time

here = os.path.dirname(os.path.abspath(__file__))
W, tests, names = sys.argv[1], sys.argv[2].split(','), sys.argv[3:]
sys.argv = [sys.argv[0], W, '--none']          # plants.py runs nothing for an unknown name list
src = open(os.path.join(here, 'plants.py'), encoding='utf-8').read()
plants = {}
exec(src[src.index('PLANTS = {'):src.index('\n\n\ndef run(')], plants)
PLANTS = plants['PLANTS']
MODEL = os.path.join(W, 'crates/skyzfs-blk/src/model.rs')
ORIG = open(MODEL, encoding='utf-8').read()
env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(W, 'target'), CARGO_INCREMENTAL='0')
try:
    for name in names:
        old, new = PLANTS[name]
        assert ORIG.count(old) == 1, name
        open(MODEL, 'w', encoding='utf-8').write(ORIG.replace(old, new))
        for t in tests:
            t0 = time.time()
            try:
                p = subprocess.run(['cargo', 'test', '-q', '-p', 'skyzfs-blk', '--test', t], cwd=W, env=env,
                                   capture_output=True, text=True, timeout=1500)
                out = p.stdout + p.stderr
                failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', out, re.M)) |
                                set(re.findall(r'^---- (\S+) stdout ----', out, re.M)))
                said = (f'{len(failed)} fail: ' + ', '.join(failed[:6]) + (' ...' if len(failed) > 6 else '')) if failed \
                    else ('DID NOT COMPILE' if 'could not compile' in out else 'nothing fails')
            except subprocess.TimeoutExpired:
                said = 'HUNG (1500 s)'
            print(f'{name} / {t}: {said} ({time.time() - t0:.0f} s)', flush=True)
finally:
    open(MODEL, 'w', encoding='utf-8').write(ORIG)

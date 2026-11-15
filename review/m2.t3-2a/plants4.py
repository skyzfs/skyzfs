#!/usr/bin/env python3
"""#103 at b12a390: plants in `copied` (blocks counted by changed content), one at a
time, against test_cut_daemon_check.py and test_cut_daemon_main.py. The file is restored.

    plants4.py CHECKOUT
"""
import os
import subprocess
import sys

ROOT = sys.argv[1]
J = 'tests/kernel/cut_daemon_check.py'
PLANTS = [
    ('R1 a block rewritten with the content the cut holds counts as copied',
     "                    if b in held and (hs is None or hs[k] is None or hs[k] != held[b]):",
     "                    if b in held:"),
    ('R2 a block the cut holds as zeros counts under a write-zeroes',
     "                               if b in held and held[b] != ZERO}",
     "                               if b in held}"),
    ('R3 a write with no hash for a block does not count',
     "                    if b in held and (hs is None or hs[k] is None or hs[k] != held[b]):",
     "                    if b in held and hs is not None and hs[k] is not None and hs[k] != held[b]:"),
    ('R4 the hash is taken from the write\'s first block for every block',
     "hs[k] is None or hs[k] != held[b]):", "hs[0] is None or hs[0] != held[b]):"),
]
for name, old, new in PLANTS:
    full = os.path.join(ROOT, J)
    orig = open(full).read()
    if orig.count(old) != 1:
        print(f'{name}: NOT APPLIED (found {orig.count(old)} times)')
        continue
    try:
        with open(full, 'w') as f:
            f.write(orig.replace(old, new))
        failed = []
        for t in ('test_cut_daemon_check.py', 'test_cut_daemon_main.py'):
            p = subprocess.run([sys.executable, t], cwd=os.path.join(ROOT, 'tests/kernel'), capture_output=True, text=True, timeout=600)
            failed += [ln.split()[1] for ln in p.stderr.splitlines() if ln.startswith(('FAIL:', 'ERROR:'))]
        print(f'{name}: ' + (f'KILLED by {len(failed)}: {", ".join(sorted(set(failed))[:5])}' if failed else 'SURVIVED'), flush=True)
    finally:
        with open(full, 'w') as f:
            f.write(orig)

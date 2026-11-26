#!/usr/bin/env python3
"""plant2.py (review/m2.t5b/pr159), with the tests' output decoded with
replacement: a FAIL line of the CLI recorder's tests can carry the odd
tag's byte 0xe9, which stopped plant2.py at V15. An optional third
argument names the plants to run (comma-separated ids, the first word
of each). Usage: plant3.py WORKTREE PLANTS.py [IDS]"""
import os, subprocess, sys
wt, spec = sys.argv[1], sys.argv[2]
only = set(sys.argv[3].split(',')) if len(sys.argv) > 3 else None
ns = {}
exec(open(spec).read(), ns)
for pid, fn, old, new, cmd in ns['PLANTS']:
    if only is not None and pid.split()[0] not in only:
        continue
    path = os.path.join(wt, fn)
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        print(pid, 'BADPLANT', n, flush=True)
        continue
    open(path, 'w').write(text.replace(old, new))
    why = ''
    try:
        p = subprocess.run(cmd, shell=True, cwd=wt, capture_output=True, timeout=600)
        out = (p.stdout + p.stderr).decode('utf-8', 'replace')
        v = 'KILLED' if p.returncode != 0 else 'SURVIVED'
        fails = [ln for ln in out.splitlines() if ln.startswith(('FAIL', 'not ok'))]
        why = ' | '.join(fails[:2])[:300]
    except subprocess.TimeoutExpired:
        v = 'KILLED(timeout)'
    finally:
        open(path, 'w').write(text)
    print(pid, v, why, flush=True)
subprocess.run(['git', '-C', wt, 'diff', '--stat'])

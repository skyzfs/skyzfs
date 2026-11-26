#!/usr/bin/env python3
"""plant.py, and the first FAIL lines of each killed plant (to tell a
timing flake from a kill). Usage: plant2.py WORKTREE PLANTS.py"""
import os, subprocess, sys
wt, spec = sys.argv[1], sys.argv[2]
ns = {}
exec(open(spec).read(), ns)
for pid, fn, old, new, cmd in ns['PLANTS']:
    path = os.path.join(wt, fn)
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        print(pid, 'BADPLANT', n, flush=True)
        continue
    open(path, 'w').write(text.replace(old, new))
    why = ''
    try:
        p = subprocess.run(cmd, shell=True, cwd=wt, capture_output=True, text=True, timeout=600)
        v = 'KILLED' if p.returncode != 0 else 'SURVIVED'
        fails = [ln for ln in (p.stdout + p.stderr).splitlines() if ln.startswith(('FAIL', 'not ok'))]
        why = ' | '.join(fails[:2])[:300]
    except subprocess.TimeoutExpired:
        v = 'KILLED(timeout)'
    finally:
        open(path, 'w').write(text)
    print(pid, v, why, flush=True)
subprocess.run(['git', '-C', wt, 'diff', '--stat'])

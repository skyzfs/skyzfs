#!/usr/bin/env python3
"""Apply one textual plant at a time in a scratch worktree, run a test
command, restore. Usage: plant.py WORKTREE PLANTS.py"""
import os, subprocess, sys
wt, spec = sys.argv[1], sys.argv[2]
ns = {}
exec(open(spec).read(), ns)
plants = ns['PLANTS']
res = []
for pid, fn, old, new, cmd in plants:
    path = os.path.join(wt, fn)
    text = open(path).read()
    n = text.count(old)
    if n != 1:
        res.append((pid, 'BADPLANT', f'{n} matches'))
        print(pid, 'BADPLANT', n, flush=True)
        continue
    open(path, 'w').write(text.replace(old, new))
    try:
        p = subprocess.run(cmd, shell=True, cwd=wt, capture_output=True, text=True, timeout=600)
        v = 'KILLED' if p.returncode != 0 else 'SURVIVED'
    except subprocess.TimeoutExpired:
        v = 'KILLED(timeout)'
    finally:
        open(path, 'w').write(text)
    res.append((pid, v, ''))
    print(pid, v, flush=True)
subprocess.run(['git', '-C', wt, 'diff', '--stat'])

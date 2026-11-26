#!/usr/bin/env python3
"""plant3.py, printing every FAIL line (each cut to 150 characters) so
that a timing flake ahead of the planted test's own FAIL does not hide
why a plant was killed. Usage: plant4.py WORKTREE PLANTS.py [IDS]"""
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
    try:
        p = subprocess.run(cmd, shell=True, cwd=wt, capture_output=True, timeout=600)
        out = (p.stdout + p.stderr).decode('utf-8', 'replace')
        v = 'KILLED' if p.returncode != 0 else 'SURVIVED'
        fails = [ln[:150] for ln in out.splitlines() if ln.startswith(('FAIL', 'not ok'))]
    except subprocess.TimeoutExpired:
        v, fails = 'KILLED(timeout)', []
    finally:
        open(path, 'w').write(text)
    print(pid, v, len(fails), flush=True)
    for f in fails:
        print('   ', f, flush=True)
subprocess.run(['git', '-C', wt, 'diff', '--stat'])

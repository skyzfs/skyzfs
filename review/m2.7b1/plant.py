#!/usr/bin/env python3
"""Plant one textual mutant at a time in the plant worktree, run a cargo
test command, record which tests failed, and restore the file.

Usage: plant.py PLANTS.json OUT.tsv [ID ...]
PLANTS.json: [{"id":..., "file":..., "old":..., "new":..., "cmd": [...]}]
"""
import json, os, subprocess, sys, re, time
TREE = os.environ['PLANT_TREE']
plants = json.load(open(sys.argv[1]))
out = sys.argv[2]
only = set(sys.argv[3:])
env = dict(os.environ)
for p in plants:
    if only and p['id'] not in only:
        continue
    path = os.path.join(TREE, p['file'])
    text = open(path).read()
    n = text.count(p['old'])
    if n != 1:
        with open(out, 'a') as f:
            f.write(f"{p['id']}\tBAD\tmatches={n}\t\n")
        continue
    open(path, 'w').write(text.replace(p['old'], p['new']))
    t0 = time.time()
    try:
        r = subprocess.run(p['cmd'], cwd=TREE, env=env, capture_output=True, text=True, timeout=600)
        rc, so = r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired as e:
        rc, so = 'timeout', (e.stdout or b'').decode() if isinstance(e.stdout, bytes) else (e.stdout or '')
    finally:
        subprocess.run(['git', 'checkout', '--', p['file']], cwd=TREE)
    failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', so, re.M)))
    compile_err = 'error[E' in so or 'error: could not compile' in so
    verdict = 'KILLED' if (rc != 0) else 'SURVIVED'
    if compile_err:
        verdict = 'COMPILE'
    with open(out, 'a') as f:
        f.write(f"{p['id']}\t{verdict}\trc={rc}\t{int(time.time()-t0)}s\t{','.join(failed)}\n")
    open(os.path.join(os.path.dirname(out), f"plant-{p['id']}.log"), 'w').write(so[-20000:])

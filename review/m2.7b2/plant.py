#!/usr/bin/env python3
"""Plant one textual mutant at a time in the plant worktree, run a cargo
test command, record which tests failed, and restore the files.

Usage: plant.py PLANTS.json OUT.tsv [ID ...]
PLANTS.json: [{"id":..., "edits": [[file, old, new], ...], "cmd": [...]}]
Each old must occur exactly once in its file.
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
    files = sorted({f for f, _, _ in p['edits']})
    texts = {f: open(os.path.join(TREE, f)).read() for f in files}
    planted = dict(texts)
    bad = None
    for f, old, new in p['edits']:
        n = planted[f].count(old)
        if n != 1:
            bad = f'{f}: matches={n}'
            break
        planted[f] = planted[f].replace(old, new)
    if bad:
        with open(out, 'a') as fh:
            fh.write(f"{p['id']}\tBAD\t{bad}\t\n")
        continue
    for f in files:
        open(os.path.join(TREE, f), 'w').write(planted[f])
    t0 = time.time()
    try:
        r = subprocess.run(p['cmd'], cwd=TREE, env=env, capture_output=True, text=True, timeout=600)
        rc, so = r.returncode, r.stdout + r.stderr
    except subprocess.TimeoutExpired as e:
        rc = 'timeout'
        so = e.stdout.decode() if isinstance(e.stdout, bytes) else (e.stdout or '')
    finally:
        # Restore the text as it was before the plant (scratch tests
        # appended to a planted file stay).
        for f in files:
            open(os.path.join(TREE, f), 'w').write(texts[f])
    failed = sorted(set(re.findall(r'^test (\S+) \.\.\. FAILED', so, re.M)))
    compile_err = 'error[E' in so or 'error: could not compile' in so
    verdict = 'KILLED' if rc != 0 else 'SURVIVED'
    if compile_err:
        verdict = 'COMPILE'
    with open(out, 'a') as fh:
        fh.write(f"{p['id']}\t{verdict}\trc={rc}\t{int(time.time()-t0)}s\t{','.join(failed)}\n")
    open(os.path.join(os.path.dirname(out), f"plant-{p['id']}.log"), "w").write(so[-20000:])

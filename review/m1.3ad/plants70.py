#!/usr/bin/env python3
"""Defects planted at 8063ef7 (pull request 70, M1.3ad), one at a time.
For each: `the_suite_notices_the_defects` alone, RUNS times, and the
in-place store's conformance tests once. Each file is put back.

    plants70.py WORKTREE TARGET_DIR RUNS [name...]
"""
import os, re, subprocess, sys, time

W, TARGET, RUNS = sys.argv[1], sys.argv[2], int(sys.argv[3])
INP = 'crates/skyzfs-local/src/inplace.rs'
HAR = 'crates/skyzfs-local/src/inplace/harness.rs'
PLANTS = {
    'F1 finish_threads also ends the sync it began': [(INP,
        "            self.collect(Duration::from_millis(10));\n        }\n    }\n}\n",
        "            self.collect(Duration::from_millis(10));\n        }\n        self.settle(true);\n    }\n}\n")],
    'F2 fail_log holds no sync around finish_threads': [
        (HAR, "            target.disk.hold_syncs_of(HOME_NAME);\n", ""),
        (HAR, "        if settle {\n            target.disk.release_syncs();\n        }\n", "")],
    'F3 no seed settles': [(HAR, "    seed % 3 == 1\n", "    seed % 3 == 1 && seed > u64::MAX - 1\n")],
}
T = 'inplace::tests::the_suite_notices_the_defects'
CONF = 'inplace::tests::the_in_place_store_passes_the_conformance_suite'


def run():
    env = dict(os.environ, CARGO_TARGET_DIR=TARGET, CARGO_INCREMENTAL='0')
    env.pop('CI', None)
    t = time.time()
    p = subprocess.run(['cargo', 'test', '-q', '-p', 'skyzfs-local', '--lib', '--no-run', '--message-format=json'],
                       cwd=W, env=env, capture_output=True, text=True)
    exe = None
    import json
    for line in p.stdout.splitlines():
        try:
            o = json.loads(line)
        except ValueError:
            continue
        if o.get('executable') and o.get('target', {}).get('name') == 'skyzfs_local':
            exe = o['executable']
    if not exe:
        return 'DOES NOT BUILD: ' + ' | '.join(l for l in p.stderr.splitlines() if l.startswith('error'))[:300]
    ok = 0
    why = set()
    for _ in range(RUNS):
        r = subprocess.run([exe, '--exact', T], cwd=W, capture_output=True, text=True, timeout=900)
        if r.returncode == 0:
            ok += 1
        else:
            why.update(m.split(' is not noticed')[0].strip() for m in re.findall(r'the defect \S+ is not noticed by \S+', r.stdout + r.stderr))
            why.update(re.findall(r'panicked at [^\n]*\n[^\n]*', r.stdout + r.stderr)[:1] if not why else [])
    c = subprocess.run([exe, CONF], cwd=W, capture_output=True, text=True, timeout=1800)
    conf = 'conformance tests pass' if c.returncode == 0 else 'CONFORMANCE FAILS: ' + ', '.join(sorted(set(re.findall(r'^    (\S+)$', c.stdout, re.M))))
    return f'notices-the-defects alone passed {ok} of {RUNS}' + (f' (failing: {sorted(why)})' if why else '') + f'; {conf} ({time.time() - t:.0f} s)'


names = sys.argv[4:]
chosen = [n for n in PLANTS if not names or n.split()[0] in names]
if not names:
    print('unplanted:', run(), flush=True)
for name in chosen:
    saved = {}
    ok = True
    for path, old, new in PLANTS[name]:
        full = os.path.join(W, path)
        text = open(full).read()
        saved.setdefault(full, text)
        if text.count(old) != 1:
            print(f'{name}: DOES NOT APPLY ({text.count(old)} in {path})', flush=True)
            ok = False
            break
        open(full, 'w').write(text.replace(old, new))
    try:
        if ok:
            print(f'{name}: {run()}', flush=True)
    finally:
        for full, text in saved.items():
            open(full, 'w').write(text)
print('worktree after:', subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout.strip() or 'clean', flush=True)

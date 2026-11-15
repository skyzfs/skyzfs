#!/usr/bin/env python3
"""#103 at d905bd4: the judge against a daemon that makes no copy, on a real recording.

    no_copy_daemon.py JUDGE_TESTS_KERNEL_DIR RECORDING_DIR NAME DEV_BYTES OUT_DIR [first]

Takes a recording of the run (its request log and the holder's files) and rewrites the holder's
hashes as a daemon without copies would have left them: a block of a written cut that a later
write changed before the write command was sent is given that later write's content. Without
`first`, only in cuts after the first written (whose map is the whole volume), so that the few
copies of content that had been written before are what is taken away. Then runs the pull
request's judge on the changed copy (OUT_DIR) and prints its held_content line and exit.
"""
import os
import shutil
import subprocess
import sys

KDIR, RDIR, NAME, DEV, OUT = sys.argv[1:6]
FIRST = sys.argv[6:] == ['first']
sys.path.insert(0, KDIR)
import cut_daemon_check as c  # noqa: E402

if os.path.exists(OUT):
    shutil.rmtree(OUT)
shutil.copytree(RDIR, OUT)
with open(os.path.join(OUT, 'requests.tsv')) as f:
    inst = c.parse_log(f.read())[0]
with open(os.path.join(OUT, 'marks.tsv')) as f:
    marks = c.read_marks(f.read())
records, dcuts = c.records_of(inst['lines'], marks)
applied = [ln['ns'] for ln in inst['lines'] if ln['event'] == 'applied']
writes = [r for r in records if r['flags'] == [] and 'tag' in r]
for r, ns in zip(writes, applied):
    r['ns'] = ns
by_number = {int(d['number']): k for k, d in enumerate(dcuts) if d['number'] != '-'}
held = []
with open(os.path.join(OUT, 'held.tsv')) as f:
    for ln in f:
        n, _, _, _, t0, _ = ln.split('\t')
        held.append((int(n), int(t0)))
model, at, changed = {}, 0, {}
for k, (n, t0) in enumerate(held):
    d = dcuts[by_number[n]]
    while at < d['prefix']:
        c.apply(model, records[at])
        at += 1
    if k == 0 and not FIRST:
        continue
    for r in records[d['prefix']:]:
        if 'ns' not in r:
            continue
        if r['ns'] >= t0:
            break
        first = r['offset'] // c.BLOCK
        for j, h in enumerate(r.get('hashes') or []):
            if h is not None and h != model.get(first + j, c.ZERO):
                changed[(n, first + j)] = h
out, hit = [], {}
with open(os.path.join(OUT, 'held-blocks.tsv')) as f:
    for ln in f:
        n, b, h = ln.rstrip('\n').split('\t')
        key = (int(n), int(b))
        if key in changed:
            hit[key[0]] = hit.get(key[0], 0) + 1
            h = changed[key]
        out.append(f'{n}\t{b}\t{h}\n')
with open(os.path.join(OUT, 'held-blocks.tsv'), 'w') as f:
    f.write(''.join(out))
print(f'{NAME}: {sum(hit.values())} blocks given their later content, in cuts {sorted(hit)}: {hit}')
p = subprocess.run([sys.executable, '-I', os.path.join(KDIR, 'cut_daemon_check.py'), NAME, OUT,
                    os.path.join(OUT, 'requests.tsv'), '-', DEV], capture_output=True, text=True)
print('exit', p.returncode)
for ln in p.stdout.splitlines():
    if 'held_content' in ln or 'ctl_content' in ln:
        print('   ', ln[:420])

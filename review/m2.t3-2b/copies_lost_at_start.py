#!/usr/bin/env python3
"""#111 at 8210332: the judge against a daemon that takes its held cuts up at
a start without the record of their copies, on a real takeover recording.

    python3 -I copies_lost_at_start.py JUDGE_TESTS_KERNEL_DIR RECORDING_DIR OUT_DIR

Design §5.2: "A new instance that had a frozen map but not the record of a
copy would upload the overwritten block, and the cut would hold a block from
after its instant." For each cut the holder wrote beside a later instance
than the one that counted it, a block of its map that a write changed in the
home after the cut's instant and before that later instance started (so the
copy was made by an instance that is gone) is given that write's content in
the holder's hashes, as such a daemon would have written it. Then the pull
request's judge runs on the changed copy (OUT_DIR)."""
import os
import shutil
import subprocess
import sys

KDIR, RDIR, OUT = sys.argv[1:4]
sys.path.insert(0, KDIR)
import cut_daemon_check as c  # noqa: E402

if os.path.exists(OUT):
    shutil.rmtree(OUT)
shutil.copytree(RDIR, OUT)
os.remove(os.path.join(OUT, 'results.tsv'))
with open(os.path.join(OUT, 'requests.tsv')) as f:
    instances = c.parse_log(f.read())
with open(os.path.join(OUT, 'instances.tsv')) as f:
    kinds = c.read_kinds(f.read())
with open(os.path.join(OUT, 'marks.tsv')) as f:
    marks = c.read_marks(f.read())
records, dcuts = c.records_of(c.log_lines(instances, kinds), marks)
starts = [(r['i'], r['ns']) for r in records if 'instance' in r['flags']]
by_number = {int(d['number']): d for d in dcuts}
held = []
with open(os.path.join(OUT, 'held.tsv')) as f:
    for ln in f:
        n, _, _, _, t0, _ = ln.split('\t')
        held.append((int(n), int(t0)))
model, at, changed, across = {}, 0, {}, []
for n, t0 in held:
    d = by_number[n]
    while at < d['prefix']:
        c.apply(model, records[at])
        at += 1
    # the last start between the cut's instant and its write
    later = [i for i, ns in starts if i > d['prefix'] and ns <= t0]
    if not later:
        continue
    across.append(n)
    for r in records[d['prefix']:later[-1]]:
        if not c.cut_ref.is_write(r) or r.get('hashes') is None:
            continue
        first = r['offset'] // c.BLOCK
        for j, h in enumerate(r['hashes']):
            if h is not None and h != model.get(first + j, c.ZERO):
                changed[(n, first + j)] = h
out, hit = [], {}
with open(os.path.join(OUT, 'held-blocks.tsv')) as f:
    for ln in f:
        n, b, h = ln.rstrip('\n').split('\t')
        key = (int(n), int(b))
        if key in changed and changed[key] != h:
            hit[key[0]] = hit.get(key[0], 0) + 1
            h = changed[key]
        out.append(f'{n}\t{b}\t{h}\n')
with open(os.path.join(OUT, 'held-blocks.tsv'), 'w') as f:
    f.write(''.join(out))
print(f'{RDIR}: cuts written across a start: {across}; blocks whose copy an earlier instance made, given their '
      f'later content: {sum(hit.values())} ({ {n: hit.get(n, 0) for n in across} })')
p = subprocess.run([sys.executable, '-I', '-B', os.path.join(KDIR, 'cut_daemon_check.py'), 'takeover', OUT,
                    os.path.join(OUT, 'requests.tsv'), '-', str(512 << 20)], capture_output=True, text=True)
print('exit', p.returncode)
for ln in p.stdout.splitlines():
    if 'held_content' in ln or 'rig_error' in ln:
        print('   ', ln[:400])

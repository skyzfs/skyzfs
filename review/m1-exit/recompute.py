"""Recompute the M1 exit round's cells from the per-row tps in the notes (8697790)."""
import re, statistics, sys
text = open(sys.argv[1]).read()
sec = text[text.index('#### The M1 exit round'):]
rows = re.findall(r'^\| (\d+) \((\d+)\) \| ([^|]+)\| ([^|]+)\| ([^|]+)\|$', sec, re.M)
jobs = {}
for num, run, cls, order, data in rows:
    kind = cls.split(':')[0].strip()
    items = [x.strip() for x in data.split(';')]
    seq = []
    for it in items:
        m = re.match(r'([mUB]) (\d+)(?: ([\d.]+))?', it)
        seq.append((m.group(1), int(m.group(2)), float(m.group(3)) if m.group(3) else None))
    jobs[int(num)] = (kind, run, seq)
cells = {}
mismatch = []
for num, (kind, run, seq) in sorted(jobs.items()):
    models = [i for i, r in enumerate(seq) if r[0] == 'm']
    for i, (t, tps, ratio) in enumerate(seq):
        if t == 'm': continue
        lo = max(j for j in models if j < i); hi = min(j for j in models if j > i)
        a, b = seq[lo][1], seq[hi][1]; mean = (a + b) / 2
        r = tps / mean; width = abs(a - b) / mean
        if ratio is not None and abs(r - ratio) > 0.0015: mismatch.append((num, i, round(r, 4), ratio))
        if width <= 0.10:
            cells.setdefault((kind, t), []).append((round(r, 3), num))
for (kind, t), v in sorted(cells.items()):
    rs = [x for x, _ in v]; js = {n for _, n in v}
    print(f"{kind:6} {'unbounded' if t == 'U' else 'bounded':9} median {statistics.median(rs):.3f} ({len(rs)} rows / {len(js)} jobs), min {min(rs):.3f}, below 0.90: {sum(1 for x in rs if x < 0.90)}")
print("ratios that differ from the notes by more than 0.0015:", mismatch or "none")
# The same rows, classed by sync speed instead of by work disk (fast-sync: buffered sync over 1000 MiB/s).
sync = {}
for num, run, cls, order, data in rows:
    m = re.search(r'(\d+) MiB/s', cls); sync[int(num)] = int(m.group(1)) if m else None
print("sync speeds:", sync)
# By sync speed (amd64 only): fast over 1000 MiB/s, slow under.
alt = {}
for num, (kind, run, seq) in sorted(jobs.items()):
    if kind == 'arm64': continue
    k = 'fast-sync' if sync[num] > 1000 else 'slow-sync'
    models = [i for i, r in enumerate(seq) if r[0] == 'm']
    for i, (t, tps, ratio) in enumerate(seq):
        if t == 'm': continue
        lo = max(j for j in models if j < i); hi = min(j for j in models if j > i)
        a, b = seq[lo][1], seq[hi][1]; mean = (a + b) / 2
        if abs(a - b) / mean <= 0.10: alt.setdefault((k, t), []).append((tps / mean, num))
for (k, t), v in sorted(alt.items()):
    rs = [x for x, _ in v]
    print(f"by sync: {k:9} {'unbounded' if t == 'U' else 'bounded':9} median {statistics.median(rs):.3f} ({len(rs)} rows / {len({n for _, n in v})} jobs)")

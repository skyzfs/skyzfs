import collections, json, re, sys
sys.path.insert(0, sys.argv[2])   # the repository's tests/bench, not the data directory
import syscalls as sc
d = sys.argv[1]
text = open(d + '/zfs-skyzfsd-tpcb.trace', errors='replace').read()
head, _, tail = text.partition('Summary of events')
lines = [l for l in head.splitlines() if l.strip()]
un = [l for l in lines if not sc.PERF_EVENT.search(l)]
print('lines before the summary', len(lines), 'not matched', len(un))
for l in un[:6]:
    print('  UNMATCHED:', l[:160])
print('with ESC', sum('\x1b' in l for l in lines), 'continued', sum('continued' in l for l in lines),
      'no duration', sum(bool(re.search(r'\(\s+\):', l)) for l in lines))
slow = sc.perf_slow(text, 1.0)
per, names = sc.perf(text)
print('threads in summary', {t: sum(n for n, _ in c.values()) for t, c in per.items()})
print('slow', {t: dict(c) for t, c in slow.items()})
below = [l for l in lines if (m := sc.PERF_EVENT.search(l)) and float(m.group(1)) < 1.0]
print('event lines below 1 ms:', len(below))
# a summary row that an event line could be taken for, or the reverse
print('event lines taken as a summary row:', sum(bool(sc.PERF_ROW.match(l)) for l in lines),
      'as a thread line:', sum(bool(sc.PERF_THREAD.match(l)) for l in lines))
# does the summary's max agree with the slow lines: a call with max >= 1 ms must have a slow line, and one with max < 1 ms none
cur = None
rows = {}
for l in tail.splitlines():
    m = sc.PERF_THREAD.match(l)
    if m:
        cur = int(m.group(2)); continue
    f = l.split()
    if cur and len(f) >= 7 and re.match(r'^[a-z_0-9]+$', f[0]) and f[1].isdigit():
        rows[(cur, f[0])] = (int(f[1]), float(f[-3]) if len(f) >= 8 else None, l)
print(tail.splitlines()[1:14])
for (tid, call), (n, _, l) in sorted(rows.items()):
    f = l.split()
    mx = float(f[-2])
    s = slow.get(tid, {}).get(call, 0)
    flag = '' if (mx >= 1.0) == (s > 0) else '  <-- max and slow lines disagree'
    print(f"  {tid} {call:18s} calls {n:>7} max {mx:>9.3f} ms slow lines {s:>5}{flag}")
j = json.load(open(d + '/zfs-skyzfsd-tpcb.syscalls'))
print(json.dumps({k: v.get('slow') for k, v in j['threads'].items()}), j.get('slow_ms'), j['tool'])

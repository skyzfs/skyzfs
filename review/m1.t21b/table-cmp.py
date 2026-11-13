# The judge's table in summary.md: name, expected, got. Counts, rows where got != expected, and rows that differ between runs.
import collections, os, re, sys
base = sys.argv[1]
def rows(d):
    out = collections.OrderedDict()
    for line in open(os.path.join(base, d, 'summary.md'), errors='replace'):
        m = re.match(r'\| `([a-z0-9_]+)` \| ([A-Z_]+) \| ([A-Z_]+) \|', line)
        if m: out[m.group(1)] = (m.group(2), m.group(3))
    return out
runs = {d: rows(d) for d in sys.argv[2:]}
for d, r in runs.items():
    got = collections.Counter(g for _, g in r.values())
    off = [(k, e, g) for k, (e, g) in r.items() if e != g]
    head = open(os.path.join(base, d, 'summary.md'), errors='replace').read().split('\n')[4][:110]
    print(f'{d}: {len(r)} rows; got {dict(got)}; rows where got is not expected: {off}; says: {head}')
names = list(runs)
for i in range(len(names)):
    for j in range(i + 1, len(names)):
        a, b = runs[names[i]], runs[names[j]]
        d = [(k, a.get(k), b.get(k)) for k in sorted(set(a) | set(b)) if a.get(k) != b.get(k)]
        print(f'{names[i]} against {names[j]}: {"no row differs" if not d else d}')

# Compare the verdict of every line of the daemon rig's results.tsv between runs.
import collections, os, re, sys
base = sys.argv[1]
def load(d):
    lines = collections.OrderedDict()
    for line in open(os.path.join(base, d, 'results.tsv'), errors='replace'):
        k, _, v = line.rstrip('\n').partition('\t')
        lines.setdefault(k, []).append(v)
    return lines
def verdict(v):
    m = re.match(r'(PASS|FAIL|NA|WAITING|ERROR|WEAKER|NOT_EXERCISED|SKIP)\b', v)
    return m.group(1) if m else None
runs = {d: load(d) for d in sys.argv[2:]}
for d, lines in runs.items():
    c = collections.Counter()
    for k, vs in lines.items():
        if k.startswith(('pin_', 'ctl_')):
            for v in vs: c[verdict(v) or 'other'] += 1
    kinds = collections.Counter(k.split('_')[0] for k in lines)
    print(d, 'pins and controls:', dict(c), '; keys by prefix:', dict(kinds), '; kernel', lines.get('kernel'), 'zfs_userland', lines.get('zfs_userland'))
pairs = [(sys.argv[i], sys.argv[j]) for i in range(2, len(sys.argv)) for j in range(i + 1, len(sys.argv))]
for a, b in pairs:
    A, Bm = runs[a], runs[b]
    if A.get('impl', [''])[0].split(':')[0] != Bm.get('impl', [''])[0].split(':')[0]: continue
    only = [k for k in A if k not in Bm], [k for k in Bm if k not in A]
    diff = []
    for k in A:
        if k in Bm and k.startswith(('pin_', 'ctl_', 'c_', 'declared', 'case')):
            va, vb = [verdict(v) for v in A[k]], [verdict(v) for v in Bm[k]]
            if va != vb: diff.append((k, va, vb))
    print(f'{a} against {b}: keys only in the first {only[0]}, only in the second {only[1]}; verdicts that differ: {diff if diff else "none"}')

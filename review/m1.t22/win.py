import json, sys
MB = 1 << 20
d = sys.argv[1]
def load(p):
    out = {}
    for line in open(p, errors='replace'):
        f = line.split()
        if len(f) > 2 and f[0] == 'mem' and f[2].isdigit():
            out[f[1]] = int(f[2])
    return out
b, a = load(d + '/zfs-skyzfsd-tpcb.before'), load(d + '/zfs-skyzfsd-tpcb.after')
j = json.load(open(d + '/zfs-skyzfsd-tpcb.json'))
m = j['diag']['mem_mb']
bad = 0
for k in sorted(a):
    if '.stat.' in k or '.events.' in k:
        mine = (a[k] - b[k]) / MB
        exact = a[k] % MB == 0 and b[k] % MB == 0
        theirs = m.get(k + '.window')
        since = m.get(k + '.since_start')
        ok = theirs == mine and since == a[k] / MB and exact and k not in m
        bad += not ok
        print(f"{k:48s} before {b[k] // MB:>9} after {a[k] // MB:>9} window {mine:>9.0f} json {theirs} {'ok' if ok else 'DIFFERS'}")
txn = j['pg']['xact_commit']
print('xact_commit', txn, 'tps', j.get('tps'), 'duration', j.get('duration_s'), 'tps*dur', (j.get('tps') or 0) * j.get('duration_s'))
for k in ('pgscan', 'pgscan_direct', 'pgsteal', 'pgsteal_direct', 'workingset_refault_file'):
    print(k, 'per ktxn', round(m[f'skz.cgroup.stat.{k}.window'] / (txn / 1000), 1))
print('differs:', bad)

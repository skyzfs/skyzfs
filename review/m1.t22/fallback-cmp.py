# Run the new head's row program on a row recorded before the change (its
# .pressure look has no cgroup lines) and compare mem_mb with the row's own JSON.
import json, os, subprocess, sys
row, here, d = sys.argv[1], sys.argv[2], sys.argv[3]
for name in sorted(os.listdir(d)):
    if not name.endswith('-tpcb.json') and not name.endswith('.json'): continue
    base = os.path.join(d, name[:-5])
    if not os.path.exists(base + '.before'): continue
    old = json.load(open(base + '.json'))
    topo, wl = old.get('topology'), old.get('workload')
    p = subprocess.run([sys.executable, '-I', row, base, str(topo), str(wl), str(old.get('duration_s', 60)), 'none', '0',
                        base + '.nolog', base + '.nofsync', 'x', str(old.get('clients', 8)), '0', here],
                       capture_output=True, text=True, timeout=120)
    if p.returncode:
        print(name, 'row program failed:', p.stderr[-300:]); continue
    new = json.loads(p.stdout)
    a, b = old.get('diag', {}).get('mem_mb', {}), new.get('diag', {}).get('mem_mb', {})
    diff = {k: (a.get(k), b.get(k)) for k in sorted(set(a) | set(b)) if a.get(k) != b.get(k)}
    has = any(l.startswith('mem skz.cgroup.') for l in open(base + '.pressure')) if os.path.exists(base + '.pressure') else None
    print(os.path.basename(d), name, 'keys', len(a), len(b), 'group lines in .pressure:', has, 'differences:', diff if diff else 'none')

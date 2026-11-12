import json, sys
def walk(o, out):
    if isinstance(o, dict):
        if 't_read' in o: out.append(o)
        for v in o.values(): walk(v, out)
    elif isinstance(o, list):
        for v in o: walk(v, out)
for path in sys.argv[1:]:
    rows = []; walk(json.load(open(path)), rows)
    tr = sorted(r['t_read'] for r in rows); ts = sorted(r.get('t_start', 0) for r in rows)
    print(path.split('/')[-3], 'points with a read:', len(tr), 'read s: min %.2f median %.2f max %.2f' % (tr[0], tr[len(tr)//2], tr[-1]),
          '| start s: max %.2f' % ts[-1])

"""diff_seed.py NEW OLD: bracket.py's seed_in_window (#132's second head) against an oracle of
the lead's rule as relayed, and every other row against the bracket.py before the rule.

NEW and OLD are checkouts (27fe831 and 861b537). Over a cartesian table of a row's readings
(from_load window, workload, the old JSON's True, none, or no upload at all; the pids and
pid_same; the lease and its epoch at each end; `uploaded` at the start and the end;
`uploaded_whole` at the end), each row is judged between two model rows (1000 and 1000, the
row at 950) by NEW and by OLD:

- a row that is not a window row must get OLD's verdict exactly;
- a window row must be judged, with the marker "(seed inside window, lease at release (epoch
  E))", exactly when the oracle says the rule holds: both pids read and pid_same True,
  `uploaded` '0' at the start, `uploaded_whole` a number of at least 1 at the end, the lease
  held or unsure at the end with a numeric epoch, and, if held or unsure at the start, at
  the end's epoch; otherwise its verdict is OLD's, with the seed's reason appended when
  OLD's reason is the lease.

Then the accepted rows whose start is not `none` are counted by class, a sample is judged
inside a bracket 26% wide and outside any bracket (the marker must not show), and named
edges are printed. Exits 1 on any disagreement. Run with python3 -I.
"""
import importlib.util
import itertools
import sys
from collections import Counter


def load(tree, name):
    spec = importlib.util.spec_from_file_location(name, f'{tree}/tests/bench/bracket.py')
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


NEW, OLD = load(sys.argv[1], 'bk_new'), load(sys.argv[2], 'bk_old')
M = object()
HELD = ('held', 'unsure')


def digits(v):
    return isinstance(v, str) and v.isdigit()


def oracle_seed(k):
    b, a = k.get('before') or {}, k.get('after') or {}
    if (k.get('upload') or {}).get('from_load') != 'window':
        return None
    if not (b.get('pid') and a.get('pid') and k.get('pid_same') is True):
        return None
    if a.get('lease') not in HELD or not digits(a.get('lease_epoch')):
        return None
    if b.get('lease') in HELD and b.get('lease_epoch') != a.get('lease_epoch'):
        return None
    if b.get('uploaded') != '0' or not digits(a.get('uploaded_whole')) or int(a['uploaded_whole']) < 1:
        return None
    return a['lease_epoch']


def put(d, key, v):
    if v is not M:
        d[key] = v


def row(seq, topo, tps, **kw):
    r = {'_seq': seq, 'topology': topo, 'tps': tps, 'machine': 'm', 'origin_delay_ms': 0, 'workload': 'tpcb'}
    r.update(kw)
    return r


def judge(bk, k, wide=False, alone=False):
    if alone:
        rows = [row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300, origin_delay_ms=10)]
        return bk.bracketed(rows[0], rows)['verdict']
    rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300 if wide else 1000)]
    return bk.bracketed(rows[1], rows)['verdict']


UP = {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'b', 'volume': 'v'}
FROM = ['window', 'workload', True, M, 'NOUPLOAD']
PIDS = [('7', '7', True), ('7', '8', False), ('7', M, False), (M, '7', False), ('7', '7', False), ('7', '7', None),
        ('7', '7', M)]
LEASES = ['held', 'unsure', 'none', 'lost', M]
EPOCHS = ['0', '1', '2', 'x', M]
UPS_START = ['0', '3', '', M]
UPS_END = ['0', '1', '27', M]
WHOLE_END = ['0', '1', '2', 'x', M]
SUFFIX = "; its seed, let go at the window's start, was not shown to commit inside it"
bad = Counter()
ex = {}
accepted = Counter()
n = 0
sample = 0
for fl, (pb, pa, ps), lb, la, eb, ea, ub, ua, wa in itertools.product(
        FROM, PIDS, LEASES, LEASES, EPOCHS, EPOCHS, UPS_START, UPS_END, WHOLE_END):
    b, a = {}, {}
    for d, kv in ((b, (('pid', pb), ('lease', lb), ('lease_epoch', eb), ('uploaded', ub))),
                  (a, (('pid', pa), ('lease', la), ('lease_epoch', ea), ('uploaded', ua), ('uploaded_whole', wa)))):
        for key, v in kv:
            put(d, key, v)
    k = {'before': b, 'after': a}
    put(k, 'pid_same', ps)
    if fl != 'NOUPLOAD':
        k['upload'] = dict(UP)
        put(k['upload'], 'from_load', fl)
    n += 1
    new, old = judge(NEW, k), judge(OLD, k)
    if fl != 'window':
        if new != old:
            bad['non-window row changed'] += 1
            ex.setdefault('non-window row changed', (k, old, new))
        continue
    e = oracle_seed(k)
    if e is not None:
        want = f'at least 0.90 (seed inside window, lease at release (epoch {e}))'
        if new != want:
            bad['seed row not judged as the rule says'] += 1
            ex.setdefault('seed row not judged as the rule says', (k, want, new))
        else:
            if b.get('lease') != 'none':
                accepted[f"start lease {b.get('lease', 'missing')}"] += 1
            if a.get('lease_epoch') == '0':
                accepted['end epoch 0'] += 1
        sample += 1
        if sample % 11 == 0:
            for how in ('wide', 'alone'):
                v = judge(NEW, k, wide=how == 'wide', alone=how == 'alone')
                if 'seed inside window' in v or not v.startswith('NO VERDICT'):
                    bad[f'marker or verdict outside a judged bracket ({how})'] += 1
                    ex.setdefault(f'marker or verdict outside a judged bracket ({how})', (k, '', v))
        continue
    lease_reason = old.startswith('NO VERDICT: the row says it uploads, but it did not hold the lease')
    if lease_reason:
        ok = new.startswith(old + SUFFIX)
    else:
        ok = new == old
    if not ok:
        key = 'window row (rule not met): ' + ('lease reason' if lease_reason else 'other reason')
        bad[key] += 1
        ex.setdefault(key, (k, old, new))
print(f'{n} rows; disagreements: {sum(bad.values())}')
for key, c in bad.items():
    k, want, got = ex[key]
    print(f'  {key}: x{c}\n    e.g. {k}\n    expected {want!r}\n    got      {got!r}')
print('accepted seed rows whose start was not `none` (unreachable from one daemon unless said):')
for key, c in sorted(accepted.items()):
    print(f'  {key}: {c}')

# Named edges, as a window row (NEW's verdict).
W = lambda b, a, ps=True: {'upload': dict(UP, from_load='window'), 'pid_same': ps,
                           'before': {'pid': '7', **b}, 'after': {'pid': '7', **a}}
SEEN = {'lease': 'held', 'lease_epoch': '1', 'uploaded': '1', 'uploaded_whole': '1'}
EDGES = [
    ('sw as run 44462902930 had it', W({'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'}, SEEN)),
    ('the seed went on into a second window: held at epoch 1 at its start, uploaded 0', W(
        {'lease': 'held', 'lease_epoch': '1', 'uploaded': '0'}, SEEN)),
    ('start lost at epoch 5 (lost is final for an instance), end held at 6', W(
        {'lease': 'lost', 'lease_epoch': '5', 'uploaded': '0'}, dict(SEEN, lease_epoch='6'))),
    ('start lease not in the status at all', W({'uploaded': '0'}, SEEN)),
    ('end held at epoch 0', W({'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'}, dict(SEEN, lease_epoch='0'))),
    ('end held, uploaded_whole 1 but uploaded 0 (inconsistent)', W(
        {'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'}, dict(SEEN, uploaded='0'))),
    ('end held, uploaded_whole 1, uploaded not in the status', W(
        {'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'}, {k: v for k, v in SEEN.items() if k != 'uploaded'})),
    ('take-over in the window: held 1 at start, held 2 at end, same pid', W(
        {'lease': 'held', 'lease_epoch': '1', 'uploaded': '0'}, dict(SEEN, lease_epoch='2'))),
    ('a whole upload at the start already (uploaded_whole 1, uploaded 1)', W(
        {'lease': 'none', 'lease_epoch': '0', 'uploaded': '1', 'uploaded_whole': '1'}, dict(SEEN, uploaded='2'))),
    ('epoch as a JSON int at the end', W({'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'},
                                         dict(SEEN, lease_epoch=1))),
    ('uploaded_whole as a JSON int at the end', W({'lease': 'none', 'lease_epoch': '0', 'uploaded': '0'},
                                                  dict(SEEN, uploaded_whole=1))),
]
for name, k in EDGES:
    try:
        v = judge(NEW, k)
    except Exception as exc:  # noqa: BLE001
        v = f'RAISED {type(exc).__name__}: {exc}'
    print(f'- {name}: {v}')
sys.exit(1 if bad else 0)

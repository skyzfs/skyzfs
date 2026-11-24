"""diff_seed_next.py NEW PLAIN: bracket.py's seed rule and seed rows at #132's head against an
oracle of the rule as the lead decided it, and every other row against the plain rule.

NEW is a checkout of the head (0a3c307 or 99b49b9: the bench files are the same), PLAIN one of
861b537 (the rule of #128, before any seed rule). Over a cartesian table of a row's readings
(from_load window, workload, none, or no upload at all; `released_here` True, False or absent;
the pids and pid_same; the lease and its epoch at each end; `uploaded` at the start and the
end; `uploaded_whole` at the end), each row is judged between two model rows (1000 and 1000,
the row at 950) by NEW and by PLAIN:

- a row that is not a window row must get PLAIN's verdict exactly;
- a window row gets the marker "(seed inside window, lease at release (epoch E))" exactly when
  the oracle says the rule holds: `released_here` True, both pids read and pid_same True, the
  lease `none` at the start, `uploaded` '0' at the start, `uploaded_whole` a number of at least
  1 at the end, the lease held or unsure at the end with a numeric epoch;
- any other window row gets PLAIN's verdict, with one of the two seed clauses appended when
  PLAIN's reason is the lease: "its window did not let MinIO go" when `released_here` is not
  True, else "its seed ... was not shown to commit inside it";
- `seed_row` is set exactly when the row uploads and `uploaded` is '0' at its start.

A sample of the marked rows is judged again in a bracket 26% wide and with no model row after
it (no verdict, no marker). Exits 1 on any disagreement. Run with python3 -I.
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


NEW, PLAIN = load(sys.argv[1], 'bk_new'), load(sys.argv[2], 'bk_plain')
M = object()
HELD = ('held', 'unsure')


def digits(v):
    return isinstance(v, str) and v.isdigit()


def oracle_seed(k):
    b, a = k.get('before') or {}, k.get('after') or {}
    if (k.get('upload') or {}).get('from_load') != 'window' or k.get('released_here') is not True:
        return None
    if not (b.get('pid') and a.get('pid') and k.get('pid_same') is True):
        return None
    if a.get('lease') not in HELD or not digits(a.get('lease_epoch')):
        return None
    if b.get('lease') != 'none':
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
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k)]
        return bk.bracketed(rows[1], rows)
    rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300 if wide else 1000)]
    return bk.bracketed(rows[1], rows)


UP = {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'b', 'volume': 'v'}
FROM = ['window', 'workload', M, 'NOUPLOAD']
HERE = [True, False, M]
PIDS = [('7', '7', True), ('7', '8', False), ('7', M, False), (M, '7', False), ('7', '7', None)]
LEASES = ['held', 'unsure', 'none', 'lost', M]
EPOCHS = ['0', '1', '2', 'x', M]
UPS_START = ['0', '3', M]
UPS_END = ['0', '1', '27', M]
WHOLE_END = ['0', '1', 'x', M]
LEASE_REASON = 'NO VERDICT: the row says it uploads, but it did not hold the lease'
NOT_HERE = '; its window did not let MinIO go, so its seed was not measured from its release'
NOT_SHOWN = "; its seed, let go at the window's start, was not shown to commit inside it"
SEED_ROW = "no upload had committed at its window's start"
bad, ex, marked, n, sample = Counter(), {}, Counter(), 0, 0
for fl, here, (pb, pa, ps), lb, la, eb, ea, ub, ua, wa in itertools.product(
        FROM, HERE, PIDS, LEASES, LEASES, EPOCHS, EPOCHS, UPS_START, UPS_END, WHOLE_END):
    b, a = {}, {}
    for d, kv in ((b, (('pid', pb), ('lease', lb), ('lease_epoch', eb), ('uploaded', ub))),
                  (a, (('pid', pa), ('lease', la), ('lease_epoch', ea), ('uploaded', ua), ('uploaded_whole', wa)))):
        for key, v in kv:
            put(d, key, v)
    k = {'before': b, 'after': a}
    put(k, 'pid_same', ps)
    put(k, 'released_here', here)
    if fl != 'NOUPLOAD':
        k['upload'] = dict(UP)
        put(k['upload'], 'from_load', fl)
    n += 1
    got, plain = judge(NEW, k), judge(PLAIN, k)
    new, old = got['verdict'], plain['verdict']
    want_seed = SEED_ROW if fl != 'NOUPLOAD' and ub == '0' else None
    if got['seed_row'] != want_seed:
        bad['seed_row'] += 1
        ex.setdefault('seed_row', (k, want_seed, got['seed_row']))
    if fl != 'window':
        if new != old:
            bad['non-window row changed'] += 1
            ex.setdefault('non-window row changed', (k, old, new))
        continue
    e = oracle_seed(k)
    if e is not None:
        want = f'at least 0.90 (seed inside window, lease at release (epoch {e}))'
        if new != want:
            bad['marked row not as the rule says'] += 1
            ex.setdefault('marked row not as the rule says', (k, want, new))
        marked[f"start epoch {b.get('lease_epoch', 'missing')}, end {a.get('lease')} {a.get('lease_epoch')}"] += 1
        sample += 1
        if sample % 7 == 0:
            for how in ('wide', 'alone'):
                v = judge(NEW, k, wide=how == 'wide', alone=how == 'alone')['verdict']
                if 'seed inside window' in v or not v.startswith('NO VERDICT'):
                    bad[f'marker or verdict outside a judged bracket ({how})'] += 1
                    ex.setdefault(f'marker or verdict outside a judged bracket ({how})', (k, '', v))
        continue
    if old.startswith(LEASE_REASON):
        ok = new.startswith(old + (NOT_HERE if here is not True else NOT_SHOWN))
    else:
        ok = new == old
    if not ok:
        key = 'window row (rule not met): ' + ('lease reason' if old.startswith(LEASE_REASON) else 'other reason')
        bad[key] += 1
        ex.setdefault(key, (k, old, new))
print(f'{n} rows; disagreements: {sum(bad.values())}')
for key, c in bad.items():
    k, want, got = ex[key]
    print(f'  {key}: x{c}\n    e.g. {k}\n    expected {want!r}\n    got      {got!r}')
print(f'marked rows: {sum(marked.values())}, by start epoch and end lease:')
for key, c in sorted(marked.items()):
    print(f'  {key}: {c}')
sys.exit(1 if bad else 0)

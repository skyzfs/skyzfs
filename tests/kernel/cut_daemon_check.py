#!/usr/bin/env python3
"""The cut rig on the daemon, first stage (M2.T3, design §5.2): the
daemon's count of cuts, their groups and whether each opens as itself,
against the reference.

Usage: cut_daemon_check.py NAME DIR REQUEST_LOG - DEV_BYTES [LOG_DEVICE_IMAGE]

Called by tests/kernel/cut-record.sh with CUT_SERVER=daemon, after each
recording: the pool's device was a volume that skyzfsd served with its
request log (bins/skyzfsd/src/record.rs). The fourth argument stands
where the dm-log-writes check takes its base image, and is not used.

The request log is read into the records of tests/kernel/cut_ref.py in
the daemon's own order, the order design §5.2 counts in:

- a write is placed at its `applied` line, when the store had applied it
  whole, with its bytes from its `label` line where it touches a label
  (the log writes those out whole);
- a flush is placed at its `barrier` line: the daemon's added flush
  after a write with FUA (`fua_flush`) is a flush of its own;
- a discard or a write-zeroes at its `recv` line, where the store places
  it relative to a flush;
- the recording's marks (DIR/marks.tsv, CLOCK_MONOTONIC as the log's)
  by their time.

The reference's cuts on those records (cut_ref.cuts, with
cut_ref.opens for whether each opens as itself) are then held to:

- `cuts`: the daemon's `cut` lines, in order: the same number, each at
  the same instant (the barrier its line follows), with the same group
  and the same answer to whether it opens as itself (`itself` or
  `rewind`);
- `status`: `skyzfsd status` read before the detach (DIR/status.txt):
  `cuts`, `cut_group` and `cut_opens` of the last cut;
- `sessions` and `import_syncs_before_return`: what ZFS says it synced,
  as the cut rig judges the reference (cut_check.py).

Controls (`ctl_*`, `EXPECTED`): the daemon's list with one cut dropped,
with one group moved by one, with one answer turned, and the
reference's cuts by the rule `early` (each at the flush before its
own): each must make `cuts` fail. They show that the comparison would
see each kind of wrong count.

Not judged here: the volume's content at a cut's instant (the next
stage, after M2.2b), and a daemon that is started again on the volume
(the count is per instance, and the rings it starts from are the
volume's). A request log with more than one instance is the rig's error.

Exit 0: everything as the reference; 1: a verdict; 2: the rig's error.
"""
import os
import re
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_check  # noqa: E402
import cut_ref  # noqa: E402
from cut_check import RigError  # noqa: E402

CONTROLS = ('drop', 'group', 'opens', 'early')
EXPECTED = {ctl: 'caught' for ctl in CONTROLS}
SECTOR = 512


def flat(text):
    return ' '.join(str(text).split())


def parse_log(text):
    """The request log -> [instance], each {'pid', 'device', 'lines': [dict]} in the file's order."""
    instances, by_pid = [], {}
    for k, ln in enumerate(text.splitlines()):
        f = ln.split('\t')
        if len(f) != 8 or not f[0].isdigit():
            raise RigError(f'request log line {k + 1}: not eight columns: {ln[:120]!r}')
        rec = {'ns': int(f[0]), 'pid': f[1], 'event': f[2], 'tag': f[3], 'op': f[4], 'sector': f[5],
               'sectors': f[6], 'result': f[7], 'line': k + 1}
        if rec['event'] == 'start':
            inst = {'pid': rec['pid'], 'device': rec['tag'], 'lines': []}
            instances.append(inst)
            by_pid[rec['pid']] = inst
            continue
        inst = by_pid.get(rec['pid'])
        if inst is None:
            raise RigError(f'request log line {k + 1}: pid {rec["pid"]} has no start line')
        inst['lines'].append(rec)
    return instances


def read_marks(text):
    """marks.tsv -> [(ns, name)]."""
    out = []
    for ln in text.splitlines():
        f = ln.split('\t')
        if len(f) != 2 or not f[0].isdigit():
            raise RigError(f'marks.tsv: not a time and a name: {ln!r}')
        out.append((int(f[0]), f[1]))
    return out


def records_of(lines, marks):
    """One instance's lines and the marks -> (records, the daemon's cuts).

    The records are cut_ref's, in the daemon's order. A daemon cut:
    {'txg', 'itself', 'prefix', 'line'}, prefix the record of the
    barrier its line follows: the records before it are those before
    the cut's instant, as a reference cut's prefix."""
    records, dcuts, pending, data = [], [], {}, {}
    marks = sorted(marks)
    mk = 0

    def add(r):
        r['i'] = len(records)
        records.append(r)
        return r

    def marks_until(ns):
        nonlocal mk
        while mk < len(marks) and marks[mk][0] <= ns:
            add({'flags': ['mark'], 'mark': marks[mk][1], 'offset': 0, 'bytes': 0})
            mk += 1

    last_barrier = None
    for ln in lines:
        marks_until(ln['ns'])
        ev = ln['event']
        if ev == 'recv':
            if not (ln['sector'].isdigit() and ln['sectors'].isdigit()):
                raise RigError(f"request log line {ln['line']}: a recv without its sectors")
            off, n = int(ln['sector']) * SECTOR, int(ln['sectors']) * SECTOR
            if ln['op'] == 'write':
                pending[ln['tag']] = (off, n)
                data.pop(ln['tag'], None)
            elif ln['op'] in ('discard', 'write_zeroes'):
                add({'flags': ['discard'], 'offset': off, 'bytes': n, 'op': ln['op']})
        elif ev == 'label':
            try:
                data[ln['tag']] = bytes.fromhex(ln['result'])
            except ValueError:
                raise RigError(f"request log line {ln['line']}: a label line whose bytes are not hex") from None
        elif ev == 'applied':
            if ln['tag'] not in pending:
                raise RigError(f"request log line {ln['line']}: write {ln['tag']} applied with no recv before it")
            off, n = pending.pop(ln['tag'])
            r = {'flags': [], 'offset': off, 'bytes': n, 'pos': None, 'tag': ln['tag']}
            b = data.pop(ln['tag'], None)
            if b is not None:
                if len(b) != n:
                    raise RigError(f"request log line {ln['line']}: write {ln['tag']} of {n} bytes, label line of {len(b)}")
                r['data'] = b
            add(r)
        elif ev == 'barrier':
            last_barrier = add({'flags': ['flush'], 'offset': 0, 'bytes': 0, 'tag': ln['tag'], 'op': ln['op']})
        elif ev == 'cut':
            if not ln['result'].isdigit() or ln['sector'] not in ('itself', 'rewind'):
                raise RigError(f"request log line {ln['line']}: a cut line that is not a group and itself or rewind")
            if last_barrier is None or last_barrier['tag'] != ln['tag'] or last_barrier['op'] != ln['op']:
                raise RigError(f"request log line {ln['line']}: a cut that does not follow its own barrier")
            dcuts.append({'txg': int(ln['result']), 'itself': ln['sector'] == 'itself',
                          'prefix': last_barrier['i'], 'line': ln['line']})
    marks_until(float('inf'))
    return records, dcuts


def read_of(records, dev_bytes):
    """cut_ref's read(r): the bytes of a write that touches a label, which the log holds."""
    labels = cut_ref.label_offsets(dev_bytes)

    def read(r):
        if 'data' not in r:
            if cut_ref.touches_label(r['offset'], r['bytes'], labels):
                raise RigError(f"write at {r['offset']} touches a label and the log has no label line for it")
            return b'\0' * r['bytes']
        return r['data']
    return read


def compare(dcuts, ref):
    """The daemon's cuts and the reference's [{'txg', 'itself', 'prefix'}] -> (ok, detail).
    The same instant (prefix), group and answer to itself, in order."""
    for k, (d, r) in enumerate(zip(dcuts, ref)):
        key_d = (d['prefix'], d['txg'], d['itself'])
        key_r = (r['prefix'], r['txg'], r['itself'])
        if key_d != key_r:
            return False, (f"cut {k + 1}: the daemon's is group {d['txg']} "
                           f"({'itself' if d['itself'] else 'rewind'}) at record {d['prefix']}, the reference's "
                           f"group {r['txg']} ({'itself' if r['itself'] else 'rewind'}) at record {r['prefix']}")
    if len(dcuts) != len(ref):
        return False, f'the daemon counted {len(dcuts)} cuts, the reference {len(ref)}'
    return True, f'{len(ref)} cuts, each at the same barrier with the same group and the same answer to itself'


def reference(cs, op):
    return [{'txg': c['txg'], 'itself': op[c['n']]['itself'], 'prefix': c['prefix']} for c in cs]


def mutants(dcuts):
    """The controls' wrong lists of the daemon's cuts -> {name: list or None when it cannot be built}."""
    out = {}
    if not dcuts:
        return {'drop': None, 'group': None, 'opens': None}
    mid = len(dcuts) // 2
    out['drop'] = dcuts[:mid] + dcuts[mid + 1:]
    out['group'] = [dict(c, txg=c['txg'] + 1) if k == mid else c for k, c in enumerate(dcuts)]
    out['opens'] = [dict(c, itself=not c['itself']) if k == mid else c for k, c in enumerate(dcuts)]
    return out


def read_status(text):
    out = {}
    for ln in text.splitlines():
        k, _, v = ln.partition(' ')
        if k:
            out[k] = v.strip()
    return out


def status_verdict(status, ref):
    """status.txt's values and the reference's cuts -> (ok, detail)."""
    want = {'cuts': str(len(ref))}
    if ref:
        want['cut_group'] = str(ref[-1]['txg'])
        want['cut_opens'] = 'itself' if ref[-1]['itself'] else 'rewind'
    bad = [f'{k} {status.get(k)!r}, not {v!r}' for k, v in want.items() if status.get(k) != v]
    if bad:
        return False, '; '.join(bad)
    return True, ', '.join(f'{k} {v}' for k, v in want.items())


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    name, rdir, log = sys.argv[1:4]
    dev = int(sys.argv[5])
    t0 = time.monotonic()
    lines = []

    def put(key, verdict, detail):
        lines.append(f'{key}\t{verdict}\t{flat(detail)}')
        print(f'{name}: {key}: {verdict}: {flat(detail)}', flush=True)

    rc = 0
    try:
        with open(log) as f:
            instances = parse_log(f.read())
        if len(instances) != 1:
            raise RigError(f'{len(instances)} instances of the daemon in the request log; this stage reads one')
        marks_path = os.path.join(rdir, 'marks.tsv')
        with open(marks_path) as f:
            marks = read_marks(f.read())
        records, dcuts = records_of(instances[0]['lines'], marks)
        read = read_of(records, dev)
        ubs = cut_ref.scan(records, read, dev)
        cs, taken = cut_ref.cuts(records, ubs)
        op = cut_ref.opens(records, read, dev, cs)
        ref = reference(cs, op)
        put('records', 'INFO', f"{len(records)} records from the request log ({sum(1 for r in records if 'data' in r)} "
            f"with their bytes), {sum(1 for u in taken if u['taken'] == 'marks')} uberblock writes that mark, of "
            f"{len(taken)}; {sum(1 for r in records if 'mark' in r['flags'])} marks")
        ok, detail = compare(dcuts, ref)
        put('cuts', 'PASS' if ok else 'FAIL', detail)
        if not ok:
            rc = 1
        n_self = sum(1 for r in ref if r['itself'])
        put('obs_opens_as_itself', 'INFO', f'{n_self} of {len(ref)} cuts open as themselves, by the reference')
        with open(os.path.join(rdir, 'status.txt')) as f:
            status = read_status(f.read())
        ok, detail = status_verdict(status, ref)
        put('status', 'PASS' if ok else 'FAIL', detail)
        if not ok:
            rc = 1
        # What ZFS says it synced, as the cut rig holds the reference to it.
        kstats = {}
        for fn in os.listdir(rdir):
            m = re.fullmatch(r'txgs-(\d+)\.txt', fn)
            if m:
                with open(os.path.join(rdir, fn)) as f:
                    kstats[int(m.group(1))] = cut_check.committed_txgs(f.read())
        sv = cut_check.session_verdicts(cs, records, kstats)
        if any(v is None for _, v, _ in sv):
            raise RigError('; '.join(d for _, v, d in sv if v is None))
        bad = [d for _, v, d in sv if not v]
        covered, order = cut_check.outside_kstat(cs, records, kstats)
        bad += order[:5]
        put('sessions', 'PASS' if not bad else 'FAIL', '; '.join(bad) if bad else
            f'{len(sv)} sessions as committed; {covered} cuts within the kstats\' windows, the others in strictly '
            'increasing groups')
        if bad:
            rc = 1
        sb = cut_check.synced_before_ready(cs, records)
        if any(n is None for _, n, _ in sb):
            raise RigError('; '.join(d for _, n, d in sb if n is None))
        none = [d for _, n, d in sb if n == 0]
        put('import_syncs_before_return', 'PASS' if not none else 'FAIL',
            '; '.join(d for _, _, d in sb) if not none else '; '.join(none))
        if none:
            rc = 1
        # The controls: each wrong list must fail the comparison.
        got = {}
        for ctl, wrong in mutants(dcuts).items():
            if wrong is None:
                got[ctl] = ('not built', 'the daemon counted no cut')
            else:
                ok, detail = compare(wrong, ref)
                got[ctl] = ('missed' if ok else 'caught', detail)
        ecs, _ = cut_ref.cuts(records, ubs, 'early')
        eref = reference(ecs, cut_ref.opens(records, read, dev, ecs))
        ok, detail = compare(dcuts, eref)
        got['early'] = ('missed' if ok else 'caught', f"the reference by the rule early: {detail}")
        for ctl in CONTROLS:
            verdict, detail = got[ctl]
            good = verdict == EXPECTED[ctl]
            put(f'ctl_{ctl}', verdict.upper().replace(' ', '_'),
                f"{detail} (expected: {EXPECTED[ctl]}{'' if good else '; DIFFERS'})")
            if not good:
                rc = 1
    except RigError as e:
        put('rig_error', 'ERROR', str(e))
        rc = 2
    except Exception as e:  # noqa: BLE001 -- anything else is the rig's too, not a verdict
        put('rig_error', 'ERROR', f'{type(e).__name__}: {e}')
        rc = 2
    put('time', 'INFO', f'{time.monotonic() - t0:.0f} s')
    with open(os.path.join(rdir, 'results.tsv'), 'a') as f:
        f.write(''.join(ln + '\n' for ln in lines))
    sys.exit(rc)


if __name__ == '__main__':
    main()

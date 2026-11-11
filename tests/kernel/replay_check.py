#!/usr/bin/env python3
"""Power-cut replay check: does the pool import at every prefix of the write log?

The recording half of the rig (replay-rig.sh) ran a workload on a pool
whose data vdev sat on dm-log-writes, marked the log at each completed
`zpool sync`, and noted which files existed at each mark. This half
rebuilds the device as it was after N records, for many N, and checks
each one:

  light check, every sampled prefix
      a read-only loop device over the image; `zpool import -o
      readonly=on -N -m -f` must succeed; the dataset must mount; every
      file recorded at the last mark at or before the prefix must have
      its checksum.
  deep check, at flush boundaries
      a throwaway dm-snapshot over the image; a writable import and
      `zpool scrub -w`, which reads every block the pool can reach;
      no error may be counted and nothing repaired.

Which prefixes: every flush boundary, every prefix within WINDOW records
of an uberblock write, and random others, as many as the time budget
allows after a short calibration. One image is advanced record by
record; the checks are serial, because one kernel cannot import the
same pool twice.

Negative controls. A rig that cannot catch a wrong replay proves
nothing, so two wrong replays are run through the same checks:
  drop      one data write from the middle of a transaction group is
            never applied
  reorder   the last data write before the flush that precedes a
            group's uberblocks is held back until that group is
            complete, so the uberblocks land without it
The victims are picked by a fixed rule (see controls()), not chosen to
be caught. What each is expected to do today is the table EXPECTED; the
job fails when a result differs from it in either direction. Today the
rig shows that the pool imports and that the promised files are intact
at every prefix. It does not yet show that every block the newest
uberblock reaches is on the device in every copy: `reorder`, whose
block is one of three copies of pool metadata, is not caught.

The image comes from a provider (default logwrites.py over the
dm-log-writes log; see its header for the two commands). The same checks
will run on images from skyzfsd's own log replay.

Needs root, ZFS, losetup, dmsetup with dm-snapshot.
"""
import argparse
import hashlib
import json
import os
import random
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import time

WINDOW = 50
HERE = os.path.dirname(os.path.abspath(__file__))


# ----------------------------------------------------------- pure logic
def parse_marks(records):
    """-> [(prefix at which the mark is in the log, sync number, files complete)], in order.

    A mark reads sync-K-C: the K-th sync, C files complete before it began."""
    marks = []
    for i, r in enumerate(records):
        text = r.get('mark', '')
        f = text.split('-')
        if len(f) == 3 and f[0] == 'sync' and f[1].isdigit() and f[2].isdigit():
            marks.append((i + 1, int(f[1]), int(f[2])))
    return marks


def files_at(marks, n):
    """Files that must verify at prefix n: those of the last mark at or before it."""
    c = 0
    for pos, _, count in marks:
        if pos > n:
            break
        c = count
    return c


def flush_points(records):
    return [i + 1 for i, r in enumerate(records) if 'flush' in r['flags']]


def ub_clusters(records, window=WINDOW):
    """Uberblock writes, grouped where their windows touch -> [(first prefix, last prefix)]."""
    total = len(records)
    out = []
    for i, r in enumerate(records):
        if not r.get('ub'):
            continue
        lo, hi = max(0, i + 1 - window), min(total, i + 1 + window)
        if out and lo <= out[-1][1] + 1:
            out[-1] = (out[-1][0], hi)
        else:
            out.append((lo, hi))
    return out


def spread(items, k):
    """k of the items, evenly spaced, keeping the first and the last."""
    items = list(items)
    if k >= len(items):
        return items
    if k <= 0:
        return []
    if k == 1:
        return [items[len(items) // 2]]
    return [items[round(j * (len(items) - 1) / (k - 1))] for j in range(k)]


def plan(records, budget_points, deep_points, seed=1, window=WINDOW):
    """Choose the prefixes to check -> (kind by prefix, deep set, what was available).

    Priority: the two ends and every flush boundary; then whole windows
    around uberblock writes, for at most 60% of what is left; then random
    prefixes. A prefix that is several things counts as the first, and a
    random pick that falls inside a window counts as the window's: when
    uberblock writes are closer together than the window, the windows
    merge and may be too large to take whole.
    """
    total = len(records)
    flush = flush_points(records)
    clusters = ub_clusters(records, window)
    in_window = set()
    for lo, hi in clusters:
        in_window.update(range(lo, hi + 1))
    # The two ends are their own kind, whatever else they are.
    avail = {'records': total, 'prefixes': total + 1, 'flush': len(set(flush) - {0, total}),
             'ub_writes': sum(1 for r in records if r.get('ub')),
             'ub_window': len(in_window - set(flush) - {0, total}),
             'other': total + 1 - len(in_window | set(flush) | {0, total})}
    kind = {0: 'end', total: 'end'}
    for n in spread(flush, max(0, budget_points - len(kind))):
        kind.setdefault(n, 'flush')
    left = budget_points - len(kind)
    quota = int(left * 0.6)
    order = spread(range(len(clusters)), len(clusters))
    # Whole windows, evenly spread over the log, while they fit.
    picked, used = [], 0
    for step in range(1, len(clusters) + 1):
        cand = spread(order, step)
        size = sum(len([n for n in range(clusters[c][0], clusters[c][1] + 1) if n not in kind]) for c in cand)
        if size > quota:
            break
        picked, used = cand, size
    for c in picked:
        for n in range(clusters[c][0], clusters[c][1] + 1):
            kind.setdefault(n, 'ub_window')
    left = budget_points - len(kind)
    rest = [n for n in range(total + 1) if n not in kind]
    rng = random.Random(seed)
    for n in rng.sample(rest, min(max(left, 0), len(rest))):
        kind[n] = 'ub_window' if n in in_window else 'random'
    deep = set(spread([n for n in sorted(kind) if kind[n] == 'flush'], deep_points))
    return kind, deep, avail


def data_writes(records, lo, hi):
    """Record numbers in [lo, hi) that are writes outside the labels."""
    return [i for i in range(lo, hi) if 'pos' in records[i] and not records[i].get('label')]


def controls(records, marks):
    """The two wrong replays -> list of dicts with a schedule and the prefixes to check.

    Fixed rule, on the transaction group of the middle mark:
      A      the last flush before that group's first uberblock write
      drop   the median data write between the previous mark and A is
             left out for good. Checked from there to the mark after
             the middle one, so that the files of the group are due.
      reorder  the last data write before A is moved to just before the
             middle mark, after the group's uberblocks and flushes.
             Checked at every prefix from A to the mark.
    """
    if len(marks) < 4:
        return []
    m = len(marks) // 2
    lo, hi = marks[m - 1][0], marks[m][0] - 1          # record numbers of the group; hi is the mark
    ubs = [i for i in range(lo, hi) if records[i].get('ub')]
    if not ubs:
        return []
    flushes = [i for i in range(lo, ubs[0]) if 'flush' in records[i]['flags']]
    if not flushes:
        return []
    a = flushes[-1]
    writes = data_writes(records, lo, a)
    if not writes:
        return []
    end = marks[m + 1][0] if m + 1 < len(marks) else len(records)
    natural = list(range(len(records)))
    out = []

    victim = writes[len(writes) // 2]
    sched = [i for i in natural if i != victim]
    pts = set(range(a, hi + 1))                         # around the group's own uberblocks
    pts.update(n for n in range(victim, end) if 'flush' in records[sched[n - 1]]['flags'])
    pts.update(spread(range(victim, end), 40))
    pts.add(end - 1)
    out.append({'name': 'drop', 'victim': victim, 'schedule': sched, 'points': sorted(p for p in pts if p >= victim),
                'what': f'record {victim} (a write of {records[victim]["bytes"]} bytes at byte {records[victim]["offset"]}) is never applied'})

    victim = writes[-1]
    sched = [i for i in natural if i != victim]
    sched.insert(hi - 1, victim)                        # just before the mark
    pts = sorted(range(a, hi + 1))
    out.append({'name': 'reorder', 'victim': victim, 'schedule': sched, 'points': pts,
                'what': f'record {victim} (a write of {records[victim]["bytes"]} bytes at byte {records[victim]["offset"]}, '
                        f'the last data write before flush {a}) is applied only after the uberblocks of its transaction group'})
    return out


# What each control is expected to do today. The job passes only when
# the observed results equal this table, and fails when they differ in
# either direction: a control that stops being caught is a regression,
# and one that starts being caught means the rig got deeper and this
# table must say so on purpose. Do not edit a control to fit the table.
#   by: 'deep'  caught by the deep check and by no light check
#       None    not caught: a known gap, stated in every run's summary
EXPECTED = {
    'drop': {'by': 'deep'},
    'reorder': {'by': None,
                'gap': 'this rig does not yet detect a device that delays or loses one copy of redundant pool '
                       'metadata: the withheld block is one of three copies, the read-only import reads '
                       'another, and the scrub runs after a writable import. To be closed by work package M1.T3 '
                       '(a deep check that reads every copy without writing first).'},
}


def judge_controls(ctl, expected=None):
    """Compare the controls' results with the table -> (rows, gaps, all as expected)."""
    expected = EXPECTED if expected is None else expected
    seen = {c['name']: c for c in ctl}
    rows, gaps, ok = [], [], True
    for name in sorted(set(expected) | set(seen)):
        want, c = expected.get(name), seen.get(name)
        if c is None:
            got = 'not run'
        elif not c['caught']:
            got = 'not caught'
        else:
            kinds = set(c['failed_by'])
            got = 'caught by the deep check only' if kinds == {'deep'} else 'caught by ' + ', '.join(sorted(kinds))
        if want is None:
            exp = 'not in the table'
        elif want['by'] is None:
            exp = 'not caught'
        elif want['by'] == 'deep':
            exp = 'caught by the deep check only'
        else:
            exp = 'caught by ' + want['by']
        match = exp == got
        ok = ok and match
        rows.append({'name': name, 'expected': exp, 'observed': got, 'match': match})
        if want and want.get('gap') and match:
            gaps.append({'name': name, 'gap': want['gap']})
    return rows, gaps, ok


def victim_facts(log_path, records, victim, base_path=None):
    """What can be said from the log alone about the block a control withholds.

    A control that is not caught needs explaining, and these are the
    cheap explanations: the write changed nothing, the same block went
    to another place as well (ZFS keeps two or three copies of its
    metadata), or it was overwritten soon after.
    """
    v = records[victim]
    lo, hi = v['offset'], v['offset'] + v['bytes']
    facts = {'copies': [], 'earlier': None, 'later': None, 'same_as_before': None}
    with open(log_path, 'rb') as f:
        f.seek(v['pos'])
        data = f.read(v['bytes'])
        facts['zeros'] = not any(data)
        for r in records:
            if 'pos' not in r:
                continue
            overlaps = r['offset'] < hi and lo < r['offset'] + r['bytes']
            if r['i'] != victim and overlaps:
                if r['i'] < victim:
                    facts['earlier'] = r['i']
                elif facts['later'] is None:
                    facts['later'] = r['i']
            if r['bytes'] < len(data):
                continue
            f.seek(r['pos'])
            payload = f.read(r['bytes'])
            at = payload.find(data)
            while at >= 0:
                if not (r['i'] == victim and at == 0) and len(facts['copies']) < 16:
                    facts['copies'].append({'record': r['i'], 'offset': r['offset'] + at})
                at = payload.find(data, at + len(data))
        before = None
        e = facts['earlier']
        if e is not None and records[e]['offset'] <= lo and hi <= records[e]['offset'] + records[e]['bytes']:
            f.seek(records[e]['pos'] + lo - records[e]['offset'])
            before = f.read(len(data))
        elif e is None and base_path:
            with open(base_path, 'rb') as b:
                b.seek(lo)
                before = b.read(len(data))
        if before is not None:
            facts['same_as_before'] = before == data
    return facts


def facts_text(facts):
    c = facts['copies']
    parts = ['all zeros' if facts['zeros'] else None,
             'the device already held the same bytes there' if facts['same_as_before'] else None,
             (f"the same bytes are also written at {len(c)} other place(s): "
              + ', '.join(f"record {x['record']} at byte {x['offset']}" for x in c[:4])) if c else 'no other write in the log carries the same bytes',
             f"that place is next written by record {facts['later']}" if facts['later'] is not None else 'that place is not written again',
             f"and was last written by record {facts['earlier']}" if facts['earlier'] is not None else 'and was not written earlier in the log']
    return '; '.join(x for x in parts if x)


# ------------------------------------------------------------- running
def run(cmd, timeout=300):
    t0 = time.monotonic()
    try:
        p = subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, timeout=timeout)
        return p.returncode, p.stdout, time.monotonic() - t0
    except subprocess.TimeoutExpired as e:
        return 124, (e.stdout or '') + f'\nTIMEOUT after {timeout}s', time.monotonic() - t0


class Rig:
    def __init__(self, args):
        self.a = args
        self.provider = shlex.split(args.provider) if args.provider else [sys.executable, os.path.join(HERE, 'logwrites.py')]
        self.index_path = os.path.join(args.work, 'index.json')
        self.imgdir = os.path.join(args.work, 'img')
        self.image = os.path.join(self.imgdir, 'replay.img')
        self.alt = os.path.join(args.work, 'alt')
        self.manifest = [line.split()[:2] for line in open(args.manifest) if line.strip()]
        self.at = 0
        self.sched_path = None
        os.makedirs(self.imgdir, exist_ok=True)
        os.makedirs(self.alt, exist_ok=True)

    # image provider
    def make_index(self):
        rc, out, _ = run(self.provider + ['index', '--log', self.a.log, '--dev-bytes', str(os.path.getsize(self.a.base)),
                                          '--out', self.index_path])
        if rc:
            sys.exit(f'provider index failed: {out}')
        with open(self.index_path) as f:
            return json.load(f)

    def reset(self, schedule=None):
        """A fresh image at prefix 0: a copy of the base."""
        if os.path.exists(self.image):
            os.unlink(self.image)
        rc, out, _ = run(['cp', '--sparse=always', self.a.base, self.image])
        if rc:
            sys.exit(f'copying the base image failed: {out}')
        self.at = 0
        self.sched_path = None
        if schedule is not None:
            self.sched_path = os.path.join(self.a.work, 'schedule.json')
            with open(self.sched_path, 'w') as f:
                json.dump(schedule, f)

    def advance(self, n):
        if n < self.at:
            raise ValueError('the image only moves forward')
        if n == self.at:
            return 0.0
        cmd = self.provider + ['advance', '--log', self.a.log, '--index', self.index_path, '--image', self.image,
                               '--from', str(self.at), '--to', str(n)]
        if self.sched_path:
            cmd += ['--schedule', self.sched_path]
        rc, out, t = run(cmd)
        if rc:
            sys.exit(f'provider advance {self.at}..{n} failed: {out}')
        self.at = n
        return t

    # checks
    def export(self):
        rc, out, _ = run(['zpool', 'export', self.a.pool], 120)
        if rc:
            rc, out, _ = run(['zpool', 'export', '-f', self.a.pool], 120)
        return rc, out

    def light(self, files):
        """Read-only import, mount, verify -> (stage that failed or None, detail, seconds by stage)."""
        t = {}
        rc, out, _ = run(['losetup', '-r', '--find', '--show', self.image])
        if rc:
            return 'losetup', out.strip(), t
        loop = out.strip()
        try:
            rc, out, t['import'] = run(['zpool', 'import', '-d', loop, '-o', 'readonly=on', '-o', 'cachefile=none',
                                        '-N', '-m', '-f', '-R', self.alt, self.a.pool], 180)
            if rc:
                return 'import', out.strip()[-400:], t
            try:
                rc, out, t['mount'] = run(['zfs', 'mount', self.a.pool], 60)
                if rc:
                    return 'mount', out.strip()[-400:], t
                t0 = time.monotonic()
                root = self.alt + self.a.mountpoint
                for want, name in self.manifest[:files]:
                    try:
                        h = hashlib.sha256()
                        with open(os.path.join(root, name), 'rb') as f:
                            for chunk in iter(lambda: f.read(1 << 20), b''):
                                h.update(chunk)
                    except OSError as e:
                        return 'verify', f'{name}: {e}', t
                    if h.hexdigest() != want:
                        return 'verify', f'{name}: checksum differs', t
                t['verify'] = time.monotonic() - t0
            finally:
                rc, out = self.export()
                if rc:
                    return 'export', out.strip()[-400:], t
        finally:
            run(['losetup', '-d', loop])
        return None, '', t

    def deep(self):
        """Writable import of a throwaway snapshot, and a scrub -> (failure or None, detail, seconds)."""
        t0 = time.monotonic()
        cow = os.path.join(self.a.work, 'cow.img')
        name = 'skz-replay-snap'
        loops = []
        try:
            with open(cow, 'wb') as f:
                f.truncate(2 << 30)
            for cmd in (['losetup', '-r', '--find', '--show', self.image], ['losetup', '--find', '--show', cow]):
                rc, out, _ = run(cmd)
                if rc:
                    return 'losetup', out.strip(), time.monotonic() - t0
                loops.append(out.strip())
            sectors = os.path.getsize(self.image) // 512
            rc, out, _ = run(['dmsetup', 'create', name, '--table', f'0 {sectors} snapshot {loops[0]} {loops[1]} N 8'])
            if rc:
                return 'dm-snapshot', out.strip(), time.monotonic() - t0
            try:
                rc, out, _ = run(['zpool', 'import', '-d', f'/dev/mapper/{name}', '-o', 'cachefile=none', '-N', '-m', '-f',
                                  '-R', self.alt, self.a.pool], 180)
                if rc:
                    return 'import (writable)', out.strip()[-400:], time.monotonic() - t0
                try:
                    rc, out, _ = run(['zpool', 'scrub', '-w', self.a.pool], 600)
                    if rc:
                        return 'scrub', out.strip()[-400:], time.monotonic() - t0
                    rc, out, _ = run(['zpool', 'status', '-p', self.a.pool], 60)
                    bad = scrub_verdict(out)
                    if bad:
                        return 'scrub', bad, time.monotonic() - t0
                finally:
                    self.export()
            finally:
                for _ in range(20):
                    if run(['dmsetup', 'remove', name])[0] == 0:
                        break
                    time.sleep(0.25)
        finally:
            for lo in loops:
                run(['losetup', '-d', lo])
            if os.path.exists(cow):
                os.unlink(cow)
        return None, '', time.monotonic() - t0

    def point(self, n, files, deep):
        res = {'n': n, 'files': files, 't_advance': self.advance(n)}
        t0 = time.monotonic()
        stage, detail, t = self.light(files)
        res['t_light'] = time.monotonic() - t0
        res['t'] = t
        if stage is None and deep:
            stage, detail, res['t_deep'] = self.deep()
            if stage:
                stage = 'deep: ' + stage
        res['ok'] = stage is None
        if stage:
            res['failed'], res['detail'] = stage, detail
        return res


def scrub_verdict(status):
    """None when `zpool status -p` shows a scrub that found nothing, else what it found."""
    errors = 0
    scan = verdict = ''
    for line in status.splitlines():
        f = line.split()
        if len(f) >= 5 and all(x.isdigit() for x in f[2:5]):
            errors += sum(int(x) for x in f[2:5])
        if line.strip().startswith('scan:'):
            scan = line.strip()
        if line.startswith('errors:'):
            verdict = line.strip()
    if errors or 'with 0 errors' not in scan or 'repaired 0B' not in scan or 'No known data errors' not in verdict:
        return f'{errors} errors counted on the vdevs; {scan}; {verdict}'
    return None


def pct(xs, p):
    xs = sorted(xs)
    return xs[min(len(xs) - 1, int(p * len(xs)))] if xs else float('nan')


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--log', required=True, help='the write log the provider reads')
    ap.add_argument('--base', required=True, help='image of the device before the first record')
    ap.add_argument('--manifest', required=True, help='"sha256 name" per file, in the order the files were completed')
    ap.add_argument('--pool', required=True)
    ap.add_argument('--mountpoint', required=True, help='mountpoint property of the pool root dataset')
    ap.add_argument('--work', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--provider', default='', help='image provider command (default: logwrites.py)')
    ap.add_argument('--budget-s', type=float, default=840, help='seconds for the main pass')
    ap.add_argument('--deep-budget-s', type=float, default=240, help='seconds of deep checks inside the main pass')
    ap.add_argument('--seed', type=int, default=1)
    ap.add_argument('--replay-log', default='', help='path of xfstests replay-log, as a witness for the provider')
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    rig = Rig(args)
    idx = rig.make_index()
    records = idx['records']
    total = len(records)
    marks = parse_marks(records)
    res = {'records': total, 'marks': len(marks), 'files': len(rig.manifest), 'sectorsize': idx['sectorsize'],
           'write_bytes': sum(r['bytes'] for r in records if 'pos' in r),
           'discards': sum(1 for r in records if 'discard' in r['flags'])}
    print(f"log: {total} records, {len(marks)} marks, {res['write_bytes'] >> 20} MiB written, {len(rig.manifest)} files", flush=True)
    if not marks:
        sys.exit('the log has no sync marks: nothing to check against')

    # Calibration: a few prefixes spread over the log, two with the deep check.
    rig.reset()
    cal = []
    flush = flush_points(records)
    cal_pts = sorted(set(spread(range(total + 1), 8)) | set(spread(flush, 2)))
    for n in cal_pts:
        r = rig.point(n, files_at(marks, n), n in set(spread(flush, 2)))
        r['kind'] = 'calibration'
        cal.append(r)
        print(f"calibration {n}: {'ok' if r['ok'] else 'FAILED ' + r['failed']} light {r['t_light']:.2f}s"
              + (f" deep {r['t_deep']:.2f}s" if 't_deep' in r else ''), flush=True)
    t_light = statistics.mean(r['t_light'] + r['t_advance'] for r in cal)
    deeps = [r['t_deep'] for r in cal if 't_deep' in r]
    t_deep = statistics.mean(deeps) if deeps else 5.0
    budget_points = max(4, int(args.budget_s * 0.9 / t_light))
    deep_points = max(1, int(args.deep_budget_s / t_deep))
    kind, deep, avail = plan(records, budget_points, deep_points, args.seed)
    res['calibration'] = {'points': len(cal), 't_light_mean': t_light, 't_deep_mean': t_deep,
                          'budget_points': budget_points, 'deep_points': deep_points}
    res['available'] = avail
    print(f'plan: {len(kind)} prefixes of {total + 1} ({budget_points} fit the budget at {t_light:.2f}s each), '
          f'{len(deep)} deep checks at {t_deep:.2f}s each', flush=True)

    # Main pass, in order, on one image.
    rig.reset()
    points, failures = list(cal), [r for r in cal if not r['ok']]
    start = time.monotonic()
    skipped = 0
    for j, n in enumerate(sorted(kind)):
        over = time.monotonic() - start - args.deep_budget_s
        # Out of time: random prefixes go first, flush boundaries never.
        if kind[n] == 'random' and over > args.budget_s or kind[n] == 'ub_window' and over > args.budget_s * 1.25:
            skipped += 1
            continue
        r = rig.point(n, files_at(marks, n), n in deep)
        r['kind'] = kind[n]
        points.append(r)
        if not r['ok']:
            failures.append(r)
            print(f"FAILED prefix {n} ({kind[n]}): {r['failed']}: {r['detail']}", flush=True)
            if len(failures) >= 25:
                print('25 failures: stopping the main pass', flush=True)
                break
        if j % 200 == 0:
            print(f'  {j} of {len(kind)} prefixes, {time.monotonic() - start:.0f}s', flush=True)
    res['main_pass_s'] = time.monotonic() - start
    res['skipped_for_time'] = skipped

    # zdb as a possible deep check without the kernel: timed, not judged.
    zdb = []
    rig.reset()
    for n in spread(flush, 3):
        rig.advance(n)
        rc, out, t = run(['zdb', '-e', '-p', rig.imgdir, '-bcc', args.pool], 300)
        said = [ln.strip() for ln in out.splitlines() if re.search(r'ASSERT|VERIFY|PANIC|panic|failed|cannot|error', ln)][:4]
        zdb.append({'n': n, 'rc': rc, 's': t, 'tail': ' | '.join(out.strip().splitlines()[-3:])[-300:],
                    'said': ' | '.join(said)[:600]})
        print(f'zdb -e -bcc at prefix {n}: rc {rc} in {t:.1f}s', flush=True)
    res['zdb'] = zdb

    # The provider against xfstests replay-log, at a middle flush boundary and at the end.
    witness = []
    if args.replay_log:
        other = os.path.join(args.work, 'witness.img')
        for n in (spread(flush, 3)[1] if len(flush) >= 3 else total, total):
            rig.reset()
            rig.advance(n)
            run(['cp', '--sparse=always', args.base, other])
            rc, out, _ = run([args.replay_log, '--log', args.log, '--replay', other, '--limit', str(n)] if n else ['true'])
            same = rc == 0 and run(['cmp', '-s', rig.image, other])[0] == 0
            witness.append({'n': n, 'rc': rc, 'same': same, 'tail': out.strip()[-200:]})
            print(f"replay-log at prefix {n}: {'same image' if same else 'DIFFERENT'} (rc {rc})", flush=True)
        if os.path.exists(other):
            os.unlink(other)
    res['witness'] = witness

    # Negative controls: wrong replays, judged against EXPECTED.
    ctl = []
    for c in controls(records, marks):
        rig.reset(c['schedule'])
        deep_at = {n for n in c['points'] if 'flush' in records[c['schedule'][n - 1]]['flags']} if c['points'] else set()
        first, by, checked = None, {}, 0
        for n in c['points']:
            r = rig.point(n, files_at(marks, n), n in deep_at)
            checked += 1
            if not r['ok']:
                by[r['failed'].split(':')[0]] = by.get(r['failed'].split(':')[0], 0) + 1
                if first is None:
                    first = {'n': n, 'failed': r['failed'], 'detail': r['detail']}
        facts = victim_facts(args.log, records, c['victim'], args.base) if not args.provider else None
        ctl.append({'name': c['name'], 'what': c['what'], 'victim': c['victim'], 'points': checked, 'facts': facts,
                    'deep_points': len(deep_at), 'caught': first is not None, 'first': first, 'failed_by': by})
        print(f"control {c['name']}: {'caught' if first else 'NOT CAUGHT'} over {checked} prefixes"
              + (f", first at {first['n']} by {first['failed']}" if first else ''), flush=True)
    res['controls'] = ctl
    res['control_rows'], res['gaps'], controls_ok = judge_controls(ctl)
    for g in res['gaps']:
        print(f"::warning title=replay rig, known gap::control {g['name']} is not caught: {g['gap']}", flush=True)
    for r in res['control_rows']:
        if not r['match']:
            print(f"::error title=replay rig::control {r['name']}: expected {r['expected']}, observed {r['observed']}", flush=True)

    # The index goes with the results: a finding needs the records around it.
    run(['cp', rig.index_path, os.path.join(args.out, 'replay-index.json')])
    res['points'] = points
    res['failures'] = failures
    with open(os.path.join(args.out, 'replay-results.json'), 'w') as f:
        json.dump(res, f, indent=1)
    with open(os.path.join(args.out, 'replay-summary.md'), 'w') as f:
        f.write(summary(res))
    print(summary(res))
    ok = not failures and controls_ok and all(w['same'] for w in witness)
    sys.exit(0 if ok else 1)


def summary(res):
    pts = [p for p in res['points'] if p.get('kind') != 'calibration']
    out = ['## Power-cut replay\n']
    av = res['available']
    out.append(f"Write log: {res['records']} records, {res['write_bytes'] >> 20} MiB written, {res['marks']} sync marks, "
               f"{res['files']} files, {av['flush']} flushes, {av['ub_writes']} uberblock writes.\n")
    verdict = 'every checked prefix imported and verified' if not res['failures'] else f"**{len(res['failures'])} prefixes FAILED**"
    out.append(f"Result: {verdict}. Main pass {res['main_pass_s']:.0f} s"
               + (f", {res['skipped_for_time']} planned prefixes skipped for time" if res['skipped_for_time'] else '') + '.\n')
    out.append('| kind of prefix | in the log | checked | failed | light check s, mean / p95 |')
    out.append('|---|---|---|---|---|')
    for k, label, n_av in (('flush', 'flush boundary', av['flush']),
                           ('ub_window', f'within {WINDOW} records of an uberblock write', av['ub_window']),
                           ('random', 'any other, at random', av['other']), ('end', 'first and last', 2)):
        sel = [p for p in pts if p['kind'] == k]
        ts = [p['t_light'] + p['t_advance'] for p in sel]
        out.append(f"| {label} | {n_av} | {len(sel)} | {sum(1 for p in sel if not p['ok'])} | "
                   + (f"{statistics.mean(ts):.2f} / {pct(ts, 0.95):.2f} |" if ts else '– |'))
    ts = [p['t_light'] + p['t_advance'] for p in pts]
    deep = [p['t_deep'] for p in pts if 't_deep' in p]
    if ts:
        per = statistics.mean(ts)
        parts = {k: statistics.mean([p['t'].get(k, 0) for p in pts]) for k in ('import', 'mount', 'verify')}
        out.append(f"\nLight check: {per:.2f} s per prefix on average (import {parts['import']:.2f}, mount {parts['mount']:.2f}, "
                   f"verify {parts['verify']:.2f}, the rest is the loop device, the export and advancing the image). "
                   f"At that rate a 30-minute job covers about {int(1800 / per)} prefixes with no deep check.")
    if deep:
        out.append(f"\nDeep check (writable import of a snapshot and a scrub): {len(deep)} flush boundaries, "
                   f"{statistics.mean(deep):.2f} s each on average, {max(deep):.2f} s at most, "
                   f"{sum(1 for p in pts if 't_deep' in p and not p['ok'])} failed.")
    if res.get('zdb'):
        out.append('\n`zdb -e -bcc` on the image, timed as an alternative deep check (not judged): '
                   + '; '.join(f"prefix {z['n']}: rc {z['rc']} in {z['s']:.1f} s" for z in res['zdb']) + '.')
        bad = [z for z in res['zdb'] if z['rc']]
        if bad:
            out.append(f"zdb said: `{bad[0].get('said') or bad[0]['tail']}`")
    if res.get('witness'):
        out.append('\nProvider against xfstests `replay-log`: '
                   + '; '.join(f"prefix {w['n']}: {'same image' if w['same'] else 'DIFFERENT'}" for w in res['witness']) + '.')
    out.append('\n### Negative controls\n')
    if not res['controls']:
        out.append('**None could be built from this log.**')
    else:
        out.append('| control | what was done | prefixes checked (deep) | caught | first failure |')
        out.append('|---|---|---|---|---|')
        for c in res['controls']:
            first = f"prefix {c['first']['n']}: {c['first']['failed']}: {c['first']['detail'][:160]}" if c['first'] else '–'
            by = ', '.join(f'{k} {v}' for k, v in c['failed_by'].items())
            out.append(f"| {c['name']} | {c['what']} | {c['points']} ({c['deep_points']}) | "
                       f"{'yes, ' + by if c['caught'] else 'no'} | {first.replace('|', '/')} |")
        out.append('\nAgainst what each control is expected to do today (`EXPECTED` in `replay_check.py`; '
                   'a difference in either direction fails the job):\n')
        out.append('| control | expected | observed | |')
        out.append('|---|---|---|---|')
        for r in res.get('control_rows', []):
            out.append(f"| {r['name']} | {r['expected']} | {r['observed']} | {'as expected' if r['match'] else '**DIFFERS**'} |")
        for g in res.get('gaps', []):
            out.append(f"\n> [!WARNING]\n> Known gap (`{g['name']}` is not caught): {g['gap']}")
        for c in res['controls']:
            if c.get('facts'):
                out.append(f"\nThe block withheld by `{c['name']}`: {facts_text(c['facts'])}.")
    if res['failures']:
        out.append('\n### Failures\n')
        for p in res['failures'][:25]:
            out.append(f"- prefix {p['n']} ({p.get('kind')}): {p['failed']}: {p['detail'][:300]}")
    return '\n'.join(out) + '\n'


if __name__ == '__main__':
    main()

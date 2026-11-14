#!/usr/bin/env python3
"""The check of the reference's cuts on a recorded pool (M2.T1).

    cut_check.py NAME DIR LOG BASE DEV_BYTES

cut-record.sh runs it after each recording: LOG is the dm-log-writes
record, BASE the device before it (zeros), DIR where the recording's
files are and the results go. Design §5.2's claim, held against real
ZFS: at every cut of the reference (cut_ref.py, rule `design`), the
volume as it was at the cut's instant is a complete pool at the cut's
group. At each cut, on one image moved forward through the record:

  group   zdb (-e -u) selects the uberblock of the cut's group and
          timestamp: the pool opens at N and at no other state;
  copies  every block that uberblock reaches, in every copy, passes its
          checksum (zdb -bbbbb and the replay rig's reader,
          replay_check.every_copy_verdict);
  import  the kernel imports it read-only (zpool import -o readonly=on).

And for each session of the pool (cut-record.sh's marks): the groups of
the cuts whose uberblock write falls in the session equal, in order, the
groups the pool's `txgs` kstat says were committed in it (those up to
the kstat's last; the export's own come after it was read).

The fixture for the daemon's parser (M2.1) is written to DIR/fixture
(cut_ref.py fixture; its format in tests/fixtures/pools/README.md).

Negative controls: wrong cuts through the same checks, at the cut in the
middle of the count (or the rule's whole list):
  early          the volume one flush early: at the last flush before
                 the uberblock write that marked the cut
  withheld       the last write before that uberblock write that holds
                 a copy of a block the cut reaches is not applied
  later          the first write or discard after the instant that
                 overlaps a copy of a block the cut reaches is applied
  txg-alone      the cuts by a rule that keys a repeat on the group alone
  count-repeats  the cuts by a rule that counts every uberblock write
A cut whose state equals an older cut's (cut_ref.stale; under
multihost=on a multihost write of an older state applied after a newer
cut) is held to its own rule: the pool opens at its group or a later
one, never an earlier one, and it is left out of the session check and
counted (obs_stale_cuts), a measurement the lead asked for.

A control is `caught` when a check fails on it, `missed` when every
check passes, `same` (the two rules) when its cuts equal the design's,
and `not built` when the record has no write it needs. The table
EXPECTED says what each must do, by recording; a result that differs,
in either direction, fails the job.

Exit: 0 when every cut passed and every control did as EXPECTED; 1 when
a cut failed a check (a verdict against the design's claim, or the
reference) or a control did otherwise; 2 for the rig's own error.
"""
import json
import os
import re
import shutil
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import cut_ref  # noqa: E402
import logwrites  # noqa: E402
import replay_check  # noqa: E402
from replay_check import run  # noqa: E402

POOL = 'skzcut'
# What each control must do, by recording. `same` is for the two rules
# where the record has nothing that tells them from the design's: no
# flush between copies of an uberblock (count-repeats) and no group
# written twice (txg-alone).
EXPECTED = {
    'early': {r: 'caught' for r in ('life', 'rewind', 'multihost', 'config', 'stray', 'autotrim', 'full')},
    'withheld': {r: 'caught' for r in ('life', 'rewind', 'multihost', 'config', 'stray', 'autotrim', 'full')},
    'later': {r: 'caught' for r in ('life', 'rewind', 'multihost', 'config', 'stray', 'autotrim', 'full')},
    'txg-alone': {'life': 'same', 'rewind': 'caught', 'multihost': 'same', 'config': 'same', 'stray': 'same',
                  'autotrim': 'same', 'full': 'same'},
    'count-repeats': {'life': 'same', 'rewind': 'same', 'multihost': 'caught', 'config': 'same', 'stray': 'caught',
                      'autotrim': 'same', 'full': 'same'},
}
UB_TXG_RE = re.compile(r'^\s*txg = (\d+)\s*$', re.M)
UB_TS_RE = re.compile(r'^\s*timestamp = (\d+)', re.M)


class RigError(Exception):
    pass


def committed_txgs(text):
    """The `txgs` kstat -> the groups in state C, in order."""
    out = []
    for line in text.splitlines():
        f = line.split()
        if len(f) >= 3 and f[0].isdigit() and f[2] == 'C':
            out.append(int(f[0]))
    return out


def sessions(records):
    """The marks session-K-begin / session-K-end -> {K: (begin record, end record)}."""
    out = {}
    for r in records:
        m = re.fullmatch(r'session-(\d+)-(begin|end)', r.get('mark', '') if 'mark' in r['flags'] else '')
        if m:
            k = int(m.group(1))
            lo, hi = out.get(k, (None, None))
            out[k] = (r['i'], hi) if m.group(2) == 'begin' else (lo, r['i'])
    return out


def session_verdicts(cs, records, kstats):
    """Cuts, records, {K: committed groups} -> [(K, ok, detail)].

    A stale cut (cut_ref.stale: the state of an older cut, from a
    multihost write applied late) is left out of the comparison; the
    check counts those separately."""
    skip = cut_ref.stale(cs)
    out = []
    for k, (lo, hi) in sorted(sessions(records).items()):
        if lo is None or hi is None:
            out.append((k, None, f'session {k} has no begin or no end mark'))
            continue
        want = kstats.get(k)
        if not want:
            out.append((k, None, f'session {k}: no committed group in its kstat'))
            continue
        got = [c['txg'] for c in cs if lo < c['ub_record'] < hi and c['txg'] <= max(want) and c['n'] not in skip]
        if got == want:
            out.append((k, True, f'session {k}: {len(got)} cuts, groups {want[0]}..{want[-1]} as committed'))
        else:
            first = next((i for i, (a, b) in enumerate(zip(got, want)) if a != b), min(len(got), len(want)))
            out.append((k, False, f'session {k}: the cuts give {len(got)} groups, the kstat {len(want)} committed; '
                        f'first difference at {first}: cuts {got[max(0, first - 2):first + 3]}, '
                        f'committed {want[max(0, first - 2):first + 3]}'))
    return out


def overlaps(rec, bps):
    """Does a write or discard overlap a copy of one of these blocks?"""
    a, b = rec['offset'], rec['offset'] + rec['bytes']
    for bp in bps:
        for _, off, asize in bp['dvas']:
            lo = off + replay_check.DVA_OFFSET
            if a < lo + asize and b > lo:
                return True
    return False


def pick_withheld(records, cut, bps):
    """The last data write before the cut's uberblock write that holds a copy of a block the cut reaches."""
    for i in range(cut['ub_record'] - 1, -1, -1):
        r = records[i]
        if cut_ref.is_write(r) and overlaps(r, bps):
            return i
    return None


def pick_later(records, cut, bps):
    """The first write or discard after the cut's instant that overlaps a copy of a block the cut reaches."""
    for r in records[cut['prefix']:]:
        if (cut_ref.is_write(r) or ('discard' in r['flags'] and r['bytes'])) and overlaps(r, bps):
            return r['i']
    return None


class Check:
    def __init__(self, name, rdir, log, base, dev_bytes):
        self.name, self.rdir, self.log, self.base, self.dev = name, rdir, log, base, dev_bytes
        self.work = os.path.join(rdir, 'work')
        self.imgdir = os.path.join(self.work, 'img')
        self.image = os.path.join(self.imgdir, 'cut.img')
        self.alt = os.path.join(self.work, 'alt')
        os.makedirs(self.imgdir, exist_ok=True)
        os.makedirs(self.alt, exist_ok=True)
        self.idx = logwrites.index(log, dev_bytes)
        self.records = self.idx['records']
        _, self.read = cut_ref.load_from(self.idx, log)
        self.at = None

    def reset(self):
        if os.path.exists(self.image):
            os.unlink(self.image)
        rc, out, _ = run(['cp', '--sparse=always', self.base, self.image])
        if rc:
            raise RigError(f'copying the base image failed: {out.strip()}')
        self.at = 0

    def advance(self, n, schedule=None):
        if self.at is None or n < self.at:
            self.reset()
        logwrites.advance(self.log, self.idx, self.image, self.at, n, schedule)
        self.at = n

    def group(self):
        """zdb's choice of uberblock -> (txg, timestamp) or a reason it gave none."""
        rc, out, _ = run(['zdb', '-e', '-p', self.imgdir, '-u', POOL], 120)
        if rc in replay_check.RIG_RC:
            raise RigError(f'zdb -u: {replay_check.stage("zdb -u", rc)}')
        t, ts = UB_TXG_RE.search(out), UB_TS_RE.search(out)
        if rc or not t or not ts:
            said = [ln.strip() for ln in out.splitlines() if ln.strip()][-2:]
            return None, f'zdb -u exit {rc}: {" | ".join(said)[:300]}'
        return (int(t.group(1)), int(ts.group(1))), ''

    def copies(self):
        rc, out, _ = run(['zdb', '-e', '-p', self.imgdir, '-bbbbb', POOL], 600)
        failed, detail, counts = replay_check.every_copy_verdict(rc, out, self.image)
        if failed and failed.startswith('rig:'):
            raise RigError(f'{failed}: {detail}')
        bps = replay_check.zdb_blocks(out)['bps'] if not failed else []
        return failed, detail, counts, [bp for bp in bps if bp['type'] != 'ZIL intent log']

    def kimport(self):
        rc, out, _ = run(['zpool', 'list', '-H', '-o', 'name', POOL], 60)
        if rc == 0:
            raise RigError(f'a pool named {POOL} is still imported')
        rc, out, _ = run(['losetup', '-r', '--find', '--show', self.image])
        if rc:
            raise RigError(f'losetup: {out.strip()}')
        loop = out.strip()
        try:
            rc, out, _ = run(['zpool', 'import', '-d', loop, '-o', 'readonly=on', '-o', 'cachefile=none', '-N', '-f',
                              '-R', self.alt, POOL], 180)
            said = out.strip()[-300:]
            if rc in replay_check.RIG_RC:
                raise RigError(f'zpool import: {replay_check.stage("import", rc)}')
            if rc == 0:
                erc, eout, _ = run(['zpool', 'export', POOL], 120)
                if erc:
                    raise RigError(f'zpool export after the import: {eout.strip()[-300:]}')
            return rc == 0, said
        finally:
            run(['losetup', '-d', loop])

    def judge(self, cut, kernel=True, stale=False):
        """The checks at the image as it is, for this cut -> (failed check or None, detail, bps).

        A stale cut (cut_ref.stale) carries the group of an older state:
        the pool may open at a later group, never at an earlier one."""
        g, why = self.group()
        if g is None:
            return 'group', why, []
        low = ''
        if stale and g[0] > cut['txg']:
            low = f"stale: the pool opens at group {g[0]}, {g[0] - cut['txg']} above the cut's; "
        elif g != (cut['txg'], cut['timestamp']):
            return 'group', f"zdb selects group {g[0]} (timestamp {g[1]}), the cut is {cut['txg']} ({cut['timestamp']})", []
        failed, detail, counts, bps = self.copies()
        if failed:
            return 'copies', detail, []
        if kernel:
            ok, said = self.kimport()
            if not ok:
                return 'import', said, bps
        return None, f"{low}{counts.get('blocks', 0)} blocks, {counts.get('copies', 0)} copies", bps


def control_verdict(failed):
    return 'caught' if failed else 'missed'


def main():
    if len(sys.argv) != 6:
        sys.exit(__doc__)
    name, rdir, log, base, dev = sys.argv[1:5] + [int(sys.argv[5])]
    t0 = time.monotonic()
    res = os.path.join(rdir, 'results.tsv')
    lines = []

    def put(key, verdict, detail):
        lines.append(f'{key}\t{verdict}\t{detail}')
        print(f'{name}: {key}: {verdict}: {detail}', flush=True)

    rc = 0
    try:
        ck = Check(name, rdir, log, base, dev)
        about = f'recording {name} (tests/kernel/cut-record.sh)'
        for f in ('zfs-version.txt',):
            p = os.path.join(os.path.dirname(rdir), f)
            if os.path.exists(p):
                about += '\n' + open(p).read().strip().replace('\n', ', ')
        about += f'\nkernel {os.uname().release} {os.uname().machine}'
        cs, taken = cut_ref.write_fixture(os.path.join(rdir, 'fixture'), ck.idx, ck.read, about)
        put('records', 'INFO', f"{len(ck.records)}, of which {sum(1 for u in taken if u['taken'] == 'marks')} "
            f"uberblock writes mark, of {len(taken)}")
        if not cs:
            raise RigError('the reference finds no cut in the record')
        # Every cut, in order, on one image.
        failures, bps_at = [], {}
        stale = cut_ref.stale(cs)
        fresh = [c for c in cs if c['n'] not in stale]
        mid = fresh[len(fresh) // 2]
        lows = []
        for c in cs:
            ck.advance(c['prefix'])
            failed, detail, bps = ck.judge(c, stale=c['n'] in stale)
            if c['n'] in stale and not failed:
                lows.append(f"cut {c['n']} (group {c['txg']}, the state of cut {stale[c['n']]}): {detail.split(';')[0]}")
            if failed:
                failures.append(c)
                put(f"cut_{c['n']}", 'FAIL', f"group {c['txg']} at record {c['prefix']}: {failed}: {detail}")
            if c is mid:
                bps_at['mid'] = bps
        put('cuts', 'PASS' if not failures else 'FAIL',
            f"{len(cs) - len(failures)} of {len(cs)} cuts: the group, every copy, the read-only import")
        put('obs_stale_cuts', str(len(stale)), '; '.join(lows[:6]) if lows else
            'no cut carries the state of an older cut')
        if failures:
            rc = 1
        # The sessions against the kstat.
        kstats = {}
        for fn in os.listdir(rdir):
            m = re.fullmatch(r'txgs-(\d+)\.txt', fn)
            if m:
                with open(os.path.join(rdir, fn)) as f:
                    kstats[int(m.group(1))] = committed_txgs(f.read())
        sv = session_verdicts(cs, ck.records, kstats)
        if any(ok is None for _, ok, _ in sv):
            raise RigError('; '.join(d for _, ok, d in sv if ok is None))
        bad = [d for _, ok, d in sv if not ok]
        put('sessions', 'PASS' if not bad else 'FAIL', '; '.join(bad) if bad else f'{len(sv)} sessions as committed')
        if bad:
            rc = 1
        # The controls.
        got = {}
        if mid in failures or not bps_at.get('mid'):
            raise RigError(f"the controls' cut {mid['n']} has no list of blocks (it failed or lists none)")
        bps = bps_at['mid']
        ecs, _ = cut_ref.cuts(ck.records, cut_ref.scan(ck.records, ck.read, dev), 'early')
        early = next(c for c in ecs if c['n'] == mid['n'])
        ck.advance(early['prefix'])
        failed, detail, _ = ck.judge(mid, kernel=False)
        got['early'] = (control_verdict(failed), f"cut {mid['n']} (group {mid['txg']}) at record {early['prefix']} "
                        f"and not {mid['prefix']}: {failed or 'passed'}: {detail}")
        v = pick_withheld(ck.records, mid, bps)
        if v is None:
            got['withheld'] = ('not built', 'no write before the uberblock write holds a block the cut reaches')
        else:
            sched = [i for i in range(mid['prefix']) if i != v]
            ck.at = None
            ck.advance(0)
            ck.advance(len(sched), sched)
            ck.at = None
            failed, detail, _ = ck.judge(mid, kernel=False)
            r = ck.records[v]
            got['withheld'] = (control_verdict(failed), f"cut {mid['n']} without record {v} ({r['bytes']} bytes at "
                               f"{r['offset']}): {failed or 'passed'}: {detail}")
        v = pick_later(ck.records, mid, bps)
        if v is None:
            got['later'] = ('not built', 'nothing after the instant overlaps a block the cut reaches')
        else:
            sched = list(range(mid['prefix'])) + [v]
            ck.at = None
            ck.advance(0)
            ck.advance(len(sched), sched)
            ck.at = None
            failed, detail, _ = ck.judge(mid, kernel=False)
            r = ck.records[v]
            got['later'] = (control_verdict(failed), f"cut {mid['n']} with record {v} ({'+'.join(r['flags']) or 'write'}, "
                            f"{r['bytes']} bytes at {r['offset']}, {v - mid['prefix']} records after the instant): "
                            f"{failed or 'passed'}: {detail}")
        for rule in ('txg-alone', 'count-repeats'):
            rcs, _ = cut_ref.cuts(ck.records, cut_ref.scan(ck.records, ck.read, dev), rule)
            same = [(c['txg'], c['prefix']) for c in rcs] == [(c['txg'], c['prefix']) for c in cs]
            if same:
                got[rule] = ('same', f'{len(rcs)} cuts, as the design rule')
                continue
            rsv = session_verdicts(rcs, ck.records, kstats)
            rbad = [d for _, ok, d in rsv if not ok]
            got[rule] = ('caught' if rbad else 'missed', f'{len(rcs)} cuts against {len(cs)}: '
                         + ('; '.join(rbad) if rbad else 'every session as committed'))
        for ctl, (verdict, detail) in got.items():
            want = EXPECTED[ctl].get(name)
            ok = verdict == want
            put(f'ctl_{ctl}', verdict.upper().replace(' ', '_'), f"{detail} (expected: {want}{'' if ok else '; DIFFERS'})")
            if not ok:
                rc = max(rc, 1)
    except RigError as e:
        put('rig', 'ERROR', str(e))
        rc = 2
    finally:
        shutil.rmtree(os.path.join(rdir, 'work'), ignore_errors=True)
        with open(res, 'a') as f:
            f.write('\n'.join(lines) + '\n')
        with open(os.path.join(rdir, 'check.json'), 'w') as f:
            json.dump({'name': name, 'exit': rc, 'seconds': round(time.monotonic() - t0)}, f)
    sys.exit(rc)


if __name__ == '__main__':
    main()

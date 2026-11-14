#!/usr/bin/env python3
"""The check of the reference's cuts on a recorded pool (M2.T1).

    cut_check.py NAME DIR LOG BASE DEV_BYTES [LOG_DEVICE_IMAGE]

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

For each session, a cut is complete (its flush included) before the
create or the import returned (the mark session-K-ready): design §5.2's
claim that a cut's group cannot err high for a publish rests on it.

And for each session of the pool (cut-record.sh's marks): the groups of
the cuts whose uberblock write falls in the session equal, in order, the
groups the pool's `txgs` kstat says were committed in it (those up to
the kstat's last; the export's own come after it was read).

With LOG_DEVICE_IMAGE (the recording `slog`, the product's layout of
design §6.1: a log device on another device, not recorded), every
import is of the data device alone with -m, as a restore would make it;
zdb, which cannot open the pool without a log device, has that image
(the log device's labels after the create, no intent log) beside it.
At the middle cut the check also says what zdb makes of the data device
alone and what the import with the log device missing said and showed
(obs_zdb_data_device_alone, obs_import_missing_log).

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
RECS = ('life', 'slog', 'rewind', 'rewindsync', 'rewindf', 'multihost', 'config', 'stray', 'autotrim', 'full')
EXPECTED = {
    'early': {r: 'caught' for r in RECS},
    'withheld': {r: 'caught' for r in RECS},
    # A later write over a block a cut reaches comes only when ZFS reuses
    # space a cut reached: in the nearly full pool and with autotrim's
    # discards; never in the short recordings on a mostly empty pool
    # (runs 43631961753 and 43637989615, every candidate cut). Where the
    # candidates reach back to the first cuts, ZFS rewrites some of their
    # blocks a few hundred records on (run 43647779663: life, config,
    # rewindsync, slog, multihost, rewindf), so in a short recording it
    # is built or not by where ZFS allocates. Never missed.
    'later': dict({r: ('not built', 'caught') for r in RECS}, autotrim='caught', full='caught'),
    # A rewind import with -T writes the new history under groups the
    # abandoned one used (run 43631961753). In rewindf -F is refused (the
    # export rewrote the labels' config in the failed group) and -FX
    # writes the failed group again with another root block pointer
    # during the import, before the kstat's history begins (run
    # 43645075993): dropped_states catches it, the sessions cannot.
    # It differs only where the new history writes a group the abandoned
    # one used: in rewind group 51 (both runs); in rewindsync the syncs
    # outran it and group 51 wrote nothing (run 43637989615).
    'txg-alone': dict({r: 'same' for r in RECS}, rewind='caught', rewindsync=('same', 'caught'),
                      rewindf=('same', 'caught')),
    # In `stray` the back labels' writes are held 20 ms, so the flusher
    # falls between a group's uberblock copies (none did without it, run
    # 43631961753).
    'count-repeats': dict({r: 'same' for r in RECS}, multihost='caught', stray='caught'),
}
# Whether every cut opens as itself: not after a rewind import with -T,
# until the new history's groups pass the abandoned ones (run
# 43631961753, design §5.2 "Whether a cut opens as itself"); a stale
# multihost cut does not either.
# Stale cuts come from the multihost thread alone (design §5.2; ZFS writes
# no uberblock of an older state without multihost=on, mmp.c): anywhere
# else one is something the design does not expect (review of #94, S4).
EXPECTED_STALE = dict({r: 'none' for r in RECS}, multihost='any')
EXPECTED_OPENS = dict({r: 'all' for r in RECS}, rewind='not all', rewindsync='not all', rewindf=('all', 'not all'),
                      multihost=('all', 'not all'))


def as_expected(verdict, want):
    return verdict in want if isinstance(want, tuple) else verdict == want
UB_TXG_RE = re.compile(r'^\s*txg = (\d+)\s*$', re.M)
UB_TS_RE = re.compile(r'^\s*timestamp = (\d+)', re.M)


class RigError(Exception):
    pass


def committed_txgs(text):
    """The `txgs` kstat -> the groups in state C that wrote to the pool
    (nwritten above 0), in order. ZFS 2.4 commits groups that wrote
    nothing and writes no uberblock for them (run 43631961753: in every
    session the cuts were exactly the committed groups that wrote)."""
    out = []
    for line in text.splitlines():
        f = line.split()
        if len(f) >= 6 and f[0].isdigit() and f[2] == 'C' and f[5].isdigit() and int(f[5]) > 0:
            out.append(int(f[0]))
    return out


def marks_of(records, kind):
    """The marks session-K-KIND -> {K: record}."""
    out = {}
    for r in records:
        m = re.fullmatch(r'session-(\d+)-' + kind, r.get('mark', '') if 'mark' in r['flags'] else '')
        if m:
            out[int(m.group(1))] = r['i']
    return out


def sessions(records):
    """The marks session-K-begin / session-K-end -> {K: (begin record, end record)}."""
    lo, hi = marks_of(records, 'begin'), marks_of(records, 'end')
    return {k: (lo.get(k), hi.get(k)) for k in sorted(set(lo) | set(hi))}


def synced_before_ready(cs, records):
    """For each session: the cuts whose flush came between its begin mark
    and its ready mark (the create or import returned) -> [(K, cuts, detail)]."""
    lo, ready = marks_of(records, 'begin'), marks_of(records, 'ready')
    out = []
    for k in sorted(lo):
        if k not in ready:
            out.append((k, None, f'session {k} has no ready mark'))
            continue
        n = [c for c in cs if lo[k] < c['ub_record'] and c['flush_record'] < ready[k]]
        out.append((k, len(n), f"session {k}: {len(n)} cuts complete before the command returned"
                    + (f" (groups {n[0]['txg']}..{n[-1]['txg']})" if n else '')))
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
        # The kstat begins after the create's or import's first group
        # and was read before the export's.
        got = [c['txg'] for c in cs if lo < c['ub_record'] < hi and min(want) <= c['txg'] <= max(want)
               and c['n'] not in skip]
        if got == want:
            out.append((k, True, f'session {k}: {len(got)} cuts, groups {want[0]}..{want[-1]} as committed'))
        else:
            first = next((i for i, (a, b) in enumerate(zip(got, want)) if a != b), min(len(got), len(want)))
            out.append((k, False, f'session {k}: the cuts give {len(got)} groups, the kstat {len(want)} committed; '
                        f'first difference at {first}: cuts {got[max(0, first - 2):first + 3]}, '
                        f'committed {want[max(0, first - 2):first + 3]}'))
    return out


LOADED_RE = re.compile(r'spa_load\(' + POOL + r', [^)]*\): using uberblock with txg=(\d+)')
DBGMSG = '/proc/spl/kstat/zfs/dbgmsg'
DBGMSG_ON = '/sys/module/zfs/parameters/zfs_dbgmsg_enable'


def loaded_txg(dbgmsg):
    """The group of the uberblock the kernel's last load of the pool used,
    from its debug messages, or None. -T allows an extreme rewind: a load
    at the group that fails goes on to older uberblocks (spa_load_best),
    and in that recovery mode zpool prints no "returned to its state" line
    (the rewind info is kept only outside it), so the kernel's own account
    is what says which state came up (review of #94, S1)."""
    found = LOADED_RE.findall(dbgmsg)
    return int(found[-1]) if found else None


def outside_kstat(cs, records, kstats):
    """The order check where the kstat does not reach -> (cuts the kstat
    covered, [failure]). The session check compares the cuts inside each
    kstat's window; the create's first group, the export's groups (after
    the end mark) and a group an import writes before its history begins
    (rewindf's -FX) are outside it (review of #94, S2). From one
    session's begin mark to the next one's, the groups of the cuts that
    are not stale must still increase strictly: a state counted twice, or
    a group counted again, is a group that does not."""
    skip = cut_ref.stale(cs)
    spans = sessions(records)
    begins = sorted(lo for lo, _ in spans.values() if lo is not None)
    covered, bad = 0, []
    for k, (lo, hi) in sorted(spans.items()):
        if lo is None:
            continue
        nxt = next((b for b in begins if b > lo), float('inf'))
        want = kstats.get(k) or []
        span = [c for c in cs if lo < c['ub_record'] < nxt and c['n'] not in skip]
        covered += sum(1 for c in span if hi is not None and c['ub_record'] < hi and want
                       and min(want) <= c['txg'] <= max(want))
        for a, b in zip(span, span[1:]):
            if b['txg'] <= a['txg']:
                bad.append(f"session {k}: cut {b['n']} (group {b['txg']}) after cut {a['n']} (group {a['txg']})")
    return covered, bad


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


def rule_same(cs, rcs, stale):
    """-> what the line says when a rule's cuts are the design's, else None.

    A stale cut (the state of an older one, which the pool does not open
    at) is all a rule can drop with nothing to see: the pool opens at the
    same state with it or without it (multihost, run 43647779663:
    txg-alone dropped cuts 10 and 27). It is left out here as in the
    session check."""
    def k(c):
        return c['txg'], c['rootbp'], c['prefix']
    key = [k(c) for c in rcs]
    if key == [k(c) for c in cs]:
        return f'{len(rcs)} cuts, as the design rule'
    fresh = [k(c) for c in cs if c['n'] not in stale]
    if key == fresh:
        return f'{len(rcs)} cuts, as the design rule but for its {len(cs) - len(fresh)} stale cuts, which it does not count'
    return None


def dropped_states(cs, rcs, op):
    """The design's cuts a rule's list lacks where the volume opens as
    that cut, at a state other than the rule's newest cut then: the rule
    says the volume holds an older state when it holds a newer one.
    cs, rcs: the design's and the rule's cuts; op: cut_ref.opens of cs.
    -> [detail]. The session check cannot see a group written before the
    kstat's history begins (rewindf: -FX writes the failed group again
    during the import, run 43645075993)."""
    have = {(c['txg'], c['prefix']) for c in rcs}
    out = []
    for c in cs:
        o = op.get(c['n'])
        if (c['txg'], c['prefix']) in have or not o or not o['itself']:
            continue
        older = [r for r in rcs if r['prefix'] <= c['prefix']]
        r = older[-1] if older else None
        if r is None or (r['txg'], r['rootbp']) != (c['txg'], c['rootbp']):
            out.append(f"at cut {c['n']}'s instant (record {c['prefix']}) the volume opens as cut {c['n']} "
                       f"(group {c['txg']}), and the rule's newest cut is "
                       + (f"group {r['txg']} at record {r['prefix']}" + (', another root block pointer' if r['txg'] == c['txg'] else '')
                          if r else 'none'))
    return out


def later_candidates(cs, mid):
    """The cuts tried for the control `later`, in order: the middle one,
    then a quarter, an eighth and a sixteenth of the way, then the first."""
    out = [mid]
    for k in (4, 8, 16):
        c = cs[len(cs) // k]
        if c not in out:
            out.append(c)
    if cs and cs[0] not in out:
        out.append(cs[0])
    return out


class Check:
    def __init__(self, name, rdir, log, base, dev_bytes, logdev=None):
        self.name, self.rdir, self.log, self.base, self.dev = name, rdir, log, base, dev_bytes
        self.logdev = logdev
        self.last_import = None
        # A pool with multihost=on: ZFS 2.4 imports one that was not
        # exported only after a claim, uberblocks it writes to every
        # writable leaf and reads back (spa_activity_check_claim,
        # mmp_claim_uberblock). A read-only import opens no leaf for
        # writing, so the claim has no good write and the import is
        # refused as "imported on host '<unknown>' (hostid=0)" (runs
        # 43638676548 and 43645075993, the second on a writable device).
        # Such a pool is imported writable, from a copy of the cut's
        # image, and the line obs_multihost_import says so.
        self.multihost = False
        # zfs_dbgmsg_enable as it was before a -T import turned it on.
        self.dbgmsg_was = None
        self.with_logdev = 0
        self.work = os.path.join(rdir, 'work')
        self.imgdir = os.path.join(self.work, 'img')
        self.image = os.path.join(self.imgdir, 'cut.img')
        self.alt = os.path.join(self.work, 'alt')
        os.makedirs(self.imgdir, exist_ok=True)
        os.makedirs(self.alt, exist_ok=True)
        # The product's layout: zdb cannot open the pool without a log
        # device, so the log device as it was after the create (its
        # labels, no intent log) stands beside the data device for zdb.
        # The kernel's import takes the data device alone, with -m, as a
        # restore would; the directory `alone` holds it alone for zdb.
        self.alone = os.path.join(self.work, 'alone')
        if logdev:
            rc, out, _ = run(['cp', '--sparse=always', logdev, os.path.join(self.imgdir, 'log-device.img')])
            if rc:
                raise RigError(f'copying the log device failed: {out.strip()}')
            os.makedirs(self.alone, exist_ok=True)
            os.symlink(self.image, os.path.join(self.alone, 'cut.img'))
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

    def zdb(self, args, txg=None, timeout=120):
        """zdb on the volume. In the product's layout on the data device
        alone first, as a restore has it; only where that does not open
        (run 43637989615: the create's groups, zdb exit 1, I/O error)
        with the log device's copy beside it, and the count says so."""
        res = None
        for d in ([self.alone, self.imgdir] if self.logdev else [self.imgdir]):
            res = run(['zdb', '-e', '-p', d] + (['-t', str(txg)] if txg else []) + args + [POOL], timeout)
            if res[0] == 0:
                if d == self.imgdir and self.logdev:
                    self.with_logdev += 1
                return res
        return res

    def group(self, txg=None):
        """zdb's choice of uberblock (the highest group, or the highest
        at or below txg) -> (txg, timestamp) or a reason it gave none."""
        rc, out, _ = self.zdb(['-u'], txg)
        if rc in replay_check.RIG_RC:
            raise RigError(f'zdb -u: {replay_check.stage("zdb -u", rc)}')
        t, ts = UB_TXG_RE.search(out), UB_TS_RE.search(out)
        if rc or not t or not ts:
            said = [ln.strip() for ln in out.splitlines() if ln.strip()][-2:]
            return None, f'zdb -u exit {rc}: {" | ".join(said)[:300]}'
        return (int(t.group(1)), int(ts.group(1))), ''

    def copies(self, txg=None):
        rc, out, _ = self.zdb(['-bbbbb'], txg, 600)
        failed, detail, counts = replay_check.every_copy_verdict(rc, out, self.image)
        if failed and failed.startswith('rig:'):
            raise RigError(f'{failed}: {detail}')
        bps = replay_check.zdb_blocks(out)['bps'] if not failed else []
        return failed, detail, counts, [bp for bp in bps if bp['type'] != 'ZIL intent log']

    def kimport(self, txg=None):
        """A read-only import (at txg with -T; writable from a copy for a
        multihost pool, see __init__) -> (ok, what zpool said). With -T the
        kernel's load must have used an uberblock of that group."""
        rc, out, _ = run(['zpool', 'list', '-H', '-o', 'name', POOL], 60)
        if rc == 0:
            raise RigError(f'a pool named {POOL} is still imported')
        image = self.image
        if self.multihost:
            image = os.path.join(self.work, 'claim.img')
            rc, out, _ = run(['cp', '--sparse=always', self.image, image])
            if rc:
                raise RigError(f'copying the image for the claim failed: {out.strip()}')
        rc, out, _ = run(['losetup'] + ([] if self.multihost else ['-r']) + ['--find', '--show', image])
        if rc:
            raise RigError(f'losetup: {out.strip()}')
        loop = out.strip()
        if txg:
            with open(DBGMSG_ON) as f:
                self.dbgmsg_was = self.dbgmsg_was if self.dbgmsg_was is not None else f.read().strip()
            with open(DBGMSG_ON, 'w') as f:
                f.write('1')
            with open(DBGMSG, 'w') as f:
                f.write('0')
        try:
            rc, out, _ = run(['zpool', 'import', '-d', loop] + ([] if self.multihost else ['-o', 'readonly=on'])
                             + ['-o', 'cachefile=none', '-N', '-f',
                              '-R', self.alt] + (['-m'] if self.logdev else []) + (['-T', str(txg)] if txg else [])
                             + [POOL], 180)
            said = out.strip()[-300:]
            if rc in replay_check.RIG_RC:
                raise RigError(f'zpool import: {replay_check.stage("import", rc)}')
            if rc == 0 and self.logdev:
                _, st, _ = run(['zpool', 'status', '-v', POOL], 60)
                self.last_import = (out.strip(), st.strip())
            if rc == 0:
                erc, eout, _ = run(['zpool', 'export', POOL], 120)
                if erc:
                    raise RigError(f'zpool export after the import: {eout.strip()[-300:]}')
            if rc == 0 and txg:
                with open(DBGMSG) as f:
                    at = loaded_txg(f.read())
                if at != txg:
                    return False, (f'zpool import -T {txg} succeeded, but the kernel loaded '
                                   + (f'group {at}: an older uberblock' if at is not None else
                                      'no uberblock it named in its debug messages'))
            return rc == 0, said
        finally:
            run(['losetup', '-d', loop])

    def whole(self, cut, txg=None, kernel=True):
        """Every copy, then the read-only import, of the state at txg (or
        the one zdb selects) -> (failed check or None, detail, bps)."""
        failed, detail, counts, bps = self.copies(txg)
        if failed:
            return 'copies', detail, []
        if kernel:
            ok, said = self.kimport(txg)
            if not ok:
                return 'import', said, bps
        return None, f"{counts.get('blocks', 0)} blocks, {counts.get('copies', 0)} copies", bps

    def judge(self, cut, pred, kernel=True):
        """The checks at the image as it is, for this cut -> (failed check
        or None, detail, bps, what was seen).

        pred is the reference's reading of the rings (cut_ref.opens): the
        uberblock ZFS should choose at this instant. zdb must choose it.
        A cut that opens as itself must be whole: every copy and the
        read-only import. One that does not (after a rewind import, or a
        stale cut) must be whole at its own group (zdb -t, import -T),
        and the line says what the state zdb chose is."""
        seen = {'zdb_txg': None, 'itself': pred['itself'], 'whole': False}
        g, why = self.group()
        if g is None:
            return 'group', why, [], seen
        seen['zdb_txg'] = g[0]
        if g != (pred['txg'], pred['timestamp']):
            return ('opens', f"zdb selects group {g[0]} (timestamp {g[1]}), the rings' best is {pred['txg']} "
                    f"({pred['timestamp']}); the cut is {cut['txg']}", [], seen)
        if pred['itself']:
            failed, detail, bps = self.whole(cut, None, kernel)
            seen['whole'] = not failed
            return failed, detail, bps, seen
        g2, why = self.group(cut['txg'])
        if g2 is None or g2 != (cut['txg'], cut['timestamp']):
            return 'group', f"zdb -t {cut['txg']} selects {g2 or why}, not the cut's uberblock", [], seen
        failed, detail, bps = self.whole(cut, cut['txg'], kernel)
        seen['whole'] = not failed
        if failed:
            return failed, f'at its own group: {detail}', [], seen
        note = f"does not open as itself: the pool opens at group {g[0]}"
        if kernel:
            f2, d2, _ = self.whole(cut, None, kernel)
            note += f", and that state is {'NOT whole: ' + f2 + ': ' + d2 if f2 else 'whole: ' + d2}"
        return None, f'{note}; at its own group (zdb -t, import -T): {detail}', bps, seen


def alone_at_mid(ck, c, put):
    """The product's layout, at the middle cut: zdb on the data device
    alone, and what the kernel's import with the log device missing (-m)
    said and showed."""
    rc, out, _ = run(['zdb', '-e', '-p', ck.alone, '-u', POOL], 120)
    t = UB_TXG_RE.search(out)
    said = [ln.strip() for ln in out.splitlines() if ln.strip()][-2:]
    put('obs_zdb_data_device_alone', 'INFO', f"cut {c['n']} (group {c['txg']}): zdb -e -u on the data device alone: exit {rc}, "
        + (f"group {t.group(1)}" if t and not rc else 'no group') + f"; it said: {' | '.join(said)[:300]}")
    if ck.last_import:
        imp, st = ck.last_import
        flat = ' | '.join(ln.strip() for ln in (imp + '\n' + st).splitlines() if ln.strip())
        put('obs_import_missing_log', 'INFO', f"cut {c['n']}: zpool import -m -o readonly=on of the data device alone said, "
            f"and zpool status -v then showed: {flat[:1500]}")
    else:
        put('obs_import_missing_log', 'INFO', f"cut {c['n']}: no import with -m succeeded at this cut")


def control_verdict(failed):
    return 'caught' if failed else 'missed'


def main():
    if len(sys.argv) not in (6, 7):
        sys.exit(__doc__)
    name, rdir, log, base, dev = sys.argv[1:5] + [int(sys.argv[5])]
    logdev = sys.argv[6] if len(sys.argv) == 7 else None
    t0 = time.monotonic()
    res = os.path.join(rdir, 'results.tsv')
    lines = []

    def put(key, verdict, detail):
        lines.append(f'{key}\t{verdict}\t{detail}')
        print(f'{name}: {key}: {verdict}: {detail}', flush=True)

    rc = 0
    try:
        ck = Check(name, rdir, log, base, dev, logdev)
        about = f'recording {name} (tests/kernel/cut-record.sh)'
        for f in ('zfs-version.txt',):
            p = os.path.join(os.path.dirname(rdir), f)
            if os.path.exists(p):
                about += '\n' + open(p).read().strip().replace('\n', ', ')
        about += f'\nkernel {os.uname().release} {os.uname().machine}'
        cs, taken = cut_ref.write_fixture(os.path.join(rdir, 'fixture'), ck.idx, ck.read, about)
        ck.multihost = any(u.get('mmp_delay') for u in taken)
        if ck.multihost:
            put('obs_multihost_import', 'INFO', 'the imports are writable, each from a copy of the cut\'s image: ZFS 2.4 '
                'refuses a read-only import of a multihost pool that was not exported (its claim writes uberblocks)')
        put('records', 'INFO', f"{len(ck.records)}, of which {sum(1 for u in taken if u['taken'] == 'marks')} "
            f"uberblock writes mark, of {len(taken)}")
        if not cs:
            raise RigError('the reference finds no cut in the record')
        # Every cut, in order, on one image.
        failures, bps_at, opened = [], {}, {}
        stale = cut_ref.stale(cs)
        op = cut_ref.opens(ck.records, ck.read, dev, cs)
        fresh = [c for c in cs if c['n'] not in stale and op[c['n']]['itself']]
        mid = fresh[len(fresh) // 2]
        notself = []
        for c in cs:
            ck.advance(c['prefix'])
            failed, detail, bps, seen = ck.judge(c, op[c['n']])
            opened[c['n']] = seen
            if not op[c['n']]['itself'] and not failed:
                notself.append(f"cut {c['n']} (group {c['txg']}{', stale' if c['n'] in stale else ''}): {detail}")
            if failed:
                failures.append(c)
                put(f"cut_{c['n']}", 'FAIL', f"group {c['txg']} at record {c['prefix']}: {failed}: {detail}")
            if c is mid:
                bps_at['mid'] = bps
                if logdev:
                    alone_at_mid(ck, c, put)
        with open(os.path.join(rdir, 'fixture', 'opens.tsv'), 'w') as f:
            f.write('# what real ZFS made of each reference cut (cut_check.py): zdb_txg the group zdb -e -u selected\n'
                    '# at the cut\'s instant, ring_txg the best uberblock in the labels by the reference, itself 1 when\n'
                    '# that is the cut\'s own state, whole 1 when the cut passed every copy and the read-only import at\n'
                    '# its own group\n# n\ttxg\tprefix\tzdb_txg\tring_txg\titself\twhole\n')
            for c in cs:
                o, s2 = op[c['n']], opened[c['n']]
                f.write(f"{c['n']}\t{c['txg']}\t{c['prefix']}\t{s2['zdb_txg']}\t{o['txg']}\t{int(o['itself'])}\t"
                        f"{int(s2['whole'])}\n")
        put('cuts', 'PASS' if not failures else 'FAIL',
            f"{len(cs) - len(failures)} of {len(cs)} cuts: zdb selects the uberblock the rings hold as the best, "
            f"and the cut is whole at its group (every copy, the read-only import)")
        n_self = sum(1 for c in cs if op[c['n']]['itself'])
        verdict = 'ALL' if n_self == len(cs) else 'NOT_ALL'
        want = EXPECTED_OPENS.get(name)
        ok = as_expected(verdict.lower().replace('_', ' '), want)
        put('opens_as_itself', verdict, f"{n_self} of {len(cs)} cuts open as themselves"
            + (f"; the others: {'; '.join(notself[:8])}" if notself else '')
            + f" (expected: {' or '.join(want) if isinstance(want, tuple) else want}{'' if ok else '; DIFFERS'})")
        if logdev:
            put('obs_zdb_with_log_device_copy', str(ck.with_logdev),
                'zdb calls that did not open the data device alone and opened it with the log device\'s copy beside it')
        stale_ok = bool(stale) <= (EXPECTED_STALE.get(name) == 'any')
        put('obs_stale_cuts', str(len(stale)), (', '.join(f"cut {n} (the state of cut {m})" for n, m in stale.items())
                                                 or 'no cut carries the state of an older cut')
            + f" (expected: {EXPECTED_STALE.get(name)}{'' if stale_ok else '; DIFFERS'})")
        if failures or not ok or not stale_ok:
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
        covered, order = outside_kstat(cs, ck.records, kstats)
        bad += order[:5]
        put('sessions', 'PASS' if not bad else 'FAIL', '; '.join(bad) if bad else
            f'{len(sv)} sessions as committed: {covered} of {len(cs) - len(stale)} cuts that are not stale within '
            f'the kstats\' windows, the other {len(cs) - len(stale) - covered} (the create\'s first group, the exports\', '
            'a group an import wrote before its history) in strictly increasing groups')
        if bad:
            rc = 1
        # A writable create or import syncs a group, with its flush,
        # before it returns (design §5.2).
        sb = synced_before_ready(cs, ck.records)
        if any(n is None for _, n, _ in sb):
            raise RigError('; '.join(d for _, n, d in sb if n is None))
        none = [d for _, n, d in sb if n == 0]
        put('import_syncs_before_return', 'PASS' if not none else 'FAIL',
            '; '.join(d for _, _, d in sb) if not none else '; '.join(none))
        if none:
            rc = 1
        # The controls.
        got = {}
        if mid in failures or not bps_at.get('mid'):
            raise RigError(f"the controls' cut {mid['n']} has no list of blocks (it failed or lists none)")
        bps = bps_at['mid']
        ecs, _ = cut_ref.cuts(ck.records, cut_ref.scan(ck.records, ck.read, dev), 'early')
        early = next(c for c in ecs if c['n'] == mid['n'])
        ck.advance(early['prefix'])
        failed, detail, _, _ = ck.judge(mid, op[mid['n']], kernel=False)
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
            failed, detail, _, _ = ck.judge(mid, op[mid['n']], kernel=False)
            r = ck.records[v]
            got['withheld'] = (control_verdict(failed), f"cut {mid['n']} without record {v} ({r['bytes']} bytes at "
                               f"{r['offset']}): {failed or 'passed'}: {detail}")
        # A later write over a block a cut reaches comes only once ZFS
        # reuses its space: the middle cut first, then earlier ones.
        later, tried = None, []
        for c in later_candidates(fresh, mid):
            cb = bps if c is mid else None
            if cb is None:
                ck.advance(c['prefix'])
                failed, _, _, cb = ck.copies()
                if failed:
                    continue
            tried.append(c['n'])
            v = pick_later(ck.records, c, cb)
            if v is not None:
                later = (c, v)
                break
        if later is None:
            got['later'] = ('not built', f"nothing after the instant overlaps a block that cuts {tried} reach")
        else:
            c, v = later
            sched = list(range(c['prefix'])) + [v]
            ck.at = None
            ck.advance(0)
            ck.advance(len(sched), sched)
            ck.at = None
            failed, detail, _, _ = ck.judge(c, op[c['n']], kernel=False)
            r = ck.records[v]
            got['later'] = (control_verdict(failed), f"cut {c['n']} (group {c['txg']}) with record {v} "
                            f"({'+'.join(r['flags']) or 'write'}, {r['bytes']} bytes at {r['offset']}, "
                            f"{v - c['prefix']} records after the instant): {failed or 'passed'}: {detail}")
        for rule in ('txg-alone', 'count-repeats'):
            rcs, _ = cut_ref.cuts(ck.records, cut_ref.scan(ck.records, ck.read, dev), rule)
            same = rule_same(cs, rcs, stale)
            if same:
                got[rule] = ('same', same)
                continue
            rsv = session_verdicts(rcs, ck.records, kstats)
            rbad = ([d for _, ok, d in rsv if not ok] + outside_kstat(rcs, ck.records, kstats)[1][:3]
                    + dropped_states(cs, rcs, op)[:3])
            got[rule] = ('caught' if rbad else 'missed', f'{len(rcs)} cuts against {len(cs)}: '
                         + ('; '.join(rbad) if rbad else 'every session as committed'))
        for ctl, (verdict, detail) in got.items():
            want = EXPECTED[ctl].get(name)
            ok = as_expected(verdict, want)
            want = ' or '.join(want) if isinstance(want, tuple) else want
            put(f'ctl_{ctl}', verdict.upper().replace(' ', '_'), f"{detail} (expected: {want}{'' if ok else '; DIFFERS'})")
            if not ok:
                rc = max(rc, 1)
    except RigError as e:
        put('rig', 'ERROR', str(e))
        rc = 2
    finally:
        ck_ = locals().get('ck')
        if ck_ is not None and ck_.dbgmsg_was is not None:
            with open(DBGMSG_ON, 'w') as f:
                f.write(ck_.dbgmsg_was)
        shutil.rmtree(os.path.join(rdir, 'work'), ignore_errors=True)
        with open(res, 'a') as f:
            f.write('\n'.join(lines) + '\n')
        with open(os.path.join(rdir, 'check.json'), 'w') as f:
            json.dump({'name': name, 'exit': rc, 'seconds': round(time.monotonic() - t0)}, f)
    sys.exit(rc)


if __name__ == '__main__':
    main()

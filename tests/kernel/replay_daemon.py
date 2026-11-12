#!/usr/bin/env python3
"""Power-cut replay of skyzfsd: the cut falls below the daemon (M1.T14).

replay-rig.sh with RIG_SKYZFSD records the daemon's own disk: the
volume's directory is on a file system (xfs) on dm-log-writes, and the
pool's only device is the ublk device that skyzfsd serves from that
directory, with no separate log device, so the intent log is on the
device under test as well:

    pool ── /dev/ublkbN ── skyzfsd serve ── volume dir (xfs) ── dm-log-writes ── lower disk

The write log is the lower disk's. A prefix of it is the disk after a
power cut at that point. For each prefix checked, this provider makes
the pool's device as the next boot would see it:

  1. the lower disk at the prefix, under a throwaway dm-snapshot;
  2. the file system mounted, which replays its journal as after a cut;
  3. `skyzfsd serve` started as after a reboot: an empty run directory
     and a boot id it has not seen (the test hook --boot-id-file), so
     the start is the real one: the log replayed, the acknowledged
     position checked (there is none in a new boot), a new device;
  4. the whole device read into an image, the daemon stopped, its
     device deleted, the file system unmounted, the snapshot removed.

The image then goes through the reference rig's own checks
(replay_check.py), unchanged. A prefix at which the file system does
not mount, the daemon does not start, the device cannot be read or the
daemon does not stop cleanly is a failure at that prefix, with what the
daemon said: a verdict, not an error of the rig.

Which prefixes:
  base     every prefix up to the first flush after recording began:
           the volume's file system had nothing synced yet since the
           mount (all of them, or an even sample of BASE_MAX)
  sync     each `zpool sync` mark: the files complete before that sync
           began must be there
  fsync    each file's fsync mark: the file must be there after the
           intent log is replayed (the fsync check, in a writable
           import of a throwaway snapshot of the image)
  flush    the lower disk's flush boundaries, an even sample in the
           time left
  between  prefixes between two flushes, at random, only where asked
           (--between), in the time left
  end      the whole log
Every checked prefix also gets the pool's transaction group, from
`zdb -u` on its image. The deep checks (the scrub and the every-copy
check) run at the first and the last checked prefix of each
transaction group seen. The deep checks' granularity is therefore that
of the prefixes checked, which the summary says.

The fsync marks map onto the lower disk's order because a file's fsync
returns only after ZFS's flush of the device returned, which returns
only after the daemon's sync of its log returned, which completes a
flush on the lower disk: dm-log-writes logs that flush before the
mark.

Negative controls, judged against EXPECTED (and the reference rig's
four controls, which pick pool blocks in a log of the pool's device,
are not built here; the summary says so):
  withheld        the lower disk's writes between two fsync marks are
                  held back until after the second mark: the disk
                  acknowledged the flushes of that fsync over data that
                  is not there. The file of that fsync is then due at
                  its mark and is not on the disk
Not built by default: recover-short, the start with the test hook
`--recover-short N`. The hook moves only the end of the log that is
compared with the acknowledged position, and after a reboot there is
none, so here it plants nothing (run 43279006013: with the hook and
without it, every start recovered the same operations). It is built
only when --recover-short N is given, and then a guard applies: a
hooked control whose starts recover as many operations as the starts
without the hook at the same prefixes is the rig's error, not a
control that was not caught. A hook that makes the recovery itself end
early (`--recover-drop`, not in the daemon yet) would be judged so.

Exit codes: 0 every check passed and every control is as the table
says; 1 a verdict against the daemon (a prefix failed, a control
differs, zdb finds fault with the final image); 2 the rig's own error
(a command of the rig failed, a prefix that should have been checked
was not, the witness disagrees), and no verdict against the daemon.

Needs root, ZFS with zdb, losetup, dmsetup with dm-snapshot, xfs, a
skyzfsd built with its test hooks, and `ublk` (the reference server's
tool) to delete the devices of the starts.
"""
import argparse
import json
import os
import random
import re
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time
import traceback
import uuid

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import logwrites  # noqa: E402
import replay_check as rc  # noqa: E402
from replay_check import files_at, fsync_marks, fsynced_at, flush_points, parse_marks, run, spread  # noqa: E402

BASE_MAX = 16
SNAP = 'skz-lower-snap'
READY_S = 120
STOP_S = 60
READY_RE = re.compile(r'^ready (/dev/ublkb(\d+))$', re.M)
TXG_RE = re.compile(r'^\s*txg = (\d+)\s*$', re.M)
START_RE = re.compile(r'recovery (\d+) ms of a log of (\d+) bytes and (\d+) operations')

# The stages at which a failure is a verdict: the reference rig's, and
# the provider's own steps that the daemon or the file system under it
# decide. Anything else is the rig failing.
JUDGING = rc.JUDGING | {'volume mount', 'start', 'device read', 'stop', 'deep: mount (writable)'}
CHECK_NAMES = dict(rc.CHECK_NAMES, **{
    'volume mount': "the volume's file system's mount",
    'start': "the daemon's start",
    'device read': 'reading the device',
    'stop': "the daemon's stop",
})

# What each control is expected to do (see replay_check.EXPECTED for the
# vocabulary). Both plant a fault that a power cut must not be allowed
# to cause, so both must be caught. Do not edit a control to fit.
EXPECTED = {
    'withheld': {'block': {'by': 'fsync'}},
}
# Only when asked for (--recover-short N); see the head of this file.
EXPECTED_HOOKED = {
    'recover-short': {'block': {'by': 'any'}},
}
RECOVER_SHORT_NOT_HERE = ('the hook --recover-short moves only the end of the log that is compared with the '
                          'acknowledged position, and after a reboot there is none, so it plants nothing here '
                          '(run 43279006013: the same operations recovered with it and without it)')
NOT_HERE = ("each picks pool blocks (labels, uberblocks, a file's block, an intent-log block) in a log of the pool's own "
            "device; this log is the lower disk's, whose writes are the volume file system's")
ABSENT = {name: NOT_HERE for name in ('drop', 'reorder', 'data', 'intent')}


class RigError(Exception):
    """The rig failed: no verdict either way."""


# ----------------------------------------------------------- pure logic
def txg_of(text):
    """`zdb -u` output -> the active uberblock's txg, or None."""
    m = TXG_RE.search(text)
    return int(m.group(1)) if m else None


def ready_device(text):
    """The serve's standard output -> (device path, number) once it said ready, else None."""
    m = READY_RE.search(text)
    return (m.group(1), int(m.group(2))) if m else None


def start_values(text):
    """The daemon's standard error -> what its start line says it recovered, or {}."""
    m = START_RE.search(text)
    return {'recovery_ms': int(m.group(1)), 'log_bytes': int(m.group(2)), 'operations': int(m.group(3))} if m else {}


def prefix_plan(records, base_max=BASE_MAX):
    """-> (kind by prefix of the prefixes that must be checked, flush prefixes to sample, counts).

    Required: the base prefixes (up to and with the first flush), every
    sync and fsync mark, and the end. A prefix that is several things
    takes the first of base, fsync, sync, end. The flush boundaries
    that are not required are sampled by the caller, as time allows.
    """
    total = len(records)
    flush = flush_points(records)
    first = flush[0] if flush else total
    base_all = list(range(0, first + 1))
    kind = {}
    for n in spread(base_all, base_max):
        kind[n] = 'base'
    for pos, _ in fsync_marks(records):
        kind.setdefault(pos, 'fsync')
    for pos, _, _ in parse_marks(records):
        kind.setdefault(pos, 'sync')
    kind.setdefault(total, 'end')
    rest = [f for f in flush if f not in kind]
    counts = {'records': total, 'prefixes': total + 1, 'flush': len(flush), 'base': len(base_all),
              'fsync': len(fsync_marks(records)), 'sync': len(parse_marks(records))}
    return kind, rest, counts


def between_points(records, kind, k, seed=1):
    """k random prefixes that are no flush boundary and not in `kind`, in order."""
    fset = set(flush_points(records))
    others = [n for n in range(1, len(records)) if n not in fset and n not in kind]
    return sorted(random.Random(seed).sample(others, min(max(k, 0), len(others))))


def withheld_schedule(records):
    """The `withheld` control -> dict (schedule, points, mark position, victims, what) or None.

    Among the fsync marks with no sync mark between them and the fsync
    mark before them, the middle one. Every write between the two marks
    is moved to just after the second: the flushes of that fsync are in
    the log, and the data they made durable is not on the disk yet. The
    file of that fsync is due at its mark (in the schedule's own order)
    and is checked there, with the flush boundaries before it.
    """
    fm = fsync_marks(records)
    sync_at = {p for p, _, _ in parse_marks(records)}
    cands = []
    for (p0, _), (p1, count) in zip(fm, fm[1:]):
        if any(p0 < s < p1 for s in sync_at):
            continue
        victims = [i for i in range(p0, p1 - 1) if rc.is_write(records[i])]
        if victims:
            cands.append((p0, p1, count, victims))
    if not cands:
        return None
    p0, p1, count, victims = cands[len(cands) // 2]
    mark = p1 - 1
    vs = set(victims)
    sched = [i for i in range(len(records)) if i not in vs]
    at = sched.index(mark) + 1
    sched[at:at] = victims
    new_p = at                                      # the prefix that ends with the mark, in the schedule
    flushes = [n for n in range(p0 + 1, new_p) if 'flush' in records[sched[n - 1]]['flags']]
    pts = sorted(set(spread(flushes, 3)) | {new_p})
    nbytes = sum(records[i]['bytes'] for i in victims)
    return {'name': 'withheld', 'schedule': sched, 'points': pts, 'mark': new_p, 'victim': victims[0],
            'victims': len(victims), 'file': count - 1,
            'what': f'the {len(victims)} writes ({nbytes >> 10} KiB) between fsync marks at records {p0 - 1} and {mark} '
                    f'are held back until after the second mark: the fsync of f.{count - 1} returned over data '
                    f'that the disk does not have'}


def hook_took(results):
    """Whether a hooked control's starts recovered less than the starts without the hook at the same prefixes.

    True when at least one did, False when every start that said both
    recovered the same, None when none said both.
    """
    pairs = [(x.get('operations_without_hook'), (x.get('start') or {}).get('operations')) for x in results]
    pairs = [(a, b) for a, b in pairs if a is not None and b is not None]
    if not pairs:
        return None
    return any(b < a for a, b in pairs)


def tally(results):
    """Results of a control's points -> (failures by judging stage, rig errors, first judging failure)."""
    by, rig_errors, first = {}, [], None
    for r in results:
        if r['ok']:
            continue
        if r['failed'] in JUDGING or r['failed'] == 'rig: zdb died with signal 6':
            key = 'copies' if r['failed'].startswith('rig: zdb') else r['failed'].split(': ')[0]
            by[key] = by.get(key, 0) + 1
            if first is None:
                first = {'n': r['n'], 'failed': r['failed'], 'detail': r['detail']}
        else:
            rig_errors.append({'n': r['n'], 'failed': r['failed'], 'detail': r['detail']})
    return by, rig_errors, first


def txg_points(points):
    """Points in prefix order, each with 'txg' -> the prefixes that are the first or last checked one of their txg.

    A point without a txg (its image could not be read for one) is
    passed over; the txgs on each side of it are still told apart.
    """
    seq = [(p['n'], p['txg']) for p in sorted(points, key=lambda p: p['n']) if p.get('txg') is not None]
    out = set()
    for k, (n, t) in enumerate(seq):
        if k == 0 or seq[k - 1][1] != t:
            out.add(n)
        if k == len(seq) - 1 or seq[k + 1][1] != t:
            out.add(n)
    return out


def verdict(failures, rig_errors, controls_ok, coverage_errors, witness_ok, zdb_cc_rc):
    """-> (exit code, what stands against the run). A verdict against the daemon comes before the rig's errors."""
    against, rig = [], []
    if failures:
        against.append(f'{len(failures)} prefix(es) failed a check')
    if not controls_ok:
        against.append('a control is not as the table has it')
    if zdb_cc_rc not in (0, None):
        against.append(f'zdb -bcc on the final image: exit code {zdb_cc_rc}')
    if zdb_cc_rc is None:
        rig.append('zdb -bcc did not run on the final image')
    if rig_errors:
        rig.append(f'the rig failed at {len(rig_errors)} prefix(es), first: {rig_errors[0]["failed"]}')
    if coverage_errors:
        rig.append('coverage: ' + '; '.join(coverage_errors))
    if not witness_ok:
        rig.append('the witness did not confirm the lower log provider')
    if against:
        return 1, against + rig
    if rig:
        return 2, rig
    return 0, []


# ------------------------------------------------------------- provider
class Daemon:
    """The lower disk at a prefix, and the pool's device that skyzfsd makes of it."""

    def __init__(self, a, idx):
        self.a = a
        self.idx = idx
        self.lower = os.path.join(a.work, 'lower.img')
        self.cow = os.path.join(a.work, 'lower-cow.img')
        self.mnt = os.path.join(a.work, 'lower-mnt')
        os.makedirs(self.mnt, exist_ok=True)
        self.at = 0
        self.schedule = None
        self.serve_args = list(a.serve_arg or [])

    def reset(self, schedule=None):
        if os.path.exists(self.lower):
            os.unlink(self.lower)
        rc_, out, _ = run(['cp', '--sparse=always', self.a.base, self.lower])
        if rc_:
            raise RigError(f'copying the lower base image failed: {out.strip()[-300:]}')
        self.at = 0
        self.schedule = schedule

    def advance(self, n):
        if n < self.at:
            raise RigError('the lower image only moves forward')
        t0 = time.monotonic()
        if n > self.at:
            try:
                logwrites.advance(self.a.log, self.idx, self.lower, self.at, n, self.schedule, self.a.discard)
            except (ValueError, OSError) as e:
                raise RigError(f'advancing the lower image {self.at}..{n}: {e}') from e
            self.at = n
        return time.monotonic() - t0

    def make(self, n, image, hooks=()):
        """The pool's device at prefix n, into `image` -> (failed stage or None, detail, info)."""
        info = {'t_advance': self.advance(n)}
        t0 = time.monotonic()
        loops = []
        res = (None, '')
        try:
            res = self._make(image, hooks, info, loops)
            return res[0], res[1], info
        finally:
            info['t_make'] = time.monotonic() - t0
            snap, mounted = info.pop('_snap', False), info.pop('_mounted', False)
            undo = []
            if mounted:
                rc_, out, _ = run(['umount', self.mnt], 120)
                if rc_:
                    undo.append(f'umount: {out.strip()[-200:]}')
            if snap:
                for _ in range(20):
                    rc_, out, _ = run(['dmsetup', 'remove', SNAP], 60)
                    if rc_ == 0:
                        break
                    time.sleep(0.25)
                if rc_:
                    undo.append(f'dmsetup remove: {out.strip()[-200:]}')
            for lo in loops:
                rc_, out, _ = run(['losetup', '-d', lo], 60)
                if rc_:
                    undo.append(f'losetup -d {lo}: {out.strip()[-200:]}')
            if os.path.exists(self.cow):
                os.unlink(self.cow)
            if undo:
                # A leftover makes every later prefix untrustworthy, so the
                # run stops; but a stage that failed before it is still this
                # prefix's result, and is reported as such (abort).
                why = f'prefix {n}: cleaning up after the start failed: ' + '; '.join(undo)
                if res[0] and res[0] in JUDGING:
                    info['abort'] = why
                else:
                    raise RigError(why)

    def _make(self, image, hooks, info, loops):
        """make()'s steps; it undoes what they set up -> (failed stage or None, detail)."""
        with open(self.cow, 'wb') as f:
            f.truncate(os.path.getsize(self.lower))
        for cmd in (['losetup', '-r', '--find', '--show', self.lower], ['losetup', '--find', '--show', self.cow]):
            rc_, out, _ = run(cmd, 60)
            if rc_:
                return rc.stage('rig: losetup', rc_), out.strip()[-300:]
            loops.append(out.strip())
        sectors = os.path.getsize(self.lower) // 512
        rc_, out, _ = run(['dmsetup', 'create', SNAP, '--table', f'0 {sectors} snapshot {loops[0]} {loops[1]} N 8'], 60)
        if rc_:
            return rc.stage('rig: dm-snapshot', rc_), out.strip()[-300:]
        info['_snap'] = True
        # The journal is replayed here, as at the first mount after a power cut.
        rc_, out, info['t_mount'] = run(['mount', '-o', 'nouuid', f'/dev/mapper/{SNAP}', self.mnt], 180)
        if rc_:
            return ('volume mount' if rc_ not in rc.RIG_RC else rc.stage('volume mount', rc_)), out.strip()[-400:]
        info['_mounted'] = True
        return self.serve(image, hooks, info)

    def serve(self, image, hooks, info):
        """Start the daemon as after a reboot, read its device, stop it -> (failed stage or None, detail)."""
        run_dir = tempfile.mkdtemp(prefix='skz-replay-run.', dir='/run')
        # Beside the run directory, not through it: removed with it below.
        boot = run_dir + '.boot'
        with open(boot, 'w') as f:
            f.write(str(uuid.uuid4()) + '\n')
        vol = os.path.join(self.mnt, self.a.volume)
        out_path, err_path = os.path.join(self.a.work, 'serve.out'), os.path.join(self.a.work, 'serve.err')
        cmd = ([self.a.skyzfsd, 'serve', '--dir', vol, '--run-dir', run_dir, '--boot-id-file', boot]
               + self.serve_args + list(hooks))
        state = {'number': None}
        before = ublk_numbers()
        t0 = time.monotonic()
        with open(out_path, 'w') as o, open(err_path, 'w') as e:
            p = subprocess.Popen(cmd, stdout=o, stderr=e)
        res = (None, '')
        try:
            res = self._serve(p, image, info, out_path, err_path, t0, state)
            return res
        finally:
            if p.poll() is None:
                p.kill()
                p.wait()
            info['said'] = said(err_path, 3)
            # The start's device, and any other that appeared while it ran:
            # a start that fails after its device was added leaves it.
            number = state['number']
            left = []
            for k in sorted((ublk_numbers() - before) | ({number} if number is not None else set())):
                rc_, out, _ = run([self.a.ublk, 'del', '-n', str(k)], 60)
                if rc_ and k in ublk_numbers():
                    left.append(f'ublk del -n {k}: {out.strip()[-200:]}')
            shutil.rmtree(run_dir, ignore_errors=True)
            if os.path.exists(boot):
                os.unlink(boot)
            if left:
                # As in make(): the run stops, and a stage that failed
                # first is still this prefix's result.
                if res[0] and res[0] in JUDGING:
                    info['abort'] = '; '.join(left)
                else:
                    raise RigError('; '.join(left))

    def _serve(self, p, image, info, out_path, err_path, t0, state):
        """serve()'s steps on the started daemon p -> (failed stage or None, detail)."""
        dev = None
        while time.monotonic() - t0 < READY_S:
            with open(out_path) as f:
                dev = ready_device(f.read())
            if dev and os.path.exists(dev[0]):
                break
            if p.poll() is not None:
                return 'start', f'exit {p.returncode} before it was ready: {said(err_path)}'
            time.sleep(0.05)
        else:
            return 'start', f'neither ready nor ended {READY_S} s after it was started: {said(err_path)}'
        info['t_start'] = time.monotonic() - t0
        state['number'] = dev[1]
        try:
            with open(err_path, errors='replace') as f:
                info['start'] = start_values(f.read())
        except OSError:
            info['start'] = {}
        if os.path.exists(image):
            os.unlink(image)
        t1 = time.monotonic()
        rc_, out, _ = run([self.a.dd, f'if={dev[0]}', f'of={image}', 'bs=1M',
                           'iflag=direct', 'conv=sparse', 'status=none'], 300)
        info['t_read'] = time.monotonic() - t1
        if rc_:
            if re.search(r'error writing|No space left', out):
                # dd could not write the image: the rig's disk, not the device.
                return 'rig: writing the image', out.strip()[-300:]
            return ('device read' if rc_ not in rc.RIG_RC else rc.stage('device read', rc_)), \
                f'{out.strip()[-300:]}; the daemon said: {said(err_path)}'
        with open(image, 'r+b') as f:
            f.truncate(self.a.dev_bytes)
        t1 = time.monotonic()
        p.send_signal(signal.SIGTERM)
        try:
            code = p.wait(STOP_S)
        except subprocess.TimeoutExpired:
            return 'stop', f'not ended {STOP_S} s after SIGTERM: {said(err_path)}'
        info['t_stop'] = time.monotonic() - t1
        if code != 0:
            return 'stop', f'exit {code} after SIGTERM: {said(err_path)}'
        return None, ''


def ublk_numbers():
    """The numbers of the ublk devices that exist, by their control nodes."""
    try:
        names = os.listdir('/dev')
    except OSError:
        return set()
    return {int(x[5:]) for x in names if x.startswith('ublkc') and x[5:].isdigit()}


def said(path, lines=2):
    """The last lines the daemon wrote to its standard error, on one line."""
    try:
        with open(path, errors='replace') as f:
            tail = [ln.strip() for ln in f.read().splitlines() if ln.strip()][-lines:]
    except OSError:
        return '(no output)'
    return ' | '.join(tail)[-600:] or '(nothing)'


# ------------------------------------------------------------- checking
class Checker:
    """The reference rig's checks on the images the daemon makes."""

    def __init__(self, a, idx, records):
        self.a = a
        self.records = records
        self.marks = parse_marks(records)
        self.fmarks = fsync_marks(records)
        self.fsync_at = {p for p, _ in self.fmarks}
        self.daemon = Daemon(a, idx)
        a.provider = ''
        self.rig = rc.Rig(a)
        # Two image directories: the image of the last prefix whose txg
        # is known stays while the next one is made, so that the last
        # prefix of a transaction group can still be checked once the
        # next prefix shows that the group has changed.
        self.dirs = [os.path.join(a.work, 'img-a'), os.path.join(a.work, 'img-b')]
        for d in self.dirs:
            os.makedirs(d, exist_ok=True)
        self.held = None                    # the point whose image is kept

    def due(self, n, marks=None, fmarks=None):
        marks = self.marks if marks is None else marks
        fmarks = self.fmarks if fmarks is None else fmarks
        return max(files_at(marks, n), fsynced_at(fmarks, n))

    def use(self, d):
        self.rig.imgdir = d
        self.rig.image = os.path.join(d, 'replay.img')

    def point(self, n, kind, hooks=(), marks=None, fmarks=None):
        """Make the image at n and run the light check, the fsync check where n ends with an fsync mark, and zdb -u."""
        marks = self.marks if marks is None else marks
        fmarks = self.fmarks if fmarks is None else fmarks
        free = next(d for d in self.dirs if self.held is None or d != self.held['dir'])
        self.use(free)
        # An image left from an earlier prefix must not be read as this one's.
        if os.path.exists(self.rig.image):
            os.unlink(self.rig.image)
        res = {'n': n, 'kind': kind, 'files': files_at(marks, n), 'dir': free}
        failed, detail, info = self.daemon.make(n, self.rig.image, hooks)
        res.update(info)
        if not failed:
            t0 = time.monotonic()
            failed, detail, res['t'] = self.rig.light(res['files'])
            res['t_light'] = time.monotonic() - t0
        fs = {p for p, _ in fmarks}
        if not failed and n in fs:
            res['fsynced'] = self.due(n, marks, fmarks)
            failed, detail, res['t_fsync'] = self.rig.deep(res['fsynced'], scrub=False)
            if failed:
                failed = 'fsync: files' if failed == 'fsync' else 'deep: ' + failed
        if not failed or failed in JUDGING:
            if os.path.exists(self.rig.image):
                rc_, out, res['t_txg'] = run(['zdb', '-e', '-p', free, '-u', self.a.pool], 120)
                res['txg'] = txg_of(out) if rc_ == 0 else None
                if res['txg'] is None and not failed:
                    res['txg_said'] = f'zdb -u exit {rc_}: {out.strip()[-200:]}'
        res['ok'] = failed is None
        if failed:
            res['failed'], res['detail'] = failed, detail
        return res

    def deep(self, p, marks=None, fmarks=None):
        """The scrub (with the fsync check) and the every-copy check on the image kept for point p."""
        self.use(p['dir'])
        failed, detail, p['t_deep'] = self.rig.deep(self.due(p['n'], marks, fmarks), scrub=True)
        p['deep'] = True
        if failed:
            failed = 'fsync: files' if failed == 'fsync' else 'deep: ' + failed
        else:
            failed, detail, p['t_copies'], p['copies'] = self.rig.copies()
        if failed and p['ok']:
            p['ok'], p['failed'], p['detail'] = False, failed, detail
        return p


def main():
    ap = argparse.ArgumentParser(description=__doc__.split('\n')[0])
    ap.add_argument('--log', required=True, help="the lower disk's dm-log-writes log")
    ap.add_argument('--base', required=True, help='the lower disk before the first record')
    ap.add_argument('--manifest', required=True)
    ap.add_argument('--pool', required=True)
    ap.add_argument('--mountpoint', required=True)
    ap.add_argument('--work', required=True)
    ap.add_argument('--out', required=True)
    ap.add_argument('--skyzfsd', required=True, help='skyzfsd built with its test hooks')
    ap.add_argument('--dd', default='dd', help='GNU dd')
    ap.add_argument('--ublk', default='ublk', help="the reference server's tool, to delete a start's device")
    ap.add_argument('--volume', required=True, help="the volume's directory, relative to the lower file system's root")
    ap.add_argument('--serve-arg', action='append', help='an option for every serve (repeat)')
    ap.add_argument('--dev-bytes', type=int, required=True, help="the pool's device size in bytes")
    ap.add_argument('--budget-s', type=float, default=600, help='seconds for the sampled flush boundaries')
    ap.add_argument('--budget-name', default='', help='what the budget is called in the summary')
    ap.add_argument('--between-budget-s', type=float, default=0,
                    help='seconds for random prefixes between two flushes (0: none)')
    ap.add_argument('--recover-short', type=int, default=0,
                    help='N of the recover-short control; 0 (the default) does not build it, see the head of this file')
    ap.add_argument('--recover-points', type=int, default=8, help='marks the recover-short control checks')
    ap.add_argument('--steps', default='', help='checkpoint steps the daemon took while recording')
    ap.add_argument('--seed', type=int, default=1)
    ap.add_argument('--replay-log', default='')
    ap.add_argument('--require-witness', action='store_true')
    ap.add_argument('--discard', choices=('zero', 'skip'), default='zero')
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    rc.CHECK_NAMES.update(CHECK_NAMES)
    try:
        code = check(a)
    except Exception as e:  # noqa: BLE001 -- whatever stops the rig is its own error, and says so
        why = what_stopped(e)
        print(f'::error title=replay rig (daemon)::the rig failed: {why}', flush=True)
        with open(os.path.join(a.out, 'replay-summary.md'), 'w') as f:
            f.write(f"## Power-cut replay through skyzfsd: the rig's own error, not a verdict\n\n{why}\n\nExit 2.\n")
        code = 2
    sys.exit(code)


def what_stopped(e):
    """An exception that stopped the rig, on one line: a RigError's own words, else its type, words and place."""
    if isinstance(e, RigError):
        return str(e)
    tb = traceback.extract_tb(e.__traceback__)
    where = f' at {os.path.basename(tb[-1].filename)}:{tb[-1].lineno}' if tb else ''
    return f'{type(e).__name__}: {e}{where}'


def classify(r, failures, rig_errors):
    """A failed point -> a verdict against the daemon (failures) or the rig's error (rig_errors)."""
    if r['failed'] in JUDGING or r['failed'] == 'rig: zdb died with signal 6':
        failures.append(r)
    else:
        rig_errors.append(r)


def abort_if(r):
    """Stop the run when cleaning up after this point failed: its own result is kept by the caller first."""
    if r.get('abort'):
        raise RigError(r['abort'])


def check(a):
    """The run; when it stops on the rig's error, what was found before is still reported, a failure first."""
    st = {'points': [], 'outside': [], 'res': {}}
    try:
        return _check(a, st)
    except Exception as e:  # noqa: BLE001 -- see main()
        why = what_stopped(e)
    failures, rig_errors = [], []
    for r in st['points'] + st['outside']:
        if not r['ok']:
            classify(r, failures, rig_errors)
    code = 1 if failures else 2
    res = dict(st['res'], stopped=why, exit=code,
               failures=[{k: v for k, v in p.items() if k != 'dir'} for p in failures],
               rig_errors=[{'n': p['n'], 'failed': p['failed'], 'detail': p['detail']} for p in rig_errors],
               points=[{k: v for k, v in p.items() if k != 'dir'} for p in st['points']])
    with open(os.path.join(a.out, 'replay-results.json'), 'w') as f:
        json.dump(res, f, indent=1, default=str)
    out = ['## Power-cut replay through skyzfsd: stopped by the rig\'s own error\n',
           f'The run stopped: {why}\n',
           f"Prefixes checked before it stopped: {len(st['points'])} in the main pass.\n"]
    if failures:
        out.append(f'Before it stopped, {len(failures)} prefix(es) failed a check. That is a verdict against the daemon, '
                   'whatever stopped the rig after it:\n')
        out.append('| prefix | kind | failed | detail |\n|---|---|---|---|')
        for r in failures:
            out.append(f"| {r['n']} | {r['kind']} | {r['failed']} | {r['detail'][:300]} |")
        out.append('\nExit 1: a verdict against the daemon.\n')
    else:
        out.append('No prefix had failed a check before it. Exit 2: the rig\'s own error, not a verdict.\n')
    text = '\n'.join(out)
    with open(os.path.join(a.out, 'replay-summary.md'), 'w') as f:
        f.write(text)
    print(text)
    print(f'::error title=replay rig (daemon)::the rig stopped: {why}', flush=True)
    return code


def _check(a, st):
    idx = logwrites.index(a.log, 0)
    records = idx['records']
    total = len(records)
    ck = Checker(a, idx, records)
    marks, fmarks = ck.marks, ck.fmarks
    res = {'mode': 'daemon', 'records': total, 'marks': len(marks), 'fsync_marks': len(fmarks), 'files': len(ck.rig.manifest),
           'write_bytes': sum(r['bytes'] for r in records if rc.is_write(r)), 'steps': a.steps,
           'budget_name': a.budget_name, 'budget_s': a.budget_s}
    st['res'] = res
    print(f"lower log: {total} records, {len(marks)} sync marks, {len(fmarks)} fsync marks, "
          f"{res['write_bytes'] >> 20} MiB written, {res['files']} files", flush=True)
    trouble = rc.manifest_trouble(marks, len(ck.rig.manifest))
    if trouble:
        raise RigError(trouble)
    if not fmarks:
        raise RigError('the log has no fsync marks')
    kind, rest, counts = prefix_plan(records)
    res['available'] = counts

    # Calibration: two flush boundaries, for the time a prefix takes.
    # They are timed for the plan, and a calibration point that fails is
    # counted like any other, whatever the main pass finds there later.
    ck.daemon.reset()
    points, failures, rig_errors = st['points'], [], []
    t_cal = []
    for n in spread(rest, 2) or spread(sorted(kind), 2):
        ck.held = None
        r = ck.point(n, 'calibration')
        if not r['ok']:
            st['outside'].append(r)
        abort_if(r)
        t_cal.append(r['t_make'] + r.get('t_light', 0) + r.get('t_txg', 0))
        print(f"calibration {n}: {'ok' if r['ok'] else 'FAILED ' + r['failed']} {t_cal[-1]:.2f}s "
              f"(make {r['t_make']:.2f}, light {r.get('t_light', 0):.2f}, zdb -u {r.get('t_txg', 0):.2f})", flush=True)
    per = statistics.mean(t_cal) if t_cal else 8.0
    sample = set(spread(rest, int(a.budget_s / per)))
    for n in sample:
        kind[n] = 'flush'
    betw = between_points(records, kind, int(a.between_budget_s / per), a.seed)
    for n in betw:
        kind[n] = 'between'
    res['calibration'] = {'points': len(t_cal), 't_point': per, 'flush_sampled': len(sample)}
    print(f'plan: {len(kind)} prefixes of {total + 1}: {sum(1 for k in kind.values() if k != "flush" and k != "between")} '
          f'required, {len(sample)} more flush boundaries for the budget of {a.budget_s:.0f} s at {per:.2f} s each, '
          f'{len(betw)} between flushes as time allows', flush=True)

    # Main pass, in prefix order on one lower image. The deep checks run
    # where the transaction group changes between two checked prefixes,
    # on both: the last of the old group (its image is kept) and the
    # first of the new.
    ck.daemon.reset()
    ck.held = None
    start = time.monotonic()
    skipped = 0
    deep_s = 0.0
    # Optional prefixes give way when the pass runs at more than twice
    # the planned time; the required ones never do.
    planned = len(kind) * per
    for n in sorted(kind):
        if kind[n] in ('flush', 'between') and time.monotonic() - start - deep_s > 2 * planned:
            skipped += 1
            continue
        r = ck.point(n, kind[n])
        points.append(r)
        abort_if(r)
        if r.get('txg') is not None:
            prev = ck.held
            if prev is None or prev['txg'] != r['txg']:
                t0 = time.monotonic()
                if prev is not None and not prev.get('deep') and prev['ok']:
                    ck.deep(prev)
                if r['ok']:
                    ck.deep(r)
                deep_s += time.monotonic() - t0
            ck.held = r
        if not r['ok']:
            print(f"FAILED prefix {n} ({r['kind']}): {r['failed']}: {r['detail']}", flush=True)
        else:
            print(f"  {n} {r['kind']}: txg {r.get('txg')}{' deep' if r.get('deep') else ''}, make {r['t_make']:.1f}s "
                  f"(start {r.get('t_start', 0):.1f}, read {r.get('t_read', 0):.1f}), light {r.get('t_light', 0):.1f}s"
                  + (f", fsync {r['t_fsync']:.1f}s" if 't_fsync' in r else '')
                  + f"; {time.monotonic() - start:.0f}s in", flush=True)
    if ck.held is not None and not ck.held.get('deep') and ck.held['ok']:
        ck.deep(ck.held)
    res['main_pass_s'] = time.monotonic() - start
    res['skipped_for_time'] = skipped
    for r in points + st['outside']:
        if not r['ok']:
            classify(r, failures, rig_errors)

    # Coverage: every required prefix checked, a txg for every one that
    # imported, and the deep checks at the first and last of each txg.
    cov = {'errors': [], 'warnings': []}
    checked = {r['n'] for r in points}
    required = {n for n, k in kind.items() if k in ('base', 'fsync', 'sync', 'end')}
    if required - checked:
        cov['errors'].append(f'{len(required - checked)} required prefixes were not checked')
    no_txg = [r for r in points if r['ok'] and r.get('txg') is None]
    if no_txg:
        cov['errors'].append(f"{len(no_txg)} prefixes passed and gave no transaction group (first: prefix {no_txg[0]['n']}: "
                             f"{no_txg[0].get('txg_said', '')})")
    ends = txg_points(points)
    deep_done = {r['n'] for r in points if r.get('deep')}
    healthy_ends = {r['n'] for r in points if r['n'] in ends and (r['ok'] or r.get('deep'))}
    if healthy_ends - deep_done:
        cov['errors'].append(f'the deep checks did not run at {len(healthy_ends - deep_done)} of {len(ends)} first and last '
                             f'prefixes of a transaction group (first: {min(healthy_ends - deep_done)})')
    txgs = sorted({r['txg'] for r in points if r.get('txg') is not None})
    flush_all = set(flush_points(records))
    cov.update(txgs=len(txgs), txg_first=min(txgs) if txgs else None, txg_last=max(txgs) if txgs else None,
               deep=len(deep_done), flush=len(flush_all), flush_checked=len(flush_all & checked),
               fsync_checked=len(ck.fsync_at & checked), between=sum(1 for r in points if r['kind'] == 'between'))
    if cov['flush_checked'] < cov['flush']:
        cov['warnings'].append(f"the light check ran at {cov['flush_checked']} of {cov['flush']} flush boundaries "
                               f"(an even sample for the budget)")
    res['coverage'] = cov
    for e in cov['errors']:
        print(f'::error title=replay rig (daemon), coverage::{e}', flush=True)

    # The lower log provider against xfstests replay-log.
    witness = []
    if a.replay_log:
        other = os.path.join(a.work, 'witness.img')
        between = [n for n in range(1, total) if rc.is_write(records[n - 1]) and rc.is_write(records[n])]
        for n, what in ((min(between, key=lambda n: abs(n - total // 2)) if between else total // 2, 'between two writes'),
                        (total, 'the end')):
            ck.daemon.reset()
            ck.daemon.advance(n)
            run(['cp', '--sparse=always', a.base, other])
            cmd = [a.replay_log, '--log', a.log, '--replay', other, '--limit', str(n)]
            rc_, out, _ = run(cmd + (['--no-discard'] if a.discard == 'skip' else []))
            same = rc_ == 0 and run(['cmp', '-s', ck.daemon.lower, other])[0] == 0
            witness.append({'n': n, 'rc': rc_, 'same': same, 'what': what, 'tail': out.strip()[-200:]})
            print(f"replay-log at prefix {n} ({what}): {'same image' if same else 'DIFFERENT'} (rc {rc_})", flush=True)
        if os.path.exists(other):
            os.unlink(other)
    res['witness'] = witness
    witness_ok, res['witness_verdict'] = rc.witness_verdict(witness, a.require_witness)

    # zdb's own checksum pass on the pool at the end of the log.
    ck.daemon.reset()
    ck.held = None
    r = ck.point(total, 'end')
    if not r['ok']:
        # Made a second time, for zdb: a failure here counts as at any prefix.
        r = dict(r, kind='end, for zdb -bcc')
        st['outside'].append(r)
        classify(r, failures, rig_errors)
    abort_if(r)
    if r['ok']:
        rc_, out, t = run(['zdb', '-e', '-p', r['dir'], '-bcc', a.pool], 300)
        found = [ln.strip() for ln in out.splitlines() if re.search(r'ASSERT|VERIFY|PANIC|rror|cannot|leak', ln)][:3]
        res['zdb_cc'] = {'n': total, 'rc': rc_, 's': t, 'said': ' | '.join(found)[:500]}
    else:
        res['zdb_cc'] = {'n': total, 'rc': None, 's': 0, 'said': f"the image was not made: {r['failed']}: {r['detail']}"}
    print(f"zdb -e -bcc at the end: {res['zdb_cc']['rc']}", flush=True)

    # Negative controls.
    ctl = []
    absent = dict(ABSENT)
    expected = dict(EXPECTED)
    if a.recover_short > 0:
        expected.update(EXPECTED_HOOKED)
        ctl.append(hooked_control(ck, points, cov, 'recover-short', ('--recover-short', str(a.recover_short)),
                                  spread(sorted(ck.fsync_at | {p for p, _, _ in marks}), a.recover_points),
                                  f'its recovery counts the log {a.recover_short} record(s) shorter'))
    else:
        absent['recover-short'] = RECOVER_SHORT_NOT_HERE
    w = withheld_schedule(records)
    if w is None:
        why = 'no two fsync marks in the log have no sync mark between them'
        absent['withheld'] = why
        cov['errors'].append(f'the withheld control could not be built: {why}')
    else:
        srec = [records[i] for i in w['schedule']]
        smarks, sfm = parse_marks(srec), fsync_marks(srec)
        ck.daemon.reset(w['schedule'])
        results = []
        for n in w['points']:
            ck.held = None
            results.append(ck.point(n, 'control', marks=smarks, fmarks=sfm))
            abort_if(results[-1])
        ctl.append(dict(control_row('withheld', w['what'], results), victim=w['victim'], victims=w['victims']))
    res['controls'] = ctl
    res['control_rows'], res['gaps'], controls_ok = rc.judge_controls(ctl, expected=expected, absent=absent)
    for row in res['control_rows']:
        if not row['match']:
            print(f"::error title=replay rig (daemon)::control {row['name']}: expected {row['expected']}, "
                  f"observed {row['observed']}", flush=True)

    res['points'] = [{k: v for k, v in p.items() if k != 'dir'} for p in points]
    res['failures'] = [{k: v for k, v in p.items() if k != 'dir'} for p in failures]
    res['rig_errors'] = [{'n': p['n'], 'failed': p['failed'], 'detail': p['detail']} for p in rig_errors]
    # A control the rig could not plant, or whose prefixes the rig failed
    # at, says nothing about the daemon: it is the rig's error, not a
    # control that differs from the table.
    unplanted = {c['name'] for c in ctl if c.get('rig_errors')}
    controls_ok = all(r['match'] for r in res['control_rows'] if r['name'] not in unplanted)
    ctl_errors = [dict(e, n=e['n'], failed=f"{c['name']}: {e['failed']}") for c in ctl for e in c.get('rig_errors', [])]
    code, against = verdict(failures, rig_errors + ctl_errors, controls_ok, cov['errors'], witness_ok,
                            res['zdb_cc']['rc'])
    res['exit'], res['against'] = code, against
    run(['cp', os.path.join(a.work, 'serve.err'), os.path.join(a.out, 'last-serve.err')])
    with open(os.path.join(a.out, 'replay-results.json'), 'w') as f:
        json.dump(res, f, indent=1)
    text = summary(res)
    with open(os.path.join(a.out, 'replay-summary.md'), 'w') as f:
        f.write(text)
    print(text)
    for x in against:
        print(f'the job fails: {x}', flush=True)
    return code


def hooked_control(ck, points, cov, name, hooks, pts, what):
    """A control that starts the daemon with a test hook at prefixes `pts` of the true log.

    With the guard: if every start recovered as many operations as the
    start without the hook at the same prefix, the fault was never
    planted, and that is the rig's error, not a control that was not
    caught.
    """
    ck.daemon.reset()
    results = []
    for n in pts:
        ck.held = None
        results.append(ck.point(n, 'control', hooks=hooks))
        abort_if(results[-1])
    normal = {p['n']: p.get('start', {}).get('operations') for p in points}
    for x in results:
        x['operations_without_hook'] = normal.get(x['n'])
    row = control_row(name, f"the start at {len(pts)} sync and fsync marks runs with {' '.join(hooks)}: {what}", results)
    took = hook_took(results)
    if took is not True:
        why = ('with the hook every start recovered as many operations as without it: the fault was not planted'
               if took is False else
               'no start with the hook said what it recovered beside one without it: the fault cannot be shown planted')
        row['rig_errors'].append({'n': results[0]['n'] if results else 0, 'failed': 'rig: the hook had no effect',
                                  'detail': why})
        cov['errors'].append(f'the {name} control: {why}')
    return row


def control_row(name, what, results):
    by, rig_errors, first = tally(results)
    print(f"control {name}: {'caught' if first else 'NOT CAUGHT'} over {len(results)} prefixes"
          + (f", first at {first['n']} by {first['failed']}: {first['detail'][:200]}" if first else '')
          + (f"; THE RIG FAILED at {len(rig_errors)}" if rig_errors else ''), flush=True)
    return {'name': name, 'what': what, 'points': len(results), 'caught': first is not None, 'first': first,
            'failed_by': by, 'rig_errors': rig_errors,
            'results': [{'n': r['n'], 'ok': r['ok'], 'failed': r.get('failed'), 'detail': r.get('detail', '')[:300],
                         'start': r.get('start'), 'operations_without_hook': r.get('operations_without_hook'),
                         'said': r.get('said')} for r in results]}


KIND_LABELS = (('base', 'before the first flush after recording began'), ('fsync', "ends with a file's fsync mark"),
               ('sync', 'ends with a sync mark'), ('flush', 'a flush boundary (sampled)'),
               ('between', 'between two flushes, at random'), ('end', 'the whole log'))


def mean(xs):
    return statistics.mean(xs) if xs else float('nan')


def summary(res):
    pts = res['points']
    av = res['available']
    cov = res['coverage']
    out = ['## Power-cut replay through skyzfsd\n']
    out.append('The cut is below the daemon: the volume\'s file system is on dm-log-writes, and every prefix checked is '
               'the lower disk after a power cut there, its file system mounted (the journal replayed) and the daemon '
               'started as after a reboot (an empty run directory and a new boot id). The pool is in the whole-pool '
               'layout: the intent log is on the device under test.\n')
    out.append(f"Lower write log: {res['records']} records, {res['write_bytes'] >> 20} MiB written, {av['flush']} flushes, "
               f"{res['marks']} sync marks, {res['fsync_marks']} fsync marks, {res['files']} files. "
               f"Checkpoint steps the daemon took while recording: **{res['steps'] or 'not said'}**.\n")
    budget = res.get('budget_name') or 'given'
    out.append(f"Budget: {budget}, {res['budget_s']:.0f} s for the sampled flush boundaries.\n")
    fails = res['failures']
    word = ('every checked prefix passed' if not fails else f"**{len(fails)} prefixes FAILED**")
    out.append(f"Result: {word}. Main pass {res['main_pass_s']:.0f} s"
               + (f", {res['skipped_for_time']} planned prefixes skipped for time" if res['skipped_for_time'] else '') + '.\n')
    out.append('| kind of prefix | in the log | checked | failed |')
    out.append('|---|---|---|---|')
    have = {'base': av['base'], 'fsync': av['fsync'], 'sync': av['sync'], 'flush': av['flush'],
            'between': av['prefixes'] - av['flush'], 'end': 1}
    for k, label in KIND_LABELS:
        sel = [p for p in pts if p['kind'] == k]
        out.append(f"| {label} | {have[k]} | {len(sel)} | {sum(1 for p in sel if not p['ok'])} |")
    main = [p for p in pts if p['kind'] != 'control']
    if main:
        def part(k):
            return mean([p[k] for p in main if k in p])
        out.append(f"\nTime per prefix: {mean([p['t_make'] for p in main]):.2f} s to make the image (advance "
                   f"{part('t_advance'):.2f}, mount {part('t_mount'):.2f}, the daemon's start {part('t_start'):.2f}, "
                   f"reading its device {part('t_read'):.2f}, its stop {part('t_stop'):.2f}), then the light check "
                   f"{part('t_light'):.2f} s and `zdb -u` {part('t_txg'):.2f} s; the fsync check {part('t_fsync'):.2f} s "
                   f"where it ran.")
        st = [p['start'] for p in main if p.get('start')]
        if st:
            out.append(f"\nThe daemon's starts recovered logs of {min(x['log_bytes'] for x in st) >> 20} to "
                       f"{max(x['log_bytes'] for x in st) >> 20} MiB, {min(x['operations'] for x in st)} to "
                       f"{max(x['operations'] for x in st)} operations, in {min(x['recovery_ms'] for x in st)} to "
                       f"{max(x['recovery_ms'] for x in st)} ms ({len(st)} starts).")
    out.append(f"\nTransaction groups seen: {cov.get('txgs', 0)} (txg {cov.get('txg_first')} to {cov.get('txg_last')}). "
               f"The deep checks (a scrub in a writable import with the fsync check, and the every-copy check) ran at "
               f"{cov.get('deep', 0)} prefixes: the first and the last checked prefix of each group. Their granularity is "
               f"that of the prefixes checked: {cov.get('flush_checked', 0)} of {cov.get('flush', 0)} flush boundaries, "
               f"every mark, and {cov.get('between', 0)} prefixes between flushes.")
    deep = [p for p in main if p.get('deep')]
    if deep:
        out.append(f"Scrub {mean([p['t_deep'] for p in deep]):.2f} s and every-copy check "
                   f"{mean([p['t_copies'] for p in deep if 't_copies' in p]):.2f} s on average.")
    out.append(f"\nFsync check: at {cov.get('fsync_checked', 0)} of {av['fsync']} fsync marks (all are required).")
    z = res.get('zdb_cc') or {}
    out.append(f"\n`zdb -e -bcc` on the pool at the end of the log: exit code {z.get('rc')}"
               + (f" — **{z.get('said')}**" if z.get('rc') else '') + '.')
    for e in cov['errors']:
        out.append(f'\n> [!CAUTION]\n> Coverage: {e}.')
    for w in cov['warnings']:
        out.append(f'\n> [!NOTE]\n> Coverage: {w}.')
    if res.get('witness'):
        out.append('\nLower log provider against xfstests `replay-log`: '
                   + '; '.join(f"prefix {w['n']} ({w['what']}): {'same image' if w['same'] else 'DIFFERENT'}"
                               for w in res['witness']) + '.')
    else:
        out.append(f"\nLower log provider against xfstests `replay-log`: **{res.get('witness_verdict')}**.")
    base = [p for p in pts if p['kind'] == 'base']
    if base:
        bad = [p for p in base if not p['ok']]
        out.append(f"\nBase prefixes (the lower disk before its first flush since recording began): {len(base)} of "
                   f"{av['base']} checked, " + (f"**{len(bad)} failed**, first at {bad[0]['n']}: {bad[0]['failed']}: "
                                                f"{bad[0]['detail'][:300]}" if bad else 'all passed') + '.')
    out.append('\n### Negative controls\n')
    out.append('| control | what was done | prefixes | caught | first failure |')
    out.append('|---|---|---|---|---|')
    for c in res['controls']:
        first = f"prefix {c['first']['n']}: {c['first']['failed']}: {c['first']['detail'][:200]}" if c['first'] else '–'
        if c.get('rig_errors'):
            first += f"; **the rig failed** at {len(c['rig_errors'])} prefixes, first {c['rig_errors'][0]['failed']}"
        by = ', '.join(f'{k} {v}' for k, v in c['failed_by'].items())
        out.append(f"| {c['name']} | {c['what']} | {c['points']} | {'yes, ' + by if c['caught'] else 'no'} | "
                   f"{first.replace('|', '/')} |")
    for c in res['controls']:
        ops = [(x['n'], x.get('operations_without_hook'), (x.get('start') or {}).get('operations'))
               for x in c.get('results', []) if x.get('operations_without_hook') is not None]
        if ops:
            out.append(f"\n`{c['name']}`, operations recovered at each prefix without the hook and with it: "
                       + ', '.join(f'{n}: {a} and {b}' for n, a, b in ops) + '.')
    out.append('\nAgainst `EXPECTED` in `replay_daemon.py` (a difference fails the job):\n')
    out.append('| control | expected | observed | |')
    out.append('|---|---|---|---|')
    for r in res['control_rows']:
        out.append(f"| {r['name']} | {r['expected']} | {r['observed']} | {'as expected' if r['match'] else '**DIFFERS**'} |")
    why = {}
    for r in res['control_rows']:
        if r.get('not_built'):
            why.setdefault(r['not_built'], []).append(f"`{r['name']}`")
    for text, names in why.items():
        out.append(f"\n{', '.join(names)} {'was' if len(names) == 1 else 'were'} not built: {text}.")
    if fails:
        out.append('\n### Failures (findings on the daemon or the stack under it)\n')
        for p in fails[:25]:
            out.append(f"- prefix {p['n']} ({p['kind']}): {p['failed']}: {p['detail'][:400]}")
    if res.get('rig_errors'):
        out.append("\n### The rig's own errors (no verdict)\n")
        for p in res['rig_errors'][:10]:
            out.append(f"- prefix {p['n']}: {p['failed']}: {p['detail'][:300]}")
    out.append(f"\nExit {res['exit']}: " + ('; '.join(res['against']) if res['against'] else 'passed') + '.')
    return '\n'.join(out) + '\n'


if __name__ == '__main__':
    main()

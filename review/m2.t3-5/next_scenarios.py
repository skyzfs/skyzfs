#!/usr/bin/env python3
"""#130 at 09860a0: the first look's F1 and F2 scenarios again, and
scenarios for what a9222c4 and 09860a0 changed.

    next_scenarios.py CHECKOUT

Nothing here needs a kernel or ZFS. `zpool`, `zfs`, `losetup` and
`dmsetup` are stood in where a scenario says so; `fuser`, `ps` and the
signals are the real ones of the machine this runs on.
"""
import argparse
import contextlib
import io
import os
import shutil
import subprocess
import sys
import tempfile
import time

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests', 'kernel'))
import replay_check as rc   # noqa: E402

LEFT = 'rig: a pool named skzreplay is still imported'
work = tempfile.mkdtemp(prefix='scn2-')
manifest = os.path.join(work, 'manifest')
with open(manifest, 'w') as f:
    f.write('0' * 64 + ' f.0\n')
rig = rc.Rig(argparse.Namespace(provider='', work=os.path.join(work, 'w'), manifest=manifest, pool='skzreplay',
                                mountpoint='/skz', base=''))
with open(rig.image, 'wb') as f:
    f.truncate(64 << 20)
rig.advance = lambda n: 0.0

BAD_SCRUB = ' scan: scrub repaired 0B in 00:00:01 with 3 errors on Tue\n\tskzreplay DEGRADED 0 0 3\nerrors: 3 data errors\n'
GOOD_SCRUB = (' scan: scrub repaired 0B in 00:00:01 with 0 errors on Tue\n\tskzreplay ONLINE 0 0 0\n'
              'errors: No known data errors\n')
st = {}


def standin(cmd, timeout=300):
    """zpool, zfs, losetup and dmsetup as the scenario in `st` has them."""
    st.setdefault('calls', []).append(cmd)
    if cmd[:2] == ['zpool', 'list']:
        return (0, 'skzreplay\n', 0.0) if st.get('imported') else (1, "cannot open 'skzreplay': no such pool\n", 0.0)
    if cmd[:2] == ['zpool', 'import']:
        if st.get('imported'):
            return 1, 'a pool with that name already exists\n', 0.0
        st['imported'] = True
        st['writable'] = any(a.startswith('/dev/mapper/') for a in cmd)
        return 0, '', 0.0
    if cmd[:2] == ['zpool', 'export']:
        busy = st.get('busy')
        if busy is not None and busy(cmd):
            return 1, "cannot export 'skzreplay': pool is busy\n", 0.0
        st['imported'] = False
        return 0, '', 0.0
    if cmd[:2] == ['zpool', 'scrub']:
        return 0, '', 0.0
    if cmd[:2] == ['zpool', 'status']:
        if 'status' in st.get('fail', ()):
            return 1, 'status: internal error\n', 0.0
        return 0, BAD_SCRUB if 'scrub' in st.get('fail', ()) else GOOD_SCRUB, 0.0
    if cmd[:2] == ['zfs', 'mount']:
        if 'mount' in st.get('fail', ()):
            return 1, "cannot mount 'skzreplay': Input/output error\n", 0.0
        return 0, '', 0.0
    if cmd[0] == 'losetup' and '--find' in cmd:
        return 0, '/dev/loop9\n', 0.0
    if cmd[:2] == ['losetup', '-d'] and ('detach' in st.get('fail', ()) and (st.get('writable') or 'deep' not in st)):
        return 1, 'losetup: /dev/loop9: detach failed: Device or resource busy\n', 0.0
    if cmd[:2] == ['dmsetup', 'remove'] and 'dmremove' in st.get('fail', ()):
        return 1, 'device-mapper: remove ioctl failed: Device or resource busy\n', 0.0
    if cmd[0] in ('fuser', 'ps'):
        return 0, '', 0.0
    return 0, '', 0.0


def always_busy(cmd):
    return True


def deep_busy(cmd):
    """Only the export of the writable import (the deep check's) is refused."""
    return bool(st.get('writable'))


@contextlib.contextmanager
def stood_in():
    real_run, real_sleep = rc.run, rc.time.sleep
    rc.run, rc.time.sleep = standin, (lambda s: st.setdefault('slept', []).append(s))
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            yield
    finally:
        rc.run, rc.time.sleep = real_run, real_sleep


def fresh(**kw):
    st.clear()
    st.update(kw)
    rig.busy, rig.forced, rig.busy_throughout, rig.exports = [], [], 0, 0


print('== N1. F1: a verdict, then a failed cleanup')
fresh(busy=always_busy)
with stood_in():
    pts = [rig.point(n, 1, False) for n in (100, 101, 102)]
fails = [p for p in pts if not p['ok']]
print('  C1 again: verify fails at 100, the export busy throughout and -f refused:',
      [(p['n'], p['failed']) for p in fails])
print(f"     its detail: {fails[0]['detail'][:90]!r}")
ctl = [{'name': n, 'class': 'block', 'caught': False, 'failed_by': {},
        'rig_errors': [{'n': 5, 'failed': LEFT, 'detail': ''}]} for n in ('data', 'drop', 'reorder')]
rows, _, cok = rc.judge_controls(ctl, absent={'intent': 'not on the device'})
ok, _ = rc.job_verdict(fails, cok, True, ['coverage short'], 0)
print(f'     exit_code = {rc.exit_code(ok, fails, ctl, rows, True, 0)} (it was 2 at c205682)')

cases = [
    ('light: mount fails, then the export', dict(fail={'mount'}, busy=always_busy), 'light', 1),
    ('light: verify fails, then losetup -d', dict(fail={'detach'}), 'light', 1),
    ('light: passes, then the export fails', dict(busy=always_busy), 'light', 0),
    ('deep: scrub finds errors, then the export', dict(fail={'scrub'}, busy=deep_busy), 'deep', True),
    ('deep: scrub finds errors, then dmsetup remove', dict(fail={'scrub', 'dmremove'}), 'deep', True),
    ('deep: scrub finds errors, then losetup -d', dict(fail={'scrub', 'detach'}, deep=True), 'deep', True),
    ('deep: fsync files missing, then the export', dict(busy=deep_busy), 'fsync', None),
    ('deep: zpool status fails (the rig), then the export', dict(fail={'status'}, busy=deep_busy), 'deep', True),
    ('deep: passes, then the export fails', dict(busy=deep_busy), 'deep', True),
]
for what, kw, kind, arg in cases:
    fresh(**kw)
    with stood_in():
        if kind == 'light':
            r = rig.point(200, arg, False)
        elif kind == 'fsync':
            r = rig.point(200, 0, 'fsync', fsynced=1)
        else:
            r = rig.point(200, 0, True)
        after = rig.point(201, 0, False)
    got = r.get('failed')
    print(f"  {what}: {got!r} ({'a verdict' if got in rc.JUDGING else 'the rig'}); the next prefix: "
          f"{after.get('failed')!r}")

print('\n== N2. F2: which differing controls the rig errors excuse')
left = {'n': 7, 'failed': LEFT, 'detail': ''}
by, errs, first = rc.tally_control([{'n': 5, 'ok': False, 'failed': 'copies: differ', 'detail': 'x'},
                                    {'n': 6, 'ok': False, 'failed': 'export', 'detail': 'busy'},
                                    {'n': 7, 'ok': False, 'failed': LEFT, 'detail': ''}])
c2 = [{'name': 'drop', 'class': 'superseded', 'caught': first is not None, 'failed_by': by, 'rig_errors': errs}]
rows2, _, ok2 = rc.judge_controls(c2, absent={'intent': 'x', 'data': 'x', 'reorder': 'x'})
print(f'  C2 again: a superseded drop caught at 5, the pool left at 7: exit_code = '
      f'{rc.exit_code(ok2, [], c2, rows2, True, 0)} (it was 2)')
c3 = [{'name': 'reorder', 'class': 'superseded', 'caught': False, 'failed_by': {}, 'rig_errors': errs[1:]}]
rows3, _, _ = rc.judge_controls(c3, absent={'intent': 'x', 'data': 'x', 'drop': 'x'})
print(f"  C2b again: reorder of a class the table does not name, pool left: exit_code = "
      f"{rc.exit_code(False, [], c3, rows3, True, 0)} (it was 2)")
print('  Every row of the table, with rig errors (a pool left at the control\'s last prefix):')
for name, classes in rc.EXPECTED.items():
    for cls, want in classes.items():
        for got, failed_by in (('not caught', {}), ('caught by the table\'s check', {want['by']: 1}),
                               ('caught by another check', {'verify' if want['by'] != 'verify' else 'copies': 1})):
            if want['by'] in (None, 'any', 'either') and got == 'caught by the table\'s check':
                continue
            if want['by'] in ('any', 'either') and got == 'caught by another check':
                failed_by = {'verify': 1}
            c = [{'name': name, 'class': cls, 'caught': bool(failed_by), 'failed_by': failed_by, 'rig_errors': [left]}]
            others = {k: 'x' for k in rc.EXPECTED if k != name}
            rows, _, okc = rc.judge_controls(c, absent=others)
            print(f"    {name:8} {cls:17} expected {rows[0]['expected']!r:42} {got:28} -> exit "
                  f"{rc.exit_code(okc, [], c, rows, True, 0)}")
# The residual: reorder (the table: by the every-copy check) judged at
# every point where that check ran without a catch; the export failed
# busy at 19 and the pool was left for 20; neither is a copies point.
res = [{'n': n, 'ok': True} for n in range(10, 19)]
res += [{'n': 19, 'ok': False, 'failed': 'export', 'detail': 'pool is busy'},
        {'n': 20, 'ok': False, 'failed': LEFT, 'detail': ''}]
by, errs, first = rc.tally_control(res)
c4 = [{'name': 'reorder', 'class': 'block', 'caught': first is not None, 'failed_by': by, 'rig_errors': errs}]
rows4, _, ok4 = rc.judge_controls(c4, absent={'intent': 'x', 'data': 'x', 'drop': 'x'})
print(f"  N2a reorder, copies run at 10 and 15 and not caught, the export busy at 19 (light only), left at 20: "
      f"exit_code = {rc.exit_code(ok4, [], c4, rows4, True, 0)}")

print("\n== N3. The lead's rule: zdb's signal 6 in the main pass")
for f in (rc.ZDB_ABORT, 'rig: zdb died with signal 9', 'rig: zdb died with signal 11', 'rig: zdb timed out',
          'rig: zdb could not start'):
    print(f"  main pass {f!r} with a pool left: exit_code = {rc.exit_code(False, [{'n': 3, 'failed': f}, {'n': 4, 'failed': LEFT}], [], [], True, 0)}")
for z in (-6, -9, -11, 124, 127, 1):
    print(f"  zdb -bcc at the end rc {z}, a pool left, nothing else: exit_code = "
          f"{rc.exit_code(False, [{'n': 4, 'failed': LEFT}], [], [], True, z)}")
t0 = time.monotonic()
r = rc.run(['sleep', '5'], 1)
print(f'  run() on a command past its limit: rc {r[0]} after {time.monotonic() - t0:.1f} s (so the rig\'s own timeout is '
      f'never a signal)')
r = rc.run(['sh', '-c', 'kill -ABRT $$'], 10)
print(f"  a SIGABRT from outside zdb's assertions: run() says rc {r[0]}; every_copy_verdict says "
      f"{rc.every_copy_verdict(r[0], r[1], rig.image)[:2]}")
said = 'lt-zdb: ../../module/zfs/arc.c:1234: Assertion `x\' failed.\nASSERT at module/zfs/dbuf.c:99:x()\n'
print(f"  with an assertion line printed: {rc.every_copy_verdict(-6, said, rig.image)[:2]}")

print('\n== N4. F3: what Rig.held records on this machine')
mp = rig.alt + rig.a.mountpoint
os.makedirs(mp, exist_ok=True)
fresh()
with contextlib.redirect_stdout(io.StringIO()) as printed:
    rig.held("cannot export 'skzreplay': pool is busy", 1)
b = rig.busy[0]
print(f"  the pool's mount point a plain directory: mounted {b['mounted']}, fuser {b['fuser']!r}, holders "
      f"{b['holders']}, ZFS commands {b['zfs_processes']}")
print('  its log line: ' + printed.getvalue().strip().replace(rig.alt, 'ALT')[:200])
for path in ('/proc', '/'):
    out = rc.run(['fuser', '-vm', path], 30)[1]
    print(f"  real `fuser -vm {path}` through run() ({len(out.splitlines())} lines; the first two: "
          f"{[ln.split() for ln in out.splitlines()[:2]]}):")
    print(f"    fuser_holders keeps {len(rc.fuser_holders(out))} processes, the first "
          f"{[h.split(' (')[0] for h in rc.fuser_holders(out)[:3]]}; header or kernel mount kept: "
          f"{any('USER' in h or 'kernel' in h for h in rc.fuser_holders(out))}")
bindir = os.path.join(work, 'bin')
os.makedirs(bindir)
for name in ('zdb', 'zpool', 'zfs_ids_to_path_x', 'mount.zfs'):
    shutil.copy('/bin/sleep', os.path.join(bindir, name))
procs = [subprocess.Popen([os.path.join(bindir, n), '30']) for n in ('zdb', 'zpool', 'zfs_ids_to_path_x', 'mount.zfs')]
spaced = subprocess.Popen([sys.executable, '-c',
                           'import ctypes,time; ctypes.CDLL(None).prctl(15, b"zed watcher", 0, 0, 0); time.sleep(30)'])
argsonly = subprocess.Popen(['sh', '-c', 'sleep 30; : zpool export skzreplay'])
time.sleep(0.5)
try:
    ps = subprocess.run(['ps', '-eo', 'pid=,etimes=,comm=,args='], capture_output=True, text=True, timeout=30).stdout
    found = rc.zfs_processes(ps)
    mine = {p.pid for p in procs} | {spaced.pid, argsonly.pid}
    print('  real `ps` with stand-ins running (zdb, zpool, a 17-character name, mount.zfs, a comm with a space '
          '"zed watcher", a shell whose arguments say zpool):')
    for line in found:
        pid = int(line.split(',')[0].split()[1])
        if pid in mine:
            print(f'    matched: {line.replace(bindir, "BIN")}')
    print(f"    their comm as ps prints it: "
          f"{[ln.split(None, 3)[2] for ln in ps.splitlines() if ln.split() and int(ln.split()[0]) in mine]}")
    kthreads = [ln.split(None, 3)[2] for ln in ps.splitlines() if len(ln.split()) > 3 and ln.split(None, 3)[3].startswith('[')]
    print(f"    kernel threads matched: {[k for k in kthreads if k in rc.ZFS_COMMANDS]} (of {len(kthreads)})")
finally:
    for p in procs + [spaced, argsonly]:
        p.kill()
        p.wait()

print('\n== N5. F4 and S4: the count, and the cap on waiting')


def run_exports(cap, holds):
    """`holds`: per export, how many plain attempts are refused, and whether -f is (`f`), or 'clock' for a hold
    that clears once a 1 s wait has passed."""
    fresh()
    real_cap = rc.EXPORT_BUSY_THROUGHOUT_MAX
    rc.EXPORT_BUSY_THROUGHOUT_MAX = cap
    out = []
    try:
        for h in holds:
            seen = {'plain': 0}

            def busy(cmd, h=h, seen=seen):
                if h == 'clock':
                    return not st.get('slept')
                n, f_refused = h
                if '-f' in cmd:
                    return f_refused
                seen['plain'] += 1
                return seen['plain'] <= n
            st.update(imported=True, busy=busy, slept=[])
            with stood_in():
                rcode, _ = rig.export()
            out.append(rcode)
        return out
    finally:
        rc.EXPORT_BUSY_THROUGHOUT_MAX = real_cap


holds = [(5, False), (5, False), 'clock']
print(f"  two exports busy through every wait whose -f then worked, then one busy for under a second:")
for cap in (rc.EXPORT_BUSY_THROUGHOUT_MAX, 10 ** 9):
    codes = run_exports(cap, holds)
    print(f"    cap {cap if cap < 1000 else 'none'}: export exit codes {codes}; pool left imported: {st.get('imported')}; "
          f"forced {[(f['export'], f['rc'], f['waited']) for f in rig.forced]}")
    print('      ' + ''.join(rc.rig_lines({'export_busy': rig.busy, 'export_forced': rig.forced})).strip()[:330])
shutil.rmtree(work)

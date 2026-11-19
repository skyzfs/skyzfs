#!/usr/bin/env python3
"""#130 at a3b37ba: M1 to M4 driven.

    next_scenarios3.py CHECKOUT

Y1: zdb -bcc's exit code on the daemon jobs' normal end and on their
stopped path. Y2: the prefix a busy export's record names in a deep
check. Y3: the stopped path's record and line. Y4: classify and tally
over zdb's failures. Y5: plant L06 as written, on a normal end with exit 2,
in a copy of tests/kernel. Nothing needs a kernel or ZFS.
"""
import contextlib
import io
import json
import os
import shutil
import subprocess
import sys
import tempfile

ROOT = sys.argv[1]
NEW = os.path.join(ROOT, 'tests', 'kernel')
sys.path.insert(0, NEW)
import replay_check as rc       # noqa: E402
import replay_daemon as rd      # noqa: E402
import test_replay_daemon as trd  # noqa: E402

print("== Y1. zdb -bcc's exit code: the daemon judge's normal end, and its stopped path (M1)")
print(f"  {'rc':>5} | {'normal end':>10} | {'stopped path':>12} | what the stopped summary says")
for z in (0, 1, 124, 127, -6, -7, -9, -11, -15):
    d = tempfile.mkdtemp(prefix='y1-')

    def fake(a, st, z=z):
        st['res'] = {'zdb_cc': {'n': 10, 'rc': z, 's': 1.0, 'said': ''}}
        raise rd.RigError('the stuck control at prefix 5: dd did not end')
    real = rd._check
    rd._check = fake
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            code = rd.check(type('A', (), {'out': d})())
    finally:
        rd._check = real
    with open(os.path.join(d, 'replay-summary.md')) as f:
        text = f.read()
    said = ('"zdb -e -bcc ... failed ... a verdict against the daemon"' if 'zdb -e -bcc` on the pool at the end of the '
            'log failed' in text else 'the rig\'s error only')
    normal = rd.verdict([], [{'n': 5, 'failed': 'rig: the stuck control'}], True, [], True, z)[0]
    print(f"  {z:>5} | {normal:>10} | {code:>12} | {said}")
    shutil.rmtree(d)

print("\n== Y2. The prefix a busy export's record names, with one inside every deep check (M2)")
t = trd.Pass('test_a_clean_pass')
t.setUp()
try:
    real_deep = rd.Checker.deep          # before the fixture replaces it
    t.fake()
    fpoint, fdeep = rd.Checker.point, rd.Checker.deep
    deep_of = []

    def point(ck, n, kind, *a, **k):
        ck.rig.at = n                   # as the real Checker.point does before it makes the image
        return fpoint(ck, n, kind, *a, **k)

    def deep(ck, p, marks=None, fmarks=None):
        # The real Checker.deep, with the Rig's scrub and every-copy check replaced by one busy export
        rig = ck.rig
        saved = rig.imgdir, rig.image

        def rig_deep(fsynced, scrub=True):
            with contextlib.redirect_stdout(io.StringIO()):
                rig.held("cannot export 'p': pool is busy", 1)
            return None, '', 1.0
        rig.deep, rig.copies = rig_deep, (lambda: (None, '', 1.0, {}))
        try:
            real_deep(ck, p, marks, fmarks)
        finally:
            del rig.deep, rig.copies
            rig.imgdir, rig.image = saved
        deep_of.append(p['n'])
        return fdeep(ck, p, marks, fmarks)
    rd.Checker.point, rd.Checker.deep = point, deep
    with contextlib.redirect_stdout(io.StringIO()):
        code = rd.check(t.args())
    with open(os.path.join(t.dir.name, 'out', 'replay-results.json')) as f:
        res = json.load(f)
    pairs = list(zip(deep_of, [b['at'] for b in res['export_busy']]))
    wrong = [(p, a) for p, a in pairs if p != a]
    print(f"  exit {code}; {len(pairs)} deep checks: (the prefix deep-checked, the prefix recorded): {pairs}")
    print(f"  recorded under another prefix: {len(wrong)} of {len(pairs)}")
finally:
    t.tearDown()

print("\n== Y3. The stopped path: a busy export in each deep check, one forced, then the stuck control stops the run (M2)")
t = trd.Pass('test_a_clean_pass')
t.setUp()
try:
    t.fake()
    fdeep, fstuck = rd.Checker.deep, rd.stuck_control

    def deep(ck, p, *a, **k):
        with contextlib.redirect_stdout(io.StringIO()):
            ck.rig.held("cannot export 'p': pool is busy", 1)
        if not ck.rig.forced:
            ck.rig.forced.append({'export': ck.rig.exports, 'at': ck.rig.at, 'rc': 1})
        return fdeep(ck, p, *a, **k)

    def stuck(ck, n):
        raise rd.RigError(f'the stuck control at prefix {n}: dd did not end')
    rd.Checker.deep, rd.stuck_control = deep, stuck
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            code = rd.check(t.args())
    finally:
        rd.Checker.deep, rd.stuck_control = fdeep, fstuck
    with open(os.path.join(t.dir.name, 'out', 'replay-results.json')) as f:
        res = json.load(f)
    with open(os.path.join(t.dir.name, 'out', 'replay-summary.md')) as f:
        text = f.read()
    line = [ln for ln in text.splitlines() if ln.startswith('Busy exports')]
    print(f"  exit {code}; stopped: {res.get('stopped', '')[:50]!r}; export_busy: {len(res.get('export_busy') or [])}, "
          f"export_forced: {res.get('export_forced')}")
    print(f"  the line: {line[0][:200] if line else None}")
    print(f"  says a pool was left imported: {'a pool was left imported' in text}")
finally:
    t.tearDown()

print("\n== Y3b. The stopped path when the Checker itself could not be made (no Rig in st)")
d = tempfile.mkdtemp(prefix='y3b-')


def fake(a, st):
    st['res'] = {}
    raise rd.RigError('the log has no fsync marks')


real = rd._check
rd._check = fake
try:
    with contextlib.redirect_stdout(io.StringIO()):
        code = rd.check(type('A', (), {'out': d})())
finally:
    rd._check = real
with open(os.path.join(d, 'replay-results.json')) as f:
    res = json.load(f)
print(f"  exit {code}; export_busy in the results: {'export_busy' in res}")
shutil.rmtree(d)

print("\n== Y4. classify and tally over what zdb's every-copy check can write (M3)")
for failed in (rc.ZDB_ABORT, 'rig: zdb died with signal 9', 'rig: zdb died with signal 11',
               'rig: zdb died with signal 7', 'rig: zdb timed out', 'copies: zdb'):
    f, r = [], []
    rd.classify({'n': 1, 'failed': failed, 'detail': ''}, f, r)
    by, rerr, first = rd.tally([{'n': 1, 'ok': False, 'failed': failed, 'detail': ''}])
    print(f"  {failed:32} classify: {'failure' if f else 'rig':7} tally: {by or 'rig error'}")

print("\n== Y5. Plant L06 as written, on a normal end whose exit is 2 (a rig stage in the main pass)")
L06 = ("    out.extend(rc.rig_lines({k: res.get(k) for k in ('export_busy', 'export_forced')}))\n",
       "    out.extend(rc.rig_lines(dict(res, exit_code=res.get('exit'))))\n")
WHOLE = ("    out.extend(rc.rig_lines({k: res.get(k) for k in ('export_busy', 'export_forced')}))\n",
         "    out.extend(rc.rig_lines(res))\n")
DRIVER = r'''
import os, sys
sys.path.insert(0, sys.argv[1])
import logwrites, replay_check, replay_daemon as rd, test_replay_daemon as trd
t = trd.Pass('test_a_rig_stage_in_the_main_pass_is_the_rigs_error')
t.setUp()
try:
    records = logwrites.index(t.log, 0)['records']
    n = replay_check.fsync_marks(records)[1][0]
    t.fake(at={n: {'ok': False, 'failed': 'rig: losetup', 'detail': 'no free loop device'}})
    import contextlib, io
    with contextlib.redirect_stdout(io.StringIO()):
        code = rd.check(t.args())
    text = t.summary_text()
    print(code, 'a pool was left imported' in text)
finally:
    t.tearDown()
'''
for name, (old, new) in (('as written (exit mapped to exit_code)', L06), ('the whole results', WHOLE),
                         ('no plant', (None, None))):
    tmp = tempfile.mkdtemp(prefix='y5-')
    k = os.path.join(tmp, 'kernel')
    shutil.copytree(NEW, k, ignore=shutil.ignore_patterns('__pycache__'))
    if old:
        p = os.path.join(k, 'replay_daemon.py')
        with open(p) as f:
            s = f.read()
        assert s.count(old) == 1
        with open(p, 'w') as f:
            f.write(s.replace(old, new))
    out = subprocess.run([sys.executable, '-B', '-c', DRIVER, k], capture_output=True, text=True, timeout=120)
    print(f"  {name}: exit and 'a pool was left imported' in the summary: {out.stdout.strip() or out.stderr[-200:]}")
    shutil.rmtree(tmp)

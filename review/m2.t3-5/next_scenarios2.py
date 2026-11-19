#!/usr/bin/env python3
"""#130 at 9710a24: zdb's exit codes and signals in both rigs, before and
after; the daemon jobs' stopped path; N1 with the stand-in; S7's prefix.

    next_scenarios2.py CHECKOUT OLD_TESTS_KERNEL_DIR

OLD_TESTS_KERNEL_DIR holds tests/kernel of 09860a0 (git archive). Each
rig's old and new code runs in a subprocess of its own, so the two
versions never share a module. Nothing needs a kernel or ZFS.
"""
import json
import os
import subprocess
import sys

ROOT, OLD = sys.argv[1], sys.argv[2]
NEW = os.path.join(ROOT, 'tests', 'kernel')
CODES = [0, 1, 2, 124, 127, 134, 135, 137, 139, -6, -7, -9, -11, -15]
LEFT = 'rig: a pool named p is still imported'

PROBE = r'''
import json, sys
sys.path.insert(0, sys.argv[1])
import replay_check as rc
import replay_daemon as rd
codes = json.loads(sys.argv[2])
left = {'n': 4, 'failed': %r}
out = {}
for z in codes:
    with_left = rc.exit_code(False, [left], [], [], True, z)
    ok, _ = rc.job_verdict([], True, True, [], z)
    no_left = rc.exit_code(ok, [], [], [], True, z)
    d = rd.verdict([], [], True, [], True, z)
    main = rc.every_copy_verdict(z, '', '/dev/null')[0] if z not in (0,) else None
    f, r = [], []
    if main:
        rd.classify({'n': 3, 'failed': main, 'detail': ''}, f, r)
    out[z] = {'rc_exit_left': with_left, 'rc_exit_noleft': no_left, 'rd_exit': d[0],
              'rd_why': (d[1] or [''])[0][:60], 'main': main,
              'main_left': rc.exit_code(False, [{'n': 3, 'failed': main}, left], [], [], True, 0) if main else None,
              'rd_main': ('failure' if f else 'rig' if r else None)}
print(json.dumps(out))
''' % LEFT


def probe(d):
    p = subprocess.run([sys.executable, '-I', '-c', PROBE, d, json.dumps(CODES)], capture_output=True, text=True,
                       timeout=120)
    if p.returncode:
        sys.exit(p.stderr)
    return {int(k): v for k, v in json.loads(p.stdout).items()}


old, new = probe(OLD), probe(NEW)
sys.path.insert(0, NEW)
import replay_check as rc   # noqa: E402
import replay_daemon as rd  # noqa: E402

print('== Z1. zdb_rig_code, and what each rig makes of zdb\'s exit code (old = 09860a0, new = 9710a24)')
print('  At the end (zdb -bcc): the reference judge\'s exit with a pool left and nothing else / with no pool left;')
print('  the daemon judge\'s exit with nothing else. In the main pass (zdb -bbbbb): the failure every_copy_verdict')
print('  writes, the reference exit with a pool left, and the daemon\'s classify.')
print(f"  {'rc':>5} {'rig code':>8} | {'end: ref, left':>14} {'ref, no left':>12} {'daemon':>9} | "
      f"{'main pass failure':32} {'ref, left':>9} {'daemon':>8}")
for z in CODES:
    o, n = old[z], new[z]

    def pair(k):
        return f"{o[k]}->{n[k]}" if o[k] != n[k] else f"{n[k]}"
    print(f"  {z:>5} {str(rc.zdb_rig_code(z)):>8} | {pair('rc_exit_left'):>14} {pair('rc_exit_noleft'):>12} "
          f"{pair('rd_exit'):>9} | {str(n['main']):32} {pair('main_left'):>9} {pair('rd_main'):>8}")
print(f"  None: zdb_rig_code {rc.zdb_rig_code(None)}; the daemon judge: {rd.verdict([], [], True, [], True, None)}")

print('\n== Z2. What run() gives for a signal, directly and through a shell')
for sig in ('ABRT', 'KILL', 'SEGV', 'BUS'):
    direct = rc.run(['sh', '-c', f'kill -{sig} $$'], 10)[0]
    wrapped = rc.run(['sh', '-c', f'sh -c "kill -{sig} \\$\\$"; exit $?'], 10)[0]
    print(f"  SIG{sig}: the command itself {direct} (rig code {rc.zdb_rig_code(direct)}); "
          f"a wrapper's exit {wrapped} (rig code {rc.zdb_rig_code(wrapped)})")
print('  Both rigs run zdb as run([\'zdb\', ...]), never through a shell.')

print('\n== Z3. The daemon judge stopped by the rig\'s own error after zdb -bcc ran (check(), its except path)')
import tempfile  # noqa: E402
for z in (-9, -11, -6, 124, 0):
    d = tempfile.mkdtemp(prefix='z3-')

    def fake(a, st, z=z):
        st['res'] = {'zdb_cc': {'n': 10, 'rc': z, 's': 1.0, 'said': ''}}
        raise rd.RigError('the stuck control at prefix 5: dd did not end')
    real = rd._check
    rd._check = fake
    try:
        import contextlib
        import io
        with contextlib.redirect_stdout(io.StringIO()):
            code = rd.check(type('A', (), {'out': d})())
    finally:
        rd._check = real
    with open(os.path.join(d, 'replay-summary.md')) as f:
        text = f.read()
    said = 'says "a verdict against the daemon"' if 'verdict against the daemon' in text else 'says the rig\'s error'
    normal = rd.verdict([], [{'n': 5, 'failed': 'rig: the stuck control'}], True, [], True, z)[0]
    print(f"  zdb -bcc rc {z:>4}: stopped path exit {code}, {said}; the same rc where the run ends normally: {normal}")

print('\n== Z4. N1 again: every busy export waits')
import argparse  # noqa: E402
work = tempfile.mkdtemp(prefix='z4-')
man = os.path.join(work, 'manifest')
open(man, 'w').close()
rig = rc.Rig(argparse.Namespace(provider='', work=os.path.join(work, 'w'), manifest=man, pool='p', mountpoint='/p'))
st = {}


def standin(cmd, timeout=300):
    if cmd[:2] == ['zpool', 'export']:
        if st['busy'](cmd):
            return 1, "cannot export 'p': pool is busy\n", 0.0
        st['imported'] = False
        return 0, '', 0.0
    return 0, '', 0.0


def run_exports(holds):
    rig.busy, rig.forced, rig.exports = [], [], 0
    codes = []
    for h in holds:
        seen = {'plain': 0}

        def busy(cmd, h=h, seen=seen):
            if h == 'clock':
                return not st['slept']
            n, f_refused = h
            if '-f' in cmd:
                return f_refused
            seen['plain'] += 1
            return seen['plain'] <= n
        st.update(imported=True, busy=busy, slept=[])
        real_run, real_sleep = rc.run, rc.time.sleep
        rc.run, rc.time.sleep = standin, st['slept'].append
        try:
            with contextlib.redirect_stdout(io.StringIO()):
                codes.append(rig.export()[0])
        finally:
            rc.run, rc.time.sleep = real_run, real_sleep
    return codes


codes = run_exports([(5, False), (5, False), 'clock'])
print(f"  two exports busy through every wait whose -f then worked, then one busy for under a second: exit codes {codes}; "
      f"pool left: {st['imported']}; forced {rig.forced}")
print('  ' + ''.join(rc.rig_lines({'export_busy': rig.busy, 'export_forced': rig.forced})).strip()[:300])
codes = run_exports([(5, True)])
print(f"  one busy through every wait and through -f: exit codes {codes}; pool left: {st['imported']}")
print('  ' + ''.join(rc.rig_lines({'export_busy': rig.busy, 'export_forced': rig.forced})).strip()[:220])

print("\n== Z5. S7: the prefix a busy export's record names in the daemon jobs")
sys.path.insert(0, NEW)
import test_replay_daemon as trd  # noqa: E402
t = trd.Pass('test_a_clean_pass')
t.setUp()
try:
    t.fake()
    fpoint, fdeep = rd.Checker.point, rd.Checker.deep
    deep_of = []

    def point(ck, n, kind, *a, **k):
        ck.rig.at = n                   # as Checker.point does, before it makes the image
        return fpoint(ck, n, kind, *a, **k)

    def deep(ck, p, *a, **k):
        # a busy export in the deep check of the image kept for p
        with contextlib.redirect_stdout(io.StringIO()):
            ck.rig.held("cannot export 'p': pool is busy", 1)
        deep_of.append(p['n'])
        return fdeep(ck, p, *a, **k)
    rd.Checker.point, rd.Checker.deep = point, deep
    with contextlib.redirect_stdout(io.StringIO()):
        code = rd.check(t.args())
    with open(os.path.join(t.dir.name, 'out', 'replay-results.json')) as f:
        res = json.load(f)
    pairs = list(zip(deep_of, [b['at'] for b in res['export_busy']]))
    wrong = [(p, a) for p, a in pairs if p != a]
    print(f"  {len(pairs)} deep checks, each with one busy export: (the prefix deep-checked, the prefix recorded): "
          f"{pairs[:8]}{' ...' if len(pairs) > 8 else ''}")
    print(f"  recorded under another prefix: {len(wrong)} of {len(pairs)}")
finally:
    t.tearDown()

print("\n== Z6. S7 on the daemon jobs' stopped path: a busy export, then the rig stops on its own error")
t = trd.Pass('test_a_clean_pass')
t.setUp()
try:
    t.fake()
    fdeep, fstuck = rd.Checker.deep, rd.stuck_control

    def deep(ck, p, *a, **k):
        with contextlib.redirect_stdout(io.StringIO()):
            ck.rig.held("cannot export 'p': pool is busy", 1)
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
    print(f"  exit {code}; stopped: {res.get('stopped', '')[:60]!r}; export_busy in the results: "
          f"{'export_busy' in res}; a 'Busy exports' line in the summary: {'Busy exports' in text}")
finally:
    t.tearDown()

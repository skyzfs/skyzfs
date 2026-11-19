#!/usr/bin/env python3
"""#130 at c205682: scenarios against the judge's bound lines, the replay
rig's busy export and its exit code.

    scenarios.py CHECKOUT

Nothing here needs a kernel. `zpool` is stood in for where a scenario
says so; `fuser` is the real one, on a directory that is not a mount
point (as the pool's mount point is once `zpool export` has unmounted it
and the kernel then refuses the export as busy, run 44421456349).
"""
import os
import shutil
import sys
import tempfile
import argparse

ROOT = sys.argv[1]
sys.path.insert(0, os.path.join(ROOT, 'tests', 'kernel'))
import cut_daemon_check as cdc   # noqa: E402
import replay_check as rc        # noqa: E402

START = 'cuts 0\ncut_state begun\nintent_state begun\nintent_regions 128\nintent_named_at_start 128\n'


def held(**kw):
    base = {'cuts_held': '1', 'cut_holes': 'held', 'cuts_abandoned': '0', 'cut_copied_bytes': '917504',
            'cut_bound_bytes': '268435456'}
    base.update(kw)
    return ''.join(f'{k} {v}\n' for k, v in base.items() if v is not None)


def bound(name, start, held_text):
    d = tempfile.mkdtemp(prefix='scn-bound-')
    try:
        if start is not None:
            with open(os.path.join(d, 'status-start-1.txt'), 'w') as f:
                f.write(start)
        if held_text is not None:
            with open(os.path.join(d, 'status-bound-held.txt'), 'w') as f:
                f.write(held_text)
        got = []
        try:
            r = cdc.bound_lines(lambda k, v, t: got.append((k, v)), d, name, 0, False)
        except cdc.RigError as e:
            return f'RigError: {e}'
        return f"rc {r[0]} errored {r[1]}: " + ', '.join(f'{k} {v}' for k, v in got)
    finally:
        shutil.rmtree(d)


print('== A. bound_lines: inputs -> lines')
FILLED = START + 'cut_holes held\ncut_hole_chunks 0\n'
SPARSE = START + 'cut_holes held\ncut_hole_chunks 8192\n'
rows = [
    ('boundsparse', 'sparse start, held 896 KiB', SPARSE, held()),
    ('boundsparse', 'sparse start, held 4 MiB - 1', SPARSE, held(cut_copied_bytes=str((4 << 20) - 1))),
    ('boundsparse', 'sparse start, held 4 MiB', SPARSE, held(cut_copied_bytes=str(4 << 20))),
    ('boundsparse', 'sparse start, held 320 MiB (copies every hole write)', SPARSE, held(cut_copied_bytes=str(320 << 20))),
    ('boundsparse', 'abandoned at the bound, map passed on (holes dropped)', SPARSE,
     held(cuts_abandoned='1', cut_holes='dropped', cut_copied_bytes='0')),
    ('boundsparse', 'held reading with no cut_holes key (older-format section)', SPARSE, held(cut_holes=None)),
    ('boundsparse', 'held reading with no cuts_abandoned key', SPARSE, held(cuts_abandoned=None)),
    ('boundsparse', 'held reading is the error text of a failed status', SPARSE,
     "skyzfsd status: no status file under the run directory\n"),
    ('boundsparse', 'held reading missing', SPARSE, None),
    ('boundsparse', 'start missing', None, held()),
    ('boundsparse', 'start: holes none, 0 chunks (no hole set taken)', START + 'cut_holes none\ncut_hole_chunks 0\n', held()),
    ('boundsparse', 'start: holes held, 0 chunks', FILLED, held()),
    ('boundsparse', 'start: chunks not a number', START + 'cut_holes held\ncut_hole_chunks x\n', held()),
    ('bound', 'filled start, held after an abandonment', FILLED,
     held(cuts_abandoned='1', cut_copied_bytes='4096')),
    ('bound', 'filled start: holes none, 0 chunks', START + 'cut_holes none\ncut_hole_chunks 0\n', held()),
    ('bound', 'sparse start (the zero fill did not happen)', SPARSE, held()),
    ('bound', 'held reading missing', FILLED, None),
    ('bound', 'held reading is an error text', FILLED, 'error\n'),
]
for name, what, start, h in rows:
    print(f'  {name:12} {what}: {bound(name, start, h)}')

print('\n== B. Rig.export and Rig.held')
work = tempfile.mkdtemp(prefix='scn-replay-')
manifest = os.path.join(work, 'manifest')
open(manifest, 'w').close()
rig = rc.Rig(argparse.Namespace(provider='', work=os.path.join(work, 'w'), manifest=manifest, pool='skzreplay',
                                mountpoint='/skz'))
# The pool's mount point under the alternate root, left behind by the
# unmount: a directory, not a mount point.
os.makedirs(rig.alt + '/skz', exist_ok=True)
import contextlib, io
with contextlib.redirect_stdout(io.StringIO()) as printed:
    rig.held("cannot export 'skzreplay': pool is busy", 1)
print('  B0 the log line begins: ' + printed.getvalue().split('held by: ', 1)[1][:30].replace(rig.alt, 'ALT') + ' ...')
b = rig.busy[0]
lines = b['fuser'].splitlines()
print(f"  B1 fuser -vm on the unmounted mount point: {len(lines)} lines kept of the record "
      f"(of {len(b['fuser'])} chars); mounts under the alternate root: {b['mounts']}")
full = rc.run(['fuser', '-vm', rig.alt + '/skz'], 30)[1]
print(f"     fuser's whole output: {len(full.splitlines())} lines; its first: {full.splitlines()[0].strip()!r}; "
      f"'kernel mount /' in it: {'kernel mount /' in full}")
said = rc.rig_lines({'export_busy': rig.busy})[0].strip()
named = said.split('held by: ', 1)[1].split(')', 1)[0]
print(f"     the summary's 'held by': {'fuser header' if named.startswith('USER') else 'the line ' + repr(lines[0].strip()) + ', cut at the 1000-character tail' if named == lines[0].strip() else repr(named)}")
print("  B2 'busy' in the output decides a retry:")
for said in ("cannot export 'skzreplay': pool is busy", "cannot unmount '/w/alt/skz': pool or dataset is busy",
             "umount: /w/alt/skz: target is busy.", "cannot export 'skzreplay': I/O error",
             "internal error: Device or resource busy", "\nTIMEOUT after 120s"):
    print(f"     {said.strip()!r}: {'retried' if 'busy' in said else 'straight to -f'}")
print(f"  B3 the waits of one export busy throughout: {sum(rc.EXPORT_WAITS)} s, then -f; "
      f"a pass of 2500 prefixes busy at each: {2500 * sum(rc.EXPORT_WAITS) / 3600:.1f} h of waits")

print('\n== C. exit_code')


def fake_rig(script):
    """A Rig whose commands answer from `script` (a function of the command)."""
    calls = []

    def fake(cmd, timeout=300):
        calls.append(cmd)
        return script(cmd)
    return fake, calls


# C1: a prefix whose verify fails (a verdict) and whose export then stays
# busy through the waits and -f: the point reads 'export', the next ones
# 'still imported'.
state = {'imported': False}


def script(cmd):
    if cmd[:2] == ['zpool', 'list']:
        return (0, 'skzreplay\n', 0.0) if state['imported'] else (1, "cannot open 'skzreplay': no such pool\n", 0.0)
    if cmd[:2] == ['zpool', 'import']:
        state['imported'] = True
        return 0, '', 0.0
    if cmd[:2] == ['zpool', 'export']:
        return 1, "cannot export 'skzreplay': pool is busy\n", 0.0
    if cmd[0] == 'losetup' and '--show' in cmd:
        return 0, '/dev/loop9\n', 0.0
    return 0, '', 0.0


fake, calls = fake_rig(script)
real_run, real_sleep = rc.run, rc.time.sleep
rc.run, rc.time.sleep = fake, (lambda s: None)
rig.advance = lambda n: 0.0
rig.verify = lambda files: 'f.12: checksum differs'
try:
    points = [rig.point(n, 12, False) for n in (100, 101, 102)]
finally:
    rc.run, rc.time.sleep = real_run, real_sleep
failures = [p for p in points if not p['ok']]
print('  C1 verify fails at 100, the export stays busy:', [(p['n'], p['failed']) for p in failures])
ctl = [{'name': n, 'rig_errors': [{'n': 5, 'failed': 'rig: a pool named skzreplay is still imported', 'detail': ''}],
        'caught': False, 'failed_by': {}, 'class': 'block'} for n in ('data', 'drop', 'reorder')]
rows, _, cok = rc.judge_controls(ctl, absent={'intent': 'not on the device'})
ok, against = rc.job_verdict(failures, cok, True, ['coverage short'], 0)
print(f"     job_verdict ok={ok}; exit_code = {rc.exit_code(ok, failures, ctl, rows, True, 0)} "
      "(the prefix's verify verdict is gone: replaced by 'export')")

# C2: Codex's case: a superseded drop (the table: not caught) caught at a
# prefix, and the pool left imported at a later prefix of the same control.
res2 = [{'n': 5, 'ok': False, 'failed': 'copies: differ', 'detail': 'x'},
        {'n': 6, 'ok': False, 'failed': 'export', 'detail': 'busy'},
        {'n': 7, 'ok': False, 'failed': 'rig: a pool named skzreplay is still imported', 'detail': ''}]
by, errs, first = rc.tally_control(res2)
c2 = [{'name': 'drop', 'class': 'superseded', 'caught': first is not None, 'failed_by': by, 'rig_errors': errs},
      {'name': 'data', 'class': 'block', 'caught': False, 'failed_by': {}, 'rig_errors': errs[1:]},
      {'name': 'reorder', 'class': 'block', 'caught': False, 'failed_by': {}, 'rig_errors': errs[1:]}]
rows2, _, cok2 = rc.judge_controls(c2, absent={'intent': 'x'})
ok2, _ = rc.job_verdict([], cok2, True, [], 0)
c2b = [dict(c2[0], rig_errors=[])]
rows2b, _, _ = rc.judge_controls(c2b, absent={'intent': 'x', 'data': 'x', 'reorder': 'x'})
print(f"  C2 superseded drop caught at 5, pool left at 7: exit_code = {rc.exit_code(ok2, [], c2, rows2, True, 0)}; "
      f"without the rig errors its row: match={rows2b[0]['match']} ({rows2b[0]['observed']})")

# C2b: a class the table does not name for the control (reorder of a
# superseded write), with rig errors: excused too.
c3 = [{'name': 'reorder', 'class': 'superseded', 'caught': False, 'failed_by': {}, 'rig_errors': errs[1:]}]
rows3, _, cok3 = rc.judge_controls(c3, absent={'intent': 'x', 'data': 'x', 'drop': 'x'})
row3 = next(r for r in rows3 if r['name'] == 'reorder')
print(f"  C2b reorder whose class the table does not name, pool left: row '{row3['expected']}'; "
      f"exit_code = {rc.exit_code(False, [], c3, rows3, True, 0)}")

# C3: the reverse: zdb timed out (124) or could not start (127) at the end,
# with a pool left imported and nothing else.
left = {'n': 9, 'failed': 'rig: a pool named skzreplay is still imported'}
for z in (124, 127, 1):
    print(f"  C3 pool left, zdb -bcc rc {z}: exit_code = {rc.exit_code(False, [left], [], [], True, z)}")

# C4: a pool left imported by the deep check (its leftover reads 'deep: rig: ...').
deep_left = {'n': 9, 'failed': 'deep: rig: a pool named skzreplay is still imported'}
print(f"  C4 left seen by the deep check: exit_code = {rc.exit_code(False, [deep_left], [], [], True, 0)}")
# C5: a deep check that timed out, then left the pool: 'deep: rig: scrub timed out' is no verdict.
print(f"  C5 scrub timed out, then left: exit_code = "
      f"{rc.exit_code(False, [{'n': 8, 'failed': 'deep: rig: scrub timed out'}, left], [], [], True, 0)}")
# C6: the witness disagrees, with a pool left imported.
print(f"  C6 witness differs, pool left: exit_code = {rc.exit_code(False, [left], [], [], False, 0)}")
shutil.rmtree(work)

"""Plant defects in #73's judge and rig, one at a time, and run the judge's tests.
usage: python3 -I plants73.py WORKTREE"""
import subprocess, sys, pathlib
W = pathlib.Path(sys.argv[1])
DC = 'tests/kernel/daemon_check.py'
RIG = 'tests/kernel/daemon-rig.sh'
PLANTS = [
    ('N1 NA expected of the two lines on the stand-in too', DC,
     "            if name in NOT_ON_THE_DAEMON:\n                want, basis = 'NA'",
     "            pass\n        if name in NOT_ON_THE_DAEMON:\n                want, basis = 'NA'"),
    ('N2 a line the table does not know is not an error', DC,
     "            errors.append(f'{name}: not in the table EXPECTED')",
     "            pass"),
    ('N3 the log files allowed again in g.4', RIG,
     "GJ_MAY_CHANGE='^status$'",
     "GJ_MAY_CHANGE='^(status|log\\.(super|[0-9]+))$'"),
    ('N4 check f a check again', DC,
     "CHECKS = ('a', 'b', 'c', 'd', 'e', 'g')",
     "CHECKS = ('a', 'b', 'c', 'd', 'e', 'f', 'g')"),
    ('N5 only one of the two lines is NA on the daemon', DC,
     "    'pin_c_kill_with_applied_unacknowledged': 'as pin_kill_under_load_applied_unacknowledged, through the suite',\n",
     ""),
    ('N6 the suite judged by the stand-in\'s counts on the daemon', DC,
     "        judge_cases(results, rows, differs, errors, impl)",
     "        judge_cases(results, rows, differs, errors, 'reference')"),
]
def run():
    p = subprocess.run([sys.executable, '-I', 'tests/kernel/test_daemon_check.py'], cwd=W, capture_output=True, text=True, timeout=300)
    fails = sorted({l.split(' ')[1] for l in p.stderr.splitlines() if l.startswith(('FAIL: ', 'ERROR: '))})
    last = [l for l in p.stderr.splitlines() if l.startswith(('Ran ', 'OK', 'FAILED'))]
    return p.returncode, fails, ' '.join(last)
for label, f, old, new in PLANTS:
    path = W / f
    text = path.read_text()
    if text.count(old) != 1:
        print(f'{label}: NOT PLANTED (found {text.count(old)} times)'); continue
    path.write_text(text.replace(old, new))
    try:
        rc, fails, last = run()
        print(f"{label}: {'FAILS: ' + ', '.join(fails) if rc else 'nothing fails'} ({last})")
    finally:
        path.write_text(text)
st = subprocess.run(['git', 'status', '--short'], cwd=W, capture_output=True, text=True).stdout.strip()
print('worktree after:', st or 'clean')

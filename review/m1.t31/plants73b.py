import subprocess, sys
W = '/home/user/skyzfs-pr73'
R = 'tests/kernel/daemon-rig.sh'
B = 'tests/bench/topology-bench.sh'
P = [
 ('K1 the pin branch of kill_verdict gone', R,
  'if [ "${1:-}" = pin ] && [ -n "$KT_REFUSED" ]; then', 'if false && [ -n "$KT_REFUSED" ]; then'),
 ('K2 exit 4 not a refusal', R,
  '"$EXIT_FAILED"|"$EXIT_REFUSED_FAILED"|"$EXIT_BUSY")', '"$EXIT_FAILED"|"$EXIT_BUSY")'),
 ('K3 any exit a refusal', R,
  '"$EXIT_FAILED"|"$EXIT_REFUSED_FAILED"|"$EXIT_BUSY")', '*)'),
 ('K4 the pin branch after the error', R,
  '    if [ "${1:-}" = pin ] && [ -n "$KT_REFUSED" ]; then KT_WHY=$KT_REFUSED; KT_VERDICT=FAIL; return; fi\n    if [ -n "$KT_ERR" ]; then KT_WHY=$KT_ERR; KT_VERDICT=ERROR; return; fi\n',
  '    if [ -n "$KT_ERR" ]; then KT_WHY=$KT_ERR; KT_VERDICT=ERROR; return; fi\n    if [ "${1:-}" = pin ] && [ -n "$KT_REFUSED" ]; then KT_WHY=$KT_REFUSED; KT_VERDICT=FAIL; return; fi\n'),
 ('K5 KT_REFUSED not reset between kill tests', R, 'KT_ERR=""; KT_REFUSED=""; ', 'KT_ERR=""; '),
 ('K6 check_b judged plainly', R, '    kill_verdict pin; v=$KT_VERDICT', '    kill_verdict; v=$KT_VERDICT'),
 ('B1 absent keys at rest with a request held', B, '&& [ "$held" = 0 ] && [ -n "$w0" ]', '&& { [ "$held" = 0 ] || [ -n "$held" ]; } && [ -n "$w0" ]'),
 ('B2 an unreadable status at rest', B, '&& [ "$held" = 0 ] && [ -n "$w0" ]', '&& { [ "$held" = 0 ] || [ -z "$held" ]; }'),
 ('B3 absent step taken only as none', B, '{ [ "$step" = none ] || [ -z "$step" ]; }', '{ [ "$step" = none ]; }'),
 ('B4 a step under way at rest', B, '{ [ "$step" = none ] || [ -z "$step" ]; }', '{ true; }'),
]
def run(cmd):
    r = subprocess.run(cmd, cwd=W, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr
for name, f, old, new in P:
    path = f'{W}/{f}'
    s = open(path).read()
    n = s.count(old)
    if n == 0:
        print(f'{name}: PLANT NOT APPLIED'); continue
    open(path, 'w').write(s.replace(old, new))
    rc1, o1 = run(['python3', '-I', 'tests/kernel/test_daemon_check.py'])
    rc2, o2 = run(['bash', 'tests/bench/test_copy_limit.sh'])
    subprocess.run(['git', 'checkout', '--', f], cwd=W)
    fails = [l for l in o1.splitlines() if l.startswith(('FAIL:', 'ERROR:'))]
    bfails = [l for l in o2.splitlines() if l.startswith('FAIL')]
    print(f'{name} (x{n}): judge rc {rc1} {fails}; bench rc {rc2} {bfails}')
print(run(['git', 'status', '--short'])[1] or 'clean')

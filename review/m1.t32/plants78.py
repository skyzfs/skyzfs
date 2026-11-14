import subprocess
W = '/home/user/skyzfs-pr75'
ZS = 'tests/kernel/zfs-stuck.sh'
RIG = 'tests/kernel/daemon-rig.sh'
DC = 'tests/kernel/daemon_check.py'
PL = [
 ('Z1 any call of zfs_stuck_say marks', ZS, '    if [ "$n_d" != 0 ]; then\n        [ -z "${ZS_STACKS:-}" ]', '    if true; then\n        [ -z "${ZS_STACKS:-}" ]'),
 ('Z2 a later call overwrites the mark', ZS, 'if [ -n "${ZS_FILE:-}" ] && [ ! -s "$ZS_FILE" ]; then', 'if [ -n "${ZS_FILE:-}" ]; then'),
 ('Z3 course never stops', RIG, '    since=$(zfs_stuck_since) || return 0\n', '    since=$(zfs_stuck_since) || return 0; return 0\n'),
 ('Z4 a missing line under the mark is not a rig error', DC, "            errors.append(f'{name}: {stuck}')\n", ''),
 ('Z5 the mark makes every line ERROR', DC, '        if value is None and stuck:', '        if stuck:'),
 ('Z6 check e not guarded', RIG, 'course e && check_e', 'check_e'),
 ('Z7 d, scrub not guarded', RIG, '    course "d, scrub" && check_d_scrub', '    check_d_scrub'),
 ('Z8 the end line does not name the courses not run', RIG, 'courses not run: ${NOT_RUN_COURSES:-none}', 'courses not run: none'),
 ('Z9 cleanup asks a stuck ZFS for its pools', RIG, '    zfs_stuck_since > /dev/null || for p in', '    for p in'),
 ('Z10 the summary has no banner', DC, "        out += [f\"**ZFS on this machine got stuck**", "        out += [f\"ZFS stuck"),
 ('Z11 no stacks at the mark', ZS, '        [ -z "${ZS_STACKS:-}" ] || zs_stacks "${ZS_WHERE:-}" >> "$ZS_STACKS" 2>&1\n', ''),
 ('Z12 a missing observation under the mark says no observation', DC, "errors.append(f'{name}: {stuck or \"no observation\"}')", "errors.append(f'{name}: no observation')"),
 ('Z13 step does not say where the rig is', RIG, "step() { ZS_WHERE=$*; printf", "step() { printf"),
 ('Z14 the mark only for a zpool in D waiting for the namespace lock (stricter, not a defect)', ZS, '    if [ "$n_d" != 0 ]; then\n        [ -z "${ZS_STACKS:-}" ]', '    if [ "$ns" != 0 ]; then\n        [ -z "${ZS_STACKS:-}" ]'),
]
def run(cmd):
    r = subprocess.run(cmd, cwd=W, capture_output=True, text=True)
    return r.returncode, r.stdout + r.stderr
for name, f, old, new in PL:
    path = f'{W}/{f}'
    s = open(path).read()
    n = s.count(old)
    if n != 1:
        print(f'{name}: PLANT NOT APPLIED ({n})', flush=True); continue
    open(path, 'w').write(s.replace(old, new))
    rc1, o1 = run(['python3', '-I', 'tests/kernel/test_daemon_check.py'])
    rc2, o2 = run(['bash', 'tests/kernel/test_zfs_stuck.sh'])
    open(path, 'w').write(s)
    f1 = sorted({l.split()[1] for l in o1.splitlines() if l.startswith(('FAIL:', 'ERROR:'))})
    f2 = [l for l in o2.splitlines() if l.startswith('FAIL')]
    print(f'{name}: judge rc {rc1} {f1}; zfs-stuck rc {rc2} {f2}', flush=True)
print(run(['git', 'status', '--short'])[1] or 'clean')

import subprocess
W = '/home/user/skyzfs-pr75'
ZS = 'tests/kernel/zfs-stuck.sh'
RIG = 'tests/kernel/daemon-rig.sh'
DC = 'tests/kernel/daemon_check.py'
PL = [
 ('Z1 any zpool in D marks (the old condition)', ZS, 'if [ "$locks" != 0 ] && [ -n "${ZS_FILE:-}" ]', 'if [ -n "${ZS_FILE:-}" ]'),
 ('Z2 a later call overwrites the mark', ZS, '[ -n "${ZS_FILE:-}" ] && [ ! -s "$ZS_FILE" ]; then', '[ -n "${ZS_FILE:-}" ]; then'),
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
 ('Z14 only namespace waiters mark', ZS, 'case $fn in spa_namespace_enter|spa_config_enter) locks', 'case $fn in spa_namespace_enter) locks'),
 ('Z15 only config-lock waiters mark', ZS, 'case $fn in spa_namespace_enter|spa_config_enter) locks', 'case $fn in spa_config_enter) locks'),
 ('Z16 a zpool in txg_wait_synced marks too', ZS, 'case $fn in spa_namespace_enter|spa_config_enter) locks', 'case $fn in spa_namespace_enter|spa_config_enter|txg_wait_synced) locks'),
 ('Z17 a zpool in zfs_ioc_clear marks too', ZS, 'case $fn in spa_namespace_enter|spa_config_enter) locks', 'case $fn in spa_namespace_enter|spa_config_enter|zfs_ioc_clear) locks'),
 ('Z18 stacks kept only when the mark is set', ZS, '    if [ "$n_d" != 0 ]; then\n        [ -z "${ZS_STACKS:-}" ]', '    if [ "$locks" != 0 ]; then\n        [ -z "${ZS_STACKS:-}" ]'),
 ('Z19 a txg_sync thread in D counts as a lock waiter', ZS, '        if [ "$c" = txg_sync ]; then txg+="${st:-?}"; continue; fi', '        if [ "$c" = txg_sync ]; then txg+="${st:-?}"; [ "$st" = D ] && locks=$((locks + 1)); continue; fi'),
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

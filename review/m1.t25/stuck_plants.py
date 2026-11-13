#!/usr/bin/env python3
"""stuck_plants.py WORKTREE: plant defects in tests/kernel/zfs-stuck.sh, one
at a time, run tests/kernel/test_zfs_stuck.sh and say which checks fail.
The file is put back from git after each plant."""
import subprocess
import sys

tree = sys.argv[1]
path = f'{tree}/tests/kernel/zfs-stuck.sh'
PLANTS = [
    ('m1 a sleeping zpool is counted',
     '        [ "$st" = D ] || continue\n', '        :\n'),
    ('m2 any process is counted',
     '        case $c in zpool|txg_sync) ;; *) continue ;; esac\n', '        :\n'),
    ('m3 a lost mark is not said',
     '            k+="; the kernel log since the mark is lost"\n', '            :\n'),
    ('m4 shown counts only the running',
     "shown=$(grep -c 'task:txg_sync ' ", "shown=$(grep -c 'task:txg_sync .*state:R' "),
    ('m7 the full stop stays',
     "awk '{ sub(/\\.$/, \"\", $NF); print $NF }'", "awk '{ print $NF }'"),
    ('m8 txg_wait_synced not in the pattern',
     'spa_config_enter|txg_wait_synced|spa_vdev_state_enter', 'spa_config_enter|spa_vdev_state_enter'),
    ('m9 the namespace waiters are named',
     'if [ "$fn" = spa_namespace_enter ]; then ns=$((ns + 1)); else ', 'if false; then ns=$((ns + 1)); else '),
    ('m10 the reports are not counted from the mark',
     'if klog_since "$1" > "$OUT/stuck.tmp"; then', 'if dmesg > "$OUT/stuck.tmp"; then'),
    ('m11 the running count takes any state',
     "run=$(grep -c 'task:txg_sync .*state:R' ", "run=$(grep -c 'task:txg_sync .*state:' "),
    ('m12 txg_sync\'s state is not read',
     'txg+="${st:-?}"; continue; fi', 'txg+="?"; continue; fi'),
    ('m13 only the first owner is kept',
     'sort -rn | head -n 4 |', 'sort -rn | head -n 1 |'),
]
orig = open(path).read()
for name, old, new in PLANTS:
    assert orig.count(old) == 1, (name, orig.count(old))
    open(path, 'w').write(orig.replace(old, new))
    try:
        out = subprocess.run(['bash', 'tests/kernel/test_zfs_stuck.sh'], cwd=tree, capture_output=True,
                             text=True, timeout=120).stdout
        fails = [line[5:45] for line in out.splitlines() if line.startswith('FAIL')]
        print(f"{name}: {'; '.join(fails) if fails else 'NOTHING FAILS'}")
    except subprocess.TimeoutExpired:
        print(f'{name}: the test did not end in 120 s')
    finally:
        open(path, 'w').write(orig)
subprocess.run(['git', 'status', '--short'], cwd=tree)

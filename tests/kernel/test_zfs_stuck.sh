#!/bin/bash
# Tests of zfs-stuck.sh with a made-up /proc and kernel log: no kernel
# and no ZFS are needed.
# Run: bash tests/kernel/test_zfs_stuck.sh
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# shellcheck source=tests/kernel/klog.sh
. "$HERE/klog.sh"
# shellcheck source=tests/kernel/zfs-stuck.sh
. "$HERE/zfs-stuck.sh"
# Every path below is under $T, and the test removes $T/proc: an empty T
# would make that /proc.
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}
OUT=$T; LOG=$T/log
dmesg() { cat "$LOG"; }
# A task: pid comm state [stack function]
task() {
    mkdir -p "$T/proc/$1"
    echo "$2" > "$T/proc/$1/comm"
    echo "$1 ($2) $3 1 1 1" > "$T/proc/$1/stat"
    printf '[<0>] __schedule+0x2a4/0x8e0\n[<0>] %s+0x8c/0x168 [zfs]\n[<0>] zfsdev_ioctl+0x74/0x120 [zfs]\n' "${4:-schedule}" > "$T/proc/$1/stack"
}
ZS_PROC=$T/proc

# The stuck pool of run 43389048554, in small: the holder of the
# namespace lock waits in spa_config_enter, one zpool sync waits for
# txg_sync, three wait for the namespace lock; txg_sync runs; a zpool
# that sleeps and another process are not counted.
task 405492 zpool D spa_config_enter
task 405179 zpool D txg_wait_synced
task 407787 zpool D spa_namespace_enter
task 409580 zpool D spa_namespace_enter
task 410892 zpool D spa_namespace_enter
task 500 zpool S spa_namespace_enter
task 403031 txg_sync R
task 302208 txg_sync S
task 77 bash D spa_namespace_enter
cat > "$LOG" <<'LOGEOF'
[ 1965.149136] WARNING: Pool 'skzd-d1' has encountered an uncorrectable I/O failure and has been suspended.
[ 1966.000000] skz-rig-mark-9-9
[ 2090.431814] INFO: task z_null_int:402995 blocked for more than 122 seconds.
[ 2090.432887] INFO: task z_null_int:402995 is blocked on a mutex likely owned by task txg_sync:403031.
[ 2090.433042] task:txg_sync        state:R  running task     stack:0     pid:403031 tgid:403031 ppid:2
[ 2090.433059] INFO: task zpool:405179 blocked for more than 122 seconds.
[ 2090.433947] INFO: task zpool:405179 is blocked on a mutex likely owned by task txg_sync:403031.
[ 2090.433950] task:txg_sync        state:R  running task     stack:0     pid:403031 tgid:403031 ppid:2
[ 2213.325014] INFO: task zpool:407787 blocked for more than 122 seconds.
[ 2213.325020] INFO: task zpool:407787 is blocked on a mutex likely owned by task zpool:405492.
[ 2213.325030] task:txg_sync        state:D stack:0     pid:302208 tgid:302208 ppid:2
LOGEOF

out=$(zfs_stuck_say skz-rig-mark-9-9)
is "the whole line" "$out" "zpool processes waiting in the kernel: 5 (3 for the namespace lock; 405179 in txg_wait_synced, 405492 in spa_config_enter); txg_sync threads' states: SR; the kernel since the mark: 3 hung-task reports, txg_sync shown 3 times and running in 2 of them, lock owners named: txg_sync:403031 x2, zpool:405492 x1"
has "the owners lose the kernel's full stop" "$out" "txg_sync:403031 x2"

out=$(zfs_stuck_say)
is "with no mark, no kernel half" "$out" "zpool processes waiting in the kernel: 5 (3 for the namespace lock; 405179 in txg_wait_synced, 405492 in spa_config_enter); txg_sync threads' states: SR"
out=$(zfs_stuck_say skz-rig-mark-0-0)
has "a mark not in the log is said to be lost" "$out" "; the kernel log since the mark is lost"

# A log with a mark and no hung-task report: the counts say none, with
# nothing that reads as a stuck pool.
printf '[ 100.000000] skz-rig-mark-5-5\n[ 101.000000] a quiet line\n' > "$T/log2"
out=$(LOG=$T/log2 zfs_stuck_say skz-rig-mark-5-5)
has "a log with no report says none" "$out" "; the kernel since the mark: 0 hung-task reports, txg_sync shown 0 times and running in 0 of them, lock owners named: none"

# Nothing stuck: no zpool in D state, no txg_sync.
rm -rf "$T/proc"; task 600 zpool S; task 601 sshd D
out=$(zfs_stuck_say skz-rig-mark-9-9)
has "nothing stuck" "$out" "zpool processes waiting in the kernel: 0; txg_sync threads' states: none; the kernel since the mark: 3"

# A stack read that never returns (a fifo with no writer, as a read of
# the stack of a task that cannot be looked at): the line still comes,
# within the limit, with "?" for that process.
rm -rf "$T/proc"; task 700 zpool D spa_config_enter; rm "$T/proc/700/stack"; mkfifo "$T/proc/700/stack"
# Under a limit of its own, so that a lost limit inside makes this case
# fail and not hang: a reader left blocked on the fifo is let go by
# opening it for writing.
s=$(date +%s)
( zfs_stuck_say > "$T/fifo.out" ) & p=$!
for _ in $(seq 1 30); do kill -0 "$p" 2>/dev/null || break; sleep 1; done
if kill -0 "$p" 2>/dev/null; then
    kill -9 "$p" 2>/dev/null
    # shellcheck disable=SC2016  # expanded by the inner shell
    timeout 5 sh -c ': > "$1"' _ "$T/proc/700/stack"
    echo "the stack read was not cut: still running after 30 s" > "$T/fifo.out"
fi
wait "$p" 2>/dev/null
out=$(cat "$T/fifo.out"); e=$(( $(date +%s) - s ))
is "a stack that cannot be read" "$out" "zpool processes waiting in the kernel: 1 (0 for the namespace lock; 700 in ?); txg_sync threads' states: none"
if [ "$e" -le 10 ]; then echo "ok   it returns within the limit ($e s)"; else echo "FAIL it took $e s"; fails=$((fails + 1)); fi

# The mark (M1.T32): written by the first call that finds a zpool
# process waiting in the kernel, with where the rig was; kept by later
# calls; read by zfs_stuck_since. Each such call adds the stacks.
rm -rf "$T/proc"; task 600 zpool S; task 403031 txg_sync R
ZS_FILE=$T/stuck.tsv; ZS_STACKS=$T/stacks.txt; : > "$ZS_FILE"
if zfs_stuck_since > /dev/null; then echo "FAIL an empty mark reads as stuck"; fails=$((fails + 1)); else echo "ok   an empty mark is not stuck"; fi
out=$(ZS_WHERE="a: the first course" zfs_stuck_say)
is "a zpool that sleeps leaves no mark" "$(cat "$ZS_FILE")" ""
is "and no stacks" "$([ -e "$ZS_STACKS" ] && echo there || echo none)" none
# The rig's own zpool sync on a pool it suspended waits in
# txg_wait_synced: no mark, though its stacks are kept.
task 401000 zpool D txg_wait_synced
out=$(ZS_WHERE="d: the pool suspends" zfs_stuck_say)
is "a zpool that waits for a sync leaves no mark" "$(cat "$ZS_FILE")" ""
is "  its stacks are kept" "$(grep -c '^-- 401000 zpool D$' "$ZS_STACKS")" 1
# Nor does a zpool waiting in another ioctl's path (the clear of run
# 43545561531, in vdev_reopen under zfs_ioc_clear) on its own.
task 394977 zpool D zfs_ioc_clear
out=$(ZS_WHERE="d: the pool suspends" zfs_stuck_say)
is "nor one in zfs_ioc_clear alone" "$(cat "$ZS_FILE")" ""
# Nor txg_sync waiting in the kernel with no zpool waiting for a lock:
# the stuck state is named by its lock waiters, not by txg_sync.
task 403032 txg_sync D
out=$(ZS_WHERE="d: the pool suspends" zfs_stuck_say)
is "nor txg_sync in D alone" "$(cat "$ZS_FILE")" ""
has "  which the line says" "$out" "txg_sync threads' states: RD"
# Namespace waiters alone mark it.
task 407787 zpool D spa_namespace_enter
out=$(ZS_WHERE="d: a namespace waiter" zfs_stuck_say)
is "a zpool waiting for the namespace lock marks it" "$(zfs_stuck_since)" "d: a namespace waiter"
: > "$ZS_FILE"; rm -rf "${T:?}/proc/407787"
task 394984 zpool D spa_config_enter
out=$(ZS_WHERE="d: the kept device answers again" zfs_stuck_say)
is "the mark's place" "$(zfs_stuck_since)" "d: the kept device answers again"
is "the mark's line" "$(cut -f2 "$ZS_FILE")" "$out"
out=$(ZS_WHERE="d, control: a volume that does not fail" zfs_stuck_say)
is "a later call keeps the first mark" "$(zfs_stuck_since)" "d: the kept device answers again"
is "and adds the stacks again" "$(grep -c '^-- 394984 zpool D$' "$ZS_STACKS")" 2
has "with the stack itself" "$(cat "$ZS_STACKS")" "-- 394984 zpool D
[<0>] __schedule+0x2a4/0x8e0
[<0>] spa_config_enter+0x8c/0x168 [zfs]"
has "txg_sync's too, though it runs" "$(cat "$ZS_STACKS")" "-- 403031 txg_sync R"
has "under where the rig was" "$(cat "$ZS_STACKS")" " d, control: a volume that does not fail"
is "the zpool that sleeps is not there" "$(grep -c '^-- 600 ' "$ZS_STACKS")" 0
out=$(ZS_FILE="" zfs_stuck_say)
if ZS_FILE="" zfs_stuck_since > /dev/null; then echo "FAIL no ZS_FILE reads as stuck"; fails=$((fails + 1)); else echo "ok   no ZS_FILE, no mark"; fi
unset ZS_FILE ZS_STACKS

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

# shellcheck shell=bash
# zfs_stuck_say [mark]: what holds ZFS when a zpool command has run out
# of its limit, in one line (M1.T25). It judges nothing.
#
# Seen on arm64 (run 43389048554): after a pool suspended, txg_sync
# stayed running for 18 minutes while the first zpool status held the
# namespace lock, and every zpool command after it waited behind that
# one. The rig's lines then read "rc 124" and "zpool create failed",
# which looked like a limit too short.
#
# The line has:
# - the zpool processes in uninterruptible sleep, counted by the ZFS
#   function each waits in (from its kernel stack). Those waiting for
#   the namespace lock are only counted; the others, among them the
#   holder, are named by pid;
# - the states of the txg_sync threads;
# - since MARK: the kernel's reports of hung tasks, how many times
#   txg_sync was shown in them and how many of those it was running,
#   and the owners they name of the locks waited on. A mark that is not
#   in the log says so.
#
# Every read of a kernel stack is under a limit of 5 s (a stack read can
# wait for the task). Needs lim (tests/lib/lim.sh), klog_since (klog.sh)
# and OUT. ZS_PROC is /proc, or a made-up tree in the tests.
zfs_stuck_say() {
    local proc=${ZS_PROC:-/proc} p c st fn ns=0 named="" n_d=0 txg="" k n shown run owners
    for p in "$proc"/[0-9]*; do
        c=$(cat "$p/comm" 2>/dev/null) || continue
        case $c in zpool|txg_sync) ;; *) continue ;; esac
        st=$(sed 's/^.*) //' "$p/stat" 2>/dev/null | cut -c1)
        if [ "$c" = txg_sync ]; then txg+="${st:-?}"; continue; fi
        [ "$st" = D ] || continue
        n_d=$((n_d + 1))
        fn=$(lim 5 cat "$p/stack" 2>/dev/null | grep -oE '(spa_namespace_enter|spa_config_enter|txg_wait_synced|spa_vdev_state_enter|zfs_ioc_[a-z_]+)' | head -n 1)
        if [ "$fn" = spa_namespace_enter ]; then ns=$((ns + 1)); else named+="${named:+, }${p##*/} in ${fn:-?}"; fi
    done
    k="zpool processes waiting in the kernel: $n_d"
    [ "$n_d" = 0 ] || k+=" ($ns for the namespace lock${named:+; $named})"
    k+="; txg_sync threads' states: ${txg:-none}"
    if [ -n "${1:-}" ]; then
        if klog_since "$1" > "$OUT/stuck.tmp"; then
            n=$(grep -c 'blocked for more than' "$OUT/stuck.tmp" || true)
            shown=$(grep -c 'task:txg_sync ' "$OUT/stuck.tmp" || true)
            run=$(grep -c 'task:txg_sync .*state:R' "$OUT/stuck.tmp" || true)
            owners=$(grep -oE 'likely owned by task [^ ]+' "$OUT/stuck.tmp" | awk '{ sub(/\.$/, "", $NF); print $NF }' | sort | uniq -c | sort -rn | head -n 4 | awk '{ printf "%s%s x%s", (NR > 1 ? ", " : ""), $2, $1 }')
            k+="; the kernel since the mark: $n hung-task reports, txg_sync shown $shown times and running in $run of them, lock owners named: ${owners:-none}"
        else
            k+="; the kernel log since the mark is lost"
        fi
    fi
    echo "$k"
}

set -euo pipefail
skz_cgroup_mem() { # the daemon's cgroup: what it holds, its limit's events, its reclaim
    local skzmax
    [ -n "$SKZ_PID" ] && [ -n "$SKZ_CGROUP" ] && [ -f "$SKZ_CGROUP/memory.stat" ] || return 0
    echo "mem skz.cgroup.current $(cat "$SKZ_CGROUP/memory.current")"
    skzmax=$(cat "$SKZ_CGROUP/memory.max"); [ "$skzmax" = max ] || echo "mem skz.cgroup.max $skzmax"
    awk '$1 ~ /^(anon|file|shmem|kernel|file_dirty|file_writeback)$/ { print "mem skz.cgroup." $1, $2 }' "$SKZ_CGROUP/memory.stat"
    awk '$1 ~ /^(max|high|oom|oom_kill)$/ { print "mem skz.cgroup.events." $1, $2 * 1048576 }' "$SKZ_CGROUP/memory.events" 2>/dev/null || true
    # Reclaim in the group, counted in pages since it was made: what was
    # scanned and taken back, by the writer itself (direct) or by
    # kswapd, and the file pages read again soon after they were taken
    # (refaults). Counts, sent as n MiB.
    awk '$1 ~ /^(pgscan|pgsteal|pgscan_direct|pgsteal_direct|pgscan_kswapd|pgsteal_kswapd|workingset_refault_file|workingset_activate_file|pgmajfault)$/ { printf "mem skz.cgroup.stat.%s %.0f\n", $1, $2 * 1048576 }' "$SKZ_CGROUP/memory.stat"
}
pressure_look() { # the pressure stall totals and the cgroups' CPU, and when they were read
    # Pressure stall information: the time some (and, for io and
    # memory, all) runnable tasks waited for the resource, in us: the
    # machine's, postgres's cgroup's (pg.) and the daemon's (skz.).
    # Postgres's only when it is known to be in its cgroup (PG_CG_OK).
    # A file that cannot be read (PSI off: it is there and its read
    # fails) leaves its keys out.
    local res cg pre f
    echo "clock now $(date +%s%N)"
    for cg in "/proc/pressure:" "$([ -z "${PG_CG_OK:-}" ] || echo "${CGROUP:-}"):pg." "${SKZ_CGROUP:-}:skz."; do
        pre=${cg##*:}; cg=${cg%:*}
        [ -n "$cg" ] || continue
        for res in cpu io memory; do
            if [ "$cg" = /proc/pressure ]; then f="$cg/$res"; else f="$cg/$res.pressure"; fi
            [ -r "$f" ] || continue
            awk -v r="$pre$res" '{ for (i = 2; i <= NF; i++) if ($i ~ /^total=/) { sub("total=", "", $i); print "psi", r "." $1, $i } }' "$f" 2>/dev/null || true
        done
        # The cgroup's CPU: usage, user and system, in us.
        [ "$cg" = /proc/pressure ] || [ ! -r "$cg/cpu.stat" ] \
            || awk -v p="$pre" '$1 ~ /^(usage_usec|user_usec|system_usec)$/ { print "cgs", p $1, $2 }' "$cg/cpu.stat" 2>/dev/null || true
    done
    # The daemon's cgroup as the window ends: its reclaim and what it
    # holds, before the sync after the window adds its own.
    skz_cgroup_mem
}
SKZ_PID=1; SKZ_CGROUP=$1; pressure_look > $1/look; echo "status $?"; SKZ_PID=""; pressure_look > $1/look-none; echo "status without a daemon $?"

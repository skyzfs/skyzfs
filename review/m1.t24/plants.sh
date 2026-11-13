#!/bin/bash
# Plants for the review of M1.T24 (skyzfs/skyzfs#51) at 50820e3.
# Run from a clean checkout of that commit: plants.sh <dir of plant.py>
set -u
P="python3 $1/plant.py"
T=tests/bench/topology-bench.sh
run() { bash tests/bench/test_copy_limit.sh 2>/dev/null | grep -E "^FAIL|all passed|failed$" | cut -c1-110 | head -8; git checkout -q -- tests .github; }
echo "=== a1: the copy has no limit (caught)"
$P $T 'lim "$SKZ_COPY_LIMIT_S" "$DD" if="$WORK/zfs-origin.img"' '"$DD" if="$WORK/zfs-origin.img"' && run
echo "=== a2: a copy that ran out takes no evidence (caught)"
$P $T '    if [ "$rc" = 124 ]; then
        log "skyzfsd: the copy of the pool image onto $dev did NOT end' '    if [ "$rc" = 999 ]; then
        log "skyzfsd: the copy of the pool image onto $dev did NOT end' && run
echo "=== a3: the sync has no limit (passes)"
$P $T '    lim "$SKZ_COPY_LIMIT_S" sync || rc=$?' '    sync || rc=$?' && run
echo "=== a4: a sync that ran out is taken for one that ended (passes)"
$P $T '    if [ "$rc" = 124 ]; then
        log "skyzfsd: the sync after the copy' '    if false; then
        log "skyzfsd: the sync after the copy' && run
echo "=== a5: the repetitions go on after one that ran out (caught)"
$P $T '            hung="repetition $i of $n"
            break' '            hung="repetition $i of $n"' && run
echo "=== a6: the row goes on after a copy that ran out, the call site (passes)"
$P $T '            skz_copy "$dev" || return 1' '            skz_copy "$dev" || true' && run
echo "=== a9: a kernel stack is read with no limit (passes)"
$P $T '            lim 5 cat "$t/stack" 2>&1' '            cat "$t/stack" 2>&1' && run
echo "=== a11: the repetition exits 0 whatever happened, the call site (passes)"
$P $T '                exit "$rrc"' '                exit 0' && run
echo "=== a12: the daemon is always said to be alive (caught)"
$P $T '    if [ -n "${SKZ_PID:-}" ] && kill -0 "$SKZ_PID" 2>/dev/null; then' '    if true; then' && run

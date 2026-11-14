#!/bin/bash
# Reproducer for #82: the kill loop's read of an exit 7 between kills.
# Runs the rig's own lines of kill_test, from the server's pid to the
# end of the new check, cut from daemon-rig.sh by text, with a made-up
# server and a made-up `ublk` (for the watcher). No kernel, no root.
# Usage: repro82.sh DAEMON_RIG
set -u
RIG=$1
T=$(mktemp -d); trap 'rm -rf "${T:?}"' EXIT
mkdir -p "$T/bin"; printf '#!/bin/sh\nexit 0\n' > "$T/bin/ublk"; chmod +x "$T/bin/ublk"; PATH="$T/bin:$PATH"
for f in now_ms serve_wait kt_refusal kill_verdict server_pid; do
    eval "$(awk -v f="$f" '$0 ~ "^" f "\\(\\) *\\{" {p=1; print; if ($0 ~ /\}$/) exit; next} p {print} p && /^}/ {exit}' "$RIG")"
    declare -F "$f" > /dev/null || { echo "no $f in the rig"; exit 2; }
done
grep -q '^EXIT_FAILED=3; EXIT_REFUSED_FAILED=4; EXIT_BUSY=5; EXIT_BROKEN_RULE=7$' "$RIG" && eval "$(grep '^EXIT_FAILED=3;' "$RIG")"
vol_stat() { echo ""; }   # no status file: server_pid falls back to SERVE_PID
# The loop body, from `pid=$(server_pid` to the new check's `fi`.
body=$(awk '/^kill_test\(\) *\{/{k=1} k && /pid=\$\(server_pid/{p=1} p && /# KT_WAIT_S:/{exit} p{print}' "$RIG")
[ -n "$body" ] || { echo "the loop body was not found"; exit 2; }
run() { # label, the made-up server's command
    local impl=x dir=$T/vol name=r inflight=$T/inflight BDEV=/dev/ublkb9 n w i pid tk watcher
    echo "0 1" > "$inflight"   # a write in flight: the loop's look ends at once
    KT_ERR=""; KT_REFUSED=""; KT_INFLIGHT=0; KT_DONE=1; SERVE_RC=""
    KT_FAILED_WRITES=0; KT_STATE=ONLINE; KT_VDEV="vdev errors 0 x"; KT_SCRUB="vdev errors 0 with 0 errors"; KT_FILES=ok
    : > "$dir.serve.log"; echo "skyzfsd: the kernel handed over tag 3 of device 9, against the driver's rule" >> "$dir.serve.log"
    bash -c "$2" & SERVE_PID=$!
    sleep 0.3   # the server ran a while, as between two kills
    WORK=$T
    for n in 1; do eval "$body"; done
    [ -z "${watcher:-}" ] || wait "$watcher" 2>/dev/null
    kill_verdict pin
    echo "$1: SERVE_RC=${SERVE_RC:-unread}; KT_ERR='$KT_ERR'; KT_REFUSED='${KT_REFUSED:0:90}'; verdict $KT_VERDICT"
    [ -z "$SERVE_PID" ] || { wait "$SERVE_PID"; echo "   the status bash still holds for the server: $?"; }
}
run "a server that ends by itself with 7 before the kill" 'exit 7'
run "a server that ends by itself with 1 before the kill" 'exit 1'
run "a server alive at the kill" 'sleep 30'

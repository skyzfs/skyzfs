#!/bin/bash
# Tests of klog.sh with a made-up kernel log: no kernel is needed.
# Run: bash tests/kernel/test_klog.sh
HERE=$(cd "$(dirname "$0")" && pwd)
# shellcheck source=tests/kernel/klog.sh
. "$HERE/klog.sh"
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
LOG=$T/log; DMESG_RC=0
dmesg() { cat "$LOG"; return "$DMESG_RC"; }

cat > "$LOG" <<'LOGEOF'
[  10.000000] blk_update_request: I/O error, dev ublkb0, sector 8
[  11.000000] skz-rig-mark-1-1
[  12.000000] a quiet line
[  13.000000] INFO: task txg_sync:4242 blocked for more than 122 seconds.
[  13.000001] "echo 0 > /proc/sys/kernel/hung_task_timeout_secs" disables this message.
[  13.000002]  schedule_timeout+0x9c/0x110
[  13.000003]  ? io_schedule_timeout+0x51/0x80
LOGEOF

# The marker is there and nothing after it is bad: 0, and the line
# before the marker is not counted.
out=$(klog_bad_since skz-rig-mark-1-1 "$T/cut"); rc=$?
is "clean interval counts 0" "$out" 0
is "clean interval returns 0" "$rc" 0
is "the cut starts after the marker" "$(wc -l < "$T/cut" | tr -d ' ')" 5
is "blocked tasks are named" "$(klog_hung "$T/cut")" " 1 txg_sync:4242 "

# A bad line after the marker is counted.
echo '[  14.000000] ublk0: request 7 timed out, aborting' >> "$LOG"
out=$(klog_bad_since skz-rig-mark-1-1 "$T/cut"); rc=$?
is "a bad line is counted" "$out" 1
is "a bad line still returns 0" "$rc" 0

# The marker is gone (the buffer wrapped): not 0 lines, but lost.
sed -i '/skz-rig-mark-1-1/d' "$LOG"
out=$(klog_bad_since skz-rig-mark-1-1 "$T/cut"); rc=$?
is "no marker is lost, not 0" "$out" lost
is "no marker returns 3" "$rc" 3
is "no marker leaves the file empty" "$(wc -c < "$T/cut" | tr -d ' ')" 0
klog_since skz-rig-mark-1-1 > "$T/cut2"; is "klog_since returns 3 with no marker" "$?" 3
# What a caller's test makes of it: "lost" is not 0.
if [ "$out" = 0 ]; then v=PASS; else v=not-PASS; fi
is "a comparison with 0 does not pass" "$v" not-PASS

# dmesg fails and prints nothing: lost as well.
: > "$LOG"; DMESG_RC=1
out=$(klog_bad_since skz-rig-mark-1-1 "$T/cut"); rc=$?
is "a failed dmesg is lost" "$out" lost
is "a failed dmesg returns 3" "$rc" 3

# An empty mark (the write to /dev/kmsg failed and the caller kept
# nothing) must not match every line.
DMESG_RC=0; echo '[  1.0] some line' > "$LOG"
out=$(klog_bad_since "" "$T/cut"); rc=$?
is "an empty mark is lost" "$out" lost

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"

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

# What counts as bad: one sample line of each kind, counted alone.
bad_of() { printf '%s\n' "$1" > "$T/one"; klog_bad "$T/one"; }
while IFS= read -r line; do
    [ -n "$line" ] || continue
    is "counted: ${line#*] }" "$(bad_of "$line")" 1
done <<'LOGEOF'
[  1.0] I/O error, dev ublkb0, sector 2048 op 0x1:(WRITE) flags 0x0 phys_seg 1 prio class 2
[  1.0] timeout error, dev ublkb0, sector 2048 op 0x1:(WRITE) flags 0x0 phys_seg 1 prio class 2
[  1.0] critical space allocation error, dev ublkb1, sector 8 op 0x1:(WRITE) flags 0x0 phys_seg 1 prio class 2
[  1.0] device offline error, dev ublkb0, sector 0 op 0x0:(READ) flags 0x0 phys_seg 1 prio class 2
[  1.0] duration limit exceeded error, dev ublkb12, sector 0 op 0x0:(READ) flags 0x0 phys_seg 1 prio class 2
[  1.0] critical medium error, dev ublkb0, sector 0 op 0x0:(READ) flags 0x0 phys_seg 1 prio class 2
[  1.0] Buffer I/O error on dev ublkb0, logical block 0, async page read
[  1.0] ublk0: request 7 timed out
[  1.0] ublk_abort_queue: aborting queue 0
LOGEOF
# What does not: a hung-task report, whole, and other devices' talk.
while IFS= read -r line; do
    [ -n "$line" ] || continue
    is "not counted: ${line#*] }" "$(bad_of "$line")" 0
done <<'LOGEOF'
[  1.0] INFO: task txg_sync:4242 blocked for more than 122 seconds.
[  1.0] "echo 0 > /proc/sys/kernel/hung_task_timeout_secs" disables this message.
[  1.0]  schedule_timeout+0x9c/0x110
[  1.0]  ? io_schedule_timeout+0x51/0x80
[  1.0]  __cv_timedwait_common+0x12d/0x170 [spl]
[  1.0]  blk_abort_request+0x10/0x40
[  1.0] task:txg_sync        state:D stack:0     pid:4242  tgid:4242  ppid:2      flags:0x00004000
[  1.0] WARNING: Pool 'skzd-d1' has encountered an uncorrectable I/O failure and has been suspended.
[  1.0] ublk0: detected capacity change from 0 to 8388608
LOGEOF
# A line that is bad twice over is one line.
is "one line is counted once" "$(bad_of '[  1.0] I/O error, dev ublkb0, sector 8: request timed out, abort')" 1

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"

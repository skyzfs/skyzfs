#!/bin/bash
# The kernel log of an interval, for the rigs. Sourced, not run.
#
# The interval starts at a line of our own written to the log, not at a
# line count: once the ring buffer has wrapped the count stops growing
# and a cut by count would be empty.
#
# A lost marker is not an empty log. klog_since returns 3 and prints
# nothing when the marker is not in the log (the buffer wrapped past
# it, the marker could not be written, or dmesg failed), and a caller
# that counts lines must then say that it has no log, and not count 0.
# klog_bad_since does that for the count every pin uses.

klog_mark() { local m; m="skz-rig-mark-$$-$(date +%s%N)"; echo "$m" > /dev/kmsg; echo "$m"; }
klog_since() { [ -n "$1" ] || return 3; dmesg 2>/dev/null | awk -v m="$1" 'found { print } index($0, m) { found = 1 } END { exit found ? 0 : 3 }'; }

# What the kernel log says went wrong with a device, in a file cut from
# it: a request that ended in an error, requests timed out, aborts.
# The block layer reports a failed request as "<status> error, dev
# ublkbN, sector ...", and the status is not only "I/O": "timeout",
# "critical space allocation", "device offline", "duration limit
# exceeded" and others. Every status is counted by "error, dev ublkb";
# the words "I/O error" stay for reports from other layers (a buffer,
# a filesystem). Not the bare word "timeout": a hung-task report is
# full of it (hung_task_timeout_secs, schedule_timeout in its stack)
# and says only that a task waited.
KLOG_BAD='error, dev ublkb|I/O error|timed out|abort'
klog_bad() { grep -vE '^\[[^]]*\] +(\? )?[A-Za-z0-9_.]+\+0x|hung_task_timeout_secs' "$1" 2>/dev/null | grep -ciE "$KLOG_BAD"; }
# The tasks the kernel reported as blocked, "name:pid" each, counted.
klog_hung() { sed -n 's/.*INFO: task \([^ ]*\) blocked for more than \([0-9]*\) seconds.*/\1/p' "$1" 2>/dev/null | sort | uniq -c | tr -s ' \n' ' '; }

# mark file: cut the log from the mark into the file and print the
# count of klog_bad, or the word "lost" (and return 3, the file left
# empty) when the marker was not found. "lost" is not a number on
# purpose: a test that compares it with 0 fails, it does not pass.
klog_bad_since() {
    if klog_since "$1" > "$2"; then klog_bad "$2"; return 0; else : > "$2"; echo lost; return 3; fi
}

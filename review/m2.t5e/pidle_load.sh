#!/usr/bin/env bash
# Third look at #145: pidle_head.sh under CPU load. Runs J copies of
# pidle_head.sh at once (N runs of each line in each copy) beside B busy
# loops, and prints the load before and after and every copy's counts.
# Usage: bash pidle_load.sh WORKTREE N J B TAG
set -u
WT=$1 N=$2 J=$3 B=$4 TAG=$5
HERE=$(cd "$(dirname "$0")" && pwd)
burn=()
for _ in $(seq "$B"); do (while :; do :; done) & burn+=($!); done
trap 'kill "${burn[@]}" 2>/dev/null' EXIT
sleep 2
echo "$TAG: $J copies of $N runs each beside $B busy loops; $(cut -d' ' -f1-3 /proc/loadavg) at the start ($(nproc) CPUs)"
pids=()
out=$(mktemp -d)
for j in $(seq "$J"); do bash "$HERE/pidle_head.sh" "$WT" "$N" "copy $j" > "$out/$j" & pids+=($!); done
wait "${pids[@]}"
echo "load at the end: $(cut -d' ' -f1-3 /proc/loadavg)"
cat "$out"/*
rm -r "$out"

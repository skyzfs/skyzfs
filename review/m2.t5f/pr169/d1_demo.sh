#!/bin/bash
# D1: how long the read side of a round lasts. Takes reading_for and
# secs_between from the recorder as it is (argument: the recorder's
# path), runs a "read" of 4.9 s and one of 5.1 s in the 10 s window the
# rounds give a step read of that length, and prints the window it
# made. The idle control's window is idle_for 10, ten seconds.
# usage: bash d1_demo.sh tests/kernel/zfs-cli-record.sh
set -u
eval "$(sed -n '/^secs_between() {/,/^}/p; /^reading_for() {/,/^}/p' "$1")"
for d in 4.9 5.1; do
    a=$(date +%s.%N)
    out=$(reading_for 10 sleep "$d")
    b=$(date +%s.%N)
    echo "run of $d s in a window of 10 s: $out, the window $(secs_between "$a" "$b") s"
done

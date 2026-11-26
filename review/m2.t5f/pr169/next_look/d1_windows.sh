#!/bin/bash
# D1 at the new head: each side's window as `writing` measures it (its
# t0 before the command, t1 after, the file window that window_of and
# the probe's report read), for the idle control and for reads made
# through rec and lim as the rounds make them. Takes the recorder as it
# is (argument: its path). Cases: runs of 5.1 s and 4.9 s in a window
# of 10 s, many short runs, fast failures, a run that overshoots its
# prediction (4 s, then 7 s), a run longer than the window (12 s), and
# rec's limit (3 s) cutting the read.
# usage: bash d1_windows.sh tests/kernel/zfs-cli-record.sh
set -u
ZCR_LIB=1
# shellcheck disable=SC1090
. "$1"
OUT=$(mktemp -d)
trap 'rm -rf "${OUT:?}"' EXIT
: > "$OUT/index.tsv"
LIMIT=60
win() { # NAME COMMAND...: as writing's lines between its margins
    local name=$1 t0 t1
    shift
    t0=$(date +%s.%N)
    "$@" > /dev/null
    t1=$(date +%s.%N)
    mkdir -p "$OUT/$name"
    printf '%s\t%s\n' "$t0" "$t1" > "$OUT/$name/window"
}
grow() { # 4 s on its first run, 7 s on the next
    local n
    n=$(($(cat "$OUT/grow.n" 2> /dev/null || echo 0) + 1))
    echo "$n" > "$OUT/grow.n"
    if [ "$n" = 1 ]; then sleep 4; else sleep 7; fi
}
say() { # NAME WHAT
    printf '%-34s window %6s s  %s\n' "$2" "$(window_of "$1")" "$(cat "$OUT/$1/stdout" 2> /dev/null | head -n 1)$( [ -e "$OUT/$1/rc" ] && echo " rc=$(cat "$OUT/$1/rc")")"
}
win idle idle_for 10;                               say idle "idle_for 10"
win r51 rec r51 reading_for 10 sleep 5.1;           say r51 "reads of 5.1 s"
win r49 rec r49 reading_for 10 sleep 4.9;           say r49 "reads of 4.9 s"
win r002 rec r002 reading_for 10 sleep 0.02;        say r002 "reads of 0.02 s"
win rfalse rec rfalse reading_for 10 false;         say rfalse "reads that fail at once"
win rgrow rec rgrow reading_for 10 grow;            say rgrow "a run of 4 s, then one of 7 s"
win r12 rec r12 reading_for 10 sleep 12;            say r12 "a run of 12 s"
LIMIT=3
win rcut rec rcut reading_for 10 sleep 1;           say rcut "rec's limit of 3 s"

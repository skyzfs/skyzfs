#!/bin/bash
# How often `the_suite_notices_the_defects` (which holds that the planted
# ACK_BEFORE_WRITE is caught) passes at 45b67f2, unplanted:
#   alone, alone on one CPU, and beside the whole library suite.
#   ackloop.sh BINARY OUTDIR RUNS
BIN=$1; OUT=$2; N=$3
mkdir -p "$OUT"
T=inplace::tests::the_suite_notices_the_defects
run() { # label, command...
    local label=$1 ok=0 bad=0 i; shift
    for i in $(seq 1 "$N"); do
        if "$@" > "$OUT/$label-$i.txt" 2>&1; then ok=$((ok + 1)); rm -f "$OUT/$label-$i.txt"; else bad=$((bad + 1)); fi
    done
    echo "$label: passed $ok of $N, failed $bad"
}
run alone "$BIN" --exact "$T"
run one-cpu taskset -c 0 "$BIN" --exact "$T"
run whole-suite "$BIN"
grep -h "panicked\|is not noticed\|FAILED" "$OUT"/*.txt 2>/dev/null | sort | uniq -c | cut -c1-400

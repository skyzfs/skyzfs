#!/bin/bash
# The review's experiment on #76: the first push's gap, idle and under load.
B=/home/user/flaky-bins/exp76
X=inplace::pool::tests::review_experiment_first_push
T=inplace::pool::tests::a_job_wakes_one_waiting_thread
run() { REVIEW_STARTS=$1 REVIEW_GAP_MS=$2 "$B" --ignored --exact "$X" --nocapture 2>&1 | grep '^REVIEW'; }
echo "== idle"
run 2000 0
run 2000 20
echo "== under load: 8 busy loops on $(nproc) CPUs"
pids=()
for i in 1 2 3 4 5 6 7 8; do ( while :; do :; done ) & pids+=($!); done
run 2000 0
run 2000 20
f=0
for i in $(seq 1 200); do "$B" -q --exact "$T" > /dev/null 2>&1 || f=$((f+1)); done
echo "REVIEW #76's test under load: $f of 200 failed"
kill "${pids[@]}"
wait "${pids[@]}" 2>/dev/null
echo "load stopped"

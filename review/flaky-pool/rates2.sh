#!/bin/bash
# head binary, base binary, output dir
HEAD_BIN=$1; BASE_BIN=$2; OUT=$3
T=inplace::pool::tests::a_job_wakes_one_waiting_thread
a=0
for i in $(seq 1 60); do "$HEAD_BIN" -q "$T" > /dev/null 2>&1 || a=$((a+1)); done
echo "head alone: $a of 60 failed"
for which in head base; do
    if [ "$which" = head ]; then B=$HEAD_BIN; else B=$BASE_BIN; fi
    f=0
    for i in 1 2 3 4 5; do
        "$B" -q > "$OUT/$which-par-a-$i.txt" 2>&1 &
        "$B" -q > "$OUT/$which-par-b-$i.txt" 2>&1 &
        wait
        for x in a b; do
            if ! grep -q 'test result: ok' "$OUT/$which-par-$x-$i.txt"; then
                f=$((f+1)); grep -E '^---- ' "$OUT/$which-par-$x-$i.txt" | head -2
            fi
        done
    done
    echo "$which: two suites at once, 5 rounds: $f of 10 suites failed"
done

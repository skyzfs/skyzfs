#!/bin/bash
# rep.sh BIN N [load]: run the two timing tests N times; with "load", beside 8 busy loops.
bin=$1; n=$2; load=${3:-}
pids=()
if [ "$load" = load ]; then for _ in 1 2 3 4 5 6 7 8; do ( while :; do :; done ) & pids+=($!); done; fi
fail=0
for i in $(seq "$n"); do
  out=$("$bin" a_read_waits_for_the_reader_and_for_nothing_of_the_worker a_flush_that_arrives_during_a_step_is_counted_with_what_was_left_of_the_step 2>&1) || { fail=$((fail+1)); echo "$out" | grep -E "panicked|assert|FAILED" | head -4; }
done
[ ${#pids[@]} = 0 ] || kill "${pids[@]}" 2>/dev/null
echo "runs $n, failed $fail"

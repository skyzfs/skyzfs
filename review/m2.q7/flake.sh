#!/bin/bash
S=$1; cd $S/head
export CARGO_TARGET_DIR=$S/q7/target CARGO_INCREMENTAL=0
O=$S/q7/flake.out; : > $O
for i in $(seq 1 30); do
  timeout 300 cargo test -p skynbd-refs --lib -- renews_in_one take_committed 2>&1 | grep -E "^test |test result" >> $O; echo "race run $i exit ${PIPESTATUS[0]}" >> $O
done
for i in 1 2 3 4; do (timeout 900 sh -c 'while :; do :; done' &); done
for i in $(seq 1 15); do
  timeout 300 cargo test -p skynbd-refs --lib -- renews_in_one take_committed 2>&1 | grep -E "test result" >> $O; echo "loaded race run $i exit ${PIPESTATUS[0]}" >> $O
  timeout 600 cargo test -p skyzfs-local --features test-hooks --lib -- failed_store 2>&1 | grep -E "test result" >> $O; echo "loaded local run $i exit ${PIPESTATUS[0]}" >> $O
done
pkill -f "while :; do :; done"
echo done >> $O

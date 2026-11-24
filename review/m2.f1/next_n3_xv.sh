#!/bin/bash
# The review's cross-version check of status code 6: the writer's half at
# NEW, the reader's half at OLD, in one worktree, each test appended to
# crates/skyzfs-ublk/src/status/tests.rs and taken out after.
# Usage: run.sh TREE NEW OLD DIR (DIR holds next_n3_xv_write.rs and next_n3_xv_read.rs)
TREE=$1; NEW=$2; OLD=$3; DIR=$4
cd "$TREE" || exit 2
T=crates/skyzfs-ublk/src/status/tests.rs
OUTF=$(mktemp -p "$DIR")
back=$(git rev-parse HEAD)
trap 'git checkout -q -- $T; git checkout -q --detach $back' EXIT
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_BACKTRACE=0
git checkout -q --detach "$NEW" && cat "$DIR/next_n3_xv_write.rs" >> $T
echo "== at $(git rev-parse --short HEAD): write"
REVIEW_STATUS_OUT="$DIR/status-row_v1.bin" cargo test -q -p skyzfs-ublk --lib review_write_a_status_that_says_row_v1 2>&1 | grep -E '^test result|panicked|error'
git checkout -q -- $T
git checkout -q --detach "$OLD" && cat "$DIR/next_n3_xv_read.rs" >> $T
echo "== at $(git rev-parse --short HEAD): read"
REVIEW_STATUS_IN="$DIR/status-row_v1.bin" cargo test -q -p skyzfs-ublk --lib review_read_a_status_that_says_row_v1 -- --nocapture > "$OUTF" 2>&1
grep -E '^test result|panicked|error|REVIEW' "$OUTF"
rm -f "$OUTF"

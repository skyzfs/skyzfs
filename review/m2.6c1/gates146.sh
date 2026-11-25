#!/bin/bash
# The gates of the look at #146, run in a clean worktree of the head.
# Usage: gates146.sh <worktree> <log>
W=$1; LOG=$2
cd "$W" || exit 2
run() { echo "=== $* ===" >>"$LOG"; "$@" >>"$LOG" 2>&1; echo "=== exit $? ===" >>"$LOG"; }
: >"$LOG"
git -C "$W" rev-parse HEAD >>"$LOG"
rustc --version >>"$LOG"
run cargo fmt --all -- --check
run cargo clippy -p skyzfs-agent --all-targets -- -D warnings
run cargo test -p skyzfs-agent
RUSTDOCFLAGS="-D warnings" run cargo doc -p skyzfs-agent --no-deps
run cargo clippy --workspace --all-targets -- -D warnings
run cargo clippy --workspace --all-targets --features skyzfsd/test-hooks,skyzfs-local/test-hooks -- -D warnings
run cargo clippy --workspace --all-targets --features skyzfs-ublk-sys/control-test-program -- -D warnings
echo "=== all done ===" >>"$LOG"

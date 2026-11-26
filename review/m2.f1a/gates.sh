#!/bin/bash
# The gates on a clean checkout of the head, one cargo command at a time.
# Usage: gates.sh WORKTREE TARGET OUT
set -u
WT=$1; export CARGO_TARGET_DIR=$2; OUT=$3
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_TERM_COLOR=never
cd "$WT" || exit 2
: > "$OUT"
run() {
    local name=$1; shift
    local t0=$(date +%s)
    "$@" > "$OUT.$name.log" 2>&1
    local rc=$?
    echo "$name	rc=$rc	$(( $(date +%s) - t0 ))s	$*" >> "$OUT"
}
echo "head $(git rev-parse HEAD)  $(rustc --version)  $(cargo clippy --version)" >> "$OUT"
run fmt cargo fmt --all -- --check
run clippy-skyzfsd cargo clippy -p skyzfsd --all-targets -- -D warnings
run clippy-skyzfsd-hooks cargo clippy -p skyzfsd --all-targets --features test-hooks -- -D warnings
run clippy-ws cargo clippy --workspace --all-targets -- -D warnings
run test-skyzfsd cargo test -p skyzfsd --no-fail-fast
run test-skyzfsd-hooks cargo test -p skyzfsd --features test-hooks --no-fail-fast
run test-ws cargo test --workspace --no-fail-fast
export RUSTDOCFLAGS="-D warnings"
run doc-skyzfsd cargo doc -p skyzfsd --no-deps
run doc-skyzfsd-hooks cargo doc -p skyzfsd --no-deps --features test-hooks
run doc-ws cargo doc --workspace --no-deps
echo done >> "$OUT"

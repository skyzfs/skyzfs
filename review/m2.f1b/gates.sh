#!/bin/bash
# The gates at the PR head in a clean worktree. Usage: gates.sh TREE OUTDIR
TREE=$1; OUT=$2
cd "$TREE" || exit 2
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_BACKTRACE=0
echo "== at $(git rev-parse --short HEAD), $(rustc --version)" > $OUT/gates.out
step() { name=$1; shift; echo "== $name" >> $OUT/gates.out; "$@" > "$OUT/g_${name// /_}.log" 2>&1; echo "$name exit $?" >> $OUT/gates.out; }
step "fmt" cargo fmt --all -- --check
step "clippy plain" cargo clippy --workspace --all-targets -- -D warnings
step "clippy hooks" cargo clippy --workspace --all-targets --features skyzfsd/test-hooks,skyzfs-local/test-hooks -- -D warnings
step "clippy ctl" cargo clippy --workspace --all-targets --features skyzfs-ublk-sys/control-test-program -- -D warnings
step "test plain" cargo test --workspace
step "test hooks" cargo test --workspace --features skyzfsd/test-hooks,skyzfs-local/test-hooks
step "test ctl" cargo test --workspace --features skyzfs-ublk-sys/control-test-program
echo "== done" >> $OUT/gates.out
git status --porcelain | sed 's/^/dirty: /' >> $OUT/gates.out

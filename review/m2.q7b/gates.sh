#!/bin/bash
# The local gates at the head of #136, in a clean worktree of it.
# Usage: gates.sh WORKTREE OUTDIR TARGETDIR
W=$1; O=$2; T=$3
cd "$W" || exit 2
export CARGO_TARGET_DIR=$T CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=${JOBS:-3}
step() { local name=$1; shift; echo "== $name: $*" >> "$O/gates.out"; "$@" > "$O/$name.out" 2>&1; local rc=$?; echo "   exit $rc" >> "$O/gates.out"; }
: > "$O/gates.out"
echo "head $(git rev-parse HEAD)" >> "$O/gates.out"
rustc --version >> "$O/gates.out"; cargo clippy --version >> "$O/gates.out"
step fmt cargo fmt --all --check
step clippy cargo clippy --workspace --all-targets -- -D warnings
step clippy_hooks cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings
step clippy_ctp cargo clippy -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings
step t_plain cargo test --no-fail-fast -p skynbd-refs -p skyzfs-upload -p skyzfsd -p skyzfs-ublk -p skyzfs-local
step t_hooks cargo test --no-fail-fast -p skyzfsd --features test-hooks
step t_local_hooks cargo test --no-fail-fast -p skyzfs-local --features test-hooks
step t_ublksys cargo test --no-fail-fast -p skyzfs-ublk-sys --features control-test-program
git status --short >> "$O/gates.out"
echo done >> "$O/gates.out"

#!/bin/bash
# The gates at the head, workspace-wide, plain and with each feature set.
# Usage: gates153f.sh SRC TARGET  (full output on stdout; summarise after)
set -u
SRC=$1; export CARGO_TARGET_DIR=$2
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cd "$SRC" || exit 2
rustc --version; git log --oneline -1
step() { echo "== $*"; "$@" 2>&1; echo "rc=$?"; df -h / | tail -1; }
step cargo fmt --all -- --check
for F in "" "skyzfsd/test-hooks,skyzfs-local/test-hooks" "skyzfs-ublk-sys/control-test-program" "skyzfs-agent-cli/test-hooks"; do
  if [ -z "$F" ]; then FA=(); else FA=(--features "$F"); fi
  step cargo clippy --workspace --all-targets "${FA[@]}" -- -D warnings
  step cargo test --workspace --no-fail-fast "${FA[@]}"
  RUSTDOCFLAGS='-D warnings' step cargo doc --workspace --no-deps "${FA[@]}"
done
echo == done

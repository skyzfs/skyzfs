#!/bin/bash
# The gates at the head, from a scratch archive of the head.
# Usage: gates153.sh SRC TARGET
set -u
SRC=$1; export CARGO_TARGET_DIR=$2
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cd "$SRC" || exit 2
rustc --version
step() { echo "== $*"; "$@"; echo "rc=$?"; }
step cargo fmt --all -- --check
step cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -- -D warnings
step cargo test -p skyzfs-agent -p skyzfs-control
RUSTDOCFLAGS='-D warnings' step cargo doc -p skyzfs-agent -p skyzfs-control --no-deps

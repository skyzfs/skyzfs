#!/bin/bash
# Gates for #81: fmt, clippy and tests of the two changed crates, plain and with the program's feature.
set -u
export CARGO_TARGET_DIR=/home/user/target-pr74
cd "$1"
r() { echo "== $*"; "$@" > /tmp/g.out 2>&1; rc=$?; grep -E 'test result|^error|^warning: unused|panicked|FAILED' /tmp/g.out | sort | uniq -c | head -20; echo "rc $rc"; }
r cargo fmt --all -- --check
r cargo clippy --locked -p skyzfs-ublk-sys -p skyzfs-ublk --all-targets -- -D warnings
r cargo clippy --locked -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings
r cargo test --locked -p skyzfs-ublk
r cargo test --locked -p skyzfs-ublk-sys --features control-test-program --bin skyzfs-ublk-control
r cargo test --locked -p skyzfs-ublk-sys

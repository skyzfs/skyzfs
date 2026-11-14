#!/bin/bash
# Gates for #87 at its head.
cd /home/user/skyzfs-pr87 || exit 2
export CARGO_TARGET_DIR=/home/user/target-pr73
run() { echo "== $*"; "$@" 2>&1 | tail -${TAILN:-6}; echo "rc ${PIPESTATUS[0]}"; }
run cargo fmt --all --check
run cargo clippy -p skyzfs-ublk -p skyzfs-ublk-sys -p skyzfsd --all-targets -- -D warnings
run cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings
run cargo clippy -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings
TAILN=4 run cargo test -p skyzfs-ublk --lib
TAILN=4 run cargo test -p skyzfs-ublk --tests
TAILN=4 run cargo test -p skyzfsd --features test-hooks --lib
TAILN=4 run cargo test -p skyzfsd --lib
TAILN=4 run cargo test -p skyzfsd --features test-hooks --tests
TAILN=12 run cargo test -p skyzfs-ublk-sys --features control-test-program
TAILN=12 run cargo test -p skyzfs-ublk-sys

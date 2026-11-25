#!/usr/bin/env bash
# Next look at #147: the gates at the head, from a clean checkout of it.
# CARGO_TARGET_DIR may point elsewhere. The workspace's tests are left to
# CI (the container's disk is shared and nearly full); the crates the
# diff touches are run here, plain, and clippy over the workspace with
# each feature set CLAUDE.md names.
set -u
run() { echo "== $*"; "$@"; echo "== exit $?"; }
rustc --version
git rev-parse HEAD
run cargo fmt --all --check
run cargo clippy --workspace --all-targets -- -D warnings
run cargo clippy --workspace --all-targets --features skyzfsd/test-hooks,skyzfs-local/test-hooks -- -D warnings
run cargo clippy --workspace --all-targets --features skyzfs-ublk-sys/control-test-program -- -D warnings
run cargo test -p skyzfs-agent -p skyzfs-control
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps -p skyzfs-agent -p skyzfs-control
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps --workspace

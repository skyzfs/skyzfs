#!/usr/bin/env bash
# Gates for #147 at its head: fmt, clippy (plain and with the features
# CLAUDE.md names), the tests of the crates touched and of skyzfsd (whose
# control tests use the client), and the docs. Run from the checkout of
# the head; CARGO_TARGET_DIR may point elsewhere.
set -u
run() { echo "== $*"; "$@"; echo "== exit $?"; }
rustc --version
git rev-parse HEAD
run cargo fmt --all --check
run cargo clippy --workspace --all-targets -- -D warnings
run cargo clippy --workspace --all-targets --features skyzfsd/test-hooks,skyzfs-local/test-hooks -- -D warnings
run cargo clippy --workspace --all-targets --features skyzfs-ublk-sys/control-test-program -- -D warnings
run cargo test -p skyzfs-agent -p skyzfs-control
run cargo test -p skyzfsd
run cargo test -p skyzfsd --features test-hooks
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps -p skyzfs-agent -p skyzfs-control

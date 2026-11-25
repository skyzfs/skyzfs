#!/usr/bin/env bash
# Third look at #147: the gates at the head, from a clean scratch
# checkout of it (CARGO_TARGET_DIR elsewhere, debug info off). fmt over
# the workspace; clippy -D warnings and the tests for the two crates the
# pull request touches, run as root. The workspace's clippy and tests
# are read from skyzfs-ci's run on the head.
set -u
run() { echo "== $*"; "$@"; echo "== exit $?"; }
rustc --version
git rev-parse HEAD
git status --short
run cargo fmt --all --check
run cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -- -D warnings
run cargo test -p skyzfs-agent -p skyzfs-control
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps -p skyzfs-agent -p skyzfs-control

#!/bin/bash
# The gates of the review of #149 at af50814, on the crates this package
# touches (skyzfs-agent, skyzfs-agent-cli), in the feature modes the PR
# names for them: plain, and with skyzfs-agent-cli/test-hooks. The
# workspace-wide clippy, tests and doc were not run here: the shared
# disk had under 500 MB free (see review.md, "Gates").
#   WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> bash gates149.sh
set -u
cd "$WORKTREE" || exit 2
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0
run() { echo "== $*"; "$@" > /tmp/gates149.$$ 2>&1; rc=$?; grep -E '^(test result|warning|error)' /tmp/gates149.$$ | sort | uniq -c; echo "exit $rc"; rm -f /tmp/gates149.$$; }
rustc --version
git log --oneline -1
run cargo fmt --all --check
run cargo clippy -p skyzfs-agent -p skyzfs-agent-cli --all-targets -- -D warnings
run cargo clippy -p skyzfs-agent -p skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks --all-targets -- -D warnings
run cargo test -p skyzfs-agent -p skyzfs-agent-cli
run cargo test -p skyzfs-agent -p skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps -p skyzfs-agent -p skyzfs-agent-cli
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps -p skyzfs-agent -p skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks
# The release build without the hooks: no hook code linked.
run cargo build -p skyzfs-agent-cli --release
B="$CARGO_TARGET_DIR/release/skyzfs-agent"
for s in 'killed at' 'trace-points: ' 'is a test hook' ; do printf '%s: %s\n' "$s" "$(grep -c -a -F "$s" "$B")"; done
echo "kill_process symbols: $(nm -C "$B" 2>/dev/null | grep -c -i kill_process)"
echo "== cargo tree: who enables skyzfs-agent-cli's features in the workspace"
cargo tree --workspace -e features -i skyzfs-agent-cli 2>&1 | sed "s#$WORKTREE#<wt>#"

#!/usr/bin/env bash
# Gates of #149 (fifth look; from the fourth look's script). Usage: gates149e.sh WORKTREE TARGET_DIR
# Run on a clean worktree of the head; the target dir is the reviewer's own.
set -u
wt=$1; export CARGO_TARGET_DIR=$2
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
cd "$wt" || exit 2
echo "head $(git rev-parse HEAD)"; rustc --version
run() {
  echo "== $*"; "$@" > "$CARGO_TARGET_DIR/gate.out" 2>&1; local rc=$?
  grep -E "^(test result|     Running|warning|error)" "$CARGO_TARGET_DIR/gate.out" | sed -E "s#\(/[^)]*/(deps|out)/#(#"
  [ $rc -ne 0 ] && tail -40 "$CARGO_TARGET_DIR/gate.out"
  echo "exit $rc"; rm -f "$CARGO_TARGET_DIR/gate.out"
}
P="-p skyzfs-agent -p skyzfs-agent-cli -p skyzfs-control"
run cargo fmt --all --check
run cargo clippy $P --all-targets -- -D warnings
run cargo clippy $P --all-targets --features skyzfs-agent-cli/test-hooks -- -D warnings
run cargo test $P
run cargo test $P --features skyzfs-agent-cli/test-hooks
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps $P
RUSTDOCFLAGS="-D warnings" run cargo doc --no-deps $P --features skyzfs-agent-cli/test-hooks
echo "== the N6 test alone, plain"
cargo test -p skyzfs-agent --lib a_withdraw_answered_does_not_count_for_the_request_asked_anew 2>&1 | grep -E "^test |test result"
echo "== C1's test alone, plain"
cargo test -p skyzfs-agent --lib a_request_is_sent_within_the_slack_of_its_record_or_not_at_all 2>&1 | grep -E "^test |test result"

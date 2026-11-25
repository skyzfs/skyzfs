#!/usr/bin/env bash
# Next look at skyzfs/skyzfs#150 (head d629834): run the scratch tests
# against a clean worktree of the head, then restore it.
#   WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> bash run150n.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
wt=${WORKTREE:?WORKTREE}
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
kt="$wt/bins/skyzfsd/src/kill/tests.rs"
cp "$kt" "$kt.orig"
restore() { mv -f "$kt.orig" "$kt"; }
trap restore EXIT
cat "$here/scratch150n.rs" >> "$kt"
cd "$wt" || exit 2
timeout 1200 cargo test -p skyzfsd --features test-hooks -- r150n_ --nocapture --test-threads 1 2>&1
echo "cargo test exit $?"

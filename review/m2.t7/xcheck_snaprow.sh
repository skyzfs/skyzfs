#!/bin/bash
# The review of #139: run node B's tests with xcheck_snaprow.rs appended
# to bins/skyzfs-nodeb/src/main.rs in WORKTREE, then put main.rs back.
# Usage: xcheck_snaprow.sh WORKTREE [cargo test filter]
set -u
wt=$1; filter=${2:-}
here=$(cd "$(dirname "$0")" && pwd)
main=$wt/bins/skyzfs-nodeb/src/main.rs
cp "$main" "$main.orig"
trap 'mv "$main.orig" "$main"' EXIT
cat "$here/xcheck_snaprow.rs" >> "$main"
cd "$wt" && timeout 2400 cargo test -p skyzfs-nodeb --no-fail-fast -- $filter 2>&1 \
    | grep -E '^test |test result|^error|panicked'

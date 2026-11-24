#!/bin/bash
# B1's fix, tried: the last Observed of observe() names the commit the
# engine is open at. The crate's tests and the review's scenarios run
# with it; the worktree is restored whatever happens.
# Usage: fix_b1.sh TREE OUT
TREE=$1 OUT=$2
F=crates/skyzfs-upload/src/remote.rs
T=crates/skyzfs-upload/tests/review_f1.rs
cd "$TREE" || exit 2
restore() { git checkout -- "$F"; rm -f "$T"; }
trap restore EXIT
cp "$(dirname "$OUT")/review_f1.rs" "$T"
n=$(grep -c '^            head: head.commit,$' "$F")
[ "$n" = 1 ] || { echo "site found $n times" > "$OUT"; exit 2; }
sed -i 's/^            head: head.commit,$/            head: engine.head_commit_hash(),/' "$F"
{
    echo "== the change"
    git diff -- "$F"
    echo "== cargo test -p skyzfs-upload --no-fail-fast"
    timeout 2400 cargo test --no-fail-fast -p skyzfs-upload 2>&1
    echo "exit $?"
    echo "== the scenarios, with their output"
    timeout 600 cargo test -p skyzfs-upload --test review_f1 -- --nocapture --test-threads 1 2>&1
    echo "exit $?"
} > "$OUT" 2>&1

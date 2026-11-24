#!/bin/bash
# The review's variant of B1's test (the race at the first cut, with no
# row), appended to the crate's tests/driver.rs in a worktree at the head
# and taken out after: run at the head, then with the plants B1H and B1
# (each alone), three runs each. Usage: next_variant.sh TREE next_variant.rs
TREE=$1; VAR=$2
cd "$TREE" || exit 2
OUTF=$(mktemp -p "$(dirname "$VAR")")
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 RUST_BACKTRACE=0
T=crates/skyzfs-upload/tests/driver.rs
R=crates/skyzfs-upload/src/remote.rs
cp $T $T.orig; cp $R $R.orig
trap 'mv $T.orig $T; mv $R.orig $R' EXIT
cat "$VAR" >> $T
run() {
  for i in 1 2 3; do
    timeout 1200 cargo test -q -p skyzfs-upload --test driver review_variant_race_at_the_first_cut_with_no_row > "$OUTF" 2>&1
    rc=$?
    echo "$1 run $i: exit $rc: $(grep -E '^test result|panicked' "$OUTF" | sed -E 's/, finished in.*//' | tr '\n' ' ')"
  done
  rm -f "$OUTF"
}
run "at the head"
sed -i 's|            head_group: engine.head_group(),|            head_group: engine.commit_group(\&head.commit).await?,|' $R
grep -c 'commit_group(&head.commit).await?,' $R | sed 's/^/B1H sites now: /'
run "B1H planted"
cp $R.orig $R
sed -i 's|            head: engine.head_commit_hash(),|            head: head.commit,|' $R
grep -c '^            head: head.commit,$' $R | sed 's/^/B1 sites now: /'
run "B1 planted"

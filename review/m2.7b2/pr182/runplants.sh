#!/bin/bash
# The plants of the review of PR 182 (plants.json from mk.py), each alone
# in a clean worktree of the head at $SCRATCH/wt182p, by ../plant.py: a
# baseline of the agent's library first, then each plant, restored after
# its run.
S=${SCRATCH:?set SCRATCH to the scratch directory}
N=${NOTES:?set NOTES to review/m2.7b2/pr182 of the notes checkout}
L=$S/plantlogs182
mkdir -p $L
export CARGO_TARGET_DIR=$S/t182p CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$S/wt182p
cd $S/wt182p || exit 9
R=$L/results.tsv   # copied to plants.out in the notes when done
[ "$1" = "--append" ] && shift || {
  : > $R
  echo "tree $(git rev-parse HEAD) $(git status --porcelain | wc -l) changed" >> $R
  cargo test -p skyzfs-agent --lib > $L/base.log 2>&1
  echo "base -p skyzfs-agent --lib rc=$? $(grep -E '^test result:' $L/base.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}')" >> $R
}
python3 -I $N/../plant.py $N/plants.json $R "$@"
echo "after: $(git status --porcelain | wc -l) changed" >> $R
echo plants-done >> $R

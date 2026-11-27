#!/bin/bash
# The plants of the next look (plants.json from mk.py), each alone in a
# clean worktree of the head at $SCRATCH/nl/plant, by ../plant.py: a
# baseline of each suite first, then each plant, restored after its run.
S=${SCRATCH:?set SCRATCH to the scratch directory}
N=${NOTES:?set NOTES to review/m2.7b2/next_look of the notes checkout}
L=$S/nl/plantlogs
mkdir -p $L
export CARGO_TARGET_DIR=$S/t179b CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$S/nl/plant
cd $S/nl/plant || exit 9
R=$L/results.tsv   # copied to plants.out in the notes when done
[ "$1" = "--append" ] && shift || {
  : > $R
  echo "tree $(git rev-parse HEAD) $(git status --porcelain | wc -l) changed" >> $R
  for c in "-p skyzfs-agent --lib" "-p skyzfs-upload --test retire" "-p skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks"; do
    tag=$(echo $c | tr ' /' '__')
    cargo test $c > $L/base$tag.log 2>&1
    echo "base $c rc=$? $(grep -E '^test result:' $L/base$tag.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}')" >> $R
  done
}
python3 -I $N/../plant.py $N/plants.json $R "$@"
echo "after: $(git status --porcelain | wc -l) changed" >> $R
echo plants-done >> $R

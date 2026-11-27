#!/bin/bash
# The plants at the second head (d68471f): a baseline, then each plant alone (plant.py).
S=${SCRATCH:?set SCRATCH to the scratch directory, with a clean worktree of the head at $SCRATCH/r179/head2}
N=$S/r179/notes/review/m2.7b2
L=$S/r179/plantlogs2
mkdir -p $L
export CARGO_TARGET_DIR=$S/t179b CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$S/r179/head2
cd $S/r179/head2 || exit 9
R=$L/results.tsv
: > $R
echo "tree $(git rev-parse HEAD) $(git status --porcelain | wc -l) changed" >> $R
cargo test -p skyzfs-agent --lib > $L/base.log 2>&1; echo "base skyzfs-agent --lib rc=$? $(grep -E '^test result:' $L/base.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}')" >> $R
python3 $N/plant.py $N/plants2.json $R "$@"
echo "after: $(git status --porcelain | wc -l) changed" >> $R
echo plants-done >> $R

#!/bin/bash
# The plants of the review of #179: baselines, then each plant alone (plant.py).
S=${SCRATCH:?set SCRATCH to the scratch directory}
N=$S/r179/notes/review/m2.7b2
L=$S/r179/plantlogs
export CARGO_TARGET_DIR=$S/tp179 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$S/r179/plant
cd $S/r179/plant || exit 9
R=$L/results.tsv
: > $R
echo "tree $(git rev-parse HEAD) $(git status --porcelain | wc -l) changed" >> $R
for c in "skyzfs-agent --lib" "skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks" "skyzfsd --features test-hooks --lib" "skyzfs-ublk --lib" "skyzfs-upload"; do
  cargo test -p $c > $L/base-${c%% *}.log 2>&1; echo "base $c rc=$? $(grep -E '^test result:' $L/base-${c%% *}.log | awk '{p+=$4; f+=$6} END {print p" passed "f" failed"}')" >> $R
done
python3 $N/plant.py $N/plants.json $R "$@"
echo "after: $(git status --porcelain | wc -l) changed" >> $R
echo plants-done >> $R

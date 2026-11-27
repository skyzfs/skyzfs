#!/bin/bash
# The next look's plants: baselines, then each plant alone (plant.py).
S=${SCRATCH:?set SCRATCH to the scratch directory}
N=$S/r175/next
export CARGO_TARGET_DIR=$S/tp175 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$S/r175/plant
cd $S/r175/plant || exit 9
: > $N/plantlogs/results.tsv
echo "tree $(git rev-parse HEAD)" >> $N/plantlogs/results.tsv
cargo test -p skyzfs-upload > $N/plantlogs/base-up.log 2>&1; echo "base-up rc=$?" >> $N/plantlogs/results.tsv
cargo test -p skyzfsd --features test-hooks --lib > $N/plantlogs/base-d.log 2>&1; echo "base-d rc=$?" >> $N/plantlogs/results.tsv
python3 $N/plant.py $N/plants.json $N/plantlogs/results.tsv
echo plants-done >> $N/plantlogs/results.tsv

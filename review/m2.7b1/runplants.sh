#!/bin/bash
S=${SCRATCH:?set SCRATCH to the scratch directory}
cd $S/r175
until grep -q warm-done warm1.log; do sleep 5; done
export CARGO_TARGET_DIR=$S/tp175 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$S/r175/plant
mkdir -p plantlogs
: > plantlogs/results.tsv
# baseline: the unplanted tests pass
(cd plant && cargo test -p skyzfs-upload --test retire --test uploader > ../plantlogs/base-up.log 2>&1; echo "base-up rc=$?" >> ../plantlogs/results.tsv)
(cd plant && cargo test -p skyzfs-control > ../plantlogs/base-ctl.log 2>&1; echo "base-ctl rc=$?" >> ../plantlogs/results.tsv)
(cd plant && cargo test -p skyzfsd --features test-hooks --lib > ../plantlogs/base-d.log 2>&1; echo "base-d rc=$?" >> ../plantlogs/results.tsv)
python3 tools/plant.py plants.json plantlogs/results.tsv
echo plants-done >> plantlogs/results.tsv

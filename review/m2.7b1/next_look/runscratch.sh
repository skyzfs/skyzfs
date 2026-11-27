#!/bin/bash
# Scratch tests S4-S6 appended in the plant worktree after the plants:
# rustfmt, clippy, the tests, then the plants they are meant for.
S=${SCRATCH:?set SCRATCH to the scratch directory}
N=$S/r175/next
T=$S/r175/plant
export CARGO_TARGET_DIR=$S/tp175 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$T
cd $T || exit 9
O=$N/scratch-tests.out
: > $O
echo "tree $(git rev-parse HEAD)" >> $O
git diff --quiet || { echo "tree not clean" >> $O; exit 9; }
python3 - "$N" "$T" <<'PY'
import sys
n, t = sys.argv[1], sys.argv[2]
def insert_before_last_brace(path, extra):
    text = open(path).read()
    i = text.rstrip().rfind('}')
    open(path, 'w').write(text[:i].rstrip('\n') + '\n' + extra + '}\n')
insert_before_last_brace(f'{t}/bins/skyzfsd/src/control/tests.rs', open(f'{n}/scratch-control.rs').read())
insert_before_last_brace(f'{t}/crates/skyzfs-upload/src/uploader.rs', open(f'{n}/scratch-uploader-unit.rs').read())
open(f'{t}/crates/skyzfs-upload/src/publish/tests.rs', 'a').write(open(f'{n}/scratch-publish-unit.rs').read())
open(f'{t}/crates/skyzfs-upload/tests/uploader.rs', 'a').write(open(f'{n}/scratch-uploader-it.rs').read())
PY
rustfmt --edition 2021 --check bins/skyzfsd/src/control/tests.rs crates/skyzfs-upload/src/uploader.rs crates/skyzfs-upload/src/publish/tests.rs crates/skyzfs-upload/tests/uploader.rs > $N/plantlogs/scratch-fmt.log 2>&1; echo "rustfmt rc=$?" >> $O
cargo clippy -p skyzfs-upload --all-targets -- -D warnings > $N/plantlogs/scratch-clippy-up.log 2>&1; echo "clippy upload rc=$?" >> $O
cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings > $N/plantlogs/scratch-clippy-d.log 2>&1; echo "clippy skyzfsd test-hooks rc=$?" >> $O
cargo test -p skyzfs-upload --lib --test uploader review_ > $N/plantlogs/scratch-up.log 2>&1; echo "test upload review_ rc=$?" >> $O
grep -E "^test .*review_|^test result" $N/plantlogs/scratch-up.log >> $O
cargo test -p skyzfsd --features test-hooks --lib review_ > $N/plantlogs/scratch-d.log 2>&1; echo "test skyzfsd review_ rc=$?" >> $O
grep -E "^test .*review_|^test result" $N/plantlogs/scratch-d.log >> $O
echo "# the plants they are meant for, with S4-S6 in:" >> $O
python3 $N/plant.py $N/plants.json $O conflict-text-generic conflict-text-noprefix retired-no-order cleared-on-error ended-text
git diff --stat >> $O
git checkout -- bins/skyzfsd/src/control/tests.rs crates/skyzfs-upload/src/uploader.rs crates/skyzfs-upload/src/publish/tests.rs crates/skyzfs-upload/tests/uploader.rs
echo scratch-done >> $O

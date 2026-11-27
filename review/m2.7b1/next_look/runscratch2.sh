#!/bin/bash
# Second pass: S4-S7 appended again (S4 as formatted), rustfmt on the
# four files, and the conflict plant without "object storage" given
# context that S5 does not share.
S=${SCRATCH:?set SCRATCH to the scratch directory}
N=$S/r175/next
T=$S/r175/plant
export CARGO_TARGET_DIR=$S/tp175 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 PLANT_TREE=$T
cd $T || exit 9
O=$N/scratch-tests.out
echo "# second pass, S4 as formatted:" >> $O
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
rustfmt --edition 2021 --check bins/skyzfsd/src/control/tests.rs crates/skyzfs-upload/src/uploader.rs crates/skyzfs-upload/src/publish/tests.rs crates/skyzfs-upload/tests/uploader.rs > $N/plantlogs/scratch-fmt2.log 2>&1; echo "rustfmt rc=$?" >> $O
python3 $N/plant.py $N/plants.json $O conflict-text-noprefix-src
git checkout -- bins/skyzfsd/src/control/tests.rs crates/skyzfs-upload/src/uploader.rs crates/skyzfs-upload/src/publish/tests.rs crates/skyzfs-upload/tests/uploader.rs
echo scratch2-done >> $O

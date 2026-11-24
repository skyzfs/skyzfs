#!/bin/bash
# The review's cut scenarios in the second worktree, appended for the run.
S=$1
W=$S/head2
F=$W/crates/skyzfs-local/src/inplace/tests/cuts.rs
cp $F $S/q7/cuts.rs.orig
cat $S/q7/scenarios_cuts.rs >> $F
cd $W
export CARGO_TARGET_DIR=$S/q7/target2 CARGO_INCREMENTAL=0
timeout 1800 cargo test -p skyzfs-local --features test-hooks --lib -- review_q7 --nocapture --test-threads 1 > $S/q7/scn_cuts.out 2>&1; echo "exit $?" >> $S/q7/scn_cuts.out
# The same with plant FA (no header's sync on a failed store).
python3 - "$W" <<'PY'
import sys
p = sys.argv[1] + "/crates/skyzfs-local/src/inplace.rs"
t = open(p).read()
old = "        let failed = self.failed;\n        self.begin_intent_sync();\n        failed || !self.failed\n"
assert t.count(old) == 1
open(p, "w").write(t.replace(old, "        let failed = self.failed;\n        if failed {\n            return true;\n        }\n        self.begin_intent_sync();\n        failed || !self.failed\n"))
PY
timeout 1800 cargo test -p skyzfs-local --features test-hooks --lib -- review_q7 --nocapture --test-threads 1 > $S/q7/scn_cuts_fa.out 2>&1; echo "exit $?" >> $S/q7/scn_cuts_fa.out
cd $W && git checkout -- crates/skyzfs-local/src/inplace.rs
cp $S/q7/cuts.rs.orig $F
git -C $W status --short > $S/q7/scn_cuts_tree.out
echo done >> $S/q7/scn_cuts_tree.out

#!/usr/bin/env bash
# look3.sh WORKTREE: #58 at 697ba46. The gates on what changed, then two
# plants in the request log: the `send` line written after the queue's
# wait, and after its flush. Each file is put back.
set -uo pipefail
cd "$1" || exit 2
export CARGO_INCREMENTAL=0
REC=bins/skyzfsd/src/record.rs
say() { grep -E '^test result|^test .* FAILED|^error' "$1" | sort | uniq -c | head -12; }
run() { echo "== $*"; "$@" > /tmp/look3.$$.out 2>&1; rc=$?; say /tmp/look3.$$.out; echo "exit $rc"; }
rustc --version
run cargo fmt --all --check
run cargo clippy -p skyzfs-local -p skyzfs-ublk -p skyzfsd --all-targets -- -D warnings
run cargo clippy -p skyzfs-local -p skyzfsd --all-targets --features skyzfs-local/test-hooks,skyzfsd/test-hooks -- -D warnings
run cargo test -p skyzfsd --features skyzfsd/test-hooks
run cargo test -p skyzfs-local --lib inplace
cp "$REC" /tmp/look3.$$.rs
python3 - "$REC" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
old = "        self.send();\n        let woken = self.inner.wait(fetched);\n"
assert s.count(old) == 1
open(p, 'w').write(s.replace(old, "        let woken = self.inner.wait(fetched);\n        self.send();\n"))
PY
echo "== plant: the send line after the wait"
cargo test -p skyzfsd --features skyzfsd/test-hooks --lib record > /tmp/look3.$$.out 2>&1; echo "exit $?"; say /tmp/look3.$$.out
cp /tmp/look3.$$.rs "$REC"
python3 - "$REC" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
old = "        self.send();\n        let flushed = self.inner.flush();\n"
assert s.count(old) == 1
open(p, 'w').write(s.replace(old, "        let flushed = self.inner.flush();\n        self.send();\n"))
PY
echo "== plant: the send line after the flush"
cargo test -p skyzfsd --features skyzfsd/test-hooks --lib record > /tmp/look3.$$.out 2>&1; echo "exit $?"; say /tmp/look3.$$.out
cp /tmp/look3.$$.rs "$REC"
rm -f /tmp/look3.$$.out /tmp/look3.$$.rs
git status --short

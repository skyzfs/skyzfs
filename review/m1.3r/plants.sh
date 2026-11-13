#!/usr/bin/env bash
# Plants in crates/skyzfs-local/src/inplace.rs, one at a time; each runs the crate's in-place tests.
# Usage: review/m1.3r/plants.sh <checkout of the head> (the checkout must be clean; it is restored after each plant)
cd "${1:?checkout}" || exit 1
HERE=$(cd "$(dirname "$0")" && pwd)
F=crates/skyzfs-local/src/inplace.rs
try() { # name old new
    git checkout -q -- crates
    if ! python3 "$HERE/plant.py" $F "$2" "$3"; then echo "== $1: PLANT DID NOT APPLY"; return; fi
    out=$(cargo test -p skyzfs-local --lib inplace 2>&1)
    echo "== $1: $(echo "$out" | grep -E '^test result|could not compile' | head -1)"
    echo "$out" | grep -E '^test .* FAILED|^error' | head -8 | sed 's/^/     /'
    git checkout -q -- crates
}
try "P3 a successful write is not counted as unsynced" "        self.unsynced = true;
        match done {
            Ok(()) => Ok(())," "        match done {
            Ok(()) => Ok(()),"
try "P4 a discard is not counted as a change" "                self.unsynced = true;
                self.complete(id, Ok(()), None)" "                self.complete(id, Ok(()), None)"
try "P5 no sync at the start" "        if store.home.sync().is_err() {" "        if false {"
try "P6 waiting flushes complete without their own sync" "                if self.unsynced {
                    self.begin_sync(waiting);" "                if false {
                    self.begin_sync(waiting);"
try "P8 a failed volume still reads" "        if self.failed {
            let buf = buffer_of(op);" "        if self.failed && !matches!(op, Op::Read { .. }) {
            let buf = buffer_of(op);"
try "P9 a failure leaves no marker" "        if !self.log.is_failed() {
            self.log.mark_failed();
        }" ""
try "P11 a flush on a failed disk after a write only: write errors ignored, sync still fails" "            Err(_) => {
                self.fail();
                self.settle_failed();
                Err(BlkError::Failed)
            }" "            Err(e) => Err(BlkError::Io(e.kind())),"
echo "== done"; git status --short | head -3

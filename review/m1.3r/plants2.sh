#!/usr/bin/env bash
# Second look (head d1641a4): plants in the in-place store and its harness, one at a time;
# each runs the crate's in-place tests.
# Usage: plants2.sh <checkout of the head> (the checkout must be clean; it is restored after each plant)
cd "${1:?checkout}" || exit 1
HERE=$(cd "$(dirname "$0")" && pwd)
F=crates/skyzfs-local/src/inplace.rs
H=crates/skyzfs-local/src/inplace/harness.rs
try() { # name file old new
    git checkout -q -- crates
    if ! python3 "$HERE/plant.py" "$2" "$3" "$4"; then echo "== $1: PLANT DID NOT APPLY"; return; fi
    out=$(cargo test -p skyzfs-local --lib inplace 2>&1)
    echo "== $1: $(echo "$out" | grep -E '^test result|could not compile' | head -1)"
    echo "$out" | grep -E '^test .* FAILED|^error' | head -8 | sed 's/^/     /'
    git checkout -q -- crates
}
try "P3 a successful write is not counted as unsynced" $F "        self.unsynced = true;
        match done {
            Ok(()) => Ok(())," "        match done {
            Ok(()) => Ok(()),"
try "P4 a discard is not counted as a change" $F "                if !self.mutants.has(Mutants::DISCARD_UNCHANGED) {
                    self.unsynced = true;
                }
" ""
try "P5 no sync at the start" $F "        if store.home.sync().is_err() {" "        if false {"
try "P6 waiting flushes complete without their own sync" $F "                if self.unsynced {
                    self.begin_sync(waiting);" "                if false {
                    self.begin_sync(waiting);"
try "P8 a failed volume still reads" $F "        if self.failed {
            let buf = buffer_of(op);" "        if self.failed && !matches!(op, Op::Read { .. }) {
            let buf = buffer_of(op);"
try "P9 a failure leaves no marker" $F "        if !self.log.is_failed() {
            self.log.mark_failed();
        }" ""
try "P11 write errors ignored, sync still fails" $F "            Err(_) => {
                self.fail();
                self.settle_failed();
                Err(BlkError::Failed)
            }" "            Err(e) => Err(BlkError::Io(e.kind())),"
try "P12 a failed volume still takes writes" $F "        if self.failed {
            let buf = buffer_of(op);" "        if self.failed && !matches!(op, Op::Write { .. }) {
            let buf = buffer_of(op);"
try "P13 wait with a zero timeout waits for the sync, as before" $F "        if self.ready.is_empty() && !timeout.is_zero() {" "        if self.ready.is_empty() || timeout.is_zero() {"
try "P14 wait never waits for the sync" $F "        if self.ready.is_empty() && !timeout.is_zero() {" "        if false {"
try "H1 fail_log is the halt again (the kinder fault)" $H "        target.disk.fail_write(0, true);
        target.disk.fail_sync(0, true);" "        target.disk.halt_after(0);"
try "H2 fail_log fails writes only" $H "        target.disk.fail_sync(0, true);
" ""
try "H3 fail_log fails without tearing" $H "        target.disk.fail_write(0, true);
        target.disk.fail_sync(0, true);" "        target.disk.fail_write(0, false);
        target.disk.fail_sync(0, false);"
echo "== done"; git status --short | head -3

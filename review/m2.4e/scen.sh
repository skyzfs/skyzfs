#!/bin/bash
# scen.sh TREE LIMIT_S "FILE ..." CARGO_ARGS...
# Appends the scenario files to the state's tests in TREE, runs cargo with
# the arguments, and restores the file on any exit. The tree must be clean
# before and is checked clean after.
set -u
tree=$1; limit=$2; files=$3; shift 3
here=$(cd "$(dirname "$0")" && pwd)
t=crates/skyzfs-local/src/inplace/state/tests.rs
cd "$tree" || exit 2
if [ -n "$(git status --short)" ]; then echo "tree not clean" >&2; exit 2; fi
cp "$t" "$here/.tests.rs.saved"
restore() { cp "$here/.tests.rs.saved" "$t"; rm -f "$here/.tests.rs.saved"; if [ -n "$(git status --short)" ]; then echo "TREE NOT CLEAN" >&2; git status --short >&2; fi; }
trap restore EXIT
for f in $files; do cat "$here/$f" >> "$t"; done
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$here/../t125} CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0
timeout -k 10 "$limit" cargo "$@"
echo "exit $?"

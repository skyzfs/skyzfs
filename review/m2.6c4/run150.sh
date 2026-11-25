#!/usr/bin/env bash
# Review of skyzfs/skyzfs#150: run the scratch tests against a clean
# worktree of the head, then restore it.
#   WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> bash run150.sh
# Appends scratch150.rs to bins/skyzfsd/src/kill/tests.rs and
# scratch150_upload.rs to bins/skyzfsd/src/upload/tests.rs, runs the r150_
# tests with the test hooks (each scratch test once), then the upload one
# REPEAT times from its binary, and restores both files whatever happens.
set -u
here=$(cd "$(dirname "$0")" && pwd)
wt=${WORKTREE:?WORKTREE}
repeat=${REPEAT:-2000}
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
kt="$wt/bins/skyzfsd/src/kill/tests.rs"
ut="$wt/bins/skyzfsd/src/upload/tests.rs"
cp "$kt" "$kt.orig" && cp "$ut" "$ut.orig"
restore() { mv -f "$kt.orig" "$kt"; mv -f "$ut.orig" "$ut"; }
trap restore EXIT
cat "$here/scratch150.rs" >> "$kt"
cat "$here/scratch150_upload.rs" >> "$ut"
cd "$wt" || exit 2
timeout 1200 cargo test -p skyzfsd --features test-hooks -- r150_ --nocapture --test-threads 1 2>&1
echo "cargo test exit $?"
bin=$(cargo test -p skyzfsd --features test-hooks --no-run --message-format json 2>/dev/null \
  | python3 -I -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["kind"]==["lib"]: print(m["executable"])')
for t in r150_the_carry_hook_skips_an_ask_whose_asker_went_before_it_was_sent \
         the_carry_hook_passes_the_channel_on_and_counts_the_publishes_accepted; do
  fail=0
  for _ in $(seq 1 "$repeat"); do
    "$bin" --exact "upload::tests::$t" -q >/dev/null 2>&1 || fail=$((fail+1))
  done
  echo "repeat upload::tests::$t: $repeat runs, $fail failed"
done

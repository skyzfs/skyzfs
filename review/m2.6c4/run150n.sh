#!/usr/bin/env bash
# Next look at skyzfs/skyzfs#150 (head d629834): run the scratch tests
# against a clean worktree of the head, then restore it, and run the
# pull request's B1 test REPEAT times from its binary.
#   WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> REPEAT=5000 bash run150n.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
wt=${WORKTREE:?WORKTREE}
repeat=${REPEAT:-5000}
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
kt="$wt/bins/skyzfsd/src/kill/tests.rs"
cp "$kt" "$kt.orig"
restore() { mv -f "$kt.orig" "$kt"; }
trap restore EXIT
cat "$here/scratch150n.rs" >> "$kt"
cd "$wt" || exit 2
timeout 1200 cargo test -p skyzfsd --features test-hooks -- r150n_ --nocapture --test-threads 1 2>&1
echo "cargo test exit $?"
restore
trap - EXIT
bin=$(cargo test -p skyzfsd --features test-hooks --no-run --message-format json 2>/dev/null \
  | python3 -I -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["kind"]==["lib"]: print(m["executable"])')
t=upload::tests::the_carry_hook_passes_the_channel_on_and_counts_the_publishes_accepted
fail=0
for _ in $(seq 1 "$repeat"); do
  "$bin" --exact "$t" -q >/dev/null 2>&1 || fail=$((fail+1))
done
echo "repeat $t: $repeat runs, $fail failed"

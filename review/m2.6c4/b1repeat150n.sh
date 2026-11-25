#!/usr/bin/env bash
# Next look at skyzfs/skyzfs#150 (head d629834): build the test-hooks
# test binary of a clean worktree of the head, copy it aside (so that
# other builds in the same target do not replace it), and run B1's test
# REPEAT times, one process each.
#   WORKTREE=<clean worktree of the head> CARGO_TARGET_DIR=<scratch> REPEAT=6000 bash b1repeat150n.sh
set -u
wt=${WORKTREE:?WORKTREE}
repeat=${REPEAT:-6000}
export CARGO_PROFILE_DEV_DEBUG=0 CARGO_INCREMENTAL=0
cd "$wt" || exit 2
bin=$(cargo test -p skyzfsd --features test-hooks --no-run --message-format json 2>/dev/null \
  | python3 -I -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["kind"]==["lib"]: print(m["executable"])')
copy="$CARGO_TARGET_DIR/b1-test-bin"
cp "$bin" "$copy" || exit 2
t=upload::tests::the_carry_hook_passes_the_channel_on_and_counts_the_publishes_accepted
"$copy" --exact "$t" 2>&1 | tail -n 3
fail=0
for _ in $(seq 1 "$repeat"); do
  "$copy" --exact "$t" -q >/dev/null 2>&1 || fail=$((fail+1))
done
echo "repeat $t: $repeat runs, $fail failed"

# How long pidle_run takes, run alone: about the chance that a run crosses a
# second of the wall clock, which is what makes the test flaky (B2).
# Usage: bash pidle_time.sh WORKTREE
WT=$1
SCRIPT=$WT/tests/kernel/cut-record.sh
T=$(mktemp -d)
pidle_fn=$(sed -n '/^pidle() {$/,/^}$/p' "$SCRIPT")
eval "$(sed -n '/^pidle_run() {/,/^}$/p' "$WT/tests/kernel/test_cut_record.sh")"
for k in 1 2 3 4 5; do
  s=$(date +%s%N); pidle_run 120 >/dev/null; m=$(date +%s%N); pidle_run 30 grow >/dev/null; e=$(date +%s%N)
  echo "idle run $(( (m-s)/1000000 )) ms, grow run $(( (e-m)/1000000 )) ms"
done
rm -r "$T"

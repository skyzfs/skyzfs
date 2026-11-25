#!/usr/bin/env bash
# Third look at #145 at 58fc660: the two pidle lines of test_cut_record.sh
# run N times each, with pidle_body and pidle_run copied from the test as
# it stands in WORKTREE (from `pidle_body='` to the end of pidle_run), and
# pidle taken from WORKTREE's cut-record.sh, as the test takes it.
# Prints how many runs answered each way, and the slowest run in ms.
# Usage: bash pidle_head.sh WORKTREE [N] [TAG]
set -u
WT=$1 N=${2:-100} TAG=${3:-}
SCRIPT=$WT/tests/kernel/cut-record.sh
TEST=$WT/tests/kernel/test_cut_record.sh
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
pidle_fn=$(sed -n '/^pidle() {$/,/^}$/p' "$SCRIPT")
eval "$(awk '/^pidle_body=/{on=1} on{print} on && /^pidle_run\(\) \{/{f=1} f && /^}$/{exit}' "$TEST")"
declare -A got
slow=0
for _ in $(seq "$N"); do
    s=$(date +%s%N); a=$(pidle_run 120); m=$(date +%s%N); b=$(pidle_run 30 grow); e=$(date +%s%N)
    ((slow = (m - s) > slow ? (m - s) : slow)); ((slow = (e - m) > slow ? (e - m) : slow))
    got["idle: $a"]=$(( ${got["idle: $a"]:-0} + 1 ))
    got["grow: $b"]=$(( ${got["grow: $b"]:-0} + 1 ))
done
echo "${TAG:+$TAG: }bash $BASH_VERSION, $N runs of each (want 'idle after 12 s, 12 sleeps' and 'error after 30 s, 30 sleeps'), slowest run $((slow / 1000000)) ms:"
for k in "${!got[@]}"; do echo "  ${got[$k]}  $k"; done | sort -k2

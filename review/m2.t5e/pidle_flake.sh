#!/usr/bin/env bash
# Next look at #145 at 9bac3dd: how often test_cut_record.sh's two new
# pidle lines (G3, plants S2 and S3) give another answer than they want,
# run alone, N times each. pidle_run is copied from the test as it stands
# at 9bac3dd; the stand-in sleep moves bash's SECONDS on by assignment,
# and bash's SECONDS also moves on with the wall clock's second.
# Usage: bash pidle_flake.sh WORKTREE [N]
set -u
WT=$1 N=${2:-50}
SCRIPT=$WT/tests/kernel/cut-record.sh
T=$(mktemp -d)
trap 'rm -rf "$T"' EXIT
pidle_fn=$(sed -n '/^pidle() {$/,/^}$/p' "$SCRIPT")
eval "$(sed -n '/^pidle_run() {/,/^}$/p' "$WT/tests/kernel/test_cut_record.sh")"
declare -A got
for k in $(seq "$N"); do
    a=$(pidle_run 120); b=$(pidle_run 30 grow)
    got["idle: $a"]=$(( ${got["idle: $a"]:-0} + 1 ))
    got["grow: $b"]=$(( ${got["grow: $b"]:-0} + 1 ))
done
echo "bash $BASH_VERSION, $N runs of each (want 'idle after 12 s, 12 sleeps' and 'error after 30 s, 30 sleeps'):"
for k in "${!got[@]}"; do echo "  ${got[$k]}  $k"; done | sort -k2

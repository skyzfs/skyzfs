# Usage: bash pidle_unset.sh WORKTREE [N]
# The flake counter with the stand-in clock made plain: `unset SECONDS`
# in pidle_run's subshell (bash: an unset SECONDS loses its special
# property), so only the stand-in sleep moves it. A sketch, run on a copy.
WT=$1 N=${2:-200}
SCRIPT=$WT/tests/kernel/cut-record.sh
T=$(mktemp -d)
pidle_fn=$(sed -n '/^pidle() {$/,/^}$/p' "$SCRIPT")
eval "$(sed -n '/^pidle_run() {/,/^}$/p' "$WT/tests/kernel/test_cut_record.sh" | sed 's/^        SECONDS=0$/        unset SECONDS; SECONDS=0/')"
declare -f pidle_run | grep -c 'unset SECONDS'
declare -A got
for k in $(seq "$N"); do
    a=$(pidle_run 120); b=$(pidle_run 30 grow)
    got["idle: $a"]=$(( ${got["idle: $a"]:-0} + 1 ))
    got["grow: $b"]=$(( ${got["grow: $b"]:-0} + 1 ))
done
echo "bash $BASH_VERSION, $N runs of each with SECONDS unset in the stand-in's subshell:"
for k in "${!got[@]}"; do echo "  ${got[$k]}  $k"; done | sort -k2
rm -r "$T"

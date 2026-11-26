#!/bin/bash
# The suites' EXIT trap run by a background child killed before it
# execs (bash 5.2). test_agent_record.sh sets trap 'rm -rf "$T"' EXIT,
# starts stand-ins with `sleep 30 &` or `bash -c '...' &` and kills
# some of them at once (or 0.2 s later); a child that the kill reaches
# before its exec runs the trap, and $T is gone for the tests after it.
# Here the trap prints which process ran it.
# usage: bash trap_race.sh [TRIES]
n=${1:-20} hit=0
for _ in $(seq 1 "$n"); do
    out=$(bash -c 'trap "echo trap-ran-in \$BASHPID" EXIT; sleep 30 & p=$!; kill $p; wait $p; echo "child $p"' 2>&1)
    c=$(sed -n 's/^child //p' <<<"$out")
    if grep -qx "trap-ran-in $c" <<<"$out"; then hit=$((hit + 1)); fi
done
echo "bash $BASH_VERSION: the parent's EXIT trap ran in the killed child in $hit of $n tries"

#!/bin/bash
# Run skz_hang_evidence from the script against this machine's /proc, with a stand-in status
# and the sysrq write sent to a file. t24-ev.sh <checkout> <dir>
set -uo pipefail
co=$1; tt=$2
script=$co/tests/bench/topology-bench.sh
for f in skz_hang_evidence; do
    awk -v f="$f" '$0 ~ "^" f "\\(\\) \\{" { on = 1 } on { print } on && /^}/ { exit }' "$script"
done > "$tt/funcs.sh"
. "$co/tests/lib/lim.sh"
OUT=$tt/out; mkdir -p "$OUT"; SKZ_CGROUP=""; SKZ_SYSRQ=$tt/sysrq; : > "$SKZ_SYSRQ"
log() { echo "log: $*"; }
skz_status() { echo "writes 7"; }
sleep 300 & SKZ_PID=$!
. "$tt/funcs.sh"
s=$(date +%s%N)
skz_hang_evidence "$OUT/skyzfsd-hang" "a look taken by hand"
echo "took $(( ($(date +%s%N) - s) / 1000000 )) ms; tasks on the machine: $(ls -d /proc/[0-9]*/task/[0-9]* | wc -l)"
kill "$SKZ_PID"

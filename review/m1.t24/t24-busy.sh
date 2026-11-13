#!/bin/bash
# Run test_copy_limit.sh N times while another process keeps writing to the disk.
# t24-busy.sh <checkout> <scratch dir> <runs>
set -u
co=$1; scratch=$2; runs=$3
w=$(mktemp -d "$scratch/dirty.XXXXXX")
: > "$w/go"
( while [ -e "$w/go" ]; do dd if=/dev/zero of="$w/f" bs=1M count=300 status=none 2>/dev/null; done ) &
bg=$!
sleep 1
cd "$co" || exit 1
for i in $(seq 1 "$runs"); do
    s=$(date +%s)
    out=$(bash tests/bench/test_copy_limit.sh 2>/dev/null); rc=$?
    echo "run $i: exit $rc in $(( $(date +%s) - s )) s; $(echo "$out" | grep -c '^FAIL') FAIL: $(echo "$out" | grep '^FAIL' | head -3 | cut -c1-70 | tr '\n' ';')"
done
rm -f "$w/go"
wait "$bg" 2>/dev/null
rm -rf "${w:?}"
t=$(date +%s%N); sync; echo "a sync afterwards: $(( ($(date +%s%N) - t) / 1000000 )) ms"

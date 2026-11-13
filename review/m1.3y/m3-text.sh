#!/usr/bin/env bash
# m3-text.sh BINDIR OUT RUNS: the whole of skyzfs-local's unit tests
# with the plant m3 (a read does not wait for a write it overlaps), up
# to RUNS times, until one fails; keeps that run's whole text in OUT.
set -uo pipefail
BIN=$1; OUT=$2; RUNS=$3
for n in $(seq 1 "$RUNS"); do
    timeout 900 "$BIN/planted.bin" > "$OUT.run" 2>&1; rc=$?
    echo "planted, run $n: rc=$rc; $(grep '^test result' "$OUT.run" | cut -c1-90)"
    if [ "$rc" != 0 ]; then mv "$OUT.run" "$OUT"; echo "kept in $OUT"; exit 0; fi
done
rm -f "$OUT.run"

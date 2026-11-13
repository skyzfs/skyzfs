#!/usr/bin/env bash
# m3-load.sh BINDIR OUTDIR ROUNDS: the large-page conformance suite of
# the in-place store alone, four copies side by side (as much load as
# the whole test binary makes on four processors), ROUNDS times with
# the plant m3 (a read does not wait for a write it overlaps) and
# ROUNDS times without. Keeps the whole text of every failure.
set -uo pipefail
BIN=$1; OUT=$2; ROUNDS=$3
T=inplace::tests::the_in_place_store_passes_the_conformance_suite_on_large_pages
mkdir -p "$OUT" || exit 2
for n in $(seq 1 "$ROUNDS"); do
    for which in planted plain; do
        pids=()
        for k in 1 2 3 4; do
            timeout 900 "$BIN/$which.bin" --exact "$T" > "$OUT/$which.$n.$k.txt" 2>&1 &
            pids+=($!)
        done
        k=0
        for pid in "${pids[@]}"; do
            k=$((k + 1)); wait "$pid"; rc=$?
            echo "$which, round $n, copy $k: rc=$rc; $(grep '^test result' "$OUT/$which.$n.$k.txt" | cut -c1-100)"
            [ "$rc" = 0 ] && rm -f "$OUT/$which.$n.$k.txt"
        done
    done
done

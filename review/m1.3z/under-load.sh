#!/usr/bin/env bash
# under-load.sh WORKTREE BASE RUNS: for #57 (M1.3z). Builds the store's
# unit tests twice, with the test file at BASE (the old tests) and as the
# worktree has it (the new ones), then runs each of the two changed tests
# RUNS times while every CPU is kept busy four times over, and counts the
# failures. No delay is planted: this shows whether load alone brings the
# race out of the old test, and whether the new one stands it.
set -uo pipefail
TREE=$1; BASE=$2; RUNS=$3
TESTS=crates/skyzfs-local/src/store/tests.rs
OUT=$(mktemp -d) || exit 2
[ -n "$OUT" ] && [ -d "$OUT" ] || exit 2
cd "$TREE" || exit 2
cp "$TESTS" "$OUT/tests.new"
build() { # -> the test binary's path
    cargo test -p skyzfs-local --lib --no-run 2>&1 | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' | tail -n 1
}
git show "$BASE:$TESTS" > "$TESTS"
old=$(build); cp "$old" "$OUT/old.bin"
cp "$OUT/tests.new" "$TESTS"
new=$(build); cp "$new" "$OUT/new.bin"
[ -s "$OUT/old.bin" ] && [ -s "$OUT/new.bin" ] || { echo "a build failed"; exit 2; }
n=$(nproc); pids=()
for _ in $(seq 1 $((4 * n))); do ( while :; do :; done ) & pids+=("$!"); done
trap 'kill "${pids[@]}" 2>/dev/null; rm -rf "${OUT:?}"' EXIT
echo "$n CPUs, $((4 * n)) busy loops, $RUNS runs of each"
for t in every_kind_of_request_is_timed_under_its_kind a_steps_wait_at_a_barrier_is_counted_as_its_own_activity; do
    for which in old new; do
        fails=0
        for _ in $(seq 1 "$RUNS"); do
            "$OUT/$which.bin" --exact "store::tests::$t" > "$OUT/run.txt" 2>&1 || { fails=$((fails + 1)); grep -m1 -o 'barrier_home: Spent { count: [0-9]*, sum_ns: [0-9]*, max_ns: [0-9]*\|reader: \[[0-9, ]*\]' "$OUT/run.txt" | head -n 1 >> "$OUT/$which-$t.why"; }
        done
        echo "$t, $which: $fails of $RUNS failed$( [ -s "$OUT/$which-$t.why" ] && echo "; first: $(head -n 1 "$OUT/$which-$t.why")" )"
    done
done

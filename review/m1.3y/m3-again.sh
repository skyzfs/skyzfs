#!/usr/bin/env bash
# m3-again.sh WORKTREE RUNS: is the large-page suite's failure under the
# plant m3 (a read does not wait for a write it overlaps) the plant's, or
# the test's own? Builds the in-place tests unplanted and planted, runs
# the one test RUNS times with each, and keeps the first failure's text.
set -uo pipefail
TREE=$1; RUNS=$2
SRC=crates/skyzfs-local/src/inplace.rs
T=inplace::tests::the_in_place_store_passes_the_conformance_suite_on_large_pages
OUT=$(mktemp -d) || exit 2
[ -n "$OUT" ] && [ -d "$OUT" ] || exit 2
cd "$TREE" || exit 2
build() { cargo test -p skyzfs-local --lib --no-run 2>&1 | sed -n 's/.*Executable unittests src\/lib.rs (\(.*\))/\1/p' | tail -n 1; }
plain=$(build); cp "$plain" "$OUT/plain.bin"
cp "$SRC" "$OUT/inplace.rs"
python3 - "$SRC" <<'PY'
import sys
p = sys.argv[1]; s = open(p).read()
old = '.any(|&(from, to, busy_write)| (write || busy_write) && from < end && offset < to)'
assert s.count(old) == 1
open(p, 'w').write(s.replace(old, old.replace('(write || busy_write)', 'write')))
PY
planted=$(build); cp "$planted" "$OUT/planted.bin"
cp "$OUT/inplace.rs" "$SRC"
[ -s "$OUT/plain.bin" ] && [ -s "$OUT/planted.bin" ] || { echo "a build failed"; exit 2; }
for which in plain planted; do
    fails=0
    for n in $(seq 1 "$RUNS"); do
        if ! timeout 600 "$OUT/$which.bin" --exact "$T" > "$OUT/run.txt" 2>&1; then
            fails=$((fails + 1))
            [ -s "$OUT/$which.why" ] || grep -v '^$' "$OUT/run.txt" | grep -A 12 'panicked at' | cut -c1-700 > "$OUT/$which.why"
        fi
    done
    echo "$which: $fails of $RUNS failed"
    [ -s "$OUT/$which.why" ] && { echo "  the first failure:"; sed 's/^/    /' "$OUT/$which.why"; }
done
git -C "$TREE" status --short
rm -rf "${OUT:?}"

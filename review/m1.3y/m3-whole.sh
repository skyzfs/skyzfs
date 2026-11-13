#!/usr/bin/env bash
# m3-whole.sh WORKTREE RUNS: the whole of skyzfs-local's unit tests,
# RUNS times unplanted and RUNS times with the plant m3 (a read does
# not wait for a write it overlaps), all tests side by side as cargo
# runs them. Keeps the text of every failure.
set -uo pipefail
TREE=$1; RUNS=$2
SRC=crates/skyzfs-local/src/inplace.rs
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
for n in $(seq 1 "$RUNS"); do
    for which in planted plain; do
        timeout 900 "$OUT/$which.bin" > "$OUT/run.txt" 2>&1; rc=$?
        echo "$which, run $n: rc=$rc; $(grep '^test result' "$OUT/run.txt" | cut -c1-90)"
        if [ "$rc" != 0 ]; then
            grep -E '^test .* FAILED|^---- ' "$OUT/run.txt" | cut -c1-200
            grep -A 14 'panicked at' "$OUT/run.txt" | cut -c1-900 | head -60
        fi
    done
done
git -C "$TREE" status --short
rm -rf "${OUT:?}"

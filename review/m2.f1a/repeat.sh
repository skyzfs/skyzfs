#!/bin/bash
# Build skyzfsd's lib tests in WORKTREE, then run one test N times, alone
# (--exact), optionally beside a CPU load of LOAD busy loops; print how
# many runs failed and the first failure's message.
# Usage: repeat.sh WORKTREE TARGET TEST N LOAD
set -u
WT=$1; export CARGO_TARGET_DIR=$2; TEST=$3; N=$4; LOAD=${5:-0}
export CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0
cd "$WT" || exit 2
BIN=$(cargo test -p skyzfsd --lib --no-run --message-format=json 2>/dev/null \
      | python3 -I -c 'import sys,json
for l in sys.stdin:
    try: m=json.loads(l)
    except Exception: continue
    if m.get("reason")=="compiler-artifact" and m.get("executable") and m["target"]["kind"]==["lib"]: print(m["executable"])' | tail -1)
[ -x "$BIN" ] || { echo "no test binary"; exit 2; }
pids=()
for _ in $(seq 1 "$LOAD"); do ( while :; do :; done ) & pids+=($!); done
fails=0; first=""
for i in $(seq 1 "$N"); do
    out=$("$BIN" "$TEST" --exact 2>&1)
    if ! grep -q "test result: ok. 1 passed" <<< "$out"; then
        fails=$((fails+1))
        [ -n "$first" ] || first=$(grep -m1 -A3 "panicked at" <<< "$out" | tr '\n' ' ')
    fi
done
for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
echo "$TEST: $fails of $N runs failed (load $LOAD busy loops, $(nproc) cores)${first:+; first: $first}"

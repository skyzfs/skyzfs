#!/usr/bin/env bash
# Review scratch (Q10, PR #148, next look): probes of main's background
# record + catch + bare `wait` (zfs-cli-record.sh at HEAD, sourced with
# ZCR_LIB=1), with fake commands and no kernel.
#
#   bash review/q10/wait_probe.sh HEAD WORKDIR
#
# W1: lim gives up on a child it cannot kill (a command in the kernel's
#     wait: here `kill` is made a no-op, so SIGKILL has no effect) and
#     leaves it as a job of the calling shell. main's bare `wait` then
#     waits for that abandoned child, not only for the background record.
# W2: the same with `wait "$!"`: only the background record is waited for.
# W3: a background record that ends before catch starts: catch keeps the
#     state after, with its "not caught" note (the D4 path stays flagged).
# W4: the index's order: the background record's line is written when it
#     ends, after the catch's records.
set -uo pipefail
HEAD=${1:?head}
W=${2:?workdir}
REPO=$(git rev-parse --show-toplevel)
rm -rf "$W/t" && mkdir -p "$W/t/tests/kernel" "$W/t/tests/lib"
for f in tests/kernel/zfs-cli-record.sh tests/lib/lim.sh tests/lib/gnu-dd.sh; do
    git -C "$REPO" show "$HEAD:$f" > "$W/t/$f"
done

probe() { # name body
    local name=$1 body=$2
    rm -rf "$W/$name" && mkdir -p "$W/$name"
    ZCR_LIB=1 OUTDIR=$W/$name bash -c '
        set -uo pipefail
        ZCR_LIB=1
        . "$0"
        OUT=$OUTDIR LUA_DIR=$OUTDIR
        : > "$OUT/index.tsv"
        '"$body" "$W/t/tests/kernel/zfs-cli-record.sh" 2>&1 | sed "s/^/$name: /"
}

probe W1 '
LIMIT=5
rec bg sleep 2 &                # the background record: 2 s, within its limit
LIMIT=1
kill() { :; }                   # SIGKILL has no effect: a child in the kernel
hang() { sleep 6; }             # a status that does not return for 6 s
t0=$SECONDS
catch c "in progress" 3 hang > /dev/null
t1=$SECONDS
wait
t2=$SECONDS
echo "catch returned after $((t1 - t0)) s (note: $(cat "$OUT/c/note")); bare wait returned $((t2 - t1)) s later; bg record rc $(cat "$OUT/bg/rc")"
'

probe W2 '
LIMIT=5
rec bg sleep 2 &
bg=$!
LIMIT=1
kill() { :; }
hang() { sleep 6; }
t0=$SECONDS
catch c "in progress" 3 hang > /dev/null
t1=$SECONDS
wait "$bg"
t2=$SECONDS
echo "catch returned after $((t1 - t0)) s; wait \"\$!\" returned $((t2 - t1)) s later; bg record rc $(cat "$OUT/bg/rc")"
'

probe W3 '
LIMIT=5
rec bg true > /dev/null &
sleep 1                          # the background command ends first
idle() { echo "scan: resilvered 1G in 00:00:01"; }
catch c "resilver in progress" 3 idle > /dev/null
echo "catch rc $?; note: $(cat "$OUT/c/note")"
wait
'

probe W4 '
LIMIT=5
rec bg sleep 1 > /dev/null &
busy() { echo "resilver in progress"; }
catch c-P "resilver in progress" 3 busy > /dev/null
catch c-g "resilver in progress" 3 busy > /dev/null
wait
echo "index order: $(cut -f1 "$OUT/index.tsv" | tr "\n" " ")"
'

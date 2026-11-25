#!/usr/bin/env bash
# Review scratch (Q10, PR #148): what rec and catch keep when a command
# does not do what the recorder expected. Fake commands, no kernel.
#
#   bash review/q10/catch_probe.sh HEAD WORKDIR
set -uo pipefail
HEAD=${1:?head}
W=${2:?workdir}
REPO=$(git rev-parse --show-toplevel)
rm -rf "$W/p" && mkdir -p "$W/p/tests/kernel" "$W/p/tests/lib" "$W/p/out" "$W/p/lua"
for f in tests/kernel/zfs-cli-record.sh tests/lib/lim.sh tests/lib/gnu-dd.sh; do
    git -C "$REPO" show "$HEAD:$f" > "$W/p/$f"
done
ZCR_LIB=1
# shellcheck source=/dev/null
. "$W/p/tests/kernel/zfs-cli-record.sh"
OUT=$W/p/out LUA_DIR=$W/p/lua LIMIT=1
: > "$OUT/index.tsv"
show() { # name
    printf -- '-- %s: rc=%s note=%s\n   stdout=%q stderr=%q\n' "$1" "$(cat "$OUT/$1/rc" 2>/dev/null || echo NONE)" \
        "$(cat "$OUT/$1/note" 2>/dev/null || echo NONE)" "$(cat "$OUT/$1/stdout" 2>/dev/null)" "$(cat "$OUT/$1/stderr" 2>/dev/null)"
}

# P1: the expected failure does not come: a second `zfs hold` that
# succeeds (rec has no expectation to compare with).
hold_ok() { return 0; }
rec x-hold-2 hold_ok > /dev/null
show x-hold-2

# P2: catch on a status that fails every time.
fails() { echo "cannot open 'skzq': no such pool" >&2; return 1; }
catch x-fails 'scrub in progress' 3 fails > /dev/null; echo "   catch returned $?"
show x-fails

# P3: catch whose last try fails although an earlier try succeeded:
# the last is kept, with its rc.
n=0
flap() { local c; c=$(($(cat "$W/p/n" 2>/dev/null || echo 0) + 1)); echo "$c" > "$W/p/n"; echo "try $c"; [ "$c" -lt 3 ] || { echo "gone" >&2; return 1; }; }
catch x-flap 'never' 3 flap > /dev/null; echo "   catch returned $?"
show x-flap

# P4: the state shows on stdout of a command that exits non-zero: caught,
# with its rc.
both() { echo "scan: scrub in progress"; return 2; }
catch x-both 'scrub in progress' 3 both > /dev/null; echo "   catch returned $?"
show x-both

# P5: a status that never ends: each try costs LIMIT; tries x LIMIT in all.
hang() { sleep 30; }
t0=$(date +%s)
catch x-hang 'scrub in progress' 3 hang > /dev/null; r=$?
echo "   catch returned $r after $(( $(date +%s) - t0 )) s for 3 tries at LIMIT=$LIMIT"
show x-hang

# P6: catch under a name already used: keep refuses (70), catch's own
# return is still 0 when caught, the output stays in OUT/.catch.
ok() { echo "scrub in progress"; }
catch x-fails 'scrub in progress' 3 ok > /dev/null 2> "$W/p/p6.err"; echo "   catch returned $? (keep said: $(cat "$W/p/p6.err"))"
show x-fails
echo "   OUT/.catch left: $([ -d "$OUT/.catch" ] && ls "$OUT/.catch" | tr '\n' ' ' || echo no)"

# P7: what the job summary shows (the workflow's sed): the note column
# only for catch; for rec, rc alone.
echo "-- the summary's rows:"
cut -f1-3 "$OUT/index.tsv" | sed 's/|/\//g; s/\t/ | /g; s/^/| /; s/$/ |/'

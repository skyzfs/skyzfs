#!/bin/bash
# Tests of cut-record.sh's handling of the holder's end, with stand-in
# holders: no kernel, ZFS or daemon. The recording runs under set -e, so a
# holder's status that is not taken with || ends the recording before the
# check, as the rig's error (the review of #113, D1).
# Run: bash tests/kernel/test_cut_record.sh
HERE=$(cd "$(dirname "$0")" && pwd)
SCRIPT=$HERE/cut-record.sh
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
trap 'rm -rf "$T"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}

# The function as the script has it.
fn=$(sed -n '/^holder_ended() {$/,/^}$/p' "$SCRIPT")
is "holder_ended is in the script" "$([ -n "$fn" ] && echo yes)" yes

# One recording's end: a stand-in holder that writes LOG and ends with
# STATUS, then holder_ended under set -e as the recording calls it.
# -> "went on" when the recording reaches the check, else what ended it.
recording() { # status log
    # The names are the function's, which eval brings in.
    # shellcheck disable=SC2034,SC2329
    (
        set -euo pipefail
        RDIR=$T/r; NAME=rec; mkdir -p "$RDIR"; printf '%b' "$2" > "$RDIR/holder.log"
        log() { :; }
        rig_error() { echo "rig error: $*" > "$T/why"; exit 2; }
        eval "$fn"
        bash -c "exit $1" & HOLD_PID=$!
        holder_ended "at the stop"
        echo "went on" > "$T/why"
    )
    echo "$? $(cat "$T/why" 2>/dev/null)"
    rm -f "$T/why"
}

is "a holder that ends with 0" "$(recording 0 '1\tdrained: 2 cuts written\n')" "0 went on"
is "a holder that ends with its verdict goes on to the check" \
    "$(recording 1 '1\tVERDICT cut 1 is still being uploaded after its write was refused\n')" "0 went on"
is "1 without a VERDICT line is the rig's error" "$(recording 1 '1\tleft\n')" \
    "2 rig error: the holder ended with 1 at the stop: 1	left"
is "the holder's own error is the rig's error" "$(recording 2 '1\tERROR HolderError: x\n')" \
    "2 rig error: the holder ended with 2 at the stop: 1	ERROR HolderError: x"

# Every wait for the holder takes its status with ||, outside cleanup
# (which runs with set +e): a bare one ends the recording at its status.
bare=$(awk '/^cleanup\(\) \{$/ { c = 1 } c && /^}$/ { c = 0; next } !c && /wait "\$HOLD_PID"/ && !/wait "\$HOLD_PID" 2>\/dev\/null \|\|/ { print NR": "$0 }' "$SCRIPT")
is "no bare wait for the holder" "$bare" ""
n=$(grep -c 'holder_ended "' "$SCRIPT")
is "the holder's leave and its stop both end through holder_ended" "$n" 2

[ "$fails" = 0 ] && echo "all passed"
exit $((fails > 0))

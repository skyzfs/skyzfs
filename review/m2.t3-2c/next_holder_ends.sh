#!/bin/bash
# #113 at ae36c2f, the next look: cut-record.sh's functions around the
# holder, taken from the script as they are and run as a recording runs
# them (a subshell under set -euo pipefail), with a stand-in holder that
# has ended with 1 and a VERDICT line in its log.
#   1. end_daemon and holder_leave: does the recording go on to the check?
#   2. holder_wrote_one and hold_idle, which wait for a sign of a holder
#      that is alive: what do they say of a holder that ended that way?
# `sleep` and `z` are replaced, so that the waits end at once.
#
#   next_holder_ends.sh CHECKOUT/tests/kernel/cut-record.sh
src=$1
work=$(mktemp -d)
take() { sed -n "/^$1() {\$/,/^}\$/p" "$src"; }
verdict='1\tVERDICT cut 1 is still being uploaded after its write was refused (refused: read block 7 of cut 1: Input/output error (os error 5))\n'
run() { # function, the holder's status, the holder's log
    (
        set -euo pipefail
        eval "$(take holder_ended)"; eval "$(take holder_leave)"; eval "$(take end_daemon)"
        eval "$(take holder_wrote_one)"; eval "$(take hold_idle)"
        log() { :; }
        sleep() { :; }
        z() { :; }
        rig_error() { echo "    rig_error: $*" | cut -c1-230; exit 2; }
        # end_daemon's status read, the first thing it does after the
        # holder: reaching it is going on to the check
        timeout() { : > "$WORK/went-on"; exit 0; }
        WORK=$work; RDIR=$work; NAME=rec; POOL=p; SKYZFSD=true; VOL=$work/vol; RUN=$work/run; SERVE_PID=""
        printf '%b' "$3" > "$RDIR/holder.log"
        (exit "$2") &
        HOLD_PID=$!
        "$1"
        echo "    $1 returned $?"
    )
    echo "$1, the holder ended with $2: the recording's subshell ended with $?$([ ! -e "$work/went-on" ] || echo ', having gone on to the status read and the check')"
    for f in holder.log hold.stop hold.leave hold.pause hold.pause.held held.img held.img.blocks went-on status.txt status-time.tsv; do
        [ ! -e "$work/$f" ] || unlink "$work/$f"
    done
}
echo "1. where a holder's end is taken"
run end_daemon 1 "$verdict"
run holder_leave 1 "$verdict"
run end_daemon 1 '1\tleft\n'
run end_daemon 2 '1\tERROR HolderError: x\n'
echo "2. where the recording waits for a sign of the holder"
run holder_wrote_one 1 "$verdict"
run hold_idle 1 "$verdict"
rmdir "$work"

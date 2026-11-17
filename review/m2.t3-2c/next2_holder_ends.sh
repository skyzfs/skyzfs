#!/bin/bash
# #113 at ab34565: where a recording waits for a sign of its holder, with
# a holder that has ended. cut-record.sh's functions are taken from the
# script as they are and run as a recording runs them: in a subshell
# under set -euo pipefail, the recording's function called plainly (so
# with errexit), and in it the line the script has,
#     holder_wrote_one || { export_pool; return 0; }
# then holder_leave, then end_daemon as `end` calls it. `sleep`, `z` and
# the daemon's commands are replaced, so that the waits end in seconds.
#
#   next2_holder_ends.sh CHECKOUT/tests/kernel/cut-record.sh
src=$1
work=$(mktemp -d)
take() { sed -n "/^$1() {\$/,/^}\$/p" "$src"; }
verdict='1\tVERDICT cut 1 is still being uploaded after its write was refused (refused: read block 7 of cut 1: Input/output error (os error 5))\n'
caller=$(grep -c '^    holder_wrote_one || { export_pool; return 0; }$' "$src")
echo "the line 'holder_wrote_one || { export_pool; return 0; }' is in the script $caller times; hold_idle is called $(grep -c '^    hold_idle$' "$src") time(s)"
run() { # title, the wait, the holder's status ('alive' for one that stays), the holder's log, held.tsv's text
    (
        set -euo pipefail
        eval "$(take holder_ended)"; eval "$(take holder_leave)"; eval "$(take end_daemon)"
        eval "$(take holder_wrote_one)"; eval "$(take hold_idle)"
        log() { echo "    log: $*" | cut -c1-200; }
        sleep() { command sleep 0.01; }
        z() { :; }
        export_pool() { echo "    export_pool"; }
        rig_error() { echo "    rig_error: $*" | cut -c1-200; exit 2; }
        # end_daemon's status read, the first thing it does after the
        # holder: reaching it is going on to the check
        timeout() { : > "$WORK/went-on"; exit 0; }
        WORK=$work; RDIR=$work; NAME=rec; POOL=p; SKYZFSD=true; VOL=$work/vol; RUN=$work/run; SERVE_PID=""
        printf '%b' "$4" > "$RDIR/holder.log"
        [ -z "$5" ] || printf '%b' "$5" > "$RDIR/held.tsv"
        if [ "$3" = alive ]; then
            python3 -c 'import os, time
while not os.path.exists(os.sys.argv[1]): time.sleep(0.01)' "$WORK/hold.leave" &
        else
            (exit "$3") &
        fi
        HOLD_PID=$!
        [ "$3" = alive ] || while kill -0 "$HOLD_PID" 2>/dev/null; do :; done
        rec() {
            if [ "$2" = holder_wrote_one ]; then
                holder_wrote_one || { export_pool; return 0; }
                echo "    the recording went on past the wait"
                holder_leave
            else
                hold_idle
                echo "    the recording went on past the wait"
            fi
        }
        rec "$@"
        end_daemon
    )
    echo "$1: the recording's subshell ended with $?$([ ! -e "$work/went-on" ] || echo ', having gone on to end_daemon, its status read and the check')"
    for f in holder.log held.tsv hold.stop hold.leave hold.pause hold.pause.held held.img held.img.blocks went-on status.txt status-time.tsv; do
        [ ! -e "$work/$f" ] || unlink "$work/$f"
    done
}
echo "1. holder_wrote_one (takeover, failed)"
run "a verdict before the first cut" holder_wrote_one 1 "$verdict" ''
run "ended with 0 before the first cut" holder_wrote_one 0 '1\tleft\n' ''
run "ended with 1 and no verdict before the first cut" holder_wrote_one 1 '1\t> t1x1 held\n' ''
run "ended with 2 (its own error) before the first cut" holder_wrote_one 2 '1\tERROR HolderError: x\n' ''
run "wrote a cut, then a verdict" holder_wrote_one 1 "$verdict" '1\t4\titself\t1\t1\t2\n'
run "wrote a cut and stays" holder_wrote_one alive '1\t> t1x1 held\n' '1\t4\titself\t1\t1\t2\n'
echo "2. hold_idle (edges)"
run "a verdict while the recording waits for the pause" hold_idle 1 "$verdict" ''
rmdir "$work"

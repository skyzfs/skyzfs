#!/bin/bash
# #113 at 98024d5: what cut-record.sh's end_daemon does when the holder
# has ended with 1, its status for a VERDICT (Codex's finding on the pull
# request). The function is taken from the script as it is and run as
# the script runs it, in a subshell under `set -e`; the first command
# after the wait is replaced so that reaching it shows.
#
#   end_daemon_wait.sh CHECKOUT/tests/kernel/cut-record.sh
src=$1
work=$(mktemp -d)
fn=$(sed -n '/^end_daemon() {$/,/^}$/p' "$src")
leave=$(sed -n '/^holder_leave() {$/,/^}$/p' "$src")
for status in 1 2; do
    (
        set -euo pipefail
        eval "$fn"
        rm() { echo "  reached the line after the wait"; exit 0; }
        rig_error() { echo "  rig_error: $*"; exit 2; }
        WORK=$work; RDIR=$work
        (exit "$status") &
        HOLD_PID=$!
        end_daemon
    )
    echo "end_daemon, the holder ended with $status: the recording's subshell ended with $?, before any check"
done
printf '1\tVERDICT cut 9 is still being uploaded after its write was refused\n' > "$work/holder.log"
(
    set -euo pipefail
    eval "$leave"
    rig_error() { echo "  rig_error: $*"; exit 2; }
    WORK=$work; RDIR=$work
    (exit 1) &
    HOLD_PID=$!
    holder_leave
    echo "  holder_leave returned $?"
)
echo "holder_leave, the holder ended with 1 and a VERDICT in its log: the subshell ended with $?"
for f in holder.log hold.stop hold.leave; do [ ! -e "$work/$f" ] || unlink "$work/$f"; done
rmdir "$work"

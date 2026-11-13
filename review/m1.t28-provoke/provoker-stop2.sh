#!/bin/bash
# Review probe, second look at pull request #62: provoke_start and
# provoke_stop taken from tests/kernel/c-repeat-rig.sh as they are, with
# provoke() replaced by the same process structure around blocking
# commands. Run with no controlling terminal (setsid), as in a CI job.
#   provoker-stop2.sh RIG_SCRIPT
TAG=review62b$$
PROVOKER=""
provoke() {
    while :; do bash -c "exec -a $TAG-reader-dd sleep 300"; done &
    while :; do bash -c "exec -a $TAG-writer-dd sleep 0.01"; bash -c "exec -a $TAG-pause sleep 0.02"; done
}
eval "$(sed -n '/^provoke_start() {/p; /^provoke_stop() {/,/^}/p' "$1")"
declare -F provoke_start provoke_stop > /dev/null || { echo "the functions were not found in $1"; exit 2; }
tty > /dev/null 2>&1 && echo "controlling terminal: yes" || echo "controlling terminal: no"
for i in 1 2 3; do
    provoke_start
    sleep 0.7
    echo "try $i: provoker $PROVOKER, its process group $(ps -o pgid= -p "$PROVOKER" | tr -d ' '), this script's $(ps -o pgid= -p $$ | tr -d ' ')"
    t0=$(date +%s%N)
    provoke_stop
    echo "try $i: stop took $(( ($(date +%s%N) - t0) / 1000000 )) ms; left running: $(pgrep -f "$TAG-" | wc -l)"
done
pkill -f "$TAG-" 2>/dev/null
exit 0

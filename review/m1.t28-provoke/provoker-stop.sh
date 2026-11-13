#!/bin/bash
# Review probe for pull request #62: the process structure of provoke()
# in tests/kernel/c-repeat-rig.sh and its stop sequence, with the reader's
# dd replaced by a command that blocks (a read of a device whose server
# is dead blocks until the next server answers). What is left running
# after the stop?
#   provoker-stop.sh as-merged | group
MODE=${1:-as-merged}
TAG=review62probe$$
provoke() {
    while :; do
        # stands for: dd if=$d of=/dev/null ... iflag=nocache  (blocked)
        bash -c "exec -a $TAG-reader-dd sleep 300"
    done &
    while :; do
        # stands for: dd if=/dev/null of=$d conv=nocreat,notrunc
        bash -c "exec -a $TAG-writer-dd sleep 0.01"
        bash -c "exec -a $TAG-pause sleep 0.02"
    done
}
if [ "$MODE" = as-merged ]; then
    provoke & PROVOKER=$!
    sleep 1
    pkill -P "$PROVOKER" 2>/dev/null; kill "$PROVOKER" 2>/dev/null; wait "$PROVOKER" 2>/dev/null
else
    # The alternative: the provoker in a process group of its own.
    set -m; provoke & PROVOKER=$!; set +m
    sleep 1
    kill -- "-$PROVOKER" 2>/dev/null; wait "$PROVOKER" 2>/dev/null
fi
sleep 0.5
echo "$MODE: left running after the stop:"
pgrep -af "$TAG" | sed 's/^/  /' || true
[ -n "$(pgrep -f "$TAG")" ] || echo "  nothing"
pkill -f "$TAG" 2>/dev/null
exit 0

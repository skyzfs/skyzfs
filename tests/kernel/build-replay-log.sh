#!/usr/bin/env bash
# Build xfstests' replay-log, the reference reader of a dm-log-writes
# log, at a pinned commit. The replay rig uses it as a witness: its
# image must equal the one logwrites.py produces.
#
#   build-replay-log.sh DEST-DIR      -> DEST-DIR/replay-log
#
# Three source files, fetched from the GitHub mirror of xfstests.
set -euo pipefail
DEST=${1:?usage: build-replay-log.sh DEST-DIR}
COMMIT=d07dca72d2455f4ba8c660ad4c008541ccccb707
BASE=https://raw.githubusercontent.com/kdave/xfstests/$COMMIT/src/log-writes
SRC=$(mktemp -d)
for f in log-writes.c log-writes.h replay-log.c; do
    curl -fsSL --retry 3 -o "$SRC/$f" "$BASE/$f"
done
mkdir -p "$DEST"
cc -O2 -o "$DEST/replay-log" "$SRC/replay-log.c" "$SRC/log-writes.c"
echo "replay-log from xfstests $COMMIT"

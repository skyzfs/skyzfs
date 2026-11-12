# The rig's writer() with its tools replaced: which status does each failing step give?
set -euo pipefail
T=$(mktemp -d); mkdir -p "$T/bin" "$T/scratch" "$T/pool"
SCRATCH=$T/scratch; MOUNTPOINT=$T/pool; MANIFEST=$T/manifest; STOP=$T/stop; LW=x; FILE_KB=8; FILE_SLEEP=0
. "$1"
try() { # name: runs writer with PATH and DD as set, one file, then stops
    local rc=0
    : > "$MANIFEST"; rm -f "$STOP"
    ( writer ) & local w=$!
    sleep 0.5; touch "$STOP"
    wait "$w" || rc=$?
    echo "$1: status $rc, manifest lines $(wc -l < "$MANIFEST")"
}
printf '#!/bin/sh\nexit 0\n' > "$T/bin/dmsetup"; chmod +x "$T/bin/dmsetup"
PATH=$T/bin:$PATH
DD=dd; try "all steps work"
DD=false; try "the write on the pool fails"
DD=dd; printf '#!/bin/sh\nexit 1\n' > "$T/bin/dmsetup"; try "the mark fails"
printf '#!/bin/sh\nexit 0\n' > "$T/bin/dmsetup"; SCRATCH=$T/no-such-dir; try "the scratch file cannot be written"
rm -rf "$T"

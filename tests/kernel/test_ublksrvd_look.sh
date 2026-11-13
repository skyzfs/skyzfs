#!/bin/bash
# Tests of ublksrvd-look.sh on a directory of sample names: no kernel.
# Run: bash tests/kernel/test_ublksrvd_look.sh
HERE=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d); trap 'rm -rf "$T"' EXIT
UBLKSRVD_DIR=$T/run
# shellcheck source=tests/kernel/ublksrvd-look.sh
. "$HERE/ublksrvd-look.sh"
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
mkdir "$UBLKSRVD_DIR"
# libublk's name for device 0 and for device 10; the reference
# server's names for device 0 and device 10; names of no device.
for f in 0000.json 0.pid ublk_0.json 0 10.pid 0010.json ublk_10.json 0001.json notes.txt ublk_.json x0.json; do
    : > "$UBLKSRVD_DIR/$f"
done
look() { : > "$T/seen"; ublksrvd_look "$1" "$2" "$T/seen"; grep '^entry ' "$T/seen" | sed 's|.*/||' | tr '\n' ' '; }

is "device 0: every padding, both servers' names" "$(look 0 0)" "0 0.pid 0000.json ublk_0.json "
is "device 10 is not device 0 or 1" "$(look 10 0)" "0010.json 10.pid ublk_10.json "
is "device 1: only its own" "$(look 1 0)" "0001.json "
is "device 2: none" "$(look 2 0)" ""
is "the serve is written down even with no entry" "$(grep -c '^serve 2$' "$T/seen")" 1
# Only what is newer than the serve's start counts.
is "nothing is newer than a time to come" "$(look 0 "$(( $(date +%s) + 3600 ))")" ""
touch -d '2001-01-01' "$UBLKSRVD_DIR/0.pid"
is "an entry older than the start is left out" "$(look 0 1000000000)" "0 0000.json ublk_0.json "
# A leading zero is not octal.
: > "$UBLKSRVD_DIR/0008.json"
is "0008 is device 8" "$(look 8 0)" "0008.json "
is "no directory is no entry" "$(UBLKSRVD_DIR=$T/none; look 0 0)" ""
is "no file to write to does nothing" "$(ublksrvd_look 0 0 ""; echo $?)" 0

[ "$fails" = 0 ] || { echo "$fails failed"; exit 1; }
echo "all passed"

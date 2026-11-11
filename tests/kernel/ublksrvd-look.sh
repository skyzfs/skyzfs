# shellcheck shell=bash
# ublksrvd_look NUMBER SINCE FILE: after a serve of the daemon has made
# /dev/ublkbNUMBER ready, write to FILE one line "serve NUMBER" and one
# line "entry PATH" for every entry of /run/ublksrvd for that number
# that is newer than SINCE (seconds since the epoch). The reference
# server keeps a pid file and its JSON there under the device's number;
# the daemon must cause none (design 4.2, Library).
ublksrvd_look() {
    [ -n "$3" ] || return 0
    echo "serve $1" >> "$3"
    find /run/ublksrvd -mindepth 1 -maxdepth 1 \( -name "$1.*" -o -name "$1" -o -name "*_$1.*" \) -newermt "@$2" 2>/dev/null |
        sed 's/^/entry /' >> "$3"
}

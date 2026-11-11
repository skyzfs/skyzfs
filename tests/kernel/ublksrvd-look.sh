# shellcheck shell=bash
# ublksrvd_look NUMBER SINCE FILE: after a serve of the daemon has made
# /dev/ublkbNUMBER ready, write to FILE one line "serve NUMBER" and one
# line "entry PATH" for every entry of /run/ublksrvd for that number
# that is newer than SINCE (seconds since the epoch). The reference
# server keeps a pid file and its JSON there under the device's number;
# the daemon must cause none (design 4.2, Library).
#
# Whose entry a name is: the number is read from the name and compared
# as a number, whatever its padding. The reference server writes
# "N.pid" and "N.json" or "ublk_N.json"; libublk writes the number
# padded to four digits, "0000.json" for device 0. A name is
# [WORD_]DIGITS[.REST]; anything else belongs to no device.
UBLKSRVD_DIR=${UBLKSRVD_DIR:-/run/ublksrvd}
ublksrvd_number() { # NAME -> the device number in it, in decimal; 1 if it has none
    local n=${1##*_}        # after the last "_", if there is one
    n=${n%%.*}              # before the first "."
    [[ $n =~ ^[0-9]+$ ]] || return 1
    echo $((10#$n))
}
ublksrvd_look() {
    local f n
    [ -n "$3" ] || return 0
    echo "serve $1" >> "$3"
    find "$UBLKSRVD_DIR" -mindepth 1 -maxdepth 1 -newermt "@$2" 2>/dev/null | sort |
        while IFS= read -r f; do
            n=$(ublksrvd_number "${f##*/}") || continue
            [ "$n" = "$((10#$1))" ] && echo "entry $f" >> "$3"
        done
    return 0
}

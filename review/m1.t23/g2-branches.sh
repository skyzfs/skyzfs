#!/bin/bash
# g2-branches.sh RIG (updated for 8a3fb89, which reads the cache just
# before the start): walk the verdict branches of check g.2 with
# stand-ins for the kernel's part. Each case sets what the rig's write
# said, what the disk held and what the start served, for g0 and for g2.
set -uo pipefail
rig=$1
eval "$(sed -n '/^check_g_start_after_failed_sync() {$/,/^}$/p' "$rig")"
eval "$(sed -n '/^g_table() {$/,/^}$/p' "$rig")"
OUT=$(mktemp -d); G_MNT=$OUT/mnt; mkdir -p "$G_MNT"; SKYZFSD=daemon; G_AT=67108864; G_LEN=65536; BDEV=/dev/ublkb9
SERVE_PID=1; SERVE_RC=0
step() { :; }; rec() { :; }
pin() { echo "    pin_$1 $2: ${3:0:110}"; }
ctl() { echo "    ctl_$1 $2: ${3:0:110}"; }
obs() { :; }
g_dir() { echo "$G_MNT/vol-$1"; }
vol_create() { echo created; }; vol_detach() { :; }; serve_wait() { :; }
g_table() { [ "${TABLE_FAILS:-}" = "$1" ] && return 1; return 0; }
serve_start() { [ "$START" = ok ]; }
kill() { :; }
fw() {
    case "$1 $2" in
        "fill "*) echo filled ;;
        "extent "*) echo "$EXTENT" ;;
        "write "*) case $2 in *g0*) echo "$W0" ;; *) echo "$W2" ;; esac ;;
        "direct /dev/"*) case $CUR in g0) echo "$S0" ;; *) echo "$S2" ;; esac ;;
        "direct "*) case $2 in *g0*) echo "$D0" ;; *) echo "$D2" ;; esac ;;
        "sync "*) echo "fdatasync ok; syncfs ok" ;;
        "read "*) case $2 in *g0*) echo "${C0:-byte 0x22}" ;; *) echo "${C2:-byte 0x22}" ;; esac ;;
    esac
}
# the function reads the device after the start; CUR says which volume
serve_start() { case $2 in *g0) CUR=g0 ;; *) CUR=g2 ;; esac; [ "$START" = ok ]; }
run() { echo "$1"; shift; ( eval "$*"; check_g_start_after_failed_sync ); }
ok0='W0="fdatasync ok; cache reads byte 0x22" D0="byte 0x22"'
ok2='W2="fdatasync EIO(5); cache reads byte 0x22" D2="byte 0x11"'
base="EXTENT='100 128' START=ok TABLE_FAILS="
run "as on a real kernel, a start that drops the cache"      "$base $ok0 $ok2 S0='byte 0x22' S2='byte 0x11'"
run "a start that serves the cache"                          "$base $ok0 $ok2 S0='byte 0x22' S2='byte 0x22'"
run "a start that serves a mix"                              "$base $ok0 $ok2 S0='byte 0x22' S2='mixed'"
run "a start that serves the old content in the control too" "$base $ok0 $ok2 S0='byte 0x11' S2='byte 0x11'"
run "the serve does not start"                               "EXTENT='100 128' START=no TABLE_FAILS= $ok0 $ok2 S0=x S2=x"
run "the kernel's fdatasync does not fail"                   "$base $ok0 W2='fdatasync ok; cache reads byte 0x22' D2='byte 0x11' S0='byte 0x22' S2='byte 0x11'"
run "the cache does not keep the write"                      "$base $ok0 W2='fdatasync EIO(5); cache reads byte 0x11' D2='byte 0x11' S0='byte 0x22' S2='byte 0x11'"
run "the disk got the write"                                 "$base $ok0 W2='fdatasync EIO(5); cache reads byte 0x22' D2='byte 0x22' S0='byte 0x22' S2='byte 0x22'"
run "the control's write does not sync"                      "$base W0='fdatasync EIO(5); cache reads byte 0x22' D0='byte 0x11' $ok2 S0='byte 0x11' S2='byte 0x11'"
run "the range is in two extents"                            "EXTENT='the range is not in one extent' START=ok TABLE_FAILS= $ok0 $ok2 S0=x S2=x"
run "the range table cannot be loaded"                       "EXTENT='100 128' START=ok TABLE_FAILS=range $ok0 $ok2 S0='byte 0x22' S2='byte 0x11'"
run "the cache no longer reads the write just before the start" "$base $ok0 $ok2 S0='byte 0x22' S2='byte 0x11' C2='byte 0x11'"
run "the control's cache does not read the write either"      "$base $ok0 $ok2 S0='byte 0x22' S2='byte 0x11' C0='byte 0x11'"
run "the cache read fails just before the start"              "$base $ok0 $ok2 S0='byte 0x22' S2='byte 0x11' C2='read EIO(5)'"
run "the linear table cannot be put back"                    "EXTENT='100 128' START=ok TABLE_FAILS=linear $ok0 $ok2 S0='byte 0x22' S2='byte 0x11'"

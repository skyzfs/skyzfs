#!/usr/bin/env bash
# A stand-in for skyzfsd in the kernel rig: the prototype command line of
# the daemon (M1.3), carried out by the reference ublk server
# (ublk-org/ublksrv, loop target, with tests/kernel/ublksrv-skz.patch).
# The rig talks to this or to the daemon through one variable, and
# everything that depends on the daemon's command line is here and in
# the table at the top of daemon-rig.sh.
#
#   skyzfsd-ref.sh create --dir DIR --size BYTES
#   skyzfsd-ref.sh serve  --dir DIR [--run-dir DIR] [--acknowledge-failure]
#                         [--fail-log-after N] [--hold-one-request SECONDS:read|write|flush]
#   skyzfsd-ref.sh detach --dir DIR
#   skyzfsd-ref.sh status --dir DIR
#
# Exit statuses: 0 done, or `serve` stopped by SIGTERM; 2 usage; 3 the
# volume failed while it was served; 4 refused, the volume failed in
# this boot; 5 refused, another instance serves the volume; 1 anything
# else, including a server that died under `serve`.
#
# Where this is not the daemon, and the rig says so on each pin:
#   - An orderly stop is a kill: the reference server has no way to stop
#     and leave its device waiting, other than dying.
#   - The lock on the volume is this script's, not a store's.
#   - Only in the environment, for the deliberately wrong servers the
#     rig's controls need: SKZ_REF_REISSUE=0 (recovery that fails the
#     requests in flight), SKZ_REF_DROP_EVERY=K (every Kth write is
#     acknowledged and not written), SKZ_REF_FAIL_STYLE=eio (a failed
#     volume keeps its device and answers every request with an I/O
#     error; the default, delete, removes the device as the daemon
#     will).
set -uo pipefail

usage() { echo "usage: skyzfsd-ref.sh create|serve|detach|status --dir DIR ..." >&2; exit 2; }
[ $# -ge 1 ] || usage
CMD=$1; shift
DIR=""; SIZE=""; RUN_DIR=/run/skyzfsd; ACK=""; FAIL_AFTER=""; HOLD=""
while [ $# -gt 0 ]; do
    case $1 in
        --dir) [ $# -ge 2 ] || usage; DIR=$2; shift 2 ;;
        --size) [ $# -ge 2 ] || usage; SIZE=$2; shift 2 ;;
        --run-dir) [ $# -ge 2 ] || usage; RUN_DIR=$2; shift 2 ;;
        --acknowledge-failure) ACK=1; shift ;;
        --fail-log-after) [ $# -ge 2 ] || usage; FAIL_AFTER=$2; shift 2 ;;
        --hold-one-request) [ $# -ge 2 ] || usage; HOLD=$2; shift 2 ;;
        *) usage ;;
    esac
done
[ -n "$DIR" ] || usage
IMG=$DIR/volume.img
STATS=$DIR/stats
DEVREC=$DIR/device          # the ublk device number of this volume
PIDREC=$DIR/server.pid

lim() { local s=$1; shift; timeout -k 5 "$s" "$@"; }
dev_state() { lim 20 ublk list -n "$1" 2>/dev/null | sed -n 's/.*state \([A-Z_]*\).*/\1/p' | head -n 1; }
dev_pid() { lim 20 ublk list -n "$1" 2>/dev/null | sed -n 's/.*daemon pid \([0-9]*\).*/\1/p' | head -n 1; }
stat_field() { # index into the patched server's counters (enum skz_stat)
    [ -s "$STATS" ] || { echo 0; return; }
    od -An -v -t u8 "$STATS" | awk -v want="$(($1 + 1))" '{ for (i = 1; i <= NF; i++) if (++n == want) print $i }'
}
# Whether something holds the device open exclusively, as an imported
# pool does. If it cannot be told, the answer is yes (design §4.2).
held_open() {
    python3 -I -c '
import errno, os, sys
try:
    os.close(os.open(sys.argv[1], os.O_RDONLY | os.O_EXCL))
except OSError as e:
    sys.exit(0 if e.errno == errno.EBUSY else 3)
sys.exit(1)' "$1"
    case $? in 1) echo 0 ;; *) echo 1 ;; esac
}
note() { echo "$RUN_DIR/$(printf '%s' "$(cd "$DIR" && pwd)" | sha256sum | cut -c1-16).failed"; }

case $CMD in
create)
    if ! [[ $SIZE =~ ^[0-9]+$ ]] || [ "$SIZE" -eq 0 ] || [ $((SIZE % 1048576)) -ne 0 ]; then usage; fi
    mkdir -p "$DIR" || exit 1
    [ ! -e "$IMG" ] || { echo "there is a volume in $DIR already" >&2; exit 1; }
    truncate -s "$SIZE" "$IMG" || exit 1
    ;;
status)
    [ -e "$IMG" ] || { echo "no volume in $DIR" >&2; exit 1; }
    n=$(cat "$DEVREC" 2>/dev/null || true)
    state=detached
    if [ -n "$n" ]; then
        case $(dev_state "$n") in
            LIVE) state=serving ;;
            QUIESCED) state=waiting ;;
            *) state=detached ;;
        esac
    fi
    [ "$(stat_field 7)" = 0 ] || state=failed
    [ ! -e "$(note)" ] || state=failed
    echo "state $state"
    [ -z "$n" ] || echo "device /dev/ublkb$n"
    [ -z "$n" ] || echo "pid $(dev_pid "$n")"
    echo "queues 1"
    echo "depth 64"
    i=0
    for k in reads writes flushes discards write_zeroes failed_requests held; do
        echo "$k $(stat_field $i)"; i=$((i + 1))
    done
    ;;
detach)
    n=$(cat "$DEVREC" 2>/dev/null || true)
    [ -n "$n" ] || exit 0
    lim 60 ublk del -n "$n" >/dev/null 2>&1
    unlink "$DEVREC" 2>/dev/null
    unlink "$PIDREC" 2>/dev/null
    exit 0
    ;;
serve)
    [ -e "$IMG" ] || { echo "no volume in $DIR" >&2; exit 1; }
    [ -z "$FAIL_AFTER" ] || [[ $FAIL_AFTER =~ ^[0-9]+$ ]] || usage
    [ -z "$HOLD" ] || [[ $HOLD =~ ^[0-9]+:(read|write|flush)$ ]] || usage
    # One instance per volume. The server must not inherit the lock: it
    # outlives this script when the script is killed.
    exec 9> "$DIR/lock" || exit 1
    flock -n 9 || { echo "another instance serves $DIR" >&2; exit 5; }
    n=$(cat "$DEVREC" 2>/dev/null || true)
    if [ -n "$n" ] && [ "$(dev_state "$n")" = LIVE ]; then
        # A server that was just killed leaves its device LIVE for a
        # moment before the kernel notices.
        old=$(cat "$PIDREC" 2>/dev/null || true)
        i=0
        while [ -n "$old" ] && ! kill -0 "$old" 2>/dev/null && [ "$(dev_state "$n")" = LIVE ] && [ $i -lt 80 ]; do
            sleep 0.25; i=$((i + 1))
        done
        if [ "$(dev_state "$n")" = LIVE ]; then
            echo "another server is live on /dev/ublkb$n" >&2; exit 5
        fi
    fi
    # A volume that failed in this boot.
    NOTE=$(note)
    if [ -e "$NOTE" ]; then
        if [ -n "$ACK" ] && grep -qx 'held 0' "$NOTE"; then
            unlink "$NOTE"
            : > "$STATS"            # the stand-in's "log" is whole again
        else
            echo "the volume failed in this boot: $(tr '\n' ' ' < "$NOTE")" >&2; exit 4
        fi
    fi
    # No note, and yet the counters say failed: the note was emptied, as
    # a reboot empties /run. The stand-in has no log to check, so the
    # volume simply starts again.
    [ -e "$NOTE" ] || [ "$(stat_field 7)" = 0 ] || : > "$STATS"
    export SKZ_STATS=$STATS
    [ -z "$FAIL_AFTER" ] || export SKZ_FAIL_AFTER=$FAIL_AFTER
    [ -z "$HOLD" ] || export SKZ_HOLD=$HOLD
    [ -z "${SKZ_REF_DROP_EVERY:-}" ] || export SKZ_DROP_EVERY=$SKZ_REF_DROP_EVERY
    if [ -n "$n" ] && [ "$(dev_state "$n")" = QUIESCED ]; then
        lim 90 ublk recover -n "$n" >&2 9>&- || { echo "recovery of /dev/ublkb$n failed" >&2; exit 1; }
    else
        n=0; while [ -e "/dev/ublkc$n" ]; do n=$((n + 1)); done
        echo "$n" > "$DEVREC"; sync "$DEVREC"
        lim 60 ublk add -t loop -n "$n" -f "$IMG" -q 1 -d 64 --max_io_buf_bytes 1048576 \
            -r 1 -i "${SKZ_REF_REISSUE:-1}" >&2 9>&- || { echo "ublk add failed" >&2; exit 1; }
    fi
    i=0
    while { [ ! -b "/dev/ublkb$n" ] || [ "$(dev_state "$n")" != LIVE ]; } && [ $i -lt 40 ]; do
        sleep 0.25; i=$((i + 1))
    done
    if [ ! -b "/dev/ublkb$n" ] || [ "$(dev_state "$n")" != LIVE ]; then
        echo "/dev/ublkb$n did not go live" >&2; exit 1
    fi
    pid=$(dev_pid "$n")
    [ -n "$pid" ] || { echo "no server pid for /dev/ublkb$n" >&2; exit 1; }
    echo "$pid" > "$PIDREC"
    STOP=""
    trap 'STOP=1' TERM INT
    echo "ready /dev/ublkb$n"
    while :; do
        if [ -n "$STOP" ]; then
            # The reference server cannot stop and leave its device
            # waiting for a server, except by dying.
            kill -9 "$pid" 2>/dev/null
            exit 0
        fi
        kill -0 "$pid" 2>/dev/null || { echo "the server died" >&2; exit 1; }
        if [ "$(stat_field 7)" != 0 ]; then
            mkdir -p "$RUN_DIR"
            { echo "boot $(cat /proc/sys/kernel/random/boot_id)"; echo "held $(held_open "/dev/ublkb$n")"; } > "$NOTE"
            if [ "${SKZ_REF_FAIL_STYLE:-delete}" = delete ]; then
                # Without waiting for whoever still holds the device
                # open: a suspended pool never lets go.
                lim 60 ublk del --async -n "$n" >/dev/null 2>&1
                unlink "$DEVREC" 2>/dev/null
                exit 3
            fi
            # eio: the device stays and answers every request with an
            # I/O error until this process is told to stop.
            while [ -z "$STOP" ] && kill -0 "$pid" 2>/dev/null; do sleep 0.2; done
            exit 3
        fi
        sleep 0.2
    done
    ;;
*) usage ;;
esac

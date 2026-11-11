#!/usr/bin/env bash
# The helper command of skyzfs-blkcheck (check c of the kernel rig): it
# makes, kills, fails and reopens volumes with a skyzfsd, or with the
# stand-in tests/kernel/skyzfsd-ref.sh, so that the conformance suite of
# skyzfs-blk runs through the kernel's block layer.
#
#   blkcheck-helper.sh create N    a new empty volume, served; prints its device
#   blkcheck-helper.sh kill N      SIGKILL to its server, then serve again; prints the device
#   blkcheck-helper.sh fail N      SIGUSR1 to its server: the log fails for good
#   blkcheck-helper.sh reopen N    the failed volume served again, as after a
#                                  reboot and with --acknowledge-failure; prints the device
#   blkcheck-helper.sh destroy N   stop serving and remove the volume
#
# Environment: BLK_SKYZFSD (the server; default the stand-in), BLK_WORK
# (a directory for the volumes, required), BLK_SERVE_ARGS (options for
# every serve), BLK_BYTES (volume size, default 64 MiB).
#
# What it writes besides the device path goes to stderr and to
# BLK_WORK/helper.log, with the time each verb took.
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
{ [ $# = 2 ] && [[ $2 =~ ^[0-9]+$ ]]; } || { echo "usage: blkcheck-helper.sh create|kill|fail|reopen|destroy N" >&2; exit 2; }
VERB=$1; N=$2
WORK=${BLK_WORK:?}
SKYZFSD=${BLK_SKYZFSD:-$HERE/skyzfsd-ref.sh}
BYTES=${BLK_BYTES:-$((64 << 20))}
read -r -a SERVE_ARGS <<< "${BLK_SERVE_ARGS:-}"
DIR=$WORK/c$N
RUN=$WORK/run$N               # one run directory per volume, so that reopen can empty it
T0=$(date +%s%N)

die() { echo "blkcheck-helper $VERB $N: $*" >&2; echo "$VERB $N FAILED: $*" >> "$WORK/helper.log"; exit 1; }
status_of() { lim 30 "$SKYZFSD" status --dir "$DIR" --run-dir "$RUN" 2>/dev/null | awk -v k="$1" '$1 == k { print $2; exit }'; }
# The process that is the server: what status calls pid if it says, else
# the serve process itself.
server_pid() { local p; p=$(status_of pid); echo "${p:-$(cat "$DIR.serve.pid" 2>/dev/null)}"; }
# [serve options]: serve in the background, wait for "ready", print the device.
serve() {
    local dev pid
    : > "$DIR.ready"
    # Detached from this script's stdout, which the caller reads to its end.
    setsid "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$DIR" --run-dir "$RUN" "$@" > "$DIR.ready" 2>> "$DIR.serve.log" < /dev/null &
    pid=$!
    echo "$pid" > "$DIR.serve.pid"
    for _ in $(seq 1 240); do
        dev=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$DIR.ready" | head -n 1)
        if [ -n "$dev" ] && [ -b "$dev" ]; then echo "$dev"; return 0; fi
        kill -0 "$pid" 2>/dev/null || { wait "$pid"; die "serve exited $? before it was ready: $(tail -n 2 "$DIR.serve.log" | tr '\n' ' ')"; }
        sleep 0.25
    done
    die "serve was not ready after 60 s"
}
stop_serve() { # SIGTERM to serve, and wait for it to go
    local pid
    pid=$(cat "$DIR.serve.pid" 2>/dev/null) || return 0
    kill -TERM "$pid" 2>/dev/null
    for _ in $(seq 1 120); do kill -0 "$pid" 2>/dev/null || return 0; sleep 0.25; done
    kill -9 "$pid" 2>/dev/null
}

mkdir -p "$WORK" || exit 1
case $VERB in
create)
    lim 60 "$SKYZFSD" create --dir "$DIR" --size "$BYTES" >&2 || die "create failed"
    DEV=$(serve) || exit 1
    ;;
kill)
    pid=$(server_pid); [ -n "$pid" ] || die "no server pid"
    kill -9 "$pid" 2>/dev/null || die "the server ($pid) was not there to kill"
    # With the stand-in the server is a child of the serve script,
    # which notices and exits; with the daemon they are one process.
    serve_pid=$(cat "$DIR.serve.pid")
    for _ in $(seq 1 120); do kill -0 "$serve_pid" 2>/dev/null || break; sleep 0.1; done
    DEV=$(serve) || exit 1
    ;;
fail)
    pid=$(cat "$DIR.serve.pid" 2>/dev/null); [ -n "$pid" ] || die "no serve pid"
    kill -USR1 "$pid" 2>/dev/null || die "serve ($pid) was not there"
    # The suite's next request must meet a failed volume: wait until the
    # server says so, or is gone.
    for _ in $(seq 1 100); do
        [ "$(status_of state)" = failed ] && break
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    [ "$(status_of state)" = failed ] || ! kill -0 "$pid" 2>/dev/null || die "the server did not say failed within 5 s of SIGUSR1"
    DEV=""
    ;;
reopen)
    # serve has exited, or does now that the client has let go.
    serve_pid=$(cat "$DIR.serve.pid" 2>/dev/null)
    for _ in $(seq 1 240); do kill -0 "$serve_pid" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$serve_pid" 2>/dev/null && stop_serve
    lim 90 "$SKYZFSD" detach --dir "$DIR" --run-dir "$RUN" >&2
    # As a reboot does: what /run held is gone.
    find "$RUN" -mindepth 1 -delete 2>/dev/null
    DEV=$(serve --acknowledge-failure) || exit 1
    ;;
destroy)
    stop_serve
    lim 90 "$SKYZFSD" detach --dir "$DIR" --run-dir "$RUN" >&2
    find "$DIR" "$RUN" "$DIR.ready" "$DIR.serve.pid" -delete 2>/dev/null
    DEV=""
    ;;
*) echo "usage: blkcheck-helper.sh create|kill|fail|reopen|destroy N" >&2; exit 2 ;;
esac
# The largest request the device takes, for the rig: the suite's cases
# about a 1 MiB request exercise the server only if this is 1024.
MAXKB=""; [ -z "$DEV" ] || MAXKB=$(cat "/sys/block/${DEV#/dev/}/queue/max_hw_sectors_kb" 2>/dev/null)
echo "$VERB $N $(( ($(date +%s%N) - T0) / 1000000 )) ms${DEV:+ $DEV max_hw_sectors_kb=$MAXKB}" >> "$WORK/helper.log"
[ -z "$DEV" ] || echo "$DEV"
exit 0

#!/usr/bin/env bash
# The helper command of skyzfs-blkcheck (check c of the kernel rig): it
# makes, kills, fails and reopens volumes with a skyzfsd, or with the
# stand-in tests/kernel/skyzfsd-ref.sh, so that the conformance suite of
# skyzfs-blk runs through the kernel's block layer.
#
#   blkcheck-helper.sh create N    a new empty volume, served; prints its device
#   blkcheck-helper.sh pid N       prints the pid of its server, for a caller that sends the
#                                  SIGKILL itself and then calls kill.
#   blkcheck-helper.sh kill N      SIGKILL to its server; when it is gone, print "dead R W"
#                                  (the kernel's reads and writes in flight at the device);
#                                  wait for the line "sorted" on stdin; then serve again;
#                                  prints the device on the last line.
#                                  After `fail` there is no server left: nothing is done
#   blkcheck-helper.sh fail N      SIGUSR1 to its server: the log fails for good. Returns
#                                  when serve has exited with status 3; the device is
#                                  then a deleted one that the caller still holds
#   blkcheck-helper.sh reopen N    the failed volume served again, as after a
#                                  reboot and with --acknowledge-failure; prints the device
#   blkcheck-helper.sh destroy N   stop serving and remove the volume
#
# Environment: BLK_SKYZFSD (the server; default the stand-in), BLK_WORK
# (a directory for the volumes, required), BLK_SERVE_ARGS (options for
# every serve), BLK_BYTES (volume size, default 64 MiB), BLK_PREFILL_MB
# (after create, so many MiB of zero data are written to the new
# volume by plain writes, over and over it: the volume still reads as
# empty, and the server's log holds that much when the caller begins;
# default 0).
#
# What it writes besides the device path goes to stderr and to
# BLK_WORK/helper.log, with the time each verb took.
set -uo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# shellcheck source=tests/kernel/ublksrvd-look.sh
. "$HERE/ublksrvd-look.sh"
# shellcheck source=tests/lib/gnu-dd.sh
. "$HERE/../lib/gnu-dd.sh"
{ [ $# = 2 ] && [[ $2 =~ ^[0-9]+$ ]]; } || { echo "usage: blkcheck-helper.sh create|pid|kill|fail|reopen|destroy N" >&2; exit 2; }
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
    local dev pid t0
    t0=$(( $(date +%s) - 1 ))
    : > "$DIR.ready"; : > "$DIR.serve.pid"
    # Detached from this script's stdout, which the caller reads to its end.
    # A later call of this script must learn the exit status of serve,
    # which is not its child: a small shell stays with it and writes
    # its pid and then its status down.
    : > "$DIR.serve.rc"
    # shellcheck disable=SC2016
    setsid bash -c '
        "$@" &
        echo $! > "$0.serve.pid"
        wait $!
        echo $? > "$0.serve.rc"' "$DIR" "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$DIR" --run-dir "$RUN" "$@" > "$DIR.ready" 2>> "$DIR.serve.log" < /dev/null &
    for _ in $(seq 1 100); do [ -s "$DIR.serve.pid" ] && break; sleep 0.02; done
    pid=$(cat "$DIR.serve.pid")
    for _ in $(seq 1 240); do
        dev=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$DIR.ready" | head -n 1)
        if [ -n "$dev" ] && [ -b "$dev" ]; then
            # BLK_UBLKSRVD_SEEN: set by the rig when the server is the daemon.
            ublksrvd_look "${dev#/dev/ublkb}" "$t0" "${BLK_UBLKSRVD_SEEN:-}"
            echo "$dev" > "$DIR.device"; echo "$dev"; return 0
        fi
        kill -0 "$pid" 2>/dev/null || { sleep 0.2; die "serve exited $(cat "$DIR.serve.rc") before it was ready: $(tail -n 2 "$DIR.serve.log" | tr '\n' ' ')"; }
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
    left=${BLK_PREFILL_MB:-0}
    while [ "$left" -gt 0 ]; do
        n=$((BYTES >> 20)); [ "$n" -le "$left" ] || n=$left
        lim 120 "$DD" if=/dev/zero of="$DEV" bs=1M count="$n" oflag=direct conv=nocreat,fsync status=none >&2 || die "the pre-fill of ${BLK_PREFILL_MB} MiB failed with $left MiB to go"
        left=$((left - n))
    done
    [ "${BLK_PREFILL_MB:-0}" = 0 ] || echo "prefill $N: ${BLK_PREFILL_MB} MiB of zero data; the server says log_live $(status_of log_live), checkpoint_steps $(status_of checkpoint_steps)" >> "$WORK/helper.log"
    ;;
kill)
    serve_pid=$(cat "$DIR.serve.pid" 2>/dev/null)
    if [ -n "$serve_pid" ] && ! kill -0 "$serve_pid" 2>/dev/null && [ "$(cat "$DIR.serve.rc" 2>/dev/null)" = 3 ]; then
        # The log failed and the server exited by itself: there is no
        # daemon to lose, and a failed volume is not served again
        # without `reopen`. The device stays the deleted one.
        echo "kill $N: nothing to kill, the server exited when its log failed" >> "$WORK/helper.log"
        cat "$DIR.device"
        exit 0
    fi
    # The caller may have sent the SIGKILL itself already, to have it
    # land while its requests are in flight (see `pid`): then the pid is
    # the one it was given, and the server is dying or gone.
    pid=$(cat "$DIR.server.pid" 2>/dev/null); [ -n "$pid" ] || pid=$(server_pid)
    [ -n "$pid" ] || die "no server pid"
    kill -9 "$pid" 2>/dev/null
    # With the stand-in the server is a child of the serve script,
    # which notices and exits; with the daemon they are one process.
    serve_pid=$(cat "$DIR.serve.pid")
    for _ in $(seq 1 120); do kill -0 "$serve_pid" 2>/dev/null || break; sleep 0.1; done
    for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.02; done
    kill -0 "$pid" 2>/dev/null && die "the server ($pid) was still there 2 s after SIGKILL"
    # No server runs now, so no request completes until the next one
    # serves. A call of the caller's that has returned by now was
    # completed by the old server; one still outstanding was in flight
    # when it died, and the kernel will issue it again. The caller sorts
    # its calls on this line (skyzfs-blkcheck, `kill`), after a moment
    # for the returns to reach it, and gets the kernel's own count of
    # reads and writes in flight at the device to check itself against.
    sleep 0.2
    read -r in_r in_w < "/sys/block/$(basename "$(cat "$DIR.device")")/inflight" 2>/dev/null || { in_r=""; in_w=""; }
    echo "dead $in_r $in_w"
    # The next server starts only when the caller has sorted: it says
    # so with a line on this script's stdin. A new server that started
    # earlier would complete calls the caller then takes for the old
    # server's.
    reply=""
    read -r -t 60 reply || die "the caller did not answer the 'dead' line within 60 s"
    [ "$reply" = sorted ] || die "the caller answered '$reply' to the 'dead' line, not 'sorted'"
    echo "kill $N: sorted with the kernel counting ${in_r:-?} reads and ${in_w:-?} writes in flight" >> "$WORK/helper.log"
    DEV=$(serve) || exit 1
    # What the new server found applied and not acknowledged, by its own
    # account; "?" from a server that does not say.
    # The daemon writes its status on an interval: wait until the
    # status is the new server's (another pid, alive) before reading.
    for _ in $(seq 1 30); do
        new=$(status_of pid)
        { [ -n "$new" ] && [ "$new" != "$pid" ] && kill -0 "$new" 2>/dev/null; } && break
        sleep 0.1
    done
    found=$(status_of recovered_unacknowledged)
    echo "kill $N take-over found ${found:-?}" >> "$WORK/helper.log"
    ;;
fail)
    pid=$(cat "$DIR.serve.pid" 2>/dev/null); [ -n "$pid" ] || die "no serve pid"
    if ! kill -0 "$pid" 2>/dev/null && [ "$(cat "$DIR.serve.rc" 2>/dev/null)" = 3 ]; then
        # The log has failed already and the server is gone: it stays failed.
        echo "fail $N: the log had failed already" >> "$WORK/helper.log"
        exit 0
    fi
    kill -USR1 "$pid" 2>/dev/null || die "serve ($pid) was not there"
    # A failed log is not a state the server stays in: it writes its
    # note, deletes the device without waiting for whoever holds it,
    # and exits with status 3. The moment the failure holds is that
    # exit. From there the suite's requests are answered by the kernel
    # for a deleted device.
    for _ in $(seq 1 200); do kill -0 "$pid" 2>/dev/null || break; sleep 0.05; done
    kill -0 "$pid" 2>/dev/null && die "serve was still running 10 s after SIGUSR1"
    sleep 0.1
    rc=$(cat "$DIR.serve.rc" 2>/dev/null)
    [ "$rc" = 3 ] || die "serve exited ${rc:-with no status} after SIGUSR1, wanted 3"
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
pid)
    # The process to send SIGKILL to. Asked for once after create, kill
    # and reopen, and remembered, so that `kill` knows it without asking
    # a server that may be dead.
    pid=$(server_pid); [ -n "$pid" ] || die "no server pid"
    echo "$pid" > "$DIR.server.pid"
    echo "$pid"
    exit 0
    ;;
destroy)
    stop_serve
    lim 90 "$SKYZFSD" detach --dir "$DIR" --run-dir "$RUN" >&2
    find "$DIR" "$RUN" "$DIR.ready" "$DIR.serve.pid" "$DIR.serve.rc" -delete 2>/dev/null
    DEV=""
    ;;
*) echo "usage: blkcheck-helper.sh create|pid|kill|fail|reopen|destroy N" >&2; exit 2 ;;
esac
# The largest request the device takes, for the rig: the suite's cases
# about a 1 MiB request exercise the server only if this is 1024.
MAXKB=""; [ -z "$DEV" ] || MAXKB=$(cat "/sys/block/${DEV#/dev/}/queue/max_hw_sectors_kb" 2>/dev/null)
echo "$VERB $N $(( ($(date +%s%N) - T0) / 1000000 )) ms${DEV:+ $DEV max_hw_sectors_kb=$MAXKB}" >> "$WORK/helper.log"
[ -z "$DEV" ] || echo "$DEV"
exit 0

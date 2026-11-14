#!/bin/bash
# Tests of the daemon rig's refused-fetch courses (check e, M1.T37): the
# rig's own rf9_start, rf9_take and check_e_fetch, run against a made-up
# server that does what `serve --refuse-one-fetch` of a test-hooks build
# does (#87), and against servers wrong on purpose. No kernel, no ublk,
# no ZFS and no daemon are needed.
# Run: bash tests/kernel/test_daemon_fetch.sh
# The rig's functions are cut from it and eval'ed: they read the
# variables set here and call the functions made up here.
# shellcheck disable=SC2034,SC2329
HERE=$(cd "$(dirname "$0")" && pwd)
RIG=$HERE/daemon-rig.sh
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# shellcheck source=tests/kernel/looks.sh
. "$HERE/looks.sh"
if ! T=$(mktemp -d) || [ -z "$T" ] || [ ! -d "$T" ]; then echo "no temporary directory"; exit 2; fi
# The made-up server and zpool wait for this file: none outlives the test.
trap 'touch "$T/stop"; sleep 0.5; rm -rf "${T:?}"' EXIT
fails=0
is() { # name got want
    if [ "$2" = "$3" ]; then echo "ok   $1"; else echo "FAIL $1: got '$2', want '$3'"; fails=$((fails + 1)); fi
}
has() { # name got part
    case $2 in *"$3"*) echo "ok   $1" ;; *) echo "FAIL $1: '$3' not in '$2'"; fails=$((fails + 1)) ;; esac
}

eval "$(sed -n '/^EXIT_FAILED=/p; /^RF9_TAIL=/p; /^RF_PINS=/p' "$RIG")"
for f in rec pin now_ms serve_wait job_end rc_pin kill_cleanup check_e_fetch rf9_start rf9_cleanup rf9_take; do
    eval "$(sed -n "/^$f() .*}\$/{p;q}; /^$f() {/,/^}\$/p" "$RIG")"
    declare -F "$f" > /dev/null || { echo "FAIL the rig has no function $f"; exit 2; }
done
[ "$EXIT_START_GIVEN_UP" = 9 ] && [ -n "$RF9_TAIL" ] && [ -n "$RF_PINS" ] || { echo "FAIL the rig has no exit 9, RF9_TAIL or RF_PINS"; exit 2; }

OUT=$T/out; WORK=$T/work; mkdir -p "$OUT" "$WORK" "$T/bin"
R=$OUT/results.tsv; : > "$R"
SKYZFSD=skyzfsd; IMPL=daemon
export T

# The device: $T/state is none, live, quiesced (no server) or deleted;
# $T/srvpid the server that serves it. A server killed leaves it
# quiesced, as the driver does.
LINE_START="skyzfsd: the kernel refused the first fetch of tag 0 on queue 0 of device 9 (Invalid argument (os error 22)), and the device's start waits for every tag's fetch with no limit: serve gives it up; device 9, which this serve added and never started, is deleted; the volume is not failed; exit 9: a supervisor should retry the serve once; a second 9 is for a person"
LINE_TAKE="skyzfsd: the kernel refused the first fetch of tag 0 on queue 0 of device 9 (Invalid argument (os error 22)), and the end of its recovery waits for every tag's fetch with no limit: serve gives it up; device 9 is left waiting for the next server, as after a kill, with the I/O it holds; the volume is not failed; exit 9: a supervisor should retry the serve once; a second 9 is for a person"
export LINE_START LINE_TAKE
FS=$T/server
cat > "$FS" <<'FSEOF'
#!/bin/bash
# dir [--refuse-one-fetch start|recovery]; SCEN says how it goes wrong.
refuse=""
while [ $# -gt 0 ]; do case $1 in --refuse-one-fetch) refuse=$2; shift ;; esac; shift; done
was=$(cat "$T/state" 2>/dev/null || echo none)
# A device whose server was killed reads live in the file: it waits.
[ "$was" = live ] && ! alive && was=quiesced
kind=start; [ "$was" = quiesced ] && kind=recovery
if [ -n "$refuse" ] && [ "$refuse" = "$kind" ] && [ "$SCEN" != ready ]; then
    line=$LINE_START; [ "$kind" = recovery ] && line=$LINE_TAKE
    # The other kind's words.
    [ "$SCEN" != startwords ] || line=$LINE_TAKE
    [ "$SCEN" != takewords ] || line=$LINE_START
    : > "$T/gaveup"
    [ "$SCEN" != notail ] || line=${line%%; exit 9*}
    if [ "$kind" = start ]; then
        [ "$SCEN" = keepdev ] && echo quiesced > "$T/state" || echo deleted > "$T/state"
    elif [ "$SCEN" = releases ]; then
        # It answers the held I/O before it gives up.
        echo $$ > "$T/srvpid"; echo live > "$T/state"; sleep 1; echo quiesced > "$T/state"
    elif [ "$SCEN" = takedeletes ]; then
        echo deleted > "$T/state"
    fi
    echo "$line" >&2
    [ "$SCEN" != exit0 ] || exit 0
    exit 9
fi
[ "$SCEN" = takeoverfails ] && [ "$was" = quiesced ] && [ -z "$refuse" ] && { echo "skyzfsd: cannot take device 9 over" >&2; exit 1; }
[ "$SCEN" = nextfails ] && [ -e "$T/gaveup" ] && [ -z "$refuse" ] && { echo "skyzfsd: cannot serve" >&2; exit 1; }
echo $$ > "$T/srvpid"; echo live > "$T/state"
echo "skyzfsd: serving device 9 ($([ "$kind" = recovery ] && echo 'taken over' || echo added)) after 50 ms: start 5 ms" >&2
: > "$T/ready.$$"
trap 'echo quiesced > "$T/state"; exit 0' TERM
while [ ! -e "$T/stop" ]; do sleep 0.05; done
FSEOF
chmod +x "$FS"
alive() { local p; p=$(cat "$T/srvpid" 2>/dev/null) && [ -n "$p" ] && kill -0 "$p" 2>/dev/null; }
export -f alive
# zpool: a sync waits until the device has a live server, as a pool on a
# device with none waits.
cat > "$T/bin/zpool" <<'ZEOF'
#!/bin/bash
case $1 in
    sync) while :; do
              if [ "$(cat "$T/state" 2>/dev/null)" = live ] && alive; then
                  # After a give-up: the held sync fails.
                  [ "$SCEN" = ioerr ] && [ -e "$T/gaveup" ] && exit 1
                  exit 0
              fi
              [ ! -e "$T/stop" ] || exit 5
              sleep 0.05
          done ;;
    *) exit 0 ;;
esac
ZEOF
chmod +x "$T/bin/zpool"
PATH=$T/bin:$PATH
DD=$T/dd
cat > "$DD" <<'DDEOF'
#!/bin/bash
for a in "$@"; do case $a in if=/dev/ublkb9) [ "$(cat "$T/state")" = live ] && exit 0; exit 1 ;; esac; done
exec dd "$@"
DDEOF
chmod +x "$DD"

vol_create() { mkdir -p "$2"; }
vol_up() { vol_create "$@" && serve_start "$@"; }
serve_start() {
    local dir=$2; shift 2
    "$FS" "$dir" "$@" >> "$dir.ready" 2>> "$dir.serve.log" &
    SERVE_PID=$!; BDEV=/dev/ublkb9; SERVE_RC=""
    for _ in $(seq 1 100); do
        [ "$(cat "$T/state" 2>/dev/null)" = live ] && [ -e "$T/ready.$SERVE_PID" ] && return 0
        if ! kill -0 "$SERVE_PID" 2>/dev/null; then wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""; return 1; fi
        sleep 0.05
    done
    SERVE_RC=none; return 2
}
server_pid() { echo "$SERVE_PID"; }
vol_stat() {
    [ "$3" = state ] || return 0
    if [ "$SCEN" = failed ]; then echo failed; return; fi
    case $(cat "$T/state" 2>/dev/null) in live) if alive; then echo serving; else echo waiting; fi ;; quiesced) echo waiting ;; *) echo detached ;; esac
}
vol_detach() { echo deleted > "$T/state"; }
ublk_state() {
    case $(cat "$T/state" 2>/dev/null) in
        live) if alive; then echo LIVE; else echo QUIESCED; fi ;;
        quiesced) echo QUIESCED ;;
    esac
}
ublk_number_free() { case $(cat "$T/state" 2>/dev/null) in deleted|none) return 0 ;; *) return 1 ;; esac; }
new_slog() { echo /dev/loop9; }
pool_create() { mkdir -p "$WORK/mnt/$1"; }
pool_state() { echo ONLINE; }
pool_errors() { if [ "$SCEN" = poolbad ]; then echo "vdev errors 2 (read 0, write 2, checksum 0), scrub: none, errors: none"; else echo "vdev errors 0 (read 0, write 0, checksum 0), scrub: none, errors: No known data errors"; fi; }
course() { :; }
step() { :; }

run() { # scenario function name -> RV, RCT_WHY
    SCEN=$1; export SCEN
    rm -f "$T/state" "$T/srvpid" "$T"/ready.* "$T/gaveup"
    rm -rf "${WORK:?}/vol-$3" "${WORK:?}/vol-$3".*
    "$2" "$3" > /dev/null 2>&1; RV=$?
}

run ok rf9_start s-ok
is "at a start: PASS" "$RV" 0
has "  exit 9 and its line" "$RCT_WHY" "serve ended with exit 9 after"
has "  the words" "$RCT_WHY" "which this serve added and never started, is deleted"
has "  the number free" "$RCT_WHY" "device 9: its number was free"
has "  the next serve" "$RCT_WHY" "the next serve: skyzfsd: serving device 9 (added)"
has "  its read and stop" "$RCT_WHY" "a read of its first block exit 0, its stop exit 0"
run exit0 rf9_start s-exit0
is "  a start given up with 0 fails" "$RV" 1
run ready rf9_start s-ready
is "  a start that is not given up fails" "$RV" 1
has "  and says so" "$RCT_WHY" "became ready on /dev/ublkb9: no start was given up"
run keepdev rf9_start s-keep
is "  a device that is not deleted fails" "$RV" 1
has "  and says so" "$RCT_WHY" "device 9: STILL THERE 5 s after"
run notail rf9_start s-notail
is "  a line without what a supervisor should do fails" "$RV" 1
run startwords rf9_start s-words
is "  the take-over's words at a start fail" "$RV" 1
run nextfails rf9_start s-next
is "  a next serve that does not start fails" "$RV" 1
has "  says so" "$RCT_WHY" "the next serve did not become ready"
run failed rf9_start s-failed
is "  a volume that reads failed fails" "$RV" 1

run ok rf9_take t-ok
is "at a take-over: PASS" "$RV" 0
has "  exit 9 and its line" "$RCT_WHY" "the take-over ended with exit 9 after"
has "  the device waits, the I/O held" "$RCT_WHY" "the device then read QUIESCED; the volume's status: waiting; the pool's write and sync still held"
has "  the next serve takes it over" "$RCT_WHY" "the next serve: skyzfsd: serving device 9 (taken over)"
has "  and the held I/O completes" "$RCT_WHY" "the held write and sync then exited 0"
has "  the pool" "$RCT_WHY" "the pool ONLINE, vdev errors 0"
run exit0 rf9_take t-exit0
is "  a take-over given up with 0 fails" "$RV" 1
run ready rf9_take t-ready
is "  a take-over that is not given up fails" "$RV" 1
run releases rf9_take t-rel
is "  a server that answers the held I/O before it gives up fails" "$RV" 1
has "  says so" "$RCT_WHY" "the pool's write and sync had ended (0"
run takedeletes rf9_take t-del
is "  a take-over that deletes the device fails" "$RV" 1
run takeoverfails rf9_take t-tof
is "  a next serve that does not take over fails" "$RV" 1
has "  says so" "$RCT_WHY" "the next serve did not become ready"
run takewords rf9_take t-words
is "  the start's words at a take-over fail" "$RV" 1
run ioerr rf9_take t-ioerr
is "  held I/O that fails after the take-over fails" "$RV" 1
has "  says so" "$RCT_WHY" "the held write and sync then exited 1"
run poolbad rf9_take t-pool
is "  vdev errors fail" "$RV" 1
run notail rf9_take t-notail
is "  a line without what a supervisor should do fails" "$RV" 1
run failed rf9_take t-failed
is "  a volume that reads failed fails" "$RV" 1

# The check's wiring: two pins on the daemon, WAITING on the stand-in.
: > "$R"; SCEN=ok; export SCEN; rm -f "$T/state" "$T/srvpid"; rm -rf "${WORK:?}"/vol-e-rf-*
check_e_fetch > /dev/null 2>&1
for p in $RF_PINS; do is "the daemon: pin_$p" "$(awk -F'\t' -v k="pin_$p" '$1 == k { split($2, a, ":"); print a[1] }' "$R")" PASS; done
: > "$R"; IMPL=reference
check_e_fetch > /dev/null 2>&1
for p in $RF_PINS; do is "the stand-in: pin_$p" "$(awk -F'\t' -v k="pin_$p" '$1 == k { split($2, a, ":"); print a[1] }' "$R")" WAITING; done

if [ "$fails" = 0 ]; then echo "all passed"; else echo "$fails failed"; exit 1; fi

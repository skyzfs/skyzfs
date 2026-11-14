#!/bin/bash
# An early look at #87 (M1.3ah) on a kernel, for M1.T37: serve
# --refuse-one-fetch start and recovery on a test-hooks build. Not a rig:
# it records and judges nothing. Every wait has a limit.
# Run: sudo bash tests/adhoc/m1t37-look.sh SKYZFSD OUT
set -u
D=${1:?skyzfsd}; OUT=${2:?out}
W=/mnt/skz-look; mkdir -p "$W" "$OUT"
L=$OUT/look.txt; : > "$L"
say() { printf '%s\n' "$*" | tee -a "$L"; }
now_ms() { echo $(( $(date +%s%N) / 1000000 )); }
devs() { local d; d=$(find /dev -maxdepth 1 -name 'ublk[bc][0-9]*' 2>/dev/null | sort | tr '\n' ' '); echo "${d:-none}"; }
procs() { local p; p=$(pgrep -a skyzfsd | tr '\n' ';'); echo "${p:-none}"; }
state() { local s; s=$(timeout 10 ublk list -n "$1" 2>/dev/null | tr '\n' ' ' | sed -n 's/.*state \([A-Z_]*\).*/\1/p'); echo "${s:-none}"; }
# dir [serve options] -> SP, BDEV: a serve in the background, ready within 30 s
bg_serve() {
    local dir=$1 t0; shift
    : > "$dir.ready"
    t0=$(now_ms)
    "$D" serve --dir "$dir" "$@" > "$dir.ready" 2>> "$dir.err" &
    SP=$!; BDEV=""
    for _ in $(seq 1 120); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        [ -n "$BDEV" ] && [ -b "$BDEV" ] && { say "  ready $BDEV after $(( $(now_ms) - t0 )) ms (pid $SP)"; return 0; }
        kill -0 "$SP" 2>/dev/null || { wait "$SP"; say "  serve ended before ready: exit $?"; return 1; }
        sleep 0.25
    done
    say "  NOT READY in 30 s"; return 1
}
stop() { # end the serve SP: SIGTERM, 30 s, kill -9

    kill -TERM "$SP" 2>/dev/null
    for _ in $(seq 1 120); do kill -0 "$SP" 2>/dev/null || break; sleep 0.25; done
    if kill -0 "$SP" 2>/dev/null; then kill -9 "$SP"; say "  SIGTERM did not end it in 30 s: kill -9"; fi
    wait "$SP" 2>/dev/null; say "  stopped: exit $?"
}
# dir [serve options]: a serve in the foreground, limited to 90 s
fg_serve() {
    local dir=$1 t0 rc; shift
    t0=$(now_ms)
    timeout -k 5 90 "$D" serve --dir "$dir" "$@" > "$dir.fg.out" 2> "$dir.fg.err"; rc=$?
    say "  serve $* ended with exit $rc after $(( $(now_ms) - t0 )) ms$([ $rc = 124 ] && echo ' (TIMED OUT at 90 s: it did not end by itself)')"
    say "  its stderr: $(tr '\n' '|' < "$dir.fg.err" | cut -c1-1200)"
}

[ -e /dev/ublk-control ] || modprobe ublk_drv || { apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" > "$OUT/apt.log" 2>&1; modprobe ublk_drv; }
# The reference tool, for `ublk list` (the state of a device), as the rigs build it.
UBLKSRV_PATCH=$(dirname "$0")/../kernel/ublksrv-skz.patch bash "$(dirname "$0")/../kernel/build-ublksrv.sh" 6d0d922614ec18e6f2b5d476af8d540a1d9c1de8 > "$OUT/ublksrv.ver" 2> "$OUT/ublksrv-build.log" || echo "ublksrv build failed" >> "$L"
say "kernel $(uname -r); $("$D" help 2>&1 | grep -c refuse-one-fetch) help lines name --refuse-one-fetch"
dmesg -c > /dev/null 2>&1

say ""
say "== 1. serve --refuse-one-fetch start, on a new volume"
"$D" create --dir "$W/v1" --size $((1 << 30)) > /dev/null 2>> "$OUT/create.err" || say "  create failed"
say "  before: devices $(devs); skyzfsd processes $(procs)"
fg_serve "$W/v1" --refuse-one-fetch start
sleep 1
say "  1 s after: devices $(devs); skyzfsd processes $(procs)"
say "  status: $("$D" status --dir "$W/v1" 2>&1 | tr '\n' ' ' | cut -c1-600)"
say "  the next plain serve:"
if bg_serve "$W/v1"; then
    timeout 30 dd if="$BDEV" of=/dev/null bs=4k count=1 iflag=direct status=none; say "  a read of its first block: exit $?"
    say "  its line: $(grep 'serving device' "$W/v1.err" | tail -n 1 | cut -c1-400)"
    stop
fi
timeout 60 "$D" detach --dir "$W/v1" > /dev/null 2>&1

say ""
say "== 2. serve, kill -9, then take-over with --refuse-one-fetch recovery"
"$D" create --dir "$W/v2" --size $((1 << 30)) > /dev/null 2>> "$OUT/create.err" || say "  create failed"
if bg_serve "$W/v2"; then
    N=${BDEV#/dev/ublkb}
    kill -9 "$SP"; wait "$SP" 2>/dev/null; say "  killed -9: exit $?"
    for _ in $(seq 1 80); do [ "$(state "$N")" = QUIESCED ] && break; sleep 0.25; done
    say "  the device then: $(state "$N"); devices $(devs)"
    # A read held while no server serves.
    ( t0=$(now_ms); timeout 300 dd if="$BDEV" of=/dev/null bs=4k count=1 skip=1000 iflag=direct status=none; echo "$? $(( $(now_ms) - t0 ))" > "$W/rd.rc" ) &
    RD=$!
    sleep 1
    say "  a direct read issued with no server: $([ -e "$W/rd.rc" ] && echo "already ended: $(cat "$W/rd.rc")" || echo waits)"
    fg_serve "$W/v2" --refuse-one-fetch recovery
    sleep 1
    say "  1 s after: the device $(state "$N"); devices $(devs); skyzfsd processes $(procs); the read $([ -e "$W/rd.rc" ] && echo "ended: $(cat "$W/rd.rc")" || echo "still waits")"
    say "  status: $("$D" status --dir "$W/v2" 2>&1 | tr '\n' ' ' | cut -c1-600)"
    say "  the next plain serve:"
    if bg_serve "$W/v2"; then
        say "  its line: $(grep 'serving device' "$W/v2.err" | tail -n 1 | cut -c1-400)"
        for _ in $(seq 1 120); do [ -e "$W/rd.rc" ] && break; sleep 0.25; done
        say "  the held read: $([ -e "$W/rd.rc" ] && echo "exit and ms: $(cat "$W/rd.rc")" || echo "STILL WAITS 30 s after the take-over")"
        stop
    fi
    kill "$RD" 2>/dev/null
fi
timeout 60 "$D" detach --dir "$W/v2" > /dev/null 2>&1

say ""
say "== 3. plain starts, timed (the start on its own thread)"
"$D" create --dir "$W/v3" --size $((1 << 30)) > /dev/null 2>> "$OUT/create.err" || say "  create failed"
for _ in 1 2 3 4 5; do
    if bg_serve "$W/v3"; then
        say "  $(grep 'serving device' "$W/v3.err" | tail -n 1 | grep -o 'start [0-9]* ms' )"
        stop
    fi
    timeout 60 "$D" detach --dir "$W/v3" > /dev/null 2>&1
done

say ""
say "== at the end: devices $(devs); skyzfsd processes $(procs)"
dmesg > "$OUT/dmesg.txt" 2>&1
cp "$W"/*.err "$W"/*.out "$OUT/" 2>/dev/null
exit 0

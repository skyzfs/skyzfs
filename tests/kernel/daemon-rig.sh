#!/usr/bin/env bash
# The kernel rig for skyzfsd (design §4.1, §4.2; work package M1.T4).
# It drives a server through the daemon's command line and records what
# the kernel and ZFS do, one "key<TAB>value" line per finding in
# OUT/results.tsv. `tests/kernel/daemon_check.py` then compares the
# results with its table `EXPECTED` and renders the summary; this script
# judges nothing beyond writing down what each stage saw.
#
#   daemon-rig.sh OUT [WORK]
#
# The server is one variable: RIG_SKYZFSD, the path of a `skyzfsd`. Left
# unset, the rig uses tests/kernel/skyzfsd-ref.sh, a stand-in built on
# the reference ublk server. The deliberately wrong servers of the
# controls are always the stand-in.
#
# Meant for a throwaway machine (a GitHub runner): it installs packages,
# builds a ublk server, creates pools named skzd-*, and leaves suspended
# pools behind. Needs root.
#
# Lines written:
#   pin_<name>   PASS | FAIL | ERROR, then ": detail". A property the
#                design depends on. ERROR is the rig's own failure (a
#                step that judges nothing went wrong) and is never a
#                verdict on the server.
#   ctl_<name>   the same check run against a server that is wrong on
#                purpose, or with the fault left out. Expected FAIL.
#   obs_<name>   an observation: what happened, not judged.
#   anything else: context.
#
# Checks, chosen with RIG_CHECKS (default "a b d e"):
#   a  parameters of the device and of a pool on it
#   b  kill -9 under load, RIG_KILLS times (default 20); a second serve;
#      an orderly stop
#   d  a failed volume: what ZFS does, and what the server refuses after
#   e  one request held RIG_HOLD_S seconds (default 300)
# Check c, the conformance suite through the block device, is a separate
# tool and is not in this script yet.
set -uo pipefail

# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

OUT=${1:?usage: daemon-rig.sh OUT [WORK]}
WORK=${2:-/mnt/skz-daemon-rig}
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)
R=$OUT/results.tsv
: > "$R"
export DEBIAN_FRONTEND=noninteractive

REF=$HERE/skyzfsd-ref.sh
SKYZFSD=${RIG_SKYZFSD:-$REF}
IMPL=daemon; [ "$SKYZFSD" = "$REF" ] && IMPL=reference
CHECKS=" ${RIG_CHECKS:-a b d e} "
KILLS=${RIG_KILLS:-20}
HOLD_S=${RIG_HOLD_S:-300}
RUN_DIR=$WORK/run            # the server's --run-dir, so that a test can empty it
VOL_BYTES=$((8 << 30))
SLOG_MB=512

# ------------------------------------------------------------ recording
rec() { # key value. Tabs and newlines in the value are flattened.
    local v=${2//$'\t'/ }
    v=${v//$'\n'/; }
    printf '%s\t%s\n' "$1" "$v" >> "$R"
    printf '== %s: %s\n' "$1" "$v"
}
pin() { rec "pin_$1" "$2: $3"; }     # name PASS|FAIL|ERROR detail
ctl() { rec "ctl_$1" "$2: $3"; }
pin_is() { # name detail test...: PASS if the test holds, else FAIL
    local name=$1 detail=$2; shift 2
    if "$@"; then pin "$name" PASS "$detail"; else pin "$name" FAIL "$detail"; fi
}
obs() { rec "obs_$1" "$2"; }
step() { printf '\n######## %s\n' "$*"; }
lim() { local s=$1; shift; timeout -k 5 "$s" "$@"; }
now_ms() { echo $(( $(date +%s%N) / 1000000 )); }
dmesg_mark() { dmesg | wc -l; }
dmesg_since() { dmesg | tail -n +"$(($1 + 1))"; }

# --------------------------------------------- the server, by its command line
# Everything that depends on the daemon's command line is in these
# functions and in skyzfsd-ref.sh.
EXIT_FAILED=3; EXIT_REFUSED_FAILED=4; EXIT_BUSY=5
SERVE_PID=""; BDEV=""; SERVE_RC=""
vol_create() { lim 60 "$1" create --dir "$2" --size "$VOL_BYTES"; }
vol_up() { # impl dir [serve arguments]: create a volume and serve it
    vol_create "$1" "$2" || return 1
    serve_start "$@"
}
# impl dir [serve arguments]: start `serve` in the background and wait
# for its "ready" line. 0 with BDEV and SERVE_PID set; 1 if it exited
# first, with SERVE_RC; 2 if neither happened in time.
serve_start() {
    local impl=$1 dir=$2 i; shift 2
    : > "$dir.ready"
    "$impl" serve --dir "$dir" --run-dir "$RUN_DIR" "$@" > "$dir.ready" 2>> "$dir.serve.log" &
    SERVE_PID=$!; BDEV=""; SERVE_RC=""
    for i in $(seq 1 480); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        [ -n "$BDEV" ] && [ -b "$BDEV" ] && return 0
        if ! kill -0 "$SERVE_PID" 2>/dev/null; then
            wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""
            return 1
        fi
        sleep 0.25
    done
    return 2
}
serve_wait() { # seconds: wait for the `serve` started last to exit; its status in SERVE_RC
    local i
    for i in $(seq 1 $(($1 * 4))); do
        if ! kill -0 "$SERVE_PID" 2>/dev/null; then
            wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""
            return 0
        fi
        sleep 0.25
    done
    return 1
}
vol_status() { lim 30 "$1" status --dir "$2" 2>/dev/null; }      # impl dir
vol_stat() { vol_status "$1" "$2" | awk -v k="$3" '$1 == k { print $2; exit }'; }
# The process to kill -9: the one `status` names, else the one started.
server_pid() { local p; p=$(vol_stat "$1" "$2" pid); echo "${p:-$SERVE_PID}"; }
vol_detach() { lim 90 "$1" detach --dir "$2" >/dev/null 2>&1; }
ublk_state() { lim 20 ublk list -n "${1#/dev/ublkb}" 2>/dev/null | sed -n 's/.*state \([A-Z_]*\).*/\1/p' | head -n 1; }

# ------------------------------------------------------------ pools
SLOGS=()
new_slog() { # name -> loop device of a written-through file, the local log device (design §6.1)
    local img=$WORK/$1.slog loop
    truncate -s "${SLOG_MB}M" "$img"
    "$DD" if=/dev/zero of="$img" bs=1M count="$SLOG_MB" oflag=direct conv=notrunc,fsync status=none
    loop=$(losetup --find --show --direct-io=on "$img" 2>/dev/null || losetup --find --show "$img")
    SLOGS+=("$loop"); echo "$loop"
}
pool_create() { # pool bdev slog [zpool create options]
    local pool=$1 bdev=$2 slog=$3; shift 3
    mkdir -p "$WORK/mnt/$pool"
    lim 180 zpool create -f -o ashift=12 -o cachefile=none -o failmode=wait "$@" \
        -O mountpoint="$WORK/mnt/$pool" -O compression=off -O atime=off "$pool" "$bdev" log "$slog"
}
pool_state() { # ONLINE, SUSPENDED, ...: from the kernel, which answers when zpool would block
    cat "/proc/spl/kstat/zfs/$1/state" 2>/dev/null || echo unknown
}
pool_errors() { # "vdev errors N (read r, write w, checksum c), <scan line>, <errors line>"
    lim 60 zpool status -p "$1" > "$OUT/status.tmp" 2>&1 || { echo "zpool status failed or timed out"; return; }
    awk '$2 ~ /^(ONLINE|DEGRADED|FAULTED|UNAVAIL|REMOVED)$/ && NF >= 5 { r += $3; w += $4; c += $5 }
         /scan:/ { sub(/^ *scan: */, ""); scan = $0 }
         /^errors:/ { sub(/^errors: */, ""); err = $0 }
         END { printf "vdev errors %d (read %d, write %d, checksum %d), %s, %s", r + w + c, r, w, c, scan, err }' "$OUT/status.tmp"
}
# A writer with its own record of what was acknowledged: 16 files of
# 4 MiB, rewritten in turn, each with an fsync. A line "name sha256" is
# appended only after dd returned success, so the last line for a name
# is what the file must hold. Failures go to LOG. Stops when STOP exists.
writer() { # mountpoint record log stopfile
    local mnt=$1 record=$2 log=$3 stop=$4 n=0 name sum
    while [ ! -e "$stop" ]; do
        name=r.$((n % 16))
        "$DD" if=/dev/urandom of="/dev/shm/skz-writer.$$" bs=1M count=4 status=none
        sum=$(sha256sum < "/dev/shm/skz-writer.$$" | cut -d' ' -f1)
        if "$DD" if="/dev/shm/skz-writer.$$" of="$mnt/$name" bs=1M conv=fsync status=none 2>> "$log"; then
            echo "$name $sum" >> "$record"
        else
            echo "write $n of $name failed" >> "$log"
        fi
        n=$((n + 1))
    done
    unlink "/dev/shm/skz-writer.$$" 2>/dev/null
}
files_match() { # mountpoint record -> "ok N files" or "MISMATCH ..."
    python3 -I - "$1" "$2" <<'PY'
import hashlib, sys
want = {}
for line in open(sys.argv[2]):
    name, digest = line.split()
    want[name] = digest
bad = []
for name, digest in sorted(want.items()):
    try:
        got = hashlib.sha256(open(f'{sys.argv[1]}/{name}', 'rb').read()).hexdigest()
    except OSError as e:
        got = f'unreadable ({e.strerror})'
    if got != digest:
        bad.append(name)
print(f'ok {len(want)} files' if want and not bad else f'MISMATCH {len(bad)} of {len(want)}: {" ".join(bad)}')
PY
}

# shellcheck disable=SC2317  # reached through the trap
cleanup() {
    set +e
    touch "$WORK/stop-all"
    local p
    for p in $(lim 30 zpool list -H -o name 2>/dev/null | grep '^skzd-'); do
        # A suspended pool cannot be exported; trying would block.
        [ "$(pool_state "$p")" = ONLINE ] && lim 60 zpool export -f "$p" 2>/dev/null
    done
    command -v ublk >/dev/null && lim 60 ublk del -a --async 2>/dev/null
    for p in "${SLOGS[@]}"; do losetup -d "$p" 2>/dev/null; done
    dmesg > "$OUT/dmesg.txt" 2>/dev/null
    chmod -R a+rX "$OUT"
}
trap cleanup EXIT

# ------------------------------------------------------------ setup
step "setup"
rec impl "$IMPL: $SKYZFSD"
rec kernel "$(uname -r)"
# shellcheck disable=SC1091
rec os "$(. /etc/os-release && echo "$PRETTY_NAME")"
SETUP_OK=1
apt-get update > /dev/null 2>&1
apt-get install -y --no-install-recommends zfsutils-linux python3 util-linux > "$OUT/apt.log" 2>&1 || SETUP_OK=""
[ -e /dev/ublk-control ] || modprobe ublk_drv 2>> "$OUT/apt.log" || {
    apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/apt.log" 2>&1
    modprobe ublk_drv 2>> "$OUT/apt.log" || SETUP_OK=""
}
modprobe zfs 2>> "$OUT/apt.log" || SETUP_OK=""
rec zfs "$(cat /sys/module/zfs/version 2>/dev/null)"
if UBLKSRV_PATCH=$HERE/ublksrv-skz.patch "$HERE/build-ublksrv.sh" \
    "${UBLKSRV_REF:-6d0d922614ec18e6f2b5d476af8d540a1d9c1de8}" > "$OUT/ublksrv.ver" 2> "$OUT/ublksrv-build.log"; then
    rec ublk_server "$(tail -n 1 "$OUT/ublksrv.ver"), with ublksrv-skz.patch"
else
    rec ublk_server "BUILD FAILED: $(tail -n 1 "$OUT/ublksrv-build.log")"
    tail -n 60 "$OUT/ublksrv-build.log"
    SETUP_OK=""
fi
rec zed "$(systemctl is-active zfs-zed 2>/dev/null || echo unknown)"
mkdir -p "$RUN_DIR" "$WORK/mnt"
if [ -z "$SETUP_OK" ]; then
    rec setup "FAILED: see apt.log and ublksrv-build.log"
    exit 2
fi
rec setup ok

# =================================================================== a
check_a() {
    local dir=$WORK/vol-a pool=skzd-a q slog v f0 f1 label
    step "a: parameters"
    if ! vol_up "$SKYZFSD" "$dir"; then
        for v in block_size max_request write_cache discard write_zeroes no_fua label_at_byte_0 flush_counted; do
            pin "$v" ERROR "the volume was not created or served (serve rc ${SERVE_RC:-none})"
        done
        return
    fi
    q=/sys/block/$(basename "$BDEV")/queue
    rec a_queue "$(for v in logical_block_size physical_block_size max_sectors_kb max_hw_sectors_kb nr_requests \
        discard_max_bytes discard_granularity write_zeroes_max_bytes write_cache fua; do
        [ -r "$q/$v" ] && printf '%s=%s ' "$v" "$(cat "$q/$v")"; done)"
    rec a_status "$(vol_status "$SKYZFSD" "$dir")"
    v="logical $(cat "$q/logical_block_size"), physical $(cat "$q/physical_block_size")"
    pin_is block_size "$v" [ "$v" = "logical 4096, physical 4096" ]
    v=$(cat "$q/max_hw_sectors_kb")
    pin_is max_request "max_hw_sectors_kb $v, wanted 1024" [ "$v" = 1024 ]
    v=$(cat "$q/write_cache")
    pin_is write_cache "$v" [ "$v" = "write back" ]
    v=$(cat "$q/discard_max_bytes")
    pin_is discard "discard_max_bytes $v" [ "$v" -gt 0 ]
    v=$(cat "$q/write_zeroes_max_bytes")
    pin_is write_zeroes "write_zeroes_max_bytes $v" [ "$v" -gt 0 ]
    v=$(cat "$q/fua")
    pin_is no_fua "fua $v (1 would mean forced-unit-access writes are advertised)" [ "$v" = 0 ]

    slog=$(new_slog a)
    if ! pool_create "$pool" "$BDEV" "$slog" > "$OUT/a-create.log" 2>&1; then
        pin label_at_byte_0 ERROR "zpool create failed: $(tail -n 1 "$OUT/a-create.log")"
        pin flush_counted ERROR "no pool"
        kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
        vol_detach "$SKYZFSD" "$dir"
        return
    fi
    lim 60 "$DD" if=/dev/urandom of="$WORK/mnt/$pool/f" bs=1M count=64 conv=fsync status=none
    lim 120 zpool sync "$pool"
    # Read through the device: byte N of the device is where the label is.
    label=$(python3 -I - "$BDEV" <<'PY'
import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
nv = os.pread(fd, 4, 16 << 10)
slots = [o for o in range(128 << 10, 256 << 10, 4096)
         if os.pread(fd, 8, o) in (b'\x0c\xb1\xba\x00\x00\x00\x00\x00', b'\x00\x00\x00\x00\x00\xba\xb1\x0c')]
print(('PASS' if nv[:2] == b'\x01\x01' and slots else 'FAIL') +
      f": nvlist header {nv.hex()} at byte 16384, {len(slots)} uberblocks in the ring at bytes 131072 to 262143")
PY
    )
    if [ -b "${BDEV}p1" ]; then
        pin label_at_byte_0 FAIL "zpool create partitioned $BDEV"
    else
        pin label_at_byte_0 "${label%%:*}" "no partition on $BDEV; ${label#*: }"
    fi
    # Flushes, counted by the server.
    f0=$(vol_stat "$SKYZFSD" "$dir" flushes)
    lim 60 "$DD" if=/dev/urandom of="$WORK/mnt/$pool/g" bs=1M count=8 status=none
    lim 120 zpool sync "$pool"
    f1=$(vol_stat "$SKYZFSD" "$dir" flushes)
    if [ -z "$f0" ] || [ -z "$f1" ]; then
        pin flush_counted ERROR "status gave no flush counter"
    elif [ $((f1 - f0)) -ge 2 ]; then
        pin flush_counted PASS "the server counted $((f1 - f0)) flushes during one zpool sync"
    else
        pin flush_counted FAIL "the server counted $((f1 - f0)) flushes during one zpool sync"
    fi
    lim 120 zpool export "$pool"
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
}

# =================================================================== b
# impl name kills [environment for the stand-in]: a pool under a writer,
# the server killed `kills` times with writes in flight and served
# again. Sets KT_* for the caller to judge.
kill_test() {
    local impl=$1 name=$2 kills=$3 dir=$WORK/vol-$2 pool=skzd-$2 mnt slog n i w pid inflight rc st ms t0
    mnt=$WORK/mnt/$pool
    KT_ERR=""; KT_DONE=0; KT_INFLIGHT=0; KT_FAILED_WRITES=""; KT_VDEV=""; KT_SCRUB=""; KT_FILES=""; KT_ROUNDS=""
    vol_up "$impl" "$dir" || { KT_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return 1; }
    slog=$(new_slog "$name")
    pool_create "$pool" "$BDEV" "$slog" > "$OUT/$name-create.log" 2>&1 || { KT_ERR="zpool create failed"; return 1; }
    inflight=/sys/block/$(basename "$BDEV")/inflight
    : > "$WORK/$name.record"; : > "$OUT/$name-writer.log"
    [ ! -e "$WORK/$name.stop" ] || unlink "$WORK/$name.stop"
    writer "$mnt" "$WORK/$name.record" "$OUT/$name-writer.log" "$WORK/$name.stop" &
    KT_WRITER=$!
    # An fsync is served by the log device alone; syncing the pool keeps
    # writes to the data vdev in flight for a kill to land on.
    ( while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do zpool sync "$pool" 2>/dev/null; sleep 0.1; done ) &
    KT_SYNCER=$!
    sleep 5
    for n in $(seq 1 "$kills"); do
        pid=$(server_pid "$impl" "$dir")
        [ -n "$pid" ] || { KT_ERR="kill $n: no server pid"; break; }
        sleep "0.$((RANDOM % 10))"
        w=0
        for i in $(seq 1 500); do
            read -r _ w < "$inflight" 2>/dev/null || break
            [ "${w:-0}" -gt 0 ] && break
            sleep 0.01
        done
        kill -9 "$pid" 2>/dev/null || { KT_ERR="kill $n: kill -9 $pid failed"; break; }
        [ "${w:-0}" -gt 0 ] && KT_INFLIGHT=$((KT_INFLIGHT + 1))
        [ -z "$SERVE_PID" ] || serve_wait 30 || kill -9 "$SERVE_PID" 2>/dev/null
        st=$(ublk_state "$BDEV")
        t0=$(now_ms)
        serve_start "$impl" "$dir"; rc=$?
        ms=$(( $(now_ms) - t0 ))
        KT_ROUNDS+="kill $n: ${w:-0} writes in flight, $st, served again rc $rc in $ms ms; "
        [ $rc = 0 ] || { KT_ERR="kill $n: serve did not come back (rc $rc, serve exit ${SERVE_RC:-none})"; break; }
        KT_DONE=$n
        sleep 2
    done
    touch "$WORK/$name.stop"
    i=0; while kill -0 "$KT_WRITER" 2>/dev/null && [ $i -lt 120 ]; do sleep 1; i=$((i + 1)); done
    if kill -0 "$KT_WRITER" 2>/dev/null; then
        kill -9 "$KT_WRITER" 2>/dev/null
        KT_FAILED_WRITES=stuck
    else
        KT_FAILED_WRITES=$(grep -c failed "$OUT/$name-writer.log" || true)
    fi
    kill -9 "$KT_SYNCER" 2>/dev/null; pkill -9 -f "zpool sync $pool" 2>/dev/null
    rec "${name}_rounds" "${KT_ROUNDS:-none}"
    # The pool as the kills left it, before anything clears it.
    KT_STATE=$(pool_state "$pool")
    KT_VDEV=$(pool_errors "$pool")
    [ "$KT_STATE" = ONLINE ] || lim 120 zpool clear "$pool" > /dev/null 2>&1
    lim 900 zpool scrub -w "$pool" > "$OUT/$name-scrub.log" 2>&1
    KT_SCRUB=$(pool_errors "$pool")
    KT_FILES=$(files_match "$mnt" "$WORK/$name.record")
    rec "${name}_result" "$KT_DONE of $kills kills served again, $KT_INFLIGHT with writes in flight; failed writes $KT_FAILED_WRITES; pool $KT_STATE, $KT_VDEV; after scrub: $KT_SCRUB; files: $KT_FILES"
    [ -z "$KT_ERR" ]
}
# The four judgments of a kill test, as pins or as one control line.
kill_verdict() { # -> PASS | FAIL | ERROR on stdout, the reason in KT_WHY
    KT_WHY=""
    if [ -n "$KT_ERR" ]; then KT_WHY=$KT_ERR; echo ERROR; return; fi
    if [ "$KT_INFLIGHT" -lt $((KT_DONE / 2)) ]; then
        KT_WHY="only $KT_INFLIGHT of $KT_DONE kills landed with writes in flight"; echo ERROR; return
    fi
    [ "$KT_FAILED_WRITES" = 0 ] || KT_WHY+="application writes failed: $KT_FAILED_WRITES; "
    case "$KT_STATE/$KT_VDEV" in "ONLINE/vdev errors 0 "*) ;; *) KT_WHY+="after the kills: pool $KT_STATE, $KT_VDEV; " ;; esac
    case "$KT_SCRUB" in "vdev errors 0 "*"with 0 errors"*) ;; *) KT_WHY+="scrub: $KT_SCRUB; " ;; esac
    case "$KT_FILES" in ok*) ;; *) KT_WHY+="files: $KT_FILES; " ;; esac
    if [ -z "$KT_WHY" ]; then echo PASS; else echo FAIL; fi
}
check_b() {
    local dir=$WORK/vol-b pool=skzd-b v pid0 pid1 rc st i
    step "b: kill -9 under load, $KILLS times"
    kill_test "$SKYZFSD" b "$KILLS"
    v=$(kill_verdict)
    pin kill_under_load "$v" "${KT_WHY:-$KT_DONE kills, $KT_INFLIGHT with writes in flight: no application write failed, no error on the vdev, scrub clean, $KT_FILES}"
    if [ -n "$KT_ERR" ] || [ -z "$BDEV" ]; then
        pin second_serve_refused ERROR "the kill test did not leave a served pool"
        pin orderly_stop_keeps_device ERROR "the kill test did not leave a served pool"
    else
        step "b: a second serve while the first is alive"
        pid0=$(server_pid "$SKYZFSD" "$dir")
        lim 60 "$SKYZFSD" serve --dir "$dir" --run-dir "$RUN_DIR" > "$OUT/b-second.out" 2> "$OUT/b-second.err"; rc=$?
        pid1=$(server_pid "$SKYZFSD" "$dir")
        if [ $rc = 124 ] || [ $rc = 137 ]; then
            pin second_serve_refused FAIL "a second serve did not exit within 60 s"
        elif [ $rc = "$EXIT_BUSY" ] && [ "$pid0" = "$pid1" ] && kill -0 "$pid0" 2>/dev/null \
            && [ "$(pool_state "$pool")" = ONLINE ] && lim 60 "$DD" if=/dev/zero of="$WORK/mnt/$pool/after-second" bs=1M count=4 conv=fsync status=none; then
            pin second_serve_refused PASS "exit $rc; the first server ($pid0) and the pool carried on"
        else
            pin second_serve_refused FAIL "exit $rc (wanted $EXIT_BUSY); server pid $pid0 then $pid1; pool $(pool_state "$pool"); $(tail -n 1 "$OUT/b-second.err")"
        fi

        step "b: an orderly stop leaves the device for the next serve"
        kill -TERM "$SERVE_PID" 2>/dev/null
        if ! serve_wait 60; then
            pin orderly_stop_keeps_device FAIL "serve did not exit within 60 s of SIGTERM"
        else
            rc=$SERVE_RC; st=$(ublk_state "$BDEV")
            ( lim 120 "$DD" if=/dev/zero of="$WORK/mnt/$pool/across-stop" bs=1M count=4 conv=fsync status=none; echo $? > "$WORK/b-across.rc" ) &
            sleep 3
            serve_start "$SKYZFSD" "$dir"; i=$?
            wait
            if [ "$rc" = 0 ] && [ -b "$BDEV" ] && [ "$st" = QUIESCED ] && [ $i = 0 ] && [ "$(cat "$WORK/b-across.rc")" = 0 ] \
                && [ "$(pool_state "$pool")" = ONLINE ]; then
                pin orderly_stop_keeps_device PASS "serve exited 0 on SIGTERM, the device stayed ($st), and a write begun while no server ran completed after serve returned; $(pool_errors "$pool")"
            else
                pin orderly_stop_keeps_device FAIL "serve exit $rc on SIGTERM, device state '$st', serve again rc $i, write across the stop rc $(cat "$WORK/b-across.rc"), pool $(pool_state "$pool")"
            fi
        fi
    fi
    [ "$(pool_state "$pool")" = ONLINE ] && lim 120 zpool export "$pool"
    [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
    vol_detach "$SKYZFSD" "$dir"

    step "b, control: recovery without reissue"
    SKZ_REF_REISSUE=0 kill_test "$REF" b0 5
    v=$(kill_verdict)
    ctl kill_without_reissue "$v" "${KT_WHY:-nothing went wrong in $KT_DONE kills}"
    [ "$(pool_state skzd-b0)" = ONLINE ] && lim 120 zpool export skzd-b0
    [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
    vol_detach "$REF" "$WORK/vol-b0"
}

# =================================================================== d
# A pool whose reads reach the device (no data cache), with files on it,
# a reader and a writer running. name impl [serve arguments].
fail_setup() {
    local name=$1 impl=$2 dir=$WORK/vol-$1 pool=skzd-$1 mnt slog i; shift 2
    mnt=$WORK/mnt/$pool
    FS_ERR=""
    vol_up "$impl" "$dir" "$@" || { FS_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return 1; }
    slog=$(new_slog "$name")
    pool_create "$pool" "$BDEV" "$slog" -O primarycache=metadata > "$OUT/$name-create.log" 2>&1 || { FS_ERR="zpool create failed"; return 1; }
    for i in 0 1 2 3; do
        lim 120 "$DD" if=/dev/urandom of="$mnt/base.$i" bs=1M count=32 conv=fsync status=none || { FS_ERR="writing the first files failed"; return 1; }
    done
    lim 120 zpool sync "$pool"
    (cd "$mnt" && sha256sum base.* > "$WORK/$name.sums")
    : > "$OUT/$name-reader.log"; : > "$OUT/$name-writer.log"; : > "$WORK/$name.record"
    [ ! -e "$WORK/$name.stop" ] || unlink "$WORK/$name.stop"
    # The reader: any error it gets is an application seeing a read fail.
    ( n=0
      while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do
          cat "$mnt/base.$((n % 4))" > /dev/null 2>> "$OUT/$name-reader.log" || echo "read $n failed" >> "$OUT/$name-reader.log"
          n=$((n + 1)); echo "$n" > "$WORK/$name.reads"
      done ) &
    FS_READER=$!
    writer "$mnt" "$WORK/$name.record" "$OUT/$name-writer.log" "$WORK/$name.stop" &
    FS_WRITER=$!
    ( while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do lim 30 zpool sync "$pool" 2>/dev/null; sleep 0.2; done ) &
    FS_SYNCER=$!
}
# name seconds: wait for the pool to suspend. Records what ZFS and the
# kernel said. -> PASS (suspended, no application error), FAIL, in FS_WHY.
fail_wait() {
    local name=$1 secs=$2 pool=skzd-$1 i st rerr werr mark=$3
    for i in $(seq 1 "$secs"); do
        st=$(pool_state "$pool"); [ "$st" = SUSPENDED ] && break
        sleep 1
    done
    sleep 3
    rerr=$(grep -c . "$OUT/$name-reader.log" || true)
    werr=$(grep -c failed "$OUT/$name-writer.log" || true)
    lim 30 zpool status -v "$pool" > "$OUT/$name-status-suspended.log" 2>&1
    dmesg_since "$mark" > "$OUT/$name-dmesg.log"
    rec "${name}_zpool_status" "$(grep -E 'state:|status:|errors:|ublkb|SUSPENDED|UNAVAIL|FAULTED|REMOVED' "$OUT/$name-status-suspended.log" | tr -s ' ' | tr '\n' ';')"
    rec "${name}_kernel_log" "$(grep -ciE 'I/O error|critical' "$OUT/$name-dmesg.log") lines with an I/O error; ZFS said: $(grep -iE 'zio|pool|WARNING' "$OUT/$name-dmesg.log" | sed 's/^\[[^]]*\] //' | sort | uniq -c | sort -rn | head -n 4 | tr -s ' ' | tr '\n' ';')"
    rec "${name}_events" "$(lim 30 zpool events -H 2>/dev/null | awk '{ c[$NF]++ } END { for (k in c) printf "%s x%d ", k, c[k] }')"
    FS_WHY="pool $st after at most ${secs} s; reads done $(cat "$WORK/$name.reads" 2>/dev/null), read errors seen by the reader: $rerr; writes failed for the writer: $werr"
    [ "$st" = SUSPENDED ] && [ "$rerr" = 0 ] && [ "$werr" = 0 ]
}
fail_stop() { # name: stop the helpers of fail_setup; they may be blocked on a suspended pool
    touch "$WORK/$1.stop"
    kill -9 "$FS_READER" "$FS_WRITER" "$FS_SYNCER" 2>/dev/null
}
check_d() {
    local mark rc n v dir st0 st1 before after
    # d.1, first form: every request answered with a plain I/O error.
    step "d: every request fails with an I/O error (the device stays)"
    mark=$(dmesg_mark)
    if SKZ_REF_FAIL_STYLE=eio fail_setup d1 "$REF" --fail-log-after 8000; then
        if fail_wait d1 300 "$mark"; then pin suspends_on_io_errors PASS "$FS_WHY"; else pin suspends_on_io_errors FAIL "$FS_WHY"; fi
        rec d1_server "$(vol_status "$REF" "$WORK/vol-d1" | tr '\n' ';')"
    else
        pin suspends_on_io_errors ERROR "$FS_ERR"
    fi
    fail_stop d1

    # The control: the same wait on a volume that does not fail.
    step "d, control: a volume that does not fail"
    mark=$(dmesg_mark)
    if fail_setup d0 "$REF"; then
        if fail_wait d0 30 "$mark"; then ctl volume_that_does_not_fail PASS "$FS_WHY"; else ctl volume_that_does_not_fail FAIL "$FS_WHY"; fi
    else
        ctl volume_that_does_not_fail ERROR "$FS_ERR"
    fi
    fail_stop d0
    sleep 2
    [ "$(pool_state skzd-d0)" = ONLINE ] && lim 120 zpool export -f skzd-d0
    [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
    vol_detach "$REF" "$WORK/vol-d0"

    # d.1, second form, with the server under test: the log fails, the
    # daemon deletes its device and exits.
    step "d: the volume fails and its device is deleted"
    mark=$(dmesg_mark)
    dir=$WORK/vol-d2
    if fail_setup d2 "$SKYZFSD" --fail-log-after 8000; then
        if fail_wait d2 300 "$mark"; then pin suspends_on_deleted_device PASS "$FS_WHY"; else pin suspends_on_deleted_device FAIL "$FS_WHY"; fi
        if serve_wait 30; then
            pin_is failed_exit_status "serve exited $SERVE_RC, wanted $EXIT_FAILED" [ "$SERVE_RC" = "$EXIT_FAILED" ]
        else
            pin failed_exit_status FAIL "serve was still running 30 s after the pool suspended"
        fi
        if [ -b "$BDEV" ]; then rec d2_device "still there after the failure"; else rec d2_device "gone after the failure"; fi
        # d.6: the pool held the device at the failure, so the volume is
        # refused in this boot, whatever the operator says.
        lim 60 "$SKYZFSD" serve --dir "$dir" --run-dir "$RUN_DIR" > /dev/null 2> "$OUT/d2-again.err"; rc=$?
        lim 60 "$SKYZFSD" serve --dir "$dir" --run-dir "$RUN_DIR" --acknowledge-failure > /dev/null 2> "$OUT/d2-again-ack.err"; n=$?
        if [ $rc = "$EXIT_REFUSED_FAILED" ] && [ $n = "$EXIT_REFUSED_FAILED" ]; then
            pin failed_held_volume_refused PASS "serve exited $rc, and $n with --acknowledge-failure"
        else
            pin failed_held_volume_refused FAIL "serve exited $rc, and $n with --acknowledge-failure; wanted $EXIT_REFUSED_FAILED twice"
        fi
        # d.5: a scrub is not running here; see d5 below.
        # d.4: the device comes back under the old name with the same
        # content, and nobody runs zpool clear. This is done with the
        # reference server directly: it is what an operator, or a
        # daemon without the rule of §4.2, would do.
        if [ "$IMPL" = reference ] && [ "$(pool_state skzd-d2)" = SUSPENDED ]; then
            step "d: the device comes back under the old name"
            st0=$(pool_state skzd-d2)
            lim 60 ublk add -t loop -n "${BDEV#/dev/ublkb}" -f "$dir/volume.img" -q 1 -d 64 --max_io_buf_bytes 1048576 > "$OUT/d2-readd.log" 2>&1; rc=$?
            for n in $(seq 1 60); do st1=$(pool_state skzd-d2); [ "$st1" = ONLINE ] && break; sleep 1; done
            obs device_back_without_clear "zed $(systemctl is-active zfs-zed 2>/dev/null || echo unknown); ublk add rc $rc; pool $st0, then $st1 after 60 s without zpool clear"
            # d.2: zpool clear.
            before=$(grep -c . "$WORK/d2.record" || true)
            lim 120 zpool clear skzd-d2 > "$OUT/d2-clear.log" 2>&1; rc=$?
            for n in $(seq 1 30); do st1=$(pool_state skzd-d2); [ "$st1" = ONLINE ] && break; sleep 1; done
            v=""; lim 120 "$DD" if=/dev/zero of="$WORK/mnt/skzd-d2/after-clear" bs=1M count=4 conv=fsync status=none 2>/dev/null && v=1
            after=$(grep -c . "$WORK/d2.record" || true)
            if [ "$st1" = ONLINE ] && [ -n "$v" ]; then
                pin clear_resumes_on_returned_device PASS "zpool clear rc $rc; pool $st1; a new write with fsync completed; the writer went from $before to $after acknowledged files; $(pool_errors skzd-d2)"
            else
                pin clear_resumes_on_returned_device FAIL "zpool clear rc $rc ($(tail -n 1 "$OUT/d2-clear.log")); pool $st1; new write $([ -n "$v" ] && echo ok || echo failed)"
            fi
        else
            obs device_back_without_clear "not run: $([ "$IMPL" = reference ] && echo "the pool is $(pool_state skzd-d2)" || echo 'needs the flat image of the reference server')"
            pin clear_resumes_on_returned_device ERROR "not run: $([ "$IMPL" = reference ] && echo "the pool is $(pool_state skzd-d2)" || echo 'needs the flat image of the reference server')"
        fi
    else
        for v in suspends_on_deleted_device failed_exit_status failed_held_volume_refused clear_resumes_on_returned_device; do
            pin "$v" ERROR "$FS_ERR"
        done
    fi
    fail_stop d2

    check_d_shorter
    check_d_scrub
    check_d_not_held
}
# d.3, an observation: the device comes back holding less than ZFS was told.
check_d_shorter() {
    local dir=$WORK/vol-d3 mark rc n st v
    [ "$IMPL" = reference ] || { obs returned_device_with_older_content "not run: needs the flat image of the reference server"; return; }
    step "d: the device comes back with older content"
    mark=$(dmesg_mark)
    if ! fail_setup d3 "$REF" --fail-log-after 16000; then obs returned_device_with_older_content "not run: $FS_ERR"; fail_stop d3; return; fi
    # The older content: a copy of the image now. Everything the writer
    # is told from here on is missing from it.
    lim 120 zpool sync skzd-d3
    cp --sparse=always "$dir/volume.img" "$WORK/d3-older.img"
    n=$(grep -c . "$WORK/d3.record" || true)
    fail_wait d3 400 "$mark"; st=$?
    v=$(grep -c . "$WORK/d3.record" || true)
    fail_stop d3
    if [ $st != 0 ]; then obs returned_device_with_older_content "not run: the pool did not suspend cleanly: $FS_WHY"; return; fi
    serve_wait 30
    lim 60 ublk add -t loop -n "${BDEV#/dev/ublkb}" -f "$WORK/d3-older.img" -q 1 -d 64 --max_io_buf_bytes 1048576 > "$OUT/d3-readd.log" 2>&1
    lim 120 zpool clear skzd-d3 > "$OUT/d3-clear.log" 2>&1; rc=$?
    sleep 10
    st=$(pool_state skzd-d3)
    lim 30 zpool status -v skzd-d3 > "$OUT/d3-status-after-clear.log" 2>&1
    lim 300 zpool scrub -w skzd-d3 > "$OUT/d3-scrub.log" 2>&1
    obs returned_device_with_older_content "the image was copied when the writer had $n files acknowledged; it had $v at the failure. After the copy came back and zpool clear (rc $rc): pool $st; $(pool_errors skzd-d3); the writer's files: $(lim 120 files_match "$WORK/mnt/skzd-d3" "$WORK/d3.record" 2>&1 | tail -n 1); status: $(grep -E 'state:|status:' "$OUT/d3-status-after-clear.log" | tr -s ' ' | tr '\n' ';')"
}
# d.5, an observation: a scrub in progress when the volume fails.
check_d_scrub() {
    local mark st
    step "d: a scrub in progress at the failure"
    mark=$(dmesg_mark)
    if ! fail_setup d5 "$SKYZFSD"; then obs scrub_in_progress_at_failure "not run: $FS_ERR"; fail_stop d5; return; fi
    # More to scrub, then the failure a little after the scrub begins.
    lim 300 "$DD" if=/dev/urandom of="$WORK/mnt/skzd-d5/big" bs=1M count=1024 conv=fsync status=none
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 60
    serve_start "$SKYZFSD" "$WORK/vol-d5" --fail-log-after 1500 || { obs scrub_in_progress_at_failure "not run: serve with --fail-log-after did not start"; fail_stop d5; return; }
    lim 30 zpool scrub skzd-d5
    fail_wait d5 180 "$mark"; st=$?
    obs scrub_in_progress_at_failure "$FS_WHY; scan line when suspended: $(grep -A2 'scan:' "$OUT/d5-status-suspended.log" | tr -s ' ' | tr '\n' ' '); $(pool_errors skzd-d5)"
    fail_stop d5
    return $st
}
# d.6, second half: nothing held the device at the failure, so an
# operator may acknowledge it. Also where the note is kept by default.
check_d_not_held() {
    local dir=$WORK/vol-d6 rc n marker=$WORK/d6.marker found
    step "d: a volume that failed with no client attached"
    : > "$marker"
    # No --run-dir here: the default must be under /run.
    vol_create "$SKYZFSD" "$dir" || { pin failed_unheld_volume_serves_when_acknowledged ERROR "create failed"; pin failure_note_under_run ERROR "create failed"; return; }
    : > "$dir.ready"
    "$SKYZFSD" serve --dir "$dir" --fail-log-after 10 > "$dir.ready" 2>> "$dir.serve.log" &
    SERVE_PID=$!
    for n in $(seq 1 120); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        [ -n "$BDEV" ] && break; sleep 0.25
    done
    [ -n "$BDEV" ] || { pin failed_unheld_volume_serves_when_acknowledged ERROR "serve did not become ready"; pin failure_note_under_run ERROR "serve did not become ready"; return; }
    # Writes straight to the device until it fails; dd then exits, so
    # nothing holds the device when the server takes note.
    for n in $(seq 1 40); do
        lim 30 "$DD" if=/dev/zero of="$BDEV" bs=4k count=1 seek="$n" oflag=direct status=none 2>/dev/null || break
    done
    serve_wait 30
    rc=$SERVE_RC
    found=$(find /run -xdev -type f -newer "$marker" -path '*skyzfs*' 2>/dev/null | head -n 3 | tr '\n' ' ')
    if [ -n "$found" ]; then pin failure_note_under_run PASS "after a failure without --run-dir: $found"
    else pin failure_note_under_run FAIL "no new file under /run with skyzfs in its path after a failure without --run-dir (serve exit $rc)"; fi
    lim 60 "$SKYZFSD" serve --dir "$dir" > /dev/null 2> "$OUT/d6-noack.err"; n=$?
    serve_start_default() { "$SKYZFSD" serve --dir "$dir" --acknowledge-failure > "$dir.ready" 2>> "$dir.serve.log" & SERVE_PID=$!; }
    : > "$dir.ready"; serve_start_default
    BDEV=""
    for found in $(seq 1 120); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        [ -n "$BDEV" ] && break
        kill -0 "$SERVE_PID" 2>/dev/null || break
        sleep 0.25
    done
    if [ "$rc" = "$EXIT_FAILED" ] && [ $n = "$EXIT_REFUSED_FAILED" ] && [ -n "$BDEV" ] \
        && lim 30 "$DD" if=/dev/zero of="$BDEV" bs=4k count=1 oflag=direct status=none 2>/dev/null; then
        pin failed_unheld_volume_serves_when_acknowledged PASS "failed (exit $rc); refused without the acknowledgement (exit $n); served and written with it"
    else
        pin failed_unheld_volume_serves_when_acknowledged FAIL "first serve exit $rc (wanted $EXIT_FAILED); without the acknowledgement exit $n (wanted $EXIT_REFUSED_FAILED); with it: $([ -n "$BDEV" ] && echo "ready $BDEV" || echo 'not ready')"
    fi
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
}

# =================================================================== e
# name seconds wanted: one read held `seconds`; judged against `wanted`.
hold_test() {
    local name=$1 secs=$2 want=$3 dir=$WORK/vol-$1 mark t0 t1 held i rc bad
    HT_ERR=""; HT_WHY=""
    mark=$(dmesg_mark)
    vol_up "$SKYZFSD" "$dir" --hold-one-request "$secs:read" || { HT_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return 2; }
    # The first read is held, whoever sends it: udev probes a new device
    # too. This reader and udev's must all complete.
    ( lim $((secs + 120)) "$DD" if="$BDEV" of=/dev/null bs=4k count=256 iflag=direct status=none 2> "$OUT/$name-dd.err"; echo $? > "$WORK/$name.rc" ) &
    for i in $(seq 1 120); do
        held=$(vol_stat "$SKYZFSD" "$dir" held); [ "${held:-0}" -ge 1 ] && break; sleep 0.25
    done
    [ "${held:-0}" -ge 1 ] || { HT_ERR="status never said a request was held"; return 2; }
    t0=$(now_ms)
    for i in $(seq 1 $(((secs + 90) * 2))); do
        held=$(vol_stat "$SKYZFSD" "$dir" held); [ "$held" = 2 ] && break; sleep 0.5
    done
    t1=$(now_ms)
    wait
    rc=$(cat "$WORK/$name.rc" 2>/dev/null)
    udevadm settle --timeout=30 2>/dev/null
    dmesg_since "$mark" > "$OUT/$name-dmesg.log"
    bad=$(grep -ciE 'I/O error|timeout|timed out|abort' "$OUT/$name-dmesg.log")
    obs "${name}_kernel_log" "$(wc -l < "$OUT/$name-dmesg.log") lines while a request was held ${secs} s; hung-task warnings: $(grep -c 'blocked for more than' "$OUT/$name-dmesg.log"); lines with an I/O error, a timeout or an abort: $bad"
    HT_WHY="held $(( (t1 - t0) / 1000 )) s by the server's own account (asked $secs s, wanted at least $want s); the reader exited $rc; kernel log lines with an I/O error, a timeout or an abort: $bad"
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
    [ "$held" = 2 ] && [ $(( (t1 - t0) / 1000 )) -ge $((want - 5)) ] && [ "$rc" = 0 ] && [ "$bad" = 0 ]
}
check_e() {
    local v
    step "e: one request held $HOLD_S s"
    hold_test e "$HOLD_S" "$HOLD_S"; v=$?
    case $v in 0) pin held_request_completes PASS "$HT_WHY" ;; 1) pin held_request_completes FAIL "$HT_WHY" ;; *) pin held_request_completes ERROR "$HT_ERR" ;; esac
    step "e, control: a request held 5 s is not one held $HOLD_S s"
    hold_test e0 5 "$HOLD_S"; v=$?
    case $v in 0) ctl hold_too_short PASS "$HT_WHY" ;; 1) ctl hold_too_short FAIL "$HT_WHY" ;; *) ctl hold_too_short ERROR "$HT_ERR" ;; esac
}

case $CHECKS in *" a "*) check_a ;; esac
case $CHECKS in *" b "*) check_b ;; esac
case $CHECKS in *" e "*) check_e ;; esac
# Last: it leaves suspended pools behind.
case $CHECKS in *" d "*) check_d ;; esac

step "done"
cat "$R"
exit 0

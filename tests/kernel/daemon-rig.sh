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
# Checks, chosen with RIG_CHECKS (default "a b c d e"):
#   a  parameters of the device and of a pool on it
#   b  kill -9 under load, RIG_KILLS times (default 20); a second serve;
#      an orderly stop
#   d  a failed volume: what ZFS does, and what the server refuses after
#   e  one request held RIG_HOLD_S seconds (default 300)
# RIG_SERVE_ARGS: options added to every serve.
# RIG_STORE: the daemon's store, log or in-place (default log). It is
#   always named on the daemon's serves (--store), never left to the
#   daemon's default. With in-place, the pins and controls that rest on
#   the log store are not taken: the rig writes NA and why, and the
#   judge's table NOT_FOR_IN_PLACE has the same names.
#   c  the conformance suite of skyzfs-blk through the block device:
#      RIG_BLKCHECK is a built skyzfs-blkcheck, which drives the server
#      through tests/kernel/blkcheck-helper.sh. RIG_C_CASES and
#      RIG_C_ROUNDS (default 16 and 4) are the suite's sizes.
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
# shellcheck source=tests/kernel/ublksrvd-look.sh
. "$HERE/ublksrvd-look.sh"
SKYZFSD=${RIG_SKYZFSD:-$REF}
IMPL=daemon; [ "$SKYZFSD" = "$REF" ] && IMPL=reference
UBLKSRVD_SEEN=$OUT/ublksrvd-seen.log; : > "$UBLKSRVD_SEEN"
# For check c's helper: it looks only when the server is the daemon.
BLK_UBLKSRVD_SEEN=""; [ "$IMPL" = reference ] || BLK_UBLKSRVD_SEEN=$UBLKSRVD_SEEN
export BLK_UBLKSRVD_SEEN
# f is not among the default: observations of what follows a refused
# start, one short course each, asked for by name.
CHECKS=" ${RIG_CHECKS:-a b c d e} "
# Every word is a check and there is at least one: a run that checks
# nothing must not look like a run.
n=0
for c in $CHECKS; do
    case $c in a|b|c|d|e|f) n=$((n + 1)) ;; *) echo "RIG_CHECKS: '$c' is not a check; the checks are a b c d e f" >&2; exit 64 ;; esac
done
[ $n -gt 0 ] || { echo "RIG_CHECKS names no check" >&2; exit 64; }
# Check f's courses, all by default. Each leaves a pool that is
# suspended or hung, and the machine's zpool commands slow down with
# each such pool (run 43157965159: the sixth course's looks were minutes
# apart), so a course is best run alone: one job, one course.
F_ALL="scrub export reopen clear writers zed zedreopen long longnolog"
F_COURSES=${RIG_F_COURSES:-$F_ALL}
n=0
for c in $F_COURSES; do
    case " $F_ALL " in *" $c "*) n=$((n + 1)) ;; *) echo "RIG_F_COURSES: '$c' is not a course; the courses are $F_ALL" >&2; exit 64 ;; esac
done
[ $n -gt 0 ] || { echo "RIG_F_COURSES names no course" >&2; exit 64; }
KILLS=${RIG_KILLS:-20}
HOLD_S=${RIG_HOLD_S:-300}
# Options for every serve, split at spaces: for the daemon on a runner,
# "--unsafe-allow-unlocked-memory --unsafe-allow-any-filesystem".
STORE=""
if [ "$IMPL" = daemon ]; then
    STORE=${RIG_STORE:-log}
    case $STORE in log|in-place) ;; *) echo "RIG_STORE: '$STORE' is not a store; the stores are log and in-place" >&2; exit 64 ;; esac
    # On every serve, the stand-in's controls too: it takes the option
    # and has one store whatever the name (a file written in place).
    RIG_SERVE_ARGS="--store $STORE${RIG_SERVE_ARGS:+ $RIG_SERVE_ARGS}"
elif [ -n "${RIG_STORE:-}" ]; then
    echo "RIG_STORE is for the daemon; the stand-in has one store" >&2; exit 64
fi
# A pin or control that rests on the log store, with --store in-place.
in_place() { [ "$STORE" = in-place ]; }
read -r -a SERVE_ARGS <<< "${RIG_SERVE_ARGS:-}"
RUN_DIR=$WORK/run            # the server's --run-dir, so that a test can empty it
VOL_BYTES=$((8 << 30))
SLOG_MB=512
HOLD_AT=$((3 << 30))         # the byte whose first read is held in check e

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
# A limit that holds even when the command cannot be killed.
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
# Three tries, a minute apart, for what needs the network.
# shellcheck source=tests/lib/net-try.sh
. "$HERE/../lib/net-try.sh"
now_ms() { echo $(( $(date +%s%N) / 1000000 )); }
# The kernel log of an interval: klog_mark, klog_since, klog_bad,
# klog_hung, klog_bad_since. A lost marker is "lost", never 0 lines.
# shellcheck source=tests/kernel/klog.sh
. "$HERE/klog.sh"
# Device paths: dev_strays, and why every dd to a device has conv=nocreat.
# shellcheck source=tests/kernel/devnode.sh
. "$HERE/devnode.sh"
# The looks at a device over a wait: look_word, looks_judge.
# shellcheck source=tests/kernel/looks.sh
. "$HERE/looks.sh"

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
    local impl=$1 dir=$2 i t0; shift 2
    strays_note "before the serve of $dir"
    : > "$dir.ready"
    t0=$(( $(date +%s) - 1 ))
    "$impl" serve "${SERVE_ARGS[@]}" --dir "$dir" --run-dir "$RUN_DIR" "$@" > "$dir.ready" 2>> "$dir.serve.log" &
    SERVE_PID=$!; BDEV=""; SERVE_RC=""
    for i in $(seq 1 480); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        if [ -n "$BDEV" ] && [ -b "$BDEV" ]; then
            [ "$impl" = "$REF" ] || ublksrvd_look "${BDEV#/dev/ublkb}" "$t0" "$UBLKSRVD_SEEN"
            return 0
        fi
        if ! kill -0 "$SERVE_PID" 2>/dev/null; then
            wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""
            return 1
        fi
        # The server said ready and the path is something else than a
        # block device: no waiting will change that.
        if [ -n "$BDEV" ] && [ -e "$BDEV" ] && [ ! -b "$BDEV" ]; then
            SERVE_RC="none: the server reported $BDEV, which is not a block device: $(ls -ld "$BDEV" 2>&1)"
            strays_note "at the serve of $dir"
            serve_stuck "$dir" "$BDEV is not a block device"
            return 2
        fi
        sleep 0.25
    done
    SERVE_RC="none: neither ready nor gone after 120 s"
    serve_stuck "$dir" "not ready after 120 s"
    return 2
}
# dir why: the rig gives up on a serve; write down what the server, the
# driver, /dev and udev have to say, each under a limit of its own.
serve_stuck() {
    local dir=$1 rc
    { echo "== serve $dir: $2"
      echo "-- BDEV as the rig read it: '${BDEV:-}'"
      echo "-- $dir.ready:"; ls -l "$dir.ready"; cat "$dir.ready"
      echo "-- processes:"; ps -eo pid,ppid,stat,etime,args --forest | awk '$5 !~ /^\[/' | tail -n 60
      echo "-- the serve log's end:"; tail -n 20 "$dir.serve.log"
      echo "-- ublk list:"; lim 20 ublk list
      echo "-- ls -l /dev/ublkb* /dev/ublkc*:"; ls -l /dev/ublkb* /dev/ublkc*
      echo "-- /sys/block/ublkb*/dev:"; grep -H . /sys/block/ublkb*/dev
      echo "-- lsblk:"; lim 20 lsblk
      echo "-- /sys/class/ublk-char:"; ls /sys/class/ublk-char
      echo "-- udevadm settle --timeout=5:"; lim 20 udevadm settle --timeout=5; rc=$?; echo "exit $rc"
      echo "-- ls -l /dev/ublkb* after the settle:"; ls -l /dev/ublkb*
    } >> "$OUT/serve-stuck.log" 2>&1
}
# where: write down every entry of /dev under a ublk block device's name
# that is not a block device. Any is the rig's error (devnode.sh); the
# run's line `dev_strays` is judged at the end.
STRAYS=$OUT/dev-strays.log
strays_note() {
    local found
    found=$(dev_strays /dev)
    [ -z "$found" ] || printf '%s: %s\n' "$1" "$found" >> "$STRAYS"
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
# PID SECONDS: wait for a background job of this shell to end. 1 when
# it has not ended in time; it is then killed and not waited for. A
# writer or reader on a pool whose device has no server does not end
# when told to (its write sleeps in the kernel), and a bare `wait` on
# it would hold the job to its limit with no line written.
job_end() {
    local i=0
    while kill -0 "$1" 2>/dev/null && [ "$i" -lt $(($2 * 4)) ]; do sleep 0.25; i=$((i + 1)); done
    if kill -0 "$1" 2>/dev/null; then kill -9 "$1" 2>/dev/null; return 1; fi
    wait "$1" 2>/dev/null
    return 0
}
# The --run-dir of `status` and `detach`: the one every serve of
# serve_start gets. A check that serves with the default empties this.
VOL_RUN_ARGS=(--run-dir "$RUN_DIR")
vol_status() { lim 30 "$1" status --dir "$2" "${VOL_RUN_ARGS[@]}" 2>/dev/null; }      # impl dir
# check dir: `unstored` (completions waiting because the acknowledged
# position could not be stored) is 0 in a healthy run. Counters are
# since the instance started, so this reads the instance serving now.
pin_unstored() {
    local v; v=$(vol_stat "$SKYZFSD" "$2" unstored)
    case $v in
        0) pin "unstored_zero_$1" PASS "status says unstored 0 at the end of the check" ;;
        '') pin "unstored_zero_$1" ERROR "status gave no unstored" ;;
        *) pin "unstored_zero_$1" FAIL "status says unstored $v" ;;
    esac
}
# impl dir key least [ms]: a counter of `status`, read again until it
# is at least `least` or the limit has passed (2 s). The daemon writes
# its status file on an interval, so a counter read at once after the
# event it counts can be a fraction of a second old.
vol_stat_least() {
    local v t0; t0=$(now_ms)
    while :; do
        v=$(vol_stat "$1" "$2" "$3")
        { [[ $v =~ ^[0-9]+$ ]] && [ "$v" -ge "$4" ]; } && break
        [ $(( $(now_ms) - t0 )) -lt "${5:-2000}" ] || break
        sleep 0.1
    done
    echo "$v"
}
vol_stat() { vol_status "$1" "$2" | awk -v k="$3" '$1 == k { print $2; exit }'; }
# The process to kill -9: the one `status` names, else the one started.
server_pid() { local p; p=$(vol_stat "$1" "$2" pid); echo "${p:-$SERVE_PID}"; }
vol_detach() { lim 90 "$1" detach --dir "$2" "${VOL_RUN_ARGS[@]}" >/dev/null 2>&1; }
# dir -> a digest of the records a refused command must leave alone:
# the attachment record in the volume's directory and every note under
# the run directory. Not the status file, which a live server rewrites.
vol_records() {
    { cat "$1/attachment" 2>/dev/null
      find "$RUN_DIR" -type f \( -name '*.note' -o -name failed -o -name '*.failed' \) -exec sha256sum {} + 2>/dev/null | sort; } | sha256sum | cut -c1-16
}
ublk_number_free() { # N: no control node and the driver knows no such device
    [ ! -e "/dev/ublkc$1" ] && ! lim 20 ublk list -n "$1" 2>/dev/null | grep -q '^dev id'
}
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
# shellcheck disable=SC2317,SC2329  # reached through lim
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

# shellcheck disable=SC2317,SC2329  # reached through the trap
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
    # The limit on reports of blocked tasks is the machine's: put back
    # what the rig found, however the run ends.
    [ -z "${HUNG_WARNINGS_WAS:-}" ] || echo "$HUNG_WARNINGS_WAS" > /proc/sys/kernel/hung_task_warnings 2>/dev/null
    chmod -R a+rX "$OUT"
}
trap cleanup EXIT

# ------------------------------------------------------------ setup
step "setup"
rec impl "$IMPL: $SKYZFSD"
rec store "${STORE:-none: the stand-in has one store, a file it writes in place}"
rec kernel "$(uname -r)"
# The kernel reports a blocked task only so many times per boot (ten by
# default) and then says nothing more: with the limit spent, a log
# without such reports means nothing. The rig lifts the limit and
# writes down what it found.
HUNG_WARNINGS_WAS=$(cat /proc/sys/kernel/hung_task_warnings 2>/dev/null)
rec hung_task_warnings_start "${HUNG_WARNINGS_WAS:-unreadable} (timeout $(cat /proc/sys/kernel/hung_task_timeout_secs 2>/dev/null || echo unreadable) s); set to $(echo -1 > /proc/sys/kernel/hung_task_warnings 2>/dev/null; cat /proc/sys/kernel/hung_task_warnings 2>/dev/null || echo unreadable) for the run"
# shellcheck disable=SC1091
rec os "$(. /etc/os-release && echo "$PRETTY_NAME")"
SETUP_OK=1; SETUP_WHY=""
# The packages and the reference server's source come over the
# network: each is tried three times, a minute apart, and a setup that
# still fails says which step and why in its own line, which the judge
# puts first.
# shellcheck disable=SC2317,SC2329  # run through net_try
setup_packages() { apt-get update && apt-get install -y --no-install-recommends zfsutils-linux python3 util-linux; }
net_try setup_packages > "$OUT/apt.log" 2>&1 || { SETUP_OK=""; SETUP_WHY+="zfsutils-linux was not installed in three tries: $(net_why "$OUT/apt.log"); "; }
[ -e /dev/ublk-control ] || modprobe ublk_drv 2>> "$OUT/apt.log" || {
    net_try apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/apt.log" 2>&1
    modprobe ublk_drv 2>> "$OUT/apt.log" || { SETUP_OK=""; SETUP_WHY+="no ublk driver (modprobe ublk_drv failed); "; }
}
modprobe zfs 2>> "$OUT/apt.log" || { SETUP_OK=""; SETUP_WHY+="modprobe zfs failed; "; }
rec zfs "$(cat /sys/module/zfs/version 2>/dev/null)"
rec zfs_userland "$(zfs version 2>&1 | tr '\n' ' ')"
# shellcheck disable=SC2317,SC2329  # run through net_try
build_reference() {
    UBLKSRV_PATCH=$HERE/ublksrv-skz.patch "$HERE/build-ublksrv.sh" \
        "${UBLKSRV_REF:-6d0d922614ec18e6f2b5d476af8d540a1d9c1de8}" > "$OUT/ublksrv.ver" 2> "$OUT/ublksrv-build.log"
}
if net_try build_reference; then
    rec ublk_server "$(tail -n 1 "$OUT/ublksrv.ver"), with ublksrv-skz.patch (built at try $NET_TRIES)"
else
    rec ublk_server "BUILD FAILED in three tries: $(tail -n 1 "$OUT/ublksrv-build.log")"
    tail -n 60 "$OUT/ublksrv-build.log"
    SETUP_OK=""; SETUP_WHY+="the reference server was not built in three tries: $(net_why "$OUT/ublksrv-build.log"); "
fi
zed_state() { local z; z=$(systemctl is-active zfs-zed 2>/dev/null); echo "${z:-unknown}"; }
rec zed "$(zed_state)"
mkdir -p "$RUN_DIR" "$WORK/mnt"
if [ -z "$SETUP_OK" ]; then
    rec setup "FAILED: ${SETUP_WHY:-see apt.log and ublksrv-build.log}"
    exit 2
fi
rec setup ok

# =================================================================== a
# Overlapping writes in flight together apply whole (§4.1): two writers
# at one range at one moment, for some seconds, on a new volume.
# name impl seconds [serve arguments] -> OV_ROUNDS OV_TORN OV_SAID, 1 if it did not run.
overlap_test() {
    local name=$1 impl=$2 secs=$3 dir=$WORK/vol-$1 rc; shift 3
    OV_ROUNDS=0; OV_TORN=0; OV_TOGETHER=0; OV_SAID=""
    if ! vol_up "$impl" "$dir" "$@"; then OV_SAID="not created or served (serve rc ${SERVE_RC:-none})"; return 1; fi
    lim $((secs + 60)) python3 -I "$HERE/overlap-tear.py" "$BDEV" "$secs" > "$OUT/$name-overlap.out" 2> "$OUT/$name-overlap.err"; rc=$?
    OV_SAID=$(tail -n 1 "$OUT/$name-overlap.out")
    read -r _ OV_ROUNDS _ OV_TORN _ OV_TOGETHER _ <<< "$OV_SAID"
    vol_status "$impl" "$dir" > "$OUT/$name-status-end.txt"
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$impl" "$dir"
    [ $rc = 0 ] && [[ ${OV_ROUNDS:-} =~ ^[0-9]+$ ]] && [[ ${OV_TORN:-} =~ ^[0-9]+$ ]] && [[ ${OV_TOGETHER:-} =~ ^[0-9]+$ ]]
}
# rounds together: whether the load was what it claims. The tool counts
# the rounds in which both calls began before either ended; a round
# without that had no two writes in flight and shows nothing on any
# server. Fewer than half of the rounds, or fewer than 200, is the
# rig's error. (Measured: see the pull request of this rule.)
# The tool also prints "well W": the rounds whose calls overlapped for
# at least a quarter of the shorter one, a stricter count that the
# evidence lines carry. The floor is still on "together" and still the
# one set before any kernel run; it moves to "well" and to a measured
# figure once runs with this count exist.
overlap_enough() { [ "$1" -ge 200 ] && [ $(($2 * 2)) -ge "$1" ] && [ "$2" -ge 200 ]; }
check_a_overlap() {
    local rounds torn together said ms=${RIG_COMMIT_DELAY_MS:-20}
    step "a: overlapping writes in flight apply whole"
    if overlap_test ao "$SKYZFSD" 30; then
        rounds=$OV_ROUNDS; torn=$OV_TORN; together=$OV_TOGETHER; said="plain: $OV_SAID"
        if ! overlap_enough "$OV_ROUNDS" "$OV_TOGETHER"; then
            pin overlapping_writes_apply_whole ERROR "the two writes were in flight together in only $OV_TOGETHER of $OV_ROUNDS rounds (at least half and at least 200 are needed): $said"
        elif overlap_test aod "$SKYZFSD" 30 --commit-delay-ms "$ms"; then
            rounds=$((rounds + OV_ROUNDS)); torn=$((torn + OV_TORN)); together=$((together + OV_TOGETHER)); said+="; acknowledged $ms ms after the apply: $OV_SAID"
            if ! overlap_enough "$OV_ROUNDS" "$OV_TOGETHER"; then
                pin overlapping_writes_apply_whole ERROR "with the delay the two writes were in flight together in only $OV_TOGETHER of $OV_ROUNDS rounds (at least half and at least 200 are needed): $said"
            else
                pin_is overlapping_writes_apply_whole "$torn of $rounds rounds torn, the two writes in flight together in $together of them; two writers of 16 blocks at one range, 30 s each; $said" [ "$torn" = 0 ]
            fi
        else
            pin overlapping_writes_apply_whole ERROR "with the delay: ${OV_SAID:-no output} $(tail -n 1 "$OUT/aod-overlap.err" 2>/dev/null)"
        fi
    else
        pin overlapping_writes_apply_whole ERROR "${OV_SAID:-no output} $(tail -n 1 "$OUT/ao-overlap.err" 2>/dev/null)"
    fi
    unflushed_obs a_overlap "$(for t in ao aod; do unflushed_said "$t" < "$OUT/$t-status-end.txt"; done 2>/dev/null)"
    # The control: a server that tears on purpose, whatever the storage
    # under it does. FAIL means torn rounds were seen, which shows in
    # every run that the pin above can fail.
    step "a, control: a server that tears overlapping writes"
    if SKZ_REF_TEAR=1 overlap_test ao1 "$REF" 20; then
        if ! overlap_enough "$OV_ROUNDS" "$OV_TOGETHER"; then
            ctl torn_overlapping_writes ERROR "the two writes were in flight together in only $OV_TOGETHER of $OV_ROUNDS rounds (at least half and at least 200 are needed): $OV_SAID"
        elif [ "$OV_TORN" -gt 0 ]; then
            ctl torn_overlapping_writes FAIL "$OV_TORN of $OV_ROUNDS rounds torn; $OV_SAID"
        else
            ctl torn_overlapping_writes PASS "no round of $OV_ROUNDS torn; $OV_SAID"
        fi
    else
        ctl torn_overlapping_writes ERROR "${OV_SAID:-no output} $(tail -n 1 "$OUT/ao1-overlap.err" 2>/dev/null)"
    fi
    # The same load on the reference server as upstream has it, every
    # request issued to the file as it arrives. An observation and not
    # a control: how often that server tears depends on the runner
    # (about 3 rounds in 100 on both images of run 43012950834; 2 of
    # 82,335 and 0 of 92,329 in run 43016426113). The pin above says
    # something in a run where this line shows torn rounds, and little
    # where it shows none.
    step "a, observation: a server that does not order overlapping writes"
    if SKZ_REF_UNORDERED=1 overlap_test ao0 "$REF" 60; then
        said="plain, 60 s: $OV_SAID"; rounds=$OV_ROUNDS; torn=$OV_TORN
        if SKZ_REF_UNORDERED=1 overlap_test ao0d "$REF" 30 --commit-delay-ms "$ms"; then
            said+="; acknowledged $ms ms after the apply, 30 s: $OV_SAID"; rounds=$((rounds + OV_ROUNDS)); torn=$((torn + OV_TORN))
        fi
        obs a_unordered_overlapping_writes "$torn of $rounds rounds torn by the reference server with its ordering off; $said"
    else
        obs a_unordered_overlapping_writes "not run: ${OV_SAID:-no output} $(tail -n 1 "$OUT/ao0-overlap.err" 2>/dev/null)"
    fi
}
check_a() {
    local dir=$WORK/vol-a pool=skzd-a q slog v f0 f1 label
    step "a: parameters"
    if ! vol_up "$SKYZFSD" "$dir"; then
        for v in block_size max_request write_cache discard write_zeroes no_fua discard_carried_out write_zeroes_carried_out label_at_byte_0 flush_counted; do
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

    # Offered is not carried out: one discard and one write of zeros, by
    # the server's own count. And which calls reach the device: the two
    # ioctls, and fallocate in its three forms.
    local d0 z0 d1 z1 at=$((1 << 30)) probes=""
    d0=$(vol_stat "$SKYZFSD" "$dir" discards); z0=$(vol_stat "$SKYZFSD" "$dir" write_zeroes)
    lim 30 blkdiscard -f -o "$at" -l 1048576 "$BDEV" > "$OUT/a-blkdiscard.log" 2>&1; rc=$?
    d1=$(vol_stat_least "$SKYZFSD" "$dir" discards $(( ${d0:-0} + 1 )))
    v=no; [ "$rc" = 0 ] && [ "${d1:-0}" -gt "${d0:-0}" ] && v=yes
    pin_is discard_carried_out "blkdiscard of 1 MiB: rc $rc; the server's discards went from ${d0:-?} to ${d1:-?}" [ "$v" = yes ]
    lim 30 blkdiscard -f -z -o "$at" -l 1048576 "$BDEV" > "$OUT/a-blkzeroout.log" 2>&1; rc=$?
    z1=$(vol_stat_least "$SKYZFSD" "$dir" write_zeroes $(( ${z0:-0} + 1 )))
    v=no; [ "$rc" = 0 ] && [ "${z1:-0}" -gt "${z0:-0}" ] && v=yes
    pin_is write_zeroes_carried_out "blkdiscard -z (BLKZEROOUT) of 1 MiB: rc $rc; the server's write_zeroes went from ${z0:-?} to ${z1:-?}" [ "$v" = yes ]
    for v in "punch-hole keep-size no-hide-stale:7" "punch-hole keep-size:3" "zero-range keep-size:17"; do
        probes+="fallocate ${v%:*}: $(lim 30 python3 -I -c '
import ctypes, os, sys
libc = ctypes.CDLL(None, use_errno=True)
libc.fallocate.argtypes = [ctypes.c_int, ctypes.c_int, ctypes.c_longlong, ctypes.c_longlong]
fd = os.open(sys.argv[1], os.O_RDWR)
r = libc.fallocate(fd, int(sys.argv[2]), int(sys.argv[3]), 1048576)
print("ok" if r == 0 else os.strerror(ctypes.get_errno()))' "$BDEV" "${v#*:}" "$at" 2>&1 | tail -n 1); "
    done
    sleep 1.2
    rec a_fallocate "$probes; after them the server counts discards $(vol_stat "$SKYZFSD" "$dir" discards), write_zeroes $(vol_stat "$SKYZFSD" "$dir" write_zeroes) (before: ${d1:-?}, ${z1:-?})"

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
    f1=$(vol_stat_least "$SKYZFSD" "$dir" flushes $(( ${f0:-0} + 2 )))
    if [ -z "$f0" ] || [ -z "$f1" ]; then
        pin flush_counted ERROR "status gave no flush counter"
    elif [ $((f1 - f0)) -ge 2 ]; then
        pin flush_counted PASS "the server counted $((f1 - f0)) flushes during one zpool sync"
    else
        pin flush_counted FAIL "the server counted $((f1 - f0)) flushes during one zpool sync"
    fi
    pin_unstored a "$dir"
    lim 120 zpool export "$pool"
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
}

# =================================================================== b
# impl name kills [environment for the stand-in]: a pool under a writer,
# the server killed `kills` times with writes in flight and served
# again. Sets KT_* for the caller to judge.
KT_SERVE_EXTRA=()      # more arguments for every serve of a kill test
kill_test() {
    local tk impl=$1 name=$2 kills=$3 dir=$WORK/vol-$2 pool=skzd-$2 mnt slog n i w pid inflight rc st ms t0 watcher live gone looks saw mark states hw klog
    KT_WAITED=""; KT_WAIT_BAD=0; LK_VERDICT=""; LK_WHY=""; LK_SAID=""
    mnt=$WORK/mnt/$pool
    KT_ERR=""; KT_DONE=0; KT_INFLIGHT=0; KT_REFUSED=0; KT_FAILED_WRITES=""; KT_VDEV=""; KT_SCRUB=""; KT_FILES=""; KT_ROUNDS=""; KT_MS=""; KT_QUIESCE=""
    # What each new server found applied and not acknowledged (status
    # key recovered_unacknowledged): the sum, and how many did not say.
    KT_RECOVERED=0; KT_RECOVERED_UNSAID=0
    vol_up "$impl" "$dir" "${KT_SERVE_EXTRA[@]}" || { KT_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return 1; }
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
    # The log's size and the steps taken, by the daemon's status, five
    # times a second: "pid log_live checkpoint_steps under|none", the pid
    # "-" when the status has none (a server that has died: without the
    # mark the columns would move left and the steps be read as
    # log_live). The
    # pin on the log's bound reads them (log_bound_pin).
    : > "$WORK/$name.logsamples"; KT_STARTS=""
    if [ "$IMPL" = daemon ] && [ "$impl" = "$SKYZFSD" ]; then
        ( while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do
              vol_status "$impl" "$dir" | awk '$1 == "pid" { p = $2 } $1 == "log_live" { l = $2 } $1 == "checkpoint_steps" { s = $2 } $1 == "checkpoint_step" { u = $2 }
                  END { if (l != "") print (p == "" ? "-" : p), l, s, u }' >> "$WORK/$name.logsamples"
              sleep 0.2
          done ) &
    fi
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
        tk=$(now_ms)
        # How long the kernel reports the device LIVE after its server
        # died: watched beside the serve that follows, which must cope
        # with it. The driver reports pid -1 once the server's task is
        # gone, whatever the state, so the look is at the state alone.
        # A look is one `ublk list`; the state it reports held at some
        # moment between its start and its end. So the device was still
        # LIVE at the start of the last look that said so, and no longer
        # LIVE with the dead server at the end of the first look that
        # said anything else (waiting for a server, or LIVE with the
        # next one already): the true time lies between the two.
        ( t0=$(now_ms); looks=0; saw=""; live=-1; gone=-1
          while [ $(( $(now_ms) - t0 )) -lt 10000 ]; do
              at=$(( $(now_ms) - t0 ))
              saw=$(timeout -s KILL 5 ublk list -n "${BDEV#/dev/ublkb}" 2>/dev/null | tr '\n' ' ' | sed -n 's/.*daemon pid \(-\{0,1\}[0-9]*\) state \([A-Z_]*\).*/\2 \1/p')
              looks=$((looks + 1))
              case $saw in
                  "LIVE $pid"|"LIVE -1") live=$at ;;
                  *) gone=$(( $(now_ms) - t0 )); break ;;
              esac
          done
          echo "$live $gone $looks ${saw:-nothing}" > "$WORK/$name.quiesce" ) &
        watcher=$!
        [ "${w:-0}" -gt 0 ] && KT_INFLIGHT=$((KT_INFLIGHT + 1))
        [ -z "$SERVE_PID" ] || serve_wait 30 || kill -9 "$SERVE_PID" 2>/dev/null
        # KT_WAIT_S: after the first kill no server comes for that long.
        if [ "${KT_WAIT_S:-0}" -gt 0 ] && [ "$n" = 1 ]; then
            mark=$(klog_mark); i=$(wc -l < "$WORK/$name.record"); states=""; t0=$(now_ms)
            hw=$(cat /proc/sys/kernel/hung_task_warnings 2>/dev/null)
            while [ $(( $(now_ms) - t0 )) -lt $((KT_WAIT_S * 1000)) ]; do
                # Every look is an attempt: one that names no state is
                # written down as "none", not left out.
                states+="$(look_word "$(ublk_state "$BDEV")") "
                sleep 10
            done
            # No marker in the kernel log is no log of the wait: "lost",
            # which the pin reports as the rig's error and not as 0 lines.
            KT_WAIT_BAD=$(klog_bad_since "$mark" "$OUT/$name-wait-dmesg.log")
            looks_judge "$states" QUIESCED "$KT_WAIT_S"
            if [ "$KT_WAIT_BAD" = lost ]; then klog="lost: the rig's marker was not found in it at the end of the wait"
            else klog="$(wc -l < "$OUT/$name-wait-dmesg.log") lines, $KT_WAIT_BAD with an I/O error, a request timed out or an abort; tasks reported blocked: $(klog_hung "$OUT/$name-wait-dmesg.log")"; fi
            KT_WAITED="no server for $(( ($(now_ms) - t0) / 1000 )) s after the first kill; the device's state every 10 s: $LK_SAID; the writer finished $(( $(wc -l < "$WORK/$name.record") - i )) files in that time (an fsync is served by the log device, so it runs until the pool's dirty data is full); kernel log: $klog (the kernel had $hw reports left to make before the wait, -1 is no limit); pool $(pool_state "$pool") at the end of the wait"
        fi
        st=$(ublk_state "$BDEV")
        t0=$(now_ms)
        serve_start "$impl" "$dir" "${KT_SERVE_EXTRA[@]}"; rc=$?
        ms=$(( $(now_ms) - t0 ))
        wait "$watcher"
        # The status of the server that took over, not the dead one's
        # last: its pid is another, and it is alive.
        for i in $(seq 1 30); do
            i=$(vol_stat "$impl" "$dir" pid)
            { [ -n "$i" ] && [ "$i" != "$pid" ] && kill -0 "$i" 2>/dev/null; } && break
            sleep 0.1
        done
        i=$(vol_stat "$impl" "$dir" recovered_unacknowledged)
        if [[ $i =~ ^[0-9]+$ ]]; then KT_RECOVERED=$((KT_RECOVERED + i)); else KT_RECOVERED_UNSAID=$((KT_RECOVERED_UNSAID + 1)); fi
        # The new server's own account of its start: how long it took
        # and how much log it replayed ("-" from a server that does
        # not say).
        # kill to the new serve's launch (the old server's exit and the
        # rig's own steps), the launch to its ready line, and within
        # that the daemon's own start: what is left is the process
        # before the daemon's clock and the rig's polling (0.25 s).
        KT_STARTS+="$(( t0 - tk ))+$ms:"
        i=$(vol_stat "$impl" "$dir" start_ms); KT_STARTS+="${i:--}:"
        i=$(vol_stat "$impl" "$dir" replayed_bytes); KT_STARTS+="${i:--} "
        read -r live gone looks saw < "$WORK/$name.quiesce" 2>/dev/null || { live=""; gone=""; looks=0; saw="no record"; }
        KT_ROUNDS+="kill $n: ${w:-0} writes in flight, $st, served again rc $rc in $ms ms; last seen LIVE with the dead server at $live ms, first seen otherwise at $gone ms ($saw, $looks looks); "
        KT_QUIESCE+="${live:--1}:${gone:--1} "
        [ $rc != 0 ] || KT_MS+="$ms "
        [ "${SERVE_RC:-}" != "$EXIT_REFUSED_FAILED" ] || KT_REFUSED=$((KT_REFUSED + 1))
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
    rec "${name}_dead_server_live_ms" "$(live_stats "$KT_QUIESCE")"
    # The pool as the kills left it, before anything clears it.
    KT_STATE=$(pool_state "$pool")
    KT_VDEV=$(pool_errors "$pool")
    [ "$KT_STATE" = ONLINE ] || lim 120 zpool clear "$pool" > /dev/null 2>&1
    lim 900 zpool scrub -w "$pool" > "$OUT/$name-scrub.log" 2>&1
    KT_SCRUB=$(pool_errors "$pool")
    # Under a limit, like the clear and the scrub: when a serve did not
    # come back or the pool is still suspended, these reads block, and
    # that is what this test exists to report, not to hang on.
    KT_FILES=$(lim 300 files_match "$mnt" "$WORK/$name.record" 2>&1)
    [ $? != 124 ] || KT_FILES="not read: the pool (${KT_STATE:-unknown} after the kills, $(pool_state "$pool") now) did not answer in 300 s"
    rec "${name}_result" "$KT_DONE of $kills kills served again, $KT_INFLIGHT with writes in flight; failed writes $KT_FAILED_WRITES; pool $KT_STATE, $KT_VDEV; after scrub: $KT_SCRUB; files: $KT_FILES"
    [ -z "$KT_ERR" ]
}
# "last:first ..." per kill, in ms after the kill (-1: never) -> the
# distribution of both ends. The time a dead server's device stays LIVE
# is, for each kill, between its two figures.
live_stats() {
    echo "$1" | tr ' ' '\n' | grep . | awk -F: '
        function dist(v, n,    i, j, t) {
            for (i = 2; i <= n; i++) { t = v[i]; for (j = i - 1; j >= 1 && v[j] > t; j--) v[j + 1] = v[j]; v[j + 1] = t }
            return sprintf("min %d, median %d, max %d", v[1], v[int((n + 1) / 2)], v[n])
        }
        { all = all " " $1 ".." $2
          if ($1 < 0) never_live++; else a[++na] = $1
          if ($2 < 0) never_gone++; else b[++nb] = $2 }
        END {
            printf "%d kills. Still LIVE at the last look that said so, ms after the kill: %s (%d kills; at %d the first look already said otherwise). ", NR, na ? dist(a, na) : "none", na, never_live
            printf "No longer LIVE with the dead server by: %s (%d kills; at %d not within 10 s). ", nb ? dist(b, nb) : "none", nb, never_gone
            printf "For each kill the true time is between its two figures; all, last-first:%s", all
        }'
}
# "MS MS ...": the take-over times in order -> their count, the median
# of the first five and of the last five, and the list.
medians() {
    # shellcheck disable=SC2086  # a list of numbers
    set -- $1
    if [ $# -lt 10 ]; then echo "only $# take-overs, too few for two medians of five: $*"; return; fi
    local first last
    first=$(printf '%s\n' "${@:1:5}" | sort -n | sed -n 3p)
    last=$(printf '%s\n' "${@: -5}" | sort -n | sed -n 3p)
    echo "$# take-overs; median of the first five $first ms, of the last five $last ms; all, in order: $*"
}
# "MS MS ...": the pin on the take-over times of a kill test.
takeover_pin() {
    # shellcheck disable=SC2086  # a list of numbers
    set -- $1
    local first last
    if [ $# -lt 10 ]; then pin takeover_time_does_not_grow ERROR "only $# take-overs; ten are needed for two medians of five"; return; fi
    first=$(printf '%s\n' "${@:1:5}" | sort -n | sed -n 3p)
    last=$(printf '%s\n' "${@: -5}" | sort -n | sed -n 3p)
    # 1.5 times plus 500 ms: the rig reads the ready line every 250 ms,
    # so two medians of a steady take-over can differ by that much.
    pin_is takeover_time_does_not_grow "over $# take-overs the median of the first five is $first ms and of the last five $last ms; allowed for the last five: 1.5 times the first plus 500 ms, $((3 * first / 2 + 500)) ms" \
        [ "$last" -le $((3 * first / 2 + 500)) ]
}
# name: the pin on the log's bound, from the samples of the kill test
# `name` and from what each new server replayed. The rule (design
# §5.1; skyzfs-local, the checkpoint rule's own words): before every
# request the log is below the high mark, so it never holds more than
# the mark and what one request adds. One request of at most 1 MiB
# adds its record, 1 MiB and a header of one alignment unit (4096);
# and, when the record does not fit what is left of a segment, what
# closes that segment and opens the next: less than one more record
# left unused, the closing marker and the next segment's header (a
# write unit each, 4096 on a file system of 4096-byte blocks). So:
# 128 MiB + 2 * (1 MiB + 4096) + 3 * 4096. A step adds nothing: it
# takes out. A test in which no step was taken never came to the mark
# and says nothing: the rig's error.
log_bound_pin() {
    local f=$WORK/$1.logsamples bound=$(( (128 << 20) + 2 * ((1 << 20) + 4096) + 3 * 4096 )) n max steps under rmax=0 rn=0 i
    read -r n max steps under <<< "$(awk '{ n++; if ($2 + 0 > m) m = $2 + 0; if ($1 != "-" && $3 + 0 > s[$1]) s[$1] = $3 + 0; if ($4 == "under") u++ }
        END { for (p in s) t += s[p]; printf "%d %d %d %d", n, m, t, u }' "$f" 2>/dev/null)"
    for i in $KT_STARTS; do
        i=${i##*:}
        [[ $i =~ ^[0-9]+$ ]] || continue
        rn=$((rn + 1)); [ "$i" -le "$rmax" ] || rmax=$i
    done
    i="$n readings of the status 0.2 s apart: log_live at most $max bytes; $steps checkpoint steps taken by the servers of the test, a step under way at $under readings; the most a new server replayed: $rmax bytes ($rn servers said); the bound is $bound bytes (the high mark of 128 MiB and what one request of 1 MiB can add: its record, and a segment closed and opened)"
    if [ "${n:-0}" -lt 20 ]; then
        pin log_stays_below_high_mark ERROR "the status was read only ${n:-0} times with a log_live in it: $i"
    elif [ "$rn" = 0 ]; then
        pin log_stays_below_high_mark ERROR "no new server said what it replayed (status key replayed_bytes): $i"
    elif [ "$steps" = 0 ]; then
        pin log_stays_below_high_mark ERROR "no checkpoint step was taken in the whole test, so the log never came to its high mark and the bound was not tried: $i"
    elif [ "$max" -le "$bound" ] && [ "$rmax" -le "$bound" ]; then
        pin log_stays_below_high_mark PASS "$i"
    else
        pin log_stays_below_high_mark FAIL "$i"
    fi
}
# The four judgments of a kill test, as pins or as one control line.
kill_verdict() { # -> KT_VERDICT PASS | FAIL | ERROR, the reason in KT_WHY
    KT_WHY=""; KT_VERDICT=""
    if [ -n "$KT_ERR" ]; then KT_WHY=$KT_ERR; KT_VERDICT=ERROR; return; fi
    # A kill with no write in flight tests a restart and not the
    # reissue of requests, so a run in which more than one kill in ten
    # found none says nothing about what the pin claims: a rig error.
    # "In flight" is the write column of the device's `inflight` file,
    # read just before the kill and not at it.
    if [ $((KT_INFLIGHT * 10)) -lt $((KT_DONE * 9)) ]; then
        KT_WHY="writes were in flight at only $KT_INFLIGHT of $KT_DONE kills; at least nine in ten are needed"; KT_VERDICT=ERROR; return
    fi
    [ "$KT_FAILED_WRITES" = 0 ] || KT_WHY+="application writes failed: $KT_FAILED_WRITES; "
    case "$KT_STATE/$KT_VDEV" in "ONLINE/vdev errors 0 "*) ;; *) KT_WHY+="after the kills: pool $KT_STATE, $KT_VDEV; " ;; esac
    case "$KT_SCRUB" in "vdev errors 0 "*"with 0 errors"*) ;; *) KT_WHY+="scrub: $KT_SCRUB; " ;; esac
    case "$KT_FILES" in ok*) ;; *) KT_WHY+="files: $KT_FILES; " ;; esac
    if [ -z "$KT_WHY" ]; then KT_VERDICT=PASS; else KT_VERDICT=FAIL; fi
}
# With the daemon: after a kill, a recovery that drops the last record
# must be refused, and the pool must suspend.
#
# The record that is dropped must be one whose write was acknowledged,
# or dropping it loses nothing and the daemon rightly serves. A kill
# under load leaves that to chance (run 43064604469: refused on one
# image; on the other the start line said recovered_unacknowledged 1,
# the daemon served, and it was right). So the server is killed when
# nothing is in flight: the writer is stopped, the pool synced, and the
# server's counts of writes and of requests in flight must stand still
# over a second, with no position left unstored (`unstored 0`: a
# position that could not be stored would leave the last record beyond
# it). The rule, as the daemon's author states it: --recover-short N
# takes the log's end as if its last N data records (writes,
# write-zeroes, discards; not pads or marks, and a flush leaves none)
# were not there, and the daemon refuses exactly when that end is at or
# before the stored acknowledged position, which is where the newest
# acknowledged record starts. At rest that record is the log's last
# data record, so N=1 is refused. Two cases would not be: a log with no
# data record, and, once the daemon checkpoints, a log whose last data
# record a checkpoint has taken; this load stays far below the high
# mark. A writer is started again after the kill, so that the
# pool has work when its device is refused. If the server never came
# to rest, the control is the rig's error and not a verdict.
# pool tag moment: what ZFS and the kernel say of a pool whose device is
# gone, each under a limit and each in a file of its own
# ($OUT/TAG-MOMENT-*.txt): the transaction groups, the events since the
# kill, the status, and the kernel stacks of ZFS's threads (all pools':
# the threads do not carry the pool's name).
refused_snap() {
    local pool=$1 tag=$2 at=$3 p
    cp "/proc/spl/kstat/zfs/$pool/txgs" "$OUT/$tag-$at-txgs.txt" 2>/dev/null
    # A sample of the events and a count by class, not all of them: a
    # pool that re-executes failed writes posts tens of thousands a
    # second (run 43100887191: files of up to 584 MB).
    # shellcheck disable=SC2016  # the inner shell expands them
    # Both are cut by `head`: while the pool posts events faster than
    # they are read, `zpool events` does not come to an end (run
    # 43107654994: the count ran into its limit at every snapshot, 40 s
    # each, and the probes were late by as much).
    lim 15 bash -c 'zpool events -v 2>&1 | head -n 1500 > "$1"; zpool events -H 2>/dev/null | head -n 20000 | awk "{ c[\$NF]++ } END { for (k in c) print c[k], k }" > "$2"' \
        events "$OUT/$tag-$at-events.txt" "$OUT/$tag-$at-event-classes.txt" 2> /dev/null
    lim 20 zpool status -v "$pool" > "$OUT/$tag-$at-status.txt" 2>&1
    { echo "== the pools at this moment (zpool list -H -o name,health; the threads below do not carry a pool's name):"
      lim 20 zpool list -H -o name,health 2>&1
      for p in $(pgrep 'txg_sync|txg_quiesce|z_wr_iss|z_wr_int|z_null|spa_async|zed|z_flush'); do
          echo "== $p $(cat "/proc/$p/comm" 2>/dev/null)"; cat "/proc/$p/stack" 2>/dev/null
      done
    } > "$OUT/$tag-$at-stacks.txt"
}
# seconds name: true once, when EL has reached SECONDS (the clock, not
# a count of looks: a look can take seconds).
rs_due() { [ "$EL" -ge "$1" ] && [[ " $RS_DID " != *" $2 "* ]] && RS_DID+="$2 "; }
# txgs file -> the newest transaction group and its state (the letters
# are ZFS's: O open, Q quiescing, W waiting for sync, S syncing, C
# committed), and how many of those listed are not committed.
txg_say() {
    awk 'NR > 1 { n++; if ($3 != "C") open++; last = $1 " in state " $3 } END { if (n) printf "newest transaction group %s, %d of the last %d not committed", last, open, n; else printf "transaction groups not read" }' "$1" 2>/dev/null
}
# status file, device number -> the vdev's error counters as zpool
# status has them: "read R write W cksum C (state S)". ZFS's "zio ...
# error" lines in the kernel log are rate-limited and cannot count
# failed writes; these counters can.
vdev_say() {
    awk -v d="ublkb$2" '$1 == d { printf "read %s write %s cksum %s (state %s)", $3, $4, $5, $2; f = 1 } END { if (!f) printf "not in the status (which gave %d lines)", NR }' "$1" 2>/dev/null || printf 'no status file'
}
# A short course after a refused start (check f): what is done 10 s
# after the refusal, with the first writer's synced writes the only I/O
# until then. Uses the locals of refused_start_arm. The command's exit
# and time go to $WORK/$tag.act; a command that has not returned leaves
# no such file.
rs_act() {
    local cmd
    case $course in
        scrub|zed) cmd=(zpool scrub "$pool") ;;
        zedreopen) cmd=(zpool reopen "$pool") ;;
        export) cmd=(zpool export "$pool") ;;
        reopen) cmd=(zpool reopen "$pool") ;;
        clear) cmd=(zpool clear "$pool") ;;
        *) return ;;
    esac
    act="${cmd[*]}"
    amark=$(klog_mark)
    ( t=$(now_ms); lim 130 "${cmd[@]}" > "$OUT/$tag-act.log" 2>&1; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.act" ) &
}
# The course "writers": a synced writer of 4 MiB every 30 s, each to a
# file of its own, with what the pool held in memory when it was sent
# (the txgs kstat's ndirty of the groups not yet committed) and how far
# the first writer had come.
rs_writers() {
    local k
    for k in 1 2 3 4 5 6 7 8 9; do
        if rs_due $((k * 30)) "w$k"; then
            cp "/proc/spl/kstat/zfs/$pool/txgs" "$OUT/$tag-w$k-txgs.txt" 2>/dev/null
            echo "sent at $EL s, the first writer then at $(grep -c . "$WORK/$tag.record") acknowledged files, $(dirty_say "$OUT/$tag-w$k-txgs.txt"), $(mem_say)" > "$WORK/$tag.w$k.sent"
            ( t=$(now_ms); lim $((330 - k * 30)) "$DD" if=/dev/zero of="$WORK/mnt/$pool/writer-$k" bs=1M count=4 conv=fsync status=none 2> "$OUT/$tag-w$k.err"; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.w$k" ) &
        fi
    done
}
# txgs file -> the bytes dirty in the groups not committed, and the
# limit ZFS delays and then stops writers at.
dirty_say() {
    awk -v max="$(cat /sys/module/zfs/parameters/zfs_dirty_data_max 2>/dev/null)" \
        'NR > 1 && $3 != "C" { d += $4; n++; st = st $3 } END { printf "%d MiB dirty in %d groups not committed (states %s; zfs_dirty_data_max %s)", d / 1048576, n, st == "" ? "none" : st, max == "" ? "not read" : int(max / 1048576) " MiB" }' "$1" 2>/dev/null \
        || printf 'transaction groups not read'
}
# What ZFS holds that is not yet on disk, machine-wide (the ARC's
# anonymous buffers are the dirty ones), and how often a transaction
# was delayed or made to wait so far: the txgs kstat's ndirty read 0 at
# every look in run 43157965159 and cannot be what says it.
mem_say() {
    local anon
    anon=$(awk '$1 == "anon_size" { print int($3 / 1048576) }' /proc/spl/kstat/zfs/arcstats 2>/dev/null)
    printf 'ARC anon_size %s MiB, dmu_tx counters above 0: %s' "${anon:-not read}" \
        "$(awk 'NR > 2 && $3 > 0 && $1 != "dmu_tx_assigned" { printf "%s %s ", $1, $3 }' /proc/spl/kstat/zfs/dmu_tx 2>/dev/null || true)"
}
# Where a process waits in the kernel: the first frames of its stack
# that are ZFS's or the block layer's, in one line; the whole stack
# goes to the file.
stack_say() { # pid file
    cat "/proc/$1/stack" > "$2" 2>/dev/null
    if [ -s "$2" ]; then
        sed -n 's/^\[<[^>]*>\] *\([A-Za-z0-9_.]*\)+.*/\1/p' "$2" | head -n 8 | tr '\n' ' '
    else
        printf 'stack not read'
    fi
}
# Seconds between two lines of a kernel log file, by its own stamps:
# from the line that holds MARK to the first line after it that matches
# RE. "none" when there is no such line, "not read" without the mark.
klog_gap() { # file mark re
    awk -v m="$2" -v re="$3" '
        function stamp(l) { if (match(l, /^\[ *[0-9]+\.[0-9]+\]/)) return substr(l, 2, RLENGTH - 2) + 0; return -1 }
        !f && index($0, m) { f = 1; t0 = stamp($0); next }
        f && $0 ~ re { t1 = stamp($0); if (t0 >= 0 && t1 >= 0) printf "%.1f s", t1 - t0; else printf "found, stamps not read"; done = 1; exit }
        END { if (!f) printf "not read"; else if (!done) printf "none" }' "$1" 2>/dev/null
}
# Does the pool let go of the device, at the end of a course: what the
# driver says of the number, who holds the block device, and whether a
# device can be added under the number (only tried when the driver no
# longer knows it; the device added is a null device and is deleted).
rs_release() { # num tag
    local num=$1 tag=$2 a="" h f
    if [ -e "/dev/ublkb$num" ]; then a="block node there"; else a="no block node"; fi
    if lim 20 ublk list -n "$num" > "$OUT/$tag-end-ublk.txt" 2>&1 && grep -q '^dev id' "$OUT/$tag-end-ublk.txt"; then
        a+=", the driver lists number $num as '$(sed -n 's/.*state \([A-Z_]*\).*/\1/p' "$OUT/$tag-end-ublk.txt" | head -n 1)'"
        a+=", so no add under the number was tried"
    else
        a+=", the driver does not know number $num"
        if lim 60 ublk add -t null -n "$num" > "$OUT/$tag-end-add.log" 2>&1; then
            a+=", and an add under it SUCCEEDED (a null device, deleted again: $(lim 60 ublk del -n "$num" > /dev/null 2>&1 && echo 'deleted' || echo 'THE DELETE DID NOT RETURN 0'))"
        else
            a+=", and an add under it failed: $(tail -n 1 "$OUT/$tag-end-add.log" | cut -c1-160)"
        fi
    fi
    h=""; for f in "/sys/block/ublkb$num/holders"/*; do [ -e "$f" ] && h+="${f##*/} "; done
    if [ -d "/sys/block/ublkb$num" ]; then a+="; /sys/block/ublkb$num/holders: ${h:-empty}"; else a+="; no /sys/block/ublkb$num"; fi
    printf '%s' "$a"
}
# The daemon's own words when a start finds the log shorter than what
# was acknowledged: `Reason::Short` in crates/skyzfs-ublk/src/lifecycle.rs.
# A change of that message must change this line on purpose.
SHORT_LOG_SAID="the log is shorter than what was acknowledged"
# The course after a refused start. With the daemon only.
#   refused_start_arm TAG LOGDEV [COURSE]   TAG bs: the control's pool,
#   with a separate log device (yes). TAG bn: the same with none (no).
#   COURSE: "long" (the default) is the course described below. The
#   others are check f's, each on a pool and a volume of its own with
#   nothing but the first writer until the one thing the course does:
#   scrub, export, reopen, clear (that zpool command 10 s after the
#   refusal), zed (the scrub, with zed running), zedreopen (the
#   reopen, with zed running), writers (rs_writers).
# The server is killed at rest, so that the log's last mutation is an
# acknowledged one and `serve --recover-short 1` must refuse. The
# control judges that answer and nothing of the pool. The rest is
# observation: what the pool, the device and I/O to the pool do in the
# 300 s after, with only a writer's synced writes waiting at first; a
# second writer at 120 s; at 180 s a read of a file that cannot be in
# memory (its dataset caches no data); at 240 s a scrub. In runs
# 43100887191 and 43100211375 the pool re-executed its failed writes
# without end and suspended only at the first read.
# -> RS_CTL ("FAIL|PASS|ERROR detail" for the control), RS_OBS.
refused_start_arm() {
    local w tag=$1 logdev=$2 dir=$WORK/vol-$1 pool=skzd-$1 slog="" pid rc i st w0 w1 held unst quiet="" num t0 dev was line="" fails mark zios
    local n0 n1 live="" end io="" inflight sus="" ev seen did="" ll steps step logsay said tw="" tr="" ts="" cold pc
    local course=${3:-long} limit=300 act="" amark="" acted="" ret="" k rel hung="" said_at=-10
    RS_CTL=""; RS_OBS=""
    case $course in long|writers) ;; short) limit=20 ;; *) limit=200 ;; esac
    if ! vol_up "$SKYZFSD" "$dir"; then RS_CTL="ERROR not created or served"; return; fi
    if [ "$logdev" = yes ]; then
        slog=$(new_slog "$tag")
        pool_create "$pool" "$BDEV" "$slog" > "$OUT/$tag-create.log" 2>&1 || { RS_CTL="ERROR zpool create failed"; return; }
    else
        mkdir -p "$WORK/mnt/$pool"
        lim 180 zpool create -f -o ashift=12 -o cachefile=none -o failmode=wait -O mountpoint="$WORK/mnt/$pool" \
            -O compression=off -O atime=off "$pool" "$BDEV" > "$OUT/$tag-create.log" 2>&1 || { RS_CTL="ERROR zpool create failed"; return; }
    fi
    # A file whose data ZFS keeps in no cache: a read of it must reach
    # the device.
    # The read of this file is the probe that shows what makes the pool
    # notice: a file that is not there, or short, would let a read that
    # reached nothing pass as that probe. So the preparation is checked
    # like the pool's creation, and the file's size before the kill.
    if ! lim 60 zfs create -o primarycache=metadata "$pool/cold" > "$OUT/$tag-cold.log" 2>&1; then
        RS_CTL="ERROR the dataset for the uncached file was not made: $(tail -n 1 "$OUT/$tag-cold.log" | cut -c1-200)"; return
    fi
    if ! lim 120 "$DD" if=/dev/urandom of="$WORK/mnt/$pool/cold/file" bs=1M count=32 conv=fsync status=none 2>> "$OUT/$tag-cold.log"; then
        RS_CTL="ERROR the uncached file was not written: $(tail -n 1 "$OUT/$tag-cold.log" | cut -c1-200)"; return
    fi
    cold=$(stat -c %s "$WORK/mnt/$pool/cold/file" 2>/dev/null)
    pc=$(lim 20 zfs get -H -o value primarycache "$pool/cold" 2>/dev/null)
    if [ "$cold" != $((32 * 1024 * 1024)) ] || [ "$pc" != metadata ]; then
        RS_CTL="ERROR the uncached file is not what the read expects: ${cold:-no} bytes (wanted $((32 * 1024 * 1024))), primarycache '${pc:-not read}' (wanted metadata)"; return
    fi
    : > "$WORK/$tag.record"; : > "$OUT/$tag-writer.log"
    writer "$WORK/mnt/$pool" "$WORK/$tag.record" "$OUT/$tag-writer.log" "$WORK/$tag.stop" &
    w=$!
    sleep 5
    touch "$WORK/$tag.stop"
    # The server is serving here and nothing has been done to it yet:
    # a writer that does not end is no part of the control, and the
    # control did not happen.
    if ! job_end "$w" 120; then
        RS_CTL="ERROR the control did not happen: before the kill, with the server serving, the writer had not ended 120 s after it was told to (pool $(pool_state "$pool"), the device $(ublk_state "$BDEV"); the server's last line: $(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-200))"
        return
    fi
    # The writer's stop and this sync are not checked for themselves:
    # the look below is the condition (writes counted, unchanged for a
    # second, nothing held, nothing unstored), and a writer that wrote
    # nothing or a sync that did not finish fails it.
    lim 120 zpool sync "$pool" > "$OUT/$tag-sync.log" 2>&1
    for i in $(seq 1 30); do
        w0=$(vol_stat "$SKYZFSD" "$dir" writes); sleep 1
        w1=$(vol_stat "$SKYZFSD" "$dir" writes); held=$(vol_stat "$SKYZFSD" "$dir" held); unst=$(vol_stat "$SKYZFSD" "$dir" unstored)
        if [[ $w0 =~ ^[0-9]+$ ]] && [ "$w0" = "$w1" ] && [ "$w0" -gt 0 ] && [ "$held" = 0 ] && [ "$unst" = 0 ]; then quiet="$w1 writes, none in flight, unstored 0, for 1 s"; break; fi
    done
    if [ -z "$quiet" ]; then
        RS_CTL="ERROR the server did not come to rest before the kill (writes '${w0:-}' then '${w1:-}', in flight '${held:-}', unstored '${unst:-}')"
        return
    fi
    pid=$(server_pid "$SKYZFSD" "$dir")
    num=${BDEV#/dev/ublkb}
    # What the log held just before the kill, by the daemon's status
    # (keys of the daemon that checkpoints; absent before it): the
    # control needs an acknowledged mutation in the log, and a
    # checkpoint could have taken it out.
    ll=$(vol_stat "$SKYZFSD" "$dir" log_live); steps=$(vol_stat "$SKYZFSD" "$dir" checkpoint_steps); step=$(vol_stat "$SKYZFSD" "$dir" checkpoint_step)
    logsay="status before the kill: log_live '${ll:-no such key}', checkpoint_steps '${steps:-no such key}', checkpoint_step '${step:-no such key}'"
    lim 20 zpool events -c > /dev/null 2>&1
    mark=$(klog_mark)
    kill -9 "$pid" 2>/dev/null
    [ -z "$SERVE_PID" ] || serve_wait 30
    unlink "$WORK/$tag.stop" 2>/dev/null
    writer "$WORK/mnt/$pool" "$WORK/$tag.record" "$OUT/$tag-writer.log" "$WORK/$tag.stop" &
    # What waits at the device when the start is refused: the block
    # layer's own count (reads, writes), while the device still has its
    # node.
    sleep 2
    inflight="state '$(ublk_state "/dev/ublkb$num")', in flight by /sys/block/ublkb$num/inflight (reads writes): $(tr -s ' \n' ' ' < "/sys/block/ublkb$num/inflight" 2>/dev/null || echo 'not read')"
    serve_start "$SKYZFSD" "$dir" --recover-short 1; rc=$?
    if [ $rc = 2 ]; then
        touch "$WORK/$tag.stop"
        RS_CTL="ERROR serve --recover-short 1 neither became ready nor exited"
        return
    elif [ $rc = 0 ]; then
        # The start is refused only if the log's last mutation had been
        # acknowledged. A kill that leaves that write applied and not
        # acknowledged makes the start serve, by design, and the server
        # then says so (recovered_unacknowledged 1 or more): the control
        # did not happen. At rest that should not occur; if it does it
        # is the rig's error. A start that serves and found nothing
        # unacknowledged let a short log through: that is the finding.
        touch "$WORK/$tag.stop"
        i=$(vol_stat "$SKYZFSD" "$dir" recovered_unacknowledged)
        # With 0 it is a finding only if the log's newest part was an
        # acknowledged mutation: with no data record in what is
        # replayed the hook compares the log's end, serves and says 0,
        # rightly. The kill at rest has seen that (this instance's
        # writes above 0, nothing held, unstored 0); a checkpoint step
        # under way could still take the record out, so the status must
        # say `checkpoint_step none`, or have no such key (a daemon
        # that does not checkpoint). log_live is reported and not
        # relied on: a log can hold records that are not mutations.
        said=$(grep 'serving device' "$dir.serve.log" | tail -n 1 | cut -c1-300)
        if [[ ${i:-} =~ ^[0-9]+$ ]] && [ "$i" = 0 ] && { [ "$step" = none ] || [ -z "$ll$steps$step" ]; }; then
            RS_CTL="PASS killed at rest ($quiet; $logsay); serve --recover-short 1 SERVED ($BDEV) and found recovered_unacknowledged 0: a log shorter than what was acknowledged was let through; the start's own line: ${said:-none}"
        elif [[ ${i:-} =~ ^[0-9]+$ ]] && [ "$i" = 0 ]; then
            RS_CTL="ERROR the control did not happen: killed at rest ($quiet), serve --recover-short 1 served with recovered_unacknowledged 0, but a checkpoint step may have taken the last mutation out of the log ($logsay); the start's own line: ${said:-none}"
        else
            RS_CTL="ERROR the control did not happen: killed at rest ($quiet), yet serve --recover-short 1 served and says recovered_unacknowledged '${i:-nothing}': the log's last mutation was not an acknowledged one; the start's own line: ${said:-none}"
        fi
        RS_OBS="the start was not refused, so there is no course after a refusal to report"
        return
    fi
    n0=$(grep -c . "$WORK/$tag.record")
    t0=$(now_ms); was=""
    RS_DID=""; EL=0
    while [ "$EL" -lt "$limit" ]; do
        EL=$(( ($(now_ms) - t0) / 1000 ))
        st=$(pool_state "$pool")
        if [ -e "/dev/ublkb$num" ]; then dev="block node there"; else dev="no block node"; fi
        if lim 20 ublk list -n "$num" 2>/dev/null | grep -q '^dev id'; then dev+=", the driver lists it as '$(ublk_state "/dev/ublkb$num")'"; else dev+=", unknown to the driver"; fi
        case $dev in *LIVE*) live=yes ;; esac
        if [ "$st; $dev" != "$was" ]; then was="$st; $dev"; line+="at $EL s: pool $st, device $num: $dev. "; fi
        if [ "$st" = SUSPENDED ] && [ -z "$sus" ]; then sus=$EL; fi
        # A line every 10 s, so that a reader of the job's log while
        # it runs sees the arm alive.
        if [ "$EL" -ge $((said_at + 10)) ]; then said_at=$EL; echo "-- $tag ($course): $EL s after the refusal: pool $st, device $num: $dev"; fi
        # The short course (check b's arms) only looks: the control
        # needs the refusal and the device live at no look, and its
        # looks are those of these $limit s.
        if [ "$course" = short ]; then sleep 1; continue; fi
        if [ -n "$sus" ] && [ "$course" = long ]; then break; fi
        # A short course goes on for 120 s after the pool suspended, to
        # see whether its command returns and what becomes of the
        # device (20 s were too few to say that a scrub does not
        # return, run 43157965159); when the command returned and the
        # pool has not suspended, for 60 s after the return.
        if [ -n "$sus" ] && [ "$EL" -ge $((sus + 120)) ]; then break; fi
        if [ -z "$ret" ] && [ -e "$WORK/$tag.act" ]; then ret=$EL; fi
        if [ -n "$ret" ] && [ "$EL" -ge $((ret + 60)) ]; then break; fi
        if [ "$course" = writers ]; then rs_writers; sleep 1; continue; fi
        if [ "$course" != long ]; then
            if rs_due 8 s8; then refused_snap "$pool" "$tag" 8s; fi
            if rs_due 10 act; then acted=$EL; rs_act; fi
            sleep 1; continue
        fi
        # Each snapshot is taken before the probe that follows it.
        if rs_due 10 s10; then refused_snap "$pool" "$tag" 10s; fi
        if rs_due 60 s60; then refused_snap "$pool" "$tag" 60s; fi
        if rs_due 110 s110; then refused_snap "$pool" "$tag" 110s; fi
        if rs_due 120 writer2; then
            tw=$(( ($(now_ms) - t0) / 1000 ))
            ( t=$(now_ms); lim 170 "$DD" if=/dev/zero of="$WORK/mnt/$pool/second-writer" bs=1M count=4 conv=fsync status=none 2> "$OUT/$tag-writer2.err"; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.writer2" ) &
        fi
        if rs_due 170 s170; then refused_snap "$pool" "$tag" 170s; fi
        if rs_due 180 reader; then
            tr=$(( ($(now_ms) - t0) / 1000 ))
            ( t=$(now_ms); lim 110 "$DD" if="$WORK/mnt/$pool/cold/file" of=/dev/null bs=1M status=none 2> "$OUT/$tag-reader.err"; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.reader" ) &
        fi
        if rs_due 230 s230; then refused_snap "$pool" "$tag" 230s; fi
        if rs_due 240 scrub; then
            ts=$(( ($(now_ms) - t0) / 1000 ))
            ( t=$(now_ms); lim 50 zpool scrub "$pool" > "$OUT/$tag-scrub.log" 2>&1; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.scrub" ) &
        fi
        sleep 1
    done
    did=$RS_DID
    end=$(( ($(now_ms) - t0) / 1000 ))
    touch "$WORK/$tag.stop"
    refused_snap "$pool" "$tag" end
    n1=$(grep -c . "$WORK/$tag.record")
    fails=$(grep -c . "$OUT/$tag-writer.log")
    klog_since "$mark" > "$OUT/$tag-klog.txt" 2>/dev/null
    zios=$(grep "$pool" "$OUT/$tag-klog.txt" | grep -o 'type=[0-9]*' | sort | uniq -c | tr -s ' \n' ' ')
    ev=$(cat "$OUT/$tag-end-event-classes.txt" 2>/dev/null | tr -s ' \n' ' ')
    seen="zpool status: $(grep -E '^ *(state|status):' "$OUT/$tag-end-status.txt" 2>/dev/null | tr -s ' \n' ' ' | cut -c1-220)$([ -s "$OUT/$tag-end-status.txt" ] || echo 'gave nothing within 20 s'); the first 20000 events ZFS held at the end, by class: ${ev:-not read}; kernel log lines naming ublkb$num or the pool since the kill: $(grep -c -E "ublkb$num|$pool" "$OUT/$tag-klog.txt")"
    case $did in *writer2*) io+="a second writer (4 MiB, synced) at $tw s: $(cat "$WORK/$tag.writer2" 2>/dev/null || echo "had not ended at $end s (hung)"); " ;; esac
    case $did in *reader*) io+="a read of the uncached file (32 MiB, primarycache=metadata, checked before the kill) at $tr s: $(cat "$WORK/$tag.reader" 2>/dev/null || echo "had not ended at $end s (hung)") $(head -c 120 "$OUT/$tag-reader.err" 2>/dev/null | tr '\n' ' '); " ;; esac
    case $did in *scrub*) io+="a scrub at $ts s: $(cat "$WORK/$tag.scrub" 2>/dev/null || echo "had not ended at $end s (hung)") $(head -c 120 "$OUT/$tag-scrub.log" 2>/dev/null | tr '\n' ' '); " ;; esac
    if [ "$course" = writers ]; then
        for k in 1 2 3 4 5 6 7 8 9; do
            [ -s "$WORK/$tag.w$k.sent" ] || continue
            io+="writer $k $(cat "$WORK/$tag.w$k.sent"): $(cat "$WORK/$tag.w$k" 2>/dev/null || echo "had not ended at $end s (hung)") $(head -c 100 "$OUT/$tag-w$k.err" 2>/dev/null | tr '\n' ' '); "
            # Where the first writer that hangs waits, by its dd's stack.
            if [ ! -e "$WORK/$tag.w$k" ] && [ -z "$hung" ]; then
                hung=$(pgrep -f "of=$WORK/mnt/$pool/writer-$k " | head -n 1)
                [ -z "$hung" ] || hung="writer $k's dd (pid $hung) waits in: $(stack_say "$hung" "$OUT/$tag-w$k-stack.txt")"
            fi
        done
        RS_OBS="pool with a separate log device, nothing read and nothing scrubbed; a synced writer of 4 MiB to a file of its own every 30 s. After the refusal (exit $SERVE_RC): ${line}$([ -n "$sus" ] && echo "SUSPENDED at $sus s. " || echo "Never suspended in $end s. ")The writers: ${io:-none was sent}${hung:+Where a hung one waits: $hung. }The first writer: $fails failed writes, $n0 files acknowledged at the refusal and $n1 at the end. At the end: $(dirty_say "$OUT/$tag-end-txgs.txt"); $(txg_say "$OUT/$tag-end-txgs.txt"). Failed I/O of the pool in the kernel log since the kill, by zio type (1 read, 2 write, 5 flush): ${zios:-none}"
    elif [ "$course" = short ]; then
        RS_OBS=""
    elif [ "$course" != long ]; then
        rel=$(rs_release "$num" "$tag")
        RS_OBS="pool with a separate log device$([[ $course == zed* ]] && echo ", zed $(zed_state) with $(pgrep -c -x zed 2>/dev/null || echo 0) process(es)"); only the first writer's synced writes until '$act' at ${acted:-never} s after the refusal (exit $SERVE_RC). The command: $(cat "$WORK/$tag.act" 2>/dev/null || echo "HAD NOT RETURNED at $end s") $(head -c 200 "$OUT/$tag-act.log" 2>/dev/null | tr '\n' ' '). ${line}$([ -n "$sus" ] && echo "SUSPENDED at $sus s by the rig's clock; by the kernel log, 'has been suspended' came $(klog_gap "$OUT/$tag-klog.txt" "$amark" "$pool.*has been suspended") after the command was sent. " || echo "Never suspended in $end s (the kernel log's 'has been suspended' after the command: $(klog_gap "$OUT/$tag-klog.txt" "$amark" "$pool.*has been suspended")). ")At the end, $end s: pool $st, its file system $(mountpoint -q "$WORK/mnt/$pool" && echo 'still mounted' || echo 'NO LONGER MOUNTED (what the first writer wrote after that went to the directory under it)'). Does the pool let go of the device: the vdev in zpool status: $(vdev_say "$OUT/$tag-end-status.txt" "$num"); $rel. Before the command (8 s): vdev $(vdev_say "$OUT/$tag-8s-status.txt" "$num"), $(txg_say "$OUT/$tag-8s-txgs.txt"); at the end: $(txg_say "$OUT/$tag-end-txgs.txt"). The first writer: $fails failed writes, $n0 files acknowledged at the refusal and $n1 at the end. Failed I/O of the pool in the kernel log since the kill, by zio type (1 read, 2 write, 5 flush): ${zios:-none}. What an operator would see at the end: $seen"
    else
    RS_OBS="pool $([ "$logdev" = yes ] && echo 'with a separate log device' || echo 'WITHOUT a separate log device'). Before the refused start, 2 s after a writer began with no server: $inflight. After the refusal (exit $SERVE_RC): ${line}$([ -n "$sus" ] && echo "SUSPENDED at $sus s. " || echo "Never suspended in $end s. ")At the end, $end s: pool $st, the device: $dev. The device's error counters in zpool status at 10 s: $(vdev_say "$OUT/$tag-10s-status.txt" "$num"); 60 s: $(vdev_say "$OUT/$tag-60s-status.txt" "$num"); 110 s: $(vdev_say "$OUT/$tag-110s-status.txt" "$num"); 170 s: $(vdev_say "$OUT/$tag-170s-status.txt" "$num"); 230 s: $(vdev_say "$OUT/$tag-230s-status.txt" "$num"); the end: $(vdev_say "$OUT/$tag-end-status.txt" "$num"). Transaction groups at 10 s: $(txg_say "$OUT/$tag-10s-txgs.txt"); at 60 s: $(txg_say "$OUT/$tag-60s-txgs.txt"); at the end: $(txg_say "$OUT/$tag-end-txgs.txt"). The first writer, which writes 4 MiB files and syncs each: $fails failed writes, $n0 files acknowledged at the refusal and $n1 at the end$([ "$n1" -gt "$n0" ] && echo ' (ACKNOWLEDGED BY THE POOL WITH NO SERVER BEHIND ITS DEVICE)'). Started while it lasted: ${io:-nothing (the pool suspended first)}. Failed I/O of the pool in the kernel log since the kill, by zio type (1 read, 2 write, 5 flush): ${zios:-none}. What an operator would see at the end: $seen. Stacks of ZFS's threads, events and status at 10 s, 60 s and the end (and just before each of the three probes) are in the files $tag-*-stacks.txt, -events.txt, -status.txt"
    # Exit 4 is every refusal of a failed volume. The control is about
    fi
    # this one, so the daemon must have given its own reason for it.
    if [ "$SERVE_RC" = "$EXIT_REFUSED_FAILED" ] && ! grep -q -F "$SHORT_LOG_SAID" "$dir.serve.log"; then
        RS_CTL="ERROR the control did not happen: serve --recover-short 1 exited $SERVE_RC, but not for a log that is too short: its log does not say '$SHORT_LOG_SAID'; its last line: $(tail -n 1 "$dir.serve.log" | cut -c1-300)"
    elif [ "$SERVE_RC" = "$EXIT_REFUSED_FAILED" ] && ! grep -q '^ready' "$dir.ready" && [ -z "$live" ]; then
        RS_CTL="FAIL killed at rest ($quiet; $logsay); serve --recover-short 1 exited $SERVE_RC saying '$SHORT_LOG_SAID' and served nothing: no ready line, the device was live at no look in $end s; the pool at the end: $st (reported, not judged; see the observation of this arm)"
    else
        RS_CTL="PASS killed at rest ($quiet); serve --recover-short 1 exited ${SERVE_RC:-?}; ready line: $(grep -c '^ready' "$dir.ready"); the device live at a look: ${live:-no}; pool $st"
    fi
    # The transaction groups and the txg_sync stacks at the end, in the
    # job's log as well: an artifact can be lost.
    echo "-- $tag: the transaction groups and the txg_sync threads at the end:"; tail -n 4 "$OUT/$tag-end-txgs.txt" 2>/dev/null
    sed -n '1,/^== [0-9]/p' "$OUT/$tag-end-stacks.txt" 2>/dev/null | sed '$d'
    awk '/^== [0-9]+ (txg_sync|spa_async)/ { n = 0; p = 1; print; next } /^== / { p = 0 } p && ++n <= 15' "$OUT/$tag-end-stacks.txt" 2>/dev/null | head -n 200
}
# a: a write of zeros through a checkpoint. 8 MiB of random data, zeros
# over its middle 4 MiB (BLKZEROOUT), then 200 MiB elsewhere so that
# the log passes its high mark and a checkpoint applies the zeros to
# the home image. The 8 MiB are read through the device before the
# checkpoint, after it, and after a kill -9 and a new serve: each time
# the data with zeros in the middle. Once as served by default, where
# the status must then say that the ranges were deallocated, and once
# with --refuse-holes, where it must say that zeros were written
# because the file system cannot deallocate (the status's own words).
#   zero_arm TAG [serve arguments] -> ZA_BAD ("" or what was wrong),
#   ZA_SAID; ZA_ERR set when the arm did not happen.
zero_arm() {
    local tag=$1 dir=$WORK/vol-$1 at=$((1 << 30)) r s0 s1 st za i pid rc
    shift
    ZA_BAD=""; ZA_SAID=""; ZA_ERR=""
    if ! vol_up "$SKYZFSD" "$dir" "$@"; then ZA_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return; fi
    "$DD" if=/dev/urandom of="$WORK/$tag.data" bs=1M count=8 status=none
    cp "$WORK/$tag.data" "$WORK/$tag.want"
    "$DD" if=/dev/zero of="$WORK/$tag.want" bs=1M seek=2 count=4 conv=notrunc status=none
    if ! lim 60 "$DD" if="$WORK/$tag.data" of="$BDEV" bs=1M seek=$((at >> 20)) oflag=direct conv=nocreat,fsync status=none 2> "$OUT/$tag-write.err"; then
        ZA_ERR="the 8 MiB were not written: $(tail -n 1 "$OUT/$tag-write.err" | cut -c1-200)"; kill_cleanup "$tag"; return
    fi
    lim 30 blkdiscard -f -z -o $((at + (2 << 20))) -l $((4 << 20)) "$BDEV" > "$OUT/$tag-zeroout.log" 2>&1; rc=$?
    if [ $rc != 0 ]; then ZA_ERR="blkdiscard -z exit $rc: $(tail -n 1 "$OUT/$tag-zeroout.log" | cut -c1-200)"; kill_cleanup "$tag"; return; fi
    za_read() { # label: the 8 MiB through the device against what they must be
        if ! lim 60 "$DD" if="$BDEV" of="$WORK/$tag.got" bs=1M skip=$((at >> 20)) count=8 iflag=direct status=none 2> "$OUT/$tag-read.err"; then
            ZA_BAD+="the read $1 failed: $(tail -n 1 "$OUT/$tag-read.err" | cut -c1-160); "; ZA_SAID+="read $1: failed; "
        elif cmp -s "$WORK/$tag.got" "$WORK/$tag.want"; then
            ZA_SAID+="read $1: the data with zeros in its middle 4 MiB; "
        else
            ZA_BAD+="the read $1 DIFFERS from the data with zeros in the middle (first difference: $(cmp "$WORK/$tag.got" "$WORK/$tag.want" 2>&1 | cut -c1-120)); "; ZA_SAID+="read $1: DIFFERS; "
        fi
    }
    za_read "before a checkpoint"
    s0=$(vol_stat "$SKYZFSD" "$dir" checkpoint_steps)
    ZA_SAID+="zeroes_applied then '$(vol_status "$SKYZFSD" "$dir" | sed -n 's/^zeroes_applied  *//p' | head -n 1 | cut -c1-60)', checkpoint_steps '${s0:-no such key}', the home image $(du -m "$dir/home.img" 2>/dev/null | cut -f1) MiB on disk; "
    if ! lim 300 "$DD" if=/dev/urandom of="$BDEV" bs=1M seek=$(((at >> 20) + 1024)) count=200 oflag=direct conv=nocreat,fsync status=none 2> "$OUT/$tag-fill.err"; then
        ZA_ERR="the 200 MiB that bring the checkpoint were not written: $(tail -n 1 "$OUT/$tag-fill.err" | cut -c1-200)"; kill_cleanup "$tag"; return
    fi
    # The checkpoint has taken the zeros when steps were counted and
    # none is under way, at two looks a second apart.
    r=""
    for i in $(seq 1 120); do
        s1=$(vol_stat "$SKYZFSD" "$dir" checkpoint_steps); st=$(vol_stat "$SKYZFSD" "$dir" checkpoint_step)
        if [[ $s1 =~ ^[0-9]+$ ]] && [ "$s1" -gt "${s0:-0}" ] && [ "$st" = none ]; then
            if [ "$r" = "$s1" ]; then rc=rest; break; fi
            r=$s1
        else
            r=""
        fi
        sleep 1
    done
    if [ "$rc" != rest ]; then
        ZA_ERR="no checkpoint came to rest within 120 s of 200 MiB written (checkpoint_steps '${s0:-}' then '${s1:-}', checkpoint_step '${st:-}', log_live '$(vol_stat "$SKYZFSD" "$dir" log_live)')"
        kill_cleanup "$tag"; return
    fi
    # The line's whole text: the status says it in words (`said` of
    # the zeroes in crates/skyzfs-ublk/src/status.rs), and the two
    # ways of writing zeros begin alike.
    za=$(vol_status "$SKYZFSD" "$dir" | sed -n 's/^zeroes_applied  *//p' | head -n 1 | cut -c1-120)
    ZA_SAID+="after 200 MiB more: checkpoint_steps $s1, log_live $(vol_stat "$SKYZFSD" "$dir" log_live), zeroes_applied '${za:-no such key}', the home image $(du -m "$dir/home.img" 2>/dev/null | cut -f1) MiB on disk; "
    ZA_APPLIED=$za
    za_read "after the checkpoint"
    pid=$(server_pid "$SKYZFSD" "$dir")
    kill -9 "$pid" 2>/dev/null
    [ -z "$SERVE_PID" ] || serve_wait 30
    if ! serve_start "$SKYZFSD" "$dir" "$@"; then
        ZA_BAD+="after a kill -9 the volume was not served again (serve rc ${SERVE_RC:-none}); "
    else
        za_read "after a kill -9 and a new serve"
    fi
    kill_cleanup "$tag"
}
check_a_zeroes() {
    if in_place; then pin write_zeroes_through_a_checkpoint NA "the in-place store takes no checkpoint, and refuses --refuse-holes"; return; fi
    local plain hard bad=""
    step "a: a write of zeros through a checkpoint"
    if [ "$IMPL" = reference ]; then
        pin write_zeroes_through_a_checkpoint WAITING "the stand-in has no checkpoint and no home image"
        return
    fi
    ZA_APPLIED=""; zero_arm az
    if [ -n "$ZA_ERR" ]; then pin write_zeroes_through_a_checkpoint ERROR "served by default: $ZA_ERR"; return; fi
    plain="served by default: $ZA_SAID"; bad+=$ZA_BAD
    [[ $ZA_APPLIED == "deallocated in the home image"* ]] || bad+="served by default the status said zeroes_applied '${ZA_APPLIED:-nothing}' after the checkpoint, not 'deallocated in the home image'; "
    ZA_APPLIED=""; zero_arm azr --refuse-holes
    if [ -n "$ZA_ERR" ]; then pin write_zeroes_through_a_checkpoint ERROR "with --refuse-holes: $ZA_ERR ($plain)"; return; fi
    hard="with --refuse-holes: $ZA_SAID"; bad+=$ZA_BAD
    [[ $ZA_APPLIED == "written as zeros: the file system of the volume's directory cannot deallocate"* ]] || bad+="with --refuse-holes the status said zeroes_applied '${ZA_APPLIED:-nothing}' after the checkpoint, not 'written as zeros: the file system of the volume's directory cannot deallocate'; "
    if [ -z "$bad" ]; then pin write_zeroes_through_a_checkpoint PASS "$plain $hard"; else pin write_zeroes_through_a_checkpoint FAIL "$bad $plain $hard"; fi
}
# a: a volume on a file system whose blocks are larger than the page
# (XFS with 16 KiB blocks, on a loop device). `create` there makes a
# volume whose write unit is the file system's block and says so; a
# volume made on 4 KiB blocks and copied there is refused by `serve`,
# with the reason and with its files untouched; the volume made there
# serves a pool that takes a synced write and scrubs clean.
check_a_xfs16k() {
    local img=$WORK/x16.img mnt=$WORK/x16 loop dir=$WORK/x16/vol small=$WORK/vol-ax copy=$WORK/x16/copied pool=skzd-ax
    local a="" bad="" rc said before after st i bs
    step "a: a volume on a file system with 16 KiB blocks"
    if [ "$IMPL" = reference ]; then
        pin volume_on_larger_blocks WAITING "the stand-in has no write unit"
        return
    fi
    ax_end() { # everything this check made, each step under a limit
        [ "$(pool_state "$pool")" = ONLINE ] && lim 120 zpool export "$pool" > /dev/null 2>&1
        [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
        vol_detach "$SKYZFSD" "$dir"
        lim 60 umount "$mnt" > /dev/null 2>&1
        [ -z "${loop:-}" ] || lim 30 losetup -d "$loop" > /dev/null 2>&1
    }
    modprobe xfs 2>/dev/null
    truncate -s 20G "$img"; mkdir -p "$mnt"
    loop=$(losetup --find --show --direct-io=on "$img" 2> "$OUT/ax-setup.log") || loop=""
    if [ -z "$loop" ] || ! lim 300 mkfs.xfs -q -f -b size=16k "$loop" >> "$OUT/ax-setup.log" 2>&1 || ! lim 60 mount "$loop" "$mnt" >> "$OUT/ax-setup.log" 2>&1; then
        pin volume_on_larger_blocks ERROR "an XFS with 16 KiB blocks was not made and mounted here: $(tail -n 1 "$OUT/ax-setup.log" | cut -c1-200)"
        ax_end; return
    fi
    bs=$(stat -f -c %S "$mnt")
    if [ "$bs" != 16384 ]; then
        pin volume_on_larger_blocks ERROR "the file system made says a block of '$bs' bytes, not 16384"
        ax_end; return
    fi
    # 1: create there
    lim 120 "$SKYZFSD" create --dir "$dir" --size "$VOL_BYTES" > "$OUT/ax-create.out" 2> "$OUT/ax-create.err"; rc=$?
    said=$(grep -m 1 "write unit is 16384 bytes" "$OUT/ax-create.err" | cut -c1-200)
    a+="create on it: exit $rc, $([ -n "$said" ] && echo "said: $said" || echo "did NOT say the write unit is 16384 bytes (stderr: $(tail -n 1 "$OUT/ax-create.err" | cut -c1-200))"); "
    { [ $rc = 0 ] && [ -n "$said" ]; } || bad+="create; "
    # 2: a volume of 4 KiB units, copied there, must not be served
    if ! vol_create "$SKYZFSD" "$small" > "$OUT/ax-small.log" 2>&1 || ! lim 300 cp -a --sparse=always "$small" "$copy"; then
        pin volume_on_larger_blocks ERROR "the volume to copy was not made or not copied: $(tail -n 1 "$OUT/ax-small.log" | cut -c1-200) ($a)"
        ax_end; return
    fi
    before=$(cd "$copy" && sha256sum -- * 2>&1 | sort -k2 | sha256sum | cut -c1-16)
    serve_start "$SKYZFSD" "$copy"; rc=$?
    said=$(grep -m 1 "larger than the volume's write unit" "$copy.serve.log" | cut -c1-240)
    if [ $rc = 0 ]; then
        a+="the copied volume (unit of the copy's source: 4096) WAS SERVED as $BDEV; "; bad+="the copy was served; "
        kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 60; vol_detach "$SKYZFSD" "$copy"
    else
        a+="serve of a volume made on 4 KiB blocks and copied there: exit ${SERVE_RC:-none}, $([ -n "$said" ] && echo "said: $said" || echo "did NOT give the reason (its last line: $(tail -n 1 "$copy.serve.log" | cut -c1-200))"); "
        { [ $rc = 1 ] && [ "$SERVE_RC" = 1 ] && [ -n "$said" ]; } || bad+="the refusal; "
    fi
    after=$(cd "$copy" && sha256sum -- * 2>&1 | sort -k2 | sha256sum | cut -c1-16)
    a+="the copy's files after it: $([ "$before" = "$after" ] && echo 'as before' || echo 'CHANGED'); "
    [ "$before" = "$after" ] || bad+="the copy's files changed; "
    # 3: the volume made there, under a pool
    if ! serve_start "$SKYZFSD" "$dir"; then
        a+="the volume made there was NOT served (serve rc ${SERVE_RC:-none}: $(tail -n 1 "$dir.serve.log" | cut -c1-200)); "; bad+="serve; "
    else
        a+="the volume made there serves as $BDEV with logical blocks of $(cat "/sys/block/${BDEV#/dev/}/queue/logical_block_size" 2>/dev/null) and physical of $(cat "/sys/block/${BDEV#/dev/}/queue/physical_block_size" 2>/dev/null); "
        mkdir -p "$WORK/mnt/$pool"
        if ! lim 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$WORK/mnt/$pool" -O compression=off "$pool" "$BDEV" > "$OUT/ax-pool.log" 2>&1; then
            a+="zpool create FAILED: $(tail -n 1 "$OUT/ax-pool.log" | cut -c1-200); "; bad+="the pool; "
        else
            lim 120 "$DD" if=/dev/urandom of="$WORK/mnt/$pool/file" bs=1M count=64 conv=fsync status=none 2> "$OUT/ax-write.err"; rc=$?
            lim 60 zpool scrub "$pool" > "$OUT/ax-scrub.log" 2>&1
            for i in $(seq 1 120); do
                lim 30 zpool status "$pool" > "$OUT/ax-status.txt" 2>&1
                grep -q 'scrub repaired' "$OUT/ax-status.txt" && break
                sleep 1
            done
            st=$(pool_errors "$pool")
            a+="a pool on it: 64 MiB written and synced with exit $rc, then a scrub: $st; "
            { [ $rc = 0 ] && grep -q 'scrub repaired 0B' "$OUT/ax-status.txt" && [[ $st == "vdev errors 0 "* ]] && [[ $st == *"No known data errors" ]]; } || bad+="the write or the scrub; "
        fi
    fi
    ax_end
    if [ -z "$bad" ]; then pin volume_on_larger_blocks PASS "$a"; else pin volume_on_larger_blocks FAIL "wrong: $bad$a"; fi
}
# With the daemon: the short-recovery control and what follows a refused
# start, on a pool with a separate log device and on one without.
check_b_short() {
    # Each arm's outcome is a judged control line of its own: an arm
    # that did not happen is ERROR there, which is the rig's error, and
    # one whose start served is PASS, which differs from the table. So
    # the observation beside it may say "nothing to report" when its
    # arm ended before a refusal: the control's line carries why, and
    # no outcome but "refused, and the course observed" leaves the run
    # green.
    refused_start_arm bs yes short
    ctl recovered_log_too_short "${RS_CTL%% *}" "${RS_CTL#* }"
    refused_start_arm bn no short
    ctl recovered_log_too_short_no_log_device "${RS_CTL%% *}" "${RS_CTL#* }"
}
# f: what follows a refused start, one short course each. Observations
# only, with the daemon only. Each course is the control of check b
# again on a pool of its own (a kill at rest, `serve --recover-short 1`
# refused), and an observation is made only if the start was refused
# as the control wants: otherwise the line says "not run" and why,
# which is the rig's error. The control itself is one judged line for
# all the courses, ctl_f_starts_refused.
check_f() {
    local c tag ld course zed_was zed_ok="" said="" worst=FAIL
    rec f_courses "$F_COURSES"
    for c in $F_ALL; do
        tag=f${c:0:1}; ld="yes"; course=$c
        case $c in
            zedreopen) tag=fy ;;
            # Check b's arms as they were until M1.T8: 300 s, a second
            # writer, a read that must reach the device, a scrub.
            long) course=long ;;
            longnolog) tag="fg"; ld="no"; course=long ;;
        esac
        # A course that was not asked for has its line, and the line
        # says so: one that is missing is the rig's error.
        if [[ " $F_COURSES " != *" $c "* ]]; then
            obs "f_refused_start_$c" "not asked for in this run, whose courses are: $F_COURSES"
            continue
        fi
        step "f: after a refused start, the course '$c'"
        if [ "$c" = zed ] || [ "$c" = zedreopen ]; then
            # zed is not among the rig's packages: it is installed for
            # this course, runs during it alone and is stopped after.
            zed_was=$(zed_state)
            if net_try apt-get install -y --no-install-recommends zfs-zed > "$OUT/f-zed-apt.log" 2>&1 \
                && lim 60 systemctl start zfs-zed > "$OUT/f-zed-start.log" 2>&1 && [ "$(zed_state)" = active ]; then
                zed_ok=yes
            else
                obs "f_refused_start_$c" "not run: zed could not be installed and started (it was '$zed_was' before): $(tail -n 1 "$OUT/f-zed-apt.log" | cut -c1-160); $(tail -n 1 "$OUT/f-zed-start.log" 2>/dev/null | cut -c1-160)"
                continue
            fi
        fi
        refused_start_arm "$tag" "$ld" "$course"
        # The courses' control, one judged line for all: FAIL when
        # every start was refused, as check b's control has it; PASS if
        # one served; ERROR if one did not happen.
        said+="$c: ${RS_CTL%% *}; "
        case ${RS_CTL%% *} in
            FAIL) ;;
            PASS) [ "$worst" = ERROR ] || worst=PASS; said+="(${RS_CTL#* }) " ;;
            *) worst=ERROR; said+="(${RS_CTL#* }) " ;;
        esac
        if [ "${RS_CTL%% *}" = FAIL ]; then
            obs "f_refused_start_$c" "$RS_OBS"
        else
            obs "f_refused_start_$c" "not run: the start was not refused as the control wants: $RS_CTL"
        fi
        if [ -n "$zed_ok" ]; then lim 60 systemctl stop zfs-zed > /dev/null 2>&1; rec "f_zed_after_$c" "$(zed_state)"; zed_ok=""; fi
    done
    ctl f_starts_refused "$worst" "the start before each course (serve --recover-short 1 after a kill at rest), by course: $said"
}
# b: the daemon keeps its files off any pool. A volume directory or a
# run directory on a ZFS dataset is refused by `serve` before a device
# exists: a pool whose device's files lie on a pool can wait for itself.
# The volume is made off ZFS and copied, so that only `serve` is asked.
check_b_on_zfs() {
    local dir=$WORK/vol-b8 z=$WORK/mnt/skzd-z k d r rc said before after bad="" a=""
    step "b: serve with its files on a ZFS dataset"
    if [ "$IMPL" = reference ]; then
        pin serve_on_zfs_refused WAITING "the stand-in does not look where its files are"
        return
    fi
    truncate -s 512M "$WORK/z.img"
    if ! vol_create "$SKYZFSD" "$dir" > "$OUT/b8-create.log" 2>&1 \
        || ! lim 60 zpool create -f -m "$z" skzd-z "$WORK/z.img" > "$OUT/b8-zpool.log" 2>&1 \
        || ! cp -a "$dir" "$z/vol" || ! mkdir "$z/run" "$WORK/b8-run"; then
        pin serve_on_zfs_refused ERROR "the volume, the pool or the copy was not made: $(tail -n 1 "$OUT/b8-zpool.log")"
        lim 60 zpool destroy -f skzd-z > /dev/null 2>&1
        return
    fi
    for k in volume run; do
        if [ $k = volume ]; then d=$z/vol; r=$WORK/b8-run; else d=$dir; r=$z/run; fi
        before=$(lim 20 ublk list 2>/dev/null | grep -c '^dev id')
        lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$d" --run-dir "$r" > "$OUT/b8-$k.out" 2> "$OUT/b8-$k.err"; rc=$?
        after=$(lim 20 ublk list 2>/dev/null | grep -c '^dev id')
        said=$(grep -m 1 'is on ZFS' "$OUT/b8-$k.err" | cut -c1-200)
        a+="$k directory on a dataset: serve exit $rc, $([ -n "$said" ] && echo "said: $said" || echo "did NOT say it is on ZFS (stderr's end: $(tail -n 1 "$OUT/b8-$k.err" | cut -c1-200))"), ublk devices $before then $after$(grep -q '^ready' "$OUT/b8-$k.out" && echo ', IT SAID READY'); "
        { [ $rc != 0 ] && [ $rc != 124 ] && [ -n "$said" ] && [ "$before" = "$after" ] && ! grep -q '^ready' "$OUT/b8-$k.out"; } || bad+="$k "
        # A serve that was not refused may have left a device.
        lim 90 "$SKYZFSD" detach --dir "$d" --run-dir "$r" > /dev/null 2>&1
    done
    if [ -z "$bad" ]; then pin serve_on_zfs_refused PASS "$a"; else pin serve_on_zfs_refused FAIL "not refused as wanted for: $bad; $a"; fi
    lim 60 zpool destroy -f skzd-z > /dev/null 2>&1
}
# b: an orderly stop under write load. The older pin stops a server
# that has nothing in hand. Here a writer is writing and syncing when
# the SIGTERM comes: the server ends with 0, the next serve of the same
# boot finds nothing applied and not acknowledged (an orderly stop ends
# what it has in hand), no write of the writer fails, and every file it
# was told is written reads back. An exit other than 0 is reported as
# such, whatever the rest says.
check_b_stop_under_load() {
    local dir=$WORK/vol-b7 pool=skzd-b7 slog w rc i w0 w1 rec st files fails a stuck=""
    step "b: an orderly stop under write load"
    if [ "$IMPL" = reference ]; then
        pin orderly_stop_under_load WAITING "the stand-in stops by being killed, so its next server can find requests in hand"
        return
    fi
    if ! vol_up "$SKYZFSD" "$dir"; then pin orderly_stop_under_load ERROR "not created or served: ${SERVE_RC:-}"; return; fi
    slog=$(new_slog b7)
    if ! pool_create "$pool" "$BDEV" "$slog" > "$OUT/b7-create.log" 2>&1; then pin orderly_stop_under_load ERROR "zpool create failed"; kill_cleanup b7; return; fi
    : > "$WORK/b7.record"; : > "$OUT/b7-writer.log"
    writer "$WORK/mnt/$pool" "$WORK/b7.record" "$OUT/b7-writer.log" "$WORK/b7.stop" &
    w=$!
    sleep 5
    # The stop must find the writer at work: writes counted by the
    # server grow over the half second before the signal.
    w0=$(vol_stat "$SKYZFSD" "$dir" writes); sleep 0.5; w1=$(vol_stat_least "$SKYZFSD" "$dir" writes $(( ${w0:-0} + 1 )) 3000)
    kill -TERM "$SERVE_PID" 2>/dev/null
    if ! serve_wait 60; then
        # The verdict is the stop that did not come. What follows is
        # the way out, each step under a limit: the next serve may not
        # come either, and the writer's write then never returns.
        touch "$WORK/b7.stop"
        kill -9 "$SERVE_PID" 2>/dev/null; serve_wait 30; serve_start "$SKYZFSD" "$dir"; i=$?
        if job_end "$w" 120; then stuck="the writer ended"; else stuck="THE WRITER HAD NOT ENDED 120 s after it was told to, and was killed"; fi
        pin orderly_stop_under_load FAIL "serve did not exit within 60 s of SIGTERM under load; it was then killed and served again with rc $i (serve exit ${SERVE_RC:-none}): $stuck; pool $(pool_state "$pool")"
        kill_cleanup b7
        return
    fi
    rc=$SERVE_RC
    serve_start "$SKYZFSD" "$dir"; i=$?
    rec=$(vol_stat "$SKYZFSD" "$dir" recovered_unacknowledged)
    sleep 5
    # Not a bare wait: when the serve above did not come back, the
    # device waits for a server, the writer's synced write sleeps in
    # the kernel and the stop file does not end it.
    touch "$WORK/b7.stop"
    job_end "$w" 120 || stuck="THE WRITER HAD NOT ENDED 120 s after it was told to, and was killed; "
    st=$(pool_state "$pool")
    fails=$(grep -c . "$OUT/b7-writer.log")
    files=$(lim 120 files_match "$WORK/mnt/$pool" "$WORK/b7.record" 2>&1 | tail -n 1)
    a="the server had counted ${w0:-?} then ${w1:-?} writes just before SIGTERM; serve exit $rc; serve again rc $i$([ $i = 0 ] || echo " (THE SERVER DID NOT COME BACK: serve exit ${SERVE_RC:-none})"), recovered_unacknowledged '${rec:-not said}'; ${stuck}the writer's failed writes: $fails; pool $st, $(pool_errors "$pool"); files: ${files:-not read in 120 s}"
    if ! { [[ ${w0:-} =~ ^[0-9]+$ ]] && [[ ${w1:-} =~ ^[0-9]+$ ]] && [ "$w1" -gt "$w0" ]; }; then
        pin orderly_stop_under_load ERROR "the stop did not find the writer at work: $a"
    elif [ "$rc" = 0 ] && [ $i = 0 ] && [ -z "$stuck" ] && [ "$rec" = 0 ] && [ "$fails" = 0 ] && [ "$st" = ONLINE ] && [[ $files == ok\ * ]]; then
        pin orderly_stop_under_load PASS "$a"
    elif [ "$rc" != 0 ]; then
        pin orderly_stop_under_load FAIL "serve ended with $rc on SIGTERM under plain load (wanted 0): $a; its log's end: $(tail -n 2 "$dir.serve.log" | tr '\n' ';' | cut -c1-300)"
    else
        pin orderly_stop_under_load FAIL "$a"
    fi
    kill_cleanup b7
}
# b: `detach` of a stopped device that a process still holds open. The
# daemon deletes it and says so with its own exit status (6): the
# number stays taken until the holder lets go, and the operator must
# know that from the status and not from a later failure. "Free" is
# then an add under the number that succeeds, not only a lookup that
# finds nothing (the two differ by some milliseconds; see the control
# rig's free_for_add).
EXIT_DELETED_HELD=6
# b: kill -9 while a checkpoint step waits at a point (test hook
# --step-pause-until with --step-pause-at), at each of the five points
# twice. Every kill is sent only when the daemon has made PATH.reached,
# its own word that a step stands at the point; what the status said of
# a step is written down beside it and not relied on. The verdict is a
# kill test's: no application write failed, no vdev error, a clean
# scrub, every file as recorded. Fewer than ten such kills is the rig's
# error. 192 MiB are written first, so that the log passes its high
# mark and steps begin.
check_b_steps() {
    local dir=$WORK/vol-bk pool=skzd-bk mnt=$WORK/mnt/skzd-bk p=$WORK/bk.pause slog i pid w syncer t0 ms under wr rc
    local landed=0 said="" err="" state vdev scrub files fw why=""
    local -a pts=(saved written synced start-written start-durable saved written synced start-written start-durable)
    if [ "$IMPL" != daemon ]; then pin kills_inside_steps_lose_nothing WAITING "the stand-in has no checkpoint"; return; fi
    if in_place; then pin kills_inside_steps_lose_nothing NA "the in-place store takes no checkpoint step, and refuses the --step-pause-* hooks"; return; fi
    step "b: kill -9 while a checkpoint step waits, at each of its five points, twice"
    unlink "$p" 2>/dev/null; unlink "$p.reached" 2>/dev/null
    if ! vol_up "$SKYZFSD" "$dir" --step-pause-until "$p" --step-pause-at "${pts[0]}"; then
        pin kills_inside_steps_lose_nothing ERROR "not created or served (serve rc ${SERVE_RC:-none}): $(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-200)"; return
    fi
    slog=$(new_slog bk)
    if ! pool_create "$pool" "$BDEV" "$slog" > "$OUT/bk-create.log" 2>&1; then pin kills_inside_steps_lose_nothing ERROR "zpool create failed"; return; fi
    : > "$WORK/bk.record"; : > "$OUT/bk-writer.log"
    [ ! -e "$WORK/bk.stop" ] || unlink "$WORK/bk.stop"
    writer "$mnt" "$WORK/bk.record" "$OUT/bk-writer.log" "$WORK/bk.stop" &
    w=$!
    ( while [ ! -e "$WORK/bk.stop" ] && [ ! -e "$WORK/stop-all" ]; do zpool sync "$pool" 2>/dev/null; sleep 0.1; done ) &
    syncer=$!
    # The fill, recorded like the writer's files: it is in flight
    # across kills and must come through whole.
    ( "$DD" if=/dev/urandom of=/dev/shm/skz-bk-fill bs=1M count=192 status=none
      i=$(sha256sum < /dev/shm/skz-bk-fill | cut -d' ' -f1)
      if lim 1200 "$DD" if=/dev/shm/skz-bk-fill of="$mnt/fill" bs=1M conv=fsync status=none 2>> "$OUT/bk-writer.log"; then echo "fill $i" >> "$WORK/bk.record"
      else echo "write of fill failed" >> "$OUT/bk-writer.log"; fi
      unlink /dev/shm/skz-bk-fill 2>/dev/null ) &
    for i in 0 1 2 3 4 5 6 7 8 9; do
        t0=$(now_ms)
        while [ ! -e "$p.reached" ] && [ $(( $(now_ms) - t0 )) -lt 300000 ] && kill -0 "$SERVE_PID" 2>/dev/null; do sleep 0.1; done
        ms=$(( $(now_ms) - t0 ))
        if [ ! -e "$p.reached" ]; then
            err="kill $((i + 1)), point ${pts[i]}: no step arrived at the point in $ms ms (the server $(kill -0 "$SERVE_PID" 2>/dev/null && echo 'alive' || echo 'gone'); log_live $(vol_stat "$SKYZFSD" "$dir" log_live), checkpoint_steps $(vol_stat "$SKYZFSD" "$dir" checkpoint_steps); its log's end: $(tail -n 1 "$dir.serve.log" | cut -c1-200))"
            break
        fi
        under=$(vol_status "$SKYZFSD" "$dir" | awk '$1 == "checkpoint_step" { $1 = ""; print; exit }' | tr -s ' ')
        read -r _ wr < "/sys/block/$(basename "$BDEV")/inflight" 2>/dev/null || wr="?"
        pid=$(server_pid "$SKYZFSD" "$dir")
        if ! kill -9 "$pid" 2>/dev/null; then err="kill $((i + 1)), point ${pts[i]}: kill -9 '$pid' failed"; break; fi
        [ -z "$SERVE_PID" ] || serve_wait 30 || kill -9 "$SERVE_PID" 2>/dev/null
        unlink "$p" 2>/dev/null; unlink "$p.reached" 2>/dev/null
        landed=$((landed + 1))
        said+="${pts[i]}: a step stood there after $ms ms, the status said of a step '${under# }', $wr writes in flight at the device; "
        t0=$(now_ms)
        if [ "$i" -lt 9 ]; then serve_start "$SKYZFSD" "$dir" --step-pause-until "$p" --step-pause-at "${pts[i + 1]}"; rc=$?
        else serve_start "$SKYZFSD" "$dir"; rc=$?; fi
        said+="served again rc $rc in $(( $(now_ms) - t0 )) ms. "
        if [ $rc != 0 ]; then
            err="after kill $((i + 1)) at ${pts[i]}: serve did not come back (rc $rc, serve exit ${SERVE_RC:-none}): $(tail -n 1 "$dir.serve.log" | cut -c1-200)"
            break
        fi
    done
    touch "$WORK/bk.stop"
    if job_end "$w" 120; then fw=$(grep -c failed "$OUT/bk-writer.log" || true); else fw=stuck; fi
    kill -9 "$syncer" 2>/dev/null; pkill -9 -f "zpool sync $pool" 2>/dev/null
    # The fill may still be on its way: it has until the scrub's end.
    for i in $(seq 1 240); do grep -q '^fill \|of fill failed' "$WORK/bk.record" "$OUT/bk-writer.log" && break; sleep 0.5; done
    state=$(pool_state "$pool"); vdev=$(pool_errors "$pool")
    [ "$state" = ONLINE ] || lim 120 zpool clear "$pool" > /dev/null 2>&1
    lim 900 zpool scrub -w "$pool" > "$OUT/bk-scrub.log" 2>&1
    scrub=$(pool_errors "$pool")
    files=$(lim 300 files_match "$mnt" "$WORK/bk.record" 2>&1)
    [ $? != 124 ] || files="not read: the pool ($state after the kills, $(pool_state "$pool") now) did not answer in 300 s"
    grep -q '^fill ' "$WORK/bk.record" || files+="; the fill of 192 MiB is not among them: it had not ended"
    rec bk_rounds "${said:-none}"
    if [ -n "$err" ]; then
        pin kills_inside_steps_lose_nothing ERROR "$err; kills sent with a step at its point before that: $landed"
    elif [ "$landed" -lt 10 ]; then
        pin kills_inside_steps_lose_nothing ERROR "only $landed kills were sent with a step at its point; ten are needed"
    else
        [ "$fw" = 0 ] || why+="application writes failed: $fw; "
        case "$state/$vdev" in "ONLINE/vdev errors 0 "*) ;; *) why+="after the kills: pool $state, $vdev; " ;; esac
        case "$scrub" in "vdev errors 0 "*"with 0 errors"*) ;; *) why+="scrub: $scrub; " ;; esac
        case "$files" in ok*";"*) why+="files: $files; " ;; ok*) ;; *) why+="files: $files; " ;; esac
        if [ -z "$why" ]; then
            pin kills_inside_steps_lose_nothing PASS "ten kills, each sent while a step stood at its point by the daemon's own mark (saved, written, synced, start-written, start-durable, twice): no application write failed, no error on the vdev, scrub clean, $files (the 192 MiB fill among them); see bk_rounds"
        else
            pin kills_inside_steps_lose_nothing FAIL "after ten kills, each sent while a step stood at its point: $why see bk_rounds"
        fi
    fi
    kill_cleanup bk
}
check_b_detach_held() {
    local dir=$WORK/vol-b6 num holder rc st t0 ms said taken=0 free="" a
    step "b: detach of a stopped device a process holds open"
    if [ "$IMPL" = reference ]; then
        pin detach_of_held_says_6 WAITING "the stand-in deletes with the reference server's tool, which waits for the holder"
        return
    fi
    if ! vol_up "$SKYZFSD" "$dir"; then pin detach_of_held_says_6 ERROR "not created or served: ${SERVE_RC:-}"; return; fi
    num=${BDEV#/dev/ublkb}
    # The holder opens the device while it is served, reads nothing and
    # keeps it open.
    # shellcheck disable=SC2217  # the open is the point, not the input
    sleep 600 < "$BDEV" &
    holder=$!
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    if [ -n "$SERVE_PID" ]; then pin detach_of_held_says_6 ERROR "the server did not stop on SIGTERM"; kill "$holder" 2>/dev/null; return; fi
    st=$(ublk_state "/dev/ublkb$num")
    t0=$(now_ms)
    lim 90 "$SKYZFSD" detach --dir "$dir" "${VOL_RUN_ARGS[@]}" > "$OUT/b-detach-held.out" 2> "$OUT/b-detach-held.err"; rc=$?
    ms=$(( $(now_ms) - t0 ))
    said=$(tail -n 1 "$OUT/b-detach-held.err" | cut -c1-200)
    if ublk_number_free "$num"; then taken=0; else taken=1; fi
    if ! kill -0 "$holder" 2>/dev/null; then
        pin detach_of_held_says_6 ERROR "the holder was gone before the detach was judged"
        return
    fi
    # Its end closes a deleted device: under a limit like every wait
    # on something that touches a device. If it has not ended, no add
    # succeeds below and the line says so.
    kill "$holder" 2>/dev/null; job_end "$holder" 30
    t0=$(now_ms)
    while [ $(( $(now_ms) - t0 )) -lt 5000 ]; do
        if lim 60 ublk add -t null -n "$num" > "$OUT/b-detach-held-add.log" 2>&1; then
            free="an add under $num succeeded $(( $(now_ms) - t0 )) ms after the holder closed it"
            lim 60 ublk del -n "$num" > /dev/null 2>&1
            break
        fi
        sleep 0.05
    done
    a="the device was $st with no server and held open by a process; detach exit $rc after $ms ms ($said); the number was $([ $taken = 1 ] && echo 'still taken' || echo 'FREE') while held; ${free:-no add under $num succeeded in 5 s after the holder closed it: $(tail -n 1 "$OUT/b-detach-held-add.log")}"
    if [ "$rc" = "$EXIT_DELETED_HELD" ] && [ $taken = 1 ] && [ -n "$free" ]; then
        pin detach_of_held_says_6 PASS "$a"
    else
        pin detach_of_held_says_6 FAIL "$a (wanted exit $EXIT_DELETED_HELD, the number taken while held, free after)"
    fi
}
# The kill test with a server that acknowledges a write some
# milliseconds after applying it: every kill then leaves writes applied
# and not acknowledged, which the next server is given again. The pool
# must come through as in the plain test, and the servers must have
# found such writes, or the test did not happen. The control takes the
# same delay before the write is applied: nothing is then left applied
# and unacknowledged, and the count must show it.
kill_cleanup() { # name [impl]: the pool exported, the server stopped, the device detached
    [ "$(pool_state "skzd-$1")" = ONLINE ] && lim 120 zpool export "skzd-$1"
    [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
    vol_detach "${2:-$SKYZFSD}" "$WORK/vol-$1"
}
check_b_delay() {
    local ms=${RIG_COMMIT_DELAY_MS:-20} kills=10 v why
    if in_place; then
        pin kill_under_load_applied_unacknowledged NA "the in-place store keeps no record of what it applied, so a new server finds nothing applied and not acknowledged; kills under load are pin_kill_under_load"
        ctl delay_before_apply NA "the control of pin_kill_under_load_applied_unacknowledged, which is not taken with the in-place store"
        return
    fi
    step "b: kill -9 under load, a write acknowledged $ms ms after it is applied, $kills times"
    KT_SERVE_EXTRA=(--commit-delay-ms "$ms")
    kill_test "$SKYZFSD" bd "$kills"
    KT_SERVE_EXTRA=()
    kill_verdict; v=$KT_VERDICT
    why="${KT_WHY:-no application write failed, no error on the vdev, scrub clean, $KT_FILES}; the servers found $KT_RECOVERED requests applied and not acknowledged over $KT_DONE take-overs ($KT_RECOVERED_UNSAID did not say)"
    if [ "$v" = PASS ] && { [ "$KT_RECOVERED" = 0 ] || [ "$KT_RECOVERED_UNSAID" != 0 ]; }; then v=ERROR; fi
    pin kill_under_load_applied_unacknowledged "$v" "$why"
    kill_cleanup bd
    # Only the stand-in can take the delay before it applies a write.
    step "b, control: the same delay before the write is applied"
    KT_SERVE_EXTRA=(--commit-delay-ms "$ms")
    SKZ_REF_COMMIT_DELAY_BEFORE=1 kill_test "$REF" bd0 5
    KT_SERVE_EXTRA=()
    # The control's FAIL is "the kills found nothing". It means that
    # only if the kills happened, all of them, with writes in flight,
    # as the pin's verdict requires, and left the pool whole: this
    # server is slow and not wrong. Anything else is the rig's error.
    kill_verdict
    if [ "$KT_VERDICT" = ERROR ]; then
        ctl delay_before_apply ERROR "$KT_WHY"
    elif [ "$KT_DONE" != 5 ]; then
        ctl delay_before_apply ERROR "only $KT_DONE of 5 kills were done"
    elif [ "$KT_VERDICT" != PASS ]; then
        ctl delay_before_apply ERROR "the pool did not come through the kills of a server that only delays: $KT_WHY"
    elif [ "$KT_RECOVERED_UNSAID" != 0 ]; then
        ctl delay_before_apply ERROR "$KT_RECOVERED_UNSAID of $KT_DONE take-overs did not say what they found"
    elif [ "$KT_RECOVERED" = 0 ]; then
        ctl delay_before_apply FAIL "the servers found 0 requests applied and not acknowledged over $KT_DONE take-overs, with writes in flight just before $KT_INFLIGHT of the $KT_DONE kills"
    else
        ctl delay_before_apply PASS "the servers found $KT_RECOVERED requests applied and not acknowledged over $KT_DONE take-overs although the delay came before the apply"
    fi
    kill_cleanup bd0 "$REF"
}
check_b() {
    local dir=$WORK/vol-b pool=skzd-b v pid0 pid1 rc st i num before after
    step "b: kill -9 under load, $KILLS times"
    kill_test "$SKYZFSD" b "$KILLS"
    kill_verdict; v=$KT_VERDICT
    pin kill_under_load "$v" "${KT_WHY:-writes were in flight just before $KT_INFLIGHT of $KT_DONE kills: no application write failed, no error on the vdev, scrub clean, $KT_FILES}"
    # How long each take-over took, in order. A daemon that does not
    # checkpoint replays a log that grows from kill to kill; one that
    # does must not. The pin on it waits for checkpoints (M1.3c); until
    # then the times are an observation, and the medians are those the
    # pin will compare: the last five against the first five.
    obs b_takeover_ms "$(medians "$KT_MS")"
    # Each take-over by the new server's own account, beside the
    # rig's clock above: start_ms is the daemon's start until the
    # device served, the rest of a take-over is before and after it.
    obs b_takeover_starts "per take-over, in order, ms from the kill to the new serve's launch + from the launch to its ready line : the daemon's own start_ms : replayed_bytes: ${KT_STARTS:-none}"
    if in_place; then
        pin takeover_time_does_not_grow NA "the in-place store has no log to replay, whose length the pin bounds; the take-over times are still observed (obs_b_takeover_ms)"
        pin log_stays_below_high_mark NA "the in-place store has no log"
    elif [ "$IMPL" = daemon ]; then
        takeover_pin "$KT_MS"
        log_bound_pin b
    else
        pin takeover_time_does_not_grow WAITING "the stand-in has no log to replay"
        pin log_stays_below_high_mark WAITING "the stand-in has no log and no checkpoint"
    fi
    # The check of what was acknowledged (§4.2) must not refuse a log
    # that a kill left whole. Kills around a change of log segment and
    # right after a checkpoint are the daemon's to arrange; here every
    # kill is at a random moment under load.
    if [ "$KT_REFUSED" -gt 0 ]; then
        pin serve_after_kill_accepts_log FAIL "$KT_REFUSED serve(s) after a kill exited $EXIT_REFUSED_FAILED: the volume was refused as failed"
    elif [ "$KT_DONE" -ge 1 ]; then
        pin serve_after_kill_accepts_log PASS "none of $KT_DONE serves after a kill was refused"
    else
        pin serve_after_kill_accepts_log ERROR "no kill was followed by a serve: $KT_ERR"
    fi
    if [ -n "$KT_ERR" ] || [ -z "$BDEV" ]; then
        pin second_serve_refused ERROR "the kill test did not leave a served pool"
        pin orderly_stop_keeps_device ERROR "the kill test did not leave a served pool"
        pin detach_refused_while_served ERROR "the kill test did not leave a served pool"
    else
        num=${BDEV#/dev/ublkb}
        step "b: a second serve while the first is alive"
        pid0=$(server_pid "$SKYZFSD" "$dir")
        lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --run-dir "$RUN_DIR" > "$OUT/b-second.out" 2> "$OUT/b-second.err"; rc=$?
        pid1=$(server_pid "$SKYZFSD" "$dir")
        if [ $rc = 124 ] || [ $rc = 137 ]; then
            pin second_serve_refused FAIL "a second serve did not exit within 60 s"
        elif [ $rc = "$EXIT_BUSY" ] && [ "$pid0" = "$pid1" ] && kill -0 "$pid0" 2>/dev/null \
            && [ "$(pool_state "$pool")" = ONLINE ] && lim 60 "$DD" if=/dev/zero of="$WORK/mnt/$pool/after-second" bs=1M count=4 conv=fsync status=none; then
            pin second_serve_refused PASS "exit $rc; the first server ($pid0) and the pool carried on"
        else
            pin second_serve_refused FAIL "exit $rc (wanted $EXIT_BUSY); server pid $pid0 then $pid1; pool $(pool_state "$pool"); $(tail -n 1 "$OUT/b-second.err")"
        fi

        step "b: detach while a server holds the volume"
        pid0=$(server_pid "$SKYZFSD" "$dir"); before=$(vol_records "$dir")
        lim 90 "$SKYZFSD" detach --dir "$dir" "${VOL_RUN_ARGS[@]}" > "$OUT/b-detach.out" 2> "$OUT/b-detach.err"; rc=$?
        pid1=$(server_pid "$SKYZFSD" "$dir"); after=$(vol_records "$dir"); st=$(ublk_state "$BDEV")
        if [ $rc = 124 ]; then
            pin detach_refused_while_served FAIL "detach did not exit within 90 s"
        elif [ $rc = "$EXIT_BUSY" ] && [ "$pid0" = "$pid1" ] && kill -0 "$pid0" 2>/dev/null && [ -b "$BDEV" ] && [ "$st" = LIVE ] \
            && [ "$before" = "$after" ] && [ "$(pool_state "$pool")" = ONLINE ] \
            && lim 60 "$DD" if=/dev/zero of="$WORK/mnt/$pool/after-detach" bs=1M count=4 conv=fsync status=none; then
            pin detach_refused_while_served PASS "exit $rc ($(tail -n 1 "$OUT/b-detach.err")); the server ($pid0), the device ($st), the attachment record and the notes are as before, and the pool carried on"
        else
            pin detach_refused_while_served FAIL "exit $rc (wanted $EXIT_BUSY); server pid $pid0 then $pid1; device $st; records $before then $after; pool $(pool_state "$pool"); $(tail -n 1 "$OUT/b-detach.err")"
        fi

        step "b: an orderly stop leaves the device for the next serve"
        kill -TERM "$SERVE_PID" 2>/dev/null
        if ! serve_wait 60; then
            pin orderly_stop_keeps_device FAIL "serve did not exit within 60 s of SIGTERM"
        else
            # The driver moves the device to QUIESCED in a work item
            # queued when the server's char device is released, which
            # can come after the process has gone (the control rig's
            # orderly stop says more): the device is given up to 5 s,
            # the daemon's limit for a take-over, and the time is said.
            rc=$SERVE_RC; t0=$(now_ms)
            while :; do
                st=$(ublk_state "$BDEV"); quiesced_ms=$(( $(now_ms) - t0 ))
                [ "$st" != QUIESCED ] && [ "$quiesced_ms" -lt 5000 ] || break
                sleep 0.1
            done
            ( lim 120 "$DD" if=/dev/zero of="$WORK/mnt/$pool/across-stop" bs=1M count=4 conv=fsync status=none; echo $? > "$WORK/b-across.rc" ) &
            across=$!
            sleep 3
            serve_start "$SKYZFSD" "$dir"; i=$?
            # Not a bare wait: the new serve is a background job too.
            wait "$across"
            if [ "$rc" = 0 ] && [ -b "$BDEV" ] && [ "$st" = QUIESCED ] && [ $i = 0 ] && [ "$(cat "$WORK/b-across.rc")" = 0 ] \
                && [ "$(pool_state "$pool")" = ONLINE ]; then
                pin orderly_stop_keeps_device PASS "serve exited 0 on SIGTERM, the device stayed ($st $quiesced_ms ms after serve returned), and a write begun while no server ran completed after serve returned; $(pool_errors "$pool")"
            else
                pin orderly_stop_keeps_device FAIL "serve exit $rc on SIGTERM, device state '$st' $quiesced_ms ms after serve returned, serve again rc $i, write across the stop rc $(cat "$WORK/b-across.rc"), pool $(pool_state "$pool")"
            fi
        fi
    fi
    if [ -n "$SERVE_PID" ]; then pin_unstored b "$dir"; else pin unstored_zero_b ERROR "no server at the end of the check"; fi
    [ "$(pool_state "$pool")" = ONLINE ] && lim 120 zpool export "$pool"
    [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
    # With the server stopped and the pool exported, detach deletes the
    # device and its number comes free.
    if [ -z "${num:-}" ]; then
        pin detach_frees_number ERROR "the kill test did not leave a served pool"
        vol_detach "$SKYZFSD" "$dir"
    elif [ "$(pool_state "$pool")" = ONLINE ] || [ -n "$SERVE_PID" ]; then
        pin detach_frees_number ERROR "the pool was not exported or the server did not stop (pool $(pool_state "$pool"), serve pid '${SERVE_PID:-}')"
        vol_detach "$SKYZFSD" "$dir"
    else
        st=$(ublk_state "/dev/ublkb$num")
        lim 90 "$SKYZFSD" detach --dir "$dir" "${VOL_RUN_ARGS[@]}" > "$OUT/b-detach-stopped.out" 2> "$OUT/b-detach-stopped.err"; rc=$?
        if [ $rc = 0 ] && ublk_number_free "$num"; then
            pin detach_frees_number PASS "the device was $st with no server; detach exit 0; number $num is free"
        else
            pin detach_frees_number FAIL "the device was $st with no server; detach exit $rc (wanted 0); number $num $(ublk_number_free "$num" && echo 'is free' || echo 'is still taken'); $(tail -n 1 "$OUT/b-detach-stopped.err")"
        fi
    fi

    check_b_detach_held
    check_b_stop_under_load
    check_b_on_zfs
    check_b_steps

    step "b, control: recovery without reissue"
    SKZ_REF_REISSUE=0 kill_test "$REF" b0 5
    kill_verdict; v=$KT_VERDICT
    ctl kill_without_reissue "$v" "${KT_WHY:-nothing went wrong in $KT_DONE kills}"
    [ "$(pool_state skzd-b0)" = ONLINE ] && lim 120 zpool export skzd-b0
    [ -z "$SERVE_PID" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
    vol_detach "$REF" "$WORK/vol-b0"

    check_b_delay

    # A waiting device with no server: the design's table of states has
    # it unchanged with no limit in time, from a reading of the driver.
    # Here it waits 300 s under a pool, and is then taken over.
    step "b, observation: a killed server's device waits ${RIG_WAIT_S:-300} s for the next"
    KT_WAIT_S=${RIG_WAIT_S:-300} kill_test "$SKYZFSD" bw 1
    kill_verdict
    if [ "${RIG_WAIT_S:-300}" -lt 100 ]; then
        # A shorter wait was asked for than the pin is about. Not a
        # verdict.
        obs b_device_waits_for_a_server "${KT_WAITED:-not run: ${KT_ERR:-the kill test did not reach the wait}}"
        pin device_waits_for_a_server ERROR "RIG_WAIT_S is ${RIG_WAIT_S} s; the pin needs at least 100 s"
    elif [ -z "$KT_WAITED" ]; then
        obs b_device_waits_for_a_server "not run: ${KT_ERR:-the kill test did not reach the wait}"
        pin device_waits_for_a_server ERROR "${KT_ERR:-the kill test did not reach the wait}"
    else
        obs b_device_waits_for_a_server "$KT_WAITED. Then served again: $KT_VERDICT, ${KT_WHY:-no application write failed, no error on the vdev, scrub clean, $KT_FILES}"
        # Waiting at every look, nothing in the kernel log about the
        # device, and the take-over as clean as after any kill. The
        # reports of blocked tasks are the observation's.
        if [ "$KT_VERDICT" = ERROR ]; then
            pin device_waits_for_a_server ERROR "$KT_WHY"
        elif [ "$KT_WAIT_BAD" = lost ]; then
            pin device_waits_for_a_server ERROR "the rig's marker was not found in the kernel log at the end of the wait (the log wrapped, or dmesg failed), so there is no kernel log of the ${RIG_WAIT_S:-300} s to judge"
        elif [ "$LK_VERDICT" = ERROR ]; then
            # The rig did not see the device the whole time.
            pin device_waits_for_a_server ERROR "$LK_WHY; $LK_SAID"
        else
            if [ "$KT_VERDICT" = PASS ] && [ "$LK_VERDICT" = PASS ] && [ "$KT_WAIT_BAD" = 0 ]; then v=PASS; else v=FAIL; fi
            pin device_waits_for_a_server "$v" "with no server, $LK_SAID ($LK_OTHER not QUIESCED; at least $LK_FLOOR looks are needed, each naming a state); kernel log lines with a failed request, an I/O error, a request timed out or an abort: $KT_WAIT_BAD; the take-over: ${KT_WHY:-no application write failed, no error on the vdev, scrub clean, $KT_FILES}"
        fi
    fi
    kill_cleanup bw
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
    # shellcheck disable=SC2016  # expanded by the inner shell
    lim 120 bash -c 'cd "$1" && sha256sum base.* > "$2"' _ "$mnt" "$WORK/$name.sums" || { FS_ERR="reading the first files back failed or did not finish"; return 1; }
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
    local name=$1 secs=$2 pool=skzd-$1 i st rerr werr mark=$3 t0 took
    t0=$(now_ms)
    for i in $(seq 1 "$secs"); do
        st=$(pool_state "$pool"); [ "$st" = SUSPENDED ] && break
        sleep 1
    done
    took=$(( ($(now_ms) - t0) / 1000 ))
    sleep 3
    rerr=$(grep -c . "$OUT/$name-reader.log" || true)
    werr=$(grep -c failed "$OUT/$name-writer.log" || true)
    lim 30 zpool status -v "$pool" > "$OUT/$name-status-suspended.log" 2>&1
    klog_since "$mark" > "$OUT/$name-dmesg.log"; i=$?
    rec "${name}_zpool_status" "$(grep -E 'state:|status:|errors:|ublkb|SUSPENDED|UNAVAIL|FAULTED|REMOVED' "$OUT/$name-status-suspended.log" | tr -s ' ' | tr '\n' ';')"
    if [ "$i" != 0 ]; then rec "${name}_kernel_log" "lost: the rig's marker was not found in the kernel log, so nothing is counted"
    else rec "${name}_kernel_log" "$(grep -ciE 'I/O error|critical' "$OUT/$name-dmesg.log") lines with an I/O error; ZFS said: $(grep -iE 'zio|pool|WARNING' "$OUT/$name-dmesg.log" | sed 's/^\[[^]]*\] //' | sort | uniq -c | sort -rn | head -n 4 | tr -s ' ' | tr '\n' ';')"
    fi
    rec "${name}_events" "$(lim 30 zpool events -H 2>/dev/null | awk '{ c[$NF]++ } END { for (k in c) printf "%s x%d ", k, c[k] }')"
    FS_WHY="pool $st after $took s of waiting (the limit is ${secs} s); reads done $(cat "$WORK/$name.reads" 2>/dev/null), read errors seen by the reader: $rerr; writes failed for the writer: $werr"
    [ "$st" = SUSPENDED ] && [ "$rerr" = 0 ] && [ "$werr" = 0 ]
}
fail_stop() { # name: stop the helpers of fail_setup; they may be blocked on a suspended pool
    touch "$WORK/$1.stop"
    kill -9 "$FS_READER" "$FS_WRITER" "$FS_SYNCER" 2>/dev/null
}
# name dir [image]: the device of a volume that failed and was kept
# gets a server that answers again, from the volume's image or from
# `image` put in its place. Done with the reference server directly,
# past the stand-in's refusal: it is what an operator, or a daemon
# without the rule of §4.2, would do. BDEV is the device. -> RK_RC.
return_kept() {
    local dir=$2 pid i
    pid=$(vol_stat "$REF" "$dir" pid)
    [ -n "$pid" ] && kill -9 "$pid" 2>/dev/null
    for i in $(seq 1 80); do [ "$(ublk_state "$BDEV")" = QUIESCED ] && break; sleep 0.25; done
    [ -z "$SERVE_PID" ] || serve_wait 30
    if [ -n "${3:-}" ]; then cp --sparse=always "$3" "$dir/volume.img" || return 1; fi
    lim 90 ublk recover -n "${BDEV#/dev/ublkb}" > "$OUT/$1-recover.log" 2>&1; RK_RC=$?
    for i in $(seq 1 40); do [ "$(ublk_state "$BDEV")" = LIVE ] && break; sleep 0.25; done
    [ "$(ublk_state "$BDEV")" = LIVE ]
}
check_d() {
    local mark rc n v dir st0 st1 before after
    # d.1, first form: every request answered with a plain I/O error.
    step "d: every request fails with an I/O error (the device stays)"
    mark=$(klog_mark)
    if SKZ_REF_FAIL_STYLE=eio fail_setup d1 "$REF" --fail-log-after 8000; then
        if fail_wait d1 300 "$mark"; then pin suspends_on_io_errors PASS "$FS_WHY"; else pin suspends_on_io_errors FAIL "$FS_WHY"; fi
        rec d1_server "$(vol_status "$REF" "$WORK/vol-d1" | tr '\n' ';')"
        # d.4 and d.2: the device was kept, and a server answers on it
        # again from the same image. Nobody runs zpool clear for 60 s,
        # then somebody does. This is the case ZFS can carry on from
        # its memory, the one §4.2 refuses to serve.
        step "d: the kept device answers again"
        if [ "$(pool_state skzd-d1)" != SUSPENDED ]; then
            obs device_back_without_clear "not run: the pool is $(pool_state skzd-d1)"
            pin clear_resumes_on_returned_device ERROR "not run: the pool is $(pool_state skzd-d1)"
        elif ! return_kept d1 "$WORK/vol-d1"; then
            obs device_back_without_clear "not run: the device did not get a server again (ublk recover rc $RK_RC, state $(ublk_state "$BDEV"))"
            pin clear_resumes_on_returned_device ERROR "the device did not get a server again (ublk recover rc $RK_RC, state $(ublk_state "$BDEV"))"
        else
            st0=$(pool_state skzd-d1)
            for n in $(seq 1 60); do st1=$(pool_state skzd-d1); [ "$st1" = ONLINE ] && break; sleep 1; done
            obs device_back_without_clear "zed $(zed_state); the kept device answers again: pool $st0, then $st1 after $n s without zpool clear"
            before=$(grep -c . "$WORK/d1.record" || true)
            lim 120 zpool clear skzd-d1 > "$OUT/d1-clear.log" 2>&1; rc=$?
            for n in $(seq 1 30); do st1=$(pool_state skzd-d1); [ "$st1" = ONLINE ] && break; sleep 1; done
            v=""; lim 120 "$DD" if=/dev/zero of="$WORK/mnt/skzd-d1/after-clear" bs=1M count=4 conv=fsync status=none 2>/dev/null && v=1
            sleep 5
            after=$(grep -c . "$WORK/d1.record" || true)
            if [ "$st1" = ONLINE ] && [ -n "$v" ]; then
                pin clear_resumes_on_returned_device PASS "zpool clear rc $rc; pool $st1; a new write with fsync completed; the writer went from $before to $after acknowledged files; $(pool_errors skzd-d1)"
            else
                pin clear_resumes_on_returned_device FAIL "zpool clear rc $rc ($(tail -n 1 "$OUT/d1-clear.log")); pool $st1; new write $([ -n "$v" ] && echo ok || echo failed)"
            fi
        fi
    else
        pin suspends_on_io_errors ERROR "$FS_ERR"
        obs device_back_without_clear "not run: $FS_ERR"
        pin clear_resumes_on_returned_device ERROR "$FS_ERR"
    fi
    fail_stop d1
    # If the pool came back, it and its device can go.
    if [ "$(pool_state skzd-d1)" = ONLINE ]; then
        sleep 2
        lim 120 zpool export -f skzd-d1
        lim 60 ublk del --async -n "${BDEV#/dev/ublkb}" > /dev/null 2>&1
    fi

    # The control: the same wait on a volume that does not fail.
    step "d, control: a volume that does not fail"
    mark=$(klog_mark)
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
    mark=$(klog_mark)
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
        lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --run-dir "$RUN_DIR" > /dev/null 2> "$OUT/d2-again.err"; rc=$?
        lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --run-dir "$RUN_DIR" --acknowledge-failure > /dev/null 2> "$OUT/d2-again-ack.err"; n=$?
        if [ $rc = "$EXIT_REFUSED_FAILED" ] && [ $n = "$EXIT_REFUSED_FAILED" ]; then
            pin failed_held_volume_refused PASS "serve exited $rc, and $n with --acknowledge-failure"
        else
            pin failed_held_volume_refused FAIL "serve exited $rc, and $n with --acknowledge-failure; wanted $EXIT_REFUSED_FAILED twice"
        fi
    else
        for v in suspends_on_deleted_device failed_exit_status failed_held_volume_refused; do
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
    # On the reference server whatever the server under test, as d.1:
    # the observation is of the kernel and ZFS, and needs the flat
    # image only the reference server has.
    step "d: the device comes back with older content"
    mark=$(klog_mark)
    if ! SKZ_REF_FAIL_STYLE=eio fail_setup d3 "$REF" --fail-log-after 16000; then obs returned_device_with_older_content "not run: $FS_ERR"; fail_stop d3; return; fi
    # The older content: a copy of the image now. Everything the writer
    # is told from here on is missing from it.
    lim 120 zpool sync skzd-d3
    cp --sparse=always "$dir/volume.img" "$WORK/d3-older.img"
    n=$(grep -c . "$WORK/d3.record" || true)
    fail_wait d3 400 "$mark"; st=$?
    v=$(grep -c . "$WORK/d3.record" || true)
    fail_stop d3
    if [ $st != 0 ]; then obs returned_device_with_older_content "not run: the pool did not suspend cleanly: $FS_WHY"; return; fi
    # The kept device answers again, from the older copy.
    if ! return_kept d3 "$dir" "$WORK/d3-older.img"; then
        obs returned_device_with_older_content "not run: the device did not get a server again (ublk recover rc ${RK_RC:-none}, state $(ublk_state "$BDEV"))"; return
    fi
    lim 120 zpool clear skzd-d3 > "$OUT/d3-clear.log" 2>&1; rc=$?
    sleep 10
    st=$(pool_state skzd-d3)
    lim 30 zpool status -v skzd-d3 > "$OUT/d3-status-after-clear.log" 2>&1
    lim 300 zpool scrub -w skzd-d3 > "$OUT/d3-scrub.log" 2>&1
    obs returned_device_with_older_content "the image was copied when the writer had $n files acknowledged; it had $v at the failure. After the copy came back and zpool clear (rc $rc): pool $st; $(pool_errors skzd-d3); the writer's files: $(lim 120 files_match "$WORK/mnt/skzd-d3" "$WORK/d3.record" 2>&1 | tail -n 1); status: $(grep -E 'state:|status:' "$OUT/d3-status-after-clear.log" | tr -s ' ' | tr '\n' ';')"
}
# d.5, an observation: a scrub in progress when the volume fails.
check_d_scrub() {
    local mark st rc before running
    step "d: a scrub in progress at the failure"
    mark=$(klog_mark)
    if ! fail_setup d5 "$SKYZFSD"; then obs scrub_in_progress_at_failure "not run: $FS_ERR"; fail_stop d5; return; fi
    # More to scrub, then the failure a little after the scrub begins.
    lim 300 "$DD" if=/dev/urandom of="$WORK/mnt/skzd-d5/big" bs=1M count=1024 conv=fsync status=none
    # The failure comes by the signal, with the server that is already
    # serving. Before, the server was restarted with a count of writes
    # after which to fail; the writes that had queued up while no
    # server ran could use the count up before `ready` was seen, and
    # the serve then counted as not started (Ubuntu 24.04, twice).
    lim 30 zpool scrub skzd-d5; rc=$?
    sleep 2
    before=$(lim 30 zpool status skzd-d5 2>&1 | grep -A2 'scan:' | tr -s ' ' | tr '\n' ' ')
    # The line is about a scrub in progress only when one was. A scrub
    # that had ended before the failure says nothing about that, and the
    # line then says so first.
    case "$before" in
        *"scrub in progress"*) running="the scrub was still running at the failure" ;;
        *"scrub repaired"*|*"scrub canceled"*) running="NOT a scrub in progress: the scrub had ended before the failure, so this line shows only a failure after a scrub" ;;
        *) running="whether the scrub was running at the failure is not known (no scan line that says so)" ;;
    esac
    kill -USR1 "$SERVE_PID" 2>/dev/null || { obs scrub_in_progress_at_failure "not run: no server to signal (serve pid '${SERVE_PID:-}')"; fail_stop d5; return; }
    fail_wait d5 180 "$mark"; st=$?
    obs scrub_in_progress_at_failure "$running; zpool scrub rc $rc; scan line 2 s later, just before the failure: ${before:-none}; $FS_WHY; scan line when suspended: $(grep -A2 'scan:' "$OUT/d5-status-suspended.log" | tr -s ' ' | tr '\n' ' '); $(pool_errors skzd-d5)"
    fail_stop d5
    return $st
}
# d.6, second half: nothing held the device at the failure, so an
# operator may acknowledge it. Also where the note is kept by default.
check_d_not_held() {
    local dir=$WORK/vol-d6 rc n marker=$WORK/d6.marker found note w0 w1 wrote spid
    local VOL_RUN_ARGS=()       # this volume is served with the default run directory
    step "d: a volume that failed with no client attached"
    : > "$marker"
    # No --run-dir here: the default must be under /run.
    vol_create "$SKYZFSD" "$dir" || { pin failed_unheld_volume_serves_when_acknowledged ERROR "create failed"; pin failure_note_under_run ERROR "create failed"; return; }
    strays_note "before the first serve of $dir"
    : > "$dir.ready"
    "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --fail-log-after 10 > "$dir.ready" 2>> "$dir.serve.log" &
    SERVE_PID=$!
    for n in $(seq 1 120); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        [ -n "$BDEV" ] && break; sleep 0.25
    done
    [ -n "$BDEV" ] || { pin failed_unheld_volume_serves_when_acknowledged ERROR "serve did not become ready"; pin failure_note_under_run ERROR "serve did not become ready"; return; }
    # Writes straight to the device until it fails; dd then exits, so
    # nothing holds the device when the server takes note. The device
    # may be failed or gone before the first write (whatever the server
    # counts towards its failure, the kernel's own reads at the start
    # may be enough) and is deleted at some moment between two writes:
    # a write goes only to a block device, and dd must not create the
    # path (devnode.sh).
    for n in $(seq 1 40); do
        [ -b "$BDEV" ] || break
        lim 30 "$DD" if=/dev/zero of="$BDEV" bs=4k count=1 seek="$n" oflag=direct conv=nocreat status=none 2>/dev/null || break
    done
    serve_wait 30
    rc=$SERVE_RC
    # The note has a name: <run dir>/v-<dev>-<ino>/failed, the device
    # and inode numbers of the volume's directory in hex, and the
    # default run directory is /run/skyzfsd. Any other new file under
    # /run (the status file, the acknowledged position) is not it.
    note=/run/skyzfsd/v-$(stat -c %D "$dir")-$(printf '%x' "$(stat -c %i "$dir")")/failed
    found=$(find /run -xdev -type f -newer "$marker" -path '*skyzfs*' 2>/dev/null | head -n 6 | tr '\n' ' ')
    if [ -f "$note" ] && [ "$note" -nt "$marker" ]; then pin failure_note_under_run PASS "after a failure without --run-dir: $note"
    else pin failure_note_under_run FAIL "no $note after a failure without --run-dir (serve exit $rc); new files under /run with skyzfs in the path: ${found:-none}"; fi
    lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" > /dev/null 2> "$OUT/d6-noack.err"; n=$?
    serve_start_default() { "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --acknowledge-failure > "$dir.ready" 2>> "$dir.serve.log" & SERVE_PID=$!; }
    strays_note "before the acknowledged serve of $dir"
    : > "$dir.ready"; serve_start_default
    BDEV=""
    for found in $(seq 1 120); do
        BDEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$dir.ready" | head -n 1)
        [ -n "$BDEV" ] && [ -b "$BDEV" ] && break
        kill -0 "$SERVE_PID" 2>/dev/null || break
        sleep 0.25
    done
    # "Served and written" is the server's word, not dd's: its own count
    # of writes must go up by the write. dd exiting 0 shows only that
    # something at that path took 4096 bytes.
    # The count before the write must be the new server's: the status
    # may still be the failed instance's last for a moment (the daemon
    # writes it on an interval), and that instance had counted the
    # writes that made it fail. The failed instance is gone, so the
    # status is the new server's once the pid in it is alive.
    wrote="not tried"; w0=""; w1=""; spid=""
    if [ -n "$BDEV" ] && [ -b "$BDEV" ]; then
        for found in $(seq 1 50); do
            spid=$(vol_stat "$SKYZFSD" "$dir" pid)
            { [ -n "$spid" ] && kill -0 "$spid" 2>/dev/null; } && break
            spid=""; sleep 0.1
        done
    fi
    if [ -n "$spid" ]; then
        w0=$(vol_stat "$SKYZFSD" "$dir" writes)
        if lim 30 "$DD" if=/dev/zero of="$BDEV" bs=4k count=1 oflag=direct conv=nocreat status=none 2>/dev/null; then
            w1=$(vol_stat_least "$SKYZFSD" "$dir" writes $(( ${w0:-0} + 1 )))
            wrote="dd exit 0"
        else
            wrote="dd failed"
        fi
    fi
    if [ -n "$BDEV" ] && [ ! -b "$BDEV" ]; then
        strays_note "at the acknowledged serve of $dir"
        pin failed_unheld_volume_serves_when_acknowledged ERROR "the acknowledged serve reported $BDEV, which is not a block device: $(ls -ld "$BDEV" 2>&1)"
    elif [ -n "$BDEV" ] && [ -z "$spid" ]; then
        pin failed_unheld_volume_serves_when_acknowledged ERROR "the acknowledged serve made $BDEV ready, and for 5 s its status named no live server (pid '$(vol_stat "$SKYZFSD" "$dir" pid)'): the count of writes before the write would be the failed instance's"
    elif [ "$wrote" = "dd exit 0" ] && ! { [[ $w0 =~ ^[0-9]+$ ]] && [[ $w1 =~ ^[0-9]+$ ]]; }; then
        pin failed_unheld_volume_serves_when_acknowledged ERROR "status gave no count of writes (before '$w0', after '$w1')"
    elif [ "$rc" = "$EXIT_FAILED" ] && [ $n = "$EXIT_REFUSED_FAILED" ] && [ "$wrote" = "dd exit 0" ] && [ "$w1" -gt "$w0" ]; then
        pin failed_unheld_volume_serves_when_acknowledged PASS "failed (exit $rc); refused without the acknowledgement (exit $n); served and written with it: the count of writes of the server now serving (pid $spid) went from $w0 to $w1"
    else
        pin failed_unheld_volume_serves_when_acknowledged FAIL "first serve exit $rc (wanted $EXIT_FAILED); without the acknowledgement exit $n (wanted $EXIT_REFUSED_FAILED); with it: $([ -n "$BDEV" ] && echo "ready $BDEV" || echo 'not ready'), the write: $wrote, the server's count of writes ${w0:-none} -> ${w1:-none}"
    fi
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
}

# =================================================================== e
# name seconds wanted: one read held `seconds`; judged against `wanted`.
hold_test() {
    local name=$1 secs=$2 want=$3 dir=$WORK/vol-$1 mark t0 t1 held i rc bad reader took rpid
    HT_ERR=""; HT_WHY=""
    mark=$(klog_mark)
    vol_up "$SKYZFSD" "$dir" --hold-one-request "$secs:read:$HOLD_AT" || { HT_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return 2; }
    # `held` in status is the number of requests in flight at the server
    # right now: 1 while the read is held, 0 once it has completed.
    # The held request is the first read of the block at HOLD_AT, a
    # place nothing probes when a device appears.
    # The reader is timed by itself: how long the kernel's side of the
    # request waited, whatever the server says of its own hold.
    ( t0=$(now_ms)
      lim $((secs + 120)) "$DD" if="$BDEV" of=/dev/null bs=4k count=16 skip=$((HOLD_AT / 4096)) iflag=direct status=none 2> "$OUT/$name-dd.err"
      echo "$? $(( $(now_ms) - t0 ))" > "$WORK/$name.rc" ) &
    reader=$!
    for i in $(seq 1 120); do
        held=$(vol_stat "$SKYZFSD" "$dir" held); [ "${held:-0}" -ge 1 ] && break; sleep 0.25
    done
    [ "${held:-0}" -ge 1 ] || { HT_ERR="status never said a request was held"; return 2; }
    rpid=$(pgrep -f "if=$BDEV of=/dev/null" | tr '\n' ' ')
    t0=$(now_ms)
    for i in $(seq 1 $(((secs + 90) * 2))); do
        held=$(vol_stat "$SKYZFSD" "$dir" held); [ "$held" = 0 ] && break; sleep 0.5
    done
    t1=$(now_ms)
    wait "$reader"
    read -r rc took < "$WORK/$name.rc" 2>/dev/null || { rc=""; took=0; }
    udevadm settle --timeout=30 2>/dev/null
    if ! klog_since "$mark" > "$OUT/$name-dmesg.log"; then
        HT_ERR="the rig's marker is no longer in the kernel log, so the log of this time cannot be cut out"
        kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; vol_detach "$SKYZFSD" "$dir"
        return 2
    fi
    # Tasks reported as blocked in this time are counted apart and by
    # name and pid: they may be left from an earlier check (a pool that
    # was suspended on purpose keeps its writer and its sync blocked for
    # good), and only one with the reader's pid is the held read.
    bad=$(klog_bad "$OUT/$name-dmesg.log")
    obs "${name}_kernel_log" "$(wc -l < "$OUT/$name-dmesg.log") lines while a request was held ${secs} s; lines with an I/O error, a request timed out or an abort: $bad; tasks reported blocked: $(klog_hung "$OUT/$name-dmesg.log")(the reader was pid ${rpid:-unknown})"
    HT_WHY="the read took $(( took / 1000 )) s at the reader and was held $(( (t1 - t0) / 1000 )) s by the server's own account (asked $secs s, wanted at least $want s, to within the polling, by both); the reader exited $rc; kernel log lines with an I/O error, a request timed out or an abort: $bad"
    [ "$name" != e ] || pin_unstored e "$dir"
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
    [ "$held" = 0 ] && [ $(( (t1 - t0) / 1000 )) -ge $((want - 5)) ] && [ "$took" -ge $(( (want - 1) * 1000 )) ] && [ "$rc" = 0 ] && [ "$bad" = 0 ]
}
# =================================================================== c
# The suite's verdict on each case, next to what the suite declares for
# a harness that can do what this one can. The judge compares the two.
check_c() {
    local tool=${RIG_BLKCHECK:-} work=$WORK/c rc t0 name result detail tag declared n n_live n_before n_at_death n_with n_zero n_dead
    # What the harness can do beyond a plain device. A failed volume is
    # opened again only by the stand-in: the daemon refuses, until the
    # next boot, a volume that failed while a client held its device,
    # and the suite's client holds it.
    local -a can=(--can fail-log)
    [ "$IMPL" = reference ] && can+=(--can reopen-failed)
    step "c: the conformance suite through the block device"
    if [ ! -x "$tool" ]; then rec c_run "ERROR: no skyzfs-blkcheck at '$tool' (RIG_BLKCHECK)"; return; fi
    mkdir -p "$work"
    t0=$(date +%s)
    # Every request the tool issues is written down, with a digest of
    # each block: a failed case is diagnosed from this, not rerun.
    BLKCHECK_TRACE=$OUT/c-trace.tsv BLK_SKYZFSD=$SKYZFSD BLK_WORK=$work BLK_SERVE_ARGS=${RIG_SERVE_ARGS:-} \
        lim 2400 "$tool" --harness "$HERE/blkcheck-helper.sh" "${can[@]}" \
        --cases "${RIG_C_CASES:-16}" --rounds "${RIG_C_ROUNDS:-4}" --timeout 60 \
        > "$OUT/c-cases.tsv" 2> "$OUT/c-blkcheck.err"; rc=$?
    C_RC=$rc
    cp "$work/helper.log" "$OUT/c-helper.log" 2>/dev/null
    # At each kill the tool sorts the calls it has outstanding: those
    # the old server had completed (their resubmission is issued) and
    # those in flight when it died (the kernel issues them again). If
    # the second number is near nothing, the kill cases test little.
    read -r n_live n_before n_at_death n_with n_zero n_dead <<< "$(awk -F'\t' '
        $3 == "sorted" { live++; split($5, w, " "); at_death += w[2] } $3 == "before_kill" { before++ }
        $3 == "sorted" && w[2] > 0 { with++; if ($6 ~ /\[0, 0\]/) zero++ } $3 == "kill" { kills++ }
        END { printf "%d %d %d %d %d %d", live, before, at_death, with, zero, kills - live }' "$OUT/c-trace.tsv" 2>/dev/null)"
    rec c_kills "${n_live:-0} kills of a live server (and ${n_dead:-0} that found none, after a failed log); calls outstanding when the suite asked for the kill: ${n_before:-0} had been completed by the old server and were issued again by the tool, ${n_at_death:-0} were in flight at its death and left to the kernel to issue again. The kernel's inflight file read 0 at ${n_zero:-0} of the ${n_with:-0} deaths with calls in flight: it does not count a request that waits to be issued again, and is no check on the sorting"
    # A kill of an idle server exercises its restart and nothing of the
    # reissue of requests. At least a third of the kills of a live
    # server must have found a call in flight at its death, or the kill
    # cases say less than they claim. In twelve image-runs it was 17 of
    # 29 once and 18 of 29 eleven times; a third is 10 of 29.
    if [ "${n_live:-0}" = 0 ]; then
        pin c_kill_with_requests_in_flight ERROR "the trace has no 'sorted' line: no kill of a live server was seen, so nothing can be said of calls in flight"
    elif [ $(( ${n_with:-0} * 3 )) -ge "$n_live" ]; then
        pin c_kill_with_requests_in_flight PASS "$n_with of $n_live kills of a live server found calls in flight at its death, ${n_at_death} calls in all (at least a third of the kills are needed)"
    else
        pin c_kill_with_requests_in_flight ERROR "only ${n_with:-0} of $n_live kills of a live server found a call in flight at its death (${n_at_death:-0} calls in all); at least a third are needed, or the kill cases exercise a restart and not the reissue"
    fi
    gzip -f "$OUT/c-trace.tsv" 2>/dev/null
    for f in "$work"/*.serve.log; do [ -s "$f" ] && cp "$f" "$OUT/c-$(basename "$f")"; done
    declare -A declared_as=()
    n=0
    while IFS=$'\t' read -r tag name result detail; do
        case $tag in
            declared) declared_as[$name]=$result ;;
            case) rec "case_$name" "$result: declared ${declared_as[$name]:-nothing}: $detail"; n=$((n + 1)) ;;
        esac
    done < "$OUT/c-cases.tsv"
    declared=$(awk -F'\t' '$1 == "declared" { n[$3]++ } END { printf "PASS %d, WEAKER %d, NOT_EXERCISED %d", n["PASS"], n["WEAKER"], n["NOT_EXERCISED"] }' "$OUT/c-cases.tsv")
    rec c_declared "$declared"
    rec c_cost "$(awk '$1 ~ /^(create|kill|fail|reopen|destroy)$/ && $4 == "ms" { n[$1]++; t[$1] += $3 }
        END { for (v in n) printf "%s x%d, mean %d ms; ", v, n[v], t[v] / n[v] }' "$OUT/c-helper.log" 2>/dev/null)the whole suite $(( $(date +%s) - t0 )) s"
    if [ $rc != 0 ]; then
        rec c_run "ERROR: skyzfs-blkcheck exit $rc after $n cases: $(tail -n 1 "$OUT/c-blkcheck.err"). This line cannot tell a tool that stopped from a server that never answered a call (124 is the rig's limit of 2400 s): the run's artifact has c-trace.tsv.gz, and \`skyzfs-blkcheck --replay c-trace.tsv VOLUME\` on the last volume shows the last call that was sent and whether it was answered"
    else
        rec c_run "ok: $n cases, cases=${RIG_C_CASES:-16} rounds=${RIG_C_ROUNDS:-4}; the harness: ${can[*]}. After the log is made to fail the server deletes its device and exits, so what a failed volume answers is the kernel's answer for a deleted device that the suite still holds open, and a kill after it finds no server ($(grep -c 'nothing to kill' "$OUT/c-helper.log" 2>/dev/null) such kills)"
    fi
    # The cases about a request of exactly 1 MiB and a discard across the
    # device test what the block layer forwards: they exercise the
    # server only if every device advertised a 1 MiB maximum.
    result=$(grep -o 'max_hw_sectors_kb=[0-9]*' "$OUT/c-helper.log" 2>/dev/null | sort | uniq -c | tr -s ' \n' ' ')
    if [ -z "$result" ]; then
        pin c_max_request ERROR "the helper wrote down no device"
    else
        pin_is c_max_request "of the suite's devices:$result" [ "$result" = " $(grep -c 'max_hw_sectors_kb=' "$OUT/c-helper.log") max_hw_sectors_kb=1024 " ]
    fi
    if [ $rc = 0 ]; then
        check_c_delay "$tool" "${can[@]}"
    else
        obs c_delay_kills "not run: the pass without the delay did not end"
        pin c_delay_cases ERROR "not run: the pass without the delay did not end"
        pin c_kill_with_applied_unacknowledged ERROR "not run: the pass without the delay did not end"
    fi
}
# The suite once more, with a server that applies a write and
# acknowledges it only after a delay: a kill then leaves writes applied
# and not acknowledged, which the kernel issues again to the next
# server. Without the delay a call in flight at a death may or may not
# have been applied, and nothing says which. Here the next server says
# how many it found, and the suite's kill cases see a write applied
# twice. The verdict is each case's result beside the first pass's.
check_c_delay() {
    local tool=$1 work=$WORK/cd rc t0 ms=${RIG_COMMIT_DELAY_MS:-20} n_live n_at_death found asked differ n
    shift
    step "c: the suite again, a write acknowledged $ms ms after it is applied"
    mkdir -p "$work"
    t0=$(date +%s)
    BLKCHECK_TRACE=$OUT/cd-trace.tsv BLK_SKYZFSD=$SKYZFSD BLK_WORK=$work \
        BLK_SERVE_ARGS="${RIG_SERVE_ARGS:-} --commit-delay-ms $ms" \
        lim 3600 "$tool" --harness "$HERE/blkcheck-helper.sh" "$@" \
        --cases "${RIG_C_CASES:-16}" --rounds "${RIG_C_ROUNDS:-4}" --timeout 60 \
        > "$OUT/cd-cases.tsv" 2> "$OUT/cd-blkcheck.err"; rc=$?
    cp "$work/helper.log" "$OUT/cd-helper.log" 2>/dev/null
    read -r n_live n_at_death <<< "$(awk -F'\t' '
        $3 == "sorted" { live++; split($5, w, " "); at_death += w[2] }
        END { printf "%d %d", live, at_death }' "$OUT/cd-trace.tsv" 2>/dev/null)"
    gzip -f "$OUT/cd-trace.tsv" 2>/dev/null
    for f in "$work"/*.serve.log; do [ -s "$f" ] && cp "$f" "$OUT/cd-$(basename "$f")"; done
    # What each new server found at its take-over, by its own account.
    read -r asked found <<< "$(awk '$3 == "take-over" && $4 == "found" { n++; if ($5 ~ /^[0-9]+$/) t += $5; else bad++ }
        END { printf "%d %s", n, bad ? "?" : t + 0 }' "$OUT/cd-helper.log" 2>/dev/null)"
    obs c_delay_kills "${n_live:-0} kills of a live server with a commit delay of $ms ms: ${n_at_death:-0} calls of the tool (reads and writes) were in flight at a death; the next server found ${found:-?} requests applied and not acknowledged over ${asked:-0} take-overs. The two count different things: a call may reach the server as several requests, and a read is never counted by the server. The whole pass took $(( $(date +%s) - t0 )) s"
    if [ $rc != 0 ]; then
        pin c_delay_cases ERROR "skyzfs-blkcheck exit $rc with the delay: $(tail -n 1 "$OUT/cd-blkcheck.err"). Whether the tool stopped or the server never answered a call: cd-trace.tsv.gz in the run's artifact, through \`skyzfs-blkcheck --replay\`"
        if in_place; then pin c_kill_with_applied_unacknowledged NA "the in-place store keeps no record of what it applied"; else pin c_kill_with_applied_unacknowledged ERROR "the pass with the delay did not end"; fi
        return
    fi
    if in_place; then
        pin c_kill_with_applied_unacknowledged NA "the in-place store keeps no record of what it applied, so a new server finds nothing applied and not acknowledged"
    else case ${found:-?} in
        '?') pin c_kill_with_applied_unacknowledged ERROR "a take-over did not say what it found (status key recovered_unacknowledged) at ${asked:-0} take-overs" ;;
        0) pin c_kill_with_applied_unacknowledged ERROR "no server found a write applied and not acknowledged over ${asked:-0} take-overs: the delay did nothing, and the kill cases did not see a write applied twice" ;;
        *) pin c_kill_with_applied_unacknowledged PASS "$found requests were applied and not acknowledged at a server's death and issued again, over ${asked} take-overs" ;;
    esac; fi
    # Every case as in the pass without the delay.
    differ=$(join -t $'\t' -j 1 \
        <(awk -F'\t' '$1 == "case" { print $2 "\t" $3 }' "$OUT/c-cases.tsv" | sort) \
        <(awk -F'\t' '$1 == "case" { print $2 "\t" $3 }' "$OUT/cd-cases.tsv" | sort) |
        awk -F'\t' '$2 != $3 { printf "%s %s, with the delay %s; ", $1, $2, $3 }')
    n=$(grep -c '^case' "$OUT/cd-cases.tsv")
    if [ "$n" != "$(grep -c '^case' "$OUT/c-cases.tsv")" ]; then
        pin c_delay_cases ERROR "$n cases with the delay, $(grep -c '^case' "$OUT/c-cases.tsv") without"
    elif [ -n "$differ" ]; then
        pin c_delay_cases FAIL "$differ$(grep -m 3 -h 'FAIL' "$OUT/cd-cases.tsv" | cut -f 2,4 | tr '\t\n' ' ;' | cut -c 1-600)"
    else
        pin c_delay_cases PASS "all $n cases as without the delay"
    fi
}

# The checked reads (status version 6, M1.3j): a read that finds a
# record of the log not yet durable reads its whole payload and checks
# it against the record's checksum. tpcb never reaches that path; the
# suite and the overlap test read before the flush that follows a
# write, and may. Observations, not judged until a run has shown the
# keys: a mismatch above 0 on a healthy volume is a defect of the
# server (a record that does not match fails the volume).
#   unflushed_said NAME < status  -> "NAME count C extra X mismatched M; "
#   or "NAME not printed; "
unflushed_said() {
    awk -v n="$1" '$1 == "time_read_unflushed_count" { c = $2 } $1 == "time_read_unflushed_extra_bytes" { x = $2 } $1 == "time_read_unflushed_mismatched" { m = $2 }
        END { if (c == "") printf "%s not printed; ", n; else printf "%s count %s extra %s mismatched %s; ", n, c, x, m }'
}
# name "NAME count C extra X mismatched M; ..." -> one observation line.
unflushed_obs() {
    local c x m np
    read -r c x m np <<< "$(echo "$2" | tr ';' '\n' | awk '$2 == "count" { c += $3; x += $5; m += $7; n++ } $2 == "not" { np++ } END { printf "%d %d %d %d", c, x, m, np }')"
    if [ -z "$(echo "$2" | tr -d ' ;')" ]; then
        obs "${1}_checked_reads" "not recorded: no status was read at the end (an observation missing, not an error of the rig)"
    elif [ "$c$x$m" = 000 ] && [ "$np" -gt 0 ] && ! echo "$2" | grep -q ' count '; then
        obs "${1}_checked_reads" "the server does not print time_read_unflushed_* (a daemon before status version 6, or the stand-in): nothing to say; $2"
    else
        obs "${1}_checked_reads" "reads that found a record not yet durable and checked its payload: $c, $x bytes read beyond those asked for, $m MISMATCHED$([ "$m" -gt 0 ] && echo ' (a mismatch on a healthy volume is a DEFECT of the server: the record did not match its checksum)'); volumes that did not print the keys: $np; each volume's count is its last server's, read when it was destroyed (a killed server's count is lost); the volumes with a checked read: $(echo "$2" | tr ';' '\n' | awk '$2 == "count" && $3 != 0' | tr -s ' \n' ' ' | cut -c1-600). The suite makes some of its volumes fail on purpose (fail-log): a mismatch there is to be read beside that case"
    fi
}
# The suite a third time, with checkpoint steps between the calls of
# its cases. The marks cannot be set below their defaults (high 2
# segments of 64 MiB, low 1) and a case writes far less than that, so
# each new volume's log is first filled with zero data to within one
# 4 KiB write of the high mark and no further (BLK_PREFILL_TO_HIGH; the
# volume still reads as empty, and no step has been taken). The case's
# first call that adds to the log (a write, discard or write-zeroes)
# takes it over the mark, and by the daemon's rule a step comes before
# the next request is executed, and one before each later one until
# the log is below the low mark. (A pre-fill past the mark, as before,
# had the daemon step back below the low mark by itself before the case
# began: steps were taken in the pass and none inside a case.)
# c_low_steps.py puts each volume in a class from the call trace and
# the helper's lines: `inside` (the pre-fill was set, the case made a
# call after its first write, and its servers took steps), `no_write`,
# `last_write` and `failed_first` (no call could follow a step, or the
# log was failed before the first write: counted and shown),
# `not_set` and `no_step` (it could, and did not). More of the last two
# than a tenth of the volumes that could hold a step is the rig's
# error, not a verdict. The verdict is each case's result beside the
# first pass's. The status of every volume is also read five times a
# second, for the observation line.
check_c_low() {
    local tool=${RIG_BLKCHECK:-} work=$WORK/cl rc t0 differ n sampler steps vols under most counts
    local -a can=(--can fail-log)
    if [ "$IMPL" != daemon ]; then pin c_suite_with_low_marks WAITING "the stand-in has no log and no checkpoint"; return; fi
    if in_place; then
        pin c_suite_with_low_marks NA "the in-place store has no log to fill to its high mark and takes no checkpoint step"
        obs c_low_steps "not taken with the in-place store: no log, no step"
        return
    fi
    if [ "${C_RC:-1}" != 0 ]; then pin c_suite_with_low_marks ERROR "not run: the first pass of the suite did not end"; return; fi
    step "c: the suite again, with checkpoint steps between the calls of each case (each volume's log filled to just below the high mark first)"
    mkdir -p "$work"; : > "$WORK/cl.samples"
    [ ! -e "$WORK/cl.stop" ] || unlink "$WORK/cl.stop"
    ( while [ ! -e "$WORK/cl.stop" ] && [ ! -e "$WORK/stop-all" ]; do
          for n in "$work"/c[0-9]*; do
              [ -d "$n" ] || continue
              lim 30 "$SKYZFSD" status --dir "$n" --run-dir "$work/run${n##*/c}" 2>/dev/null | awk -v v="${n##*/c}" '
                  $1 == "pid" { p = $2 } $1 == "log_live" { l = $2 } $1 == "checkpoint_steps" { s = $2 } $1 == "checkpoint_step" { u = $2 }
                  END { if (l != "") print v, (p == "" ? "-" : p), l, s, u }' >> "$WORK/cl.samples"
          done
          sleep 0.2
      done ) &
    sampler=$!
    t0=$(date +%s)
    BLKCHECK_TRACE=$OUT/cl-trace.tsv BLK_SKYZFSD=$SKYZFSD BLK_WORK=$work BLK_SERVE_ARGS=${RIG_SERVE_ARGS:-} BLK_PREFILL_TO_HIGH=1 \
        lim 3600 "$tool" --harness "$HERE/blkcheck-helper.sh" "${can[@]}" \
        --cases "${RIG_C_CASES:-16}" --rounds "${RIG_C_ROUNDS:-4}" --timeout 60 \
        > "$OUT/cl-cases.tsv" 2> "$OUT/cl-blkcheck.err"; rc=$?
    touch "$WORK/cl.stop"; job_end "$sampler" 60
    cp "$work/helper.log" "$OUT/cl-helper.log" 2>/dev/null
    python3 -I "$HERE/c_low_steps.py" "$OUT/cl-trace.tsv" "$OUT/cl-helper.log" > "$OUT/cl-volumes.txt" 2>&1 || echo "c_low_steps.py failed: $(tail -n 1 "$OUT/cl-volumes.txt")" >> "$OUT/cl-volumes.txt"
    gzip -f "$OUT/cl-trace.tsv" 2>/dev/null
    for n in "$work"/*.serve.log; do [ -s "$n" ] && cp "$n" "$OUT/cl-$(basename "$n")"; done
    # Steps: each server's count is its last reading; the sum over the
    # servers of the pass. Volumes: how many were seen, how many with a
    # step taken by any of their servers.
    read -r n steps vols under most <<< "$(awk '{ n++; k = $1 " " $2; if ($2 != "-" && $4 + 0 > s[k]) s[k] = $4 + 0; seen[$1] = 1; if ($5 == "under") u++; if ($3 + 0 > m) m = $3 + 0 }
        END { for (k in s) { t += s[k]; split(k, a, " "); if (s[k] > 0) with[a[1]] = 1 } for (v in seen) nv++; for (v in with) nw++
              printf "%d %d %d/%d %d %d", n, t, nw, nv, u, m }' "$WORK/cl.samples")"
    counts=$(sed -n 's/^c_low counts //p' "$OUT/cl-volumes.txt" | head -n 1)
    obs c_low_steps "volumes by class (cl-volumes.txt): ${counts:-none: $(tail -n 1 "$OUT/cl-volumes.txt" | cut -c1-200)}; pre-fills set $(grep -c '^prefill [0-9]*: set' "$OUT/cl-helper.log" 2>/dev/null), not set $(grep -c '^prefill [0-9]*: NOT SET' "$OUT/cl-helper.log" 2>/dev/null)$(grep -m 1 '^prefill [0-9]*: NOT SET' "$OUT/cl-helper.log" 2>/dev/null | sed 's/^/ (first: /; s/$/)/' | cut -c1-200); not_set and no_step volumes: $(awk '$1 == "c_low" && $2 == "volume" && ($4 == "not_set" || $4 == "no_step") { printf "%s ", $3 }' "$OUT/cl-volumes.txt" | cut -c1-200); ${n:-0} readings of the status: ${steps:-0} steps, on ${vols:-0/0} of the volumes seen, a step under way at ${under:-0} readings, log_live at most ${most:-0} bytes; the whole pass $(( $(date +%s) - t0 )) s"
    if [ $rc != 0 ]; then
        pin c_suite_with_low_marks ERROR "skyzfs-blkcheck exit $rc in this pass: $(tail -n 1 "$OUT/cl-blkcheck.err" | cut -c1-300). Whether the tool stopped or the server never answered a call: cl-trace.tsv.gz in the run's artifact, through \`skyzfs-blkcheck --replay\`"
        return
    fi
    if [ -z "$counts" ]; then
        pin c_suite_with_low_marks ERROR "the volumes were not classed: $(tail -n 1 "$OUT/cl-volumes.txt" | cut -c1-300)"
        return
    fi
    case $counts in
        *verdict=OK*) ;;
        *) pin c_suite_with_low_marks ERROR "too few volumes had a step between their calls: $counts. Not a verdict on the server: the pass did not run its cases beside a checkpoint (cl-volumes.txt, cl-helper.log)"; return ;;
    esac
    differ=$(join -t $'\t' -j 1 \
        <(awk -F'\t' '$1 == "case" { print $2 "\t" $3 }' "$OUT/c-cases.tsv" | sort) \
        <(awk -F'\t' '$1 == "case" { print $2 "\t" $3 }' "$OUT/cl-cases.tsv" | sort) |
        awk -F'\t' '$2 != $3 { printf "%s %s, while checkpointing %s; ", $1, $2, $3 }')
    n=$(grep -c '^case' "$OUT/cl-cases.tsv")
    if [ "$n" != "$(grep -c '^case' "$OUT/c-cases.tsv")" ]; then
        pin c_suite_with_low_marks ERROR "$n cases in this pass, $(grep -c '^case' "$OUT/c-cases.tsv") in the first"
    elif [ -n "$differ" ]; then
        pin c_suite_with_low_marks FAIL "$differ$(grep -m 3 -h 'FAIL' "$OUT/cl-cases.tsv" | cut -f 2,4 | tr '\t\n' ' ;' | cut -c 1-600)"
    else
        pin c_suite_with_low_marks PASS "all $n cases as in the first pass, with checkpoint steps between the calls of their cases: $counts"
    fi
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

case $CHECKS in *" a "*) check_a; check_a_overlap; check_a_zeroes; check_a_xfs16k ;; esac
case $CHECKS in *" b "*) check_b ;; esac
case $CHECKS in *" c "*)
    check_c
    check_c_low
    unflushed_obs c "$(for f in c cd cl; do awk -v p="$f" '$1 == "unflushed" { sub(/^unflushed /, ""); printf "%s/%s; ", p, $0 }' "$OUT/$f-helper.log" 2>/dev/null; done | sed 's|/\([0-9]*\): |\1 |g')" ;;
esac
case $CHECKS in *" e "*) check_e ;; esac
# Last: it leaves suspended pools behind.
case $CHECKS in *" d "*)
    check_d
    # A volume that fails while the server holds a request: waits for
    # the author's word on what the failure path does then, and for the
    # control rig's figures on a delete under a held request.
    pin failed_volume_while_a_request_is_held WAITING "not built: waits for what the failure path is to do while a request is held" ;;
esac
# Check b's control of a recovered log that is too short, after
# everything else: on the daemon it suspends a pool on purpose with a
# writer on it, and nothing can take that pool or its blocked tasks
# away again. Run in its place among check b's steps it left them
# beside checks c and e, whose kernel log then held reports of tasks
# blocked for longer than the checks themselves.
case $CHECKS in *" b "*)
    step "b, control: a recovered log that is too short"
    if [ "$IMPL" = reference ]; then
        ctl recovered_log_too_short WAITING "the stand-in has no log whose recovery could come out short"
        ctl recovered_log_too_short_no_log_device WAITING "the stand-in has no log whose recovery could come out short"
    elif in_place; then
        ctl recovered_log_too_short NA "the in-place store has no log whose recovery could come out short"
        ctl recovered_log_too_short_no_log_device NA "the in-place store has no log whose recovery could come out short"
    else
        check_b_short
    fi ;;
esac

# Check f after that, for the same reason: each of its courses leaves a
# suspended pool. With the stand-in there is nothing to refuse a start.
case $CHECKS in *" f "*)
    if [ "$IMPL" = reference ]; then
        ctl f_starts_refused WAITING "the stand-in has no log whose recovery could come out short"
    elif in_place; then
        ctl f_starts_refused NA "the in-place store has no log whose recovery could come out short, so no start is refused"
        for c in $F_ALL; do obs "f_refused_start_$c" "not taken with the in-place store: no start is refused"; done
    else
        check_f
    fi ;;
esac

# The files a reading needs most often, in the job's log as well: an
# artifact can be lost (GitHub's upload failed for a run's four jobs on
# 2026-11-11, and the serve logs went with them).
step "the servers' own lines"
for f in "$WORK"/vol-*.serve.log; do
    [ -s "$f" ] || continue
    echo "-- ${f##*/} ($(grep -c . "$f") lines; the last 6):"
    tail -n 6 "$f" | cut -c1-400
done

step "done"
# Nothing but block devices under the names of ublk devices in /dev,
# at any serve and now: a regular file there is a leftover of a tool
# that created the path, and what was "written to the device" after it
# went to that file.
strays_note "at the end of the run"
if [ -s "$STRAYS" ]; then rec dev_strays "ERROR: $(tr '\n' ';' < "$STRAYS" | cut -c1-1500)"; else rec dev_strays none; fi
# What the reference server's tool leaves under its own run directory:
# the daemon's library writes files of the same kind there, and the two
# servers follow each other on one machine under reused numbers.
# The daemon ends a recovery itself so that its library writes nothing
# there: no entry for a device's number may be newer than the start of
# the daemon's serve that made the device ready. Looked at after every
# such serve (ublksrvd_look), here and in check c's helper.
n_serves=$(grep -c '^serve ' "$UBLKSRVD_SEEN" 2>/dev/null)
n_entries=$(grep -c '^entry ' "$UBLKSRVD_SEEN" 2>/dev/null)
# The pin is check b's in the table, where the take-overs are; a result
# of a check that was not asked for is a rig error.
if [[ $CHECKS != *" b "* ]]; then
    :
elif [ "$IMPL" = reference ]; then
    pin no_ublksrvd_entry_from_daemon WAITING "the stand-in is the reference server, whose directory this is"
elif [ "${n_serves:-0}" = 0 ]; then
    pin no_ublksrvd_entry_from_daemon ERROR "no serve of the daemon was looked at"
elif [ "${n_entries:-0}" = 0 ]; then
    pin no_ublksrvd_entry_from_daemon PASS "after each of $n_serves serves of the daemon, /run/ublksrvd had no entry for the device's number newer than that serve's start"
else
    pin no_ublksrvd_entry_from_daemon FAIL "$n_entries entries newer than the daemon's serve, over $n_serves serves: $(grep '^entry ' "$UBLKSRVD_SEEN" | head -n 5 | cut -d' ' -f2- | tr '\n' ' ')"
fi
# The limit itself is put back by cleanup, at the exit.
rec hung_task_warnings_end "$(cat /proc/sys/kernel/hung_task_warnings 2>/dev/null || echo unreadable); set back to ${HUNG_WARNINGS_WAS:-nothing, it was unreadable} at the exit"
rec ublksrv_run_dir "$(find /run/ublksrvd -mindepth 1 -maxdepth 1 2>/dev/null | sort | head -n 20 | tr '\n' ' ')($(find /run/ublksrvd -mindepth 1 2>/dev/null | wc -l) entries at the end of the run)"
for f in "$WORK"/*.serve.log; do [ -s "$f" ] && cp "$f" "$OUT/"; done
cat "$R"
exit 0

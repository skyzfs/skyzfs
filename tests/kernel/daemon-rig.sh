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
# Checks, chosen with RIG_CHECKS (default "a b c d e g"):
#   a  parameters of the device and of a pool on it
#   b  kill -9 under load, RIG_KILLS times (default 20); a second serve;
#      an orderly stop
#   c  the conformance suite of skyzfs-blk through the block device:
#      RIG_BLKCHECK is a built skyzfs-blkcheck, which drives the server
#      through tests/kernel/blkcheck-helper.sh. RIG_C_CASES and
#      RIG_C_ROUNDS (default 16 and 4) are the suite's sizes.
#   d  a failed volume: what ZFS does, and what the server refuses after
#   e  one request held RIG_HOLD_S seconds (default 300); a commit the
#      driver refuses, of a read, a write and a flush, and of a write
#      held when the server is stopped; a first fetch the driver
#      refuses, at a start and at a take-over (the daemon's test hooks)
#   g  a disk that fails under the daemon's home image
# RIG_SERVE_ARGS: options added to every serve. The daemon has one
#   store since M1.3t, and the rig names none: a build from before it
#   (a negative control) is given `--store in-place` here by the
#   workflow. The pins that rested on the log store are gone with it.
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
CHECKS=" ${RIG_CHECKS:-a b c d e g} "
# Every word is a check and there is at least one: a run that checks
# nothing must not look like a run. Check f (what follows a start
# refused for a short log) went with the log store (M1.3t).
n=0
for c in $CHECKS; do
    case $c in a|b|c|d|e|g) n=$((n + 1)) ;; *) echo "RIG_CHECKS: '$c' is not a check; the checks are a b c d e g" >&2; exit 64 ;; esac
done
[ $n -gt 0 ] || { echo "RIG_CHECKS names no check" >&2; exit 64; }
[ -z "${RIG_STORE:-}" ] || { echo "RIG_STORE: the daemon has one store since M1.3t; a build from before it takes --store in RIG_SERVE_ARGS" >&2; exit 64; }
KILLS=${RIG_KILLS:-20}
HOLD_S=${RIG_HOLD_S:-300}
# Options for every serve, split at spaces: for the daemon on a runner,
# "--unsafe-allow-unlocked-memory --unsafe-allow-any-filesystem".
read -r -a SERVE_ARGS <<< "${RIG_SERVE_ARGS:-}"

RUN_DIR=$WORK/run            # the server's --run-dir, so that a test can empty it
VOL_BYTES=$((8 << 30))
SLOG_MB=512
HOLD_AT=$((3 << 30))         # the byte whose first read is held in check e
RC_AT=$((HOLD_AT + (256 << 20)))  # the byte of check e's refused commits, which nothing else touches

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
# shellcheck disable=SC2034  # ZS_WHERE: where zfs_stuck_say's mark says the rig was
step() { ZS_WHERE=$*; printf '\n######## %s\n' "$*"; }
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
# shellcheck source=tests/kernel/blktrace.sh
. "$HERE/blktrace.sh"
# shellcheck source=tests/kernel/request-log.sh
. "$HERE/request-log.sh"
# Device paths: dev_strays, and why every dd to a device has conv=nocreat.
# shellcheck source=tests/kernel/devnode.sh
. "$HERE/devnode.sh"
# The looks at a device over a wait: look_word, looks_judge.
# shellcheck source=tests/kernel/looks.sh
. "$HERE/looks.sh"
# What holds ZFS when a zpool command has run out: zfs_stuck_say.
# shellcheck source=tests/kernel/zfs-stuck.sh
. "$HERE/zfs-stuck.sh"
# Once ZFS on this machine is stuck (zfs-stuck.sh), every zpool command
# waits behind the same lock to the end of the job, each for its whole
# limit, and the lines it leaves say only that (M1.T32). The courses
# after the mark are not run (`course NAME && FUNCTION`): `course` names
# them, and the judge reads each line they would have written as ERROR
# "not run: ZFS on this machine was stuck since STEP". A course already
# under way ends as it would have; what it judged stays judged.
ZS_FILE=$OUT/zfs-stuck.tsv; : > "$ZS_FILE"
ZS_STACKS=$OUT/zfs-stuck-stacks.txt; rm -f "${ZS_STACKS:?}"
NOT_RUN_COURSES=""
course() { # name: 0 if the course may run; 1 when ZFS is stuck, and the course is named as not run
    local since
    since=$(zfs_stuck_since) || return 0
    printf '\n######## not run: %s (ZFS on this machine is stuck since "%s")\n' "$1" "$since"
    NOT_RUN_COURSES+="${NOT_RUN_COURSES:+ / }$1"   # names hold commas
    return 1
}

# --------------------------------------------- the server, by its command line
# Everything that depends on the daemon's command line is in these
# functions and in skyzfsd-ref.sh.
EXIT_FAILED=3; EXIT_REFUSED_FAILED=4; EXIT_BUSY=5; EXIT_BROKEN_RULE=7; EXIT_COMMIT_REFUSED=8; EXIT_START_GIVEN_UP=9
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
# check dir: `commands_failed` (the queues' commands that the driver
# ended with a result other than OK and other than its abort at a stop:
# a commit or a fetch it refused, M1.3aa) is 0 in a healthy run.
# Counters are since the instance started, so this reads the instance
# serving at the end of the check; an empty dir is a check that ended
# with no server.
pin_commands_failed() {
    local v
    if [ "$IMPL" = reference ]; then
        pin "commands_failed_zero_$1" WAITING "the stand-in's server does not count the commands the driver ended"
        return
    fi
    if [ -z "$2" ]; then pin "commands_failed_zero_$1" ERROR "no server at the end of the check"; return; fi
    v=$(vol_stat "$SKYZFSD" "$2" commands_failed)
    case $v in
        0) pin "commands_failed_zero_$1" PASS "status says commands_failed 0 at the end of the check (commands_aborted $(vol_stat "$SKYZFSD" "$2" commands_aborted))" ;;
        '') pin "commands_failed_zero_$1" ERROR "status gave no commands_failed (status_version '$(vol_stat "$SKYZFSD" "$2" status_version)'): a build before M1.3aa does not count them" ;;
        *) pin "commands_failed_zero_$1" FAIL "status says commands_failed $v, the last with the result $(vol_stat "$SKYZFSD" "$2" last_command_failure)" ;;
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
    local rc=0
    lim 60 zpool status -p "$1" > "$OUT/status.tmp" 2>&1 || rc=$?
    if [ "$rc" = 124 ]; then echo "zpool status did not return in 60 s: $(zfs_stuck_say)"; return; fi
    [ "$rc" = 0 ] || { echo "zpool status failed with $rc"; return; }
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
    blktrace_off
    [ -z "${C_MONITOR:-}" ] || kill "$C_MONITOR" 2>/dev/null
    c_watch_back
    local p
    # A stuck ZFS (zfs-stuck.sh) answers no zpool command.
    zfs_stuck_since > /dev/null || for p in $(lim 30 zpool list -H -o name 2>/dev/null | grep '^skzd-'); do
        # A suspended pool cannot be exported; trying would block.
        [ "$(pool_state "$p")" = ONLINE ] && lim 60 zpool export -f "$p" 2>/dev/null
    done
    command -v ublk >/dev/null && lim 60 ublk del -a --async 2>/dev/null
    for p in "${SLOGS[@]}"; do losetup -d "$p" 2>/dev/null; done
    # Check g's file system, its device-mapper device and its loop device.
    if [ -n "${G_LOOP:-}" ]; then
        lim 30 umount -l "$G_MNT" 2>/dev/null
        lim 30 dmsetup remove --force "$G_DM" 2>/dev/null
        lim 30 losetup -d "$G_LOOP" 2>/dev/null
    fi
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
[ "$IMPL" = reference ] || rec daemon_build "${RIG_DAEMON_REF:-the checkout under test}"
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
setup_packages() { apt-get update && apt-get install -y --no-install-recommends zfsutils-linux python3 util-linux dmsetup e2fsprogs; }
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
    pin_commands_failed a "$dir"
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
    KT_ERR=""; KT_REFUSED=""; KT_DONE=0; KT_INFLIGHT=0; KT_FAILED_WRITES=""; KT_VDEV=""; KT_SCRUB=""; KT_FILES=""; KT_ROUNDS=""; KT_MS=""; KT_QUIESCE=""
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
    KT_STARTS=""
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
        if ! kill -9 "$pid" 2>/dev/null; then
            # A server that ended by itself is reaped by the shell at
            # once, so the kill fails; wait still gives its status. A 7
            # there is the daemon's word that the driver broke its rule
            # (M1.3ac, #77), an 8 that the driver refused one of its
            # commits (#83), a 9 that serve gave a take-over's start up
            # (#87): the next take-over would hide any of them.
            if [ -n "$SERVE_PID" ] && ! kill -0 "$SERVE_PID" 2>/dev/null; then wait "$SERVE_PID"; SERVE_RC=$?; SERVE_PID=""; fi
            if [ "$SERVE_RC" = "$EXIT_BROKEN_RULE" ] || [ "$SERVE_RC" = "$EXIT_COMMIT_REFUSED" ] || [ "$SERVE_RC" = "$EXIT_START_GIVEN_UP" ]; then
                KT_REFUSED=$(kt_refusal "$n" "$SERVE_RC" "$dir.serve.log" before)
                KT_ERR="before kill $n: the server had ended by itself, with exit $SERVE_RC"
            else
                KT_ERR="kill $n: kill -9 $pid failed${SERVE_RC:+ (the server had ended by itself, with exit $SERVE_RC)}"
            fi
            break
        fi
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
        # A daemon that ended by itself with exit 7, 8 or 9 between the
        # shell's last reaping and the kill (a race; the usual case is
        # the failed kill above): the next serve would take the device
        # over and the status be lost. Any other status is left as it was: the
        # stand-in's serve ends with 1 when its server is killed, and a
        # volume failed while served (3) has the next serve refuse it,
        # which kt_refusal names.
        if [ "$SERVE_RC" = "$EXIT_BROKEN_RULE" ] || [ "$SERVE_RC" = "$EXIT_COMMIT_REFUSED" ] || [ "$SERVE_RC" = "$EXIT_START_GIVEN_UP" ]; then
            KT_REFUSED=$(kt_refusal "$n" "$SERVE_RC" "$dir.serve.log" before)
            KT_ERR="before kill $n: the server had ended by itself, with exit $SERVE_RC"
            wait "$watcher"
            break
        fi
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
        i=$(vol_stat "$impl" "$dir" start_ms); KT_STARTS+="${i:--} "
        read -r live gone looks saw < "$WORK/$name.quiesce" 2>/dev/null || { live=""; gone=""; looks=0; saw="no record"; }
        KT_ROUNDS+="kill $n: ${w:-0} writes in flight, $st, served again rc $rc in $ms ms; last seen LIVE with the dead server at $live ms, first seen otherwise at $gone ms ($saw, $looks looks); "
        KT_QUIESCE+="${live:--1}:${gone:--1} "
        [ $rc != 0 ] || KT_MS+="$ms "
        if [ $rc != 0 ]; then
            KT_ERR="kill $n: serve did not come back (rc $rc, serve exit ${SERVE_RC:-none})"
            KT_REFUSED=$(kt_refusal "$n" "${SERVE_RC:-}" "$dir.serve.log")
            break
        fi
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
# kill exit serve-log [before] -> why, or nothing: the serve after a
# kill ran and refused the volume by its own exit status (failed,
# refused as failed, busy). That is a false alarm that
# suspends a healthy pool, a verdict against the server and not the
# rig's failure. So is exit 7 (M1.3ac, #77): the daemon says that the
# kernel's driver handed over a tag whose request was still out, and
# leaves the device waiting for the next serve, which the kill test
# does not start; `before`: the server that a kill was for had ended so
# by itself. That the driver broke its rule is the daemon's word, and
# the pins hold that it does not (lead's ruling on M1.T34). A serve that
# never started or ran out (no such status) is not one.
kt_refusal() {
    local who="kill $1: the next serve" said
    [ -z "${4:-}" ] || who="before kill $1: the server"
    said=$(tail -n 1 "$3" 2>/dev/null | cut -c1-600)   # a line of 9 runs to about 360
    case $2 in
        "$EXIT_BROKEN_RULE")
            printf "%s ended with exit %s: the driver handed over a tag whose request was still out, by the daemon's word; the device waits for a server, and the next serve should take it over: %s" "$who" "$2" "$said" ;;
        "$EXIT_START_GIVEN_UP")
            printf "%s ended with exit %s: serve gave the device's start up (the driver refused a first fetch, or the start or the end of the recovery had no answer within its limit), by the daemon's word; the device waits for a server, and the next serve should take it over: %s" "$who" "$2" "$said" ;;
        "$EXIT_COMMIT_REFUSED")
            printf "%s ended with exit %s: the driver refused one of its answers (a commit), by the daemon's word; the device waits for a server, and the next serve should be handed that request again: %s" "$who" "$2" "$said" ;;
        "$EXIT_FAILED"|"$EXIT_REFUSED_FAILED"|"$EXIT_BUSY")
            printf '%s refused the volume with exit %s: %s' "$who" "$2" "$said" ;;
    esac
}
# The four judgments of a kill test, as pins or as one control line.
# `kill_verdict pin`: a serve after a kill that the server refused by
# its own exit status is a FAIL of the pin. For a control it stays the
# rig's ERROR: a control whose kills did not all happen has not shown
# that the check fails, whatever it reads.
kill_verdict() { # [pin] -> KT_VERDICT PASS | FAIL | ERROR, the reason in KT_WHY
    KT_WHY=""; KT_VERDICT=""
    if [ "${1:-}" = pin ] && [ -n "$KT_REFUSED" ]; then KT_WHY=$KT_REFUSED; KT_VERDICT=FAIL; return; fi
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
# status file, device number -> the vdev's error counters as zpool
# status has them: "read R write W cksum C (state S)". ZFS's "zio ...
# error" lines in the kernel log are rate-limited and cannot count
# failed writes; these counters can.
vdev_say() {
    awk -v d="ublkb$2" '$1 == d { printf "read %s write %s cksum %s (state %s)", $3, $4, $5, $2; f = 1 } END { if (!f) printf "not in the status (which gave %d lines)", NR }' "$1" 2>/dev/null || printf 'no status file'
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
    local dir=$WORK/vol-b7 pool=skzd-b7 slog w rc i w0 w1 st files fails a stuck=""
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
    sleep 5
    # Not a bare wait: when the serve above did not come back, the
    # device waits for a server, the writer's synced write sleeps in
    # the kernel and the stop file does not end it.
    touch "$WORK/b7.stop"
    job_end "$w" 120 || stuck="THE WRITER HAD NOT ENDED 120 s after it was told to, and was killed; "
    st=$(pool_state "$pool")
    fails=$(grep -c . "$OUT/b7-writer.log")
    files=$(lim 120 files_match "$WORK/mnt/$pool" "$WORK/b7.record" 2>&1 | tail -n 1)
    a="the server had counted ${w0:-?} then ${w1:-?} writes just before SIGTERM; serve exit $rc; serve again rc $i$([ $i = 0 ] || echo " (THE SERVER DID NOT COME BACK: serve exit ${SERVE_RC:-none})"); ${stuck}the writer's failed writes: $fails; pool $st, $(pool_errors "$pool"); files: ${files:-not read in 120 s}"
    if ! { [[ ${w0:-} =~ ^[0-9]+$ ]] && [[ ${w1:-} =~ ^[0-9]+$ ]] && [ "$w1" -gt "$w0" ]; }; then
        pin orderly_stop_under_load ERROR "the stop did not find the writer at work: $a"
    elif [ "$rc" = 0 ] && [ $i = 0 ] && [ -z "$stuck" ] && [ "$fails" = 0 ] && [ "$st" = ONLINE ] && [[ $files == ok\ * ]]; then
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
    step "b: kill -9 under load, a write acknowledged $ms ms after it is applied, $kills times"
    KT_SERVE_EXTRA=(--commit-delay-ms "$ms")
    kill_test "$SKYZFSD" bd "$kills"
    KT_SERVE_EXTRA=()
    kill_verdict pin; v=$KT_VERDICT
    # The daemon's store keeps no record of what it applied, so a new
    # server cannot count writes applied and not acknowledged. The kills
    # still land, by the delay, between a write's apply and its
    # acknowledgement, where the kernel must issue the write again: the
    # pin is judged as pin_kill_under_load is, without the count. The
    # stand-in counts them, and its control below shows the count works.
    if [ "$IMPL" = daemon ]; then
        pin kill_under_load_applied_unacknowledged "$v" "${KT_WHY:-writes were in flight just before $KT_INFLIGHT of $KT_DONE kills, each write acknowledged $ms ms after it was applied: no application write failed, no error on the vdev, scrub clean, $KT_FILES}; not counted: the in-place store keeps no record of what it applied"
        ctl delay_before_apply NA "the control of the count of writes applied and not acknowledged, which the daemon's store cannot take"
        kill_cleanup bd
        return
    fi
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
    kill_verdict pin; v=$KT_VERDICT
    pin kill_under_load "$v" "${KT_WHY:-writes were in flight just before $KT_INFLIGHT of $KT_DONE kills: no application write failed, no error on the vdev, scrub clean, $KT_FILES}"
    # How long each take-over took, in order, and the medians of the
    # first five and the last five. The pin that bounded their growth
    # went with the log store (M1.3t), which replayed a log that grew
    # from kill to kill; the times stay an observation.
    obs b_takeover_ms "$(medians "$KT_MS")"
    # Each take-over by the new server's own account, beside the
    # rig's clock above: start_ms is the daemon's start until the
    # device served, the rest of a take-over is before and after it.
    obs b_takeover_starts "per take-over, in order, ms from the kill to the new serve's launch + from the launch to its ready line : the daemon's own start_ms: ${KT_STARTS:-none}"
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
    if [ -n "$SERVE_PID" ]; then pin_commands_failed b "$dir"; else pin_commands_failed b ""; fi
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
    kill_verdict pin
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
    # FS_DIR: the volume's directory somewhere else than under $WORK (check g).
    local name=$1 impl=$2 dir=${FS_DIR:-$WORK/vol-$1} pool=skzd-$1 mnt slog i; shift 2
    mnt=$WORK/mnt/$pool
    FS_ERR=""
    vol_up "$impl" "$dir" "$@" || { FS_ERR="not created or served (serve rc ${SERVE_RC:-none})"; return 1; }
    slog=$(new_slog "$name")
    pool_create "$pool" "$BDEV" "$slog" -O primarycache=metadata > "$OUT/$name-create.log" 2>&1; i=$?
    if [ "$i" = 124 ]; then
        FS_ERR="zpool create ran out of its 180 s: $(zfs_stuck_say)"; return 1
    elif [ "$i" != 0 ]; then
        FS_ERR="zpool create failed with $i: $(tail -n 1 "$OUT/$name-create.log")"; return 1
    fi
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
          cat "$mnt/base.$((n % 4))" > /dev/null 2>> "$OUT/$name-reader.log" || echo "read $n failed at $(date +%s.%N)" >> "$OUT/$name-reader.log"
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
# With FS_READS=observed a read that failed to the reader is not judged
# (g.1: a failed read fails its request alone, design §5.1).
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
    lim 30 zpool status -v "$pool" > "$OUT/$name-status-suspended.log" 2>&1; i=$?
    if [ "$i" = 124 ]; then
        rec "${name}_zpool_status" "did not return in 30 s: $(zfs_stuck_say)"
    else
        rec "${name}_zpool_status" "$(grep -E 'state:|status:|errors:|ublkb|SUSPENDED|UNAVAIL|FAULTED|REMOVED' "$OUT/$name-status-suspended.log" | tr -s ' ' | tr '\n' ';')"
    fi
    klog_since "$mark" > "$OUT/$name-dmesg.log"; i=$?
    if [ "$i" != 0 ]; then rec "${name}_kernel_log" "lost: the rig's marker was not found in the kernel log, so nothing is counted"
    else rec "${name}_kernel_log" "$(grep -ciE 'I/O error|critical' "$OUT/$name-dmesg.log") lines with an I/O error; ZFS said: $(grep -iE 'zio|pool|WARNING' "$OUT/$name-dmesg.log" | sed 's/^\[[^]]*\] //' | sort | uniq -c | sort -rn | head -n 4 | tr -s ' ' | tr '\n' ';')"
    fi
    rec "${name}_events" "$(lim 30 zpool events -H 2>/dev/null | awk '{ c[$NF]++ } END { for (k in c) printf "%s x%d ", k, c[k] }')"
    FS_WHY="pool $st after $took s of waiting (the limit is ${secs} s); reads done $(cat "$WORK/$name.reads" 2>/dev/null), read errors seen by the reader: $rerr$([ "${FS_READS:-}" != observed ] || echo ' (not judged here)'); writes failed for the writer: $werr"
    [ "${FS_READS:-}" = observed ] && rerr=0
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
    local mark rc n v st0 st1 before after
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
                pin clear_resumes_on_returned_device FAIL "zpool clear rc $rc ($(tail -n 1 "$OUT/d1-clear.log")); pool $st1; new write $([ -n "$v" ] && echo ok || echo failed)$([ "$rc" = 124 ] && echo "; the clear ran out of its 120 s: $(zfs_stuck_say "$mark")")"
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

    course "d, control" && check_d_control
    course "d.1 with the server under test" && check_d_deleted
    course d.3 && check_d_shorter
    course "d, scrub" && check_d_scrub
    course "d, not held" && check_d_not_held
}
# The control: the same wait on a volume that does not fail.
check_d_control() {
    local mark
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
}
# d.1, second form, with the server under test: the log fails, the
# daemon deletes its device and exits.
check_d_deleted() {
    local mark dir rc n v
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
    obs returned_device_with_older_content "the image was copied when the writer had $n files acknowledged; it had $v at the failure. After the copy came back and zpool clear (rc $rc$([ "$rc" = 124 ] && echo ", ran out of its 120 s: $(zfs_stuck_say)")): pool $st; $(pool_errors skzd-d3); the writer's files: $(lim 120 files_match "$WORK/mnt/skzd-d3" "$WORK/d3.record" 2>&1 | tail -n 1); status: $(grep -E 'state:|status:' "$OUT/d3-status-after-clear.log" | tr -s ' ' | tr '\n' ';')"
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
    [ "$name" != e ] || pin_commands_failed e "$dir"
    kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    vol_detach "$SKYZFSD" "$dir"
    [ "$held" = 0 ] && [ $(( (t1 - t0) / 1000 )) -ge $((want - 5)) ] && [ "$took" -ge $(( (want - 1) * 1000 )) ] && [ "$rc" = 0 ] && [ "$bad" = 0 ]
}
# =================================================================== c
# The suite's verdict on each case, next to what the suite declares for
# a harness that can do what this one can. The judge compares the two.
# What check c keeps beside the tool's trace (M1.T28, skyzfs/skyzfs#58):
# the daemon's own record of what each instance was handed and answered
# (--request-log, in its test build from M1.3y on; a volume's file and
# the extents of its files, by the helper), and the kernel's block
# trace (tests/kernel/blktrace.sh: issue, split, requeue, completion,
# on the clock of the request log). The artifact keeps them for a pass
# in which a case failed or the tool did not end; a pass that passed
# leaves only their counts in the line obs_c_evidence. A failure of
# 43455086383 was read from both sides this way: the call the model took
# as reissued had been applied and completed by the old server, and
# returned late only because a udev read of the device, requeued at the
# server's death, held the page the direct write's return waits on.
# udev's watch on the suite's devices is turned off for check c, so that
# a write's close does not make udev read the device under the suite.
C_NOWATCH=/etc/udev/rules.d/61-skyzfs-rig-ublkb-nowatch.rules
C_EVIDENCE=""
c_watch_off() {
    printf '%s\n' '# The daemon rig, check c: no udev watch on ublk devices (M1.T28).' \
        'ACTION!="remove", SUBSYSTEM=="block", KERNEL=="ublkb*", OPTIONS+="nowatch"' > "$C_NOWATCH" &&
        lim 30 udevadm control --reload
}
c_watch_back() {
    [ -e "$C_NOWATCH" ] || return 0
    rm -f "$C_NOWATCH"
    lim 30 udevadm control --reload
}
# c_evidence_start TAG: the block trace and a count of udev's change
# events for the pass TAG (c or cd). C_TRACE_WHY is empty when the trace
# can be taken.
c_evidence_start() {
    udevadm monitor --udev --subsystem-match=block > "$OUT/$1-udev.txt" 2>&1 &
    C_MONITOR=$!
    if [ -z "$C_TRACE_WHY" ]; then
        blktrace_on "$OUT/$1-ftrace.txt"
        blktrace_mark "check $1 start"
    fi
}
# c_evidence_end TAG WORK RC: stop both; keep the evidence if a case of
# the pass failed or the tool did not end; add the pass's part of the
# line obs_c_evidence to C_EVIDENCE.
c_evidence_end() {
    local tag=$1 work=$2 rc=$3 f n_logs n_lines n_change n_udev kept n_ab first
    blktrace_off
    kill "$C_MONITOR" 2>/dev/null; wait "$C_MONITOR" 2>/dev/null
    C_MONITOR=""
    n_change=$(grep -c 'change .*/ublkb[0-9]* (block)' "$OUT/$tag-udev.txt" 2>/dev/null)
    n_logs=0
    for f in "$work"/c*.requests.tsv "$work"/c*.extents; do
        [ -s "$f" ] || continue
        cp "$f" "$OUT/$tag-$(basename "$f")"
        case $f in *.requests.tsv) n_logs=$((n_logs + 1)) ;; esac
    done
    n_lines=$({ wc -l < "$OUT/$tag-ftrace.txt"; } 2>/dev/null || echo 0)
    # The commands the driver ended, from every instance's request log
    # of the pass (its `ended` lines, M1.3aa): -19 is the driver's abort
    # at a stop; any other result is a command that took no effect.
    if [ "$n_logs" -gt 0 ]; then
        read -r f n_ab first <<< "$(awk -F'\t' '
            $3 == "ended" && $8 == -19 { a++ }
            $3 == "ended" && $8 != -19 { f++; if (!e) e = "tag " $4 ", " $5 ", result " $8 }
            END { printf "%d %d %s", f, a, e }' "$OUT/$tag"-c*.requests.tsv)"
        C_CMD_LOGS=$((${C_CMD_LOGS:-0} + n_logs)); C_CMD_FAILED=$((${C_CMD_FAILED:-0} + f)); C_CMD_ABORTED=$((${C_CMD_ABORTED:-0} + n_ab))
        [ -n "${C_CMD_FIRST:-}" ] || [ -z "$first" ] || C_CMD_FIRST="pass $tag: $first"
        C_CMD_PASSES="${C_CMD_PASSES:-}$tag "
    fi
    # Requests that udev's own processes issued to any device during the
    # pass: its probe of each new device (its "add" event), and with a
    # watch its read after a writable close. That read, requeued at a
    # server's death, held back the return of a write in #58.
    n_udev=$(grep 'block_rq_issue:' "$OUT/$tag-ftrace.txt" 2>/dev/null | grep -c 'udev[^]]*\]$')
    # What the pass must leave, whether it is kept or not: a trace, and a
    # request log from a server that keeps one.
    [ "${n_lines:-0}" -gt 0 ] || C_MISSING+="pass $tag left no block trace; "
    # And no change event of a ublk device: the watch is off.
    case ${n_change:-x} in
        0) ;;
        x) C_MISSING+="pass $tag has no count of udev's events (udevadm monitor); " ;;
        *) C_MISSING+="pass $tag saw $n_change change events of a ublk device with udev's watch meant to be off; " ;;
    esac
    [ -z "$C_RL" ] || [ "$n_logs" -gt 0 ] || C_MISSING+="pass $tag left no request log, though the server keeps one; "
    if [ "$rc" != 0 ] || awk -F'\t' '$1 == "case" && $3 == "FAIL" { f = 1 } END { exit !f }' "$OUT/$tag-cases.tsv" 2>/dev/null; then
        gzip -f "$OUT/$tag-ftrace.txt" "$OUT/$tag"-c*.requests.tsv "$OUT/$tag"-c*.extents 2>/dev/null
        kept=""
        [ ! -s "$OUT/$tag-ftrace.txt.gz" ] || kept+="$tag-ftrace.txt.gz "
        ! compgen -G "$OUT/$tag-c*.requests.tsv.gz" > /dev/null || kept+="$tag-cN.requests.tsv.gz "
        ! compgen -G "$OUT/$tag-c*.extents.gz" > /dev/null || kept+="$tag-cN.extents.gz "
        kept="kept in the artifact (${kept% }), since a case failed or the tool did not end"
    else
        rm -f "$OUT/$tag-ftrace.txt" "$OUT/$tag"-c*.requests.tsv "$OUT/$tag"-c*.extents
        kept="not kept: no case failed"
    fi
    C_EVIDENCE+="pass $tag: change events of a ublk device $n_change; requests issued by udev $n_udev; request logs $n_logs; block trace $n_lines lines; $kept. "
}
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
    # The evidence beside the tool's trace (see c_evidence_end).
    C_EVIDENCE=""; C_RL=""; C_MISSING=""
    C_CMD_LOGS=0; C_CMD_FAILED=0; C_CMD_ABORTED=0; C_CMD_FIRST=""; C_CMD_PASSES=""
    if has_request_log "$SKYZFSD"; then C_RL=1; C_EVIDENCE+="the server keeps a request log; "
    else C_EVIDENCE+="the server keeps no request log (its build has no --request-log); "; fi
    if c_watch_off; then C_EVIDENCE+="udev's watch on ublk devices off ($C_NOWATCH); "
    else C_EVIDENCE+="udev's watch could NOT be turned off; "; fi
    C_TRACE_WHY=$(blktrace_ready)
    t0=$(date +%s)
    c_evidence_start c
    # Every request the tool issues is written down, with a digest of
    # each block: a failed case is diagnosed from this, not rerun.
    BLKCHECK_TRACE=$OUT/c-trace.tsv BLK_SKYZFSD=$SKYZFSD BLK_WORK=$work BLK_SERVE_ARGS=${RIG_SERVE_ARGS:-} BLK_REQUEST_LOG=$C_RL \
        lim 2400 "$tool" --harness "$HERE/blkcheck-helper.sh" "${can[@]}" \
        --cases "${RIG_C_CASES:-16}" --rounds "${RIG_C_ROUNDS:-4}" --timeout 60 \
        > "$OUT/c-cases.tsv" 2> "$OUT/c-blkcheck.err"; rc=$?
    c_evidence_end c "$work" "$rc"
    cp "$work/helper.log" "$OUT/c-helper.log" 2>/dev/null
    # At each kill the tool sorts the calls it has outstanding: those
    # the old server had completed and those in flight when it died (the
    # kernel issues them again). If the second number is near nothing,
    # the kill cases test little. The tool's trace word for the first
    # kind says what it does with them: before_kill, issued again by the
    # tool; answered_before_kill (the tool's late-result handover),
    # handed to the suite with the old server's answer.
    read -r n_live n_before n_at_death n_with n_zero n_dead n_answered <<< "$(awk -F'\t' '
        $3 == "sorted" { live++; split($5, w, " "); at_death += w[2] } $3 == "before_kill" { before++ }
        $3 == "answered_before_kill" { answered++ }
        $3 == "sorted" && w[2] > 0 { with++; if ($6 ~ /\[0, 0\]/) zero++ } $3 == "kill" { kills++ }
        END { printf "%d %d %d %d %d %d %d", live, before, at_death, with, zero, kills - live, answered }' "$OUT/c-trace.tsv" 2>/dev/null)"
    rec c_kills "${n_live:-0} kills of a live server (and ${n_dead:-0} that found none, after a failed log); calls outstanding when the suite asked for the kill: ${n_before:-0} had been completed by the old server and were issued again by the tool, ${n_answered:-0} had been completed by the old server and were handed to the suite with its answer, ${n_at_death:-0} were in flight at its death and left to the kernel to issue again. The kernel's inflight file read 0 at ${n_zero:-0} of the ${n_with:-0} deaths with calls in flight: it does not count a request that waits to be issued again, and is no check on the sorting"
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
        C_EVIDENCE+="no pass with the delay. "
    fi
    pin_c_commands_failed
    # The trace, and the request log of a server that keeps one, are
    # asked for on every run, and judged by what each pass left: a pass
    # without them is the rig's error, not a verdict.
    if [ -n "$C_TRACE_WHY" ]; then
        obs c_evidence "not run: no block trace: $C_TRACE_WHY. ${C_EVIDENCE}"
    elif [ -n "$C_MISSING" ]; then
        obs c_evidence "not run: ${C_MISSING}${C_EVIDENCE}"
    else
        obs c_evidence "$C_EVIDENCE"
    fi
}
# No command of the suite's servers ended with a result other than OK
# or the driver's abort, by the request logs of both passes: every
# instance of every volume, the killed ones too, which pin_commands_failed
# (one instance, read at the end) cannot see. A build from before
# M1.3aa writes no `ended` line, so 0 there would say nothing: such a
# build, known by its binary not holding the status word, is the rig's
# error.
pin_c_commands_failed() {
    if [ "$IMPL" = reference ]; then
        pin c_commands_failed_zero WAITING "the stand-in keeps no request log"
    elif [ -z "$C_RL" ]; then
        pin c_commands_failed_zero ERROR "the server keeps no request log (its build has no --request-log), so the commands the driver ended are not seen"
    elif ! grep -qa commands_failed "$SKYZFSD"; then
        pin c_commands_failed_zero ERROR "this build of the daemon predates M1.3aa: its request log has no ended lines"
    elif [ "$C_CMD_LOGS" = 0 ]; then
        pin c_commands_failed_zero ERROR "no pass left a request log"
    elif [ "$C_CMD_FAILED" = 0 ]; then
        pin c_commands_failed_zero PASS "in $C_CMD_LOGS request logs of the passes ${C_CMD_PASSES% }: no command ended with a result other than OK or the driver's abort ($C_CMD_ABORTED aborts, -19)"
    else
        pin c_commands_failed_zero FAIL "in $C_CMD_LOGS request logs of the passes ${C_CMD_PASSES% }: $C_CMD_FAILED commands ended with a result other than OK or the driver's abort, the first in $C_CMD_FIRST ($C_CMD_ABORTED aborts, -19)"
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
    c_evidence_start cd
    BLKCHECK_TRACE=$OUT/cd-trace.tsv BLK_SKYZFSD=$SKYZFSD BLK_WORK=$work BLK_REQUEST_LOG=$C_RL \
        BLK_SERVE_ARGS="${RIG_SERVE_ARGS:-} --commit-delay-ms $ms" \
        lim 3600 "$tool" --harness "$HERE/blkcheck-helper.sh" "$@" \
        --cases "${RIG_C_CASES:-16}" --rounds "${RIG_C_ROUNDS:-4}" --timeout 60 \
        > "$OUT/cd-cases.tsv" 2> "$OUT/cd-blkcheck.err"; rc=$?
    c_evidence_end cd "$work" "$rc"
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
        if [ "$IMPL" = daemon ]; then pin c_kill_with_applied_unacknowledged NA "the daemon's store keeps no record of what it applied"; else pin c_kill_with_applied_unacknowledged ERROR "the pass with the delay did not end"; fi
        return
    fi
    if [ "$IMPL" = daemon ]; then
        pin c_kill_with_applied_unacknowledged NA "the daemon's store keeps no record of what it applied, so a new server finds nothing applied and not acknowledged"
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
check_e() {
    local v
    step "e: one request held $HOLD_S s"
    hold_test e "$HOLD_S" "$HOLD_S"; v=$?
    case $v in 0) pin held_request_completes PASS "$HT_WHY" ;; 1) pin held_request_completes FAIL "$HT_WHY" ;; *) pin held_request_completes ERROR "$HT_ERR" ;; esac
    step "e, control: a request held 5 s is not one held $HOLD_S s"
    hold_test e0 5 "$HOLD_S"; v=$?
    case $v in 0) ctl hold_too_short PASS "$HT_WHY" ;; 1) ctl hold_too_short FAIL "$HT_WHY" ;; *) ctl hold_too_short ERROR "$HT_ERR" ;; esac
    check_e_refused
    check_e_fetch
}

# e, a refused commit (M1.T35). A test-hooks build of the daemon sends
# one answer with no buffer when told `serve --refuse-one-commit`, and
# the driver refuses that commit (#83). serve must end with 8 and its
# line; the device must wait for a server and the client's request with
# it; and the next serve, with no hook, must be handed the request again
# and complete it: a write reads back through the device and from the
# home image, a read returns what was written before it, a flush lets
# the client's fsync return. The stand-in has no hooks: WAITING.
# A stop (M1.T35, review of #83): one write is held and its commit is
# refused, and the server is stopped during the hold. The stop sends
# the held write's commit at its last flush, and the refusal must still
# be seen: exit 8, not 0, since the stop was then not orderly. A build
# whose stop drops what that flush reaps (before 343dd01) ends 0 there:
# the negative control, by dispatch (daemon_ref 3c496d5, checks e).
RC_PINS="refused_read_commit_taken_over refused_write_commit_taken_over refused_flush_commit_taken_over refused_commit_at_a_stop_ends_8"
check_e_refused() {
    local k p
    if [ "$IMPL" = reference ]; then
        for p in $RC_PINS; do pin "$p" WAITING "the stand-in has no test hooks: it cannot have the driver refuse a commit"; done
        return
    fi
    for k in read write flush; do
        course "e, a refused commit of a $k" || continue
        step "e: the commit of a $k refused by the driver, then a take-over"
        rc_course "e-rc-$k" "$k"; rc_pin "refused_${k}_commit_taken_over" $?
    done
    course "e, a refused commit at a stop" || return
    step "e: a held write whose commit is refused, SIGTERM during the hold"
    rc_course e-rc-stop write hold; rc_pin refused_commit_at_a_stop_ends_8 $?
}
# e, a refused fetch (M1.T37). A test-hooks build of the daemon sends
# tag 0's first fetch with no buffer when told `serve --refuse-one-fetch
# start|recovery`; the driver refuses it, and the start (START_DEV) or
# the end of the recovery (END_USER_RECOVERY) then waits for that fetch
# for good (the control rig, M1.T36). serve gives the start up with exit
# 9 and its line (#87), and the volume is not failed.
# - At a start the device, which this serve added and never started, is
#   deleted: the number is free, and the next serve starts. That shows
#   that a delete answers a START_DEV that waits.
# - At a take-over the device is left waiting with the pool's I/O held,
#   and the next serve takes it over and the held I/O completes. That
#   shows that the end of the process answers an END_USER_RECOVERY that
#   waits.
# The stand-in has no hooks: WAITING.
RF_PINS="refused_fetch_at_a_start_gives_up_9 refused_fetch_at_a_take_over_gives_up_9"
check_e_fetch() {
    local p
    if [ "$IMPL" = reference ]; then
        for p in $RF_PINS; do pin "$p" WAITING "the stand-in has no test hooks: it cannot send a fetch the driver refuses"; done
        return
    fi
    if course "e, a refused fetch at a start"; then
        step "e: a first fetch refused by the driver at a start"
        rf9_start e-rf-start; rc_pin refused_fetch_at_a_start_gives_up_9 $?
    fi
    if course "e, a refused fetch at a take-over"; then
        step "e: a first fetch refused by the driver at a take-over, the pool's I/O held"
        rf9_take e-rf-take; rc_pin refused_fetch_at_a_take_over_gives_up_9 $?
    fi
}
# The words every line of a 9 ends with (#87).
RF9_TAIL="exit 9: a supervisor should retry the serve once; a second 9 is for a person"
# name -> RCT_WHY; 0 PASS, 1 FAIL, 2 the rig's error
rf9_start() {
    local name=$1 dir=$WORK/vol-$1 t0 rc said n free st back stop
    RCT_WHY=""
    vol_create "$SKYZFSD" "$dir" > /dev/null 2>> "$OUT/$name-create.err" || { RCT_WHY="create failed: $(tail -n 1 "$OUT/$name-create.err")"; return 2; }
    t0=$(now_ms)
    serve_start "$SKYZFSD" "$dir" --refuse-one-fetch start; rc=$?
    if [ "$rc" = 0 ]; then
        RCT_WHY="serve --refuse-one-fetch start became ready on $BDEV: no start was given up"
        kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30 || kill -9 "$SERVE_PID" 2>/dev/null; vol_detach "$SKYZFSD" "$dir"
        return 1
    fi
    if [ "$rc" != 1 ]; then
        RCT_WHY="serve neither ended nor became ready: ${SERVE_RC:-}"
        [ -z "$SERVE_PID" ] || kill -9 "$SERVE_PID" 2>/dev/null
        vol_detach "$SKYZFSD" "$dir"; return 2
    fi
    said=$(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-600)
    RCT_WHY="serve ended with exit $SERVE_RC after $(( $(now_ms) - t0 )) ms, saying: $said"
    rc=$SERVE_RC
    n=$(sed -n 's/.* of device \([0-9][0-9]*\)[ (,:].*/\1/p' <<< "$said" | head -n 1)
    free=""
    if [ -n "$n" ]; then
        for _ in $(seq 1 20); do ublk_number_free "$n" && { free=yes; break; }; sleep 0.25; done
        if [ -n "$free" ]; then RCT_WHY+="; device $n: its number was free (no node, unknown to the driver) within 5 s"
        else RCT_WHY+="; device $n: STILL THERE 5 s after ($(find /dev -maxdepth 1 -name "ublk[bc]$n" | tr '\n' ' '))"; fi
    else
        RCT_WHY+="; the line names no device"
    fi
    st=$(vol_stat "$SKYZFSD" "$dir" state)
    RCT_WHY+="; the volume's status: ${st:-none}"
    if serve_start "$SKYZFSD" "$dir"; then
        lim 30 "$DD" if="$BDEV" of=/dev/null bs=4096 count=1 iflag=direct status=none 2>> "$OUT/$name-dd.err"; back=$?
        kill -TERM "$SERVE_PID" 2>/dev/null
        if serve_wait 30; then stop=$SERVE_RC; else stop="none in 30 s"; kill -9 "$SERVE_PID" 2>/dev/null; SERVE_PID=""; fi
        RCT_WHY+="; the next serve: $(grep 'serving device' "$dir.serve.log" | tail -n 1 | cut -c1-120), a read of its first block exit $back, its stop exit $stop"
    else
        back=none; stop=none
        RCT_WHY+="; the next serve did not become ready (rc ${SERVE_RC:-none}: $(tail -n 1 "$dir.serve.log" | cut -c1-200))"
        [ -z "$SERVE_PID" ] || kill -9 "$SERVE_PID" 2>/dev/null
    fi
    vol_detach "$SKYZFSD" "$dir"
    [ "$rc" = "$EXIT_START_GIVEN_UP" ] && [[ $said == *"the kernel refused the first fetch of tag 0"* ]] \
        && [[ $said == *"which this serve added and never started, is deleted"* ]] && [[ $said == *"the volume is not failed; $RF9_TAIL" ]] \
        && [ -n "$free" ] && [ "${st:-}" != failed ] && [ "$back" = 0 ] && [ "$stop" = 0 ]
}
# The end of a take-over course that went wrong: a server for the device
# first (a pool on a device with none cannot be exported), then
# kill_cleanup.
rf9_cleanup() {
    if [ -z "$SERVE_PID" ] && [ "$(ublk_state "$bdev")" = QUIESCED ]; then serve_start "$SKYZFSD" "$dir" > /dev/null 2>&1; fi
    [ -z "${writer:-}" ] || job_end "$writer" 60
    kill_cleanup "$name"
}
rf9_take() {
    local name=$1 dir=$WORK/vol-$1 pool=skzd-$1 slog t0 rc said st n vst held pst perr wrc took writer="" bdev
    RCT_WHY=""
    vol_up "$SKYZFSD" "$dir" || { RCT_WHY="not created or served (serve rc ${SERVE_RC:-none})"; return 2; }
    bdev=$BDEV; n=${BDEV#/dev/ublkb}
    slog=$(new_slog "$name")
    pool_create "$pool" "$bdev" "$slog" > "$OUT/$name-create.log" 2>&1 || { RCT_WHY="zpool create failed"; rf9_cleanup; return 2; }
    lim 60 zpool sync "$pool" || { RCT_WHY="the first zpool sync failed"; rf9_cleanup; return 2; }
    kill -9 "$(server_pid "$SKYZFSD" "$dir")" 2>/dev/null
    serve_wait 30 || { RCT_WHY="the killed server did not end in 30 s"; rf9_cleanup; return 2; }
    t0=$(now_ms); st=""
    while [ $(( $(now_ms) - t0 )) -lt 20000 ]; do st=$(ublk_state "$bdev"); [ "$st" = QUIESCED ] && break; sleep 0.25; done
    [ "$st" = QUIESCED ] || { RCT_WHY="after the kill the device read ${st:-nothing}, not QUIESCED"; rf9_cleanup; return 2; }
    # The pool's I/O, held: a write synced to the data device (zpool
    # sync), which no server answers.
    rm -f "${WORK:?}/${name:?}.wrc"
    ( t0=$(now_ms); lim 300 "$DD" if=/dev/urandom of="$WORK/mnt/$pool/held" bs=1M count=4 status=none 2>> "$OUT/$name-dd.err" && lim 300 zpool sync "$pool"; echo "$? $(( $(now_ms) - t0 ))" > "$WORK/$name.wrc" ) &
    writer=$!
    sleep 3
    if [ -e "$WORK/$name.wrc" ]; then RCT_WHY="the pool's write and sync ended with no server: $(cat "$WORK/$name.wrc")"; rf9_cleanup; return 2; fi
    t0=$(now_ms)
    serve_start "$SKYZFSD" "$dir" --refuse-one-fetch recovery; rc=$?
    if [ "$rc" = 0 ]; then
        RCT_WHY="the take-over with --refuse-one-fetch recovery became ready: no start was given up"
        rf9_cleanup; return 1
    fi
    if [ "$rc" != 1 ]; then
        RCT_WHY="the take-over neither ended nor became ready: ${SERVE_RC:-}"
        [ -z "$SERVE_PID" ] || kill -9 "$SERVE_PID" 2>/dev/null
        SERVE_PID=""; rf9_cleanup; return 2
    fi
    said=$(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-600)
    rc=$SERVE_RC
    RCT_WHY="SIGKILL, then the pool's write and sync held; the take-over ended with exit $rc after $(( $(now_ms) - t0 )) ms, saying: $said"
    t0=$(now_ms); st=""
    while [ $(( $(now_ms) - t0 )) -lt 20000 ]; do st=$(ublk_state "$bdev"); [ "$st" = QUIESCED ] && break; sleep 0.25; done
    vst=$(vol_stat "$SKYZFSD" "$dir" state)
    if [ -e "$WORK/$name.wrc" ]; then held="had ended ($(cat "$WORK/$name.wrc"))"; else held="still held"; fi
    RCT_WHY+="; the device then read $(look_word "$st"); the volume's status: ${vst:-none}; the pool's write and sync $held; the pool read $(pool_state "$pool")"
    if serve_start "$SKYZFSD" "$dir"; then
        RCT_WHY+="; the next serve: $(grep 'serving device' "$dir.serve.log" | tail -n 1 | cut -c1-120)"
        if job_end "$writer" 120; then read -r wrc took < "$WORK/$name.wrc" 2>/dev/null || wrc="no status"; else wrc="NO END in 120 s after the take-over"; took=0; fi
        writer=""
        pst=$(pool_state "$pool"); perr=$(pool_errors "$pool")
        RCT_WHY+="; the held write and sync then exited $wrc ($(( ${took:-0} / 1000 )) s in all); the pool $pst, $perr"
    else
        wrc=none; pst=none; perr=none
        RCT_WHY+="; the next serve did not become ready (rc ${SERVE_RC:-none}: $(tail -n 1 "$dir.serve.log" | cut -c1-200))"
    fi
    rf9_cleanup
    [ "$rc" = "$EXIT_START_GIVEN_UP" ] && [[ $said == *"the kernel refused the first fetch of tag 0"* ]] \
        && [[ $said == *"device $n is left waiting for the next server"* ]] && [[ $said == *"the volume is not failed; $RF9_TAIL" ]] \
        && [ "$st" = QUIESCED ] && [ "${vst:-}" != failed ] && [ "$held" = "still held" ] \
        && [ "$wrc" = 0 ] && [ "$pst" = ONLINE ] && [[ $perr == "vdev errors 0 "* ]]
}
rc_pin() { case $2 in 0) pin "$1" PASS "$RCT_WHY" ;; 1) pin "$1" FAIL "$RCT_WHY" ;; *) pin "$1" ERROR "$RCT_WHY" ;; esac; }
# The end of a course that went wrong: the server stopped, the device
# deleted (which ends a client that waits on it), the client gone.
rc_cleanup() {
    if [ -n "$SERVE_PID" ]; then kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30 || kill -9 "$SERVE_PID" 2>/dev/null; fi
    vol_detach "$SKYZFSD" "$dir"
    [ -z "${client:-}" ] || job_end "$client" 30
}
# name read|write|flush [hold] -> RCT_WHY; 0 PASS, 1 FAIL, 2 the rig's error
rc_course() {
    local name=$1 kind=$2 hold=${3:-} dir=$WORK/vol-$1 pat=$WORK/$1.pat got=$WORK/$1.got client="" t0 rc said st held crc took waited back home stop
    local -a hook cmd
    RCT_WHY=""
    head -c 4096 /dev/urandom > "$pat"
    : > "$got"
    if [ "$kind" = flush ]; then hook=(--refuse-one-commit flush); else hook=(--refuse-one-commit "$kind:$RC_AT"); fi
    [ -z "$hold" ] || hook+=(--hold-one-request "$HOLD_S:write:$RC_AT")
    if ! vol_up "$SKYZFSD" "$dir" "${hook[@]}"; then
        RCT_WHY="not created or served with ${hook[*]} (serve rc ${SERVE_RC:-none}: $(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-200))"
        rc_cleanup; return 2
    fi
    # For a read and a flush, the block is written first, by a dd of its
    # own: a write is not refused. A flush refused before then (the
    # client's must be the first) leaves that write waiting, and it fails
    # at its limit: the rig's error, not a course on someone else's flush.
    if [ "$kind" != write ] && ! lim 30 "$DD" if="$pat" of="$BDEV" bs=4096 count=1 seek=$((RC_AT / 4096)) oflag=direct conv=notrunc status=none 2>> "$OUT/$name-dd.err"; then
        RCT_WHY="the write before the $kind failed: $(tail -n 1 "$OUT/$name-dd.err")"; rc_cleanup; return 2
    fi
    case $kind in
        read) cmd=("$DD" if="$BDEV" of="$got" bs=4096 count=1 skip=$((RC_AT / 4096)) iflag=direct status=none) ;;
        flush)
            # The flush alone: an fsync of the device, nothing written.
            # shellcheck disable=SC2054  # dd's conv= is a list with commas
            cmd=("$DD" if=/dev/null of="$BDEV" conv=notrunc,fsync status=none) ;;
        *) cmd=("$DD" if="$pat" of="$BDEV" bs=4096 count=1 seek=$((RC_AT / 4096)) oflag=direct conv=notrunc status=none) ;;
    esac
    rm -f "$WORK/$name.rc"
    ( t0=$(now_ms); lim 240 "${cmd[@]}" 2>> "$OUT/$name-dd.err"; echo "$? $(( $(now_ms) - t0 ))" > "$WORK/$name.rc" ) &
    client=$!
    t0=$(now_ms)
    if [ -n "$hold" ]; then
        # 30 s in all, whatever each look takes.
        while [ $(( $(now_ms) - t0 )) -lt 30000 ]; do
            held=$(vol_stat "$SKYZFSD" "$dir" held); [ "${held:-0}" -ge 1 ] && break; sleep 0.25
        done
        if [ "${held:-0}" -lt 1 ]; then RCT_WHY="status never said the write was held"; rc_cleanup; return 2; fi
        kill -TERM "$SERVE_PID" 2>/dev/null
        RCT_WHY="SIGTERM while the write was held; "
        t0=$(now_ms)
    fi
    # serve ends by itself at the refusal (or at the stop): within 60 s.
    if ! serve_wait 60; then
        RCT_WHY+="serve did not end in 60 s; the device read $(look_word "$(ublk_state "$BDEV")"); the client had $([ -e "$WORK/$name.rc" ] && echo "ended ($(cat "$WORK/$name.rc"))" || echo "not ended")"
        rc_cleanup; return 1
    fi
    rc=$SERVE_RC
    said=$(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-300)
    RCT_WHY+="serve ended with exit $rc after $(( $(now_ms) - t0 )) ms, saying: $said"
    # The driver moves the device to QUIESCED in a work item: 20 s in all.
    t0=$(now_ms); st=""
    while [ $(( $(now_ms) - t0 )) -lt 20000 ]; do st=$(ublk_state "$BDEV"); [ "$st" = QUIESCED ] && break; sleep 0.25; done
    sleep 2
    if [ -e "$WORK/$name.rc" ]; then waited="had ended ($(cat "$WORK/$name.rc")) before the take-over"; else waited="still waited 2 s after"; fi
    RCT_WHY+="; the device then read $(look_word "$st"); the client $waited"
    if ! serve_start "$SKYZFSD" "$dir"; then
        RCT_WHY+="; the take-over did not become ready (rc ${SERVE_RC:-none}: $(tail -n 1 "$dir.serve.log" 2>/dev/null | cut -c1-200))"
        rc_cleanup; return 1
    fi
    if job_end "$client" 120; then read -r crc took < "$WORK/$name.rc" 2>/dev/null || crc="no status"; else crc="NO END in 120 s after the take-over"; took=0; fi
    client=""
    RCT_WHY+="; after the take-over the client exited $crc ($(( ${took:-0} / 1000 )) s in all)"
    lim 30 "$DD" if="$BDEV" of="$WORK/$name.back" bs=4096 count=1 skip=$((RC_AT / 4096)) iflag=direct status=none 2>> "$OUT/$name-dd.err"
    if cmp -s "$pat" "$WORK/$name.back"; then back=same; else back=DIFFERENT; fi
    [ "$kind" != read ] || { cmp -s "$pat" "$got" && back+=", and the client read it" || back+=", and the client read SOMETHING ELSE"; }
    kill -TERM "$SERVE_PID" 2>/dev/null
    if serve_wait 30; then stop=$SERVE_RC; else stop="none in 30 s"; kill -9 "$SERVE_PID" 2>/dev/null; SERVE_PID=""; fi
    lim 30 "$DD" if="$dir/home.img" of="$WORK/$name.home" bs=4096 count=1 skip=$((RC_AT / 4096)) status=none 2>> "$OUT/$name-dd.err"
    if cmp -s "$pat" "$WORK/$name.home"; then home=same; else home=DIFFERENT; fi
    RCT_WHY+="; the block read back through the device: $back; the next stop: exit $stop; the home image's block: $home"
    vol_detach "$SKYZFSD" "$dir"
    case $back in same|"same, and the client read it") ;; *) return 1 ;; esac
    [ "$rc" = "$EXIT_COMMIT_REFUSED" ] && [[ $said == *"refused serve's answer"* ]] && [ "$st" = QUIESCED ] \
        && [ "$waited" = "still waited 2 s after" ] && [ "$crc" = 0 ] && [ "$stop" = 0 ] && [ "$home" = same ]
}

# ------------------------------------------------------------ check g
# The disk under the home image fails, for real (design §4.1, M1.T23).
# Check d makes its failures with the server's hooks; this one makes a
# write of the volume's own file fail in the kernel. The volume's
# directory is on an ext4 without a journal (so that a failed write
# fails the data and nothing else; errors=continue keeps the file
# system going) on a device-mapper device that maps a loop device one
# to one. The rig replaces that table by dm-error over the whole device
# or over the sectors of one range of the home image (FIEMAP), and puts
# the linear table back. The daemon only: the stand-in has no rule of
# its own here.
G_DM=skzg; G_LOOP=""; G_SECTORS=""; G_MNT=$WORK/g-fs
FW=$HERE/failed_write.py
G_AT=$((64 << 20)); G_LEN=65536   # check g.2's range of the volume
G_BOOT=$((1 << 20))               # in the 3.5 MiB ZFS leaves free after its first two labels
G_PINS="start_serves_what_the_disk_holds failed_start_fails_volume failed_start_suspends_pool failed_start_volume_refused sync_failure_fails_volume sync_failure_suspends_pool sync_failed_volume_refused failure_reason_is_the_images data_journal_refused"
g_fs_up() {
    lim 30 truncate -s 16G "$WORK/g-disk.img" || return 1
    G_LOOP=$(lim 30 losetup -f --show "$WORK/g-disk.img") || return 1
    G_SECTORS=$(lim 30 blockdev --getsz "$G_LOOP") || return 1
    echo "0 $G_SECTORS linear $G_LOOP 0" | lim 30 dmsetup create "$G_DM" || return 1
    lim 300 mkfs.ext4 -q -F -O ^has_journal "/dev/mapper/$G_DM" || return 1
    mkdir -p "$G_MNT" && lim 30 mount -o errors=continue "/dev/mapper/$G_DM" "$G_MNT"
}
# linear | error | range START COUNT (512-byte sectors): the table of
# the device under check g's file system. Suspended without a freeze of
# the file system, which would try to write what cannot be written.
g_table() {
    local t
    case $1 in
        linear) t="0 $G_SECTORS linear $G_LOOP 0" ;;
        error) t="0 $G_SECTORS error" ;;
        range)
            t=""
            [ "$2" -gt 0 ] && t="0 $2 linear $G_LOOP 0"$'\n'
            t+="$2 $3 error"
            [ $(( $2 + $3 )) -ge "$G_SECTORS" ] || t+=$'\n'"$(( $2 + $3 )) $(( G_SECTORS - $2 - $3 )) linear $G_LOOP $(( $2 + $3 ))" ;;
        *) return 2 ;;
    esac
    lim 60 dmsetup suspend --nolockfs "$G_DM" || return 1
    printf '%s\n' "$t" | lim 30 dmsetup load "$G_DM" || { lim 60 dmsetup resume "$G_DM"; return 1; }
    lim 60 dmsetup resume "$G_DM" || return 1
    echo "$1 ${2:-} ${3:-}: $(lim 30 dmsetup table "$G_DM" | tr '\n' ';')" >> "$OUT/g-tables.log"
}
fw() { lim 60 python3 -I "$FW" "$@" 2>&1; }
# name -> the volume's directory: on the failing file system, reached by
# a link from $WORK, so that the serve's own files ($dir.ready,
# $dir.serve.log) stay on the rig's disk, which does not fail.
g_dir() { mkdir -p "$G_MNT/vol-$1" && ln -sfn "$G_MNT/vol-$1" "$WORK/vol-$1" && echo "$WORK/vol-$1"; }

check_g() {
    local p
    step "g: a file system whose disk can fail, under the volumes"
    if ! g_fs_up > "$OUT/g-fs.log" 2>&1; then
        for p in $G_PINS; do pin "$p" ERROR "check g's file system was not set up: $(tail -n 2 "$OUT/g-fs.log" | tr '\n' ' ')"; done
        ctl g_old_content_after_good_sync ERROR "check g's file system was not set up"
        obs g_failed_sync "not run: no file system"; obs g_new_descriptor_after_failed_sync "not run: no file system"
        obs g_data_journal_cache "not run: check g did not start"; obs g_chattr_journal "not run: check g did not start"
        return
    fi
    rec g_fs "ext4 without a journal, errors=continue, on /dev/mapper/$G_DM over $G_LOOP ($G_SECTORS sectors)"
    course g.3 && check_g_start_after_failed_sync
    # Both leave a suspended pool.
    course g.2 && check_g_failed_start
    course g.1 && check_g_sync_fails_while_serving
    course "g, the failure's reason" && check_g_reason
    course g.4 && check_g_data_journal
}

# g.4 (M1.3y): on ext4 mounted data=journal a file's data goes through
# the journal, and statx reports no direct-I/O alignment for it, so the
# in-place store, which does all its I/O direct, cannot serve the image
# from the disk alone. The start must refuse the volume by reason, with
# no ready line and its files untouched, and leave it unfailed: the same
# file system mounted again data=ordered serves it. The control is the
# serve on data=ordered before and after, and the mount's own options.
# Its own file system on a loop file, not check g's failing one, which
# has no journal. Two observations beside it (M1.T30): after each serve,
# what the page cache holds of the home image once 16 MiB were written
# to the device with O_DIRECT and synced (cachestat: a store that writes
# past the cache leaves none; one served through it leaves the 4096
# pages), and what serve does when the image alone journals its data
# (chattr +j on data=ordered).
GJ_MIB=16
# What a refused start may leave changed in the volume's directory (the
# lead, M1.T30): nothing but the status file. The image and the volume's
# identity file (`volume`, from M1.3t) are byte-identical, and no
# failure marker appears or goes (log.failed, volume.failed). The
# allowance for the log store's files (log.super, log.N), which its
# checkpoint at a start rewrote, went with the log store (M1.3t). Every
# changed file is named in the line.
GJ_MAY_CHANGE='^status$'
# dir -> "DIGEST  NAME" for every entry under it, dotfiles and
# directories too (a directory's digest is "dir"), sorted by name
gj_digests() {
    (cd "$1" && find . -mindepth 1 -printf '%P\n' | sort | while IFS= read -r f; do
        if [ -f "$f" ] && [ ! -L "$f" ]; then printf '%s  %s\n' "$(sha256sum < "$f" | cut -d' ' -f1)" "$f"
        else printf '%s  %s\n' "$(stat -c %F "$f" | tr ' ' '_')" "$f"; fi
    done)
}
# new-or-rewritten gone-or-rewritten -> what is wrong ("" when nothing)
gj_files_judged() {
    local f out=""
    for f in $1 $2; do
        case $f in
            home.img|volume) [[ $out == *"$f changed"* ]] || out+="$f changed; " ;;
            log.failed|volume.failed)
                if [[ " $1 " == *" $f "* ]]; then [[ $out == *"$f appeared"* ]] || out+="a failure marker $f appeared or was rewritten; "
                else [[ $out == *"$f removed"* ]] || out+="a failure marker $f removed; "; fi ;;
            *) [[ $f =~ $GJ_MAY_CHANGE ]] || [[ $out == *"$f rewritten"* ]] || out+="$f rewritten; " ;;
        esac
    done
    printf '%s' "$out"
}
check_g_data_journal() {
    local img=$WORK/g-journal.img mnt=$WORK/g-journal loop="" link=$WORK/vol-gj a="" bad="" rc said before after mode
    local cache="" attr j="" changed gone
    gj_mount() { # data mode -> 0 when mounted with it, as the mount table says
        lim 30 mount -o "data=$1" "$loop" "$mnt" >> "$OUT/gj-setup.log" 2>&1 || return 1
        mode=$(findmnt -n -o OPTIONS "$mnt" | tr ',' '\n' | sed -n 's/^data=//p')
        # The kernel names data=ordered, its default, only in some versions.
        [ "${mode:-ordered}" = "$1" ]
    }
    gj_cache() { # -> what the cache holds of the image after a write to the device
        local w
        lim 60 "$DD" if=/dev/urandom of="$BDEV" bs=1M count=$GJ_MIB seek=64 oflag=direct conv=fsync,nocreat status=none 2>> "$OUT/gj-setup.log"
        w=$?
        echo "$GJ_MIB MiB written to the device with O_DIRECT and synced (exit $w), then cachestat of the image: $(lim 30 python3 -I "$HERE/cachestat.py" "$mnt/vol/home.img" 2>&1 | head -n 1)"
    }
    gj_serves() { # what: serve, measure, stop; 0 when it served
        local c
        if serve_start "$SKYZFSD" "$link"; then
            c=$(gj_cache)
            a+="$1: served as $BDEV; "; cache+="$1: $c; "
            kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
            return 0
        fi
        a+="$1: NOT served (serve rc ${SERVE_RC:-none}: $(tail -n 1 "$link.serve.log" | cut -c1-200)); "
        return 1
    }
    gj_end() {
        [ -z "${SERVE_PID:-}" ] || { kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30; }
        vol_detach "$SKYZFSD" "$link"
        lim 60 umount "$mnt" > /dev/null 2>&1
        [ -z "$loop" ] || lim 30 losetup -d "$loop" > /dev/null 2>&1
    }
    gj_obs() { # the two observations, whatever happened
        obs g_data_journal_cache "${cache:-not run: no serve got as far as a write}"
        obs g_chattr_journal "${j:-not run: the steps before it did not end}"
    }
    step "g.4: ext4 mounted data=journal under a volume"
    : > "$OUT/gj-setup.log"
    mkdir -p "$mnt"
    if ! lim 30 truncate -s 16G "$img" >> "$OUT/gj-setup.log" 2>&1 \
        || ! loop=$(lim 30 losetup -f --show "$img" 2>> "$OUT/gj-setup.log") \
        || ! lim 300 mkfs.ext4 -q -F "$loop" >> "$OUT/gj-setup.log" 2>&1 || ! gj_mount ordered \
        || ! mkdir -p "$mnt/vol" || ! ln -sfn "$mnt/vol" "$link"; then
        pin data_journal_refused ERROR "ext4 with a journal was not made and mounted data=ordered: $(tail -n 1 "$OUT/gj-setup.log" | cut -c1-200)"
        gj_end; gj_obs; return
    fi
    if ! vol_create "$SKYZFSD" "$link" >> "$OUT/gj-setup.log" 2>&1; then
        pin data_journal_refused ERROR "the volume was not created on ext4 data=ordered: $(tail -n 1 "$OUT/gj-setup.log" | cut -c1-200)"
        gj_end; gj_obs; return
    fi
    # 1: the control, before
    gj_serves "on data=ordered, before" || bad+="the serve before; "
    lim 60 umount "$mnt" >> "$OUT/gj-setup.log" 2>&1
    if ! gj_mount journal; then
        pin data_journal_refused ERROR "the file system was not mounted again data=journal (the mount table says data=${mode:-nothing}): $(tail -n 1 "$OUT/gj-setup.log" | cut -c1-200)"
        gj_end; gj_obs; return
    fi
    # 2: the refusal. A serve here is a FAIL, and what the cache then
    # holds is the figure that says how it served.
    # Each file's digest: the image must be as it was; any other file a
    # refused start rewrote is named, not judged.
    before=$(gj_digests "$mnt/vol")
    # The digest read the whole image through the cache: evicted, so that
    # what the cache holds after the serve is the server's (Codex, #72).
    cache+="on data=journal, before the serve: $(lim 30 python3 -I "$HERE/cachestat.py" --drop "$mnt/vol/home.img" 2>&1 | head -n 1); "
    serve_start "$SKYZFSD" "$link"; rc=$?
    said=$(grep -m 1 'no direct I/O' "$link.serve.log" | sed 's/^skyzfsd: //' | cut -c1-240)
    if [ $rc = 0 ]; then
        a+="on data=journal: SERVED as $BDEV; "; bad+="the volume was served on data=journal; "
        cache+="on data=journal: $(gj_cache); "
        kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    else
        a+="on data=journal: serve exit ${SERVE_RC:-none}, $([ -n "$said" ] && echo "said: $said" || echo "did NOT give the reason (its last line: $(tail -n 1 "$link.serve.log" | cut -c1-200))"); "
        { [ $rc = 1 ] && [ "$SERVE_RC" = 1 ] && [ -n "$said" ] && [[ $said == *"has not failed"* ]]; } || bad+="the refusal; "
        ! grep -q '^ready' "$link.ready" || bad+="a ready line; "
        cache+="on data=journal: refused, nothing written; "
        after=$(gj_digests "$mnt/vol")
        changed=$(diff <(echo "$before") <(echo "$after") | sed -n 's/^> [^ ]*  //p' | tr '\n' ' ')
        gone=$(diff <(echo "$before") <(echo "$after") | sed -n 's/^< [^ ]*  //p' | tr '\n' ' ')
        a+="files of the volume new or rewritten: ${changed:-none}; gone or rewritten: ${gone:-none}; "
        bad+=$(gj_files_judged "$changed" "$gone")
    fi
    lim 60 umount "$mnt" >> "$OUT/gj-setup.log" 2>&1
    # 3: not failed: the control, after
    if ! gj_mount ordered; then
        pin data_journal_refused ERROR "the file system was not mounted again data=ordered: $(tail -n 1 "$OUT/gj-setup.log" | cut -c1-200) ($a)"
        gj_end; gj_obs; return
    fi
    gj_serves "on data=ordered again" || bad+="the serve after; "
    if [ -z "$bad" ]; then pin data_journal_refused PASS "$a"; else pin data_journal_refused FAIL "wrong: $bad$a"; fi
    # 4: the observation: the image alone journals its data.
    if ! lim 30 chattr +j "$mnt/vol/home.img" >> "$OUT/gj-setup.log" 2>&1; then
        j="chattr +j on the image was refused: $(tail -n 1 "$OUT/gj-setup.log" | cut -c1-200)"
    else
        attr=$(lsattr "$mnt/vol/home.img" 2>&1 | cut -d' ' -f1)
        if serve_start "$SKYZFSD" "$link"; then
            j="with chattr +j (lsattr $attr) on data=ordered: SERVED as $BDEV; $(gj_cache)"
            kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
        else
            j="with chattr +j (lsattr $attr) on data=ordered: serve exit ${SERVE_RC:-none}: $(tail -n 1 "$link.serve.log" | sed 's/^skyzfsd: //' | cut -c1-240)"
        fi
        lim 30 chattr -j "$mnt/vol/home.img" >> "$OUT/gj-setup.log" 2>&1 || j+="; chattr -j afterwards failed"
    fi
    gj_end; gj_obs
}

# The reason the daemon gives when the home image's write or sync fails,
# where an operator reads it: the failed serve's line and the refusal of
# the serve after it, in g.3 (at a start) and g.1 (while served). g.2
# fails nothing. The in-place store's reason (M1.3x, code 7 of the
# failure note) is the image's, not the log's.
G_REASON="a write or sync of the volume's image failed"
check_g_reason() {
    local said="" bad=0 miss="" place line name file lead
    for place in "g3 start:$WORK/vol-g3.serve.log:the volume failed at the start of the in-place store" \
                 "g3 refusal:$OUT/g3-again.err:skyzfsd: " \
                 "g1 serve:$WORK/vol-g1.serve.log:the volume failed while it was served" \
                 "g1 refusal:$OUT/g1-again.err:skyzfsd: "; do
        IFS=: read -r name file lead <<< "$place"
        # A serve's log: its first line of the failure. A refusal: the
        # refusal is the last line it writes, after any note.
        line=$(grep -F -- "$lead" "$file" 2>/dev/null | if [[ $name == *refusal ]]; then tail -n 1; else head -n 1; fi | sed 's/^skyzfsd: //' | cut -d';' -f1 | cut -c1-200)
        if [ -z "$line" ]; then miss="$miss $name"; continue; fi
        [[ $line == *"$G_REASON"* ]] || bad=$((bad + 1))
        said="$said$name: \"$line\"; "
    done
    # A place with no line is a FAIL: the daemon did not fail the volume
    # where it must (the start served, or the serve did not fail), or the
    # part did not run, and then its own pins are ERROR already.
    if [ "${G1_PREMISE:-}" = missed ]; then
        pin failure_reason_is_the_images ERROR "g.1's premise did not happen (the rig's write could not be issued), so there is no failure of g.1 to give a reason for; ${said:-nothing read}"
    elif [ -n "$miss" ]; then
        pin failure_reason_is_the_images FAIL "no failure line of the daemon's at:$miss; ${said:-nothing read}"
    elif [ "$bad" = 0 ]; then
        pin failure_reason_is_the_images PASS "all four name the image: $said"
    else
        pin failure_reason_is_the_images FAIL "$bad of 4 do not say \"$G_REASON\": $said"
    fi
}

# g.2: pages of the home image that a failed sync left in the cache,
# reading as written, and a disk that never got them. The rig makes
# them itself, with no daemon running: a write of 64 KiB in place of a
# range the disk holds, its fdatasync failing on the range's sectors.
# The error is then taken, and the next start's own sync succeeds (the
# kernel reports a write-back error once). A start that serves from the
# cache would serve what the disk does not hold (§4.1: "every start,
# after its sync, drops the kernel's cache of the volume's file and
# checks that it is gone, and serves only what the disk holds"). The
# control does the same with no error.
check_g_start_after_failed_sync() {
    local name dir ext said disk again cached served ok
    for name in g0 g2; do
        dir=$(g_dir "$name")
        if [ "$name" = g0 ]; then step "g, control: a start after a sync that succeeded"; else step "g: a start after a sync that failed"; fi
        said=""; ext=""; served=""
        if vol_create "$SKYZFSD" "$dir" > "$OUT/$name-create.log" 2>&1; then
            said=$(fw fill "$dir/home.img" "$G_AT" "$G_LEN" 0x11)
            ext=$(fw extent "$dir/home.img" "$G_AT" "$G_LEN")
        fi
        if [ "$said" != filled ] || ! [[ $ext =~ ^[0-9]+\ [0-9]+$ ]]; then
            ok="the volume was not made, or its range not written and found: create $(tail -n 1 "$OUT/$name-create.log"); fill '$said'; extent '$ext'"
            if [ "$name" = g0 ]; then ctl g_old_content_after_good_sync ERROR "$ok"; else pin start_serves_what_the_disk_holds ERROR "$ok"; obs g_failed_sync "not run: $ok"; obs g_new_descriptor_after_failed_sync "not run: $ok"; fi
            continue
        fi
        # shellcheck disable=SC2086  # START COUNT
        [ "$name" = g0 ] || g_table range $ext || { pin start_serves_what_the_disk_holds ERROR "the table could not be changed"; continue; }
        said=$(fw write "$dir/home.img" "$G_AT" "$G_LEN" 0x22)
        [ "$name" = g0 ] || g_table linear || { pin start_serves_what_the_disk_holds ERROR "the linear table could not be put back"; continue; }
        disk=$(fw direct "$dir/home.img" "$G_AT" "$G_LEN")
        again=$(fw sync "$dir/home.img")
        # What the cache holds as the start meets it: the last look.
        cached=$(fw read "$dir/home.img" "$G_AT" "$G_LEN")
        rec "${name}_before_the_start" "a write of 0x22 over 0x11 at $G_AT (sectors $ext): $said; the disk then holds $disk; a new descriptor: $again; the cache just before the start reads $cached"
        if serve_start "$SKYZFSD" "$dir"; then
            served=$(fw direct "$BDEV" "$G_AT" "$G_LEN")
            kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
        else
            served="nothing: the serve did not start (rc ${SERVE_RC:-none}; $(tail -n 1 "$dir.serve.log" | cut -c1-300))"
        fi
        vol_detach "$SKYZFSD" "$dir"
        if [ "$name" = g0 ]; then
            # The pin's test, on a write that synced: the start serves
            # the write, so "it serves 0x11" fails.
            if ! [[ $said == "fdatasync ok; cache reads byte 0x22" && $disk == "byte 0x22" && $cached == "byte 0x22" ]]; then
                ctl g_old_content_after_good_sync ERROR "the rig's write did not sync as it should: $said; the disk holds $disk"
            elif [ "$served" = "byte 0x11" ]; then
                ctl g_old_content_after_good_sync PASS "the write synced and the disk holds 0x22, and the start served 0x11"
            elif [ "$served" = "byte 0x22" ]; then
                ctl g_old_content_after_good_sync FAIL "the write synced and the disk holds 0x22; the start served 0x22"
            else
                ctl g_old_content_after_good_sync ERROR "the write synced and the disk holds 0x22; the start served $served"
            fi
            continue
        fi
        # What the kernel did is the check's premise, and an observation.
        obs g_failed_sync "the rig's write: $said; the disk then holds $disk (on $(uname -r))"
        obs g_new_descriptor_after_failed_sync "after the rig's own fdatasync took the error, a newly opened descriptor: $again"
        if ! [[ $said == fdatasync\ EIO* && $said == *"cache reads byte 0x22" && $disk == "byte 0x11" && $cached == "byte 0x22" ]]; then
            pin start_serves_what_the_disk_holds ERROR "the kernel did not leave what the check needs (a failed fdatasync, the cache reading 0x22 up to the start, the disk 0x11): $said; disk $disk; just before the start $cached"
        elif [ "$served" = "byte 0x11" ]; then
            pin start_serves_what_the_disk_holds PASS "the cache read 0x22 after the failed sync and the disk held 0x11; the start served 0x11"
        else
            pin start_serves_what_the_disk_holds FAIL "the cache read 0x22 after the failed sync and the disk held 0x11; the start served $served"
        fi
    done
}

# g.3: a start whose own sync fails while a client holds the device.
# The pool's server is killed (the device waits for the next one), the
# disk fails under one block of the home image, a dirty page is left
# there in the cache, as by the instance that died, and a new serve
# starts. Its sync fails: the volume has failed at the start, with
# status 3, and the pool suspends without an application error; a
# start after the disk is back is refused while the client holds the
# device (§4.1, §4.2). The block is in the part of the device ZFS
# leaves free, so the pool's own data is not in the failure.
check_g_failed_start() {
    local name=g3 dir ext said mark rc n
    dir=$(g_dir g3)
    step "g: a start whose sync fails, a client holding the device"
    if ! FS_DIR=$dir fail_setup "$name" "$SKYZFSD"; then
        for n in failed_start_fails_volume failed_start_suspends_pool failed_start_volume_refused; do pin "$n" ERROR "$FS_ERR"; done
        fail_stop "$name"; return
    fi
    # A block of the home image on the disk, where the pool does not
    # look: written into the file directly (a kernel without
    # BLK_DEV_WRITE_MOUNTED refuses a write to a device ZFS holds).
    said=$(fw fill "$dir/home.img" "$G_BOOT" 4096 0x33)
    ext=$(fw extent "$dir/home.img" "$G_BOOT" 4096)
    kill -9 "$SERVE_PID" 2>/dev/null; serve_wait 30
    for n in $(seq 1 80); do [ "$(ublk_state "$BDEV")" = QUIESCED ] && break; sleep 0.25; done
    rec g3_after_kill "the device is $(ublk_state "$BDEV"); a block of the home image written and synced: $said, at sectors $ext"
    lim 60 sync -f "$G_MNT/"
    # shellcheck disable=SC2086  # START COUNT
    if [ "$said" != filled ] || ! [[ $ext =~ ^[0-9]+\ [0-9]+$ ]] || ! g_table range $ext; then
        for n in failed_start_fails_volume failed_start_suspends_pool failed_start_volume_refused; do pin "$n" ERROR "the block was not written ($said), found ($ext) or put on a failing range"; done
        fail_stop "$name"; return
    fi
    said=$(fw dirty "$dir/home.img" "$G_BOOT" 4096 0x44)
    rec g3_dirty "the dead instance's last write, as the rig left it in the cache: $said; Dirty in /proc/meminfo: $(awk '$1 == "Dirty:" { print $2, $3 }' /proc/meminfo)"
    mark=$(klog_mark)
    serve_start "$SKYZFSD" "$dir"; rc=$?
    case $rc in
        0) pin failed_start_fails_volume FAIL "the start served the device ($BDEV) although its sync must fail"
           kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30 ;;
        1) n=$(grep -m 1 -o 'the volume failed at the start of the in-place store[^;]*' "$dir.serve.log")
           if [ "$SERVE_RC" = "$EXIT_FAILED" ] && [ -n "$n" ]; then
               pin failed_start_fails_volume PASS "serve exited $SERVE_RC: $n"
           else
               pin failed_start_fails_volume FAIL "serve exited $SERVE_RC, wanted $EXIT_FAILED with \"the volume failed at the start\"; its last line: $(tail -n 1 "$dir.serve.log" | cut -c1-300)"
           fi ;;
        *) pin failed_start_fails_volume ERROR "the serve neither started nor ended: $SERVE_RC" ;;
    esac
    if fail_wait "$name" 120 "$mark"; then pin failed_start_suspends_pool PASS "$FS_WHY"; else pin failed_start_suspends_pool FAIL "$FS_WHY"; fi
    if ! g_table linear; then
        pin failed_start_volume_refused ERROR "the linear table could not be put back"
    else
        lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --run-dir "$RUN_DIR" > /dev/null 2> "$OUT/g3-again.err"; rc=$?
        pin_is failed_start_volume_refused "with the disk back, serve exited $rc, wanted $EXIT_REFUSED_FAILED: $(tail -n 1 "$OUT/g3-again.err" | cut -c1-300)" [ "$rc" = "$EXIT_REFUSED_FAILED" ]
    fi
    fail_stop "$name"
}

# g.1: the disk under the home image fails while the volume is served
# and written. Every write of the device fails from then on, and every
# read too (dm-error fails both); the daemon's next sync of the home
# image fails. The volume has failed: status 3, the pool suspends with
# no write failed to an application, and with the disk back a new serve
# is refused while the client holds the device (§4.1, §4.2). A read
# that fails before the volume does fails its request alone (§5.1): the
# check does not judge reads but records, in obs_g_reads_while_failing,
# how many failed to ZFS and whether the reader saw one before the
# volume failed.
check_g_sync_fails_while_serving() {
    local name=g1 dir mark rc n at seen before watch rw w0 w1 f0 f1 wrc held
    dir=$(g_dir g1)
    step "g: the disk fails while the volume is served"
    if ! FS_DIR=$dir fail_setup "$name" "$SKYZFSD"; then
        for n in sync_failure_fails_volume sync_failure_suspends_pool sync_failed_volume_refused; do pin "$n" ERROR "$FS_ERR"; done
        obs g_reads_while_failing "not run: $FS_ERR"
        fail_stop "$name"; return
    fi
    sleep 5
    mark=$(klog_mark)
    # When the daemon says the volume failed, to a tenth of a second. It
    # looks for up to 600 s, longer than the rig waits (300 s for the
    # pool, then 60 s for the serve), and is stopped when the serve ends.
    : > "$WORK/g1.failed-at"
    ( for n in $(seq 1 6000); do
          if grep -q 'the volume failed' "$dir.serve.log" 2>/dev/null; then date +%s.%N > "$WORK/g1.failed-at"; exit 0; fi
          sleep 0.1
      done ) &
    watch=$!
    # The daemon's counters at the switch: a write or flush it finished
    # afterwards, whatever its result, moves them.
    w0=$(vol_stat "$SKYZFSD" "$dir" writes); f0=$(vol_stat "$SKYZFSD" "$dir" flushes)
    if ! g_table error; then
        for n in sync_failure_fails_volume sync_failure_suspends_pool sync_failed_volume_refused; do pin "$n" ERROR "the table could not be changed"; done
        obs g_reads_while_failing "not run: the table could not be changed"
        kill "$watch" 2>/dev/null; fail_stop "$name"; return
    fi
    # The premise made sure of: a write of the rig's own reaches the
    # daemon after the switch, outside the pool's data (the boot area
    # after the first two labels, as in g.2), with O_DIRECT and O_DSYNC,
    # so that a write and a flush of the device reach the failing disk
    # whatever ZFS does. In one run ZFS suspended the pool on its own
    # reads before it wrote again (43521074465), and the daemon, whose
    # failed reads fail their requests alone (§5.1), had nothing to fail
    # on: a run without this write tests nothing of a failed sync.
    lim 30 "$DD" if=/dev/urandom of="$BDEV" bs=64K count=1 seek=$((G_BOOT / 65536)) oflag=direct,dsync conv=nocreat status=none 2> "$OUT/g1-rig-write.err"; wrc=$?
    rw="the rig's own write of 64 KiB at byte $G_BOOT after the switch, O_DIRECT and O_DSYNC: exit $wrc$([ "$wrc" = 124 ] && echo " (not answered in 30 s)")$([ "$wrc" = 0 ] || [ "$wrc" = 124 ] || echo ", $(tail -n 1 "$OUT/g1-rig-write.err" | cut -c1-120)")"
    if FS_READS=observed fail_wait "$name" 300 "$mark"; then pin sync_failure_suspends_pool PASS "$FS_WHY"; else pin sync_failure_suspends_pool FAIL "$FS_WHY"; fi
    # And at the suspension: from the status file, which a failed
    # instance leaves behind.
    w1=$(vol_stat "$SKYZFSD" "$dir" writes); f1=$(vol_stat "$SKYZFSD" "$dir" flushes)
    rw+="; the daemon's writes ${w0:-?} then ${w1:-?}, flushes ${f0:-?} then ${f1:-?}, from the switch to the suspension"
    # The serve first: a daemon that exits failed with its line passes,
    # whatever the counters say (they count finished requests, and the
    # status file is written on an interval). Only one still serving is
    # judged by what it was handed: the rig's write not answered, or a
    # request held, is a request held on a failed disk; a write answered
    # while the volume did not fail is a volume that should have; and
    # only a write that could not be issued at all leaves g.1 without its
    # premise, the rig's error.
    G1_PREMISE=""
    if serve_wait 60; then
        n=$(grep -m 1 -o 'the volume failed while it was served[^;]*' "$dir.serve.log")
        if [ "$SERVE_RC" = "$EXIT_FAILED" ] && [ -n "$n" ]; then
            pin sync_failure_fails_volume PASS "serve exited $SERVE_RC: $n (a write or a sync of the image failed: the rig's own write may be the one); $rw"
        else
            pin sync_failure_fails_volume FAIL "serve exited $SERVE_RC, wanted $EXIT_FAILED with \"the volume failed while it was served\": ${n:-no line of a failure in its log}; $rw"
        fi
    else
        held=$(vol_stat "$SKYZFSD" "$dir" held)
        rw+="; status held '${held:-not said}' 60 s after the pool's wait ended"
        if [ "$wrc" = 124 ] || [[ ${held:-0} =~ ^[1-9][0-9]*$ ]]; then
            pin sync_failure_fails_volume FAIL "serve was still running 60 s after the pool's wait ended, with a request held on the failed disk; $rw"
        # The device would not open: usually it is gone because the daemon
        # failed the volume and deleted it. Only with the counters as at
        # the switch, nothing held and no failure line in the serve log is
        # nothing known to have reached the daemon (review of #72).
        elif [ "$wrc" != 0 ] && grep -q 'failed to open' "$OUT/g1-rig-write.err" 2>/dev/null \
            && [[ "$w0$w1$f0$f1" =~ ^[0-9]+$ ]] && [ "$w1" = "$w0" ] && [ "$f1" = "$f0" ] && [ "${held:-}" = 0 ] \
            && ! grep -q 'the volume failed' "$dir.serve.log" 2>/dev/null; then
            pin sync_failure_fails_volume ERROR "serve was still running 60 s after the pool's wait ended, but the rig's own write could not be issued (the device would not open), the counters did not move, nothing was held and the serve logged no failure: nothing that could fail reached the daemon, the premise of g.1 did not happen; $rw"
            G1_PREMISE=missed
        else
            pin sync_failure_fails_volume FAIL "serve was still running 60 s after the pool's wait ended, though a request reached it (the rig's write answered or not issued, with the counters moved, a request held or a failure logged); $rw"
        fi
        kill -TERM "$SERVE_PID" 2>/dev/null; serve_wait 30
    fi
    # The watcher looks until the serve has ended: the daemon's line may
    # come after the pool suspended on its own reads.
    kill "$watch" 2>/dev/null; wait "$watch" 2>/dev/null
    at=$(cat "$WORK/g1.failed-at")
    seen=$(grep -c ' failed at ' "$OUT/$name-reader.log" || true)
    if [ -n "$at" ]; then
        before=$(awk -v t="$at" '/ failed at / && $NF + 0 < t + 0 { c++ } END { print c + 0 }' "$OUT/$name-reader.log")
        before="$before of them before the daemon's line that the volume failed"
    else
        before="the daemon's line that the volume failed was not seen while the rig looked (until the serve ended), so none is placed before it"
    fi
    obs g_reads_while_failing "ZFS's counts for the device (zpool status): $(vdev_say "$OUT/$name-status-suspended.log" "${BDEV#/dev/ublkb}"); the reader (an uncached file read in a loop): $(cat "$WORK/$name.reads" 2>/dev/null || echo '?') reads, $seen failed, $before (the line is looked for every 0.1 s)"
    if ! g_table linear; then
        pin sync_failed_volume_refused ERROR "the linear table could not be put back"
    else
        lim 60 "$SKYZFSD" serve "${SERVE_ARGS[@]}" --dir "$dir" --run-dir "$RUN_DIR" > /dev/null 2> "$OUT/g1-again.err"; rc=$?
        if [ "$G1_PREMISE" = missed ]; then
            pin sync_failed_volume_refused ERROR "g.1's premise did not happen (the rig's write could not be issued), so the volume did not fail: serve exited $rc"
        else
            pin_is sync_failed_volume_refused "with the disk back, serve exited $rc, wanted $EXIT_REFUSED_FAILED: $(tail -n 1 "$OUT/g1-again.err" | cut -c1-300)" [ "$rc" = "$EXIT_REFUSED_FAILED" ]
        fi
    fi
    fail_stop "$name"
}

check_c_all() {
    check_c
    # udev's watch stays off through both passes of the suite.
    c_watch_back
    unflushed_obs c "$(for f in c cd; do
 awk -v p="$f" '$1 == "unflushed" { sub(/^unflushed /, ""); printf "%s/%s; ", p, $0 }' "$OUT/$f-helper.log" 2>/dev/null; done | sed 's|/\([0-9]*\): |\1 |g')"
}
case $CHECKS in *" a "*) course a && check_a; course "a, overlapping writes" && check_a_overlap; course "a, XFS with 16 KiB blocks" && check_a_xfs16k ;; esac
case $CHECKS in *" b "*) course b && check_b ;; esac
case $CHECKS in *" c "*) course c && check_c_all ;; esac
case $CHECKS in *" e "*) course e && check_e ;; esac
# Last: it leaves suspended pools behind.
case $CHECKS in *" d "*)
    course d && check_d
    # A volume that fails while the server holds a request: waits for
    # the author's word on what the failure path does then, and for the
    # control rig's figures on a delete under a held request.
    pin failed_volume_while_a_request_is_held WAITING "not built: waits for what the failure path is to do while a request is held" ;;
esac
# Check g last of all: its courses leave suspended pools, and its file
# system's disk is made to fail. The daemon only.
case $CHECKS in *" g "*)
    if [ "$IMPL" = reference ]; then
        for p in $G_PINS; do pin "$p" WAITING "check g is of the daemon's store; the stand-in has no rule of its own for a failed sync"; done
        ctl g_old_content_after_good_sync WAITING "check g is of the daemon's store"
    else
        course g && check_g
    fi ;;
esac
# ZFS stuck: where, what held it, and the courses not run. The lines of
# those courses are missing, and the judge names why.
if since=$(zfs_stuck_since); then
    rec zfs_stuck "since \"$since\": $(cut -f2- "$ZS_FILE" | head -n 1); courses not run: ${NOT_RUN_COURSES:-none}; every waiting task's kernel stack in zfs-stuck-stacks.txt"
    zs_stacks "the end of the run" >> "$ZS_STACKS" 2>&1
fi

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

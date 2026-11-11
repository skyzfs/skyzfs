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
CHECKS=" ${RIG_CHECKS:-a b c d e} "
# Every word is a check and there is at least one: a run that checks
# nothing must not look like a run.
n=0
for c in $CHECKS; do
    case $c in a|b|c|d|e) n=$((n + 1)) ;; *) echo "RIG_CHECKS: '$c' is not a check; the checks are a b c d e" >&2; exit 64 ;; esac
done
[ $n -gt 0 ] || { echo "RIG_CHECKS names no check" >&2; exit 64; }
KILLS=${RIG_KILLS:-20}
HOLD_S=${RIG_HOLD_S:-300}
# Options for every serve, split at spaces: for the daemon on a runner,
# "--unsafe-allow-unlocked-memory --unsafe-allow-any-filesystem".
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
    local impl=$1 name=$2 kills=$3 dir=$WORK/vol-$2 pool=skzd-$2 mnt slog n i w pid inflight rc st ms t0 watcher live gone looks saw mark states hw klog
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
    lim 20 zpool events -v > "$OUT/$tag-$at-events.txt" 2>&1
    lim 20 zpool status -v "$pool" > "$OUT/$tag-$at-status.txt" 2>&1
    for p in $(pgrep 'txg_sync|txg_quiesce|z_wr_iss|z_wr_int|z_null|spa_async|zed|z_flush'); do
        echo "== $p $(cat "/proc/$p/comm" 2>/dev/null)"; cat "/proc/$p/stack" 2>/dev/null
    done > "$OUT/$tag-$at-stacks.txt"
}
# txgs file -> the newest transaction group and its state (the letters
# are ZFS's: O open, Q quiescing, W waiting for sync, S syncing, C
# committed), and how many of those listed are not committed.
txg_say() {
    awk 'NR > 1 { n++; if ($3 != "C") open++; last = $1 " in state " $3 } END { if (n) printf "newest transaction group %s, %d of the last %d not committed", last, open, n; else printf "transaction groups not read" }' "$1" 2>/dev/null
}
# The course after a refused start. With the daemon only.
#   refused_start_arm TAG LOGDEV    TAG bs: the control's pool, with a
#   separate log device (yes). TAG bn: the same with none (no).
# The server is killed at rest, so that the log's last mutation is an
# acknowledged one and `serve --recover-short 1` must refuse. The
# control judges that answer and nothing of the pool. The rest is
# observation: what the pool, the device and I/O to the pool do in the
# 300 s after, with only a writer's synced writes waiting at first; a
# second writer at 120 s; at 180 s a read of a file that cannot be in
# memory (its dataset caches no data); at 240 s a scrub.
# -> RS_CTL ("FAIL|PASS|ERROR detail" for the control), RS_OBS.
refused_start_arm() {
    local tag=$1 logdev=$2 dir=$WORK/vol-$1 pool=skzd-$1 slog="" pid rc i st w0 w1 held unst quiet="" num t0 dev was line="" fails mark zios
    local n0 n1 live="" end io="" inflight sus="" ev seen did=""
    RS_CTL=""; RS_OBS=""
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
    lim 60 zfs create -o primarycache=metadata "$pool/cold" > "$OUT/$tag-cold.log" 2>&1 \
        && lim 120 "$DD" if=/dev/urandom of="$WORK/mnt/$pool/cold/file" bs=1M count=32 conv=fsync status=none 2>> "$OUT/$tag-cold.log"
    : > "$WORK/$tag.record"; : > "$OUT/$tag-writer.log"
    writer "$WORK/mnt/$pool" "$WORK/$tag.record" "$OUT/$tag-writer.log" "$WORK/$tag.stop" &
    sleep 5
    touch "$WORK/$tag.stop"; wait "$!" 2>/dev/null
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
        if [[ ${i:-} =~ ^[0-9]+$ ]] && [ "$i" = 0 ]; then
            RS_CTL="PASS killed at rest ($quiet); serve --recover-short 1 SERVED ($BDEV) and found recovered_unacknowledged 0: a log shorter than what was acknowledged was let through"
        else
            RS_CTL="ERROR the control did not happen: killed at rest ($quiet), yet serve --recover-short 1 served and says recovered_unacknowledged '${i:-nothing}': the log's last mutation was not an acknowledged one"
        fi
        RS_OBS="the start was not refused, so there is no course after a refusal to report"
        return
    fi
    n0=$(grep -c . "$WORK/$tag.record")
    t0=$(now_ms); was=""
    for i in $(seq 1 300); do
        st=$(pool_state "$pool")
        if [ -e "/dev/ublkb$num" ]; then dev="block node there"; else dev="no block node"; fi
        if lim 20 ublk list -n "$num" 2>/dev/null | grep -q '^dev id'; then dev+=", the driver lists it as '$(ublk_state "/dev/ublkb$num")'"; else dev+=", unknown to the driver"; fi
        case $dev in *LIVE*) live=yes ;; esac
        if [ "$st; $dev" != "$was" ]; then was="$st; $dev"; line+="at $(( ($(now_ms) - t0) / 1000 )) s: pool $st, device $num: $dev. "; fi
        if [ "$st" = SUSPENDED ]; then sus=$(( ($(now_ms) - t0) / 1000 )); break; fi
        case $i in
            10) refused_snap "$pool" "$tag" 10s ;;
            60) refused_snap "$pool" "$tag" 60s ;;
            120) ( t=$(now_ms); lim 170 "$DD" if=/dev/zero of="$WORK/mnt/$pool/second-writer" bs=1M count=4 conv=fsync status=none 2> "$OUT/$tag-writer2.err"; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.writer2" ) &
                 did+="writer2 " ;;
            180) ( t=$(now_ms); lim 110 "$DD" if="$WORK/mnt/$pool/cold/file" of=/dev/null bs=1M status=none 2> "$OUT/$tag-reader.err"; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.reader" ) &
                 did+="reader " ;;
            240) ( t=$(now_ms); lim 50 zpool scrub "$pool" > "$OUT/$tag-scrub.log" 2>&1; echo "exit $? after $(( $(now_ms) - t )) ms" > "$WORK/$tag.scrub" ) &
                 did+="scrub " ;;
        esac
        sleep 1
    done
    end=$(( ($(now_ms) - t0) / 1000 ))
    touch "$WORK/$tag.stop"
    refused_snap "$pool" "$tag" end
    n1=$(grep -c . "$WORK/$tag.record")
    fails=$(grep -c . "$OUT/$tag-writer.log")
    klog_since "$mark" > "$OUT/$tag-klog.txt" 2>/dev/null
    zios=$(grep "$pool" "$OUT/$tag-klog.txt" | grep -o 'type=[0-9]*' | sort | uniq -c | tr -s ' \n' ' ')
    ev=$(grep -o -E '(ereport|sysevent|resource)\.[a-z_.]+' "$OUT/$tag-end-events.txt" 2>/dev/null | sort | uniq -c | tr -s ' \n' ' ')
    seen="zpool status: $(grep -E '^ *(state|status):' "$OUT/$tag-end-status.txt" 2>/dev/null | tr -s ' \n' ' ' | cut -c1-220)$([ -s "$OUT/$tag-end-status.txt" ] || echo 'gave nothing within 20 s'); events since the kill by class: ${ev:-none}; kernel log lines naming ublkb$num or the pool since the kill: $(grep -c -E "ublkb$num|$pool" "$OUT/$tag-klog.txt")"
    case $did in *writer2*) io+="a second writer (4 MiB, synced) at 120 s: $(cat "$WORK/$tag.writer2" 2>/dev/null || echo "had not ended at $end s (hung)"); " ;; esac
    case $did in *reader*) io+="a read of the uncached file at 180 s: $(cat "$WORK/$tag.reader" 2>/dev/null || echo "had not ended at $end s (hung)") $(head -c 120 "$OUT/$tag-reader.err" 2>/dev/null | tr '\n' ' '); " ;; esac
    case $did in *scrub*) io+="a scrub at 240 s: $(cat "$WORK/$tag.scrub" 2>/dev/null || echo "had not ended at $end s (hung)") $(head -c 120 "$OUT/$tag-scrub.log" 2>/dev/null | tr '\n' ' '); " ;; esac
    RS_OBS="pool $([ "$logdev" = yes ] && echo 'with a separate log device' || echo 'WITHOUT a separate log device'). Before the refused start, 2 s after a writer began with no server: $inflight. After the refusal (exit $SERVE_RC): ${line}$([ -n "$sus" ] && echo "SUSPENDED at $sus s. " || echo "Never suspended in $end s. ")At the end, $end s: pool $st, the device: $dev. Transaction groups at 10 s: $(txg_say "$OUT/$tag-10s-txgs.txt"); at 60 s: $(txg_say "$OUT/$tag-60s-txgs.txt"); at the end: $(txg_say "$OUT/$tag-end-txgs.txt"). The first writer, which writes 4 MiB files and syncs each: $fails failed writes, $n0 files acknowledged at the refusal and $n1 at the end$([ "$n1" -gt "$n0" ] && echo ' (ACKNOWLEDGED BY THE POOL WITH NO SERVER BEHIND ITS DEVICE)'). Started while it lasted: ${io:-nothing (the pool suspended first)}. Failed I/O of the pool in the kernel log since the kill, by zio type (1 read, 2 write, 5 flush): ${zios:-none}. What an operator would see at the end: $seen. Stacks of ZFS's threads, events and status at 10 s, 60 s and the end are in the files $tag-*-stacks.txt, -events.txt, -status.txt"
    if [ "$SERVE_RC" = "$EXIT_REFUSED_FAILED" ] && ! grep -q '^ready' "$dir.ready" && [ -z "$live" ]; then
        RS_CTL="FAIL killed at rest ($quiet); serve --recover-short 1 exited $SERVE_RC and served nothing: no ready line, the device was live at no look in $end s; the pool at the end: $st (reported, not judged; see obs_b_refused_start_under_writes)"
    else
        RS_CTL="PASS killed at rest ($quiet); serve --recover-short 1 exited ${SERVE_RC:-?}; ready line: $(grep -c '^ready' "$dir.ready"); the device live at a look: ${live:-no}; pool $st"
    fi
    # The transaction groups and the txg_sync stacks at the end, in the
    # job's log as well: an artifact can be lost.
    echo "-- $tag: the transaction groups and the txg_sync threads at the end:"; tail -n 4 "$OUT/$tag-end-txgs.txt" 2>/dev/null
    awk '/^== [0-9]+ txg_sync/ { p = 1; print; next } /^== / { p = 0 } p' "$OUT/$tag-end-stacks.txt" 2>/dev/null | head -n 60
}
# With the daemon: the short-recovery control and what follows a refused
# start, on a pool with a separate log device and on one without.
check_b_short() {
    refused_start_arm bs yes
    ctl recovered_log_too_short "${RS_CTL%% *}" "${RS_CTL#* }"
    obs b_refused_start_under_writes "${RS_OBS:-nothing to report: the control ended before a refusal ($RS_CTL)}"
    refused_start_arm bn no
    obs b_refused_start_no_log_device "${RS_OBS:-nothing to report: this arm ended before a refusal}; the daemon's answer in this arm: $RS_CTL"
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
    local dir=$WORK/vol-b7 pool=skzd-b7 slog w rc i w0 w1 rec st files fails a
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
        touch "$WORK/b7.stop"
        pin orderly_stop_under_load FAIL "serve did not exit within 60 s of SIGTERM under load"
        kill -9 "$SERVE_PID" 2>/dev/null; serve_wait 30; serve_start "$SKYZFSD" "$dir"; wait "$w" 2>/dev/null; kill_cleanup b7
        return
    fi
    rc=$SERVE_RC
    serve_start "$SKYZFSD" "$dir"; i=$?
    rec=$(vol_stat "$SKYZFSD" "$dir" recovered_unacknowledged)
    sleep 5
    touch "$WORK/b7.stop"; wait "$w" 2>/dev/null
    st=$(pool_state "$pool")
    fails=$(grep -c . "$OUT/b7-writer.log")
    files=$(lim 120 files_match "$WORK/mnt/$pool" "$WORK/b7.record" 2>&1 | tail -n 1)
    a="the server had counted ${w0:-?} then ${w1:-?} writes just before SIGTERM; serve exit $rc; serve again rc $i, recovered_unacknowledged '${rec:-not said}'; the writer's failed writes: $fails; pool $st, $(pool_errors "$pool"); files: $files"
    if ! { [[ ${w0:-} =~ ^[0-9]+$ ]] && [[ ${w1:-} =~ ^[0-9]+$ ]] && [ "$w1" -gt "$w0" ]; }; then
        pin orderly_stop_under_load ERROR "the stop did not find the writer at work: $a"
    elif [ "$rc" = 0 ] && [ $i = 0 ] && [ "$rec" = 0 ] && [ "$fails" = 0 ] && [ "$st" = ONLINE ] && [[ $files == ok\ * ]]; then
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
    kill "$holder" 2>/dev/null; wait "$holder" 2>/dev/null
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
    for v in takeover_time_does_not_grow log_stays_below_high_mark kills_inside_steps_lose_nothing; do
        pin "$v" WAITING "the daemon does not checkpoint yet (M1.3c); see obs_b_takeover_ms"
    done
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
            rc=$SERVE_RC; st=$(ublk_state "$BDEV")
            ( lim 120 "$DD" if=/dev/zero of="$WORK/mnt/$pool/across-stop" bs=1M count=4 conv=fsync status=none; echo $? > "$WORK/b-across.rc" ) &
            across=$!
            sleep 3
            serve_start "$SKYZFSD" "$dir"; i=$?
            # Not a bare wait: the new serve is a background job too.
            wait "$across"
            if [ "$rc" = 0 ] && [ -b "$BDEV" ] && [ "$st" = QUIESCED ] && [ $i = 0 ] && [ "$(cat "$WORK/b-across.rc")" = 0 ] \
                && [ "$(pool_state "$pool")" = ONLINE ]; then
                pin orderly_stop_keeps_device PASS "serve exited 0 on SIGTERM, the device stayed ($st), and a write begun while no server ran completed after serve returned; $(pool_errors "$pool")"
            else
                pin orderly_stop_keeps_device FAIL "serve exit $rc on SIGTERM, device state '$st', serve again rc $i, write across the stop rc $(cat "$WORK/b-across.rc"), pool $(pool_state "$pool")"
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
        rec c_run "ERROR: skyzfs-blkcheck exit $rc after $n cases: $(tail -n 1 "$OUT/c-blkcheck.err")"
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
        pin c_delay_cases ERROR "skyzfs-blkcheck exit $rc with the delay: $(tail -n 1 "$OUT/cd-blkcheck.err")"
        pin c_kill_with_applied_unacknowledged ERROR "the pass with the delay did not end"
        return
    fi
    case ${found:-?} in
        '?') pin c_kill_with_applied_unacknowledged ERROR "a take-over did not say what it found (status key recovered_unacknowledged) at ${asked:-0} take-overs" ;;
        0) pin c_kill_with_applied_unacknowledged ERROR "no server found a write applied and not acknowledged over ${asked:-0} take-overs: the delay did nothing, and the kill cases did not see a write applied twice" ;;
        *) pin c_kill_with_applied_unacknowledged PASS "$found requests were applied and not acknowledged at a server's death and issued again, over ${asked} take-overs" ;;
    esac
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

check_e() {
    local v
    step "e: one request held $HOLD_S s"
    hold_test e "$HOLD_S" "$HOLD_S"; v=$?
    case $v in 0) pin held_request_completes PASS "$HT_WHY" ;; 1) pin held_request_completes FAIL "$HT_WHY" ;; *) pin held_request_completes ERROR "$HT_ERR" ;; esac
    step "e, control: a request held 5 s is not one held $HOLD_S s"
    hold_test e0 5 "$HOLD_S"; v=$?
    case $v in 0) ctl hold_too_short PASS "$HT_WHY" ;; 1) ctl hold_too_short FAIL "$HT_WHY" ;; *) ctl hold_too_short ERROR "$HT_ERR" ;; esac
}

case $CHECKS in *" a "*) check_a; check_a_overlap ;; esac
case $CHECKS in *" b "*) check_b ;; esac
case $CHECKS in *" c "*)
    check_c
    # A third pass of the suite with the daemon's marks set low, so
    # that it checkpoints while the suite runs: waits for M1.3c.
    pin c_suite_with_low_marks WAITING "the daemon does not checkpoint yet (M1.3c)" ;;
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
    else
        check_b_short
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

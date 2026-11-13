#!/usr/bin/env bash
# Power-cut replay rig (design §9, M1): record every write a pool makes,
# then check that the pool imports, and holds what it should, at many
# prefixes of that record. This script records and then runs the check
# (replay_check.py, which documents the checks and the negative
# controls).
#
#   replay-rig.sh OUT [WORK]
#
# The stack while recording:
#
#   ZFS pool ── data vdev ── dm-log-writes ── device under test
#            └─ log device (a local file, written through beforehand)
#
# Two steps are replaceable, for skyzfsd:
#   - the device under test. By default the pinned reference ublk server
#     serves a file. RIG_DEVICE=/dev/... uses a device that is already
#     up instead, and this script then leaves it alone.
#   - "the image at prefix N". By default logwrites.py replays the
#     dm-log-writes log. RIG_PROVIDER names another provider command,
#     and RIG_LOG the log it reads (see logwrites.py for the contract).
#
# Workload: a writer creates files that are never changed afterwards
# (each has its own name and is written once, so what a file held when
# its fsync returned is what it holds for good),
# each with an fsync, and records each file's checksum when it is
# complete. Meanwhile the pool is synced RIG_SYNCS times, and after each
# sync a mark goes into the log with the number of files that were
# complete before that sync began. Those files must be in the pool at
# every prefix from the mark on. A mark also follows each file's fsync:
# where the intent log is on the device under test (RIG_LAYOUT=whole),
# the file must survive from there on, once the intent log is replayed.
#
# RIG_LAYOUT=slog (default) is the product's layout: the log device is
# local and is not recorded. RIG_LAYOUT=whole has no separate log
# device: the intent log and its flushes land on the device under test,
# and the image is the whole pool, which zdb needs.
# RIG_AUTOTRIM=on sets the pool's autotrim property, as the product
# will, so that discards reach the device under test and the log.
# RIG_RECORD_ONLY=1 stops after recording and leaves base.img, log.img
# and the manifest in WORK.
#
# Knobs (environment): RIG_SYNCS=40, RIG_FILE_KB=256, RIG_FILE_SLEEP=0.05
# (seconds between files), RIG_SYNC_SLEEP=0.6 (seconds between syncs),
# RIG_BUDGET_S=840, RIG_DEEP_BUDGET_S=240 and RIG_COPIES_BUDGET_S=300
# (the check's main pass), RIG_DISCARD=zero|skip (what a discarded range
# holds afterwards on the image),
# RIG_REPLAY_LOG (path of xfstests replay-log, a witness for the
# provider; skipped when unset, which RIG_REQUIRE_WITNESS=1 makes a
# failure).
#
# Needs root, ZFS with zdb, dm-log-writes, dm-snapshot, and `ublk` unless
# RIG_DEVICE is set. For a throwaway machine: it creates a pool named
# skzreplay.
#
# Through skyzfsd (RIG_SKYZFSD names a build with its test hooks): the
# cut falls below the daemon. The stack while recording is
#
#   ZFS pool ── /dev/ublkbN ── skyzfsd serve ── volume dir (xfs)
#            ── dm-log-writes ── lower disk (a loop file)
#
# and the pool is in the whole-pool layout, so the intent log is on the
# device under test. The base image is the lower disk after the volume
# was made, served, the pool created and exported, and the daemon
# stopped. The marks go into the lower disk's log. The check is
# replay_daemon.py, which makes the pool's device at each prefix by
# mounting the lower disk and starting the daemon as after a reboot.
# Knobs of this mode: RIG_VOLUME_MB=1024, RIG_FILL_MB=160 (a file
# written at the start of the workload and synced by the first sync, so
# that the log passes the daemon's high mark and checkpoint steps fall
# inside the recording), RIG_SERVE_ARGS (options for every serve,
# split at spaces), RIG_BETWEEN_BUDGET_S=0, RIG_BUDGET_NAME,
# RIG_EVERY_PREFIX=1 (every record prefix, no time limit). A
# recording in which the daemon took no checkpoint step is the rig's
# error (exit 2): it would say nothing about a cut inside a step, and
# so is a fill that does not fit in the pool. With
# the in-place store (the default, M1.3t) there is no log and so no
# step, and none is asked for. Every command on the pool while recording runs
# under a time limit (pool_step); a daemon that stops answering is then
# a verdict, not a job that hangs.
set -euo pipefail

# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

OUT=${1:?usage: replay-rig.sh OUT [WORK]}
WORK=${2:-/mnt/skz-replay}
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)

POOL=skzreplay
MOUNTPOINT=/skzreplay
LW=skz-lw
UBLK_ID=3
VDEV_MB=2048
SLOG_MB=256
LOG_MB=8192
SYNCS=${RIG_SYNCS:-40}
FILE_KB=${RIG_FILE_KB:-256}
# A file starts with a 4 KiB block that names it; the rest is random.
if ! [[ $FILE_KB =~ ^[0-9]+$ ]] || [ "$FILE_KB" -lt 4 ]; then
    echo "RIG_FILE_KB must be a number, at least 4: $FILE_KB" >&2; exit 64
fi
FILE_SLEEP=${RIG_FILE_SLEEP:-0.05}
SYNC_SLEEP=${RIG_SYNC_SLEEP:-0.6}
DEVICE=${RIG_DEVICE:-}
SKYZFSD=${RIG_SKYZFSD:-}
LAYOUT=${RIG_LAYOUT:-slog}
if [ -n "$SKYZFSD" ]; then
    # Through the daemon there is one device, and it is under test.
    [ -z "${RIG_LAYOUT:-}" ] || [ "$LAYOUT" = whole ] || { echo "through skyzfsd the layout is whole" >&2; exit 64; }
    [ -z "$DEVICE" ] || { echo "RIG_DEVICE and RIG_SKYZFSD exclude each other" >&2; exit 64; }
    [ -x "$SKYZFSD" ] || { echo "RIG_SKYZFSD=$SKYZFSD is not an executable" >&2; exit 64; }
    LAYOUT=whole
fi
case $LAYOUT in slog|whole) ;; *) echo "RIG_LAYOUT must be slog or whole" >&2; exit 64 ;; esac
VOLUME_MB=${RIG_VOLUME_MB:-1024}
LOWER_MB=$((VOLUME_MB * 3))
FILL_MB=${RIG_FILL_MB:-160}
for v in "$VOLUME_MB" "$FILL_MB"; do [[ $v =~ ^[0-9]+$ ]] || { echo "not a number: $v" >&2; exit 64; }; done
read -r -a SERVE_ARGS <<< "${RIG_SERVE_ARGS:-}"
# `--store in-place` (M1.3r): the daemon keeps no log and takes no
# checkpoint step, so a recording without one is what it should be.
# The store is always named: the rig never rests on the daemon's
# default, which before M1.3t is the log. Without --store in
# RIG_SERVE_ARGS the rig names the in-place store; --store log asks for
# the log store of a build from before M1.3t.
IN_PLACE=""; STORE_NAMED=""
for i in "${!SERVE_ARGS[@]}"; do
    case ${SERVE_ARGS[$i]} in
        --store=*) STORE_NAMED=1; [ "${SERVE_ARGS[$i]}" != --store=in-place ] || IN_PLACE=1 ;;
        --store) STORE_NAMED=1; [ "${SERVE_ARGS[$((i + 1))]:-}" != in-place ] || IN_PLACE=1 ;;
    esac
done
[ -n "$STORE_NAMED" ] || { SERVE_ARGS=(--store in-place "${SERVE_ARGS[@]}"); IN_PLACE=1; }
LOWER_MNT=$WORK/lower-mnt
VOL=$LOWER_MNT/vol
REC_RUN=/run/skz-replay-rec
MANIFEST=$WORK/manifest
STOP=$WORK/stop
SCRATCH=/dev/shm/skz-replay.$$

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
SLOG_LOOP=""; LOG_LOOP=""; UBLK_UP=""; WRITER=""
LOWER_LOOP=""; LOWER_MOUNTED=""; SERVE_PID=""; PHASE=setup; VERDICT=""
# Through the daemon, an error before the check is the rig's own (exit
# 2), and the summary says where it happened.
rig_error() {
    { echo "## Power-cut replay through skyzfsd: the rig's own error, not a verdict"
      echo
      echo "In the $PHASE phase: $*"
      [ ! -s "$OUT/serve.log" ] || { echo; echo "The daemon's last lines:"; echo; echo '```'; tail -n 15 "$OUT/serve.log"; echo '```'; }
    } > "$OUT/replay-summary.md"
    echo "$*" >&2
    exit 2
}
# The daemon failed at something it is judged on (its start, its stop,
# its detach, the pool's writes through it) before the check: a verdict
# against the daemon (exit 1), as the same events are in the check.
daemon_verdict() {
    { echo "## Power-cut replay through skyzfsd: a verdict against the daemon"
      echo
      echo "In the $PHASE phase: $*"
      [ ! -s "$OUT/serve.log" ] || { echo; echo "The daemon's last lines:"; echo; echo '```'; tail -n 15 "$OUT/serve.log"; echo '```'; }
    } > "$OUT/replay-summary.md"
    echo "$*" >&2
    VERDICT=1
    exit 1
}
# Start `serve` on the volume and wait for its ready line: DEVICE.
serve_up() {
    : > "$WORK/serve.ready"
    "$SKYZFSD" serve --dir "$VOL" --run-dir "$REC_RUN" "${SERVE_ARGS[@]}" > "$WORK/serve.ready" 2>> "$OUT/serve.log" &
    SERVE_PID=$!
    local i st
    for i in $(seq 1 480); do
        DEVICE=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$WORK/serve.ready" | head -n 1)
        [ -n "$DEVICE" ] && [ -b "$DEVICE" ] && return 0
        if ! kill -0 "$SERVE_PID" 2>/dev/null; then
            st=0
            wait "$SERVE_PID" || st=$?
            SERVE_PID=""
            daemon_verdict "skyzfsd serve exited with status $st before it was ready"
        fi
        sleep 0.25
    done
    daemon_verdict "skyzfsd serve was not ready after 120 s"
}
# pool_step SECONDS WHAT COMMAND...: a command on the pool. Through the
# daemon it runs under a time limit that holds when the command cannot
# be killed (lim), since a daemon that stops answering leaves a command
# on its device asleep in the kernel; a failure or the limit is then a
# verdict. On the reference server it runs as it always did.
pool_step() {
    local secs=$1 what=$2 rc=0
    shift 2
    if [ -z "$SKYZFSD" ]; then "$@"; return; fi
    lim "$secs" "$@" || rc=$?
    [ "$rc" = 0 ] && return 0
    # The command could not start (127), or lim could not (125): the rig's.
    [ "$rc" != 127 ] && [ "$rc" != 125 ] || rig_error "$what could not be started (status $rc)"
    [ "$rc" != 124 ] || daemon_verdict "$what had not ended after $secs s (left behind), and the pool's device is the daemon's"
    daemon_verdict "$what failed with status $rc, and the pool's device is the daemon's"
}
# SIGTERM, then the device is deleted: an orderly stop and a detach.
serve_down() {
    local i rc=0
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    for i in $(seq 1 240); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
    if kill -0 "$SERVE_PID" 2>/dev/null; then kill -9 "$SERVE_PID"; SERVE_PID=""; daemon_verdict "skyzfsd serve had not ended 60 s after SIGTERM"; fi
    wait "$SERVE_PID" || rc=$?
    SERVE_PID=""
    [ "$rc" = 0 ] || daemon_verdict "skyzfsd serve ended with status $rc after SIGTERM"
    timeout 120 "$SKYZFSD" detach --dir "$VOL" --run-dir "$REC_RUN" >&2 || daemon_verdict "skyzfsd detach failed"
}
# The writer is told to stop and waited for, 300 s at most: a write to
# a pool whose device does not answer never returns, and a bare `wait`
# would then hold the job to its limit with nothing said.
#   0                      the writer ended by itself, with status 0
#   its own status         it ended by itself and failed (a write, an
#                          fsync or a mark): the manifest is incomplete
#   125, WRITER_KILLED=1   it had not ended at the limit and was killed
#                          (its dd may stay behind; the machine is thrown
#                          away)
# WRITER_KILLED tells the last two apart, whatever status the writer had.
WRITER_LIMIT_TICKS=${WRITER_LIMIT_TICKS:-1200}   # of 0.25 s
writer_end() {
    local i=0 rc=0
    WRITER_KILLED=""
    touch "$STOP"
    while kill -0 "$WRITER" 2>/dev/null && [ "$i" -lt "$WRITER_LIMIT_TICKS" ]; do sleep 0.25; i=$((i + 1)); done
    if kill -0 "$WRITER" 2>/dev/null; then
        kill -9 "$WRITER" 2>/dev/null || true
        WRITER=""; WRITER_KILLED=1
        return 125
    fi
    wait "$WRITER" 2>/dev/null || rc=$?
    WRITER=""
    return "$rc"
}
cleanup() {
    rc=$?
    set +e
    if [ -n "$WRITER" ]; then
        writer_end; wrc=$?
        if [ -n "$WRITER_KILLED" ]; then log "the writer had not ended 300 s after it was told to; killed"
        elif [ "$wrc" -ne 0 ]; then log "the writer ended with status $wrc"; fi
    fi
    # lim and not timeout(1): after a daemon that stopped answering, these
    # can sleep in the kernel for ever, and timeout waits for its child.
    lim 120 zpool export -f "$POOL" 2>/dev/null
    if [ -n "$SERVE_PID" ]; then kill -9 "$SERVE_PID" 2>/dev/null; lim 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$REC_RUN" >/dev/null 2>&1; fi
    [ -z "$LOWER_MOUNTED" ] || lim 120 umount "$LOWER_MNT" 2>/dev/null
    dmsetup remove "$LW" 2>/dev/null
    [ -z "$LOWER_LOOP" ] || losetup -d "$LOWER_LOOP" 2>/dev/null
    [ -n "$UBLK_UP" ] && lim 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1
    for l in "$SLOG_LOOP" "$LOG_LOOP"; do [ -n "$l" ] && losetup -d "$l" 2>/dev/null; done
    [ -d "$SCRATCH" ] && find "$SCRATCH" -maxdepth 1 -type f -delete && rmdir "$SCRATCH"
    if [ "$rc" -ne 0 ] && [ -n "$SKYZFSD" ] && [ "$PHASE" != check ] && [ "$rc" != 2 ] && [ "$rc" != 64 ] && [ -z "$VERDICT" ]; then
        # set -e ended a step of the rig's own before the check.
        { echo "## Power-cut replay through skyzfsd: the rig's own error, not a verdict"; echo
          echo "A command of the rig failed in the $PHASE phase (status $rc); see the job's log."; } > "$OUT/replay-summary.md"
        rc=2
    fi
    if [ "$rc" -ne 0 ]; then
        log "replay rig FAILED (rc=$rc); recent kernel messages:"
        dmesg 2>/dev/null | grep -iE 'ublk|log-writes|device-mapper|zfs|zio|loop[0-9]|error' | tail -n 30 >&2
    fi
    exit "$rc"
}
trap cleanup EXIT

for m in ublk_drv dm_log_writes dm_snapshot zfs; do modprobe "$m" 2>/dev/null || true; done
[ -r /proc/spl/kstat/zfs/arcstats ] || { echo "the zfs kernel module is not loaded" >&2; exit 64; }
dmsetup targets | grep -q '^log-writes' || { echo "no dm-log-writes target in this kernel" >&2; exit 64; }
dmsetup targets | grep -q '^snapshot ' || { echo "no dm-snapshot target in this kernel" >&2; exit 64; }
timeout 60 zpool export -f "$POOL" 2>/dev/null || true   # a previous run's leftover
dmsetup remove "$LW" 2>/dev/null || true

# ---------------------------------------------------- device under test
if [ -n "$SKYZFSD" ]; then
    command -v ublk >/dev/null || { echo "no ublk command: the check deletes the devices of its starts with it (build-ublksrv.sh)" >&2; exit 64; }
    command -v mkfs.xfs >/dev/null || { echo "no mkfs.xfs" >&2; exit 64; }
    rm -rf "$REC_RUN"
    : > "$WORK/lower.img"
    truncate -s "${LOWER_MB}M" "$WORK/lower.img"
    LOWER_LOOP=$(losetup --find --show --direct-io=on "$WORK/lower.img" 2>/dev/null || losetup --find --show "$WORK/lower.img")
    mkfs.xfs -q -f "$LOWER_LOOP"
    mkdir -p "$LOWER_MNT"
    mount "$LOWER_LOOP" "$LOWER_MNT"; LOWER_MOUNTED=1
    "$SKYZFSD" create --dir "$VOL" --size $((VOLUME_MB << 20)) >&2 || rig_error "skyzfsd create failed"
    serve_up
    log "device under test: $DEVICE, skyzfsd over $VOL (xfs on a loop file); serve options: ${SERVE_ARGS[*]:-none}"
elif [ -z "$DEVICE" ]; then
    command -v ublk >/dev/null || { echo "no ublk command: build the reference server (build-ublksrv.sh) or set RIG_DEVICE" >&2; exit 64; }
    [ -e /dev/ublk-control ] || { echo "no /dev/ublk-control: ublk_drv is not loaded" >&2; exit 64; }
    timeout 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1 || true
    : > "$WORK/vdev.img"
    truncate -s "${VDEV_MB}M" "$WORK/vdev.img"
    timeout 60 ublk add -t loop -n "$UBLK_ID" -f "$WORK/vdev.img" >&2
    UBLK_UP=1
    DEVICE=/dev/ublkb$UBLK_ID
    for _ in $(seq 1 40); do [ -b "$DEVICE" ] && break; sleep 0.5; done
    [ -b "$DEVICE" ] || { echo "ublk add returned, but $DEVICE did not appear" >&2; exit 1; }
    log "device under test: $DEVICE, the reference ublk server over a file"
else
    [ -b "$DEVICE" ] || { echo "RIG_DEVICE=$DEVICE is not a block device" >&2; exit 64; }
    log "device under test: $DEVICE, as given"
fi

# The log device is allocated storage: a file written through once.
LOGVDEV=(); LOGDIR=()
if [ "$LAYOUT" = slog ]; then
    : > "$WORK/slog.img"
    "$DD" if=/dev/zero of="$WORK/slog.img" bs=1M count="$SLOG_MB" oflag=direct conv=fsync status=none
    SLOG_LOOP=$(losetup --find --show --direct-io=on "$WORK/slog.img" 2>/dev/null || losetup --find --show "$WORK/slog.img")
    LOGVDEV=(log "$SLOG_LOOP"); LOGDIR=(-d "$SLOG_LOOP")
fi

# ------------------------------------------------- the pool, then the base
# Created on the bare device, before any recording: the pool's labels
# are then where they are in production, and the record starts from an
# exported pool, which is the base image every prefix is built on.
pool_step 300 "zpool create" zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$MOUNTPOINT" -O atime=off \
    "$POOL" "$DEVICE" "${LOGVDEV[@]}"
[ ! -b "${DEVICE}p1" ] || { echo "zpool create partitioned $DEVICE: the labels are not at byte 0" >&2; exit 1; }
pool_step 300 "the export after zpool create" zpool export "$POOL"
# The log device as it is now, for tools that cannot open the pool
# without one (zdb): it matches the base image and nothing later.
[ -z "$SLOG_LOOP" ] || cp --sparse=always "$WORK/slog.img" "$WORK/slog-base.img"
if [ -n "$SKYZFSD" ]; then
    # Through the daemon the base is the lower disk, the daemon stopped.
    serve_down
    umount "$LOWER_MNT"; LOWER_MOUNTED=""
    losetup -d "$LOWER_LOOP"; LOWER_LOOP=""
    log "copying the lower disk as the base image"
    cp --sparse=always "$WORK/lower.img" "$WORK/base.img"
else
    log "copying the device as the base image"
    "$DD" if="$DEVICE" of="$WORK/base.img" bs=1M iflag=direct conv=sparse status=none
    truncate -s "$(blockdev --getsize64 "$DEVICE")" "$WORK/base.img"
fi

# ------------------------------------------------------------ recording
: > "$WORK/log.img"
truncate -s "${LOG_MB}M" "$WORK/log.img"
LOG_LOOP=$(losetup --find --show --direct-io=on "$WORK/log.img" 2>/dev/null || losetup --find --show "$WORK/log.img")
PHASE=recording
if [ -n "$SKYZFSD" ]; then
    # dm-log-writes under the volume's file system; the daemon on top.
    LOWER_LOOP=$(losetup --find --show --direct-io=on "$WORK/lower.img" 2>/dev/null || losetup --find --show "$WORK/lower.img")
    dmsetup create "$LW" --table "0 $(blockdev --getsz "$LOWER_LOOP") log-writes $LOWER_LOOP $LOG_LOOP"
    mount "/dev/mapper/$LW" "$LOWER_MNT"; LOWER_MOUNTED=1
    serve_up
    log "recording below the daemon: $DEVICE"
    pool_step 300 "the import for recording" zpool import -d "$DEVICE" -o cachefile=none "$POOL"
else
    dmsetup create "$LW" --table "0 $(blockdev --getsz "$DEVICE") log-writes $DEVICE $LOG_LOOP"
    zpool import -d "/dev/mapper/$LW" "${LOGDIR[@]}" -o cachefile=none "$POOL"
fi
[ -z "${RIG_AUTOTRIM:-}" ] || zpool set autotrim="$RIG_AUTOTRIM" "$POOL"
zpool status -P "$POOL" >&2

mkdir -p "$SCRATCH"
: > "$MANIFEST"
[ ! -e "$STOP" ] || unlink "$STOP"
# Its status says which step failed: WRITER_POOL_RC for the write and
# fsync of a file on the pool (the daemon's device in the daemon mode),
# WRITER_RIG_RC for its own steps (the scratch file, the mark).
WRITER_POOL_RC=90; WRITER_RIG_RC=91
writer() {
    local n=0 sum
    while [ ! -e "$STOP" ]; do
        # The first 4 KiB name the file (256 lines of 16 bytes), so that
        # its first block can be found in the write log; the rest is
        # random.
        {
            printf "SKZ-FILE-$(printf %06d "$n")\\n%.0s" {1..256}
            head -c $((FILE_KB * 1024 - 4096)) /dev/urandom
        } > "$SCRATCH/f" || exit "$WRITER_RIG_RC"
        sum=$(sha256sum < "$SCRATCH/f" | cut -d' ' -f1) || exit "$WRITER_RIG_RC"
        "$DD" if="$SCRATCH/f" of="$MOUNTPOINT/f.$n" bs=1M conv=fsync status=none || exit "$WRITER_POOL_RC"
        # The fsync has returned: from here on in the log, the file is safe.
        dmsetup message "$LW" 0 mark "fsync-$n" || exit "$WRITER_RIG_RC"
        # Listed only once complete and fsynced: a line here is a promise.
        echo "$sum f.$n" >> "$MANIFEST" || exit "$WRITER_RIG_RC"
        n=$((n + 1))
        sleep "$FILE_SLEEP"
    done
}
if [ -n "$SKYZFSD" ] && [ "$FILL_MB" -gt 0 ]; then
    # Not in the manifest: it is there to fill the daemon's log, so that
    # checkpoint steps run while the files are written.
    # A fill that cannot fit is the rig's setting, not the daemon's failure:
    # refused before it is written. 64 MiB are kept for the files.
    avail=$(zfs get -Hp -o value available "$POOL")
    [[ $avail =~ ^[0-9]+$ ]] || rig_error "zfs get available gave '$avail'"
    [ $(((FILL_MB + 64) << 20)) -le "$avail" ] \
        || rig_error "RIG_FILL_MB=$FILL_MB does not fit in the pool on RIG_VOLUME_MB=$VOLUME_MB: $((avail >> 20)) MiB available, and 64 MiB are kept for the files"
    frc=0
    lim 600 "$DD" if=/dev/urandom of="$MOUNTPOINT/fill" bs=1M count="$FILL_MB" status=none 2> "$WORK/fill.err" || frc=$?
    if [ "$frc" != 0 ]; then
        cat "$WORK/fill.err" >&2
        # Out of space all the same: the sizes, so the rig's.
        ! grep -q 'No space left' "$WORK/fill.err" || rig_error "the fill file ran out of space in the pool: $(head -c 200 "$WORK/fill.err")"
        [ "$frc" != 124 ] || daemon_verdict "the fill file had not been written after 600 s (left behind), and the pool's device is the daemon's"
        daemon_verdict "the fill file could not be written to the pool (status $frc), and the pool's device is the daemon's"
    fi
fi
writer &
WRITER=$!
log "workload: $SYNCS syncs, files of $FILE_KB KiB"
for k in $(seq 1 "$SYNCS"); do
    sleep "$SYNC_SLEEP"
    c=$(wc -l < "$MANIFEST")
    pool_step 300 "zpool sync $k" zpool sync "$POOL"
    # One word: the target takes exactly one argument after "mark".
    dmsetup message "$LW" 0 mark "sync-$k-$c"
done
wrc=0; writer_end || wrc=$?
if [ -n "$WRITER_KILLED" ]; then
    why="the writer had not ended 300 s after it was told to stop (pool $(cat "/proc/spl/kstat/zfs/$POOL/state" 2>/dev/null || echo unknown)): the workload did not finish, nothing is checked"
    [ -z "$SKYZFSD" ] || daemon_verdict "$why; the pool's device is the daemon's"
    echo "$why" >&2
    exit 1
fi
# As the bare `wait` under errexit did before the limit: a writer whose
# write, fsync or mark failed leaves a manifest that promises less than
# was done, and nothing is judged on it.
if [ "$wrc" -ne 0 ]; then
    why="the recording writer failed with status $wrc (a write, an fsync or a mark): the manifest is incomplete, nothing is checked"
    if [ -n "$SKYZFSD" ] && [ "$wrc" = "$WRITER_POOL_RC" ]; then
        daemon_verdict "$why: the write or fsync of a file on the pool failed, and the pool's device is the daemon's"
    fi
    echo "$why" >&2
    exit "$wrc"
fi
c=$(wc -l < "$MANIFEST")
pool_step 300 "the last zpool sync" zpool sync "$POOL"
dmsetup message "$LW" 0 mark "sync-$((SYNCS + 1))-$c"
pool_step 300 "the export after recording" zpool export "$POOL"
log "recorded: $c files, log-writes status: $(dmsetup status "$LW")"
STEPS=""
if [ -n "$SKYZFSD" ]; then
    timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$REC_RUN" > "$OUT/status-after-recording.txt" 2>&1 || true
    STEPS=$(awk '$1 == "checkpoint_steps" { print $2; exit }' "$OUT/status-after-recording.txt")
    log "checkpoint steps while recording: ${STEPS:-not said}"
    serve_down
    umount "$LOWER_MNT"; LOWER_MOUNTED=""
fi
for _ in $(seq 1 20); do dmsetup remove "$LW" 2>/dev/null && break; sleep 0.25; done
! dmsetup info "$LW" >/dev/null 2>&1 || { echo "could not remove $LW" >&2; exit 1; }
losetup -d "$LOG_LOOP"; LOG_LOOP=""
[ -z "$LOWER_LOOP" ] || losetup -d "$LOWER_LOOP"; LOWER_LOOP=""
[ -z "$SLOG_LOOP" ] || losetup -d "$SLOG_LOOP"; SLOG_LOOP=""
if [ -n "$UBLK_UP" ]; then timeout 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1 || true; UBLK_UP=""; fi
log "log: $(du -m "$WORK/log.img" | cut -f1) MiB on disk"

if [ -n "${RIG_RECORD_ONLY:-}" ]; then log "recorded only, as asked: $WORK"; exit 0; fi
if [ -n "$SKYZFSD" ]; then
    [[ ${STEPS:-} =~ ^[0-9]+$ ]] || rig_error "the daemon's status after recording gave no checkpoint_steps: $(head -c 300 "$OUT/status-after-recording.txt")"
    if [ -n "$IN_PLACE" ]; then
        # No log, so no step: there is no cut inside one to look for.
        [ "$STEPS" = 0 ] || rig_error "the in-place store reports $STEPS checkpoint steps, and it has no log"
        STEPS="none: the in-place store has no log"
    else
        [ "$STEPS" -gt 0 ] || rig_error "the daemon took no checkpoint step while recording (RIG_FILL_MB=$FILL_MB, serve options: ${SERVE_ARGS[*]:-none}): a run without one says nothing about a cut inside a step"
    fi
    PHASE=check
    args=(--log "$WORK/log.img" --base "$WORK/base.img" --manifest "$MANIFEST" --pool "$POOL" --mountpoint "$MOUNTPOINT"
          --work "$WORK/check" --out "$OUT" --skyzfsd "$SKYZFSD" --ublk "$(command -v ublk)" --dd "$DD" --volume vol
          --dev-bytes $((VOLUME_MB << 20)) --budget-s "${RIG_BUDGET_S:-600}" --budget-name "${RIG_BUDGET_NAME:-}"
          --between-budget-s "${RIG_BETWEEN_BUDGET_S:-0}" --steps "$STEPS" --discard "${RIG_DISCARD:-zero}")
    for o in "${SERVE_ARGS[@]}"; do args+=("--serve-arg=$o"); done
    [ -z "${RIG_REPLAY_LOG:-}" ] || args+=(--replay-log "$RIG_REPLAY_LOG")
    [ -z "${RIG_REQUIRE_WITNESS:-}" ] || args+=(--require-witness)
    [ -z "${RIG_EVERY_PREFIX:-}" ] || args+=(--every-prefix)
    mkdir -p "$WORK/check"
    # Not exec: the EXIT trap still runs, for the kernel's messages after
    # a failed check and the scratch directory.
    crc=0
    python3 "$HERE/replay_daemon.py" "${args[@]}" || crc=$?
    exit "$crc"
fi

# ----------------------------------------------------------------- check
args=(--log "${RIG_LOG:-$WORK/log.img}" --base "$WORK/base.img" --manifest "$MANIFEST"
      --pool "$POOL" --mountpoint "$MOUNTPOINT" --work "$WORK/check" --out "$OUT"
      --budget-s "${RIG_BUDGET_S:-840}" --deep-budget-s "${RIG_DEEP_BUDGET_S:-240}"
      --copies-budget-s "${RIG_COPIES_BUDGET_S:-300}" --discard "${RIG_DISCARD:-zero}")
[ "$LAYOUT" != slog ] || args+=(--log-device-image "$WORK/slog-base.img")
# Only where the intent log is on the device under test can the image
# show whether an fsynced file survived.
[ "$LAYOUT" != whole ] || args+=(--fsync-check)
[ -z "${RIG_PROVIDER:-}" ] || args+=(--provider "$RIG_PROVIDER")
[ -z "${RIG_REPLAY_LOG:-}" ] || args+=(--replay-log "$RIG_REPLAY_LOG")
[ -z "${RIG_REQUIRE_WITNESS:-}" ] || args+=(--require-witness)
mkdir -p "$WORK/check"
python3 "$HERE/replay_check.py" "${args[@]}"

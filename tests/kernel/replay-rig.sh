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
# Workload: a writer creates files that are never changed afterwards,
# each with an fsync, and records each file's checksum when it is
# complete. Meanwhile the pool is synced RIG_SYNCS times, and after each
# sync a mark goes into the log with the number of files that were
# complete before that sync began. Those files must be in the pool at
# every prefix from the mark on.
#
# Knobs (environment): RIG_SYNCS=40, RIG_FILE_KB=256, RIG_FILE_SLEEP=0.05
# (seconds between files), RIG_SYNC_SLEEP=0.6 (seconds between syncs),
# RIG_BUDGET_S=840 and RIG_DEEP_BUDGET_S=240 (the check's main pass),
# RIG_REPLAY_LOG (path of xfstests replay-log, a witness for the
# provider; skipped when unset, which RIG_REQUIRE_WITNESS=1 makes a
# failure).
#
# Needs root, ZFS, dm-log-writes, dm-snapshot, and `ublk` unless
# RIG_DEVICE is set. For a throwaway machine: it creates a pool named
# skzreplay.
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
FILE_SLEEP=${RIG_FILE_SLEEP:-0.05}
SYNC_SLEEP=${RIG_SYNC_SLEEP:-0.6}
DEVICE=${RIG_DEVICE:-}
MANIFEST=$WORK/manifest
STOP=$WORK/stop
SCRATCH=/dev/shm/skz-replay.$$

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
SLOG_LOOP=""; LOG_LOOP=""; UBLK_UP=""; WRITER=""
cleanup() {
    rc=$?
    set +e
    [ -n "$WRITER" ] && { touch "$STOP"; wait "$WRITER" 2>/dev/null; }
    timeout 120 zpool export -f "$POOL" 2>/dev/null
    dmsetup remove "$LW" 2>/dev/null
    [ -n "$UBLK_UP" ] && timeout 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1
    for l in "$SLOG_LOOP" "$LOG_LOOP"; do [ -n "$l" ] && losetup -d "$l" 2>/dev/null; done
    [ -d "$SCRATCH" ] && find "$SCRATCH" -maxdepth 1 -type f -delete && rmdir "$SCRATCH"
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
if [ -z "$DEVICE" ]; then
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
: > "$WORK/slog.img"
"$DD" if=/dev/zero of="$WORK/slog.img" bs=1M count="$SLOG_MB" oflag=direct conv=fsync status=none
SLOG_LOOP=$(losetup --find --show --direct-io=on "$WORK/slog.img" 2>/dev/null || losetup --find --show "$WORK/slog.img")

# ------------------------------------------------- the pool, then the base
# Created on the bare device, before any recording: the pool's labels
# are then where they are in production, and the record starts from an
# exported pool, which is the base image every prefix is built on.
zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$MOUNTPOINT" -O atime=off \
    "$POOL" "$DEVICE" log "$SLOG_LOOP"
[ ! -b "${DEVICE}p1" ] || { echo "zpool create partitioned $DEVICE: the labels are not at byte 0" >&2; exit 1; }
zpool export "$POOL"
log "copying the device as the base image"
"$DD" if="$DEVICE" of="$WORK/base.img" bs=1M iflag=direct conv=sparse status=none
truncate -s "$(blockdev --getsize64 "$DEVICE")" "$WORK/base.img"

# ------------------------------------------------------------ recording
: > "$WORK/log.img"
truncate -s "${LOG_MB}M" "$WORK/log.img"
LOG_LOOP=$(losetup --find --show --direct-io=on "$WORK/log.img" 2>/dev/null || losetup --find --show "$WORK/log.img")
dmsetup create "$LW" --table "0 $(blockdev --getsz "$DEVICE") log-writes $DEVICE $LOG_LOOP"
zpool import -d "/dev/mapper/$LW" -d "$SLOG_LOOP" -o cachefile=none "$POOL"
zpool status -P "$POOL" >&2

mkdir -p "$SCRATCH"
: > "$MANIFEST"
[ ! -e "$STOP" ] || unlink "$STOP"
writer() {
    local n=0 sum
    while [ ! -e "$STOP" ]; do
        head -c $((FILE_KB * 1024)) /dev/urandom > "$SCRATCH/f"
        sum=$(sha256sum < "$SCRATCH/f" | cut -d' ' -f1)
        "$DD" if="$SCRATCH/f" of="$MOUNTPOINT/f.$n" bs=1M conv=fsync status=none
        # Listed only once complete and fsynced: a line here is a promise.
        echo "$sum f.$n" >> "$MANIFEST"
        n=$((n + 1))
        sleep "$FILE_SLEEP"
    done
}
writer &
WRITER=$!
log "workload: $SYNCS syncs, files of $FILE_KB KiB"
for k in $(seq 1 "$SYNCS"); do
    sleep "$SYNC_SLEEP"
    c=$(wc -l < "$MANIFEST")
    zpool sync "$POOL"
    # One word: the target takes exactly one argument after "mark".
    dmsetup message "$LW" 0 mark "sync-$k-$c"
done
touch "$STOP"; wait "$WRITER"; WRITER=""
c=$(wc -l < "$MANIFEST")
zpool sync "$POOL"
dmsetup message "$LW" 0 mark "sync-$((SYNCS + 1))-$c"
zpool export "$POOL"
log "recorded: $c files, log-writes status: $(dmsetup status "$LW")"
for _ in $(seq 1 20); do dmsetup remove "$LW" 2>/dev/null && break; sleep 0.25; done
! dmsetup info "$LW" >/dev/null 2>&1 || { echo "could not remove $LW" >&2; exit 1; }
losetup -d "$LOG_LOOP"; LOG_LOOP=""
losetup -d "$SLOG_LOOP"; SLOG_LOOP=""
if [ -n "$UBLK_UP" ]; then timeout 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1 || true; UBLK_UP=""; fi
log "log: $(du -m "$WORK/log.img" | cut -f1) MiB on disk"

# ----------------------------------------------------------------- check
args=(--log "${RIG_LOG:-$WORK/log.img}" --base "$WORK/base.img" --manifest "$MANIFEST"
      --pool "$POOL" --mountpoint "$MOUNTPOINT" --work "$WORK/check" --out "$OUT"
      --budget-s "${RIG_BUDGET_S:-840}" --deep-budget-s "${RIG_DEEP_BUDGET_S:-240}")
[ -z "${RIG_PROVIDER:-}" ] || args+=(--provider "$RIG_PROVIDER")
[ -z "${RIG_REPLAY_LOG:-}" ] || args+=(--replay-log "$RIG_REPLAY_LOG")
[ -z "${RIG_REQUIRE_WITNESS:-}" ] || args+=(--require-witness)
mkdir -p "$WORK/check"
python3 "$HERE/replay_check.py" "${args[@]}"

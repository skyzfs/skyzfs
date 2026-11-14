#!/usr/bin/env bash
# The cut rig's recordings (M2.T1): real pools whose every request is
# recorded, for the reference for cuts (cut_ref.py) and its check
# (cut_check.py).
#
#   cut-record.sh OUT [WORK] [RECORDING...]
#
# The stack while recording, from a zeroed device:
#
#   ZFS pool ── dm-log-writes ── loop ── a file
#                  └─ its log ── loop ── a file
#
# Every recording begins before `zpool create`, so the base image is
# zeros and the record holds the pool's whole life. Each recording ends
# with the pool exported. Between them nothing is kept but what OUT/NAME
# holds: the fixture (cut_ref.py fixture), the pool's `txgs` kstat at
# the end of each session, and the check's results.
#
# The recordings (all by default, or those named):
#   life       create, load with snapshots, export, import, more load
#              with a snapshot destroyed, export
#   rewind     create, load, export, an import rewound a few groups
#              (zpool import -T), load, export
#   multihost  multihost=on (a host id is made when there is none),
#              load for 20 s, so that MMP writes uberblocks of its own
#   config     a change of the pool's configuration: a comment, a new
#              guid (reguid), a mirror attached and detached
#   stray      flushes that ZFS did not ask for, every few milliseconds
#              while the pool is loaded (fsync of the block device)
#   autotrim   autotrim=on with a trim batch of 4 groups, files deleted,
#              then a manual trim
#   full       a pool filled to ENOSPC, then rewritten in place, so that
#              ZFS reuses freed blocks at once
#
# A session of the pool (create or import, to export) is bracketed in
# the record by the marks `session-K-begin` (just before the create or
# the import) and `session-K-end`, and the
# pool's `txgs` kstat is copied to OUT/NAME/txgs-K.txt just before the
# end mark: the check holds the reference's cuts of each session against
# the groups ZFS says it synced.
#
# Knobs: CUT_VDEV_MB=512 (the pool's device), CUT_LOG_MB=8192 (the
# record's device, sparse), CUT_FILES=6 (files per sync of the load).
# After each recording, CUT_CHECK (default: the check, cut_check.py)
# runs as `$CUT_CHECK NAME DIR LOG BASE DEV_BYTES`; CUT_CHECK=none
# records only. Exit: 0 when every recording was made and checked, 1
# when a check gave a verdict against a cut, 2 for the rig's own error.
#
# Needs root, ZFS with zdb, dm-log-writes. For a throwaway machine: it
# creates a pool named skzcut.
# The recordings are called by name ("rec_$r"), which shellcheck cannot
# follow to them and the functions they call.
# shellcheck disable=SC2329
set -euo pipefail
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
OUT=${1:?usage: cut-record.sh OUT [WORK] [RECORDING...]}
WORK=${2:-/mnt/skz-cut}
shift $(( $# >= 2 ? 2 : 1 ))
ALL="life rewind multihost config stray autotrim full"
RECS=${*:-$ALL}
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)
POOL=skzcut
MNT=/skzcut
LW=skz-cut-lw
DEV=/dev/mapper/$LW
VDEV_MB=${CUT_VDEV_MB:-512}
LOG_MB=${CUT_LOG_MB:-8192}
FILES=${CUT_FILES:-6}
CHECK=${CUT_CHECK:-python3 $HERE/cut_check.py}
for v in "$VDEV_MB" "$LOG_MB" "$FILES"; do [[ $v =~ ^[0-9]+$ ]] || { echo "not a number: $v" >&2; exit 64; }; done
for r in $RECS; do [[ " $ALL " == *" $r "* ]] || { echo "no recording $r (there are: $ALL)" >&2; exit 64; }; done

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
VLOOP=""; LLOOP=""; MLOOP=""; STRAY=""; SESSION=0; NAME=""; RDIR=""
rig_error() { log "the rig's own error in $NAME: $*"; echo "$NAME	rig error	$*" >> "$OUT/recordings.tsv"; exit 2; }
z() { # SECONDS COMMAND...: a pool command under a limit; its failure is the rig's error
    local secs=$1; shift
    timeout "$secs" "$@" || rig_error "$* failed (status $?)"
}
mark() { dmsetup message "$LW" 0 mark "$1"; }
# The pool's txgs kstat, then the end mark: what ZFS says it synced in
# this session, up to the export (which syncs a few more).
session_end() {
    cat "/proc/spl/kstat/zfs/$POOL/txgs" > "$RDIR/txgs-$SESSION.txt" 2>/dev/null || : > "$RDIR/txgs-$SESSION.txt"
    mark "session-$SESSION-end"
}
# Before the create or the import, so that the groups it syncs are in the session.
session_begin() { SESSION=$((SESSION + 1)); mark "session-$SESSION-begin"; }
export_pool() { session_end; z 120 zpool export "$POOL"; }
# The mark session-K-ready when the create or import has returned: the
# check asks whether a writable import synced a group, with its flush,
# before it returned (design §5.2: a cut's group cannot err high for a
# publish).
import_pool() {
    session_begin
    z 120 zpool import -d "$DEV" -o cachefile=none "$@" "$POOL"
    mark "session-$SESSION-ready"
}
create_pool() {
    session_begin
    z 120 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$MNT" -O atime=off -O compression=off \
        "$@" "$POOL" "$DEV"
    mark "session-$SESSION-ready"
}
# K: K rounds of FILES files of 1 MiB each, a snapshot and a sync.
LOADED=0
load() {
    local k
    for k in $(seq 1 "$1"); do
        for _ in $(seq 1 "$FILES"); do
            LOADED=$((LOADED + 1))
            dd if=/dev/urandom of="$MNT/f$LOADED" bs=64K count=16 status=none
        done
        z 60 zfs snapshot "$POOL@s$LOADED"
        z 60 zpool sync "$POOL"
    done
}
committed() { awk 'NR > 2 && $3 == "C" { print $1 }' "/proc/spl/kstat/zfs/$POOL/txgs"; }

cleanup() {
    local rc=$?
    set +e
    [ -z "$STRAY" ] || { kill "$STRAY" 2>/dev/null; wait "$STRAY" 2>/dev/null; }
    timeout 120 zpool export -f "$POOL" 2>/dev/null
    dmsetup remove "$LW" 2>/dev/null
    for l in "$VLOOP" "$LLOOP" "$MLOOP"; do [ -z "$l" ] || losetup -d "$l" 2>/dev/null; done
    exit "$rc"
}
trap cleanup EXIT

for m in dm_log_writes zfs; do modprobe "$m" 2>/dev/null || true; done
[ -r /proc/spl/kstat/zfs/arcstats ] || { echo "the zfs kernel module is not loaded" >&2; exit 64; }
dmsetup targets | grep -q '^log-writes' || { echo "no dm-log-writes target in this kernel" >&2; exit 64; }
command -v zdb > /dev/null || { echo "no zdb" >&2; exit 64; }
timeout 60 zpool export -f "$POOL" 2>/dev/null || true
dmsetup remove "$LW" 2>/dev/null || true
# Enough history for a session's groups to be all in the kstat.
echo 2000 > /sys/module/zfs/parameters/zfs_txg_history

begin() {
    NAME=$1; RDIR=$OUT/$1; SESSION=0; LOADED=0
    rm -rf "${RDIR:?}"; mkdir -p "$RDIR"
    : > "$WORK/vdev.img"; truncate -s "${VDEV_MB}M" "$WORK/vdev.img"
    : > "$WORK/log.img"; truncate -s "${LOG_MB}M" "$WORK/log.img"
    VLOOP=$(losetup --find --show "$WORK/vdev.img")
    LLOOP=$(losetup --find --show "$WORK/log.img")
    dmsetup create "$LW" --table "0 $((VDEV_MB << 11)) log-writes $VLOOP $LLOOP"
    log "$NAME: recording on $DEV ($VDEV_MB MiB)"
}
end() {
    dmsetup remove "$LW"
    losetup -d "$VLOOP"; VLOOP=""
    losetup -d "$LLOOP"; LLOOP=""
    : > "$WORK/base.img"; truncate -s "${VDEV_MB}M" "$WORK/base.img"
    log "$NAME: recorded, $(du -h "$WORK/log.img" | cut -f1) of record"
    echo "$NAME	recorded	$SESSION sessions" >> "$OUT/recordings.tsv"
    [ "$CHECK" = none ] && return 0
    local rc=0
    # shellcheck disable=SC2086 # CHECK is a command line
    $CHECK "$NAME" "$RDIR" "$WORK/log.img" "$WORK/base.img" $((VDEV_MB << 20)) || rc=$?
    echo "$NAME	checked	exit $rc" >> "$OUT/recordings.tsv"
    WORST=$(( rc > WORST ? rc : WORST ))
}

rec_life() {
    create_pool
    load 4
    export_pool
    import_pool
    load 3
    z 60 zfs destroy "$POOL@s$FILES"
    z 60 zpool sync "$POOL"
    export_pool
}
rec_rewind() {
    local groups t
    create_pool
    load 8
    mapfile -t groups < <(committed)
    [ "${#groups[@]}" -ge 6 ] || rig_error "only ${#groups[@]} groups committed before the rewind"
    t=${groups[$(( ${#groups[@]} - 4 ))]}
    echo "$t" > "$RDIR/rewind-txg"
    export_pool
    log "rewind: importing at group $t of ${groups[-1]}"
    import_pool -T "$t"
    load 3
    export_pool
}
rec_multihost() {
    [ -s /etc/hostid ] || zgenhostid
    create_pool -o multihost=on
    local t0=$SECONDS
    while [ $((SECONDS - t0)) -lt 20 ]; do load 1; sleep 1; done
    export_pool
}
rec_config() {
    : > "$WORK/mirror.img"; truncate -s "${VDEV_MB}M" "$WORK/mirror.img"
    MLOOP=$(losetup --find --show "$WORK/mirror.img")
    create_pool
    load 2
    z 60 zpool set comment="a cut fixture" "$POOL"
    z 60 zpool reguid "$POOL"
    load 1
    z 60 zpool attach -w "$POOL" "$DEV" "$MLOOP"
    load 1
    z 60 zpool detach "$POOL" "$MLOOP"
    load 2
    export_pool
    losetup -d "$MLOOP"; MLOOP=""
    rm -f "$WORK/mirror.img"
}
rec_stray() {
    create_pool
    python3 -c '
import os, sys, time
fd = os.open(sys.argv[1], os.O_RDONLY)
while not os.path.exists(sys.argv[2]):
    os.fsync(fd)
    time.sleep(0.002)
' "$DEV" "$WORK/stray.stop" &
    STRAY=$!
    load 6
    touch "$WORK/stray.stop"; wait "$STRAY"; STRAY=""; rm -f "$WORK/stray.stop"
    export_pool
}
rec_autotrim() {
    local batch
    batch=$(cat /sys/module/zfs/parameters/zfs_trim_txg_batch)
    echo 4 > /sys/module/zfs/parameters/zfs_trim_txg_batch
    create_pool -o autotrim=on
    load 4
    rm -f "$MNT"/f1 "$MNT"/f2 "$MNT"/f3
    for s in $(zfs list -H -r -t snapshot -o name "$POOL"); do z 60 zfs destroy "$s"; done
    for _ in 1 2 3 4 5 6; do z 60 zpool sync "$POOL"; sleep 0.5; done
    rm -f "$MNT"/f4 "$MNT"/f5
    z 60 zpool sync "$POOL"
    z 300 zpool trim -w "$POOL"
    load 1
    export_pool
    echo "$batch" > /sys/module/zfs/parameters/zfs_trim_txg_batch
}
rec_full() {
    local k
    create_pool
    dd if=/dev/urandom of="$MNT/big" bs=1M status=none 2>/dev/null || true
    z 60 zpool sync "$POOL"
    zfs list -H -o used,avail "$POOL" > "$RDIR/space.txt"
    for k in $(seq 1 12); do
        dd if=/dev/urandom of="$MNT/big" bs=1M count=4 seek=$(( (k * 37) % 200 )) conv=notrunc status=none 2>/dev/null || true
        z 60 zpool sync "$POOL"
    done
    zfs list -H -o used,avail "$POOL" >> "$RDIR/space.txt"
    export_pool
}

WORST=0
: > "$OUT/recordings.tsv"
zfs version > "$OUT/zfs-version.txt" 2>&1 || true
for r in $RECS; do
    begin "$r"
    "rec_$r"
    end
done
log "done: worst exit $WORST"
exit "$WORST"

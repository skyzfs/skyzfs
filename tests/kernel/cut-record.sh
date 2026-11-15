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
#   rewindsync as rewind, then after the import a small write and a
#              `zpool sync` at a time until the newest group that wrote
#              passes the abandoned history's newest (zdb -u after the
#              export); OUT/NAME/rewind-sync.txt says how many and how
#              long
#   rewindf    create, load, export, the newest group's root block (the
#              MOS's objset, all its copies) overwritten with zeros
#              through the recorded device, `zpool import -F`, load,
#              export; OUT/NAME/rewind-f.txt says what the import said
#   slog       the product's layout (design §6.1): the data device
#              recorded, a log device of 256 MiB on another loop file
#              (not recorded); after the create an export, a copy of
#              the log device (its labels, no intent log: what a restore
#              can place beside the data device for zdb) and an import;
#              then load with synchronous writes going on (dd
#              oflag=sync, 4 KiB at a time, into the intent log), export
#   multihost  multihost=on (the host id is 5eed1234 for every
#              recording), load for 20 s, so that MMP writes uberblocks
#              of its own; MMP at a 100 ms interval with no failure
#              intervals and one import interval (see below)
#   config     a change of the pool's configuration: a comment, a new
#              guid (reguid), a mirror attached and detached
#   stray      flushes that ZFS did not ask for, every 2 ms while the
#              pool is loaded (fsync of the block device), with the back
#              labels' writes held 20 ms (dm-delay) so that flushes fall
#              between a group's uberblock writes
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
#
# CUT_SERVER=daemon (M2.T3) records through the daemon instead: the
# pool's device is a volume that skyzfsd serves (CUT_SKYZFSD, a build
# with its test hooks), from a directory under WORK, and the record is
# the daemon's own request log (DIR/requests.tsv: when the store applied
# each write, the bytes of every write to the labels, each flush's
# barrier and each cut the daemon counted). The marks go to
# DIR/marks.tsv with CLOCK_MONOTONIC, the request log's clock. Its
# status is read into DIR/status.txt before it is detached, and the
# check is called with the request log in place of LOG and `-` for
# BASE. stray's 20 ms delay of the back labels has no place below the
# daemon and is left out.
# After each recording, CUT_CHECK (default: the check, cut_check.py)
# runs as `$CUT_CHECK NAME DIR LOG BASE DEV_BYTES [LOG_DEVICE_IMAGE]`; CUT_CHECK=none
# records only. Exit: 0 when every recording was made and checked, 1
# when a check gave a verdict against a cut, 2 for the rig's own error.
#
# A pool with multihost=on that was not exported, which is what every
# cut of it before the export is (and what a restore or a node B meets
# whenever the pool's own node did not export it), cannot be imported
# read-only in ZFS 2.4.1, on any device. The import first waits out the
# activity check (failure intervals x interval x 2, 20 s at the
# defaults; 1 s at the least) and then makes a claim: it writes
# uberblocks with a random MMP sequence to every writable leaf and reads
# them back over 10 intervals (spa_activity_check_claim,
# mmp_claim_uberblock; only the tryimport, an extreme rewind and zdb are
# spared). A readonly=on import opens no leaf for writing, so the claim
# has no good write, and zpool says "pool is imported on host
# '<unknown>' (hostid=0)", which is not about a host id at all (runs
# 43638676548 and 43645075993, the second from a writable device). So
# the check imports such a pool writable, from a copy of the cut, and
# the recording shortens MMP's timings so that an import takes about 2 s
# and not 30 s. A restore of such a pool must import it writable, and
# expect the claim's writes and that wait.
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
ALL="life slog rewind rewindsync rewindf multihost config stray autotrim full"
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
SERVER=${CUT_SERVER:-log-writes}
SKYZFSD=${CUT_SKYZFSD:-}
case $SERVER in
    log-writes) ;;
    daemon) [ -x "$SKYZFSD" ] || { echo "CUT_SERVER=daemon needs CUT_SKYZFSD, the daemon's binary" >&2; exit 64; }
            DEV="" ;;
    *) echo "CUT_SERVER is log-writes or daemon, not $SERVER" >&2; exit 64 ;;
esac
for v in "$VDEV_MB" "$LOG_MB" "$FILES"; do [[ $v =~ ^[0-9]+$ ]] || { echo "not a number: $v" >&2; exit 64; }; done
for r in $RECS; do [[ " $ALL " == *" $r "* ]] || { echo "no recording $r (there are: $ALL)" >&2; exit 64; }; done

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
VLOOP=""; LLOOP=""; MLOOP=""; SLOOP=""; STRAY=""; SESSION=0; NAME=""; RDIR=""; SERVE_PID=""
VOL=$WORK/vol; RUN=$WORK/run; SERVE_RC=""
# One line in recordings.tsv whatever the message holds.
rig_error() { local m; m=$(tr '\t\n' '  ' <<< "$*"); log "the rig's own error in $NAME: $m"; echo "$NAME	rig error	$m" >> "$OUT/recordings.tsv"; exit 2; }
z() { # SECONDS COMMAND...: a pool command under a limit; its failure is the rig's error
    local secs=$1; shift
    timeout "$secs" "$@" || rig_error "$* failed (status $?)"
}
mark() {
    if [ "$SERVER" = daemon ]; then
        printf '%s\t%s\n' "$(python3 -c 'import time; print(time.monotonic_ns())')" "$1" >> "$RDIR/marks.tsv"
    else
        dmsetup message "$LW" 0 mark "$1"
    fi
}
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
# Each snapshot's group and session go to snaps.tsv, and the SHA-256 of
# every file it holds, read through .zfs/snapshot, to snap-NAME.sha256:
# what node B (M2.T2) compares a published snapshot with.
LOADED=0
load() {
    local k s
    for k in $(seq 1 "$1"); do
        for _ in $(seq 1 "$FILES"); do
            LOADED=$((LOADED + 1))
            dd if=/dev/urandom of="$MNT/f$LOADED" bs=64K count=16 status=none
        done
        s=s$LOADED
        z 60 zfs snapshot "$POOL@$s"
        # From the snapshot itself: a file a writer goes on appending to
        # (slog's synced) differs in the live directory by then.
        (cd "$MNT/.zfs/snapshot/$s" && find . -maxdepth 1 -type f -printf '%P\n' | LC_ALL=C sort | xargs -r sha256sum --) > "$RDIR/snap-$s.sha256"
        printf '%s\t%s\t%s\n' "$s" "$(zfs get -Hp -o value createtxg "$POOL@$s")" "$SESSION" >> "$RDIR/snaps.tsv"
        z 60 zpool sync "$POOL"
    done
}
committed() { awk 'NR > 2 && $3 == "C" { print $1 }' "/proc/spl/kstat/zfs/$POOL/txgs"; }

cleanup() {
    local rc=$?
    set +e
    [ -z "$STRAY" ] || { kill "$STRAY" 2>/dev/null; wait "$STRAY" 2>/dev/null; }
    timeout 120 zpool export -f "$POOL" 2>/dev/null
    if [ -n "$SERVE_PID" ]; then
        timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > /dev/null 2>&1
        serve_end 30 || { kill -9 "$SERVE_PID" 2>/dev/null; SERVE_PID=""; }
    fi
    dmsetup remove "$LW" 2>/dev/null
    [ -z "$LOWER_UP" ] || dmsetup remove "$LOWER" 2>/dev/null
    for l in "$VLOOP" "$LLOOP" "$MLOOP" "$SLOOP"; do [ -z "$l" ] || losetup -d "$l" 2>/dev/null; done
    [ -z "$MH_SAVED" ] || mmp_restore
    exit "$rc"
}
trap cleanup EXIT

for m in dm_log_writes dm_delay zfs; do modprobe "$m" 2>/dev/null || true; done
[ -r /proc/spl/kstat/zfs/arcstats ] || { echo "the zfs kernel module is not loaded" >&2; exit 64; }
[ "$SERVER" = daemon ] || dmsetup targets | grep -q '^log-writes' || { echo "no dm-log-writes target in this kernel" >&2; exit 64; }
command -v zdb > /dev/null || { echo "no zdb" >&2; exit 64; }
timeout 60 zpool export -f "$POOL" 2>/dev/null || true
dmsetup remove "$LW" 2>/dev/null || true
dmsetup remove skz-cut-lower 2>/dev/null || true
# Enough history for a session's groups to be all in the kstat.
echo 2000 > /sys/module/zfs/parameters/zfs_txg_history
# One host id, in /etc/hostid and in the loaded module, before any pool:
# a multihost pool records it, and the check's zdb and import must find
# the same one or refuse the pool as another host's (run 43631961753:
# "pool is imported on host '<unknown>' (hostid=0)").
zgenhostid -f 5eed1234
echo 0x5eed1234 > /sys/module/spl/parameters/spl_hostid
[ "$(hostid)" = 5eed1234 ] || { echo "the host id is $(hostid), not 5eed1234" >&2; exit 64; }

# NAME [delay-end]: with delay-end, the device's last 512 KiB (labels L2
# and L3) sits behind dm-delay, writes held 20 ms: a group's uberblock
# writes to the back labels then complete after those to the front, and
# a flush can fall between them, which exercises the repeat rule (M2.1's
# fixture test found that none did without it).
LOWER=skz-cut-lower; LOWER_UP=""
# SECONDS: wait for the daemon's serve to end; 1 if it has not.
serve_end() {
    local _i
    for _i in $(seq 1 $(($1 * 4))); do
        kill -0 "$SERVE_PID" 2>/dev/null || { SERVE_RC=0; wait "$SERVE_PID" 2>/dev/null || SERVE_RC=$?; SERVE_PID=""; return 0; }
        sleep 0.25
    done
    return 1
}
# The volume, served by the daemon with its request log: DEV is its device.
begin_daemon() {
    local _i
    rm -rf "${VOL:?}" "$VOL.ready"; mkdir -p "$RUN"
    timeout 60 "$SKYZFSD" create --dir "$VOL" --size $((VDEV_MB << 20)) --run-dir "$RUN" > "$RDIR/create.log" 2>&1 \
        || rig_error "skyzfsd create failed: $(tail -n 2 "$RDIR/create.log")"
    "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" --request-log "$RDIR/requests.tsv" > "$VOL.ready" 2> "$RDIR/serve.log" &
    SERVE_PID=$!; DEV=""
    for _i in $(seq 1 480); do
        DEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$VOL.ready" | head -n 1)
        [ -z "$DEV" ] || [ ! -b "$DEV" ] || break
        kill -0 "$SERVE_PID" 2>/dev/null || { SERVE_PID=""; rig_error "skyzfsd serve ended before it was ready: $(tail -n 3 "$RDIR/serve.log")"; }
        sleep 0.25
    done
    [ -n "$DEV" ] && [ -b "$DEV" ] || rig_error "skyzfsd serve was not ready in 120 s"
    : > "$RDIR/marks.tsv"
    log "$NAME: recording through skyzfsd on $DEV ($VDEV_MB MiB)"
}
end_daemon() {
    timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$RUN" > "$RDIR/status.txt" 2>&1 \
        || rig_error "skyzfsd status failed: $(tail -n 2 "$RDIR/status.txt")"
    timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > "$RDIR/detach.log" 2>&1 \
        || rig_error "skyzfsd detach failed: $(tail -n 2 "$RDIR/detach.log")"
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after the detach"
    echo "serve's exit after the detach: $SERVE_RC" >> "$RDIR/detach.log"
    rm -rf "${VOL:?}" "$VOL.ready"
}
begin() {
    local sectors=$((VDEV_MB << 11)) under
    NAME=$1; RDIR=$OUT/$1; SESSION=0; LOADED=0
    rm -rf "${RDIR:?}"; mkdir -p "$RDIR"
    if [ "$SERVER" = daemon ]; then
        [ "${2:-}" != delay-end ] || log "$NAME: no delay of the back labels below the daemon"
        begin_daemon
        return 0
    fi
    : > "$WORK/vdev.img"; truncate -s "${VDEV_MB}M" "$WORK/vdev.img"
    : > "$WORK/log.img"; truncate -s "${LOG_MB}M" "$WORK/log.img"
    VLOOP=$(losetup --find --show "$WORK/vdev.img")
    LLOOP=$(losetup --find --show "$WORK/log.img")
    under=$VLOOP
    if [ "${2:-}" = delay-end ]; then
        printf '0 %s linear %s 0\n%s 1024 delay %s %s 0 %s %s 20\n' "$((sectors - 1024))" "$VLOOP" \
            "$((sectors - 1024))" "$VLOOP" "$((sectors - 1024))" "$VLOOP" "$((sectors - 1024))" | dmsetup create "$LOWER"
        LOWER_UP=1; under=/dev/mapper/$LOWER
    fi
    dmsetup create "$LW" --table "0 $sectors log-writes $under $LLOOP"
    log "$NAME: recording on $DEV ($VDEV_MB MiB)"
}
end() {
    local record=$WORK/log.img base=$WORK/base.img
    if [ "$SERVER" = daemon ]; then
        end_daemon
        record=$RDIR/requests.tsv; base=-
        log "$NAME: recorded, $(du -h "$record" | cut -f1) of request log"
    else
        dmsetup remove "$LW"
        [ -z "$LOWER_UP" ] || { dmsetup remove "$LOWER"; LOWER_UP=""; }
        losetup -d "$VLOOP"; VLOOP=""
        losetup -d "$LLOOP"; LLOOP=""
        : > "$base"; truncate -s "${VDEV_MB}M" "$base"
        log "$NAME: recorded, $(du -h "$record" | cut -f1) of record"
    fi
    echo "$NAME	recorded	$SESSION sessions" >> "$OUT/recordings.tsv"
    [ "$CHECK" = none ] && return 0
    local rc=0
    # shellcheck disable=SC2086 # CHECK is a command line
    local logdev=()
    [ ! -e "$WORK/slog-base.img" ] || logdev=("$WORK/slog-base.img")
    $CHECK "$NAME" "$RDIR" "$record" "$base" $((VDEV_MB << 20)) "${logdev[@]}" || rc=$?
    rm -f "$WORK/slog-base.img"
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
# The newest uberblock of the exported pool, by zdb: GROUP and the root
# block pointer's line.
# zdb 2.2 (the userland of ubuntu-24.04, beside a 2.4 module) prints the
# root block pointer at -uuuu only, as `rootbp =`; 2.4 as `bp =`.
newest_ub() { timeout 60 zdb -e -p "$DEV" -uuuu "$POOL" 2> /dev/null; }
groups_written() { awk 'NR > 2 && $3 == "C" && $6 > 0 { print $1 }' "/proc/spl/kstat/zfs/$POOL/txgs"; }
rec_rewindsync() {
    local groups t old k t0 top
    create_pool
    load 8
    mapfile -t groups < <(committed)
    [ "${#groups[@]}" -ge 6 ] || rig_error "only ${#groups[@]} groups committed before the rewind"
    t=${groups[$(( ${#groups[@]} - 4 ))]}
    export_pool
    old=$(newest_ub | awk '$1 == "txg" { print $3; exit }')
    [[ $old =~ ^[0-9]+$ ]] || rig_error "zdb gave no newest group after the export"
    import_pool -T "$t"
    t0=$(date +%s%N); k=0; top=0
    while [ "$k" -lt 300 ]; do
        top=$(groups_written | tail -n 1)
        [ "${top:-0}" -le "$old" ] || break
        k=$((k + 1))
        dd if=/dev/urandom of="$MNT/sync-$k" bs=4K count=1 status=none
        z 60 zpool sync "$POOL"
    done
    printf 'rewound to %s; abandoned newest %s; %s syncs, %s ms, until group %s wrote
' \
        "$t" "$old" "$k" "$(( ($(date +%s%N) - t0) / 1000000 ))" "$top" > "$RDIR/rewind-sync.txt"
    [ "${top:-0}" -gt "$old" ] || rig_error "300 syncs did not pass group $old"
    load 1
    export_pool
}
rec_rewindf() {
    local ub dvas d off asize n=0
    create_pool
    load 6
    export_pool
    ub=$(newest_ub)
    echo "$ub" > "$RDIR/newest-before-damage.txt"
    # zdb 2.4 prints the root block pointer as `bp = DVA[0]=<...> ...`.
    mapfile -t dvas < <(sed -n 's/^[[:space:]]*\(root\)\{0,1\}bp = //p' <<< "$ub" | head -n 1 | grep -o 'DVA\[[0-9]\]=<[0-9]*:[0-9a-f]*:[0-9a-f]*>')
    [ "${#dvas[@]}" -ge 1 ] || rig_error "zdb gave no root block pointer: $(head -c 300 <<< "$ub")"
    mark "damage"
    for d in "${dvas[@]}"; do
        off=$(( 0x$(cut -d: -f2 <<< "$d") + (4 << 20) )); asize=$(( 0x$(cut -d: -f3 <<< "$d" | tr -d '>') ))
        dd if=/dev/zero of="$DEV" bs=512 seek=$((off / 512)) count=$((asize / 512)) oflag=direct conv=notrunc status=none
        n=$((n + 1))
    done
    printf 'zeroed %s copies of the root block of group %s: %s\n' "$n" "$(awk '$1 == "txg" { print $3; exit }' <<< "$ub")" "${dvas[*]}" > "$RDIR/rewind-f.txt"
    # What zpool import sees, and the kernel's account of each attempt
    # (run 43638676548: -F refused the pool as "one or more devices is
    # currently unavailable" and nothing said why): -F, then -FX.
    { echo "== zpool import -d (the listing)"; timeout 120 zpool import -d "$DEV" 2>&1; } >> "$RDIR/rewind-f.txt"
    echo 1 > /sys/module/zfs/parameters/zfs_dbgmsg_enable
    session_begin
    local how="" opt
    for opt in -F -FX; do
        echo 0 > /proc/spl/kstat/zfs/dbgmsg
        echo "== zpool import $opt" >> "$RDIR/rewind-f.txt"
        if timeout 300 zpool import -d "$DEV" -o cachefile=none "$opt" "$POOL" >> "$RDIR/rewind-f.txt" 2>&1; then how=$opt; fi
        { grep -E 'spa_load|mmp|rewind|txg|error|fail' /proc/spl/kstat/zfs/dbgmsg || true; } | tail -n 60 > "$RDIR/dbgmsg$opt.txt"
        [ -z "$how" ] || break
    done
    echo 0 > /sys/module/zfs/parameters/zfs_dbgmsg_enable
    [ -n "$how" ] || rig_error "zpool import -F and -FX failed: $(grep -v '^==' "$RDIR/rewind-f.txt" | tail -n 3 | tr '\n' ' ')"
    echo "imported with $how" >> "$RDIR/rewind-f.txt"
    mark "session-$SESSION-ready"
    committed | tail -n 3 | tr '\n' ' ' | sed 's/^/groups committed after the import: /' >> "$RDIR/rewind-f.txt"
    load 3
    export_pool
}
rec_slog() {
    : > "$WORK/slog.img"; truncate -s 256M "$WORK/slog.img"
    SLOOP=$(losetup --find --show "$WORK/slog.img")
    session_begin
    z 120 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$MNT" -O atime=off -O compression=off \
        "$POOL" "$DEV" log "$SLOOP"
    mark "session-$SESSION-ready"
    export_pool
    cp --sparse=always "$WORK/slog.img" "$WORK/slog-base.img"
    session_begin
    z 120 zpool import -d "$DEV" -d "$SLOOP" -o cachefile=none "$POOL"
    mark "session-$SESSION-ready"
    python3 -c '
import os, sys, time
fd = os.open(sys.argv[1] + "/synced", os.O_WRONLY | os.O_CREAT | os.O_SYNC, 0o644)
k = 0
while not os.path.exists(sys.argv[2]):
    os.write(fd, os.urandom(4096)); k += 1
    time.sleep(0.005)
print(k)
' "$MNT" "$WORK/sync.stop" > "$RDIR/sync-writes.txt" &
    STRAY=$!
    load 6
    touch "$WORK/sync.stop"; wait "$STRAY"; STRAY=""; rm -f "$WORK/sync.stop"
    zpool iostat -v "$POOL" > "$RDIR/iostat.txt" 2>&1 || true
    export_pool
    losetup -d "$SLOOP"; SLOOP=""
    rm -f "$WORK/slog.img"
}
# MMP's timings for the multihost recording, and the old values for the
# cleanup to put back. ZFS 2.4 imports a pool with multihost=on that was
# not exported only after an activity check of fail_intervals x interval
# x 2 (20 s at the defaults) and a claim of 10 intervals (10 s), read
# only or not: the check imports every cut. A 100 ms interval with no
# failure intervals (the pool never suspends itself) and one import
# interval make that about 2 s, and give MMP ten writes a second.
MH_SAVED=""
mmp_fast() {
    local p v
    for p in zfs_multihost_interval=100 zfs_multihost_fail_intervals=0 zfs_multihost_import_intervals=1; do
        v=$(cat "/sys/module/zfs/parameters/${p%%=*}") || return 1
        MH_SAVED+=" ${p%%=*}=$v"
        echo "${p#*=}" > "/sys/module/zfs/parameters/${p%%=*}" || return 1
    done
    echo "MMP for this recording:${MH_SAVED/#/ was} (now 100 ms, 0 failure intervals, 1 import interval)" > "$RDIR/mmp.txt"
}
mmp_restore() {
    local p
    for p in $MH_SAVED; do echo "${p#*=}" > "/sys/module/zfs/parameters/${p%%=*}"; done
    MH_SAVED=""
}
rec_multihost() {
    mmp_fast || rig_error "could not set MMP's timings"
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

# NAME, in its own subshell: exits with this recording's status alone,
# not the worst so far.
record() {
    WORST=0
    if [ "$1" = stray ]; then begin "$1" delay-end; else begin "$1"; fi
    "rec_$1"
    end
    exit "$WORST"
}

WORST=0
: > "$OUT/recordings.tsv"
zfs version > "$OUT/zfs-version.txt" 2>&1 || true
# Each recording in a subshell with its own cleanup: the rig's error in
# one (exit 2) is that recording's, and the next one still runs. The
# subshell is not the left side of `||`: there bash ignores errexit in
# everything it runs, and a failed losetup, dmsetup or mark would let
# the recording go on.
for r in $RECS; do
    set +e
    (
        set -e
        trap cleanup EXIT
        record "$r"
    )
    rc=$?
    set -e
    # A command that failed outside z and rig_error stopped the recording
    # with its own status and no line: that is the rig's error, not a
    # verdict.
    if [ "$rc" -ne 0 ] && ! awk -F '\t' -v r="$r" '$1 == r && ($2 == "rig error" || $2 == "checked") { f = 1 } END { exit !f }' "$OUT/recordings.tsv"; then
        log "the rig's own error in $r: a command failed (status $rc); the log above names it"
        printf '%s\trig error\ta command failed (status %s); the job log names it\n' "$r" "$rc" >> "$OUT/recordings.tsv"
        rc=2
    fi
    WORST=$(( rc > WORST ? rc : WORST ))
done
log "done: worst exit $WORST"
exit "$WORST"

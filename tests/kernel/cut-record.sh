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
# status is read into DIR/status.txt before it is stopped (SIGTERM) and
# its device deleted (detach), and the
# check is called with the request log in place of LOG and `-` for
# BASE. stray's 20 ms delay of the back labels has no place below the
# daemon and is left out.
# With CUT_HOLD=1 as well (M2.T3, second stage) the daemon holds cuts
# (`serve --hold-cuts`), and a holder (cut_hold.py) stands in for the
# uploader beside the recording: it writes each cut being uploaded onto
# an image of the rig's, hashes the blocks of its map, and releases it;
# after the export it records what is held beside the status and
# drains. Its files (DIR/held*.tsv, held-final.txt, status-held.txt,
# holder.log) are for the check's second stage.
# With CUT_UPLOAD=1 instead (M2.T3, the daemon's uploads, M2.4c) the
# volume is made in object storage too (`create --endpoint --bucket
# --volume`; the endpoint and bucket from SKZ_S3_ENDPOINT and
# SKZ_S3_BUCKET, the keys from AWS_ACCESS_KEY_ID and
# AWS_SECRET_ACCESS_KEY, the volume named cut-NAME), so the daemon holds
# its cuts and uploads each one itself, and no holder runs (the daemon
# refuses `release` while it uploads). A watcher (cut_watch.py) stands
# beside the recording as node B: CUT_NODEB, bins/skyzfs-nodeb, reads
# head and the cut's row twice a second and fetches each cut head comes
# to hold. After the export the recording waits up to 300 s for the
# daemon to upload what it still holds (`upload_state idle` and
# `cuts_held 0`; DIR/drain.txt says how it ended), then the watcher
# fetches head once more and ends. Its files (DIR/uploaded*.tsv,
# rows.tsv, watch.log) are for the check's upload lines.
# The recordings umoved, urow, ukill, ulease, ulost, uv1 and uv1moved
# are made only then: the uploader beside another writer, a kill between
# a commit and its cut's release, the lease, and a row of version 1 (the
# upload courses, below rec_failed).
# The publish courses (the channel rig, skyzfs-publish-rig.yml; below
# rec_ufailed) are made then too, by name only: pplain, prewind, pkill,
# pkillman, pkillpub and pchannel. An agent (publish_agent.py) asks the
# daemon through its channel to publish snapshots of the pool, and
# DIR/ends.tsv gets each answer under the course's label for it, for
# publish_check.py (their CUT_CHECK).
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
# The writes to the device below are O_DIRECT, which uutils' dd on
# ubuntu-26.04 does not do the GNU way: run "$DD" for them.
# shellcheck source=tests/lib/gnu-dd.sh
. "$HERE/../lib/gnu-dd.sh"
OUT=${1:?usage: cut-record.sh OUT [WORK] [RECORDING...]}
WORK=${2:-/mnt/skz-cut}
shift $(( $# >= 2 ? 2 : 1 ))
ALL="life slog rewind rewindsync rewindf multihost config stray autotrim full"
# Only where the daemon holds cuts (CUT_HOLD=1): what the ten above
# never make the daemon do (M2.T3).
HELD_ONLY="bound boundsparse edges takeover failed"
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
HOLD=${CUT_HOLD:-0}
UPLOAD=${CUT_UPLOAD:-0}
NODEB=${CUT_NODEB:-}
S3_ENDPOINT=${SKZ_S3_ENDPOINT:-}
S3_BUCKET=${SKZ_S3_BUCKET:-}
# unostore: the commands that take object storage down and bring it back.
S3_DOWN=${CUT_S3_DOWN:-}
S3_UP=${CUT_S3_UP:-}
[ "$SERVER" != daemon ] || [ "$HOLD" != 1 ] || ALL="$ALL $HELD_ONLY"
# Only where the daemon uploads (CUT_UPLOAD=1): the uploader beside
# another writer, a kill between a commit and its release, the lease
# (M2.4c; the courses below), and a row of version 1 (F1).
UPLOAD_ONLY="umoved urow ukill ulease ulost uv1 uv1moved"
# By name only, until they have run stable: object storage down at the
# start, a kill inside an upload, a failed volume.
UPLOAD_NAMED="unostore ukillput ufailed"
# The channel rig's (skyzfs-publish-rig.yml), by name only.
PUBLISH_NAMED="pplain prewind pkill pkillman pkillpub pchannel"
KNOWN=$ALL
[ "$UPLOAD" != 1 ] || { ALL="$ALL $UPLOAD_ONLY"; KNOWN="$ALL $UPLOAD_NAMED $PUBLISH_NAMED"; }
RECS=${*:-$ALL}
case $SERVER in
    log-writes) ;;
    daemon) [ -x "$SKYZFSD" ] || { echo "CUT_SERVER=daemon needs CUT_SKYZFSD, the daemon's binary" >&2; exit 64; }
            DEV="" ;;
    *) echo "CUT_SERVER is log-writes or daemon, not $SERVER" >&2; exit 64 ;;
esac
if [ "$UPLOAD" = 1 ]; then
    [ "$SERVER" = daemon ] || { echo "CUT_UPLOAD=1 needs CUT_SERVER=daemon" >&2; exit 64; }
    [ "$HOLD" != 1 ] || { echo "CUT_UPLOAD=1 and CUT_HOLD=1 exclude each other: an uploading daemon refuses the holder's release" >&2; exit 64; }
    [ -x "$NODEB" ] || { echo "CUT_UPLOAD=1 needs CUT_NODEB, node B's binary" >&2; exit 64; }
    [ -n "$S3_ENDPOINT" ] && [ -n "$S3_BUCKET" ] || { echo "CUT_UPLOAD=1 needs SKZ_S3_ENDPOINT and SKZ_S3_BUCKET" >&2; exit 64; }
fi
for v in "$VDEV_MB" "$LOG_MB" "$FILES"; do [[ $v =~ ^[0-9]+$ ]] || { echo "not a number: $v" >&2; exit 64; }; done
for r in $RECS; do [[ " $KNOWN " == *" $r "* ]] || { echo "no recording $r (there are: $KNOWN)" >&2; exit 64; }; done

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
# A kill -9 cuts a line of the request log on a page of the file
# (cut_daemon_check.py's torn_line): say where the logs are written.
[ "$SERVER" != daemon ] || log "request logs on $(stat -f -c %T "$OUT"), page size $(getconf PAGESIZE)"
VLOOP=""; LLOOP=""; MLOOP=""; SLOOP=""; STRAY=""; SESSION=0; NAME=""; RDIR=""; SERVE_PID=""
VOL=$WORK/vol; RUN=$WORK/run; SERVE_RC=""; HOLD_PID=""; WATCH_PID=""
# The upload courses': extra options of the next serve, a function run
# between the create and the first serve, the uploader's rest (drain), a
# writer in the background.
SERVE_ARGS=(); CREATE_ARGS=(); PRE_SERVE=""; DRAIN_STATE=idle; DRAIN_HELD=1; WRITER=""
# Set while a course has taken object storage down (CUT_S3_DOWN): the
# cleanup brings it back.
S3_IS_DOWN=""
NOFLUSH=/sys/module/zfs/parameters/zfs_nocacheflush
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
    [ -z "$HOLD_PID" ] || { kill "$HOLD_PID" 2>/dev/null; wait "$HOLD_PID" 2>/dev/null; }
    [ -z "$WATCH_PID" ] || { kill "$WATCH_PID" 2>/dev/null; wait "$WATCH_PID" 2>/dev/null; }
    [ -z "$WRITER" ] || { touch "$WORK/writer.stop"; kill "$WRITER" 2>/dev/null; }
    [ -z "$S3_IS_DOWN" ] || bash -c "$S3_UP" > /dev/null 2>&1
    [ -z "$SERVE_PID" ] || kill -CONT "$SERVE_PID" 2>/dev/null
    timeout 120 zpool export -f "$POOL" 2>/dev/null
    if [ -n "$SERVE_PID" ]; then
        kill -TERM "$SERVE_PID" 2>/dev/null
        serve_end 30 || { kill -9 "$SERVE_PID" 2>/dev/null; SERVE_PID=""; }
        timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > /dev/null 2>&1
    fi
    dmsetup remove "$LW" 2>/dev/null
    [ -z "$LOWER_UP" ] || dmsetup remove "$LOWER" 2>/dev/null
    for l in "$VLOOP" "$LLOOP" "$MLOOP" "$SLOOP"; do [ -z "$l" ] || losetup -d "$l" 2>/dev/null; done
    [ -z "$MH_SAVED" ] || mmp_restore
    [ ! -w "$NOFLUSH" ] || echo 0 > "$NOFLUSH"
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
# K: serve the volume with its request log, the K-th instance on it (1:
# the volume's first); DEV is its device. The instance takes the
# kernel's device over where there is one, else creates it:
# DIR/instances.tsv says which for each pid, from serve's line "serving
# device N (taken over)" or "(added)". With held cuts, its status once it
# is ready goes to DIR/status-start-K.txt (stage 2b). With `acknowledged`
# the serve acknowledges a failure, and instances.tsv says so (stage 2c).
serve_start() {
    local k=$1 ack=${2:-} slog=$RDIR/serve.log _i kind=""
    [ "$k" = 1 ] || slog=$RDIR/serve-$k.log
    local hold=()
    [ "$HOLD" != 1 ] || hold=(--hold-cuts)
    [ -z "$ack" ] || hold+=(--acknowledge-failure)
    hold+=("${SERVE_ARGS[@]}")
    "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" "${hold[@]}" --request-log "$RDIR/requests.tsv" > "$VOL.ready" 2> "$slog" &
    SERVE_PID=$!; DEV=""
    for _i in $(seq 1 480); do
        DEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$VOL.ready" | head -n 1)
        kind=$(sed -n 's/.*serving device [0-9]* (\(taken over\|added\)).*/\1/p' "$slog" | head -n 1)
        [ -z "$DEV" ] || [ ! -b "$DEV" ] || [ -z "$kind" ] || break
        kill -0 "$SERVE_PID" 2>/dev/null || { SERVE_PID=""; rig_error "skyzfsd serve $k ended before it was ready: $(tail -n 3 "$slog")"; }
        sleep 0.25
    done
    [ -n "$DEV" ] && [ -b "$DEV" ] && [ -n "$kind" ] || rig_error "skyzfsd serve $k was not ready, or did not say how, in 120 s"
    printf '%s\t%s%s\n' "$SERVE_PID" "${kind/ /-}" "${ack:+$'\t'acknowledged}" >> "$RDIR/instances.tsv"
    if [ "$HOLD" = 1 ] || [ "$UPLOAD" = 1 ]; then
        for _i in $(seq 1 40); do
            timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$RUN" > "$RDIR/status-start-$k.txt" 2>&1 \
                && grep -q '^cut_state ' "$RDIR/status-start-$k.txt" && break
            sleep 0.25
        done
        grep -q '^cut_state ' "$RDIR/status-start-$k.txt" \
            || rig_error "no status with cut_state 10 s after serve $k was ready: $(tail -n 2 "$RDIR/status-start-$k.txt")"
    fi
    log "$NAME: skyzfsd serve $k on $DEV, the device $kind"
}
# The holder beside the daemon SERVE_PID. Each holder adds to the same
# files and writes onto the same image (cut_hold.py).
holder_start() {
    rm -f "${WORK:?}/hold.pause" "$WORK/hold.pause.held" "$WORK/hold.leave"
    CUT_HOLD_PAUSE="$WORK/hold.pause" CUT_HOLD_LEAVE="$WORK/hold.leave" python3 -I "$HERE/cut_hold.py" "$RDIR" \
        "$SERVE_PID" "$VOL" "$RUN" "$WORK/held.img" $((VDEV_MB << 20)) "$WORK/hold.stop" "$SKYZFSD" 2>> "$RDIR/holder.err" &
    HOLD_PID=$!
}
# Wait for the holder's first written cut: its step ends with the
# release, so a holder asked to leave after this has committed one, and
# the intent map is taken up at the next start (with no committed cut the
# daemon begins it anew, which the check then allows). -> 1 where the
# holder ended with a verdict before it wrote one (the daemon refused to
# read the first cut): the recording then ends there, with the pool
# exported, and the check fails it (held_reads). A holder that ended
# otherwise is the rig's error.
holder_wrote_one() {
    local _i
    for _i in $(seq 1 300); do
        [ -s "$RDIR/held.tsv" ] && return 0
        kill -0 "$HOLD_PID" 2>/dev/null || { holder_gone "before it wrote a cut"; return 1; }
        sleep 1
    done
    rig_error "the holder wrote no cut in 300 s: $(tail -n 2 "$RDIR/holder.log" 2>/dev/null)"
}
# The holder ends between two steps and drains nothing: what the daemon
# holds stays held for the next instance's holder.
holder_leave() {
    local _i
    touch "$WORK/hold.leave"
    for _i in $(seq 1 720); do kill -0 "$HOLD_PID" 2>/dev/null || break; sleep 1; done
    if kill -0 "$HOLD_PID" 2>/dev/null; then
        kill "$HOLD_PID" 2>/dev/null; wait "$HOLD_PID" 2>/dev/null || true; HOLD_PID=""
        rig_error "the holder did not leave 720 s after it was asked: $(tail -n 2 "$RDIR/holder.log" 2>/dev/null)"
    fi
    rm -f "$WORK/hold.leave"
    holder_ended "when asked to leave"
}
# The holder has ended while the recording waits on it (WHEN): with its
# verdict (1 and a VERDICT line) the recording ends there and goes to the
# check; any other end is the rig's error, 0 as well, since it was not
# asked to leave.
holder_gone() {
    holder_ended "$1"
    grep -q "$(printf '\tVERDICT ')" "$RDIR/holder.log" \
        || rig_error "the holder ended with 0 $1: $(tail -n 2 "$RDIR/holder.log" 2>/dev/null)"
}
# The holder has ended (WHEN says when): take its status. 1 with a VERDICT
# line in its log is the daemon refusing to read a cut it held, which the
# check fails (held_reads), so the recording goes on to the check; any
# other status but 0 is the rig's error. Wherever a holder ends, its
# status goes through here: a bare wait under set -e would end the
# recording at a verdict as the rig's error.
holder_ended() {
    local rc=0
    wait "$HOLD_PID" 2>/dev/null || rc=$?
    HOLD_PID=""
    if [ "$rc" = 1 ] && grep -q "$(printf '\tVERDICT ')" "$RDIR/holder.log"; then
        log "$NAME: the holder ended with a verdict $1: $(grep "$(printf '\tVERDICT ')" "$RDIR/holder.log" | tail -n 1)"
        return 0
    fi
    [ "$rc" = 0 ] || rig_error "the holder ended with $rc $1: $(tail -n 2 "$RDIR/holder.log" 2>/dev/null)"
}
# Node B's COMMAND on the recording's volume in object storage.
nodeb() { local c=$1; shift; timeout 300 "$NODEB" "$c" --endpoint "$S3_ENDPOINT" --bucket "$S3_BUCKET" --volume "cut-$NAME" "$@"; }
# The watcher beside the daemon, node B's side of the uploads.
watcher_start() {
    rm -f "${WORK:?}/watch.stop"
    CUT_WATCH_PAUSE="$WORK/watch.pause" python3 -I "$HERE/cut_watch.py" "$RDIR" "$NODEB" "$S3_ENDPOINT" "$S3_BUCKET" \
        "cut-$NAME" "$WORK/watch.stop" 2>> "$RDIR/watch.err" &
    WATCH_PID=$!
}
# After the export: wait up to 300 s for the daemon to upload what it
# holds (drain), then let the watcher fetch head once more. Its
# verdict (1 and a VERDICT line: node B could not read head's content)
# goes to the check; any other end but 0 is the rig's error.
watcher_end() {
    local _i rc=0
    drain drain.txt
    touch "$WORK/watch.stop"
    for _i in $(seq 1 600); do kill -0 "$WATCH_PID" 2>/dev/null || break; sleep 1; done
    if kill -0 "$WATCH_PID" 2>/dev/null; then
        kill "$WATCH_PID" 2>/dev/null; wait "$WATCH_PID" 2>/dev/null || true; WATCH_PID=""
        rig_error "the watcher did not end 600 s after the stop: $(tail -n 2 "$RDIR/watch.log" 2>/dev/null)"
    fi
    wait "$WATCH_PID" 2>/dev/null || rc=$?
    WATCH_PID=""
    if [ "$rc" = 1 ] && grep -q "$(printf '\tVERDICT ')" "$RDIR/watch.log"; then
        log "$NAME: the watcher ended with a verdict: $(grep "$(printf '\tVERDICT ')" "$RDIR/watch.log" | tail -n 1)"
    elif [ "$rc" != 0 ]; then
        rig_error "the watcher ended with $rc: $(tail -n 2 "$RDIR/watch.log" 2>/dev/null)"
    fi
}
# FILE [STATE [HELD]]: up to 300 s for the uploader to come to rest, its
# status then in DIR/FILE with a first line `seconds N`: `upload_state`
# STATE (DRAIN_STATE, idle unless a course says otherwise) and, unless
# HELD (DRAIN_HELD) is empty, no cut held. One that does not is the
# check's to judge.
drain() {
    local _i st="" t0=$SECONDS want=${2:-$DRAIN_STATE} held=${3-$DRAIN_HELD}
    for _i in $(seq 1 300); do
        st=$(timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$RUN" 2>&1) || st=""
        grep -qx "upload_state $want" <<< "$st" && { [ -z "$held" ] || grep -qx 'cuts_held 0' <<< "$st"; } && break
        sleep 1
    done
    { echo "seconds $((SECONDS - t0))"; printf '%s\n' "$st"; } > "$RDIR/$1"
}
# The volume, served by the daemon with its request log.
begin_daemon() {
    local place=()
    rm -rf "${VOL:?}" "$VOL.ready"; mkdir -p "$RUN"
    if [ "$UPLOAD" = 1 ]; then
        nodeb bucket > "$RDIR/bucket.log" 2>&1 || rig_error "node B could not make the bucket: $(tail -n 2 "$RDIR/bucket.log")"
        place=(--endpoint "$S3_ENDPOINT" --bucket "$S3_BUCKET" --volume "cut-$NAME")
    fi
    echo "${CREATE_ARGS[*]}" > "$RDIR/create-args.txt"
    timeout 90 "$SKYZFSD" create --dir "$VOL" --size $((VDEV_MB << 20)) --run-dir "$RUN" "${CREATE_ARGS[@]}" "${place[@]}" > "$RDIR/create.log" 2>&1 \
        || rig_error "skyzfsd create failed: $(tail -n 2 "$RDIR/create.log")"
    # A course's PRE_SERVE may pause the watcher (unostore): the pause
    # holds until the course lifts it, past the watcher's start.
    rm -f "${WORK:?}/watch.pause"
    [ -z "$PRE_SERVE" ] || "$PRE_SERVE"
    serve_start 1
    : > "$RDIR/marks.tsv"
    if [ "$HOLD" = 1 ]; then
        rm -f "${WORK:?}/held.img" "$WORK/held.img.blocks" "$WORK/hold.stop"
        holder_start
    fi
    [ "$UPLOAD" != 1 ] || watcher_start
    log "$NAME: recording through skyzfsd on $DEV ($VDEV_MB MiB)"
}
end_daemon() {
    local _i
    if [ -n "$HOLD_PID" ]; then
        # The holder records what is held beside the status, then
        # writes and releases every cut still held (its drain is bound
        # to 600 s; the check reads its log).
        touch "$WORK/hold.stop"
        for _i in $(seq 1 720); do kill -0 "$HOLD_PID" 2>/dev/null || break; sleep 1; done
        if kill -0 "$HOLD_PID" 2>/dev/null; then
            kill "$HOLD_PID" 2>/dev/null; wait "$HOLD_PID" 2>/dev/null || true; HOLD_PID=""
            rig_error "the holder did not end 720 s after the stop: $(tail -n 2 "$RDIR/holder.log" 2>/dev/null)"
        fi
        holder_ended "at the stop"
        rm -f "${WORK:?}/held.img" "$WORK/held.img.blocks" "$WORK/hold.stop"
    fi
    [ -z "$WATCH_PID" ] || watcher_end
    # The daemon rewrites its status at most every 100 ms, when a counter
    # changed: a second after the export it holds the last cut. The times
    # around the read bound, with the status's age_ms, when it was written.
    sleep 1
    local before after
    before=$(python3 -c 'import time; print(time.monotonic_ns())')
    timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$RUN" > "$RDIR/status.txt" 2>&1 \
        || rig_error "skyzfsd status failed: $(tail -n 2 "$RDIR/status.txt")"
    after=$(python3 -c 'import time; print(time.monotonic_ns())')
    printf '%s\t%s\n' "$before" "$after" > "$RDIR/status-time.tsv"
    # serve ends on SIGTERM, with 0; detach then deletes the device.
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGTERM"
    [ "$SERVE_RC" = 0 ] || rig_error "skyzfsd serve ended with $SERVE_RC on SIGTERM: $(tail -n 2 "$RDIR/serve.log")"
    timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > "$RDIR/detach.log" 2>&1 \
        || rig_error "skyzfsd detach failed: $(tail -n 2 "$RDIR/detach.log")"
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
    local ub dvas d off asize ss n=0
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
        # In the device's logical blocks: a direct write of less is
        # refused (EINVAL) where they are 4 KiB, as skyzfsd's are.
        ss=$(blockdev --getss "$DEV")
        "$DD" if=/dev/zero of="$DEV" bs="$ss" seek=$((off / ss)) count=$((asize / ss)) oflag=direct conv=notrunc status=none
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
# bound (held cuts only): the holder keeps the first cut, which maps the
# whole volume of a new state, while the pool writes 320 MiB of new
# data. The volume is written through at its create (--zero-fill), so
# that it has no holes: every block first written after the cut is
# copied, the copies pass the bound (1/256 of the volume, at least
# 256 MiB), and the cut is abandoned: its map passes to the cut that
# waits. On a sparse volume a daemon that keeps the image's holes
# (M2.4e) copies no write into a hole and reads the hole as zeros:
# boundsparse, the same writes on a volume made sparse. The status is
# read once the writes are synced and before the holder lets the first
# cut go (DIR/status-bound-held.txt): the copies are judged there, while
# the cut that made them is held (cut_daemon_check.py, cut_copied_held).
# The status file shows a change within 200 ms (one 100 ms interval and
# a write), so the read waits a little longer than that after the last
# sync. The holder is paused before the create, so that the first cut is
# the one it holds.
rec_bound() {
    local k
    touch "$WORK/hold.pause"
    create_pool
    for k in $(seq 1 10); do
        dd if=/dev/urandom of="$MNT/b$k" bs=1M count=32 status=none
        z 60 zpool sync "$POOL"
    done
    sleep 0.3
    ustatus status-bound-held.txt
    rm -f "$WORK/hold.pause"
    load 2
    export_pool
}
rec_boundsparse() { rec_bound; }

# The holder paused with no cut held: a cut taken from here on is the
# one being uploaded, and stays held until hold_go. The pool is synced
# and left idle; the holder writes what it holds, and is paused again
# until it says it holds nothing (cut_hold.py writes hold.pause.held).
# Returns 1 where the holder ended with its verdict (holder_gone).
hold_idle() {
    local k t
    z 60 zpool sync "$POOL"
    for k in $(seq 1 20); do
        rm -f "$WORK/hold.pause" "$WORK/hold.pause.held"
        sleep 5
        touch "$WORK/hold.pause"
        for t in $(seq 1 600); do
            [ -e "$WORK/hold.pause.held" ] && break
            kill -0 "$HOLD_PID" 2>/dev/null || { holder_gone "while it was asked to pause"; return 1; }
            sleep 0.2
        done
        [ -e "$WORK/hold.pause.held" ] || rig_error "the holder did not say it had paused in 120 s"
        [ -s "$WORK/hold.pause.held" ] || return 0
    done
    rig_error "the holder still held cuts after 20 pauses on an idle pool: $(cat "$WORK/hold.pause.held")"
}
hold_go() { rm -f "$WORK/hold.pause" "$WORK/hold.pause.held"; }

# edges (held cuts only): requests ZFS does not send, into the area
# between the front labels and the first metaslab (512 KiB to 4 MiB,
# which ZFS leaves alone): a write, write-zeroes over half of it, a
# discard over a quarter, a write of one block. The device's logical
# block is 4 KiB (skyzfs-ublk-sys's parameters), so no request covers
# part of a block. The holder is paused with nothing held before the
# write, and the sync after it takes a cut that stays held, as the one
# being uploaded, across the write-zeroes, the discard and the last
# write: written after them, it must hold the first write's data where
# the home now has zeros (the review of #103, S5). Loads between the
# others make cuts that wait and are replaced.
rec_edges() {
    local base=$((1 << 20))
    create_pool
    load 1
    hold_idle || { export_pool; return 0; }
    "$DD" if=/dev/urandom of="$DEV" bs=64K count=4 seek=$((base >> 16)) oflag=direct conv=notrunc status=none
    z 60 zpool sync "$POOL"
    load 1
    z 60 blkdiscard -f -z -o "$base" -l $((128 << 10)) "$DEV"
    load 1
    z 60 blkdiscard -f -o $((base + (128 << 10))) -l $((64 << 10)) "$DEV"
    load 1
    "$DD" if=/dev/urandom of="$DEV" bs=4K count=1 seek=$(( (base + (192 << 10)) >> 12 )) oflag=direct conv=notrunc status=none
    hold_go
    load 1
    export_pool
}

# takeover (held cuts only; stage 2b): the daemon started again on the
# volume three times, its holder handed on each time (it leaves, and a
# new one starts beside the next instance):
# (a) killed with -9 while the pool writes and syncs, and taken over;
# (b) killed with -9 on an idle pool with a mark waiting (a group synced
#     with zfs_nocacheflush: its uberblocks applied, no flush after
#     them), and taken over: the first flush after carries the mark's
#     cut (design §5.2, "leaves the mark to the next instance");
# (c) a mark waiting again, the pool exported, the daemon stopped and
#     its device deleted, then served again: a created device, where
#     the mark is gone ("a mark does not outlive the attachment").
# The marks noflush-begin and noflush-end bound the groups synced with
# no flush, which the check leaves out of the sessions where the
# reference does not cut them.
noflush() { mark noflush-begin; echo 1 > "$NOFLUSH"; }
flushing() { echo 0 > "$NOFLUSH"; mark noflush-end; }
kill_serve() {
    kill -9 "$SERVE_PID" 2>/dev/null || rig_error "kill -9 $SERVE_PID failed: the daemon had ended ($(tail -n 2 "$RDIR"/serve*.log | tr '\n' ' '))"
    wait "$SERVE_PID" 2>/dev/null || true
    SERVE_PID=""
}
rec_takeover() {
    local w k g0 n
    [ -w "$NOFLUSH" ] || rig_error "no $NOFLUSH to write"
    create_pool
    load 2
    # (a)
    holder_wrote_one || { export_pool; return 0; }
    holder_leave
    g0=$(committed | tail -n 1)
    [ -n "$g0" ] || rig_error "no group committed in $POOL's txgs before the writer"
    # Files of 1 MiB, each synced, until the stop file (at most 2000).
    rm -f "$WORK/writer.stop"
    ( k=0
      while [ ! -e "$WORK/writer.stop" ] && [ "$k" -lt 2000 ]; do
          k=$((k + 1))
          dd if=/dev/urandom of="$MNT/t$((k % 40))" bs=64K count=16 status=none
          zpool sync "$POOL"
      done
      echo "$k files" ) > "$RDIR/takeover-writer.log" 2>&1 &
    w=$!
    # The kill waits for 40 groups that wrote after the holder left,
    # whatever the runner's speed: the held cut's own slot of the
    # uberblock ring (txg mod 31) is written again 31 such groups after
    # it while they come at an even step (31 is prime), and
    # held_across_start's floor needs a block of the held cut changed
    # before the kill (the review of #115, S1; at 3 s the slowest run
    # went 95 cuts past it). Groups that wrote nothing write no
    # uberblock and carry no cut: here two of every three are empty.
    n=0
    for k in $(seq 1 600); do
        n=$(groups_written | awk -v g="$g0" '$1 > g' | wc -l)
        [ "$n" -ge 40 ] && break
        kill -0 "$w" 2>/dev/null || break
        sleep 0.5
    done
    kill -0 "$w" 2>/dev/null || rig_error "the writer had ended before the kill: $(tail -n 2 "$RDIR/takeover-writer.log")"
    [ "$n" -ge 40 ] || rig_error "$POOL wrote $n groups in 300 s after the holder left; the kill waits for 40"
    log "takeover: the kill after $n groups written since the holder left (the newest committed then $g0)"
    kill_serve
    serve_start 2
    holder_start
    sleep 3
    touch "$WORK/writer.stop"
    for k in $(seq 1 300); do kill -0 "$w" 2>/dev/null || break; sleep 1; done
    if kill -0 "$w" 2>/dev/null; then
        kill -9 "$w" 2>/dev/null
        rig_error "the writer did not end 300 s after the take-over"
    fi
    wait "$w" || rig_error "the writer failed across the take-over: $(tail -n 2 "$RDIR/takeover-writer.log")"
    load 1
    # (b)
    holder_leave
    z 60 zpool sync "$POOL"
    noflush
    dd if=/dev/urandom of="$MNT/nf1" bs=64K count=16 status=none
    z 60 zpool sync "$POOL"
    kill_serve
    serve_start 3
    flushing
    holder_start
    load 2
    # (c)
    holder_leave
    noflush
    dd if=/dev/urandom of="$MNT/nf2" bs=64K count=16 status=none
    z 60 zpool sync "$POOL"
    export_pool
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGTERM"
    [ "$SERVE_RC" = 0 ] || rig_error "skyzfsd serve ended with $SERVE_RC on SIGTERM: $(tail -n 2 "$RDIR/serve-3.log")"
    timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > "$RDIR/detach-3.log" 2>&1 \
        || rig_error "skyzfsd detach failed: $(tail -n 2 "$RDIR/detach-3.log")"
    serve_start 4
    flushing
    holder_start
    import_pool
    load 2
    export_pool
    awk -F '\t' '{ print $2 }' "$RDIR/instances.tsv" | tr '\n' ' ' | grep -qx 'added taken-over taken-over added ' \
        || rig_error "the instances were $(cut -f 2 "$RDIR/instances.tsv" | tr '\n' ' '), not added, taken over twice, added"
}

# failed (held cuts only; stage 2c): the volume fails with cuts held and
# no client holding its device (the pool exported), as a failed sync of
# the image would leave it; the test build's SIGUSR1 fails it. The
# instance that saw the failure ends with 3, and the cuts it held go
# with it (design §5.2 as of d26841d: uploading them is M2.5's). A start
# without --acknowledge-failure is refused with 4; one with it trusts
# none of the cut state and begins a new one, taking the intent map up,
# and its holder writes its cuts onto the image the first state's
# holder left, as an upload onto the newest committed cut. What the
# check judges of the failure goes to DIR/failure.tsv.
rec_failed() {
    local rc=0
    create_pool
    load 2
    holder_wrote_one || { export_pool; return 0; }
    holder_leave
    load 1
    export_pool
    # Nobody holds the device: no udev reader of the export's last
    # writes, and the status a second after them.
    udevadm settle -t 30 > /dev/null 2>&1 || true
    sleep 1
    timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$RUN" > "$RDIR/status-failure.txt" 2>&1 || true
    printf 'cuts_held\t%s\n' "$(sed -n 's/^cuts_held //p' "$RDIR/status-failure.txt" | head -n 1)" > "$RDIR/failure.tsv"
    kill -USR1 "$SERVE_PID" 2>/dev/null || rig_error "SIGUSR1 to $SERVE_PID failed: the daemon had ended"
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGUSR1"
    printf 'failed_rc\t%s\n' "$SERVE_RC" >> "$RDIR/failure.tsv"
    # Without the acknowledgement, and without the request log: a refused
    # start is no instance of the recording. One that served instead is
    # stopped at the limit and named by its status.
    timeout 60 "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" --hold-cuts > "$RDIR/serve-refused.out" \
        2> "$RDIR/serve-refused.log" || rc=$?
    printf 'refused_rc\t%s\n' "$rc" >> "$RDIR/failure.tsv"
    serve_start 2 acknowledged
    holder_start
    import_pool
    load 2
    export_pool
    awk -F '\t' '{ print $2 $3 }' "$RDIR/instances.tsv" | tr '\n' ' ' | grep -qx 'added addedacknowledged ' \
        || rig_error "the instances were $(cut -f 2,3 "$RDIR/instances.tsv" | tr '\n' ' '), not added, then added acknowledging the failure"
}

# The upload courses (CUT_UPLOAD=1 only; M2.4c, design §5.2 and the
# notes' "M2.4's shape"). Another writer is node B with the commands of
# bins/skyzfs-nodeb: what it puts at head or in the cut's row goes to
# DIR/others.tsv (`head HEX`, `row STATE NUMBER GROUP`), so that the
# check judges the content only of the cuts the daemon uploaded. The
# check judges each course from the files named below and the status at
# the end; a count the daemon does not reach in time is the check's
# verdict, not the rig's error.

# The status into DIR/FILE, and the value of KEY in it.
ustatus() {
    timeout 30 "$SKYZFSD" status --dir "$VOL" --run-dir "$RUN" > "$RDIR/$1" 2>&1 \
        || rig_error "skyzfsd status failed: $(tail -n 2 "$RDIR/$1")"
}
skey() { sed -n "s/^$2 //p" "$RDIR/$1" | head -n 1; }
# Node B's row of the volume (head, the cut's row, the lease) into DIR/FILE.
nbrow() { nodeb row > "$RDIR/$1" 2> "$RDIR/$1.err" || rig_error "node B row failed: $(tail -n 2 "$RDIR/$1.err")"; }
# The cut the row in DIR/FILE names: `STATE NUMBER GROUP`, or nothing.
rowcut() { awk -F '\t' '$1 == "row" && NF == 4 && $3 ~ /^[0-9]+$/ { print $2, $3, $4 }' "$RDIR/$1"; }
# FILE KEY N SECONDS: write 1 MiB and sync the pool, so that it cuts,
# until the status's KEY is at least N, for up to SECONDS; the status
# then in DIR/FILE. 1 when it was not, which the check judges.
nudge_until() {
    local t0=$SECONDS v="" k=0
    while [ $((SECONDS - t0)) -lt "$4" ]; do
        ustatus "$1"; v=$(skey "$1" "$2")
        [[ $v =~ ^[0-9]+$ ]] && [ "$v" -ge "$3" ] && return 0
        k=$((k + 1))
        dd if=/dev/urandom of="$MNT/n$((k % 8))" bs=64K count=16 status=none
        z 60 zpool sync "$POOL"
        sleep 1
    done
    log "$NAME: the status's $2 was ${v:-nothing}, not at least $3, $4 s on"
    return 1
}
# The volume's state and its newest uploaded cut before another writer
# acts: DIR/STEP-status.txt after a sync of the pool (where it is
# imported) and a drain, DIR/STEP-row.txt. Then up to 120 s for the
# watcher to have fetched that cut under that head: a commit, the row's
# read and the other writer's act can all fall between two of its looks
# (run 44253158409, umoved), and the cuts after it are not the daemon's
# to upload. Every 5 s node B reads the row again (DIR/STEP-now.txt):
# where head or the row has moved on, the daemon cut after the look (a
# sync of data the pool still held: run 44280164949, urow), the watcher
# can no longer fetch that cut under that head, and the look is taken
# again, up to five times, each a line in DIR/looks-again.txt. The
# status is read before the row, and the daemon rewrites it at most once
# in 100 ms: just after a sync it can still say idle, nothing held, one
# upload before the row's (the review of #119, F1). Where its cut_group
# is older than the row's group, the look is taken again too.
before_other() {
    local st n g h cg now _i _k
    for _k in 1 2 3 4 5; do
        if zpool list -H "$POOL" > /dev/null 2>&1; then z 60 zpool sync "$POOL"; fi
        drain "$1-status.txt" idle 1
        nbrow "$1-row.txt"
        read -r st n g <<< "$(rowcut "$1-row.txt")"
        [ -n "$st" ] || rig_error "no cut in the row before $1: $(tr '\t\n' '  ' < "$RDIR/$1-row.txt")"
        h=$(awk -F '\t' '$1 == "head" { print $2 }' "$RDIR/$1-row.txt")
        cg=$(skey "$1-status.txt" cut_group)
        if [[ $cg =~ ^[0-9]+$ ]] && [ "$cg" -lt "$g" ]; then
            log "$NAME: the status before $1 was older than its row (cut group $cg, the row's $g): again"
            printf '%s\t%s\t%s\n' "$1" "$h $st $n $g" "status at group $cg" >> "$RDIR/looks-again.txt"
            # The daemon rewrites its status within 100 ms; with the
            # pool exported there is no sync to wait for, so wait here.
            sleep 1
            continue
        fi
        now=""
        for _i in $(seq 1 240); do
            awk -F '\t' -v n="$n" -v g="$g" -v s="$st" -v h="$h" '$1 == n && $2 == g && $3 == s && $4 == h { f = 1 }
                END { exit !f }' "$RDIR/uploaded.tsv" 2>/dev/null && return 0
            if [ $((_i % 10)) = 0 ]; then
                nbrow "$1-now.txt"
                now="$(awk -F '\t' '$1 == "head" { print $2 }' "$RDIR/$1-now.txt") $(rowcut "$1-now.txt")"
                [ "$now" = "$h $st $n $g" ] || break
                now=""
            fi
            sleep 0.5
        done
        [ -n "$now" ] || rig_error "the watcher had not fetched cut $n (group $g) under head $h 120 s after $1's row named it"
        log "$NAME: the daemon cut after the look before $1 (cut $n, then $now): again"
        printf '%s\t%s\t%s\n' "$1" "$h $st $n $g" "$now" >> "$RDIR/looks-again.txt"
    done
    rig_error "the row or the status moved on after the look before $1 five times in a row"
}

# umoved (b): another writer moves head. The uploader finds head's group
# not the row's (head_moved), abandons what it holds and counts it, and
# never moves head back: nothing it holds afterwards is uploaded, and
# head stays where the other writer put it.
rec_umoved() {
    create_pool
    load 2
    other_act moved act_move
    nudge_until moved-after.txt upload_mismatches 1 120 || true
    load 2
    export_pool
    nbrow end-row.txt
}

# urow (b): another writer puts a row of another state on head's group,
# then one of the daemon's own state with a higher number than it
# committed (the notes' Q8). Each is a mismatch (not_the_base), and the
# next cut is whole: it uploads, so the daemon's state is at head again.
rec_urow() {
    create_pool
    load 2
    other_act row1 act_row1
    nudge_until row1-after.txt upload_mismatches 1 120 || true
    nudge_until row1-whole.txt uploaded_whole 2 120 || true
    load 1
    other_act row2 act_row2
    nudge_until row2-after.txt upload_mismatches 2 120 || true
    nudge_until row2-whole.txt uploaded_whole 3 120 || true
    load 1
    export_pool
    nbrow end-row.txt
}
# uv1: another writer writes the daemon's row again in version 1 (F1):
# the same cut, no commit, head left alone. A row of version 1 does not
# say that head is its commit, so the next delta is a mismatch (row_v1,
# #134) and the next cut goes up whole, writing a row of version 2 that
# names its commit. Then (v1in) it moves head to a commit of its own in
# the row's group and leaves the row in version 1, in one transaction:
# a row of version 1 cannot show that move, so the mismatch is row_v1
# again (the review of #134), and the next whole cut goes up onto that
# commit. Node B's set-row v1:S:N:G and move-head --row write them; a
# node B without those forms fails the act, which is the rig's error.
rec_uv1() {
    create_pool
    load 2
    other_act v1 act_v1
    nudge_until v1-after.txt upload_mismatches 1 120 || true
    nudge_until v1-whole.txt uploaded_whole 2 120 || true
    load 1
    other_act v1in act_v1in
    nudge_until v1in-after.txt upload_mismatches 2 120 || true
    nudge_until v1in-whole.txt uploaded_whole 3 120 || true
    load 1
    export_pool
    nbrow end-row.txt
}
# uv1moved: as uv1's second act, but head moves to a commit of another
# group. That a row of version 1 can show: the mismatch is head_moved,
# and as in umoved the uploader uploads nothing more and leaves head
# where the other writer put it.
rec_uv1moved() {
    create_pool
    load 2
    other_act v1moved act_v1out
    nudge_until v1moved-after.txt upload_mismatches 1 120 || true
    load 2
    export_pool
    nbrow end-row.txt
}
# STEP ACT: the other writer's act on the row it read. before_other STEP
# reads the row once the daemon is at rest; ACT STATE NUMBER GROUP acts
# on that row with node B's --expect-row and returns 6 where the row had
# moved since (the daemon cut between the look and the act, which made
# the act land on a group that was no longer head's: the review of #119,
# G2). Then both are done again, up to five times; each time is a line
# in DIR/acts-again.txt.
other_act() {
    local st n g rc _i
    for _i in 1 2 3 4 5; do
        before_other "$1"
        read -r st n g <<< "$(rowcut "$1-row.txt")"
        # The guard's control (the review of #119, F2): the same act on
        # a row node B did not read must end with 6 and change nothing,
        # whether or not the daemon cuts meanwhile. Its group is 0, which
        # no cut has, so that it cannot meet the daemon's next row.
        rc=0
        "$2" "$st" "$n" 0 || rc=$?
        [ "$rc" = 6 ] || rig_error "node B's act at $1 on the row $st:$n:0, which it did not read, ended with $rc, not 6: the guard is not wired"
        printf '%s\t%s\n' "$1" "$st:$n:0" >> "$RDIR/guard-controls.txt"
        rc=0
        "$2" "$st" "$n" "$g" || rc=$?
        [ "$rc" = 6 ] || return 0
        log "$NAME: the daemon cut between the look before $1 and node B's act: again"
        printf '%s\n' "$1" >> "$RDIR/acts-again.txt"
    done
    rig_error "node B's act at $1 found the row moved five times in a row"
}
# node B's COMMAND ARGS on the row STATE:NUMBER:GROUP, its output in
# DIR/FILE: 0, or 6 where the row had moved; anything else is the rig's
# error.
guarded() { # file row command args...
    local f=$1 row=$2 rc=0
    shift 2
    nodeb "$@" --expect-row "$row" > "$RDIR/$f" 2>&1 || rc=$?
    [ "$rc" = 0 ] || [ "$rc" = 6 ] || rig_error "node B $1 failed: $(tail -n 2 "$RDIR/$f")"
    return "$rc"
}
# umoved's act: head moves to a commit of node B's, with a group the
# daemon never made; the row stays.
act_move() {
    guarded moved.txt "$1:$2:$3" move-head --at 0 --group $(($3 + 1000)) || return 6
    awk -F '\t' '$1 == "head" { print "head\t" $2 }' "$RDIR/moved.txt" >> "$RDIR/others.tsv"
}
# urow's acts: a row of another state on head's group, then one of the
# daemon's state with a number five past the one it committed.
act_row1() { other_row "$(printf '%032x' $((0x5eed)))" "$2" "$3" "$1:$2:$3"; }
act_row2() { other_row "$1" $(($2 + 5)) "$3" "$1:$2:$3"; }
# uv1's act: the row node B read, written again in version 1.
act_v1() {
    guarded set-row.txt "$1:$2:$3" set-row --row "v1:$1:$2:$3" || return 6
    v1_said set-row.txt "v1:$1:$2:$3"
    printf 'v1\t%s\t%s\t%s\n' "$1" "$2" "$3" >> "$RDIR/others.tsv"
}
# uv1's second act and uv1moved's: head moves to a commit of node B's
# in the row's group (v1in) or another (v1out), and the row read is
# left in version 1, in one transaction.
act_v1in() { v1_move "$1" "$2" "$3" "$3"; }
act_v1out() { v1_move "$1" "$2" "$3" $(($3 + 1000)); }
v1_move() { # state number group head's-group
    guarded move-row.txt "$1:$2:$3" move-head --at 0 --group "$4" --row "v1:$1:$2:$3" || return 6
    v1_said move-row.txt "v1:$1:$2:$3"
    awk -F '\t' '$1 == "head" { print "head\t" $2 }' "$RDIR/move-row.txt" >> "$RDIR/others.tsv"
    printf 'v1\t%s\t%s\t%s\n' "$1" "$2" "$3" >> "$RDIR/others.tsv"
}
v1_said() { # file row
    grep -qx $'row_commit\tnone' "$RDIR/$1" \
        || rig_error "node B's write of $2 did not say row_commit none: $(tr '\t\n' '  ' < "$RDIR/$1")"
}
other_row() { # state number group expected-row
    guarded set-row.txt "$4" set-row --row "$1:$2:$3" || return 6
    printf 'row\t%s\t%s\t%s\n' "$1" "$2" "$3" >> "$RDIR/others.tsv"
}

# ukill (c): the daemon kills itself after the third commit's swap has
# landed and before its cut is let go (--kill-at-upload swap:3, the test
# hooks' build), with a writer on the pool. The next instance takes the
# device over and the state up; its uploader finds the row naming the
# cut it holds (rule 1), lets it go and uploads nothing for it.
rec_ukill() {
    local k
    create_pool
    rm -f "$WORK/writer.stop"
    ( k=0
      while [ ! -e "$WORK/writer.stop" ] && [ "$k" -lt 2000 ]; do
          k=$((k + 1))
          dd if=/dev/urandom of="$MNT/k$((k % 40))" bs=64K count=16 status=none
          zpool sync "$POOL"
      done
      echo "$k files" ) > "$RDIR/kill-writer.log" 2>&1 &
    WRITER=$!
    for k in $(seq 1 1200); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$SERVE_PID" 2>/dev/null && rig_error "the daemon had not killed itself 300 s on: $(tail -n 2 "$RDIR/serve.log")"
    serve_end 5 || rig_error "the daemon's pid went away and serve_end did not see it end"
    printf 'killed_rc\t%s\n' "$SERVE_RC" > "$RDIR/kill.tsv"
    SERVE_ARGS=()
    serve_start 2
    sleep 3
    touch "$WORK/writer.stop"
    for k in $(seq 1 300); do kill -0 "$WRITER" 2>/dev/null || break; sleep 1; done
    if kill -0 "$WRITER" 2>/dev/null; then
        kill -9 "$WRITER" 2>/dev/null
        rig_error "the writer did not end 300 s after the take-over"
    fi
    wait "$WRITER" || rig_error "the writer failed across the kill: $(tail -n 2 "$RDIR/kill-writer.log")"
    WRITER=""
    # The balance's start look (upload_balance), with nothing moving:
    # a look while the cut found committed is let go can catch the
    # uploader's count before the store's.
    drain balance-start.txt idle 1
    load 1
    export_pool
}

# ulease (d): another holder has the lease when the volume is first
# served. A serve is refused with 10 and writes nothing; one with
# --serve-without-upload serves and uploads nothing. Once the other has
# let go, the next serve takes the lease and uploads the pool's cuts,
# its first one whole.
ulease_other() {
    nodeb lease --holder other-node --ttl-s 3600 > "$RDIR/other-lease.txt" 2>&1 \
        || rig_error "node B could not take the lease: $(tail -n 2 "$RDIR/other-lease.txt")"
    local rc=0
    # Without the request log: a refused start is no instance. One that
    # served instead is stopped at the limit. What the start wrote under
    # the volume's and the run's directories goes to DIR/refused-wrote.txt.
    touch "$WORK/refused.before"
    sleep 1
    timeout 60 "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" > "$RDIR/serve-refused.out" \
        2> "$RDIR/serve-refused.log" || rc=$?
    printf 'refused_rc\t%s\n' "$rc" > "$RDIR/lease.tsv"
    find "$VOL" "$RUN" -newer "$WORK/refused.before" > "$RDIR/refused-wrote.txt" 2>&1 || true
    SERVE_ARGS=(--serve-without-upload)
}
rec_ulease() {
    create_pool
    load 2
    export_pool
    ustatus without-status.txt
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGTERM"
    [ "$SERVE_RC" = 0 ] || rig_error "skyzfsd serve ended with $SERVE_RC on SIGTERM: $(tail -n 2 "$RDIR/serve.log")"
    timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > "$RDIR/detach-1.log" 2>&1 \
        || rig_error "skyzfsd detach failed: $(tail -n 2 "$RDIR/detach-1.log")"
    # Until here head is none at every look of node B's (rows.tsv): the
    # time, on the clock of its lines, that the check reads them up to.
    python3 -I -c 'import time; print(time.monotonic_ns())' > "$RDIR/release-ns.txt"
    nodeb release --holder other-node > "$RDIR/other-release.txt" 2>&1 \
        || rig_error "node B could not let the lease go: $(tail -n 2 "$RDIR/other-release.txt")"
    SERVE_ARGS=()
    serve_start 2
    drain balance-start.txt idle 1
    import_pool
    load 2
    export_pool
}

# ulost (d): the daemon's lease runs out while it is stopped (SIGSTOP,
# a TTL of 3 s), and another holder takes it. Once going again the
# daemon goes on serving and uploads nothing more: head and the row stay
# what they were.
rec_ulost() {
    create_pool
    load 2
    export_pool
    before_other lost
    kill -STOP "$SERVE_PID" || rig_error "SIGSTOP to $SERVE_PID failed"
    sleep 8
    nodeb lease --holder other-node --ttl-s 3600 > "$RDIR/other-lease.txt" 2>&1 || true
    kill -CONT "$SERVE_PID" || rig_error "SIGCONT to $SERVE_PID failed"
    grep -q '^lease	acquired	' "$RDIR/other-lease.txt" \
        || rig_error "node B did not take the lease 8 s into the stop: $(tail -n 2 "$RDIR/other-lease.txt")"
    import_pool
    load 2
    export_pool
    nbrow end-row.txt
}

# unostore (e, by name): object storage is down (CUT_S3_DOWN) when the
# volume is first served. The serve goes on with no lease, its uploader
# in its backoff; once the store is back (CUT_S3_UP) it takes the lease
# and uploads, its first cut whole. The watcher pauses meanwhile.
unostore_down() {
    [ -n "$S3_DOWN" ] && [ -n "$S3_UP" ] || rig_error "unostore needs CUT_S3_DOWN and CUT_S3_UP"
    touch "$WORK/watch.pause"
    bash -c "$S3_DOWN" > "$RDIR/s3-down.log" 2>&1 || rig_error "CUT_S3_DOWN failed: $(tail -n 2 "$RDIR/s3-down.log")"
}
rec_unostore() {
    local _i
    create_pool
    load 2
    sleep 20
    ustatus nostore-status.txt
    bash -c "$S3_UP" > "$RDIR/s3-up.log" 2>&1 || rig_error "CUT_S3_UP failed: $(tail -n 2 "$RDIR/s3-up.log")"
    for _i in $(seq 1 60); do nodeb row > /dev/null 2>&1 && break; sleep 1; done
    rm -f "$WORK/watch.pause"
    load 1
    export_pool
}

# ukillput (f, by name): the daemon kills itself after its fifth segment
# PUT has landed (--kill-at-upload put:5), inside an upload, with a
# writer on the pool. The next instance takes over and uploads the cut
# again; nothing of the first attempt is at head.
rec_ukillput() { rec_ukill; }

# ufailed (f, by name): the volume fails (SIGUSR1, the test build)
# after the pool's export, once node B has fetched the old state's last
# cut. Before the failure, RAW_BLOCKS blocks are written to the device
# itself at 2 MiB, in the area between the front labels and the first
# metaslab that ZFS leaves alone (as rec_edges uses it; the end of the
# device can fall inside the last metaslab): the intent map names them,
# no cut of the old state holds them, and nothing writes them again. The
# instance ends with 3; one that acknowledges the failure begins a new
# state, and its cuts must hold those blocks, whether the first goes up
# whole or as a delta onto the map's base.
RAW_BLOCKS=16
raw_write() {
    local at=$(((2 << 20) >> 12))
    python3 -I -c 'import sys; sys.path.insert(0, sys.argv[1]); import cut_daemon_check as c
sys.stdout.buffer.write(b"".join(c.raw_block(k) for k in range(int(sys.argv[2]))))' "$HERE" "$RAW_BLOCKS" > "$WORK/raw.img"
    "$DD" if="$WORK/raw.img" of="$DEV" bs=4K seek="$at" oflag=direct conv=notrunc,fsync status=none \
        || rig_error "the write of $RAW_BLOCKS blocks to $DEV at block $at failed"
    printf 'raw\t%s\t%s\n' "$at" "$RAW_BLOCKS" > "$RDIR/raw.tsv"
}
rec_ufailed() {
    create_pool
    load 2
    export_pool
    before_other failed
    raw_write
    kill -USR1 "$SERVE_PID" 2>/dev/null || rig_error "SIGUSR1 to $SERVE_PID failed: the daemon had ended"
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGUSR1"
    printf 'failed_rc\t%s\n' "$SERVE_RC" > "$RDIR/failure.tsv"
    serve_start 2 acknowledged
    drain balance-start.txt idle 1
    import_pool
    load 2
    export_pool
}

# The publish courses (the channel rig; design §5.2, "A snapshot is
# published by a cut of the pool's present history"). The agent is
# publish_agent.py, through skyzfsd's commands; every request and answer
# goes to DIR/agent.tsv, each publish asked to DIR/published.tsv, and the
# answers the check judges to DIR/ends.tsv as LABEL and the answer
# (publish_check.py's EXPECTED names the labels).
pagent() { python3 -I "$HERE/publish_agent.py" "$1" "$SKYZFSD" "$VOL" "$RUN" "$RDIR/agent.tsv" "${@:2}"; }
pend() { printf '%s\t%s\n' "$1" "$2" >> "$RDIR/ends.tsv"; }
# LABEL T [OPTION...]: the agent's whole publish of POOL@T; its end.
pflow() {
    local label=$1 t=$2 out rc=0
    shift 2
    out=$(timeout 1200 python3 -I "$HERE/publish_agent.py" flow "$SKYZFSD" "$VOL" "$RUN" "$RDIR/agent.tsv" \
        "$POOL" "$t" "$@" 2>> "$RDIR/agent.err") || rc=$?
    [ "$rc" -le 1 ] || rig_error "the agent's publish of $t ended $rc: $(tail -n 2 "$RDIR/agent.err")"
    pend "$label" "$out"
}
# LABEL REQUEST ARGS...: one request (publish_agent.py ask); its answer.
pask() {
    local label=$1 out rc=0
    shift
    out=$(pagent ask "$@" 2>> "$RDIR/agent.err") || rc=$?
    [ "$rc" -le 1 ] || rig_error "the agent's $1 ($label) ended $rc: $(tail -n 2 "$RDIR/agent.err")"
    pend "$label" "$out"
}
# T K: field K of T's last request in published.tsv (2 G, 3 A, 4 S, 5 C,
# 6 the manifest, 7 the snapshot's createtxg).
preq() { awk -F '\t' -v t="$1" -v k="$2" '$1 == t { v = $k } END { print v }' "$RDIR/published.tsv"; }
# LABEL T G [record]: T asked again as it was last asked, with G; with
# `record`, as the request its publish is judged by.
pagain() {
    local a s c m x
    a=$(preq "$2" 3); s=$(preq "$2" 4); c=$(preq "$2" 5); m=$(preq "$2" 6); x=$(preq "$2" 7)
    [ -n "$a" ] || rig_error "no request of $2 in published.tsv"
    [ "${4:-}" != record ] || printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$2" "$3" "$a" "$s" "$c" "$m" "$x" >> "$RDIR/published.tsv"
    pask "$1" publish "$2" "$a" "$s" "$c" "$3" "$m"
}
# LABEL T SECONDS [sync]: how T stands, asked until it no longer pends
# (`pending after N s` when it still does); with sync, 4 KiB written and
# the pool synced between two looks, so that the pool cuts.
pwait() {
    local t0=$SECONDS out rc
    while :; do
        rc=0
        out=$(pagent ask progress "$2" 2>> "$RDIR/agent.err") || rc=$?
        [ "$rc" -le 1 ] || rig_error "the agent's look at $2 ended $rc: $(tail -n 2 "$RDIR/agent.err")"
        [ "$out" = pending ] || break
        [ $((SECONDS - t0)) -lt "$3" ] || { out="pending after $3 s"; break; }
        if [ "${4:-}" = sync ]; then
            dd if=/dev/urandom of="$MNT/publish-nudge" bs=4K count=1 status=none
            z 60 zpool sync "$POOL"
        fi
        sleep 1
    done
    pend "$1" "$out"
}
# LABEL BYTES: BYTES (printf %b) sent as they are to the socket.
praw() {
    local out rc=0
    out=$(printf '%b' "$2" | python3 -I "$HERE/publish_agent.py" raw "$RUN" "$RDIR/agent.tsv" "$1" 2>> "$RDIR/agent.err") || rc=$?
    [ "$rc" -le 1 ] || rig_error "the raw request $1 ended $rc: $(tail -n 2 "$RDIR/agent.err")"
    pend "$1" "$out"
}

# pplain (P1): publishes on a pool going on as it is. t1 plainly, then
# asked again (asking twice is asking once: done on the same commit),
# then asked with another manifest (accepted with no I/O, then refused
# `taken` by the lookup). t2 asked twice before the first look. t3 of
# another attachment and of another cut state: refused, and never there.
# t4 asked with G 20 above its snapshot's createtxg, so that cuts counted
# after C fall below G and must be passed over (the lead's addition: a
# cut below G is never what snap/t names). t5 can never be carried and
# expires in 5 s. t7 likewise pends; asked with other parameters it is
# refused `asked`; withdrawn; then asked anew with its snapshot's G, and
# published.
rec_pplain() {
    local zero
    zero=$(printf '0%.0s' $(seq 1 32))
    create_pool
    load 2
    pflow t1 t1 --limit-s 300
    pagain t1-again t1 "$(preq t1 2)"
    pwait t1-again-end t1 120
    { cat "$RDIR/manifest-t1"; printf '%064d  another\n' 0; } > "$RDIR/manifest-t1-other"
    pask t1-other publish t1 "$(preq t1 3)" "$(preq t1 4)" "$(preq t1 5)" "$(preq t1 2)" "$RDIR/manifest-t1-other"
    pwait t1-other-end t1 120
    pflow t2 t2 --twice --limit-s 300
    pask att publish t3 "$zero" "$(preq t1 4)" "$(preq t1 5)" "$(preq t1 2)" "$RDIR/manifest-t1"
    pask state publish t3 "$(preq t1 3)" "$zero" "$(preq t1 5)" "$(preq t1 2)" "$RDIR/manifest-t1"
    pflow t4 t4 --group-plus 20 --sync --limit-s 600
    pflow t5 t5 --group-plus 1000000 --expiry-s 5 --limit-s 120
    pflow t7 t7 --group-plus 1000000 --expiry-s 900 --limit-s 3
    pask t7-other publish t7 "$(preq t7 3)" "$(preq t7 4)" "$(preq t7 5)" "$(preq t7 7)" "$(preq t7 6)"
    pask t7-withdraw withdraw t7
    pagain t7-again t7 "$(preq t7 7)" record
    pwait t7-end t7 300 sync
    load 1
    export_pool
}

# prewind (P2): object storage down (CUT_S3_DOWN) while the pool goes
# on, so that the daemon holds the cuts of a history that a rewind then
# abandons (an import at a group a few back, zpool import -T). With the
# store still down, the agent takes r1 and asks; then the store comes
# back (CUT_S3_UP), the daemon uploads the abandoned cuts and the
# present history's, and r1 must be published by a cut of the present
# history: counted after C, of group G or later, opening as itself. The
# agent syncs a group at a time while it waits (after the rewind no cut
# opens as itself until the pool's groups pass the abandoned ones). The
# lease is 900 s, so the outage does not take it away. The status with
# the store down goes to DIR/rewind-status.txt.
rec_prewind() {
    local groups t pid _i
    [ -n "$S3_DOWN" ] && [ -n "$S3_UP" ] || rig_error "prewind needs CUT_S3_DOWN and CUT_S3_UP"
    create_pool
    load 3
    touch "$WORK/watch.pause"
    S3_IS_DOWN=1
    bash -c "$S3_DOWN" > "$RDIR/s3-down.log" 2>&1 || rig_error "CUT_S3_DOWN failed: $(tail -n 2 "$RDIR/s3-down.log")"
    load 3
    mapfile -t groups < <(committed)
    [ "${#groups[@]}" -ge 6 ] || rig_error "only ${#groups[@]} groups committed before the rewind"
    t=${groups[$(( ${#groups[@]} - 4 ))]}
    echo "$t" > "$RDIR/rewind-txg"
    export_pool
    log "prewind: importing at group $t of ${groups[-1]}, object storage down"
    import_pool -T "$t"
    ustatus rewind-status.txt
    pflow r1 r1 --sync --limit-s 900 &
    pid=$!
    sleep 20
    bash -c "$S3_UP" > "$RDIR/s3-up.log" 2>&1 || rig_error "CUT_S3_UP failed: $(tail -n 2 "$RDIR/s3-up.log")"
    S3_IS_DOWN=""
    for _i in $(seq 1 60); do nodeb row > /dev/null 2>&1 && break; sleep 1; done
    rm -f "$WORK/watch.pause"
    wait "$pid"
    load 1
    export_pool
}

# pkill (P3): two publishes pend, each with G 30 above its snapshot's:
# tw is withdrawn, tk is not. The daemon is killed (kill -9) and the next
# instance takes over: neither is known to it (pending publishes are in
# memory). tk asked again is published; tw, never asked again, must read
# withdrawn or unknown, and never be there, although the pool's cuts go
# past its G while tk waits (the lead's addition: a kill after the
# withdrawal was answered and before the publish could land).
rec_pkill() {
    create_pool
    load 1
    pflow tw tw --group-plus 30 --expiry-s 900 --limit-s 3
    pask tw-withdraw withdraw tw
    pflow tk tk --group-plus 30 --expiry-s 900 --limit-s 3
    kill_serve
    serve_start 2
    pask tw-after progress tw
    pask tk-after progress tk
    pagain tk-again tk "$(preq tk 2)" record
    pwait tk-end tk 600 sync
    pask tw-last progress tw
    load 1
    export_pool
}

# pkillman, pkillpub (P3): the daemon kills itself inside its first
# publish (--kill-at-upload): manifest:1 after the snapshot's framed
# manifest landed and before the transaction that makes snap/T,
# publish:1 once that transaction landed and before the uploader knew.
# Node B reads snap/T between the kill and the take-over
# (DIR/T-between.txt). The next instance does not know T; asked again,
# the publish is made (manifest:1) or found (publish:1), done on the
# commit snap/T names.
rec_pkillat() {
    local t=$1 fpid _i
    create_pool
    load 1
    ( rc=0
      out=$(timeout 1200 python3 -I "$HERE/publish_agent.py" flow "$SKYZFSD" "$VOL" "$RUN" "$RDIR/agent.tsv" \
          "$POOL" "$t" --sync --limit-s 600 2>> "$RDIR/agent.err") || rc=$?
      pend "$t" "${out:-none (exit $rc)}" ) &
    fpid=$!
    for _i in $(seq 1 2400); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
    kill -0 "$SERVE_PID" 2>/dev/null && rig_error "the daemon had not killed itself in its first publish 600 s on"
    serve_end 5 || rig_error "the daemon's pid went away and serve_end did not see it end"
    printf 'killed_rc\t%s\n' "$SERVE_RC" > "$RDIR/kill.tsv"
    nodeb snap-row --snapshot "$t" > "$RDIR/$t-between.txt" 2> "$RDIR/$t-between.err" \
        || rig_error "node B snap-row failed: $(tail -n 2 "$RDIR/$t-between.err")"
    SERVE_ARGS=()
    serve_start 2
    for _i in $(seq 1 300); do kill -0 "$fpid" 2>/dev/null || break; sleep 1; done
    kill -0 "$fpid" 2>/dev/null && rig_error "the agent had not ended 300 s after the take-over"
    wait "$fpid" || true
    pask "$t-after" progress "$t"
    pagain "$t-again" "$t" "$(preq "$t" 2)" record
    pwait "$t-end" "$t" 600 sync
    load 1
    export_pool
}
rec_pkillman() { rec_pkillat m1; }
rec_pkillpub() { rec_pkillat p1; }

# pchannel (P4): the channel itself. First served while another holder
# has the lease (ulease_other, --serve-without-upload): `cuts` is refused
# `no-upload`. Then, served with the lease, requests that the daemon's
# commands would not send: another greeting (`version`), a key the
# request does not take (`malformed`), a name no publish can have
# (`name`), a well-formed `cuts` (that the raw path works), and one from
# a peer of another user (`peer`: nobody, with the socket made 0666 and
# the directories above it searchable for the request, and put back).
# The status after them goes to DIR/channel-status.txt.
ppeer() {
    local sock dir tmp out rc=0 m modes=()
    sock=$(python3 -I -c 'import sys; sys.path.insert(0, sys.argv[1]); import publish_agent as p; print(p.socket_path(sys.argv[2]))' \
        "$HERE" "$RUN") || rig_error "no socket under $RUN"
    dir=$(dirname "$sock")
    while [ "$dir" != / ]; do modes+=("$(stat -c %a "$dir") $dir"); chmod o+x "$dir"; dir=$(dirname "$dir"); done
    chmod 0666 "$sock"
    tmp=$(mktemp -d)
    cp "$HERE/publish_agent.py" "$tmp/"
    chmod 0755 "$tmp"; chmod 0644 "$tmp/publish_agent.py"; chown nobody "$tmp"
    out=$(printf 'skyzfs-control 1\ncuts\n' | runuser -u nobody -- python3 -I "$tmp/publish_agent.py" raw "$sock" \
        "$tmp/agent.tsv" peer 2>> "$RDIR/agent.err") || rc=$?
    cat "$tmp/agent.tsv" >> "$RDIR/agent.tsv" 2>/dev/null || true
    rm -rf "${tmp:?}"
    chmod 0600 "$sock"
    for m in "${modes[@]}"; do chmod "${m%% *}" "${m#* }"; done
    [ "$rc" -le 1 ] || rig_error "nobody's request ended $rc: $(tail -n 2 "$RDIR/agent.err")"
    pend peer "$out"
}
rec_pchannel() {
    create_pool
    load 1
    pask nu cuts
    export_pool
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    serve_end 60 || rig_error "skyzfsd serve did not end 60 s after SIGTERM"
    [ "$SERVE_RC" = 0 ] || rig_error "skyzfsd serve ended with $SERVE_RC on SIGTERM: $(tail -n 2 "$RDIR/serve.log")"
    timeout 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" > "$RDIR/detach-1.log" 2>&1 \
        || rig_error "skyzfsd detach failed: $(tail -n 2 "$RDIR/detach-1.log")"
    nodeb release --holder other-node > "$RDIR/other-release.txt" 2>&1 \
        || rig_error "node B could not let the lease go: $(tail -n 2 "$RDIR/other-release.txt")"
    SERVE_ARGS=()
    serve_start 2
    drain balance-start.txt idle 1
    import_pool
    praw version 'skyzfs-control 9\ncuts\n'
    praw malformed 'skyzfs-control 1\ncuts extra=1\n'
    praw name 'skyzfs-control 1\nprogress name=a/b\n'
    praw cuts 'skyzfs-control 1\ncuts\n'
    ppeer
    ustatus channel-status.txt
    load 1
    export_pool
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
    case $1 in
        bound) CREATE_ARGS=(--zero-fill) ;;
        ulease) PRE_SERVE=ulease_other ;;
        ukill) SERVE_ARGS=(--kill-at-upload swap:3) ;;
        ukillput) SERVE_ARGS=(--kill-at-upload put:5) ;;
        unostore) PRE_SERVE=unostore_down ;;
        prewind) SERVE_ARGS=(--lease-ttl-s 900) ;;
        pkillman) SERVE_ARGS=(--kill-at-upload manifest:1) ;;
        pkillpub) SERVE_ARGS=(--kill-at-upload publish:1) ;;
        pchannel) PRE_SERVE=ulease_other ;;
        ulost) SERVE_ARGS=(--lease-ttl-s 3); DRAIN_STATE=lease_lost; DRAIN_HELD="" ;;
    esac
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

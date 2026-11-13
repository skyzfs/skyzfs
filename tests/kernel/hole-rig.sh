#!/usr/bin/env bash
# Power-cut replay of a direct write into a hole, through skyzfsd
# (M1.T27; design §4.1, §4.2, §10's row on the home image's
# allocation). For each file system and each kind of home image this
# records a course of writes below the daemon and checks it at record
# prefixes (tests/kernel/replay_hole.py, which documents the course,
# the checks and the negative control).
#
#   hole-rig.sh OUT [WORK]
#
# The stack while recording, as in replay-rig.sh through skyzfsd but
# with no pool: the course writes the ublk device itself, so that each
# block it writes is one the image has never held:
#
#   course ── /dev/ublkbN ── skyzfsd serve ── volume dir (ext4 | xfs)
#          ── dm-log-writes ── lower disk (a loop file)
#
# Kinds of home image (RIG_HOLE_KINDS):
#   sparse     as `skyzfsd create` makes it: a hole
#   fallocate  the same, then fallocate(1) over the whole image: unwritten
#              extents
#   zero-fill  `skyzfsd create --zero-fill`: written through at create.
#              A daemon without the option has this kind skipped, and the
#              summary says so
# File systems (RIG_HOLE_FS): ext4 and xfs, each made fresh per course.
#
# Knobs (environment): RIG_SKYZFSD (required: a build with its test
# hooks), RIG_SERVE_ARGS (options for every serve; the store is
# always in place, a --store among them is replaced), RIG_HOLE_FS="ext4
# xfs", RIG_HOLE_KINDS="sparse fallocate zero-fill",
# RIG_HOLE_VOLUME_MB=256, RIG_HOLE_WRITES=16, RIG_HOLE_FLUSH_EVERY=3,
# RIG_HOLE_TAIL=2, RIG_HOLE_EVERY_PREFIX=1 (every prefix of each
# course, no time limit) or RIG_HOLE_SAMPLE=40 and RIG_HOLE_BUDGET_S=240
# (the marks' prefixes and this many more at random, in this many
# seconds a course), RIG_BUDGET_NAME, RIG_DAEMON_FROM (what the daemon
# was built from, for the summary).
#
# Exit: 0 every prefix as design §4.1 allows; 1 a verdict against the
# daemon; 2 the rig's own error, not a verdict. The summary is
# OUT/hole-summary.md. Needs root, dm-log-writes, dm-snapshot, mkfs.ext4,
# mkfs.xfs and `ublk` (build-ublksrv.sh), which deletes the devices of
# the check's starts.
set -Eeuo pipefail

HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# shellcheck source=tests/lib/gnu-dd.sh
. "$HERE/../lib/gnu-dd.sh"
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"

OUT=${1:?usage: hole-rig.sh OUT [WORK]}
WORK=${2:-/mnt/skz-hole}
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)

SKYZFSD=${RIG_SKYZFSD:?RIG_SKYZFSD names skyzfsd built with its test hooks}
[ -x "$SKYZFSD" ] || { echo "RIG_SKYZFSD=$SKYZFSD is not an executable" >&2; exit 64; }
read -r -a GIVEN <<< "${RIG_SERVE_ARGS:-}"
# The courses are of the in-place store whatever the options name: a
# --store among them is replaced (with the log store the first record
# that carries a block's data would be the log's, not the image's).
SERVE_ARGS=(--store in-place)
for ((i = 0; i < ${#GIVEN[@]}; i++)); do
    case ${GIVEN[i]} in
        --store) i=$((i + 1)) ;;
        --store=*) ;;
        *) SERVE_ARGS+=("${GIVEN[i]}") ;;
    esac
done
read -r -a FSES <<< "${RIG_HOLE_FS:-ext4 xfs}"
read -r -a KINDS <<< "${RIG_HOLE_KINDS:-sparse fallocate zero-fill}"
VOLUME_MB=${RIG_HOLE_VOLUME_MB:-256}
WRITES=${RIG_HOLE_WRITES:-16}
FLUSH_EVERY=${RIG_HOLE_FLUSH_EVERY:-3}
TAIL=${RIG_HOLE_TAIL:-2}
SAMPLE=${RIG_HOLE_SAMPLE:-40}
BUDGET_S=${RIG_HOLE_BUDGET_S:-240}
for v in "$VOLUME_MB" "$WRITES" "$FLUSH_EVERY" "$TAIL" "$SAMPLE" "$BUDGET_S"; do
    [[ $v =~ ^[0-9]+$ ]] || { echo "not a number: $v" >&2; exit 64; }
done
for f in "${FSES[@]}"; do case $f in ext4|xfs) ;; *) echo "RIG_HOLE_FS: ext4 or xfs, not $f" >&2; exit 64 ;; esac; done
for k in "${KINDS[@]}"; do case $k in sparse|fallocate|zero-fill) ;; *) echo "RIG_HOLE_KINDS: sparse, fallocate or zero-fill, not $k" >&2; exit 64 ;; esac; done
LOWER_MB=$((VOLUME_MB * 4))
LOG_MB=2048
LW=skz-hole-lw
RUN=/run/skz-hole-rec

for m in ublk_drv dm_log_writes dm_snapshot ext4 xfs; do modprobe "$m" 2>/dev/null || true; done
dmsetup targets | grep -q '^log-writes' || { echo "no dm-log-writes target in this kernel" >&2; exit 64; }
dmsetup targets | grep -q '^snapshot ' || { echo "no dm-snapshot target in this kernel" >&2; exit 64; }
command -v ublk >/dev/null || { echo "no ublk command (build-ublksrv.sh)" >&2; exit 64; }
dmsetup remove "$LW" 2>/dev/null || true

log() { echo "== $*" >&2; }
# A command of the rig's own that fails unexpectedly: the rig's error,
# exit 2, not a verdict.
trap 'echo "hole-rig.sh: the rig failed at line $LINENO (status $?): not a verdict on the daemon" >&2; exit 2' ERR

# The state of one course, undone by course_end and by the trap.
SERVE_PID=""; MNT=""; LOWER_LOOP=""; LOG_LOOP=""; LW_UP=""; VOL=""
course_end() {
    if [ -n "$SERVE_PID" ]; then
        kill -9 "$SERVE_PID" 2>/dev/null || true
        lim 60 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" >/dev/null 2>&1 || true
        SERVE_PID=""
    fi
    [ -z "$MNT" ] || { lim 120 umount "$MNT" 2>/dev/null || true; MNT=""; }
    [ -z "$LW_UP" ] || { lim 60 dmsetup remove "$LW" 2>/dev/null || true; LW_UP=""; }
    [ -z "$LOWER_LOOP" ] || { lim 60 losetup -d "$LOWER_LOOP" 2>/dev/null || true; LOWER_LOOP=""; }
    [ -z "$LOG_LOOP" ] || { lim 60 losetup -d "$LOG_LOOP" 2>/dev/null || true; LOG_LOOP=""; }
    rm -rf "$RUN"
    return 0
}
trap course_end EXIT

# A course's result when it ends before its check: the JSON the summary
# reads. WHY is the rig's own error unless VERDICT is set.
course_json() { # fs kind field text
    python3 -I - "$OUT/hole-result-$1-$2.json" "$1" "$2" "$3" "$4" <<'PY'
import json, sys
path, fs, kind, field, text = sys.argv[1:]
r = {'fs': fs, 'kind': kind, 'checked': 0, 'zero_prefixes': 0, 'zero_blocks': 0, 'violations': [],
     'failures': [], 'rig_error': None, 'control': None}
if field == 'skipped':
    r = {'fs': fs, 'kind': kind, 'skipped': text}
elif field == 'failure':
    r['failures'] = [text]
else:
    r['rig_error'] = text
json.dump(r, open(path, 'w'))
PY
}

mkfs_for() { # fs device
    case $1 in
        ext4) lim 300 mkfs.ext4 -q -F "$2" ;;
        xfs) lim 300 mkfs.xfs -q -f "$2" ;;
    esac
}
mount_opts() { if [ "$1" = xfs ]; then echo "-o nouuid"; fi; }

serve_up() { # ready file, log file -> DEV
    : > "$1"
    "$SKYZFSD" serve --dir "$VOL" --run-dir "$RUN" "${SERVE_ARGS[@]}" > "$1" 2>> "$2" &
    SERVE_PID=$!
    for _ in $(seq 1 480); do
        DEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$1" | head -n 1)
        [ -n "$DEV" ] && [ -b "$DEV" ] && return 0
        kill -0 "$SERVE_PID" 2>/dev/null || { SERVE_RC=0; wait "$SERVE_PID" || SERVE_RC=$?; SERVE_PID=""; return 1; }
        sleep 0.25
    done
    SERVE_RC=timeout
    return 1
}
serve_down() {
    local rc=0
    kill -TERM "$SERVE_PID" 2>/dev/null || true
    for _ in $(seq 1 240); do kill -0 "$SERVE_PID" 2>/dev/null || break; sleep 0.25; done
    ! kill -0 "$SERVE_PID" 2>/dev/null || return 1
    wait "$SERVE_PID" || rc=$?
    SERVE_PID=""
    [ "$rc" = 0 ] || return 1
    lim 120 "$SKYZFSD" detach --dir "$VOL" --run-dir "$RUN" >&2
}

# One course: make the lower disk and the volume, record the course,
# check it. Its result is OUT/hole-result-FS-KIND.json.
course() {
    local fs=$1 kind=$2 w=$WORK/$1-$2 create=() mopts
    log "course: $fs, $kind"
    rm -rf "$w"; mkdir -p "$w/mnt" "$w/check"
    if [ "$kind" = zero-fill ]; then
        if ! "$SKYZFSD" help 2>&1 | grep -q -- '--zero-fill'; then
            course_json "$fs" "$kind" skipped "this daemon has no create --zero-fill"
            return 0
        fi
        create=(--zero-fill)
    fi
    # shellcheck disable=SC2207  # one word or none
    mopts=($(mount_opts "$fs"))
    # The lower disk, its file system and the volume, with no recording.
    truncate -s "${LOWER_MB}M" "$w/lower.img"
    LOWER_LOOP=$(losetup --find --show --direct-io=on "$w/lower.img" 2>/dev/null || losetup --find --show "$w/lower.img")
    mkfs_for "$fs" "$LOWER_LOOP" > "$OUT/hole-$fs-$kind-mkfs.log" 2>&1 || { course_json "$fs" "$kind" error "mkfs.$fs failed: $(tail -n 1 "$OUT/hole-$fs-$kind-mkfs.log")"; course_end; return 0; }
    lim 120 mount "${mopts[@]}" "$LOWER_LOOP" "$w/mnt" || { course_json "$fs" "$kind" error "mount of the new $fs failed"; course_end; return 0; }
    MNT=$w/mnt; VOL=$w/mnt/vol
    if ! lim 600 "$SKYZFSD" create --dir "$VOL" --size $((VOLUME_MB << 20)) "${create[@]}" > "$OUT/hole-$fs-$kind-create.log" 2>&1; then
        course_json "$fs" "$kind" failure "skyzfsd create ${create[*]} failed: $(tail -n 1 "$OUT/hole-$fs-$kind-create.log")"
        course_end; return 0
    fi
    if [ "$kind" = fallocate ]; then
        lim 300 fallocate -l $((VOLUME_MB << 20)) "$VOL/home.img" || { course_json "$fs" "$kind" error "fallocate of the home image failed"; course_end; return 0; }
    fi
    # What the image is made of, as the file system says: an observation.
    { echo "# $fs, $kind: filefrag -v of the home image before the course"; lim 60 filefrag -v "$VOL/home.img" | head -n 40; } > "$OUT/hole-$fs-$kind-extents.txt" 2>&1 || true
    lim 120 sync -f "$VOL/home.img" || true
    lim 120 umount "$MNT"; MNT=""
    losetup -d "$LOWER_LOOP"; LOWER_LOOP=""
    cp --sparse=always "$w/lower.img" "$w/base.img"
    # Recording.
    truncate -s "${LOG_MB}M" "$w/log.img"
    LOG_LOOP=$(losetup --find --show --direct-io=on "$w/log.img" 2>/dev/null || losetup --find --show "$w/log.img")
    LOWER_LOOP=$(losetup --find --show --direct-io=on "$w/lower.img" 2>/dev/null || losetup --find --show "$w/lower.img")
    dmsetup create "$LW" --table "0 $(blockdev --getsz "$LOWER_LOOP") log-writes $LOWER_LOOP $LOG_LOOP"; LW_UP=1
    lim 120 mount "${mopts[@]}" "/dev/mapper/$LW" "$w/mnt"; MNT=$w/mnt
    rm -rf "$RUN"
    if ! serve_up "$w/serve.ready" "$OUT/hole-$fs-$kind-serve.log"; then
        course_json "$fs" "$kind" failure "skyzfsd serve did not get ready (status ${SERVE_RC:-?}): $(tail -n 1 "$OUT/hole-$fs-$kind-serve.log")"
        course_end; return 0
    fi
    local crc=0
    lim 300 python3 -I "$HERE/replay_hole.py" course --device "$DEV" --lw "$LW" --manifest "$w/manifest.json" \
        --writes "$WRITES" --flush-every "$FLUSH_EVERY" --tail "$TAIL" > "$OUT/hole-$fs-$kind-course.log" 2>&1 || crc=$?
    if [ "$crc" != 0 ]; then
        # A write or a flush of the device that fails is the daemon's; a
        # mark that cannot be put in the log is the rig's.
        if grep -q 'dmsetup message' "$OUT/hole-$fs-$kind-course.log"; then
            course_json "$fs" "$kind" error "the course's mark failed: $(tail -n 1 "$OUT/hole-$fs-$kind-course.log")"
        else
            course_json "$fs" "$kind" failure "the course on $DEV failed (status $crc): $(tail -n 1 "$OUT/hole-$fs-$kind-course.log")"
        fi
        course_end; return 0
    fi
    serve_down || { course_json "$fs" "$kind" failure "skyzfsd serve did not stop cleanly after the course"; course_end; return 0; }
    lim 120 umount "$MNT"; MNT=""
    dmsetup remove "$LW"; LW_UP=""
    losetup -d "$LOWER_LOOP"; LOWER_LOOP=""
    losetup -d "$LOG_LOOP"; LOG_LOOP=""
    # The check: every prefix, or the marks' and a sample.
    local how=(--sample "$SAMPLE" --budget-s "$BUDGET_S")
    [ -z "${RIG_HOLE_EVERY_PREFIX:-}" ] || how=(--every-prefix)
    local args=()
    for s in "${SERVE_ARGS[@]}"; do args+=(--serve-arg="$s"); done
    python3 -I "$HERE/replay_hole.py" check --log "$w/log.img" --base "$w/base.img" --manifest "$w/manifest.json" \
        --work "$w/check" --out "$OUT" --skyzfsd "$SKYZFSD" --volume vol --fs "$fs" --kind "$kind" \
        --ublk "$(command -v ublk)" --dd "$DD" "${args[@]}" "${how[@]}" --budget-name "${RIG_BUDGET_NAME:-}" \
        --control >&2 || course_json "$fs" "$kind" error "the check of the course ended with an error (see the job's log)"
    cp "$w/manifest.json" "$OUT/hole-$fs-$kind-manifest.json"
    # Room for the next course.
    rm -f "$w/lower.img" "$w/base.img" "$w/log.img"; rm -rf "$w/check"
}

for fs in "${FSES[@]}"; do
    for kind in "${KINDS[@]}"; do
        course "$fs" "$kind"
    done
done
rc=0
python3 -I "$HERE/replay_hole.py" summary "$OUT"/hole-result-*.json --kernel "$(uname -r)" --daemon "${RIG_DAEMON_FROM:-}" --out "$OUT/hole-summary.md" > /dev/null || rc=$?
cat "$OUT/hole-summary.md"
exit "$rc"

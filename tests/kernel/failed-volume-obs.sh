#!/usr/bin/env bash
# What ZFS does when the only device of a pool is deleted under it, and
# when it comes back (design §4.2, "A failed volume gives up its
# attachment"; work package M1.T4, pins d.1 to d.5). The rule that a
# failed volume is not served again in the same boot rests on what
# `zpool clear` does with a device that reappears under the old name,
# and this script is where that is seen.
#
#   failed-volume-obs.sh OUT [WORK [SCENARIOS]]
#
# SCENARIOS is a list of the names below, in quotes; all of them if it
# is left out.
#
# It needs only the stock reference ublk server (`ublk add`, `ublk del`):
# no stand-in for the daemon and no patch. Everything it writes is an
# observation, one "key<TAB>value" line per finding in OUT/results.tsv;
# it judges nothing and exits 0 unless it could not set up. Each
# scenario has its own pool with failmode=wait, a local log device, a
# reader, and a writer that keeps its own record:
#
#   sync     what a sync waits for while a pool is suspended: fsync,
#            fdatasync and syncfs of a file on another file system,
#            sync(1) with a file, and at the very end sync() of everything.
#   other    the device is deleted; the same content comes back. The old
#            number is tried first; if the kernel refuses it, the
#            content comes back under another number with nothing at the
#            old path. First nobody runs zpool clear, then zpool clear.
#   link     the same, with the old path a symbolic link to the new device.
#   gone     the device is deleted for good, with a delete that waits:
#            zpool clear with no device, what is stuck with it, and
#            whether export -f or destroy -f gets rid of the pool.
#   zed      the first again with the event daemon running: what zed
#            does when the device goes and when one comes back.
#   older    the device comes back, by a route that resumed the pool,
#            with a copy of its image taken earlier: less than ZFS was told.
#   scrub    the device is deleted while a scrub is running.
#
# Meant for a throwaway machine: it leaves suspended pools behind.
set -uo pipefail

# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

OUT=${1:?usage: failed-volume-obs.sh OUT [WORK]}
WORK=${2:-/mnt/skz-failed-volume}
SCENARIOS=" ${3:-sync gone zed routes older scrub} "
want() { case $SCENARIOS in *" $1 "*) return 0 ;; esac; return 1; }
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK/mnt"
OUT=$(cd "$OUT" && pwd)
R=$OUT/results.tsv
: > "$R"
export DEBIAN_FRONTEND=noninteractive
VOL_GB=8
SLOG_MB=512

rec() {
    local v=${2//$'\t'/ }
    v=${v//$'\n'/; }
    printf '%s\t%s\n' "$1" "$v" >> "$R"
    printf '== %s: %s\n' "$1" "$v"
}
step() { printf '\n######## %s\n' "$*"; }
# A limit that holds even when the command cannot be killed.
# shellcheck source=tests/lib/lim.sh
. "$HERE/../lib/lim.sh"
pool_state() { cat "/proc/spl/kstat/zfs/$1/state" 2>/dev/null || echo unknown; }
status_lines() { # pool file: zpool status, kept whole in the file, the lines that matter on stdout
    lim 30 zpool status -v "$1" > "$2" 2>&1 || echo "zpool status failed or timed out (rc $?)" >> "$2"
    grep -E 'state:|status:|action:|scan:|errors:|ublkb|loop|repaired|in progress|timed out' "$2" | tr -s ' ' | tr '\n' ';'
}
kernel_lines() { # line count before -> what the kernel said since, most frequent first
    dmesg | tail -n +"$(($1 + 1))" | sed 's/^\[[^]]*\] //; s/[0-9]\{4,\}/N/g' | sort | uniq -c | sort -rn | head -n 8 | tr -s ' ' | tr '\n' ';'
}
add_dev() { lim 60 ublk add -t loop -n "$1" -f "$2" -q 1 -d 64 --max_io_buf_bytes 1048576 -r 1 -i 1; }   # number image

SLOGS=()
# shellcheck disable=SC2317,SC2329  # reached through the trap
cleanup() {
    set +e
    touch "$WORK/stop-all"
    local p
    for p in $(lim 30 zpool list -H -o name 2>/dev/null | grep '^skzf-'); do
        [ "$(pool_state "$p")" = ONLINE ] && lim 60 zpool export -f "$p" 2>/dev/null
    done
    command -v ublk >/dev/null && lim 60 ublk del -a --async 2>/dev/null
    for p in "${SLOGS[@]}"; do losetup -d "$p" 2>/dev/null; done
    dmesg > "$OUT/dmesg.txt" 2>/dev/null
    chmod -R a+rX "$OUT"
}
trap cleanup EXIT

step "setup"
rec kernel "$(uname -r)"
# shellcheck disable=SC1091
rec os "$(. /etc/os-release && echo "$PRETTY_NAME")"
apt-get update > /dev/null 2>&1
apt-get install -y --no-install-recommends zfsutils-linux zfs-zed python3 > "$OUT/apt.log" 2>&1
[ -e /dev/ublk-control ] || modprobe ublk_drv 2>> "$OUT/apt.log" || {
    apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/apt.log" 2>&1
    modprobe ublk_drv 2>> "$OUT/apt.log"
}
modprobe zfs 2>> "$OUT/apt.log"
rec zfs "$(cat /sys/module/zfs/version 2>/dev/null)"
rec zfs_userland "$(zfs version 2>&1 | tr '\n' ' ')"
# The package starts the event daemon; only the zed scenario wants it.
systemctl stop zfs-zed > /dev/null 2>&1
if ! "$HERE/build-ublksrv.sh" "${UBLKSRV_REF:-6d0d922614ec18e6f2b5d476af8d540a1d9c1de8}" > "$OUT/ublksrv.ver" 2> "$OUT/ublksrv-build.log" \
    || [ ! -e /dev/ublk-control ] || [ ! -r /proc/spl/kstat/zfs/arcstats ]; then
    rec setup "FAILED: see apt.log and ublksrv-build.log"
    exit 2
fi
rec ublk_server "$(tail -n 1 "$OUT/ublksrv.ver")"
rec zed "$(systemctl is-active zfs-zed 2>/dev/null || echo unknown); $(pgrep -c -x zed 2>/dev/null || echo 0) zed process(es)"
rec setup ok

# name number: a pool on /dev/ublkb<number> with files, a reader whose
# reads reach the device, and a writer with its own record.
scenario_start() {
    local name=$1 n=$2 pool=skzf-$1 mnt=$WORK/mnt/skzf-$1 img=$WORK/$1.img slog i
    truncate -s "${VOL_GB}G" "$img"
    add_dev "$n" "$img" > "$OUT/$name-add.log" 2>&1 || { rec "${name}_setup" "ublk add failed: $(tail -n 1 "$OUT/$name-add.log")"; return 1; }
    for i in $(seq 1 40); do [ -b "/dev/ublkb$n" ] && break; sleep 0.25; done
    truncate -s "${SLOG_MB}M" "$WORK/$name.slog"
    "$DD" if=/dev/zero of="$WORK/$name.slog" bs=1M count="$SLOG_MB" oflag=direct conv=notrunc,fsync status=none
    slog=$(losetup --find --show --direct-io=on "$WORK/$name.slog" 2>/dev/null || losetup --find --show "$WORK/$name.slog")
    SLOGS+=("$slog")
    mkdir -p "$mnt"
    lim 180 zpool create -f -o ashift=12 -o cachefile=none -o failmode=wait -O mountpoint="$mnt" -O compression=off \
        -O atime=off -O primarycache=metadata "$pool" "/dev/ublkb$n" log "$slog" > "$OUT/$name-create.log" 2>&1 \
        || { rec "${name}_setup" "zpool create failed: $(tail -n 1 "$OUT/$name-create.log")"; return 1; }
    for i in 0 1 2 3; do
        lim 120 "$DD" if=/dev/urandom of="$mnt/base.$i" bs=1M count=32 conv=fsync status=none
    done
    lim 120 zpool sync "$pool"
    : > "$OUT/$name-reader.log"; : > "$OUT/$name-writer.log"; : > "$WORK/$name.record"; echo 0 > "$WORK/$name.reads"
    ( i=0
      while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do
          cat "$mnt/base.$((i % 4))" > /dev/null 2>> "$OUT/$name-reader.log" || echo "read $i failed" >> "$OUT/$name-reader.log"
          i=$((i + 1)); echo "$i" > "$WORK/$name.reads"
      done ) &
    ( i=0
      while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do
          "$DD" if=/dev/urandom of="/dev/shm/skzf-$name" bs=1M count=4 status=none
          sum=$(sha256sum < "/dev/shm/skzf-$name" | cut -d' ' -f1)
          if "$DD" if="/dev/shm/skzf-$name" of="$mnt/r.$((i % 16))" bs=1M conv=fsync status=none 2>> "$OUT/$name-writer.log"; then
              echo "r.$((i % 16)) $sum" >> "$WORK/$name.record"
          else
              echo "write $i failed" >> "$OUT/$name-writer.log"
          fi
          i=$((i + 1))
      done ) &
    ( while [ ! -e "$WORK/$name.stop" ] && [ ! -e "$WORK/stop-all" ]; do lim 30 zpool sync "$pool" 2>/dev/null; sleep 0.2; done ) &
    sleep 8
}
# What the kernel still has of device number N.
device_state() {
    local n=$1
    echo "ublk list: $(lim 20 ublk list -n "$n" 2>&1 | tr -s ' \t\n' ' ' | cut -c1-260); /dev: $(find /dev -maxdepth 1 -name "ublk[bc]$n" | sort | tr '\n' ' '); /sys/block/ublkb$n: $([ -e "/sys/block/ublkb$n" ] && echo present || echo absent); holders: $(find "/sys/block/ublkb$n/holders" -mindepth 1 2>/dev/null | tr '\n' ' '); ublk control devices: $(find /sys/class/ublk-char -mindepth 1 -maxdepth 1 2>/dev/null | sed 's|.*/||' | sort | tr '\n' ' ')"
}
# name number [how]: delete the device under the pool and write down what
# follows. how is async (ublk del --async, the default) or wait (ublk
# del, which waits for the device to be gone).
scenario_delete() {
    local name=$1 n=$2 how=${3:-async} pool=skzf-$1 mark i st rc t0
    mark=$(dmesg | wc -l)
    rec "${name}_before" "pool $(pool_state "$pool"); $(grep -c . "$WORK/$name.record") files acknowledged; $(cat "$WORK/$name.reads") reads done"
    t0=$(date +%s)
    if [ "$how" = wait ]; then
        lim 90 ublk del -n "$n" > "$OUT/$name-del.log" 2>&1; rc=$?
        rec "${name}_del_waiting" "ublk del -n $n without --async: rc $rc after $(( $(date +%s) - t0 )) s (124: it had not returned after 90 s)"
    else
        lim 60 ublk del --async -n "$n" > "$OUT/$name-del.log" 2>&1; rc=$?
    fi
    for i in $(seq 1 180); do st=$(pool_state "$pool"); [ "$st" = SUSPENDED ] && break; sleep 1; done
    sleep 3
    rec "${name}_after_delete" "ublk del rc $rc; pool $st $(( $(date +%s) - t0 )) s after the delete began; read errors seen by the reader: $(grep -c . "$OUT/$name-reader.log"); writes failed for the writer: $(grep -c failed "$OUT/$name-writer.log"); $(grep -c . "$WORK/$name.record") files acknowledged"
    rec "${name}_device_after_delete" "$(device_state "$n")"
    rec "${name}_status_suspended" "$(status_lines "$pool" "$OUT/$name-status-suspended.log")"
    rec "${name}_kernel_log" "$(kernel_lines "$mark")"
    rec "${name}_events" "$(lim 60 zpool events -H 2>/dev/null | wc -l) events in all so far; by class: $(lim 60 zpool events -H 2>/dev/null | awk '{ c[$NF]++ } END { for (k in c) printf "%s x%d ", k, c[k] }' | cut -c1-300)"
    [ "$st" = SUSPENDED ]
}
# name number image route: the content comes back as a device. First the
# old number is tried. If the kernel refuses it, the route decides:
#   other   another number, and nothing at the old path
#   link    another number, and the old path a symbolic link to it
# Sets BACK to how it came back.
bring_back() {
    local name=$1 n=$2 img=$3 route=$4 mark rc m=$(($2 + 10)) i
    mark=$(dmesg | wc -l)
    add_dev "$n" "$img" > "$OUT/$name-readd.log" 2>&1; rc=$?
    for i in $(seq 1 20); do [ -b "/dev/ublkb$n" ] && break; sleep 0.25; done
    if [ $rc = 0 ] && [ -b "/dev/ublkb$n" ]; then
        BACK="under the old number, /dev/ublkb$n"
        rec "${name}_old_number" "ublk add -n $n succeeded"
    else
        rec "${name}_old_number" "ublk add -n $n refused: rc $rc ($(tr -s ' \t\n' ' ' < "$OUT/$name-readd.log" | cut -c1-200)); $(device_state "$n"); kernel at the add: $(dmesg | tail -n +"$((mark + 1))" | grep -vi 'zio\|WARNING: Pool' | sed 's/^\[[^]]*\] //' | sort | uniq -c | head -n 6 | tr -s ' ' | tr '\n' ';')"
        add_dev "$m" "$img" > "$OUT/$name-readd-other.log" 2>&1; rc=$?
        for i in $(seq 1 40); do [ -b "/dev/ublkb$m" ] && break; sleep 0.25; done
        BACK="as /dev/ublkb$m (ublk add rc $rc), nothing at /dev/ublkb$n"
        if [ "$route" = link ] && [ -b "/dev/ublkb$m" ] && [ ! -e "/dev/ublkb$n" ]; then
            ln -s "/dev/ublkb$m" "/dev/ublkb$n" && BACK="as /dev/ublkb$m (ublk add rc $rc), with /dev/ublkb$n a symbolic link to it"
        fi
    fi
    rec "${name}_back_as" "$BACK"
}
# name seconds: with the device back and no zpool clear, does the pool resume by itself?
wait_unaided() {
    local name=$1 secs=$2 pool=skzf-$1 i st
    for i in $(seq 1 "$secs"); do st=$(pool_state "$pool"); [ "$st" = ONLINE ] && break; sleep 1; done
    rec "${name}_back_without_clear" "pool $st after $i s with no zpool clear; $(status_lines "$pool" "$OUT/$name-status-back.log")"
}
# name: zpool clear, and what the pool is afterwards.
clear_and_see() {
    local name=$1 pool=skzf-$1 mnt=$WORK/mnt/skzf-$1 mark rc i st before v t0
    mark=$(dmesg | wc -l)
    before=$(grep -c . "$WORK/$name.record")
    t0=$(date +%s)
    lim 120 zpool clear "$pool" > "$OUT/$name-clear.log" 2>&1; rc=$?
    for i in $(seq 1 30); do st=$(pool_state "$pool"); [ "$st" = ONLINE ] && break; sleep 1; done
    sleep 10
    v="not tried: the pool is $st"
    if [ "$st" = ONLINE ]; then
        v=failed; lim 120 "$DD" if=/dev/zero of="$mnt/after-clear" bs=1M count=4 conv=fsync status=none 2>/dev/null && v=completed
    fi
    rec "${name}_after_clear" "zpool clear rc $rc after $(( $(date +%s) - t0 )) s (124: it had not returned after 120 s, and could not be killed) ($(tail -n 1 "$OUT/$name-clear.log")); pool $st; a new write with fsync: $v; files acknowledged $before before the clear, $(grep -c . "$WORK/$name.record") after; read errors seen by the reader: $(grep -c . "$OUT/$name-reader.log"); writes failed for the writer: $(grep -c failed "$OUT/$name-writer.log")"
    rec "${name}_status_after_clear" "$(status_lines "$pool" "$OUT/$name-status-after-clear.log")"
    rec "${name}_kernel_log_return" "$(kernel_lines "$mark")"
    touch "$WORK/$name.stop"
    [ "$st" = ONLINE ] || return 1
    sleep 5
    lim 600 zpool scrub -w "$pool" > "$OUT/$name-scrub.log" 2>&1; rc=$?
    rec "${name}_scrub_after" "zpool scrub rc $rc; $(status_lines "$pool" "$OUT/$name-status-scrubbed.log")"
    rec "${name}_files_after" "$(lim 300 python3 -I - "$mnt" "$WORK/$name.record" <<'PY' 2>&1 | tail -n 1
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
        got = e.strerror
    if got != digest:
        bad.append(f'{name} ({got if len(got) < 40 else "other content"})')
print(f'all {len(want)} files are as the writer recorded them' if not bad else
      f'{len(bad)} of {len(want)} files are not as the writer recorded them: {"; ".join(bad)}')
PY
    )"
}

# What a sync blocks on while a pool is suspended. FILE is on the root
# file system, which has nothing to do with the pool. Each call under
# the limit that holds for a process that cannot be killed.
sync_probe() { # name
    local f=/var/tmp/skz-sync-probe t0 rc what
    echo x > "$f"
    for what in fsync fdatasync syncfs; do
        t0=$(date +%s%N)
        lim 30 python3 -I -c '
import ctypes, os, sys
fd = os.open(sys.argv[2], os.O_RDWR)
os.write(fd, b"y")
if sys.argv[1] == "fsync":
    os.fsync(fd)
elif sys.argv[1] == "fdatasync":
    os.fdatasync(fd)
else:
    libc = ctypes.CDLL(None, use_errno=True)
    if libc.syncfs(fd) != 0:
        sys.exit(ctypes.get_errno() or 1)' "$what" "$f"; rc=$?
        rec "$1_$what" "$what of a file on the root file system: rc $rc after $(( ($(date +%s%N) - t0) / 1000000 )) ms (124: not returned after 30 s)"
    done
    t0=$(date +%s%N)
    lim 30 sync "$f"; rc=$?
    rec "$1_sync_file" "sync FILE ($(sync --version 2>&1 | head -n 1)): rc $rc after $(( ($(date +%s%N) - t0) / 1000000 )) ms"
    t0=$(date +%s%N)
    lim 30 sync -f "$f"; rc=$?
    rec "$1_sync_f_file" "sync -f FILE: rc $rc after $(( ($(date +%s%N) - t0) / 1000000 )) ms"
}
sync_all_probe() { # name: sync() of everything, last, since it may never return
    local t0 rc
    t0=$(date +%s%N)
    lim 30 python3 -I -c 'import os; os.sync()'; rc=$?
    rec "$1_sync_all" "sync() of everything: rc $rc after $(( ($(date +%s%N) - t0) / 1000000 )) ms (124: not returned after 30 s)"
}

step "sync: what a sync waits for while a pool is suspended"
rec sync_version "$(sync --version 2>&1 | head -n 1); $(readlink -f "$(command -v sync)")"
want sync && sync_probe sync_before
if want sync && scenario_start syncp 6; then
    scenario_delete syncp 6 || rec syncp_note "the pool did not suspend"
    sync_probe sync_suspended
fi
touch "$WORK/syncp.stop"

step "gone: the device is deleted for good, with a delete that waits"
if want gone && scenario_start gone 2; then
    scenario_delete gone 2 wait || rec gone_note "the pool did not suspend"
    # zpool clear with no device behind the pool, and what is stuck with it.
    ( lim 100 zpool clear skzf-gone > "$OUT/gone-clear.log" 2>&1; echo $? > "$WORK/gone-clear.rc" ) &
    clear_pid=$!
    sleep 15
    t0=$(date +%s)
    lim 30 zpool status skzf-gone > "$OUT/gone-status-during-clear.log" 2>&1; rc=$?
    rec gone_status_during_clear "zpool status while the first zpool clear has run 15 s: rc $rc after $(( $(date +%s) - t0 )) s; first clear $([ -e "$WORK/gone-clear.rc" ] && echo "already returned $(cat "$WORK/gone-clear.rc")" || echo 'still running')"
    t0=$(date +%s)
    lim 30 zpool clear skzf-gone > "$OUT/gone-clear2.log" 2>&1; rc=$?
    rec gone_second_clear "a second zpool clear: rc $rc after $(( $(date +%s) - t0 )) s ($(tail -n 1 "$OUT/gone-clear2.log"))"
    # Not a bare wait: the readers and writers of the earlier scenarios
    # are background jobs too, and they never return.
    wait "$clear_pid"
    rec gone_first_clear "zpool clear with no device: rc $(cat "$WORK/gone-clear.rc" 2>/dev/null) (124: not returned after 100 s) ($(tail -n 1 "$OUT/gone-clear.log")); pool $(pool_state skzf-gone)"
    # Is there a way to be rid of the pool short of a reboot?
    t0=$(date +%s)
    lim 60 zpool export -f skzf-gone > "$OUT/gone-export.log" 2>&1; rc=$?
    rec gone_export "zpool export -f: rc $rc after $(( $(date +%s) - t0 )) s ($(tail -n 1 "$OUT/gone-export.log")); pool $(pool_state skzf-gone)"
    t0=$(date +%s)
    lim 60 zpool destroy -f skzf-gone > "$OUT/gone-destroy.log" 2>&1; rc=$?
    rec gone_destroy "zpool destroy -f: rc $rc after $(( $(date +%s) - t0 )) s ($(tail -n 1 "$OUT/gone-destroy.log")); pool $(pool_state skzf-gone); pools listed: $(lim 30 zpool list -H -o name,health 2>&1 | tr '\t\n' ' ;' | cut -c1-200)"
    rec gone_device_at_end "$(device_state 2)"
fi
touch "$WORK/gone.stop"

step "zed: the same with the event daemon running"
zed_log() { journalctl -u zfs-zed --since "$1" --no-pager -o cat 2>/dev/null | sed 's/[0-9]\{4,\}/N/g' | sort | uniq -c | sort -rn | head -n 8 | tr -s ' ' | tr '\n' ';'; }
want zed && systemctl start zfs-zed > "$OUT/zed-start.log" 2>&1
sleep 2
rec zed_started "$(systemctl is-active zfs-zed 2>/dev/null || echo unknown); $(pgrep -c -x zed 2>/dev/null || echo 0) zed process(es); $(tail -n 1 "$OUT/zed-start.log")"
if want zed && scenario_start zed 3; then
    since=$(date '+%Y-%m-%d %H:%M:%S')
    scenario_delete zed 3 || rec zed_note "the pool did not suspend"
    sleep 30
    rec zed_after_delete "30 s after the delete: pool $(pool_state skzf-zed); zed $(systemctl is-active zfs-zed 2>/dev/null), cpu and memory: $(ps -o pcpu=,rss= -C zed 2>/dev/null | tr -s ' \n' ' '); events in all: $(lim 60 zpool events -H 2>/dev/null | wc -l); zed said: $(zed_log "$since"); $(status_lines skzf-zed "$OUT/zed-status-30s.log")"
    since=$(date '+%Y-%m-%d %H:%M:%S')
    bring_back zed 3 "$WORK/zed.img" other
    wait_unaided zed 60
    rec zed_after_return "with the device back as another number for 60 s: zed said: $(zed_log "$since")"
    if [ "$(pool_state skzf-zed)" != ONLINE ] && [ -b /dev/ublkb13 ] && [ ! -e /dev/ublkb3 ]; then
        since=$(date '+%Y-%m-%d %H:%M:%S')
        ln -s /dev/ublkb13 /dev/ublkb3
        udevadm trigger --action=change /dev/ublkb13 2>/dev/null
        for i in $(seq 1 60); do [ "$(pool_state skzf-zed)" = ONLINE ] && break; sleep 1; done
        rec zed_after_link "with /dev/ublkb3 a symbolic link to it, and a udev change event, for $i s: pool $(pool_state skzf-zed); zed said: $(zed_log "$since")"
    fi
    clear_and_see zed
fi
touch "$WORK/zed.stop"
systemctl stop zfs-zed > /dev/null 2>&1

# The routes by which the content can come back, each on its own pool.
RESUMED=""
for route in other link; do
    step "$route: the device is deleted; the same content comes back ($route)"
    n=0; [ "$route" = link ] && n=1
    if want routes && scenario_start "$route" "$n"; then
        scenario_delete "$route" "$n" || rec "${route}_note" "the pool did not suspend; the rest is recorded as it went"
        bring_back "$route" "$n" "$WORK/$route.img" "$route"
        wait_unaided "$route" 60
        if clear_and_see "$route"; then RESUMED+="$route "; fi
    fi
    touch "$WORK/$route.stop"
done
rec routes_that_resumed "${RESUMED:-none}"

step "older: the device comes back with an earlier copy of its image"
route=${RESUMED%% *}; route=${route:-link}
if want older && scenario_start older 4; then
    lim 120 zpool sync skzf-older
    cp --sparse=always "$WORK/older.img" "$WORK/older-copy.img"
    rec older_copy "taken with $(grep -c . "$WORK/older.record") files acknowledged; it comes back by the route '$route'"
    sleep 20
    scenario_delete older 4 || rec older_note "the pool did not suspend; the rest is recorded as it went"
    bring_back older 4 "$WORK/older-copy.img" "$route"
    clear_and_see older
fi
touch "$WORK/older.stop"

step "scrub: the device is deleted while a scrub is running"
if want scrub && scenario_start scrub 5; then
    lim 300 "$DD" if=/dev/urandom of="$WORK/mnt/skzf-scrub/big" bs=1M count=2048 conv=fsync status=none
    lim 30 zpool scrub skzf-scrub
    sleep 1
    rec scrub_running "$(status_lines skzf-scrub "$OUT/scrub-status-running.log")"
    scenario_delete scrub 5 || rec scrub_note "the pool did not suspend"
fi
touch "$WORK/scrub.stop"

step "sync, the end: everything at once, and the single file again after it"
if want sync; then
    sync_all_probe sync_suspended
    sync_probe sync_after_sync_all
fi

step "done"
cat "$R"
exit 0

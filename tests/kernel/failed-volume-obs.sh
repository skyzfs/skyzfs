#!/usr/bin/env bash
# What ZFS does when the only device of a pool is deleted under it, and
# when it comes back (design §4.2, "A failed volume gives up its
# attachment"; work package M1.T4, pins d.1 to d.5). The rule that a
# failed volume is not served again in the same boot rests on what
# `zpool clear` does with a device that reappears under the old name,
# and this script is where that is seen.
#
#   failed-volume-obs.sh OUT [WORK]
#
# It needs only the stock reference ublk server (`ublk add`, `ublk del`):
# no stand-in for the daemon and no patch. Everything it writes is an
# observation, one "key<TAB>value" line per finding in OUT/results.tsv;
# it judges nothing and exits 0 unless it could not set up. Three
# scenarios, each on its own pool with failmode=wait, a local log
# device, a reader, and a writer that keeps its own record:
#
#   same     the device is deleted; it comes back with the same content.
#            First nobody runs zpool clear (does zed bring it back?),
#            then zpool clear.
#   older    the same, but the device comes back with a copy of its
#            image taken earlier: less than ZFS was told.
#   scrub    the device is deleted while a scrub is running.
#
# Meant for a throwaway machine: it leaves suspended pools behind.
set -uo pipefail

# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

OUT=${1:?usage: failed-volume-obs.sh OUT [WORK]}
WORK=${2:-/mnt/skz-failed-volume}
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
events() { lim 30 zpool events -H 2>/dev/null | awk '{ c[$NF]++ } END { for (k in c) printf "%s x%d ", k, c[k] }'; }
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
apt-get install -y --no-install-recommends zfsutils-linux python3 > "$OUT/apt.log" 2>&1
[ -e /dev/ublk-control ] || modprobe ublk_drv 2>> "$OUT/apt.log" || {
    apt-get install -y --no-install-recommends "linux-modules-extra-$(uname -r)" >> "$OUT/apt.log" 2>&1
    modprobe ublk_drv 2>> "$OUT/apt.log"
}
modprobe zfs 2>> "$OUT/apt.log"
rec zfs "$(cat /sys/module/zfs/version 2>/dev/null)"
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
# name number: delete the device under the pool and write down what follows.
scenario_delete() {
    local name=$1 n=$2 pool=skzf-$1 mark i st rc
    mark=$(dmesg | wc -l)
    rec "${name}_before" "pool $(pool_state "$pool"); $(grep -c . "$WORK/$name.record") files acknowledged; $(cat "$WORK/$name.reads") reads done"
    lim 60 ublk del --async -n "$n" > "$OUT/$name-del.log" 2>&1; rc=$?
    for i in $(seq 1 180); do st=$(pool_state "$pool"); [ "$st" = SUSPENDED ] && break; sleep 1; done
    sleep 3
    rec "${name}_after_delete" "ublk del rc $rc; pool $st after $i s; device node $([ -b "/dev/ublkb$n" ] && echo present || echo gone); read errors seen by the reader: $(grep -c . "$OUT/$name-reader.log"); writes failed for the writer: $(grep -c failed "$OUT/$name-writer.log"); $(grep -c . "$WORK/$name.record") files acknowledged"
    rec "${name}_status_suspended" "$(status_lines "$pool" "$OUT/$name-status-suspended.log")"
    rec "${name}_kernel_log" "$(kernel_lines "$mark")"
    rec "${name}_events" "$(events)"
    [ "$st" = SUSPENDED ]
}
# name number image: the device comes back under the old name; first
# without zpool clear, then with it.
scenario_return() {
    local name=$1 n=$2 img=$3 pool=skzf-$1 mnt=$WORK/mnt/skzf-$1 mark rc i st before v m how
    mark=$(dmesg | wc -l)
    add_dev "$n" "$img" > "$OUT/$name-readd.log" 2>&1; rc=$?
    if [ $rc != 0 ] || [ ! -b "/dev/ublkb$n" ]; then
        # The old number cannot be had again. Write down what the kernel
        # still holds, then bring the content back as another device and
        # put it at the old path by hand, as an operator who wants the
        # pool back might.
        rec "${name}_old_number" "ublk add -n $n failed, rc $rc ($(tail -n 1 "$OUT/$name-readd.log")); ublk list -n $n: $(lim 20 ublk list -n "$n" 2>&1 | tr -s ' \t\n' ' ' | cut -c1-300); /dev: $(find /dev -maxdepth 1 -name 'ublk[bc]*' | sort | tr '\n' ' '); /sys/block: $(find /sys/block -maxdepth 1 -name 'ublkb*' | sort | tr '\n' ' '); kernel: $(dmesg | tail -n 400 | grep -i ublk | sed 's/^\[[^]]*\] //' | sort | uniq -c | head -n 5 | tr -s ' ' | tr '\n' ';')"
        m=$((n + 10))
        add_dev "$m" "$img" > "$OUT/$name-readd-other.log" 2>&1; rc=$?
        for i in $(seq 1 40); do [ -b "/dev/ublkb$m" ] && break; sleep 0.25; done
        how="as /dev/ublkb$m (ublk add rc $rc)"
        if [ -b "/dev/ublkb$m" ] && [ ! -e "/dev/ublkb$n" ]; then
            ln -s "/dev/ublkb$m" "/dev/ublkb$n" && how+=", with /dev/ublkb$n a symbolic link to it"
        fi
        rec "${name}_back_as" "$how"
    else
        rec "${name}_back_as" "under the old number, /dev/ublkb$n (ublk add rc 0)"
    fi
    for i in $(seq 1 60); do st=$(pool_state "$pool"); [ "$st" = ONLINE ] && break; sleep 1; done
    rec "${name}_back_without_clear" "pool $st after $i s with no zpool clear; $(status_lines "$pool" "$OUT/$name-status-back.log")"
    before=$(grep -c . "$WORK/$name.record")
    lim 120 zpool clear "$pool" > "$OUT/$name-clear.log" 2>&1; rc=$?
    for i in $(seq 1 30); do st=$(pool_state "$pool"); [ "$st" = ONLINE ] && break; sleep 1; done
    sleep 10
    v=failed; lim 120 "$DD" if=/dev/zero of="$mnt/after-clear" bs=1M count=4 conv=fsync status=none 2>/dev/null && v=completed
    rec "${name}_after_clear" "zpool clear rc $rc ($(tail -n 1 "$OUT/$name-clear.log")); pool $st; a new write with fsync $v; files acknowledged $before before the clear, $(grep -c . "$WORK/$name.record") ten seconds after; read errors seen by the reader: $(grep -c . "$OUT/$name-reader.log"); writes failed for the writer: $(grep -c failed "$OUT/$name-writer.log")"
    rec "${name}_status_after_clear" "$(status_lines "$pool" "$OUT/$name-status-after-clear.log")"
    touch "$WORK/$name.stop"; sleep 5
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
    rec "${name}_kernel_log_return" "$(kernel_lines "$mark")"
}

step "same: the device is deleted and comes back with the same content"
if scenario_start same 0; then
    scenario_delete same 0 || rec same_note "the pool did not suspend; the rest is recorded as it went"
    scenario_return same 0 "$WORK/same.img"
fi
touch "$WORK/same.stop"

step "older: the device comes back with an earlier copy of its image"
if scenario_start older 1; then
    lim 120 zpool sync skzf-older
    cp --sparse=always "$WORK/older.img" "$WORK/older-copy.img"
    rec older_copy "taken with $(grep -c . "$WORK/older.record") files acknowledged"
    sleep 20
    scenario_delete older 1 || rec older_note "the pool did not suspend; the rest is recorded as it went"
    scenario_return older 1 "$WORK/older-copy.img"
fi
touch "$WORK/older.stop"

step "scrub: the device is deleted while a scrub is running"
if scenario_start scrub 2; then
    lim 300 "$DD" if=/dev/urandom of="$WORK/mnt/skzf-scrub/big" bs=1M count=2048 conv=fsync status=none
    lim 30 zpool scrub skzf-scrub
    sleep 1
    rec scrub_running "$(status_lines skzf-scrub "$OUT/scrub-status-running.log")"
    scenario_delete scrub 2 || rec scrub_note "the pool did not suspend"
fi
touch "$WORK/scrub.stop"

step "done"
cat "$R"
exit 0

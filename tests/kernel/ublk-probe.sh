#!/usr/bin/env bash
# What does this kernel give SkyZFS? Probes ublk, ZFS and ZFS on a ublk
# device, and records one "key<TAB>value" line per finding in
# OUT/results.tsv. A step that fails is a finding, and the steps after
# it that can still run do. The exceptions are the `pin_*` lines:
# properties the design depends on (docs/skyzfs-design.md §4.2, §6.2),
# each PASS or FAIL, and any FAIL makes the probe exit 1 at the end.
#
#   ublk-probe.sh OUT [WORK]
#
# Meant for a throwaway machine (a GitHub runner): it installs packages,
# loads modules, builds the reference ublk server and creates a pool
# named skyzfsprobe. Needs root.
#
# The steps:
#   1. kernel, ublk_drv, the feature mask from UBLK_U_CMD_GET_FEATURES
#   2. ZFS from packages; dm targets a write-log rig would use
#   3. a ublk server: from packages, else ublk-org/ublksrv from source
#   4. a file served as /dev/ublkbN, raw read-back
#   5. a pool on it (ashift=12, a file-backed log device written through
#      beforehand): write, scrub, export, import, verify, trim; where
#      the labels are; whether flushes reach the ublk queue; what ZFS
#      does when every flush fails (zinject)
#   6. the same pool on a device with user recovery: kill -9 the server
#      while ZFS writes, recover, scrub; with reissue (pinned) and
#      without (the contrast)
set -uo pipefail

# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

OUT=${1:?usage: ublk-probe.sh OUT [WORK]}
WORK=${2:-/mnt/ublk-probe}
[ "$(id -u)" = 0 ] || { echo "needs root" >&2; exit 64; }
mkdir -p "$OUT" "$WORK"
OUT=$(cd "$OUT" && pwd)
R=$OUT/results.tsv
: > "$R"
export DEBIAN_FRONTEND=noninteractive

POOL=skyzfsprobe
MNT=$WORK/fs
DEV_ID=0
BDEV=/dev/ublkb$DEV_ID
IMG=$WORK/vdev.img
SLOG_IMG=$WORK/slog.img
SLOG_LOOP=""
VDEV_GB=8
SLOG_MB=512

rec() { # key value: one finding. Tabs and newlines in the value are flattened.
    local v=${2//$'\t'/ }
    v=${v//$'\n'/; }
    printf '%s\t%s\n' "$1" "$v" >> "$R"
    printf '== %s: %s\n' "$1" "$v"
}
step() { printf '\n######## %s\n' "$*"; }
# Every command that touches the device or the pool runs under a limit:
# a ublk device whose server is gone blocks its callers.
lim() { local s=$1; shift; timeout -k 5 "$s" "$@"; }
try() { # key seconds command...: record ok, or the last line of what it said
    local key=$1 s=$2 rc; shift 2
    # timeout(1) cannot run a shell function; the functions passed here
    # put their own limits on what they call.
    if declare -F "$1" >/dev/null; then
        "$@" > "$OUT/$key.log" 2>&1; rc=$?
    else
        lim "$s" "$@" > "$OUT/$key.log" 2>&1; rc=$?
    fi
    if [ $rc -eq 0 ]; then rec "$key" ok
    elif [ $rc -eq 124 ] || [ $rc -eq 137 ]; then rec "$key" "TIMEOUT after ${s}s"
    else rec "$key" "FAILED rc=$rc: $(tail -n 1 "$OUT/$key.log")"
    fi
    sed "s/^/[$key] /" "$OUT/$key.log"
    return $rc
}
ublk_field() { # "daemon pid" | "state": from `ublk list`
    lim 20 ublk list -n "$DEV_ID" 2>/dev/null | sed -n "s/.*$1 \([A-Za-z0-9_]*\).*/\1/p" | head -n 1
}
pool_errors() { # read/write/checksum error counters of every vdev, summed, plus the scan line
    lim 60 zpool status -p "$POOL" > "$OUT/status.tmp" 2>&1 || { echo "zpool status failed"; return; }
    awk '$2 ~ /^(ONLINE|DEGRADED|FAULTED|UNAVAIL|REMOVED)$/ && NF >= 5 { r += $3; w += $4; c += $5 }
         /scan:/ { sub(/^ *scan: */, ""); scan = $0 }
         /^errors:/ { sub(/^errors: */, ""); err = $0 }
         END { printf "vdev errors %d (read %d, write %d, checksum %d), %s, %s", r + w + c, r, w, c, scan, err }' "$OUT/status.tmp"
}
cleanup() {
    set +e
    lim 60 zpool export -f "$POOL" 2>/dev/null
    [ -n "$SLOG_LOOP" ] && losetup -d "$SLOG_LOOP" 2>/dev/null
    command -v ublk >/dev/null && lim 60 ublk del -a 2>/dev/null
    dmesg > "$OUT/dmesg.txt" 2>/dev/null
    chmod -R a+rX "$OUT"
}
trap cleanup EXIT

# ------------------------------------------------------------ 1. kernel
step "kernel and ublk_drv"
KREL=$(uname -r)
rec kernel "$KREL"
# shellcheck disable=SC1091
rec os "$(. /etc/os-release && echo "$PRETTY_NAME")"
rec cpus_mem "$(nproc) CPUs, $(awk '/MemTotal/ { printf "%.0f", $2 / 1048576 }' /proc/meminfo) GiB"
rec config_ublk "$(grep -E '^CONFIG_BLK_DEV_UBLK' "/boot/config-$KREL" 2>/dev/null | tr '\n' ' ' || true)"

how=""
if [ -e /dev/ublk-control ]; then
    how="already present"
elif modprobe ublk_drv 2>"$OUT/modprobe-ublk.log"; then
    how="modprobe ublk_drv"
else
    apt-get update >/dev/null 2>&1
    apt-get install -y --no-install-recommends "linux-modules-extra-$KREL" > "$OUT/modules-extra.log" 2>&1
    if modprobe ublk_drv 2>>"$OUT/modprobe-ublk.log"; then
        how="modprobe ublk_drv, after installing linux-modules-extra-$KREL"
    fi
fi
if [ -e /dev/ublk-control ]; then
    rec ublk_drv "yes ($how)"
else
    rec ublk_drv "NO: $(tail -n 1 "$OUT/modprobe-ublk.log" 2>/dev/null)"
fi
rec ublk_module_file "$(modinfo -n ublk_drv 2>&1 | sed "s|/lib/modules/$KREL/||")"
rec ublk_params "$(grep -H . /sys/module/ublk_drv/parameters/* 2>/dev/null | sed 's|.*/||' | tr '\n' ' ')"

step "feature mask from the kernel"
apt-get update >/dev/null 2>&1
apt-get install -y --no-install-recommends build-essential liburing-dev linux-libc-dev > "$OUT/apt-build.log" 2>&1
rec liburing "$(dpkg-query -W -f '${Version}' liburing-dev 2>/dev/null)"
cp /usr/include/linux/ublk_cmd.h "$OUT/ublk_cmd.distro.h" 2>/dev/null
FEATURES=""
if cc -O2 -o "$WORK/ublk-features" "$HERE/ublk-features.c" -luring 2>"$OUT/ublk-features.build.log"; then
    FEATURES=$("$WORK/ublk-features" 2>"$OUT/ublk-features.log")
    rec ublk_features "${FEATURES:-FAILED: $(cat "$OUT/ublk-features.log")}"
else
    rec ublk_features "BUILD FAILED: $(tail -n 1 "$OUT/ublk-features.build.log")"
fi

# --------------------------------------------------------------- 2. ZFS
step "ZFS from packages"
if apt-get install -y --no-install-recommends zfsutils-linux > "$OUT/apt-zfs.log" 2>&1; then
    rec zfs_userland "$(dpkg-query -W -f '${Version}' zfsutils-linux)"
else
    rec zfs_userland "INSTALL FAILED: $(tail -n 1 "$OUT/apt-zfs.log")"
fi
rec zfs_module_file "$(modinfo -n zfs 2>&1 | sed "s|/lib/modules/$KREL/||")"
if modprobe zfs 2>"$OUT/modprobe-zfs.log"; then
    rec zfs_module "loads, prebuilt for $KREL: $(cat /sys/module/zfs/version 2>/dev/null)"
else
    # No module for this kernel flavour in the distribution: DKMS.
    apt-get install -y --no-install-recommends "linux-headers-$KREL" zfs-dkms > "$OUT/apt-zfs-dkms.log" 2>&1
    if modprobe zfs 2>>"$OUT/modprobe-zfs.log"; then
        rec zfs_module "loads only after a zfs-dkms build: $(cat /sys/module/zfs/version 2>/dev/null)"
    else
        rec zfs_module "NO: $(tail -n 1 "$OUT/modprobe-zfs.log")"
    fi
fi
lim 30 zfs version > "$OUT/zfs-version.log" 2>&1
ZFS_OK=""; [ -r /proc/spl/kstat/zfs/arcstats ] && ZFS_OK=1

step "device-mapper targets for a write-log rig"
for m in dm_log_writes dm_flakey dm_dust dm_delay dm_snapshot; do
    if modprobe "$m" 2>/dev/null; then rec "mod_$m" yes; else rec "mod_$m" no; fi
done

# ------------------------------------------------------- 3. ublk server
step "a ublk server"
pkgs=$(apt-cache search --names-only 'ublk|ubdsrv' 2>/dev/null | tr '\n' ';')
rec ublk_packages "${pkgs:-none}"
SRV=""
if "$HERE/build-ublksrv.sh" "${UBLKSRV_REF:-}" > "$OUT/ublksrv.ver" 2> "$OUT/ublksrv-build.log"; then
    SRV=$(tail -n 1 "$OUT/ublksrv.ver")
    rec ublk_server "built from source: $SRV"
else
    rec ublk_server "BUILD FAILED: $(tail -n 1 "$OUT/ublksrv-build.log")"
    tail -n 40 "$OUT/ublksrv-build.log"
fi
if [ -n "$SRV" ]; then
    lim 20 ublk features > "$OUT/ublk-features.server.log" 2>&1
    cat "$OUT/ublk-features.server.log"
    lim 20 ublk help > "$OUT/ublk-help.log" 2>&1
fi

# ------------------------------------------------- 4. a file as ublkbN
# shellcheck disable=SC2120  # the recovery step passes arguments, through try
serve() { # extra `ublk add` arguments: serve $IMG as $BDEV
    lim 60 ublk add -t loop -n "$DEV_ID" -f "$IMG" "$@" || return 1
    local i=0
    while [ ! -b "$BDEV" ] && [ $i -lt 20 ]; do sleep 0.5; i=$((i + 1)); done
    [ -b "$BDEV" ]
}
SERVED=""
if [ -n "$SRV" ] && [ -e /dev/ublk-control ]; then
    step "serve a file as $BDEV"
    truncate -s "${VDEV_GB}G" "$IMG"
    if try serve_file 90 serve; then
        SERVED=1
        lim 20 ublk list > "$OUT/ublk-list.log" 2>&1; cat "$OUT/ublk-list.log"
        rec ublk_dev_flags "$(ublk_field flags)"
        rec ublk_target "$(grep -o 'target.*' "$OUT/ublk-list.log" | head -n 1)"
        q=/sys/block/$(basename "$BDEV")/queue
        rec queue_limits "$(for f in logical_block_size physical_block_size max_sectors_kb nr_requests \
            discard_max_bytes write_zeroes_max_bytes write_cache fua rotational; do
            [ -r "$q/$f" ] && printf '%s=%s ' "$f" "$(cat "$q/$f")"; done)"
        rec nr_hw_queues "$(find "/sys/block/$(basename "$BDEV")/mq" -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)"
        # Raw data path: 64 MiB written through the device must be what
        # the backing file holds, and must read back through the device.
        "$DD" if=/dev/urandom of="$WORK/pattern" bs=1M count=64 status=none
        if lim 120 "$DD" if="$WORK/pattern" of="$BDEV" bs=1M oflag=direct conv=fsync status=none 2>"$OUT/raw.log" \
            && lim 120 "$DD" if="$BDEV" of="$WORK/readback" bs=1M count=64 iflag=direct status=none 2>>"$OUT/raw.log" \
            && cmp -s "$WORK/pattern" "$WORK/readback" \
            && cmp -s -n $((64 << 20)) "$WORK/pattern" "$IMG"; then
            rec raw_io "ok: 64 MiB written, read back, and found in the backing file"
        else
            rec raw_io "FAILED: $(tail -n 1 "$OUT/raw.log")"
        fi
        rm -f "$WORK/pattern" "$WORK/readback"
        # A clean slate for ZFS: no stale data where the labels go.
        lim 60 ublk del -n "$DEV_ID" >/dev/null 2>&1
        rm -f "$IMG"; truncate -s "${VDEV_GB}G" "$IMG"
        # shellcheck disable=SC2119  # no extra arguments here, on purpose
        serve > "$OUT/serve2.log" 2>&1 || { rec serve_again "FAILED: $(tail -n 1 "$OUT/serve2.log")"; SERVED=""; }
    fi
fi

# ------------------------------------------------- 5. ZFS on the device
kstat_slog() { awk '$1 == "zil_itx_metaslab_slog_count" { print $3 }' /proc/spl/kstat/zfs/zil 2>/dev/null; }
import_pool() {
    local args=(-d "$BDEV" -d "$SLOG_LOOP") p
    for p in "${BDEV}p1" "${BDEV}p9"; do [ -b "$p" ] && args+=(-d "$p"); done
    lim 180 zpool import -o cachefile=none "${args[@]}" "$POOL"
}
write_data() { # 256 MiB in one file, then 200 small synchronous writes for the log device
    lim 240 "$DD" if=/dev/urandom of="$MNT/big" bs=1M count=256 conv=fsync status=none || return 1
    local i
    for i in $(seq 1 200); do
        lim 30 "$DD" if=/dev/urandom of="$MNT/small.$((i % 20))" bs=8k count=1 oflag=dsync status=none || return 1
    done
    (cd "$MNT" && sha256sum big small.* > "$WORK/sums")
}
POOL_OK=""
PINS_FAILED=0
pin() { # key PASS|FAIL detail: a property the design depends on (§4.2, §6.2). A FAIL fails the probe.
    rec "pin_$1" "$2: $3"
    [ "$2" = PASS ] || PINS_FAILED=$((PINS_FAILED + 1))
}
# Flush requests issued to the ublk queue, from the block layer's own
# tracepoint: a request whose flags start with F on this device. The
# kernel completes flushes by itself for a device without a volatile
# write cache, and then none would show here.
TR=/sys/kernel/tracing
flush_trace_start() {
    [ -d "$TR/events/block" ] || mount -t tracefs nodev "$TR" 2>/dev/null
    [ -d "$TR/events/block" ] || return 1
    echo 0 > "$TR/tracing_on"; echo > "$TR/trace"
    echo 1 > "$TR/events/block/block_rq_issue/enable"
    echo 1 > "$TR/tracing_on"
}
flush_trace_stop() { # file: keeps this device's flush requests there, prints how many
    echo 0 > "$TR/events/block/block_rq_issue/enable" 2>/dev/null
    local devno; devno=$(tr ':' ',' < "/sys/block/$(basename "$BDEV")/dev")
    grep "block_rq_issue: $devno F" "$TR/trace" > "$1" 2>/dev/null
    grep -c "block_rq_issue: $devno " "$TR/trace" > "$1.all" 2>/dev/null
    wc -l < "$1"
}
stat_flushes() { awk '{ print $16 }' "/sys/block/$(basename "$BDEV")/stat" 2>/dev/null; }
sync_with_flush_count() { # tag -> "<traced> <diskstats delta> <rc of zpool sync>"
    local tag=$1 f0 n rc traced=1
    flush_trace_start || traced=""
    f0=$(stat_flushes)
    lim 60 "$DD" if=/dev/urandom of="$MNT/flush.$tag" bs=1M count=8 status=none 2>/dev/null
    lim 120 zpool sync "$POOL" > "$OUT/sync-$tag.log" 2>&1; rc=$?
    n=na; [ -z "$traced" ] || n=$(flush_trace_stop "$OUT/flush-trace-$tag.log")
    echo "$n $(( $(stat_flushes) - f0 )) $rc"
}
if [ -n "$SERVED" ] && [ -n "$ZFS_OK" ]; then
    step "ZFS pool on $BDEV"
    # The log device is allocated storage: a file written through once.
    truncate -s "${SLOG_MB}M" "$SLOG_IMG"
    "$DD" if=/dev/zero of="$SLOG_IMG" bs=1M count="$SLOG_MB" oflag=direct conv=notrunc,fsync status=none
    SLOG_LOOP=$(losetup --find --show --direct-io=on "$SLOG_IMG" 2>/dev/null || losetup --find --show "$SLOG_IMG")
    mkdir -p "$MNT"
    if try zpool_create 180 zpool create -f -o ashift=12 -o cachefile=none -O mountpoint="$MNT" \
        -O compression=lz4 -O atime=off "$POOL" "$BDEV" log "$SLOG_LOOP"; then
        POOL_OK=1
        lim 30 zpool status -P -L "$POOL" > "$OUT/zpool-status-created.log" 2>&1; cat "$OUT/zpool-status-created.log"
        lsblk -o NAME,SIZE,TYPE,PHY-SEC,LOG-SEC "$BDEV" > "$OUT/lsblk.log" 2>&1; cat "$OUT/lsblk.log"
        rec vdev_path "$(awk -v d="$BDEV" 'index($1, d) == 1 { print $1 }' "$OUT/zpool-status-created.log" | head -n 1)"
        rec vdev_partitioned "$([ -b "${BDEV}p1" ] && echo "yes: ZFS made ${BDEV}p1 and p9 (whole-disk vdev)" || echo "no: the pool is on the bare device")"
        # From the label on the device, not from the pool's own report.
        lim 30 zdb -l "$BDEV" > "$OUT/zdb-label.log" 2>&1
        rec vdev_ashift "$(awk '$1 == "ashift:" { print $2; exit }' "$OUT/zdb-label.log")"
        rec zdb_labels "$(grep -E '^ *labels = ' "$OUT/zdb-label.log" | tr -s ' ' | head -n 1)"
        s0=$(kstat_slog)
        try zfs_write 300 write_data
        rec slog_used "$(( $(kstat_slog) - s0 )) intent-log records on the log device"
        try zpool_sync 120 zpool sync "$POOL"

        # Where the labels are, read from the file the server stores the
        # device in: byte N of the file is byte N of the device. A label
        # is 256 KiB: the configuration (an XDR nvlist) at 16 KiB, the
        # uberblock ring from 128 KiB. Cut detection finds uberblocks by
        # these positions.
        label=$(python3 - "$IMG" <<'PY'
import os, sys
fd = os.open(sys.argv[1], os.O_RDONLY)
nv = os.pread(fd, 4, 16 << 10)
slots = [o for o in range(128 << 10, 256 << 10, 4096)
         if os.pread(fd, 8, o) in (b'\x0c\xb1\xba\x00\x00\x00\x00\x00', b'\x00\x00\x00\x00\x00\xba\xb1\x0c')]
ok = nv[:2] == b'\x01\x01' and slots
print(('PASS' if ok else 'FAIL') + f": label 0 starts at byte 0 of the device: nvlist header {nv.hex()} at byte 16384, "
      f"{len(slots)} uberblocks in the ring at bytes 131072 to 262143, the first at byte {slots[0] if slots else 'none'}")
PY
        )
        if [ -b "${BDEV}p1" ]; then
            pin no_partition_table FAIL "zpool create partitioned $BDEV"
        else
            pin no_partition_table PASS "no partition on $BDEV after zpool create"
        fi
        pin label_at_byte_0 "${label%%:*}" "${label#*: }"

        # Flushes must reach the server: without them there is no
        # durability and there are no cuts.
        read -r traced delta rc <<< "$(sync_with_flush_count plain)"
        rec flush_requests "8 MiB written, then zpool sync (rc $rc): $traced flush requests issued to the ublk queue (block_rq_issue), diskstats flush count +$delta"
        if [ "$traced" != na ] && [ "$traced" -ge 2 ]; then
            pin flush_delivered PASS "$traced flush requests reached the ublk queue during one zpool sync"
        elif [ "$traced" = na ] && [ "$delta" -ge 2 ]; then
            pin flush_delivered PASS "diskstats counted $delta flushes during one zpool sync (no tracefs)"
        else
            pin flush_delivered FAIL "traced $traced, diskstats +$delta flush requests during zpool sync"
        fi

        try zpool_scrub 600 zpool scrub -w "$POOL"
        rec scrub_result "$(pool_errors)"
        try zpool_export 120 zpool export "$POOL"
        try zpool_import 180 import_pool
        if (cd "$MNT" && lim 300 sha256sum -c --quiet "$WORK/sums") > "$OUT/verify.log" 2>&1; then
            rec verify_after_import "ok: every file has the checksum it was written with"
        else
            rec verify_after_import "FAILED: $(tail -n 1 "$OUT/verify.log")"
        fi
        try zpool_trim 300 zpool trim -w "$POOL"
        rec backing_file_after_trim "$(du -m "$IMG" | cut -f1) MiB allocated of $((VDEV_GB * 1024))"
        rec pool_after "$(pool_errors)"

        # What ZFS does when every flush to the data vdev fails. The
        # fault is injected inside ZFS, so these flushes never reach the
        # device: the trace then shows whether ZFS went on to write.
        step "a failing flush (zinject)"
        if command -v zinject >/dev/null; then
            rec zinject_from "$(dpkg -S "$(command -v zinject)" 2>/dev/null | cut -d: -f1)"
        else
            # Not in zfsutils-linux on every release; the test package has it.
            apt-get install -y --no-install-recommends zfs-test > "$OUT/apt-zfs-test.log" 2>&1
            hash -r
            ZINJECT=$(command -v zinject || find /usr/sbin /usr/libexec /usr/lib/zfs-linux /usr/share/zfs -name zinject -type f 2>/dev/null | head -n 1)
            if [ -n "$ZINJECT" ]; then
                PATH=$PATH:$(dirname "$ZINJECT")
                rec zinject_from "$ZINJECT, after installing zfs-test"
            else
                rec zinject_from "not found: not in zfsutils-linux, and zfs-test did not provide it ($(tail -n 1 "$OUT/apt-zfs-test.log"))"
            fi
        fi
        if command -v zinject >/dev/null && try zinject_flush 30 zinject -d "$BDEV" -e io -T flush "$POOL"; then
            read -r traced delta rc <<< "$(sync_with_flush_count injected)"
            # Listed after the sync: its MATCH and INJECT columns say
            # whether the fault fired at all. Without them a clean sync
            # proves nothing.
            lim 30 zinject > "$OUT/zinject-list.log" 2>&1
            rec zinject_fired "$(awk '$4 == "flush" { printf "flush zios matched %s, failed %s", $(NF - 1), $NF }' "$OUT/zinject-list.log")"
            rec zinject_sync "zpool sync with every flush failing: rc $rc$([ "$rc" = 124 ] && echo ' (TIMEOUT)'); $traced flush requests reached the device, diskstats +$delta"
            rec zinject_pool_state "health $(lim 30 zpool list -H -o health "$POOL" 2>&1); $(pool_errors)"
            rec zinject_events "$(lim 30 zpool events -H 2>/dev/null | awk '{ c[$NF]++ } END { for (k in c) printf "%s x%d ", k, c[k] }')"
            lim 30 zinject -c all > /dev/null 2>&1
            read -r traced delta rc <<< "$(sync_with_flush_count cleared)"
            rec zinject_after_clear "zpool sync after the fault is removed: rc $rc; $traced flush requests reached the device (0 would mean ZFS stopped flushing this vdev)"
            lim 60 zpool clear "$POOL" > /dev/null 2>&1
        fi
    fi
fi

# --------------------------------------- 6. kill the server, recover
kill_test() { # prefix, then `ublk add` arguments: kill -9 the server while ZFS writes, recover, check
    local p=$1 pid before at_kill after i failed health scrub verify names; shift
    local wlog="$OUT/$p-writer.log"
    try "${p}_export" 120 zpool export "$POOL" || return 1
    lim 60 ublk del -n "$DEV_ID" > "$OUT/$p-del.log" 2>&1
    try "${p}_serve" 90 serve "$@" || return 1
    names=$(lim 20 ublk list -n "$DEV_ID" 2>/dev/null | sed -n 's/.*flags \(0x[0-9a-f]* \[[^]]*\]\).*/\1/p' | head -n 1)
    rec "${p}_dev_flags" "${names:-$(ublk_field flags)}"
    try "${p}_import" 180 import_pool || return 1
    : > "$wlog"; : > "$WORK/writer.count"
    [ ! -e "$WORK/stop" ] || unlink "$WORK/stop"
    (
        n=0
        while [ ! -e "$WORK/stop" ]; do
            "$DD" if=/dev/urandom of="$MNT/r.$((n % 16))" bs=1M count=4 conv=fsync status=none 2>>"$wlog" \
                || echo "write $n failed" >> "$wlog"
            n=$((n + 1)); echo "$n" > "$WORK/writer.count"
        done
    ) &
    WRITER=$!
    # An fsync is served by the log device alone. Syncing the pool in a
    # loop keeps writes to the data vdev in flight, so that the kill
    # lands on some.
    ( while [ ! -e "$WORK/stop" ]; do zpool sync "$POOL" 2>/dev/null; sleep 0.1; done ) &
    SYNCER=$!
    sleep 5
    pid=$(ublk_field "daemon pid")
    before=$(cat "$WORK/writer.count" 2>/dev/null); before=${before:-0}
    rec "${p}_server_pid" "${pid:-not found}"
    at_kill=$before
    if [ -n "$pid" ] && kill -9 "$pid"; then
        sleep 3
        rec "${p}_state_after_kill" "$(ublk_field state)"
        rec "${p}_inflight_at_kill" "$(tr -s ' ' < "/sys/block/$(basename "$BDEV")/inflight" 2>/dev/null)"
        at_kill=$(cat "$WORK/writer.count" 2>/dev/null); at_kill=${at_kill:-0}
        t0=$(date +%s%N)
        try "${p}_recover" 90 ublk recover -n "$DEV_ID"
        rec "${p}_recover_ms" "$(( ($(date +%s%N) - t0) / 1000000 ))"
        rec "${p}_state_after_recover" "$(ublk_field state)"
        sleep 5
        health=$(lim 30 zpool list -H -o health "$POOL" 2>&1)
        rec "${p}_pool_after_recover" "health ${health:-unknown}; $(pool_errors)"
        if [ "$health" != ONLINE ]; then
            try "${p}_zpool_clear" 120 zpool clear "$POOL"
            rec "${p}_pool_after_clear" "health $(lim 30 zpool list -H -o health "$POOL" 2>&1); $(pool_errors)"
        fi
        sleep 5
    fi
    touch "$WORK/stop"
    # The writer blocks while the device has no server; it must come
    # back once the device does.
    i=0; while kill -0 "$WRITER" 2>/dev/null && [ $i -lt 90 ]; do sleep 1; i=$((i + 1)); done
    failed=$(grep -c failed "$wlog" 2>/dev/null || true)
    if kill -0 "$WRITER" 2>/dev/null; then
        rec "${p}_writer" "STUCK: still blocked 90 s after recovery; failed writes: ${failed:-0}"
        kill -9 "$WRITER" 2>/dev/null
        failed=stuck
    else
        after=$(cat "$WORK/writer.count" 2>/dev/null)
        rec "${p}_writer" "resumed: $before files before the kill, $at_kill when recovery began, ${after:-0} at the end; failed writes: ${failed:-0}"
    fi
    kill -9 "$SYNCER" 2>/dev/null; pkill -9 -f "zpool sync $POOL" 2>/dev/null
    try "${p}_scrub" 600 zpool scrub -w "$POOL"
    scrub=$(pool_errors)
    rec "${p}_scrub_result" "$scrub"
    if (cd "$MNT" && lim 300 sha256sum -c --quiet "$WORK/sums") > "$OUT/$p-verify.log" 2>&1; then
        verify=ok
        rec "${p}_verify" "ok: the files written before the kill are intact"
    else
        verify=failed
        rec "${p}_verify" "FAILED: $(tail -n 1 "$OUT/$p-verify.log")"
    fi
    KT_SUMMARY="flags $names; failed writes ${failed:-0}; $scrub; earlier files $verify"
    [ "${failed:-0}" = 0 ] && [ "$verify" = ok ] && case "$scrub" in "vdev errors 0 "*"with 0 errors"*) true ;; *) false ;; esac
}
if [ -n "$POOL_OK" ]; then
    # Recovery is chosen when the device is added, so each run replaces
    # the device: -r 1 is UBLK_F_USER_RECOVERY, -i 1 adds _REISSUE (the
    # requests the dead server held are issued again, not failed).
    step "user recovery with reissue: kill -9 the server under load"
    KT_SUMMARY="did not run to the end"
    if kill_test rec -r 1 -i 1 && case "$KT_SUMMARY" in *RECOVERY_REISSUE*) true ;; *) false ;; esac; then
        pin recovery_reissue PASS "$KT_SUMMARY"
    else
        pin recovery_reissue FAIL "$KT_SUMMARY"
    fi
    # The contrast, last because it may leave the pool unusable: with
    # recovery but without reissue the kernel fails the requests the
    # dead server held. A finding either way, not a pin.
    step "user recovery without reissue: the contrast"
    KT_SUMMARY="did not run to the end"
    kill_test rec0 -r 1 -i 0
    rec rec0_outcome "$KT_SUMMARY"
    try rec0_export_end 120 zpool export "$POOL"
fi

step "done"
cat "$R"
if [ "$PINS_FAILED" -gt 0 ]; then
    echo "$PINS_FAILED pinned properties FAILED" >&2
    exit 1
fi

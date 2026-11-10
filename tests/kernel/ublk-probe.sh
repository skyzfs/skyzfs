#!/usr/bin/env bash
# What does this kernel give SkyZFS? Probes ublk, ZFS and ZFS on a ublk
# device, and records one "key<TAB>value" line per finding in
# OUT/results.tsv. Nothing here is fatal: a step that fails is a
# finding, and the steps after it that can still run do.
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
#      beforehand): write, scrub, export, import, verify, trim
#   6. the same pool on a device with user recovery: kill -9 the server
#      under fsync load, recover, scrub
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
    awk '$2 ~ /^(ONLINE|DEGRADED|FAULTED|UNAVAIL|REMOVED)$/ && NF >= 5 { e += $3 + $4 + $5 }
         /scan:/ { sub(/^ *scan: */, ""); scan = $0 }
         /^errors:/ { sub(/^errors: */, ""); err = $0 }
         END { printf "vdev errors %d, %s, %s", e, scan, err }' "$OUT/status.tmp"
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
rec ublk_packages "$(apt-cache search --names-only 'ublk|ubdsrv' 2>/dev/null | tr '\n' ';' | sed 's/;$//;s/^$/none/')"
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
        rec vdev_ashift "$(lim 30 zdb -C "$POOL" 2>/dev/null | awk '/ashift:/ { print $2; exit }')"
        s0=$(kstat_slog)
        try zfs_write 300 write_data
        rec slog_used "$(( $(kstat_slog) - s0 )) intent-log records on the log device"
        try zpool_sync 120 zpool sync "$POOL"
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
    fi
fi

# --------------------------------------- 6. kill the server, recover
if [ -n "$POOL_OK" ]; then
    step "user recovery: kill -9 the server under load"
    # Recovery is chosen when the device is added, so the device is
    # replaced: -r 1 is UBLK_F_USER_RECOVERY, -i 1 adds _REISSUE (the
    # requests the dead server held are issued again, not failed).
    try rec_export 120 zpool export "$POOL"
    lim 60 ublk del -n "$DEV_ID" > "$OUT/rec-del.log" 2>&1
    if try rec_serve 90 serve -r 1 -i 1; then
        rec rec_dev_flags "$(ublk_field flags)"
        if try rec_import 180 import_pool; then
            rm -f "$WORK/stop" "$WORK/writer.count"
            (
                n=0
                while [ ! -e "$WORK/stop" ]; do
                    "$DD" if=/dev/urandom of="$MNT/r.$((n % 16))" bs=1M count=4 conv=fsync status=none 2>>"$OUT/writer.log" \
                        || echo "write $n failed" >> "$OUT/writer.log"
                    n=$((n + 1)); echo "$n" > "$WORK/writer.count"
                done
            ) &
            WRITER=$!
            sleep 5
            pid=$(ublk_field "daemon pid")
            before=$(cat "$WORK/writer.count" 2>/dev/null || echo 0)
            rec rec_server_pid "${pid:-not found}"
            if [ -n "$pid" ] && kill -9 "$pid"; then
                sleep 3
                rec rec_state_after_kill "$(ublk_field state)"
                at_kill=$(cat "$WORK/writer.count" 2>/dev/null || echo 0)
                t0=$(date +%s%N)
                try rec_recover 90 ublk recover -n "$DEV_ID"
                rec rec_recover_ms "$(( ($(date +%s%N) - t0) / 1000000 ))"
                rec rec_state_after_recover "$(ublk_field state)"
                sleep 10
            fi
            touch "$WORK/stop"
            # The writer blocks in fsync while the device has no server;
            # it must come back once the device does.
            i=0; while kill -0 "$WRITER" 2>/dev/null && [ $i -lt 90 ]; do sleep 1; i=$((i + 1)); done
            if kill -0 "$WRITER" 2>/dev/null; then
                rec rec_writer "STUCK: still blocked 90 s after recovery"
                kill -9 "$WRITER" 2>/dev/null
            else
                after=$(cat "$WORK/writer.count" 2>/dev/null || echo 0)
                rec rec_writer "resumed: $before files before the kill, $at_kill when recovery began, $after at the end; failed writes: $(grep -c failed "$OUT/writer.log" 2>/dev/null || echo 0)"
            fi
            try rec_scrub 600 zpool scrub -w "$POOL"
            rec rec_scrub_result "$(pool_errors)"
            if (cd "$MNT" && lim 300 sha256sum -c --quiet "$WORK/sums") > "$OUT/rec-verify.log" 2>&1; then
                rec rec_verify "ok: the files written before the kill are intact"
            else
                rec rec_verify "FAILED: $(tail -n 1 "$OUT/rec-verify.log")"
            fi
            try rec_export_end 120 zpool export "$POOL"
        fi
    fi
fi

step "done"
cat "$R"

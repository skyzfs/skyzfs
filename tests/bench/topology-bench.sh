#!/usr/bin/env bash
# topology-bench — read-cache topology benchmark for the origin-advance
# stack, WITHOUT skynbd. RECORD-ONLY like bench.sh: it reports numbers
# and never judges them.
#
# What it models. The §5/§7.1 node stack is a dm-thin volume on a local
# pool whose external origin is a read-only export: after a restage or
# a reclaim every unwritten block is a network read, and writes
# provision into the pool. This script builds that shape from local
# loop files (an immutable origin image carrying a pgbench database, a
# fresh thin pool per topology, a cache carve) and runs postgres on top
# of each read-cache candidate, measuring at the BLOCK layer where the
# design's costs actually land:
#
#   origin reads   what the NBD attach would have served (network)
#   pool writes    thin-pool data-device writes (local flash, 1x)
#   cache writes   the cache device's own writes — the write
#                  amplification each topology adds on top of the pool
#   cache reads    hits served from the cache device
#
# Topologies (TB_TOPOLOGIES, space-separated; default runs all):
#
#   nocache   fs → thin. The floor: no cache, every miss is an origin read.
#   baseline  fs → dm-cache(writethrough, smq) → thin. The shipped §7.1
#             top-of-stack read cache: writes hit both cache and pool.
#   twovol    baseline for PGDATA plus a SECOND, uncached thin volume
#             holding pg_wal — the split discussed as the cheap way to
#             stop double-writing the WAL. The WAL thin has NO external
#             origin here: the shape where the WAL volume is never
#             snapshotted.
#   twovol-origin    twovol, but the WAL thin sits on its own (delayed)
#             origin image with nothing mapped locally: the WAL volume
#             right after an advance or a restore. Every first write to
#             a chunk copies it from the origin.
#   twovol-hydrated  twovol-origin, then every chunk is pulled local
#             with whole-chunk writes before postgres starts (dm-thin
#             skips the origin copy when a write covers the whole
#             chunk): the WAL volume that is snapshotted but never
#             advanced, or hydrated on attach after a restore.
#   bcache    fs → bcache(writearound, cutoffs off) → thin. bcache's
#             backing superblock lives on a tiny local carve concatenated
#             in front of the thin (a two-segment linear), so nothing is
#             ever written into the thin's address space at offset 0.
#   opencas   fs → Open CAS core (mode TB_CAS_MODE, default wi =
#             write-invalidate, the true write-around; needs casadm built
#             with CAS_EXT_EXP=1) → thin.
#
# ZFS topologies. The exploration these model is "ZFS on a userspace
# block backend" (a ublk device served by a node-local daemon with a
# local write log and S3 behind it): the pool's one vdev is a device
# whose written blocks are local and whose never-written blocks cost an
# origin round trip, with NO read-before-write at any granularity. The
# stand-in is a transient dm-snapshot with a chunk of one ZFS sector
# (1 << ashift) over a read-only, delayed image of an exported pool
# that carries the same pgbench database: a write that covers a whole
# chunk never reads the origin (dm-snap's io_overwrites_chunk path),
# and with chunk == ashift every ZFS write does. PGDATA and pg_wal are
# two datasets of the one pool.
#
#   zfs       recordsize 8k, lz4, full_page_writes off (safe on a
#             filesystem whose record writes are atomic), wal_recycle
#             and wal_init_zero off. The configuration one would deploy.
#   zfs-plain zfs with compression off and full_page_writes on: the
#             control that separates the copy-on-write layout from the
#             two savings pgbench's very compressible rows flatter.
#   zfs-l2arc zfs plus an L2ARC device of TB_CACHE_PCT: the counterpart
#             of baseline's dm-cache, a flash read cache that is not on
#             the write path.
#
# For these rows `origin` is the delayed pool image, `pool` is the
# dm-snapshot's COW device (the local flash every vdev write lands on),
# `fs` is the vdev itself and `cache` is the L2ARC device. The ARC is
# not charged to postgres's cgroup, so with TB_MEM_LIMIT set it is
# capped at TB_MEM_LIMIT minus shared_buffers — what the page cache
# gets in the other topologies.
#
# Usage (manual, as root, on the NVMe you want to measure):
#
#   sudo TB_WORK=/nvme/topology-bench tests/bench/topology-bench.sh
#   tests/bench/topology-bench.sh summarize DIR   # re-render summary.md
#                                                 # from *.json under DIR
#
# Knobs (env):
#   TB_OUT=dir              results: per-run JSON + summary.md
#                           (default /tmp/topology-bench)
#   TB_WORK=dir             backing files (origin/pool/cache images) —
#                           put this on the disk under test
#                           (default /var/tmp/topology-bench)
#   TB_TOPOLOGIES="..."     subset of: nocache baseline twovol twovol-origin
#                           twovol-hydrated bcache opencas zfs zfs-plain
#                           zfs-l2arc
#   TB_WORKLOADS="..."      pgbench builtins by short name: tpcb select update
#   TB_SCALE=100            pgbench scale (~15 MiB per unit)
#   TB_CLIENTS=8 TB_JOBS=4  pgbench -c / -j
#   TB_WARMUP=30            seconds of unmeasured pgbench before each run
#   TB_DURATION=120         measured seconds per run
#   TB_SHARED_BUFFERS=256MB postgres shared_buffers
#   TB_MEM_LIMIT=           cgroup v2 memory.max for postgres (e.g. 2G) so
#                           the page cache stops hiding the block layer
#   TB_CACHE_PCT=25         cache device size as % of the loaded database
#   TB_CACHE_BLOCK_KB=32    dm-cache block size (min 32; multiple of 32)
#   TB_MIGRATION_THRESHOLD= dm-cache migration_threshold (sectors; kernel
#                           default 2048 when unset)
#   TB_CAS_MODE=wi          Open CAS cache mode (wi|wa|wt|pt|wo)
#   TB_CAS_LINE_KB=         Open CAS cache line size (4..64; default =
#                           TB_CACHE_BLOCK_KB capped at 64)
#   TB_ORIGIN_DELAY_MS=10   dm-delay read latency on the origin, so it
#                           behaves like the RO NBD attach over an S3-backed
#                           daemon rather than a local file. 10 ms is the
#                           design's planning number for one object-store
#                           round trip (multinode design §1); 0 disables.
#                           Models latency per bio only — not bandwidth,
#                           not the daemon's segment locality.
#   TB_CHUNK_KB=64          thin-pool block size
#   TB_HYDRATE_JOBS=8       parallel 1 MiB read+write streams used to
#                           hydrate the WAL thin (twovol-hydrated)
#   TB_MAX_WAL_GB=1         postgres max_wal_size, which also bounds the
#                           image sizes derived from it
#   TB_VOL_GB / TB_POOL_GB / TB_WAL_GB   sizes (defaults derived from
#                           scale and TB_MAX_WAL_GB; images are sparse)
#   TB_ZFS_RECORDSIZE=8k    recordsize of the PGDATA dataset. Above 8k a
#                           page write is a partial-record write, which
#                           reads the record first when it is not in the ARC
#   TB_ZFS_WAL_RECORDSIZE=128k  recordsize of the pg_wal dataset
#   TB_ZFS_COMPRESSION=lz4  compression (zfs-plain forces off)
#   TB_ZFS_FPW=off          full_page_writes (zfs-plain forces on)
#   TB_ZFS_ASHIFT=12        pool ashift; also the dm-snapshot chunk
#   TB_ZFS_GB=              pool size (default 2x TB_VOL_GB: copy-on-write
#                           needs free space, and the window snapshot pins
#                           what the window overwrites)
#   TB_ZFS_ARC_MB=          ARC cap (default TB_MEM_LIMIT - shared_buffers;
#                           uncapped when TB_MEM_LIMIT is unset)
#   TB_ZFS_WINDOW_SNAPSHOT=1  snapshot both datasets at the start of each
#                           measured window, so `written@` at the end is
#                           the live bytes born in the window: what one
#                           upload per window would have to carry. 0 skips
#                           it (the snapshot pins every overwritten block
#                           for the window).
#   TB_PG_USER=postgres     unprivileged user postgres runs as
#   TB_KEEP=1               leave the stack up on failure for inspection
#   TB_REUSE_ORIGIN=1       keep an existing origin image (same scale)
#
# Needs root, device-mapper (dm-thin-pool, dm-cache, dm-delay when a
# delay is set), losetup, mkfs.ext4, python3, a PostgreSQL server with
# pgbench; bcache-tools for the bcache topology; casadm for opencas;
# zfsutils (zpool, zfs, the zfs module) and dm-snapshot for the zfs ones.
set -euo pipefail

# GNU dd as "$DD" (`gnudd` on ubuntu-26.04, plain `dd` before it): the
# O_DIRECT calls below fail with the Rust coreutils dd. See the helper.
# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"

need() { for t in "$@"; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 64; }; done; }
if [ "${1:-}" != summarize ]; then
    need dmsetup losetup blockdev mkfs.ext4 python3 pgbench
    [ "$(id -u)" = 0 ] || { echo "needs root (device-mapper)" >&2; exit 64; }
fi

OUT=${TB_OUT:-/tmp/topology-bench}

summarize() { # dir -> summary.md on stdout (all *.json under dir, any depth)
    python3 - "$1" <<'PY'
import glob, json, os, sys
out = sys.argv[1]
runs = [json.load(open(p)) for p in sorted(glob.glob(os.path.join(out, '**', '*.json'), recursive=True))]
order = ['nocache', 'baseline', 'twovol', 'twovol-origin', 'twovol-hydrated', 'bcache', 'opencas',
         'zfs', 'zfs-plain', 'zfs-l2arc']
runs.sort(key=lambda r: (r['workload'], order.index(r['topology']) if r['topology'] in order else 99))
print('## topology-bench\n')
print('Block-layer bytes over the measured window. `origin read` includes the WAL origin where there is one; '
      '`flash write` = pool + cache device writes; '
      '`amp` = flash write relative to the nocache run of the same workload.\n')
print('| workload | topology | tps | lat ms | origin read MB | pool read MB | pool write MB | cache write MB | cache read MB | flash write MB | amp | WAL MB | flushes (pool) |')
print('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
base = {}
rows = []
for r in runs:
    d = r['devices']; c = d.get('cache', {})
    flash = d['pool']['write_mb'] + c.get('write_mb', 0)
    if r['topology'] == 'nocache': base[r['workload']] = flash
    amp = flash / base[r['workload']] if base.get(r['workload']) else float('nan')
    print(f"| {r['workload']} | {r['topology']} | {r['tps'] or 0:.0f} | {r['latency_ms'] or 0:.2f} | "
          f"{d['origin']['read_mb'] + d.get('walorigin', {}).get('read_mb', 0):.0f} | {d['pool']['read_mb']:.0f} | {d['pool']['write_mb']:.0f} | {c.get('write_mb', 0):.0f} | "
          f"{c.get('read_mb', 0):.0f} | {flash:.0f} | {amp:.2f} | {r['pg']['wal_mb']:.0f} | {d['pool']['flushes']} |")
    rows.append((r, d, c, flash))
# Absolute bytes over a fixed window mislead when throughput differs (a
# stalled run writes less because it did less), so the same bytes per
# thousand transactions committed. Time-driven background writes
# (checkpoints, CoW) inflate the low-throughput rows: an upper bound there.
print('\nPer 1000 transactions committed (`amp` again relative to nocache):\n')
print('| workload | topology | transactions | pool MB/ktxn | cache MB/ktxn | flash MB/ktxn | amp/txn |')
print('|---|---|---|---|---|---|---|')
base_txn = {}
for r, d, c, flash in rows:
    txn = r['pg'].get('xact_commit', 0) or 0
    k = txn / 1000 if txn else float('nan')
    per = flash / k if txn else float('nan')
    if r['topology'] == 'nocache': base_txn[r['workload']] = per
    ampt = per / base_txn[r['workload']] if base_txn.get(r['workload']) else float('nan')
    print(f"| {r['workload']} | {r['topology']} | {txn} | {d['pool']['write_mb'] / k if txn else float('nan'):.1f} | "
          f"{c.get('write_mb', 0) / k if txn else float('nan'):.1f} | {per:.1f} | {ampt:.2f} |")
# ZFS rows: the vdev is the device a userspace backend would serve, so
# split what reached it. `first-touch` is the growth of the dm-snapshot
# COW store: bytes written to LBAs never written since the stage (ZFS
# rewriting an LBA it freed, or the reused ZIL region, does not grow
# it). `live at end` is `written@` against a snapshot taken at window
# start: the blocks born in the window and still referenced, which is
# what one upload per window carries when the uploader knows what was
# freed. The gap between vdev writes and that is dead versions.
zrows = [(r, d) for r, d, c, flash in rows if r.get('zfs')]
if zrows:
    MB = 1 << 20
    print('\nZFS rows, per 1000 transactions: what reached the vdev, and how much of it one upload per window would carry:\n')
    print('| workload | topology | vdev write MB/ktxn | first-touch LBA MB/ktxn | live at end MB/ktxn | ZIL MB/ktxn | vdev read MB | ARC hit % | L2ARC hit % of ARC misses |')
    print('|---|---|---|---|---|---|---|---|---|')
    for r, d in zrows:
        z = r['zfs']; dl = z.get('delta', {}); af = z.get('after', {})
        txn = r['pg'].get('xact_commit', 0) or 0
        k = txn / 1000 if txn else float('nan')
        zil = sum(dl.get(f'zil.zil_itx_metaslab_{cls}_alloc', dl.get(f'zil.zil_itx_metaslab_{cls}_bytes', 0))
                  for cls in ('normal', 'slog')) / MB
        live = af.get('window.written_bytes')
        hits = dl.get('arc.hits', 0); miss = dl.get('arc.misses', 0) + dl.get('arc.iohits', 0)
        l2h = dl.get('arc.l2_hits', 0); l2m = dl.get('arc.l2_misses', 0)
        print(f"| {r['workload']} | {r['topology']} | {d['fs']['write_mb'] / k:.1f} | "
              f"{dl.get('cow.allocated_bytes', 0) / MB / k:.1f} | "
              f"{'n/a' if live is None else format(live / MB / k, '.1f')} | {zil / k:.1f} | {d['fs']['read_mb']:.0f} | "
              f"{100 * hits / (hits + miss) if hits + miss else float('nan'):.1f} | "
              f"{100 * l2h / (l2h + l2m) if l2h + l2m else float('nan'):.1f} |")
PY
}
if [ "${1:-}" = summarize ]; then
    summarize "${2:-$OUT}"
    exit 0
fi

WORK=${TB_WORK:-/var/tmp/topology-bench}
TOPOLOGIES=${TB_TOPOLOGIES:-"nocache baseline twovol twovol-origin twovol-hydrated bcache opencas zfs zfs-plain zfs-l2arc"}
WORKLOADS=${TB_WORKLOADS:-"tpcb select"}
SCALE=${TB_SCALE:-100}
CLIENTS=${TB_CLIENTS:-8}
JOBS=${TB_JOBS:-4}
WARMUP=${TB_WARMUP:-30}
DURATION=${TB_DURATION:-120}
SHARED_BUFFERS=${TB_SHARED_BUFFERS:-256MB}
MEM_LIMIT=${TB_MEM_LIMIT:-}
CACHE_PCT=${TB_CACHE_PCT:-25}
CACHE_BLOCK_KB=${TB_CACHE_BLOCK_KB:-32}
MIGRATION_THRESHOLD=${TB_MIGRATION_THRESHOLD:-}
CAS_MODE=${TB_CAS_MODE:-wi}
CAS_LINE_KB=${TB_CAS_LINE_KB:-$(( CACHE_BLOCK_KB > 64 ? 64 : CACHE_BLOCK_KB ))}
ORIGIN_DELAY_MS=${TB_ORIGIN_DELAY_MS:-10}
CHUNK_KB=${TB_CHUNK_KB:-64}
HYDRATE_JOBS=${TB_HYDRATE_JOBS:-8}
PG_USER=${TB_PG_USER:-postgres}
KEEP=${TB_KEEP:-}
# ~15 MiB per scale unit loaded. The origin holds the database plus up
# to max_wal_size of WAL; the pool can at most CoW the whole origin
# address space plus the twovol WAL volume. Images are SPARSE — a
# preflight below refuses to start unless the worst case fits, because
# a full disk under a loop device takes the whole host down (a runner
# VM died that way).
MAX_WAL_GB=${TB_MAX_WAL_GB:-1}
VOL_GB=${TB_VOL_GB:-$(( (SCALE * 16 + 1023) / 1024 + MAX_WAL_GB + 1 ))}
WAL_GB=${TB_WAL_GB:-$(( MAX_WAL_GB + 1 ))}
POOL_GB=${TB_POOL_GB:-$(( VOL_GB + WAL_GB + 1 ))}

# ZFS topologies. The pool is twice the ext4 volume: copy-on-write needs
# free space to turn over in, and the per-window snapshot pins every
# block the window overwrites.
ZFS_POOL=tbzpool
ZFS_RECORDSIZE=${TB_ZFS_RECORDSIZE:-8k}
ZFS_WAL_RECORDSIZE=${TB_ZFS_WAL_RECORDSIZE:-128k}
ZFS_COMPRESSION=${TB_ZFS_COMPRESSION:-lz4}
ZFS_FPW=${TB_ZFS_FPW:-off}
ZFS_ASHIFT=${TB_ZFS_ASHIFT:-12}
ZFS_GB=${TB_ZFS_GB:-$(( VOL_GB * 2 ))}
ZFS_WINDOW_SNAPSHOT=${TB_ZFS_WINDOW_SNAPSHOT:-1}
to_mb() { # "1G" | "256MB" | "512M" | bytes -> MiB
    python3 -c '
import re, sys
m = re.fullmatch(r"(\d+)\s*([kmg]?)b?", sys.argv[1].strip().lower())
if not m: sys.exit("cannot parse size: " + sys.argv[1])
n = int(m.group(1))
print({"": n >> 20, "k": n >> 10, "m": n, "g": n << 10}[m.group(2)])' "$1"
}
ZFS_ARC_MB=${TB_ZFS_ARC_MB:-}

if [ $(( CACHE_BLOCK_KB % 32 )) -ne 0 ] || [ "$CACHE_BLOCK_KB" -lt 32 ]; then
    echo "TB_CACHE_BLOCK_KB must be a multiple of 32 (dm-cache minimum)" >&2; exit 64
fi

ANY_THIN=""; ANY_ZFS=""
# shellcheck disable=SC2086  # TB_TOPOLOGIES is a space-separated list by contract
for t in $TOPOLOGIES; do
    case $t in
        nocache|baseline|twovol|twovol-origin|twovol-hydrated) ANY_THIN=1 ;;
        bcache) need make-bcache; ANY_THIN=1 ;;
        opencas) need casadm; ANY_THIN=1 ;;
        zfs|zfs-plain|zfs-l2arc) need zpool zfs tar; ANY_ZFS=1 ;;
        *) echo "unknown topology: $t" >&2; exit 64 ;;
    esac
done
if [ -n "$ANY_ZFS" ] && [ -z "$ZFS_ARC_MB" ] && [ -n "$MEM_LIMIT" ] && [ "$MEM_LIMIT" != max ]; then
    # The ARC is kernel memory outside postgres's cgroup. Give it what
    # the page cache can have in the other topologies: the cap minus
    # shared_buffers. 64 MiB is the module's own floor for zfs_arc_max.
    ZFS_ARC_MB=$(( $(to_mb "$MEM_LIMIT") - $(to_mb "$SHARED_BUFFERS") ))
    [ "$ZFS_ARC_MB" -lt 64 ] && ZFS_ARC_MB=64
fi

# Debian/Ubuntu keep server binaries out of PATH (pg_wrapper covers
# clients only); pick the newest cluster's bin dir there.
if command -v pg_ctl >/dev/null; then
    PG_BIN=$(dirname "$(command -v pg_ctl)")
else
    PG_BIN=$(printf '%s\n' /usr/lib/postgresql/*/bin | sort -V | tail -1)
fi
[ -x "$PG_BIN/pg_ctl" ] || { echo "no PostgreSQL server binaries (pg_ctl) found" >&2; exit 64; }
PGBENCH=$PG_BIN/pgbench; [ -x "$PGBENCH" ] || PGBENCH=$(command -v pgbench)
id -u "$PG_USER" >/dev/null 2>&1 || useradd -r -s /bin/bash -d "$WORK" "$PG_USER"

mkdir -p "$OUT" "$WORK"
# Preflight: origins fully written + pool worst case + cache + slack.
# The per-topology images are deleted between topologies, so the worst
# case is the larger of the thin pool and the ZFS pair (the pool image,
# and a COW store that copy-on-write can sweep across the whole vdev).
STACK_MB=0
[ -n "$ANY_THIN" ] && STACK_MB=$(( POOL_GB * 1024 ))
if [ -n "$ANY_ZFS" ] && [ $(( ZFS_GB * 2 * 1024 + 64 )) -gt "$STACK_MB" ]; then
    STACK_MB=$(( ZFS_GB * 2 * 1024 + 64 ))
fi
NEED_MB=$(( (VOL_GB + WAL_GB) * 1024 + STACK_MB + SCALE * 16 * CACHE_PCT / 100 + 1024 ))
AVAIL_MB=$(df -Pm "$WORK" | awk 'NR==2 {print $4}')
if [ "$AVAIL_MB" -lt "$NEED_MB" ]; then
    echo "not enough space under $WORK: ${AVAIL_MB} MiB free, worst case needs ${NEED_MB} MiB" \
         "(lower TB_SCALE / TB_MAX_WAL_GB, or point TB_WORK at a bigger disk)" >&2
    exit 64
fi
MNT="$WORK/mnt"; MNT_WAL="$WORK/mnt-wal"; SOCK="$WORK/sock"
mkdir -p "$MNT" "$MNT_WAL" "$SOCK"
chown "$PG_USER" "$SOCK" "$WORK"
PGDATA="$MNT/data"
CGROUP=""
if [ -n "$MEM_LIMIT" ]; then
    [ -f /sys/fs/cgroup/cgroup.controllers ] || { echo "TB_MEM_LIMIT needs cgroup v2 at /sys/fs/cgroup" >&2; exit 64; }
    CGROUP=/sys/fs/cgroup/topology-bench
    mkdir -p "$CGROUP"
    echo "$MEM_LIMIT" > "$CGROUP/memory.max"
fi

for m in dm_thin_pool dm_cache dm_cache_smq; do modprobe "$m" 2>/dev/null || true; done
[ "$ORIGIN_DELAY_MS" -gt 0 ] && { modprobe dm_delay 2>/dev/null || true; }
if [ -n "$ANY_ZFS" ]; then
    for m in dm_snapshot zfs; do modprobe "$m" 2>/dev/null || true; done
    [ -r /proc/spl/kstat/zfs/arcstats ] || { echo "the zfs kernel module is not loaded (modprobe zfs)" >&2; exit 64; }
fi

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
as_pg() { su -s /bin/bash "$PG_USER" -c "$*"; }
psqlq() { as_pg "'$PG_BIN/psql' -h '$SOCK' -d bench -Atq -c \"$1\"" 2>/dev/null; }
sectors() { blockdev --getsz "$1"; }
dm_blk() { dmsetup info -c --noheadings -o blkdevname "$1"; }
mb() { echo $(( $1 << 20 )); }
attach_loop() {
    losetup --find --show --direct-io=on "$1" 2>/dev/null || losetup --find --show "$1"
}
mkimage() { # file bytes — sparse on purpose (see the sizing note)
    rm -f "$1"
    truncate -s "$2" "$1"
}
zero_head() { # device: zero the first 4 KiB, BYPASSING the bdev page cache.
    # A buffered dd leaves a dirty page in the block device's page cache
    # that writeback lands later — on top of the superblock dm-thin or
    # dm-cache has meanwhile written with direct bios (seen as
    # "cache metadata: sb_check failed: magic 0" mid-run).
    "$DD" if=/dev/zero of="$1" bs=4096 count=1 conv=notrunc,fsync oflag=direct status=none
}
wait_gone() { # path, seconds
    local i=0
    while [ -e "$1" ] && [ $i -lt "$2" ]; do sleep 1; i=$((i + 1)); done
    [ ! -e "$1" ]
}
wait_for() { # path, seconds
    local i=0
    while [ ! -e "$1" ] && [ $i -lt "$2" ]; do sleep 1; i=$((i + 1)); done
    [ -e "$1" ]
}

# ---------------------------------------------------------------- state
ORIGIN_LOOP=""; POOL_LOOP=""; CACHE_LOOP=""; BSB_LOOP=""
ORIGIN_DEV=""; FS_DEV=""; TWO_VOL=""; WAL_FROM_ORIGIN=""; TOPO=""
WAL_ORIGIN_LOOP=""; WAL_ORIGIN_DEV=""
BCACHE_DEV=""; BCACHE_CSET=""; CAS_STARTED=""
ZFS_UP=""; ZORIGIN_LOOP=""; ZSRC_LOOP=""; ZFS_WIN=""; ZFS_ARC_SAVED=""
STAT_ORIGIN=""; STAT_POOL=""   # diskstats names, when not the thin stack's
DB_MB=0

# ---------------------------------------------------------- teardown
stop_pg() {
    if [ -f "$PGDATA/postmaster.pid" ]; then
        as_pg "'$PG_BIN/pg_ctl' -D '$PGDATA' stop -m fast -w" >/dev/null 2>&1 || true
    fi
}
teardown_topology() {
    local errexit=0; case $- in *e*) errexit=1 ;; esac
    set +e
    stop_pg
    umount "$MNT" 2>/dev/null; umount "$MNT_WAL" 2>/dev/null
    umount "$WORK/mnt-src" 2>/dev/null
    if [ -n "$ZFS_UP" ]; then
        # The pool is scratch: if it will not export, destroy it rather
        # than leave an imported pool holding the dm devices below.
        zpool export -f "$ZFS_POOL" 2>/dev/null || zpool destroy -f "$ZFS_POOL" 2>/dev/null \
            || log "WARNING: pool $ZFS_POOL is still imported"
        ZFS_UP=""
    fi
    if [ -n "$ZFS_ARC_SAVED" ]; then
        # shellcheck disable=SC2086  # "min max", two words on purpose
        set -- $ZFS_ARC_SAVED
        echo "$2" > /sys/module/zfs/parameters/zfs_arc_max 2>/dev/null
        echo "$1" > /sys/module/zfs/parameters/zfs_arc_min 2>/dev/null
        ZFS_ARC_SAVED=""
    fi
    if [ -n "$CAS_STARTED" ]; then
        casadm --stop-cache --cache-id 1 >/dev/null 2>&1 || casadm --stop-cache --cache-id 1 --no-data-flush >/dev/null 2>&1
        CAS_STARTED=""
    fi
    if [ -n "$BCACHE_DEV" ]; then
        echo 1 > "/sys/block/$(basename "$BCACHE_DEV")/bcache/stop" 2>/dev/null
        [ -n "$BCACHE_CSET" ] && echo 1 > "/sys/fs/bcache/$BCACHE_CSET/stop" 2>/dev/null
        wait_gone "$BCACHE_DEV" 30 || log "WARNING: $BCACHE_DEV did not go away"
        BCACHE_DEV=""; BCACHE_CSET=""
    fi
    for d in tb-rcache tb-cmeta tb-cdata tb-bback tb-ccache tb-wal tb-data tb-pool tb-tdata tb-tmeta \
             tb-zdev tb-zorigin; do
        dmsetup remove "$d" 2>/dev/null
    done
    for l in "$POOL_LOOP" "$CACHE_LOOP" "$BSB_LOOP" "$ZORIGIN_LOOP" "$ZSRC_LOOP"; do
        [ -n "$l" ] && losetup -d "$l" 2>/dev/null
    done
    # The ZFS origin is per topology (its properties differ), and big.
    rm -f "$WORK/zfs-origin.img"
    POOL_LOOP=""; CACHE_LOOP=""; BSB_LOOP=""; FS_DEV=""; TWO_VOL=""; WAL_FROM_ORIGIN=""
    ZORIGIN_LOOP=""; ZSRC_LOOP=""; ZFS_WIN=""; STAT_ORIGIN=""; STAT_POOL=""
    [ "$errexit" = 1 ] && set -e
    return 0
}
teardown_all() {
    rc=$?
    set +e
    if [ "$rc" -ne 0 ] && [ -n "$KEEP" ]; then
        log "FAILED (rc=$rc) — TB_KEEP set, leaving the stack up: dmsetup ls; losetup -a"
        exit "$rc"
    fi
    teardown_topology
    dmsetup remove tb-origin 2>/dev/null
    dmsetup remove tb-walorigin 2>/dev/null
    [ -n "$ORIGIN_LOOP" ] && losetup -d "$ORIGIN_LOOP" 2>/dev/null
    [ -n "$WAL_ORIGIN_LOOP" ] && losetup -d "$WAL_ORIGIN_LOOP" 2>/dev/null
    [ -n "$CGROUP" ] && rmdir "$CGROUP" 2>/dev/null
    if [ "$rc" -ne 0 ]; then
        log "topology-bench FAILED (rc=$rc) — recent kernel messages:"
        dmesg 2>/dev/null | grep -iE 'device-mapper|dm-[0-9]|loop[0-9]|bcache|cas_|OCF|EXT4|ZFS|SPL|zio|WARN|BUG|Call Trace|error' | tail -40 >&2
    fi
    exit "$rc"
}
trap teardown_all EXIT

# ------------------------------------------------- origin: the database
pg_conf() {
    cat >> "$PGDATA/postgresql.conf" <<EOF
listen_addresses = ''
unix_socket_directories = '$SOCK'
shared_buffers = $SHARED_BUFFERS
checkpoint_timeout = 60s
max_wal_size = ${MAX_WAL_GB}GB
min_wal_size = 256MB
wal_compression = off
synchronous_commit = on
autovacuum = on
track_io_timing = on
log_checkpoints = on
logging_collector = off
EOF
}
start_pg() {
    as_pg "'$PG_BIN/pg_ctl' -D '$PGDATA' -l '$WORK/pg.log' start -w" >/dev/null
    # Memory cap: move the whole postmaster tree in AFTER the start —
    # `su` opens a PAM session, and pam_systemd parks it in a fresh
    # scope, so a cgroup joined before the su is silently escaped.
    # Backends forked later inherit the postmaster's cgroup.
    if [ -n "$CGROUP" ]; then
        local p
        for p in $(pgrep -u "$PG_USER" postgres); do
            echo "$p" > "$CGROUP/cgroup.procs" 2>/dev/null || true
        done
        log "memory cap $MEM_LIMIT on $(wc -l < "$CGROUP/cgroup.procs") postgres processes"
    fi
}
prepare_origin() {
    local img="$WORK/origin.img" wimg="$WORK/wal-origin.img" ready="$WORK/origin.ready"
    if [ -n "${TB_REUSE_ORIGIN:-}" ] && [ -f "$ready" ] && grep -q "^scale=$SCALE\$" "$ready" \
       && grep -q "^wal_gb=$WAL_GB\$" "$ready" && [ -f "$wimg" ]; then
        log "reusing origin images ($img, $wimg)"
    else
        log "building origin image: pgbench scale $SCALE on a ${VOL_GB}G ext4"
        rm -f "$ready"
        mkimage "$img" "$(mb $((VOL_GB * 1024)))"
        local l; l=$(attach_loop "$img")
        mkfs.ext4 -q -F "$l"
        mount -o noatime "$l" "$MNT"
        mkdir -p "$PGDATA"; chown "$PG_USER" "$PGDATA"
        as_pg "'$PG_BIN/initdb' -D '$PGDATA' -A trust -U '$PG_USER'" >/dev/null
        pg_conf
        start_pg
        as_pg "'$PG_BIN/createdb' -h '$SOCK' bench"
        as_pg "'$PGBENCH' -h '$SOCK' -i -s $SCALE -q bench" 2>&1 | tail -1 >&2
        psqlq "CHECKPOINT" >/dev/null
        local dbmb; dbmb=$(du -sm "$PGDATA/base" | cut -f1)
        stop_pg
        # The WAL origin: what a snapshot of a separate pg_wal volume
        # holds — the same segments, recycled ones included, on their
        # own ext4. The data image keeps its pg_wal too, for the
        # single-volume topologies.
        log "building WAL origin image: $(du -sm "$PGDATA/pg_wal" | cut -f1) MiB of pg_wal on a ${WAL_GB}G ext4"
        mkimage "$wimg" "$(mb $((WAL_GB * 1024)))"
        local w; w=$(attach_loop "$wimg")
        mkfs.ext4 -q -F "$w"
        mount -o noatime "$w" "$MNT_WAL"
        cp -a "$PGDATA/pg_wal" "$MNT_WAL/pg_wal"
        umount "$MNT_WAL"
        losetup -d "$w"
        umount "$MNT"
        losetup -d "$l"
        printf 'scale=%s\ndb_mb=%s\nwal_gb=%s\n' "$SCALE" "$dbmb" "$WAL_GB" > "$ready"
    fi
    DB_MB=$(sed -n 's/^db_mb=//p' "$ready")
    ORIGIN_LOOP=$(attach_loop "$img")
    blockdev --setro "$ORIGIN_LOOP"
    ORIGIN_DEV="$ORIGIN_LOOP"
    if [ "$ORIGIN_DELAY_MS" -gt 0 ]; then
        # Reads delayed, writes (there are none: the thin only reads its
        # origin) untouched. This sits UNDER the thin, so the thin's own
        # copy-on-write of a chunk on first write pays it too — as the
        # real stack does against the NBD attach.
        dmsetup create --readonly tb-origin --table \
            "0 $(sectors "$ORIGIN_LOOP") delay $ORIGIN_LOOP 0 $ORIGIN_DELAY_MS $ORIGIN_LOOP 0 0"
        ORIGIN_DEV=/dev/mapper/tb-origin
    fi
    WAL_ORIGIN_LOOP=$(attach_loop "$wimg")
    blockdev --setro "$WAL_ORIGIN_LOOP"
    WAL_ORIGIN_DEV="$WAL_ORIGIN_LOOP"
    if [ "$ORIGIN_DELAY_MS" -gt 0 ]; then
        dmsetup create --readonly tb-walorigin --table \
            "0 $(sectors "$WAL_ORIGIN_LOOP") delay $WAL_ORIGIN_LOOP 0 $ORIGIN_DELAY_MS $WAL_ORIGIN_LOOP 0 0"
        WAL_ORIGIN_DEV=/dev/mapper/tb-walorigin
    fi
    log "origin ready: database ${DB_MB} MiB, origin device $ORIGIN_DEV, WAL origin device $WAL_ORIGIN_DEV"
}

# ------------------------------------------- pool + thin (per topology)
build_pool() {
    mkimage "$WORK/pool.img" "$(mb $((POOL_GB * 1024)))"
    POOL_LOOP=$(attach_loop "$WORK/pool.img")
    local total meta chunk data
    total=$(sectors "$POOL_LOOP")
    meta=$(( 256 << 11 ))                       # 256 MiB of pool metadata
    chunk=$(( CHUNK_KB * 2 ))
    data=$(( (total - meta) / chunk * chunk ))
    zero_head "$POOL_LOOP"   # fresh metadata ("format me")
    dmsetup create tb-tmeta --table "0 $meta linear $POOL_LOOP 0"
    dmsetup create tb-tdata --table "0 $data linear $POOL_LOOP $meta"
    # Same feature set the agent's provision::build uses.
    dmsetup create tb-pool --table \
        "0 $data thin-pool /dev/mapper/tb-tmeta /dev/mapper/tb-tdata $chunk 0 2 skip_block_zeroing no_discard_passdown"
    dmsetup message tb-pool 0 "create_thin 0"
    dmsetup create tb-data --table "0 $(sectors "$ORIGIN_LOOP") thin /dev/mapper/tb-pool 0 $ORIGIN_DEV"
}
build_wal_volume() { # [origin]: a fresh thin, or a thin over the WAL origin image
    dmsetup message tb-pool 0 "create_thin 1"
    if [ "${1:-}" = origin ]; then
        dmsetup create tb-wal --table "0 $(sectors "$WAL_ORIGIN_LOOP") thin /dev/mapper/tb-pool 1 $WAL_ORIGIN_DEV"
        WAL_FROM_ORIGIN=1
    else
        dmsetup create tb-wal --table "0 $(( WAL_GB << 21 )) thin /dev/mapper/tb-pool 1"
        mkfs.ext4 -q -F /dev/mapper/tb-wal
    fi
    TWO_VOL=1
}
hydrate_thin() { # dm name: map every chunk locally, without reading the origin twice.
    # dm-thin copies a chunk from the external origin on the first
    # write to it UNLESS the write covers the whole chunk (schedule_copy:
    # io_overwrites_block → remap_and_issue_overwrite). So a read of
    # each region followed by a whole-chunk write of what was read
    # pulls the volume local at the origin's bandwidth, once, in
    # parallel — what an agent would do on attach for a volume whose
    # write set is the whole volume.
    local dev=/dev/mapper/$1 total per i start t0
    total=$(( $(sectors "$dev") / 2048 ))          # MiB
    per=$(( (total + HYDRATE_JOBS - 1) / HYDRATE_JOBS ))
    t0=$(date +%s)
    for (( i = 0; i < HYDRATE_JOBS; i++ )); do
        start=$(( i * per ))
        [ "$start" -ge "$total" ] && break
        "$DD" if="$dev" of="$dev" bs=1M skip="$start" seek="$start" count="$per" \
           iflag=direct oflag=direct conv=notrunc status=none &
    done
    wait
    sync
    log "$1: hydrated ${total} MiB in $(( $(date +%s) - t0 ))s ($HYDRATE_JOBS streams); pool: $(dmsetup status tb-pool | awk '{print $6}') data blocks used/total"
}
build_cache_dev() {
    local bytes=$(( DB_MB * CACHE_PCT / 100 ))
    [ "$bytes" -lt 64 ] && bytes=64
    mkimage "$WORK/cache.img" "$(mb "$bytes")"
    CACHE_LOOP=$(attach_loop "$WORK/cache.img")
    log "cache device: ${bytes} MiB ($CACHE_PCT% of the database) on $CACHE_LOOP"
}

# ------------------------------------------------------- ZFS stack
attach_loop_ro() {
    losetup --find --show --read-only --direct-io=on "$1" 2>/dev/null || losetup --find --show --read-only "$1"
}
arcstat() { awk -v k="$1" '$1 == k { print $3 }' /proc/spl/kstat/zfs/arcstats; }
zfs_cap_arc() {
    [ -n "$ZFS_ARC_MB" ] || { log "ARC uncapped (no TB_MEM_LIMIT / TB_ZFS_ARC_MB): c_max=$(( $(arcstat c_max) >> 20 )) MiB"; return 0; }
    local p=/sys/module/zfs/parameters
    ZFS_ARC_SAVED="$(cat $p/zfs_arc_min) $(cat $p/zfs_arc_max)"
    # Lower the floor first: zfs_arc_max is ignored unless it is above
    # arc_c_min, which defaults to 1/32 of RAM.
    echo $(( 32 << 20 )) > $p/zfs_arc_min 2>/dev/null || true
    echo $(( ZFS_ARC_MB << 20 )) > $p/zfs_arc_max 2>/dev/null || true
    local got=$(( $(arcstat c_max) >> 20 ))
    [ "$got" = "$ZFS_ARC_MB" ] || log "WARNING: asked for a ${ZFS_ARC_MB} MiB ARC, the module reports c_max=${got} MiB"
    log "ARC capped: c_max=${got} MiB (TB_MEM_LIMIT=${MEM_LIMIT:-unset} - shared_buffers=$SHARED_BUFFERS)"
}
build_zfs_origin() { # compression full_page_writes: an exported pool image holding the database
    local comp=$1 fpw=$2 img="$WORK/zfs-origin.img" srcmnt="$WORK/mnt-src"
    log "building ZFS origin: ${ZFS_GB}G pool, ashift=$ZFS_ASHIFT recordsize=$ZFS_RECORDSIZE (wal $ZFS_WAL_RECORDSIZE) compression=$comp full_page_writes=$fpw ($(zfs version 2>/dev/null | tr '\n' ' '))"
    zpool export -f "$ZFS_POOL" 2>/dev/null || true   # a previous run's leftover
    mkimage "$img" "$(mb $((ZFS_GB * 1024)))"
    ZORIGIN_LOOP=$(attach_loop "$img")
    zpool create -f -o ashift="$ZFS_ASHIFT" -o cachefile=none \
        -O mountpoint=none -O atime=off -O xattr=sa -O redundant_metadata=most \
        -O compression="$comp" -O recordsize="$ZFS_RECORDSIZE" "$ZFS_POOL" "$ZORIGIN_LOOP"
    ZFS_UP=1
    zfs create -o mountpoint="$MNT" "$ZFS_POOL/data"
    zfs create -o mountpoint="$MNT_WAL" -o recordsize="$ZFS_WAL_RECORDSIZE" "$ZFS_POOL/wal"
    # The same database the other topologies run, copied from the ext4
    # origin image through a second read-only loop (the first is claimed
    # by the dm-delay). pg_wal goes to its own dataset.
    ZSRC_LOOP=$(attach_loop_ro "$WORK/origin.img")
    mkdir -p "$srcmnt"
    mount -o ro,noload "$ZSRC_LOOP" "$srcmnt"
    install -d -o "$PG_USER" -m 700 "$PGDATA"
    tar -C "$srcmnt/data" --exclude=./pg_wal -cf - . | tar -C "$PGDATA" -xpf -
    cp -a "$srcmnt/data/pg_wal" "$MNT_WAL/pg_wal"
    ln -s "$MNT_WAL/pg_wal" "$PGDATA/pg_wal"
    chown -h "$PG_USER" "$PGDATA/pg_wal"
    umount "$srcmnt"
    losetup -d "$ZSRC_LOOP"; ZSRC_LOOP=""
    # Later lines win. wal_recycle/wal_init_zero off is the setting for
    # any copy-on-write filesystem: a recycled segment is a file of old
    # records to overwrite in place, which is exactly what CoW cannot do.
    cat >> "$PGDATA/postgresql.conf" <<EOF
full_page_writes = $fpw
wal_init_zero = off
wal_recycle = off
EOF
    zpool sync "$ZFS_POOL"
    log "ZFS origin datasets (name used logicalused ratio): $(zfs list -H -o name,used,logicalused,compressratio -r "$ZFS_POOL" | tr '\t\n' ' ;')"
    zpool export "$ZFS_POOL"; ZFS_UP=""
    losetup -d "$ZORIGIN_LOOP"; ZORIGIN_LOOP=""
}
build_zfs_stack() { # the pool on a no-read-before-write device over the delayed origin image
    ZORIGIN_LOOP=$(attach_loop_ro "$WORK/zfs-origin.img")
    local odev=$ZORIGIN_LOOP sect chunk
    sect=$(sectors "$ZORIGIN_LOOP")
    if [ "$ORIGIN_DELAY_MS" -gt 0 ]; then
        dmsetup create --readonly tb-zorigin --table \
            "0 $sect delay $ZORIGIN_LOOP 0 $ORIGIN_DELAY_MS $ZORIGIN_LOOP 0 0"
        odev=/dev/mapper/tb-zorigin
    fi
    # The COW store is local flash. Transient ("N"): no on-disk
    # exception metadata, so a flush is the data device's flush alone.
    # Sized to the whole vdev because copy-on-write can sweep it.
    mkimage "$WORK/pool.img" "$(mb $((ZFS_GB * 1024 + 64)))"
    POOL_LOOP=$(attach_loop "$WORK/pool.img")
    chunk=$(( (1 << ZFS_ASHIFT) / 512 ))
    dmsetup create tb-zdev --table "0 $sect snapshot $odev $POOL_LOOP N $chunk"
    STAT_ORIGIN=$(basename "$ZORIGIN_LOOP"); STAT_POOL=$(basename "$POOL_LOOP")
    zfs_cap_arc
    local t0; t0=$(date +%s%N)
    zpool import -N -o cachefile=none -d /dev/mapper/tb-zdev "$ZFS_POOL"
    ZFS_UP=1
    log "zpool import over the ${ORIGIN_DELAY_MS} ms origin took $(( ($(date +%s%N) - t0) / 1000000 )) ms"
    FS_DEV=/dev/mapper/tb-zdev
}
zfs_stats() { # counters as "zfsc key n" (the window's delta), gauges as "zfsg key n"
    awk 'NR > 2 { print "zfsc zil." $1, $3 }' /proc/spl/kstat/zfs/zil
    awk 'NR > 2 && $1 ~ /^(hits|iohits|misses|l2_hits|l2_misses|l2_read_bytes|l2_write_bytes)$/ { print "zfsc arc." $1, $3 }
         NR > 2 && $1 ~ /^(size|c_max|compressed_size|uncompressed_size|l2_size|l2_asize)$/ { print "zfsg arc." $1, $3 }' \
        /proc/spl/kstat/zfs/arcstats
    # Growth of the COW store = bytes written to LBAs never written
    # since the stage. dm-snapshot status: <allocated>/<total> sectors.
    echo "zfsc cow.allocated_bytes $(dmsetup status tb-zdev | awk '{ split($4, a, "/"); print a[1] * 512 }')"
    echo "zfsg pool.allocated $(zpool get -Hp -o value allocated "$ZFS_POOL")"
    echo "zfsg pool.fragmentation $(zpool get -Hp -o value fragmentation "$ZFS_POOL" | tr -dc '0-9')"
    if [ -n "$ZFS_WIN" ]; then
        local w=0 ds
        for ds in data wal; do
            w=$(( w + $(zfs get -Hp -o value "written@$ZFS_WIN" "$ZFS_POOL/$ds") ))
        done
        echo "zfsg window.written_bytes $w"
    fi
    echo "zfsinfo $(zfs list -H -o name,used,logicalused,compressratio -r "$ZFS_POOL" | tr '\t\n' ' ;')"
}

# ------------------------------------------------------- topologies
topo_nocache() { FS_DEV=/dev/mapper/tb-data; }

topo_baseline() {
    build_cache_dev
    local total meta data block
    total=$(sectors "$CACHE_LOOP")
    meta=$(( total / 500 / 8 * 8 )); [ "$meta" -lt $(( 8 << 11 )) ] && meta=$(( 8 << 11 ))
    block=$(( CACHE_BLOCK_KB * 2 ))
    data=$(( (total - meta) / block * block ))
    zero_head "$CACHE_LOOP"  # "format me"
    dmsetup create tb-cmeta --table "0 $meta linear $CACHE_LOOP 0"
    dmsetup create tb-cdata --table "0 $data linear $CACHE_LOOP $meta"
    dmsetup create tb-rcache --table \
        "0 $(sectors /dev/mapper/tb-data) cache /dev/mapper/tb-cmeta /dev/mapper/tb-cdata /dev/mapper/tb-data $block 1 writethrough smq 0"
    if [ -n "$MIGRATION_THRESHOLD" ]; then
        dmsetup message tb-rcache 0 "migration_threshold $MIGRATION_THRESHOLD"
    fi
    FS_DEV=/dev/mapper/tb-rcache
}

topo_twovol() { topo_baseline; build_wal_volume; }
topo_twovol_origin() { topo_baseline; build_wal_volume origin; }
topo_twovol_hydrated() { topo_baseline; build_wal_volume origin; hydrate_thin tb-wal; }

topo_bcache() {
    build_cache_dev
    # Two-segment backing: 16 sectors of local carve for bcache's own
    # superblock, then the thin. With the default --data-offset of 16
    # the bcache device IS the thin, byte for byte, and the thin's
    # origin never sees a bcache signature.
    mkimage "$WORK/bsb.img" "$(mb 1)"
    BSB_LOOP=$(attach_loop "$WORK/bsb.img")
    printf '0 16 linear %s 0\n16 %s linear /dev/mapper/tb-data 0\n' \
        "$BSB_LOOP" "$(sectors /dev/mapper/tb-data)" | dmsetup create tb-bback
    local out
    out=$(make-bcache --wipe-bcache -C "$CACHE_LOOP" -B /dev/mapper/tb-bback --data-offset 16 2>&1) \
        || { echo "$out" >&2; return 1; }
    # udev may already have registered either device; the register
    # write then fails harmlessly.
    echo "$CACHE_LOOP" > /sys/fs/bcache/register 2>/dev/null || true
    echo /dev/mapper/tb-bback > /sys/fs/bcache/register 2>/dev/null || true
    local backing i b
    backing=$(dm_blk tb-bback)
    for i in $(seq 1 30); do
        for b in /sys/block/bcache*; do
            [ -e "$b/bcache" ] || continue
            if [ "$(basename "$(readlink -f "$b/bcache/../")")" = "$backing" ] || \
               readlink -f "$b/bcache" | grep -q "/$backing/"; then
                BCACHE_DEV="/dev/$(basename "$b")"
            fi
        done
        [ -n "$BCACHE_DEV" ] && [ -e "$BCACHE_DEV" ] && break
        sleep 1
    done
    [ -n "$BCACHE_DEV" ] || { echo "bcache device for tb-bback never appeared" >&2; return 1; }
    local sysb; sysb="/sys/block/$(basename "$BCACHE_DEV")/bcache"
    wait_for "$sysb/cache" 30 || { echo "bcache backing not attached to its cache set" >&2; return 1; }
    BCACHE_CSET=$(basename "$(readlink -f "$sysb/cache")")
    echo writearound > "$sysb/cache_mode"
    echo 0 > "$sysb/sequential_cutoff"
    echo 0 > "/sys/fs/bcache/$BCACHE_CSET/congested_read_threshold_us"
    echo 0 > "/sys/fs/bcache/$BCACHE_CSET/congested_write_threshold_us"
    log "bcache: $BCACHE_DEV mode=$(cat "$sysb/cache_mode") cset=$BCACHE_CSET"
    FS_DEV="$BCACHE_DEV"
}

by_id_of() { # dm name -> /dev/disk/by-id/dm-name-<name>, made by hand when udev did not
    local link="/dev/disk/by-id/dm-name-$1"
    wait_for "$link" 10 || { mkdir -p /dev/disk/by-id; ln -sfn "/dev/mapper/$1" "$link"; }
    echo "$link"
}
topo_opencas() {
    build_cache_dev
    modprobe cas_cache 2>/dev/null || true
    # casadm only accepts /dev/disk/by-id or by-path links (it refuses a
    # raw /dev/loopN, which has neither); dm devices get dm-name-* links.
    dmsetup create tb-ccache --table "0 $(sectors "$CACHE_LOOP") linear $CACHE_LOOP 0"
    local cachedev core
    cachedev=$(by_id_of tb-ccache); core=$(by_id_of tb-data)
    if ! casadm --start-cache --cache-device "$cachedev" --cache-mode "$CAS_MODE" \
            --cache-line-size "$CAS_LINE_KB" --cache-id 1 --force; then
        [ "$CAS_MODE" = wi ] && echo "casadm rejected mode wi: build Open CAS with 'make CAS_EXT_EXP=1' (write-invalidate is compile-gated), or set TB_CAS_MODE=wa/wt" >&2
        return 1
    fi
    CAS_STARTED=1
    casadm --add-core --cache-id 1 --core-device "$core" --core-id 1
    casadm --set-param --name seq-cutoff --cache-id 1 --core-id 1 --seq-policy never 2>/dev/null \
        || casadm --set-param --name seq-cutoff --cache-id 1 --core-id 1 --policy never \
        || log "WARNING: could not disable the Open CAS sequential cutoff (flag names vary by release); sequential reads above the default threshold will bypass the cache"
    wait_for /dev/cas1-1 30 || { echo "/dev/cas1-1 never appeared" >&2; return 1; }
    log "opencas: /dev/cas1-1 mode=$CAS_MODE line=${CAS_LINE_KB}K"
    FS_DEV=/dev/cas1-1
}

topo_zfs() { build_zfs_origin "$ZFS_COMPRESSION" "$ZFS_FPW"; build_zfs_stack; }
topo_zfs_plain() { build_zfs_origin off on; build_zfs_stack; }
topo_zfs_l2arc() {
    topo_zfs
    build_cache_dev
    # Kernel defaults for the feed (l2arc_write_max, l2arc_noprefetch):
    # record-only, like dm-cache's migration_threshold above.
    zpool add -f "$ZFS_POOL" cache "$CACHE_LOOP"
    log "l2arc: $CACHE_LOOP write_max=$(cat /sys/module/zfs/parameters/l2arc_write_max) noprefetch=$(cat /sys/module/zfs/parameters/l2arc_noprefetch)"
}

# --------------------------------------------------------- measuring
diskstats_line() { # kernel device name -> "reads rsect writes wsect flush"
    awk -v d="$1" '$3==d { f = (NF >= 19) ? $19 : 0; print $4, $6, $8, $10, f }' /proc/diskstats
}
snapshot() { # file
    {
        echo "dev origin $(diskstats_line "${STAT_ORIGIN:-$(basename "$ORIGIN_LOOP")}")"
        echo "dev walorigin $(diskstats_line "$(basename "$WAL_ORIGIN_LOOP")")"
        echo "dev pool $(diskstats_line "${STAT_POOL:-$(dm_blk tb-tdata)}")"
        [ -n "$TWO_VOL" ] && echo "dev wal $(diskstats_line "$(dm_blk tb-wal)")"
        [ -n "$CACHE_LOOP" ] && echo "dev cache $(diskstats_line "$(basename "$CACHE_LOOP")")"
        echo "dev fs $(diskstats_line "$(basename "$(readlink -f "$FS_DEV")")")"
        echo "pg wal_bytes $(psqlq "select wal_bytes from pg_stat_wal" || echo 0)"
        echo "pg xact_commit $(psqlq "select xact_commit from pg_stat_database where datname='bench'" || echo 0)"
        echo "pg blks_read $(psqlq "select blks_read from pg_stat_database where datname='bench'" || echo 0)"
        echo "pg blks_hit $(psqlq "select blks_hit from pg_stat_database where datname='bench'" || echo 0)"
        echo "pg rel_writes $(psqlq "select coalesce(sum(writes),0) from pg_stat_io where object='relation'" || echo 0)"
        echo "pg rel_reads $(psqlq "select coalesce(sum(reads),0) from pg_stat_io where object='relation'" || echo 0)"
        case $TOPO in
            baseline|twovol*) echo "dmcache $(dmsetup status tb-rcache)" ;;
            bcache) echo "bcache $(cat "/sys/block/$(basename "$BCACHE_DEV")/bcache/stats_total/cache_hits") $(cat "/sys/block/$(basename "$BCACHE_DEV")/bcache/stats_total/cache_misses") $(cat "/sys/block/$(basename "$BCACHE_DEV")/bcache/stats_total/cache_bypass_hits") $(cat "/sys/block/$(basename "$BCACHE_DEV")/bcache/stats_total/cache_bypass_misses")" ;;
            opencas) casadm --stats --cache-id 1 --core-id 1 --output-format csv 2>/dev/null | sed 's/^/cas /' ;;
            zfs*) zfs_stats ;;
        esac
    } > "$1"
}

run_workload() { # topology workload
    local topo=$1 wl=$2 builtin tag="$1-$2"
    case $wl in
        tpcb) builtin=tpcb-like ;;
        select) builtin=select-only ;;
        update) builtin=simple-update ;;
        *) echo "unknown workload $wl (tpcb|select|update)" >&2; return 1 ;;
    esac
    log "$tag: warm-up ${WARMUP}s"
    as_pg "'$PGBENCH' -h '$SOCK' -T $WARMUP -c $CLIENTS -j $JOBS -b $builtin bench" >/dev/null 2>&1
    sync; sleep 2
    if [ -n "$ZFS_UP" ] && [ "$ZFS_WINDOW_SNAPSHOT" = 1 ]; then
        # One atomic snapshot of both datasets marks the window's start;
        # `written@` at the end is then the live bytes born in it.
        ZFS_WIN="tb-$wl"
        zfs snapshot -r "$ZFS_POOL@$ZFS_WIN"
    fi
    snapshot "$OUT/$tag.before"
    log "$tag: measuring ${DURATION}s"
    as_pg "'$PGBENCH' -h '$SOCK' -T $DURATION -c $CLIENTS -j $JOBS -P 30 -b $builtin bench" > "$OUT/$tag.pgbench" 2>&1
    sync; sleep 2
    snapshot "$OUT/$tag.after"
    if [ -n "$ZFS_WIN" ]; then
        zfs destroy -r "$ZFS_POOL@$ZFS_WIN"
        ZFS_WIN=""
    fi
    python3 - "$OUT/$tag" "$topo" "$wl" "$DURATION" > "$OUT/$tag.json" <<'PY'
import json, re, sys
base, topo, wl, dur = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
zc, zg = {}, {}   # ZFS counters / gauges, keyed by file
def load(p):
    dev, pg, other = {}, {}, {}
    zc[p], zg[p] = {}, {}
    for line in open(p):
        f = line.split()
        if not f: continue
        if f[0] == 'dev': dev[f[1]] = [int(x) for x in f[2:7]]
        elif f[0] == 'pg': pg[f[1]] = int(f[2]) if len(f) > 2 and f[2].isdigit() else 0
        elif f[0] in ('zfsc', 'zfsg'):
            if len(f) > 2 and f[2].isdigit(): (zc if f[0] == 'zfsc' else zg)[p][f[1]] = int(f[2])
        else: other.setdefault(f[0], []).append(line.strip())
    return dev, pg, other
bd, bp, bo = load(base + '.before'); ad, ap, ao = load(base + '.after')
MB = 1 << 20
r = {'topology': topo, 'workload': wl, 'duration_s': dur, 'devices': {}, 'pg': {}}
for name in ad:
    d = [a - b for a, b in zip(ad[name], bd.get(name, [0]*5))]
    r['devices'][name] = {'read_ios': d[0], 'read_mb': d[1]*512/MB, 'write_ios': d[2],
                          'write_mb': d[3]*512/MB, 'flushes': d[4]}
for k in ap:
    r['pg'][k] = ap[k] - bp.get(k, 0)
r['pg']['wal_mb'] = r['pg'].get('wal_bytes', 0) / MB
r['pg']['rel_write_mb'] = r['pg'].get('rel_writes', 0) * 8192 / MB
r['pg']['rel_read_mb'] = r['pg'].get('rel_reads', 0) * 8192 / MB
txt = open(base + '.pgbench').read()
m = re.search(r'tps = ([\d.]+)', txt); r['tps'] = float(m.group(1)) if m else None
m = re.search(r'latency average = ([\d.]+)', txt); r['latency_ms'] = float(m.group(1)) if m else None
r['cache_stats'] = {'before': bo, 'after': ao}
if zc[base + '.after'] or zg[base + '.after']:
    zb, za = zc[base + '.before'], zc[base + '.after']
    r['zfs'] = {'delta': {k: za[k] - zb.get(k, 0) for k in za}, 'after': zg[base + '.after']}
print(json.dumps(r, indent=1))
PY
    log "$tag: tps=$(python3 -c "import json;print(json.load(open('$OUT/$tag.json'))['tps'])")"
    # Per-device bytes in the job log too (the artifact holds the JSON).
    log "$tag: $(python3 -c "import json;r=json.load(open('$OUT/$tag.json'));print(' '.join(f\"{k}=r{v['read_mb']:.0f}/w{v['write_mb']:.0f}MB\" for k,v in r['devices'].items()))")"
    # The cache's own counters, before and after, so a hit-rate question
    # can be answered from the job log without the artifact.
    grep -E '^(dmcache|bcache|cas)' "$OUT/$tag.before" | sed "s/^/[$tag before] /" >&2 || true
    grep -E '^(dmcache|bcache|cas)' "$OUT/$tag.after" | sed "s/^/[$tag after] /" >&2 || true
    if [ -n "$ZFS_UP" ]; then
        log "$tag: $(python3 -c "
import json
z = json.load(open('$OUT/$tag.json'))['zfs']; d = z['delta']; a = z['after']; MB = 1 << 20
zil = sum(d.get(f'zil.zil_itx_metaslab_{c}_alloc', d.get(f'zil.zil_itx_metaslab_{c}_bytes', 0)) for c in ('normal', 'slog'))
print(f\"zfs first-touch={d.get('cow.allocated_bytes', 0) / MB:.0f}MB live-at-end={a.get('window.written_bytes', 0) / MB:.0f}MB \"
      f\"zil={zil / MB:.0f}MB commits={d.get('zil.zil_commit_count', 0)} arc hits={d.get('arc.hits', 0)} misses={d.get('arc.misses', 0)} \"
      f\"l2 hits={d.get('arc.l2_hits', 0)} l2 written={d.get('arc.l2_write_bytes', 0) / MB:.0f}MB arc size={a.get('arc.size', 0) / MB:.0f}MB\")")"
        grep -E '^zfsinfo' "$OUT/$tag.after" | sed "s/^/[$tag after] /" >&2 || true
    fi
}

mount_and_start() {
    if [ -n "$ZFS_UP" ]; then
        zfs mount "$ZFS_POOL/data"
        zfs mount "$ZFS_POOL/wal"
    else
        mount -o noatime "$FS_DEV" "$MNT"
    fi
    if [ -n "$WAL_FROM_ORIGIN" ]; then
        # The WAL volume already carries pg_wal (its origin is a
        # snapshot of it); drop the data volume's copy and point at it.
        mount -o noatime /dev/mapper/tb-wal "$MNT_WAL"
        as_pg "rm -rf '$PGDATA/pg_wal' && ln -s '$MNT_WAL/pg_wal' '$PGDATA/pg_wal'"
    elif [ -n "$TWO_VOL" ]; then
        mount -o noatime /dev/mapper/tb-wal "$MNT_WAL"
        mkdir -p "$MNT_WAL/pg_wal"; chown "$PG_USER" "$MNT_WAL/pg_wal"; chmod 700 "$MNT_WAL/pg_wal"
        as_pg "mv '$PGDATA/pg_wal/'* '$MNT_WAL/pg_wal/' && rmdir '$PGDATA/pg_wal' && ln -s '$MNT_WAL/pg_wal' '$PGDATA/pg_wal'"
    fi
    rm -f "$PGDATA/postmaster.pid"
    start_pg
}

# ---------------------------------------------------------------- main
prepare_origin
# shellcheck disable=SC2086  # space-separated lists by contract
for TOPO in $TOPOLOGIES; do
    log "==== topology: $TOPO"
    # The ZFS topologies bring their own stack; everything else sits on
    # a thin volume over the ext4 origin.
    case $TOPO in zfs*) ;; *) build_pool ;; esac
    "topo_${TOPO//-/_}"
    mount_and_start
    # shellcheck disable=SC2086
    for wl in $WORKLOADS; do run_workload "$TOPO" "$wl"; done
    teardown_topology
done

summarize "$OUT" > "$OUT/summary.md"
cat "$OUT/summary.md"
echo "topology-bench OK — per-run JSON, snapshots and summary.md under $OUT"

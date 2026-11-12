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
#   zfs-slog  zfs plus a separate log device on local flash, so ZIL
#             blocks never reach the vdev the backend would serve: the
#             ZFS-native form of the two-volume split.
#   zfs-slog-l2arc  zfs-slog plus an L2ARC device of twice the ARC cap
#             (upstream's floor for a useful L2ARC), with the WAL dataset
#             excluded from it (secondarycache=none): the ZFS-native flash
#             read cache, sized so that it does not churn as zfs-l2arc's
#             25% device does.
#   zfs-slog-rcache  zfs-slog plus a dm-cache (writethrough, smq, the
#             same policy and TB_CACHE_PCT as baseline) on the ORIGIN
#             path, under the dm-snapshot: the stand-in for a backend
#             that keeps what it fetched on local flash. The origin is
#             immutable, so this cache only ever sees reads: nothing is
#             written twice and nothing can go stale.
#
#   zfs-ublk  the pool's data vdev is a ublk device (design §4.2): the
#             reference server's loop target (ublk-org/ublksrv, the
#             `ublk` command; tests/kernel/build-ublksrv.sh builds it)
#             serves the pool image itself, read-write, with one copy
#             per request, and the pool is imported from /dev/ublkbN.
#             Otherwise as zfs-slog: same datasets, same log device.
#             There is no dm-snapshot and no delayed origin under it:
#             everything is local, so compare it at TB_ORIGIN_DELAY_MS=0.
#   zfs-loop  zfs-ublk with a kernel loop device (direct I/O) in place
#             of the ublk device: the control. zfs-ublk against zfs-loop
#             is the ublk hop alone; zfs-loop against zfs-slog is what
#             the dm-snapshot model itself costs.
#   zfs-skyzfsd  zfs-ublk with the project's daemon (`skyzfsd serve`,
#             TB_SKYZFSD names the binary) in place of the reference
#             server: the row M1's exit criterion is about. The daemon
#             keeps a volume in its own format (a home image and a
#             write log), so the pool image is not handed to it as a
#             file: a volume of the image's size is created and served,
#             and the exported image is copied onto the served device
#             with direct I/O, 1 MiB at a time, leaving out every 1 MiB
#             that is all zeros (dd conv=sparse), so that what the other
#             rows' sparse image never wrote the daemon is never given
#             either. Everything of the image that is not zeros has
#             passed through the daemon's log and its checkpoint before
#             the pool is imported. The script then waits for the daemon
#             to be at rest (no checkpoint step under way, nothing
#             unstored or held, its write count still) and says where
#             the log stood. Same datasets and log device as zfs-slog.
#             The daemon's status is recorded before and after each
#             window and sampled every second during it: steps taken,
#             the range of `log_live`, and how many samples found a
#             step under way. `origin`, `pool` and `fs` report the
#             served device, which is the pool's view: what the daemon
#             writes to its own files (the log and the home image, in
#             TB_WORK) is NOT counted, so this row has no flash-write
#             figure that can be compared with the others.
#
# For these rows `origin` is the delayed pool image, `pool` is the
# dm-snapshot's COW device (the local flash every vdev write lands on),
# `fs` is the vdev itself, `cache` is the L2ARC device and `slog` the
# log device (counted into `flash write`). The ARC is
# not charged to postgres's cgroup, so with TB_MEM_LIMIT set it is
# capped at TB_MEM_LIMIT minus shared_buffers — what the page cache
# gets in the other topologies. In zfs-ublk and zfs-loop one image is
# origin, pool and vdev at once: `origin`, `pool` and `fs` all report
# the vdev, and there is no first-touch figure.
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
#                           zfs-l2arc zfs-slog zfs-slog-l2arc
#                           zfs-slog-rcache; and, never in the default
#                           set, zfs-ublk zfs-loop zfs-skyzfsd
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
#   TB_ZFS_GB=              pool size (default 32, or 2x TB_VOL_GB if that
#                           is more). ZFS charges dirty data against free
#                           space at spa_asize_inflation (24) times its
#                           size and makes every writer wait for a
#                           transaction group once that estimate exceeds
#                           the pool, so a pool of a few GiB stalls at a
#                           few hundred MiB of dirty data. The images are
#                           sparse: the size costs nothing until written
#   TB_ZFS_ARC_MB=          ARC cap (default TB_MEM_LIMIT - shared_buffers;
#                           uncapped when TB_MEM_LIMIT is unset)
#   TB_ZFS_WINDOW_SNAPSHOT=1  snapshot both datasets at the start of each
#                           measured window, so `written@` at the end is
#                           the live bytes born in the window: what one
#                           upload per window would have to carry. 0 skips
#                           it (the snapshot pins every overwritten block
#                           for the window).
#   TB_ZFS_SLOG_MB=1024     size of zfs-slog's log device
#   TB_ZFS_SLOG_PREALLOC=1  write the log device's image through once
#                           before use. The image is a sparse file, so
#                           otherwise the first write to each block also
#                           allocates it in the host filesystem, and the
#                           flush behind an fsync then commits that
#                           filesystem's journal — a cost a real flash
#                           partition does not have. 0 leaves it sparse
#   TB_UBLK_ARGS=           extra arguments for zfs-ublk's `ublk add`, comma-
#                           separated ("-q,4,-d,256": four queues of depth
#                           256; the server's default is one queue of 128)
#   TB_SKYZFSD=             the skyzfsd binary for zfs-skyzfsd (a release
#                           build without the test hooks is what the
#                           workflow builds)
#   TB_SKYZFSD_REF=         what TB_SKYZFSD was built from (a commit or
#                           ref; the workflow sets it, and builds that ref
#                           when a sequence entry names it): written down
#                           and printed beside the row, with the name and
#                           hash of the binary that read the row's status
#                           (always the row's own)
#   TB_SKYZFSD_ARGS=        extra arguments for its `serve`, comma-separated
#                           ("--depth,128,--log-high,8,--log-low,4"; the
#                           marks are in segments of 64 MiB, default 2 and 1)
#   TB_SKYZFSD_MEM=         cgroup v2 memory.max for the daemon (e.g. 1G).
#                           The daemon's files (home image and log) go
#                           through the host's page cache, which is
#                           outside postgres's TB_MEM_LIMIT: unset, the
#                           daemon rows have a cache under the pool as
#                           large as the machine's free memory, which the
#                           other rows (direct I/O under the pool) have
#                           not. Set, the daemon runs in a cgroup of its
#                           own from its first instruction, so the pages
#                           of its files are charged there and bounded.
#                           This bounds the cache; it is NOT what direct
#                           I/O would cost: at the limit the kernel
#                           reclaims and throttles the daemon's
#                           write-back. The daemon's resident memory and
#                           its cgroup's file pages are recorded either way
#   TB_ZFS_L2ARC_MB=        size of zfs-slog-l2arc's cache device (default
#                           twice the ARC cap, or twice the database when
#                           the ARC is uncapped)
#   TB_ZFS_PARAMS=          zfs module parameters to set for the ZFS
#                           topologies, "name=value ..." (for instance
#                           "l2arc_noprefetch=0 l2arc_headroom=0"); restored
#                           afterwards
#   TB_SNAP_MODES=none      snapshots taken DURING the measured window, one
#                           phase per listed mode on the same stack, in
#                           order (so the phases share a machine and are
#                           comparable). `none` takes none. For the thin
#                           stacks `freeze` is fsfreeze → suspend →
#                           create_snap → resume → thaw (what the agent's
#                           capture does) and `suspend` is the same without
#                           the freeze: a crash-consistent snapshot. `sync`
#                           is `suspend` after a syncfs of the filesystem,
#                           which writes the dirty backlog out first without
#                           blocking writers (the flush a freeze does,
#                           without the freeze). The
#                           ZFS stacks have one mechanism, `zfs snapshot`,
#                           so any mode other than none runs it once.
#                           twovol snapshots both volumes in one bracket.
#   TB_SNAP_INTERVAL=60     seconds between snapshots
#   TB_SNAP_KEEP=2          snapshots retained; older ones are deleted
#   TB_ZFS_PROPS=           dataset properties for the ZFS topologies,
#                           comma-separated, set on the pool root (and so
#                           inherited) or on one dataset with a data: or wal:
#                           prefix: "sync=disabled", "wal:logbias=throughput"
#   TB_PG_CONF=             postgresql.conf settings for this run, comma-
#                           separated name=value ("synchronous_commit=off")
#   TB_PREALLOC=0           1 writes every local-flash image through once
#                           before use: the thin pool, the cache device,
#                           the ZFS vdev's COW store and the log device.
#                           They are sparse files otherwise, so a first
#                           write to a block also allocates it in the host
#                           filesystem and the next flush commits that
#                           filesystem's journal, which a flash partition
#                           would not do. The stacks pay this unevenly:
#                           dm-thin overwrites in place once a chunk is
#                           provisioned, ZFS keeps moving to fresh blocks.
#                           Costs the images' full size in writes and disk
#   TB_FSYNC_TEST=1         run pg_test_fsync on the WAL filesystem before the
#                           workloads: the commit path with nothing else
#                           running (about 35 s per topology; 0 skips it)
#   TB_LABEL=               tag appended to the topology name in the results,
#                           for runs of one topology under different settings
#   TB_PG_USER=postgres     unprivileged user postgres runs as
#   TB_KEEP=1               leave the stack up on failure for inspection
#   TB_REUSE_ORIGIN=1       keep an existing origin image (same scale)
#
# Needs root, device-mapper (dm-thin-pool, dm-cache, dm-delay when a
# delay is set), losetup, mkfs.ext4, python3, a PostgreSQL server with
# pgbench; bcache-tools for the bcache topology; casadm for opencas;
# zfsutils (zpool, zfs, the zfs module) and dm-snapshot for the zfs ones.
set -euo pipefail

# lim SECONDS COMMAND: a time limit that also gives up on a command
# that cannot be killed. A pool whose device has lost its server makes
# such commands, and zfs-skyzfsd's teardown must not hang on one.
# shellcheck source=tests/lib/lim.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/lim.sh"
# It holds under this script's errexit and pipefail (tests/lib/test_lim.sh).

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

# Where skz_time.py is: the daemon's times, their differences and their
# tables (tested by tests/bench/test_skz_time.py).
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
# TB_SKZ_TIME_DIR: a directory with another skz_time.py to read the
# daemon's times with (the workflow's TB_SKZ_TIME_REF puts a ref's copy
# there): a daemon newer than this branch has keys its copy does not
# know. Default: the one beside this script.
SKZ_TIME_DIR=${TB_SKZ_TIME_DIR:-$HERE}
summarize() { # dir -> summary.md on stdout (all *.json under dir, any depth)
    python3 - "$1" "$SKZ_TIME_DIR" <<'PY'
import glob, json, os, sys
out = sys.argv[1]
sys.path.insert(0, sys.argv[2])
import skz_time
runs = []
for p in sorted(glob.glob(os.path.join(out, '**', '*.json'), recursive=True)):
    r = json.load(open(p))
    # A sequence writes its N-th entry under TB_OUT/N: the order the
    # rows ran in on their machine. None outside a sequence.
    d = os.path.basename(os.path.dirname(p))
    r['_seq'] = int(d) if d.isdigit() else None
    runs.append(r)
order = ['nocache', 'baseline', 'twovol', 'twovol-origin', 'twovol-hydrated', 'bcache', 'opencas',
         'zfs', 'zfs-plain', 'zfs-l2arc', 'zfs-slog', 'zfs-slog-l2arc', 'zfs-slog-rcache',
         'zfs-loop', 'zfs-ublk', 'zfs-skyzfsd']
modes = ['none', 'sync', 'suspend', 'freeze', 'zfs']
def mode(r): return r.get('snap', {}).get('mode', 'none')
def label(r): return r.get('label', r['topology']) + ('' if mode(r) == 'none' else ' +' + mode(r))
runs.sort(key=lambda r: (r['workload'], order.index(r['topology']) if r['topology'] in order else 99,
                         r.get('label', ''), modes.index(mode(r)) if mode(r) in modes else 99))
print('## topology-bench\n')
# The work disk's sync, measured at the start of a job (disk-sync.py):
# which kind of machine the rows of that job ran on.
disk = []
for p in sorted(glob.glob(os.path.join(out, '**', 'disk-sync.txt'), recursive=True)):
    for line in open(p):
        f = line.split()
        if f[:1] == ['disk-sync']:
            disk.append(dict(x.split('=', 1) for x in f[1:] if '=' in x))
if disk:
    print('The work disk, measured at the start of the job (tests/bench/disk-sync.py): 64 MiB written through the page cache '
          'and the fdatasync after it, then 64 MiB with O_DIRECT and its fdatasync. The daemon\'s files are on this disk, '
          'and its syncs wait for what the page cache holds of them; an observation, not judged:\n')
    print('| machine | buffered: write ms | its fdatasync ms | MiB/s of the fdatasync | direct: write ms | its fdatasync ms | MiB/s of both | the rows of this machine |')
    print('|---|---|---|---|---|---|---|---|')
    for d in disk:
        m = d.get('machine', '')
        m = m[:36] + ' ' + m[37:] if len(m) > 37 else m
        here = sorted({r.get('label', r['topology']) for r in runs if r.get('machine') == m})
        print(f"| {m} | {d.get('buffered_write_ms', '-')} | {d.get('buffered_sync_ms', '-')} | {d.get('buffered_sync_mib_s', '-')} | "
              f"{d.get('direct_write_ms', '-')} | {d.get('direct_sync_ms', '-')} | {d.get('direct_all_mib_s', '-')} | {', '.join(here) or 'none'} |")
    print()
print('Block-layer bytes over the measured window. `origin read` includes the WAL origin where there is one; '
      '`flash write` = pool + cache (+ ZFS log) device writes; '
      '`amp` = flash write relative to the nocache run of the same workload.\n')
print('| workload | topology | tps | lat ms | origin read MB | pool read MB | pool write MB | cache write MB | cache read MB | flash write MB | amp | WAL MB | flushes (pool) |')
print('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
base = {}
rows = []
for r in runs:
    d = r['devices']; c = d.get('cache', {})
    flash = d['pool']['write_mb'] + c.get('write_mb', 0) + d.get('slog', {}).get('write_mb', 0)
    if label(r) == 'nocache': base[r['workload']] = flash
    amp = flash / base[r['workload']] if base.get(r['workload']) else float('nan')
    print(f"| {r['workload']} | {label(r)} | {r['tps'] or 0:.0f} | {r['latency_ms'] or 0:.2f} | "
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
    if label(r) == 'nocache': base_txn[r['workload']] = per
    ampt = per / base_txn[r['workload']] if base_txn.get(r['workload']) else float('nan')
    print(f"| {r['workload']} | {label(r)} | {txn} | {d['pool']['write_mb'] / k if txn else float('nan'):.1f} | "
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
    print('| workload | topology | vdev write MB/ktxn | log dev write MB/ktxn | first-touch LBA MB/ktxn | live at end MB/ktxn | ZIL MB/ktxn | vdev read MB | ARC hit % | L2ARC hit % of ARC misses |')
    print('|---|---|---|---|---|---|---|---|---|---|')
    for r, d in zrows:
        z = r['zfs']; dl = z.get('delta', {}); af = z.get('after', {})
        txn = r['pg'].get('xact_commit', 0) or 0
        k = txn / 1000 if txn else float('nan')
        zil = sum(dl.get(f'zil.zil_itx_metaslab_{cls}_alloc', dl.get(f'zil.zil_itx_metaslab_{cls}_bytes', 0))
                  for cls in ('normal', 'slog')) / MB
        live = af.get('window.written_bytes')
        hits = dl.get('arc.hits', 0); miss = dl.get('arc.misses', 0) + dl.get('arc.iohits', 0)
        l2h = dl.get('arc.l2_hits', 0); l2m = dl.get('arc.l2_misses', 0)
        print(f"| {r['workload']} | {label(r)} | {d['fs']['write_mb'] / k:.1f} | "
              f"{d.get('slog', {}).get('write_mb', 0) / k:.1f} | "
              f"{dl.get('cow.allocated_bytes', 0) / MB / k:.1f} | "
              f"{'n/a' if live is None else format(live / MB / k, '.1f')} | {zil / k:.1f} | {d['fs']['read_mb']:.0f} | "
              f"{100 * hits / (hits + miss) if hits + miss else float('nan'):.1f} | "
              f"{100 * l2h / (l2h + l2m) if l2h + l2m else float('nan'):.1f} |")
# Where the time goes: postgres's own I/O timers, the commit path
# measured alone, and the CPU. Runs that predate these counters are
# skipped.
drows = [(r, d) for r, d, c, flash in rows if r.get('diag')]
if drows:
    def pgsum(r, obj, k): return sum(v for n, v in r['pg'].items() if n.startswith(f'io_{obj}_') and n.endswith('_' + k))
    def nf(v, f='.2f'): return 'n/a' if v is None else format(v, f)
    print('\nWhere the time goes. `WAL sync ms` is postgres\'s own timer per WAL fsync; '
          '`8k sync alone` is pg_test_fsync fdatasync on the WAL filesystem before the run; '
          'CPU is the share of all CPUs over the window:\n')
    print('| workload | topology | tps | p50 ms | WAL syncs/s | txn per WAL sync | WAL sync ms | 8k sync alone ms | rel reads/txn | rel read ms | CPU user/sys/iowait/idle % | ctx switches/txn | CPU s by thread |')
    print('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
    for r, d in drows:
        g = r['diag']; txn = r['pg'].get('xact_commit', 0) or 0; dur = r['duration_s']
        wf = pgsum(r, 'wal', 'fsyncs') or r['pg'].get('walstat_sync', 0)
        wus = pgsum(r, 'wal', 'fsync_us') or r['pg'].get('walstat_sync_us', 0)
        rr = pgsum(r, 'relation', 'reads'); rus = pgsum(r, 'relation', 'read_us')
        cp = g.get('cpu_pct', {})
        top = ', '.join(f'{k} {v:.0f}' for k, v in list(g.get('cpu_s_by_thread', {}).items())[:6])
        ft = r.get('fsync_test_us', {}).get('fdatasync')
        print(f"| {r['workload']} | {label(r)} | {r['tps'] or 0:.0f} | {nf(r.get('latency', {}).get('p50_ms'))} | "
              f"{nf(wf / dur if wf else None, '.0f')} | {nf(txn / wf if wf else None)} | {nf(wus / wf / 1000 if wf else None, '.3f')} | "
              f"{nf(ft / 1000 if ft else None, '.3f')} | {nf(rr / txn if txn else None)} | {nf(rus / rr / 1000 if rr else None, '.3f')} | "
              f"{cp.get('user', 0):.0f}/{cp.get('system', 0):.0f}/{cp.get('iowait', 0):.0f}/{cp.get('idle', 0):.0f} | "
              f"{nf(g.get('ctxt', 0) / txn if txn else None, '.1f')} | {top} |")
    md = [(r, d) for r, d in drows if r['diag'].get('mem_mb')]
    if md:
        print('\nMemory at the end of the window, MiB. The cgroup is postgres under TB_MEM_LIMIT; its `file` is the '
              'page cache it was charged for, and the ARC is outside it. `host page cache` is the whole machine\'s: '
              'a buffered loop device would put the images there, outside any cap. `buffered loops` should be 0. '
              'The daemon columns are for zfs-skyzfsd: its own resident memory, the limit of its cgroup (TB_SKYZFSD_MEM), '
              'the page cache its files were charged for there, and how many times the cgroup reached its limit, '
              'during the window and since the daemon started, which includes the copy of the pool image and the '
              'warm-up (counts, not MiB):\n')
        print('| workload | topology | cgroup total | cgroup file | cgroup anon + shmem | ARC | ARC limit | host page cache | host dirty | buffered loops | daemon resident | daemon cgroup limit | daemon cgroup file | times at the limit in the window | since the daemon started |')
        print('|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|')
        for r, d in md:
            m = r['diag']['mem_mb']; lp = r['diag'].get('loops', {})
            arc = r.get('zfs', {}).get('after', {}).get('arc.size')
            print(f"| {r['workload']} | {label(r)} | {nf(m.get('cgroup.current'), '.0f')} | {nf(m.get('cgroup.file'), '.0f')} | "
                  f"{nf(m.get('cgroup.anon', 0) + m.get('cgroup.shmem', 0) if 'cgroup.anon' in m else None, '.0f')} | "
                  f"{nf(arc / (1 << 20) if arc else None, '.0f')} | {nf(m.get('zfs.arc_max'), '.0f')} | {nf(m.get('host.Cached'), '.0f')} | {nf(m.get('host.Dirty'), '.0f')} | "
                  f"{sum(1 for v in lp.values() if not v['dio'])} of {len(lp)} | "
                  f"{nf(m.get('skz.VmRSS'), '.0f')} | {nf(m.get('skz.cgroup.max'), '.0f') if 'skz.cgroup.max' in m else ('none' if 'skz.VmRSS' in m else 'n/a')} | "
                  f"{nf(m.get('skz.cgroup.file'), '.0f')} | {nf(m.get('skz.cgroup.events.max.window'), '.0f')} | "
                  f"{nf(m.get('skz.cgroup.events.max.since_start'), '.0f')} |")
    zd = [(r, d) for r, d in drows if r.get('zfs')]
    if zd:
        MB = 1 << 20
        print('\nZFS rows: the intent log and the transaction groups. ZIL payload by dataset separates WAL commits '
              'from data-file fsyncs; `tx delayed` counts transactions the write throttle or the ARC made wait; '
              '`space-check waits` counts transactions sent to wait for a transaction group to sync because the '
              'pool\'s free space is less than the dirty data in flight times `spa_asize_inflation` (24) — a pool '
              'too small for its dirty data, and then `lat max ms` tracks the longest sync:\n')
        print('| workload | topology | ZIL commits/s | txn per ZIL commit | ZIL payload MB/ktxn, wal / data | txgs | txg sync ms mean / max | dirty MB per txg | tx delayed | ARC throttles | space-check waits | lat max ms |')
        print('|---|---|---|---|---|---|---|---|---|---|---|---|')
        for r, d in zd:
            z = r['zfs']; dl = z['delta']; tx = z.get('txg', {})
            txn = r['pg'].get('xact_commit', 0) or 0; k = txn / 1000 if txn else float('nan'); dur = r['duration_s']
            zc = dl.get('zil.zil_commit_count', 0)
            def pay(ds): return sum(dl.get(f'ds.{ds}.zil_itx_{t}_bytes', 0) for t in ('copied', 'needcopy', 'indirect')) / MB
            delayed = sum(dl.get('dmutx.' + n, 0) for n in ('dmu_tx_delay', 'dmu_tx_dirty_delay', 'dmu_tx_dirty_throttle',
                                                             'dmu_tx_dirty_over_max', 'dmu_tx_memory_reserve',
                                                             'dmu_tx_memory_reclaim', 'dmu_tx_wrlog_delay'))
            print(f"| {r['workload']} | {label(r)} | {zc / dur:.0f} | {nf(txn / zc if zc else None)} | "
                  f"{pay('wal') / k:.1f} / {pay('data') / k:.1f} | {tx.get('n', 0)} | "
                  f"{nf(tx.get('sync_ms_mean'), '.0f')} / {nf(tx.get('sync_ms_max'), '.0f')} | {nf(tx.get('dirty_mb_mean'), '.0f')} | "
                  f"{delayed} | {dl.get('arc.memory_throttle_count', 0)} | {dl.get('dmutx.dmu_tx_quota', 0)} | "
                  f"{nf(r.get('latency', {}).get('max_ms'), '.0f')} |")
# Snapshots during the window. Phases of one topology ran on the same
# stack and machine, in the order listed, so `vs none` compares like
# with like; rows of different topologies are different machines.
# `snapshot ms` is the bracket the mechanism holds: freeze..thaw,
# suspend..resume, or the `zfs snapshot` call (which blocks only its
# caller). What the workload felt is the latency columns.
if any(mode(r) != 'none' for r in runs):
    print('\nSnapshots during the window (`vs none` = tps against the same stack with no snapshots):\n')
    print('| workload | topology | mode | every s | taken | snapshot ms mean | snapshot ms max | tps | vs none | lat p99 ms | lat max ms | slowest 1 s tps | flash MB/ktxn |')
    print('|---|---|---|---|---|---|---|---|---|---|---|---|---|')
    ref = {(r['workload'], r['topology']): r['tps'] for r in runs if mode(r) == 'none'}
    for r, d, c, flash in rows:
        sn = r.get('snap', {}); lat = r.get('latency', {})
        txn = r['pg'].get('xact_commit', 0) or 0
        base_tps = ref.get((r['workload'], r['topology']))
        ms = sn.get('ms', [])
        fmt = lambda v, f='.1f': 'n/a' if v is None else format(v, f)
        print(f"| {r['workload']} | {r['topology']} | {mode(r)} | {sn.get('interval_s', '-')} | {len(ms)} | "
              f"{fmt(sum(ms) / len(ms) if ms else None, '.0f')} | {fmt(max(ms) if ms else None, '.0f')} | "
              f"{r['tps'] or 0:.0f} | {fmt(r['tps'] / base_tps if base_tps and r['tps'] else None, '.2f')} | "
              f"{fmt(lat.get('p99_ms'))} | {fmt(lat.get('max_ms'), '.0f')} | {fmt(lat.get('min_tps_1s'), '.0f')} | "
              f"{flash / (txn / 1000) if txn else float('nan'):.1f} |")
# The daemon against the benchmark model (design §9, M1: tpcb within
# 10% of the benchmark model on the same machine). The model is the
# zfs-slog row. A ratio, and with it a verdict, is only formed between
# two rows of one machine: each row carries the boot id and the host
# name of the machine it ran on, and rows are compared only where it is the same. The
# workflow's summary job puts the rows of every matrix runner in one
# directory; those end with NO VERDICT, and so does a row that does not
# say where it ran.
skz = [r for r in runs if r['topology'] == 'zfs-skyzfsd' and mode(r) == 'none']
if skz:
    def others(topo, r):
        """-> (rows of `topo` and r's workload from r's machine, how many from elsewhere)."""
        rows = [o for o in runs if o['topology'] == topo and o['workload'] == r['workload'] and mode(o) == 'none' and o['tps']]
        here = [o for o in rows if r.get('machine') and o.get('machine') == r['machine']]
        return here, len(rows) - len(here)
    print('\nThe daemon against the benchmark model (`zfs-slog`). A ratio is formed only between two rows of one machine '
          '(the same boot id and host name); rows of other machines are counted and not compared. '
          'The criterion is a ratio of at least 0.90 to a zfs-slog row with no origin delay (TB_ORIGIN_DELAY_MS=0); '
          'the ratios to zfs-loop and zfs-ublk are reported and not judged.\n')
    print('| workload | daemon row | its build; serve arguments; memory limit | tps | against | its tps | ratio | verdict |')
    print('|---|---|---|---|---|---|---|---|')
    for r in skz:
        k_ = r.get('skyzfsd', {})
        lim_ = (f"{k_.get('build', 'not said')}; {k_.get('serve_args') or 'defaults'}; "
                f"{k_.get('info', {}).get('mem_max', 'n/a')}")
        if not r['tps']:
            print(f"| {r['workload']} | {label(r)} | {lim_} | none | - | - | - | no tps: the run did not measure |")
            continue
        model, elsewhere = others('zfs-slog', r)
        if not r.get('machine'):
            print(f"| {r['workload']} | {label(r)} | {lim_} | {r['tps']:.0f} | zfs-slog | - | - | NO VERDICT: this row does not say what machine it ran on |")
        elif not model:
            print(f"| {r['workload']} | {label(r)} | {lim_} | {r['tps']:.0f} | zfs-slog | none on this machine | - | "
                  f"NO VERDICT: no zfs-slog row of this workload from the same machine ({elsewhere} from other machines, not compared) |")
        # In a sequence the first model row of the machine is the one
        # judged against; a later one (run to see whether the machine
        # changed over the job) is reported beside it.
        first = min((o['_seq'] for o in model if o.get('_seq') is not None), default=None)
        for o in model:
            q = r['tps'] / o['tps']
            if first is not None and o.get('_seq') is not None and o['_seq'] != first:
                print(f"| {r['workload']} | {label(r)} | {lim_} | {r['tps']:.0f} | {label(o)} (row {o['_seq']} of the sequence) | {o['tps']:.0f} | {q:.3f} | "
                      f"not judged: the first model row of the sequence (row {first}) is the one judged against |")
                continue
            # The daemon's row has no origin under it. A model row with
            # an origin delay was slowed by it at every origin read, and
            # a ratio to it would flatter the daemon: no verdict.
            delay = o.get('origin_delay_ms')
            if delay == 0:
                verdict = 'at least 0.90' if q >= 0.90 else 'BELOW 0.90'
            elif delay is None:
                verdict = 'NO VERDICT: the model row does not say what origin delay it had'
            else:
                verdict = f'NO VERDICT: the model row had an origin delay of {delay} ms, the daemon row has no origin'
            print(f"| {r['workload']} | {label(r)} | {lim_} | {r['tps']:.0f} | {label(o)} | {o['tps']:.0f} | {q:.3f} | {verdict} |")
        for topo in ('zfs-loop', 'zfs-ublk'):
            for o in others(topo, r)[0]:
                print(f"| {r['workload']} | {label(r)} | {lim_} | {r['tps']:.0f} | {label(o)} | {o['tps']:.0f} | {r['tps'] / o['tps']:.3f} | not judged |")
    print('\nWhat the daemon\'s status showed (sampled every second during the window; `log_live` in MiB):\n')
    print('| workload | daemon row | build; serve arguments | samples | with a step under way | steps in the window | syncs in the window | writes in the window | log_live min | max | at the start | at the end | most unstored in a sample | one server throughout | the pool at the end | before the import |')
    print('|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|')
    for r in skz:
        k = r.get('skyzfsd', {})
        w = k.get('window', {})
        mib = lambda v: 'n/a' if v is None else f'{v / (1 << 20):.0f}'
        nn = lambda v: 'n/a' if v is None else str(v)
        print(f"| {r['workload']} | {label(r)} | {k.get('build', 'not said')}; {k.get('serve_args') or 'defaults'} | {nn(w.get('samples'))} | {nn(w.get('step_under_way'))} | "
              f"{nn(k.get('steps'))} | {nn(k.get('syncs'))} | {nn(k.get('writes'))} | {mib(w.get('log_live_min'))} | {mib(w.get('log_live_max'))} | "
              f"{mib(k.get('log_live_before'))} | {mib(k.get('log_live_after'))} | {nn(w.get('unstored_max'))} | "
              f"{'n/a' if 'pid_same' not in k else 'yes' if k['pid_same'] else 'NO: the pid changed'} | "
              f"{k.get('pool_status', 'not asked')} | {k.get('info', {}).get('settle', 'n/a')} |")
    # Where a request's time goes, by the daemon's own account.
    def rel_read_us(r):
        io = lambda k: sum(v for n, v in r.get('pg', {}).items() if n.startswith('io_relation_') and n.endswith('_' + k))
        return (io('read_us') / io('reads') if io('reads') else None), io('reads')
    print('\n'.join(skz_time.render([
        {'label': label(r), 'workload': r['workload'], 'tps': r.get('tps'), 'time': r.get('skyzfsd', {}).get('time'),
         'steps': r.get('skyzfsd', {}).get('steps'), 'syncs': r.get('skyzfsd', {}).get('syncs'),
         'relation_read_us': rel_read_us(r)[0],
         'relation_reads': rel_read_us(r)[1], 'duration_s': r.get('duration_s'),
         'ages': (r.get('skyzfsd', {}).get('before', {}).get('age_ms'), r.get('skyzfsd', {}).get('after', {}).get('age_ms')),
         'model_relation_read_us': next((rel_read_us(o)[0] for o in others('zfs-slog', r)[0]), None)}
        for r in skz])))
PY
}
if [ "${1:-}" = summarize ]; then
    summarize "${2:-$OUT}"
    exit 0
fi

WORK=${TB_WORK:-/var/tmp/topology-bench}
TOPOLOGIES=${TB_TOPOLOGIES:-"nocache baseline twovol twovol-origin twovol-hydrated bcache opencas zfs zfs-plain zfs-l2arc zfs-slog zfs-slog-l2arc zfs-slog-rcache"}
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
# Snapshots during the window (see the knob list).
SNAP_MODES=${TB_SNAP_MODES:-none}
SNAP_MODES=${SNAP_MODES//,/ }   # commas too, so a sequence entry can carry a list
SNAP_INTERVAL=${TB_SNAP_INTERVAL:-60}
SNAP_KEEP=${TB_SNAP_KEEP:-2}
ANY_SNAP=""
# shellcheck disable=SC2086  # a space-separated list by contract
for m in $SNAP_MODES; do
    case $m in
        none) ;;
        freeze) need fsfreeze; ANY_SNAP=1 ;;
        suspend|sync) ANY_SNAP=1 ;;
        *) echo "unknown TB_SNAP_MODES entry: $m (none|sync|suspend|freeze)" >&2; exit 64 ;;
    esac
done
if [ "$SNAP_INTERVAL" -lt 1 ] || [ "$SNAP_KEEP" -lt 1 ]; then
    echo "TB_SNAP_INTERVAL and TB_SNAP_KEEP must be at least 1" >&2; exit 64
fi
# A retained thin snapshot pins the old copy of every chunk rewritten
# after it, so the pool can hold several versions of the hot set.
if [ -n "$ANY_SNAP" ]; then
    POOL_GB=${TB_POOL_GB:-$(( (VOL_GB + WAL_GB) * 2 + 1 ))}
else
    POOL_GB=${TB_POOL_GB:-$(( VOL_GB + WAL_GB + 1 ))}
fi

# ZFS topologies. The pool is twice the ext4 volume: copy-on-write needs
# free space to turn over in, and the per-window snapshot pins every
# block the window overwrites.
ZFS_POOL=tbzpool
ZFS_RECORDSIZE=${TB_ZFS_RECORDSIZE:-8k}
ZFS_WAL_RECORDSIZE=${TB_ZFS_WAL_RECORDSIZE:-128k}
ZFS_COMPRESSION=${TB_ZFS_COMPRESSION:-lz4}
ZFS_FPW=${TB_ZFS_FPW:-off}
ZFS_ASHIFT=${TB_ZFS_ASHIFT:-12}
ZFS_GB=${TB_ZFS_GB:-$(( VOL_GB * 2 > 32 ? VOL_GB * 2 : 32 ))}
ZFS_WINDOW_SNAPSHOT=${TB_ZFS_WINDOW_SNAPSHOT:-1}
ZFS_SLOG_MB=${TB_ZFS_SLOG_MB:-1024}
ZFS_SLOG_PREALLOC=${TB_ZFS_SLOG_PREALLOC:-1}
PREALLOC=${TB_PREALLOC:-0}
ZFS_L2ARC_MB=${TB_ZFS_L2ARC_MB:-}
UBLK_ARGS=${TB_UBLK_ARGS:-}
UBLK_ID=${TB_UBLK_ID:-7}
SKYZFSD=${TB_SKYZFSD:-}
SKYZFSD_ARGS=${TB_SKYZFSD_ARGS:-}
SKYZFSD_MEM=${TB_SKYZFSD_MEM:-}
ZFS_PARAMS=${TB_ZFS_PARAMS:-}
ZFS_PROPS=${TB_ZFS_PROPS:-}
PG_CONF=${TB_PG_CONF:-}
FSYNC_TEST=${TB_FSYNC_TEST:-1}
LABEL=${TB_LABEL:-}
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

ANY_THIN=""; ANY_ZFS=""; ANY_UBLK=""
# shellcheck disable=SC2086  # TB_TOPOLOGIES is a space-separated list by contract
for t in $TOPOLOGIES; do
    case $t in
        nocache|baseline|twovol|twovol-origin|twovol-hydrated) ANY_THIN=1 ;;
        bcache) need make-bcache; ANY_THIN=1 ;;
        opencas) need casadm; ANY_THIN=1 ;;
        zfs|zfs-plain|zfs-l2arc|zfs-slog|zfs-slog-l2arc|zfs-slog-rcache|zfs-loop) need zpool zfs tar; ANY_ZFS=1 ;;
        zfs-ublk) need zpool zfs tar ublk; ANY_ZFS=1; ANY_UBLK=1 ;;
        zfs-skyzfsd)
            need zpool zfs tar; ANY_ZFS=1; ANY_UBLK=1
            { [ -n "$SKYZFSD" ] && [ -x "$SKYZFSD" ]; } || { echo "zfs-skyzfsd needs TB_SKYZFSD, the skyzfsd binary: '${SKYZFSD}' is not a program" >&2; exit 64; }
            ;;
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
# ZFS: the COW store can fill to the pool's size; the origin image is as
# large but holds only the database that was copied into it.
if [ -n "$ANY_ZFS" ] && [ $(( (ZFS_GB + VOL_GB) * 1024 + 64 )) -gt "$STACK_MB" ]; then
    STACK_MB=$(( (ZFS_GB + VOL_GB) * 1024 + 64 ))
fi
# zfs-slog-l2arc's cache device: its own knob, else twice the ARC cap,
# else twice the database.
L2ARC_NEED_MB=0
case " $TOPOLOGIES " in *" zfs-slog-l2arc "*)
    L2ARC_NEED_MB=${ZFS_L2ARC_MB:-$(( ${ZFS_ARC_MB:-0} > 0 ? ZFS_ARC_MB * 2 : SCALE * 32 ))} ;;
esac
NEED_MB=$(( (VOL_GB + WAL_GB) * 1024 + STACK_MB + ZFS_SLOG_MB + L2ARC_NEED_MB + SCALE * 16 * CACHE_PCT / 100 + 1024 ))
AVAIL_MB=$(df -Pm "$WORK" | awk 'NR==2 {print $4}')
if [ "$AVAIL_MB" -lt "$NEED_MB" ]; then
    echo "not enough space under $WORK: ${AVAIL_MB} MiB free, worst case needs ${NEED_MB} MiB" \
         "(lower TB_SCALE / TB_MAX_WAL_GB / TB_ZFS_GB, or point TB_WORK at a bigger disk)" >&2
    exit 64
fi
MNT="$WORK/mnt"; MNT_WAL="$WORK/mnt-wal"; SOCK="$WORK/sock"
mkdir -p "$MNT" "$MNT_WAL" "$SOCK"
chown "$PG_USER" "$SOCK" "$WORK"
PGDATA="$MNT/data"
CGROUP=""
# postgres's own I/O timers, as "name value" rows. One file for every
# server version: a statement that names a column this version lacks
# fails by itself and the rest still run. pg_stat_io carries WAL rows
# from 18; before that pg_stat_wal has the sync counters.
cat > "$WORK/io-stats.sql" <<'SQL'
select 'io_' || object || '_' || grp || '_' || k, v::bigint
from (select replace(object, ' ', '') as object,
             case when backend_type = 'client backend' then 'backend'
                  when backend_type in ('checkpointer', 'background writer') then 'bg'
                  else 'other' end as grp,
             sum(reads) as reads, sum(read_time) * 1000 as read_us,
             sum(writes) as writes, sum(write_time) * 1000 as write_us,
             sum(extends) as extends, sum(extend_time) * 1000 as extend_us,
             sum(fsyncs) as fsyncs, sum(fsync_time) * 1000 as fsync_us
      from pg_stat_io where object in ('relation', 'wal') group by 1, 2) s
     cross join lateral (values ('reads', reads), ('read_us', read_us), ('writes', writes),
                                ('write_us', write_us), ('extends', extends), ('extend_us', extend_us),
                                ('fsyncs', fsyncs), ('fsync_us', fsync_us)) t(k, v)
where v is not null;
select 'walstat_sync', wal_sync from pg_stat_wal;
select 'walstat_sync_us', (wal_sync_time * 1000)::bigint from pg_stat_wal;
select 'walstat_write', wal_write from pg_stat_wal;
select 'walstat_write_us', (wal_write_time * 1000)::bigint from pg_stat_wal;
select 'ckpt_' || k, v::bigint
from pg_stat_checkpointer,
     lateral (values ('timed', num_timed::float8), ('requested', num_requested::float8),
                     ('write_ms', write_time), ('sync_ms', sync_time),
                     ('buffers', buffers_written::float8)) t(k, v);
SQL
chmod 644 "$WORK/io-stats.sql"
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
if [ -n "$ANY_UBLK" ]; then
    modprobe ublk_drv 2>/dev/null || true
    [ -e /dev/ublk-control ] || { echo "no /dev/ublk-control: the ublk_drv kernel module is not loaded (modprobe ublk_drv)" >&2; exit 64; }
fi

log() { printf '[%s] %s\n' "$(date +%H:%M:%S)" "$*" >&2; }
as_pg() { su -s /bin/bash "$PG_USER" -c "$*"; }
psqlq() { as_pg "'$PG_BIN/psql' -h '$SOCK' -d bench -Atq -c \"$1\"" 2>/dev/null; }
sectors() { blockdev --getsz "$1"; }
dm_blk() { dmsetup info -c --noheadings -o blkdevname "$1"; }
mb() { echo $(( $1 << 20 )); }
loop_dio() { losetup --noheadings --output DIO "$1" 2>/dev/null | tr -d ' '; }
attach_loop() { # file: asks for direct I/O, so the image is not also in the host page cache
    local l
    l=$(losetup --find --show --direct-io=on "$1" 2>/dev/null || losetup --find --show "$1")
    [ "$(loop_dio "$l")" = 1 ] || log "WARNING: $l on $(basename "$1") is BUFFERED (no direct I/O)"
    echo "$l"
}
mkimage() { # file bytes — sparse on purpose (see the sizing note)
    rm -f "$1"
    truncate -s "$2" "$1"
}
mkflash() { # file bytes: an image that stands for local flash. TB_PREALLOC writes it through.
    mkimage "$1" "$2"
    [ "$PREALLOC" = 1 ] || return 0
    local t0; t0=$(date +%s)
    "$DD" if=/dev/zero of="$1" bs=1M count=$(( $2 >> 20 )) oflag=direct conv=notrunc,fsync status=none
    log "$(basename "$1"): wrote $(( $2 >> 20 )) MiB through in $(( $(date +%s) - t0 ))s"
}
zero_head() { # device: zero the first 4 KiB, BYPASSING the bdev page cache.
    # A buffered dd leaves a dirty page in the block device's page cache
    # that writeback lands later — on top of the superblock dm-thin or
    # dm-cache has meanwhile written with direct bios (seen as
    # "cache metadata: sb_check failed: magic 0" mid-run).
    "$DD" if=/dev/zero of="$1" bs=4096 count=1 conv=notrunc,nocreat,fsync oflag=direct status=none
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
ZFS_UP=""; ZORIGIN_LOOP=""; ZSRC_LOOP=""; ZFS_WIN=""; ZFS_ARC_SAVED=""; SLOG_LOOP=""
SNAP_PID=""; SNAP_STOP=""; ZFS_RCACHE=""; ZFS_PARAMS_SAVED=""; UBLK_UP=""
SKZ_PID=""; SKZ_UP=""; SKZ_DEV=""; SKZ_SAMPLER=""; SKZ_CGROUP=""; SKZ_DIR=$WORK/skyzfsd-vol; SKZ_RUN=$WORK/skyzfsd-run
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
    if [ -n "$SNAP_PID" ]; then
        # Never kill the snapshot loop: it may be inside a freeze. Ask
        # it to stop and wait; then undo anything it could have left.
        touch "$SNAP_STOP"; wait "$SNAP_PID" 2>/dev/null; SNAP_PID=""
    fi
    if [ -n "$ANY_SNAP" ]; then
        fsfreeze -u "$MNT_WAL" 2>/dev/null; fsfreeze -u "$MNT" 2>/dev/null
        dmsetup resume tb-data 2>/dev/null; dmsetup resume tb-wal 2>/dev/null
    fi
    if [ -n "$SKZ_SAMPLER" ]; then
        kill "$SKZ_SAMPLER" 2>/dev/null; SKZ_SAMPLER=""
    fi
    stop_pg
    if [ -n "$SKZ_UP" ]; then
        # The pool is on the daemon's device. If the daemon is gone the
        # pool waits for it, and an unmount or an export then never
        # returns: each of them has a limit here, and what could not be
        # undone is said.
        lim 120 umount "$MNT" 2>/dev/null; lim 120 umount "$MNT_WAL" 2>/dev/null
    else
        umount "$MNT" 2>/dev/null; umount "$MNT_WAL" 2>/dev/null
    fi
    umount "$WORK/mnt-src" 2>/dev/null
    if [ -n "$ZFS_UP" ] && [ -n "$SKZ_UP" ]; then
        lim 300 zpool export -f "$ZFS_POOL" 2>/dev/null || lim 120 zpool destroy -f "$ZFS_POOL" 2>/dev/null \
            || log "WARNING: pool $ZFS_POOL is still imported (its state: $(cat "/proc/spl/kstat/zfs/$ZFS_POOL/state" 2>/dev/null || echo unknown))"
        ZFS_UP=""
    elif [ -n "$ZFS_UP" ]; then
        # The pool is scratch: if it will not export, destroy it rather
        # than leave an imported pool holding the dm devices below.
        zpool export -f "$ZFS_POOL" 2>/dev/null || zpool destroy -f "$ZFS_POOL" 2>/dev/null \
            || log "WARNING: pool $ZFS_POOL is still imported"
        ZFS_UP=""
    fi
    if [ -n "$SKZ_UP" ]; then
        # After the export, like the reference server's device below:
        # an orderly stop, then the device is detached.
        skz_stop
        SKZ_UP=""
    fi
    if [ -n "$UBLK_UP" ]; then
        # After the export: deleting a device under an imported pool
        # would leave the pool waiting on a vdev that is gone.
        timeout 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1 || log "WARNING: ublk device $UBLK_ID was not deleted"
        UBLK_UP=""
    fi
    for kv in $ZFS_PARAMS_SAVED; do
        echo "${kv#*=}" > "/sys/module/zfs/parameters/${kv%%=*}" 2>/dev/null
    done
    ZFS_PARAMS_SAVED=""
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
             tb-zdev tb-zrcache tb-zcmeta tb-zcdata tb-zorigin; do
        dmsetup remove "$d" 2>/dev/null
    done
    for l in "$POOL_LOOP" "$CACHE_LOOP" "$BSB_LOOP" "$ZORIGIN_LOOP" "$ZSRC_LOOP" "$SLOG_LOOP"; do
        [ -n "$l" ] && losetup -d "$l" 2>/dev/null
    done
    # The ZFS origin is per topology (its properties differ), and big.
    rm -f "$WORK/zfs-origin.img"
    POOL_LOOP=""; CACHE_LOOP=""; BSB_LOOP=""; FS_DEV=""; TWO_VOL=""; WAL_FROM_ORIGIN=""
    ZORIGIN_LOOP=""; ZSRC_LOOP=""; ZFS_WIN=""; STAT_ORIGIN=""; STAT_POOL=""; SLOG_LOOP=""; ZFS_RCACHE=""
    SKZ_DEV=""
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
    mkflash "$WORK/pool.img" "$(mb $((POOL_GB * 1024)))"
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
           iflag=direct oflag=direct conv=notrunc,nocreat status=none &
    done
    wait
    sync
    log "$1: hydrated ${total} MiB in $(( $(date +%s) - t0 ))s ($HYDRATE_JOBS streams); pool: $(dmsetup status tb-pool | awk '{print $6}') data blocks used/total"
}
build_cache_dev() { # [MiB]: default TB_CACHE_PCT of the database
    local bytes=${1:-$(( DB_MB * CACHE_PCT / 100 ))}
    [ "$bytes" -lt 64 ] && bytes=64
    mkflash "$WORK/cache.img" "$(mb "$bytes")"
    CACHE_LOOP=$(attach_loop "$WORK/cache.img")
    log "cache device: ${bytes} MiB ($(( bytes * 100 / DB_MB ))% of the database) on $CACHE_LOOP"
}

# ------------------------------------------------------- ZFS stack
attach_loop_ro() {
    local l
    l=$(losetup --find --show --read-only --direct-io=on "$1" 2>/dev/null || losetup --find --show --read-only "$1")
    [ "$(loop_dio "$l")" = 1 ] || log "WARNING: $l on $(basename "$1") is BUFFERED (no direct I/O)"
    echo "$l"
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
zfs_param_set() { # name value: set a zfs module parameter, saving the old value for teardown
    local p=/sys/module/zfs/parameters/$1
    [ -e "$p" ] || return 1
    ZFS_PARAMS_SAVED="$ZFS_PARAMS_SAVED $1=$(cat "$p")"
    echo "$2" > "$p"
}
zfs_set_params() { # TB_ZFS_PARAMS: "name=value ..." module parameters
    local kv
    # Keep every transaction group of a window in the txgs kstat.
    zfs_param_set zfs_txg_history 1000 || true
    for kv in $ZFS_PARAMS; do
        zfs_param_set "${kv%%=*}" "${kv#*=}" \
            || { echo "TB_ZFS_PARAMS: no such zfs module parameter: ${kv%%=*}" >&2; return 1; }
        log "zfs parameter ${ZFS_PARAMS_SAVED##* } -> $(cat "/sys/module/zfs/parameters/${kv%%=*}")"
    done
}
zfs_set_props() { # TB_ZFS_PROPS: "[data:|wal:]property=value,..."
    local kv ds
    for kv in ${ZFS_PROPS//,/ }; do
        ds=$ZFS_POOL
        case $kv in data:*|wal:*) ds=$ZFS_POOL/${kv%%:*}; kv=${kv#*:} ;; esac
        zfs set "$kv" "$ds"
        log "zfs property $kv on $ds"
    done
}
l2arc_params() { # the feed's settings, for the log
    local n out=""
    for n in l2arc_write_max l2arc_headroom l2arc_noprefetch l2arc_mfuonly l2arc_dwpd_limit; do
        out="$out $n=$(cat "/sys/module/zfs/parameters/$n" 2>/dev/null || echo absent)"
    done
    echo "$out"
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
    local odev sect chunk ro=--readonly
    if [ -n "$ZFS_RCACHE" ]; then
        # dm-cache opens its origin for writing even though a
        # writethrough cache that only sees reads never writes it, so
        # the origin path cannot be read-only here.
        ZORIGIN_LOOP=$(attach_loop "$WORK/zfs-origin.img"); ro=""
    else
        ZORIGIN_LOOP=$(attach_loop_ro "$WORK/zfs-origin.img")
    fi
    odev=$ZORIGIN_LOOP
    sect=$(sectors "$ZORIGIN_LOOP")
    if [ "$ORIGIN_DELAY_MS" -gt 0 ]; then
        # shellcheck disable=SC2086  # $ro is one flag or nothing
        dmsetup create $ro tb-zorigin --table \
            "0 $sect delay $ZORIGIN_LOOP 0 $ORIGIN_DELAY_MS $ZORIGIN_LOOP 0 0"
        odev=/dev/mapper/tb-zorigin
    fi
    if [ -n "$ZFS_RCACHE" ]; then
        # The backend's read cache: what was fetched from the origin
        # stays on local flash. Below the dm-snapshot, so it sees only
        # reads of never-written blocks; locally written blocks do not
        # compete for it, and no write is ever doubled.
        build_cache_dev
        local total meta data block
        total=$(sectors "$CACHE_LOOP")
        meta=$(( total / 500 / 8 * 8 )); [ "$meta" -lt $(( 8 << 11 )) ] && meta=$(( 8 << 11 ))
        block=$(( CACHE_BLOCK_KB * 2 ))
        data=$(( (total - meta) / block * block ))
        zero_head "$CACHE_LOOP"  # "format me"
        dmsetup create tb-zcmeta --table "0 $meta linear $CACHE_LOOP 0"
        dmsetup create tb-zcdata --table "0 $data linear $CACHE_LOOP $meta"
        dmsetup create tb-zrcache --table \
            "0 $sect cache /dev/mapper/tb-zcmeta /dev/mapper/tb-zcdata $odev $block 1 writethrough smq 0"
        if [ -n "$MIGRATION_THRESHOLD" ]; then
            dmsetup message tb-zrcache 0 "migration_threshold $MIGRATION_THRESHOLD"
        fi
        odev=/dev/mapper/tb-zrcache
    fi
    # The COW store is local flash. Transient ("N"): no on-disk
    # exception metadata, so a flush is the data device's flush alone.
    # Sized to the whole vdev because copy-on-write can sweep it.
    mkflash "$WORK/pool.img" "$(mb $((ZFS_GB * 1024 + 64)))"
    POOL_LOOP=$(attach_loop "$WORK/pool.img")
    chunk=$(( (1 << ZFS_ASHIFT) / 512 ))
    dmsetup create tb-zdev --table "0 $sect snapshot $odev $POOL_LOOP N $chunk"
    STAT_ORIGIN=$(basename "$ZORIGIN_LOOP"); STAT_POOL=$(basename "$POOL_LOOP")
    zfs_cap_arc
    zfs_set_params
    local t0; t0=$(date +%s%N)
    zpool import -N -o cachefile=none -d /dev/mapper/tb-zdev "$ZFS_POOL"
    ZFS_UP=1
    log "zpool import over the ${ORIGIN_DELAY_MS} ms origin took $(( ($(date +%s%N) - t0) / 1000000 )) ms"
    zfs_set_props
    FS_DEV=/dev/mapper/tb-zdev
}
# ---- zfs-skyzfsd: the project's daemon serves the vdev
skz_status() { lim 30 "$SKYZFSD" status --dir "$SKZ_DIR" --run-dir "$SKZ_RUN" 2>/dev/null || true; }
# key -> its value. awk reads to the end: one that left early could end
# the writer with SIGPIPE, which pipefail turns into a failure here.
skz_stat() { skz_status | awk -v k="$1" '$1 == k && !seen { seen = 1; $1 = ""; sub(/^ /, ""); print }'; }
# The low mark of the log in bytes: `--log-low N` of TB_SKYZFSD_ARGS in
# segments of 64 MiB, or the daemon's default of 1.
skz_low_bytes() {
    local n
    n=$(printf '%s\n' "$SKYZFSD_ARGS" | tr ',' '\n' | awk 'prev == "--log-low" { print; exit } { prev = $0 } /^--log-low=/ { sub(/^--log-low=/, ""); print; exit }')
    [[ $n =~ ^[0-9]+$ ]] || n=1
    echo $(( n * 64 << 20 ))
}
skz_start() { # serve in the background; SKZ_DEV is the device it says is ready
    local i args=()
    [ -z "$SKYZFSD_ARGS" ] || read -r -a args <<< "${SKYZFSD_ARGS//,/ }"
    : > "$WORK/skyzfsd.ready"
    # A cgroup of the daemon's own, with or without a limit: its file
    # pages (the home image and the log, through the page cache) are
    # then counted apart from everything else on the machine. The shell
    # joins it before it becomes the daemon, so the first page the
    # daemon touches is charged there.
    SKZ_CGROUP=""
    if [ -f /sys/fs/cgroup/cgroup.controllers ]; then
        SKZ_CGROUP=/sys/fs/cgroup/topology-bench-skyzfsd
        mkdir -p "$SKZ_CGROUP"
        echo "${SKYZFSD_MEM:-max}" > "$SKZ_CGROUP/memory.max"
    elif [ -n "$SKYZFSD_MEM" ]; then
        echo "TB_SKYZFSD_MEM needs cgroup v2 at /sys/fs/cgroup" >&2
        return 1
    fi
    # shellcheck disable=SC2016  # the inner shell expands them
    bash -c '[ -z "$1" ] || echo $$ > "$1/cgroup.procs" || exit 97; shift; exec "$@"' skz "$SKZ_CGROUP" \
        "$SKYZFSD" serve --dir "$SKZ_DIR" --run-dir "$SKZ_RUN" "${args[@]}" > "$WORK/skyzfsd.ready" 2>> "$OUT/skyzfsd-serve.log" &
    SKZ_PID=$!
    for i in $(seq 1 240); do
        SKZ_DEV=$(sed -n 's/^ready \(\/dev\/ublkb[0-9]*\)$/\1/p' "$WORK/skyzfsd.ready" | head -n 1)
        if [ -n "$SKZ_DEV" ] && [ -b "$SKZ_DEV" ]; then return 0; fi
        if ! kill -0 "$SKZ_PID" 2>/dev/null; then
            echo "skyzfsd serve ended before it was ready: $(tail -n 3 "$OUT/skyzfsd-serve.log" | tr '\n' ';')" >&2
            SKZ_PID=""
            return 1
        fi
        sleep 0.25
    done
    echo "skyzfsd serve was not ready after 60 s: $(tail -n 3 "$OUT/skyzfsd-serve.log" | tr '\n' ';')" >&2
    return 1
}
skz_stop() { # SIGTERM, a wait of 60 s, then SIGKILL; the device detached; the volume removed
    local i
    if [ -n "$SKZ_PID" ]; then
        kill -TERM "$SKZ_PID" 2>/dev/null
        for i in $(seq 1 240); do kill -0 "$SKZ_PID" 2>/dev/null || break; sleep 0.25; done
        if kill -0 "$SKZ_PID" 2>/dev/null; then
            log "WARNING: skyzfsd did not end within 60 s of SIGTERM; killed"
            kill -9 "$SKZ_PID" 2>/dev/null
        else
            wait "$SKZ_PID" 2>/dev/null; i=$?
            [ "$i" = 0 ] || log "WARNING: skyzfsd serve ended with $i"
        fi
        SKZ_PID=""
    fi
    lim 90 "$SKYZFSD" detach --dir "$SKZ_DIR" --run-dir "$SKZ_RUN" >/dev/null 2>&1 \
        || log "WARNING: skyzfsd detach did not succeed; ublk devices: $(lim 20 ls /dev 2>/dev/null | grep -c '^ublkb' || true)"
    rm -rf -- "$SKZ_DIR" "$SKZ_RUN"
    if [ -n "$SKZ_CGROUP" ]; then rmdir "$SKZ_CGROUP" 2>/dev/null; SKZ_CGROUP=""; fi
}
# Wait for the daemon to be at rest after the copy, so that a window
# does not begin on the copy's backlog: no checkpoint step under way,
# nothing unstored, nothing held, and the write count the same over a
# second. Then `log_live` is looked at: under the low mark is what a
# finished checkpoint leaves. If the daemon rests with the log at or
# above the low mark (a step only begins at the high mark), waiting
# longer changes nothing: after 30 s of rest the script goes on and the
# row says where the log stood. 1: never at rest in 600 s.
skz_settle() {
    local low t0 step unst held w0 w1 ll rest=0 said=""
    low=$(skz_low_bytes)
    t0=$(date +%s)
    while [ $(( $(date +%s) - t0 )) -lt 600 ]; do
        w0=$(skz_stat writes); sleep 1
        w1=$(skz_stat writes); step=$(skz_stat checkpoint_step); unst=$(skz_stat unstored); held=$(skz_stat held); ll=$(skz_stat log_live)
        if [ "$step" = none ] && [ "$unst" = 0 ] && [ "$held" = 0 ] && [ -n "$w0" ] && [ "$w0" = "$w1" ] && [[ $ll =~ ^[0-9]+$ ]]; then
            rest=$((rest + 1))
            if [ "$ll" -lt "$low" ]; then
                said="at rest after $(( $(date +%s) - t0 )) s: no step under way, unstored 0, held 0, log_live $ll under the low mark of $low"
                break
            elif [ "$rest" -ge 30 ]; then
                said="at rest for 30 s after $(( $(date +%s) - t0 )) s: no step under way, unstored 0, held 0, but log_live $ll is NOT under the low mark of $low: the window begins with that much in the log"
                break
            fi
        else
            rest=0
        fi
    done
    if [ -z "$said" ]; then
        echo "skyzfsd did not come to rest within 600 s of the copy: checkpoint_step '${step:-}', unstored '${unst:-}', held '${held:-}', writes '${w0:-}' then '${w1:-}', log_live '${ll:-}'" >&2
        return 1
    fi
    log "skyzfsd: $said"
    echo "settle $said" >> "$OUT/$TOPO.skzinfo"
}
skz_sample() { # file: one line a second, "epoch key=value ...", until killed
    while :; do
        # A pool whose daemon is gone waits for it, and pgbench with it,
        # until the job's limit. End the measurement instead: pgbench
        # is killed, the script fails on that, and its teardown has
        # limits.
        if ! kill -0 "$SKZ_PID" 2>/dev/null; then
            log "skyzfsd (pid $SKZ_PID) is GONE during the window; its log's end: $(tail -n 5 "$OUT/skyzfsd-serve.log" | tr '\n' ';'). Ending pgbench: this run has no result"
            echo "$(date +%s) daemon_gone=1" >> "$1"
            pkill -9 -x pgbench 2>/dev/null || true
            return 0
        fi
        printf '%s %s\n' "$(date +%s)" "$(skz_status | awk '$1 ~ /^(log_live|checkpoint_steps|checkpoint_syncs|checkpoint_step|writes|unstored|held)$/ { v = $2; if ($3 != "") v = v "_" $3; printf "%s=%s ", $1, v }')" >> "$1"
        sleep 1
    done
}
build_zfs_direct() { # ublk|loop|skyzfsd: the pool imported from its own image, served read-write by that device
    local dev t0
    if [ "$ORIGIN_DELAY_MS" -gt 0 ]; then
        log "WARNING: $TOPO has no origin under it: TB_ORIGIN_DELAY_MS=$ORIGIN_DELAY_MS does not apply to it"
    fi
    [ "$PREALLOC" != 1 ] || log "WARNING: TB_PREALLOC does not reach $TOPO's pool image, which stays sparse"
    case $1 in
        ublk)
            dev=/dev/ublkb$UBLK_ID
            timeout 60 ublk del -n "$UBLK_ID" >/dev/null 2>&1 || true   # a previous run's leftover
            # shellcheck disable=SC2086  # a comma-separated argument list by contract
            timeout 60 ublk add -t loop -n "$UBLK_ID" -f "$WORK/zfs-origin.img" ${UBLK_ARGS//,/ } >&2
            UBLK_UP=1
            wait_for "$dev" 20 || { echo "ublk add returned, but $dev did not appear" >&2; return 1; }
            log "ublk: $(ublk list -n "$UBLK_ID" 2>&1 | tr -s ' \t\n' ' ')"
            ;;
        loop)
            ZORIGIN_LOOP=$(attach_loop "$WORK/zfs-origin.img")
            dev=$ZORIGIN_LOOP
            ;;
        skyzfsd)
            # A previous run's leftover: its device and its files.
            lim 90 "$SKYZFSD" detach --dir "$SKZ_DIR" --run-dir "$SKZ_RUN" >/dev/null 2>&1 || true
            rm -rf -- "$SKZ_DIR" "$SKZ_RUN"
            mkdir -p "$SKZ_RUN"
            : > "$OUT/$TOPO.skzinfo"
            lim 120 "$SKYZFSD" create --dir "$SKZ_DIR" --size "$(stat -c %s "$WORK/zfs-origin.img")" >&2 \
                || { echo "skyzfsd create failed or did not end in 120 s" >&2; return 1; }
            SKZ_UP=1
            skz_start
            dev=$SKZ_DEV
            log "skyzfsd: serve ${SKYZFSD_ARGS//,/ } -> $dev, pid $SKZ_PID, memory.max of its cgroup: $(cat "$SKZ_CGROUP/memory.max" 2>/dev/null || echo 'no cgroup') (cgroup of the pid: $(cut -d: -f3 "/proc/$SKZ_PID/cgroup" 2>/dev/null | tr '\n' ' '))"
            echo "mem_max ${SKYZFSD_MEM:-none}" >> "$OUT/$TOPO.skzinfo"
            # The exported image onto the served device, through the
            # daemon. A 1 MiB block of zeros is left out, so what the
            # image never held the daemon is never given.
            t0=$(date +%s%N)
            "$DD" if="$WORK/zfs-origin.img" of="$dev" bs=1M oflag=direct conv=sparse,nocreat status=none
            sync
            t0=$(( ($(date +%s%N) - t0) / 1000000 ))
            log "skyzfsd: the pool image ($(du -m "$WORK/zfs-origin.img" | cut -f1) MiB allocated of $(( $(stat -c %s "$WORK/zfs-origin.img") >> 20 )) MiB) was copied onto $dev in $t0 ms, direct I/O, conv=sparse; the daemon counted $(skz_stat writes) writes, checkpoint_steps $(skz_stat checkpoint_steps), log_live $(skz_stat log_live)"
            echo "copy_ms $t0" >> "$OUT/$TOPO.skzinfo"
            echo "serve_args ${SKYZFSD_ARGS//,/ }" >> "$OUT/$TOPO.skzinfo"
            echo "build ${TB_SKYZFSD_REF:-not said}" >> "$OUT/$TOPO.skzinfo"
            # Every status of this row is read by skz_status, with this
            # row's own binary: a newer `status` prints an older status
            # file's missing keys as zeros, which skz_time.py would read
            # as that daemon's (a reader thread that never ran).
            echo "status_reader $(basename "$SKYZFSD") sha256:$(sha256sum "$SKYZFSD" | cut -c1-12)" >> "$OUT/$TOPO.skzinfo"
            skz_settle
            ;;
        *) echo "build_zfs_direct: $1?" >&2; return 1 ;;
    esac
    log "$TOPO: vdev $dev, logical/physical block $(cat "/sys/block/$(basename "$dev")/queue/logical_block_size")/$(cat "/sys/block/$(basename "$dev")/queue/physical_block_size"), max_sectors_kb $(cat "/sys/block/$(basename "$dev")/queue/max_sectors_kb")"
    STAT_ORIGIN=$(basename "$dev"); STAT_POOL=$(basename "$dev")
    zfs_cap_arc
    zfs_set_params
    t0=$(date +%s%N)
    zpool import -N -o cachefile=none -d "$dev" "$ZFS_POOL"
    ZFS_UP=1
    log "zpool import from $dev took $(( ($(date +%s%N) - t0) / 1000000 )) ms"
    zfs_set_props
    FS_DEV=$dev
}
zfs_stats() { # counters as "zfsc key n" (the window's delta), gauges as "zfsg key n"
    awk 'NR > 2 { print "zfsc zil." $1, $3 }' /proc/spl/kstat/zfs/zil
    awk 'NR > 2 && $1 ~ /^(hits|iohits|misses|l2_hits|l2_misses|l2_read_bytes|l2_write_bytes|memory_throttle_count)$/ { print "zfsc arc." $1, $3 }
         NR > 2 && $1 ~ /^(size|c_max|compressed_size|uncompressed_size|l2_size|l2_asize|anon_size)$/ { print "zfsg arc." $1, $3 }' \
        /proc/spl/kstat/zfs/arcstats
    # Transactions that were made to wait, and why.
    awk 'NR > 2 { print "zfsc dmutx." $1, $3 }' /proc/spl/kstat/zfs/dmu_tx 2>/dev/null
    # Per-dataset counters: ZIL bytes by dataset tell WAL commits from
    # the checkpointer's fsync of data files.
    local f
    for f in "/proc/spl/kstat/zfs/$ZFS_POOL"/objset-*; do
        [ -r "$f" ] || continue
        awk 'NR > 2 && $1 == "dataset_name" { n = $3; sub(/.*\//, "", n) }
             NR > 2 && $1 != "dataset_name" && $3 ~ /^[0-9]+$/ { v[$1] = $3 }
             END { if (n != "" && n !~ /@/) for (k in v) print "zfsc ds." n "." k, v[k] }' "$f"
    done
    # Completed transaction groups: txg ndirty nwritten otime qtime wtime stime (bytes, ns).
    awk '$3 == "C" { print "zfstxg", $1, $4, $6, $9, $10, $11, $12 }' "/proc/spl/kstat/zfs/$ZFS_POOL/txgs" 2>/dev/null
    # Growth of the COW store = bytes written to LBAs never written
    # since the stage. dm-snapshot status: <allocated>/<total> sectors.
    # (zfs-ublk and zfs-loop have no COW store, and no such figure.)
    if dmsetup info tb-zdev >/dev/null 2>&1; then
        echo "zfsc cow.allocated_bytes $(dmsetup status tb-zdev | awk '{ split($4, a, "/"); print a[1] * 512 }')"
    fi
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
    log "l2arc: $CACHE_LOOP$(l2arc_params)"
}
topo_zfs_slog_l2arc() {
    topo_zfs_slog
    # WAL is never read back in normal running: keep it out of the cache.
    zfs set secondarycache=none "$ZFS_POOL/wal"
    # Twice the ARC: below that an L2ARC wraps faster than its contents
    # are asked for again (zfs-l2arc), and upstream's write limiter only
    # engages from that size.
    local mb=$ZFS_L2ARC_MB
    [ -n "$mb" ] || mb=$(( ${ZFS_ARC_MB:-0} > 0 ? ZFS_ARC_MB * 2 : DB_MB * 2 ))
    build_cache_dev "$mb"
    zpool add -f "$ZFS_POOL" cache "$CACHE_LOOP"
    log "l2arc: $CACHE_LOOP, ${mb} MiB, WAL excluded;$(l2arc_params)"
}
topo_zfs_slog_rcache() { ZFS_RCACHE=1; topo_zfs_slog; }
topo_zfs_slog() { topo_zfs; add_slog; }
topo_zfs_ublk() { build_zfs_origin "$ZFS_COMPRESSION" "$ZFS_FPW"; build_zfs_direct ublk; add_slog; }
topo_zfs_loop() { build_zfs_origin "$ZFS_COMPRESSION" "$ZFS_FPW"; build_zfs_direct loop; add_slog; }
topo_zfs_skyzfsd() { build_zfs_origin "$ZFS_COMPRESSION" "$ZFS_FPW"; build_zfs_direct skyzfsd; add_slog; }
add_slog() {
    # The log device is plain local flash: ZIL blocks land here and
    # never on the vdev. A cut of the vdev then holds the last synced
    # transaction group, not the fsynced tail.
    mkimage "$WORK/slog.img" "$(mb "$ZFS_SLOG_MB")"
    if [ "$ZFS_SLOG_PREALLOC" = 1 ] || [ "$PREALLOC" = 1 ]; then
        "$DD" if=/dev/zero of="$WORK/slog.img" bs=1M count="$ZFS_SLOG_MB" \
            oflag=direct conv=notrunc,fsync status=none
    fi
    SLOG_LOOP=$(attach_loop "$WORK/slog.img")
    zpool add -f "$ZFS_POOL" log "$SLOG_LOOP"
    log "slog: ${ZFS_SLOG_MB} MiB on $SLOG_LOOP$([ "$ZFS_SLOG_PREALLOC$PREALLOC" != 00 ] && echo ', written through once')"
}

# ------------------------------------------- snapshots in the window
take_snap() { # n mode: one snapshot of the stack. Leaves nothing frozen or suspended.
    local n=$1 mode=$2 rc=0 d
    if [ -n "$ZFS_UP" ]; then
        # Both datasets, atomically, in one transaction group. Blocks
        # this caller until the group is synced; nobody else.
        zfs snapshot -r "$ZFS_POOL@tbs$n" || rc=1
        return $rc
    fi
    local devs="tb-data:0"
    [ -n "$TWO_VOL" ] && devs="$devs tb-wal:1"
    if [ "$mode" = freeze ]; then
        fsfreeze -f "$MNT" || rc=1
        if [ -n "$TWO_VOL" ]; then fsfreeze -f "$MNT_WAL" || rc=1; fi
    fi
    # --nolockfs: dm's own suspend would otherwise freeze a filesystem
    # mounted on the device, which is the thing `suspend` mode omits
    # and `freeze` mode has already done.
    for d in $devs; do dmsetup suspend --nolockfs "${d%%:*}" || rc=1; done
    for d in $devs; do
        dmsetup message tb-pool 0 "create_snap $(( 100 + 2 * n + ${d##*:} )) ${d##*:}" || rc=1
    done
    for d in $devs; do dmsetup resume "${d%%:*}" || rc=1; done
    if [ "$mode" = freeze ]; then
        [ -n "$TWO_VOL" ] && fsfreeze -u "$MNT_WAL"
        fsfreeze -u "$MNT" || rc=1
    fi
    return $rc
}
drop_snap() { # n: delete snapshot n (untimed; its cost lands in the workload)
    local n=$1
    if [ -n "$ZFS_UP" ]; then
        zfs destroy -r "$ZFS_POOL@tbs$n" 2>/dev/null
    else
        dmsetup message tb-pool 0 "delete $(( 100 + 2 * n ))" 2>/dev/null
        [ -n "$TWO_VOL" ] && dmsetup message tb-pool 0 "delete $(( 101 + 2 * n ))" 2>/dev/null
    fi
    return 0
}
snap_loop() { # file mode: a snapshot every SNAP_INTERVAL s until the stop file appears
    local out=$1 mode=$2 n=0 i tp t0 t1 rc
    set +e
    while :; do
        for (( i = 0; i < SNAP_INTERVAL; i++ )); do
            [ -e "$SNAP_STOP" ] && return 0
            sleep 1
        done
        # `sync`: write the dirty backlog out before the snapshot, as a
        # freeze would, but with writers running. Not part of the
        # bracket: nothing is paused while it runs.
        tp=$(date +%s%N); rc=0
        if [ "$mode" = sync ] && [ -z "$ZFS_UP" ]; then
            sync -f "$MNT" || rc=1
            if [ -n "$TWO_VOL" ]; then sync -f "$MNT_WAL" || rc=1; fi
        fi
        t0=$(date +%s%N)
        take_snap "$n" "$mode" || rc=1
        t1=$(date +%s%N)
        # epoch-seconds  bracket-ms  rc  ms-before-the-bracket
        echo "$(( t1 / 1000000000 )) $(( (t1 - t0) / 1000000 )) $rc $(( (t0 - tp) / 1000000 ))" >> "$out"
        [ "$n" -ge "$SNAP_KEEP" ] && drop_snap $(( n - SNAP_KEEP ))
        n=$(( n + 1 ))
    done
}

# --------------------------------------------------------- measuring
diskstats_line() { # kernel device name -> "reads rsect writes wsect flush"
    awk -v d="$1" '$3==d { f = (NF >= 19) ? $19 : 0; print $4, $6, $8, $10, f }' /proc/diskstats
}
diag_stats() { # where the time goes: CPU, context switches, postgres's own I/O timers
    local skzmax
    awk '$1 == "cpu" { print "cpu total", $2, $3, $4, $5, $6, $7, $8, $9 }
         $1 == "ctxt" { print "cpu ctxt", $2 }' /proc/stat
    # CPU ticks by thread name (kernel threads included). A process's
    # reaped children are in its cutime/cstime: postgres backends land
    # on the postmaster, pgbench on the shell that ran it.
    python3 - <<'PY' || true
import collections, glob, re
c = collections.Counter()
def parse(path):
    s = open(path).read()
    return s[s.index('(') + 1:s.rindex(')')], s[s.rindex(')') + 2:].split()
def norm(comm):
    return (re.sub(r'[-_]?\d+$', '', re.split(r'[/:]', comm)[0]) or comm).replace(' ', '_')
for p in glob.glob('/proc/[0-9]*'):
    try:
        for t in glob.glob(p + '/task/[0-9]*/stat'):
            comm, f = parse(t)
            c[norm(comm)] += int(f[11]) + int(f[12])
        comm, f = parse(p + '/stat')
        c[norm(comm) + '(reaped)'] += int(f[13]) + int(f[14])
    except (OSError, ValueError, IndexError):
        pass
for k, v in c.most_common():
    if v: print('thr', k, v)
PY
    as_pg "'$PG_BIN/psql' -h '$SOCK' -d bench -Atq -F ' ' -f '$WORK/io-stats.sql'" 2>/dev/null | sed 's/^/pg /' || true
    # Who holds the memory: the host's page cache (a buffered loop
    # would put the images there, outside the cap), postgres's cgroup
    # by kind, and the kernel's own.
    awk '$1 ~ /^(MemTotal|MemFree|MemAvailable|Cached|Buffers|Dirty|Writeback|Slab|SUnreclaim):$/ { sub(":", "", $1); print "mem host." $1, $2 * 1024 }' /proc/meminfo
    if [ -n "${CGROUP:-}" ] && [ -f "$CGROUP/memory.stat" ]; then
        echo "mem cgroup.current $(cat "$CGROUP/memory.current")"
        awk '$1 ~ /^(anon|file|shmem|kernel|slab|file_dirty|file_writeback)$/ { print "mem cgroup." $1, $2 }' "$CGROUP/memory.stat"
    fi
    # The ARC's limit: the cache the model allows under postgres.
    [ ! -r /sys/module/zfs/parameters/zfs_arc_max ] || echo "mem zfs.arc_max $(cat /sys/module/zfs/parameters/zfs_arc_max)"
    # The daemon: its own resident memory, and what its cgroup holds
    # (file = the page cache of its home image and log).
    if [ -n "$SKZ_PID" ]; then
        awk '$1 ~ /^(VmRSS|VmLck|RssAnon|RssFile|RssShmem):$/ { sub(":", "", $1); print "mem skz." $1, $2 * 1024 }' "/proc/$SKZ_PID/status" 2>/dev/null || true
        if [ -n "$SKZ_CGROUP" ] && [ -f "$SKZ_CGROUP/memory.stat" ]; then
            echo "mem skz.cgroup.current $(cat "$SKZ_CGROUP/memory.current")"
            skzmax=$(cat "$SKZ_CGROUP/memory.max"); [ "$skzmax" = max ] || echo "mem skz.cgroup.max $skzmax"
            awk '$1 ~ /^(anon|file|shmem|kernel|file_dirty|file_writeback)$/ { print "mem skz.cgroup." $1, $2 }' "$SKZ_CGROUP/memory.stat"
            awk '$1 ~ /^(max|high|oom|oom_kill)$/ { print "mem skz.cgroup.events." $1, $2 * 1048576 }' "$SKZ_CGROUP/memory.events" 2>/dev/null || true
        fi
    fi
    losetup --list --noheadings --output NAME,DIO,RO,BACK-FILE 2>/dev/null \
        | awk '{ n = split($4, a, "/"); print "loop", a[n], $2, $3 }' || true
}
fsync_test() { # the commit path with nothing else running: pg_test_fsync on the WAL filesystem
    rm -f "$OUT/$TOPO.fsync"
    if [ "$FSYNC_TEST" != 1 ] || [ ! -x "$PG_BIN/pg_test_fsync" ]; then return 0; fi
    local d
    d=$(dirname "$(readlink -f "$PGDATA/pg_wal")")/tb-fsync
    mkdir -p "$d"; chown "$PG_USER" "$d"
    as_pg "'$PG_BIN/pg_test_fsync' -s 2 -f '$d/test'" > "$OUT/$TOPO.fsync" 2>&1 || true
    rm -f "$d/test"; rmdir "$d" 2>/dev/null || true
    log "$TOPO: one 8 kB write + sync on the WAL filesystem: $(awk '/one 8kB write/ { on = 1; next } /^Compare/ { on = 0 } on && /usecs\/op/ { printf "%s=%sus ", $1, $(NF - 1) }' "$OUT/$TOPO.fsync")"
}
snapshot() { # file
    {
        diag_stats
        [ -n "$POOL_LOOP" ] && echo "dev poolloop $(diskstats_line "$(basename "$POOL_LOOP")")"
        echo "dev origin $(diskstats_line "${STAT_ORIGIN:-$(basename "$ORIGIN_LOOP")}")"
        echo "dev walorigin $(diskstats_line "$(basename "$WAL_ORIGIN_LOOP")")"
        echo "dev pool $(diskstats_line "${STAT_POOL:-$(dm_blk tb-tdata)}")"
        [ -n "$TWO_VOL" ] && echo "dev wal $(diskstats_line "$(dm_blk tb-wal)")"
        [ -n "$CACHE_LOOP" ] && echo "dev cache $(diskstats_line "$(basename "$CACHE_LOOP")")"
        [ -n "$SLOG_LOOP" ] && echo "dev slog $(diskstats_line "$(basename "$SLOG_LOOP")")"
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
            zfs*) zfs_stats; [ -z "$ZFS_RCACHE" ] || echo "dmcache $(dmsetup status tb-zrcache)" ;;
        esac
        # The daemon's whole status, "skz key value" (a value of two
        # words joined by _).
        # The clock just before the reading: the worker's times of two
        # readings must add up to the time between them.
        [ -z "$SKZ_UP" ] || echo "skz read_at_ns $(date +%s%N)"
        [ -z "$SKZ_UP" ] || skz_status | awk '{ k = $1; $1 = ""; sub(/^ /, ""); gsub(/ /, "_"); print "skz", k, $0 }'
        # Whether the pool has seen an error on the daemon's device: a
        # server that returned wrong data shows here first (a checksum
        # error), and it is the one check on correctness this benchmark
        # has. `zpool status -x` says "pool 'NAME' is healthy" or what
        # is wrong.
        [ -z "$SKZ_UP" ] || echo "skzpool $(lim 60 zpool status -x "$ZFS_POOL" 2>&1 | tr -s ' \t\n' ' ' | cut -c1-300)"
    } > "$1"
}

run_workload() { # topology workload [snapshot mode]
    local topo=$1 wl=$2 mode=${3:-none} builtin tag="$1-$2" plog taken i
    [ "$mode" = none ] || tag="$tag+$mode"
    # Per-transaction latencies, for the tail the average hides.
    plog="$WORK/pgbench-log/$tag"
    mkdir -p "$WORK/pgbench-log"; chown "$PG_USER" "$WORK/pgbench-log"
    rm -f "$plog".*
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
    rm -f "$OUT/$tag.snaps"
    if [ "$mode" != none ]; then
        SNAP_STOP="$OUT/$tag.snapstop"; rm -f "$SNAP_STOP"
        snap_loop "$OUT/$tag.snaps" "$mode" &
        SNAP_PID=$!
    fi
    if [ -n "$SKZ_UP" ]; then
        log "$tag: skyzfsd before the window: $(grep '^skz ' "$OUT/$tag.before" | cut -d' ' -f2- | tr '\n' ';')"
        : > "$OUT/$tag.skz"
        skz_sample "$OUT/$tag.skz" &
        SKZ_SAMPLER=$!
    fi
    log "$tag: measuring ${DURATION}s$([ "$mode" = none ] || echo ", a $mode snapshot every ${SNAP_INTERVAL}s, keeping $SNAP_KEEP")"
    as_pg "'$PGBENCH' -h '$SOCK' -T $DURATION -c $CLIENTS -j $JOBS -P 30 -l --log-prefix='$plog' -b $builtin bench" > "$OUT/$tag.pgbench" 2>&1
    if [ -n "$SNAP_PID" ]; then
        touch "$SNAP_STOP"; wait "$SNAP_PID" || true; SNAP_PID=""
        rm -f "$SNAP_STOP"
    fi
    if [ -n "$SKZ_SAMPLER" ]; then
        # Before the sync below: the samples are of the window.
        kill "$SKZ_SAMPLER" 2>/dev/null || true; wait "$SKZ_SAMPLER" 2>/dev/null || true; SKZ_SAMPLER=""
    fi
    sync; sleep 2
    snapshot "$OUT/$tag.after"
    [ -z "$SKZ_UP" ] || log "$tag: skyzfsd after the window: $(grep '^skz ' "$OUT/$tag.after" | cut -d' ' -f2- | tr '\n' ';')"
    if [ -s "$OUT/$tag.snaps" ]; then
        # The next phase starts from a stack with no snapshots.
        taken=$(wc -l < "$OUT/$tag.snaps")
        for (( i = taken > SNAP_KEEP ? taken - SNAP_KEEP : 0; i < taken; i++ )); do drop_snap "$i"; done
        log "$tag: $taken snapshots, bracket ms (+ms before it): $(awk '{ printf "%s%s(+%s)", sep, $2, $4; sep = " " } $3 != 0 { bad++ } END { if (bad) printf "  (%d FAILED)", bad }' "$OUT/$tag.snaps")"
    fi
    if [ -n "$ZFS_WIN" ]; then
        zfs destroy -r "$ZFS_POOL@$ZFS_WIN"
        ZFS_WIN=""
    fi
    # The origin delay that was under this row: the knob's value where
    # the topology has an origin, 0 where it has none (the pool served
    # from its own image by a loop device, the reference server or the
    # daemon), whatever the knob says.
    case $topo in zfs-loop|zfs-ublk|zfs-skyzfsd) i=0 ;; *) i=$ORIGIN_DELAY_MS ;; esac
    python3 - "$OUT/$tag" "$topo" "$wl" "$DURATION" "$mode" "$SNAP_INTERVAL" "$plog" "$OUT/$topo.fsync" "$LABEL" "$CLIENTS" "$i" "$SKZ_TIME_DIR" > "$OUT/$tag.json" <<'PY'
import collections, glob, json, os, re, sys
sys.path.insert(0, sys.argv[12])
import skz_time
base, topo, wl, dur = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
snap_mode, snap_interval, plog = sys.argv[5], int(sys.argv[6]), sys.argv[7]
fsync_file, run_label = sys.argv[8], sys.argv[9]
zc, zg = {}, {}   # ZFS counters / gauges, keyed by file
skz = {}          # the daemon's status, keyed by file
cpu, thr, txgs, mem, loops = {}, {}, {}, {}, {}
def load(p):
    dev, pg, other = {}, {}, {}
    zc[p], zg[p] = {}, {}
    skz[p] = {}
    cpu[p], thr[p], txgs[p], mem[p], loops[p] = {}, {}, {}, {}, {}
    for line in open(p):
        f = line.split()
        if not f: continue
        if f[0] == 'dev': dev[f[1]] = [int(x) for x in f[2:7]]
        elif f[0] == 'pg': pg[f[1]] = int(f[2]) if len(f) > 2 and f[2].isdigit() else 0
        elif f[0] == 'cpu' and len(f) > 2: cpu[p][f[1]] = [int(x) for x in f[2:]]
        elif f[0] == 'thr' and len(f) > 2: thr[p][f[1]] = int(f[2])
        elif f[0] == 'mem' and len(f) > 2 and f[2].isdigit(): mem[p][f[1]] = int(f[2])
        elif f[0] == 'loop' and len(f) > 2: loops[p][f[1]] = {'dio': f[2] == '1', 'ro': len(f) > 3 and f[3] == '1'}
        elif f[0] == 'zfstxg' and len(f) >= 8: txgs[p][int(f[1])] = [int(x) for x in f[2:8]]
        elif f[0] == 'skz' and len(f) > 2: skz[p][f[1]] = f[2]
        elif f[0] in ('zfsc', 'zfsg'):
            if len(f) > 2 and f[2].isdigit(): (zc if f[0] == 'zfsc' else zg)[p][f[1]] = int(f[2])
        else: other.setdefault(f[0], []).append(line.strip())
    return dev, pg, other
bd, bp, bo = load(base + '.before'); ad, ap, ao = load(base + '.after')
MB = 1 << 20
r = {'topology': topo, 'workload': wl, 'duration_s': dur, 'devices': {}, 'pg': {}}
if run_label: r['label'] = f'{topo} [{run_label}]'
r['clients'] = int(sys.argv[10])
r['origin_delay_ms'] = int(sys.argv[11])
# The machine the row ran on: this boot of this runner. Two rows are
# compared only where it is the same.
# The host name beside the boot id: machines resumed from one memory
# snapshot could share a boot id.
try:
    r['machine'] = open('/proc/sys/kernel/random/boot_id').read().strip() + ' ' + os.uname().nodename
except OSError:
    pass
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
# zfs-skyzfsd: the daemon's own account of the window.
if skz[base + '.after']:
    sb, sa = skz[base + '.before'], skz[base + '.after']
    num = lambda d, k: int(d[k]) if d.get(k, '').isdigit() else None
    diff = lambda k: None if num(sa, k) is None or num(sb, k) is None else num(sa, k) - num(sb, k)
    k = {'before': sb, 'after': sa, 'steps': diff('checkpoint_steps'), 'syncs': diff('checkpoint_syncs'),
         'writes': diff('writes'), 'log_live_before': num(sb, 'log_live'), 'log_live_after': num(sa, 'log_live'),
         'pid_same': sb.get('pid') == sa.get('pid'), 'info': {}}
    # Where a request's time goes: the difference of every time_* key
    # over the window, the end value of the longest ones and their
    # causes, and the two readings as they were. A reading that failed
    # or a status without the keys is said in the row; the run goes on.
    k['time'] = skz_time.delta(sb, sa, num(sb, 'read_at_ns'), num(sa, 'read_at_ns'))
    samples = []
    if os.path.exists(base + '.skz'):
        for line in open(base + '.skz'):
            f = line.split()
            if len(f) > 1: samples.append(dict(x.split('=', 1) for x in f[1:] if '=' in x))
    live = [int(x['log_live']) for x in samples if x.get('log_live', '').isdigit()]
    k['window'] = {'samples': len(samples), 'step_under_way': sum(1 for x in samples if x.get('checkpoint_step') not in (None, 'none')),
                   'log_live_min': min(live) if live else None, 'log_live_max': max(live) if live else None,
                   'unstored_max': max([int(x['unstored']) for x in samples if x.get('unstored', '').isdigit()] or [0])}
    info = os.path.join(os.path.dirname(base), topo + '.skzinfo')
    if os.path.exists(info):
        for line in open(info):
            key, _, value = line.strip().partition(' ')
            if key: k['info'][key] = value
    k['serve_args'] = k['info'].get('serve_args', '')
    k['build'] = k['info'].get('build', 'not said')
    if k['info'].get('status_reader'):
        k['build'] += f" (status read by {k['info']['status_reader']})"
    said = ao.get('skzpool', [''])[0].split(None, 1)
    k['pool_status'] = said[1] if len(said) > 1 else 'not asked'
    r['skyzfsd'] = k
# Snapshots taken during the window: "epoch-s bracket-ms rc" per line.
r['snap'] = {'mode': snap_mode, 'interval_s': snap_interval, 'ms': [], 'pre_ms': [], 'failed': 0}
if os.path.exists(base + '.snaps'):
    for line in open(base + '.snaps'):
        f = line.split()
        if len(f) >= 3:
            r['snap']['ms'].append(int(f[1])); r['snap']['failed'] += f[2] != '0'
            if len(f) > 3: r['snap']['pre_ms'].append(int(f[3]))
# pgbench -l: client txn latency_us script epoch_s epoch_us. The tail,
# and the slowest whole second (a stall shows as a second with few or
# no completions; the first and last seconds are partial and skipped).
lats, per_s = [], collections.Counter()
for path in glob.glob(plog + '.*'):
    for line in open(path):
        f = line.split()
        if len(f) >= 5 and f[2].isdigit():
            lats.append(int(f[2])); per_s[int(f[4])] += 1
if lats:
    lats.sort()
    q = lambda p: lats[min(len(lats) - 1, int(p * len(lats)))] / 1000
    secs = range(min(per_s) + 1, max(per_s))
    r['latency'] = {'p50_ms': q(0.50), 'p99_ms': q(0.99), 'p999_ms': q(0.999), 'max_ms': lats[-1] / 1000,
                    'min_tps_1s': min((per_s.get(x, 0) for x in secs), default=None),
                    'stalled_s': sum(1 for x in secs if per_s.get(x, 0) == 0)}
if zc[base + '.after'] or zg[base + '.after']:
    zb, za = zc[base + '.before'], zc[base + '.after']
    r['zfs'] = {'delta': {k: za[k] - zb.get(k, 0) for k in za}, 'after': zg[base + '.after']}
    # Transaction groups completed inside the window.
    new = [v for k, v in txgs[base + '.after'].items() if k not in txgs[base + '.before']]
    if new:
        ms = lambda i: [v[i] / 1e6 for v in new]
        r['zfs']['txg'] = {'n': len(new), 'dirty_mb_mean': sum(v[0] for v in new) / len(new) / MB,
                           'written_mb': sum(v[1] for v in new) / MB,
                           'open_ms_mean': sum(ms(2)) / len(new), 'sync_ms_mean': sum(ms(5)) / len(new),
                           'sync_ms_max': max(ms(5)), 'wait_ms_max': max(ms(4))}
# Where the time went. CPU as shares of all CPUs over the window, CPU
# seconds by thread name, context switches.
HZ = 100
diag = {}
cb, ca = cpu[base + '.before'], cpu[base + '.after']
if 'total' in ca and 'total' in cb:
    d = [a - b for a, b in zip(ca['total'], cb['total'])]; tot = sum(d) or 1
    diag['cpu_pct'] = {'user': 100 * (d[0] + d[1]) / tot, 'system': 100 * (d[2] + d[5] + d[6]) / tot,
                       'idle': 100 * d[3] / tot, 'iowait': 100 * d[4] / tot, 'steal': 100 * d[7] / tot}
    diag['cpu_s_total'] = tot / HZ
if 'ctxt' in ca and 'ctxt' in cb: diag['ctxt'] = ca['ctxt'][0] - cb['ctxt'][0]
tb, ta = thr[base + '.before'], thr[base + '.after']
by = collections.Counter()
for k, v in ta.items():
    dv = v - tb.get(k, 0)
    if dv <= 0: continue
    name = k.replace('(reaped)', '')
    # pgbench and the helpers of this script all end up as some shell's children.
    if name in ('su', 'bash', 'sh', 'sudo', 'pgbench', 'python', 'runuser', 'awk', 'sed'): name = 'pgbench+harness'
    by[name] += dv / HZ
diag['cpu_s_by_thread'] = dict(by.most_common(20))
# Memory at the end of the window, MiB, and how the loops are attached.
if mem[base + '.after']: diag['mem_mb'] = {k: round(v / MB, 1) for k, v in mem[base + '.after'].items()}
# The cgroup counts the times it reached its limit since it was made,
# which is before the copy and the warm-up: the window's count is the
# difference of the two snapshots. (The counts travel as n MiB.)
for ev in ('skz.cgroup.events.max', 'skz.cgroup.events.high', 'skz.cgroup.events.oom', 'skz.cgroup.events.oom_kill'):
    if ev in mem[base + '.after']:
        m_ = diag['mem_mb']
        m_[ev + '.since_start'] = m_.pop(ev)
        if ev in mem[base + '.before']:
            m_[ev + '.window'] = round((mem[base + '.after'][ev] - mem[base + '.before'][ev]) / MB, 1)
if loops[base + '.after']: diag['loops'] = loops[base + '.after']
r['diag'] = diag
# pg_test_fsync, first table: one 8 kB write per sync, by wal_sync_method.
if os.path.exists(fsync_file):
    on, ft = False, {}
    for line in open(fsync_file):
        if 'one 8kB write' in line: on = True; continue
        if line.startswith('Compare'): on = False
        m = re.match(r'\s+(\S+)\s+([\d.]+) ops/sec\s+(\d+) usecs/op', line)
        if on and m: ft[m.group(1)] = int(m.group(3))
    if ft: r['fsync_test_us'] = ft
print(json.dumps(r, indent=1))
PY
    rm -f "$plog".*
    log "$tag: tps=$(python3 -c "import json;print(json.load(open('$OUT/$tag.json'))['tps'])")"
    log "$tag: latency $(python3 -c "import json;l=json.load(open('$OUT/$tag.json')).get('latency',{});print(' '.join(f'{k}={v}' for k,v in l.items()))")"
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
    # Per-run settings; later lines win over what the origin carries.
    local kv
    {
        echo "track_wal_io_timing = on"
        for kv in ${PG_CONF//,/ }; do echo "${kv%%=*} = ${kv#*=}"; done
    } >> "$PGDATA/postgresql.conf"
    [ -z "$PG_CONF" ] || log "postgresql.conf for this run: $PG_CONF"
    rm -f "$PGDATA/postmaster.pid"
    # Start cold, and with nothing in the page cache that this script
    # put there. A page is charged to the cgroup that first touched it:
    # the pg_wal files moved above were written by this script, so
    # without this about 1 GiB of WAL page cache stays outside
    # TB_MEM_LIMIT for the whole run, in the two-volume rows only.
    sync
    echo 3 > /proc/sys/vm/drop_caches
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
    fsync_test
    # One phase per snapshot mode, on this one stack. The ZFS stacks
    # have a single mechanism, so every mode but none collapses to it.
    PHASES=""
    # shellcheck disable=SC2086
    for m in $SNAP_MODES; do
        case $TOPO in zfs*) [ "$m" = none ] || m=zfs ;; esac
        case " $PHASES " in *" $m "*) ;; *) PHASES="$PHASES $m" ;; esac
    done
    # shellcheck disable=SC2086
    for wl in $WORKLOADS; do
        for m in $PHASES; do run_workload "$TOPO" "$wl" "$m"; done
    done
    teardown_topology
done

summarize "$OUT" > "$OUT/summary.md"
cat "$OUT/summary.md"
echo "topology-bench OK — per-run JSON, snapshots and summary.md under $OUT"

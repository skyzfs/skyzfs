#!/usr/bin/env bash
# dm-linear discard-port PoC (D-M15): verify the kernel claim contract
# and measure live-reclaim cost — the §6.1 pacing-default numbers.
#
# Shape under test (no skynbd code involved — pure device-mapper):
#
#   fs → poc-fs (linear shim; the fs claim lands here)
#           └────────────► poc-origin (thin, external origin) → poc-pool
#   poc-rec (linear discard port, unclaimed) ──┘
#
# Checks: BLKDISCARD refused on the claimed origin and shim, accepted on
# the port; port discards unmap origin blocks; content byte-identical
# across a reclaim whose backing was pre-seeded (Invariant B); fs intact.
# Measures: per-64KiB-discard latency (in-process ioctls), one-big-range
# discard, and full freeze→batch-discard→thaw windows per batch size.
#
# Part 2 measures the alternative: a two-pool ping-pong swing. Snapshot
# the head thin (the publish point S), advance the backing, write a
# fresh dirty set, then: build pool B + an empty thin over the advanced
# backing, enumerate dirty-since-S from a thin metadata dump (the same
# same-data-block walk shared_mappings does), live-copy it behind a
# convergence-barrier snapshot, frozen top-up copy (diffed against the
# barrier, not head-vs-head — see the barrier comment), retarget the
# shim (load+suspend+resume), and tear pool A down wholesale. No
# discards anywhere; every phase timed.
set -euo pipefail

# GNU dd as "$DD" (`gnudd` on ubuntu-26.04, plain `dd` before it): the
# O_DIRECT calls below fail with the Rust coreutils dd. See the helper.
# shellcheck source=tests/lib/gnu-dd.sh
. "$(dirname "${BASH_SOURCE[0]}")/../lib/gnu-dd.sh"
[ "$(id -u)" = 0 ] || { echo "needs root"; exit 64; }
for t in dmsetup losetup blkdiscard fsfreeze mkfs.ext4 python3 blockdev thin_dump; do
  command -v "$t" >/dev/null || { echo "missing tool: $t"; exit 64; }
done

SIZE_MB=${SIZE_MB:-1024}     # thin device size
MEASURE_MB=${MEASURE_MB:-256} # region reclaimed during measurement
DIRTY_MB=${DIRTY_MB:-128}    # dirty-since-S set carried by the ping-pong swing
CHUNK=$((64 * 1024))         # pool block = 64 KiB (128 sectors)
WORK=$(mktemp -d)
BACK=""; DATA=""; META=""; DATA2=""; META2=""; MOUNTED=""

cleanup() {
  set +e
  [ -n "$MOUNTED" ] && { fsfreeze -u "$WORK/mnt" 2>/dev/null; umount "$WORK/mnt"; }
  dmsetup remove poc-rec poc-fs poc-origin poc2-origin 2>/dev/null
  dmsetup remove poc-pool poc2-pool 2>/dev/null
  [ -n "$BACK" ] && losetup -d "$BACK" "$DATA" "$META" 2>/dev/null
  [ -n "$DATA2" ] && losetup -d "$DATA2" "$META2" 2>/dev/null
  rm -rf "$WORK"
}
trap cleanup EXIT

say() { echo "== $*"; }
summary() { echo "$*"; [ -z "${GITHUB_STEP_SUMMARY:-}" ] || echo "$*" >> "$GITHUB_STEP_SUMMARY" || true; }

# ---- stack ---------------------------------------------------------------
say "building stack (${SIZE_MB} MiB thin, ${CHUNK} B pool blocks)"
truncate -s "${SIZE_MB}M" "$WORK/back.img" "$WORK/data.img"
truncate -s 64M "$WORK/tmeta.img"
BACK=$(losetup -f --show "$WORK/back.img")
DATA=$(losetup -f --show "$WORK/data.img")
META=$(losetup -f --show "$WORK/tmeta.img")
"$DD" if=/dev/zero of="$META" bs=4096 count=1 conv=notrunc 2>/dev/null
SECT=$(blockdev --getsz "$BACK")
dmsetup create poc-pool --table \
  "0 $(blockdev --getsz "$DATA") thin-pool ${META} ${DATA} 128 32 2 skip_block_zeroing no_discard_passdown"
dmsetup message poc-pool 0 "create_thin 1"
dmsetup create poc-origin --table "0 ${SECT} thin /dev/mapper/poc-pool 1 ${BACK}"
dmsetup create poc-fs  --table "0 ${SECT} linear /dev/mapper/poc-origin 0"
dmsetup create poc-rec --table "0 ${SECT} linear /dev/mapper/poc-origin 0"
mkdir -p "$WORK/mnt"
mkfs.ext4 -q -E nodiscard,lazy_itable_init=0,lazy_journal_init=0 /dev/mapper/poc-fs
mount /dev/mapper/poc-fs "$WORK/mnt"; MOUNTED=1

pool_used() { dmsetup status poc-pool | awk '{print $6}' | cut -d/ -f1; }

# ---- the claim contract ---------------------------------------------------
say "claim contract"
# Tool cell: util-linux blkdiscard opens O_EXCL (since 2.36), so it fails
# on any claimed device at open — this pins the tool, not the kernel.
for dev in poc-origin poc-fs; do
  if blkdiscard -o 0 -l $CHUNK "/dev/mapper/$dev" 2>/dev/null; then
    echo "FAIL: blkdiscard(1) on claimed $dev succeeded"; exit 1
  fi
done
summary "- blkdiscard(1) (O_EXCL open) refused on claimed origin and shim ✓"
# Kernel cell — production's exact path (dm_linux.rs discard_dev): plain
# O_RDWR open + BLKDISCARD ioctl, no O_EXCL. The two claim kinds differ
# (measured): a merely dm-claimed node (the origin under the shim) may
# accept the raw ioctl, while an fs-claimed node (the mounted shim) is
# refused — the fs-claimed refusal is the load-bearing cell (it is what
# broke the shimless smoke) and is asserted; the dm-claimed outcome is
# kernel-dependent and reported. Chunk 0 is saved via the port first so
# an allowed probe cannot corrupt the fs.
"$DD" if=/dev/mapper/poc-rec of="$WORK/chunk0" bs=$CHUNK count=1 2>/dev/null
for dev in poc-origin poc-fs; do
  out=$(python3 - "$dev" "$CHUNK" <<'RAW'
import fcntl, os, struct, sys
dev, chunk = sys.argv[1], int(sys.argv[2])
try:
    fd = os.open(f"/dev/mapper/{dev}", os.O_RDWR)
    fcntl.ioctl(fd, 0x1277, struct.pack("QQ", 0, chunk))
    print("ALLOWED")
except OSError as e:
    print(f"refused ({e.strerror})")
RAW
)
  if [ "$out" = "ALLOWED" ]; then
    "$DD" if="$WORK/chunk0" of=/dev/mapper/poc-rec bs=$CHUNK count=1 conv=notrunc 2>/dev/null
    sync
  fi
  if [ "$dev" = poc-fs ] && [ "$out" = "ALLOWED" ]; then
    echo "FAIL: raw BLKDISCARD ioctl on the fs-claimed shim was allowed"; exit 1
  fi
  summary "- raw BLKDISCARD ioctl (production path) on claimed $dev: $out"
done

# ---- correctness: content-equal reclaim ------------------------------
say "correctness (Invariant B shape)"
# Provision a region through the fs, then reclaim it after seeding the
# backing with identical content — reads must not move a byte.
"$DD" if=/dev/urandom of="$WORK/mnt/blob" bs=1M count=64 conv=fsync 2>/dev/null
sync
USED0=$(pool_used)
# Everything happens inside one freeze window so no fs write can slip in
# between seeding the backing and the discard (that write would be
# silently reverted — the lost-write race the production path recomputes
# shared_mappings per window to avoid).
fsfreeze -f "$WORK/mnt"
# Seed the backing with the full current device content: read via the
# (unclaimed) port, write into the backing loop.
"$DD" if=/dev/mapper/poc-rec of="$BACK" bs=1M count=$((SIZE_MB)) oflag=direct 2>/dev/null
# iflag=direct: the AFTER read must exercise the post-discard read path,
# not the port node's page cache.
BEFORE=$("$DD" if=/dev/mapper/poc-rec bs=1M count="${SIZE_MB}" iflag=direct 2>/dev/null | md5sum | cut -d' ' -f1)
blkdiscard -o 0 -l $((SIZE_MB * 1024 * 1024)) /dev/mapper/poc-rec
AFTER=$("$DD" if=/dev/mapper/poc-rec bs=1M count="${SIZE_MB}" iflag=direct 2>/dev/null | md5sum | cut -d' ' -f1)
fsfreeze -u "$WORK/mnt"
USED1=$(pool_used)
[ "$BEFORE" = "$AFTER" ] || { echo "FAIL: content changed across reclaim"; exit 1; }
[ "$USED1" -lt "$USED0" ] || { echo "FAIL: pool did not shrink ($USED0 -> $USED1)"; exit 1; }
md5sum "$WORK/mnt/blob" >/dev/null # fs still serves the file (read-through)
summary "- full-device port reclaim: content identical, pool ${USED0} → ${USED1} blocks ✓"

# ---- measurement ----------------------------------------------------------
say "measurement (${MEASURE_MB} MiB reclaim region)"
"$DD" if=/dev/urandom of="$WORK/mnt/blob2" bs=1M count="${MEASURE_MB}" conv=fsync 2>/dev/null
sync
# Re-seed under freeze; the fs stays idle (synced, no reads → no atime
# writes) between the measurement windows below, so the seed stays valid.
fsfreeze -f "$WORK/mnt"
"$DD" if=/dev/mapper/poc-rec of="$BACK" bs=1M count="${SIZE_MB}" oflag=direct 2>/dev/null
fsfreeze -u "$WORK/mnt"

python3 - "$WORK/mnt" "$MEASURE_MB" "$CHUNK" <<'PY' | tee "$WORK/measure.txt"
import fcntl, os, struct, subprocess, sys, time
mnt, measure_mb, chunk = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
BLKDISCARD = 0x1277
fd = os.open("/dev/mapper/poc-rec", os.O_RDWR)
size = measure_mb * 1024 * 1024
ranges = [(off, chunk) for off in range(0, size, chunk)]

def freeze(on):
    subprocess.run(["fsfreeze", "-f" if on else "-u", mnt], check=True)

def discard(off, ln):
    fcntl.ioctl(fd, BLKDISCARD, struct.pack("QQ", off, ln))

# Per-op latency: individual 64 KiB discards inside one freeze window.
n = min(2048, len(ranges))
freeze(True)
t0 = time.monotonic_ns()
for off, ln in ranges[:n]:
    discard(off, ln)
t1 = time.monotonic_ns()
freeze(False)
per_op_us = (t1 - t0) / n / 1000
print(f"per-64KiB-discard: {per_op_us:.1f} us  ({n} ops, window {(t1-t0)/1e6:.1f} ms)")

# Freeze→batch→thaw window per production batch size, remaining region.
rest = ranges[n:]
for batch in (64, 256, 1024):
    if not rest:
        break
    take, rest = rest[:batch], rest[batch:]
    w0 = time.monotonic_ns()
    freeze(True)
    d0 = time.monotonic_ns()
    for off, ln in take:
        discard(off, ln)
    d1 = time.monotonic_ns()
    freeze(False)
    w1 = time.monotonic_ns()
    print(f"batch {batch:5}: discards {(d1-d0)/1e6:8.2f} ms, full freeze window {(w1-w0)/1e6:8.2f} ms")

# One big-range discard for whatever is left.
if rest:
    lo = rest[0][0]
    hi = rest[-1][0] + rest[-1][1]
    freeze(True)
    b0 = time.monotonic_ns()
    discard(lo, hi - lo)
    b1 = time.monotonic_ns()
    freeze(False)
    print(f"single {(hi-lo)>>20} MiB discard: {(b1-b0)/1e6:.2f} ms")
os.close(fd)
PY
while IFS= read -r line; do summary "- $line"; done < "$WORK/measure.txt"

# ---- two-pool ping-pong swing ----------------------------------------------
say "two-pool ping-pong swing (${DIRTY_MB} MiB dirty set)"
# "Publish": snapshot the head thin (dev 2 = S) and seed the backing with
# the frozen device content — the backing now plays the advanced origin.
fsfreeze -f "$WORK/mnt"
dmsetup suspend poc-origin
dmsetup message poc-pool 0 "create_snap 2 1"
dmsetup resume poc-origin
"$DD" if=/dev/mapper/poc-rec of="$BACK" bs=1M count="${SIZE_MB}" oflag=direct 2>/dev/null
fsfreeze -u "$WORK/mnt"
# Dirty-since-S: the writes the swing must carry over.
"$DD" if=/dev/urandom of="$WORK/mnt/blob3" bs=1M count="${DIRTY_MB}" conv=fsync 2>/dev/null
sync

# Convergence barrier: snapshot 3 fences the live copy pass. The frozen
# top-up diffs head vs THIS snapshot — every write after it either COWs
# (mapping changes) or hits a block already COW'd after it (mapping
# already differs), so the diff provably covers all later writes. A
# head-vs-head diff cannot: an in-place rewrite of an already-unshared
# block (journal, inode table) changes no mapping and goes unseen.
t0=$(date +%s%N)
fsfreeze -f "$WORK/mnt"
dmsetup suspend poc-origin
dmsetup message poc-pool 0 "create_snap 3 1"
dmsetup resume poc-origin
fsfreeze -u "$WORK/mnt"
t1=$(date +%s%N)
summary "- pingpong: convergence-barrier snapshot in $(( (t1 - t0) / 1000000 )) ms"

# Pool B with an empty thin over the same (advanced) backing.
t0=$(date +%s%N)
truncate -s "${SIZE_MB}M" "$WORK/data2.img"
truncate -s 64M "$WORK/tmeta2.img"
DATA2=$(losetup -f --show "$WORK/data2.img")
META2=$(losetup -f --show "$WORK/tmeta2.img")
"$DD" if=/dev/zero of="$META2" bs=4096 count=1 conv=notrunc 2>/dev/null
dmsetup create poc2-pool --table \
  "0 $(blockdev --getsz "$DATA2") thin-pool ${META2} ${DATA2} 128 32 2 skip_block_zeroing no_discard_passdown"
dmsetup message poc2-pool 0 "create_thin 1"
dmsetup create poc2-origin --table "0 ${SECT} thin /dev/mapper/poc2-pool 1 ${BACK}"
t1=$(date +%s%N)
summary "- pingpong: pool B + empty thin built in $(( (t1 - t0) / 1000000 )) ms"

python3 - "$META" "$WORK" "$CHUNK" "$SECT" <<'PY' | tee "$WORK/swing.txt"
import os, subprocess, sys, time
import xml.etree.ElementTree as ET
meta, work, chunk, sect = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4]
mnt = work + "/mnt"

def run(*cmd):
    subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)

# Mappings per thin dev from a pool-A metadata snapshot — what the agent
# reads via DmOps in production.
def dump_maps():
    run("dmsetup", "message", "poc-pool", "0", "reserve_metadata_snap")
    try:
        xml = subprocess.run(["thin_dump", "-m", meta], check=True,
                             capture_output=True, text=True).stdout
    finally:
        run("dmsetup", "message", "poc-pool", "0", "release_metadata_snap")
    # thin-provisioning-tools 1.x (ubuntu-26.04) writes a mapping subtree
    # shared between devices ONCE, as a top-level <def name=...>, and puts
    # <ref name=.../> where each device (or another def) uses it. 0.9.x
    # (ubuntu-24.04) writes every mapping inline. Reading only a device's
    # inline mappings made a fresh snapshot, which shares nearly all of
    # its tree with the head, look almost empty: the dirty set came out
    # at 4 blocks instead of ~2050, and the swing copied too little.
    # Expanding refs reads both formats the same.
    root = ET.fromstring(xml)
    defs = {e.attrib["name"]: e for e in root if e.tag == "def"}
    def collect(elem, m):
        for e in elem:
            a = e.attrib
            if e.tag == "single_mapping":
                m[int(a["origin_block"])] = int(a["data_block"])
            elif e.tag == "range_mapping":
                ob, db = int(a["origin_begin"]), int(a["data_begin"])
                for i in range(int(a["length"])):
                    m[ob + i] = db + i
            elif e.tag == "ref":
                collect(defs[a["name"]], m)
    maps = {}
    for dev in root:
        if dev.tag != "device":
            continue
        m = {}
        collect(dev, m)
        maps[int(dev.attrib["dev_id"])] = m
    return maps

def runs_of(blocks):
    out = []
    for b in sorted(blocks):
        if out and b == out[-1][1]:
            out[-1][1] = b + 1
        else:
            out.append([b, b + 1])
    return out

def copy(blocks, tag):
    runs = runs_of(blocks)
    # The port's page cache may hold pages from earlier passes; the fs
    # wrote through a different device node since.
    run("blockdev", "--flushbufs", "/dev/mapper/poc-rec")
    src = os.open("/dev/mapper/poc-rec", os.O_RDONLY)
    dst = os.open("/dev/mapper/poc2-origin", os.O_RDWR)
    nbytes = 0
    t0 = time.monotonic_ns()
    for b0, b1 in runs:
        os.lseek(src, b0 * chunk, 0)
        os.lseek(dst, b0 * chunk, 0)
        for _ in range(b1 - b0):
            buf = os.read(src, chunk)
            os.write(dst, buf)
            nbytes += len(buf)
    os.fsync(dst)
    t1 = time.monotonic_ns()
    os.close(src)
    os.close(dst)
    ms = (t1 - t0) / 1e6
    mib = nbytes / (1 << 20)
    rate = mib / (ms / 1000) if ms else 0.0
    print(f"{tag}: {mib:.0f} MiB in {len(runs)} runs, {ms:.1f} ms ({rate:.0f} MiB/s)")

t0 = time.monotonic_ns()
maps = dump_maps()
snap_s, barrier = maps[2], maps[3]
dirty = {b for b, d in barrier.items() if snap_s.get(b) != d}
t1 = time.monotonic_ns()
print(f"delta enumeration (metadata snap + dump + diff): {(t1-t0)/1e6:.1f} ms, {len(dirty)} blocks")
copy(dirty, "live copy pass")

# A write landing while the live pass ran — the frozen top-up's work.
with open(mnt + "/blob4", "wb") as f:
    f.write(os.urandom(8 << 20))
    f.flush()
    os.fsync(f.fileno())
subprocess.run(["sync"], check=True)

# Hash every file through the OLD stack; bash re-checks them cold through
# the new one after the switch.
with open(work + "/sums.txt", "w") as f:
    subprocess.run(["md5sum", "blob", "blob2", "blob3", "blob4"],
                   cwd=mnt, check=True, stdout=f)

run("fsfreeze", "-f", mnt)
t0 = time.monotonic_ns()
head = dump_maps()[1]
dirty2 = {b for b, d in head.items() if barrier.get(b) != d}
t1 = time.monotonic_ns()
print(f"frozen top-up delta: {(t1-t0)/1e6:.1f} ms, {len(dirty2)} blocks")
copy(dirty2, "frozen top-up copy")
t2 = time.monotonic_ns()
run("dmsetup", "load", "poc-fs", "--table", f"0 {sect} linear /dev/mapper/poc2-origin 0")
run("dmsetup", "suspend", "poc-fs")
run("dmsetup", "resume", "poc-fs")
t3 = time.monotonic_ns()
run("fsfreeze", "-u", mnt)
t4 = time.monotonic_ns()
print(f"shim retarget (load+suspend+resume): {(t3-t2)/1e6:.2f} ms")
print(f"full frozen switch window: {(t4-t0)/1e6:.1f} ms")
PY
while IFS= read -r line; do summary "- pingpong: $line"; done < "$WORK/swing.txt"

# Post-switch verification: cold-cache file reads through the new stack.
echo 3 > /proc/sys/vm/drop_caches
(cd "$WORK/mnt" && md5sum -c "$WORK/sums.txt" >/dev/null) \
  || { echo "FAIL: file content changed across the swing"; exit 1; }
USED_B=$(dmsetup status poc2-pool | awk '{print $6}' | cut -d/ -f1)
summary "- pingpong: content identical cold; pool B ${USED_B} blocks vs pool A $(pool_used) ✓"

# Wholesale pool-A reclaim: remove and hole-punch, no per-block discards.
t0=$(date +%s%N)
dmsetup remove poc-rec poc-origin
dmsetup remove poc-pool
blkdiscard "$DATA" 2>/dev/null || fallocate --dig-holes "$WORK/data.img"
t1=$(date +%s%N)
summary "- pingpong: pool A wholesale teardown (remove + hole-punch) in $(( (t1 - t0) / 1000000 )) ms"

# ---- final integrity -------------------------------------------------------
md5sum "$WORK/mnt/"blob* >/dev/null
fsfreeze -u "$WORK/mnt" 2>/dev/null || true
umount "$WORK/mnt"; MOUNTED=""
fsck.ext4 -f -n /dev/mapper/poc-fs >/dev/null || { echo "FAIL: fsck after reclaims"; exit 1; }
summary "- fsck clean after all reclaims and the swing ✓"
say "PASS"

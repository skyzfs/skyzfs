#!/usr/bin/env bash
# skynbd benchmark — RECORD-ONLY, like S10: it reports numbers and never
# judges them (a benchmark that gates CI is a flake generator; trends
# belong to whoever reads the artifacts).
#
# Two data paths, deliberately the two a workstation actually feels:
#
#   head  fio against the raw kernel-NBD head attach (/dev/nbdX) — the
#         server data path S10 baselines, kept here so both numbers
#         come from the same run on the same runner.
#   thin  fio against a daemon-managed `mount --thin` filesystem — the
#         full CSI-parity publish-only stack (§5 local pool, §7.1 read
#         cache over the shim), staged by the resident agent exactly as
#         PR #91 ships it. Includes a cold-vs-warm randread pair (the
#         second pass is what the rcache exists for) and the duration
#         of the unmount's D-M13 final publish cycle.
#
# Usage (manual):  sudo tests/bench/bench.sh
#   env: E2E_SKIP_BUILD=1  skip cargo build (CI builds unprivileged first)
#        BENCH_OUT=dir     where fio JSON + summary.md land
#                          (default /tmp/skynbd-bench)
#
# Needs root (kernel NBD + device-mapper), fio, nbd-client, dmsetup,
# losetup, python3. The store is memory-backed on purpose: this measures
# the node data path and the dm stack, not the object store.
set -euo pipefail
cd "$(dirname "$0")/../.."

need() { for t in "$@"; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 64; }; done; }
need fio nbd-client dmsetup losetup python3

[ "$(id -u)" = 0 ] || { echo "needs root (kernel NBD + device-mapper)" >&2; exit 64; }
modprobe nbd max_part=8 2>/dev/null || true
[ -e /dev/nbd0 ] || { echo "no /dev/nbd0 (nbd module missing?)" >&2; exit 64; }

OUT=${BENCH_OUT:-/tmp/skynbd-bench}
mkdir -p "$OUT"
[ -n "${E2E_SKIP_BUILD:-}" ] || cargo build -p skynbdd -p skynbd-agent-cli

WORK=$(mktemp -d)
PORT=$((21000 + RANDOM % 20000))
API_PORT=$((PORT + 1))
API="http://127.0.0.1:${API_PORT}"
STATE_DIR="$WORK/agent"
MNT="$WORK/mnt"
mkdir -p "$STATE_DIR" "$MNT"
DAEMON_PID=""
AGENT_PID=""

cleanup() {
  rc=$?
  set +e
  [ -n "$AGENT_PID" ] && kill -9 "$AGENT_PID" 2>/dev/null
  umount "$MNT" 2>/dev/null
  for d in rcache rcdata rcmeta fs rec origin pool data tmeta; do
    dmsetup remove "sky-wsv-$d" 2>/dev/null
  done
  for f in "$STATE_DIR/cache/wsv.cache" "$STATE_DIR/rcache/wsv.rcache"; do
    losetup -j "$f" 2>/dev/null | cut -d: -f1 | while read -r l; do
      losetup -d "$l" 2>/dev/null
    done
  done
  for i in 0 1 2 3; do
    nbd-client -c "/dev/nbd$i" >/dev/null 2>&1 && nbd-client -d "/dev/nbd$i" >/dev/null 2>&1
  done
  [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null
  if [ "$rc" -ne 0 ]; then
    echo "bench FAILED (rc=$rc): logs under $WORK are about to be removed —"
    tail -30 "$WORK/skynbdd.log" 2>/dev/null
    tail -30 "$WORK/agent.log" 2>/dev/null
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# ---- server: one head volume for the raw path, one for the thin mount
cat > "$WORK/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${WORK}/refs.redb"

[store]
kind = "memory"

[[volumes]]
name = "bench"
size = 268435456

[[volumes]]
name = "wsv"
size = 1073741824
EOF
./target/debug/skynbdd "$WORK/skynbd.toml" > "$WORK/skynbdd.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 50); do
  curl -sf "$API/healthz" >/dev/null 2>&1 && break
  kill -0 "$DAEMON_PID" 2>/dev/null || { echo "skynbdd died:"; cat "$WORK/skynbdd.log"; exit 1; }
  sleep 0.2
done
curl -sf "$API/healthz" >/dev/null || { echo "skynbdd never became ready"; exit 1; }

# ---- bench A: raw kernel-NBD head ----------------------------------
echo "== bench A: raw NBD head (/dev/nbd0) =="
if ! nbd-client -N bench/head 127.0.0.1 "$PORT" /dev/nbd0 -b 4096 >/dev/null; then
  sleep 1
  nbd-client -N bench/head 127.0.0.1 "$PORT" /dev/nbd0 -b 4096 >/dev/null
fi
fio --stonewall --direct=1 --output-format=json --output="$OUT/head.json" \
    --name=head-seqwrite --filename=/dev/nbd0 --rw=write --bs=1M --size=64M \
    --name=head-randwrite --filename=/dev/nbd0 --rw=randwrite --bs=4k --size=16M \
    --name=head-randread --filename=/dev/nbd0 --rw=randread --bs=4k --size=16M \
    >/dev/null
nbd-client -d /dev/nbd0 >/dev/null

# ---- bench B: daemon-managed thin mount (publish-only + read cache) -
echo "== bench B: mount --thin via the resident agent =="
cat > "$WORK/agent.toml" <<EOF
server = "${API}"
state_dir = "${STATE_DIR}"

[mounts]
nbd_host = "127.0.0.1"
nbd_port = ${PORT}
cache_bytes = 536870912
read_cache_dir = "${STATE_DIR}/rcache"
read_cache_bytes = 134217728
chunk_bytes = 65536
interval_s = 3600
freeze_budget_ms = 30000
EOF
AGENT=./target/debug/skynbd-agent
"$AGENT" run "$WORK/agent.toml" > "$WORK/agent.log" 2>&1 &
AGENT_PID=$!
for _ in $(seq 1 50); do
  [ -S "$STATE_DIR/ctl.sock" ] && break
  kill -0 "$AGENT_PID" 2>/dev/null || { echo "agent died:"; tail -30 "$WORK/agent.log"; exit 1; }
  sleep 0.2
done
"$AGENT" mount wsv "$MNT" --thin --state-dir "$STATE_DIR"

# One file, four phases with stonewalls: sequential write, random
# write, then the cold/warm randread pair — the warm pass re-reads the
# same blocks so the second number is the §7.1 rcache serving them.
fio --directory="$MNT" --stonewall --direct=1 \
    --output-format=json --output="$OUT/thin.json" \
    --name=thin-seqwrite --rw=write --bs=1M --size=64M --filename=benchfile \
    --name=thin-randwrite --rw=randwrite --bs=4k --size=16M --filename=benchfile \
    --name=thin-randread-cold --rw=randread --bs=4k --size=16M --filename=benchfile \
    --name=thin-randread-warm --rw=randread --bs=4k --size=16M --filename=benchfile \
    >/dev/null

# The unmount runs the D-M13 final publish cycle — its wall time is the
# "how long does closing the laptop lid actually take" number.
t0=$(date +%s.%N)
"$AGENT" unmount "$MNT" --state-dir "$STATE_DIR"
t1=$(date +%s.%N)
UNMOUNT_S=$(python3 -c "print(f'{$t1 - $t0:.1f}')")

# ---- summary --------------------------------------------------------
# Provenance rides with the numbers: shared-runner IO variance is the
# main thing that makes cross-run comparison misleading, and the disk
# class (rota=0 SSD-class vs 1 rotational) plus kernel/cpu is the
# one-line context that keeps a diff honest.
{
  echo "kernel: $(uname -r), cpus: $(nproc)"
  lsblk -dno NAME,SIZE,ROTA,MODEL 2>/dev/null | sed 's/^/disk: /'
} > "$OUT/provenance.txt"
python3 - "$OUT/head.json" "$OUT/thin.json" "$UNMOUNT_S" > "$OUT/summary.md" <<'PY'
import json, sys

rows = []
for path in sys.argv[1:3]:
    for job in json.load(open(path))["jobs"]:
        rw = job["read"] if "read" in job["jobname"] else job["write"]
        lat = rw.get("clat_ns", {}).get("mean", 0) / 1000
        rows.append((job["jobname"], rw["iops"], rw["bw"] / 1024, lat))

print("# skynbd bench (record-only)\n")
print("| job | iops | MiB/s | mean lat (us) |")
print("|---|---|---|---|")
for name, iops, bw, lat in rows:
    print(f"| {name} | {iops:.0f} | {bw:.1f} | {lat:.0f} |")
print(f"\nunmount (D-M13 final publish cycle): {sys.argv[3]} s")
PY
{
  echo
  echo '```'
  cat "$OUT/provenance.txt"
  echo '```'
} >> "$OUT/summary.md"
cat "$OUT/summary.md"
echo
echo "bench OK — raw fio JSON + summary.md under $OUT"

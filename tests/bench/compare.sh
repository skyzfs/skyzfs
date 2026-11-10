#!/usr/bin/env bash
# Paired A/B benchmark — RECORD-ONLY, like bench.sh: it reports numbers
# and never judges them. Compares prebuilt skynbdd/skynbd-agent binaries
# (e.g. main vs a PR stack head) on ONE runner, interleaved, so runner
# variance lands on every variant alike instead of between them.
#
# Unlike bench.sh (memory store, QD1), the store here is a real
# S3-compatible endpoint (MinIO) behind an injected round trip, and the
# workloads include queue depth > 1: issue #97's changes are all about
# concurrency against store latency, which a zero-latency memory store
# and a QD1 client cannot see.
#
#   VARIANTS="main:/bins/main:default head:/bins/head:default head-tuned:/bins/head:tuned" \
#   S3_ENDPOINT=http://172.17.0.2:9000 NETEM_DEV=docker0 \
#   sudo -E tests/bench/compare.sh
#
# env:
#   VARIANTS     space-separated label:bindir:profile. bindir holds
#                skynbdd + skynbd-agent; profile is `default` (no new
#                knobs: valid for any revision) or `tuned` (the #97
#                knobs: [nbd] max_in_flight, [mounts]
#                publish_pipeline_depth — head revisions only, both
#                configs are deny_unknown_fields).
#   S3_ENDPOINT  MinIO endpoint; reach it via an interface netem can
#                delay (the container IP over docker0, not a host port
#                mapping on lo, which would also delay NBD and the API).
#   NETEM_DEV    interface carrying ONLY store traffic (default docker0).
#   RTTS         latency points (default "0 10"), each `S` or `S+A`: S ms
#                added to the server<->store round trip (netem on
#                NETEM_DEV, one direction by the full value), A ms to the
#                agent<->API round trip (netem on lo, filtered to the
#                daemon's API port, half each way; A absent = 0). A
#                delays only the agent's HTTP calls: not NBD, not the
#                agent CLI (a Unix socket).
#   REPS         repetitions of the whole matrix (default 2).
#   IN_FLIGHT    tuned [nbd] max_in_flight (default 32).
#   PIPELINE     tuned publish_pipeline_depth (default 16).
#   SECS         runtime of each time-based fio job (default 15).
#   STORE_CONTAINER  docker container running the store (e.g. minio):
#                when set, each run's bucket size is recorded (store MiB
#                written: write amplification) and the bucket's data is
#                deleted after the run, so runs can't fill the disk under
#                each other. STORE_DATA_DIR is its data dir (default /data).
#   BENCH_OUT    output dir (default /tmp/skynbd-compare): raw fio JSON
#                under raw/, results.json (every sample), summary.md.
#
# Per run (fresh daemon, fresh bucket): raw-head writes (seq 1M QD1,
# rand 4k QD1/QD32, each ending in fsync so the store PUTs are inside
# the number), then three COLD read passes, each after a daemon restart
# (refs in redb, data in the bucket, nothing in memory): seq 1M QD1,
# rand 4k QD1, rand 4k QD32. Then the thin-mount publish: write 64 MiB
# of unique data through `mount --thin` and time the unmount, whose
# D-M13 final cycle backfills all of it (interval_s is 1 h, so nothing
# publishes earlier).
#
# What the injected RTTs do NOT cover: NBD client<->server stays on
# undelayed loopback (the raw-head and thin-mount data paths).
set -Eeuo pipefail
cd "$(dirname "$0")/../.."

need() { for t in "$@"; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 64; }; done; }
need fio nbd-client dmsetup losetup python3 curl tc

[ "$(id -u)" = 0 ] || { echo "needs root (kernel NBD, device-mapper, tc)" >&2; exit 64; }
modprobe nbd max_part=8 2>/dev/null || true
[ -e /dev/nbd0 ] || { echo "no /dev/nbd0 (nbd module missing?)" >&2; exit 64; }

VARIANTS=${VARIANTS:?VARIANTS=label:bindir:profile ...}
S3_ENDPOINT=${S3_ENDPOINT:?S3_ENDPOINT}
NETEM_DEV=${NETEM_DEV:-docker0}
RTTS=${RTTS:-0 10}
REPS=${REPS:-2}
IN_FLIGHT=${IN_FLIGHT:-32}
PIPELINE=${PIPELINE:-16}
SECS=${SECS:-15}
OUT=${BENCH_OUT:-/tmp/skynbd-compare}
mkdir -p "$OUT/raw"
SAMPLES="$OUT/samples.jsonl"
: > "$SAMPLES"

WORK=$(mktemp -d)
DAEMON_PID=""
AGENT_PID=""
RUN_DIR=""
MNT=""
BUCKET=""
RUN_ID=""
STORE_CONTAINER=${STORE_CONTAINER:-}
STORE_DATA_DIR=${STORE_DATA_DIR:-/data}

nbd_detach_all() {
  for i in 0 1 2 3; do
    if nbd-client -c "/dev/nbd$i" >/dev/null 2>&1; then nbd-client -d "/dev/nbd$i" >/dev/null 2>&1 || true; fi
  done
  return 0
}

stop_agent() {
  if [ -n "$AGENT_PID" ]; then
    kill "$AGENT_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do kill -0 "$AGENT_PID" 2>/dev/null || break; sleep 0.2; done
    kill -9 "$AGENT_PID" 2>/dev/null || true
    wait "$AGENT_PID" 2>/dev/null || true
    AGENT_PID=""
  fi
  return 0
}

thin_teardown() {
  if [ -n "$MNT" ]; then umount "$MNT" 2>/dev/null || true; fi
  for d in rcache rcdata rcmeta fs rec origin pool data tmeta; do
    dmsetup remove "sky-wsv-$d" 2>/dev/null || true
  done
  # Every loop device backed by a file under $WORK, whatever the agent
  # named it: a loop still attached keeps its (even deleted) backing
  # file's blocks allocated, so a missed one leaks the file's whole size.
  local dev back
  while read -r dev back; do
    case $back in "$WORK"/*) losetup -d "$dev" 2>/dev/null || true ;; esac
  done < <(losetup -l -n -O NAME,BACK-FILE 2>/dev/null || true)
  return 0
}

stop_daemon() {
  if [ -n "$DAEMON_PID" ]; then
    kill "$DAEMON_PID" 2>/dev/null || true
    for _ in $(seq 1 50); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.2; done
    kill -9 "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
    DAEMON_PID=""
  fi
  return 0
}

# Per-run teardown, the EXIT trap of each run's subshell: stop
# everything the run started and, when it failed, dump what explains it
# into the job log (artifacts are not readable everywhere results get
# triaged): configs, daemon/agent/fio output, and the kernel's messages
# from this run (an EIO from the kernel NBD client is explained only there).
run_teardown() {
  rc=$?
  trap - ERR
  set +e
  stop_agent
  thin_teardown
  nbd_detach_all
  stop_daemon
  tc qdisc del dev lo root 2>/dev/null
  if [ "$rc" -ne 0 ] && [ -n "$RUN_DIR" ]; then
    echo "run FAILED (rc=$rc) in $RUN_DIR:"
    for f in skynbd.toml agent.toml fio.stderr; do
      [ -s "$RUN_DIR/$f" ] && { echo "--- $f"; tail -40 "$RUN_DIR/$f"; }
    done
    for f in skynbdd.log agent.log; do
      [ -s "$RUN_DIR/$f" ] || continue
      echo "--- $f (warnings/errors, then tail)"
      grep -E ' (WARN|ERROR) |panicked' "$RUN_DIR/$f" | tail -40
      tail -15 "$RUN_DIR/$f"
    done
    echo "--- kernel messages during this run"
    dmesg 2>/dev/null | tail -n +"$((KMSG_START + 1))" | tail -60
    echo "--- disk"
    df -h / /tmp 2>/dev/null
    if [ -n "$STORE_CONTAINER" ]; then
      docker exec "$STORE_CONTAINER" df -h "$STORE_DATA_DIR" 2>/dev/null
      echo "--- $STORE_CONTAINER log"
      docker logs --tail 30 "$STORE_CONTAINER" 2>&1
    fi
    mkdir -p "$OUT/failed/$(basename "$RUN_DIR")"
    cp "$RUN_DIR"/*.log "$RUN_DIR"/*.toml "$RUN_DIR"/fio.* "$OUT/failed/$(basename "$RUN_DIR")/" 2>/dev/null
  fi
  # What the run left in the store, then free it for the next run.
  if [ -n "$STORE_CONTAINER" ] && [ -n "$BUCKET" ]; then
    bytes=$(docker exec "$STORE_CONTAINER" du -sb "$STORE_DATA_DIR/$BUCKET" 2>/dev/null | cut -f1)
    if [ -n "$bytes" ]; then
      read -r v r p <<< "$RUN_ID"
      printf '{"variant": "%s", "rtt": "%s", "rep": %s, "pos": %s, "job": "store-written", "mib": %s}\n' \
        "$v" "$r" "$p" "${RUN_POS:-0}" "$((bytes / 1048576))" >> "$SAMPLES"
    fi
    docker exec "$STORE_CONTAINER" rm -rf "${STORE_DATA_DIR:?}/$BUCKET" 2>/dev/null
  fi
  # The run's own files go too: the agent's cache and read-cache files
  # alone are ~640 MiB per run, and keeping them until the job ends
  # filled the runner after 19 runs (run 7). Logs a failed run needs are
  # already copied to $OUT/failed above.
  [ -n "$RUN_DIR" ] && rm -rf "$RUN_DIR"
  # Disk accounting, so a leak names itself: free space, loops still
  # pointing into $WORK, and what the store holds in total.
  local loops store_mib="-"
  loops=$(losetup -l -n -O BACK-FILE 2>/dev/null | grep -c "^$WORK/" || true)
  if [ -n "$STORE_CONTAINER" ]; then
    store_mib=$(docker exec "$STORE_CONTAINER" du -sm "$STORE_DATA_DIR" 2>/dev/null | cut -f1)
  fi
  echo "   after: disk free $(df -h --output=avail / | tail -1 | tr -d ' '), run dirs left $(find "$WORK" -mindepth 1 -maxdepth 1 -type d | wc -l), loops under work dir ${loops:-0}, store total ${store_mib:-?} MiB"
  exit "$rc"
}

cleanup() {
  trap - ERR
  set +e
  tc qdisc del dev "$NETEM_DEV" root 2>/dev/null
  tc qdisc del dev lo root 2>/dev/null
  nbd_detach_all
  rm -rf "$WORK"
}
trap cleanup EXIT
KMSG_START=0
# The job log is the only window CI gives into a failure, so name the
# failing line and command before cleanup runs.
trap 'echo "compare: line $LINENO failed (rc=$?): $BASH_COMMAND" >&2' ERR

set_rtt() { # $1 = ms
  tc qdisc del dev "$NETEM_DEV" root 2>/dev/null || true
  if [ "$1" != 0 ]; then
    tc qdisc add dev "$NETEM_DEV" root netem delay "${1}ms"
  fi
}

# Agent<->API delay for this run: a prio qdisc on lo whose 4th band is
# netem, fed only by packets to or from the API port, so the agent's
# HTTP calls pay the round trip and nothing else on lo does (NBD, the
# raw-head data path). Half each way: both directions cross lo egress.
set_api_rtt() { # $1 = ms round trip, $2 = API port
  tc qdisc del dev lo root 2>/dev/null || true
  [ "$1" = 0 ] && return 0
  tc qdisc add dev lo root handle 1: prio bands 4
  tc qdisc add dev lo parent 1:4 handle 40: netem delay "$(( $1 * 500 ))us" limit 100000
  tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip dport "$2" 0xffff flowid 1:4
  tc filter add dev lo parent 1: protocol ip prio 1 u32 match ip sport "$2" 0xffff flowid 1:4
}

# Round trip as a client sees it: median TCP connect time to a URL.
measure_rtt() { # $1 = URL
  python3 - "$1" <<'PY'
import socket, sys, time, urllib.parse
u = urllib.parse.urlparse(sys.argv[1])
ts = []
for _ in range(5):
    t = time.perf_counter()
    socket.create_connection((u.hostname, u.port or 80), timeout=5).close()
    ts.append((time.perf_counter() - t) * 1000)
print(f"{sorted(ts)[2]:.2f}")
PY
}

start_daemon() { # $1 = bindir
  RUST_LOG=${RUST_LOG:-info} "$1/skynbdd" "$RUN_DIR/skynbd.toml" >> "$RUN_DIR/skynbdd.log" 2>&1 &
  DAEMON_PID=$!
  for _ in $(seq 1 100); do
    curl -sf "$API/healthz" >/dev/null 2>&1 && return 0
    kill -0 "$DAEMON_PID" 2>/dev/null || { echo "skynbdd died"; tail -40 "$RUN_DIR/skynbdd.log"; exit 1; }
    sleep 0.2
  done
  echo "skynbdd never became ready"; exit 1
}

attach() {
  if ! nbd-client -N bench/head 127.0.0.1 "$PORT" /dev/nbd0 -b 4096 >/dev/null; then
    sleep 1
    nbd-client -N bench/head 127.0.0.1 "$PORT" /dev/nbd0 -b 4096 >/dev/null
  fi
}

# Detach, restart the daemon, re-attach: the next read pass is cold
# (engine state rebuilt from refs + bucket; O_DIRECT skips the page cache).
cold_restart() { # $1 = bindir
  nbd-client -d /dev/nbd0 >/dev/null
  stop_daemon
  start_daemon "$1"
  attach
}

# Append one sample per fio job: {variant, rtt point, rep, job, iops, mibs, p50/p99/mean ms}.
record() { # $1 = fio json, $2 = variant, $3 = rtt point, $4 = rep
  python3 - "$@" >> "$SAMPLES" <<'PY'
import json, os, sys
path, variant, rtt, rep = sys.argv[1:5]
for job in json.load(open(path))["jobs"]:
    name = job["jobname"]
    rw = job["read"] if "read" in name else job["write"]
    pct = rw.get("clat_ns", {}).get("percentile", {})
    ms = lambda ns: round(ns / 1e6, 3)
    print(json.dumps({
        "variant": variant, "rtt": rtt, "rep": int(rep), "pos": int(os.environ.get("RUN_POS", 0)), "job": name,
        "iops": round(rw["iops"], 1), "mibs": round(rw["bw"] / 1024, 2),
        "lat_mean_ms": ms(rw.get("clat_ns", {}).get("mean", 0)),
        "lat_p50_ms": ms(pct.get("50.000000", 0)),
        "lat_p99_ms": ms(pct.get("99.000000", 0)),
    }))
PY
}

# fio with its stderr kept: on failure print it and the JSON's error
# field (fio reports many job errors only there).
run_fio() { # $1 = json out, rest = fio args
  local out=$1; shift
  if ! fio --output-format=json --output="$out" "$@" > "$RUN_DIR/fio.stdout" 2> "$RUN_DIR/fio.stderr"; then
    echo "fio failed ($out):" >&2
    cat "$RUN_DIR/fio.stderr" >&2
    python3 -c "import json,sys; [print(j['jobname'], 'error', j.get('error')) for j in json.load(open(sys.argv[1]))['jobs']]" "$out" >&2 2>/dev/null || tail -20 "$out" >&2
    return 1
  fi
}

FIO_COMMON=(--direct=1 --ioengine=libaio --filename=/dev/nbd0)

run_one() { # $1 label, $2 bindir, $3 profile, $4 rtt point (S or S+A), $5 rep
  local label=$1 bin=$2 profile=$3 rtt=$4 rep=$5
  local api_rtt=0
  case $rtt in *+*) api_rtt=${rtt#*+} ;; esac
  RUN_DIR="$WORK/$label-rtt$rtt-rep$rep"
  local raw="$OUT/raw/$label/rtt$rtt/rep$rep"
  mkdir -p "$RUN_DIR/agent" "$raw"
  KMSG_START=$(dmesg 2>/dev/null | wc -l || echo 0)
  MNT="$RUN_DIR/mnt"; mkdir -p "$MNT"
  PORT=$((21000 + RANDOM % 20000)); API_PORT=$((PORT + 1)); API="http://127.0.0.1:${API_PORT}"
  BUCKET=$(printf 'cmp-%s-r%s-%s-%s' "$label" "$rtt" "$rep" "$RANDOM" | tr -c 'a-z0-9-' '-' | cut -c1-60)
  RUN_ID="$label $rtt $rep"

  {
    echo "nbd_listen = \"127.0.0.1:${PORT}\""
    echo "api_listen = \"127.0.0.1:${API_PORT}\""
    echo "refs_path = \"${RUN_DIR}/refs.redb\""
    if [ "$profile" = tuned ]; then printf '\n[nbd]\nmax_in_flight = %s\n' "$IN_FLIGHT"; fi
    printf '\n[store]\nkind = "s3"\nendpoint = "%s"\nbucket = "%s"\n' "$S3_ENDPOINT" "$BUCKET"
    printf 'access_key = "minioadmin"\nsecret_key = "minioadmin"\n'
    printf '\n[[volumes]]\nname = "bench"\nsize = 268435456\n'
    printf '\n[[volumes]]\nname = "wsv"\nsize = 1073741824\n'
  } > "$RUN_DIR/skynbd.toml"

  set_api_rtt "$api_rtt" "$API_PORT"
  echo "== $label rtt=$rtt rep=$rep (store connect $(measure_rtt "$S3_ENDPOINT") ms, disk free $(df -h --output=avail / | tail -1 | tr -d ' '))"
  start_daemon "$bin"
  echo "   api connect $(measure_rtt "$API") ms"
  attach

  # Writes: each job ends in fsync, so the store PUTs count. Random
  # writes are size-bounded, not time-based: at QD32 a time-based job
  # writes without limit, and with chunk-granular content addressing
  # every 4k write can become a new 64 KiB object (run 5 filled the
  # runner that way and every later flush returned EIO).
  run_fio "$raw/write.json" "${FIO_COMMON[@]}" --stonewall --end_fsync=1 \
    --name=seqwrite-1m-qd1 --rw=write --bs=1M --size=64M --iodepth=1 \
    --name=randwrite-4k-qd1 --rw=randwrite --bs=4k --size=64M --io_size=16M --iodepth=1 --runtime=60 \
    --name=randwrite-4k-qd32 --rw=randwrite --bs=4k --size=64M --io_size=16M --iodepth=32 --runtime=60 \

  record "$raw/write.json" "$label" "$rtt" "$rep"

  # Cold reads, one restart before each pass.
  cold_restart "$bin"
  run_fio "$raw/seqread.json" "${FIO_COMMON[@]}" \
    --name=cold-seqread-1m-qd1 --rw=read --bs=1M --size=64M --iodepth=1 --runtime=$((SECS * 2))
  record "$raw/seqread.json" "$label" "$rtt" "$rep"
  cold_restart "$bin"
  run_fio "$raw/randread1.json" "${FIO_COMMON[@]}" \
    --name=cold-randread-4k-qd1 --rw=randread --bs=4k --size=64M --iodepth=1 --time_based --runtime="$SECS"
  record "$raw/randread1.json" "$label" "$rtt" "$rep"
  cold_restart "$bin"
  run_fio "$raw/randread32.json" "${FIO_COMMON[@]}" \
    --name=cold-randread-4k-qd32 --rw=randread --bs=4k --size=64M --iodepth=32 --time_based --runtime="$SECS"
  record "$raw/randread32.json" "$label" "$rtt" "$rep"
  nbd-client -d /dev/nbd0 >/dev/null

  # Thin-mount publish: the unmount's final cycle backfills everything.
  {
    echo "server = \"${API}\""
    echo "state_dir = \"${RUN_DIR}/agent\""
    printf '\n[mounts]\nnbd_host = "127.0.0.1"\nnbd_port = %s\n' "$PORT"
    printf 'cache_bytes = 536870912\nread_cache_dir = "%s/agent/rcache"\n' "$RUN_DIR"
    printf 'read_cache_bytes = 134217728\nchunk_bytes = 65536\ninterval_s = 3600\nfreeze_budget_ms = 30000\n'
    if [ "$profile" = tuned ]; then echo "publish_pipeline_depth = $PIPELINE"; fi
  } > "$RUN_DIR/agent.toml"
  RUST_LOG=${RUST_LOG:-info} "$bin/skynbd-agent" run "$RUN_DIR/agent.toml" > "$RUN_DIR/agent.log" 2>&1 &
  AGENT_PID=$!
  for _ in $(seq 1 50); do
    [ -S "$RUN_DIR/agent/ctl.sock" ] && break
    kill -0 "$AGENT_PID" 2>/dev/null || { echo "agent died"; tail -30 "$RUN_DIR/agent.log"; exit 1; }
    sleep 0.2
  done
  "$bin/skynbd-agent" mount wsv "$MNT" --thin --state-dir "$RUN_DIR/agent"
  run_fio "$raw/thinwrite.json" --directory="$MNT" \
    --name=thin-write-1m --rw=write --bs=1M --size=64M --refill_buffers --end_fsync=1 \
    --filename=pubfile
  record "$raw/thinwrite.json" "$label" "$rtt" "$rep"
  local t0 t1
  t0=$(date +%s.%N)
  "$bin/skynbd-agent" unmount "$MNT" --state-dir "$RUN_DIR/agent"
  t1=$(date +%s.%N)
  printf '{"variant": "%s", "rtt": "%s", "rep": %s, "pos": %s, "job": "publish-64m-unmount", "secs": %s}\n' \
    "$label" "$rtt" "$rep" "$RUN_POS" "$(python3 -c "print(round($t1 - $t0, 2))")" >> "$SAMPLES"

  stop_agent
  thin_teardown
  stop_daemon
  if grep -q panicked "$RUN_DIR/skynbdd.log"; then echo "skynbdd panicked"; tail -40 "$RUN_DIR/skynbdd.log"; exit 1; fi
  cp "$RUN_DIR/skynbdd.log" "$raw/" 2>/dev/null || true
  # RUN_DIR stays set: run_teardown (this subshell's EXIT trap) removes
  # it. Clearing it here skipped that removal on every successful run,
  # which is what leaked ~640 MiB per run in runs 7-9.
}

# Below this much free disk the matrix stops early (and still writes its
# summary): a runner that fills up kills the job and every number with it.
MIN_FREE_KB=${MIN_FREE_KB:-2097152}

# Interleave: every rep runs every RTT, and every RTT every variant, so
# no variant owns a quiet (or noisy) stretch of the runner's life. The
# variant order rotates per (rep, point), so no variant always runs
# first after a netem change or always follows the same predecessor's
# cleanup; each sample records its position (`pos`, 0-based).
read -r -a VARIANT_LIST <<< "$VARIANTS"
NVAR=${#VARIANT_LIST[@]}
point_idx=0
for rep in $(seq 1 "$REPS"); do
  for rtt in $RTTS; do
    set_rtt "${rtt%%+*}"
    start=$(( point_idx % NVAR ))
    point_idx=$((point_idx + 1))
    for i in $(seq 0 $((NVAR - 1))); do
      v=${VARIANT_LIST[$(( (start + i) % NVAR ))]}
      export RUN_POS=$i
      free_kb=$(df --output=avail -k / | tail -1 | tr -d ' ')
      if [ "$free_kb" -lt "$MIN_FREE_KB" ]; then
        echo "compare: stopping early, only ${free_kb} KiB free on /" >&2
        echo "{\"job\": \"STOPPED\", \"free_kb\": $free_kb, \"rtt\": \"$rtt\", \"rep\": $rep, \"variant\": \"-\"}" >> "$SAMPLES"
        break 3
      fi
      IFS=: read -r label bin profile <<< "$v"
      # Own subshell per run: a failing variant is recorded and the
      # matrix goes on, so one broken configuration can't take the
      # others' numbers down with it. set -e stays live inside (it would
      # not be in an `if`/`||` context).
      set +e
      (trap run_teardown EXIT; set -e; run_one "$label" "$bin" "${profile:-default}" "$rtt" "$rep")
      rc=$?
      set -e
      if [ "$rc" -ne 0 ]; then
        printf '{"variant": "%s", "rtt": "%s", "rep": %s, "pos": %s, "job": "FAILED", "rc": %s}\n' \
          "$label" "$rtt" "$rep" "$RUN_POS" "$rc" >> "$SAMPLES"
      fi
    done
  done
done
set_rtt 0

# Provenance: what a cross-run comparison would have to hold equal. Deltas
# are only valid within one run (same runner VM, interleaved); runner VMs
# differ by up to ~30% on the same binary.
{
  echo "kernel: $(uname -r), cpus: $(nproc), reps: $REPS, fio secs: $SECS"
  echo "cpu: $(lscpu 2>/dev/null | sed -n 's/^Model name: *//p' | head -1)"
  echo "fio: $(fio --version 2>/dev/null)"
  echo "tuned: [nbd] max_in_flight=$IN_FLIGHT, publish_pipeline_depth=$PIPELINE"
  for v in $VARIANTS; do
    IFS=: read -r label bin _ <<< "$v"
    echo "variant $label: $(cat "$bin/REVISION" 2>/dev/null || echo "revision unknown") ($bin)"
  done
  if [ -n "$STORE_CONTAINER" ]; then
    echo "store: $(docker inspect -f '{{.Config.Image}} {{.Image}}' "$STORE_CONTAINER" 2>/dev/null)"
  fi
  lsblk -dno NAME,SIZE,ROTA,MODEL 2>/dev/null | grep -v '^nbd' | sed 's/^/disk: /'
} > "$OUT/provenance.txt"

python3 - "$SAMPLES" "$VARIANTS" "$OUT/results.json" > "$OUT/summary.md" <<'PY'
import json, statistics, sys
from collections import defaultdict

samples = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
variants = [v.split(":")[0] for v in sys.argv[2].split()]
json.dump(samples, open(sys.argv[3], "w"), indent=1)
base = variants[0]

# (job, metric, higher_is_better, label)
METRICS = [
    ("seqwrite-1m-qd1", "mibs", True, "MiB/s"),
    ("randwrite-4k-qd1", "iops", True, "IOPS"),
    ("randwrite-4k-qd32", "iops", True, "IOPS"),
    ("cold-seqread-1m-qd1", "mibs", True, "MiB/s"),
    ("cold-seqread-1m-qd1", "lat_p50_ms", False, "p50 ms"),
    ("cold-randread-4k-qd1", "iops", True, "IOPS"),
    ("cold-randread-4k-qd1", "lat_p50_ms", False, "p50 ms"),
    ("cold-randread-4k-qd1", "lat_p99_ms", False, "p99 ms"),
    ("cold-randread-4k-qd32", "iops", True, "IOPS"),
    ("cold-randread-4k-qd32", "lat_p50_ms", False, "p50 ms"),
    ("cold-randread-4k-qd32", "lat_p99_ms", False, "p99 ms"),
    ("thin-write-1m", "mibs", True, "MiB/s"),
    ("publish-64m-unmount", "secs", False, "s"),
    ("store-written", "mib", False, "MiB in store"),
]

by = defaultdict(list)
for s in samples:
    for m in ("iops", "mibs", "lat_p50_ms", "lat_p99_ms", "secs", "mib"):
        if m in s:
            by[(s["rtt"], s["job"], m, s["variant"])].append(s[m])

def med(xs):
    return statistics.median(xs) if xs else None

def fmt(x):
    if x is None:
        return "–"
    return f"{x:.0f}" if abs(x) >= 100 else f"{x:.2f}" if abs(x) < 10 else f"{x:.1f}"

def cell(xs):
    # median, then the min–max spread once there are 3+ reps to judge noise by
    if not xs:
        return "–"
    if len(xs) < 3:
        return fmt(med(xs))
    return f"{fmt(med(xs))} ({fmt(min(xs))}–{fmt(max(xs))})"

def point(label):
    store, _, api = label.partition("+")
    return (float(store), float(api or 0))

def title(label):
    store, api = point(label)
    return f"store RTT +{store:g} ms, agent↔API RTT +{api:g} ms"

print("# skynbd A/B bench (record-only)\n")
failed = [s for s in samples if s["job"] == "FAILED"]
for s in samples:
    if s["job"] == "STOPPED":
        print(f"**Matrix stopped early** before rep {s['rep']} at {s['rtt']}: only {s['free_kb'] // 1024} MiB free on the runner.\n")
if failed:
    print("**Failed runs** (their partial numbers are still in the tables; diagnostics in the job log):\n")
    for f in failed:
        print(f"- `{f['variant']}` at {title(f['rtt'])}, rep {f['rep']} (rc={f['rc']})")
    print()
print("Cells: median (min–max over reps). Δ is the median vs `%s` (+ = better, either direction of metric);" % base)
print("treat |Δ| inside the spread, or under ~10%, as noise on shared runners. Δs are valid only")
print("within this run (one runner VM, interleaved): don't compare numbers across runs.\n")
for rtt in sorted({s["rtt"] for s in samples}, key=point):
    print(f"## {title(rtt)}\n")
    hdr = ["workload", "metric"] + variants + [f"Δ {v}" for v in variants[1:]]
    print("| " + " | ".join(hdr) + " |")
    print("|" + "---|" * len(hdr))
    for job, m, hib, unit in METRICS:
        xss = [by[(rtt, job, m, v)] for v in variants]
        vals = [med(xs) for xs in xss]
        if all(x is None for x in vals):
            continue
        deltas = []
        for x in vals[1:]:
            b = vals[0]
            if x is None or not x or not b:
                deltas.append("–")
            else:
                d = (x / b - 1) * 100 if hib else (b / x - 1) * 100
                deltas.append(f"{d:+.0f}%")
        print("| " + " | ".join([job, unit] + [cell(xs) for xs in xss] + deltas) + " |")
    print()
PY
{
  echo '```'
  cat "$OUT/provenance.txt"
  echo '```'
} >> "$OUT/summary.md"
cat "$OUT/summary.md"

# A variant whose every run failed (e.g. its store never came up) means
# the comparison measured nothing for it: fail, rather than end green.
dead=$(python3 - "$SAMPLES" "$VARIANTS" <<'PY'
import json, sys
samples = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
for v in (x.split(":")[0] for x in sys.argv[2].split()):
    runs = {(s["rtt"], s["rep"]) for s in samples if s.get("variant") == v}
    failed = {(s["rtt"], s["rep"]) for s in samples if s.get("variant") == v and s["job"] == "FAILED"}
    if not runs or runs == failed:
        print(v)
PY
)
if [ -n "$dead" ]; then
  echo "compare: every run failed for: $dead" >&2
  exit 3
fi
echo "compare OK — results.json, summary.md, raw/ under $OUT"

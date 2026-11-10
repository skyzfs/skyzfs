#!/usr/bin/env bash
# E2E scenario runner (design §14.3, Tier A).
#
# Usage: tests/e2e/run.sh <S1|S2> [more scenarios...]
#
#   S1  basic IO round-trip: qemu-io write/flush/read + nbdinfo
#       negotiation conformance against a real skynbdd
#   S2  flush durability: write+flush, kill -9 skynbdd, restart, verify
#       (needs a persistent store: set SKYNBD_S3_TEST_ENDPOINT, e.g. MinIO
#       or moto; skipped otherwise)
#
# Env: SKYNBD_S3_TEST_ENDPOINT, AWS_ACCESS_KEY_ID, AWS_SECRET_ACCESS_KEY
set -euo pipefail
cd "$(dirname "$0")/../.."

need() { for t in "$@"; do command -v "$t" >/dev/null || { echo "missing tool: $t" >&2; exit 64; }; done; }
need qemu-io

# GNU dd as "$DD" (`gnudd` on ubuntu-26.04, plain `dd` before it): see
# the helper for why. Every dd below runs through it.
# shellcheck source=tests/lib/gnu-dd.sh
. tests/lib/gnu-dd.sh

WORK=$(mktemp -d)
SCEN_DIR="$WORK"       # per-scenario state dir; see scenario_init
RUN_ID=$$              # unique bucket per run: reruns never see stale objects
DAEMON_PID=""
AGENT_PID=""     # S15j: the long-running `agent run` under test
NETCUT_PID=""    # S15j: the cuttable link in front of the API
OPENERS_PID=""   # S15j: the origin-thin opener sampler
BT_PID=""        # S15j: blktrace|blkparse over the origin thin
cleanup() {
  rc=${rc:-$?}
  [ -z "$DAEMON_PID" ] || kill -9 "$DAEMON_PID" 2>/dev/null || true
  # Order matters on a failure path: kill the agent before the link, or
  # it spends a 60 s read timeout dying.
  [ -z "$AGENT_PID" ] || kill -9 "$AGENT_PID" 2>/dev/null || true
  [ -z "$NETCUT_PID" ] || kill -9 "$NETCUT_PID" 2>/dev/null || true
  [ -z "$OPENERS_PID" ] || kill -9 "$OPENERS_PID" 2>/dev/null || true
  # blktrace holds debugfs buffers and a kernel tracepoint per CPU, so a
  # leaked one outlives the run and poisons the next scenario's trace.
  # SIGINT first — it is the only signal that makes blktrace close its
  # buffers cleanly — then the pipeline's own subshell.
  [ -z "$BT_PID" ] || { kill -INT "$BT_PID" 2>/dev/null || true; \
                        kill -9 "$BT_PID" 2>/dev/null || true; }
  if [ "$rc" -ne 0 ]; then
    # Preserve evidence for CI log collection (deleted only on success).
    KEEP="/var/tmp/skynbd-e2e-failed"
    { mkdir -p "$KEEP" && cp -r "$WORK" "$KEEP/" 2>/dev/null; } || true
    echo "e2e FAILED (rc=$rc): logs preserved under $KEEP"
  fi
  rm -rf "$WORK"
}
trap cleanup EXIT

# Every scenario gets a fresh state dir (refs, config, logs): scenarios
# must be independent — sharing refs.redb across store backends is how the
# first version of this harness corrupted S2 after S1.
scenario_init() {
  SCEN_DIR="$WORK/$1"
  mkdir -p "$SCEN_DIR"
}

PORT=$((20000 + RANDOM % 20000))
URI="nbd://127.0.0.1:${PORT}/vol1/head"
API_PORT=$((PORT + 1))
API="http://127.0.0.1:${API_PORT}"

build_daemon() {
  [ -n "${E2E_SKIP_BUILD:-}" ] && return 0   # CI builds before sudo re-exec
  local features=()
  [ -z "${SKYNBD_S3_TEST_ENDPOINT:-}" ] || features=(--features skynbdd/s3 --features skynbdctl/s3)
  cargo build -q -p skynbdd -p skynbdctl "${features[@]}"
}

# ---- Tier B helpers (kernel NBD; needs root) --------------------------
tierb_available() {
  [ "$(id -u)" = 0 ] || { echo "needs root"; return 1; }
  command -v nbd-client >/dev/null || { echo "needs nbd-client"; return 1; }
  modprobe nbd max_part=8 2>/dev/null || [ -e /dev/nbd0 ] || { echo "cannot load nbd module"; return 1; }
  [ -e /dev/nbd0 ] || { echo "no /dev/nbd0"; return 1; }
  return 0
}

NBD_ATTACHED=()
nbd_attach() { # $1 = export, $2 = device
  # Retry once: nbd-client -d releases devices asynchronously, so an
  # immediate re-attach can race "already in use" (nightly round 5).
  if ! nbd-client -N "$1" 127.0.0.1 "$PORT" "$2" -b 4096 >/dev/null; then
    sleep 1
    nbd-client -N "$1" 127.0.0.1 "$PORT" "$2" -b 4096 >/dev/null
  fi
  NBD_ATTACHED+=("$2")
}

tierb_cleanup() {
  set +e
  [ -n "${MOUNT_A:-}" ] && { fsfreeze -u "$MOUNT_A" 2>/dev/null; umount "$MOUNT_A" 2>/dev/null; }
  [ -n "${MOUNT_B:-}" ] && umount "$MOUNT_B" 2>/dev/null
  [ -n "${VG:-}" ] && { vgchange -an "$VG" 2>/dev/null; vgremove -ff "$VG" 2>/dev/null; }
  for d in "${NBD_ATTACHED[@]:-}"; do
    # VERIFIED release, not an issued command. `nbd-client -d` is
    # asynchronous (nbd_attach's retry note measured it) and can be
    # refused outright while dm/udev still hold the device from the VG
    # teardown above — silently, under the 2>/dev/null. The wait reads
    # the same kernel marker the dispatch loop's leak probe reads, so
    # "released" here means what the probe will conclude: without this,
    # S14's success path handed the detector a /dev/nbd0 whose release
    # was in flight, and the WARNING blamed a scenario that had done
    # everything but wait. Re-issued each round because a refused
    # disconnect does not retry itself once the holder lets go.
    for _ in $(seq 1 25); do
      nbd-client -d "$d" 2>/dev/null
      [ -e "/sys/block/${d#/dev/}/pid" ] || break
      sleep 0.2
    done
    # Evidence travels with the failure: if the bound runs out, say so
    # here in the scenario's own output — the dispatch probe's WARNING
    # one step later is the alarm, this line is the diagnosis. Worded to
    # stay out of the nightly leak grep (^WARNING: scenario .* left),
    # which belongs to the probe alone.
    [ -e "/sys/block/${d#/dev/}/pid" ] &&
      echo "cleanup: $d still connected after 5s of disconnect retries — a holder never let go"
  done
  NBD_ATTACHED=()
  set -e
}

# Umount and mount the same device again — the "cold read" check every dm
# scenario ends with.
#
# Written as a helper because S15i fails here in CI and nowhere else, and
# `mount` answers EBUSY ("already mounted or mount point busy") on a
# device the preceding `umount` just released. EBUSY on the SOURCE means
# something still holds the block device exclusively; the target is
# demonstrably free, since umount returned 0 and `set -e` would have
# stopped us otherwise. The usual suspects are a udev worker probing the
# device after a dm table change, and a superblock teardown still in
# flight — both transient, both invisible after the fact, which is why
# the first failed attempt dumps who holds what BEFORE retrying.
#
# The retry is not the fix if the cause turns out to be structural; it is
# what turns a dead run into evidence. A scenario that only ever passes
# on attempt 2+ is telling us something, and the diagnostics say what.
# The no-retry form. A property that is never transient must not be
# asserted with a retry loop: the swing either leaves the block device
# frozen or it does not, so ten seconds of retrying can only turn a
# partial regression — one of the four retargets leaking under a race —
# into a pass with a `succeeded on attempt N` line scrolling by. The
# evidence for the fix was "both remounts took the FIRST attempt"; this
# is that sentence, encoded.
remount_fs_strict() { # $1 = device, $2 = mountpoint, $3 = optional sibling
  umount "$2"
  if command -v udevadm >/dev/null 2>&1; then udevadm settle --timeout=10 2>/dev/null || true; fi
  if mount "$1" "$2" 2>"$SCEN_DIR/remount.err"; then
    return 0
  fi
  remount_diag "$1" "$2"
  echo "FAIL: $1 was refused on the FIRST attempt, and this property does not retry"
  remount_probe "$1" "$2" "${3:-}"
  return 1
}

remount_fs() { # $1 = device, $2 = mountpoint
  umount "$2"
  if command -v udevadm >/dev/null 2>&1; then udevadm settle --timeout=10 2>/dev/null || true; fi
  local i=0
  while [ "$i" -lt 20 ]; do
    if mount "$1" "$2" 2>"$SCEN_DIR/remount.err"; then
      [ "$i" -gt 0 ] && echo "note: remount of $1 succeeded on attempt $((i + 1)) — see remount-diag above"
      return 0
    fi
    if [ "$i" = 0 ]; then
      remount_diag "$1" "$2"
    fi
    i=$((i + 1))
    sleep 0.5
  done
  echo "FAIL: $1 could not be remounted on $2 after 10s of retries"
  cat "$SCEN_DIR/remount.err" 2>/dev/null
  remount_probe "$1" "$2" "${3:-}"
  return 1
}

remount_diag() { # $1 = device, $2 = mountpoint
  {
      echo "=== remount-diag: $1 -> $2 refused on the first attempt ==="
      cat "$SCEN_DIR/remount.err" 2>/dev/null
      echo "--- mounts mentioning the device or target ---"
      grep -E "$(basename "$1")|$2" /proc/self/mountinfo 2>/dev/null || echo "(none)"
      command -v findmnt >/dev/null 2>&1 && findmnt -o SOURCE,TARGET,FSTYPE,OPTIONS 2>/dev/null | grep -E "vlin15i|s15b|s15j" || true
      echo "--- dm devices ---"
      dmsetup ls 2>/dev/null || true
      dmsetup info -c 2>/dev/null | head -20 || true
      echo "--- table of $(basename "$1") ---"
      dmsetup table "$(basename "$1")" 2>/dev/null || true
      # The decisive datum when it is a process rather than the block
      # layer: sysfs `holders` only shows device stacking, never who has
      # the fd. If a udev worker is mid-probe, this is what names it.
      # `fuser` exits 1 when NOTHING holds the file, so its status must not
      # be read as "tool missing" — that mislabelling cost a round.
      echo "--- processes holding the device ---"
      if command -v fuser >/dev/null 2>&1; then
        fuser -v "$1" 2>&1 || echo "(fuser: no process holds it)"
      else
        echo "(fuser unavailable)"
      fi
      if command -v lsof >/dev/null 2>&1; then
        lsof "$1" 2>/dev/null || echo "(lsof: no process holds it)"
      fi
      echo "--- what the device actually contains ---"
      command -v blkid >/dev/null 2>&1 && { blkid -p -o full "$1" 2>&1 || echo "(blkid found no signature)"; }
      echo "--- the target ---"
      ls -ld "$2" 2>&1
      command -v findmnt >/dev/null 2>&1 && { findmnt -T "$2" 2>&1 | head -5 || true; }
      echo "--- kernel ring buffer (the mount's own reason) ---"
      dmesg 2>/dev/null | tail -40 || echo "(dmesg unavailable)"
      echo "--- udev queue ---"
      command -v udevadm >/dev/null 2>&1 && udevadm settle --timeout=1 >/dev/null 2>&1 \
        && echo "(settled)" || echo "(udev queue not drained or udevadm unavailable)"
      echo "--- /dev/mapper nodes and holders ---"
      ls -l /dev/mapper/ 2>/dev/null || true
      for h in /sys/block/dm-*/holders /sys/block/loop*/holders; do
        [ -d "$h" ] && { printf '%s: ' "$h"; find "$h" -mindepth 1 -maxdepth 1 -printf '%f ' 2>/dev/null; echo; }
      done
      command -v lsblk >/dev/null 2>&1 && lsblk -o NAME,MAJ:MIN,SIZE,MOUNTPOINT 2>/dev/null || true
      echo "=== end remount-diag ==="
  } || true
}

# Active probes, run only once the remount is already lost — each splits
# the hypothesis space in a different place, and each undoes itself.
# Read-only throughout: nothing here can be what broke the scenario.
remount_probe() { # $1 = device, $2 = mountpoint, $3 = optional sibling
  echo "=== remount-probe: narrowing what is actually refused ==="
  echo "--- explicit type, read-only (separates 'needs recovery' from 'busy') ---"
  if mount -t ext4 -o ro,noload "$1" "$2" 2>&1; then
    echo "PROBE: ro+noload mount SUCCEEDED — the image is fine and the device is free;"
    echo "PROBE: the refusal is specific to the read-write mount."
    umount "$2" 2>/dev/null || true
  else
    echo "PROBE: ro+noload mount also refused."
  fi
  # $3 = optional sibling device over the SAME image (the discard port).
  # If the sibling mounts and the shim does not, the image is innocent and
  # the shim's device node is the suspect; if neither does, it is the image.
  if [ -n "${3:-}" ] && [ -e "$3" ]; then
    echo "--- sibling node over the same image: $3 ---"
    if mount -t ext4 -o ro,noload "$3" "$2" 2>&1; then
      echo "PROBE: the SIBLING mounted — the filesystem image is intact, so the"
      echo "PROBE: refusal belongs to $1 as a device, not to its contents."
      umount "$2" 2>/dev/null || true
    else
      echo "PROBE: the sibling is refused too — the image itself is the problem."
    fi
  fi
  echo "--- kernel ring buffer after the probes ---"
  dmesg 2>/dev/null | tail -40 || true
  echo "=== end remount-probe ==="
  # Diagnostics are never themselves the failure: returning non-zero here
  # would trip `set -e` in the caller before it reached its own verdict.
  return 0
}

write_config() { # $1 = store kind: memory | s3
  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"

[rpo]
interval_s = ${RPO_INTERVAL:-0}

[[volumes]]
name = "vol1"
size = 67108864
EOF
  if [ "$1" = s3 ]; then
    cat >> "$SCEN_DIR/skynbd.toml" <<EOF

[store]
kind = "s3"
endpoint = "${SKYNBD_S3_TEST_ENDPOINT}"
bucket = "skynbd-e2e-${RUN_ID}-$(basename "$SCEN_DIR")"
access_key = "${AWS_ACCESS_KEY_ID:-minioadmin}"
secret_key = "${AWS_SECRET_ACCESS_KEY:-minioadmin}"
EOF
  else
    printf '\n[store]\nkind = "memory"\n' >> "$SCEN_DIR/skynbd.toml"
  fi
}

start_daemon() {
  ./target/debug/skynbdd "$SCEN_DIR/skynbd.toml" >> "$SCEN_DIR/daemon.log" 2>&1 &
  DAEMON_PID=$!
  # Probe the API, not an NBD export: works for every config shape
  # (lineage-only volumes, unix: NBD listeners, authed APIs — /healthz is
  # auth-exempt). NBD listeners are bound before the API serves, so a
  # healthy API implies connectable NBD (backlog covers the accept race).
  for _ in $(seq 1 50); do
    curl -sf "$API/healthz" >/dev/null 2>&1 && return 0
    kill -0 "$DAEMON_PID" 2>/dev/null || { echo "daemon died:"; cat "$SCEN_DIR/daemon.log"; exit 1; }
    sleep 0.2
  done
  echo "daemon never became ready"; cat "$SCEN_DIR/daemon.log"; exit 1
}

# Global postconditions (design §14.3): no panics in the daemon log.
postconditions() {
  if grep -q 'panicked' "$SCEN_DIR/daemon.log"; then
    echo "FAIL: daemon panicked"; cat "$SCEN_DIR/daemon.log"; exit 1
  fi
}

# FREEZEPROBE (Tier B): ONE dm suspend/resume pair DOES leak a block-device
# freeze when the filesystem is already frozen by userspace. Re-measured
# every run, because the suppressions below depend on it.
#
# ANSWERED — and this scenario now REGRESSION-TESTS the answer rather
# than posing the question. It was written open, because the S15i fix
# had inferred "one of those four pairs did not come back" without
# measuring it. The measurement came back:
#
#   one_lockfs_pair_on_frozen_fs=leaks
#
# So it is a dm-over-already-frozen-superblock interaction, not an
# ordering property of the swing's four pairs — EVERY pair leaks, not
# one of four (32db824). Which meant the branch that predicted
# `sweep_b_inner` had the same exposure was the live one: the sweep
# documents itself as running with "the freeze outliving the process
# that armed it", and its `thaw_fs` could not save it, since FITHAW is
# `thaw_super`, not `bdev_thaw`, so lowering `bd_fsfreeze_count` is the
# one thing it cannot do. The sweep was fixed accordingly (07949c0,
# 95901de).
#
# Every retarget that can run against a frozen superblock now suppresses
# dm's implicit `lock_fs`: the swing (dm.rs, four sites) and
# `sweep_b_inner`, which computes `suppress` and takes
# `reload_device_in_frozen_window` on all three frozen branches — `Ok`,
# `AlreadyFrozen` and `Wedged`. The only plain `reload_device` caller
# left is `provision.rs`'s `flush_cache`, at decommission with nothing
# frozen, where the implicit freeze balances itself.
#
# Kept, rather than retired, for the reason the verdict is phrased as a
# measurement and not a judgement: it is the only thing that would
# notice the kernel behaviour changing under us, and `leaks` is the
# result that says the suppressions are still load-bearing. A future
# `clean` here is not good news — it means the premise this design rests
# on has moved, and the suppressions want re-deriving rather than
# deleting.
#
# No agent, no thin pool: a loop, a linear shim, an ext4, and dmsetup.
freezeprobe() {
  echo "== FREEZEPROBE (Tier B): re-measure the lockfs leak on a frozen fs (expect: leaks) =="
  if ! reason=$(tierb_available); then echo "FREEZEPROBE SKIPPED: $reason"; return 0; fi
  for t in dmsetup fsfreeze mkfs.ext4 losetup blockdev; do
    command -v "$t" >/dev/null || { echo "FREEZEPROBE SKIPPED: missing $t"; return 0; }
  done
  scenario_init freezeprobe
  # This is the one scenario that deliberately holds an `fsfreeze -f`
  # across several commands, so it is the one that most needs the trap:
  # any failure in that span otherwise leaves a frozen filesystem mounted
  # under $WORK, and `cleanup`'s `rm -rf` then blocks on it forever — a
  # hung job instead of a failed one. `tierb_cleanup` opens with
  # `fsfreeze -u "$MOUNT_A"` for exactly this.
  trap 'rc=$?; tierb_cleanup; cleanup' EXIT
  truncate -s 64M "$SCEN_DIR/fp.img"
  FP_LOOP=$(losetup -f --show "$SCEN_DIR/fp.img")
  mkfs.ext4 -q "$FP_LOOP"
  FP_SZ=$(blockdev --getsz "$FP_LOOP")
  dmsetup create sky-fp-shim --table "0 ${FP_SZ} linear ${FP_LOOP} 0"
  MOUNT_A="$SCEN_DIR/fpmnt"; mkdir -p "$MOUNT_A"
  mount /dev/mapper/sky-fp-shim "$MOUNT_A"
  echo probe > "$MOUNT_A/f"; sync

  # Exactly the swing's shape, once: userspace freeze, one plain
  # (lockfs-carrying) suspend/reload/resume of the mounted device,
  # userspace thaw.
  fsfreeze -f "$MOUNT_A"
  dmsetup suspend sky-fp-shim
  dmsetup reload sky-fp-shim --table "0 ${FP_SZ} linear ${FP_LOOP} 0"
  dmsetup resume sky-fp-shim
  fsfreeze -u "$MOUNT_A"

  umount "$MOUNT_A"
  FP_VERDICT=leaks
  if mount /dev/mapper/sky-fp-shim "$MOUNT_A" 2>"$SCEN_DIR/fp.err"; then
    FP_VERDICT=clean
    umount "$MOUNT_A"
  fi
  MOUNT_A=""
  echo "FREEZEPROBE-RESULT: one_lockfs_pair_on_frozen_fs=${FP_VERDICT}"
  if [ "$FP_VERDICT" = leaks ]; then
    cat "$SCEN_DIR/fp.err" 2>/dev/null
    dmesg 2>/dev/null | tail -5 || true
    echo "FREEZEPROBE: expected — a single pair leaks, so the lock_fs suppression"
    echo "FREEZEPROBE: in the swing and in sweep_b_inner is still load-bearing."
  else
    echo "FREEZEPROBE: UNEXPECTED — a single pair balanced. This measured 'leaks'"
    echo "FREEZEPROBE: when the suppressions were derived, so the kernel behaviour"
    echo "FREEZEPROBE: they rest on has changed. Re-derive before deleting anything."
  fi
  dmsetup remove sky-fp-shim
  losetup -d "$FP_LOOP"
  echo "FREEZEPROBE PASS (this scenario reports a measurement; it does not judge)"
}

s1() {
  echo "== S1: basic IO round-trip (qemu-io + nbdinfo) =="
  scenario_init s1
  write_config memory
  start_daemon

  qemu-io -f raw --cache none \
    -c 'write -P 0xaa 0 1M' \
    -c 'flush' \
    -c 'read -P 0xaa 0 1M' \
    -c 'write -z 256k 128k' \
    -c 'read -P 0x00 256k 128k' \
    -c 'read -P 0xaa 384k 128k' \
    "$URI"
  # Reconnect: committed state must serve identical bytes (I3/I5 wire-level).
  qemu-io -f raw -r -c 'read -P 0xaa 0 256k' "$URI"

  if command -v nbdinfo >/dev/null; then
    nbdinfo "$URI" | tee "$SCEN_DIR/nbdinfo.txt"
    grep -q '67108864' "$SCEN_DIR/nbdinfo.txt"       # advertised size
    nbdinfo --list "nbd://127.0.0.1:${PORT}" | grep -q vol1
  fi

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S1 PASS"
}

s2() {
  echo "== S2: flush durability across kill -9 =="
  if [ -z "${SKYNBD_S3_TEST_ENDPOINT:-}" ]; then
    echo "S2 SKIPPED: SKYNBD_S3_TEST_ENDPOINT not set (needs persistent store)"
    return 0
  fi
  scenario_init s2
  write_config s3
  start_daemon

  qemu-io -f raw --cache none -c 'write -P 0x5c 0 512k' -c 'flush' "$URI"
  kill -9 "$DAEMON_PID"; DAEMON_PID=""   # crash: no shutdown checkpoint

  start_daemon                            # reopen from S3 + redb (I1, I4)
  qemu-io -f raw -r -c 'read -P 0x5c 0 512k' "$URI"

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S2 PASS"
}

s3() {
  echo "== S3: snapshot correctness & instant export =="
  need curl
  scenario_init s3
  write_config memory
  start_daemon

  # Write A, snapshot s1 (timed), overwrite with B.
  qemu-io -f raw --cache none -c 'write -P 0xa1 0 256k' -c 'flush' "$URI"
  t0=$(date +%s%N)
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" \
    -H 'content-type: application/json' -d '{"name":"s1"}' > /dev/null
  # Instant publication: poll until the snapshot export serves reads.
  SNAP_URI="nbd://127.0.0.1:${PORT}/vol1/snap/s1"
  until qemu-io -f raw -r -c 'read 0 512' "$SNAP_URI" >/dev/null 2>&1; do
    [ $(( ($(date +%s%N) - t0) / 1000000 )) -lt 1000 ] || {
      echo "FAIL: snapshot not mountable within 1s"; exit 1; }
    sleep 0.02
  done
  publish_ms=$(( ($(date +%s%N) - t0) / 1000000 ))
  echo "snapshot API -> mountable in ${publish_ms}ms"

  qemu-io -f raw --cache none -c 'write -P 0xb2 0 256k' -c 'flush' "$URI"

  # I5: snapshot reads A; head reads B; snapshot export is read-only.
  qemu-io -f raw -r -c 'read -P 0xa1 0 256k' "$SNAP_URI"
  qemu-io -f raw -r -c 'read -P 0xb2 0 256k' "$URI"
  if qemu-io -f raw -c 'write -P 0xff 0 4k' "$SNAP_URI" >/dev/null 2>&1; then
    echo "FAIL: snapshot export accepted a write"; exit 1
  fi

  # List, rollback, delete via the API.
  curl -sf "$API/v1/volumes/vol1/snapshots" | grep -q '"s1"'
  curl -sf -X POST "$API/v1/volumes/vol1/rollback" \
    -H 'content-type: application/json' -d '{"snapshot":"s1"}' > /dev/null
  qemu-io -f raw -r -c 'read -P 0xa1 0 256k' "$URI"   # head restored to A
  curl -sf -X DELETE "$API/v1/volumes/vol1/snapshots/s1" -o /dev/null
  until ! qemu-io -f raw -r -c 'read 0 512' "$SNAP_URI" >/dev/null 2>&1; do sleep 0.05; done

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S3 PASS"
}

s5() {
  echo "== S5: trim / write-zeroes / sparse map =="
  need qemu-img
  scenario_init s5
  write_config memory
  start_daemon

  qemu-io -f raw --cache none \
    -c 'write -P 0xcc 0 256k' \
    -c 'discard 64k 64k' \
    -c 'write -z 128k 64k' \
    -c 'flush' "$URI"
  # Reads-as-zero for both discard and write-zeroes.
  qemu-io -f raw -r -c 'read -P 0xcc 0 64k' -c 'read -P 0x00 64k 128k' \
    -c 'read -P 0xcc 192k 64k' "$URI"

  # qemu-img map exercises structured replies + base:allocation (§8.3).
  qemu-img map --output=json "$URI" > "$SCEN_DIR/map.json"
  python3 - "$SCEN_DIR/map.json" <<'PY'
import json, sys
m = json.load(open(sys.argv[1]))
def seg(off): return next(e for e in m if e["start"] <= off < e["start"] + e["length"])
assert seg(0)["data"] is True, m
assert seg(64*1024)["data"] is False and seg(64*1024)["zero"] is True, m
assert seg(128*1024)["data"] is False, m
assert seg(192*1024)["data"] is True, m
assert seg(256*1024)["data"] is False, m
print("map OK")
PY

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S5 PASS"
}

md5_of_export() { # $1 = export uri suffix after port/
  qemu-img convert -f raw -O raw "nbd://127.0.0.1:${PORT}/$1" "$SCEN_DIR/dump.raw"
  md5sum "$SCEN_DIR/dump.raw" | cut -d' ' -f1
  rm -f "$SCEN_DIR/dump.raw"
}

s8() {
  echo "== S8: GC safety and reclamation =="
  need qemu-img curl
  scenario_init s8
  write_config memory
  start_daemon

  qemu-io -f raw --cache none -c 'write -P 0x01 0 1M' -c 'flush' "$URI"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"s1"}' >/dev/null
  qemu-io -f raw --cache none -c 'write -P 0x02 0 1M' -c 'flush' "$URI"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"s2"}' >/dev/null
  qemu-io -f raw --cache none -c 'write -P 0x03 0 1M' -c 'flush' "$URI"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"s3"}' >/dev/null

  sleep 0.3   # allow snapshot exports to publish
  s1_md5=$(md5_of_export vol1/snap/s1)
  s3_md5=$(md5_of_export vol1/snap/s3)
  head_md5=$(md5_of_export vol1/head)

  # Baseline GC, then delete s2 and reclaim.
  curl -sf -X POST "$API/v1/volumes/vol1/gc" >/dev/null
  curl -sf -X DELETE "$API/v1/volumes/vol1/snapshots/s2" -o /dev/null
  swept=$(curl -sf -X POST "$API/v1/volumes/vol1/gc" | python3 -c 'import json,sys; print(json.load(sys.stdin)["swept_objects"])')
  [ "$swept" -gt 0 ] || { echo "FAIL: GC swept nothing after snapshot delete"; exit 1; }
  echo "GC swept $swept objects for s2"

  # Survivors unchanged (I6), fsck clean; concurrent-create round.
  [ "$(md5_of_export vol1/snap/s1)" = "$s1_md5" ] || { echo "FAIL: s1 changed"; exit 1; }
  [ "$(md5_of_export vol1/snap/s3)" = "$s3_md5" ] || { echo "FAIL: s3 changed"; exit 1; }
  [ "$(md5_of_export vol1/head)" = "$head_md5" ] || { echo "FAIL: head changed"; exit 1; }
  curl -sf -X POST "$API/v1/volumes/vol1/fsck" | grep -qv '"error"' || true
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"s4"}' >/dev/null
  curl -sf -X POST "$API/v1/volumes/vol1/gc" >/dev/null
  sleep 0.2
  [ "$(md5_of_export vol1/snap/s4)" = "$head_md5" ] || { echo "FAIL: s4 wrong after GC"; exit 1; }

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S8 PASS"
}

s9() {
  echo "== S9: restart and ref recovery =="
  if [ -z "${SKYNBD_S3_TEST_ENDPOINT:-}" ]; then
    echo "S9 SKIPPED: SKYNBD_S3_TEST_ENDPOINT not set (needs persistent store)"
    return 0
  fi
  need qemu-img curl
  scenario_init s9
  write_config s3
  start_daemon

  # 9a: graceful restart mid-workload — flushed data resumes.
  qemu-io -f raw --cache none -c 'write -P 0x11 0 512k' -c 'flush' "$URI"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"keep"}' >/dev/null
  kill -INT "$DAEMON_PID"; wait "$DAEMON_PID" 2>/dev/null || true; DAEMON_PID=""
  start_daemon
  qemu-io -f raw -r -c 'read -P 0x11 0 512k' "$URI"
  qemu-io -f raw --cache none -c 'write -P 0x12 512k 256k' -c 'flush' "$URI"
  head_md5=$(md5_of_export vol1/head)
  snap_md5=$(md5_of_export vol1/snap/keep)

  # 9b: total ref-store loss — rebuild from S3 alone (§6.5), offline.
  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  rm -f "$SCEN_DIR/refs.redb"
  ./target/debug/skynbdctl fsck "$SCEN_DIR/skynbd.toml" vol1 --rebuild-refs > "$SCEN_DIR/fsck.json"
  grep -q '"head_rebuilt": true' "$SCEN_DIR/fsck.json" || { echo "FAIL: head not rebuilt"; cat "$SCEN_DIR/fsck.json"; exit 1; }
  grep -q 'snap/keep' "$SCEN_DIR/fsck.json" || { echo "FAIL: snapshot ref not rebuilt"; exit 1; }

  start_daemon
  [ "$(md5_of_export vol1/head)" = "$head_md5" ] || { echo "FAIL: head content after rebuild"; exit 1; }
  [ "$(md5_of_export vol1/snap/keep)" = "$snap_md5" ] || { echo "FAIL: snapshot content after rebuild"; exit 1; }

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S9 PASS"
}

s6() {
  echo "== S6: RPO guarantee =="
  if [ -z "${SKYNBD_S3_TEST_ENDPOINT:-}" ]; then
    echo "S6 SKIPPED: SKYNBD_S3_TEST_ENDPOINT not set (needs persistent store)"
    return 0
  fi
  need curl
  scenario_init s6
  RPO_INTERVAL=2 write_config s3
  start_daemon

  # Generation writes: slot byte = i, flushed each iteration (~0.4s apart).
  last=0
  for i in $(seq 1 20); do
    printf '%02x' "$i" > /dev/null
    qemu-io -f raw --cache none -c "write -P $i 0 64k" -c flush "$URI" >/dev/null
    last=$i
    sleep 0.4
  done
  kill -9 "$DAEMON_PID"; DAEMON_PID=""   # crash

  start_daemon
  # vol/rpo must be mountable, and its generation within the rpo window:
  # interval 2s / 0.4s per gen => lag <= ~6 generations (2x slack + 1).
  qemu-img convert -f raw -O raw "nbd://127.0.0.1:${PORT}/vol1/rpo" "$SCEN_DIR/rpo.raw"
  gen=$(python3 -c "print(open('$SCEN_DIR/rpo.raw','rb').read(1)[0])")
  echo "rpo generation: $gen (last written: $last)"
  [ "$gen" -ge $((last - 6)) ] || { echo "FAIL: rpo lag too large"; exit 1; }
  [ "$gen" -le "$last" ] || { echo "FAIL: rpo ahead of writes?"; exit 1; }

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S6 PASS"
}

s7() {
  echo "== S7: diff / send / recv =="
  need curl qemu-img
  scenario_init s7
  # Two volumes: vol1 source, vol2 replica target.
  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"

[store]
kind = "memory"

[[volumes]]
name = "vol1"
size = 67108864

[[volumes]]
name = "vol2"
size = 67108864
EOF
  start_daemon

  # s1 baseline content, then a 256k mutation -> s2.
  qemu-io -f raw --cache none -c 'write -P 0xa5 0 4M' -c flush "$URI"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"s1"}' >/dev/null
  qemu-io -f raw --cache none -c 'write -P 0x5a 1M 256k' -c flush "$URI"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" -H 'content-type: application/json' -d '{"name":"s2"}' >/dev/null

  # Baseline stream -> vol2; incremental s1->s2 -> vol2.
  curl -sf "$API/v1/volumes/vol1/diff?to=s1" -o "$SCEN_DIR/base.skyd"
  curl -sf -X POST --data-binary @"$SCEN_DIR/base.skyd" "$API/v1/volumes/vol2/import" >/dev/null
  curl -sf "$API/v1/volumes/vol1/diff?from=s1&to=s2" -o "$SCEN_DIR/delta.skyd"

  # O(changed): 256k changed -> delta stream well under 1M.
  delta_size=$(stat -c%s "$SCEN_DIR/delta.skyd")
  echo "delta stream: ${delta_size} bytes for 262144 changed"
  [ "$delta_size" -lt 1048576 ] || { echo "FAIL: delta not O(changed)"; exit 1; }
  [ "$delta_size" -ge 262144 ] || { echo "FAIL: delta impossibly small"; exit 1; }

  # Torn stream: truncated upload must change nothing (409/400, content same).
  head -c $((delta_size / 2)) "$SCEN_DIR/delta.skyd" > "$SCEN_DIR/torn.skyd"
  code=$(curl -s -o /dev/null -w '%{http_code}' -X POST --data-binary @"$SCEN_DIR/torn.skyd" "$API/v1/volumes/vol2/import")
  [ "$code" = "400" ] || { echo "FAIL: torn import returned $code"; exit 1; }
  qemu-img convert -f raw -O raw "nbd://127.0.0.1:${PORT}/vol2/head" "$SCEN_DIR/v2a.raw"
  qemu-img convert -f raw -O raw "nbd://127.0.0.1:${PORT}/vol1/snap/s1" "$SCEN_DIR/s1.raw"
  cmp -s "$SCEN_DIR/v2a.raw" "$SCEN_DIR/s1.raw" || { echo "FAIL: torn stream mutated vol2"; exit 1; }

  # Good delta: vol2 == s2 byte-for-byte; import advanced vol2's rpo.
  curl -sf -X POST --data-binary @"$SCEN_DIR/delta.skyd" "$API/v1/volumes/vol2/import" | grep -q '"rpo_advanced":true'
  qemu-img convert -f raw -O raw "nbd://127.0.0.1:${PORT}/vol2/head" "$SCEN_DIR/v2b.raw"
  qemu-img convert -f raw -O raw "nbd://127.0.0.1:${PORT}/vol1/snap/s2" "$SCEN_DIR/s2.raw"
  cmp -s "$SCEN_DIR/v2b.raw" "$SCEN_DIR/s2.raw" || { echo "FAIL: replica != s2"; exit 1; }
  sleep 0.3
  qemu-io -f raw -r -c 'read 0 512' "nbd://127.0.0.1:${PORT}/vol2/rpo" >/dev/null \
    || { echo "FAIL: vol2/rpo not mountable after import"; exit 1; }

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S7 PASS"
}

# Config with an extra lineage volume for the ingestion scenarios (D14).
write_config_lineage() { # $1 = store kind
  write_config "$1"
  cat >> "$SCEN_DIR/skynbd.toml" <<EOF

[[volumes]]
name = "vlin"
size = 8388608
kind = "lineage"
EOF
}

COORD="python3 tests/e2e/coordinator.py"
LIN_SNAP() { echo "nbd://127.0.0.1:${PORT}/vlin/snap/$1"; }

s11() {
  echo "== S11: prepared lifecycle & idempotent retry =="
  need curl python3
  scenario_init s11
  write_config_lineage memory
  start_daemon

  # Source images: 512k baseline, then a mutated copy.
  python3 -c "open('$SCEN_DIR/src1.img','wb').write(bytes((i*7+1)%256 for i in range(524288)))"
  cp "$SCEN_DIR/src1.img" "$SCEN_DIR/src2.img"
  python3 -c "
d=bytearray(open('$SCEN_DIR/src2.img','rb').read()); d[131072:196608]=bytes(65536); d[0:65536]=b'\xEE'*65536
open('$SCEN_DIR/src2.img','wb').write(d)"

  # Bootstrap publish g0; hidden until published.
  $COORD upload "$API" vlin "$SCEN_DIR/src1.img" g0 | tee "$SCEN_DIR/up0.json"
  curl -sf "$API/v1/exports" | grep -q 'vlin/snap/g0' || { echo "FAIL: g0 not exported"; exit 1; }
  sleep 0.2
  qemu-img convert -f raw -O raw "$(LIN_SNAP g0)" "$SCEN_DIR/g0.img"
  head -c 524288 "$SCEN_DIR/g0.img" | cmp -s - "$SCEN_DIR/src1.img" || { echo "FAIL: g0 content"; exit 1; }

  # Incremental g1 from g0 with dedupe: unchanged chunks must dedupe.
  $COORD upload "$API" vlin "$SCEN_DIR/src2.img" g1 --base g0 | tee "$SCEN_DIR/up1.json"
  deduped=$(python3 -c "import json;print(json.load(open('$SCEN_DIR/up1.json'))['deduped_objects'])")
  [ "$deduped" -ge 5 ] || { echo "FAIL: expected dedupe of unchanged chunks (got $deduped)"; exit 1; }
  sleep 0.2
  qemu-img convert -f raw -O raw "$(LIN_SNAP g1)" "$SCEN_DIR/g1.img"
  head -c 524288 "$SCEN_DIR/g1.img" | cmp -s - "$SCEN_DIR/src2.img" || { echo "FAIL: g1 content"; exit 1; }
  # g0 unchanged (I5).
  qemu-img convert -f raw -O raw "$(LIN_SNAP g0)" "$SCEN_DIR/g0b.img"
  cmp -s "$SCEN_DIR/g0.img" "$SCEN_DIR/g0b.img" || { echo "FAIL: g0 mutated"; exit 1; }

  # Abort flow: prepared root vanishes without visible change.
  pid=$($COORD prepare "$API" vlin --base g1)
  curl -sf "$API/v1/volumes/vlin/prepared" | grep -q "$pid"
  curl -sf -X DELETE "$API/v1/volumes/vlin/prepared/$pid" -o /dev/null
  curl -sf "$API/v1/volumes/vlin/prepared" | grep -q "$pid" && { echo "FAIL: abort left state"; exit 1; }

  curl -sf -X POST "$API/v1/volumes/vlin/fsck" | python3 -c "import json,sys; r=json.load(sys.stdin); assert all(e is None for _,e in r['refs']), r" 

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S11 PASS"
}

s12() {
  echo "== S12: pins & GC safety under ingestion =="
  need curl python3
  scenario_init s12
  write_config_lineage memory
  start_daemon

  python3 -c "open('$SCEN_DIR/a.img','wb').write(bytes((i*3+5)%256 for i in range(262144)))"
  $COORD upload "$API" vlin "$SCEN_DIR/a.img" g0 >/dev/null

  # Publish-implies-pin: delete refused (409) right after publish (I8).
  code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$API/v1/volumes/vlin/snapshots/g0")
  [ "$code" = "409" ] || { echo "FAIL: pinned delete returned $code"; exit 1; }

  # Prepared-from-base pins the base independently.
  pid=$($COORD prepare "$API" vlin --base g0)
  curl -sf -X DELETE "$API/v1/volumes/vlin/snapshots/g0/pins/coordinator" -o /dev/null
  code=$(curl -s -o /dev/null -w '%{http_code}' -X DELETE "$API/v1/volumes/vlin/snapshots/g0")
  [ "$code" = "409" ] || { echo "FAIL: base pin not enforced ($code)"; exit 1; }

  # GC during backfill: attach half, gc, then finish — content must be intact.
  $COORD attach-half "$API" vlin "$pid" "$SCEN_DIR/a.img" >/dev/null
  curl -sf -X POST "$API/v1/volumes/vlin/gc" >/dev/null
  $COORD resume "$API" vlin "$pid" "$SCEN_DIR/a.img" g1 >/dev/null
  sleep 0.2
  qemu-img convert -f raw -O raw "$(LIN_SNAP g1)" "$SCEN_DIR/g1.img"
  head -c 262144 "$SCEN_DIR/g1.img" | cmp -s - "$SCEN_DIR/a.img" || { echo "FAIL: g1 after mid-backfill GC"; exit 1; }

  # All pins released -> delete + gc reclaims; g1 untouched.
  curl -sf -X DELETE "$API/v1/volumes/vlin/snapshots/g1/pins/coordinator" -o /dev/null
  curl -sf -X DELETE "$API/v1/volumes/vlin/snapshots/g0" -o /dev/null
  swept=$(curl -sf -X POST "$API/v1/volumes/vlin/gc" | python3 -c 'import json,sys;print(json.load(sys.stdin)["swept_objects"])')
  echo "GC swept $swept after g0 delete"
  qemu-img convert -f raw -O raw "$(LIN_SNAP g1)" "$SCEN_DIR/g1b.img"
  cmp -s "$SCEN_DIR/g1.img" "$SCEN_DIR/g1b.img" || { echo "FAIL: g1 changed after GC"; exit 1; }
  curl -sf -X POST "$API/v1/volumes/vlin/fsck" | python3 -c "import json,sys; r=json.load(sys.stdin); assert all(e is None for _,e in r['refs']), r"

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S12 PASS"
}

s13() {
  echo "== S13: server crash during backfill; resume from watermark =="
  if [ -z "${SKYNBD_S3_TEST_ENDPOINT:-}" ]; then
    echo "S13 SKIPPED: SKYNBD_S3_TEST_ENDPOINT not set (needs persistent store)"
    return 0
  fi
  need curl python3
  scenario_init s13
  write_config_lineage s3
  start_daemon

  python3 -c "open('$SCEN_DIR/big.img','wb').write(bytes((i*11+3)%256 for i in range(4194304)))"
  pid=$($COORD prepare "$API" vlin)
  $COORD attach-half "$API" vlin "$pid" "$SCEN_DIR/big.img" >/dev/null

  kill -9 "$DAEMON_PID"; DAEMON_PID=""   # crash mid-backfill
  start_daemon

  # Status shows a durable watermark; resume replays + finishes + publishes.
  wm=$(curl -sf "$API/v1/volumes/vlin/prepared/$pid" | python3 -c 'import json,sys;print(json.load(sys.stdin)["op_watermark"])')
  echo "durable watermark after crash: $wm"
  $COORD resume "$API" vlin "$pid" "$SCEN_DIR/big.img" g | tee "$SCEN_DIR/resume.json"
  sleep 0.3
  qemu-img convert -f raw -O raw "$(LIN_SNAP g)" "$SCEN_DIR/g.img"
  head -c 4194304 "$SCEN_DIR/g.img" | cmp -s - "$SCEN_DIR/big.img" || { echo "FAIL: resumed content"; exit 1; }
  curl -sf "$API/v1/volumes/vlin/prepared" | grep -q "$pid" && { echo "FAIL: prepared state lingered"; exit 1; }
  curl -sf -X POST "$API/v1/volumes/vlin/fsck" | python3 -c "import json,sys; r=json.load(sys.stdin); assert all(e is None for _,e in r['refs']), r"

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S13 PASS"
}

make_corpus() { # $1 = dir
  mkdir -p "$1/corpus"
  for i in $(seq 1 24); do
    head -c $((i * 7919)) /dev/urandom > "$1/corpus/f$i"
  done
  (cd "$1" && find corpus -type f -exec md5sum {} + | sort) > "$1/corpus.md5"
}

check_corpus() { # $1 = mounted dir, $2 = md5 file
  (cd "$1" && find corpus -type f -exec md5sum {} + | sort) | diff -q - "$2" >/dev/null
}

s4() {
  echo "== S4 (Tier B): filesystem-consistent snapshot via fsfreeze =="
  if ! reason=$(tierb_available); then echo "S4 SKIPPED: $reason"; return 0; fi
  need curl mkfs.ext4
  scenario_init s4
  write_config memory
  start_daemon
  trap 'rc=$?; tierb_cleanup; cleanup' EXIT

  nbd_attach vol1/head /dev/nbd0
  mkfs.ext4 -q /dev/nbd0
  MOUNT_A="$SCEN_DIR/mnt"; mkdir -p "$MOUNT_A"
  mount /dev/nbd0 "$MOUNT_A"
  make_corpus "$MOUNT_A"
  sync "$MOUNT_A"

  # fs-consistent snapshot: freeze spans only the API call (D10 + V3.2 §9.10).
  fsfreeze -f "$MOUNT_A"
  curl -sf -X POST "$API/v1/volumes/vol1/snapshots" \
    -H 'content-type: application/json' -d '{"name":"s1"}' >/dev/null
  fsfreeze -u "$MOUNT_A"

  # Keep writing after the cut.
  head -c 1048576 /dev/urandom > "$MOUNT_A/corpus/after-snap"
  sync "$MOUNT_A"

  sleep 0.3
  nbd_attach vol1/snap/s1 /dev/nbd1
  fsck.ext4 -n -f /dev/nbd1 || { echo "FAIL: snapshot fs not clean"; exit 1; }
  MOUNT_B="$SCEN_DIR/mntb"; mkdir -p "$MOUNT_B"
  mount -o ro /dev/nbd1 "$MOUNT_B"
  check_corpus "$MOUNT_B" "$MOUNT_A/../mnt/corpus.md5" 2>/dev/null \
    || check_corpus "$MOUNT_B" "$MOUNT_A/corpus.md5" \
    || { echo "FAIL: snapshot corpus mismatch"; exit 1; }
  [ ! -e "$MOUNT_B/corpus/after-snap" ] || { echo "FAIL: post-cut write visible in snapshot"; exit 1; }

  tierb_cleanup
  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S4 PASS"
}

s10() {
  echo "== S10 (Tier B): fio performance baseline (record-only) =="
  if ! reason=$(tierb_available); then echo "S10 SKIPPED: $reason"; return 0; fi
  command -v fio >/dev/null || { echo "S10 SKIPPED: fio not installed"; return 0; }
  scenario_init s10
  write_config memory
  start_daemon
  trap 'rc=$?; tierb_cleanup; cleanup' EXIT

  nbd_attach vol1/head /dev/nbd0
  fio --name=seqwrite --filename=/dev/nbd0 --rw=write --bs=1M --size=32M \
      --name=randwrite --filename=/dev/nbd0 --rw=randwrite --bs=4k --size=8M \
      --name=randread --filename=/dev/nbd0 --rw=randread --bs=4k --size=8M \
      --direct=1 --output-format=json --output="$SCEN_DIR/fio.json" >/dev/null
  python3 - "$SCEN_DIR/fio.json" <<'PY'
import json, sys
j = json.load(open(sys.argv[1]))
for job in j["jobs"]:
    rw = job["read" if "read" in job["jobname"] else "write"]
    print(f"{job['jobname']}: {rw['iops']:.0f} iops, {rw['bw']/1024:.1f} MiB/s")
PY
  cp "$SCEN_DIR/fio.json" "${FIO_BASELINE_OUT:-/tmp/skynbd-fio-baseline.json}"

  tierb_cleanup
  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S10 PASS (baseline recorded)"
}

s14() {
  echo "== S14 (Tier B): full V3.2 coordinator loop =="
  if ! reason=$(tierb_available); then echo "S14 SKIPPED: $reason"; return 0; fi
  for t in lvcreate vgcreate thin_delta dmsetup fsfreeze mkfs.ext4; do
    command -v "$t" >/dev/null || { echo "S14 SKIPPED: missing $t"; return 0; }
  done
  need curl python3 qemu-img
  scenario_init s14
  # Attached pool volume (the V3.2 shape-B head) + lineage volume sized to
  # the thin LV (design §3.3): 1 GiB pool device, 512 MiB logical volume.
  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"

[store]
kind = "memory"

[[volumes]]
name = "pool"
size = 1073741824

[[volumes]]
name = "vlin"
size = 536870912
kind = "lineage"
EOF
  start_daemon
  trap 'rc=$?; tierb_cleanup; cleanup' EXIT

  # V3.2 shape B: dm-thin pool whose data device is the skynbd head.
  # (Test simplification vs §6.5: pool metadata also lands on the NBD PV —
  # placement policy is operational, not part of the hash-equality gate.)
  nbd_attach pool/head /dev/nbd0
  VG="skv$$"
  vgcreate -q "$VG" /dev/nbd0
  lvcreate -q -T "$VG/tp" -L 768M -c 64k
  lvcreate -q -V 512M -T "$VG/tp" -n A
  mkfs.ext4 -q "/dev/$VG/A"
  MOUNT_A="$SCEN_DIR/mnt"; mkdir -p "$MOUNT_A"
  mount "/dev/$VG/A" "$MOUNT_A"
  make_corpus "$MOUNT_A"; sync "$MOUNT_A"
  cp "$MOUNT_A/corpus.md5" "$SCEN_DIR/corpus-g0.md5"

  COORD_ARGS=(--api "$API" --vol vlin --vg "$VG" --pool tp --origin A \
    --mount "$MOUNT_A" --state "$SCEN_DIR/coord.json" --nbd-port "$PORT")

  # Bootstrap: G0 = full objectize of S0; hash(S0) == hash(G0) inside.
  python3 tests/coordinator/coordinator_v32.py bootstrap "${COORD_ARGS[@]}"

  # Cycle 1: overwrite + new file + discard (fstrim → left_only).
  head -c 2097152 /dev/urandom > "$MOUNT_A/corpus/f1"
  rm -f "$MOUNT_A/corpus/f2"
  head -c 524288 /dev/urandom > "$MOUNT_A/corpus/new1"
  sync "$MOUNT_A"; fstrim "$MOUNT_A" || true
  (cd "$MOUNT_A" && find corpus -type f -exec md5sum {} + | sort) > "$SCEN_DIR/corpus-g1.md5"
  python3 tests/coordinator/coordinator_v32.py cycle "${COORD_ARGS[@]}"

  # Cycle 2: more churn.
  for i in 5 6 7; do head -c 262144 /dev/urandom > "$MOUNT_A/corpus/f$i"; done
  sync "$MOUNT_A"
  (cd "$MOUNT_A" && find corpus -type f -exec md5sum {} + | sort) > "$SCEN_DIR/corpus-g2.md5"
  python3 tests/coordinator/coordinator_v32.py cycle "${COORD_ARGS[@]}"

  # Published snapshots are complete filesystems: mount g1 + g2 via kernel
  # NBD, fsck clean, corpora byte-identical (S14 pass criteria + S4 shape).
  for gen in 1 2; do
    nbd_attach "vlin/snap/g$gen" /dev/nbd$((gen + 1))
    fsck.ext4 -n -f "/dev/nbd$((gen + 1))" || { echo "FAIL: g$gen fsck"; exit 1; }
    MOUNT_B="$SCEN_DIR/mnt-g$gen"; mkdir -p "$MOUNT_B"
    mount -o ro "/dev/nbd$((gen + 1))" "$MOUNT_B"
    check_corpus "$MOUNT_B" "$SCEN_DIR/corpus-g$gen.md5" \
      || { echo "FAIL: g$gen corpus mismatch"; exit 1; }
    umount "$MOUNT_B"
  done
  MOUNT_B=""

  # fsck the lineage volume server-side too.
  curl -sf -X POST "$API/v1/volumes/vlin/fsck" | python3 -c "import json,sys; r=json.load(sys.stdin); assert all(e is None for _,e in r['refs']), r"

  tierb_cleanup
  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S14 PASS"
}


# Merge adjacent same-kind lines of a delta ("data|clear <off> <len>",
# sorted by offset), so two deltas that cover the same bytes compare
# equal however each side split its runs. See S14a's cross-check.
coalesce_delta() {
  awk '$1 == k && $2 == end { len += $3; end += $3; next }
       { if (k != "") print k, off, len; k = $1; off = $2; len = $3; end = $2 + $3 }
       END { if (k != "") print k, off, len }'
}

# S14a (Tier B, M10b): agent parity — the Rust agent (DmStack, raw
# dm-thin, no lvm2) runs bootstrap + 2 cycles over UDS + authed API.
s14a() {
  echo "== S14a (Tier B): Rust agent on raw dm-thin =="
  if ! reason=$(tierb_available); then echo "S14a SKIPPED: $reason"; return 0; fi
  for t in dmsetup fsfreeze mkfs.ext4 losetup fstrim; do
    command -v "$t" >/dev/null || { echo "S14a SKIPPED: missing $t"; return 0; }
  done
  AGENT=./target/debug/skynbd-agent
  [ -x "$AGENT" ] || { echo "S14a SKIPPED: build with cargo build -p skynbd-agent-cli"; return 0; }
  need curl python3 qemu-img
  scenario_init s14a
  STATE_DIR=/var/tmp/skynbd-agent-s14a-$$   # /var/tmp: not the frozen fs
  mkdir -p "$STATE_DIR"
  echo "s14a-secret" > "$STATE_DIR/token"

  # Daemon: pool head over a UNIX socket; authed API; lineage target.
  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "unix:${SCEN_DIR}/nbd.sock"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"
api_auth_token_path = "${STATE_DIR}/token"

[store]
kind = "memory"

[[volumes]]
name = "pool"
size = 1073741824

[[volumes]]
name = "vlin"
size = 536870912
kind = "lineage"
EOF
  start_daemon
  AUTH="Authorization: Bearer s14a-secret"

  # Pool head as a block device over the socket (no TCP anywhere).
  nbd-client -unix "${SCEN_DIR}/nbd.sock" -N pool/head /dev/nbd0 -b 4096 >/dev/null
  NBD_ATTACHED+=(/dev/nbd0)

  # Raw dm-thin: tmeta on a local loop file (V3.2 §6.5), data on the NBD
  # device; 64 KiB blocks (128 sectors), low-water 32.
  truncate -s 64M "$SCEN_DIR/tmeta.img"
  META=$(losetup -f --show "$SCEN_DIR/tmeta.img")
  "$DD" if=/dev/zero of="$META" bs=4096 count=1 conv=notrunc 2>/dev/null
  DATA_SECTORS=$(blockdev --getsz /dev/nbd0)
  dmsetup create sky-s14a-tpool --table "0 ${DATA_SECTORS} thin-pool ${META} /dev/nbd0 128 32"
  dmsetup message sky-s14a-tpool 0 "create_thin 1"
  VOL_SECTORS=$((536870912 / 512))
  dmsetup create sky-s14a-origin --table "0 ${VOL_SECTORS} thin /dev/mapper/sky-s14a-tpool 1"

  MOUNT_A="$SCEN_DIR/mnt"
  mkdir -p "$MOUNT_A"
  mkfs.ext4 -q /dev/mapper/sky-s14a-origin
  mount /dev/mapper/sky-s14a-origin "$MOUNT_A"
  make_corpus "$MOUNT_A"
  sync

  cat > "$STATE_DIR/agent.toml" <<EOF
server = "http://127.0.0.1:${API_PORT}"
token_path = "${STATE_DIR}/token"
state_dir = "${STATE_DIR}"

[[volume]]
name = "vlin"
pool_name = "sky-s14a-tpool"
origin_name = "sky-s14a-origin"
origin_dev_id = 1
metadata_dev = "${META}"
mountpoint = "${MOUNT_A}"
size_bytes = 536870912
chunk_bytes = 65536
freeze_budget_ms = 5000
EOF

  "$AGENT" preflight "$STATE_DIR/agent.toml" || { echo "FAIL: preflight"; exit 1; }

  # Generation loop: bootstrap + 2 delta cycles with mutations + discards.
  for gen in 0 1 2; do
    if [ "$gen" = 1 ]; then
      cp "$MOUNT_A/corpus/f1" "$MOUNT_A/corpus/f1.copy"
      "$DD" if=/dev/urandom of="$MOUNT_A/corpus/f3" bs=64K count=8 conv=notrunc 2>/dev/null
      rm "$MOUNT_A/corpus/f5"; fstrim "$MOUNT_A" || true
    elif [ "$gen" = 2 ]; then
      "$DD" if=/dev/urandom of="$MOUNT_A/corpus/big" bs=1M count=32 2>/dev/null
    fi
    sync
    (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
      > "$SCEN_DIR/manifest.g${gen}"
    "$AGENT" once "$STATE_DIR/agent.toml" 2>&1 | tee "$SCEN_DIR/once.g${gen}.log"
    grep -q "published snap/g${gen}" "$SCEN_DIR/once.g${gen}.log" \
      || { echo "FAIL: g${gen} not published"; exit 1; }
  done

  # Verify each published generation: fsck clean + corpus manifest match.
  MOUNT_B="$SCEN_DIR/mntb"
  mkdir -p "$MOUNT_B"
  for gen in 0 1 2; do
    dev="/dev/nbd$((gen + 1))"   # fresh device per gen: nbd-client -d releases async
    nbd-client -unix "${SCEN_DIR}/nbd.sock" -N "vlin/snap/g${gen}" "$dev" -b 4096 >/dev/null
    NBD_ATTACHED+=("$dev")
    fsck.ext4 -n -f "$dev" >/dev/null || { echo "FAIL: fsck g${gen}"; exit 1; }
    mount -o ro "$dev" "$MOUNT_B"
    (cd "$MOUNT_B/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
      | diff -u "$SCEN_DIR/manifest.g${gen}" - || { echo "FAIL: corpus g${gen}"; exit 1; }
    umount "$MOUNT_B"
    nbd-client -d "$dev"
  done

  # Cross-check the linked-thinp delta against the thin_delta binary, if
  # installed: snapshot the origin manually under agent naming, diff.
  if command -v thin_delta >/dev/null; then
    BASE_LOGICAL=$("$AGENT" state-dump "$STATE_DIR/agent.toml" | python3 -c \
      "import json,sys; print(json.load(sys.stdin)['baseline']['local']['lv_name'])")
    BASE_DEVID=$("$AGENT" state-dump "$STATE_DIR/agent.toml" | python3 -c \
      "import json,sys; print(json.load(sys.stdin)['baseline']['local']['thin_dev_id'])")
    "$DD" if=/dev/urandom of="$MOUNT_A/corpus/xchk" bs=64K count=4 2>/dev/null; sync
    fsfreeze -f "$MOUNT_A"
    dmsetup message sky-s14a-tpool 0 "create_snap 99 1"
    fsfreeze -u "$MOUNT_A"
    dmsetup create "sky-vlin-c-xcheck" --uuid "SKYNBD-vlin-99" \
      --table "0 ${VOL_SECTORS} thin /dev/mapper/sky-s14a-tpool 99"
    # Both sides go through `coalesce_delta`: thin_delta reports a run
    # whose blocks are mapped in both snapshots (`different`) and a run
    # mapped only in the newer one (`right_only`) as separate elements
    # even when they are adjacent, while the agent emits adjacent data as
    # one range. One nightly failed on exactly that: 2359296+65536,
    # 2424832+6225920 and 8650752+65536 against the agent's
    # 2359296+6356992, the same bytes. Where the split falls depends on
    # where ext4 and the test file land, so the raw diff was a coin flip.
    # Merging adjacent same-kind ranges compares what a delta means,
    # which bytes are data and which are clear, and is still exact.
    "$AGENT" debug-delta "$STATE_DIR/agent.toml" "$BASE_LOGICAL" xcheck \
      | sort -n -k2 | coalesce_delta > "$SCEN_DIR/agent.delta"
    dmsetup message sky-s14a-tpool 0 reserve_metadata_snap
    thin_delta --metadata-snap --snap1 "$BASE_DEVID" --snap2 99 "$META" > "$SCEN_DIR/td.xml"
    dmsetup message sky-s14a-tpool 0 release_metadata_snap
    python3 - "$SCEN_DIR/td.xml" <<'PY' | sort -n -k2 | coalesce_delta > "$SCEN_DIR/td.delta"
import sys, xml.etree.ElementTree as ET
root = ET.parse(sys.argv[1]).getroot()
bs = int(root.get("data_block_size")) * 512
for d in root.iter():
    b, l = d.get("begin"), d.get("length")
    if b is None: continue
    off, ln = int(b)*bs, int(l)*bs
    if d.tag in ("different", "right_only"): print(f"data {off} {ln}")
    elif d.tag == "left_only": print(f"clear {off} {ln}")
PY
    diff -u "$SCEN_DIR/td.delta" "$SCEN_DIR/agent.delta" \
      || { echo "FAIL: linked delta != thin_delta binary"; exit 1; }
    dmsetup remove sky-vlin-c-xcheck
    dmsetup message sky-s14a-tpool 0 "delete 99"
    echo "delta cross-check OK"
  else
    echo "note: thin_delta binary absent; cross-check skipped"
  fi

  # Server-side integrity.
  curl -sf -H "$AUTH" -X POST "$API/v1/volumes/vlin/fsck" | python3 -c '
import json,sys
r = json.load(sys.stdin)
bad = [x for x in r.get("refs", []) if x[1] is not None]
sys.exit(1 if bad else 0)' || { echo "FAIL: server fsck"; exit 1; }

  # Teardown (targeted — never remove_all on a shared host).
  umount "$MOUNT_A"; MOUNT_A=""
  for d in $(dmsetup ls 2>/dev/null | awk '/^sky-vlin-/ {print $1}'); do
    dmsetup remove "$d" 2>/dev/null || true
  done
  dmsetup remove sky-s14a-origin 2>/dev/null || true
  dmsetup remove sky-s14a-tpool 2>/dev/null || true
  losetup -d "$META" || true
  nbd-client -d /dev/nbd0; NBD_ATTACHED=()
  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  rm -rf "$STATE_DIR"
  echo "S14a PASS"
}

# S15b (Tier B, multi-node design D-M15 task 15b): reclaim content
# equivalence on real dm-thin. A thin device with an external origin
# (D-M11 shape) provisions blocks over its base; `debug-reclaim` discards
# mapped(S) − dirtied-since(S) after the backing has been given the
# published content (the repoint precondition) — and not one observable
# byte may move, with the reclaimed ranges now served through the origin
# and the pool measurably smaller. `no_discard_passdown` is on the pool
# table exactly as production provisions it.
s15b() {
  echo "== S15b (Tier B): dm-thin reclaim content equivalence =="
  if ! reason=$(tierb_available); then echo "S15b SKIPPED: $reason"; return 0; fi
  for t in dmsetup fsfreeze mkfs.ext4 losetup blockdev; do
    command -v "$t" >/dev/null || { echo "S15b SKIPPED: missing $t"; return 0; }
  done
  AGENT=./target/debug/skynbd-agent
  [ -x "$AGENT" ] || { echo "S15b SKIPPED: build with cargo build -p skynbd-agent-cli"; return 0; }
  scenario_init s15b
  STATE_DIR=/var/tmp/skynbd-agent-s15b-$$   # /var/tmp: not the frozen fs
  mkdir -p "$STATE_DIR"

  # Backing (stands in for the published generation the origin export
  # serves), pool data, pool metadata — all local loops; no daemon.
  truncate -s 256M "$SCEN_DIR/backing.img"
  truncate -s 256M "$SCEN_DIR/data.img"
  truncate -s 16M  "$SCEN_DIR/tmeta.img"
  BACK=$(losetup -f --show "$SCEN_DIR/backing.img")
  DATA=$(losetup -f --show "$SCEN_DIR/data.img")
  META=$(losetup -f --show "$SCEN_DIR/tmeta.img")
  "$DD" if=/dev/zero of="$META" bs=4096 count=1 conv=notrunc 2>/dev/null

  # The base filesystem lives on the backing; the thin device overlays
  # it copy-on-write. lazy-init off so background writes don't churn the
  # mapping set mid-scenario.
  mkfs.ext4 -q -E lazy_itable_init=0,lazy_journal_init=0 "$BACK"
  MOUNT_A="$SCEN_DIR/mnt"; mkdir -p "$MOUNT_A"
  mount "$BACK" "$MOUNT_A"; make_corpus "$MOUNT_A"; sync; umount "$MOUNT_A"

  SECT=$(blockdev --getsz "$BACK")
  dmsetup create sky-s15b-tpool --table \
    "0 $(blockdev --getsz "$DATA") thin-pool ${META} ${DATA} 128 32 2 skip_block_zeroing no_discard_passdown"
  dmsetup message sky-s15b-tpool 0 "create_thin 1"
  dmsetup create sky-s15b-origin --table \
    "0 ${SECT} thin /dev/mapper/sky-s15b-tpool 1 ${BACK}"
  # The D-M15 discard-port shape: the fs mounts a linear SHIM (its claim
  # lands there; dm claims the origin under its shared holder), and a
  # sibling linear PORT over the same origin gives userspace an
  # unclaimed node whose BLKDISCARDs flow down into dm-thin. Discarding
  # the origin (or the shim) directly is refused by the kernel: EBUSY
  # on a claimed bdev — the contract this scenario exists to pin.
  dmsetup create sky-s15b-fs --table \
    "0 ${SECT} linear /dev/mapper/sky-s15b-origin 0"
  dmsetup create sky-s15b-rec --table \
    "0 ${SECT} linear /dev/mapper/sky-s15b-origin 0"

  mount /dev/mapper/sky-s15b-fs "$MOUNT_A"
  # Provision over the base, then capture S under agent naming.
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/f2" bs=64K count=8 conv=notrunc 2>/dev/null
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/new1" bs=64K count=16 2>/dev/null
  sync
  fsfreeze -f "$MOUNT_A"
  dmsetup suspend sky-s15b-origin
  dmsetup message sky-s15b-tpool 0 "create_snap 2 1"
  dmsetup resume sky-s15b-origin
  fsfreeze -u "$MOUNT_A"
  dmsetup create sky-vlin15b-c-cap --uuid "SKYNBD-vlin15b-2" \
    --table "0 ${SECT} thin /dev/mapper/sky-s15b-tpool 2"

  # Dirtied-since: rewrite part of what S captured.
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/new1" bs=64K count=4 conv=notrunc 2>/dev/null
  sync

  cat > "$STATE_DIR/agent.toml" <<EOF
server = "http://127.0.0.1:1"
state_dir = "${STATE_DIR}"

[[volume]]
name = "vlin15b"
pool_name = "sky-s15b-tpool"
origin_name = "sky-s15b-origin"
origin_dev_id = 1
metadata_dev = "${META}"
mountpoint = "${MOUNT_A}"
size_bytes = $((SECT * 512))
chunk_bytes = 65536
freeze_budget_ms = 30000
discard_name = "sky-s15b-rec"
EOF

  # Pin the kernel contract first, with production's exact semantics
  # (plain O_RDWR open + BLKDISCARD ioctl, dm_linux.rs discard_dev — the
  # blkdiscard binary opens O_EXCL since util-linux 2.36 and would pin
  # util-linux, not the kernel). The load-bearing cell is the FS-CLAIMED
  # node: a raw discard of the mounted shim must be refused — that
  # refusal is what broke the shimless smoke and why staging needs the
  # shim shape at all. If it ever starts succeeding, the shim+port
  # apparatus can be retired. The merely dm-claimed origin is a
  # kernel-dependent cell (current CI kernels allow it; restricted-writes
  # kernels refuse the open) — the discard-port-poc workflow reports it;
  # production keeps using the port regardless as the config-proof path.
  # Probing chunk 0 of the mounted fs: save it via the port first so an
  # unexpectedly-allowed discard cannot cascade into confusing failures.
  "$DD" if=/dev/mapper/sky-s15b-rec of="$SCEN_DIR/chunk0" bs=65536 count=1 2>/dev/null
  if python3 - <<'NEG' 2>/dev/null
import fcntl, os, struct
fd = os.open("/dev/mapper/sky-s15b-fs", os.O_RDWR)
fcntl.ioctl(fd, 0x1277, struct.pack("QQ", 0, 65536))
NEG
  then
    "$DD" if="$SCEN_DIR/chunk0" of=/dev/mapper/sky-s15b-rec bs=65536 count=1 conv=notrunc 2>/dev/null
    sync
    echo "FAIL: raw BLKDISCARD ioctl on the fs-claimed shim unexpectedly succeeded"; exit 1
  fi

  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    > "$SCEN_DIR/manifest.before"
  USED_BEFORE=$(dmsetup status sky-s15b-tpool | awk '{print $6}' | cut -d/ -f1)

  # "Repoint": copy S's content into the backing over every reclaimable
  # range, so the backing serves exactly what the generation published
  # from S would (Invariant D's precondition for the §3 step-6 discard).
  # O_DIRECT so dm-thin's later reads of the backing can't miss it in a
  # stale loop buffer.
  "$AGENT" debug-reclaim --dry-run "$STATE_DIR/agent.toml" cap > "$SCEN_DIR/ranges"
  [ -s "$SCEN_DIR/ranges" ] || { echo "FAIL: empty reclaim set"; exit 1; }
  while read -r _ off len; do
    "$DD" if=/dev/mapper/sky-vlin15b-c-cap of="$BACK" bs=64K \
      skip=$((off / 65536)) seek=$((off / 65536)) count=$((len / 65536)) \
      iflag=fullblock oflag=direct conv=notrunc 2>/dev/null
  done < "$SCEN_DIR/ranges"
  blockdev --flushbufs "$BACK"

  # §7 calibration inputs (task 15i): the port executor's R and wall
  # time, for the S15B-CALIBRATION table s15i emits.
  "$AGENT" debug-enumerate "$STATE_DIR/agent.toml" cap | tee "$SCEN_DIR/enumerate.port"
  S15B_PORT_R=$(sed -n 's/.*shared_ranges=\([0-9]*\).*/\1/p' "$SCEN_DIR/enumerate.port")
  S15B_PORT_T0=$(date +%s%3N)
  "$AGENT" debug-reclaim "$STATE_DIR/agent.toml" cap | tee "$SCEN_DIR/reclaim.out"
  S15B_PORT_MS=$(( $(date +%s%3N) - S15B_PORT_T0 ))
  BYTES=$(awk '{print $2}' "$SCEN_DIR/reclaim.out")
  [ "$BYTES" -gt 0 ] || { echo "FAIL: nothing reclaimed"; exit 1; }

  # Content equivalence — live, and again after a full cache drop
  # (umount/remount): reclaimed ranges now read through the backing.
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    | diff -u "$SCEN_DIR/manifest.before" - \
    || { echo "FAIL: content changed after reclaim"; exit 1; }
  remount_fs /dev/mapper/sky-s15b-fs "$MOUNT_A" /dev/mapper/sky-s15b-rec
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    | diff -u "$SCEN_DIR/manifest.before" - \
    || { echo "FAIL: content changed after remount"; exit 1; }

  # A second reclaim finds nothing (needs the capture snapshot alive).
  "$AGENT" debug-reclaim "$STATE_DIR/agent.toml" cap | grep -q "^reclaimed 0 bytes" \
    || { echo "FAIL: reclaim not idempotent"; exit 1; }

  # §3 ordering: the pool reaches its minimum only after S is deleted —
  # the reclaim set is by definition blocks whose data blocks are SHARED
  # with S, so unmapping the origin's reference leaves S holding the
  # same allocations; `used_data` drops when the last reference goes
  # (production's step 7, delete-S-after-reclaim). Delete S, THEN assert
  # the shrink.
  dmsetup remove sky-vlin15b-c-cap
  dmsetup message sky-s15b-tpool 0 "delete 2"
  USED_AFTER=$(dmsetup status sky-s15b-tpool | awk '{print $6}' | cut -d/ -f1)
  [ "$USED_AFTER" -lt "$USED_BEFORE" ] \
    || { echo "FAIL: pool usage did not shrink (${USED_BEFORE} -> ${USED_AFTER})"; exit 1; }

  # Teardown (targeted — never remove_all on a shared host).
  umount "$MOUNT_A"; MOUNT_A=""
  dmsetup remove sky-s15b-rec
  dmsetup remove sky-s15b-fs
  dmsetup remove sky-s15b-origin
  dmsetup remove sky-s15b-tpool
  losetup -d "$BACK" "$DATA" "$META" || true
  rm -rf "$STATE_DIR"

  # Swing rows (task 15i): S15b grows the §7 major-compaction side of
  # the calibration table — a sibling scenario so it also runs alone.
  s15i

  echo "S15b PASS"
}

# S15i (Tier B, multi-node design D-M15 task 15i): the two-pool swing on
# a slot-shaped rig — ONE slot file carries pool tmeta and data, the
# agent's SlotConfig shape, so `debug-swing` can rebuild it as slot B,
# copy D behind snapshot barriers, retarget the shim inside one freeze
# window, and canonicalize. Asserts the §7 postconditions: durable flip,
# a `debug-sweep` recovery pass that no-ops on the finished swing,
# canonical-only device set, single slot file, byte-identical corpus
# (live and cold), clean fsck, a pool reset to roughly the dirty set,
# and a post-swing enumeration — the one assertion that needs a real
# kernel, since `metadata_dev` is opened by path and the canonical tmeta
# node survives the rename only if udev caught it. Emits the
# S15B-CALIBRATION table (port numbers from s15b's port block when run
# through it; "-" standalone).
s15i() {
  echo "== S15i (Tier B): dm-thin two-pool swing on a slot rig =="
  if ! reason=$(tierb_available); then echo "S15i SKIPPED: $reason"; return 0; fi
  for t in dmsetup fsfreeze mkfs.ext4 losetup blockdev fsck.ext4; do
    command -v "$t" >/dev/null || { echo "S15i SKIPPED: missing $t"; return 0; }
  done
  AGENT=./target/debug/skynbd-agent
  [ -x "$AGENT" ] || { echo "S15i SKIPPED: build with cargo build -p skynbd-agent-cli"; return 0; }
  scenario_init s15i
  STATE_DIR=/var/tmp/skynbd-agent-s15i-$$   # /var/tmp: not the frozen fs
  mkdir -p "$STATE_DIR"

  # Backing with the ext4 corpus (same recipe as S15b's): the external
  # origin BOTH slots read through — swings never touch it.
  truncate -s 256M "$SCEN_DIR/backing.img"
  BACK=$(losetup -f --show "$SCEN_DIR/backing.img")
  mkfs.ext4 -q -E lazy_itable_init=0,lazy_journal_init=0 "$BACK"
  MOUNT_A="$SCEN_DIR/mnt"; mkdir -p "$MOUNT_A"
  mount "$BACK" "$MOUNT_A"; make_corpus "$MOUNT_A"; sync; umount "$MOUNT_A"

  # The slot stack (SlotConfig's normative shape): one file → loop →
  # linear carves {pool}-tm (meta @ 0) + {pool}-da (data behind it) →
  # thin-pool → origin thin id 0 with the backing as external origin →
  # shim + port linears. Carve names are the agent's canonical
  # {pool_name}-tm/-da — the swing renames its B carves back to these.
  META_BYTES=$((16 * 1024 * 1024))
  DATA_BYTES=$((256 * 1024 * 1024))
  SLOT="$SCEN_DIR/slot.img"
  truncate -s $((META_BYTES + DATA_BYTES)) "$SLOT"
  SLOOP=$(losetup -f --show "$SLOT")
  dmsetup create sky-vlin15i-pool-tm --table \
    "0 $((META_BYTES / 512)) linear ${SLOOP} 0"
  dmsetup create sky-vlin15i-pool-da --table \
    "0 $((DATA_BYTES / 512)) linear ${SLOOP} $((META_BYTES / 512))"
  "$DD" if=/dev/zero of=/dev/mapper/sky-vlin15i-pool-tm bs=4096 count=1 conv=notrunc 2>/dev/null
  dmsetup create sky-vlin15i-pool --table \
    "0 $((DATA_BYTES / 512)) thin-pool /dev/mapper/sky-vlin15i-pool-tm /dev/mapper/sky-vlin15i-pool-da 128 32 2 skip_block_zeroing no_discard_passdown"
  dmsetup message sky-vlin15i-pool 0 "create_thin 0"
  SECT=$(blockdev --getsz "$BACK")
  dmsetup create sky-vlin15i-origin --table \
    "0 ${SECT} thin /dev/mapper/sky-vlin15i-pool 0 ${BACK}"
  dmsetup create sky-vlin15i-fs --table \
    "0 ${SECT} linear /dev/mapper/sky-vlin15i-origin 0"
  dmsetup create sky-vlin15i-rec --table \
    "0 ${SECT} linear /dev/mapper/sky-vlin15i-origin 0"
  mount /dev/mapper/sky-vlin15i-fs "$MOUNT_A"

  cat > "$STATE_DIR/agent.toml" <<EOF
server = "http://127.0.0.1:1"
state_dir = "${STATE_DIR}"

[[volume]]
name = "vlin15i"
pool_name = "sky-vlin15i-pool"
origin_name = "sky-vlin15i-origin"
origin_dev_id = 0
metadata_dev = "/dev/mapper/sky-vlin15i-pool-tm"
mountpoint = "${MOUNT_A}"
size_bytes = $((SECT * 512))
chunk_bytes = 65536
freeze_budget_ms = 30000
discard_name = "sky-vlin15i-rec"

[volume.slot]
live_file = "${SLOT}"
data_bytes = ${DATA_BYTES}
meta_bytes = ${META_BYTES}
shim_name = "sky-vlin15i-fs"
external_origin = "${BACK}"
EOF

  # Provision over the base, then capture S under agent naming.
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/f2" bs=64K count=8 conv=notrunc 2>/dev/null
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/new1" bs=64K count=16 2>/dev/null
  sync
  fsfreeze -f "$MOUNT_A"
  dmsetup suspend sky-vlin15i-origin
  dmsetup message sky-vlin15i-pool 0 "create_snap 1 0"
  dmsetup resume sky-vlin15i-origin
  fsfreeze -u "$MOUNT_A"
  dmsetup create sky-vlin15i-c-cap --uuid "SKYNBD-vlin15i-1" \
    --table "0 ${SECT} thin /dev/mapper/sky-vlin15i-pool 1"

  # Dirtied-since: rewrite part of what S captured.
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/new1" bs=64K count=4 conv=notrunc 2>/dev/null
  sync

  # "Repoint": copy S's content into the backing over every reclaimable
  # range (Invariant D's precondition), s15b's exact recipe.
  "$AGENT" debug-reclaim --dry-run "$STATE_DIR/agent.toml" cap > "$SCEN_DIR/ranges"
  [ -s "$SCEN_DIR/ranges" ] || { echo "FAIL: empty reclaim set"; exit 1; }
  while read -r _ off len; do
    "$DD" if=/dev/mapper/sky-vlin15i-c-cap of="$BACK" bs=64K \
      skip=$((off / 65536)) seek=$((off / 65536)) count=$((len / 65536)) \
      iflag=fullblock oflag=direct conv=notrunc 2>/dev/null
  done < "$SCEN_DIR/ranges"
  blockdev --flushbufs "$BACK"

  # §7 enumeration: R and D for the calibration table.
  "$AGENT" debug-enumerate "$STATE_DIR/agent.toml" cap | tee "$SCEN_DIR/enumerate.out"
  SWING_D=$(sed -n 's/.*dirty_bytes=\([0-9]*\).*/\1/p' "$SCEN_DIR/enumerate.out")
  [ -n "$SWING_D" ] || { echo "FAIL: no dirty_bytes in enumerate output"; exit 1; }

  # Live delta between the enumeration and the swing, so the copy
  # rounds converge over something real; part of the manifest, so its
  # survival is asserted with everything else.
  echo "swing-live-delta" > "$MOUNT_A/corpus/marker"
  sync
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    > "$SCEN_DIR/manifest.before"
  USED_BEFORE=$(dmsetup status sky-vlin15i-pool | awk '{print $6}' | cut -d/ -f1)

  "$AGENT" debug-swing "$STATE_DIR/agent.toml" cap | tee "$SCEN_DIR/swing.out"
  SWING_ROUNDS=$(sed -n 's/.*rounds=\([0-9]*\).*/\1/p' "$SCEN_DIR/swing.out")
  SWING_LIVE_MS=$(sed -n 's/.*live_ms=\([0-9]*\).*/\1/p' "$SCEN_DIR/swing.out")
  SWING_FROZEN_MS=$(sed -n 's/.*frozen_ms=\([0-9]*\).*/\1/p' "$SCEN_DIR/swing.out")
  [ -n "$SWING_ROUNDS" ] || { echo "FAIL: no swing stats line"; exit 1; }

  # The durable flip (the §7 crash discriminator) persisted inside the
  # freeze window.
  [ -f "$STATE_DIR/vlin15i.flip" ] || { echo "FAIL: flip marker missing"; exit 1; }

  # The swing must not leave the shim's BLOCK DEVICE frozen. It did:
  # every run that ever reached this scenario was refused here with
  # "dm-4: Can't mount, blockdev is frozen", because dm suspends with an
  # implicit lockfs and each in-window retarget took a second,
  # kernel-held freeze on top of the swing's own FIFREEZE — one of which
  # never came back.
  #
  # Asserted HERE rather than only at the scenario's end because the
  # distinction is worth keeping sharp: the freeze is a property of the
  # swing, and the sweep that follows is innocent. A regression shows up
  # as a frozen device the moment the swing returns, and a failure at
  # this line says so without implicating anything downstream.
  #
  # Note the superblock is NOT frozen when this fires — the umount inside
  # `remount_fs` succeeds, which a frozen superblock would refuse. Only
  # the block device's count is up, which is why nothing else in the
  # scenario (reads, md5sums, fsck) notices.
  remount_fs_strict /dev/mapper/sky-vlin15i-fs "$MOUNT_A" /dev/mapper/sky-vlin15i-rec \
    || { echo "FAIL: the SWING left the shim's block device frozen"; exit 1; }
  echo "checkpoint: shim remounts cleanly after the swing, on the first attempt"

  # The recovery path the marker exists for. Against a completed swing
  # the flipped sweep is a no-op (everything is already canonical), so it
  # reports found=false and clears the marker — which is also what makes
  # the next debug-swing safe to run.
  "$AGENT" debug-sweep "$STATE_DIR/agent.toml" | tee "$SCEN_DIR/sweep.out"
  grep -q "swept flipped=true found=false" "$SCEN_DIR/sweep.out" \
    || { echo "FAIL: post-swing sweep is not a no-op"; exit 1; }
  [ ! -e "$STATE_DIR/vlin15i.flip" ] \
    || { echo "FAIL: flip marker survived the sweep"; exit 1; }

  # Steady state is canonical: the shim reads through the (renamed)
  # origin, and the device set holds exactly the canonical names — no
  # -b transients, no barriers, no old-A leftovers (S's activation dies
  # with pool A; production replays delete-S tolerantly).
  ORIGIN_MM=$(dmsetup info -c --noheadings -o major,minor sky-vlin15i-origin | tr -d ' ')
  dmsetup table sky-vlin15i-fs | grep -q "linear ${ORIGIN_MM}" \
    || { echo "FAIL: shim does not reference the canonical origin"; exit 1; }
  dmsetup ls 2>/dev/null | awk '/^sky-vlin15i-/ {print $1}' | sort > "$SCEN_DIR/devs.after"
  printf '%s\n' sky-vlin15i-fs sky-vlin15i-origin sky-vlin15i-pool \
    sky-vlin15i-pool-da sky-vlin15i-pool-tm sky-vlin15i-rec | sort \
    | diff -u - "$SCEN_DIR/devs.after" \
    || { echo "FAIL: non-canonical devices after swing"; exit 1; }

  # The file swap completed: one live slot file, no .b transient.
  [ -e "$SLOT" ] || { echo "FAIL: slot.img missing after swing"; exit 1; }
  [ ! -e "$SLOT.b" ] || { echo "FAIL: slot.img.b survived the swing"; exit 1; }

  # Content equivalence — live, and cold across umount/remount.
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    | diff -u "$SCEN_DIR/manifest.before" - \
    || { echo "FAIL: content changed after swing"; exit 1; }
  remount_fs /dev/mapper/sky-vlin15i-fs "$MOUNT_A" /dev/mapper/sky-vlin15i-rec
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    | diff -u "$SCEN_DIR/manifest.before" - \
    || { echo "FAIL: content changed after remount"; exit 1; }

  # ---- the ARMED sweep, on a real kernel -----------------------------
  # `sweep_b_inner` is the crash path: a death between the retarget and
  # the flip persist leaves the shim aimed at slot B with the record
  # still naming A, AND the swing's freeze outliving the process that
  # armed it. The sweep must put the tables back, thaw, and destroy B.
  #
  # Until now nothing exercised it on a kernel. S15i's only `debug-sweep`
  # is the FLIPPED one, which reports `found=false` and does nothing —
  # so the branch that actually retargets was covered by mocks alone,
  # and a mock cannot see a block-device freeze. That is exactly the gap
  # the swing's own leak hid in.
  #
  # Reconstructed by hand, because there is no crash-injection hook:
  # build slot B under the agent's own names, aim the shim and port at
  # it, freeze the filesystem, then let the sweep repair it.
  SLOT_B="${SLOT}.b"
  truncate -s $((META_BYTES + DATA_BYTES)) "$SLOT_B"
  SLOOP_B=$(losetup -f --show "$SLOT_B")
  # NO --uuid on any of these. The swing renames its B-side devices to
  # the canonical names but they KEEP their B-side UUIDs, so after a
  # completed swing the LIVE pool already owns `SKYNBD-<vol>-slot-btm`
  # and `-slot-bda` — reusing them here is an EBUSY from dm, which is
  # what the first run of this phase hit. The sweep resolves every one of
  # these by NAME (`device_exists`, `remove_if_exists`), so a uuid would
  # buy nothing even if it were free.
  dmsetup create sky-vlin15i-pool-btm \
    --table "0 $((META_BYTES / 512)) linear ${SLOOP_B} 0"
  dmsetup create sky-vlin15i-pool-bda \
    --table "0 $((DATA_BYTES / 512)) linear ${SLOOP_B} $((META_BYTES / 512))"
  "$DD" if=/dev/zero of=/dev/mapper/sky-vlin15i-pool-btm bs=4096 count=1 conv=notrunc 2>/dev/null
  dmsetup create sky-vlin15i-pool-b \
    --table "0 $((DATA_BYTES / 512)) thin-pool /dev/mapper/sky-vlin15i-pool-btm /dev/mapper/sky-vlin15i-pool-bda 128 32 2 skip_block_zeroing no_discard_passdown"
  dmsetup message sky-vlin15i-pool-b 0 "create_thin 0"
  dmsetup create sky-vlin15i-origin-b \
    --table "0 ${SECT} thin /dev/mapper/sky-vlin15i-pool-b 0 ${BACK}"
  B_MM=$(dmsetup info -c --noheadings -o major,minor sky-vlin15i-origin-b | tr -d ' ')
  dmsetup reload sky-vlin15i-fs --table "0 ${SECT} linear ${B_MM} 0"
  dmsetup suspend sky-vlin15i-fs; dmsetup resume sky-vlin15i-fs
  dmsetup reload sky-vlin15i-rec --table "0 ${SECT} linear ${B_MM} 0"
  dmsetup suspend sky-vlin15i-rec; dmsetup resume sky-vlin15i-rec
  # The freeze the dead process left behind.
  fsfreeze -f "$MOUNT_A"
  "$AGENT" debug-sweep "$STATE_DIR/agent.toml" | tee "$SCEN_DIR/sweep.armed"
  grep -q "swept flipped=false found=true" "$SCEN_DIR/sweep.armed" \
    || { echo "FAIL: the armed sweep did not report repairing anything"; exit 1; }

  A_MM=$(dmsetup info -c --noheadings -o major,minor sky-vlin15i-origin | tr -d ' ')
  for n in sky-vlin15i-fs sky-vlin15i-rec; do
    dmsetup table "$n" | grep -q "linear ${A_MM}" \
      || { echo "FAIL: $n was not put back on slot A"; dmsetup table "$n"; exit 1; }
  done
  for gone in sky-vlin15i-origin-b sky-vlin15i-pool-b sky-vlin15i-pool-bda sky-vlin15i-pool-btm; do
    dmsetup info "$gone" >/dev/null 2>&1 \
      && { echo "FAIL: $gone survived the armed sweep"; exit 1; }
  done
  [ ! -e "$SLOT_B" ] || { echo "FAIL: slot B file survived the armed sweep"; exit 1; }

  # THE assertion this phase exists for. The sweep retargets a
  # filesystem that is already frozen, so a plain reload would take dm's
  # implicit lockfs on top of that freeze and leak the BLOCK-DEVICE
  # freeze — which `thaw_fs` cannot release, because FITHAW is
  # thaw_super, not bdev_thaw. Everything above would still pass: the
  # tables are right, the devices are gone, the file is gone, and the
  # superblock is thawed. Only a mount can tell, and only on the first
  # attempt — this property never resolves by waiting.
  remount_fs_strict /dev/mapper/sky-vlin15i-fs "$MOUNT_A" /dev/mapper/sky-vlin15i-rec \
    || { echo "FAIL: the ARMED SWEEP left the shim's block device frozen"; exit 1; }
  echo "armed sweep: repaired, thawed, and the shim still mounts"
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    | diff -u "$SCEN_DIR/manifest.before" - \
    || { echo "FAIL: content changed across the armed sweep"; exit 1; }

  # The swing IS the pool reset: slot B holds roughly the dirty set,
  # strictly less than what slot A had provisioned.
  USED_AFTER=$(dmsetup status sky-vlin15i-pool | awk '{print $6}' | cut -d/ -f1)
  [ "$USED_AFTER" -lt "$USED_BEFORE" ] \
    || { echo "FAIL: pool did not reset (${USED_BEFORE} -> ${USED_AFTER})"; exit 1; }

  # The question only a real kernel answers: `thin_mappings` opens
  # metadata_dev BY PATH, and post-swing `/dev/mapper/{pool}-tm` is a
  # node the canonicalizing rename left to udev — which the dm layer's
  # own contract notes say never to assume (raw ioctls make no mapper
  # nodes). The mock cannot see this at all (it ignores metadata_dev),
  # so a post-swing enumeration here is the only place it is settled.
  # If this fails, the fix is for the swing to re-create or verify the
  # mapper node after the rename.
  fsfreeze -f "$MOUNT_A"
  dmsetup suspend sky-vlin15i-origin
  dmsetup message sky-vlin15i-pool 0 "create_snap 2 0"
  dmsetup resume sky-vlin15i-origin
  fsfreeze -u "$MOUNT_A"
  dmsetup create sky-vlin15i-c-cap2 --uuid "SKYNBD-vlin15i-2" \
    --table "0 ${SECT} thin /dev/mapper/sky-vlin15i-pool 2"
  "$AGENT" debug-enumerate "$STATE_DIR/agent.toml" cap2 \
    | tee "$SCEN_DIR/enumerate.after" \
    || { echo "FAIL: post-swing enumerate (metadata_dev path after the rename)"; exit 1; }
  grep -q "dirty_bytes=" "$SCEN_DIR/enumerate.after" \
    || { echo "FAIL: post-swing enumerate produced no sets"; exit 1; }
  dmsetup remove sky-vlin15i-c-cap2
  dmsetup message sky-vlin15i-pool 0 "delete 2"

  umount "$MOUNT_A"; MOUNT_A=""
  fsck.ext4 -f -n /dev/mapper/sky-vlin15i-fs \
    || { echo "FAIL: fsck after swing"; exit 1; }

  # The §7 timing table (design D-M15: reclaimRangeThreshold is
  # calibrated from these rows; port numbers come from s15b's port
  # block when run through it).
  echo "S15B-CALIBRATION: port_R=${S15B_PORT_R:--} port_ms=${S15B_PORT_MS:--} swing_D_bytes=${SWING_D} swing_live_ms=${SWING_LIVE_MS} swing_frozen_ms=${SWING_FROZEN_MS} swing_rounds=${SWING_ROUNDS}"

  # Teardown (targeted). The swing replaced the slot loop: the live
  # file's CURRENT loop is the one to detach, not the build-time one.
  dmsetup remove sky-vlin15i-rec
  dmsetup remove sky-vlin15i-fs
  dmsetup remove sky-vlin15i-origin
  dmsetup remove sky-vlin15i-pool
  dmsetup remove sky-vlin15i-pool-da
  dmsetup remove sky-vlin15i-pool-tm
  SLOT_LOOP=$(losetup -j "$SLOT" | cut -d: -f1)
  [ -n "$SLOT_LOOP" ] || { echo "FAIL: no loop backs $SLOT after swing"; exit 1; }
  losetup -d "$SLOT_LOOP"
  losetup -d "$BACK" || true
  rm -rf "$STATE_DIR"
  echo "S15i PASS"
}


# Wait for a pattern to appear N times. `wait_log` answers "has this
# happened", which is the wrong question when the assertion is about
# what a REPEAT does — the first occurrence is already there, so the
# wait returns instantly and the sample lands before the repetition it
# was meant to bracket.
wait_count() { # $1 = file, $2 = pattern, $3 = n, $4 = timeout_s, $5 = label
  local f=$1 pat=$2 n=$3 deadline=$(( $(date +%s) + $4 )) label=$5
  while [ "$(date +%s)" -lt "$deadline" ]; do
    # No `|| echo 0`: `grep -c` PRINTS 0 and then EXITS 1 on no match, so
    # the fallback appends a second line and `[` gets "0\n0" —
    # "integer expression expected", every 0.5 s for the whole timeout.
    # The `-f` test already covers the file not existing yet, which is
    # the case the fallback looked like it was for.
    if [ -f "$f" ] && [ "$(grep -c "$pat" "$f" 2>/dev/null)" -ge "$n" ]; then
      return 0
    fi
    sleep 0.5
  done
  echo "FAIL: timed out after $4s waiting for $label"
  echo "--- tail of $f ---"; tail -40 "$f" 2>/dev/null
  return 1
}

# Wait for a line to appear in a growing log. Returns non-zero on timeout
# so callers can print their own diagnosis; never greps a file that may
# not exist yet (the agent creates it asynchronously).
wait_log() { # $1 = file, $2 = pattern, $3 = timeout_s, $4 = label
  local f=$1 pat=$2 deadline=$(( $(date +%s) + $3 )) label=$4
  while [ "$(date +%s)" -lt "$deadline" ]; do
    [ -f "$f" ] && grep -q "$pat" "$f" && return 0
    sleep 0.5
  done
  echo "FAIL: timed out after $3s waiting for $label"
  echo "--- tail of $f ---"; tail -40 "$f" 2>/dev/null
  return 1
}

# Sectors read from a block device since boot (/sys/block/<dev>/stat
# field 3). The origin-read counter for S15j's zero-read assertion.
read_sectors() { # $1 = block device (S15j passes the origin export's /dev/nbdN)
  awk '{print $3}' "/sys/block/$(basename "$1")/stat"
}

# Sectors read from a dm device, by major:minor — `read_sectors` takes a
# /dev path and basenames it, which resolves to the dm NAME rather than
# the dm-N that /sys/block is keyed on.
dm_read_sectors() { # $1 = dm name
  local mm
  mm=$(dmsetup info -c --noheadings -o major,minor "$1" 2>/dev/null | tr -d ' ')
  [ -n "$mm" ] && awk '{print $3}' "/sys/dev/block/${mm}/stat" 2>/dev/null || echo 0
}

# Used blocks in a thin pool, raw. `pool_pct` rounds to a percent, which
# cannot resolve the ~17 blocks this correlation is about.
pool_used() { # $1 = pool name -> used data blocks
  dmsetup status "$1" 2>/dev/null | awk '{split($6,u,"/"); print u[1]+0}'
}

pool_pct() { # $1 = pool name -> integer percent used
  dmsetup status "$1" | awk '{split($6,u,"/"); printf "%d", (u[1]*100)/u[2]}'
}

# Why an agent went quiet.
#
# Called when a wait for a cycle OUTCOME times out, which is the one
# failure shape the log cannot explain by itself: "nothing was logged"
# reads identically whether the agent never attempted the call, attempted
# it and is blocked in the kernel, or attempted it and is waiting on a
# reply that will never arrive. Each line below picks exactly one of
# those apart, and the middle one is the reason this exists at all.
#
#   netcut.log — did a connection even ARRIVE during the cut? If not,
#                the agent never got as far as the control plane and the
#                stall is local (a freeze, a dm suspend, a device read).
#   ss         — the discriminator that matters. `read_timeout` bounds
#                the wait for a RESPONSE and nothing else; it does not
#                bound a request body that cannot be flushed. Those two
#                look the same from userspace and opposite here: a
#                stalled upload shows a backed-up Send-Q, a stalled
#                response shows Send-Q 0 with the request long gone.
#   wchan      — what the kernel has each thread waiting on. A thread
#                parked in a socket write and a thread parked in
#                `freeze_super` are not confusable at this level.
cut_diag() { # $1 = agent pid, $2 = cut port, $3 = netcut log
  set +e
  echo "--- netcut.log ---"
  tail -30 "$3" 2>/dev/null || echo "(no netcut log)"
  echo "--- sockets to the cut port (Send-Q backed up => stalled upload) ---"
  ss -tnpo 2>/dev/null | grep -E "State|:$2" || echo "(no socket to :$2 at all)"
  echo "--- agent threads (tid comm wchan) ---"
  for t in /proc/"$1"/task/*; do
    printf '%s %s %s\n' "${t##*/}" \
      "$(cat "$t/comm" 2>/dev/null)" "$(cat "$t/wchan" 2>/dev/null)"
  done
  echo "--- dmesg tail ---"
  dmesg 2>/dev/null | tail -20
  set -e
}

# S15j (Tier B, origin-advance design §8.2): outage conformance — the
# whole degradation-and-recovery runbook walked end to end against a real
# dm-thin stack and a real control plane:
#
#   cut network -> fill pool -> clean RO -> reconnect
#     -> backlog publish with ZERO origin reads -> advance -> remount
#
# The cut is the reason this scenario could not be written before: the
# harness had exactly one way to be "unreachable" — point the agent at a
# dead port — and connection-REFUSED fails instantly, which is the one
# partition shape that never exercises the ladder. §8.2 is written about
# the other shape: accepted and silent. `netcut.py` supplies it, and the
# ladder is only reachable because a silent server now *fails* a cycle
# rather than hanging one forever (measured here: 60 s, the agent's
# READ_TIMEOUT; before that fix this call never returned at all, so
# `failing_since` was never set and rule 3 could not fire in the outage
# it exists for).
#
# What the cut does and does not model, stated because it bounds the
# claim: it partitions the CONTROL PLANE, which is what the failing
# clock, the publish and the repoint all ride. §8.2's other half — cold
# reads hanging on the origin attach — is deliberately NOT modelled. The
# origin export stays reachable on its own NBD connection throughout, so
# reads through it succeed rather than hang, which is what keeps "zero
# origin reads" a statement about the recovery's behaviour instead of an
# artefact of a link that could not have served a read anyway.
s15j() {
  echo "== S15j (Tier B): §8.2 outage conformance — cut, degrade, recover =="
  if ! reason=$(tierb_available); then echo "S15j SKIPPED: $reason"; return 0; fi
  # blktrace/blkparse are load-bearing, not optional: §8.2 step 4 is
  # asserted on per-issuer attribution now, because the raw device
  # counter cannot distinguish the agent from udev's blkid probe and
  # that probe lands in a different phase every run. Listed here with
  # the rest so a host without them SKIPS the scenario loudly rather
  # than running it with its central assertion quietly vacuous.
  for t in dmsetup fsfreeze mkfs.ext4 losetup blockdev blktrace blkparse; do
    command -v "$t" >/dev/null || { echo "S15j SKIPPED: missing $t"; return 0; }
  done
  # debugfs belongs in the SKIP preamble with the binaries, not further
  # down as a tracer precondition. Without it the tracer took its `else`
  # branch, printed a note, ran the entire ladder, and only then hard
  # failed on a missing fence marker — a skip promised in one place and
  # a failure delivered in another, after the full runtime. Moot on the
  # nightly, where blktrace is installed and debugfs is mounted; it
  # bites whoever runs this locally.
  [ -d /sys/kernel/debug/block ] \
    || { echo "S15j SKIPPED: /sys/kernel/debug/block absent (mount debugfs: \
sudo mount -t debugfs none /sys/kernel/debug)"; return 0; }
  AGENT=./target/debug/skynbd-agent
  [ -x "$AGENT" ] || { echo "S15j SKIPPED: build with cargo build -p skynbd-agent-cli"; return 0; }
  need curl python3
  scenario_init s15j
  STATE_DIR=/var/tmp/skynbd-agent-s15j-$$   # /var/tmp: not the frozen fs
  mkdir -p "$STATE_DIR"
  CUT_FILE="$SCEN_DIR/cut"
  CUT_PORT=$((API_PORT + 2))

  # Daemon: the publish target. The agent reaches it only through the
  # cuttable link, never directly — a scenario that talked to the real
  # port anywhere would silently un-cut itself.
  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"

[store]
kind = "memory"

[[volumes]]
name = "vlin15j"
size = 268435456
kind = "lineage"
EOF
  start_daemon
  # This scenario asserts its way through a ladder and can bail at any
  # rung with the filesystem still mounted; without this the EXIT path
  # would try to `rm -rf` a live mount.
  trap 'rc=$?; tierb_cleanup; cleanup' EXIT
  python3 tests/e2e/netcut.py --listen "$CUT_PORT" --to "127.0.0.1:${API_PORT}" \
    --cut-file "$CUT_FILE" > "$SCEN_DIR/netcut.log" 2>&1 &
  NETCUT_PID=$!
  for _ in $(seq 1 50); do
    curl -sf "http://127.0.0.1:${CUT_PORT}/healthz" >/dev/null 2>&1 && break
    sleep 0.2
  done
  curl -sf "http://127.0.0.1:${CUT_PORT}/healthz" >/dev/null \
    || { echo "FAIL: the cuttable link never carried a request"; exit 1; }

  # The stack. The external origin is deliberately NOT a local loop
  # holding a copy of the base — it is the server's own repointable
  # origin export, attached over kernel NBD.
  #
  # That is not a fidelity nicety, it is what makes the scenario
  # CORRECT. The recovery advance repoints and then reclaims, unmapping
  # every block it believes the origin now serves identically. With a
  # loop backing frozen at the base (S15b's stand-in, fine there because
  # S15b copies the published content in by hand before reclaiming),
  # those ranges would read back pre-outage bytes and the delta this
  # scenario exists to protect would be destroyed by its own recovery.
  # Pointing the origin at the generation the agent just published is
  # the only arrangement where the reclaim is content-preserving.
  VOL_BYTES=268435456
  SECT=$((VOL_BYTES / 512))
  truncate -s 128M "$SCEN_DIR/data.img"   # small pool: it has to be fillable
  truncate -s 16M  "$SCEN_DIR/tmeta.img"
  DATA=$(losetup -f --show "$SCEN_DIR/data.img")
  META=$(losetup -f --show "$SCEN_DIR/tmeta.img")
  "$DD" if=/dev/zero of="$META" bs=4096 count=1 conv=notrunc 2>/dev/null

  dmsetup create sky-s15j-tpool --table \
    "0 $(blockdev --getsz "$DATA") thin-pool ${META} ${DATA} 128 32 2 skip_block_zeroing no_discard_passdown"
  dmsetup message sky-s15j-tpool 0 "create_thin 1"
  # No external origin yet: the base has to BE somewhere before it can be
  # published, so it starts fully mapped in the pool and the origin is
  # attached below, once a generation exists for it to serve.
  dmsetup create sky-s15j-origin --table \
    "0 ${SECT} thin /dev/mapper/sky-s15j-tpool 1"
  # D-M15 shim + port shape (see S15b): the fs claims the shim, the port
  # is the unclaimed node reclaim discards through.
  dmsetup create sky-s15j-fs  --table "0 ${SECT} linear /dev/mapper/sky-s15j-origin 0"
  dmsetup create sky-s15j-rec --table "0 ${SECT} linear /dev/mapper/sky-s15j-origin 0"
  MOUNT_A="$SCEN_DIR/mnt"; mkdir -p "$MOUNT_A"
  mkfs.ext4 -q -E lazy_itable_init=0,lazy_journal_init=0 /dev/mapper/sky-s15j-fs
  mount /dev/mapper/sky-s15j-fs "$MOUNT_A"
  make_corpus "$MOUNT_A"; sync

  # Dials, all five of them on the config-file surface (the agent parses
  # these only since the §4/§8 config-file plumbing landed):
  #   rule 3 at 60% + 5 s   — the clean-RO rung
  #   advance at 50%        — so the recovery cycle is an ADVANCE, which
  #                           is what returns pool blocks and licenses
  #                           the remount (§8.2 steps 5-6)
  # interval_s is short so the failing clock advances in cycles, not
  # minutes; each cut cycle still costs the full 60 s read timeout.
  cat > "$STATE_DIR/agent.toml" <<EOF
server = "http://127.0.0.1:${CUT_PORT}"
state_dir = "${STATE_DIR}"

[[volume]]
name = "vlin15j"
pool_name = "sky-s15j-tpool"
origin_name = "sky-s15j-origin"
origin_dev_id = 1
metadata_dev = "${META}"
mountpoint = "${MOUNT_A}"
size_bytes = $((SECT * 512))
chunk_bytes = 65536
interval_s = 5
freeze_budget_ms = 30000
discard_name = "sky-s15j-rec"
shim_name = "sky-s15j-fs"
advance_watermark = 0.50
publish_failing_ro_watermark = 0.60
publish_failing_ro_seconds = 5
EOF

  # Bootstrap with the link UP: the first cycle is a full objectize, so
  # after it the server holds the whole base as a generation.
  "$AGENT" once "$STATE_DIR/agent.toml" 2>&1 | tee "$SCEN_DIR/bootstrap.log"
  grep -q "published" "$SCEN_DIR/bootstrap.log" \
    || { echo "FAIL: bootstrap did not publish"; exit 1; }

  # Point the origin export at that generation (an empty POST body
  # initializes the pointer — design D-M15 Invariant D), then attach the
  # export and give it to the thin device as its external origin. The
  # reload is production's own shape: `thin <pool> <dev_id>
  # <external_origin>` loaded over a suspended device. Every block is
  # mapped locally right now, so nothing reads through it yet; it starts
  # mattering the moment the recovery advance unmaps something.
  # No `-f`: it suppresses the body on an HTTP error, which is exactly
  # the body worth printing — "no published generations to initialize
  # from" is the message that would explain this failing.
  ORIGIN_HTTP=$(curl -s -o "$SCEN_DIR/origin.json" -w '%{http_code}' \
    -X POST "http://127.0.0.1:${CUT_PORT}/v1/volumes/vlin15j/origin" \
    -H 'content-type: application/json' -d '{}')
  [ "$ORIGIN_HTTP" = 200 ] || {
    echo "FAIL: could not initialize the origin pointer (HTTP ${ORIGIN_HTTP})"
    cat "$SCEN_DIR/origin.json" 2>/dev/null; echo; exit 1; }
  nbd_attach vlin15j/origin /dev/nbd0
  ORIGIN_DEV=/dev/nbd0
  [ "$(blockdev --getsz "$ORIGIN_DEV")" -ge "$SECT" ] \
    || { echo "FAIL: origin export is smaller than the thin device"; exit 1; }
  dmsetup suspend sky-s15j-origin
  dmsetup reload sky-s15j-origin --table \
    "0 ${SECT} thin /dev/mapper/sky-s15j-tpool 1 ${ORIGIN_DEV}"
  dmsetup resume sky-s15j-origin

  # ---- 1. A healthy agent, and THEN the cut --------------------------
  # The order is load-bearing, not cosmetic. `run` recovers before it
  # loops and exits(70) if that fails, so an agent started *into* a
  # partition dies at startup instead of walking the ladder — and §8.2
  # is a story about an outage arriving at a running agent anyway, not
  # about booting into one. One clean cycle is the synchronisation
  # point: past it the loop is live and the clock is provably clear.
  # Debug logging on the agent, because at INFO this loop emits exactly
  # one line per cycle and nothing at all while one is in flight — so a
  # cycle that does not finish produces a log identical to an agent that
  # never woke up. That ambiguity is what made the first three failures
  # unreadable from the outside: `ss` could say the socket was idle and
  # `wchan` could say the runtime was in `ep_poll`, and neither could say
  # WHICH call was outstanding. hyper/reqwest at debug name the request.
  #
  # `hyper_util::…::pool` stays at INFO. At debug it emits a line per
  # connection checkout AND per return — ~60 of them for a single
  # advance cycle — which filled the nightly collector's `tail -80` of
  # this file with pure churn on the very run that added the debug
  # logging, burying the assertion that actually failed. The lines that
  # name a call are in `reqwest::connect` and
  # `hyper_util::…::connect::http`, and those stay.
  RUST_LOG="${RUST_LOG:-skynbd_agent=debug,skynbd_agent_cli=debug,reqwest=debug,hyper=debug,hyper_util::client::legacy::pool=info}" \
    "$AGENT" run "$STATE_DIR/agent.toml" > "$SCEN_DIR/agent.log" 2>&1 &
  AGENT_PID=$!
  wait_log "$SCEN_DIR/agent.log" "cycle published" 90 "the agent's first healthy cycle" || exit 1
  touch "$CUT_FILE"

  # `touch` arms the cut; it does not APPLY it. netcut polls the flag
  # every 50 ms, so for up to one interval after the touch the link is
  # still relaying — and a localhost request completes inside that
  # window comfortably. The first version of the probe below raced it
  # exactly this way and reported `ok`, which read as "the cut is
  # broken" when the cut simply had not happened yet.
  #
  # Wait for netcut to say so itself. Everything downstream assumes the
  # partition is in effect from this line on, so the barrier belongs
  # here rather than inside the probe: the agent's next cycle is 5 s
  # out and would never have noticed, but nothing should have to reason
  # about which side of a 50 ms window it lands on.
  wait_log "$SCEN_DIR/netcut.log" "netcut: CUT" 10 "netcut to apply the cut" || exit 1

  # Now the cut is real, confirm it cuts the way §8.2 requires. Costs
  # one connection and removes an entire hypothesis class from every
  # failure below: if this says `ok`, netcut is relaying and the silence
  # downstream is the harness's fault rather than the agent's. `silent`
  # is the shape §8.2 is written about — accepted, and never answered —
  # and it is what distinguishes this from `refused`, which every
  # earlier scenario meant by "unreachable" and which never exercises
  # the ladder at all.
  CUT_STATE=$(python3 - "$CUT_PORT" <<'PY'
import socket, sys
try:
    s = socket.create_connection(("127.0.0.1", int(sys.argv[1])), timeout=5)
    s.settimeout(5)
    s.sendall(b"GET /v1/volumes HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
    print("ok" if s.recv(1) else "eof")
except socket.timeout:
    print("silent")
except OSError as e:
    print(f"refused:{e.errno}")
PY
)
  echo "cut probe: $CUT_STATE"
  [ "$CUT_STATE" = "silent" ] \
    || { echo "FAIL: the cut is not cutting (probe answered '$CUT_STATE')"; exit 1; }

  # ---- 2. Fill the pool past rule 3's watermark ----------------------
  # Past the watermark, deliberately NOT to exhaustion: rule 3 exists to
  # act BEFORE the pool runs dry, and the clean-RO assertion below is
  # only meaningful on a filesystem that never hit ENOSPC.
  OUTAGE_DATA="$MOUNT_A/corpus/outage"
  mkdir -p "$OUTAGE_DATA"
  # A dd can die mid-file if rule 3 fires during the fill (the pool
  # crosses the watermark and the clock has already run out). That is a
  # legitimate outcome, not a failure — but a half-written file would
  # make the manifest below depend on page-cache timing, so drop it.
  # Bounded well above what the arithmetic needs (the pool is 128 MiB and
  # mkfs + corpus have already taken a slice of it), because the loop
  # exits on the watermark, not on the count.
  for i in $(seq 1 100); do
    if ! "$DD" if=/dev/urandom of="$OUTAGE_DATA/w$i" bs=1M count=2 2>/dev/null; then
      rm -f "$OUTAGE_DATA/w$i" 2>/dev/null || true
      break
    fi
    sync
    [ "$(pool_pct sky-s15j-tpool)" -ge 65 ] && break
  done
  sync
  PCT=$(pool_pct sky-s15j-tpool)
  echo "pool after outage writes: ${PCT}%"
  [ "$PCT" -ge 60 ] || { echo "FAIL: pool only reached ${PCT}%, below rule 3's watermark"; exit 1; }
  [ "$PCT" -lt 100 ] || { echo "FAIL: pool exhausted — the fill overshot the rung under test"; exit 1; }
  # The acked application data the outage produced: §8.2's "not garbage".
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    > "$SCEN_DIR/manifest.outage"

  # ---- 3. A cut cycle FAILS rather than hanging ----------------------
  # The property the whole ladder rests on: under a partition a cycle
  # must FAIL, because rule 3's clock is driven by failures and a cycle
  # that hangs never sets `failing_since`.
  #
  # The budget was 150 s on the theory that a cycle costs one 60 s read
  # timeout plus a cycle boundary. That theory is wrong, and it is the
  # arithmetic rather than the product that was: READ_TIMEOUT is a
  # PER-REQUEST deadline (armed at dispatch, never reset — see
  # `http.rs`), and a cycle makes several control-plane calls in
  # sequence. So a cut cycle's time-to-failure is a MULTIPLE of 60 s,
  # not 60 s, and 150 s could cut the very first one off mid-ladder.
  # 300 s covers four such calls with room for a cycle boundary.
  #
  # Deliberately still bounded, and generously rather than infinitely:
  # the assertion is that the cycle fails at all, and an unbounded wait
  # would turn the hang this exists to catch into a hung job.
  wait_log "$SCEN_DIR/agent.log" "cycle failed" 300 "a cut cycle to fail (not hang)" \
    || { cut_diag "$AGENT_PID" "$CUT_PORT" "$SCEN_DIR/netcut.log"; exit 1; }
  echo "cut cycle failed as required — the failing clock is running"

  # The candidate the cut cycle stranded, identified by its THIN DEV ID.
  #
  # `resume_from_captured` dies at `cp.prepare`, so a failed cut cycle
  # leaves `phase = Captured, candidate = Some(..), prepared = None` and
  # the candidate activation behind. The next cycle must RESUME from it,
  # not capture again — and the dev id is how that is visible from
  # outside the agent: `dmsetup table` on a thin reports
  # `0 <sectors> thin <pool> <dev_id>`, and a fresh capture allocates a
  # new one. This is exactly the counter that was seen climbing 48, 49,
  # 50, 51, 52 while the name stayed fixed, back when the collision was
  # a livelock.
  #
  # Sampled here and again after rule 3 has fired, which is at least one
  # further cycle later, so the window genuinely contains a re-entry.
  cand_dev_id() { # -> thin dev id of the candidate activation, or empty
    local n
    n=$(dmsetup ls 2>/dev/null | awk '/^sky-vlin15j-c-/ {print $1; exit}')
    [ -n "$n" ] || return 0
    dmsetup table "$n" 2>/dev/null | awk '$3=="thin" {print $5}'
  }
  CAND_ID_FIRST=$(cand_dev_id)

  # ---- 4. Rule 3 takes the filesystem READ-ONLY, cleanly -------------
  wait_log "$SCEN_DIR/agent.log" "taking the filesystem READ-ONLY" 120 "rule 3 to fire" || exit 1
  grep -q " $MOUNT_A .*[( ]ro[,)]" /proc/mounts \
    || { echo "FAIL: rule 3 logged but the mount is not ro"; grep " $MOUNT_A " /proc/mounts; exit 1; }
  if "$DD" if=/dev/urandom of="$MOUNT_A/corpus/should-fail" bs=4k count=1 2>/dev/null; then
    echo "FAIL: filesystem still accepted a write after rule 3"; exit 1
  fi
  # CLEAN read-only, not an aborted super — and this is the discriminator
  # §8.2 step 3 turns on: `freeze_super` short-circuits on `sb_rdonly` and
  # returns 0, while an ABORTED superblock answers EROFS. A successful
  # freeze here is therefore positive evidence that rule 3 got there
  # first and the volume still has a consistent capture available.
  # The agent is still cycling underneath this, and its capture freezes
  # the same filesystem — so this probe can lose a race with it. EBUSY is
  # exactly that race (which is why §8.2 step 3 refuses to read EBUSY as
  # a wedged filesystem), so retry it; any OTHER errno is the verdict.
  FROZE=""
  for _ in $(seq 1 10); do
    if err=$(fsfreeze -f "$MOUNT_A" 2>&1); then FROZE=yes; break; fi
    case "$err" in
      *[Bb]usy*) sleep 2 ;;
      *) echo "FAIL: fsfreeze refused ($err) — the fs is aborted, not cleanly read-only"; exit 1 ;;
    esac
  done
  [ -n "$FROZE" ] || { echo "FAIL: never won the freeze race against the agent in 20s"; exit 1; }
  fsfreeze -u "$MOUNT_A"
  grep -q " $MOUNT_A .*abort" /proc/mounts \
    && { echo "FAIL: superblock aborted — rule 3 did not get there first"; exit 1; }
  echo "clean read-only confirmed (freezes cleanly, no abort)"

  # THE OUTAGE MUST NOT KEEP RE-CAPTURING. At least one more cycle has
  # failed since the first sample — rule 3 evaluates on a cycle, and the
  # first failure is what started its clock — so if the loop were still
  # starting at capture every interval, the dev id would have moved.
  #
  # This is the kernel-level half of the engine tests in
  # `outage_degradation.rs`. It is worth having separately because the
  # resume's precondition is dm-specific: the agent decides from
  # `stack.observe()`, which on `DmStack` means `list_devices()` plus
  # `parse_name()` classifying `sky-<vol>-c-<logical>` as a candidate,
  # where `FakeStack` just reads a role field it was handed. If that
  # classification ever stopped holding for a real stranded candidate,
  # the agent would fall back to capturing, the supersede would absorb
  # the collision, every engine test would still pass, and production
  # would go back to paying a FIFREEZE on a live guest filesystem once
  # per cycle for the whole outage. Silent and green is the failure this
  # assertion exists to make loud.
  # A FULL second cycle must complete before the second sample, or the
  # check is vacuous — and it would have been. `run_cycle` evaluates
  # rule 3 at the TOP, "before anything that can block", so the
  # READ-ONLY line is written BEFORE that same cycle reaches its
  # capture. Sampling on the READ-ONLY line therefore compares the
  # stranded candidate against itself and passes whatever the agent
  # does. Waiting for the second `cycle failed` is what makes the
  # window actually contain a re-entry.
  #
  # It costs a cut cycle of wall clock (~180 s: READ_TIMEOUT is per
  # request and a cycle attempts several). Worth it — the failure this
  # guards against is silent, so a cheap probabilistic version of it
  # would be worth roughly nothing.
  wait_count "$SCEN_DIR/agent.log" "cycle failed" 2 400 \
    "a second cut cycle to complete, so the sample brackets a re-entry" || exit 1
  CAND_ID_LATER=$(cand_dev_id)
  echo "stranded candidate thin dev id: first ${CAND_ID_FIRST:-none}, \
after a second cut cycle ${CAND_ID_LATER:-none}"
  [ -n "$CAND_ID_FIRST" ] \
    || { echo "FAIL: the cut cycle left no candidate activation — the \
scenario cannot tell whether the recovery resumes or re-captures"; exit 1; }
  [ "$CAND_ID_FIRST" = "$CAND_ID_LATER" ] \
    || { echo "FAIL: the outage re-captured — candidate thin dev id moved \
${CAND_ID_FIRST} -> ${CAND_ID_LATER:-none}. Each re-capture is a fresh \
FIFREEZE on a live guest filesystem for a snapshot the next cycle \
discards; the cycle should have resumed from the stranded candidate"; exit 1; }

  # ---- 5. Reconnect; the recovery cycle reads NOTHING from the origin -
  # "Zero origin reads by construction" (§8.2 step 4): the backlog is the
  # locally mapped set, so nothing in the recovery needs the base.
  #
  # The bracket ends at the REPOINT, and the earlier version of this was
  # wrong to run it through step 6's remount. It tripped at 2256 sectors
  # on the first run that got this far, and the comment here had already
  # promised that the bracket — not the claim — is what to narrow.
  #
  # Narrowed because the wide version asserted something the design
  # deliberately does NOT hold. Step 4's claim is about the PUBLISH: the
  # backlog comes out of the local pool, so producing it must not need
  # the origin. After the repoint that stops being true by construction
  # — the origin now serves the ranges the reclaim just unmapped, so
  # step 6's read-write remount reads filesystem metadata straight
  # through it. Those reads are the advance working, not a violation of
  # it, and folding them into a "stronger claim in the useful direction"
  # was an over-claim rather than a stricter test.
  #
  # Both numbers are still reported, so a regression in the second phase
  # is visible even though only the first is asserted.
  #
  # Sampled before any verification read: the corpus check further down
  # legitimately reads the origin, which is the entire point of reclaim.
  # Count publishes so far, so the fence below can spot the recovery's
  # own without hard-coding a generation number.
  # WHO issues each read, at PID and LBA granularity.
  #
  # THE READER IS UDEV. Nine mechanisms were proposed for the ~2150
  # sectors this device is read during the recovery; the first run of
  # this tracer answered it outright:
  #
  #   (udev-worker)[9339]: 2112 sectors in 49 reqs
  #   skynbd-agent[7934]:    40 sectors in  5 reqs
  #   offsets: 0+8 32+8 64+8 128+8 256+8 512+8 1024+8 2048+8 4096+8
  #            8192+8 524160+8 523888+8 523696+8 ...
  #
  # A doubling walk from sector 0 across every offset a filesystem
  # superblock magic can live at, then the tail of the device for backup
  # superblocks and the secondary GPT header. That is `blkid`, and the
  # recovery triggers it because the repoint and the reclaim are each a
  # dm table change and udev answers a table change by re-probing.
  #
  # Hypothesis 6 was therefore RIGHT and its test was WRONG. The
  # "udev probe cost: 0 sectors" measurement further down asks udev for a
  # re-probe with `udevadm trigger --action=change`, which re-runs the
  # rules but serves blkid from udev's own database — nothing re-reads
  # the device, so a correct hypothesis was retired by a broken
  # instrument. Left in place, annotated, because a diagnostic that
  # reliably reads zero is worth more as a documented trap than deleted.
  #
  # Started BEFORE the baseline counters below, so the calibration read
  # is outside every bracket the assertions use.
  #
  # RECORDED NOW, PARSED LATER — deliberately not `blktrace -o - |
  # blkparse -i -`.
  #
  # Live-piped, blkparse timestamps events before it has established the
  # trace genesis, and the FIRST event comes out unnormalized. A run
  # caught it exactly there: the fences read
  #   start=1.26687e+09 capture=0.0482509 publish=0.384841 ...
  # where 1.26687e+09 is nanoseconds — 1.267 s, precisely when the start
  # marker fires (tracer start plus the settle below) — while every
  # later fence is relative seconds. One raw timestamp, the rest
  # normalized, in the same stream.
  #
  # Recording to per-CPU files and parsing after the tracer stops gives
  # blkparse the whole trace before it prints any of it, so the genesis
  # is known and every timestamp is relative to the same origin. This
  # removes the class rather than the instance.
  #
  # Diagnostics, never a gate. If blktrace is absent or debugfs is not
  # mounted the scenario behaves exactly as before. The format is pinned
  # so the awk that reads it cannot drift with a blkparse default:
  #   %T sec, %t nsec, %a action, %d RWBS, %S start sector, %n sectors,
  #   %p pid, %C comm. Seconds and nanoseconds stay SEPARATE fields —
  #   "%T.%9t" looks like one decimal but pads with spaces, not zeros,
  #   so a sub-100ms timestamp would silently split into two columns.
  if command -v blktrace >/dev/null 2>&1 \
     && command -v blkparse >/dev/null 2>&1 \
     && [ -d /sys/kernel/debug/block ]; then
    ( { set +x; } 2>/dev/null
      exec blktrace -d /dev/mapper/sky-s15j-origin -D "$SCEN_DIR" -o bt ) \
      > "$SCEN_DIR/origin-thin.bt.err" 2>&1 &
    BT_PID=$!
    # blktrace forks, opens per-CPU debugfs buffers and arms the
    # tracepoints; a read issued before that lands in no trace at all.
    # One second is far more than it needs, happens once, during the
    # outage, with nothing waiting on it.
    sleep 1
  else
    echo "note: blktrace unavailable — origin-thin read attribution skipped"
  fi

  # PHASE BOUNDARIES AS TRACE EVENTS, not as converted wall clock.
  #
  # Three separate bugs came out of trying to reconcile blkparse's clock
  # with `date`, and the third one produced a FAILING assertion that
  # looked exactly like a product defect: every event in the run piled
  # into the upload window while `repoint+reclaim` and `remount` came
  # back empty — on a run whose remount demonstrably happened, since the
  # scenario only reaches that assertion after the agent logs
  # `filesystem back READ-WRITE`. The agent's 40 sectors moved in
  # lockstep with udev's 2016, which two independent activities do not
  # do. It was the offset, again.
  #
  # blkparse's %T/%t are relative to the trace's own origin, which
  # nothing outside the trace can observe. So stop trying to observe it:
  # drop a distinctive read at each fence and read the boundaries back
  # OUT of the trace, in the trace's own clock. There is no conversion
  # left to get wrong, and a missing marker is detectable rather than
  # silently wrong.
  #
  # Mid-device sectors, 8 apart, where no blkid probe goes — blkid walks
  # the head (0, 32, 64, ... 8192) and the tail, never the middle. Each
  # marker is one O_DIRECT read of ONE LOGICAL BLOCK, so each PRINTED raw
  # bracket below carries exactly one extra block. Those counters are
  # printed and never asserted, and the markers come from a separate
  # process, so agent attribution — which is what IS asserted — cannot
  # see them at all.
  #
  # Not `dd`. The ubuntu-26.04 runners ship the Rust coreutils, whose
  # `dd iflag=direct` fails with "IO error: Invalid input" (its buffer
  # is not aligned the way O_DIRECT requires). The error went to
  # /dev/null, so every marker silently went missing while the trace
  # itself was fine, and the check below failed on fences that were
  # never dropped. The read is now a few lines of Python into an mmap
  # buffer (page-aligned by construction), sized to the device's logical
  # block, and a marker read that fails says so rather than being
  # discovered as an absence two hundred lines later.
  BT_MARK_START=100000
  BT_MARK_PUBLISH=100016
  BT_MARK_REPOINT=100024
  BT_MARK_AFTER=100032
  BT_LBS=$(blockdev --getss /dev/mapper/sky-s15j-origin)
  echo "origin-thin logical block size: ${BT_LBS} bytes"
  bt_mark() { # $1 = sector; a no-op without a tracer
    [ -n "$BT_PID" ] || return 0
    if ! python3 - /dev/mapper/sky-s15j-origin "$1" "$BT_LBS" \
         2>"$SCEN_DIR/bt-mark.err" <<'PY'
import errno, mmap, os, sys
dev, sector, lbs = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
fd = os.open(dev, os.O_RDONLY | os.O_DIRECT)
buf = mmap.mmap(-1, lbs)  # page-aligned, as O_DIRECT requires
try:
    got = os.preadv(fd, [buf], sector * 512)
except OSError as e:
    # EIO is expected: during the outage the origin's unmapped blocks
    # come over the cut link. The read was still queued, so it is in the
    # trace, which is all a marker is for. Anything else (EINVAL, a bad
    # path) means no marker, and is reported.
    if e.errno == errno.EIO:
        sys.exit(0)
    raise
if got != lbs:
    sys.exit(f"short read: {got} of {lbs} bytes")
PY
    then
      echo "WARNING: fence marker read at sector $1 failed: \
$(tail -1 "$SCEN_DIR/bt-mark.err")"
    fi
  }
  # Trace time of a marker, empty if it never reached the trace. Matched
  # on size as well as sector so a larger read that happens to begin
  # there cannot stand in for the marker.
  bt_mark_t() { # $1 = sector -> trace seconds
    awk -v s="$1" -v n=$(( BT_LBS / 512 )) \
      '$3=="Q" && $5==s && $6==n { print $1 + $2/1e9; exit }' \
      "$SCEN_DIR/origin-thin.bt" 2>/dev/null || true
  }
  bt_mark "$BT_MARK_START"

  PUBLISHED_BEFORE=$(grep -c "snapshot export published" "$SCEN_DIR/daemon.log" || true)
  ORIGIN_READS_BEFORE=$(read_sectors "$ORIGIN_DEV")
  POOL_USED_BEFORE=$(pool_used sky-s15j-tpool)
  SHIM_READS_BEFORE=$(dm_read_sectors sky-s15j-fs)
  THIN_READS_BEFORE=$(dm_read_sectors sky-s15j-origin)

  # WHO opens the origin thin, sampled at 0.1 s. Superseded by blktrace
  # below, which sees every request rather than whoever happens to hold
  # the device when the poll lands — this found NO pids across a recovery
  # in which blktrace counted 54 reads, because udev's probe opens,
  # scans and closes well inside the sampling interval. Kept because
  # "nothing held it open" is still the right answer to a different
  # question, and the blktrace pid is not a substitute for it.
  # `{ set +x; } 2>/dev/null` rather than a bare `set +x`: the enclosing
  # shell traces the disabling command itself, and that trace is written
  # to the subshell's stderr — which is redirected into the log. The
  # first version wrote a bare `set +x` and the log's only line was
  # `+ set +x`, so the sampler reported its own trace as the opener set.
  ( { set +x; } 2>/dev/null
    while :; do
      for pid in $(lsof -t /dev/mapper/sky-s15j-origin 2>/dev/null); do
        c=$(cat "/proc/$pid/comm" 2>/dev/null)
        # The tracer below holds the device for the whole recovery, so
        # without this it is the only opener the sampler ever reports.
        [ "$c" = blktrace ] || echo "$pid $c"
      done
      sleep 0.1
    done ) > "$SCEN_DIR/origin-openers.log" 2>&1 &
  OPENERS_PID=$!

  # Wall clock at each fence, in the same units the anchor converts the
  # trace into. Plain `date`, because the trace clock is now reconciled
  # to wall clock once, at the anchor, rather than assumed equal to it
  # at every fence.
  POOL_PCT_BEFORE=$(pool_pct sky-s15j-tpool)
  rm -f "$CUT_FILE"

  # ---- 6. Advance completes, and only THEN the remount ---------------
  # Sample the moment the repoint lands — before the reclaim and the
  # remount — so the publish gets a bracket of its own. Polled tighter
  # than `wait_log`'s 0.5 s on purpose: the reclaim and the remount
  # follow the repoint within a few hundred milliseconds, and a coarse
  # poll would sample straight past both and measure exactly what this
  # bracket exists to exclude. "origin repointed" appears once, at
  # recovery; the bootstrap's line is "origin export published".
  # THE RECOVERY NO LONGER CAPTURES, so there is no capture to fence.
  #
  # This used to wait for the candidate activation to appear, on the
  # reasoning that it is the first externally visible moment of the
  # recovery cycle. That stopped being true the moment the agent learned
  # to resume a stranded candidate: the activation has been present for
  # the whole outage, so the poll returns on its FIRST iteration and the
  # start->capture window collapses to nothing. It would not have gone
  # red — it would have passed while gating an empty window, and the
  # later brackets would have named a cycle other than the one they
  # measured. That is precisely the vacuity #82 spent several commits
  # removing, reintroduced through the back door by a behaviour change
  # somewhere else.
  #
  # So the fence is replaced by the assertion it should always have
  # been. What distinguishes a resumed cycle from a fresh capture is the
  # candidate's THIN DEV ID, and requiring it unchanged from the strand
  # right through to the publish is a stronger statement than any window
  # over the old fence: the recovery published without taking a second
  # snapshot, which is the property this whole change exists to produce.
  #
  # Consequence, stated rather than buried: this removes S15j's last
  # GATED blktrace window. The upload window was already reported rather
  # than gated (its fence trails the agent), and with the capture gone
  # there is no soundly fenced window left. The trace stays as
  # attribution, which is what it is good for; §8.2 step 4's claim is
  # carried by `a_captured_candidate_cannot_read_through_to_the_origin`,
  # which does not depend on a fence at all.
  # No `*_AT_CAPTURE` samples and no CAPTURE marker: with the fence gone
  # they would sit microseconds after their `_BEFORE` counterparts, so
  # every window built on them is zero-width BY CONSTRUCTION. A boundary
  # that always reads 0 does not merely fail to inform — it gets quoted
  # later as evidence that nothing happened there, which is the whole
  # lesson this scenario has paid for repeatedly. The reports below
  # bracket start->publish instead, and say so.

  # The publish bracket closes when the GENERATION IS PUBLISHED, which
  # is strictly before the repoint and the reclaim.
  #
  # The previous fence — the daemon's "origin repointed" — was too late.
  # The daemon logs that when it processes the repoint, and the agent's
  # reclaim runs immediately after, so the sample caught a variable
  # leading edge of post-repoint I/O. That is the whole race: the phase
  # split measured freeze+capture 0, upload 216, remount 2040 on one run
  # and publish 2088 on another, with the total pinned at ~2256 both
  # times. The reads were never moving; the fence was.
  #
  # After the repoint the origin serves the ranges the reclaim unmaps,
  # so reading it is the advance working (§8.2 steps 5-6). Before the
  # publish completes, nothing should need it — that is step 4's claim,
  # and this is the boundary it is actually about.
  PUBLISH_DEADLINE=$(( $(date +%s) + 300 ))
  while [ "$(date +%s)" -lt "$PUBLISH_DEADLINE" ]; do
    [ "$(grep -c "snapshot export published" "$SCEN_DIR/daemon.log" || true)" \
      -gt "$PUBLISHED_BEFORE" ] && break
    sleep 0.05
  done
  ORIGIN_READS_AT_PUBLISH=$(read_sectors "$ORIGIN_DEV")
  POOL_USED_AT_PUBLISH=$(pool_used sky-s15j-tpool)
  bt_mark "$BT_MARK_PUBLISH"

  REPOINT_DEADLINE=$(( $(date +%s) + 300 ))
  while [ "$(date +%s)" -lt "$REPOINT_DEADLINE" ]; do
    grep -q "origin repointed" "$SCEN_DIR/daemon.log" && break
    sleep 0.05
  done
  ORIGIN_READS_AT_REPOINT=$(read_sectors "$ORIGIN_DEV")
  POOL_USED_AT_REPOINT=$(pool_used sky-s15j-tpool)
  bt_mark "$BT_MARK_REPOINT"
  # Phase breakdown, because "the publish read the origin" is still too
  # coarse to act on. The publish's OWN reads are already excluded by
  # construction: `activate_ro` builds the candidate with no external
  # origin (dm.rs), so an unmapped read of it returns zeroes and cannot
  # reach this device at all. Whatever these sectors are, they came
  # through the LIVE stack — the origin thin and the shim over it — not
  # through the delta path. Sampling either side of the capture splits
  # the two suspects that remain: the freeze forcing writeback/metadata
  # on the still-mounted read-only filesystem, versus something during
  # the upload itself.
  echo "origin reads: cut ${ORIGIN_READS_BEFORE}, repoint ${ORIGIN_READS_AT_REPOINT}"
  grep -q "origin repointed" "$SCEN_DIR/daemon.log" \
    || { echo "FAIL: the recovery never repointed the origin (§8.2 step 5)"; exit 1; }

  wait_log "$SCEN_DIR/agent.log" "filesystem back READ-WRITE" 240 "the advance to restore read-write" || exit 1
  # Stop the loop BEFORE sampling. With the agent still cycling, the
  # exact-equality assertion below would be racing the next cycle's first
  # read of the now-unmapped set, and its whole margin would silently be
  # `interval_s` — a coupling nothing states and nothing enforces.
  kill "$AGENT_PID" 2>/dev/null || true
  wait "$AGENT_PID" 2>/dev/null || true
  AGENT_PID=""
  ORIGIN_READS_AFTER=$(read_sectors "$ORIGIN_DEV")
  bt_mark "$BT_MARK_AFTER"

  # NOT checked here: whether the recovery resumed the stranded candidate
  # rather than re-capturing. Two versions of that check lived at this
  # point and both read `none`, and the second one — with the agent dead
  # and the recovery complete, so no race left to blame — is what finally
  # made the reason legible. The recovery cycle is an ADVANCE, and
  # `promote_advance` ends with §3 step 7: `snap_delete(cand)`, deleting S
  # and rebaselining to a pair with no local member (cycle.rs, "the
  # memoryless post-advance shape"). A correct resume therefore leaves no
  # `-c-` and no `-b-` device behind — exactly the state the assertion
  # read as a failure. The check was never observing the difference it
  # claimed to; it was asserting a post-state that does not exist.
  #
  # The resume-vs-re-capture discrimination lives where the two devices
  # actually coexist: the MID-OUTAGE samples above, where a fresh capture
  # would have allocated a new thin dev id across two `cycle failed`
  # events ~180 s apart and did not. The end-state properties are covered
  # deterministically in `outage_degradation.rs`.
  POOL_USED_AFTER=$(pool_used sky-s15j-tpool)
  POOL_PCT_AFTER=$(pool_pct sky-s15j-tpool)
  echo "origin sectors read: publish ${ORIGIN_READS_BEFORE} -> ${ORIGIN_READS_AT_REPOINT}, \
then through the remount -> ${ORIGIN_READS_AFTER}"
  echo "pool: ${POOL_PCT_BEFORE}% -> ${POOL_PCT_AFTER}%"
  # §8.2 step 4's claim, on the bracket the claim is actually about: the
  # backlog comes out of the LOCAL pool, so producing it must not need
  # the origin.
  # Monotonicity guard. These are samples of a counter that only ever
  # increases, so a decreasing sequence means the SAMPLES are out of
  # causal order, not that the kernel un-read something. The first
  # version of this block sampled the publish before the capture and
  # reported "upload -808" — a number that is obviously impossible and
  # was still easy to read past. Fail on it instead.
  for pair in "$ORIGIN_READS_BEFORE $ORIGIN_READS_AT_PUBLISH" \
              "$ORIGIN_READS_AT_PUBLISH $ORIGIN_READS_AT_REPOINT" \
              "$ORIGIN_READS_AT_REPOINT $ORIGIN_READS_AFTER"; do
    # shellcheck disable=SC2086 # deliberate: split the "before after" tuple
    set -- $pair
    [ "$1" -le "$2" ] || {
      echo "FAIL: origin-read samples are out of order ($1 > $2) — the \
scenario's sample points do not follow the cycle's causal order"; exit 1; }
  done
  # The raw per-phase counts, PRINTED AND NEVER ASSERTED. They are the
  # totals for the whole device, and the device is read by more than the
  # thing under test — see the assertion further down for why that
  # matters and what replaced them.
  echo "origin reads by phase: start->publish \
$((ORIGIN_READS_AT_PUBLISH - ORIGIN_READS_BEFORE)), \
repoint+reclaim $((ORIGIN_READS_AT_REPOINT - ORIGIN_READS_AT_PUBLISH)), \
remount $((ORIGIN_READS_AFTER - ORIGIN_READS_AT_REPOINT))"
  # ATTRIBUTION. Counting the origin device says only THAT it was read.
  # These two say through WHAT. The shim is the only device the guest
  # filesystem is mounted on, and the origin thin is what passes an
  # unmapped read through to /dev/nbd0. So:
  #   shim delta ~= origin delta  -> guest-side reads (readahead on the
  #                                  still-mounted RO filesystem, or a
  #                                  probe of the mounted device);
  #   thin delta >> shim delta    -> something reads the origin thin
  #                                  DIRECTLY, bypassing the shim — the
  #                                  agent, udev, or blkid after a table
  #                                  change.
  # Three fence positions could not tell these apart, because a count
  # cannot. Printed, never asserted: this is here to identify the reads,
  # and a number I do not understand should not gate the suite.
  echo "attribution across the recovery: origin(nbd) \
$((ORIGIN_READS_AFTER - ORIGIN_READS_BEFORE)), \
shim $(( $(dm_read_sectors sky-s15j-fs) - SHIM_READS_BEFORE )), \
origin-thin $(( $(dm_read_sectors sky-s15j-origin) - THIN_READS_BEFORE ))"
  # IDENTIFY THE READER. Attribution says the origin THIN is read and the
  # shim is not (measured: origin-thin 2136, shim 40), so this is not the
  # guest filesystem and not the delta path. The remaining suspect is
  # udev: the recovery repoints the thin and then reclaims, each a dm
  # table change, and udev answers a table change by re-probing the
  # device — a scan of unmapped ranges reads straight through to the
  # external origin. That would also explain the variance, since the
  # probe is asynchronous and lands in a different phase every run
  # (measured 0, 88, 1264, 1400, 2088 with the total pinned near 2256).
  #
  # Test it rather than assume it: ask udev for one re-probe and price it.
  # A probe costing hundreds of sectors identifies the reader; a probe
  # costing nothing means the suspect is innocent and the search
  # continues. Done AFTER the recovery and after the agent is stopped,
  # so it perturbs nothing being asserted.
  kill "$OPENERS_PID" 2>/dev/null || true
  wait "$OPENERS_PID" 2>/dev/null || true
  OPENERS_PID=""
  echo "origin-thin openers during the recovery: \
$(sort -u "$SCEN_DIR/origin-openers.log" 2>/dev/null | tr '\n' ';' || echo none)"
  # Close the trace here, at the same fence as the opener sampler: what
  # follows is the udev probe and the assertions, and neither belongs in
  # a trace of the recovery. SIGINT is the only way blktrace flushes its
  # per-CPU buffers; killing the subshell first would truncate them.
  if [ -n "$BT_PID" ]; then
    # SIGINT is the only signal that makes blktrace flush its per-CPU
    # buffers; anything harsher truncates the trace.
    kill -INT "$BT_PID" 2>/dev/null || true
    wait "$BT_PID" 2>/dev/null || true
    BT_PID=""
    # Now parse, with the whole trace in hand — see the recorder above
    # for why this is not a live pipe.
    blkparse -i bt -D "$SCEN_DIR" -f "%T %t %a %d %S %n %p %C\n" \
      > "$SCEN_DIR/origin-thin.bt" 2>>"$SCEN_DIR/origin-thin.bt.err" || true
  fi
  # Resolve the fences out of the completed trace, in the trace's own
  # clock. Printed because a boundary that is wrong should be visible as
  # a boundary — the previous revision's failure looked like a product
  # defect precisely because its windows were invisible and only their
  # consequences were printed. Monotonically increasing marker times are
  # the cheap check that the fences mean what they say.
  BT_T_START=$(bt_mark_t "$BT_MARK_START")
  BT_T_PUBLISH=$(bt_mark_t "$BT_MARK_PUBLISH")
  BT_T_REPOINT=$(bt_mark_t "$BT_MARK_REPOINT")
  BT_T_AFTER=$(bt_mark_t "$BT_MARK_AFTER")
  echo "trace fences (trace seconds): start=${BT_T_START:-MISSING} \
publish=${BT_T_PUBLISH:-MISSING} \
repoint=${BT_T_REPOINT:-MISSING} after=${BT_T_AFTER:-MISSING}"

  # HYPOTHESIS 8, now REFUTED: dm-thin external-origin COPY-ON-WRITE. A
  # thin with an external origin must read the rest of a block from that
  # origin when it provisions a pool block for a partial write, so if
  # that were these sectors they would not be a defect at all — they
  # would be how external origins work, and §8.2 step 4's "zero" would be
  # the thing to correct.
  #
  # It was worth more than the seven stories before it because it
  # predicted a NUMBER: reads should track blocks PROVISIONED, at one
  # 64 KiB block = 128 sectors, so ~2128 sectors is ~17 blocks. The run
  # measured `upload 2088/-1` — 2088 sectors read while the pool
  # provisioned nothing (it shrank by one block). Reads accrue with zero
  # provisioning, so no copy-on-write is occurring and the claim in §8.2
  # stands as written. That is eight mechanisms killed by measurement,
  # and the attribution above is where the answer now has to come from.
  #
  # Kept, not deleted: it is two counters and a subtraction, and it is
  # the regression signal that would catch a future change which DOES
  # start provisioning here.
  echo "cow correlation (origin sectors / pool blocks provisioned): \
start->publish $((ORIGIN_READS_AT_PUBLISH - ORIGIN_READS_BEFORE))/\
$((POOL_USED_AT_PUBLISH - POOL_USED_BEFORE)), \
repoint+reclaim $((ORIGIN_READS_AT_REPOINT - ORIGIN_READS_AT_PUBLISH))/\
$((POOL_USED_AT_REPOINT - POOL_USED_AT_PUBLISH)), \
remount $((ORIGIN_READS_AFTER - ORIGIN_READS_AT_REPOINT))/\
$((POOL_USED_AFTER - POOL_USED_AT_REPOINT))"

  # KNOWN FALSE NEGATIVE — reads 0, and 0 is not the answer.
  #
  # This was written to price udev's re-probe and it retired the udev
  # hypothesis on the strength of measuring nothing. blktrace then
  # caught udev-worker issuing 2112 of the 2152 sectors, walking every
  # superblock magic offset from sector 0 and then the tail of the
  # device. The hypothesis was right; this instrument is what was wrong.
  #
  # `udevadm trigger --action=change` re-runs the rules but udev serves
  # blkid results from its own database for a device whose identity has
  # not changed, so nothing re-reads the disk and the probe prices at
  # zero however many times it is asked. A real table change invalidates
  # that cache; a synthetic uevent does not.
  #
  # Kept, and kept printing, because a diagnostic that reliably reads
  # zero for a reason worth knowing is more useful annotated than
  # deleted — the next person to reach for `udevadm trigger` as a
  # measurement should find out here rather than the long way.
  UDEV_R1=$(dm_read_sectors sky-s15j-origin)
  udevadm trigger --action=change /dev/mapper/sky-s15j-origin 2>/dev/null || true
  udevadm settle --timeout=10 2>/dev/null || true
  UDEV_R2=$(dm_read_sectors sky-s15j-origin)
  echo "udev probe cost: $((UDEV_R2 - UDEV_R1)) sectors for one re-probe of the \
origin thin (expected 0 — udev caches blkid; see comment, this does NOT \
exonerate udev)"

  # THE ATTRIBUTION, printed last of the measurements and immediately
  # before the assertion it explains. Everything above narrows; this
  # names — and it already has: udev's blkid probe issues essentially
  # all of these reads. What is printed now is the part that decides
  # what the assertion should say.
  #
  #   by phase — the probe is ASYNCHRONOUS, so which phase it lands in
  #              varies run to run while the total stays near 2256. Only
  #              the upload window is under §8.2 step 4, so the question
  #              is not "how many sectors" but "how many of them are the
  #              agent's". Per-phase, per-issuer answers that directly.
  #   agent    — the 40 sectors in 5 requests attributed to the agent
  #              across the whole recovery are the one thing blktrace
  #              has not yet explained. Their OFFSETS say what they are:
  #              repeated sector 0 is a size or superblock probe, and
  #              anything else is a finding. This is the last unknown,
  #              and the assertion is not being rewritten until it is
  #              resolved — scoping a claim around a mechanism while
  #              part of that mechanism is unattributed is exactly the
  #              mistake the one-chunk bound made.
  #
  # A DIAGNOSTIC MUST NOT BE ABLE TO FAIL THE RUN. The previous revision
  # of this block ended the scenario with rc=2 partway through printing
  # itself, under `set -euo pipefail`, and it could not say why — because
  # it sent its own awk's stderr to /dev/null. An instrument that
  # silences its instruments is how a wrong answer survives, which is
  # the lesson this whole scenario keeps re-teaching. So: stderr is kept
  # (in the log, tagged), the body cannot abort the caller, and the
  # awk does its own sorting and limiting instead of feeding a pipeline
  # whose failure modes have to be reasoned about under `pipefail`.
  #
  # Tracing off for the whole block: a multi-line awk under `set -x`
  # prints continuation lines WITHOUT a `+` prefix, so the nightly's
  # `^\++ ` filter cannot strip them and one run printed the awk source
  # into the middle of its own results.
  bt_report() {
    { set +x; } 2>/dev/null
    if [ ! -s "$SCEN_DIR/origin-thin.bt" ]; then
      echo "origin-thin blktrace: no trace captured"
      { [ -s "$SCEN_DIR/origin-thin.bt.err" ] \
        && echo "  tracer stderr: $(tail -2 "$SCEN_DIR/origin-thin.bt.err" | tr '\n' ' ')"; } || true
      return 0
    fi
    # WHAT THE AGENT READ, unconditionally — this needs no clock. It was
    # the last unknown and it resolved: 40 sectors in 5 requests, at
    # `280+8 296+8 304+8 312+8 320+8`, and the per-phase split below put
    # every one of them in the REMOUNT window. That is §8.2 step 6
    # working — after the repoint the origin serves the ranges the
    # reclaim unmapped, so bringing the filesystem back read-write reads
    # metadata through it. Nothing in the upload window, which is what
    # step 4 is about. Kept printing so a future change that moves them
    # is visible rather than silent.
    echo "origin-thin reads by the AGENT (sector+count, whole recovery):"
    awk '$3=="Q" && $4 ~ /R/ && $8 ~ /skynbd/ { printf " %s+%s", $5, $6 }
         END { print "" }' "$SCEN_DIR/origin-thin.bt" | cut -c1-300
    # PER PHASE, from the fence markers. §8.2 step 4 constrains the
    # upload window specifically, and udev's probe is asynchronous — it
    # lands in a different phase every run (the upload figure has
    # measured 0, 88, 120, 1264, 1400 and 2088 across runs of unchanged
    # commits) while the total stays near 2256. Without fences these
    # windows are fiction, so print nothing rather than fiction.
    if [ -z "$BT_T_START" ] || [ -z "$BT_T_AFTER" ]; then
      echo "origin-thin phase attribution: unavailable (fence markers missing)"
      return 0
    fi
    for ph in "start->publish $BT_T_START $BT_T_PUBLISH" \
              "repoint+reclaim $BT_T_PUBLISH $BT_T_REPOINT" \
              "remount $BT_T_REPOINT $BT_T_AFTER"; do
      # shellcheck disable=SC2086 # deliberate: split the "phase lo hi" tuple
      set -- $ph
      echo "origin-thin issuers, $1 phase (blktrace):"
      awk -v lo="$2" -v hi="$3" \
        '$3=="Q" && $4 ~ /R/ {
           w = $1 + $2/1e9
           if (w >= lo && w < hi) { k = $8"["$7"]"; s[k] += $6; n[k]++ }
         }
         END {
           c = 0
           for (k in s) { c++; printf "  %s: %d sectors in %d reqs\n", k, s[k], n[k] }
           if (c == 0) print "  (nothing)"
         }' "$SCEN_DIR/origin-thin.bt"
    done
  }
  bt_report || echo "origin-thin blktrace: reporting failed (rc=$?) — see above"

  # §8.2 STEP 4, asserted on what it actually claims.
  #
  # The claim is about the AGENT: the backlog comes out of the local
  # pool, so producing it must not need the origin. For a dozen runs
  # this was asserted on the origin device's raw read counter, which
  # measures the device rather than the claim — and the difference is
  # not academic. blktrace named the reader:
  #
  #   (udev-worker)[9339]: 2112 sectors in 49 reqs
  #   skynbd-agent[7934]:    40 sectors in  5 reqs
  #   offsets: 0+8 32+8 64+8 128+8 256+8 512+8 1024+8 2048+8 4096+8 ...
  #            524160+8 523888+8 523696+8 ...
  #
  # A doubling walk from sector 0 across every offset a filesystem
  # superblock magic can occupy, then the tail of the device for backup
  # superblocks and the secondary GPT header. That is `blkid`, and the
  # recovery triggers it because the repoint and the reclaim are each a
  # dm table change and udev answers a table change by re-probing. The
  # agent's own 40 sectors all landed in the REMOUNT window, which is
  # step 6 working as designed.
  #
  # udev's probe is asynchronous, so it lands in a different phase every
  # run: this window has measured 0, 88, 120, 1264, 1400 and 2088 across
  # runs of UNCHANGED commits while the run total stayed near 2256. A
  # counter-based assertion here is therefore a coin flip on a kernel
  # housekeeping task, which is exactly what it behaved like. Attributing
  # by issuer removes the confounder instead of tolerating it — this is
  # not a widened threshold, it is the same claim measured against the
  # right subject, and it is STRICTER than the counter ever was: exactly
  # zero agent sectors, in both windows the claim covers.
  #
  # Not a silent no-op without blktrace: the scenario SKIPS loudly if
  # blktrace or blkparse is missing (see the tool check at the top), and
  # fails here if the fences cannot be trusted.
  #
  # BOTH conditions are checked, because the failure that cost the most
  # was not a missing measurement — it was a measurement placed in the
  # wrong window, which is indistinguishable from a product defect at
  # the point of the assertion. Every fence must have reached the trace,
  # and they must be strictly increasing. Non-monotonic fences mean the
  # markers landed out of order, and any window built from them is
  # fiction whether or not the numbers look plausible.
  BT_PREV=""
  for f in "start $BT_T_START" "publish $BT_T_PUBLISH" \
           "repoint $BT_T_REPOINT" "after $BT_T_AFTER"; do
    # shellcheck disable=SC2086 # deliberate: split the "name time" tuple
    set -- $f
    [ -n "${2:-}" ] || { echo "FAIL: the $1 fence marker never reached the \
blktrace stream, so §8.2 step 4 cannot be evaluated"; exit 1; }
    if [ -n "$BT_PREV" ]; then
      awk -v a="$BT_PREV" -v b="$2" 'BEGIN{ exit !(a <= b) }' \
        || { echo "FAIL: trace fences are out of order at $1 ($BT_PREV > $2) — \
the phase windows do not correspond to the cycle's phases"; exit 1; }
    fi
    BT_PREV=$2
  done
  # Read sectors in a window, grouped by ISSUER, with `(null)` comms
  # resolved.
  #
  # Two holes in the obvious `comm ~ /skynbd/` filter, both of which
  # would make this assertion pass VACUOUSLY — the worst way for a check
  # like this to be wrong, since it claims something it never tested:
  #
  #  1. blkparse emits `(null)` for a comm it could not resolve, and a
  #     passing run shows exactly that — `(null)[9334]: 24 sectors`
  #     alongside `(udev-worker)[9334]: 128`, the same pid under two
  #     names. A `(null)` that was really the agent would slip the
  #     filter. Pass one over the trace learns pid -> comm from every
  #     line that IS named, and pass two uses it to fill the blanks.
  #  2. comm is the THREAD name, not the process name. The agent is
  #     async; a read issued from a runtime worker thread need not carry
  #     `skynbd` in its comm at all.
  #
  # So invert the test. Do not ask "did the agent read?" — ask "did
  # anything read that is not accounted for?". Everything in these two
  # windows must be on the allowlist below, and the allowlist contains
  # only issuers that are not the agent BY CONSTRUCTION. That catches
  # the agent under any thread name and any comm resolution failure, and
  # an unattributable read fails rather than being assumed harmless.
  bt_window_issuers() { # $1 = lo, $2 = hi -> "<comm> <sectors>" per line
    awk -v lo="$1" -v hi="$2" '
      NR==FNR { if ($8 != "(null)" && $8 != "") c[$7]=$8; next }
      $3=="Q" && $4 ~ /R/ {
        w = $1 + $2/1e9
        if (w < lo || w >= hi) next
        k = $8
        if (k == "(null)" || k == "") k = ($7 in c) ? c[$7] : "unattributed[" $7 "]"
        s[k] += $6
      }
      END { for (k in s) printf "%s %d\n", k, s[k] }
    ' "$SCEN_DIR/origin-thin.bt" "$SCEN_DIR/origin-thin.bt"
  }
  # Reported, all of it. Nothing here gates any more — see the fence
  # note at step 6 for why the capture window went away with the capture
  # it covered, and the paragraph below for why the upload window was
  # never gated. Printed per issuer with `(null)` comms resolved, so a
  # reader who does show up is named rather than merely counted.
  echo "recovery windows by issuer (reported, not gated):"
  echo "  start->publish: $(bt_window_issuers "$BT_T_START" "$BT_T_PUBLISH" | tr '\n' ';')"
  echo "  publish->repoint: $(bt_window_issuers "$BT_T_PUBLISH" "$BT_T_REPOINT" | tr '\n' ';')"
  # NOTHING HERE GATES, and both reasons are worth keeping written down
  # because they are different reasons.
  #
  # The capture window went away with the capture: the recovery resumes
  # a stranded candidate rather than taking a new one, so there is no
  # freeze and no snapshot in that cycle to bracket. Its fence — the
  # candidate appearing in `dmsetup ls` — would return on its first poll
  # against a device that had been there all outage, gating an empty
  # window and quietly passing. The property it was reaching for is now
  # asserted directly, on the candidate's dev id at the publish.
  #
  # The upload fence cannot be trusted, and a run proved it rather than
  # my suspecting it. The agent's publish → repoint → reclaim → remount
  # sequence completes in about 10 ms; the scenario observes each step
  # by polling logs at 50 ms. On one run the fences came out
  # publish=0.339 repoint=0.351 after=0.362 — an ELEVEN MILLISECOND
  # remount window where the passing run had 530 ms — and the remount's
  # reads, the same five requests at 280..328 that every run produces,
  # landed on the upload side of a fence that had not caught up yet.
  # The raw device counter agreed with the trace that time
  # (`repoint+reclaim 0, remount 0`), which is how this is
  # distinguishable from the earlier clock bug, where the two
  # disagreed. Nothing was misplaced RELATIVE to anything else; the
  # whole fence set simply trailed the agent.
  #
  # So the upload window is printed and not asserted, because a gate
  # that fires when the product is correct is worse than no gate. The
  # claim it was trying to make is now carried where it can be made
  # exactly — `a_captured_candidate_cannot_read_through_to_the_origin`
  # in crates/skynbd-agent/tests/dm_swing.rs asserts that an unmapped
  # read of the candidate returns ZEROS, which is §8.2 step 4's "by
  # construction" stated as the construction: `activate_ro` builds the
  # candidate through `create_ro_thin`, whose table carries no external
  # origin, so the publish has nowhere to fall through to. That test is
  # deterministic and mutation-verified; this window is neither.
  #
  # To gate it here properly, the fence would have to come from inside
  # the trace rather than from a log poll — the reclaim's discards on
  # this device are the obvious candidate, being both agent-issued and
  # strictly between the publish and the remount.
  # The capture gate is gone with the capture it gated — see the fence
  # note at step 6. Nothing here is asserted from the trace now; the
  # attribution above is printed, and §8.2 step 4's claim lives in
  # `a_captured_candidate_cannot_read_through_to_the_origin`.
  # Step 6 is keyed on space actually returned, not on "an advance
  # completed" — so the reclaim must be visible in the pool, not just in
  # the log, or the remount happened over a still-full pool.
  [ "$POOL_PCT_AFTER" -lt "$POOL_PCT_BEFORE" ] \
    || { echo "FAIL: read-write restored without the pool shrinking (${POOL_PCT_BEFORE}% -> ${POOL_PCT_AFTER}%)"; exit 1; }
  grep -q " $MOUNT_A .*[( ]rw[,)]" /proc/mounts \
    || { echo "FAIL: agent logged read-write but the mount is still ro"; exit 1; }

  # ---- 7. The delta survived, and the volume takes writes again ------
  "$DD" if=/dev/urandom of="$MOUNT_A/corpus/post-recovery" bs=1M count=1 2>/dev/null \
    || { echo "FAIL: filesystem still refuses writes after recovery"; exit 1; }
  rm -f "$MOUNT_A/corpus/post-recovery"; sync
  umount "$MOUNT_A"; mount /dev/mapper/sky-s15j-fs "$MOUNT_A"
  (cd "$MOUNT_A/corpus" && find . -type f -print0 | sort -z | xargs -0 md5sum) \
    | diff -u "$SCEN_DIR/manifest.outage" - \
    || { echo "FAIL: the delta written during the outage did not survive"; exit 1; }
  echo "outage delta intact across the remount"

  kill "$NETCUT_PID" 2>/dev/null; NETCUT_PID=""

  # Teardown (targeted — never remove_all on a shared host).
  umount "$MOUNT_A"; MOUNT_A=""
  for d in $(dmsetup ls 2>/dev/null | awk '/^sky-vlin15j-/ {print $1}'); do
    dmsetup remove "$d" 2>/dev/null || true
  done
  dmsetup remove sky-s15j-rec
  dmsetup remove sky-s15j-fs
  dmsetup remove sky-s15j-origin
  dmsetup remove sky-s15j-tpool
  losetup -d "$DATA" "$META" || true
  # RELEASE /dev/nbd0. Not optional, and it must come after the dm
  # removals above: every one of those targets references the nbd device,
  # so a disconnect before them cannot succeed.
  #
  # The `trap ... EXIT` this scenario installs is not a substitute. An
  # EXIT trap fires when the SCRIPT exits, not when the function returns,
  # so in a multi-scenario run it does nothing between scenarios — it
  # runs once, at the very end, long after the next scenario has already
  # tried and failed to claim the device. Every other Tier B scenario
  # releases on its success path for this reason; S15j was the one that
  # did not, and `S4 S15j S14` is the order the nightly runs.
  tierb_cleanup
  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  rm -rf "$STATE_DIR"
  echo "S15j PASS"
}


# S1u (Tier A, no root): UDS NBD listener + authed API + lineage volume —
# the config shapes that made the first nightly fail, exercised on every
# PR. qemu speaks nbd+unix://; the coordinator sends bearer auth.
s1u() {
  echo "== S1u: unix-socket NBD + authed API + lineage ingestion =="
  need curl python3 qemu-img qemu-io
  scenario_init s1u
  echo "s1u-secret" > "$SCEN_DIR/token"
  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "unix:${SCEN_DIR}/nbd.sock"
api_listen = "127.0.0.1:${API_PORT}"
api_auth_token_path = "${SCEN_DIR}/token"

[store]
kind = "memory"

[[volumes]]
name = "vol1"
size = 8388608

[[volumes]]
name = "vlin"
size = 4194304
kind = "lineage"
EOF
  start_daemon
  AUTH="Authorization: Bearer s1u-secret"

  # Wrong/absent token rejected; healthz open (auth middleware contract).
  curl -sf "$API/healthz" >/dev/null || { echo "FAIL: healthz"; exit 1; }
  [ "$(curl -s -o /dev/null -w '%{http_code}' "$API/v1/volumes")" = 401 ] \
    || { echo "FAIL: unauthed not rejected"; exit 1; }

  # Head IO over the unix socket.
  UDS_URI="nbd+unix:///vol1/head?socket=${SCEN_DIR}/nbd.sock"
  qemu-io -f raw "$UDS_URI" -c 'write -P 0x3c 0 1M' -c flush -c 'read -P 0x3c 0 1M' >/dev/null \
    || { echo "FAIL: qemu-io over UDS"; exit 1; }

  # Authed ingestion: bootstrap a generation into the lineage volume.
  python3 -c "open('$SCEN_DIR/src.img','wb').write(bytes((i*13+7)%256 for i in range(4194304)))"
  SKYNBD_TOKEN=s1u-secret python3 tests/e2e/coordinator.py upload "$API" vlin "$SCEN_DIR/src.img" g0 >/dev/null
  sleep 0.3
  qemu-img convert -f raw -O raw "nbd+unix:///vlin/snap/g0?socket=${SCEN_DIR}/nbd.sock" "$SCEN_DIR/g0.img"
  cmp -s "$SCEN_DIR/g0.img" "$SCEN_DIR/src.img" || { echo "FAIL: g0 content over UDS"; exit 1; }

  # fsck response shape as the rigs consume it.
  curl -sf -H "$AUTH" -X POST "$API/v1/volumes/vlin/fsck" | python3 -c '
import json,sys; r=json.load(sys.stdin)
assert all(e is None for _, e in r["refs"]), r'

  kill -9 "$DAEMON_PID"; DAEMON_PID=""
  postconditions
  echo "S1u PASS"
}

s15k() {
  echo "== S15k (Tier B): §7.1 read cache — mount device, mask, fresh-pool cold cache =="
  if ! reason=$(tierb_available); then echo "S15k SKIPPED: $reason"; return 0; fi
  for t in dmsetup losetup mkfs.ext4 findmnt fstrim blockdev; do
    command -v "$t" >/dev/null || { echo "S15k SKIPPED: missing $t"; return 0; }
  done
  AGENT=./target/debug/skynbd-agent
  [ -x "$AGENT" ] || { echo "S15k SKIPPED: build with cargo build -p skynbd-agent-cli"; return 0; }
  scenario_init s15k
  STATE_DIR=/var/tmp/skynbd-agent-s15k-$$
  mkdir -p "$STATE_DIR"
  MNT="$SCEN_DIR/mnt"; mkdir -p "$MNT"

  # The driver is the resident daemon's [mounts] path — the same shared
  # NodeMechanics staging the CSI node runs, which is what builds the
  # rcache trio, stamps cold_read_cache from the fresh-pool condition,
  # and owns the D-M13 final cycle this scenario's phases pivot on.
  s15k_cleanup() {
    set +e
    [ -n "$AGENT_PID" ] && kill -9 "$AGENT_PID" 2>/dev/null
    AGENT_PID=""
    # The scenario's OWN skynbdd dies here, success path included —
    # every other scenario kills $DAEMON_PID before its PASS, and the
    # first scheduled nightly with this scenario proved why: a leaked
    # daemon keeps answering /healthz on the shared harness port, so
    # the NEXT scenario's start_daemon adopts it and runs against a
    # server with the wrong volumes (S15e over S15k's daemon passed by
    # coincidence — same volume name; S14 over S15e's died at its
    # attach with "Unknown error returned by server").
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null
    DAEMON_PID=""
    umount "$MNT" 2>/dev/null
    for d in rcache rcdata rcmeta fs rec origin pool data tmeta; do
      dmsetup remove "sky-wsv-$d" 2>/dev/null || true
    done
    for f in "$STATE_DIR/cache/wsv.cache" "$STATE_DIR/rcache/wsv.rcache"; do
      losetup -j "$f" 2>/dev/null | cut -d: -f1 | while read -r l; do
        losetup -d "$l" 2>/dev/null || true
      done
    done
    for i in 0 1 2 3 4 5 6 7; do
      nbd-client -c "/dev/nbd$i" >/dev/null 2>&1 && nbd-client -d "/dev/nbd$i" >/dev/null 2>&1
    done
    rm -rf "$STATE_DIR"
    set -e
  }

  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"

[store]
kind = "memory"

[[volumes]]
name = "wsv"
size = 268435456
EOF
  start_daemon
  trap 'rc=$?; s15k_cleanup; cleanup' EXIT

  # interval_s is long on purpose: the ONLY publishes this scenario
  # wants are the D-M13 final cycles at unmount, so "unpublished delta"
  # in phase B stays a controlled fact rather than a race against the
  # cadence.
  cat > "$STATE_DIR/agent.toml" <<EOF
server = "${API}"
state_dir = "${STATE_DIR}"

[mounts]
nbd_host = "127.0.0.1"
nbd_port = ${PORT}
cache_bytes = 134217728
read_cache_dir = "${STATE_DIR}/rcache"
read_cache_bytes = 67108864
chunk_bytes = 65536
interval_s = 3600
freeze_budget_ms = 30000
EOF
  start_mounts_daemon() {
    RUST_LOG="${RUST_LOG:-info}" "$AGENT" run "$STATE_DIR/agent.toml" \
      >> "$SCEN_DIR/mounts-agent.log" 2>&1 &
    AGENT_PID=$!
    for _ in $(seq 1 50); do
      [ -S "$STATE_DIR/ctl.sock" ] && return 0
      kill -0 "$AGENT_PID" 2>/dev/null \
        || { echo "FAIL: mounts daemon died at startup"; tail -30 "$SCEN_DIR/mounts-agent.log"; return 1; }
      sleep 0.2
    done
    echo "FAIL: ctl.sock never appeared"; return 1
  }
  rcache_src() { findmnt -n -o SOURCE --target "$MNT" 2>/dev/null; }
  assert_on_rcache() {
    [ "$(rcache_src)" = "/dev/mapper/sky-wsv-rcache" ] \
      || { echo "FAIL($1): fs is on '$(rcache_src)', not the rcache"; exit 1; }
  }
  assert_fstrim_refused() {
    # Invariant B on the device the guest actually sees: dm-cache
    # advertises nonzero virtual discard limits with passdown disabled,
    # so this probes the RCACHE queue's own mask (§7.1 / review
    # finding 2). Success here would mean dm-cache's absorb is the only
    # wall.
    if fstrim "$MNT" 2>/dev/null; then
      echo "FAIL($1): guest fstrim succeeded on the cached mount (Invariant B)"; exit 1
    fi
  }

  start_mounts_daemon || exit 1

  # ---- phase A: fresh mount, write + publish the baseline ------------
  "$AGENT" mount wsv "$MNT" --thin --state-dir "$STATE_DIR" \
    || { echo "FAIL: mount --thin refused"; tail -30 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  assert_on_rcache A
  assert_fstrim_refused A
  "$DD" if=/dev/urandom of="$SCEN_DIR/patA" bs=1M count=8 2>/dev/null
  "$DD" if="$SCEN_DIR/patA" of="$MNT/probe" bs=1M oflag=direct conv=fsync 2>/dev/null
  cmp -s "$SCEN_DIR/patA" "$MNT/probe" || { echo "FAIL: phase-A readback"; exit 1; }
  # The unmount's D-M13 final cycle publishes generation A.
  "$AGENT" unmount "$MNT" --state-dir "$STATE_DIR" \
    || { echo "FAIL: unmount refused"; tail -30 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  # findmnt --target walks UP to the parent mount for an unmounted dir,
  # so "unmounted" is asserted as "no longer the rcache", not "no row".
  [ "$(rcache_src)" = "/dev/mapper/sky-wsv-rcache" ] \
    && { echo "FAIL: still mounted after unmount"; exit 1; }

  # ---- phase B: warm remount, UNPUBLISHED delta, warm cache ----------
  "$AGENT" mount wsv "$MNT" --thin --state-dir "$STATE_DIR" \
    || { echo "FAIL: warm remount refused"; exit 1; }
  assert_on_rcache B
  "$DD" if=/dev/urandom of="$SCEN_DIR/patB" bs=1M count=8 2>/dev/null
  # In-place overwrite (conv=notrunc, O_DIRECT): the same file extents,
  # so the same shim LBAs — which is what makes any later resurrection
  # byte-attributable to the cache rather than to allocator drift.
  "$DD" if="$SCEN_DIR/patB" of="$MNT/probe" bs=1M conv=notrunc,fsync oflag=direct 2>/dev/null
  # Warm the rcache on B's blocks: repeated O_DIRECT re-reads until
  # dm-cache reports resident blocks. Without observed residency the
  # cold-cache assertion below would be vacuously green.
  warm=0
  for _ in $(seq 1 30); do
    "$DD" if="$MNT/probe" of=/dev/null bs=1M iflag=direct 2>/dev/null
    used=$(dmsetup status sky-wsv-rcache | awk '{split($7, a, "/"); print a[1]}')
    [ "${used:-0}" -gt 0 ] && { warm=1; break; }
  done
  [ "$warm" = 1 ] || { echo "FAIL: rcache never promoted a block — the scenario premise is vacuous"; \
                       dmsetup status sky-wsv-rcache; exit 1; }
  cmp -s "$SCEN_DIR/patB" "$MNT/probe" || { echo "FAIL: phase-B readback"; exit 1; }

  # Kill the daemon (no final cycle: the B delta stays UNPUBLISHED),
  # then simulate the reboot by hand: umount, stack down, loops
  # detached — and the pool file LOST while the warm rcache file
  # survives. This is the §7.1 third path: content below the cache
  # replaced without traversing it.
  kill -9 "$AGENT_PID"; AGENT_PID=""
  umount "$MNT"
  for d in rcache rcdata rcmeta fs rec origin pool data tmeta; do
    dmsetup remove "sky-wsv-$d" 2>/dev/null || true
  done
  for f in "$STATE_DIR/cache/wsv.cache" "$STATE_DIR/rcache/wsv.rcache"; do
    losetup -j "$f" 2>/dev/null | cut -d: -f1 | while read -r l; do
      losetup -d "$l" 2>/dev/null || true
    done
  done
  for i in 0 1 2 3 4 5 6 7; do
    nbd-client -c "/dev/nbd$i" >/dev/null 2>&1 && nbd-client -d "/dev/nbd$i" >/dev/null 2>&1
  done
  rm "$STATE_DIR/cache/wsv.cache"
  [ -f "$STATE_DIR/rcache/wsv.rcache" ] || { echo "FAIL: rcache file vanished"; exit 1; }

  # ---- phase C: replay = fresh pool over the warm cache file ---------
  # The daemon's mount-table replay re-stages: pool file gone → the
  # pool-lost fresh path (asserted from its log), origin attach on the
  # published generation, and — the point of this scenario — a COLD
  # cache: the zero written through write_dev must be device-visible
  # before the cache ctr (the fsync ordering), or dm-cache adopts the
  # warm mappings and serves B.
  : > "$SCEN_DIR/mounts-agent.log"
  start_mounts_daemon || exit 1
  for _ in $(seq 1 150); do
    [ "$(rcache_src)" = "/dev/mapper/sky-wsv-rcache" ] && break
    sleep 1
  done
  assert_on_rcache C
  grep -q "pool file lost" "$SCEN_DIR/mounts-agent.log" \
    || { echo "FAIL: replay did not take the pool-lost fresh path"; \
         tail -40 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  "$DD" if="$MNT/probe" of="$SCEN_DIR/readback" bs=1M iflag=direct 2>/dev/null
  if cmp -s "$SCEN_DIR/patB" "$SCEN_DIR/readback"; then
    echo "FAIL: the read cache RESURRECTED the dead unpublished delta over a fresh pool"
    echo "      (fresh pool => cold cache did not hold — §7.1's dormant-entry class)"
    exit 1
  fi
  cmp -s "$SCEN_DIR/patA" "$SCEN_DIR/readback" \
    || { echo "FAIL: post-restage probe matches NEITHER generation — corruption"; exit 1; }
  assert_fstrim_refused C
  echo "-- fresh pool came up cold: published content served, dead delta not resurrected"

  "$AGENT" unmount "$MNT" --state-dir "$STATE_DIR" || true
  s15k_cleanup
  trap 'rc=$?; cleanup' EXIT
  echo "S15k PASS"
}

s15e() {
  echo "== S15e (Tier B): two-pool swing on a PROVISIONED mount, under the §7.1 read cache =="
  if ! reason=$(tierb_available); then echo "S15e SKIPPED: $reason"; return 0; fi
  for t in dmsetup losetup mkfs.ext4 findmnt fstrim; do
    command -v "$t" >/dev/null || { echo "S15e SKIPPED: missing $t"; return 0; }
  done
  AGENT=./target/debug/skynbd-agent
  [ -x "$AGENT" ] || { echo "S15e SKIPPED: build with cargo build -p skynbd-agent-cli"; return 0; }
  scenario_init s15e
  STATE_DIR=/var/tmp/skynbd-agent-s15e-$$
  mkdir -p "$STATE_DIR"
  MNT="$SCEN_DIR/mnt"; mkdir -p "$MNT"

  # The claim under test is 15e's swing arming: the daemon stages the
  # CSI-parity publish-only stack (loop-backed pool, rcache over the
  # shim), and a watermark-promoted advance runs the §7 two-pool swing
  # against THAT stack on the real kernel — the pool file swapped for
  # its `.b` build, the provision-named carves (`sky-wsv-tmeta`/`-data`)
  # removed and reclaimed by the canonicalize, the shim retargeted
  # UNDER a live dm-cache holding it open, and the mount never noticing.
  # S15b proves the swing mechanics on a config rig; what it cannot
  # prove is this composition — provision names, the cache above, and
  # the loop identity surviving the rename (the inode-match fix in
  # `backing_of` and the unstage's associated-loop resolution).
  s15e_cleanup() {
    set +e
    [ -n "$AGENT_PID" ] && kill -9 "$AGENT_PID" 2>/dev/null
    AGENT_PID=""
    # The scenario's OWN skynbdd dies here, success path included —
    # every other scenario kills $DAEMON_PID before its PASS, and the
    # first scheduled nightly with this scenario proved why: a leaked
    # daemon keeps answering /healthz on the shared harness port, so
    # the NEXT scenario's start_daemon adopts it and runs against a
    # server with the wrong volumes (S15e over S15k's daemon passed by
    # coincidence — same volume name; S14 over S15e's died at its
    # attach with "Unknown error returned by server").
    [ -n "$DAEMON_PID" ] && kill -9 "$DAEMON_PID" 2>/dev/null
    DAEMON_PID=""
    umount "$MNT" 2>/dev/null
    dmsetup ls 2>/dev/null | awk '{print $1}' | grep -E '^sky-wsv' | while read -r d; do
      dmsetup remove "$d" 2>/dev/null || true
    done
    for f in "$STATE_DIR/cache/wsv.cache" "$STATE_DIR/cache/wsv.cache.b" \
             "$STATE_DIR/rcache/wsv.rcache"; do
      losetup -j "$f" 2>/dev/null | cut -d: -f1 | while read -r l; do
        losetup -d "$l" 2>/dev/null || true
      done
    done
    for i in 0 1 2 3 4 5 6 7; do
      nbd-client -c "/dev/nbd$i" >/dev/null 2>&1 && nbd-client -d "/dev/nbd$i" >/dev/null 2>&1
    done
    rm -rf "$STATE_DIR"
    set -e
  }

  cat > "$SCEN_DIR/skynbd.toml" <<EOF
nbd_listen = "127.0.0.1:${PORT}"
api_listen = "127.0.0.1:${API_PORT}"
refs_path = "${SCEN_DIR}/refs.redb"

[store]
kind = "memory"

[[volumes]]
name = "wsv"
size = 268435456
EOF
  start_daemon
  trap 'rc=$?; s15e_cleanup; cleanup' EXIT

  # Short cadence + a low watermark + `reclaim_strategy = "swing"`: the
  # pool is 124 MiB of data carve (128 MiB minus the 4 MiB tmeta), so
  # crossing ~50 MiB of occupancy promotes the next interval cycle to
  # an advance, and the strategy override sends its reclaim through the
  # swing whenever the headroom guard clears.
  cat > "$STATE_DIR/agent.toml" <<EOF
server = "${API}"
state_dir = "${STATE_DIR}"

[mounts]
nbd_host = "127.0.0.1"
nbd_port = ${PORT}
cache_bytes = 134217728
read_cache_dir = "${STATE_DIR}/rcache"
read_cache_bytes = 67108864
chunk_bytes = 65536
interval_s = 4
freeze_budget_ms = 30000
advance_watermark = 0.4
reclaim_strategy = "swing"
EOF
  start_s15e_daemon() {
    RUST_LOG="${RUST_LOG:-info}" "$AGENT" run "$STATE_DIR/agent.toml" \
      >> "$SCEN_DIR/mounts-agent.log" 2>&1 &
    AGENT_PID=$!
    for _ in $(seq 1 50); do
      [ -S "$STATE_DIR/ctl.sock" ] && return 0
      kill -0 "$AGENT_PID" 2>/dev/null \
        || { echo "FAIL: mounts daemon died at startup"; tail -30 "$SCEN_DIR/mounts-agent.log"; return 1; }
      sleep 0.2
    done
    echo "FAIL: ctl.sock never appeared"; return 1
  }
  POOL_FILE="$STATE_DIR/cache/wsv.cache"
  pool_loop() { losetup -j "$POOL_FILE" 2>/dev/null | cut -d: -f1 | head -1; }
  pool_used() { dmsetup status sky-wsv-pool 2>/dev/null | awk '{split($6, a, "/"); print a[1]}'; }
  rcache_src() { findmnt -n -o SOURCE --target "$MNT" 2>/dev/null; }

  start_s15e_daemon || exit 1

  # ---- phase A: first mount, seed content, publish via unmount -------
  # A brand-new volume stages with no origin attach (nothing published
  # yet), and the cycle loop never advances without one. The origin
  # pointer is initialized only by a FRESH-cache stage over published
  # generations — a surviving pool is self-contained and stays
  # origin-less by design — so phase B loses the pool file on purpose:
  # generation A published here, pool dropped, and the fresh restage
  # attaches the lineage's origin export, which is what makes advances
  # (and therefore the swing) reachable.
  "$AGENT" mount wsv "$MNT" --thin --state-dir "$STATE_DIR" \
    || { echo "FAIL: mount --thin refused"; tail -30 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  "$DD" if=/dev/urandom of="$SCEN_DIR/patA" bs=1M count=8 2>/dev/null
  "$DD" if="$SCEN_DIR/patA" of="$MNT/base" bs=1M oflag=direct conv=fsync 2>/dev/null
  "$AGENT" unmount "$MNT" --state-dir "$STATE_DIR" \
    || { echo "FAIL: phase-A unmount refused"; tail -30 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  rm "$POOL_FILE"

  # ---- phase B: origin-attached fresh stage, walk the pool over the
  # watermark, and let the advance swing -------------------------------
  "$AGENT" mount wsv "$MNT" --thin --state-dir "$STATE_DIR" \
    || { echo "FAIL: remount refused"; tail -30 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  # The origin fall-through serves generation A on the fresh pool —
  # also the proof the attach actually happened, without which the
  # swing poll below would time out for a reason worth telling apart.
  "$DD" if="$MNT/base" of="$SCEN_DIR/readback" bs=1M iflag=direct 2>/dev/null
  cmp -s "$SCEN_DIR/patA" "$SCEN_DIR/readback" \
    || { echo "FAIL: fresh pool does not serve generation A through the origin"; exit 1; }
  [ "$(rcache_src)" = "/dev/mapper/sky-wsv-rcache" ] \
    || { echo "FAIL: fs is on '$(rcache_src)', not the rcache"; exit 1; }
  loop_before=$(pool_loop)
  [ -n "$loop_before" ] || { echo "FAIL: no loop backs the pool file"; exit 1; }
  # First batch, then one cadence so a publish re-establishes a local
  # baseline: the advance's reclaim set R is what is SHARED with the
  # baseline, and an advance whose R is 0 is a no-op that never reaches
  # an executor — these blocks are what the swing will reclaim.
  "$DD" if=/dev/urandom of="$SCEN_DIR/patX" bs=1M count=20 2>/dev/null
  "$DD" if="$SCEN_DIR/patX" of="$MNT/warm" bs=1M oflag=direct conv=fsync 2>/dev/null
  sleep 6
  # Second batch crosses the watermark (8+20+32 MiB > 0.4 x 124 MiB):
  # the next cycle promotes to an advance and the strategy override
  # picks the swing.
  "$DD" if=/dev/urandom of="$SCEN_DIR/patY" bs=1M count=32 2>/dev/null
  "$DD" if="$SCEN_DIR/patY" of="$MNT/fill" bs=1M oflag=direct conv=fsync 2>/dev/null
  used_before=$(pool_used)
  # The swing's completion is observed at the loop layer: the teardown
  # releases slot A's loop and the canonicalize renames the `.b` file
  # over the live name, so the pool file answers with a NEW loop device
  # (found by inode — the kernel still advertises the `.b` path).
  swung=0
  for _ in $(seq 1 120); do
    now=$(pool_loop)
    [ -n "$now" ] && [ "$now" != "$loop_before" ] && { swung=1; break; }
    kill -0 "$AGENT_PID" 2>/dev/null || { echo "FAIL: daemon died mid-swing"; \
      tail -40 "$SCEN_DIR/mounts-agent.log"; exit 1; }
    sleep 1
  done
  [ "$swung" = 1 ] || { echo "FAIL: no swing observed (pool loop still $loop_before)"; \
    echo "-- losetup:"; losetup -a; echo "-- pool: $(dmsetup status sky-wsv-pool 2>/dev/null)"; \
    tail -60 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  echo "-- swing observed: pool loop $loop_before -> $now"

  # ---- post-swing invariants -----------------------------------------
  # Canonical world, no slot debris: the B side wears the provision
  # names, the `.b` file is gone, no barrier or `-b` device survives.
  [ ! -e "$POOL_FILE.b" ] || { echo "FAIL: .b file survived the canonicalize"; exit 1; }
  for d in tmeta data pool origin fs rec rcache; do
    dmsetup info "sky-wsv-$d" >/dev/null 2>&1 \
      || { echo "FAIL: canonical device sky-wsv-$d missing after the swing"; dmsetup ls; exit 1; }
  done
  if dmsetup ls | awk '{print $1}' | grep -E '(^sky-wsv.*-b(tm|da)?$|swb)'; then
    echo "FAIL: slot-B debris survived the swing"; exit 1
  fi
  # The swing IS the pool reset (§7): occupancy collapsed to the
  # post-capture dirty tail. Half is a generous bound — ~60 MiB of
  # writes were shared-and-published, and only late writes remain.
  used_after=$(pool_used)
  { [ -n "$used_after" ] && [ "$used_after" -lt $((used_before / 2)) ]; } \
    || { echo "FAIL: pool did not shrink (used $used_before -> ${used_after:-?})"; exit 1; }
  # The mount never noticed: still on the rcache, content bit-exact,
  # Invariant B's guest-facing mask still refusing fstrim (the retarget
  # re-masked the shim; the rcache mask was never dropped), and writes
  # still flowing.
  [ "$(rcache_src)" = "/dev/mapper/sky-wsv-rcache" ] \
    || { echo "FAIL: mount moved off the rcache across the swing"; exit 1; }
  for f in base:patA warm:patX fill:patY; do
    "$DD" if="$MNT/${f%%:*}" of="$SCEN_DIR/readback" bs=1M iflag=direct 2>/dev/null
    cmp -s "$SCEN_DIR/${f##*:}" "$SCEN_DIR/readback" \
      || { echo "FAIL: $MNT/${f%%:*} corrupted across the swing"; exit 1; }
  done
  if fstrim "$MNT" 2>/dev/null; then
    echo "FAIL: guest fstrim succeeded after the swing (Invariant B)"; exit 1
  fi
  { echo postswing > "$MNT/alive" && sync "$MNT/alive"; } \
    || { echo "FAIL: post-swing write refused"; exit 1; }

  # ---- phase C: the post-swing stack publishes and survives a full
  # unmount/remount — D-M13 on the canonicalized pool, and the unstage
  # detaching the loop the FILE answers for (the record is stale) ------
  "$AGENT" unmount "$MNT" --state-dir "$STATE_DIR" \
    || { echo "FAIL: post-swing unmount refused"; tail -40 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  losetup -j "$POOL_FILE" | grep -q . \
    && { echo "FAIL: unstage left the pool loop attached (stale-record detach?)"; losetup -a; exit 1; }
  "$AGENT" mount wsv "$MNT" --thin --state-dir "$STATE_DIR" \
    || { echo "FAIL: post-swing remount refused"; tail -40 "$SCEN_DIR/mounts-agent.log"; exit 1; }
  for f in base:patA warm:patX fill:patY; do
    "$DD" if="$MNT/${f%%:*}" of="$SCEN_DIR/readback" bs=1M iflag=direct 2>/dev/null
    cmp -s "$SCEN_DIR/${f##*:}" "$SCEN_DIR/readback" \
      || { echo "FAIL: $MNT/${f%%:*} wrong after the post-swing remount"; exit 1; }
  done
  [ "$(cat "$MNT/alive")" = postswing ] || { echo "FAIL: post-swing write lost"; exit 1; }
  echo "-- post-swing publish + remount: all generations intact"

  "$AGENT" unmount "$MNT" --state-dir "$STATE_DIR" || true
  s15e_cleanup
  trap 'rc=$?; cleanup' EXIT
  echo "S15e PASS"
}

[ -n "${E2E_TRACE:-}" ] && set -x

[ $# -ge 1 ] || { echo "usage: $0 <S1..S14|S14a|S15b|S15e|S15i|S15j|S15k|FREEZEPROBE> [...]"; exit 64; }
build_daemon
for s in "$@"; do
  case "$s" in
    S1|s1) s1 ;;
    S2|s2) s2 ;;
    S3|s3) s3 ;;
    S5|s5) s5 ;;
    S6|s6) s6 ;;
    S7|s7) s7 ;;
    S8|s8) s8 ;;
    S11|s11) s11 ;;
    S12|s12) s12 ;;
    S13|s13) s13 ;;
    S4|s4) s4 ;;
    S10|s10) s10 ;;
    S14a|s14a) s14a ;;
    S15b|s15b) s15b ;;
    S15i|s15i) s15i ;;
    S15j|s15j) s15j ;;
    S15k|s15k) s15k ;;
    S15e|s15e) s15e ;;
    FREEZEPROBE|freezeprobe) freezeprobe ;;
    S1u|s1u) s1u ;;
    S14|s14) s14 ;;
    S9|s9) s9 ;;
    *) echo "unknown or not-yet-implemented scenario: $s (see design §14.4)"; exit 64 ;;
  esac
  # DETECT, then clean — in that order, and the order is the point.
  #
  # This exists because of how the one leak we had actually presented.
  # S15j finished PASS holding /dev/nbd0; S14 ran next, could not claim
  # the device, and failed at its attach — so the run blamed S14, the
  # blame was written down as "S14's freeze window exceeds budget is an
  # outstanding failure on main", and that misdiagnosis then drove a
  # scenario-ordering decision in the nightly workflow.
  #
  # Cleaning ALONE would fix the cascade and leave nothing behind: the
  # next scenario stops failing, the leaking one still says nothing, and
  # a regression here goes green and unrecorded. Cleaning does not make
  # a leak attributable — it makes it silent, which beats misattributed
  # and is still not a signal. The warning is the signal; the clean is
  # what stops one scenario's bug from becoming the next one's.
  #
  # The probe reads the KERNEL, not `NBD_ATTACHED`. The array records
  # what a scenario meant to hold, and a leak is precisely the case
  # where intent and reality diverged, so testing the bookkeeping is
  # circular — a near variant of the bug this fixes (S15j clearing the
  # array without disconnecting) would read as clean. The array happens
  # to be in sync at every call site today; that is a property worth
  # verifying, not one to build a detector on. `/sys/block/nbdN/pid`
  # exists only while the device is genuinely connected, which is how
  # tests/k3s/smoke.sh already asks this question.
  #
  # Warned, not fatal: a cleanup improvement should not invent a new way
  # for the suite to fail. Every scenario does release today, so a
  # warning here is a regression by definition — promoting it to an
  # error is the honest next step once it has been quiet for a while.
  for p in /sys/block/nbd*/pid; do
    [ -e "$p" ] || continue
    leaked=${p%/pid}
    echo "WARNING: scenario $s left /dev/${leaked##*/} connected — released \
here, but it is $s's bug, not the next scenario's"
  done
  # The same bug one layer up, found exactly the way the comment above
  # predicts a leak presents (blamed on the NEXT scenario first): a
  # scenario whose success path forgets its skynbdd leaves it answering
  # /healthz on the shared harness port, and the next scenario's
  # start_daemon takes that answer as its own daemon coming up — the
  # whole scenario then runs against a server with the wrong volumes.
  # S15k and S15e shipped this; S15k→S15e survived by coincidence (both
  # define volume "wsv"), S15e→S14 died at its attach with "Unknown
  # error returned by server". Same posture as the device probe: the
  # WARNING is the signal and it names the leaking scenario; the kill is
  # what stops its bug from becoming the next scenario's. The port
  # probe, not $DAEMON_PID bookkeeping, is what detects — the variable
  # records intent, and a leak is intent diverging from reality.
  if curl -sf "$API/healthz" >/dev/null 2>&1; then
    echo "WARNING: scenario $s left a daemon answering on $API — killed \
here, but it is $s's bug, not the next scenario's"
    [ -z "$DAEMON_PID" ] || kill -9 "$DAEMON_PID" 2>/dev/null || true
    DAEMON_PID=""
    # Backstop for a daemon whose pid the bookkeeping lost: kill the
    # listener itself (psmisc is in the nightly's tool set).
    fuser -k "${API_PORT}/tcp" 2>/dev/null || true
    sleep 0.3
  fi
  # Narrower than "clean up after the scenario" suggests: this covers
  # mounts, the VG and nbd devices — NOT the dm targets or loop devices
  # every Tier B scenario also creates. Those are removed by each
  # scenario's own teardown, and a scenario that fails `exit`s the
  # script so the EXIT trap reaches them, which is why that gap has not
  # bitten. Do not read this call as covering the dm stack.
  tierb_cleanup
done

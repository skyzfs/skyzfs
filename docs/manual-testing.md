# Manual Testing & Debugging Guide

Hands-on recipes for poking a local skynbd with standard CLI tools. The
automated equivalents live in `tests/e2e/run.sh` (scenarios S1–S14) — when a
manual session finds a bug, turn the reproduction into a scenario.

Everything in **Tier A needs no root**. Tier B (kernel NBD, filesystems,
LVM) is at the end.

## 1. Prerequisites

```sh
# Build (S3 feature on so both store backends are available):
cargo build -p skynbdd -p skynbdctl --features skynbdd/s3 --features skynbdctl/s3

# Client tools (Debian/Ubuntu names):
sudo apt-get install -y qemu-utils libnbd-bin   # qemu-io, qemu-img, nbdinfo
pip install blake3                              # for ingestion recipes / coordinator
```

## 2. An object store to talk to

Three options, in order of effort:

```sh
# (a) none — `store.kind = "memory"`: everything is RAM, lost on exit.
#     Fine for NBD/API poking; useless for restart/crash testing.

# (b) moto (pure-python S3, what this repo uses in sandboxes):
pip install 'moto[server]'
python3 -m moto.server -p 9010 &

# (c) MinIO (what CI uses; minio/minio left Docker Hub, so this is the
#     community fork pgsty/minio):
docker run -d --name minio -p 9000:9000 \
  -e MINIO_ROOT_USER=minioadmin -e MINIO_ROOT_PASSWORD=minioadmin \
  pgsty/minio:RELEASE.2026-08-04T00-00-00Z server /data
```

## 3. Config and daemon

`/tmp/sky/skynbd.toml` — a writable volume plus a lineage volume:

```toml
nbd_listen = "127.0.0.1:10809"
api_listen = "127.0.0.1:8080"
refs_path  = "/tmp/sky/refs.redb"      # omit → in-memory refs (no restarts)
# refs_kind = "object"                 # refs in the bucket itself (multi-node);
# node_id   = "dev-1"                  #   this server's lease identity (D-M3)
# lease_ttl_s = 30                     #   steal-after silence window
# refs_export = "cadence"              #   D-M4: head exports on the interval,
# refs_export_interval_s = 300         #   not per flush; crash gap closed at serve

[store]
kind       = "s3"                      # or "memory"
endpoint   = "http://127.0.0.1:9010"
bucket     = "skynbd-dev"              # created on demand
access_key = "test"                    # moto accepts anything;
secret_key = "test"                    # MinIO: minioadmin/minioadmin
region     = "us-east-1"
# Omit access_key/secret_key to use the ambient AWS credential chain
# (web identity, env keys, profile, IMDS) — auth design §5.

[[volumes]]
name = "vol1"
size = 67108864                        # 64 MiB

[[volumes]]
name = "vlin"                          # ingestion target: no writable head
size = 67108864                        # = the agent's origin size in §10
kind = "lineage"

[rpo]
interval_s = 0                         # >0 → periodic vol@rpo advance (§9.3)
```

```sh
mkdir -p /tmp/sky
RUST_LOG=info ./target/debug/skynbdd /tmp/sky/skynbd.toml
# RUST_LOG=debug for per-request noise; RUST_LOG=skynbd_nbd=trace to focus.
# Ctrl-C checkpoints all volumes before exit (clean shutdown).
```

Export names: `vol1/head` (writable), `vol1/snap/<name>` and `vol1/rpo`
(read-only, appear when they exist). Lineage volumes export snapshots only.

## 4. Poking the NBD side (Tier A, no root)

```sh
URI=nbd://127.0.0.1:10809/vol1/head

nbdinfo --list nbd://127.0.0.1:10809      # all exports + sizes + flags
nbdinfo "$URI"                            # one export incl. block-status support

# Write / read / flush / trim / zero:
qemu-io -f raw "$URI" \
  -c 'write -P 0xab 0 1M' -c 'flush' \
  -c 'read -P 0xab 0 1M' \
  -c 'write -z 256k 128k' \
  -c 'discard 512k 128k'

# Allocation map (BLOCK_STATUS; holes come from trim/zero):
qemu-img map --output=json "$URI" | head

# Full image off any export (this is how snapshots are verified everywhere):
qemu-img convert -f raw -O raw "nbd://127.0.0.1:10809/vol1/snap/s1" /tmp/s1.img
```

Mismatch hunting: `qemu-io -c 'read -v <off> <len>'` hex-dumps a range;
compare against `xxd -s <off> -l <len> /tmp/s1.img`.

## 5. REST cookbook (Tier A)

`API=http://127.0.0.1:8080`. All bodies JSON. Errors map to 404/409/400.

```sh
curl -s $API/healthz
curl -s $API/v1/volumes | jq          # name, size, flush_seq
curl -s $API/v1/exports | jq          # what the NBD side is serving

# Dynamic volumes (design §12.1 — what the CSI controller calls).
# Idempotent: identical replay → 200; same name, other size → 409.
# Cataloged in the refs store, so they survive a daemon restart.
curl -s -X POST $API/v1/volumes \
  -H 'content-type: application/json' \
  -d '{"name":"pvc-1","size":67108864}' | jq   # + optional "kind":"lineage"
curl -s -X DELETE $API/v1/volumes/pvc-1        # 204; config volumes → 409

# D-M6 handoff (what NodeUnstage/NodeStage call; meaningful with
# refs_kind = "object", where a second server can pick the volume up):
curl -s -X POST $API/v1/volumes/pvc-1/release  # commit + stop serving + drop lease
curl -s -X POST $API/v1/volumes/pvc-1/serve | jq  # re-serve from the catalog; 409 if leased elsewhere

# Rollback head to a snapshot (refuses while dirty — flush first):
curl -s -X POST $API/v1/volumes/vol1/rollback \
  -H 'content-type: application/json' -d '{"snapshot":"s1"}'

# Maintenance:
curl -s -X POST $API/v1/volumes/vol1/fsck | jq    # closure walk + checksums
curl -s -X POST $API/v1/volumes/vol1/rpo          # advance vol1/rpo now

# SKYD1 streams (§9.4) — replicate vol1 snapshots into vlin:
curl -s "$API/v1/volumes/vol1/diff?to=s1" -o /tmp/base.skyd          # baseline
curl -s "$API/v1/volumes/vol1/diff?from=s1&to=s2" -o /tmp/delta.skyd # incremental
curl -s -X POST --data-binary @/tmp/base.skyd $API/v1/volumes/vlin/import | jq
```

Cert enrollment (auth design §3.2 — needs `[api.enroll]` in the config
and an OIDC token whose binding grants the `enroll` op; static/disabled
auth cannot enroll because the SAN comes from the *verified* `sub`):

```sh
openssl req -new -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes \
  -keyout client.key -out client.csr -subj "/CN=ignored"   # CN is ignored
jq -n --rawfile csr client.csr '{csr_pem: $csr}' \
  | curl -s -X POST $API/v1/certs/enroll \
      -H "authorization: Bearer $TOKEN" \
      -H 'content-type: application/json' -d @- | jq
# → cert_pem (URI SAN spiffe://<trust_domain>/<sub path>), ca_pem,
#   identity, not_after. Pair cert_pem with client.key for NBD mTLS;
#   the CA cert must be in [nbd.tls] client_ca_paths.
```

Snapshot creation/deletion, pins, and GC have their own walkthrough: §6.

## 6. Snapshot lifecycle: create, delete, GC (Tier A)

Snapshots are commits with a name: creation is O(1) (design §7), the
data is shared content-addressed between snapshots and the head, deletion
only drops the ref, and space returns when GC sweeps objects nothing
reachable references anymore (design §10). The whole lifecycle is
observable from outside — use an S3 store (not `memory`) so the object
counts below mean something.

```sh
API=http://127.0.0.1:8080
URI=nbd://127.0.0.1:10809/vol1/head

# 1. Write something, then snapshot. Creation forces a commit and writes
#    a manifest — no data copy, O(1) regardless of volume size:
qemu-io -f raw "$URI" -c 'write -P 0x11 0 4M' -c 'flush'
curl -s -X POST $API/v1/volumes/vol1/snapshots \
  -H 'content-type: application/json' -d '{"name":"s1"}' | jq
curl -s $API/v1/volumes/vol1/snapshots | jq   # [{name, commit}]

# The read-only export appears within ~1s (registry sync):
nbdinfo --list nbd://127.0.0.1:10809 | grep export=   # … vol1/snap/s1

# 2. Overwrite part of it + write new data, snapshot again:
qemu-io -f raw "$URI" -c 'write -P 0x22 1M 1M' -c 'write -P 0x33 8M 2M' -c 'flush'
curl -s -X POST $API/v1/volumes/vol1/snapshots \
  -H 'content-type: application/json' -d '{"name":"s2"}' | jq

# Sharing is real: only the CHANGED data hit the store. Count data
# segments in the bucket (§9 layout) between the two snapshots — e.g.
# 4 seg objects after s1, 7 after s2 for the writes above — not 2×.

# 3. Each snapshot is its own frozen image. NOTE: snapshot exports are
#    read-only — qemu-io needs -r or it fails with "Permission denied":
qemu-io -r -f raw nbd://127.0.0.1:10809/vol1/snap/s1 -c 'read -P 0x11 1M 1M'
qemu-io -r -f raw nbd://127.0.0.1:10809/vol1/snap/s2 -c 'read -P 0x22 1M 1M'
qemu-img convert -f raw -O raw nbd://127.0.0.1:10809/vol1/snap/s2 /tmp/s2.img

# 4. Pins protect a snapshot from deletion (I8) — this is how the agent
#    guards its baseline:
curl -s -X POST $API/v1/volumes/vol1/snapshots/s1/pins \
  -H 'content-type: application/json' -d '{"holder":"me"}'
curl -s -X DELETE $API/v1/volumes/vol1/snapshots/s1 \
  -o /dev/null -w '%{http_code}\n'                       # → 409
curl -s $API/v1/volumes/vol1/snapshots/s1/pins | jq      # who holds it
curl -s -X DELETE $API/v1/volumes/vol1/snapshots/s1/pins/me

# 5. Delete: drops the ref and the export; the data stays until GC.
curl -s -X DELETE $API/v1/volumes/vol1/snapshots/s1     # → 204
nbdinfo --list nbd://127.0.0.1:10809 | grep export=     # s1 gone

# 6. GC: mark from every live ref (head, snapshots, rpo, prepared
#    ingestion roots + their EnsureObject promises), sweep the rest.
#    Refuses with 409 if the head overlay is dirty (same rule as
#    rollback) — rarely seen from these recipes, since a disconnecting
#    NBD client leaves the overlay flushed.
curl -s -X POST $API/v1/volumes/vol1/gc | jq
# → {"refs_scanned":2,"live_objects":11,"swept_objects":22}  (example run)

# 7. Trust, then verify: everything still reachable is intact.
curl -s -X POST $API/v1/volumes/vol1/fsck | jq          # {"refs":[…]}, no errors
qemu-io -r -f raw nbd://127.0.0.1:10809/vol1/snap/s2 -c 'read -P 0x11 0 1M'
```

Notes:

- `swept_objects` counts store objects removed (data segments AND the
  dead snapshot's nodes/commits/manifest), so it is normally larger than
  the segment count alone.
- GC never touches anything a ref can reach — deleting `s1` above cannot
  perturb `s2` even though they share most of their data; step 7 is the
  proof.
- A snapshot that is the base of an in-flight prepared ingestion session
  is protected the same way pins are (the prepared root is a GC root).
- Lineage volumes get snapshots via publish (§7), not this POST; delete +
  pins + GC work the same there.

On-demand snapshot requests (the CSI thin path, multi-node design
D-M12) are observable while pending — this is how you tell "uploading
40 GB" from "wedged on a freeze":

```sh
# File a request (what the CSI controller does on CreateSnapshot):
curl -s -X POST $API/v1/volumes/vol1/snapshot-requests \
  -H 'content-type: application/json' -d '{"name":"csi-s1"}'

# Its cycle status: phase + backfill ops from the agent's progress
# reports, attempts/last_error from its failure reports. 404 once the
# request completes (readiness = the named snapshot exists).
curl -s $API/v1/volumes/vol1/snapshot-requests/csi-s1 | jq
# → {"phase":"backfilling","ops_done":128,"ops_total":4096,
#    "attempts":0,"last_error":null}

# What the agent sends (volume-level; the server stamps every pending
# request of the volume) — useful for faking a wedged cycle:
curl -s -X POST $API/v1/volumes/vol1/cycle-status \
  -H 'content-type: application/json' \
  -d '{"attempts":3,"last_error":"freeze: device busy"}'
```

The origin pointer (multi-node design D-M15): `<vol>/origin` is a
read-only NBD export serving whatever generation the pointer names, and
repointing it swaps the backing live — attached clients follow without
reconnecting:

```sh
# Resolved pointer. initialized=false means the row is absent/stale and
# this is the newest-generation fallback an init would adopt:
curl -s $API/v1/volumes/vol1/origin | jq
# → {"ref":"snap/g3","commit":"…","export":"vol1/origin","initialized":true}

# Initialize (empty body: adopt current row, else newest generation).
# Publishes the vol1/origin export; until this, the export doesn't exist:
curl -s -X POST $API/v1/volumes/vol1/origin \
  -H 'content-type: application/json' -d '{}' | jq

# Explicit repoint (what the agent's advance cycle sends). The pointer
# row, its dedicated "origin" pin, and the live export move atomically:
curl -s -X POST $API/v1/volumes/vol1/origin \
  -H 'content-type: application/json' -d '{"ref":"snap/g4"}' | jq
curl -s $API/v1/volumes/vol1/snapshots/g4/pins | jq   # → ["origin"]
```

## 7. Ingestion plane by hand (§9A — what the agent automates)

The fastest path is the Tier A helper (does hashing/chunking for you):

```sh
python3 tests/e2e/coordinator.py upload $API vlin /path/to/image.img g0
python3 tests/e2e/coordinator.py upload $API vlin /path/to/image2.img g1 --base g0
# → prints {"published": ..., "deduped_objects": N} ; exports vlin/snap/g0…
```

Or fully manual, one 64 KiB chunk, to see every op:

```sh
# 1. Prepare a hidden root from base g0 (omit "base" for bootstrap):
PID=$(curl -s -X POST $API/v1/volumes/vlin/prepared \
  -H 'content-type: application/json' -d '{"base":"g0"}' | jq -r .snap_id)

# 2. Hash a chunk, ask the server if it already has it:
H=$(python3 -c "import blake3,sys;print(blake3.blake3(open('chunk.bin','rb').read()).hexdigest())")
curl -s -X POST $API/v1/volumes/vlin/prepared/$PID/ensure \
  -H 'content-type: application/json' -d "{\"hashes\":[\"$H\"]}"   # {"exists":[false]}

# 3. Upload (server verifies the hash) and attach at volume offset 0:
curl -s -X PUT --data-binary @chunk.bin $API/v1/volumes/vlin/objects/$H
curl -s -X POST $API/v1/volumes/vlin/prepared/$PID/attach \
  -H 'content-type: application/json' \
  -d "{\"op_id\":1,\"ranges\":[{\"off\":0,\"len\":65536,\"hash\":\"$H\"}]}"

# Idempotency check: repeat the attach with the same op_id after the
# watermark advances → {"applied": false}. Discards use /clear the same way.

# 4. Status / publish / abort:
curl -s $API/v1/volumes/vlin/prepared/$PID | jq    # op_watermark = durable
curl -s -X POST $API/v1/volumes/vlin/prepared/$PID/publish \
  -H 'content-type: application/json' -d '{"name":"g1","holder":"me"}'
# (or) curl -s -X DELETE $API/v1/volumes/vlin/prepared/$PID

# Until publish, g1 is invisible everywhere (I9); after, it exports within 1s.
```

## 8. Crash & recovery drills (Tier A)

```sh
# Durability (S2 shape): write+flush, kill -9, restart, verify:
qemu-io -f raw "$URI" -c 'write -P 0x5a 0 1M' -c flush
kill -9 $(pgrep skynbdd)
RUST_LOG=info ./target/debug/skynbdd /tmp/sky/skynbd.toml &
qemu-io -f raw "$URI" -c 'read -P 0x5a 0 1M'      # must pass

# Backfill crash (S13 shape): kill -9 mid-attach-loop, restart, read the
# prepared status — replay everything with op_id > op_watermark, publish.

# Ref-store loss (S9b): stop daemon, delete refs, rebuild from the bucket:
rm /tmp/sky/refs.redb
./target/debug/skynbdctl fsck /tmp/sky/skynbd.toml vol1 --rebuild-refs

# Total loss incl. the volume *names* (D-M9): the name-index markers make
# the catalog reconstructible, and --with-refs rebuilds each head too:
./target/debug/skynbdctl rebuild-catalog /tmp/sky/skynbd.toml --with-refs
# Online equivalent: curl -s -X POST $API/v1/catalog/rebuild | jq
./target/debug/skynbdctl fsck /tmp/sky/skynbd.toml vol1        # verify-only
```

## 9. Looking at the raw state

```sh
# Bucket layout: skynbd/v1/<vol_ulid>/{sb,seg,node,commit,snap,obj}/...
AWS_ACCESS_KEY_ID=test AWS_SECRET_ACCESS_KEY=test \
  aws --endpoint-url http://127.0.0.1:9010 s3 ls --recursive s3://skynbd-dev/ | head -30

# Every object is an 8-byte frame (magic|ver|flags) + postcard. Quick peek:
aws ... s3 cp s3://skynbd-dev/skynbd/v1/<vol>/commit/<hash> - | xxd | head -3
# → magics: SSEG SNOD SCMT SSUP SSNP (design §5.2); SAGT = agent state files.

# Volume ULID ↔ name: the id is deterministic — blake3(name)[..16] as a ULID
# (bins/skynbdd derivation) — so the same config always maps to the same keys.
```

The refs.redb file is not meant to be read by hand — `skynbdctl fsck`
(verify mode) prints head/snapshot refs and validates closures instead.

## 10. Tier B: kernel NBD + the production agent (root, dev box)

Never on a machine you care about; a throwaway VM is ideal. The automated
versions are `sudo -E tests/e2e/run.sh S4 S14a S14 S10`; the recipes below
are the manual S14a shape (same moves, human pace).

### 10.1 The production path: `skynbd-agent once` on a raw dm-thin pool

The agent does the whole V3.2 loop — freeze, dm-thin snapshot, delta via
linked thinp, ship only changed chunks, publish, promote — against the
lineage volume `vlin` from §3. No lvm2, no thin-provisioning-tools.

```sh
sudo apt-get install -y nbd-client dmsetup fio
sudo modprobe nbd max_part=8
cargo build -p skynbd-agent-cli             # build headers: libdevmapper-dev libudev-dev
                                            # (build-time only; the binary links neither)

# A thin pool from two loop files — tmeta local (V3.2 §6.5), 64 KiB blocks
# (128 sectors; MUST equal the agent's chunk_bytes):
truncate -s 64M /tmp/tmeta.img && META=$(sudo losetup -f --show /tmp/tmeta.img)
truncate -s 256M /tmp/tdata.img && DATA=$(sudo losetup -f --show /tmp/tdata.img)
sudo dd if=/dev/zero of=$META bs=4096 count=1 conv=notrunc
sudo dmsetup create skypool --table "0 $(sudo blockdev --getsz $DATA) thin-pool $META $DATA 128 32"
sudo dmsetup message skypool 0 "create_thin 1"
sudo dmsetup create skyorigin --table "0 131072 thin /dev/mapper/skypool 1"  # 64 MiB = vlin size

sudo mkdir -p /mnt/skythin && sudo mkfs.ext4 -q /dev/mapper/skyorigin
sudo mount /dev/mapper/skyorigin /mnt/skythin
echo hello | sudo tee /mnt/skythin/f1 >/dev/null && sync
```

`agent.toml` — `size_bytes` must equal the server volume's size,
`chunk_bytes` the pool block size, and `state_dir` must NOT live on the
filesystem being frozen (preflight enforces both):

```toml
server    = "http://127.0.0.1:8080"
state_dir = "/var/lib/skynbd-agent"
# token_path = "/etc/skynbd/token"   # if the server enables bearer auth
#   (server side: `api_auth_token_path` or `[api.auth]` — static token or
#   OIDC issuers with bindings, auth design §4; the agent re-reads the
#   file per request, so rotated projected tokens are picked up live)

[[volume]]
name           = "vlin"
pool_name      = "skypool"
origin_name    = "skyorigin"
origin_dev_id  = 1
metadata_dev   = "/dev/loop0"        # the $META loop device
mountpoint     = "/mnt/skythin"
size_bytes     = 67108864
chunk_bytes    = 65536
freeze_budget_ms = 5000
# interval_s = 900                   # cadence for `run` mode
```

```sh
sudo ./target/debug/skynbd-agent preflight agent.toml   # placement + chunk rules
sudo ./target/debug/skynbd-agent once agent.toml        # bootstrap → "published snap/g0"

# Mutate, cycle again — only the changed chunks travel:
sudo dd if=/dev/urandom of=/mnt/skythin/f2 bs=64K count=8; sync
sudo ./target/debug/skynbd-agent once agent.toml        # → "published snap/g1"

sudo ./target/debug/skynbd-agent status agent.toml      # phase, baseline pair
sudo ./target/debug/skynbd-agent state-dump agent.toml | jq   # raw SAGT record
# `run` instead of `once` = daemon mode (recover, then cycle every interval_s);
# `abort` clears a stuck prepared session after quarantine.

# Verify a published generation exactly like the rig does:
sudo nbd-client -N vlin/snap/g1 127.0.0.1 10809 /dev/nbd1 -b 4096
sudo fsck.ext4 -n -f /dev/nbd1              # must be clean
sudo mkdir -p /mnt/skysnap && sudo mount -o ro /dev/nbd1 /mnt/skysnap
diff /mnt/skythin/f1 /mnt/skysnap/f1

# Inspect a delta without running a cycle. Both args are *managed*
# snapshot logical names; after a cycle only the current baseline device
# survives (promotion deletes the old one — `state-dump` shows its name
# under baseline.local.lv_name), so snapshot a candidate yourself first:
sudo fsfreeze -f /mnt/skythin
sudo dmsetup message skypool 0 "create_snap 99 1"
sudo fsfreeze -u /mnt/skythin
sudo dmsetup create sky-vlin-c-probe --uuid "SKYNBD-vlin-99" \
  --table "0 131072 thin /dev/mapper/skypool 99"
sudo ./target/debug/skynbd-agent debug-delta agent.toml g1 probe
# (the S14a rig cross-checks this output against the thin_delta binary)
```

Crash-recovery drill: `kill -9` the agent mid-`once`, then run `once`
again — recovery replays from the durable state + op watermark and the
interrupted generation either publishes or is cleanly abandoned (this is
exactly what `crash_matrix_dm.rs` does at every crash point).

### 10.2 The mechanism by hand (S4 shape — what the agent automates)

```sh
sudo nbd-client -N vol1/head 127.0.0.1 10809 /dev/nbd0 -b 4096
sudo mkfs.ext4 -q /dev/nbd0 && sudo mount /dev/nbd0 /mnt/skytest

sudo fsfreeze -f /mnt/skytest
curl -s -X POST $API/v1/volumes/vol1/snapshots -H 'content-type: application/json' -d '{"name":"fs1"}'
sudo fsfreeze -u /mnt/skytest
sudo nbd-client -N vol1/snap/fs1 127.0.0.1 10809 /dev/nbd2 -b 4096
sudo fsck.ext4 -n -f /dev/nbd2              # must be clean
sudo mount -o ro /dev/nbd2 /mnt/skysnap     # compare files at will
```

The legacy LVM reference path (Python coordinator, S14 shape) lives in
`s14()` in `tests/e2e/run.sh` and
`tests/coordinator/coordinator_v32.py` (`selftest` runs rootless).

### 10.3 Cleanup (order matters — see `tierb_cleanup()` in run.sh)

```sh
sudo umount /mnt/skysnap /mnt/skythin /mnt/skytest
sudo nbd-client -d /dev/nbd2; sudo nbd-client -d /dev/nbd1; sudo nbd-client -d /dev/nbd0
sudo dmsetup remove skyorigin; sudo dmsetup ls | grep ^sky-vlin- | awk '{print $1}' \
  | xargs -rn1 sudo dmsetup remove          # agent-managed sky-vlin-{b,c,u}-* snapshots
sudo dmsetup remove skypool
sudo losetup -d $META $DATA
```

## 11. Debugging tips & known sharp edges

- **First stop is always the daemon log.** `RUST_LOG=debug` per-module:
  `RUST_LOG=info,skynbd_engine=debug,skynbd_nbd=trace`. E2e runs write
  `daemon.log` into per-scenario dirs under `/tmp/skynbd-e2e-*/`.
- **One daemon per refs.redb, one bucket prefix per daemon.** Two daemons
  sharing refs or a bucket produce CAS-conflict panics by design (single
  writer, I2). The e2e harness isolates per scenario for this reason.
- **`409 Conflict`** on snapshot delete = pinned (list pins);
  on rollback / gc = dirty head (flush via `qemu-io -c flush` first);
  on prepared create = a prepared root already exists for that volume
  (one-in-flight, §7.9 — status or abort it).
- **Restart testing requires `refs_path` + an S3-backed store.** With
  `kind = "memory"` a restart is a wipe; nothing to recover.
- **`qemu-img convert` right after publish**: export registration is
  watch-driven off the ref store, so it is near-instant — but it is still
  asynchronous; an immediate read can race it (the e2e scripts sleep 0.2 s).
- **Ports**: NBD 10809 / API 8080 defaults collide with a second instance —
  override both in the config. e2e picks random ports (20000+).
- **Reproduce a CI scenario locally**: `SKYNBD_S3_TEST_ENDPOINT=... \
  tests/e2e/run.sh S11` — each scenario is self-contained and prints the
  daemon log on failure. `E2E_SKIP_BUILD=1` reuses the last build.
- **Turn any manual find into a test**: crash-point bugs → engine crash
  matrix; protocol bugs → `crates/skynbd-nbd` golden tests; loop/agent bugs
  → `skynbd-agent` crash matrix (see `tests/README.md`).

## 12. Test coverage

`cargo-llvm-cov` is wired in (CI job `coverage`, report-only — full lcov
artifact + two summaries in the job's step summary). Locally:

```sh
cargo install cargo-llvm-cov --locked
rustup component add llvm-tools-preview   # if this download fails, use the
                                          # system fallback below

cargo llvm-cov --workspace --summary-only            # per-file table
cargo llvm-cov --workspace --html                    # browsable report
cargo llvm-cov report --summary-only \
  --ignore-filename-regex '(^|/)bins/|src/dm\.rs'    # library-only view

# System-LLVM fallback (needs the *matching* major version — rustc 1.95
# emits profraw v10, which wants llvm-19 tools):
LLVM_COV=/usr/bin/llvm-cov-19 LLVM_PROFDATA=/usr/bin/llvm-profdata-19 \
  cargo llvm-cov --workspace --summary-only
```

Reading the two views: the **full-workspace** number includes the three
binaries (0% — they are exercised by the e2e scenarios, which run
uninstrumented release-style processes) and `dm_linux.rs` (the kernel
glue below the M9 `DmOps` seam, only executed in the nightly Tier B rig;
the `dm.rs` orchestration above the seam is covered rootlessly via
`MockDmOps`). The **library-only** view is the number that tracks
unit/property/integration test quality; CI enforces
`--fail-under-lines 85` on it (excluding `bins/` and `dm_linux.rs`).

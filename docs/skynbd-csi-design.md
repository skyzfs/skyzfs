# SkyNBD CSI: data path, snapshots, clones, and workstation mounts

Status: roadmap for finishing rollout item 6 (auth design §7). Items
6(a) volume lifecycle API, 6(b) driver/images/chart, 6(c) k3s smoke +
issuer `tls_ca`, 6(d) CSI snapshots, 6(e) RO snapshot mounts, 6(f)
server fast clone, and 6(g) CSI clone-backed volumes are merged.
6(h) thin-stack node mode ships in two PRs — 6h-1 stage/unstage (the
Shape-B stack build, `mode: thin`, node-sticky topology, flush-on-
unstage; merged) and 6h-2 the embedded agent cycle (fs-consistent
snapshots into lineage volumes); 6(i) is the workstation CLI. §2.1
(node-local Unix data path) lands after them, on top of the same node
plugin. This
document breaks down the rest and records the design decisions they
implement. Companion documents: `docs/skynbd-auth-design.md` (identity,
enrollment, ACLs), `docs/skynbd-snapshot-design-v3_2.md` ("V3.2": the
thin-stack snapshot architecture), `docs/skynbd_design.md` (shapes A/B,
server engine).

## 1. Where we are

The merged driver (6b) implements what `docs/skynbd_design.md` calls
**Shape A**: NodeStage attaches `<volume>/head` on a raw `/dev/nbdX`
(mTLS via enrollment), formats, mounts. That is correct for simple
consumers but deliberately incomplete:

- **No CSI snapshots** — the control plane has snapshot APIs, but the
  driver answers `UNIMPLEMENTED`.
- **Raw NBD, not the thin stack** — Shape A gives no local dm-thin
  snapshot capability and no dm-cache writeback, so every byte round
  trips to the server and fs-consistent snapshots (V3.2 §9) are
  impossible. The production data path per V3.2 §6.5 is **Shape B**:
  NBD head = thin-pool *data device* under **dm-cache (writeback)**,
  local thin metadata, fs-consistent snapshots through the ingestion
  plane into a lineage volume.
- **No workstation story** — `skynbd-agent-cli` is the snapshot-cycle
  agent (run/once/status), not a "list and mount a volume here" tool.
- **No clone-vs-RO distinction** — mounting a snapshot read-only and
  fast-cloning a snapshot into a new writable volume are different
  operations with different costs; neither is exposed.

## 2. Node data-path modes

The driver gains a per-StorageClass `mode` parameter:

| mode | stack (top→bottom) | snapshots | write cost | intended use |
|---|---|---|---|---|
| `raw` (today) | fs → /dev/nbdX (mTLS) | server-native, crash-consistent | every write to server | simple/dev volumes, RO snapshot mounts |
| `thin` (V3.2 Shape B) | fs → thin LV → thin pool → dm-cache (writeback, local fast dev) → /dev/nbdX | fs-consistent via fsfreeze + thin snapshot + ingestion plane (V3.2 §9) | absorbed locally, coalesced writeback | production PVs |

Thin-mode notes (all from V3.2, not new design):

- The NBD head backs the **pool data device**; its server-side bytes are
  pool-physical and opaque (V3.2 §15.7) — server-native snapshots of a
  thin-mode head are *not* taken. Logical snapshots flow through the
  ingestion plane into the volume's **lineage volume**, which is where
  RO exports and clones come from.
- dm-cache's fast device comes from a node-local resource declared in
  chart values (`node.cache.device` hostPath LV/partition, or a
  fixed-size loop file for dev). Cache loss ⇒ head untrustworthy ⇒
  rehydrate from lineage (V3.2 §15.7); the driver must surface this as
  a volume condition, not hide it.
- Pool layout, sizing, and metadata placement follow V3.2 §6.5/§8; the
  driver reuses `crates/skynbd-agent`'s `DmStack`/`LocalStack` code
  rather than shelling its own dm-setup, and runs the agent cycle
  (crates/skynbd-agent `run` mode) per staged thin volume so snapshot
  publication is continuous.
- **External-origin restage** (multi-node design D-M11): a thin stage
  onto a **fresh** cache file — a rescheduled or rebuilt node — clones
  the lineage's newest published generation as `<vol>-org`, seeds the
  agent's baseline on that generation, and attaches the clone's head
  as a second, read-only-by-role NBD device wired into the thin table
  as the external origin. Unprovisioned blocks read through, writes
  provision locally, and the next cycle publishes base ⊕ delta — so a
  planned reschedule stages O(1) with the last published generation's
  content, and the §15.7 rehydration error's recovery ("delete the
  stale binding, re-stage") lands here instead of on a blank pool.
  Cost: two NBD attaches per rescheduled thin volume, and the node
  identity needs the `clone` op on lineage volumes (plus its existing
  `volume` grant, which covers the clone's create/delete/serve).

### 2.1 Data-path transport: TCP+mTLS or node-local Unix socket

Orthogonal to `mode`: how the node's `nbd-client` reaches the server.

| transport | reach | identity / access control | when |
|---|---|---|---|
| TCP + forced TLS (default) | any server over the network | enrolled client cert → NBD ACL (auth design §3.1–§3.3) | central control plane, multi-node clusters |
| Unix domain socket | a skynbdd on the *same node* | the socket file's mode (server design §15 M10a) | single-host/edge deployments; lowest-overhead data path |

The socket transport is the fastest shape available — the data path
never enters the TCP stack, and there is no TLS record layer between
the kernel's NBD device and the server's page cache — and the simplest
to configure: no enrollment, no cert renewal, no CA plumbing on the
node. The chart exposes it as `nbd.socket.{enabled,path}` (a hostPath
directory shared between the skynbdd DaemonSet and the node plugin);
`nbd.tls.enabled` must be false, and the chart fails rendering
otherwise rather than quietly serving plaintext where mTLS was asked
for. Both the driver (`SKYNBD_NBD_SOCKET`) and the chart enforce that.

The constraint the topology carries: the server on a node's socket must
serve the volumes the controller creates through `api.url`. That holds
when the control plane is node-local (the Alpine-class single-host
deployment agent design M10b targets) or when per-node servers share a
control-plane state backend. Central control plane + per-node data
planes would need a shared refs store — redb is node-local, so that
combination is not yet supported. `mode: thin` composes with either
transport: the Shape-B stack sits above whatever `/dev/nbdX` was
attached.

## 3. Snapshot semantics through CSI

- **raw mode**: `CreateSnapshot` → `POST /v1/volumes/{vol}/snapshots`
  (exists today). Crash-consistent (no freeze hook at the server);
  documented as such. `snapshot_id = "<vol>/<name>"`.
- **thin mode** (6h-2): every thin volume gets a **lineage sibling**
  `<vol>-lin` (server `kind: lineage`), created and deleted with it.
  `CreateSnapshot` files a *snapshot request* against the lineage
  volume (`POST /v1/volumes/{lin}/snapshot-requests`, in-memory — the
  snapshotter's retries recreate lost requests). Before the agent
  reports the capture cut, the controller returns retriable
  `UNAVAILABLE`, so the first successful response is inside the
  caller's application-quiesce window. It then returns a handle with
  `ready_to_use=false`. The node's embedded agent (one CycleEngine per
  staged, mounted thin volume) polls requests, runs a prompt cycle
  (fsfreeze → thin snapshot → objectize → publish, V3.2 §9) — plus
  unprompted cadence cycles for continuous publication — then
  **aliases** the published generation to the requested name (a second
  `snap/<name>` ref to the same commit) and completes the request.
  `snapshot_id = "<vol>-lin/<name>"`, so RO mounts and fast clones of
  thin snapshots reuse the raw-mode paths verbatim. Block-mode thin
  volumes run no cycle (no filesystem to freeze).
  While a request is pending it carries the cycle's reported status
  (multi-node design D-M12: phase, capture time, backfill ops, attempts,
  last error; `GET snapshot-requests/{name}`). The controller turns a
  status with `last_error` into a real `CreateSnapshot` gRPC error,
  which the external-snapshotter copies into the `VolumeSnapshot` error
  field — a healthy post-capture cycle stays a quiet
  `ready_to_use=false`. If the agent discards a captured cut before
  publishing it, the request is marked `capture_lost` and
  `CreateSnapshot` fails with `FAILED_PRECONDITION`. The agent never
  serves that name from a later cut. Delete and re-create the
  `VolumeSnapshot`. External backup hooks such as Velero's must
  quiesce application state only; they must not call `fsfreeze`, because
  the agent owns the `FIFREEZE`/`FITHAW` window and nested `FIFREEZE`
  returns `EBUSY`.
- `NodeUnstage` on a cycling thin volume runs a **final publish cycle
  first** (multi-node design D-M13): it files a request under the
  reserved transient `skynbd-unstage` alias and waits on its D-M12
  status before the cycle stop, the umount and the teardown. If the
  publish doesn't land within the budget the unstage fails
  (`UNAVAILABLE`, message carries the cycle status) and kubelet's retry
  resumes the resumable cycle — a planned reschedule loses nothing,
  and `cycleIntervalSeconds` is the handoff-latency knob, not only the
  RPO knob.
- Chart adds the `external-snapshotter` sidecar + RBAC +
  `VolumeSnapshotClass`.

## 4. Consuming snapshots: RO mount vs fast clone

Two explicit paths, chosen by the PVC:

1. **Read-only snapshot mount** (cheap, O(1), no new server volume):
   PVC with `dataSource: VolumeSnapshot` and `accessModes:
   [ReadOnlyMany-style single-node RO]` → CreateVolume returns a
   *reference volume* (synthetic id `ro:<vol>/snap/<name>`, no server
   call); NodeStage attaches the existing `<vol>/snap/<name>` RO export
   (the registry already serves every published snapshot RO — design
   §7.1) and mounts read-only. DeleteVolume is a no-op. Server ACLs
   already scope who may attach which exports.
2. **Fast clone to a new writable volume**: PVC with `dataSource:
   VolumeSnapshot` and RW access → CreateVolume calls the new server
   clone API (below); the new volume starts at the snapshot's content
   and diverges independently.

### 4.1 Server-side fast clone (engine work)

Object keys are per-volume (`{vol}/obj/{hash}`, engine `keys.rs`), so a
clone is not free today. Options, in preference order:

- **A. Cross-volume base reference** *(implemented, 6f)*: the clone
  records its provenance in a per-volume `base` object (`CloneBase`,
  additive — no existing format changes); its initial commit points at
  the source snapshot commit's shard index. Reads resolve keys in the
  clone's namespace first and fall through the base chain on miss
  (sound because every object is content-addressed or ULID-unique; a
  location cache keeps the miss round-trips off the hot path), new
  writes land in the clone's own namespace. GC pins the source
  snapshot across volumes (holder `clone:<new-vol>`, released when the
  clone is deleted); a source volume with live clone pins refuses
  deletion, and clone GC/fsck walk the cross-namespace closure.
  O(1) clone, true sharing. The spike matrix
  (`crates/skynbd-engine/tests/clone.rs`) proves GC on either side
  preserves shared objects, fsck verifies across namespaces (and
  loudly fails an unpinned clone whose base was swept), and chained
  clones resolve through the full chain.
- **B. Server-side key copy**: S3 CopyObject per object. Correct and
  simple but O(objects) time and 2× storage; acceptable fallback,
  wrong default.

API: `POST /v1/volumes {name, size?, from: "<vol>/<snapshot>"}` —
idempotent like plain create; `size` omitted/0 inherits the source
size, and may only grow. Authz: needs `volume` **and** the distinct
`clone` op on the source volume (enforced in the handler — the source
is body-carried).

## 5. Workstation CLI

`skynbd-agent-cli` gains mount-oriented subcommands (it already links
the dm stack and is the root-privileged binary people install):

```
skynbd-agent volumes   <config|--api URL>            # list volumes
skynbd-agent snapshots <vol>                          # list published snapshots
skynbd-agent mount     <vol> <dir>                    # RW head (raw or --thin)
skynbd-agent mount     <vol> --snapshot g7 --ro <dir> # RO snapshot mount
skynbd-agent mount     <vol> --snapshot g7 --clone dev-scratch <dir>
                                                      # fast clone, mount RW
skynbd-agent unmount   <dir>                          # tear down fs→dm→nbd
```

- Auth: `--token-file` (as today) or ambient OIDC token; TLS creds via
  the enrollment endpoint automatically when the server requires mTLS
  (same flow as the CSI node). For humans, `skynbd-agent login` (§7:
  Zitadel browser/PKCE and device-code flows) lands as a follow-up and
  feeds the same token cache.
- `--thin` builds the Shape-B stack locally (pool + cache on a loop
  file by default) so a workstation gets snapshot-capable mounts too;
  default is raw for one-shot inspection.
- `unmount` must find and tear down whatever stack `mount` built
  (state file per mountpoint, same pattern as the CSI node's
  device-binding state).

## 6. PR breakdown

Ordered; each PR is independently green and shippable.

| PR | scope | depends on |
|---|---|---|
| **6c — k3s install smoke** | k3s on the CI runner, import locally-built images, `helm install` devServer profile, PVC → pod write → teardown; plus issuer `tls_ca` (JWKS fetch trusting a cluster CA) to unlock full in-cluster OIDC later. | — |
| **6d — CSI snapshots (raw mode)** | external-snapshotter sidecar + RBAC + VolumeSnapshotClass in chart; CreateSnapshot/DeleteSnapshot → existing snapshot API; crash-consistency documented; smoke extended with snapshot round-trip. | 6c |
| **6e — RO snapshot mounts** | reference-volume CreateVolume for RO dataSource PVCs, NodeStage of `<vol>/snap/<name>` exports, ACL docs; smoke: snapshot → RO mount → verify content. | 6d |
| **6f — server fast clone** | engine cross-volume base references (option A; spike first), `POST /v1/volumes {from}` API + `clone` authz, GC/fsck tests, `skynbdctl` support (fsck is chain-aware). | — (server-only; parallel with 6d/6e) |
| **6g — CSI clone-backed volumes** | CreateVolume with RW snapshot dataSource → clone API; StorageClass params; smoke: clone → write → source snapshot unchanged. | 6e, 6f |
| **6h — thin-stack node mode** | `mode: thin` StorageClass param; NodeStage builds dm-cache(writeback)+thin pool over NBD via the agent's `DmStack`; embedded agent cycle per volume for fs-consistent snapshots into lineage volumes; cache-device chart values; failure/rehydration conditions (V3.2 §15.7). Largest PR; may split stage/unstage vs agent-cycle integration. | 6c (smoke), 6d |
| **6i — workstation CLI** | §5 subcommands on `skynbd-agent-cli`; enrollment-aware mTLS; raw + `--thin` mounts (shared `cachedev`/`provision`/`thinstack` agent modules); RO vs `--clone`. `login` follows 6h-2. | 6e, 6f (clone flag), 6h-1 (`--thin`) |

Deferred riders (tracked, not in the table): CSI volume expansion,
`NodeGetVolumeStats`, arm64-native CI runners, auth-failure metrics.

## 7. Decisions (previously open questions)

- **Thin-mode pool granularity: pool-per-volume** (decided). Isolation
  and PV-lifecycle == pool-lifecycle teardown win over the density of a
  shared pool; V3.2 §8 policy knobs apply per pool. Revisit only if a
  deployment demonstrates density pressure.
- **Clone authz: distinct `clone` op** (decided). Cloning exfiltrates a
  full volume; `read` on snapshots is a weaker capability and must not
  imply it.
- **Workstation OIDC: Zitadel browser + device-code flows** (decided,
  follow-up after 6i). 6i's first cut authenticates via token file /
  ambient token; a subsequent PR adds `skynbd-agent login` implementing
  OIDC authorization-code+PKCE (opens a browser, loopback redirect) and
  RFC 8628 device-code (headless boxes), both against Zitadel (auth
  design §4.1), caching the token where the mount commands find it.

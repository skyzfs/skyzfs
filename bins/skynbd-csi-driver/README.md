# skynbd-csi-driver

SkyNBD volumes as Kubernetes PersistentVolumes (auth design §9): dynamic
provisioning against the SkyNBD control plane, node attachment over
forced-TLS NBD via `nbd-client`, zero long-lived secrets.

## How it works

- **Controller** (Deployment + `external-provisioner` sidecar):
  `CreateVolume`/`DeleteVolume` → `POST/DELETE /v1/volumes`. Its
  identity needs a binding with the `volume` op (auth design §4).
- **Node** (DaemonSet + `node-driver-registrar` sidecar): at startup it
  generates a keypair and enrolls the CSR at `POST /v1/certs/enroll`
  (needs the `enroll` op), then renews on a timer; `NodeStageVolume`
  attaches `<volume>/head` on a free `/dev/nbdX` with
  `nbd-client -enable-tls` (a GnuTLS proxy process per device bridges
  TLS to the kernel — the image runs under tini to reap them),
  formats blank devices, and mounts; `NodePublishVolume` bind-mounts
  into the pod. Device↔volume bindings persist as state files so a
  plugin restart can still unstage.
- Identity chain: projected ServiceAccount token (audience per §2) →
  control plane verifies via OIDC → cert SAN derived from the verified
  `sub` → NBD ACL match. The Zitadel token-exchange hop (§9.2) is a
  deployment concern in front of the same token file.

## Data-path transport (CSI design §2.1)

Two ways the node reaches its NBD server, orthogonal to raw/thin mode:

- **TCP + mTLS** (default): `nbd.host`/`nbd.port`, forced TLS with the
  enrolled client cert, as described above.
- **Node-local Unix socket**: `nbd.socket.enabled=true` points the node
  plugin at a socket on the host (`nbd.socket.path`, default
  `/run/skynbd/nbd.sock`, mounted by directory so a server restart can
  recreate it) and it attaches with `nbd-client -unix`. The server is a
  skynbdd on the same node — the chart's `devServer.kind: DaemonSet` for
  dev/CI, or your own DaemonSet with `nbd_listen = "unix:<path>"`.

  The socket takes the TCP stack out of the data path and drops the
  entire enrollment story: a socket connection carries no client
  certificate, so access control is the socket file's mode. The chart
  refuses `nbd.socket.enabled` together with `nbd.tls.enabled`, and the
  driver refuses `SKYNBD_NBD_SOCKET` with an explicit
  `SKYNBD_NBD_TLS=1`, rather than silently attaching in plaintext.

  What it requires: the skynbdd on the node's socket must serve the same
  volumes the controller provisions through `api.url` — a node-local
  control plane (the single-host/edge deployment of server design §15
  M10a), or per-node servers sharing one control-plane state backend.
  A central control plane with per-node data planes needs shared refs
  storage, which does not exist yet.

## Capabilities (current)

`CREATE_DELETE_VOLUME`, `CREATE_DELETE_SNAPSHOT` (server-native,
crash-consistent for raw-mode volumes; `snapshot_id = "<vol>/<name>"`),
`STAGE_UNSTAGE_VOLUME`, `SINGLE_NODE_WRITER`, Mount (ext4 default,
fstype honored) and Block volumes. Snapshot CRDs
(external-snapshotter) are a cluster prerequisite; the chart adds the
csi-snapshotter sidecar and a VolumeSnapshotClass when they exist.

**Snapshot restores** (CSI design §4) — the PVC's access modes choose
the path:

- *Read-only* (`ReadOnlyMany`): a **reference volume** — synthetic id
  `ro:<vol>/snap/<name>`, no server call, DeleteVolume a no-op — and
  the node stages the snapshot's existing RO export, mounted read-only
  (`noload`/`norecovery`: crash-consistent images may carry a dirty
  journal, which cannot replay on a read-only device). Who may attach
  which snapshot exports is governed by the server's NBD ACLs
  (`*/snap/*` patterns).
- *Writable* (`ReadWriteOnce`): a **server fast clone** (design §4.1)
  — `POST /v1/volumes {from: "<vol>/<snap>"}` creates a real volume
  starting at the snapshot's content, O(1) via cross-volume base
  references, diverging independently. Capacity may only grow (a
  smaller request is OUT_OF_RANGE; omitted inherits the source size).
  The clone pins its source snapshot until deleted — deleting the
  VolumeSnapshot first is refused until its clones are gone. The
  controller identity additionally needs the `clone` op on the source
  volume (cloning reads its full content, so `volume` alone must not
  grant it).

**Thin mode** (CSI design §2, V3.2 Shape B): a StorageClass with
`parameters: {mode: thin}` stages volumes as
`fs → thin origin → thin pool → dm-cache (writeback) → NBD`, built via
the agent crate's dm provisioning (kernel ioctls, no dm-setup
shell-outs). Writes are absorbed locally and coalesced by writeback;
the cache's fast device is a per-volume file under the node's cache
dir (`node.cache` chart values), **preallocated with `fallocate`** (a
sparse file could hit ENOSPC inside the writeback path and wedge the
stack; reservation makes out-of-space a clean stage-time failure —
unwritten extents still read as zeros, which is the fresh-metadata
contract) and looped with `--direct-io=on` where the filesystem
supports it (avoids double page-caching under the dm stack; falls back
to buffered with a warning). The file carries the dm-cache *and*
thin-pool metadata beside — never behind — the cached path. Consequences the driver enforces rather than hides:

- **Node-sticky**: the thin metadata in the cache file is the volume's
  logical structure, so the PV pins to its first node via topology —
  the StorageClass must use `volumeBindingMode: WaitForFirstConsumer`.
- **Unstage flushes**: dirty writeback blocks are local-only (V3.2
  §6.5); unstage switches the cache to the `cleaner` policy and waits
  for zero dirty blocks before teardown. The cache file survives for
  the next stage (reboot recovery reads the dm metadata back).
- **Host udevd required**: dm ioctls synchronize with the host udev
  daemon (uevent cookies over SysV semaphores), so the node pod runs
  with `hostIPC` and the host's `/run/udev` mounted (chart does both
  when `node.cache.enabled`). A node without udevd cannot stage thin
  volumes.
- **Cache loss is loud** (V3.2 §15.7): a binding whose cache file is
  gone fails NodeStage with `FAILED_PRECONDITION` — the head is
  missing an unknown set of writes; recover from the last published
  snapshot instead of trusting it.
- **Snapshots are fs-consistent and continuous** (6h-2, V3.2 §9): the
  head's server-side bytes are pool-physical, so they are *never*
  snapshotted server-natively. Instead every thin volume gets a
  **lineage volume** sibling (`<vol>-lin`, created with the volume),
  and the node runs an embedded agent cycle per staged thin volume:
  fsfreeze → thin snapshot → objectize → publish into the lineage
  volume, every `node.cache.cycleIntervalSeconds` (default 900) —
  and on the next snapshot-request poll when a CSI `VolumeSnapshot` is
  created. Before the agent reports its capture cut, `CreateSnapshot`
  returns retriable `UNAVAILABLE`; after the cut it returns
  `snapshot_id = "<vol>-lin/<name>"` with `ready_to_use: false` until
  publication and aliasing finish. The external-snapshotter polls
  throughout, typically for a few seconds. Backup hooks (including
  Velero hooks) must quiesce the **application** only, never run
  `fsfreeze`: the agent owns `FIFREEZE`/`FITHAW`, and nested
  `FIFREEZE` fails with `EBUSY`. RO snapshot mounts and fast clones of
  thin snapshots then work exactly like raw-mode ones — the id already
  points into the lineage volume. Restores *into* thin mode (thin PVC
  from a snapshot dataSource) remain unsupported; restore via a
  raw-mode StorageClass or a clone.
- **Node identities need the ingestion ops** for lineage volumes when
  auth is enabled: `ingest` (prepare/attach/publish/alias +
  snapshot-request poll/complete) and `read` on `*-lin`, beside the
  existing `enroll`. The controller keeps `volume`/`snapshot`
  (snapshot-request creation rides the `snapshot` op).

Not yet: expansion, per-volume stats, thin restores from snapshots.

## Deploy

Helm chart: `deploy/charts/skynbd-csi-driver` (CSIDriver, controller,
node DaemonSet, StorageClass). Images: `ghcr.io/skyzfs/
skynbd-csi-driver` and `ghcr.io/skyzfs/skynbdd`
(`.github/workflows/images.yml`). The chart's `devServer.enabled`
deploys an in-cluster memory-store skynbdd with auth and TLS off —
strictly for dev/CI smoke.

Node hosts need the `nbd` kernel module; the DaemonSet loads it via an
init container (`node.loadNbdModule`).

# SkyNBD Multi-Node: Ownership, Recovery, and Handoff

**Status**: design accepted, implementation in progress. Extends `skynbd_design.md`
(D6, §6.1, §6.5, §7.2, §16 item 1) and `skynbd-csi-design.md`.

This document exists because a question with a short answer — *what happens when a
pod is rescheduled to another node?* — turned out to have a long one. It records
the goals, the decisions taken, the measurements they rest on, and what is left to
build.

---

## 1. Goals and non-goals

**Goals**

1. **No full object scan on any live path.** Volume recovery and handoff must cost
   a bounded number of requests, independent of how many commits or snapshots the
   volume has.
2. **Reschedule is a handoff, not a race.** Exactly one server serves a volume's
   head at a time, and the winner is decided by the store, not by hope.
3. **A server never republishes live data as an empty volume.** Silent data loss
   is the only outcome worse than refusing to serve.
4. **Opening a volume is cheap enough not to matter.** Attach latency should be
   dominated by the filesystem, not by the extent map.

**Non-goals (unchanged from `skynbd_design.md` §1.2)**

- Multi-writer access to one export. Single-writer-per-volume remains the model;
  this work makes it *enforceable* across hosts rather than assumed.
- Synchronous cross-region replication, or any consistency stronger than the
  object store's own per-object linearizability.

---

## 2. Where we are today (measured)

All numbers from `crates/skynbd-engine/tests/{open_walk,extent_shape}.rs` against a
reference volume: 100 GiB, 50 GiB live, 64 KiB average extent, 100 retained
snapshots, 10 ms object-store round trip.

| path | cost | why |
|---|---|---|
| `Engine::open` (head) | ~3,300 GETs, **~33 s** serialized | materializes the whole extent map |
| snapshot export publish | one full walk **per snapshot ref**, at every startup and watch-lag resync | `sync.rs` → `Engine::open_at_commit` |
| `clone_volume` | one full walk | O(1) in objects written, not in latency |
| `rebuild_refs` | `N+1` walks + a GET of every commit object ≈ 10⁶ round trips, **hours** | no cache shared between refs |
| `fsck` / `gc` | same walk, once per ref; `gc` holds `&mut Engine` | writes stall for the duration |

Two fixes have landed (§7.2): walks now fan out with bounded concurrency (12.5×
measured at 10 ms on a 265-node map), and extents coalesce at fold time, which
collapses 8,192 sequential 64 KiB IOs into 64 extents and the open walk from 35
GETs to 4. Neither changes the *shape* of the problem: the work is still
proportional to the whole map, and still repeated per ref.

**The correctness hole.** `Engine::open` cannot distinguish "no such volume" from
"volume exists, its refs are elsewhere" by error alone. Before the `has_history`
fix, the second fell through to `create_volume` and republished a live volume as a
blank disk — which a CSI node plugin then formats. Reachable through ordinary
operation: `[[volumes]]` config on more than one host against one bucket, or an
external-provisioner `CreateVolume` retry landing on a second replica.

---

## 3. Decisions

### D-M1 — Refs live in the object store, one object per volume

`skynbd/v1/{vol}/refs` holds the volume's refs, pins and meta rows, written with a
conditional PUT (`If-Match` on the ETag). redb is demoted from authority to cache.

Per-*volume* rather than per-*ref* granularity is load-bearing, not a compromise:
`RefStore::transact(vol, ops)` is atomic across `CasRef` + `Pin` + `MetaPut` for one
volume (D13/I8, publish-implies-pin). One object per ref cannot implement that; one
object per volume gets it for free, because a single conditional PUT *is* the
transaction.

Conditional writes are available on AWS S3 (since 2024-08), MinIO and R2; GCS's XML
API wants `x-goog-if-generation-match` instead. So the backend is selected by
capability probe plus config, redb stays the single-node default, and etcd remains
the option for heavy multi-writer (§16 item 1). `RefStore` was shaped for exactly
this swap (D6), and `exercise_ref_store` is the conformance target.

`watch()` has no object-store analogue, and needs none: under single-writer-
per-volume the owner makes every change, so the registry's change stream is a local
broadcast.

### D-M2 — Serialize the rows, not the redb file

The refs object is a postcard-encoded row dump behind the standard 8-byte frame
(`SREF`), ~6 KB for a volume with 100 snapshots. A raw redb file copy would be
megabytes of pages and would weld disaster recovery to redb's on-disk format —
restoring a 2.6.x file into a future binary is a bad day for no benefit.

### D-M3 — CAS gives safety; a lease gives exclusivity. Both, in one object

A conditional PUT prevents *divergence*: the loser learns it lost. It does **not**
prevent two servers from both serving reads and acking writes until one of them
writes. Exclusivity needs a lease, so the refs object also carries
`{owner, epoch, expires_at}`.

Folding them together is what makes it cheap: every commit renews the lease for
free, and an idle volume renews on a timer. The **epoch, not the clock, is
authoritative** — the object store offers no monotonic time and node clocks skew, so
the TTL is only how long a taker waits before stealing.

A fenced writer cannot corrupt the volume: its next commit's CAS fails, which is
already `EngineError::Conflict` ("lease lost", I2) and already means *drop the
overlay, close connections with `NBD_ESHUTDOWN`*. What it can do is ack writes it
never lands; the filesystem discovers that at flush and goes read-only, which is the
standard storage failure mode.

### D-M4 — Export cadence is a tunable; the gap is closed by a bounded forward scan

Writing the refs object on every commit gives zero-scan recovery but costs a
conditional PUT per fsync (~$26/volume/month at 2 fsync/s). Writing it on the
snapshot cadence costs ~$0.02 and leaves a gap, closed at recovery by listing
`commit/` and GETting only objects newer than the export's timestamp — a LIST is
1,000 keys per request, so the scan is bounded by the cadence, not by history.
Default: cadence plus timer; per-commit is available for deployments that want it.

### D-M5 — `Commit` carries the fence epoch

The forward scan of D-M4 is **unsafe without this**. A fenced writer's commits are
byte-for-byte indistinguishable from the winner's — same prefix, complete closure,
plausible `flush_seq` — so a recovering server would adopt them and resurrect
exactly the writes the fence discarded. `Commit.epoch` (a version bump on `SCMT`)
lets the scan accept only the current epoch, and lets GC treat other epochs'
commits as garbage by construction.

### D-M6 — Handoff happens at `NodeUnstageVolume`, not in a pod lifecycle hook

Kubelet already calls `NodeUnstage` on graceful pod deletion, before the pod is
rescheduled, and the driver already does the hard part there: flush dirty writeback
*before* teardown, failing the unstage rather than stranding the head's latest
writes. It needs to additionally commit and release the lease. A container `preStop`
hook is strictly weaker — it cannot unstage the volume, and a freeze + objectize
cycle will exceed the default 30 s `terminationGracePeriodSeconds`.

Neither hook runs on node failure. That case is the lease TTL and nothing else,
which is what the TTL is for; `node.kubernetes.io/out-of-service` is the
cluster-side half of the same handshake.

### D-M7 — Flat extent map as an immutable, commit-addressed cache

`skynbd/v1/{vol}/emap/{hex(commit)}`: the materialized map for exactly that commit,
immutable like everything else under the prefix. Present → one GET; absent → fall
back to the tree walk. No CAS, no staleness question, no second source of truth that
can disagree with the tree, and `fsck` can re-walk and compare. GC reaches it from
the commit that names it and sweeps it with that commit.

Format: gap/len varints plus an interned target table (one 8 MiB segment backs ~128
× 64 KiB extents, so interning turns 16–32 bytes per extent into a varint index),
structure-of-arrays columns, per-shard byte offsets in the header so a reader can
range-GET only the shards it wants. ~6–8 MB for 819k extents versus ~30 MB naive —
60 ms of transfer against 33 s of walking, in one request instead of 3,302. Written
on clean unstage and on the snapshot cadence, **never per commit** (6 MB × 173k
flushes/day is a terabyte).

Loading must build the `ExtentMap` from the sorted, disjoint dump in one pass;
replaying 819k `insert()` calls would spend a third of a second re-deriving what the
dump already states.

### D-M8 — Materialize lazily; full reachability is `fsck`'s job

`Engine::open` eagerly walking the closure was buying **I1** (reachability before
visibility) on every open. That is the wrong place to pay for it:

- For a finalized snapshot the closure was verified complete when it was published,
  nothing mutates it afterwards, and only a GC bug could break it — which I6 forbids
  and `fsck` exists to catch. Re-verifying at every open pays a whole walk to
  re-answer an answered question, once per snapshot, at every startup.
- A clone needs nothing materialized: its map *is* the base's shard index.
- Even the writable head needs only the shards it touches — the fold rewrites
  touched shards, and `query` reads only those shards' leaves.

So: shards materialize on first touch (shard index → shard root → leaf), and I1 is
verified where it is cheap — at publish, and in `fsck`. §6.2's statement of I1 is
unchanged as an *invariant*; what changes is where it is checked.

Export *registration* stays eager, so instant publication (D4, scenario S3) is
preserved: the registry needs the export's size, which comes from the superblock,
not from the map.

### D-M10 — The committed extent map is immutable, per shard, and shared

Most maps in the system are read-only: snapshot exports, rpo exports, clone bases,
and (D-M11) external origins. `ExtentMap` is a `BTreeMap` whose entire mutation
machinery — predecessor lookup, truncation, split, re-insert — exists to serve
`insert()`, which a read-only map never calls. So the committed half of every engine
becomes an immutable sorted index, and **the unit of immutability is the shard**:

- **Commit** rebuilds only touched shards and `Arc::clone`s the rest. Today's fold
  clones the whole map and re-inserts every overlay entry; a per-shard merge is one
  linear sweep over the affected shard, coalescing (§7.2) as it goes rather than
  afterwards.
- **Sharing** is per shard, which is what actually solves the memory problem: a
  snapshot differs from head by the shards it touched, so N snapshot exports cost
  N × (a few shards), not N × the volume.
- **Lazy materialization** (D-M8) and the `emap` object's per-shard byte offsets
  (D-M7) already sit on the same boundary; loading an `emap` shard *is* the live
  structure, so there is no map to build.
- `Placed.lba0` disappears. It exists so a split clone can rebase its target offset;
  an immutable index never splits an entry, and partial overlap is computed at query
  time from the extent's own start. One piece of error-prone arithmetic stops
  existing.

Shard-scoping is also what makes the layout compact: within a 1 GiB shard the offset,
the length, the interned target index and the offset within an 8 MiB segment all fit
in `u32` — 16 B/extent of payload. A volume-wide flat array would need `u64` starts
and lose a third of that. Per-shard extent counts are bounded in practice by IO
granularity, not by any enforced cap: ≤256 Ki extents at 4 KiB granularity
(§5.3's depth argument), 2 Mi structural worst case at the 512 B sector floor —
each vector stays comfortably `u32`-indexed either way.

**The target table is per volume, not per shard.** A segment's ~128 extents scatter
across shards, so a per-shard table holds a near-complete copy of the segment list in
*every* shard. Measured on the reference volume, that mistake alone cost 20 MB of the
34 MB the first prototype used — more than the columns themselves.

**Implemented** in `crates/skynbd-engine/src/index.rs` (task 3a). Shard membership is derived from each extent's LBA rather than its position in the tree's shard array, and extents straddling a boundary are split: a tree can be written under one `shard_bytes` and read under another, since the ingestion path builds with `EngineConfig::default()` whatever the head engine is configured with. The merge sweep and the query are held against `ExtentMap` as an oracle by proptest.

**Measured** (`tests/extent_index.rs`, reference volume: 100 GiB, 644,619 extents
after coalescing). Byte counts are deterministic; times are from `opt-level = 1` on a
shared runner and vary run to run, so they are given as observed ranges over three
runs and only the order of magnitude should be read into them.

| | `BTreeMap` today | per-shard columnar | |
|---|---|---|---|
| resident | 108 MB (167.6 B/extent) | 13.5 MB (21.0 B/extent) | **8.0×** |
| allocator churn, one commit | 105 MB | 2.0 MB | **52×** |
| build from extents | 407–681 ms | 140–231 ms | ~3× |
| 20k point queries | 21.5–26.4 ms | 3.8–4.1 ms | ~5.8× |
| one commit (8 MiB of writes) | 136–581 ms | 4.3–8.4 ms | ~16–135× |

Three things to take from it. `BTreeMap` costs **167.6 B/extent**, roughly twice what
a back-of-envelope suggests — node overhead plus a 48-byte `Placed` add up. The
**commit path, not the read path**, is where the current shape hurts: cloning the
whole map and re-inserting the overlay churns 105 MB *per flush* on a large volume,
and the wall-clock cost is the same order as the flush interval itself. And the
21.0 B/extent includes `Vec` growth slack — a loader that knows the extent count up
front (the `emap` header does) can reserve exactly and land near the 16 B payload
figure.

The `u32` packing is checked rather than assumed — the harness reports the maxima it
saw, every workload fits, and `ShardBuilder::push` refuses an oversized field at
build time rather than truncating a target offset and returning wrong bytes.

End to end on the shipped code, a 4 GiB volume with 32k random 4 KiB writes: an
opened engine holds **529 KB** where the same extents cost 5.4 MB in the previous
representation — the 10.2× the prototype predicted for that shape.

The overlay stays mutable and stays as it is: small, bounded by the checkpoint
policy, layered over the committed shards. Writable heads — raw *and* thin, plus
lineage volumes under ingestion — keep the write path unchanged. The split is
writable-head versus read-only-export, not raw versus thin.

### D-M11 — Thin volumes stage from an external origin

A rescheduled thin volume cannot attach its old head: the pool's data blocks are
pool-physical and the LBA→block mapping lives in the previous node's cache file. So
"recoverable from the last published `G_n`" would otherwise mean a full restore
proportional to used size.

dm-thin's target takes an optional external origin — `thin <pool> <dev_id>
[<external_origin>]`, where unprovisioned blocks read through to another device.
Point it at an O(1) server-side clone of the volume's newest published generation
(the §4.1 fast clone, already built) and staging on a new node is O(1): reads fall
through to the snapshot, writes provision into the pool.

**Configuration (normative).** The pool's data device stays dm-cache over the NBD
head, exactly as today, so new writes remain continuously durable in S3. The
external origin is a *second, read-only* NBD attach of the clone. Two attaches per
thin volume is the cost.

This does not retire V3.2 §15.7: losing the cache file still loses the tmeta, so
provisioned blocks in the head stay uninterpretable. What changes is that the
fallback — restage from the last published `G_n` — becomes O(1) instead of a full
restore, so the exposure is the *unpublished delta* rather than the volume.

### D-M12 — Snapshot finalization is observable, not blocking

A published snapshot is already fully durable by construction: the cycle uploads the
delta, crosses a durable barrier (`checkpoint`), then publishes ref + manifest + pin
in one transaction (I8), and I1 forbids a ref pointing at an incomplete closure.
`ready_to_use` remains "does this name appear among the lineage volume's published
snapshots," but the handle itself is a separate safety barrier: before the agent
reports the capture cut, thin `CreateSnapshot` returns retriable `UNAVAILABLE`.
The external-snapshotter retries instead of leaving an RPC worker blocked; once the
cut is reported it receives the handle with `ready_to_use=false` until publication.

What is missing is a *reason*. A failing cycle reports no usable response forever
with no phase, no progress and no last error. A status endpoint on the snapshot
request (phase, capture time, ops remaining, attempt count, last error) lets the
driver both establish the cut-time handle barrier and put a real message in the
`VolumeSnapshot`'s error field, lets an operator distinguish "uploading 40 GB" from
"wedged on a freeze", and gives D-M13 something to wait on.

The request-to-cut binding is the agent's, and it is durable: the requests pending
when a freeze begins are written into the agent's state record together with the
candidate they will alias (`candidate_requests`), so a request filed mid-cycle waits
for a cut of its own. Every later state transition carries the binding with the
candidate. Publication moves it to `served_requests` (alias to exactly that
`snap/gN`, at the original cut time, then complete). Discarding the candidate moves
it to `lost_requests` (abort, recovery normalization, or a swept activation
re-captured under the same deterministic `snap/gN`). A lost request is never
re-captured, since its caller may already hold the discarded cut's time. Instead the
server marks it `capture_lost`, CSI fails it permanently, and D-M13 re-files. The
agent's tombstone outlives the report (the newest 1024 are kept): the server's mark
is in memory, so after an API restart the controller's re-POST would look like a
fresh request. The agent re-reports the loss whenever the name is pending again.
Only D-M13's reserved `skynbd-unstage` name, which no caller holds a time for, is
forgotten once its loss is on record. The
agent re-sends its bindings with every report, so the server's in-memory status
(first binding wins: a request's time never moves) is rebuilt after an API restart.
Completion removes a request from the agent's queue but retains its status, bounded
per volume, until the aliased snapshot is deleted.

### D-M13 — `NodeUnstage` runs a final cycle before teardown

The graceful handoff drives the unpublished delta to zero: unstage files a snapshot
request, waits for it to publish (D-M12's status endpoint), and only then tears the
stack down. A planned reschedule then loses nothing, and an unplanned one loses at
most a cycle interval — so `cycleIntervalSeconds` becomes the handoff-latency knob,
not only the RPO knob.

Ordering is fixed by the freeze watchdog: it holds a pre-opened fd on the mounted
origin (V3.2 §7.11), which is why unstage already stops the cycle *before* the
umount. The final cycle therefore runs first — and its freeze is uncontended, since
`NodeUnpublish` has already removed the pod's bind mount.

This is viable only because the cycle is resumable: it persists `Capturing →
Captured → Backfilling` with an op cursor and runs the §15.2 recovery planner on
re-entry, so kubelet's unstage timeout and retry *resume* the upload instead of
restarting it.

### D-M9 — A name index object, so discovery needs no scan

`volume_id = blake3(name)` is deterministic, so any server computes a volume's prefix
from its export name and checks existence with one HEAD — discovery never needs a
listing. The reverse direction is what is missing: `Superblock` records no name, so
"list every volume in this bucket" cannot reconstruct the export namespace. A
`skynbd/v1/index/vol/{name}` marker written at create closes it, and makes the
catalog reconstructible after total refs loss.

### D-M14 — Shard geometry belongs to the volume

Implementing D-M10 surfaced that `shard_bytes` was recorded nowhere: the
shard index is positional, so the tree's meaning came from whichever
`EngineConfig` happened to open it. That was reachable — the ingestion path
builds with `EngineConfig::default()` regardless of the head engine — and
the consequences were asymmetric. A *smaller* geometry extends the shard
array, leaving a stale root and a rewritten one both claiming a range; a
*larger* one truncates it, dropping roots outright and letting GC reclaim
what they referenced.

So the superblock records it (v2; a v1 body decodes as 1 GiB, the only
value it could have been written under), the engine adopts it on open, and
a clone inherits its base's, since its first commit points at the base's
shard index verbatim. `commit()` refuses a shrinking resize rather than
truncating.

The version bump rather than an additive object was a deliberate call:
additive costs nothing today and needs no migration, but an old binary
meeting a tuned volume would ignore the object and misread it *silently*.
A version bump makes that case fail loudly. The rollback cliff it creates —
new volumes unreadable by older binaries — is acceptable because there is
no production deployment to roll back.

### Rejected alternatives

- **A redb backup per snapshot, restored on recovery.** Right instinct (refs belong
  in the object store), wrong granularity. Head advances on every fsync while
  snapshots land on a 15-minute cadence, so restoring a snapshot-time backup
  *rewinds* head and silently discards everything since. It is also one file for all
  volumes, so a backup taken for volume X captures Y at an arbitrary point; and with
  per-node redb files it produces N stale copies with no way to choose between them.
  D-M1 is the same idea at the granularity that works.
- **An interval-map crate (`nodit`) for `ExtentMap`.** Its serde support is the
  least relevant property — the emap format is interned and delta-encoded, which no
  derive produces. Its merge-touching inserts key on `V: Eq`, while our adjacency
  predicate is arithmetic on the value (`Extent::continues_into`), so the one
  feature worth importing does not transfer. A `BTreeMap`-backed interval map is
  also no better on memory, which is the constraint that actually binds — and D-M10
  goes the other way entirely, dropping the mutable machinery for the maps that
  never mutate.
- **Implicit adoption of another server's head.** A server that recovers a head by
  scanning the store has forked the volume, not recovered it. Adoption stays an
  operator decision (`ProvisionError::RefsLost` → `fsck --rebuild-refs`) until D-M3
  ownership exists.

### D-M15 — The origin advances with the generation chain

D-M11's external origin is chosen at stage time and then static: the pool
accumulates every write since stage, and the stage-time clone pins its
source generation against GC indefinitely. D-M15 makes the published
generation chain the volume's advancing immutable head, with the
indirection owned by **skynbdd, not the kernel**: the origin is a
repointable read-only export (`<lineage>/origin`) whose backing
generation the server switches atomically (an `Arc` swap over D-M10's
immutable committed state — the node's single origin attach and its dm
table never change), recorded as a pin-ordered meta row in the refs
object so restarts and reconnects resolve it. Publish and advance run on
decoupled cadences: every cycle publishes (`publishIntervalSeconds` is
the RPO knob, cheap enough for minute-level cadence — no flat emap, no
reclaim), while the origin advances only at configured snapshots or a
pool-usage watermark. After a repoint the agent discards exactly the
locally provisioned blocks the origin now serves identically and
persists the rebaselined pair — so **every advance cycle ends in the
same state a fresh restage starts in**, the pool stays a delta buffer
bounded by the advance interval, the clone machinery of task 12 is
retired, and the GC pin overhang rides the baseline pins the agent
already manages. Phase 2 (a separate, per-StorageClass decision)
retires the pool-physical writeback head entirely: pool data on the
local fast device, a single NBD attach, the §15.7 failure class gone,
RPO explicitly `publishIntervalSeconds`.

Full design — invariants (swap invisibility, reclaim safety, the
memoryless post-advance shape, the monotone-pointer/fenced-reclaim
guard, the node-epoch publish fence), the advance sequence with its crash matrix, the
publish/advance cadence split, the two-executor reclaim policy
(discard port vs two-pool swing, chosen per advance), the
head-retirement tradeoff table, and
the task breakdown (15a–15k) — lives in
**`skynbd-origin-advance-design.md`**.

---

## 4. Remaining tasks

Ordered by dependency; the first two have landed.

| # | task | status |
|---|---|---|
| 1 | Bounded fan-out for the tree walks (§7.2) | **done** |
| 2 | Fold-time extent coalescing (§7.2); `has_history` / `RefsLost` (§6.5) | **done** |
| 3a | **Immutable per-shard extent index** (D-M10): `committed` becomes sorted columnar shards behind `Arc`; commit rewrites only touched shards and shares the rest; the overlay and the write path are unchanged. | **done** |
| 3b | **Lazy materialization** (D-M8): open records shard roots and stops (3–4 reads regardless of size, measured 45 ms at 10 ms latency where the eager walk was 2.65 s serial); shards materialize on first touch; snapshot exports and clones cost nothing until read; I1 verified at publish + `fsck`. Crash matrix green over the async read path. | **done** |
| 4 | **`ObjectRefStore`** (D-M1, D-M2): conditional writes on `ObjectStore` (Memory + S3 + Fault), the `SREF` per-volume object with the D-M3 owner fields reserved, `exercise_ref_store` conformance + cross-instance CAS/transaction race tests + a MinIO-gated live run, skynbdd `refs_kind = "object"` with a startup capability probe. The redb-as-cache / cadence tier is D-M4 and waits on task 6's epoch. | **done** |
| 5 | **Ownership** (D-M3): `lease_acquire/release/get` on `RefStore` (one shared state machine: renew keeps the epoch, steal bumps it, release zeroes expiry so epochs stay monotonic), fenced `ObjectRefStore` writes (`LeaseLost` → `EngineError::Conflict` → `ESHUTDOWN`) with free renewal on every owner write, provisioner lease-before-open + release-on-delete + idle renewal timer, skynbdd `node_id`/`lease_ttl_s`. Cross-instance steal/fence races + engine-level fence test + S3-gated live run. | **done** |
| 6 | **`Commit.epoch`** (D-M5): `SCMT` v2 carries the writer's fence epoch (v1 decodes as 0 — every pre-epoch commit was unowned); engines stamp the epoch acquired at lease time (`EngineConfig.epoch`, wired by the provisioner). Recovery (`rebuild_refs`) orders head candidates by `(epoch, flush_seq, created_at_ms)` and, when a lease row survives, ignores epochs the lease history never granted — a zombie's higher `flush_seq` can no longer resurrect fenced writes. GC needs no change: fenced commits are unreachable from refs, and the epoch keeps them unreachable through rebuilds. The D-M4 cadence's forward scan is now safe to build on this. | **done** |
| 7 | **Lease release in `NodeUnstage`** (D-M6): `POST /v1/volumes/{vol}/release` (commit-if-dirty → stop serving → release lease) and `.../serve` (pick up from the shared catalog, acquiring the lease) on the API; the node plugin brackets the attachment — serve before `nbd-client` dials on stage, release after detach on unstage, both best-effort (the TTL stays the backstop for every ungraceful path) and skipped for read-only stages. Two-server handoff proven over the API in `tests/handoff.rs`, including the dirty-overlay commit-before-release. `attachRequired: true` + `ControllerPublish` deferred: kubelet's stage/unstage ordering plus the lease already serialize, and the CSIDriver-object change is not needed until multi-server placement exists. | **done** |
| 8 | **`emap` object** (D-M7): `SEMP` — postcard header (commit, grid, interned object table, per-shard byte ranges) + gap-encoded `u32` columns per shard, so a future reader can range-GET single shards. Written by `Engine::write_emap` on clean unstage (`Provisioner::release`) and at every `snapshot_create`; open fetches it in one GET and each shard's first touch decodes straight into the D-M10 columns — no build step, and any per-shard problem falls back to the tree walk (the tree stays the only truth). GC derives live emap keys from live commit keys and sweeps the rest; `fsck` re-walks and compares, reporting a disagreeing emap by key. Proven by reading a volume whose subtree nodes were deleted — the emap alone serves every byte. | **done** |
| 9 | **Name index** (D-M9): `index/vol/{name}` marker (the catalog row's bytes) written at create, removed at delete, living outside every volume prefix so GC never sees it. `rebuild_catalog` — free fn, `Provisioner` method, `POST /v1/catalog/rebuild` ("gc" perm), and `skynbdctl rebuild-catalog [--with-refs]` — restores missing rows from markers; existing rows win, an unreadable marker aborts rather than half-restoring. With `--with-refs` composing `rebuild_refs` per volume, total refs loss (redb gone, bucket intact) recovers to a serving daemon with data intact — the round trip the tests prove. The bounded forward scan half of this row landed with D-M4 (`recover_head`). | **done** |
| 10 | **Snapshot request status** (D-M12): each pending request carries `{phase, capture_time_ms, completed, capture_lost, ops_done, ops_total, attempts, last_error}` — read via `GET snapshot-requests/{name}` ("snapshot" perm). The cycle engine reports through a default-no-op `ControlPlane::report_progress` at every phase seam plus every 64 backfill ops; every report also carries the agent's durable per-request capture bindings, which `cycle-status` ("ingest" perm) stamps onto exactly the named requests (first binding wins; a conflicting one, or an explicit loss report, sets `capture_lost`). The node's cycle loop stamps attempts/last_error on failures — all best-effort by contract (a down reporter, or a server without the route, never fails a cycle). The CSI driver returns retriable `UNAVAILABLE` until `capture_time_ms` proves the cut, `FAILED_PRECONDITION` for a lost cut, and turns a status with `last_error` into a real gRPC error — the external-snapshotter copies it into the `VolumeSnapshot` error field — while a healthy post-capture cycle stays a quiet not-ready. Completed requests leave the agent queue but retain this status (the most recent 256 per volume) until snapshot deletion, preserving the CSI creation time. Proven end-to-end over real HTTP (engine reports → request status) and at the driver (capture-gated handle, cut timestamp, error message carries phase/attempts/error; readiness beats a stale error). | **done** |
| 11 | **Final cycle in `NodeUnstage`** (D-M13): `CycleRunner::finish` files a snapshot request under the reserved, transient `skynbd-unstage` alias (deleted before filing so it lands on the fresh generation, and after so it never pins a commit) and polls the D-M12 status until the request completes — the loop completes requests only after a successful publish, so completion IS "delta published". Runs first in unstage, before the node lock, the cycle stop and the umount (freeze needs the mount; the wait is control-plane-bound). On timeout (`finish_budget`, default 120 s) the unstage **fails** with the last observed cycle status in the message (gRPC `UNAVAILABLE`) and teardown is refused — kubelet's retry resumes the resumable cycle, so a planned reschedule loses nothing and `cycleIntervalSeconds` becomes the handoff-latency knob. Volumes without a running cycle (raw/block/RO) pass through untouched. | **done** |
| 12 | **External-origin staging** (D-M11): `ThinStackSpec.external_origin` puts the third argument on the thin table (`thin <pool> <dev_id> <origin>`), so unprovisioned blocks read through. A fresh-cache thin stage (no cache file = this node holds none of the volume's structure) clones the newest published generation as `<vol>-org` (§4.1 fast clone), **seeds the agent's state with a rebaselined pair on the same generation** — the empty-local-member shape the engine already understands, so the first cycle prepares on `gN` and uploads only the locally provisioned delta — and attaches the clone's head as a second NBD device for the table. Ordering makes crashes safe: clone + seed become durable *before* the cache file exists, so a retried stage lands in the reuse branch with a consistent pair. The clone is load-bearing for the pool's whole life (unprovisioned reads keep falling through): its server-side existence is the durable restage marker, reboot restages reuse it, unstage only detaches and releases its lease, and it is deleted with the volume (before the lineage, whose snapshot it pins). Origin errors fail the stage closed — a fresh pool without its origin would present history as a blank disk. The planner gained the row that makes seeded/rebaselined pairs converge (established pair + pinned base = quiescent). Proven by an engine-level test: node B on a fresh stack with seeded state publishes `g1` = `g0` ⊕ local writes, base content intact. | **done** |
| 13 | Revisit `segment_max_bytes`: the coalescing ceiling is one extent per segment, so segment size is the remaining fragmentation lever for sequential workloads | |
| 14 | **Origin advance** (D-M15): the immutable base moves with the generation chain — decoupled publish/advance cadence (advance at configured snapshots or pool watermark), bounded-delta pool, and (phase 2) writeback-head retirement. Sub-tasks 15a–15k broken down in `skynbd-origin-advance-design.md` §9; depends on task 12. | 15a done: repointable `<lineage>/origin` export + pointer meta row + `GET`/`POST /v1/volumes/{vol}/origin`. 15b done: freeze-windowed reclaim (`DmStack::reclaim`) + `BLKDISCARD` plumbing + `no_discard_passdown`, with the S15b Tier B equivalence scenario. 15c done: two cycle kinds, watermark backstop, §3 advance tail in promote with the Invariant E fence read |

Deferred, tracked here so it is not lost: `self.committed` keeps the *unmerged* map
in the running process, so coalescing's memory win only materializes on the next
open. Task 3 subsumes it — the per-shard merge coalesces as it builds, so there is
no unmerged form to keep.

---

## 5. Open questions

1. **Cadence default for the refs object** (D-M4). ~~Per-commit is simplest to reason
   about and costs real money at high fsync rates; cadence-plus-scan is nearly free
   but only correct once `Commit.epoch` exists.~~ **Resolved, implemented**:
   `refs_export = "per_commit"` (default) or `"cadence"`. The cadenced tier fronts the
   refs object with an in-memory working copy: lease operations pass through
   synchronously (exclusivity is never cached), snapshots/pins/meta export
   immediately (an API-confirmed snapshot must survive the owner's death, I8), and
   `head`/`rpo` export on `refs_export_interval_s` (default 300 s), at serve, and at
   release. The crash gap is closed at the next serve by an epoch-filtered forward
   scan (`recover_head`): commits of exactly the owner-of-record's epoch, newer than
   the exported head (bounded by listing mtimes), newest complete closure wins.
   The scan's epoch filter is airtight because every owner exports at serve and at
   release, so the un-exported tail always belongs to exactly the lease row's owner
   — the *single-epoch-gap invariant*. Per-commit stays the default until the
   cadence tier has soaked.
2. **Thin handoff on the ungraceful path.** D-M11 + D-M13 make a *planned*
   reschedule lossless and an O(1) restage. A node that dies without running unstage
   still loses the delta since the last cycle — bounded, but non-zero. Whether that
   warrants a writethrough-cache option (trading steady-state write latency for a
   zero-delta failure path) is a product decision, not a design gap.
3. **Read-path latency after D-M8/D-M10.** Lazy materialization turns a synchronous
   map lookup into a possible object GET on the first touch of a shard. The `emap`
   object should absorb most of it, but the tail deserves measurement before it
   lands on the hot path.
4. **Two NBD attaches per thin volume** (D-M11). Doubling attachments per staged
   volume affects node fd budgets and the `nbd` module's device count, which the
   chart sizes. Worth checking against the largest node shape we intend to support.

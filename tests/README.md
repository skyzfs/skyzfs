# Test layout (design §14)

| Dir | Layer | Arrives |
|---|---|---|
| in-crate `#[cfg(test)]` | 1–2: unit + property vs reference model | M0 (live now) |
| `tests/integration/` | 3: engine ↔ MinIO (testcontainers), crash-point matrix | M1 |
| `tests/e2e/` | 4: scenarios S1–S14; Tier A (qemu-io, no root) on every PR, Tier B (S4/S10/S14: root + kernel NBD + LVM thin) nightly | M2+ |
| `tests/coordinator/` | reference V3.2 coordinator (fsfreeze + lvcreate + thin_delta → control plane), scenario S14, Tier B | M7 (live now) |

Scenario table and pass criteria: `docs/skynbd_design.md` §14.4.
Coordinator contract: `docs/skynbd-snapshot-design-v3_2.md` §11.

## Reference coordinator (`tests/coordinator/coordinator_v32.py`)

The V3.2 Phase A client: fsfreeze → dm-thin snapshot → thin_delta →
objectize (EnsureObject/PutObject/AttachRange/ClearRange) → Publish →
hash-equality verify → baseline promotion with the §15.4 pin transfer.
`selftest` runs its thin_delta XML parser against fixtures with no root
needed. Scenario S14 in `tests/e2e/run.sh` drives `bootstrap` plus two
`cycle` runs against a real thin pool layered on a skynbd NBD device
and verifies every published generation bit-for-bit.

Deviation from V3.2 §6.5, documented: in S14 the thin *pool metadata*
LV also lives on the NBD-backed PV (one PV total) — an operational
simplification for the rig; production deployments should keep pool
metadata on local storage.

## Read-cache topology benchmark (`tests/bench/topology-bench.sh`)

Record-only, no skynbd in the loop: builds the §5/§7.1 node shape from
loop files (immutable pgbench origin image → dm-thin with a read-only
external origin → a read cache on top) and runs pgbench against each
candidate cache — `nocache`, `baseline` (dm-cache writethrough, the
shipped stack), `twovol` (cached PGDATA + uncached pg_wal volume),
`bcache` (writearound over a two-segment backing) and `opencas`
(write-invalidate; casadm built with `CAS_EXT_EXP=1`). Reports
origin/pool/cache device bytes, tps and latency per run, so write
amplification and origin (network) reads are measured where the
design's costs land. Dispatch `.github/workflows/topology-bench.yml`
for a shape check, or run it as root on a workstation NVMe for real
numbers: `sudo TB_WORK=/nvme/tb tests/bench/topology-bench.sh`. Findings
so far: `docs/skynbd-postgres-study.md`.

## Lessons encoded from the first nightly campaign (5 rounds to green)

Every bug that reached a privileged nightly is pinned by a rootless test
so its class cannot return unseen:

| Round | Bug | Now caught by |
|---|---|---|
| 1 | readiness probe assumed `vol1`/TCP; auth + fsck-shape drift | **S1u** (Tier A, every PR): UDS listener + authed API + lineage ingestion + fsck shape |
| 1 | rig asserted a nonexistent `errors` field | `response_shapes_are_pinned` in `skynbd-api` tests |
| 2 | `create_snap` before origin suspend (kernel EINVAL) | `dm_contract.rs` (M9 seam): mock **rejects** the wrong order; capture ordering asserted event-by-event. S14a still proves it on real kernels |
| 2 | whole-device TRIM > 32 MiB cap → EINVAL | NBD interop tests exercise discard; cap removed for no-payload cmds |
| 3 | no `/dev/mapper` nodes under raw DM ioctls | `dm_contract.rs`: mock read fails without activation; the mknod fallback itself stays in `dm_linux.rs` (Tier B) |
| 4 | O_EXCL tmeta open EBUSY under live pool | S14a (nightly) — the open-flag choice lives below the seam in `dm_linux.rs`; the mock pins the adjacent contract (mappings only under a reserved metadata snap, point-in-time) |
| 5 | nbd device release race on re-attach | per-generation devices + `nbd_attach` retry |
| 1–3 | failure artifacts contained no logs (trap `$?` clobber) | cleanup preserves `$WORK`; traps capture rc first |

The M9 `DmOps` seam delivered on the theme above: `DmStack`'s
orchestration (freeze/suspend ordering, dev-id retry, watchdog interplay,
role renames, reserve/release hygiene) now runs rootlessly against
`MockDmOps`, an in-memory thin pool that *enforces* the kernel's rules —
including the full §21.1 crash matrix with the production stack
(`crash_matrix_dm.rs`) and the whole pipeline end-to-end
(`cycle_dm_mock.rs`). The seam caught its first real bug on day one: a
no-op untag left two `b-` devices visible mid-promotion and recovery
quarantined — fixed with the `u-` (untagged) role rename, a state no
prior test (including S14a, which never crashes mid-promotion) could
reach. What stays Tier-B-only is the thin glue in `dm_linux.rs`: real
ioctl marshalling, the mknod fallback, and thinp's non-exclusive tmeta
open.

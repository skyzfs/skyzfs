//! Thin-stack provisioning (V3.2 §6.5, CSI design 6h, origin-advance
//! §5 publish-only + §7.1 read cache): build and tear down the
//! cache-less device stack for one volume —
//!
//! ```text
//!   fs → [sky-{v}-rcache (dm-cache, writethrough)] → sky-{v}-fs (shim) ─┐
//!        sky-{v}-rec (linear discard port) ─────────────────────────────┴→ sky-{v}-origin (thin)
//!            → sky-{v}-pool (thin-pool)
//!            data ⤷ sky-{v}-data  (linear on fast dev)
//!            meta ⤷ sky-{v}-tmeta (linear on fast dev)
//!            external origin ⤷ the RO NBD origin attach, raw (D-M11)
//!        rcache fast ⤷ sky-{v}-rcmeta / sky-{v}-rcdata (linear on read-cache dev)
//! ```
//!
//! The pool lives entirely on the node-local fast device — `[tmeta]
//! [data]`, carved with dm-linear — and the volume's durable identity
//! is the generation chain alone (origin-advance §5): writes reach the
//! network only as published snapshots, the D-M13 final cycle is the
//! flush, and there is no writeback cache and no writable NBD path
//! from the node. One pool per volume keeps pools single-tenant, so
//! `skip_block_zeroing` is safe (§8.7).
//!
//! The read cache (§7.1, task 15k) is the optional bracketed layer: a
//! writethrough dm-cache over the SHIM, on its own device, and the
//! filesystem mounts the cache when it is present ([`ThinStackSpec::
//! mount_path`]). Coherence is structural — everything that mutates
//! the shim's content traverses the cache, and every agent-side
//! operation below (repoint, reclaim, eviction, swing) is
//! content-preserving by Invariants A/B — so there is no invalidation
//! machinery, and never a dirty block: a cold drop is always correct,
//! with no counterpart to the old writeback flush. Layout, rationale
//! and the superseded origin-side placement live in the design's §7.1.
//!
//! Everything speaks [`DmOps`], so the whole build/teardown
//! orchestration runs against [`crate::dm_mock::MockDmOps`] in plain
//! `cargo test` — the mock enforces the kernel's creation-order and
//! in-use contracts.
//!
//! A **fresh** device must read as zeros in the metadata regions
//! (dm-thin and dm-cache both refuse garbage superblocks): a sparse
//! loop file satisfies this by construction; an LV or partition must
//! be zeroed by the operator before first use. A fast device that
//! *survives* re-staging is the reboot-recovery path — dm-thin reads
//! its own metadata back and the pool resumes with the unpublished
//! delta intact. A fast device that is *lost* loses exactly that
//! unpublished delta (bounded by the publish cadence, §5): the caller
//! re-stages a fresh pool from the lineage origin rather than
//! refusing. A lost read-cache device is nothing at all — writethrough
//! holds no state the stack cannot re-derive.

use crate::dm::DmOps;
use crate::stack::StackError;

/// Sector size everything below speaks.
const SECTOR: u64 = 512;
/// Minimum thin-pool metadata region (dm-thin refuses tiny metadata).
const TMETA_MIN: u64 = 4 << 20;
/// Minimum pool data region — the pool must hold a real unpublished
/// delta or the stack stalls into watermark advances immediately.
const DATA_MIN: u64 = 64 << 20;
/// Minimum dm-cache metadata region (read cache).
const RCMETA_MIN: u64 = 8 << 20;
/// Minimum useful cache-data region — below this the stack costs more
/// than it caches.
const RCDATA_MIN: u64 = 16 << 20;

/// Everything needed to build one volume's stack.
#[derive(Debug, Clone)]
pub struct ThinStackSpec {
    /// dm-name-safe volume id (Kubernetes `pvc-<uuid>` qualifies).
    pub volume: String,
    /// Node-local fast device carrying the whole pool — `[tmeta][data]`
    /// (loop over a preallocated file in dev, LV or partition in
    /// production), e.g. `/dev/loop3`.
    pub fast_dev: String,
    /// Volume (= thin origin, virtual) size in bytes.
    pub data_bytes: u64,
    /// Usable size of `fast_dev` in bytes. The pool's physical
    /// capacity is this minus the metadata carve — it holds the
    /// unpublished delta, not the volume (§5 sizing guidance:
    /// worst-case write volume per advance interval, with headroom).
    pub fast_bytes: u64,
    /// Thin chunk == dm-cache block (§8.2 preferred alignment). Must be
    /// a multiple of 4 KiB and at least 64 KiB.
    pub chunk_bytes: u64,
    /// External origin for the thin device (multi-node design D-M11):
    /// a read-only device serving the volume's newest published
    /// generation; unprovisioned blocks read through to it, so a fresh
    /// pool stages O(1) instead of needing a full restore. Load-bearing
    /// for the pool's whole life on this node — never-written blocks
    /// keep falling through, so the same origin must be present on
    /// every rebuild of the stack (reboot restage included). Named RAW
    /// in the thin table; the read cache sits above the shim, never
    /// here (§7.1 — the origin-side placement is the superseded one).
    pub external_origin: Option<String>,
    /// §7.1 / task 15k: the device backing the top-of-stack read
    /// cache, e.g. a second local flash carve. `None` means no cache:
    /// the filesystem mounts the shim directly, exactly as before.
    ///
    /// Deliberately its own device rather than a carve of `fast_dev`:
    /// every byte shared would come out of the pool's delta capacity
    /// on volumes that already exist — a silent capacity change,
    /// applied on upgrade, with no measurement saying the trade is
    /// right.
    ///
    /// Most useful with an `external_origin` (it is what turns origin
    /// fall-through reads local on the second touch), but not gated on
    /// one: the cache is coherent over any content below, and gating
    /// would flip the mount device when a lineage publishes its first
    /// generation.
    pub read_cache_dev: Option<String>,
    /// Usable size of `read_cache_dev` in bytes. Ignored when
    /// `read_cache_dev` is `None`.
    pub read_cache_bytes: u64,
    /// §7.1's one invalidation event: **fresh pool ⇒ cold cache.** Set
    /// by stages that REBUILD the pool from the origin rather than
    /// resume it (pool file lost or quarantined); [`build`] then zeroes
    /// the cache metadata carve instead of adopting it, so dm-cache
    /// formats fresh. Surviving `rcmeta` is otherwise load-bearing —
    /// resident mappings persisting across a reboot is what reboot
    /// warmth *is* — but a fresh pool replaces the content below the
    /// cache without traversing it: every LBA that held unpublished
    /// delta reverts to published content, and a warm cache over that
    /// would serve the dead delta from flash. A cold drop is always
    /// correct (writethrough — never dirty); a warm cache over a fresh
    /// pool is the one constructible incoherence in this layout.
    pub cold_read_cache: bool,
}

/// Derived dm device names, build order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackNames {
    pub tmeta: String,
    pub data: String,
    pub pool: String,
    pub origin: String,
    /// D-M15 shim: the linear the filesystem mounts/claims (through
    /// the read cache when one is present), so the origin below stays
    /// merely dm-claimed (Invariant B note).
    pub fs: String,
    /// D-M15 discard port: the unclaimed linear sibling the agent's
    /// reclaim discards flow through.
    pub rec: String,
    /// §7.1: metadata carve for the read cache.
    pub rcmeta: String,
    /// §7.1: data carve for the read cache.
    pub rcdata: String,
    /// §7.1: the writethrough dm-cache the filesystem mounts, sitting
    /// OVER the shim. Present only when the stack was built with a
    /// read-cache device.
    pub rcache: String,
}

impl StackNames {
    pub fn for_volume(volume: &str) -> Self {
        // Suffixes deliberately never collide with `DmStack`'s
        // single-letter role names (`sky-{v}-{b|c|u}-…`).
        Self {
            tmeta: format!("sky-{volume}-tmeta"),
            data: format!("sky-{volume}-data"),
            pool: format!("sky-{volume}-pool"),
            origin: format!("sky-{volume}-origin"),
            fs: format!("sky-{volume}-fs"),
            rec: format!("sky-{volume}-rec"),
            rcmeta: format!("sky-{volume}-rcmeta"),
            rcdata: format!("sky-{volume}-rcdata"),
            rcache: format!("sky-{volume}-rcache"),
        }
    }

    /// Build order: components before consumers (the kernel resolves
    /// table references at load time).
    ///
    /// The read-cache trio is NOT here. It is conditional on the stack
    /// having a read-cache device, and this list is what `observe`
    /// treats as "all of it" — putting an optional device in it would
    /// report every cache-less stack as `Partial` forever. See
    /// [`Self::read_cache_order`].
    pub fn build_order(&self) -> [&str; 6] {
        [
            &self.tmeta,
            &self.data,
            &self.pool,
            &self.origin,
            &self.fs,
            &self.rec,
        ]
    }

    /// §7.1's devices, in build order, for stacks that have them.
    /// These are built AFTER the shim (the cache's table names it) and
    /// torn down BEFORE it, for the same reason — the exact inverse of
    /// the superseded origin-side trio, because the cache moved from
    /// under the thin device to over the shim.
    pub fn read_cache_order(&self) -> [&str; 3] {
        [&self.rcmeta, &self.rcdata, &self.rcache]
    }
}

/// Byte layout of the fast device: `[tmeta][data]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastLayout {
    pub tmeta_bytes: u64,
    pub data_bytes: u64,
}

/// Byte layout of the read-cache device: `[rcmeta][rcdata]` (§7.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadCacheLayout {
    pub rcmeta_bytes: u64,
    pub rcdata_bytes: u64,
}

/// The stack's presence on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackState {
    /// No stack device exists.
    Absent,
    /// All six unconditional devices exist.
    Complete,
    /// Some but not all — an interrupted build (or teardown). Names are
    /// the missing ones, in build order.
    Partial(Vec<String>),
}

impl ThinStackSpec {
    pub fn names(&self) -> StackNames {
        StackNames::for_volume(&self.volume)
    }

    /// The thin device under the shim (thin dev 0 of the pool). The
    /// filesystem does NOT live here — it lives on [`Self::mount_path`]
    /// (D-M15) — so this node stays merely dm-claimed.
    pub fn origin_path(&self) -> String {
        format!("/dev/mapper/{}", self.names().origin)
    }

    /// The linear shim (D-M15): the layer that keeps the origin below
    /// merely dm-claimed, and where the guest-discard mask sits. The
    /// filesystem mounts [`Self::mount_path`], which is this shim
    /// unless a read cache sits over it.
    pub fn shim_path(&self) -> String {
        format!("/dev/mapper/{}", self.names().fs)
    }

    /// The device the filesystem lives on: the read cache when the
    /// stack has one, the shim otherwise. The fs claim lands here;
    /// everything below stays dm-claimed and therefore repointable,
    /// reclaimable, and (for the cache) droppable.
    pub fn mount_path(&self) -> String {
        if self.read_cache() {
            format!("/dev/mapper/{}", self.names().rcache)
        } else {
            self.shim_path()
        }
    }

    fn validate(&self) -> Result<(), StackError> {
        validate_geometry(self.data_bytes, self.chunk_bytes)
    }

    /// Deterministic carve of the fast device — see [`fast_layout`].
    pub fn layout(&self) -> Result<FastLayout, StackError> {
        fast_layout(self.data_bytes, self.fast_bytes, self.chunk_bytes)
    }

    /// Is the read cache (§7.1) in play for this stack?
    pub fn read_cache(&self) -> bool {
        self.read_cache_dev.is_some()
    }

    /// Carve of the SEPARATE read-cache device: `[rcmeta][rcdata]`.
    ///
    /// Same shape and floors posture as [`Self::layout`], minus the
    /// thin-pool metadata — this device carries nothing but the cache.
    /// Returns `Ok(None)` when there is no cache to size, so callers
    /// can stay branch-free.
    pub fn read_cache_layout(&self) -> Result<Option<ReadCacheLayout>, StackError> {
        if !self.read_cache() {
            return Ok(None);
        }
        self.validate()?;
        let align = |b: u64| b.next_multiple_of(self.chunk_bytes);
        let rcmeta = align((self.read_cache_bytes / 500).clamp(RCMETA_MIN, 1 << 30));
        let rest = self.read_cache_bytes.saturating_sub(rcmeta);
        let rcdata = rest / self.chunk_bytes * self.chunk_bytes;
        if rcdata < RCDATA_MIN {
            return Err(StackError::NotFound(format!(
                "read-cache device too small: {} bytes leaves {} for cache \
                 data (rcmeta {}, need >= {})",
                self.read_cache_bytes, rcdata, rcmeta, RCDATA_MIN
            )));
        }
        Ok(Some(ReadCacheLayout {
            rcmeta_bytes: rcmeta,
            rcdata_bytes: rcdata,
        }))
    }
}

fn validate_geometry(data_bytes: u64, chunk_bytes: u64) -> Result<(), StackError> {
    if chunk_bytes < (64 << 10) || !chunk_bytes.is_multiple_of(4096) {
        return Err(StackError::NotFound(format!(
            "chunk_bytes {chunk_bytes} must be a 4KiB multiple >= 64KiB"
        )));
    }
    if data_bytes == 0 || !data_bytes.is_multiple_of(chunk_bytes) {
        return Err(StackError::NotFound(format!(
            "data_bytes {data_bytes} must be a non-zero multiple of chunk_bytes {chunk_bytes}"
        )));
    }
    Ok(())
}

/// Deterministic carve of the fast device: `[tmeta][data]`, proportional
/// guesses with hard floors — the metadata region is cheap to
/// over-provision and fatal to exhaust (§8.6 conservative-guardrails
/// posture); everything left is pool data.
///
/// A free function on purpose, not only [`ThinStackSpec::layout`]: the
/// slot wiring (the §7 two-pool swing on provisioned stacks) must derive
/// the SAME split the build used from the same three inputs — a swing's
/// B side is carved at these offsets and a reboot's idempotent resume
/// reads pool metadata back at them, so a second copy of this math would
/// be a data-corrupting divergence waiting for a tuning commit.
pub fn fast_layout(
    data_bytes: u64,
    fast_bytes: u64,
    chunk_bytes: u64,
) -> Result<FastLayout, StackError> {
    validate_geometry(data_bytes, chunk_bytes)?;
    let align = |b: u64| b.next_multiple_of(chunk_bytes);
    let tmeta = align((data_bytes / 1000).clamp(TMETA_MIN, 1 << 30));
    let rest = fast_bytes.saturating_sub(tmeta);
    // Round the pool data region down to whole chunks.
    let data = rest / chunk_bytes * chunk_bytes;
    if data < DATA_MIN {
        return Err(StackError::NotFound(format!(
            "fast device too small: {fast_bytes} bytes leaves {data} for pool data \
             (tmeta {tmeta}, need >= {DATA_MIN})"
        )));
    }
    Ok(FastLayout {
        tmeta_bytes: tmeta,
        data_bytes: data,
    })
}

fn mm(ops: &impl DmOps, name: &str) -> Result<String, StackError> {
    let info = ops.device_info(name)?;
    Ok(format!("{}:{}", info.major, info.minor))
}

fn uuid(volume: &str, part: &str) -> String {
    // Distinct namespace from DmStack's snapshot uuids
    // (`SKYNBD-{volume}-{dev_id}`).
    format!("SKYNBD-{volume}-stack-{part}")
}

/// Which stack devices exist right now.
pub fn observe(ops: &impl DmOps, volume: &str) -> Result<StackState, StackError> {
    let names = StackNames::for_volume(volume);
    let have: std::collections::HashSet<String> =
        ops.list_devices()?.into_iter().map(|d| d.name).collect();
    let missing: Vec<String> = names
        .build_order()
        .iter()
        .filter(|n| !have.contains(**n))
        .map(|n| n.to_string())
        .collect();
    Ok(if missing.len() == names.build_order().len() {
        StackState::Absent
    } else if missing.is_empty() {
        StackState::Complete
    } else {
        StackState::Partial(missing)
    })
}

/// Build (or finish building) the stack. Idempotent: devices that
/// already exist are kept — which is both the crash-resume path and the
/// reboot re-stage path (the fast device's own metadata carries the
/// pool state; dm reads it back on activation).
pub fn build(ops: &impl DmOps, spec: &ThinStackSpec) -> Result<(), StackError> {
    let layout = spec.layout()?;
    let names = spec.names();
    let mut have: std::collections::HashSet<String> =
        ops.list_devices()?.into_iter().map(|d| d.name).collect();
    let sectors = |b: u64| b / SECTOR;
    let data_sectors = sectors(spec.data_bytes);
    let chunk_sectors = sectors(spec.chunk_bytes);

    // Two linear carves of the fast device: the whole pool is local.
    let regions = [
        (&names.tmeta, "tmeta", 0u64, layout.tmeta_bytes),
        (&names.data, "data", layout.tmeta_bytes, layout.data_bytes),
    ];
    for (name, part, off, len) in regions {
        if have.contains(name.as_str()) {
            continue;
        }
        ops.create_device(
            name,
            &uuid(&spec.volume, part),
            &[(
                0,
                sectors(len),
                "linear".into(),
                format!("{} {}", spec.fast_dev, sectors(off)),
            )],
        )?;
    }

    // Thin pool: data AND metadata are the local carves — no cache, no
    // head, no writable network path (origin-advance §5). Physical
    // capacity is the data carve; the volume's virtual size lives on
    // the thin device. Low-water in pool blocks; single-tenant pool
    // skips zeroing (§8.7). `no_discard_passdown` (D-M15 Invariant B):
    // the agent's reclaim unmaps blocks whose content the external
    // origin serves identically — the unmap must stop at the pool,
    // never punching through to the carve below.
    if !have.contains(names.pool.as_str()) {
        let blocks = layout.data_bytes / spec.chunk_bytes;
        let low_water = (blocks / 20).max(32);
        ops.create_device(
            &names.pool,
            &uuid(&spec.volume, "pool"),
            &[(
                0,
                sectors(layout.data_bytes),
                "thin-pool".into(),
                format!(
                    "{} {} {} {} 2 skip_block_zeroing no_discard_passdown",
                    mm(ops, &names.tmeta)?,
                    mm(ops, &names.data)?,
                    chunk_sectors,
                    low_water
                ),
            )],
        )?;
    }

    // The origin thin device (dev id 0). EEXIST from a previous
    // interrupted build is fine — the id survives in pool metadata.
    if !have.contains(names.origin.as_str()) {
        match ops.pool_message(&names.pool, "create_thin 0") {
            Ok(()) => {}
            Err(e) if e.to_string().contains("EEXIST") => {}
            Err(e) => return Err(e),
        }
        // `thin <pool> <dev_id> [<external_origin>]` — with an origin,
        // unprovisioned reads fall through to it (D-M11), RAW: the read
        // cache does not sit here (§7.1 — a cache below this reference
        // is the superseded placement whose staleness rule this layout
        // exists to avoid).
        let table = match &spec.external_origin {
            Some(origin) => format!("{} 0 {origin}", mm(ops, &names.pool)?),
            None => format!("{} 0", mm(ops, &names.pool)?),
        };
        ops.create_device(
            &names.origin,
            &uuid(&spec.volume, "origin"),
            &[(0, data_sectors, "thin".into(), table)],
        )?;
    }

    // D-M15 shim + port (the Invariant B implementation note): two
    // linears over the thin origin. The filesystem's claim lands on
    // the mount device — the shim, or the read cache over it — so the
    // origin below stays merely dm-claimed, repointable and
    // reclaimable. The PORT is the unclaimed node the agent's reclaim
    // discards flow through: discard bios, unlike the BLKDISCARD ioctl
    // entry, never re-check claims, so they pass through dm-linear
    // into dm-thin and unmap the origin's blocks.
    for (name, part) in [(&names.fs, "fs"), (&names.rec, "rec")] {
        if have.contains(name.as_str()) {
            continue;
        }
        ops.create_device(
            name,
            &uuid(&spec.volume, part),
            &[(
                0,
                data_sectors,
                "linear".into(),
                format!("{} 0", mm(ops, &names.origin)?),
            )],
        )?;
    }
    // Invariant B's mask on the shim. On an uncached stack this is the
    // guest-facing wall: the filesystem claims the shim, so a guest
    // `fstrim` bottoms out here with EOPNOTSUPP. On a cached stack its
    // job is different — dm-cache reads the slow device's zeroed
    // discard limit at ctr time and DISABLES passdown, so nothing the
    // cache does can forward a discard to the shim. What dm-cache does
    // NOT do is re-export the mask: with passdown off it advertises
    // its own virtual discard limits and ABSORBS guest discard bios
    // (marks its discard bitset and completes them — no invalidation,
    // no forwarding), so the guest-facing refusal has to be re-asserted
    // on the cache device itself, below. Ordering is load-bearing
    // either way: the shim must be masked before the cache ctr reads
    // its limits. Unconditional, not inside the create above: an
    // idempotent resume keeps an existing shim, and a table this stack
    // did not load carries whatever queue limits it was born with.
    // That unconditionality is the re-derivation task 15j asks for —
    // every stage re-asserts the mask from observation, so a shim
    // rebuilt outside this function cannot lapse unnoticed.
    //
    // FATAL here, unlike at the swing's retarget (§8.1 / task 15j): a
    // mounted origin-backed volume whose shim passes guest discards is
    // the hazard Invariant B declares unsupported, and the stage is the
    // one moment nothing is mounted yet, so refusing costs a failed
    // stage instead of a diverged image.
    crate::dm::mask_guest_discard(ops, &names.fs)?;

    // §7.1: the read cache, OVER the shim — built last because its
    // table names the shim, and after the mask so an unmaskable shim
    // fails the stage before any cache exists to tear down. Two linear
    // carves of the separate read-cache device, then a WRITETHROUGH
    // dm-cache whose slow device is the shim; the filesystem mounts
    // this instead of the shim ([`ThinStackSpec::mount_path`]).
    //
    // Writethrough is load-bearing, not a tuning choice: every write
    // lands on the shim (local pool) synchronously, so the cache never
    // holds dirty state and a cold drop is always correct — crash
    // recovery included. There is deliberately no flush counterpart.
    match spec.read_cache_layout()? {
        Some(rl) => {
            // Fresh pool ⇒ cold cache (§7.1's one invalidation event,
            // see [`ThinStackSpec::cold_read_cache`]): a pool rebuilt
            // from the origin replaced the content below the cache
            // without traversing it, so surviving resident mappings
            // would serve the dead unpublished delta. Take any
            // surviving trio down (consumers first — nothing can be
            // zeroed under a live cache) and zero the metadata carve
            // below so dm-cache formats fresh instead of adopting it.
            if spec.cold_read_cache {
                for name in names.read_cache_order().iter().rev() {
                    if have.remove(*name) {
                        ops.remove_device(name)?;
                    }
                }
            }
            let rregions = [
                (&names.rcmeta, "rcmeta", 0u64, rl.rcmeta_bytes),
                (&names.rcdata, "rcdata", rl.rcmeta_bytes, rl.rcdata_bytes),
            ];
            for (name, part, off, len) in rregions {
                if have.contains(name.as_str()) {
                    continue;
                }
                ops.create_device(
                    name,
                    &uuid(&spec.volume, part),
                    &[(
                        0,
                        sectors(len),
                        "linear".into(),
                        format!(
                            "{} {}",
                            spec.read_cache_dev.as_deref().unwrap(),
                            sectors(off)
                        ),
                    )],
                )?;
            }
            // The zero targets the dm-cache SUPERBLOCK (the first 4 KiB
            // metadata block): an all-zero superblock is dm-cache's own
            // "format me" marker, so the next ctr initializes fresh
            // metadata over the whole carve. Written through the rcmeta
            // linear (not the raw device) so the offset math lives in
            // one place, and BEFORE the cache ctr below reads it.
            if spec.cold_read_cache {
                ops.write_dev(&names.rcmeta, 0, &[0u8; 4096])?;
            }
            if !have.contains(names.rcache.as_str()) {
                ops.create_device(
                    &names.rcache,
                    &uuid(&spec.volume, "rcache"),
                    &[(
                        0,
                        data_sectors,
                        "cache".into(),
                        format!(
                            "{} {} {} {} 1 writethrough default 0",
                            mm(ops, &names.rcmeta)?,
                            mm(ops, &names.rcdata)?,
                            mm(ops, &names.fs)?,
                            chunk_sectors
                        ),
                    )],
                )?;
            }
            // Invariant B's guest-facing mask, restored on the device
            // the guest actually sees: with passdown disabled, dm-cache
            // advertises nonzero virtual discard limits — the shim's
            // mask above is invisible through it, so an unmasked rcache
            // turns `fstrim` from EOPNOTSUPP into real (absorbed) bios.
            // The absorb keeps Invariant B true today, but it is a
            // load-time, DMWARN-only kernel decision with no readback;
            // this mask makes the guest never issue discards at all —
            // today's posture — and demotes the absorb to defense in
            // depth. Unconditional and FATAL for the same reasons as
            // the shim's, and AFTER the ctr so the ctr's passdown
            // decision has already read the shim's zero.
            crate::dm::mask_guest_discard(ops, &names.rcache)?;
        }
        None => {
            // A surviving trio under a spec that carries no cache — the
            // Some→None restage (config rollback) — would otherwise
            // wedge the stage: the stale rcache's table still names the
            // shim, so the shim stays dm-claimed and the mount answers
            // EBUSY, and the idempotent-resume path never calls
            // teardown to clear it. Presence-keyed removal, consumers
            // first — the mirror image of teardown's ordering.
            for name in names.read_cache_order().iter().rev() {
                if have.remove(*name) {
                    ops.remove_device(name)?;
                }
            }
        }
    }
    Ok(())
}

/// Remove the stack, consumers before components (reverse build order).
/// Tolerates absent devices (interrupted builds, replayed teardowns);
/// propagates EBUSY — a mounted device must be unmounted by the caller
/// first.
///
/// The read-cache trio goes FIRST, and that ordering is forced: the
/// cache's table names the shim, so the shim cannot be removed until
/// the cache over it is gone. Keyed on presence rather than on a spec
/// — teardown takes only a volume name, and a stack built with the
/// cache must tear down cleanly even when whatever is calling has no
/// idea it was there.
pub fn teardown(ops: &impl DmOps, volume: &str) -> Result<(), StackError> {
    let names = StackNames::for_volume(volume);
    let have: std::collections::HashSet<String> =
        ops.list_devices()?.into_iter().map(|d| d.name).collect();
    for name in names.read_cache_order().iter().rev() {
        if !have.contains(*name) {
            continue;
        }
        ops.remove_device(name)?;
    }
    for name in names.build_order().iter().rev() {
        if !have.contains(*name) {
            continue;
        }
        ops.remove_device(name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dm_mock::{MockDmOps, MockEvent};

    pub(super) fn spec() -> ThinStackSpec {
        ThinStackSpec {
            volume: "pvc-1".into(),
            fast_dev: "/dev/loop7".into(),
            data_bytes: 1 << 30,
            fast_bytes: 256 << 20,
            chunk_bytes: 64 << 10,
            external_origin: None,
            read_cache_dev: None,
            read_cache_bytes: 0,
            cold_read_cache: false,
        }
    }

    /// The same stack WITH §7.1's read cache and an external origin —
    /// the deployment shape the cache exists for.
    pub(super) fn spec_with_read_cache() -> ThinStackSpec {
        ThinStackSpec {
            external_origin: Some("/dev/nbd1".into()),
            read_cache_dev: Some("/dev/loop8".into()),
            read_cache_bytes: 256 << 20,
            ..spec()
        }
    }

    #[test]
    fn layout_carves_with_floors_and_alignment() {
        let l = spec().layout().unwrap();
        // 1 GiB / 1000 < 4 MiB floor.
        assert_eq!(l.tmeta_bytes, (4u64 << 20).next_multiple_of(64 << 10));
        assert!(l.data_bytes >= DATA_MIN);
        assert!(l.data_bytes.is_multiple_of(64 << 10));
        assert!(l.tmeta_bytes + l.data_bytes <= 256 << 20);

        // Too small to be useful → loud error.
        let mut s = spec();
        s.fast_bytes = 16 << 20;
        assert!(s.layout().is_err());
        // Unaligned volume size → loud error.
        let mut s = spec();
        s.data_bytes = (1 << 30) + 4096;
        assert!(s.layout().is_err());
        // Undersized chunk → loud error.
        let mut s = spec();
        s.chunk_bytes = 4096;
        assert!(s.layout().is_err());
    }

    #[test]
    fn build_creates_the_stack_in_dependency_order() {
        let ops = MockDmOps::empty();
        let s = spec();
        build(&ops, &s).unwrap();

        let created: Vec<String> = ops
            .events()
            .into_iter()
            .filter_map(|e| match e {
                MockEvent::Create(n) => Some(n),
                _ => None,
            })
            .collect();
        assert_eq!(
            created,
            vec![
                "sky-pvc-1-tmeta",
                "sky-pvc-1-data",
                "sky-pvc-1-pool",
                "sky-pvc-1-origin",
                "sky-pvc-1-fs",
                "sky-pvc-1-rec"
            ]
        );
        assert_eq!(observe(&ops, "pvc-1").unwrap(), StackState::Complete);

        // Idempotent replay: nothing new created.
        build(&ops, &s).unwrap();
        let creates = ops
            .events()
            .iter()
            .filter(|e| matches!(e, MockEvent::Create(_)))
            .count();
        assert_eq!(creates, 6);
    }

    /// Origin-advance §5: the pool is entirely local — data and
    /// metadata are carves of the fast device, no device in the stack
    /// is a writeback cache, and no table names an NBD device except
    /// the thin's external origin.
    #[test]
    fn the_pool_is_local_and_nothing_is_writeback() {
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        let data_mm = {
            let info = ops.device_info("sky-pvc-1-data").unwrap();
            format!("{}:{}", info.major, info.minor)
        };
        let pool = ops.table("sky-pvc-1-pool").unwrap();
        assert!(
            pool[0].3.contains(&data_mm),
            "pool data is the local carve: {}",
            pool[0].3
        );
        for name in [
            "sky-pvc-1-tmeta",
            "sky-pvc-1-data",
            "sky-pvc-1-pool",
            "sky-pvc-1-origin",
            "sky-pvc-1-fs",
            "sky-pvc-1-rec",
            "sky-pvc-1-rcmeta",
            "sky-pvc-1-rcdata",
            "sky-pvc-1-rcache",
        ] {
            let t = ops.table(name).unwrap();
            assert!(
                !t[0].3.contains("writeback"),
                "{name} must not be writeback: {}",
                t[0].3
            );
        }
    }

    /// D-M15 shim + port: both linears sit directly over the thin
    /// origin, full-size, so the fs claim lands on the shim and reclaim
    /// discards flow through the unclaimed port.
    #[test]
    fn shim_and_port_are_linears_over_the_origin() {
        let ops = MockDmOps::empty();
        let s = spec();
        build(&ops, &s).unwrap();
        let origin_mm = {
            let info = ops.device_info("sky-pvc-1-origin").unwrap();
            format!("{}:{}", info.major, info.minor)
        };
        for name in ["sky-pvc-1-fs", "sky-pvc-1-rec"] {
            let t = ops.table(name).unwrap();
            assert_eq!(t.len(), 1);
            assert_eq!(t[0].2, "linear");
            assert_eq!(t[0].1, s.data_bytes / SECTOR, "{name} spans the volume");
            assert_eq!(
                t[0].3,
                format!("{origin_mm} 0"),
                "{name} targets the origin"
            );
        }
        assert_eq!(s.shim_path(), "/dev/mapper/sky-pvc-1-fs");
        assert_eq!(s.mount_path(), s.shim_path(), "no cache → mount the shim");
    }

    /// Invariant B's guest-discard mask, interim form: the build zeroes
    /// the SHIM queue's discard limit — a guest `fstrim` (the default
    /// `fstrim.timer`) is refused rather than unmapping blocks the
    /// origin still serves — while the PORT keeps its own, because the
    /// agent's reclaim discards enter there. Getting the two the wrong
    /// way round disables reclaim, so both halves are asserted.
    #[test]
    fn the_shim_masks_guest_discard_and_the_port_does_not() {
        let ops = MockDmOps::empty();
        build(&ops, &spec()).unwrap();
        assert_eq!(ops.discard_max_set("sky-pvc-1-fs"), Some(0));
        assert_eq!(ops.discard_max_set("sky-pvc-1-rec"), None);
        assert_eq!(ops.discard_max_set("sky-pvc-1-origin"), None);
        // What the mask means, as the block layer means it.
        let bytes = spec().data_bytes;
        assert!(
            ops.discard_dev("sky-pvc-1-fs", 0, bytes)
                .unwrap_err()
                .to_string()
                .contains("EOPNOTSUPP"),
            "the shim must refuse guest discards"
        );
        // The port's queue is untouched, so whatever it refuses is about
        // the range, never the mask (this fixture models no pool
        // geometry — `dm_swing`'s rig discards through a real one).
        let port = ops.discard_dev("sky-pvc-1-rec", 0, bytes).unwrap_err();
        assert!(
            !port.to_string().contains("EOPNOTSUPP"),
            "the port stays discardable — reclaim depends on it: {port}"
        );
    }

    /// Task 15j: the mask is verified, not assumed. A queue that
    /// accepts the write and drops it leaves a shim that will pass a
    /// guest `fstrim` through while every log line says it is guarded —
    /// so the readback is what decides, and at the stage the verdict is
    /// FATAL: nothing is mounted yet, so refusing costs a failed stage
    /// instead of an image that diverges from the published generation.
    #[test]
    fn a_mask_that_does_not_take_fails_the_stage() {
        let ops = MockDmOps::empty();
        ops.set_discard_knob_ignored("sky-pvc-1-fs");
        let e = build(&ops, &spec()).expect_err("an unmasked shim must fail the build");
        assert!(e.to_string().contains("guest-discard mask"), "{e}");
        // And the mask really is off: the block layer would take the
        // guest's discard.
        assert!(!ops
            .discard_dev("sky-pvc-1-fs", 0, spec().data_bytes)
            .unwrap_err()
            .to_string()
            .contains("EOPNOTSUPP"));
    }

    /// The tolerated arm: a kernel with no `queue/discard_max_bytes` at
    /// all. There is nothing to assert and nothing to re-assert, and a
    /// capability the kernel never had must not make a volume
    /// unstageable — the build succeeds and the host is logged once.
    #[test]
    fn a_kernel_without_the_knob_still_stages() {
        let ops = MockDmOps::empty();
        ops.set_discard_knob_absent("sky-pvc-1-fs");
        build(&ops, &spec()).expect("no knob is not a stage failure");
        assert_eq!(ops.discard_max_set("sky-pvc-1-fs"), None);
    }

    /// The re-derivation half of the 15j row: `build` asserts the mask
    /// unconditionally, so a resume over a shim this stack did NOT
    /// create — a table carrying whatever queue limits it was born with
    /// — is re-masked rather than trusted.
    #[test]
    fn a_resume_re_asserts_the_mask_on_a_foreign_shim() {
        let ops = MockDmOps::empty();
        build(&ops, &spec()).unwrap();
        // Something outside this stack reloaded the shim and its queue
        // limits came back at the kernel default.
        ops.set_discard_max_bytes("sky-pvc-1-fs", 1 << 30).unwrap();
        assert_eq!(ops.discard_max_set("sky-pvc-1-fs"), Some(1 << 30));
        build(&ops, &spec()).expect("idempotent resume");
        assert_eq!(ops.discard_max_set("sky-pvc-1-fs"), Some(0));
    }

    /// D-M11 + §7.1: the thin origin's table names the external origin
    /// device RAW when the spec carries one — the read cache never sits
    /// in this reference, whatever else is configured — and stays the
    /// plain two-argument form otherwise.
    #[test]
    fn origin_table_carries_the_raw_external_origin() {
        let ops = MockDmOps::empty();
        build(&ops, &spec()).unwrap();
        let t = ops.table("sky-pvc-1-origin").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].2, "thin");
        assert_eq!(
            t[0].3.split_whitespace().count(),
            2,
            "no origin → `<pool> <dev_id>`: {}",
            t[0].3
        );

        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        let t = ops.table("sky-pvc-1-origin").unwrap();
        assert!(
            t[0].3.ends_with(" 0 /dev/nbd1"),
            "`<pool> <dev_id> <external_origin>`, raw even with the \
             cache on: {}",
            t[0].3
        );
    }

    #[test]
    fn interrupted_build_resumes_from_where_it_stopped() {
        let ops = MockDmOps::empty();
        let s = spec();
        build(&ops, &s).unwrap();
        // Simulate a crash between pool and origin: remove the origin
        // (holders first — the mock enforces the EBUSY contract).
        ops.remove_device("sky-pvc-1-rec").unwrap();
        ops.remove_device("sky-pvc-1-fs").unwrap();
        ops.remove_device("sky-pvc-1-origin").unwrap();
        assert_eq!(
            observe(&ops, "pvc-1").unwrap(),
            StackState::Partial(vec![
                "sky-pvc-1-origin".into(),
                "sky-pvc-1-fs".into(),
                "sky-pvc-1-rec".into()
            ])
        );
        build(&ops, &s).unwrap();
        assert_eq!(observe(&ops, "pvc-1").unwrap(), StackState::Complete);
    }

    #[test]
    fn teardown_removes_consumers_first_and_replays_clean() {
        let ops = MockDmOps::empty();
        let s = spec();
        build(&ops, &s).unwrap();
        teardown(&ops, "pvc-1").unwrap();
        assert_eq!(observe(&ops, "pvc-1").unwrap(), StackState::Absent);
        // The mock's EBUSY contract would have failed any wrong order.
        teardown(&ops, "pvc-1").unwrap(); // replay: nothing to do
    }

    #[test]
    fn component_removal_while_stack_lives_is_ebusy() {
        let ops = MockDmOps::empty();
        build(&ops, &spec()).unwrap();
        let err = ops.remove_device("sky-pvc-1-data").unwrap_err();
        assert!(err.to_string().contains("EBUSY"), "{err}");
    }

    #[test]
    fn two_volumes_coexist() {
        let ops = MockDmOps::empty();
        let a = spec();
        let mut b = spec();
        b.volume = "pvc-2".into();
        b.fast_dev = "/dev/loop8".into();
        build(&ops, &a).unwrap();
        build(&ops, &b).unwrap();
        assert_eq!(observe(&ops, "pvc-1").unwrap(), StackState::Complete);
        assert_eq!(observe(&ops, "pvc-2").unwrap(), StackState::Complete);
        teardown(&ops, "pvc-1").unwrap();
        assert_eq!(observe(&ops, "pvc-1").unwrap(), StackState::Absent);
        assert_eq!(observe(&ops, "pvc-2").unwrap(), StackState::Complete);
    }
}

#[cfg(test)]
mod read_cache_tests {
    use super::tests::{spec, spec_with_read_cache};
    use super::*;

    #[test]
    fn the_read_cache_is_keyed_on_the_device_alone() {
        // Unlike the superseded origin-side placement, no external
        // origin is required: the top cache is coherent over any
        // content below it, and gating on the origin would flip the
        // MOUNT DEVICE when a lineage publishes its first generation.
        let mut s = spec();
        assert!(!s.read_cache(), "no device → no cache");
        s.read_cache_dev = Some("/dev/loop8".into());
        s.read_cache_bytes = 256 << 20;
        assert!(s.read_cache(), "a device alone is enough — no origin");
        assert!(spec_with_read_cache().read_cache());
    }

    #[test]
    fn no_read_cache_means_no_layout_and_no_error() {
        // `Ok(None)`, not `Err`: a stack without the cache is the normal
        // case, not a misconfiguration.
        assert_eq!(spec().read_cache_layout().unwrap(), None);
    }

    #[test]
    fn the_cache_device_is_carved_metadata_then_data() {
        let s = spec_with_read_cache();
        let l = s.read_cache_layout().unwrap().unwrap();
        assert!(l.rcmeta_bytes >= RCMETA_MIN, "floor honoured");
        assert!(l.rcdata_bytes >= RCDATA_MIN);
        assert!(
            l.rcmeta_bytes.is_multiple_of(s.chunk_bytes)
                && l.rcdata_bytes.is_multiple_of(s.chunk_bytes),
            "both regions are whole cache blocks"
        );
        assert!(
            l.rcmeta_bytes + l.rcdata_bytes <= s.read_cache_bytes,
            "the carve fits the device it is carving"
        );
    }

    #[test]
    fn a_cache_device_too_small_to_be_worth_it_is_refused() {
        let mut s = spec_with_read_cache();
        s.read_cache_bytes = 16 << 20; // swallowed by the metadata floor
        let err = s.read_cache_layout().unwrap_err().to_string();
        assert!(
            err.contains("read-cache device too small"),
            "names the CACHE device, not the pool one: {err}"
        );
    }

    #[test]
    fn the_pool_carve_is_untouched_by_all_of_this() {
        // The whole point of a separate device: turning the read cache
        // on must not move a single byte of the existing layout, or the
        // pool's delta capacity silently shrinks on upgrade.
        assert_eq!(
            spec().layout().unwrap(),
            spec_with_read_cache().layout().unwrap()
        );
    }

    #[test]
    fn the_read_cache_devices_are_not_in_the_base_build_order() {
        // `observe` treats `build_order` as "all of it". An optional
        // device in that list reports every cache-less stack as Partial.
        let n = StackNames::for_volume("pvc-1");
        for d in n.read_cache_order() {
            assert!(
                !n.build_order().contains(&d),
                "{d} must not be in the unconditional build order"
            );
        }
        assert_eq!(
            n.read_cache_order(),
            ["sky-pvc-1-rcmeta", "sky-pvc-1-rcdata", "sky-pvc-1-rcache"]
        );
    }
}

#[cfg(test)]
mod read_cache_build_tests {
    use super::tests::{spec, spec_with_read_cache};
    use super::*;
    use crate::dm_mock::{MockDmOps, MockEvent};

    fn table_of(ops: &MockDmOps, name: &str) -> String {
        ops.table(name)
            .unwrap_or_else(|| panic!("{name} not created"))
            .iter()
            .map(|(_, _, t, p)| format!("{t} {p}"))
            .collect::<Vec<_>>()
            .join(";")
    }

    #[test]
    fn the_filesystem_mounts_the_cache_over_the_shim() {
        // The point of the placement: the cache brackets the WHOLE
        // stack — its slow device is the shim, and the mount device is
        // the cache, so every read and every write traverses it and
        // coherence is structural (§7.1).
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        let shim_mm = {
            let info = ops.device_info(&s.names().fs).unwrap();
            format!("{}:{}", info.major, info.minor)
        };
        let t = table_of(&ops, &s.names().rcache);
        assert!(
            t.contains(&shim_mm),
            "the cache's slow device is the shim: {t}"
        );
        assert_eq!(s.mount_path(), "/dev/mapper/sky-pvc-1-rcache");
    }

    #[test]
    fn without_the_cache_the_stack_is_exactly_as_before() {
        // Mutation guard for the regression that would matter most:
        // the cache must not reroute stacks that did not ask for it.
        let ops = MockDmOps::empty();
        let mut s = spec();
        s.external_origin = Some("/dev/nbd1".into());
        build(&ops, &s).unwrap();
        let thin = table_of(&ops, &s.names().origin);
        assert!(thin.contains("/dev/nbd1"), "raw origin preserved: {thin}");
        assert!(ops.table(&s.names().rcache).is_none(), "no cache built");
        assert_eq!(s.mount_path(), s.shim_path());
    }

    #[test]
    fn the_read_cache_is_writethrough() {
        // Never dirty is the safety argument for dropping it cold —
        // and, with the cache in the write path, writethrough is also
        // what keeps a write's durability on the local pool rather
        // than in cache state. The mode is load-bearing, not a knob.
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        let t = table_of(&ops, &s.names().rcache);
        assert!(t.contains("writethrough"), "{t}");
        assert!(!t.contains("writeback"), "{t}");
    }

    #[test]
    fn teardown_removes_the_cache_before_the_shim_below_it() {
        // The cache's table references the shim, so removing the shim
        // first is EBUSY. The mock enforces the kernel's in-use rule,
        // which is what makes this a real check and not a shape test.
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        teardown(&ops, &s.volume).unwrap();
        for d in s.names().read_cache_order() {
            assert!(ops.table(d).is_none(), "{d} survived teardown");
        }
        assert_eq!(observe(&ops, &s.volume).unwrap(), StackState::Absent);
    }

    #[test]
    fn teardown_of_a_cache_less_stack_is_unchanged() {
        let ops = MockDmOps::empty();
        let s = spec();
        build(&ops, &s).unwrap();
        teardown(&ops, &s.volume).unwrap();
    }

    #[test]
    fn build_replays_over_a_half_built_read_cache() {
        // Interrupted builds are the normal case on a node that
        // rebooted mid-stage; the trio must be idempotent like the rest.
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        build(&ops, &s).unwrap();
        assert!(ops.table(&s.names().rcache).is_some());
    }

    #[test]
    fn the_mount_device_is_masked_after_the_cache_ctr() {
        // Invariant B's guest-facing mask, on the device the guest
        // actually sees: dm-cache advertises its own virtual discard
        // limits (passdown disabled), so the shim's mask does not
        // re-export through it. Ordering matters twice — the SHIM's
        // mask before the cache ctr (that zero is what disables
        // passdown), the CACHE's mask after its own creation.
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        let names = s.names();
        let ev = ops.events();
        let pos = |e: &MockEvent| {
            ev.iter()
                .position(|x| x == e)
                .unwrap_or_else(|| panic!("{e:?} missing"))
        };
        let shim_mask = pos(&MockEvent::SetDiscardMax(names.fs.clone(), 0));
        let ctr = pos(&MockEvent::Create(names.rcache.clone()));
        let cache_mask = pos(&MockEvent::SetDiscardMax(names.rcache.clone(), 0));
        assert!(
            shim_mask < ctr,
            "shim masked before the cache ctr reads its limits"
        );
        assert!(ctr < cache_mask, "mount device masked after it exists");
    }

    #[test]
    fn a_cold_build_zeroes_the_cache_metadata_before_the_ctr() {
        // Fresh pool ⇒ cold cache (§7.1's one invalidation event): the
        // metadata carve persists on its own device, so a pool rebuilt
        // from the origin — content replaced below the cache without
        // traversing it — must not adopt the surviving mappings. The
        // zero is the dm-cache superblock ("format me"), and it lands
        // before the ctr that would read it.
        let ops = MockDmOps::empty();
        let mut s = spec_with_read_cache();
        s.cold_read_cache = true;
        build(&ops, &s).unwrap();
        let names = s.names();
        let ev = ops.events();
        let zero = ev
            .iter()
            .position(|e| matches!(e, MockEvent::WriteDev(n, 0, 4096) if *n == names.rcmeta))
            .expect("cold build must zero the rcmeta superblock");
        let ctr = ev
            .iter()
            .position(|e| *e == MockEvent::Create(names.rcache.clone()))
            .unwrap();
        assert!(zero < ctr, "the zero must precede the cache ctr");
        assert_eq!(observe(&ops, &s.volume).unwrap(), StackState::Complete);
    }

    #[test]
    fn a_warm_resume_adopts_the_cache_metadata_untouched() {
        // The counterpart rule: metadata persistence is load-bearing
        // for reboot warmth, so a resume (cold_read_cache unset) must
        // never write the carve.
        let ops = MockDmOps::empty();
        let s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        build(&ops, &s).unwrap();
        assert!(
            !ops.events()
                .iter()
                .any(|e| matches!(e, MockEvent::WriteDev(..))),
            "warm builds must not touch cache metadata"
        );
    }

    #[test]
    fn a_cold_build_over_a_live_trio_replaces_it() {
        // The pool-file-lost path can find the trio still activated
        // (deleted-but-open backing keeps a stack alive). Nothing can
        // be zeroed under a live cache, so the cold build takes the
        // trio down — consumers first, the mock's EBUSY contract
        // enforcing the order — and rebuilds it fresh.
        let ops = MockDmOps::empty();
        let mut s = spec_with_read_cache();
        build(&ops, &s).unwrap();
        s.cold_read_cache = true;
        build(&ops, &s).unwrap();
        let names = s.names();
        let ev = ops.events();
        assert!(
            ev.iter()
                .any(|e| *e == MockEvent::Remove(names.rcache.clone())),
            "the live cache must come down before the cold rebuild"
        );
        assert!(ops.table(&names.rcache).is_some(), "and be rebuilt");
        assert_eq!(observe(&ops, &s.volume).unwrap(), StackState::Complete);
    }

    #[test]
    fn a_spec_without_the_cache_removes_a_surviving_trio() {
        // The Some→None restage (config rollback): a surviving rcache
        // still names the shim, so leaving it up keeps the shim
        // dm-claimed and the mount answers EBUSY — with nothing on the
        // idempotent-resume path ever clearing it. The cache-less
        // build must remove the trio, consumers first.
        let ops = MockDmOps::empty();
        let cached = spec_with_read_cache();
        build(&ops, &cached).unwrap();
        let mut s = spec();
        s.external_origin = cached.external_origin.clone();
        build(&ops, &s).unwrap();
        let names = s.names();
        for d in names.read_cache_order() {
            assert!(
                ops.table(d).is_none(),
                "{d} must not survive a cache-less build"
            );
        }
        assert_eq!(observe(&ops, &s.volume).unwrap(), StackState::Complete);
        assert_eq!(
            s.mount_path(),
            s.shim_path(),
            "the mount device is the shim again"
        );
    }
}

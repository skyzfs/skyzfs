//! [`MockDmOps`]: an in-memory dm-thin pool that *enforces the kernel's
//! contracts* — the M9 `DmOps` seam's rootless half.
//!
//! Each rule it enforces is one the real kernel enforced first, at the
//! cost of a failed nightly (tests/README.md lessons table):
//!
//! - `create_snap` of an **active, unsuspended** origin → `EINVAL`
//!   (nightly round 2).
//! - `read_dev` of a device that was never activated → error: raw DM
//!   ioctls create no `/dev/mapper` nodes, so "it exists in the pool" is
//!   not "it is readable" (round 3).
//! - `thin_mappings` without a reserved metadata snapshot → error, and
//!   reads see the **reserved point-in-time tree**, not the live one
//!   (matches `thinp::read_superblock_snap`; round 4 adjacency).
//! - `create_snap` of an existing dev id → `EEXIST` (drives the sidecar
//!   retry); double freeze / double reserve → `EBUSY`; `delete` of a thin
//!   with an active dm device → `EBUSY`; rename onto an existing name →
//!   `EEXIST`.
//!
//! Data is modeled with real COW at pool-block granularity, so
//! `DmStack<MockDmOps>` produces bit-exact snapshots and true deltas —
//! good enough to drive the full `CycleEngine` in plain `cargo test`.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::dm::{DmDeviceInfo, DmOps};
use crate::stack::StackError;

/// What an unmasked queue reports for `queue/discard_max_bytes`: any
/// nonzero limit at all is "this device passes discards through".
const KERNEL_DISCARD_MAX_DEFAULT: u64 = 1 << 30;

/// Every kernel-visible action, in order — tests assert sequences.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockEvent {
    Freeze,
    Thaw,
    ForceThaw,
    /// §8.1 rule 3's actuator: `true` = the superblock went read-only.
    Remount(bool),
    Suspend(String),
    Resume(String),
    CreateSnap(u64),
    DeleteThin(u64),
    ReserveMetaSnap,
    ReleaseMetaSnap,
    ActivateRo(String),
    Create(String),
    Reload(String),
    /// A retarget that suppressed dm's implicit `lock_fs` because the
    /// caller already owns the freeze. Recorded SEPARATELY from `Reload`
    /// so a test can tell the two apart: the kernel effect is invisible
    /// to a mock, but the call shape is not, and the call shape is what
    /// a refactor breaks.
    ReloadInFrozenWindow(String),
    Remove(String),
    Rename(String, String),
    Discard(String, u64, u64),
    SetDiscardMax(String, u64),
    WriteDev(String, u64, u64),
    PrepareBacking(String),
    Preallocate(String),
    ReleaseBacking(String),
    DeleteBacking(String),
    RenameBacking(String, String),
}

/// One entry in the mock's backing-file registry (the loop/file half of
/// the swing's slot machinery).
#[derive(Debug, Clone)]
struct MockBacking {
    size: u64,
    prealloc: bool,
    loop_dev: Option<String>,
    /// Allocated bytes (`st_blocks × 512`) when a test pins them —
    /// §8.1 rule 2's partially-allocated case, which neither
    /// `prepare_backing` nor `ensure_preallocated` can produce. `None`
    /// derives it from `prealloc`: all of `size`, or nothing.
    allocated: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct MockThin {
    /// thin block → data block.
    mappings: BTreeMap<u64, u64>,
}

#[derive(Debug, Clone, Default)]
struct MockDev {
    uuid: Option<String>,
    minor: u32,
    suspended: bool,
    /// `(pool, thin_dev_id)` for thin targets; `None` for the pool itself.
    thin: Option<(String, u64)>,
    /// Table lines for devices built via `create_device` (provisioning
    /// seam); empty for the fixture pool/origin and `create_ro_thin`.
    table: Vec<(u64, u64, String, String)>,
    /// External-origin backing (`thin <pool> <id> <origin>` semantics,
    /// D-M11/D-M15): unprovisioned blocks of THIS device read through to
    /// this image instead of zeros. Set via `set_external_origin`.
    backing: Option<Vec<u8>>,
}

struct Inner {
    pool_name: String,
    size_bytes: u64,
    chunk: u64,
    devices: BTreeMap<String, MockDev>,
    thins: BTreeMap<u64, MockThin>,
    data: HashMap<u64, Vec<u8>>,
    next_data: u64,
    next_minor: u32,
    tx_id: u64,
    meta_snap: Option<BTreeMap<u64, MockThin>>,
    /// Thin-pools created through `create_device` (the provisioning and
    /// swing seams): pool dm-name → its thins. These accept only
    /// `create_thin`/`delete` messages — the fixture pool keeps the full
    /// snapshot-cycle surface — but their thins carry real mappings, so
    /// devices over them read/write/discard like the fixture's.
    extra_pools: BTreeMap<String, BTreeMap<u64, MockThin>>,
    /// Backing-file registry: path → (size, prealloc, attached loop).
    backings: BTreeMap<PathBuf, MockBacking>,
    next_loop: u32,
    /// What `fs_free_bytes` answers (settable; default unbounded).
    fs_free: u64,
    /// Content served at arbitrary device paths — external-origin
    /// backings resolved when a `thin … <origin>` table names the path.
    path_images: HashMap<String, Vec<u8>>,
    frozen: bool,
    /// The modeled superblock's read-only flag (§8.1 rule 3). A CLEAN
    /// read-only super, which freezes successfully — see `freeze_fs`.
    read_only: bool,
    /// The modeled superblock has been ABORTED (ext4 `emergency_ro`, an
    /// XFS shutdown): every write fails, `FIFREEZE` answers `EROFS`,
    /// and `/proc/mounts` still says `rw`. Distinct from `read_only` on
    /// purpose — the kernel distinguishes them and so must this.
    aborted: bool,
    /// Injected `freeze_fs` failure, taken once — the errno half of
    /// §8.2 step 3, so a test can drive `Wedged` without an actual
    /// shut-down filesystem underneath.
    fail_next_freeze: Option<StackError>,
    events: Vec<MockEvent>,
    /// `queue/discard_max_bytes` per device; absent = the kernel's own
    /// (unbounded) default. `Some(0)` is Invariant B's guest-discard
    /// mask, and the mock enforces what it means: `discard_dev` on such
    /// a queue answers `EOPNOTSUPP`, exactly as the block layer does.
    discard_max: BTreeMap<String, u64>,
    /// Devices this (modeled) kernel has no `queue/discard_max_bytes`
    /// for — ENOENT on both the write and the readback, the tolerated
    /// arm of Invariant B's mask (§8.1 / task 15j).
    discard_knob_absent: BTreeSet<String>,
    /// Devices whose queue ACCEPTS the write and drops it: the mask
    /// reads back unmasked, which is exactly the shim that will pass a
    /// guest `fstrim` through while its write said `Ok`.
    discard_knob_ignored: BTreeSet<String>,
    /// Devices whose knob VANISHES between the write and the readback:
    /// the write lands, then the name resolves to a device that has no
    /// such file. Models the reload race — a different device wearing
    /// the same dm name one syscall later.
    discard_knob_vanishing: BTreeSet<String>,
    fail_next_suspend: bool,
    /// Fail the next `thaw_fs`, leaving the filesystem frozen.
    fail_next_thaw: bool,
    /// Fail the next `discard_dev` on this device — the crash-between-
    /// the-dual-discards injection (§7.1 B3 ordering).
    fail_next_discard: Option<String>,
    create_snap_delay: Option<Duration>,
    freeze_delay: Option<Duration>,
    reload_delay: Option<Duration>,
}

#[derive(Clone)]
pub struct MockDmOps {
    inner: Arc<Mutex<Inner>>,
}

impl MockDmOps {
    /// A pool named `pool_name` with one active thin origin `origin_name`
    /// (thin dev `origin_dev_id`), fully unprovisioned.
    pub fn new(
        pool_name: &str,
        origin_name: &str,
        origin_dev_id: u64,
        size_bytes: u64,
        chunk_bytes: u64,
    ) -> Self {
        assert!(size_bytes.is_multiple_of(chunk_bytes));
        let mut devices = BTreeMap::new();
        devices.insert(
            pool_name.to_string(),
            MockDev {
                uuid: None,
                minor: 0,
                suspended: false,
                thin: None,
                table: Vec::new(),
                backing: None,
            },
        );
        devices.insert(
            origin_name.to_string(),
            MockDev {
                uuid: None,
                minor: 1,
                suspended: false,
                thin: Some((pool_name.to_string(), origin_dev_id)),
                table: Vec::new(),
                backing: None,
            },
        );
        let mut thins = BTreeMap::new();
        thins.insert(origin_dev_id, MockThin::default());
        Self {
            inner: Arc::new(Mutex::new(Inner {
                pool_name: pool_name.to_string(),
                size_bytes,
                chunk: chunk_bytes,
                devices,
                thins,
                data: HashMap::new(),
                next_data: 100,
                next_minor: 2,
                tx_id: 1,
                meta_snap: None,
                extra_pools: BTreeMap::new(),
                backings: BTreeMap::new(),
                next_loop: 0,
                fs_free: u64::MAX,
                path_images: HashMap::new(),
                frozen: false,
                read_only: false,
                aborted: false,
                fail_next_freeze: None,
                events: Vec::new(),
                discard_max: BTreeMap::new(),
                discard_knob_absent: BTreeSet::new(),
                discard_knob_ignored: BTreeSet::new(),
                discard_knob_vanishing: BTreeSet::new(),
                fail_next_suspend: false,
                fail_next_thaw: false,
                fail_next_discard: None,
                create_snap_delay: None,
                freeze_delay: None,
                reload_delay: None,
            })),
        }
    }

    /// An empty device table: no fixture pool, no origin. The shape a
    /// provisioner starts from.
    pub fn empty() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                pool_name: String::new(),
                size_bytes: 0,
                chunk: 4096,
                devices: BTreeMap::new(),
                thins: BTreeMap::new(),
                data: HashMap::new(),
                next_data: 100,
                next_minor: 0,
                tx_id: 1,
                meta_snap: None,
                extra_pools: BTreeMap::new(),
                backings: BTreeMap::new(),
                next_loop: 0,
                fs_free: u64::MAX,
                path_images: HashMap::new(),
                frozen: false,
                read_only: false,
                aborted: false,
                fail_next_freeze: None,
                events: Vec::new(),
                discard_max: BTreeMap::new(),
                discard_knob_absent: BTreeSet::new(),
                discard_knob_ignored: BTreeSet::new(),
                discard_knob_vanishing: BTreeSet::new(),
                fail_next_suspend: false,
                fail_next_thaw: false,
                fail_next_discard: None,
                create_snap_delay: None,
                freeze_delay: None,
                reload_delay: None,
            })),
        }
    }

    // ---- test-facing controls ------------------------------------------

    /// Write to the origin with real COW: a data block shared with any
    /// snapshot is copied before modification.
    pub fn write_origin(&self, origin_name: &str, off: u64, data: &[u8]) {
        let mut g = self.inner.lock().unwrap();
        assert!(!g.frozen, "test wrote to a frozen filesystem");
        let (pool, origin_id) = g.devices[origin_name].thin.clone().expect("origin is thin");
        g.write_thin(&pool, origin_id, off, data);
    }

    /// Discard (unmap) chunk-aligned ranges on the origin.
    pub fn discard_origin(&self, origin_name: &str, off: u64, len: u64) {
        let mut g = self.inner.lock().unwrap();
        let chunk = g.chunk;
        assert!(
            off.is_multiple_of(chunk) && len.is_multiple_of(chunk),
            "chunk-aligned only"
        );
        let (pool, origin_id) = g.devices[origin_name].thin.clone().expect("origin is thin");
        for b in (off / chunk)..((off + len) / chunk) {
            g.thin_mut(&pool, origin_id).unwrap().mappings.remove(&b);
        }
        g.gc_data();
    }

    /// Materialize the origin's full image (unmapped ⇒ the external
    /// origin backing when set, else zeros).
    pub fn origin_image(&self, origin_name: &str) -> Vec<u8> {
        let g = self.inner.lock().unwrap();
        let d = &g.devices[origin_name];
        let (pool, id) = d.thin.clone().expect("origin is thin");
        g.image_of(&pool, id, d.backing.as_deref())
    }

    /// Give a thin device an external-origin backing (the
    /// `thin <pool> <id> <origin>` read-through, D-M11/D-M15): its
    /// unprovisioned blocks read from `backing` instead of zeros. The
    /// D-M15 repoint is modeled by calling this again with the new
    /// generation's image.
    pub fn set_external_origin(&self, name: &str, backing: Vec<u8>) {
        let mut g = self.inner.lock().unwrap();
        assert_eq!(backing.len() as u64, g.size_bytes, "backing size mismatch");
        let d = g.devices.get_mut(name).expect("no such device");
        assert!(d.thin.is_some(), "external origin on a non-thin device");
        d.backing = Some(backing);
    }

    /// Thin blocks currently mapped for a device (reclaim assertions).
    pub fn mapped_blocks(&self, name: &str) -> Vec<u64> {
        let g = self.inner.lock().unwrap();
        let (pool, id) = g.devices[name].thin.clone().expect("thin device");
        g.thin_ref(&pool, id)
            .expect("thin exists")
            .mappings
            .keys()
            .copied()
            .collect()
    }

    /// Pre-create a bare thin dev id (simulates a stale sidecar: ids the
    /// pool already knows that the agent's file does not).
    pub fn seed_thin(&self, dev_id: u64) {
        let mut g = self.inner.lock().unwrap();
        assert!(g.thins.insert(dev_id, MockThin::default()).is_none());
    }

    pub fn fail_next_suspend(&self) {
        self.inner.lock().unwrap().fail_next_suspend = true;
    }

    /// Fail the next `thaw_fs` the way a real one fails: the filesystem
    /// stays FROZEN. `FITHAW`'s EINVAL ("was not frozen") is `Ok` on
    /// every implementation, so what this models is the other errno —
    /// EBUSY/EIO — and the mount that is still frozen behind it.
    pub fn fail_next_thaw(&self) {
        self.inner.lock().unwrap().fail_next_thaw = true;
    }

    /// Fail the next `discard_dev` aimed at `name` — the injection that
    /// lets a test die BETWEEN the §7.1 eviction's two discards, where
    /// only the ordering keeps the surviving asymmetry benign.
    pub fn fail_next_discard_on(&self, name: &str) {
        self.inner.lock().unwrap().fail_next_discard = Some(name.to_string());
    }

    /// `queue/discard_max_bytes` as last set on `name`; `None` = never
    /// set (Invariant B's mask is `Some(0)` on the shim). The inherent
    /// accessor deliberately keeps the `Option` shape — it answers
    /// "was this device ever masked", which the [`DmOps`] readback (a
    /// kernel-shaped `Result<u64>`) cannot.
    pub fn discard_max_set(&self, name: &str) -> Option<u64> {
        self.inner.lock().unwrap().discard_max.get(name).copied()
    }

    /// Model a kernel with no `queue/discard_max_bytes` for `name`:
    /// both the write and the readback answer ENOENT
    /// ([`StackError::Unsupported`]) — the arm Invariant B's mask
    /// tolerates.
    pub fn set_discard_knob_absent(&self, name: &str) {
        self.inner
            .lock()
            .unwrap()
            .discard_knob_absent
            .insert(name.to_string());
    }

    /// Model the queue that accepts the write and drops it: the mask
    /// reads back at the kernel default, so the shim still passes guest
    /// discards. The hazard the readback exists to catch.
    pub fn set_discard_knob_ignored(&self, name: &str) {
        self.inner
            .lock()
            .unwrap()
            .discard_knob_ignored
            .insert(name.to_string());
    }

    /// Model the reload race: the write lands on a knob that exists,
    /// and by the readback the name resolves to a device that has none.
    /// A kernel that never had the knob refuses the WRITE; only a
    /// swapped device can answer the write and not the read.
    pub fn set_discard_knob_vanishing(&self, name: &str) {
        self.inner
            .lock()
            .unwrap()
            .discard_knob_vanishing
            .insert(name.to_string());
    }

    /// Pin a backing file's allocated bytes (`st_blocks × 512`) —
    /// §8.1 rule 2's partially-allocated case. Beyond `size` is
    /// meaningless; the surface only asks whether allocation covers the
    /// logical length.
    pub fn set_backing_allocated(&self, file: &Path, bytes: u64) {
        if let Some(b) = self.inner.lock().unwrap().backings.get_mut(file) {
            b.allocated = Some(bytes);
        }
    }

    /// Stall `create_snap` (without holding the mock's lock) so the freeze
    /// watchdog can fire mid-capture.
    pub fn delay_create_snap(&self, d: Duration) {
        self.inner.lock().unwrap().create_snap_delay = Some(d);
    }

    /// Table lines of a device built via `create_device` (tests assert
    /// target parameters, e.g. the thin external origin — D-M11).
    pub fn table(&self, name: &str) -> Option<Vec<(u64, u64, String, String)>> {
        self.inner
            .lock()
            .unwrap()
            .devices
            .get(name)
            .map(|d| d.table.clone())
    }

    pub fn events(&self) -> Vec<MockEvent> {
        self.inner.lock().unwrap().events.clone()
    }

    pub fn frozen(&self) -> bool {
        self.inner.lock().unwrap().frozen
    }

    pub fn meta_snap_reserved(&self) -> bool {
        self.inner.lock().unwrap().meta_snap.is_some()
    }

    /// Thin dev ids currently in the pool.
    pub fn thin_ids(&self) -> Vec<u64> {
        self.inner.lock().unwrap().thins.keys().copied().collect()
    }

    /// Thin dev ids of any pool by name — the fixture pool or one
    /// stood up through `create_device` (a swing's slot B, once
    /// canonicalization has renamed it into place).
    pub fn thin_ids_of(&self, pool: &str) -> Vec<u64> {
        let g = self.inner.lock().unwrap();
        match g.extra_pools.get(pool) {
            Some(ids) => ids.keys().copied().collect(),
            None if g.pool_name == pool => g.thins.keys().copied().collect(),
            None => Vec::new(),
        }
    }

    /// Every pool's thin dev ids, pool name first — the fixture pool and
    /// any pool stood up through `create_device`. [`Self::thin_ids`]
    /// sees only the fixture map, so a leaked slot-B pool (a recovery
    /// that never swept it) is invisible there while its thins go on
    /// holding data blocks; leak assertions must count across pools.
    pub fn all_thin_ids(&self) -> Vec<(String, Vec<u64>)> {
        let g = self.inner.lock().unwrap();
        std::iter::once((g.pool_name.clone(), g.thins.keys().copied().collect()))
            .filter(|(n, _): &(String, Vec<u64>)| !n.is_empty())
            .chain(
                g.extra_pools
                    .iter()
                    .map(|(n, t)| (n.clone(), t.keys().copied().collect())),
            )
            .collect()
    }

    /// dm device names currently in the table.
    pub fn device_names(&self) -> Vec<String> {
        self.inner.lock().unwrap().devices.keys().cloned().collect()
    }

    /// What `fs_free_bytes` reports (headroom-guard scenarios).
    pub fn set_fs_free(&self, bytes: u64) {
        self.inner.lock().unwrap().fs_free = bytes;
    }

    /// Serve `image` at an arbitrary device path: a `thin … <origin>`
    /// table naming the path binds it as that thin's external-origin
    /// backing (the swing's B thin reads through the same repointable
    /// attach as A's).
    pub fn set_path_image(&self, path: &str, image: Vec<u8>) {
        self.inner
            .lock()
            .unwrap()
            .path_images
            .insert(path.to_string(), image);
    }

    /// Backing-registry entry for `file`: `(size, preallocated, loop)`.
    pub fn backing_info(&self, file: &Path) -> Option<(u64, bool, Option<String>)> {
        self.inner
            .lock()
            .unwrap()
            .backings
            .get(file)
            .map(|b| (b.size, b.prealloc, b.loop_dev.clone()))
    }

    /// Live data blocks in the (shared) data space — pool-usage
    /// assertions for eviction's last-ref drops.
    pub fn data_block_count(&self) -> usize {
        self.inner.lock().unwrap().data.len()
    }

    /// Stall the next `freeze_fs` AFTER the freeze lands (lock released)
    /// so the watchdog can fire inside a freeze window.
    pub fn delay_next_freeze(&self, d: Duration) {
        self.inner.lock().unwrap().freeze_delay = Some(d);
    }

    /// Stall the next `reload_device` — the swing's retarget — so the
    /// freeze watchdog fires while the shim is being aimed at slot B.
    pub fn delay_next_reload(&self, d: Duration) {
        self.inner.lock().unwrap().reload_delay = Some(d);
    }

    /// Fail the next `freeze_fs` with `e`. Unlike `fail_next_thaw` this
    /// takes the error, because for a freeze the *classification* is
    /// the behaviour under test: `Wedged` is the one §8.2 step 3
    /// proceeds through, and every other errno must still abort the
    /// capture.
    pub fn fail_next_freeze(&self, e: StackError) {
        self.inner.lock().unwrap().fail_next_freeze = Some(e);
    }

    /// Whether the modeled superblock is currently read-only.
    pub fn read_only(&self) -> bool {
        self.inner.lock().unwrap().read_only
    }

    /// Abort the modeled filesystem — the ext4 `emergency_ro` / XFS
    /// shutdown state that actually arms §8.2 step 3. Deliberately not
    /// the same knob as `remount_ro`.
    pub fn abort_fs(&self) {
        self.inner.lock().unwrap().aborted = true;
    }
}

impl Inner {
    fn push(&mut self, e: MockEvent) {
        self.events.push(e);
    }

    /// A thin's mappings, in a provisioned pool or the fixture one.
    /// Provisioned pools win a name tie: a swing's canonicalize rename
    /// hands the fixture pool's NAME to the B pool after the fixture
    /// pool device is gone, and lookups must follow the live pool.
    fn thin_ref(&self, pool: &str, id: u64) -> Option<&MockThin> {
        if let Some(p) = self.extra_pools.get(pool) {
            return p.get(&id);
        }
        if pool == self.pool_name {
            return self.thins.get(&id);
        }
        None
    }

    fn thin_mut(&mut self, pool: &str, id: u64) -> Option<&mut MockThin> {
        if let Some(p) = self.extra_pools.get_mut(pool) {
            return p.get_mut(&id);
        }
        if pool == self.pool_name {
            return self.thins.get_mut(&id);
        }
        None
    }

    /// Data blocks a pool's own thins reference — dm-thin's used count,
    /// with shared blocks counted once. Per-pool, not global: during a
    /// swing two pools are live at once, and a status line that added
    /// them together would report slot A's occupancy as slot B's (which
    /// is what made "the pool shrank" unassertable, and post-swing
    /// watermark pressure unreachable, in every mock-backed test).
    fn pool_used_blocks(&self, pool: &str) -> u64 {
        let thins = match self.extra_pools.get(pool) {
            Some(p) => p,
            None if pool == self.pool_name => &self.thins,
            None => return 0,
        };
        thins
            .values()
            .flat_map(|t| t.mappings.values().copied())
            .collect::<std::collections::BTreeSet<u64>>()
            .len() as u64
    }

    /// References to `db` across every pool's thins and the reserved
    /// tree. Data-block ids are allocated globally, so cross-pool
    /// counting can never alias.
    fn refs_of(&self, db: u64) -> usize {
        let live = self
            .thins
            .values()
            .chain(self.extra_pools.values().flat_map(|p| p.values()))
            .flat_map(|t| t.mappings.values())
            .filter(|&&d| d == db)
            .count();
        let snap = self
            .meta_snap
            .iter()
            .flat_map(|m| m.values())
            .flat_map(|t| t.mappings.values())
            .filter(|&&d| d == db)
            .count();
        live + snap
    }

    /// Drop data blocks nothing references anymore — dm-thin returns
    /// unmapped pool blocks to the free space, so used-space accounting
    /// must see discards and deletes as real frees.
    fn gc_data(&mut self) {
        let dead: Vec<u64> = self
            .data
            .keys()
            .copied()
            .filter(|&db| self.refs_of(db) == 0)
            .collect();
        for db in dead {
            self.data.remove(&db);
        }
    }

    /// Ensure `block` of thin `id` in `pool` is exclusively mapped;
    /// return its data block (allocating / copying as needed).
    fn cow_block(&mut self, pool: &str, id: u64, block: u64) -> u64 {
        let chunk = self.chunk as usize;
        match self
            .thin_ref(pool, id)
            .expect("cow of unknown thin")
            .mappings
            .get(&block)
            .copied()
        {
            None => {
                let db = self.next_data;
                self.next_data += 1;
                self.data.insert(db, vec![0u8; chunk]);
                self.thin_mut(pool, id).unwrap().mappings.insert(block, db);
                db
            }
            Some(db) if self.refs_of(db) > 1 => {
                let copy = self.data[&db].clone();
                let nb = self.next_data;
                self.next_data += 1;
                self.data.insert(nb, copy);
                self.thin_mut(pool, id).unwrap().mappings.insert(block, nb);
                nb
            }
            Some(db) => db,
        }
    }

    /// COW-write `data` at `off` on thin `id` of `pool`.
    fn write_thin(&mut self, pool: &str, id: u64, off: u64, data: &[u8]) {
        let chunk = self.chunk;
        let mut pos = 0usize;
        while pos < data.len() {
            let abs = off + pos as u64;
            let block = abs / chunk;
            let in_block = (abs % chunk) as usize;
            let n = ((chunk as usize) - in_block).min(data.len() - pos);
            let db = self.cow_block(pool, id, block);
            self.data.get_mut(&db).unwrap()[in_block..in_block + n]
                .copy_from_slice(&data[pos..pos + n]);
            pos += n;
        }
        self.gc_data(); // CoW may have orphaned a block's last ref
    }

    fn image_of(&self, pool: &str, id: u64, backing: Option<&[u8]>) -> Vec<u8> {
        let mut out = match backing {
            Some(b) => b.to_vec(),
            None => vec![0u8; self.size_bytes as usize],
        };
        let chunk = self.chunk as usize;
        for (&b, &db) in &self
            .thin_ref(pool, id)
            .expect("image of unknown thin")
            .mappings
        {
            let s = b as usize * chunk;
            out[s..s + chunk].copy_from_slice(&self.data[&db]);
        }
        out
    }

    fn check_pool(&self, pool: &str) -> Result<(), StackError> {
        if pool != self.pool_name {
            return Err(StackError::NotFound(format!("no pool {pool}")));
        }
        Ok(())
    }

    /// Remap `name` through single-target zero-offset dm-linear tables,
    /// as the kernel remaps bios. `Ok(name)` = a thin dm device;
    /// `Err(component)` = the chain left the dm table (a loop node or
    /// raw path — real IO would hit that bdev directly).
    fn resolve_dev(&self, name: &str) -> Result<Result<String, String>, StackError> {
        let mut target = name.to_string();
        loop {
            let Some(d) = self.devices.get(&target) else {
                return Ok(Err(target));
            };
            if d.thin.is_some() {
                return Ok(Ok(target));
            }
            match d.table.as_slice() {
                [(0, _, ttype, params)] if ttype == "linear" => {
                    let mut words = params.split_whitespace();
                    let dev = words.next().unwrap_or_default();
                    if words.next() != Some("0") {
                        return Err(nf(format!(
                            "resolve {target}: non-zero linear offset unmodeled"
                        )));
                    }
                    target = match dev.strip_prefix("/dev/mapper/") {
                        Some(n) => n.to_string(),
                        None => {
                            // 252 is the mock's own fixed major (what its
                            // DmDeviceInfo reports), not a claim about real
                            // systems — real dm majors vary (commonly 253+).
                            let mm = dev.split_once(':').and_then(|(a, b)| {
                                Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?))
                            });
                            let Some((252, minor)) = mm else {
                                return Ok(Err(dev.to_string()));
                            };
                            self.devices
                                .iter()
                                .find(|(_, d)| d.minor == minor)
                                .map(|(n, _)| n.clone())
                                .ok_or_else(|| {
                                    nf(format!("resolve {target}: {dev} does not resolve"))
                                })?
                        }
                    };
                }
                _ => return Err(nf(format!("resolve {target}: not a thin or linear device"))),
            }
        }
    }
}

fn nf(msg: String) -> StackError {
    StackError::NotFound(msg)
}

impl DmOps for MockDmOps {
    fn list_devices(&self) -> Result<Vec<DmDeviceInfo>, StackError> {
        let g = self.inner.lock().unwrap();
        Ok(g.devices
            .iter()
            .map(|(name, d)| DmDeviceInfo {
                name: name.clone(),
                uuid: d.uuid.clone(),
                major: 252,
                minor: d.minor,
            })
            .collect())
    }

    fn device_info(&self, name: &str) -> Result<DmDeviceInfo, StackError> {
        let g = self.inner.lock().unwrap();
        let d = g
            .devices
            .get(name)
            .ok_or_else(|| nf(format!("info {name}: no such device")))?;
        Ok(DmDeviceInfo {
            name: name.to_string(),
            uuid: d.uuid.clone(),
            major: 252,
            minor: d.minor,
        })
    }

    fn pool_message(&self, pool: &str, msg: &str) -> Result<(), StackError> {
        // create_snap may be configured to stall; sleep WITHOUT the lock so
        // the watchdog's force-thaw can run (as in the kernel, where the
        // pool message and the thaw ioctl do not serialize on one lock).
        let delay = {
            let g = self.inner.lock().unwrap();
            if msg.starts_with("create_snap") {
                g.create_snap_delay
            } else {
                None
            }
        };
        if let Some(d) = delay {
            std::thread::sleep(d);
        }

        let mut g = self.inner.lock().unwrap();
        // Provisioned pools (created via `create_device` with a thin-pool
        // table) speak only the provisioning subset.
        if g.extra_pools.contains_key(pool) {
            let parts: Vec<&str> = msg.split_whitespace().collect();
            return match parts.as_slice() {
                ["create_thin", id] => {
                    let id: u64 = id.parse().map_err(|_| nf(format!("bad id in {msg:?}")))?;
                    let ids = g.extra_pools.get_mut(pool).unwrap();
                    if ids.contains_key(&id) {
                        return Err(nf(format!("EEXIST: thin dev {id} in {pool}")));
                    }
                    ids.insert(id, MockThin::default());
                    Ok(())
                }
                ["delete", id] => {
                    let id: u64 = id.parse().map_err(|_| nf(format!("bad id in {msg:?}")))?;
                    if g.devices
                        .values()
                        .any(|d| matches!(&d.thin, Some((p, tid)) if p == pool && *tid == id))
                    {
                        return Err(nf(format!("EBUSY: thin dev {id} has an active device")));
                    }
                    if g.extra_pools.get_mut(pool).unwrap().remove(&id).is_none() {
                        return Err(nf(format!("delete: no thin dev {id} in {pool}")));
                    }
                    g.gc_data();
                    Ok(())
                }
                _ => Err(nf(format!("unsupported message {msg:?} for pool {pool}"))),
            };
        }
        g.check_pool(pool)?;
        let parts: Vec<&str> = msg.split_whitespace().collect();
        match parts.as_slice() {
            ["create_snap", id, origin] => {
                let id: u64 = id.parse().map_err(|_| nf(format!("bad id in {msg:?}")))?;
                let origin: u64 = origin
                    .parse()
                    .map_err(|_| nf(format!("bad origin in {msg:?}")))?;
                if !g.thins.contains_key(&origin) {
                    return Err(nf(format!("create_snap: no thin dev {origin}")));
                }
                // ROUND 2: an ACTIVE origin must be suspended first —
                // newer kernels reject the wrong order with EINVAL.
                if let Some((dn, dev)) = g
                    .devices
                    .iter()
                    .find(|(_, d)| matches!(&d.thin, Some((_, tid)) if *tid == origin))
                {
                    if !dev.suspended {
                        return Err(nf(format!(
                            "EINVAL: create_snap of active unsuspended origin {dn} \
                             (suspend it first — nightly round 2)"
                        )));
                    }
                }
                if g.thins.contains_key(&id) {
                    return Err(nf(format!("EEXIST: thin dev {id} exists")));
                }
                let snap = g.thins[&origin].clone();
                g.thins.insert(id, snap);
                g.tx_id += 1;
                g.push(MockEvent::CreateSnap(id));
                Ok(())
            }
            ["delete", id] => {
                let id: u64 = id.parse().map_err(|_| nf(format!("bad id in {msg:?}")))?;
                if !g.thins.contains_key(&id) {
                    return Err(nf(format!("delete: no thin dev {id}")));
                }
                if g.devices
                    .values()
                    .any(|d| matches!(&d.thin, Some((p, tid)) if *p == g.pool_name && *tid == id))
                {
                    return Err(nf(format!("EBUSY: thin dev {id} has an active device")));
                }
                g.thins.remove(&id);
                g.gc_data();
                g.tx_id += 1;
                g.push(MockEvent::DeleteThin(id));
                Ok(())
            }
            ["reserve_metadata_snap"] => {
                if g.meta_snap.is_some() {
                    return Err(nf("EBUSY: metadata snap already reserved".into()));
                }
                g.meta_snap = Some(g.thins.clone());
                g.push(MockEvent::ReserveMetaSnap);
                Ok(())
            }
            ["release_metadata_snap"] => {
                if g.meta_snap.is_none() {
                    return Err(nf("EINVAL: no metadata snap reserved".into()));
                }
                g.meta_snap = None;
                g.push(MockEvent::ReleaseMetaSnap);
                Ok(())
            }
            _ => Err(nf(format!("unknown pool message {msg:?}"))),
        }
    }

    fn pool_status_params(&self, pool: &str) -> Result<String, StackError> {
        let g = self.inner.lock().unwrap();
        // Provisioned devices report status per their table's target: a
        // cache device answers in dm-cache status shape (dirty count at
        // token 10 — the decommission flush polls it, always clean in the
        // mock), a provisioned thin-pool in thin-pool shape.
        if let Some(d) = g.devices.get(pool).filter(|d| !d.table.is_empty()) {
            return match d.table[0].2.as_str() {
                "cache" => Ok("8 5/1000 128 10/2000 0 0 0 0 0 0 0 1 writeback 2 \
                     migration_threshold 2048 cleaner 0 rw -"
                    .into()),
                // Data usage from the pool's own mappings, capacity from
                // its data target's length — a provisioned pool is a
                // real pool, and the cycle after a swing reads occupancy
                // out of exactly this line.
                "thin-pool" => Ok(format!(
                    "{} 10/1000 {}/{} - rw no_discard_passdown",
                    g.tx_id,
                    g.pool_used_blocks(pool),
                    d.table[0].1 * 512 / g.chunk
                )),
                other => Err(nf(format!("no status model for target {other:?}"))),
            };
        }
        g.check_pool(pool)?;
        // Same accounting for the fixture pool, so occupancy is
        // comparable either side of a swing's promotion — the §7 claim
        // that which executor ran is not observable has to hold for the
        // status line too.
        Ok(format!(
            "{} 10/1000 {}/{} - rw discard_passdown queue_if_no_space - 1024",
            g.tx_id,
            g.pool_used_blocks(pool),
            g.size_bytes / g.chunk
        ))
    }

    fn suspend_device(&self, name: &str) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.fail_next_suspend {
            g.fail_next_suspend = false;
            return Err(StackError::Fault("injected suspend failure"));
        }
        let d = g
            .devices
            .get_mut(name)
            .ok_or_else(|| nf(format!("suspend {name}: no such device")))?;
        d.suspended = true;
        g.push(MockEvent::Suspend(name.to_string()));
        Ok(())
    }

    fn resume_device(&self, name: &str) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        let d = g
            .devices
            .get_mut(name)
            .ok_or_else(|| nf(format!("resume {name}: no such device")))?;
        d.suspended = false;
        g.push(MockEvent::Resume(name.to_string()));
        Ok(())
    }

    fn create_ro_thin(
        &self,
        name: &str,
        uuid: &str,
        pool: &str,
        sectors: u64,
        thin_dev_id: u64,
    ) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        g.check_pool(pool)?;
        if g.devices.contains_key(name) {
            return Err(nf(format!("EEXIST: device {name}")));
        }
        if !g.thins.contains_key(&thin_dev_id) {
            return Err(nf(format!("no thin dev {thin_dev_id} in pool")));
        }
        if sectors * 512 != g.size_bytes {
            return Err(nf(
                format!("table length {sectors} sectors != volume size",),
            ));
        }
        let minor = g.next_minor;
        g.next_minor += 1;
        g.devices.insert(
            name.to_string(),
            MockDev {
                uuid: Some(uuid.to_string()),
                minor,
                suspended: false,
                thin: Some((pool.to_string(), thin_dev_id)),
                table: Vec::new(),
                backing: None,
            },
        );
        g.push(MockEvent::ActivateRo(name.to_string()));
        Ok(())
    }

    fn create_device(
        &self,
        name: &str,
        uuid: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.devices.contains_key(name) {
            return Err(nf(format!("EEXIST: device {name}")));
        }
        if table.is_empty() {
            return Err(nf(format!("empty table for {name}")));
        }
        // The kernel resolves referenced devices at table-load time: a
        // `/dev/mapper/<name>` path that doesn't exist yet (or is
        // suspended, for targets that open their components) fails the
        // load. Enforcing it here is what makes build-order a tested
        // contract rather than a comment.
        for (_, _, ttype, params) in table {
            match ttype.as_str() {
                "linear" | "cache" | "thin-pool" | "thin" | "error" | "zero" => {}
                other => return Err(nf(format!("unknown target type {other:?} for {name}"))),
            }
            for word in params.split_whitespace() {
                if let Some(dep) = word.strip_prefix("/dev/mapper/") {
                    let d = g
                        .devices
                        .get(dep)
                        .ok_or_else(|| nf(format!("table {name}: {dep} does not exist")))?;
                    if d.suspended {
                        return Err(nf(format!("table {name}: {dep} is suspended")));
                    }
                } else if let Some((maj, min)) = word
                    .split_once(':')
                    .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)))
                {
                    // major:minor reference — must resolve to a live dm
                    // device (build order is a correctness contract).
                    let dep = g
                        .devices
                        .iter()
                        .find(|(_, d)| maj == 252 && d.minor == min)
                        .ok_or_else(|| nf(format!("table {name}: {word} does not exist")))?;
                    if dep.1.suspended {
                        return Err(nf(format!("table {name}: {word} is suspended")));
                    }
                }
            }
        }
        let minor = g.next_minor;
        g.next_minor += 1;
        if table[0].2 == "thin-pool" {
            g.extra_pools.insert(name.to_string(), Default::default());
        }
        // A `thin <pool> <dev_id> [<external_origin>]` table binds real
        // thin semantics, exactly like `create_ro_thin`: the pool (by
        // path or major:minor) must exist and already know the dev id,
        // and an external origin serves the unprovisioned blocks.
        let mut thin = None;
        let mut backing = None;
        if table[0].2 == "thin" {
            let mut words = table[0].3.split_whitespace();
            let pool_ref = words.next().unwrap_or_default();
            let dev_id: u64 = words
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| nf(format!("table {name}: bad thin dev id")))?;
            let pool = match pool_ref.strip_prefix("/dev/mapper/") {
                Some(p) => p.to_string(),
                None => {
                    let mm = pool_ref
                        .split_once(':')
                        .and_then(|(a, b)| Some((a.parse::<u32>().ok()?, b.parse::<u32>().ok()?)));
                    let Some((252, minor)) = mm else {
                        return Err(nf(format!("table {name}: bad pool ref {pool_ref}")));
                    };
                    g.devices
                        .iter()
                        .find(|(_, d)| d.minor == minor)
                        .map(|(n, _)| n.clone())
                        .ok_or_else(|| nf(format!("table {name}: {pool_ref} does not resolve")))?
                }
            };
            if g.thin_ref(&pool, dev_id).is_none() {
                return Err(nf(format!("table {name}: no thin dev {dev_id} in {pool}")));
            }
            backing = words.next().and_then(|ext| g.path_images.get(ext).cloned());
            thin = Some((pool, dev_id));
        }
        g.devices.insert(
            name.to_string(),
            MockDev {
                uuid: Some(uuid.to_string()),
                minor,
                suspended: false,
                thin,
                table: table.to_vec(),
                backing,
            },
        );
        g.push(MockEvent::Create(name.to_string()));
        Ok(())
    }

    fn reload_device(
        &self,
        name: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        // Stall WITHOUT the lock, like the create_snap delay: the
        // watchdog's force-thaw must be able to run against a retarget
        // in flight.
        let delay = self.inner.lock().unwrap().reload_delay.take();
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        let mut g = self.inner.lock().unwrap();
        if table.is_empty() {
            return Err(nf(format!("empty table for {name}")));
        }
        let d = g
            .devices
            .get_mut(name)
            .ok_or_else(|| nf(format!("reload {name}: no such device")))?;
        d.table = table.to_vec();
        g.push(MockEvent::Reload(name.to_string()));
        Ok(())
    }

    /// Identical to [`MockDmOps::reload_device`] except for the event it
    /// records — a mock cannot observe `DM_SKIP_LOCKFS`, but it can
    /// observe that the swing asked for it.
    fn reload_device_in_frozen_window(
        &self,
        name: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        let delay = self.inner.lock().unwrap().reload_delay.take();
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        let mut g = self.inner.lock().unwrap();
        if table.is_empty() {
            return Err(nf(format!("empty table for {name}")));
        }
        let d = g
            .devices
            .get_mut(name)
            .ok_or_else(|| nf(format!("reload {name}: no such device")))?;
        d.table = table.to_vec();
        g.push(MockEvent::ReloadInFrozenWindow(name.to_string()));
        Ok(())
    }

    fn remove_device(&self, name: &str) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        // In-use check, as the kernel does it: removal fails while any
        // other device's table references this one (by name or by
        // major:minor).
        let dep_path = format!("/dev/mapper/{name}");
        let dep_mm = g.devices.get(name).map(|d| format!("252:{}", d.minor));
        if let Some(user) = g.devices.iter().find(|(other, d)| {
            *other != name
                && d.table.iter().any(|(_, _, _, params)| {
                    params
                        .split_whitespace()
                        .any(|w| w == dep_path || Some(w) == dep_mm.as_deref())
                })
        }) {
            return Err(nf(format!("EBUSY: {name} is used by {}", user.0)));
        }
        if g.devices.remove(name).is_none() {
            return Err(nf(format!("remove {name}: no such device")));
        }
        g.extra_pools.remove(name);
        g.push(MockEvent::Remove(name.to_string()));
        Ok(())
    }

    fn rename_device(&self, from: &str, to: &str) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.devices.contains_key(to) {
            return Err(nf(format!("EEXIST: rename target {to}")));
        }
        let d = g
            .devices
            .remove(from)
            .ok_or_else(|| nf(format!("rename {from}: no such device")))?;
        g.devices.insert(to.to_string(), d);
        // Thin targets hold their pool by bdev reference, not by name —
        // a pool rename must not orphan them, so the mock's by-name
        // bookkeeping follows the rename.
        //
        // Promotion (§7 canonicalization): when the target IS the
        // fixture pool's name, this rename is a swing handing the
        // canonical name to slot B, and it only reaches here once pool
        // A's device is gone (the EEXIST guard above). In the kernel
        // that name now simply is the pool: A's metadata died with its
        // device, and the full message surface — create_snap,
        // reserve_metadata_snap, delete — answers on B. Fold B's thins
        // into the fixture map rather than leaving them in the
        // provisioning subset, so the cycle after a swing runs exactly
        // as the cycle before it did and §7's "which executor ran is
        // not observable from outside the node" is assertable.
        //
        // The shared data space is untouched: no block is copied, freed
        // or re-keyed here, so `data_block_count()` is identical either
        // side of the promotion. A's mappings go away with A's
        // metadata; whatever they alone held is reclaimed by the next
        // message's `gc_data`, as dm-thin frees a dead pool's blocks.
        if let Some(pool) = g.extra_pools.remove(from) {
            if to == g.pool_name {
                g.thins = pool;
            } else {
                g.extra_pools.insert(to.to_string(), pool);
            }
        }
        for dev in g.devices.values_mut() {
            if let Some((p, _)) = &mut dev.thin {
                if p == from {
                    *p = to.to_string();
                }
            }
        }
        g.push(MockEvent::Rename(from.to_string(), to.to_string()));
        Ok(())
    }

    fn read_dev(&self, name: &str, off: u64, len: u64) -> Result<Vec<u8>, StackError> {
        let g = self.inner.lock().unwrap();
        // ROUND 3: raw DM ioctls create no /dev/mapper nodes. A thin dev
        // that exists in the pool but was never activated as a dm device
        // has nothing to open — reads must go through activation.
        if !g.devices.contains_key(name) {
            return Err(nf(format!(
                "read {name}: no such dm device (activate first — nightly round 3)"
            )));
        }
        // Reads remap through dm-linear like any bio (the shim/port and
        // the swing's frozen top-up read through linears).
        let target = match g.resolve_dev(name)? {
            Ok(t) => t,
            Err(ext) => return Err(nf(format!("read {name}: {ext} is not a thin device"))),
        };
        let d = &g.devices[&target];
        let (pool, id) = d.thin.clone().expect("resolved to thin");
        if off + len > g.size_bytes {
            return Err(nf(format!("read {name}: beyond end of device")));
        }
        let img = g.image_of(&pool, id, d.backing.as_deref());
        Ok(img[off as usize..(off + len) as usize].to_vec())
    }

    fn set_discard_max_bytes(&self, name: &str, bytes: u64) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if !g.devices.contains_key(name) {
            return Err(nf(format!("queue limits {name}: no such dm device")));
        }
        if g.discard_knob_absent.contains(name) {
            return Err(StackError::Unsupported(format!(
                "no queue/discard_max_bytes for {name}"
            )));
        }
        // The ignored arm still *writes*: the event is what the caller
        // did, and only the readback reveals it did not stick.
        if !g.discard_knob_ignored.contains(name) {
            g.discard_max.insert(name.to_string(), bytes);
        }
        // The swap happens after the write took: from here the name
        // resolves to a device with no such file.
        if g.discard_knob_vanishing.remove(name) {
            g.discard_knob_absent.insert(name.to_string());
        }
        g.push(MockEvent::SetDiscardMax(name.to_string(), bytes));
        Ok(())
    }

    fn discard_max_bytes(&self, name: &str) -> Result<u64, StackError> {
        let g = self.inner.lock().unwrap();
        if !g.devices.contains_key(name) {
            return Err(nf(format!("queue limits {name}: no such dm device")));
        }
        if g.discard_knob_absent.contains(name) {
            return Err(StackError::Unsupported(format!(
                "no queue/discard_max_bytes for {name}"
            )));
        }
        // Never masked ⇒ the kernel's own limit, which is nonzero for
        // every queue that supports discard at all.
        Ok(g.discard_max
            .get(name)
            .copied()
            .unwrap_or(KERNEL_DISCARD_MAX_DEFAULT))
    }

    fn discard_dev(&self, name: &str, off: u64, len: u64) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.fail_next_discard.as_deref() == Some(name) {
            g.fail_next_discard = None;
            return Err(StackError::Fault("injected discard failure"));
        }
        // A zeroed `discard_max_bytes` is a queue that advertises no
        // discard support, and the block layer refuses the request
        // outright — Invariant B's guest-discard mask, enforced so a
        // guard applied to the wrong device (the port, say) fails a
        // test instead of silently disabling reclaim.
        if g.discard_max.get(name) == Some(&0) {
            return Err(nf(format!("EOPNOTSUPP: discard {name}: queue masked")));
        }
        // Resolve through a dm-linear port (D-M15 discard port): the
        // discard bio is remapped to the linear's component device, so a
        // port over a thin device unmaps the thin's blocks. Only the
        // whole-device zero-offset shape is modeled — the only one the
        // stack builds.
        if !g.devices.contains_key(name) {
            return Err(nf(format!("discard {name}: no such dm device")));
        }
        let target = match g.resolve_dev(name)? {
            Ok(t) => t,
            Err(ext) => return Err(nf(format!("discard {name}: {ext} is not a thin device"))),
        };
        let (pool, id) = g.devices[&target].thin.clone().expect("resolved to thin");
        if off + len > g.size_bytes {
            return Err(nf(format!("discard {target}: beyond end of device")));
        }
        // dm-thin unmaps only whole pool blocks; a partial block would be
        // silently kept mapped. Reject misalignment so misuse fails
        // loudly instead of half-reclaiming.
        let chunk = g.chunk;
        if len == 0 || !off.is_multiple_of(chunk) || !len.is_multiple_of(chunk) {
            return Err(nf(format!(
                "discard {name}: range [{off}, +{len}) is not block-aligned"
            )));
        }
        for b in off / chunk..(off + len) / chunk {
            g.thin_mut(&pool, id).unwrap().mappings.remove(&b);
        }
        // Unmapped pool blocks return to the pool's free space — the
        // last-ref drop the dual-discard eviction counts on.
        g.gc_data();
        g.push(MockEvent::Discard(name.to_string(), off, len));
        Ok(())
    }

    fn write_dev(&self, name: &str, off: u64, data: &[u8]) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if !g.devices.contains_key(name) {
            return Err(nf(format!(
                "write {name}: no such dm device (activate first — nightly round 3)"
            )));
        }
        match g.resolve_dev(name)? {
            Ok(target) => {
                let (pool, id) = g.devices[&target].thin.clone().expect("resolved to thin");
                if off + data.len() as u64 > g.size_bytes {
                    return Err(nf(format!("write {name}: beyond end of device")));
                }
                g.write_thin(&pool, id, off, data);
            }
            Err(_ext) => {
                // The chain leaves the dm table (a linear carve over a
                // loop node — the swing's slot-metadata zeroing). The
                // bytes land on the backing file; nothing reads them
                // back through the mock, so recording the event is the
                // model.
            }
        }
        g.push(MockEvent::WriteDev(
            name.to_string(),
            off,
            data.len() as u64,
        ));
        Ok(())
    }

    fn prepare_backing(
        &self,
        file: &Path,
        bytes: u64,
        prealloc: bool,
    ) -> Result<String, StackError> {
        let mut g = self.inner.lock().unwrap();
        if prealloc && g.fs_free < bytes {
            return Err(nf(format!("ENOSPC: prepare {}", file.display())));
        }
        if !g.backings.contains_key(file) {
            g.backings.insert(
                file.to_path_buf(),
                MockBacking {
                    size: bytes,
                    prealloc,
                    loop_dev: None,
                    allocated: None,
                },
            );
        }
        let dev = match g.backings.get(file).unwrap().loop_dev.clone() {
            Some(l) => l,
            None => {
                let l = format!("mockloop-{}", g.next_loop);
                g.next_loop += 1;
                g.backings.get_mut(file).unwrap().loop_dev = Some(l.clone());
                l
            }
        };
        g.push(MockEvent::PrepareBacking(file.display().to_string()));
        Ok(dev)
    }

    fn ensure_preallocated(&self, file: &Path) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        let free = g.fs_free;
        let entry = g
            .backings
            .get_mut(file)
            .ok_or_else(|| nf(format!("fallocate {}: no such file", file.display())))?;
        if !entry.prealloc {
            if free < entry.size {
                return Err(nf(format!("ENOSPC: fallocate {}", file.display())));
            }
            entry.prealloc = true;
        }
        g.push(MockEvent::Preallocate(file.display().to_string()));
        Ok(())
    }

    fn backing_of(&self, file: &Path) -> Result<Option<String>, StackError> {
        let g = self.inner.lock().unwrap();
        Ok(g.backings.get(file).and_then(|b| b.loop_dev.clone()))
    }

    fn backing_exists(&self, file: &Path) -> Result<bool, StackError> {
        Ok(self.inner.lock().unwrap().backings.contains_key(file))
    }

    fn release_backing(&self, loop_dev: &str) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        let Some(entry) = g
            .backings
            .values_mut()
            .find(|b| b.loop_dev.as_deref() == Some(loop_dev))
        else {
            return Err(nf(format!("losetup -d {loop_dev}: not attached")));
        };
        entry.loop_dev = None;
        g.push(MockEvent::ReleaseBacking(loop_dev.to_string()));
        Ok(())
    }

    fn delete_backing(&self, file: &Path) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if let Some(b) = g.backings.get(file) {
            // A loop device still holds the file open: deleting under it
            // is an ordering bug the mock refuses to hide.
            if let Some(l) = &b.loop_dev {
                return Err(nf(format!("rm {}: still attached to {l}", file.display())));
            }
            g.backings.remove(file);
            g.push(MockEvent::DeleteBacking(file.display().to_string()));
        }
        Ok(())
    }

    fn rename_backing(&self, from: &Path, to: &Path) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.backings.contains_key(to) {
            return Err(nf(format!("rename target {} exists", to.display())));
        }
        let entry = g
            .backings
            .remove(from)
            .ok_or_else(|| nf(format!("rename {}: no such file", from.display())))?;
        g.backings.insert(to.to_path_buf(), entry);
        g.push(MockEvent::RenameBacking(
            from.display().to_string(),
            to.display().to_string(),
        ));
        Ok(())
    }

    fn fs_free_bytes(&self, dir: &Path) -> Result<u64, StackError> {
        // One more kernel contract worth enforcing here: `statvfs("")`
        // is ENOENT, not "the current directory". `Path::parent` of a
        // bare relative filename yields exactly that empty path, so a
        // caller that forgets to resolve it loses §8.1 rule 2's second
        // surface silently — which is the failure this rejection turns
        // into a test failure.
        if dir.as_os_str().is_empty() {
            return Err(nf("statvfs \"\": ENOENT".to_string()));
        }
        Ok(self.inner.lock().unwrap().fs_free)
    }

    fn backing_alloc(&self, file: &Path) -> Result<(u64, u64), StackError> {
        let g = self.inner.lock().unwrap();
        let b = g
            .backings
            .get(file)
            .ok_or_else(|| nf(format!("stat {}: no such file", file.display())))?;
        // Sparse until something allocates: `prepare_backing(prealloc)`
        // and `ensure_preallocated` cover the file, a pinned value
        // models everything in between.
        let allocated = b.allocated.unwrap_or(if b.prealloc { b.size } else { 0 });
        Ok((b.size, allocated))
    }

    fn freeze_fs(&self, mount: &Path) -> Result<(), StackError> {
        let delay = {
            let mut g = self.inner.lock().unwrap();
            if let Some(e) = g.fail_next_freeze.take() {
                return Err(e);
            }
            if g.frozen {
                return Err(StackError::AlreadyFrozen(
                    "EBUSY: filesystem already frozen".into(),
                ));
            }
            // A cleanly read-only super FREEZES SUCCESSFULLY. Measured
            // on 6.18.5, and it is the opposite of what this mock
            // asserted at first: `freeze_super` short-circuits on
            // `sb_rdonly` and returns 0, because a filesystem with no
            // writers is already quiescent.
            //
            //   ro,relatime                     FIFREEZE -> 0
            //   rw,relatime,abort,emergency_ro  FIFREEZE -> EROFS
            //
            // So §8.1 rule 3's own remount does NOT arm the §8.2 step 3
            // fallback — a volume rule 3 protected gets an ordinary,
            // filesystem-consistent capture, which is strictly better.
            // The ABORTED super is what answers `EROFS`, and it is
            // modeled separately because the kernel reports it as `rw`
            // in `/proc/mounts` while refusing every write: read-only
            // and aborted are different states, and conflating them is
            // what hid this.
            if g.aborted {
                return Err(StackError::Wedged(format!(
                    "FIFREEZE {}: EROFS — the filesystem has shut down; \
                     its on-disk state is already crash-consistent",
                    mount.display()
                )));
            }
            g.frozen = true;
            g.push(MockEvent::Freeze);
            g.freeze_delay.take()
        };
        // Stall AFTER the freeze landed, without the lock, so a freeze
        // watchdog can fire inside the window (as in the kernel, where
        // the freeze and the thaw ioctl do not serialize on one lock).
        if let Some(d) = delay {
            std::thread::sleep(d);
        }
        Ok(())
    }

    fn thaw_fs(&self, _mount: &Path) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if std::mem::take(&mut g.fail_next_thaw) {
            // Still frozen — the whole reason a refused thaw cannot be
            // reported as a successful repair.
            return Err(StackError::Fault("EBUSY: FITHAW refused"));
        }
        // `FITHAW` on an un-frozen filesystem is EINVAL, which
        // `LinuxDmOps` maps to `Ok` — thawing what is already thawed is
        // the ordinary case on every unconditional-thaw path, and the
        // errors worth propagating are the ones that leave it frozen.
        // No event: nothing changed state.
        if !g.frozen {
            return Ok(());
        }
        g.frozen = false;
        g.push(MockEvent::Thaw);
        Ok(())
    }

    fn remount_ro(&self, _mount: &Path) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        // A frozen filesystem cannot be remounted — the kernel blocks
        // the writer that a remount is. Rule 3 only ever fires between
        // cycles, so this is a contract assertion, not a case to handle.
        if g.frozen {
            return Err(StackError::Fault("EBUSY: remount of a frozen filesystem"));
        }
        g.read_only = true;
        g.push(MockEvent::Remount(true));
        Ok(())
    }

    fn remount_rw(&self, _mount: &Path) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.frozen {
            return Err(StackError::Fault("EBUSY: remount of a frozen filesystem"));
        }
        g.read_only = false;
        g.push(MockEvent::Remount(false));
        Ok(())
    }

    fn mount_is_writable(&self, _mount: &Path) -> Result<bool, StackError> {
        let g = self.inner.lock().unwrap();
        // Both states refuse writes; only one of them is visible to
        // `statvfs`, which is the whole point.
        Ok(!g.read_only && !g.aborted)
    }

    fn mount_is_read_only(&self, _mount: &Path) -> Result<bool, StackError> {
        // Only the CLEAN read-only flag. An aborted super still reports
        // `rw` — that asymmetry is the kernel's, and modeling it is
        // what keeps the restart-adoption path honest.
        Ok(self.inner.lock().unwrap().read_only)
    }

    fn watchdog_thaw(&self, _mount: &Path) -> Result<Box<dyn FnMut() + Send>, StackError> {
        let inner = Arc::clone(&self.inner);
        Ok(Box::new(move || {
            let mut g = inner.lock().unwrap();
            if g.frozen {
                g.frozen = false;
                g.events.push(MockEvent::ForceThaw);
            }
        }))
    }

    fn thin_mappings(
        &self,
        _metadata_dev: &Path,
        dev_id: u64,
    ) -> Result<Vec<(u64, u64, u64)>, StackError> {
        let g = self.inner.lock().unwrap();
        // ROUND 4 adjacency: mapping reads are only valid under a reserved
        // metadata snapshot (thinp's read_superblock_snap fails without
        // one), and they see the point-in-time tree, not the live one.
        let snap = g.meta_snap.as_ref().ok_or_else(|| {
            nf("tmeta superblock: no metadata snap reserved (reserve first)".into())
        })?;
        let t = snap
            .get(&dev_id)
            .ok_or_else(|| nf(format!("dev {dev_id} not in metadata snap")))?;
        let mut out: Vec<(u64, u64, u64)> = Vec::new();
        for (&tb, &db) in &t.mappings {
            if let Some(last) = out.last_mut() {
                if last.0 + last.2 == tb && last.1 + last.2 == db {
                    last.2 += 1;
                    continue;
                }
            }
            out.push((tb, db, 1));
        }
        Ok(out)
    }
}

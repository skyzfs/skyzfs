//! `DmStack`: raw dm-thin `LocalStack` — no lvm2, no subprocesses
//! (agent design doc §14 M10b notes, §14 M9 `DmOps` seam).
//!
//! Everything the kernel is asked to do goes through the [`DmOps`] trait:
//! [`crate::dm_linux::LinuxDmOps`] (feature `dm`) speaks real DM ioctls,
//! `FIFREEZE`/`FITHAW`, and `thinp` metadata reads; [`crate::dm_mock::MockDmOps`]
//! models a thin pool in memory and *enforces the kernel's contracts* —
//! the ones that cost nightly rounds 2–4 — so this file's orchestration
//! (freeze/suspend ordering, dev-id retry, watchdog interplay, role
//! renames, reserve/release hygiene) is exercised by plain `cargo test`.
//!
//! Naming: managed snapshots are dm devices
//! `sky-{volume}-{b|c|u}-{logical}` — the role letter (`b`aseline,
//! `c`andidate, `u`ntagged) is part of the (renameable) dm name, so
//! observation reconstructs roles from the kernel's device list alone.
//! Untag is a REAL rename to `u-` — the §15.4 promotion window must never
//! show two baselines to a recovering observer (the DmStack crash matrix
//! caught a no-op untag doing exactly that). dm UUIDs are immutable and
//! carry only identity: `SKYNBD-{volume}-{dev_id}`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use crate::planner::LocalObservation;
use crate::stack::{
    DeltaRange, LocalStack, Pressure, ReclaimSets, SnapRole, StackError, SwingHooks, SwingStats,
};
use crate::state::LocalSnap;
use crate::watchdog::{FreezeOutcome, Watchdog};

/// One device in the kernel's device-mapper table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DmDeviceInfo {
    pub name: String,
    /// Immutable dm UUID (`SKYNBD-{volume}-{dev_id}` for ours).
    pub uuid: Option<String>,
    pub major: u32,
    pub minor: u32,
}

/// The kernel surface `DmStack` depends on — the M9 seam.
///
/// Contracts every implementation (and the mock's *enforcement*) must
/// honor, learned the hard way in the first nightly campaign:
///
/// - `pool_message("create_snap …")` requires the **origin thin device to
///   be suspended** first; newer kernels reject the wrong order with
///   `EINVAL` (round 2).
/// - Raw DM ioctls create **no `/dev/mapper` nodes**; `read_dev` must not
///   assume udev ran (round 3 — `LinuxDmOps` mknods a private node).
/// - The pool's tmeta device is held open by the live pool target;
///   metadata reads must open it **non-exclusively** (round 4).
/// - `reserve_metadata_snap`/`release_metadata_snap` must pair; reads of
///   `thin_mappings` are only valid under a reservation and see the
///   reserved (point-in-time) tree, not the live one.
pub trait DmOps: Send + Sync {
    fn list_devices(&self) -> Result<Vec<DmDeviceInfo>, StackError>;
    fn device_info(&self, name: &str) -> Result<DmDeviceInfo, StackError>;
    /// Send a message to the thin-pool target (`create_snap`, `delete`,
    /// `reserve_metadata_snap`, `release_metadata_snap`).
    fn pool_message(&self, pool: &str, msg: &str) -> Result<(), StackError>;
    /// Raw params string of the pool target's status line.
    fn pool_status_params(&self, pool: &str) -> Result<String, StackError>;
    fn suspend_device(&self, name: &str) -> Result<(), StackError>;
    fn resume_device(&self, name: &str) -> Result<(), StackError>;
    /// Retarget a device from inside a freeze window the CALLER already
    /// owns — the swing's, between its `freeze_fs` and its `thaw_fs`.
    ///
    /// dm suspends with an implicit `lock_fs` unless told otherwise, so
    /// the ordinary [`DmOps::reload_device`] freezes the filesystem a
    /// second time. That second freeze is a *kernel*-held one, counted
    /// separately from the `FIFREEZE` the caller took, and the swing
    /// leaked it: S15i's post-swing remount was refused with `dm-4:
    /// Can't mount, blockdev is frozen` on every run that ever reached
    /// it, with the superblock demonstrably thawed (the umount before it
    /// succeeded) and only the block device's count still raised.
    ///
    /// The fix is to not take the redundant freeze at all rather than to
    /// compensate for it afterwards: inside an owned window there is
    /// nothing for dm's lockfs to protect — the filesystem is already
    /// frozen, which is the precondition the retarget runs under.
    ///
    /// Defaults to the ordinary reload so mock-driven tests keep their
    /// existing behaviour; the flag only means anything to a real kernel.
    fn reload_device_in_frozen_window(
        &self,
        name: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        self.reload_device(name, table)
    }
    /// Create + load + resume a read-only `thin` target of `sectors`
    /// length over `pool`/`thin_dev_id`, with the given dm name and UUID.
    fn create_ro_thin(
        &self,
        name: &str,
        uuid: &str,
        pool: &str,
        sectors: u64,
        thin_dev_id: u64,
    ) -> Result<(), StackError>;
    /// Create + load + resume a device with an arbitrary table (the
    /// provisioning seam, V3.2 §6.5 stack build). Each line is
    /// `(start_sectors, len_sectors, target_type, params)` exactly as
    /// the kernel's table format. Referenced dm devices must already
    /// exist and be resumed — the kernel resolves `/dev/mapper/<name>`
    /// paths at load time, so creation order is a correctness contract
    /// (the mock enforces it).
    fn create_device(
        &self,
        name: &str,
        uuid: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError>;
    /// Replace an existing device's table (suspend → load → resume).
    /// Used to switch a cache device to the `cleaner` policy for the
    /// decommission flush (V3.2 §6.5: dirty writeback blocks are
    /// local-only until written back — an unstage that abandons them
    /// strands the head's latest writes on this node).
    fn reload_device(
        &self,
        name: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError>;
    fn remove_device(&self, name: &str) -> Result<(), StackError>;
    fn rename_device(&self, from: &str, to: &str) -> Result<(), StackError>;
    /// Read `len` bytes at `off` from an **activated** dm device.
    fn read_dev(&self, name: &str, off: u64, len: u64) -> Result<Vec<u8>, StackError>;
    fn freeze_fs(&self, mount: &Path) -> Result<(), StackError>;
    fn thaw_fs(&self, mount: &Path) -> Result<(), StackError>;
    /// Flip a mounted filesystem's SUPERBLOCK read-only in place, and
    /// back (`MS_REMOUNT`) — §8.1 rule 3's actuator.
    ///
    /// Deliberately not the CSI's `mount -o remount,bind,ro`
    /// (`node.rs`, the read-only publish): that flips a bind mount's own
    /// flag and leaves the superblock writable, so the filesystem keeps
    /// allocating right up to the dirty shutdown rule 3 exists to
    /// forestall. Rule 3 needs the writers stopped, not the view.
    fn remount_ro(&self, mount: &Path) -> Result<(), StackError>;
    fn remount_rw(&self, mount: &Path) -> Result<(), StackError>;
    /// Whether the mounted filesystem's superblock is read-only right
    /// now (`statvfs`'s `ST_RDONLY`).
    ///
    /// `statvfs` on purpose, and it is the only syscall that works
    /// here: on a shut-down XFS, `open`, `stat` and `readdir` all
    /// answer `EIO` while `statfs` still returns cleanly. Deriving
    /// this is what lets a restarted agent discover that §8.1 rule 3
    /// already took its volume read-only, without persisting anything.
    fn mount_is_read_only(&self, mount: &Path) -> Result<bool, StackError>;
    /// Whether the mounted filesystem will actually accept a write.
    ///
    /// Distinct from `!mount_is_read_only`, and the gap between them is
    /// a measured one: an ABORTED superblock reports `ST_RDONLY = 0`
    /// and lets `access(W_OK)` succeed while refusing every write with
    /// `EROFS`. `MS_REMOUNT` on such a super returns 0 and changes
    /// nothing, so a remount cannot be trusted to have taken — only a
    /// write can tell:
    ///
    /// ```text
    ///   clean ro  -> remount rc=0, rw,relatime                     write OK
    ///   ABORTED   -> remount rc=0, rw,...,abort,emergency_ro       write EROFS
    /// ```
    ///
    /// `Ok(false)` is a DEFINITE negative (`EROFS`/`EIO`). Any other
    /// errno is inconclusive and returns `Err` — a full pool answers
    /// `ENOSPC`, which says nothing about the superblock's health.
    fn mount_is_writable(&self, mount: &Path) -> Result<bool, StackError>;
    /// An independent force-thaw closure for the watchdog (V3.2 §7.11):
    /// must not share fds or locks with the capture path.
    fn watchdog_thaw(&self, mount: &Path) -> Result<Box<dyn FnMut() + Send>, StackError>;
    /// Mapping runs `(thin_begin, data_begin, len)` in pool blocks for
    /// `dev_id`, read from the pool metadata device. Caller must hold a
    /// reserved metadata snapshot.
    fn thin_mappings(
        &self,
        metadata_dev: &Path,
        dev_id: u64,
    ) -> Result<Vec<(u64, u64, u64)>, StackError>;
    /// Discard (unmap) a block-aligned byte range on an activated thin
    /// device — `BLKDISCARD` on the device node (D-M15 Invariant B). The
    /// pool must run `no_discard_passdown`, so the unmap never reaches
    /// the device below; dm-thin only unmaps whole pool blocks, so the
    /// range must be block-aligned (the mock rejects misalignment — the
    /// kernel would silently keep the partial blocks mapped).
    fn discard_dev(&self, name: &str, off: u64, len: u64) -> Result<(), StackError>;
    /// Write `data` at `off` on an **activated** dm device (the swing's
    /// copy target, slot-metadata zeroing, and the cold-cache
    /// superblock zero). CONTRACT: on return the bytes are visible to
    /// a subsequent table load — dm's metadata layers read the device
    /// through dm-bufio, not the bdev page cache, so an implementation
    /// that buffers must sync before returning (see
    /// `LinuxDmOps::write_dev`); the mock is synchronous by nature.
    fn write_dev(&self, name: &str, off: u64, data: &[u8]) -> Result<(), StackError>;
    /// Create `file` at `bytes` length (sparse via truncate; fully
    /// allocated when `prealloc`) and attach a loop device over it.
    /// Idempotent-ish: an existing file is reused (and an existing
    /// attachment returned). Returns the loop device path/name.
    fn prepare_backing(
        &self,
        file: &Path,
        bytes: u64,
        prealloc: bool,
    ) -> Result<String, StackError>;
    /// Allocate `file`'s full extent (fallocate) — the never-sparse
    /// steady-state posture, restored once slot A's space is freed.
    fn ensure_preallocated(&self, file: &Path) -> Result<(), StackError>;
    /// The loop device currently backed by `file`, if any.
    fn backing_of(&self, file: &Path) -> Result<Option<String>, StackError>;
    /// Whether `file` exists — the sweep's slot discriminator probe.
    fn backing_exists(&self, file: &Path) -> Result<bool, StackError>;
    /// Detach a loop device (`LOOP_CLR_FD`).
    fn release_backing(&self, loop_dev: &str) -> Result<(), StackError>;
    /// Remove `file`; absent is fine (sweeps replay).
    fn delete_backing(&self, file: &Path) -> Result<(), StackError>;
    /// Rename `from` → `to` on the slot filesystem (the swing's
    /// canonicalize step).
    fn rename_backing(&self, from: &Path, to: &Path) -> Result<(), StackError>;
    /// Available bytes on the filesystem holding `dir` (statvfs) — the
    /// §7 headroom guard's fs side, and §8.1 rule 2's second surface.
    fn fs_free_bytes(&self, dir: &Path) -> Result<u64, StackError>;
    /// `(logical, allocated)` bytes of a backing file — `st_size` and
    /// `st_blocks × 512`. The sparseness discriminator §8.1 rule 2
    /// turns on: `allocated >= logical` is a fully preallocated file
    /// whose space is already reserved, so the filesystem cannot
    /// exhaust underneath the pool; anything less can.
    fn backing_alloc(&self, file: &Path) -> Result<(u64, u64), StackError>;
    /// Set a dm device's `queue/discard_max_bytes`; `0` makes the queue
    /// advertise no discard support, so `BLKDISCARD` on it answers
    /// `EOPNOTSUPP` (Invariant B's guest-discard masking — see
    /// [`mask_guest_discard`]). A sysfs write, not an ioctl: the knob
    /// lives under the device's KERNEL name (`dm-N`), which is not the
    /// `/dev/mapper` name.
    ///
    /// [`StackError::Unsupported`] means the running kernel has no such
    /// knob; every other error is a failure to write one that exists.
    fn set_discard_max_bytes(&self, name: &str, bytes: u64) -> Result<(), StackError>;
    /// Read `queue/discard_max_bytes` back — the same sysfs file
    /// [`DmOps::set_discard_max_bytes`] writes. Task 15j's completion of
    /// Invariant B's mask: the write alone is not evidence, because a
    /// queue whose limits are recomputed (or clamped) still advertises
    /// discard support and still passes a guest `fstrim` through.
    /// [`StackError::Unsupported`] = the kernel lacks the knob.
    fn discard_max_bytes(&self, name: &str) -> Result<u64, StackError>;
}

/// Mask guest-initiated discard on the filesystem's shim (D-M15
/// Invariant B): zero the shim queue's `discard_max_bytes`, so a guest
/// `fstrim` — a *default* `fstrim.timer` on many distros — gets
/// `EOPNOTSUPP` instead of unmapping blocks the origin still serves.
/// Unmasked, such an unmap diverges the served image from the published
/// generation in both directions at once: the block reads back as the
/// origin's published content while `thin_delta` reports the range
/// removed, so the next publish writes a hole over data the filesystem
/// still owns.
///
/// The mask sits on the SHIM — and, when §7.1's read cache sits over
/// the shim, on the cache device too, because dm-cache does not
/// re-export the mask: with passdown disabled (its ctr read the masked
/// shim's zero limit) it advertises nonzero virtual discard limits and
/// ABSORBS guest discard bios rather than forwarding or invalidating,
/// so the guest-facing refusal exists only if the mount device carries
/// its own zero. It cannot be the pool's `ignore_discard` and it must
/// not touch the port: the agent's own reclaim and eviction discards
/// enter through the unclaimed port, and disabling those disables
/// Invariant B's whole reclaim licence.
///
/// Verified, not best-effort (task 15j completes 15i's interim guard):
/// the write is followed by a **readback**, because the write landing
/// is not evidence the queue is masked — a limit that was clamped, or
/// recomputed by a reload racing this call, leaves a shim that passes
/// guest discards straight through while the log says it is guarded.
///
/// Two outcomes, deliberately different:
/// - the knob does not exist (ENOENT — the running kernel lacks it):
///   tolerated, logged once. A capability the kernel never had cannot
///   make a volume unstageable, and there is nothing to re-assert.
/// - write or readback fails otherwise, or the readback is non-zero:
///   this is a shim that WILL pass guest discards. Return the error and
///   let the caller decide — [`crate::provision::build`] fails the
///   stage (a mounted origin-backed volume without the mask is the
///   hazard Invariant B declares unsupported), while the swing's and
///   the sweep's retarget sites log loudly and continue (aborting a
///   completed swing over a queue knob would be the worse failure).
///
/// The no-knob arm is asymmetric on purpose. It is a property of the
/// **write**: a kernel that has no such file never had one, so nothing
/// is being left unmasked. A `Unsupported` from the *readback* after a
/// write that SUCCEEDED says the opposite — the knob existed one
/// syscall ago, so the device behind the name was replaced between the
/// two resolutions, which is the reload race the readback exists to
/// catch. That combination must never report masked.
///
/// Queue limits are recomputed by a table reload, so every retarget of
/// the shim re-asserts this, `provision::build` re-asserts it
/// unconditionally at every stage — including a resume over a shim this
/// stack did not load — and the engine re-asserts it at the head of
/// every cycle ([`crate::LocalStack::assert_guest_discard_mask`]), so a
/// lapse cannot outlive one cadence interval unobserved.
pub fn mask_guest_discard<O: DmOps + ?Sized>(ops: &O, shim: &str) -> Result<(), StackError> {
    match ops.set_discard_max_bytes(shim, 0) {
        Ok(()) => {}
        Err(StackError::Unsupported(what)) => {
            no_knob(shim, &what);
            return Ok(());
        }
        Err(e) => return Err(e),
    }
    match ops.discard_max_bytes(shim) {
        Ok(0) => Ok(()),
        Ok(n) => {
            eprintln!(
                "skynbd-agent: guest-discard mask on {shim} did not take \
                 (queue/discard_max_bytes reads back {n}, not 0); a guest \
                 fstrim can unmap blocks the origin serves, diverging the \
                 served image from the published generation (Invariant B)"
            );
            Err(StackError::Fault(
                "guest-discard mask did not take on the shim (Invariant B)",
            ))
        }
        Err(StackError::Unsupported(what)) => {
            eprintln!(
                "skynbd-agent: the guest-discard knob on {shim} vanished between the \
                 write and the readback ({what}); the write proved it existed, so the \
                 device behind that name was replaced — the reload race the readback \
                 exists to catch, and the shim now in place is unverified (Invariant B)"
            );
            Err(StackError::Fault(
                "guest-discard knob vanished between write and readback — \
                 the shim was replaced mid-mask (Invariant B)",
            ))
        }
        Err(e) => Err(e),
    }
}

/// Print `msg` the first time this process sees `key`.
///
/// Per KEY, never process-global. These messages report a permanent
/// condition of one device or one file, and the call sites run once per
/// cycle per volume — a single `Once` would name whichever volume
/// tripped first and then stay silent forever about every other one on
/// the node, which is exactly the silence the messages were added to
/// break. Suppression is per process rather than durable on purpose:
/// the condition is re-derived every cycle, so a restart re-announces
/// it and nothing has to be persisted to learn it again.
fn warn_once(key: &str, msg: impl FnOnce() -> String) {
    if first_sighting(key) {
        eprintln!("{}", msg());
    }
}

/// Whether `key` is being seen for the first time in this process.
fn first_sighting(key: &str) -> bool {
    static SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let seen = SEEN.get_or_init(Default::default);
    let mut g = seen.lock().unwrap_or_else(|e| e.into_inner());
    g.insert(key.to_string())
}

/// The tolerated arm, logged once per shim: a kernel without the knob
/// is a property of the host, not of this call site, and every retarget
/// would otherwise repeat it.
fn no_knob(shim: &str, what: &str) {
    warn_once(&format!("no-knob:{shim}"), || {
        format!(
            "skynbd-agent: this kernel has no guest-discard mask knob for {shim} ({what}); \
             Invariant B's mask cannot be asserted here — a guest fstrim on such a host \
             is outside the supported envelope (§8.1)"
        )
    });
}

/// The swing/sweep call sites' half of that split: they run after (or
/// inside) an irreversible retarget, where the alternative to
/// continuing is unwinding a completed swap over a queue knob.
fn mask_guest_discard_or_warn<O: DmOps + ?Sized>(ops: &O, shim: &str) {
    if let Err(e) = mask_guest_discard(ops, shim) {
        eprintln!(
            "skynbd-agent: re-asserting the guest-discard mask on {shim} after a retarget \
             failed ({e}); the swap stands and the shim is unmasked — a guest fstrim can \
             now diverge the served image from the published generation (Invariant B)"
        );
    }
}

/// Static description of the managed thin stack on this host.
#[derive(Debug, Clone)]
pub struct DmConfig {
    /// dm name of the thin-pool (tpool) device, e.g. `skypool`.
    pub pool_name: String,
    /// dm name of the active thin origin device.
    pub origin_name: String,
    /// Thin device id of the origin within the pool.
    pub origin_dev_id: u64,
    /// The pool's *metadata* device path (for mapping reads under a
    /// reserved metadata snapshot). Known from provisioning config.
    pub metadata_dev: PathBuf,
    /// Mounted filesystem on the origin (freeze target).
    pub mountpoint: PathBuf,
    /// Lineage volume name (namespace for device names).
    pub volume: String,
    /// Origin size in bytes (= server volume size).
    pub size_bytes: u64,
    /// Thin-pool data block size in bytes (== object chunk size; the
    /// preflight enforces the divisor rule).
    pub chunk_bytes: u64,
    /// Sidecar file recording the next thin dev id to allocate. Lives in
    /// the (non-freezable) state dir; create_snap retries past EEXIST
    /// races, so a stale value is self-healing.
    pub ids_path: PathBuf,
    /// Hard cap on the freeze window (`capture_freeze_watchdog_ms`,
    /// V3.2 §8.5). On expiry the watchdog force-thaws and the capture
    /// fails cleanly.
    pub freeze_budget_ms: u64,
    /// The reclaim's **discard port** (D-M15): a dm-linear sibling of
    /// the origin whose bdev nothing claims. The kernel refuses
    /// `BLKDISCARD` on a claimed device (`EBUSY` — the mounted fs, or
    /// dm itself once a shim sits between fs and origin, holds the
    /// claim), so live reclaims must go through this port: dm claims
    /// components under one shared holder, making a second linear over
    /// the origin legal, and discard *bios* — unlike the ioctl entry —
    /// never check claims. `None` = discard the origin device directly
    /// (only valid while nothing has it claimed, e.g. unmounted rigs).
    pub discard_name: Option<String>,
    /// The file whose filesystem bounds this pool's growth — §8.1 rule
    /// 2's second exhaustion surface. Independent of [`Self::slot`] and
    /// deliberately so: every CSI volume is slot-less until 15e, but
    /// every CSI volume's pool data already lives inside one file on a
    /// node filesystem, and that filesystem can exhaust underneath a
    /// pool that believes it has free blocks. Keying the surface on the
    /// slot would leave it dark exactly where it is needed today.
    /// `None` = the pool's growth is not file-bounded (a raw device),
    /// so pressure is pool occupancy alone.
    pub backing_file: Option<PathBuf>,
    /// The dm-linear the guest filesystem claims — where a guest
    /// `fstrim` enters, and therefore where Invariant B's mask has to
    /// sit. Carried here (not only on [`SlotConfig::shim_name`])
    /// because every CSI stack has a shim while `slot` stays `None`
    /// until 15e: keying the per-cycle re-assert on the slot would
    /// leave the mask unmonitored on every deployment that has one.
    /// `None` = no shim (unmounted config-file rigs), so there is
    /// nothing a guest could trim.
    pub shim_name: Option<String>,
    /// The guest-visible MOUNT device when it is not the shim — §7.1's
    /// read cache sitting over it. dm-cache does not re-export the
    /// masked shim's discard limits: with passdown disabled it
    /// advertises nonzero virtual limits of its own, so the per-cycle
    /// re-assert keyed on the shim alone would verify a mask the guest
    /// cannot see while the device the guest CAN see advertises
    /// discard support. When set, [`LocalStack::assert_guest_discard_mask`]
    /// masks this device as well. `None` = the shim is the mount
    /// device (no read cache), the pre-§7.1 shape.
    pub mount_name: Option<String>,
    /// The managed pool slot, when this stack owns its pool's backing
    /// file — the two-pool swing's precondition (§7). `None` = the pool
    /// data sits on something the stack must not duplicate (today's
    /// writeback dm-cache over the shared NBD head: a second pool there
    /// would be a second head attach), so the swing is unsupported and
    /// the policy falls back to the port.
    pub slot: Option<SlotConfig>,
}

/// The slot-managed pool shape (normative for slot-configured stacks;
/// the carve names below are the default — [`Self::tmeta_name`] /
/// [`Self::data_name`] substitute a provisioned stack's own):
///
/// ```text
/// live_file → loop → {pool_name}-tm (linear, meta_bytes @ 0)
///                  ├→ {pool_name}-da (linear, data_bytes @ meta_bytes)
///                  └→ pool_name (thin-pool) → origin_name (thin id 0,
///                        external origin) → shim/port linears above
/// ```
///
/// During a swing the B side wears the canonical names with a `-b`
/// suffix (`{pool_name}-b`, `{origin_name}-b`, `{pool_name}-btm`,
/// `{pool_name}-bda`) and its file is [`slot_b_path`]`(live_file)`;
/// teardown removes the A side wholesale and renames B back to
/// canonical — steady state is always canonical, so nothing outside the
/// swing ever sees a slot name.
#[derive(Debug, Clone)]
pub struct SlotConfig {
    /// The live slot's backing file (canonical name).
    pub live_file: PathBuf,
    /// Pool data region size in bytes.
    pub data_bytes: u64,
    /// Pool tmeta region size in bytes (carved at offset 0).
    pub meta_bytes: u64,
    /// The dm-linear the filesystem claims — the swing's retarget
    /// target (the shim is what keeps the stack hot-swappable).
    pub shim_name: String,
    /// Device path for the B pool thin's external origin — the same
    /// repointable origin attach the A thin reads through (§7: both
    /// slots share it; swings never touch it).
    pub external_origin: Option<String>,
    /// Canonical names of the two carve linears splitting the loop into
    /// `[tmeta][data]` — what the swing's teardown removes on the A side
    /// and its canonicalize renames the B side to. `None` = the config-
    /// rig convention (`{pool_name}-tm` / `{pool_name}-da`); provisioned
    /// stacks pass their [`crate::provision::StackNames`] carve names
    /// (`sky-{v}-tmeta` / `sky-{v}-data`) so the devices the swing
    /// leaves behind are exactly the ones `provision::build`'s
    /// idempotent resume (and `observe`, and teardown) already own.
    /// Half-set is a config bug; the resolver treats it as unset.
    pub tmeta_name: Option<String>,
    pub data_name: Option<String>,
}

/// The directory §8.1 rule 2's `statvfs` should ask about.
///
/// `Path::parent` answers `Some("")` for a bare relative name, and
/// `statvfs("")` is ENOENT — which would have taken the second surface
/// down silently on any stack configured with a relative backing path.
/// An empty parent means "the current directory", which is where a
/// relative path resolves, so say so.
fn backing_dir(file: &Path) -> PathBuf {
    match file.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// Report that §8.1 rule 2's second surface is off — once per BACKING
/// FILE, i.e. once per volume, like the mask's no-knob path: the
/// condition is a property of that file or its filesystem, not of the
/// cycle that happened to notice, and the cycle notices every time.
fn surface_off(file: &Path, why: &str) {
    warn_once(&format!("surface-off:{}", file.display()), || {
        format!(
            "skynbd-agent: cannot measure the backing filesystem under {} ({why}); \
             §8.1 rule 2's second exhaustion surface is OFF for this volume and the \
             watermark now sees pool occupancy alone. A backing renamed out from under \
             a live stack (a quarantine) reads exactly like this — the pool keeps \
             reporting free blocks while the filesystem beneath it can still exhaust, \
             which surfaces as loop-device EIO below the whole queue/timeout ladder",
            file.display()
        )
    });
}

/// The B slot's backing file: the live file's name with `.b` appended.
/// One shared helper so the swing, the sweeps, and any external
/// inspector agree on the transient name.
pub fn slot_b_path(live: &Path) -> PathBuf {
    let mut name = live
        .file_name()
        .map(|n| n.to_os_string())
        .unwrap_or_default();
    name.push(".b");
    live.with_file_name(name)
}

pub struct DmStack<O: DmOps> {
    cfg: DmConfig,
    ops: O,
    /// Serializes capture/rename/delete against observation.
    lock: Mutex<()>,
    /// Freeze watchdog with its own pre-opened thaw path (V3.2 §7.11): no
    /// child process or engine lock can hold the thaw hostage.
    watchdog: Watchdog,
}

impl<O: DmOps> DmStack<O> {
    pub fn new(cfg: DmConfig, ops: O) -> Result<Self, StackError> {
        let thaw = ops.watchdog_thaw(&cfg.mountpoint)?;
        let watchdog = Watchdog::spawn(thaw);
        Ok(Self {
            cfg,
            ops,
            lock: Mutex::new(()),
            watchdog,
        })
    }

    fn dm_name(&self, role: Option<SnapRole>, logical: &str) -> String {
        let r = match role {
            Some(SnapRole::Baseline) => 'b',
            Some(SnapRole::Candidate) => 'c',
            None => 'u',
        };
        format!("sky-{}-{}-{}", self.cfg.volume, r, logical)
    }

    /// Parse `sky-{volume}-{b|c|u}-{logical}` for *this* volume.
    fn parse_name<'a>(&self, name: &'a str) -> Option<(Option<SnapRole>, &'a str)> {
        let rest = name.strip_prefix(&format!("sky-{}-", self.cfg.volume))?;
        let (role, logical) = rest.split_at(2.min(rest.len()));
        match role {
            "b-" => Some((Some(SnapRole::Baseline), logical)),
            "c-" => Some((Some(SnapRole::Candidate), logical)),
            "u-" => Some((None, logical)),
            _ => None,
        }
    }

    fn find_device(&self, logical: &str) -> Result<(String, Option<SnapRole>), StackError> {
        for d in self.ops.list_devices()? {
            if let Some((role, l)) = self.parse_name(&d.name) {
                if l == logical {
                    return Ok((d.name, role));
                }
            }
        }
        Err(StackError::NotFound(logical.to_string()))
    }

    fn pool_message(&self, msg: &str) -> Result<(), StackError> {
        self.ops.pool_message(&self.cfg.pool_name, msg)
    }

    /// `<transaction_id>` is the first field of thin-pool status.
    fn pool_tx_id(&self) -> Result<u64, StackError> {
        let params = self.ops.pool_status_params(&self.cfg.pool_name)?;
        params
            .split_whitespace()
            .next()
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| StackError::NotFound(format!("unparsable pool status {params:?}")))
    }

    fn next_dev_id(&self) -> u64 {
        std::fs::read_to_string(&self.cfg.ids_path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(self.cfg.origin_dev_id + 1)
    }

    fn persist_next_dev_id(&self, next: u64) {
        let _ = std::fs::write(&self.cfg.ids_path, next.to_string());
    }

    /// Activate a thin dev id read-only under `dm_name`.
    fn activate_ro(&self, dm_name: &str, dev_id: u64) -> Result<(), StackError> {
        let uuid = format!("SKYNBD-{}-{}", self.cfg.volume, dev_id);
        self.ops.create_ro_thin(
            dm_name,
            &uuid,
            &self.cfg.pool_name,
            self.cfg.size_bytes / 512,
            dev_id,
        )
    }

    fn dev_id_of(&self, dm_name: &str) -> Result<u64, StackError> {
        // Identity UUID: SKYNBD-{volume}-{dev_id}.
        let info = self.ops.device_info(dm_name)?;
        info.uuid
            .as_deref()
            .and_then(|u| u.rsplit('-').next())
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| StackError::NotFound(format!("no dev-id uuid on {dm_name}")))
    }
}

/// Two-pointer diff of sorted mapping-run lists → chunk-aligned byte
/// ranges. Mirrors upstream `dump_diff` semantics: same data block ⇒
/// shared (skip); different or right-only ⇒ objectize; left-only ⇒ clear.
/// Pure — unit-tested rootless.
pub fn diff_mappings(
    left: &[(u64, u64, u64)],  // (thin_begin, data_begin, len) in blocks
    right: &[(u64, u64, u64)], // same
    block_bytes: u64,
) -> Vec<DeltaRange> {
    let mut out: Vec<DeltaRange> = Vec::new();
    let mut push = |r: DeltaRange| {
        // Coalesce adjacent same-kind ranges.
        if let Some(last) = out.last_mut() {
            match (last, r) {
                (DeltaRange::Data { off, len }, DeltaRange::Data { off: o2, len: l2 })
                    if *off + *len == o2 =>
                {
                    *len += l2;
                    return;
                }
                (DeltaRange::Clear { off, len }, DeltaRange::Clear { off: o2, len: l2 })
                    if *off + *len == o2 =>
                {
                    *len += l2;
                    return;
                }
                _ => {}
            }
        }
        out.push(r);
    };
    let (mut li, mut ri) = (0usize, 0usize);
    let mut l = left.first().copied();
    let mut r = right.first().copied();
    loop {
        match (l, r) {
            (None, None) => break,
            (Some((tb, _, len)), None) => {
                push(DeltaRange::Clear {
                    off: tb * block_bytes,
                    len: len * block_bytes,
                });
                li += 1;
                l = left.get(li).copied();
            }
            (None, Some((tb, _, len))) => {
                push(DeltaRange::Data {
                    off: tb * block_bytes,
                    len: len * block_bytes,
                });
                ri += 1;
                r = right.get(ri).copied();
            }
            (Some((ltb, ldb, llen)), Some((rtb, rdb, rlen))) => {
                if ltb + llen <= rtb {
                    push(DeltaRange::Clear {
                        off: ltb * block_bytes,
                        len: llen * block_bytes,
                    });
                    li += 1;
                    l = left.get(li).copied();
                } else if rtb + rlen <= ltb {
                    push(DeltaRange::Data {
                        off: rtb * block_bytes,
                        len: rlen * block_bytes,
                    });
                    ri += 1;
                    r = right.get(ri).copied();
                } else if ltb < rtb {
                    let n = rtb - ltb;
                    push(DeltaRange::Clear {
                        off: ltb * block_bytes,
                        len: n * block_bytes,
                    });
                    l = Some((ltb + n, ldb + n, llen - n));
                } else if rtb < ltb {
                    let n = ltb - rtb;
                    push(DeltaRange::Data {
                        off: rtb * block_bytes,
                        len: n * block_bytes,
                    });
                    r = Some((rtb + n, rdb + n, rlen - n));
                } else {
                    // Aligned starts: compare data blocks over the overlap.
                    let n = llen.min(rlen);
                    if ldb != rdb {
                        push(DeltaRange::Data {
                            off: rtb * block_bytes,
                            len: n * block_bytes,
                        });
                    }
                    l = if llen > n {
                        Some((ltb + n, ldb + n, llen - n))
                    } else {
                        li += 1;
                        left.get(li).copied()
                    };
                    r = if rlen > n {
                        Some((rtb + n, rdb + n, rlen - n))
                    } else {
                        ri += 1;
                        right.get(ri).copied()
                    };
                }
            }
        }
    }
    out
}

/// Two-pointer walk of sorted mapping-run lists → the chunk-aligned byte
/// ranges mapped in BOTH devices with the SAME data block: the D-M15
/// Invariant B reclaim set, `mapped(S) − dirtied-since(S)`. COW gives
/// every post-capture rewrite a fresh data block (the snapshot holds the
/// old one), so same-data-block is exactly "not rewritten since". Within
/// one pair of overlapping runs the thin→data shift is constant, so an
/// overlap is shared all-or-nothing. Pure — unit-tested rootless.
pub fn shared_mappings(
    left: &[(u64, u64, u64)],  // (thin_begin, data_begin, len) in blocks
    right: &[(u64, u64, u64)], // same
    block_bytes: u64,
) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    let (mut li, mut ri) = (0usize, 0usize);
    while li < left.len() && ri < right.len() {
        let (ltb, ldb, llen) = left[li];
        let (rtb, rdb, rlen) = right[ri];
        let (lend, rend) = (ltb + llen, rtb + rlen);
        let s = ltb.max(rtb);
        let e = lend.min(rend);
        // Same data block at LBA x ⟺ ldb + (x−ltb) == rdb + (x−rtb).
        if s < e && ldb + rtb == rdb + ltb {
            let (off, len) = (s * block_bytes, (e - s) * block_bytes);
            match out.last_mut() {
                Some(last) if last.0 + last.1 == off => last.1 += len,
                _ => out.push((off, len)),
            }
        }
        if lend <= rend {
            li += 1;
        } else {
            ri += 1;
        }
    }
    out
}

/// Two-pointer walk of sorted mapping-run lists → the chunk-aligned byte
/// ranges mapped in `right` that are NOT same-data-block shared with
/// `left`: dirtied-since plus newly-provisioned — the swing's copy set
/// D, and [`shared_mappings`]'s exact complement within `right`. Built
/// on [`diff_mappings`], whose `Data` arm is precisely "different data
/// block or right-only". Pure — unit-tested rootless.
pub fn dirty_mappings(
    left: &[(u64, u64, u64)],  // (thin_begin, data_begin, len) in blocks
    right: &[(u64, u64, u64)], // same
    block_bytes: u64,
) -> Vec<(u64, u64)> {
    diff_mappings(left, right, block_bytes)
        .into_iter()
        .filter_map(|r| match r {
            DeltaRange::Data { off, len } => Some((off, len)),
            DeltaRange::Clear { .. } => None,
        })
        .collect()
}

/// How many shared ranges one freeze window may discard before thawing —
/// the reclaim batching answer to design §6.1 open question 1. Each
/// window recomputes the (monotonically shrinking) set under its own
/// freeze, so bounded windows compose into an arbitrarily large reclaim
/// without an unbounded write stall.
const RECLAIM_BATCH_RANGES: usize = 1024;

impl<O: DmOps> DmStack<O> {
    /// The D-M15 Invariant B reclaim: discard (unmap) every origin block
    /// that capture snapshot `snapshot` maps and the origin has not
    /// rewritten since. Caller contract (§3 step 6): the repoint to the
    /// generation published FROM `snapshot` is durable first, so every
    /// discarded block re-reads byte-identically through the external
    /// origin.
    ///
    /// Runs under an fs freeze, watchdog-armed like capture. The freeze
    /// is a correctness boundary, not caution: a live write racing a
    /// discard of the same block COWs to a fresh data block, and if the
    /// discard is processed after it, the unmap eats the new write —
    /// silent loss. Computing the set and discarding only while nothing
    /// can write closes the race; batching (recompute per window) keeps
    /// each freeze bounded.
    ///
    /// Returns the total bytes discarded.
    pub fn reclaim(&self, snapshot: &str) -> Result<u64, StackError> {
        let _g = self.lock.lock().unwrap();
        let (s_name, _) = self.find_device(snapshot)?;
        let s_id = self.dev_id_of(&s_name)?;
        let mut total = 0u64;
        loop {
            let (bytes, remaining) = self.reclaim_window(s_id)?;
            total += bytes;
            if remaining == 0 {
                return Ok(total);
            }
        }
    }

    /// One freeze window: recompute the shared set, discard at most
    /// [`RECLAIM_BATCH_RANGES`] ranges, thaw. Returns (bytes discarded,
    /// ranges left for the next window).
    fn reclaim_window(&self, snap_dev_id: u64) -> Result<(u64, usize), StackError> {
        self.watchdog
            .arm(std::time::Duration::from_millis(self.cfg.freeze_budget_ms));
        self.ops.freeze_fs(&self.cfg.mountpoint).inspect_err(|_| {
            self.watchdog.disarm();
        })?;
        let work = (|| -> Result<(u64, usize), StackError> {
            self.pool_message("reserve_metadata_snap")?;
            let computed = (|| {
                let snap = self
                    .ops
                    .thin_mappings(&self.cfg.metadata_dev, snap_dev_id)?;
                let live = self
                    .ops
                    .thin_mappings(&self.cfg.metadata_dev, self.cfg.origin_dev_id)?;
                Ok(shared_mappings(&snap, &live, self.cfg.chunk_bytes))
            })();
            let rel = self.pool_message("release_metadata_snap");
            let shared = computed?;
            rel?;
            let take = shared.len().min(RECLAIM_BATCH_RANGES);
            let mut bytes = 0u64;
            for &(off, len) in &shared[..take] {
                // If the watchdog force-thawed, writes may be landing —
                // stop before the next discard rather than race one.
                if self.watchdog.fired() {
                    return Err(StackError::Fault("reclaim aborted: freeze watchdog fired"));
                }
                let port = self
                    .cfg
                    .discard_name
                    .as_deref()
                    .unwrap_or(&self.cfg.origin_name);
                self.ops.discard_dev(port, off, len)?;
                bytes += len;
            }
            Ok((bytes, shared.len() - take))
        })();
        let _ = self.ops.thaw_fs(&self.cfg.mountpoint); // always thaw
        let outcome = self.watchdog.disarm();
        let result = work?;
        if outcome == FreezeOutcome::ForceThawed {
            return Err(StackError::Fault("reclaim aborted: freeze watchdog fired"));
        }
        Ok(result)
    }

    /// The reclaim set as it stands, without freezing or discarding
    /// (`debug-reclaim --dry-run`). Advisory: concurrent writes only ever
    /// shrink it, so it is an upper bound on what [`Self::reclaim`] will
    /// discard.
    pub fn reclaim_ranges(&self, snapshot: &str) -> Result<Vec<(u64, u64)>, StackError> {
        let _g = self.lock.lock().unwrap();
        let (s_name, _) = self.find_device(snapshot)?;
        let s_id = self.dev_id_of(&s_name)?;
        self.pool_message("reserve_metadata_snap")?;
        let computed = (|| {
            let snap = self.ops.thin_mappings(&self.cfg.metadata_dev, s_id)?;
            let live = self
                .ops
                .thin_mappings(&self.cfg.metadata_dev, self.cfg.origin_dev_id)?;
            Ok(shared_mappings(&snap, &live, self.cfg.chunk_bytes))
        })();
        let rel = self.pool_message("release_metadata_snap");
        let ranges = computed?;
        rel?;
        Ok(ranges)
    }
}

/// `(thin_begin, data_begin, len)` runs in pool blocks — what
/// [`DmOps::thin_mappings`] returns.
type MappingRuns = Vec<(u64, u64, u64)>;

/// Sector size the dm tables speak.
const SECTOR: u64 = 512;
/// Copy IO granularity for the swing's range copies.
const COPY_IO_BYTES: u64 = 1 << 20;
/// A copy round has converged when its dirty set shrinks to this many
/// chunks — the frozen top-up handles the remainder.
const SWING_CONVERGED_CHUNKS: u64 = 16;
/// Bound on live copy rounds: a workload dirtying faster than the copy
/// converges gets its tail folded into the frozen top-up instead of
/// looping forever.
const SWING_MAX_ROUNDS: u32 = 4;
/// Transient slack the headroom guard demands beyond D + slot metadata
/// (pool bootstrap allocations, fs metadata for the B file).
const SLOT_TRANSIENT_SLACK: u64 = 8 << 20;

/// The `-b`-suffixed names the transient B side wears during a swing,
/// plus the canonical A-side carve names ([`SlotConfig`]'s shape).
struct SlotNames {
    pool_b: String,
    origin_b: String,
    btm: String,
    bda: String,
    tm: String,
    da: String,
}

// ---------------------------------------------------------------- swing

impl<O: DmOps> DmStack<O> {
    fn slot_names(&self) -> SlotNames {
        // The canonical carve names are the slot's when it names them
        // (provisioned stacks: the same devices provision::build owns),
        // the `-tm`/`-da` convention otherwise. Both-or-neither: acting
        // on a half-set override would rename one carve into the
        // provision namespace and strand the other under a name nothing
        // tears down.
        let (tm, da) = match self
            .cfg
            .slot
            .as_ref()
            .and_then(|s| s.tmeta_name.clone().zip(s.data_name.clone()))
        {
            Some(pair) => pair,
            None => (
                format!("{}-tm", self.cfg.pool_name),
                format!("{}-da", self.cfg.pool_name),
            ),
        };
        SlotNames {
            pool_b: format!("{}-b", self.cfg.pool_name),
            origin_b: format!("{}-b", self.cfg.origin_name),
            btm: format!("{}-btm", self.cfg.pool_name),
            bda: format!("{}-bda", self.cfg.pool_name),
            tm,
            da,
        }
    }

    fn slot_uuid(&self, part: &str) -> String {
        // Distinct namespace from snapshot uuids (`SKYNBD-{v}-{dev_id}`)
        // and provisioning uuids (`SKYNBD-{v}-stack-{part}`).
        format!("SKYNBD-{}-slot-{}", self.cfg.volume, part)
    }

    /// Barrier device names: `sky-{v}-swb-{dev_id}` — outside the role
    /// namespace (`b|c|u` are two-char prefixes, `swb-` is not one), so
    /// observation never mistakes a barrier for a snapshot.
    fn barrier_prefix(&self) -> String {
        format!("sky-{}-swb-", self.cfg.volume)
    }

    fn mm(&self, name: &str) -> Result<String, StackError> {
        let info = self.ops.device_info(name)?;
        Ok(format!("{}:{}", info.major, info.minor))
    }

    fn device_exists(&self, name: &str) -> Result<bool, StackError> {
        Ok(self.ops.list_devices()?.iter().any(|d| d.name == name))
    }

    fn remove_if_exists(&self, name: &str) -> Result<bool, StackError> {
        if self.device_exists(name)? {
            self.ops.remove_device(name)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Remove every role-named snapshot activation of this volume
    /// (`sky-{v}-{b|c|u}-…`) — they are thins over pool A, which cannot
    /// be removed while they hold it open. Only called when pool A is
    /// condemned wholesale (the flip is durable), so the planner's row-8
    /// rebaseline covers the record still naming them.
    fn remove_role_devices(&self) -> Result<bool, StackError> {
        let mut found = false;
        for d in self.ops.list_devices()? {
            if self.parse_name(&d.name).is_some() {
                self.ops.remove_device(&d.name)?;
                found = true;
            }
        }
        Ok(found)
    }

    /// A snapshot barrier (Invariant B's swing note): suspend origin →
    /// `create_snap` → resume, activated RO under a transient name. NO
    /// freeze — barriers are mapping fences, not consistency points; the
    /// suspend is the kernel's create_snap contract, exactly as in
    /// capture.
    fn create_barrier(&self) -> Result<(String, u64), StackError> {
        let mut dev_id = self.next_dev_id();
        let sus = self.ops.suspend_device(&self.cfg.origin_name);
        let created = if sus.is_ok() {
            let start = dev_id;
            loop {
                match self.pool_message(&format!("create_snap {dev_id} {}", self.cfg.origin_dev_id))
                {
                    Ok(()) => break Ok(dev_id),
                    Err(_) if dev_id < start + 64 => {
                        dev_id += 1;
                        continue;
                    }
                    Err(e) => break Err(e),
                }
            }
        } else {
            Err(StackError::Fault("origin suspend failed"))
        };
        let res = self.ops.resume_device(&self.cfg.origin_name); // always resume
        let dev_id = created?;
        sus?;
        res?;
        self.persist_next_dev_id(dev_id + 1);
        let name = format!("{}{}", self.barrier_prefix(), dev_id);
        self.activate_ro(&name, dev_id)?;
        Ok((name, dev_id))
    }

    fn drop_barrier(&self, name: &str, dev_id: u64) -> Result<(), StackError> {
        self.ops.remove_device(name)?;
        self.pool_message(&format!("delete {dev_id}"))
    }

    /// Two devices' mapping runs from ONE reserved metadata snapshot.
    fn mappings_pair(
        &self,
        left_id: u64,
        right_id: u64,
    ) -> Result<(MappingRuns, MappingRuns), StackError> {
        self.pool_message("reserve_metadata_snap")?;
        let computed = (|| {
            let l = self.ops.thin_mappings(&self.cfg.metadata_dev, left_id)?;
            let r = self.ops.thin_mappings(&self.cfg.metadata_dev, right_id)?;
            Ok((l, r))
        })();
        let rel = self.pool_message("release_metadata_snap");
        let pair = computed?;
        rel?;
        Ok(pair)
    }

    fn copy_ranges(&self, from: &str, to: &str, ranges: &[(u64, u64)]) -> Result<u64, StackError> {
        let mut copied = 0u64;
        for &(off, len) in ranges {
            let mut pos = off;
            while pos < off + len {
                let take = COPY_IO_BYTES.min(off + len - pos);
                let data = self.ops.read_dev(from, pos, take)?;
                self.ops.write_dev(to, pos, &data)?;
                pos += take;
                copied += take;
            }
        }
        Ok(copied)
    }

    /// The §7 enumeration: one metadata-snapshot dump yields both the
    /// reclaim set R (shared) and the copy set D (dirty). Advisory — no
    /// freeze; the executors re-verify under their own windows.
    pub fn enumerate(&self, snapshot: &str) -> Result<ReclaimSets, StackError> {
        let _g = self.lock.lock().unwrap();
        let (s_name, _) = self.find_device(snapshot)?;
        let s_id = self.dev_id_of(&s_name)?;
        let (snap, live) = self.mappings_pair(s_id, self.cfg.origin_dev_id)?;
        Ok(ReclaimSets {
            shared: shared_mappings(&snap, &live, self.cfg.chunk_bytes),
            dirty: dirty_mappings(&snap, &live, self.cfg.chunk_bytes),
        })
    }

    /// The file whose filesystem bounds the pool's growth (§8.1 rule
    /// 2). `backing_file` wins over the slot: it is configured for
    /// every file-backed stack, while the slot arrives only with 15e's
    /// cache-less shape — but a slot-managed stack that predates the
    /// config still answers correctly through `live_file`.
    fn backing_path(&self) -> Option<&Path> {
        self.cfg
            .backing_file
            .as_deref()
            .or_else(|| self.cfg.slot.as_ref().map(|s| s.live_file.as_path()))
    }

    /// The shim carrying the guest filesystem's claim — Invariant B's
    /// mask target. Same precedence as [`Self::backing_path`]: the
    /// explicit config wins, a slot-managed stack answers through its
    /// retarget target.
    fn shim_path(&self) -> Option<&str> {
        self.cfg
            .shim_name
            .as_deref()
            .or_else(|| self.cfg.slot.as_ref().map(|s| s.shim_name.as_str()))
    }

    /// Invariant B, re-derived: write the mask and read it back on
    /// whatever shim this stack currently has. The engine calls this at
    /// the head of every cycle, so an unmasked shim — left by a
    /// retarget whose re-assert failed, or by a table this stack never
    /// loaded — is caught within one cadence interval instead of
    /// surviving until the next swing or a cold re-stage.
    ///
    /// A stack with NO shim configured answers
    /// [`StackError::Unsupported`], never `Ok`. `shim_name` is
    /// `#[serde(default)]`, so every config-file deployment provisioned
    /// before it existed carries `None` — and an `Ok` there is
    /// indistinguishable from a verified mask, which would make "a
    /// lapse cannot outlive one cadence interval unnoticed" false for
    /// exactly the deployments that never opted in. The distinction is
    /// the caller's to act on: this is not a lapse to retry (nothing
    /// was ever configured to assert against) but it is not protection
    /// either, so the engine says so once per volume and carries on.
    pub fn assert_guest_discard_mask(&self) -> Result<(), StackError> {
        match self.shim_path() {
            Some(shim) => {
                mask_guest_discard(&self.ops, shim)?;
                // §7.1: on a cached stack the guest mounts the rcache,
                // which advertises its own virtual discard limits — the
                // shim's mask is invisible through it, so the re-assert
                // must land on the mount device too.
                if let Some(mount) = self.cfg.mount_name.as_deref() {
                    mask_guest_discard(&self.ops, mount)?;
                }
                Ok(())
            }
            None => Err(StackError::Unsupported(format!(
                "volume {}: no shim is configured (`shim_name`, or `[volume.slot].shim_name` \
                 on a slot-managed stack), so there is no queue to mask",
                self.cfg.volume
            ))),
        }
    }

    /// Both §8.1 surfaces in one measurement: thin-pool occupancy from
    /// the pool status line, and — only when the backing file is SPARSE
    /// — the free space of the filesystem under it.
    ///
    /// A fully allocated backing (`st_blocks × 512 >= st_size`) has its
    /// space reserved already: a low `fs_free` cannot fail a pool write,
    /// and letting it bind would advance spuriously. A sparse backing
    /// can exhaust underneath the pool, which is §8.1 rule 2's whole
    /// point, so its free space is taken as an upper bound on the pool's
    /// free space. That is exact in the preallocated case and
    /// conservative in the sparse one (it ignores blocks already
    /// allocated to the file but not yet used by the pool) — every error
    /// is toward advancing earlier, never later.
    ///
    /// A backing the stack cannot stat (or a filesystem it cannot
    /// statvfs) degrades to the pool surface alone rather than to
    /// `None`: losing the second surface must not also disable the
    /// occupancy watermark that has worked since 15c. That degradation
    /// is **logged once per process**, because it is silent otherwise
    /// and it is permanent: a backing renamed out from under the stack
    /// (`quarantine_cache` does exactly that) turns the surface off for
    /// the life of the stack, and a surface that is off announces
    /// nothing.
    pub fn pool_pressure(&self) -> Option<Pressure> {
        let (used_blocks, total_blocks) = LocalStack::pool_usage(self)?;
        // Pool geometry comes from a status line the kernel wrote, but
        // the multiply is ours: a garbage `total` must not wrap into a
        // small number that reads as a nearly-full pool.
        let used_bytes = used_blocks.saturating_mul(self.cfg.chunk_bytes);
        let total_bytes = total_blocks.saturating_mul(self.cfg.chunk_bytes);
        let pool_free = total_bytes.saturating_sub(used_bytes);
        let mut effective_free = pool_free;
        let mut fs_bound = false;
        if let Some(file) = self.backing_path() {
            match self.ops.backing_alloc(file) {
                Ok((logical, allocated)) if allocated < logical => {
                    let dir = backing_dir(file);
                    match self.ops.fs_free_bytes(&dir) {
                        Ok(fs_free) => {
                            if fs_free < effective_free {
                                effective_free = fs_free;
                                fs_bound = true;
                            }
                        }
                        Err(e) => surface_off(file, &format!("statvfs {}: {e}", dir.display())),
                    }
                }
                Ok(_) => {}
                Err(e) => surface_off(file, &format!("stat: {e}")),
            }
        }
        Some(Pressure {
            used_bytes,
            total_bytes,
            effective_free,
            fs_bound,
        })
    }

    /// `(fs_free, required_transient)` for the §7 headroom guard —
    /// answerable only on slot-managed stacks (the swing's transient
    /// lands on the slot filesystem).
    pub fn headroom(&self) -> Option<(u64, u64)> {
        let slot = self.cfg.slot.as_ref()?;
        let dir = slot.live_file.parent().unwrap_or_else(|| Path::new("."));
        let free = self.ops.fs_free_bytes(dir).ok()?;
        Some((free, slot.meta_bytes + SLOT_TRANSIENT_SLACK))
    }

    /// The two-pool swing (§7 major compaction): build slot B sparse,
    /// copy D behind snapshot barriers in converging live rounds, then
    /// freeze → top-up → retarget → persist flip → thaw, and tear slot A
    /// down wholesale. Caller contract is [`Self::reclaim`]'s: the
    /// repoint to the generation published from `snapshot` is durable
    /// and served-origin fenced FIRST — the shim retarget is this
    /// executor's first irreversible act (Invariant E). Nothing is
    /// irreversible before `hooks.persist_flip` runs: any earlier
    /// failure (mid-copy ENOSPC included) aborts into the caller's
    /// sweep-B + port fallback in the same cycle.
    pub fn swing(
        &self,
        snapshot: &str,
        hooks: &mut SwingHooks<'_>,
    ) -> Result<SwingStats, StackError> {
        let _g = self.lock.lock().unwrap();
        let Some(slot) = self.cfg.slot.clone() else {
            return Err(StackError::Fault("swing on a slot-less stack"));
        };
        let names = self.slot_names();
        let (s_name, _) = self.find_device(snapshot)?;
        let s_id = self.dev_id_of(&s_name)?;

        // Idempotent pre-clean: an earlier aborted swing's B leftovers.
        self.sweep_b_inner(&slot, &names)?;

        // Build slot B — sparse (set_len), so the abortable copy phase
        // consumes only what it writes; preallocation waits until slot
        // A's space is freed.
        let bpath = slot_b_path(&slot.live_file);
        let loop_b = self
            .ops
            .prepare_backing(&bpath, slot.meta_bytes + slot.data_bytes, false)?;
        let meta_sectors = slot.meta_bytes / SECTOR;
        let data_sectors = slot.data_bytes / SECTOR;
        self.ops.create_device(
            &names.btm,
            &self.slot_uuid("btm"),
            &[(0, meta_sectors, "linear".into(), format!("{loop_b} 0"))],
        )?;
        // Fresh-superblock guarantee even if a leftover file was reused:
        // dm-thin refuses garbage metadata.
        self.ops.write_dev(&names.btm, 0, &[0u8; 4096])?;
        self.ops.create_device(
            &names.bda,
            &self.slot_uuid("bda"),
            &[(
                0,
                data_sectors,
                "linear".into(),
                format!("{loop_b} {meta_sectors}"),
            )],
        )?;
        // Same pool table shape as provisioning: single-tenant pool, so
        // skip_block_zeroing is safe; no_discard_passdown is Invariant
        // B's precondition.
        let blocks = slot.data_bytes / self.cfg.chunk_bytes;
        let low_water = (blocks / 20).max(32);
        let chunk_sectors = self.cfg.chunk_bytes / SECTOR;
        self.ops.create_device(
            &names.pool_b,
            &self.slot_uuid("pool"),
            &[(
                0,
                data_sectors,
                "thin-pool".into(),
                format!(
                    "{} {} {chunk_sectors} {low_water} 2 skip_block_zeroing no_discard_passdown",
                    self.mm(&names.btm)?,
                    self.mm(&names.bda)?
                ),
            )],
        )?;
        // B's thin carries A's dev id, not a fresh 0: after the flip the
        // B pool answers to the canonical names, so every later
        // enumeration, reclaim and eviction reads `cfg.origin_dev_id`
        // out of it. Minting a different id there would leave the whole
        // post-swing config pointing at a device that does not exist —
        // and "which executor ran is not observable" (§7) has to hold
        // for the cycle after the swing too.
        let origin_id = self.cfg.origin_dev_id;
        self.ops
            .pool_message(&names.pool_b, &format!("create_thin {origin_id}"))?;
        let origin_sectors = self.cfg.size_bytes / SECTOR;
        let thin_params = match &slot.external_origin {
            Some(ext) => format!("{} {origin_id} {ext}", self.mm(&names.pool_b)?),
            None => format!("{} {origin_id}", self.mm(&names.pool_b)?),
        };
        self.ops.create_device(
            &names.origin_b,
            &self.slot_uuid("origin"),
            &[(0, origin_sectors, "thin".into(), thin_params)],
        )?;

        // Live copy rounds behind snapshot barriers. The barrier rule is
        // normative (Invariant B's swing note): every round diffs
        // against its barrier snapshot, never head-vs-head, because
        // in-place rewrites of already-unshared blocks change no
        // mapping. Round 1 diffs the advance capture S itself; S's
        // lifecycle stays the engine's.
        let live_t = Instant::now();
        let mut stats = SwingStats::default();
        let mut prev: Option<(String, u64)> = None;
        let mut prev_id = s_id;
        loop {
            stats.rounds += 1;
            let (b_name, b_id) = self.create_barrier()?;
            let (prev_maps, barrier_maps) = self.mappings_pair(prev_id, b_id)?;
            let dirty = dirty_mappings(&prev_maps, &barrier_maps, self.cfg.chunk_bytes);
            let round_bytes: u64 = dirty.iter().map(|&(_, l)| l).sum();
            // The barrier image is stable; the head is not.
            stats.copied_bytes += self.copy_ranges(&b_name, &names.origin_b, &dirty)?;
            if let Some((pn, pid)) = prev.take() {
                self.drop_barrier(&pn, pid)?;
            }
            prev = Some((b_name, b_id));
            prev_id = b_id;
            if round_bytes <= SWING_CONVERGED_CHUNKS * self.cfg.chunk_bytes
                || stats.rounds >= SWING_MAX_ROUNDS
            {
                break;
            }
        }
        stats.live_ns = live_t.elapsed().as_nanos() as u64;
        let (last_name, last_id) = prev.expect("at least one copy round ran");

        // Frozen window, §7 normative order: top-up → retarget →
        // persist flip → thaw. The flip lands INSIDE the window, after
        // the retarget and before any write can flow — that is what
        // makes both crash arms exact (the state dir is off the frozen
        // fs, the same property capture relies on).
        let frozen_t = Instant::now();
        self.watchdog
            .arm(std::time::Duration::from_millis(self.cfg.freeze_budget_ms));
        self.ops.freeze_fs(&self.cfg.mountpoint).inspect_err(|_| {
            self.watchdog.disarm();
        })?;
        let work = (|| -> Result<u64, StackError> {
            let (barrier_maps, head_maps) = self.mappings_pair(last_id, self.cfg.origin_dev_id)?;
            let topup = dirty_mappings(&barrier_maps, &head_maps, self.cfg.chunk_bytes);
            // Frozen, so the head is stable; read through the port (the
            // unclaimed node — read-opens of the origin are legal too,
            // which is what slot rigs without a port use).
            let src = self
                .cfg
                .discard_name
                .as_deref()
                .unwrap_or(&self.cfg.origin_name);
            let copied = self.copy_ranges(src, &names.origin_b, &topup)?;
            // Retarget the shim — the swing's first irreversible act for
            // an authoritative attachment (Invariant E: the served-origin
            // fence already ran) — and the port, which must follow the
            // live thin wherever it goes. Build both tables first so the
            // last thing between the watchdog check and the suspend is
            // the reload itself: a force-thaw before this point means
            // writes may have landed on A after the top-up read it, and
            // retargeting would strand them.
            let b_mm = self.mm(&names.origin_b)?;
            let table = vec![(
                0u64,
                origin_sectors,
                "linear".to_string(),
                format!("{b_mm} 0"),
            )];
            if self.watchdog.fired() {
                return Err(StackError::Fault("swing aborted: freeze watchdog fired"));
            }
            self.ops
                .reload_device_in_frozen_window(&slot.shim_name, &table)?;
            *hooks.retargeted = true;
            // The reload recomputed the shim queue's limits from the new
            // table, dropping Invariant B's mask with them.
            mask_guest_discard_or_warn(&self.ops, &slot.shim_name);
            if let Some(port) = self.cfg.discard_name.as_deref() {
                self.ops.reload_device_in_frozen_window(port, &table)?;
            }
            // A fire in the sliver between that check and the suspend
            // let writes onto A after the top-up read it, and the
            // retarget has now stranded them there. The retarget is also
            // what makes them recoverable: A is quiescent behind it, so
            // one more diff against the same barrier converges in a
            // single pass. Copy it before the flip, or the wholesale
            // teardown below destroys acked writes.
            if self.watchdog.fired() {
                let (barrier_maps, head_maps) =
                    self.mappings_pair(last_id, self.cfg.origin_dev_id)?;
                let late = dirty_mappings(&barrier_maps, &head_maps, self.cfg.chunk_bytes);
                // Read A's thin directly: the port followed the shim to B.
                let extra = self.copy_ranges(&self.cfg.origin_name, &names.origin_b, &late)?;
                stats.copied_bytes += extra;
            }
            // Past the retarget a force-thaw is NOT a reason to abort:
            // the shim already points at B, so anything the thaw lets
            // through is landing on B, and persisting the flip is what
            // keeps those writes. Rolling back here would be the
            // data-losing choice, which is why the post-window
            // `ForceThawed` outcome is deliberately not fatal either.
            //
            // The durable flip. On failure the flip is NOT durable, so
            // this must become a clean pre-flip abort: put the A tables
            // back before the thaw (frozen, so no write ever reached B —
            // unless the watchdog fired, which `retargeted` has already
            // told the caller to treat as unrecoverable rather than
            // fall-back-able).
            if let Err(e) = (hooks.persist_flip)() {
                // Order matters: ask about the fire BEFORE rolling back,
                // never after. A fire means the thaw let guest writes
                // through onto B, and putting the shim back on A would
                // hand those writes to the armed sweep to delete — the
                // data-losing choice this window exists to avoid. Leave
                // the tables on B, keep the flag, and stall: recovery
                // reconciles against the record with both sides intact.
                if self.watchdog.fired() {
                    return Err(e);
                }
                let a_table = self.mm(&self.cfg.origin_name).map(|a_mm| {
                    vec![(
                        0u64,
                        origin_sectors,
                        "linear".to_string(),
                        format!("{a_mm} 0"),
                    )]
                })?;
                self.ops
                    .reload_device_in_frozen_window(&slot.shim_name, &a_table)?;
                mask_guest_discard_or_warn(&self.ops, &slot.shim_name);
                if let Some(port) = self.cfg.discard_name.as_deref() {
                    self.ops.reload_device_in_frozen_window(port, &a_table)?;
                }
                // Both tables are back on A and the freeze held, so this
                // is once again a clean pre-flip abort — the one path
                // that un-sets the flag. A rollback that fails leaves it
                // set (the `?`s above), and the caller stalls.
                *hooks.retargeted = false;
                return Err(e);
            }
            Ok(copied)
        })();
        let _ = self.ops.thaw_fs(&self.cfg.mountpoint); // always thaw
        let outcome = self.watchdog.disarm();
        stats.copied_bytes += work?;
        stats.frozen_ns = frozen_t.elapsed().as_nanos() as u64;
        // A force-thaw AFTER the flip persisted is survivable by
        // construction — the flip is durable and the crash arm covers a
        // death here — so teardown proceeds.
        let _ = outcome;

        // Teardown A + canonicalize. Every step here is replayed by
        // `sweep_slot(true)` after a crash, so ordering mirrors it:
        // devices, then the file swap, then renames, then prealloc.
        self.drop_barrier(&last_name, last_id)?;
        // Pool A dies wholesale — precondition honored: pools are
        // per-volume, so this destroys exactly one volume's state. The
        // kernel refuses to remove a pool with open thins, so every
        // remaining snapshot activation over it (S included — its thin
        // ids die with the pool's metadata) goes first; the engine's
        // §3 step-7 delete-S is `.ok()`-tolerant of the device already
        // being gone.
        self.remove_role_devices()?;
        self.ops.remove_device(&self.cfg.origin_name)?;
        self.ops.remove_device(&self.cfg.pool_name)?;
        self.remove_if_exists(&names.da)?;
        self.remove_if_exists(&names.tm)?;
        if let Some(l) = self.ops.backing_of(&slot.live_file)? {
            self.ops.release_backing(&l)?;
        }
        if self.ops.backing_exists(&slot.live_file)? {
            self.ops.delete_backing(&slot.live_file)?;
        }
        self.ops.rename_backing(&bpath, &slot.live_file)?;
        self.ops
            .rename_device(&names.origin_b, &self.cfg.origin_name)?;
        self.ops.rename_device(&names.pool_b, &self.cfg.pool_name)?;
        self.ops.rename_device(&names.btm, &names.tm)?;
        self.ops.rename_device(&names.bda, &names.da)?;
        // Slot A's space is free: restore the never-sparse posture. A
        // failure here is §8 exhaustion territory — log loudly, keep the
        // completed swing.
        if let Err(e) = self.ops.ensure_preallocated(&slot.live_file) {
            eprintln!(
                "skynbd-agent: slot preallocation of {} failed after swing ({e}); \
                 pool file is sparse until the next successful preallocation",
                slot.live_file.display()
            );
        }
        Ok(stats)
    }

    /// Sweep slot leftovers; `flipped` is the durable discriminator (§7
    /// crash windows). Returns whether anything was swept.
    pub fn sweep_slot(&self, flipped: bool) -> Result<bool, StackError> {
        let _g = self.lock.lock().unwrap();
        let Some(slot) = self.cfg.slot.clone() else {
            return Ok(false);
        };
        let names = self.slot_names();
        if flipped {
            self.sweep_flipped_inner(&slot, &names)
        } else {
            self.sweep_b_inner(&slot, &names)
        }
    }

    /// Pre-flip: slot B is garbage regardless of how far the build got
    /// (frozen, so no write ever reached B). Barriers, B devices
    /// bottom-up, the loop, the file.
    fn sweep_b_inner(&self, slot: &SlotConfig, names: &SlotNames) -> Result<bool, StackError> {
        let mut found = false;
        // A death between the retarget and the flip persist leaves the
        // shim (and port) aimed at B with the record still naming A, and
        // the freeze outliving the process that armed the watchdog. The
        // durable record is the authority: A is live, so put the tables
        // back and thaw BEFORE anything touches B — the shim's reference
        // is also the EBUSY that would otherwise fail the removal below,
        // turning this sweep into a permanently frozen filesystem on a
        // pool the next reclaim would discard through.
        if self.device_exists(&names.origin_b)? {
            let a_mm = self.mm(&self.cfg.origin_name)?;
            let table = vec![(
                0u64,
                self.cfg.size_bytes / SECTOR,
                "linear".to_string(),
                format!("{a_mm} 0"),
            )];
            // The ORDINARY reload here, and the reason is NOT that this
            // path owns no freeze window — ownership is not what caused
            // the swing's leak. What caused it was a `lock_fs` taken on a
            // superblock that was ALREADY frozen, and this branch can be
            // in exactly that state: the comment above describes the
            // freeze outliving the process that armed it.
            //
            // Nor does the `thaw_fs` below cover it. That is `FITHAW` on
            // the mountpoint — `thaw_super`, not `bdev_thaw` — so
            // lowering `bd_fsfreeze_count` is the one thing it cannot do.
            // If a leak is reachable here, this sweep reports a
            // permanently unmountable volume as a successful repair,
            // which is worse than the swing's version of the same bug
            // because this is the crash path.
            //
            // Own the freeze, so the retarget below can suppress dm's.
            //
            // MEASURED, and the answer is the bad one. FREEZEPROBE (a
            // loop, a linear shim, an ext4, `FIFREEZE`, ONE
            // lockfs-carrying suspend/reload/resume, `FITHAW`) reports
            // `one_lockfs_pair_on_frozen_fs=leaks` on the CI kernel. So
            // the swing's leak was never "one of four pairs did not come
            // back" — EVERY pair leaks once the superblock is already
            // frozen, and this site inherits that whenever it runs after
            // the swing's freeze outlived its process.
            //
            // The fix is NOT simply the frozen-window form, because this
            // branch also runs with the filesystem un-frozen — a crash
            // BEFORE the window leaves `origin_b` present on a live fs —
            // and there dm's lockfs is doing the quiescing it exists for.
            // Suppressing it unconditionally would retarget a live
            // filesystem with nothing holding its writes still.
            //
            // So the sweep takes the freeze itself, and the retarget
            // suppresses dm's. Note what this is and is not: it does not
            // ADD a freeze — dm was already taking one on every reload —
            // it moves that freeze onto a counter this code can actually
            // release. `FIFREEZE` answering `EBUSY` is the "already
            // frozen" case (measured), which is precisely the state this
            // sweep documents itself as recovering from; the freeze is
            // then the dead process's, and the `thaw_fs?` below closes it
            // exactly as it always has.
            //
            // Thawing FIRST would be the cheap way to reach a known
            // state, and it is forbidden: thawing while the shim still
            // points at B hands guest writes to the pool this sweep is
            // about to destroy.
            //
            // The outcome decides TWO independent things, and collapsing
            // them into one is what the first version of this got wrong:
            //
            //   suppress — must dm's implicit lockfs be suppressed on the
            //              retargets below?
            //   close    — is there a frozen window that this sweep has to
            //              close afterwards?
            let (suppress, close) = match self.ops.freeze_fs(&self.cfg.mountpoint) {
                // Ours. The window is open and it is this sweep's to close.
                Ok(()) => (true, true),
                // The window was already open — the dead process's, which
                // is the state this branch exists to recover from. Still
                // frozen, so dm's lockfs still leaks against it, and the
                // thaw below closes it exactly as it always has.
                Err(StackError::AlreadyFrozen(_)) => (true, true),
                // Shut down. No writes to quiesce, so suppressing costs
                // nothing — and nothing is frozen, so there is no window.
                //
                // Do NOT thaw here. `thaw_fs` opens the mountpoint before
                // it does anything else, and on the shape that produces
                // this errno from a shutdown rather than an abort — XFS
                // after `xfs_io -c shutdown` — that open is ITSELF `EIO`.
                // An unconditional thaw would fail the repair it had just
                // completed. (ext4-aborted survives it: the mountpoint
                // opens and `FITHAW` answers the tolerated `EINVAL`. The
                // runbook names both filesystems, so tolerating one is
                // not tolerating the case.)
                Err(StackError::Wedged(_)) => (true, false),
                // No freeze op on this superblock — and dm's `lock_fs`
                // does not need one. It calls `freeze_super` directly
                // rather than going through `ioctl_fsfreeze`, whose
                // `s_op->freeze_fs == NULL && s_op->freeze_super == NULL`
                // check is the only thing that produces `EOPNOTSUPP`. So
                // dm's lockfs WOULD have quiesced here, and suppressing
                // it would remove quiescing that was really happening.
                // Take the ordinary reload, which balances its own
                // freeze. Unreachable in practice — every block
                // filesystem that can sit under dm has a freeze op — but
                // this is the sentence the next person reasons from.
                Err(StackError::Unsupported(_)) => (false, false),
                Err(e) => return Err(e),
            };
            let retarget = |name: &str| -> Result<(), StackError> {
                if suppress {
                    self.ops.reload_device_in_frozen_window(name, &table)
                } else {
                    self.ops.reload_device(name, &table)
                }
            };
            // No `?` between the freeze and the thaw. An early return
            // here leaves the GUEST's filesystem frozen, and a retarget
            // that fails is usually a persistent condition — every retry
            // would re-enter, take `AlreadyFrozen`, fail at the same
            // place and leave it frozen again. A repair that cannot
            // complete must not also stop the workload. Same shape as
            // the swing's frozen section, for the same reason.
            let work = (|| -> Result<(), StackError> {
                retarget(&slot.shim_name)?;
                mask_guest_discard_or_warn(&self.ops, &slot.shim_name);
                if let Some(port) = self.cfg.discard_name.as_deref() {
                    retarget(port)?;
                }
                Ok(())
            })();
            // Thawing an un-frozen filesystem is `Ok` (the ordinary
            // case — a clean pre-swing sweep), so anything this reports
            // is a thaw that genuinely refused. Propagate it: the
            // removals below need the writes it releases to drain, and
            // a swallowed failure reports a permanently frozen mount as
            // a successful repair. When both fail the retarget's error
            // wins, because it is the cause.
            let thawed = if close {
                self.ops.thaw_fs(&self.cfg.mountpoint)
            } else {
                Ok(())
            };
            work?;
            thawed?;
            found = true;
        }
        for d in self.ops.list_devices()? {
            if d.name.starts_with(&self.barrier_prefix()) {
                let id = self.dev_id_of(&d.name)?;
                self.ops.remove_device(&d.name)?;
                self.pool_message(&format!("delete {id}"))?;
                found = true;
            }
        }
        for n in [&names.origin_b, &names.pool_b, &names.bda, &names.btm] {
            found |= self.remove_if_exists(n)?;
        }
        let bpath = slot_b_path(&slot.live_file);
        if let Some(l) = self.ops.backing_of(&bpath)? {
            self.ops.release_backing(&l)?;
            found = true;
        }
        if self.ops.backing_exists(&bpath)? {
            self.ops.delete_backing(&bpath)?;
            found = true;
        }
        Ok(found)
    }

    /// Post-flip: slot A is garbage; finish the swing's teardown and
    /// canonicalize B. Idempotent across every partial state — after a
    /// reboot dm is empty and only the file swap remains; same-process,
    /// the surviving `-b` names say which canonical devices are still A.
    fn sweep_flipped_inner(
        &self,
        slot: &SlotConfig,
        names: &SlotNames,
    ) -> Result<bool, StackError> {
        let mut found = false;
        // Barriers are unconditionally garbage; pool A may already be
        // gone, so the thin-id delete is best-effort (it dies with the
        // pool anyway).
        for d in self.ops.list_devices()? {
            if d.name.starts_with(&self.barrier_prefix()) {
                let id = self.dev_id_of(&d.name)?;
                self.ops.remove_device(&d.name)?;
                let _ = self.pool_message(&format!("delete {id}"));
                found = true;
            }
        }
        // While the B set still wears `-b` names, canonically named
        // devices are the A side — remove them wholesale (consumers
        // before components).
        let b_alive = [&names.origin_b, &names.pool_b, &names.btm, &names.bda]
            .iter()
            .map(|n| self.device_exists(n))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .any(|x| x);
        if b_alive {
            found |= self.remove_role_devices()?;
            found |= self.remove_if_exists(&self.cfg.origin_name)?;
            found |= self.remove_if_exists(&self.cfg.pool_name)?;
            found |= self.remove_if_exists(&names.da)?;
            found |= self.remove_if_exists(&names.tm)?;
        }
        // File swap, discriminated by the B file's existence: once the
        // rename happened, the live file IS the B file — never touch it.
        let bpath = slot_b_path(&slot.live_file);
        if self.ops.backing_exists(&bpath)? {
            if let Some(l) = self.ops.backing_of(&slot.live_file)? {
                self.ops.release_backing(&l)?;
            }
            if self.ops.backing_exists(&slot.live_file)? {
                self.ops.delete_backing(&slot.live_file)?;
            }
            self.ops.rename_backing(&bpath, &slot.live_file)?;
            found = true;
        }
        for (from, to) in [
            (&names.origin_b, &self.cfg.origin_name),
            (&names.pool_b, &self.cfg.pool_name),
            (&names.btm, &names.tm),
            (&names.bda, &names.da),
        ] {
            if self.device_exists(from)? {
                self.ops.rename_device(from, to)?;
                found = true;
            }
        }
        if self.ops.backing_exists(&slot.live_file)? {
            if let Err(e) = self.ops.ensure_preallocated(&slot.live_file) {
                eprintln!(
                    "skynbd-agent: slot preallocation of {} failed during sweep ({e})",
                    slot.live_file.display()
                );
            }
        }
        // Invariant B: this arm is reached only because a swing's
        // retarget is durable, and the retarget that put the shim on B
        // recomputed its queue limits. Whether the mask was re-asserted
        // afterwards is exactly what the crash lost, so re-derive it
        // here rather than trust the pre-crash attempt.
        mask_guest_discard_or_warn(&self.ops, &slot.shim_name);
        Ok(found)
    }

    /// Lazy-timing eviction (§7.1): one freeze window, at most
    /// `max_ranges` (≤ [`RECLAIM_BATCH_RANGES`]) still-shared ranges,
    /// discarded on BOTH sides — the live origin through the port, and
    /// the retained snapshot through a transient RW activation of its
    /// thin dev id. The dual discard frees the data blocks (last-ref
    /// drop) and keeps every future `thin_delta(S, S′)` honest: evicted
    /// ranges vanish from both sides, so no delta ever spans an
    /// eviction. Caller contract is [`Self::reclaim`]'s (served-origin
    /// fenced first) — every eviction event is a reclaim and carries the
    /// same obligations in its own window.
    pub fn evict(&self, snapshot: &str, max_ranges: usize) -> Result<u64, StackError> {
        let _g = self.lock.lock().unwrap();
        let (s_name, _) = self.find_device(snapshot)?;
        let s_id = self.dev_id_of(&s_name)?;
        self.watchdog
            .arm(std::time::Duration::from_millis(self.cfg.freeze_budget_ms));
        self.ops.freeze_fs(&self.cfg.mountpoint).inspect_err(|_| {
            self.watchdog.disarm();
        })?;
        let work = (|| -> Result<u64, StackError> {
            let (snap, live) = self.mappings_pair(s_id, self.cfg.origin_dev_id)?;
            let shared = shared_mappings(&snap, &live, self.cfg.chunk_bytes);
            let take = shared.len().min(max_ranges).min(RECLAIM_BATCH_RANGES);
            if take == 0 {
                return Ok(0);
            }
            // Two activations of one thin dev id are not sanctioned:
            // drop S's RO device, stand up a transient RW thin over the
            // same id for the snapshot-side discards, restore after.
            self.ops.remove_device(&s_name)?;
            let evict_name = format!("sky-{}-evict", self.cfg.volume);
            self.ops.create_device(
                &evict_name,
                &format!("SKYNBD-{}-evict-{s_id}", self.cfg.volume),
                &[(
                    0,
                    self.cfg.size_bytes / SECTOR,
                    "thin".into(),
                    format!("{} {s_id}", self.mm(&self.cfg.pool_name)?),
                )],
            )?;
            let inner = (|| -> Result<u64, StackError> {
                let port = self
                    .cfg
                    .discard_name
                    .as_deref()
                    .unwrap_or(&self.cfg.origin_name);
                let mut bytes = 0u64;
                for &(off, len) in &shared[..take] {
                    if self.watchdog.fired() {
                        return Err(StackError::Fault("evict aborted: freeze watchdog fired"));
                    }
                    // Snapshot side FIRST — the order is a correctness
                    // rule, not a preference. A failure (or death)
                    // between the two discards leaves the range mapped
                    // on exactly one side, and only one asymmetry is
                    // safe: mapped-in-S-only becomes a `Clear` in the
                    // next `thin_delta(S, S′)`, which publishes
                    // `write_zeroes` over an LBA the filesystem still
                    // owns and the origin still serves — silent
                    // corruption once the advance repoints onto that
                    // generation. Mapped-in-live-only is a `Data` range:
                    // the block re-uploads with its true content, and
                    // the leak is one interval of wasted space.
                    self.ops.discard_dev(&evict_name, off, len)?;
                    self.ops.discard_dev(port, off, len)?;
                    bytes += len;
                }
                Ok(bytes)
            })();
            // Restore S's RO activation exactly as found, even on error.
            let rem = self.ops.remove_device(&evict_name);
            let act = self.activate_ro(&s_name, s_id);
            let bytes = inner?;
            rem?;
            act?;
            Ok(bytes)
        })();
        let _ = self.ops.thaw_fs(&self.cfg.mountpoint); // always thaw
        let outcome = self.watchdog.disarm();
        let bytes = work?;
        if outcome == FreezeOutcome::ForceThawed {
            return Err(StackError::Fault("evict aborted: freeze watchdog fired"));
        }
        Ok(bytes)
    }

    /// The freeze-INDEPENDENT core of a capture: suspend the origin,
    /// `create_snap`, resume it. Returns `(created dev id, suspend
    /// result, resume result)` so the caller can order the errors —
    /// and, more to the point, so the caller decides whether a freeze
    /// wrapped it at all (§8.2 step 3).
    ///
    /// Nothing here touches the watchdog: the watchdog bounds a freeze
    /// window, and this is precisely the part that is not one.
    fn snap_core(
        &self,
        mut dev_id: u64,
    ) -> (
        Result<u64, StackError>,
        Result<(), StackError>,
        Result<(), StackError>,
    ) {
        // Kernel dm-thin contract: the active origin must be SUSPENDED
        // before create_snap (flushes in-flight I/O; newer kernels reject
        // the wrong order with EINVAL) — then resumed. LVM does exactly
        // this dance; on raw dm it is ours.
        let sus = self.ops.suspend_device(&self.cfg.origin_name);
        let created = if sus.is_ok() {
            let start = dev_id;
            loop {
                match self.pool_message(&format!("create_snap {dev_id} {}", self.cfg.origin_dev_id))
                {
                    Ok(()) => break Ok(dev_id),
                    // Retry a bounded window past ids the pool already has
                    // (stale sidecar after a crash is self-healing).
                    Err(_) if dev_id < start + 64 => {
                        dev_id += 1;
                        continue;
                    }
                    Err(e) => break Err(e),
                }
            }
        } else {
            Err(StackError::Fault("origin suspend failed"))
        };
        let res = self.ops.resume_device(&self.cfg.origin_name); // always resume
        (created, sus, res)
    }
}

impl<O: DmOps> LocalStack for DmStack<O> {
    /// V3.2 §9.3 capture with raw dm-thin: freeze fs → suspend origin →
    /// `create_snap` → resume origin → thaw → activate snapshot RO.
    ///
    /// One documented exception to the freeze: a filesystem the kernel
    /// reports as [`StackError::Wedged`] is captured crash-consistently,
    /// with no freeze and no thaw (§8.2 step 3).
    fn capture(&self, name: &str) -> Result<LocalSnap, StackError> {
        let _g = self.lock.lock().unwrap();

        // Allocate a dev id, retrying past ids the pool already knows.
        let dev_id = self.next_dev_id();

        self.watchdog
            .arm(std::time::Duration::from_millis(self.cfg.freeze_budget_ms));
        let dev_id = match self.ops.freeze_fs(&self.cfg.mountpoint) {
            Ok(()) => {
                let (id, sus, res) = self.snap_core(dev_id);
                let _ = self.ops.thaw_fs(&self.cfg.mountpoint); // always thaw
                let outcome = self.watchdog.disarm();
                let dev_id = id?;
                sus?;
                res?;
                self.persist_next_dev_id(dev_id + 1);
                if outcome == FreezeOutcome::ForceThawed {
                    // The window blew the budget: the snapshot may span a
                    // thaw — delete it and fail the capture (V3.2 §9.3
                    // failure rules).
                    let _ = self.pool_message(&format!("delete {dev_id}"));
                    return Err(StackError::Fault("freeze watchdog fired"));
                }
                dev_id
            }
            // §8.2 step 3: a filesystem that has shut down cannot be
            // frozen and needs no freeze — its block state already IS
            // "crashed at ENOSPC", and a crash-consistent capture of
            // that is legitimate. Refusing here would not protect
            // anything; it would only strand the delta on a node that
            // just proved it cannot hold it, which is the exact loss the
            // extended-outage runbook exists to prevent. The staged
            // read-only mount already assumes a dirty journal is
            // possible (`ro_mount_opts`: ext4 `noload`, xfs
            // `norecovery`), so nothing downstream has to change.
            //
            // Deliberately NOT extended to the other freeze brackets
            // (`reclaim_window`, `evict`, `swing`). Those are optional
            // work and SHOULD fail cleanly on a wedged filesystem;
            // capture is the only one that must proceed, because
            // capture is how the delta escapes.
            Err(StackError::Wedged(why)) => {
                // Disarm FIRST. There is no freeze to bound, and an
                // armed watchdog would fire a force-thaw against a
                // filesystem nobody froze.
                self.watchdog.disarm();
                eprintln!(
                    "skynbd-agent: {}: capturing WITHOUT a freeze — {why}. The snapshot is \
                     crash-consistent, not filesystem-consistent (§8.2 step 3).",
                    self.cfg.origin_name,
                );
                let (id, sus, res) = self.snap_core(dev_id);
                let dev_id = id?;
                sus?;
                res?;
                self.persist_next_dev_id(dev_id + 1);
                dev_id
            }
            Err(e) => {
                self.watchdog.disarm();
                return Err(e);
            }
        };

        let dm_name = self.dm_name(Some(SnapRole::Candidate), name);
        // A CANDIDATE activation for this target may already exist,
        // left by an earlier cycle that captured and then failed before
        // publishing. Under §8.2 that is not an edge case, it is the
        // ORDINARY outcome of an outage: the publish is the call that
        // hangs, so every cut cycle dies with its candidate activated.
        //
        // The name is a pure function of the target, so `activate_ro`
        // below answers EBUSY — and answers it IDENTICALLY forever. The
        // recovery cycle can never complete, which is the one cycle the
        // whole runbook is waiting on. Worse, each attempt has already
        // taken a fresh thin snapshot by the time it gets here, so the
        // livelock leaks a snapshot per interval into a pool that is
        // under pressure by construction. Measured in S15j, once the
        // link healed: `create sky-<vol>-c-<target>: EBUSY` every 5 s,
        // with the dev id climbing 48, 49, 50, … and the name fixed.
        //
        // Supersede it. The role is the safety argument: `promote`
        // renames `-c-` to `-b-` via `set_role(.., Baseline)`, so a
        // device still carrying `-c-` has not been promoted, and the
        // capture that just succeeded replaces it.
        //
        // The role is not quite the same as the INSTANT, and the
        // difference is worth naming. Publish succeeds server-side
        // before promote renames, so a death in that window leaves a
        // `-c-` device whose generation IS published. Nothing durable
        // is at risk — the generation is in the store — but superseding
        // it drops the local baseline, and the next cycle then
        // full-objectizes instead of sending a delta. Recoverable and
        // cheap, and not the same thing as "refers to a snapshot no one
        // can reference".
        //
        // Delete the thin id too, or the leak survives the fix that
        // stops it.
        if self.device_exists(&dm_name)? {
            let stale = match self.dev_id_of(&dm_name) {
                Ok(id) => Some(id),
                Err(e) => {
                    // Two failures hide behind one `.ok()`, and only one
                    // of them is benign. "Already reclaimed by a sweep"
                    // is expected; an activation with no parseable
                    // dev-id UUID means the thin id is about to be
                    // orphaned — precisely the leak the `delete` below
                    // exists to prevent — so say so rather than
                    // discarding it silently.
                    eprintln!(
                        "skynbd-agent: superseding candidate {dm_name}, whose dev id could \
                         not be resolved ({e}); the activation is removed but its thin id \
                         cannot be deleted here, so it stays allocated in the pool"
                    );
                    None
                }
            };
            self.ops.remove_device(&dm_name)?;
            if let Some(id) = stale {
                // Best effort: the activation is gone either way, so the
                // capture must not fail because a thin id was already
                // reclaimed by a previous sweep.
                let _ = self.pool_message(&format!("delete {id}"));
            }
        }
        self.activate_ro(&dm_name, dev_id)?;
        Ok(LocalSnap {
            lv_name: name.to_string(),
            thin_dev_id: dev_id,
            pool_tx_id: self.pool_tx_id().unwrap_or(0),
            created_at_ms: 0, // stamped by the caller's clock policy
        })
    }

    fn degrade_read_only(&self) -> Result<(), StackError> {
        let _g = self.lock.lock().unwrap();
        self.ops.remount_ro(&self.cfg.mountpoint)
    }

    fn restore_read_write(&self) -> Result<(), StackError> {
        let _g = self.lock.lock().unwrap();
        self.ops.remount_rw(&self.cfg.mountpoint)
    }

    fn mount_is_read_only(&self) -> Option<bool> {
        self.ops.mount_is_read_only(&self.cfg.mountpoint).ok()
    }

    fn mount_is_writable(&self) -> Option<bool> {
        self.ops.mount_is_writable(&self.cfg.mountpoint).ok()
    }

    fn snap_delete(&self, name: &str) -> Result<(), StackError> {
        let _g = self.lock.lock().unwrap();
        let (dm_name, _) = self.find_device(name)?;
        let dev_id = self.dev_id_of(&dm_name)?;
        self.ops.remove_device(&dm_name)?;
        self.pool_message(&format!("delete {dev_id}"))
    }

    fn set_role(&self, name: &str, role: Option<SnapRole>) -> Result<(), StackError> {
        let _g = self.lock.lock().unwrap();
        let (dm_name, current) = self.find_device(name)?;
        if role == current {
            return Ok(());
        }
        // Untag included: `u-` is a real rename. §15.4's untag-old-before-
        // tag-new only protects a recovering observer if the old baseline
        // actually stops LOOKING like a baseline — the DmStack crash matrix
        // quarantined on two `b-` devices when this was a no-op.
        let to = self.dm_name(role, name);
        self.ops.rename_device(&dm_name, &to)
    }

    fn observe(&self) -> LocalObservation {
        let mut obs = LocalObservation::default();
        let Ok(devs) = self.ops.list_devices() else {
            return obs;
        };
        for d in devs {
            match self.parse_name(&d.name) {
                Some((Some(SnapRole::Baseline), l)) => obs.baselines.push(l.to_string()),
                Some((Some(SnapRole::Candidate), l)) => obs.candidates.push(l.to_string()),
                // `u-` devices are mid-promotion garbage en route to
                // deletion — invisible to the planner by design.
                Some((None, _)) | None => {}
            }
        }
        obs
    }

    fn thin_delta(&self, older: &str, newer: &str) -> Result<Vec<DeltaRange>, StackError> {
        let (b_name, _) = self.find_device(older)?;
        let (s_name, _) = self.find_device(newer)?;
        let (b_id, s_id) = (self.dev_id_of(&b_name)?, self.dev_id_of(&s_name)?);

        self.pool_message("reserve_metadata_snap")?;
        let result = (|| {
            let left = self.ops.thin_mappings(&self.cfg.metadata_dev, b_id)?;
            let right = self.ops.thin_mappings(&self.cfg.metadata_dev, s_id)?;
            Ok(diff_mappings(&left, &right, self.cfg.chunk_bytes))
        })();
        // Release unconditionally — a leaked reservation blocks every
        // future delta on the pool (§7 startup hygiene).
        let rel = self.pool_message("release_metadata_snap");
        let ranges = result?;
        rel?;
        Ok(ranges)
    }

    fn full_mapped(&self, name: &str) -> Result<Vec<DeltaRange>, StackError> {
        let (dm_name, _) = self.find_device(name)?;
        let id = self.dev_id_of(&dm_name)?;
        self.pool_message("reserve_metadata_snap")?;
        let result = self.ops.thin_mappings(&self.cfg.metadata_dev, id);
        let rel = self.pool_message("release_metadata_snap");
        let maps = result?;
        rel?;
        Ok(maps
            .iter()
            .map(|&(thin_begin, _, len)| DeltaRange::Data {
                off: thin_begin * self.cfg.chunk_bytes,
                len: len * self.cfg.chunk_bytes,
            })
            .collect())
    }

    fn read(&self, name: &str, off: u64, len: u64) -> Result<Vec<u8>, StackError> {
        let (dm_name, _) = self.find_device(name)?;
        self.ops.read_dev(&dm_name, off, len)
    }

    fn volume_size(&self) -> u64 {
        self.cfg.size_bytes
    }

    fn chunk_size(&self) -> u64 {
        self.cfg.chunk_bytes
    }

    fn reclaim(&self, snapshot: &str) -> Result<u64, StackError> {
        DmStack::reclaim(self, snapshot)
    }

    fn enumerate(&self, snapshot: &str) -> Result<ReclaimSets, StackError> {
        DmStack::enumerate(self, snapshot)
    }

    /// Swing-capable iff the stack owns its pool slot (§7): the cached
    /// writeback stack must never grow a second pool over the shared
    /// head.
    fn swing_supported(&self) -> bool {
        self.cfg.slot.is_some()
    }

    fn headroom(&self) -> Option<(u64, u64)> {
        DmStack::headroom(self)
    }

    fn swing(&self, snapshot: &str, hooks: &mut SwingHooks<'_>) -> Result<SwingStats, StackError> {
        DmStack::swing(self, snapshot, hooks)
    }

    fn sweep_slot(&self, flipped: bool) -> Result<bool, StackError> {
        DmStack::sweep_slot(self, flipped)
    }

    fn evict(&self, snapshot: &str, max_ranges: usize) -> Result<u64, StackError> {
        DmStack::evict(self, snapshot, max_ranges)
    }

    fn pool_usage(&self) -> Option<(u64, u64)> {
        // thin-pool status params: `<tx> <meta used/total> <data used/total> …`
        let params = self.ops.pool_status_params(&self.cfg.pool_name).ok()?;
        let data = params.split_whitespace().nth(2)?;
        let (used, total) = data.split_once('/')?;
        Some((used.parse().ok()?, total.parse().ok()?))
    }

    fn pressure(&self) -> Option<Pressure> {
        DmStack::pool_pressure(self)
    }

    fn assert_guest_discard_mask(&self) -> Result<(), StackError> {
        DmStack::assert_guest_discard_mask(self)
    }
}

/// Release a stale metadata-snap reservation (startup hygiene, §7).
pub fn release_stale_metadata_snap<O: DmOps>(stack: &DmStack<O>) -> bool {
    stack.pool_message("release_metadata_snap").is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: u64 = 65536;

    /// The one-shot reports are keyed per shim and per backing file,
    /// never per process. Both conditions are permanent and both call
    /// sites run once per cycle per volume, so a process-global `Once`
    /// names whichever volume tripped first and then says nothing about
    /// any other one on the node — the silence the messages exist to
    /// break, aimed at every volume but the first.
    #[test]
    fn the_one_shot_reports_are_keyed_per_volume() {
        assert!(first_sighting("test:/dev/mapper/sky-a-fs"));
        assert!(
            first_sighting("test:/dev/mapper/sky-b-fs"),
            "a second volume must still be named"
        );
        assert!(!first_sighting("test:/dev/mapper/sky-a-fs"));
        assert!(!first_sighting("test:/dev/mapper/sky-b-fs"));
    }

    #[test]
    fn diff_semantics() {
        // left: blocks 0..2 (data 10..12), 5..6 (data 20)
        // right: 0..1 same (10), 1..2 differ (99), 3..4 new (30)
        // → differ block 1, clear block 5, data block 3
        let left = [(0, 10, 2), (5, 20, 1)];
        let right = [(0, 10, 1), (1, 99, 1), (3, 30, 1)];
        let d = diff_mappings(&left, &right, B);
        assert_eq!(
            d,
            vec![
                DeltaRange::Data { off: B, len: B },
                DeltaRange::Data { off: 3 * B, len: B },
                DeltaRange::Clear { off: 5 * B, len: B },
            ]
        );
    }

    #[test]
    fn shared_semantics() {
        // left (snapshot S): blocks 0..2 → data 10..12, 5..6 → data 20
        // right (origin): 0..1 same (10), 1..2 rewritten (99), 3..4 new
        // (30), 5..6 same (20) → shared = block 0 and block 5.
        let left = [(0, 10, 2), (5, 20, 1)];
        let right = [(0, 10, 1), (1, 99, 1), (3, 30, 1), (5, 20, 1)];
        assert_eq!(shared_mappings(&left, &right, B), vec![(0, B), (5 * B, B)]);
    }

    #[test]
    fn shared_handles_partial_overlaps_and_coalesces() {
        // One long left run vs a right side that shares its middle: left
        // 0..8 → data 10..18; right 2..5 same shift (12..15), 5..8
        // rewritten (50..53). Shared = blocks 2..5, coalesced.
        let d = shared_mappings(&[(0, 10, 8)], &[(2, 12, 3), (5, 50, 3)], B);
        assert_eq!(d, vec![(2 * B, 3 * B)]);
        // Identical mappings → everything shared, one coalesced run.
        let d = shared_mappings(&[(0, 7, 4), (4, 11, 2)], &[(0, 7, 4), (4, 11, 2)], B);
        assert_eq!(d, vec![(0, 6 * B)]);
        // Same LBAs, different data blocks → nothing shared.
        assert!(shared_mappings(&[(0, 1, 4)], &[(0, 9, 4)], B).is_empty());
        // Disjoint LBAs → nothing shared.
        assert!(shared_mappings(&[(0, 1, 2)], &[(4, 1, 2)], B).is_empty());
    }

    #[test]
    fn dirty_semantics() {
        // left (snapshot S): blocks 0..2 → data 10..12, 5..6 → data 20
        // right (origin): 0..1 same (10), 1..2 rewritten (99), 3..4 new
        // (30), 5..6 same (20) → dirty = rewritten block 1 + new block 3.
        let left = [(0, 10, 2), (5, 20, 1)];
        let right = [(0, 10, 1), (1, 99, 1), (3, 30, 1), (5, 20, 1)];
        assert_eq!(dirty_mappings(&left, &right, B), vec![(B, B), (3 * B, B)]);
    }

    #[test]
    fn dirty_handles_partial_overlaps_and_coalesces() {
        // One long left run vs a right side that shares its middle: left
        // 0..8 → data 10..18; right 2..5 same shift (12..15), 5..8
        // rewritten (50..53). Dirty = the rewritten 5..8, coalesced.
        let d = dirty_mappings(&[(0, 10, 8)], &[(2, 12, 3), (5, 50, 3)], B);
        assert_eq!(d, vec![(5 * B, 3 * B)]);
        // Right-only runs (newly provisioned) are dirty in full, and
        // adjacent dirty runs coalesce across data-block seams.
        let d = dirty_mappings(&[], &[(0, 7, 2), (2, 30, 1)], B);
        assert_eq!(d, vec![(0, 3 * B)]);
        // Same mappings → nothing dirty (left-only runs are NOT dirty:
        // an unprovisioned B block reads through the same origin).
        assert!(dirty_mappings(&[(0, 7, 4)], &[(0, 7, 4)], B).is_empty());
        assert!(dirty_mappings(&[(0, 7, 4)], &[(2, 9, 2)], B).is_empty());
        // Same LBAs, different data blocks → everything dirty.
        assert_eq!(
            dirty_mappings(&[(0, 1, 4)], &[(0, 9, 4)], B),
            vec![(0, 4 * B)]
        );
    }

    /// `shared ⊎ dirty` partitions `right` exactly: every right-mapped
    /// block is in exactly one of the two sets.
    #[test]
    fn dirty_and_shared_partition_the_right_side() {
        let left = [(0, 10, 4), (8, 40, 2)];
        let right = [(0, 10, 2), (2, 90, 2), (5, 30, 1), (8, 40, 1), (9, 91, 1)];
        let shared = shared_mappings(&left, &right, B);
        let dirty = dirty_mappings(&left, &right, B);
        let total: u64 = shared.iter().chain(dirty.iter()).map(|&(_, l)| l).sum();
        assert_eq!(total, 7 * B, "every right block accounted for once");
        assert_eq!(shared, vec![(0, 2 * B), (8 * B, B)]);
        assert_eq!(dirty, vec![(2 * B, 2 * B), (5 * B, B), (9 * B, B)]);
    }

    #[test]
    fn diff_coalesces_and_splits_partial_overlaps() {
        // left run 0..4 (data 10..14); right run 2..6 (data 12..16):
        // 0..2 left-only → clear; 2..4 same data → skip; 4..6 right-only → data.
        let d = diff_mappings(&[(0, 10, 4)], &[(2, 12, 4)], B);
        assert_eq!(
            d,
            vec![
                DeltaRange::Clear { off: 0, len: 2 * B },
                DeltaRange::Data {
                    off: 4 * B,
                    len: 2 * B
                },
            ]
        );
        // Identical → empty delta.
        assert!(diff_mappings(&[(0, 7, 8)], &[(0, 7, 8)], B).is_empty());
        // Adjacent differ runs coalesce.
        let d = diff_mappings(&[(0, 1, 1), (1, 2, 1)], &[(0, 8, 1), (1, 9, 1)], B);
        assert_eq!(d, vec![DeltaRange::Data { off: 0, len: 2 * B }]);
    }
}

//! `LocalStack`: the kernel/LVM boundary (agent design doc §3.3).
//!
//! Everything the agent does to the local thin pool goes through this
//! trait. `FakeStack` is a deterministic in-memory thin-pool model — the
//! keystone that lets the cycle engine and the V3.2 §21.1 crash matrix run
//! as ordinary `cargo test` with no root. The real `LvmStack` (lvm2 CLI +
//! linked `thinp` deltas) arrives in a later M8 slice and is exercised only
//! in Tier B.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::planner::LocalObservation;
use crate::state::LocalSnap;

/// Role tag carried by a snapshot LV (`--addtag` in the real stack). This is
/// what makes local state reconstructible from observation alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapRole {
    Baseline,
    Candidate,
}

/// One range of the delta between two snapshots, in bytes, chunk-aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaRange {
    /// Present/different in the newer snapshot: objectize these bytes.
    Data { off: u64, len: u64 },
    /// Mapped in the older snapshot only (discarded since): clear.
    Clear { off: u64, len: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum StackError {
    #[error("snapshot not found: {0}")]
    NotFound(String),
    #[error("stack fault: {0}")]
    Fault(&'static str),
    /// The kernel does not offer this surface at all (ENOENT on a sysfs
    /// knob the running kernel lacks) — distinct from "the operation
    /// failed", because a capability the kernel never had cannot be a
    /// deployment fault. Task 15j uses it for the guest-discard mask
    /// (§8.1 / Invariant B): a missing `queue/discard_max_bytes` is
    /// tolerated and logged, every other failure is an error.
    #[error("unsupported by this kernel: {0}")]
    Unsupported(String),
    /// The filesystem cannot be frozen because it is no longer running:
    /// an ext4 in `emergency_ro` (which still reports `rw` in
    /// `/proc/mounts`, and answers `EROFS`) or a shut-down XFS (whose
    /// mountpoint will not even `open`, answering `EIO`). Distinct
    /// from "the freeze failed" (EBUSY: someone else holds the freeze,
    /// and that is transient) because a shut-down filesystem NEEDS no
    /// freeze: its block state already is "crashed at ENOSPC", and a
    /// crash-consistent capture of that is legitimate (§8.2 step 3).
    ///
    /// NOT a cleanly read-only super — that one freezes successfully
    /// (measured), so §8.1 rule 3's own remount never produces this.
    #[error("filesystem is wedged, not freezable: {0}")]
    Wedged(String),
    /// `FIFREEZE` answered `EBUSY`: the superblock is ALREADY frozen.
    ///
    /// Distinct from a failed freeze, and the distinction is what lets a
    /// caller that needs the filesystem quiesced tell "someone beat me to
    /// it, the window is already open" from "I could not open one". It is
    /// emphatically NOT [`StackError::Wedged`] — a contended freeze is
    /// transient and a capture must still fail on it rather than take the
    /// crash-consistent path (§8.2 step 3), which is why this is its own
    /// variant instead of widening that one.
    ///
    /// Measured, on loop-backed ext4: `FIFREEZE` on an already-frozen
    /// filesystem answers `EBUSY`, and `FITHAW` on an un-frozen one
    /// answers `EINVAL`.
    #[error("filesystem is already frozen: {0}")]
    AlreadyFrozen(String),
}

/// One metadata-snapshot dump's worth of reclaim enumeration (design doc
/// §7): both executors consume the same two sets. Advisory — computed
/// without a freeze; the executors re-verify under their own windows
/// (concurrent writes only move ranges from `shared` to `dirty`).
#[derive(Debug, Clone, Default)]
pub struct ReclaimSets {
    /// `mapped(S) − dirtied-since(S)` as coalesced byte ranges — the
    /// reclaim set (R = its length).
    pub shared: Vec<(u64, u64)>,
    /// `dirtied-since(S)` plus newly-provisioned — what a swing copies
    /// (D = its byte total).
    pub dirty: Vec<(u64, u64)>,
}

impl ReclaimSets {
    /// R — the range count the §7 threshold compares against.
    pub fn shared_count(&self) -> usize {
        self.shared.len()
    }

    /// D — the byte total the §7 headroom guard compares against.
    pub fn dirty_bytes(&self) -> u64 {
        self.dirty.iter().map(|&(_, len)| len).sum()
    }
}

/// What a completed swing cost (the S15b timing-table inputs).
#[derive(Debug, Clone, Copy, Default)]
pub struct SwingStats {
    /// Live copy rounds run behind snapshot barriers.
    pub rounds: u32,
    pub copied_bytes: u64,
    /// Wall time of the live (unfrozen) copy phase.
    pub live_ns: u64,
    /// Wall time of the frozen top-up/retarget/flip window.
    pub frozen_ns: u64,
}

/// The exhaustion state of one pool across BOTH surfaces §8.1 rule 2
/// names: the thin pool's own occupancy, and the filesystem the pool's
/// backing file lives on — which can exhaust *underneath* a pool that
/// believes it has free blocks, surfacing as loop-device EIO that
/// bypasses the whole queue/timeout ladder.
///
/// The second surface binds only when the backing is **sparse**. A
/// fully preallocated backing has its space reserved already, so a low
/// `fs_free` cannot threaten a pool write and counting it would advance
/// spuriously; a sparse backing can lose the race. The sparse arm is
/// deliberately conservative — it ignores space already allocated to
/// the file but not yet used by the pool — so *within one volume's
/// view* the measure is exact in the preallocated case and pessimistic
/// in the sparse one, and every rounding there is toward advancing
/// earlier.
///
/// That claim is scoped to one volume on purpose. Two honest ways it
/// reads OPTIMISTIC — i.e. under-reserves — both of which the measure
/// cannot see and neither of which it pretends to:
///
/// 1. **`fs_free` is whole-filesystem, and every volume claims all of
///    it.** N sparse backings on one node filesystem each read the same
///    free bytes as if they were theirs alone; nothing subtracts a
///    sibling's unallocated-but-owed extent. The aggregate is therefore
///    optimistic by up to (N−1) volumes' worth, precisely when the
///    shared filesystem is what exhausts. Closing it needs cross-volume
///    accounting — a node-level registry of every managed backing's
///    (logical − allocated) owed bytes, subtracted from `fs_free`
///    before each volume compares — which is a node-scoped structure
///    this per-stack measure has no access to and does not fake.
/// 2. **A compressing filesystem (btrfs, ZFS) can report
///    `allocated < logical` for a fully-written file**, because
///    `st_blocks` counts what was stored, not what was addressed. Such
///    a backing reads as sparse forever and arms the surface
///    permanently, so `fs_free` binds on a file whose space is in fact
///    committed. That direction is the safe one (an early advance, not
///    a late one), but the "exact in the preallocated case" half of the
///    claim above simply does not hold there. Detecting it reliably is
///    filesystem-specific; the honest response is to say so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pressure {
    pub used_bytes: u64,
    pub total_bytes: u64,
    /// Free bytes after taking the tighter of the two surfaces.
    pub effective_free: u64,
    /// Whether the filesystem surface bound (a sparse backing).
    pub fs_bound: bool,
}

impl Pressure {
    /// Pool occupancy as a fraction — what `advancePoolWatermark`
    /// compares against (§4). An unsized pool reads as empty.
    pub fn occupancy(&self) -> f64 {
        if self.total_bytes == 0 {
            return 0.0;
        }
        self.used_bytes as f64 / self.total_bytes as f64
    }
}

/// The engine's hook into a swing's freeze window. `persist_flip` writes
/// the durable slot flip — the §7 crash discriminator — and its call
/// site is normative: inside the window, after the retarget, before the
/// thaw (the state dir is off the frozen filesystem, the same property
/// capture relies on).
pub struct SwingHooks<'a> {
    pub persist_flip: &'a mut dyn FnMut() -> Result<(), StackError>,
    /// Set by the executor the instant the shim is aimed at slot B, and
    /// never cleared. It marks the point past which a failure is no
    /// longer a clean pre-flip abort: the live filesystem is on B while
    /// the durable record still names A, so the caller must NOT fall
    /// back to the port (its discards would land on the pool the record
    /// says is garbage). Stalling for recovery — which repairs the
    /// tables from the record — is the only safe response.
    pub retargeted: &'a mut bool,
}

/// The kernel/LVM boundary. Synchronous by design — these are blocking
/// device and CLI operations; the fake is pure memory.
pub trait LocalStack {
    /// fsfreeze → thin snapshot → thaw (V3.2 §9.3). Returns the snapshot's
    /// identity; the caller tags it via [`LocalStack::set_role`].
    fn capture(&self, name: &str) -> Result<LocalSnap, StackError>;
    fn snap_delete(&self, name: &str) -> Result<(), StackError>;
    /// Tag or untag a snapshot; `None` clears the role.
    fn set_role(&self, name: &str, role: Option<SnapRole>) -> Result<(), StackError>;
    /// What exists right now, by role tag — the planner's local input.
    fn observe(&self) -> LocalObservation;
    /// dm-thin delta between two snapshots (older, newer).
    fn thin_delta(&self, older: &str, newer: &str) -> Result<Vec<DeltaRange>, StackError>;
    /// All mapped ranges of one snapshot (bootstrap objectize set).
    fn full_mapped(&self, name: &str) -> Result<Vec<DeltaRange>, StackError>;
    /// Read from a snapshot device — never the origin (V3.2 §12.1).
    fn read(&self, name: &str, off: u64, len: u64) -> Result<Vec<u8>, StackError>;
    fn volume_size(&self) -> u64;
    fn chunk_size(&self) -> u64;
    /// D-M15 Invariant B reclaim: unmap origin blocks that `snapshot`
    /// maps and the origin has not rewritten since. Caller contract (§3
    /// step 6): the repoint to the generation published from `snapshot`
    /// is durable and fenced first, so every discarded block re-reads
    /// identically through the external origin. Returns bytes reclaimed.
    fn reclaim(&self, snapshot: &str) -> Result<u64, StackError>;
    /// Pool occupancy `(used, total)` in pool blocks, if the stack can
    /// answer — the `advancePoolWatermark` input (§4). `None` = unknown
    /// (never promotes a cycle).
    fn pool_usage(&self) -> Option<(u64, u64)>;
    /// The same occupancy plus §8.1 rule 2's second surface — the
    /// richer sibling of [`LocalStack::pool_usage`], and what the
    /// watermark (occupancy OR margin) reads. `None` = unknown, which
    /// never promotes a cycle.
    fn pressure(&self) -> Option<Pressure>;
    /// The §7 enumeration: one metadata dump yields both the reclaim set
    /// and the dirty set for `snapshot` vs the live origin — the inputs
    /// to [`crate::policy::decide`].
    fn enumerate(&self, snapshot: &str) -> Result<ReclaimSets, StackError>;
    /// Whether this stack can run the two-pool swing at all. `false` for
    /// writeback-cache stacks (a second pool over the shared head would
    /// be a second head attach — §7/§5) and any stack without a managed
    /// slot; the policy falls back to the port.
    fn swing_supported(&self) -> bool {
        false
    }
    /// `(fs_free, required_transient)` for the §7 headroom guard: free
    /// bytes on the slot filesystem, and the transient the swing needs
    /// beyond D (slot metadata plus slack). `None` = unknown, which the
    /// policy treats conservatively.
    fn headroom(&self) -> Option<(u64, u64)> {
        None
    }
    /// The two-pool swing (§7 major compaction). Same caller contract as
    /// [`LocalStack::reclaim`] and the identical post-state — which
    /// executor ran is not observable from outside the node.
    fn swing(
        &self,
        _snapshot: &str,
        _hooks: &mut SwingHooks<'_>,
    ) -> Result<SwingStats, StackError> {
        Err(StackError::Fault("swing unsupported on this stack"))
    }
    /// Remove slot leftovers after an interrupted swing. `flipped` is
    /// the durable discriminator (§7 crash windows): `false` = slot B is
    /// garbage (sweep it, re-decide), `true` = the flip is durable so
    /// slot A is garbage (finish the teardown and canonicalize B).
    /// Idempotent across all partial states. Returns whether anything
    /// was swept.
    ///
    /// CONTRACT (task 15e): a sweep must never need the slot's external
    /// origin — the stage resolves slots BEFORE any origin attach
    /// exists, so `resolve_slot` runs it against a `SlotConfig` whose
    /// `external_origin` is `None`. Only the swing's B *build* names
    /// the origin; a future sweep step that comes to depend on it
    /// breaks that stage-time resolution silently.
    fn sweep_slot(&self, _flipped: bool) -> Result<bool, StackError> {
        Ok(false)
    }
    /// Lazy-timing watermark eviction (§7.1): reclaim at most
    /// `max_ranges` still-shared ranges, discarding them on BOTH the
    /// live origin and the retained snapshot inside one freeze window —
    /// the dual discard frees the data blocks (last-ref drop) and keeps
    /// `thin_delta(S, S′)` from ever spanning an eviction. Same caller
    /// contract as [`LocalStack::reclaim`] (served-origin fenced first).
    /// Returns bytes discarded.
    fn evict(&self, snapshot: &str, max_ranges: usize) -> Result<u64, StackError>;
    /// §8.1 rule 3: take the mounted filesystem's SUPERBLOCK read-only,
    /// and put it back. The worst exhaustion failure is the default one
    /// — 60 s of hung writes, `EIO` mid-journal, a shut-down filesystem
    /// that needs a remount to come back — and a clean `EROFS` reached
    /// deliberately is strictly better: it stops the writers, it damages
    /// nothing, and it resumes the instant space returns.
    ///
    /// Two rules the caller owns, not this method:
    ///
    /// - the restore runs only AFTER a reclaim has actually freed pool
    ///   blocks (§8.2 step 6 — journal replay allocates, so remounting
    ///   first re-wedges instantly), and
    /// - a volume in this state reclaims with the PORT executor (§8.2
    ///   step 5), which [`crate::policy::decide`] enforces.
    ///
    /// The default is a no-op error: a stack with no mounted filesystem
    /// of its own has nothing to degrade.
    fn degrade_read_only(&self) -> Result<(), StackError> {
        Err(StackError::Unsupported(
            "no mounted filesystem to remount read-only".into(),
        ))
    }
    fn restore_read_write(&self) -> Result<(), StackError> {
        Err(StackError::Unsupported(
            "no mounted filesystem to remount read-write".into(),
        ))
    }
    /// Whether the volume's filesystem is read-only right now, derived
    /// from the kernel rather than remembered. `None` = the stack
    /// cannot answer.
    ///
    /// This exists because the remount OUTLIVES THE PROCESS. Rule 3's
    /// `degraded` latch is engine-local, so an agent that restarts
    /// mid-outage would come back believing the volume is writable, and
    /// nothing would ever remount it back: `restore_read_write` is
    /// gated on the latch, so the advance would reclaim into the pool
    /// and leave the workload stopped until a human intervened. That
    /// turns rule 3's mitigation into the wedge §8.2 exists to exit.
    ///
    /// Re-deriving costs no record-format bump — the same reasoning
    /// that keeps the guest-discard mask out of the state record
    /// (§8.1 / Invariant B): the kernel is asked, never trusted to have
    /// been remembered.
    ///
    /// Note this is NOT symmetric with `failing_since`, which stays in
    /// memory deliberately. Forgetting T errs *late*, and late is safe.
    /// Forgetting `degraded` errs *permanent*.
    fn mount_is_read_only(&self) -> Option<bool> {
        None
    }
    /// Whether the volume's filesystem will actually accept a write.
    /// `None` = the stack cannot answer, or the answer was
    /// inconclusive.
    ///
    /// The §8.2 step 6 restore needs this because `MS_REMOUNT`
    /// SUCCEEDS on an aborted superblock and changes nothing (measured:
    /// rc=0, `abort,emergency_ro` still set, writes still `EROFS`). A
    /// restore that trusted its own return value would clear the
    /// degradation latch and log "filesystem back READ-WRITE" about a
    /// volume that will not take a byte — a false all-clear, which is a
    /// worse failure than the stall it replaces.
    fn mount_is_writable(&self) -> Option<bool> {
        None
    }
    /// Invariant B: re-derive the guest-discard mask on the filesystem's
    /// shim — write the queue knob and read it back — and report what
    /// the kernel says *now*. Three answers, and the engine treats each
    /// differently:
    ///
    /// - `Ok(())` — masked. The claim the invariant rests on, and the
    ///   only answer that may be read as protection.
    /// - [`StackError::Unsupported`] — there is nothing here to mask:
    ///   no shim is configured on a stack that would otherwise have
    ///   one. Not a lapse (no assert ever ran, and retrying asserts
    ///   nothing), but not protection either — the volume is unguarded
    ///   and an operator has to hear it, so the engine says so once per
    ///   volume rather than every cycle.
    /// - any other error — a live, mounted volume IS passing guest
    ///   discards through to blocks the origin serves. The engine
    ///   publishes anyway (an unpublished delta is the worse loss) and
    ///   re-asserts every cycle until one takes.
    ///
    /// The default is `Ok(())`: a stack with no shim mechanism at all
    /// (the fake, an unmounted rig) has no path by which a guest
    /// `fstrim` could reach the pool.
    fn assert_guest_discard_mask(&self) -> Result<(), StackError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// FakeStack
// ---------------------------------------------------------------------------

/// Where an injected [`FakeStack::swing`] failure lands relative to the
/// durable slot flip — the two §7 crash arms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailPoint {
    /// Nothing irreversible happened: slot B is garbage, the engine
    /// sweeps and falls back to the port in the same cycle.
    BeforeFlip,
    /// After the retarget, before the flip, with the tables NOT put
    /// back: the live filesystem is on B under a record still naming A.
    /// No port fallback is safe from here — the engine must stall and
    /// let recovery repair the tables from the record.
    AfterRetarget,
    /// The flip is durable: recovery must sweep slot A and finish the
    /// §3 tail.
    AfterFlip,
}

#[derive(Clone)]
struct FakeSnap {
    image: Vec<u8>,
    /// Chunk indices mapped at capture time.
    mapped: Vec<bool>,
    role: Option<SnapRole>,
}

struct FakeInner {
    size: u64,
    chunk: u64,
    origin: Vec<u8>,
    origin_mapped: Vec<bool>,
    /// External-origin backing (D-M15): unmapped origin chunks read
    /// through to this instead of zeros. `reclaim` copies content here
    /// before unmapping — modeling "the repointed origin now serves it".
    backing: Vec<u8>,
    snaps: BTreeMap<String, FakeSnap>,
    next_dev_id: u64,
    pool_tx: u64,
    ticks: u64,
    /// Fail the next N captures (fault injection).
    fail_captures: u32,
    /// Whether this fake models a swing-capable (cache-less, slotted)
    /// stack.
    swing_supported: bool,
    /// Explicit headroom answer; `None` derives one from
    /// `swing_supported` (unbounded room when capable).
    headroom: Option<(u64, u64)>,
    /// §8.1 rule 2's filesystem surface, in bytes. `None` = the surface
    /// does not bind (a preallocated backing, or none at all), which is
    /// the default so pressure reduces to pool occupancy exactly as
    /// before this existed.
    fs_free: Option<u64>,
    /// Invariant B: whether the modeled shim refuses to be masked. The
    /// default `false` is the healthy stack — the fake has no queue at
    /// all, so it asserts trivially.
    mask_fails: bool,
    /// Invariant B's other negative answer: no shim is configured, so
    /// nothing was asserted and nothing can be retried — the volume is
    /// simply unguarded (the config-file stack with no `shim_name`).
    mask_unsupported: bool,
    mask_asserts: u32,
    /// One-shot injected swing failure.
    fail_swing: Option<FailPoint>,
    /// Slot B exists (a swing started and has not completed or been
    /// swept) — what `sweep_slot` reports having found.
    slot_dirty: bool,
    /// One-shot injected `sweep_slot` failure (a slot that will not
    /// clean up: EBUSY behind a straggling reference, a loop that
    /// refuses to detach).
    fail_sweep: bool,
    swings: u32,
    reclaims: u32,
    evicts: u32,
    /// Every `sweep_slot` call's `flipped` argument, in order.
    sweeps: Vec<bool>,
    /// Space-affecting and degradation calls in the order they arrived —
    /// what §8.2 step 6's "only AFTER the reclaim" is asserted against.
    /// A counter cannot express an ordering constraint, and wall time
    /// would only make the assertion flaky.
    calls: Vec<&'static str>,
    /// Injected `degrade_read_only` / `restore_read_write` failures:
    /// rule 3 is a mitigation, so a remount that fails must not take the
    /// cycle down with it.
    remount_fails: bool,
    /// Whether the modeled filesystem is currently read-only.
    read_only: bool,
    /// Whether the modeled superblock has ABORTED. `MS_REMOUNT`
    /// succeeds on such a super and changes nothing, so
    /// `restore_read_write` returns `Ok(())` while the volume stays
    /// unwritable — the false all-clear §8.2 step 6 has to survive.
    aborted: bool,
    /// Make `mount_is_read_only` unanswerable (`None`).
    ro_probe_fails: bool,
}

/// In-memory thin pool. Cloning shares the pool (Arc), mirroring how the
/// real pool survives an agent restart while the `CycleEngine` does not.
#[derive(Clone)]
pub struct FakeStack {
    inner: Arc<Mutex<FakeInner>>,
}

impl FakeStack {
    pub fn new(size: u64, chunk: u64) -> Self {
        assert!(size.is_multiple_of(chunk), "size must be chunk-aligned");
        let chunks = (size / chunk) as usize;
        Self {
            inner: Arc::new(Mutex::new(FakeInner {
                size,
                chunk,
                origin: vec![0; size as usize],
                origin_mapped: vec![false; chunks],
                backing: vec![0; size as usize],
                snaps: BTreeMap::new(),
                next_dev_id: 1,
                pool_tx: 1,
                ticks: 0,
                fail_captures: 0,
                swing_supported: false,
                headroom: None,
                fs_free: None,
                mask_fails: false,
                mask_unsupported: false,
                mask_asserts: 0,
                fail_swing: None,
                slot_dirty: false,
                fail_sweep: false,
                swings: 0,
                reclaims: 0,
                evicts: 0,
                sweeps: Vec::new(),
                calls: Vec::new(),
                remount_fails: false,
                read_only: false,
                aborted: false,
                ro_probe_fails: false,
            })),
        }
    }

    /// Test workload: write to the origin (maps the touched chunks).
    pub fn write_origin(&self, off: u64, data: &[u8]) {
        let mut g = self.inner.lock().unwrap();
        let chunk = g.chunk;
        g.origin[off as usize..off as usize + data.len()].copy_from_slice(data);
        let first = off / chunk;
        let last = (off + data.len() as u64 - 1) / chunk;
        for c in first..=last {
            g.origin_mapped[c as usize] = true;
        }
    }

    /// Test workload: discard (fstrim) — unmaps whole chunks, zeroes bytes.
    pub fn discard_origin(&self, off: u64, len: u64) {
        let mut g = self.inner.lock().unwrap();
        let chunk = g.chunk;
        assert!(
            off.is_multiple_of(chunk) && len.is_multiple_of(chunk),
            "chunk-aligned discard"
        );
        for c in off / chunk..(off + len) / chunk {
            g.origin_mapped[c as usize] = false;
        }
        g.origin[off as usize..(off + len) as usize].fill(0);
        // fstrim semantics: a trimmed chunk really reads zero.
        g.backing[off as usize..(off + len) as usize].fill(0);
    }

    /// A logical-content view of the origin (unmapped chunks read the
    /// external-origin backing — zeros until a reclaim put content there).
    pub fn origin_image(&self) -> Vec<u8> {
        let g = self.inner.lock().unwrap();
        let mut out = g.backing.clone();
        let chunk = g.chunk as usize;
        for (c, m) in g.origin_mapped.iter().enumerate() {
            if *m {
                let a = c * chunk;
                out[a..a + chunk].copy_from_slice(&g.origin[a..a + chunk]);
            }
        }
        out
    }

    /// Arm capture failures (freeze/lvcreate faults).
    pub fn fail_next_captures(&self, n: u32) {
        self.inner.lock().unwrap().fail_captures = n;
    }

    /// Number of live snapshot LVs (leak assertions in tests).
    pub fn snap_count(&self) -> usize {
        self.inner.lock().unwrap().snaps.len()
    }

    /// Model a swing-capable stack (cache-less, slot-managed).
    pub fn set_swing_supported(&self, yes: bool) {
        self.inner.lock().unwrap().swing_supported = yes;
    }

    /// Pin the headroom answer (`(fs_free, required_transient)`); without
    /// this a swing-capable fake reports unbounded room.
    pub fn set_headroom(&self, h: Option<(u64, u64)>) {
        self.inner.lock().unwrap().headroom = h;
    }

    /// Bind §8.1 rule 2's filesystem surface at `bytes` free — what a
    /// SPARSE backing exposes. `None` (the default) leaves the surface
    /// unbound, the preallocated posture.
    pub fn set_fs_free(&self, bytes: Option<u64>) {
        self.inner.lock().unwrap().fs_free = bytes;
    }

    /// Invariant B: model a shim whose guest-discard mask will not take
    /// — the live, mounted, thawed volume that passes a guest `fstrim`
    /// straight through onto blocks the origin serves.
    pub fn set_mask_fails(&self, yes: bool) {
        self.inner.lock().unwrap().mask_fails = yes;
    }

    /// Invariant B: model a stack with NO shim configured — the
    /// `shim_name`-less config-file deployment, whose mask is not
    /// failing but was never asserted at all.
    pub fn set_mask_unsupported(&self, yes: bool) {
        self.inner.lock().unwrap().mask_unsupported = yes;
    }

    /// How many times the engine has re-derived the mask from this
    /// stack — Invariant B's "cannot lapse unnoticed" is a claim about
    /// this number growing every cycle, failing or not.
    pub fn mask_asserts(&self) -> u32 {
        self.inner.lock().unwrap().mask_asserts
    }

    /// Arm a one-shot swing failure at the given crash arm.
    pub fn fail_swing(&self, at: FailPoint) {
        self.inner.lock().unwrap().fail_swing = Some(at);
    }

    /// Arm a one-shot `sweep_slot` failure — the case §7's C2 turns on:
    /// a slot the engine cannot clean up must stay armed in the record,
    /// never be handed to the port.
    pub fn fail_next_sweep(&self) {
        self.inner.lock().unwrap().fail_sweep = true;
    }

    /// Completed swings (the aging counter's ground truth in tests).
    pub fn swings(&self) -> u32 {
        self.inner.lock().unwrap().swings
    }

    /// Completed port reclaims.
    pub fn reclaims(&self) -> u32 {
        self.inner.lock().unwrap().reclaims
    }

    /// Eviction calls served.
    pub fn evicts(&self) -> u32 {
        self.inner.lock().unwrap().evicts
    }

    /// Every `sweep_slot` call's `flipped` argument, in order.
    pub fn sweeps(&self) -> Vec<bool> {
        self.inner.lock().unwrap().sweeps.clone()
    }

    /// Space-affecting and degradation calls, in arrival order:
    /// `"reclaim"`, `"swing"`, `"evict"`, `"degrade_ro"`,
    /// `"restore_rw"`. §8.2 step 6 is an ordering rule, so this is what
    /// tests it.
    pub fn calls(&self) -> Vec<&'static str> {
        self.inner.lock().unwrap().calls.clone()
    }

    /// Whether the modeled filesystem is read-only right now.
    pub fn read_only(&self) -> bool {
        self.inner.lock().unwrap().read_only
    }

    /// Make every remount fail — rule 3's mitigation failing, which must
    /// not fail the cycle it was trying to protect.
    pub fn set_remount_fails(&self, v: bool) {
        self.inner.lock().unwrap().remount_fails = v;
    }

    /// Abort the modeled superblock. Two measured properties, both
    /// load-bearing: it reports `rw` (so `statvfs` and the read-only
    /// probe cannot see it), and remounts still SUCCEED while changing
    /// nothing.
    pub fn abort_fs(&self) {
        let mut g = self.inner.lock().unwrap();
        g.aborted = true;
        // `rw,...,abort,emergency_ro` — the clean read-only flag is
        // gone the moment the super aborts.
        g.read_only = false;
    }

    /// Make the read-only probe unanswerable, so the restart-adoption
    /// path sees `None` rather than a verdict.
    pub fn set_ro_probe_fails(&self, v: bool) {
        self.inner.lock().unwrap().ro_probe_fails = v;
    }
}

fn logical_image(raw: &[u8], mapped: &[bool], chunk: u64) -> Vec<u8> {
    let mut out = vec![0u8; raw.len()];
    for (c, m) in mapped.iter().enumerate() {
        if *m {
            let a = c * chunk as usize;
            out[a..a + chunk as usize].copy_from_slice(&raw[a..a + chunk as usize]);
        }
    }
    out
}

impl FakeInner {
    /// A chunk is shared iff mapped in both snapshot and origin with
    /// identical content (the fake's stand-in for same-data-block).
    fn is_shared(&self, snap: &FakeSnap, c: usize) -> bool {
        let (a, z) = (c * self.chunk as usize, (c + 1) * self.chunk as usize);
        snap.mapped[c] && self.origin_mapped[c] && snap.image[a..z] == self.origin[a..z]
    }

    /// Reclaim one shared chunk: the repoint made the backing serve
    /// exactly this content (Invariant B); model it, then unmap.
    fn reclaim_chunk(&mut self, c: usize) {
        let (a, z) = (c * self.chunk as usize, (c + 1) * self.chunk as usize);
        let content = self.origin[a..z].to_vec();
        self.backing[a..z].copy_from_slice(&content);
        self.origin_mapped[c] = false;
    }

    /// The full Invariant B reclaim — both executors' identical
    /// post-state (pool = unpublished delta).
    fn compact(&mut self, snapshot: &str) -> Result<u64, StackError> {
        let snap = self
            .snaps
            .get(snapshot)
            .ok_or_else(|| StackError::NotFound(snapshot.into()))?
            .clone();
        let mut bytes = 0u64;
        for c in 0..snap.mapped.len() {
            if self.is_shared(&snap, c) {
                self.reclaim_chunk(c);
                bytes += self.chunk;
            }
        }
        Ok(bytes)
    }
}

impl LocalStack for FakeStack {
    fn capture(&self, name: &str) -> Result<LocalSnap, StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.fail_captures > 0 {
            g.fail_captures -= 1;
            return Err(StackError::Fault("injected capture failure"));
        }
        if g.snaps.contains_key(name) {
            return Err(StackError::Fault("snapshot name exists"));
        }
        g.ticks += 1;
        g.next_dev_id += 1;
        g.pool_tx += 1;
        let meta = LocalSnap {
            lv_name: name.to_string(),
            thin_dev_id: g.next_dev_id,
            pool_tx_id: g.pool_tx,
            created_at_ms: g.ticks,
        };
        let snap = FakeSnap {
            image: g.origin.clone(),
            mapped: g.origin_mapped.clone(),
            role: None,
        };
        g.snaps.insert(name.to_string(), snap);
        Ok(meta)
    }

    fn snap_delete(&self, name: &str) -> Result<(), StackError> {
        self.inner
            .lock()
            .unwrap()
            .snaps
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| StackError::NotFound(name.into()))
    }

    fn set_role(&self, name: &str, role: Option<SnapRole>) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        let s = g
            .snaps
            .get_mut(name)
            .ok_or_else(|| StackError::NotFound(name.into()))?;
        s.role = role;
        Ok(())
    }

    fn observe(&self) -> LocalObservation {
        let g = self.inner.lock().unwrap();
        let mut obs = LocalObservation::default();
        for (name, s) in &g.snaps {
            match s.role {
                Some(SnapRole::Baseline) => obs.baselines.push(name.clone()),
                Some(SnapRole::Candidate) => obs.candidates.push(name.clone()),
                None => {}
            }
        }
        obs
    }

    fn thin_delta(&self, older: &str, newer: &str) -> Result<Vec<DeltaRange>, StackError> {
        let g = self.inner.lock().unwrap();
        let b = g
            .snaps
            .get(older)
            .ok_or_else(|| StackError::NotFound(older.into()))?;
        let s = g
            .snaps
            .get(newer)
            .ok_or_else(|| StackError::NotFound(newer.into()))?;
        let chunk = g.chunk as usize;
        let mut out = Vec::new();
        for c in 0..b.mapped.len() {
            let (a, z) = (c * chunk, (c + 1) * chunk);
            let range = (a as u64, chunk as u64);
            if s.mapped[c] {
                if !b.mapped[c] || b.image[a..z] != s.image[a..z] {
                    out.push(DeltaRange::Data {
                        off: range.0,
                        len: range.1,
                    });
                }
            } else if b.mapped[c] {
                out.push(DeltaRange::Clear {
                    off: range.0,
                    len: range.1,
                });
            }
        }
        Ok(out)
    }

    fn full_mapped(&self, name: &str) -> Result<Vec<DeltaRange>, StackError> {
        let g = self.inner.lock().unwrap();
        let s = g
            .snaps
            .get(name)
            .ok_or_else(|| StackError::NotFound(name.into()))?;
        let chunk = g.chunk;
        Ok(s.mapped
            .iter()
            .enumerate()
            .filter(|(_, m)| **m)
            .map(|(c, _)| DeltaRange::Data {
                off: c as u64 * chunk,
                len: chunk,
            })
            .collect())
    }

    fn read(&self, name: &str, off: u64, len: u64) -> Result<Vec<u8>, StackError> {
        let g = self.inner.lock().unwrap();
        let s = g
            .snaps
            .get(name)
            .ok_or_else(|| StackError::NotFound(name.into()))?;
        let img = logical_image(&s.image, &s.mapped, g.chunk);
        Ok(img[off as usize..(off + len) as usize].to_vec())
    }

    fn volume_size(&self) -> u64 {
        self.inner.lock().unwrap().size
    }

    fn chunk_size(&self) -> u64 {
        self.inner.lock().unwrap().chunk
    }

    fn reclaim(&self, snapshot: &str) -> Result<u64, StackError> {
        let mut g = self.inner.lock().unwrap();
        let bytes = g.compact(snapshot)?;
        g.reclaims += 1;
        g.calls.push("reclaim");
        Ok(bytes)
    }

    fn degrade_read_only(&self) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.remount_fails {
            return Err(StackError::Fault("injected remount failure"));
        }
        g.read_only = true;
        g.calls.push("degrade_ro");
        Ok(())
    }

    fn restore_read_write(&self) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        if g.remount_fails {
            return Err(StackError::Fault("injected remount failure"));
        }
        g.calls.push("restore_rw");
        // Returning `Ok` on an aborted super is not a shortcut — it is
        // what the kernel does (rc=0, flags unchanged, writes still
        // refused), and it is the whole hazard. Clearing `read_only` is
        // harmless either way: an aborted super already reports rw, and
        // what governs writability here is `aborted`.
        g.read_only = false;
        Ok(())
    }

    fn mount_is_read_only(&self) -> Option<bool> {
        let g = self.inner.lock().unwrap();
        if g.ro_probe_fails {
            return None;
        }
        Some(g.read_only)
    }

    fn mount_is_writable(&self) -> Option<bool> {
        let g = self.inner.lock().unwrap();
        Some(!g.read_only && !g.aborted)
    }

    fn pool_usage(&self) -> Option<(u64, u64)> {
        let g = self.inner.lock().unwrap();
        let used = g.origin_mapped.iter().filter(|m| **m).count() as u64;
        Some((used, g.origin_mapped.len() as u64))
    }

    fn assert_guest_discard_mask(&self) -> Result<(), StackError> {
        let mut g = self.inner.lock().unwrap();
        g.mask_asserts += 1;
        if g.mask_unsupported {
            return Err(StackError::Unsupported(
                "no shim is configured on this stack, so there is no queue to mask".into(),
            ));
        }
        if g.mask_fails {
            return Err(StackError::Fault(
                "guest-discard mask did not take on the shim (Invariant B)",
            ));
        }
        Ok(())
    }

    fn pressure(&self) -> Option<Pressure> {
        let g = self.inner.lock().unwrap();
        let used = g.origin_mapped.iter().filter(|m| **m).count() as u64 * g.chunk;
        let total = g.origin_mapped.len() as u64 * g.chunk;
        let pool_free = total.saturating_sub(used);
        let (effective_free, fs_bound) = match g.fs_free {
            Some(fs) if fs < pool_free => (fs, true),
            _ => (pool_free, false),
        };
        Some(Pressure {
            used_bytes: used,
            total_bytes: total,
            effective_free,
            fs_bound,
        })
    }

    fn enumerate(&self, snapshot: &str) -> Result<ReclaimSets, StackError> {
        let g = self.inner.lock().unwrap();
        let snap = g
            .snaps
            .get(snapshot)
            .ok_or_else(|| StackError::NotFound(snapshot.into()))?;
        let mut sets = ReclaimSets::default();
        for c in 0..g.origin_mapped.len() {
            let shared = g.is_shared(snap, c);
            let dirty = g.origin_mapped[c] && !shared;
            let (off, len) = (c as u64 * g.chunk, g.chunk);
            let push = |v: &mut Vec<(u64, u64)>| match v.last_mut() {
                Some(last) if last.0 + last.1 == off => last.1 += len,
                _ => v.push((off, len)),
            };
            if shared {
                push(&mut sets.shared);
            } else if dirty {
                push(&mut sets.dirty);
            }
        }
        Ok(sets)
    }

    fn swing_supported(&self) -> bool {
        self.inner.lock().unwrap().swing_supported
    }

    fn headroom(&self) -> Option<(u64, u64)> {
        let g = self.inner.lock().unwrap();
        match g.headroom {
            Some(h) => Some(h),
            None if g.swing_supported => Some((u64::MAX, 0)),
            None => None,
        }
    }

    /// The swing modeled at the fake's level of abstraction: post-state
    /// identical to [`LocalStack::reclaim`] (which executor ran is not
    /// observable from outside the node), with the state change landing
    /// at the retarget — i.e. before the flip persists — so the two
    /// injected crash arms reproduce exactly the §7 crash windows:
    /// pre-flip failure leaves the pool untouched (B was garbage),
    /// post-flip failure leaves the compacted pool behind a durable flip.
    fn swing(&self, snapshot: &str, hooks: &mut SwingHooks<'_>) -> Result<SwingStats, StackError> {
        {
            let mut g = self.inner.lock().unwrap();
            if !g.swing_supported {
                return Err(StackError::Fault("swing unsupported on this stack"));
            }
            g.slot_dirty = true; // slot B built
            if g.fail_swing == Some(FailPoint::BeforeFlip) {
                g.fail_swing = None;
                return Err(StackError::Fault("injected swing failure before flip"));
            }
            // Retarget: the live thin is now the fresh pool over the
            // repointed origin — shared content reads through the
            // backing, dirty content was copied.
            let bytes = g.compact(snapshot)?;
            let _ = bytes;
            *hooks.retargeted = true;
            if g.fail_swing == Some(FailPoint::AfterRetarget) {
                g.fail_swing = None;
                return Err(StackError::Fault("injected swing failure after retarget"));
            }
        }
        // The durable flip, inside the (modeled) freeze window.
        (hooks.persist_flip)()?;
        let mut g = self.inner.lock().unwrap();
        if g.fail_swing == Some(FailPoint::AfterFlip) {
            g.fail_swing = None;
            return Err(StackError::Fault("injected swing failure after flip"));
        }
        g.slot_dirty = false; // teardown + canonicalize done
        g.swings += 1;
        g.calls.push("swing");
        Ok(SwingStats {
            rounds: 1,
            ..SwingStats::default()
        })
    }

    fn sweep_slot(&self, flipped: bool) -> Result<bool, StackError> {
        let mut g = self.inner.lock().unwrap();
        g.sweeps.push(flipped);
        if std::mem::take(&mut g.fail_sweep) {
            // Nothing is swept: the slot stays exactly as dirty as the
            // caller must assume it is.
            return Err(StackError::Fault("injected sweep failure"));
        }
        Ok(std::mem::take(&mut g.slot_dirty))
    }

    /// Partial reclaim: the first `max_ranges` coalesced shared runs are
    /// discarded on BOTH sides — origin (unmap, backing serves it) and
    /// snapshot (so no later `thin_delta` from this snapshot ever spans
    /// the eviction).
    fn evict(&self, snapshot: &str, max_ranges: usize) -> Result<u64, StackError> {
        let mut g = self.inner.lock().unwrap();
        g.evicts += 1;
        g.calls.push("evict");
        let snap = g
            .snaps
            .get(snapshot)
            .ok_or_else(|| StackError::NotFound(snapshot.into()))?
            .clone();
        // Coalesce shared chunks into runs, take the first N (FIFO =
        // LBA order — heat data is an optimization, not a dependency).
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for c in 0..snap.mapped.len() {
            if g.is_shared(&snap, c) {
                match runs.last_mut() {
                    Some(last) if last.0 + last.1 == c => last.1 += 1,
                    _ => runs.push((c, 1)),
                }
            }
        }
        runs.truncate(max_ranges);
        let mut bytes = 0u64;
        for (start, n) in runs {
            for c in start..start + n {
                g.reclaim_chunk(c);
                g.snaps.get_mut(snapshot).unwrap().mapped[c] = false;
                bytes += g.chunk;
            }
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHUNK: u64 = 4096;

    #[test]
    fn delta_semantics_match_thin_delta() {
        let st = FakeStack::new(8 * CHUNK, CHUNK);
        st.write_origin(0, &[1; 2 * 4096]); // chunks 0,1
        st.write_origin(3 * CHUNK, &[2; 4096]); // chunk 3
        st.capture("b").unwrap();

        st.write_origin(CHUNK, &[9; 4096]); // differ: chunk 1
        st.write_origin(5 * CHUNK, &[7; 4096]); // right-only: chunk 5
        st.discard_origin(3 * CHUNK, CHUNK); // left-only: chunk 3
        st.capture("s").unwrap();

        let d = st.thin_delta("b", "s").unwrap();
        assert_eq!(
            d,
            vec![
                DeltaRange::Data {
                    off: CHUNK,
                    len: CHUNK
                },
                DeltaRange::Clear {
                    off: 3 * CHUNK,
                    len: CHUNK
                },
                DeltaRange::Data {
                    off: 5 * CHUNK,
                    len: CHUNK
                },
            ]
        );

        // Same chunks (0) appear in neither list; snapshots are immutable
        // under later origin writes.
        st.write_origin(0, &[8; 4096]);
        assert_eq!(st.thin_delta("b", "s").unwrap(), d);
    }

    #[test]
    fn snapshot_reads_are_zero_filled_for_unmapped() {
        let st = FakeStack::new(4 * CHUNK, CHUNK);
        st.write_origin(2 * CHUNK, &[5; 4096]);
        st.capture("s").unwrap();
        let r = st.read("s", 0, 4 * CHUNK).unwrap();
        assert!(r[..2 * 4096].iter().all(|&b| b == 0));
        assert!(r[2 * 4096..3 * 4096].iter().all(|&b| b == 5));
    }

    #[test]
    fn roles_drive_observation() {
        let st = FakeStack::new(4 * CHUNK, CHUNK);
        st.capture("b0").unwrap();
        st.capture("s1").unwrap();
        st.set_role("b0", Some(SnapRole::Baseline)).unwrap();
        st.set_role("s1", Some(SnapRole::Candidate)).unwrap();
        let o = st.observe();
        assert_eq!(o.baselines, vec!["b0"]);
        assert_eq!(o.candidates, vec!["s1"]);
        st.set_role("s1", Some(SnapRole::Baseline)).unwrap();
        st.set_role("b0", None).unwrap();
        let o = st.observe();
        assert_eq!(o.baselines, vec!["s1"]);
        assert!(o.candidates.is_empty());
    }
}

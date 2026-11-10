//! [`LinuxDmOps`]: the real kernel implementation of [`DmOps`] —
//! `devicemapper` crate ioctls on `/dev/mapper/control`, raw
//! `FIFREEZE`/`FITHAW`, and linked `thinp` metadata reads (feature `dm`).
//!
//! This file is deliberately thin: it contains ONLY the code that cannot
//! run without a kernel. All orchestration lives in [`crate::dm`] and is
//! exercised rootlessly against `MockDmOps`; this glue is exercised by the
//! S14a nightly rig.

use std::fs::File;
use std::os::fd::AsRawFd;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use devicemapper::{DevId, DmFlags, DmName, DmOptions, DmUuid, DM};

use crate::dm::{DmConfig, DmDeviceInfo, DmOps, DmStack};
use crate::stack::StackError;

const FIFREEZE: libc::c_ulong = 0xC004_5877;
const FITHAW: libc::c_ulong = 0xC004_5878;

/// `mount(2)` with `MS_REMOUNT`, the raw form. A subprocess would be the
/// obvious way to do this and is exactly what must not happen here:
/// `LinuxDmOps` has no [`Runner`](crate::hostcmd::Runner), and the
/// remount is reached from the agent crate, which keeps it that way.
///
/// `source`, `fstype` and `data` are all NULL: on a remount the kernel
/// takes the filesystem-specific options from the existing mount.
///
/// **The per-mount flags are NOT preserved**, and that is a real edge
/// with teeth. `MS_REMOUNT` REPLACES `nosuid` / `nodev` / `noexec` with
/// whatever is passed, which is nothing here — measured:
///
/// ```text
///   before:                     rw,nosuid,nodev,noexec,relatime
///   after MS_REMOUNT|MS_RDONLY: ro,relatime
///   after MS_REMOUNT:           rw,relatime
/// ```
///
/// (Atime flags the kernel does carry over; only those three are lost.
/// `mount(8)` avoids this by re-reading mountinfo and re-passing them.)
///
/// Inert today: the node never forwards `mount_flags` and the staging
/// mount passes no `-o`, so no managed mount carries any of the three.
/// It is a silent security downgrade waiting for whoever plumbs
/// `mountOptions` through to staging — at which point this helper must
/// read `/proc/self/mountinfo` and re-pass the flags it finds.
fn remount(mount: &Path, flags: libc::c_ulong, what: &str) -> Result<(), StackError> {
    let target = std::ffi::CString::new(mount.as_os_str().as_bytes())
        .map_err(|e| StackError::NotFound(format!("mountpoint: {e}")))?;
    let rc = unsafe {
        libc::mount(
            std::ptr::null(),
            target.as_ptr(),
            std::ptr::null(),
            flags,
            std::ptr::null(),
        )
    };
    if rc != 0 {
        return Err(StackError::NotFound(format!(
            "remount,{what} {}: {}",
            mount.display(),
            std::io::Error::last_os_error()
        )));
    }
    Ok(())
}

/// Whether an errno means the SUPERBLOCK is refusing writes, as
/// opposed to the operation failing for a reason that says nothing
/// about the filesystem's health.
///
/// Shared by both halves of [`write_probe`] on purpose: the create and
/// the unlink are the same question asked twice, and an unlink that
/// answers `EROFS` is exactly as conclusive as a create that does.
/// `ENOSPC` is deliberately absent — a full pool is the state this
/// whole subsystem exists to survive, not evidence of a dead super.
fn refuses_writes(e: &std::io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::EROFS) | Some(libc::EIO))
}

/// §8.2 step 6's writability check, as a free function: it needs a
/// DIRECTORY, not a device-mapper handle.
///
/// Split out for the same reason as `classify_freeze_errno`, and for a
/// sharper one. The first version was a method, so its test had to
/// build a `LinuxDmOps` — and `LinuxDmOps::new` opens
/// `/dev/mapper/control`. On a host without it the test took an early
/// return and reported `ok`, so a mutation that broke the leftover
/// handling left it green. A skip that looks like a pass is worse than
/// no test; this shape cannot have one.
pub fn write_probe(mount: &Path) -> Result<bool, StackError> {
    // A real write, because nothing cheaper is honest: on an
    // aborted super `statvfs` says rw and `access(W_OK)` says yes,
    // and both are wrong. `/proc/self/mountinfo` would catch ext4's
    // `emergency_ro` marker but not an XFS shutdown, which carries
    // no marker at all — so the probe is also the only form that
    // covers both filesystems the runbook names.
    //
    // `O_EXCL` so it can never adopt someone else's file, unlinked
    // immediately, and a distinctive name so a leftover from a
    // crash between the two syscalls is identifiable.
    let probe = mount.join(".skynbd-rw-probe");
    let attempt = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&probe)
    };
    let mut r = attempt();
    // A leftover from a crash between the create and the unlink
    // answers `EEXIST`, which would otherwise land in the
    // inconclusive arm below — and STAY there, because the name is
    // fixed and nothing else ever removes the file. That is worse
    // than a transient: it would disable the §8.2 step 6 check for
    // the life of the volume, and a leftover that survives into the
    // next capture is in the baseline, so every volume descended
    // from it would start with the check already disabled.
    //
    // The distinctive name is what makes clearing it safe: nothing
    // but this probe ever creates it, so removing one can only be
    // reclaiming our own litter. One retry, never a loop — a second
    // `EEXIST` means something is actively recreating it, which is
    // genuinely inconclusive.
    if matches!(
        r.as_ref().err().and_then(|e| e.raw_os_error()),
        Some(libc::EEXIST)
    ) {
        // The UNLINK's errno is an answer too, and discarding it was
        // the hole: on a super that aborted while a leftover was
        // already present, the unlink fails `EROFS`, the retry then
        // fails `EEXIST` again, and the whole probe reads as
        // inconclusive — the false all-clear, reached through the
        // leftover path instead of the wiring one. Measured, on a
        // read-only tmpfs holding a leftover:
        //
        //   create -> EEXIST   (the existence check wins)
        //   unlink -> EROFS
        //
        // Nothing that refuses to remove a file will accept a write, so
        // that errno is a definite negative on its own — and
        // classifying it makes the second `EEXIST` unreachable here
        // rather than merely handled.
        r = match std::fs::remove_file(&probe) {
            Ok(()) => attempt(),
            // Hand the unlink's error to the SAME classification below
            // rather than judging it here. One classification site, not
            // two: the create and the unlink are the same question, and
            // a second copy of the predicate is a second place for it
            // to drift.
            Err(e) => Err(e),
        };
    }
    match r {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&probe);
            Ok(true)
        }
        // The superblock is refusing writes: aborted (`EROFS`) or
        // shut down (`EIO`, which is also what a shut-down XFS
        // answers for the open itself).
        Err(e) if refuses_writes(&e) => Ok(false),
        // Anything else says nothing about the superblock — a full
        // pool answers `ENOSPC`, a read-only-by-permission path
        // `EACCES`. Inconclusive, and the caller must not read it
        // as a verdict either way.
        Err(e) => Err(StackError::NotFound(format!(
            "write probe {}: {e}",
            probe.display()
        ))),
    }
}

/// Classify a failed `FIFREEZE` (§8.2 step 3). The symmetric half of the
/// discrimination [`LinuxDmOps::thaw_fs`] already makes: a thaw has to
/// tell "was not frozen" from "still frozen", and a freeze has to tell
/// "could not freeze it right now" from "there is nothing left to
/// freeze". Collapsing the second distinction is what would make
/// [`DmStack::capture`] refuse to run on exactly the volume whose delta
/// most needs to escape.
///
/// Split out as a free function because the interesting behaviour is the
/// errno table, and the table is testable without a kernel.
///
/// Every row was measured against a live kernel (6.18.5), not inferred —
/// the first draft got three of them wrong in ways no mock-driven test
/// could see.
pub fn classify_freeze_errno(err: &std::io::Error, mount: &Path) -> StackError {
    match err.raw_os_error() {
        // The filesystem is no longer running.
        //
        // `EIO` is the errno a shut-down XFS answers for the
        // mountpoint *open* itself — which is why this classifier is
        // applied to the open as well as to the ioctl. It is NOT an
        // ext4 answer: measured, an ext4 that hits `ext4_handle_error`
        // under the default `errors=remount-ro` lands in
        // `emergency_ro` and answers `EROFS` below, and under
        // `errors=continue` it freezes normally (rc=0).
        //
        // `EROFS` is the ABORTED super, not a cleanly read-only one.
        // The distinction is the whole row and it is counter-intuitive:
        // a cleanly read-only super FREEZES SUCCESSFULLY (`freeze_super`
        // short-circuits on `sb_rdonly` and returns 0), while an aborted
        // ext4 still reports `rw` in `/proc/mounts` and answers `EROFS`:
        //
        //   ro,relatime                     FIFREEZE -> 0
        //   rw,relatime,abort,emergency_ro  FIFREEZE -> EROFS
        //
        // So this arm never fires for a filesystem §8.1 rule 3 took
        // read-only — that one freezes normally, and its capture is
        // filesystem-consistent, which is better. It fires for the
        // volume rule 3 did not reach in time.
        Some(libc::EIO) | Some(libc::EROFS) => StackError::Wedged(format!(
            "FIFREEZE {}: {err} — the filesystem has shut down; \
             its on-disk state is already crash-consistent",
            mount.display()
        )),
        // No freeze support on this superblock at all (tmpfs, some
        // overlay and network mounts). `EOPNOTSUPP` is the real errno:
        // `FIFREEZE` is dispatched generically in `do_vfs_ioctl`, so it
        // never reaches an unknown-ioctl path, and `ioctl_fsfreeze`
        // returns `-EOPNOTSUPP` when the super has no `freeze_fs` op.
        // `ENOTTY` is kept beside it as a belt-and-braces row.
        //
        // A capability that was never there is not a deployment fault —
        // the same reading `Unsupported` was introduced for in part A.
        Some(libc::EOPNOTSUPP) | Some(libc::ENOTTY) => {
            StackError::Unsupported(format!("FIFREEZE {}: {err}", mount.display()))
        }
        // `EBUSY` — the superblock is already frozen. Classified, not
        // lumped into the catch-all, because a caller that needs the
        // filesystem quiesced (the sweep, putting tables back before it
        // destroys a pool) has to tell "the window is already open" from
        // "I could not open one". It stays OUT of `Wedged`: a contended
        // freeze is transient, and letting it take the crash-consistent
        // path would silently downgrade every capture that merely raced
        // another freezer — `capture` matches `Wedged` alone, so this
        // still fails there exactly as before.
        Some(libc::EBUSY) => {
            StackError::AlreadyFrozen(format!("FIFREEZE {}: {err}", mount.display()))
        }
        // Everything else is a real failure and must propagate.
        //
        //
        // `EINVAL` is deliberately not `Wedged` even though a super
        // going down can produce it: the same errno also means "this fd
        // is not a mountpoint" and "this fs has no freeze support", and
        // guessing wrong here takes the crash-consistent path for a
        // reason that has nothing to do with the filesystem's health.
        //
        // (`NotFound` carrying an arbitrary failure is this module's
        // established shape — `thaw_fs`, `discard_dev` and the loop
        // ioctls all do it — because `Fault` takes a `&'static str` and
        // so cannot carry an errno at all.)
        _ => StackError::NotFound(format!("FIFREEZE {}: {err}", mount.display())),
    }
}

// Loop-device control (util-linux losetup's kernel surface — no
// subprocesses here; `LinuxDmOps` has no `Runner`).
const LOOP_SET_FD: libc::c_ulong = 0x4C00;
const LOOP_CLR_FD: libc::c_ulong = 0x4C01;
const LOOP_SET_STATUS64: libc::c_ulong = 0x4C04;
const LOOP_GET_STATUS64: libc::c_ulong = 0x4C05;
const LOOP_CTL_GET_FREE: libc::c_ulong = 0x4C82;
const LO_FLAGS_DIRECT_IO: u32 = 16;

/// `struct loop_info64` (linux/loop.h), zero-initialized.
#[repr(C)]
#[derive(Clone, Copy)]
struct LoopInfo64 {
    lo_device: u64,
    lo_inode: u64,
    lo_rdevice: u64,
    lo_offset: u64,
    lo_sizelimit: u64,
    lo_number: u32,
    lo_encrypt_type: u32,
    lo_encrypt_key_size: u32,
    lo_flags: u32,
    lo_file_name: [u8; 64],
    lo_crypt_name: [u8; 64],
    lo_encrypt_key: [u8; 32],
    lo_init: [u64; 2],
}

impl LoopInfo64 {
    fn zeroed() -> Self {
        // SAFETY: all-zero is a valid bit pattern for this plain-old-data
        // struct.
        unsafe { std::mem::zeroed() }
    }
}

/// NUL-terminated path for the raw syscall surface.
///
/// The bytes come from `OsStr`, not from `to_string_lossy`: a path is a
/// byte string on Linux, and the lossy conversion replaces every
/// non-UTF-8 byte with U+FFFD — silently producing a DIFFERENT path
/// that either does not exist or, worse, does. An interior NUL is the
/// only shape that legitimately has no C string, and that is the error
/// returned.
fn cpath(p: &Path) -> Result<std::ffi::CString, StackError> {
    std::ffi::CString::new(p.as_os_str().as_bytes())
        .map_err(|e| StackError::NotFound(format!("{}: {e}", p.display())))
}

/// Convenience alias for the production stack.
pub type LinuxDmStack = DmStack<LinuxDmOps>;

impl DmStack<LinuxDmOps> {
    /// Construct the production stack over the real kernel.
    pub fn new_linux(cfg: DmConfig) -> Result<Self, StackError> {
        DmStack::new(cfg, LinuxDmOps::new()?)
    }
}

pub struct LinuxDmOps {
    dm: DM,
}

impl LinuxDmOps {
    pub fn new() -> Result<Self, StackError> {
        let dm = DM::new().map_err(|e| StackError::NotFound(format!("dm control: {e}")))?;
        Ok(Self { dm })
    }

    fn name_id<'a>(name: &'a str) -> Result<DevId<'a>, StackError> {
        Ok(DevId::Name(
            DmName::new(name).map_err(|e| StackError::NotFound(e.to_string()))?,
        ))
    }

    /// Open a dm device WITHOUT relying on udev: raw DM ioctls create no
    /// /dev/mapper nodes (dmsetup mknods them itself; udev does it for
    /// LVM). Try /dev/mapper/<name> first, else mknod a private block node
    /// from the ioctl-reported device number. This was the S14a backfill
    /// failure: reads raced (or never saw) node creation.
    /// `write` opens O_RDWR — `BLKDISCARD` (reclaim) needs it.
    fn open_dm_dev(&self, dm_name: &str, write: bool) -> Result<File, StackError> {
        let open = |p: &Path| std::fs::OpenOptions::new().read(true).write(write).open(p);
        if let Ok(f) = open(Path::new(&format!("/dev/mapper/{dm_name}"))) {
            return Ok(f);
        }
        let info = self
            .dm
            .device_info(&Self::name_id(dm_name)?)
            .map_err(|e| StackError::NotFound(format!("info {dm_name}: {e}")))?;
        let dev = info.device();
        let dir = Path::new("/run/skynbd-agent");
        std::fs::create_dir_all(dir).map_err(|e| StackError::NotFound(e.to_string()))?;
        let node = dir.join(dm_name);
        if !node.exists() {
            let c = cpath(&node)?;
            let rc = unsafe {
                libc::mknod(
                    c.as_ptr(),
                    libc::S_IFBLK | 0o600,
                    libc::makedev(dev.major, dev.minor),
                )
            };
            if rc != 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(StackError::NotFound(format!(
                    "mknod {}: {}",
                    node.display(),
                    std::io::Error::last_os_error()
                )));
            }
        }
        open(&node).map_err(|e| StackError::NotFound(format!("open {}: {e}", node.display())))
    }

    fn suspend_opts(&self, name: &str, opts: DmOptions) -> Result<(), StackError> {
        self.dm
            .device_suspend(&Self::name_id(name)?, opts)
            .map(|_| ())
            .map_err(|e| StackError::NotFound(format!("suspend/resume {name}: {e}")))
    }

    /// Make sure `/dev/mapper/<name>` exists — external tools (mkfs,
    /// mount) resolve dm devices by that path, and raw DM ioctls never
    /// create it. Racing host udev is not a plan; mknod is.
    pub fn ensure_mapper_node(&self, dm_name: &str) -> Result<(), StackError> {
        let node = Path::new("/dev/mapper").join(dm_name);
        if node.exists() {
            return Ok(());
        }
        let info = self
            .dm
            .device_info(&Self::name_id(dm_name)?)
            .map_err(|e| StackError::NotFound(format!("info {dm_name}: {e}")))?;
        let dev = info.device();
        std::fs::create_dir_all("/dev/mapper").map_err(|e| StackError::NotFound(e.to_string()))?;
        let c = cpath(&node)?;
        let rc = unsafe {
            libc::mknod(
                c.as_ptr(),
                libc::S_IFBLK | 0o600,
                libc::makedev(dev.major, dev.minor),
            )
        };
        if rc != 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
            return Err(StackError::NotFound(format!(
                "mknod {}: {}",
                node.display(),
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    /// Path of a device's `queue/discard_max_bytes` — the one sysfs
    /// file both halves of Invariant B's mask (write and readback) must
    /// address, so they cannot drift apart.
    ///
    /// The queue lives under the device's KERNEL name (`dm-N`), and
    /// `/dev/mapper/<name>` is udev's — the two do not correspond, and
    /// raw DM ioctls make no mapper node at all (round 3). The
    /// dev_t-keyed `/sys/dev/block/<maj>:<min>` symlink is the mapping
    /// the kernel itself publishes: its target's basename IS the kernel
    /// name.
    fn discard_knob(&self, name: &str) -> Result<String, StackError> {
        let info = self.device_info(name)?;
        let link = format!("/sys/dev/block/{}:{}", info.major, info.minor);
        let target = std::fs::read_link(&link)
            .map_err(|e| StackError::NotFound(format!("readlink {link}: {e}")))?;
        let kname = target
            .file_name()
            .and_then(|k| k.to_str())
            .ok_or_else(|| StackError::NotFound(format!("{link}: unnameable target")))?;
        Ok(format!("/sys/block/{kname}/queue/discard_max_bytes"))
    }
}

/// ENOENT on the knob is a kernel that does not have it, which task
/// 15j tolerates; anything else is a failure to drive a knob that
/// exists.
fn absent_or(knob: &str, e: std::io::Error) -> StackError {
    if e.kind() == std::io::ErrorKind::NotFound {
        return StackError::Unsupported(format!("{knob}: {e}"));
    }
    StackError::NotFound(format!("{knob}: {e}"))
}

impl DmOps for LinuxDmOps {
    fn list_devices(&self) -> Result<Vec<DmDeviceInfo>, StackError> {
        let devs = self
            .dm
            .list_devices()
            .map_err(|e| StackError::NotFound(format!("dm list: {e}")))?;
        Ok(devs
            .into_iter()
            .map(|(name, dev, _ev)| DmDeviceInfo {
                name: name.to_string(),
                uuid: None, // list ioctl has no uuids; device_info fills them
                major: dev.major,
                minor: dev.minor,
            })
            .collect())
    }

    fn device_info(&self, name: &str) -> Result<DmDeviceInfo, StackError> {
        let info = self
            .dm
            .device_info(&Self::name_id(name)?)
            .map_err(|e| StackError::NotFound(format!("info {name}: {e}")))?;
        let dev = info.device();
        Ok(DmDeviceInfo {
            name: name.to_string(),
            uuid: info.uuid().map(|u| u.to_string()),
            major: dev.major,
            minor: dev.minor,
        })
    }

    fn pool_message(&self, pool: &str, msg: &str) -> Result<(), StackError> {
        self.dm
            .target_msg(&Self::name_id(pool)?, Some(0), msg)
            .map(|_| ())
            .map_err(|e| StackError::NotFound(format!("pool msg {msg:?}: {e}")))
    }

    fn pool_status_params(&self, pool: &str) -> Result<String, StackError> {
        let (_info, status) = self
            .dm
            .table_status(&Self::name_id(pool)?, DmOptions::default())
            .map_err(|e| StackError::NotFound(format!("pool status: {e}")))?;
        Ok(status
            .first()
            .map(|(_, _, _, p)| p.clone())
            .unwrap_or_default())
    }

    fn suspend_device(&self, name: &str) -> Result<(), StackError> {
        self.suspend_opts(name, DmOptions::default().set_flags(DmFlags::DM_SUSPEND))
    }

    fn resume_device(&self, name: &str) -> Result<(), StackError> {
        // Resume = suspend ioctl without the SUSPEND flag.
        self.suspend_opts(name, DmOptions::default())
    }

    fn create_ro_thin(
        &self,
        name: &str,
        uuid: &str,
        pool: &str,
        sectors: u64,
        thin_dev_id: u64,
    ) -> Result<(), StackError> {
        let pool_info = self.device_info(pool)?;
        let dm_name = DmName::new(name).map_err(|e| StackError::NotFound(e.to_string()))?;
        let dm_uuid = DmUuid::new(uuid).map_err(|e| StackError::NotFound(e.to_string()))?;
        let table = vec![(
            0u64,
            sectors,
            "thin".to_string(),
            format!("{}:{} {}", pool_info.major, pool_info.minor, thin_dev_id),
        )];
        let id = DevId::Name(dm_name);
        self.dm
            .device_create(
                dm_name,
                Some(dm_uuid),
                DmOptions::default().set_flags(DmFlags::DM_READONLY),
            )
            .map_err(|e| StackError::NotFound(format!("create {name}: {e}")))?;
        self.dm
            .table_load(
                &id,
                &table,
                DmOptions::default().set_flags(DmFlags::DM_READONLY),
            )
            .map_err(|e| StackError::NotFound(format!("table {name}: {e}")))?;
        self.resume_device(name)
    }

    fn create_device(
        &self,
        name: &str,
        uuid: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        let dm_name = DmName::new(name).map_err(|e| StackError::NotFound(e.to_string()))?;
        let dm_uuid = DmUuid::new(uuid).map_err(|e| StackError::NotFound(e.to_string()))?;
        let id = DevId::Name(dm_name);
        self.dm
            .device_create(dm_name, Some(dm_uuid), DmOptions::default())
            .map_err(|e| StackError::NotFound(format!("create {name}: {e}")))?;
        self.dm
            .table_load(&id, table, DmOptions::default())
            .map_err(|e| StackError::NotFound(format!("table {name}: {e}")))?;
        self.resume_device(name)
    }

    fn reload_device(
        &self,
        name: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        // Resolved BEFORE the suspend. Unreachable in practice, since
        // `suspend_device` validates the same name a line later — but as
        // a `?` between the suspend and the resume it is structurally the
        // very shape being fixed here, and structurally safe beats
        // accidentally safe.
        let id = Self::name_id(name)?;
        self.suspend_device(name)?;
        // A failed load must not leave the device suspended — and,
        // because the suspend above carries dm's implicit lockfs, frozen
        // as well. Resume before propagating, so a bad table costs a
        // failed reload rather than an unmountable volume.
        if let Err(e) = self.dm.table_load(&id, table, DmOptions::default()) {
            let _ = self.resume_device(name);
            return Err(StackError::NotFound(format!("reload {name}: {e}")));
        }
        self.resume_device(name)
    }

    /// The same reload with dm's implicit `lock_fs` suppressed; see
    /// [`DmOps::reload_device_in_frozen_window`] for why the second
    /// freeze is the bug rather than a redundancy worth tolerating.
    fn reload_device_in_frozen_window(
        &self,
        name: &str,
        table: &[(u64, u64, String, String)],
    ) -> Result<(), StackError> {
        let id = Self::name_id(name)?;
        self.suspend_opts(
            name,
            DmOptions::default().set_flags(DmFlags::DM_SUSPEND | DmFlags::DM_SKIP_LOCKFS),
        )?;
        if let Err(e) = self.dm.table_load(&id, table, DmOptions::default()) {
            let _ = self.resume_device(name);
            return Err(StackError::NotFound(format!("reload {name}: {e}")));
        }
        self.resume_device(name)
    }

    fn remove_device(&self, name: &str) -> Result<(), StackError> {
        self.dm
            .device_remove(&Self::name_id(name)?, DmOptions::default())
            .map(|_| ())
            .map_err(|e| StackError::NotFound(format!("remove {name}: {e}")))
    }

    fn rename_device(&self, from: &str, to: &str) -> Result<(), StackError> {
        let from_name = DmName::new(from).map_err(|e| StackError::NotFound(e.to_string()))?;
        let to_name = DmName::new(to).map_err(|e| StackError::NotFound(e.to_string()))?;
        self.dm
            .device_rename(from_name, &DevId::Name(to_name))
            .map(|_| ())
            .map_err(|e| StackError::NotFound(format!("rename {from}→{to}: {e}")))
    }

    fn read_dev(&self, name: &str, off: u64, len: u64) -> Result<Vec<u8>, StackError> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = self.open_dm_dev(name, false)?;
        f.seek(SeekFrom::Start(off))
            .map_err(|e| StackError::NotFound(e.to_string()))?;
        let mut buf = vec![0u8; len as usize];
        f.read_exact(&mut buf)
            .map_err(|e| StackError::NotFound(format!("read {name}: {e}")))?;
        Ok(buf)
    }

    fn discard_dev(&self, name: &str, off: u64, len: u64) -> Result<(), StackError> {
        // BLKDISCARD = _IO(0x12, 119): unmap the range at the block
        // layer. On a dm-thin target this releases whole pool blocks
        // back to the pool; with `no_discard_passdown` on the pool table
        // nothing propagates to the data device below (D-M15 Invariant
        // B's precondition).
        const BLKDISCARD: libc::c_ulong = 0x1277;
        let f = self.open_dm_dev(name, true)?;
        let range: [u64; 2] = [off, len];
        let rc = unsafe { libc::ioctl(f.as_raw_fd(), BLKDISCARD, range.as_ptr()) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "BLKDISCARD {name} [{off}, +{len}): {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn freeze_fs(&self, mount: &Path) -> Result<(), StackError> {
        // The OPEN goes through the same classifier as the ioctl,
        // because on some filesystems the shutdown is what makes the
        // mountpoint unopenable: a shut-down XFS answers `EIO` to
        // `open`, `stat` and `readdir` alike (only `statfs` still
        // works), so the freeze attempt never reaches the ioctl at all.
        // Classifying only the ioctl would strand the delta on exactly
        // the volume §8.2 step 3 exists for — and XFS is one of the two
        // filesystems the runbook names.
        //
        // ext4 hides this: an aborted ext4 mountpoint still opens
        // cleanly, so the ioctl arm is the one that fires there. That
        // asymmetry is why no mock-driven test can see it.
        let f = File::open(mount).map_err(|e| classify_freeze_errno(&e, mount))?;
        let rc = unsafe { libc::ioctl(f.as_raw_fd(), FIFREEZE, 0) };
        if rc != 0 {
            return Err(classify_freeze_errno(
                &std::io::Error::last_os_error(),
                mount,
            ));
        }
        Ok(())
    }

    fn thaw_fs(&self, mount: &Path) -> Result<(), StackError> {
        let f = File::open(mount).map_err(|e| StackError::NotFound(format!("mountpoint: {e}")))?;
        let rc = unsafe { libc::ioctl(f.as_raw_fd(), FITHAW, 0) };
        if rc != 0 {
            let err = std::io::Error::last_os_error();
            // `FITHAW` answers EINVAL for "was not frozen", which every
            // unconditional-thaw call site means to tolerate — a
            // replayed repair, a window the watchdog already
            // force-thawed. Every other errno (EBUSY, EIO) is a
            // filesystem that is STILL frozen, and collapsing the two
            // into one opaque fault is what made those call sites
            // swallow the failure they exist to catch.
            if err.raw_os_error() == Some(libc::EINVAL) {
                return Ok(());
            }
            return Err(StackError::NotFound(format!(
                "FITHAW {}: {err}",
                mount.display()
            )));
        }
        Ok(())
    }

    fn remount_ro(&self, mount: &Path) -> Result<(), StackError> {
        remount(mount, libc::MS_REMOUNT | libc::MS_RDONLY, "ro")
    }

    fn remount_rw(&self, mount: &Path) -> Result<(), StackError> {
        remount(mount, libc::MS_REMOUNT, "rw")
    }

    fn mount_is_writable(&self, mount: &Path) -> Result<bool, StackError> {
        write_probe(mount)
    }

    fn mount_is_read_only(&self, mount: &Path) -> Result<bool, StackError> {
        let path = cpath(mount)?;
        // SAFETY: `statvfs` fills the struct on success; it is read only
        // when the call returned 0.
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(path.as_ptr(), &mut st) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "statvfs {}: {}",
                mount.display(),
                std::io::Error::last_os_error()
            )));
        }
        Ok(st.f_flag & libc::ST_RDONLY != 0)
    }

    fn watchdog_thaw(&self, mount: &Path) -> Result<Box<dyn FnMut() + Send>, StackError> {
        // Pre-opened fd of its own (V3.2 §7.11): no child process or engine
        // lock can hold the thaw hostage.
        let thaw_fd =
            File::open(mount).map_err(|e| StackError::NotFound(format!("mountpoint: {e}")))?;
        Ok(Box::new(move || {
            unsafe { libc::ioctl(thaw_fd.as_raw_fd(), FITHAW, 0) };
        }))
    }

    fn write_dev(&self, name: &str, off: u64, data: &[u8]) -> Result<(), StackError> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = self.open_dm_dev(name, true)?;
        f.seek(SeekFrom::Start(off))
            .map_err(|e| StackError::NotFound(e.to_string()))?;
        f.write_all(data)
            .map_err(|e| StackError::NotFound(format!("write {name}: {e}")))?;
        // The sync is LOAD-BEARING, not hygiene. A buffered write to a
        // bdev lands in its page cache, but dm's metadata layers read
        // the device through dm-bufio — their own buffer cache, bios
        // straight to the device — so a caller that writes and then
        // loads a table (the cold-cache superblock zero: write, then
        // cache ctr) would race delayed writeback twice over: the ctr
        // reads the STALE bytes (default dirty writeback ~5 s, the ctr
        // milliseconds behind — the stale read essentially always
        // wins), and the writeback then lands the page over the
        // metadata of a by-then-live device. fsync on a block device
        // writes back its dirty pages and issues a device flush, which
        // is exactly the write-visible-before-ctr ordering callers are
        // relying on.
        f.sync_all()
            .map_err(|e| StackError::NotFound(format!("sync {name}: {e}")))
    }

    fn prepare_backing(
        &self,
        file: &Path,
        bytes: u64,
        prealloc: bool,
    ) -> Result<String, StackError> {
        // Reuse an existing attachment (crash-replay idempotence).
        if let Some(dev) = self.backing_of(file)? {
            return Ok(dev);
        }
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(file)
            .map_err(|e| StackError::NotFound(format!("open {}: {e}", file.display())))?;
        let cur = f
            .metadata()
            .map_err(|e| StackError::NotFound(e.to_string()))?
            .len();
        if cur < bytes {
            // Sparse by default: slot B stays abortable until the flip;
            // full allocation waits until slot A's space is freed.
            f.set_len(bytes)
                .map_err(|e| StackError::NotFound(format!("truncate {}: {e}", file.display())))?;
        }
        if prealloc {
            let rc = unsafe { libc::fallocate(f.as_raw_fd(), 0, 0, bytes as libc::off_t) };
            if rc != 0 {
                return Err(StackError::NotFound(format!(
                    "fallocate {}: {}",
                    file.display(),
                    std::io::Error::last_os_error()
                )));
            }
        }
        // losetup, ioctl-for-ioctl: LOOP_CTL_GET_FREE → LOOP_SET_FD →
        // best-effort LOOP_SET_STATUS64 with LO_FLAGS_DIRECT_IO.
        let ctl = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/loop-control")
            .map_err(|e| StackError::NotFound(format!("loop-control: {e}")))?;
        let n = unsafe { libc::ioctl(ctl.as_raw_fd(), LOOP_CTL_GET_FREE) };
        if n < 0 {
            return Err(StackError::NotFound(format!(
                "LOOP_CTL_GET_FREE: {}",
                std::io::Error::last_os_error()
            )));
        }
        let dev = format!("/dev/loop{n}");
        let ld = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&dev)
            .map_err(|e| StackError::NotFound(format!("open {dev}: {e}")))?;
        let rc = unsafe { libc::ioctl(ld.as_raw_fd(), LOOP_SET_FD, f.as_raw_fd()) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "LOOP_SET_FD {dev}: {}",
                std::io::Error::last_os_error()
            )));
        }
        let mut info = LoopInfo64::zeroed();
        info.lo_flags = LO_FLAGS_DIRECT_IO;
        let name_bytes = file.as_os_str().as_bytes();
        let take = name_bytes.len().min(63);
        info.lo_file_name[..take].copy_from_slice(&name_bytes[..take]);
        // Best-effort: direct IO needs an aligned backing fs; falling
        // back to the page cache is slower, never wrong.
        let _ = unsafe { libc::ioctl(ld.as_raw_fd(), LOOP_SET_STATUS64, &info) };
        Ok(dev)
    }

    fn ensure_preallocated(&self, file: &Path) -> Result<(), StackError> {
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(file)
            .map_err(|e| StackError::NotFound(format!("open {}: {e}", file.display())))?;
        let len = f
            .metadata()
            .map_err(|e| StackError::NotFound(e.to_string()))?
            .len();
        let rc = unsafe { libc::fallocate(f.as_raw_fd(), 0, 0, len as libc::off_t) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "fallocate {}: {}",
                file.display(),
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn backing_of(&self, file: &Path) -> Result<Option<String>, StackError> {
        // Identity is the INODE, not the recorded path. The kernel
        // stores the path a loop device was attached with and never
        // updates it, so after the swing's canonicalize (`.b` renamed
        // over the live name) the live pool's loop still ADVERTISES the
        // `.b` path in sysfs. Matching by path string here was wrong in
        // both directions from the second swing of a slot on: it missed
        // the live loop under the live name (leaking it through the
        // teardown), and — much worse — it MATCHED the live loop when
        // asked about the `.b` path, handing the next swing's pre-clean
        // the live pool's loop to release and its B build the live
        // pool's loop to zero fresh metadata onto. The mock has always
        // modeled rename as following the file; this is the kernel-side
        // half of that contract: stat the file and compare
        // `(st_dev, st_ino)` against each loop's `LOOP_GET_STATUS64`
        // identity, the same match `losetup --associated` makes.
        let c = cpath(file)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        if unsafe { libc::stat(c.as_ptr(), &mut st) } != 0 {
            // No file at this path — an attachment could only be found
            // through some OTHER path holding the inode, which is not
            // the question callers ask.
            return Ok(None);
        }
        // (The casts are load-bearing on 32-bit targets, as in
        // `backing_alloc`.)
        #[allow(clippy::unnecessary_cast)]
        let want = (st.st_dev as u64, st.st_ino as u64);
        let Ok(entries) = std::fs::read_dir("/sys/block") else {
            return Ok(None);
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if !name.starts_with("loop") {
                continue;
            }
            // /sys/block/loopN without a loop/ subdir is a detached
            // device — skip before opening the node.
            if !e.path().join("loop").exists() {
                continue;
            }
            let dev = format!("/dev/{name}");
            let Ok(fd) = std::fs::File::open(&dev) else {
                continue;
            };
            let mut info = LoopInfo64::zeroed();
            let rc = unsafe { libc::ioctl(fd.as_raw_fd(), LOOP_GET_STATUS64, &mut info) };
            if rc == 0 && (info.lo_device, info.lo_inode) == want {
                return Ok(Some(dev));
            }
        }
        Ok(None)
    }

    fn backing_exists(&self, file: &Path) -> Result<bool, StackError> {
        Ok(file.exists())
    }

    fn release_backing(&self, loop_dev: &str) -> Result<(), StackError> {
        let ld = std::fs::OpenOptions::new()
            .read(true)
            .open(loop_dev)
            .map_err(|e| StackError::NotFound(format!("open {loop_dev}: {e}")))?;
        let rc = unsafe { libc::ioctl(ld.as_raw_fd(), LOOP_CLR_FD, 0) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "LOOP_CLR_FD {loop_dev}: {}",
                std::io::Error::last_os_error()
            )));
        }
        Ok(())
    }

    fn delete_backing(&self, file: &Path) -> Result<(), StackError> {
        match std::fs::remove_file(file) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(StackError::NotFound(format!("rm {}: {e}", file.display()))),
        }
    }

    /// The swing's canonicalize step (§7). `rename(2)` replaces its
    /// destination silently, so a live slot-A file under the canonical
    /// name would be unlinked mid-swing with no error; `MockDmOps`
    /// refuses that case, and the kernel must enforce the same promise
    /// here. `RENAME_NOREPLACE` does exactly that; EEXIST maps to the
    /// mock's `NotFound("rename target … exists")` shape so the two
    /// executors are indistinguishable to the caller.
    fn rename_backing(&self, from: &Path, to: &Path) -> Result<(), StackError> {
        let (cfrom, cto) = (cpath(from)?, cpath(to)?);
        // Raw syscall, not glibc's wrapper: the wrapper only exists from
        // glibc 2.28 and this binary is built against whatever the node
        // image ships.
        let rc = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                libc::AT_FDCWD,
                cfrom.as_ptr(),
                libc::AT_FDCWD,
                cto.as_ptr(),
                libc::RENAME_NOREPLACE,
            )
        };
        if rc == 0 {
            return Ok(());
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EEXIST) | Some(libc::ENOTEMPTY) => Err(StackError::NotFound(format!(
                "rename target {} exists",
                to.display()
            ))),
            // Kernels < 3.15, and filesystems whose rename op predates
            // the flags argument (some overlay/network mounts), answer
            // ENOSYS/EINVAL for ANY flag. Fall back to look-then-rename.
            // That check is racy in general, but not here: §7 gives one
            // agent per volume and the slot filenames are volume-scoped,
            // so the only writer that could create `to` between the two
            // calls is this same swing.
            Some(libc::ENOSYS) | Some(libc::EINVAL) => {
                if to.symlink_metadata().is_ok() {
                    return Err(StackError::NotFound(format!(
                        "rename target {} exists",
                        to.display()
                    )));
                }
                std::fs::rename(from, to).map_err(|e| {
                    StackError::NotFound(format!(
                        "rename {} → {}: {e}",
                        from.display(),
                        to.display()
                    ))
                })
            }
            _ => Err(StackError::NotFound(format!(
                "rename {} → {}: {err}",
                from.display(),
                to.display()
            ))),
        }
    }

    fn fs_free_bytes(&self, dir: &Path) -> Result<u64, StackError> {
        let c = cpath(dir)?;
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "statvfs {}: {}",
                dir.display(),
                std::io::Error::last_os_error()
            )));
        }
        // Available-to-unprivileged blocks, matching what an agent-run
        // fallocate could actually get. (The casts are load-bearing on
        // 32-bit targets, where statvfs fields are not u64.)
        #[allow(clippy::unnecessary_cast)]
        Ok(st.f_bavail as u64 * st.f_frsize as u64)
    }

    fn set_discard_max_bytes(&self, name: &str, bytes: u64) -> Result<(), StackError> {
        let knob = self.discard_knob(name)?;
        std::fs::write(&knob, format!("{bytes}\n")).map_err(|e| absent_or(&knob, e))
    }

    fn discard_max_bytes(&self, name: &str) -> Result<u64, StackError> {
        let knob = self.discard_knob(name)?;
        let raw = std::fs::read_to_string(&knob).map_err(|e| absent_or(&knob, e))?;
        raw.trim()
            .parse()
            .map_err(|_| StackError::NotFound(format!("{knob}: unparsable {raw:?}")))
    }

    fn backing_alloc(&self, file: &Path) -> Result<(u64, u64), StackError> {
        // §8.1 rule 2's sparseness discriminator, straight from the
        // inode: `st_size` is what the pool addresses, `st_blocks × 512`
        // is what the filesystem has actually committed to it.
        let c = cpath(file)?;
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::stat(c.as_ptr(), &mut st) };
        if rc != 0 {
            return Err(StackError::NotFound(format!(
                "stat {}: {}",
                file.display(),
                std::io::Error::last_os_error()
            )));
        }
        // `st_blocks` is 512-byte units by POSIX definition, regardless
        // of the filesystem's own block size. (The casts are
        // load-bearing on 32-bit targets.)
        #[allow(clippy::unnecessary_cast)]
        Ok((st.st_size as u64, st.st_blocks as u64 * 512))
    }

    /// Mappings (chunk-granular runs) of one thin device, read under a
    /// reserved metadata snapshot.
    fn thin_mappings(
        &self,
        metadata_dev: &Path,
        dev_id: u64,
    ) -> Result<Vec<(u64, u64, u64)>, StackError> {
        use thinp::io_engine::{IoEngine, SyncIoEngine};
        use thinp::pdata::btree_walker::btree_to_map;
        use thinp::thin::delta::get_mappings;
        use thinp::thin::superblock::read_superblock_snap;

        // NON-exclusive open (excl=false): the tmeta device is held by the
        // live thin-pool target, so O_EXCL fails with EBUSY. Reading under a
        // reserved metadata snapshot is exactly the case upstream thin_delta
        // opens non-exclusively for (EngineBuilder.exclusive(!metadata_snap)).
        let engine = SyncIoEngine::new_with(metadata_dev, false, false)
            .map_err(|e| StackError::NotFound(format!("tmeta open: {e}")))?;
        let engine: std::sync::Arc<dyn IoEngine + Send + Sync> = std::sync::Arc::new(engine);
        let sb = read_superblock_snap(engine.as_ref())
            .map_err(|e| StackError::NotFound(format!("tmeta superblock: {e}")))?;
        let mut path = Vec::new();
        let roots = btree_to_map::<u64>(&mut path, engine.as_ref(), false, sb.mapping_root)
            .map_err(|e| StackError::NotFound(format!("device tree: {e}")))?;
        let root = *roots
            .get(&dev_id)
            .ok_or_else(|| StackError::NotFound(format!("dev {dev_id} not in metadata snap")))?;
        let maps = get_mappings(engine, root)
            .map_err(|e| StackError::NotFound(format!("mappings: {e}")))?;
        Ok(maps
            .iter()
            .map(|m| (m.thin_begin, m.data_begin, m.len))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// §8.2 step 3's whole decision, as a table. Everything else in this
    /// module needs a kernel; this does not, which is the reason it was
    /// split out of `freeze_fs` at all.
    ///
    /// These tests DO run under CI's plain `cargo test --workspace`,
    /// via feature unification: `dm` is not a default feature, but
    /// `bins/skynbd-csi-driver` and `bins/skynbd-agent` both depend on
    /// this crate with `features = ["dm"]`, so a workspace build turns
    /// it on. Measured — the lib target runs 52 tests without the
    /// feature and 56 with it, and a workspace run reports 56.
    ///
    /// That is worth knowing rather than relying on: it rests on a
    /// dependency edge, not on an explicit `--features dm` anywhere in
    /// `ci.yml`, so a resolver or dependency change could take it away
    /// silently. The BEHAVIOUR classified here is covered
    /// feature-independently in `tests/wedged_capture.rs` through
    /// `MockDmOps` regardless.
    #[test]
    fn freeze_errnos_classify() {
        let mount = Path::new("/mnt/x");
        let at = |e: i32| classify_freeze_errno(&std::io::Error::from_raw_os_error(e), mount);

        // The filesystem has shut down: proceed crash-consistently.
        for e in [libc::EIO, libc::EROFS] {
            assert!(
                matches!(at(e), StackError::Wedged(_)),
                "errno {e} must be Wedged, got {:?}",
                at(e)
            );
        }
        // No freeze support on the superblock. `EOPNOTSUPP` is the row
        // that actually fires — measured on tmpfs, which answers
        // errno 95 — and an earlier draft carried only `ENOTTY`, which
        // left this arm unreachable and sent every freeze-less mount to
        // the catch-all failure arm instead.
        assert_eq!(libc::EOPNOTSUPP, 95, "the errno tmpfs was measured at");
        assert!(matches!(at(libc::EOPNOTSUPP), StackError::Unsupported(_)));
        assert!(matches!(at(libc::ENOTTY), StackError::Unsupported(_)));
        // Transients and deployment faults must propagate as failures —
        // above all EBUSY, which is a race, not a wedged filesystem.
        for e in [libc::EBUSY, libc::EPERM, libc::EINVAL, libc::ENOMEM] {
            let got = at(e);
            assert!(
                !matches!(got, StackError::Wedged(_) | StackError::Unsupported(_)),
                "errno {e} must not take the crash-consistent path, got {got:?}"
            );
        }
    }

    /// The classifier both halves of the write probe share. Pure, so
    /// it needs neither a kernel nor a mount — and its errnos are
    /// measured: a read-only tmpfs holding a leftover answers `EEXIST`
    /// to the create and `EROFS` to the unlink.
    #[test]
    fn only_a_refusing_superblock_is_a_definite_negative() {
        let at = |e: i32| refuses_writes(&std::io::Error::from_raw_os_error(e));
        assert!(at(libc::EROFS), "an aborted or read-only super");
        assert!(at(libc::EIO), "a shut-down XFS");
        // `ENOSPC` is the one that must NOT be a verdict: a full pool
        // is what this subsystem exists to survive.
        assert!(!at(libc::ENOSPC));
        for e in [libc::EEXIST, libc::EACCES, libc::EPERM, libc::ENOENT] {
            assert!(!at(e), "errno {e} says nothing about the superblock");
        }
    }

    /// The write probe's leftover path. Needs a directory, not a
    /// kernel, so it runs wherever this module compiles.
    #[test]
    fn a_leftover_probe_file_does_not_disable_the_check() {
        let dir = tempfile::tempdir().unwrap();
        let probe = dir.path().join(".skynbd-rw-probe");
        std::fs::write(&probe, b"stale").unwrap();
        assert!(probe.exists());

        assert!(
            write_probe(dir.path()).unwrap(),
            "a leftover must be cleared, not read as inconclusive"
        );
        assert!(
            !probe.exists(),
            "and the probe cleans up after itself either way"
        );

        // Second call on a clean directory still works.
        assert!(write_probe(dir.path()).unwrap());
        assert!(!probe.exists());
    }

    /// The errno has to survive into the message: `Fault(&'static str)`
    /// cannot carry one, and losing it is what made a failed freeze
    /// unactionable in the first place.
    #[test]
    fn the_errno_reaches_the_caller() {
        let mount = Path::new("/mnt/x");
        let msg = classify_freeze_errno(&std::io::Error::from_raw_os_error(libc::EBUSY), mount)
            .to_string();
        assert!(msg.contains("/mnt/x"), "{msg}");
        assert!(
            msg.contains(&std::io::Error::from_raw_os_error(libc::EBUSY).to_string()),
            "{msg}"
        );
    }
}

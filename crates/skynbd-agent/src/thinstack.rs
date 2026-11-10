//! Thin-mode stack seam (CSI design 6h, origin-advance §5): consumers
//! build `fs → [read cache] → shim → thin origin → local thin pool`
//! through [`crate::provision`] instead of shelling dm-setup. The trait
//! exists so the CSI node plugin and the workstation CLI (`mount
//! --thin`) plan thin stages against a recorder in plain `cargo test`;
//! the real implementation drives kernel ioctls via `LinuxDmOps`.

use crate::provision::ThinStackSpec;

pub trait ThinStack: Send + Sync {
    /// Build (or resume building) the stack; idempotent.
    fn build(&self, spec: &ThinStackSpec) -> Result<(), String>;
    /// Is the stack's unconditional device set complete on this host?
    /// The driver-restart replay keys on this: dm state survives a
    /// process restart and dies with a reboot, exactly the line the
    /// replay needs to draw — and, publish-only stacks having no NBD
    /// head, there is no attachment to probe instead.
    fn built(&self, volume: &str) -> Result<bool, String>;
    /// Remove the stack, consumers first. There is nothing to flush:
    /// the pool is local and the D-M13 final cycle IS the durability
    /// step; the read cache, when present, is writethrough and
    /// therefore never dirty (§7.1).
    fn teardown(&self, spec: &ThinStackSpec) -> Result<(), String>;
    /// Invariant B: re-derive the guest-discard mask on this stack's
    /// shim, for callers that skip [`ThinStack::build`] and therefore
    /// skip the assert built into it. The driver-restart replay is that
    /// caller: it finds a live stack and returns before the build
    /// runs, so without this the doc's "a shim rebuilt outside `build`
    /// is re-masked from observation rather than trusted" would hold
    /// only for cold builds.
    fn assert_guest_discard_mask(&self, spec: &ThinStackSpec) -> Result<(), String>;
}

/// Kernel-backed implementation.
#[cfg(feature = "dm")]
pub struct DmThin {
    ops: crate::dm_linux::LinuxDmOps,
}

#[cfg(feature = "dm")]
impl DmThin {
    pub fn new() -> Result<Self, String> {
        Ok(Self {
            ops: crate::dm_linux::LinuxDmOps::new().map_err(|e| e.to_string())?,
        })
    }
}

#[cfg(feature = "dm")]
impl ThinStack for DmThin {
    fn build(&self, spec: &ThinStackSpec) -> Result<(), String> {
        crate::provision::build(&self.ops, spec).map_err(|e| e.to_string())?;
        // mkfs/mount resolve devices via /dev/mapper, which raw DM
        // ioctls never populate — create the nodes instead of racing
        // udev. The mount device (read cache when present, shim
        // otherwise) is what the CSI node mounts; the origin node stays
        // for unmounted rigs and diagnostics (the discard port needs
        // none — the agent opens it by device number).
        let names = spec.names();
        let mut nodes = vec![names.origin.clone(), names.fs.clone()];
        if spec.read_cache() {
            nodes.push(names.rcache.clone());
        }
        for name in &nodes {
            self.ops
                .ensure_mapper_node(name)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn built(&self, volume: &str) -> Result<bool, String> {
        Ok(matches!(
            crate::provision::observe(&self.ops, volume).map_err(|e| e.to_string())?,
            crate::provision::StackState::Complete
        ))
    }

    fn assert_guest_discard_mask(&self, spec: &ThinStackSpec) -> Result<(), String> {
        let names = spec.names();
        crate::dm::mask_guest_discard(&self.ops, &names.fs).map_err(|e| e.to_string())?;
        // On a cached stack the guest sees the rcache, not the shim —
        // dm-cache advertises its own (nonzero) virtual discard limits
        // with passdown disabled — so the guest-facing mask has to be
        // re-derived on the mount device too (§7.1 / Invariant B).
        if spec.read_cache() {
            crate::dm::mask_guest_discard(&self.ops, &names.rcache).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn teardown(&self, spec: &ThinStackSpec) -> Result<(), String> {
        crate::provision::teardown(&self.ops, &spec.volume).map_err(|e| e.to_string())
    }
}

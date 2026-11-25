//! What the agent asks of ZFS (the plan of M2.6, §1 and §6), as a trait
//! so that the engine runs on a fake pool in its tests and on `zfs` and
//! `zpool` in the program (M2.6c), and the premise: whether a pool's
//! layout and state allow a publish.
//!
//! # The premise
//!
//! A cut is a complete pool only where the volume holds the whole pool
//! (design §5.2): the pool is one plain data device, this volume, with
//! its log device. So a publish is refused unless:
//!
//! - the data class is exactly one top-level vdev, a leaf disk whose
//!   device is the volume's (its device number, `st_rdev`, is the one
//!   of the daemon's `/dev/ublkbN`);
//! - there is no special or dedup vdev;
//! - no resilver, rebuild or removal is under way, and no vdev was
//!   removed (no indirect vdev: conservative);
//! - the pool and the leaf are the ones the agent enrolled, where it
//!   has enrolled one (the lead's Q4);
//! - `multihost=off`, there is no checkpoint, the pool is not read-only,
//!   and `zfs_nocacheflush` is 0.
//!
//! Log, cache and spare devices are allowed (the lead's Q3): the log is
//! not in a cut (design D2), and a cache or a spare not in use holds
//! none of the pool's blocks. A spare in use is a `spare` vdev in the
//! data class, and is refused. A change of the layout that breaks the
//! premise while a publish is under way gives it up (the planner's
//! `Premise`), withdrawing a request that pends.

use std::collections::BTreeSet;
use std::fmt;

use crate::manifest;
use crate::record::{Enrolled, Member};

/// The tag of the agent's hold on a snapshot it publishes (the lead's
/// Q7).
pub const HOLD_TAG: &str = "skyzfs-publish";

/// A pool's vdevs, as `zpool status -P` shows them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// The top-level vdevs of the data class.
    pub data: Vec<Vdev>,
    /// The top-level vdevs of the special class.
    pub special: usize,
    /// The top-level vdevs of the dedup class.
    pub dedup: usize,
    /// The log, cache and spare devices: allowed, counted for reports.
    pub other: usize,
    /// A resilver, rebuild or removal under way, in words; `None` where
    /// there is none.
    pub busy: Option<String>,
}

/// A vdev.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vdev {
    /// Its kind.
    pub kind: VdevKind,
    /// Its GUID.
    pub guid: u64,
    /// A leaf's device number (`st_rdev` of its path), where it could be
    /// read; `None` for an interior vdev.
    pub rdev: Option<u64>,
    /// An interior vdev's children.
    pub children: Vec<Vdev>,
}

/// The kinds of vdev the premise tells apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VdevKind {
    /// A leaf on a block device.
    Disk,
    /// A leaf on a file.
    File,
    /// A mirror.
    Mirror,
    /// A raidz group.
    Raidz,
    /// A dRAID group.
    Draid,
    /// A leaf being replaced, with its replacement.
    Replacing,
    /// A spare in use, with the leaf it stands in for.
    Spare,
    /// What a removed vdev leaves.
    Indirect,
    /// Another kind, by its name.
    Other(String),
}

/// A pool's properties that the premise and the planner read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Props {
    /// `guid`.
    pub guid: u64,
    /// `load_guid`: changes at each import (the plan's rule (L)).
    pub load_guid: u64,
    /// `multihost=on`.
    pub multihost: bool,
    /// Whether the pool has a checkpoint.
    pub checkpoint: bool,
    /// `readonly=on`.
    pub readonly: bool,
}

/// Why a pool is refused a publish ([module docs](self)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The data class is not one leaf disk.
    Layout(String),
    /// The leaf is not the volume's device.
    Device {
        /// The leaf's device number, if read.
        leaf: Option<u64>,
        /// The volume's.
        volume: u64,
    },
    /// A special or dedup vdev.
    Class,
    /// A resilver, rebuild or removal under way, or a vdev removed.
    Busy(String),
    /// Another pool or leaf than the one enrolled.
    Enrolled,
    /// `multihost=on`.
    Multihost,
    /// A checkpoint.
    Checkpoint,
    /// Read-only.
    Readonly,
    /// `zfs_nocacheflush` is set.
    Nocacheflush,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::Layout(what) => write!(f, "the data class is not one leaf disk: {what}"),
            Refusal::Device { leaf, volume } => match leaf {
                Some(leaf) => write!(f, "the leaf is device {leaf:#x}, the volume {volume:#x}"),
                None => write!(f, "the leaf's device could not be read"),
            },
            Refusal::Class => write!(f, "the pool has a special or dedup vdev"),
            Refusal::Busy(what) => write!(f, "{what}"),
            Refusal::Enrolled => write!(f, "not the pool and leaf the agent enrolled"),
            Refusal::Multihost => write!(f, "multihost is on"),
            Refusal::Checkpoint => write!(f, "the pool has a checkpoint"),
            Refusal::Readonly => write!(f, "the pool is read-only"),
            Refusal::Nocacheflush => write!(f, "zfs_nocacheflush is set"),
        }
    }
}

/// The premise ([module docs](self)) for `pool` on the volume whose
/// device number is `device`. The leaf's GUID where it holds, for the
/// enrolment.
pub fn premise(
    layout: &Layout,
    props: &Props,
    nocacheflush: bool,
    device: u64,
    pool: &str,
    enrolled: Option<&Enrolled>,
) -> Result<u64, Refusal> {
    let leaf = match layout.data.as_slice() {
        [leaf] => leaf,
        data => return Err(Refusal::Layout(format!("{} top-level vdevs", data.len()))),
    };
    if leaf.kind != VdevKind::Disk || !leaf.children.is_empty() {
        return Err(Refusal::Layout(format!("a {:?} vdev", leaf.kind)));
    }
    if leaf.rdev != Some(device) {
        return Err(Refusal::Device {
            leaf: leaf.rdev,
            volume: device,
        });
    }
    if layout.special > 0 || layout.dedup > 0 {
        return Err(Refusal::Class);
    }
    if let Some(what) = &layout.busy {
        return Err(Refusal::Busy(what.clone()));
    }
    if let Some(enrolled) = enrolled {
        if enrolled.pool != pool
            || enrolled.pool_guid != props.guid
            || enrolled.leaf_guid != leaf.guid
        {
            return Err(Refusal::Enrolled);
        }
    }
    if props.multihost {
        return Err(Refusal::Multihost);
    }
    if props.checkpoint {
        return Err(Refusal::Checkpoint);
    }
    if props.readonly {
        return Err(Refusal::Readonly);
    }
    if nocacheflush {
        return Err(Refusal::Nocacheflush);
    }
    Ok(leaf.guid)
}

/// What the channel program read in the group that took the snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// `createtxg` of `pool@t`.
    pub createtxg: u64,
    /// Each member: the pool's root, then its descendants.
    pub members: Vec<manifest::Member>,
}

/// Why ZFS did not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZfsError {
    /// The pool is not imported.
    NoPool,
    /// The snapshot does not exist.
    NoSnapshot,
    /// A member's `@t` exists already: the program took nothing.
    Exists,
    /// Anything else, in words.
    Other(String),
}

impl fmt::Display for ZfsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ZfsError::NoPool => write!(f, "no such pool"),
            ZfsError::NoSnapshot => write!(f, "no such snapshot"),
            ZfsError::Exists => write!(f, "a snapshot of that name exists"),
            ZfsError::Other(what) => write!(f, "{what}"),
        }
    }
}

impl std::error::Error for ZfsError {}

/// What the agent asks of ZFS (the plan of M2.6, §1). Snapshots are
/// named by the pool and `t`: `pool@t` and each descendant's `@t`.
pub trait Zfs {
    /// The pool's vdevs, each leaf's device number read.
    fn layout(&mut self, pool: &str) -> Result<Layout, ZfsError>;
    /// The pool's properties.
    fn props(&mut self, pool: &str) -> Result<Props, ZfsError>;
    /// Whether the module parameter `zfs_nocacheflush` is set.
    fn nocacheflush(&mut self) -> Result<bool, ZfsError>;
    /// The channel program: take `@t` of the pool and every descendant
    /// in one group, and read the manifest's content in it. A member's
    /// `@t` that exists fails it as a whole ([`ZfsError::Exists`]).
    fn snapshot(&mut self, pool: &str, t: &str) -> Result<Program, ZfsError>;
    /// Each member's `@t` that exists, with its GUID; `None` where
    /// `pool@t` does not.
    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError>;
    /// `zfs hold -r TAG pool@t`; a hold already there is held.
    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError>;
    /// `zfs release -r TAG pool@t`; no such hold, or no such snapshot,
    /// is released.
    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError>;
    /// Whether `pool@t` has the hold `tag`; `false` where it does not
    /// exist.
    fn held(&mut self, pool: &str, t: &str, tag: &str) -> Result<bool, ZfsError>;
    /// The names `t` of every snapshot in the pool, of any dataset, that
    /// has the hold `tag`.
    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError>;
    /// `zpool sync pool`: a group written.
    fn sync(&mut self, pool: &str) -> Result<(), ZfsError>;
}

#[cfg(test)]
mod tests;

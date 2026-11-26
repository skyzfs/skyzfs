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
//! - no resilver or rebuild is under way, and `zpool status` shows no
//!   `remove:` line: no vdev is being removed or was ever removed (the
//!   lead's L4 on #147; `zpool status` never shows an indirect vdev);
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

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::manifest::{self, Kind};
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
    /// The `remove:` line of a vdev removal, under way or done; `None`
    /// where there is none.
    pub removed: Option<String>,
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
    /// A resilver, rebuild or removal under way.
    Busy(String),
    /// A vdev removed, or being removed (its `remove:` line).
    Removed(String),
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
            Refusal::Removed(what) => write!(f, "a vdev was removed: {what}"),
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
    if let Some(what) = &layout.removed {
        return Err(Refusal::Removed(what.clone()));
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

/// A file system or volume of the pool, as the members program reads it
/// in full ([`Zfs::datasets`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dataset {
    /// Its name.
    pub name: String,
    /// A file system or a volume.
    pub kind: Kind,
    /// Its GUID.
    pub guid: u64,
    /// Its `createtxg`.
    pub createtxg: u64,
    /// Its properties set locally, as they are when read: ZFS keeps on a
    /// snapshot only the properties set on the snapshot itself, and reads
    /// any other through its dataset (`dsl_prop_get_ds`).
    pub local: BTreeMap<String, Vec<u8>>,
    /// Its `@t`, where it exists.
    pub at: Option<Snap>,
}

/// A member's `@t`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Snap {
    /// Its GUID.
    pub guid: u64,
    /// Its `createtxg`.
    pub createtxg: u64,
}

/// A take's snapshots as read after it, whole: what the manifest holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Program {
    /// `createtxg` of `pool@t`.
    pub createtxg: u64,
    /// Each member: the pool's root, then its descendants.
    pub members: Vec<manifest::Member>,
}

/// What [`taken`] finds of `pool@t`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Taken {
    /// `pool@t` does not exist.
    Absent,
    /// `pool@t` exists, and its members are not one take of the pool:
    /// the names of the datasets that break it (an `@t` of another group,
    /// or none on a dataset created in the take's group or before), in
    /// the order read, or the pool's own name where its root was not read.
    NotWhole(Vec<String>),
    /// One take of the pool: its manifest's content.
    Whole(Program),
}

/// Whether `datasets` (the pool's, read after a take of `@t`) hold one
/// take of the pool (design §6.2; the lead's rule for an `EEXIST` at the
/// take, M2.6c-2b):
///
/// - every member's `@t` has the `createtxg` of `pool@t`;
/// - every dataset created in that group or before has an `@t`.
///
/// A dataset created after that group has none and is not a member. One
/// created in the take's own group after `zfs snapshot -r` listed the
/// pool's datasets, or before its group (a race of a few milliseconds),
/// has none either, and is read as not whole: the conservative reading
/// (the lead's, M2.6c-2b), which costs a take under another name. So is
/// a dataset that `zfs snapshot -r` skips (libzfs skips one that is
/// inconsistent, a full `zfs receive` into it under way or interrupted,
/// with its children; D1 of #153's review): it is not exempted, and a
/// pool that holds one fails visibly, given up as
/// [`crate::record::Why::NotWhole`] (the lead's ruling).
pub fn taken(datasets: &[Dataset], pool: &str) -> Taken {
    let Some(root) = datasets.iter().find(|d| d.name == pool) else {
        return Taken::NotWhole(vec![pool.to_string()]);
    };
    let Some(root_at) = root.at else {
        return Taken::Absent;
    };
    let group = root_at.createtxg;
    let mut members = Vec::new();
    let mut breaking = Vec::new();
    for dataset in datasets {
        match dataset.at {
            Some(at) if at.createtxg != group => breaking.push(dataset.name.clone()),
            Some(at) => members.push(manifest::Member {
                name: dataset.name.clone(),
                kind: dataset.kind,
                guid: dataset.guid,
                snapshot_guid: at.guid,
                local: dataset.local.clone(),
            }),
            None if dataset.createtxg <= group => breaking.push(dataset.name.clone()),
            None => {}
        }
    }
    if !breaking.is_empty() {
        return Taken::NotWhole(breaking);
    }
    members.sort_by(|a, b| (a.name != pool, &a.name).cmp(&(b.name != pool, &b.name)));
    Taken::Whole(Program {
        createtxg: group,
        members,
    })
}

/// Where the agent's hold is on a publish's snapshot ([`Zfs::held`]).
///
/// The planner holds again unless it is [`Held::All`], and releases
/// wherever it is not [`Held::No`]: a hold on some members only (a
/// person released the others, or a member's `@t` came after the hold)
/// is released when the publish ends, as a whole one is (TL-F1 of
/// #147's review).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// No member's `@t` has the tag.
    No,
    /// Some members' `@t` have it and others not, or `pool@t` does not
    /// exist and some descendant's `@t` has it (a person released
    /// `pool@t` and destroyed it alone: FL-F1 of #147's review).
    Part,
    /// Every member's `@t` has it.
    All,
}

/// Why ZFS did not do what was asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ZfsError {
    /// The pool is not imported.
    NoPool,
    /// The snapshot does not exist.
    NoSnapshot,
    /// A member's `@t` exists already: `zfs snapshot -r` took nothing.
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
    /// How many file systems and volumes the pool has. The count sets
    /// the time limits of the calls that follow it, until the next count
    /// (the adapter's `Limits`; the lead's rule on M2.6c-2b). Where it
    /// fails, they are the bases (N2 of #153's review: the round's own
    /// calls then say what is wrong). The engine counts once at the start
    /// of each round, before the sweep.
    fn count(&mut self, pool: &str) -> Result<u64, ZfsError>;
    /// `zfs snapshot -r pool@t`: `@t` of the pool and every descendant
    /// in one group, or none. A member's `@t` that exists fails it as a
    /// whole ([`ZfsError::Exists`]).
    fn snapshot(&mut self, pool: &str, t: &str) -> Result<(), ZfsError>;
    /// Every file system and volume of the pool, with its `@t` where it
    /// exists: what [`taken`] reads.
    fn datasets(&mut self, pool: &str, t: &str) -> Result<Vec<Dataset>, ZfsError>;
    /// `zfs destroy -r pool@t`; no such snapshot is destroyed.
    fn destroy(&mut self, pool: &str, t: &str) -> Result<(), ZfsError>;
    /// Each member's `@t` that exists, with its GUID; `None` where
    /// `pool@t` does not.
    fn members(&mut self, pool: &str, t: &str) -> Result<Option<Vec<Member>>, ZfsError>;
    /// `zfs hold -r TAG pool@t`; a hold already there is held.
    fn hold(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError>;
    /// `zfs release -r TAG pool@t`; no such hold, or no such snapshot,
    /// is released.
    fn release(&mut self, pool: &str, t: &str, tag: &str) -> Result<(), ZfsError>;
    /// Where the hold `tag` is on the `@t` of the pool and its
    /// descendants, whether `pool@t` exists or not ([`Held`]). `members`
    /// are `@t`'s members as [`Zfs::members`] read them in the same round
    /// (`None` where `pool@t` did not exist), which this does not read
    /// again (TL-S2 of #147's review: at 10 000 file systems the read
    /// took 24 s). A member's `@t` made between the two reads is seen at
    /// the next round.
    fn held(
        &mut self,
        pool: &str,
        t: &str,
        tag: &str,
        members: Option<&[Member]>,
    ) -> Result<Held, ZfsError>;
    /// The names `t` of every snapshot in the pool, of any dataset, that
    /// has the hold `tag`.
    fn tagged(&mut self, pool: &str, tag: &str) -> Result<BTreeSet<String>, ZfsError>;
    /// The agent's sync task: `zfs set skyzfs:assist=<value> pool`, then
    /// `zpool sync pool`. The property is a change of the pool's own: the
    /// set is a sync task and waits for its own group to be written. A
    /// plain `zpool sync` writes nothing on an idle pool: ZFS commits a
    /// group with no changes and writes no uberblock for it (M2.T1; N1 of
    /// #142's review). `value` differs at each call, so that no call
    /// sets the property to what it already is. The kernel probe of #145
    /// (ZFS 2.4.1, one run on each of Ubuntu 24.04 and 26.04) saw each
    /// call write two groups, the set's own, written before the set
    /// returns, and the next, which the sync writes with the set's
    /// history record (about 310 KiB each); a bare `zpool sync` write
    /// none; and, through the daemon, the second call's set carry a
    /// waiting publish about 11 s after it was accepted. A set to the
    /// value the property already holds wrote two groups as well, as for
    /// a new value, so a new value at each call is not needed on 2.4.1;
    /// it is kept, as it costs nothing.
    fn assist(&mut self, pool: &str, value: u64) -> Result<(), ZfsError>;
}

/// The user property of the agent's sync task ([`Zfs::assist`]).
pub const ASSIST_PROPERTY: &str = "skyzfs:assist";

pub mod cli;

#[cfg(test)]
mod tests;

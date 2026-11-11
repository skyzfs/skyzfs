//! `SimDisk`: a disk in memory that fails the way disks fail.
//!
//! It implements [`Dir`] and [`File`] and keeps, beside what a reader
//! sees now, what would be left after a power cut. Everything it does at
//! random comes from a seed, so a failing test can be replayed.
//!
//! What it models:
//!
//! - **Unsynced data.** A write is visible at once and durable only after
//!   a successful [`File::sync`] of that file. Until then the disk keeps,
//!   for every [`SECTOR`] the write touched, each version written since
//!   the last sync.
//! - **A power cut** ([`SimDisk::power_cut`]). Every sector with unsynced
//!   versions ends up, independently of every other, as its durable
//!   content, as any one of the unsynced versions, or as garbage: a later
//!   sector can survive while an earlier one is lost, and a write that
//!   spans sectors can be torn at any sector boundary. Which, is the
//!   [`Cut`]'s choice. A file's length ends up as it was at the last
//!   sync or as it is now. Of the creations, removals and renames since
//!   the last [`Dir::sync`], a prefix survives. What is left is durable,
//!   and every handle opened before the cut is dead.
//! - **A process kill** ([`SimDisk::kill`]). Every handle is dead and
//!   nothing is lost: what was written is still there to read. It is
//!   still unsynced, so a later power cut can take it.
//! - **Injected errors** ([`SimDisk::fail_write`], [`SimDisk::fail_sync`]).
//!   A failed write may have written any subset of its sectors. A failed
//!   sync makes nothing durable, and each unsynced sector goes one of
//!   three ways, as kernels do with pages they could not write. It stays
//!   unsynced, and a later sync may still write it. Or it is dropped: the
//!   file reads as it did before, without any power cut. Or it is marked
//!   clean while only the cache holds it: [`File::read_at`] goes on
//!   showing the data, also after a process kill, a later sync succeeds
//!   without writing it, and only [`File::read_device_at`] shows that the
//!   disk never got it. A power cut, or [`SimDisk::drop_caches`], ends
//!   the illusion.
//!
//! - **The write unit** ([`SimDisk::set_write_unit`]). A kernel writes
//!   back whole pages and a disk whole physical sectors: a write of a
//!   few bytes rewrites everything around them in the same unit. With a
//!   write unit set, every sector of every unit a write touches counts
//!   as written again, with the same content, and so is the power
//!   cut's to lose or to turn into garbage.
//!
//! What it does not model: a disk that reports a sync it did not do, a
//! write that lands in the wrong place, corruption of durable data on its
//! own (a test can corrupt a file itself, with [`SimDisk::corrupt`]),
//! running out of space, and a file system without a from-the-device
//! read (a test can switch it off, with [`SimDisk::set_device_reads`]).

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::io::{Dir, File, SECTOR};

const SECTOR_USIZE: usize = SECTOR as usize;

/// SplitMix64: small, seedable, and good enough to pick what survives.
#[derive(Debug, Clone)]
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A number below `n`, or 0 if `n` is 0.
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next() % n
        }
    }

    fn coin(&mut self) -> bool {
        self.next() & 1 == 1
    }
}

/// What a power cut leaves of one sector that has unsynced versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fate {
    /// The content at the last sync: everything since is lost.
    Durable,
    /// The content after the `n`-th write to the sector since the last
    /// sync, counting from 0. A number beyond the last write means the
    /// last.
    Version(usize),
    /// The content after the last write: nothing is lost.
    Latest,
    /// Neither old nor new: bytes that were never written.
    Garbage,
}

/// The choices a power cut makes. [`SeededCut`], [`LoseAll`] and
/// [`KeepAll`] are ready made; [`BySector`] takes a closure.
pub trait Cut {
    /// What is left of sector `index` of the file called `name`, which has
    /// `versions` unsynced versions. `name` is the file's name just before
    /// the cut, or `None` if it had been removed.
    fn sector(&mut self, name: Option<&str>, index: u64, versions: usize) -> Fate;

    /// How many of the `pending` creations, removals and renames since the
    /// last directory sync survive: the first so many. Anything above
    /// `pending` means all.
    fn names(&mut self, pending: usize) -> usize;

    /// Whether a file whose length changed since its last sync keeps the
    /// new length.
    fn length(&mut self, name: Option<&str>) -> bool;
}

/// A power cut that loses everything that was not synced.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoseAll;

impl Cut for LoseAll {
    fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> Fate {
        Fate::Durable
    }
    fn names(&mut self, _: usize) -> usize {
        0
    }
    fn length(&mut self, _: Option<&str>) -> bool {
        false
    }
}

/// A power cut that happens to lose nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct KeepAll;

impl Cut for KeepAll {
    fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> Fate {
        Fate::Latest
    }
    fn names(&mut self, pending: usize) -> usize {
        pending
    }
    fn length(&mut self, _: Option<&str>) -> bool {
        true
    }
}

/// A power cut that decides everything from a seed: each sector's fate is
/// drawn independently among its durable content and its unsynced
/// versions, and, if `garbage` is set, sometimes garbage.
#[derive(Debug, Clone)]
pub struct SeededCut {
    rng: Rng,
    garbage: bool,
}

impl SeededCut {
    /// A cut drawn from `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed ^ 0xC2A5_11ED_0D15_C0DE),
            garbage: false,
        }
    }

    /// The same, where about one lost sector in four is garbage instead
    /// of its old content.
    pub fn with_garbage(seed: u64) -> Self {
        Self {
            garbage: true,
            ..Self::new(seed)
        }
    }
}

impl Cut for SeededCut {
    fn sector(&mut self, _: Option<&str>, _: u64, versions: usize) -> Fate {
        let pick = self.rng.below(versions as u64 + 1) as usize;
        if pick == versions {
            if self.garbage && self.rng.below(4) == 0 {
                Fate::Garbage
            } else {
                Fate::Durable
            }
        } else {
            Fate::Version(pick)
        }
    }
    fn names(&mut self, pending: usize) -> usize {
        self.rng.below(pending as u64 + 1) as usize
    }
    fn length(&mut self, _: Option<&str>) -> bool {
        self.rng.coin()
    }
}

/// A power cut whose sectors are decided by a closure of the file's name
/// and the sector's index. Names and lengths survive.
pub struct BySector<F>(pub F);

impl<F: FnMut(Option<&str>, u64) -> Fate> Cut for BySector<F> {
    fn sector(&mut self, name: Option<&str>, index: u64, _: usize) -> Fate {
        (self.0)(name, index)
    }
    fn names(&mut self, pending: usize) -> usize {
        pending
    }
    fn length(&mut self, _: Option<&str>) -> bool {
        true
    }
}

/// How many operations a [`SimDisk`] has carried out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    /// Calls of [`File::write_at`], failed ones included.
    pub writes: u64,
    /// Calls of [`File::sync`] that succeeded.
    pub syncs: u64,
    /// Calls of [`Dir::sync`] that succeeded.
    pub dir_syncs: u64,
}

#[derive(Debug, Default)]
struct Sector {
    /// The content at the last sync. `None` is zeros.
    durable: Option<Box<[u8]>>,
    /// The content after each write since.
    unsynced: Vec<Box<[u8]>>,
    /// A page a failed sync marked clean without writing it: what the
    /// cache shows in place of the durable content.
    cached: Option<Box<[u8]>>,
}

impl Sector {
    /// What the device holds, counting what it has been handed.
    fn device(&self) -> Option<&[u8]> {
        self.unsynced.last().or(self.durable.as_ref()).map(|b| &**b)
    }

    /// What a read through the cache sees.
    fn current(&self) -> Option<&[u8]> {
        self.unsynced
            .last()
            .or(self.cached.as_ref())
            .or(self.durable.as_ref())
            .map(|b| &**b)
    }
}

#[derive(Debug, Default)]
struct Inode {
    sectors: BTreeMap<u64, Sector>,
    len: u64,
    durable_len: u64,
}

impl Inode {
    fn settle(&mut self) {
        for sector in self.sectors.values_mut() {
            if let Some(last) = sector.unsynced.pop() {
                sector.durable = Some(last);
                sector.unsynced.clear();
            }
        }
        self.durable_len = self.len;
    }
}

#[derive(Debug, Clone)]
enum NameOp {
    Create(String, u64),
    Remove(String),
    Rename(String, String),
}

impl NameOp {
    fn apply(&self, names: &mut BTreeMap<String, u64>) {
        match self {
            NameOp::Create(name, inode) => {
                names.insert(name.clone(), *inode);
            }
            NameOp::Remove(name) => {
                names.remove(name);
            }
            NameOp::Rename(from, to) => {
                if let Some(inode) = names.remove(from) {
                    names.insert(to.clone(), inode);
                }
            }
        }
    }
}

/// An error waiting to happen.
#[derive(Debug, Clone, Copy)]
struct Fault {
    /// Calls that still succeed before it.
    after: u64,
    /// Whether every later call fails too.
    sticky: bool,
}

#[derive(Debug)]
struct State {
    inodes: BTreeMap<u64, Inode>,
    names: BTreeMap<String, u64>,
    durable_names: BTreeMap<String, u64>,
    pending_names: Vec<NameOp>,
    next_inode: u64,
    /// Handles of an earlier generation are dead.
    generation: u64,
    rng: Rng,
    write_fault: Option<Fault>,
    sync_fault: Option<Fault>,
    device_reads: bool,
    /// What a write rewrites as a whole, in bytes.
    write_unit: u64,
    stats: Stats,
}

fn injected(what: &str) -> io::Error {
    io::Error::other(format!("injected {what} error"))
}

impl State {
    /// Whether the fault in `slot` strikes this call.
    fn strikes(slot: &mut Option<Fault>) -> bool {
        match slot {
            None => false,
            Some(fault) if fault.after > 0 => {
                fault.after -= 1;
                false
            }
            Some(fault) => {
                if !fault.sticky {
                    *slot = None;
                }
                true
            }
        }
    }

    fn name_of(&self, inode: u64) -> Option<&str> {
        self.names
            .iter()
            .find(|(_, i)| **i == inode)
            .map(|(name, _)| name.as_str())
    }

    fn drop_unreachable(&mut self) {
        let (names, durable) = (&self.names, &self.durable_names);
        self.inodes
            .retain(|inode, _| names.values().chain(durable.values()).any(|i| i == inode));
    }
}

/// A simulated disk holding one directory. Clones share the disk. See the
/// [module documentation](self).
#[derive(Debug, Clone)]
pub struct SimDisk {
    state: Arc<Mutex<State>>,
}

impl SimDisk {
    /// An empty disk. `seed` decides what injected errors leave behind.
    pub fn new(seed: u64) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                inodes: BTreeMap::new(),
                names: BTreeMap::new(),
                durable_names: BTreeMap::new(),
                pending_names: Vec::new(),
                next_inode: 1,
                generation: 0,
                rng: Rng(seed),
                write_fault: None,
                sync_fault: None,
                device_reads: true,
                write_unit: SECTOR,
                stats: Stats::default(),
            })),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        // The state is plain data and every operation leaves it whole, so
        // a panic elsewhere while it was held does not make it unusable.
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The process dies. Every open handle is dead; nothing written is
    /// lost, and nothing unsynced has become durable.
    pub fn kill(&self) {
        self.lock().generation += 1;
    }

    /// Power is cut and returns. `cut` decides what is left of everything
    /// that was not made durable; what is left is durable. Every open
    /// handle is dead, and pending faults are cleared.
    pub fn power_cut(&self, mut cut: impl Cut) {
        let mut guard = self.lock();
        let state = &mut *guard;
        state.generation += 1;
        state.write_fault = None;
        state.sync_fault = None;
        let inodes: Vec<u64> = state.inodes.keys().copied().collect();
        for id in inodes {
            let name = state.name_of(id).map(str::to_owned);
            let name = name.as_deref();
            let Some(inode) = state.inodes.get_mut(&id) else {
                continue;
            };
            for (index, sector) in &mut inode.sectors {
                // The cache does not survive.
                sector.cached = None;
                if sector.unsynced.is_empty() {
                    continue;
                }
                let versions = std::mem::take(&mut sector.unsynced);
                match cut.sector(name, *index, versions.len()) {
                    Fate::Durable => {}
                    Fate::Latest => sector.durable = versions.into_iter().next_back(),
                    Fate::Version(n) => {
                        let n = n.min(versions.len() - 1);
                        sector.durable = versions.into_iter().nth(n);
                    }
                    Fate::Garbage => {
                        let mut bytes = vec![0u8; SECTOR_USIZE];
                        for chunk in bytes.chunks_mut(8) {
                            chunk.copy_from_slice(&state.rng.next().to_le_bytes()[..chunk.len()]);
                        }
                        sector.durable = Some(bytes.into_boxed_slice());
                    }
                }
            }
            if inode.len != inode.durable_len && !cut.length(name) {
                inode.len = inode.durable_len;
                // What lay beyond the old end is not part of the file.
                let end = inode.len;
                inode.sectors.retain(|index, _| index * SECTOR < end);
                if let Some(last) = inode.sectors.get_mut(&(end / SECTOR)) {
                    let keep = (end % SECTOR) as usize;
                    if let Some(bytes) = last.durable.as_mut() {
                        bytes[keep..].fill(0);
                    }
                }
            }
            inode.durable_len = inode.len;
        }
        let pending = std::mem::take(&mut state.pending_names);
        let kept = cut.names(pending.len()).min(pending.len());
        for op in &pending[..kept] {
            op.apply(&mut state.durable_names);
        }
        state.names = state.durable_names.clone();
        state.drop_unreachable();
    }

    /// The write after the next `after` writes fails, and, if `sticky`,
    /// every write after it. A failed write may have written any subset
    /// of its sectors.
    pub fn fail_write(&self, after: u64, sticky: bool) {
        self.lock().write_fault = Some(Fault { after, sticky });
    }

    /// The sync (of a file or of the directory) after the next `after`
    /// syncs fails, and, if `sticky`, every sync after it. A failed sync
    /// makes nothing durable and may drop unsynced data on the spot.
    pub fn fail_sync(&self, after: u64, sticky: bool) {
        self.lock().sync_fault = Some(Fault { after, sticky });
    }

    /// The operating system drops its clean pages, as it may at any
    /// time: what a failed sync left only in the cache is gone, and
    /// [`File::read_at`] shows what the disk holds.
    pub fn drop_caches(&self) {
        let mut state = self.lock();
        for inode in state.inodes.values_mut() {
            for sector in inode.sectors.values_mut() {
                sector.cached = None;
            }
        }
    }

    /// What a write rewrites as a whole: `bytes`, a multiple of
    /// [`SECTOR`] (which is what it is at first). Every sector of a unit
    /// that a later write touches gets a new unsynced version, also the
    /// sectors whose content the write does not change.
    pub fn set_write_unit(&self, bytes: u64) {
        self.lock().write_unit = (bytes / SECTOR).max(1) * SECTOR;
    }

    /// Whether [`File::read_device_at`] works (it does at first) or
    /// fails as unsupported, as on a file system without such a read.
    pub fn set_device_reads(&self, supported: bool) {
        self.lock().device_reads = supported;
    }

    /// Forget the faults that have not struck yet, and the sticky ones.
    pub fn clear_faults(&self) {
        let mut state = self.lock();
        state.write_fault = None;
        state.sync_fault = None;
    }

    /// Operation counts so far.
    pub fn stats(&self) -> Stats {
        self.lock().stats
    }

    /// How many sectors hold data that a power cut could still take.
    pub fn unsynced_sectors(&self) -> u64 {
        let state = self.lock();
        state
            .inodes
            .values()
            .flat_map(|inode| inode.sectors.values())
            .filter(|sector| !sector.unsynced.is_empty())
            .count() as u64
    }

    /// Flip the bits of `mask` in the byte at `offset` of the file `name`,
    /// in what a reader sees and in what is durable, as bit rot would.
    pub fn corrupt(&self, name: &str, offset: u64, mask: u8) -> io::Result<()> {
        let mut guard = self.lock();
        let state = &mut *guard;
        let id = *state
            .names
            .get(name)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        let inode = state
            .inodes
            .get_mut(&id)
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        if offset >= inode.len {
            return Err(io::ErrorKind::UnexpectedEof.into());
        }
        let sector = inode.sectors.entry(offset / SECTOR).or_default();
        let at = (offset % SECTOR) as usize;
        let durable = sector
            .durable
            .get_or_insert_with(|| vec![0u8; SECTOR_USIZE].into_boxed_slice());
        durable[at] ^= mask;
        for version in sector.unsynced.iter_mut().chain(&mut sector.cached) {
            version[at] ^= mask;
        }
        Ok(())
    }

    fn handle(&self, inode: u64, generation: u64) -> SimFile {
        SimFile {
            disk: self.clone(),
            inode,
            generation,
        }
    }
}

impl Dir for SimDisk {
    type File = SimFile;

    fn create(&self, name: &str) -> io::Result<SimFile> {
        let mut state = self.lock();
        if state.names.contains_key(name) {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let inode = state.next_inode;
        state.next_inode += 1;
        state.inodes.insert(inode, Inode::default());
        state.names.insert(name.to_owned(), inode);
        state
            .pending_names
            .push(NameOp::Create(name.to_owned(), inode));
        Ok(self.handle(inode, state.generation))
    }

    fn open(&self, name: &str) -> io::Result<SimFile> {
        let state = self.lock();
        match state.names.get(name) {
            Some(inode) => Ok(self.handle(*inode, state.generation)),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        let mut state = self.lock();
        if state.names.remove(name).is_none() {
            return Err(io::ErrorKind::NotFound.into());
        }
        state.pending_names.push(NameOp::Remove(name.to_owned()));
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let mut state = self.lock();
        let Some(inode) = state.names.remove(from) else {
            return Err(io::ErrorKind::NotFound.into());
        };
        state.names.insert(to.to_owned(), inode);
        state
            .pending_names
            .push(NameOp::Rename(from.to_owned(), to.to_owned()));
        Ok(())
    }

    fn list(&self) -> io::Result<Vec<String>> {
        Ok(self.lock().names.keys().cloned().collect())
    }

    fn sync(&self) -> io::Result<()> {
        let mut guard = self.lock();
        let state = &mut *guard;
        if State::strikes(&mut state.sync_fault) {
            return Err(injected("sync"));
        }
        state.durable_names = state.names.clone();
        state.pending_names.clear();
        state.drop_unreachable();
        state.stats.dir_syncs += 1;
        Ok(())
    }
}

/// An open file of a [`SimDisk`]. It stops working when the disk's
/// process is killed or its power is cut.
#[derive(Debug)]
pub struct SimFile {
    disk: SimDisk,
    inode: u64,
    generation: u64,
}

impl SimFile {
    fn check(&self, state: &State) -> io::Result<()> {
        if state.generation == self.generation {
            Ok(())
        } else {
            Err(io::Error::other(
                "the handle was opened before a kill or a power cut",
            ))
        }
    }
}

fn gone() -> io::Error {
    io::Error::other("the file no longer exists")
}

impl SimFile {
    fn read(&self, offset: u64, buf: &mut [u8], device: bool) -> io::Result<()> {
        let state = self.disk.lock();
        self.check(&state)?;
        if device && !state.device_reads {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let inode = state.inodes.get(&self.inode).ok_or_else(gone)?;
        let end = offset
            .checked_add(buf.len() as u64)
            .filter(|end| *end <= inode.len)
            .ok_or_else(|| io::Error::from(io::ErrorKind::UnexpectedEof))?;
        let mut at = offset;
        while at < end {
            let index = at / SECTOR;
            let from = (at % SECTOR) as usize;
            let n = (SECTOR_USIZE - from).min((end - at) as usize);
            let dst = &mut buf[(at - offset) as usize..][..n];
            let view = if device {
                Sector::device
            } else {
                Sector::current
            };
            match inode.sectors.get(&index).and_then(view) {
                Some(bytes) => dst.copy_from_slice(&bytes[from..from + n]),
                None => dst.fill(0),
            }
            at += n as u64;
        }
        Ok(())
    }
}

impl File for SimFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf, false)
    }

    fn read_device_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf, true)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        state.stats.writes += 1;
        let failing = State::strikes(&mut state.write_fault);
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let rng = &mut state.rng;
        let unit = state.write_unit;
        let inode = state.inodes.get_mut(&self.inode).ok_or_else(gone)?;
        // The units the write touches are rewritten as a whole: their
        // other sectors are written again as they are.
        if unit > SECTOR && !data.is_empty() {
            let first = offset / unit * unit;
            let last = (end.div_ceil(unit) * unit).min(inode.len.max(end));
            for index in first / SECTOR..last.div_ceil(SECTOR) {
                // The sectors the write itself touches go from what they
                // were straight to what it makes them, below.
                let touched = (index + 1) * SECTOR > offset && index * SECTOR < end;
                if touched || index * SECTOR >= inode.len {
                    continue;
                }
                let sector = inode.sectors.entry(index).or_default();
                let same: Box<[u8]> = match sector.current() {
                    Some(current) => current.into(),
                    None => vec![0u8; SECTOR_USIZE].into_boxed_slice(),
                };
                sector.cached = None;
                sector.unsynced.push(same);
            }
        }
        let mut at = offset;
        while at < end {
            let index = at / SECTOR;
            let from = (at % SECTOR) as usize;
            let n = (SECTOR_USIZE - from).min((end - at) as usize);
            let src = &data[(at - offset) as usize..][..n];
            at += n as u64;
            // A failed write leaves any subset of its sectors written.
            if failing && rng.coin() {
                continue;
            }
            let sector = inode.sectors.entry(index).or_default();
            let mut bytes: Box<[u8]> = match sector.current() {
                Some(current) => current.into(),
                None => vec![0u8; SECTOR_USIZE].into_boxed_slice(),
            };
            bytes[from..from + n].copy_from_slice(src);
            // The page is dirty again, whatever a failed sync made of it.
            sector.cached = None;
            sector.unsynced.push(bytes);
            inode.len = inode.len.max(index * SECTOR + (from + n) as u64);
        }
        if failing {
            Err(injected("write"))
        } else {
            Ok(())
        }
    }

    fn sync(&mut self) -> io::Result<()> {
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        let failing = State::strikes(&mut state.sync_fault);
        let rng = &mut state.rng;
        let inode = state.inodes.get_mut(&self.inode).ok_or_else(gone)?;
        if failing {
            for sector in inode.sectors.values_mut() {
                if sector.unsynced.is_empty() {
                    continue;
                }
                match rng.below(3) {
                    // Still dirty: a later sync may write it.
                    0 => {}
                    // Dropped: the file reads as it did before.
                    1 => sector.unsynced.clear(),
                    // Marked clean, and only the cache holds it.
                    _ => {
                        sector.cached = sector.unsynced.pop();
                        sector.unsynced.clear();
                    }
                }
            }
            return Err(injected("sync"));
        }
        inode.settle();
        state.stats.syncs += 1;
        Ok(())
    }

    fn preallocate(&mut self, len: u64) -> io::Result<()> {
        let mut state = self.disk.lock();
        self.check(&state)?;
        let inode = state.inodes.get_mut(&self.inode).ok_or_else(gone)?;
        if len < inode.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "preallocate does not shrink a file",
            ));
        }
        inode.len = len;
        Ok(())
    }

    fn len(&self) -> io::Result<u64> {
        let state = self.disk.lock();
        self.check(&state)?;
        Ok(state.inodes.get(&self.inode).ok_or_else(gone)?.len)
    }
}

#[cfg(test)]
mod tests {
    //! Each test names the property of the simulated disk it checks. The
    //! write log's tests rest on these.

    use super::*;

    const S: usize = SECTOR_USIZE;

    fn read(disk: &SimDisk, name: &str, offset: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0xEE; len];
        disk.open(name).unwrap().read_at(offset, &mut buf).unwrap();
        buf
    }

    /// A disk with one file `f` of `sectors` sectors, each filled with
    /// `fill`, all durable.
    fn disk_with(sectors: usize, fill: u8) -> SimDisk {
        let disk = SimDisk::new(1);
        let mut f = disk.create("f").unwrap();
        f.write_at(0, &vec![fill; sectors * S]).unwrap();
        f.sync().unwrap();
        Dir::sync(&disk).unwrap();
        disk
    }

    /// Visibility: a write is read back at once, synced or not, and a
    /// read beyond the end is an error, not zeros.
    #[test]
    fn a_write_is_visible_before_it_is_synced() {
        let disk = SimDisk::new(0);
        let mut f = disk.create("f").unwrap();
        f.write_at(100, b"hello").unwrap();
        assert_eq!(f.len().unwrap(), 105);
        let mut buf = [0u8; 7];
        f.read_at(98, &mut buf).unwrap();
        assert_eq!(&buf, b"\0\0hello");
        assert_eq!(
            f.read_at(100, &mut buf).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(disk.unsynced_sectors(), 1);
    }

    /// Process kill: every handle dies, everything written is still
    /// there, and it is still unsynced: a power cut after the kill can
    /// take it.
    #[test]
    fn a_kill_loses_nothing_and_makes_nothing_durable() {
        let disk = disk_with(2, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![2; 2 * S]).unwrap();
        disk.kill();
        assert!(f.write_at(0, b"x").is_err());
        assert!(f.len().is_err());
        assert_eq!(read(&disk, "f", 0, 2 * S), vec![2; 2 * S]);
        assert_eq!(disk.unsynced_sectors(), 2);
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, 2 * S), vec![1; 2 * S]);
    }

    /// Sync: what was written before a successful sync survives a power
    /// cut that loses everything it can; what was written after does not.
    #[test]
    fn a_sync_makes_what_was_written_durable() {
        let disk = disk_with(2, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![2; S]).unwrap();
        f.sync().unwrap();
        f.write_at(S as u64, &vec![3; S]).unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, S), vec![2; S]);
        assert_eq!(read(&disk, "f", S as u64, S), vec![1; S]);
        assert_eq!(disk.unsynced_sectors(), 0);
    }

    /// Power cut, any subset: one write over three sectors can lose its
    /// first and its last sector and keep the one in the middle, so a
    /// later sector survives an earlier one and a write is torn at
    /// sector boundaries.
    #[test]
    fn a_power_cut_keeps_any_subset_of_sectors() {
        let disk = disk_with(3, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![2; 3 * S]).unwrap();
        disk.power_cut(BySector(|_: Option<&str>, index| {
            if index == 1 {
                Fate::Latest
            } else {
                Fate::Durable
            }
        }));
        let mut expected = vec![1; S];
        expected.extend(vec![2; S]);
        expected.extend(vec![1; S]);
        assert_eq!(read(&disk, "f", 0, 3 * S), expected);
    }

    /// Power cut, versions: of two unsynced writes to one sector the cut
    /// can keep the first and lose the second.
    #[test]
    fn a_power_cut_can_keep_an_earlier_unsynced_version() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &[2; 8]).unwrap();
        f.write_at(8, &[3; 8]).unwrap();
        disk.power_cut(BySector(|_: Option<&str>, _| Fate::Version(0)));
        let got = read(&disk, "f", 0, 24);
        assert_eq!(&got[..8], &[2; 8]);
        assert_eq!(&got[8..], &[1; 16]);
    }

    /// Power cut, garbage: a lost sector can read as bytes that were
    /// never written, neither the old content nor the new.
    #[test]
    fn a_power_cut_can_leave_garbage() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![2; S]).unwrap();
        disk.power_cut(BySector(|_: Option<&str>, _| Fate::Garbage));
        let got = read(&disk, "f", 0, S);
        assert_ne!(got, vec![1; S]);
        assert_ne!(got, vec![2; S]);
    }

    /// Power cut, stability: what a power cut leaves is durable. A second
    /// cut straight after changes nothing.
    #[test]
    fn what_a_power_cut_leaves_is_durable() {
        let disk = disk_with(4, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![2; 4 * S]).unwrap();
        disk.power_cut(SeededCut::with_garbage(7));
        let first = read(&disk, "f", 0, 4 * S);
        assert_eq!(disk.unsynced_sectors(), 0);
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, 4 * S), first);
    }

    /// Seeds: the same seed gives the same cut, and over a few seeds a
    /// sector is both kept and lost.
    #[test]
    fn a_seeded_cut_is_reproducible_and_varied() {
        let outcome = |seed| {
            let disk = disk_with(8, 1);
            let mut f = disk.open("f").unwrap();
            f.write_at(0, &vec![2; 8 * S]).unwrap();
            disk.power_cut(SeededCut::new(seed));
            read(&disk, "f", 0, 8 * S)
        };
        assert_eq!(outcome(3), outcome(3));
        let first_sectors: Vec<u8> = (0..16).map(|seed| outcome(seed)[0]).collect();
        assert!(first_sectors.contains(&1) && first_sectors.contains(&2));
    }

    /// Names: a file created and synced, but whose directory was not
    /// synced, is gone after a power cut; after a directory sync it
    /// stays.
    #[test]
    fn a_created_file_needs_a_directory_sync() {
        let disk = SimDisk::new(0);
        let mut f = disk.create("f").unwrap();
        f.write_at(0, b"data").unwrap();
        f.sync().unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(disk.open("f").unwrap_err().kind(), io::ErrorKind::NotFound);

        let mut f = disk.create("f").unwrap();
        f.write_at(0, b"data").unwrap();
        f.sync().unwrap();
        Dir::sync(&disk).unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, 4), b"data");
        assert_eq!(disk.list().unwrap(), vec!["f".to_owned()]);
    }

    /// Names: of the directory operations since the last directory sync
    /// a prefix survives, in order: here the creation without the rename
    /// and the removal that followed it.
    #[test]
    fn directory_operations_survive_as_a_prefix() {
        struct FirstOnly;
        impl Cut for FirstOnly {
            fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> Fate {
                Fate::Latest
            }
            fn names(&mut self, pending: usize) -> usize {
                assert_eq!(pending, 3);
                1
            }
            fn length(&mut self, _: Option<&str>) -> bool {
                true
            }
        }
        let disk = disk_with(1, 1);
        disk.create("a").unwrap();
        disk.rename("a", "b").unwrap();
        disk.remove("f").unwrap();
        assert_eq!(disk.list().unwrap(), vec!["b".to_owned()]);
        disk.power_cut(FirstOnly);
        assert_eq!(disk.list().unwrap(), vec!["a".to_owned(), "f".to_owned()]);
    }

    /// Names: a rename replaces its target, and is durable only after a
    /// directory sync.
    #[test]
    fn a_rename_replaces_its_target_and_needs_a_directory_sync() {
        let disk = disk_with(1, 1);
        let mut g = disk.create("g").unwrap();
        g.write_at(0, &[9; 4]).unwrap();
        g.sync().unwrap();
        Dir::sync(&disk).unwrap();
        disk.rename("g", "f").unwrap();
        assert_eq!(read(&disk, "f", 0, 4), vec![9; 4]);
        assert_eq!(disk.list().unwrap(), vec!["f".to_owned()]);
        assert_eq!(
            disk.rename("g", "h").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        let undone = disk.clone();
        undone.power_cut(LoseAll);
        assert_eq!(read(&undone, "f", 0, 4), vec![1; 4]);
        assert_eq!(read(&undone, "g", 0, 4), vec![9; 4]);
    }

    /// Names: a removed file's open handle keeps working until the
    /// process dies, and a name cannot be created twice.
    #[test]
    fn open_handles_outlive_removal() {
        let disk = disk_with(1, 1);
        let mut g = disk.create("g").unwrap();
        g.write_at(0, &[9; 4]).unwrap();
        assert_eq!(
            disk.create("g").unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        disk.remove("g").unwrap();
        let mut buf = [0u8; 4];
        g.read_at(0, &mut buf).unwrap();
        assert_eq!(buf, [9; 4]);
        assert_eq!(
            disk.remove("g").unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
    }

    /// Length: growth that was not synced can be lost at a power cut,
    /// and with it the data beyond the old end.
    #[test]
    fn unsynced_growth_can_be_lost() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        f.preallocate(4 * SECTOR).unwrap();
        f.write_at(2 * SECTOR, &[5; 8]).unwrap();
        assert_eq!(read(&disk, "f", SECTOR, 8), vec![0; 8]);
        assert!(f.preallocate(SECTOR).is_err());
        disk.power_cut(LoseAll);
        let f = disk.open("f").unwrap();
        assert_eq!(f.len().unwrap(), SECTOR);
        assert!(f.is_empty().is_ok_and(|empty| !empty));
    }

    /// Injected write error: the chosen write fails and the ones around
    /// it do not; over a few seeds the failed write leaves some of its
    /// sectors written and some not.
    #[test]
    fn a_failed_write_leaves_any_part_written() {
        let mut written = Vec::new();
        for seed in 0..8 {
            let disk = SimDisk::new(seed);
            let mut f = disk.create("f").unwrap();
            f.write_at(0, &vec![1; 4 * S]).unwrap();
            disk.fail_write(1, false);
            f.write_at(0, &vec![2; S]).unwrap();
            assert!(f.write_at(0, &vec![3; 4 * S]).is_err());
            f.write_at(0, &[4; 1]).unwrap();
            let got = read(&disk, "f", 0, 4 * S);
            written.extend((1..4).map(|i| got[i * S] == 3));
        }
        assert!(written.contains(&true) && written.contains(&false));
        assert_eq!(SimDisk::new(0).stats().writes, 0);
    }

    /// Injected sync error: nothing becomes durable, and over a few
    /// seeds an unsynced sector goes each of three ways. It stays
    /// unsynced and a later sync writes it. Or it is dropped at once,
    /// without a power cut. Or the cache goes on showing it, also after a
    /// kill and after a later sync that succeeds, while the device read
    /// shows the old content, and a power cut or a dropped cache shows
    /// the same to everyone.
    #[test]
    fn a_failed_sync_leaves_data_dirty_dropped_or_only_cached() {
        let (mut dirty, mut dropped, mut only_cached) = (0, 0, 0);
        for seed in 0..16 {
            let disk = disk_with(4, 1);
            disk.lock().rng = Rng(seed);
            let mut f = disk.open("f").unwrap();
            f.write_at(0, &vec![2; 4 * S]).unwrap();
            disk.fail_sync(0, false);
            assert!(f.sync().is_err());
            let cached = read(&disk, "f", 0, 4 * S);
            let mut device = vec![0; 4 * S];
            f.read_device_at(0, &mut device).unwrap();
            let unsynced = disk.unsynced_sectors();
            // A later sync succeeds, and proves nothing about the rest.
            f.sync().unwrap();
            disk.kill();
            assert_eq!(read(&disk, "f", 0, 4 * S), cached);
            let after_cut = disk.clone();
            let evicted = disk;
            evicted.drop_caches();
            assert_eq!(read(&evicted, "f", 0, 4 * S), device);
            after_cut.power_cut(LoseAll);
            assert_eq!(read(&after_cut, "f", 0, 4 * S), device);
            for i in 0..4 {
                match (cached[i * S], device[i * S]) {
                    (2, 2) => dirty += 1,
                    (1, 1) => dropped += 1,
                    (2, 1) => only_cached += 1,
                    other => panic!("impossible sector state {other:?}"),
                }
            }
            assert_eq!(
                unsynced,
                device.chunks(S).filter(|c| c[0] == 2).count() as u64
            );
        }
        assert!(dirty > 0 && dropped > 0 && only_cached > 0);
    }

    /// Write unit: with one set, a small write puts the durable sectors
    /// around it in the same unit at risk, and no others.
    #[test]
    fn a_write_rewrites_its_whole_unit() {
        let disk = disk_with(16, 1);
        disk.set_write_unit(4096);
        let mut f = disk.open("f").unwrap();
        // One byte in the second unit (sectors 8 to 15).
        f.write_at(4096 + 700, &[2]).unwrap();
        assert_eq!(disk.unsynced_sectors(), 8);
        disk.power_cut(BySector(|_: Option<&str>, _| Fate::Garbage));
        let got = read(&disk, "f", 0, 16 * S);
        assert_eq!(got[..8 * S], vec![1; 8 * S][..]);
        for sector in 8..16 {
            assert_ne!(
                got[sector * S..(sector + 1) * S],
                vec![1; S][..],
                "{sector}"
            );
        }
        // Lost without garbage, the untouched sectors are as they were.
        let disk = disk_with(16, 1);
        disk.set_write_unit(4096);
        let mut f = disk.open("f").unwrap();
        f.write_at(4096 + 700, &[2]).unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, 16 * S), vec![1; 16 * S]);
    }

    /// Device reads: they include data that is written and not synced,
    /// and a disk can be told to have none, which is an error and not a
    /// cached read.
    #[test]
    fn device_reads_see_unsynced_data_or_are_unsupported() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &[2; 8]).unwrap();
        let mut buf = [0u8; 16];
        f.read_device_at(0, &mut buf).unwrap();
        assert_eq!(buf[..8], [2; 8]);
        assert_eq!(buf[8..], [1; 8]);
        disk.set_device_reads(false);
        assert_eq!(
            f.read_device_at(0, &mut buf).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        f.read_at(0, &mut buf).unwrap();
    }

    /// Injected errors: a sticky fault keeps striking, also on the
    /// directory, until it is cleared; counters count successes.
    #[test]
    fn a_sticky_fault_strikes_until_cleared() {
        let disk = disk_with(1, 1);
        let before = disk.stats();
        let mut f = disk.open("f").unwrap();
        disk.fail_sync(0, true);
        assert!(f.sync().is_err());
        assert!(Dir::sync(&disk).is_err());
        assert_eq!(disk.stats(), before);
        disk.clear_faults();
        f.sync().unwrap();
        Dir::sync(&disk).unwrap();
        assert_eq!(disk.stats().syncs, before.syncs + 1);
        assert_eq!(disk.stats().dir_syncs, before.dir_syncs + 1);
    }

    /// Corruption hook: a flipped bit shows to readers and survives a
    /// power cut.
    #[test]
    fn corrupt_flips_bits_for_good() {
        let disk = disk_with(1, 1);
        disk.corrupt("f", 3, 0x80).unwrap();
        assert_eq!(read(&disk, "f", 2, 3), vec![1, 0x81, 1]);
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 3, 1), vec![0x81]);
        assert!(disk.corrupt("f", SECTOR, 1).is_err());
        assert!(disk.corrupt("nope", 0, 1).is_err());
    }
}

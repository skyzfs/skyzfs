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
//!   the last [`Dir::sync`], any subset survives, applied in order: a
//!   later file's name can stay while an earlier one's is lost, as on a
//!   file system that logs each file's name with that file's own sync.
//!   What is left is durable, and every handle opened before the cut is
//!   dead.
//! - **Open handles.** A file that was removed, or replaced by a
//!   rename, stays readable and writable through the handles that are
//!   open on it, also across a directory sync, until the last of them
//!   is dropped or the process is lost. A kill and a power cut end
//!   every handle: the process that held them is gone.
//! - **A process kill** ([`SimDisk::kill`]). Every handle is dead and
//!   nothing is lost: what was written is still there to read. It is
//!   still unsynced, so a later power cut can take it.
//! - **Injected errors** ([`SimDisk::fail_write`], [`SimDisk::fail_sync`],
//!   [`SimDisk::fail_read`]). A failed read, through the cache or from
//!   the device, returns nothing and changes nothing.
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
//! - **The file system's block** ([`SimDisk::set_block_size`]). A file
//!   system writes back whole blocks: where its block is larger than
//!   the write unit, the block is what a write rewrites.
//!
//! - **A sync that ends later** ([`File::sync_begin`],
//!   [`File::sync_end`]). The versions of the file's sectors that are
//!   not synced when it begins are taken note of, and its end makes
//!   those durable and nothing written after its begin. It ends at the
//!   first call of `sync_end`, or, with [`SimDisk::set_sync_delay`],
//!   after as many calls that do not wait as a seed chooses, or when a
//!   call waits. It counts as a write or sync for
//!   [`SimDisk::halt_after`] where it ends, not where it begins. A test
//!   can hold a file's syncs at their end ([`SimDisk::hold_syncs_of`])
//!   as a slow device would. No thread is involved: the sync begins and
//!   ends at two points of its caller's own course.
//!
//! What it does not model: a disk that reports a sync it did not do, a
//! write that lands in the wrong place, corruption of durable data on its
//! own (a test can corrupt a file itself, with [`SimDisk::corrupt`]),
//! running out of space, and a file system without a from-the-device
//! read (a test can switch it off, with [`SimDisk::set_device_reads`]).
//!
//! Where it is kinder than a real disk:
//!
//! - **A kill never tears a write.** A kernel can return a short
//!   buffered write when a fatal signal arrives. The bytes that leaves
//!   are those of a failed write, which is modelled.
//! - **A file's length after a power cut is the old or the new one**,
//!   never one between.
//! - **A sync error reaches the caller that issued the sync.** A kernel
//!   can report it to another descriptor of the file, or only once.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::io::{Dir, DirectAlign, File, NotBegun, SECTOR};

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

    /// Whether the `index`-th of the `pending` creations, removals and
    /// renames since the last directory sync survives. Asked once for
    /// each, in order; the survivors are applied in order.
    fn name(&mut self, index: usize, pending: usize) -> bool;

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
    fn name(&mut self, _: usize, _: usize) -> bool {
        false
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
    fn name(&mut self, _: usize, _: usize) -> bool {
        true
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
    /// How the name operations go, drawn at the first of them: the
    /// first so many survive, or each on its own.
    names: Option<Option<usize>>,
}

impl SeededCut {
    /// A cut drawn from `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed ^ 0xC2A5_11ED_0D15_C0DE),
            garbage: false,
            names: None,
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
    fn name(&mut self, index: usize, pending: usize) -> bool {
        // Half of the cuts keep a prefix, which is what most file
        // systems do; the others decide each operation on its own.
        let rng = &mut self.rng;
        let plan = *self.names.get_or_insert_with(|| {
            let prefix = rng.below(pending as u64 + 1) as usize;
            rng.coin().then_some(prefix)
        });
        match plan {
            Some(prefix) => index < prefix,
            None => self.rng.coin(),
        }
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
    fn name(&mut self, _: usize, _: usize) -> bool {
        true
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
    /// Bytes handed to [`File::write_at`], failed calls included.
    pub written: u64,
    /// Bytes deallocated by [`File::deallocate`].
    pub deallocated: u64,
    /// Calls of [`File::start_writeback`] that succeeded.
    pub writebacks: u64,
    /// The bytes those calls named.
    pub writeback_bytes: u64,
    /// Calls of [`File::sync_begin`] that succeeded.
    pub sync_begins: u64,
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
    /// Whether the newest unsynced version was written by direct I/O:
    /// it is on the device, and no page of the cache holds it, so
    /// [`File::cached_pages`] does not count it (a kernel's `cachestat`
    /// does not see a device's volatile cache). A version written
    /// through the cache is a dirty page until a sync.
    direct: bool,
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
    /// The file's 4096-byte pages in the cache: read or written through
    /// it since the cache last lost them ([`File::cached_pages`]).
    pages: BTreeSet<u64>,
    /// Pages a read-ahead is still bringing in
    /// ([`SimDisk::hold_read_ahead_of`]): no drop takes them, and they
    /// count.
    under_io: BTreeSet<u64>,
}

/// The 4096-byte pages that `len` bytes at `offset` touch.
fn pages_of(offset: u64, len: u64) -> std::ops::Range<u64> {
    if len == 0 {
        return 0..0;
    }
    offset / 4096..(offset + len).div_ceil(4096)
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

    /// What a sync that began at `begun` makes durable: each sector's
    /// last version written before then, and the length it had then.
    /// Later versions stay unsynced.
    fn settle_before(&mut self, begun: &PendingSync) {
        for (index, &count) in &begun.versions {
            let Some(sector) = self.sectors.get_mut(index) else {
                continue;
            };
            // A whole sync since may have settled them already.
            if count == 0 || sector.unsynced.len() < count {
                continue;
            }
            let mut before: Vec<_> = sector.unsynced.drain(..count).collect();
            sector.durable = before.pop();
        }
        // A whole sync in between may have made a greater length
        // durable already.
        self.durable_len = self.durable_len.max(begun.len);
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
    /// The file whose syncs all fail.
    sync_fault_of: Option<String>,
    /// The file whose writes all fail, and the one whose reads do.
    write_fault_of: Option<String>,
    read_fault_of: Option<String>,
    read_fault: Option<Fault>,
    /// The file system has no call that deallocates.
    no_holes: bool,
    /// The file system's block size, or none if it cannot be asked.
    block_size: Option<u64>,
    /// The page size the system reports.
    page_size: u64,
    /// The next deallocation fails, having zeroed any part of its range.
    hole_fault: bool,
    /// The file system has no call that starts writeback.
    no_writeback: bool,
    /// The file whose every call that starts writeback fails.
    writeback_fault_of: Option<String>,
    /// How many of the next calls that start writeback the system
    /// declines this time, short of memory.
    writeback_refusals: u64,
    /// Every call that started writeback: the file's name, the offset
    /// and the length.
    writebacks: Vec<(String, u64, u64)>,
    /// A sync that ends later ends after up to this many calls of
    /// `sync_end` that do not wait: see [`SimDisk::set_sync_delay`].
    sync_delay: u64,
    /// The file whose syncs that end later do not end until released.
    held_syncs_of: Option<String>,
    /// The file whose whole syncs ([`File::sync`]) do not return until
    /// released.
    held_whole_syncs_of: Option<String>,
    /// Callers waiting at the end of a held sync.
    held_waiting: u64,
    /// How many of the next syncs that would end later are not begun:
    /// see [`SimDisk::refuse_sync_begins`].
    begins_refused: u64,
    /// Calls that wrote to or deallocated in each file, by name: see
    /// [`SimDisk::writes_of`].
    writes_of: BTreeMap<String, u64>,
    /// The files with a sync that ended later under way, by inode: how
    /// many handles have one.
    syncing: BTreeMap<u64, u64>,
    /// Calls that wrote to or deallocated in a file while it had such a
    /// sync under way, by name: see [`SimDisk::writes_while_syncing`].
    writes_while_syncing: BTreeMap<String, u64>,
    /// The machine stops after this many more writes and syncs.
    halt: Option<u64>,
    /// Who holds the directory's lock, and the next holder's number.
    lock: Option<u64>,
    next_lock: u64,
    /// How many live handles are open on each inode.
    open: BTreeMap<u64, u64>,
    device_reads: bool,
    /// How many of the next drops of a file's cache
    /// ([`File::drop_cache`]) leave its pages, as where something else
    /// maps, locks or reads them.
    cache_kept: u64,
    /// [`File::cached_pages`] answers; otherwise it is unsupported.
    cache_counts: bool,
    /// [`File::cached_pages`] answers 0 whatever the cache holds, as
    /// `cachestat` through a file of an overlay file system does.
    cache_count_blind: bool,
    /// The file whose next read through the cache leaves pages after
    /// it under I/O, and how many ([`SimDisk::hold_read_ahead_of`]).
    read_ahead_of: Option<(String, u64)>,
    /// What direct I/O asks of a request's alignment, or `None` where
    /// the file system has none ([`SimDisk::set_direct_align`]).
    direct_align: Option<DirectAlign>,
    /// What a write rewrites as a whole, in bytes.
    write_unit: u64,
    /// How many of the next direct reads and writes wait, before they
    /// do anything, until [`SimDisk::release_direct_io`].
    direct_holds: u64,
    /// Releases of held direct I/O so far: a held call waits until it
    /// changes.
    direct_releases: u64,
    /// The direct read or write after this many more panics.
    direct_panic: Option<u64>,
    stats: Stats,
}

fn injected(what: &str) -> io::Error {
    io::Error::other(format!("injected {what} error"))
}

impl State {
    /// Whether a direct request at `offset` of `len` bytes, in memory at
    /// `address`, is one the file system takes: unsupported where it
    /// has no direct I/O, invalid where it is not aligned.
    fn direct_fits(&self, offset: u64, len: usize, address: usize) -> io::Result<()> {
        match self.direct_align {
            None => Err(io::ErrorKind::Unsupported.into()),
            Some(align) if !align.fits(offset, len, address) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "not aligned for direct I/O",
            )),
            Some(_) => Ok(()),
        }
    }

    /// Whether the machine has stopped. A write or a sync (`counts`)
    /// uses up one of the calls it still takes.
    fn halted(&mut self, counts: bool) -> bool {
        match &mut self.halt {
            None => false,
            Some(0) => true,
            Some(left) => {
                if counts {
                    *left -= 1;
                }
                false
            }
        }
    }

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

    /// A call wrote to the file `inode`: counted under its name, and
    /// again if a sync of the file that ends later is under way.
    fn count_write(&mut self, inode: u64) {
        if let Some(name) = self.name_of(inode).map(str::to_string) {
            if self.syncing.contains_key(&inode) {
                *self.writes_while_syncing.entry(name.clone()).or_default() += 1;
            }
            *self.writes_of.entry(name).or_default() += 1;
        }
    }

    /// A sync that ends later, of the file `inode`, is no longer under
    /// way.
    fn sync_over(&mut self, inode: u64) {
        if let Some(count) = self.syncing.get_mut(&inode) {
            *count -= 1;
            if *count == 0 {
                self.syncing.remove(&inode);
            }
        }
    }

    fn name_of(&self, inode: u64) -> Option<&str> {
        self.names
            .iter()
            .find(|(_, i)| **i == inode)
            .map(|(name, _)| name.as_str())
    }

    /// Forget the files that nothing can reach any more: no name, no
    /// name a power cut could bring back, and no open handle. A power
    /// cut can bring back a durable name, and the name of a creation
    /// that is not synced yet, whatever was done to that name since: it
    /// may keep the creation and lose a removal that followed it.
    fn drop_unreachable(&mut self) {
        let (names, durable, open) = (&self.names, &self.durable_names, &self.open);
        let pending = &self.pending_names;
        self.inodes.retain(|inode, _| {
            open.contains_key(inode)
                || names.values().chain(durable.values()).any(|i| i == inode)
                || pending
                    .iter()
                    .any(|op| matches!(op, NameOp::Create(_, created) if created == inode))
        });
    }

    /// The process is gone, and its handles with it.
    fn close_all(&mut self) {
        self.generation += 1;
        self.open.clear();
        self.syncing.clear();
        self.lock = None;
    }
}

/// A simulated disk holding one directory. Clones share the disk. See the
/// [module documentation](self).
#[derive(Debug, Clone)]
pub struct SimDisk {
    state: Arc<Mutex<State>>,
    /// Woken when held syncs are released, and at a kill or power cut.
    released: Arc<Condvar>,
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
                sync_fault_of: None,
                write_fault_of: None,
                read_fault_of: None,
                read_fault: None,
                no_holes: false,
                block_size: Some(SECTOR),
                page_size: SECTOR,
                hole_fault: false,
                no_writeback: false,
                writeback_fault_of: None,
                writeback_refusals: 0,
                writebacks: Vec::new(),
                sync_delay: 0,
                held_syncs_of: None,
                held_whole_syncs_of: None,
                held_waiting: 0,
                begins_refused: 0,
                writes_of: BTreeMap::new(),
                syncing: BTreeMap::new(),
                writes_while_syncing: BTreeMap::new(),
                halt: None,
                lock: None,
                next_lock: 0,
                open: BTreeMap::new(),
                device_reads: true,
                cache_kept: 0,
                cache_counts: true,
                cache_count_blind: false,
                read_ahead_of: None,
                direct_align: Some(DirectAlign {
                    memory: SECTOR as u32,
                    offset: SECTOR as u32,
                }),
                write_unit: SECTOR,
                direct_holds: 0,
                direct_releases: 0,
                direct_panic: None,
                stats: Stats::default(),
            })),
            released: Arc::new(Condvar::new()),
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
        let mut state = self.lock();
        state.close_all();
        state.drop_unreachable();
        drop(state);
        self.released.notify_all();
    }

    /// Power is cut and returns. `cut` decides what is left of everything
    /// that was not made durable; what is left is durable. Every open
    /// handle is dead, and pending faults are cleared.
    pub fn power_cut(&self, mut cut: impl Cut) {
        let mut guard = self.lock();
        let state = &mut *guard;
        state.close_all();
        state.write_fault = None;
        state.sync_fault = None;
        state.sync_fault_of = None;
        state.write_fault_of = None;
        state.writeback_fault_of = None;
        state.read_fault_of = None;
        state.halt = None;
        state.read_fault = None;
        let inodes: Vec<u64> = state.inodes.keys().copied().collect();
        for id in inodes {
            let name = state.name_of(id).map(str::to_owned);
            let name = name.as_deref();
            let Some(inode) = state.inodes.get_mut(&id) else {
                continue;
            };
            inode.pages.clear();
            inode.under_io.clear();
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
        for (index, op) in pending.iter().enumerate() {
            if cut.name(index, pending.len()) {
                op.apply(&mut state.durable_names);
            }
        }
        state.names = state.durable_names.clone();
        state.drop_unreachable();
        // A sync held at its end wakes once the state has changed, and
        // finds its handle dead: woken before, it could take the lock
        // first, find nothing changed, and wait for good.
        drop(guard);
        self.released.notify_all();
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

    /// Every sync of the file called `name` fails from now on, until
    /// the faults are cleared or the power is cut: for a test that
    /// needs one particular file to stay unsynced, whatever else is
    /// synced before it. What a failed sync leaves is as with
    /// [`SimDisk::fail_sync`].
    pub fn fail_sync_of(&self, name: &str) {
        self.lock().sync_fault_of = Some(name.to_owned());
    }

    /// Every write to the file called `name` fails from now on and
    /// writes nothing, until the faults are cleared or the power is
    /// cut: for a test that needs one particular file to refuse.
    pub fn fail_write_of(&self, name: &str) {
        self.lock().write_fault_of = Some(name.to_owned());
    }

    /// Every read of the file called `name` fails from now on, through
    /// the cache and from the device, until the faults are cleared or
    /// the power is cut.
    pub fn fail_read_of(&self, name: &str) {
        self.lock().read_fault_of = Some(name.to_owned());
    }

    /// The machine stops taking anything: after the next `after` writes
    /// and syncs, every write, sync, change of a length and change of a
    /// name fails and leaves nothing, until the faults are cleared or
    /// the power is cut. With a power cut next, this is the power going
    /// at that point of whatever the caller does in between, which the
    /// caller then sees as errors and cannot act on.
    pub fn halt_after(&self, after: u64) {
        self.lock().halt = Some(after);
    }

    /// The read after the next `after` reads fails, and, if `sticky`,
    /// every read after it: reads through the cache and reads from the
    /// device count alike. A failed read changes nothing.
    pub fn fail_read(&self, after: u64, sticky: bool) {
        self.lock().read_fault = Some(Fault { after, sticky });
    }

    /// The names that do not lead to a file. There are none: this is
    /// for tests to check the disk itself with.
    pub fn dangling_names(&self) -> Vec<String> {
        let state = self.lock();
        state
            .names
            .iter()
            .chain(&state.durable_names)
            .filter(|(_, inode)| !state.inodes.contains_key(inode))
            .map(|(name, _)| name.clone())
            .collect()
    }

    /// The bytes the device holds of every file, by name, and each
    /// file's length: what a test compares to show that something left
    /// the disk as it was.
    pub fn image(&self) -> Vec<(String, Vec<u8>)> {
        let state = self.lock();
        let mut out = Vec::new();
        for (name, id) in &state.names {
            let Some(inode) = state.inodes.get(id) else {
                continue;
            };
            let mut bytes = vec![0u8; inode.len as usize];
            for (index, sector) in &inode.sectors {
                let at = (*index * SECTOR) as usize;
                if let (Some(src), true) = (sector.device(), at < bytes.len()) {
                    let n = SECTOR_USIZE.min(bytes.len() - at);
                    bytes[at..at + n].copy_from_slice(&src[..n]);
                }
            }
            out.push((name.clone(), bytes));
        }
        out
    }

    /// The operating system drops its clean pages, as it may at any
    /// time: what a failed sync left only in the cache is gone, and
    /// [`File::read_at`] shows what the disk holds.
    pub fn drop_caches(&self) {
        let mut state = self.lock();
        for inode in state.inodes.values_mut() {
            inode.pages.clear();
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

    /// Whether a drop of a file's cache ([`File::drop_cache`]) leaves
    /// its pages (it does not at first), as where something else maps
    /// or locks them: they stay, and [`File::cached_pages`] counts them.
    pub fn keep_cache(&self, kept: bool) {
        self.lock().cache_kept = if kept { u64::MAX } else { 0 };
    }

    /// The next `drops` drops of a file's cache leave its pages, and
    /// those after them do not.
    pub fn keep_cache_for(&self, drops: u64) {
        self.lock().cache_kept = drops;
    }

    /// Whether [`File::cached_pages`] answers 0 whatever the cache
    /// holds (it does not at first), as `cachestat` does through a file
    /// of an overlay file system, whose pages are the file's below.
    pub fn blind_cache_count(&self, blind: bool) {
        self.lock().cache_count_blind = blind;
    }

    /// The next read through the cache of the file named `name` also
    /// brings in the `pages` pages after it, as the kernel's read-ahead
    /// does, and leaves them under I/O: no drop of the file's cache
    /// ([`File::drop_cache`], [`SimDisk::drop_caches`]) takes them, and
    /// [`File::cached_pages`] counts them, until a power cut. Where the
    /// block read is a hole, the kernel answers it at once while its
    /// read-ahead is still being read from the disk, and a drop leaves
    /// pages under I/O. A read alone ([`File::read_alone_at`]) asks for
    /// no read-ahead, and does not take this one.
    pub fn hold_read_ahead_of(&self, name: &str, pages: u64) {
        self.lock().read_ahead_of = Some((name.to_string(), pages));
    }

    /// What direct I/O ([`File::read_direct_at`],
    /// [`File::write_direct_at`]) asks of a request's alignment (at
    /// first a sector for both), or `None`: the file system has no
    /// direct I/O, and it is unsupported.
    pub fn set_direct_align(&self, align: Option<DirectAlign>) {
        self.lock().direct_align = align;
    }

    /// The next `calls` direct reads and writes wait, before they do
    /// anything, until [`SimDisk::release_direct_io`] or until the
    /// process is gone; each counts in [`SimDisk::held_waiters`] while
    /// it waits. Later calls go on.
    pub fn hold_direct_io(&self, calls: u64) {
        self.lock().direct_holds = calls;
    }

    /// Let the held direct reads and writes go on, and hold no more.
    pub fn release_direct_io(&self) {
        let mut state = self.lock();
        state.direct_holds = 0;
        state.direct_releases += 1;
        drop(state);
        self.released.notify_all();
    }

    /// The direct read or write after the next `after` panics, before it
    /// does anything, as a bug in the call would.
    pub fn panic_in_direct_io(&self, after: u64) {
        self.lock().direct_panic = Some(after);
    }

    /// Whether [`File::cached_pages`] answers (it does at first) or is
    /// unsupported, as on a kernel without `cachestat`.
    pub fn set_cache_counts(&self, supported: bool) {
        self.lock().cache_counts = supported;
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
        state.sync_fault_of = None;
        state.write_fault_of = None;
        state.writeback_fault_of = None;
        state.read_fault_of = None;
        state.halt = None;
        state.read_fault = None;
    }

    /// Whether the file system can deallocate a range of a file
    /// ([`File::deallocate`]). It can, unless this says otherwise; a
    /// power cut does not change it.
    pub fn set_deallocation(&self, supported: bool) {
        self.lock().no_holes = !supported;
    }

    /// The block size the file system reports ([`Dir::block_size`],
    /// [`File::block_size`]), or `None` for one that cannot be asked.
    /// One sector unless set. A write, and a deallocation that ends
    /// inside a block, rewrite the other sectors of every block they
    /// touch, as they do a write unit's: the larger of the two counts.
    /// Like the write unit, it is a property of where the files are
    /// and not of the files: it can be set anew at any time, as when a
    /// directory is copied to another file system.
    pub fn set_block_size(&self, bytes: Option<u64>) {
        self.lock().block_size = bytes;
    }

    /// The page size the system reports ([`Dir::page_size`]). One
    /// sector unless set. It only says what the system would answer:
    /// how far a write reaches is the write unit's
    /// ([`SimDisk::set_write_unit`]), which a test sets with it.
    pub fn set_page_size(&self, bytes: u64) {
        self.lock().page_size = bytes;
    }

    /// The next deallocation fails with an I/O error, having zeroed any
    /// part of its range.
    pub fn fail_deallocation(&self) {
        self.lock().hole_fault = true;
    }

    /// Whether the file system can start writeback of a range
    /// ([`File::start_writeback`]). It can, unless this says otherwise.
    pub fn set_writeback(&self, supported: bool) {
        self.lock().no_writeback = !supported;
    }

    /// The next `calls` calls that start writeback, of any file, are
    /// declined this time ([`io::ErrorKind::OutOfMemory`], as `ENOMEM`)
    /// and do nothing; they are not recorded.
    pub fn refuse_writeback(&self, calls: u64) {
        self.lock().writeback_refusals = calls;
    }

    /// Every call that starts writeback of the file named `name` fails
    /// with an I/O error, and does nothing.
    pub fn fail_writeback_of(&self, name: &str) {
        self.lock().writeback_fault_of = Some(name.to_string());
    }

    /// A sync that ends later ([`File::sync_begin`]) ends after up to
    /// `calls` calls of [`File::sync_end`] that do not wait, as many as
    /// the seed chooses for each sync; at the first such call with 0,
    /// which is where it starts.
    pub fn set_sync_delay(&self, calls: u64) {
        self.lock().sync_delay = calls;
    }

    /// Syncs of the file named `name` that end later do not end until
    /// [`SimDisk::release_syncs`]: a call of [`File::sync_end`] that does
    /// not wait finds them under way, and one that waits waits.
    pub fn hold_syncs_of(&self, name: &str) {
        self.lock().held_syncs_of = Some(name.to_string());
    }

    /// Whole syncs ([`File::sync`]) of the file named `name` do not
    /// return until [`SimDisk::release_syncs`], or until the process is
    /// gone, as on a disk that stalls. The caller waits, and counts in
    /// [`SimDisk::held_waiters`].
    pub fn hold_whole_syncs_of(&self, name: &str) {
        self.lock().held_whole_syncs_of = Some(name.to_string());
    }

    /// The next `calls` calls of [`File::sync_begin`] begin nothing and
    /// say so ([`NotBegun`]), as a file whose helper cannot be started
    /// does.
    pub fn refuse_sync_begins(&self, calls: u64) {
        self.lock().begins_refused = calls;
    }

    /// How many callers wait now at the end of a held sync: a call of
    /// [`File::sync_end`] that waits, and found the sync held, or a
    /// whole sync held ([`SimDisk::hold_whole_syncs_of`]).
    pub fn held_waiters(&self) -> u64 {
        self.lock().held_waiting
    }

    /// How many calls of [`File::write_at`] and [`File::deallocate`] the
    /// file named `name` had while a sync of it that ends later
    /// ([`File::sync_begin`]) was under way: what such a sync may or may
    /// not cover.
    pub fn writes_while_syncing(&self, name: &str) -> u64 {
        self.lock()
            .writes_while_syncing
            .get(name)
            .copied()
            .unwrap_or(0)
    }

    /// Let the held syncs end.
    pub fn release_syncs(&self) {
        let mut state = self.lock();
        state.held_syncs_of = None;
        state.held_whole_syncs_of = None;
        drop(state);
        self.released.notify_all();
    }

    /// How many calls of [`File::write_at`] and [`File::deallocate`]
    /// the file named `name` has had so far, failed and refused ones
    /// included (counted under the name it had at the call).
    pub fn writes_of(&self, name: &str) -> u64 {
        self.lock().writes_of.get(name).copied().unwrap_or(0)
    }

    /// Every call that started writeback so far, in order: the file's
    /// name (where it still has one), the offset and the length.
    pub fn writebacks(&self) -> Vec<(String, u64, u64)> {
        self.lock().writebacks.clone()
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

    fn handle(&self, state: &mut State, inode: u64) -> SimFile {
        *state.open.entry(inode).or_default() += 1;
        SimFile {
            disk: self.clone(),
            inode,
            generation: state.generation,
            pending: None,
        }
    }
}

/// The simulated directory's lock, held. A kill and a power cut release
/// it, as the end of a process does; a holder that is dropped after
/// that releases nothing.
#[derive(Debug)]
pub struct SimLock {
    disk: SimDisk,
    holder: u64,
}

impl Drop for SimLock {
    fn drop(&mut self) {
        let mut state = self.disk.lock();
        if state.lock == Some(self.holder) {
            state.lock = None;
        }
    }
}

impl Dir for SimDisk {
    type File = SimFile;
    type Lock = SimLock;

    fn page_size(&self) -> u64 {
        self.lock().page_size
    }

    fn block_size(&self) -> io::Result<u64> {
        self.lock()
            .block_size
            .ok_or_else(|| io::Error::other("the file system does not say"))
    }

    /// The lock is no file here: taking it leaves the disk as it was.
    fn try_lock(&self) -> io::Result<SimLock> {
        let mut state = self.lock();
        if state.lock.is_some() {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let holder = state.next_lock;
        state.next_lock += 1;
        state.lock = Some(holder);
        Ok(SimLock {
            disk: self.clone(),
            holder,
        })
    }

    fn create(&self, name: &str) -> io::Result<SimFile> {
        let mut state = self.lock();
        if state.halted(false) {
            return Err(injected("halt"));
        }
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
        Ok(self.handle(&mut state, inode))
    }

    fn open(&self, name: &str) -> io::Result<SimFile> {
        let mut state = self.lock();
        match state.names.get(name).copied() {
            Some(inode) => Ok(self.handle(&mut state, inode)),
            None => Err(io::ErrorKind::NotFound.into()),
        }
    }

    fn remove(&self, name: &str) -> io::Result<()> {
        let mut state = self.lock();
        if state.halted(false) {
            return Err(injected("halt"));
        }
        if state.names.remove(name).is_none() {
            return Err(io::ErrorKind::NotFound.into());
        }
        state.pending_names.push(NameOp::Remove(name.to_owned()));
        Ok(())
    }

    fn rename(&self, from: &str, to: &str) -> io::Result<()> {
        let mut state = self.lock();
        if state.halted(false) {
            return Err(injected("halt"));
        }
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
        if state.halted(true) {
            return Err(injected("halt"));
        }
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

/// How a file is read or written: through the cache, from the device
/// for a check ([`File::read_device_at`]), or by direct I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Through {
    Cache,
    Device,
    Direct,
}

/// An open file of a [`SimDisk`]. It stops working when the disk's
/// process is killed or its power is cut.
#[derive(Debug)]
pub struct SimFile {
    disk: SimDisk,
    inode: u64,
    generation: u64,
    /// The sync that [`File::sync_begin`] began and that has not ended.
    pending: Option<PendingSync>,
}

/// A sync under way: what it is to make durable, and how many more
/// calls of `sync_end` that do not wait find it under way.
#[derive(Debug)]
struct PendingSync {
    /// By sector, how many of its unsynced versions were written before
    /// the sync began.
    versions: BTreeMap<u64, usize>,
    /// The file's length when it began.
    len: u64,
    polls: u64,
}

impl SimFile {
    /// A sync, of everything written so far or, for one that began
    /// earlier, of what was written before `begun`.
    fn sync_locked(&self, state: &mut State, begun: Option<&PendingSync>) -> io::Result<()> {
        self.check(state)?;
        if state.halted(true) {
            return Err(injected("halt"));
        }
        let named = state.sync_fault_of.as_deref();
        let failing = (named.is_some() && named == state.name_of(self.inode))
            || State::strikes(&mut state.sync_fault);
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
        match begun {
            None => inode.settle(),
            Some(begun) => inode.settle_before(begun),
        }
        state.stats.syncs += 1;
        Ok(())
    }

    /// A write through the cache, or by direct I/O past it.
    fn write(&self, offset: u64, data: &[u8], how: Through) -> io::Result<()> {
        if how == Through::Direct {
            self.direct_gate();
        }
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        if how == Through::Direct {
            state.direct_fits(offset, data.len(), data.as_ptr() as usize)?;
        }
        if state.halted(true) {
            return Err(injected("halt"));
        }
        if state.write_fault_of.is_some()
            && state.write_fault_of.as_deref() == state.name_of(self.inode)
        {
            return Err(injected("write"));
        }
        state.stats.writes += 1;
        state.stats.written += data.len() as u64;
        state.count_write(self.inode);
        let failing = State::strikes(&mut state.write_fault);
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        let rng = &mut state.rng;
        // A direct write writes its own sectors and nothing around them.
        let unit = match how {
            Through::Direct => SECTOR,
            _ => state.write_unit.max(state.block_size.unwrap_or(SECTOR)),
        };
        let inode = state.inodes.get_mut(&self.inode).ok_or_else(gone)?;
        let range = pages_of(offset, data.len() as u64);
        if how == Through::Direct {
            // The cache keeps no page of the range.
            inode.pages.retain(|page| !range.contains(page));
            inode.under_io.retain(|page| !range.contains(page));
        } else {
            // A write goes through the cache and leaves its pages there.
            inode.pages.extend(range);
        }
        // The units the write touches, or the file system's blocks if
        // those are larger, are rewritten as a whole: their other
        // sectors are written again as they are.
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
                sector.direct = false;
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
            // A direct write of part of a sector starts from what the
            // device holds, not from what the cache shows.
            let base = match how {
                Through::Direct => sector.device(),
                _ => sector.current(),
            };
            let mut bytes: Box<[u8]> = match base {
                Some(current) => current.into(),
                None => vec![0u8; SECTOR_USIZE].into_boxed_slice(),
            };
            bytes[from..from + n].copy_from_slice(src);
            // The page is dirty again, whatever a failed sync made of it.
            sector.cached = None;
            sector.unsynced.push(bytes);
            sector.direct = how == Through::Direct;
            inode.len = inode.len.max(index * SECTOR + (from + n) as u64);
        }
        if failing {
            Err(injected("write"))
        } else {
            Ok(())
        }
    }

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

impl Drop for SimFile {
    fn drop(&mut self) {
        let mut state = self.disk.lock();
        if state.generation != self.generation {
            // The process this handle belonged to is gone already.
            return;
        }
        if self.pending.is_some() {
            state.sync_over(self.inode);
        }
        if let Some(count) = state.open.get_mut(&self.inode) {
            *count -= 1;
            if *count == 0 {
                state.open.remove(&self.inode);
                state.drop_unreachable();
            }
        }
    }
}

fn gone() -> io::Error {
    io::Error::other("the file no longer exists")
}

impl SimFile {
    /// Where a direct read or write begins: it waits while held
    /// ([`SimDisk::hold_direct_io`]), and panics where asked
    /// ([`SimDisk::panic_in_direct_io`]), outside the disk's lock.
    fn direct_gate(&self) {
        let disk = &self.disk;
        let mut guard = disk.lock();
        if guard.direct_holds > 0 {
            guard.direct_holds -= 1;
            let release = guard.direct_releases;
            guard.held_waiting += 1;
            while guard.direct_releases == release && guard.generation == self.generation {
                guard = disk
                    .released
                    .wait(guard)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            guard.held_waiting -= 1;
        }
        let panics = match &mut guard.direct_panic {
            Some(0) => {
                guard.direct_panic = None;
                true
            }
            Some(left) => {
                *left -= 1;
                false
            }
            None => false,
        };
        drop(guard);
        assert!(!panics, "injected panic in direct I/O");
    }

    /// Read through `how`; `ahead` lets a read through the cache take
    /// a held read-ahead ([`SimDisk::hold_read_ahead_of`]).
    fn read(&self, offset: u64, buf: &mut [u8], how: Through, ahead: bool) -> io::Result<()> {
        if how == Through::Direct {
            self.direct_gate();
        }
        let mut state = self.disk.lock();
        self.check(&state)?;
        if how == Through::Device && !state.device_reads {
            return Err(io::ErrorKind::Unsupported.into());
        }
        if how == Through::Direct {
            state.direct_fits(offset, buf.len(), buf.as_ptr() as usize)?;
        }
        let device = how != Through::Cache;
        if state.read_fault_of.is_some()
            && state.read_fault_of.as_deref() == state.name_of(self.inode)
        {
            return Err(injected("read"));
        }
        if State::strikes(&mut state.read_fault) {
            return Err(injected("read"));
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
        // A read through the cache leaves its pages there, and its
        // read-ahead's under I/O where one is held.
        if !device {
            let ahead = match &state.read_ahead_of {
                Some((name, pages))
                    if ahead && state.name_of(self.inode) == Some(name.as_str()) =>
                {
                    let pages = *pages;
                    state.read_ahead_of = None;
                    pages
                }
                _ => 0,
            };
            if let Some(inode) = state.inodes.get_mut(&self.inode) {
                let read = pages_of(offset, buf.len() as u64);
                inode.under_io.extend(read.end..read.end + ahead);
                inode.pages.extend(read);
            }
        }
        Ok(())
    }
}

impl File for SimFile {
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf, Through::Cache, true)
    }

    /// A read through the cache that takes no held read-ahead, as with
    /// `POSIX_FADV_RANDOM`.
    fn read_alone_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf, Through::Cache, false)
    }

    fn read_device_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf, Through::Device, false)
    }

    fn direct_align(&self) -> io::Result<DirectAlign> {
        let state = self.disk.lock();
        self.check(&state)?;
        state
            .direct_align
            .ok_or_else(|| io::ErrorKind::Unsupported.into())
    }

    fn read_direct_at(&self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        self.read(offset, buf, Through::Direct, false)
    }

    fn write_direct_at(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        self.write(offset, data, Through::Direct)
    }

    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        self.write(offset, data, Through::Cache)
    }

    fn sync(&mut self) -> io::Result<()> {
        let disk = self.disk.clone();
        let mut guard = disk.lock();
        while guard.held_whole_syncs_of.is_some()
            && guard.held_whole_syncs_of.as_deref() == guard.name_of(self.inode)
            && guard.generation == self.generation
        {
            guard.held_waiting += 1;
            guard = disk
                .released
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.held_waiting -= 1;
        }
        self.sync_locked(&mut guard, None)
    }

    fn sync_begin(&mut self, wake: Box<dyn FnOnce() + Send>) -> io::Result<()> {
        // Nothing wakes: the caller asks (`sync_wakes`).
        drop(wake);
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        if state.halted(false) {
            return Err(injected("halt"));
        }
        if state.begins_refused > 0 {
            state.begins_refused -= 1;
            return Err(NotBegun::error("the simulated disk refused it"));
        }
        let delay = state.sync_delay;
        // The seed is drawn from only with a delay, so that what a seed
        // decides elsewhere is as it was without one.
        let polls = if delay > 0 {
            state.rng.below(delay + 1)
        } else {
            0
        };
        let inode = state.inodes.get(&self.inode).ok_or_else(gone)?;
        let versions = inode
            .sectors
            .iter()
            .filter(|(_, sector)| !sector.unsynced.is_empty())
            .map(|(index, sector)| (*index, sector.unsynced.len()))
            .collect();
        self.pending = Some(PendingSync {
            versions,
            len: inode.len,
            polls,
        });
        *state.syncing.entry(self.inode).or_default() += 1;
        state.stats.sync_begins += 1;
        Ok(())
    }

    fn sync_end(&mut self, wait: bool) -> Option<io::Result<()>> {
        let disk = self.disk.clone();
        let mut guard = disk.lock();
        // Held: under way until released, or until the process is gone.
        loop {
            let held = guard.held_syncs_of.is_some()
                && guard.held_syncs_of.as_deref() == guard.name_of(self.inode)
                && guard.generation == self.generation;
            if !held {
                break;
            }
            if !wait {
                return None;
            }
            guard.held_waiting += 1;
            guard = disk
                .released
                .wait(guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.held_waiting -= 1;
        }
        let Some(mut pending) = self.pending.take() else {
            // Nothing was begun: a whole sync, as by default.
            return Some(self.sync_locked(&mut guard, None));
        };
        if !wait && pending.polls > 0 {
            pending.polls -= 1;
            self.pending = Some(pending);
            return None;
        }
        guard.sync_over(self.inode);
        Some(self.sync_locked(&mut guard, Some(&pending)))
    }

    fn preallocate(&mut self, len: u64) -> io::Result<()> {
        let mut state = self.disk.lock();
        self.check(&state)?;
        if state.halted(false) {
            return Err(injected("halt"));
        }
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

    fn extend(&mut self, len: u64) -> io::Result<()> {
        // The simulated disk reserves nothing in either case.
        self.preallocate(len)
    }

    /// A hint that changes nothing the disk holds or will hold: the
    /// simulated disk keeps no cache apart from what a sync makes
    /// durable, so there is nothing to start. It is recorded
    /// ([`SimDisk::writebacks`]), is no operation for
    /// [`SimDisk::halt_after`], and does not fail on a halted machine:
    /// it fails only where a test says so ([`SimDisk::set_writeback`],
    /// [`SimDisk::fail_writeback_of`], [`SimDisk::refuse_writeback`]),
    /// so that whatever a store does
    /// on this disk, and what each request is answered, is the same
    /// with the hint as without it.
    fn start_writeback(&mut self, offset: u64, len: u64) -> io::Result<()> {
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        if state.no_writeback {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let name = state.name_of(self.inode).map(str::to_string);
        if state.writeback_fault_of.is_some() && state.writeback_fault_of == name {
            return Err(injected("writeback"));
        }
        if state.writeback_refusals > 0 {
            state.writeback_refusals -= 1;
            return Err(io::ErrorKind::OutOfMemory.into());
        }
        state.stats.writebacks += 1;
        state.stats.writeback_bytes += len;
        state
            .writebacks
            .push((name.unwrap_or_default(), offset, len));
        Ok(())
    }

    /// A hole that is not synced yet is one more version of each of
    /// its sectors, zeros, so a power cut leaves each as it was or as
    /// zeros, as it does a write's (or as garbage, where the cut makes
    /// garbage of what was being rewritten: more than a hole can do,
    /// and a store that survives it survives a hole). Where the range
    /// ends inside a write unit or inside a block of the file system,
    /// whichever is larger, that unit's or block's other sectors are
    /// rewritten as they are, as by a write: the file system zeroes a
    /// part of a block by writing it. Whole ones inside the range take
    /// no neighbour with them. It counts as one operation for
    /// [`SimDisk::halt_after`].
    fn deallocate(&mut self, offset: u64, len: u64) -> io::Result<()> {
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        if state.halted(true) {
            return Err(injected("halt"));
        }
        if state.no_holes {
            return Err(io::ErrorKind::Unsupported.into());
        }
        let end = offset
            .checked_add(len)
            .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
        state.count_write(self.inode);
        let failing = std::mem::take(&mut state.hole_fault);
        let rng = &mut state.rng;
        let unit = state.write_unit.max(state.block_size.unwrap_or(SECTOR));
        let inode = state.inodes.get_mut(&self.inode).ok_or_else(gone)?;
        if !offset.is_multiple_of(SECTOR) || !len.is_multiple_of(SECTOR) || end > inode.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "a deallocation of whole blocks inside the file",
            ));
        }
        state.stats.deallocated += len;
        let zeros = || vec![0u8; SECTOR_USIZE].into_boxed_slice();
        if unit > SECTOR && len > 0 {
            // The units the range covers only in part.
            let head = offset / unit * unit..offset;
            let tail = end..(end.div_ceil(unit) * unit).min(inode.len);
            for index in (head.start / SECTOR..head.end / SECTOR)
                .chain(tail.start / SECTOR..tail.end.div_ceil(SECTOR))
            {
                let sector = inode.sectors.entry(index).or_default();
                let same: Box<[u8]> = match sector.current() {
                    Some(current) => current.into(),
                    None => zeros(),
                };
                sector.cached = None;
                sector.unsynced.push(same);
                sector.direct = false;
            }
        }
        for index in offset / SECTOR..end / SECTOR {
            // A failed deallocation leaves any part of its range zero.
            if failing && rng.coin() {
                continue;
            }
            let sector = inode.sectors.entry(index).or_default();
            sector.cached = None;
            sector.unsynced.push(zeros());
            sector.direct = false;
        }
        if failing {
            Err(injected("deallocate"))
        } else {
            Ok(())
        }
    }

    fn block_size(&self) -> io::Result<u64> {
        let state = self.disk.lock();
        self.check(&state)?;
        state
            .block_size
            .ok_or_else(|| io::Error::other("the file system does not say"))
    }

    /// The file's pages leave the cache: what a failed sync left only
    /// there is gone, as [`SimDisk::drop_caches`] does for every file;
    /// unsynced versions stay, as dirty pages do (and count). With
    /// [`SimDisk::keep_cache`] or [`SimDisk::keep_cache_for`], nothing
    /// leaves.
    fn drop_cache(&mut self) -> io::Result<()> {
        let mut guard = self.disk.lock();
        let state = &mut *guard;
        self.check(state)?;
        if state.cache_kept > 0 {
            state.cache_kept -= 1;
            return Ok(());
        }
        let inode = state.inodes.get_mut(&self.inode).ok_or_else(gone)?;
        inode.pages.clear();
        for sector in inode.sectors.values_mut() {
            sector.cached = None;
        }
        Ok(())
    }

    /// The pages read or written through the cache since it last lost
    /// them, those a held read-ahead brings in, and those that hold a
    /// version the disk has not (a failed sync's, or an unsynced one
    /// written through the cache: one written by direct I/O is on the
    /// device and in no page). With [`SimDisk::blind_cache_count`], 0.
    fn cached_pages(&self) -> io::Result<u64> {
        let guard = self.disk.lock();
        self.check(&guard)?;
        if !guard.cache_counts {
            return Err(io::ErrorKind::Unsupported.into());
        }
        if guard.cache_count_blind {
            return Ok(0);
        }
        let inode = guard.inodes.get(&self.inode).ok_or_else(gone)?;
        let mut pages = inode.pages.clone();
        pages.extend(&inode.under_io);
        pages.extend(
            inode
                .sectors
                .iter()
                .filter(|(_, sector)| {
                    sector.cached.is_some() || (!sector.unsynced.is_empty() && !sector.direct)
                })
                .map(|(index, _)| index * SECTOR / 4096),
        );
        Ok(pages.len() as u64)
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

    /// Direct I/O, the unit: on a disk whose writes through the cache
    /// rewrite 16 KiB around them, a direct write writes its own
    /// sectors and nothing else, so a cut that turns every sector it can
    /// to garbage leaves a block beside it as it was. Through the cache,
    /// the same write exposes that block.
    #[test]
    fn a_direct_write_writes_only_its_own_sectors() {
        for direct in [true, false] {
            let disk = disk_with(32, 1);
            disk.set_write_unit(16 << 10);
            let mut f = disk.open("f").unwrap();
            let data = skyzfs_blk::Buf::filled(1, 2);
            if direct {
                f.write_direct_at(4096, &data).unwrap();
            } else {
                f.write_at(4096, &data).unwrap();
            }
            disk.power_cut(BySector(|_: Option<&str>, _| Fate::Garbage));
            let beside = read(&disk, "f", 0, 4096);
            assert_eq!(beside == vec![1; 4096], direct, "direct: {direct}");
        }
    }

    /// Direct I/O and the cache: a direct write leaves no page of its
    /// range in the cache, also before a sync makes it durable (it is
    /// in the device's volatile cache, which a count of the page cache
    /// does not see), and a direct read brings none in. A write through
    /// the cache leaves a dirty page that a drop does not drop until a
    /// sync.
    #[test]
    fn direct_io_leaves_no_page_in_the_cache() {
        let disk = disk_with(16, 1);
        let mut f = disk.open("f").unwrap();
        f.drop_cache().unwrap();
        let mut buf = skyzfs_blk::Buf::filled(1, 0);
        f.read_at(0, &mut buf).unwrap();
        assert_eq!(f.cached_pages().unwrap(), 1);
        f.write_direct_at(0, &skyzfs_blk::Buf::filled(1, 2))
            .unwrap();
        assert_eq!(f.cached_pages().unwrap(), 0, "unsynced");
        f.write_at(4096, &skyzfs_blk::Buf::filled(1, 3)).unwrap();
        f.drop_cache().unwrap();
        assert_eq!(f.cached_pages().unwrap(), 1, "a dirty page");
        f.sync().unwrap();
        f.drop_cache().unwrap();
        assert_eq!(f.cached_pages().unwrap(), 0);
        f.read_direct_at(0, &mut buf).unwrap();
        assert_eq!(&buf[..], &[2; 4096][..]);
        assert_eq!(f.cached_pages().unwrap(), 0);
    }

    /// Direct I/O, alignment: a request that is not aligned as the file
    /// system asks fails, and on a file system with no direct I/O every
    /// request is unsupported.
    #[test]
    fn direct_io_refuses_what_is_not_aligned() {
        let disk = disk_with(16, 1);
        let f = disk.open("f").unwrap();
        disk.set_direct_align(Some(DirectAlign {
            memory: 4096,
            offset: 4096,
        }));
        assert_eq!(
            f.direct_align().unwrap(),
            DirectAlign {
                memory: 4096,
                offset: 4096
            }
        );
        let data = skyzfs_blk::Buf::filled(2, 2);
        let kind = |r: io::Result<()>| r.unwrap_err().kind();
        assert_eq!(
            kind(f.write_direct_at(512, &data[..4096])),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            kind(f.write_direct_at(0, &data[..512])),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(
            kind(f.write_direct_at(0, &data[512..4608])),
            io::ErrorKind::InvalidInput
        );
        f.write_direct_at(4096, &data[..4096]).unwrap();
        disk.set_direct_align(None);
        assert_eq!(
            f.direct_align().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert_eq!(
            kind(f.write_direct_at(0, &data[..4096])),
            io::ErrorKind::Unsupported
        );
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
    /// any subset survives, applied in order. Here the creation without
    /// the rename and the removal that followed it; and a later
    /// creation without an earlier one.
    #[test]
    fn directory_operations_survive_as_any_subset() {
        struct Keep(Vec<bool>);
        impl Cut for Keep {
            fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> Fate {
                Fate::Latest
            }
            fn name(&mut self, index: usize, pending: usize) -> bool {
                assert_eq!(pending, self.0.len());
                self.0[index]
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
        disk.power_cut(Keep(vec![true, false, false]));
        assert_eq!(disk.list().unwrap(), vec!["a".to_owned(), "f".to_owned()]);

        disk.create("x").unwrap();
        disk.create("y").unwrap();
        disk.power_cut(Keep(vec![false, true]));
        assert_eq!(
            disk.list().unwrap(),
            vec!["a".to_owned(), "f".to_owned(), "y".to_owned()]
        );
    }

    /// Injected errors: a failed read, through the cache or from the
    /// device, returns an error and changes nothing; the read after it
    /// works.
    #[test]
    fn a_failed_read_changes_nothing() {
        let disk = disk_with(2, 5);
        let f = disk.open("f").unwrap();
        let before = disk.image();
        let writes = disk.stats().writes;
        let mut buf = [0u8; 16];
        disk.fail_read(1, false);
        f.read_at(0, &mut buf).unwrap();
        assert!(f.read_device_at(0, &mut buf).is_err());
        f.read_device_at(0, &mut buf).unwrap();
        assert_eq!(buf, [5u8; 16]);
        disk.fail_read(0, false);
        assert!(f.read_at(0, &mut buf).is_err());
        assert_eq!(disk.image(), before);
        assert_eq!(disk.stats().writes, writes);
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

    /// Open handles: a file that was removed, or replaced by a rename,
    /// keeps working through a handle that is open on it, also after
    /// the directory was synced; the name shows the new file. A kill
    /// ends the handle, and so does a power cut.
    #[test]
    fn open_handles_outlive_removal_and_replacement_across_a_directory_sync() {
        for replace in [false, true] {
            for cut in [false, true] {
                let disk = disk_with(1, 1);
                let mut old = disk.open("f").unwrap();
                if replace {
                    let mut new = disk.create("n").unwrap();
                    new.write_at(0, &[7; 4]).unwrap();
                    new.sync().unwrap();
                    disk.rename("n", "f").unwrap();
                } else {
                    disk.remove("f").unwrap();
                }
                Dir::sync(&disk).unwrap();
                let mut buf = [0u8; 4];
                old.read_at(0, &mut buf).unwrap();
                assert_eq!(buf, [1; 4]);
                old.write_at(0, &[2; 4]).unwrap();
                old.sync().unwrap();
                old.read_device_at(0, &mut buf).unwrap();
                assert_eq!(buf, [2; 4]);
                if replace {
                    assert_eq!(read(&disk, "f", 0, 4), [7; 4]);
                } else {
                    assert_eq!(disk.open("f").unwrap_err().kind(), io::ErrorKind::NotFound);
                }
                if cut {
                    disk.power_cut(KeepAll);
                } else {
                    disk.kill();
                }
                assert!(old.read_at(0, &mut buf).is_err());
                assert_eq!(disk.list().unwrap().len(), usize::from(replace));
            }
        }
    }

    /// Names: a file that was created and removed again before any
    /// directory sync is still there if a power cut keeps the creation
    /// and loses the removal, with its synced content, also when its
    /// last handle was dropped or the process was killed before the
    /// cut. The same when it was renamed in between.
    #[test]
    fn a_power_cut_can_bring_back_a_file_that_was_created_and_removed() {
        struct Keep(Vec<bool>);
        impl Cut for Keep {
            fn sector(&mut self, _: Option<&str>, _: u64, _: usize) -> Fate {
                Fate::Latest
            }
            fn name(&mut self, index: usize, pending: usize) -> bool {
                assert_eq!(pending, self.0.len());
                self.0[index]
            }
            fn length(&mut self, _: Option<&str>) -> bool {
                true
            }
        }
        for rename in [false, true] {
            for kill in [false, true] {
                let disk = SimDisk::new(0);
                let mut a = disk.create("a").unwrap();
                a.write_at(0, &[4; 8]).unwrap();
                a.sync().unwrap();
                let last = if rename {
                    disk.rename("a", "b").unwrap();
                    "b"
                } else {
                    "a"
                };
                disk.remove(last).unwrap();
                drop(a);
                if kill {
                    disk.kill();
                }
                assert!(disk.dangling_names().is_empty());
                // The creation survives, and the rename; the removal
                // does not.
                let kept = if rename {
                    vec![true, true, false]
                } else {
                    vec![true, false]
                };
                disk.power_cut(Keep(kept));
                assert_eq!(disk.list().unwrap(), vec![last.to_owned()]);
                assert!(disk.dangling_names().is_empty());
                assert_eq!(read(&disk, last, 0, 8), [4; 8]);
                // Once the removal is durable the file is gone for good.
                disk.remove(last).unwrap();
                Dir::sync(&disk).unwrap();
                disk.power_cut(KeepAll);
                assert!(disk.list().unwrap().is_empty());
            }
        }
    }

    /// Injected errors: with the syncs of one file failing, that file
    /// stays unsynced while the others sync as usual.
    #[test]
    fn the_syncs_of_one_file_can_fail() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        let mut g = disk.create("g").unwrap();
        disk.fail_sync_of("f");
        f.write_at(0, &[2; 4]).unwrap();
        g.write_at(0, &[3; 4]).unwrap();
        g.sync().unwrap();
        assert!(f.sync().is_err());
        assert!(f.sync().is_err());
        Dir::sync(&disk).unwrap();
        disk.clear_faults();
        g.sync().unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "g", 0, 4), [3; 4]);
    }

    /// The lock: one holder at a time; dropping it, a kill and a power
    /// cut release it, and a holder from before a kill releases nothing
    /// that was taken after.
    #[test]
    fn the_directory_lock_has_one_holder_and_dies_with_the_process() {
        let disk = disk_with(1, 1);
        let first = disk.try_lock().unwrap();
        let refused = disk.try_lock().unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::WouldBlock);
        drop(first);
        let second = disk.try_lock().unwrap();
        disk.kill();
        let third = disk.try_lock().unwrap();
        drop(second);
        assert!(disk.try_lock().is_err());
        disk.power_cut(LoseAll);
        let fourth = disk.try_lock().unwrap();
        drop(third);
        assert!(disk.try_lock().is_err());
        drop(fourth);
        disk.try_lock().unwrap();
    }

    /// A deallocated range reads as zeros at once and keeps the file's
    /// length. Until a sync it is one more version of each sector: a
    /// power cut leaves each as it was or as zeros. What lies beside
    /// the range is not touched, and with a write unit of one sector
    /// has no pending version at all.
    #[test]
    fn a_hole_reads_as_zeros_and_is_durable_only_after_a_sync() {
        let disk = disk_with(8, 7);
        let mut f = disk.open("f").unwrap();
        f.deallocate(2 * SECTOR, 3 * SECTOR).unwrap();
        assert_eq!(f.len().unwrap(), 8 * SECTOR);
        assert_eq!(read(&disk, "f", 0, 2 * S), vec![7; 2 * S]);
        assert_eq!(read(&disk, "f", 2 * SECTOR, 3 * S), vec![0; 3 * S]);
        assert_eq!(read(&disk, "f", 5 * SECTOR, 3 * S), vec![7; 3 * S]);
        assert_eq!(disk.stats().deallocated, 3 * SECTOR);
        // Not synced: sector 2 is lost, sector 3 kept, and a sector
        // outside the range has nothing pending to make garbage of.
        disk.power_cut(BySector(|_: Option<&str>, index| match index {
            2 => Fate::Durable,
            3 => Fate::Latest,
            _ => Fate::Garbage,
        }));
        assert_eq!(read(&disk, "f", 0, 3 * S), vec![7; 3 * S]);
        assert_eq!(read(&disk, "f", 3 * SECTOR, S), vec![0; S]);
        assert_eq!(read(&disk, "f", 5 * SECTOR, 3 * S), vec![7; 3 * S]);
        // Synced: it survives whatever the cut does.
        let mut f = disk.open("f").unwrap();
        f.deallocate(0, 2 * SECTOR).unwrap();
        f.sync().unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, 2 * S), vec![0; 2 * S]);
    }

    /// Where a hole begins or ends inside a write unit, the unit's
    /// other sectors are rewritten, as by a write: a power cut can
    /// make garbage of them. Units that the hole covers whole take no
    /// neighbour with them.
    #[test]
    fn a_hole_that_ends_inside_a_write_unit_rewrites_the_unit() {
        let disk = disk_with(16, 7);
        disk.set_write_unit(4 * SECTOR);
        let mut f = disk.open("f").unwrap();
        // Sectors 2 to 9: units 0 and 2 in part, unit 1 whole.
        f.deallocate(2 * SECTOR, 8 * SECTOR).unwrap();
        disk.power_cut(BySector(|_: Option<&str>, _| Fate::Garbage));
        let now = read(&disk, "f", 0, 16 * S);
        let same = |sector: usize| now[sector * S..(sector + 1) * S] == [7u8; S];
        // The neighbours in the two edge units had a pending version.
        assert!(!same(0) && !same(1) && !same(10) && !same(11));
        // Unit 3 was not touched.
        assert!((12..16).all(same));
    }

    /// The same for a block of the file system that is larger than the
    /// write unit: a hole that ends inside one rewrites the block's
    /// other sectors. A disk that does not say its block size refuses
    /// the question.
    #[test]
    fn a_hole_that_ends_inside_a_block_rewrites_the_block() {
        let disk = disk_with(16, 7);
        disk.set_write_unit(2 * SECTOR);
        let f = disk.open("f").unwrap();
        assert_eq!(f.block_size().unwrap(), SECTOR);
        disk.set_block_size(Some(8 * SECTOR));
        assert_eq!(f.block_size().unwrap(), 8 * SECTOR);
        let mut f = f;
        // Sectors 2 and 3: one whole write unit inside block 0.
        f.deallocate(2 * SECTOR, 2 * SECTOR).unwrap();
        disk.power_cut(BySector(|_: Option<&str>, _| Fate::Garbage));
        let now = read(&disk, "f", 0, 16 * S);
        let same = |sector: usize| now[sector * S..(sector + 1) * S] == [7u8; S];
        assert!((0..8).all(|sector| !same(sector)));
        assert!((8..16).all(same));
        disk.set_block_size(None);
        assert!(disk.open("f").unwrap().block_size().is_err());
    }

    /// A file system that cannot deallocate says so and changes
    /// nothing; a deallocation that fails may have zeroed any part of
    /// its range and nothing outside it; a range that is not of whole
    /// sectors inside the file is refused.
    #[test]
    fn a_hole_refused_changes_nothing_and_one_that_fails_stays_in_its_range() {
        let disk = disk_with(8, 7);
        let mut f = disk.open("f").unwrap();
        disk.set_deallocation(false);
        let refused = f.deallocate(0, 4 * SECTOR).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::Unsupported);
        assert_eq!(read(&disk, "f", 0, 8 * S), vec![7; 8 * S]);
        assert_eq!(disk.stats().deallocated, 0);
        disk.set_deallocation(true);
        assert!(f.deallocate(1, SECTOR).is_err());
        assert!(f.deallocate(4 * SECTOR, 8 * SECTOR).is_err());
        disk.fail_deallocation();
        assert!(f.deallocate(0, 4 * SECTOR).is_err());
        let now = read(&disk, "f", 0, 8 * S);
        for sector in 0..4 {
            let bytes = &now[sector * S..(sector + 1) * S];
            assert!(bytes == [7u8; S] || bytes == [0u8; S], "sector {sector}");
        }
        assert_eq!(now[4 * S..], vec![7; 4 * S]);
        // The fault struck once.
        f.deallocate(0, 4 * SECTOR).unwrap();
        assert_eq!(read(&disk, "f", 0, 4 * S), vec![0; 4 * S]);
    }

    /// Injected errors: a halted disk takes the writes and syncs it was
    /// still given, then refuses every write, sync, length and name
    /// change and keeps nothing of them, until the power is cut.
    #[test]
    fn a_halted_disk_takes_nothing_more() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        disk.halt_after(2);
        f.write_at(0, &[2; 4]).unwrap();
        f.sync().unwrap();
        assert!(f.write_at(0, &[3; 4]).is_err());
        assert!(f.sync().is_err());
        assert!(f.preallocate(8 * SECTOR).is_err());
        assert!(disk.create("g").is_err());
        assert!(disk.rename("f", "h").is_err());
        assert!(disk.remove("f").is_err());
        assert!(Dir::sync(&disk).is_err());
        assert_eq!(read(&disk, "f", 0, 4), [2; 4]);
        assert_eq!(disk.list().unwrap(), ["f"]);
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, 4), [2; 4]);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &[4; 4]).unwrap();
        f.sync().unwrap();
    }

    /// Injected errors: the writes and the reads of one file can fail
    /// while the others work, and a write that fails so leaves nothing.
    #[test]
    fn the_writes_and_reads_of_one_file_can_fail() {
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        let mut g = disk.create("g").unwrap();
        f.write_at(0, &[1; 4]).unwrap();
        disk.fail_write_of("f");
        assert!(f.write_at(0, &[2; 4]).is_err());
        g.write_at(0, &[3; 4]).unwrap();
        assert_eq!(read(&disk, "f", 0, 4), [1; 4]);
        disk.fail_read_of("f");
        let mut buf = [0u8; 4];
        assert!(f.read_at(0, &mut buf).is_err());
        assert!(f.read_device_at(0, &mut buf).is_err());
        assert_eq!(read(&disk, "g", 0, 4), [3; 4]);
        disk.clear_faults();
        f.write_at(0, &[2; 4]).unwrap();
        assert_eq!(read(&disk, "f", 0, 4), [2; 4]);
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

    /// The file system's block: where it is larger than the write
    /// unit, a small write puts the durable sectors of the whole block
    /// around it at risk, in whatever file, and the directory says the
    /// same size as its files.
    #[test]
    fn a_write_rewrites_its_whole_file_system_block() {
        let disk = disk_with(32, 1);
        disk.set_write_unit(2 * SECTOR);
        disk.set_block_size(Some(16 * SECTOR));
        assert_eq!(Dir::block_size(&disk).unwrap(), 16 * SECTOR);
        let mut f = disk.open("f").unwrap();
        // One byte in the second block (sectors 16 to 31).
        f.write_at(20 * SECTOR + 7, &[2]).unwrap();
        assert_eq!(disk.unsynced_sectors(), 16);
        disk.power_cut(BySector(|_: Option<&str>, _| Fate::Garbage));
        let got = read(&disk, "f", 0, 32 * S);
        assert_eq!(got[..16 * S], vec![1; 16 * S][..]);
        for sector in 16..32 {
            assert_ne!(
                got[sector * S..(sector + 1) * S],
                vec![1; S][..],
                "{sector}"
            );
        }
        disk.set_block_size(None);
        assert!(Dir::block_size(&disk).is_err());
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

    /// A sync that ends later makes durable what was written before its
    /// begin, and nothing written after it; a power cut between its
    /// begin and its end loses what any unsynced write would.
    #[test]
    fn a_sync_that_ends_later_covers_what_was_written_before_it_began() {
        let disk = disk_with(3, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![2; S]).unwrap();
        f.sync_begin(Box::new(|| {})).unwrap();
        f.write_at(S as u64, &vec![3; S]).unwrap();
        f.write_at(0, &vec![4; S]).unwrap();
        assert!(f.sync_end(false).unwrap().is_ok());
        disk.power_cut(LoseAll);
        // The first sector as it was at the begin; the later writes lost.
        assert_eq!(read(&disk, "f", 0, S), vec![2; S]);
        assert_eq!(read(&disk, "f", S as u64, S), vec![1; S]);

        // Cut between begin and end: as if no sync had begun.
        let disk = disk_with(1, 1);
        let mut f = disk.open("f").unwrap();
        f.write_at(0, &vec![5; S]).unwrap();
        f.sync_begin(Box::new(|| {})).unwrap();
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", 0, S), vec![1; S]);
        assert!(f.sync_end(true).unwrap().is_err());
    }

    /// A whole sync between the begin and the end keeps what it made
    /// durable, the file's length included; a refused begin begins
    /// nothing.
    #[test]
    fn a_whole_sync_between_keeps_its_length_and_a_refused_begin_begins_nothing() {
        let disk = SimDisk::new(1);
        let mut f = disk.create("f").unwrap();
        Dir::sync(&disk).unwrap();
        f.write_at(0, &[1; S]).unwrap();
        f.sync_begin(Box::new(|| {})).unwrap();
        f.write_at(S as u64, &[2; S]).unwrap();
        f.sync().unwrap();
        assert!(f.sync_end(true).unwrap().is_ok());
        disk.power_cut(LoseAll);
        assert_eq!(read(&disk, "f", S as u64, S), vec![2; S]);

        let disk = SimDisk::new(1);
        let mut f = disk.create("f").unwrap();
        disk.refuse_sync_begins(1);
        let refused = f.sync_begin(Box::new(|| {})).unwrap_err();
        assert!(NotBegun::is(&refused), "{refused}");
        assert_eq!(disk.stats().sync_begins, 0);
        f.sync_begin(Box::new(|| {})).unwrap();
        assert_eq!(disk.stats().sync_begins, 1);
    }

    /// With a delay the end comes after as many calls that do not wait
    /// as the seed chooses, at most the delay; a call that waits ends
    /// it at once; the end counts for a halt where it is, not the
    /// begin.
    #[test]
    fn a_delayed_sync_ends_within_its_delay_or_when_waited_for() {
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..40 {
            let disk = SimDisk::new(seed);
            disk.set_sync_delay(3);
            let mut f = disk.create("f").unwrap();
            f.write_at(0, &[1; S]).unwrap();
            f.sync_begin(Box::new(|| {})).unwrap();
            let mut polls = 0;
            while f.sync_end(false).is_none() {
                polls += 1;
                assert!(polls <= 3);
            }
            seen.insert(polls);
            assert_eq!(disk.stats().syncs, 1);
            assert_eq!(disk.stats().sync_begins, 1);
        }
        assert_eq!(seen.len(), 4, "{seen:?}");
        let disk = SimDisk::new(1);
        disk.set_sync_delay(1000);
        let mut f = disk.create("f").unwrap();
        f.write_at(0, &[1; S]).unwrap();
        f.sync_begin(Box::new(|| {})).unwrap();
        disk.halt_after(1);
        f.write_at(0, &[2; S]).unwrap();
        // The halt took the write; the end is refused.
        assert!(f.sync_end(true).unwrap().is_err());
    }

    /// A caller waiting at the end of a held sync is let go by a power
    /// cut or a kill, and finds its handle dead.
    #[test]
    fn a_power_cut_or_a_kill_lets_a_waiter_at_a_held_sync_go() {
        for cut in [true, false] {
            let disk = SimDisk::new(1);
            let mut f = disk.create("f").unwrap();
            f.write_at(0, &[1; S]).unwrap();
            disk.hold_syncs_of("f");
            f.sync_begin(Box::new(|| {})).unwrap();
            let (done, ended) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = done.send(f.sync_end(true).map(|r| r.is_err()));
            });
            while disk.held_waiters() == 0 {
                std::thread::yield_now();
            }
            if cut {
                disk.power_cut(LoseAll);
            } else {
                disk.kill();
            }
            let ended = ended.recv_timeout(std::time::Duration::from_secs(20));
            assert_eq!(ended, Ok(Some(true)), "cut {cut}");
        }
    }

    /// A held sync is under way for a call that does not wait, and ends
    /// for one that waits once it is released; writes to each file are
    /// counted by name.
    #[test]
    fn a_held_sync_ends_when_released() {
        let disk = SimDisk::new(1);
        let mut f = disk.create("f").unwrap();
        f.write_at(0, &[1; S]).unwrap();
        f.write_at(0, &[2; S]).unwrap();
        assert_eq!(disk.writes_of("f"), 2);
        assert_eq!(disk.writes_of("g"), 0);
        disk.hold_syncs_of("f");
        f.sync_begin(Box::new(|| {})).unwrap();
        for _ in 0..5 {
            assert!(f.sync_end(false).is_none());
        }
        // A write while the sync is under way is counted as such.
        f.write_at(S as u64, &[3; S]).unwrap();
        assert_eq!(disk.writes_while_syncing("f"), 1);
        let waiter = {
            let disk = disk.clone();
            std::thread::spawn(move || {
                let ended = f.sync_end(true).unwrap().is_ok();
                (ended, disk.stats().syncs)
            })
        };
        while disk.held_waiters() == 0 {
            std::thread::yield_now();
        }
        disk.release_syncs();
        assert_eq!(waiter.join().unwrap(), (true, 1));
        assert_eq!(disk.held_waiters(), 0);
    }
}

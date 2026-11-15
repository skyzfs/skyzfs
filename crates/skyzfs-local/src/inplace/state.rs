//! The cut state: what the store records about cuts so that a held cut
//! reads as the volume was at its instant (design §5.2, "What a cut
//! needs from the store", and D7; work package M2.2).
//!
//! # What it records
//!
//! - **The count of cuts and the epochs.** The changes after cut *n*−1's
//!   instant and up to cut *n*'s are epoch *n*, and the open epoch is
//!   the count plus one.
//! - **Maps of changed blocks**, one bit a block ([`BLOCK_SIZE`]): one
//!   for each held cut and one open map, [`MAPS`] in all. A write or a
//!   write-zeroes sets the block's bit in the open map. A **freeze** at
//!   a cut's instant is one store: the word that holds the count also
//!   names the open map, so the count moves on and a clean map becomes
//!   the open one in the same store, and the map that was open is the
//!   new cut's. A death cannot split it. The cut's own record (its
//!   number, its `lo`, its map, its group and root block pointer and
//!   whether it opens as itself) is written into a free slot before that
//!   store, and is a cut only once the count reaches its number.
//! - **A discard is not noted** (coding's reading, M2.2, put to the
//!   lead with the layout): it changes no content in the store (design
//!   §5.1), so a cut holds the block's content as it was, which the
//!   discard rule allows, and nothing is copied for it. Holes in cuts
//!   come with discard tracking (M4). A noted discard would need either
//!   a copy before it or one more bit a block.
//! - **Each held cut's `lo`**: its map is the changes in the epochs
//!   `(lo, number]`. The `lo` says from which epoch a block's content
//!   in the cut can date, and so which copies the cut reads.
//! - **The copies, and their records.** Before a block that a held cut
//!   reads changes for the first time after the newest cut's instant,
//!   its content is copied, whole, into the copies file, and a record
//!   says for which epochs `[lo, hi)` the copy is the block's content.
//!   The copy's place in the file is the record's index. The copy is
//!   written whole before the record is, and the record before the
//!   block's bit in the open map and before the write that changes the
//!   block is begun: a death at any point leaves a block whose change
//!   has not begun, or one whose record holds what a cut reads. A
//!   record is kept only while a held cut needs it (below).
//! - **The held cuts** (D7), [`HELD`] at most: the one being uploaded,
//!   the one that waits, and the one whose flush is under way. A cut is
//!   held from its instant. When its flush completes it replaces the
//!   one that waits and takes its map, or becomes the one being
//!   uploaded if none is. A cut frozen while another's flush is under
//!   way takes that cut's map through the open map, and the other is no
//!   longer held (coding's reading, M2.2, put to the lead): the cut
//!   that waits is untouched, so whichever flush fails it is still the
//!   newest good one, and with ZFS the cut let go is a stray flush's,
//!   of the same group. Until an uploader exists (M2.4), the oldest held
//!   cut whose flush has completed is the one being uploaded from the
//!   moment it is held (the design names no uploader before M2.4).
//! - **The bound** on the copies' bytes, counted over every held cut:
//!   when a copy would take the copies past it, the oldest held cut is
//!   abandoned, and the next oldest while still at it. An abandoned
//!   cut being uploaded gives its map to the cut that waits; with none
//!   waiting, to the cut whose flush is under way; with neither, to the
//!   open map. Writes are never held.
//! - **A copy that cannot be made**: the copies file is not allocated
//!   up to the bound; it grows a slot at a time, and reuses the slots
//!   of records it frees, so a full file system can refuse a copy, as
//!   can a failing write of the file or read of the image. The cut that
//!   reads the block's content then goes, with every newer one, which
//!   would read it once it took that cut's map; older cuts stay, and
//!   the client's write goes on. A recovery point is worth less than
//!   the volume (the owner's ruling, through the lead, M2.2b).
//! - **The mark** of an applied uberblock write that no cut has taken
//!   yet, and the newest cut's group and root block pointer: the repeat
//!   rule compares with them (`cuts.rs`). Beside the newest cut's state,
//!   in the word that chooses its slot, is whether that cut opens as
//!   itself, which the status shows after the cut is let go.
//!
//! # Taking a map
//!
//! A cut that takes another's map names it as a second part of its
//! own, with the lower `lo` of the two, in one store of its map word,
//! before the other is let go; a block is in its map if it is in either
//! part. (The open map's `lo` is a word of its own, which counts only
//! when no cut is held, and is stored first.) Then the donor goes; then
//! the blocks of the smaller part are moved into the larger, each set
//! in the larger before it is cleared in the smaller; then a store
//! names the larger part alone. A death anywhere in that leaves every
//! held cut's map the union, as a reader beside the daemon reads it at
//! any store, and the next instance finishes the move. The maps are
//! behind `state/maps.rs`, which says what the protocol asks of them
//! and lays them out: a move and a clearing visit only the words that
//! are set, so a pass costs the words set in the smaller map, not the
//! volume. (How the maps would take memory in proportion to a cache
//! rather than to the volume, for M3, and what that changes, is there
//! too.) A cut
//! let go gives its map to a newer one or to the open map, unless it is
//! the oldest: the maps of the newer cuts and the open map are what
//! tells an older cut that a block changed after its instant.
//!
//! A map let go is cleared before it is used again, and a start clears
//! every map that nothing names. That rests on one rule: a map holds
//! bits that count only while the open map or a held cut names it, as
//! its first part or the part it is taking. A cut is held from the store
//! of the count that reaches its number; before it, the record a
//! freeze writes names the open map, which the open map still names. A
//! donor's map is named by its heir before the donor is let go. The map
//! that becomes the open one at a freeze is one nothing named, cleared
//! when it was let go, and named in the store of the count. So the maps
//! nothing names are those of the oldest cut once uploaded, and the
//! smaller part of a finished move, which the move left clean.
//!
//! # Which records a held cut needs
//!
//! A held cut reads block *b* when *b* is in its map, or when the whole
//! volume counts as changed in its epochs (below). It reads the home
//! image if *b* did not change after its instant (in the open map or a
//! newer held cut's map), and otherwise the record whose epochs hold its
//! number. A record no held cut reads is freed with its copy. A copy
//! that the cut which waits needs is never freed with an abandoned cut:
//! a block in the abandoned cut's map that did not change between the
//! two instants reads the same in both (design §5.2).
//!
//! # The whole volume
//!
//! A state that begins on a volume, or that is not trusted, counts every
//! block as changed in the open epoch ([`CutState::begin`]): no image is
//! uploaded yet, or the state cannot say what changed. Its next cut's
//! map is then the whole volume, until M2.3 narrows that after a restart
//! of the machine.
//!
//! # The region, and its size
//!
//! The state lives in a [`Region`] of 64-bit words, which a death of the
//! daemon leaves as the last store left it: a file mapped into the
//! daemon is such a region, since the kernel keeps its pages when the
//! process dies (`skyzfs-mapped`, [`StoreRegion`]). Its header names
//! its owner ([`Owner`]): the volume's identity, the machine's boot,
//! and the start token of the open under which it was kept
//! ([`crate::volume::START_NAME`]), without which a start does not take
//! it up. Every change is ordered
//! so that a death after any store leaves a state that the next
//! instance can take up ([`CutState::view`], [`CutState::take_up`]), finishing what a death
//! interrupted: a cut whose flush was under way takes the start's sync
//! as its flush, a map being taken is moved whole, and a map no cut
//! owns is cleared. What is
//! kept only in memory (where each block's records are, which records
//! are free) is made again from the region at each start.
//!
//! The region is locked in memory (design §5.2), so its size is a
//! figure of the design ([`words_for`]): [`MAPS`] bits a block and,
//! in the flat layout of the maps, a summary bit for every 64 of them,
//! 130 MiB for a TiB at 4 KiB blocks; and 16 bytes for each block of
//! the bound on the copies.

// Reading and letting go of held cuts is for the uploader (M2.4) and,
// until then, the test-hooks build's commands (M2.2b); a test region's
// deaths are for the tests.
#![cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex, MutexGuard};

use skyzfs_blk::BLOCK_SIZE;
use skyzfs_uberblock::{State, ROOT_WORDS};

use super::Mutants;
use maps::Maps;

/// Where the state lives: words that a death leaves as the last store
/// left them.
pub trait Region: Send {
    /// Its length, in words.
    fn words(&self) -> usize;
    /// The word at `at`.
    fn load(&self, at: usize) -> u64;
    /// Store `value` at `at`. The stores of one thread are seen by a
    /// later instance in the order they were made.
    fn store(&mut self, at: usize, value: u64);
}

/// What a death at a store into a [`MemRegion`] panics with: a test
/// catches it as the daemon's death.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lost;

/// A region in memory, shared by every instance that a test starts on
/// it: what the kernel's pages of a mapped file are to a process that
/// dies. A test can make a store the last one ([`MemRegion::die_at`]).
#[derive(Debug, Clone, Default)]
pub struct MemRegion(Arc<Mutex<Mem>>);

#[derive(Debug, Default)]
struct Mem {
    words: Vec<u64>,
    stores: u64,
    /// The stores left before the death, if one is to come.
    left: Option<u64>,
}

impl MemRegion {
    /// A region of `words` zeros.
    pub fn new(words: usize) -> Self {
        Self(Arc::new(Mutex::new(Mem {
            words: vec![0; words],
            ..Mem::default()
        })))
    }

    fn mem(&self) -> MutexGuard<'_, Mem> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Die at the `n`th store from now, counting from 0: that store and
    /// every one after it are not made, and the first panics with
    /// `Lost`.
    pub fn die_at(&self, n: u64) {
        self.mem().left = Some(n);
    }

    /// No death to come: the next instance stores again.
    pub fn revive(&self) {
        self.mem().left = None;
    }

    /// The stores made so far.
    pub fn stores(&self) -> u64 {
        self.mem().stores
    }
}

impl Region for MemRegion {
    fn words(&self) -> usize {
        self.mem().words.len()
    }

    fn load(&self, at: usize) -> u64 {
        self.mem().words.get(at).copied().unwrap_or(0)
    }

    fn store(&mut self, at: usize, value: u64) {
        let mut mem = self.mem();
        if let Some(left) = mem.left {
            if left == 0 {
                drop(mem);
                std::panic::panic_any(Lost);
            }
            mem.left = Some(left - 1);
        }
        if let Some(word) = mem.words.get_mut(at) {
            *word = value;
        }
        mem.stores += 1;
    }
}

/// The region a store's cut state lives in ([`super::CutRegion`]).
#[derive(Debug)]
pub enum StoreRegion {
    /// A region in memory (the tests').
    Memory(MemRegion),
    /// A region the kernel keeps while the machine is up, mapped and
    /// locked.
    Mapped(skyzfs_mapped::Mapped),
}

impl Region for StoreRegion {
    fn words(&self) -> usize {
        match self {
            StoreRegion::Memory(region) => region.words(),
            StoreRegion::Mapped(region) => region.words(),
        }
    }

    fn load(&self, at: usize) -> u64 {
        match self {
            StoreRegion::Memory(region) => region.load(at),
            StoreRegion::Mapped(region) => region.load(at),
        }
    }

    fn store(&mut self, at: usize, value: u64) {
        match self {
            StoreRegion::Memory(region) => region.store(at, value),
            StoreRegion::Mapped(region) => region.store(at, value),
        }
    }
}

/// A store's cut state's region as its open had it, with what its
/// start needs to take the state up or begin one.
#[derive(Debug)]
pub(crate) struct Taken {
    pub(crate) region: StoreRegion,
    /// Whether the region was there with the length the volume needs:
    /// only then can it hold a state to take up.
    pub(crate) kept: bool,
    /// Whose a state must be to be taken up: with the start token this
    /// open found.
    pub(crate) owner: Owner,
    /// The start token this open wrote, under which the state is kept
    /// from this start on.
    pub(crate) start: [u8; 16],
    /// The bound on the copies, in bytes.
    pub(crate) bound: u64,
}

impl Taken {
    /// Have the region `hold` names for a volume of `size` bytes: map a
    /// named one, allocated in full and locked, or take the memory
    /// given. An error says why the region cannot be had.
    ///
    /// `found` is the start token this open found, none where there was
    /// none to read whole, and `start` the one it writes.
    pub(crate) fn open(
        hold: &super::HoldCuts,
        size: u64,
        identity: [u8; 16],
        found: Option<[u8; 16]>,
        start: [u8; 16],
    ) -> Result<Self, String> {
        let bound = hold.bound.unwrap_or_else(|| default_bound(size));
        let words = words_for(size / BLOCK_SIZE, records_for(bound));
        let (region, kept) = match &hold.region {
            super::CutRegion::Memory(region) if region.words() >= words => {
                (StoreRegion::Memory(region.clone()), true)
            }
            super::CutRegion::Memory(_) => (StoreRegion::Memory(MemRegion::new(words)), false),
            super::CutRegion::Named(path) => {
                let (region, opened) =
                    skyzfs_mapped::Mapped::open(path, words).map_err(|e| e.to_string())?;
                (
                    StoreRegion::Mapped(region),
                    opened == skyzfs_mapped::Opened::Kept,
                )
            }
        };
        Ok(Self {
            region,
            kept,
            owner: Owner {
                volume: identity,
                boot: hold.boot,
                // No token found matches no state: a begun state's
                // token is random.
                start: found.unwrap_or_default(),
            },
            start,
            bound,
        })
    }
}

/// The region's magic: `SKYZCUT1`.
const MAGIC: u64 = u64::from_le_bytes(*b"SKYZCUT1");
/// The version of the region's format this code reads and writes.
pub const VERSION: u64 = 1;

/// The words of a pool state: its group, then its root block pointer.
const STATE_WORDS: usize = 1 + ROOT_WORDS;
/// Held cuts at most: the one being uploaded, the one that waits, and
/// the one whose flush is under way (D7).
pub const HELD: usize = 3;
/// Maps: one for each held cut, and the open map.
pub const MAPS: usize = HELD + 1;
/// No map, in a word that names maps.
const NO_MAP: u64 = 0xff;
/// Where the open map's number lies in the word of the count.
const OPEN_SHIFT: u32 = 56;
/// A held cut's words: its number; its map word (the first part in the
/// low byte, a second part being taken in the next, and its `lo` above
/// them, so that a taking is one store); its phase; whether it opens as
/// itself; then its state.
const CUT_WORDS: usize = 4 + STATE_WORDS;
/// Where a cut's `lo` lies in its map word.
const LO_SHIFT: u32 = 16;
/// The bit of the newest state's selector that says the newest cut
/// opens as itself. The status reads it after that cut is let go, when
/// no held cut has the flag any more.
const NEWEST_ITSELF: u64 = 1 << 8;
/// The bits of a selector that choose a slot.
const SEL_SLOT: u64 = 0xff;
/// Where the entry of a cut being abandoned lies in the word of the
/// abandoned cuts.
const ABANDONING_SHIFT: u32 = 56;
/// A record's words: the block plus one (0: free), then its epochs
/// (`lo` in the low half, `hi` in the high). Its copy lies in the slot
/// of the copies file with the record's index.
const RECORD_WORDS: usize = 2;

/// Where each field lies, in words.
mod at {
    use super::{CUT_WORDS, HELD, STATE_WORDS};

    pub(super) const MAGIC: usize = 0;
    pub(super) const VERSION: usize = 1;
    pub(super) const BLOCKS: usize = 2;
    pub(super) const RECORDS: usize = 3;
    /// The boot the state was begun in: two words, the boot id's bytes
    /// in order.
    pub(super) const BOOT: usize = 4;
    /// The volume's identity, two words likewise.
    pub(super) const IDENTITY: usize = 6;
    /// The start token under which the state is kept, two words
    /// likewise.
    pub(super) const START: usize = 8;
    /// The count of cuts, and in the top byte the open map: one store
    /// moves both.
    pub(super) const COUNT: usize = 10;
    /// The `lo` of the next cut when no cut is held.
    pub(super) const OPEN_LO: usize = 11;
    /// The epoch in which the whole volume counts as changed; 0: none.
    pub(super) const WHOLE: usize = 12;
    /// The cuts abandoned at the bound or let go at a copy that could
    /// not be made, and in the top byte the entry plus one of a cut
    /// being abandoned (0: none): one store counts
    /// that cut and ends its abandoning.
    pub(super) const ABANDONED: usize = 13;
    /// The second part of the open map while it takes a cut's map.
    pub(super) const OPEN_ALSO: usize = 14;
    /// Which of the two mark slots holds the mark: 0 none, 1 or 2.
    pub(super) const MARK_SEL: usize = 15;
    pub(super) const MARK: usize = 16;
    /// Which of the two slots holds the newest cut's state: 0 none;
    /// and, at `NEWEST_ITSELF`, whether that cut opens as itself, so
    /// that one store chooses both.
    pub(super) const NEWEST_SEL: usize = MARK + 2 * STATE_WORDS;
    pub(super) const NEWEST: usize = NEWEST_SEL + 1;
    pub(super) const CUTS: usize = NEWEST + 2 * STATE_WORDS;
    pub(super) const HEADER: usize = CUTS + HELD * CUT_WORDS;
}

/// The words a state needs for a volume of `blocks` blocks with room
/// for `records` records: what is locked.
pub fn words_for(blocks: u64, records: usize) -> usize {
    at::HEADER + Maps::words(blocks) + records * RECORD_WORDS
}

/// How many records a state keeps room for, with a bound of `bound`
/// bytes: one for each block of copies the bound allows.
pub fn records_for(bound: u64) -> usize {
    ((bound / BLOCK_SIZE) as usize).max(1)
}

/// The provisional default of the bound on the copies: 1/256 of the
/// volume, at least 256 MiB, so that its records, which are locked,
/// take 1/65536 of the volume (put to the lead with the layout, M2.2;
/// set from M2.T4's figures).
pub fn default_bound(size: u64) -> u64 {
    (size / 256).max(256 << 20)
}

/// Where a held cut stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Its flush has not completed yet.
    Flushing,
    /// Its flush completed; it waits for the one being uploaded.
    Waiting,
    /// It is the one being uploaded.
    Uploading,
}

impl Phase {
    fn code(self) -> u64 {
        match self {
            Phase::Flushing => 0,
            Phase::Waiting => 1,
            Phase::Uploading => 2,
        }
    }

    fn of(code: u64) -> Phase {
        match code {
            1 => Phase::Waiting,
            2 => Phase::Uploading,
            _ => Phase::Flushing,
        }
    }
}

/// A held cut, as the state records it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeldCut {
    /// Its number in the count of cuts.
    pub number: u64,
    /// Its map is the changes in the epochs after `lo` up to `number`.
    pub lo: u64,
    /// Where it stands.
    pub phase: Phase,
    /// The pool state of the uberblock that marked it.
    pub state: State,
    /// Whether it opens as itself (`cuts.rs`).
    pub itself: bool,
}

/// What a held cut reads for a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Read {
    /// The block is not in the cut's map: the image that the cut is
    /// uploaded onto holds it.
    Unchanged,
    /// The block's content is the home image's, now.
    Image,
    /// The block's content is the copy in this slot of the copies file.
    Copy(u32),
}

/// The I/O the state asks for when it keeps a block's content: read the
/// block from the home image, and write a copy.
pub trait Copier {
    /// Read block `block` of the home image into `buf`, one block.
    fn read_image(&mut self, block: u64, buf: &mut [u8]) -> io::Result<()>;
    /// Write `buf`, one block, into slot `slot` of the copies file.
    /// Whole before it returns: a later instance reads it. The store
    /// writes it by direct I/O, so that a failure of the write is its
    /// error and not a later writeback's (M2.2c).
    fn write_copy(&mut self, slot: u32, buf: &[u8]) -> io::Result<()>;
}

/// The figures for the status (M2.2b).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Figures {
    /// The cuts held now.
    pub held: u64,
    /// The bytes the copies take now.
    pub copied: u64,
    /// The bound on them.
    pub bound: u64,
    /// The cuts abandoned at the bound or let go at a copy that could
    /// not be made.
    pub abandoned: u64,
}

/// A record, as read from the region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Record {
    block: u64,
    lo: u64,
    hi: u64,
}

/// A map: its part, and a second part it is taking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Parts {
    first: usize,
    also: Option<usize>,
}

impl Parts {
    fn of(word: u64) -> Parts {
        let also = (word >> 8) & 0xff;
        Parts {
            first: (word & 0xff) as usize,
            also: (also != NO_MAP).then_some(also as usize),
        }
    }

    fn word(self) -> u64 {
        self.first as u64 | (self.also.map_or(NO_MAP, |also| also as u64) << 8)
    }

    /// A held cut's map word: these parts and `lo`.
    fn with_lo(self, lo: u64) -> u64 {
        self.word() | (lo << LO_SHIFT)
    }
}

/// A held cut with where it lies and its map.
#[derive(Debug, Clone, Copy)]
struct Held {
    cut: HeldCut,
    entry: usize,
    parts: Parts,
}

/// Who takes a map that a cut lets go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Heir {
    /// The held cut with this number.
    Cut(u64),
    /// The open map: the next cut.
    Open,
}

/// The cut state: see the [module documentation](self).
pub struct CutState<R: Region> {
    region: R,
    blocks: u64,
    /// The maps of changed blocks (`state/maps.rs`).
    maps: Maps,
    /// Where the records begin.
    records_base: usize,
    capacity: usize,
    bound: u64,
    mutants: Mutants,
    /// Where each block's records are, by block.
    by_block: BTreeMap<u64, Vec<usize>>,
    free_records: Vec<usize>,
    /// Under [`Mutants::COPY_UNRECORDED`]: records kept in memory only,
    /// by word.
    volatile: BTreeMap<usize, u64>,
}

impl<R: Region> std::fmt::Debug for CutState<R> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CutState")
            .field("blocks", &self.blocks)
            .field("count", &self.count())
            .field("held", &self.held())
            .finish_non_exhaustive()
    }
}

/// Why a region holds no state that can be taken up.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RegionError {
    #[error("the region holds no cut state (no magic)")]
    Empty,
    #[error("the cut state is of version {0}, and this daemon reads version {VERSION}")]
    Version(u64),
    #[error("the cut state is of another volume: {0}")]
    Shape(String),
    /// The state was begun in another boot of the machine: a region
    /// kept on a file system that outlives a boot (the daemon's is on
    /// tmpfs, which does not), whose copies a crash may have lost.
    #[error("the cut state was begun in another boot")]
    Boot,
    /// The state is of a volume with another identity: one made again
    /// in the same directory, whose run directory is the same.
    #[error("the cut state is of another volume (its identity differs)")]
    Identity,
    /// The volume was opened again after the instance that kept the
    /// state: by a start that held no cuts, by one with another run
    /// directory, or by one lost before it took the state up or began
    /// a new one. What that open served, or what it heard of the
    /// copies, the state does not know.
    #[error("the volume was opened after the instance that kept the cut state")]
    Start,
}

/// Whose a state is: the volume's identity, the machine's boot, and
/// the start token of the open under which it is kept. A state is taken
/// up only by its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Owner {
    /// The volume's identity ([`crate::volume::Volume::identity`]).
    pub volume: [u8; 16],
    /// The boot id of the machine.
    pub boot: [u8; 16],
    /// The start token ([`crate::volume::START_NAME`]): the one this
    /// open found, when a state is checked; the one it wrote, when a
    /// state is begun or taken up.
    pub start: [u8; 16],
}

/// Sixteen bytes as the two words of the region's header.
fn id_words(id: [u8; 16]) -> [u64; 2] {
    let mut words = [0; 2];
    let (eights, _) = id.as_chunks::<8>();
    for (word, eight) in words.iter_mut().zip(eights) {
        *word = u64::from_le_bytes(*eight);
    }
    words
}

impl<R: Region> CutState<R> {
    /// Begin a state in `region` for a volume of `size` bytes, with a
    /// bound of `bound` bytes on the copies, owned by `owner`: no cut,
    /// and the whole volume counted as changed in the first epoch.
    pub fn begin(mut region: R, size: u64, bound: u64, owner: Owner, mutants: Mutants) -> Self {
        let blocks = size / BLOCK_SIZE;
        let capacity = records_for(bound);
        for i in 0..region.words() {
            if region.load(i) != 0 {
                region.store(i, 0);
            }
        }
        region.store(at::VERSION, VERSION);
        region.store(at::BLOCKS, blocks);
        region.store(at::RECORDS, capacity as u64);
        for (i, word) in id_words(owner.boot).into_iter().enumerate() {
            region.store(at::BOOT + i, word);
        }
        for (i, word) in id_words(owner.volume).into_iter().enumerate() {
            region.store(at::IDENTITY + i, word);
        }
        for (i, word) in id_words(owner.start).into_iter().enumerate() {
            region.store(at::START + i, word);
        }
        region.store(at::WHOLE, 1);
        region.store(at::OPEN_ALSO, NO_MAP);
        region.store(at::MAGIC, MAGIC);
        Self::new(region, blocks, capacity, bound, mutants)
    }

    /// The state that `region` holds, read as it is, with nothing
    /// finished: what a reader beside the daemon sees at any store. The
    /// tests read it so at the instant of a death. A state that is not
    /// `owner`'s is not read. Where none is read, the region comes back
    /// with the reason.
    pub(crate) fn view(
        region: R,
        size: u64,
        bound: u64,
        owner: Owner,
        mutants: Mutants,
    ) -> Result<Self, (RegionError, R)> {
        match Self::check(&region, size, owner, mutants) {
            Ok((blocks, capacity)) => {
                let mut state = Self::new(region, blocks, capacity, bound, mutants);
                state.index();
                Ok(state)
            }
            Err(e) => Err((e, region)),
        }
    }

    /// Whether `region` holds a state this daemon reads, for a volume of
    /// `size` bytes, owned by `owner`: its blocks and its records.
    fn check(
        region: &R,
        size: u64,
        owner: Owner,
        mutants: Mutants,
    ) -> Result<(u64, usize), RegionError> {
        if region.load(at::MAGIC) != MAGIC {
            return Err(RegionError::Empty);
        }
        let version = region.load(at::VERSION);
        if version != VERSION {
            return Err(RegionError::Version(version));
        }
        let blocks = size / BLOCK_SIZE;
        if region.load(at::BLOCKS) != blocks {
            return Err(RegionError::Shape(format!(
                "{} blocks, and the volume has {blocks}",
                region.load(at::BLOCKS)
            )));
        }
        let capacity = region.load(at::RECORDS) as usize;
        if region.words() < words_for(blocks, capacity) {
            return Err(RegionError::Shape(format!(
                "{} words, too few for {blocks} blocks and {capacity} records",
                region.words()
            )));
        }
        let words = |at: usize| [region.load(at), region.load(at + 1)];
        if words(at::IDENTITY) != id_words(owner.volume)
            && !mutants.has(Mutants::IDENTITY_UNCHECKED)
        {
            return Err(RegionError::Identity);
        }
        if words(at::BOOT) != id_words(owner.boot) && !mutants.has(Mutants::BOOT_UNCHECKED) {
            return Err(RegionError::Boot);
        }
        if words(at::START) != id_words(owner.start) && !mutants.has(Mutants::START_UNCHECKED) {
            return Err(RegionError::Start);
        }
        Ok((blocks, capacity))
    }

    /// The region, given back.
    pub(crate) fn into_region(self) -> R {
        self.region
    }

    /// Take up the state that `region` holds for a volume of `size`
    /// bytes, owned by `owner`, as a start after a lost daemon does,
    /// once the start's sync has succeeded: finish what a death
    /// interrupted. The tests' start: it keeps the token it found,
    /// where the store's start reads with [`CutState::view`] and takes
    /// up with [`CutState::take_up`] under the token its open wrote.
    #[cfg(test)]
    pub fn restore(
        region: R,
        size: u64,
        bound: u64,
        owner: Owner,
        mutants: Mutants,
    ) -> Result<Self, (RegionError, R)> {
        let state = Self::view(region, size, bound, owner, mutants)?;
        Ok(state.take_up(owner.start))
    }

    /// Take up this state, read with [`CutState::view`] and trusted, as
    /// a start does once it has written `start` as the volume's start
    /// token: the state is kept under that token from its first store,
    /// and then what a death interrupted is finished. A start lost
    /// while it finishes leaves a state the next start takes up and
    /// finishes; one lost before leaves the token it found, which the
    /// next start no longer finds.
    pub(crate) fn take_up(mut self, start: [u8; 16]) -> Self {
        for (i, word) in id_words(start).into_iter().enumerate() {
            self.store(at::START + i, word);
        }
        self.settle();
        self
    }

    fn new(region: R, blocks: u64, capacity: usize, bound: u64, mutants: Mutants) -> Self {
        Self {
            region,
            blocks,
            maps: Maps::new(at::HEADER, blocks, mutants),
            records_base: at::HEADER + Maps::words(blocks),
            capacity,
            bound,
            mutants,
            by_block: BTreeMap::new(),
            free_records: (0..capacity).rev().collect(),
            volatile: BTreeMap::new(),
        }
    }

    // ---- words ----

    fn load(&self, at: usize) -> u64 {
        match self.volatile.get(&at) {
            Some(value) => *value,
            None => self.region.load(at),
        }
    }

    fn store(&mut self, at: usize, value: u64) {
        self.volatile.remove(&at);
        self.region.store(at, value);
    }

    /// A store of a record's word: in memory only under the defect.
    fn store_record(&mut self, at: usize, value: u64) {
        if self.mutants.has(Mutants::COPY_UNRECORDED) {
            self.volatile.insert(at, value);
        } else {
            self.store(at, value);
        }
    }

    // ---- the count, the open map and the epochs ----

    /// The count of cuts.
    pub fn count(&self) -> u64 {
        self.load(at::COUNT) & ((1 << OPEN_SHIFT) - 1)
    }

    /// The open epoch.
    fn epoch(&self) -> u64 {
        self.count() + 1
    }

    fn whole(&self) -> u64 {
        self.load(at::WHOLE)
    }

    /// Whether the whole volume counts as changed in `cut`'s epochs.
    fn is_whole(&self, cut: &HeldCut) -> bool {
        let whole = self.whole();
        whole > cut.lo && whole <= cut.number
    }

    fn open(&self) -> Parts {
        let also = self.load(at::OPEN_ALSO);
        Parts {
            first: (self.load(at::COUNT) >> OPEN_SHIFT) as usize,
            also: (also != NO_MAP).then_some(also as usize),
        }
    }

    // ---- states: the mark and the newest cut ----

    fn state_at(&self, base: usize) -> State {
        let mut root = [0u64; ROOT_WORDS];
        for (i, word) in root.iter_mut().enumerate() {
            *word = self.load(base + 1 + i);
        }
        State {
            txg: self.load(base),
            root,
        }
    }

    fn put_state(&mut self, base: usize, state: &State) {
        self.store(base, state.txg);
        for (i, word) in state.root.iter().enumerate() {
            self.store(base + 1 + i, *word);
        }
    }

    /// A state held in one of two slots, chosen by `sel`: written into
    /// the other slot, then chosen by one store.
    /// `flags` go into the selector beside the slot's number.
    fn put_double(&mut self, sel: usize, slots: usize, state: Option<&State>, flags: u64) {
        let Some(state) = state else {
            self.store(sel, 0);
            return;
        };
        let next = if self.load(sel) & SEL_SLOT == 1 { 2 } else { 1 };
        let base = slots + (next as usize - 1) * STATE_WORDS;
        self.put_state(base, state);
        self.store(sel, next | flags);
    }

    fn double(&self, sel: usize, slots: usize) -> Option<State> {
        match self.load(sel) & SEL_SLOT {
            n @ (1 | 2) => Some(self.state_at(slots + (n as usize - 1) * STATE_WORDS)),
            _ => None,
        }
    }

    /// The mark: the state of an applied uberblock write that no cut has
    /// taken yet.
    pub fn mark(&self) -> Option<State> {
        self.double(at::MARK_SEL, at::MARK)
    }

    /// Record the mark, before the write's completion goes to the
    /// transport; `None` clears it.
    pub fn set_mark(&mut self, state: Option<&State>) {
        self.put_double(at::MARK_SEL, at::MARK, state, 0);
    }

    /// The newest cut's state, which a repeat may equal.
    pub fn newest(&self) -> Option<State> {
        self.double(at::NEWEST_SEL, at::NEWEST)
    }

    /// The newest cut's state and whether that cut opens as itself,
    /// which stays known after the cut is let go or abandoned.
    pub fn newest_cut(&self) -> Option<(State, bool)> {
        let itself = self.load(at::NEWEST_SEL) & NEWEST_ITSELF != 0;
        self.newest().map(|state| (state, itself))
    }

    fn put_newest(&mut self, state: &State, itself: bool) {
        let flags = if itself { NEWEST_ITSELF } else { 0 };
        self.put_double(at::NEWEST_SEL, at::NEWEST, Some(state), flags);
    }

    // ---- held cuts ----

    fn cut_base(entry: usize) -> usize {
        at::CUTS + entry * CUT_WORDS
    }

    fn held_at(&self, entry: usize) -> Option<Held> {
        let base = Self::cut_base(entry);
        let number = self.load(base);
        if number == 0 || number > self.count() {
            return None;
        }
        let map = self.load(base + 1);
        Some(Held {
            cut: HeldCut {
                number,
                lo: map >> LO_SHIFT,
                phase: Phase::of(self.load(base + 2)),
                itself: self.load(base + 3) == 1,
                state: self.state_at(base + 4),
            },
            entry,
            parts: Parts::of(map & ((1 << LO_SHIFT) - 1)),
        })
    }

    /// The held cuts with where they lie, oldest first.
    fn holds(&self) -> Vec<Held> {
        let mut held: Vec<Held> = (0..HELD).filter_map(|e| self.held_at(e)).collect();
        held.sort_by_key(|h| h.cut.number);
        held
    }

    fn hold(&self, number: u64) -> Option<Held> {
        self.holds().into_iter().find(|h| h.cut.number == number)
    }

    /// The held cuts, oldest first.
    pub fn held(&self) -> Vec<HeldCut> {
        self.holds().into_iter().map(|h| h.cut).collect()
    }

    fn set_phase(&mut self, number: u64, phase: Phase) {
        if let Some(held) = self.hold(number) {
            self.store(Self::cut_base(held.entry) + 2, phase.code());
        }
    }

    fn drop_cut(&mut self, number: u64) {
        if let Some(held) = self.hold(number) {
            self.store(Self::cut_base(held.entry), 0);
        }
    }

    /// The `lo` of a cut frozen now: the newest held cut's number, or
    /// the open map's own `lo` when none is held.
    fn next_lo(&self) -> u64 {
        match self.held().last() {
            Some(cut) => cut.number,
            None => self.load(at::OPEN_LO),
        }
    }

    // ---- maps ----

    fn in_parts(&self, parts: Parts, block: u64) -> bool {
        self.maps.contains(&self.region, parts.first, block)
            || parts
                .also
                .is_some_and(|also| self.maps.contains(&self.region, also, block))
    }

    /// Which maps the open map and the held cuts name.
    fn owned(&self) -> [bool; MAPS] {
        let mut owned = [false; MAPS];
        let parts = std::iter::once(self.open()).chain(self.holds().into_iter().map(|h| h.parts));
        for parts in parts {
            for map in std::iter::once(parts.first).chain(parts.also) {
                if let Some(slot) = owned.get_mut(map) {
                    *slot = true;
                }
            }
        }
        owned
    }

    /// A map that nothing names: clean, since a map is cleared when it
    /// is let go.
    fn free_map(&self) -> Option<usize> {
        let owned = self.owned();
        (0..MAPS).find(|&map| !owned[map])
    }

    /// Clear map `map` if nothing names it any more.
    fn let_go(&mut self, map: usize) {
        if map < MAPS && !self.mutants.has(Mutants::MAP_NOT_CLEARED) && !self.owned()[map] {
            self.maps.clear(&mut self.region, map);
        }
    }

    fn parts_of(&self, heir: Heir) -> Option<Parts> {
        match heir {
            Heir::Open => Some(self.open()),
            Heir::Cut(number) => self.hold(number).map(|h| h.parts),
        }
    }

    /// Name the maps of `heir`: one store for a cut. The open map's
    /// first part lies in the word of the count, so changing both of its
    /// parts is two stores, which the callers order so that each leaves
    /// the union whole.
    fn set_parts(&mut self, heir: Heir, parts: Parts) {
        match heir {
            Heir::Cut(number) => {
                if let Some(held) = self.hold(number) {
                    self.store(Self::cut_base(held.entry) + 1, parts.with_lo(held.cut.lo));
                }
            }
            Heir::Open => {
                let open = self.open();
                if open.first != parts.first {
                    let count = self.count();
                    self.store(at::COUNT, count | ((parts.first as u64) << OPEN_SHIFT));
                }
                if open.also != parts.also {
                    let also = parts.also.map_or(NO_MAP, |also| also as u64);
                    self.store(at::OPEN_ALSO, also);
                }
            }
        }
    }

    /// Finish a taking: move the smaller part of `heir`'s map into the
    /// larger, then name the larger alone.
    fn finish(&mut self, heir: Heir) {
        let Some(parts) = self.parts_of(heir) else {
            return;
        };
        let Some(also) = parts.also else {
            return;
        };
        let large = if also == parts.first {
            also
        } else {
            let (small, large) = if self.maps.weight(&self.region, also)
                <= self.maps.weight(&self.region, parts.first)
            {
                (also, parts.first)
            } else {
                (parts.first, also)
            };
            self.maps.move_into(&mut self.region, small, large);
            large
        };
        if heir == Heir::Open && large != parts.first {
            // Both parts are the larger map first, then the second goes.
            self.set_parts(
                heir,
                Parts {
                    first: large,
                    also: Some(large),
                },
            );
        }
        self.set_parts(
            heir,
            Parts {
                first: large,
                also: None,
            },
        );
    }

    /// `donor`, a held cut, gives its map to `heir` and is let go. The
    /// taking is one store: a cut names the donor's map as a second part
    /// of its own, with the lower `lo`, in its map word (the open map
    /// takes its `lo` first, which counts only once no cut is held).
    /// Then the donor goes, then the move.
    fn give(&mut self, donor: Held, heir: Heir) {
        self.finish(Heir::Cut(donor.cut.number));
        let Some(donor) = self.hold(donor.cut.number) else {
            return;
        };
        let taken = self
            .parts_of(heir)
            .is_some_and(|parts| parts.also == Some(donor.parts.first));
        if !taken {
            self.finish(heir);
            match heir {
                Heir::Cut(number) => {
                    if let Some(held) = self.hold(number) {
                        let parts = Parts {
                            also: Some(donor.parts.first),
                            ..held.parts
                        };
                        self.store(
                            Self::cut_base(held.entry) + 1,
                            parts.with_lo(held.cut.lo.min(donor.cut.lo)),
                        );
                    }
                }
                Heir::Open => {
                    if self.held().len() == 1 {
                        self.store(at::OPEN_LO, donor.cut.lo);
                    }
                    self.store(at::OPEN_ALSO, donor.parts.first as u64);
                }
            }
        }
        self.drop_cut(donor.cut.number);
        self.finish(heir);
    }

    /// `donor`, a held cut, is let go and its map with it (a defect).
    fn drop_map(&mut self, donor: Held) {
        self.drop_cut(donor.cut.number);
        self.let_go(donor.parts.first);
    }

    // ---- what happens to cuts ----

    /// Freeze the open map as a new cut at its instant, marked by
    /// `state`: its number. The cut is held, its flush under way.
    pub fn freeze(&mut self, state: &State, itself: bool) -> u64 {
        // A cut whose flush is under way gives its map to the open map
        // first, and is no longer held (see the module documentation).
        if let Some(flushing) = self
            .holds()
            .into_iter()
            .find(|h| h.cut.phase == Phase::Flushing)
        {
            self.give(flushing, Heir::Open);
            self.free_unneeded(None);
        }
        let number = self.count() + 1;
        let lo = self.next_lo();
        let open = self.open().first;
        let (Some(entry), Some(clean)) = (
            (0..HELD).find(|&e| self.held_at(e).is_none()),
            self.free_map(),
        ) else {
            // Not reached: at most two cuts are held here, which leaves
            // a slot and a map.
            return self.count();
        };
        let base = Self::cut_base(entry);
        let next = number | ((clean as u64) << OPEN_SHIFT);
        if self.mutants.has(Mutants::FREEZE_SPLIT) {
            // The defect: the count moves on before the cut is recorded.
            self.store(at::COUNT, next);
        }
        self.store(base, number);
        self.store(
            base + 1,
            Parts {
                first: open,
                also: None,
            }
            .with_lo(lo),
        );
        self.store(base + 2, Phase::Flushing.code());
        self.store(base + 3, u64::from(itself));
        self.put_state(base + 4, state);
        if !self.mutants.has(Mutants::FREEZE_SPLIT) {
            self.store(at::COUNT, next);
        }
        self.put_newest(state, itself);
        if self.mark().as_ref() == Some(state) {
            self.set_mark(None);
        }
        if self.mutants.has(Mutants::REPLACED_AT_INSTANT) {
            // The defect: the cut that waits is replaced at the newer
            // cut's instant, not when its flush completes.
            if let Some(waiting) = self
                .holds()
                .into_iter()
                .find(|h| h.cut.phase == Phase::Waiting)
            {
                self.give(waiting, Heir::Cut(number));
                self.free_unneeded(Some(waiting.cut.number));
            }
        }
        number
    }

    /// The flush of cut `number` has completed without error: it
    /// replaces the cut that waits and takes its map, or is the one
    /// being uploaded if none is.
    pub fn flushed(&mut self, number: u64) {
        let Some(cut) = self.hold(number) else {
            return;
        };
        if cut.cut.phase != Phase::Flushing {
            return;
        }
        let holds = self.holds();
        let waiting = holds
            .iter()
            .find(|h| h.cut.phase == Phase::Waiting && h.cut.number < number)
            .copied();
        let uploading = holds.iter().any(|h| h.cut.phase == Phase::Uploading);
        match waiting {
            Some(waiting) => {
                if self.mutants.has(Mutants::REPLACED_MAP_DROPPED) {
                    self.drop_map(waiting);
                } else {
                    self.give(waiting, Heir::Cut(number));
                }
                self.set_phase(number, Phase::Waiting);
                self.free_unneeded(
                    self.mutants
                        .has(Mutants::REPLACED_COPIES_FREED)
                        .then_some(waiting.cut.number),
                );
            }
            None if !uploading => self.set_phase(number, Phase::Uploading),
            None => self.set_phase(number, Phase::Waiting),
        }
    }

    /// The flush of cut `number` failed: the volume has failed, and the
    /// cut is never uploaded. Its map goes to the next cut held after
    /// it, or to the open map: an older cut reads by it which blocks
    /// changed after its instant.
    pub fn flush_failed(&mut self, number: u64) {
        let Some(cut) = self.hold(number) else {
            return;
        };
        let heir = self
            .held()
            .into_iter()
            .find(|c| c.number > number)
            .map_or(Heir::Open, |c| Heir::Cut(c.number));
        self.give(cut, heir);
        self.free_unneeded(None);
    }

    /// Cut `number`, the one being uploaded, is uploaded: it is let go,
    /// and the cut that waits is the next to be uploaded.
    pub fn release(&mut self, number: u64) -> bool {
        let holds = self.holds();
        let Some(cut) = holds
            .iter()
            .find(|h| h.cut.number == number && h.cut.phase == Phase::Uploading)
            .copied()
        else {
            return false;
        };
        if holds.len() == 1 {
            self.store(at::OPEN_LO, number);
        }
        self.drop_cut(number);
        self.let_go(cut.parts.first);
        if let Some(waiting) = holds.iter().find(|h| h.cut.phase == Phase::Waiting) {
            self.set_phase(waiting.cut.number, Phase::Uploading);
        }
        self.free_unneeded(None);
        true
    }

    /// The oldest held cut is abandoned at the bound. The one being
    /// uploaded gives its map to the cut that waits; with none waiting,
    /// to the one whose flush is under way; with neither, to the open
    /// map. A cut whose flush is under way, the last held, gives it to
    /// the open map.
    ///
    /// The cut's entry is recorded first, and the count moves with the
    /// store that clears it, after the cut is gone: a start that finds
    /// the entry counts the cut if it is gone, which is exact whatever
    /// store a death came at (the review of #98).
    fn abandon(&mut self) -> bool {
        let holds = self.holds();
        let Some(oldest) = holds.first().copied() else {
            return false;
        };
        let abandoned = self.begin_abandoning(&oldest);
        let waiting = holds
            .iter()
            .find(|h| h.cut.phase == Phase::Waiting)
            .copied();
        let flushing = holds
            .iter()
            .find(|h| h.cut.phase == Phase::Flushing)
            .copied();
        let heir = if oldest.cut.phase == Phase::Uploading {
            match (waiting, flushing) {
                (Some(_), Some(flushing)) if self.mutants.has(Mutants::ABANDONED_TO_FLUSHING) => {
                    Heir::Cut(flushing.cut.number)
                }
                (Some(waiting), _) => Heir::Cut(waiting.cut.number),
                (None, Some(_)) if self.mutants.has(Mutants::ABANDONED_TO_OPEN) => Heir::Open,
                (None, Some(flushing)) => Heir::Cut(flushing.cut.number),
                (None, None) => Heir::Open,
            }
        } else {
            holds
                .get(1)
                .map_or(Heir::Open, |next| Heir::Cut(next.cut.number))
        };
        if self.mutants.has(Mutants::ABANDONED_MAP_DROPPED) {
            if heir == Heir::Open && holds.len() == 1 {
                self.store(at::OPEN_LO, oldest.cut.number);
            }
            self.drop_map(oldest);
        } else {
            self.give(oldest, heir);
        }
        if oldest.cut.phase == Phase::Uploading {
            if let Some(waiting) = waiting {
                self.set_phase(waiting.cut.number, Phase::Uploading);
            }
        }
        self.store(at::ABANDONED, abandoned + 1);
        self.free_unneeded(
            self.mutants
                .has(Mutants::ABANDONED_COPIES_FREED)
                .then_some(oldest.cut.number),
        );
        true
    }

    /// Finish what a death interrupted, at a start whose sync has
    /// succeeded: a cut recorded but not counted is none; a taking is
    /// finished; a cut whose abandoning a death interrupted is counted
    /// if it is gone; every cut whose flush was under way takes that sync as
    /// its flush; the oldest whose flush completed is the one being
    /// uploaded if none is; every map nothing names is cleared; and the
    /// records no held cut needs are freed.
    fn settle(&mut self) {
        let count = self.count();
        for entry in 0..HELD {
            let base = Self::cut_base(entry);
            if self.load(base) > count {
                self.store(base, 0);
            }
        }
        // The newest cut's state and flag, if a death came between the
        // count and their record.
        if let Some(newest) = self.held().last().copied() {
            if newest.number == count && self.newest_cut() != Some((newest.state, newest.itself)) {
                self.put_newest(&newest.state, newest.itself);
            }
        }
        // A taking under way: the heir names the donor's map, and the
        // donor is still held. The donor goes, and the move is finished.
        loop {
            let holds = self.holds();
            let open = self.open();
            let taken = holds.iter().enumerate().find_map(|(i, donor)| {
                if open.also == Some(donor.parts.first) {
                    return Some((*donor, Heir::Open));
                }
                holds[i + 1..]
                    .iter()
                    .find(|heir| heir.parts.also == Some(donor.parts.first))
                    .map(|heir| (*donor, Heir::Cut(heir.cut.number)))
            });
            let Some((donor, heir)) = taken else { break };
            self.give(donor, heir);
        }
        // An abandoning under way, now that every taking is finished:
        // its cut is counted if it is gone; if it is still held, the
        // death came before its heir named its map, and it is not
        // abandoned.
        let entry = (self.load(at::ABANDONED) >> ABANDONING_SHIFT) as usize;
        if entry != 0 {
            let gone = self.held_at(entry - 1).is_none();
            self.store(at::ABANDONED, self.abandoned() + u64::from(gone));
        }
        for held in self.holds() {
            self.finish(Heir::Cut(held.cut.number));
        }
        self.finish(Heir::Open);
        for cut in self.held() {
            if cut.phase == Phase::Flushing {
                self.flushed(cut.number);
            }
        }
        let waiting: Vec<Held> = self
            .holds()
            .into_iter()
            .filter(|h| h.cut.phase == Phase::Waiting)
            .collect();
        if let Some((newest, older)) = waiting.split_last() {
            // A newer cut whose flush completed takes the older's map.
            for older in older {
                self.give(*older, Heir::Cut(newest.cut.number));
            }
        }
        let held = self.held();
        if !held.iter().any(|c| c.phase == Phase::Uploading) {
            if let Some(oldest) = held.iter().find(|c| c.phase == Phase::Waiting) {
                self.set_phase(oldest.number, Phase::Uploading);
            }
        }
        if self.mark() == self.newest() {
            self.set_mark(None);
        }
        for map in 0..MAPS {
            self.let_go(map);
        }
        self.free_unneeded(None);
    }

    // ---- records ----

    fn record_base(&self, index: usize) -> usize {
        self.records_base + index * RECORD_WORDS
    }

    fn record(&self, index: usize) -> Option<Record> {
        let base = self.record_base(index);
        let block = self.load(base);
        if block == 0 {
            return None;
        }
        let epochs = self.load(base + 1);
        Some(Record {
            block: block - 1,
            lo: epochs & 0xffff_ffff,
            hi: epochs >> 32,
        })
    }

    /// Make the in-memory index from the region: where the records are.
    fn index(&mut self) {
        self.by_block.clear();
        self.free_records.clear();
        for index in (0..self.capacity).rev() {
            match self.record(index) {
                Some(record) => self.by_block.entry(record.block).or_default().push(index),
                None => self.free_records.push(index),
            }
        }
    }

    /// Whether a held cut reads `record`: one whose number lies in its
    /// epochs and whose map holds its block.
    fn needed(&self, record: &Record, holds: &[Held]) -> bool {
        holds.iter().any(|h| {
            record.lo <= h.cut.number
                && h.cut.number < record.hi
                && (self.in_parts(h.parts, record.block) || self.is_whole(&h.cut))
        })
    }

    /// Free the records no held cut needs. Under a defect, `all_of`
    /// names a cut just let go whose records are all freed, whatever
    /// other cut needs them.
    fn free_unneeded(&mut self, all_of: Option<u64>) {
        let holds = self.holds();
        let indices: Vec<usize> = self.by_block.values().flatten().copied().collect();
        for index in indices {
            let Some(record) = self.record(index) else {
                continue;
            };
            let of_gone = all_of.is_some_and(|number| record.lo <= number && number < record.hi);
            if of_gone || !self.needed(&record, &holds) {
                self.free_record(index, &record);
            }
        }
    }

    fn free_record(&mut self, index: usize, record: &Record) {
        let base = self.record_base(index);
        self.store_record(base, 0);
        if let Some(list) = self.by_block.get_mut(&record.block) {
            list.retain(|&i| i != index);
            if list.is_empty() {
                self.by_block.remove(&record.block);
            }
        }
        self.free_records.push(index);
    }

    fn records_of(&self, block: u64) -> Vec<(usize, Record)> {
        self.by_block
            .get(&block)
            .into_iter()
            .flatten()
            .filter_map(|&index| self.record(index).map(|record| (index, record)))
            .collect()
    }

    /// The bytes the copies take.
    fn copied(&self) -> u64 {
        (self.capacity - self.free_records.len()) as u64 * BLOCK_SIZE
    }

    /// The bound in force: the one given, within the room for records.
    fn bound(&self) -> u64 {
        self.bound.min(self.capacity as u64 * BLOCK_SIZE)
    }

    /// Record that `held` is being abandoned, before anything of it
    /// changes: the count of abandoned cuts, which the caller stores
    /// plus one once the cut is gone, and which `settle` finishes after
    /// a death.
    fn begin_abandoning(&mut self, held: &Held) -> u64 {
        let abandoned = self.abandoned();
        self.store(
            at::ABANDONED,
            abandoned | ((held.entry as u64 + 1) << ABANDONING_SHIFT),
        );
        abandoned
    }

    /// The cuts abandoned, at the bound or for a copy that could not be
    /// made.
    fn abandoned(&self) -> u64 {
        self.load(at::ABANDONED) & ((1 << ABANDONING_SHIFT) - 1)
    }

    /// The figures for the status.
    pub fn figures(&self) -> Figures {
        Figures {
            held: self.held().len() as u64,
            copied: self.copied(),
            bound: self.bound(),
            abandoned: self.abandoned(),
        }
    }

    // ---- the changes ----

    /// The `lo` of a record of the block's content now, if a held cut
    /// reads that content: the newest held cut whose map holds the block
    /// wrote it after its own `lo`; with none, it dates from before
    /// every held cut, and a held cut reads it only if the whole volume
    /// counts as changed in its epochs.
    fn wanted(&self, holds: &[Held], block: u64) -> Option<u64> {
        if let Some(newest) = holds.iter().rev().find(|h| self.in_parts(h.parts, block)) {
            return Some(newest.cut.lo + 1);
        }
        holds.iter().any(|h| self.is_whole(&h.cut)).then_some(0)
    }

    /// Blocks `first` to `first + count` are about to be written or
    /// zeroed in the open epoch. The content a held cut reads of each is
    /// copied first, whole, through `io`, and recorded; then the block's
    /// bit is set in the open map. The caller begins the change only
    /// after this returns.
    ///
    /// A copy that cannot be made (the copies file's file system is
    /// full, its write fails, or the read of the image fails) lets go
    /// the cuts that need it ([`CutState::abandon_needing`]), and the
    /// change goes on: a recovery point is worth less than the volume
    /// (the owner's ruling, through the lead, M2.2b). This returns an
    /// error only with the defect that fails the change instead.
    pub fn change(&mut self, first: u64, count: u64, io: &mut dyn Copier) -> io::Result<()> {
        let epoch = self.epoch();
        let end = first.saturating_add(count).min(self.blocks);
        // Read once for the change, and again only after the bound let
        // cuts go.
        let mut holds = self.holds();
        let mut open = self.open();
        for block in first..end {
            if self.in_parts(open, block) {
                // Changed already since the newest cut's instant: no cut
                // reads what it holds now.
                continue;
            }
            // A record made for this change by an instance that died
            // before the block's bit.
            let done = !holds.is_empty()
                && self
                    .records_of(block)
                    .iter()
                    .any(|(_, record)| record.hi == epoch);
            if !done && self.wanted(&holds, block).is_some() {
                if self.copied() + BLOCK_SIZE > self.bound() {
                    // At the bound the oldest cuts go, and with them
                    // perhaps the need.
                    while self.copied() + BLOCK_SIZE > self.bound() && self.abandon() {}
                    holds = self.holds();
                    open = self.open();
                }
                if let Some(lo) = self.wanted(&holds, block) {
                    if let Err(e) = self.copy(block, lo, epoch, io) {
                        if self.mutants.has(Mutants::COPY_FAILURE_FAILS) {
                            return Err(e);
                        }
                        self.abandon_needing(&holds, block);
                        holds = self.holds();
                        open = self.open();
                    }
                }
            }
            self.maps.insert(&mut self.region, open.first, block);
        }
        Ok(())
    }

    /// A copy of block `block`'s content now could not be made: let go
    /// the held cut that reads that content, and every newer one, which
    /// would read it as soon as it took that cut's map. Each goes newest
    /// first and gives its map to the open map, as a cut whose flush is
    /// under way does at a freeze, and each counts as abandoned. The
    /// cuts older than the one that reads the content read their own
    /// content of the block (their copy, or the image they are uploaded
    /// onto), so they stay. Where no held cut's map holds the block, the
    /// cut that reads it is the newest that counts the whole volume as
    /// changed.
    fn abandon_needing(&mut self, holds: &[Held], block: u64) {
        let needing = holds
            .iter()
            .rposition(|h| self.in_parts(h.parts, block))
            .or_else(|| holds.iter().rposition(|h| self.is_whole(&h.cut)));
        let Some(from) = needing else {
            return;
        };
        for held in holds[from..].iter().rev() {
            let abandoned = self.begin_abandoning(held);
            self.give(*held, Heir::Open);
            self.store(at::ABANDONED, abandoned + 1);
        }
        self.free_unneeded(None);
    }

    /// Copy block `block` of the home image, whole, into a free record's
    /// slot, then record it for the epochs `[lo, hi)`.
    fn copy(&mut self, block: u64, lo: u64, hi: u64, io: &mut dyn Copier) -> io::Result<()> {
        let Some(index) = self.free_records.pop() else {
            // Not reached: the bound in force is within the room.
            return Ok(());
        };
        // Aligned, for a read by direct I/O.
        let mut buf = skyzfs_blk::Buf::zeroed(1);
        let done = io
            .read_image(block, &mut buf)
            .and_then(|()| io.write_copy(index as u32, &buf));
        if let Err(e) = done {
            self.free_records.push(index);
            return Err(e);
        }
        let base = self.record_base(index);
        self.store_record(base + 1, lo | (hi << 32));
        self.store_record(base, block + 1);
        self.by_block.entry(block).or_default().push(index);
        Ok(())
    }

    // ---- reading a held cut ----

    /// What held cut `number` reads for `block`, or `None` if no such
    /// cut is held.
    pub fn read(&self, number: u64, block: u64) -> Option<Read> {
        let holds = self.holds();
        let at = holds.iter().position(|h| h.cut.number == number)?;
        let cut = holds[at];
        if !(self.in_parts(cut.parts, block) || self.is_whole(&cut.cut)) {
            return Some(Read::Unchanged);
        }
        let changed = self.in_parts(self.open(), block)
            || holds[at + 1..]
                .iter()
                .any(|h| self.in_parts(h.parts, block));
        if !changed {
            return Some(Read::Image);
        }
        // The copy whose epochs hold the cut's number. With none, the
        // cut reads what the block holds now. That is right while a
        // later cut takes this cut's map, between the heir naming it and
        // this cut going: the heir's part that makes the block look
        // changed is this cut's own map, and the block did not change
        // after this cut's instant. A reader beside the daemon (M2.4)
        // must not take that for an error. Otherwise a defect lost the
        // copy, which the tests see.
        Some(
            self.records_of(block)
                .into_iter()
                .find(|(_, record)| record.lo <= number && number < record.hi)
                .map_or(Read::Image, |(index, _)| Read::Copy(index as u32)),
        )
    }

    /// The blocks in held cut `number`'s map, in order.
    pub fn map(&self, number: u64) -> Vec<u64> {
        let Some(cut) = self.hold(number) else {
            return Vec::new();
        };
        if self.is_whole(&cut.cut) {
            return (0..self.blocks).collect();
        }
        let maps: Vec<usize> = std::iter::once(cut.parts.first)
            .chain(cut.parts.also)
            .collect();
        self.maps.blocks(&self.region, &maps)
    }
}

mod maps;

#[cfg(test)]
pub(crate) mod tests;

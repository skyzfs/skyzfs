//! The intent map (design §5.2, "An intent map for the other case";
//! work package M2.3).
//!
//! The cut state lives in memory the kernel keeps while the machine
//! runs, and is trusted only in the boot that wrote it. After a restart
//! of the machine a new state begins, and without more it counts the
//! whole volume as changed: the next cut would be the whole volume. The
//! intent map narrows that. It holds one bit for each region of the
//! volume, in a file on the volume's own durable file system, and keeps
//! this invariant:
//!
//! > A region whose content may differ from the newest cut committed in
//! > object storage (the map's **base**) has its bit set on the disk.
//!
//! Two rules keep it:
//!
//! - **Set before written.** A write or a write-zeroes into a region
//!   whose bit the disk does not hold for certain waits until the bit
//!   is written and synced ([`IntentMap::covers`], [`IntentMap::set`],
//!   [`IntentMap::write_begin`], [`IntentMap::synced`]). A write into a
//!   region whose bit is held goes on at once.
//! - **Cleared only behind a committed cut.** Once cut *C* is committed
//!   in object storage, and not before, the bits are cleared of the
//!   regions in which nothing changed after *C*'s instant
//!   ([`IntentMap::clear`]); a block in the open map or in a held cut
//!   newer than *C* is such a change (`state.rs`). The header names *C*
//!   as the base ([`IntentMap::commit`]). Neither is synced: a clear
//!   that a power cut loses names a region too many, which costs an
//!   upload and no block, and a base that a power cut loses is an older
//!   committed cut, or none, which names more than *C* would (none: the
//!   whole volume). What the regions not named hold is the newest
//!   committed cut's content whichever base the header names, since
//!   commits only move forward.
//!
//! What the disk holds of a page is known only once a sync after its
//! write has ended. A bit the disk holds for certain is **durable**
//! here. A write of a page with a bit cleared takes that certainty away
//! at once; a sync gives back, for each page, the bits that both the
//! write it covered and every later write of the page have set. A page
//! written while a sync runs takes out of that sync's record the bits
//! it clears, whatever a later write sets again: the disk may keep the
//! write that cleared it and lose the one that set it again.
//!
//! The caller keeps two rules this map cannot see. A sync covers only
//! the writes that have ended when it begins
//! ([`IntentMap::sync_begin`]): the store writes the pages by direct
//! I/O on its own thread, so a page's write has ended when the sync
//! begins. And two writes of one page are never in flight together,
//! so that the later one is the one the disk keeps.
//!
//! # The file
//!
//! [`INTENT_NAME`] in the volume's directory, written by direct I/O as
//! the image is. Pages of 4 KiB: page 0 is the header, and page *p* ≥ 1
//! holds the bits of regions `(p - 1) * PER_PAGE ..`; a bit set is a
//! region named. Every page ends with a CRC-32C of the rest of it, and
//! a bit page begins with its own number, so that a page torn by a
//! power cut, or one written for another place, is seen. A map that is
//! missing or cannot be read is not taken up.
//!
//! # Trust
//!
//! A start takes up the map only if all of these hold, and otherwise
//! the whole volume counts as changed, as before M2.3 ([`Distrust`]):
//!
//! - every page is whole (its checksum holds), of this format, for this
//!   volume (its identity) and this region size;
//! - it names the start token of the start before this one: every open
//!   writes a new token before it changes anything else
//!   ([`crate::volume::START_NAME`]), and a start that kept the map
//!   records its own token in it, synced, before it serves. A start
//!   that did not keep it (one that held no cuts, a refused one, a
//!   build without the map) leaves the token of an older start, and a
//!   write it made can be in a region whose bit is clear;
//! - it has a base: without a committed cut, there is nothing for its
//!   regions to differ from, and the whole volume is the next upload.
//!
//! A start that acknowledges a failure takes the map up by the same
//! rules (the lead's question 1 on M2.3). A failure ends the volume at
//! a write or a sync that failed, but no write begins before its bit is
//! durable, and a failed write or sync of the map fails the request it
//! guards before that request begins. To the map, then, a failed volume
//! is a power cut: every region a write may have changed is named on
//! the disk, whichever writes reached it and whichever did not. The cut
//! state is not taken up after a failure, so this is where the map
//! keeps a failure from costing an upload of the whole volume. No case
//! was found that needs the map distrusted there; a write to the image
//! by anything but the daemon is outside these rules, after a failure
//! or not.
//!
//! # Discards
//!
//! A discard is not noted, as the cut state does not note one until M4:
//! the store's discard changes no content (rule R6 lets a discarded
//! block keep it). A block discarded since the base therefore reads, at
//! the start after a power cut, as the last write before the discard
//! left it: in a region the map names if that write came after the
//! base, and otherwise as the base holds it.

// Clearing a bit follows a commit, which only the test-hooks build's
// release stands in for until M2.4.
#![cfg_attr(not(any(test, feature = "test-hooks")), allow(dead_code))]

use std::collections::BTreeMap;

use skyzfs_blk::Buf;

use super::Mutants;

/// The intent map's name in the volume's directory.
pub(crate) const INTENT_NAME: &str = "intent.map";

/// A page of the file.
pub(crate) const PAGE: usize = 4096;

/// The header's first bytes.
const MAGIC: [u8; 8] = *b"SKZINTNT";

/// This format's version. A file of another is not taken up.
pub(crate) const VERSION: u32 = 1;

/// Where a bit page's bits begin: after its number.
const BITS_AT: usize = 8;

/// Where every page's checksum lies: its last four bytes.
const SUM_AT: usize = PAGE - 4;

/// The regions one bit page holds.
pub(crate) const PER_PAGE: u64 = ((SUM_AT - BITS_AT) * 8) as u64;

/// The words of a bit page's bits in memory, the last one partly used.
const WORDS_PER_PAGE: usize = (PER_PAGE as usize).div_ceil(64);

/// The newest cut committed in object storage, which the regions not
/// named hold as it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Base {
    /// The cut's number, in the cut state that made it.
    pub number: u64,
    /// Its transaction group.
    pub group: u64,
}

/// What a map is for: the volume and its regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Shape {
    /// The volume's identity ([`crate::volume::Volume::identity`]).
    pub identity: [u8; 16],
    /// The blocks of a region: a power of two, at least 64.
    pub region_blocks: u64,
    /// The volume's blocks.
    pub blocks: u64,
}

impl Shape {
    /// The regions of the volume, the last one perhaps short.
    pub(crate) fn regions(&self) -> u64 {
        self.blocks.div_ceil(self.region_blocks)
    }

    /// The bit pages.
    pub(crate) fn bit_pages(&self) -> u64 {
        self.regions().div_ceil(PER_PAGE)
    }

    /// The file's length in bytes.
    pub(crate) fn file_len(&self) -> u64 {
        (1 + self.bit_pages()) * PAGE as u64
    }

    /// The regions `first..first + count` blocks touch.
    fn regions_of(&self, first: u64, count: u64) -> std::ops::Range<u64> {
        let end = first.saturating_add(count).min(self.blocks);
        if count == 0 || first >= end {
            return 0..0;
        }
        first / self.region_blocks..(end - 1) / self.region_blocks + 1
    }

    /// The blocks of region `region`, as `(first, count)`.
    pub(crate) fn blocks_of(&self, region: u64) -> (u64, u64) {
        let first = region * self.region_blocks;
        (first, self.region_blocks.min(self.blocks - first))
    }
}

/// Why a start does not take up the map it found.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Distrust {
    /// There is no map.
    #[error("there is no intent map")]
    Missing,
    /// It could not be read whole.
    #[error("the intent map could not be read: {0}")]
    Unreadable(String),
    /// A page's checksum does not hold, or a bit page names another
    /// place: a page torn by a power cut, or damaged.
    #[error("page {0} of the intent map is not whole")]
    Torn(u64),
    /// Another format, or another version of it.
    #[error("the intent map is of another format or version")]
    Format,
    /// Another volume's: one made again in the same directory.
    #[error("the intent map is another volume's")]
    Identity,
    /// Another region size, or another length of volume.
    #[error("the intent map has another region size or volume length")]
    Shape,
    /// It does not name the token of the start before this one: an open
    /// that did not keep it came between.
    #[error("the intent map was not kept by the start before this one")]
    Token,
    /// No cut was committed since it began.
    #[error("the intent map has no committed cut")]
    NoBase,
}

/// The intent map in memory: see the [module documentation](self).
#[derive(Debug, Clone)]
pub(crate) struct IntentMap {
    shape: Shape,
    /// The start token of the start that keeps it.
    token: [u8; 16],
    base: Option<Base>,
    /// The bits as they are to be, one `WORDS_PER_PAGE` run a page.
    bits: Vec<u64>,
    /// The bits the disk holds set for certain.
    durable: Vec<u64>,
    /// Pages to write: the header (0) or a bit page (≥ 1).
    dirty: std::collections::BTreeSet<u64>,
    /// The bit pages written since the last sync began, with the bits
    /// each was written with last.
    unsynced: BTreeMap<u64, Vec<u64>>,
    /// The bit pages the sync under way covers, with the bits that
    /// every write of each since the covered one has set.
    syncing: Option<BTreeMap<u64, Vec<u64>>>,
    mutants: Mutants,
}

impl IntentMap {
    /// A new map, kept under `token`: no base, and every region named,
    /// so that the invariant holds whatever was written before it began
    /// (a cut state taken up beside a map that was lost holds changes
    /// after a cut's instant that no bit of a new map would name).
    /// Without a base the whole volume is the next upload in any case,
    /// and the first commit clears what it may. Every page is to be
    /// written ([`IntentMap::write_begin`]).
    pub(crate) fn new(shape: Shape, token: [u8; 16], mutants: Mutants) -> Self {
        let mut map = Self::empty(shape, token, mutants);
        for region in 0..shape.regions() {
            let (word, bit) = Self::place(region);
            map.bits[word] |= bit;
        }
        map.dirty = (0..=shape.bit_pages()).collect();
        map
    }

    /// A map with nothing named and nothing to write.
    fn empty(shape: Shape, token: [u8; 16], mutants: Mutants) -> Self {
        let words = shape.bit_pages() as usize * WORDS_PER_PAGE;
        Self {
            shape,
            token,
            base: None,
            bits: vec![0; words],
            durable: vec![0; words],
            dirty: std::collections::BTreeSet::new(),
            unsynced: BTreeMap::new(),
            syncing: None,
            mutants,
        }
    }

    /// The map in `bytes`, the whole file as read, checked against
    /// `shape`. Its token and base are checked by [`IntentMap::trust`].
    /// What it names is taken as durable: the caller syncs the file
    /// before it relies on that (a page a dead instance wrote by direct
    /// I/O can still be in the device's cache).
    pub(crate) fn read(bytes: &[u8], shape: Shape, mutants: Mutants) -> Result<Self, Distrust> {
        if bytes.len() < PAGE {
            return Err(Distrust::Format);
        }
        let page = |p: u64| &bytes[p as usize * PAGE..(p as usize + 1) * PAGE];
        let header = page(0);
        if header[..8] != MAGIC {
            return Err(Distrust::Format);
        }
        if !whole(header) {
            return Err(Distrust::Torn(0));
        }
        let u32_at =
            |at: usize| u32::from_le_bytes(header[at..at + 4].try_into().unwrap_or([0; 4]));
        let u64_at =
            |at: usize| u64::from_le_bytes(header[at..at + 8].try_into().unwrap_or([0; 8]));
        if u32_at(8) != VERSION {
            return Err(Distrust::Format);
        }
        if header[16..32] != shape.identity {
            return Err(Distrust::Identity);
        }
        if u64_at(32) != shape.region_blocks || u64_at(40) != shape.blocks {
            return Err(Distrust::Shape);
        }
        if bytes.len() as u64 != shape.file_len() {
            return Err(Distrust::Shape);
        }
        let mut token = [0; 16];
        token.copy_from_slice(&header[48..64]);
        let base = (header[64] == 1).then(|| Base {
            number: u64_at(72),
            group: u64_at(80),
        });
        let mut map = Self::empty(shape, token, mutants);
        map.base = base;
        for p in 1..=shape.bit_pages() {
            let bits = page(p);
            if !whole(bits) && !mutants.has(Mutants::INTENT_TORN_TRUSTED) {
                return Err(Distrust::Torn(p));
            }
            if u64::from_le_bytes(bits[..8].try_into().unwrap_or([0; 8])) != p {
                return Err(Distrust::Torn(p));
            }
            let words = map.page_words_mut(p);
            for (i, word) in words.iter_mut().enumerate() {
                let at = BITS_AT + i * 8;
                let mut eight = [0u8; 8];
                let end = (at + 8).min(SUM_AT);
                eight[..end - at].copy_from_slice(&bits[at..end]);
                *word = u64::from_le_bytes(eight);
            }
        }
        map.durable.clone_from(&map.bits);
        Ok(map)
    }

    /// Whether a start that found start token `found` may take this map
    /// up. Whether the start acknowledges a failure does not matter:
    /// see "Trust" in the [module documentation](self).
    pub(crate) fn trust(&self, found: Option<[u8; 16]>) -> Result<(), Distrust> {
        if found != Some(self.token) && !self.mutants.has(Mutants::INTENT_TOKEN_UNCHECKED) {
            return Err(Distrust::Token);
        }
        if self.base.is_none() {
            return Err(Distrust::NoBase);
        }
        Ok(())
    }

    /// Keep the map under `token` from now on: the header is to be
    /// written, and synced before the start serves.
    pub(crate) fn kept_by(&mut self, token: [u8; 16]) {
        self.token = token;
        self.dirty.insert(0);
    }

    pub(crate) fn shape(&self) -> Shape {
        self.shape
    }

    #[cfg(test)]
    pub(crate) fn base(&self) -> Option<Base> {
        self.base
    }

    /// The regions named now.
    pub(crate) fn named(&self) -> Vec<u64> {
        let mut named = Vec::new();
        for region in 0..self.shape.regions() {
            if self.is_set(&self.bits, region) {
                named.push(region);
            }
        }
        named
    }

    /// Where region `region`'s bit lies in a vector of bits.
    fn place(region: u64) -> (usize, u64) {
        let page = region / PER_PAGE;
        let within = region % PER_PAGE;
        (
            page as usize * WORDS_PER_PAGE + (within / 64) as usize,
            1 << (within % 64),
        )
    }

    fn is_set(&self, bits: &[u64], region: u64) -> bool {
        let (word, bit) = Self::place(region);
        bits.get(word).is_some_and(|w| w & bit != 0)
    }

    fn page_words(bits: &[u64], page: u64) -> &[u64] {
        let from = (page as usize - 1) * WORDS_PER_PAGE;
        &bits[from..from + WORDS_PER_PAGE]
    }

    fn page_words_mut(&mut self, page: u64) -> &mut [u64] {
        let from = (page as usize - 1) * WORDS_PER_PAGE;
        &mut self.bits[from..from + WORDS_PER_PAGE]
    }

    /// Whether every region that blocks `first..first + count` touch is
    /// named, and its bit held by the disk: a write there may begin. A
    /// bit cleared here and not yet written is still on the disk, but
    /// the write of its page will clear it there, so it does not cover
    /// a write until it is set again ([`IntentMap::set`]).
    pub(crate) fn covers(&self, first: u64, count: u64) -> bool {
        if self.mutants.has(Mutants::INTENT_WRITE_UNCOVERED) {
            return true;
        }
        self.shape
            .regions_of(first, count)
            .all(|region| self.is_set(&self.bits, region) && self.is_set(&self.durable, region))
    }

    /// Name every region that blocks `first..first + count` touch: their
    /// pages are to be written and synced before a write there begins.
    pub(crate) fn set(&mut self, first: u64, count: u64) {
        for region in self.shape.regions_of(first, count) {
            let (word, bit) = Self::place(region);
            if let Some(w) = self.bits.get_mut(word) {
                *w |= bit;
            }
            if !self.is_set(&self.durable, region) {
                self.dirty.insert(region / PER_PAGE + 1);
            }
        }
    }

    /// Whether a page is to be written.
    #[cfg(test)]
    pub(crate) fn has_dirty(&self) -> bool {
        !self.dirty.is_empty()
    }

    /// The pages to write now, as `(offset, image)`, the header first.
    /// A bit cleared in a page written is no longer durable from here
    /// on, and the sync under way, if any, will not make it so; a bit
    /// set becomes durable when a sync that began after this write
    /// ends ([`IntentMap::sync_begin`], [`IntentMap::synced`]).
    pub(crate) fn write_begin(&mut self) -> Vec<(u64, Buf)> {
        let mut out = Vec::new();
        for page in std::mem::take(&mut self.dirty) {
            let mut image = Buf::zeroed(1);
            if page == 0 {
                self.header_into(&mut image);
            } else {
                image[..8].copy_from_slice(&page.to_le_bytes());
                let words = Self::page_words(&self.bits, page).to_vec();
                for (i, word) in words.iter().enumerate() {
                    let at = BITS_AT + i * 8;
                    let end = (at + 8).min(SUM_AT);
                    image[at..end].copy_from_slice(&word.to_le_bytes()[..end - at]);
                }
                let from = (page as usize - 1) * WORDS_PER_PAGE;
                for (i, word) in words.iter().enumerate() {
                    self.durable[from + i] &= word;
                }
                // The sync under way covers an earlier write of this
                // page: what this write clears, it does not make
                // durable, though a later write may set it again (D1 of
                // the review of #105).
                let narrowed = !self.mutants.has(Mutants::INTENT_SYNC_UNNARROWED);
                if let Some(covered) = self
                    .syncing
                    .as_mut()
                    .and_then(|syncing| syncing.get_mut(&page))
                    .filter(|_| narrowed)
                {
                    for (bits, word) in covered.iter_mut().zip(&words) {
                        *bits &= word;
                    }
                }
                self.unsynced.insert(page, words);
            }
            seal(&mut image);
            out.push((page * PAGE as u64, image));
        }
        out
    }

    fn header_into(&self, image: &mut [u8]) {
        image[..8].copy_from_slice(&MAGIC);
        image[8..12].copy_from_slice(&VERSION.to_le_bytes());
        image[16..32].copy_from_slice(&self.shape.identity);
        image[32..40].copy_from_slice(&self.shape.region_blocks.to_le_bytes());
        image[40..48].copy_from_slice(&self.shape.blocks.to_le_bytes());
        image[48..64].copy_from_slice(&self.token);
        if let Some(base) = self.base {
            image[64] = 1;
            image[72..80].copy_from_slice(&base.number.to_le_bytes());
            image[80..88].copy_from_slice(&base.group.to_le_bytes());
        }
    }

    /// A sync of the file begins: it covers every write issued so far,
    /// each of which the caller has seen end, and no later one.
    pub(crate) fn sync_begin(&mut self) {
        let covered = std::mem::take(&mut self.unsynced);
        match &mut self.syncing {
            // Not reached: one sync at a time. Kept whole all the same.
            Some(syncing) => syncing.extend(covered),
            None => self.syncing = Some(covered),
        }
    }

    /// The sync under way has ended, well or not. Where it succeeded,
    /// each bit page it covered holds for certain the bits set both in
    /// the write it covered and in every later write of the page
    /// ([`IntentMap::write_begin`] takes the others out of its record).
    pub(crate) fn synced(&mut self, ok: bool) {
        let Some(covered) = self.syncing.take() else {
            return;
        };
        if !ok {
            return;
        }
        for (page, words) in covered {
            let from = (page as usize - 1) * WORDS_PER_PAGE;
            for (i, word) in words.iter().enumerate() {
                self.durable[from + i] |= word;
            }
        }
    }

    /// Cut `base` is committed in object storage: the header is to name
    /// it. Called only after the commit, as [`IntentMap::clear`] is.
    pub(crate) fn commit(&mut self, base: Base) {
        self.base = Some(base);
        self.dirty.insert(0);
    }

    /// Clear the bit of every region for which `changed` says that
    /// nothing in it changed after the instant of the cut just
    /// committed. Called only once that cut is committed in object
    /// storage. The pages are to be written; no sync is needed for a
    /// clear.
    pub(crate) fn clear(&mut self, changed: impl Fn(u64, u64) -> bool) -> u64 {
        let mut cleared = 0;
        for region in 0..self.shape.regions() {
            if !self.is_set(&self.bits, region) {
                continue;
            }
            let (first, count) = self.shape.blocks_of(region);
            if changed(first, count) && !self.mutants.has(Mutants::INTENT_CLEARED_CHANGED) {
                continue;
            }
            let (word, bit) = Self::place(region);
            if let Some(w) = self.bits.get_mut(word) {
                *w &= !bit;
            }
            self.dirty.insert(region / PER_PAGE + 1);
            cleared += 1;
        }
        cleared
    }
}

/// Whether a page's checksum holds.
fn whole(page: &[u8]) -> bool {
    let sum = u32::from_le_bytes(page[SUM_AT..].try_into().unwrap_or([0; 4]));
    crc32c::crc32c(&page[..SUM_AT]) == sum
}

/// Put a page's checksum in place.
fn seal(page: &mut [u8]) {
    let sum = crc32c::crc32c(&page[..SUM_AT]);
    page[SUM_AT..].copy_from_slice(&sum.to_le_bytes());
}

#[cfg(test)]
mod model;
#[cfg(test)]
mod tests;

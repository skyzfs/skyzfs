//! The maps of changed blocks (design §5.2), behind what the cut state
//! asks of them.
//!
//! The protocol in `state.rs` names a map by its number, below
//! [`MAPS`], and asks of it six things, never how it is laid out:
//! whether a block is in it ([`Maps::contains`]), to put a block in it
//! ([`Maps::insert`]), to move all of one map into another
//! ([`Maps::move_into`]), to clear one ([`Maps::clear`]), what a pass
//! over one costs ([`Maps::weight`]), and the blocks in some maps, in
//! order ([`Maps::blocks`]), some of them from a block on
//! ([`Maps::blocks_from`]), or the first of them in a range of blocks
//! ([`Maps::first_in`]). A freeze and a taking only rename maps (the
//! word of the count names the open map, a cut's map word its parts),
//! so they do not reach in here. What the protocol relies on, at every
//! store a death can follow, is all a layout has to keep:
//!
//! - after `insert` returns, the block is in the map, and every later
//!   pass over the map (a move, a clearing, a listing) finds it;
//! - during `move_into(from, to)`, a block in either map before is in
//!   one of them after each store; at the end it is in `to`, and `from`
//!   is empty, so that a move a death interrupted is finished by doing
//!   it again;
//! - `clear` is for a map that nothing names; one a death left partly
//!   cleared is finished by clearing it again.
//!
//! # This layout: flat
//!
//! A map is a bit for each block of the volume, in 64-bit words, with a
//! summary before it: a bit for each of its words that may be non-zero.
//! The summary's bit is stored before the word's, so it is never short
//! of a word that is set. A move and a clearing visit only the words the
//! summary names, so a pass costs the words set, not the volume; the
//! summary itself is a sixty-fourth of the map, read whole by a pass. A
//! move sets each word in `to` (its summary first) before it clears the
//! word in `from`, and clears `from`'s summary last. For a TiB at 4 KiB
//! blocks a map is 32 MiB and its summary 512 KiB.
//!
//! # A layout in proportion to a cache (M3; design goal 9)
//!
//! The flat layout is in proportion to the volume. Behind a cache much
//! smaller than the volume (M3's `cache` policy), the blocks that can be
//! in any map are at most the blocks written since the oldest held
//! cut's instant, which are local; so a layout in proportion to the
//! cache is a pool of chunks (a word of bits for 64 blocks, say, or a
//! few) shared by the maps, sized by the cache, and for each map a
//! directory from a chunk's place in the volume to a chunk of the pool,
//! itself a table sized by the pool (a flat directory for a hundred TiB
//! would be as large as the maps it saves). It keeps the guarantees
//! above as follows: `insert` takes a clean chunk from the pool and
//! names it in the directory in one store, before it sets the bit; a
//! move hands a chunk over by renaming its directory entry in one store
//! when `to` has none at that place, and otherwise sets the bits in
//! `to`'s chunk before it frees `from`'s; `clear` frees the chunks after
//! it has cleared their entries. None of that changes the protocol.
//!
//! What changes is the pool running out. A chunk counted as changed
//! whole (a directory entry with no bits) costs no chunk and keeps a
//! held cut's map a superset of its changes, which is safe for what is
//! uploaded. Two rules of `state.rs` then need care, and are where the
//! death-safety argument changes: the open map must stay exact, since a
//! block in it is one that needs no copy (a block counted in it wrongly
//! would be overwritten uncopied); and a record's `lo` is taken from the
//! newest held map that holds the block, as the epoch its content
//! dates from, which a map that only may hold it does not say (it would
//! have to fall through to the next exact map below, or to 0). So with
//! a pool that can run out, the open map gets chunks first, a held map
//! gives up chunks by counting them whole, and the copy rule reads
//! exactness from the directory. Until M3 the store holds the whole
//! volume, and this layout is enough; the region's format version is
//! what lets M3 change it.

use super::{Mutants, Region, MAPS};

/// The maps' layout in the region: see the [module documentation](self).
#[derive(Debug, Clone, Copy)]
pub(super) struct Maps {
    /// Where the first map's summary lies.
    base: usize,
    /// A map's summary, in words.
    summary: usize,
    /// A map's bits, in words.
    bits: usize,
    blocks: u64,
    mutants: Mutants,
}

impl Maps {
    /// The words of one map for a volume of `blocks` blocks: its
    /// summary, then its bits.
    fn shape(blocks: u64) -> (usize, usize) {
        let bits = blocks.div_ceil(64) as usize;
        (bits.div_ceil(64), bits)
    }

    /// The words every map takes in the region, for a volume of `blocks`
    /// blocks.
    pub(super) fn words(blocks: u64) -> usize {
        let (summary, bits) = Self::shape(blocks);
        MAPS * (summary + bits)
    }

    /// The maps of a volume of `blocks` blocks, from word `base` of the
    /// region on.
    pub(super) fn new(base: usize, blocks: u64, mutants: Mutants) -> Self {
        let (summary, bits) = Self::shape(blocks);
        Self {
            base,
            summary,
            bits,
            blocks,
            mutants,
        }
    }

    fn summary_base(&self, map: usize) -> usize {
        self.base + map * (self.summary + self.bits)
    }

    fn bits_base(&self, map: usize) -> usize {
        self.summary_base(map) + self.summary
    }

    /// Whether block `block` is in map `map`.
    pub(super) fn contains(&self, region: &impl Region, map: usize, block: u64) -> bool {
        region.load(self.bits_base(map) + (block / 64) as usize) & (1 << (block % 64)) != 0
    }

    /// Note word `word` of map `map` in its summary.
    fn note_word(&self, region: &mut impl Region, map: usize, word: usize) {
        let at = self.summary_base(map) + word / 64;
        let summary = region.load(at);
        let bit = 1 << (word % 64);
        if summary & bit == 0 {
            region.store(at, summary | bit);
        }
    }

    /// Put block `block` in map `map`: its summary first, so that a pass
    /// never misses a word.
    pub(super) fn insert(&self, region: &mut impl Region, map: usize, block: u64) {
        let word = (block / 64) as usize;
        let at = self.bits_base(map) + word;
        let bits = region.load(at);
        let bit = 1 << (block % 64);
        if bits & bit != 0 {
            return;
        }
        let after = self.mutants.has(Mutants::SUMMARY_AFTER_BIT);
        if !after {
            self.note_word(region, map, word);
        }
        region.store(at, bits | bit);
        if after {
            self.note_word(region, map, word);
        }
    }

    /// Put blocks `first..first + count` in map `map`, a word at a time:
    /// each word's summary bit before the word, as [`Maps::insert`]
    /// does. For a map nothing names yet (a state being begun, M2.3).
    pub(super) fn insert_range(
        &self,
        region: &mut impl Region,
        map: usize,
        first: u64,
        count: u64,
    ) {
        let end = first.saturating_add(count).min(self.blocks);
        let mut block = first;
        while block < end {
            let word = (block / 64) as usize;
            let within = block % 64;
            let take = (64 - within).min(end - block);
            let mask = if take == 64 {
                u64::MAX
            } else {
                ((1u64 << take) - 1) << within
            };
            self.note_word(region, map, word);
            let at = self.bits_base(map) + word;
            let bits = region.load(at);
            if bits | mask != bits {
                region.store(at, bits | mask);
            }
            block += take;
        }
    }

    /// The words set in any of `maps`, in order: a word `w` holds blocks
    /// `64 w .. 64 w + 64`.
    pub(super) fn set_words(&self, region: &impl Region, maps: &[usize]) -> Vec<usize> {
        let mut words: Vec<usize> = maps
            .iter()
            .flat_map(|&map| self.noted_words(region, map))
            .filter(|&word| {
                maps.iter()
                    .any(|&map| region.load(self.bits_base(map) + word) != 0)
            })
            .collect();
        words.sort_unstable();
        words.dedup();
        words
    }

    /// The words of map `map` that its summary says may be set.
    fn noted_words(&self, region: &impl Region, map: usize) -> Vec<usize> {
        let base = self.summary_base(map);
        let mut words = Vec::new();
        for i in 0..self.summary {
            let mut summary = region.load(base + i);
            while summary != 0 {
                words.push(i * 64 + summary.trailing_zeros() as usize);
                summary &= summary - 1;
            }
        }
        words
    }

    /// Move every block of map `from` into map `to`: each word set in
    /// `to` before it is cleared in `from`, so that a death leaves the
    /// union whole. `from` is empty after.
    pub(super) fn move_into(&self, region: &mut impl Region, from: usize, to: usize) {
        let first = self.mutants.has(Mutants::MOVE_CLEARS_FIRST);
        for word in self.noted_words(region, from) {
            let at_from = self.bits_base(from) + word;
            let bits = region.load(at_from);
            if bits == 0 {
                continue;
            }
            if first {
                region.store(at_from, 0);
            }
            self.note_word(region, to, word);
            let at_to = self.bits_base(to) + word;
            let there = region.load(at_to);
            if there | bits != there {
                region.store(at_to, there | bits);
            }
            if !first {
                region.store(at_from, 0);
            }
        }
        self.clear_summary(region, from);
    }

    /// Clear map `map`: every word its summary names, then the summary.
    pub(super) fn clear(&self, region: &mut impl Region, map: usize) {
        for word in self.noted_words(region, map) {
            let at = self.bits_base(map) + word;
            if region.load(at) != 0 {
                region.store(at, 0);
            }
        }
        self.clear_summary(region, map);
    }

    fn clear_summary(&self, region: &mut impl Region, map: usize) {
        let base = self.summary_base(map);
        for i in 0..self.summary {
            if region.load(base + i) != 0 {
                region.store(base + i, 0);
            }
        }
    }

    /// What a pass over map `map` costs, in words visited.
    pub(super) fn weight(&self, region: &impl Region, map: usize) -> u64 {
        let base = self.summary_base(map);
        (0..self.summary)
            .map(|i| u64::from(region.load(base + i).count_ones()))
            .sum()
    }

    /// The blocks in any of `maps` from block `from` on, in order, at
    /// most `limit` of them (at least one); and the block after them in
    /// the maps, or `None` where there is none. A pass visits only the
    /// summary from `from` on and the words it names, until the limit.
    pub(super) fn blocks_from(
        &self,
        region: &impl Region,
        maps: &[usize],
        from: u64,
        limit: usize,
    ) -> (Vec<u64>, Option<u64>) {
        let mut blocks = Vec::new();
        let first_word = (from / 64) as usize;
        for i in first_word / 64..self.summary {
            let mut summary = maps
                .iter()
                .fold(0, |acc, &map| acc | region.load(self.summary_base(map) + i));
            if i == first_word / 64 {
                summary &= u64::MAX << (first_word % 64);
            }
            while summary != 0 {
                let word = i * 64 + summary.trailing_zeros() as usize;
                summary &= summary - 1;
                let mut bits = maps
                    .iter()
                    .fold(0, |acc, &map| acc | region.load(self.bits_base(map) + word));
                if word == first_word {
                    bits &= u64::MAX << (from % 64);
                }
                while bits != 0 {
                    let block = word as u64 * 64 + u64::from(bits.trailing_zeros());
                    bits &= bits - 1;
                    if block >= self.blocks {
                        return (blocks, None);
                    }
                    if blocks.len() >= limit.max(1) {
                        return (blocks, Some(block));
                    }
                    blocks.push(block);
                }
            }
        }
        (blocks, None)
    }

    /// The first block in any of `maps` in `from..until`, or `None`
    /// where there is none. A look visits the summary over the range
    /// only, and of the words it names those up to the first block: a
    /// slice bounds each look for a run's end so, and the slices of a
    /// cut read the summary once over (F4 of the review of #125).
    pub(super) fn first_in(
        &self,
        region: &impl Region,
        maps: &[usize],
        from: u64,
        until: u64,
    ) -> Option<u64> {
        let until = until.min(self.blocks);
        if from >= until {
            return None;
        }
        let first_word = (from / 64) as usize;
        let last_word = ((until - 1) / 64) as usize;
        for i in first_word / 64..=last_word / 64 {
            let mut summary = maps
                .iter()
                .fold(0, |acc, &map| acc | region.load(self.summary_base(map) + i));
            if i == first_word / 64 {
                summary &= u64::MAX << (first_word % 64);
            }
            if i == last_word / 64 {
                summary &= u64::MAX >> (63 - last_word % 64);
            }
            while summary != 0 {
                let word = i * 64 + summary.trailing_zeros() as usize;
                summary &= summary - 1;
                let mut bits = maps
                    .iter()
                    .fold(0, |acc, &map| acc | region.load(self.bits_base(map) + word));
                if word == first_word {
                    bits &= u64::MAX << (from % 64);
                }
                if bits != 0 {
                    let block = word as u64 * 64 + u64::from(bits.trailing_zeros());
                    return (block < until).then_some(block);
                }
            }
        }
        None
    }

    /// The blocks in any of `maps`, in order.
    pub(super) fn blocks(&self, region: &impl Region, maps: &[usize]) -> Vec<u64> {
        let mut words: Vec<usize> = maps
            .iter()
            .flat_map(|&map| self.noted_words(region, map))
            .collect();
        words.sort_unstable();
        words.dedup();
        let mut blocks = Vec::new();
        for word in words {
            let mut bits = maps
                .iter()
                .fold(0, |acc, &map| acc | region.load(self.bits_base(map) + word));
            while bits != 0 {
                let block = word as u64 * 64 + u64::from(bits.trailing_zeros());
                if block < self.blocks {
                    blocks.push(block);
                }
                bits &= bits - 1;
            }
        }
        blocks
    }
}

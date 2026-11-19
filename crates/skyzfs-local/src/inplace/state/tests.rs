//! Tests of the cut state alone: a region in memory, and a volume whose
//! blocks hold one word each. A world runs a program of changes, cuts,
//! completed and failed flushes and uploads against the state and
//! against two models: the volume as it was at each cut's instant, and
//! the rules of D7 for which cuts are held. After every step each held
//! cut must read, block by block, as the volume did at its instant when
//! uploaded onto the image of the last cut uploaded; and the held cuts
//! must be those the rules hold. A death at any store of the region
//! leaves the world to a new instance, which takes up the state and does
//! again what the death interrupted, as the kernel issues a request
//! again.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::*;

/// The volume's blocks: four words of a map, so that a pass over a map
/// meets several words and their summary.
const BLOCKS: u64 = 200;
const SIZE: u64 = BLOCKS * BLOCK_SIZE;
/// The boot the tests' states are begun in.
pub(crate) const BOOT: [u8; 16] = *b"the tests' boot.";
/// The owner of the tests' states.
const OWNER: Owner = Owner {
    volume: *b"the tests' disk.",
    boot: BOOT,
    start: *b"the tests' start",
};
/// Where a program's changes begin: twelve blocks, some at the edges of
/// a map's words, so that programs change the same blocks often.
const STARTS: [u64; 12] = [0, 1, 2, 62, 63, 64, 65, 126, 127, 128, 190, 197];

/// The chunks of [`HOLE_BLOCKS`] blocks that are holes of the image when
/// a world with holes begins (M2.4e): of the thirteen, the first, the
/// last (eight blocks inside the volume), runs of two and three, and
/// chunks next to the edges of a map's words. Every start of
/// [`STARTS`] but 190 lies in one or next to one.
const HOLE_CHUNKS: [u64; 8] = [0, 2, 3, 5, 7, 8, 9, 12];
/// Blocks that are zeros in a world with holes but lie in a chunk of
/// data, so that the set does not name them: a range that covers a
/// chunk in part names none of it.
const PART_HOLE: (u64, u64) = (100, 8);
/// The content of block `b` of a world with holes, where it is data.
const DATA: u64 = 1 << 40;

/// The ranges of blocks that a world with holes gives its state's
/// begin.
fn hole_ranges() -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = HOLE_CHUNKS
        .iter()
        .map(|&chunk| (chunk * HOLE_BLOCKS, HOLE_BLOCKS))
        .collect();
    ranges.push(PART_HOLE);
    ranges
}

/// Whether block `block` is zeros in a world with holes when it begins.
fn zero_at_begin(block: u64) -> bool {
    HOLE_CHUNKS.contains(&(block / HOLE_BLOCKS))
        || (PART_HOLE.0..PART_HOLE.0 + PART_HOLE.1).contains(&block)
}

/// The home image and the copies file, one word for each block: what a
/// death leaves as it was.
#[derive(Debug, Clone, Default)]
struct Disk {
    image: Vec<u64>,
    copies: BTreeMap<u32, u64>,
}

impl Copier for Disk {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        for (i, block) in buf.chunks_mut(BLOCK_SIZE as usize).enumerate() {
            block[..8].copy_from_slice(&self.image[first as usize + i].to_le_bytes());
        }
        Ok(())
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        for (i, block) in buf.chunks(BLOCK_SIZE as usize).enumerate() {
            let mut word = [0u8; 8];
            word.copy_from_slice(&block[..8]);
            self.copies
                .insert(first + i as u32, u64::from_le_bytes(word));
        }
        Ok(())
    }
}

/// The state that marks cut `number` in these tests.
fn marked(number: u64) -> State {
    State {
        txg: 100 + number,
        root: [number; ROOT_WORDS],
    }
}

/// One step of a program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Write blocks `from` to `from + n`.
    Write(u64, u64),
    /// Zero them.
    Zero(u64, u64),
    /// Discard them.
    Discard(u64, u64),
    /// A cut's instant.
    Cut,
    /// The oldest flush under way completes.
    Flushed,
    /// The oldest flush under way fails: the volume fails.
    Fail,
    /// The cut being uploaded is uploaded and let go.
    Release,
    /// The pool's history went back past the cuts (a mismatch, D7):
    /// every held cut is let go, and the next cut counts the whole
    /// volume as changed (`abandon_all`). Only in the programs of
    /// [`program_with_mismatches`] (the review of #125, G1).
    Mismatch,
}

/// The rules of D7, independently of the state: the held cuts as
/// `(number, lo, phase)`, oldest first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Rules {
    held: Vec<(u64, u64, Phase)>,
    open_lo: u64,
}

impl Rules {
    /// A cut frozen while another's flush is under way takes that
    /// cut's map through the open map, and the other is not held.
    fn freeze(&mut self, number: u64) {
        if let Some(f) = self.held.iter().position(|c| c.2 == Phase::Flushing) {
            let (_, lo, _) = self.held.remove(f);
            if self.held.is_empty() {
                self.open_lo = lo;
            }
        }
        let lo = self.held.last().map_or(self.open_lo, |cut| cut.0);
        self.held.push((number, lo, Phase::Flushing));
    }

    fn flushed(&mut self, number: u64) {
        let Some(i) = self.held.iter().position(|c| c.0 == number) else {
            return;
        };
        if let Some(w) = self
            .held
            .iter()
            .position(|c| c.2 == Phase::Waiting && c.0 < number)
        {
            let lo = self.held[w].1;
            self.held[i].1 = self.held[i].1.min(lo);
            self.held[i].2 = Phase::Waiting;
            self.held.remove(w);
        } else if self.held.iter().any(|c| c.2 == Phase::Uploading) {
            self.held[i].2 = Phase::Waiting;
        } else {
            self.held[i].2 = Phase::Uploading;
        }
    }

    /// A cut whose flush failed gives its map to the open map.
    fn failed(&mut self, number: u64) {
        if let Some(i) = self.held.iter().position(|c| c.0 == number) {
            let (_, lo, _) = self.held.remove(i);
            if self.held.is_empty() {
                self.open_lo = lo;
            }
        }
    }

    fn uploading(&self) -> Option<u64> {
        self.held
            .iter()
            .find(|c| c.2 == Phase::Uploading)
            .map(|c| c.0)
    }

    fn release(&mut self) {
        let Some(u) = self.held.iter().position(|c| c.2 == Phase::Uploading) else {
            return;
        };
        let number = self.held[u].0;
        self.held.remove(u);
        if self.held.is_empty() {
            self.open_lo = number;
        }
        if let Some(w) = self.held.iter_mut().find(|c| c.2 == Phase::Waiting) {
            w.2 = Phase::Uploading;
        }
    }

    /// The oldest held cut is abandoned at the bound.
    fn abandon(&mut self) {
        if self.held.is_empty() {
            return;
        }
        let (_, lo, phase) = self.held.remove(0);
        let heir = if phase == Phase::Uploading {
            self.held
                .iter()
                .position(|c| c.2 == Phase::Waiting)
                .or_else(|| self.held.iter().position(|c| c.2 == Phase::Flushing))
        } else {
            (!self.held.is_empty()).then_some(0)
        };
        match heir {
            Some(h) => {
                self.held[h].1 = self.held[h].1.min(lo);
                if self.held[h].2 == Phase::Waiting {
                    self.held[h].2 = Phase::Uploading;
                }
            }
            None => self.open_lo = lo,
        }
    }

    /// A mismatch lets every held cut go, oldest first, each as at the
    /// bound: the open map ends with the lowest `lo`.
    fn mismatch(&mut self) {
        while !self.held.is_empty() {
            self.abandon();
        }
    }
}

/// A program's run: the state, the disk under it and the models.
struct World {
    region: MemRegion,
    state: CutState<MemRegion>,
    disk: Disk,
    mutants: Mutants,
    bound: u64,
    /// The volume at each cut's instant, and which of its blocks were
    /// discarded and not written since (any content will do).
    snaps: BTreeMap<u64, (Vec<u64>, Vec<bool>)>,
    discarded: Vec<bool>,
    /// The blocks a step has changed or discarded: the others hold what
    /// they held at the start in every cut, and are not read back.
    touched: Vec<bool>,
    /// For each block, the epochs in which it was written or zeroed.
    changes: Vec<BTreeSet<u64>>,
    /// The epochs that count the whole volume as changed: the first, of
    /// a state begun with nothing known, and the open epoch at each
    /// mismatch.
    whole: BTreeSet<u64>,
    /// The image the next upload goes onto: `None` where nothing was
    /// uploaded or a hole was.
    uploaded: Vec<Option<u64>>,
    rules: Rules,
    flushing: VecDeque<u64>,
    abandoned: u64,
    failed: bool,
    next_value: u64,
    deaths: u64,
    /// Whether the state began with the image's holes (M2.4e).
    holes: bool,
}

impl World {
    /// A world whose state begins with the holes of an image of data
    /// and holes (`holes`), or of an image of zeros with none. With
    /// holes the first and last blocks of every chunk, and of the zeros
    /// in a chunk of data, are read back, touched or not: a block of
    /// data that read as a hole's zeros would read 0, not its data.
    fn begun(mutants: u128, bound_blocks: u64, holes: bool) -> Self {
        let bound = bound_blocks * BLOCK_SIZE;
        let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
        let (state, image) = if holes {
            let state = CutState::begin_with(
                region.clone(),
                SIZE,
                bound,
                OWNER,
                Mutants(mutants),
                None,
                Some(&hole_ranges()),
            );
            let image = (0..BLOCKS)
                .map(|block| {
                    if zero_at_begin(block) {
                        0
                    } else {
                        DATA + block
                    }
                })
                .collect();
            (state, image)
        } else {
            let state = CutState::begin(region.clone(), SIZE, bound, OWNER, Mutants(mutants));
            (state, vec![0; BLOCKS as usize])
        };
        Self {
            region,
            state,
            disk: Disk {
                image,
                copies: BTreeMap::new(),
            },
            mutants: Mutants(mutants),
            bound,
            snaps: BTreeMap::new(),
            discarded: vec![false; BLOCKS as usize],
            touched: (0..BLOCKS)
                .map(|block| {
                    let edge = matches!(block % HOLE_BLOCKS, 0 | 15)
                        || block + 1 == BLOCKS
                        || block == PART_HOLE.0
                        || block + 1 == PART_HOLE.0 + PART_HOLE.1;
                    holes && edge
                })
                .collect(),
            changes: vec![BTreeSet::new(); BLOCKS as usize],
            whole: BTreeSet::from([1]),
            uploaded: vec![None; BLOCKS as usize],
            rules: Rules::default(),
            flushing: VecDeque::new(),
            abandoned: 0,
            failed: false,
            next_value: 1,
            deaths: 0,
            holes,
        }
    }

    /// Run `step`; a death at a store leaves it to a new instance, which
    /// does it again.
    fn step(&mut self, step: Step) -> Result<(), String> {
        quiet_deaths();
        let value = self.next_value;
        // What the step is to do, to tell after a death whether it did.
        let cut = self.state.count() + 1;
        let uploading = self.rules.uploading();
        let held = self.state.held();
        let done = catch_unwind(AssertUnwindSafe(|| self.apply(step, value)));
        match done {
            Ok(result) => {
                self.next_value += 1;
                result?;
                self.check_rules(step)?;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
                self.deaths += 1;
                // What a reader beside the daemon sees at the store where
                // it died, before anything is finished: every held cut's
                // map whole, mid-taking too, and its content.
                self.region.revive();
                let view =
                    CutState::view(self.region.clone(), SIZE, self.bound, OWNER, self.mutants)
                        .map_err(|(e, _)| e.to_string())?;
                self.check_cuts(&view)
                    .map_err(|e| format!("read at the death in {step:?}: {e}"))?;
                self.restart(step, &held)?;
                self.adopt();
                // The kernel issues the request again: a change is done
                // again whole, a cut's flush makes the cut if its freeze
                // was not counted, and an upload not let go is let go.
                // The start's sync completed every flush under way.
                let again = match step {
                    Step::Cut => self.state.count() < cut,
                    Step::Release => uploading
                        .is_some_and(|number| self.state.held().iter().any(|c| c.number == number)),
                    Step::Flushed | Step::Fail => false,
                    _ => true,
                };
                if again {
                    self.apply(step, value)?;
                }
                self.next_value += 1;
                self.adopt();
            }
        }
        self.check_holes()?;
        self.check_content()
    }

    /// The hole set holds while a cut may read by it, and is dropped
    /// once none can: a held cut has `lo` 0, or none is held and the
    /// next cut's `lo` is 0 (M2.4e). A world without holes has none.
    fn check_holes(&self) -> Result<(), String> {
        let held = self.state.held();
        let needed = held.iter().any(|c| c.lo == 0)
            || (held.is_empty() && self.state.load(at::OPEN_LO) == 0);
        let want = match (self.holes, needed) {
            (false, _) => Holes::None,
            (true, true) => Holes::Held,
            (true, false) => Holes::Dropped,
        };
        let figures = self.state.figures();
        if figures.holes != want {
            return Err(format!(
                "the hole set is {:?}, and {want:?} with {held:?} held",
                figures.holes
            ));
        }
        let chunks = if self.holes {
            HOLE_CHUNKS.len() as u64
        } else {
            0
        };
        if figures.hole_chunks != chunks {
            return Err(format!(
                "the hole set names {} chunks, not {chunks}",
                figures.hole_chunks
            ));
        }
        Ok(())
    }

    fn apply(&mut self, step: Step, value: u64) -> Result<(), String> {
        if let Step::Write(from, n) | Step::Zero(from, n) | Step::Discard(from, n) = step {
            for block in from..(from + n).min(BLOCKS) {
                self.touched[block as usize] = true;
            }
        }
        match step {
            Step::Write(from, n) | Step::Zero(from, n) => {
                if self.failed {
                    return Ok(());
                }
                let content = if matches!(step, Step::Zero(..)) {
                    0
                } else {
                    value
                };
                let end = (from + n).min(BLOCKS);
                let epoch = self.state.count() + 1;
                for block in from..end {
                    self.changes[block as usize].insert(epoch);
                }
                if self.mutants.has(Mutants::COPY_AFTER_OVERWRITE) {
                    self.overwrite(from, end, content);
                }
                self.state
                    .change(from, end - from, &mut self.disk)
                    .map_err(|e| e.to_string())?;
                self.overwrite(from, end, content);
                self.count_abandoned();
            }
            Step::Discard(from, n) => {
                if self.failed {
                    return Ok(());
                }
                // Not noted: a discard changes no content in the store.
                let end = (from + n).min(BLOCKS);
                for block in from..end {
                    self.discarded[block as usize] = true;
                }
            }
            Step::Cut => {
                if self.failed {
                    return Ok(());
                }
                let number = self.state.count() + 1;
                self.snaps
                    .insert(number, (self.disk.image.clone(), self.discarded.clone()));
                let got = self.state.freeze(&marked(number), true);
                if got != number {
                    return Err(format!("frozen as {got}, the count's next is {number}"));
                }
                self.rules.freeze(number);
                self.flushing.push_back(number);
            }
            Step::Flushed => {
                if let Some(&number) = self.flushing.front() {
                    self.state.flushed(number);
                    self.rules.flushed(number);
                    self.flushing.pop_front();
                }
            }
            Step::Fail => {
                if self.flushing.is_empty() || self.failed {
                    return Ok(());
                }
                // Every flush under way fails with the volume; no death
                // comes after it in these programs.
                self.region.revive();
                self.failed = true;
                while let Some(number) = self.flushing.pop_front() {
                    self.state.flush_failed(number);
                    self.rules.failed(number);
                }
            }
            Step::Release => {
                let Some(number) = self.rules.uploading() else {
                    return Ok(());
                };
                self.upload(number)?;
                if !self.state.release(number) {
                    return Err(format!("cut {number} is not the one being uploaded"));
                }
                self.rules.release();
            }
            Step::Mismatch => {
                if self.failed {
                    return Ok(());
                }
                let epoch = self.state.count() + 1;
                let abandoned = self.state.abandon_all();
                self.whole.insert(epoch);
                self.rules.mismatch();
                self.abandoned += abandoned.len() as u64;
                // A flush under way still completes; its cut is not held,
                // and the state passes it by.
                self.flushing.clear();
            }
        }
        Ok(())
    }

    fn overwrite(&mut self, from: u64, end: u64, content: u64) {
        for block in from..end {
            self.disk.image[block as usize] = content;
            self.discarded[block as usize] = false;
        }
    }

    fn count_abandoned(&mut self) {
        let abandoned = self.state.figures().abandoned;
        while self.abandoned < abandoned {
            self.rules.abandon();
            self.abandoned += 1;
        }
    }

    /// What cut `number` reads for `block`: `None` where nothing was
    /// uploaded.
    fn read(&self, number: u64, block: u64) -> Result<Option<u64>, String> {
        self.read_in(&self.state, number, block)
    }

    /// The same, read through `state`.
    fn read_in(
        &self,
        state: &CutState<MemRegion>,
        number: u64,
        block: u64,
    ) -> Result<Option<u64>, String> {
        let read = state
            .read(number, block)
            .ok_or_else(|| format!("cut {number} is not held"))?;
        Ok(match read {
            Read::Unchanged => self.uploaded[block as usize],
            Read::Zero => Some(0),
            Read::Image => Some(self.disk.image[block as usize]),
            Read::Copy(slot) => Some(*self.disk.copies.get(&slot).ok_or_else(|| {
                format!("cut {number}, block {block}: slot {slot} never written")
            })?),
        })
    }

    /// Upload cut `number` onto the image, as the uploader reads it: in
    /// slices of a few blocks ([`CutState::slice_from`]), each block
    /// read and each run of zeros written as zeros. The slices hold the
    /// cut's map, every block once and in order; a block named for a
    /// read does not read as a hole's zeros, and every block of a run
    /// does.
    fn upload(&mut self, number: u64) -> Result<(), String> {
        let mut covered = Vec::new();
        let mut from = 0;
        loop {
            let slice = self
                .state
                .slice_from(number, from, 7)
                .ok_or_else(|| format!("cut {number} is not held"))?;
            let mut pieces: Vec<(u64, u64, bool)> = slice
                .blocks
                .iter()
                .map(|&block| (block, 1, false))
                .collect();
            pieces.extend(slice.zeroes.iter().map(|&(first, n)| (first, n, true)));
            pieces.sort_unstable();
            for (first, n, zeros) in pieces {
                if n == 0 {
                    return Err(format!("cut {number}: an empty run at {first}"));
                }
                for block in first..first + n {
                    let read = self.state.read(number, block);
                    if zeros != (read == Some(Read::Zero)) {
                        return Err(format!(
                            "cut {number}, block {block}: in a run of zeros {zeros}, reads {read:?}"
                        ));
                    }
                    self.uploaded[block as usize] = self.read(number, block)?;
                    covered.push(block);
                }
            }
            match slice.next {
                Some(next) if next <= from => {
                    return Err(format!(
                        "cut {number}: the slice from {from} goes on from {next}"
                    ));
                }
                Some(next) => from = next,
                None => break,
            }
        }
        let map = self.state.map(number);
        if covered != map {
            return Err(format!(
                "cut {number}: its slices hold {covered:?}, and its map is {map:?}"
            ));
        }
        Ok(())
    }

    /// A new instance takes up the state after a death at a store in
    /// `step`, before which `held` were held.
    fn restart(&mut self, step: Step, held: &[HeldCut]) -> Result<(), String> {
        self.region.revive();
        let before = self.state.held();
        self.state = CutState::restore(self.region.clone(), SIZE, self.bound, OWNER, self.mutants)
            .map_err(|(e, _)| e.to_string())?;
        // Flushes under way are done by the start's sync.
        self.flushing.clear();
        let after = self.state.held();
        if after.iter().any(|c| c.phase == Phase::Flushing) {
            return Err(format!(
                "after a start a cut's flush is still under way: {after:?}"
            ));
        }
        if !after.is_empty() && !after.iter().any(|c| c.phase == Phase::Uploading) {
            return Err(format!("after a start no cut is being uploaded: {after:?}"));
        }
        // No cut is lost by a death but the one being uploaded, whose
        // abandoning or release the death interrupted; one that a newer
        // cut replaced, taking its map, when the start's sync completed
        // their flushes; one whose flush was under way, which a freeze
        // lets go; and at the bound, in a change, the oldest cuts. A cut
        // whose freeze the death interrupted is held whole or not
        // counted.
        for (i, cut) in before.iter().enumerate() {
            if after.iter().any(|c| c.number == cut.number) {
                continue;
            }
            let replaced = cut.phase != Phase::Uploading
                && after
                    .iter()
                    .any(|c| c.number > cut.number && c.lo <= cut.lo);
            let let_go = cut.phase == Phase::Uploading
                || (cut.phase == Phase::Flushing && step == Step::Cut)
                || step == Step::Mismatch;
            let at_bound = matches!(step, Step::Write(..) | Step::Zero(..))
                && before[..i]
                    .iter()
                    .all(|older| !after.iter().any(|c| c.number == older.number));
            if !(replaced || let_go || at_bound) {
                return Err(format!(
                    "a death at {step:?} lost cut {} ({:?}): held before {before:?}, after {after:?}",
                    cut.number, cut.phase
                ));
            }
        }
        // The cuts abandoned: in a change or a mismatch, the oldest cuts
        // held before it that are lost, each once; in any other step,
        // none. This
        // world's copier never refuses, so a change abandons only at the
        // bound, oldest first. A refused copy lets go the newest cuts
        // instead, which this check would have to know if the world
        // refused copies; its count across a death is held by
        // `a_death_while_a_refused_copy_lets_a_cut_go_counts_it_once`.
        let lost_oldest = held
            .iter()
            .take_while(|cut| !after.iter().any(|c| c.number == cut.number))
            .count() as u64;
        let abandoned = if matches!(step, Step::Write(..) | Step::Zero(..) | Step::Mismatch) {
            lost_oldest
        } else {
            0
        };
        let counted = self.state.figures().abandoned;
        if counted != self.abandoned + abandoned {
            return Err(format!(
                "a death at {step:?} left {counted} cuts abandoned, not {}: held before the step {held:?}, after {after:?}",
                self.abandoned + abandoned
            ));
        }
        let count = self.state.count();
        if count > self.snaps.keys().last().copied().unwrap_or(0) {
            return Err(format!("count {count} with no instant for it"));
        }
        if let Some(newest) = self.snaps.keys().last() {
            if count == *newest && !after.iter().any(|c| c.number == count) && step == Step::Cut {
                return Err(format!(
                    "cut {count} is counted and not held after a death in its freeze"
                ));
            }
        }
        Ok(())
    }

    /// After a death, the rules take the held cuts the restart left
    /// (checked in `restart`), and the step done again is not checked
    /// against them.
    fn adopt(&mut self) {
        self.rules.held = self
            .state
            .held()
            .iter()
            .map(|c| (c.number, c.lo, c.phase))
            .collect();
        self.rules.open_lo = self.state.load(at::OPEN_LO);
        self.flushing = self
            .state
            .held()
            .iter()
            .filter(|c| c.phase == Phase::Flushing)
            .map(|c| c.number)
            .collect();
        self.abandoned = self.state.figures().abandoned;
    }

    fn check_rules(&self, step: Step) -> Result<(), String> {
        let held: Vec<(u64, u64, Phase)> = self
            .state
            .held()
            .iter()
            .map(|c| (c.number, c.lo, c.phase))
            .collect();
        if held != self.rules.held {
            return Err(format!(
                "after {step:?} the state holds {held:?}, and the rules {:?}",
                self.rules.held
            ));
        }
        Ok(())
    }

    /// Every held cut, uploaded onto the image that the held cuts
    /// before it make, is the volume at its instant, and carries the
    /// state that marked it.
    fn check_content(&self) -> Result<(), String> {
        self.check_cuts(&self.state)
    }

    /// The blocks written or zeroed in the epochs `(lo, number]`, or
    /// every block if the whole volume counts as changed in them.
    fn model_map(&self, lo: u64, number: u64) -> Vec<u64> {
        if self.whole.range(lo + 1..=number).next().is_some() {
            return (0..BLOCKS).collect();
        }
        (0..BLOCKS)
            .filter(|&block| {
                self.changes[block as usize]
                    .range(lo + 1..=number)
                    .next()
                    .is_some()
            })
            .collect()
    }

    /// The same check of the held cuts of `state`, which may be a view
    /// of the region at a death: each held cut's map is the blocks
    /// changed in its epochs, and the cut reads as the volume was at its
    /// instant.
    fn check_cuts(&self, state: &CutState<MemRegion>) -> Result<(), String> {
        let mut base = self.uploaded.clone();
        for cut in state.held() {
            let map = state.map(cut.number);
            let want = self.model_map(cut.lo, cut.number);
            if map != want {
                return Err(format!(
                    "cut {} ({:?}, lo {}): its map is {map:?}, and the blocks changed in its \
                     epochs are {want:?}",
                    cut.number, cut.phase, cut.lo
                ));
            }
            if cut.state != marked(cut.number) {
                return Err(format!(
                    "cut {} carries group {}",
                    cut.number, cut.state.txg
                ));
            }
            let Some((volume, discarded)) = self.snaps.get(&cut.number) else {
                return Err(format!("cut {} has no instant", cut.number));
            };
            for block in 0..BLOCKS {
                if discarded[block as usize] || !self.touched[block as usize] {
                    continue;
                }
                let read = match state.read(cut.number, block) {
                    Some(Read::Unchanged) => base[block as usize],
                    _ => self.read_in(state, cut.number, block)?,
                };
                let want = volume[block as usize];
                if read != Some(want) {
                    return Err(format!(
                        "cut {} ({:?}, lo {}), block {block}: reads {read:?}, the volume held {want} \
                         at its instant; held {:?}",
                        cut.number,
                        cut.phase,
                        cut.lo,
                        state.held()
                    ));
                }
            }
            for block in map {
                base[block as usize] = self.read_in(state, cut.number, block)?;
            }
        }
        Ok(())
    }
}

/// A death at a store panics with [`Lost`], which the world catches: the
/// panic hook says nothing of it, and passes every other panic on.
pub(crate) fn quiet_deaths() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<Lost>().is_none() {
                default(info);
            }
        }));
    });
}

/// A program of `len` steps from `seed`.
fn program(seed: u64, len: usize) -> Vec<Step> {
    let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    (0..len)
        .map(|_| {
            let roll = next() % 100;
            let from = STARTS[(next() % STARTS.len() as u64) as usize];
            let n = 1 + next() % 3;
            match roll {
                0..=34 => Step::Write(from, n),
                35..=39 => Step::Zero(from, n),
                40..=49 => Step::Discard(from, n),
                50..=67 => Step::Cut,
                68..=83 => Step::Flushed,
                84..=85 => Step::Fail,
                _ => Step::Release,
            }
        })
        .collect()
}

/// A program of `len` steps from `seed` as [`program`] makes it, with
/// about one step in twelve a mismatch in its place (the review of
/// #125, G1): every held cut let go while the hole set may hold.
fn program_with_mismatches(seed: u64, len: usize) -> Vec<Step> {
    let mut x = seed.wrapping_mul(0xd1b5_4a32_d192_ed03) | 1;
    program(seed, len)
        .into_iter()
        .map(|step| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            if x.is_multiple_of(12) {
                Step::Mismatch
            } else {
                step
            }
        })
        .collect()
}

/// Run `steps` with a death at store `death` (counted from the
/// program's first step), if one is given.
fn run(
    steps: &[Step],
    mutants: u128,
    bound_blocks: u64,
    death: Option<u64>,
) -> Result<World, String> {
    run_in(false, steps, mutants, bound_blocks, death)
}

/// The same in a world with holes, or not.
fn run_in(
    holes: bool,
    steps: &[Step],
    mutants: u128,
    bound_blocks: u64,
    death: Option<u64>,
) -> Result<World, String> {
    let mut world = World::begun(mutants, bound_blocks, holes);
    let base = world.region.stores();
    if let Some(death) = death {
        world.region.die_at(death);
    }
    for (i, step) in steps.iter().enumerate() {
        world
            .step(*step)
            .map_err(|e| format!("step {i} ({step:?}): {e}"))?;
    }
    let _ = base;
    Ok(world)
}

/// The stores a run of `steps` makes with no death.
fn stores_of(steps: &[Step], mutants: u128, bound_blocks: u64) -> u64 {
    stores_in(false, steps, mutants, bound_blocks)
}

/// The same in a world with holes, or not.
fn stores_in(holes: bool, steps: &[Step], mutants: u128, bound_blocks: u64) -> u64 {
    let world = World::begun(mutants, bound_blocks, holes);
    let base = world.region.stores();
    drop(world);
    match run_in(holes, steps, mutants, bound_blocks, None) {
        Ok(world) => world.region.stores() - base,
        Err(_) => 0,
    }
}

/// Whether some seed below `seeds`, with no death or with one at some
/// store, fails under `mutants`.
fn caught(mutants: u128, seeds: u64, bound_blocks: u64, deaths: bool) -> Option<String> {
    for seed in 0..seeds {
        let steps = program(seed, 60);
        if let Err(e) = run(&steps, mutants, bound_blocks, None) {
            return Some(format!("seed {seed}: {e}"));
        }
        if deaths {
            let stores = stores_of(&steps, 0, bound_blocks);
            for death in 0..stores {
                if let Err(e) = run(&steps, mutants, bound_blocks, Some(death)) {
                    return Some(format!("seed {seed}, death at store {death}: {e}"));
                }
            }
        }
    }
    None
}

/// Over random programs, with the bound at 2 blocks (reached often) and
/// at 64 (never), every held cut reads as the volume at its instant, and
/// the held cuts are those the rules hold.
#[test]
fn every_held_cut_reads_as_the_volume_at_its_instant() {
    for holes in [false, true] {
        for bound in [2, 64] {
            for seed in 0..300 {
                let steps = program(seed, 80);
                if let Err(e) = run_in(holes, &steps, 0, bound, None) {
                    panic!("holes {holes}, bound {bound}, seed {seed}: {e}");
                }
            }
        }
    }
}

/// Programs of `seeds` seeds with the daemon lost at every store of the
/// region, each in a run of its own: the next instance takes up the
/// state, and does again the step the death interrupted. With and
/// without the image's holes (M2.4e). The runs.
fn deaths_at_every_store(seeds: std::ops::Range<u64>) -> u64 {
    let mut runs = 0;
    for holes in [false, true] {
        for bound in [2, 64] {
            for seed in seeds.clone() {
                let steps = program(seed, 50);
                let stores = stores_in(holes, &steps, 0, bound);
                for death in 0..stores {
                    runs += 1;
                    if let Err(e) = run_in(holes, &steps, 0, bound, Some(death)) {
                        panic!(
                            "holes {holes}, bound {bound}, seed {seed}, death at store {death}: {e}"
                        );
                    }
                }
            }
        }
    }
    runs
}

/// The same with the daemon lost at every store of the region: six
/// programs for each bound, every store of each.
#[test]
fn a_death_at_any_store_leaves_every_held_cut_whole() {
    let runs = deaths_at_every_store(0..6);
    assert!(runs > 1000, "{runs} runs");
}

/// The same over forty programs for each bound (some two minutes in a
/// debug build): run by hand, `--ignored`.
#[test]
#[ignore]
fn a_death_at_any_store_leaves_every_held_cut_whole_over_forty_programs() {
    deaths_at_every_store(0..40);
}

/// The first cut of a new state holds the whole volume; uploaded, it
/// leaves the cuts after it their own changes.
const UPLOADED_FIRST: [Step; 3] = [Step::Cut, Step::Flushed, Step::Release];

/// A program of fixed steps for the defects that random programs reach
/// rarely: the review's case of the bound (design §5.2), with a bound of
/// two blocks. Blocks 0 and 2 change for cut 2, which is being uploaded;
/// blocks 1 and 2 for cut 3, which waits, so that block 2 is copied for
/// cut 2. Then block 0 is written: a copy for cut 2 alone, since block 0
/// did not change between the two instants, and the copies are at the
/// bound. Then block 1, which cut 3 needs copied: cut 2 is abandoned, and
/// cut 3 takes its map and with it the copy of block 0, which it now
/// reads; the copy of block 2 is freed, being of a block in cut 3's own
/// map, and that leaves room under the bound for block 1's.
fn the_reviews_case() -> Vec<Step> {
    let mut steps = UPLOADED_FIRST.to_vec();
    steps.extend([
        Step::Write(0, 1),
        Step::Write(2, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(1, 1),
        Step::Write(2, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(0, 1),
        Step::Write(1, 1),
    ]);
    steps
}

/// A copy made while the newer cut's flush runs, of a block outside that
/// cut's own map that did not change between the two instants, is the
/// newer cut's content too, and stays when the newer cut replaces the
/// one that waits. Block 1 changes for cut 3, which waits; while cut 4's
/// flush is under way block 1 is written, a copy for cut 3; then cut 4's
/// flush completes, and cut 4 takes cut 3's map and reads that copy.
fn a_copy_the_replacing_cut_needs() -> Vec<Step> {
    let mut steps = UPLOADED_FIRST.to_vec();
    steps.extend([
        Step::Write(0, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(2, 1),
        Step::Cut,
        Step::Write(1, 1),
        Step::Flushed,
    ]);
    steps
}

/// A cut that replaces the one that waits takes a map of three words
/// of bits into its own of four: the move passes over several words,
/// each with a store into the larger map and one into the smaller, and
/// then the cut that replaced reads copies of blocks it took.
fn a_taking_over_several_words() -> Vec<Step> {
    let mut steps = UPLOADED_FIRST.to_vec();
    steps.extend([
        Step::Write(0, 1),
        Step::Write(64, 1),
        Step::Write(128, 1),
        Step::Write(192, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(1, 1),
        Step::Write(65, 1),
        Step::Write(129, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(2, 1),
        Step::Write(66, 1),
        Step::Write(130, 1),
        Step::Write(193, 1),
        Step::Cut,
        Step::Flushed,
        Step::Release,
        Step::Write(1, 1),
        Step::Write(129, 1),
    ]);
    steps
}

/// Never a fourth cut (design §5.2): cut 4's flush is under way when cut
/// 5's instant comes, so cut 5 takes cut 4's map and cut 4 is no longer
/// held; cut 3 waits, untouched. Then the flushes fail: `first_fails`,
/// cut 4's fails, and with the volume cut 5's; otherwise cut 4's
/// completes and cut 5's fails. Either way cut 3 is the newest good one.
fn a_flush_fails_after_a_fourth_instant(first_fails: bool) -> Vec<Step> {
    let mut steps = UPLOADED_FIRST.to_vec();
    steps.extend([
        Step::Write(0, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(2, 1),
        Step::Cut,
        Step::Write(1, 1),
        Step::Write(3, 1),
        Step::Cut,
        Step::Write(0, 1),
    ]);
    if !first_fails {
        steps.push(Step::Flushed);
    }
    steps.push(Step::Fail);
    steps
}

/// The fixed programs, each with its bound in blocks.
fn fixed_programs() -> [(&'static str, Vec<Step>, u64); 5] {
    [
        ("the review's case", the_reviews_case(), 2),
        (
            "a copy the replacing cut needs",
            a_copy_the_replacing_cut_needs(),
            64,
        ),
        (
            "a taking over several words",
            a_taking_over_several_words(),
            64,
        ),
        (
            "the first flush fails after a fourth instant",
            a_flush_fails_after_a_fourth_instant(true),
            64,
        ),
        (
            "the second flush fails after a fourth instant",
            a_flush_fails_after_a_fourth_instant(false),
            64,
        ),
    ]
}

#[test]
fn the_rules_of_the_bound_and_of_replacing_hold_on_fixed_programs() {
    for (name, steps, bound) in fixed_programs() {
        let stores = stores_of(&steps, 0, bound);
        assert!(stores > 0, "{name}: no run");
        for death in 0..stores {
            if let Err(e) = run(&steps, 0, bound, Some(death)) {
                panic!("{name}, death at {death}: {e}");
            }
        }
    }
    let world = run(&the_reviews_case(), 0, 2, None).expect("runs");
    assert_eq!(
        world.state.figures().abandoned,
        1,
        "cut 2 is abandoned at the bound"
    );
    assert_eq!(
        world
            .state
            .held()
            .iter()
            .map(|c| (c.number, c.lo, c.phase))
            .collect::<Vec<_>>(),
        vec![(3, 1, Phase::Uploading)]
    );
    assert!(matches!(world.state.read(3, 0), Some(Read::Copy(_))));
    assert!(matches!(world.state.read(3, 1), Some(Read::Copy(_))));
    assert_eq!(world.state.figures().copied, 2 * BLOCK_SIZE);
    let world = run(&a_copy_the_replacing_cut_needs(), 0, 64, None).expect("runs");
    assert_eq!(
        world
            .state
            .held()
            .iter()
            .map(|c| (c.number, c.lo, c.phase))
            .collect::<Vec<_>>(),
        vec![(2, 1, Phase::Uploading), (4, 2, Phase::Waiting)]
    );
    assert!(matches!(world.state.read(4, 1), Some(Read::Copy(_))));
    let world = run(&a_taking_over_several_words(), 0, 64, None).expect("runs");
    assert_eq!(
        world.state.map(4),
        vec![1, 2, 65, 66, 129, 130, 193],
        "cut 4's map, its own and cut 3's"
    );
    assert!(matches!(world.state.read(4, 129), Some(Read::Copy(_))));
    for first_fails in [true, false] {
        let world = run(
            &a_flush_fails_after_a_fourth_instant(first_fails),
            0,
            64,
            None,
        )
        .expect("runs");
        assert_eq!(
            world
                .state
                .held()
                .iter()
                .map(|c| (c.number, c.lo, c.phase))
                .collect::<Vec<_>>(),
            vec![(2, 1, Phase::Uploading), (3, 2, Phase::Waiting)],
            "first fails: {first_fails}"
        );
        assert!(matches!(world.state.read(3, 1), Some(Read::Copy(_))));
    }
}

/// Each defect of the cut state is noticed: by the random programs, by
/// a death at some store, or by the fixed programs.
#[test]
fn the_tests_notice_each_defect_of_the_cut_state() {
    let fixed = |mutants: u128| -> Option<String> {
        for (name, steps, bound) in fixed_programs() {
            if let Err(e) = run(&steps, mutants, bound, None) {
                return Some(format!("{name}: {e}"));
            }
        }
        None
    };
    for (name, mutant, deaths) in [
        ("no record of a copy", Mutants::COPY_UNRECORDED, true),
        (
            "a copy made after the overwrite",
            Mutants::COPY_AFTER_OVERWRITE,
            false,
        ),
        (
            "an abandoned map dropped",
            Mutants::ABANDONED_MAP_DROPPED,
            false,
        ),
        (
            "a replaced cut's map dropped",
            Mutants::REPLACED_MAP_DROPPED,
            false,
        ),
        (
            "the abandoned cut's copies all freed",
            Mutants::ABANDONED_COPIES_FREED,
            false,
        ),
        (
            "the replaced cut's copies all freed",
            Mutants::REPLACED_COPIES_FREED,
            false,
        ),
        (
            "an abandoned map given to a flushing cut while one waits",
            Mutants::ABANDONED_TO_FLUSHING,
            false,
        ),
        (
            "an abandoned map put in the open map while a flush is under way",
            Mutants::ABANDONED_TO_OPEN,
            false,
        ),
        (
            "the waiting cut replaced at the newer one's instant",
            Mutants::REPLACED_AT_INSTANT,
            false,
        ),
        (
            "a freeze that a death can split",
            Mutants::FREEZE_SPLIT,
            true,
        ),
        (
            "a map's summary noted after the block's bit",
            Mutants::SUMMARY_AFTER_BIT,
            true,
        ),
        (
            "a map moved with each word cleared before it is set",
            Mutants::MOVE_CLEARS_FIRST,
            true,
        ),
        (
            "a map let go and not cleared",
            Mutants::MAP_NOT_CLEARED,
            false,
        ),
    ] {
        let seen = fixed(mutant)
            .or_else(|| caught(mutant, 200, 2, false))
            .or_else(|| deaths.then(|| caught(mutant, 20, 2, true)).flatten());
        assert!(seen.is_some(), "{name}: not noticed");
    }
}

/// A region of another version is refused, and one with no magic, one
/// of another volume's size or identity, and one begun in another boot,
/// each unless its check is planted away; the region comes back with
/// the reason.
#[test]
fn a_region_of_another_version_volume_or_boot_is_refused() {
    let bound = 2 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let refused = |size: u64, owner: Owner, mutants: u128| {
        CutState::restore(region.clone(), size, bound, owner, Mutants(mutants))
            .err()
            .map(|(e, _)| e)
    };
    assert_eq!(refused(SIZE, OWNER, 0), Some(RegionError::Empty));
    let _ = CutState::begin(region.clone(), SIZE, bound, OWNER, Mutants(0));
    assert_eq!(refused(SIZE, OWNER, 0), None);
    assert!(matches!(
        refused(SIZE * 2, OWNER, 0),
        Some(RegionError::Shape(_))
    ));
    let mut another_boot = OWNER;
    another_boot.boot[15] ^= 1;
    assert_eq!(refused(SIZE, another_boot, 0), Some(RegionError::Boot));
    assert_eq!(refused(SIZE, another_boot, Mutants::BOOT_UNCHECKED), None);
    let mut another_volume = OWNER;
    another_volume.volume[0] ^= 1;
    assert_eq!(
        refused(SIZE, another_volume, 0),
        Some(RegionError::Identity)
    );
    assert_eq!(
        refused(SIZE, another_volume, Mutants::IDENTITY_UNCHECKED),
        None
    );
    let mut other = region.clone();
    // Version 1 kept no state id (M2.4): it is read no more, and its
    // volume counts as changed once.
    other.store(at::VERSION, 1);
    assert_eq!(refused(SIZE, OWNER, 0), Some(RegionError::Version(1)));
    other.store(at::VERSION, VERSION + 1);
    assert_eq!(
        refused(SIZE, OWNER, 0),
        Some(RegionError::Version(VERSION + 1))
    );
    let Err((_, back)) = CutState::restore(other.clone(), SIZE, bound, OWNER, Mutants(0)) else {
        panic!("a region of another version was taken up");
    };
    assert_eq!(back.load(at::VERSION), VERSION + 1);
}

/// A state is named by the start that began it, and keeps that name
/// when a later start takes it up under its own token; a state begun
/// anew takes its own start's. Numbers of cuts begin again with every
/// state, so the name and a number say which cut an upload holds
/// (M2.4).
#[test]
fn a_state_keeps_the_name_of_the_start_that_began_it() {
    let bound = 2 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let state = CutState::begin(region, SIZE, bound, OWNER, Mutants(0));
    assert_eq!(state.id(), OWNER.start);
    let later = Owner {
        start: *b"a later start...",
        ..OWNER
    };
    let Ok(state) = CutState::view(state.into_region(), SIZE, bound, OWNER, Mutants(0)) else {
        panic!("the begun state was not read");
    };
    let state = state.take_up(later.start);
    assert_eq!(state.id(), OWNER.start, "taken up under a later token");
    let Ok(state) = CutState::view(state.into_region(), SIZE, bound, later, Mutants(0)) else {
        panic!("the state was not read under the token that took it up");
    };
    assert_eq!(state.id(), OWNER.start, "read under the later token");
    let state = CutState::begin(state.into_region(), SIZE, bound, later, Mutants(0));
    assert_eq!(state.id(), later.start, "begun anew");
}

/// A new state counts the whole volume as changed: its first cut's map
/// is every block, and reads the volume as it is.
#[test]
fn a_new_states_first_cut_holds_the_whole_volume() {
    let steps = [Step::Write(3, 1), Step::Cut, Step::Flushed];
    let world = run(&steps, 0, 64, None).expect("runs");
    assert_eq!(world.state.map(1), (0..BLOCKS).collect::<Vec<_>>());
    let world = run(
        &[
            Step::Write(3, 1),
            Step::Cut,
            Step::Flushed,
            Step::Release,
            Step::Write(5, 1),
            Step::Cut,
        ],
        0,
        64,
        None,
    )
    .expect("runs");
    assert_eq!(
        world.state.map(2),
        vec![5],
        "the second cut's map is its own changes"
    );
}

/// The locked region for a TiB at 4 KiB blocks with the default bound:
/// 130 MiB of maps and their summaries, 16 MiB of records, 2 MiB of the
/// hole set (a bit for 64 KiB, M2.4e), and a header. A state of version
/// 2 has no hole set.
#[test]
fn the_state_of_a_tebibyte_takes_148_mib() {
    const MIB: u64 = 1 << 20;
    let size = 1u64 << 40;
    let blocks = size / BLOCK_SIZE;
    let bound = default_bound(size);
    assert_eq!(bound, 4 << 30);
    let maps = words_of(2, blocks, 0) as u64 * 8;
    let old = words_of(2, blocks, records_for(bound)) as u64 * 8;
    let all = words_for(blocks, records_for(bound)) as u64 * 8;
    assert_eq!(maps / MIB, 130, "maps: {maps} bytes");
    assert_eq!((old - maps) / MIB, 16, "records: {} bytes", old - maps);
    assert_eq!(all - old, 2 * MIB + 16, "the hole set: {} bytes", all - old);
    assert!(all - 148 * MIB < 4096, "header: {} bytes", all - 148 * MIB);
}

/// A copy record's epochs are stored before its block. A free slot
/// keeps the epochs of its last use, here [2,4), which hold cut 3; the
/// slot is taken again for block 1, of which cut 3 has its own copy in
/// a lower slot. With the block stored first, a death between the two
/// words leaves a record of block 1 for [2,4), and cut 3 reads the newer
/// copy. A death at each store of the 17 steps (the review of #98, S4).
#[test]
fn a_death_between_a_records_two_words() {
    let mut steps = UPLOADED_FIRST.to_vec();
    steps.extend([
        // Cut 2 (being uploaded) changes blocks 0 and 1.
        Step::Write(0, 1),
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        // Cut 3 (waits) changes block 1 alone: slot 0 = (1, [2,3)).
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        // Epoch 4: slot 1 = (1, [3,4)), cut 3's own copy; slot 2 = (0, [2,4)), cut 2's.
        Step::Write(1, 1),
        Step::Write(0, 1),
        // Cut 2 goes: slots 2 and 0 are freed, slot 2 with epochs that hold cut 3.
        Step::Release,
        // Cut 4 (waits) holds blocks 0 and 1.
        Step::Cut,
        Step::Flushed,
        // Epoch 5: block 0's copy takes slot 0, block 1's takes slot 2.
        Step::Write(0, 1),
        Step::Write(1, 1),
    ]);
    let stores = stores_of(&steps, 0, 64);
    assert!(stores > 0);
    for death in 0..stores {
        if let Err(e) = run(&steps, 0, 64, Some(death)) {
            panic!("death at store {death} of {stores}: {e}");
        }
    }
}

/// A cut recorded and not counted is none, whatever comes before the
/// freeze is done again: here a death in cut 3's freeze after the first
/// word of its record, then cut 1 let go before the flush comes again,
/// which frees a lower slot. Where the start leaves the stale record in
/// its slot, the freeze done again takes the lower slot and the count's
/// store makes both records held cuts (the review of #98, S3).
#[test]
fn a_cut_recorded_and_not_counted_is_none() {
    quiet_deaths();
    let bound = 64 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let mut state = CutState::begin(region.clone(), SIZE, bound, OWNER, Mutants(0));
    let mut disk = Disk {
        image: vec![0; BLOCKS as usize],
        copies: BTreeMap::new(),
    };
    state.change(0, 1, &mut disk).unwrap();
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    state.change(1, 1, &mut disk).unwrap();
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    state.change(2, 1, &mut disk).unwrap();
    // Lost in cut 3's freeze, after the first word of its record.
    region.die_at(1);
    let lost = catch_unwind(AssertUnwindSafe(|| state.freeze(&marked(3), true)));
    assert!(lost.is_err());
    region.revive();
    let mut state = CutState::restore(region.clone(), SIZE, bound, OWNER, Mutants(0)).unwrap();
    // Cut 1 is uploaded before the flush comes again.
    assert!(state.release(1));
    state.change(3, 1, &mut disk).unwrap();
    assert_eq!(state.freeze(&marked(3), true), 3);
    let held: Vec<(u64, Phase)> = state.held().iter().map(|c| (c.number, c.phase)).collect();
    assert_eq!(held, [(2, Phase::Uploading), (3, Phase::Flushing)]);
    assert_eq!(state.map(3), [2, 3]);
}

/// a disk that refuses copies while told to.
struct Refusing {
    disk: Disk,
    refuse: bool,
}

impl Copier for Refusing {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        self.disk.read_image(first, buf)
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        if self.refuse {
            return Err(io::Error::other("no space left"));
        }
        self.disk.write_copies(first, buf)
    }
}

const REFUSING_BOUND: u64 = 64 * BLOCK_SIZE;

/// cut 1 was let go as uploaded; cut 2 is being uploaded (block
/// 20), cut 3 waits (blocks 30 to 32), cut 4's flush is under way
/// (block 40), and block 50 is in the open map.
fn three_held(region: &MemRegion) -> (CutState<MemRegion>, Refusing) {
    let mut state = CutState::begin(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0));
    let mut io = Refusing {
        disk: Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        },
        refuse: false,
    };
    state.change(10, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    assert!(state.release(1));
    state.change(20, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    state.change(30, 3, &mut io).expect("the change");
    // Two more blocks in other words of the map, so that cut 3's map
    // weighs more than the open map that takes it.
    state.change(100, 1, &mut io).expect("the change");
    state.change(170, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(3), true), 3);
    state.flushed(3);
    state.change(40, 1, &mut io).expect("the change");
    assert_eq!(state.freeze(&marked(4), true), 4);
    state.change(50, 1, &mut io).expect("the change");
    let held: Vec<(u64, Phase)> = state.held().iter().map(|c| (c.number, c.phase)).collect();
    assert_eq!(
        held,
        [
            (2, Phase::Uploading),
            (3, Phase::Waiting),
            (4, Phase::Flushing)
        ]
    );
    (state, io)
}

#[test]
fn a_refused_copy_lets_go_the_cut_that_reads_it_and_every_newer_one() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let (mut state, mut io) = three_held(&region);
    io.refuse = true;
    // Block 32 is in cut 3's map; block 33 has not changed before.
    state.change(32, 2, &mut io).expect("the change goes on");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert_eq!(
        held,
        [2],
        "cut 3 read block 32, and cut 4 would once it took cut 3's map"
    );
    io.refuse = false;
    let five = state.freeze(&marked(5), true);
    let map = state.map(five);
    for block in [30, 31, 32, 33, 40, 50, 100, 170] {
        assert!(
            map.contains(&block),
            "cut {five} lacks block {block}: {map:?}"
        );
    }
}

#[test]
fn a_death_while_a_refused_copy_lets_cuts_go() {
    quiet_deaths();
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
        let (mut state, mut io) = three_held(&region);
        io.refuse = true;
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.change(32, 2, &mut io)));
        region.revive();
        match done {
            Ok(_) => break,
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let mut state = CutState::restore(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state");
        let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        // The map never skips a cut.
        if held.contains(&4) && !held.contains(&3) {
            let map = state.map(4);
            assert!(
                map.contains(&30),
                "a death at store {death}: cut 4 is held without cut 3 and lacks its blocks: held {held:?}, cut 4's map {map:?}"
            );
        }
        // The write comes again, with copies still refused.
        state.change(32, 2, &mut io).expect("the change goes on");
        let next = state.freeze(&marked(9), true);
        let mut all = BTreeSet::new();
        for cut in state.held() {
            if cut.number > 2 {
                all.extend(state.map(cut.number));
            }
        }
        for block in [30, 31, 32, 33, 40, 50, 100, 170] {
            assert!(
                all.contains(&block),
                "a death at store {death}: block {block} is in no cut after cut 2 (the next is {next}): {all:?}"
            );
        }
    }
    assert!(deaths > 0);
}

#[test]
fn a_refused_copy_for_the_oldest_cut_leaves_the_next_cut_its_lo() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let (mut state, mut io) = three_held(&region);
    io.refuse = true;
    // Block 20 is in cut 2's map alone: cut 2 reads it, and every cut goes.
    state.change(20, 1, &mut io).expect("the change goes on");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert!(held.is_empty(), "{held:?}");
    io.refuse = false;
    let five = state.freeze(&marked(5), true);
    let cut = state.held()[0];
    let map = state.map(five);
    assert_eq!(
        cut.lo, 1,
        "cut {five} holds the changes since cut 1, the last let go as uploaded"
    );
    for block in [20, 30, 31, 32, 40, 50, 100, 170] {
        assert!(
            map.contains(&block),
            "cut {five} lacks block {block}: {map:?}"
        );
    }
}

/// A refused copy in a write over several blocks gives back all the
/// room of the cuts that went: their copies, and the slot of the copy
/// that failed. Cut 3 has a copy (block 31 changes again after cut 4's
/// instant); the write over blocks 30 to 33 is refused at block 30,
/// which cut 3's map holds with no copy yet. Cuts 3 and 4 go, each
/// counted once, no byte stays copied, the next cut holds every block
/// the cuts that went had counted, and the copies made for it count
/// from nothing. From the review of #100 (`refused_copy_figures.py`):
/// with the copies of the cuts that went kept until a cut is next let
/// go, or with the failed copy's slot not given back, the copied bytes
/// stay above 0.
#[test]
fn a_refused_copy_gives_back_the_room_of_the_cuts_that_went() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let (mut state, mut io) = three_held(&region);
    state.change(31, 1, &mut io).expect("the change");
    let before = state.figures();
    assert!(before.copied > 0, "cut 3 holds a copy of block 31");
    io.refuse = true;
    state.change(30, 4, &mut io).expect("the write goes on");
    io.refuse = false;
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    let after = state.figures();
    assert_eq!(held, [2]);
    assert_eq!(
        after.abandoned,
        before.abandoned + 2,
        "cuts 3 and 4 went, once each"
    );
    assert_eq!(
        after.copied, 0,
        "cut 2 has no copy, and the cuts that had one are gone"
    );
    let five = state.freeze(&marked(5), true);
    let map = state.map(five);
    for block in [30, 31, 32, 33, 40, 50, 100, 170] {
        assert!(
            map.contains(&block),
            "cut {five} lacks block {block}: {map:?}"
        );
    }
    state.flushed(five);
    let mut made = 0;
    for block in [30, 31, 32, 33, 40, 50, 100, 170] {
        state.change(block, 1, &mut io).expect("the change");
        made += 1;
    }
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert_eq!(held, [2, five]);
    assert_eq!(state.figures().copied, made * BLOCK_SIZE);
}

const BEGIN_BOUND: u64 = 64 * BLOCK_SIZE;

/// A daemon lost at any store of [`CutState::begin`] leaves no state
/// that the next start takes up with a block missing: either the state
/// is not read, or its first cut counts the whole volume as changed, as
/// a state begun does. Over an empty region, and over one that holds
/// another boot's state with a cut held; the next start is the same
/// owner, with the same token, which is the most a start can share
/// with the one lost. From the review of #100 (`begin_death.py`): with
/// the magic stored first, a death before the word that counts the
/// whole volume leaves a state whose first cut counts no block.
#[test]
fn a_daemon_lost_inside_begin_leaves_no_state_half_begun() {
    quiet_deaths();
    let other = Owner {
        boot: [0x77; 16],
        ..OWNER
    };
    for held_before in [false, true] {
        let mut deaths = 0;
        for death in 0u64.. {
            let region = MemRegion::new(words_for(BLOCKS, records_for(BEGIN_BOUND)));
            if held_before {
                let mut old = CutState::begin(region.clone(), SIZE, BEGIN_BOUND, other, Mutants(0));
                let n = old.freeze(&marked(1), true);
                old.flushed(n);
            }
            region.die_at(death);
            let done = catch_unwind(AssertUnwindSafe(|| {
                drop(CutState::begin(
                    region.clone(),
                    SIZE,
                    BEGIN_BOUND,
                    OWNER,
                    Mutants(0),
                ));
            }));
            region.revive();
            match done {
                Ok(()) => break,
                Err(payload) => {
                    if payload.downcast_ref::<Lost>().is_none() {
                        std::panic::resume_unwind(payload);
                    }
                }
            }
            deaths += 1;
            let Ok(mut state) =
                CutState::restore(region.clone(), SIZE, BEGIN_BOUND, OWNER, Mutants(0))
            else {
                continue;
            };
            let first = state.freeze(&marked(2), true);
            let counted = state.map(first).len() as u64;
            assert_eq!(
                counted, BLOCKS,
                "a death at store {death} of begin (a state held before: {held_before}): the next \
                 start reads the state, and its first cut counts {counted} of {BLOCKS} blocks"
            );
        }
        assert!(deaths > 0);
    }
}

/// A disk that refuses the first copy asked of it, and makes the rest.
struct RefusingFirst {
    disk: Disk,
    refused: bool,
}

impl Copier for RefusingFirst {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        self.disk.read_image(first, buf)
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        if !self.refused {
            self.refused = true;
            return Err(io::Error::other("no space left"));
        }
        self.disk.write_copies(first, buf)
    }
}

/// Once a refused copy has let every cut go, the rest of the write
/// copies nothing: the held cuts are read again after the cuts went.
/// The first cut of a state counts the whole volume as changed, so it
/// reads every block in no map, and a refused copy of such a block
/// lets every cut go. From the review of #100 (its plant A9, which
/// none of its tests noticed): with the held cuts not read again, the
/// write's other blocks are copied for a cut that is gone.
#[test]
fn a_refused_copy_that_lets_every_cut_go_copies_nothing_more() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let mut state = CutState::begin(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0));
    let mut io = RefusingFirst {
        disk: Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        },
        refused: false,
    };
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    state.change(5, 3, &mut io).expect("the write goes on");
    assert!(io.refused, "block 5's copy was asked and refused");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert!(held.is_empty(), "{held:?}");
    assert_eq!(
        state.figures().copied,
        0,
        "blocks 6 and 7 were copied for a cut that is gone"
    );
    assert!(io.disk.copies.is_empty(), "{:?}", io.disk.copies.keys());
}

/// Calls that each moved a run, as `(first, blocks)`.
type Runs<T> = Vec<(T, usize)>;

/// A disk that counts the reads of the image and the writes of the
/// copies.
struct Counting {
    disk: Disk,
    reads: Runs<u64>,
    writes: Runs<u32>,
}

impl Counting {
    fn new(blocks: u64) -> Self {
        Counting {
            disk: Disk {
                image: (0..blocks).map(|b| 1000 + b).collect(),
                copies: BTreeMap::new(),
            },
            reads: Vec::new(),
            writes: Vec::new(),
        }
    }

    /// The I/O since the last call, and none after it.
    fn taken(&mut self) -> (Runs<u64>, Runs<u32>) {
        (
            std::mem::take(&mut self.reads),
            std::mem::take(&mut self.writes),
        )
    }
}

impl Copier for Counting {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        self.reads.push((first, buf.len() / BLOCK_SIZE as usize));
        self.disk.read_image(first, buf)
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        self.writes.push((first, buf.len() / BLOCK_SIZE as usize));
        self.disk.write_copies(first, buf)
    }
}

/// A change copies the blocks a held cut reads in runs (M2.2d): one read
/// of the image and one write of the copies file for each run of blocks
/// that are consecutive and go to consecutive slots, not one of each a
/// block (M2.2c). Cut 1 counts the whole volume (the state is new), so
/// it reads every block.
///
/// - A change of 32 blocks is one run.
/// - A change with a block of the open map in its middle is two: that
///   block changed already since cut 1's instant.
/// - A change of 300 blocks is planned 256 at a time, the most one read
///   or one write moves (1 MiB).
/// - After cut 1 is let go its slots are free again, in the order its
///   records were freed. A change for cut 2 takes 32 of them, and they
///   are given in the order of their blocks: one run, not 32.
///
/// Each held cut reads every block of these as it was at its instant.
#[test]
fn a_change_copies_each_run_in_one_read_and_one_write() {
    const MANY: u64 = 600;
    let size = MANY * BLOCK_SIZE;
    let bound = MANY * BLOCK_SIZE;
    let region = MemRegion::new(words_for(MANY, records_for(bound)));
    let mut state = CutState::begin(region, size, bound, OWNER, Mutants(0));
    let mut io = Counting::new(MANY);
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    let reads = |state: &CutState<MemRegion>, io: &Counting, number: u64, blocks: &[u64]| {
        for &block in blocks {
            let content = match state.read(number, block) {
                Some(Read::Copy(slot)) => io.disk.copies.get(&slot).copied(),
                other => panic!("cut {number}, block {block}: {other:?}"),
            };
            assert_eq!(content, Some(1000 + block), "cut {number}, block {block}");
        }
    };

    state.change(0, 32, &mut io).expect("the change");
    assert_eq!(io.taken(), (vec![(0, 32)], vec![(0, 32)]));

    state.change(50, 1, &mut io).expect("the change");
    assert_eq!(io.taken(), (vec![(50, 1)], vec![(32, 1)]));
    state.change(45, 10, &mut io).expect("the change");
    assert_eq!(
        io.taken(),
        (vec![(45, 5), (51, 4)], vec![(33, 5), (38, 4)]),
        "block 50 is in the open map"
    );

    state.change(100, 300, &mut io).expect("the change");
    assert_eq!(
        io.taken(),
        (vec![(100, 256), (356, 44)], vec![(42, 256), (298, 44)])
    );
    let changed: Vec<u64> = (0..32).chain(45..55).chain(100..400).collect();
    reads(&state, &io, 1, &changed);

    assert!(state.release(1));
    assert_eq!(state.figures().copied, 0, "cut 1's copies are freed");
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    state.change(0, 32, &mut io).expect("the change");
    let (read, written) = io.taken();
    assert_eq!(read, [(0, 32)]);
    assert_eq!(written.len(), 1, "the slots of one run: {written:?}");
    reads(&state, &io, 2, &(0..32).collect::<Vec<_>>());
}

/// A refused run stops a change's copies there (M2.2d): nothing after it
/// is read or written, and the change goes on from the next block,
/// planned again under the cuts still held. Cut 1 counts the whole
/// volume (the state is new) and reads every block. Block 6 is copied
/// first, so it is in the open map, and blocks 5 and 7 are two runs.
/// The first is refused, and cut 1 goes with block 6's copy; block 7's
/// copy, for a cut that is gone, is never written.
#[test]
fn a_refused_run_stops_the_copies_after_it() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let mut state = CutState::begin(region, SIZE, REFUSING_BOUND, OWNER, Mutants(0));
    let mut io = RefusingFirst {
        disk: Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        },
        // The first refusal is to come later.
        refused: true,
    };
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    state.change(6, 1, &mut io).expect("the change");
    let slots: Vec<u32> = io.disk.copies.keys().copied().collect();
    assert_eq!(slots.len(), 1, "block 6's copy");
    io.refused = false;
    state.change(5, 3, &mut io).expect("the write goes on");
    assert!(io.refused, "block 5's copy was asked and refused");
    assert!(state.held().is_empty(), "{:?}", state.held());
    assert_eq!(state.figures().copied, 0);
    assert_eq!(
        io.disk.copies.keys().copied().collect::<Vec<_>>(),
        slots,
        "block 7 was copied for a cut that is gone"
    );
}

/// A disk that fails the `nth` write of copies, once.
struct FailingNth {
    disk: Disk,
    nth: u64,
    writes: u64,
}

impl Copier for FailingNth {
    fn read_image(&mut self, first: u64, buf: &mut [u8]) -> io::Result<()> {
        self.disk.read_image(first, buf)
    }

    fn write_copies(&mut self, first: u32, buf: &[u8]) -> io::Result<()> {
        self.writes += 1;
        if self.writes == self.nth {
            return Err(io::Error::other("no space left"));
        }
        self.disk.write_copies(first, buf)
    }
}

/// A run that fails in the middle of a change, with an older cut that
/// stays (G1 of the review of #107; its scenarios). On `three_held`,
/// the second write of copies fails:
///
/// - **X**, blocks 20 to 40. The runs are block 20 for cut 2, blocks 30
///   to 32 for cut 3, and block 40 for cut 4. Cuts 3 and 4 go. Cut 2
///   stays: it reads block 20 from the copy made before the failure,
///   and the next cut's map holds every block of the change, those
///   before the failed run included.
/// - **Y**, blocks 30 to 170. The runs are blocks 30 to 32 for cut 3,
///   block 40 for cut 4, then blocks 100 and 170 for cut 3. Cut 4 goes
///   alone. Cut 3 stays: it reads 100 and 170 from copies made when the
///   rest is planned again, not from the slots planned for them before
///   the failure, which were never written.
#[test]
fn a_failed_run_in_the_middle_of_a_change_keeps_what_an_older_cut_reads() {
    for (name, first, count, cut, blocks, held) in [
        ("X", 20u64, 21u64, 2u64, vec![20u64], vec![2u64]),
        ("Y", 30, 141, 3, vec![30, 31, 32, 100, 170], vec![2, 3]),
    ] {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
        let (mut state, io) = three_held(&region);
        let mut io = FailingNth {
            disk: io.disk,
            nth: 2,
            writes: 0,
        };
        for block in 0..BLOCKS {
            io.disk.image[block as usize] = 1000 + block;
        }
        state
            .change(first, count, &mut io)
            .expect("the change goes on");
        // The change's own writes.
        for block in first..first + count {
            io.disk.image[block as usize] = 9000 + block;
        }
        let now: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        assert_eq!(now, held, "{name}");
        for &block in &blocks {
            let content = match state.read(cut, block) {
                Some(Read::Copy(slot)) => io.disk.copies.get(&slot).copied(),
                Some(Read::Image) => Some(io.disk.image[block as usize]),
                other => panic!("{name}: cut {cut}, block {block}: {other:?}"),
            };
            assert_eq!(
                content,
                Some(1000 + block),
                "{name}: cut {cut}, block {block}"
            );
        }
        let next = state.freeze(&marked(5), true);
        let map = state.map(next);
        let lacks: Vec<u64> = (first..first + count)
            .filter(|block| !map.contains(block))
            .collect();
        assert!(
            lacks.is_empty(),
            "{name}: the next cut's map lacks {lacks:?}"
        );
    }
}

/// A cut replaced before its upload is counted, in both ways a newer cut
/// takes its map: the cut that waits when a newer cut's flush completes,
/// and a cut whose flush is still under way at the next instant. A
/// release, and a flush with none waiting, replace nothing.
#[test]
fn the_cuts_replaced_before_their_upload_are_counted() {
    let bound = 64 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let mut state = CutState::begin(region, SIZE, bound, OWNER, Mutants(0));
    let replaced = |state: &CutState<MemRegion>| state.figures().replaced;
    // Cut 1 is uploaded; cut 2 waits; cut 3's flush completes and
    // replaces it.
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    assert_eq!(replaced(&state), 0, "nothing waited");
    assert_eq!(state.freeze(&marked(3), true), 3);
    assert_eq!(replaced(&state), 0, "nothing replaced at the instant");
    state.flushed(3);
    assert_eq!(replaced(&state), 1, "cut 2 replaced by cut 3");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert_eq!(held, [1, 3]);
    // Cut 4's flush is under way at cut 5's instant: cut 5 takes its
    // map.
    assert_eq!(state.freeze(&marked(4), true), 4);
    assert_eq!(state.freeze(&marked(5), true), 5);
    assert_eq!(replaced(&state), 2, "cut 4 replaced by cut 5");
    let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
    assert_eq!(held, [1, 3, 5]);
    // Releases let cuts go, uploaded; cut 5's flush completes while cut 3
    // is being uploaded, and none waits: no replacement.
    assert!(state.release(1));
    state.flushed(5);
    assert!(state.release(3));
    assert!(state.release(5));
    assert_eq!(replaced(&state), 2);
    let figures = state.figures();
    assert_eq!((figures.held, figures.abandoned), (0, 0));
}

/// A cut abandoned at the bound while a change is planned takes with it
/// the copies planned for it (M2.2d), as M2.2c freed their records with
/// the cut: no copy stays that no held cut reads. Cut 1, the whole
/// volume, is the only cut held, and the bound is three blocks. Blocks
/// 0 and 1 are copied; then a change of blocks 2 and 3 plans block 2's
/// copy, the third, and block 3's would pass the bound. Cut 1 goes, and
/// gives its map to the open map, so no cut reads block 2 either.
#[test]
fn a_cut_abandoned_while_a_change_is_planned_takes_its_planned_copies() {
    let bound = 3 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let mut state = CutState::begin(region, SIZE, bound, OWNER, Mutants(0));
    let mut io = Counting::new(BLOCKS);
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    state.change(0, 2, &mut io).expect("the change");
    assert_eq!(state.figures().copied, 2 * BLOCK_SIZE);
    io.taken();
    state.change(2, 2, &mut io).expect("the change");
    assert!(state.held().is_empty(), "cut 1 is abandoned at the bound");
    assert_eq!(state.figures().abandoned, 1);
    assert_eq!(state.figures().copied, 0, "a copy no cut reads stays");
    assert_eq!(io.taken(), (vec![], vec![]), "a copy for no cut was made");
}

/// Two starts in a row after a daemon lost at any store of a change that
/// abandons a cut at the bound count it once: the first start counts the
/// cut whose abandoning a death interrupted and clears the entry in one
/// store, so the second finds nothing to count. Cut 1 counts the whole
/// volume, the bound is two blocks, and the third copy abandons it.
/// From the review of #98 (`second_start.py`): with the entry left in
/// the word when its cut is counted, the second start counts it again.
#[test]
fn a_second_start_after_a_death_in_an_abandoning_counts_the_cut_once() {
    quiet_deaths();
    let bound = 2 * BLOCK_SIZE;
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
        let mut state = CutState::begin(region.clone(), SIZE, bound, OWNER, Mutants(0));
        let mut disk = Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        };
        state.change(0, 1, &mut disk).expect("the change");
        assert_eq!(state.freeze(&marked(1), true), 1);
        state.flushed(1);
        state.change(1, 1, &mut disk).expect("the change");
        state.change(2, 1, &mut disk).expect("the change");
        assert_eq!(state.figures().abandoned, 0);
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.change(3, 1, &mut disk)));
        region.revive();
        match done {
            Ok(result) => {
                result.expect("the change");
                assert_eq!(state.figures().abandoned, 1);
                break;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let start = || {
            CutState::restore(region.clone(), SIZE, bound, OWNER, Mutants(0))
                .map_err(|(e, _)| e)
                .expect("the state")
        };
        let first = start();
        let (held, once) = (first.held().len() as u64, first.figures().abandoned);
        drop(first);
        let twice = start().figures().abandoned;
        assert_eq!(
            once, twice,
            "a death at store {death}: the first start counts {once} abandoned, the second {twice}"
        );
        assert_eq!(
            once + held,
            1,
            "a death at store {death}: cut 1 is held or counted"
        );
    }
    assert!(deaths > 0);
}

/// The row in object storage is not what the held cuts were made
/// against (M2.4, the uploader's check): every held cut is abandoned
/// and counted once, their copies go with them, and the next cut holds
/// the whole volume; the cut after it holds only its own changes.
#[test]
fn a_mismatch_abandons_every_held_cut_and_the_next_cut_holds_the_whole_volume() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let (mut state, mut io) = three_held(&region);
    // Block 20 is in cut 2's map: cut 2 reads it from a copy now.
    state.change(20, 1, &mut io).expect("the change");
    let before = state.figures();
    assert!(before.copied > 0);
    assert!(!state.is_whole_cut(2), "cut 2 holds its own changes");
    assert_eq!(state.abandon_all(), [2, 3, 4]);
    assert!(state.held().is_empty());
    let after = state.figures();
    assert_eq!(after.abandoned, before.abandoned + 3, "each counted once");
    assert_eq!(after.copied, 0, "the copies go with the cuts");
    state.change(60, 1, &mut io).expect("the change");
    let five = state.freeze(&marked(5), true);
    assert!(state.is_whole_cut(five));
    assert_eq!(state.map(five), (0..BLOCKS).collect::<Vec<_>>());
    state.flushed(five);
    assert!(state.release(five));
    state.change(61, 1, &mut io).expect("the change");
    let six = state.freeze(&marked(6), true);
    assert!(!state.is_whole_cut(six));
    assert_eq!(state.map(six), [61], "the cut after holds its own changes");
}

/// The same with no cut held: nothing is counted, and the next cut
/// holds the whole volume.
#[test]
fn a_mismatch_with_no_cut_held_makes_the_next_cut_whole() {
    let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
    let mut state = CutState::begin(region, SIZE, REFUSING_BOUND, OWNER, Mutants(0));
    let mut disk = Disk {
        image: vec![0; BLOCKS as usize],
        copies: BTreeMap::new(),
    };
    state.change(3, 1, &mut disk).expect("the change");
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    assert!(state.release(1));
    state.change(4, 1, &mut disk).expect("the change");
    assert!(state.abandon_all().is_empty());
    assert_eq!(state.figures().abandoned, 0);
    let two = state.freeze(&marked(2), true);
    assert!(state.is_whole_cut(two));
    assert_eq!(state.map(two), (0..BLOCKS).collect::<Vec<_>>());
}

/// A death at any store while a mismatch abandons the held cuts, with
/// `flushing` whether the newest's flush is under way: the start after
/// it holds the newest of them, none skipped, the oldest being
/// uploaded; the abandoning done again (the uploader checks again
/// before any upload) leaves none held and the next cut whole. Each cut
/// is counted once in all, but one: where the death left cut 3 waiting
/// and cut 4 flushing, the start gives cut 4 cut 3's place, as every
/// start does (it takes the sync as the flush), and cut 3 is not
/// counted (review of M2.4b-1, N2). At each death each held cut, read
/// as the region is, is listed in slices as its map lists it.
fn deaths_while_a_mismatch_abandons(flushing: bool) -> u64 {
    quiet_deaths();
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
        let (mut state, _io) = three_held(&region);
        if !flushing {
            // Cut 4 takes cut 3's map and waits.
            state.flushed(4);
        }
        let numbers: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        let before = state.figures().abandoned;
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.abandon_all()));
        region.revive();
        match done {
            Ok(abandoned) => {
                assert_eq!(abandoned, numbers);
                assert_eq!(state.figures().abandoned - before, numbers.len() as u64);
                break;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let at_death = CutState::view(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state as the death left it");
        let phases: Vec<(u64, Phase)> = at_death
            .held()
            .iter()
            .map(|c| (c.number, c.phase))
            .collect();
        let case = format!("flushing {flushing}, a death at store {death}: held {phases:?}");
        for &(number, _) in &phases {
            assert_listed_in_slices(&at_death, number, &case);
        }
        let merges =
            phases.contains(&(3, Phase::Waiting)) && phases.contains(&(4, Phase::Flushing));
        drop(at_death);
        let mut state = CutState::restore(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state");
        let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        let case = format!("{case}, after the start {held:?}");
        let cuts: Vec<u64> = numbers
            .iter()
            .copied()
            .filter(|&number| !(merges && number == 3))
            .collect();
        assert!(cuts.ends_with(&held), "{case}");
        if let Some(oldest) = state.held().first() {
            assert_eq!(oldest.phase, Phase::Uploading, "{case}");
        }
        let counted = state.figures().abandoned - before;
        assert_eq!(
            counted + held.len() as u64,
            cuts.len() as u64,
            "{case}: each cut held or counted"
        );
        assert_eq!(state.abandon_all(), held, "{case}");
        let counted = state.figures().abandoned - before;
        assert_eq!(counted, cuts.len() as u64, "{case}: each cut counted once");
        let next = state.freeze(&marked(9), true);
        assert!(state.is_whole_cut(next), "{case}");
        assert_eq!(state.map(next), (0..BLOCKS).collect::<Vec<_>>(), "{case}");
    }
    deaths
}

#[test]
fn a_death_while_a_mismatch_abandons_the_held_cuts() {
    for flushing in [false, true] {
        let deaths = deaths_while_a_mismatch_abandons(flushing);
        assert!(deaths > 3, "flushing {flushing}: {deaths} deaths");
    }
}

/// Held cut `number` listed by `map_from` a block at a time, as an
/// upload reads it in slices: what `map` lists.
fn assert_listed_in_slices(state: &CutState<MemRegion>, number: u64, case: &str) {
    let mut listed = Vec::new();
    let mut from = Some(0);
    for _ in 0..=BLOCKS {
        let Some(at) = from else { break };
        let (blocks, next) = state.map_from(number, at, 1).expect("a held cut");
        listed.extend(blocks);
        from = next;
    }
    assert_eq!(
        listed,
        state.map(number),
        "{case}: cut {number} in slices of 1"
    );
}

/// A death at any store while a mismatch abandons the held cuts of a
/// state with nothing uploaded, whose oldest cut holds the whole volume
/// (review of M2.4b-1, G1): the oldest cut the start holds still holds
/// the whole volume, since the uploader uploads a whole cut with no
/// base (Q6) and any other onto the intent map's base, which names
/// none here; and the next cut after the abandoning done again is
/// whole. The whole mark comes after the cuts are gone. The defect,
/// planted by hand: the mark stored first.
#[test]
fn a_death_while_a_mismatch_abandons_leaves_the_oldest_cut_whole() {
    quiet_deaths();
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
        let mut state = CutState::begin(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0));
        let mut disk = Disk {
            image: vec![0; BLOCKS as usize],
            copies: BTreeMap::new(),
        };
        for (number, block) in [(1, 10), (2, 20), (3, 30)] {
            state.change(block, 1, &mut disk).expect("the change");
            assert_eq!(state.freeze(&marked(number), true), number);
            if number < 3 {
                state.flushed(number);
            }
        }
        let held: Vec<(u64, Phase)> = state.held().iter().map(|c| (c.number, c.phase)).collect();
        assert_eq!(
            held,
            [
                (1, Phase::Uploading),
                (2, Phase::Waiting),
                (3, Phase::Flushing)
            ]
        );
        assert!(state.is_whole_cut(1));
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.abandon_all()));
        region.revive();
        match done {
            Ok(abandoned) => {
                assert_eq!(abandoned, [1, 2, 3]);
                break;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let mut state = CutState::restore(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state");
        let held: Vec<u64> = state.held().iter().map(|c| c.number).collect();
        let case = format!("a death at store {death}: held {held:?}");
        if let Some(&oldest) = held.first() {
            assert!(
                state.is_whole_cut(oldest),
                "{case}: cut {oldest}, the oldest held, does not hold the whole volume"
            );
        }
        assert_eq!(state.abandon_all(), held, "{case}");
        let next = state.freeze(&marked(9), true);
        assert!(state.is_whole_cut(next), "{case}");
    }
    assert!(deaths > 3, "{deaths} deaths");
}

/// `map_from` lists what `map` lists from a block on, over a volume
/// whose maps have several words of summary (review of M2.4b-1, G3): a
/// word of summary covers 4096 blocks, and the tests' other volumes have
/// one. From blocks at the edges of words and of words of summary, with
/// several limits; for a whole cut, a cut of its own changes, and a cut
/// that took another's map. The defects, planted by hand: the summary
/// read from its first word whatever block is asked, and the mask put on
/// the summary's first word and not on the word of the block asked.
#[test]
fn a_map_is_listed_from_any_block_over_several_words_of_summary() {
    let blocks: u64 = 3 * 4096 + 100;
    let region = MemRegion::new(words_for(blocks, records_for(REFUSING_BOUND)));
    let mut state = CutState::begin(
        region,
        blocks * BLOCK_SIZE,
        REFUSING_BOUND,
        OWNER,
        Mutants(0),
    );
    let mut disk = Disk {
        image: vec![0; blocks as usize],
        copies: BTreeMap::new(),
    };
    state.change(0, 1, &mut disk).expect("the change");
    assert_eq!(state.freeze(&marked(1), true), 1);
    state.flushed(1);
    for from in [0, 1, 4095, 4096, 8191, 8192, blocks - 1, blocks, blocks + 5] {
        for limit in [1usize, 3, 64, 5000] {
            let first = from.min(blocks);
            let end = (first + limit as u64).min(blocks);
            assert_eq!(
                state.map_from(1, from, limit),
                Some(((first..end).collect(), (end < blocks).then_some(end))),
                "the whole cut from block {from}, {limit} blocks"
            );
        }
    }
    assert!(state.release(1));
    let own: Vec<u64> = vec![
        0, 63, 64, 65, 4095, 4096, 4097, 8191, 8192, 9000, 12287, 12288, 12387,
    ];
    for &block in &own {
        state.change(block, 1, &mut disk).expect("the change");
    }
    assert_eq!(state.freeze(&marked(2), true), 2);
    state.flushed(2);
    assert!(!state.is_whole_cut(2));
    assert_eq!(state.map(2), own);
    let froms: Vec<u64> = own
        .iter()
        .flat_map(|&block| [block.saturating_sub(1), block, block + 1])
        .chain([
            2,
            100,
            4000,
            5000,
            8000,
            10000,
            12000,
            blocks,
            blocks + 5,
            1 << 40,
        ])
        .collect();
    let listed = |state: &CutState<MemRegion>, number: u64, map: &[u64], limit: usize| {
        for &from in &froms {
            let rest: Vec<u64> = map.iter().copied().filter(|&block| block >= from).collect();
            assert_eq!(
                state.map_from(number, from, limit),
                Some((
                    rest.iter().copied().take(limit).collect(),
                    rest.get(limit).copied()
                )),
                "cut {number} from block {from}, {limit} blocks"
            );
        }
    };
    for limit in [1, 2, 3, 5, 64] {
        listed(&state, 2, &own, limit);
    }
    // Cut 3 takes cut 2's map as the bound lets cut 2 go.
    state.change(5000, 1, &mut disk).expect("the change");
    state.change(64, 1, &mut disk).expect("the change");
    assert_eq!(state.freeze(&marked(3), true), 3);
    state.flushed(3);
    assert!(state.abandon_uploading(2));
    let mut union = own.clone();
    union.push(5000);
    union.sort_unstable();
    assert_eq!(state.map(3), union);
    for limit in [1, 4, 64] {
        listed(&state, 3, &union, limit);
    }
}

/// A death at any store of a release (its commit in object storage has
/// landed, M2.4): the start after it still holds the cut, as the one
/// being uploaded and as it was marked, which the uploader's check
/// finds in the row and releases; or the cut is gone. Either way the
/// cut being uploaded next is made against it.
#[test]
fn a_death_in_a_release_leaves_the_cut_being_uploaded_or_gone() {
    quiet_deaths();
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(REFUSING_BOUND)));
        let (mut state, _io) = three_held(&region);
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| state.release(2)));
        region.revive();
        match done {
            Ok(released) => {
                assert!(released);
                break;
            }
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        drop(state);
        let mut state = CutState::restore(region.clone(), SIZE, REFUSING_BOUND, OWNER, Mutants(0))
            .map_err(|(e, _)| e)
            .expect("the state");
        if let Some(two) = state.held().into_iter().find(|c| c.number == 2) {
            assert_eq!(
                (two.phase, two.state),
                (Phase::Uploading, marked(2)),
                "a death at store {death}"
            );
            assert!(state.release(2), "a death at store {death}");
        }
        let next = state.held().first().map(|c| (c.phase, c.lo));
        assert_eq!(
            next,
            Some((Phase::Uploading, 2)),
            "a death at store {death}: the next cut is made against cut 2"
        );
    }
    assert!(deaths > 0);
}

// ---- the image's holes (M2.4e) ----

/// A world with holes that holds cut 1, the whole volume, flushed: the
/// one being uploaded.
fn whole_cut_held(bound_blocks: u64) -> World {
    run_in(true, &[Step::Cut, Step::Flushed], 0, bound_blocks, None).expect("runs")
}

/// A write into a hole under a held cut that holds the whole volume
/// makes no copy, and the cut reads zeros there; a write into a chunk
/// of data copies, as before M2.4e, and so does a write into a hole
/// where the state has no hole set.
#[test]
fn a_write_into_a_hole_under_a_held_whole_cut_makes_no_copy() {
    let mut world = whole_cut_held(64);
    assert_eq!(world.state.figures().holes, Holes::Held);
    world.step(Step::Write(0, 2)).expect("written");
    world.step(Step::Write(33, 1)).expect("written");
    assert_eq!(world.state.figures().copied, 0, "copies for holes");
    for block in [0, 1, 33] {
        assert_eq!(
            world.state.read(1, block),
            Some(Read::Zero),
            "block {block}"
        );
    }
    world.step(Step::Write(64, 1)).expect("written");
    assert_eq!(world.state.figures().copied, BLOCK_SIZE, "a copy for data");
    assert!(matches!(world.state.read(1, 64), Some(Read::Copy(_))));
    // Zeros that the set does not name, in a chunk of data, are copied.
    world.step(Step::Write(PART_HOLE.0, 1)).expect("written");
    assert_eq!(world.state.figures().copied, 2 * BLOCK_SIZE);

    let mut world = run_in(false, &[Step::Cut, Step::Flushed], 0, 64, None).expect("runs");
    assert_eq!(world.state.figures().holes, Holes::None);
    world.step(Step::Write(0, 1)).expect("written");
    assert_eq!(world.state.figures().copied, BLOCK_SIZE, "no set, a copy");
}

/// A write into a hole before the cut's instant is in the cut's map
/// and reads as written; the rest of its chunk, which nothing wrote,
/// reads as zeros. The cut that follows reads the block from the image
/// it is uploaded onto.
#[test]
fn a_write_into_a_hole_before_the_instant_reads_as_written() {
    let steps = [Step::Write(1, 1), Step::Cut, Step::Flushed];
    let world = run_in(true, &steps, 0, 64, None).expect("runs");
    assert_eq!(world.state.read(1, 1), Some(Read::Image));
    assert_eq!(world.disk.image[1], 1, "the write's content");
    for block in [0, 2, 15] {
        assert_eq!(
            world.state.read(1, block),
            Some(Read::Zero),
            "block {block}"
        );
    }
    assert_eq!(world.state.read(1, 16), Some(Read::Image), "data");
}

/// Once the set is dropped no cut reads by it, also a cut made whole
/// again later: after a mismatch lets every held cut go, the next cut
/// counts the whole volume as changed with a `lo` that is not 0, and a
/// block of a hole that was written and uploaded before the drop reads
/// from the image, though the dropped set still names its chunk. Each
/// of the two conditions that keep it so (the set holds, the cut's `lo`
/// is 0) covers the other: planted together they are caught here.
#[test]
fn a_cut_made_whole_after_the_drop_reads_the_holes_written_before_it() {
    let steps = [
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        Step::Release,
        Step::Write(16, 1),
        Step::Cut,
        Step::Flushed,
    ];
    let mut world = run_in(true, &steps, 0, 64, None).expect("runs");
    assert_eq!(world.state.figures().holes, Holes::Dropped);
    assert_ne!(world.disk.image[1], 0, "block 1 was written");
    assert_eq!(world.state.abandon_all(), [2]);
    let number = world.state.freeze(&marked(3), true);
    world.state.flushed(number);
    assert!(world.state.is_whole_cut(number));
    assert_eq!(world.state.held().first().map(|c| c.lo), Some(1));
    assert_eq!(world.state.read(number, 1), Some(Read::Image));
    assert_eq!(world.state.read(number, 2), Some(Read::Image));
    let slice = world.state.slice_from(number, 0, 64).expect("held");
    assert!(slice.zeroes.is_empty(), "{slice:?}");
    assert_eq!(slice.blocks, (0..64).collect::<Vec<u64>>());
}

/// A mismatch while the hole set holds (the review of #125, G1): every
/// held cut is let go, and their maps go to the open map with the
/// lowest `lo`, 0, so the set stays held and the next cut, whole and
/// with `lo` 0, reads by it. Its map still holds every block written
/// since the state began: block 1, written into a hole before the
/// mismatch, reads as written, and the rest of its chunk as zeros; so
/// do blocks 33 and 49, written into holes under the cuts let go. A
/// write into a hole after the mismatch makes no copy for the new cut.
/// The defects this catches: the open map cleared before the open epoch
/// is marked whole (the new cut would read block 1 as a hole's zeros),
/// and the set dropped at the mismatch (the write after it would be
/// copied).
#[test]
fn a_mismatch_while_the_holes_are_held_keeps_them_and_the_writes_before_it() {
    // Chunks 0, 2 and 3 (blocks 0 to 15 and 32 to 63) are holes, and
    // chunk 1 (16 to 31) is data.
    let steps = [
        Step::Write(1, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(33, 1),
        Step::Cut,
        Step::Write(49, 1),
        Step::Mismatch,
        Step::Write(50, 1),
        Step::Cut,
        Step::Flushed,
    ];
    let mut world = run_in(true, &steps, 0, 64, None).expect("runs");
    assert_eq!(world.state.figures().abandoned, 2);
    assert_eq!(world.state.figures().holes, Holes::Held);
    let held = world.state.held();
    assert_eq!(held.len(), 1, "{held:?}");
    let (number, lo) = (held[0].number, held[0].lo);
    assert_eq!((number, lo), (3, 0));
    assert!(world.state.is_whole_cut(number));
    for block in [1, 16, 31, 33, 49, 50] {
        assert_eq!(
            world.state.read(number, block),
            Some(Read::Image),
            "block {block}, written or data"
        );
    }
    for block in [0, 2, 15, 32, 34, 47, 48, 51, 63] {
        assert_eq!(
            world.state.read(number, block),
            Some(Read::Zero),
            "block {block}, a hole's"
        );
    }
    assert_eq!(world.state.figures().copied, 0);
    world.step(Step::Write(40, 1)).expect("written");
    assert_eq!(
        world.state.figures().copied,
        0,
        "a write into a hole after it"
    );
    assert_eq!(world.state.read(number, 40), Some(Read::Zero));
    world.step(Step::Release).expect("released");
    assert_eq!(world.state.figures().holes, Holes::Dropped);
}

/// Over random programs with mismatches, with and without the image's
/// holes, the bound at 2 blocks and at 64, every held cut reads as the
/// volume at its instant and the hole set holds exactly while a cut may
/// read by it (the review of #125, G1).
#[test]
fn every_held_cut_reads_as_the_volume_at_its_instant_through_mismatches() {
    let mut mismatches = 0;
    for holes in [false, true] {
        for bound in [2, 64] {
            for seed in 0..200 {
                let steps = program_with_mismatches(seed, 80);
                mismatches += steps.iter().filter(|s| **s == Step::Mismatch).count();
                if let Err(e) = run_in(holes, &steps, 0, bound, None) {
                    panic!("holes {holes}, bound {bound}, seed {seed}: {e}");
                }
            }
        }
    }
    assert!(mismatches > 1000, "{mismatches} mismatches");
}

/// The same with the daemon lost at every store of the region, a
/// mismatch's stores included: four programs for each bound.
#[test]
fn a_death_at_any_store_through_mismatches_leaves_every_held_cut_whole() {
    let mut runs = 0;
    for holes in [false, true] {
        for bound in [2, 64] {
            for seed in 0..4 {
                let steps = program_with_mismatches(seed, 50);
                assert!(steps.contains(&Step::Mismatch), "seed {seed}");
                let stores = stores_in(holes, &steps, 0, bound);
                for death in 0..stores {
                    runs += 1;
                    if let Err(e) = run_in(holes, &steps, 0, bound, Some(death)) {
                        panic!(
                            "holes {holes}, bound {bound}, seed {seed}, death at store {death}: {e}"
                        );
                    }
                }
            }
        }
    }
    assert!(runs > 500, "{runs} runs");
}

/// A copier for changes made while no cut is held: none needs a copy.
struct NoCopies;

impl Copier for NoCopies {
    fn read_image(&mut self, _: u64, _: &mut [u8]) -> io::Result<()> {
        Err(io::Error::other("no copy is needed while no cut is held"))
    }

    fn write_copies(&mut self, _: u32, _: &[u8]) -> io::Result<()> {
        Err(io::Error::other("no copy is needed while no cut is held"))
    }
}

/// The slices of the first cut over a large hole that blocks were
/// written into read the hole set once over, and not once for each gap
/// between those blocks (Codex on #125): a run of zeros ends at the next
/// block of the cut's map, and the look into the hole set for the run's
/// end stops there too. Before, each run's end was looked for to the
/// end of the hole, twice, which over a sparse 1 TiB volume made the
/// first upload's slicing cost the gaps times the volume, on the
/// store's queue thread.
#[test]
fn the_slices_of_a_large_hole_read_the_hole_set_once_over() {
    // 4 GiB: a hole set of 1024 words, an open map of 16384.
    const LARGE: u64 = 1 << 20;
    const APART: u64 = 512;
    let bound = 64 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(LARGE, records_for(bound)));
    let mut state = CutState::begin_with(
        region.clone(),
        LARGE * BLOCK_SIZE,
        bound,
        OWNER,
        Mutants(0),
        None,
        Some(&[(0, LARGE)]),
    );
    for block in (APART / 2..LARGE).step_by(APART as usize) {
        state.change(block, 1, &mut NoCopies).expect("the change");
    }
    let number = state.freeze(&marked(1), true);
    state.flushed(number);
    let before = region.loads();
    let (mut from, mut zeroes, mut blocks, mut slices) = (Some(0), 0, 0, 0);
    while let Some(at) = from {
        let slice = state.slice_from(number, at, usize::MAX).expect("held");
        zeroes += slice.zeroes.iter().map(|&(_, count)| count).sum::<u64>();
        blocks += slice.blocks.len() as u64;
        slices += 1;
        from = slice.next;
    }
    let loads = region.loads() - before;
    assert_eq!(blocks, LARGE / APART);
    assert_eq!(zeroes + blocks, LARGE);
    assert!(slices > 1, "the runs fill more than one slice");
    // One pass over the open map's words and the hole set's, with room
    // for the few loads each block of the map takes.
    let once = LARGE / 64 + hole_words(LARGE) as u64;
    assert!(loads <= 4 * once, "{loads} loads for {once} words");
}

/// The slices of the first cut over a sparse volume, a chunk of data in
/// every MiB and nothing written before the cut, read the open map's
/// summary and the hole set once over, in slices of 64 blocks as the
/// uploader takes them (F4 of the review of #125, Codex's P2 at
/// `af7b524`). Before, each slice looked for the next block of the map
/// from its first run to the volume's end: over 64 GiB, 37.6 million
/// loads, and about two minutes at a TiB, on the store's queue thread.
/// Counts of loads rather than times, so that the bounds hold on any
/// machine: one in proportion to a pass, and one on how the cost grows
/// with the volume.
#[test]
fn the_slices_of_a_sparse_volume_read_the_map_and_hole_set_once_over() {
    // 64 GiB: a summary of 4096 words, a hole set of 16384.
    const LARGE: u64 = 1 << 24;
    let (loads, once) = sparse_slicing(LARGE);
    assert!(loads <= 8 * once, "{loads} loads for {once}");
    // A quarter of the volume costs about a quarter: the code at
    // `af7b524` made 3.1 million loads at 16 GiB, a twelfth.
    let (quarter, _) = sparse_slicing(LARGE / 4);
    assert!(
        loads <= 5 * quarter,
        "{loads} loads at 64 GiB, {quarter} at 16 GiB"
    );
}

/// [`Maps::first_in`] gives the first block of the maps in a range, or
/// none, over ranges that span several words of a map's summary. The
/// state calls it on one stretch of [`RUN_LOOK`] blocks, which a single
/// word of the summary covers, so no other test reaches past the first
/// word (F4g, a plant for F4 of the review of #125).
#[test]
fn the_first_block_in_a_range_is_found_over_several_words_of_the_summary() {
    const BLOCKS: u64 = 5 * RUN_LOOK + 100;
    let mut region = MemRegion::new(Maps::words(BLOCKS));
    let maps = Maps::new(0, BLOCKS, Mutants(0));
    let in_first = [5, RUN_LOOK + 70, 3 * RUN_LOOK + 1];
    let in_second = [2 * RUN_LOOK + 9, BLOCKS - 1];
    for block in in_first {
        maps.insert(&mut region, 0, block);
    }
    for block in in_second {
        maps.insert(&mut region, 1, block);
    }
    let first: BTreeSet<u64> = in_first.into_iter().collect();
    let both: BTreeSet<u64> = in_first.into_iter().chain(in_second).collect();
    let froms = [
        0,
        5,
        6,
        RUN_LOOK,
        RUN_LOOK + 71,
        2 * RUN_LOOK + 9,
        2 * RUN_LOOK + 10,
        3 * RUN_LOOK + 2,
        BLOCKS - 1,
        BLOCKS,
    ];
    for from in froms {
        for length in [0, 1, 64, RUN_LOOK, 3 * RUN_LOOK, BLOCKS, BLOCKS + 10] {
            let until = from + length;
            let end = until.min(BLOCKS).max(from);
            for (named, set) in [(&[0][..], &first), (&[0, 1][..], &both)] {
                assert_eq!(
                    maps.first_in(&region, named, from, until),
                    set.range(from..end).next().copied(),
                    "maps {named:?}, {from}..{until}"
                );
            }
        }
    }
}

/// A run of zeros is not cut where a look for its end stops, every
/// [`RUN_LOOK`] blocks, and it ends at the next block of the cut's map
/// however many stretches away that lies: at the first block of one,
/// at its last, or with none to the volume's end (F4 of the review of
/// #125).
#[test]
fn a_run_of_zeros_goes_on_over_the_looks_to_the_next_block_of_the_map() {
    const BLOCKS: u64 = 4 * RUN_LOOK;
    let bound = 64 * BLOCK_SIZE;
    let written = [
        None,
        Some(RUN_LOOK - 1),
        Some(RUN_LOOK + 5),
        Some(2 * RUN_LOOK),
        Some(BLOCKS - 1),
    ];
    for written in written {
        let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
        let mut state = CutState::begin_with(
            region,
            BLOCKS * BLOCK_SIZE,
            bound,
            OWNER,
            Mutants(0),
            None,
            Some(&[(0, BLOCKS)]),
        );
        if let Some(block) = written {
            state.change(block, 1, &mut NoCopies).expect("the change");
        }
        let number = state.freeze(&marked(1), true);
        state.flushed(number);
        let slice = state.slice_from(number, 7, 64).expect("held");
        let (zeroes, blocks) = match written {
            None => (vec![(7, BLOCKS - 7)], vec![]),
            Some(block) if block + 1 == BLOCKS => (vec![(7, block - 7)], vec![block]),
            Some(block) => (
                vec![(7, block - 7), (block + 1, BLOCKS - block - 1)],
                vec![block],
            ),
        };
        assert_eq!(slice.zeroes, zeroes, "written {written:?}");
        assert_eq!(slice.blocks, blocks, "written {written:?}");
        assert_eq!(slice.next, None, "written {written:?}");
    }
}

/// The slices of the first cut over an empty volume, one hole from end
/// to end, read the cut's map and the hole set once over, though its
/// one run of zeros crosses every stretch of [`RUN_LOOK`] blocks: each
/// look goes on from where the last one stopped, in the map and in the
/// hole set. So with one block written, in the middle or last (G3 of
/// the review of #125: a look into the hole set, or into the map, from
/// the run's first block at every stretch made 33.6 million loads, or
/// 8.4 million, at 64 GiB, where the code makes about 29,000; no other
/// cost test has a run longer than a stretch).
#[test]
fn a_run_of_zeros_over_an_empty_volume_reads_the_map_and_hole_set_once_over() {
    // 64 GiB: 4096 stretches, a summary of 4096 words, a hole set of
    // 16384.
    const LARGE: u64 = 1 << 24;
    for written in [None, Some(LARGE / 2 + 3), Some(LARGE - 1)] {
        let (loads, once) = empty_slicing(LARGE, written);
        assert!(
            loads <= 4 * once,
            "written {written:?}: {loads} loads for {once}"
        );
    }
    // A quarter of the volume costs about a quarter: either look from
    // the run's first block makes it a sixteenth.
    let (loads, _) = empty_slicing(LARGE, None);
    let (quarter, _) = empty_slicing(LARGE / 4, None);
    assert!(
        loads <= 5 * quarter,
        "{loads} loads at 64 GiB, {quarter} at 16 GiB"
    );
}

/// The loads that the slices of the first cut over an empty volume of
/// `large` blocks take, in slices of 64 blocks, with block `written`
/// written before the cut; and one pass over the open map's summary
/// and the hole set.
fn empty_slicing(large: u64, written: Option<u64>) -> (u64, u64) {
    let bound = 64 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(large, records_for(bound)));
    let mut state = CutState::begin_with(
        region.clone(),
        large * BLOCK_SIZE,
        bound,
        OWNER,
        Mutants(0),
        None,
        Some(&[(0, large)]),
    );
    if let Some(block) = written {
        state.change(block, 1, &mut NoCopies).expect("the change");
    }
    let number = state.freeze(&marked(1), true);
    state.flushed(number);
    let before = region.loads();
    let (mut from, mut zeroes, mut blocks) = (Some(0), Vec::new(), Vec::new());
    while let Some(at) = from {
        let slice = state.slice_from(number, at, 64).expect("held");
        zeroes.extend(slice.zeroes);
        blocks.extend(slice.blocks);
        from = slice.next;
    }
    let loads = region.loads() - before;
    let runs: Vec<(u64, u64)> = match written {
        None => vec![(0, large)],
        Some(block) => [(0, block), (block + 1, large - block - 1)]
            .into_iter()
            .filter(|&(_, count)| count > 0)
            .collect(),
    };
    assert_eq!(zeroes, runs, "written {written:?}");
    assert_eq!(blocks, Vec::from_iter(written), "written {written:?}");
    (loads, large / 64 / 64 + hole_words(large) as u64)
}

/// Slices of the first cut over volumes of a few stretches of
/// [`RUN_LOOK`] blocks and part of one, with holes and writes at
/// random, many of them at or beside the edge of a stretch, of a hole
/// or of a chunk, taken from the volume's start with limits at random:
/// together they read every block once and in order, as zeros where it
/// lay in a hole and was not written before the cut, as a block
/// otherwise; a run of zeros goes on as long as the zeros do; and no
/// slice has more blocks than its limit or more runs than
/// [`SLICE_ZERO_RUNS`] (G4 of the review of #125: no other test slices
/// at random across the edge of a stretch).
#[test]
fn random_slices_over_several_stretches_read_each_block_once_as_the_holes_say() {
    let bound = 64 * BLOCK_SIZE;
    let (mut crossing, mut at_edges) = (0, 0);
    for seed in 0..64u64 {
        let mut x = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        let mut next = move |n: u64| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x % n.max(1)
        };
        let stretches = 1 + next(4);
        let blocks = stretches * RUN_LOOK + next(RUN_LOOK);
        let chunks = blocks.div_ceil(HOLE_BLOCKS);
        // The edges of the stretches, the volume's end among them.
        let edges: Vec<u64> = (1..=stretches)
            .map(|k| k * RUN_LOOK)
            .chain([blocks])
            .collect();
        let mut holes = Vec::new();
        for _ in 0..=next(12) {
            let first = match next(3) {
                0 => next(chunks),
                // At a stretch's edge, or a chunk or two before it.
                _ => {
                    (edges[next(edges.len() as u64) as usize] / HOLE_BLOCKS).saturating_sub(next(3))
                }
            };
            let count = 1 + match next(3) {
                0 => next(4),
                1 => next(RUN_LOOK / HOLE_BLOCKS),
                _ => next(chunks),
            };
            holes.push((first * HOLE_BLOCKS, count * HOLE_BLOCKS));
        }
        if next(8) == 0 {
            holes.push((0, blocks));
        }
        let mut writes = Vec::new();
        for _ in 0..next(48) {
            let edge = edges[next(edges.len() as u64) as usize];
            let (first, count) = holes[next(holes.len() as u64) as usize];
            let at = match next(4) {
                0 => next(blocks),
                // At a stretch's edge, or a block or two either side.
                1 => (edge + next(5)).saturating_sub(2),
                // At a hole's first or last block, or just outside.
                2 if next(2) == 0 => (first + next(3)).saturating_sub(1),
                2 => (first + count + next(3)).saturating_sub(2),
                // Inside the chunk before or after a stretch's edge.
                _ => (edge + HOLE_BLOCKS / 2).saturating_sub(next(2) * HOLE_BLOCKS),
            };
            let count = if next(4) == 0 { 1 + next(40) } else { 1 };
            if at < blocks {
                writes.push((at, count.min(blocks - at)));
            }
        }
        // What the cut reads as zeros, from the holes as given: a chunk
        // a hole holds whole, up to the volume's end.
        let in_hole = |block: u64| {
            let start = block / HOLE_BLOCKS * HOLE_BLOCKS;
            let end = (start + HOLE_BLOCKS).min(blocks);
            holes
                .iter()
                .any(|&(first, count)| first <= start && end <= (first + count).min(blocks))
        };
        let mut zero: Vec<bool> = (0..blocks).map(in_hole).collect();
        for &(at, count) in &writes {
            for block in at..at + count {
                zero[block as usize] = false;
            }
        }
        let zero = |block: u64| zero.get(block as usize).copied().unwrap_or(false);
        let region = MemRegion::new(words_for(blocks, records_for(bound)));
        let mut state = CutState::begin_with(
            region,
            blocks * BLOCK_SIZE,
            bound,
            OWNER,
            Mutants(0),
            None,
            Some(&holes),
        );
        for &(at, count) in &writes {
            state.change(at, count, &mut NoCopies).expect("the change");
        }
        let number = state.freeze(&marked(1), true);
        state.flushed(number);
        let case = format!("seed {seed}, {blocks} blocks");
        let (mut from, mut at) = (Some(0), 0);
        while let Some(start) = from {
            assert_eq!(start, at, "{case}");
            let limit = match next(3) {
                0 => 1,
                1 => 1 + next(64) as usize,
                _ => usize::MAX,
            };
            let slice = state.slice_from(number, start, limit).expect("held");
            assert!(slice.blocks.len() <= limit, "{case}: from {start}");
            assert!(
                slice.zeroes.len() <= SLICE_ZERO_RUNS,
                "{case}: from {start}"
            );
            let mut read: Vec<(u64, u64, bool)> = slice
                .zeroes
                .iter()
                .map(|&(first, count)| (first, count, true))
                .chain(slice.blocks.iter().map(|&block| (block, 1, false)))
                .collect();
            read.sort_unstable();
            assert!(!read.is_empty(), "{case}: from {start}");
            for (first, count, zeros) in read {
                assert_eq!(first, at, "{case}: a gap or an overlap");
                assert!(count > 0, "{case}: an empty run at {first}");
                let end = first + count;
                if zeros {
                    assert!(
                        (first..end).all(zero),
                        "{case}: the run ({first}, {count}) reads a block"
                    );
                    assert!(!zero(end), "{case}: the run ({first}, {count}) stops short");
                    crossing += u64::from(first / RUN_LOOK < (end - 1) / RUN_LOOK);
                    at_edges += u64::from(end % RUN_LOOK == 0 && end < blocks);
                } else {
                    assert!(!zero(first), "{case}: block {first} read, not zeros");
                }
                at = end;
            }
            assert_eq!(slice.next, (at < blocks).then_some(at), "{case}");
            from = slice.next;
        }
        assert_eq!(at, blocks, "{case}");
    }
    // The cases reach what they are for: runs across a stretch's edge,
    // and runs that end at one.
    assert!(crossing >= 20, "{crossing} runs cross an edge");
    assert!(at_edges >= 5, "{at_edges} runs end at an edge");
}

/// The loads that the slices of the first cut over a volume of `large`
/// blocks take, a chunk of data in every MiB, in slices of 64 blocks;
/// and what they would be at a few loads for each block read and each
/// run, and one pass over the open map's summary and the hole set.
fn sparse_slicing(large: u64) -> (u64, u64) {
    const MIB: u64 = (1 << 20) / BLOCK_SIZE;
    const SLICE: usize = 64;
    let bound = 64 * BLOCK_SIZE;
    let holes: Vec<(u64, u64)> = (0..large / MIB)
        .map(|mib| (mib * MIB + HOLE_BLOCKS, MIB - HOLE_BLOCKS))
        .collect();
    let region = MemRegion::new(words_for(large, records_for(bound)));
    let mut state = CutState::begin_with(
        region.clone(),
        large * BLOCK_SIZE,
        bound,
        OWNER,
        Mutants(0),
        None,
        Some(&holes),
    );
    let number = state.freeze(&marked(1), true);
    state.flushed(number);
    let before = region.loads();
    let (mut from, mut zeroes, mut runs, mut blocks) = (Some(0), 0, 0, 0);
    while let Some(at) = from {
        let slice = state.slice_from(number, at, SLICE).expect("held");
        assert!(slice.blocks.len() <= SLICE);
        zeroes += slice.zeroes.iter().map(|&(_, count)| count).sum::<u64>();
        runs += slice.zeroes.len() as u64;
        blocks += slice.blocks.len() as u64;
        from = slice.next;
    }
    let loads = region.loads() - before;
    assert_eq!(blocks, large / MIB * HOLE_BLOCKS);
    assert_eq!(runs, large / MIB);
    assert_eq!(zeroes + blocks, large);
    let once = blocks + runs + large / 64 / 64 + hole_words(large) as u64;
    (loads, once)
}

/// The bit that says a block of a hole was written is the block's bit
/// in the open map, stored before the write is begun, so a death
/// between the bit and the write, or between the write and its answer,
/// leaves a state in which the next cut reads the block from the image:
/// as written where the write landed, or as zeros where it did not,
/// and either is right for a write not acknowledged. Without the bit
/// the cut would read zeros over the data the write left.
#[test]
fn a_death_between_a_holes_bit_and_its_write_reads_the_image() {
    for landed in [false, true] {
        let mut world = run_in(true, &[], 0, 64, None).expect("runs");
        world
            .state
            .change(5, 1, &mut world.disk)
            .expect("the change");
        // The daemon dies before or after the write lands, and before
        // the write is answered.
        if landed {
            world.disk.image[5] = 77;
        }
        let mut state = CutState::restore(
            world.region.clone(),
            SIZE,
            64 * BLOCK_SIZE,
            OWNER,
            Mutants(0),
        )
        .map_err(|(e, _)| e)
        .expect("taken up");
        let number = state.freeze(&marked(1), true);
        state.flushed(number);
        assert_eq!(state.read(number, 5), Some(Read::Image), "landed {landed}");
        assert_eq!(state.read(number, 4), Some(Read::Zero), "landed {landed}");
        let slice = state.slice_from(number, 0, 64).expect("held");
        assert_eq!(slice.zeroes.first(), Some(&(0, 5)), "landed {landed}");
        assert_eq!(slice.blocks.first(), Some(&5), "landed {landed}");
    }
}

/// A cut that holds the whole volume from the state's beginning reads
/// its holes as runs of zeros and the rest as blocks, in slices: the
/// runs end at a chunk of data or at a block of the cut's map, and a
/// block written after the instant is in a run.
#[test]
fn a_whole_cut_slices_its_holes_as_runs_of_zeros() {
    let steps = [
        Step::Write(1, 1),
        Step::Write(64, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(33, 1),
    ];
    let world = run_in(true, &steps, 0, 64, None).expect("runs");
    let slice = world.state.slice_from(1, 0, 1000).expect("held");
    let blocks: Vec<u64> = std::iter::once(1)
        .chain(16..32)
        .chain(64..80)
        .chain(96..112)
        .chain(160..192)
        .collect();
    assert_eq!(slice.blocks, blocks);
    assert_eq!(
        slice.zeroes,
        vec![(0, 1), (2, 14), (32, 32), (80, 16), (112, 48), (192, 8)]
    );
    assert_eq!(slice.next, None);
    // From inside a run, and with a limit that ends inside the data.
    let slice = world.state.slice_from(1, 40, 3).expect("held");
    assert_eq!(slice.zeroes, vec![(40, 24)]);
    assert_eq!(slice.blocks, vec![64, 65, 66]);
    assert_eq!(slice.next, Some(67));
    // The cut after it, made against it, has no runs.
    let mut world = world;
    world.step(Step::Release).expect("released");
    world.step(Step::Cut).expect("cut");
    let slice = world.state.slice_from(2, 0, 1000).expect("held");
    assert_eq!((slice.blocks, slice.zeroes), (vec![33], vec![]));
}

/// A slice names at most [`SLICE_ZERO_RUNS`] runs: a volume of
/// alternating chunks of holes and data, 300 of each, is read in two
/// slices of runs.
#[test]
fn a_slice_names_at_most_its_runs_of_zeros() {
    const CHUNKS: u64 = 600;
    let blocks = CHUNKS * HOLE_BLOCKS;
    let size = blocks * BLOCK_SIZE;
    let bound = 64 * BLOCK_SIZE;
    let holes: Vec<(u64, u64)> = (0..CHUNKS)
        .step_by(2)
        .map(|chunk| (chunk * HOLE_BLOCKS, HOLE_BLOCKS))
        .collect();
    let region = MemRegion::new(words_for(blocks, records_for(bound)));
    let mut state =
        CutState::begin_with(region, size, bound, OWNER, Mutants(0), None, Some(&holes));
    assert_eq!(state.figures().hole_chunks, CHUNKS / 2);
    let number = state.freeze(&marked(1), true);
    let first = state.slice_from(number, 0, usize::MAX).expect("held");
    assert_eq!(first.zeroes.len(), SLICE_ZERO_RUNS);
    assert_eq!(
        first.blocks.len() as u64,
        (SLICE_ZERO_RUNS as u64 - 1) * HOLE_BLOCKS
    );
    let next = first.next.expect("more");
    assert_eq!(next, (2 * SLICE_ZERO_RUNS as u64 - 1) * HOLE_BLOCKS);
    let second = state.slice_from(number, next, usize::MAX).expect("held");
    assert_eq!(
        second.zeroes.len() as u64,
        CHUNKS / 2 - SLICE_ZERO_RUNS as u64
    );
    assert_eq!(second.next, None);
}

/// The hole set holds while a cut with `lo` 0 is held, is still held
/// when such a cut is abandoned at the bound and its map goes to the
/// cut that waits, and is dropped once the first cut is uploaded; the
/// cut after it reads by its own map. A death at any store of these
/// steps leaves the same (the world checks the set after every step).
#[test]
fn the_hole_set_holds_until_the_first_whole_cut_is_uploaded() {
    // A bound of one block: block 16's copy is cut 1's, and block 17's
    // abandons cut 1, whose map and `lo` 0 go to cut 2.
    let abandoning = [
        Step::Cut,
        Step::Flushed,
        Step::Write(16, 1),
        Step::Cut,
        Step::Flushed,
        Step::Write(17, 1),
    ];
    let world = run_in(true, &abandoning, 0, 1, None).expect("runs");
    assert_eq!(world.abandoned, 1, "the bound let no cut go");
    assert_eq!(
        world
            .state
            .held()
            .first()
            .map(|c| (c.number, c.lo, c.phase)),
        Some((2, 0, Phase::Uploading))
    );
    assert_eq!(world.state.figures().holes, Holes::Held);
    assert_eq!(world.state.read(2, 0), Some(Read::Zero));
    let mut steps = abandoning.to_vec();
    steps.extend([Step::Release, Step::Write(0, 1), Step::Cut, Step::Flushed]);
    let world = run_in(true, &steps, 0, 1, None).expect("runs");
    assert_eq!(world.state.figures().holes, Holes::Dropped);
    assert_eq!(world.state.figures().hole_chunks, HOLE_CHUNKS.len() as u64);
    assert_eq!(
        world.state.held().first().map(|c| (c.number, c.lo)),
        Some((3, 2))
    );
    assert_eq!(world.state.read(3, 1), Some(Read::Unchanged));
    let stores = stores_in(true, &steps, 0, 1);
    for death in 0..stores {
        if let Err(e) = run_in(true, &steps, 0, 1, Some(death)) {
            panic!("death at store {death}: {e}");
        }
    }
}

/// A death at any store of a begin with holes leaves no state, or one
/// whose hole set is whole: the set is stored before the magic.
#[test]
fn a_daemon_lost_inside_a_begin_with_holes_leaves_the_set_whole_or_no_state() {
    quiet_deaths();
    let bound = 2 * BLOCK_SIZE;
    let mut deaths = 0;
    for death in 0u64.. {
        let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
        region.die_at(death);
        let done = catch_unwind(AssertUnwindSafe(|| {
            drop(CutState::begin_with(
                region.clone(),
                SIZE,
                bound,
                OWNER,
                Mutants(0),
                None,
                Some(&hole_ranges()),
            ));
        }));
        region.revive();
        match done {
            Ok(()) => break,
            Err(payload) => {
                if payload.downcast_ref::<Lost>().is_none() {
                    std::panic::resume_unwind(payload);
                }
            }
        }
        deaths += 1;
        if let Ok(mut state) = CutState::restore(region.clone(), SIZE, bound, OWNER, Mutants(0)) {
            let figures = state.figures();
            assert_eq!(
                (figures.holes, figures.hole_chunks),
                (Holes::Held, HOLE_CHUNKS.len() as u64),
                "a death at store {death}"
            );
            let number = state.freeze(&marked(1), true);
            let zeros = (0..BLOCKS)
                .filter(|&b| state.read(number, b) == Some(Read::Zero))
                .count() as u64;
            assert_eq!(zeros, 7 * HOLE_BLOCKS + 8, "a death at store {death}");
        }
    }
    assert!(deaths > 0);
}

/// Only whole chunks are holes: a range that covers a chunk in part
/// names none of it, ranges past the volume are cut at its end, and the
/// last chunk, shorter than the others, is a hole where all of it is.
#[test]
fn a_chunk_is_a_hole_only_where_all_of_it_is() {
    let bits = hole_bits(BLOCKS, &[(1, 40), (64, 15), (192, 100), (500, 3)]);
    let chunks: Vec<u64> = (0..hole_chunks(BLOCKS))
        .filter(|&c| (bits[(c / 64) as usize] >> (c % 64)) & 1 == 1)
        .collect();
    assert_eq!(chunks, vec![1, 12]);
    assert_eq!(hole_words(BLOCKS), 1);
}

/// A region of version 2, as the code of version 2 wrote it
/// (`state/region-v2.txt`), is read as it was written: the same held
/// cuts, the same reads of every block and the same figures, with no
/// hole set; and it goes on as a state of version 2.
#[test]
fn a_region_of_version_2_is_read_as_it_was_written() {
    let text = include_str!("region-v2.txt");
    let mut region = None;
    let mut cuts = Vec::new();
    let mut reads = BTreeMap::new();
    let (mut figures, mut count) = (None, None);
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split(' ').collect();
        let n = |i: usize| f[i].parse::<u64>().expect("a number");
        match f[0] {
            "len" => region = Some(MemRegion::new(n(1) as usize)),
            "w" => {
                let value = u64::from_str_radix(f[2], 16).expect("hex");
                region
                    .as_mut()
                    .expect("len first")
                    .store(n(1) as usize, value);
            }
            "cut" => cuts.push((n(1), n(2), f[3].to_owned(), f[4] == "true")),
            "r" => {
                let read = match f[3] {
                    "i" => Read::Image,
                    copy => Read::Copy(copy[1..].parse().expect("a slot")),
                };
                reads.insert((n(1), n(2)), read);
            }
            "figures" => figures = Some((n(1), n(2), n(3), n(4))),
            "count" => count = Some(n(1)),
            other => panic!("a line {other}"),
        }
    }
    let region = region.expect("a region");
    let bound = 64 * BLOCK_SIZE;
    assert_eq!(region.words(), words_of(2, BLOCKS, records_for(bound)));
    assert_eq!(region.load(at::VERSION), 2);
    // Read as it is, with nothing finished: as the code of version 2
    // left it.
    let state = CutState::view(region.clone(), SIZE, bound, OWNER, Mutants(0))
        .map_err(|(e, _)| e)
        .expect("version 2 is read");
    let held: Vec<(u64, u64, String, bool)> = state
        .held()
        .iter()
        .map(|c| (c.number, c.lo, format!("{:?}", c.phase), c.itself))
        .collect();
    assert_eq!(held, cuts);
    for (number, ..) in &cuts {
        for block in 0..BLOCKS {
            let want = reads
                .get(&(*number, block))
                .copied()
                .unwrap_or(Read::Unchanged);
            assert_eq!(
                state.read(*number, block),
                Some(want),
                "cut {number}, block {block}"
            );
        }
    }
    let f = state.figures();
    assert_eq!(Some((f.held, f.copied, f.bound, f.abandoned)), figures);
    assert_eq!((f.holes, f.hole_chunks), (Holes::None, 0));
    assert_eq!(Some(state.count()), count);
    assert_eq!(state.mark(), Some(marked(9)));
    // It goes on: taken up, cut 5's flush is the start's sync; cut 4 is
    // uploaded, and cut 5 is next, with its map.
    let mut state = state.take_up(*b"a later start...");
    assert!(state.release(4));
    assert_eq!(
        state.held().first().map(|c| (c.number, c.phase)),
        Some((5, Phase::Uploading))
    );
    assert_eq!(state.map(5), vec![1, 129]);
    assert_eq!(region.load(at::VERSION), 2, "still of version 2");
    assert_eq!(state.slice_from(5, 0, 10).map(|s| s.zeroes), Some(vec![]));
}

/// A state begun in a region of a version 2 state's length is of
/// version 2, with no hole set whatever the image's holes; one begun
/// in a region of version 3's length has the set.
#[test]
fn a_state_begun_in_a_region_of_version_2s_length_is_of_version_2() {
    let bound = 2 * BLOCK_SIZE;
    let region = MemRegion::new(words_of(2, BLOCKS, records_for(bound)));
    let state = CutState::begin_with(
        region.clone(),
        SIZE,
        bound,
        OWNER,
        Mutants(0),
        None,
        Some(&hole_ranges()),
    );
    assert_eq!(region.load(at::VERSION), 2);
    assert_eq!(state.figures().holes, Holes::None);
    let mut state = CutState::restore(region, SIZE, bound, OWNER, Mutants(0))
        .map_err(|(e, _)| e)
        .expect("read");
    let number = state.freeze(&marked(1), true);
    assert!((0..BLOCKS).all(|b| state.read(number, b) == Some(Read::Image)));

    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let state = CutState::begin_with(
        region.clone(),
        SIZE,
        bound,
        OWNER,
        Mutants(0),
        None,
        Some(&hole_ranges()),
    );
    assert_eq!(region.load(at::VERSION), VERSION);
    assert_eq!(state.figures().holes, Holes::Held);
}

/// A state begun from an intent map, with only what it names counted as
/// changed, has no hole set whatever holes it is given: its first cut
/// is uploaded onto the newest committed cut, not as the whole volume.
#[test]
fn a_state_begun_from_an_intent_map_has_no_hole_set() {
    let bound = 2 * BLOCK_SIZE;
    let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
    let state = CutState::begin_with(
        region,
        SIZE,
        bound,
        OWNER,
        Mutants(0),
        Some(&[(0, 64)]),
        Some(&hole_ranges()),
    );
    assert_eq!(state.figures().holes, Holes::None);
}

/// The words of the state of version 2 in `state/region-v2.txt`.
fn region_v2_words() -> Vec<u64> {
    let mut words = Vec::new();
    for line in include_str!("region-v2.txt")
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty())
    {
        let f: Vec<&str> = line.split(' ').collect();
        match f[0] {
            "len" => words = vec![0; f[1].parse().expect("a length")],
            "w" => {
                let at: usize = f[1].parse().expect("a word's place");
                words[at] = u64::from_str_radix(f[2], 16).expect("hex");
            }
            _ => {}
        }
    }
    words
}

/// A named region that holds a state of version 2, at that version's
/// length, is opened at its own length and kept, with every word as it
/// was, so that the start can take it up (M2.4e). A file of version 2 at
/// another length, and no file, are made at this version's length; the
/// version found is said either way.
#[cfg(target_os = "linux")]
#[test]
fn a_named_region_of_version_2_is_kept_at_its_own_length() {
    let old = region_v2_words();
    let bound = 64 * BLOCK_SIZE;
    let records = records_for(bound);
    assert_eq!(old.len(), words_of(2, BLOCKS, records));
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("cuts");
    let hold = crate::inplace::HoldCuts {
        region: crate::inplace::CutRegion::Named(path.clone()),
        boot: BOOT,
        bound: Some(bound),
        intent_region: None,
        intent_header_sync: false,
    };
    let open = || Taken::open(&hold, SIZE, OWNER.volume, None, [9; 16]).expect("the region");
    let bytes: Vec<u8> = old.iter().flat_map(|word| word.to_le_bytes()).collect();

    std::fs::write(&path, &bytes).expect("the file");
    let taken = open();
    assert!(taken.kept);
    assert_eq!(taken.found_version, Some(2));
    assert_eq!(taken.region.words(), old.len());
    for (at, &word) in old.iter().enumerate() {
        assert_eq!(taken.region.load(at), word, "word {at}");
    }
    drop(taken);

    std::fs::write(&path, &bytes[..bytes.len() - 8]).expect("the file");
    let taken = open();
    assert!(!taken.kept);
    assert_eq!(taken.found_version, Some(2));
    assert_eq!(taken.region.words(), words_for(BLOCKS, records));
    drop(taken);

    std::fs::remove_file(&path).expect("removed");
    let taken = open();
    assert!(!taken.kept);
    assert_eq!(taken.found_version, None);
    assert_eq!(taken.region.words(), words_for(BLOCKS, records));
}

//! The intent map's model test (M2.3): the map and the cut state as the
//! store drives them, on a map file that a power cut can leave as a
//! volatile cache allows, with the power cut at every prefix of the
//! file's and the volume's writes in random programs.
//!
//! After every power cut, a start reads the map. Where it takes the map
//! up, every block written after the instant of the newest cut uploaded
//! (released here, as the test-hooks build's command does until M2.4)
//! must lie in a region the map names: the next cut holds it. Where it
//! does not, the whole volume is the next cut, which holds everything.
//! The test also counts the restarts that took a map up and named fewer
//! than every region, so that it shows the map narrowing something.
//!
//! Two volumes: a small one written anywhere, and one of 4200 regions,
//! written in a few regions at each end, whose bits lie in more than
//! the first sector of their page (a page torn in its first sector
//! loses its number, which is checked whatever its checksum says).

use std::collections::{BTreeMap, BTreeSet};
use std::io;

use skyzfs_blk::BLOCK_SIZE;
use skyzfs_uberblock::{State, ROOT_WORDS};

use super::*;
use crate::inplace::state::{records_for, words_for, Copier, CutState, MemRegion, Owner, Phase};

const REGION_BLOCKS: u64 = 64;
const ID: [u8; 16] = *b"the model's disk";
const T1: [u8; 16] = *b"the first start.";
const T2: [u8; 16] = *b"a start between.";
const SECTOR: usize = 512;

/// A volume of the model: its regions, the last one short, and where
/// its programs write.
#[derive(Debug, Clone, Copy)]
struct Volume {
    regions: u64,
    /// The regions written: every one, or these few.
    hot: Option<&'static [u64]>,
}

impl Volume {
    fn blocks(&self) -> u64 {
        REGION_BLOCKS * (self.regions - 1) + 20
    }

    fn shape(&self) -> Shape {
        Shape {
            identity: ID,
            region_blocks: REGION_BLOCKS,
            blocks: self.blocks(),
        }
    }
}

const SMALL: Volume = Volume {
    regions: 12,
    hot: None,
};

const LARGE: Volume = Volume {
    regions: 4200,
    hot: Some(&[0, 1, 2, 4100, 4101, 4150, 4199]),
};

/// A copier that keeps nothing: the content of cuts is the cut state's
/// own test's to show; this one is about which blocks a cut holds.
struct NoCopies;

impl Copier for NoCopies {
    fn read_image(&mut self, _block: u64, _buf: &mut [u8]) -> io::Result<()> {
        Ok(())
    }

    fn write_copy(&mut self, _slot: u32, _buf: &[u8]) -> io::Result<()> {
        Ok(())
    }
}

/// splitmix64: the programs' and the power cuts' choices.
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

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The map's file. A write a completed sync covered is on the disk; a
/// later one, at a power cut, is there, gone, or torn: written with one
/// sector left neither old nor new (zeros here).
#[derive(Debug, Clone, Default)]
struct MapFile {
    durable: Vec<u8>,
    issued: Vec<(u64, Vec<u8>)>,
}

impl MapFile {
    fn write(&mut self, at: u64, image: &[u8]) {
        self.issued.push((at, image.to_vec()));
    }

    fn sync(&mut self) {
        for (at, image) in std::mem::take(&mut self.issued) {
            put(&mut self.durable, at, &image);
        }
    }

    /// What a power cut leaves.
    fn after_power_cut(&self, rng: &mut Rng) -> Vec<u8> {
        let mut bytes = self.durable.clone();
        for (at, image) in &self.issued {
            match rng.below(3) {
                0 => put(&mut bytes, *at, image),
                1 => {}
                _ => {
                    let mut torn = image.clone();
                    let sector = rng.below((PAGE / SECTOR) as u64) as usize * SECTOR;
                    torn[sector..sector + SECTOR].fill(0);
                    put(&mut bytes, *at, &torn);
                }
            }
        }
        bytes
    }
}

fn put(bytes: &mut Vec<u8>, at: u64, image: &[u8]) {
    let end = at as usize + image.len();
    if bytes.len() < end {
        bytes.resize(end, 0);
    }
    bytes[at as usize..end].copy_from_slice(image);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Write blocks `from .. from + n`.
    Write(u64, u64),
    /// A cut's instant, and its flush, which completes.
    Cut,
    /// The cut being uploaded is committed and let go.
    Release,
}

fn program(volume: Volume, rng: &mut Rng, len: usize) -> Vec<Step> {
    let blocks = volume.blocks();
    (0..len)
        .map(|_| match rng.below(10) {
            0..=5 => {
                let from = match volume.hot {
                    None => rng.below(blocks),
                    Some(hot) => {
                        let region = hot[rng.below(hot.len() as u64) as usize];
                        (region * REGION_BLOCKS + rng.below(REGION_BLOCKS)).min(blocks - 1)
                    }
                };
                let most = if rng.below(4) == 0 { 140 } else { 8 };
                let n = 1 + rng.below(most);
                Step::Write(from, n)
            }
            6 | 7 => Step::Cut,
            _ => Step::Release,
        })
        .collect()
}

/// The power cut came: the run stops here.
#[derive(Debug)]
struct PowerCut;

/// One instance: the cut state and the map as the store keeps them,
/// and what the oracle needs.
struct Run {
    volume: Volume,
    state: CutState<MemRegion>,
    map: IntentMap,
    file: MapFile,
    mutants: Mutants,
    /// The I/O events so far, and the one the power cut comes after.
    events: u64,
    cut_after: u64,
    /// For each block written, the epochs in which a write of it was
    /// handed to the disk.
    written: BTreeMap<u64, BTreeSet<u64>>,
    /// The newest cut committed in object storage: its number.
    released: Option<u64>,
}

impl Run {
    fn new(volume: Volume, mutants: u128, cut_after: u64) -> Self {
        let bound = 64 * BLOCK_SIZE;
        let blocks = volume.blocks();
        let region = MemRegion::new(words_for(blocks, records_for(bound)));
        let owner = Owner {
            volume: ID,
            boot: [1; 16],
            start: T1,
        };
        Self {
            volume,
            state: CutState::begin(region, blocks * BLOCK_SIZE, bound, owner, Mutants(mutants)),
            map: IntentMap::new(volume.shape(), T1, Mutants(mutants)),
            file: MapFile::default(),
            mutants: Mutants(mutants),
            events: 0,
            cut_after,
            written: BTreeMap::new(),
            released: None,
        }
    }

    /// An I/O event: the power cut may come before it.
    fn event(&mut self) -> Result<(), PowerCut> {
        if self.events >= self.cut_after {
            return Err(PowerCut);
        }
        self.events += 1;
        Ok(())
    }

    fn write_pages(&mut self) -> Result<(), PowerCut> {
        for (at, image) in self.map.write_begin() {
            self.event()?;
            self.file.write(at, &image);
        }
        Ok(())
    }

    fn sync(&mut self) -> Result<(), PowerCut> {
        self.map.sync_begin();
        self.event()?;
        self.file.sync();
        self.map.synced(true);
        Ok(())
    }

    /// The start: the new map written whole and synced before it serves.
    fn start(&mut self) -> Result<(), PowerCut> {
        self.write_pages()?;
        self.sync()
    }

    fn step(&mut self, step: Step) -> Result<(), PowerCut> {
        match step {
            Step::Write(from, n) => {
                let n = n.min(self.volume.blocks() - from);
                if !self.map.covers(from, n) {
                    self.map.set(from, n);
                    if !self.map.covers(from, n) {
                        self.write_pages()?;
                        self.sync()?;
                    }
                }
                let _ = self.state.change(from, n, &mut NoCopies);
                // The write is handed to the disk.
                self.event()?;
                let epoch = self.state.count() + 1;
                for block in from..from + n {
                    self.written.entry(block).or_default().insert(epoch);
                }
            }
            Step::Cut => {
                let number = self.state.count() + 1;
                let state = State {
                    txg: 100 + number,
                    root: [number; ROOT_WORDS],
                };
                self.state.freeze(&state, true);
                self.state.flushed(number);
            }
            Step::Release => {
                let Some(oldest) = self.state.held().first().copied() else {
                    return Ok(());
                };
                if oldest.phase != Phase::Uploading {
                    return Ok(());
                }
                let early = self.mutants.has(Mutants::INTENT_CLEARED_BEFORE_COMMIT);
                if early {
                    self.clear_for(oldest.number);
                    self.write_pages()?;
                }
                // The commit in object storage: from here the oracle
                // asks for every block written after this cut's instant.
                self.event()?;
                self.released = Some(oldest.number);
                self.state.release(oldest.number);
                self.map.commit(Base {
                    number: oldest.number,
                    group: oldest.state.txg,
                });
                if !early {
                    self.clear_for(oldest.number);
                }
                self.write_pages()?;
            }
        }
        Ok(())
    }

    /// Clear the bits of the regions in which nothing changed after cut
    /// `number`'s instant.
    fn clear_for(&mut self, number: u64) {
        let changed: Option<BTreeSet<u64>> = self.state.changed_after(number).map(|words| {
            words
                .iter()
                .map(|&w| w as u64 * 64 / REGION_BLOCKS)
                .collect()
        });
        self.map.clear(|first, _| match &changed {
            None => true,
            Some(regions) => regions.contains(&(first / REGION_BLOCKS)),
        });
    }

    /// The blocks the next cut must hold: those written after the
    /// newest committed cut's instant (epochs above its number).
    fn must_hold(&self) -> Vec<u64> {
        let after = self.released.unwrap_or(0);
        self.written
            .iter()
            .filter(|(_, epochs)| epochs.iter().any(|&e| e > after))
            .map(|(&block, _)| block)
            .collect()
    }
}

/// What a start after a power cut does with the map it reads.
#[derive(Debug, PartialEq, Eq)]
enum Taken {
    /// The whole volume: no map taken up.
    Whole(Distrust),
    /// The regions the map names.
    Named(Vec<u64>),
}

fn take_up(volume: Volume, bytes: &[u8], found: [u8; 16], mutants: Mutants) -> Taken {
    match IntentMap::read(bytes, volume.shape(), mutants).and_then(|map| {
        map.trust(Some(found))?;
        Ok(map)
    }) {
        Ok(map) => Taken::Named(map.named()),
        Err(why) => Taken::Whole(why),
    }
}

/// Run `steps` with the power cut after event `cut_after` (or none),
/// then check the start after it. Whether the map was taken up and
/// named fewer than every region.
fn check(
    volume: Volume,
    steps: &[Step],
    mutants: u128,
    cut_after: u64,
    rng: &mut Rng,
) -> Result<(bool, u64), String> {
    let mut run = Run::new(volume, mutants, cut_after);
    let mut stopped = run.start().is_err();
    if !stopped {
        for &step in steps {
            if run.step(step).is_err() {
                stopped = true;
                break;
            }
        }
    }
    let bytes = run.file.after_power_cut(rng);
    let must = run.must_hold();
    let narrowed = match take_up(volume, &bytes, T1, run.mutants) {
        Taken::Whole(_) => false,
        Taken::Named(named) => {
            let named: BTreeSet<u64> = named.into_iter().collect();
            if let Some(block) = must
                .iter()
                .find(|&&b| !named.contains(&(b / REGION_BLOCKS)))
            {
                return Err(format!(
                    "block {block} was written after cut {:?}'s instant and its region is not \
                     named (power cut after event {cut_after}, {} events run, stopped {stopped})",
                    run.released, run.events
                ));
            }
            (named.len() as u64) < volume.regions
        }
    };
    Ok((narrowed, run.events))
}

/// Every prefix of `steps`, each power cut with `tries` outcomes of the
/// writes no sync covered. Whether any start narrowed.
fn every_prefix(
    volume: Volume,
    steps: &[Step],
    mutants: u128,
    seed: u64,
    tries: u64,
) -> Result<bool, String> {
    let mut rng = Rng(seed);
    let (_, total) = check(volume, steps, mutants, u64::MAX, &mut rng)?;
    let mut narrowed = false;
    for cut_after in 0..=total {
        for _ in 0..tries {
            narrowed |= check(volume, steps, mutants, cut_after, &mut rng)?.0;
        }
    }
    Ok(narrowed)
}

/// Every prefix of the programs of `seeds` on both volumes; how many
/// programs narrowed.
fn programs(seeds: std::ops::Range<u64>, mutants: u128) -> Result<u64, String> {
    let mut narrowed = 0;
    for volume in [SMALL, LARGE] {
        for seed in seeds.clone() {
            let steps = program(volume, &mut Rng(seed), 40);
            if every_prefix(volume, &steps, mutants, seed ^ 0x5eed, 3)
                .map_err(|e| format!("{} regions, seed {seed}: {e}", volume.regions))?
            {
                narrowed += 1;
            }
        }
    }
    Ok(narrowed)
}

/// The rule of M2.3's row: with the power cut at every prefix of random
/// programs and the map's file left as a power cut leaves it (one from
/// before a write no sync covered included), the next cut holds every
/// block that differs from the newest uploaded cut.
#[test]
fn after_a_power_cut_at_any_point_the_next_cut_holds_every_changed_block() {
    let narrowed = programs(0..60, 0).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        narrowed > 60,
        "only {narrowed} of 120 programs ever narrowed"
    );
}

/// Each defect of the map's own rules fails the model test.
#[test]
fn the_model_test_notices_each_defect_of_the_map() {
    for mutant in [
        Mutants::INTENT_WRITE_UNCOVERED,
        Mutants::INTENT_CLEARED_CHANGED,
        Mutants::INTENT_CLEARED_BEFORE_COMMIT,
        Mutants::INTENT_TORN_TRUSTED,
    ] {
        assert!(
            programs(0..60, mutant).is_err(),
            "the defect {mutant:#x} passed"
        );
    }
}

/// A start that did not keep the map came between: it wrote a block
/// whose region's bit was clear. The next start finds that start's
/// token and does not take the map up; the defect does, and misses the
/// block.
#[test]
fn a_map_is_not_taken_up_after_a_start_that_did_not_keep_it() {
    for (mutants, whole) in [(0, true), (Mutants::INTENT_TOKEN_UNCHECKED, false)] {
        let mut run = Run::new(SMALL, mutants, u64::MAX);
        let steps = [Step::Write(0, 1), Step::Cut, Step::Release];
        run.start().ok();
        for step in steps {
            run.step(step).ok();
        }
        assert_eq!(run.map.named(), Vec::<u64>::new(), "every region cleared");
        // The start between: token T2, block 70 written, no map kept.
        run.file.sync();
        let bytes = run.file.durable.clone();
        match take_up(SMALL, &bytes, T2, Mutants(mutants)) {
            Taken::Whole(why) => {
                assert!(whole, "mutants {mutants:#x}");
                assert_eq!(why, Distrust::Token);
            }
            Taken::Named(named) => {
                assert!(!whole, "mutants {mutants:#x}");
                assert!(
                    !named.contains(&(70 / REGION_BLOCKS)),
                    "the defect misses block 70"
                );
            }
        }
    }
}

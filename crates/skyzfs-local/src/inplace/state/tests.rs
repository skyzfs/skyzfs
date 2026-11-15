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
};
/// Where a program's changes begin: twelve blocks, some at the edges of
/// a map's words, so that programs change the same blocks often.
const STARTS: [u64; 12] = [0, 1, 2, 62, 63, 64, 65, 126, 127, 128, 190, 197];

/// The home image and the copies file, one word for each block: what a
/// death leaves as it was.
#[derive(Debug, Clone, Default)]
struct Disk {
    image: Vec<u64>,
    copies: BTreeMap<u32, u64>,
}

impl Copier for Disk {
    fn read_image(&mut self, block: u64, buf: &mut [u8]) -> io::Result<()> {
        buf[..8].copy_from_slice(&self.image[block as usize].to_le_bytes());
        Ok(())
    }

    fn write_copy(&mut self, slot: u32, buf: &[u8]) -> io::Result<()> {
        let mut word = [0u8; 8];
        word.copy_from_slice(&buf[..8]);
        self.copies.insert(slot, u64::from_le_bytes(word));
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
    /// The image the next upload goes onto: `None` where nothing was
    /// uploaded or a hole was.
    uploaded: Vec<Option<u64>>,
    rules: Rules,
    flushing: VecDeque<u64>,
    abandoned: u64,
    failed: bool,
    next_value: u64,
    deaths: u64,
}

impl World {
    fn new(mutants: u64, bound_blocks: u64) -> Self {
        let bound = bound_blocks * BLOCK_SIZE;
        let region = MemRegion::new(words_for(BLOCKS, records_for(bound)));
        let state = CutState::begin(region.clone(), SIZE, bound, OWNER, Mutants(mutants));
        Self {
            region,
            state,
            disk: Disk {
                image: vec![0; BLOCKS as usize],
                copies: BTreeMap::new(),
            },
            mutants: Mutants(mutants),
            bound,
            snaps: BTreeMap::new(),
            discarded: vec![false; BLOCKS as usize],
            touched: vec![false; BLOCKS as usize],
            changes: vec![BTreeSet::new(); BLOCKS as usize],
            uploaded: vec![None; BLOCKS as usize],
            rules: Rules::default(),
            flushing: VecDeque::new(),
            abandoned: 0,
            failed: false,
            next_value: 1,
            deaths: 0,
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
        self.check_content()
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
            Read::Image => Some(self.disk.image[block as usize]),
            Read::Copy(slot) => Some(*self.disk.copies.get(&slot).ok_or_else(|| {
                format!("cut {number}, block {block}: slot {slot} never written")
            })?),
        })
    }

    /// Upload cut `number` onto the image.
    fn upload(&mut self, number: u64) -> Result<(), String> {
        for block in self.state.map(number) {
            let read = self.read(number, block)?;
            self.uploaded[block as usize] = read;
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
                || (cut.phase == Phase::Flushing && step == Step::Cut);
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
        // The cuts abandoned: in a change, the oldest cuts held before
        // it that are lost, each once; in any other step, none.
        let lost_oldest = held
            .iter()
            .take_while(|cut| !after.iter().any(|c| c.number == cut.number))
            .count() as u64;
        let abandoned = if matches!(step, Step::Write(..) | Step::Zero(..)) {
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
        if lo < 1 && number >= 1 {
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

/// Run `steps` with a death at store `death` (counted from the
/// program's first step), if one is given.
fn run(
    steps: &[Step],
    mutants: u64,
    bound_blocks: u64,
    death: Option<u64>,
) -> Result<World, String> {
    let mut world = World::new(mutants, bound_blocks);
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
fn stores_of(steps: &[Step], mutants: u64, bound_blocks: u64) -> u64 {
    let world = World::new(mutants, bound_blocks);
    let base = world.region.stores();
    drop(world);
    match run(steps, mutants, bound_blocks, None) {
        Ok(world) => world.region.stores() - base,
        Err(_) => 0,
    }
}

/// Whether some seed below `seeds`, with no death or with one at some
/// store, fails under `mutants`.
fn caught(mutants: u64, seeds: u64, bound_blocks: u64, deaths: bool) -> Option<String> {
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
    for bound in [2, 64] {
        for seed in 0..300 {
            let steps = program(seed, 80);
            if let Err(e) = run(&steps, 0, bound, None) {
                panic!("bound {bound}, seed {seed}: {e}");
            }
        }
    }
}

/// Programs of `seeds` seeds with the daemon lost at every store of the
/// region, each in a run of its own: the next instance takes up the
/// state, and does again the step the death interrupted. The runs.
fn deaths_at_every_store(seeds: std::ops::Range<u64>) -> u64 {
    let mut runs = 0;
    for bound in [2, 64] {
        for seed in seeds.clone() {
            let steps = program(seed, 50);
            let stores = stores_of(&steps, 0, bound);
            for death in 0..stores {
                runs += 1;
                if let Err(e) = run(&steps, 0, bound, Some(death)) {
                    panic!("bound {bound}, seed {seed}, death at store {death}: {e}");
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
    let fixed = |mutants: u64| -> Option<String> {
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
    let refused = |size: u64, owner: Owner, mutants: u64| {
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
/// 130 MiB of maps and their summaries, 16 MiB of records, and a header.
#[test]
fn the_state_of_a_tebibyte_takes_146_mib() {
    const MIB: u64 = 1 << 20;
    let size = 1u64 << 40;
    let blocks = size / BLOCK_SIZE;
    let bound = default_bound(size);
    assert_eq!(bound, 4 << 30);
    let maps = words_for(blocks, 0) as u64 * 8;
    let all = words_for(blocks, records_for(bound)) as u64 * 8;
    assert_eq!(maps / MIB, 130, "maps: {maps} bytes");
    assert_eq!((all - maps) / MIB, 16, "records: {} bytes", all - maps);
    assert!(all - 146 * MIB < 4096, "header: {} bytes", all - 146 * MIB);
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

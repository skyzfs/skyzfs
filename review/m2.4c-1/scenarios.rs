//! Scenarios for the review of #117 at 5bebcc0 (M2.4c-1): the lease, the
//! backoff and the desk, as the head's own tests do not have them.
//! `scenarios.py` copies this file into `crates/skyzfs-upload/tests/` as
//! `review_scn.rs`, runs it and removes it. Nothing here is the product's.
//!
//! A scenario prints what it found on lines that begin with `##`. One that
//! asserts says so in its doc; the others only record.

mod common;

use std::cell::RefCell;
use std::io;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use skynbd_format::{Codec, RefsObject};
use skynbd_refs::{refs_key, ActingRefStore, LeaseOutcome, ObjectRefStore, RefStore};
use skynbd_store::{FaultOp, FaultStore, ObjectStore};
use skynbd_types::VolumeId;
use skyzfs_blk::Buf;
use skyzfs_local::inplace::{CutSlice, InPlace};
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::lease::Keeper;
use skyzfs_upload::remote::{Observed, Remote, RemoteError};
use skyzfs_upload::uploader::{State, Uploader};
use skyzfs_upload::{Desk, Doing, EngineRemote, LeaseState, Local, Queued, Released, Uploadable};

use common::*;

const TTL: u64 = 30_000;

// ---------------------------------------------------------------------
// What the scenarios share
// ---------------------------------------------------------------------

/// The in-place store, reached by the uploader and by the scenario: the
/// uploader owns its `Local`, and the scenario writes to the pool all the
/// same.
#[derive(Clone)]
struct Shared(Rc<RefCell<InPlace<SimDisk>>>);

impl Shared {
    fn new(store: InPlace<SimDisk>) -> Self {
        Self(Rc::new(RefCell::new(store)))
    }
}

impl skyzfs_upload::Store for Shared {
    fn uploadable(&mut self) -> Option<Uploadable> {
        skyzfs_upload::Store::uploadable(&mut *self.0.borrow_mut())
    }
    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> io::Result<CutSlice> {
        skyzfs_upload::Store::read(&mut *self.0.borrow_mut(), number, from, buf)
    }
    fn sync_copies(&mut self, number: u64) -> io::Result<()> {
        skyzfs_upload::Store::sync_copies(&mut *self.0.borrow_mut(), number)
    }
    fn release(&mut self, number: u64) -> Released {
        skyzfs_upload::Store::release(&mut *self.0.borrow_mut(), number)
    }
    fn name_committed(&mut self, number: u64, group: u64) -> bool {
        skyzfs_upload::Store::name_committed(&mut *self.0.borrow_mut(), number, group)
    }
    fn abandon_all(&mut self) -> Vec<u64> {
        skyzfs_upload::Store::abandon_all(&mut *self.0.borrow_mut())
    }
}

/// A node's clock for the lease's expiry (the refs object's), apart from
/// tokio's: two nodes' clocks can differ.
#[derive(Clone)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1_000_000_000)))
    }
    fn now(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
    fn pass(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }
    fn set(&self, ms: u64) {
        self.0.store(ms, Ordering::SeqCst);
    }
}

type Faulty = FaultStore<Recording>;
type LeaseRefs = Arc<ObjectRefStore<Faulty>>;
type Fenced = EngineRemote<Faulty, Arc<ActingRefStore<LeaseRefs>>>;

fn lease_refs(store: &Faulty, clock: &Clock) -> LeaseRefs {
    let clock = clock.clone();
    Arc::new(ObjectRefStore::new(store.clone()).with_clock(Arc::new(move || clock.now())))
}

/// The remote an uploader makes at its first take: the engine, every
/// commit fenced by `holder` at the epoch given. `made` gets each epoch.
fn fenced(
    store: &Faulty,
    vol: VolumeId,
    clock: &Clock,
    holder: &'static str,
    made: Rc<RefCell<Vec<u64>>>,
) -> impl FnMut(u64) -> Fenced {
    let (store, refs) = (store.clone(), lease_refs(store, clock));
    move |epoch| {
        made.borrow_mut().push(epoch);
        let acting = Arc::new(ActingRefStore::new(Arc::clone(&refs), holder, epoch));
        EngineRemote::new(store.clone(), acting, vol, cfg())
    }
}

/// A remote with knobs: a PUT of sealed segments that fails every so
/// often, an observe that takes its time or fails, a commit refused.
struct Tweak<R> {
    inner: R,
    knobs: Rc<Knobs>,
}

#[derive(Default)]
struct Knobs {
    /// Every `n`th call of `put_sealed` fails, once (0: none).
    put_fails_every: AtomicU64,
    puts: AtomicU64,
    put_failures: AtomicU64,
    /// `observe` sleeps this long (tokio's time) before it answers.
    observe_ms: AtomicU64,
    /// `observe` fails after its sleep.
    observe_fails: AtomicBool,
    observes: AtomicU64,
    commits: AtomicU64,
}

impl<R: Remote> Remote for Tweak<R> {
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        self.knobs.observes.fetch_add(1, Ordering::SeqCst);
        let ms = self.knobs.observe_ms.load(Ordering::SeqCst);
        if ms > 0 {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        if self.knobs.observe_fails.load(Ordering::SeqCst) {
            return Err(RemoteError::Store("the scenario's timeout".into()));
        }
        self.inner.observe().await
    }
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        self.inner.write(offset, data)
    }
    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }
    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        let n = self.knobs.puts.fetch_add(1, Ordering::SeqCst) + 1;
        let every = self.knobs.put_fails_every.load(Ordering::SeqCst);
        if every > 0 && n % every == 0 {
            self.knobs.put_failures.fetch_add(1, Ordering::SeqCst);
            return Err(RemoteError::Store("the scenario's lost PUT".into()));
        }
        self.inner.put_sealed().await
    }
    async fn commit(
        &mut self,
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<(), RemoteError> {
        self.knobs.commits.fetch_add(1, Ordering::SeqCst);
        self.inner.commit(group, expected, row).await
    }
    fn discard(&mut self) {
        self.inner.discard()
    }
    fn reset(&mut self) {
        self.inner.reset()
    }
}

/// A small generator, so that a seed names a program.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// The data blocks of the fixture's volume that no label ring covers.
fn data_block(rng: &mut Rng) -> u64 {
    let n = rng.below(56);
    if n < 28 {
        2 + n
    } else {
        66 + (n - 28)
    }
}

async fn head_of(inner: &Recording, vol: VolumeId) -> Option<skynbd_refs::RefValue> {
    ObjectRefStore::new(inner.clone())
        .get(vol, "head")
        .await
        .expect("head")
}

async fn lease_row(inner: &Recording, vol: VolumeId) -> Option<(String, u64, u64)> {
    ObjectRefStore::new(inner.clone())
        .lease_get(vol)
        .await
        .expect("the lease row")
        .map(|l| (l.holder, l.epoch, l.expires_at_ms))
}

// ---------------------------------------------------------------------
// L1: two nodes on one volume, programs from seeds
// ---------------------------------------------------------------------

type NodeUp = Uploader<Shared, Fenced, LeaseRefs, Box<dyn FnMut(u64) -> Fenced>>;

struct Node {
    name: &'static str,
    fx: Fixture,
    clock: Clock,
    store: Option<Shared>,
    up: Option<NodeUp>,
    /// The epochs this instance's uploader made a remote with.
    made: Rc<RefCell<Vec<u64>>>,
    txg: u64,
    out: bool,
    landed: u64,
    starts: u64,
}

impl Node {
    /// A node with a disk of its own. `of` gives the volume in object
    /// storage where it is another node's: this node reaches the same
    /// objects through a fault layer of its own.
    async fn new(
        name: &'static str,
        seed: u64,
        txg: u64,
        of: Option<(&Recording, VolumeId)>,
    ) -> Self {
        let mut fx = Fixture::new(seed).await;
        if let Some((inner, vol)) = of {
            fx.store = FaultStore::new(inner.clone());
            fx.vol = vol;
        }
        let store = Shared::new(fx.start());
        Self {
            name,
            fx,
            clock: Clock::new(),
            store: Some(store),
            up: None,
            made: Rc::new(RefCell::new(Vec::new())),
            txg,
            out: false,
            landed: 0,
            starts: 0,
        }
    }

    /// The daemon starts its uploader on the store it has.
    fn start_uploader(&mut self) {
        self.made = Rc::new(RefCell::new(Vec::new()));
        let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(
            &self.fx.store,
            self.fx.vol,
            &self.clock,
            self.name,
            Rc::clone(&self.made),
        ));
        let lease = Keeper::new(
            lease_refs(&self.fx.store, &self.clock),
            self.fx.vol,
            self.name,
            TTL,
        );
        let store = self.store.clone().expect("a store");
        self.up = Some(Uploader::with_slices(store, lease, make, 16));
        self.starts += 1;
    }

    /// The daemon dies and starts again: the store takes its cut state up.
    fn restart(&mut self) {
        self.up = None;
        self.store = None;
        self.fx.disk.kill();
        self.fx.region.revive();
        self.store = Some(Shared::new(self.fx.start()));
        self.start_uploader();
    }

    /// The pool writes a block and a group: a cut.
    fn cut(&mut self, rng: &mut Rng) {
        let block = data_block(rng);
        let byte = 1 + rng.below(250) as u8;
        self.txg += 1;
        let store = self.store.as_ref().expect("a store");
        let mut store = store.0.borrow_mut();
        self.fx.write(&mut store, block, byte);
        self.fx.group(&mut store, self.txg);
    }
}

#[derive(Default)]
struct Tally {
    programs: u64,
    turns: u64,
    landings: u64,
    landings_a: u64,
    landings_b: u64,
    /// A landing by one node after a landing by the other.
    changes_of_hand: u64,
    lost_states: u64,
    unsure_turns: u64,
    end_one_uploads: u64,
    /// Both nodes landed commits in the end phase.
    end_changed_hands: u64,
    end_none_holder_away: u64,
    end_none_both_running: u64,
    end_none_other: u64,
    examples: Vec<String>,
}

/// One program: two nodes with a disk each and one volume in object
/// storage, their uploaders turned in any order at any time, with cuts,
/// outages of either node, clocks that jump, deaths and clean stops.
/// Asserted after every turn in which head moved: the lease row names the
/// node that turned, at the epoch its remote was made with; head holds
/// that node's volume at the instant of the cut head's group names; fsck
/// passes; and an uploader made its remote once.
async fn two_nodes(seed: u64, same_name: bool, tally: &mut Tally) {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15 ^ (seed.wrapping_mul(0x1000_0000_01B3) + 1));
    let a = Node::new("node-a", 7_000 + seed, 1_000, None).await;
    let inner = a.fx.store.inner().clone();
    let vol = a.fx.vol;
    let b_name = if same_name { "node-a" } else { "node-b" };
    let b = Node::new(b_name, 9_000 + seed, 5_000, Some((&inner, vol))).await;
    let mut nodes = [a, b];
    nodes[0].start_uploader();
    let mut last_landing: Option<usize> = None;
    let mut last_epoch = 0u64;
    let mut trace: Vec<String> = Vec::new();

    macro_rules! turn {
        ($i:expr) => {{
            let i: usize = $i;
            if nodes[i].up.is_some() {
                let before = head_of(&inner, vol).await;
                let wait = nodes[i].up.as_mut().expect("up").turn().await;
                let after = head_of(&inner, vol).await;
                tally.turns += 1;
                let shown = nodes[i].up.as_ref().expect("up").shown();
                if matches!(shown.lease, LeaseState::Unsure { .. }) {
                    tally.unsure_turns += 1;
                }
                assert!(
                    nodes[i].made.borrow().len() <= 1,
                    "seed {seed}: {} made its remote {:?} times\n{trace:#?}",
                    nodes[i].name,
                    nodes[i].made.borrow()
                );
                if before != after {
                    let epoch = *nodes[i].made.borrow().last().expect("a remote");
                    let row = lease_row(&inner, vol).await;
                    trace.push(format!(
                        "LANDED by {} (node {i}) at epoch {epoch}; row {row:?}; lease {:?}",
                        nodes[i].name, shown.lease
                    ));
                    let named = row.as_ref().map(|(h, e, _)| (h.as_str(), *e));
                    assert_eq!(
                        named,
                        Some((nodes[i].name, epoch)),
                        "seed {seed}: a commit landed under a lease row that does not name its node and epoch\n{trace:#?}"
                    );
                    assert_eq!(
                        shown.lease.epoch(),
                        Some(epoch),
                        "seed {seed}: a commit landed for an uploader that does not hold the lease\n{trace:#?}"
                    );
                    assert!(
                        epoch >= last_epoch,
                        "seed {seed}: a commit landed under an older epoch than one before it\n{trace:#?}"
                    );
                    last_epoch = epoch;
                    let up = nodes[i].fx.uploaded().await;
                    let group = up.group.expect("head's group");
                    assert_eq!(up.row.map(|r| r.group), Some(group), "seed {seed}: the row's group");
                    let at = nodes[i].fx.at.get(&group).unwrap_or_else(|| {
                        panic!("seed {seed}: head's group {group} is no cut of {}\n{trace:#?}", nodes[i].name)
                    });
                    assert!(
                        &up.content == at,
                        "seed {seed}: head does not hold {}'s volume at the cut of group {group}{}\n{trace:#?}",
                        nodes[i].name,
                        differing(&up.content, at)
                    );
                    assert!(up.fsck, "seed {seed}: fsck");
                    tally.landings += 1;
                    nodes[i].landed += 1;
                    if i == 0 {
                        tally.landings_a += 1;
                    } else {
                        tally.landings_b += 1;
                    }
                    if last_landing.is_some_and(|l| l != i) {
                        tally.changes_of_hand += 1;
                    }
                    last_landing = Some(i);
                }
                wait
            } else {
                Duration::ZERO
            }
        }};
    }

    for step in 0..260u64 {
        let i = rng.below(2) as usize;
        match rng.below(100) {
            0..=44 => {
                // One step of the driver a turn, and a whole cut is 130
                // steps: take a random number of turns, so that a node
                // stops anywhere inside an upload and often finishes one.
                for _ in 0..1 + rng.below(160) {
                    if turn!(i) > Duration::ZERO {
                        break;
                    }
                }
            }
            45..=59 => {
                let ms = [300, 3_000, 11_000, 35_000, 70_000][rng.below(5) as usize];
                tokio::time::advance(Duration::from_millis(ms)).await;
                for node in &nodes {
                    node.clock.pass(ms);
                }
                trace.push(format!("{step}: {ms} ms pass"));
            }
            60..=74 => nodes[i].cut(&mut rng),
            75..=80 => {
                nodes[i].out = !nodes[i].out;
                if nodes[i].out {
                    nodes[i].fx.store.arm(FaultOp::Any, "", 0).await;
                } else {
                    nodes[i].fx.store.disarm().await;
                }
                trace.push(format!("{step}: node {i} out of reach: {}", nodes[i].out));
            }
            81..=84 => {
                let ms = 1_000 + rng.below(100_000);
                if rng.below(2) == 0 {
                    nodes[i].clock.pass(ms);
                    trace.push(format!("{step}: node {i}'s clock jumps {ms} ms ahead"));
                } else {
                    let now = nodes[i].clock.now();
                    nodes[i].clock.set(now - ms);
                    trace.push(format!("{step}: node {i}'s clock jumps {ms} ms back"));
                }
            }
            85..=90 => {
                if nodes[i].up.is_some() {
                    nodes[i].restart();
                    trace.push(format!("{step}: node {i} dies and starts again"));
                } else {
                    nodes[i].start_uploader();
                    trace.push(format!("{step}: node {i} starts its uploader"));
                }
            }
            91..=93 => {
                if let Some(up) = nodes[i].up.as_mut() {
                    let released = up.lease().release().await;
                    nodes[i].up = None;
                    trace.push(format!("{step}: node {i} stops, lease released: {released:?}"));
                }
            }
            _ => {}
        }
    }

    // The end: both nodes reach the store, the clocks agree, and time
    // passes with both pools writing. Who uploads?
    for node in &nodes {
        node.fx.store.disarm().await;
    }
    let now = nodes[0].clock.now().max(nodes[1].clock.now());
    for node in &nodes {
        node.clock.set(now);
    }
    trace.push("the end begins".into());
    let before: Vec<u64> = nodes.iter().map(|n| n.landed).collect();
    // Changes of hand are counted inside the end phase from here.
    last_landing = None;
    let hand_before = tally.changes_of_hand;
    for _ in 0..60 {
        for i in 0..2 {
            nodes[i].cut(&mut rng);
            for _ in 0..60 {
                if turn!(i) > Duration::ZERO {
                    break;
                }
            }
        }
        tokio::time::advance(Duration::from_secs(7)).await;
        for node in &nodes {
            node.clock.pass(7_000);
        }
    }
    let landed: Vec<u64> = nodes.iter().zip(&before).map(|(n, b)| n.landed - b).collect();
    let states: Vec<Option<State>> = nodes
        .iter()
        .map(|n| n.up.as_ref().map(|up| up.shown().state))
        .collect();
    let lost: u64 = states
        .iter()
        .filter(|s| matches!(s, Some(State::LeaseLost { .. })))
        .count() as u64;
    tally.lost_states += lost;
    tally.programs += 1;
    let running = states.iter().filter(|s| s.is_some()).count();
    let row = lease_row(&inner, vol).await;
    if landed.iter().any(|&n| n > 0) {
        tally.end_one_uploads += 1;
        if landed.iter().filter(|&&n| n > 0).count() == 2 {
            // Setting the clocks equal is itself a jump: a lease taken by
            // the clock that was behind is expired by the other, and a
            // first take that waited may take it. Once, and no more.
            tally.end_changed_hands += 1;
            if !same_name {
                assert!(
                    tally.changes_of_hand - hand_before <= 1,
                    "seed {seed}: the lease went back and forth in the end: {landed:?}\n{trace:#?}"
                );
            }
        }
    } else {
        let example = format!("seed {seed}: states {states:?}, row {row:?}");
        if running == 2 {
            tally.end_none_both_running += 1;
            tally.examples.push(example);
        } else if running == 1 && lost == 1 {
            tally.end_none_holder_away += 1;
        } else {
            tally.end_none_other += 1;
            if running > 0 {
                tally.examples.push(example);
            }
        }
    }
}

/// Asserts, in every program: see [`two_nodes`]. Records how the programs
/// end.
#[tokio::test(start_paused = true)]
async fn l1_two_nodes_never_commit_under_a_lease_that_is_not_theirs() {
    quiet_deaths();
    let mut tally = Tally::default();
    for seed in 0..400 {
        two_nodes(seed, false, &mut tally).await;
    }
    println!(
        "## L1: {} programs, {} turns, {} commits landed ({} by node-a, {} by node-b), {} changes of hand; \
         turns with the lease unsure: {}",
        tally.programs,
        tally.turns,
        tally.landings,
        tally.landings_a,
        tally.landings_b,
        tally.changes_of_hand,
        tally.unsure_turns
    );
    println!(
        "## L1: at the end (both reach the store, clocks agree, 7 minutes, both pools write): \
         somebody uploads in {} (in {} of them the lease changed hands once more, when the clocks were set equal); nobody uploads in {} because the holder's daemon is stopped and the other lost the lease, \
         in {} with both uploaders running, in {} otherwise (no uploader running, or a case below)",
        tally.end_one_uploads,
        tally.end_changed_hands,
        tally.end_none_holder_away,
        tally.end_none_both_running,
        tally.end_none_other
    );
    for example in tally.examples.iter().take(12) {
        println!("## L1:   {example}");
    }
    assert!(tally.landings > 1_000, "the programs upload: {}", tally.landings);
    assert!(tally.changes_of_hand > 20, "the lease changes hands: {}", tally.changes_of_hand);
}

/// Records: the same programs with both nodes under one holder name. The
/// asserts of [`two_nodes`] still hold (the row names the name), and say
/// nothing: the tally shows the two nodes committing in turn.
#[tokio::test(start_paused = true)]
async fn l2_two_nodes_under_one_holder_name_both_commit() {
    quiet_deaths();
    let mut tally = Tally::default();
    for seed in 0..60 {
        two_nodes(seed, true, &mut tally).await;
    }
    println!(
        "## L2 (one holder name for both nodes): {} programs, {} commits landed ({} by the first node, {} by the second), \
         {} changes of hand; at the end a node uploads in {} programs",
        tally.programs,
        tally.landings,
        tally.landings_a,
        tally.landings_b,
        tally.changes_of_hand,
        tally.end_one_uploads
    );
}

/// Records, step by step: two daemons under one holder name, both alive,
/// on one volume. Each takes the lease at the same epoch, and each lands
/// whole cuts of its own image onto the other's head.
#[tokio::test(start_paused = true)]
async fn l3_one_holder_name_on_two_nodes_step_by_step() {
    quiet_deaths();
    let mut rng = Rng(77);
    let mut a = Node::new("node-a", 7_901, 1_000, None).await;
    let inner = a.fx.store.inner().clone();
    let vol = a.fx.vol;
    let mut b = Node::new("node-a", 7_902, 5_000, Some((&inner, vol))).await;
    a.start_uploader();
    b.start_uploader();
    let mut said = Vec::new();
    for round in 0..6 {
        for (i, node) in [&mut a, &mut b].into_iter().enumerate() {
            node.cut(&mut rng);
            let before = head_of(&inner, vol).await;
            let mut states = Vec::new();
            for _ in 0..80 {
                let wait = node.up.as_mut().expect("up").turn().await;
                let state = node.up.as_ref().expect("up").shown().state;
                if states.last() != Some(&state) {
                    states.push(state);
                }
                if wait > Duration::ZERO {
                    break;
                }
            }
            let after = head_of(&inner, vol).await;
            let shown = node.up.as_ref().expect("up").shown();
            said.push(format!(
                "round {round}, node {i}: lease {:?}; head moved: {}; uploaded {} (whole {}), mismatches {}",
                shown.lease.epoch(),
                before != after,
                shown.figures.uploaded,
                shown.figures.uploaded_whole,
                shown.figures.mismatches
            ));
        }
        tokio::time::advance(Duration::from_secs(5)).await;
        a.clock.pass(5_000);
        b.clock.pass(5_000);
    }
    for line in &said {
        println!("## L3: {line}");
    }
    println!("## L3: the row at the end: {:?}", lease_row(&inner, vol).await);
}

// ---------------------------------------------------------------------
// L4: the take that comes back at a newer epoch, and the holder it took from
// ---------------------------------------------------------------------

/// A refs store that runs a step of the scenario after each read of the
/// lease row: what another node does between this holder's read and take.
struct Hooked {
    inner: LeaseRefs,
    hook: Hook,
}

type Hook = Arc<std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>>;

#[async_trait::async_trait]
impl RefStore for Hooked {
    async fn get(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Option<skynbd_refs::RefValue>> {
        self.inner.get(vol, name).await
    }
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<skynbd_refs::RefValue>,
        new_commit: Option<skynbd_types::CommitHash>,
    ) -> skynbd_refs::Result<skynbd_refs::CasOutcome> {
        self.inner.compare_and_swap(vol, name, expected, new_commit).await
    }
    async fn list(&self, vol: VolumeId) -> skynbd_refs::Result<Vec<(String, skynbd_refs::RefValue)>> {
        self.inner.list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<skynbd_refs::RefEvent> {
        self.inner.watch()
    }
    async fn transact(&self, vol: VolumeId, ops: Vec<skynbd_refs::RefOp>) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.inner.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.inner.transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Vec<String>> {
        self.inner.pins(vol, name).await
    }
    async fn meta_get(&self, vol: VolumeId, key: &str) -> skynbd_refs::Result<Option<Vec<u8>>> {
        self.inner.meta_get(vol, key).await
    }
    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
        self.inner.meta_list(vol, prefix).await
    }
    async fn lease_acquire(&self, vol: VolumeId, holder: &str, ttl_ms: u64) -> skynbd_refs::Result<LeaseOutcome> {
        self.inner.lease_acquire(vol, holder, ttl_ms).await
    }
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> skynbd_refs::Result<()> {
        self.inner.lease_release(vol, holder).await
    }
    async fn lease_get(&self, vol: VolumeId) -> skynbd_refs::Result<Option<skynbd_refs::Lease>> {
        let row = self.inner.lease_get(vol).await;
        let hook = self.hook.lock().expect("the hook").take();
        if let Some(hook) = hook {
            hook();
        }
        row
    }
}

/// Records: node A's renewal after an outage reads its own row; node B, a
/// daemon that starts just then, takes the expired lease; then B cannot
/// reach the store for longer than the lease's time, so it does not renew
/// (or A's clock is that far ahead of B's); A's take lands. What each
/// node holds afterwards. B is a real keeper, and B is alive.
#[tokio::test(start_paused = true)]
async fn l4_a_take_at_a_newer_epoch_costs_both_nodes_the_lease() {
    let fx = Fixture::new(7_910).await;
    let clock = Clock::new();
    let refs = lease_refs(&fx.store, &clock);
    let hook: Hook = Arc::default();
    let hooked = Hooked {
        inner: Arc::clone(&refs),
        hook: Arc::clone(&hook),
    };
    let mut a = Keeper::new(hooked, fx.vol, "node-a", TTL);
    println!("## L4: A's first take: {:?}", a.keep().await);
    // A's outage, longer than the lease.
    clock.pass(2 * TTL);
    let b = Arc::new(std::sync::Mutex::new(Keeper::new(
        Arc::clone(&refs),
        fx.vol,
        "node-b",
        TTL,
    )));
    let b_first: Arc<std::sync::Mutex<Option<LeaseState>>> = Arc::default();
    {
        let (b, b_first, clock) = (Arc::clone(&b), Arc::clone(&b_first), clock.clone());
        *hook.lock().expect("the hook") = Some(Box::new(move || {
            // Between A's read and A's take: B's first take (the store is
            // in memory and answers at once), then time with no renewal
            // by B.
            let mut b = b.lock().expect("b");
            let state = futures_util::FutureExt::now_or_never(b.keep())
                .expect("a store in memory answers at once")
                .clone();
            *b_first.lock().expect("b's first") = Some(state);
            clock.pass(2 * TTL);
        }));
    }
    let a_state = a.keep().await.clone();
    println!(
        "## L4: B's first take, between A's read and A's take: {:?}",
        b_first.lock().expect("b's first")
    );
    println!("## L4: A after its renewal: {a_state:?}");
    println!("## L4: the row: {:?}", lease_row(fx.store.inner(), fx.vol).await);
    let b_state = {
        let mut b = b.lock().expect("b");
        b.keep().await.clone()
    };
    println!("## L4: B at its next renewal: {b_state:?}");
    clock.pass(10 * TTL);
    let a_later = a.keep().await.clone();
    let b_later = {
        let mut b = b.lock().expect("b");
        b.keep().await.clone()
    };
    println!("## L4: ten lease times later: A {a_later:?}; B {b_later:?}");
    println!("## L4: the row then: {:?}", lease_row(fx.store.inner(), fx.vol).await);
    assert!(matches!(a_state, LeaseState::Lost { epoch: 3, .. }));
    assert!(matches!(b_state, LeaseState::Lost { epoch: 3, .. }));
}

// ---------------------------------------------------------------------
// L5: the lease row gone under an uploader that holds it (G7 of #114)
// ---------------------------------------------------------------------

/// Remove the lease row from the volume's refs object, leaving head and
/// the cut's row: what a tool that rewrites the object, or a mistake,
/// would do.
async fn drop_lease_row(inner: &Recording, vol: VolumeId) {
    let key = refs_key(vol);
    let bytes = inner.get(&key, None).await.expect("the refs object");
    let mut obj = RefsObject::decode(&bytes).expect("a refs object");
    assert!(obj.owner.is_some(), "the row is there to remove");
    obj.owner = None;
    inner
        .put(&key, obj.encode().expect("encodes").into())
        .await
        .expect("the refs object put");
}

/// Records: an uploader holds the lease and has uploaded a cut; the lease
/// row goes; the pool makes another cut. How often the cut goes up before
/// the uploader stops, and what stops it.
#[tokio::test(start_paused = true)]
async fn l5_the_lease_row_gone_under_an_uploader() {
    quiet_deaths();
    let mut rng = Rng(5);
    let mut a = Node::new("node-a", 7_920, 1_000, None).await;
    let inner = a.fx.store.inner().clone();
    let vol = a.fx.vol;
    a.start_uploader();
    a.cut(&mut rng);
    for _ in 0..100 {
        if a.up.as_mut().expect("up").turn().await > Duration::ZERO {
            break;
        }
    }
    let first = a.up.as_ref().expect("up").shown();
    assert_eq!(first.figures.uploaded, 1, "the first cut went up");
    let bytes_before = a.fx.segment_bytes().await;
    drop_lease_row(&inner, vol).await;
    for _ in 0..40 {
        a.cut(&mut rng);
    }
    let began = tokio::time::Instant::now();
    let mut log: Vec<(u64, State)> = Vec::new();
    let mut begun = 0;
    while began.elapsed() < Duration::from_secs(120) {
        let wait = a.up.as_mut().expect("up").turn().await;
        let state = a.up.as_ref().expect("up").shown().state;
        if matches!(state, State::Working(Doing::Reading, _))
            && !matches!(log.last(), Some((_, State::Working(Doing::Reading, _))))
        {
            begun += 1;
        }
        if log.last().map(|(_, s)| s) != Some(&state) {
            log.push((began.elapsed().as_millis() as u64, state));
        }
        let wait = wait.max(Duration::from_millis(1));
        tokio::time::advance(wait).await;
        a.clock.pass(wait.as_millis() as u64);
    }
    for (ms, state) in &log {
        println!("## L5: at {ms} ms: {state:?}");
    }
    let shown = a.up.as_ref().expect("up").shown();
    println!(
        "## L5: uploads begun after the row went: {begun}; commits given up: {}; errors: {}; \
         segment bytes in the store: {} before, {} after; the lease: {:?}; the row: {:?}",
        shown.figures.gave_up,
        shown.figures.errors,
        bytes_before,
        a.fx.segment_bytes().await,
        shown.lease,
        lease_row(&inner, vol).await
    );
    assert!(
        matches!(shown.state, State::LeaseLost { ref holder, epoch: 0 } if holder.is_empty()),
        "the uploader stops: {:?}",
        shown.state
    );
    assert_eq!(a.fx.uploaded().await.group, Some(1_001), "nothing landed after the row went");
}

// ---------------------------------------------------------------------
// B1: a long upload over a link that loses a PUT now and then
// ---------------------------------------------------------------------

/// Records: one cut of 64 blocks read a block a slice, on a store where
/// every `every`th PUT of sealed segments fails once and every other
/// call succeeds. The waits, and how long the upload takes.
async fn flaky(every: u64) -> (u64, u64, Vec<u64>, u64) {
    let mut fx = Fixture::new(7_930 + every).await;
    let mut store = fx.start();
    for n in 0..28 {
        fx.write(&mut store, 2 + n, 0x40 + n as u8);
        fx.write(&mut store, 66 + n, 0x80 + n as u8);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let knobs = Rc::new(Knobs::default());
    knobs.put_fails_every.store(every, Ordering::SeqCst);
    let made = Rc::new(RefCell::new(Vec::new()));
    let mut inner_make = fenced(&fx.store, fx.vol, &clock, "node-a", made);
    let make = {
        let knobs = Rc::clone(&knobs);
        move |epoch| Tweak {
            inner: inner_make(epoch),
            knobs: Rc::clone(&knobs),
        }
    };
    let lease = Keeper::new(lease_refs(&fx.store, &clock), fx.vol, "node-a", TTL);
    let mut up = Uploader::with_slices(store, lease, make, 1);
    let began = tokio::time::Instant::now();
    let mut waits = Vec::new();
    let mut errors = 0;
    for _ in 0..100_000 {
        let wait = up.turn().await;
        let shown = up.shown();
        if shown.figures.uploaded == 1 {
            break;
        }
        // A wait begins at each step that fails.
        if shown.figures.errors != errors {
            errors = shown.figures.errors;
            match shown.state {
                State::Backoff { wait, .. } => waits.push(wait.as_secs()),
                other => panic!("a failed step and no wait: {other:?}"),
            }
        }
        let wait = wait.max(Duration::from_millis(1));
        tokio::time::advance(wait).await;
        clock.pass(wait.as_millis() as u64);
    }
    assert_eq!(up.shown().figures.uploaded, 1, "the cut went up");
    fx.assert_uploaded(7, "the flaky link").await;
    (
        began.elapsed().as_secs(),
        knobs.put_failures.load(Ordering::SeqCst),
        waits,
        knobs.puts.load(Ordering::SeqCst),
    )
}

#[tokio::test(start_paused = true)]
async fn b1_a_long_upload_over_a_link_that_loses_a_put_now_and_then() {
    for every in [0u64, 50, 20, 8] {
        let (secs, failures, waits, puts) = flaky(every).await;
        println!(
            "## B1: every {every}th PUT call of {puts} fails ({failures} failures): the cut of 60 data blocks and 4 slots \
             is uploaded after {secs} s of tokio's time; waits (s): {waits:?}"
        );
    }
}

// ---------------------------------------------------------------------
// B2: a cut found committed resets the wait
// ---------------------------------------------------------------------

/// Asserts: after two steps that fail (waits of 1 s and 2 s), the check
/// finds the cut committed (its commit had landed before a death), and
/// the next failure waits 1 s again.
#[tokio::test(start_paused = true)]
async fn b2_a_cut_found_committed_resets_the_wait() {
    quiet_deaths();
    let mut fx = Fixture::new(7_940).await;
    let driver = committed_unheard(&mut fx).await;
    fx.died(driver);
    let store = fx.start();
    let clock = Clock::new();
    let knobs = Rc::new(Knobs::default());
    let made = Rc::new(RefCell::new(Vec::new()));
    let mut inner_make = fenced(&fx.store, fx.vol, &clock, "node-a", made);
    let make = {
        let knobs = Rc::clone(&knobs);
        move |epoch| Tweak {
            inner: inner_make(epoch),
            knobs: Rc::clone(&knobs),
        }
    };
    let lease = Keeper::new(lease_refs(&fx.store, &clock), fx.vol, "node-a", TTL);
    let mut up = Uploader::with_slices(store, lease, make, 16);
    knobs.observe_fails.store(true, Ordering::SeqCst);
    let mut waits = Vec::new();
    for _ in 0..2 {
        let wait = up.turn().await;
        assert!(matches!(up.shown().state, State::Backoff { .. }), "{:?}", up.shown().state);
        waits.push(wait.as_secs());
        tokio::time::advance(wait).await;
    }
    assert_eq!(waits, [1, 2]);
    knobs.observe_fails.store(false, Ordering::SeqCst);
    up.turn().await;
    let shown = up.shown();
    assert_eq!(shown.figures.found_committed, 1, "{:?}", shown.state);
    knobs.observe_fails.store(true, Ordering::SeqCst);
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 9);
    }
    let mut next = None;
    for _ in 0..10 {
        let wait = up.turn().await;
        if matches!(up.shown().state, State::Backoff { .. }) {
            next = Some(wait.as_secs());
            break;
        }
    }
    println!("## B2: waits before the cut was found committed: {waits:?}; the wait after: {next:?}");
    assert_eq!(next, Some(1), "found committed is a success");
}

// ---------------------------------------------------------------------
// B3: the lease is kept while the uploader waits out a long backoff
// ---------------------------------------------------------------------

/// Asserts: with every step failing for twenty minutes (the waits reach
/// five minutes), the lease row never passes its expiry: the uploader
/// turns for the lease during a wait.
#[tokio::test(start_paused = true)]
async fn b3_the_lease_is_kept_during_a_long_wait() {
    let mut fx = Fixture::new(7_950).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let knobs = Rc::new(Knobs::default());
    knobs.observe_fails.store(true, Ordering::SeqCst);
    let made = Rc::new(RefCell::new(Vec::new()));
    let mut inner_make = fenced(&fx.store, fx.vol, &clock, "node-a", made);
    let make = {
        let knobs = Rc::clone(&knobs);
        move |epoch| Tweak {
            inner: inner_make(epoch),
            knobs: Rc::clone(&knobs),
        }
    };
    let lease = Keeper::new(lease_refs(&fx.store, &clock), fx.vol, "node-a", TTL);
    let mut up = Uploader::with_slices(store, lease, make, 16);
    let began = tokio::time::Instant::now();
    let mut longest = Duration::ZERO;
    let mut least_left = u64::MAX;
    while began.elapsed() < Duration::from_secs(1_200) {
        let wait = up.turn().await;
        if let State::Backoff { wait: w, .. } = up.shown().state {
            longest = longest.max(w);
        }
        let (_, _, expires) = lease_row(fx.store.inner(), fx.vol).await.expect("the row");
        let wait = wait.max(Duration::from_millis(1));
        tokio::time::advance(wait).await;
        clock.pass(wait.as_millis() as u64);
        let left = expires.saturating_sub(clock.now());
        least_left = least_left.min(left);
        assert!(
            expires > clock.now(),
            "the lease ran out during a wait of {wait:?} at {:?}",
            began.elapsed()
        );
    }
    println!(
        "## B3: twenty minutes of failing steps: the longest wait {longest:?}; the least time left on the lease before a turn: {least_left} ms; observes: {}",
        knobs.observes.load(Ordering::SeqCst)
    );
    assert_eq!(longest, Duration::from_secs(300));
}

// ---------------------------------------------------------------------
// C1: Codex's finding: the wait counted from the start of the turn
// ---------------------------------------------------------------------

/// Records: the store answers each read of head after `slow_ms` and with
/// an error (a timeout). When the lease is renewed, against when it runs
/// out. The lease's clock follows tokio's here, so that time that passes
/// inside a step passes for the lease too.
async fn slow_steps(slow_ms: u64) -> (Vec<u64>, u64) {
    let mut fx = Fixture::new(7_960).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let start = tokio::time::Instant::now();
    let now = move || 1_000_000_000 + start.elapsed().as_millis() as u64;
    let refs: LeaseRefs = Arc::new(ObjectRefStore::new(fx.store.clone()).with_clock(Arc::new(now)));
    let knobs = Rc::new(Knobs::default());
    knobs.observe_fails.store(true, Ordering::SeqCst);
    knobs.observe_ms.store(slow_ms, Ordering::SeqCst);
    let make = {
        let (knobs, store, vol, refs) = (Rc::clone(&knobs), fx.store.clone(), fx.vol, Arc::clone(&refs));
        move |epoch| Tweak {
            inner: EngineRemote::new(
                store.clone(),
                Arc::new(ActingRefStore::new(Arc::clone(&refs), "node-a", epoch)),
                vol,
                cfg(),
            ),
            knobs: Rc::clone(&knobs),
        }
    };
    let lease = Keeper::new(Arc::clone(&refs), fx.vol, "node-a", TTL);
    let mut up = Uploader::with_slices(store, lease, make, 16);
    // As `Uploader::run` does: turn, then sleep the wait.
    let mut kept_at = Vec::new();
    let mut last_expiry = 0;
    let mut least_left = u64::MAX;
    while start.elapsed() < Duration::from_secs(600) {
        // What is left of the lease when this turn begins.
        if let Some((_, _, expires)) = lease_row(fx.store.inner(), fx.vol).await {
            least_left = least_left.min(expires.saturating_sub(now()));
        }
        let wait = up.turn().await;
        let (_, _, expires) = lease_row(fx.store.inner(), fx.vol).await.expect("the row");
        if expires != last_expiry {
            kept_at.push(expires - TTL - 1_000_000_000);
            last_expiry = expires;
        }
        tokio::time::sleep(wait).await;
    }
    (kept_at, least_left)
}

#[tokio::test(start_paused = true)]
async fn c1_a_slow_step_delays_the_renewal_by_the_wait_as_well() {
    for slow_ms in [0u64, 5_000, 15_000, 19_000, 21_000, 25_000] {
        let (kept_at, least_left) = slow_steps(slow_ms).await;
        let gaps: Vec<u64> = kept_at.windows(2).map(|w| w[1] - w[0]).collect();
        let longest = gaps.iter().copied().max().unwrap_or(0);
        println!(
            "## C1: steps that fail after {slow_ms} ms: the longest time between two renewals {longest} ms \
             (the lease's time is {TTL} ms); the least left on the lease when a turn began: {least_left} ms; \
             first renewals at (ms): {:?}",
            &kept_at[..kept_at.len().min(8)]
        );
    }
}

// ---------------------------------------------------------------------
// The desk: a queue's thread under the scenario's control
// ---------------------------------------------------------------------

struct QueueCtl {
    /// The turns of the queue's thread.
    turns: AtomicU64,
    /// Set while the thread is inside `Desk::answer`.
    answering: AtomicBool,
    /// The longest time one `Desk::answer` took, in microseconds.
    longest_us: AtomicU64,
    /// The thread answers nothing while the count of answers is at this.
    stall_at: AtomicU64,
}

fn controlled_queue(
    desk: Arc<Desk>,
    mut store: InPlace<SimDisk>,
    woken: std::sync::mpsc::Receiver<()>,
    ctl: Arc<QueueCtl>,
) -> std::thread::JoinHandle<InPlace<SimDisk>> {
    std::thread::spawn(move || loop {
        let _ = woken.recv_timeout(Duration::from_millis(2));
        ctl.turns.fetch_add(1, Ordering::SeqCst);
        if desk.closed() {
            return store;
        }
        if desk.answered() == ctl.stall_at.load(Ordering::SeqCst) {
            continue;
        }
        let began = std::time::Instant::now();
        ctl.answering.store(true, Ordering::SeqCst);
        desk.answer(&mut store);
        ctl.answering.store(false, Ordering::SeqCst);
        let us = began.elapsed().as_micros() as u64;
        ctl.longest_us.fetch_max(us, Ordering::SeqCst);
    })
}

fn new_ctl() -> Arc<QueueCtl> {
    Arc::new(QueueCtl {
        turns: AtomicU64::new(0),
        answering: AtomicBool::new(false),
        longest_us: AtomicU64::new(0),
        stall_at: AtomicU64::new(u64::MAX),
    })
}

/// Records, and asserts that the upload ends: the cut read a copy, so the
/// uploader asks for the sync of the copies, and the copies file's sync is
/// held as a slow disk holds it. What the queue's thread does meanwhile.
#[tokio::test]
async fn d1_the_sync_of_the_copies_is_waited_for_on_the_queues_thread() {
    let mut fx = Fixture::new(7_970).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    // Block 10 changes after the cut: the cut reads it from its copy.
    fx.write(&mut store, 10, 0x11);
    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let ctl = new_ctl();
    let queue = controlled_queue(Arc::clone(&desk), store, woken, Arc::clone(&ctl));
    let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let lease = Keeper::new(refs, fx.vol, "node-a", TTL);
    let mut remote = Some(fx.remote());
    let make = move |_epoch| remote.take().expect("made once");
    let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 16);

    fx.disk.hold_syncs_of("copies.img");
    let disk = fx.disk.clone();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let upload = async {
        let mut states = Vec::new();
        for _ in 0..400 {
            let wait = up.turn().await;
            let state = up.shown().state;
            if states.last() != Some(&state) {
                states.push(state.clone());
            }
            if state == State::Idle {
                break;
            }
            tokio::time::sleep(wait).await;
        }
        states
    };
    let watch = async {
        // Wait until the queue's thread is inside an answer and a caller
        // waits at the end of the held sync.
        let mut waited = 0;
        while !(ctl.answering.load(Ordering::SeqCst) && disk.held_waiters() == 1) {
            tokio::time::sleep(Duration::from_millis(5)).await;
            waited += 5;
            assert!(waited < 20_000, "the sync of the copies was never asked for");
        }
        let turns = ctl.turns.load(Ordering::SeqCst);
        let began = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(500)).await;
        seen.lock().expect("seen").push(format!(
            "with the copies file's sync held for {} ms: the queue's thread is inside Desk::answer: {}; \
             callers waiting at the end of a held sync: {}; turns of the queue's thread meanwhile: {}",
            began.elapsed().as_millis(),
            ctl.answering.load(Ordering::SeqCst),
            disk.held_waiters(),
            ctl.turns.load(Ordering::SeqCst) - turns
        ));
        disk.release_syncs();
    };
    let (states, ()) = tokio::join!(upload, watch);
    for line in seen.lock().expect("seen").iter() {
        println!("## D1: {line}");
    }
    println!(
        "## D1: the longest single Desk::answer took {} ms; the uploader's states: {states:?}",
        ctl.longest_us.load(Ordering::SeqCst) / 1_000
    );
    fx.assert_uploaded(7, "once the sync is let go").await;
    desk.close();
    queue.join().expect("the queue's thread");
}

/// Records: `Uploader::run` is stopped while a read waits at the desk (the
/// queue's thread is busy), and then run again, as a caller might that
/// stops and starts the uploads. What the second run does.
#[tokio::test]
async fn d2_a_run_stopped_at_a_read_and_run_again() {
    let mut fx = Fixture::new(7_980).await;
    let mut store = fx.start();
    for n in 0..20 {
        fx.write(&mut store, 2 + n, 0x50 + n as u8);
    }
    fx.group(&mut store, 7);
    let (wake, woken) = std::sync::mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let ctl = new_ctl();
    // The queue's thread answers two calls (the cut to upload, the first
    // slice) and then is busy.
    ctl.stall_at.store(2, Ordering::SeqCst);
    let queue = controlled_queue(Arc::clone(&desk), store, woken, Arc::clone(&ctl));
    let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let lease = Keeper::new(refs, fx.vol, "node-a", TTL);
    let mut remote = Some(fx.remote());
    let make = move |_epoch| remote.take().expect("made once");
    let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 4);

    let first = RefCell::new(Vec::new());
    {
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel::<()>();
        let run = up.run(
            async {
                let _ = stop_rx.await;
            },
            |shown| first.borrow_mut().push(shown.state),
        );
        let stopper = async {
            let mut waited = 0;
            while desk.answered() < 2 {
                tokio::time::sleep(Duration::from_millis(2)).await;
                waited += 2;
                assert!(waited < 20_000, "two calls are answered");
            }
            // The second slice's read is at the desk by now.
            tokio::time::sleep(Duration::from_millis(100)).await;
            let _ = stop_tx.send(());
        };
        let began = std::time::Instant::now();
        tokio::join!(run, stopper);
        println!(
            "## D2: the first run ended {} ms after it began, with a read waiting at the desk; states published: {:?}",
            began.elapsed().as_millis(),
            first.borrow()
        );
    }
    // The queue's thread is free again.
    ctl.stall_at.store(u64::MAX, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(50)).await;
    println!("## D2: calls answered before the second run: {}", desk.answered());

    let second = RefCell::new(Vec::new());
    let turns = std::cell::Cell::new(0u64);
    {
        let run = up.run(tokio::time::sleep(Duration::from_millis(1_500)), |shown| {
            turns.set(turns.get() + 1);
            let mut second = second.borrow_mut();
            if second.len() < 12 {
                second.push(shown.state);
            }
        });
        run.await;
    }
    let shown = up.shown();
    println!(
        "## D2: the second run, 1.5 s: {} turns; its first states: {:?}",
        turns.get(),
        second.borrow()
    );
    println!(
        "## D2: after it: state {:?}; uploaded {}; cuts lost {}; checks answered by the desk in all: {}; head's group: {:?}",
        shown.state,
        shown.figures.uploaded,
        shown.figures.lost,
        desk.answered(),
        fx.uploaded().await.group
    );
    desk.close();
    let store = queue.join().expect("the queue's thread");
    println!(
        "## D2: the store still holds {} cut(s)",
        store.held_cuts().len()
    );
}

/// Asserts: a queue's thread that turns only when woken (it does not look
/// at the desk on its own) answers every call. 20 000 calls; a call that
/// was left unanswered for 200 ms counts as a wake lost.
#[tokio::test]
async fn d3_no_wake_is_lost_with_a_queue_that_turns_only_when_woken() {
    let mut fx = Fixture::new(7_990).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let (wake, woken) = std::sync::mpsc::channel::<()>();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let late = Arc::new(AtomicU64::new(0));
    let queue = {
        let (desk, late) = (Arc::clone(&desk), Arc::clone(&late));
        std::thread::spawn(move || loop {
            let woke = woken.recv_timeout(Duration::from_millis(200)).is_ok();
            if desk.closed() {
                return store;
            }
            // A turn. A queue that was not woken has nothing to do, and
            // finds a call only if its wake was lost.
            if desk.answer(&mut store) && !woke {
                late.fetch_add(1, Ordering::SeqCst);
            }
        })
    };
    let mut q = Queued::new(Arc::clone(&desk));
    let began = std::time::Instant::now();
    let mut asked = 0u64;
    while asked < 20_000 && began.elapsed() < Duration::from_secs(60) {
        assert!(q.uploadable().await.is_some());
        asked += 1;
    }
    desk.close();
    queue.join().expect("the queue's thread");
    println!(
        "## D3: {asked} calls in {} ms; wakes lost: {}",
        began.elapsed().as_millis(),
        late.load(Ordering::SeqCst)
    );
    assert_eq!(late.load(Ordering::SeqCst), 0, "a wake was lost");
    assert_eq!(asked, 20_000);
}

/// Asserts: the desk closes at any moment of an upload, on another thread
/// than the one that answers; the uploader always comes to rest, and what
/// object storage holds is nothing or the cut, whole. 300 runs.
#[tokio::test]
async fn d4_a_desk_closed_at_any_moment_of_an_upload() {
    let mut nothing = 0;
    let mut whole = 0;
    let mut not_held = 0;
    let mut lost = 0;
    for run in 0..300u64 {
        let mut fx = Fixture::new(8_000 + run).await;
        let mut store = fx.start();
        for n in 0..12 {
            fx.write(&mut store, 2 + n, 0x50 + n as u8);
        }
        fx.group(&mut store, 7);
        let (wake, woken) = std::sync::mpsc::channel();
        let desk = Desk::new(move || {
            let _ = wake.send(());
        });
        let ctl = new_ctl();
        let queue = controlled_queue(Arc::clone(&desk), store, woken, Arc::clone(&ctl));
        let refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
        let lease = Keeper::new(refs, fx.vol, "node-a", TTL);
        let mut remote = Some(fx.remote());
        let make = move |_epoch| remote.take().expect("made once");
        let mut up = Uploader::with_slices(Queued::new(Arc::clone(&desk)), lease, make, 2);
        // Close after 0 to 139 answers, from a third thread: the cut is
        // the first of its state, so it holds the whole volume, 128
        // slices of two blocks, and its release is the 131st call.
        let closer = {
            let desk = Arc::clone(&desk);
            let after = (run * 7) % 140;
            let jitter = run % 5;
            std::thread::spawn(move || {
                while desk.answered() < after {
                    std::thread::yield_now();
                }
                std::thread::sleep(Duration::from_micros(50 * jitter));
                desk.close();
            })
        };
        let rest = tokio::time::timeout(Duration::from_secs(20), async {
            let mut idle = 0;
            loop {
                up.turn().await;
                if up.shown().state == State::Idle && desk.closed() {
                    idle += 1;
                    if idle == 3 {
                        return;
                    }
                }
            }
        })
        .await;
        assert!(rest.is_ok(), "run {run}: the uploader does not come to rest after the close");
        closer.join().expect("the closer");
        queue.join().expect("the queue's thread");
        let shown = up.shown();
        let uploaded = fx.uploaded().await;
        match uploaded.group {
            None => nothing += 1,
            Some(7) => {
                fx.assert_uploaded(7, "closed at any moment").await;
                whole += 1;
            }
            other => panic!("run {run}: head's group {other:?}"),
        }
        not_held += shown.figures.not_held;
        lost += shown.figures.lost;
    }
    println!(
        "## D4: 300 uploads with the desk closed after 0 to 139 answers: nothing landed in {nothing}, the cut landed whole in {whole}; \
         figures over all runs: cuts counted as lost {lost}, cuts counted as committed and gone from the store {not_held}"
    );
}

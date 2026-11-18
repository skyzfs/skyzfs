//! Scenarios of the review of #114 (M2.4b-2) that the head's tests do not
//! have. Copied into `crates/skyzfs-upload/tests/` of a checkout of the
//! head by `scenarios.py`, run there, and removed. Each prints what it
//! saw; the asserts are the review's own expectations.

mod common;

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use common::*;
use futures_util::FutureExt;
use skynbd_store::{FaultOp, FaultWhen};
use skyzfs_blk::{BlockTarget, Buf, Op, Request, RequestId};
use skyzfs_local::inplace::{
    lock, CutPhase, CutRegion, HoldCuts, InPlace, IntentAtStart, Lost, OpenOptions, Opened,
};
use skyzfs_local::sim::{BySector, Fate, SimDisk};
use skyzfs_uberblock::{seal, ByteOrder, Labels, MAGIC};
use skyzfs_upload::{Checked, Doing, Mismatch, Rule, Step};

const INTENT: &str = "intent.map";
const OTHER_BOOT: [u8; 16] = *b"another boot....";

fn told(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter(|step| !matches!(step, Step::Read { .. }))
        .cloned()
        .collect()
}

async fn step_until(up: &mut Up, doing: Doing) {
    for _ in 0..400 {
        if up.doing() == doing {
            return;
        }
        let step = up.step().await.expect("a step");
        assert_ne!(step, Step::Idle, "at rest before {doing:?}");
    }
    panic!("{doing:?} not reached");
}

/// A start as after a restart of the machine: another boot, a device it
/// creates, and the region as the last boot left it (not trusted).
fn start_in_boot(fx: &Fixture, boot: [u8; 16], acknowledge: bool) -> InPlace<SimDisk> {
    let options = OpenOptions {
        acknowledge_failure: acknowledge,
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(fx.region.clone()),
            boot,
            bound: Some(BOUND),
            intent_region: Some(64 * BLOCK),
        }),
        ..OpenOptions::default()
    };
    let opened = Opened::open(fx.disk.clone(), lock(&fx.disk).expect("the lock"), options)
        .map_err(|(e, _)| e)
        .expect("opens");
    opened.start(None).expect("the in-place store")
}

/// Run `op` on `store` and wait for it.
fn request(store: &mut InPlace<SimDisk>, id: &mut u64, op: Op) {
    *id += 1;
    let rid = RequestId((1 << 51) | *id);
    store.submit(Request { id: rid, op }).expect("submitted");
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, Duration::from_secs(60));
    }
    done.pop().expect("a completion").result.expect("the request");
}

/// A 4 KiB slot holding an uberblock of group `txg` (as the fixture's).
fn uberblock(txg: u64) -> Buf {
    let mut slot = Buf::filled(1, 0);
    let mut put = |at: usize, word: u64| slot[at..at + 8].copy_from_slice(&word.to_le_bytes());
    put(0, MAGIC);
    put(8, 5000);
    put(16, txg);
    for n in 0..16 {
        put(40 + 8 * n, (txg * 31).rotate_left(n as u32) ^ 0x5a5a);
    }
    slot
}

/// The slot of group `txg` in label `label`, sealed, and where it goes.
fn slot_of(label: usize, txg: u64) -> (u64, Buf) {
    let ring = Labels::for_size(SIZE)
        .rings()
        .nth(label)
        .expect("four labels")
        .start;
    let slots = skyzfs_uberblock::RING_SIZE / BLOCK;
    let offset = ring + (txg % slots) * BLOCK;
    let mut slot = uberblock(txg);
    seal(&mut slot, offset, ByteOrder::Little);
    (offset, slot)
}

/// Group `txg`'s uberblock to the first label alone, then the flush:
/// a cut of group `txg` that names one region of the intent map and not
/// all four, as the fixture's group to four labels does on a volume of
/// four regions.
fn group_one_label(fx: &mut Fixture, store: &mut InPlace<SimDisk>, txg: u64, id: &mut u64) {
    let (offset, slot) = slot_of(0, txg);
    let at = offset as usize;
    fx.now[at..at + BLOCK as usize].copy_from_slice(&slot);
    request(store, id, Op::Write { offset, data: slot });
    request(store, id, Op::Flush);
    fx.at.insert(txg, fx.now.clone());
}

/// The volume as the store reads it now.
fn volume_now(store: &mut InPlace<SimDisk>, id: &mut u64) -> Vec<u8> {
    let mut all = Vec::with_capacity(SIZE as usize);
    for block in 0..BLOCKS {
        *id += 1;
        let rid = RequestId((1 << 50) | *id);
        store
            .submit(Request {
                id: rid,
                op: Op::Read {
                    offset: block * BLOCK,
                    buf: Buf::filled(1, 0xEE),
                },
            })
            .expect("submitted");
        let mut done = Vec::new();
        while done.is_empty() {
            store.wait(&mut done, Duration::from_secs(60));
        }
        let done = done.pop().expect("a completion");
        done.result.expect("the read");
        all.extend_from_slice(&done.buf.expect("the buffer"));
    }
    all
}

// ---------------------------------------------------------------------
// S1. The crash matrix, counted: the operations on object storage of the
// run, and the stores into the cut state.
// ---------------------------------------------------------------------

fn two_cuts(fx: &mut Fixture) -> InPlace<SimDisk> {
    let mut store = fx.start();
    for block in 60..100 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    fx.write(&mut store, 61, 0xA1);
    fx.write(&mut store, 70, 0x70);
    fx.group(&mut store, 8);
    fx.write(&mut store, 70, 0xB0);
    store
}

#[tokio::test]
async fn s1_the_matrix_counted() {
    quiet_deaths();
    // The run with no death: its operations, its steps, and the stores
    // into the cut state after each step.
    let mut fx = Fixture::new(20).await;
    let store = two_cuts(&mut fx);
    let mut up = fx.driver(store, 16);
    fx.store.inner().record(true);
    let before = fx.region.stores();
    let mut at_step = Vec::new();
    loop {
        let ops = fx.store.inner().log().len();
        let step = up.step().await.expect("a step");
        if step == Step::Idle {
            break;
        }
        at_step.push((
            format!("{step:?}"),
            ops,
            fx.store.inner().log().len(),
            fx.region.stores() - before,
        ));
    }
    fx.store.inner().record(false);
    let ops = fx.store.inner().log();
    println!("S1 operations on object storage: {}", ops.len());
    for (i, op) in ops.iter().enumerate() {
        println!("S1   op {i:2}: {op}");
    }
    let count = |what: &str| ops.iter().filter(|op| *op == what).count();
    println!(
        "S1 put seg {}, put node {}, put commit {}, put_if refs {}, get_versioned refs {}, others {}",
        count("put seg"),
        count("put node"),
        count("put commit"),
        count("put_if refs"),
        count("get_versioned refs"),
        ops.len()
            - count("put seg")
            - count("put node")
            - count("put commit")
            - count("put_if refs")
            - count("get_versioned refs"),
    );
    for (step, from, to, stores) in &at_step {
        println!("S1   step {step}: operations {from}..{to}, stores so far {stores}");
    }
    let stores = fx.region.stores() - before;
    println!("S1 stores into the cut state during the run: {stores}");

    // A death at each store: what the restart found.
    let mut points = 0;
    let mut held_for_the_row = Vec::new();
    for n in 0.. {
        let mut fx = Fixture::new(20).await;
        let store = two_cuts(&mut fx);
        let mut up = fx.driver(store, 16);
        fx.region.die_at(n);
        let ran = AssertUnwindSafe(up.run()).catch_unwind().await;
        let died = match ran {
            Ok(_) => false,
            Err(payload) if payload.downcast_ref::<Lost>().is_some() => true,
            Err(payload) => std::panic::resume_unwind(payload),
        };
        if !died {
            break;
        }
        points += 1;
        let left = fx.uploaded().await;
        fx.died(up);
        let store = fx.start();
        let held: Vec<(u64, CutPhase)> = store
            .held_cuts()
            .iter()
            .map(|cut| (cut.number, cut.phase))
            .collect();
        let base = store.committed_cut().map(|cut| cut.number);
        let mut up = fx.driver(store, 16);
        let steps = up.run().await.expect("the run after the restart");
        if up.figures().found_committed > 0 {
            held_for_the_row.push(n);
        }
        println!(
            "S1   store {n:2}: head's group {:?}, held at the restart {held:?}, the map's base {base:?}, then {:?}",
            left.group,
            told(&steps)
        );
        assert_eq!(up.figures().mismatches, 0, "store {n}");
    }
    println!("S1 deaths at a store into the cut state: {points}; the cut held for the row to name at {held_for_the_row:?}");
    assert_eq!(points, stores, "a death at each store of the run");

    // A death at each operation, in both modes.
    for when in [FaultWhen::Before, FaultWhen::After] {
        let mut points = 0;
        for k in 0.. {
            let mut fx = Fixture::new(20).await;
            let store = two_cuts(&mut fx);
            let mut up = fx.driver(store, 16);
            fx.store.arm_when(FaultOp::Any, "", k, when).await;
            let ran = up.run().await;
            fx.store.disarm().await;
            if ran.is_ok() {
                break;
            }
            points += 1;
        }
        println!("S1 deaths at an operation, {when:?}: {points}");
        assert_eq!(points, ops.len());
    }
}

// ---------------------------------------------------------------------
// S2. The naming's write of the map fails. What the same instance does
// next, against what `Step::UploadedNotHeld::named` says of it.
// ---------------------------------------------------------------------

#[tokio::test]
async fn s2_a_naming_whose_write_fails() {
    let mut fx = Fixture::new(31).await;
    let mut up = committed_then_taken(&mut fx).await;
    let before = up.local().committed_cut().map(|cut| (cut.number, cut.group));
    fx.disk.fail_write_of(INTENT);
    let step = up.step().await;
    println!("S2 the step: {step:?}");
    println!(
        "S2 the volume failed: {}; the map's base in memory before {before:?}, after {:?}",
        up.local().is_failed(),
        up.local().committed_cut().map(|cut| (cut.number, cut.group))
    );
    let steps = up.run().await.expect("the run after");
    println!("S2 then: {:?}", told(&steps));
    let now = fx.uploaded().await;
    println!(
        "S2 head's group {:?}, the row {:?}, head reads as the volume at cut 3's instant: {}",
        now.group,
        now.row.map(|row| (row.number, row.group)),
        now.content == fx.at[&9]
    );
    println!(
        "S2 figures: mismatches {}, named {}, kept {}, doing {:?}",
        up.figures().mismatches,
        up.figures().named,
        up.figures().kept,
        up.doing()
    );
    // The same with the file refusing only once: the naming fails, and
    // the release after it writes the map.
    let mut fx = Fixture::new(32).await;
    let mut up = committed_then_taken(&mut fx).await;
    fx.disk.fail_write_of(INTENT);
    let step = up.step().await;
    fx.disk.clear_faults();
    let steps = up.run().await.expect("the run after");
    println!("S2 (the file takes writes again) the step: {step:?}; then {:?}", told(&steps));
    println!(
        "S2 (the file takes writes again) the map's base {:?}, held {:?}, failed {}",
        up.local().committed_cut().map(|cut| (cut.number, cut.group)),
        up.local().held_cuts().len(),
        up.local().is_failed()
    );
    fx.assert_uploaded(9, "S2").await;
}

// ---------------------------------------------------------------------
// S3. The bound takes a WHOLE cut after its commit has landed. The map
// names it; how does its heir go up?
// ---------------------------------------------------------------------

#[tokio::test]
async fn s3_a_whole_cut_taken_after_its_commit() {
    let mut fx = Fixture::new(33).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    step_until(&mut up, Doing::Committing).await;
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
        .await;
    assert!(up.step().await.is_err());
    fx.store.disarm().await;
    assert_eq!(fx.uploaded().await.group, Some(7), "cut 1's swap landed");
    for block in 0..OVER {
        fx.write(up.local(), 100 + block, 0x33);
    }
    fx.group(up.local(), 8);
    let held: Vec<u64> = up.local().held_cuts().iter().map(|c| c.number).collect();
    println!("S3 held after the bound took cut 1: {held:?}");
    let step = up.step().await.expect("the step");
    println!(
        "S3 the step: {step:?}; the map's base {:?}",
        up.local().committed_cut().map(|cut| (cut.number, cut.group))
    );
    let steps = up.run().await.expect("the heir");
    println!("S3 then: {:?}", told(&steps));
    println!(
        "S3 figures: uploaded {}, whole {}, blocks {}",
        up.figures().uploaded,
        up.figures().uploaded_whole,
        up.figures().blocks
    );
    fx.assert_uploaded(8, "S3").await;
}

// ---------------------------------------------------------------------
// S4. A restart of the machine under the uploader: the new state's
// first cut holds the regions the intent map names, and goes up as a
// delta onto the cut the map names, where the row is that cut; else a
// mismatch and a whole cut. Head reads as the volume at the cut's
// instant either way.
// ---------------------------------------------------------------------

/// Cut the power, start in another boot, and take the volume as it
/// reads now for the fixture's.
fn reboot(
    fx: &mut Fixture,
    up: Up,
    boot: [u8; 16],
    cut: impl skyzfs_local::sim::Cut,
    id: &mut u64,
) -> InPlace<SimDisk> {
    drop(up);
    fx.disk.power_cut(cut);
    fx.region.revive();
    let mut store = start_in_boot(fx, boot, false);
    fx.now = volume_now(&mut store, id);
    store
}

#[tokio::test]
async fn s4_a_restart_of_the_machine_under_the_uploader() {
    let mut id = 0;
    // (a) Nothing special: two cuts uploaded, more written, the power cut.
    let mut fx = Fixture::new(41).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    fx.write(up.local(), 20, 0x20);
    fx.group(up.local(), 8);
    up.run().await.expect("cut 2");
    fx.write(up.local(), 30, 0x30);
    fx.write(up.local(), 200, 0xC8);
    fx.group(up.local(), 9);
    fx.write(up.local(), 201, 0xC9);
    let store = reboot(&mut fx, up, OTHER_BOOT, skyzfs_local::sim::KeepAll, &mut id);
    println!(
        "S4a after the reboot: cuts {:?}, the map {:?}, its base {:?}",
        store.cuts_at_start(),
        store.intent_at_start(),
        store.committed_cut().map(|cut| (cut.number, cut.group))
    );
    let mut up = fx.driver(store, 64);
    fx.group(up.local(), 10);
    let steps = up.run().await.expect("the first cut of the new state");
    println!("S4a then: {:?}", told(&steps));
    fx.assert_uploaded(10, "S4a").await;
    assert!(told(&steps).contains(&Step::Began {
        number: 1,
        rule: Rule::Delta
    }));

    // (b) A cut the bound took after its commit, named, then the power
    // cut that keeps every write.
    let mut fx = Fixture::new(42).await;
    let mut up = committed_then_taken(&mut fx).await;
    assert_eq!(
        up.step().await,
        Ok(Step::UploadedNotHeld {
            number: 2,
            named: true
        })
    );
    fx.write(up.local(), 240, 0xF0);
    let store = reboot(&mut fx, up, OTHER_BOOT, skyzfs_local::sim::KeepAll, &mut id);
    println!(
        "S4b after the reboot: the map {:?}, its base {:?}",
        store.intent_at_start(),
        store.committed_cut().map(|cut| (cut.number, cut.group))
    );
    let mut up = fx.driver(store, 64);
    fx.group(up.local(), 10);
    let steps = up.run().await.expect("the first cut of the new state");
    println!("S4b then: {:?}", told(&steps));
    fx.assert_uploaded(10, "S4b").await;
    assert!(told(&steps).contains(&Step::Began {
        number: 1,
        rule: Rule::Delta
    }));

    // (c) The same, and the power cut loses what the map's file had not
    // synced: the header that names cut 2.
    let mut fx = Fixture::new(43).await;
    let mut up = committed_then_taken(&mut fx).await;
    assert_eq!(
        up.step().await,
        Ok(Step::UploadedNotHeld {
            number: 2,
            named: true
        })
    );
    let lose_the_map = BySector(|name: Option<&str>, _| {
        if name == Some(INTENT) {
            Fate::Durable
        } else {
            Fate::Latest
        }
    });
    let store = reboot(&mut fx, up, OTHER_BOOT, lose_the_map, &mut id);
    println!(
        "S4c after the reboot: the map {:?}, its base {:?}",
        store.intent_at_start(),
        store.committed_cut().map(|cut| (cut.number, cut.group))
    );
    let mut up = fx.driver(store, 64);
    fx.group(up.local(), 10);
    let steps = up.run().await.expect("the check");
    println!("S4c then: {:?}", told(&steps));
    assert!(matches!(
        told(&steps).first(),
        Some(Step::Mismatch {
            why: Mismatch::NotTheBase,
            ..
        })
    ));
    fx.group(up.local(), 11);
    let steps = up.run().await.expect("the whole cut");
    println!("S4c then: {:?}", told(&steps));
    fx.assert_uploaded(11, "S4c").await;
}

// ---------------------------------------------------------------------
// S5. Random programs under the uploader, with cuts the bound takes
// after their commit, and a power cut that keeps or loses each unsynced
// sector of each file on its own. After the restart of the machine, and
// for two cuts after it, head reads as the volume at each cut's instant
// whenever a cut is uploaded: a delta that lacks a block shows here.
// ---------------------------------------------------------------------

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

/// What a program saw, for the summary.
#[derive(Default, Debug)]
struct Tally {
    programs: u64,
    named: u64,
    named_whole: u64,
    reboots_taken_up: u64,
    reboots_narrowed: u64,
    reboots_begun: u64,
    first_delta: u64,
    first_mismatch: u64,
    first_whole: u64,
    uploads_checked: u64,
}

/// Run the driver until it rests, and check head against the volume at
/// the instant of the newest cut uploaded, if any was.
async fn run_and_check(fx: &Fixture, up: &mut Up, tally: &mut Tally, case: &str) -> Vec<Step> {
    let steps = up
        .run()
        .await
        .unwrap_or_else(|e| panic!("{case}: the run: {e}"));
    let uploaded = steps.iter().any(|step| {
        matches!(
            step,
            Step::Uploaded { .. } | Step::UploadedNotHeld { .. } | Step::FoundCommitted { .. }
        )
    });
    if uploaded {
        let now = fx.uploaded().await;
        let txg = now.group.expect("a commit with a group");
        fx.assert_uploaded(txg, case).await;
        tally.uploads_checked += 1;
    }
    steps
}

async fn program(seed: u64, one_label: bool, tally: &mut Tally) {
    let case = format!("seed {seed}, one label {one_label}");
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut fx = Fixture::new(1000 + seed).await;
    let mut id = 0;
    let mut txg = 6;
    let store = fx.start();
    let mut up = fx.driver(store, 16 << rng.below(3));
    let ops = 4 + rng.below(10);
    for _ in 0..ops {
        match rng.below(8) {
            // A few writes.
            0..=2 => {
                for _ in 0..1 + rng.below(6) {
                    let block = rng.below(BLOCKS);
                    fx.write(up.local(), block, 1 + rng.below(250) as u8);
                }
            }
            // A group.
            3 | 4 => {
                txg += 1;
                make_group(&mut fx, &mut up, txg, one_label, &mut id);
            }
            // The uploader runs to its rest.
            5 => {
                run_and_check(&fx, &mut up, tally, &case).await;
            }
            // The uploader takes a few steps.
            6 => {
                for _ in 0..rng.below(6) {
                    up.step().await.expect("a step");
                }
            }
            // A cut with more blocks than the bound, its commit landed
            // unheard, and the bound takes it before its release.
            _ => {
                run_and_check(&fx, &mut up, tally, &case).await;
                let at = rng.below(BLOCKS - OVER);
                let byte = 1 + rng.below(250) as u8;
                for block in 0..OVER {
                    fx.write(up.local(), at + block, byte);
                }
                txg += 1;
                make_group(&mut fx, &mut up, txg, one_label, &mut id);
                let whole = {
                    let local = up.local();
                    let first = local.held_cuts().first().map(|cut| cut.number);
                    first.is_some_and(|number| local.cut_is_whole(number))
                };
                let mut reached = false;
                for _ in 0..400 {
                    if up.doing() == Doing::Committing {
                        reached = true;
                        break;
                    }
                    let step = up.step().await.expect("a step");
                    if step == Step::Idle {
                        break;
                    }
                }
                if !reached {
                    continue;
                }
                fx.store
                    .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
                    .await;
                let failed = up.step().await;
                fx.store.disarm().await;
                if failed.is_ok() {
                    continue;
                }
                for block in 0..OVER {
                    fx.write(up.local(), at + block, byte ^ 0xFF);
                }
                let step = up.step().await.expect("the step after");
                if let Step::UploadedNotHeld { named, .. } = step {
                    assert!(named, "{case}");
                    tally.named += 1;
                    if whole {
                        tally.named_whole += 1;
                    }
                    let now = fx.uploaded().await;
                    fx.assert_uploaded(now.group.expect("a group"), &case).await;
                }
            }
        }
    }
    // The power cut, each unsynced sector on its own.
    let mut fate = Rng(seed ^ 0x5EED);
    let cut = BySector(move |_: Option<&str>, _| {
        if fate.below(2) == 0 {
            Fate::Durable
        } else {
            Fate::Latest
        }
    });
    let store = reboot(&mut fx, up, OTHER_BOOT, cut, &mut id);
    match store.intent_at_start() {
        Some(IntentAtStart::TakenUp { named, regions }) => {
            tally.reboots_taken_up += 1;
            if named < regions {
                tally.reboots_narrowed += 1;
            }
        }
        _ => tally.reboots_begun += 1,
    }
    let mut up = fx.driver(store, 64);
    for round in 0..3 {
        for _ in 0..rng.below(4) {
            let block = rng.below(BLOCKS);
            fx.write(up.local(), block, 1 + rng.below(250) as u8);
        }
        txg += 1;
        make_group(&mut fx, &mut up, txg, one_label, &mut id);
        let steps = run_and_check(&fx, &mut up, tally, &format!("{case}, round {round}")).await;
        if round == 0 {
            match told(&steps).first() {
                Some(Step::Began {
                    rule: Rule::Delta, ..
                }) => tally.first_delta += 1,
                Some(Step::Began {
                    rule: Rule::Whole, ..
                }) => tally.first_whole += 1,
                Some(Step::Mismatch { .. }) => tally.first_mismatch += 1,
                other => panic!("{case}: {other:?}"),
            }
        }
    }
    // Two rounds after any mismatch the newest cut is up.
    fx.assert_uploaded(txg, &format!("{case}, at the end")).await;
    tally.programs += 1;
}

fn make_group(fx: &mut Fixture, up: &mut Up, txg: u64, one_label: bool, id: &mut u64) {
    if one_label {
        group_one_label(fx, up.local(), txg, id);
    } else {
        fx.group(up.local(), txg);
    }
}

#[tokio::test]
async fn s5_random_programs_and_a_power_cut() {
    for one_label in [false, true] {
        let mut tally = Tally::default();
        for seed in 0..600 {
            program(seed, one_label, &mut tally).await;
        }
        println!("S5 groups to {} : {tally:?}", if one_label { "one label" } else { "four labels" });
    }
}

// ---------------------------------------------------------------------
// S6. A commit fails before anything lands (the commit object's PUT),
// and another writer then moves head, leaving the row as it was. The
// engine holds the cut's writes on a head that is no longer the
// volume's: the commit must be given up, not made again.
// ---------------------------------------------------------------------

#[tokio::test]
async fn s6_a_failed_commit_and_then_a_head_moved_by_another_writer() {
    let mut fx = Fixture::new(51).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    fx.write(up.local(), 20, 0x20);
    fx.group(up.local(), 8);
    step_until(&mut up, Doing::Committing).await;
    fx.store
        .arm_when(FaultOp::Put, "/commit/", 0, FaultWhen::Before)
        .await;
    let failed = up.step().await;
    fx.store.disarm().await;
    println!("S6 the commit: {failed:?}");
    assert!(failed.is_err());
    assert_eq!(fx.uploaded().await.group, Some(7), "nothing of cut 2 landed");
    fx.move_head().await;
    let moved = fx.uploaded().await;
    let step = up.step().await;
    println!("S6 the step after head moved: {step:?}");
    let steps = up.run().await.expect("the run after");
    println!("S6 then: {:?}", told(&steps));
    let now = fx.uploaded().await;
    println!(
        "S6 head's group {:?}, the row {:?}; head as the other writer left it: {}",
        now.group,
        now.row.map(|row| (row.number, row.group)),
        now.content == moved.content
    );
    assert_eq!(step, Ok(Step::GaveUp { number: 2 }));
    assert!(now.content == moved.content, "the uploader moved head");
    assert_eq!(now.row.map(|row| row.number), Some(1), "the row was swapped");
}

// ---------------------------------------------------------------------
// S7. A cut whose flush is still under way is not offered to the
// uploader (the M2.4 row: a cut is uploaded only once its flush has
// completed).
// ---------------------------------------------------------------------

#[tokio::test]
async fn s7_a_cut_whose_flush_is_under_way_is_not_uploaded() {
    let fx = Fixture::new(52).await;
    let mut store = fx.start();
    let mut id = 0;
    request(
        &mut store,
        &mut id,
        Op::Write {
            offset: 10 * BLOCK,
            data: Buf::filled(1, 0x10),
        },
    );
    for label in 0..4 {
        let (offset, slot) = slot_of(label, 7);
        request(&mut store, &mut id, Op::Write { offset, data: slot });
    }
    fx.disk.hold_syncs_of("home.img");
    store
        .submit(Request {
            id: RequestId(77),
            op: Op::Flush,
        })
        .expect("submitted");
    let mut done = Vec::new();
    for _ in 0..200 {
        store.poll(&mut done);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(done.is_empty(), "the flush's sync is held");
    let held: Vec<(u64, CutPhase)> = store.held_cuts().iter().map(|c| (c.number, c.phase)).collect();
    println!("S7 held while the flush is under way: {held:?}");
    let mut up = fx.driver(store, 64);
    let step = up.step().await;
    println!("S7 the step: {step:?}; checked {:?}", up.figures().checked);
    assert_eq!(step, Ok(Step::Idle));
    assert_eq!(up.figures().checked, Checked::Unchecked);
    assert_eq!(fx.uploaded().await.group, None);
    fx.disk.release_syncs();
    while done.is_empty() {
        up.local().wait(&mut done, Duration::from_secs(60));
    }
    let step = up.step().await;
    println!("S7 the step once the flush has ended: {step:?}");
    assert!(matches!(step, Ok(Step::Began { number: 1, .. })));
}

// ---------------------------------------------------------------------
// S8. A volume that failed for another reason than the map (a write of
// the image refused) still uploads the cut whose flush had completed,
// and lets it go (design §5.2; F2 as built: the driver stops at a cut
// the store kept, and at nothing else).
// ---------------------------------------------------------------------

#[tokio::test]
async fn s8_a_volume_failed_at_a_write_of_the_image_still_uploads_its_cut() {
    let mut fx = Fixture::new(53).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    fx.write(up.local(), 20, 0x20);
    fx.group(up.local(), 8);
    fx.disk.fail_write_of("home.img");
    let phases = |store: &mut InPlace<SimDisk>| -> Vec<(u64, CutPhase)> {
        store.held_cuts().iter().map(|c| (c.number, c.phase)).collect()
    };
    let (write_failed, failed, held) = {
        let store = up.local();
        store
            .submit(Request {
                id: RequestId(901),
                op: Op::Write {
                    offset: 30 * BLOCK,
                    data: Buf::filled(1, 0x30),
                },
            })
            .expect("submitted");
        let mut done = Vec::new();
        while done.is_empty() {
            store.wait(&mut done, Duration::from_secs(60));
        }
        let write_failed = done.pop().map(|c| c.result.is_err());
        (write_failed, store.is_failed(), phases(store))
    };
    println!("S8 the write failed: {write_failed:?}; the volume failed: {failed}; held {held:?}");
    assert!(failed, "the volume did not fail");
    assert_eq!(held, [(2, CutPhase::Uploading)]);
    let steps = up.run().await.expect("the run on the failed volume");
    let held = phases(up.local());
    println!(
        "S8 the run: {:?}; the driver is {:?}; held after {held:?}",
        told(&steps),
        up.doing()
    );
    assert!(
        matches!(
            steps.last(),
            Some(Step::Uploaded {
                number: 2,
                rule: Rule::Delta,
                ..
            })
        ),
        "{steps:?}"
    );
    assert_eq!(up.doing(), Doing::Checking);
    assert!(held.is_empty());
    fx.assert_uploaded(8, "S8").await;
}

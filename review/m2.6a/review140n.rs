//! Next look at #140 (M2.6a), scratch scenarios. Not for the repository.
//! Runs as crates/skyzfs-upload/tests/review140n.rs in a worktree of the
//! head, beside tests/common.

mod common;

use common::*;
use skynbd_refs::{ObjectRefStore, RefStore};
use skyzfs_upload::publish::{snap_ref, Publishing, EXPIRY_DEFAULT};
use skyzfs_upload::{Answer, Progress, Refusal, Request, Step};

const ATTACHMENT: [u8; 16] = *b"the attachment..";

fn manifest(name: &str) -> Vec<u8> {
    format!("the ZFS manifest of pool@{name}").into_bytes()
}

fn driver(fx: &Fixture, store: skyzfs_local::inplace::InPlace<skyzfs_local::sim::SimDisk>) -> Up {
    let mut up = fx.driver(store, 64);
    let state = up.local().cut_state_id().expect("a store that holds cuts");
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    up
}

fn request(up: &mut Up, name: &str, after: u64, group: u64) -> Request {
    Request {
        name: name.into(),
        attachment: ATTACHMENT,
        state: up.local().cut_state_id().expect("a store that holds cuts"),
        after,
        group,
        manifest: manifest(name),
        expiry: EXPIRY_DEFAULT,
    }
}

/// As the daemon asks (`Uploader::control`, `Control::Ask`): the store's
/// mark read at the desk, given to the publishing, then the ask.
fn ask_as_the_channel(up: &mut Up, request: Request) -> Answer {
    let mark = up.local().cuts().rewound;
    up.publishing().set_rewound(mark);
    up.publishing().ask(request)
}

async fn ref_group(fx: &Fixture, name: &str) -> Option<u64> {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let _found = refs.get(fx.vol, &snap_ref(name)).await.expect("the ref")?;
    let row = refs
        .meta_get(fx.vol, &skyzfs_upload::publish::snap_row_key(name))
        .await
        .expect("the row")
        .expect("a row");
    let row = skyzfs_upload::publish::SnapRow::decode(&row).expect("decodes");
    Some(row.cut.group)
}

/// F2a of the first review, asked as the channel now asks (the mark
/// with the request): C = 1, cut 2 (group 51) committed, `-T` to a
/// group below, the new history's first cut (group 40) counted, then
/// the ask.
#[tokio::test]
async fn next_f2a_with_the_mark_at_the_ask() {
    let mut fx = Fixture::new(140).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x30);
    fx.group(&mut store, 30);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let after = up.local().cuts().count;
    fx.write(up.local(), 101, 0x51);
    fx.group(up.local(), 51);
    up.run().await.expect("cut 2, the abandoned history");
    fx.write(up.local(), 102, 0x40);
    fx.group_anew(up.local(), 40);
    let cuts = up.local().cuts();
    println!("F2a' store: count {} rewound {}", cuts.count, cuts.rewound);
    let asked = request(&mut up, "t", after, 40);
    let answer = ask_as_the_channel(&mut up, asked);
    println!("F2a' answer: {answer:?}");
    let steps = up.run().await.expect("run");
    println!("F2a' steps: {steps:?}");
    let group = ref_group(&fx, "t").await;
    println!("F2a' snap/t on group: {group:?}");
    assert_eq!(answer, Answer::Refused(Refusal::Rewound));
    assert_eq!(group, None);
}

/// F2b of the first review, asked as the channel now asks: cut 2
/// (group 51) being uploaded when the rewind is counted and the request
/// asked.
#[tokio::test]
async fn next_f2b_with_the_mark_at_the_ask() {
    let mut fx = Fixture::new(141).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x30);
    fx.group(&mut store, 30);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let after = up.local().cuts().count;
    fx.write(up.local(), 101, 0x51);
    fx.group(up.local(), 51);
    let began = up.step().await.expect("cut 2 begins");
    assert!(matches!(began, Step::Began { number: 2, .. }), "{began:?}");
    fx.write(up.local(), 102, 0x40);
    fx.group_anew(up.local(), 40);
    let cuts = up.local().cuts();
    println!("F2b' store: count {} rewound {}", cuts.count, cuts.rewound);
    let asked = request(&mut up, "t", after, 40);
    let answer = ask_as_the_channel(&mut up, asked);
    println!("F2b' answer: {answer:?}");
    let steps = up.run().await.expect("run");
    println!("F2b' steps: {steps:?}");
    let group = ref_group(&fx, "t").await;
    println!("F2b' snap/t on group: {group:?}");
    assert_eq!(answer, Answer::Refused(Refusal::Rewound));
    assert_eq!(group, None);
}

/// A pending publish whose carrying cut of its own history is being
/// uploaded, ended by the mark that another request's ask brings
/// (before the check of the rewind's cut would come): P1 asked at C = 1
/// (snapshot at group 51), cut 2 (group 51) uploading, `-T` counted
/// (cut 3, group 40), then another name asked with a fresh count.
#[tokio::test]
async fn next_a_pending_publish_ended_by_another_ask() {
    let mut fx = Fixture::new(143).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x30);
    fx.group(&mut store, 30);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let after = up.local().cuts().count;
    fx.write(up.local(), 101, 0x51);
    fx.group(up.local(), 51);
    let p1 = request(&mut up, "p1", after, 51);
    assert_eq!(ask_as_the_channel(&mut up, p1), Answer::Accepted);
    let began = up.step().await.expect("a step");
    println!("P1 first step: {began:?}");
    let began = up.step().await.expect("a step");
    println!("P1 second step: {began:?}");
    fx.write(up.local(), 102, 0x40);
    fx.group_anew(up.local(), 40);
    let fresh = up.local().cuts().count;
    let p2 = request(&mut up, "p2", fresh, 40);
    let answer = ask_as_the_channel(&mut up, p2);
    println!("P2 answer: {answer:?}; P1 now {:?}", up.publishing().progress("p1"));
    let steps = up.run().await.expect("run");
    println!("steps: {steps:?}");
    println!(
        "P1 {:?} on group {:?}",
        up.publishing().progress("p1"),
        ref_group(&fx, "p1").await
    );
}

/// The same without the second ask: P1 is given cut 2 at its commit and
/// goes on (the check-only path).
#[tokio::test]
async fn next_a_pending_publish_without_another_ask() {
    let mut fx = Fixture::new(144).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x30);
    fx.group(&mut store, 30);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let after = up.local().cuts().count;
    fx.write(up.local(), 101, 0x51);
    fx.group(up.local(), 51);
    let p1 = request(&mut up, "p1", after, 51);
    assert_eq!(ask_as_the_channel(&mut up, p1), Answer::Accepted);
    let _ = up.step().await.expect("a step");
    let _ = up.step().await.expect("a step");
    fx.write(up.local(), 102, 0x40);
    fx.group_anew(up.local(), 40);
    let steps = up.run().await.expect("run");
    println!("steps: {steps:?}");
    println!(
        "P1 alone {:?} on group {:?}",
        up.publishing().progress("p1"),
        ref_group(&fx, "p1").await
    );
}

/// The cost of (R') after `-T`, as the publish rig's prewind course
/// meets it: groups 20 to 28, `-T` to 21, the new history's groups 22
/// on, each in another state. The agent reads C after the import's
/// first cut, takes its snapshot (one group), and asks with the mark as
/// the channel gives it; refused, it asks again with a count read anew
/// after each answer, a group synced between two asks. How many asks
/// until one is done, and on which group.
#[tokio::test]
async fn next_the_cost_after_t() {
    let mut fx = Fixture::new(145).await;
    let mut store = fx.start();
    let mut block = 100;
    for g in 20..=28 {
        fx.write(&mut store, block, g as u8);
        block += 1;
        fx.group(&mut store, g);
    }
    let mut up = driver(&fx, store);
    up.run().await.expect("the old history");
    // -T to 21: the import's first group, 22, written anew.
    fx.write(up.local(), block, 0xa0);
    block += 1;
    fx.group_anew(up.local(), 22);
    let c = up.local().cuts().count;
    // The snapshot's own group.
    fx.write(up.local(), block, 0xa1);
    block += 1;
    fx.group_anew(up.local(), 23);
    let snapshot = 23;
    let mut after = c;
    let mut asks = Vec::new();
    let mut g = 24;
    loop {
        let asked = request(&mut up, "t", after, snapshot);
        let answer = ask_as_the_channel(&mut up, asked);
        let cuts = up.local().cuts();
        asks.push(format!(
            "ask after {after} (count {}, mark {}): {answer:?}",
            cuts.count, cuts.rewound
        ));
        if answer == Answer::Accepted {
            // One group of the new history, then let the uploader go on.
            fx.write(up.local(), block, g as u8);
            block += 1;
            fx.group_anew(up.local(), g);
            g += 1;
            up.run().await.expect("run");
            let progress = up.publishing().progress("t");
            asks.push(format!("  then group {}: {progress:?}", g - 1));
            if matches!(progress, Progress::Done(_)) {
                break;
            }
        }
        after = up.local().cuts().count;
        if asks.len() > 60 {
            break;
        }
    }
    for line in &asks {
        println!("COST {line}");
    }
    println!("COST snap/t on group {:?}", ref_group(&fx, "t").await);
}

/// Rule (R') by table: the first review's fourteen rows, with the
/// marks the running maximum gives, and rows it adds.
#[tokio::test]
async fn next_table_of_rule_r() {
    enum E {
        G(u64),
        Anew(u64),
        Same(u64),
        Restart,
        Reboot,
        // zpool create on the volume: each ring written whole, with no
        // uberblock that counts (vdev_label_init's template has group 0).
        Wipe,
    }
    use E::*;
    let table: Vec<(&str, Vec<E>, (u64, u64), (u64, u64))> = vec![
        // (case, events, the first form's mark, the running maximum's)
        ("rising", vec![G(10), G(11), G(12)], (3, 0), (3, 0)),
        ("-F: equal group, new root", vec![G(10), G(11), Anew(11)], (3, 3), (3, 3)),
        ("-T: lower group", vec![G(10), G(11), G(9)], (3, 3), (3, 3)),
        ("same uberblock again: a repeat", vec![G(10), G(11), Same(11)], (2, 0), (2, 0)),
        ("one lower (errs low)", vec![G(10), G(12), G(11)], (3, 3), (3, 3)),
        ("two rewinds", vec![G(10), G(11), G(9), G(12), G(8)], (5, 5), (5, 5)),
        ("rewind then rising", vec![G(10), G(20), G(15), G(16), G(25)], (5, 3), (5, 4)),
        ("take-over", vec![G(10), G(11), Restart], (2, 2), (2, 2)),
        ("take-over then rising", vec![G(10), G(11), Restart, G(12)], (3, 2), (3, 2)),
        ("take-over after a rewind", vec![G(10), G(11), G(9), G(12), Restart], (4, 4), (4, 4)),
        ("reboot: new state", vec![G(10), G(11), Reboot], (0, 0), (0, 0)),
        ("reboot then lower", vec![G(10), G(11), Reboot, G(9)], (1, 0), (1, 1)),
        ("reboot then lower twice", vec![G(10), G(11), Reboot, G(9), G(8)], (2, 2), (2, 2)),
        ("checkpoint rewind: higher group, new root", vec![G(10), G(11), Anew(12)], (3, 0), (3, 0)),
        // Added at the next look.
        ("errs low, then -F at the top", vec![G(8), G(6), Anew(8), G(9)], (4, 2), (4, 3)),
        ("-T then the abandoned top's group anew", vec![G(10), G(11), G(12), Anew(10), Anew(11), Anew(12), G(13)], (7, 4), (7, 6)),
        ("take-over in the -T window", vec![G(10), G(11), G(12), Anew(10), Restart, Anew(11), G(13)], (6, 4), (6, 5)),
        ("a pool made anew, one daemon", vec![G(100), G(101), Wipe, G(4), G(5), G(6)], (5, 3), (5, 5)),
        ("a pool made anew, then a restart", vec![G(100), G(101), Wipe, G(4), Restart, G(5), G(6)], (5, 3), (5, 3)),
    ];
    let mut wrong = Vec::new();
    for (seed, (case, events, _first, want)) in table.into_iter().enumerate() {
        let mut fx = Fixture::new(300 + seed as u64).await;
        let mut store = Some(fx.start());
        for (i, event) in events.iter().enumerate() {
            let byte = (i + 1) as u8;
            match event {
                G(g) => {
                    let s = store.as_mut().expect("store");
                    fx.write(s, 100 + i as u64, byte);
                    fx.group(s, *g);
                }
                Anew(g) => {
                    let s = store.as_mut().expect("store");
                    fx.write(s, 100 + i as u64, byte);
                    fx.group_anew(s, *g);
                }
                Same(g) => {
                    let s = store.as_mut().expect("store");
                    fx.group(s, *g);
                }
                Restart => {
                    let up = driver(&fx, store.take().expect("store"));
                    fx.died(up);
                    store = Some(fx.start());
                }
                Reboot => {
                    let up = driver(&fx, store.take().expect("store"));
                    fx.rebooted(up);
                    store = Some(fx.start());
                }
                Wipe => {
                    let s = store.as_mut().expect("store");
                    let starts: Vec<u64> = skyzfs_uberblock::Labels::for_size(SIZE)
                        .rings()
                        .map(|ring| ring.start)
                        .collect();
                    for start in starts {
                        for b in 0..skyzfs_uberblock::RING_SIZE / BLOCK {
                            fx.write(s, start / BLOCK + b, 0);
                        }
                    }
                }
            }
        }
        let cuts = store.as_ref().expect("store").cuts();
        let got = (cuts.count, cuts.rewound);
        println!("TABLE {case}: (count, rewound) = {got:?}, want {want:?}");
        if got != want {
            wrong.push(format!("{case}: {got:?} want {want:?}"));
        }
    }
    assert!(wrong.is_empty(), "{wrong:#?}");
}

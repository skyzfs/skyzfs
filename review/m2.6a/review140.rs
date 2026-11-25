//! Review of #140 (M2.6a), scratch scenarios. Not for the repository.

mod common;

use common::*;
use skyzfs_upload::publish::{snap_ref, Publishing, EXPIRY_DEFAULT};
use skyzfs_upload::{Answer, Request, Step};
use skynbd_refs::{ObjectRefStore, RefStore};

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

async fn ref_group(fx: &Fixture, name: &str) -> Option<u64> {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let found = refs.get(fx.vol, &snap_ref(name)).await.expect("the ref")?;
    let row = refs
        .meta_get(fx.vol, &skyzfs_upload::publish::snap_row_key(name))
        .await
        .expect("the row")
        .expect("a row");
    let row = skyzfs_upload::publish::SnapRow::decode(&row).expect("decodes");
    let _ = found;
    Some(row.cut.group)
}

/// F2 (a): C read before a `-T` rewind, the snapshot taken on the new
/// history (createtxg 40), asked after the rewind's first cut is counted
/// (the store's mark is 3) and before the driver's next check. Cut 2
/// (group 51, the abandoned history, committed) is known and carries it.
#[tokio::test]
async fn review_f2a_stale_mark_known_cut_of_the_old_history() {
    let mut fx = Fixture::new(140).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x30);
    fx.group(&mut store, 30);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let after = up.local().cuts().count;
    assert_eq!(after, 1);
    fx.write(up.local(), 101, 0x51);
    fx.group(up.local(), 51);
    up.run().await.expect("cut 2, the abandoned history");
    // export; import -T: the new history's first cut, group 40.
    fx.write(up.local(), 102, 0x40);
    fx.group(up.local(), 40);
    let cuts = up.local().cuts();
    println!("F2a store: count {} rewound {}", cuts.count, cuts.rewound);
    let asked = request(&mut up, "t", after, 40);
    let answer = up.publishing().ask(asked);
    println!("F2a answer: {answer:?}");
    let steps = up.run().await.expect("run");
    println!("F2a steps: {steps:?}");
    let group = ref_group(&fx, "t").await;
    println!("F2a snap/t on group: {group:?}");
    assert_eq!(
        group, None,
        "snap/t published on a cut of the abandoned history (group 51)"
    );
}

/// F2 (b): the same, with cut 2 (group 51) being uploaded when the rewind
/// is counted and the request asked: it waits for cut 2, which is
/// committed before the next check, and is published on it.
#[tokio::test]
async fn review_f2b_stale_mark_cut_being_uploaded() {
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
    fx.group(up.local(), 40);
    let cuts = up.local().cuts();
    println!("F2b store: count {} rewound {}", cuts.count, cuts.rewound);
    let asked = request(&mut up, "t", after, 40);
    let answer = up.publishing().ask(asked);
    println!("F2b answer: {answer:?}");
    let steps = up.run().await.expect("run");
    println!("F2b steps: {steps:?}");
    let group = ref_group(&fx, "t").await;
    println!("F2b snap/t on group: {group:?}");
    assert_eq!(
        group, None,
        "snap/t published on a cut of the abandoned history (group 51)"
    );
}

/// F1: `zpool import --rewind-to-checkpoint` writes the checkpointed
/// uberblock with group (last + 1), its own root and a later time: the
/// cut has a group above the one before and opens as itself. A publish
/// asked before the rewind is carried by it; no mark.
#[tokio::test]
async fn review_f1_checkpoint_rewind_marks_nothing() {
    let mut fx = Fixture::new(142).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x30);
    fx.group(&mut store, 30);
    let mut up = driver(&fx, store);
    // Cut 1 (group 30, the checkpoint's) is being uploaded, slowly.
    let began = up.step().await.expect("cut 1 begins");
    assert!(matches!(began, Step::Began { number: 1, .. }), "{began:?}");
    let after = up.local().cuts().count;
    // pool@t at group 45 on the history after the checkpoint; cut 2
    // waits behind cut 1.
    fx.write(up.local(), 101, 0x45);
    fx.group(up.local(), 45);
    let asked = request(&mut up, "t", after, 45);
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    // export; import --rewind-to-checkpoint: block 101 back to its
    // content at the checkpoint, the checkpoint's uberblock written as
    // group 46 (the last + 1) with a later time. Its cut replaces the
    // one that waits (D7).
    fx.write(up.local(), 101, 0x00);
    fx.group_anew(up.local(), 46);
    let cuts = up.local().cuts();
    println!("F1 store: count {} rewound {}", cuts.count, cuts.rewound);
    let held = up.local().held_cuts();
    println!(
        "F1 held: {:?}",
        held.iter()
            .map(|c| (c.number, c.state.txg, c.itself))
            .collect::<Vec<_>>()
    );
    up.run().await.expect("run");
    let group = ref_group(&fx, "t").await;
    println!("F1 snap/t on group: {group:?}");
    assert_eq!(group, None, "snap/t published on the checkpoint's state");
}

/// Rule (R) by table: group sequences and events, the store's count and
/// mark after each.
#[tokio::test]
async fn review_table_of_rule_r() {
    enum E {
        G(u64),
        Anew(u64),
        Same(u64),
        Restart,
        Reboot,
    }
    use E::*;
    let table: Vec<(&str, Vec<E>, (u64, u64))> = vec![
        ("rising", vec![G(10), G(11), G(12)], (3, 0)),
        ("-F: equal group, new root", vec![G(10), G(11), Anew(11)], (3, 3)),
        ("-T: lower group", vec![G(10), G(11), G(9)], (3, 3)),
        ("same uberblock again: a repeat", vec![G(10), G(11), Same(11)], (2, 0)),
        ("one lower (errs low)", vec![G(10), G(12), G(11)], (3, 3)),
        ("two rewinds", vec![G(10), G(11), G(9), G(12), G(8)], (5, 5)),
        ("rewind then rising", vec![G(10), G(20), G(15), G(16), G(25)], (5, 3)),
        ("take-over", vec![G(10), G(11), Restart], (2, 2)),
        ("take-over then rising", vec![G(10), G(11), Restart, G(12)], (3, 2)),
        ("take-over after a rewind", vec![G(10), G(11), G(9), G(12), Restart], (4, 4)),
        ("reboot: new state", vec![G(10), G(11), Reboot], (0, 0)),
        ("reboot then lower", vec![G(10), G(11), Reboot, G(9)], (1, 0)),
        ("reboot then lower twice", vec![G(10), G(11), Reboot, G(9), G(8)], (2, 2)),
        ("checkpoint rewind: higher group, new root", vec![G(10), G(11), Anew(12)], (3, 0)),
    ];
    let mut wrong = Vec::new();
    for (seed, (case, events, want)) in table.into_iter().enumerate() {
        let mut fx = Fixture::new(200 + seed as u64).await;
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

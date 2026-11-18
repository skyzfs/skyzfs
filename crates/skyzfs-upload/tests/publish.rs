//! Publishing a cut as a snapshot (work package M2.5-1; design §5.2,
//! "A snapshot is published by a cut of the pool's present history"):
//! the driver publishes `snap/t` on the first committed cut counted after
//! *C*, of group *G* or later, that opens as itself, in a refs
//! transaction of its own once that cut's commit has landed.
//!
//! The done-when of the M2.5 row, each a test here: a cut counted before
//! *C* with a higher group (the rewind case) is not published; a cut
//! that does not open as itself is not published; an abandoned cut's
//! publish goes to the next cut; the ref names a commit only once that
//! commit is whole (the crash matrix); an attachment's publish is not
//! carried out on the next.

mod common;

use std::panic::AssertUnwindSafe;

use common::*;
use futures_util::FutureExt;
use skynbd_engine::{keys, Engine};
use skynbd_format::{Codec, SnapKind, SnapshotManifest};
use skynbd_refs::{ObjectRefStore, RefOp, RefStore};
use skynbd_store::{FaultOp, FaultWhen, ObjectStore};
use skynbd_types::CommitHash;
use skyzfs_upload::publish::{manifest_key, snap_ref, snap_row_key, SnapRow};
use skyzfs_upload::{Answer, Doing, Progress, Refusal, Request, Rule, Step};

const ATTACHMENT: [u8; 16] = *b"the attachment..";
const OTHER: [u8; 16] = *b"the next one....";

/// The ZFS manifest the agent sends with snapshot `name`.
fn manifest(name: &str) -> Vec<u8> {
    format!("the ZFS manifest of pool@{name}").into_bytes()
}

/// A driver of the fixture's store that serves [`ATTACHMENT`].
fn driver(fx: &Fixture, store: skyzfs_local::inplace::InPlace<skyzfs_local::sim::SimDisk>) -> Up {
    let mut up = fx.driver(store, 64);
    up.publishing().set_attachment(ATTACHMENT);
    up
}

/// The request for `pool@name`, taken after the store had counted
/// `after` cuts, of `createtxg` `group`.
fn request(up: &mut Up, name: &str, after: u64, group: u64) -> Request {
    Request {
        name: name.into(),
        attachment: ATTACHMENT,
        state: up.local().cut_state_id().expect("a store that holds cuts"),
        after,
        group,
        manifest: manifest(name),
    }
}

/// Ask `up` for `pool@name` with *C* the store's count of cuts now.
fn ask_now(up: &mut Up, name: &str, group: u64) -> Answer {
    let after = up.local().cuts().count;
    let asked = request(up, name, after, group);
    up.publishing().ask(asked)
}

/// The steps that are not slices read.
fn told(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter(|step| !matches!(step, Step::Read { .. }))
        .cloned()
        .collect()
}

/// What object storage holds of snapshot `name`.
#[derive(Debug)]
struct Published {
    commit: CommitHash,
    row: SnapRow,
}

/// `snap/<name>` as object storage holds it, checked whole: where the ref
/// exists, its row reads, its two manifests are there and name it, and
/// its commit reads as the volume at the instant of the cut its row
/// names. `None` where the ref does not exist; then the row does not
/// either.
async fn published(fx: &Fixture, name: &str, case: &str) -> Option<Published> {
    let inner = fx.store.inner().clone();
    let refs = ObjectRefStore::new(inner.clone());
    let found = refs.get(fx.vol, &snap_ref(name)).await.expect("the ref");
    let row = refs
        .meta_get(fx.vol, &snap_row_key(name))
        .await
        .expect("the row");
    let Some(found) = found else {
        assert_eq!(row, None, "{case}: a row with no ref");
        return None;
    };
    let row = SnapRow::decode(&row.unwrap_or_else(|| panic!("{case}: a ref with no row")))
        .unwrap_or_else(|e| panic!("{case}: the row: {e}"));
    let zfs = inner
        .get(&manifest_key(fx.vol, row.snap), None)
        .await
        .unwrap_or_else(|e| panic!("{case}: the ZFS manifest: {e}"));
    assert_eq!(&zfs[..], &manifest(name)[..], "{case}: the ZFS manifest");
    assert_eq!(row.manifest_len as usize, zfs.len(), "{case}");
    assert_eq!(
        row.manifest_hash,
        *blake3::hash(&zfs).as_bytes(),
        "{case}: the row's hash of the ZFS manifest"
    );
    let framed = inner
        .get(&keys::snapshot(fx.vol, row.snap), None)
        .await
        .unwrap_or_else(|e| panic!("{case}: SkyNBD's manifest: {e}"));
    let framed = SnapshotManifest::decode(&framed).expect("SkyNBD's manifest decodes");
    assert_eq!(framed.snap_id, row.snap, "{case}");
    assert_eq!(framed.vol_id, fx.vol, "{case}");
    assert_eq!(framed.ref_name, snap_ref(name), "{case}");
    assert_eq!(framed.commit, found.commit, "{case}");
    assert_eq!(framed.kind, SnapKind::Manual, "{case}");
    assert_eq!(framed.parent_snap, None, "{case}");
    let engine = Engine::open_at_commit(inner.clone(), refs, fx.vol, found.commit, cfg())
        .await
        .unwrap_or_else(|e| panic!("{case}: the snapshot's commit: {e}"));
    assert_eq!(
        engine.head_group(),
        Some(row.cut.group),
        "{case}: the commit is the cut's"
    );
    let content = engine.read(0, SIZE).await.expect("the snapshot");
    let instant = &fx.at[&row.cut.group];
    assert!(
        content == *instant,
        "{case}: the snapshot does not hold the volume at the instant of the cut of group {}{}",
        row.cut.group,
        differing(&content, instant)
    );
    Some(Published {
        commit: found.commit,
        row,
    })
}

/// The head's commit, as the refs object names it.
async fn head(fx: &Fixture) -> CommitHash {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.get(fx.vol, "head")
        .await
        .expect("head")
        .expect("a head")
        .commit
}

/// The rewind case of design §5.2: after `zpool import -T`, the abandoned
/// history's uberblocks are still on the labels, of higher groups than
/// the snapshot's. Cut 1 (group 51) is counted before *C*; cut 2
/// (group 40, written while 51 is still on the labels) is counted after
/// *C* but does not open as itself; neither is published. Cut 3
/// (group 52) is: it is the first that carries the request.
#[tokio::test]
async fn a_cut_before_c_or_not_opening_as_itself_is_not_published() {
    let mut fx = Fixture::new(40).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x51);
    fx.group(&mut store, 51);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    assert_eq!(ask_now(&mut up, "t", 40), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(steps, [Step::PublishLooked { name: "t".into() }]);
    assert!(published(&fx, "t", "cut 1").await.is_none(), "cut 1");

    fx.write(up.local(), 101, 0x40);
    fx.group(up.local(), 40);
    let held = up.local().held_cuts();
    assert_eq!(
        held.iter()
            .map(|cut| (cut.number, cut.itself))
            .collect::<Vec<_>>(),
        [(2, false)],
        "group 40 does not open as itself while 51 is on the labels"
    );
    let steps = up.run().await.expect("cut 2");
    assert!(
        told(&steps)
            .iter()
            .all(|step| matches!(step, Step::Began { .. } | Step::Uploaded { .. })),
        "{steps:?}"
    );
    fx.assert_uploaded(40, "cut 2").await;
    assert!(published(&fx, "t", "cut 2").await.is_none(), "cut 2");
    assert_eq!(up.publishes().progress("t"), Progress::Pending);

    fx.write(up.local(), 102, 0x52);
    fx.group(up.local(), 52);
    let steps = up.run().await.expect("cut 3");
    let commit = head(&fx).await;
    assert_eq!(
        told(&steps),
        [
            Step::Began {
                number: 3,
                rule: Rule::Delta
            },
            Step::Uploaded {
                number: 3,
                rule: Rule::Delta,
                blocks: 5
            },
            Step::Published {
                name: "t".into(),
                number: 3,
                commit,
                found: false
            },
        ]
    );
    let snapshot = published(&fx, "t", "cut 3").await.expect("published");
    assert_eq!(snapshot.commit, commit);
    assert_eq!(snapshot.row.cut.number, 3);
    assert_eq!(snapshot.row.cut.group, 52);
    assert_eq!(up.publishes().progress("t"), Progress::Done(commit));
    let figures = up.publishes().figures();
    assert_eq!((figures.accepted, figures.published), (1, 1));
    assert_eq!(
        figures
            .last
            .as_ref()
            .map(|(name, cut, _)| (name.as_str(), cut.number)),
        Some(("t", 3))
    );
    // The volume and its refs pass fsck: the snapshot's closure is whole.
    fx.assert_uploaded(52, "after the publish").await;
}

/// A publish asked after its first qualifying cut was committed goes on
/// that cut at once: the first counted after *C*, not the newest.
#[tokio::test]
async fn a_publish_goes_on_the_first_committed_cut_after_c() {
    let mut fx = Fixture::new(41).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let first = head(&fx).await;
    let after = up.local().cuts().count;
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    up.run().await.expect("cut 2");
    let second = head(&fx).await;
    fx.write(up.local(), 100, 0x09);
    fx.group(up.local(), 9);
    up.run().await.expect("cut 3");

    let asked = request(&mut up, "t", after, 7);
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the publish");
    assert_eq!(
        steps,
        [
            Step::PublishLooked { name: "t".into() },
            Step::Published {
                name: "t".into(),
                number: 2,
                commit: second,
                found: false
            },
        ]
    );
    let snapshot = published(&fx, "t", "cut 2").await.expect("published");
    assert_eq!(snapshot.row.cut.group, 8);
    assert_ne!(snapshot.commit, first);
    // The publish made no commit: head is cut 3's still.
    fx.assert_uploaded(9, "head").await;
}

/// The cut that would carry a publish is abandoned at the bound while it
/// is read: it is never committed, and the next cut, which took its map,
/// carries the publish.
#[tokio::test]
async fn an_abandoned_cut_hands_its_publish_to_the_next() {
    let mut fx = Fixture::new(42).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    assert_eq!(ask_now(&mut up, "t", 8), Answer::Accepted);
    assert_eq!(
        up.step().await,
        Ok(Step::PublishLooked { name: "t".into() })
    );
    // Cut 2 (group 8) holds more blocks than the bound has room for
    // copies of; all are overwritten once its upload has begun, so that
    // it is abandoned, and cut 3 (group 9) takes its map.
    for block in 0..OVER {
        fx.write(up.local(), 100 + block, 0x08);
    }
    fx.group(up.local(), 8);
    assert!(matches!(up.step().await, Ok(Step::Began { number: 2, .. })));
    for block in 0..OVER {
        fx.write(up.local(), 100 + block, 0x09);
    }
    fx.group(up.local(), 9);
    let lost = up.step().await.expect("cut 2's slice");
    assert!(matches!(lost, Step::Lost { number: 2, .. }), "{lost:?}");
    let steps = up.run().await.expect("cut 3");
    let commit = head(&fx).await;
    assert_eq!(
        told(&steps).last(),
        Some(&Step::Published {
            name: "t".into(),
            number: 3,
            commit,
            found: false
        }),
        "{steps:?}"
    );
    let snapshot = published(&fx, "t", "cut 3").await.expect("published");
    assert_eq!(snapshot.row.cut.group, 9);
}

/// A request of another attachment is refused; one accepted by an
/// uploader that ends is not carried out by the next, which has nothing
/// pending; a request asked again of the next is looked up anew.
#[tokio::test]
async fn an_attachments_publish_is_not_carried_out_on_the_next() {
    let mut fx = Fixture::new(43).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let mut asked = request(&mut up, "other", 1, 7);
    asked.attachment = OTHER;
    assert_eq!(
        up.publishing().ask(asked),
        Answer::Refused(Refusal::Attachment)
    );
    assert_eq!(ask_now(&mut up, "t", 8), Answer::Accepted);
    up.run().await.expect("the lookup");

    // The uploader ends with the publish pending; the next serves
    // another attachment.
    fx.died(up);
    let mut up = fx.driver(fx.start(), 64);
    up.publishing().set_attachment(OTHER);
    assert_eq!(up.publishes().progress("t"), Progress::Unknown);
    assert_eq!(up.publishes().pending().0, 0);
    let stale = request(&mut up, "t", 1, 8);
    assert_eq!(
        up.publishing().ask(stale),
        Answer::Refused(Refusal::Attachment)
    );
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    let steps = up.run().await.expect("the next cut");
    assert!(
        !steps
            .iter()
            .any(|step| matches!(step, Step::Published { .. })),
        "{steps:?}"
    );
    assert!(published(&fx, "t", "the next attachment").await.is_none());
    assert!(published(&fx, "other", "the next attachment")
        .await
        .is_none());
}

/// A name another has taken is refused once looked up, and the cuts go
/// up as before: a ref with no row of ours, and a ref whose row records
/// another manifest.
#[tokio::test]
async fn a_name_taken_is_refused_and_the_cuts_go_on() {
    let mut fx = Fixture::new(44).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let cut1 = head(&fx).await;
    // Another snapshot on cut 1's commit, as SkyNBD's engine makes one:
    // a ref and no row.
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.transact(
        fx.vol,
        vec![RefOp::CasRef {
            name: snap_ref("theirs"),
            expected: None,
            new_commit: Some(cut1),
        }],
    )
    .await
    .expect("their snapshot");
    assert_eq!(ask_now(&mut up, "theirs", 0), Answer::Accepted);
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    let steps = up.run().await.expect("cut 2");
    assert!(
        steps.contains(&Step::PublishRefused {
            name: "theirs".into(),
            why: Refusal::Taken
        }),
        "{steps:?}"
    );
    assert_eq!(
        up.publishes().progress("theirs"),
        Progress::Refused(Refusal::Taken)
    );
    fx.assert_uploaded(8, "cut 2").await;
    // Ours, asked with C before cut 2: published on it.
    let after = up.local().cuts().count - 1;
    let ours = request(&mut up, "t", after, 0);
    assert_eq!(up.publishing().ask(ours), Answer::Accepted);
    up.run().await.expect("t");
    let ours = published(&fx, "t", "t").await.expect("t");
    assert_eq!(ours.row.cut.number, 2);

    fx.died(up);
    let mut up = driver(&fx, fx.start());
    let mut other = request(&mut up, "t", 0, 0);
    other.manifest = b"another manifest".to_vec();
    assert_eq!(up.publishing().ask(other), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(
        steps,
        [Step::PublishRefused {
            name: "t".into(),
            why: Refusal::Taken
        }]
    );
    // The same name, a C its cut does not come after: refused too.
    let late = request(&mut up, "t", after + 5, 0);
    assert_eq!(up.publishing().ask(late), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(
        steps,
        [Step::PublishRefused {
            name: "t".into(),
            why: Refusal::Taken
        }]
    );
    let still = published(&fx, "t", "t, after").await.expect("t");
    assert_eq!(still.commit, ours.commit);
}

/// The agent asks again after the uploader that published `snap/t`
/// ended: the next looks it up and answers done, publishing nothing.
#[tokio::test]
async fn asked_again_after_a_restart_answers_done() {
    let mut fx = Fixture::new(45).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    up.run().await.expect("cut 1 and its publish");
    let first = published(&fx, "t", "first").await.expect("published");
    // Asking twice is asking once.
    assert_eq!(
        up.publishing().ask(asked.clone()),
        Answer::Done(first.commit)
    );

    fx.died(up);
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(
        steps,
        [Step::Published {
            name: "t".into(),
            number: 1,
            commit: first.commit,
            found: true
        }]
    );
    assert_eq!(up.publishes().progress("t"), Progress::Done(first.commit));
    let figures = up.publishes().figures();
    assert_eq!((figures.found, figures.published), (1, 0));
    let again = published(&fx, "t", "again").await.expect("published");
    assert_eq!(again.row, first.row);
}

/// How the uploader comes back after a death in [`publish_killed_at`].
#[derive(Debug, Clone, Copy)]
enum Back {
    /// The same uploader takes its next step: an error of object storage
    /// that it outlives.
    Retry,
    /// The daemon died: a new uploader, asked again by the agent.
    Restart,
}

/// What one failure in [`publish_killed_at`] showed.
#[derive(Debug, Default)]
struct Seen {
    /// The failure came: the run did not end on its own.
    died: bool,
    /// `snap/t` existed as the failure left it.
    landed: bool,
    /// The cut `snap/t` was published on in the end.
    on: u64,
}

/// Cut 1 (group 7) of 40 blocks, read in slices of 16, with `pool@t`
/// asked before its upload begins; object storage fails at its `k`th
/// operation from there, `when`.
async fn publish_killed_at(when: FaultWhen, k: u64, back: Back) -> Seen {
    quiet_deaths();
    let case = format!("{when:?} {k} {back:?}");
    let mut fx = Fixture::new(46).await;
    let mut store = fx.start();
    for block in 60..100 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 16);
    up.publishing().set_attachment(ATTACHMENT);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    fx.store.arm_when(FaultOp::Any, "", k, when).await;
    let ran = AssertUnwindSafe(up.run()).catch_unwind().await;
    fx.store.disarm().await;
    match ran {
        Ok(Ok(steps)) => {
            assert!(
                matches!(steps.last(), Some(Step::Published { found: false, .. })),
                "{case}: {steps:?}"
            );
            let done = published(&fx, "t", &case).await.expect("published");
            return Seen {
                died: false,
                landed: true,
                on: done.row.cut.number,
            };
        }
        Ok(Err(_)) => {}
        Err(payload) => std::panic::resume_unwind(payload),
    }

    // As the death left it: no ref, or a whole one on cut 1; every ref's
    // closure whole.
    let left = published(&fx, "t", &format!("{case}, as the death left it")).await;
    if let Some(left) = &left {
        assert_eq!(left.row.cut.number, 1, "{case}");
    }
    let uploaded = fx.uploaded().await;
    assert!(uploaded.fsck, "{case}: fsck");

    let mut up = match back {
        Back::Retry => up,
        Back::Restart => {
            fx.died(up);
            let mut up = driver(&fx, fx.start());
            assert_eq!(up.publishing().ask(asked), Answer::Accepted, "{case}");
            up
        }
    };
    // A cut still held is found committed, or committed, by the run
    // after: the uploader knows it.
    let held = !up.local().held_cuts().is_empty();
    up.run()
        .await
        .unwrap_or_else(|e| panic!("{case}: the run after: {e}"));
    if up.publishes().progress("t") == Progress::Pending {
        // A new uploader whose cut 1 was let go before the death knows no
        // cut: the publish asked of the uploader that died is lost with
        // it, and the one asked again waits for the next qualifying cut
        // (the lead's Q2 of M2.5).
        assert!(
            matches!(back, Back::Restart) && left.is_none() && !held,
            "{case}: pending"
        );
        fx.write(up.local(), 100, 0x08);
        fx.group(up.local(), 8);
        up.run().await.expect("cut 2");
    }
    let done = published(&fx, "t", &format!("{case}, after"))
        .await
        .unwrap_or_else(|| panic!("{case}: not published"));
    assert_eq!(
        up.publishes().progress("t"),
        Progress::Done(done.commit),
        "{case}"
    );
    let landed = left.is_some();
    if let Some(left) = left {
        assert_eq!(left.commit, done.commit, "{case}: the ref moved");
        assert_eq!(left.row, done.row, "{case}: the row changed");
    }
    assert_eq!(up.figures().mismatches, 0, "{case}");
    Seen {
        died: true,
        landed,
        on: done.row.cut.number,
    }
}

/// The run with no death: its operations on object storage.
async fn operations_of_the_publish() -> Vec<String> {
    let mut fx = Fixture::new(46).await;
    let mut store = fx.start();
    for block in 60..100 {
        fx.write(&mut store, block, block as u8);
    }
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 16);
    up.publishing().set_attachment(ATTACHMENT);
    let asked = request(&mut up, "t", 0, 7);
    up.publishing().ask(asked);
    fx.store.inner().record(true);
    let steps = up.run().await.expect("the run");
    fx.store.inner().record(false);
    assert!(
        matches!(steps.last(), Some(Step::Published { .. })),
        "{steps:?}"
    );
    fx.store.inner().log()
}

/// The crash matrix: object storage fails at every operation of a run
/// that uploads cut 1 and publishes it, before the operation and after
/// it (the operation lands, its reply is lost). As each failure left it,
/// `snap/t` is absent or names cut 1's whole commit, with its row and
/// both manifests; the same uploader, stepping on, publishes it, and so
/// does a new one asked again, without moving a ref that landed.
///
/// A death between the commit and the publish's transaction (the lead's
/// ask on M2.5-1): cut 1 has been let go, no ref is left, and the
/// publish pending dies with the uploader. Asked again, it goes on the
/// next qualifying cut, cut 2. A death before cut 1 was let go leaves it
/// held, and the next uploader, which finds it committed or commits it,
/// publishes it on cut 1.
#[tokio::test]
async fn a_failure_at_any_operation_leaves_no_ref_or_a_whole_one() {
    let operations = operations_of_the_publish().await;
    let swaps: Vec<usize> = operations
        .iter()
        .enumerate()
        .filter(|(_, op)| *op == "put_if refs")
        .map(|(i, _)| i)
        .collect();
    let [swap, publish] = swaps[..] else {
        panic!("the head swap and the publish's own transaction: {operations:?}");
    };
    for back in [Back::Retry, Back::Restart] {
        for when in [FaultWhen::Before, FaultWhen::After] {
            let mut points = 0;
            let mut left = 0;
            let mut on_the_next = Vec::new();
            for k in 0.. {
                let seen = publish_killed_at(when, k, back).await;
                if !seen.died {
                    break;
                }
                points += 1;
                left += u64::from(seen.landed);
                if seen.on == 2 {
                    on_the_next.push(k as usize);
                }
            }
            let case = format!("{back:?} {when:?}");
            assert_eq!(
                points,
                operations.len(),
                "{case}: a failure at each operation of {operations:?}"
            );
            // Only the publish's transaction, landed with its reply
            // lost, leaves the ref.
            let expected = match when {
                FaultWhen::Before => 0,
                FaultWhen::After => 1,
            };
            assert_eq!(left, expected, "{case}");
            // The publish goes to the next cut exactly where the daemon
            // died between cut 1's commit and the publish's transaction:
            // after the head swap landed, and before the publish's own
            // transaction did.
            let between: Vec<usize> = match (back, when) {
                (Back::Retry, _) => Vec::new(),
                (Back::Restart, FaultWhen::Before) => (swap + 1..=publish).collect(),
                (Back::Restart, FaultWhen::After) => (swap + 1..publish).collect(),
            };
            assert_eq!(on_the_next, between, "{case}: {operations:?}");
        }
    }
}

/// A publish waits while the cut that carries it is uploaded, and its
/// transaction comes only after the head swap: the run's operations end
/// with the head swap, then the two manifests' PUTs and the publish's
/// own transaction.
#[tokio::test]
async fn the_publish_is_made_after_the_cuts_commit() {
    let operations = operations_of_the_publish().await;
    let swaps: Vec<usize> = operations
        .iter()
        .enumerate()
        .filter(|(_, op)| *op == "put_if refs")
        .map(|(i, _)| i)
        .collect();
    let manifests: Vec<usize> = operations
        .iter()
        .enumerate()
        .filter(|(_, op)| *op == "put ?")
        .map(|(i, _)| i)
        .collect();
    assert_eq!(manifests.len(), 2, "{operations:?}");
    assert!(swaps[0] < manifests[0], "{operations:?}");
    assert!(manifests[1] < swaps[1], "{operations:?}");
    assert_eq!(swaps[1], operations.len() - 1, "{operations:?}");
}

/// The driver steps a pending publish before the cut it uploads, and a
/// publish that waits takes no step: a run with a publish waiting ends
/// idle, as one with none does.
#[tokio::test]
async fn a_publish_waiting_takes_no_step() {
    let mut fx = Fixture::new(47).await;
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    assert_eq!(ask_now(&mut up, "t", 8), Answer::Accepted);
    assert_eq!(
        up.step().await,
        Ok(Step::PublishLooked { name: "t".into() })
    );
    assert_eq!(up.step().await, Ok(Step::Idle));
    assert_eq!(up.step().await, Ok(Step::Idle));
    assert_eq!(up.doing(), Doing::Checking);
    assert_eq!(up.publishes().progress("t"), Progress::Pending);
}

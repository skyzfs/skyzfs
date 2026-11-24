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
//!
//! Then the review of #122: a publish is never made on a commit that is
//! not its cut's (F1), a request of another cut state is refused at once
//! (F2), a name asked again is looked up anew (F5), a ref made anew beside
//! a row left behind is not ours (F6), and a transaction refused because
//! the name was taken after its lookup is not taken for ours (T1).

mod common;

use std::panic::AssertUnwindSafe;
use std::time::Duration;

use common::*;
use futures_util::FutureExt;
use skynbd_engine::{keys, Engine};
use skynbd_format::{Codec, SnapKind, SnapshotManifest};
use skynbd_refs::{ObjectRefStore, RefOp, RefStore};
use skynbd_store::{FaultOp, FaultWhen, ObjectStore};
use skynbd_types::CommitHash;
use skyzfs_upload::publish::{
    manifest_key, snap_ref, snap_row_key, Publishing, SnapRow, EXPIRY_DEFAULT,
};
use skyzfs_upload::{Answer, Doing, Progress, Refusal, RemoteError, Request, Rule, Step};

const ATTACHMENT: [u8; 16] = *b"the attachment..";
const OTHER: [u8; 16] = *b"the next one....";

/// The ZFS manifest the agent sends with snapshot `name`.
fn manifest(name: &str) -> Vec<u8> {
    format!("the ZFS manifest of pool@{name}").into_bytes()
}

/// A driver of the fixture's store that serves [`ATTACHMENT`].
fn driver(fx: &Fixture, store: skyzfs_local::inplace::InPlace<skyzfs_local::sim::SimDisk>) -> Up {
    serving(fx.driver(store, 64), ATTACHMENT)
}

/// `up`, serving `attachment`, its store's cut state known: what the
/// daemon tells the uploader when it makes it.
fn serving(mut up: Up, attachment: [u8; 16]) -> Up {
    let state = up.local().cut_state_id().expect("a store that holds cuts");
    *up.publishing() = Publishing::new(Some(attachment), Some(state));
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
        expiry: EXPIRY_DEFAULT,
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
/// exists, its row reads and names its commit, its two manifests are there
/// and name it, it is listed as SkyNBD lists a snapshot, and its commit
/// reads as the volume at the instant of the cut its row names. `None`
/// where the ref does not exist; then the row does not either.
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
    assert_eq!(
        row.commit, found.commit,
        "{case}: the row names the ref's commit"
    );
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
    // Listed through SkyNBD as any snapshot is (the M2.5 row; N7 of
    // #122's review).
    let listed = engine.snapshot_list().await.expect("the snapshots");
    assert!(
        listed.contains(&(name.to_string(), found.commit)),
        "{case}: {listed:?}"
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
    let mut up = serving(fx.driver(fx.start(), 64), OTHER);
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

/// The agent asks again: of the uploader that published `snap/t`, and
/// after it ended, of the next. Each looks the name up and answers done,
/// publishing nothing.
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
    // Asking twice is asking once: looked up, and found ours.
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
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
    assert_eq!(up.publishes().figures().published, 1);

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
    let mut up = serving(fx.driver(store, 16), ATTACHMENT);
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
    let mut up = serving(fx.driver(store, 16), ATTACHMENT);
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

// ------------------------------------------------- the review of #122

/// Step until the cut under way is at its commit.
async fn to_the_commit(up: &mut Up) {
    for _ in 0..200 {
        if up.doing() == Doing::Committing {
            return;
        }
        up.step().await.expect("a step");
    }
    panic!("the commit was not reached");
}

/// Cut 1 (group 7), with `pool@t` asked before it: its head swap lands
/// and the reply is lost, and the driver is left at the commit. The
/// request, and cut 1's commit.
async fn swap_unheard(fx: &mut Fixture) -> (Up, Request, CommitHash) {
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    to_the_commit(&mut up).await;
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
        .await;
    assert!(up.step().await.is_err(), "the reply to the swap is lost");
    fx.store.disarm().await;
    let cut1 = head(fx).await;
    assert_eq!(fx.uploaded().await.group, Some(7), "cut 1's swap landed");
    (up, asked, cut1)
}

/// Delete `snap/<name>` as SkyNBD's engine does: the ref alone, and the
/// row stays.
async fn delete(fx: &Fixture, name: &str) {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let current = refs.get(fx.vol, &snap_ref(name)).await.expect("the ref");
    refs.transact(
        fx.vol,
        vec![RefOp::CasRef {
            name: snap_ref(name),
            expected: current,
            new_commit: None,
        }],
    )
    .await
    .expect("the delete");
}

/// F1 of #122's review, the same uploader: cut 1's head swap landed and
/// its reply was lost, and another writer moved head (an operator's
/// write: a commit with no group) before the uploader read the row and
/// head again. The row still names cut 1, so cut 1 is committed and let
/// go, as before; but head is not its commit, so no publish goes on it,
/// and `snap/t` is not made on the other writer's commit. The next cut's
/// check finds head moved.
#[tokio::test]
async fn a_cut_found_committed_after_head_moved_carries_no_publish() {
    let mut fx = Fixture::new(60).await;
    let (mut up, _, cut1) = swap_unheard(&mut fx).await;
    fx.move_head().await;
    assert_ne!(head(&fx).await, cut1);
    let steps = up.run().await.expect("the run after");
    assert!(
        matches!(told(&steps)[..], [Step::Uploaded { number: 1, .. }]),
        "{steps:?}"
    );
    assert!(published(&fx, "t", "head moved").await.is_none());
    assert_eq!(up.publishes().progress("t"), Progress::Pending);
    assert_eq!(up.figures().not_offered, 1);

    fx.write(up.local(), 101, 0x08);
    fx.group(up.local(), 8);
    let steps = up.run().await.expect("cut 2");
    assert!(
        steps.iter().any(|step| matches!(
            step,
            Step::Mismatch {
                why: skyzfs_upload::Mismatch::HeadMoved,
                ..
            }
        )),
        "{steps:?}"
    );
    assert!(published(&fx, "t", "after cut 2").await.is_none());
}

/// F1 of #122's review, a new uploader: as above, but the daemon dies
/// with the reply lost, head is moved while it is down, and the next
/// uploader, asked again, finds cut 1 committed at its check. Head is
/// not its commit: no publish goes on it.
#[tokio::test]
async fn a_cut_found_committed_at_the_check_after_head_moved_carries_no_publish() {
    let mut fx = Fixture::new(61).await;
    let (up, asked, _) = swap_unheard(&mut fx).await;
    fx.died(up);
    fx.move_head().await;
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the run after");
    assert_eq!(
        steps,
        [
            Step::PublishLooked { name: "t".into() },
            Step::FoundCommitted { number: 1 }
        ]
    );
    assert!(published(&fx, "t", "head moved").await.is_none());
    assert_eq!(up.publishes().progress("t"), Progress::Pending);
    assert_eq!(up.figures().not_offered, 1);
}

/// How head came to a commit of the cut's group that is not the cut's
/// (n1 and n2 of the review of #122).
#[derive(Debug, Clone, Copy)]
enum Moved {
    /// Another history's commit of the cut's group: a pool imported with
    /// `zpool import -T` begins its groups again from an older one.
    Repeated,
    /// An operator rolled the volume back to `snap/s`, published on a cut
    /// of the cut's group by the cut state before (after a reboot, a cut
    /// state begins anew, and so can its groups).
    RolledBack,
}

/// What came between the cut's head swap and the uploader's next read of
/// the row and head.
#[derive(Debug, Clone, Copy)]
enum Then {
    /// The swap's reply was lost; the same uploader steps on.
    LostReply,
    /// The swap's reply was lost and the daemon died; the next uploader is
    /// asked again.
    Restart,
}

/// `snap/s` published on cut 1 (group 7) of a cut state whose block 100
/// is full of 0x70; then the host rebooted, and the next start begins a
/// cut state of its own. `snap/s`'s commit.
async fn published_before_a_reboot(fx: &mut Fixture) -> CommitHash {
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x70);
    fx.group(&mut store, 7);
    let mut up = driver(fx, store);
    let asked = request(&mut up, "s", 0, 7);
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    up.run().await.expect("cut 1 and its publish");
    let snapshot = published(fx, "s", "snap/s").await.expect("snap/s");
    assert_eq!(snapshot.row.cut.group, 7);
    fx.rebooted(up);
    snapshot.commit
}

/// n1 and n2 of the review of #122, the remainder of its F1: head moved,
/// after cut 1's head swap landed and before the uploader read the row
/// and head again, to a commit **of cut 1's group** that is not cut 1's,
/// by another history whose groups repeat or by a rollback onto a
/// snapshot of a cut of the same group. Head's group then says nothing;
/// the row names cut 1's commit, and head is not it.
///
/// n1: cut 1 is found committed and let go, and `pool@t`, asked for cut
/// 1, is not published on head (it would be another volume's snapshot
/// read as cut 1's). n2: the next cut is not uploaded as a delta onto
/// head (it would write cut 2's blocks onto another history): its check
/// finds head moved, and head stays where the other writer left it.
#[tokio::test]
async fn head_at_another_commit_of_the_cuts_group_takes_no_publish_and_no_delta() {
    let cases = [
        (70, Moved::Repeated, Then::LostReply),
        (71, Moved::Repeated, Then::Restart),
        (72, Moved::RolledBack, Then::LostReply),
        (73, Moved::RolledBack, Then::Restart),
    ];
    for (seed, moved, then) in cases {
        let case = format!("{moved:?}, {then:?}");
        let mut fx = Fixture::new(seed).await;
        let earlier = match moved {
            Moved::RolledBack => Some(published_before_a_reboot(&mut fx).await),
            Moved::Repeated => None,
        };
        let (up, asked, cut1) = swap_unheard(&mut fx).await;
        match moved {
            Moved::Repeated => fx.move_head_with_group(7).await,
            Moved::RolledBack => fx.rollback("s").await,
        }
        let moved_to = fx.uploaded().await;
        assert_ne!(moved_to.head, cut1, "{case}: head moved");
        assert_eq!(moved_to.group, Some(7), "{case}: of cut 1's group");
        if let Some(earlier) = earlier {
            assert_eq!(moved_to.head, earlier, "{case}: snap/s's commit");
        }
        assert!(
            moved_to.content != fx.at[&7],
            "{case}: head is not the volume at cut 1"
        );

        // n1.
        let mut up = match then {
            Then::LostReply => {
                let mut up = up;
                let steps = up.run().await.expect("the run after");
                assert!(
                    matches!(told(&steps)[..], [Step::Uploaded { number: 1, .. }]),
                    "{case}: {steps:?}"
                );
                up
            }
            Then::Restart => {
                fx.died(up);
                let mut up = driver(&fx, fx.start());
                assert_eq!(up.publishing().ask(asked), Answer::Accepted, "{case}");
                let steps = up.run().await.expect("the run after");
                assert_eq!(
                    steps,
                    [
                        Step::PublishLooked { name: "t".into() },
                        Step::FoundCommitted { number: 1 }
                    ],
                    "{case}"
                );
                up
            }
        };
        assert!(
            published(&fx, "t", &case).await.is_none(),
            "{case}: snap/t on a commit not cut 1's"
        );
        assert_eq!(up.publishes().progress("t"), Progress::Pending, "{case}");
        assert_eq!(up.figures().not_offered, 1, "{case}");

        // n2.
        fx.write(up.local(), 101, 0x08);
        fx.group(up.local(), 8);
        let steps = up.run().await.expect("cut 2");
        assert_eq!(
            told(&steps),
            [Step::Mismatch {
                why: skyzfs_upload::Mismatch::HeadMoved,
                abandoned: vec![2]
            }],
            "{case}: cut 2 is not a delta onto another history"
        );
        let now = fx.uploaded().await;
        assert_eq!(now.head, moved_to.head, "{case}: the uploader moved head");
        assert_eq!(now.row, moved_to.row, "{case}: the row");
        assert!(published(&fx, "t", &case).await.is_none(), "{case}");
    }
}

/// A row of version 1, written by a daemon from before the row named its
/// commit, names cut 1, which that daemon committed before it died. The
/// next daemon finds cut 1 committed at its check and lets it go; the row
/// names no commit, so head is not known to be cut 1's, and `pool@t` is
/// not published on it (the lead's decision on F1).
#[tokio::test]
async fn a_cut_found_committed_under_a_row_of_version_1_takes_no_publish() {
    let mut fx = Fixture::new(74).await;
    let (up, asked, cut1) = swap_unheard(&mut fx).await;
    let named = fx.uploaded().await.row.expect("the row");
    assert_eq!(named.commit, Some(cut1), "the row names cut 1's commit");
    fx.died(up);
    fx.put_row(row_v1(&named.cut)).await;
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the run after");
    assert_eq!(
        steps,
        [
            Step::PublishLooked { name: "t".into() },
            Step::FoundCommitted { number: 1 }
        ]
    );
    assert_eq!(head(&fx).await, cut1, "head is cut 1's");
    assert!(published(&fx, "t", "a row of version 1").await.is_none());
    assert_eq!(up.publishes().progress("t"), Progress::Pending);
    assert_eq!(up.figures().not_offered, 1);
}

/// F2 of #122's review: a request of another cut state than the store's
/// is refused at once, before any cut has been checked, and does not keep
/// the name from a request of the store's state.
#[tokio::test]
async fn a_request_of_another_cut_state_is_refused_before_any_cut() {
    let mut fx = Fixture::new(62).await;
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.step().await, Ok(Step::Idle), "no cut yet");
    let right = up.local().cut_state_id().expect("the store's cut state");
    let stale = Request {
        name: "t".into(),
        attachment: ATTACHMENT,
        state: *b"the state before",
        after: 3,
        group: 7,
        manifest: manifest("t"),
        expiry: EXPIRY_DEFAULT,
    };
    assert_ne!(stale.state, right);
    assert_eq!(up.publishing().ask(stale), Answer::Refused(Refusal::State));
    assert_eq!(up.publishes().pending().0, 0);
    let fresh = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(fresh), Answer::Accepted);
    fx.write(up.local(), 100, 0x07);
    fx.group(up.local(), 7);
    up.run().await.expect("cut 1 and its publish");
    let done = published(&fx, "t", "the store's state")
        .await
        .expect("published");
    assert_eq!(done.row.cut.number, 1);
}

/// F5 of #122's review: `snap/t` published, then deleted in object
/// storage (the ref, as retention deletes a snapshot). Asked again, the
/// name is looked up, not answered from memory: the same request is
/// published anew on the first cut in view that carries it, and, once
/// deleted again, a new snapshot of the name goes on the next cut that
/// carries it.
#[tokio::test]
async fn a_name_deleted_and_asked_again_is_looked_up() {
    let mut fx = Fixture::new(68).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    up.run().await.expect("cut 1 and its publish");
    let first = published(&fx, "t", "first").await.expect("published");

    delete(&fx, "t").await;
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the publish again");
    assert_eq!(
        steps,
        [
            Step::PublishLooked { name: "t".into() },
            Step::Published {
                name: "t".into(),
                number: 1,
                commit: first.commit,
                found: false
            }
        ]
    );
    let again = published(&fx, "t", "again").await.expect("published anew");
    assert_ne!(again.row.snap, first.row.snap, "a snapshot of its own");

    delete(&fx, "t").await;
    let after = up.local().cuts().count;
    let anew = request(&mut up, "t", after, 8);
    assert_eq!(up.publishing().ask(anew), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(steps, [Step::PublishLooked { name: "t".into() }]);
    fx.write(up.local(), 101, 0x08);
    fx.group(up.local(), 8);
    up.run().await.expect("cut 2 and its publish");
    let anew = published(&fx, "t", "a new snapshot")
        .await
        .expect("published");
    assert_eq!((anew.row.cut.number, anew.row.cut.group), (2, 8));
}

/// F6 of #122's review (and Codex's P2 on it): `snap/t` published on cut
/// 1, then deleted as SkyNBD deletes a snapshot (the ref alone: the row
/// `skyzfs/snap/t` stays), and made anew by another on cut 2's commit. A
/// new uploader asked for the first publish again does not take that ref
/// for its own: the row names cut 1's commit, the ref cut 2's.
#[tokio::test]
async fn a_ref_made_anew_beside_a_row_left_behind_is_not_ours() {
    let mut fx = Fixture::new(66).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(up.publishing().ask(asked.clone()), Answer::Accepted);
    up.run().await.expect("cut 1 and its publish");
    published(&fx, "t", "first").await.expect("published");
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    up.run().await.expect("cut 2");
    let cut2 = head(&fx).await;
    delete(&fx, "t").await;
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    assert!(
        refs.meta_get(fx.vol, &snap_row_key("t"))
            .await
            .expect("the row")
            .is_some(),
        "the row left behind by the delete"
    );
    refs.transact(
        fx.vol,
        vec![RefOp::CasRef {
            name: snap_ref("t"),
            expected: None,
            new_commit: Some(cut2),
        }],
    )
    .await
    .expect("another's snapshot of the name");

    fx.died(up);
    let mut up = driver(&fx, fx.start());
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(
        steps,
        [Step::PublishRefused {
            name: "t".into(),
            why: Refusal::Taken
        }]
    );
    assert_eq!(
        up.publishes().progress("t"),
        Progress::Refused(Refusal::Taken)
    );
    let theirs = refs.get(fx.vol, &snap_ref("t")).await.expect("the ref");
    assert_eq!(theirs.map(|value| value.commit), Some(cut2));
}

/// T1 of #122's review: the name is taken by another between the lookup,
/// which found it absent, and the publish's transaction. The transaction
/// is refused (a conflict, once), the lookup after it finds the other's
/// ref, the publish ends refused, nothing of ours is written beside their
/// ref, and the cuts go on.
#[tokio::test]
async fn a_name_taken_between_the_lookup_and_the_transaction_is_refused() {
    let mut fx = Fixture::new(67).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let cut1 = head(&fx).await;
    assert_eq!(ask_now(&mut up, "t", 8), Answer::Accepted);
    assert_eq!(
        up.step().await,
        Ok(Step::PublishLooked { name: "t".into() })
    );
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.transact(
        fx.vol,
        vec![RefOp::CasRef {
            name: snap_ref("t"),
            expected: None,
            new_commit: Some(cut1),
        }],
    )
    .await
    .expect("another's snapshot");
    fx.write(up.local(), 100, 0x08);
    fx.group(up.local(), 8);
    let mut steps = Vec::new();
    let mut errors = 0;
    for _ in 0..50 {
        match up.step().await {
            Ok(Step::Idle) => break,
            Ok(step) => steps.push(step),
            Err(e) => {
                errors += 1;
                assert_eq!(e, RemoteError::Conflict);
            }
        }
    }
    assert_eq!(errors, 1, "the transaction is refused once: {steps:?}");
    assert_eq!(
        told(&steps).last(),
        Some(&Step::PublishRefused {
            name: "t".into(),
            why: Refusal::Taken
        }),
        "{steps:?}"
    );
    assert_eq!(
        up.publishes().progress("t"),
        Progress::Refused(Refusal::Taken)
    );
    let theirs = refs.get(fx.vol, &snap_ref("t")).await.expect("the ref");
    assert_eq!(theirs.map(|value| value.commit), Some(cut1));
    assert_eq!(
        refs.meta_get(fx.vol, &snap_row_key("t"))
            .await
            .expect("the row"),
        None,
        "no row of ours beside their ref"
    );
    assert_eq!(up.publishes().figures().refused, 1, "counted once refused");
    assert_eq!(up.publishes().figures().published, 0);
    fx.assert_uploaded(8, "cut 2").await;
}

/// A driver its maker did not tell its store's cut state learns it from
/// the first cut it checks (C03 of #122's review): a request asked before
/// then is refused (`State`); the same request asked after it is
/// accepted, and published on the cut that carries it.
#[tokio::test]
async fn a_driver_not_told_its_cut_state_learns_it_at_its_first_check() {
    let mut fx = Fixture::new(63).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.publishing().set_attachment(ATTACHMENT);
    let asked = request(&mut up, "t", 0, 7);
    assert_eq!(
        up.publishing().ask(asked.clone()),
        Answer::Refused(Refusal::State)
    );
    up.run().await.expect("cut 1");
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    up.run().await.expect("the publish");
    let done = published(&fx, "t", "after the first check")
        .await
        .expect("published");
    assert_eq!(done.row.cut.number, 1);
}

/// Q1 of #122's review, as the lead decided it, on the driver alone: a
/// publish whose deadline passes while it waits is not made on the cut
/// that carries it, though that cut goes up. It ends expired, counted,
/// and no ref, row or manifest is written.
#[tokio::test(start_paused = true)]
async fn a_publish_past_its_deadline_is_not_made_on_the_cut_that_carries_it() {
    let mut fx = Fixture::new(41).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x51);
    fx.group(&mut store, 51);
    let mut up = driver(&fx, store);
    up.run().await.expect("cut 1");
    let after = up.local().cuts().count;
    let mut asked = request(&mut up, "t", after, 52);
    asked.expiry = Duration::from_secs(60);
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    let steps = up.run().await.expect("the lookup");
    assert_eq!(steps, [Step::PublishLooked { name: "t".into() }]);
    assert_eq!(up.publishes().progress("t"), Progress::Pending);

    tokio::time::advance(Duration::from_secs(61)).await;
    fx.write(up.local(), 101, 0x52);
    fx.group(up.local(), 52);
    let steps = up.run().await.expect("cut 2");
    assert!(
        told(&steps)
            .iter()
            .all(|step| matches!(step, Step::Began { .. } | Step::Uploaded { .. })),
        "{steps:?}"
    );
    fx.assert_uploaded(52, "cut 2").await;
    assert_eq!(up.publishes().progress("t"), Progress::Expired);
    let figures = up.publishes().figures();
    assert_eq!((figures.expired, figures.published), (1, 0));
    assert!(published(&fx, "t", "cut 2").await.is_none());
}

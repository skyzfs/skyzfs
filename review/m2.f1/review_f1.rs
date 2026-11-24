//! The review's scenarios for F1 (#134), on the fixture of `tests/common`.

mod common;

use std::sync::{Arc, Mutex};

use common::*;
use skynbd_engine::Engine;
use skynbd_refs::{
    CasOutcome, Lease, LeaseOutcome, ObjectRefStore, RefEvent, RefOp, RefStore, RefValue,
    RenewOutcome, TxnOutcome,
};
use skynbd_store::{FaultOp, FaultWhen};
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_upload::publish::{snap_ref, Publishing, EXPIRY_DEFAULT};
use skyzfs_upload::{Answer, Doing, Driver, EngineRemote, Mismatch, Request, Rule, Step, ROW_KEY};

/// A refs store that, armed, moves head to `to` right after the next
/// read of head has returned: another writer's swap between two reads of
/// head (`observe`'s and `Engine::open`'s).
struct Racing {
    inner: Refs,
    armed: Mutex<Option<CommitHash>>,
}

#[async_trait::async_trait]
impl RefStore for Racing {
    async fn get(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Option<RefValue>> {
        let got = self.inner.get(vol, name).await?;
        if name == "head" {
            let to = self.armed.lock().unwrap().take();
            if let (Some(to), Some(cur)) = (to, got) {
                let out = self
                    .inner
                    .compare_and_swap(vol, "head", Some(cur), Some(to))
                    .await?;
                assert!(matches!(out, CasOutcome::Committed(_)), "the racing swap");
            }
        }
        Ok(got)
    }
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> skynbd_refs::Result<CasOutcome> {
        self.inner
            .compare_and_swap(vol, name, expected, new_commit)
            .await
    }
    async fn list(&self, vol: VolumeId) -> skynbd_refs::Result<Vec<(String, RefValue)>> {
        self.inner.list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        self.inner.watch()
    }
    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> skynbd_refs::Result<TxnOutcome> {
        self.inner.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<TxnOutcome> {
        self.inner.transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Vec<String>> {
        self.inner.pins(vol, name).await
    }
    async fn meta_get(&self, vol: VolumeId, key: &str) -> skynbd_refs::Result<Option<Vec<u8>>> {
        self.inner.meta_get(vol, key).await
    }
    async fn meta_list(
        &self,
        vol: VolumeId,
        prefix: &str,
    ) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
        self.inner.meta_list(vol, prefix).await
    }
    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<LeaseOutcome> {
        self.inner.lease_acquire(vol, holder, ttl_ms).await
    }
    async fn lease_renew(
        &self,
        vol: VolumeId,
        holder: &str,
        epoch: u64,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<RenewOutcome> {
        self.inner.lease_renew(vol, holder, epoch, ttl_ms).await
    }
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> skynbd_refs::Result<()> {
        self.inner.lease_release(vol, holder).await
    }
    async fn lease_get(&self, vol: VolumeId) -> skynbd_refs::Result<Option<Lease>> {
        self.inner.lease_get(vol).await
    }
}

type RaceUp = Driver<
    skyzfs_local::inplace::InPlace<skyzfs_local::sim::SimDisk>,
    EngineRemote<Store, Arc<Racing>>,
>;

fn refs(fx: &Fixture) -> ObjectRefStore<Recording> {
    ObjectRefStore::new(fx.store.inner().clone())
}

async fn head_value(fx: &Fixture) -> RefValue {
    refs(fx)
        .get(fx.vol, "head")
        .await
        .expect("head")
        .expect("a head")
}

async fn row_bytes(fx: &Fixture) -> Option<Vec<u8>> {
    refs(fx).meta_get(fx.vol, ROW_KEY).await.expect("the row")
}

fn told(steps: &[Step]) -> Vec<Step> {
    steps
        .iter()
        .filter(|s| !matches!(s, Step::Read { .. }))
        .cloned()
        .collect()
}

/// Another history's commit onto head (block 0 full of 0xCD, of group
/// `group`); then head swapped back where it was. Its hash.
async fn another_history(fx: &Fixture, group: Option<u64>) -> CommitHash {
    let before = head_value(fx).await;
    let inner = fx.store.inner().clone();
    let r = ObjectRefStore::new(inner.clone());
    let mut engine = Engine::open(inner, r, fx.vol, cfg()).await.expect("engine");
    engine.write(0, &[0xCD; BLOCK as usize]).expect("write");
    engine.set_group(group);
    let z = engine
        .commit(skynbd_format::CommitKind::Flush)
        .await
        .expect("commit");
    let now = head_value(fx).await;
    let out = refs(fx)
        .compare_and_swap(fx.vol, "head", Some(now), Some(before.commit))
        .await
        .expect("swap back");
    assert!(matches!(out, CasOutcome::Committed(_)));
    z
}

/// S1: cut 1 whole; a restart; cut 2 a delta onto cut 1. Another writer
/// swaps head to `z` (group `group`) right after `observe`'s read of head
/// and before the new engine's. What the run did, whether head then holds
/// the volume at cut 2, and whether head is still `z`.
async fn s1(seed: u64, group: Option<u64>) -> (Vec<Step>, bool, bool) {
    let mut fx = Fixture::new(seed).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    let c1 = head_value(&fx).await.commit;
    let z = another_history(&fx, group).await;
    assert_eq!(head_value(&fx).await.commit, c1, "head back at cut 1");
    fx.died(up);
    let mut store = fx.start();
    fx.write(&mut store, 11, 0x11);
    fx.group(&mut store, 8);
    let racing = Arc::new(Racing {
        inner: Arc::new(ObjectRefStore::new(fx.store.clone())),
        armed: Mutex::new(Some(z)),
    });
    let remote = EngineRemote::new(fx.store.clone(), racing, fx.vol, cfg());
    let mut up: RaceUp = Driver::with_slices(store, remote, 64);
    let steps = up.run().await.expect("cut 2");
    let content = fx.uploaded().await.content;
    let head = head_value(&fx).await.commit;
    (told(&steps), content == fx.at[&8], head == z)
}

#[tokio::test]
async fn review_f1_s1_a_head_swapped_between_observes_two_reads_takes_no_delta() {
    let mut bad = Vec::new();
    for (seed, group) in [(201, None), (202, Some(7)), (203, Some(99))] {
        let (steps, holds_cut2, still_z) = s1(seed, group).await;
        println!(
            "S1 z's group {group:?}: {steps:?}; head holds the volume at cut 2: {holds_cut2}; head still the other writer's: {still_z}"
        );
        let delta = steps.iter().any(|s| {
            matches!(
                s,
                Step::Uploaded {
                    rule: Rule::Delta,
                    ..
                }
            )
        });
        if delta && !holds_cut2 {
            bad.push(group);
        }
    }
    assert!(
        bad.is_empty(),
        "a delta went up onto another history, z's group {bad:?}"
    );
}

/// S2: the upgrade's whole cut, its reply lost. The row it replaces is
/// of version 1; the swap lands; the same uploader reads the row again,
/// now of version 2 naming the cut and head: the cut is handed on, and
/// `snap/t`, asked before, is published on its commit.
#[tokio::test]
async fn review_f1_s2_the_upgrades_whole_cut_with_its_reply_lost_carries_its_publish() {
    let mut fx = Fixture::new(204).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    let state = up.local().cut_state_id().expect("a state");
    *up.publishing() = Publishing::new(Some(*b"the attachment.."), Some(state));
    let named = skyzfs_upload::row::decode(&row_bytes(&fx).await.expect("a row"))
        .expect("a row")
        .cut;
    fx.put_row(row_v1(&named)).await;
    fx.write(up.local(), 11, 0x11);
    fx.group(up.local(), 8);
    let steps = up.run().await.expect("cut 2");
    assert!(
        steps.contains(&Step::Mismatch {
            why: Mismatch::HeadMoved,
            abandoned: vec![2]
        }),
        "{steps:?}"
    );
    let asked = Request {
        name: "t".into(),
        attachment: *b"the attachment..",
        state,
        after: 2,
        group: 9,
        manifest: b"m".to_vec(),
        expiry: EXPIRY_DEFAULT,
    };
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    fx.write(up.local(), 12, 0x12);
    fx.group(up.local(), 9);
    for _ in 0..400 {
        if up.doing() == Doing::Committing && !up.publish_due() {
            break;
        }
        up.step().await.expect("a step");
    }
    assert_eq!(up.doing(), Doing::Committing);
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
        .await;
    assert!(up.step_cut().await.is_err(), "the reply to the swap is lost");
    fx.store.disarm().await;
    let steps = up.run().await.expect("after");
    let head = head_value(&fx).await.commit;
    let snap = refs(&fx).get(fx.vol, &snap_ref("t")).await.expect("snap/t");
    println!(
        "S2 after the lost reply: {:?}; snap/t on head: {}; not_offered {}",
        told(&steps),
        snap.map(|v| v.commit) == Some(head),
        up.figures().not_offered
    );
    assert_eq!(snap.map(|v| v.commit), Some(head), "snap/t on cut 3's commit");
    assert!(fx.uploaded().await.content == fx.at[&9]);
}

/// S3: head moved away by another writer and rolled back onto exactly
/// the row's commit (a snapshot of the cut the row names) before the next
/// check: the commit is the cut's, so the delta goes up onto it, right.
#[tokio::test]
async fn review_f1_s3_head_back_on_the_rows_own_commit_takes_the_delta() {
    let mut fx = Fixture::new(205).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    let c1 = head_value(&fx).await.commit;
    refs(&fx)
        .transact(
            fx.vol,
            vec![RefOp::CasRef {
                name: snap_ref("op"),
                expected: None,
                new_commit: Some(c1),
            }],
        )
        .await
        .expect("snap/op");
    fx.move_head().await;
    assert_ne!(head_value(&fx).await.commit, c1);
    let inner = fx.store.inner().clone();
    let mut engine = Engine::open(inner.clone(), ObjectRefStore::new(inner), fx.vol, cfg())
        .await
        .expect("engine");
    engine.rollback("op").await.expect("rollback");
    assert_eq!(head_value(&fx).await.commit, c1, "head back on cut 1's commit");
    fx.write(up.local(), 11, 0x11);
    fx.group(up.local(), 8);
    // Head's generation moved under the same hash: the first commit's
    // swap conflicts, and the driver gives it up and checks again.
    let mut steps = Vec::new();
    let mut errors = Vec::new();
    for _ in 0..200 {
        match up.step().await {
            Ok(Step::Idle) => break,
            Ok(step) => steps.push(step),
            Err(e) => errors.push(e),
        }
    }
    println!("S3 {:?}; errors {errors:?}", told(&steps));
    assert!(
        steps.contains(&Step::Began {
            number: 2,
            rule: Rule::Delta
        }),
        "{steps:?}"
    );
    assert!(fx.uploaded().await.content == fx.at[&8]);
}

/// S4: a row of version 1 left by a daemon before F1, while another
/// writer had moved head to another history of the row's group (the
/// residual risk in the description): the whole cut after the upgrade's
/// mismatch goes up onto it, and head then holds the volume at that cut;
/// the delta after it is right too.
#[tokio::test]
async fn review_f1_s4_a_version_1_row_on_another_history_of_its_group_ends_right() {
    let mut fx = Fixture::new(206).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    let named = skyzfs_upload::row::decode(&row_bytes(&fx).await.expect("a row"))
        .expect("a row")
        .cut;
    fx.put_row(row_v1(&named)).await;
    let z = another_history(&fx, Some(7)).await;
    let now = head_value(&fx).await;
    refs(&fx)
        .compare_and_swap(fx.vol, "head", Some(now), Some(z))
        .await
        .expect("to z");
    fx.write(up.local(), 11, 0x11);
    fx.group(up.local(), 8);
    let s2 = up.run().await.expect("cut 2");
    fx.write(up.local(), 12, 0x12);
    fx.group(up.local(), 9);
    let s3 = up.run().await.expect("cut 3");
    let at3 = fx.uploaded().await.content == fx.at[&9];
    fx.write(up.local(), 13, 0x13);
    fx.group(up.local(), 10);
    let s4 = up.run().await.expect("cut 4");
    println!(
        "S4 cut 2 {:?}\nS4 cut 3 {:?} (head the volume at cut 3: {at3})\nS4 cut 4 {:?}",
        told(&s2),
        told(&s3),
        told(&s4)
    );
    let up4 = fx.uploaded().await;
    assert!(up4.fsck);
    assert!(at3);
    assert!(up4.content == fx.at[&10], "head is the volume at cut 4");
}

/// S5: a row of version 2 naming the cut being uploaded, written by a
/// commit whose reply was lost, while head was then moved by another
/// writer to a commit with **no** group (an operator's write) and back
/// to the row's commit by a rollback before the next read: the cut is
/// found committed on its own commit and carries its publish.
#[tokio::test]
async fn review_f1_s5_found_committed_with_head_back_on_its_commit_carries_its_publish() {
    let mut fx = Fixture::new(207).await;
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    let state = up.local().cut_state_id().expect("a state");
    *up.publishing() = Publishing::new(Some(*b"the attachment.."), Some(state));
    let asked = Request {
        name: "t".into(),
        attachment: *b"the attachment..",
        state,
        after: 0,
        group: 7,
        manifest: b"m".to_vec(),
        expiry: EXPIRY_DEFAULT,
    };
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    for _ in 0..400 {
        if up.doing() == Doing::Committing && !up.publish_due() {
            break;
        }
        up.step().await.expect("a step");
    }
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
        .await;
    assert!(up.step_cut().await.is_err(), "the reply to the swap is lost");
    fx.store.disarm().await;
    let c1 = head_value(&fx).await.commit;
    refs(&fx)
        .transact(
            fx.vol,
            vec![RefOp::CasRef {
                name: snap_ref("op"),
                expected: None,
                new_commit: Some(c1),
            }],
        )
        .await
        .expect("snap/op");
    fx.move_head().await;
    let inner = fx.store.inner().clone();
    let mut engine = Engine::open(inner.clone(), ObjectRefStore::new(inner), fx.vol, cfg())
        .await
        .expect("engine");
    engine.rollback("op").await.expect("rollback");
    assert_eq!(head_value(&fx).await.commit, c1);
    let steps = up.run().await.expect("after");
    let snap = refs(&fx).get(fx.vol, &snap_ref("t")).await.expect("snap/t");
    println!(
        "S5 {:?}; snap/t on cut 1's commit: {}",
        told(&steps),
        snap.map(|v| v.commit) == Some(c1)
    );
    assert_eq!(snap.map(|v| v.commit), Some(c1));
}

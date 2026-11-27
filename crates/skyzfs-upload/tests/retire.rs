//! Retiring a snapshot (work package M2.7b-1): `snap/<name>` and its row
//! deleted in one transaction, where the ref still names the commit the
//! agent recorded. Node B lists snapshots by their refs, so a retired
//! snapshot is no longer listed.

mod common;

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use common::*;
use skynbd_refs::{
    CasOutcome, Lease, LeaseOutcome, ObjectRefStore, RefEvent, RefOp, RefStore, RefValue,
    RenewOutcome, TxnOutcome,
};
use skynbd_store::FaultOp;
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_upload::publish::{snap_ref, snap_row_key, Publishing, EXPIRY_DEFAULT};
use skyzfs_upload::remote::{Remote, RemoteError, Retired, RETIRE_TRIES};
use skyzfs_upload::{Answer, EngineRemote, Request};

const ATTACHMENT: [u8; 16] = *b"the attachment..";

/// `snap/t` published on cut 1, of group 7: its commit.
async fn published(fx: &mut Fixture) -> CommitHash {
    let mut store = fx.start();
    fx.write(&mut store, 100, 0x07);
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    let state = up.local().cut_state_id().expect("a store that holds cuts");
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    up.run().await.expect("cut 1");
    let after = 0;
    let asked = Request {
        name: "t".into(),
        attachment: ATTACHMENT,
        state,
        after,
        group: 7,
        manifest: b"the ZFS manifest of pool@t".to_vec(),
        expiry: EXPIRY_DEFAULT,
    };
    assert_eq!(up.publishing().ask(asked), Answer::Accepted);
    up.run().await.expect("the publish");
    let commit = snap(fx, "t").await.0.expect("snap/t");
    assert_eq!(commit.commit, head(fx).await);
    commit.commit
}

/// `snap/<name>` and its row, as the refs object has them.
async fn snap(fx: &Fixture, name: &str) -> (Option<RefValue>, Option<Vec<u8>>) {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let found = refs.get(fx.vol, &snap_ref(name)).await.expect("the ref");
    let row = refs
        .meta_get(fx.vol, &snap_row_key(name))
        .await
        .expect("the row");
    (found, row)
}

async fn head(fx: &Fixture) -> CommitHash {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.get(fx.vol, "head")
        .await
        .expect("head")
        .expect("a head")
        .commit
}

/// A ref that names `commit`, whatever its generation.
fn names(value: &Option<RefValue>, commit: CommitHash) -> bool {
    value.is_some_and(|v| v.commit == commit)
}

/// Mutants: the ref deleted and the row kept, or the other way round;
/// a ref of another commit deleted; a ref that is gone answered as
/// retired (an outcome line that a monitor could not tell from the
/// first).
#[tokio::test]
async fn a_retire_deletes_the_ref_and_its_row_only_where_they_name_its_commit() {
    let mut fx = Fixture::new(81).await;
    let commit = published(&mut fx).await;
    let before = snap(&fx, "t").await;
    assert!(before.1.is_some(), "the row");
    let head_before = head(&fx).await;
    let mut remote = fx.remote();

    let other = [0x0b; 32];
    assert_eq!(
        remote.retire("t", other).await,
        Ok(Retired::Other(commit)),
        "another commit than the ref's"
    );
    assert_eq!(snap(&fx, "t").await, before, "left as it is");

    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Retired));
    assert_eq!(snap(&fx, "t").await, (None, None), "the ref and its row");
    assert_eq!(head(&fx).await, head_before, "head is not touched");

    // Asked again, as after an answer lost: gone, and nothing else moves.
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Gone));
    assert_eq!(remote.retire("u", commit).await, Ok(Retired::Gone));
    assert_eq!(head(&fx).await, head_before);
}

/// A row left with no ref (a publish's row whose ref a person deleted)
/// is not the retire's: `gone`, and the row stays for whoever reads it.
#[tokio::test]
async fn a_row_with_no_ref_is_gone_and_left() {
    let mut fx = Fixture::new(82).await;
    let commit = published(&mut fx).await;
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let value = refs.get(fx.vol, &snap_ref("t")).await.expect("the ref");
    let deleted = refs
        .transact(
            fx.vol,
            vec![RefOp::CasRef {
                name: snap_ref("t"),
                expected: value,
                new_commit: None,
            }],
        )
        .await
        .expect("the delete");
    assert_eq!(deleted, TxnOutcome::Committed);
    let row = snap(&fx, "t").await.1;
    assert!(row.is_some());
    let mut remote = fx.remote();
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Gone));
    assert_eq!(snap(&fx, "t").await, (None, row));
}

/// A ref with no row (made by hand, or by a writer that is not an
/// uploader) is retired as one with its row. Mutant: a retire that
/// expects the row and refuses without it.
#[tokio::test]
async fn a_ref_with_no_row_is_retired() {
    let mut fx = Fixture::new(89).await;
    let commit = published(&mut fx).await;
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let outcome = refs
        .transact(
            fx.vol,
            vec![RefOp::MetaDelete {
                key: snap_row_key("t"),
            }],
        )
        .await
        .expect("the row's delete");
    assert_eq!(outcome, TxnOutcome::Committed);
    let mut remote = fx.remote();
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Retired));
    assert_eq!(snap(&fx, "t").await, (None, None));
}

/// Object storage that fails: an error, and nothing deleted.
#[tokio::test]
async fn a_retire_that_object_storage_fails_deletes_nothing() {
    let mut fx = Fixture::new(83).await;
    let commit = published(&mut fx).await;
    let before = snap(&fx, "t").await;
    let mut remote = fx.remote();
    fx.store.arm(FaultOp::Any, "", 0).await;
    let failed = remote.retire("t", commit).await;
    fx.store.disarm().await;
    assert!(matches!(failed, Err(RemoteError::Store(_))), "{failed:?}");
    assert_eq!(snap(&fx, "t").await, before);
}

/// A refs store that, before each of the retire's transactions, applies
/// the next of `moves` to the refs object: another writer that moves the
/// ref between the retire's read and its transaction.
#[derive(Clone)]
struct Moving {
    inner: Refs,
    moves: Arc<Mutex<VecDeque<Vec<RefOp>>>>,
}

impl Moving {
    async fn moved(&self, vol: VolumeId) {
        let next = self.moves.lock().expect("moves").pop_front();
        if let Some(ops) = next {
            let outcome = self.inner.transact(vol, ops).await.expect("the move");
            assert_eq!(outcome, TxnOutcome::Committed, "the move lands");
        }
    }
}

#[async_trait::async_trait]
impl RefStore for Moving {
    async fn get(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Option<RefValue>> {
        self.inner.get(vol, name).await
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
        self.moved(vol).await;
        self.inner.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<TxnOutcome> {
        self.moved(vol).await;
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

/// The fixture's remote, with `moves` made before its transactions.
fn moving(fx: &Fixture, moves: Vec<Vec<RefOp>>) -> EngineRemote<Store, Moving> {
    let refs = Moving {
        inner: Arc::new(ObjectRefStore::new(fx.store.clone())),
        moves: Arc::new(Mutex::new(moves.into())),
    };
    EngineRemote::new(fx.store.clone(), refs, fx.vol, cfg())
}

/// A rewrite of `snap/t` to `commit`, from `from`: a move that keeps or
/// changes the commit, and always changes the generation.
fn rewrite(from: RefValue, commit: CommitHash) -> Vec<RefOp> {
    vec![RefOp::CasRef {
        name: snap_ref("t"),
        expected: Some(from),
        new_commit: Some(commit),
    }]
}

/// The ref rewritten on the same commit between the read and the
/// transaction: the conflict's answer is read, and the retire is made on
/// it. Mutant: a conflict taken as `other` or as an error at once.
#[tokio::test]
async fn a_ref_rewritten_on_its_commit_under_the_retire_is_retired() {
    let mut fx = Fixture::new(84).await;
    let commit = published(&mut fx).await;
    let value = snap(&fx, "t").await.0.expect("snap/t");
    let mut remote = moving(&fx, vec![rewrite(value, commit)]);
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Retired));
    assert_eq!(snap(&fx, "t").await, (None, None));
}

/// The ref moved to another commit between the read and the
/// transaction: `other`, with the commit it names now, and it is left
/// with its row. Mutant: the conflict's answer not read, or deleted.
#[tokio::test]
async fn a_ref_moved_to_another_commit_under_the_retire_is_other() {
    let mut fx = Fixture::new(85).await;
    let commit = published(&mut fx).await;
    let value = snap(&fx, "t").await.0.expect("snap/t");
    let theirs = [0x7e; 32];
    let mut remote = moving(&fx, vec![rewrite(value, theirs)]);
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Other(theirs)));
    let (now, row) = snap(&fx, "t").await;
    assert!(names(&now, theirs), "{now:?}");
    assert!(row.is_some(), "the row is left");
}

/// The ref deleted between the read and the transaction: `gone`.
#[tokio::test]
async fn a_ref_deleted_under_the_retire_is_gone() {
    let mut fx = Fixture::new(86).await;
    let commit = published(&mut fx).await;
    let value = snap(&fx, "t").await.0.expect("snap/t");
    let delete = vec![RefOp::CasRef {
        name: snap_ref("t"),
        expected: Some(value),
        new_commit: None,
    }];
    let mut remote = moving(&fx, vec![delete]);
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Gone));
}

/// A ref rewritten before each of the retire's transactions: it gives up
/// after [`RETIRE_TRIES`] with a conflict, the ref and its row as they
/// were left. Mutant: a retire that tries without end, or once.
#[tokio::test]
async fn a_ref_that_moves_at_each_try_is_a_conflict_after_the_tries() {
    let mut fx = Fixture::new(87).await;
    let commit = published(&mut fx).await;
    let mut value = snap(&fx, "t").await.0.expect("snap/t");
    let mut moves = Vec::new();
    for _ in 0..RETIRE_TRIES {
        moves.push(rewrite(value, commit));
        value = RefValue {
            commit,
            generation: value.generation + 1,
        };
    }
    let mut remote = moving(&fx, moves);
    assert_eq!(remote.retire("t", commit).await, Err(RemoteError::Conflict));
    let (now, row) = snap(&fx, "t").await;
    assert_eq!(now, Some(value), "every move landed, and no delete");
    assert!(row.is_some());

    // One move fewer: the last try retires it.
    let mut fx = Fixture::new(88).await;
    let commit = published(&mut fx).await;
    let mut value = snap(&fx, "t").await.0.expect("snap/t");
    let mut moves = Vec::new();
    for _ in 1..RETIRE_TRIES {
        moves.push(rewrite(value, commit));
        value = RefValue {
            commit,
            generation: value.generation + 1,
        };
    }
    let mut remote = moving(&fx, moves);
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Retired));
    assert_eq!(snap(&fx, "t").await, (None, None));
}

/// A retire whose transaction lands and whose answer is lost: the ref
/// and its row went together, so asking again finds `gone` and nothing
/// is left behind. G1 of the review of #175. Mutant: the ref and the row
/// deleted in two transactions, the first of which lands and fails
/// (`split-txn`), leaving the row.
#[tokio::test]
async fn a_retire_that_lands_unheard_deleted_the_ref_and_its_row_together() {
    let mut fx = Fixture::new(84).await;
    let commit = published(&mut fx).await;
    let mut remote = fx.remote();
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, skynbd_store::FaultWhen::After)
        .await;
    let failed = remote.retire("t", commit).await;
    fx.store.disarm().await;
    assert!(matches!(failed, Err(RemoteError::Store(_))), "{failed:?}");
    assert_eq!(snap(&fx, "t").await, (None, None), "both went, at once");
    let mut remote = fx.remote();
    assert_eq!(remote.retire("t", commit).await, Ok(Retired::Gone));
}

/// A refs store that fences writes and has no lease row for the volume
/// refuses the retire's transaction on its first try: given as the
/// volume's state, and not as `snap/T` moving under each of the tries
/// (L5 of the next look at #175). Nothing is deleted.
///
/// Mutant: the missing row given as [`RemoteError::Conflict`].
#[tokio::test]
async fn a_retire_with_no_lease_row_is_refused_as_the_volume() {
    let mut fx = Fixture::new(77).await;
    let commit = published(&mut fx).await;
    let refs: Refs = Arc::new(ObjectRefStore::new(fx.store.clone()));
    let acting = Arc::new(skynbd_refs::ActingRefStore::new(refs, "a holder", 1));
    let mut remote = EngineRemote::new(fx.store.clone(), acting, fx.vol, cfg());
    let refused = remote.retire("t", commit).await;
    assert!(
        matches!(&refused, Err(RemoteError::Volume(why)) if why.contains("no lease row")),
        "{refused:?}"
    );
    let (found, row) = snap(&fx, "t").await;
    assert!(names(&found, commit), "{found:?}");
    assert!(row.is_some(), "the row is kept");
}

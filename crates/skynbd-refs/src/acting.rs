//! `ActingRefStore`: a `RefStore` that performs every mutation *as* a
//! fixed `(holder, epoch)` principal (Invariant F, task 15h).
//!
//! The ambient-write knot it resolves: when a volume's lease is held
//! under a **node's** identity (a lineage serve with a holder override),
//! the serving server's own engine writes — head CAS during ingest,
//! prepared-ref commits — would trip the object store's ambient
//! `cfg.holder` fence, because the server is not the lease holder; the
//! node is. Wrapping the volume's refs handle in this type at serve time
//! makes every engine write for that volume fenced under the node's
//! identity and stage-time epoch *by construction*: a superseded epoch
//! fails the write with `LeaseLost` no matter which code path issued it.
//!
//! Reads and lease operations delegate untouched — fencing is a write
//! concern, and the lease ops are their own authority on identity.

use skynbd_types::{CommitHash, VolumeId};

use crate::{
    CasOutcome, Lease, LeaseOutcome, RefEvent, RefOp, RefStore, RefValue, Result, TxnOutcome,
};

pub struct ActingRefStore<R> {
    inner: R,
    holder: String,
    epoch: u64,
}

impl<R> ActingRefStore<R> {
    pub fn new(inner: R, holder: impl Into<String>, epoch: u64) -> Self {
        Self {
            inner,
            holder: holder.into(),
            epoch,
        }
    }

    pub fn holder(&self) -> &str {
        &self.holder
    }

    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}

#[async_trait::async_trait]
impl<R: RefStore> RefStore for ActingRefStore<R> {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>> {
        self.inner.get(vol, name).await
    }

    /// CAS as one fenced single-op transaction. The stored value a commit
    /// reports is reconstructed from the CAS inputs — generation
    /// assignment (`expected.generation + 1`, or 0) is the store
    /// contract, so the answer is exact.
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> Result<CasOutcome> {
        let op = RefOp::CasRef {
            name: name.to_string(),
            expected,
            new_commit,
        };
        match self
            .inner
            .transact_as(vol, vec![op], &self.holder, self.epoch)
            .await?
        {
            TxnOutcome::Committed => Ok(CasOutcome::Committed(new_commit.map(|commit| RefValue {
                commit,
                generation: expected.map(|e| e.generation + 1).unwrap_or(0),
            }))),
            TxnOutcome::Conflict { actual, .. } => Ok(CasOutcome::Conflict(actual)),
        }
    }

    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>> {
        self.inner.list(vol).await
    }

    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        self.inner.watch()
    }

    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> Result<TxnOutcome> {
        self.inner
            .transact_as(vol, ops, &self.holder, self.epoch)
            .await
    }

    /// An explicit principal supersedes the stored one: a route that
    /// validated a request's epoch fences under exactly that epoch.
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> Result<TxnOutcome> {
        self.inner.transact_as(vol, ops, holder, epoch).await
    }

    async fn pins(&self, vol: VolumeId, name: &str) -> Result<Vec<String>> {
        self.inner.pins(vol, name).await
    }

    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.meta_get(vol, key).await
    }

    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        self.inner.meta_list(vol, prefix).await
    }

    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<LeaseOutcome> {
        self.inner.lease_acquire(vol, holder, ttl_ms).await
    }

    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()> {
        self.inner.lease_release(vol, holder).await
    }

    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>> {
        self.inner.lease_get(vol).await
    }

    async fn flush_volume(&self, vol: VolumeId) -> Result<()> {
        self.inner
            .flush_volume_as(vol, &self.holder, self.epoch)
            .await
    }

    async fn flush_volume_as(&self, vol: VolumeId, holder: &str, epoch: u64) -> Result<()> {
        self.inner.flush_volume_as(vol, holder, epoch).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryRefStore;

    #[tokio::test]
    async fn acting_store_fences_ambient_writes() {
        let inner = MemoryRefStore::new();
        let v = VolumeId(ulid::Ulid::from_parts(0xAC, 1));
        inner.lease_acquire(v, "node-a", 60_000).await.unwrap();

        let acting = ActingRefStore::new(inner.clone(), "node-a", 1);
        // Plain transact and CAS both route through the fence and commit
        // under the matching principal.
        let out = acting
            .transact(
                v,
                vec![RefOp::MetaPut {
                    key: "k".into(),
                    value: b"v".to_vec(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed);
        let out = acting
            .compare_and_swap(v, "head", None, Some([1u8; 32]))
            .await
            .unwrap();
        match out {
            CasOutcome::Committed(Some(rv)) => assert_eq!(rv.generation, 0),
            other => panic!("expected committed, got {other:?}"),
        }
        // The reconstructed generation matches the stored one.
        assert_eq!(inner.get(v, "head").await.unwrap().unwrap().generation, 0);
        // CAS conflict semantics survive the transact_as detour.
        let stale = RefValue {
            commit: [9u8; 32],
            generation: 4,
        };
        match acting
            .compare_and_swap(v, "head", Some(stale), Some([2u8; 32]))
            .await
            .unwrap()
        {
            CasOutcome::Conflict(Some(actual)) => assert_eq!(actual.commit, [1u8; 32]),
            other => panic!("expected conflict, got {other:?}"),
        }

        // A superseded principal is fenced on every write path.
        let stale_epoch = ActingRefStore::new(inner.clone(), "node-a", 0);
        assert!(matches!(
            stale_epoch
                .transact(
                    v,
                    vec![RefOp::Pin {
                        name: "head".into(),
                        holder: "x".into()
                    }]
                )
                .await,
            Err(crate::RefError::LeaseLost { epoch: 1, .. })
        ));
        let cur = inner.get(v, "head").await.unwrap();
        assert!(matches!(
            stale_epoch
                .compare_and_swap(v, "head", cur, Some([3u8; 32]))
                .await,
            Err(crate::RefError::LeaseLost { .. })
        ));
    }
}

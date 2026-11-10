//! The cadenced refs tier (design D-M4): the refs *object* is written on
//! the snapshot cadence, a timer, and the handoff points — never per
//! head CAS — which turns ~$26/volume/month of conditional PUTs at
//! 2 fsync/s into ~$0.02.
//!
//! The shape: a [`MemoryRefStore`] is the working copy, hydrated from
//! the refs object on a volume's first touch; every mutation applies to
//! it synchronously (same semantics, same events, same generations —
//! seeded verbatim). What differs is *when* the object learns about it:
//!
//! - **Lease operations pass through immediately.** Exclusivity is the
//!   whole point of the object; a cached lease would be fiction.
//! - **Snapshots, pins, meta — anything that is not `head`/`rpo` —
//!   export immediately**, and the operation fails if the export does:
//!   an API-confirmed snapshot must survive the owner's death
//!   (publish-implies-pin durability, I8).
//! - **`head` and `rpo` stay local** between exports. They move once per
//!   fsync/interval, they are exactly the expensive rows, and the gap
//!   they leave is what the D-M4 recovery scan closes: commits carry
//!   their epoch (task 6), so the next owner adopts the tail of the
//!   *owner of record* and nothing else.
//!
//! The single-epoch-gap invariant the scan needs — the un-exported tail
//! belongs to exactly the lease row's owner — holds because callers
//! export at both handoff edges: [`RefStore::flush_volume`] at serve
//! (after recovery) and before release ([`Self::lease_release`] exports
//! internally). Between those, only the owner of record can commit.

use std::collections::HashSet;

use skynbd_store::ObjectStore;
use skynbd_types::VolumeId;
use tokio::sync::Mutex;

use crate::object_store::VolumeRows;
use crate::{
    CasOutcome, Lease, LeaseOutcome, MemoryRefStore, ObjectRefStore, RefEvent, RefOp, RefStore,
    RefValue, Result, TxnOutcome,
};

/// Ref names that defer to the cadence rather than exporting on every
/// committed change: the per-fsync movers.
fn deferred(name: &str) -> bool {
    matches!(name, "head" | "rpo")
}

/// Whether any op in a transaction must export immediately. Pins and
/// meta rows always do: they are the durability side of
/// publish-implies-pin (I8).
fn any_immediate(ops: &[RefOp]) -> bool {
    ops.iter().any(|op| match op {
        RefOp::CasRef { name, .. } => !deferred(name),
        RefOp::Pin { .. } | RefOp::Unpin { .. } => true,
        RefOp::MetaPut { .. } | RefOp::MetaCas { .. } | RefOp::MetaDelete { .. } => true,
    })
}

pub struct CadencedRefStore<S: ObjectStore> {
    authority: ObjectRefStore<S>,
    cache: MemoryRefStore,
    /// Volumes whose cache is hydrated. Guarded by an async mutex so a
    /// volume hydrates exactly once, without holding any lock across
    /// the actual mutation paths.
    loaded: Mutex<HashSet<VolumeId>>,
}

impl<S: ObjectStore> CadencedRefStore<S> {
    pub fn new(authority: ObjectRefStore<S>) -> Self {
        Self {
            authority,
            cache: MemoryRefStore::new(),
            loaded: Mutex::new(HashSet::new()),
        }
    }

    /// Hydrate the volume's cache from the refs object, once. Seeds the
    /// rows verbatim — generations included — so the cache answers
    /// exactly as the authority would have.
    async fn ensure_loaded(&self, vol: VolumeId) -> Result<()> {
        let mut loaded = self.loaded.lock().await;
        if loaded.contains(&vol) {
            return Ok(());
        }
        let rows = self.authority.volume_rows(vol).await?;
        {
            let mut refs = self.cache.inner.write().await;
            let mut pins = self.cache.pins.write().await;
            let mut meta = self.cache.meta.write().await;
            for (name, rv) in rows.refs {
                refs.insert((vol, name), rv);
            }
            for (name, holder) in rows.pins {
                pins.entry((vol, name)).or_default().insert(holder);
            }
            for (key, value) in rows.meta {
                meta.insert((vol, key), value);
            }
        }
        loaded.insert(vol);
        Ok(())
    }

    /// The cache's current rows for a volume, for export.
    async fn snapshot_rows(&self, vol: VolumeId) -> VolumeRows {
        let refs = self.cache.inner.read().await;
        let pins = self.cache.pins.read().await;
        let meta = self.cache.meta.read().await;
        VolumeRows {
            refs: refs
                .iter()
                .filter(|((v, _), _)| *v == vol)
                .map(|((_, n), rv)| (n.clone(), *rv))
                .collect(),
            pins: pins
                .iter()
                .filter(|((v, _), _)| *v == vol)
                .flat_map(|((_, n), hs)| hs.iter().map(move |h| (n.clone(), h.clone())))
                .collect(),
            meta: meta
                .iter()
                .filter(|((v, _), _)| *v == vol)
                .map(|((_, k), b)| (k.clone(), b.clone()))
                .collect(),
        }
    }

    /// Export the volume's cached rows to the refs object (fenced,
    /// lease-renewing). The cadence write.
    pub async fn export(&self, vol: VolumeId) -> Result<()> {
        // Only export what was hydrated (or mutated) here: exporting an
        // untouched volume would round-trip its own state at best.
        if !self.loaded.lock().await.contains(&vol) {
            return Ok(());
        }
        let rows = self.snapshot_rows(vol).await;
        self.authority.replace_volume(vol, rows).await
    }

    /// [`Self::export`] under an explicit `(holder, epoch)` principal
    /// (task 15h): the export write of a volume whose lease a node holds.
    pub async fn export_as(&self, vol: VolumeId, holder: &str, epoch: u64) -> Result<()> {
        if !self.loaded.lock().await.contains(&vol) {
            return Ok(());
        }
        let rows = self.snapshot_rows(vol).await;
        self.authority
            .replace_volume_as(vol, rows, holder, epoch)
            .await
    }

    /// Drop the volume's hydration (task 15h): remove it from `loaded`
    /// and purge its cached rows, so the next touch re-hydrates from the
    /// authority. Called whenever a fence refusal (`LeaseLost`) proves
    /// the cache can no longer be trusted — either it holds ops a fenced
    /// transaction applied locally before the export was refused, or it
    /// predates an interim owner's writes. Without this, a later
    /// successful export would replay rejected ops (or erase the interim
    /// owner's history) wholesale.
    async fn invalidate(&self, vol: VolumeId) {
        let mut loaded = self.loaded.lock().await;
        let mut refs = self.cache.inner.write().await;
        let mut pins = self.cache.pins.write().await;
        let mut meta = self.cache.meta.write().await;
        loaded.remove(&vol);
        refs.retain(|(v, _), _| *v != vol);
        pins.retain(|(v, _), _| *v != vol);
        meta.retain(|(v, _), _| *v != vol);
    }

    /// Route a result through the invalidation rule: ANY `LeaseLost`
    /// drops the volume's cache before the error propagates.
    async fn seal<T>(&self, vol: VolumeId, r: Result<T>) -> Result<T> {
        if let Err(crate::RefError::LeaseLost { .. }) = &r {
            self.invalidate(vol).await;
        }
        r
    }

    /// Export every hydrated volume — the timer's tick. Failures are
    /// returned per volume; the timer logs them and the recovery scan
    /// covers the gap regardless.
    pub async fn export_all(&self) -> Vec<(VolumeId, crate::RefError)> {
        let vols: Vec<VolumeId> = self.loaded.lock().await.iter().copied().collect();
        let mut failed = Vec::new();
        for vol in vols {
            if let Err(e) = self.export(vol).await {
                failed.push((vol, e));
            }
        }
        failed
    }
}

#[async_trait::async_trait]
impl<S: ObjectStore> RefStore for CadencedRefStore<S> {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>> {
        self.ensure_loaded(vol).await?;
        self.cache.get(vol, name).await
    }

    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<skynbd_types::CommitHash>,
    ) -> Result<CasOutcome> {
        self.ensure_loaded(vol).await?;
        let out = self
            .cache
            .compare_and_swap(vol, name, expected, new_commit)
            .await?;
        if matches!(out, CasOutcome::Committed(_)) && !deferred(name) {
            let export = self.export(vol).await;
            self.seal(vol, export).await?;
        }
        Ok(out)
    }

    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>> {
        self.ensure_loaded(vol).await?;
        self.cache.list(vol).await
    }

    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        self.cache.watch()
    }

    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> Result<TxnOutcome> {
        self.ensure_loaded(vol).await?;
        let immediate = any_immediate(&ops);
        let out = self.cache.transact(vol, ops).await?;
        if matches!(out, TxnOutcome::Committed) && immediate {
            let export = self.export(vol).await;
            self.seal(vol, export).await?;
        }
        Ok(out)
    }

    /// The fenced transact (task 15h). The fence is read from the
    /// authority — a cached lease would be fiction — *before* the cache
    /// mutates; the export that makes an immediate op durable re-runs
    /// the same check atomically with its conditional PUT, so a steal
    /// interleaving the two fails the operation rather than landing a
    /// superseded write. Any `LeaseLost` — from the pre-check or from
    /// the export — invalidates the volume's cache: on the export path
    /// the cache already applied (and broadcast) the rejected ops, and a
    /// later successful export must not replay them; on the pre-check
    /// path the cache predates the interim owner's writes and must
    /// re-hydrate before it may export again.
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> Result<TxnOutcome> {
        self.ensure_loaded(vol).await?;
        let current = self.authority.lease_get(vol).await?;
        if let Err(e) = crate::lease::fence(current.as_ref(), vol, holder, epoch) {
            self.invalidate(vol).await;
            return Err(e);
        }
        let immediate = any_immediate(&ops);
        let out = self.cache.transact(vol, ops).await?;
        if matches!(out, TxnOutcome::Committed) && immediate {
            let export = self.export_as(vol, holder, epoch).await;
            self.seal(vol, export).await?;
        }
        Ok(out)
    }

    async fn pins(&self, vol: VolumeId, name: &str) -> Result<Vec<String>> {
        self.ensure_loaded(vol).await?;
        self.cache.pins(vol, name).await
    }

    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>> {
        self.ensure_loaded(vol).await?;
        self.cache.meta_get(vol, key).await
    }

    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        self.ensure_loaded(vol).await?;
        self.cache.meta_list(vol, prefix).await
    }

    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<LeaseOutcome> {
        self.authority.lease_acquire(vol, holder, ttl_ms).await
    }

    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()> {
        // Export-before-release: the successor must see everything this
        // owner acked — the write half of the single-epoch-gap
        // invariant. A failed export fails the release; the lease TTL
        // then covers the crash-shaped outcome. The export runs under
        // the owner's own principal (a node-held lineage serve, task
        // 15h, releases as the node) — and ONLY when the releasing
        // holder actually owns the lease. A non-owner's release is a
        // no-op by contract (the thief's state must survive a
        // straggler's shutdown path), so it must never export this
        // process's stale cache over the current owner's rows.
        match self.authority.lease_get(vol).await? {
            Some(l) if l.holder == holder => {
                let export = self.export_as(vol, &l.holder, l.epoch).await;
                self.seal(vol, export).await?;
            }
            _ => {}
        }
        self.authority.lease_release(vol, holder).await
    }

    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>> {
        self.authority.lease_get(vol).await
    }

    async fn flush_volume(&self, vol: VolumeId) -> Result<()> {
        let export = self.export(vol).await;
        self.seal(vol, export).await
    }

    async fn flush_volume_as(&self, vol: VolumeId, holder: &str, epoch: u64) -> Result<()> {
        let export = self.export_as(vol, holder, epoch).await;
        self.seal(vol, export).await
    }
}

/// Shared-authority conformance: the cadenced store must be
/// indistinguishable from the store it fronts, to a single caller.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;
    use skynbd_store::MemoryStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn cadenced_store_passes_ref_conformance() {
        let s = CadencedRefStore::new(ObjectRefStore::new(MemoryStore::new()));
        conformance::exercise_ref_store(&s).await;
    }

    #[tokio::test]
    async fn cadenced_store_passes_lease_conformance() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock: crate::Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let s = CadencedRefStore::new(ObjectRefStore::new(MemoryStore::new()).with_clock(clock));
        let advance = move |ms: u64| _ = now.fetch_add(ms, Ordering::SeqCst);
        conformance::exercise_lease(&s, &advance).await;
    }

    #[tokio::test]
    async fn cadenced_store_passes_fenced_transact_conformance() {
        use std::sync::atomic::{AtomicU64, Ordering};
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock: crate::Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let s = CadencedRefStore::new(ObjectRefStore::new(MemoryStore::new()).with_clock(clock));
        let advance = move |ms: u64| _ = now.fetch_add(ms, Ordering::SeqCst);
        conformance::exercise_fenced_transact(&s, &advance).await;
    }

    use crate::{LeaseOutcome, RefError};
    use skynbd_store::Precondition;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    fn test_clock() -> (Arc<AtomicU64>, crate::Clock) {
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock: crate::Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        (now, clock)
    }

    fn cas(name: &str, byte: u8) -> RefOp {
        RefOp::CasRef {
            name: name.into(),
            expected: None,
            new_commit: Some([byte; 32]),
        }
    }

    const TTL: u64 = 10_000;

    /// B2: a straggler's `lease_release` (its lease was stolen while it
    /// was away) is a no-op end to end — it must NOT export this
    /// process's stale cache over the current owner's rows.
    #[tokio::test]
    async fn straggler_release_leaves_the_owners_export_untouched() {
        let shared = Arc::new(MemoryStore::new());
        let (now, clock) = test_clock();
        let v = skynbd_types::VolumeId(ulid::Ulid::from_parts(0xB2, 1));

        let old = CadencedRefStore::new(
            ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn skynbd_store::ObjectStore>)
                .with_clock(Arc::clone(&clock)),
        );
        // node-a owns, publishes snap/a (immediate → exported + cached).
        old.lease_acquire(v, "node-a", TTL).await.unwrap();
        old.transact_as(v, vec![cas("snap/a", 1)], "node-a", 1)
            .await
            .unwrap();

        // node-a goes silent; node-b steals and publishes snap/b.
        now.fetch_add(TTL * 6, Ordering::SeqCst);
        let new = CadencedRefStore::new(
            ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn skynbd_store::ObjectStore>)
                .with_clock(Arc::clone(&clock)),
        );
        match new.lease_acquire(v, "node-b", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => assert_eq!(l.epoch, 2),
            other => panic!("expected steal, got {other:?}"),
        }
        new.transact_as(v, vec![cas("snap/b", 2)], "node-b", 2)
            .await
            .unwrap();

        // The straggler's shutdown path releases. Its cache never saw
        // snap/b — an export here would erase it.
        old.lease_release(v, "node-a").await.unwrap();

        let reader = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn skynbd_store::ObjectStore>);
        assert!(
            reader.get(v, "snap/b").await.unwrap().is_some(),
            "the current owner's exported state must survive a straggler's release"
        );
        let lease = reader.lease_get(v).await.unwrap().unwrap();
        assert_eq!(
            (lease.holder.as_str(), lease.epoch),
            ("node-b", 2),
            "a non-owner release must not touch the thief's lease"
        );
    }

    /// Interposer staging the B1 race: the first `put_if` after arming
    /// first lets a thief steal the volume's lease (bumping epoch AND
    /// the object's etag), then forwards the PUT — which loses the etag
    /// race, re-reads, and fences.
    struct StealOnPut {
        inner: Arc<MemoryStore>,
        armed: Arc<AtomicBool>,
        vol: skynbd_types::VolumeId,
        thief_clock: crate::Clock,
    }

    #[async_trait::async_trait]
    impl skynbd_store::ObjectStore for StealOnPut {
        async fn put(&self, key: &str, data: bytes::Bytes) -> skynbd_store::Result<()> {
            self.inner.put(key, data).await
        }
        async fn get(
            &self,
            key: &str,
            range: Option<std::ops::Range<u64>>,
        ) -> skynbd_store::Result<bytes::Bytes> {
            self.inner.get(key, range).await
        }
        async fn exists(&self, key: &str) -> skynbd_store::Result<bool> {
            self.inner.exists(key).await
        }
        async fn list(&self, prefix: &str) -> skynbd_store::Result<Vec<String>> {
            self.inner.list(prefix).await
        }
        async fn delete(&self, key: &str) -> skynbd_store::Result<()> {
            self.inner.delete(key).await
        }
        async fn get_versioned(&self, key: &str) -> skynbd_store::Result<(bytes::Bytes, String)> {
            self.inner.get_versioned(key).await
        }
        async fn put_if(
            &self,
            key: &str,
            data: bytes::Bytes,
            pre: Precondition,
        ) -> skynbd_store::Result<String> {
            if self.armed.swap(false, Ordering::SeqCst) {
                let thief = ObjectRefStore::new(
                    Arc::clone(&self.inner) as Arc<dyn skynbd_store::ObjectStore>
                )
                .with_clock(Arc::clone(&self.thief_clock));
                match thief.lease_acquire(self.vol, "node-b", TTL).await.unwrap() {
                    LeaseOutcome::Acquired(l) => assert_eq!(l.epoch, 2, "the staged steal"),
                    other => panic!("staged steal failed: {other:?}"),
                }
            }
            self.inner.put_if(key, data, pre).await
        }
    }

    /// B1: a `transact_as` whose pre-fence passes but whose EXPORT is
    /// fenced (the lease moved in between) has already applied the ops
    /// to the cache — the cache must be invalidated so no later export
    /// replays the rejected transaction, and a re-acquiring holder
    /// re-hydrates the interim owner's writes instead of erasing them.
    #[tokio::test]
    async fn fenced_export_invalidates_the_cache_and_rehydrates() {
        let shared = Arc::new(MemoryStore::new());
        let (now, clock) = test_clock();
        let v = skynbd_types::VolumeId(ulid::Ulid::from_parts(0xB1, 1));
        let armed = Arc::new(AtomicBool::new(false));
        let thief_clock: crate::Clock = {
            // Far enough ahead that node-a's live lease looks expired to
            // the thief — the staged steal must succeed.
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst) + TTL * 6)
        };
        let s = CadencedRefStore::new(
            ObjectRefStore::new(StealOnPut {
                inner: Arc::clone(&shared),
                armed: Arc::clone(&armed),
                vol: v,
                thief_clock,
            })
            .with_clock(Arc::clone(&clock)),
        );

        // node-a owns (epoch 1) and exports a benign row.
        s.lease_acquire(v, "node-a", TTL).await.unwrap();
        s.transact_as(v, vec![cas("snap/keep", 1)], "node-a", 1)
            .await
            .unwrap();

        // Armed: this transaction's export PUT races the staged thief.
        armed.store(true, Ordering::SeqCst);
        let err = s
            .transact_as(
                v,
                vec![
                    cas("snap/rejected", 9),
                    RefOp::Pin {
                        name: "snap/rejected".into(),
                        holder: "zombie".into(),
                    },
                ],
                "node-a",
                1,
            )
            .await
            .unwrap_err();
        match err {
            RefError::LeaseLost { holder, epoch, .. } => {
                assert_eq!((holder.as_str(), epoch), ("node-b", 2))
            }
            other => panic!("expected LeaseLost from the fenced export, got {other:?}"),
        }

        // The interim owner publishes under its epoch.
        let direct = ObjectRefStore::new(Arc::clone(&shared) as Arc<dyn skynbd_store::ObjectStore>);
        direct
            .transact_as(v, vec![cas("snap/interim", 3)], "node-b", 2)
            .await
            .unwrap();

        // node-b expires; node-a legitimately re-acquires (epoch 3).
        now.fetch_add(TTL * 12, Ordering::SeqCst);
        match s.lease_acquire(v, "node-a", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => assert_eq!(l.epoch, 3),
            other => panic!("expected re-acquire, got {other:?}"),
        }

        // The invalidated cache re-hydrated from the authority: the
        // rejected ops are gone, the interim owner's write is visible.
        assert_eq!(
            s.get(v, "snap/rejected").await.unwrap(),
            None,
            "the fenced transaction must not survive in the cache"
        );
        assert!(s.pins(v, "snap/rejected").await.unwrap().is_empty());
        assert!(s.get(v, "snap/interim").await.unwrap().is_some());

        // And the next export replays neither the rejected ops nor a
        // pre-steal view of the volume.
        s.flush_volume_as(v, "node-a", 3).await.unwrap();
        assert_eq!(direct.get(v, "snap/rejected").await.unwrap(), None);
        assert!(direct.get(v, "snap/interim").await.unwrap().is_some());
        assert!(direct.get(v, "snap/keep").await.unwrap().is_some());
    }
}

//! `ObjectRefStore`: the refs store *in* the object store (design D-M1,
//! D-M2) — the piece that makes multi-node correct, because every server
//! that can reach the bucket sees the same head.
//!
//! One `SREF` object per volume holds all of that volume's refs, pins and
//! meta rows; every mutation is read → apply → conditional PUT, so the
//! single write is the D13/I8 multi-op transaction and a concurrent
//! writer *loses the PUT* rather than diverging. An etag race (the PUT
//! lost but the caller's CAS expectations may still hold against the new
//! state) re-reads and re-applies; a real conflict surfaces exactly as it
//! does from the in-memory store.
//!
//! `watch()` is a local broadcast, as D-M1 argues it can be: under
//! single-writer-per-volume the owner makes every change, so its own
//! committed CASes are the change stream. A second process's writes are
//! not observed here — exclusivity is the D-M3 lease's job, not this
//! type's.
//!
//! Semantics are defined by `exercise_ref_store`, the same conformance
//! the in-memory store passes; the two-instance race tests in
//! `tests/object_refs.rs` are what this store adds over it.

use std::collections::{BTreeMap, BTreeSet};

use skynbd_format::{Codec, RefsObject};
use skynbd_store::{ObjectStore, Precondition, StoreError};
use skynbd_types::VolumeId;

use crate::{
    lease, CasOutcome, Clock, Lease, LeaseConfig, LeaseOutcome, RefError, RefEvent, RefOp,
    RefStore, RefValue, Result, TxnOutcome, WATCH_CAPACITY,
};

/// The volume's refs object (design §5.1). Lives beside `sb/` and `base`
/// at the volume root, which GC never sweeps (it collects only the
/// content prefixes).
pub fn refs_key(vol: VolumeId) -> String {
    format!("skynbd/v1/{vol}/refs")
}

/// Retries for pure etag races — writers colliding on the *object*, not
/// on a ref. Each retry re-reads and re-evaluates, so a genuine CAS
/// conflict exits the loop as `Conflict` on its first re-evaluation;
/// only live interleaving consumes attempts.
const RACE_RETRIES: usize = 16;

/// A volume's rows in transferable form (hydration and export, D-M4).
#[derive(Debug, Default, Clone)]
pub(crate) struct VolumeRows {
    pub(crate) refs: Vec<(String, RefValue)>,
    pub(crate) pins: Vec<(String, String)>,
    pub(crate) meta: Vec<(String, Vec<u8>)>,
}

/// A volume's decoded state plus the version that guards its next write.
#[derive(Debug, Default, Clone)]
struct VolumeState {
    refs: BTreeMap<String, RefValue>,
    pins: BTreeMap<String, BTreeSet<String>>,
    meta: BTreeMap<String, Vec<u8>>,
    owner: Option<(String, u64, u64)>,
}

impl VolumeState {
    fn decode(bytes: &[u8]) -> Result<Self> {
        let obj = RefsObject::decode(bytes).map_err(|e| RefError::Backend(e.to_string()))?;
        let mut state = Self {
            owner: obj.owner,
            ..Self::default()
        };
        for (name, commit, generation) in obj.refs {
            state.refs.insert(name, RefValue { commit, generation });
        }
        for (name, holder) in obj.pins {
            state.pins.entry(name).or_default().insert(holder);
        }
        state.meta = obj.meta.into_iter().collect();
        Ok(state)
    }

    fn encode(&self) -> Result<bytes::Bytes> {
        let obj = RefsObject {
            refs: self
                .refs
                .iter()
                .map(|(n, v)| (n.clone(), v.commit, v.generation))
                .collect(),
            pins: self
                .pins
                .iter()
                .flat_map(|(n, hs)| hs.iter().map(move |h| (n.clone(), h.clone())))
                .collect(),
            meta: self
                .meta
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            owner: self.owner.clone(),
        };
        Ok(obj
            .encode()
            .map_err(|e| RefError::Backend(e.to_string()))?
            .into())
    }

    /// The transaction semantics of [`crate::MemoryRefStore::transact`],
    /// verbatim: every CAS validates against the pre-transaction state,
    /// then all ops apply. Returns the events a commit should broadcast.
    fn apply(
        &mut self,
        vol: VolumeId,
        ops: &[RefOp],
    ) -> std::result::Result<Vec<RefEvent>, TxnOutcome> {
        for (i, op) in ops.iter().enumerate() {
            match op {
                RefOp::CasRef { name, expected, .. } => {
                    let current = self.refs.get(name).copied();
                    if current != *expected {
                        return Err(TxnOutcome::Conflict {
                            op_index: i,
                            actual: current,
                        });
                    }
                }
                RefOp::MetaCas { key, expected, .. } if self.meta.get(key) != expected.as_ref() => {
                    return Err(TxnOutcome::Conflict {
                        op_index: i,
                        actual: None,
                    });
                }
                _ => {}
            }
        }
        let mut events = Vec::new();
        for op in ops {
            match op {
                RefOp::CasRef {
                    name,
                    expected,
                    new_commit,
                } => {
                    let stored = match new_commit {
                        Some(commit) => {
                            let generation = expected.map(|e| e.generation + 1).unwrap_or(0);
                            let v = RefValue {
                                commit: *commit,
                                generation,
                            };
                            self.refs.insert(name.clone(), v);
                            Some(v)
                        }
                        None => {
                            self.refs.remove(name);
                            None
                        }
                    };
                    events.push(RefEvent {
                        vol,
                        name: name.clone(),
                        value: stored,
                    });
                }
                RefOp::Pin { name, holder } => {
                    self.pins
                        .entry(name.clone())
                        .or_default()
                        .insert(holder.clone());
                }
                RefOp::Unpin { name, holder } => {
                    if let Some(set) = self.pins.get_mut(name) {
                        set.remove(holder);
                        if set.is_empty() {
                            self.pins.remove(name);
                        }
                    }
                }
                RefOp::MetaPut { key, value } | RefOp::MetaCas { key, value, .. } => {
                    self.meta.insert(key.clone(), value.clone());
                }
                RefOp::MetaDelete { key } => {
                    self.meta.remove(key);
                }
            }
        }
        Ok(events)
    }
}

pub struct ObjectRefStore<S> {
    store: S,
    events: tokio::sync::broadcast::Sender<RefEvent>,
    clock: Clock,
    /// The identity this store writes under. `Some` makes every mutation
    /// fenced (design D-M3): a volume owned by anyone else refuses the
    /// write with `LeaseLost` — checked against the freshly-read state on
    /// every attempt, which is exactly the moment a thief's etag bump
    /// forces a re-read. `None` (default) trusts the caller: single-node
    /// mode and tests.
    owner: Option<LeaseConfig>,
}

impl<S: ObjectStore> ObjectRefStore<S> {
    pub fn new(store: S) -> Self {
        Self {
            store,
            events: tokio::sync::broadcast::channel(WATCH_CAPACITY).0,
            clock: crate::system_clock(),
            owner: None,
        }
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// Fence all writes to `cfg.holder`'s leases, and renew for free: any
    /// successful write by the recorded owner pushes its expiry out by
    /// `cfg.ttl_ms` — the object is being rewritten anyway (D-M3's
    /// "every commit renews the lease").
    pub fn with_owner(mut self, cfg: LeaseConfig) -> Self {
        self.owner = Some(cfg);
        self
    }

    /// Startup capability check: a backend without conditional writes
    /// cannot host refs, and that must fail at boot with a clear message
    /// rather than mid-transaction.
    pub async fn probe(store: &S) -> Result<()> {
        let key = "skynbd/v1/.cond-probe";
        match store
            .put_if(
                key,
                bytes::Bytes::from_static(b"probe"),
                Precondition::IfNoneMatch,
            )
            .await
        {
            Ok(_) | Err(StoreError::PreconditionFailed(_)) => {
                let _ = store.delete(key).await;
                Ok(())
            }
            Err(StoreError::Unsupported(what)) => Err(RefError::Backend(format!(
                "object store lacks conditional writes ({what}); \
                 refs cannot live there — use redb"
            ))),
            Err(e) => Err(RefError::Backend(format!("conditional probe: {e}"))),
        }
    }

    /// The volume's full row set, verbatim — the cadence tier's
    /// hydration read (design D-M4). Generations and pin holders carry
    /// over exactly, so a cache seeded from this is indistinguishable
    /// from the authority it mirrors.
    pub(crate) async fn volume_rows(&self, vol: VolumeId) -> Result<VolumeRows> {
        let state = self.load(vol).await?.0;
        Ok(VolumeRows {
            refs: state.refs.into_iter().collect(),
            pins: state
                .pins
                .into_iter()
                .flat_map(|(n, hs)| hs.into_iter().map(move |h| (n.clone(), h)))
                .collect(),
            meta: state.meta.into_iter().collect(),
        })
    }

    /// Replace the volume's refs/pins/meta wholesale — the cadence
    /// tier's export write. Fenced like any other mutation (a foreign
    /// owner is `LeaseLost`), renews the lease for free, and preserves
    /// the freshly-read owner row rather than what the cache believes.
    /// Emits no events: the exporting cache already broadcast them.
    pub(crate) async fn replace_volume(&self, vol: VolumeId, rows: VolumeRows) -> Result<()> {
        self.update_inner(vol, true, replace_rows(rows)).await
    }

    /// [`Self::replace_volume`] under an explicit principal (task 15h):
    /// the export write of a volume whose lease a *node* holds — fenced
    /// against exactly that `(holder, epoch)` instead of the ambient
    /// `cfg.holder`.
    pub(crate) async fn replace_volume_as(
        &self,
        vol: VolumeId,
        rows: VolumeRows,
        holder: &str,
        epoch: u64,
    ) -> Result<()> {
        self.update_as(vol, holder, epoch, replace_rows(rows)).await
    }

    /// Current state + the precondition for replacing it. An absent
    /// object is an empty volume created with `IfNoneMatch`.
    async fn load(&self, vol: VolumeId) -> Result<(VolumeState, Precondition)> {
        match self.store.get_versioned(&refs_key(vol)).await {
            Ok((bytes, etag)) => Ok((VolumeState::decode(&bytes)?, Precondition::IfMatch(etag))),
            Err(StoreError::NotFound(_)) => Ok((VolumeState::default(), Precondition::IfNoneMatch)),
            Err(e) => Err(RefError::Backend(e.to_string())),
        }
    }

    /// Read → mutate → conditional write, retrying pure etag races.
    /// `mutate` runs against fresh state on every attempt, so its own
    /// conflict answers stay correct under interleaving.
    async fn update<T>(
        &self,
        vol: VolumeId,
        mutate: impl FnMut(&mut VolumeState) -> std::result::Result<(T, Vec<RefEvent>), T>,
    ) -> Result<T> {
        self.update_inner(vol, true, mutate).await
    }

    /// `fenced: false` is for the lease operations themselves — stealing
    /// is precisely a write by a non-owner, and [`lease::acquire`] is its
    /// own authority on when that is legal.
    async fn update_inner<T>(
        &self,
        vol: VolumeId,
        fenced: bool,
        mut mutate: impl FnMut(&mut VolumeState) -> std::result::Result<(T, Vec<RefEvent>), T>,
    ) -> Result<T> {
        for _ in 0..RACE_RETRIES {
            let (mut state, pre) = self.load(vol).await?;
            // The D-M3 fence. A thief's steal bumped the object's etag, so
            // our next put_if fails, we land back here with fresh state,
            // and see the foreign owner. This is the moment the design
            // describes as "its next commit's CAS fails".
            if fenced {
                if let (Some(cfg), Some((h, epoch, _))) = (&self.owner, &state.owner) {
                    if h != &cfg.holder {
                        return Err(RefError::LeaseLost {
                            vol,
                            holder: h.clone(),
                            epoch: *epoch,
                        });
                    }
                }
            }
            let (out, events) = match mutate(&mut state) {
                Ok(v) => v,
                // The mutation itself says no (CAS conflict): nothing to
                // write, the answer is final for the state we read.
                Err(out) => return Ok(out),
            };
            // Renew for free: this write is the owner's heartbeat.
            if fenced {
                if let (Some(cfg), Some((h, _, expires))) = (&self.owner, &mut state.owner) {
                    if h == &cfg.holder {
                        *expires = (self.clock)().saturating_add(cfg.ttl_ms);
                    }
                }
            }
            match self
                .store
                .put_if(&refs_key(vol), state.encode()?, pre)
                .await
            {
                Ok(_) => {
                    for ev in events {
                        let _ = self.events.send(ev);
                    }
                    return Ok(out);
                }
                Err(StoreError::PreconditionFailed(_)) => continue,
                Err(e) => return Err(RefError::Backend(e.to_string())),
            }
        }
        Err(RefError::Backend(format!(
            "refs object for {vol} contended beyond {RACE_RETRIES} retries"
        )))
    }

    /// [`Self::update_inner`] fenced under an **explicit** `(holder,
    /// epoch)` principal (Invariant F, task 15h) instead of the ambient
    /// `cfg.holder`: unless the freshly-read owner row matches exactly,
    /// the mutation fails with `LeaseLost` carrying the actual owner.
    /// The explicit fence supersedes the ambient one — a server
    /// executing a node's transaction acts under the node's identity.
    /// A matching write still renews the lease for free when this store
    /// knows a TTL (D-M3: every commit is the owner's heartbeat).
    async fn update_as<T>(
        &self,
        vol: VolumeId,
        holder: &str,
        epoch: u64,
        mut mutate: impl FnMut(&mut VolumeState) -> std::result::Result<(T, Vec<RefEvent>), T>,
    ) -> Result<T> {
        for _ in 0..RACE_RETRIES {
            let (mut state, pre) = self.load(vol).await?;
            let current = state.owner.as_ref().map(tuple_to_lease);
            lease::fence(current.as_ref(), vol, holder, epoch)?;
            let (out, events) = match mutate(&mut state) {
                Ok(v) => v,
                Err(out) => return Ok(out),
            };
            if let (Some(cfg), Some((_, _, expires))) = (&self.owner, &mut state.owner) {
                *expires = (self.clock)().saturating_add(cfg.ttl_ms);
            }
            match self
                .store
                .put_if(&refs_key(vol), state.encode()?, pre)
                .await
            {
                Ok(_) => {
                    for ev in events {
                        let _ = self.events.send(ev);
                    }
                    return Ok(out);
                }
                Err(StoreError::PreconditionFailed(_)) => continue,
                Err(e) => return Err(RefError::Backend(e.to_string())),
            }
        }
        Err(RefError::Backend(format!(
            "refs object for {vol} contended beyond {RACE_RETRIES} retries"
        )))
    }
}

/// The `replace_volume` mutation, shared by the ambient and explicit
/// export paths.
fn replace_rows(
    rows: VolumeRows,
) -> impl FnMut(&mut VolumeState) -> std::result::Result<((), Vec<RefEvent>), ()> {
    move |state| {
        state.refs = rows.refs.iter().cloned().collect();
        state.pins.clear();
        for (name, holder) in &rows.pins {
            state
                .pins
                .entry(name.clone())
                .or_default()
                .insert(holder.clone());
        }
        state.meta = rows.meta.iter().cloned().collect();
        Ok(((), vec![]))
    }
}

#[async_trait::async_trait]
impl<S: ObjectStore> RefStore for ObjectRefStore<S> {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>> {
        Ok(self.load(vol).await?.0.refs.get(name).copied())
    }

    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<skynbd_types::CommitHash>,
    ) -> Result<CasOutcome> {
        let op = RefOp::CasRef {
            name: name.to_string(),
            expected,
            new_commit,
        };
        self.update(vol, |state| {
            let ops = std::slice::from_ref(&op);
            match state.apply(vol, ops) {
                Ok(events) => {
                    let stored = state.refs.get(name).copied();
                    Ok((CasOutcome::Committed(stored), events))
                }
                Err(TxnOutcome::Conflict { actual, .. }) => Err(CasOutcome::Conflict(actual)),
                Err(TxnOutcome::Committed) => unreachable!("apply never returns Committed"),
            }
        })
        .await
    }

    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>> {
        Ok(self.load(vol).await?.0.refs.into_iter().collect())
    }

    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        self.events.subscribe()
    }

    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> Result<TxnOutcome> {
        self.update(vol, |state| match state.apply(vol, &ops) {
            Ok(events) => Ok((TxnOutcome::Committed, events)),
            Err(conflict) => Err(conflict),
        })
        .await
    }

    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> Result<TxnOutcome> {
        self.update_as(vol, holder, epoch, |state| match state.apply(vol, &ops) {
            Ok(events) => Ok((TxnOutcome::Committed, events)),
            Err(conflict) => Err(conflict),
        })
        .await
    }

    async fn pins(&self, vol: VolumeId, name: &str) -> Result<Vec<String>> {
        Ok(self
            .load(vol)
            .await?
            .0
            .pins
            .get(name)
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default())
    }

    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.load(vol).await?.0.meta.get(key).cloned())
    }

    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(self
            .load(vol)
            .await?
            .0
            .meta
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect())
    }

    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<LeaseOutcome> {
        let holder = holder.to_string();
        let clock = std::sync::Arc::clone(&self.clock);
        self.update_inner(vol, false, move |state| {
            let current = state.owner.as_ref().map(tuple_to_lease);
            let (write, outcome) = lease::acquire(current.as_ref(), &holder, ttl_ms, clock());
            match write {
                Some(l) => {
                    state.owner = Some((l.holder.clone(), l.epoch, l.expires_at_ms));
                    Ok((outcome, vec![]))
                }
                // Held: nothing to write, the answer is final.
                None => Err(outcome),
            }
        })
        .await
    }

    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()> {
        let holder = holder.to_string();
        self.update_inner(vol, false, move |state| {
            let current = state.owner.as_ref().map(tuple_to_lease);
            match lease::release(current.as_ref(), &holder) {
                Some(l) => {
                    state.owner = Some((l.holder.clone(), l.epoch, l.expires_at_ms));
                    Ok(((), vec![]))
                }
                None => Err(()),
            }
        })
        .await
    }

    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>> {
        Ok(self.load(vol).await?.0.owner.as_ref().map(tuple_to_lease))
    }
}

fn tuple_to_lease((holder, epoch, expires_at_ms): &(String, u64, u64)) -> Lease {
    Lease {
        holder: holder.clone(),
        epoch: *epoch,
        expires_at_ms: *expires_at_ms,
    }
}

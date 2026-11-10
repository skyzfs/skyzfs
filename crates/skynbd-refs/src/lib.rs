//! `RefStore`: the mutable-pointer seam (design §6.1).
//!
//! Everything durable lives in S3; refs are the only mutable state. The
//! trait deliberately exposes *only* operations etcd can also provide, so
//! the MVP cannot depend on stronger local semantics (design D6). The
//! transactional `transact` primitive exists for publish/prepare atomicity
//! with pins (design D13, I8).
//!
//! Implementations: `MemoryRefStore` (now), `RedbRefStore` — TODO(M1),
//! `EtcdRefStore` — post-MVP.

use std::collections::BTreeMap;
use std::sync::Arc;

use skynbd_types::{CommitHash, VolumeId};
use tokio::sync::RwLock;

mod acting;
mod cadence;
mod object_store;
mod redb_store;
pub use acting::ActingRefStore;
pub use cadence::CadencedRefStore;
pub use object_store::{refs_key, ObjectRefStore};
pub use redb_store::RedbRefStore;

/// A ref value: commit pointer + CAS generation (design §6.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RefValue {
    pub commit: CommitHash,
    pub generation: u64,
}

/// A volume's ownership lease (design D-M3). CAS gives safety; this gives
/// exclusivity — both live in the same refs row so a commit renews the
/// lease for free.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lease {
    pub holder: String,
    /// The fence epoch: bumped on every change of holder — steal or
    /// re-acquire after release — never on renewal, and monotonic across
    /// release. The epoch, not the clock, is authoritative; task 6 stamps
    /// it into commits so a fenced writer's history is filterable.
    pub epoch: u64,
    /// Unix milliseconds. Only how long a taker waits before stealing —
    /// never a permission to write (that is the epoch's job).
    pub expires_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LeaseOutcome {
    Acquired(Lease),
    /// Another holder's unexpired lease. Wait, or don't.
    Held(Lease),
}

/// Identity + TTL a store or provisioner acts under.
#[derive(Debug, Clone)]
pub struct LeaseConfig {
    pub holder: String,
    pub ttl_ms: u64,
}

/// Milliseconds since the Unix epoch, injectable so expiry is testable
/// without sleeping. Only ever compared against `expires_at_ms` written
/// through the same store family — cross-node skew is why the epoch, not
/// this, is authoritative.
pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

pub fn system_clock() -> Clock {
    Arc::new(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock before Unix epoch")
            .as_millis() as u64
    })
}

/// The one lease state machine, shared by every store so the semantics
/// cannot drift between backends.
pub(crate) mod lease {
    use super::{Lease, LeaseOutcome, RefError};
    use skynbd_types::VolumeId;

    /// The Invariant F fence (task 15h), shared by every store's
    /// `transact_as`: the transaction proceeds only under the exact
    /// `(holder, epoch)` the lease row records. Expiry is deliberately
    /// NOT consulted — the epoch, not the clock, is authoritative, and
    /// an expired-but-unstolen lease still belongs to its holder.
    pub(crate) fn fence(
        current: Option<&Lease>,
        vol: VolumeId,
        holder: &str,
        epoch: u64,
    ) -> Result<(), RefError> {
        match current {
            Some(l) if l.holder == holder && l.epoch == epoch => Ok(()),
            Some(l) => Err(RefError::LeaseLost {
                vol,
                holder: l.holder.clone(),
                epoch: l.epoch,
            }),
            None => Err(RefError::LeaseLost {
                vol,
                holder: String::new(),
                epoch: 0,
            }),
        }
    }

    /// Returns the row to write (`None` = write nothing) and the outcome.
    pub(crate) fn acquire(
        current: Option<&Lease>,
        holder: &str,
        ttl_ms: u64,
        now_ms: u64,
    ) -> (Option<Lease>, LeaseOutcome) {
        let fresh = |epoch| Lease {
            holder: holder.to_string(),
            epoch,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
        };
        let granted = |l: Lease| (Some(l.clone()), LeaseOutcome::Acquired(l));
        match current {
            None => granted(fresh(1)),
            // Renewal: same epoch — the holder never fences itself.
            Some(cur) if cur.holder == holder => granted(fresh(cur.epoch)),
            // Steal: expired (release zeroes expiry, so released rows
            // land here too) — epoch bumps, staying monotonic forever.
            Some(cur) if cur.expires_at_ms <= now_ms => granted(fresh(cur.epoch + 1)),
            Some(cur) => (None, LeaseOutcome::Held(cur.clone())),
        }
    }

    /// Release keeps `(holder, epoch)` and zeroes the expiry: a released
    /// lease is an expired one, so the next taker still bumps the epoch.
    /// Dropping the row instead would restart epochs at 1 and break the
    /// fence filter. Releasing a lease someone else holds is a no-op —
    /// the thief's lease must survive a straggler's shutdown path.
    pub(crate) fn release(current: Option<&Lease>, holder: &str) -> Option<Lease> {
        match current {
            Some(cur) if cur.holder == holder => Some(Lease {
                expires_at_ms: 0,
                ..cur.clone()
            }),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasOutcome {
    /// The swap was applied; contains the new stored value (None = deleted).
    Committed(Option<RefValue>),
    /// Expectation mismatch; contains the actual current value.
    Conflict(Option<RefValue>),
}

#[derive(Debug, thiserror::Error)]
pub enum RefError {
    #[error("backend error: {0}")]
    Backend(String),
    /// A fenced write: this store's holder no longer owns the volume
    /// (design D-M3). The engine maps this to `Conflict` — drop the
    /// overlay, `NBD_ESHUTDOWN` — the I2 self-fence.
    #[error("lease lost: {vol} is owned by {holder:?} (epoch {epoch})")]
    LeaseLost {
        vol: VolumeId,
        holder: String,
        epoch: u64,
    },
}

pub type Result<T> = std::result::Result<T, RefError>;

/// Ref names are plain strings: `head`, `rpo`, `snap/<name>`, `prepared/<id>`.
/// Hidden refs (`prepared/*`) are excluded from the export registry by
/// prefix — TODO(M2) in the registry, not here.
#[async_trait::async_trait]
pub trait RefStore: Send + Sync {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>>;

    /// Compare-and-swap. `expected: None` = create-if-absent;
    /// `new: None` = delete-if-matches. The generation of a stored `new`
    /// value is assigned by the store (expected.generation + 1, or 0).
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> Result<CasOutcome>;

    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>>;

    /// Change stream feeding the export registry (design §3.1, §7.1).
    /// Emits one event per *committed* CAS. Best-effort broadcast: slow
    /// subscribers may observe `Lagged` and should resynchronize via
    /// `list` (the registry sync task does exactly that).
    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent>;

    /// Atomic multi-op transaction (design D13): CAS refs, pin/unpin, and
    /// meta rows commit together or not at all — the publish-implies-pin
    /// primitive (I8). A `Conflict` reports the first failing CAS and
    /// leaves everything untouched.
    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> Result<TxnOutcome>;

    /// [`RefStore::transact`], fenced under an explicit principal
    /// (Invariant F, task 15h): atomically with the transaction, the
    /// volume's lease row is read and — unless `lease.holder == holder`
    /// **and** `lease.epoch == epoch` — the whole transaction fails with
    /// [`RefError::LeaseLost`] carrying the actual holder and epoch (an
    /// absent lease row reports holder `""`, epoch 0) and applies
    /// nothing. An *expired* but matching lease passes: the holder never
    /// fences itself, and every commit renews the lease for free. This
    /// explicit fence supersedes any ambient (store-configured) holder
    /// fence: a server executing a node's transaction acts under the
    /// node's identity, not its own.
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> Result<TxnOutcome>;

    /// Current pin holders of a ref (design §9A.4). Refcounted by row:
    /// deletable only when empty.
    async fn pins(&self, vol: VolumeId, name: &str) -> Result<Vec<String>>;

    /// Small mutable metadata rows (the design §6.1 `prepared` table).
    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>>;
    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>>;

    /// Acquire, renew (same holder, epoch kept) or steal (expired holder,
    /// epoch bumped) the volume's lease (design D-M3). One method for all
    /// three because the semantics are one state machine: [`lease::acquire`].
    async fn lease_acquire(&self, vol: VolumeId, holder: &str, ttl_ms: u64)
        -> Result<LeaseOutcome>;

    /// Zero the expiry, keeping `(holder, epoch)` so epochs stay
    /// monotonic. No-op unless `holder` holds the lease.
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()>;

    /// The stored row, verbatim — a returned lease may be expired or
    /// released; callers judge against their own clock and identity.
    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>>;

    /// Make the volume's state durable *now*, wherever this store's
    /// authority lives. A no-op everywhere except the cadenced store
    /// (design D-M4), where it exports the cached rows to the refs
    /// object — callers invoke it at the handoff points (serve, release)
    /// that keep the recovery gap single-epoch.
    async fn flush_volume(&self, vol: VolumeId) -> Result<()> {
        let _ = vol;
        Ok(())
    }

    /// [`RefStore::flush_volume`] under an explicit principal (task 15h):
    /// the cadenced store's export write is fenced like any other
    /// mutation, and a volume served under a *node's* lease must export
    /// under that identity, not the server's ambient one. Everywhere
    /// flushing is a no-op the principal is irrelevant — the default
    /// delegates.
    async fn flush_volume_as(&self, vol: VolumeId, holder: &str, epoch: u64) -> Result<()> {
        let _ = (holder, epoch);
        self.flush_volume(vol).await
    }
}

/// One operation inside a [`RefStore::transact`] transaction.
#[derive(Debug, Clone)]
pub enum RefOp {
    CasRef {
        name: String,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    },
    Pin {
        name: String,
        holder: String,
    },
    Unpin {
        name: String,
        holder: String,
    },
    MetaPut {
        key: String,
        value: Vec<u8>,
    },
    /// Compare-and-swap on a meta row (design D-M15 Invariant E: the
    /// monotone origin pointer). `expected: None` = create-if-absent.
    /// A mismatch aborts the whole transaction with `Conflict`; its
    /// `actual` is always `None` for this op (`RefValue` cannot carry
    /// meta bytes) — callers re-read via `meta_get`, and `op_index`
    /// names the failing op.
    MetaCas {
        key: String,
        expected: Option<Vec<u8>>,
        value: Vec<u8>,
    },
    MetaDelete {
        key: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxnOutcome {
    Committed,
    /// First failing CAS: its op index and the actual current value.
    Conflict {
        op_index: usize,
        actual: Option<RefValue>,
    },
}

/// A committed ref change: `value: None` means the ref was deleted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefEvent {
    pub vol: VolumeId,
    pub name: String,
    pub value: Option<RefValue>,
}

/// Delegation through `Arc`, enabling type-erased `Arc<dyn RefStore>`.
#[async_trait::async_trait]
impl<T: RefStore + ?Sized> RefStore for Arc<T> {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>> {
        (**self).get(vol, name).await
    }
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> Result<CasOutcome> {
        (**self)
            .compare_and_swap(vol, name, expected, new_commit)
            .await
    }
    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>> {
        (**self).list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        (**self).watch()
    }
    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> Result<TxnOutcome> {
        (**self).transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> Result<TxnOutcome> {
        (**self).transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(&self, vol: VolumeId, name: &str) -> Result<Vec<String>> {
        (**self).pins(vol, name).await
    }
    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>> {
        (**self).meta_get(vol, key).await
    }
    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        (**self).meta_list(vol, prefix).await
    }
    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<LeaseOutcome> {
        (**self).lease_acquire(vol, holder, ttl_ms).await
    }
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()> {
        (**self).lease_release(vol, holder).await
    }
    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>> {
        (**self).lease_get(vol).await
    }
    async fn flush_volume(&self, vol: VolumeId) -> Result<()> {
        (**self).flush_volume(vol).await
    }
    async fn flush_volume_as(&self, vol: VolumeId, holder: &str, epoch: u64) -> Result<()> {
        (**self).flush_volume_as(vol, holder, epoch).await
    }
}

/// Broadcast capacity: registry sync resyncs via `list` on lag, so a
/// bounded channel is safe.
pub(crate) const WATCH_CAPACITY: usize = 1024;

type Shared<V> = Arc<RwLock<BTreeMap<(VolumeId, String), V>>>;

/// In-memory `RefStore` for tests and fast local development.
#[derive(Clone)]
pub struct MemoryRefStore {
    inner: Shared<RefValue>,
    pins: Shared<std::collections::BTreeSet<String>>,
    meta: Shared<Vec<u8>>,
    leases: Arc<RwLock<BTreeMap<VolumeId, Lease>>>,
    clock: Clock,
    events: tokio::sync::broadcast::Sender<RefEvent>,
}

impl Default for MemoryRefStore {
    fn default() -> Self {
        Self {
            inner: Arc::default(),
            pins: Arc::default(),
            meta: Arc::default(),
            leases: Arc::default(),
            clock: system_clock(),
            events: tokio::sync::broadcast::channel(WATCH_CAPACITY).0,
        }
    }
}

impl MemoryRefStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// [`RefStore::transact`] / [`RefStore::transact_as`]: one big lock
    /// over all tables = atomicity; `fence` (the lease-row check) is read
    /// under the same locks, so the validation and the transaction are
    /// one atomic step.
    async fn transact_fenced(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        fence: Option<(&str, u64)>,
    ) -> Result<TxnOutcome> {
        let mut refs = self.inner.write().await;
        let mut pins = self.pins.write().await;
        let mut meta = self.meta.write().await;
        // The guard is held across the apply below: releasing it after
        // the check would let a lease steal land between validation and
        // application, making the fence advisory.
        let _leases = match fence {
            Some((holder, epoch)) => {
                let leases = self.leases.read().await;
                lease::fence(leases.get(&vol), vol, holder, epoch)?;
                Some(leases)
            }
            None => None,
        };

        // Validate every CAS first (all-or-nothing).
        for (i, op) in ops.iter().enumerate() {
            match op {
                RefOp::CasRef { name, expected, .. } => {
                    let current = refs.get(&(vol, name.clone())).copied();
                    if current != *expected {
                        return Ok(TxnOutcome::Conflict {
                            op_index: i,
                            actual: current,
                        });
                    }
                }
                RefOp::MetaCas { key, expected, .. }
                    if meta.get(&(vol, key.clone())) != expected.as_ref() =>
                {
                    return Ok(TxnOutcome::Conflict {
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
                    let key = (vol, name.clone());
                    let stored = match new_commit {
                        Some(commit) => {
                            let generation = expected.map(|e| e.generation + 1).unwrap_or(0);
                            let v = RefValue { commit, generation };
                            refs.insert(key, v);
                            Some(v)
                        }
                        None => {
                            refs.remove(&key);
                            None
                        }
                    };
                    events.push(RefEvent {
                        vol,
                        name,
                        value: stored,
                    });
                }
                RefOp::Pin { name, holder } => {
                    pins.entry((vol, name)).or_default().insert(holder);
                }
                RefOp::Unpin { name, holder } => {
                    if let Some(set) = pins.get_mut(&(vol, name.clone())) {
                        set.remove(&holder);
                        if set.is_empty() {
                            pins.remove(&(vol, name));
                        }
                    }
                }
                RefOp::MetaPut { key, value } | RefOp::MetaCas { key, value, .. } => {
                    meta.insert((vol, key), value);
                }
                RefOp::MetaDelete { key } => {
                    meta.remove(&(vol, key));
                }
            }
        }
        drop((refs, pins, meta));
        for e in events {
            let _ = self.events.send(e);
        }
        Ok(TxnOutcome::Committed)
    }
}

#[async_trait::async_trait]
impl RefStore for MemoryRefStore {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>> {
        Ok(self
            .inner
            .read()
            .await
            .get(&(vol, name.to_string()))
            .copied())
    }

    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> Result<CasOutcome> {
        let key = (vol, name.to_string());
        let mut map = self.inner.write().await;
        let current = map.get(&key).copied();
        if current != expected {
            return Ok(CasOutcome::Conflict(current));
        }
        let stored = match new_commit {
            Some(commit) => {
                let generation = expected.map(|e| e.generation + 1).unwrap_or(0);
                let v = RefValue { commit, generation };
                map.insert(key, v);
                Some(v)
            }
            None => {
                map.remove(&key);
                None
            }
        };
        let _ = self.events.send(RefEvent {
            vol,
            name: name.to_string(),
            value: stored,
        });
        Ok(CasOutcome::Committed(stored))
    }

    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>> {
        Ok(self
            .inner
            .read()
            .await
            .iter()
            .filter(|((v, _), _)| *v == vol)
            .map(|((_, n), val)| (n.clone(), *val))
            .collect())
    }

    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        self.events.subscribe()
    }

    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> Result<TxnOutcome> {
        self.transact_fenced(vol, ops, None).await
    }

    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> Result<TxnOutcome> {
        self.transact_fenced(vol, ops, Some((holder, epoch))).await
    }

    async fn pins(&self, vol: VolumeId, name: &str) -> Result<Vec<String>> {
        Ok(self
            .pins
            .read()
            .await
            .get(&(vol, name.to_string()))
            .map(|s| s.iter().cloned().collect())
            .unwrap_or_default())
    }

    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.meta.read().await.get(&(vol, key.to_string())).cloned())
    }

    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        Ok(self
            .meta
            .read()
            .await
            .iter()
            .filter(|((v, k), _)| *v == vol && k.starts_with(prefix))
            .map(|((_, k), val)| (k.clone(), val.clone()))
            .collect())
    }

    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<LeaseOutcome> {
        let now = (self.clock)();
        let mut leases = self.leases.write().await;
        let (write, outcome) = lease::acquire(leases.get(&vol), holder, ttl_ms, now);
        if let Some(l) = write {
            leases.insert(vol, l);
        }
        Ok(outcome)
    }

    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()> {
        let mut leases = self.leases.write().await;
        if let Some(l) = lease::release(leases.get(&vol), holder) {
            leases.insert(vol, l);
        }
        Ok(())
    }

    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>> {
        Ok(self.leases.read().await.get(&vol).cloned())
    }
}

/// Backend-agnostic conformance suite: every `RefStore` implementation
/// must pass this (memory + redb now, etcd later). Panics on violation.
/// Each behavior maps to invariant I2 (design §6.2).
pub mod conformance {
    use super::*;

    pub async fn exercise_ref_store(s: &dyn RefStore) {
        let v1 = VolumeId(ulid::Ulid::from_parts(0xC0, 1));
        let v2 = VolumeId(ulid::Ulid::from_parts(0xC0, 2));

        // create-if-absent, generation 0
        let g0 = match s
            .compare_and_swap(v1, "head", None, Some([1u8; 32]))
            .await
            .unwrap()
        {
            CasOutcome::Committed(Some(rv)) => {
                assert_eq!(rv.generation, 0);
                rv
            }
            other => panic!("expected committed, got {other:?}"),
        };

        // advance by CAS, generation increments
        match s
            .compare_and_swap(v1, "head", Some(g0), Some([2u8; 32]))
            .await
            .unwrap()
        {
            CasOutcome::Committed(Some(rv)) => {
                assert_eq!((rv.generation, rv.commit), (1, [2u8; 32]))
            }
            other => panic!("expected committed, got {other:?}"),
        }

        // stale expectation conflicts, reports actual, mutates nothing
        let stale = RefValue {
            commit: [9u8; 32],
            generation: 41,
        };
        match s
            .compare_and_swap(v1, "head", Some(stale), Some([3u8; 32]))
            .await
            .unwrap()
        {
            CasOutcome::Conflict(Some(actual)) => assert_eq!(actual.commit, [2u8; 32]),
            other => panic!("expected conflict, got {other:?}"),
        }
        assert_eq!(s.get(v1, "head").await.unwrap().unwrap().commit, [2u8; 32]);

        // create conflicts when present
        assert!(matches!(
            s.compare_and_swap(v1, "head", None, Some([4u8; 32]))
                .await
                .unwrap(),
            CasOutcome::Conflict(Some(_))
        ));

        // delete-if-matches
        s.compare_and_swap(v1, "snap/a", None, Some([5u8; 32]))
            .await
            .unwrap();
        let cur = s.get(v1, "snap/a").await.unwrap().unwrap();
        assert!(matches!(
            s.compare_and_swap(v1, "snap/a", Some(cur), None)
                .await
                .unwrap(),
            CasOutcome::Committed(None)
        ));
        assert_eq!(s.get(v1, "snap/a").await.unwrap(), None);

        // watch: committed CASes emit ordered events; conflicts emit none
        let mut rx = s.watch();
        s.compare_and_swap(v1, "snap/w", None, Some([7u8; 32]))
            .await
            .unwrap();
        let stale2 = RefValue {
            commit: [0u8; 32],
            generation: 99,
        };
        s.compare_and_swap(v1, "snap/w", Some(stale2), Some([8u8; 32]))
            .await
            .unwrap(); // conflict — no event
        let cur = s.get(v1, "snap/w").await.unwrap().unwrap();
        s.compare_and_swap(v1, "snap/w", Some(cur), None)
            .await
            .unwrap();

        let e1 = rx.recv().await.unwrap();
        assert_eq!((e1.vol, e1.name.as_str()), (v1, "snap/w"));
        assert_eq!(e1.value.unwrap().commit, [7u8; 32]);
        let e2 = rx.recv().await.unwrap();
        assert_eq!(e2.value, None, "delete event");
        assert!(rx.try_recv().is_err(), "conflict must not emit an event");

        // transact: atomic CAS + pin + meta (I8 shape); conflict aborts all
        let out = s
            .transact(
                v1,
                vec![
                    RefOp::CasRef {
                        name: "snap/t".into(),
                        expected: None,
                        new_commit: Some([0xF0u8; 32]),
                    },
                    RefOp::Pin {
                        name: "snap/t".into(),
                        holder: "publisher".into(),
                    },
                    RefOp::MetaPut {
                        key: "prepared/x".into(),
                        value: b"m".to_vec(),
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed);
        assert_eq!(
            s.pins(v1, "snap/t").await.unwrap(),
            vec!["publisher".to_string()]
        );
        assert_eq!(s.meta_get(v1, "prepared/x").await.unwrap().unwrap(), b"m");
        assert_eq!(s.meta_list(v1, "prepared/").await.unwrap().len(), 1);

        // Conflicting txn leaves everything untouched.
        let out = s
            .transact(
                v1,
                vec![
                    RefOp::Pin {
                        name: "snap/t".into(),
                        holder: "second".into(),
                    },
                    RefOp::CasRef {
                        name: "snap/t".into(),
                        expected: None,
                        new_commit: Some([1u8; 32]),
                    },
                ],
            )
            .await
            .unwrap();
        assert!(matches!(out, TxnOutcome::Conflict { op_index: 1, .. }));
        assert_eq!(
            s.pins(v1, "snap/t").await.unwrap().len(),
            1,
            "aborted pin must not exist"
        );

        // Unpin to empty removes the row set.
        s.transact(
            v1,
            vec![RefOp::Unpin {
                name: "snap/t".into(),
                holder: "publisher".into(),
            }],
        )
        .await
        .unwrap();
        assert!(s.pins(v1, "snap/t").await.unwrap().is_empty());
        s.transact(
            v1,
            vec![
                RefOp::MetaDelete {
                    key: "prepared/x".into(),
                },
                RefOp::CasRef {
                    name: "snap/t".into(),
                    expected: s.get(v1, "snap/t").await.unwrap(),
                    new_commit: None,
                },
            ],
        )
        .await
        .unwrap();
        assert!(s.meta_get(v1, "prepared/x").await.unwrap().is_none());

        // MetaCas (D-M15 Invariant E): create-if-absent, guarded swap,
        // and a mismatch that aborts the whole transaction.
        let out = s
            .transact(
                v1,
                vec![RefOp::MetaCas {
                    key: "origin".into(),
                    expected: None,
                    value: b"snap/g0".to_vec(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed, "create-if-absent");
        assert_eq!(s.meta_get(v1, "origin").await.unwrap().unwrap(), b"snap/g0");
        // Duplicate create conflicts (row now present)…
        let out = s
            .transact(
                v1,
                vec![RefOp::MetaCas {
                    key: "origin".into(),
                    expected: None,
                    value: b"snap/g9".to_vec(),
                }],
            )
            .await
            .unwrap();
        assert!(
            matches!(
                out,
                TxnOutcome::Conflict {
                    op_index: 0,
                    actual: None
                }
            ),
            "MetaCas conflict reports op_index and no RefValue, got {out:?}"
        );
        // …and a stale expectation aborts every op in the transaction.
        let out = s
            .transact(
                v1,
                vec![
                    RefOp::Pin {
                        name: "snap/t".into(),
                        holder: "cas-aborted".into(),
                    },
                    RefOp::MetaCas {
                        key: "origin".into(),
                        expected: Some(b"snap/gX".to_vec()),
                        value: b"snap/g1".to_vec(),
                    },
                ],
            )
            .await
            .unwrap();
        assert!(matches!(out, TxnOutcome::Conflict { op_index: 1, .. }));
        assert!(
            s.pins(v1, "snap/t").await.unwrap().is_empty(),
            "aborted MetaCas txn must apply nothing"
        );
        assert_eq!(s.meta_get(v1, "origin").await.unwrap().unwrap(), b"snap/g0");
        // Matching expectation swaps.
        let out = s
            .transact(
                v1,
                vec![RefOp::MetaCas {
                    key: "origin".into(),
                    expected: Some(b"snap/g0".to_vec()),
                    value: b"snap/g1".to_vec(),
                }],
            )
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed);
        assert_eq!(s.meta_get(v1, "origin").await.unwrap().unwrap(), b"snap/g1");
        s.transact(
            v1,
            vec![RefOp::MetaDelete {
                key: "origin".into(),
            }],
        )
        .await
        .unwrap();

        // volume-scoped list
        s.compare_and_swap(v2, "head", None, Some([6u8; 32]))
            .await
            .unwrap();
        assert_eq!(s.list(v1).await.unwrap().len(), 1); // head only
        assert_eq!(s.list(v2).await.unwrap().len(), 1);
    }

    /// The D-M3 lease state machine, uniform across stores. The store must
    /// have been built on a clock that `advance_ms` moves forward — expiry
    /// is exercised by advancing time, never by sleeping.
    pub async fn exercise_lease(s: &dyn RefStore, advance_ms: &(dyn Fn(u64) + Send + Sync)) {
        let v = VolumeId(ulid::Ulid::from_parts(0xD3, 1));
        const TTL: u64 = 10_000;

        assert_eq!(s.lease_get(v).await.unwrap(), None);

        // Fresh acquire: epoch 1.
        let a = match s.lease_acquire(v, "node-a", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => l,
            other => panic!("expected acquired, got {other:?}"),
        };
        assert_eq!((a.holder.as_str(), a.epoch), ("node-a", 1));

        // A competing taker is Held off while the lease is live.
        match s.lease_acquire(v, "node-b", TTL).await.unwrap() {
            LeaseOutcome::Held(l) => assert_eq!((l.holder.as_str(), l.epoch), ("node-a", 1)),
            other => panic!("expected held, got {other:?}"),
        }

        // Renewal keeps the epoch and pushes the expiry out.
        advance_ms(4_000);
        let renewed = match s.lease_acquire(v, "node-a", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => l,
            other => panic!("expected renewal, got {other:?}"),
        };
        assert_eq!(renewed.epoch, 1, "renewal must not bump the epoch");
        assert!(renewed.expires_at_ms > a.expires_at_ms);

        // Steal after expiry: epoch bumps — the fence line moves.
        advance_ms(TTL + 1);
        let b = match s.lease_acquire(v, "node-b", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => l,
            other => panic!("expected steal, got {other:?}"),
        };
        assert_eq!((b.holder.as_str(), b.epoch), ("node-b", 2));

        // Releasing someone else's lease is a no-op.
        s.lease_release(v, "node-a").await.unwrap();
        assert_eq!(s.lease_get(v).await.unwrap().unwrap().holder, "node-b");

        // Release keeps (holder, epoch), zeroes expiry...
        s.lease_release(v, "node-b").await.unwrap();
        let released = s.lease_get(v).await.unwrap().unwrap();
        assert_eq!((released.epoch, released.expires_at_ms), (2, 0));

        // ...so the next taker still bumps: epochs are monotonic across
        // release, which is what the task-6 fence filter stands on.
        match s.lease_acquire(v, "node-c", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => assert_eq!((l.holder.as_str(), l.epoch), ("node-c", 3)),
            other => panic!("expected acquire after release, got {other:?}"),
        }
    }

    /// The Invariant F fenced transaction (task 15h), uniform across
    /// stores: `transact_as` commits only under the exact `(holder,
    /// epoch)` the lease row records, fails closed with the *actual*
    /// holder and epoch otherwise, and applies nothing on failure. Like
    /// [`exercise_lease`], expiry is exercised by advancing the store's
    /// injected clock.
    pub async fn exercise_fenced_transact(
        s: &dyn RefStore,
        advance_ms: &(dyn Fn(u64) + Send + Sync),
    ) {
        let v = VolumeId(ulid::Ulid::from_parts(0xF0, 15));
        const TTL: u64 = 10_000;
        let cas_create = |name: &str, byte: u8| RefOp::CasRef {
            name: name.into(),
            expected: None,
            new_commit: Some([byte; 32]),
        };
        let pin = |name: &str, holder: &str| RefOp::Pin {
            name: name.into(),
            holder: holder.into(),
        };

        // Absent lease row: LeaseLost with holder "" epoch 0, nothing lands.
        match s
            .transact_as(v, vec![cas_create("snap/f0", 1)], "node-a", 1)
            .await
        {
            Err(RefError::LeaseLost { holder, epoch, .. }) => {
                assert_eq!((holder.as_str(), epoch), ("", 0))
            }
            other => panic!("expected LeaseLost on absent lease, got {other:?}"),
        }
        assert_eq!(s.get(v, "snap/f0").await.unwrap(), None);

        // Matching (holder, epoch) commits — refs, pins and meta together.
        let a = match s.lease_acquire(v, "node-a", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => l,
            other => panic!("expected acquire, got {other:?}"),
        };
        assert_eq!(a.epoch, 1);
        let out = s
            .transact_as(
                v,
                vec![
                    cas_create("snap/f0", 1),
                    pin("snap/f0", "publisher"),
                    RefOp::MetaPut {
                        key: "f/meta".into(),
                        value: b"m".to_vec(),
                    },
                ],
                "node-a",
                1,
            )
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed);
        assert!(s.get(v, "snap/f0").await.unwrap().is_some());
        assert_eq!(s.pins(v, "snap/f0").await.unwrap(), vec!["publisher"]);

        // Wrong epoch → LeaseLost carrying the actual (holder, epoch);
        // nothing applied.
        match s
            .transact_as(v, vec![cas_create("snap/f1", 2)], "node-a", 7)
            .await
        {
            Err(RefError::LeaseLost { holder, epoch, .. }) => {
                assert_eq!((holder.as_str(), epoch), ("node-a", 1))
            }
            other => panic!("expected LeaseLost on wrong epoch, got {other:?}"),
        }
        assert_eq!(s.get(v, "snap/f1").await.unwrap(), None);

        // Wrong holder → LeaseLost, nothing applied.
        match s
            .transact_as(
                v,
                vec![cas_create("snap/f1", 2), pin("snap/f0", "intruder")],
                "node-b",
                1,
            )
            .await
        {
            Err(RefError::LeaseLost { holder, epoch, .. }) => {
                assert_eq!((holder.as_str(), epoch), ("node-a", 1))
            }
            other => panic!("expected LeaseLost on wrong holder, got {other:?}"),
        }
        assert_eq!(s.get(v, "snap/f1").await.unwrap(), None);
        assert_eq!(
            s.pins(v, "snap/f0").await.unwrap(),
            vec!["publisher"],
            "a fenced-out txn must apply nothing"
        );

        // Expired-but-matching passes: the holder never fences itself.
        advance_ms(TTL + 1);
        let out = s
            .transact_as(v, vec![cas_create("snap/f1", 2)], "node-a", 1)
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed, "expired-but-matching is OK");

        // A steal moves the fence line: the old (holder, epoch) is refused
        // with the thief's identity in the error.
        advance_ms(TTL + 1);
        match s.lease_acquire(v, "node-b", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => assert_eq!((l.holder.as_str(), l.epoch), ("node-b", 2)),
            other => panic!("expected steal, got {other:?}"),
        }
        match s
            .transact_as(v, vec![cas_create("snap/f2", 3)], "node-a", 1)
            .await
        {
            Err(RefError::LeaseLost { holder, epoch, .. }) => {
                assert_eq!((holder.as_str(), epoch), ("node-b", 2))
            }
            other => panic!("expected LeaseLost after steal, got {other:?}"),
        }
        assert_eq!(s.get(v, "snap/f2").await.unwrap(), None);

        // §8.2 tip-unchanged leniency, refs level: the thief dies (lease
        // expires), the original holder re-acquires — epoch bumps — and
        // its transactions under the NEW epoch commit. There is no
        // resurrection of the old epoch: the same call under epoch 1
        // stays fenced forever.
        advance_ms(TTL + 1);
        let a2 = match s.lease_acquire(v, "node-a", TTL).await.unwrap() {
            LeaseOutcome::Acquired(l) => l,
            other => panic!("expected re-acquire after the thief expired, got {other:?}"),
        };
        assert_eq!(a2.epoch, 3);
        match s
            .transact_as(v, vec![cas_create("snap/f2", 3)], "node-a", 1)
            .await
        {
            Err(RefError::LeaseLost { epoch, .. }) => assert_eq!(epoch, 3),
            other => panic!("the superseded epoch must stay fenced, got {other:?}"),
        }
        let out = s
            .transact_as(v, vec![cas_create("snap/f2", 3)], "node-a", 3)
            .await
            .unwrap();
        assert_eq!(out, TxnOutcome::Committed, "re-acquired epoch publishes");

        // A conflicting CAS under a valid fence is still an ordinary
        // Conflict — fencing must not change transact semantics.
        let out = s
            .transact_as(v, vec![cas_create("snap/f2", 9)], "node-a", 3)
            .await
            .unwrap();
        assert!(matches!(out, TxnOutcome::Conflict { op_index: 0, .. }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_passes_conformance() {
        conformance::exercise_ref_store(&MemoryRefStore::new()).await;
    }

    #[tokio::test]
    async fn memory_store_passes_lease_conformance() {
        let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000_000));
        let clock: Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(std::sync::atomic::Ordering::SeqCst))
        };
        let s = MemoryRefStore::new().with_clock(clock);
        let advance = move |ms: u64| _ = now.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
        conformance::exercise_lease(&s, &advance).await;
    }

    #[tokio::test]
    async fn memory_store_passes_fenced_transact_conformance() {
        let now = Arc::new(std::sync::atomic::AtomicU64::new(1_000_000));
        let clock: Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(std::sync::atomic::Ordering::SeqCst))
        };
        let s = MemoryRefStore::new().with_clock(clock);
        let advance = move |ms: u64| _ = now.fetch_add(ms, std::sync::atomic::Ordering::SeqCst);
        conformance::exercise_fenced_transact(&s, &advance).await;
    }

    fn vol() -> VolumeId {
        VolumeId(ulid::Ulid::from_parts(1, 1))
    }

    #[tokio::test]
    async fn create_if_absent_then_advance() {
        let s = MemoryRefStore::new();
        let v = vol();

        // I2: head advances only by CAS from the current value.
        let out = s
            .compare_and_swap(v, "head", None, Some([1u8; 32]))
            .await
            .unwrap();
        let g0 = match out {
            CasOutcome::Committed(Some(rv)) => {
                assert_eq!(rv.generation, 0);
                rv
            }
            other => panic!("expected commit, got {other:?}"),
        };

        let out = s
            .compare_and_swap(v, "head", Some(g0), Some([2u8; 32]))
            .await
            .unwrap();
        match out {
            CasOutcome::Committed(Some(rv)) => {
                assert_eq!(rv.generation, 1);
                assert_eq!(rv.commit, [2u8; 32]);
            }
            other => panic!("expected commit, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn stale_expectation_conflicts_and_reports_actual() {
        let s = MemoryRefStore::new();
        let v = vol();
        s.compare_and_swap(v, "head", None, Some([1u8; 32]))
            .await
            .unwrap();
        let stale = RefValue {
            commit: [9u8; 32],
            generation: 7,
        };
        match s
            .compare_and_swap(v, "head", Some(stale), Some([2u8; 32]))
            .await
            .unwrap()
        {
            CasOutcome::Conflict(Some(actual)) => {
                assert_eq!(actual.commit, [1u8; 32]);
                assert_eq!(actual.generation, 0);
            }
            other => panic!("expected conflict, got {other:?}"),
        }
        // Losing a CAS must not modify the ref (I2).
        assert_eq!(s.get(v, "head").await.unwrap().unwrap().commit, [1u8; 32]);
    }

    #[tokio::test]
    async fn create_conflicts_if_already_present() {
        let s = MemoryRefStore::new();
        let v = vol();
        s.compare_and_swap(v, "snap/a", None, Some([1u8; 32]))
            .await
            .unwrap();
        assert!(matches!(
            s.compare_and_swap(v, "snap/a", None, Some([2u8; 32]))
                .await
                .unwrap(),
            CasOutcome::Conflict(Some(_))
        ));
    }

    #[tokio::test]
    async fn delete_if_matches() {
        let s = MemoryRefStore::new();
        let v = vol();
        s.compare_and_swap(v, "snap/a", None, Some([1u8; 32]))
            .await
            .unwrap();
        let cur = s.get(v, "snap/a").await.unwrap().unwrap();
        assert!(matches!(
            s.compare_and_swap(v, "snap/a", Some(cur), None)
                .await
                .unwrap(),
            CasOutcome::Committed(None)
        ));
        assert_eq!(s.get(v, "snap/a").await.unwrap(), None);
    }

    #[tokio::test]
    async fn list_is_volume_scoped() {
        let s = MemoryRefStore::new();
        let v1 = VolumeId(ulid::Ulid::from_parts(1, 1));
        let v2 = VolumeId(ulid::Ulid::from_parts(2, 2));
        s.compare_and_swap(v1, "head", None, Some([1u8; 32]))
            .await
            .unwrap();
        s.compare_and_swap(v1, "snap/a", None, Some([2u8; 32]))
            .await
            .unwrap();
        s.compare_and_swap(v2, "head", None, Some([3u8; 32]))
            .await
            .unwrap();
        assert_eq!(s.list(v1).await.unwrap().len(), 2);
        assert_eq!(s.list(v2).await.unwrap().len(), 1);
    }
}

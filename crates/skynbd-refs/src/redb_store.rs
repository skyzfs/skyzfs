//! `RedbRefStore`: the production single-node ref store (design D6, §6.1).
//!
//! One redb file holds all refs. CAS runs inside a single redb write
//! transaction, giving exactly the atomicity `MemoryRefStore` models and
//! etcd will later provide. Calls are executed inline (redb ops on a local
//! file are microseconds); TODO(M2): move behind `spawn_blocking` when the
//! NBD hot path starts sharing the runtime.

use redb::{Database, ReadableTable, TableDefinition};
use skynbd_types::{CommitHash, VolumeId};

use crate::{
    lease, CasOutcome, Clock, Lease, LeaseOutcome, RefError, RefEvent, RefOp, RefStore, RefValue,
    Result, TxnOutcome,
};

/// key: (volume ulid as u128, ref name) → value: (generation, commit bytes)
const REFS: TableDefinition<(u128, &str), (u64, &[u8])> = TableDefinition::new("refs");
/// key: (vol, ref name, holder) → unit. Refcount = row count per ref.
const PINS: TableDefinition<(u128, &str, &str), ()> = TableDefinition::new("pins");
/// key: (vol, meta key) → bytes (the design §6.1 `prepared` table).
const META: TableDefinition<(u128, &str), &[u8]> = TableDefinition::new("meta");
/// key: vol → (holder, epoch, expires_at_ms) — the D-M3 lease row. On a
/// single-node redb this is bookkeeping, not exclusion (the file lock
/// already excludes other processes), but the semantics match the other
/// stores so a redb→object migration carries the epoch along.
const OWNER: TableDefinition<u128, (&str, u64, u64)> = TableDefinition::new("owner");

pub struct RedbRefStore {
    db: Database,
    clock: Clock,
    events: tokio::sync::broadcast::Sender<RefEvent>,
}

impl RedbRefStore {
    pub fn open(path: impl AsRef<std::path::Path>) -> Result<Self> {
        let db = Database::create(path).map_err(|e| RefError::Backend(e.to_string()))?;
        // Ensure the table exists so first reads don't error.
        let txn = db.begin_write().map_err(backend)?;
        txn.open_table(REFS).map_err(backend)?;
        txn.open_table(PINS).map_err(backend)?;
        txn.open_table(META).map_err(backend)?;
        txn.open_table(OWNER).map_err(backend)?;
        txn.commit().map_err(backend)?;
        Ok(Self {
            db,
            clock: crate::system_clock(),
            events: tokio::sync::broadcast::channel(crate::WATCH_CAPACITY).0,
        })
    }

    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// [`RefStore::transact`] / [`RefStore::transact_as`]: one redb write
    /// transaction; the lease-row fence (when given) is read inside it,
    /// so validation and mutation are one atomic step.
    async fn transact_fenced(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        fence: Option<(&str, u64)>,
    ) -> Result<TxnOutcome> {
        let txn = self.db.begin_write().map_err(backend)?;
        let mut events = Vec::new();
        let outcome = {
            if let Some((holder, epoch)) = fence {
                let owner = txn.open_table(OWNER).map_err(backend)?;
                let current = owner
                    .get(vol.0 .0)
                    .map_err(backend)?
                    .map(|g| decode_lease(g.value()));
                lease::fence(current.as_ref(), vol, holder, epoch)?;
            }
            let mut refs = txn.open_table(REFS).map_err(backend)?;
            let mut pins = txn.open_table(PINS).map_err(backend)?;
            let mut meta = txn.open_table(META).map_err(backend)?;

            // Validate all CAS expectations first (all-or-nothing).
            let mut conflict = None;
            for (i, op) in ops.iter().enumerate() {
                match op {
                    RefOp::CasRef { name, expected, .. } => {
                        let current = match refs.get((vol.0 .0, name.as_str())).map_err(backend)? {
                            Some(g) => Some(decode(g.value())?),
                            None => None,
                        };
                        if current != *expected {
                            conflict = Some(TxnOutcome::Conflict {
                                op_index: i,
                                actual: current,
                            });
                            break;
                        }
                    }
                    RefOp::MetaCas { key, expected, .. } => {
                        let current = meta
                            .get((vol.0 .0, key.as_str()))
                            .map_err(backend)?
                            .map(|g| g.value().to_vec());
                        if current != *expected {
                            conflict = Some(TxnOutcome::Conflict {
                                op_index: i,
                                actual: None,
                            });
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match conflict {
                Some(c) => c,
                None => {
                    for op in ops {
                        match op {
                            RefOp::CasRef {
                                name,
                                expected,
                                new_commit,
                            } => {
                                let key = (vol.0 .0, name.as_str());
                                let stored = match new_commit {
                                    Some(commit) => {
                                        let generation =
                                            expected.map(|e| e.generation + 1).unwrap_or(0);
                                        refs.insert(key, (generation, commit.as_slice()))
                                            .map_err(backend)?;
                                        Some(RefValue { commit, generation })
                                    }
                                    None => {
                                        refs.remove(key).map_err(backend)?;
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
                                pins.insert((vol.0 .0, name.as_str(), holder.as_str()), ())
                                    .map_err(backend)?;
                            }
                            RefOp::Unpin { name, holder } => {
                                pins.remove((vol.0 .0, name.as_str(), holder.as_str()))
                                    .map_err(backend)?;
                            }
                            RefOp::MetaPut { key, value } | RefOp::MetaCas { key, value, .. } => {
                                meta.insert((vol.0 .0, key.as_str()), value.as_slice())
                                    .map_err(backend)?;
                            }
                            RefOp::MetaDelete { key } => {
                                meta.remove((vol.0 .0, key.as_str())).map_err(backend)?;
                            }
                        }
                    }
                    TxnOutcome::Committed
                }
            }
        };
        match outcome {
            TxnOutcome::Committed => {
                txn.commit().map_err(backend)?;
                for e in events {
                    let _ = self.events.send(e);
                }
                Ok(TxnOutcome::Committed)
            }
            conflict => Ok(conflict), // txn dropped: nothing applied
        }
    }
}

fn backend<E: std::fmt::Display>(e: E) -> RefError {
    RefError::Backend(e.to_string())
}

fn decode(gen_commit: (u64, &[u8])) -> Result<RefValue> {
    let (generation, raw) = gen_commit;
    let commit: CommitHash = raw
        .try_into()
        .map_err(|_| RefError::Backend(format!("corrupt commit hash of len {}", raw.len())))?;
    Ok(RefValue { commit, generation })
}

#[async_trait::async_trait]
impl RefStore for RedbRefStore {
    async fn get(&self, vol: VolumeId, name: &str) -> Result<Option<RefValue>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(REFS).map_err(backend)?;
        match table.get((vol.0 .0, name)).map_err(backend)? {
            Some(guard) => Ok(Some(decode(guard.value())?)),
            None => Ok(None),
        }
    }

    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> Result<CasOutcome> {
        let txn = self.db.begin_write().map_err(backend)?;
        let outcome = {
            let mut table = txn.open_table(REFS).map_err(backend)?;
            let key = (vol.0 .0, name);
            let current = match table.get(key).map_err(backend)? {
                Some(guard) => Some(decode(guard.value())?),
                None => None,
            };
            if current != expected {
                CasOutcome::Conflict(current)
            } else {
                match new_commit {
                    Some(commit) => {
                        let generation = expected.map(|e| e.generation + 1).unwrap_or(0);
                        table
                            .insert(key, (generation, commit.as_slice()))
                            .map_err(backend)?;
                        CasOutcome::Committed(Some(RefValue { commit, generation }))
                    }
                    None => {
                        table.remove(key).map_err(backend)?;
                        CasOutcome::Committed(None)
                    }
                }
            }
        };
        txn.commit().map_err(backend)?;
        if let CasOutcome::Committed(stored) = &outcome {
            let _ = self.events.send(RefEvent {
                vol,
                name: name.to_string(),
                value: *stored,
            });
        }
        Ok(outcome)
    }

    async fn list(&self, vol: VolumeId) -> Result<Vec<(String, RefValue)>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(REFS).map_err(backend)?;
        let mut out = Vec::new();
        let range = table
            .range((vol.0 .0, "")..=(vol.0 .0, "\u{10FFFF}"))
            .map_err(backend)?;
        for entry in range {
            let (k, v) = entry.map_err(backend)?;
            out.push((k.value().1.to_string(), decode(v.value())?));
        }
        Ok(out)
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
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(PINS).map_err(backend)?;
        let mut out = Vec::new();
        let range = table
            .range((vol.0 .0, name, "")..=(vol.0 .0, name, "\u{10FFFF}"))
            .map_err(backend)?;
        for entry in range {
            let (k, _) = entry.map_err(backend)?;
            out.push(k.value().2.to_string());
        }
        Ok(out)
    }

    async fn meta_get(&self, vol: VolumeId, key: &str) -> Result<Option<Vec<u8>>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(META).map_err(backend)?;
        Ok(table
            .get((vol.0 .0, key))
            .map_err(backend)?
            .map(|g| g.value().to_vec()))
    }

    async fn meta_list(&self, vol: VolumeId, prefix: &str) -> Result<Vec<(String, Vec<u8>)>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(META).map_err(backend)?;
        let mut out = Vec::new();
        let range = table
            .range((vol.0 .0, "")..=(vol.0 .0, "\u{10FFFF}"))
            .map_err(backend)?;
        for entry in range {
            let (k, v) = entry.map_err(backend)?;
            let key = k.value().1.to_string();
            if key.starts_with(prefix) {
                out.push((key, v.value().to_vec()));
            }
        }
        Ok(out)
    }

    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> Result<LeaseOutcome> {
        let now = (self.clock)();
        let txn = self.db.begin_write().map_err(backend)?;
        let outcome = {
            let mut table = txn.open_table(OWNER).map_err(backend)?;
            let current = table
                .get(vol.0 .0)
                .map_err(backend)?
                .map(|g| decode_lease(g.value()));
            let (write, outcome) = lease::acquire(current.as_ref(), holder, ttl_ms, now);
            if let Some(l) = write {
                table
                    .insert(vol.0 .0, (l.holder.as_str(), l.epoch, l.expires_at_ms))
                    .map_err(backend)?;
            }
            outcome
        };
        txn.commit().map_err(backend)?;
        Ok(outcome)
    }

    async fn lease_release(&self, vol: VolumeId, holder: &str) -> Result<()> {
        let txn = self.db.begin_write().map_err(backend)?;
        {
            let mut table = txn.open_table(OWNER).map_err(backend)?;
            let current = table
                .get(vol.0 .0)
                .map_err(backend)?
                .map(|g| decode_lease(g.value()));
            if let Some(l) = lease::release(current.as_ref(), holder) {
                table
                    .insert(vol.0 .0, (l.holder.as_str(), l.epoch, l.expires_at_ms))
                    .map_err(backend)?;
            }
        }
        txn.commit().map_err(backend)?;
        Ok(())
    }

    async fn lease_get(&self, vol: VolumeId) -> Result<Option<Lease>> {
        let txn = self.db.begin_read().map_err(backend)?;
        let table = txn.open_table(OWNER).map_err(backend)?;
        Ok(table
            .get(vol.0 .0)
            .map_err(backend)?
            .map(|g| decode_lease(g.value())))
    }
}

fn decode_lease((holder, epoch, expires_at_ms): (&str, u64, u64)) -> Lease {
    Lease {
        holder: holder.to_string(),
        epoch,
        expires_at_ms,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    #[tokio::test]
    async fn redb_store_passes_conformance() {
        let dir = tempfile::tempdir().unwrap();
        let store = RedbRefStore::open(dir.path().join("refs.redb")).unwrap();
        conformance::exercise_ref_store(&store).await;
    }

    #[tokio::test]
    async fn redb_store_passes_lease_conformance() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock: Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let store = RedbRefStore::open(dir.path().join("refs.redb"))
            .unwrap()
            .with_clock(clock);
        let advance = move |ms: u64| _ = now.fetch_add(ms, Ordering::SeqCst);
        conformance::exercise_lease(&store, &advance).await;
    }

    #[tokio::test]
    async fn redb_store_passes_fenced_transact_conformance() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        let dir = tempfile::tempdir().unwrap();
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock: Clock = {
            let now = Arc::clone(&now);
            Arc::new(move || now.load(Ordering::SeqCst))
        };
        let store = RedbRefStore::open(dir.path().join("refs.redb"))
            .unwrap()
            .with_clock(clock);
        let advance = move |ms: u64| _ = now.fetch_add(ms, Ordering::SeqCst);
        conformance::exercise_fenced_transact(&store, &advance).await;
    }

    #[tokio::test]
    async fn redb_store_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("refs.redb");
        let vol = VolumeId(ulid::Ulid::from_parts(7, 7));
        {
            let store = RedbRefStore::open(&path).unwrap();
            store
                .compare_and_swap(vol, "head", None, Some([5u8; 32]))
                .await
                .unwrap();
        }
        let store = RedbRefStore::open(&path).unwrap();
        let head = store.get(vol, "head").await.unwrap().unwrap();
        assert_eq!(head.commit, [5u8; 32]);
        assert_eq!(head.generation, 0);
    }
}

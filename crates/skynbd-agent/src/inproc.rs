//! In-process `ControlPlane` wired to `skynbd-engine`'s `PreparedSession`
//! (agent design doc §3.3): agent-vs-server integration in one process,
//! no network, and the server side is the *real* server logic.
//!
//! Live sessions are held in a map exactly like `skynbd-api`'s
//! `VolumeHandle`; [`InprocControlPlane::drop_sessions`] simulates a server
//! crash (staged ops above the durable watermark are lost, S13-style).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;

use skynbd_engine::prepared::{list_prepared, PreparedMeta, PreparedSession};
use skynbd_engine::{keys, Engine, EngineConfig, EngineError};
use skynbd_refs::{RefOp, RefStore, TxnOutcome};
use skynbd_store::ObjectStore;
use skynbd_types::{SnapId, VolumeId};

use crate::control::{ControlPlane, CpError, PreparedInfo, RangeOp};

impl From<EngineError> for CpError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::SnapshotExists(s) => CpError::Conflict(s),
            EngineError::SnapshotNotFound(s) | EngineError::PreparedNotFound(s) => {
                CpError::NotFound(s)
            }
            // Invariant F: the epoch-specific refusal stays
            // distinguishable in-process exactly as over HTTP.
            EngineError::Fenced { holder, epoch } => CpError::Fenced { holder, epoch },
            other => CpError::Other(other.to_string()),
        }
    }
}

type Sessions<S, R> = Arc<Mutex<HashMap<String, PreparedSession<S, R>>>>;

#[derive(Clone)]
pub struct InprocControlPlane<S, R> {
    store: S,
    refs: R,
    vol: VolumeId,
    sessions: Sessions<S, R>,
}

impl<S: ObjectStore + Clone, R: RefStore + Clone> InprocControlPlane<S, R> {
    /// Create the lineage volume server-side and return the plane.
    pub async fn create_volume(
        store: S,
        refs: R,
        vol: VolumeId,
        size: u64,
    ) -> Result<Self, CpError> {
        Engine::create_volume(
            store.clone(),
            refs.clone(),
            vol,
            size,
            EngineConfig::default(),
        )
        .await?;
        Ok(Self {
            store,
            refs,
            vol,
            sessions: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    /// Simulate a server restart: live sessions (and their un-checkpointed
    /// overlays) vanish; durable refs/meta/objects survive.
    pub async fn drop_sessions(&self) {
        self.sessions.lock().await.clear();
    }

    fn parse_snap_id(snap_id: &str) -> Result<SnapId, CpError> {
        SnapId::from_string(snap_id).map_err(|_| CpError::NotFound(snap_id.to_string()))
    }

    /// Run `f` against the live (or reopened) session for `snap_id`.
    async fn with_session<T>(
        &self,
        snap_id: &str,
        f: impl AsyncFnOnce(&mut PreparedSession<S, R>) -> Result<T, EngineError>,
    ) -> Result<T, CpError> {
        let sid = Self::parse_snap_id(snap_id)?;
        let mut sessions = self.sessions.lock().await;
        if !sessions.contains_key(snap_id) {
            let s = PreparedSession::open_existing(
                self.store.clone(),
                self.refs.clone(),
                self.vol,
                sid,
            )
            .await?;
            sessions.insert(snap_id.to_string(), s);
        }
        Ok(f(sessions.get_mut(snap_id).unwrap()).await?)
    }

    /// Take the session out of the map (publish/abort consume it).
    async fn take_session(&self, snap_id: &str) -> Result<PreparedSession<S, R>, CpError> {
        let sid = Self::parse_snap_id(snap_id)?;
        let mut sessions = self.sessions.lock().await;
        match sessions.remove(snap_id) {
            Some(s) => Ok(s),
            None => Ok(PreparedSession::open_existing(
                self.store.clone(),
                self.refs.clone(),
                self.vol,
                sid,
            )
            .await?),
        }
    }
}

impl<S: ObjectStore + Clone, R: RefStore + Clone> ControlPlane for InprocControlPlane<S, R> {
    async fn prepare(&self, base_ref: Option<&str>) -> Result<String, CpError> {
        let base = match base_ref {
            Some(name) => {
                let rv = self
                    .refs
                    .get(self.vol, name)
                    .await
                    .map_err(|e| CpError::Other(e.to_string()))?
                    .ok_or_else(|| CpError::NotFound(name.to_string()))?;
                Some((name.to_string(), rv.commit))
            }
            None => None,
        };
        let snap_id = SnapId(ulid::Ulid::new());
        let session = PreparedSession::create(
            self.store.clone(),
            self.refs.clone(),
            self.vol,
            snap_id,
            base,
        )
        .await?;
        self.sessions
            .lock()
            .await
            .insert(snap_id.to_string(), session);
        Ok(snap_id.to_string())
    }

    async fn find_prepared(&self) -> Result<Option<PreparedInfo>, CpError> {
        let ids = list_prepared(&self.refs, self.vol).await?;
        let Some(sid) = ids.first() else {
            return Ok(None);
        };
        let bytes = self
            .refs
            .meta_get(self.vol, &format!("prepared/{sid}"))
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
            .ok_or_else(|| CpError::NotFound(sid.to_string()))?;
        let meta: PreparedMeta =
            postcard::from_bytes(&bytes).map_err(|e| CpError::Other(e.to_string()))?;
        Ok(Some(PreparedInfo {
            snap_id: sid.to_string(),
            base_ref: meta.base_ref,
            op_watermark: meta.op_watermark,
        }))
    }

    async fn ensure(&self, snap_id: &str, hashes: &[[u8; 32]]) -> Result<Vec<bool>, CpError> {
        let hashes = hashes.to_vec();
        self.with_session(snap_id, async move |s| s.ensure(&hashes).await)
            .await
    }

    async fn put_object(&self, hash: [u8; 32], data: Vec<u8>) -> Result<(), CpError> {
        if *blake3::hash(&data).as_bytes() != hash {
            return Err(CpError::Other("object hash mismatch".into()));
        }
        self.store
            .put(&keys::cas_object(self.vol, &hash), data.into())
            .await
            .map_err(|e| CpError::Other(e.to_string()))
    }

    async fn attach(
        &self,
        snap_id: &str,
        op_id: u64,
        ranges: &[(u64, u64, [u8; 32], u64)],
    ) -> Result<(), CpError> {
        let ranges = ranges.to_vec();
        self.with_session(snap_id, async move |s| {
            s.attach(&ranges, op_id).await.map(|_| ())
        })
        .await
    }

    async fn clear(&self, snap_id: &str, op_id: u64, ranges: &[(u64, u64)]) -> Result<(), CpError> {
        let ranges = ranges.to_vec();
        self.with_session(snap_id, async move |s| {
            s.clear(&ranges, op_id).await.map(|_| ())
        })
        .await
    }

    async fn apply(&self, snap_id: &str, op_id: u64, ops: &[RangeOp]) -> Result<(), CpError> {
        let ops = ops.to_vec();
        self.with_session(snap_id, async move |s| {
            s.apply(&ops, op_id).await.map(|_| ())
        })
        .await
    }

    async fn checkpoint(&self, snap_id: &str, op_id: u64) -> Result<(), CpError> {
        self.with_session(snap_id, async move |s| s.checkpoint(op_id).await)
            .await
    }

    async fn publish(&self, snap_id: &str, name: &str, publisher: &str) -> Result<(), CpError> {
        let mut session = self.take_session(snap_id).await?;
        session.publish(name, publisher).await?;
        Ok(())
    }

    async fn abort(&self, snap_id: &str) -> Result<(), CpError> {
        let session = self.take_session(snap_id).await?;
        session.abort().await?;
        Ok(())
    }

    async fn ref_exists(&self, ref_name: &str) -> Result<bool, CpError> {
        Ok(self
            .refs
            .get(self.vol, ref_name)
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
            .is_some())
    }

    async fn pinned_by(&self, ref_name: &str, holder: &str) -> Result<bool, CpError> {
        Ok(self
            .refs
            .pins(self.vol, ref_name)
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
            .iter()
            .any(|h| h == holder))
    }

    async fn pin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.txn(RefOp::Pin {
            name: ref_name.to_string(),
            holder: holder.to_string(),
        })
        .await
    }

    async fn unpin(&self, ref_name: &str, holder: &str) -> Result<(), CpError> {
        self.txn(RefOp::Unpin {
            name: ref_name.to_string(),
            holder: holder.to_string(),
        })
        .await
    }

    async fn repoint_origin(&self, target: &str) -> Result<(), CpError> {
        // Mirror the server route's transaction exactly (15a + 15g):
        // Invariant E's monotone decision, then pin-new + MetaCas-row +
        // unpin-old, atomic, with the dedicated `origin` holder.
        if self
            .refs
            .get(self.vol, target)
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
            .is_none()
        {
            return Err(CpError::NotFound(target.to_string()));
        }
        let old_bytes = self
            .refs
            .meta_get(self.vol, "origin")
            .await
            .map_err(|e| CpError::Other(e.to_string()))?;
        let old = old_bytes
            .as_deref()
            .map(|b| String::from_utf8_lossy(b).into_owned());
        let forward = skynbd_types::origin::repoint_forward(old.as_deref(), target)
            .map_err(CpError::Other)?;
        if !forward {
            // Row already at the target or a descendant — the replayed
            // repoint no-ops (the advance's fence accepts descendants).
            return Ok(());
        }
        let mut ops = vec![
            RefOp::Pin {
                name: target.to_string(),
                holder: "origin".into(),
            },
            RefOp::MetaCas {
                key: "origin".into(),
                expected: old_bytes,
                value: target.as_bytes().to_vec(),
            },
        ];
        if let Some(old) = old.filter(|o| o != target) {
            ops.push(RefOp::Unpin {
                name: old,
                holder: "origin".into(),
            });
        }
        match self
            .refs
            .transact(self.vol, ops)
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
        {
            TxnOutcome::Committed => Ok(()),
            TxnOutcome::Conflict { .. } => Err(CpError::Conflict("repoint txn".into())),
        }
    }

    async fn origin_served(&self) -> Result<Option<String>, CpError> {
        // In-process there is no live NBD export; the pointer row IS what
        // a fresh resolution would serve. (The end-to-end staleness the
        // fence exists for needs the real server — 15g's matrix.)
        Ok(self
            .refs
            .meta_get(self.vol, "origin")
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
            .map(|b| String::from_utf8_lossy(&b).into_owned()))
    }

    async fn read_snapshot(&self, ref_name: &str, off: u64, len: u64) -> Result<Vec<u8>, CpError> {
        let ro = Engine::open_ref(
            self.store.clone(),
            self.refs.clone(),
            self.vol,
            ref_name,
            EngineConfig::default(),
        )
        .await?;
        Ok(ro.read(off, len).await?)
    }
}

impl<S: ObjectStore + Clone, R: RefStore + Clone> InprocControlPlane<S, R> {
    async fn txn(&self, op: RefOp) -> Result<(), CpError> {
        match self
            .refs
            .transact(self.vol, vec![op])
            .await
            .map_err(|e| CpError::Other(e.to_string()))?
        {
            TxnOutcome::Committed => Ok(()),
            TxnOutcome::Conflict { .. } => Err(CpError::Conflict("ref txn".into())),
        }
    }
}

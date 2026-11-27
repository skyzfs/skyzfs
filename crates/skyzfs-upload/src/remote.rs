//! What the uploader asks of object storage, behind a small trait
//! ([`Remote`]) so that the crash matrix can drive the real engine over a
//! store that dies at any operation (the lead's condition on M2.4b).
//!
//! [`EngineRemote`] is the engine of `skynbd-engine` opened at the
//! volume's head, with the refs object for the row: the writes of a cut,
//! its sealed segments PUT as it goes (M2.4a), and one commit that swaps
//! head and the cut's row together. A publish (M2.5) PUTs the manifests
//! and makes one transaction of its own on the same refs, and a retire
//! (M2.7b) deletes what a publish made there.

use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_format::{Codec, CommitKind, SnapKind, SnapshotManifest};
use skynbd_refs::{RefError, RefOp, RefStore, TxnOutcome};
use skynbd_store::{ObjectStore, StoreError};
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_local::inplace::CommittedCut;

use crate::publish::{manifest_key, snap_ref, snap_row_key, Found, Snapshot};
use crate::row::{self, ROW_KEY};

/// How many times [`Remote::retire`] reads `snap/<name>` and tries its
/// transaction, each try after a transaction refused because the ref
/// moved between the read and the transaction.
pub const RETIRE_TRIES: usize = 3;

/// What [`Remote::retire`] found and did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retired {
    /// `snap/<name>` named the commit, and it and its row are deleted.
    Retired,
    /// There is no `snap/<name>`: nothing was deleted.
    Gone,
    /// `snap/<name>` names this other commit: nothing was deleted.
    Other(CommitHash),
}

/// Why a call to object storage failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RemoteError {
    /// The commit's transaction was refused: head or the row was not as
    /// the commit expected, or a refs store that fences writes found no
    /// lease row for the volume (`LeaseLost` with no holder, which the
    /// engine maps to a conflict; G7 of #114's review). The daemon creates
    /// the row when it takes the lease (M2.4c), so the second is not
    /// expected there; either way a conflict is given up, and the
    /// uploader waits before the next ([`crate::backoff`]). The commit did
    /// not land, unless the error itself was a retry of a request that
    /// had landed (the uploader reads the row again to know).
    #[error("the commit was refused: head or the cut's row moved, or the volume has no lease row")]
    Conflict,
    /// The volume's lease is held by another holder: nothing is written
    /// under it until the daemon holds it again (M2.4c).
    #[error("the volume's lease is held by {holder:?} at epoch {epoch}")]
    Fenced {
        /// The holder.
        holder: String,
        /// Its epoch.
        epoch: u64,
    },
    /// The volume has no head ref, or its state cannot be read.
    #[error("the volume in object storage: {0}")]
    Volume(String),
    /// Any other failure: the store or the refs could not be reached, or
    /// answered with an error. A call made again may succeed.
    #[error("object storage: {0}")]
    Store(String),
}

impl From<EngineError> for RemoteError {
    fn from(e: EngineError) -> Self {
        match e {
            EngineError::Conflict | EngineError::OpConflict { .. } => RemoteError::Conflict,
            EngineError::Fenced { holder, epoch } => RemoteError::Fenced { holder, epoch },
            EngineError::Corrupt(why) => RemoteError::Volume(why),
            other => RemoteError::Store(other.to_string()),
        }
    }
}

impl From<RefError> for RemoteError {
    fn from(e: RefError) -> Self {
        RemoteError::from(EngineError::from(e))
    }
}

impl From<StoreError> for RemoteError {
    fn from(e: StoreError) -> Self {
        RemoteError::from(EngineError::from(e))
    }
}

/// Head's row and head's commit, as read now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The row's bytes; `None` where there is no row.
    pub row: Option<Vec<u8>>,
    /// The group of the commit in `head`, `None` for a commit with none:
    /// the engine's own where it is open (at head, or opened again), so
    /// that the group judged is that of the commit judged (L1 of the
    /// review of #134). Where the engine is left behind head (the branch
    /// with writes), it is read from that commit's object, not taken from
    /// the engine. The check reads it only under a row of version 1,
    /// which names no commit, or no row.
    pub head_group: Option<u64>,
    /// Whether the engine is open at head as read now. An engine with no
    /// writes is opened again where head has moved, so only one with
    /// writes (a commit that failed) can be left behind it. An engine
    /// opened again reads head a second time, and is not `current`
    /// where another writer moved head between the two reads.
    pub current: bool,
    /// Head's commit as the engine's next commit will find it: where the
    /// engine is open (it was at head, or was opened again), the commit
    /// it sits on, which its head swap expects; where it is left behind
    /// head, head as read now. The check judges this commit, so that the
    /// head it judged is the one a commit goes onto, even where another
    /// writer swapped head while the engine was opened again (B1 of the
    /// review of #134). Where the row names it, this is the cut's commit:
    /// this uploader moves the row only with head, in one transaction,
    /// and a writer that moves head alone leaves the row naming the
    /// commit it moved head from (F1).
    pub head: CommitHash,
}

/// Object storage, as the uploader reaches it.
// As for `Local`: one thread's runtime, and the crate's own trait.
#[allow(async_fn_in_trait)]
pub trait Remote {
    /// Read head's row and head, and open the engine at head where it has
    /// none or has moved and holds no writes.
    async fn observe(&mut self) -> Result<Observed, RemoteError>;

    /// Write `data` at `offset` into the engine, uncommitted.
    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError>;

    /// Write `len` bytes of zeros at `offset`, as a zero extent.
    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError>;

    /// PUT the segments sealed so far, so that a cut is not held in
    /// memory until its commit.
    async fn put_sealed(&mut self) -> Result<(), RemoteError>;

    /// Commit what was written, with `cut`'s group in the commit, swapping
    /// head and the cut's row in one transaction to a row that names `cut`
    /// and the commit's hash ([`row::encode`]); the commit's hash. The
    /// row's swap expects `expected`: the row as the check read it, `None`
    /// for no row. After an error the engine keeps its writes for the
    /// commit made again, and the error does not say whether it landed.
    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> Result<CommitHash, RemoteError>;

    /// `snap/<name>` and its row, `None` where there is no such ref.
    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError>;

    /// Publish `snapshot` on `commit`, a commit that has landed: PUT the
    /// ZFS manifest and SkyNBD's framed manifest, then, in one
    /// transaction, create `snap/<name>` on `commit` (expecting none) and
    /// put the snapshot's row. [`RemoteError::Conflict`] where the ref
    /// exists. After any error the caller looks the ref up: no error
    /// says that the transaction did not land.
    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError>;

    /// Retire `snap/<name>` where it names `commit` (M2.7b): in one
    /// transaction, delete the ref, compared with the value read just
    /// before, and its row. A ref that moved between the read and the
    /// transaction is read again, at most [`RETIRE_TRIES`] times in all,
    /// and then the error is [`RemoteError::Conflict`]. As for a publish,
    /// no error says that the transaction did not land: asked again, a
    /// retire that landed finds [`Retired::Gone`]. The commit's blocks and
    /// the manifests stay, for garbage collection (M4).
    ///
    /// A pin is not looked at: where `skynbd_engine::Engine`'s own
    /// snapshot delete refuses a pinned ref, a retire deletes it all the
    /// same. Nothing in SkyZFS pins `snap/T` today; a clone or a browse
    /// that will (M3, M4) needs a check here first (D2 of the review of
    /// #175, the lead's ruling).
    async fn retire(&mut self, name: &str, commit: CommitHash) -> Result<Retired, RemoteError>;

    /// Drop the writes not committed.
    fn discard(&mut self);

    /// Drop the writes and the engine itself: the next
    /// [`Remote::observe`] opens it at head. After a conflict, whose
    /// engine has a head that is no longer the volume's.
    fn reset(&mut self);
}

/// The engine of `skynbd-engine`, opened at the volume's head when first
/// observed.
pub struct EngineRemote<S, R> {
    store: S,
    refs: R,
    vol: VolumeId,
    cfg: EngineConfig,
    engine: Option<Engine<S, R>>,
}

impl<S, R> EngineRemote<S, R> {
    /// The volume `vol` in `store`, its refs in `refs`. Nothing is read
    /// until the first [`Remote::observe`].
    pub fn new(store: S, refs: R, vol: VolumeId, cfg: EngineConfig) -> Self {
        Self {
            store,
            refs,
            vol,
            cfg,
            engine: None,
        }
    }

    /// The engine, where one is open.
    fn engine(&mut self) -> Result<&mut Engine<S, R>, RemoteError> {
        self.engine
            .as_mut()
            .ok_or_else(|| RemoteError::Volume("no engine is open: observe first".into()))
    }
}

impl<S, R> Remote for EngineRemote<S, R>
where
    S: ObjectStore + Clone,
    R: RefStore + Clone,
{
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        let row = self.refs.meta_get(self.vol, ROW_KEY).await?;
        let head = self
            .refs
            .get(self.vol, "head")
            .await?
            .ok_or_else(|| RemoteError::Volume("the volume has no head".into()))?;
        let at_head = |engine: &Engine<S, R>| engine.head_commit_hash() == head.commit;
        match &self.engine {
            Some(engine) if at_head(engine) => {}
            Some(engine) if !engine.is_clean() => {
                let head_group = engine.commit_group(&head.commit).await?;
                return Ok(Observed {
                    row,
                    head_group,
                    current: false,
                    head: head.commit,
                });
            }
            _ => {
                self.engine = None;
                let engine = Engine::open(
                    self.store.clone(),
                    self.refs.clone(),
                    self.vol,
                    self.cfg.clone(),
                )
                .await?;
                self.engine = Some(engine);
            }
        }
        let engine = self.engine()?;
        // Head as the engine sits on it, not as read above: an engine
        // opened again read head a second time, and its swap expects
        // that read (B1 of the review of #134).
        Ok(Observed {
            row,
            head_group: engine.head_group(),
            current: engine.head_commit_hash() == head.commit,
            head: engine.head_commit_hash(),
        })
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        Ok(self.engine()?.write(offset, data)?)
    }

    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        Ok(self.engine()?.write_zeroes(offset, len)?)
    }

    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        Ok(self.engine()?.put_sealed().await?)
    }

    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> Result<CommitHash, RemoteError> {
        let engine = self.engine()?;
        engine.set_group(Some(cut.group));
        // The row names the commit it lands with: the engine knows the
        // commit's hash before the swap, and builds the ops from it (F1).
        let ops = move |commit: CommitHash| {
            vec![RefOp::MetaCas {
                key: ROW_KEY.into(),
                expected,
                value: row::encode(&cut, &commit),
            }]
        };
        Ok(engine.commit_naming(CommitKind::Flush, ops).await?)
    }

    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        Ok(crate::look::find(&self.refs, self.vol, name).await?)
    }

    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError> {
        let ref_name = snap_ref(snapshot.name);
        // The manifests first, as `Engine::snapshot_create` PUTs its own
        // before the ref: a ref never names a snapshot whose manifest is
        // missing. A death between them leaves garbage, no ref.
        self.store
            .put(
                &manifest_key(self.vol, snapshot.snap),
                snapshot.manifest.to_vec().into(),
            )
            .await?;
        let manifest = SnapshotManifest {
            snap_id: snapshot.snap,
            vol_id: self.vol,
            ref_name: ref_name.clone(),
            commit,
            parent_snap: None,
            kind: SnapKind::Manual,
            created_at_ms: now_ms(),
        };
        let encoded = manifest
            .encode()
            .map_err(|e| RemoteError::Volume(format!("the snapshot's manifest: {e}")))?;
        self.store
            .put(
                &skynbd_engine::keys::snapshot(self.vol, snapshot.snap),
                encoded.into(),
            )
            .await?;
        let ops = vec![
            RefOp::CasRef {
                name: ref_name,
                expected: None,
                new_commit: Some(commit),
            },
            RefOp::MetaPut {
                key: snap_row_key(snapshot.name),
                value: snapshot.row.encode(),
            },
        ];
        match self.refs.transact(self.vol, ops).await? {
            TxnOutcome::Committed => Ok(()),
            TxnOutcome::Conflict { .. } => Err(RemoteError::Conflict),
        }
    }

    async fn retire(&mut self, name: &str, commit: CommitHash) -> Result<Retired, RemoteError> {
        let ref_name = snap_ref(name);
        let mut current = self.refs.get(self.vol, &ref_name).await?;
        for _ in 0..RETIRE_TRIES {
            let Some(value) = current else {
                return Ok(Retired::Gone);
            };
            if value.commit != commit {
                return Ok(Retired::Other(value.commit));
            }
            let ops = vec![
                RefOp::CasRef {
                    name: ref_name.clone(),
                    expected: Some(value),
                    new_commit: None,
                },
                RefOp::MetaDelete {
                    key: snap_row_key(name),
                },
            ];
            match self.refs.transact(self.vol, ops).await? {
                TxnOutcome::Committed => return Ok(Retired::Retired),
                TxnOutcome::Conflict { actual, .. } => current = actual,
            }
        }
        Err(RemoteError::Conflict)
    }

    fn discard(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            engine.discard_uncommitted();
        }
    }

    fn reset(&mut self) {
        self.engine = None;
    }
}

/// Now, in milliseconds since the Unix epoch, for a manifest's
/// `created_at_ms` (0 where the clock is before the epoch).
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

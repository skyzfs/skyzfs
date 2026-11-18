//! What the uploader asks of object storage, behind a small trait
//! ([`Remote`]) so that the crash matrix can drive the real engine over a
//! store that dies at any operation (the lead's condition on M2.4b).
//!
//! [`EngineRemote`] is the engine of `skynbd-engine` opened at the
//! volume's head, with the refs object for the row: the writes of a cut,
//! its sealed segments PUT as it goes (M2.4a), and one commit that swaps
//! head and the cut's row together.

use skynbd_engine::{Engine, EngineConfig, EngineError};
use skynbd_format::CommitKind;
use skynbd_refs::{RefError, RefOp, RefStore};
use skynbd_store::ObjectStore;
use skynbd_types::VolumeId;

use crate::row::ROW_KEY;

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

/// Head's row and head's commit, as read now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Observed {
    /// The row's bytes; `None` where there is no row.
    pub row: Option<Vec<u8>>,
    /// The group of head's commit, `None` for a commit with none: what
    /// the engine was opened at, which is head as read now where
    /// `current`.
    pub head_group: Option<u64>,
    /// Whether the engine is open at head as read now. An engine with no
    /// writes is opened again where head has moved, so only one with
    /// writes (a commit that failed) can be left behind it.
    pub current: bool,
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

    /// Commit what was written, with `group` in the commit, swapping head
    /// and the cut's row to `row` in one transaction. The row's swap
    /// expects `expected`: the row as the check read it, `None` for no
    /// row. After an error the engine keeps its writes for the commit
    /// made again, and the error does not say whether it landed.
    async fn commit(
        &mut self,
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<(), RemoteError>;

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
                return Ok(Observed {
                    row,
                    head_group: engine.head_group(),
                    current: false,
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
        Ok(Observed {
            row,
            head_group: engine.head_group(),
            current: engine.head_commit_hash() == head.commit,
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
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<(), RemoteError> {
        let engine = self.engine()?;
        engine.set_group(Some(group));
        let ops = vec![RefOp::MetaCas {
            key: ROW_KEY.into(),
            expected,
            value: row,
        }];
        engine.commit_with(CommitKind::Flush, ops).await?;
        Ok(())
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

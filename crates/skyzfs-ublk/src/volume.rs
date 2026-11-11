//! The lifecycle's [`Volume`] over the real store: the lock as a step
//! of its own ([`Store::lock_volume`]) and the open under it
//! ([`Store::open_locked`]).
//!
//! # How the store's errors are read
//!
//! The lifecycle acts differently on four kinds of failed open, and
//! [`classify`] sorts the store's errors into them:
//!
//! - **The log is shorter than what was acknowledged** ([`OpenError::Short`]):
//!   the store's own comparison failed, or the valid prefix ends before
//!   a position the log's superblock says was durable. Both say that
//!   something the log held is gone. (The second is this module's
//!   reading: the design names only the comparison.)
//! - **The recovery failed the log** ([`OpenError::SyncFailed`]): a
//!   sync or a write of the recovery returned an error, or the log had
//!   failed already. Never "try again".
//! - **The volume is marked as failed** ([`OpenError::MarkedFailed`]).
//! - **Anything else** ([`OpenError::Other`]): a read that failed, a
//!   directory that holds no log, files that do not fit together. The
//!   daemon stops and deletes nothing.

use std::path::{Path, PathBuf};

use skyzfs_blk::BlockTarget;
use skyzfs_local::format::Lsn;
use skyzfs_local::fs::{FsDir, FsLock};
use skyzfs_local::log::{Corruption, LogError};
use skyzfs_local::store::{OpenOptions, Store, StoreError};

use crate::lifecycle::{LockError, MustReach, OpenError, Volume};

/// A volume in a directory on a real file system.
#[derive(Debug, Clone)]
pub struct StoreVolume {
    dir: PathBuf,
    options: OpenOptions,
}

impl StoreVolume {
    /// The volume in `dir`, to be opened with `options`. The position
    /// the log must reach and whether a failure is acknowledged are
    /// the lifecycle's to give: whatever `options` says about them is
    /// replaced at the open.
    pub fn new(dir: &Path, options: OpenOptions) -> Self {
        Self {
            dir: dir.into(),
            options,
        }
    }
}

/// Which kind of failed open `error` is: see the
/// [module documentation](self).
pub fn classify(error: &StoreError) -> OpenError {
    match error {
        StoreError::Log(LogError::Short { .. })
        | StoreError::Log(LogError::Corrupt(Corruption::BelowDurableFloor { .. })) => {
            OpenError::Short
        }
        StoreError::Log(LogError::MarkedFailed) => OpenError::MarkedFailed,
        StoreError::Log(LogError::Failed) | StoreError::Log(LogError::Io { failed: true, .. }) => {
            OpenError::SyncFailed
        }
        other => OpenError::Other(other.to_string()),
    }
}

impl Volume for StoreVolume {
    type Lock = FsLock;
    type Open = Store<FsDir>;

    fn lock(&mut self) -> Result<FsLock, LockError> {
        let dir = FsDir::new(&self.dir).map_err(|e| LockError::Other(e.to_string()))?;
        match Store::<FsDir>::lock_volume(&dir) {
            Ok(lock) => Ok(lock),
            Err(StoreError::Locked) => Err(LockError::Locked),
            Err(other) => Err(LockError::Other(other.to_string())),
        }
    }

    fn open(
        &mut self,
        lock: FsLock,
        must_reach: Option<MustReach>,
        acknowledge_failure: bool,
    ) -> Result<(Store<FsDir>, u64), (OpenError, FsLock)> {
        let dir = match FsDir::new(&self.dir) {
            Ok(dir) => dir,
            Err(e) => return Err((OpenError::Other(e.to_string()), lock)),
        };
        let mut options = self.options.clone();
        options.acknowledge_failure = acknowledge_failure;
        options.must_reach = must_reach.map(|m| Lsn(m.position));
        options.must_reach_identity = must_reach.map(|m| m.identity);
        match Store::open_locked(dir, lock, options) {
            Ok((store, _)) => {
                let size = store.geometry().size_bytes();
                Ok((store, size))
            }
            Err((error, lock)) => Err((classify(&error), lock)),
        }
    }
}

#[cfg(test)]
mod tests;

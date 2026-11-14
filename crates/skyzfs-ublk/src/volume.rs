//! The lifecycle's [`Volume`] over the in-place store: the lock as a
//! step of its own ([`skyzfs_local::inplace::lock`]) and the open under
//! it ([`Opened::open`]), the first phase of a start. The second phase,
//! [`Opened::start`], is the daemon's, once the device is its.
//!
//! # How the store's errors are read
//!
//! The lifecycle acts differently on kinds of failed open, and
//! [`classify`] sorts the store's errors into them:
//!
//! - **The volume is marked as failed** ([`OpenError::MarkedFailed`]):
//!   its failure marker, which the lifecycle may acknowledge.
//! - **Anything else** ([`OpenError::Other`]): a volume file that is
//!   missing, damaged, of a version this daemon does not know or of the
//!   log store; a home image that does not fit it; a file system or a
//!   page larger than the volume's write unit; a call that failed. The
//!   daemon stops, deletes nothing, and the volume has not failed.
//!
//! Since M1.3t nothing here gives [`OpenError::Short`] or
//! [`OpenError::SyncFailed`]: they were the log store's (a log shorter
//! than what was acknowledged, a recovery whose sync failed). The
//! lifecycle keeps reading them until M1.3t-b. The in-place store has
//! no log to hold to a position of what was acknowledged
//! ([`Volume::reaches_positions`] is false): the lifecycle then gives it
//! none and does not read its record, so an unreadable record no longer
//! fails the volume (`Reason::PositionUnreadable`, code 5).

use std::path::{Path, PathBuf};

use skyzfs_local::fs::{CacheCountCall, FsDir, FsLock};
use skyzfs_local::inplace::{self, OpenOptions, Opened};

use crate::lifecycle::{LockError, MustReach, OpenError, Volume};

/// A volume in a directory on a real file system.
#[derive(Debug, Clone)]
pub struct StoreVolume {
    dir: PathBuf,
    options: OpenOptions,
    cache_count: Option<CacheCountCall>,
}

impl StoreVolume {
    /// The volume in `dir`, to be opened with `options`. Whether a
    /// failure is acknowledged is the lifecycle's to give: whatever
    /// `options` says about it is replaced at the open.
    pub fn new(dir: &Path, options: OpenOptions) -> Self {
        Self {
            dir: dir.into(),
            options,
            cache_count: None,
        }
    }

    /// The same, whose files count their pages in the kernel's cache
    /// with `call` ([`FsDir::with_cache_count`]). Without it the start
    /// refuses the volume.
    pub fn with_cache_count(self, call: CacheCountCall) -> Self {
        Self {
            cache_count: Some(call),
            ..self
        }
    }
}

/// Which kind of failed open `error` is: see the
/// [module documentation](self).
pub fn classify(error: &inplace::OpenError) -> OpenError {
    use inplace::OpenError as E;
    match error {
        E::MarkedFailed => OpenError::MarkedFailed,
        // The volume is not one this daemon serves here: a refusal that
        // wrote nothing, and no failure of the volume.
        refused @ (E::Volume(_)
        | E::Damaged(_)
        | E::Blocks { .. }
        | E::BlocksUnknown { .. }
        | E::Pages { .. }) => OpenError::Other(format!(
            "{refused}. Nothing was written and the volume is not failed. A device of this \
                 volume that waits for a server is left as it is; `skyzfsd detach` removes it"
        )),
        other => OpenError::Other(other.to_string()),
    }
}

impl Volume for StoreVolume {
    type Lock = FsLock;
    type Open = Opened<FsDir>;

    fn lock(&mut self) -> Result<FsLock, LockError> {
        let dir = FsDir::new(&self.dir).map_err(|e| LockError::Other(e.to_string()))?;
        match inplace::lock(&dir) {
            Ok(lock) => Ok(lock),
            Err(inplace::OpenError::Locked) => Err(LockError::Locked),
            Err(other) => Err(LockError::Other(other.to_string())),
        }
    }

    /// The position to reach is ignored: see the
    /// [module documentation](self).
    fn open(
        &mut self,
        lock: FsLock,
        _must_reach: Option<MustReach>,
        acknowledge_failure: bool,
    ) -> Result<(Opened<FsDir>, u64), (OpenError, FsLock)> {
        let dir = match FsDir::new(&self.dir) {
            Ok(dir) => match self.cache_count {
                Some(call) => dir.with_cache_count(call),
                None => dir,
            },
            Err(e) => return Err((OpenError::Other(e.to_string()), lock)),
        };
        let mut options = self.options.clone();
        options.acknowledge_failure = acknowledge_failure;
        match Opened::open(dir, lock, options) {
            Ok(opened) => {
                let size = opened.geometry().size_bytes();
                Ok((opened, size))
            }
            Err((error, lock)) => Err((classify(&error), lock)),
        }
    }

    /// The in-place store holds to no position: the lifecycle does not
    /// read the record of what was acknowledged for it.
    fn reaches_positions(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests;

//! `serve --kill-at-upload` (test hooks only): the uploader's thread
//! kills the daemon, as `kill -9` would, at a point of an upload that a
//! rig cannot hit by timing. The rig then serves the volume again and
//! checks what object storage and the store hold.
//!
//! - `put:N`: once the Nth PUT of a segment (a key under `seg/`) has
//!   landed, before the step that sent it goes on. Other PUTs (nodes,
//!   commits, the refs object) are not counted.
//! - `swap:N`: once the Nth commit of a cut has landed, which swaps head
//!   and the cut's row in one transaction, and before the cut is let go
//!   in the store.
//! - `manifest:N`: once the Nth PUT of a snapshot's framed manifest (a
//!   key under `snap/`, the last PUT of a publish) has landed, before the
//!   transaction that makes its ref (M2.5-2). The ZFS manifest's PUT
//!   (under `skyzfs/manifest/`) comes before it and is not counted.
//! - `publish:N`: once the Nth publish's transaction has landed (its ref
//!   `snap/<name>` and its row), before the uploader knows it (M2.5-2).
//!
//! Only a PUT, a commit or a publish that came back as landed is counted:
//! one that failed or was refused is not, and is not killed at.
//!
//! The daemon says on its standard error which point it reached, then
//! sends itself SIGKILL.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use skynbd_refs::{ActingRefStore, RefStore};
use skynbd_store::{ObjectStore, Precondition, Result};
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_local::inplace::CommittedCut;
use skyzfs_upload::publish::{Found, Snapshot};
use skyzfs_upload::{EngineRemote, Observed, Remote, RemoteError};

use crate::cli::KillAt;

/// What a kill point does once reached: given what was reached.
pub type Die = Arc<dyn Fn(&str) + Send + Sync>;

/// The daemon's: say what was reached on standard error, then SIGKILL
/// to the daemon itself.
pub fn sigkill() -> Die {
    Arc::new(|reached: &str| {
        crate::to_stderr(&format!(
            "skyzfsd: test hook --kill-at-upload: killed {reached}\n"
        ));
        let me = rustix::process::getpid();
        let _ = rustix::process::kill_process(me, rustix::process::Signal::KILL);
        // The signal is delivered before the call returns to this thread;
        // should it not be, the daemon still goes no further.
        std::process::abort();
    })
}

/// The remote the uploader makes ([`crate::upload::maker`]), with its
/// objects in a [`KillStore`] and itself in a [`KillRemote`]: what
/// [`crate::upload::Uploads::begin`] gives the uploader in a build with
/// the test hooks. With no `kill`, neither kills.
pub fn maker<S, F>(
    objects: S,
    refs: F,
    holder: String,
    vol: VolumeId,
    kill: Option<KillAt>,
    die: Die,
) -> impl FnMut(u64) -> KillRemote<EngineRemote<KillStore<S>, Arc<ActingRefStore<F>>>> + Send + 'static
where
    S: ObjectStore + Clone + 'static,
    F: RefStore + Clone + 'static,
{
    let objects = KillStore::new(objects, kill, Arc::clone(&die));
    let mut make = crate::upload::maker(objects, refs, holder, vol);
    move |epoch| KillRemote::new(make(epoch), kill, Arc::clone(&die))
}

/// Which PUTs a [`KillStore`] counts.
#[derive(Debug, Clone, Copy)]
enum Counted {
    /// `put:N`: a segment's.
    Segment,
    /// `manifest:N`: a snapshot's framed manifest.
    Manifest,
}

impl Counted {
    fn counts(self, key: &str) -> bool {
        match self {
            Self::Segment => key.contains("/seg/"),
            Self::Manifest => key.contains("/snap/"),
        }
    }

    fn reached(self, n: u64, key: &str) -> String {
        match self {
            Self::Segment => format!("after segment PUT {n} landed ({key})"),
            Self::Manifest => {
                format!("after the framed manifest PUT {n} landed ({key}), before its ref was made")
            }
        }
    }
}

/// The objects of the uploader's engine, with `put:N` and `manifest:N`.
pub struct KillStore<S> {
    inner: S,
    at: Option<(Counted, u64)>,
    puts: Arc<AtomicU64>,
    die: Die,
}

impl<S: Clone> Clone for KillStore<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            at: self.at,
            puts: Arc::clone(&self.puts),
            die: Arc::clone(&self.die),
        }
    }
}

impl<S> KillStore<S> {
    /// `inner`, killed at by `die` where `kill` is `put:N` or
    /// `manifest:N`.
    pub fn new(inner: S, kill: Option<KillAt>, die: Die) -> Self {
        let at = match kill {
            Some(KillAt::Put(n)) => Some((Counted::Segment, n)),
            Some(KillAt::Manifest(n)) => Some((Counted::Manifest, n)),
            _ => None,
        };
        Self {
            inner,
            at,
            puts: Arc::new(AtomicU64::new(0)),
            die,
        }
    }
}

#[async_trait::async_trait]
impl<S: ObjectStore> ObjectStore for KillStore<S> {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.inner.put(key, data).await?;
        if let Some((counted, at)) = self.at {
            if counted.counts(key) {
                let n = self.puts.fetch_add(1, Ordering::SeqCst) + 1;
                if n == at {
                    (self.die)(&counted.reached(n, key));
                }
            }
        }
        Ok(())
    }

    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        self.inner.get(key, range).await
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        self.inner.exists(key).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }

    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        self.inner.list_with_mtime(prefix).await
    }

    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        self.inner.get_versioned(key).await
    }

    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> Result<String> {
        self.inner.put_if(key, data, pre).await
    }
}

/// The uploader's remote, with `swap:N` and `publish:N`.
pub struct KillRemote<R> {
    inner: R,
    at: Option<u64>,
    swaps: u64,
    publish_at: Option<u64>,
    publishes: u64,
    die: Die,
}

impl<R> KillRemote<R> {
    /// `inner`, killed at by `die` where `kill` is `swap:N` or
    /// `publish:N`.
    pub fn new(inner: R, kill: Option<KillAt>, die: Die) -> Self {
        let (at, publish_at) = match kill {
            Some(KillAt::Swap(n)) => (Some(n), None),
            Some(KillAt::Publish(n)) => (None, Some(n)),
            _ => (None, None),
        };
        Self {
            inner,
            at,
            swaps: 0,
            publish_at,
            publishes: 0,
            die,
        }
    }
}

impl<R: Remote> Remote for KillRemote<R> {
    async fn observe(&mut self) -> std::result::Result<Observed, RemoteError> {
        self.inner.observe().await
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> std::result::Result<(), RemoteError> {
        self.inner.write(offset, data)
    }

    fn write_zeroes(&mut self, offset: u64, len: u64) -> std::result::Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }

    async fn put_sealed(&mut self) -> std::result::Result<(), RemoteError> {
        self.inner.put_sealed().await
    }

    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> std::result::Result<CommitHash, RemoteError> {
        let commit = self.inner.commit(cut, expected).await?;
        self.swaps += 1;
        if self.at == Some(self.swaps) {
            (self.die)(&format!(
                "after the swap of commit {} (group {}) landed, before its cut was let go",
                self.swaps, cut.group
            ));
        }
        Ok(commit)
    }

    async fn snapshot(&mut self, name: &str) -> std::result::Result<Option<Found>, RemoteError> {
        self.inner.snapshot(name).await
    }

    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> std::result::Result<(), RemoteError> {
        self.inner.publish(commit, snapshot).await?;
        self.publishes += 1;
        if self.publish_at == Some(self.publishes) {
            (self.die)(&format!(
                "after the transaction of publish {} ({}) landed, before the uploader knew it",
                self.publishes, snapshot.name
            ));
        }
        Ok(())
    }

    fn discard(&mut self) {
        self.inner.discard();
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

#[cfg(test)]
mod tests;

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
//! - `carry-put:N`: once the first PUT of a segment of the cut chosen to
//!   carry publish N has landed (M2.6c-4).
//! - `carry-swap:N`: once the commit of the cut chosen to carry publish N
//!   has landed, and before the cut is let go in the store (M2.6c-4).
//!
//! Only a PUT, a commit or a publish that came back as landed is counted:
//! one that failed or was refused is not, and is not killed at.
//!
//! The daemon says on its standard error which point it reached, then
//! sends itself SIGKILL.
//!
//! # The cut chosen to carry a publish
//!
//! Publish N is the Nth publish the uploader accepted in this serve
//! (`Answer::Accepted` to the channel's `publish`). A request refused
//! is not counted; one asked again under a name already accepted is
//! accepted again, and counted again. The hook ([`Carry`]) chooses
//! the first cut whose reading begins after publish N was accepted (the
//! uploader's read of it from block 0, after its check found it to
//! upload) and that is of the request's cut state, counted after its
//! *C* and of its group *G* or later: by state, number and group, as
//! `skyzfs_upload::publish::Request::carried_by` judges, but for opening
//! as itself. The hook fires only if that cut carries the publish, that
//! is also opens as itself; otherwise, and where the chosen cut ends
//! without the hook having fired (its commit landed, its read failed, it
//! was abandoned), it chooses the next cut whose reading begins, by the
//! same rule. A cut that was found committed is not read and is never
//! chosen; neither is one whose reading began before publish N was
//! accepted, though it may be the cut that carries it.
//!
//! `carry-swap:N` fires once the chosen cut's commit has landed, so only
//! where it commits. `carry-put:N` fires at the first segment PUT of the
//! chosen cut that landed, where the hook cannot know whether the cut
//! will commit: it fires where the cut carries the publish should it
//! commit. A chosen cut that puts no segment (its blocks are all zeroes)
//! is passed over at its end, and the next is chosen.
//!
//! The stderr line names the cut chosen by its number and group.

use std::collections::VecDeque;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use bytes::Bytes;
use skynbd_refs::{ActingRefStore, RefStore};
use skynbd_store::{ObjectStore, Precondition, Result};
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_blk::Buf;
use skyzfs_local::inplace::{CommittedCut, CutSlice};
use skyzfs_upload::publish::{Found, Request, Snapshot};
use skyzfs_upload::{
    Answer, Control, EngineRemote, Local, Observed, Queued, Released, Remote, RemoteError,
    Uploadable,
};
use tokio::sync::{mpsc, oneshot};

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
/// the test hooks. With no `kill`, neither kills. `carry` is the hook of
/// `carry-put:N` and `carry-swap:N`, which both are given.
pub fn maker<S, F>(
    objects: S,
    refs: F,
    holder: String,
    vol: VolumeId,
    kill: Option<KillAt>,
    die: Die,
    carry: Option<Arc<Carry>>,
) -> impl FnMut(u64) -> KillRemote<EngineRemote<KillStore<S>, Arc<ActingRefStore<F>>>> + Send + 'static
where
    S: ObjectStore + Clone + 'static,
    F: RefStore + Clone + 'static,
{
    let objects = KillStore::new(objects, kill, Arc::clone(&die)).carrying(carry.clone());
    let mut make = crate::upload::maker(objects, refs, holder, vol);
    move |epoch| KillRemote::new(make(epoch), kill, Arc::clone(&die)).carrying(carry.clone())
}

/// What a cut must be to be chosen for a publish ([module docs](self)).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Wanted {
    state: [u8; 16],
    after: u64,
    group: u64,
}

impl Wanted {
    fn of(request: &Request) -> Self {
        Self {
            state: request.state,
            after: request.after,
            group: request.group,
        }
    }

    /// Whether `cut` is chosen: of the state, counted after *C*, of group
    /// *G* or later.
    fn chooses(&self, cut: &Uploadable) -> bool {
        cut.state == self.state && cut.number > self.after && cut.group >= self.group
    }
}

/// A publish asked, passed on to the uploader with an answer of the
/// hook's own: the uploader's answer is read from `answer` and given to
/// `reply`, the asker's.
struct Asked {
    wanted: Wanted,
    answer: oneshot::Receiver<Answer>,
    reply: oneshot::Sender<Answer>,
}

/// Which of the two carry hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CarryAt {
    Put,
    Swap,
}

/// What [`Carry`] has seen.
#[derive(Default)]
struct Seen {
    /// The publishes passed on and not yet answered, in order.
    asked: VecDeque<Asked>,
    /// The publishes accepted.
    accepted: u64,
    /// Publish N's, once accepted.
    wanted: Option<Wanted>,
    /// The cut last offered.
    offered: Option<Uploadable>,
    /// The cut whose reading began last.
    reading: Option<Uploadable>,
    /// The cut chosen to carry publish N.
    chosen: Option<Uploadable>,
    /// The hook fired.
    fired: bool,
}

/// Whether `a` and `b` are the same cut.
fn same(a: &Uploadable, b: &Uploadable) -> bool {
    a.state == b.state && a.number == b.number
}

/// `carry-put:N` and `carry-swap:N` ([module docs](self)): what the
/// uploader's thread is given around the uploader
/// ([`crate::upload::Around`]), its objects ([`KillStore`]) and its
/// remote ([`KillRemote`]). Every call is made on the uploader's thread.
pub struct Carry {
    at: CarryAt,
    n: u64,
    die: Die,
    seen: Mutex<Seen>,
}

impl Carry {
    /// The hook of `kill` where it is `carry-put:N` or `carry-swap:N`,
    /// killing by `die`; `None` otherwise.
    pub fn new(kill: Option<KillAt>, die: Die) -> Option<Arc<Self>> {
        let (at, n) = match kill {
            Some(KillAt::CarryPut(n)) => (CarryAt::Put, n),
            Some(KillAt::CarrySwap(n)) => (CarryAt::Swap, n),
            _ => return None,
        };
        Some(Arc::new(Self {
            at,
            n,
            die,
            seen: Mutex::new(Seen::default()),
        }))
    }

    fn seen(&self) -> MutexGuard<'_, Seen> {
        // What a panic left half done is a test hook's bookkeeping, not
        // the daemon's: go on with it.
        self.seen.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A request of the channel, as the uploader is given it: a publish
    /// with an answer of the hook's own, which [`Carry::answered`] reads
    /// and passes on to the asker; any other request as it was. An ask
    /// whose asker has gone is passed on with that answer gone too, so
    /// that the uploader takes nothing, as it would have.
    pub fn ask(&self, control: Control) -> Control {
        let Control::Ask {
            request,
            rewound,
            reply,
        } = control
        else {
            return control;
        };
        let (mine, answer) = oneshot::channel();
        if !reply.is_closed() {
            self.seen().asked.push_back(Asked {
                wanted: Wanted::of(&request),
                answer,
                reply,
            });
        }
        Control::Ask {
            request,
            rewound,
            reply: mine,
        }
    }

    /// After each post of the uploader, which follows each request it
    /// answered: pass each answer on to its asker, in order, counting the
    /// publishes accepted, and drop a publish whose asker has gone before
    /// it was answered (the uploader then takes nothing).
    pub fn answered(&self) {
        let mut seen = self.seen();
        for mut asked in std::mem::take(&mut seen.asked) {
            match asked.answer.try_recv() {
                Ok(answer) => {
                    if answer == Answer::Accepted {
                        seen.accepted += 1;
                        if seen.accepted == self.n {
                            seen.wanted = Some(asked.wanted);
                        }
                    }
                    let _ = asked.reply.send(answer);
                }
                Err(oneshot::error::TryRecvError::Empty) if !asked.reply.is_closed() => {
                    seen.asked.push_back(asked);
                }
                // Not answered: the asker went, or the uploader dropped it.
                Err(_) => {}
            }
        }
    }

    /// The uploader was offered `cut`.
    fn offered(&self, cut: Option<Uploadable>) {
        if cut.is_some() {
            self.seen().offered = cut;
        }
    }

    /// The uploader reads cut `number` from block `from`: from block 0,
    /// its reading begins, and the cut is chosen where none is that has
    /// not ended.
    fn reads(&self, number: u64, from: u64) {
        if from != 0 {
            return;
        }
        let mut seen = self.seen();
        let Some(cut) = seen.offered.filter(|cut| cut.number == number) else {
            return;
        };
        seen.reading = Some(cut);
        let Some(wanted) = seen.wanted else {
            return;
        };
        if seen.fired || seen.chosen.is_some_and(|chosen| same(&chosen, &cut)) {
            return;
        }
        seen.chosen = wanted.chooses(&cut).then_some(cut);
    }

    /// The chosen cut, where it carries the publish and the hook has not
    /// fired: it fires now.
    fn fires(&self, seen: &mut Seen, cut: impl Fn(&Uploadable) -> bool) -> Option<Uploadable> {
        let chosen = seen.chosen.filter(|chosen| chosen.itself && cut(chosen))?;
        if seen.fired {
            return None;
        }
        seen.fired = true;
        Some(chosen)
    }

    /// A PUT of `key` landed.
    fn put_landed(&self, key: &str) {
        if self.at != CarryAt::Put || !key.contains("/seg/") {
            return;
        }
        let fired = {
            let mut seen = self.seen();
            let reading = seen.reading;
            self.fires(&mut seen, |chosen| {
                reading.is_some_and(|reading| same(chosen, &reading))
            })
        };
        if let Some(cut) = fired {
            (self.die)(&format!(
                "carry-put:{n}: after the first segment PUT of cut {} (group {}), chosen to \
                 carry publish {n}, landed ({key})",
                cut.number,
                cut.group,
                n = self.n
            ));
        }
    }

    /// The commit of `cut` landed.
    fn committed(&self, cut: CommittedCut) {
        if self.at != CarryAt::Swap {
            return;
        }
        let fired = self.fires(&mut self.seen(), |chosen| chosen.row() == cut);
        if let Some(cut) = fired {
            (self.die)(&format!(
                "carry-swap:{n}: after the commit of cut {} (group {}), which carries publish \
                 {n}, landed, before the cut was let go",
                cut.number,
                cut.group,
                n = self.n
            ));
        }
    }

    /// The publishes accepted so far.
    #[cfg(test)]
    pub fn accepted(&self) -> u64 {
        self.seen().accepted
    }
}

impl crate::upload::Around for Arc<Carry> {
    type Local = KillLocal<Queued>;

    fn local(&self, queued: Queued) -> Self::Local {
        KillLocal::new(queued, Arc::clone(self))
    }

    fn inbox(&self, mut inbox: mpsc::Receiver<Control>) -> mpsc::Receiver<Control> {
        let (passed, passes) = mpsc::channel(1);
        let carry = Arc::clone(self);
        tokio::spawn(async move {
            while let Some(control) = inbox.recv().await {
                if passed.send(carry.ask(control)).await.is_err() {
                    return;
                }
            }
        });
        passes
    }

    fn posted(&self) {
        self.answered();
    }
}

/// The uploader's store, which tells [`Carry`] of each cut offered and
/// of each read.
pub struct KillLocal<L> {
    inner: L,
    carry: Arc<Carry>,
}

impl<L> KillLocal<L> {
    /// `inner`, told to `carry`.
    pub fn new(inner: L, carry: Arc<Carry>) -> Self {
        Self { inner, carry }
    }

    /// The store within.
    #[cfg(test)]
    pub fn inner_mut(&mut self) -> &mut L {
        &mut self.inner
    }
}

impl<L: Local> Local for KillLocal<L> {
    async fn uploadable(&mut self) -> Option<Uploadable> {
        let cut = self.inner.uploadable().await;
        self.carry.offered(cut);
        cut
    }

    async fn read(&mut self, number: u64, from: u64, buf: Buf) -> (Buf, io::Result<CutSlice>) {
        self.carry.reads(number, from);
        self.inner.read(number, from, buf).await
    }

    async fn sync_copies(&mut self, number: u64) -> io::Result<()> {
        self.inner.sync_copies(number).await
    }

    async fn release(&mut self, number: u64) -> Released {
        self.inner.release(number).await
    }

    async fn name_committed(&mut self, number: u64, group: u64) -> bool {
        self.inner.name_committed(number, group).await
    }

    async fn abandon_all(&mut self) -> Vec<u64> {
        self.inner.abandon_all().await
    }
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
    carry: Option<Arc<Carry>>,
}

impl<S: Clone> Clone for KillStore<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            at: self.at,
            puts: Arc::clone(&self.puts),
            die: Arc::clone(&self.die),
            carry: self.carry.clone(),
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
            carry: None,
        }
    }

    /// As it was, telling `carry` of each PUT that landed.
    pub fn carrying(mut self, carry: Option<Arc<Carry>>) -> Self {
        self.carry = carry;
        self
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
        if let Some(carry) = &self.carry {
            carry.put_landed(key);
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
    carry: Option<Arc<Carry>>,
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
            carry: None,
        }
    }

    /// As it was, telling `carry` of each commit that landed.
    pub fn carrying(mut self, carry: Option<Arc<Carry>>) -> Self {
        self.carry = carry;
        self
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
        if let Some(carry) = &self.carry {
            carry.committed(cut);
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

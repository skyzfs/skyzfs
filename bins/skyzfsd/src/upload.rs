//! The daemon's side of the uploader (M2.4c): the volume in object
//! storage at `create`, the lease at the start of `serve`, the uploader's
//! thread while it serves, and what the status shows of it. The loop the
//! thread turns, the lease's rules, the backoff and the desk are the
//! crate `skyzfs-upload`'s.
//!
//! # At `create` ([`place`])
//!
//! A volume created with `--endpoint --bucket --volume` is made in object
//! storage before the local volume is: its superblock, the empty commit
//! and head, with shards of [`SHARD_BYTES`] (decided for M2.4). Where the
//! name's volume is there already, `create` takes it only if a create
//! left it so: of the size asked, with shards of [`SHARD_BYTES`], not a
//! clone, head the empty commit a create makes (every shard a hole, a
//! checkpoint under no lease), and no lease row (no daemon has served
//! it). Anything else is refused (exit 1, the lead's decision 1): the
//! name is another volume's. A create that made the volume in object
//! storage and then failed leaves it, empty, for the next create of the
//! name.
//!
//! # At the start of `serve` ([`Start`], [`Leased`])
//!
//! The lease is taken under the volume's lock, after the store's open and
//! before a device is looked for (the lead's decision 3), within
//! [`LEASE_LIMIT`]:
//!
//! - **Taken** (made where there was no row, or this holder's): the
//!   uploader's thread starts once the device serves.
//! - **Another holder's**: the start is refused, exit 10, the device
//!   untouched. What the open wrote stays: its start token, and the
//!   region of the held cuts' state (`OpenOptions::hold_cuts`). With
//!   `--serve-without-upload` the volume is served with no uploader, and
//!   the status says "no lease: held by H at epoch E".
//! - **Object storage out of reach, or no answer within the limit**: the
//!   volume is served, and the uploader's thread takes the lease when it
//!   can (its backoff).
//!
//! The lease's time goes into no figure of the start's line: it is said
//! on a line of its own.
//!
//! # While it serves ([`Uploading`])
//!
//! A thread of its own, with a runtime of one thread, turns the
//! uploader. The store stays on the queue's thread: the uploader's calls
//! go through a desk (`skyzfs_upload::Desk`) that the queue's thread
//! answers after each turn. After each turn the uploader posts what the
//! status shows ([`shown`]).
//!
//! At the end the desk is closed and the thread stopped; an upload under
//! way is dropped, and the next start's check takes it up. At exit 0 the
//! lease is let go, within [`RELEASE_LIMIT`], so that another holder need
//! not wait out its time; at every other exit it is kept (decision 4).

use std::io;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use skynbd_engine::{keys, Engine, EngineConfig};
use skynbd_format::{Codec as _, Commit, CommitKind, Node, Superblock};
use skynbd_refs::{ActingRefStore, ObjectRefStore, RefStore};
use skynbd_store::{ObjectStore, S3Store};
use skyzfs_ublk::lifecycle::{LockError, OpenError, Volume};
use skyzfs_ublk::status::{
    CheckedShown, LeaseShown, RuleShown, Text, UploadFigures, UploadShown, UploadStateShown,
};
use skyzfs_upload::{
    Checked, Desk, Doing, EngineRemote, Figures, Keeper, LeaseState, Mismatch, Queued, Rule, Shown,
    State, Uploader,
};

use crate::remote::Remote;

/// The shard size of a volume `create` makes in object storage: 128 MiB
/// (decided for M2.4: a shard's tree holds at most a level of extents,
/// which 4 KiB blocks do not fill in shards of at most 256 MiB).
pub const SHARD_BYTES: u64 = 128 << 20;

/// How long the start waits for the first take of the lease (decision 3).
pub const LEASE_LIMIT: Duration = Duration::from_secs(10);

/// The lease's time unless `--lease-ttl-s` is given, in seconds.
pub const DEFAULT_LEASE_TTL_S: u64 = 30;

/// The shortest lease `--lease-ttl-s` takes: it is renewed every third of
/// its time, and a renewal is a read and a write in object storage.
pub const MIN_LEASE_TTL_S: u64 = 3;

/// The longest lease `--lease-ttl-s` takes: an hour, for which another
/// holder waits after a daemon that was killed.
pub const MAX_LEASE_TTL_S: u64 = 3600;

/// How long `create` waits for object storage to make or find the
/// volume. Not in the design: this module's reading, so that a create
/// against a store that does not answer ends, and says so.
pub const PLACE_LIMIT: Duration = Duration::from_secs(30);

/// How long exit 0 waits for the lease to be let go.
pub const RELEASE_LIMIT: Duration = Duration::from_secs(5);

/// How long any other end waits for the uploader's thread: it stops at
/// once unless a call outside the runtime holds it, and the process ends
/// after it all the same.
pub const END_LIMIT: Duration = Duration::from_secs(1);

/// The region the daemon names to an S3 client given keys: the one node
/// B and the rigs' MinIO use. A client of the ambient chain takes the
/// environment's or the profile's.
pub const REGION: &str = "us-east-1";

/// The objects of a volume's bucket.
pub type Objects = Arc<S3Store>;

/// The refs object over them.
pub type Refs = Arc<ObjectRefStore<Objects>>;

/// The engine's configuration for a volume of SkyZFS, writing under the
/// lease's `epoch` (0: no lease, at `create`).
pub fn engine_config(epoch: u64) -> EngineConfig {
    EngineConfig {
        shard_bytes: SHARD_BYTES,
        epoch,
        ..EngineConfig::default()
    }
}

/// A runtime of one thread, for `create` and for the uploader's thread.
pub fn runtime() -> io::Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
}

/// The bucket of `remote` and its refs. With fixed keys in the
/// environment ([`fixed_keys`]), the client signs with those keys in
/// [`REGION`], as node B does; otherwise it takes the AWS SDK's ambient
/// chain (decision 2), in the region `AWS_REGION` names, or [`REGION`]
/// where it names none (so that no region is looked for on a network the
/// SDK would ask: a profile's region is not read). The ambient chain
/// reads the environment's keys first, with their session token. The
/// keys are never on the command line. Nothing is asked of the store
/// here.
pub async fn connect(remote: &Remote) -> (Objects, Refs) {
    let store = match fixed_keys(|name| std::env::var(name).ok()) {
        Some((key, secret)) => {
            S3Store::new(
                &remote.endpoint,
                &remote.bucket,
                &key,
                &secret,
                REGION,
                true,
            )
            .await
        }
        None => {
            let region = std::env::var_os("AWS_REGION").is_none().then_some(REGION);
            S3Store::from_env(Some(&remote.endpoint), &remote.bucket, region, true).await
        }
    };
    let store = Arc::new(store);
    let refs = Arc::new(ObjectRefStore::new(Arc::clone(&store)));
    (store, refs)
}

/// The fixed keys the client signs with, read through `env`:
/// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, where both are set
/// and `AWS_SESSION_TOKEN` is unset or empty. Keys that come with a
/// token are temporary, and the client of fixed keys would sign without
/// the token, so every request would be refused: those go to the
/// ambient chain, which sends all three (Codex on #118).
pub fn fixed_keys(env: impl Fn(&str) -> Option<String>) -> Option<(String, String)> {
    if env("AWS_SESSION_TOKEN").is_some_and(|token| !token.is_empty()) {
        return None;
    }
    Some((env("AWS_ACCESS_KEY_ID")?, env("AWS_SECRET_ACCESS_KEY")?))
}

/// [`place`] in the bucket `remote` names, within [`PLACE_LIMIT`]:
/// what `create` calls.
pub fn place_within(remote: &Remote, size: u64) -> Result<Placed, String> {
    let runtime = runtime().map_err(|e| format!("a runtime for object storage: {e}"))?;
    runtime.block_on(async {
        let (objects, refs) = connect(remote).await;
        tokio::time::timeout(PLACE_LIMIT, place(objects, refs, remote, size))
            .await
            .unwrap_or_else(|_| {
                Err(format!(
                    "object storage did not answer within {} s",
                    PLACE_LIMIT.as_secs()
                ))
            })
    })
}

/// What [`place`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placed {
    /// It made the volume.
    Made,
    /// It found the volume as a create leaves it, and took it.
    Found,
}

impl Placed {
    /// In words, for `create`'s line.
    pub fn said(self) -> &'static str {
        match self {
            Placed::Made => "made",
            Placed::Found => "found empty",
        }
    }
}

/// Make the volume of `remote`, of `size` bytes, in `store` and `refs`,
/// or take it where a create left it: see the module's documentation.
/// The error says why it is refused.
pub async fn place<S, R>(store: S, refs: R, remote: &Remote, size: u64) -> Result<Placed, String>
where
    S: ObjectStore + Clone,
    R: RefStore + Clone,
{
    let vol = remote.id;
    let named = format!("the volume {} in object storage", remote.name);
    let lease = refs
        .lease_get(vol)
        .await
        .map_err(|e| format!("reading the lease of {named}: {e}"))?;
    if let Some(lease) = lease {
        return Err(format!(
            "{named} has been served: its lease names {} at epoch {}. The name is another \
             volume's",
            lease.holder, lease.epoch
        ));
    }
    let head = refs
        .get(vol, "head")
        .await
        .map_err(|e| format!("reading the head of {named}: {e}"))?;
    let Some(head) = head else {
        Engine::create_volume(store, refs, vol, size, engine_config(0))
            .await
            .map_err(|e| format!("making {named}: {e}"))?;
        return Ok(Placed::Made);
    };
    let read = |what: &'static str, e: &dyn std::fmt::Display| {
        format!("reading the {what} of {named}: {e}")
    };
    let block = store
        .get(&keys::superblock(vol, 0), None)
        .await
        .map_err(|e| read("superblock", &e))?;
    let superblock = Superblock::decode(&block).map_err(|e| read("superblock", &e))?;
    if superblock.size != size {
        return Err(format!(
            "{named} holds {} bytes, not {size}. The name is another volume's",
            superblock.size
        ));
    }
    if superblock.shard_bytes != SHARD_BYTES {
        return Err(format!(
            "{named} has shards of {} bytes, not {SHARD_BYTES}: it was not made by skyzfsd. The \
             name is another volume's",
            superblock.shard_bytes
        ));
    }
    let clone = store
        .exists(&keys::clone_base(vol))
        .await
        .map_err(|e| read("clone base", &e))?;
    if clone {
        return Err(format!(
            "{named} is a clone of another volume: what it reads is its base's. The name is \
             another volume's"
        ));
    }
    let object = store
        .get(&keys::commit(vol, &head.commit), None)
        .await
        .map_err(|e| read("head commit", &e))?;
    let commit = Commit::decode(&object).map_err(|e| read("head commit", &e))?;
    let empty = empty_index(size).map_err(|e| format!("the empty shard index: {e}"))?;
    if !as_created(&commit, vol, &empty) {
        return Err(format!(
            "{named} holds data: its head is not the empty commit a create makes. The name is \
             another volume's"
        ));
    }
    Ok(Placed::Found)
}

/// The hash of the shard index a create makes for a volume of `size`
/// bytes: every shard a hole, so that the volume reads as zeros.
fn empty_index(size: u64) -> Result<skynbd_types::NodeHash, skynbd_format::FormatError> {
    let shards = usize::try_from(size.div_ceil(SHARD_BYTES)).unwrap_or(usize::MAX);
    Node::ShardIndex(vec![None; shards]).content_hash()
}

/// Whether `commit` is the commit a create makes for `vol`, the time it
/// was made aside: no parent, segment or group, no flush, written under
/// no lease, a checkpoint, and the index `empty` ([`empty_index`]). A
/// commit with no parent, segment or group is not enough: a clone's
/// first commit is one, with its base's index (Codex on #118).
fn as_created(
    commit: &Commit,
    vol: skynbd_types::VolumeId,
    empty: &skynbd_types::NodeHash,
) -> bool {
    commit.vol_id == vol
        && commit.parent.is_none()
        && commit.segments.is_empty()
        && commit.group.is_none()
        && commit.flush_seq == 0
        && commit.epoch == 0
        && commit.kind == CommitKind::Checkpoint
        && commit.shard_index == *empty
}

/// What the start's take of the lease found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AtStart {
    /// Taken, at this epoch: the uploader's thread starts.
    Taken {
        /// The epoch.
        epoch: u64,
    },
    /// Another holder has it, and `--serve-without-upload` serves the
    /// volume with no uploader.
    Another {
        /// The holder.
        holder: String,
        /// Its epoch.
        epoch: u64,
    },
    /// Object storage could not be reached, or did not answer within the
    /// limit, and why: the volume is served, and the uploader's thread
    /// takes the lease when it can.
    Unreached(String),
}

impl AtStart {
    /// Whether the uploader's thread starts: there is a lease, or one may
    /// come.
    pub fn uploads(&self) -> bool {
        !matches!(self, AtStart::Another { .. })
    }

    /// The daemon's line about it.
    pub fn said(&self, spent: Duration) -> String {
        let ms = spent.as_millis();
        match self {
            AtStart::Taken { epoch } => format!("lease: held at epoch {epoch}, taken in {ms} ms"),
            AtStart::Another { holder, epoch } => format!(
                "lease: held by {holder} at epoch {epoch}: served with no upload \
                 (--serve-without-upload)"
            ),
            AtStart::Unreached(why) => format!(
                "lease: none after {ms} ms: {why}. Served, and the uploader takes the lease \
                 when it can"
            ),
        }
    }
}

/// The lease at the start: the runtime and the lease's keeper that the
/// uploader's thread takes over afterwards.
pub struct Start<R> {
    runtime: tokio::runtime::Runtime,
    keeper: Keeper<R>,
    serve_without_upload: bool,
    limit: Duration,
    found: Option<AtStart>,
    spent: Duration,
}

impl<R: RefStore> Start<R> {
    /// The lease kept by `keeper`, taken on `runtime` within
    /// [`LEASE_LIMIT`]. With `serve_without_upload`, another holder's
    /// lease does not refuse the volume.
    pub fn new(
        runtime: tokio::runtime::Runtime,
        keeper: Keeper<R>,
        serve_without_upload: bool,
    ) -> Self {
        Self {
            runtime,
            keeper,
            serve_without_upload,
            limit: LEASE_LIMIT,
            found: None,
            spent: Duration::ZERO,
        }
    }

    /// The same, with another limit, for the tests.
    pub fn with_limit(self, limit: Duration) -> Self {
        Self { limit, ..self }
    }

    /// Take the lease (the module's documentation). `Err` refuses the
    /// volume, with why: another holder has it, and the operator did not
    /// say `--serve-without-upload`.
    pub fn take(&mut self) -> Result<(), String> {
        let began = Instant::now();
        let (keeper, limit) = (&mut self.keeper, self.limit);
        let taken = self
            .runtime
            .block_on(async move { tokio::time::timeout(limit, keeper.keep()).await.cloned() });
        self.spent += began.elapsed();
        let found = match taken {
            Ok(LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. }) => {
                AtStart::Taken { epoch }
            }
            Ok(LeaseState::Lost { holder, epoch }) if !self.serve_without_upload => {
                return Err(format!(
                    "another holder has the volume's lease in object storage: {holder} at \
                     epoch {epoch}. Nothing was served and the volume has not failed. \
                     --serve-without-upload serves it with no upload"
                ));
            }
            Ok(LeaseState::Lost { holder, epoch }) => AtStart::Another { holder, epoch },
            Ok(LeaseState::None(why)) => {
                AtStart::Unreached(format!("object storage could not be reached: {why}"))
            }
            Err(_) => AtStart::Unreached(format!(
                "object storage did not answer within {} ms",
                limit.as_millis()
            )),
        };
        self.found = Some(found);
        Ok(())
    }

    /// What the take found, and how long it took: `None` before a take.
    pub fn found(&self) -> Option<(&AtStart, Duration)> {
        self.found.as_ref().map(|found| (found, self.spent))
    }

    /// What the status shows before the uploader's first turn.
    pub fn shown(&self) -> Option<UploadShown> {
        let shown = match self.found.as_ref()? {
            AtStart::Another { holder, epoch } => {
                return Some(UploadShown {
                    state: UploadStateShown::NoLease,
                    why: Text::new(&format!("held by {holder} at epoch {epoch}")),
                    holder: Text::new(holder),
                    epoch: *epoch,
                    ..UploadShown::default()
                })
            }
            AtStart::Taken { .. } => Shown {
                state: State::Idle,
                lease: self.keeper.state().clone(),
                figures: Figures::default(),
            },
            AtStart::Unreached(why) => Shown {
                state: State::NoLease(why.clone()),
                lease: LeaseState::None(why.clone()),
                figures: Figures::default(),
            },
        };
        Some(shown_of(&shown, self.keeper.holder()))
    }

    /// The runtime and the keeper, for the uploader's thread.
    pub fn into_parts(self) -> (tokio::runtime::Runtime, Keeper<R>) {
        (self.runtime, self.keeper)
    }
}

/// The uploads of a volume with a place in object storage, from the
/// serve's start to the uploader's thread: the client, and the lease
/// taken at the start.
pub struct Uploads {
    vol: skynbd_types::VolumeId,
    holder: String,
    objects: Objects,
    refs: Refs,
    start: Start<Refs>,
}

impl Uploads {
    /// The client of `remote`'s bucket, on a runtime of one thread, and
    /// the lease of `holder` for `ttl_s` seconds, not yet taken. With
    /// `serve_without_upload`, another holder's lease does not refuse the
    /// volume. Nothing is asked of object storage here.
    pub fn new(
        remote: &Remote,
        holder: String,
        ttl_s: u64,
        serve_without_upload: bool,
    ) -> io::Result<Self> {
        let runtime = runtime()?;
        let (objects, refs) = runtime.block_on(connect(remote));
        let keeper = Keeper::new(
            Arc::clone(&refs),
            remote.id,
            holder.clone(),
            ttl_s.saturating_mul(1000),
        );
        Ok(Self {
            vol: remote.id,
            holder,
            objects,
            refs,
            start: Start::new(runtime, keeper, serve_without_upload),
        })
    }

    /// The lease at the start.
    pub fn start(&mut self) -> &mut Start<Refs> {
        &mut self.start
    }

    /// The lease at the start, to read.
    pub fn started(&self) -> &Start<Refs> {
        &self.start
    }

    /// Start the uploader's thread over `desk`, posting with `publish`:
    /// see [`Uploading::start`]. Its remote writes as `holder` under the
    /// epoch the lease holds when its first turn makes it.
    ///
    /// `kill` is `serve --kill-at-upload`, which only a build with the
    /// test hooks takes ([`crate::kill`]); a default build has none.
    pub fn begin(
        self,
        desk: Arc<Desk>,
        publish: impl FnMut(UploadShown) + Send + 'static,
        kill: Option<crate::cli::KillAt>,
    ) -> io::Result<Uploading> {
        let (runtime, keeper) = self.start.into_parts();
        let (objects, refs, holder, vol) = (self.objects, self.refs, self.holder, self.vol);
        #[cfg(all(target_os = "linux", feature = "test-hooks"))]
        let make = crate::kill::maker(objects, refs, holder, vol, kill, crate::kill::sigkill());
        #[cfg(not(all(target_os = "linux", feature = "test-hooks")))]
        let make = {
            let _ = kill;
            maker(objects, refs, holder, vol)
        };
        Uploading::start(runtime, keeper, make, desk, publish)
    }
}

/// The remote the uploader makes at its first take, under the lease's
/// `epoch`: the engine over `objects`, writing as `holder` with every
/// commit fenced by that epoch.
pub fn maker<S, F>(
    objects: S,
    refs: F,
    holder: String,
    vol: skynbd_types::VolumeId,
) -> impl FnMut(u64) -> EngineRemote<S, Arc<ActingRefStore<F>>> + Send + 'static
where
    S: ObjectStore + Clone + 'static,
    F: RefStore + Clone + 'static,
{
    move |epoch| {
        let acting = ActingRefStore::new(refs.clone(), holder.clone(), epoch);
        EngineRemote::new(objects.clone(), Arc::new(acting), vol, engine_config(epoch))
    }
}

/// A volume whose open is followed by a take of the lease, under the
/// volume's lock: [`Volume::open`] succeeds only where `take` does. A
/// take that refuses gives the lock back, held by `share` from the open
/// store, which is dropped, with [`OpenError::Refused`]. A volume whose
/// open fails is not taken for.
pub struct Leased<'a, V: Volume, F> {
    inner: &'a mut V,
    share: fn(&V::Open) -> V::Lock,
    take: F,
}

impl<'a, V: Volume, F: FnMut() -> Result<(), String>> Leased<'a, V, F> {
    /// `inner`, whose open is followed by `take`.
    pub fn new(inner: &'a mut V, share: fn(&V::Open) -> V::Lock, take: F) -> Self {
        Self { inner, share, take }
    }
}

impl<V: Volume, F: FnMut() -> Result<(), String>> Volume for Leased<'_, V, F> {
    type Lock = V::Lock;
    type Open = V::Open;

    fn lock(&mut self) -> Result<V::Lock, LockError> {
        self.inner.lock()
    }

    fn open(
        &mut self,
        lock: V::Lock,
        acknowledge_failure: bool,
    ) -> Result<(V::Open, u64), (OpenError, V::Lock)> {
        let (open, size) = self.inner.open(lock, acknowledge_failure)?;
        match (self.take)() {
            Ok(()) => Ok((open, size)),
            Err(why) => {
                let lock = (self.share)(&open);
                drop(open);
                Err((OpenError::Refused(why), lock))
            }
        }
    }
}

/// What the status shows of the uploader as it is, whose lease is kept
/// as `holder`. The holder shown is the one the lease row names: this
/// daemon's while the lease is held, or unsure (as last held), and
/// another's when it is lost.
pub fn shown_of(shown: &Shown, holder: &str) -> UploadShown {
    let mut out = UploadShown::default();
    match &shown.lease {
        LeaseState::None(_) => out.lease = LeaseShown::None,
        LeaseState::Held {
            epoch,
            expires_at_ms,
        } => {
            out.lease = LeaseShown::Held;
            out.epoch = *epoch;
            out.expires_unix_ms = *expires_at_ms;
            out.holder = Text::new(holder);
        }
        LeaseState::Unsure { epoch, why } => {
            out.lease = LeaseShown::Unsure;
            out.epoch = *epoch;
            out.why = Text::new(&format!("the lease's last renewal failed: {why}"));
            out.holder = Text::new(holder);
        }
        LeaseState::Lost { holder, epoch } => {
            out.lease = LeaseShown::Lost;
            out.epoch = *epoch;
            out.holder = Text::new(holder);
        }
    }
    match &shown.state {
        State::NoLease(why) => {
            out.state = UploadStateShown::NoLease;
            out.why = Text::new(why);
        }
        State::LeaseLost { holder, epoch } => {
            out.state = UploadStateShown::LeaseLost;
            out.holder = Text::new(holder);
            out.epoch = *epoch;
        }
        State::Idle => out.state = UploadStateShown::Idle,
        State::Working(doing, cut) => {
            out.state = match doing {
                Doing::Checking => UploadStateShown::Checking,
                Doing::Reading => UploadStateShown::Reading,
                Doing::Syncing => UploadStateShown::Syncing,
                Doing::Committing => UploadStateShown::Committing,
                Doing::Stopped => UploadStateShown::Stopped,
            };
            out.cut = cut.unwrap_or(0);
        }
        State::Backoff { why, wait } => {
            out.state = UploadStateShown::Backoff;
            out.why = Text::new(why);
            out.wait_ms = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
        }
        State::Stopped => out.state = UploadStateShown::Stopped,
    }
    let f = &shown.figures;
    out.figures = UploadFigures {
        uploaded: f.uploaded,
        uploaded_whole: f.uploaded_whole,
        blocks: f.blocks,
        found_committed: f.found_committed,
        not_held: f.not_held,
        named: f.named,
        kept: f.kept,
        mismatches: f.mismatches,
        abandoned_at_mismatch: f.abandoned_at_mismatch,
        lost: f.lost,
        gave_up: f.gave_up,
        errors: f.errors,
    };
    out.checked = match f.checked {
        Checked::Unchecked => CheckedShown::Unchecked,
        Checked::Matched => CheckedShown::Matched,
        Checked::Mismatched(Mismatch::NoRow) => CheckedShown::NoRow,
        Checked::Mismatched(Mismatch::NotTheBase) => CheckedShown::NotTheBase,
        Checked::Mismatched(Mismatch::HeadMoved) => CheckedShown::HeadMoved,
        Checked::Mismatched(Mismatch::Unreadable) => CheckedShown::Unreadable,
    };
    out.rule = match f.last_rule {
        None => RuleShown::None,
        Some(Rule::Delta) => RuleShown::Delta,
        Some(Rule::Whole) => RuleShown::Whole,
    };
    if let Some(error) = &f.last_error {
        out.last_error = Text::new(&error.to_string());
    }
    out
}

/// How the uploader's thread ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ended {
    /// The lease was let go (exit 0).
    Released,
    /// The lease was kept, as at every other exit, or there was none to
    /// let go.
    Kept,
    /// The release failed, and why: the lease runs out in its time.
    NotReleased(String),
    /// The thread did not end within the limit, and was left to the
    /// process's end.
    NoAnswer,
}

/// The uploader's thread, while the volume is served.
pub struct Uploading {
    desk: Arc<Desk>,
    stop: tokio::sync::watch::Sender<Option<bool>>,
    ended: mpsc::Receiver<Ended>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Uploading {
    /// Start the thread: the uploader of the store behind `desk`, under
    /// the lease `keeper` keeps, uploading through what `make` makes for
    /// the epoch held, on `runtime`. `publish` is given what the status
    /// shows after each turn.
    pub fn start<F, M, R>(
        runtime: tokio::runtime::Runtime,
        keeper: Keeper<F>,
        make: M,
        desk: Arc<Desk>,
        mut publish: impl FnMut(UploadShown) + Send + 'static,
    ) -> io::Result<Self>
    where
        F: RefStore + Send + 'static,
        M: FnMut(u64) -> R + Send + 'static,
        R: skyzfs_upload::Remote + Send + 'static,
    {
        let (stop, mut stopped) = tokio::sync::watch::channel(None::<bool>);
        let (said, ended) = mpsc::channel();
        let local = Queued::new(Arc::clone(&desk));
        let holder = keeper.holder().to_owned();
        let thread = std::thread::Builder::new()
            .name("skyzfsd-upload".into())
            .spawn(move || {
                let mut uploader = Uploader::new(local, keeper, make);
                let end = runtime.block_on(async {
                    let asked = async {
                        let _ = stopped.wait_for(Option::is_some).await;
                    };
                    uploader
                        .run(asked, |s| publish(shown_of(&s, &holder)))
                        .await;
                    let release = stopped.borrow().unwrap_or(false);
                    if !release {
                        return Ended::Kept;
                    }
                    let held = uploader.lease().state().epoch().is_some();
                    match tokio::time::timeout(RELEASE_LIMIT, uploader.lease().release()).await {
                        Ok(Ok(())) if held => Ended::Released,
                        Ok(Ok(())) => Ended::Kept,
                        Ok(Err(why)) => Ended::NotReleased(why),
                        Err(_) => Ended::NotReleased(format!(
                            "no answer within {} s",
                            RELEASE_LIMIT.as_secs()
                        )),
                    }
                });
                let _ = said.send(end);
            })?;
        Ok(Self {
            desk,
            stop,
            ended,
            thread: Some(thread),
        })
    }

    /// End the thread: the desk closed (a call waiting is dropped), the
    /// upload under way dropped, and the lease let go if `release`.
    /// Waits at most `limit` for the thread.
    pub fn end(mut self, release: bool, limit: Duration) -> Ended {
        self.finish(release, limit)
    }

    fn finish(&mut self, release: bool, limit: Duration) -> Ended {
        let Some(thread) = self.thread.take() else {
            return Ended::Kept;
        };
        self.desk.close();
        let _ = self.stop.send(Some(release));
        match self.ended.recv_timeout(limit) {
            Ok(end) => {
                let _ = thread.join();
                end
            }
            Err(_) => Ended::NoAnswer,
        }
    }
}

/// An end that was not asked for (an early return, a panic): the lease
/// is kept.
impl Drop for Uploading {
    fn drop(&mut self) {
        self.finish(false, END_LIMIT);
    }
}

#[cfg(test)]
mod tests;

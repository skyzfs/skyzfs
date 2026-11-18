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
//! Where there is no head, the create first writes the superblock with
//! the condition that there is none ([`reserve`]), and goes on to make
//! the volume only where it wrote it or the one there is of its size
//! and shards. Of two creates of the name at once, of two sizes, one is
//! refused before it writes anything, and the superblock is the
//! volume's whose head was made (Codex on #118). Of one size, the one
//! that loses the head's swap looks at the name again and takes the
//! volume the other made, as any create that finds it (#118's review,
//! 2.2). A create that stopped
//! between its superblock and its head leaves the superblock: the next
//! create of that size goes on, and one of another size is refused, as
//! for a volume of that size, with the object to delete to free the name
//! in its message ([`refused_over`]). This module's reading, which the
//! lead accepted: the design does not say how two creates of a name
//! meet. A `create --replace` that would delete it is for later.
//!
//! # At the start of `serve` ([`Start`], [`Leased`])
//!
//! The lease is taken under the volume's lock, after the store's open and
//! before a device is looked for (the lead's decision 3), within
//! [`LEASE_LIMIT`]:
//!
//! - **Taken** (made where there was no row, or this holder's): the
//!   uploader's thread starts once the device serves.
//! - **Another holder's**: the start is refused, exit 10: no device is
//!   added or taken over (an acknowledging start has deleted the failed
//!   volume's waiting device before the open, as every acknowledging
//!   start does). What the open wrote stays: its start token, the region
//!   of the held cuts' state (`OpenOptions::hold_cuts`), and, where it
//!   acknowledged a failure the store's marker alone knew, the marker's
//!   removal. With
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
//! Between its turns it answers the daemon's channel ([`crate::control`],
//! M2.5-2) through an inbox ([`Publishes`]): the count of cuts, and the
//! publishes of cuts as snapshots, whose rule is
//! `skyzfs_upload::publish`'s. It is told the attachment the daemon
//! serves and the store's cut state before its first turn; a request of
//! another is refused. Each publish that ends is said on a line of the
//! daemon's log ([`publish_ended_said`]).
//!
//! At the end the desk is closed and the thread stopped; an upload under
//! way is dropped, and the next start's check takes it up. At exit 0 the
//! lease is let go, within [`RELEASE_LIMIT`], so that another holder need
//! not wait out its time; at every other exit it is kept (decision 4).

use std::io;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use skynbd_engine::{keys, Engine, EngineConfig, EngineError};
use skynbd_format::{Codec as _, Commit, CommitKind, Node, Superblock};
use skynbd_refs::{ActingRefStore, ObjectRefStore, RefStore};
use skynbd_store::{ObjectStore, Precondition, S3Store, StoreError};
use skyzfs_ublk::lifecycle::{LockError, OpenError, Volume};
use skyzfs_ublk::status::{
    CheckedShown, LeaseShown, RuleShown, Text, UploadFigures, UploadShown, UploadStateShown,
};
use skyzfs_upload::{
    Checked, Control, Desk, Doing, EngineRemote, Figures, Keeper, LeaseState, Mismatch, Progress,
    Queued, Rule, Shown, State, Uploader,
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

/// The region the daemon names to an S3 client where `AWS_REGION` names
/// none: the one node B and the rigs' MinIO use.
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

/// A runtime of one thread, for `create`, the lease at the start and the
/// uploader's thread.
pub fn runtime() -> io::Result<Runtime> {
    let inner = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    Ok(Runtime { inner: Some(inner) })
}

/// A runtime whose drop does not wait for the work left on its blocking
/// pool, where the HTTP client looks names up. A plain runtime's drop
/// waits for it, so a lookup that hangs would hold `create` past
/// [`PLACE_LIMIT`] (Codex on #118), a serve that ends after the lease's
/// take past its end, and the uploader's end past its limit (F2 of
/// #118's review). The work left ends with the process.
pub struct Runtime {
    /// `None` only in `drop`.
    inner: Option<tokio::runtime::Runtime>,
}

impl Runtime {
    /// Run `future` to its end on the runtime. The error is only for a
    /// runtime shut down, which nothing but its drop does.
    pub fn block_on<F: std::future::Future>(&self, future: F) -> io::Result<F::Output> {
        match &self.inner {
            Some(runtime) => Ok(runtime.block_on(future)),
            None => Err(io::Error::other("the runtime has been shut down")),
        }
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Some(runtime) = self.inner.take() {
            runtime.shutdown_background();
        }
    }
}

/// The bucket of `remote` and its refs. With fixed keys in the
/// environment ([`fixed_keys`]), the client signs with those keys;
/// otherwise it takes the AWS SDK's ambient chain (decision 2), which
/// reads the environment's keys first, with their session token. Either
/// signs in the region [`region`] gives: `AWS_REGION`'s, or [`REGION`]
/// where it names none (a profile's region is not read, so that no
/// region is looked for on a network the SDK would ask). The keys are
/// never on the command line. Nothing is asked of the store here.
pub async fn connect(remote: &Remote) -> (Objects, Refs) {
    let env = |name: &str| std::env::var(name).ok();
    let store = match fixed_keys(env) {
        Some(keys) => {
            S3Store::new(
                &remote.endpoint,
                &remote.bucket,
                &keys.key,
                &keys.secret,
                &keys.region,
                true,
            )
            .await
        }
        None => {
            let region = ambient_region(env);
            S3Store::from_env(Some(&remote.endpoint), &remote.bucket, region, true).await
        }
    };
    let store = Arc::new(store);
    let refs = Arc::new(ObjectRefStore::new(Arc::clone(&store)));
    (store, refs)
}

/// What a client of fixed keys signs with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FixedKeys {
    /// `AWS_ACCESS_KEY_ID`.
    pub key: String,
    /// `AWS_SECRET_ACCESS_KEY`.
    pub secret: String,
    /// The region, as [`region`] reads it.
    pub region: String,
}

/// The fixed keys the client signs with, read through `env`:
/// `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`, where both are set
/// and `AWS_SESSION_TOKEN` is unset or empty, and the region they sign
/// in. Keys that come with a token are temporary, and the client of
/// fixed keys would sign without the token, so every request would be
/// refused: those go to the ambient chain, which sends all three (Codex
/// on #118).
pub fn fixed_keys(env: impl Fn(&str) -> Option<String>) -> Option<FixedKeys> {
    if env("AWS_SESSION_TOKEN").is_some_and(|token| !token.is_empty()) {
        return None;
    }
    Some(FixedKeys {
        key: env("AWS_ACCESS_KEY_ID")?,
        secret: env("AWS_SECRET_ACCESS_KEY")?,
        region: region(&env),
    })
}

/// The region the ambient chain's client is given, read through `env`:
/// [`REGION`] where `AWS_REGION` is unset or empty, as with fixed keys
/// ([`region`]); else none, and the SDK reads `AWS_REGION` itself. An
/// empty one was left to the SDK (#118's review, 2.4).
pub fn ambient_region(env: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    env("AWS_REGION")
        .filter(|region| !region.is_empty())
        .is_none()
        .then_some(REGION)
}

/// The region a client of fixed keys signs in, read through `env`:
/// `AWS_REGION` where it is set and not empty, else [`REGION`] (F5 of
/// #118's review: a bucket in another region refused every request).
pub fn region(env: impl Fn(&str) -> Option<String>) -> String {
    env("AWS_REGION")
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| REGION.to_owned())
}

/// [`place`] in the bucket `remote` names, within [`PLACE_LIMIT`]:
/// what `create` calls. The making of the client counts in the limit
/// too, and what the work leaves on the runtime at the limit, a name
/// lookup that hangs, is not waited for ([`Runtime`]; Codex on #118).
pub fn place_within(remote: &Remote, size: u64) -> Result<Placed, String> {
    within(PLACE_LIMIT, async {
        let (objects, refs) = connect(remote).await;
        place(objects, refs, remote, size).await
    })
}

/// `work` on a runtime of its own, ended at `limit`, with no wait for
/// what it leaves on the runtime ([`Runtime`]).
fn within<T>(
    limit: Duration,
    work: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let runtime = runtime().map_err(|e| format!("a runtime for object storage: {e}"))?;
    runtime
        .block_on(async { tokio::time::timeout(limit, work).await })
        .map_err(|e| format!("a runtime for object storage: {e}"))?
        .unwrap_or_else(|_| {
            Err(format!(
                "object storage did not answer within {} s",
                limit.as_secs()
            ))
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
    unserved(&refs, vol, &named).await?;
    let head = refs
        .get(vol, "head")
        .await
        .map_err(|e| format!("reading the head of {named}: {e}"))?;
    let Some(head) = head else {
        let there = reserve(&store, vol, size)
            .await
            .map_err(|e| format!("making {named}: {e}"))?;
        if let Some(superblock) = there {
            if (superblock.size, superblock.shard_bytes) != (size, SHARD_BYTES) {
                return Err(refused_over(&refs, remote, &named, &superblock, size).await);
            }
        }
        match Engine::create_volume(store.clone(), refs.clone(), vol, size, engine_config(0)).await
        {
            Ok(_) => return Ok(Placed::Made),
            // Another create of the name, of this size and shards (the
            // superblock is of them), swapped its head in first (#118's
            // review, 2.2): what it made is looked at as any volume found
            // under the name, its lease row first.
            Err(EngineError::Conflict) => {
                unserved(&refs, vol, &named).await?;
                let head = refs
                    .get(vol, "head")
                    .await
                    .map_err(|e| format!("reading the head of {named}: {e}"))?;
                let Some(head) = head else {
                    return Err(format!("making {named}: {}", EngineError::Conflict));
                };
                return found(&store, vol, &named, &head, size).await;
            }
            Err(e) => return Err(format!("making {named}: {e}")),
        }
    };
    found(&store, vol, &named, &head, size).await
}

/// No lease row for `vol`, `named` in words: a volume that a daemon has
/// served is not one a create left.
async fn unserved<R: RefStore>(
    refs: &R,
    vol: skynbd_types::VolumeId,
    named: &str,
) -> Result<(), String> {
    let lease = refs
        .lease_get(vol)
        .await
        .map_err(|e| format!("reading the lease of {named}: {e}"))?;
    match lease {
        Some(lease) => Err(format!(
            "{named} has been served: its lease names {} at epoch {}. The name is another \
             volume's",
            lease.holder, lease.epoch
        )),
        None => Ok(()),
    }
}

/// `Found` where the volume `vol` (`named` in words), whose head is
/// `head`, is as a create of `size` bytes leaves it; else why it is
/// another volume's.
async fn found<S: ObjectStore>(
    store: &S,
    vol: skynbd_types::VolumeId,
    named: &str,
    head: &skynbd_refs::RefValue,
    size: u64,
) -> Result<Placed, String> {
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

/// Write the superblock of a create of `size` bytes where there is none,
/// and only there (Codex on #118). `Engine::create_volume` writes the
/// superblock unconditionally before its head's compare-and-swap: of two
/// creates of the name that both read no head, the one that loses the
/// swap would otherwise have written its superblock over the winner's. A
/// create goes on to the engine only where this write made the
/// superblock, or where the one there is of its own size and shards, so
/// that any superblock the engine writes again is the same but for its
/// time. `None` where this write made it; else the superblock there.
///
/// The conditional write is no new demand on the store: the refs' swaps
/// are conditional writes in it too.
async fn reserve<S: ObjectStore>(
    store: &S,
    vol: skynbd_types::VolumeId,
    size: u64,
) -> Result<Option<Superblock>, String> {
    let created_at_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        });
    let superblock = Superblock {
        vol_id: vol,
        size,
        format_major: 1,
        created_at_ms,
        shard_bytes: SHARD_BYTES,
    }
    .encode()
    .map_err(|e| format!("the superblock: {e}"))?;
    let key = keys::superblock(vol, 0);
    match store
        .put_if(&key, superblock.into(), Precondition::IfNoneMatch)
        .await
    {
        Ok(_) => Ok(None),
        Err(StoreError::PreconditionFailed(_)) => {
            let there = store
                .get(&key, None)
                .await
                .map_err(|e| format!("reading the superblock there: {e}"))?;
            Superblock::decode(&there)
                .map(Some)
                .map_err(|e| format!("reading the superblock there: {e}"))
        }
        Err(e) => Err(format!("writing the superblock where there is none: {e}")),
    }
}

/// Why a create of `size` bytes is refused over `there`, a superblock of
/// another size or shards that another create of the name wrote. The
/// head is read again: where that create has made it, the name is that
/// volume's. Where there is still none, that create is under way or
/// stopped before its head, and the message names the object whose
/// deletion frees the name (the lead's ask on #118; a `create
/// --replace` that would do it is for later).
async fn refused_over<R: RefStore>(
    refs: &R,
    remote: &Remote,
    named: &str,
    there: &Superblock,
    size: u64,
) -> String {
    let what = format!("{} bytes in shards of {}", there.size, there.shard_bytes);
    let asked = format!("{size} in shards of {SHARD_BYTES}");
    match refs.get(remote.id, "head").await {
        Ok(Some(_)) => format!(
            "{named} holds {what}, not {asked}: another create of the name made it first. The \
             name is another volume's"
        ),
        Ok(None) => {
            let same = if there.shard_bytes == SHARD_BYTES {
                format!("; a create with --size {} goes on with it", there.size)
            } else {
                String::new()
            };
            format!(
                "{named} has a superblock of {what}, not {asked}, and no head: another create of \
                 the name is under way, or stopped before its head. If none is under way, delete \
                 the object {} in the bucket {} to free the name{same}",
                keys::superblock(remote.id, 0),
                remote.bucket
            )
        }
        Err(e) => format!(
            "{named} has a superblock of {what}, not {asked}, from another create of the name, \
             and its head could not be read again: {e}"
        ),
    }
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
    runtime: Runtime,
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
    pub fn new(runtime: Runtime, keeper: Keeper<R>, serve_without_upload: bool) -> Self {
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
            Ok(Ok(LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. })) => {
                AtStart::Taken { epoch }
            }
            Ok(Ok(LeaseState::Lost { holder, epoch })) if !self.serve_without_upload => {
                return Err(format!(
                    "another holder has the volume's lease in object storage: {holder} at \
                     epoch {epoch}. Nothing was served, and this refusal did not fail the volume. \
                     --serve-without-upload serves it with no upload"
                ));
            }
            Ok(Ok(LeaseState::Lost { holder, epoch })) => AtStart::Another { holder, epoch },
            Ok(Ok(LeaseState::None(why))) => {
                AtStart::Unreached(format!("object storage could not be reached: {why}"))
            }
            Ok(Err(_)) => AtStart::Unreached(format!(
                "object storage did not answer within {} ms",
                limit.as_millis()
            )),
            Err(e) => AtStart::Unreached(format!("the uploader's runtime: {e}")),
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
                publishes: Default::default(),
            },
            AtStart::Unreached(why) => Shown {
                state: State::NoLease(why.clone()),
                lease: LeaseState::None(why.clone()),
                figures: Figures::default(),
                publishes: Default::default(),
            },
        };
        Some(shown_of(&shown, self.keeper.holder()))
    }

    /// The lease's time, in milliseconds.
    pub fn ttl_ms(&self) -> u64 {
        self.keeper.ttl_ms()
    }

    /// The runtime and the keeper, for the uploader's thread.
    pub fn into_parts(self) -> (Runtime, Keeper<R>) {
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
        let (objects, refs) = runtime.block_on(connect(remote))?;
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
        publishes: Publishes,
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
        Uploading::start(runtime, keeper, make, desk, publishes, publish)
    }
}

/// What the uploader's thread publishes from: the attachment the daemon
/// serves (its attachment record's nonce; `None` where the record could
/// not be read, and every publish is then refused), the store's cut
/// state at the start (`InPlace::cut_state_id`, `None` before its first
/// cut), and the channel's requests. An inbox whose sender is gone (the
/// channel could not be made) is no longer read.
pub struct Publishes {
    /// The attachment record's nonce.
    pub attachment: Option<[u8; 16]>,
    /// The store's cut state.
    pub state: Option<[u8; 16]>,
    /// The channel's requests.
    pub inbox: tokio::sync::mpsc::Receiver<Control>,
}

/// The daemon's line for the publish of `name` that ended as `progress`:
/// its answer in the channel's words.
pub fn publish_ended_said(name: &str, progress: &Progress) -> String {
    format!(
        "publish {name}: {}",
        crate::control::Reply::of(progress).line()
    )
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
        zero_blocks: Some(f.zero_blocks),
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
    /// the epoch held, on `runtime`, answering the channel's requests from
    /// `publishes`. `publish` is given what the status shows after each
    /// turn, and each publish that ends is said on standard error.
    pub fn start<F, M, R>(
        runtime: Runtime,
        keeper: Keeper<F>,
        make: M,
        desk: Arc<Desk>,
        publishes: Publishes,
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
        let posted = Arc::clone(&desk);
        let thread = std::thread::Builder::new()
            .name("skyzfsd-upload".into())
            .spawn(move || {
                let mut uploader = Uploader::new(local, keeper, make);
                if let Some(attachment) = publishes.attachment {
                    uploader.publishing().set_attachment(attachment);
                }
                if let Some(state) = publishes.state {
                    uploader.publishing().set_state(state);
                }
                let end = runtime.block_on(async {
                    let asked = async {
                        let _ = stopped.wait_for(Option::is_some).await;
                    };
                    // Once the desk is closed the turns' figures are not
                    // what happened (a cut counted lost, or committed and
                    // gone, where the daemon was only stopping): the
                    // status keeps the last figures from before the close.
                    let mut keeper = uploader
                        .run_with(asked, publishes.inbox, |s| {
                            for (name, progress) in &s.publishes.ended {
                                crate::to_stderr(&format!(
                                    "{}\n",
                                    publish_ended_said(name, progress)
                                ));
                            }
                            if !posted.closed() {
                                publish(shown_of(&s, &holder));
                            }
                        })
                        .await;
                    let release = stopped.borrow().unwrap_or(false);
                    if !release {
                        return Ended::Kept;
                    }
                    let held = keeper.state().epoch().is_some();
                    match tokio::time::timeout(RELEASE_LIMIT, keeper.release()).await {
                        Ok(Ok(())) if held => Ended::Released,
                        Ok(Ok(())) => Ended::Kept,
                        Ok(Err(why)) => Ended::NotReleased(why),
                        Err(_) => Ended::NotReleased(format!(
                            "no answer within {} s",
                            RELEASE_LIMIT.as_secs()
                        )),
                    }
                });
                let end = end
                    .unwrap_or_else(|e| Ended::NotReleased(format!("the uploader's runtime: {e}")));
                // Dropped before the answer: the drop does not wait for
                // the runtime's blocking pool ([`Runtime`]), so neither
                // does the join in `finish`, and `end`'s limit holds (F2
                // of #118's review).
                drop(runtime);
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
    /// Waits at most `limit` for the thread's answer; once it has
    /// answered, the thread ends without waiting for work left on its
    /// runtime's blocking pool.
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

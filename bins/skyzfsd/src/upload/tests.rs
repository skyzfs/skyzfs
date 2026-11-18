//! The daemon's side of the uploader, against a store in memory: the
//! volume `create` makes or takes, the lease at the start, what the
//! status shows, and the thread's end.

use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use bytes::Bytes;
use skynbd_engine::{keys, Engine, EngineConfig};
use skynbd_format::{Commit, CommitKind, Superblock};
use skynbd_refs::{ActingRefStore, LeaseOutcome, ObjectRefStore, RefStore};
use skynbd_store::{FaultOp, FaultStore, MemoryStore, ObjectStore, Result as StoreResult};
use skyzfs_blk::Buf;
use skyzfs_local::inplace::CutSlice;
use skyzfs_ublk::lifecycle::{LockError, OpenError, Volume};
use skyzfs_ublk::status::{CheckedShown, LeaseShown, RuleShown, UploadShown, UploadStateShown};
use skyzfs_upload::{
    Checked, Desk, Doing, EngineRemote, Figures, Keeper, LeaseState, Mismatch, Released,
    RemoteError, Rule, Shown, State, Store, Uploadable,
};

use super::*;
use crate::remote::Remote;

const SIZE: u64 = 64 << 20;
const TTL: u64 = 30_000;

fn remote() -> Remote {
    Remote::new("http://127.0.0.1:9000", "volumes", "pg-data").expect("a place")
}

type Mem = Arc<MemoryStore>;

/// A store in memory, its refs with a clock the test moves, and the
/// clock.
struct World {
    store: Mem,
    now: Arc<AtomicU64>,
}

impl World {
    fn new() -> Self {
        Self {
            store: Arc::new(MemoryStore::new()),
            now: Arc::new(AtomicU64::new(1_000_000)),
        }
    }

    fn refs(&self) -> Arc<ObjectRefStore<Mem>> {
        let now = Arc::clone(&self.now);
        Arc::new(
            ObjectRefStore::new(Arc::clone(&self.store))
                .with_clock(Arc::new(move || now.load(Ordering::SeqCst))),
        )
    }

    fn keeper(&self, holder: &str) -> Keeper<Arc<ObjectRefStore<Mem>>> {
        Keeper::new(self.refs(), remote().id, holder, TTL)
    }

    fn place(&self, size: u64) -> std::result::Result<Placed, String> {
        block_on(place(Arc::clone(&self.store), self.refs(), &remote(), size))
    }

    fn get(&self, key: &str) -> Bytes {
        block_on(self.store.get(key, None)).expect("there")
    }
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    runtime()
        .expect("a runtime")
        .block_on(future)
        .expect("the runtime runs")
}

/// `create`'s volume: the superblock of the size asked with shards of
/// 128 MiB, and head the empty commit; a second create of the name finds
/// it so and takes it.
#[test]
fn a_create_makes_the_volume_with_the_empty_commit_and_takes_it_again() {
    let world = World::new();
    assert_eq!(world.place(SIZE), Ok(Placed::Made));
    let vol = remote().id;
    let superblock = Superblock::decode(&world.get(&keys::superblock(vol, 0))).expect("decoded");
    assert_eq!(
        (superblock.size, superblock.shard_bytes),
        (SIZE, SHARD_BYTES)
    );
    let head = block_on(world.refs().get(vol, "head"))
        .expect("read")
        .expect("a head");
    let commit = Commit::decode(&world.get(&keys::commit(vol, &head.commit))).expect("decoded");
    assert!(commit.parent.is_none() && commit.segments.is_empty() && commit.group.is_none());
    assert_eq!(world.place(SIZE), Ok(Placed::Found));
    let again = block_on(world.refs().get(vol, "head"))
        .expect("read")
        .expect("a head");
    assert_eq!(again.commit, head.commit, "taken as it was");
}

/// A name whose volume holds data, is of another size, has shards of
/// another size or has been served is refused, and nothing is written.
#[test]
fn a_create_refuses_a_name_that_is_another_volumes() {
    let vol = remote().id;

    let world = World::new();
    world.place(SIZE).expect("made");
    let mut engine = block_on(Engine::open(
        Arc::clone(&world.store),
        world.refs(),
        vol,
        engine_config(0),
    ))
    .expect("opened");
    engine.write(0, &[7u8; 4096]).expect("written");
    block_on(engine.commit(CommitKind::Import)).expect("committed");
    let refused = world.place(SIZE).expect_err("holds data");
    assert!(refused.contains("holds data"), "{refused}");

    let world = World::new();
    world.place(SIZE).expect("made");
    let refused = world.place(2 * SIZE).expect_err("another size");
    assert!(
        refused.contains(&format!("holds {SIZE} bytes, not {}", 2 * SIZE)),
        "{refused}"
    );

    let world = World::new();
    block_on(Engine::create_volume(
        Arc::clone(&world.store),
        world.refs(),
        vol,
        SIZE,
        EngineConfig::default(),
    ))
    .expect("made with the engine's default shards");
    let refused = world.place(SIZE).expect_err("other shards");
    assert!(refused.contains("not made by skyzfsd"), "{refused}");

    let world = World::new();
    world.place(SIZE).expect("made");
    block_on(world.refs().lease_acquire(vol, "node-a", TTL)).expect("taken");
    let refused = world.place(SIZE).expect_err("served");
    assert!(
        refused.contains("has been served: its lease names node-a at epoch 1"),
        "{refused}"
    );

    // A served name is refused before anything is made: no head.
    let world = World::new();
    block_on(world.refs().lease_acquire(vol, "node-a", TTL)).expect("taken");
    world.place(SIZE).expect_err("served");
    assert_eq!(block_on(world.refs().get(vol, "head")).expect("read"), None);
}

/// Another volume `name`, of [`SIZE`] with skyzfsd's shards, holding one
/// block of data if `data`: its head's commit.
fn another(world: &World, name: &str, data: bool) -> (skynbd_types::VolumeId, Commit) {
    let vol = crate::remote::volume_id(name);
    let mut engine = block_on(Engine::create_volume(
        Arc::clone(&world.store),
        world.refs(),
        vol,
        SIZE,
        engine_config(0),
    ))
    .expect("made");
    if data {
        engine.write(0, &[7u8; 4096]).expect("written");
        block_on(engine.commit(CommitKind::Import)).expect("committed");
    }
    (vol, world.head(vol).1)
}

impl World {
    /// `vol`'s head and its commit.
    fn head(&self, vol: skynbd_types::VolumeId) -> (skynbd_refs::RefValue, Commit) {
        let head = block_on(self.refs().get(vol, "head"))
            .expect("read")
            .expect("a head");
        let commit = Commit::decode(&self.get(&keys::commit(vol, &head.commit))).expect("decoded");
        (head, commit)
    }
}

/// A clone under the name is refused though its first commit has no
/// parent, no segment and no group, as the empty commit has: its shard
/// index is its base's, and it reads through the base. So is a clone of
/// an empty base, whose index is the empty one: what it reads is the
/// base's. Nothing is written.
#[test]
fn a_create_refuses_a_clone_under_the_name() {
    let vol = remote().id;
    for data in [true, false] {
        let world = World::new();
        let (base, _) = another(&world, "base", data);
        let (head, _) = world.head(base);
        block_on(Engine::clone_volume(
            Arc::clone(&world.store),
            world.refs(),
            vol,
            SIZE,
            base,
            head.commit,
            engine_config(0),
        ))
        .expect("the clone");
        let (first, commit) = world.head(vol);
        assert!(
            commit.parent.is_none() && commit.segments.is_empty() && commit.group.is_none(),
            "the clone's first commit looks empty"
        );
        let refused = world.place(SIZE).expect_err("a clone");
        assert!(refused.contains("is a clone of"), "{data}: {refused}");
        assert_eq!(world.head(vol).0, first, "nothing written");
    }
}

/// A head that is not the commit a create makes is refused, one field
/// at a time: another kind, another volume's shard index, an epoch, a
/// count of flushes, another volume's commit, a parent, segments, a
/// group (the last three: the review's U16 to U18).
#[test]
fn a_create_refuses_a_head_that_only_looks_empty() {
    use skynbd_format::Codec;
    let vol = remote().id;
    let world = World::new();
    world.place(SIZE).expect("made");
    let (mut at, made) = world.head(vol);
    let (other, filled) = another(&world, "other", true);
    let cases = [
        (
            "kind",
            Commit {
                kind: CommitKind::Import,
                ..made.clone()
            },
        ),
        (
            "index",
            Commit {
                shard_index: filled.shard_index,
                ..made.clone()
            },
        ),
        (
            "epoch",
            Commit {
                epoch: 3,
                ..made.clone()
            },
        ),
        (
            "flushes",
            Commit {
                flush_seq: 1,
                ..made.clone()
            },
        ),
        (
            "volume",
            Commit {
                vol_id: other,
                ..made.clone()
            },
        ),
        (
            "parent",
            Commit {
                parent: filled.parent,
                ..made.clone()
            },
        ),
        (
            "segments",
            Commit {
                segments: filled.segments.clone(),
                ..made.clone()
            },
        ),
        (
            "group",
            Commit {
                group: Some(7),
                ..made.clone()
            },
        ),
    ];
    assert!(filled.parent.is_some() && !filled.segments.is_empty());
    let refs = world.refs();
    for (what, commit) in cases {
        let hash = commit.content_hash().expect("hashed");
        block_on(world.store.put(
            &keys::commit(vol, &hash),
            commit.encode().expect("encoded").into(),
        ))
        .expect("put");
        let swapped =
            block_on(refs.compare_and_swap(vol, "head", Some(at), Some(hash))).expect("swapped");
        let skynbd_refs::CasOutcome::Committed(Some(now)) = swapped else {
            panic!("{what}: not swapped: {swapped:?}");
        };
        at = now;
        let refused = world.place(SIZE).expect_err(what);
        assert!(refused.contains("holds data"), "{what}: {refused}");
    }
}

/// The store of a [`World`], holding the first write of `key` (a put or
/// a conditional put) until the test lets it go: it says it has come,
/// then waits.
struct Held {
    inner: Mem,
    key: String,
    gate: std::sync::Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
}

impl Held {
    fn wait(&self, key: &str) {
        if key != self.key {
            return;
        }
        let gate = self.gate.lock().expect("the gate").take();
        if let Some((came, go)) = gate {
            came.send(()).expect("the test waits");
            go.recv().expect("let go");
        }
    }
}

#[async_trait::async_trait]
impl ObjectStore for Held {
    async fn put(&self, key: &str, data: Bytes) -> StoreResult<()> {
        self.wait(key);
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> StoreResult<Bytes> {
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> StoreResult<bool> {
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> StoreResult<Vec<String>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> StoreResult<()> {
        self.inner.delete(key).await
    }
    async fn get_versioned(&self, key: &str) -> StoreResult<(Bytes, String)> {
        self.inner.get_versioned(key).await
    }
    async fn put_if(
        &self,
        key: &str,
        data: Bytes,
        pre: skynbd_store::Precondition,
    ) -> StoreResult<String> {
        self.wait(key);
        self.inner.put_if(key, data, pre).await
    }
}

/// A create of `size` bytes of the volume of [`remote`] on a thread of
/// its own, returned once it has read no head and is held at its first
/// write of the superblock; it goes on when the sender is used.
fn create_held_at_the_superblock(
    world: &World,
    size: u64,
) -> (
    std::thread::JoinHandle<Result<Placed, String>>,
    mpsc::Sender<()>,
) {
    let (came, arrived) = mpsc::channel();
    let (go, gone) = mpsc::channel();
    let held = Arc::new(Held {
        inner: Arc::clone(&world.store),
        key: keys::superblock(remote().id, 0),
        gate: std::sync::Mutex::new(Some((came, gone))),
    });
    let refs = world.refs();
    let other = std::thread::spawn(move || block_on(place(held, refs, &remote(), size)));
    arrived
        .recv_timeout(Duration::from_secs(10))
        .expect("the create held at the superblock");
    (other, go)
}

/// Two creates of the name at once (Codex on #118). The second has read
/// no head and is held at its first write of the superblock while the
/// first makes the volume. Of another size, the second is refused there
/// and the superblock stays the first's: the engine writes the
/// superblock unconditionally before its head's swap, so the create that
/// lost the swap used to write its size over the volume's. Of the same
/// size, the second goes on to the engine, loses the head's swap, and
/// looks at the volume again: it finds the first's as a create left it
/// (#118's review, 2.2: it said "head CAS conflict: lease lost").
/// Either way the next create of the first's size finds its volume.
#[test]
fn two_creates_of_the_name_at_once_leave_the_superblock_of_the_volume_made() {
    let vol = remote().id;
    for second in [2 * SIZE, SIZE] {
        let world = World::new();
        let (other, go) = create_held_at_the_superblock(&world, second);
        assert_eq!(world.place(SIZE), Ok(Placed::Made), "the first create");
        let head = block_on(world.refs().get(vol, "head"))
            .expect("read")
            .expect("a head");
        go.send(()).expect("the second create waits");
        let ended = other.join().expect("the second create ends");
        if second == SIZE {
            assert_eq!(ended, Ok(Placed::Found), "the second create");
        } else {
            let refused = ended.expect_err("the second create");
            assert!(
                refused.contains(&format!(
                    "holds {SIZE} bytes in shards of {SHARD_BYTES}, not {second} in shards of \
                     {SHARD_BYTES}: another create of the name made it first"
                )),
                "{refused}"
            );
        }
        let superblock =
            Superblock::decode(&world.get(&keys::superblock(vol, 0))).expect("decoded");
        assert_eq!(
            (superblock.size, superblock.shard_bytes),
            (SIZE, SHARD_BYTES),
            "the second create of {second} bytes"
        );
        assert_eq!(world.place(SIZE), Ok(Placed::Found));
        let again = block_on(world.refs().get(vol, "head"))
            .expect("read")
            .expect("a head");
        assert_eq!(again.commit, head.commit, "the first create's head");
    }
}

/// A create that loses the head's swap to another create of the name
/// and size looks at the volume as at one found under the name, its
/// lease row first (#118's review, 2.2): a volume whose lease a daemon
/// has taken by then, or that holds data by then, is refused, and its
/// head is left as it was.
#[test]
fn a_create_that_loses_the_heads_swap_refuses_a_volume_served_or_written_since() {
    let vol = remote().id;
    for served in [true, false] {
        let world = World::new();
        let (other, go) = create_held_at_the_superblock(&world, SIZE);
        assert_eq!(world.place(SIZE), Ok(Placed::Made), "the first create");
        if served {
            let taken = block_on(world.refs().lease_acquire(vol, "node-b", TTL));
            assert!(matches!(taken, Ok(LeaseOutcome::Acquired(_))), "{taken:?}");
        } else {
            let mut engine = block_on(Engine::open(
                Arc::clone(&world.store),
                world.refs(),
                vol,
                engine_config(0),
            ))
            .expect("opened");
            engine.write(0, &[7u8; 4096]).expect("written");
            block_on(engine.commit(CommitKind::Import)).expect("committed");
        }
        let head = block_on(world.refs().get(vol, "head"))
            .expect("read")
            .expect("a head");
        go.send(()).expect("the second create waits");
        let refused = other
            .join()
            .expect("the second create ends")
            .expect_err("the second create");
        let said = if served {
            "has been served: its lease names node-b at epoch 1"
        } else {
            "holds data"
        };
        assert!(refused.contains(said), "{refused}");
        let again = block_on(world.refs().get(vol, "head"))
            .expect("read")
            .expect("a head");
        assert_eq!(again.commit, head.commit, "the head as it was");
    }
}

/// A create that stopped between its superblock and its head (a write of
/// the engine's failed) leaves the superblock and no head. The next
/// create of its size and shards makes the volume; one of another size,
/// or over a superblock of other shards, is refused and nothing is
/// written. A superblock that cannot be written refuses the create, and
/// the engine is not called.
#[test]
fn a_create_that_stopped_before_its_head_counts_as_a_volume_of_its_size() {
    let vol = remote().id;
    let key = keys::superblock(vol, 0);
    // The superblock a create of `config`'s shards left when it stopped.
    let stopped = |config: EngineConfig| {
        let world = World::new();
        let failing = FaultStore::new(Arc::clone(&world.store));
        block_on(failing.arm(FaultOp::Put, "", 1));
        let made = block_on(Engine::create_volume(
            failing,
            world.refs(),
            vol,
            SIZE,
            config,
        ));
        assert!(made.is_err(), "stopped before its head");
        assert_eq!(block_on(world.refs().get(vol, "head")).expect("read"), None);
        let left = world.get(&key);
        (world, left)
    };

    let (world, left) = stopped(engine_config(0));
    let refused = world.place(2 * SIZE).expect_err("another size");
    assert!(
        refused.contains(&format!(
            "a superblock of {SIZE} bytes in shards of {SHARD_BYTES}, not {} in shards of \
             {SHARD_BYTES}, and no head",
            2 * SIZE
        )),
        "{refused}"
    );
    assert!(
        refused.contains(&format!(
            "delete the object {key} in the bucket volumes to free the name; a create with \
             --size {SIZE} goes on with it"
        )),
        "{refused}"
    );
    assert_eq!(world.get(&key), left, "the superblock as it was");
    assert_eq!(block_on(world.refs().get(vol, "head")).expect("read"), None);
    assert_eq!(world.place(SIZE), Ok(Placed::Made));
    assert_eq!(world.place(SIZE), Ok(Placed::Found));

    let (world, left) = stopped(EngineConfig::default());
    let refused = world.place(SIZE).expect_err("other shards");
    assert!(
        refused.contains(&format!(
            "not {SIZE} in shards of {SHARD_BYTES}, and no head"
        )),
        "{refused}"
    );
    assert!(
        refused.ends_with(&format!(
            "delete the object {key} in the bucket volumes to free the name"
        )),
        "no create of that superblock's shards goes on: {refused}"
    );
    assert_eq!(world.get(&key), left, "the superblock as it was");
    assert_eq!(block_on(world.refs().get(vol, "head")).expect("read"), None);

    let world = World::new();
    let failing = FaultStore::new(Arc::clone(&world.store));
    block_on(failing.arm(FaultOp::Put, &key, 0));
    let refused =
        block_on(place(failing, world.refs(), &remote(), SIZE)).expect_err("no superblock");
    assert!(
        refused
            .contains("writing the superblock where there is none: backend error: injected fault"),
        "{refused}"
    );
    assert!(!block_on(world.store.exists(&key)).expect("asked"));
    assert_eq!(block_on(world.refs().get(vol, "head")).expect("read"), None);
}

/// The keys of the environment sign only where both are set and no
/// session token is: temporary keys come with a token, which the client
/// of fixed keys would not send, so the ambient chain takes all three
/// (Codex on #118).
#[test]
fn fixed_keys_sign_only_with_no_session_token() {
    fn env<'a>(vars: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            vars.iter()
                .find(|(n, _)| *n == name)
                .map(|(_, v)| (*v).to_owned())
        }
    }
    let key = ("AWS_ACCESS_KEY_ID", "AKIA");
    let secret = ("AWS_SECRET_ACCESS_KEY", "s3cr3t");
    let keys = |region: &str| {
        Some(FixedKeys {
            key: "AKIA".into(),
            secret: "s3cr3t".into(),
            region: region.into(),
        })
    };
    assert_eq!(fixed_keys(env(&[key, secret])), keys(REGION));
    assert_eq!(
        fixed_keys(env(&[key, secret, ("AWS_SESSION_TOKEN", "")])),
        keys(REGION),
        "an empty token is none"
    );
    assert_eq!(
        fixed_keys(env(&[key, secret, ("AWS_REGION", "eu-west-1")])),
        keys("eu-west-1"),
        "the keys sign in AWS_REGION (F5 of #118's review)"
    );
    assert_eq!(
        fixed_keys(env(&[key, secret, ("AWS_SESSION_TOKEN", "FQoG")])),
        None,
        "temporary keys: the ambient chain"
    );
    assert_eq!(fixed_keys(env(&[key])), None, "no secret");
    assert_eq!(fixed_keys(env(&[secret])), None, "no key");
    assert_eq!(fixed_keys(env(&[])), None, "nothing");
}

/// A store that never answers.
#[derive(Clone)]
struct Stalled;

#[async_trait::async_trait]
impl ObjectStore for Stalled {
    async fn put(&self, _: &str, _: Bytes) -> StoreResult<()> {
        std::future::pending().await
    }
    async fn get(&self, _: &str, _: Option<Range<u64>>) -> StoreResult<Bytes> {
        std::future::pending().await
    }
    async fn exists(&self, _: &str) -> StoreResult<bool> {
        std::future::pending().await
    }
    async fn list(&self, _: &str) -> StoreResult<Vec<String>> {
        std::future::pending().await
    }
    async fn delete(&self, _: &str) -> StoreResult<()> {
        std::future::pending().await
    }
    async fn get_versioned(&self, _: &str) -> StoreResult<(Bytes, String)> {
        std::future::pending().await
    }
    async fn put_if(
        &self,
        _: &str,
        _: Bytes,
        _: skynbd_store::Precondition,
    ) -> StoreResult<String> {
        std::future::pending().await
    }
}

fn start<R: RefStore>(keeper: Keeper<R>, serve_without_upload: bool) -> Start<R> {
    Start::new(runtime().expect("a runtime"), keeper, serve_without_upload)
}

/// A lease nobody holds is taken at epoch 1, and the status shows it
/// held before the uploader's first turn.
#[test]
fn the_start_takes_a_lease_nobody_holds() {
    let world = World::new();
    let mut at = start(world.keeper("node-a"), false);
    assert_eq!(at.found(), None);
    assert_eq!(at.take(), Ok(()));
    let (found, _) = at.found().expect("taken");
    assert_eq!(found, &AtStart::Taken { epoch: 1 });
    assert!(found.uploads());
    let shown = at.shown().expect("shown");
    assert_eq!(
        (shown.state, shown.lease, shown.epoch),
        (UploadStateShown::Idle, LeaseShown::Held, 1)
    );
    assert_eq!(shown.expires_unix_ms, 1_000_000 + TTL);
    assert_eq!(shown.holder.said(), "node-a");
    // The keeper goes on to the thread holding it.
    let (_, keeper) = at.into_parts();
    assert_eq!(keeper.state().epoch(), Some(1));
}

/// Another holder's lease refuses the volume, naming the holder; with
/// `--serve-without-upload` the volume is served with no uploader, and
/// the status says the lease is held by that holder.
#[test]
fn another_holders_lease_refuses_the_start_unless_the_operator_says_otherwise() {
    let world = World::new();
    block_on(world.keeper("node-b").keep());

    let mut at = start(world.keeper("node-a"), false);
    let refused = at.take().expect_err("refused");
    assert!(
        refused.contains(
            "another holder has the volume's lease in object storage: node-b at \
             epoch 1"
        ),
        "{refused}"
    );
    assert!(refused.contains("--serve-without-upload"), "{refused}");
    assert_eq!(at.found(), None);

    let mut at = start(world.keeper("node-a"), true);
    assert_eq!(at.take(), Ok(()));
    let (found, _) = at.found().expect("found");
    assert_eq!(
        found,
        &AtStart::Another {
            holder: "node-b".into(),
            epoch: 1
        }
    );
    assert!(!found.uploads(), "no uploader");
    let shown = at.shown().expect("shown");
    assert_eq!(shown.said(), "no lease: held by node-b at epoch 1");
    assert_eq!(shown.lease, LeaseShown::None);
    // Nothing was taken: the row is node-b's still.
    let lease = block_on(world.refs().lease_get(remote().id))
        .expect("read")
        .expect("a row");
    assert_eq!((lease.holder.as_str(), lease.epoch), ("node-b", 1));
}

/// Object storage out of reach, or silent past the limit: the volume is
/// served, with no lease yet, and the status says why.
#[test]
fn a_store_out_of_reach_or_silent_serves_with_no_lease_yet() {
    let failing = FaultStore::new(MemoryStore::new());
    block_on(failing.arm(FaultOp::Any, "", 0));
    let keeper = Keeper::new(
        Arc::new(ObjectRefStore::new(failing)),
        remote().id,
        "node-a",
        TTL,
    );
    let mut at = start(keeper, false);
    assert_eq!(at.take(), Ok(()));
    let (found, _) = at.found().expect("found");
    assert!(
        matches!(found, AtStart::Unreached(why) if why.starts_with("object storage could not be reached")),
        "{found:?}"
    );
    assert!(found.uploads(), "the uploader takes it later");
    let shown = at.shown().expect("shown");
    assert_eq!(shown.state, UploadStateShown::NoLease);
    assert!(
        shown
            .said()
            .starts_with("no lease: object storage could not"),
        "{}",
        shown.said()
    );

    let keeper = Keeper::new(
        Arc::new(ObjectRefStore::new(Stalled)),
        remote().id,
        "node-a",
        TTL,
    );
    let mut at = start(keeper, false).with_limit(Duration::from_millis(50));
    assert_eq!(at.take(), Ok(()));
    let (found, spent) = at.found().expect("found");
    assert_eq!(
        found,
        &AtStart::Unreached("object storage did not answer within 50 ms".into())
    );
    assert!(spent >= Duration::from_millis(50), "{spent:?}");
    assert!(spent < LEASE_LIMIT, "{spent:?}");
}

/// The lock, the open store, and whether the open store was dropped.
#[derive(Debug, PartialEq, Eq)]
struct FakeLock(u32);

struct FakeOpen {
    lock: u32,
    dropped: Arc<AtomicU64>,
}

impl Drop for FakeOpen {
    fn drop(&mut self) {
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

struct FakeVolume {
    fails: bool,
    dropped: Arc<AtomicU64>,
}

impl Volume for FakeVolume {
    type Lock = FakeLock;
    type Open = FakeOpen;

    fn lock(&mut self) -> std::result::Result<FakeLock, LockError> {
        Ok(FakeLock(7))
    }

    fn open(
        &mut self,
        lock: FakeLock,
        _: bool,
    ) -> std::result::Result<(FakeOpen, u64), (OpenError, FakeLock)> {
        if self.fails {
            return Err((OpenError::Other("no".into()), lock));
        }
        Ok((
            FakeOpen {
                lock: lock.0,
                dropped: Arc::clone(&self.dropped),
            },
            SIZE,
        ))
    }
}

/// The take follows a successful open only. A take that refuses drops
/// the open store and hands the lock back with the refusal.
#[test]
fn the_lease_is_taken_after_the_open_and_a_refusal_gives_the_lock_back() {
    let share: fn(&FakeOpen) -> FakeLock = |open| FakeLock(open.lock);
    let dropped = Arc::new(AtomicU64::new(0));
    let takes = AtomicU64::new(0);

    let mut volume = FakeVolume {
        fails: false,
        dropped: Arc::clone(&dropped),
    };
    let mut leased = Leased::new(&mut volume, share, || {
        takes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let lock = leased.lock().expect("locked");
    let (open, size) = leased.open(lock, false).map_err(|e| e.0).expect("open");
    assert_eq!((open.lock, size), (7, SIZE));
    assert_eq!(takes.load(Ordering::SeqCst), 1);
    drop(open);

    let mut leased = Leased::new(&mut volume, share, || Err("leased".to_string()));
    let lock = leased.lock().expect("locked");
    let before = dropped.load(Ordering::SeqCst);
    match leased.open(lock, false) {
        Err((OpenError::Refused(why), lock)) => {
            assert_eq!((why.as_str(), lock), ("leased", FakeLock(7)));
        }
        Err((other, _)) => panic!("refused otherwise: {other:?}"),
        Ok(_) => panic!("opened"),
    }
    assert_eq!(
        dropped.load(Ordering::SeqCst),
        before + 1,
        "the open dropped"
    );

    let mut failing = FakeVolume {
        fails: true,
        dropped,
    };
    let takes = AtomicU64::new(0);
    let mut leased = Leased::new(&mut failing, share, || {
        takes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let lock = leased.lock().expect("locked");
    assert!(matches!(
        leased.open(lock, false),
        Err((OpenError::Other(_), _))
    ));
    assert_eq!(
        takes.load(Ordering::SeqCst),
        0,
        "no take after a failed open"
    );
}

/// Each state, lease and figure of the uploader as the status file
/// holds it.
#[test]
fn the_status_shows_the_uploader_as_it_is() {
    let shown = |state: State, lease: LeaseState, figures: Figures| {
        shown_of(
            &Shown {
                state,
                lease,
                figures,
                publishes: Default::default(),
            },
            "node-a",
        )
    };
    let held = LeaseState::Held {
        epoch: 4,
        expires_at_ms: 99,
    };

    let s = shown(
        State::NoLease("nope".into()),
        LeaseState::None("nope".into()),
        Figures::default(),
    );
    assert_eq!(
        (s.state, s.lease, s.said()),
        (
            UploadStateShown::NoLease,
            LeaseShown::None,
            "no lease: nope".to_string()
        )
    );
    assert_eq!(s.holder.said(), "", "no holder with no lease");

    let s = shown(
        State::LeaseLost {
            holder: "node-b".into(),
            epoch: 5,
        },
        LeaseState::Lost {
            holder: "node-b".into(),
            epoch: 5,
        },
        Figures::default(),
    );
    assert_eq!(
        (s.state, s.lease, s.epoch),
        (UploadStateShown::LeaseLost, LeaseShown::Lost, 5)
    );
    assert_eq!(s.said(), "lease lost to node-b at epoch 5");
    assert_eq!(s.holder.said(), "node-b", "the holder the row names");

    let s = shown(State::Idle, held.clone(), Figures::default());
    assert_eq!(
        (s.state, s.lease, s.epoch, s.expires_unix_ms),
        (UploadStateShown::Idle, LeaseShown::Held, 4, 99)
    );
    assert_eq!(
        s.holder.said(),
        "node-a",
        "held: this daemon (Codex on #118)"
    );

    for (doing, state) in [
        (Doing::Checking, UploadStateShown::Checking),
        (Doing::Reading, UploadStateShown::Reading),
        (Doing::Syncing, UploadStateShown::Syncing),
        (Doing::Committing, UploadStateShown::Committing),
        (Doing::Stopped, UploadStateShown::Stopped),
    ] {
        let s = shown(
            State::Working(doing, Some(12)),
            held.clone(),
            Figures::default(),
        );
        assert_eq!((s.state, s.cut), (state, 12), "{doing:?}");
    }
    let s = shown(
        State::Working(Doing::Checking, None),
        held.clone(),
        Figures::default(),
    );
    assert_eq!(s.cut, 0);

    let s = shown(
        State::Backoff {
            why: "commit of cut 3 refused: conflict".into(),
            wait: Duration::from_millis(1500),
        },
        held.clone(),
        Figures::default(),
    );
    assert_eq!(s.said(), "commit of cut 3 refused: conflict, retry in 2 s");
    assert_eq!(s.wait_ms, 1500);

    let s = shown(State::Stopped, held.clone(), Figures::default());
    assert_eq!(s.state, UploadStateShown::Stopped);

    let s = shown(
        State::Idle,
        LeaseState::Unsure {
            epoch: 4,
            why: "timed out".into(),
        },
        Figures::default(),
    );
    assert_eq!((s.lease, s.epoch), (LeaseShown::Unsure, 4));
    assert_eq!(
        s.holder.said(),
        "node-a",
        "unsure: this daemon, as last held"
    );
    assert_eq!(s.why.said(), "the lease's last renewal failed: timed out");

    let figures = Figures {
        uploaded: 1,
        uploaded_whole: 2,
        blocks: 3,
        found_committed: 4,
        // Not in the status until M2.5-2's version.
        not_offered: 0,
        not_held: 5,
        named: 6,
        kept: 7,
        mismatches: 8,
        abandoned_at_mismatch: 9,
        lost: 10,
        gave_up: 11,
        errors: 12,
        zero_blocks: 13,
        last_error: Some(RemoteError::Volume("gone".into())),
        checked: Checked::Mismatched(Mismatch::HeadMoved),
        last_rule: Some(Rule::Whole),
    };
    let s = shown(State::Idle, held.clone(), figures);
    let f = s.figures;
    assert_eq!(
        [
            f.uploaded,
            f.uploaded_whole,
            f.blocks,
            f.found_committed,
            f.not_held,
            f.named,
            f.kept,
            f.mismatches,
            f.abandoned_at_mismatch,
            f.lost,
            f.gave_up,
            f.errors
        ],
        [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12]
    );
    assert_eq!(f.zero_blocks, Some(13));
    assert_eq!(
        (s.checked, s.rule),
        (CheckedShown::HeadMoved, RuleShown::Whole)
    );
    assert!(
        s.last_error.said().contains("gone"),
        "{}",
        s.last_error.said()
    );
    for (mismatch, checked) in [
        (Mismatch::NoRow, CheckedShown::NoRow),
        (Mismatch::NotTheBase, CheckedShown::NotTheBase),
        (Mismatch::Unreadable, CheckedShown::Unreadable),
    ] {
        let figures = Figures {
            checked: Checked::Mismatched(mismatch),
            last_rule: Some(Rule::Delta),
            ..Figures::default()
        };
        let s = shown(State::Idle, held.clone(), figures);
        assert_eq!((s.checked, s.rule), (checked, RuleShown::Delta));
    }
    let s = shown(
        State::Idle,
        held,
        Figures {
            checked: Checked::Matched,
            ..Figures::default()
        },
    );
    assert_eq!(
        (s.checked, s.rule, s.last_error.said()),
        (CheckedShown::Matched, RuleShown::None, String::new())
    );
}

/// A store that holds no cut: what the queue's thread answers from when
/// nothing is held.
struct Empty;

impl Store for Empty {
    fn uploadable(&mut self) -> Option<Uploadable> {
        None
    }
    fn read(&mut self, _: u64, _: u64, _: &mut Buf) -> std::io::Result<CutSlice> {
        Err(std::io::Error::other("no cut"))
    }
    fn begin_sync_copies(&mut self, _: u64) -> std::io::Result<()> {
        Err(std::io::Error::other("no cut"))
    }
    fn copies_synced(&mut self, _: bool) -> Option<std::io::Result<()>> {
        None
    }
    fn release(&mut self, _: u64) -> Released {
        Released::Gone
    }
    fn name_committed(&mut self, _: u64, _: u64) -> bool {
        false
    }
    fn abandon_all(&mut self) -> Vec<u64> {
        Vec::new()
    }

    fn cuts(&mut self) -> skyzfs_upload::CutsNow {
        skyzfs_upload::CutsNow::default()
    }
}

/// The queue's thread: answers after each wake until the desk closes.
fn queue_thread(desk: Arc<Desk>, woken: mpsc::Receiver<()>) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut store = Empty;
        loop {
            let _ = woken.recv_timeout(Duration::from_millis(5));
            if desk.closed() {
                return;
            }
            desk.answer(&mut store);
        }
    })
}

/// What [`uploading`] gives: the uploader's thread, what it posts, the
/// queue's thread and the desk.
type Up = (
    Uploading,
    mpsc::Receiver<UploadShown>,
    std::thread::JoinHandle<()>,
    Arc<Desk>,
);

/// The uploader's thread over a volume made by `create`, with the lease
/// taken at the start: what it posts, its end, and its desk.
fn uploading(world: &World) -> Up {
    uploading_with(world, true, |_| {})
}

/// [`uploading`], with the lease taken at the start only if `take`, and
/// `before` given the runtime before the thread takes it over.
fn uploading_with(world: &World, take: bool, before: impl FnOnce(&Runtime)) -> Up {
    let (_, inbox) = tokio::sync::mpsc::channel(1);
    let publishes = Publishes {
        attachment: None,
        state: None,
        inbox,
    };
    uploading_from(world, take, before, publishes)
}

/// [`uploading_with`], publishing from `publishes`.
fn uploading_from(
    world: &World,
    take: bool,
    before: impl FnOnce(&Runtime),
    publishes: Publishes,
) -> Up {
    world.place(SIZE).expect("made");
    let mut at = start(world.keeper("node-a"), false);
    if take {
        at.take().expect("taken");
    }
    let (runtime, keeper) = at.into_parts();
    before(&runtime);
    let (wake, woken) = mpsc::channel();
    let desk = Desk::new(move || {
        let _ = wake.send(());
    });
    let queue = queue_thread(Arc::clone(&desk), woken);
    let asked = Arc::clone(&desk);
    let make = maker(
        Arc::clone(&world.store),
        world.refs(),
        "node-a".into(),
        remote().id,
    );
    let (posted, posts) = mpsc::channel();
    let up = Uploading::start(runtime, keeper, make, desk, publishes, move |shown| {
        let _ = posted.send(shown);
    })
    .expect("started");
    (up, posts, queue, asked)
}

/// The first post that is idle with the lease held.
fn idle_and_held(posts: &mpsc::Receiver<UploadShown>) -> UploadShown {
    loop {
        let shown = posts
            .recv_timeout(Duration::from_secs(10))
            .expect("a post within 10 s");
        if shown.state == UploadStateShown::Idle && shown.lease == LeaseShown::Held {
            return shown;
        }
    }
}

/// At exit 0 the lease is let go: its row says it expired, the holder and
/// epoch kept, and the queue's thread ends with the desk closed.
#[test]
fn the_end_at_exit_0_lets_the_lease_go() {
    let world = World::new();
    let (up, posts, queue, _) = uploading(&world);
    let shown = idle_and_held(&posts);
    assert_eq!(shown.epoch, 1);
    assert_eq!(
        shown.holder.said(),
        "node-a",
        "the thread's posts name the holder"
    );
    assert_eq!(up.end(true, RELEASE_LIMIT), Ended::Released);
    queue.join().expect("the queue's thread ended at the close");
    let lease = block_on(world.refs().lease_get(remote().id))
        .expect("read")
        .expect("a row");
    assert_eq!((lease.holder.as_str(), lease.epoch), ("node-a", 1));
    assert_eq!(lease.expires_at_ms, 0, "let go");
}

/// At any other end the lease is kept, asked for or not (an early return
/// drops the thread's handle).
#[test]
fn any_other_end_keeps_the_lease() {
    for asked in [true, false] {
        let world = World::new();
        let (up, posts, queue, _) = uploading(&world);
        idle_and_held(&posts);
        if asked {
            assert_eq!(up.end(false, END_LIMIT), Ended::Kept);
        } else {
            drop(up);
        }
        queue.join().expect("the queue's thread ended at the close");
        let lease = block_on(world.refs().lease_get(remote().id))
            .expect("read")
            .expect("a row");
        assert_eq!(lease.holder, "node-a");
        assert!(lease.expires_at_ms > 0, "kept, asked: {asked}");
    }
}

/// Once the desk is closed the thread posts nothing more (N5 of #117's
/// review): the uploader still turns until it is told to stop, and its
/// figures after the close (a cut counted lost, or committed and gone)
/// are not what happened. An idle uploader turns four times a second, so
/// without the rule two seconds hold about eight posts; one may be a
/// turn that read the desk open just before the close.
#[test]
fn nothing_is_posted_once_the_desk_is_closed() {
    let world = World::new();
    let (up, posts, queue, desk) = uploading(&world);
    idle_and_held(&posts);
    desk.close();
    queue.join().expect("the queue's thread ended at the close");
    let before: Vec<UploadShown> = posts.try_iter().collect();
    std::thread::sleep(Duration::from_secs(2));
    let after: Vec<UploadShown> = posts.try_iter().collect();
    assert!(
        after.len() <= 1,
        "{} posts after the close, {} before",
        after.len(),
        before.len()
    );
    assert_eq!(up.end(false, END_LIMIT), Ended::Kept);
}

/// The end's limit holds with work left on the runtime's blocking pool,
/// as a name lookup of the HTTP client is (F2 of #118's review): the
/// runtime's drop waited for it, and `end` waited for the drop.
#[test]
fn the_end_does_not_wait_for_work_left_on_the_runtime() {
    let world = World::new();
    let (began, begun) = mpsc::channel();
    let (up, posts, queue, _) = uploading_with(&world, true, move |runtime| {
        leave_blocking_work(runtime, began);
    });
    begun
        .recv_timeout(Duration::from_secs(10))
        .expect("the blocking work began");
    idle_and_held(&posts);
    let at = std::time::Instant::now();
    assert_eq!(up.end(true, Duration::from_secs(2)), Ended::Released);
    assert!(
        at.elapsed() < Duration::from_secs(2),
        "the end took {:?}",
        at.elapsed()
    );
    queue.join().expect("the queue's thread ended at the close");
}

/// Leave 5 s of blocking work on `runtime`, as a name lookup of the
/// HTTP client that hangs, and say when it has begun.
fn leave_blocking_work(runtime: &Runtime, began: mpsc::Sender<()>) {
    runtime
        .block_on(async move {
            drop(tokio::task::spawn_blocking(move || {
                let _ = began.send(());
                std::thread::sleep(Duration::from_secs(5));
            }));
        })
        .expect("the runtime runs");
}

/// The daemon's runtime is dropped without waiting for the work left on
/// its blocking pool (Codex on #118): a plain runtime's drop waits for
/// it, and with it `create`, a serve that ends after the lease's take,
/// and the uploader's end.
#[test]
fn a_runtime_is_dropped_without_waiting_for_its_blocking_work() {
    let runtime = runtime().expect("a runtime");
    let (began, begun) = mpsc::channel();
    leave_blocking_work(&runtime, began);
    begun
        .recv_timeout(Duration::from_secs(10))
        .expect("the blocking work began");
    let at = std::time::Instant::now();
    drop(runtime);
    assert!(
        at.elapsed() < Duration::from_secs(2),
        "the drop took {:?}",
        at.elapsed()
    );
}

/// `create`'s work in object storage ends at its limit, even where it
/// leaves blocking work on its runtime, as a name lookup that hangs
/// (Codex on #118): the limit's refusal is said, at the limit.
#[test]
fn work_in_object_storage_ends_at_its_limit_with_blocking_work_left() {
    let at = std::time::Instant::now();
    let ended = within(Duration::from_secs(1), async {
        let (began, begun) = tokio::sync::oneshot::channel();
        drop(tokio::task::spawn_blocking(move || {
            let _ = began.send(());
            std::thread::sleep(Duration::from_secs(5));
        }));
        let _ = begun.await;
        std::future::pending::<Result<(), String>>().await
    });
    assert_eq!(
        ended,
        Err("object storage did not answer within 1 s".to_owned())
    );
    assert!(
        at.elapsed() < Duration::from_secs(3),
        "the work ended after {:?}",
        at.elapsed()
    );
}

/// An end asked to let the lease go, where this daemon holds none (the
/// lease is another holder's), says it was kept, and the other holder's
/// row is left as it was.
#[test]
fn an_end_with_no_lease_held_says_it_was_kept() {
    let world = World::new();
    let (up, posts, queue, _) = uploading_with(&world, false, |_| {
        let taken = block_on(world.refs().lease_acquire(remote().id, "node-b", TTL));
        assert!(matches!(taken, Ok(LeaseOutcome::Acquired(_))), "{taken:?}");
    });
    loop {
        let shown = posts
            .recv_timeout(Duration::from_secs(10))
            .expect("a post within 10 s");
        if shown.lease == LeaseShown::Lost {
            assert_eq!(shown.holder.said(), "node-b");
            break;
        }
    }
    assert_eq!(up.end(true, RELEASE_LIMIT), Ended::Kept);
    queue.join().expect("the queue's thread ended at the close");
    let lease = block_on(world.refs().lease_get(remote().id))
        .expect("read")
        .expect("a row");
    assert_eq!(lease.holder, "node-b");
    assert!(lease.expires_at_ms > 0, "not let go");
}

/// The remote the daemon makes for the uploader ([`maker`]) commits
/// under the epoch it is made for: its commit carries that epoch, and
/// one made for another epoch than the lease's is fenced, and nothing
/// lands (G2 of #118's review: `maker` was reached only in the build
/// with the test hooks).
#[test]
fn the_daemons_remote_commits_under_the_epoch_it_is_made_for() {
    use skyzfs_upload::Remote as _;
    let world = World::new();
    world.place(SIZE).expect("made");
    let vol = remote().id;
    let taken = block_on(world.refs().lease_acquire(vol, "node-a", TTL)).expect("the take");
    let LeaseOutcome::Acquired(lease) = taken else {
        panic!("not taken: {taken:?}");
    };
    let mut make = maker(Arc::clone(&world.store), world.refs(), "node-a".into(), vol);
    let commit = |up: &mut EngineRemote<Mem, Arc<ActingRefStore<Arc<ObjectRefStore<Mem>>>>>| {
        block_on(async {
            up.observe().await?;
            up.write(0, &[7; 4096])?;
            up.commit(7, None, b"row 7".to_vec()).await
        })
    };
    let (before, _) = world.head(vol);
    for epoch in [lease.epoch + 1, lease.epoch - 1] {
        let refused = commit(&mut make(epoch));
        assert_eq!(
            refused,
            Err(RemoteError::Fenced {
                holder: "node-a".into(),
                epoch: lease.epoch
            }),
            "made for epoch {epoch}"
        );
        assert_eq!(world.head(vol).0, before, "nothing landed");
    }
    commit(&mut make(lease.epoch)).expect("committed under the lease's epoch");
    let (after, landed) = world.head(vol);
    assert_ne!(after, before);
    assert_eq!(
        landed.epoch, lease.epoch,
        "the commit carries the lease's epoch"
    );
}

/// The region named to an S3 client with fixed keys: `AWS_REGION`, or
/// us-east-1 where it is unset or empty (F5 of #118's review); the
/// ambient chain's client is given us-east-1 there too, and no region
/// where `AWS_REGION` names one (2.4 of its next look).
#[test]
fn the_region_is_aws_region_or_the_default() {
    let unset = |_: &str| None;
    assert_eq!(region(unset), REGION);
    assert_eq!(REGION, "us-east-1");
    let empty = |name: &str| (name == "AWS_REGION").then(String::new);
    assert_eq!(region(empty), REGION);
    let set = |name: &str| (name == "AWS_REGION").then(|| "eu-west-1".to_owned());
    assert_eq!(region(set), "eu-west-1");
    // The ambient chain's client is given the default where fixed keys
    // sign in it, and no region where the SDK reads `AWS_REGION` itself
    // (#118's review, 2.4).
    assert_eq!(ambient_region(unset), Some(REGION));
    assert_eq!(ambient_region(empty), Some(REGION));
    assert_eq!(ambient_region(set), None);
}

/// The uploader's thread answers the channel's requests from its inbox,
/// for the attachment and the cut state it was given: a publish of
/// another attachment is refused, one of this attachment and state is
/// taken.
///
/// Mutant: the attachment or the state not given to the uploader (the
/// first publish is refused), or the inbox not read (no answer).
#[test]
fn the_thread_answers_the_channel_for_its_attachment_and_state() {
    use skyzfs_upload::publish::{Request as Ask, EXPIRY_DEFAULT};
    use skyzfs_upload::{Answer, Control, Progress, Refusal};
    let world = World::new();
    let (requests, inbox) = tokio::sync::mpsc::channel(4);
    let publishes = Publishes {
        attachment: Some([0xa1; 16]),
        state: Some([0x5e; 16]),
        inbox,
    };
    let (up, posts, queue, _) = uploading_from(&world, true, |_| {}, publishes);
    idle_and_held(&posts);
    // An answer within 10 s, or none: an inbox that is not read fails
    // the test rather than holding it.
    fn within<T>(mut answer: tokio::sync::oneshot::Receiver<T>) -> Option<T> {
        for _ in 0..1000 {
            if let Ok(got) = answer.try_recv() {
                return Some(got);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        None
    }
    let (reply, answer) = tokio::sync::oneshot::channel();
    requests.blocking_send(Control::Cuts(reply)).expect("sent");
    let now = within(answer).expect("the count");
    assert_eq!(now, skyzfs_upload::CutsNow::default());
    let ask = |attachment| Ask {
        name: "t".into(),
        attachment,
        state: [0x5e; 16],
        after: 0,
        group: 1,
        manifest: Vec::new(),
        expiry: EXPIRY_DEFAULT,
    };
    let (reply, answer) = tokio::sync::oneshot::channel();
    requests
        .blocking_send(Control::Ask(ask([0xa1; 16]), reply))
        .expect("sent");
    assert_eq!(within(answer), Some(Answer::Accepted));
    let (reply, answer) = tokio::sync::oneshot::channel();
    let mut other = ask([0xa2; 16]);
    other.name = "u".into();
    requests.blocking_send(Control::Ask(other, reply)).expect("sent");
    assert_eq!(
        within(answer),
        Some(Answer::Refused(Refusal::Attachment))
    );
    let (reply, answer) = tokio::sync::oneshot::channel();
    requests
        .blocking_send(Control::Progress("t".into(), reply))
        .expect("sent");
    assert_eq!(within(answer), Some(Progress::Pending));
    assert_eq!(up.end(false, END_LIMIT), Ended::Kept);
    queue.join().expect("the queue's thread");
}

//! Review of #118 (M2.4c-2): scenarios the head's tests lack, as run at
//! 4c733d3 (the third look).
//!
//! Not part of the pull request. Run as an integration test of `skyzfsd`
//! in a scratch worktree (`scenarios.py` copies it to
//! `bins/skyzfsd/tests/review_m24c2.rs`); the fixture is the upload
//! crate's own (`crates/skyzfs-upload/tests/common/mod.rs`).
//!
//! Two groups:
//!
//! - **D1 to D7: the daemon's queue loop with the desk in it.** `serve`'s
//!   own `pumped`, a pump over the in-place store on the simulated disk,
//!   a queue of the test's that waits as the kernel's does (a wake, a
//!   request, a stop, or 20 s of nothing), the each-turn closure with
//!   the desk's two lines as `serve.rs` has them (1117 to 1126), and the
//!   daemon's `Start`, `maker` and `Uploading` over object storage in
//!   memory. What is not the daemon's: the closure is a copy, since
//!   `run` cannot be called without a kernel.
//! - **S1 to S6: the start, the status and `create`.** S1b needs the
//!   harness of the first group.
//!
//! A scenario named `defect_...` states what should hold and fails on
//! the head.

#[path = "../../../crates/skyzfs-upload/tests/common/mod.rs"]
mod common;

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use skynbd_engine::Engine;
use skynbd_format::{CloneBase, Codec as _, CommitKind, Superblock};
use skynbd_refs::{ObjectRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Precondition, Result as StoreResult};
use skyzfs_blk::Buf;
use skyzfs_local::fs::FsDir;
use skyzfs_local::inplace::{
    lock, CutRegion, HoldCuts, InPlace, OpenOptions, Opened, VolumeConfig,
};
use skyzfs_local::sim::SimDisk;
use skyzfs_local::store::Waker;
use skyzfs_uberblock::{seal, ByteOrder, Labels, MAGIC};
use skyzfs_ublk::lifecycle::{OpenError, Volume};
use skyzfs_ublk::pump::{Fetched, Pump, Queue, QueueDown, Stopped, Unfetched, Wake};
use skyzfs_ublk::status::{LeaseShown, UploadShown, UploadStateShown};
use skyzfs_ublk::translate::{op, Desc};
use skyzfs_ublk::volume::StoreVolume;
use skyzfs_upload::{Desk, Keeper, LeaseState, Shown, State};

use skyzfsd::remote::Remote;
use skyzfsd::serve::pumped;
use skyzfsd::upload::{
    engine_config, maker, place, runtime, shown_of, AtStart, Ended, Leased, Placed, Start,
    Uploading, END_LIMIT, RELEASE_LIMIT, SHARD_BYTES,
};

use common::*;

const TTL: u64 = 30_000;
/// The kernel queue's idle time-out (`flush_and_wake_io_tasks`, 20 s): a
/// turn nobody asked for comes only then.
const IDLE: Duration = Duration::from_secs(20);
const SLOT: u64 = 4 * KIB;
const RING_SLOTS: u64 = skyzfs_uberblock::RING_SIZE / SLOT;

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    runtime()
        .expect("a runtime")
        .block_on(future)
        .expect("the runtime runs")
}

// ---------------------------------------------------------------- queue

enum Msg {
    Fetch(Fetched, Option<Buf>),
    Wake,
    Stop,
    Panic,
}

/// A queue that waits as the kernel's does: for a request, a wake, a
/// stop, or the idle time-out.
struct ChanQueue {
    rx: mpsc::Receiver<Msg>,
    bufs: HashMap<u16, Buf>,
    done: mpsc::Sender<(u16, i32)>,
    idle: Duration,
}

impl Queue for ChanQueue {
    fn depth(&self) -> u16 {
        64
    }

    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        _unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        match self.rx.recv_timeout(self.idle) {
            Ok(Msg::Fetch(request, buf)) => {
                if let Some(buf) = buf {
                    self.bufs.insert(request.tag, buf);
                }
                fetched.push(request);
                Ok(Wake::Work)
            }
            Ok(Msg::Wake) => Ok(Wake::Work),
            Ok(Msg::Stop) => Ok(Wake::Stop),
            Ok(Msg::Panic) => panic!("the queue's thread dies in a turn (a scenario's)"),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(Wake::Work),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(QueueDown),
        }
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        Some(self.bufs.remove(&tag).unwrap_or_else(|| Buf::zeroed(blocks)))
    }

    fn commit(&mut self, tag: u16, result: i32, _buf: Option<Buf>) {
        let _ = self.done.send((tag, result));
    }

    fn flush(&mut self, _unfetched: &mut Vec<Unfetched>) -> Result<(), QueueDown> {
        Ok(())
    }
}

// ------------------------------------------------------- object storage

/// Object storage whose calls can be made slow, and which can tell the
/// test when a swap of the refs object has landed and hold its answer.
#[derive(Clone)]
struct Slow<S> {
    inner: S,
    put_ms: Arc<AtomicU64>,
    get_ms: Arc<AtomicU64>,
    /// Armed: the next conditional PUT of the refs object is told to the
    /// test once it has landed, and answered only after `hold_ms`.
    armed: Arc<AtomicBool>,
    landed: Arc<Mutex<Option<mpsc::Sender<()>>>>,
    hold_ms: Arc<AtomicU64>,
    /// Not zero: the next read of the refs object leaves a blocking task
    /// of this many ms on the runtime, as a name lookup does that is slow
    /// to answer (the HTTP client resolves names on the blocking pool).
    block_ms: Arc<AtomicU64>,
}

impl<S> Slow<S> {
    fn new(inner: S) -> Self {
        Self {
            inner,
            put_ms: Arc::new(AtomicU64::new(0)),
            get_ms: Arc::new(AtomicU64::new(0)),
            armed: Arc::new(AtomicBool::new(false)),
            landed: Arc::new(Mutex::new(None)),
            hold_ms: Arc::new(AtomicU64::new(0)),
            block_ms: Arc::new(AtomicU64::new(0)),
        }
    }
}

async fn pause(ms: &AtomicU64) {
    let ms = ms.load(Ordering::SeqCst);
    if ms > 0 {
        tokio::time::sleep(Duration::from_millis(ms)).await;
    }
}

#[async_trait::async_trait]
impl<S: ObjectStore> ObjectStore for Slow<S> {
    async fn put(&self, key: &str, data: Bytes) -> StoreResult<()> {
        pause(&self.put_ms).await;
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> StoreResult<Bytes> {
        pause(&self.get_ms).await;
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
    async fn list_with_mtime(&self, prefix: &str) -> StoreResult<Vec<(String, Option<u64>)>> {
        self.inner.list_with_mtime(prefix).await
    }
    async fn get_versioned(&self, key: &str) -> StoreResult<(Bytes, String)> {
        pause(&self.get_ms).await;
        let ms = self.block_ms.swap(0, Ordering::SeqCst);
        if ms > 0 {
            tokio::task::spawn_blocking(move || std::thread::sleep(Duration::from_millis(ms)));
        }
        self.inner.get_versioned(key).await
    }
    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> StoreResult<String> {
        pause(&self.put_ms).await;
        let put = self.inner.put_if(key, data, pre).await?;
        if self.armed.swap(false, Ordering::SeqCst) {
            if let Some(landed) = self.landed.lock().expect("the lock").take() {
                let _ = landed.send(());
            }
            pause(&self.hold_ms).await;
        }
        Ok(put)
    }
}

// ------------------------------------------------------------ the daemon

type Objects = Slow<Store>;
type SlowRefs = Arc<ObjectRefStore<Objects>>;

/// What the scenario plants in the copy of the each-turn closure.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Wiring {
    /// As `serve.rs` has it.
    AsBuilt,
    /// The desk's wake does nothing.
    NoWake,
}

/// An instance of the daemon, as far as the uploader goes: the queue's
/// thread with the pump and the desk, and the uploader's thread.
struct Daemon {
    tx: mpsc::Sender<Msg>,
    done: mpsc::Receiver<(u16, i32)>,
    next_tag: u16,
    queue: Option<std::thread::JoinHandle<(Option<Stopped>, InPlace<SimDisk>)>>,
    desk: Arc<Desk>,
    uploading: Option<Uploading>,
    posts: mpsc::Receiver<UploadShown>,
    seen: Vec<UploadShown>,
    objects: Objects,
    refs: SlowRefs,
}

/// The store started as the daemon starts it: its waker wakes the
/// queue's thread. (`Fixture::start`, with a waker.)
fn start_store(fx: &Fixture, tx: &mpsc::Sender<Msg>) -> InPlace<SimDisk> {
    let options = OpenOptions {
        hold_cuts: Some(HoldCuts {
            region: CutRegion::Memory(fx.region.clone()),
            boot: *b"the tests' boot.",
            bound: Some(BOUND),
            intent_region: Some(64 * BLOCK),
        }),
        ..OpenOptions::default()
    };
    let mut opened = Opened::open(fx.disk.clone(), lock(&fx.disk).expect("the lock"), options)
        .map_err(|(e, _)| e)
        .expect("opens");
    opened.device_taken_over();
    let wake = Mutex::new(tx.clone());
    opened
        .start(Some(Waker::new(move || {
            let _ = wake.lock().expect("the lock").send(Msg::Wake);
        })))
        .expect("the in-place store")
}

impl Daemon {
    /// Serve `fx`'s volume as `holder`: the lease taken at the start,
    /// the queue's thread, then the uploader's thread.
    fn serve(fx: &Fixture, objects: &Objects, holder: &str, wiring: Wiring) -> Self {
        Self::serve_with(fx, objects, holder, wiring, TTL, None)
    }

    /// The same, with a lease of `ttl` ms and, with `clock`, the refs
    /// object's clock (what a lease's time is judged by).
    fn serve_with(
        fx: &Fixture,
        objects: &Objects,
        holder: &str,
        wiring: Wiring,
        ttl: u64,
        clock: Option<Arc<AtomicU64>>,
    ) -> Self {
        let refs = ObjectRefStore::new(objects.clone());
        let refs: SlowRefs = Arc::new(match clock {
            Some(now) => refs.with_clock(Arc::new(move || now.load(Ordering::SeqCst))),
            None => refs,
        });
        let keeper = Keeper::new(Arc::clone(&refs), fx.vol, holder, ttl);
        let mut start = Start::new(runtime().expect("a runtime"), keeper, false);
        start.take().expect("the lease at the start");
        assert!(
            matches!(start.found(), Some((AtStart::Taken { .. }, _))),
            "{:?}",
            start.found()
        );

        let (tx, rx) = mpsc::channel();
        let (done_tx, done) = mpsc::channel();
        let store = start_store(fx, &tx);
        let desk = {
            let wake = Mutex::new(tx.clone());
            Desk::new(move || {
                if wiring != Wiring::NoWake {
                    let _ = wake.lock().expect("the lock").send(Msg::Wake);
                }
            })
        };
        let queue = {
            let desk = Arc::clone(&desk);
            std::thread::Builder::new()
                .name("queue".into())
                .spawn(move || {
                    let queue = ChanQueue {
                        rx,
                        bufs: HashMap::new(),
                        done: done_tx,
                        idle: IDLE,
                    };
                    // serve.rs 1104 to 1127, the desk's lines.
                    pumped(Pump::new(queue, store), |pump, ended| {
                        if ended {
                            desk.close();
                        } else {
                            desk.answer(pump.target());
                        }
                    })
                })
                .expect("the queue's thread")
        };
        let (runtime, keeper) = start.into_parts();
        let make = maker(objects.clone(), Arc::clone(&refs), holder.to_owned(), fx.vol);
        let (posted, posts) = mpsc::channel();
        let uploading = Uploading::start(runtime, keeper, make, Arc::clone(&desk), move |shown| {
            let _ = posted.send(shown);
        })
        .expect("the uploader's thread");
        Self {
            tx,
            done,
            next_tag: 0,
            queue: Some(queue),
            desk,
            uploading: Some(uploading),
            posts,
            seen: Vec::new(),
            objects: objects.clone(),
            refs,
        }
    }

    /// One request through the queue, waited for.
    fn request(&mut self, operation: u8, offset: u64, data: Option<Buf>) {
        let tag = self.next_tag;
        self.next_tag = (self.next_tag + 1) % 64;
        let sectors = data.as_ref().map_or(0, |buf| (buf.len() / 512) as u32);
        let request = Fetched {
            tag,
            desc: Desc {
                op_flags: u32::from(operation),
                nr_sectors: sectors,
                start_sector: offset / 512,
            },
        };
        self.tx.send(Msg::Fetch(request, data)).expect("the queue");
        let (answered, result) = self
            .done
            .recv_timeout(Duration::from_secs(30))
            .expect("the request's answer within 30 s");
        assert_eq!(answered, tag);
        assert!(result >= 0, "request {operation} at {offset}: {result}");
    }

    /// Block `block` full of `byte`, through the queue.
    fn write(&mut self, fx: &mut Fixture, block: u64, byte: u8) {
        fx.wrote(block, byte);
        self.request(op::WRITE, block * BLOCK, Some(Buf::filled(1, byte)));
    }

    /// Group `txg`: its uberblock to the four labels and the flush, all
    /// through the queue. The cut's instant is the volume now.
    fn group(&mut self, fx: &mut Fixture, txg: u64) {
        let labels = Labels::for_size(SIZE);
        let rings: Vec<u64> = labels.rings().map(|ring| ring.start).collect();
        for ring in rings {
            let offset = ring + (txg % RING_SLOTS) * SLOT;
            let mut slot = uberblock(txg);
            seal(&mut slot, offset, ByteOrder::Little);
            let at = offset as usize;
            fx.now[at..at + SLOT as usize].copy_from_slice(&slot);
            self.request(op::WRITE, offset, Some(slot));
        }
        self.request(op::FLUSH, 0, None);
        fx.at.insert(txg, fx.now.clone());
    }

    /// Posts until `until` holds of one, within `limit`: the post, or
    /// `None`.
    fn posted(
        &mut self,
        limit: Duration,
        until: impl Fn(&UploadShown) -> bool,
    ) -> Option<UploadShown> {
        let deadline = Instant::now() + limit;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.posts.recv_timeout(left) {
                Ok(shown) => {
                    self.seen.push(shown);
                    if until(&shown) {
                        return Some(shown);
                    }
                }
                Err(_) => return None,
            }
        }
    }

    fn states(&self) -> Vec<UploadStateShown> {
        let mut states: Vec<UploadStateShown> = Vec::new();
        for shown in &self.seen {
            if states.last() != Some(&shown.state) {
                states.push(shown.state);
            }
        }
        states
    }

    /// SIGTERM: the queue stops and its thread is joined; then the
    /// uploader's thread is ended as at exit 0. (`run`, 1466 to 1487.)
    fn stop(mut self) -> (Option<Stopped>, InPlace<SimDisk>, Ended, Duration) {
        self.tx.send(Msg::Stop).expect("the queue");
        let (stopped, store) = self
            .queue
            .take()
            .expect("the queue's thread")
            .join()
            .expect("joined");
        let began = Instant::now();
        let ended = self
            .uploading
            .take()
            .expect("the uploader's thread")
            .end(stopped == Some(Stopped::Orderly), RELEASE_LIMIT + END_LIMIT);
        (stopped, store, ended, began.elapsed())
    }

    /// As [`Daemon::stop`], with `pause` between the queue's end (the
    /// desk is closed there) and the end of the uploader's thread, as
    /// the daemon has while it stops its device: what the thread posted
    /// from the queue's end on.
    fn stop_slowly(mut self, pause: Duration) -> (Ended, Vec<UploadShown>) {
        while let Ok(shown) = self.posts.try_recv() {
            self.seen.push(shown);
        }
        self.tx.send(Msg::Stop).expect("the queue");
        let (stopped, _store) = self
            .queue
            .take()
            .expect("the queue's thread")
            .join()
            .expect("joined");
        std::thread::sleep(pause);
        let ended = self
            .uploading
            .take()
            .expect("the uploader's thread")
            .end(stopped == Some(Stopped::Orderly), RELEASE_LIMIT + END_LIMIT);
        let mut after = Vec::new();
        while let Ok(shown) = self.posts.try_recv() {
            after.push(shown);
        }
        (ended, after)
    }
}

/// A 4 KiB slot holding an uberblock of group `txg` (the fixture's).
fn uberblock(txg: u64) -> Buf {
    let mut slot = Buf::filled(1, 0);
    let mut put = |at: usize, word: u64| slot[at..at + 8].copy_from_slice(&word.to_le_bytes());
    put(0, MAGIC);
    put(8, 5000);
    put(16, txg);
    for n in 0..16 {
        put(40 + 8 * n, (txg * 31).rotate_left(n as u32) ^ 0x5a5a);
    }
    slot
}

fn uploaded_at_least(n: u64) -> impl Fn(&UploadShown) -> bool {
    move |shown| shown.figures.uploaded >= n && shown.state == UploadStateShown::Idle
}

fn lease_row(refs: &SlowRefs, fx: &Fixture) -> (String, u64, u64) {
    let lease = block_on(refs.lease_get(fx.vol))
        .expect("read")
        .expect("a lease row");
    (lease.holder, lease.epoch, lease.expires_at_ms)
}

// ------------------------------------------------------------- D1 to D7

/// D1. The loop as built: a cut made through the queue goes up while
/// the device is idle (only the desk's wake turns the queue: nothing
/// else comes for 20 s), a second cut whose blocks the pool overwrites
/// before the uploader reads them goes up as it was at its instant, and
/// a stop ends in order with the lease let go.
#[test]
fn d1_a_cut_goes_up_through_the_queue_loop_and_the_stop_lets_the_lease_go() {
    let mut fx = block_on(Fixture::new(118));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);

    let began = Instant::now();
    daemon.write(&mut fx, 10, 0x10);
    daemon.write(&mut fx, 70, 0x30);
    daemon.group(&mut fx, 7);
    let shown = daemon
        .posted(Duration::from_secs(10), uploaded_at_least(1))
        .unwrap_or_else(|| panic!("cut 7 uploaded within 10 s: {:?}", daemon.states()));
    let spent = began.elapsed();
    assert!(spent < Duration::from_secs(10), "{spent:?}");
    block_on(fx.assert_uploaded(7, "D1, the first cut"));
    assert_eq!(shown.lease, LeaseShown::Held);
    // The commit is stamped with the lease's epoch (N2 of #117's review).
    let head = block_on(daemon.refs.get(fx.vol, "head"))
        .expect("read")
        .expect("a head");
    let object = block_on(objects.get(&skynbd_engine::keys::commit(fx.vol, &head.commit), None))
        .expect("head's commit");
    let commit = skynbd_format::Commit::decode(&object).expect("a commit");
    assert_eq!(commit.epoch, 1, "the commit's epoch is the lease's");
    let answered = daemon.desk.answered();
    assert!(answered > 3, "the desk answered {answered} calls");
    println!(
        "D1: cut 7 (whole) uploaded {spent:?} after its first write, {answered} calls answered \
         on the queue's thread; states {:?}",
        daemon.states()
    );

    // The second cut: object storage is slow to read, so the check of
    // the cut takes a while, and the pool overwrites the cut's block
    // before the uploader has read it.
    daemon.seen.clear();
    objects.get_ms.store(150, Ordering::SeqCst);
    daemon.write(&mut fx, 10, 0x11);
    daemon.group(&mut fx, 8);
    daemon.write(&mut fx, 10, 0x99);
    daemon.write(&mut fx, 11, 0x98);
    objects.get_ms.store(0, Ordering::SeqCst);
    daemon
        .posted(Duration::from_secs(20), uploaded_at_least(2))
        .unwrap_or_else(|| panic!("cut 8 uploaded within 20 s: {:?}", daemon.states()));
    let uploaded = block_on(fx.assert_uploaded(8, "D1, the cut overwritten under the upload"));
    let at = (10 * BLOCK) as usize;
    assert_eq!(uploaded.content[at], 0x11, "the copy, not the later write");
    let states = daemon.states();
    assert!(
        states.contains(&UploadStateShown::Syncing),
        "a copy was read, so the copies were synced: {states:?}"
    );
    println!("D1: cut 8 uploaded from a copy; states {states:?}");

    let refs = Arc::clone(&daemon.refs);
    let (stopped, store, ended, took) = daemon.stop();
    assert_eq!(stopped, Some(Stopped::Orderly));
    assert_eq!(ended, Ended::Released);
    let (holder, epoch, expires) = lease_row(&refs, &fx);
    assert_eq!((holder.as_str(), epoch, expires), ("node-a", 1, 0));
    println!("D1: stop in order, {ended:?} in {took:?}; held cuts {:?}", store.held_cuts().len());
    // Another holder takes it at once.
    let mut other = Keeper::new(Arc::clone(&refs), fx.vol, "node-b", TTL);
    assert!(matches!(
        block_on(other.keep()),
        LeaseState::Held { epoch: 2, .. }
    ));
}

/// D2. The harness sees a wake that is missing: with a desk whose wake
/// does nothing, an idle queue answers no call, and nothing goes up
/// before the queue's idle time-out.
#[test]
fn d2_without_the_desks_wake_an_idle_queue_answers_nothing() {
    let mut fx = block_on(Fixture::new(119));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::NoWake);
    daemon.write(&mut fx, 10, 0x10);
    daemon.group(&mut fx, 7);
    let shown = daemon.posted(Duration::from_secs(4), uploaded_at_least(1));
    assert!(shown.is_none(), "uploaded with no wake: {:?}", daemon.states());
    println!(
        "D2: with no wake, nothing uploaded in 4 s on an idle queue; {} calls answered",
        daemon.desk.answered()
    );
    // The uploader's thread still ends when asked, with its call waiting.
    let began = Instant::now();
    drop(daemon.uploading.take());
    assert!(began.elapsed() < END_LIMIT + Duration::from_secs(1));
    daemon.tx.send(Msg::Stop).expect("the queue");
    daemon.queue.take().expect("the thread").join().expect("joined");
}

/// D3. A stop while a commit is under way: the queue stops and closes
/// the desk, the uploader's thread drops the commit and lets the lease
/// go, well within its limit. Head is the commit it was, whole. The
/// next instance takes the cut state up and uploads the cut.
#[test]
fn d3_a_stop_under_a_commit_ends_in_time_and_the_next_instance_uploads_the_cut() {
    let mut fx = block_on(Fixture::new(120));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    objects.put_ms.store(700, Ordering::SeqCst);
    daemon.write(&mut fx, 10, 0x10);
    daemon.group(&mut fx, 7);
    daemon
        .posted(Duration::from_secs(10), |s| {
            s.state == UploadStateShown::Committing
        })
        .unwrap_or_else(|| panic!("committing within 10 s: {:?}", daemon.states()));
    let refs = Arc::clone(&daemon.refs);
    let (stopped, store, ended, took) = daemon.stop();
    objects.put_ms.store(0, Ordering::SeqCst);
    assert_eq!(stopped, Some(Stopped::Orderly));
    assert!(took < RELEASE_LIMIT + END_LIMIT, "{took:?}");
    assert_eq!(ended, Ended::Released, "after {took:?}");
    let (_, _, expires) = lease_row(&refs, &fx);
    assert_eq!(expires, 0, "let go");
    let held = store.held_cuts().len();
    let before = block_on(fx.uploaded());
    assert!(before.fsck, "head passes fsck after the stop");
    println!(
        "D3: stop under a commit: {ended:?} in {took:?}; head's group {:?}, {held} cut held",
        before.group
    );
    drop(store);

    let mut next = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    let shown = next
        .posted(Duration::from_secs(10), |s| {
            s.state == UploadStateShown::Idle
                && s.figures.uploaded + s.figures.found_committed >= 1
        })
        .unwrap_or_else(|| panic!("the next instance's upload: {:?}", next.states()));
    block_on(fx.assert_uploaded(7, "D3, the next instance"));
    println!(
        "D3: the next instance: uploaded {}, found committed {}, lease epoch {}",
        shown.figures.uploaded, shown.figures.found_committed, shown.epoch
    );
    let (_, _, ended, _) = next.stop();
    assert_eq!(ended, Ended::Released);
}

/// D4. The queue stops between a commit's swap and its release: the
/// swap has landed, the desk is closed before the uploader can let the
/// cut go, and the lease is let go at exit 0 with the cut uploaded and
/// still held. The next instance finds the cut committed, lets it go and
/// uploads nothing twice.
#[test]
fn d4_a_stop_between_the_swap_and_the_release_is_found_committed_by_the_next_instance() {
    let mut fx = block_on(Fixture::new(121));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    let (landed, lands) = mpsc::channel();
    *objects.landed.lock().expect("the lock") = Some(landed);
    objects.hold_ms.store(1500, Ordering::SeqCst);
    // Slow reads of object storage, so that the arming below comes
    // before the commit's swap and after the lease's first renewal.
    daemon.write(&mut fx, 10, 0x10);
    daemon.group(&mut fx, 7);
    daemon
        .posted(Duration::from_secs(10), |s| {
            s.state == UploadStateShown::Reading
        })
        .unwrap_or_else(|| panic!("reading within 10 s: {:?}", daemon.states()));
    objects.armed.store(true, Ordering::SeqCst);
    lands
        .recv_timeout(Duration::from_secs(10))
        .expect("the swap landed within 10 s");
    // The swap is in object storage and its answer is held: the queue
    // stops now, before the release.
    let refs = Arc::clone(&daemon.refs);
    let (stopped, store, ended, took) = daemon.stop();
    assert_eq!(stopped, Some(Stopped::Orderly));
    let held = store.held_cuts().len();
    let after = block_on(fx.uploaded());
    println!(
        "D4: stop between the swap and the release: {ended:?} in {took:?}; head's group {:?}, \
         the row's {:?}, {held} cut still held",
        after.group,
        after.row.map(|row| row.group)
    );
    assert_eq!(after.group, Some(7), "the swap had landed");
    assert_eq!(held, 1, "the cut was not let go");
    assert_eq!(ended, Ended::Released);
    let (_, _, expires) = lease_row(&refs, &fx);
    assert_eq!(expires, 0);
    drop(store);

    objects.hold_ms.store(0, Ordering::SeqCst);
    let mut next = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    let shown = next
        .posted(Duration::from_secs(10), |s| {
            s.state == UploadStateShown::Idle
                && s.figures.uploaded + s.figures.found_committed >= 1
        })
        .unwrap_or_else(|| panic!("the next instance: {:?}", next.states()));
    assert_eq!(
        (shown.figures.found_committed, shown.figures.uploaded),
        (1, 0),
        "found committed, nothing uploaded twice"
    );
    block_on(fx.assert_uploaded(7, "D4, the next instance"));
    let (_, store, _, _) = next.stop();
    assert!(store.held_cuts().is_empty(), "let go by the next instance");
    println!("D4: the next instance found cut 7 committed and let it go");
}

/// D5. The queue's thread panics in a turn: `pumped` catches it and the
/// each-turn closure is not called with `ended`, so the queue's thread
/// does not close the desk. The uploader's call waits; the daemon's drop
/// of the thread's handle (every end but exit 0) closes the desk and
/// ends the thread within its limit, the lease kept.
#[test]
fn d5_a_queue_thread_that_panics_leaves_the_desk_open_and_the_drop_ends_the_uploader() {
    let fx = block_on(Fixture::new(122));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    daemon
        .posted(Duration::from_secs(10), |s| s.state == UploadStateShown::Idle)
        .expect("idle");
    let quiet = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    daemon.tx.send(Msg::Panic).expect("the queue");
    let (stopped, store) = daemon
        .queue
        .take()
        .expect("the thread")
        .join()
        .expect("joined");
    std::panic::set_hook(quiet);
    assert_eq!(stopped, None, "the pump panicked");
    let open = !daemon.desk.closed();
    let began = Instant::now();
    drop(daemon.uploading.take());
    let took = began.elapsed();
    assert!(took < END_LIMIT + Duration::from_secs(1), "{took:?}");
    assert!(daemon.desk.closed(), "the drop closed it");
    let (_, _, expires) = lease_row(&daemon.refs, &fx);
    assert!(expires > 0, "kept");
    println!(
        "D5: after a panic of the queue's thread the desk was {}; the drop ended the uploader \
         in {took:?}, lease kept",
        if open { "left open" } else { "closed" }
    );
    drop(store);
    let _ = &daemon.objects;
}

/// D6. What #117's review left to this pull request: the remote the
/// daemon makes (`maker`) is fenced by the epoch the lease holds. The
/// daemon's lease runs out (its renewals are an hour apart here, so only
/// a commit can tell it), another holder takes it, and the daemon's next
/// cut is read and PUT and its commit refused: nothing lands, the status
/// says who has the lease, and the end at exit 0 lets nothing go of the
/// other holder's lease.
#[test]
fn d6_the_daemons_commit_is_fenced_once_another_holder_has_the_lease() {
    const HOUR: u64 = 3_600_000;
    let mut fx = block_on(Fixture::new(123));
    let objects = Slow::new(fx.store.clone());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let mut daemon = Daemon::serve_with(
        &fx,
        &objects,
        "node-a",
        Wiring::AsBuilt,
        HOUR,
        Some(Arc::clone(&now)),
    );
    daemon.write(&mut fx, 10, 0x10);
    daemon.group(&mut fx, 7);
    daemon
        .posted(Duration::from_secs(10), uploaded_at_least(1))
        .unwrap_or_else(|| panic!("cut 7 uploaded: {:?}", daemon.states()));

    now.fetch_add(HOUR + 1, Ordering::SeqCst);
    let theirs = {
        let now = Arc::clone(&now);
        Arc::new(
            ObjectRefStore::new(objects.clone())
                .with_clock(Arc::new(move || now.load(Ordering::SeqCst))),
        )
    };
    let mut other = Keeper::new(Arc::clone(&theirs), fx.vol, "node-b", HOUR);
    let taken = block_on(other.keep()).clone();
    assert!(matches!(taken, LeaseState::Held { epoch: 2, .. }), "{taken:?}");

    daemon.seen.clear();
    daemon.write(&mut fx, 11, 0x21);
    daemon.group(&mut fx, 8);
    let shown = daemon
        .posted(Duration::from_secs(10), |s| {
            s.state == UploadStateShown::LeaseLost
        })
        .unwrap_or_else(|| panic!("the lease lost within 10 s: {:?}", daemon.states()));
    assert_eq!((shown.holder.said(), shown.epoch), ("node-b".to_string(), 2));
    assert_eq!(shown.lease, LeaseShown::Lost);
    let after = block_on(fx.uploaded());
    assert_eq!(after.group, Some(7), "nothing landed");
    assert_eq!(after.row.map(|row| row.group), Some(7));
    assert!(after.fsck);
    let states = daemon.states();
    let refs = Arc::clone(&daemon.refs);
    let (_, store, ended, _) = daemon.stop();
    let (holder, epoch, expires) = lease_row(&refs, &fx);
    println!(
        "D6: after node-b took the lease at epoch 2: states {states:?}; head still group 7; \
         the end at exit 0: {ended:?}; the row: {holder} at epoch {epoch}, {}; {} cut held",
        if expires > 0 { "running" } else { "let go" },
        store.held_cuts().len()
    );
    assert_eq!((holder.as_str(), epoch), ("node-b", 2));
    assert!(expires > 0, "the other holder's lease was let go");
    assert_eq!(ended, Ended::Kept);
}

/// D7. The end of the uploader's thread is given a limit (`end`: "waits
/// at most `limit` for the thread"), and waits longer where the runtime
/// has blocking work left: the thread's answer comes in time, and then
/// the thread is joined with no limit, and its last act is to drop the
/// runtime, which waits for the blocking pool. Here a blocking task of
/// 8 s stands for a name lookup that is slow to answer.
#[test]
fn was_f2_d7_the_end_keeps_its_limit_with_blocking_work_left_on_the_runtime() {
    let fx = block_on(Fixture::new(124));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    daemon
        .posted(Duration::from_secs(10), |s| s.state == UploadStateShown::Idle)
        .expect("idle");
    objects.block_ms.store(8000, Ordering::SeqCst);
    let limit = RELEASE_LIMIT + END_LIMIT;
    let (_, _store, ended, took) = daemon.stop();
    println!("D7: the end at exit 0, given {limit:?}: {ended:?} after {took:?}");
    assert_eq!(ended, Ended::Released);
    assert!(
        took <= limit + Duration::from_millis(500),
        "the end took {took:?}, its limit was {limit:?}"
    );
}

// ------------------------------------------------------------- S1 to S6

type Mem = Arc<MemoryStore>;

fn mem() -> (Mem, Arc<ObjectRefStore<Mem>>) {
    let store: Mem = Arc::new(MemoryStore::new());
    let refs = Arc::new(ObjectRefStore::new(Arc::clone(&store)));
    (store, refs)
}

fn place_of(name: &str) -> Remote {
    Remote::new("http://127.0.0.1:9000", "volumes", name).expect("a place")
}

/// S1 (Codex's first finding, checked). A clone of a volume that holds
/// data, never served, of the size asked: its first commit has no
/// parent, no segments and no group, and names the base's data.
/// `create` must not take it as empty.
#[test]
fn was_f1_s1_create_refuses_a_clone_that_holds_data() {
    const SIZE: u64 = 64 << 20;
    let (store, refs) = mem();
    let base = place_of("pg-base");
    assert_eq!(
        block_on(place(Arc::clone(&store), Arc::clone(&refs), &base, SIZE)),
        Ok(Placed::Made)
    );
    let mut engine = block_on(Engine::open(
        Arc::clone(&store),
        Arc::clone(&refs),
        base.id,
        engine_config(1),
    ))
    .expect("opened");
    engine.write(0, &[7u8; 4096]).expect("written");
    block_on(engine.commit(CommitKind::Flush)).expect("committed");
    let head = block_on(refs.get(base.id, "head"))
        .expect("read")
        .expect("a head");

    let clone = place_of("pg-clone");
    let cloned = block_on(Engine::clone_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        clone.id,
        SIZE,
        base.id,
        head.commit,
        engine_config(0),
    ))
    .expect("cloned");
    let first = block_on(cloned.read(0, 4096)).expect("read");
    assert_eq!(&first[..], &[7u8; 4096][..], "the clone holds the base's data");
    drop(cloned);

    let placed = block_on(place(Arc::clone(&store), Arc::clone(&refs), &clone, SIZE));
    println!("S1: create --volume pg-clone on a clone that holds data: {placed:?}");
    assert!(
        placed.is_err(),
        "a clone that holds data was taken as a volume a create left: {placed:?}"
    );
}

/// S1b. What follows from S1, with the daemon's uploader: the volume
/// served is bound to a clone that holds data (as `create` leaves it),
/// and its first cut, which holds the whole local volume, goes up onto
/// the clone's head (no row, and a head commit with no group: the
/// check's rule 2). Head then holds the local volume, and the clone's
/// data is at no ref.
#[test]
fn s1b_the_first_cut_of_a_volume_bound_to_a_clone_replaces_the_clones_data() {
    let mut fx = block_on(Fixture::new(125));
    let store = fx.store.clone();
    let refs = Arc::new(ObjectRefStore::new(store.clone()));
    let base = skynbd_types::VolumeId(ulid::Ulid::from(0xba5e_u128));
    let mut engine = block_on(Engine::create_volume(
        store.clone(),
        Arc::clone(&refs),
        base,
        SIZE,
        cfg(),
    ))
    .expect("the base");
    let at = 20 * BLOCK;
    engine.write(at, &[7u8; 4096]).expect("written");
    block_on(engine.commit(CommitKind::Flush)).expect("committed");
    let head = block_on(refs.get(base, "head")).expect("read").expect("a head");
    let clone = skynbd_types::VolumeId(ulid::Ulid::from(0xc10e_u128));
    drop(
        block_on(Engine::clone_volume(
            store.clone(),
            Arc::clone(&refs),
            clone,
            SIZE,
            base,
            head.commit,
            cfg(),
        ))
        .expect("cloned"),
    );
    // The local volume, empty, bound to the clone.
    fx.vol = clone;
    let before = block_on(fx.uploaded());
    assert_eq!(before.content[at as usize], 7, "the clone holds the base's data");

    let objects = Slow::new(store);
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    daemon.write(&mut fx, 10, 0x10);
    daemon.group(&mut fx, 7);
    daemon
        .posted(Duration::from_secs(10), uploaded_at_least(1))
        .unwrap_or_else(|| panic!("the first cut uploaded: {:?}", daemon.states()));
    let after = block_on(fx.assert_uploaded(7, "S1b"));
    println!(
        "S1b: the clone's block 20 read {} before the first cut of the volume bound to it, and \
         reads {} after: head is the local volume, group {:?}",
        before.content[at as usize], after.content[at as usize], after.group
    );
    assert_eq!(after.content[at as usize], 0, "the clone's data is gone from head");
    daemon.stop();
}

/// S2 (Codex's third finding, checked). The status's `lease_holder` is
/// "this daemon's while held" (`status.rs`, the table of version 15).
#[test]
fn was_f3_s2_the_status_names_the_holder_while_the_lease_is_held() {
    let (_, refs) = mem();
    let vol = place_of("pg-data").id;
    let keeper = Keeper::new(Arc::clone(&refs), vol, "node-a", TTL);
    let mut start = Start::new(runtime().expect("a runtime"), keeper, false);
    start.take().expect("taken");
    let at_start = start.shown().expect("shown");
    let later = shown_of(&Shown {
        state: State::Idle,
        lease: LeaseState::Held {
            epoch: 1,
            expires_at_ms: 9,
        },
        figures: Default::default(),
    }, "node-a");
    println!(
        "S2: lease held: lease_holder at the start {:?}, after a turn {:?}",
        at_start.holder.said(),
        later.holder.said()
    );
    assert_eq!(at_start.lease, LeaseShown::Held);
    assert_eq!(at_start.holder.said(), "node-a", "at the start");
    assert_eq!(later.holder.said(), "node-a", "after a turn");
}

/// S3. A start that acknowledges a failure and is then refused for the
/// lease, on the real store in a directory: the store's marker is gone
/// and the start token is new, so the next start, without
/// `--acknowledge-failure`, opens the volume. (The failure note in the
/// run directory is the lifecycle's and stays: its test.)
#[test]
fn s3_a_refusal_after_an_acknowledging_open_has_removed_the_stores_marker() {
    let keep = tempfile::tempdir().expect("a directory");
    let path = keep.path();
    let dir = FsDir::new(path).expect("the directory");
    let config = VolumeConfig {
        volume: *b"a volume refused",
        size_mib: 1,
        base: skyzfs_blk::Base::Empty,
        write_unit: 4096,
    };
    skyzfs_local::inplace::create(&dir, &config).expect("created");
    let marker = path.join(skyzfs_local::volume::MARKER_NAME);
    std::fs::write(&marker, b"").expect("marked");
    let token = || std::fs::read(path.join(skyzfs_local::volume::START_NAME)).ok();
    let before = token();

    let mut volume = StoreVolume::new(path, OpenOptions::default());
    let held = volume.lock().expect("locked");
    let held = match volume.open(held, false) {
        Err((OpenError::MarkedFailed, lock)) => lock,
        Err((other, _)) => panic!("{other:?}"),
        Ok(_) => panic!("a marked volume opened"),
    };
    assert_eq!(token(), before, "a refused open of a marked volume writes no token");
    // The lifecycle's second open, acknowledging; the lease refuses it.
    let mut leased = Leased::new(&mut volume, Opened::<FsDir>::share_lock, || {
        Err("another holder has the volume's lease".to_string())
    });
    let lock = match leased.open(held, true) {
        Err((OpenError::Refused(_), lock)) => lock,
        Err((other, _)) => panic!("{other:?}"),
        Ok(_) => panic!("opened"),
    };
    drop(lock);
    let marked = marker.exists();
    let moved = token() != before;
    // The next start, with no acknowledgement.
    let held = volume.lock().expect("the lock was given back");
    let opens = volume.open(held, false).is_ok();
    println!(
        "S3: after exit 10 on an acknowledging start: marker {}, start token {}, the next start \
         without --acknowledge-failure {}",
        if marked { "still there" } else { "removed" },
        if moved { "new" } else { "as it was" },
        if opens { "opens the volume" } else { "is refused" }
    );
    assert!(!marked && moved && opens);
}

/// Object storage where the first conditional PUT lands and is never
/// answered.
#[derive(Clone)]
struct LandsUnheard {
    inner: Mem,
    stall: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl ObjectStore for LandsUnheard {
    async fn put(&self, key: &str, data: Bytes) -> StoreResult<()> {
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
    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> StoreResult<String> {
        let put = self.inner.put_if(key, data, pre).await;
        if self.stall.swap(false, Ordering::SeqCst) {
            std::future::pending::<()>().await;
        }
        put
    }
}

/// S4. The start's take lands in object storage and its answer does not
/// come within the limit: the volume is served with "no lease yet"
/// while the row names this holder. The uploader's first take then
/// holds the lease at the same epoch: the holder does not fence itself,
/// and nothing is lost but the status's word until then.
#[test]
fn s4_a_take_that_landed_unheard_is_held_at_the_same_epoch_by_the_uploader() {
    let inner: Mem = Arc::new(MemoryStore::new());
    let store = LandsUnheard {
        inner: Arc::clone(&inner),
        stall: Arc::new(AtomicBool::new(true)),
    };
    let refs = Arc::new(ObjectRefStore::new(store));
    let vol = place_of("pg-data").id;
    let keeper = Keeper::new(Arc::clone(&refs), vol, "node-a", TTL);
    let mut start =
        Start::new(runtime().expect("a runtime"), keeper, false).with_limit(Duration::from_millis(100));
    start.take().expect("served");
    let found = start.found().expect("found").0.clone();
    assert!(matches!(found, AtStart::Unreached(_)), "{found:?}");
    let row = block_on(refs.lease_get(vol)).expect("read").expect("a row");
    assert_eq!((row.holder.as_str(), row.epoch), ("node-a", 1), "it landed");
    let (rt, mut keeper) = start.into_parts();
    let state = rt
        .block_on(keeper.keep())
        .expect("the runtime runs")
        .clone();
    println!("S4: the start said {found:?}; the row names node-a at epoch 1; the uploader's take: {state:?}");
    assert!(matches!(state, LeaseState::Held { epoch: 1, .. }), "{state:?}");
}

/// S5. A start that took the lease and ended before it served (the
/// device could not be added, say: exit 1) keeps the lease: another
/// holder's start is refused until the lease's time has run out, and
/// the same holder's next start has it at once, at the same epoch.
#[test]
fn s5_a_start_that_fails_after_the_take_keeps_the_lease_for_its_time() {
    let store: Mem = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let refs = {
        let now = Arc::clone(&now);
        Arc::new(
            ObjectRefStore::new(Arc::clone(&store))
                .with_clock(Arc::new(move || now.load(Ordering::SeqCst))),
        )
    };
    let vol = place_of("pg-data").id;
    let start_of = |holder: &str| {
        let keeper = Keeper::new(Arc::clone(&refs), vol, holder, TTL);
        Start::new(runtime().expect("a runtime"), keeper, false)
    };
    let mut first = start_of("node-a");
    first.take().expect("taken");
    drop(first); // run() returns Err: `uploads` is dropped, nothing is released.

    let mut other = start_of("node-b");
    let refused = other.take().expect_err("refused while the lease runs");
    now.fetch_add(TTL - 1, Ordering::SeqCst);
    let still = start_of("node-b").take().is_err();
    let mut again = start_of("node-a");
    again.take().expect("the same holder");
    let same = again.found().map(|(found, _)| found.clone());
    now.fetch_add(TTL + 1, Ordering::SeqCst);
    let mut late = start_of("node-b");
    late.take().expect("taken once it ran out");
    println!(
        "S5: after a start that took the lease and failed: node-b refused ({}), still refused \
         {} ms later: {still}; node-a again: {same:?}; node-b after the lease's time: {:?}",
        refused.split('.').next().unwrap_or(""),
        TTL - 1,
        late.found().map(|(found, _)| found.clone())
    );
    assert!(still);
    assert_eq!(same, Some(AtStart::Taken { epoch: 1 }));
    assert_eq!(
        late.found().map(|(found, _)| found.clone()),
        Some(AtStart::Taken { epoch: 2 })
    );
}

/// S6. Two creates of one name before either volume is served: both
/// succeed, the second "found empty", and nothing in object storage
/// says which local volume the name is.
#[test]
fn s6_two_creates_of_one_name_before_a_serve_both_succeed() {
    const SIZE: u64 = 64 << 20;
    let (store, refs) = mem();
    let name = place_of("pg-data");
    let first = block_on(place(Arc::clone(&store), Arc::clone(&refs), &name, SIZE));
    let second = block_on(place(Arc::clone(&store), Arc::clone(&refs), &name, SIZE));
    println!("S6: the first create: {first:?}; a second, of another directory or node: {second:?}");
    assert_eq!((first, second), (Ok(Placed::Made), Ok(Placed::Found)));
}

// ------------------------------------------------- the next look, at da0b4c9

/// 2.1 of the look at da0b4c9 (Codex's finding on a18bedf), which
/// failed there as `defect_n1_...`: `create`'s place has a limit
/// (`place_within`), and what the work leaves on the runtime's blocking
/// pool at the limit must not be waited for. `within` is private, so
/// this is its body with the limit made 200 ms, on the daemon's own
/// `runtime()`: a store that never answers, and a blocking task of 3 s
/// in place of a name lookup that is slow to answer.
#[test]
fn was_2_1_a_place_ends_at_its_limit_with_blocking_work_left() {
    let limit = Duration::from_millis(200);
    let at = Instant::now();
    let placed: Result<Placed, String> = {
        let runtime = runtime().expect("a runtime");
        runtime
            .block_on(async {
                drop(tokio::task::spawn_blocking(|| {
                    std::thread::sleep(Duration::from_secs(3))
                }));
                tokio::time::timeout(limit, std::future::pending::<Result<Placed, String>>())
                    .await
                    .unwrap_or_else(|_| {
                        Err("object storage did not answer within the limit".into())
                    })
            })
            .expect("the runtime runs")
    };
    let took = at.elapsed();
    println!("2.1: a place given {limit:?} came back with {placed:?} after {took:?}");
    assert!(placed.is_err());
    assert!(
        took < limit + Duration::from_secs(1),
        "the place took {took:?}, its limit was {limit:?}"
    );
}

/// 2.1, the second place (`defect_n1b_...` at da0b4c9): a start that
/// ends before the uploader's thread is started (a refusal, a device
/// that cannot be added). The lease at the start owns the runtime, and
/// its drop must not wait for the blocking pool.
#[test]
fn was_2_1b_a_start_dropped_before_the_thread_does_not_wait_for_blocking_work() {
    let (_, refs) = mem();
    let vol = place_of("pg-data").id;
    let keeper = Keeper::new(Arc::clone(&refs), vol, "node-a", TTL);
    let runtime = runtime().expect("a runtime");
    runtime
        .block_on(async {
            drop(tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_secs(3))
            }));
        })
        .expect("the runtime runs");
    let mut start = Start::new(runtime, keeper, false);
    start.take().expect("taken");
    let at = Instant::now();
    drop(start);
    let took = at.elapsed();
    println!("2.1b: the drop of a start that took the lease, with blocking work left: {took:?}");
    assert!(took < Duration::from_secs(1), "the drop took {took:?}");
}

/// 2.1, the third: the runtime handed on by `into_parts` (what the
/// uploader's thread takes over) and dropped where the thread could not
/// be started.
#[test]
fn n5_the_runtime_handed_on_by_a_start_does_not_wait_at_its_drop_either() {
    let (_, refs) = mem();
    let vol = place_of("pg-data").id;
    let keeper = Keeper::new(Arc::clone(&refs), vol, "node-a", TTL);
    let mut start = Start::new(runtime().expect("a runtime"), keeper, false);
    start.take().expect("taken");
    let (runtime, keeper) = start.into_parts();
    runtime
        .block_on(async {
            drop(tokio::task::spawn_blocking(|| {
                std::thread::sleep(Duration::from_secs(3))
            }));
        })
        .expect("the runtime runs");
    let at = Instant::now();
    drop(keeper);
    drop(runtime);
    let took = at.elapsed();
    println!("N5: the drop of the runtime a start handed on, with blocking work left: {took:?}");
    assert!(took < Duration::from_secs(1), "the drop took {took:?}");
}

/// N2. Two creates of one name at once, on two threads with nothing
/// held, many times over, of two sizes and of one. Whatever the
/// interleaving: at most one says it made the volume; where there is a
/// head, the superblock is of the size of a create that did not fail;
/// and a create that says "made" or "found empty" asked for the
/// superblock's size.
#[test]
fn n2_two_creates_of_one_name_on_two_threads_leave_one_volume_of_one_size() {
    const A: u64 = 256 << 20;
    const B: u64 = 512 << 20;
    let mut seen = std::collections::BTreeMap::<String, u32>::new();
    for round in 0..300u32 {
        let (store, refs) = mem();
        let name = place_of("pg-data");
        let sizes = if round % 3 == 0 { [A, A] } else { [A, B] };
        let go = Arc::new(std::sync::Barrier::new(2));
        let threads: Vec<_> = sizes
            .into_iter()
            .map(|size| {
                let (store, refs, name, go) =
                    (Arc::clone(&store), Arc::clone(&refs), name.clone(), Arc::clone(&go));
                std::thread::spawn(move || {
                    go.wait();
                    block_on(place(store, refs, &name, size))
                })
            })
            .collect();
        let ends: Vec<_> = threads
            .into_iter()
            .map(|t| t.join().expect("joined"))
            .collect();
        let made = ends.iter().filter(|e| **e == Ok(Placed::Made)).count();
        assert!(made <= 1, "round {round}: both creates made the volume: {ends:?}");
        let head = block_on(refs.get(name.id, "head")).expect("read");
        let block = block_on(store.get(&skynbd_engine::keys::superblock(name.id, 0), None));
        let size = block
            .ok()
            .map(|b| Superblock::decode(&b).expect("a superblock").size);
        for (end, asked) in ends.iter().zip(sizes) {
            if end.is_ok() {
                assert!(head.is_some(), "round {round}: {end:?} and no head");
                assert_eq!(size, Some(asked), "round {round}: {ends:?} of {sizes:?}");
            }
        }
        if sizes[0] == sizes[1] {
            // 2.2 of the look at da0b4c9: the loser of the head's swap
            // takes the volume the other made.
            let found = ends.iter().filter(|e| **e == Ok(Placed::Found)).count();
            assert_eq!(
                (made, found),
                (1, 1),
                "round {round}: two creates of one size: {ends:?}"
            );
        }
        if head.is_some() {
            assert!(
                ends.iter().any(Result::is_ok),
                "round {round}: a head, and neither create says it made the volume: {ends:?}"
            );
        }
        let key = format!(
            "{} sizes: {}",
            if sizes[0] == sizes[1] { "one" } else { "two" },
            ends.iter()
                .map(|e| match e {
                    Ok(p) => p.said().to_owned(),
                    Err(why) if why.contains("no head") => "refused (superblock, no head)".into(),
                    Err(why) if why.contains("made it first") => "refused (made first)".into(),
                    Err(why) if why.contains("another volume's") => "refused (another's)".into(),
                    Err(why) => format!("failed ({})", why.rsplit(": ").next().unwrap_or("")),
                })
                .collect::<Vec<_>>()
                .join(" / ")
        );
        *seen.entry(key).or_default() += 1;
    }
    for (what, n) in &seen {
        println!("N2: {n:3} rounds: {what}");
    }
}

/// N3. The path of a create that finds no head looks at the superblock
/// and not for a clone's base. A clone that stopped between its base
/// marker and its head (`Engine::clone_volume` writes the superblock,
/// then the marker, then the head), of the size asked and with skyzfsd's
/// shards, is gone on with: the create makes the head, and the volume
/// carries the marker. A second create of the name then refuses it as a
/// clone. No tool in the repository makes a clone with no lease row;
/// this records what the path does.
#[test]
fn n3_a_create_over_a_clone_that_stopped_before_its_head() {
    const SIZE: u64 = 256 << 20;
    let (store, refs) = mem();
    let name = place_of("pg-clone");
    let base = place_of("pg-base");
    assert_eq!(
        block_on(place(Arc::clone(&store), Arc::clone(&refs), &base, SIZE)),
        Ok(Placed::Made)
    );
    let head = block_on(refs.get(base.id, "head")).expect("read").expect("a head");
    let superblock = Superblock {
        vol_id: name.id,
        size: SIZE,
        format_major: 1,
        created_at_ms: 1,
        shard_bytes: SHARD_BYTES,
    };
    block_on(store.put(
        &skynbd_engine::keys::superblock(name.id, 0),
        superblock.encode().expect("encoded").into(),
    ))
    .expect("put");
    let marker = CloneBase {
        base_vol: base.id,
        base_commit: head.commit,
        created_at_ms: 1,
    };
    block_on(store.put(
        &skynbd_engine::keys::clone_base(name.id),
        marker.encode().expect("encoded").into(),
    ))
    .expect("put");
    let first = block_on(place(Arc::clone(&store), Arc::clone(&refs), &name, SIZE));
    let second = block_on(place(Arc::clone(&store), Arc::clone(&refs), &name, SIZE));
    println!("N3: a create over a clone that stopped before its head: {first:?}; the next: {second:?}");
    assert_eq!(first, Ok(Placed::Made));
    assert!(second.is_err_and(|why| why.contains("clone")));
}

/// N4. The status after a stop (N5 of #117's review). A second cut is
/// under way, with slow PUTs, when the queue ends and closes the desk;
/// the uploader's thread is ended two seconds later, as after a device's
/// stop. On a closed desk the driver's turns count the cut as lost, or
/// as committed and gone: the thread must post none of that.
#[test]
fn n4_the_status_after_a_stop_keeps_the_figures_from_before_the_close() {
    let mut fx = block_on(Fixture::new(131));
    let objects = Slow::new(fx.store.clone());
    let mut daemon = Daemon::serve(&fx, &objects, "node-a", Wiring::AsBuilt);
    daemon.write(&mut fx, 10, 0x10);
    daemon.group(&mut fx, 7);
    let up = daemon
        .posted(Duration::from_secs(10), uploaded_at_least(1))
        .unwrap_or_else(|| panic!("the first cut uploaded: {:?}", daemon.states()));
    objects.put_ms.store(300, Ordering::SeqCst);
    daemon.write(&mut fx, 11, 0x11);
    daemon.group(&mut fx, 8);
    daemon
        .posted(Duration::from_secs(10), |s| {
            s.state != UploadStateShown::Idle && s.figures.uploaded == up.figures.uploaded
        })
        .unwrap_or_else(|| panic!("the second cut under way: {:?}", daemon.states()));
    let (ended, after) = daemon.stop_slowly(Duration::from_secs(2));
    println!(
        "N4: a stop with cut 2 under way: {ended:?}; {} posts from the queue's end on: {:?}",
        after.len(),
        after
            .iter()
            .map(|s| (s.state, s.figures.uploaded, s.figures.lost, s.figures.not_held))
            .collect::<Vec<_>>()
    );
    assert!(after.len() <= 1, "posts after the close: {}", after.len());
    for shown in &after {
        assert_eq!(shown.figures.lost, 0, "no cut was lost: the daemon was stopping");
        assert_eq!(shown.figures.not_held, 0);
    }
}

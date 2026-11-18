//! The fixture of the uploader's tests: a volume of 1 MiB on the
//! simulated disk, whose store holds cuts made as a pool makes them; the
//! volume's content at each cut's instant; and the volume in object
//! storage behind a store that can die at any operation.
//!
//! The simulated disk of [`Fixture::new`] cannot say where its files'
//! holes are, so a whole cut reads every block of the volume, as before
//! M2.4e; that of [`Fixture::sparse`] can, so a whole cut reads only what
//! was written since its state began, and its holes go up as zeros.
//!
//! The volume's four labels have their rings at 128, 384, 640 and
//! 896 KiB. A group's uberblock is written to the four, one 4 KiB slot
//! each with its label checksum, then a flush: the cut of that group
//! ([`Fixture::group`]).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;

use bytes::Bytes;

use skynbd_engine::{fsck, Engine, EngineConfig};
use skynbd_refs::{ObjectRefStore, RefOp, RefStore};
use skynbd_store::{
    FaultOp, FaultStore, FaultWhen, MemoryStore, ObjectStore, Precondition, StoreError,
};
use skynbd_types::VolumeId;
use skyzfs_blk::{Base, BlockTarget, Buf, Op, Request, RequestId};
use skyzfs_local::inplace::{
    create, lock, records_for, words_for, CommittedCut, CutRegion, CutSlice, HoldCuts, InPlace,
    Lost, MemRegion, OpenOptions, Opened, VolumeConfig,
};
use skyzfs_local::io::Dir;
use skyzfs_local::sim::SimDisk;
use skyzfs_uberblock::{seal, ByteOrder, Labels, MAGIC};
use skyzfs_upload::{row, Doing, Driver, EngineRemote, Local, Released, Uploadable, ROW_KEY};

pub const KIB: u64 = 1 << 10;
pub const BLOCK: u64 = 4 * KIB;
pub const SIZE: u64 = 1 << 20;
pub const BLOCKS: u64 = SIZE / BLOCK;
/// The bound on the copies: 64 blocks.
pub const BOUND: u64 = 64 * BLOCK;
const INTENT_REGION: u64 = 64 * BLOCK;
const BOOT: [u8; 16] = *b"the tests' boot.";
const WAIT: Duration = Duration::from_secs(60);
const SLOT: u64 = 4 * KIB;
const RING_SLOTS: u64 = skyzfs_uberblock::RING_SIZE / SLOT;

pub type Store = FaultStore<Recording>;
pub type Refs = Arc<ObjectRefStore<Store>>;
pub type Remote = EngineRemote<Store, Refs>;
pub type Up = Driver<InPlace<SimDisk>, Remote>;

/// Small segments, so that a cut seals several and PUTs them before its
/// commit, and shards of 256 KiB.
pub fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 16 * KIB,
        shard_bytes: 256 * KIB,
        ..EngineConfig::default()
    }
}

/// A death at a store of the cut state's region is the daemon's: the
/// tests catch it, and its panic says nothing.
pub fn quiet_deaths() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<Lost>().is_none() {
                default(info);
            }
        }));
    });
}

/// The volume, its store's cut state, and its object storage.
pub struct Fixture {
    pub disk: SimDisk,
    pub region: MemRegion,
    /// The volume as written so far.
    pub now: Vec<u8>,
    /// The volume at each cut's instant, by the cut's group.
    pub at: BTreeMap<u64, Vec<u8>>,
    next_id: u64,
    /// Object storage, as the uploader reaches it.
    pub store: Store,
    pub vol: VolumeId,
}

impl Fixture {
    /// A volume of zeros, on a disk of `seed` that cannot say where a
    /// file's holes are, and its volume in object storage with no commit
    /// but the empty one.
    pub async fn new(seed: u64) -> Self {
        Self::on(seed, false).await
    }

    /// [`Fixture::new`], on a disk that says where a file's holes are: the
    /// image, made sparse, is all holes when its first state begins
    /// (M2.4e).
    pub async fn sparse(seed: u64) -> Self {
        Self::on(seed, true).await
    }

    async fn on(seed: u64, holes: bool) -> Self {
        let disk = SimDisk::new(seed);
        disk.set_seek_data(holes);
        disk.set_write_unit(BLOCK);
        disk.set_page_size(BLOCK);
        disk.set_block_size(Some(BLOCK));
        let config = VolumeConfig {
            volume: *b"upload's volume.",
            size_mib: 1,
            base: Base::Empty,
            write_unit: 4096,
        };
        create(&disk, &config).expect("the volume");
        let region = MemRegion::new(words_for(BLOCKS, records_for(BOUND)));
        let store = FaultStore::new(Recording::default());
        let vol = VolumeId(ulid::Ulid::from(seed as u128 + 1));
        let refs = Arc::new(ObjectRefStore::new(store.clone()));
        Engine::create_volume(store.clone(), refs, vol, SIZE, cfg())
            .await
            .expect("the volume in object storage");
        Self {
            disk,
            region,
            now: vec![0; SIZE as usize],
            at: BTreeMap::new(),
            next_id: 1,
            store,
            vol,
        }
    }

    /// Start the store, taking the device over: a start after the
    /// daemon before it died takes up its cut state.
    pub fn start(&self) -> InPlace<SimDisk> {
        let options = OpenOptions {
            hold_cuts: Some(HoldCuts {
                region: CutRegion::Memory(self.region.clone()),
                boot: BOOT,
                bound: Some(BOUND),
                intent_region: Some(INTENT_REGION),
                intent_header_sync: false,
            }),
            ..OpenOptions::default()
        };
        let mut opened = Opened::open(
            self.disk.clone(),
            lock(&self.disk).expect("the lock"),
            options,
        )
        .map_err(|(e, _)| e)
        .expect("opens");
        opened.device_taken_over();
        opened.start(None).expect("the in-place store")
    }

    /// Object storage, through a refs object of its own over the store.
    pub fn remote(&self) -> Remote {
        let refs = Arc::new(ObjectRefStore::new(self.store.clone()));
        EngineRemote::new(self.store.clone(), refs, self.vol, cfg())
    }

    /// A driver of `store`, slices of `blocks` blocks.
    pub fn driver(&self, store: InPlace<SimDisk>, blocks: usize) -> Up {
        Driver::with_slices(store, self.remote(), blocks)
    }

    /// The daemon died: its store and its driver are gone, and the next
    /// start takes the device over.
    pub fn died(&self, driver: Up) {
        drop(driver);
        self.disk.kill();
        self.region.revive();
    }

    fn run(&mut self, store: &mut InPlace<SimDisk>, op: Op) {
        let id = RequestId(self.next_id);
        self.next_id += 1;
        run(store, id, op);
    }

    /// The host rebooted: the daemon died, the cut state's region on its
    /// memory file system is lost, and so is the intent map's file (as
    /// for a volume that had none): the next start begins a state with
    /// nothing known, whose first cut holds the whole volume.
    pub fn rebooted(&mut self, driver: Up) {
        drop(driver);
        self.disk.kill();
        self.region = MemRegion::new(words_for(BLOCKS, records_for(BOUND)));
        // The intent map's file, as the store names it.
        self.disk
            .remove("intent.map")
            .expect("the intent map removed");
    }

    /// Write block `block` full of `byte`.
    pub fn write(&mut self, store: &mut InPlace<SimDisk>, block: u64, byte: u8) {
        self.wrote(block, byte);
        self.run(store, write_op(block, byte));
    }

    /// Block `block` was written full of `byte` elsewhere ([`Racing`]).
    pub fn wrote(&mut self, block: u64, byte: u8) {
        let at = (block * BLOCK) as usize;
        self.now[at..at + BLOCK as usize].fill(byte);
    }

    /// Group `txg`'s uberblock to the four labels, then the flush after
    /// it: the cut of group `txg`, whose instant is the volume now.
    pub fn group(&mut self, store: &mut InPlace<SimDisk>, txg: u64) {
        self.labels(store, txg);
        self.run(store, Op::Flush);
        self.at.insert(txg, self.now.clone());
    }

    /// Group `txg`'s uberblock to the four labels, and no flush: the
    /// flush after it takes the cut of group `txg`.
    pub fn labels(&mut self, store: &mut InPlace<SimDisk>, txg: u64) {
        let labels = Labels::for_size(SIZE);
        let rings: Vec<u64> = labels.rings().map(|ring| ring.start).collect();
        for ring in rings {
            let offset = ring + (txg % RING_SLOTS) * SLOT;
            let mut slot = uberblock(txg);
            seal(&mut slot, offset, ByteOrder::Little);
            let at = offset as usize;
            self.now[at..at + SLOT as usize].copy_from_slice(&slot);
            self.run(store, Op::Write { offset, data: slot });
        }
    }

    /// The volume in object storage, read through an engine opened now
    /// on the store under the faults.
    pub async fn uploaded(&self) -> Uploaded {
        let inner = self.store.inner().clone();
        let refs = ObjectRefStore::new(inner.clone());
        let row = refs
            .meta_get(self.vol, ROW_KEY)
            .await
            .expect("the row")
            .map(|bytes| row::decode(&bytes).expect("a row of version 1"));
        let engine = Engine::open(inner.clone(), refs, self.vol, cfg())
            .await
            .expect("the engine at head");
        let content = engine.read(0, SIZE).await.expect("the volume");
        let fsck = {
            let refs = ObjectRefStore::new(inner.clone());
            fsck(&inner, &refs, self.vol).await.expect("fsck").ok()
        };
        Uploaded {
            content,
            group: engine.head_group(),
            row,
            fsck,
        }
    }

    /// Head's commit is the cut of group `txg`, named so by the row, and
    /// the volume passes fsck.
    pub async fn assert_uploaded(&self, txg: u64, case: &str) -> Uploaded {
        let uploaded = self.uploaded().await;
        assert_eq!(uploaded.group, Some(txg), "{case}: head's group");
        assert_eq!(
            uploaded.row.map(|row| row.group),
            Some(txg),
            "{case}: the row's group"
        );
        assert!(
            uploaded.content == self.at[&txg],
            "{case}: head does not hold the volume at the instant of the cut of group {txg}{}",
            differing(&uploaded.content, &self.at[&txg])
        );
        assert!(uploaded.fsck, "{case}: fsck");
        uploaded
    }

    /// The bytes of the volume's segments in object storage.
    pub async fn segment_bytes(&self) -> u64 {
        let inner = self.store.inner().clone();
        let prefix = format!("{}seg/", skynbd_engine::keys::volume_prefix(self.vol));
        let mut bytes = 0;
        for key in inner.list(&prefix).await.expect("the segments") {
            bytes += inner.get(&key, None).await.expect("a segment").len() as u64;
        }
        bytes
    }

    /// Another writer swaps the row to `value`, leaving head as it is.
    pub async fn put_row(&self, value: Vec<u8>) {
        let refs = ObjectRefStore::new(self.store.inner().clone());
        refs.transact(
            self.vol,
            vec![RefOp::MetaPut {
                key: ROW_KEY.into(),
                value,
            }],
        )
        .await
        .expect("the row put");
    }

    /// Another writer commits a block onto head, with no group and no
    /// row: an operator's write, say.
    pub async fn move_head(&self) {
        let inner = self.store.inner().clone();
        let refs = ObjectRefStore::new(inner.clone());
        let mut engine = Engine::open(inner, refs, self.vol, cfg())
            .await
            .expect("the engine");
        engine.write(0, &[0xAB; BLOCK as usize]).expect("a write");
        engine
            .commit(skynbd_format::CommitKind::Flush)
            .await
            .expect("another writer's commit");
    }
}

/// Run `op` on `store` as request `id`, and wait for it.
fn run(store: &mut InPlace<SimDisk>, id: RequestId, op: Op) {
    store.submit(Request { id, op }).expect("submitted");
    let mut done = Vec::new();
    while done.is_empty() {
        store.wait(&mut done, WAIT);
    }
    let done = done.pop().expect("a completion");
    assert_eq!(done.id, id);
    done.result.expect("the request");
}

/// A write of block `block` full of `byte`.
fn write_op(block: u64, byte: u8) -> Op {
    Op::Write {
        offset: block * BLOCK,
        data: Buf::filled(1, byte),
    }
}

/// The store as the daemon's queue's thread answers the driver (M2.4c):
/// between its answers it serves the pool, so the bound can take a cut
/// between the check that found it committed and its release. The
/// writes in `at_release` are made just before the next release is
/// answered; the test tells the fixture of them ([`Fixture::wrote`]).
pub struct Racing {
    pub store: InPlace<SimDisk>,
    pub at_release: Vec<(u64, u8)>,
}

impl Local for Racing {
    async fn uploadable(&mut self) -> Option<Uploadable> {
        self.store.uploadable().await
    }

    async fn read(&mut self, number: u64, from: u64, buf: Buf) -> (Buf, io::Result<CutSlice>) {
        Local::read(&mut self.store, number, from, buf).await
    }

    async fn sync_copies(&mut self, number: u64) -> io::Result<()> {
        self.store.sync_copies(number).await
    }

    async fn release(&mut self, number: u64) -> Released {
        for (i, (block, byte)) in std::mem::take(&mut self.at_release).into_iter().enumerate() {
            run(
                &mut self.store,
                RequestId((1 << 40) | i as u64),
                write_op(block, byte),
            );
        }
        self.store.release(number).await
    }

    async fn name_committed(&mut self, number: u64, group: u64) -> bool {
        self.store.name_committed(number, group).await
    }

    async fn abandon_all(&mut self) -> Vec<u64> {
        self.store.abandon_all().await
    }

    async fn cuts(&mut self) -> skyzfs_upload::CutsNow {
        skyzfs_upload::Local::cuts(&mut self.store).await
    }
}

/// The blocks of data cut 2 holds in [`committed_unheard`]: more than the
/// bound has room for, so that overwriting them all abandons it.
pub const OVER: u64 = BOUND / BLOCK + 8;

/// Cut 1 (group 7) goes up whole. Cut 2 (group 8) holds [`OVER`] blocks
/// from block 100; its head swap lands and its reply is lost, so that the
/// driver stays at the commit.
pub async fn committed_unheard(fx: &mut Fixture) -> Up {
    let mut store = fx.start();
    fx.group(&mut store, 7);
    let mut up = fx.driver(store, 64);
    up.run().await.expect("cut 1");
    for block in 0..OVER {
        fx.write(up.local(), 100 + block, 0x22);
    }
    fx.group(up.local(), 8);
    for _ in 0..100 {
        if up.doing() == Doing::Committing {
            break;
        }
        up.step().await.expect("a step of cut 2");
    }
    assert_eq!(up.doing(), Doing::Committing, "cut 2 read");
    // The refs object's PUT is the head swap.
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, FaultWhen::After)
        .await;
    assert!(
        up.step().await.is_err(),
        "the reply to cut 2's swap is lost"
    );
    fx.store.disarm().await;
    assert_eq!(fx.uploaded().await.group, Some(8), "cut 2's swap landed");
    up
}

/// The bound takes a cut after its commit has landed and before it is
/// let go (P1 of the review of M2.4b-1): after [`committed_unheard`],
/// cut 2's blocks are overwritten, which abandons it at the bound and
/// hands its map on, and group 9 makes cut 3 with it. The driver's next
/// step reads the row and head again and finds cut 2 committed.
pub async fn committed_then_taken(fx: &mut Fixture) -> Up {
    let mut up = committed_unheard(fx).await;
    for block in 0..OVER {
        fx.write(up.local(), 100 + block, 0x33);
    }
    fx.group(up.local(), 9);
    let held: Vec<u64> = up
        .local()
        .held_cuts()
        .iter()
        .map(|cut| cut.number)
        .collect();
    assert_eq!(held, [3], "the bound took cut 2, and cut 3 took its map");
    up
}

/// What object storage holds.
#[derive(Debug)]
pub struct Uploaded {
    pub content: Vec<u8>,
    /// The group of head's commit.
    pub group: Option<u64>,
    pub row: Option<CommittedCut>,
    pub fsck: bool,
}

/// The first block where two images differ, said for a message.
pub fn differing(a: &[u8], b: &[u8]) -> String {
    match a
        .chunks(BLOCK as usize)
        .zip(b.chunks(BLOCK as usize))
        .position(|(x, y)| x != y)
    {
        Some(block) => format!(" (block {block} differs first)"),
        None => String::new(),
    }
}

/// A 4 KiB slot holding an uberblock of group `txg`, little endian.
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

/// A store in memory that writes down each operation while recording:
/// its kind and the class of its key ("put seg", "put_if refs"), as the
/// engine's crash test does (M2.4a).
#[derive(Clone, Default)]
pub struct Recording {
    inner: MemoryStore,
    log: Arc<Mutex<Vec<String>>>,
    on: Arc<AtomicBool>,
}

impl Recording {
    /// Record from now on, or no more.
    pub fn record(&self, on: bool) {
        self.on.store(on, Ordering::SeqCst);
    }

    /// The operations recorded.
    pub fn log(&self) -> Vec<String> {
        let log = self.log.lock().expect("the log");
        log.clone()
    }

    fn note(&self, op: &str, key: &str) {
        if !self.on.load(Ordering::SeqCst) {
            return;
        }
        let class = ["/seg/", "/node/", "/commit/", "/refs", "/sb", "/emap/"]
            .iter()
            .find(|class| key.contains(*class))
            .map_or("?", |class| class.trim_matches('/'));
        self.log
            .lock()
            .expect("the log")
            .push(format!("{op} {class}"));
    }
}

#[async_trait::async_trait]
impl ObjectStore for Recording {
    async fn put(&self, key: &str, data: Bytes) -> Result<(), StoreError> {
        self.note("put", key);
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes, StoreError> {
        self.note("get", key);
        self.inner.get(key, range).await
    }
    async fn exists(&self, key: &str) -> Result<bool, StoreError> {
        self.note("exists", key);
        self.inner.exists(key).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.note("list", prefix);
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        self.note("delete", key);
        self.inner.delete(key).await
    }
    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String), StoreError> {
        self.note("get_versioned", key);
        self.inner.get_versioned(key).await
    }
    async fn put_if(
        &self,
        key: &str,
        data: Bytes,
        pre: Precondition,
    ) -> Result<String, StoreError> {
        self.note("put_if", key);
        self.inner.put_if(key, data, pre).await
    }
}

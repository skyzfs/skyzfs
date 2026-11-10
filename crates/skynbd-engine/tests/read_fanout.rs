//! Issue #97 items 4–5: a read's store-resident pieces are coalesced and
//! fetched concurrently (bounded by `read_fanout`); tree walks and
//! `ensure` honour `walk_concurrency`.

use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use skynbd_engine::prepared::PreparedSession;
use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore, Result};
use skynbd_types::{SnapId, VolumeId};

const KIB: u64 = 1024;
const VOL: u64 = 1 << 20;

/// Delays data GETs and `exists` probes by a few ms and tracks how many
/// are in flight at once (high-water) and in total.
#[derive(Default)]
struct Probe {
    inner: MemoryStore,
    in_flight: AtomicUsize,
    high_water: AtomicUsize,
    data_gets: AtomicUsize,
    probes: AtomicUsize,
    /// Node GETs, tracked apart (tree walks).
    node_in_flight: AtomicUsize,
    node_high_water: AtomicUsize,
}

impl Probe {
    async fn track<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.high_water.fetch_max(now, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(5)).await;
        let out = f.await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        out
    }

    fn reset(&self) {
        self.node_high_water.store(0, Ordering::SeqCst);
        self.high_water.store(0, Ordering::SeqCst);
        self.data_gets.store(0, Ordering::SeqCst);
        self.probes.store(0, Ordering::SeqCst);
    }
}

fn is_data(key: &str) -> bool {
    key.contains("/seg/") || key.contains("/obj/")
}

#[async_trait::async_trait]
impl ObjectStore for Probe {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.inner.put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        if key.contains("/node/") {
            let now = self.node_in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            self.node_high_water.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(5)).await;
            let out = self.inner.get(key, range).await;
            self.node_in_flight.fetch_sub(1, Ordering::SeqCst);
            return out;
        }
        if !is_data(key) {
            return self.inner.get(key, range).await;
        }
        self.data_gets.fetch_add(1, Ordering::SeqCst);
        self.track(self.inner.get(key, range)).await
    }
    async fn exists(&self, key: &str) -> Result<bool> {
        self.probes.fetch_add(1, Ordering::SeqCst);
        self.track(self.inner.exists(key)).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix).await
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await
    }
}

fn block(i: u64) -> Vec<u8> {
    (0..4 * KIB).map(|b| (i * 31 + b * 7) as u8).collect()
}

/// A committed volume of `blocks` sequential 4 KiB blocks, reopened so
/// every byte is store-resident, with the touched shard warmed.
async fn committed(
    blocks: u64,
    segment_max_bytes: u64,
    shard_bytes: u64,
    read_fanout: usize,
) -> (Arc<Probe>, Engine<Arc<Probe>, Arc<dyn RefStore>>) {
    let store = Arc::new(Probe::default());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::from(9u128));
    let cfg = EngineConfig {
        segment_max_bytes,
        shard_bytes,
        read_fanout,
        ..EngineConfig::default()
    };
    let mut e = Engine::create_volume(Arc::clone(&store), Arc::clone(&refs), vol, VOL, cfg.clone())
        .await
        .unwrap();
    for i in 0..blocks {
        e.write(i * 4 * KIB, &block(i)).unwrap();
    }
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);
    let e = Engine::open(Arc::clone(&store), refs, vol, cfg)
        .await
        .unwrap();
    e.read(0, 512).await.unwrap(); // materialize the shard
    store.reset();
    (store, e)
}

fn expected(blocks: u64) -> Vec<u8> {
    (0..blocks).flat_map(block).collect()
}

/// One block per segment: a 16-segment read keeps the GETs in flight
/// together, never more than `read_fanout` of them, and at fan-out 1 is
/// strictly serial. Every byte is right either way.
#[tokio::test(flavor = "multi_thread")]
async fn cold_read_across_segments_fans_out_within_the_bound() {
    for (fanout, want_hw) in [(16, 16), (4, 4), (1, 1), (0, 1)] {
        let (store, e) = committed(16, 4 * KIB, 1 << 30, fanout).await;
        let got = e.read(0, 64 * KIB).await.unwrap();
        assert_eq!(got, expected(16), "fan-out {fanout}");
        assert_eq!(
            store.data_gets.load(Ordering::SeqCst),
            16,
            "one GET per segment"
        );
        assert_eq!(
            store.high_water.load(Ordering::SeqCst),
            want_hw,
            "fan-out {fanout}: GETs in flight at once"
        );
    }
}

/// One run of one segment, split by a 16 KiB shard geometry into four
/// committed extents that continue each other in the object: the read is
/// still a single ranged GET — including one that starts and ends
/// mid-block, across a boundary.
#[tokio::test(flavor = "multi_thread")]
async fn pieces_contiguous_in_their_object_coalesce_into_one_get() {
    let (store, e) = committed(16, 8 << 20, 16 * KIB, 16).await;
    assert_eq!(e.read(0, 64 * KIB).await.unwrap(), expected(16));
    assert_eq!(store.data_gets.load(Ordering::SeqCst), 1);

    store.reset();
    let (a, b) = (6 * KIB + 512, 50 * KIB - 512);
    let got = e.read(a, b - a).await.unwrap();
    assert_eq!(got, expected(16)[a as usize..b as usize]);
    assert_eq!(store.data_gets.load(Ordering::SeqCst), 1);
}

/// Un-committed writes in the middle of a cold range are served from
/// memory and split the coalesced GET in two; holes stay zero.
#[tokio::test(flavor = "multi_thread")]
async fn overlay_and_holes_interleave_with_cold_fetches() {
    let (store, mut e) = committed(8, 8 << 20, 1 << 30, 16).await;
    let fresh = vec![0xEE; 8 * KIB as usize];
    e.write(2 * 4 * KIB, &fresh).unwrap(); // blocks 2..4, uncommitted
    store.reset();
    let got = e.read(0, 48 * KIB).await.unwrap();
    let mut want = expected(8);
    want[8 * KIB as usize..16 * KIB as usize].copy_from_slice(&fresh);
    want.resize(48 * KIB as usize, 0); // 32..48 KiB: never written
    assert_eq!(got, want);
    assert_eq!(
        store.data_gets.load(Ordering::SeqCst),
        2,
        "blocks 0..2 and 4..8, one GET each"
    );
}

/// `ensure`'s existence checks honour the walk fan-out the serving
/// volume configures.
#[tokio::test(flavor = "multi_thread")]
async fn ensure_honours_walk_concurrency() {
    for (walk, want_hw) in [(32, 32), (3, 3), (1, 1)] {
        let store = Arc::new(Probe::default());
        let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
        let vol = VolumeId(ulid::Ulid::from(11u128));
        Engine::create_volume(
            Arc::clone(&store),
            Arc::clone(&refs),
            vol,
            VOL,
            EngineConfig::default(),
        )
        .await
        .unwrap();
        let mut p = PreparedSession::create(
            Arc::clone(&store),
            refs,
            vol,
            SnapId(ulid::Ulid::new()),
            None,
        )
        .await
        .unwrap()
        .with_walk_concurrency(walk);
        store.reset();
        let hashes: Vec<[u8; 32]> = (0..64u8).map(|i| [i; 32]).collect();
        assert_eq!(p.ensure(&hashes).await.unwrap(), vec![false; 64]);
        assert_eq!(store.probes.load(Ordering::SeqCst), 64);
        assert_eq!(
            store.high_water.load(Ordering::SeqCst),
            want_hw,
            "walk {walk}"
        );
    }
}

/// A fresh store-backed engine over `store`, with `cfg`.
async fn fresh(
    store: &Arc<Probe>,
    cfg: EngineConfig,
) -> (
    Engine<Arc<Probe>, Arc<dyn RefStore>>,
    Arc<dyn RefStore>,
    VolumeId,
) {
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol = VolumeId(ulid::Ulid::new());
    let e = Engine::create_volume(Arc::clone(store), Arc::clone(&refs), vol, VOL, cfg)
        .await
        .unwrap();
    (e, refs, vol)
}

/// The object-offset half of the merge condition: blocks written out of
/// order land in one segment reversed, so pieces adjacent in the output
/// are not adjacent in the object — two GETs, and the right bytes.
#[tokio::test(flavor = "multi_thread")]
async fn pieces_not_contiguous_in_their_object_do_not_merge() {
    let store = Arc::new(Probe::default());
    let (mut e, refs, vol) = fresh(&store, EngineConfig::default()).await;
    e.write(4 * KIB, &block(1)).unwrap(); // segment offset 0
    e.write(0, &block(0)).unwrap(); // segment offset 4 KiB
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);
    let e = Engine::open(Arc::clone(&store), refs, vol, EngineConfig::default())
        .await
        .unwrap();
    e.read(16 * KIB, 512).await.unwrap(); // materialize
    store.reset();
    assert_eq!(e.read(0, 8 * KIB).await.unwrap(), expected(2));
    assert_eq!(store.data_gets.load(Ordering::SeqCst), 2);
}

/// The object half: back-to-back ranges of two DIFFERENT CAS objects,
/// whose offsets happen to continue each other, are two GETs.
#[tokio::test(flavor = "multi_thread")]
async fn pieces_of_different_objects_do_not_merge() {
    let store = Arc::new(Probe::default());
    let (mut e, refs, vol) = fresh(&store, EngineConfig::default()).await;
    let (a, b) = (vec![0xAA; 8 * KIB as usize], vec![0xBB; 8 * KIB as usize]);
    let (ha, hb) = (*blake3::hash(&a).as_bytes(), *blake3::hash(&b).as_bytes());
    for (h, d) in [(ha, &a), (hb, &b)] {
        store
            .put(
                &skynbd_engine::keys::cas_object(vol, &h),
                Bytes::copy_from_slice(d),
            )
            .await
            .unwrap();
    }
    e.attach_cas(0, 4 * KIB, ha, 0).unwrap();
    e.attach_cas(4 * KIB, 4 * KIB, hb, 4 * KIB).unwrap();
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);
    let e = Engine::open(Arc::clone(&store), refs, vol, EngineConfig::default())
        .await
        .unwrap();
    e.read(16 * KIB, 512).await.unwrap(); // materialize
    store.reset();
    let got = e.read(0, 8 * KIB).await.unwrap();
    assert_eq!(&got[..4 * KIB as usize], &a[..4 * KIB as usize]);
    assert_eq!(&got[4 * KIB as usize..], &b[4 * KIB as usize..]);
    assert_eq!(store.data_gets.load(Ordering::SeqCst), 2);
}

/// `max_read_gets` caps data GETs across ALL of an engine's reads: two
/// concurrent 16-segment reads at fan-out 16 hold at most 4 at once.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_reads_share_the_engine_wide_get_cap() {
    let store = Arc::new(Probe::default());
    let cfg = EngineConfig {
        segment_max_bytes: 4 * KIB,
        read_fanout: 16,
        max_read_gets: 4,
        ..EngineConfig::default()
    };
    let (mut e, refs, vol) = fresh(&store, cfg.clone()).await;
    for i in 0..32 {
        e.write(i * 4 * KIB, &block(i)).unwrap();
    }
    e.commit(CommitKind::Flush).await.unwrap();
    drop(e);
    let e = Engine::open(Arc::clone(&store), refs, vol, cfg)
        .await
        .unwrap();
    e.read(0, 512).await.unwrap(); // materialize
    store.reset();
    let (a, b) = tokio::join!(e.read(0, 64 * KIB), e.read(64 * KIB, 64 * KIB));
    assert_eq!([a.unwrap(), b.unwrap()].concat(), expected(32));
    assert_eq!(store.data_gets.load(Ordering::SeqCst), 32);
    assert_eq!(store.high_water.load(Ordering::SeqCst), 4);
}

/// `EngineConfig::walk_concurrency` reaches the engine's walks: opening
/// with width 3 and materializing a shard whose subtree is several
/// leaves keeps at most 3 node GETs in flight (and the default keeps
/// more), with no other knob set.
#[tokio::test(flavor = "multi_thread")]
async fn engine_config_walk_concurrency_bounds_shard_materialization() {
    let mut seen = Vec::new();
    for walk in [3usize, 32] {
        let store = Arc::new(Probe::default());
        let cfg = EngineConfig {
            walk_concurrency: walk,
            ..EngineConfig::default()
        };
        let (mut e, refs, vol) = fresh(&store, cfg.clone()).await;
        // 1024 separated 512 B extents: well past one leaf's 256 entries.
        for i in 0..1024u64 {
            e.write(i * KIB, &[i as u8; 512]).unwrap();
        }
        e.commit(CommitKind::Flush).await.unwrap();
        drop(e);
        let e = Engine::open(Arc::clone(&store), refs, vol, cfg)
            .await
            .unwrap();
        store.reset();
        e.read(0, 512).await.unwrap(); // materialize shard 0
        seen.push(store.node_high_water.load(Ordering::SeqCst));
    }
    assert_eq!(seen[0], 3, "walk 3: node GETs in flight {seen:?}");
    assert!(seen[1] > 3, "the default walk never fanned out: {seen:?}");
}

/// Engines opened from one config with a shared pool
/// ([`EngineConfig::share_read_gets`], carried by `tuning()`) share ONE
/// cap: a head engine and a snapshot-style reader reading at once hold at
/// most 4 GETs together, where separate pools would hold 8.
#[tokio::test(flavor = "multi_thread")]
async fn engines_sharing_a_pool_share_one_get_cap() {
    let mut seen = Vec::new();
    for shared in [true, false] {
        let store = Arc::new(Probe::default());
        let base = EngineConfig {
            segment_max_bytes: 4 * KIB,
            read_fanout: 16,
            max_read_gets: 4,
            ..EngineConfig::default()
        };
        let (mut e, refs, vol) = fresh(&store, base.clone()).await;
        for i in 0..32 {
            e.write(i * 4 * KIB, &block(i)).unwrap();
        }
        let head = e.commit(CommitKind::Flush).await.unwrap();
        drop(e);
        let cfg = if shared { base.share_read_gets() } else { base };
        let a = Engine::open(Arc::clone(&store), Arc::clone(&refs), vol, cfg.clone())
            .await
            .unwrap();
        let b = Engine::open_at_commit(Arc::clone(&store), refs, vol, head, cfg.tuning())
            .await
            .unwrap();
        a.read(0, 512).await.unwrap(); // materialize both
        b.read(0, 512).await.unwrap();
        store.reset();
        let (x, y) = tokio::join!(a.read(0, 128 * KIB), b.read(0, 128 * KIB));
        assert_eq!(x.unwrap(), expected(32));
        assert_eq!(y.unwrap(), expected(32));
        seen.push(store.high_water.load(Ordering::SeqCst));
    }
    assert_eq!(
        seen,
        vec![4, 8],
        "GETs in flight [shared pool, separate pools]"
    );
}

/// A volume of 1024 separated 512 B extents — a shard subtree of several
/// leaves — committed twice, so a walk of either commit fans out.
async fn many_leaves(
    store: &Arc<Probe>,
) -> (
    Arc<dyn RefStore>,
    VolumeId,
    skynbd_types::CommitHash,
    skynbd_types::CommitHash,
) {
    let (mut e, refs, vol) = fresh(store, EngineConfig::default()).await;
    for i in 0..1024u64 {
        e.write(i * KIB, &[i as u8; 512]).unwrap();
    }
    let c1 = e.commit(CommitKind::Flush).await.unwrap();
    for i in 0..1024u64 {
        e.write(i * KIB, &[!(i as u8); 512]).unwrap();
    }
    let c2 = e.commit(CommitKind::Flush).await.unwrap();
    (refs, vol, c1, c2)
}

/// `fsck_with` and `recover_head_with` walk at the width they are given
/// (the API's fsck and the provisioner's serve-time recovery pass the
/// volume's `walk_concurrency`), and the defaults walk wider.
#[tokio::test(flavor = "multi_thread")]
async fn fsck_and_recovery_walk_at_the_width_they_are_given() {
    for (walk, exact) in [(3usize, true), (skynbd_engine::WALK_CONCURRENCY, false)] {
        let store = Arc::new(Probe::default());
        let (refs, vol, c1, c2) = many_leaves(&store).await;

        store.reset();
        let report = skynbd_engine::fsck_with(&store, &refs, vol, walk)
            .await
            .unwrap();
        assert!(report.ok(), "{:?}", report.refs);
        let fsck = store.node_high_water.load(Ordering::SeqCst);

        // Roll head back one commit, as a crash between the commit's PUT
        // and its ref CAS leaves it: recovery walks c2's closure.
        let head = refs.get(vol, "head").await.unwrap().unwrap();
        refs.compare_and_swap(vol, "head", Some(head), Some(c1))
            .await
            .unwrap();
        store.reset();
        let adopted = skynbd_engine::recover_head_with(&store, &refs, vol, 0, walk)
            .await
            .unwrap();
        assert_eq!(adopted, Some(c2));
        let recover = store.node_high_water.load(Ordering::SeqCst);

        if exact {
            assert_eq!(
                (fsck, recover),
                (walk, walk),
                "walk {walk}: [fsck, recovery]"
            );
        } else {
            assert!(fsck > 3 && recover > 3, "default walk: [{fsck}, {recover}]");
        }
    }
}

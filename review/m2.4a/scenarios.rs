//! #110 at dcf2df3 (M2.4a): the reviewer's scenarios, run as a test file of
//! skynbd-engine (scenarios.py copies it into the checkout as
//! tests/zz_review_scenarios.rs, runs it with --nocapture and removes it).
//! They print what the head does; the asserts are only those that hold on
//! the head, so that a line of output is a fact and not a failure.

use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use skynbd_engine::{fsck, keys, Engine, EngineConfig, EngineError};
use skynbd_format::{Codec, Commit, CommitKind};
use skynbd_refs::{MemoryRefStore, ObjectRefStore, RedbRefStore, RefOp, RefStore};
use skynbd_store::{
    FaultOp, FaultStore, FaultWhen, MemoryStore, ObjectStore, Precondition, StoreError,
};
use skynbd_types::{CommitHash, VolumeId};

const KIB: u64 = 1024;
const BLOCK: u64 = 4 * KIB;
const CUT_ROW: &str = "skyzfs/cut";

fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: BLOCK,
        shard_bytes: 64 * KIB,
        ..EngineConfig::default()
    }
}

fn vol(n: u128) -> VolumeId {
    VolumeId(ulid::Ulid::from(n))
}

fn block(seed: u8) -> Vec<u8> {
    (0..BLOCK as usize)
        .map(|i| seed.wrapping_add(i as u8))
        .collect()
}

fn row(value: &[u8]) -> RefOp {
    RefOp::MetaPut {
        key: CUT_ROW.into(),
        value: value.to_vec(),
    }
}

async fn commit_of<S: ObjectStore>(store: &S, v: VolumeId, hash: &CommitHash) -> Commit {
    Commit::decode(&store.get(&keys::commit(v, hash), None).await.unwrap()).unwrap()
}

/// A store that writes down every operation, and can hold a PUT of a
/// segment for ever (a request that never answers).
#[derive(Clone)]
struct Recording {
    inner: MemoryStore,
    log: Arc<Mutex<Vec<String>>>,
    /// Hold the PUT of a segment once this many have passed.
    hold_after: Arc<Mutex<Option<usize>>>,
    seg_puts: Arc<Mutex<usize>>,
    on: Arc<AtomicBool>,
}

impl Recording {
    fn new() -> Self {
        Self {
            inner: MemoryStore::new(),
            log: Arc::default(),
            hold_after: Arc::default(),
            seg_puts: Arc::default(),
            on: Arc::new(AtomicBool::new(false)),
        }
    }
    fn note(&self, op: &str, key: &str) {
        if !self.on.load(Ordering::SeqCst) {
            return;
        }
        let class = ["/seg/", "/node/", "/commit/", "/refs", "/sb", "/emap/"]
            .iter()
            .find(|c| key.contains(*c))
            .copied()
            .unwrap_or("?");
        self.log.lock().unwrap().push(format!("{op} {class}"));
    }
}

#[async_trait::async_trait]
impl ObjectStore for Recording {
    async fn put(&self, key: &str, data: Bytes) -> Result<(), StoreError> {
        self.note("put", key);
        if key.contains("/seg/") {
            let n = {
                let mut n = self.seg_puts.lock().unwrap();
                *n += 1;
                *n
            };
            let hold = *self.hold_after.lock().unwrap();
            if hold.is_some_and(|h| n > h) {
                std::future::pending::<()>().await;
            }
        }
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

/// The upload of the pull request's crash test, word for word.
async fn upload<S: ObjectStore, R: RefStore>(eng: &mut Engine<S, R>) -> Result<(), EngineError> {
    eng.write(0, &block(20))?;
    eng.write(2 * BLOCK, &block(21))?;
    eng.put_sealed().await?;
    eng.write_zeroes(BLOCK, BLOCK)?;
    eng.write(5 * BLOCK, &block(22))?;
    eng.write(80 * KIB, &block(23))?;
    eng.put_sealed().await?;
    eng.set_group(Some(41));
    eng.commit_with(CommitKind::Flush, vec![row(b"cut 1")])
        .await?;
    Ok(())
}

/// S3. Which store operations the crash test's upload makes, in order:
/// those are its kill points.
#[tokio::test]
async fn s3_the_operations_of_the_crash_tests_upload() {
    let store = Recording::new();
    let v = vol(4);
    let refs = Arc::new(ObjectRefStore::new(store.clone()));
    let mut eng = Engine::create_volume(store.clone(), refs, v, 96 * KIB, cfg())
        .await
        .unwrap();
    for i in 0..4u8 {
        eng.write(u64::from(i) * BLOCK, &block(10 + i)).unwrap();
    }
    eng.commit(CommitKind::Flush).await.unwrap();
    store.on.store(true, Ordering::SeqCst);
    upload(&mut eng).await.unwrap();
    store.on.store(false, Ordering::SeqCst);
    let log = store.log.lock().unwrap().clone();
    println!("S3 the upload's {} operations: {}", log.len(), log.join(", "));
    // The commit starts after the fourth PUT of a segment.
    let after_put_sealed = log.iter().skip(4).filter(|l| *l == "put /seg/").count();
    println!("S3 PUTs of a segment made by the commit itself: {after_put_sealed}");
}

/// S1. The group outlives a discard of the writes it was set for.
#[tokio::test]
async fn s1_a_group_outlives_the_discard_of_its_writes() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(101);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    eng.write(0, &block(1)).unwrap();
    let h0 = eng.commit(CommitKind::Flush).await.unwrap();

    // The upload of the cut of group 41 begins and is given up.
    eng.set_group(Some(41));
    eng.write(0, &block(2)).unwrap();
    eng.put_sealed().await.unwrap();
    eng.discard_uncommitted();
    println!("S1 after the discard: is_clean() = {}", eng.is_clean());

    // Any commit after it, with no group set for it.
    let h1 = eng.commit(CommitKind::Flush).await.unwrap();
    let c1 = commit_of(&store, v, &h1).await;
    println!(
        "S1 a commit after the discard, no group set for it: a new commit = {}, its group = {:?}, \
         head_group() = {:?}, block 0 is the old content = {}",
        h1 != h0,
        c1.group,
        eng.head_group(),
        eng.read(0, BLOCK).await.unwrap() == block(1)
    );

    // The same through a snapshot: it forces a commit.
    eng.set_group(Some(42));
    eng.write(0, &block(3)).unwrap();
    eng.discard_uncommitted();
    let (_, hs) = eng.snapshot_create("s").await.unwrap();
    println!(
        "S1 a snapshot after a discard: its commit's group = {:?}",
        commit_of(&store, v, &hs).await.group
    );

    // And a rollback: the group set before it goes into the first commit
    // after it.
    eng.set_group(Some(43));
    eng.rollback("s").await.unwrap();
    eng.write(BLOCK, &block(4)).unwrap();
    let hr = eng.commit(CommitKind::Flush).await.unwrap();
    println!(
        "S1 a commit after a rollback: its group = {:?}",
        commit_of(&store, v, &hr).await.group
    );
}

/// S2. A put_sealed whose future is dropped while a PUT is outstanding
/// (a time limit around it): what the engine lists afterwards.
#[tokio::test]
async fn s2_a_put_sealed_dropped_at_a_put() {
    let store = Recording::new();
    let refs = MemoryRefStore::new();
    let v = vol(102);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    for i in 0..3u8 {
        eng.write(u64::from(i) * BLOCK, &block(i)).unwrap();
    }
    // The second PUT of a segment never answers.
    *store.hold_after.lock().unwrap() = Some(1);
    // Polled until it waits for that PUT, then dropped: what a time limit
    // around the call does.
    let ended = futures_util::FutureExt::now_or_never(eng.put_sealed());
    println!("S2 put_sealed dropped at its second PUT: it had ended = {}", ended.is_some());
    *store.hold_after.lock().unwrap() = None;
    println!(
        "S2 held bytes after it: {} (three segments of {BLOCK} were sealed, one was PUT)",
        eng.held_segment_bytes()
    );
    eng.put_sealed().await.unwrap();
    let hash = eng.commit(CommitKind::Flush).await.unwrap();
    let commit = commit_of(&store.inner, v, &hash).await;
    let mut ids: Vec<_> = commit.segments.iter().map(|s| s.id).collect();
    let listed = ids.len();
    ids.sort();
    ids.dedup();
    println!(
        "S2 the commit lists {listed} segments, {} of them distinct",
        ids.len()
    );
    for i in 0..3u8 {
        assert_eq!(
            eng.read(u64::from(i) * BLOCK, BLOCK).await.unwrap(),
            block(i)
        );
    }
    assert!(fsck(&store.inner, &refs, v).await.unwrap().ok());
    println!("S2 the content reads right and fsck passes");
}

/// S4. A commit that fails at each of its operations, before and after,
/// with segments already PUT and one still open, made again on the same
/// engine (no death): the pull request's tests make a commit again only
/// with nothing PUT early.
#[tokio::test]
async fn s4_a_failed_commit_made_again_on_the_same_engine() {
    for when in [FaultWhen::Before, FaultWhen::After] {
        let mut points = 0;
        let mut landed_unheard = 0;
        for k in 0.. {
            let store = FaultStore::new(MemoryStore::new());
            let v = vol(103);
            let refs = Arc::new(ObjectRefStore::new(store.clone()));
            let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 96 * KIB, cfg())
                .await
                .unwrap();
            eng.write(0, &block(1)).unwrap();
            eng.commit(CommitKind::Flush).await.unwrap();

            eng.write(0, &block(20)).unwrap();
            eng.write(2 * BLOCK, &block(21)).unwrap();
            eng.put_sealed().await.unwrap();
            eng.write(80 * KIB, &block(23)).unwrap();
            // Part of a block: the commit seals and PUTs it.
            eng.write(5 * BLOCK, &block(22)[..512]).unwrap();
            eng.set_group(Some(41));

            store.arm_when(FaultOp::Any, "", k, when).await;
            let first = eng
                .commit_with(CommitKind::Flush, vec![row(b"cut 1")])
                .await;
            store.disarm().await;
            let hash = match first {
                Ok(h) => h,
                Err(_) => {
                    points += 1;
                    match eng
                        .commit_with(CommitKind::Flush, vec![row(b"cut 1")])
                        .await
                    {
                        Ok(h) => h,
                        Err(EngineError::Conflict) => {
                            // The swap had landed and its reply was lost.
                            landed_unheard += 1;
                            refs.get(v, "head").await.unwrap().unwrap().commit
                        }
                        Err(e) => panic!("{when:?} at op {k}: the commit again: {e}"),
                    }
                }
            };
            let commit = commit_of(store.inner(), v, &hash).await;
            let mut ids: Vec<_> = commit.segments.iter().map(|s| s.id).collect();
            assert_eq!(ids.len(), 4, "{when:?} at op {k}: segments listed");
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), 4, "{when:?} at op {k}: each once");
            assert_eq!(commit.group, Some(41), "{when:?} at op {k}");
            drop(eng);
            let refs = Arc::new(ObjectRefStore::new(store.clone()));
            let eng = Engine::open(store.clone(), refs.clone(), v, cfg())
                .await
                .unwrap();
            assert_eq!(eng.read(0, BLOCK).await.unwrap(), block(20));
            assert_eq!(eng.read(2 * BLOCK, BLOCK).await.unwrap(), block(21));
            assert_eq!(eng.read(5 * BLOCK, 512).await.unwrap(), &block(22)[..512]);
            assert_eq!(eng.read(80 * KIB, BLOCK).await.unwrap(), block(23));
            assert_eq!(
                refs.meta_get(v, CUT_ROW).await.unwrap().as_deref(),
                Some(&b"cut 1"[..])
            );
            assert!(fsck(&store, &refs, v).await.unwrap().ok());
            if first.is_ok() {
                break;
            }
        }
        println!(
            "S4 {when:?}: the commit failed at each of {points} operations and was made again \
             on the same engine; it lists its four segments once each and reads right. \
             The swap had landed unheard at {landed_unheard} (the commit again is then a Conflict)"
        );
    }
}

/// S5. The head an engine holds after a commit with ops, on the two ref
/// stores a daemon can have: the commit after it swaps from that head.
#[tokio::test]
async fn s5_the_commit_after_a_commit_with_ops() {
    // The refs object in the object store.
    let store = MemoryStore::new();
    let v = vol(104);
    let refs = Arc::new(ObjectRefStore::new(store.clone()));
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    for i in 0..3u8 {
        eng.write(u64::from(i) * BLOCK, &block(i)).unwrap();
        eng.commit_with(CommitKind::Flush, vec![row(&[i])])
            .await
            .unwrap();
    }
    eng.write(0, &block(9)).unwrap();
    let h = eng.commit(CommitKind::Flush).await.unwrap();
    let head = refs.get(v, "head").await.unwrap().unwrap();
    assert_eq!(head.commit, h);
    println!(
        "S5 ObjectRefStore: three commits with ops and one without follow each other; \
         the head's generation is {}",
        head.generation
    );

    // redb.
    let dir = tempfile::tempdir().unwrap();
    let refs = Arc::new(RedbRefStore::open(dir.path().join("refs.redb")).unwrap());
    let store = MemoryStore::new();
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    for i in 0..3u8 {
        eng.write(u64::from(i) * BLOCK, &block(i)).unwrap();
        eng.commit_with(CommitKind::Flush, vec![row(&[i])])
            .await
            .unwrap();
    }
    eng.write(0, &block(9)).unwrap();
    let h = eng.commit(CommitKind::Flush).await.unwrap();
    let head = refs.get(v, "head").await.unwrap().unwrap();
    assert_eq!(head.commit, h);
    println!(
        "S5 RedbRefStore: the same; the head's generation is {}",
        head.generation
    );
}

/// S6. A commit with an empty list of ops and nothing written.
#[tokio::test]
async fn s6_a_commit_with_no_ops_and_nothing_written() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(105);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    let h0 = eng.head_commit_hash();
    let h1 = eng.commit_with(CommitKind::Flush, vec![]).await.unwrap();
    println!(
        "S6 commit_with(kind, vec![]) with nothing written makes a commit: {}",
        h1 != h0
    );
}

/// S7. The commit after a commit that listed segments PUT early lists only
/// its own. No test of the pull request makes a second commit after a
/// put_sealed.
#[tokio::test]
async fn s7_the_commit_after_one_with_segments_put_early() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let v = vol(106);
    let mut eng = Engine::create_volume(store.clone(), refs.clone(), v, 256 * KIB, cfg())
        .await
        .unwrap();
    eng.write(0, &block(1)).unwrap();
    eng.write(BLOCK, &block(2)).unwrap();
    eng.put_sealed().await.unwrap();
    let h1 = eng.commit(CommitKind::Flush).await.unwrap();
    eng.write(2 * BLOCK, &block(3)).unwrap();
    let h2 = eng.commit(CommitKind::Flush).await.unwrap();
    let (n1, n2) = (
        commit_of(&store, v, &h1).await.segments.len(),
        commit_of(&store, v, &h2).await.segments.len(),
    );
    println!("S7 the first commit lists {n1} segments, the one after it {n2}");
    assert_eq!((n1, n2), (2, 1));
}

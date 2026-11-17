//! #110 at 4a51efc (M2.4a, the next look): which store operations the
//! crash test's upload makes on the new head (scenarios.py runs it as it
//! runs scenarios.rs).
#![allow(dead_code, unused_imports)]

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

/// The upload of the pull request's crash test at 4a51efc, word for word.
async fn upload<S: ObjectStore, R: RefStore>(eng: &mut Engine<S, R>) -> Result<(), EngineError> {
    eng.write(0, &block(20))?;
    eng.write(2 * BLOCK, &block(21))?;
    eng.put_sealed().await?;
    eng.write_zeroes(BLOCK, BLOCK)?;
    eng.write(5 * BLOCK, &block(22))?;
    eng.write(80 * KIB, &block(23))?;
    eng.put_sealed().await?;
    eng.write(6 * BLOCK, &block(24)[..512])?;
    eng.set_group(Some(41));
    eng.commit_with(CommitKind::Flush, vec![row(b"cut 1")])
        .await?;
    Ok(())
}

/// S8. The store operations of that upload, in order: the kill points of
/// the crash test on the new head.
#[tokio::test]
async fn s8_the_operations_of_the_crash_tests_upload_at_4a51efc() {
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
    println!("S8 the upload's {} operations: {}", log.len(), log.join(", "));
    let by_commit = log.iter().skip(4).filter(|l| *l == "put /seg/").count();
    println!("S8 PUTs of a segment made by the commit itself: {by_commit}");
}

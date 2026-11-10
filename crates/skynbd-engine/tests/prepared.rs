//! Prepared-snapshot (V3.2 ingestion) tests — design §9A, scenarios
//! S11–S13 in-process.

use skynbd_engine::prepared::{list_prepared, PreparedSession};
use skynbd_engine::{fsck, keys, Engine, EngineConfig, EngineError};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::{SnapId, VolumeId};

const KIB: u64 = 1024;
const VOL: u64 = 256 * KIB;

fn cfg() -> EngineConfig {
    EngineConfig {
        segment_max_bytes: 4 * KIB,
        shard_bytes: 64 * KIB,
        epoch: 0,
        ..EngineConfig::default()
    }
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// PUT a CAS object; returns its hash.
async fn put_obj(store: &MemoryStore, vol: VolumeId, data: &[u8]) -> [u8; 32] {
    let hash = *blake3::hash(data).as_bytes();
    store
        .put(
            &keys::cas_object(vol, &hash),
            bytes::Bytes::copy_from_slice(data),
        )
        .await
        .unwrap();
    hash
}

async fn lineage_vol() -> (MemoryStore, MemoryRefStore, VolumeId) {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    // Lineage volumes still have a head engine for gc/snapshot mgmt (D14).
    Engine::create_volume(store.clone(), refs.clone(), vol, VOL, cfg())
        .await
        .unwrap();
    (store, refs, vol)
}

/// S11 core: bootstrap publish == attached objects; hidden until publish;
/// idempotent replay; clear over base; abort leaves nothing visible.
#[tokio::test]
async fn s11_prepared_lifecycle_and_idempotency() {
    let (store, refs, vol) = lineage_vol().await;

    // Bootstrap G0: prepare from None, attach two 64k objects.
    let d0 = pattern(64 * KIB as usize, 1);
    let d1 = pattern(64 * KIB as usize, 2);
    let h0 = put_obj(&store, vol, &d0).await;
    let h1 = put_obj(&store, vol, &d1).await;

    let sid = SnapId(ulid::Ulid::new());
    let mut p0 = PreparedSession::create(store.clone(), refs.clone(), vol, sid, None)
        .await
        .unwrap();
    assert!(p0.attach(&[(0, 64 * KIB, h0, 0)], 1).await.unwrap());
    assert!(p0.attach(&[(64 * KIB, 64 * KIB, h1, 0)], 2).await.unwrap());

    // Hidden until publish (I9): no snap refs yet.
    assert!(refs
        .list(vol)
        .await
        .unwrap()
        .iter()
        .all(|(n, _)| !n.starts_with("snap/")));

    let g0 = p0.publish("g0", "coordinator").await.unwrap();

    // Visible, pinned, correct content.
    assert!(refs.get(vol, "snap/g0").await.unwrap().is_some());
    assert_eq!(
        refs.pins(vol, "snap/g0").await.unwrap(),
        vec!["coordinator".to_string()]
    );
    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, g0, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 64 * KIB).await.unwrap(), d0);
    assert_eq!(ro.read(64 * KIB, 64 * KIB).await.unwrap(), d1);
    // Prepared state fully gone.
    assert!(list_prepared(&refs, vol).await.unwrap().is_empty());
    assert!(refs
        .get(vol, &format!("prepared/{sid}"))
        .await
        .unwrap()
        .is_none());

    // Incremental P1 = clone(g0) + one changed object + a clear.
    let d2 = pattern(16 * KIB as usize, 9);
    let h2 = put_obj(&store, vol, &d2).await;
    let sid1 = SnapId(ulid::Ulid::new());
    let mut p1 = PreparedSession::create(
        store.clone(),
        refs.clone(),
        vol,
        sid1,
        Some(("snap/g0".into(), g0)),
    )
    .await
    .unwrap();
    // Base is pinned for the session's lifetime (V3.2 §13.6).
    assert!(refs
        .pins(vol, "snap/g0")
        .await
        .unwrap()
        .contains(&format!("prepared:{sid1}")));

    assert!(p1.attach(&[(8 * KIB, 16 * KIB, h2, 0)], 1).await.unwrap());
    assert!(p1.clear(&[(96 * KIB, 8 * KIB)], 2).await.unwrap());
    // Replay of an already-applied op id: no-op (idempotency, §9A.2).
    p1.checkpoint(2).await.unwrap();
    assert!(!p1.attach(&[(8 * KIB, 16 * KIB, h2, 0)], 1).await.unwrap());
    assert!(!p1.clear(&[(96 * KIB, 8 * KIB)], 2).await.unwrap());

    let g1 = p1.publish("g1", "coordinator").await.unwrap();

    // g1 == clone(g0) + ops, exactly (§9A.2 publish semantics).
    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, g1, cfg())
        .await
        .unwrap();
    let mut expect = [d0.clone(), d1.clone()].concat();
    expect[8 * KIB as usize..24 * KIB as usize].copy_from_slice(&d2);
    expect[96 * KIB as usize..104 * KIB as usize].fill(0);
    assert_eq!(ro.read(0, 128 * KIB).await.unwrap(), expect);
    // g0 untouched (I5).
    let ro0 = Engine::open_at_commit(store.clone(), refs.clone(), vol, g0, cfg())
        .await
        .unwrap();
    assert_eq!(
        ro0.read(8 * KIB, 16 * KIB).await.unwrap(),
        d0[8 * KIB as usize..24 * KIB as usize]
    );
    // Publish released the base pin holder; the publisher pin remains.
    assert_eq!(
        refs.pins(vol, "snap/g0").await.unwrap(),
        vec!["coordinator".to_string()]
    );

    // Duplicate prepare on same id refused; abort leaves nothing.
    let sid2 = SnapId(ulid::Ulid::new());
    let mut p2 = PreparedSession::create(
        store.clone(),
        refs.clone(),
        vol,
        sid2,
        Some(("snap/g1".into(), g1)),
    )
    .await
    .unwrap();
    assert!(matches!(
        PreparedSession::create(store.clone(), refs.clone(), vol, sid2, None).await,
        Err(EngineError::SnapshotExists(_))
    ));
    p2.attach(&[(0, 16 * KIB, h2, 0)], 1).await.unwrap();
    p2.abort().await.unwrap();
    assert!(refs
        .pins(vol, "snap/g1")
        .await
        .unwrap()
        .iter()
        .all(|h| h == "coordinator"));
    assert!(list_prepared(&refs, vol).await.unwrap().is_empty());
    // Duplicate publish name refused.
    let sid3 = SnapId(ulid::Ulid::new());
    let mut p3 = PreparedSession::create(store.clone(), refs.clone(), vol, sid3, None)
        .await
        .unwrap();
    assert!(matches!(
        p3.publish("g0", "x").await,
        Err(EngineError::SnapshotExists(_))
    ));

    assert!(fsck(&store, &refs, vol).await.unwrap().ok());
}

/// S12 core: pins block deletion (I8); GC during backfill preserves the
/// prepared closure and EnsureObject promises (I6, §10.2).
#[tokio::test]
async fn s12_pins_and_gc_safety() {
    let (store, refs, vol) = lineage_vol().await;

    let d0 = pattern(32 * KIB as usize, 1);
    let h0 = put_obj(&store, vol, &d0).await;
    let sid = SnapId(ulid::Ulid::new());
    let mut p0 = PreparedSession::create(store.clone(), refs.clone(), vol, sid, None)
        .await
        .unwrap();
    p0.attach(&[(0, 32 * KIB, h0, 0)], 1).await.unwrap();
    let g0 = p0.publish("g0", "h1").await.unwrap();

    // Publish-implies-pin: delete refused immediately after publish (I8).
    let mut head = Engine::open(store.clone(), refs.clone(), vol, cfg())
        .await
        .unwrap();
    assert!(matches!(
        head.snapshot_delete("g0").await,
        Err(EngineError::Pinned(_))
    ));

    // Prepared-from-base pins the base: still refused after publisher unpin.
    let sid1 = SnapId(ulid::Ulid::new());
    let mut p1 = PreparedSession::create(
        store.clone(),
        refs.clone(),
        vol,
        sid1,
        Some(("snap/g0".into(), g0)),
    )
    .await
    .unwrap();
    refs.transact(
        vol,
        vec![skynbd_refs::RefOp::Unpin {
            name: "snap/g0".into(),
            holder: "h1".into(),
        }],
    )
    .await
    .unwrap();
    assert!(matches!(
        head.snapshot_delete("g0").await,
        Err(EngineError::Pinned(_))
    ));

    // Mid-backfill: attach one object, checkpoint; ensure() an orphan CAS
    // object (unreachable) — the promise must survive GC (§10.2).
    let d1 = pattern(16 * KIB as usize, 7);
    let h1 = put_obj(&store, vol, &d1).await;
    p1.attach(&[(0, 16 * KIB, h1, 0)], 1).await.unwrap();
    p1.checkpoint(1).await.unwrap();

    let orphan = put_obj(&store, vol, &pattern(4 * KIB as usize, 0x55)).await;
    assert_eq!(
        p1.ensure(&[orphan, [0xEE; 32]]).await.unwrap(),
        vec![true, false]
    );

    let report = head.gc().await.unwrap();
    // Prepared closure + promised object survive.
    assert!(
        store.exists(&keys::cas_object(vol, &h1)).await.unwrap(),
        "attached CAS swept"
    );
    assert!(
        store.exists(&keys::cas_object(vol, &orphan)).await.unwrap(),
        "promise swept (I6)"
    );
    let _ = report;

    // Publish g1; abort-path cleanup: delete g0 after all pins released.
    let g1 = p1.publish("g1", "h1").await.unwrap();
    refs.transact(
        vol,
        vec![skynbd_refs::RefOp::Unpin {
            name: "snap/g0".into(),
            holder: "h1".into(),
        }],
    )
    .await
    .unwrap();
    // g0 has no holders now (prepared holder released at publish).
    assert!(refs.pins(vol, "snap/g0").await.unwrap().is_empty());
    head.snapshot_delete("g0").await.unwrap();
    let swept = head.gc().await.unwrap().swept_objects;
    assert!(swept > 0, "g0-only objects reclaimed");

    // g1 intact after the sweep (I6).
    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, g1, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 16 * KIB).await.unwrap(), d1);
    assert!(fsck(&store, &refs, vol).await.unwrap().ok());
}

/// S13 core: crash mid-backfill → resume from the durable watermark;
/// replayed and fresh ops converge to the no-crash content.
#[tokio::test]
async fn s13_crash_resume_from_watermark() {
    let (store, refs, vol) = lineage_vol().await;

    let chunks: Vec<Vec<u8>> = (0..4)
        .map(|i| pattern(16 * KIB as usize, i as u8 + 1))
        .collect();
    let hashes = {
        let mut v = Vec::new();
        for c in &chunks {
            v.push(put_obj(&store, vol, c).await);
        }
        v
    };

    let sid = SnapId(ulid::Ulid::new());
    let mut p = PreparedSession::create(store.clone(), refs.clone(), vol, sid, None)
        .await
        .unwrap();
    // Ops 1,2 checkpointed (durable); op 3 applied but NOT checkpointed.
    p.attach(&[(0, 16 * KIB, hashes[0], 0)], 1).await.unwrap();
    p.attach(&[(16 * KIB, 16 * KIB, hashes[1], 0)], 2)
        .await
        .unwrap();
    p.checkpoint(2).await.unwrap();
    p.attach(&[(32 * KIB, 16 * KIB, hashes[2], 0)], 3)
        .await
        .unwrap();
    drop(p); // kill -9: op 3 was volatile

    // Resume: durable watermark says 2 — coordinator replays 3, adds 4.
    let mut p = PreparedSession::open_existing(store.clone(), refs.clone(), vol, sid)
        .await
        .unwrap();
    assert_eq!(p.status().op_watermark, 2, "durable watermark");
    assert!(p
        .attach(&[(32 * KIB, 16 * KIB, hashes[2], 0)], 3)
        .await
        .unwrap());
    assert!(p
        .attach(&[(48 * KIB, 16 * KIB, hashes[3], 0)], 4)
        .await
        .unwrap());
    // Replaying already-durable ops is a no-op.
    assert!(!p.attach(&[(0, 16 * KIB, hashes[0], 0)], 1).await.unwrap());
    let g = p.publish("g", "coordinator").await.unwrap();

    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, g, cfg())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 64 * KIB).await.unwrap(), chunks.concat());

    // Publish atomicity: nothing prepared remains, snapshot visible+pinned.
    assert!(list_prepared(&refs, vol).await.unwrap().is_empty());
    assert_eq!(
        refs.pins(vol, "snap/g").await.unwrap(),
        vec!["coordinator".to_string()]
    );
    assert!(fsck(&store, &refs, vol).await.unwrap().ok());
}

/// Issue #97 follow-up: `apply` stages a mixed batch in order under one
/// op id (a later op wins over an earlier one on the same range), skips
/// a replay at or below the durable watermark, and refuses a batch with
/// one bad op without staging any of it.
#[tokio::test]
async fn mixed_apply_is_ordered_replay_safe_and_all_or_nothing() {
    use skynbd_engine::prepared::RangeOp;
    let (store, refs, vol) = lineage_vol().await;
    let d = pattern(32 * KIB as usize, 7);
    let h = put_obj(&store, vol, &d).await;
    let attach = |off, len, obj_off| RangeOp::Attach {
        off,
        len,
        hash: h,
        obj_off,
    };

    let sid = SnapId(ulid::Ulid::new());
    let mut p = PreparedSession::create(store.clone(), refs.clone(), vol, sid, None)
        .await
        .unwrap();
    // Op 1: attach 0..32K, then clear 8..16K inside it — order matters.
    let batch = [
        attach(0, 32 * KIB, 0),
        RangeOp::Clear {
            off: 8 * KIB,
            len: 8 * KIB,
        },
    ];
    assert!(p.apply(&batch, 1).await.unwrap());
    p.checkpoint(1).await.unwrap();
    assert!(!p.apply(&batch, 1).await.unwrap(), "durable replay skips");

    // Op 2: a valid attach followed by an out-of-bounds clear. Refused
    // whole: the attach at 64K must not land.
    let bad = [
        attach(64 * KIB, 16 * KIB, 0),
        RangeOp::Clear {
            off: VOL,
            len: 4 * KIB,
        },
    ];
    assert!(matches!(
        p.apply(&bad, 2).await,
        Err(EngineError::InvalidIo(_))
    ));
    let g = p.publish("g", "coordinator").await.unwrap();

    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, g, cfg())
        .await
        .unwrap();
    let mut want = d.clone();
    want[8 * KIB as usize..16 * KIB as usize].fill(0);
    assert_eq!(ro.read(0, 32 * KIB).await.unwrap(), want);
    assert_eq!(
        ro.read(64 * KIB, 16 * KIB).await.unwrap(),
        vec![0; 16 * KIB as usize],
        "a refused batch stages nothing"
    );
}

/// The other overlap order: clear then attach inside it. An `apply` that
/// staged all attaches before all clears would publish zeros here.
#[tokio::test]
async fn mixed_apply_stages_a_clear_before_a_later_attach() {
    use skynbd_engine::prepared::RangeOp;
    let (store, refs, vol) = lineage_vol().await;
    let d = pattern(8 * KIB as usize, 9);
    let h = put_obj(&store, vol, &d).await;
    let sid = SnapId(ulid::Ulid::new());
    let mut p = PreparedSession::create(store.clone(), refs.clone(), vol, sid, None)
        .await
        .unwrap();
    let batch = [
        RangeOp::Clear {
            off: 0,
            len: 32 * KIB,
        },
        RangeOp::Attach {
            off: 8 * KIB,
            len: 8 * KIB,
            hash: h,
            obj_off: 0,
        },
    ];
    assert!(p.apply(&batch, 1).await.unwrap());
    let g = p.publish("g", "coordinator").await.unwrap();
    let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, g, cfg())
        .await
        .unwrap();
    let mut want = vec![0u8; 32 * KIB as usize];
    want[8 * KIB as usize..16 * KIB as usize].copy_from_slice(&d);
    assert_eq!(ro.read(0, 32 * KIB).await.unwrap(), want);
}

/// A ref store whose next `MetaPut` transaction fails, once armed.
#[derive(Clone)]
struct FailNextMetaPut {
    inner: MemoryRefStore,
    armed: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[async_trait::async_trait]
impl RefStore for FailNextMetaPut {
    async fn get(
        &self,
        vol: VolumeId,
        name: &str,
    ) -> skynbd_refs::Result<Option<skynbd_refs::RefValue>> {
        self.inner.get(vol, name).await
    }
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<skynbd_refs::RefValue>,
        new_commit: Option<skynbd_types::CommitHash>,
    ) -> skynbd_refs::Result<skynbd_refs::CasOutcome> {
        self.inner
            .compare_and_swap(vol, name, expected, new_commit)
            .await
    }
    async fn list(
        &self,
        vol: VolumeId,
    ) -> skynbd_refs::Result<Vec<(String, skynbd_refs::RefValue)>> {
        self.inner.list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<skynbd_refs::RefEvent> {
        self.inner.watch()
    }
    async fn transact(
        &self,
        vol: VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        let meta_put = ops
            .iter()
            .any(|op| matches!(op, skynbd_refs::RefOp::MetaPut { .. }));
        if meta_put && self.armed.swap(false, std::sync::atomic::Ordering::SeqCst) {
            return Err(skynbd_refs::RefError::Backend(
                "injected meta write failure".into(),
            ));
        }
        self.inner.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.inner.transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Vec<String>> {
        self.inner.pins(vol, name).await
    }
    async fn meta_get(&self, vol: VolumeId, key: &str) -> skynbd_refs::Result<Option<Vec<u8>>> {
        self.inner.meta_get(vol, key).await
    }
    async fn meta_list(
        &self,
        vol: VolumeId,
        prefix: &str,
    ) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
        self.inner.meta_list(vol, prefix).await
    }
    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<skynbd_refs::LeaseOutcome> {
        self.inner.lease_acquire(vol, holder, ttl_ms).await
    }
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> skynbd_refs::Result<()> {
        self.inner.lease_release(vol, holder).await
    }
    async fn lease_get(&self, vol: VolumeId) -> skynbd_refs::Result<Option<skynbd_refs::Lease>> {
        self.inner.lease_get(vol).await
    }
}

/// An `ensure` promise counts only once it is durable (#114 review): when
/// persisting it fails (or is cancelled — an agent dropping its
/// overlapped staging), a retried `ensure` must persist it before
/// answering "exists" again. Otherwise the agent skips the upload, and a
/// GC — which roots only the durable row — sweeps the object out from
/// under the attach.
#[tokio::test]
async fn an_ensure_promise_is_durable_before_it_answers_positive() {
    let (store, refs, vol) = lineage_vol().await;
    let flaky = FailNextMetaPut {
        inner: refs.clone(),
        armed: Default::default(),
    };
    // Uploaded by an earlier attempt, referenced by nothing yet.
    let left = put_obj(&store, vol, &pattern(4 * KIB as usize, 0x3C)).await;
    let sid = SnapId(ulid::Ulid::new());
    let mut p = PreparedSession::create(store.clone(), flaky.clone(), vol, sid, None)
        .await
        .unwrap();

    flaky.armed.store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        p.ensure(&[left]).await.is_err(),
        "the persist failure surfaces"
    );
    assert_eq!(p.ensure(&[left]).await.unwrap(), vec![true]);

    let mut head = Engine::open(store.clone(), refs.clone(), vol, cfg())
        .await
        .unwrap();
    head.gc().await.unwrap();
    assert!(
        store.exists(&keys::cas_object(vol, &left)).await.unwrap(),
        "a positive ensure answer was not a durable GC root"
    );
}

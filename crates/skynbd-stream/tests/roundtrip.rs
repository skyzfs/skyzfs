//! SKYD1 stream tests (design §9.4 — milestone M5, scenario S7 in-process).

use skynbd_engine::{Engine, EngineConfig};
use skynbd_format::CommitKind;
use skynbd_refs::MemoryRefStore;
use skynbd_store::MemoryStore;
use skynbd_stream::{apply, send, StreamError};
use skynbd_types::VolumeId;

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

async fn new_vol(
    store: &MemoryStore,
    refs: &MemoryRefStore,
) -> (VolumeId, Engine<MemoryStore, MemoryRefStore>) {
    let vol = VolumeId(ulid::Ulid::new());
    let eng = Engine::create_volume(store.clone(), refs.clone(), vol, VOL, cfg())
        .await
        .unwrap();
    (vol, eng)
}

/// Baseline (from=None) + incremental streams replicate exactly, and the
/// incremental stream carries only the changed bytes (O(changed), §9.4).
#[tokio::test]
async fn baseline_plus_incremental_replicates_exactly() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let (vol, mut src) = new_vol(&store, &refs).await;

    // s1: 128k of data + a zero-punched hole.
    src.write(0, &pattern(128 * KIB as usize, 1)).unwrap();
    src.write_zeroes(32 * KIB, 16 * KIB).unwrap();
    let (_, s1) = src.snapshot_create("s1").await.unwrap();

    // s2: mutate exactly 24k, trim 8k.
    src.write(64 * KIB, &pattern(24 * KIB as usize, 9)).unwrap();
    src.write_zeroes(200 * KIB, 8 * KIB).unwrap();
    let (_, s2) = src.snapshot_create("s2").await.unwrap();

    // Baseline stream from empty → s1 into a fresh volume.
    let mut base = Vec::new();
    let stats = send(store.clone(), refs.clone(), vol, None, s1, cfg(), &mut base)
        .await
        .unwrap();
    assert!(stats.data_bytes >= 112 * KIB, "baseline carries s1 data");

    let (_, mut dst) = new_vol(&store, &refs).await;
    apply(&mut dst, &base).await.unwrap();
    assert_eq!(dst.read(0, VOL).await.unwrap(), {
        let ro = Engine::open_at_commit(store.clone(), refs.clone(), vol, s1, cfg())
            .await
            .unwrap();
        ro.read(0, VOL).await.unwrap()
    });

    // Incremental s1 → s2: payload ≈ changed bytes only.
    let mut delta = Vec::new();
    let stats = send(
        store.clone(),
        refs.clone(),
        vol,
        Some(s1),
        s2,
        cfg(),
        &mut delta,
    )
    .await
    .unwrap();
    assert_eq!(
        stats.data_bytes,
        24 * KIB,
        "delta payload must be exactly the changed data"
    );
    assert!(
        (delta.len() as u64) < 32 * KIB,
        "stream stays near payload size"
    );

    apply(&mut dst, &delta).await.unwrap();
    let ro = Engine::open_at_commit(store.clone(), refs, vol, s2, cfg())
        .await
        .unwrap();
    assert_eq!(
        dst.read(0, VOL).await.unwrap(),
        ro.read(0, VOL).await.unwrap()
    );
}

/// A torn or corrupted stream changes nothing (§9.4 transactionality).
#[tokio::test]
async fn torn_and_corrupt_streams_change_nothing() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let (vol, mut src) = new_vol(&store, &refs).await;
    src.write(0, &pattern(64 * KIB as usize, 5)).unwrap();
    let (_, s1) = src.snapshot_create("s1").await.unwrap();

    let mut base = Vec::new();
    send(store.clone(), refs.clone(), vol, None, s1, cfg(), &mut base)
        .await
        .unwrap();

    let (_, mut dst) = new_vol(&store, &refs).await;
    let before = dst.read(0, VOL).await.unwrap();
    let seq_before = dst.flush_seq();

    // Torn: cut mid-stream.
    let torn = &base[..base.len() / 2];
    assert!(matches!(
        apply(&mut dst, torn).await,
        Err(StreamError::Torn)
    ));
    assert_eq!(dst.read(0, VOL).await.unwrap(), before);
    assert_eq!(dst.flush_seq(), seq_before, "no commit happened");

    // Corrupt: flip a payload byte (checksum trips).
    let mut corrupt = base.clone();
    let mid = corrupt.len() / 2;
    corrupt[mid] ^= 0xFF;
    let e = apply(&mut dst, &corrupt).await.unwrap_err();
    assert!(
        matches!(e, StreamError::Checksum | StreamError::Malformed(_)),
        "got {e:?}"
    );
    assert_eq!(dst.read(0, VOL).await.unwrap(), before);
    assert_eq!(dst.flush_seq(), seq_before);

    // The volume still applies a good stream afterwards.
    apply(&mut dst, &base).await.unwrap();
    assert_eq!(
        dst.read(0, 64 * KIB).await.unwrap(),
        pattern(64 * KIB as usize, 5)
    );
}

/// Guard rails: wrong size, dirty target, baseline onto used volume.
#[tokio::test]
async fn apply_guards() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let (vol, mut src) = new_vol(&store, &refs).await;
    src.write(0, &pattern(4096, 1)).unwrap();
    let (_, s1) = src.snapshot_create("s1").await.unwrap();
    let mut base = Vec::new();
    send(store.clone(), refs.clone(), vol, None, s1, cfg(), &mut base)
        .await
        .unwrap();

    // Baseline onto a used volume refused.
    let (_, mut used) = new_vol(&store, &refs).await;
    used.write(0, &pattern(512, 2)).unwrap();
    used.commit(CommitKind::Flush).await.unwrap();
    assert!(matches!(
        apply(&mut used, &base).await,
        Err(StreamError::TargetMismatch(_))
    ));

    // Dirty target refused (un-flushed writes are not clobbered).
    let (_, mut dirty) = new_vol(&store, &refs).await;
    dirty.write(0, &pattern(512, 3)).unwrap();
    assert!(matches!(
        apply(&mut dirty, &base).await,
        Err(StreamError::TargetMismatch(_))
    ));
    assert_eq!(
        dirty.read(0, 512).await.unwrap(),
        pattern(512, 3),
        "writes preserved"
    );
}

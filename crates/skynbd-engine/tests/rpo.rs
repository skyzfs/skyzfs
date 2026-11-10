//! RPO ref advancement (design §9.3 — milestone M5, scenario S6 in-process).

use skynbd_engine::{Engine, EngineConfig};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::MemoryStore;
use skynbd_types::VolumeId;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

#[tokio::test]
async fn rpo_advances_replaces_and_noops() {
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let mut eng = Engine::create_volume(
        store.clone(),
        refs.clone(),
        vol,
        128 * 1024,
        EngineConfig::default(),
    )
    .await
    .unwrap();

    // First advance: captures dirty state (forced commit) + creates ref.
    eng.write(0, &pattern(4096, 1)).unwrap();
    let c1 = eng.rpo_advance().await.unwrap().expect("advanced");
    assert_eq!(refs.get(vol, "rpo").await.unwrap().unwrap().commit, c1);

    // Clean + current: no-op, ref unchanged (design §9.3 skip rule).
    assert!(eng.rpo_advance().await.unwrap().is_none());
    assert_eq!(refs.get(vol, "rpo").await.unwrap().unwrap().commit, c1);

    // New writes: advance replaces the ref (same ref name, new commit).
    eng.write(0, &pattern(4096, 2)).unwrap();
    let c2 = eng.rpo_advance().await.unwrap().expect("advanced again");
    assert_ne!(c1, c2);
    assert_eq!(refs.get(vol, "rpo").await.unwrap().unwrap().commit, c2);

    // The rpo view serves the captured content (mountable guarantee).
    let ro = Engine::open_at_commit(store, refs, vol, c2, EngineConfig::default())
        .await
        .unwrap();
    assert_eq!(ro.read(0, 4096).await.unwrap(), pattern(4096, 2));
}

/// `rpo_is_current` is the RPO tick's shared-lock fast path: true exactly
/// when `rpo_advance` would no-op. A wrong `true` would make the tick skip
/// forever and the rpo ref stop advancing.
#[tokio::test]
async fn rpo_is_current_tracks_whether_an_advance_would_noop() {
    use skynbd_format::CommitKind;
    let store = MemoryStore::new();
    let refs = MemoryRefStore::new();
    let vol = VolumeId(ulid::Ulid::new());
    let mut eng = Engine::create_volume(
        store.clone(),
        refs.clone(),
        vol,
        128 * 1024,
        EngineConfig::default(),
    )
    .await
    .unwrap();

    // Fresh volume: no rpo ref yet.
    assert!(!eng.rpo_is_current().await.unwrap(), "fresh volume");
    assert!(eng.rpo_advance().await.unwrap().is_some());
    assert!(eng.rpo_is_current().await.unwrap(), "just advanced");
    assert!(eng.rpo_advance().await.unwrap().is_none(), "agrees: no-op");

    // Dirty overlay.
    eng.write(0, &pattern(4096, 3)).unwrap();
    assert!(!eng.rpo_is_current().await.unwrap(), "un-flushed write");

    // A FLUSH moves head past the rpo commit: clean, yet not current.
    eng.commit(CommitKind::Flush).await.unwrap();
    assert!(!eng.rpo_is_current().await.unwrap(), "head moved");
    let c = eng.rpo_advance().await.unwrap().expect("advanced");
    assert!(eng.rpo_is_current().await.unwrap(), "advanced again");
    assert_eq!(refs.get(vol, "rpo").await.unwrap().unwrap().commit, c);
}

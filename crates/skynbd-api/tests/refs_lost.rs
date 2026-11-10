//! A volume whose refs this server cannot see is not an absent volume
//! (design §6.5). The object store is shared; a redb refs store is not,
//! so "superblock present, no head ref" means the refs were lost or
//! belong to another server — and creating over it would republish a
//! live volume as an empty disk, which a CSI node plugin would `mkfs`.

use std::sync::Arc;

use skynbd_api::{ProvisionError, Provisioner, VolumeSpec};
use skynbd_format::CommitKind;
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};

fn provisioner(store: Arc<dyn ObjectStore>, refs: Arc<dyn RefStore>) -> Arc<Provisioner> {
    Arc::new(Provisioner::new(
        store,
        refs,
        Arc::new(ExportRegistry::new()),
        0,
    ))
}

fn spec(name: &str) -> VolumeSpec {
    VolumeSpec {
        name: name.into(),
        size: 128 * 1024,
        kind: "attached".into(),
        from: None,
    }
}

/// Node A creates and writes; node B has the same bucket and an empty
/// refs store. B must refuse rather than serve an empty volume.
#[tokio::test]
async fn written_volume_with_foreign_refs_is_refused() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs_a: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());

    let a = provisioner(Arc::clone(&store), refs_a);
    a.create(&spec("pvc-1")).await.expect("create");
    {
        let handle = a.volumes.read().await["pvc-1"].clone();
        let mut engine = handle.engine.write().await;
        engine.write(0, &[0xab; 4096]).expect("write");
        engine.commit(CommitKind::Flush).await.expect("commit");
    }

    let b = provisioner(Arc::clone(&store), Arc::new(MemoryRefStore::new()));
    match b.serve_config_volume(&spec("pvc-1")).await {
        Err(ProvisionError::RefsLost(v)) => assert_eq!(v, "pvc-1"),
        Err(e) => panic!("expected RefsLost, got {e}"),
        Ok(_) => panic!("served a volume whose data it cannot see"),
    }
    // The API path must refuse identically — CreateVolume is retried by
    // the external-provisioner and can land on any server.
    assert!(matches!(
        b.create(&spec("pvc-1")).await,
        Err(ProvisionError::RefsLost(_))
    ));
}

/// The other side of the same coin: a create interrupted before its head
/// CAS leaves a superblock and no ref, and *that* must stay creatable —
/// otherwise a crashed create wedges the name forever.
#[tokio::test]
async fn unwritten_volume_with_no_refs_is_creatable() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let a = provisioner(Arc::clone(&store), Arc::new(MemoryRefStore::new()));
    a.create(&spec("pvc-2")).await.expect("create");

    // Same bucket, no refs: the volume holds only its initial empty
    // commit, so re-creating it is content-identical.
    let b = provisioner(Arc::clone(&store), Arc::new(MemoryRefStore::new()));
    b.create(&spec("pvc-2")).await.expect("re-create");
    let handle = b.volumes.read().await["pvc-2"].clone();
    assert_eq!(handle.size, 128 * 1024);
}

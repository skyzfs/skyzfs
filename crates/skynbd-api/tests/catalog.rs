//! The D-M9 name index: `volume_id = blake3(name)` makes name → prefix
//! free, and the `index/vol/{name}` marker closes the reverse direction
//! — after total refs loss (redb gone, bucket intact), the catalog is
//! reconstructible, and with `rebuild_refs` per volume, so is everything
//! else. That round trip is the test that matters here.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_engine::keys;
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use tower::ServiceExt;

const VOL_BYTES: u64 = 128 * 1024;

struct Rig {
    router: axum::Router,
    registry: Arc<ExportRegistry>,
    provisioner: Arc<Provisioner>,
}

fn rig(store: &Arc<MemoryStore>, refs: &Arc<dyn RefStore>) -> Rig {
    let registry = Arc::new(ExportRegistry::new());
    let provisioner = Arc::new(Provisioner::new(
        Arc::clone(store) as Arc<dyn ObjectStore>,
        Arc::clone(refs),
        Arc::clone(&registry),
        0,
    ));
    let state = Arc::new(ApiState {
        volumes: Arc::clone(&provisioner.volumes),
        registry: Arc::clone(&registry),
        auth: ApiAuth::Disabled,
        enroll: None,
        provisioner: Some(Arc::clone(&provisioner)),
        snap_requests: Default::default(),
    });
    Rig {
        router: skynbd_api::router(state),
        registry,
        provisioner,
    }
}

async fn post(router: &axum::Router, path: &str, body: Option<serde_json::Value>) -> StatusCode {
    let req = match body {
        Some(b) => Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => Request::builder()
            .method("POST")
            .uri(path)
            .body(Body::empty())
            .unwrap(),
    };
    router.clone().oneshot(req).await.unwrap().status()
}

/// Create writes the marker; delete removes it.
#[tokio::test]
async fn markers_track_the_volume_lifecycle() {
    let store = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let r = rig(&store, &refs);

    post(
        &r.router,
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-m", "size": VOL_BYTES })),
    )
    .await;
    assert!(store.exists(&keys::vol_index("pvc-m")).await.unwrap());

    let req = Request::builder()
        .method("DELETE")
        .uri("/v1/volumes/pvc-m")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        r.router.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NO_CONTENT
    );
    assert!(
        !store.exists(&keys::vol_index("pvc-m")).await.unwrap(),
        "delete must remove the marker"
    );
}

/// The disaster: the refs store is lost entirely, the bucket survives.
/// rebuild-catalog restores the rows from the markers, rebuild_refs
/// restores each head, and a fresh daemon serves everything — data
/// intact.
#[tokio::test]
async fn total_refs_loss_is_recoverable_from_the_bucket() {
    let store = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let r = rig(&store, &refs);

    for (name, seed) in [("pvc-r1", 0xAAu8), ("pvc-r2", 0xBBu8)] {
        assert_eq!(
            post(
                &r.router,
                "/v1/volumes",
                Some(serde_json::json!({ "name": name, "size": VOL_BYTES })),
            )
            .await,
            StatusCode::CREATED
        );
        let export = r.registry.get(&format!("{name}/head")).await.unwrap();
        let mut eng = export.engine.write().await;
        eng.write(0, &[seed; 4096]).unwrap();
        eng.commit(skynbd_format::CommitKind::Flush).await.unwrap();
    }
    drop(r); // the daemon and its refs store die

    // A new daemon over the same bucket, with an empty refs store.
    let fresh: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let restored = skynbd_api::provision::rebuild_catalog(
        &(Arc::clone(&store) as Arc<dyn ObjectStore>),
        &fresh,
    )
    .await
    .unwrap();
    assert_eq!(restored, vec!["pvc-r1".to_string(), "pvc-r2".to_string()]);

    for name in &restored {
        let report = skynbd_engine::rebuild_refs(
            &(Arc::clone(&store) as Arc<dyn ObjectStore>),
            &fresh,
            skynbd_api::provision::volume_id(name),
        )
        .await
        .unwrap();
        assert!(report.head_rebuilt, "{name}: head must rebuild");
    }

    let r2 = rig(&store, &fresh);
    assert_eq!(r2.provisioner.load_catalog().await.unwrap(), 2);
    for (name, seed) in [("pvc-r1", 0xAAu8), ("pvc-r2", 0xBBu8)] {
        let export = r2.registry.get(&format!("{name}/head")).await.unwrap();
        let read = export.engine.write().await.read(0, 4096).await.unwrap();
        assert_eq!(read, vec![seed; 4096], "{name}: data must survive");
    }

    // And a second rebuild is a clean no-op: existing rows win.
    let again = skynbd_api::provision::rebuild_catalog(
        &(Arc::clone(&store) as Arc<dyn ObjectStore>),
        &fresh,
    )
    .await
    .unwrap();
    assert!(again.is_empty());
}

/// The online route: 200 with the restored names (none, here — the
/// catalog is intact).
#[tokio::test]
async fn rebuild_route_answers() {
    let store = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let r = rig(&store, &refs);
    post(
        &r.router,
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-o", "size": VOL_BYTES })),
    )
    .await;
    assert_eq!(
        post(&r.router, "/v1/catalog/rebuild", None).await,
        StatusCode::OK
    );
}

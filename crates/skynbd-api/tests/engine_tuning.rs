//! `Provisioner::with_engine_tuning` reaches the volumes it serves: each
//! served volume's config carries the configured widths and cap, and a
//! GET-permit pool of its own that its auxiliary readers (`tuning()`:
//! snapshot data, diff) share — the cap is per volume, not per engine.

use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_engine::EngineConfig;
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use tower::ServiceExt;

#[tokio::test]
async fn served_volumes_carry_the_tuning_and_a_pool_each() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let registry = Arc::new(ExportRegistry::new());
    let provisioner = Arc::new(
        Provisioner::new(store, refs, Arc::clone(&registry), 0).with_engine_tuning(EngineConfig {
            read_fanout: 4,
            walk_concurrency: 3,
            max_read_gets: 2,
            // A pool passed in is not shared across volumes.
            ..EngineConfig::default().share_read_gets()
        }),
    );
    let router = skynbd_api::router(Arc::new(ApiState {
        volumes: Arc::clone(&provisioner.volumes),
        registry,
        auth: ApiAuth::Disabled,
        enroll: None,
        provisioner: Some(Arc::clone(&provisioner)),
        snap_requests: Default::default(),
    }));
    for name in ["tune-a", "tune-b"] {
        let req = Request::builder()
            .method("POST")
            .uri("/v1/volumes")
            .header("content-type", "application/json")
            .body(Body::from(
                serde_json::json!({ "name": name, "size": 128 * 1024 }).to_string(),
            ))
            .unwrap();
        let st = router.clone().oneshot(req).await.unwrap().status();
        assert!(st.is_success(), "{name}: {st}");
    }

    let vols = provisioner.volumes.read().await;
    let (a, b) = (&vols["tune-a"].cfg, &vols["tune-b"].cfg);
    for cfg in [a, b] {
        assert_eq!(
            (cfg.read_fanout, cfg.walk_concurrency, cfg.max_read_gets),
            (4, 3, 2)
        );
        assert!(cfg.read_gets.is_some(), "no shared pool");
    }
    assert_eq!(
        a.tuning().read_gets,
        a.read_gets,
        "auxiliary readers get the volume's pool"
    );
    assert_ne!(a.read_gets, b.read_gets, "volumes share a pool");
}

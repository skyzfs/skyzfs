//! Dynamic volume lifecycle tests (design §12.1; the CSI driver's
//! CreateVolume/DeleteVolume contract): idempotent create, spec-mismatch
//! conflicts, delete scoping (dynamic-only), catalog persistence across
//! a provisioner restart, and the `volume` authorization group.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{mint, spawn_issuer, AUD};
use http_body_util::BodyExt;
use serde_json::json;
use skynbd_api::{ApiAuth, ApiState, Binding, OidcAuth, Provisioner, TrustedIssuer, VolumeSpec};
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use tower::ServiceExt;

const VOL_BYTES: u64 = 128 * 1024;

struct Rig {
    router: axum::Router,
    provisioner: Arc<Provisioner>,
    registry: Arc<ExportRegistry>,
}

fn rig_with(store: Arc<dyn ObjectStore>, refs: Arc<dyn RefStore>, auth: ApiAuth) -> Rig {
    let registry = Arc::new(ExportRegistry::new());
    let provisioner = Arc::new(Provisioner::new(
        Arc::clone(&store),
        Arc::clone(&refs),
        Arc::clone(&registry),
        0,
    ));
    let state = Arc::new(ApiState {
        volumes: Arc::clone(&provisioner.volumes),
        registry: Arc::clone(&registry),
        auth,
        enroll: None,
        provisioner: Some(Arc::clone(&provisioner)),
        snap_requests: Default::default(),
    });
    Rig {
        router: skynbd_api::router(state),
        provisioner,
        registry,
    }
}

fn rig() -> Rig {
    rig_with(
        Arc::new(MemoryStore::new()),
        Arc::new(MemoryRefStore::new()),
        ApiAuth::Disabled,
    )
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
    token: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => Body::from(v.to_string()),
        None => Body::empty(),
    };
    let resp = router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, val)
}

#[tokio::test(flavor = "multi_thread")]
async fn create_serves_and_is_idempotent() {
    let r = rig();
    let spec = json!({"name": "pvc-1", "size": VOL_BYTES});
    let (status, body) = call(&r.router, "POST", "/v1/volumes", Some(spec.clone()), None).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["name"], "pvc-1");
    assert_eq!(body["size"], VOL_BYTES);

    // Visible in the volume list and served over NBD.
    let (_, vols) = call(&r.router, "GET", "/v1/volumes", None, None).await;
    assert!(vols
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == "pvc-1"));
    assert!(r.registry.get("pvc-1/head").await.is_some());

    // The full API surface works on it immediately.
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/pvc-1/snapshots",
        Some(json!({"name": "s1"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Identical replay: 200, not 409/500 (CSI idempotency).
    let (status, _) = call(&r.router, "POST", "/v1/volumes", Some(spec), None).await;
    assert_eq!(status, StatusCode::OK);
    // Same name, different size: 409.
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "pvc-1", "size": VOL_BYTES * 2})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    // Lineage volumes get no writable head export.
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "lin-1", "size": VOL_BYTES, "kind": "lineage"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert!(r.registry.get("lin-1/head").await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_is_idempotent_and_dynamic_only() {
    let r = rig();
    r.provisioner
        .serve_config_volume(&VolumeSpec {
            name: "cfg1".into(),
            size: VOL_BYTES,
            kind: "attached".into(),
            from: None,
        })
        .await
        .unwrap();
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "pvc-2", "size": VOL_BYTES})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let (status, _) = call(&r.router, "DELETE", "/v1/volumes/pvc-2", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(r.registry.get("pvc-2/head").await.is_none());
    let (status, _) = call(&r.router, "GET", "/v1/volumes/pvc-2/snapshots", None, None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Replayed delete: still 204 (CSI idempotency).
    let (status, _) = call(&r.router, "DELETE", "/v1/volumes/pvc-2", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);

    // Config-defined volumes are not deletable over the API.
    let (status, body) = call(&r.router, "DELETE", "/v1/volumes/cfg1", None, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    assert!(r.registry.get("cfg1/head").await.is_some());
}

#[tokio::test(flavor = "multi_thread")]
async fn catalog_survives_provisioner_restart() {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let r1 = rig_with(Arc::clone(&store), Arc::clone(&refs), ApiAuth::Disabled);
    let (status, _) = call(
        &r1.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "pvc-3", "size": VOL_BYTES})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // "Restart": a fresh provisioner over the same store+refs.
    let r2 = rig_with(Arc::clone(&store), Arc::clone(&refs), ApiAuth::Disabled);
    assert_eq!(r2.provisioner.load_catalog().await.unwrap(), 1);
    let (_, vols) = call(&r2.router, "GET", "/v1/volumes", None, None).await;
    assert!(vols
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == "pvc-3"));
    assert!(r2.registry.get("pvc-3/head").await.is_some());

    // Delete on the new instance; a third restart sees an empty catalog.
    let (status, _) = call(&r2.router, "DELETE", "/v1/volumes/pvc-3", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let r3 = rig_with(Arc::clone(&store), Arc::clone(&refs), ApiAuth::Disabled);
    assert_eq!(r3.provisioner.load_catalog().await.unwrap(), 0);
    let _ = (r1, r2);
}

#[tokio::test(flavor = "multi_thread")]
async fn bad_specs_are_rejected() {
    let r = rig();
    for spec in [
        json!({"name": "has/slash", "size": VOL_BYTES}),
        json!({"name": "", "size": VOL_BYTES}),
        json!({"name": "ok", "size": 0}),
        json!({"name": "ok", "size": VOL_BYTES, "kind": "weird"}),
    ] {
        let (status, body) = call(&r.router, "POST", "/v1/volumes", Some(spec), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    }
}

/// Write through an export's engine and make it durable.
async fn write_via_export(r: &Rig, export: &str, off: u64, data: &[u8]) {
    let e = r.registry.get(export).await.unwrap();
    let mut eng = e.engine.write().await;
    eng.write(off, data).unwrap();
    eng.commit(skynbd_format::CommitKind::Flush).await.unwrap();
}

async fn read_via_export(r: &Rig, export: &str, off: u64, len: u64) -> Vec<u8> {
    let e = r.registry.get(export).await.unwrap();
    let eng = e.engine.write().await;
    eng.read(off, len).await.unwrap()
}

/// Fast clone through the API (CSI design §4.1): content sharing, source
/// pinning for the clone's lifetime, and the delete ordering that the
/// pin enforces.
#[tokio::test(flavor = "multi_thread")]
async fn clone_shares_content_pins_source_and_orders_deletes() {
    let r = rig();
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "src-1", "size": VOL_BYTES})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let data: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
    write_via_export(&r, "src-1/head", 0, &data).await;
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/src-1/snapshots",
        Some(json!({"name": "s1"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Clone with size omitted: inherits the source size.
    let clone_spec = json!({"name": "c1", "from": "src-1/s1"});
    let (status, body) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(clone_spec.clone()),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    assert_eq!(body["size"], VOL_BYTES);
    assert_eq!(read_via_export(&r, "c1/head", 0, 4096).await, data);

    // Divergence: writes to the clone leave the source untouched.
    write_via_export(&r, "c1/head", 0, &vec![9u8; 4096]).await;
    assert_eq!(read_via_export(&r, "src-1/head", 0, 4096).await, data);

    // Identical replay: 200 (CSI idempotency).
    let (status, _) = call(&r.router, "POST", "/v1/volumes", Some(clone_spec), None).await;
    assert_eq!(status, StatusCode::OK);

    // The pin makes the source snapshot and volume undeletable.
    let (status, body) = call(
        &r.router,
        "DELETE",
        "/v1/volumes/src-1/snapshots/s1",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");
    let (status, body) = call(&r.router, "DELETE", "/v1/volumes/src-1", None, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{body}");

    // Deleting the clone releases the pin; then the source unwinds.
    let (status, _) = call(&r.router, "DELETE", "/v1/volumes/c1", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, body) = call(
        &r.router,
        "DELETE",
        "/v1/volumes/src-1/snapshots/s1",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");
    let (status, _) = call(&r.router, "DELETE", "/v1/volumes/src-1", None, None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread")]
async fn clone_bad_sources_are_rejected() {
    let r = rig();
    // Malformed `from`.
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "c2", "from": "no-slash"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Unknown source volume/snapshot.
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "c2", "from": "ghost/s1"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    // Clone below source size.
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "src-2", "size": VOL_BYTES})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/src-2/snapshots",
        Some(json!({"name": "s1"})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, body) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "c2", "from": "src-2/s1", "size": VOL_BYTES / 2})),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}

/// The `clone` op is distinct from `volume` (design §7 decision): cloning
/// exfiltrates the source, so `volume` alone must not grant it.
#[tokio::test(flavor = "multi_thread")]
async fn clone_requires_the_clone_op_on_the_source() {
    let issuer = spawn_issuer().await;
    let auth = ApiAuth::Oidc(OidcAuth::new(vec![TrustedIssuer {
        issuer: issuer.clone(),
        audience: AUD.to_string(),
        jwks_url: Some(format!("{issuer}/jwks.json")),
        roles_claim: None,
        tls_ca_pem: None,
        bindings: vec![
            Binding {
                sub: Some("plain-controller".into()),
                role: None,
                volumes: vec!["*".into()],
                ops: vec!["volume".into(), "snapshot".into()],
            },
            Binding {
                sub: Some("cloner".into()),
                role: None,
                volumes: vec!["*".into()],
                ops: vec!["volume".into(), "clone".into()],
            },
        ],
    }]));
    let r = rig_with(
        Arc::new(MemoryStore::new()),
        Arc::new(MemoryRefStore::new()),
        auth,
    );
    let plain = mint(&issuer, "test-key-1", json!({"sub": "plain-controller"}));
    let cloner = mint(&issuer, "test-key-1", json!({"sub": "cloner"}));

    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(json!({"name": "src-3", "size": VOL_BYTES})),
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/src-3/snapshots",
        Some(json!({"name": "s1"})),
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let spec = json!({"name": "c3", "from": "src-3/s1"});
    let (status, body) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(spec.clone()),
        Some(&plain),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    let (status, body) = call(&r.router, "POST", "/v1/volumes", Some(spec), Some(&cloner)).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn volume_op_group_gates_create_and_delete() {
    let issuer = spawn_issuer().await;
    let auth = ApiAuth::Oidc(OidcAuth::new(vec![TrustedIssuer {
        issuer: issuer.clone(),
        audience: AUD.to_string(),
        jwks_url: Some(format!("{issuer}/jwks.json")),
        roles_claim: None,
        tls_ca_pem: None,
        bindings: vec![
            Binding {
                sub: Some("csi-controller".into()),
                role: None,
                volumes: vec!["pvc-*".into()],
                ops: vec!["volume".into()],
            },
            Binding {
                sub: Some("reader".into()),
                role: None,
                volumes: vec!["*".into()],
                ops: vec!["read".into()],
            },
        ],
    }]));
    let r = rig_with(
        Arc::new(MemoryStore::new()),
        Arc::new(MemoryRefStore::new()),
        auth,
    );

    let controller = mint(&issuer, "test-key-1", json!({"sub": "csi-controller"}));
    let reader = mint(&issuer, "test-key-1", json!({"sub": "reader"}));
    let spec = json!({"name": "pvc-9", "size": VOL_BYTES});

    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(spec.clone()),
        Some(&reader),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(spec),
        Some(&controller),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    // Delete is volume-scoped: the controller's pvc-* glob covers it.
    let (status, _) = call(
        &r.router,
        "DELETE",
        "/v1/volumes/pvc-9",
        None,
        Some(&reader),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _) = call(
        &r.router,
        "DELETE",
        "/v1/volumes/pvc-9",
        None,
        Some(&controller),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

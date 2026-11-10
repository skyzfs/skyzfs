//! REST API tests (design §12.1) including instant snapshot publication
//! through the registry sync task (design §7.1 — the S3 scenario's core
//! assertion, in-process).

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use skynbd_api::{ApiState, VolumeHandle};
use skynbd_engine::{Engine, EngineConfig};
use skynbd_nbd::{spawn_registry_sync, DynEngine, Export, ExportRegistry, VolumeBinding};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

const VOL_BYTES: u64 = 128 * 1024;

async fn fixture() -> (axum::Router, Arc<ApiState>, Arc<RwLock<DynEngine>>) {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol_id = VolumeId(ulid::Ulid::new());
    let engine: DynEngine = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol_id,
        VOL_BYTES,
        EngineConfig::default(),
    )
    .await
    .unwrap();
    let engine = Arc::new(RwLock::new(engine));

    let registry = Arc::new(ExportRegistry::new());
    registry
        .register(
            "vol1/head",
            Export {
                engine: Arc::clone(&engine),
                read_only: false,
                size: VOL_BYTES,
            },
        )
        .await;
    spawn_registry_sync(
        Arc::clone(&registry),
        VolumeBinding {
            vol_id,
            vol_name: "vol1".into(),
            store: Arc::clone(&store),
            refs: Arc::clone(&refs),
            cfg: EngineConfig::default(),
        },
    );

    let state = Arc::new(ApiState {
        auth: skynbd_api::ApiAuth::Disabled,
        enroll: None,
        provisioner: None,
        snap_requests: Default::default(),
        volumes: skynbd_api::static_volumes(HashMap::from([(
            "vol1".to_string(),
            VolumeHandle {
                vol_id,
                size: VOL_BYTES,
                engine: Arc::clone(&engine),
                store: Arc::clone(&store),
                refs: Arc::clone(&refs),
                cfg: EngineConfig::default(),
                lease_holder: None,
                prepared: Arc::new(Mutex::new(Default::default())),
            },
        )])),
        registry,
    });
    (skynbd_api::router(Arc::clone(&state)), state, engine)
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(match body {
            Some(v) => Body::from(v.to_string()),
            None => Body::empty(),
        })
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap()
    };
    (status, json)
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_lifecycle_and_instant_publication() {
    let (router, state, engine) = fixture().await;

    // Volume listing.
    let (st, v) = call(&router, "GET", "/v1/volumes", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v[0]["name"], "vol1");
    assert_eq!(v[0]["size"], VOL_BYTES);

    // Write some data, then snapshot via the API.
    engine.write().await.write(0, &[7u8; 4096]).unwrap();
    let t0 = std::time::Instant::now();
    let (st, s) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "s1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{s}");
    assert_eq!(s["name"], "s1");

    // Instant publication (design §7.1): the RO export appears via the
    // sync task well inside the S3 scenario's 1s budget.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    loop {
        if let Some(exp) = state.registry.get("vol1/snap/s1").await {
            assert!(exp.read_only);
            assert_eq!(
                exp.engine.write().await.read(0, 4096).await.unwrap(),
                vec![7u8; 4096]
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "export not published within 1s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    eprintln!("snapshot→mountable in {:?}", t0.elapsed());

    // Duplicate name → 409; bad name → 400; unknown volume → 404.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "s1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "bad/name"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/nope/snapshots",
        Some(serde_json::json!({"name": "x"})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // List + exports include the snapshot.
    let (_, snaps) = call(&router, "GET", "/v1/volumes/vol1/snapshots", None).await;
    assert_eq!(snaps.as_array().unwrap().len(), 1);
    let (_, exports) = call(&router, "GET", "/v1/exports", None).await;
    let names: Vec<String> = exports
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().into())
        .collect();
    assert!(names.contains(&"vol1/head".to_string()));
    assert!(names.contains(&"vol1/snap/s1".to_string()));

    // Delete: 204, ref gone, export withdrawn.
    let (st, _) = call(&router, "DELETE", "/v1/volumes/vol1/snapshots/s1", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while state.registry.get("vol1/snap/s1").await.is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "export not withdrawn within 1s"
        );
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let (st, _) = call(&router, "DELETE", "/v1/volumes/vol1/snapshots/s1", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn rollback_endpoint() {
    let (router, _state, engine) = fixture().await;

    engine.write().await.write(0, &[0xA; 4096]).unwrap();
    call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "s1"})),
    )
    .await;
    {
        let mut eng = engine.write().await;
        eng.write(0, &[0xB; 4096]).unwrap();
        eng.commit(skynbd_format::CommitKind::Flush).await.unwrap();
    }

    // Dirty rollback → 409.
    engine.write().await.write(0, &[0xE; 512]).unwrap();
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/rollback",
        Some(serde_json::json!({"snapshot": "s1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    engine
        .write()
        .await
        .commit(skynbd_format::CommitKind::Flush)
        .await
        .unwrap();

    // Clean rollback restores snapshot content.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/rollback",
        Some(serde_json::json!({"snapshot": "s1"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        engine.write().await.read(0, 4096).await.unwrap(),
        vec![0xA; 4096]
    );

    // Unknown snapshot → 404.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/rollback",
        Some(serde_json::json!({"snapshot": "nope"})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

// ------------------------------------------------------------- M10a

/// Bearer auth: /v1 routes 401 without/with-wrong token, pass with the
/// right one; /healthz stays open (agent design doc §9).
#[tokio::test(flavor = "multi_thread")]
async fn bearer_auth_guards_v1_not_healthz() {
    let (_router, state, engine) = fixture().await;
    // Rebuild the router with auth enabled on the same state contents.
    let state = Arc::new(ApiState {
        auth: skynbd_api::ApiAuth::Static("sekrit".into()),
        enroll: None,
        provisioner: None,
        snap_requests: Default::default(),
        volumes: state.volumes.clone(),
        registry: Arc::clone(&state.registry),
    });
    let router = skynbd_api::router(Arc::clone(&state));
    drop(engine);

    let req = |auth: Option<&str>, uri: &str| {
        let mut b = Request::builder().method("GET").uri(uri);
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(Body::empty()).unwrap()
    };

    let r = router.clone().oneshot(req(None, "/healthz")).await.unwrap();
    assert_eq!(r.status(), StatusCode::OK, "healthz open");

    let r = router
        .clone()
        .oneshot(req(None, "/v1/volumes"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = router
        .clone()
        .oneshot(req(Some("Bearer wrong"), "/v1/volumes"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

    let r = router
        .clone()
        .oneshot(req(Some("Bearer sekrit"), "/v1/volumes"))
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
}

/// The features header rides on every response — successes, auth
/// refusals and routing misses alike — since the agent reads it from
/// whichever response it saw last (prepare and ensure, before any batch).
#[tokio::test(flavor = "multi_thread")]
async fn every_response_advertises_the_api_features() {
    let (_router, state, _engine) = fixture().await;
    let state = Arc::new(ApiState {
        auth: skynbd_api::ApiAuth::Static("sekrit".into()),
        enroll: None,
        provisioner: None,
        snap_requests: Default::default(),
        volumes: state.volumes.clone(),
        registry: Arc::clone(&state.registry),
    });
    let router = skynbd_api::router(state);
    for (auth, method, uri, want) in [
        (None, "GET", "/healthz", StatusCode::OK),
        (None, "GET", "/v1/volumes", StatusCode::UNAUTHORIZED),
        (Some("Bearer sekrit"), "GET", "/v1/volumes", StatusCode::OK),
        (
            Some("Bearer sekrit"),
            "GET",
            "/v1/no-such-route",
            StatusCode::NOT_FOUND,
        ),
    ] {
        let mut b = Request::builder().method(method).uri(uri);
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        let r = router
            .clone()
            .oneshot(b.body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), want, "{method} {uri}");
        let features = r
            .headers()
            .get(skynbd_api::FEATURES_HEADER)
            .unwrap_or_else(|| panic!("{method} {uri}: no features header"))
            .to_str()
            .unwrap();
        assert!(
            features.split(',').any(|f| f.trim() == "prepared-apply"),
            "{method} {uri}: {features}"
        );
    }
}

/// Pins listing + snapshot range read (the agent's observation/verify
/// endpoints).
#[tokio::test(flavor = "multi_thread")]
async fn pins_list_and_snapshot_data_read() {
    let (router, _state, engine) = fixture().await;

    // Content + snapshot.
    let pat: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    engine.write().await.write(4096, &pat).unwrap();
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "s1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // Pins: empty → pin → listed → unpin → empty.
    let (st, v) = call(&router, "GET", "/v1/volumes/vol1/snapshots/s1/pins", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v, serde_json::json!([]));
    call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots/s1/pins",
        Some(serde_json::json!({"holder": "h1"})),
    )
    .await;
    let (_, v) = call(&router, "GET", "/v1/volumes/vol1/snapshots/s1/pins", None).await;
    assert_eq!(v, serde_json::json!(["h1"]));

    // Data read: bytes match what was written, offset-correct.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/volumes/vol1/snapshots/s1/data?off=4096&len=8192")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&bytes[..], &pat[..]);

    // Over-cap len rejected.
    let req = Request::builder()
        .method("GET")
        .uri("/v1/volumes/vol1/snapshots/s1/data?off=0&len=999999999")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

/// Response-shape pins (nightly lesson, round 1): the S14a rig asserted a
/// nonexistent "errors" field on fsck. Pin the exact JSON shapes external
/// consumers (rig scripts, HttpControlPlane) depend on, so drift fails
/// here in PR CI instead of in a privileged nightly.
#[tokio::test(flavor = "multi_thread")]
async fn response_shapes_are_pinned() {
    let (router, _state, engine) = fixture().await;
    engine.write().await.write(0, &[7u8; 4096]).unwrap();
    call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "s1"})),
    )
    .await;

    // fsck: {"refs": [[name, err|null], ...]} — NO "errors" field.
    let (st, v) = call(&router, "POST", "/v1/volumes/vol1/fsck", None).await;
    assert_eq!(st, StatusCode::OK);
    let refs = v["refs"].as_array().expect("refs array");
    assert!(refs.iter().any(|e| e[0] == "head" && e[1].is_null()));
    assert!(v.get("errors").is_none(), "no errors field exists");

    // volumes: name/size/flush_seq exactly.
    let (_, v) = call(&router, "GET", "/v1/volumes", None).await;
    let vol = v[0].as_object().unwrap();
    for k in ["name", "size", "flush_seq"] {
        assert!(vol.contains_key(k), "volumes missing {k}");
    }

    // prepared status: the fields HttpControlPlane deserializes.
    let (_, p) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/prepared",
        Some(serde_json::json!({"base": "s1"})),
    )
    .await;
    let id = p["snap_id"].as_str().expect("snap_id").to_string();
    let (_, s) = call(
        &router,
        "GET",
        &format!("/v1/volumes/vol1/prepared/{id}"),
        None,
    )
    .await;
    for k in [
        "snap_id",
        "state",
        "base_ref",
        "op_watermark",
        "last_commit",
    ] {
        assert!(s.get(k).is_some(), "prepared status missing {k}");
    }
    assert!(s["op_watermark"].is_u64());
    call(
        &router,
        "DELETE",
        &format!("/v1/volumes/vol1/prepared/{id}"),
        None,
    )
    .await;
}

/// `/apply` refuses what it does not understand — an unknown op kind, a
/// per-op field or a top-level field — instead of applying the rest and
/// checkpointing past the part it dropped. Each refused batch carries an
/// attach of real data at offset 0: the published snapshot reads zeros
/// there, so nothing of a refused batch staged — while the accepted
/// batch's attach of the same object, at 4 KiB, reads back.
#[tokio::test(flavor = "multi_thread")]
async fn apply_refuses_unknown_ops_and_fields() {
    let (router, _state, _engine) = fixture().await;
    let data = vec![0xA7u8; 4096];
    let hash = blake3::hash(&data).to_hex().to_string();
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/v1/volumes/vol1/objects/{hash}"))
        .body(Body::from(data))
        .unwrap();
    let st = router.clone().oneshot(req).await.unwrap().status();
    assert!(st.is_success(), "object PUT: {st}");

    let (st, p) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/prepared",
        Some(serde_json::json!({})),
    )
    .await;
    assert!(st.is_success(), "{st} {p}");
    let id = p["snap_id"].as_str().expect("snap_id").to_string();
    let uri = format!("/v1/volumes/vol1/prepared/{id}/apply");
    let post = |body: serde_json::Value| {
        let req = Request::builder()
            .method("POST")
            .uri(&uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let router = router.clone();
        async move { router.oneshot(req).await.unwrap().status() }
    };
    let attach = serde_json::json!({"op": "attach", "off": 0, "len": 4096, "hash": hash});
    let clear = serde_json::json!({"op": "clear", "off": 8192, "len": 4096});
    for bad in [
        serde_json::json!({"op_id": 1, "ops": [attach, {"op": "punch", "off": 0, "len": 4096}]}),
        serde_json::json!({"op_id": 1, "ops": [attach, {"op": "clear", "off": 8192, "len": 4096, "fua": true}]}),
        serde_json::json!({"op_id": 1, "ops": [attach], "epoch": 7}),
    ] {
        assert_eq!(
            post(bad.clone()).await,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{bad}"
        );
    }
    let (st, v) = call(
        &router,
        "POST",
        &uri,
        Some(serde_json::json!({"op_id": 1, "ops": [
            clear,
            {"op": "attach", "off": 4096, "len": 4096, "hash": hash},
        ]})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    let (st, v) = call(
        &router,
        "POST",
        &format!("/v1/volumes/vol1/prepared/{id}/publish"),
        Some(serde_json::json!({"name": "refused"})),
    )
    .await;
    assert!(st.is_success(), "publish: {st} {v}");
    let req = Request::builder()
        .uri("/v1/volumes/vol1/snapshots/refused/data?off=0&len=8192")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let got = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        &got[..4096],
        &[0u8; 4096][..],
        "a refused batch's attach was staged"
    );
    // The check can see an attach: the accepted batch's one is there.
    assert_eq!(&got[4096..], &[0xA7u8; 4096][..]);
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_requests_and_alias_lifecycle() {
    let (router, _state, engine) = fixture().await;

    // Requests: empty → accepted (idempotently) → listed → completed.
    let (st, body) = call(&router, "GET", "/v1/volumes/vol1/snapshot-requests", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body, serde_json::json!([]));
    for _ in 0..2 {
        let (st, _) = call(
            &router,
            "POST",
            "/v1/volumes/vol1/snapshot-requests",
            Some(serde_json::json!({"name": "csi-snap-1"})),
        )
        .await;
        assert_eq!(st, StatusCode::ACCEPTED);
    }
    let (_, body) = call(&router, "GET", "/v1/volumes/vol1/snapshot-requests", None).await;
    assert_eq!(body, serde_json::json!(["csi-snap-1"]));
    // Unknown volume → 404, both directions.
    let (st, _) = call(&router, "GET", "/v1/volumes/nope/snapshot-requests", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &router,
        "DELETE",
        "/v1/volumes/vol1/snapshot-requests/csi-snap-1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, body) = call(&router, "GET", "/v1/volumes/vol1/snapshot-requests", None).await;
    assert_eq!(body, serde_json::json!([]));
    let (st, status) = call(
        &router,
        "GET",
        "/v1/volumes/vol1/snapshot-requests/csi-snap-1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(status["completed"], true);

    // Alias: publish g0 the plain way, alias it, and both names list
    // with the same commit; deleting the alias leaves the source.
    let (st, g0) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "g0"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, aliased) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots/g0/alias",
        Some(serde_json::json!({"name": "csi-snap-1"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(aliased["commit"], g0["commit"]);
    // Idempotent replay.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots/g0/alias",
        Some(serde_json::json!({"name": "csi-snap-1"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // Alias of a missing snapshot → 404; name collision → 409.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots/gX/alias",
        Some(serde_json::json!({"name": "other"})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    engine.write().await.write(0, &[3u8; 4096]).unwrap(); // distinct commit for g1
    let (st, snap2) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "g1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_ne!(snap2["commit"], g0["commit"], "distinct commits expected");
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots/g1/alias",
        Some(serde_json::json!({"name": "csi-snap-1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    let (_, listed) = call(&router, "GET", "/v1/volumes/vol1/snapshots", None).await;
    let names: Vec<&str> = listed
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["csi-snap-1", "g0", "g1"]);

    let (st, _) = call(
        &router,
        "DELETE",
        "/v1/volumes/vol1/snapshots/csi-snap-1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, listed) = call(&router, "GET", "/v1/volumes/vol1/snapshots", None).await;
    assert_eq!(listed.as_array().unwrap().len(), 2);
    let (st, _) = call(
        &router,
        "GET",
        "/v1/volumes/vol1/snapshot-requests/csi-snap-1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// D-M12: a pending snapshot request exposes the agent cycle's reported
/// status — phase/progress from progress reports, attempts/last_error
/// from failure reports, each kind leaving the other's fields alone —
/// and the capture time of exactly the cut the agent bound it to.
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_request_status_reports_the_cycle() {
    let (router, _state, _engine) = fixture().await;

    // No request yet → 404.
    let (st, _) = call(
        &router,
        "GET",
        "/v1/volumes/vol1/snapshot-requests/s1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Cycle reports against a volume with no pending requests are an
    // accepted no-op — the agent doesn't track whether anyone watches.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/cycle-status",
        Some(serde_json::json!({"phase": "capturing"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    // Unknown volume still 404s.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/nope/cycle-status",
        Some(serde_json::json!({"phase": "capturing"})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Fresh request: pending, zero progress, no error, no cut yet.
    file_request(&router, "s1").await;
    let (st, body) = call(
        &router,
        "GET",
        "/v1/volumes/vol1/snapshot-requests/s1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        body,
        serde_json::json!({
            "phase": "pending", "ops_done": 0, "ops_total": 0,
            "attempts": 0, "last_error": null, "capture_time_ms": null,
            "completed": false, "capture_lost": false
        })
    );

    // A failure before the cut: attempts/error land on the request.
    report(
        &router,
        serde_json::json!({"attempts": 1, "last_error": "freeze: EBUSY"}),
    )
    .await;
    assert_eq!(
        request_status(&router, "s1").await["last_error"],
        "freeze: EBUSY"
    );

    // The cut binds s1: its time lands and the stale failure (which
    // described the cycle that never made this cut) clears.
    report(
        &router,
        serde_json::json!({
            "phase": "backfilling", "ops_done": 10, "ops_total": 100,
            "captures": [{"name": "s1", "capture_time_ms": 1_700_000_123_456u64}]
        }),
    )
    .await;
    let body = request_status(&router, "s1").await;
    assert_eq!(body["phase"], "backfilling");
    assert_eq!(body["ops_done"], 10);
    assert_eq!(body["ops_total"], 100);
    assert_eq!(body["attempts"], 0);
    assert_eq!(body["last_error"], serde_json::Value::Null);
    assert_eq!(body["capture_time_ms"], 1_700_000_123_456u64);

    // Failure report after the cut → attempts/error land, progress and
    // the cut's time untouched.
    report(
        &router,
        serde_json::json!({"attempts": 2, "last_error": "publish: timeout"}),
    )
    .await;
    let body = request_status(&router, "s1").await;
    assert_eq!(body["phase"], "backfilling");
    assert_eq!(body["ops_done"], 10);
    assert_eq!(body["attempts"], 2);
    assert_eq!(body["last_error"], "publish: timeout");
    assert_eq!(body["capture_time_ms"], 1_700_000_123_456u64);

    // Re-filing the request (the external-snapshotter re-polls) keeps
    // the accumulated status instead of resetting it.
    file_request(&router, "s1").await;
    assert_eq!(
        request_status(&router, "s1").await["last_error"],
        "publish: timeout"
    );

    // A request filed later shares the volume's progress, but never an
    // earlier cut: only the agent's per-request binding gives it a time.
    file_request(&router, "s2").await;
    report(
        &router,
        serde_json::json!({
            "phase": "visible", "ops_done": 100,
            "captures": [{"name": "s1", "capture_time_ms": 1_700_000_123_456u64}]
        }),
    )
    .await;
    for name in ["s1", "s2"] {
        let body = request_status(&router, name).await;
        assert_eq!(body["phase"], "visible", "{name}");
        assert_eq!(body["ops_done"], 100, "{name}");
    }
    assert_eq!(
        request_status(&router, "s2").await["capture_time_ms"],
        serde_json::Value::Null
    );

    // Completion takes the request off the agent's queue but keeps its
    // status (and the cut's time) for CSI's ready/replay response.
    let (st, _) = call(
        &router,
        "DELETE",
        "/v1/volumes/vol1/snapshot-requests/s1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let body = request_status(&router, "s1").await;
    assert_eq!(body["completed"], true);
    assert_eq!(body["capture_time_ms"], 1_700_000_123_456u64);
    let (_, listed) = call(&router, "GET", "/v1/volumes/vol1/snapshot-requests", None).await;
    assert_eq!(listed, serde_json::json!(["s2"]));
}

/// A request's point in time never moves. Re-announcing the same cut is a
/// no-op; a different time for an already-bound request means the agent
/// lost the original cut, so the request fails rather than silently
/// adopting a later one. An explicit loss report fails it the same way.
/// Lost requests leave the agent's queue but stay readable.
#[tokio::test(flavor = "multi_thread")]
async fn capture_bindings_are_first_write_wins_and_losses_fail_the_request() {
    let (router, _state, _engine) = fixture().await;
    for name in ["moved", "dropped", "fine"] {
        file_request(&router, name).await;
    }
    report(
        &router,
        serde_json::json!({
            "captures": [
                {"name": "moved", "capture_time_ms": 100u64},
                {"name": "fine", "capture_time_ms": 100u64}
            ]
        }),
    )
    .await;
    report(
        &router,
        serde_json::json!({
            "captures": [
                {"name": "moved", "capture_time_ms": 200u64},
                {"name": "fine", "capture_time_ms": 100u64}
            ],
            "lost": ["dropped"]
        }),
    )
    .await;
    let moved = request_status(&router, "moved").await;
    assert_eq!(
        moved["capture_time_ms"], 100u64,
        "the first cut's time stays"
    );
    assert_eq!(moved["capture_lost"], true);
    assert_eq!(
        request_status(&router, "dropped").await["capture_lost"],
        true
    );
    let fine = request_status(&router, "fine").await;
    assert_eq!(fine["capture_time_ms"], 100u64);
    assert_eq!(fine["capture_lost"], false);
    let (_, listed) = call(&router, "GET", "/v1/volumes/vol1/snapshot-requests", None).await;
    assert_eq!(listed, serde_json::json!(["fine"]));

    // A binding-only report leaves the volume-level fields alone.
    assert_eq!(request_status(&router, "fine").await["phase"], "pending");

    // Re-filing a lost request does not revive it; deleting its (never
    // aliased) snapshot releases it even though the delete itself 404s.
    file_request(&router, "dropped").await;
    assert_eq!(
        request_status(&router, "dropped").await["capture_lost"],
        true
    );
    let (st, _) = call(
        &router,
        "DELETE",
        "/v1/volumes/vol1/snapshots/dropped",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &router,
        "GET",
        "/v1/volumes/vol1/snapshot-requests/dropped",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

/// Re-filing a completed request is idempotent while its snapshot exists
/// (a CSI re-POST that raced the agent's alias must not erase the cut's
/// time), and starts a new request once the snapshot is gone.
#[tokio::test(flavor = "multi_thread")]
async fn refiling_a_completed_request_keeps_it_while_the_snapshot_exists() {
    let (router, _state, engine) = fixture().await;
    file_request(&router, "csi-1").await;
    report(
        &router,
        serde_json::json!({"captures": [{"name": "csi-1", "capture_time_ms": 100u64}]}),
    )
    .await;
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "csi-1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, _) = call(
        &router,
        "DELETE",
        "/v1/volumes/vol1/snapshot-requests/csi-1",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    file_request(&router, "csi-1").await;
    let body = request_status(&router, "csi-1").await;
    assert_eq!(body["completed"], true);
    assert_eq!(body["capture_time_ms"], 100u64);

    // Snapshot removed out of band (not through this API's delete, which
    // would drop the request too): the next filing is a new request.
    engine.write().await.snapshot_delete("csi-1").await.unwrap();
    file_request(&router, "csi-1").await;
    let body = request_status(&router, "csi-1").await;
    assert_eq!(body["completed"], false);
    assert_eq!(body["capture_time_ms"], serde_json::Value::Null);
}

/// Completed statuses are retained for replays, but only the most recent
/// ones: the in-memory map cannot grow with every snapshot ever taken.
#[tokio::test(flavor = "multi_thread")]
async fn completed_request_statuses_are_bounded() {
    let (router, _state, _engine) = fixture().await;
    let cap = skynbd_api::COMPLETED_SNAP_REQUESTS_RETAINED;
    for i in 0..=cap {
        let name = format!("csi-{i}");
        file_request(&router, &name).await;
        let (st, _) = call(
            &router,
            "DELETE",
            &format!("/v1/volumes/vol1/snapshot-requests/{name}"),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::NO_CONTENT);
    }
    let (st, _) = call(
        &router,
        "GET",
        "/v1/volumes/vol1/snapshot-requests/csi-0",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND, "oldest completion evicted");
    assert_eq!(
        request_status(&router, &format!("csi-{cap}")).await["completed"],
        true
    );
    assert_eq!(request_status(&router, "csi-1").await["completed"], true);
}

async fn file_request(router: &axum::Router, name: &str) {
    let (st, _) = call(
        router,
        "POST",
        "/v1/volumes/vol1/snapshot-requests",
        Some(serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED);
}

async fn report(router: &axum::Router, body: serde_json::Value) {
    let (st, _) = call(router, "POST", "/v1/volumes/vol1/cycle-status", Some(body)).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

async fn request_status(router: &axum::Router, name: &str) -> serde_json::Value {
    let (st, body) = call(
        router,
        "GET",
        &format!("/v1/volumes/vol1/snapshot-requests/{name}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{name}: {body}");
    body
}

/// Origin pointer lifecycle over the API (D-M15 task 15a): init adopts
/// the newest generation, explicit repoint moves the row, the dedicated
/// `origin` pin, and the live export together.
#[tokio::test(flavor = "multi_thread")]
async fn origin_pointer_lifecycle() {
    let (router, state, engine) = fixture().await;
    let (refs, vol_id) = {
        let vols = state.volumes.read().await;
        let h = &vols["vol1"];
        (Arc::clone(&h.refs), h.vol_id)
    };

    // No generations yet: nothing to resolve or initialize from.
    let (st, _) = call(&router, "GET", "/v1/volumes/vol1/origin", None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Publish two generations with distinct content.
    engine.write().await.write(0, &[0xA0u8; 4096]).unwrap();
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "g0"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    engine.write().await.write(0, &[0xB1u8; 4096]).unwrap();
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "g1"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // GET previews the init target without writing anything.
    let (st, body) = call(&router, "GET", "/v1/volumes/vol1/origin", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["ref"], "snap/g1");
    assert_eq!(body["initialized"], false);
    assert!(state.registry.get("vol1/origin").await.is_none());

    // Init (empty body): adopt the newest generation, pin it, publish
    // the export.
    let (st, body) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["ref"], "snap/g1");
    assert_eq!(body["initialized"], true);
    assert_eq!(body["export"], "vol1/origin");
    assert_eq!(
        refs.pins(vol_id, "snap/g1").await.unwrap(),
        vec!["origin".to_string()],
        "the pointer must pin what it names"
    );
    let export = state
        .registry
        .get("vol1/origin")
        .await
        .expect("origin export");
    assert!(export.read_only);

    // Invariant E (15g): a repoint naming an older generation no-ops —
    // 200, the response reports the row's actual state, and neither the
    // row, the pins, nor the served commit move.
    let (st, body) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({"ref": "snap/g0"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["ref"], "snap/g1",
        "regression must not move the pointer"
    );
    assert!(refs.pins(vol_id, "snap/g0").await.unwrap().is_empty());
    assert_eq!(
        refs.pins(vol_id, "snap/g1").await.unwrap(),
        vec!["origin".to_string()]
    );
    let g1 = refs.get(vol_id, "snap/g1").await.unwrap().unwrap().commit;
    let served = {
        let export = state.registry.get("vol1/origin").await.unwrap();
        let commit = export.engine.write().await.head_commit_hash();
        commit
    };
    assert_eq!(served, g1, "no-op repoint must leave the served commit");

    // A pinned generation refuses deletion: the pointer's target is safe.
    let (st, _) = call(&router, "DELETE", "/v1/volumes/vol1/snapshots/g1", None).await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Forward repoint: row, pin, and served commit all move together.
    engine.write().await.write(0, &[0xC2u8; 4096]).unwrap();
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "g2"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, body) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({"ref": "snap/g2"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["ref"], "snap/g2");
    assert!(refs.pins(vol_id, "snap/g1").await.unwrap().is_empty());
    assert_eq!(
        refs.pins(vol_id, "snap/g2").await.unwrap(),
        vec!["origin".to_string()]
    );
    let g2 = refs.get(vol_id, "snap/g2").await.unwrap().unwrap().commit;
    let served = {
        let export = state.registry.get("vol1/origin").await.unwrap();
        let commit = export.engine.write().await.head_commit_hash();
        commit
    };
    assert_eq!(served, g2, "live export must serve the repointed commit");

    // Bad targets: unknown snapshot, non-snapshot ref, and — new with
    // Invariant E — an alias: it exists, but the pointer only accepts
    // the totally ordered generation namespace.
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({"ref": "snap/nope"})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({"ref": "head"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/snapshots",
        Some(serde_json::json!({"name": "golden"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, body) = call(
        &router,
        "POST",
        "/v1/volumes/vol1/origin",
        Some(serde_json::json!({"ref": "snap/golden"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{body}");

    // GET now reflects the initialized pointer.
    let (st, body) = call(&router, "GET", "/v1/volumes/vol1/origin", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["ref"], "snap/g2");
    assert_eq!(body["initialized"], true);
    assert_eq!(body["served"], "snap/g2");
}

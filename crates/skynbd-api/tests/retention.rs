//! Retention thinning (design §6.2, task 15f): the property under test
//! is the full chain — thin decides, ONE transaction deletes, the §10
//! sweep frees actual bytes — because each link alone can pass while
//! the system still leaks: a thin that deletes refs without a sweep
//! bounds nothing, and a sweep with nothing thinned frees nothing.
//!
//! Time is not mocked; the dials are. `window_s: 0` makes every
//! generation old, `window_s: huge` makes none old, `prepared_ttl_s: 0`
//! makes every session abandoned — the boundary arithmetic is the
//! planner's, the tests pick which side of it the world sits on.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{ObjectRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use tower::ServiceExt;

const VOL_BYTES: u64 = 1024 * 1024;
const HUGE_WINDOW_S: u64 = 100 * 365 * 24 * 60 * 60;

struct Rig {
    router: axum::Router,
    state: Arc<ApiState>,
    store: Arc<dyn ObjectStore>,
    refs: Arc<dyn RefStore>,
}

fn rig() -> Rig {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(ObjectRefStore::new(Arc::clone(&store)));
    let registry = Arc::new(ExportRegistry::new());
    let provisioner = Arc::new(Provisioner::new(
        Arc::clone(&store),
        Arc::clone(&refs),
        Arc::clone(&registry),
        0,
    ));
    let state = Arc::new(ApiState {
        volumes: Arc::clone(&provisioner.volumes),
        registry,
        auth: ApiAuth::Disabled,
        enroll: None,
        provisioner: Some(provisioner),
        snap_requests: Default::default(),
    });
    Rig {
        router: skynbd_api::router(Arc::clone(&state)),
        state,
        store,
        refs,
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let req = match body {
        Some(v) => Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .unwrap(),
    };
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

/// A lineage with distinct data under each generation, so every
/// generation's closure holds at least one segment nothing else
/// references — the bytes whose fate the tests assert.
async fn lineage(r: &Rig, vol: &str, generations: u32) {
    let (st, _) = call(
        &r.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({"name": vol, "size": VOL_BYTES})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    for g in 0..generations {
        {
            let h = r.state.volumes.read().await.get(vol).cloned().unwrap();
            let mut eng = h.engine.write().await;
            let block = vec![g as u8 + 1; 65536];
            eng.write(u64::from(g) * 65536, &block).unwrap();
        }
        let (st, _) = call(
            &r.router,
            "POST",
            &format!("/v1/volumes/{vol}/snapshots"),
            Some(serde_json::json!({"name": format!("g{g}")})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED, "snapshot g{g}");
    }
}

async fn object_count(store: &Arc<dyn ObjectStore>) -> usize {
    store.list("skynbd/v1/").await.unwrap().len()
}

#[tokio::test(flavor = "multi_thread")]
async fn thin_deletes_old_generations_and_the_sweep_frees_bytes() {
    let r = rig();
    lineage(&r, "lin", 4).await;

    // g1 pinned (the system's own liveness protocol wins over age),
    // g2 aliased (a configured snapshot on the retention track).
    let (st, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/lin/snapshots/g1/pins",
        Some(serde_json::json!({"holder": "test"})),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/lin/snapshots/g2/alias",
        Some(serde_json::json!({"name": "nightly"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let before = object_count(&r.store).await;
    let (st, body) = call(
        &r.router,
        "POST",
        "/v1/volumes/lin/thin",
        Some(serde_json::json!({"window_s": 0})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["deleted_generations"],
        serde_json::json!(["snap/g0"]),
        "g0 is the only candidate: g3 newest, g1 pinned, g2 aliased"
    );
    assert_eq!(body["kept_pinned"], 1, "{body}");
    assert_eq!(body["kept_configured"], 1, "{body}");
    assert_eq!(body["gc"], "ran", "{body}");

    let names: Vec<String> = r
        .refs
        .list(skynbd_api::provision::volume_id("lin"))
        .await
        .unwrap()
        .into_iter()
        .map(|(n, _)| n)
        .collect();
    assert!(!names.contains(&"snap/g0".to_string()));
    for keep in ["snap/g1", "snap/g2", "snap/g3", "snap/nightly"] {
        assert!(names.contains(&keep.to_string()), "{keep} must survive");
    }

    let after = object_count(&r.store).await;
    assert!(
        after < before,
        "the sweep must free g0's objects ({before} -> {after})"
    );

    // The repack measurement rides the report: live segments with
    // their live-byte accounting (task 15f groundwork).
    let (st, gc) = call(&r.router, "POST", "/v1/volumes/lin/gc", None).await;
    assert_eq!(st, StatusCode::OK);
    let segs = gc["segments"].as_array().expect("segments in GcReport");
    assert!(!segs.is_empty(), "live segments must be reported");
    for s in segs {
        assert!(s["live_bytes"].as_u64().unwrap() > 0, "{s}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_and_the_newest_generation_are_absolute() {
    let r = rig();
    lineage(&r, "win", 3).await;
    let (st, body) = call(
        &r.router,
        "POST",
        "/v1/volumes/win/thin",
        Some(serde_json::json!({"window_s": HUGE_WINDOW_S})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(body["deleted_generations"], serde_json::json!([]));
    assert_eq!(body["kept_window"], 2, "g0,g1 in-window; g2 newest");
    assert_eq!(body["gc"], "nothing", "an empty plan runs no sweep");
}

#[tokio::test(flavor = "multi_thread")]
async fn abandoned_prepared_sessions_are_collected_with_their_pins() {
    let r = rig();
    lineage(&r, "zom", 2).await;
    let (st, body) = call(
        &r.router,
        "POST",
        "/v1/volumes/zom/prepared",
        Some(serde_json::json!({"base": "g0"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    let vid = skynbd_api::provision::volume_id("zom");
    assert_eq!(
        r.refs.pins(vid, "snap/g0").await.unwrap().len(),
        1,
        "the session pins its base"
    );

    // TTL 0: every session is on the abandoned side of the boundary.
    // The generous production default is the same arithmetic.
    let (st, body) = call(
        &r.router,
        "POST",
        "/v1/volumes/zom/thin",
        Some(serde_json::json!({"window_s": HUGE_WINDOW_S, "prepared_ttl_s": 0})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(body["swept_sessions"].as_array().unwrap().len(), 1);

    assert!(
        r.refs.pins(vid, "snap/g0").await.unwrap().is_empty(),
        "the zombie's base pin is released"
    );
    assert!(
        r.refs.meta_list(vid, "prepared/").await.unwrap().is_empty(),
        "the session row is collected"
    );
    assert!(
        !r.refs
            .list(vid)
            .await
            .unwrap()
            .iter()
            .any(|(n, _)| n.starts_with("prepared/")),
        "the hidden ref is collected"
    );
}

/// The fail-safe direction on the session path (#102 review, finding
/// 1): a hidden ref whose commit object cannot be read makes the
/// session's last activity invisible — the pass must skip it, not
/// judge it on the older ULID bound. The CAS-exact delete cannot
/// protect a live session here (its ref hasn't moved; the evidence
/// read failed), so the planner itself has to refuse.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_with_unreadable_evidence_is_never_swept() {
    let r = rig();
    lineage(&r, "evi", 2).await;
    let (st, body) = call(
        &r.router,
        "POST",
        "/v1/volumes/evi/prepared",
        Some(serde_json::json!({"base": "g0"})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");

    // Break the evidence read: delete the commit object the session's
    // hidden ref names (a transient store failure at plan time looks
    // identical to the planner).
    let vid = skynbd_api::provision::volume_id("evi");
    let hidden_commit = r
        .refs
        .list(vid)
        .await
        .unwrap()
        .into_iter()
        .find(|(n, _)| n.starts_with("prepared/"))
        .map(|(_, rv)| rv.commit)
        .expect("prepared session has a hidden ref");
    r.store
        .delete(&skynbd_engine::keys::commit(vid, &hidden_commit))
        .await
        .unwrap();

    // TTL 0 would sweep any session it judges — so surviving proves
    // the session was skipped, not merely young.
    let (st, body) = call(
        &r.router,
        "POST",
        "/v1/volumes/evi/thin",
        Some(serde_json::json!({"window_s": HUGE_WINDOW_S, "prepared_ttl_s": 0})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    assert_eq!(
        body["swept_sessions"],
        serde_json::json!([]),
        "unreadable evidence must keep the session"
    );
    assert_eq!(
        r.refs.pins(vid, "snap/g0").await.unwrap().len(),
        1,
        "the live session's base pin survives"
    );
}

/// The 15h boundary: an epoch-stamped thin (the agent-driven path for
/// node-held lineages) is refused when the epoch doesn't match the
/// lease row — same 409 shape as every other fenced op.
#[tokio::test(flavor = "multi_thread")]
async fn fenced_thin_against_a_stale_epoch_is_409() {
    let r = rig();
    lineage(&r, "fen", 2).await;
    let (st, _) = call(
        &r.router,
        "POST",
        "/v1/volumes/fen/thin",
        Some(serde_json::json!({"window_s": 0, "epoch": 5})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
}

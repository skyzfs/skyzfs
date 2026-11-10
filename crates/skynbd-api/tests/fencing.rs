//! Invariant F over the API (task 15h): node-epoch transaction fencing.
//!
//! The zombie-node story, end to end: a node serves a lineage under its
//! own identity and stamps the granted epoch into every mutating
//! request; a reschedule moves the lease (epoch bump) and every request
//! still stamped with the superseded epoch is refused with the
//! distinguishable 409 `lease_lost` — nothing lands. Plus the two §8.2
//! extended-outage refinements: the salvage branch (a fenced publish
//! re-executed as a write-once `salvage/*` fork under the current
//! epoch) and tip-unchanged leniency (a re-acquire after the thief
//! expires publishes normally under the NEW epoch).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{Clock, LeaseConfig, ObjectRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use tower::ServiceExt;

const VOL_BYTES: u64 = 128 * 1024;
const TTL_MS: u64 = 10_000;

struct Server {
    router: axum::Router,
    refs: Arc<dyn RefStore>,
    prov: Arc<Provisioner>,
}

/// One skynbdd over the shared bucket, on a test-controlled clock so
/// lease expiry is exercised by advancing time.
fn server(shared: &Arc<MemoryStore>, holder: &str, now: &Arc<AtomicU64>) -> Server {
    let store: Arc<dyn ObjectStore> = Arc::clone(shared) as Arc<dyn ObjectStore>;
    let clock: Clock = {
        let now = Arc::clone(now);
        Arc::new(move || now.load(Ordering::SeqCst))
    };
    let lc = LeaseConfig {
        holder: holder.to_string(),
        ttl_ms: TTL_MS,
    };
    let refs: Arc<dyn RefStore> = Arc::new(
        ObjectRefStore::new(Arc::clone(&store))
            .with_clock(clock)
            .with_owner(lc.clone()),
    );
    let registry = Arc::new(ExportRegistry::new());
    let provisioner = Arc::new(
        Provisioner::new(
            Arc::clone(&store),
            Arc::clone(&refs),
            Arc::clone(&registry),
            0,
        )
        .with_lease(lc),
    );
    let state = Arc::new(ApiState {
        volumes: Arc::clone(&provisioner.volumes),
        registry,
        auth: ApiAuth::Disabled,
        enroll: None,
        provisioner: Some(Arc::clone(&provisioner)),
        snap_requests: Default::default(),
    });
    Server {
        router: skynbd_api::router(state),
        refs,
        prov: provisioner,
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let req = match body {
        Some(b) => Request::builder()
            .method(method)
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => Request::builder()
            .method(method)
            .uri(path)
            .body(Body::empty())
            .unwrap(),
    };
    let resp = router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, json)
}

fn assert_lease_lost(status: StatusCode, body: &serde_json::Value, holder: &str, epoch: u64) {
    assert_eq!(status, StatusCode::CONFLICT, "expected 409, body {body}");
    assert_eq!(body["error"], "lease_lost", "body {body}");
    assert_eq!(body["holder"], holder, "body {body}");
    assert_eq!(body["epoch"], epoch, "body {body}");
}

/// Create the lineage volume and hand its lease to `node`, returning the
/// granted epoch from the serve response.
async fn serve_as(router: &axum::Router, vol: &str, node: &str) -> u64 {
    let (st, body) = call(
        router,
        "POST",
        &format!("/v1/volumes/{vol}/serve"),
        Some(serde_json::json!({ "holder": node })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "serve as {node}: {body}");
    assert_eq!(body["holder"], node, "{body}");
    body["epoch"].as_u64().expect("serve grants an epoch")
}

/// Open a prepared session, stamped with the caller's stage epoch (the
/// prepared lifecycle is fenced too — C2: prepare pins the base).
async fn prepare(router: &axum::Router, vol: &str, epoch: u64) -> String {
    let (st, body) = call(
        router,
        "POST",
        &format!("/v1/volumes/{vol}/prepared"),
        Some(serde_json::json!({ "base": null, "epoch": epoch })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    body["snap_id"].as_str().unwrap().to_string()
}

/// The zombie-node conformance flow (task 15h, decision 10): node A
/// serves the lineage → epoch E1; node B steals after A's lease expires
/// → E2; every A-stamped mutation (publish, repoint, pin, unpin) is a
/// 409 `lease_lost` that lands nothing; the same mutations stamped E2
/// succeed.
#[tokio::test]
async fn zombie_node_transactions_are_fenced() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s = server(&shared, "srv-1", &now);

    // Controller-side create: the server's own holder takes the lease.
    let (st, _) = call(
        &s.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "lin", "size": VOL_BYTES, "kind": "lineage" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // NodeStage A: the server hands its own lease to the node (epoch
    // bumps — a holder change is a fence-line move).
    let e1 = serve_as(&s.router, "lin", "node-a").await;
    assert_eq!(e1, 2, "srv-1 held epoch 1; the handover bumps");

    // A's in-flight prepared session.
    let a_prep = prepare(&s.router, "lin", e1).await;

    // Reschedule attempt while A's lease is live: refused, nothing moves.
    let (st, _) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/serve",
        Some(serde_json::json!({ "holder": "node-b" })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "a live foreign lease must hold");

    // A's lease expires (partition); B's stage steals it.
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);
    let e2 = serve_as(&s.router, "lin", "node-b").await;
    assert_eq!(e2, e1 + 1, "the steal moves the fence line");

    // B publishes g0 under its epoch: the lineage tip advances.
    let b_prep = prepare(&s.router, "lin", e2).await;
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{b_prep}/publish"),
        Some(serde_json::json!({ "name": "g0", "epoch": e2 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "current-epoch publish: {body}");

    // The zombie's publish, stamped E1: 409 lease_lost carrying the
    // actual owner, and nothing lands.
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep}/publish"),
        Some(serde_json::json!({ "name": "g1", "epoch": e1 })),
    )
    .await;
    assert_lease_lost(st, &body, "node-b", e2);
    let vid = skynbd_api::provision::volume_id("lin");
    assert_eq!(
        s.refs.get(vid, "snap/g1").await.unwrap(),
        None,
        "a fenced publish must land nothing"
    );

    // Zombie repoint, pin, unpin: all fenced; the same ops under E2 work.
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/origin",
        Some(serde_json::json!({ "ref": "snap/g0", "epoch": e1 })),
    )
    .await;
    assert_lease_lost(st, &body, "node-b", e2);
    assert_eq!(
        s.refs.meta_get(vid, "origin").await.unwrap(),
        None,
        "a fenced repoint must not move the pointer row"
    );
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/origin",
        Some(serde_json::json!({ "ref": "snap/g0", "epoch": e2 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/snapshots/g0/pins",
        Some(serde_json::json!({ "holder": "agent-baseline:lin", "epoch": e1 })),
    )
    .await;
    assert_lease_lost(st, &body, "node-b", e2);
    assert!(
        !s.refs
            .pins(vid, "snap/g0")
            .await
            .unwrap()
            .contains(&"agent-baseline:lin".to_string()),
        "a fenced pin must not land"
    );
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/snapshots/g0/pins",
        Some(serde_json::json!({ "holder": "agent-baseline:lin", "epoch": e2 })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");

    // The GC hazard Invariant E never covered: a zombie's UNPIN is
    // fenced too — it could release the generation the new node stages
    // from.
    let (st, body) = call(
        &s.router,
        "DELETE",
        &format!("/v1/volumes/lin/snapshots/g0/pins/agent-baseline:lin?epoch={e1}"),
        None,
    )
    .await;
    assert_lease_lost(st, &body, "node-b", e2);
    assert!(s.refs.pins(vid, "snap/g0").await.unwrap().len() >= 2);
    let (st, body) = call(
        &s.router,
        "DELETE",
        &format!("/v1/volumes/lin/snapshots/g0/pins/agent-baseline:lin?epoch={e2}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");

    // Alias with a stale epoch is refused at the door too.
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/snapshots/g0/alias",
        Some(serde_json::json!({ "name": "csi-snap", "epoch": e1 })),
    )
    .await;
    assert_lease_lost(st, &body, "node-b", e2);

    // Epoch-less (legacy) callers FAIL CLOSED on a node-held volume:
    // an unfenced mutation here would bypass Invariant F wholesale.
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/snapshots/g0/pins",
        Some(serde_json::json!({ "holder": "legacy" })),
    )
    .await;
    assert_lease_lost(st, &body, "node-b", e2);
    assert!(
        !s.refs
            .pins(vid, "snap/g0")
            .await
            .unwrap()
            .contains(&"legacy".to_string()),
        "the refused epoch-less pin must not land"
    );
}

/// §8.2 salvage: a fenced publish with `salvage: true` re-executes as
/// the write-once fork `salvage/<name>-e<stale_epoch>` under the
/// CURRENT lease — no lineage pins move, the fork takes no pins of its
/// own (its ref keeps the closure alive), and the lineage tip is
/// untouched.
#[tokio::test]
async fn fenced_publish_can_salvage_out_of_lineage() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s = server(&shared, "srv-1", &now);
    call(
        &s.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "lin", "size": VOL_BYTES, "kind": "lineage" })),
    )
    .await;
    let e1 = serve_as(&s.router, "lin", "node-a").await;
    let a_prep = prepare(&s.router, "lin", e1).await;

    // A uploads a chunk before being fenced (the outage's real data).
    let data = vec![0xABu8; 4096];
    let hash = blake3::hash(&data);
    let (st, _) = call(
        &s.router,
        "PUT",
        &format!("/v1/volumes/lin/objects/{}", hash.to_hex()),
        None,
    )
    .await;
    // PUT with an empty body mismatches the hash; send properly instead.
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/v1/volumes/lin/objects/{}", hash.to_hex()))
        .body(Body::from(data.clone()))
        .unwrap();
    let resp = s.router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep}/attach"),
        Some(serde_json::json!({
            "op_id": 1,
            "ranges": [{ "off": 0, "len": 4096, "hash": hash.to_hex().to_string() }],
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Reschedule: B steals after expiry and publishes g0 (tip advances —
    // no leniency for A).
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);
    let e2 = serve_as(&s.router, "lin", "node-b").await;
    let b_prep = prepare(&s.router, "lin", e2).await;
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{b_prep}/publish"),
        Some(serde_json::json!({ "name": "g0", "epoch": e2 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // Without salvage: fenced, nothing lands (proven above); with
    // salvage: the fork lands OUTSIDE the lineage under the current
    // epoch.
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep}/publish"),
        Some(serde_json::json!({ "name": "backlog", "epoch": e1, "salvage": true })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "salvage publish: {body}");
    let salvage_ref = format!("salvage/backlog-e{e1}");
    assert_eq!(body["name"], salvage_ref.as_str(), "{body}");

    let vid = skynbd_api::provision::volume_id("lin");
    let rv = s
        .refs
        .get(vid, &salvage_ref)
        .await
        .unwrap()
        .expect("the salvage ref exists");
    assert_ne!(rv.commit, [0u8; 32]);
    // Write-once namespace outside the lineage: no pins of its own, no
    // lineage pins moved, tip untouched.
    assert!(
        s.refs.pins(vid, &salvage_ref).await.unwrap().is_empty(),
        "salvage takes no pins — its ref is its anchor"
    );
    assert!(
        s.refs
            .pins(vid, "snap/g0")
            .await
            .unwrap()
            .contains(&"publisher".to_string()),
        "the lineage tip's publish pin is untouched"
    );
    // The prepared session is consumed (its hidden ref is gone).
    assert_eq!(
        s.refs
            .get(vid, &format!("prepared/{a_prep}"))
            .await
            .unwrap(),
        None
    );
    // A salvage replay is write-once: the ref exists → conflict.
    let a_prep2 = prepare(&s.router, "lin", e2).await;
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep2}/publish"),
        Some(serde_json::json!({ "name": "backlog", "epoch": e1, "salvage": true })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{body}");
    assert_ne!(
        body["error"], "lease_lost",
        "a name collision is an ordinary 409"
    );
}

/// §8.2 tip-unchanged leniency: needs no new server path — re-acquire is
/// possible exactly when the thief's lease expired, and a re-acquired
/// holder publishes normally under its NEW epoch. The superseded epoch
/// itself stays fenced forever.
#[tokio::test]
async fn tip_unchanged_reacquire_publishes_under_the_new_epoch() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s = server(&shared, "srv-1", &now);
    call(
        &s.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "lin", "size": VOL_BYTES, "kind": "lineage" })),
    )
    .await;
    let e1 = serve_as(&s.router, "lin", "node-a").await;
    let a_prep = prepare(&s.router, "lin", e1).await;

    // B steals mid-outage… and dies before publishing anything: the tip
    // never advanced.
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);
    let e2 = serve_as(&s.router, "lin", "node-b").await;
    assert_eq!(e2, e1 + 1);
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);

    // A returns: re-acquire succeeds (the thief's lease expired) at the
    // next epoch — the two-live-writers race is excluded by exactly this
    // gate.
    let e3 = serve_as(&s.router, "lin", "node-a").await;
    assert_eq!(e3, e2 + 1);

    // The old epoch stays fenced (no resurrection)…
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep}/publish"),
        Some(serde_json::json!({ "name": "g0", "epoch": e1 })),
    )
    .await;
    assert_lease_lost(st, &body, "node-a", e3);

    // …while the re-acquired epoch publishes the backlog normally.
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep}/publish"),
        Some(serde_json::json!({ "name": "g0", "epoch": e3 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let vid = skynbd_api::provision::volume_id("lin");
    assert!(s.refs.get(vid, "snap/g0").await.unwrap().is_some());
}

/// B4b, both halves: on a SERVER-held volume every epoch-less mutation
/// keeps the legacy unfenced path (config-volume back-compat); the
/// moment the lease is handed to a NODE, the same epoch-less requests
/// are refused with the structured 409 — and the correctly-stamped
/// variants still work.
#[tokio::test]
async fn epochless_requests_fail_closed_only_on_node_held_volumes() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s = server(&shared, "srv-1", &now);
    let (st, _) = call(
        &s.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "lin", "size": VOL_BYTES, "kind": "lineage" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let vid = skynbd_api::provision::volume_id("lin");

    // Server-held (srv-1, epoch 1): the whole epoch-less legacy surface
    // works — prepare, publish, pin, unpin, alias, repoint.
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/prepared",
        Some(serde_json::json!({ "base": null })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
    let prep = body["snap_id"].as_str().unwrap().to_string();
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{prep}/publish"),
        Some(serde_json::json!({ "name": "g0" })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::OK,
        "epoch-less publish, server-held: {body}"
    );
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/snapshots/g0/pins",
        Some(serde_json::json!({ "holder": "legacy" })),
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    let (st, body) = call(
        &s.router,
        "DELETE",
        "/v1/volumes/lin/snapshots/g0/pins/legacy",
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "{body}");
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/snapshots/g0/alias",
        Some(serde_json::json!({ "name": "legacy-snap" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let (st, body) = call(&s.router, "POST", "/v1/volumes/lin/origin", None).await;
    assert_eq!(
        st,
        StatusCode::OK,
        "epoch-less origin init, server-held: {body}"
    );

    // Hand the lease to node-a: everything epoch-less now fails closed.
    let e = serve_as(&s.router, "lin", "node-a").await;
    let refused: &[(&str, &str, Option<serde_json::Value>)] = &[
        (
            "POST",
            "/v1/volumes/lin/prepared",
            Some(serde_json::json!({ "base": null })),
        ),
        (
            "POST",
            "/v1/volumes/lin/snapshots/g0/pins",
            Some(serde_json::json!({ "holder": "legacy2" })),
        ),
        (
            "DELETE",
            "/v1/volumes/lin/snapshots/g0/pins/publisher",
            None,
        ),
        (
            "POST",
            "/v1/volumes/lin/snapshots/g0/alias",
            Some(serde_json::json!({ "name": "late-snap" })),
        ),
        (
            "POST",
            "/v1/volumes/lin/origin",
            Some(serde_json::json!({ "ref": "snap/g0" })),
        ),
    ];
    for (method, path, body) in refused {
        let (st, resp) = call(&s.router, method, path, body.clone()).await;
        assert_lease_lost(st, &resp, "node-a", e);
    }
    assert!(
        !s.refs
            .pins(vid, "snap/g0")
            .await
            .unwrap()
            .contains(&"legacy2".to_string()),
        "nothing epoch-less lands on a node-held volume"
    );

    // The correctly-stamped prepared lifecycle still works: prepare,
    // then abort (which unpins the base) — both fenced (C2).
    let prep = prepare(&s.router, "lin", e).await;
    let (st, body) = call(
        &s.router,
        "DELETE",
        &format!("/v1/volumes/lin/prepared/{prep}?epoch={}", e + 7),
        None,
    )
    .await;
    assert_lease_lost(st, &body, "node-a", e);
    let (st, body) = call(
        &s.router,
        "DELETE",
        &format!("/v1/volumes/lin/prepared/{prep}?epoch={e}"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT, "stamped abort: {body}");

    // A stale-stamped prepare is refused outright (C2).
    let (st, body) = call(
        &s.router,
        "POST",
        "/v1/volumes/lin/prepared",
        Some(serde_json::json!({ "base": null, "epoch": e + 7 })),
    )
    .await;
    assert_lease_lost(st, &body, "node-a", e);
}

/// C3: a legitimate re-acquire (epoch bumped) whose local baseline is
/// STALE — the interim owner advanced the tip — publishes the SAME
/// generation number the interim owner already published. The write-once
/// CasRef collision refuses it (an ordinary 409, not lease_lost) and
/// nothing lands: the interim owner's generation is untouched.
#[tokio::test]
async fn stale_baseline_republish_of_same_generation_is_refused() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s = server(&shared, "srv-1", &now);
    call(
        &s.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "lin", "size": VOL_BYTES, "kind": "lineage" })),
    )
    .await;
    let vid = skynbd_api::provision::volume_id("lin");

    // A publishes g0 under its epoch, then goes silent.
    let e1 = serve_as(&s.router, "lin", "node-a").await;
    let a_prep = prepare(&s.router, "lin", e1).await;
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep}/publish"),
        Some(serde_json::json!({ "name": "g0", "epoch": e1 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // The interim owner B steals and advances the tip: g1 exists now.
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);
    let e2 = serve_as(&s.router, "lin", "node-b").await;
    let b_prep = prepare(&s.router, "lin", e2).await;
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{b_prep}/publish"),
        Some(serde_json::json!({ "name": "g1", "epoch": e2 })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{body}");
    let b_g1 = s.refs.get(vid, "snap/g1").await.unwrap().unwrap();

    // B dies; A legitimately re-acquires (epoch bumps again) — but its
    // stale baseline still says the tip is g0, so its next target is g1.
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);
    let e3 = serve_as(&s.router, "lin", "node-a").await;
    assert_eq!(e3, e2 + 1);
    let a_prep2 = prepare(&s.router, "lin", e3).await;
    let (st, body) = call(
        &s.router,
        "POST",
        &format!("/v1/volumes/lin/prepared/{a_prep2}/publish"),
        Some(serde_json::json!({ "name": "g1", "epoch": e3 })),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "the write-once generation ref must collide: {body}"
    );
    assert_ne!(
        body["error"], "lease_lost",
        "a name collision is an ordinary conflict, not a fence: {body}"
    );
    assert_eq!(
        s.refs.get(vid, "snap/g1").await.unwrap().unwrap(),
        b_g1,
        "the interim owner's generation is untouched"
    );
}

/// C8: a stale server's AMBIENT write (no request epoch anywhere) after
/// another server stole the volume's lease surfaces as the structured
/// 409 `lease_lost` — never a 500.
#[tokio::test]
async fn stale_servers_ambient_write_is_409_lease_lost() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s1 = server(&shared, "srv-1", &now);
    let (st, _) = call(
        &s1.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-a", "size": VOL_BYTES })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);

    // srv-1 goes silent; srv-2 steals via serve after the TTL.
    now.fetch_add(TTL_MS + 1, Ordering::SeqCst);
    let s2 = server(&shared, "srv-2", &now);
    let (st, body) = call(&s2.router, "POST", "/v1/volumes/pvc-a/serve", None).await;
    assert_eq!(st, StatusCode::OK, "{body}");

    // srv-1's stale handle mutates ambiently (snapshot create): the
    // ambient fence refuses with the actual owner — 409, structured.
    let (st, body) = call(
        &s1.router,
        "POST",
        "/v1/volumes/pvc-a/snapshots",
        Some(serde_json::json!({ "name": "stale" })),
    )
    .await;
    assert_lease_lost(st, &body, "srv-2", 2);
}

/// C5: the server's renewal loop renews only SERVER-held leases. A
/// node-held lease is the node's own to renew (every fenced write is
/// its heartbeat), so a dead node's lease EXPIRES and TTL failover
/// works — while the server's own idle volumes stay renewed.
#[tokio::test]
async fn lease_renewal_skips_node_held_volumes() {
    let shared = Arc::new(MemoryStore::new());
    let now = Arc::new(AtomicU64::new(1_000_000));
    let s = server(&shared, "srv-1", &now);
    for name in ["lin", "pvc"] {
        let (st, _) = call(
            &s.router,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({ "name": name, "size": VOL_BYTES, "kind": "lineage" })),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
    }
    // Hand lin to node-a; pvc stays server-held.
    let e_lin = serve_as(&s.router, "lin", "node-a").await;
    let (lin, pvc) = (
        skynbd_api::provision::volume_id("lin"),
        skynbd_api::provision::volume_id("pvc"),
    );
    let lin_before = s.refs.lease_get(lin).await.unwrap().unwrap();

    // Half a TTL later the server's renewal tick runs.
    now.fetch_add(TTL_MS / 2, Ordering::SeqCst);
    let lost = s.prov.renew_leases().await;
    assert!(lost.is_empty(), "{lost:?}");

    let lin_after = s.refs.lease_get(lin).await.unwrap().unwrap();
    assert_eq!(
        (lin_after.holder.as_str(), lin_after.expires_at_ms),
        ("node-a", lin_before.expires_at_ms),
        "the node-held lease must NOT be renewed by the server"
    );
    let pvc_after = s.refs.lease_get(pvc).await.unwrap().unwrap();
    assert!(
        pvc_after.expires_at_ms > lin_before.expires_at_ms,
        "the server-held lease renews: {pvc_after:?}"
    );

    // The dead node's lease runs out despite further renewal ticks —
    // TTL failover: another node's stage steals at the next epoch.
    now.fetch_add(TTL_MS / 2 + 1, Ordering::SeqCst);
    let lost = s.prov.renew_leases().await;
    assert!(lost.is_empty(), "{lost:?}");
    let e2 = serve_as(&s.router, "lin", "node-b").await;
    assert_eq!(e2, e_lin + 1, "the expired node lease is stealable");
    // …while the renewed server-held volume is still firmly held.
    let pvc_lease = s.refs.lease_get(pvc).await.unwrap().unwrap();
    assert_eq!(pvc_lease.holder, "srv-1");
    assert!(pvc_lease.expires_at_ms > now.load(Ordering::SeqCst));
}

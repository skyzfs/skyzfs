//! The D-M6 handoff over the API: two servers (two provisioners, two
//! fenced `ObjectRefStore` instances) sharing one bucket. Reschedule is
//! a handoff, not a race — serve is refused while the lease is held,
//! granted after release at the next epoch, and the loser of any
//! interleaving is told so.

mod common;

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{LeaseConfig, ObjectRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use tower::ServiceExt;

const VOL_BYTES: u64 = 128 * 1024;

struct Server {
    router: axum::Router,
    registry: Arc<ExportRegistry>,
    refs: Arc<dyn RefStore>,
}

/// One "skynbdd" over the shared bucket: its own fenced refs store and
/// its own provisioner, leasing as `holder`.
fn server(shared: &Arc<MemoryStore>, holder: &str) -> Server {
    let store: Arc<dyn ObjectStore> = Arc::clone(shared) as Arc<dyn ObjectStore>;
    let lc = LeaseConfig {
        holder: holder.to_string(),
        ttl_ms: 60_000,
    };
    let refs: Arc<dyn RefStore> =
        Arc::new(ObjectRefStore::new(Arc::clone(&store)).with_owner(lc.clone()));
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
        registry: Arc::clone(&registry),
        auth: ApiAuth::Disabled,
        enroll: None,
        provisioner: Some(provisioner),
        snap_requests: Default::default(),
    });
    Server {
        router: skynbd_api::router(state),
        registry,
        refs,
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> StatusCode {
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
    router.clone().oneshot(req).await.unwrap().status()
}

/// The full reschedule story: A creates and owns; B is refused while A
/// holds; A releases (commit + teardown + lease); B serves at the next
/// epoch; A is now the refused one. Exports follow the lease.
#[tokio::test]
async fn reschedule_is_a_handoff_not_a_race() {
    let shared = Arc::new(MemoryStore::new());
    let a = server(&shared, "node-a");
    let b = server(&shared, "node-b");

    // A creates (and thereby leases) the volume.
    let st = call(
        &a.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-h", "size": VOL_BYTES })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    assert!(a.registry.get("pvc-h/head").await.is_some());
    let vid = skynbd_api::provision::volume_id("pvc-h");
    let lease = a.refs.lease_get(vid).await.unwrap().unwrap();
    assert_eq!((lease.holder.as_str(), lease.epoch), ("node-a", 1));

    // B cannot serve while A holds the lease.
    let st = call(&b.router, "POST", "/v1/volumes/pvc-h/serve", None).await;
    assert_eq!(st, StatusCode::CONFLICT, "held lease must refuse serve");
    assert!(b.registry.get("pvc-h/head").await.is_none());

    // A releases: the D-M6 unstage path.
    let st = call(&a.router, "POST", "/v1/volumes/pvc-h/release", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(
        a.registry.get("pvc-h/head").await.is_none(),
        "release stops serving"
    );

    // B serves from the shared catalog, at the next epoch.
    let st = call(&b.router, "POST", "/v1/volumes/pvc-h/serve", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(b.registry.get("pvc-h/head").await.is_some());
    let lease = b.refs.lease_get(vid).await.unwrap().unwrap();
    assert_eq!(
        (lease.holder.as_str(), lease.epoch),
        ("node-b", 2),
        "handoff moves the fence line"
    );

    // The tables have turned: A is refused now.
    let st = call(&a.router, "POST", "/v1/volumes/pvc-h/serve", None).await;
    assert_eq!(st, StatusCode::CONFLICT);
}

/// Both endpoints are idempotent, and serve of an unknown name is 404 —
/// the node plugin calls them best-effort and must get clean answers.
#[tokio::test]
async fn serve_and_release_are_idempotent() {
    let shared = Arc::new(MemoryStore::new());
    let a = server(&shared, "node-a");

    call(
        &a.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-i", "size": VOL_BYTES })),
    )
    .await;

    // Serve while already serving: fine.
    let st = call(&a.router, "POST", "/v1/volumes/pvc-i/serve", None).await;
    assert_eq!(st, StatusCode::OK);
    // Release twice: fine.
    assert_eq!(
        call(&a.router, "POST", "/v1/volumes/pvc-i/release", None).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&a.router, "POST", "/v1/volumes/pvc-i/release", None).await,
        StatusCode::NO_CONTENT
    );
    // Serve again after own release: the SAME holder resumes at its own
    // epoch — a holder never fences itself, so nothing it wrote needs
    // fencing out. Only a different taker bumps.
    assert_eq!(
        call(&a.router, "POST", "/v1/volumes/pvc-i/serve", None).await,
        StatusCode::OK
    );
    let vid = skynbd_api::provision::volume_id("pvc-i");
    assert_eq!(a.refs.lease_get(vid).await.unwrap().unwrap().epoch, 1);

    // Unknown volume: 404, not a surprise.
    assert_eq!(
        call(&a.router, "POST", "/v1/volumes/nope/serve", None).await,
        StatusCode::NOT_FOUND
    );
}

/// A released volume's data is where the next server expects it: writes
/// flushed by A before release are readable through B's engine.
#[tokio::test]
async fn handoff_carries_the_data() {
    let shared = Arc::new(MemoryStore::new());
    let a = server(&shared, "node-a");
    let b = server(&shared, "node-b");

    call(
        &a.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-d", "size": VOL_BYTES })),
    )
    .await;
    // Write through A's engine (the way the NBD server does) and leave
    // the overlay dirty — release must commit it (D-M6's "commit and
    // release").
    let a_export = a.registry.get("pvc-d/head").await.unwrap();
    a_export
        .engine
        .write()
        .await
        .write(0, &[0xABu8; 4096])
        .unwrap();

    assert_eq!(
        call(&a.router, "POST", "/v1/volumes/pvc-d/release", None).await,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        call(&b.router, "POST", "/v1/volumes/pvc-d/serve", None).await,
        StatusCode::OK
    );
    let b_export = b.registry.get("pvc-d/head").await.unwrap();
    let read = b_export.engine.write().await.read(0, 4096).await.unwrap();
    assert_eq!(
        read,
        vec![0xABu8; 4096],
        "release committed the dirty overlay under A's epoch"
    );

    // And the clean unstage left the emap behind (D-M7), so B's open —
    // and every open after a graceful handoff — was one GET, not a walk.
    let vid = skynbd_api::provision::volume_id("pvc-d");
    let head_commit = b
        .refs
        .get(vid, "head")
        .await
        .unwrap()
        .expect("head exists")
        .commit;
    assert!(
        shared
            .exists(&skynbd_engine::keys::emap(vid, &head_commit))
            .await
            .unwrap(),
        "release must write the head commit's emap"
    );
}

/// The k3s smoke's sequence: stage, write, unstage (release), then
/// CreateSnapshot through the API. A released volume is still in the
/// catalog, and an API operation must pick it back up (re-serving,
/// re-acquiring the lease) rather than 404 — snapshots of an unstaged
/// volume are the canonical case.
#[tokio::test]
async fn api_ops_reserve_a_released_volume() {
    let shared = Arc::new(MemoryStore::new());
    let a = server(&shared, "node-a");

    call(
        &a.router,
        "POST",
        "/v1/volumes",
        Some(serde_json::json!({ "name": "pvc-s", "size": VOL_BYTES })),
    )
    .await;
    // Write + flush through the engine (what the NBD client's fsync
    // does), then release — the unstage path.
    let export = a.registry.get("pvc-s/head").await.unwrap();
    {
        let mut eng = export.engine.write().await;
        eng.write(0, &[0x5Au8; 4096]).unwrap();
        eng.commit(skynbd_format::CommitKind::Flush).await.unwrap();
    }
    drop(export);
    assert_eq!(
        call(&a.router, "POST", "/v1/volumes/pvc-s/release", None).await,
        StatusCode::NO_CONTENT
    );
    assert!(a.registry.get("pvc-s/head").await.is_none());

    // CreateSnapshot on the released volume: re-serves, snapshots.
    let st = call(
        &a.router,
        "POST",
        "/v1/volumes/pvc-s/snapshots",
        Some(serde_json::json!({ "name": "s1" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "snapshot must re-serve, not 404");
    // And the volume is being served again (exports re-registered).
    assert!(a.registry.get("pvc-s/head").await.is_some());

    // A name nobody ever created still 404s.
    assert_eq!(
        call(
            &a.router,
            "POST",
            "/v1/volumes/ghost/snapshots",
            Some(serde_json::json!({ "name": "s1" })),
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

/// The D-M4 crash handoff over the API: a *cadenced* server dies without
/// release, its head trailing in the refs object. The successor's serve
/// steals the lease after the TTL, runs the epoch-filtered recovery
/// scan, and serves everything the dead server acked.
#[tokio::test]
async fn cadenced_crash_handoff_recovers_acked_flushes() {
    use skynbd_refs::{CadencedRefStore, ObjectRefStore};

    let shared = Arc::new(MemoryStore::new());
    let cadenced = |holder: &str| {
        let store: Arc<dyn ObjectStore> = Arc::clone(&shared) as Arc<dyn ObjectStore>;
        let lc = LeaseConfig {
            holder: holder.to_string(),
            ttl_ms: 50, // a crash test waits this out for real
        };
        let refs: Arc<dyn RefStore> = Arc::new(CadencedRefStore::new(
            ObjectRefStore::new(Arc::clone(&store)).with_owner(lc.clone()),
        ));
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
            registry: Arc::clone(&registry),
            auth: ApiAuth::Disabled,
            enroll: None,
            provisioner: Some(provisioner),
            snap_requests: Default::default(),
        });
        Server {
            router: skynbd_api::router(state),
            registry,
            refs,
        }
    };

    // A creates (serve exports the create state), then flushes into the
    // gap and dies without release.
    let a = cadenced("node-a");
    assert_eq!(
        call(
            &a.router,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({ "name": "pvc-c", "size": VOL_BYTES })),
        )
        .await,
        StatusCode::CREATED
    );
    let a_export = a.registry.get("pvc-c/head").await.unwrap();
    {
        let mut eng = a_export.engine.write().await;
        eng.write(0, &[0xCDu8; 4096]).unwrap();
        eng.commit(skynbd_format::CommitKind::Flush).await.unwrap();
    }
    drop(a_export);
    drop(a); // no release, no export: the crash

    // The refs object still shows the pre-flush head.
    let vid = skynbd_api::provision::volume_id("pvc-c");

    // B steals after the TTL and serves: recovery closes the gap.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let b = cadenced("node-b");
    assert_eq!(
        call(&b.router, "POST", "/v1/volumes/pvc-c/serve", None).await,
        StatusCode::OK
    );
    let lease = b.refs.lease_get(vid).await.unwrap().unwrap();
    assert_eq!((lease.holder.as_str(), lease.epoch), ("node-b", 2));

    let b_export = b.registry.get("pvc-c/head").await.unwrap();
    let read = b_export.engine.write().await.read(0, 4096).await.unwrap();
    assert_eq!(
        read,
        vec![0xCDu8; 4096],
        "the successor must serve every flush the dead server acked"
    );
}

/// C1 (task 15h): the D-M4 recovery scan runs under the GRANTED
/// principal. A cadenced server dies with an acked flush in the cadence
/// gap; the successor's serve hands the lease to a NODE holder — the
/// recovery head CAS and the serve-time export both act as the node,
/// and the node-held serve still recovers everything the dead server
/// acked.
#[tokio::test]
async fn node_held_serve_still_recovers_the_cadence_gap() {
    use skynbd_refs::{CadencedRefStore, ObjectRefStore};

    let shared = Arc::new(MemoryStore::new());
    let cadenced = |holder: &str| {
        let store: Arc<dyn ObjectStore> = Arc::clone(&shared) as Arc<dyn ObjectStore>;
        let lc = LeaseConfig {
            holder: holder.to_string(),
            ttl_ms: 50,
        };
        let refs: Arc<dyn RefStore> = Arc::new(CadencedRefStore::new(
            ObjectRefStore::new(Arc::clone(&store)).with_owner(lc.clone()),
        ));
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
            registry: Arc::clone(&registry),
            auth: ApiAuth::Disabled,
            enroll: None,
            provisioner: Some(provisioner),
            snap_requests: Default::default(),
        });
        Server {
            router: skynbd_api::router(state),
            registry,
            refs,
        }
    };

    let a = cadenced("srv-1");
    assert_eq!(
        call(
            &a.router,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({ "name": "pvc-n", "size": VOL_BYTES })),
        )
        .await,
        StatusCode::CREATED
    );
    let a_export = a.registry.get("pvc-n/head").await.unwrap();
    {
        let mut eng = a_export.engine.write().await;
        eng.write(0, &[0xEEu8; 4096]).unwrap();
        eng.commit(skynbd_format::CommitKind::Flush).await.unwrap();
    }
    drop(a_export);
    drop(a); // crash: no release, the flush lives only in the gap

    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    let b = cadenced("srv-2");
    // The successor serves under a NODE's identity (a thin stage).
    assert_eq!(
        call(
            &b.router,
            "POST",
            "/v1/volumes/pvc-n/serve",
            Some(serde_json::json!({ "holder": "node-x" })),
        )
        .await,
        StatusCode::OK
    );
    let vid = skynbd_api::provision::volume_id("pvc-n");
    let lease = b.refs.lease_get(vid).await.unwrap().unwrap();
    assert_eq!((lease.holder.as_str(), lease.epoch), ("node-x", 2));

    let b_export = b.registry.get("pvc-n/head").await.unwrap();
    let read = b_export.engine.write().await.read(0, 4096).await.unwrap();
    assert_eq!(
        read,
        vec![0xEEu8; 4096],
        "gap recovery must run under the node principal, not get ambient-fenced"
    );
}

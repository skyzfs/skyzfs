//! The serve path OWNS the origin export (D-M15 Invariant D, the
//! structural fix for the restage-wedge class): a successful serve of a
//! volume whose origin pointer row is initialized means `<vol>/origin`
//! is registered before serve returns — synchronously and fallibly, not
//! delegated to the registry sync task's asynchronous, warn-only
//! initial full sync.
//!
//! The failure these tests reconstruct is the one the k3s smoke hit: a
//! node's stage attaches `<vol>/origin` the instant its lineage serve
//! returns, and the export's only builder was the spawned full sync —
//! a race on a good day, and on a bad one (a single transient refs
//! failure inside that task) an export that never appears at all, with
//! every stage retry looping against a registry entry nothing repairs.
//! The client-side POST /origin papered over it; the serve path now
//! guarantees it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_nbd::ExportRegistry;
use skynbd_refs::{
    CasOutcome, Lease, LeaseOutcome, ObjectRefStore, RefError, RefEvent, RefOp, RefStore, RefValue,
    TxnOutcome,
};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::CommitHash;
use skynbd_types::VolumeId;
use tower::ServiceExt;

const VOL_BYTES: u64 = 128 * 1024;

/// A delegating ref store whose NEXT `list` fails once — the exact
/// shape of the transient failure that used to strand the origin
/// export: `full_sync` calls `list` first, so an armed failure kills
/// the sync task's initial pass (warn-only in production) while every
/// other path keeps working. The serve-path reconcile never lists (the
/// initialized pointer resolves through `meta_get` + `get`), which is
/// what lets this test discriminate the two builders deterministically.
struct FlakyRefs {
    inner: Arc<dyn RefStore>,
    fail_next_list: AtomicBool,
    fired: AtomicBool,
    /// When set, the next transaction that moves the origin row (a
    /// `set_origin` that has read the row and is about to commit) says
    /// so on `held` and waits for a permit on `go` (M1.3aj).
    hold_origin_cas: AtomicBool,
    held: tokio::sync::Notify,
    go: tokio::sync::Semaphore,
}

impl FlakyRefs {
    async fn hold_if_armed(&self, ops: &[RefOp]) {
        let moves_the_row = ops.iter().any(
            |op| matches!(op, RefOp::MetaCas { key, .. } if key == skynbd_nbd::origin::ORIGIN_META_KEY),
        );
        if moves_the_row && self.hold_origin_cas.swap(false, Ordering::SeqCst) {
            self.held.notify_one();
            self.go
                .acquire()
                .await
                .expect("the gate is never closed")
                .forget();
        }
    }
}

#[async_trait::async_trait]
impl RefStore for FlakyRefs {
    async fn get(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Option<RefValue>> {
        self.inner.get(vol, name).await
    }
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<CommitHash>,
    ) -> skynbd_refs::Result<CasOutcome> {
        self.inner
            .compare_and_swap(vol, name, expected, new_commit)
            .await
    }
    async fn list(&self, vol: VolumeId) -> skynbd_refs::Result<Vec<(String, RefValue)>> {
        if self.fail_next_list.swap(false, Ordering::SeqCst) {
            self.fired.store(true, Ordering::SeqCst);
            return Err(RefError::Backend("injected transient list failure".into()));
        }
        self.inner.list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<RefEvent> {
        self.inner.watch()
    }
    async fn transact(&self, vol: VolumeId, ops: Vec<RefOp>) -> skynbd_refs::Result<TxnOutcome> {
        self.hold_if_armed(&ops).await;
        self.inner.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<TxnOutcome> {
        self.hold_if_armed(&ops).await;
        self.inner.transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Vec<String>> {
        self.inner.pins(vol, name).await
    }
    async fn meta_get(&self, vol: VolumeId, key: &str) -> skynbd_refs::Result<Option<Vec<u8>>> {
        self.inner.meta_get(vol, key).await
    }
    async fn meta_list(
        &self,
        vol: VolumeId,
        prefix: &str,
    ) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
        self.inner.meta_list(vol, prefix).await
    }
    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<LeaseOutcome> {
        self.inner.lease_acquire(vol, holder, ttl_ms).await
    }
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> skynbd_refs::Result<()> {
        self.inner.lease_release(vol, holder).await
    }
    async fn lease_get(&self, vol: VolumeId) -> skynbd_refs::Result<Option<Lease>> {
        self.inner.lease_get(vol).await
    }
    async fn flush_volume(&self, vol: VolumeId) -> skynbd_refs::Result<()> {
        self.inner.flush_volume(vol).await
    }
    async fn flush_volume_as(
        &self,
        vol: VolumeId,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<()> {
        self.inner.flush_volume_as(vol, holder, epoch).await
    }
}

struct Rig {
    router: axum::Router,
    registry: Arc<ExportRegistry>,
    flaky: Arc<FlakyRefs>,
    provisioner: Arc<Provisioner>,
}

impl Rig {
    /// The refs as they are, under the test's hooks.
    fn refs(&self) -> &Arc<dyn RefStore> {
        &self.flaky.inner
    }
}

fn rig() -> Rig {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let flaky = Arc::new(FlakyRefs {
        inner: Arc::new(ObjectRefStore::new(Arc::clone(&store))),
        fail_next_list: AtomicBool::new(false),
        fired: AtomicBool::new(false),
        hold_origin_cas: AtomicBool::new(false),
        held: tokio::sync::Notify::new(),
        go: tokio::sync::Semaphore::new(0),
    });
    let refs: Arc<dyn RefStore> = Arc::clone(&flaky) as Arc<dyn RefStore>;
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
        auth: ApiAuth::Disabled,
        enroll: None,
        provisioner: Some(Arc::clone(&provisioner)),
        snap_requests: Default::default(),
    });
    Rig {
        router: skynbd_api::router(state),
        registry,
        flaky,
        provisioner,
    }
}

async fn call(
    router: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> StatusCode {
    // The content-type header only when there is a body: an empty body
    // declared as JSON is a 400 from `Option<Json<..>>` extractors.
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
    router.clone().oneshot(req).await.unwrap().status()
}

/// [`call`], with the answer's body.
async fn call_body(
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
    let bytes = http_body_util::BodyExt::collect(resp.into_body())
        .await
        .unwrap()
        .to_bytes();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|_| {
            serde_json::Value::String(String::from_utf8_lossy(&bytes).into_owned())
        })
    };
    (status, json)
}

/// Create a lineage-shaped volume, publish a generation, initialize the
/// pointer — the durable state a staged node leaves behind.
async fn establish(r: &Rig) {
    assert_eq!(
        call(
            &r.router,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({"name": "lin", "size": VOL_BYTES})),
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &r.router,
            "POST",
            "/v1/volumes/lin/snapshots",
            Some(serde_json::json!({"name": "g0"})),
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &r.router,
            "POST",
            "/v1/volumes/lin/origin",
            Some(serde_json::json!({})),
        )
        .await,
        StatusCode::OK
    );
    assert!(
        r.registry.get("lin/origin").await.is_some(),
        "POST /origin publishes the export"
    );
}

/// The core guarantee: after a release killed the export, a successful
/// re-serve has rebuilt it BY THE TIME SERVE RETURNS — even when the
/// sync task's initial full sync dies on a transient refs failure, the
/// exact combination that used to leave the export absent until the
/// next generation event (never, on an idle lineage) while every stage
/// retry failed its attach.
#[tokio::test(flavor = "multi_thread")]
async fn serve_rebuilds_the_origin_export_despite_a_dead_full_sync() {
    let r = rig();
    establish(&r).await;

    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/release", None).await,
        StatusCode::NO_CONTENT,
    );
    assert!(
        r.registry.get("lin/origin").await.is_none(),
        "release tears the export down with the serving rig"
    );

    // Arm the transient failure the wedge was made of: the re-serve's
    // spawned full sync will die on its first call.
    r.flaky.fail_next_list.store(true, Ordering::SeqCst);
    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/serve", None).await,
        StatusCode::OK,
    );
    assert!(
        r.registry.get("lin/origin").await.is_some(),
        "the serve path itself rebuilt the origin export — a client \
         attaching the moment serve returns finds it, full sync or no"
    );

    // The replayed serve (a kubelet-retried stage) converges too: kill
    // the export behind the serving rig's back and replay.
    r.registry.unregister("lin/origin").await;
    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/serve", None).await,
        StatusCode::OK,
    );
    assert!(
        r.registry.get("lin/origin").await.is_some(),
        "the already-serving replay re-converges the export"
    );
    // The consumer of the armed failure is the SPAWNED full sync, so
    // "was it consumed" is an eventually-true fact, not a same-instant
    // one — reading `fired` right after serve returned lost that race
    // on a loaded CI runner. Bounded wait, then assert.
    for _ in 0..500 {
        if r.flaky.fired.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        r.flaky.fired.load(Ordering::SeqCst),
        "the armed failure was actually consumed (by the full sync) — \
         without this the test could rot into asserting nothing"
    );
}

/// A volume with NO pointer row serves exactly as before — the
/// reconcile is a no-op that must neither publish an export (an
/// uninitialized origin tracking \"newest\" would move on every
/// publish) nor fail the serve.
#[tokio::test(flavor = "multi_thread")]
async fn serve_without_a_pointer_row_publishes_no_origin() {
    let r = rig();
    assert_eq!(
        call(
            &r.router,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({"name": "plain", "size": VOL_BYTES})),
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        call(
            &r.router,
            "POST",
            "/v1/volumes/plain/snapshots",
            Some(serde_json::json!({"name": "g0"})),
        )
        .await,
        StatusCode::CREATED
    );
    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/plain/release", None).await,
        StatusCode::NO_CONTENT,
    );
    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/plain/serve", None).await,
        StatusCode::OK,
    );
    assert!(
        r.registry.get("plain/origin").await.is_none(),
        "no pointer row, no origin export — the pointer moves only by \
         explicit repoint"
    );
}

/// Publish a second generation, `g1`, on top of [`establish`]'s g0,
/// with a block written between them so that the two differ. The
/// pointer stays on g0.
async fn publish_g1(r: &Rig) {
    let head = r.provisioner.volumes.read().await["lin"].engine.clone();
    head.write()
        .await
        .write(0, &[0xB1; 4096])
        .expect("a write to the head");
    assert_eq!(
        call(
            &r.router,
            "POST",
            "/v1/volumes/lin/snapshots",
            Some(serde_json::json!({"name": "g1"})),
        )
        .await,
        StatusCode::CREATED
    );
}

/// Start `POST /origin {"ref": "snap/g1"}` and return once it has read
/// the row and is held just before its commit.
async fn a_repoint_held_before_its_commit(
    r: &Rig,
) -> tokio::task::JoinHandle<(StatusCode, serde_json::Value)> {
    r.flaky.hold_origin_cas.store(true, Ordering::SeqCst);
    let router = r.router.clone();
    let post = tokio::spawn(async move {
        call_body(
            &router,
            "POST",
            "/v1/volumes/lin/origin",
            Some(serde_json::json!({"ref": "snap/g1"})),
        )
        .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), r.flaky.held.notified())
        .await
        .expect("the POST reached its commit");
    post
}

async fn served(r: &Rig) -> serde_json::Value {
    let (status, origin) = call_body(&r.router, "GET", "/v1/volumes/lin/origin", None).await;
    assert_eq!(status, StatusCode::OK, "{origin}");
    origin["served"].clone()
}

/// A release that completes while a repoint is past its row read leaves
/// no `<vol>/origin` behind (M1.3aj). The POST read the volume's handle
/// and the row while the volume was served; the release then tears the
/// serving down; only then does the POST commit and converge. Before
/// M1.3aj its convergence registered the export again, bound to the
/// released serving, and nothing removed it until the volume was served
/// again on this server. The convergence now learns that the serving
/// ended, and the POST answers 409.
///
/// The row and its `origin` pin move together in the POST's commit and
/// stay (a release is not a delete, and the pointer pins what it names,
/// Invariant D): no `origin` pin is left on a generation the row does
/// not name, and the next serve converges the export on the row.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_while_a_repoint_is_past_its_row_read_leaves_no_origin_export() {
    let r = rig();
    establish(&r).await;
    publish_g1(&r).await;
    let post = a_repoint_held_before_its_commit(&r).await;

    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/release", None).await,
        StatusCode::NO_CONTENT,
        "the release completes while the POST is held"
    );
    assert!(
        r.registry.get("lin/origin").await.is_none(),
        "release tears the export down"
    );
    r.flaky.go.add_permits(1);
    let (status, answer) = tokio::time::timeout(std::time::Duration::from_secs(5), post)
        .await
        .expect("the POST answers")
        .expect("joined");
    assert!(
        r.registry.get("lin/origin").await.is_none(),
        "a repoint that outlived the volume's serving registered lin/origin again"
    );
    assert_eq!(status, StatusCode::CONFLICT, "{answer}");

    let vid = skynbd_api::provision::volume_id("lin");
    let origin = skynbd_nbd::origin::ORIGIN_PIN_HOLDER.to_string();
    assert_eq!(
        r.refs()
            .meta_get(vid, skynbd_nbd::origin::ORIGIN_META_KEY)
            .await
            .unwrap()
            .as_deref(),
        Some(&b"snap/g1"[..]),
        "the POST's commit moved the row"
    );
    assert!(
        r.refs()
            .pins(vid, "snap/g1")
            .await
            .unwrap()
            .contains(&origin),
        "the row's generation holds the origin pin"
    );
    assert!(
        !r.refs()
            .pins(vid, "snap/g0")
            .await
            .unwrap()
            .contains(&origin),
        "no origin pin is left on a generation the row does not name"
    );

    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/serve", None).await,
        StatusCode::OK
    );
    assert_eq!(
        served(&r).await,
        "snap/g1",
        "the next serve converges on the row"
    );
}

/// A repoint whose serving ended and was followed by another before it
/// converged converges the serving that is current (M1.3aj). The served
/// again volume's own serve read the row before the POST committed, so
/// the export would stay on g0, whose pin the commit moved, unless the
/// POST converges it. It does so through the current serving, not the
/// ended one whose handle it began with, and answers 200.
#[tokio::test(flavor = "multi_thread")]
async fn a_repoint_that_outlives_its_serving_converges_the_one_after_it() {
    let r = rig();
    establish(&r).await;
    publish_g1(&r).await;
    let post = a_repoint_held_before_its_commit(&r).await;

    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/release", None).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        call(&r.router, "POST", "/v1/volumes/lin/serve", None).await,
        StatusCode::OK
    );
    assert_eq!(
        served(&r).await,
        "snap/g0",
        "the new serving read the row before the commit"
    );
    r.flaky.go.add_permits(1);
    let (status, answer) = tokio::time::timeout(std::time::Duration::from_secs(5), post)
        .await
        .expect("the POST answers")
        .expect("joined");
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(answer["served"], "snap/g1", "{answer}");
    assert_eq!(
        served(&r).await,
        "snap/g1",
        "the current serving's export follows the row"
    );
}

/// A release that comes while a repoint of its serving waits for the
/// swap lock takes down what that repoint swaps (M1.3aj). The test holds
/// the origin export's swap lock; the POST commits, finds the volume
/// still served, and waits for the lock to converge. The release then
/// has to wait for the lock too: it ends the serving's term under it.
/// Whichever of the two gets the lock first, no export is left once
/// both are done. A teardown that only unregistered would finish while
/// the lock is held, and the POST's swap would then register the export
/// of a serving that has ended.
///
/// The POST gets 300 ms after its commit to reach the lock, and the
/// release 300 ms to finish while the lock is held. Were the POST
/// slower, it would find no serving and converge nothing, and a teardown
/// that does not end the term could pass; a right one cannot fail.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_takes_down_what_a_repoint_of_its_serving_swaps_after_it() {
    let r = rig();
    establish(&r).await;
    publish_g1(&r).await;
    let vid = skynbd_api::provision::volume_id("lin");

    let lock = r.registry.swap_lock("lin/origin").await;
    let router = r.router.clone();
    let post = tokio::spawn(async move {
        call_body(
            &router,
            "POST",
            "/v1/volumes/lin/origin",
            Some(serde_json::json!({"ref": "snap/g1"})),
        )
        .await
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while r
        .refs()
        .meta_get(vid, skynbd_nbd::origin::ORIGIN_META_KEY)
        .await
        .unwrap()
        .as_deref()
        != Some(&b"snap/g1"[..])
    {
        assert!(std::time::Instant::now() < deadline, "the POST commits");
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    let router = r.router.clone();
    let mut release =
        tokio::spawn(async move { call(&router, "POST", "/v1/volumes/lin/release", None).await });
    let early = tokio::time::timeout(std::time::Duration::from_millis(300), &mut release).await;
    let released_early = early.is_ok();
    drop(lock);
    let (status, answer) = tokio::time::timeout(std::time::Duration::from_secs(5), post)
        .await
        .expect("the POST answers")
        .expect("joined");
    let released = match early {
        Ok(done) => done.expect("joined"),
        Err(_) => tokio::time::timeout(std::time::Duration::from_secs(5), release)
            .await
            .expect("the release ends")
            .expect("joined"),
    };
    assert_eq!(released, StatusCode::NO_CONTENT);
    assert!(
        r.registry.get("lin/origin").await.is_none(),
        "a repoint's swap outlived the release of its serving"
    );
    // Which of the two took the lock first decides the answer: 200 when
    // the POST swapped and answered before the release ended the term.
    assert!(
        status == StatusCode::OK || status == StatusCode::CONFLICT,
        "{status}: {answer}"
    );
    assert!(
        !released_early,
        "the release finished while a repoint of its serving waited to swap"
    );
}

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
        self.inner.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<TxnOutcome> {
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
}

fn rig() -> Rig {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let flaky = Arc::new(FlakyRefs {
        inner: Arc::new(ObjectRefStore::new(Arc::clone(&store))),
        fail_next_list: AtomicBool::new(false),
        fired: AtomicBool::new(false),
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
        provisioner: Some(provisioner),
        snap_requests: Default::default(),
    });
    Rig {
        router: skynbd_api::router(state),
        registry,
        flaky,
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

//! The restage loop the k3s smoke walks, in-process: stage → cycles →
//! unstage (final cycle + RELEASE) → restage (re-serve) → final cycle
//! again. The server side is a real `Provisioner` behind the real
//! router over real HTTP — the release path's commit-before-release
//! and the re-serve's engine reopen are exactly production's, which is
//! the part `http_s15`'s static-volume rig cannot exercise.
//!
//! Regression: the smoke's second unstage wedged forever on
//! "head CAS conflict (I2)" after a release → re-serve round trip.

use std::sync::Arc;

use skynbd_agent::{
    ControlPlane, CycleEngine, FakeStack, HttpControlPlane, LocalStack, StateStore,
};
use skynbd_api::{ApiAuth, ApiState, Provisioner};
use skynbd_nbd::ExportRegistry;
use skynbd_refs::RefStore;
use skynbd_store::{MemoryStore, ObjectStore};

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
}

/// A provisioner-backed server (the smoke's dev-server shape: memory
/// refs, no lease config) on a loopback listener.
async fn start_server() -> (String, tokio::task::JoinHandle<()>) {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(skynbd_refs::MemoryRefStore::new());
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = skynbd_api::router(state);
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), task)
}

async fn http(base: &str, method: &str, path: &str, body: Option<serde_json::Value>) -> u16 {
    let client = reqwest::Client::new();
    let url = format!("{base}{path}");
    let req = match method {
        "POST" => client.post(&url),
        "DELETE" => client.delete(&url),
        _ => unreachable!(),
    };
    let req = match body {
        Some(b) => req.json(&b),
        None => req,
    };
    req.send().await.unwrap().status().as_u16()
}

/// unstage → restage → unstage: the second final cycle must publish,
/// not wedge. Mirrors the k3s smoke's thin volume lifecycle (two
/// consumers of one PVC in sequence, then teardown).
#[tokio::test(flavor = "multi_thread")]
async fn a_released_and_reserved_lineage_accepts_the_next_final_cycle() {
    let (base, _server) = start_server().await;
    // The lineage volume, dynamically provisioned like CSI's.
    assert_eq!(
        http(
            &base,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({"name": "vlin", "size": VOL})),
        )
        .await,
        201
    );

    let stack = FakeStack::new(VOL, CHUNK);
    let cp = HttpControlPlane::new(base.clone(), "vlin", None);
    let dir = tempfile::tempdir().unwrap();

    // ---- stage #1: bootstrap + a publish + the D-M13 final cycle ----
    let mut eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap();
    stack.write_origin(0, &pattern(3 * CHUNK as usize, 1));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g0");
    // CSI snapshot: the requested name is an ALIAS of the generation
    // (6h-2), and a clone volume is minted from it — the smoke's
    // thin-snapshot round trip.
    cp.alias_snapshot("g0", "csisnap").await.unwrap();
    assert_eq!(
        http(
            &base,
            "POST",
            "/v1/volumes",
            Some(serde_json::json!({"name": "cl", "size": VOL, "from": "vlin/csisnap"})),
        )
        .await,
        201
    );
    assert_eq!(
        http(&base, "POST", "/v1/volumes/cl/release", None).await,
        204
    );
    assert_eq!(http(&base, "DELETE", "/v1/volumes/cl", None).await, 204);

    stack.write_origin(5 * CHUNK, &pattern(CHUNK as usize, 9));
    // The unstage's final cycle, aliased like agentcycle::finish does.
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g1");
    cp.alias_snapshot("g1", "skynbd-unstage").await.unwrap();
    drop(eng);

    // ---- unstage #1: the D-M6 release --------------------------------
    assert_eq!(
        http(&base, "POST", "/v1/volumes/vlin/release", None).await,
        204
    );

    // ---- restage: re-serve + a fresh engine over the SAME state ------
    // The holder rides along exactly as the CSI stage sends it
    // (serve_required with the node's identity). On a lease-less
    // server this is what wedged: the re-serve built an ACTING
    // (fenced) refs handle with no lease row to fence against, and
    // every transaction on the restaged volume answered
    // `LeaseLost { holder: "" }` — the smoke's `(I2)` conflict. The
    // first serve dodged it only because volume creation had already
    // put an unwrapped handle in the serving map.
    assert_eq!(
        http(
            &base,
            "POST",
            "/v1/volumes/vlin/serve",
            Some(serde_json::json!({"holder": "node-x"})),
        )
        .await,
        200
    );
    let mut eng = CycleEngine::open(
        stack.clone(),
        cp.clone(),
        StateStore::new(dir.path()),
        "vlin",
    )
    .unwrap();

    // The external-snapshotter's DeleteSnapshot retry, landing AFTER
    // the restage (its first attempt was refused while the clone
    // pinned the snapshot; the backoff put the retry here in the
    // smoke's timeline).
    assert_eq!(
        http(&base, "DELETE", "/v1/volumes/vlin/snapshots/csisnap", None).await,
        204
    );

    // The restage also LOSES the local snapshot LVs: unstage's teardown
    // deactivated every agent device, and the rebuilt stack activates
    // none of them — while the persisted agent state still NAMES the
    // baseline LV. (The smoke's second failure mode: the incremental
    // delta resolved the recorded LV and got "snapshot not found".)
    let baseline_lv = eng
        .state()
        .unwrap()
        .baseline
        .as_ref()
        .unwrap()
        .local
        .lv_name
        .clone();
    assert!(!baseline_lv.is_empty());
    stack.snap_delete(&baseline_lv).unwrap();

    // ---- unstage #2: the final cycle that wedged in the smoke --------
    stack.write_origin(7 * CHUNK, &pattern(CHUNK as usize, 33));
    let gen = eng
        .run_cycle()
        .await
        .expect("the post-restage final cycle must publish, not wedge");
    assert_eq!(gen, "snap/g2");
    // The degraded (full-mapped) delta must still be BIT-EXACT.
    assert_eq!(
        cp.read_snapshot("snap/g2", 0, VOL).await.unwrap(),
        stack.origin_image()
    );
    cp.alias_snapshot("g2", "skynbd-unstage-2").await.unwrap();

    // The promotion re-established a live local pair: the next cycle is
    // an ordinary incremental again.
    stack.write_origin(2 * CHUNK, &pattern(CHUNK as usize, 55));
    assert_eq!(eng.run_cycle().await.unwrap(), "snap/g3");
    assert_eq!(
        cp.read_snapshot("snap/g3", 0, VOL).await.unwrap(),
        stack.origin_image()
    );
    assert_eq!(
        http(&base, "POST", "/v1/volumes/vlin/release", None).await,
        204
    );
}

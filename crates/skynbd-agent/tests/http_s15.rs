//! S15: the V3.2 §21.1 crash matrix through the REAL REST API over REAL
//! HTTP (agent design doc §13). Local side is `FakeStack`; the server is
//! `skynbd-api`'s router served by axum on a loopback socket — the same
//! code path production traffic takes, bearer auth included.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use skynbd_agent::{
    AgentError, ControlPlane, CycleEngine, FakeStack, HttpControlPlane, LocalStack, Phase,
    StateStore,
};
use skynbd_api::{ApiState, VolumeHandle};
use skynbd_engine::{Engine, EngineConfig};
use skynbd_nbd::{DynEngine, ExportRegistry};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;
use tokio::sync::{Mutex, RwLock};

const CHUNK: u64 = 4096;
const VOL: u64 = 16 * CHUNK;
const TOKEN: &str = "s15-test-token";

struct Rig {
    stack: FakeStack,
    cp: HttpControlPlane,
    base: String,
    dir: tempfile::TempDir,
    applies: Arc<AtomicUsize>,
    /// The server's behavior, switchable at runtime ([`CURRENT`],
    /// [`LEGACY`], [`OLD_REPLICA`]).
    mode: Arc<std::sync::atomic::AtomicU8>,
    overlap: Arc<OverlapProbe>,
    _server: tokio::task::JoinHandle<()>,
}

/// What the server saw of the agent's overlap: `overlapped` is set when an
/// `/ensure` or object upload arrives while an attach-family request
/// (`/attach`, `/clear`, `/apply`) is in flight. `slow_attach` holds each
/// attach-family request 20 ms before serving it, so a staging request
/// sent beside it cannot race past unseen.
#[derive(Default)]
struct OverlapProbe {
    attaching: AtomicUsize,
    overlapped: std::sync::atomic::AtomicBool,
    slow_attach: std::sync::atomic::AtomicBool,
}

/// The current server: every response advertises `prepared-apply`.
const CURRENT: u8 = 0;
/// A server that predates `…/apply`: no features header anywhere, and
/// `…/apply` answered with the rig's bare legacy status.
const LEGACY: u8 = 1;
/// Behind a load balancer mid-upgrade (or mid-rollback): every response
/// advertises the feature except `…/apply`, which lands on an old replica
/// and gets its bare 404 — the agent has already seen the header.
const OLD_REPLICA: u8 = 2;
/// A current server behind a proxy that strips the features header from
/// every response; `…/apply` itself works.
const STRIPPED: u8 = 3;

/// `legacy`: start in [`LEGACY`] mode, answering `…/apply` with this bare
/// status (the router's 404, or the fail-closed 403 an OIDC-restricted
/// identity gets for an unknown route); `None` starts [`CURRENT`]. The
/// returned counter counts `…/apply` requests that reached the server;
/// the mode can be switched at runtime.
async fn start_server_with(
    legacy: Option<axum::http::StatusCode>,
) -> (
    String,
    tokio::task::JoinHandle<()>,
    Arc<AtomicUsize>,
    Arc<std::sync::atomic::AtomicU8>,
    Arc<OverlapProbe>,
) {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let vol_id = VolumeId(ulid::Ulid::new());
    let engine: DynEngine = Engine::create_volume(
        Arc::clone(&store),
        Arc::clone(&refs),
        vol_id,
        VOL,
        EngineConfig::default(),
    )
    .await
    .unwrap();
    let state = Arc::new(ApiState {
        auth: skynbd_api::ApiAuth::Static(TOKEN.into()),
        enroll: None,
        provisioner: None,
        snap_requests: Default::default(),
        volumes: skynbd_api::static_volumes(HashMap::from([(
            "vlin".to_string(),
            VolumeHandle {
                vol_id,
                size: VOL,
                engine: Arc::new(RwLock::new(engine)),
                store,
                refs,
                cfg: EngineConfig::default(),
                lease_holder: None,
                prepared: Arc::new(Mutex::new(Default::default())),
            },
        )])),
        registry: Arc::new(ExportRegistry::new()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let applies = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&applies);
    let mode = Arc::new(std::sync::atomic::AtomicU8::new(if legacy.is_some() {
        LEGACY
    } else {
        CURRENT
    }));
    let m = Arc::clone(&mode);
    let code = legacy.unwrap_or(axum::http::StatusCode::NOT_FOUND);
    let router = skynbd_api::router(state).layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let (hits, m) = (Arc::clone(&hits), Arc::clone(&m));
            async move {
                use axum::response::IntoResponse;
                let apply = req.uri().path().ends_with("/apply");
                if apply {
                    hits.fetch_add(1, Ordering::SeqCst);
                }
                match m.load(Ordering::SeqCst) {
                    CURRENT => next.run(req).await,
                    LEGACY if apply => code.into_response(),
                    LEGACY => {
                        let mut resp = next.run(req).await;
                        resp.headers_mut().remove(skynbd_api::FEATURES_HEADER);
                        resp
                    }
                    STRIPPED => {
                        let mut resp = next.run(req).await;
                        resp.headers_mut().remove(skynbd_api::FEATURES_HEADER);
                        resp
                    }
                    _ if apply => axum::http::StatusCode::NOT_FOUND.into_response(),
                    _ => next.run(req).await,
                }
            }
        },
    ));
    let overlap = Arc::new(OverlapProbe::default());
    let o = Arc::clone(&overlap);
    let router = router.layer(axum::middleware::from_fn(
        move |req: axum::extract::Request, next: axum::middleware::Next| {
            let o = Arc::clone(&o);
            async move {
                let path = req.uri().path().to_string();
                let attaching = ["/attach", "/clear", "/apply"]
                    .iter()
                    .any(|t| path.ends_with(t));
                let staging = path.ends_with("/ensure") || path.contains("/objects/");
                if staging && o.attaching.load(Ordering::SeqCst) > 0 {
                    o.overlapped.store(true, Ordering::SeqCst);
                }
                if !attaching {
                    return next.run(req).await;
                }
                o.attaching.fetch_add(1, Ordering::SeqCst);
                if o.slow_attach.load(Ordering::SeqCst) {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                }
                let resp = next.run(req).await;
                o.attaching.fetch_sub(1, Ordering::SeqCst);
                resp
            }
        },
    ));
    let task = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (format!("http://{addr}"), task, applies, mode, overlap)
}

impl Rig {
    async fn new() -> Self {
        Self::new_with(None).await
    }

    async fn new_with(legacy: Option<axum::http::StatusCode>) -> Self {
        let (base, server, applies, mode, overlap) = start_server_with(legacy).await;
        let stack = FakeStack::new(VOL, CHUNK);
        let cp = HttpControlPlane::new(base.clone(), "vlin", Some(TOKEN.into()));
        Rig {
            stack,
            cp,
            base,
            dir: tempfile::tempdir().unwrap(),
            applies,
            mode,
            overlap,
            _server: server,
        }
    }

    fn engine(&self) -> CycleEngine<FakeStack, HttpControlPlane> {
        CycleEngine::open(
            self.stack.clone(),
            self.cp.clone(),
            StateStore::new(self.dir.path()),
            "vlin",
        )
        .unwrap()
    }

    fn mutate(&self, phase: u8) {
        let p = |len: usize, seed: u8| -> Vec<u8> {
            (0..len).map(|i| seed.wrapping_add(i as u8)).collect()
        };
        match phase {
            0 => {
                self.stack.write_origin(0, &p(3 * CHUNK as usize, 1));
                self.stack.write_origin(9 * CHUNK, &p(CHUNK as usize, 7));
            }
            1 => {
                self.stack
                    .write_origin(2 * CHUNK, &p(2 * CHUNK as usize, 21));
                self.stack.discard_origin(9 * CHUNK, CHUNK);
            }
            _ => {
                self.stack
                    .write_origin(5 * CHUNK, &p(3 * CHUNK as usize, 91));
            }
        }
    }

    async fn assert_converged(&self, eng: &CycleEngine<FakeStack, HttpControlPlane>) {
        let s = eng.state().expect("state exists");
        assert_eq!(s.phase, Phase::Idle);
        assert!(s.candidate.is_none() && s.prepared.is_none());
        if let Some(b) = s.baseline.as_ref() {
            assert!(self.cp.ref_exists(&b.remote).await.unwrap());
            assert!(self
                .cp
                .pinned_by(&b.remote, "agent-baseline:vlin")
                .await
                .unwrap());
        }
        assert!(self.cp.find_prepared().await.unwrap().is_none());
        assert!(self.stack.observe().candidates.is_empty());
        assert!(self.stack.snap_count() <= 1);
    }
}

/// Bearer auth is actually enforced on the wire.
#[tokio::test(flavor = "multi_thread")]
async fn wrong_token_is_rejected() {
    let rig = Rig::new().await;
    let bad = HttpControlPlane::new(rig.base.clone(), "vlin", Some("wrong".into()));
    let e = bad.find_prepared().await.unwrap_err();
    assert!(format!("{e}").contains("401"), "got: {e}");
    let none = HttpControlPlane::new(rig.base.clone(), "vlin", None);
    assert!(none.find_prepared().await.is_err());
}

/// Token files are re-read per request (auth design §4): a rotated
/// (kubelet-projected) token is picked up live, without rebuilding the
/// control plane.
#[tokio::test(flavor = "multi_thread")]
async fn token_file_rotation_is_picked_up() {
    let rig = Rig::new().await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    std::fs::write(&path, "stale\n").unwrap();
    let cp = HttpControlPlane::with_token_file(rig.base.clone(), "vlin", &path);
    let e = cp.find_prepared().await.unwrap_err();
    assert!(format!("{e}").contains("401"), "got: {e}");
    // Rotate in place — same control plane instance must succeed now.
    std::fs::write(&path, format!("{TOKEN}\n")).unwrap();
    assert!(cp.find_prepared().await.unwrap().is_none());
}

/// D-M12 round trip over real HTTP: a pending snapshot request picks up
/// the engine's progress reports during a cycle, and the failure-report
/// helper stamps attempts/last_error without touching the phase.
#[tokio::test(flavor = "multi_thread")]
async fn cycle_status_round_trip_over_http() {
    let rig = Rig::new().await;
    let http = reqwest::Client::new();

    // File a request the way the CSI controller does.
    let st = http
        .post(format!("{}/v1/volumes/vlin/snapshot-requests", rig.base))
        .bearer_auth(TOKEN)
        .json(&serde_json::json!({ "name": "csi-1" }))
        .send()
        .await
        .unwrap()
        .status();
    assert_eq!(st.as_u16(), 202);
    let status = || async {
        http.get(format!(
            "{}/v1/volumes/vlin/snapshot-requests/csi-1",
            rig.base
        ))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json::<serde_json::Value>()
        .await
        .unwrap()
    };
    assert_eq!(status().await["phase"], "pending");

    // A full cycle reports through the same authenticated client the
    // ops travel on; the request ends carrying the last phase seam.
    rig.mutate(0);
    let mut eng = rig.engine();
    eng.set_capture_requests(vec!["csi-1".into()]);
    eng.run_cycle().await.unwrap();
    let st = status().await;
    assert_eq!(st["phase"], "promoting", "{st}");
    assert_eq!(st["last_error"], serde_json::Value::Null);
    let capture_time_ms = st["capture_time_ms"].as_u64().expect("cut time reported");
    let served = eng.served_requests();
    assert_eq!(served.len(), 1);
    assert_eq!(served[0].name, "csi-1");
    assert_eq!(served[0].target, "snap/g0");
    assert_eq!(served[0].capture_time_ms, capture_time_ms);

    // Failure report (what the CSI node's cycle loop sends on errors).
    rig.cp
        .report_cycle_failure(2, "freeze: EBUSY")
        .await
        .unwrap();
    let st = status().await;
    assert_eq!(st["attempts"], 2);
    assert_eq!(st["last_error"], "freeze: EBUSY");
    assert_eq!(st["phase"], "promoting", "failure report keeps the phase");
    // A binding-only report leaves the cycle status alone, re-announcing
    // the same cut is a no-op, and a lost cut fails the request and takes
    // it off the agent's queue.
    eng.report_captures(&["csi-1".to_string()]).await.unwrap();
    let st = status().await;
    assert_eq!(st["capture_time_ms"], capture_time_ms);
    assert_eq!(st["phase"], "promoting");
    assert_eq!(st["capture_lost"], false);
    rig.cp
        .report_captures(&[], &["csi-1".to_string()])
        .await
        .unwrap();
    let st = status().await;
    assert_eq!(st["capture_lost"], true, "{st}");
    assert!(rig.cp.snapshot_requests().await.unwrap().is_empty());
}

/// D-M15 repoint round trip over real HTTP: init adopts the newest
/// published generation, a forward repoint moves the pointer, a
/// regression no-ops reporting the actual pointer (Invariant E, 15g),
/// and a bad target surfaces as NotFound — the client surface the §3
/// advance sequence stands on.
#[tokio::test(flavor = "multi_thread")]
async fn origin_repoint_round_trip_over_http() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    rig.mutate(0);
    let g0 = eng.run_cycle().await.unwrap();
    rig.mutate(1);
    let g1 = eng.run_cycle().await.unwrap();

    assert_eq!(rig.cp.repoint_origin(None).await.unwrap(), g1);
    // Invariant E: naming an older generation no-ops — the pointer
    // never moves backwards, and the reply reports what it still names.
    assert_eq!(rig.cp.repoint_origin(Some(&g0)).await.unwrap(), g1);
    // Initializing again is a no-op that reports the current pointer.
    assert_eq!(rig.cp.repoint_origin(None).await.unwrap(), g1);
    // A forward repoint moves.
    rig.mutate(2);
    let g2 = eng.run_cycle().await.unwrap();
    assert_eq!(rig.cp.repoint_origin(Some(&g2)).await.unwrap(), g2);
    let e = rig.cp.repoint_origin(Some("snap/gone")).await.unwrap_err();
    assert!(matches!(e, skynbd_agent::CpError::NotFound(_)), "{e}");
}

/// Happy path over HTTP: bootstrap + 2 cycles, bit-exact via the data
/// endpoint.
#[tokio::test(flavor = "multi_thread")]
async fn http_bootstrap_and_cycles_bit_exact() {
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    for phase in 0u8..3 {
        rig.mutate(phase);
        let img = rig.stack.origin_image();
        let t = eng.run_cycle().await.unwrap();
        assert_eq!(rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img);
    }
    rig.assert_converged(&eng).await;
}

/// Issue #97 follow-up over the real REST surface: with the backfill
/// pipelined, phase 1's write + discard is one mixed batch, sent as one
/// `apply`; against a server that does not advertise it (no features
/// header; `/apply` answers a bare 404, or the 403 an OIDC-restricted
/// identity gets there) the agent never sends `/apply` and attaches per
/// run. Bit-exact every way. A missing prepared session is still
/// `NotFound`, not "unsupported".
#[tokio::test(flavor = "multi_thread")]
async fn mixed_batches_over_http_with_and_without_apply() {
    use axum::http::StatusCode;
    for legacy in [
        None,
        Some(StatusCode::NOT_FOUND),
        Some(StatusCode::FORBIDDEN),
    ] {
        let rig = Rig::new_with(legacy).await;
        let mut eng = rig.engine().with_publish_pipeline_depth(8);
        for phase in 0u8..3 {
            rig.mutate(phase);
            let img = rig.stack.origin_image();
            let t = eng.run_cycle().await.unwrap();
            assert_eq!(
                rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(),
                img,
                "legacy={legacy:?} phase {phase}"
            );
        }
        rig.assert_converged(&eng).await;
        // Against the current server the mixed batches went through
        // `/apply`; against a legacy one (no features header) the agent never
        // asks — the split is chosen from the header, not from a 404/403.
        let hits = rig.applies.load(Ordering::SeqCst);
        if legacy.is_none() {
            assert!(hits > 0, "mixed batches never used /apply");
        } else {
            assert_eq!(hits, 0, "legacy={legacy:?}: /apply was asked");
        }

        // A fresh plane that has seen no response yet: against a legacy
        // server the bare status maps to Unsupported (the header is absent
        // on it too); against the current one a missing session is a plain
        // NotFound, not a reason to stop using `/apply`.
        let cp = HttpControlPlane::new(rig.base.clone(), "vlin", Some(TOKEN.into()));
        let op = skynbd_agent::RangeOp::Clear { off: 0, len: CHUNK };
        let missing = ulid::Ulid::new().to_string();
        match cp.apply(&missing, 1, &[op]).await {
            Err(skynbd_agent::CpError::Unsupported(_)) if legacy.is_some() => {}
            Err(skynbd_agent::CpError::NotFound(_)) if legacy.is_none() => {}
            other => panic!("legacy={legacy:?}: {other:?}"),
        }
        // And the plane that has seen the legacy server answers locally.
        if legacy.is_some() {
            let before = rig.applies.load(Ordering::SeqCst);
            let op = skynbd_agent::RangeOp::Clear { off: 0, len: CHUNK };
            assert!(matches!(
                rig.cp.apply(&missing, 1, &[op]).await,
                Err(skynbd_agent::CpError::Unsupported(_))
            ));
            assert_eq!(rig.applies.load(Ordering::SeqCst), before);
        }
    }
}

/// S15 proper: crash the agent at every boundary point of a scripted
/// 3-generation history; recover over HTTP; finish 2 more cycles; verify
/// every recorded generation bit-exact through the API.
#[tokio::test(flavor = "multi_thread")]
async fn s15_crash_matrix_over_http() {
    // Size the matrix.
    let rig = Rig::new().await;
    let mut eng = rig.engine();
    for phase in 0u8..3 {
        rig.mutate(phase);
        eng.run_cycle().await.unwrap();
    }
    let total = eng.crash.counter;
    assert!(total > 30, "dense matrix expected, got {total}");
    drop(rig);

    for k in 0..total {
        let rig = Rig::new().await;
        let mut expected: Vec<(String, Vec<u8>)> = Vec::new();
        let mut eng = rig.engine();
        eng.crash.at = Some(k);
        let mut crashed = false;
        for phase in 0u8..3 {
            rig.mutate(phase);
            let img = rig.stack.origin_image();
            match eng.run_cycle().await {
                Ok(t) => expected.push((t, img)),
                Err(AgentError::InjectedCrash(_)) => {
                    crashed = true;
                    break;
                }
                Err(e) => panic!("point {k}: unexpected {e}"),
            }
        }
        assert!(crashed, "point {k} never fired");

        let mut eng2 = rig.engine();
        eng2.recover()
            .await
            .unwrap_or_else(|e| panic!("point {k}: recovery failed: {e}"));
        rig.assert_converged(&eng2).await;
        for phase in 0u8..2 {
            rig.mutate(phase + 10);
            let img = rig.stack.origin_image();
            let t = eng2
                .run_cycle()
                .await
                .unwrap_or_else(|e| panic!("point {k}: post-recovery cycle: {e}"));
            expected.push((t, img));
        }
        for (t, img) in &expected {
            let got = rig.cp.read_snapshot(t, 0, VOL).await.unwrap();
            assert_eq!(&got, img, "point {k}: generation {t} corrupted");
        }
    }
}

/// Overlapped backfill against the real server (issue #97 follow-up): at
/// depth > 1 with several batches per cycle, batch k+1's `ensure` and
/// uploads run beside batch k's attach on ONE prepared session over
/// HTTP. A full write, then interleaved writes and discards (mixed
/// batches, also split when the server lacks `/apply`), then a sparse
/// rewrite — every generation bit-exact. The server sees the overlap
/// happen (an `/ensure` or object upload arriving while an attach is in
/// flight), and sees none with overlap switched off.
#[tokio::test(flavor = "multi_thread")]
async fn overlapped_multi_batch_cycles_over_http() {
    for (depth, legacy, overlap) in [
        (2usize, None, true),
        (3, None, true),
        (4, None, true),
        (3, Some(axum::http::StatusCode::NOT_FOUND), true),
        (4, None, false),
    ] {
        let rig = Rig::new_with(legacy).await;
        rig.overlap.slow_attach.store(true, Ordering::SeqCst);
        let mut eng = rig
            .engine()
            .with_publish_pipeline_depth(depth)
            .with_publish_overlap(overlap);
        for cycle in 0u8..3 {
            for i in 0..VOL / CHUNK {
                let mut b = vec![cycle.wrapping_mul(31).wrapping_add(i as u8); CHUNK as usize];
                b[0] = cycle;
                match cycle {
                    0 => rig.stack.write_origin(i * CHUNK, &b),
                    1 if i % 2 == 0 => rig.stack.write_origin(i * CHUNK, &b),
                    1 => rig.stack.discard_origin(i * CHUNK, CHUNK),
                    _ if i % 5 == 1 => rig.stack.write_origin(i * CHUNK, &b),
                    _ => {}
                }
            }
            let img = rig.stack.origin_image();
            let t = eng.run_cycle().await.unwrap();
            assert_eq!(
                rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(),
                img,
                "depth {depth} legacy={legacy:?} cycle {cycle}"
            );
        }
        rig.assert_converged(&eng).await;
        let hits = rig.applies.load(Ordering::SeqCst);
        assert_eq!(
            hits > 0,
            legacy.is_none(),
            "depth {depth}: /apply hits {hits}"
        );
        assert_eq!(
            rig.overlap.overlapped.load(Ordering::SeqCst),
            overlap,
            "depth {depth} overlap={overlap}: staging seen during an attach"
        );
    }
}

/// Feature flips inside a running agent (no restart). Mid-upgrade or
/// mid-rollback, `/apply` reaches an old replica — a bare 404 without the
/// header — though every other response advertises the feature: the
/// cycle splits (bit-exact), asking `/apply` once that cycle, not once
/// per mixed batch. The fleet then finishes upgrading: the next cycle
/// uses `/apply` again. A full rollback (no header anywhere) splits
/// without asking; upgrading again resumes `/apply`.
#[tokio::test(flavor = "multi_thread")]
async fn apply_follows_the_server_through_upgrades_and_rollbacks() {
    let rig = Rig::new().await;
    let mut eng = rig.engine().with_publish_pipeline_depth(2);
    let mut cycle = 0u8;
    // Interleaved writes and discards across all 16 chunks: 8 mixed
    // batches at depth 2.
    let mut mixed_cycle = |rig: &Rig| {
        cycle += 1;
        for i in 0..VOL / CHUNK {
            if (i + cycle as u64).is_multiple_of(2) {
                rig.stack
                    .write_origin(i * CHUNK, &vec![cycle ^ i as u8; CHUNK as usize]);
            } else {
                rig.stack.discard_origin(i * CHUNK, CHUNK);
            }
        }
        rig.stack.origin_image()
    };
    let run = async |eng: &mut CycleEngine<FakeStack, HttpControlPlane>, img: Vec<u8>| {
        let before = rig.applies.load(Ordering::SeqCst);
        let t = eng.run_cycle().await.unwrap();
        assert_eq!(rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img);
        rig.applies.load(Ordering::SeqCst) - before
    };

    // Map every chunk first, so the discards below are real deltas.
    for i in 0..VOL / CHUNK {
        rig.stack
            .write_origin(i * CHUNK, &vec![0x5A ^ i as u8; CHUNK as usize]);
    }
    let img = rig.stack.origin_image();
    run(&mut eng, img).await;

    let img = mixed_cycle(&rig);
    assert!(
        run(&mut eng, img).await > 1,
        "current: every mixed batch applies"
    );

    rig.mode.store(OLD_REPLICA, Ordering::SeqCst);
    let img = mixed_cycle(&rig);
    assert_eq!(
        run(&mut eng, img).await,
        1,
        "old replica: asked once, then split"
    );

    rig.mode.store(CURRENT, Ordering::SeqCst);
    let img = mixed_cycle(&rig);
    assert!(run(&mut eng, img).await > 1, "upgraded: /apply resumes");

    rig.mode.store(LEGACY, Ordering::SeqCst);
    let img = mixed_cycle(&rig);
    assert_eq!(run(&mut eng, img).await, 0, "rolled back: never asked");

    rig.mode.store(CURRENT, Ordering::SeqCst);
    let img = mixed_cycle(&rig);
    assert!(
        run(&mut eng, img).await > 1,
        "upgraded again: /apply resumes"
    );
    rig.assert_converged(&eng).await;
}

/// `always` against a server without `/apply` (an old replica, or a
/// rollback under an operator who set `always` for a header-stripping
/// proxy): the refusal still splits the rest of the cycle, so every cycle
/// publishes, bit-exact, with at most one refused call per cycle — not a
/// failed cycle per mixed batch.
#[tokio::test(flavor = "multi_thread")]
async fn always_still_splits_around_a_server_without_apply() {
    for code in [
        axum::http::StatusCode::NOT_FOUND,
        axum::http::StatusCode::FORBIDDEN,
    ] {
        let mut rig = Rig::new_with(Some(code)).await;
        rig.cp = rig
            .cp
            .clone()
            .with_mixed_apply(skynbd_agent::MixedApply::Always);
        let mut eng = rig.engine().with_publish_pipeline_depth(4);
        let cycles = 3;
        for phase in 0u8..cycles {
            rig.mutate(phase);
            let img = rig.stack.origin_image();
            let t = eng
                .run_cycle()
                .await
                .unwrap_or_else(|e| panic!("{code}: cycle {phase} failed: {e}"));
            assert_eq!(rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img);
        }
        rig.assert_converged(&eng).await;
        let hits = rig.applies.load(Ordering::SeqCst);
        assert!(
            (1..=cycles as usize).contains(&hits),
            "{code}: {hits} refused /apply calls over {cycles} cycles"
        );
    }
}

/// The `mixedApply` knob: behind a proxy that strips the features header,
/// `auto` never sends `/apply` (the header is all it goes by), `always`
/// uses it anyway — and the server takes it — and `never` splits even
/// against a server that advertises the call. Bit-exact every way.
#[tokio::test(flavor = "multi_thread")]
async fn mixed_apply_mode_overrides_the_header() {
    use skynbd_agent::MixedApply;
    for (mode, server, want_apply) in [
        (MixedApply::Auto, STRIPPED, false),
        (MixedApply::Always, STRIPPED, true),
        (MixedApply::Never, CURRENT, false),
    ] {
        let mut rig = Rig::new().await;
        rig.mode.store(server, Ordering::SeqCst);
        rig.cp = rig.cp.clone().with_mixed_apply(mode);
        let mut eng = rig.engine().with_publish_pipeline_depth(4);
        for phase in 0u8..3 {
            rig.mutate(phase);
            let img = rig.stack.origin_image();
            let t = eng.run_cycle().await.unwrap();
            assert_eq!(rig.cp.read_snapshot(&t, 0, VOL).await.unwrap(), img);
        }
        rig.assert_converged(&eng).await;
        let hits = rig.applies.load(Ordering::SeqCst);
        assert_eq!(hits > 0, want_apply, "{mode:?}: /apply hits {hits}");
    }
}

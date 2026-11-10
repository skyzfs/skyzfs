//! OIDC bearer auth integration tests (auth design §4): a real JWKS
//! served over HTTP, locally-minted RS256 tokens, and the full
//! accept/reject + binding-enforcement matrix through the router.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde_json::json;
use skynbd_api::{ApiAuth, ApiState, Binding, OidcAuth, TrustedIssuer, VolumeHandle};
use skynbd_engine::{Engine, EngineConfig};
use skynbd_nbd::{DynEngine, ExportRegistry};
use skynbd_refs::{MemoryRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;
use tokio::sync::{Mutex, RwLock};
use tower::ServiceExt;

mod common;
use common::{mint, now, spawn_issuer, AUD};

/// Router with volumes db1 + cache1, OIDC auth trusting the local issuer:
/// - sub "system:serviceaccount:prod:agent" → ingest+snapshot+read on db*
/// - role "skynbd-operator" (Zitadel-shaped claim) → all ops on *
async fn fixture(issuer: &str, explicit_jwks: bool) -> axum::Router {
    let store: Arc<dyn ObjectStore> = Arc::new(MemoryStore::new());
    let refs: Arc<dyn RefStore> = Arc::new(MemoryRefStore::new());
    let mut volumes = HashMap::new();
    for name in ["db1", "cache1"] {
        let vol_id = VolumeId(ulid::Ulid::new());
        let engine: DynEngine = Engine::create_volume(
            Arc::clone(&store),
            Arc::clone(&refs),
            vol_id,
            128 * 1024,
            EngineConfig::default(),
        )
        .await
        .unwrap();
        volumes.insert(
            name.to_string(),
            VolumeHandle {
                vol_id,
                size: 128 * 1024,
                engine: Arc::new(RwLock::new(engine)),
                store: Arc::clone(&store),
                refs: Arc::clone(&refs),
                cfg: EngineConfig::default(),
                lease_holder: None,
                prepared: Arc::new(Mutex::new(Default::default())),
            },
        );
    }
    let auth = ApiAuth::Oidc(OidcAuth::new(vec![TrustedIssuer {
        issuer: issuer.to_string(),
        audience: AUD.to_string(),
        jwks_url: explicit_jwks.then(|| format!("{issuer}/jwks.json")),
        roles_claim: Some("urn:zitadel:iam:org:project:roles".to_string()),
        tls_ca_pem: None,
        bindings: vec![
            Binding {
                sub: Some("system:serviceaccount:prod:agent".to_string()),
                role: None,
                volumes: vec!["db*".to_string()],
                ops: vec!["ingest".into(), "snapshot".into(), "read".into()],
            },
            Binding {
                sub: None,
                role: Some("skynbd-operator".to_string()),
                volumes: vec!["*".to_string()],
                ops: vec!["*".into()],
            },
        ],
    }]));
    let state = Arc::new(ApiState {
        volumes: skynbd_api::static_volumes(volumes),
        registry: Arc::new(ExportRegistry::new()),
        auth,
        enroll: None,
        provisioner: None,
        snap_requests: Default::default(),
    });
    skynbd_api::router(state)
}

async fn status(router: &axum::Router, method: &str, uri: &str, token: Option<&str>) -> StatusCode {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let body = if method == "POST" {
        Body::from(json!({"name": "s1"}).to_string())
    } else {
        Body::empty()
    };
    router
        .clone()
        .oneshot(req.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

const AGENT_SUB: &str = "system:serviceaccount:prod:agent";

#[tokio::test(flavor = "multi_thread")]
async fn accepts_valid_token_within_bindings() {
    let issuer = spawn_issuer().await;
    let router = fixture(&issuer, true).await;
    let tok = mint(&issuer, "test-key-1", json!({"sub": AGENT_SUB}));
    // read + snapshot on a db* volume: granted.
    assert_eq!(
        status(&router, "GET", "/v1/volumes/db1/snapshots", Some(&tok)).await,
        StatusCode::OK
    );
    assert_eq!(
        status(&router, "POST", "/v1/volumes/db1/snapshots", Some(&tok)).await,
        StatusCode::CREATED
    );
    // /healthz needs no token even in OIDC mode.
    assert_eq!(
        status(&router, "GET", "/healthz", None).await,
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn jwks_discovery_resolves_when_no_explicit_url() {
    let issuer = spawn_issuer().await;
    let router = fixture(&issuer, false).await; // via openid-configuration
    let tok = mint(&issuer, "test-key-1", json!({"sub": AGENT_SUB}));
    assert_eq!(
        status(&router, "GET", "/v1/volumes/db1/snapshots", Some(&tok)).await,
        StatusCode::OK
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_scopes_are_enforced() {
    let issuer = spawn_issuer().await;
    let router = fixture(&issuer, true).await;
    let tok = mint(&issuer, "test-key-1", json!({"sub": AGENT_SUB}));
    // Volume outside the db* glob: 403.
    assert_eq!(
        status(&router, "GET", "/v1/volumes/cache1/snapshots", Some(&tok)).await,
        StatusCode::FORBIDDEN
    );
    // Op outside the granted set (rollback): 403 on an allowed volume.
    assert_eq!(
        status(&router, "POST", "/v1/volumes/db1/rollback", Some(&tok)).await,
        StatusCode::FORBIDDEN
    );
    // The agent's mixed-batch step is an ingest op like attach/clear:
    // authorized (then refused on its merits — no such session), never
    // a 403.
    assert_ne!(
        status(
            &router,
            "POST",
            "/v1/volumes/db1/prepared/p/apply",
            Some(&tok)
        )
        .await,
        StatusCode::FORBIDDEN
    );
    // Unknown /v1 path fails closed for restricted identities.
    assert_eq!(
        status(&router, "GET", "/v1/volumes/db1/surprise", Some(&tok)).await,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn role_binding_via_zitadel_shaped_claim() {
    let issuer = spawn_issuer().await;
    let router = fixture(&issuer, true).await;
    let tok = mint(
        &issuer,
        "test-key-1",
        json!({
            "sub": "operator@example.com",
            "urn:zitadel:iam:org:project:roles": {"skynbd-operator": {"org": "x"}},
        }),
    );
    // Wildcard grant: rollback on any volume clears authorization (the
    // handler then rejects the request itself — anything but 401/403
    // proves the role binding admitted us).
    let s = status(&router, "POST", "/v1/volumes/cache1/rollback", Some(&tok)).await;
    assert!(
        s != StatusCode::UNAUTHORIZED && s != StatusCode::FORBIDDEN,
        "role binding should authorize rollback, got {s}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn rejects_bad_tokens() {
    let issuer = spawn_issuer().await;
    let router = fixture(&issuer, true).await;
    let path = "/v1/volumes/db1/snapshots";
    let unauthorized = |s: StatusCode| assert_eq!(s, StatusCode::UNAUTHORIZED);

    unauthorized(status(&router, "GET", path, None).await);
    unauthorized(status(&router, "GET", path, Some("not-a-jwt")).await);
    // Wrong audience.
    let t = mint(
        &issuer,
        "test-key-1",
        json!({"sub": AGENT_SUB, "aud": "other"}),
    );
    unauthorized(status(&router, "GET", path, Some(&t)).await);
    // Expired (beyond the 60 s leeway).
    let t = mint(
        &issuer,
        "test-key-1",
        json!({"sub": AGENT_SUB, "exp": now() - 3600}),
    );
    unauthorized(status(&router, "GET", path, Some(&t)).await);
    // Unknown issuer.
    let t = mint(
        "http://unknown.invalid",
        "test-key-1",
        json!({"sub": AGENT_SUB}),
    );
    unauthorized(status(&router, "GET", path, Some(&t)).await);
    // Unknown kid (refresh finds nothing).
    let t = mint(&issuer, "other-key", json!({"sub": AGENT_SUB}));
    unauthorized(status(&router, "GET", path, Some(&t)).await);
    // Tampered payload breaks the signature.
    let t = mint(&issuer, "test-key-1", json!({"sub": AGENT_SUB}));
    let mut parts: Vec<&str> = t.split('.').collect();
    let forged = json!({"iss": issuer, "aud": AUD, "sub": "root", "exp": now() + 600});
    let forged_b64 = {
        use base64::Engine as _;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(forged.to_string())
    };
    parts[1] = &forged_b64;
    unauthorized(status(&router, "GET", path, Some(&parts.join("."))).await);
}

#[tokio::test(flavor = "multi_thread")]
async fn authenticated_but_unbound_identity_is_forbidden() {
    let issuer = spawn_issuer().await;
    let router = fixture(&issuer, true).await;
    let tok = mint(&issuer, "test-key-1", json!({"sub": "someone-else"}));
    assert_eq!(
        status(&router, "GET", "/v1/volumes/db1/snapshots", Some(&tok)).await,
        StatusCode::FORBIDDEN
    );
}

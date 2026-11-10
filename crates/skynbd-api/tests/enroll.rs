//! Cert enrollment integration tests (auth design §3.2 amendment):
//! OIDC-authorized `POST /v1/certs/enroll` through the real router, and
//! the money test — the enrolled cert completing an mTLS handshake
//! against skynbd-nbd's own client verifier, which must extract exactly
//! the issued SPIFFE identity.

mod common;

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use common::{mint, spawn_issuer, AUD};
use http_body_util::BodyExt;
use serde_json::json;
use skynbd_api::{ApiAuth, ApiState, Binding, CertIssuer, OidcAuth, TrustedIssuer};
use skynbd_nbd::ExportRegistry;
use tower::ServiceExt;

const NODE_SUB: &str = "system:serviceaccount:prod:node-1";
const TRUST_DOMAIN: &str = "test-td";

/// On-disk throwaway CA + the issuer over it. The TempDir keeps the
/// files alive for the test's duration.
fn test_ca(dir: &tempfile::TempDir) -> (CertIssuer, rcgen::Certificate, rcgen::KeyPair) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "skynbd-enroll-it-ca");
    let cert = params.self_signed(&key).unwrap();
    let cert_path = dir.path().join("ca.pem");
    let key_path = dir.path().join("ca.key");
    std::fs::write(&cert_path, cert.pem()).unwrap();
    std::fs::write(&key_path, key.serialize_pem()).unwrap();
    let issuer = CertIssuer {
        ca_cert_path: cert_path,
        ca_key_path: key_path,
        trust_domain: TRUST_DOMAIN.into(),
        ttl: std::time::Duration::from_secs(900),
    };
    (issuer, cert, key)
}

/// Router with no volumes; bindings: node-1 may enroll, reader may not.
fn fixture(issuer_url: &str, auth: Option<ApiAuth>, enroll: Option<CertIssuer>) -> axum::Router {
    let auth = auth.unwrap_or_else(|| {
        ApiAuth::Oidc(OidcAuth::new(vec![TrustedIssuer {
            issuer: issuer_url.to_string(),
            audience: AUD.to_string(),
            jwks_url: Some(format!("{issuer_url}/jwks.json")),
            roles_claim: None,
            tls_ca_pem: None,
            bindings: vec![
                Binding {
                    sub: Some(NODE_SUB.to_string()),
                    role: None,
                    volumes: vec!["*".to_string()],
                    ops: vec!["enroll".into()],
                },
                Binding {
                    sub: Some("system:serviceaccount:prod:reader".to_string()),
                    role: None,
                    volumes: vec!["*".to_string()],
                    ops: vec!["read".into()],
                },
            ],
        }]))
    });
    let state = Arc::new(ApiState {
        volumes: skynbd_api::static_volumes(HashMap::new()),
        registry: Arc::new(ExportRegistry::new()),
        auth,
        enroll,
        provisioner: None,
        snap_requests: Default::default(),
    });
    skynbd_api::router(state)
}

fn make_csr() -> (String, rcgen::KeyPair) {
    let key = rcgen::KeyPair::generate().unwrap();
    let params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    let csr = params.serialize_request(&key).unwrap();
    (csr.pem().unwrap(), key)
}

async fn enroll(
    router: &axum::Router,
    token: Option<&str>,
    csr_pem: &str,
) -> (StatusCode, serde_json::Value) {
    let mut req = Request::builder()
        .method("POST")
        .uri("/v1/certs/enroll")
        .header("content-type", "application/json");
    if let Some(t) = token {
        req = req.header("authorization", format!("Bearer {t}"));
    }
    let body = json!({ "csr_pem": csr_pem }).to_string();
    let resp = router
        .clone()
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let val = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, val)
}

#[tokio::test(flavor = "multi_thread")]
async fn enrolled_cert_passes_the_nbd_verifier() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_issuer, ca_cert, ca_key) = test_ca(&dir);
    let ca_pem = ca_cert.pem();
    let oidc = spawn_issuer().await;
    let router = fixture(&oidc, None, Some(cert_issuer));

    let tok = mint(&oidc, "test-key-1", json!({"sub": NODE_SUB}));
    let (csr, client_key) = make_csr();
    let (status, resp) = enroll(&router, Some(&tok), &csr).await;
    assert_eq!(status, StatusCode::OK, "enroll failed: {resp}");
    let expected_id = format!("spiffe://{TRUST_DOMAIN}/system/serviceaccount/prod/node-1");
    assert_eq!(resp["identity"], expected_id.as_str());
    assert_eq!(resp["ca_pem"].as_str().unwrap().trim(), ca_pem.trim());
    let cert_pem = resp["cert_pem"].as_str().unwrap();

    // The round trip: an NBD server trusting the enrollment root accepts
    // an mTLS handshake with the issued cert, and its identity extractor
    // sees exactly the issued SPIFFE ID.
    let server_key = rcgen::KeyPair::generate().unwrap();
    let server_cert = rcgen::CertificateParams::new(vec!["localhost".to_string()])
        .unwrap()
        .signed_by(&server_key, &ca_cert, &ca_key)
        .unwrap();
    let server_config = skynbd_nbd::tls::build_server_config(
        server_cert.pem().as_bytes(),
        server_key.serialize_pem().as_bytes(),
        ca_pem.as_bytes(),
    )
    .unwrap();

    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut ca_pem.as_bytes()) {
        roots.add(c.unwrap()).unwrap();
    }
    let client_certs: Vec<_> = rustls_pemfile::certs(&mut cert_pem.as_bytes())
        .collect::<Result<_, _>>()
        .unwrap();
    let client_priv = rustls_pemfile::private_key(&mut client_key.serialize_pem().as_bytes())
        .unwrap()
        .unwrap();
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let client_config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_root_certificates(roots)
        .with_client_auth_cert(client_certs, client_priv)
        .unwrap();

    let (client_io, server_io) = tokio::io::duplex(64 * 1024);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config);
    let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config));
    let server = tokio::spawn(async move {
        let tls = acceptor.accept(server_io).await.unwrap();
        let peer = tls.get_ref().1.peer_certificates().unwrap()[0].clone();
        skynbd_nbd::tls::peer_identity(&peer)
    });
    let sni = rustls::pki_types::ServerName::try_from("localhost").unwrap();
    let _client = connector.connect(sni, client_io).await.unwrap();
    assert_eq!(server.await.unwrap().as_deref(), Some(expected_id.as_str()));
}

#[tokio::test(flavor = "multi_thread")]
async fn enroll_requires_the_enroll_grant() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_issuer, _, _) = test_ca(&dir);
    let oidc = spawn_issuer().await;
    let router = fixture(&oidc, None, Some(cert_issuer));
    let tok = mint(
        &oidc,
        "test-key-1",
        json!({"sub": "system:serviceaccount:prod:reader"}),
    );
    let (csr, _) = make_csr();
    let (status, _) = enroll(&router, Some(&tok), &csr).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    // No token at all: 401.
    let (status, _) = enroll(&router, None, &csr).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread")]
async fn static_mode_carries_no_identity_so_cannot_enroll() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_issuer, _, _) = test_ca(&dir);
    let router = fixture(
        "unused",
        Some(ApiAuth::Static("tok".into())),
        Some(cert_issuer),
    );
    let (csr, _) = make_csr();
    let (status, body) = enroll(&router, Some("tok"), &csr).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
}

#[tokio::test(flavor = "multi_thread")]
async fn unconfigured_enrollment_is_404() {
    let oidc = spawn_issuer().await;
    let router = fixture(&oidc, None, None);
    let tok = mint(&oidc, "test-key-1", json!({"sub": NODE_SUB}));
    let (csr, _) = make_csr();
    let (status, _) = enroll(&router, Some(&tok), &csr).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn garbage_csr_is_400() {
    let dir = tempfile::tempdir().unwrap();
    let (cert_issuer, _, _) = test_ca(&dir);
    let oidc = spawn_issuer().await;
    let router = fixture(&oidc, None, Some(cert_issuer));
    let tok = mint(&oidc, "test-key-1", json!({"sub": NODE_SUB}));
    let (status, _) = enroll(&router, Some(&tok), "-----BEGIN JUNK-----").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

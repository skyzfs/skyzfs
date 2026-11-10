//! Issuer `tls_ca` tests (CSI design 6c): the JWKS fetch must succeed
//! against an issuer whose HTTPS endpoint is signed by a private CA
//! (e.g. a Kubernetes API server signed by the cluster CA) when that CA
//! is configured — and must keep failing closed without it.

mod common;

use std::sync::Arc;

use common::{mint, AUD, TEST_JWKS};
use serde_json::json;
use skynbd_api::auth::AuthError;
use skynbd_api::{Binding, OidcAuth, TrustedIssuer};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Minimal HTTPS server answering every request with the test JWKS,
/// using a self-signed cert for 127.0.0.1. Returns (base_url, ca_pem).
async fn spawn_tls_jwks() -> (String, String) {
    let key = rcgen::KeyPair::generate().unwrap();
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params
        .subject_alt_names
        .push(rcgen::SanType::IpAddress(std::net::IpAddr::V4(
            std::net::Ipv4Addr::new(127, 0, 0, 1),
        )));
    let cert = params.self_signed(&key).unwrap();
    let ca_pem = cert.pem();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let certs = rustls_pemfile::certs(&mut ca_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let priv_key = rustls_pemfile::private_key(&mut key.serialize_pem().as_bytes())
        .unwrap()
        .unwrap();
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(certs, priv_key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("https://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        loop {
            let Ok((tcp, _)) = listener.accept().await else {
                return;
            };
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                // Read until the end of the request head, then answer.
                let mut buf = vec![0u8; 4096];
                let mut n = 0;
                while !buf[..n].windows(4).any(|w| w == b"\r\n\r\n") {
                    match tls.read(&mut buf[n..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(m) => n += m,
                    }
                }
                let body = TEST_JWKS;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(resp.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    (base, ca_pem)
}

fn issuer(base: &str, tls_ca_pem: Option<Vec<u8>>) -> TrustedIssuer {
    TrustedIssuer {
        issuer: base.to_string(),
        audience: AUD.to_string(),
        jwks_url: Some(format!("{base}/jwks.json")),
        roles_claim: None,
        tls_ca_pem,
        bindings: vec![Binding {
            sub: Some("*".into()),
            role: None,
            volumes: vec!["*".into()],
            ops: vec!["read".into()],
        }],
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn private_ca_jwks_works_with_tls_ca_and_fails_closed_without() {
    let (base, ca_pem) = spawn_tls_jwks().await;
    let tok = mint(&base, "test-key-1", json!({"sub": "workload"}));

    // Without the CA the fetch fails TLS verification → no key → 401.
    let bare = OidcAuth::new(vec![issuer(&base, None)]);
    match bare.authenticate(&tok).await {
        Err(AuthError::Unauthenticated(msg)) => assert_eq!(msg, "unknown signing key"),
        _ => panic!("expected unknown signing key without tls_ca"),
    }

    // With the CA configured the same token validates.
    let trusted = OidcAuth::new(vec![issuer(&base, Some(ca_pem.into_bytes()))]);
    let ctx = trusted.authenticate(&tok).await.expect("token validates");
    assert_eq!(ctx.sub.as_deref(), Some("workload"));
}

#[test]
#[should_panic(expected = "invalid PEM certificate")]
fn garbage_tls_ca_fails_at_startup() {
    let _ = OidcAuth::new(vec![issuer("https://x", Some(b"not pem".to_vec()))]);
}

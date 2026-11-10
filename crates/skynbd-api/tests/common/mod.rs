//! Shared OIDC test rig: a local JWKS+discovery server and an RS256
//! token mint, used by the auth and enrollment integration tests.
#![allow(dead_code)] // each test binary uses a different subset

use axum::routing::get;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde_json::json;

/// Test-only RSA keypair: private half signs tokens, public half is the
/// JWKS the server fetches. Generated for these tests; secures nothing.
pub const TEST_PEM: &str = "-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC1zx4SrRNmLuMX
aeVE/qvXvnet0NmxVyQn60LvcvzjQ1H6m5sTOOKEwY7YajC2aP+MP3VYH2EQoqA5
e/ORGtGk51DqjYD8N3lvuvJcqkO0fSyrY4aDtf3atzM4+xn+1+fvKb8mbdtKW3bH
uQf8uQKO+mQ4G1FqrPAQ/6sG0GePWgXQnHSbxTVMRS7F/BdHWUCj/0OIrcNv6jWL
WXfHLHbmrkh1cKOw0E5t6IaWA6Sx7X/mQfyHO7mzKxsoeik/qavGd/jyfFRmZ8Ey
bkWcWTeLsE4i32HPlWTTppJkNtTEb3fbMOaHw48LX3oKpN1X0JkxpSND14m2CTol
nr1F8iAlAgMBAAECggEAAXzGd2BQx9BmDY4mu4fv2k3Vxydtwyx+KxDFf+sAvvqR
LvaF8Bl835mBHyvN5o15Bb82fmLnVY5XDDwDiUeJuTUmmCdR4P5dMSOmPEmCUKAz
ijAVbM9uL74O87jjhLkPIzREED7sK9wQCTXuJvb72oqZi6H26mNW0B90rpNsPXkm
Bd5HnGzpHlyKS+WYWAcfI+W/GwdNCDZHnjxelhdwD765JglqkIln47h0CEZjO1NM
o2BBRYB8ryqYJ7fGoP+0hP45G/oyXK6dMHjfOm+XriJ8bvVLxyi0BN0AF7MTCw3n
EKOGpdx5ihMtqBw9h7acV9kzFPH0QMVQUa+QbZx/QQKBgQDuTjyCTdxzWxmMJZk4
VxJ6K/zn5LdAFsX5YtEl8LrtZfLipJMkhevbYjtkoNT/f2bjcaXOgxfqdPZ/jfpH
D4+NHEazq6Zxskv/F59iCjGtgzyECnIKd0xC3qD44sYyw6m0lNEbFzuUjyNpo9F8
4FpqqbqRryQIw5ut6rP9NHqe4QKBgQDDTvuHpwZ/mpgaa5lELoW4X98u8icGuVxW
pm38Js4XimJSbx0H0vlBFyGDVLaPXv6hi1cyCBMrKTkmPLX9Wgn084bEbM6Pwyjz
KajqG7/fydM/5Xe7jdmjt0K8bJcxBX5TNdu3/zvzOFSztysVXisdCx5oIbHlqfsS
ouaOzkJ9xQKBgCaYOSiJX6i3Gl4eQMBlKbLrqNwNqLgSeCG5pVHVuRB1sG6QZNxH
bDltoVzcx2JRyeIQJ+BYTulD5CAoaF5V4NPjvnee1ygWzkQUGVYRfJYzaPuhjXKw
e4Ng9wUYBS13juaWK3+NtbKLtBqJ04W7+CjKMFMj4FwjAfWay2e4thtBAoGBAMMa
V3J/i6JVELTz37uYrcQtIYqIJCw3ZuK4hw5oxWAGwYmPrfRu8MfxD21eZrB6OOuv
gn9cSanIGGr2Iubf1yp3S51VBFPgpTuj1jVnWxl3SCyAUkdtwIgnPed0ouQ5lKQT
HVaFBaJGF3cf9031MCRnQwzK/Cds0n8fdCocKZfxAoGBAJVll1MiLlFo0nvrLgWj
lMYZioRlRQQdGK4zrTiX1fhzIcM2s8ABB02kCuECrXV/KIlgdDs6F5cMk0OupyxH
8wUL6irWFQySAKTmzRBdRJhbbJ6BXG4vIXpLCpz4N+am+QWCQMSXXsUvWcyGMKz/
qdD1J/4i34IjFXnymG4IFLbY
-----END PRIVATE KEY-----";

pub const TEST_JWKS: &str = r#"{"keys": [{"kty": "RSA", "use": "sig", "alg": "RS256", "kid": "test-key-1", "n": "tc8eEq0TZi7jF2nlRP6r1753rdDZsVckJ-tC73L840NR-pubEzjihMGO2Gowtmj_jD91WB9hEKKgOXvzkRrRpOdQ6o2A_Dd5b7ryXKpDtH0sq2OGg7X92rczOPsZ_tfn7ym_Jm3bSlt2x7kH_LkCjvpkOBtRaqzwEP-rBtBnj1oF0Jx0m8U1TEUuxfwXR1lAo_9DiK3Db-o1i1l3xyx25q5IdXCjsNBObeiGlgOkse1_5kH8hzu5sysbKHopP6mrxnf48nxUZmfBMm5FnFk3i7BOIt9hz5Vk06aSZDbUxG932zDmh8OPC196CqTdV9CZMaUjQ9eJtgk6JZ69RfIgJQ", "e": "AQAB"}]}"#;

pub const AUD: &str = "skynbd-api";

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

/// Serve the JWKS (and an OIDC discovery doc pointing at it) on a local
/// port; the returned base URL doubles as the issuer string.
pub async fn spawn_issuer() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let disc_base = base.clone();
    let app = axum::Router::new()
        .route("/jwks.json", get(|| async { TEST_JWKS }))
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let jwks = format!("{disc_base}/jwks.json");
                async move { axum::Json(json!({ "jwks_uri": jwks })) }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

pub fn mint(issuer: &str, kid: &str, mut claims: serde_json::Value) -> String {
    let defaults = json!({
        "iss": issuer, "aud": AUD,
        "iat": now(), "exp": now() + 600,
    });
    for (k, v) in defaults.as_object().unwrap() {
        claims
            .as_object_mut()
            .unwrap()
            .entry(k.clone())
            .or_insert(v.clone());
    }
    let mut header = Header::new(Algorithm::RS256);
    header.kid = Some(kid.to_string());
    let key = EncodingKey::from_rsa_pem(TEST_PEM.as_bytes()).unwrap();
    jsonwebtoken::encode(&header, &claims, &key).unwrap()
}

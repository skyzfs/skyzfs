//! Bearer authentication + authorization (auth design §4).
//!
//! Three modes: `Disabled` (dev/tests), `Static` (single shared token,
//! break-glass), and `Oidc` — offline JWT validation against one or more
//! trusted issuers (multi-issuer: cluster ServiceAccount issuers for
//! workloads, an org IdP such as Zitadel for humans/CI, design §4.1),
//! with per-identity bindings mapping `sub` or a roles claim to volume
//! globs and operation groups. JWKS are fetched from OIDC discovery (or
//! an explicit `jwks_url`), cached, and refreshed on unknown `kid`.

use std::sync::Arc;
use std::time::Duration;

use axum::http::Method;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation};
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tokio::time::Instant;

/// Signature algorithms we accept from issuers. `HS*` is deliberately
/// absent: symmetric algs would let anyone with the JWKS mint tokens.
const ALLOWED_ALGS: &[Algorithm] = &[
    Algorithm::RS256,
    Algorithm::RS384,
    Algorithm::RS512,
    Algorithm::ES256,
    Algorithm::ES384,
    Algorithm::EdDSA,
];

/// Minimum interval between JWKS refetches per issuer, so a flood of
/// bad-kid tokens cannot hammer the issuer.
const JWKS_REFRESH_MIN_INTERVAL: Duration = Duration::from_secs(30);

/// One `sub`/role → grants rule (design §4). `sub` and `role` are glob
/// patterns (`*` wildcard); a binding matches if either matches the
/// authenticated identity. Ops name route groups (see [`route_perm`]).
#[derive(Debug, Clone)]
pub struct Binding {
    pub sub: Option<String>,
    pub role: Option<String>,
    pub volumes: Vec<String>,
    pub ops: Vec<String>,
}

/// One trusted issuer (design §4): tokens are dispatched by their `iss`
/// claim, validated offline against this issuer's JWKS, then authorized
/// through its bindings.
#[derive(Debug, Clone)]
pub struct TrustedIssuer {
    pub issuer: String,
    pub audience: String,
    /// Explicit JWKS endpoint; `None` resolves it from
    /// `{issuer}/.well-known/openid-configuration` on first use.
    pub jwks_url: Option<String>,
    /// Claim carrying role names for `role` bindings (e.g. Zitadel's
    /// `urn:zitadel:iam:org:project:roles`). Accepts a string, an array
    /// of strings, or an object (role names as keys).
    pub roles_claim: Option<String>,
    /// Extra root CA (PEM) trusted for this issuer's HTTPS JWKS/discovery
    /// fetches — required when the issuer is a Kubernetes API server
    /// signed by the cluster CA (CSI design 6c). `None` = system roots.
    pub tls_ca_pem: Option<Vec<u8>>,
    pub bindings: Vec<Binding>,
}

pub enum ApiAuth {
    Disabled,
    Static(String),
    Oidc(OidcAuth),
}

pub struct OidcAuth {
    issuers: Vec<IssuerState>,
}

struct IssuerState {
    cfg: TrustedIssuer,
    /// Per-issuer client: carries the issuer's extra root CA when one is
    /// configured, system roots otherwise.
    http: reqwest::Client,
    /// Resolved JWKS endpoint (discovery result or the explicit URL).
    jwks_url: RwLock<Option<String>>,
    jwks: RwLock<Option<Arc<JwkSet>>>,
    last_refresh: RwLock<Option<Instant>>,
}

/// What an authenticated caller may do. `grants: None` = unrestricted
/// (disabled/static modes); `Some` = the union of matched bindings.
#[derive(Clone, Default)]
pub struct AuthzCtx {
    pub identity: Option<String>,
    /// The verified `sub` claim alone (OIDC mode only) — the input to
    /// cert-enrollment SAN derivation, which must never come from an
    /// unverified source.
    pub sub: Option<String>,
    grants: Option<Arc<Vec<Grant>>>,
}

#[derive(Debug)]
struct Grant {
    volumes: Vec<String>,
    ops: Vec<String>,
}

#[derive(Debug)]
pub enum AuthError {
    /// 401: missing/invalid credentials.
    Unauthenticated(&'static str),
    /// 403: authenticated, but no binding grants this operation.
    Forbidden,
}

impl AuthzCtx {
    pub fn full() -> Self {
        Self::default()
    }

    pub fn is_restricted(&self) -> bool {
        self.grants.is_some()
    }

    /// May this caller perform `op` (optionally on volume `vol`)?
    pub fn allows(&self, op: &str, vol: Option<&str>) -> bool {
        let Some(grants) = &self.grants else {
            return true;
        };
        grants.iter().any(|g| {
            g.ops.iter().any(|o| o == op || o == "*")
                && match vol {
                    None => true, // non-volume-scoped route: op alone decides
                    Some(v) => g.volumes.iter().any(|pat| glob_match(pat, v)),
                }
        })
    }
}

/// Constant-time static-token comparison (design §4).
pub fn static_token_matches(expected: &str, got: &str) -> bool {
    expected.as_bytes().ct_eq(got.as_bytes()).into()
}

/// Both issuer clients, with timeouts. Neither had any, and this one sits
/// on the *request* path: a JWKS refresh happens while authenticating, so
/// an issuer that accepts and never answers holds the request that
/// triggered it for as long as it stays silent. An issuer is a third
/// party over the network, which is the failure everyone plans for and
/// nobody bounds.
///
/// Exactly ONE request per issuer is held, not every concurrent one:
/// `refresh_jwks` stamps `last_refresh` and drops the guard *before* the
/// fetch, so the 30 s rate limit turns every other caller around
/// immediately (measured: 46 µs, against a `last_refresh` already
/// stamped). And one read timeout, not two — `resolve_jwks_url` does
/// `.ok()?` on a failed discovery, so a silent issuer never reaches the
/// second fetch. Both are deliberate and worth not un-learning.
///
/// Shorter than the agent's 60 s because the shapes differ: a JWKS
/// document is a small fixed fetch with no upload, and the caller is a
/// client waiting on one HTTP request, not a background cycle with an
/// hour to spend. Failing the fetch is not fatal — a cached JWKS keeps
/// serving, and only a genuinely-new key needs the network.
fn jwks_client_builder() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .read_timeout(std::time::Duration::from_secs(10))
}

impl OidcAuth {
    /// Panics on an unparseable `tls_ca_pem` — that is a config error and
    /// belongs at startup, not buried in a later fetch failure.
    pub fn new(issuers: Vec<TrustedIssuer>) -> Self {
        Self {
            issuers: issuers
                .into_iter()
                .map(|cfg| IssuerState {
                    http: match &cfg.tls_ca_pem {
                        Some(pem) => {
                            // reqwest defers cert parsing to connect time
                            // with the rustls backend; validate here so a
                            // bad file is a startup error, not a mystery
                            // fetch failure later.
                            let n = rustls_pemfile::certs(&mut pem.as_slice())
                                .filter(|c| c.is_ok())
                                .count();
                            assert!(n > 0, "issuer tls_ca: invalid PEM certificate");
                            jwks_client_builder()
                                .add_root_certificate(
                                    reqwest::Certificate::from_pem(pem)
                                        .expect("issuer tls_ca: invalid PEM certificate"),
                                )
                                .build()
                                .expect("build issuer http client")
                        }
                        None => jwks_client_builder()
                            .build()
                            .expect("build issuer http client"),
                    },
                    jwks_url: RwLock::new(cfg.jwks_url.clone()),
                    jwks: RwLock::new(None),
                    last_refresh: RwLock::new(None),
                    cfg,
                })
                .collect(),
        }
    }

    /// Validate a bearer token and compute its grants. Offline except for
    /// JWKS (re)fetches.
    pub async fn authenticate(&self, token: &str) -> Result<AuthzCtx, AuthError> {
        const BAD: AuthError = AuthError::Unauthenticated("invalid bearer token");
        // Dispatch on the *unverified* iss claim — trust still comes from
        // the signature check against that issuer's JWKS below.
        let iss = unverified_issuer(token).ok_or(BAD)?;
        let state = self
            .issuers
            .iter()
            .find(|s| s.cfg.issuer == iss)
            .ok_or(AuthError::Unauthenticated("unknown issuer"))?;

        let header = jsonwebtoken::decode_header(token).map_err(|_| BAD)?;
        if !ALLOWED_ALGS.contains(&header.alg) {
            return Err(AuthError::Unauthenticated("disallowed algorithm"));
        }
        let kid = header.kid.ok_or(BAD)?;

        let key = match self.find_key(state, &kid, false).await {
            Some(k) => k,
            // Unknown kid: the issuer may have rotated — refetch once.
            None => self
                .find_key(state, &kid, true)
                .await
                .ok_or(AuthError::Unauthenticated("unknown signing key"))?,
        };

        let mut validation = Validation::new(header.alg);
        validation.set_audience(&[&state.cfg.audience]);
        validation.set_issuer(&[&state.cfg.issuer]);
        validation.leeway = 60;
        let data =
            jsonwebtoken::decode::<serde_json::Value>(token, &key, &validation).map_err(|_| BAD)?;

        let sub = data
            .claims
            .get("sub")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let roles = state
            .cfg
            .roles_claim
            .as_deref()
            .map(|c| extract_roles(&data.claims, c))
            .unwrap_or_default();

        let grants: Vec<Grant> = state
            .cfg
            .bindings
            .iter()
            .filter(|b| {
                let sub_ok = b.sub.as_deref().is_some_and(|pat| glob_match(pat, sub));
                let role_ok = b
                    .role
                    .as_deref()
                    .is_some_and(|pat| roles.iter().any(|r| glob_match(pat, r)));
                sub_ok || role_ok
            })
            .map(|b| Grant {
                volumes: b.volumes.clone(),
                ops: b.ops.clone(),
            })
            .collect();
        if grants.is_empty() {
            return Err(AuthError::Forbidden);
        }
        Ok(AuthzCtx {
            identity: Some(format!("{iss}#{sub}")),
            sub: Some(sub.to_string()),
            grants: Some(Arc::new(grants)),
        })
    }

    /// Look up `kid` in the cached JWKS, optionally forcing a rate-limited
    /// refetch first.
    async fn find_key(&self, state: &IssuerState, kid: &str, refresh: bool) -> Option<DecodingKey> {
        if refresh || state.jwks.read().await.is_none() {
            self.refresh_jwks(state).await;
        }
        let jwks = state.jwks.read().await.clone()?;
        let jwk = jwks.find(kid)?;
        DecodingKey::from_jwk(jwk).ok()
    }

    async fn refresh_jwks(&self, state: &IssuerState) {
        let mut last = state.last_refresh.write().await;
        if last.is_some_and(|t| t.elapsed() < JWKS_REFRESH_MIN_INTERVAL) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);

        let url = match self.resolve_jwks_url(state).await {
            Some(u) => u,
            None => return,
        };
        match state.http.get(&url).send().await {
            Ok(resp) => match resp.json::<JwkSet>().await {
                Ok(set) => *state.jwks.write().await = Some(Arc::new(set)),
                Err(e) => tracing::warn!(issuer = %state.cfg.issuer, error = %e, "bad JWKS body"),
            },
            Err(e) => tracing::warn!(issuer = %state.cfg.issuer, error = %e, "JWKS fetch failed"),
        }
    }

    async fn resolve_jwks_url(&self, state: &IssuerState) -> Option<String> {
        if let Some(u) = state.jwks_url.read().await.clone() {
            return Some(u);
        }
        let disc = format!(
            "{}/.well-known/openid-configuration",
            state.cfg.issuer.trim_end_matches('/')
        );
        let doc: serde_json::Value = state.http.get(&disc).send().await.ok()?.json().await.ok()?;
        let url = doc.get("jwks_uri")?.as_str()?.to_string();
        *state.jwks_url.write().await = Some(url.clone());
        Some(url)
    }
}

/// Read the `iss` claim without verifying — dispatch only.
fn unverified_issuer(token: &str) -> Option<String> {
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    Some(v.get("iss")?.as_str()?.to_string())
}

/// Roles from a claim that may be a string, array of strings, or object
/// keyed by role name (Zitadel's project-roles shape).
fn extract_roles(claims: &serde_json::Value, claim: &str) -> Vec<String> {
    match claims.get(claim) {
        Some(serde_json::Value::String(s)) => vec![s.clone()],
        Some(serde_json::Value::Array(a)) => a
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        Some(serde_json::Value::Object(o)) => o.keys().cloned().collect(),
        _ => Vec::new(),
    }
}

/// `*`-wildcard glob match (no other metacharacters).
pub fn glob_match(pat: &str, s: &str) -> bool {
    fn inner(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => inner(&p[1..], s) || (!s.is_empty() && inner(p, &s[1..])),
            (Some(pc), Some(sc)) if pc == sc => inner(&p[1..], &s[1..]),
            _ => false,
        }
    }
    inner(pat.as_bytes(), s.as_bytes())
}

/// Route → (operation group, volume) table for authorization (design §4).
/// Returns `None` for paths outside the known /v1 surface — restricted
/// tokens are denied there (fail closed); unrestricted callers fall
/// through to the router's 404.
///
/// Groups: `read` (all GETs), `snapshot` (create/delete/pin), `ingest`
/// (prepared lifecycle, object upload, import), `diff`, `rollback`,
/// `rpo`, `gc` (gc + fsck), `enroll` (mTLS client-cert issuance, §3.2),
/// `volume` (dynamic volume create/delete — the CSI controller's grant),
/// and `clone` — enforced in the create-volume *handler* against the
/// body-carried source volume (CSI design §4.1: cloning exfiltrates a
/// volume, so it is a distinct capability, never implied by `volume`).
pub fn route_perm(method: &Method, path: &str) -> Option<(&'static str, Option<String>)> {
    let seg: Vec<&str> = path.trim_matches('/').split('/').collect();
    let get = *method == Method::GET;
    match seg.as_slice() {
        ["v1", "volumes"] if get => Some(("read", None)),
        ["v1", "volumes"] if *method == Method::POST => Some(("volume", None)),
        ["v1", "volumes", vol] if *method == Method::DELETE => {
            Some(("volume", Some(vol.to_string())))
        }
        ["v1", "exports"] if get => Some(("read", None)),
        // D-M9 catalog rebuild: maintenance-grade, like gc/fsck.
        ["v1", "catalog", "rebuild"] if *method == Method::POST => Some(("gc", None)),
        ["v1", "certs", "enroll"] if *method == Method::POST => Some(("enroll", None)),
        ["v1", "volumes", vol, rest @ ..] => {
            let vol = Some(vol.to_string());
            let op = match (rest, method.as_str()) {
                (["snapshots"], "GET") => "read",
                (["snapshots"], "POST") => "snapshot",
                (["snapshots", _], "DELETE") => "snapshot",
                (["snapshots", _, "pins"], "GET") => "read",
                (["snapshots", _, "pins"], "POST") => "snapshot",
                (["snapshots", _, "pins", _], "DELETE") => "snapshot",
                (["snapshots", _, "data"], "GET") => "read",
                // Alias: the agent's final ingestion step (6h-2).
                (["snapshots", _, "alias"], "POST") => "ingest",
                // Requests: created by the snapshot caller, consumed by
                // the volume's ingesting agent.
                (["snapshot-requests"], "POST") => "snapshot",
                (["snapshot-requests"], "GET") => "ingest",
                // Status read (D-M12): the snapshot caller asking why
                // its request isn't ready — same grant that filed it.
                (["snapshot-requests", _], "GET") => "snapshot",
                (["snapshot-requests", _], "DELETE") => "ingest",
                // Cycle progress/failure reports from the ingesting agent.
                (["cycle-status"], "POST") => "ingest",
                // Origin pointer (D-M15): read is snapshot metadata; the
                // repoint mutates lineage serving state like the other
                // agent-side ingestion steps.
                (["origin"], "GET") => "read",
                (["origin"], "POST") => "ingest",
                // D-M6 handoff: the node plugin's stage/unstage calls.
                (["serve"], "POST") | (["release"], "POST") => "volume",
                (["rollback"], "POST") => "rollback",
                (["prepared"], "GET") => "read",
                (["prepared"], "POST") => "ingest",
                (["prepared", _], "GET") => "read",
                (["prepared", _], "DELETE") => "ingest",
                (
                    ["prepared", _, "attach" | "clear" | "apply" | "ensure" | "publish" | "checkpoint"],
                    "POST",
                ) => "ingest",
                (["objects", _], "PUT") => "ingest",
                (["gc"], "POST") | (["fsck"], "POST") | (["thin"], "POST") => "gc",
                (["diff"], "GET") => "diff",
                (["import"], "POST") => "ingest",
                (["rpo"], "POST") => "rpo",
                _ => return None,
            };
            Some((op, vol))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_basics() {
        assert!(glob_match("*", "anything"));
        assert!(glob_match("db-*", "db-prod"));
        assert!(!glob_match("db-*", "cache-prod"));
        assert!(glob_match("*/head", "vol1/head"));
        assert!(glob_match("exact", "exact"));
        assert!(!glob_match("exact", "exact2"));
        assert!(glob_match("a*b*c", "a-x-b-y-c"));
        assert!(!glob_match("", "x"));
        assert!(glob_match("", ""));
    }

    #[test]
    fn route_perm_table() {
        let g = Method::GET;
        let p = Method::POST;
        let d = Method::DELETE;
        let u = Method::PUT;
        let vol = |o: &'static str| Some((o, Some("v".to_string())));
        assert_eq!(route_perm(&g, "/v1/volumes"), Some(("read", None)));
        assert_eq!(route_perm(&g, "/v1/exports"), Some(("read", None)));
        assert_eq!(route_perm(&p, "/v1/certs/enroll"), Some(("enroll", None)));
        assert_eq!(route_perm(&g, "/v1/certs/enroll"), None);
        assert_eq!(route_perm(&p, "/v1/volumes"), Some(("volume", None)));
        assert_eq!(route_perm(&d, "/v1/volumes/v"), vol("volume"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/serve"), vol("volume"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/release"), vol("volume"));
        assert_eq!(route_perm(&g, "/v1/volumes/v/serve"), None);
        assert_eq!(route_perm(&p, "/v1/catalog/rebuild"), Some(("gc", None)));
        assert_eq!(route_perm(&g, "/v1/catalog/rebuild"), None);
        assert_eq!(route_perm(&g, "/v1/volumes/v/snapshots"), vol("read"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/snapshots"), vol("snapshot"));
        assert_eq!(route_perm(&d, "/v1/volumes/v/snapshots/s"), vol("snapshot"));
        assert_eq!(
            route_perm(&g, "/v1/volumes/v/snapshots/s/pins"),
            vol("read")
        );
        assert_eq!(
            route_perm(&d, "/v1/volumes/v/snapshots/s/pins/h"),
            vol("snapshot")
        );
        assert_eq!(
            route_perm(&g, "/v1/volumes/v/snapshots/s/data"),
            vol("read")
        );
        assert_eq!(
            route_perm(&p, "/v1/volumes/v/snapshots/s/alias"),
            vol("ingest")
        );
        assert_eq!(
            route_perm(&p, "/v1/volumes/v/snapshot-requests"),
            vol("snapshot")
        );
        assert_eq!(
            route_perm(&g, "/v1/volumes/v/snapshot-requests"),
            vol("ingest")
        );
        assert_eq!(
            route_perm(&g, "/v1/volumes/v/snapshot-requests/n"),
            vol("snapshot")
        );
        assert_eq!(
            route_perm(&d, "/v1/volumes/v/snapshot-requests/n"),
            vol("ingest")
        );
        assert_eq!(route_perm(&p, "/v1/volumes/v/cycle-status"), vol("ingest"));
        assert_eq!(route_perm(&g, "/v1/volumes/v/origin"), vol("read"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/origin"), vol("ingest"));
        assert_eq!(route_perm(&d, "/v1/volumes/v/origin"), None);
        assert_eq!(route_perm(&g, "/v1/volumes/v/cycle-status"), None);
        assert_eq!(route_perm(&p, "/v1/volumes/v/rollback"), vol("rollback"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/prepared"), vol("ingest"));
        assert_eq!(route_perm(&g, "/v1/volumes/v/prepared/i"), vol("read"));
        assert_eq!(route_perm(&d, "/v1/volumes/v/prepared/i"), vol("ingest"));
        // Every prepared-session step the agent posts is an ingest op —
        // a step missing here is a 403 for every OIDC-bound agent.
        for sub in [
            "attach",
            "clear",
            "apply",
            "ensure",
            "publish",
            "checkpoint",
        ] {
            assert_eq!(
                route_perm(&p, &format!("/v1/volumes/v/prepared/i/{sub}")),
                vol("ingest")
            );
        }
        assert_eq!(route_perm(&u, "/v1/volumes/v/objects/abc"), vol("ingest"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/gc"), vol("gc"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/fsck"), vol("gc"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/thin"), vol("gc"));
        assert_eq!(route_perm(&g, "/v1/volumes/v/diff"), vol("diff"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/import"), vol("ingest"));
        assert_eq!(route_perm(&p, "/v1/volumes/v/rpo"), vol("rpo"));
        // Fail closed on the unknown.
        assert_eq!(route_perm(&g, "/v1/surprise"), None);
        assert_eq!(route_perm(&p, "/v1/volumes/v/surprise"), None);
        // Wrong method on a known path is also unknown.
        assert_eq!(route_perm(&d, "/v1/volumes"), None);
    }

    #[test]
    fn authz_ctx_grants() {
        let ctx = AuthzCtx {
            identity: Some("i#s".into()),
            sub: Some("s".into()),
            grants: Some(Arc::new(vec![Grant {
                volumes: vec!["db-*".into()],
                ops: vec!["ingest".into(), "snapshot".into()],
            }])),
        };
        assert!(ctx.allows("ingest", Some("db-prod")));
        assert!(!ctx.allows("ingest", Some("cache")));
        assert!(!ctx.allows("rollback", Some("db-prod")));
        assert!(ctx.allows("snapshot", None));
        assert!(AuthzCtx::full().allows("anything", Some("any")));
    }

    #[test]
    fn roles_claim_shapes() {
        let j = serde_json::json!({
            "s": "admin",
            "a": ["r1", "r2"],
            "o": {"skynbd-operator": {"org": "x"}},
        });
        assert_eq!(extract_roles(&j, "s"), vec!["admin"]);
        assert_eq!(extract_roles(&j, "a"), vec!["r1", "r2"]);
        assert_eq!(extract_roles(&j, "o"), vec!["skynbd-operator"]);
        assert!(extract_roles(&j, "missing").is_empty());
    }

    #[test]
    fn static_compare() {
        assert!(static_token_matches("tok", "tok"));
        assert!(!static_token_matches("tok", "tok2"));
        assert!(!static_token_matches("tok", ""));
    }
}

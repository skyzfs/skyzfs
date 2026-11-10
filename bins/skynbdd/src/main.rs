//! skynbdd — the SkyNBD daemon (design §3.1).
//!
//! M2 scope: serve NBD from a static TOML config (volumes by name, memory
//! or S3 store, redb or memory refs). The REST control plane replaces the
//! static volume list in M3 (design §12); graceful shutdown checkpoints
//! every open volume.
//!
//! Usage: `skynbdd <config.toml>`

use std::sync::Arc;

use serde::Deserialize;
use skynbd_api::{ApiState, Provisioner, VolumeSpec};
use skynbd_nbd::{ExportRegistry, Server};
use skynbd_refs::{MemoryRefStore, ObjectRefStore, RedbRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default = "default_listen")]
    nbd_listen: String,
    #[serde(default = "default_api_listen")]
    api_listen: String,
    /// Path to the redb refs file; omit for in-memory refs (testing only).
    refs_path: Option<String>,
    /// Where refs live: "redb" (default when `refs_path` is set),
    /// "memory" (default otherwise; testing only), or "object" — the
    /// per-volume refs object in the same store as the data (design
    /// D-M1), which is what lets a second server see this one's volumes.
    /// "object" requires a store with conditional writes; startup probes
    /// and refuses with a clear message otherwise.
    refs_kind: Option<String>,
    /// This server's identity as a lease holder (design D-M3); only
    /// meaningful with `refs_kind = "object"`. Defaults to $HOSTNAME —
    /// set it explicitly anywhere identity must survive a pod rename.
    node_id: Option<String>,
    /// Lease TTL in seconds (default 30) — how long a taker waits before
    /// stealing a dead server's volumes. The epoch, not this clock,
    /// fences the dead server's writes.
    lease_ttl_s: Option<u64>,
    /// With refs_kind = "object": "per_commit" (default — every ref
    /// change is a conditional PUT) or "cadence" (design D-M4 — head and
    /// rpo export on refs_export_interval_s, snapshots/pins immediately,
    /// and the serve-time epoch-filtered scan closes the crash gap).
    refs_export: Option<String>,
    /// Seconds between cadenced exports (default 300).
    refs_export_interval_s: Option<u64>,
    #[serde(default)]
    store: StoreConfig,
    #[serde(default)]
    volumes: Vec<VolumeConfig>,
    #[serde(default)]
    rpo: RpoConfig,
    /// Deprecated alias for `[api.auth] mode = "static"` +
    /// `token_path` — kept so pre-OIDC configs work unchanged.
    api_auth_token_path: Option<String>,
    #[serde(default)]
    api: ApiSection,
    #[serde(default)]
    nbd: NbdSection,
    /// Engine store-fetch tuning (issue #97 items 4–5). Absent = the
    /// engine's defaults.
    #[serde(default)]
    engine: EngineSection,
    /// Retention thinning (design §6.2, task 15f). Absent = no pass —
    /// deletion stays an operator action, exactly as before.
    retention: Option<RetentionSection>,
}

#[derive(Debug, Default, Deserialize)]
struct EngineSection {
    /// Store GETs one NBD read keeps in flight (default 16).
    read_fanout: Option<usize>,
    /// Node fetches / existence probes a tree walk keeps in flight
    /// (default 32).
    walk_concurrency: Option<usize>,
    /// Data GETs all of a volume's reads together keep in flight — its
    /// head, its snapshot/rpo/origin exports, and API snapshot reads and
    /// diffs share one pool (default 256, clamped to 1..=65536). The two
    /// fan-outs above are clamped to 256.
    max_read_gets: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct RetentionSection {
    /// §6.2's `publishRetentionWindow`: publishes younger than this are
    /// kept regardless.
    #[serde(default = "default_retention_window_s")]
    window_s: u64,
    /// Seconds between passes.
    #[serde(default = "default_retention_interval_s")]
    interval_s: u64,
    /// A prepared session with no durable activity for this long is
    /// collected as fenced-abandoned (#75 C7).
    #[serde(default = "default_prepared_ttl_s")]
    prepared_ttl_s: u64,
}

fn default_retention_window_s() -> u64 {
    24 * 60 * 60
}
fn default_retention_interval_s() -> u64 {
    60 * 60
}
fn default_prepared_ttl_s() -> u64 {
    7 * 24 * 60 * 60
}

#[derive(Debug, Default, Deserialize)]
struct NbdSection {
    /// STARTTLS (auth design §3.1). Absent = plaintext (dev/UDS).
    tls: Option<NbdTlsConfig>,
    /// Export ACLs (§3.3); only meaningful with TLS on. Empty table +
    /// TLS = deny all (fail closed).
    #[serde(default)]
    acl: Vec<NbdAclConfig>,
    /// Handshake deadline (greeting through option haggling + TLS).
    #[serde(default = "default_negotiation_timeout")]
    negotiation_timeout_s: u64,
    /// Requests serviced concurrently per connection (issue #97 item 3),
    /// replying out of order by cookie. Absent = 1, one at a time.
    #[serde(default)]
    max_in_flight: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct NbdTlsConfig {
    /// Only "forced" is offered: pre-TLS, every option except
    /// STARTTLS/ABORT gets REP_ERR_TLS_REQD, so nothing leaks cleartext.
    #[serde(default = "default_tls_mode")]
    mode: String,
    /// Server certificate chain + key (short-lived, rotated in place —
    /// re-read on change, no restart).
    cert_path: String,
    key_path: String,
    /// Trust anchor for client certs — single file (may itself hold a
    /// PEM bundle).
    client_ca_path: Option<String>,
    /// Trust anchors as a list (auth design §3.2: skynbd's own enrollment
    /// root and an external CA can coexist). Concatenated with
    /// `client_ca_path`; at least one of the two must be set.
    #[serde(default)]
    client_ca_paths: Vec<String>,
    /// Seconds between rotation checks (file mtime).
    #[serde(default = "default_tls_reload")]
    reload_interval_s: u64,
}

impl NbdTlsConfig {
    fn ca_paths(&self) -> Vec<&String> {
        self.client_ca_path
            .iter()
            .chain(self.client_ca_paths.iter())
            .collect()
    }
}

#[derive(Debug, Deserialize)]
struct NbdAclConfig {
    /// Glob on the client cert identity (URI SAN > DNS SAN > CN).
    identity: String,
    exports: Vec<String>,
    #[serde(default)]
    write: bool,
}

fn default_tls_mode() -> String {
    "forced".into()
}

fn default_tls_reload() -> u64 {
    30
}

fn default_negotiation_timeout() -> u64 {
    30
}

/// Build the TLS context from PEM files and spawn the mtime-watch reload
/// task (auth design §3.1: hot reload is mandatory with short-lived
/// certs). Returns `None` when TLS is off.
fn build_nbd_tls(cfg: &NbdSection) -> Option<Arc<skynbd_nbd::TlsContext>> {
    let tls = cfg.tls.as_ref()?;
    assert!(
        tls.mode == "forced",
        "[nbd.tls] mode must be \"forced\" (or omit [nbd.tls] for plaintext)"
    );
    assert!(
        !tls.ca_paths().is_empty(),
        "[nbd.tls] needs client_ca_path or client_ca_paths"
    );
    let read_all = |paths: &NbdTlsConfig| -> std::io::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
        let mut ca = Vec::new();
        for p in paths.ca_paths() {
            ca.extend(std::fs::read(p)?);
            ca.push(b'\n');
        }
        Ok((
            std::fs::read(&paths.cert_path)?,
            std::fs::read(&paths.key_path)?,
            ca,
        ))
    };
    let (cert, key, ca) = read_all(tls).expect("read [nbd.tls] PEM files");
    let config = skynbd_nbd::tls::build_server_config(&cert, &key, &ca).expect("build TLS config");
    let acl = cfg
        .acl
        .iter()
        .map(|r| skynbd_nbd::AclRule {
            identity: r.identity.clone(),
            exports: r.exports.clone(),
            write: r.write,
        })
        .collect();
    let ctx = Arc::new(skynbd_nbd::TlsContext::new(config, acl));
    if cfg.acl.is_empty() {
        tracing::warn!("[nbd.tls] on with empty [[nbd.acl]]: all clients will be denied");
    }

    // Rotation watcher: rebuild + swap when any PEM's mtime moves; a bad
    // intermediate state (mid-rotation) is retried next tick.
    let watcher_ctx = Arc::clone(&ctx);
    let paths = NbdTlsConfig {
        mode: tls.mode.clone(),
        cert_path: tls.cert_path.clone(),
        key_path: tls.key_path.clone(),
        client_ca_path: tls.client_ca_path.clone(),
        client_ca_paths: tls.client_ca_paths.clone(),
        reload_interval_s: tls.reload_interval_s,
    };
    tokio::spawn(async move {
        let mtimes = |p: &NbdTlsConfig| {
            [&p.cert_path, &p.key_path]
                .into_iter()
                .chain(p.ca_paths())
                .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
                .collect::<Vec<_>>()
        };
        let mut last = mtimes(&paths);
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(
            paths.reload_interval_s.max(1),
        ));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let now = mtimes(&paths);
            if now == last {
                continue;
            }
            match read_all(&paths) {
                Ok((cert, key, ca)) => {
                    match skynbd_nbd::tls::build_server_config(&cert, &key, &ca) {
                        Ok(config) => {
                            watcher_ctx.swap(config).await;
                            last = now;
                            tracing::info!("NBD TLS material reloaded");
                        }
                        Err(e) => tracing::warn!(error = %e, "TLS reload failed; keeping old"),
                    }
                }
                Err(e) => tracing::warn!(error = %e, "TLS files unreadable; keeping old"),
            }
        }
    });
    tracing::info!("NBD forced-TLS enabled (mTLS, {} ACL rules)", cfg.acl.len());
    Some(ctx)
}

#[derive(Debug, Default, Deserialize)]
struct ApiSection {
    #[serde(default)]
    auth: AuthConfig,
    /// `[api.enroll]` (auth design §3.2 amendment): mTLS client-cert
    /// issuance for token-verified identities. Absent = endpoint 404s.
    enroll: Option<EnrollConfig>,
}

#[derive(Debug, Deserialize)]
struct EnrollConfig {
    /// skynbd's own signing root. The cert must also appear in the NBD
    /// trust bundle (`[nbd.tls] client_ca_paths`) for enrolled certs to
    /// be accepted. Re-read per issuance, so rotation needs no restart.
    ca_cert_path: String,
    ca_key_path: String,
    /// SPIFFE trust domain: issued SANs are
    /// `spiffe://{trust_domain}/{path derived from the token's sub}`.
    trust_domain: String,
    /// Cert lifetime. Short-lived is the point (§3.2), but size it to
    /// ride out realistic control-plane outages (§9.4): renewal needs
    /// the endpoint, in-flight IO does not.
    #[serde(default = "default_enroll_ttl")]
    ttl_s: u64,
}

fn default_enroll_ttl() -> u64 {
    3600
}

/// Build the cert issuer, failing fast on unusable CA material (a broken
/// enrollment path should be a startup error, not a runtime 500).
fn build_api_enroll(cfg: &Config) -> Option<skynbd_api::CertIssuer> {
    let e = cfg.api.enroll.as_ref()?;
    assert!(e.ttl_s >= 60, "[api.enroll] ttl_s must be >= 60");
    assert!(
        !e.trust_domain.is_empty() && !e.trust_domain.contains('/'),
        "[api.enroll] trust_domain must be a bare domain name"
    );
    let issuer = skynbd_api::CertIssuer {
        ca_cert_path: e.ca_cert_path.clone().into(),
        ca_key_path: e.ca_key_path.clone().into(),
        trust_domain: e.trust_domain.clone(),
        ttl: std::time::Duration::from_secs(e.ttl_s),
    };
    if let Err(err) = issuer.validate() {
        panic!("[api.enroll] CA material unusable: {err}");
    }
    tracing::info!(
        trust_domain = %e.trust_domain,
        ttl_s = e.ttl_s,
        "cert enrollment enabled"
    );
    Some(issuer)
}

/// `[api.auth]` (auth design §4). `mode` is normally inferred: issuers
/// configured → "oidc"; a token path → "static"; neither → "disabled".
/// Set it explicitly to catch config mistakes (e.g. `mode = "oidc"` with
/// no issuers is a startup error instead of silently-disabled auth).
#[derive(Debug, Default, Deserialize)]
struct AuthConfig {
    mode: Option<String>,
    /// Static mode: file whose trimmed contents are the bearer token.
    token_path: Option<String>,
    #[serde(default)]
    issuers: Vec<IssuerConfig>,
}

#[derive(Debug, Deserialize)]
struct IssuerConfig {
    issuer: String,
    audience: String,
    /// Explicit JWKS endpoint; omit to use OIDC discovery.
    jwks_url: Option<String>,
    /// PEM file with an extra root CA for this issuer's HTTPS
    /// JWKS/discovery fetches (CSI design 6c) — e.g. the Kubernetes
    /// cluster CA when the issuer is the API server. Omit for public
    /// issuers with system-trusted certs.
    tls_ca_path: Option<String>,
    /// Claim carrying role names for `role` bindings (e.g. Zitadel's
    /// `urn:zitadel:iam:org:project:roles`).
    roles_claim: Option<String>,
    #[serde(default)]
    bindings: Vec<BindingConfig>,
}

#[derive(Debug, Deserialize)]
struct BindingConfig {
    /// Glob on the token's `sub` claim.
    sub: Option<String>,
    /// Glob matched against the roles claim's values.
    role: Option<String>,
    #[serde(default = "default_all")]
    volumes: Vec<String>,
    ops: Vec<String>,
}

fn default_all() -> Vec<String> {
    vec!["*".into()]
}

/// Resolve the auth mode from config (explicit `mode` wins; else infer).
fn build_api_auth(cfg: &Config) -> skynbd_api::ApiAuth {
    use skynbd_api::{ApiAuth, Binding, OidcAuth, TrustedIssuer};
    let auth = &cfg.api.auth;
    let token_path = auth
        .token_path
        .as_ref()
        .or(cfg.api_auth_token_path.as_ref());
    let mode = auth.mode.as_deref().unwrap_or({
        if !auth.issuers.is_empty() {
            "oidc"
        } else if token_path.is_some() {
            "static"
        } else {
            "disabled"
        }
    });
    match mode {
        "disabled" => ApiAuth::Disabled,
        "static" => {
            let p = token_path.expect("[api.auth] mode=static requires token_path");
            let token = std::fs::read_to_string(p)
                .expect("read api auth token file")
                .trim()
                .to_string();
            tracing::info!("API bearer-token auth enabled (static)");
            ApiAuth::Static(token)
        }
        "oidc" => {
            assert!(
                !auth.issuers.is_empty(),
                "[api.auth] mode=oidc requires at least one [[api.auth.issuers]]"
            );
            let issuers = auth
                .issuers
                .iter()
                .map(|i| {
                    for b in &i.bindings {
                        assert!(
                            b.sub.is_some() || b.role.is_some(),
                            "[api.auth] binding needs sub or role (issuer {})",
                            i.issuer
                        );
                        assert!(
                            b.role.is_none() || i.roles_claim.is_some(),
                            "[api.auth] role binding needs roles_claim (issuer {})",
                            i.issuer
                        );
                    }
                    TrustedIssuer {
                        issuer: i.issuer.clone(),
                        audience: i.audience.clone(),
                        jwks_url: i.jwks_url.clone(),
                        roles_claim: i.roles_claim.clone(),
                        tls_ca_pem: i.tls_ca_path.as_ref().map(|p| {
                            std::fs::read(p).unwrap_or_else(|e| {
                                panic!("[api.auth] issuer {} tls_ca_path {p:?}: {e}", i.issuer)
                            })
                        }),
                        bindings: i
                            .bindings
                            .iter()
                            .map(|b| Binding {
                                sub: b.sub.clone(),
                                role: b.role.clone(),
                                volumes: b.volumes.clone(),
                                ops: b.ops.clone(),
                            })
                            .collect(),
                    }
                })
                .collect();
            tracing::info!(issuers = auth.issuers.len(), "API OIDC auth enabled");
            ApiAuth::Oidc(OidcAuth::new(issuers))
        }
        other => panic!("[api.auth] unknown mode {other:?}"),
    }
}

#[derive(Debug, Default, Deserialize)]
struct RpoConfig {
    /// Seconds between rpo advances; 0 disables (design §9.3).
    #[serde(default)]
    interval_s: u64,
}

const DEFAULT_LEASE_TTL_S: u64 = 30;
const DEFAULT_REFS_EXPORT_INTERVAL_S: u64 = 300;

/// $HOSTNAME is the pod name under Kubernetes — a reasonable default
/// identity. A steal after rename costs one TTL wait, nothing more.
fn default_node_id() -> String {
    std::env::var("HOSTNAME").unwrap_or_else(|_| "skynbdd".to_string())
}

fn default_listen() -> String {
    "127.0.0.1:10809".into()
}

fn default_api_listen() -> String {
    "127.0.0.1:8080".into()
}

#[derive(Debug, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum StoreConfig {
    #[default]
    Memory,
    // Fields are read only by the s3-feature build of `build_store`.
    #[cfg_attr(not(feature = "s3"), allow(dead_code))]
    S3 {
        /// Omit to defer to the environment (`AWS_ENDPOINT_URL_S3`, or the
        /// AWS default for the resolved region). Required with static keys.
        endpoint: Option<String>,
        bucket: String,
        /// Static credentials (dev/MinIO). Omit BOTH for ambient AWS auth
        /// (auth design §5): web identity / env keys / profile / IMDS via
        /// the SDK default chain — no secrets in this file.
        access_key: Option<String>,
        secret_key: Option<String>,
        /// Omit to defer to the environment (`AWS_REGION`, profile);
        /// static keys fall back to "us-east-1" as before.
        region: Option<String>,
        /// Path-style addressing (MinIO/moto). Set false for AWS
        /// virtual-hosted buckets.
        #[serde(default = "default_true")]
        force_path_style: bool,
        /// Create the bucket at startup (dev convenience). Set false in
        /// production: a least-privilege role has no `s3:CreateBucket`.
        #[serde(default = "default_true")]
        create_bucket: bool,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct VolumeConfig {
    name: String,
    size: u64,
    /// "attached" (default): writable NBD head. "lineage" (design D14):
    /// no head export; content arrives via the ingestion plane (§9A).
    #[serde(default = "default_kind")]
    kind: String,
}

fn default_kind() -> String {
    "attached".into()
}

/// Deterministic volume id from its configured name, so restarts reopen
/// the same volume without a registry (the M3 control plane replaces
/// this with proper volume metadata).
async fn build_store(cfg: &StoreConfig) -> Arc<dyn ObjectStore> {
    match cfg {
        StoreConfig::Memory => Arc::new(MemoryStore::new()),
        #[cfg(feature = "s3")]
        StoreConfig::S3 {
            endpoint,
            bucket,
            access_key,
            secret_key,
            region,
            force_path_style,
            create_bucket,
        } => {
            let s = match (access_key, secret_key) {
                (Some(ak), Some(sk)) => {
                    let endpoint = endpoint
                        .as_deref()
                        .expect("[store] endpoint is required with static keys");
                    let region = region.as_deref().unwrap_or("us-east-1");
                    skynbd_store::S3Store::new(endpoint, bucket, ak, sk, region, *force_path_style)
                        .await
                }
                (None, None) => {
                    tracing::info!("S3 store using ambient AWS credentials");
                    skynbd_store::S3Store::from_env(
                        endpoint.as_deref(),
                        bucket,
                        region.as_deref(),
                        *force_path_style,
                    )
                    .await
                }
                _ => panic!("[store] access_key and secret_key must be set together"),
            };
            if *create_bucket {
                s.ensure_bucket()
                    .await
                    .expect("bucket create/verify failed");
            }
            Arc::new(s)
        }
        #[cfg(not(feature = "s3"))]
        StoreConfig::S3 { .. } => {
            panic!("this skynbdd was built without the 's3' feature");
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        // Colour only for a human at a terminal. Under kubelet these logs
        // are a machine-readable record — escape codes there break every
        // `grep "field=value"`, ours included, and land verbatim in
        // whatever collects them.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stdout()))
        .init();

    let path = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: skynbdd <config.toml>");
        std::process::exit(2);
    });
    let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).expect("read config"))
        .expect("parse config");

    let store = build_store(&cfg.store).await;
    let refs_kind = cfg.refs_kind.as_deref().unwrap_or(match &cfg.refs_path {
        Some(_) => "redb",
        None => "memory",
    });
    // Ownership (design D-M3) rides with "object" refs: shared refs are
    // the only mode where a second server can contend for a volume.
    let lease_cfg = (refs_kind == "object").then(|| skynbd_refs::LeaseConfig {
        holder: cfg.node_id.clone().unwrap_or_else(default_node_id),
        ttl_ms: cfg.lease_ttl_s.unwrap_or(DEFAULT_LEASE_TTL_S) * 1000,
    });
    let refs: Arc<dyn RefStore> = match refs_kind {
        "redb" => {
            let p = cfg
                .refs_path
                .as_deref()
                .expect("refs_kind = \"redb\" requires refs_path");
            Arc::new(RedbRefStore::open(p).expect("open refs"))
        }
        "memory" => Arc::new(MemoryRefStore::new()),
        "object" => {
            ObjectRefStore::probe(&store)
                .await
                .expect("refs_kind = \"object\"");
            let lc = lease_cfg.clone().expect("object mode always has a lease");
            tracing::info!(node_id = %lc.holder, ttl_ms = lc.ttl_ms, "volume ownership on");
            let authority = ObjectRefStore::new(Arc::clone(&store)).with_owner(lc);
            match cfg.refs_export.as_deref().unwrap_or("per_commit") {
                // Every ref operation is a conditional PUT: zero-scan
                // recovery, ~$26/volume/month at 2 fsync/s.
                "per_commit" => Arc::new(authority),
                // The D-M4 tier: head/rpo stay local between exports;
                // the serve-time recovery scan closes the gap.
                "cadence" => {
                    let c = Arc::new(skynbd_refs::CadencedRefStore::new(authority));
                    let interval = cfg
                        .refs_export_interval_s
                        .unwrap_or(DEFAULT_REFS_EXPORT_INTERVAL_S);
                    tracing::info!(interval_s = interval, "cadenced refs export on");
                    let timer = Arc::clone(&c);
                    tokio::spawn(async move {
                        let mut tick =
                            tokio::time::interval(std::time::Duration::from_secs(interval.max(1)));
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        loop {
                            tick.tick().await;
                            for (vol, e) in timer.export_all().await {
                                tracing::warn!(volume = %vol, error = %e, "refs export failed");
                            }
                        }
                    });
                    c
                }
                other => panic!("refs_export {other:?}: expected \"per_commit\" or \"cadence\""),
            }
        }
        other => panic!("refs_kind {other:?}: expected \"redb\", \"memory\" or \"object\""),
    };
    tracing::info!(refs = refs_kind, "refs store");

    let registry = Arc::new(ExportRegistry::new());
    // All serving — config volumes, catalog reloads, API-driven creates —
    // goes through the provisioner (head export, snapshot-export sync,
    // RPO ticker; design §12.1).
    let mut provisioner = Provisioner::new(
        Arc::clone(&store),
        Arc::clone(&refs),
        Arc::clone(&registry),
        cfg.rpo.interval_s,
    );
    if let Some(lc) = &lease_cfg {
        provisioner = provisioner.with_lease(lc.clone());
    }
    let mut tuning = skynbd_engine::EngineConfig::default();
    if let Some(n) = cfg.engine.read_fanout {
        tuning.read_fanout = n;
    }
    if let Some(n) = cfg.engine.walk_concurrency {
        tuning.walk_concurrency = n;
    }
    if let Some(n) = cfg.engine.max_read_gets {
        tuning.max_read_gets = n;
    }
    provisioner = provisioner.with_engine_tuning(tuning);
    let provisioner = Arc::new(provisioner);
    // The idle-volume renewal timer (busy volumes renew on every commit
    // through the fenced store). ttl/3 keeps two missed ticks from
    // costing the lease.
    if let Some(lc) = &lease_cfg {
        let p = Arc::clone(&provisioner);
        let period = std::time::Duration::from_millis((lc.ttl_ms / 3).max(1000));
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(period);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let _ = p.renew_leases().await;
            }
        });
    }
    // Retention thinning cadence (design §6.2, task 15f): thin, then
    // sweep, for every volume this server is SERVING. Node-held
    // lineages answer 409 at the refs backend (their agents drive the
    // pass with the fence epoch instead) — that is a skip, not an
    // error, and it is the fencing model working. Released cataloged
    // volumes (in the catalog, not currently served) are visited by
    // neither driver; closing that gap without dragging
    // serve_cataloged's lease + origin-export side effects into a
    // maintenance sweep is scoped to 15f part B.
    if let Some(rc) = &cfg.retention {
        let policy = skynbd_api::retention::ThinPolicy {
            publish_window_ms: rc.window_s * 1000,
            prepared_ttl_ms: rc.prepared_ttl_s * 1000,
        };
        let interval = rc.interval_s.max(1);
        tracing::info!(
            window_s = rc.window_s,
            interval_s = interval,
            "retention thinning on"
        );
        let p = Arc::clone(&provisioner);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(interval));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let handles: Vec<_> = p
                    .volumes
                    .read()
                    .await
                    .iter()
                    .map(|(n, h)| (n.clone(), h.clone()))
                    .collect();
                for (name, h) in handles {
                    match skynbd_api::thin_pass(&h, policy, None).await {
                        Ok(r)
                            if !r.deleted_generations.is_empty()
                                || !r.swept_sessions.is_empty() =>
                        {
                            tracing::info!(volume = %name,
                                deleted = r.deleted_generations.len(),
                                sessions = r.swept_sessions.len(),
                                gc = r.gc, "retention pass");
                        }
                        Ok(_) => {}
                        Err(e) if e.status() == axum::http::StatusCode::CONFLICT => {
                            tracing::debug!(volume = %name,
                                "retention pass skipped (node-held)");
                        }
                        Err(e) => {
                            tracing::warn!(volume = %name, status = %e.status(),
                                "retention pass failed");
                        }
                    }
                }
            }
        });
    }

    for v in &cfg.volumes {
        provisioner
            .serve_config_volume(&VolumeSpec {
                name: v.name.clone(),
                size: v.size,
                kind: v.kind.clone(),
                // Clones are API-created only; config volumes never are.
                from: None,
            })
            .await
            .unwrap_or_else(|e| panic!("serve volume {}: {e}", v.name));
    }
    // Dynamically-created volumes from previous runs (refs-store catalog).
    match provisioner.load_catalog().await {
        Ok(0) => {}
        Ok(n) => tracing::info!(volumes = n, "reloaded dynamic volumes from catalog"),
        Err(e) => panic!("load volume catalog: {e}"),
    }
    let volume_count = provisioner.volumes.read().await.len();

    enum NbdListener {
        Tcp(tokio::net::TcpListener),
        Unix(tokio::net::UnixListener),
    }
    // `unix:/path` binds a Unix domain socket (design §15 M10a); the socket
    // file is recreated on start and access control is its file mode.
    let listener = if let Some(path) = cfg.nbd_listen.strip_prefix("unix:") {
        let _ = std::fs::remove_file(path);
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).expect("create socket dir");
        }
        let l = tokio::net::UnixListener::bind(path).expect("bind NBD unix listener");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o660))
            .expect("socket permissions");
        NbdListener::Unix(l)
    } else {
        NbdListener::Tcp(
            tokio::net::TcpListener::bind(&cfg.nbd_listen)
                .await
                .expect("bind NBD listener"),
        )
    };

    let api_state = Arc::new(ApiState {
        volumes: Arc::clone(&provisioner.volumes),
        registry: Arc::clone(&registry),
        auth: build_api_auth(&cfg),
        enroll: build_api_enroll(&cfg),
        provisioner: Some(Arc::clone(&provisioner)),
        snap_requests: Default::default(),
    });
    let api_listener = tokio::net::TcpListener::bind(&cfg.api_listen)
        .await
        .expect("bind API listener");
    let api = tokio::spawn(async move {
        axum::serve(api_listener, skynbd_api::router(api_state))
            .await
            .expect("api server");
    });

    tracing::info!(nbd = %cfg.nbd_listen, api = %cfg.api_listen,
        volumes = volume_count, "skynbdd serving");

    let mut nbd_server = Server::new(Arc::clone(&registry))
        .negotiation_timeout(std::time::Duration::from_secs(
            cfg.nbd.negotiation_timeout_s.max(1),
        ))
        .max_in_flight(cfg.nbd.max_in_flight.unwrap_or(1));
    if let Some(tls) = build_nbd_tls(&cfg.nbd) {
        nbd_server = nbd_server.with_tls(tls);
    }
    let server = match listener {
        NbdListener::Tcp(l) => tokio::spawn(nbd_server.run(l)),
        NbdListener::Unix(l) => tokio::spawn(nbd_server.run_unix(l)),
    };

    tokio::signal::ctrl_c().await.expect("signal handler");
    tracing::info!("shutting down: checkpointing volumes");
    server.abort();
    api.abort();
    provisioner.checkpoint_all().await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store_config(body: &str) -> StoreConfig {
        let cfg: Config = toml::from_str(&format!("[store]\n{body}")).unwrap();
        cfg.store
    }

    #[test]
    fn empty_config_defaults_to_memory_store() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(matches!(cfg.store, StoreConfig::Memory));
        assert_eq!(cfg.nbd_listen, "127.0.0.1:10809");
        assert_eq!(cfg.api_listen, "127.0.0.1:8080");
    }

    /// Keyless [store] (auth design §5): keys/endpoint/region all optional,
    /// path-style and bucket creation default on.
    #[test]
    fn keyless_s3_config_parses_with_defaults() {
        let StoreConfig::S3 {
            endpoint,
            bucket,
            access_key,
            secret_key,
            region,
            force_path_style,
            create_bucket,
        } = store_config("kind = \"s3\"\nbucket = \"b\"")
        else {
            panic!("expected s3 store");
        };
        assert_eq!(bucket, "b");
        assert!(endpoint.is_none() && access_key.is_none() && secret_key.is_none());
        assert!(region.is_none());
        assert!(force_path_style);
        assert!(create_bucket);
    }

    #[test]
    fn static_s3_config_parses_with_overrides() {
        let StoreConfig::S3 {
            endpoint,
            access_key,
            secret_key,
            force_path_style,
            create_bucket,
            ..
        } = store_config(
            "kind = \"s3\"\nbucket = \"b\"\nendpoint = \"http://127.0.0.1:9000\"\n\
             access_key = \"ak\"\nsecret_key = \"sk\"\n\
             force_path_style = false\ncreate_bucket = false",
        )
        else {
            panic!("expected s3 store");
        };
        assert_eq!(endpoint.as_deref(), Some("http://127.0.0.1:9000"));
        assert_eq!(access_key.as_deref(), Some("ak"));
        assert_eq!(secret_key.as_deref(), Some("sk"));
        assert!(!force_path_style);
        assert!(!create_bucket);
    }

    #[test]
    fn api_auth_oidc_config_parses_and_builds() {
        let cfg: Config = toml::from_str(
            r#"
[[api.auth.issuers]]
issuer = "https://oidc.example"
audience = "skynbd-api"
roles_claim = "urn:zitadel:iam:org:project:roles"

[[api.auth.issuers.bindings]]
sub = "system:serviceaccount:prod:skynbd-agent"
volumes = ["db-*"]
ops = ["ingest", "snapshot"]

[[api.auth.issuers.bindings]]
role = "skynbd-operator"
ops = ["*"]
"#,
        )
        .unwrap();
        // Mode inferred from the issuer list; default volumes glob is "*".
        assert!(matches!(build_api_auth(&cfg), skynbd_api::ApiAuth::Oidc(_)));
        assert_eq!(cfg.api.auth.issuers[0].bindings[1].volumes, vec!["*"]);
    }

    #[test]
    fn legacy_token_path_still_selects_static_mode() {
        let path = std::env::temp_dir().join(format!("skynbd-test-token-{}", std::process::id()));
        std::fs::write(&path, "tok\n").unwrap();
        let cfg: Config = toml::from_str(&format!(
            "api_auth_token_path = {:?}\n",
            path.to_str().unwrap()
        ))
        .unwrap();
        match build_api_auth(&cfg) {
            skynbd_api::ApiAuth::Static(t) => assert_eq!(t, "tok"),
            _ => panic!("expected static mode from legacy key"),
        }
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn issuer_tls_ca_path_reads_pem() {
        let dir = std::env::temp_dir().join(format!("skynbd-tlsca-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ca = dir.join("cluster-ca.pem");
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = rcgen::CertificateParams::new(Vec::<String>::new())
            .unwrap()
            .self_signed(&key)
            .unwrap();
        std::fs::write(&ca, cert.pem()).unwrap();

        let toml_src = format!(
            "[[api.auth.issuers]]\nissuer = \"https://kubernetes.default.svc\"\naudience = \"skynbd-api\"\ntls_ca_path = {ca:?}\n[[api.auth.issuers.bindings]]\nsub = \"*\"\nops = [\"read\"]\n"
        );
        let cfg: Config = toml::from_str(&toml_src).unwrap();
        assert!(matches!(build_api_auth(&cfg), skynbd_api::ApiAuth::Oidc(_)));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    #[should_panic(expected = "tls_ca_path")]
    fn issuer_tls_ca_path_missing_file_panics() {
        let cfg: Config = toml::from_str(
            "[[api.auth.issuers]]\nissuer = \"https://x\"\naudience = \"a\"\ntls_ca_path = \"/nonexistent-ca.pem\"\n[[api.auth.issuers.bindings]]\nsub = \"*\"\nops = [\"read\"]\n",
        )
        .unwrap();
        build_api_auth(&cfg);
    }

    #[test]
    fn auth_defaults_to_disabled() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(matches!(
            build_api_auth(&cfg),
            skynbd_api::ApiAuth::Disabled
        ));
    }

    #[test]
    #[should_panic(expected = "requires at least one")]
    fn explicit_oidc_mode_without_issuers_panics() {
        let cfg: Config = toml::from_str("[api.auth]\nmode = \"oidc\"\n").unwrap();
        build_api_auth(&cfg);
    }

    #[test]
    fn nbd_tls_config_parses_with_defaults() {
        let cfg: Config = toml::from_str(
            r#"
[nbd.tls]
cert_path = "server.pem"
key_path = "server.key"
client_ca_path = "ca.pem"

[[nbd.acl]]
identity = "spiffe://prod/agent/*"
exports = ["*/head", "*/snap/*"]
write = true

[[nbd.acl]]
identity = "spiffe://prod/restore/*"
exports = ["*/snap/*"]
"#,
        )
        .unwrap();
        let tls = cfg.nbd.tls.as_ref().unwrap();
        assert_eq!(tls.mode, "forced");
        assert_eq!(tls.reload_interval_s, 30);
        assert_eq!(cfg.nbd.negotiation_timeout_s, 30);
        assert_eq!(cfg.nbd.max_in_flight, None, "absent = the serial default");
        assert_eq!(cfg.nbd.acl.len(), 2);
        assert!(cfg.nbd.acl[0].write);
        assert!(!cfg.nbd.acl[1].write); // write defaults off
    }

    #[test]
    fn engine_fanout_parses_and_defaults() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.engine.read_fanout, None);
        assert_eq!(cfg.engine.walk_concurrency, None);
        assert_eq!(cfg.engine.max_read_gets, None);
        let cfg: Config =
            toml::from_str("[engine]\nread_fanout = 4\nwalk_concurrency = 8\nmax_read_gets = 64\n")
                .unwrap();
        assert_eq!(cfg.engine.read_fanout, Some(4));
        assert_eq!(cfg.engine.walk_concurrency, Some(8));
        assert_eq!(cfg.engine.max_read_gets, Some(64));
    }

    #[test]
    fn nbd_max_in_flight_parses() {
        let cfg: Config = toml::from_str("[nbd]\nmax_in_flight = 32\n").unwrap();
        assert_eq!(cfg.nbd.max_in_flight, Some(32));
    }

    #[test]
    fn nbd_tls_trust_bundle_may_be_a_list() {
        let cfg: Config = toml::from_str(
            r#"
[nbd.tls]
cert_path = "server.pem"
key_path = "server.key"
client_ca_paths = ["skynbd-root.pem", "spire-bundle.pem"]
"#,
        )
        .unwrap();
        let tls = cfg.nbd.tls.as_ref().unwrap();
        assert_eq!(tls.ca_paths(), vec!["skynbd-root.pem", "spire-bundle.pem"]);

        // Single path and list concatenate.
        let cfg: Config = toml::from_str(
            r#"
[nbd.tls]
cert_path = "s.pem"
key_path = "s.key"
client_ca_path = "a.pem"
client_ca_paths = ["b.pem"]
"#,
        )
        .unwrap();
        assert_eq!(
            cfg.nbd.tls.as_ref().unwrap().ca_paths(),
            vec!["a.pem", "b.pem"]
        );
    }

    #[test]
    fn enroll_config_parses_and_builds() {
        // Generated CA on disk: build_api_enroll must accept it.
        let dir = std::env::temp_dir().join(format!("skynbd-enroll-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = rcgen::KeyPair::generate().unwrap();
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let cert = params.self_signed(&key).unwrap();
        let cert_path = dir.join("ca.pem");
        let key_path = dir.join("ca.key");
        std::fs::write(&cert_path, cert.pem()).unwrap();
        std::fs::write(&key_path, key.serialize_pem()).unwrap();

        let cfg: Config = toml::from_str(&format!(
            "[api.enroll]\nca_cert_path = {:?}\nca_key_path = {:?}\ntrust_domain = \"prod\"\n",
            cert_path, key_path
        ))
        .unwrap();
        assert_eq!(cfg.api.enroll.as_ref().unwrap().ttl_s, 3600);
        let issuer = build_api_enroll(&cfg).unwrap();
        assert_eq!(issuer.trust_domain, "prod");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enroll_absent_disables_endpoint() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(build_api_enroll(&cfg).is_none());
    }

    #[test]
    #[should_panic(expected = "CA material unusable")]
    fn enroll_with_missing_ca_fails_at_startup() {
        let cfg: Config = toml::from_str(
            "[api.enroll]\nca_cert_path = \"/nonexistent.pem\"\nca_key_path = \"/nonexistent.key\"\ntrust_domain = \"prod\"\n",
        )
        .unwrap();
        build_api_enroll(&cfg);
    }

    #[test]
    fn nbd_tls_absent_means_plaintext() {
        let cfg: Config = toml::from_str("").unwrap();
        assert!(cfg.nbd.tls.is_none());
        assert!(build_nbd_tls(&cfg.nbd).is_none());
    }

    // Dispatch tests need the real S3 client; constructors resolve
    // credentials lazily, so nothing here touches the network (the
    // endpoint is a dead port and create_bucket is off).
    #[cfg(feature = "s3")]
    mod dispatch {
        use super::*;

        #[tokio::test]
        async fn ambient_mode_builds_without_keys_or_network() {
            let cfg = store_config(
                "kind = \"s3\"\nbucket = \"b\"\nendpoint = \"http://127.0.0.1:1\"\n\
                 region = \"us-east-1\"\ncreate_bucket = false",
            );
            let _store = build_store(&cfg).await;
        }

        #[tokio::test]
        async fn static_mode_builds_without_network() {
            let cfg = store_config(
                "kind = \"s3\"\nbucket = \"b\"\nendpoint = \"http://127.0.0.1:1\"\n\
                 access_key = \"ak\"\nsecret_key = \"sk\"\ncreate_bucket = false",
            );
            let _store = build_store(&cfg).await;
        }

        #[tokio::test]
        #[should_panic(expected = "set together")]
        async fn lone_access_key_is_rejected() {
            let cfg = store_config(
                "kind = \"s3\"\nbucket = \"b\"\nendpoint = \"http://127.0.0.1:1\"\n\
                 access_key = \"ak\"\ncreate_bucket = false",
            );
            build_store(&cfg).await;
        }

        #[tokio::test]
        #[should_panic(expected = "endpoint is required")]
        async fn static_keys_require_endpoint() {
            let cfg = store_config(
                "kind = \"s3\"\nbucket = \"b\"\n\
                 access_key = \"ak\"\nsecret_key = \"sk\"\ncreate_bucket = false",
            );
            build_store(&cfg).await;
        }
    }
}

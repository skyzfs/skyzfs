//! skynbd-csi-driver — SkyNBD volumes as Kubernetes PVs (auth design §9).
//!
//! One binary, role picked by SKYNBD_CSI_MODE:
//!   controller — CreateVolume/DeleteVolume against the control plane
//!                (paired with the external-provisioner sidecar);
//!   node       — stage/publish via nbd-client over forced-TLS mTLS,
//!                enrolling its client cert at startup and renewing it
//!                (paired with node-driver-registrar);
//!   all        — both (single-binary dev setups).
//!
//! Configuration is all environment variables (set by the Helm chart):
//!   CSI_ENDPOINT            unix:///csi/csi.sock
//!   SKYNBD_API_URL          http(s)://skynbd-control-plane:8080
//!   SKYNBD_TOKEN_FILE       projected ServiceAccount token (aud per §2)
//!   SKYNBD_NBD_HOST/PORT    NBD data-plane address (node mode)
//!   SKYNBD_NBD_SOCKET       node-local NBD Unix socket; replaces
//!                           HOST/PORT and implies plaintext (access
//!                           control is the socket's file mode)
//!   SKYNBD_NBD_TLS          "1" → enroll + mTLS (default), "0" → plaintext
//!   SKYNBD_TLS_HOSTNAME     cert hostname when it differs from NBD_HOST
//!   SKYNBD_CREDS_DIR        where enrolled PEMs live (/var/lib/skynbd-csi/creds)
//!   SKYNBD_STATE_DIR        device-binding state (/var/lib/skynbd-csi)
//!   SKYNBD_CERT_RENEW_S     re-enroll cadence (default 1800; ttl_s ≥ 2×)
//!   SKYNBD_DEFAULT_FSTYPE   ext4
//!   SKYNBD_CACHE_DIR        per-volume cache files for `mode: thin`
//!                           (unset → thin staging refused)
//!   SKYNBD_CACHE_BYTES      cache file size (default 1 GiB)
//!   SKYNBD_READ_CACHE_DIR   per-volume §7.1 read-cache files
//!                           (unset → no read cache; the fs mounts the shim)
//!   SKYNBD_READ_CACHE_BYTES read-cache file size (default 256 MiB)
//!   SKYNBD_CHUNK_BYTES      thin chunk == cache block (default 64 KiB;
//!                           must divide 1 MiB)
//!   SKYNBD_CYCLE_INTERVAL_S embedded agent cadence for thin volumes
//!                           (default 900)
//!   SKYNBD_FREEZE_BUDGET_MS fsfreeze watchdog budget (default 5000)
//!   NODE_ID                 spec.nodeName (downward API)

use std::sync::Arc;

pub mod csi {
    // Generated from the vendored csi.proto — its doc comments are the
    // spec's text, not ours to restyle.
    #[allow(clippy::all)]
    pub mod v1 {
        tonic::include_proto!("csi.v1");
    }
}

// Node mechanics, the embedded cycle, the lease handoff, and the
// control-plane client live in the shared agent crate (the resident
// workstation agent drives the same publish-only staging over its
// control socket); this bin keeps the CSI-specific gRPC surface and
// enrollment.
pub(crate) use skynbd_agent::nodemech as node;
pub(crate) use skynbd_agent::{agentcycle, cp, handoff};
mod creds;
mod runner;
mod services;
mod thin;

pub const DRIVER_NAME: &str = "csi.skynbd.io";

fn env(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}

fn env_or(key: &str, default: &str) -> String {
    env(key).unwrap_or_else(|| default.to_string())
}

/// A boolean setting: `true`/`false` or `1`/`0`, any case. Anything else
/// is a misconfiguration, refused at startup with the variable's name.
fn flag(key: &str, v: &str) -> bool {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => true,
        "false" | "0" => false,
        other => panic!("{key}: {other:?} is not true/false/1/0"),
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

    let mode = env_or("SKYNBD_CSI_MODE", "all");
    let endpoint = env_or("CSI_ENDPOINT", "unix:///csi/csi.sock");
    let socket_path = endpoint
        .strip_prefix("unix://")
        .unwrap_or(&endpoint)
        .to_string();
    let api_url = env("SKYNBD_API_URL").expect("SKYNBD_API_URL is required");
    let token_file = env("SKYNBD_TOKEN_FILE").map(std::path::PathBuf::from);
    let client = cp::Client::new(&api_url, token_file.clone());

    let with_controller = matches!(mode.as_str(), "controller" | "all");
    let with_node = matches!(mode.as_str(), "node" | "all");
    assert!(
        with_controller || with_node,
        "SKYNBD_CSI_MODE must be controller|node|all"
    );

    let router = tonic::transport::Server::builder().add_service(
        csi::v1::identity_server::IdentityServer::new(services::IdentitySvc { with_controller }),
    );
    let router = router.add_optional_service(with_controller.then(|| {
        csi::v1::controller_server::ControllerServer::new(services::ControllerSvc {
            cp: client.clone(),
        })
    }));
    let node_service = if with_node {
        // A node-local Unix socket carries no peer certificate, so mTLS
        // has nothing to bind to: it is plaintext by construction. Do
        // not silently downgrade an explicit SKYNBD_NBD_TLS=1 — refuse
        // the combination instead.
        let socket = env("SKYNBD_NBD_SOCKET").map(std::path::PathBuf::from);
        let tls = match (&socket, env("SKYNBD_NBD_TLS").as_deref()) {
            (Some(_), Some("1")) => panic!(
                "SKYNBD_NBD_SOCKET and SKYNBD_NBD_TLS=1 are incompatible: a \
                 Unix data path is guarded by the socket's file mode, not by \
                 client certificates"
            ),
            (Some(_), _) => false,
            (None, tls) => tls != Some("0"),
        };
        let creds_dir =
            std::path::PathBuf::from(env_or("SKYNBD_CREDS_DIR", "/var/lib/skynbd-csi/creds"));
        if tls {
            // Fail fast: a node that cannot enroll cannot attach anything.
            creds::enroll_into(&client, &creds_dir)
                .await
                .expect("initial cert enrollment");
            let renew = std::time::Duration::from_secs(
                env_or("SKYNBD_CERT_RENEW_S", "1800")
                    .parse()
                    .expect("SKYNBD_CERT_RENEW_S"),
            );
            tokio::spawn(creds::renew_loop(client.clone(), creds_dir.clone(), renew));
        }
        let cache_dir = env("SKYNBD_CACHE_DIR").map(std::path::PathBuf::from);
        let endpoint = match socket {
            Some(path) => node::NbdEndpoint::Unix(path),
            None => node::NbdEndpoint::Tcp {
                host: env("SKYNBD_NBD_HOST")
                    .expect("SKYNBD_NBD_HOST (or SKYNBD_NBD_SOCKET) is required in node mode"),
                port: env_or("SKYNBD_NBD_PORT", "10809")
                    .parse()
                    .expect("SKYNBD_NBD_PORT"),
                tls_hostname: env("SKYNBD_TLS_HOSTNAME"),
                creds_dir: tls.then_some(creds_dir),
            },
        };
        let cfg = node::NodeConfig {
            endpoint,
            sysfs_block: "/sys/block".into(),
            state_dir: env_or("SKYNBD_STATE_DIR", "/var/lib/skynbd-csi").into(),
            default_fstype: env_or("SKYNBD_DEFAULT_FSTYPE", "ext4"),
            cache_dir: cache_dir.clone(),
            cache_bytes: env_or("SKYNBD_CACHE_BYTES", &(1u64 << 30).to_string())
                .parse()
                .expect("SKYNBD_CACHE_BYTES"),
            chunk_bytes: env_or("SKYNBD_CHUNK_BYTES", &(64u64 << 10).to_string())
                .parse()
                .expect("SKYNBD_CHUNK_BYTES"),
            read_cache_dir: env("SKYNBD_READ_CACHE_DIR").map(std::path::PathBuf::from),
            read_cache_bytes: env_or("SKYNBD_READ_CACHE_BYTES", &(256u64 << 20).to_string())
                .parse()
                .expect("SKYNBD_READ_CACHE_BYTES"),
        };
        // §8.1 posture at node startup, log-only. The check has to run
        // HERE and not only in the config-file agent's `preflight`
        // subcommand: `no_space_timeout` is a node-wide module
        // parameter and Kubernetes is where the exhaustion ladder
        // actually gets walked, so a report only the workstation shape
        // ever sees is a report about the deployment that does not have
        // the hazard. Never fatal — the knob is the operator's call and
        // a node that refuses to start over it would be worse than one
        // running with a documented posture.
        let freeze_budget_ms: u64 = env_or("SKYNBD_FREEZE_BUDGET_MS", "5000")
            .parse()
            .expect("SKYNBD_FREEZE_BUDGET_MS");
        let posture = skynbd_agent::preflight::check_no_space_timeout(freeze_budget_ms);
        for n in &posture.notes {
            tracing::info!(posture = %n, "dm-thin exhaustion posture");
        }
        for w in &posture.warnings {
            tracing::warn!(warning = %w, "dm-thin exhaustion posture");
        }
        // Thin-stack support only where a cache dir is configured — a
        // node without one refuses `mode: thin` stages loudly.
        let thin_stack = cache_dir.as_ref().map(|_| {
            Arc::new(thin::DmThin::new().expect("device-mapper access")) as Arc<dyn thin::ThinStack>
        });
        // Embedded agent cycle (6h-2): continuous fs-consistent
        // publication into each staged thin volume's lineage volume.
        let cycle = cache_dir.as_ref().map(|_| {
            let mut dm_cycle = agentcycle::DmCycle::new(
                api_url.clone(),
                token_file.clone(),
                cfg.state_dir.join("agent"),
                cfg.chunk_bytes,
                std::time::Duration::from_secs(
                    env_or("SKYNBD_CYCLE_INTERVAL_S", "900")
                        .parse()
                        .expect("SKYNBD_CYCLE_INTERVAL_S"),
                ),
                freeze_budget_ms,
            );
            // D-M15 §4 watermark backstop: pool occupancy fraction above
            // which a publish cycle is promoted to an advance cycle.
            // "0.8" = advance when the pool is 80% provisioned; unset =
            // advance only at configured snapshots.
            if let Ok(w) = std::env::var("SKYNBD_ADVANCE_POOL_WATERMARK") {
                dm_cycle.advance_watermark =
                    Some(w.parse().expect("SKYNBD_ADVANCE_POOL_WATERMARK"));
            }
            // §6.2 retention (task 15f): arm the post-advance thinning
            // pass. The node drives it because the server's own cadence
            // is fenced off volumes this node leases. Unset = no
            // thinning from this node.
            if let Ok(w) = std::env::var("SKYNBD_PUBLISH_RETENTION_WINDOW_SECONDS") {
                dm_cycle.retention_window_s =
                    Some(w.parse().expect("SKYNBD_PUBLISH_RETENTION_WINDOW_SECONDS"));
            }
            // Issue #97: backfill upload pipelining — ops whose objects
            // upload together before the batch attaches in order. Unset =
            // 1, one op at a time.
            if let Ok(d) = std::env::var("SKYNBD_PUBLISH_PIPELINE_DEPTH") {
                dm_cycle.publish_pipeline_depth =
                    Some(d.parse().expect("SKYNBD_PUBLISH_PIPELINE_DEPTH"));
            }
            // Whether mixed attach/clear batches use the server's `apply`
            // call: auto (the features header's say), always, never.
            if let Ok(m) = std::env::var("SKYNBD_MIXED_APPLY") {
                dm_cycle.mixed_apply = Some(m.parse().expect("SKYNBD_MIXED_APPLY"));
            }
            // Overlap consecutive backfill batches above depth 1. Unset =
            // on; "false" keeps the batching but stages each batch only
            // after the previous one attached.
            if let Ok(o) = std::env::var("SKYNBD_PUBLISH_OVERLAP") {
                dm_cycle.publish_overlap = Some(flag("SKYNBD_PUBLISH_OVERLAP", &o));
            }
            // §8.1 rule 1's margin (task 15j): the watermark must fire
            // while there is still room for the rescue advance's own
            // freeze — observed write rate × this window, plus the
            // dirty-flush reserve. Both only bite under a configured
            // watermark.
            if let Ok(s) = std::env::var("SKYNBD_ADVANCE_WINDOW_SECONDS") {
                dm_cycle.advance_window = Some(std::time::Duration::from_secs(
                    s.parse().expect("SKYNBD_ADVANCE_WINDOW_SECONDS"),
                ));
            }
            if let Ok(b) = std::env::var("SKYNBD_DIRTY_FLUSH_RESERVE_BYTES") {
                dm_cycle.dirty_flush_reserve =
                    Some(b.parse().expect("SKYNBD_DIRTY_FLUSH_RESERVE_BYTES"));
            }
            // §8.1 rule 3 (`publishFailingReadOnlyWatermark`): when
            // publishes have been failing this long AND the pool is at
            // or above this occupancy, take the filesystem read-only on
            // purpose. Both must be set — one term is not the rule, and
            // defaulting the missing half would arm a destructive action
            // from a partial configuration.
            let ro_pair = (
                std::env::var("SKYNBD_PUBLISH_FAILING_RO_WATERMARK").ok(),
                std::env::var("SKYNBD_PUBLISH_FAILING_RO_SECONDS").ok(),
            );
            match ro_pair {
                (Some(w), Some(s)) => {
                    dm_cycle.ro_watermark = Some((
                        w.parse().expect("SKYNBD_PUBLISH_FAILING_RO_WATERMARK"),
                        std::time::Duration::from_secs(
                            s.parse().expect("SKYNBD_PUBLISH_FAILING_RO_SECONDS"),
                        ),
                    ));
                }
                (None, None) => {}
                _ => panic!(
                    "SKYNBD_PUBLISH_FAILING_RO_WATERMARK and \
                     SKYNBD_PUBLISH_FAILING_RO_SECONDS must be set together (§8.1 rule 3)"
                ),
            }
            // Node-level §7 compaction-policy defaults (task 15i);
            // per-StorageClass dials override these field-wise.
            if let Ok(s) = std::env::var("SKYNBD_RECLAIM_STRATEGY") {
                dm_cycle.policy.strategy = s.parse().expect("SKYNBD_RECLAIM_STRATEGY");
            }
            if let Ok(t) = std::env::var("SKYNBD_RECLAIM_TIMING") {
                dm_cycle.policy.timing = t.parse().expect("SKYNBD_RECLAIM_TIMING");
            }
            // Through the same helpers the StorageClass keys go
            // through: the node default and the per-volume override
            // land on one dial of one engine, so a value one surface
            // refuses cannot be a value the other accepts.
            if let Ok(r) = std::env::var("SKYNBD_RECLAIM_RANGE_THRESHOLD") {
                dm_cycle.policy.range_threshold =
                    agentcycle::parse_range_threshold(&r).expect("SKYNBD_RECLAIM_RANGE_THRESHOLD");
            }
            if let Ok(n) = std::env::var("SKYNBD_RECLAIM_SWING_EVERY_N") {
                dm_cycle.policy.swing_every_n = Some(
                    agentcycle::parse_swing_every_n(&n).expect("SKYNBD_RECLAIM_SWING_EVERY_N"),
                );
            }
            Arc::new(dm_cycle) as Arc<dyn agentcycle::CycleRunner>
        });
        // D-M6 handoff + D-M11 external origins over the same client
        // the node already holds: serve-before-attach,
        // release-after-detach, clone-the-newest-generation on a
        // fresh-cache thin restage. The node's identity is the lease
        // holder for lineage serves (Invariant F, task 15h).
        let node_id = env("NODE_ID").expect("NODE_ID is required in node mode");
        let mechanics = Arc::new(
            node::NodeMechanics::new(cfg, Arc::new(runner::HostRunner), thin_stack, cycle)
                .with_handoff(Arc::new(handoff::CpHandoff::new(
                    cp::Client::new(&api_url, token_file.clone()),
                    node_id.clone(),
                )))
                .with_origins(Arc::new(handoff::CpOrigins::new(cp::Client::new(
                    &api_url,
                    token_file.clone(),
                )))),
        );
        Some(csi::v1::node_server::NodeServer::new(services::NodeSvc {
            node_id,
            mechanics,
        }))
    } else {
        None
    };
    let router = router.add_optional_service(node_service);

    if let Some(dir) = std::path::Path::new(&socket_path).parent() {
        std::fs::create_dir_all(dir).expect("create socket dir");
    }
    let _ = std::fs::remove_file(&socket_path);
    let listener = tokio::net::UnixListener::bind(&socket_path).expect("bind CSI socket");
    tracing::info!(%mode, socket = %socket_path, api = %api_url, "skynbd-csi-driver serving");

    let incoming = tokio_stream::wrappers::UnixListenerStream::new(listener);
    tokio::select! {
        r = router.serve_with_incoming(incoming) => r.expect("grpc server"),
        _ = tokio::signal::ctrl_c() => tracing::info!("shutting down"),
    }
}

#[cfg(test)]
mod tests {
    use super::flag;

    #[test]
    fn flags_take_true_false_and_one_zero() {
        for (v, want) in [
            ("true", true),
            ("FALSE", false),
            ("1", true),
            (" 0 ", false),
        ] {
            assert_eq!(flag("K", v), want, "{v:?}");
        }
        let err = std::panic::catch_unwind(|| flag("SKYNBD_X", "off")).unwrap_err();
        let msg = err.downcast_ref::<String>().unwrap();
        assert!(msg.contains("SKYNBD_X"), "{msg}");
    }
}

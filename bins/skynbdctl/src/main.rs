//! skynbdctl — operator tooling (design §12.1).
//!
//! M4 scope: offline `fsck` and `--rebuild-refs` (design §6.5) working
//! directly against the store + refs from the daemon's own config file,
//! so recovery works with the daemon stopped. The full CLI mirror of the
//! REST API arrives with M5+.
//!
//! Usage:
//!   skynbdctl fsck <config.toml> <volume> [--rebuild-refs]

use std::sync::Arc;

use serde::Deserialize;
use skynbd_refs::{MemoryRefStore, RedbRefStore, RefStore};
use skynbd_store::{MemoryStore, ObjectStore};
use skynbd_types::VolumeId;

// Config shapes mirror bins/skynbdd (kept in sync by the e2e harness,
// which feeds both tools the same file).
#[derive(Debug, Deserialize)]
struct Config {
    refs_path: Option<String>,
    #[serde(default)]
    store: StoreConfig,
    #[serde(default)]
    #[allow(dead_code)]
    volumes: Vec<VolumeConfig>,
}

// Mirrors skynbdd's [store] schema so both parse the same skynbd.toml
// (`create_bucket` is daemon-only; serde ignores it here).
#[derive(Debug, Default, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum StoreConfig {
    #[default]
    Memory,
    #[cfg_attr(not(feature = "s3"), allow(dead_code))]
    S3 {
        endpoint: Option<String>,
        bucket: String,
        access_key: Option<String>,
        secret_key: Option<String>,
        region: Option<String>,
        #[serde(default = "default_true")]
        force_path_style: bool,
    },
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct VolumeConfig {
    #[allow(dead_code)]
    name: String,
    #[allow(dead_code)]
    size: u64,
}

/// Same deterministic name→id derivation as skynbdd.
fn volume_id(name: &str) -> VolumeId {
    let h = blake3::hash(name.as_bytes());
    let mut b = [0u8; 16];
    b.copy_from_slice(&h.as_bytes()[..16]);
    VolumeId(ulid::Ulid::from(u128::from_be_bytes(b)))
}

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
            Arc::new(s)
        }
        #[cfg(not(feature = "s3"))]
        StoreConfig::S3 { .. } => {
            panic!("this skynbdctl was built without the 's3' feature");
        }
    }
}

fn usage() -> ! {
    eprintln!(
        "usage: skynbdctl fsck <config.toml> <volume> [--rebuild-refs]\n\
         \x20      skynbdctl rebuild-catalog <config.toml> [--with-refs]"
    );
    std::process::exit(2);
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (cmd, rest) = match args.split_first() {
        Some((c, r)) => (c.as_str(), r),
        None => usage(),
    };
    // rebuild-catalog (design D-M9): restore catalog rows from the
    // name-index markers; --with-refs also rebuilds each restored
    // volume's refs from the bucket (§6.5) — together, full recovery
    // from a lost refs store with the daemon stopped.
    if cmd == "rebuild-catalog" {
        let Some(config_path) = rest.first() else {
            usage();
        };
        let with_refs = rest.iter().any(|a| a == "--with-refs");
        let cfg: Config =
            toml::from_str(&std::fs::read_to_string(config_path).expect("read config"))
                .expect("parse config");
        let store = build_store(&cfg.store).await;
        let refs: Arc<dyn RefStore> = match &cfg.refs_path {
            Some(p) => Arc::new(RedbRefStore::open(p).expect("open refs")),
            None => Arc::new(MemoryRefStore::new()),
        };
        let restored = skynbd_api::provision::rebuild_catalog(&store, &refs)
            .await
            .expect("rebuild catalog");
        let mut out = serde_json::json!({ "restored": restored });
        if with_refs {
            let mut reports = serde_json::Map::new();
            for name in out["restored"].as_array().unwrap().clone() {
                let name = name.as_str().unwrap();
                let report = skynbd_engine::rebuild_refs(&store, &refs, volume_id(name))
                    .await
                    .expect("rebuild refs");
                reports.insert(name.to_string(), serde_json::to_value(report).unwrap());
            }
            out["refs"] = reports.into();
        }
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
        return;
    }
    if cmd != "fsck" || rest.len() < 2 {
        usage();
    }
    let config_path = &rest[0];
    let vol_name = &rest[1];
    let rebuild = rest.iter().any(|a| a == "--rebuild-refs");

    let cfg: Config = toml::from_str(&std::fs::read_to_string(config_path).expect("read config"))
        .expect("parse config");
    if matches!(cfg.store, StoreConfig::Memory) {
        eprintln!("warning: memory store holds no durable state to check");
    }
    let store = build_store(&cfg.store).await;
    let refs: Arc<dyn RefStore> = match &cfg.refs_path {
        Some(p) => Arc::new(RedbRefStore::open(p).expect("open refs")),
        None => Arc::new(MemoryRefStore::new()),
    };
    let vol = volume_id(vol_name);

    if rebuild {
        let report = skynbd_engine::rebuild_refs(&store, &refs, vol)
            .await
            .expect("rebuild");
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }
    let report = skynbd_engine::fsck(&store, &refs, vol).await.expect("fsck");
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if !report.ok() {
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// skynbdctl reads the daemon's skynbd.toml: a keyless [store] with
    /// daemon-only fields (create_bucket) must parse here too.
    #[test]
    fn parses_daemon_config_with_keyless_store() {
        let cfg: Config = toml::from_str(
            "refs_path = \"refs.redb\"\n[store]\nkind = \"s3\"\nbucket = \"b\"\n\
             create_bucket = false\n",
        )
        .unwrap();
        let StoreConfig::S3 {
            access_key,
            secret_key,
            endpoint,
            force_path_style,
            ..
        } = cfg.store
        else {
            panic!("expected s3 store");
        };
        assert!(access_key.is_none() && secret_key.is_none() && endpoint.is_none());
        assert!(force_path_style);
    }

    #[test]
    fn parses_static_key_store() {
        let cfg: Config = toml::from_str(
            "[store]\nkind = \"s3\"\nbucket = \"b\"\nendpoint = \"http://127.0.0.1:9000\"\n\
             access_key = \"ak\"\nsecret_key = \"sk\"\nforce_path_style = false\n",
        )
        .unwrap();
        let StoreConfig::S3 {
            access_key,
            force_path_style,
            ..
        } = cfg.store
        else {
            panic!("expected s3 store");
        };
        assert_eq!(access_key.as_deref(), Some("ak"));
        assert!(!force_path_style);
    }
}

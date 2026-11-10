//! `S3Store`: aws-sdk-s3 implementation (design §11).
//!
//! Works against MinIO with `force_path_style` and a custom endpoint.
//! Retries are delegated to the SDK's standard retry mode; the engine maps
//! exhausted retries to `NBD_EIO` (design §8.3, M2).
//!
//! Integration tests are env-gated: set `SKYNBD_S3_TEST_ENDPOINT` (e.g.
//! `http://127.0.0.1:9000`) plus standard AWS credential env vars, and the
//! conformance suite runs against the live endpoint (CI wires MinIO).

use std::ops::Range;

use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::error::SdkError;
use aws_sdk_s3::Client;
use bytes::Bytes;

use crate::{ObjectStore, Precondition, Result, StoreError};

pub struct S3Store {
    client: Client,
    bucket: String,
}

impl S3Store {
    /// Build from explicit parts (tests, MinIO) — no ambient config.
    pub async fn new(
        endpoint: &str,
        bucket: &str,
        access_key: &str,
        secret_key: &str,
        region: &str,
        force_path_style: bool,
    ) -> Self {
        let creds = Credentials::new(access_key, secret_key, None, None, "skynbd-static");
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .endpoint_url(endpoint)
            .region(Region::new(region.to_string()))
            .credentials_provider(creds)
            .force_path_style(force_path_style)
            .build();
        Self {
            client: Client::from_conf(config),
            bucket: bucket.to_string(),
        }
    }

    /// Build from ambient AWS config (auth design §5): the default
    /// credential chain resolves web identity
    /// (`AWS_WEB_IDENTITY_TOKEN_FILE` + `AWS_ROLE_ARN`, honoring
    /// `AWS_ENDPOINT_URL_STS`), env keys, profiles, and ECS/IMDS roles,
    /// refreshing session credentials before expiry. `None` endpoint or
    /// region defers to the environment too (`AWS_ENDPOINT_URL_S3`,
    /// `AWS_REGION`, profile).
    pub async fn from_env(
        endpoint: Option<&str>,
        bucket: &str,
        region: Option<&str>,
        force_path_style: bool,
    ) -> Self {
        let mut loader = aws_config::defaults(BehaviorVersion::latest());
        if let Some(r) = region {
            loader = loader.region(Region::new(r.to_string()));
        }
        if let Some(e) = endpoint {
            loader = loader.endpoint_url(e);
        }
        let shared = loader.load().await;
        let config = aws_sdk_s3::config::Builder::from(&shared)
            .force_path_style(force_path_style)
            .build();
        Self {
            client: Client::from_conf(config),
            bucket: bucket.to_string(),
        }
    }

    /// Create the bucket if it doesn't exist (dev/test convenience).
    pub async fn ensure_bucket(&self) -> Result<()> {
        match self
            .client
            .create_bucket()
            .bucket(&self.bucket)
            .send()
            .await
        {
            Ok(_) => Ok(()),
            Err(SdkError::ServiceError(e))
                if e.err().is_bucket_already_owned_by_you()
                    || e.err().is_bucket_already_exists() =>
            {
                Ok(())
            }
            Err(e) => Err(StoreError::Backend(format!("create_bucket: {e}"))),
        }
    }
}

fn backend<E: std::fmt::Display>(op: &str) -> impl FnOnce(E) -> StoreError + '_ {
    move |e| StoreError::Backend(format!("{op}: {e}"))
}

#[async_trait::async_trait]
impl ObjectStore for S3Store {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(data.into())
            .send()
            .await
            .map(|_| ())
            .map_err(backend("put"))
    }

    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        let mut req = self.client.get_object().bucket(&self.bucket).key(key);
        if let Some(r) = &range {
            if r.start > r.end {
                return Err(StoreError::BadRange {
                    range: r.clone(),
                    len: 0,
                });
            }
            if r.start == r.end {
                return Ok(Bytes::new());
            }
            // HTTP byte ranges are inclusive.
            req = req.range(format!("bytes={}-{}", r.start, r.end - 1));
        }
        match req.send().await {
            Ok(out) => {
                let data = out
                    .body
                    .collect()
                    .await
                    .map_err(|e| StoreError::Backend(format!("get body: {e}")))?;
                Ok(data.into_bytes())
            }
            Err(SdkError::ServiceError(e)) if e.err().is_no_such_key() => {
                Err(StoreError::NotFound(key.to_string()))
            }
            Err(e) => Err(StoreError::Backend(format!("get: {e}"))),
        }
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        match self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(e)) if e.err().is_not_found() => Ok(false),
            Err(e) => Err(StoreError::Backend(format!("head: {e}"))),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .list_with_mtime(prefix)
            .await?
            .into_iter()
            .map(|(k, _)| k)
            .collect())
    }

    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        let mut keys = Vec::new();
        let mut cont: Option<String> = None;
        loop {
            let mut req = self
                .client
                .list_objects_v2()
                .bucket(&self.bucket)
                .prefix(prefix);
            if let Some(token) = &cont {
                req = req.continuation_token(token);
            }
            let out = req.send().await.map_err(backend("list"))?;
            for obj in out.contents() {
                if let Some(k) = obj.key() {
                    let mtime = obj
                        .last_modified()
                        .map(|t| (t.as_secs_f64() * 1000.0) as u64);
                    keys.push((k.to_string(), mtime));
                }
            }
            match out.next_continuation_token() {
                Some(t) => cont = Some(t.to_string()),
                None => break,
            }
        }
        keys.sort();
        Ok(keys)
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map(|_| ())
            .map_err(backend("delete"))
    }

    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        match self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
        {
            Ok(out) => {
                let etag = out
                    .e_tag()
                    .ok_or_else(|| StoreError::Backend("get: no etag".into()))?
                    .to_string();
                let data = out
                    .body
                    .collect()
                    .await
                    .map_err(|e| StoreError::Backend(format!("get body: {e}")))?;
                Ok((data.into_bytes(), etag))
            }
            Err(SdkError::ServiceError(e)) if e.err().is_no_such_key() => {
                Err(StoreError::NotFound(key.to_string()))
            }
            Err(e) => Err(StoreError::Backend(format!("get: {e}"))),
        }
    }

    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> Result<String> {
        let mut req = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .body(data.into());
        req = match &pre {
            Precondition::IfMatch(etag) => req.if_match(etag),
            Precondition::IfNoneMatch => req.if_none_match("*"),
        };
        match req.send().await {
            Ok(out) => out
                .e_tag()
                .map(str::to_string)
                .ok_or_else(|| StoreError::Backend("put: no etag".into())),
            Err(SdkError::ServiceError(e)) => {
                let status = e.raw().status().as_u16();
                // 412 = the precondition lost; 409 = S3's answer when a
                // concurrent conditional create is in flight. Both mean
                // "somebody else got there": re-read and retry.
                if status == 412 || status == 409 {
                    Err(StoreError::PreconditionFailed(key.to_string()))
                } else if status == 501 || status == 400 {
                    // Not-implemented from backends without conditional
                    // support (older MinIO answers 501, some gateways
                    // reject the header outright).
                    Err(StoreError::Unsupported("put_if on this backend"))
                } else {
                    Err(StoreError::Backend(format!("put_if: {status}")))
                }
            }
            Err(e) => Err(StoreError::Backend(format!("put_if: {e}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conformance;

    /// Constructor-only: the default chain resolves credentials lazily
    /// (on first request), so ambient construction needs no env vars,
    /// no credentials, and no network. Guards against a regression that
    /// makes `from_env` block or fail at startup when the environment
    /// is empty — the daemon must still boot and report errors per-op.
    #[tokio::test]
    async fn from_env_constructs_offline() {
        let _store = S3Store::from_env(
            Some("http://127.0.0.1:1"),
            "bucket",
            Some("us-east-1"),
            true,
        )
        .await;
    }

    /// Runs only when SKYNBD_S3_TEST_ENDPOINT is set (CI wires MinIO).
    #[tokio::test]
    async fn s3_conformance_against_live_endpoint() {
        let Ok(endpoint) = std::env::var("SKYNBD_S3_TEST_ENDPOINT") else {
            eprintln!("skipped: SKYNBD_S3_TEST_ENDPOINT not set");
            return;
        };
        let access = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or("minioadmin".into());
        let secret = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or("minioadmin".into());
        let store = S3Store::new(
            &endpoint,
            "skynbd-conformance",
            &access,
            &secret,
            "us-east-1",
            true,
        )
        .await;
        store.ensure_bucket().await.unwrap();
        conformance::exercise_object_store(&store).await;
    }

    /// Conditional-write conformance against a live endpoint. MinIO has
    /// supported If-None-Match/If-Match PUTs since 2024/2025; backends
    /// without them (SeaweedFS today) answer Unsupported from the probe
    /// and the test reports that instead of failing — the capability is
    /// optional, and skynbdd probes it the same way at startup.
    #[tokio::test]
    async fn s3_conditional_conformance() {
        let Ok(endpoint) = std::env::var("SKYNBD_S3_TEST_ENDPOINT") else {
            eprintln!("skipped: SKYNBD_S3_TEST_ENDPOINT not set");
            return;
        };
        let access = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or("minioadmin".into());
        let secret = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or("minioadmin".into());
        let store = S3Store::new(
            &endpoint,
            "skynbd-conditional",
            &access,
            &secret,
            "us-east-1",
            true,
        )
        .await;
        store.ensure_bucket().await.unwrap();
        let _ = store.delete("cond/probe").await;
        match store
            .put_if(
                "cond/probe",
                bytes::Bytes::from_static(b"p"),
                Precondition::IfNoneMatch,
            )
            .await
        {
            Err(StoreError::Unsupported(what)) => {
                eprintln!("skipped: backend lacks conditional writes ({what})");
            }
            Err(e) => panic!("conditional probe failed unexpectedly: {e}"),
            Ok(_) => {
                store.delete("cond/probe").await.unwrap();
                conformance::exercise_conditional_store(&store).await;
            }
        }
    }

    /// WIF conformance (auth design §5; ci.yml `s3-seaweedfs`, rig in
    /// tests/wif/): with only web-identity env vars exported, the ambient
    /// chain must exchange `AWS_WEB_IDENTITY_TOKEN_FILE` via STS
    /// `AssumeRoleWithWebIdentity` before any S3 request can succeed —
    /// unlike `s3_conformance_from_env`, which the chain satisfies from
    /// env keys without ever touching STS.
    #[tokio::test]
    async fn s3_conformance_web_identity() {
        let Ok(endpoint) = std::env::var("SKYNBD_S3_WIF_ENDPOINT") else {
            eprintln!("skipped: SKYNBD_S3_WIF_ENDPOINT not set");
            return;
        };
        assert!(
            std::env::var("AWS_ACCESS_KEY_ID").is_err(),
            "AWS_ACCESS_KEY_ID is set: the chain would resolve env keys \
             and never exercise web identity"
        );
        assert!(
            std::env::var("AWS_WEB_IDENTITY_TOKEN_FILE").is_ok()
                && std::env::var("AWS_ROLE_ARN").is_ok(),
            "web identity env vars missing"
        );
        let store = S3Store::from_env(Some(&endpoint), "skynbd-wif", Some("us-east-1"), true).await;
        store.ensure_bucket().await.unwrap();
        conformance::exercise_object_store(&store).await;
    }

    /// Same live-endpoint gate, but through the ambient-config path: the
    /// default chain must resolve the env credentials CI already exports.
    #[tokio::test]
    async fn s3_conformance_from_env() {
        let Ok(endpoint) = std::env::var("SKYNBD_S3_TEST_ENDPOINT") else {
            eprintln!("skipped: SKYNBD_S3_TEST_ENDPOINT not set");
            return;
        };
        if std::env::var("AWS_ACCESS_KEY_ID").is_err() {
            eprintln!("skipped: AWS_ACCESS_KEY_ID not set (from_env needs ambient creds)");
            return;
        }
        let store = S3Store::from_env(
            Some(&endpoint),
            "skynbd-conformance-env",
            Some("us-east-1"),
            true,
        )
        .await;
        store.ensure_bucket().await.unwrap();
        conformance::exercise_object_store(&store).await;
    }
}

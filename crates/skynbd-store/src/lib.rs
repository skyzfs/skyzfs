//! `ObjectStore`: the S3 seam (design §3.1).
//!
//! Two production-relevant implementations are planned:
//! - `S3Store` (aws-sdk-s3, path-style for MinIO) — TODO(M1)
//! - `FaultStore` wrapper (latency/error/kill-point injection for the
//!   crash-point matrix, design §14.2) — TODO(M1)
//!
//! `MemoryStore` ships now so every layer above can be developed and
//! tested without network.

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::RwLock;

mod fault;
pub use fault::{FaultOp, FaultStore};

#[cfg(feature = "s3")]
mod s3;
#[cfg(feature = "s3")]
pub use s3::S3Store;

#[derive(Debug, Clone, thiserror::Error)]
pub enum StoreError {
    #[error("object not found: {0}")]
    NotFound(String),
    #[error("range {range:?} out of bounds for object of len {len}")]
    BadRange { range: Range<u64>, len: u64 },
    /// A conditional write lost: the object changed (or appeared) since
    /// the version the caller read. Retryable by re-reading.
    #[error("precondition failed on {0}: concurrent write")]
    PreconditionFailed(String),
    /// The backend cannot do conditional writes at all — a capability,
    /// not a transient state. Callers probe for this at startup rather
    /// than discovering it mid-transaction.
    #[error("conditional writes unsupported: {0}")]
    Unsupported(&'static str),
    #[error("backend error: {0}")]
    Backend(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Concurrency guard for [`ObjectStore::put_if`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Precondition {
    /// Replace only this exact version (an etag from
    /// [`ObjectStore::get_versioned`]).
    IfMatch(String),
    /// Create only if the key does not exist.
    IfNoneMatch,
}

/// Minimal object-store surface the engine needs (design §11: no crate
/// outside `skynbd-store` knows about S3).
#[async_trait::async_trait]
pub trait ObjectStore: Send + Sync {
    /// Idempotent full-object write (keys are content-addressed or ULID-unique).
    async fn put(&self, key: &str, data: Bytes) -> Result<()>;
    /// Full read, or a byte-range read when `range` is set (design §7.2).
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes>;
    async fn exists(&self, key: &str) -> Result<bool>;
    /// Ordered key listing under a prefix (GC sweep, recovery).
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
    async fn delete(&self, key: &str) -> Result<()>;

    /// [`Self::list`] plus each key's last-modified time in Unix ms,
    /// where the backend knows it (S3: `LastModified`). `None` is legal
    /// and means "unknown" — a caller using the mtime as a scan bound
    /// (design D-M4) must treat `None` as possibly-new and read the
    /// object. The default keeps simple backends simple.
    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        Ok(self
            .list(prefix)
            .await?
            .into_iter()
            .map(|k| (k, None))
            .collect())
    }

    /// Full read plus the object's version (S3: the ETag) — the token
    /// [`Self::put_if`] guards against. Optional capability (design
    /// D-M1): backends that cannot provide it return
    /// [`StoreError::Unsupported`], and callers that need it probe first.
    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        let _ = key;
        Err(StoreError::Unsupported("get_versioned"))
    }

    /// Conditional full-object write: the compare-and-swap primitive the
    /// object ref store is built on (design D-M1). Returns the new
    /// version. A lost race is [`StoreError::PreconditionFailed`] —
    /// re-read and retry; an incapable backend is
    /// [`StoreError::Unsupported`].
    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> Result<String> {
        let _ = (key, data, pre);
        Err(StoreError::Unsupported("put_if"))
    }
}

/// Delegation through `Arc`, enabling type-erased `Arc<dyn ObjectStore>`
/// engines (the NBD server's registry holds these).
#[async_trait::async_trait]
impl<T: ObjectStore + ?Sized> ObjectStore for Arc<T> {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        (**self).put(key, data).await
    }
    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        (**self).get(key, range).await
    }
    async fn exists(&self, key: &str) -> Result<bool> {
        (**self).exists(key).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        (**self).list(prefix).await
    }
    async fn delete(&self, key: &str) -> Result<()> {
        (**self).delete(key).await
    }
    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        (**self).list_with_mtime(prefix).await
    }
    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        (**self).get_versioned(key).await
    }
    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> Result<String> {
        (**self).put_if(key, data, pre).await
    }
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock before Unix epoch")
        .as_millis() as u64
}

/// A stored object: bytes, monotonically increasing per-key version
/// (the etag), and write time in Unix ms (`list_with_mtime`).
type Stored = (Bytes, u64, u64);

/// In-memory `ObjectStore` for tests and fast local development.
#[derive(Debug, Default, Clone)]
pub struct MemoryStore {
    inner: Arc<RwLock<BTreeMap<String, Stored>>>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn object_count(&self) -> usize {
        self.inner.read().await.len()
    }
}

#[async_trait::async_trait]
impl ObjectStore for MemoryStore {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        let mut map = self.inner.write().await;
        let version = map.get(key).map(|(_, v, _)| v + 1).unwrap_or(0);
        map.insert(key.to_string(), (data, version, unix_ms()));
        Ok(())
    }

    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        let map = self.inner.read().await;
        let (data, _, _) = map
            .get(key)
            .ok_or_else(|| StoreError::NotFound(key.to_string()))?;
        match range {
            None => Ok(data.clone()),
            Some(r) => {
                let len = data.len() as u64;
                if r.start > r.end || r.end > len {
                    return Err(StoreError::BadRange { range: r, len });
                }
                Ok(data.slice(r.start as usize..r.end as usize))
            }
        }
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.inner.read().await.contains_key(key))
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self
            .inner
            .read()
            .await
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, _)| k.clone())
            .collect())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.write().await.remove(key);
        Ok(())
    }

    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        Ok(self
            .inner
            .read()
            .await
            .range(prefix.to_string()..)
            .take_while(|(k, _)| k.starts_with(prefix))
            .map(|(k, (_, _, m))| (k.clone(), Some(*m)))
            .collect())
    }

    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        let map = self.inner.read().await;
        let (data, version, _) = map
            .get(key)
            .ok_or_else(|| StoreError::NotFound(key.to_string()))?;
        Ok((data.clone(), version.to_string()))
    }

    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> Result<String> {
        let mut map = self.inner.write().await;
        let current = map.get(key).map(|(_, v, _)| *v);
        let next = match (&pre, current) {
            (Precondition::IfNoneMatch, None) => 0,
            (Precondition::IfMatch(etag), Some(v)) if *etag == v.to_string() => v + 1,
            _ => return Err(StoreError::PreconditionFailed(key.to_string())),
        };
        map.insert(key.to_string(), (data, next, unix_ms()));
        Ok(next.to_string())
    }
}

/// Backend-agnostic conformance suite: every `ObjectStore` implementation
/// must pass this (memory in unit tests, S3/MinIO in the env-gated
/// integration test). Panics on violation.
pub mod conformance {
    use super::*;

    /// Conformance for the optional conditional-write capability (design
    /// D-M1). Only call on stores that claim it — the object ref store
    /// probes with `put_if(.., IfNoneMatch)` and treats `Unsupported` as
    /// "pick another backend", not a failure.
    pub async fn exercise_conditional_store(store: &dyn ObjectStore) {
        let k = "cond/probe";

        // Create-if-absent, exactly once.
        let e0 = store
            .put_if(k, Bytes::from_static(b"v0"), Precondition::IfNoneMatch)
            .await
            .expect("first conditional create");
        let dup = store
            .put_if(k, Bytes::from_static(b"x"), Precondition::IfNoneMatch)
            .await;
        assert!(
            matches!(dup, Err(StoreError::PreconditionFailed(_))),
            "second create must lose: {dup:?}"
        );

        // The read's version guards the next write.
        let (data, etag) = store.get_versioned(k).await.expect("get_versioned");
        assert_eq!(&data[..], b"v0");
        assert_eq!(etag, e0, "get must observe the version put_if returned");

        let e1 = store
            .put_if(
                k,
                Bytes::from_static(b"v1"),
                Precondition::IfMatch(etag.clone()),
            )
            .await
            .expect("if-match with current version");
        assert_ne!(e1, e0, "a successful write must change the version");

        // The old version is now stale.
        let stale = store
            .put_if(k, Bytes::from_static(b"x"), Precondition::IfMatch(etag))
            .await;
        assert!(
            matches!(stale, Err(StoreError::PreconditionFailed(_))),
            "stale etag must lose: {stale:?}"
        );

        // An unconditional overwrite invalidates outstanding etags too.
        store.put(k, Bytes::from_static(b"v2")).await.expect("put");
        let raced = store
            .put_if(k, Bytes::from_static(b"x"), Precondition::IfMatch(e1))
            .await;
        assert!(
            matches!(raced, Err(StoreError::PreconditionFailed(_))),
            "etag from before an unconditional put must lose: {raced:?}"
        );

        store.delete(k).await.expect("cleanup");
    }

    pub async fn exercise_object_store(store: &dyn ObjectStore) {
        let p = "conformance/";
        // Re-runnable against persistent backends: clear leftovers first.
        for k in store.list(p).await.unwrap() {
            store.delete(&k).await.unwrap();
        }

        // put/get roundtrip + idempotent overwrite
        let key = format!("{p}seg/a");
        store
            .put(&key, Bytes::from_static(b"hello world"))
            .await
            .unwrap();
        store
            .put(&key, Bytes::from_static(b"hello world"))
            .await
            .unwrap();
        assert_eq!(
            store.get(&key, None).await.unwrap(),
            Bytes::from_static(b"hello world")
        );

        // range get (byte-exact, half-open) + empty range
        assert_eq!(
            store.get(&key, Some(6..11)).await.unwrap(),
            Bytes::from_static(b"world")
        );
        assert_eq!(store.get(&key, Some(3..3)).await.unwrap(), Bytes::new());

        // missing key
        assert!(matches!(
            store.get(&format!("{p}nope"), None).await,
            Err(StoreError::NotFound(_))
        ));
        assert!(!store.exists(&format!("{p}nope")).await.unwrap());
        assert!(store.exists(&key).await.unwrap());

        // list: prefix-scoped, ordered
        store
            .put(&format!("{p}node/b"), Bytes::new())
            .await
            .unwrap();
        store
            .put(&format!("{p}node/a"), Bytes::new())
            .await
            .unwrap();
        let nodes = store.list(&format!("{p}node/")).await.unwrap();
        assert_eq!(nodes, vec![format!("{p}node/a"), format!("{p}node/b")]);
        assert_eq!(store.list(p).await.unwrap().len(), 3);

        // delete: absent afterwards; deleting the absent is fine
        store.delete(&key).await.unwrap();
        assert!(!store.exists(&key).await.unwrap());
        store.delete(&key).await.unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_store_passes_conformance() {
        conformance::exercise_object_store(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn memory_store_passes_conditional_conformance() {
        conformance::exercise_conditional_store(&MemoryStore::new()).await;
    }

    #[tokio::test]
    async fn put_get_roundtrip_and_idempotent_overwrite() {
        let s = MemoryStore::new();
        s.put("v/seg/a", Bytes::from_static(b"hello"))
            .await
            .unwrap();
        s.put("v/seg/a", Bytes::from_static(b"hello"))
            .await
            .unwrap(); // idempotent re-PUT
        assert_eq!(
            s.get("v/seg/a", None).await.unwrap(),
            Bytes::from_static(b"hello")
        );
        assert_eq!(s.object_count().await, 1);
    }

    #[tokio::test]
    async fn range_get_and_bounds() {
        let s = MemoryStore::new();
        s.put("k", Bytes::from_static(b"0123456789")).await.unwrap();
        assert_eq!(
            s.get("k", Some(2..5)).await.unwrap(),
            Bytes::from_static(b"234")
        );
        assert!(matches!(
            s.get("k", Some(5..11)).await,
            Err(StoreError::BadRange { .. })
        ));
    }

    #[tokio::test]
    async fn missing_key_is_not_found() {
        let s = MemoryStore::new();
        assert!(matches!(
            s.get("nope", None).await,
            Err(StoreError::NotFound(_))
        ));
        assert!(!s.exists("nope").await.unwrap());
    }

    #[tokio::test]
    async fn list_is_prefix_scoped_and_ordered() {
        let s = MemoryStore::new();
        for k in ["v1/node/b", "v1/node/a", "v1/seg/x", "v2/node/c"] {
            s.put(k, Bytes::new()).await.unwrap();
        }
        assert_eq!(
            s.list("v1/node/").await.unwrap(),
            vec!["v1/node/a", "v1/node/b"]
        );
        assert_eq!(s.list("v1/").await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn delete_then_absent() {
        let s = MemoryStore::new();
        s.put("k", Bytes::from_static(b"x")).await.unwrap();
        s.delete("k").await.unwrap();
        assert!(!s.exists("k").await.unwrap());
        s.delete("k").await.unwrap(); // deleting the absent is fine (GC batches)
    }
}

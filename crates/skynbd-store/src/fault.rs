//! Fault-injecting `ObjectStore` wrapper — the crash-point-matrix harness
//! (design §14.2, layer 3).
//!
//! A "crash" is modeled as: the Nth matching operation fails, and the
//! store stays **poisoned** (every later matching op also fails) so the
//! rest of the in-flight commit dies exactly as it would with a dead
//! process. The test then drops the engine and reopens from durable state,
//! asserting I1/I4/I7.

use std::ops::Range;
use std::sync::Arc;

use bytes::Bytes;
use tokio::sync::Mutex;

use crate::{ObjectStore, Result, StoreError};

/// Which operations a fault applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultOp {
    Put,
    Delete,
    Any,
}

#[derive(Debug, Clone)]
struct Fault {
    op: FaultOp,
    key_substr: String,
    /// Matching ops allowed through before failing starts.
    remaining_ok: u64,
    poisoned: bool,
}

/// Wraps any store; see module docs. Clones share fault state, so a test
/// can keep one handle for `arm`/`disarm` while the engine owns another.
pub struct FaultStore<S> {
    inner: S,
    state: Arc<Mutex<Option<Fault>>>,
}

impl<S: Clone> Clone for FaultStore<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl<S: ObjectStore> FaultStore<S> {
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            state: Arc::new(Mutex::new(None)),
        }
    }

    pub fn inner(&self) -> &S {
        &self.inner
    }

    /// Let `allow_n` matching ops succeed, then fail that op and every
    /// matching op after it (poison). `key_substr` filters by substring
    /// (`""` matches all keys).
    pub async fn arm(&self, op: FaultOp, key_substr: &str, allow_n: u64) {
        *self.state.lock().await = Some(Fault {
            op,
            key_substr: key_substr.to_string(),
            remaining_ok: allow_n,
            poisoned: false,
        });
    }

    /// Clear any fault (the "restarted process" has a healthy store).
    pub async fn disarm(&self) {
        *self.state.lock().await = None;
    }

    async fn check(&self, op: FaultOp, key: &str) -> Result<()> {
        let mut guard = self.state.lock().await;
        if let Some(f) = guard.as_mut() {
            let matches = (f.op == FaultOp::Any || f.op == op) && key.contains(&f.key_substr);
            if matches {
                if f.poisoned {
                    return Err(StoreError::Backend("injected fault (poisoned)".into()));
                }
                if f.remaining_ok == 0 {
                    f.poisoned = true;
                    return Err(StoreError::Backend("injected fault".into()));
                }
                f.remaining_ok -= 1;
            }
        }
        Ok(())
    }
}

#[async_trait::async_trait]
impl<S: ObjectStore> ObjectStore for FaultStore<S> {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.check(FaultOp::Put, key).await?;
        self.inner.put(key, data).await
    }

    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        self.check(FaultOp::Any, key).await?;
        self.inner.get(key, range).await
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        self.check(FaultOp::Any, key).await?;
        self.inner.exists(key).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.check(FaultOp::Any, prefix).await?;
        self.inner.list(prefix).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.check(FaultOp::Delete, key).await?;
        self.inner.delete(key).await
    }

    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        self.check(FaultOp::Any, prefix).await?;
        self.inner.list_with_mtime(prefix).await
    }

    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        self.check(FaultOp::Any, key).await?;
        self.inner.get_versioned(key).await
    }

    async fn put_if(&self, key: &str, data: Bytes, pre: crate::Precondition) -> Result<String> {
        // A conditional write is still a write: the crash matrix's
        // poisoned-put faults must hit it like any other PUT.
        self.check(FaultOp::Put, key).await?;
        self.inner.put_if(key, data, pre).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStore;

    #[tokio::test]
    async fn fault_fires_on_nth_matching_put_and_poisons() {
        let s = FaultStore::new(MemoryStore::new());
        s.arm(FaultOp::Put, "/seg/", 1).await;

        s.put("v/seg/a", Bytes::from_static(b"1")).await.unwrap(); // allowed (1)
        s.put("v/node/x", Bytes::from_static(b"m")).await.unwrap(); // non-matching
        assert!(s.put("v/seg/b", Bytes::from_static(b"2")).await.is_err()); // fails
        assert!(s.put("v/seg/c", Bytes::from_static(b"3")).await.is_err()); // poisoned
                                                                            // Non-matching keys still work while poisoned (a dead process is
                                                                            // modeled per-key-class by arming with "" instead).
        s.put("v/node/y", Bytes::from_static(b"m")).await.unwrap();

        s.disarm().await;
        s.put("v/seg/d", Bytes::from_static(b"4")).await.unwrap();
        // Partial state: a and d exist, b and c never landed.
        assert!(s.inner().exists("v/seg/a").await.unwrap());
        assert!(!s.inner().exists("v/seg/b").await.unwrap());
    }
}

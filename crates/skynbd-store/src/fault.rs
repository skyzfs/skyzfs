//! Fault-injecting `ObjectStore` wrapper — the crash-point-matrix harness
//! (design §14.2, layer 3).
//!
//! A "crash" is modeled as: the Nth matching operation fails, and the
//! store stays **poisoned** (every later matching op also fails) so the
//! rest of the in-flight commit dies exactly as it would with a dead
//! process. The test then drops the engine and reopens from durable state,
//! asserting I1/I4/I7.
//!
//! The Nth operation fails either before the inner store sees it
//! ([`FaultStore::arm`]: it never happened) or after the inner store has
//! completed it ([`FaultStore::arm_after`]: it took effect, and its caller
//! still sees an error). The second is the death between a request's
//! success and its reply, such as "the head swap landed, then the daemon
//! died" (SkyZFS M2.4).

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

/// When an armed fault fails the operation it falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FaultWhen {
    /// Before the inner store sees it: the operation never happened.
    Before,
    /// After the inner store has completed it: the operation took effect
    /// and its caller still sees an error.
    After,
}

#[derive(Debug, Clone)]
struct Fault {
    op: FaultOp,
    key_substr: String,
    /// Matching ops allowed through before failing starts.
    remaining_ok: u64,
    /// When the first failing op fails. Every op after it fails before
    /// the inner store (a dead process does nothing more).
    when: FaultWhen,
    poisoned: bool,
}

/// What [`FaultStore::check`] decided for one operation.
enum Verdict {
    Pass,
    FailAfter,
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
        self.arm_when(op, key_substr, allow_n, FaultWhen::Before)
            .await;
    }

    /// As [`FaultStore::arm`], except that the first failing op reaches
    /// the inner store and completes before its error is returned: the
    /// write landed, and the process died before it heard so. Every
    /// matching op after it fails before the inner store.
    pub async fn arm_after(&self, op: FaultOp, key_substr: &str, allow_n: u64) {
        self.arm_when(op, key_substr, allow_n, FaultWhen::After)
            .await;
    }

    /// [`FaultStore::arm`] or [`FaultStore::arm_after`], by `when`.
    pub async fn arm_when(&self, op: FaultOp, key_substr: &str, allow_n: u64, when: FaultWhen) {
        *self.state.lock().await = Some(Fault {
            op,
            key_substr: key_substr.to_string(),
            remaining_ok: allow_n,
            when,
            poisoned: false,
        });
    }

    /// Clear any fault (the "restarted process" has a healthy store).
    pub async fn disarm(&self) {
        *self.state.lock().await = None;
    }

    /// `Err` fails the op before the inner store; `FailAfter` fails it
    /// once the inner store has completed it.
    async fn check(&self, op: FaultOp, key: &str) -> Result<Verdict> {
        let mut guard = self.state.lock().await;
        if let Some(f) = guard.as_mut() {
            let matches = (f.op == FaultOp::Any || f.op == op) && key.contains(&f.key_substr);
            if matches {
                if f.poisoned {
                    return Err(StoreError::Backend("injected fault (poisoned)".into()));
                }
                if f.remaining_ok == 0 {
                    f.poisoned = true;
                    return match f.when {
                        FaultWhen::Before => Err(StoreError::Backend("injected fault".into())),
                        FaultWhen::After => Ok(Verdict::FailAfter),
                    };
                }
                f.remaining_ok -= 1;
            }
        }
        Ok(Verdict::Pass)
    }

    /// Run `inner` under the fault: not at all when it fails before, to
    /// completion and then an error when it fails after. Whatever the
    /// inner op returned, a fault after it is an error: the process died.
    async fn gated<T>(
        &self,
        op: FaultOp,
        key: &str,
        inner: impl std::future::Future<Output = Result<T>>,
    ) -> Result<T> {
        match self.check(op, key).await? {
            Verdict::Pass => inner.await,
            Verdict::FailAfter => {
                let _ = inner.await;
                Err(StoreError::Backend(
                    "injected fault (after the operation completed)".into(),
                ))
            }
        }
    }
}

#[async_trait::async_trait]
impl<S: ObjectStore> ObjectStore for FaultStore<S> {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.gated(FaultOp::Put, key, self.inner.put(key, data))
            .await
    }

    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        self.gated(FaultOp::Any, key, self.inner.get(key, range))
            .await
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        self.gated(FaultOp::Any, key, self.inner.exists(key)).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.gated(FaultOp::Any, prefix, self.inner.list(prefix))
            .await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.gated(FaultOp::Delete, key, self.inner.delete(key))
            .await
    }

    async fn list_with_mtime(&self, prefix: &str) -> Result<Vec<(String, Option<u64>)>> {
        self.gated(FaultOp::Any, prefix, self.inner.list_with_mtime(prefix))
            .await
    }

    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        self.gated(FaultOp::Any, key, self.inner.get_versioned(key))
            .await
    }

    async fn put_if(&self, key: &str, data: Bytes, pre: crate::Precondition) -> Result<String> {
        // A conditional write is still a write: the crash matrix's
        // poisoned-put faults must hit it like any other PUT.
        self.gated(FaultOp::Put, key, self.inner.put_if(key, data, pre))
            .await
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

    /// The after-mode models "the operation landed, then the process
    /// died": the inner store has the write, its caller sees an error, and
    /// every later matching operation fails without reaching the store.
    #[tokio::test]
    async fn an_after_fault_lets_the_operation_land_then_fails_it_and_poisons() {
        let s = FaultStore::new(MemoryStore::new());
        s.arm_after(FaultOp::Put, "/refs", 1).await;

        s.put("v/refs", Bytes::from_static(b"1")).await.unwrap(); // allowed (1)
        assert!(s.put("v/refs", Bytes::from_static(b"2")).await.is_err());
        // It failed, and it landed.
        assert_eq!(&s.inner().get("v/refs", None).await.unwrap()[..], b"2");
        // Poisoned: the next one fails before the store sees it.
        assert!(s.put("v/refs", Bytes::from_static(b"3")).await.is_err());
        assert_eq!(&s.inner().get("v/refs", None).await.unwrap()[..], b"2");
        // Non-matching keys still work.
        s.put("v/seg/a", Bytes::from_static(b"a")).await.unwrap();
    }

    /// A conditional PUT is a PUT for the after-mode too: the head swap of
    /// the refs object is the operation "the swap landed, then the daemon
    /// died" is about.
    #[tokio::test]
    async fn an_after_fault_on_a_conditional_put_lands_it() {
        let s = FaultStore::new(MemoryStore::new());
        s.arm_after(FaultOp::Put, "/refs", 0).await;
        let landed = s
            .put_if(
                "v/refs",
                Bytes::from_static(b"x"),
                crate::Precondition::IfNoneMatch,
            )
            .await;
        assert!(landed.is_err());
        assert_eq!(&s.inner().get("v/refs", None).await.unwrap()[..], b"x");
    }

    /// The before-mode, as `arm` always was: the failed operation never
    /// reaches the store.
    #[tokio::test]
    async fn a_before_fault_never_reaches_the_store() {
        let s = FaultStore::new(MemoryStore::new());
        s.arm(FaultOp::Put, "/refs", 0).await;
        assert!(s.put("v/refs", Bytes::from_static(b"x")).await.is_err());
        assert!(!s.inner().exists("v/refs").await.unwrap());
        s.arm(FaultOp::Delete, "/refs", 0).await;
        s.inner()
            .put("v/refs", Bytes::from_static(b"y"))
            .await
            .unwrap();
        assert!(s.delete("v/refs").await.is_err());
        assert!(s.inner().exists("v/refs").await.unwrap());
    }

    /// A delete in the after-mode is gone although its caller saw an error.
    #[tokio::test]
    async fn an_after_fault_on_a_delete_removes_the_object() {
        let s = FaultStore::new(MemoryStore::new());
        s.inner()
            .put("v/refs", Bytes::from_static(b"y"))
            .await
            .unwrap();
        s.arm_after(FaultOp::Delete, "/refs", 0).await;
        assert!(s.delete("v/refs").await.is_err());
        assert!(!s.inner().exists("v/refs").await.unwrap());
    }
}

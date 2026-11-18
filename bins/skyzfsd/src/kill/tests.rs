//! `--kill-at-upload`'s points, with a kill that only writes down what it
//! reached: over a store in memory that writes down each write that
//! landed, so that the order of the writes and the kill shows where the
//! kill fell.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use skynbd_refs::{LeaseOutcome, ObjectRefStore, RefStore};
use skynbd_store::{FaultOp, FaultStore, MemoryStore, ObjectStore, Precondition, Result};

use super::*;
use crate::cli::KillAt;
use crate::upload::{place, runtime, Placed};

type Said = Arc<Mutex<Vec<String>>>;

/// A store in memory that writes down each write once it has landed.
#[derive(Clone)]
struct Log {
    inner: Arc<MemoryStore>,
    said: Said,
}

impl Log {
    fn say(&self, what: String) {
        self.said.lock().expect("the log").push(what);
    }
}

#[async_trait::async_trait]
impl ObjectStore for Log {
    async fn put(&self, key: &str, data: Bytes) -> Result<()> {
        self.inner.put(key, data).await?;
        self.say(format!("put {key}"));
        Ok(())
    }

    async fn get(&self, key: &str, range: Option<Range<u64>>) -> Result<Bytes> {
        self.inner.get(key, range).await
    }

    async fn exists(&self, key: &str) -> Result<bool> {
        self.inner.exists(key).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        self.inner.list(prefix).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(key).await?;
        self.say(format!("delete {key}"));
        Ok(())
    }

    async fn get_versioned(&self, key: &str) -> Result<(Bytes, String)> {
        self.inner.get_versioned(key).await
    }

    async fn put_if(&self, key: &str, data: Bytes, pre: Precondition) -> Result<String> {
        let version = self.inner.put_if(key, data, pre).await?;
        self.say(format!("put_if {key}"));
        Ok(version)
    }
}

fn log() -> (Log, Said) {
    let said = Said::default();
    let log = Log {
        inner: Arc::new(MemoryStore::new()),
        said: Arc::clone(&said),
    };
    (log, said)
}

/// A kill that writes down what it reached, in the same log.
fn noted(said: &Said) -> Die {
    let said = Arc::clone(said);
    Arc::new(move |reached: &str| {
        said.lock()
            .expect("the log")
            .push(format!("kill {reached}"))
    })
}

fn taken(said: &Said) -> Vec<String> {
    std::mem::take(&mut *said.lock().expect("the log"))
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    runtime()
        .expect("a runtime")
        .block_on(future)
        .expect("the runtime runs")
}

/// `put:N` falls once the Nth PUT of a segment has landed: other PUTs
/// are not counted, nor a PUT that failed, and it falls once.
#[test]
fn put_n_falls_once_the_nth_segment_put_has_landed() {
    let (inner, said) = log();
    let faults = FaultStore::new(inner);
    let store = KillStore::new(faults.clone(), Some(KillAt::Put(2)), noted(&said));
    let put = |key: &str| block_on(store.put(key, Bytes::from_static(b"x")));
    put("vols/a/node/1").expect("a node");
    put("vols/a/seg/1").expect("the first segment");
    put("vols/a/refs").expect("refs");
    block_on(faults.arm(FaultOp::Put, "seg/2", 0));
    assert!(put("vols/a/seg/2").is_err(), "a PUT that failed");
    block_on(faults.disarm());
    put("vols/a/seg/3").expect("the second segment to land");
    put("vols/a/seg/4").expect("a third");
    assert_eq!(
        taken(&said),
        [
            "put vols/a/node/1",
            "put vols/a/seg/1",
            "put vols/a/refs",
            "put vols/a/seg/3",
            "kill after segment PUT 2 landed (vols/a/seg/3)",
            "put vols/a/seg/4",
        ]
    );

    let (inner, said) = log();
    for kill in [None, Some(KillAt::Swap(1))] {
        let store = KillStore::new(inner.clone(), kill, noted(&said));
        block_on(store.put("vols/a/seg/1", Bytes::new())).expect("a segment");
    }
    assert_eq!(
        taken(&said),
        ["put vols/a/seg/1", "put vols/a/seg/1"],
        "no kill"
    );
}

/// The volume `pg-data` placed in a store in memory, its lease taken by
/// `node-a`: the refs and the epoch.
fn placed(store: &Log) -> (crate::remote::Remote, Arc<ObjectRefStore<Log>>, u64) {
    let remote =
        crate::remote::Remote::new("http://127.0.0.1:9000", "volumes", "pg-data").expect("a place");
    let refs = Arc::new(ObjectRefStore::new(store.clone()));
    let made = block_on(place(store.clone(), Arc::clone(&refs), &remote, 64 << 20));
    assert_eq!(made, Ok(Placed::Made));
    let taken = block_on(refs.lease_acquire(remote.id, "node-a", 30_000)).expect("the take");
    let LeaseOutcome::Acquired(lease) = taken else {
        panic!("not taken: {taken:?}");
    };
    (remote, refs, lease.epoch)
}

/// One cut's commit through `remote`: a block written, then the swap of
/// head and the row from `expected` to `row`.
fn commit<R: skyzfs_upload::Remote>(
    remote: &mut R,
    group: u64,
    expected: Option<&[u8]>,
    row: &[u8],
) -> std::result::Result<(), RemoteError> {
    block_on(async {
        remote.observe().await?;
        remote.write(0, &[group as u8; 4096])?;
        remote
            .commit(group, expected.map(<[u8]>::to_vec), row.to_vec())
            .await
            .map(|_| ())
    })
}

/// Through the remote the uploader makes: `swap:N` falls once the Nth
/// commit's swap has landed, and not at a commit that was refused;
/// `put:1` falls at the first commit's segment, before its swap.
#[test]
fn the_uploaders_remote_falls_after_the_nth_swap_and_at_a_segment_before_its_swap() {
    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let mut make = maker(
        store.clone(),
        Arc::clone(&refs),
        "node-a".into(),
        remote.id,
        Some(KillAt::Swap(2)),
        noted(&said),
    );
    let mut up = make(epoch);
    taken(&said);
    commit(&mut up, 7, None, b"row 7").expect("the first commit");
    let first = taken(&said);
    assert!(
        first
            .last()
            .is_some_and(|s| s.starts_with("put_if") && s.contains("refs")),
        "no kill at the first: {first:?}"
    );
    assert!(
        commit(&mut up, 8, Some(b"another row"), b"row 8").is_err(),
        "a commit refused"
    );
    up.reset();
    commit(&mut up, 8, Some(b"row 7"), b"row 8").expect("the second commit");
    let second = taken(&said);
    let n = second.len();
    assert!(n >= 2, "{second:?}");
    assert_eq!(
        second[n - 1],
        "kill after the swap of commit 2 (group 8) landed, before its cut was let go"
    );
    assert!(
        second[n - 2].starts_with("put_if") && second[n - 2].contains("refs"),
        "the swap landed first: {second:?}"
    );
    let row = block_on(refs.meta_get(remote.id, skyzfs_upload::ROW_KEY)).expect("read");
    assert_eq!(
        row.as_deref(),
        Some(&b"row 8"[..]),
        "the swap is in the refs"
    );

    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let mut make = maker(
        store.clone(),
        refs,
        "node-a".into(),
        remote.id,
        Some(KillAt::Put(1)),
        noted(&said),
    );
    let mut up = make(epoch);
    taken(&said);
    commit(&mut up, 7, None, b"row 7").expect("the commit");
    let said = taken(&said);
    let kill = said
        .iter()
        .position(|s| s.starts_with("kill after segment PUT 1 landed"))
        .unwrap_or_else(|| panic!("no kill: {said:?}"));
    assert!(
        said[kill - 1].starts_with("put ") && said[kill - 1].contains("/seg/"),
        "{said:?}"
    );
    let swap = said
        .iter()
        .position(|s| s.starts_with("put_if") && s.contains("refs"))
        .unwrap_or_else(|| panic!("no swap: {said:?}"));
    assert!(kill < swap, "the kill before the swap: {said:?}");
}

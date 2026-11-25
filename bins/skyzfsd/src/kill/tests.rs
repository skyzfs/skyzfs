//! `--kill-at-upload`'s points, with a kill that only writes down what it
//! reached: over a store in memory that writes down each write that
//! landed, so that the order of the writes and the kill shows where the
//! kill fell.

use std::ops::Range;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use skynbd_refs::{LeaseOutcome, ObjectRefStore, RefStore};
use skynbd_store::{FaultOp, FaultStore, MemoryStore, ObjectStore, Precondition, Result};
use skyzfs_blk::Buf;
use skyzfs_local::inplace::CutSlice;
use skyzfs_upload::publish::{Refusal, Request, EXPIRY_DEFAULT};
use skyzfs_upload::{Answer, Control, CutsNow, Local, Progress, Released, Store, Uploadable};

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

/// The cut of group `group` these tests commit.
fn cut(group: u64) -> CommittedCut {
    CommittedCut {
        state: [1; 16],
        number: group,
        group,
    }
}

/// One cut's commit through `remote`: a block written, then the swap of
/// head and the row from `expected` to the row of the cut of group
/// `group`.
fn commit<R: skyzfs_upload::Remote>(
    remote: &mut R,
    group: u64,
    expected: Option<&[u8]>,
) -> std::result::Result<CommitHash, RemoteError> {
    block_on(async {
        remote.observe().await?;
        remote.write(0, &[group as u8; 4096])?;
        remote
            .commit(cut(group), expected.map(<[u8]>::to_vec))
            .await
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
        None,
    );
    let mut up = make(epoch);
    taken(&said);
    commit(&mut up, 7, None).expect("the first commit");
    let row_7 = block_on(refs.meta_get(remote.id, skyzfs_upload::ROW_KEY))
        .expect("read")
        .expect("the row of the first commit");
    let first = taken(&said);
    assert!(
        first
            .last()
            .is_some_and(|s| s.starts_with("put_if") && s.contains("refs")),
        "no kill at the first: {first:?}"
    );
    assert!(
        commit(&mut up, 8, Some(b"another row")).is_err(),
        "a commit refused"
    );
    up.reset();
    let second_commit = commit(&mut up, 8, Some(&row_7)).expect("the second commit");
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
        row,
        Some(skyzfs_upload::row::encode(&cut(8), &second_commit)),
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
        None,
    );
    let mut up = make(epoch);
    taken(&said);
    commit(&mut up, 7, None).expect("the commit");
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

/// `manifest:N` falls once the Nth PUT of a snapshot's framed manifest
/// (a key under `snap/`) has landed: not at a segment's PUT, nor at the
/// ZFS manifest's, nor at one that failed, and it falls once.
#[test]
fn manifest_n_falls_once_the_nth_framed_manifest_put_has_landed() {
    let (inner, said) = log();
    let faults = FaultStore::new(inner);
    let store = KillStore::new(faults.clone(), Some(KillAt::Manifest(2)), noted(&said));
    let put = |key: &str| block_on(store.put(key, Bytes::from_static(b"x")));
    put("vols/a/seg/1").expect("a segment");
    put("vols/a/skyzfs/manifest/1").expect("a ZFS manifest");
    put("vols/a/snap/1").expect("the first framed manifest");
    block_on(faults.arm(FaultOp::Put, "snap/2", 0));
    assert!(put("vols/a/snap/2").is_err(), "a PUT that failed");
    block_on(faults.disarm());
    put("vols/a/seg/2").expect("another segment");
    put("vols/a/snap/3").expect("the second framed manifest");
    put("vols/a/snap/4").expect("a third");
    assert_eq!(
        taken(&said),
        [
            "put vols/a/seg/1",
            "put vols/a/skyzfs/manifest/1",
            "put vols/a/snap/1",
            "put vols/a/seg/2",
            "put vols/a/snap/3",
            "kill after the framed manifest PUT 2 landed (vols/a/snap/3), before its ref \
             was made",
            "put vols/a/snap/4",
        ]
    );

    let (inner, said) = log();
    for kill in [Some(KillAt::Put(1)), Some(KillAt::Publish(1))] {
        let store = KillStore::new(inner.clone(), kill, noted(&said));
        block_on(store.put("vols/a/snap/1", Bytes::new())).expect("a manifest");
    }
    assert_eq!(
        taken(&said),
        ["put vols/a/snap/1", "put vols/a/snap/1"],
        "no kill"
    );
}

/// A snapshot of the cut `number` on `commit`, named `name`.
fn publish<R: skyzfs_upload::Remote>(
    remote: &mut R,
    commit: CommitHash,
    name: &str,
    number: u64,
) -> std::result::Result<(), RemoteError> {
    let manifest = b"a ZFS manifest";
    let snap = skynbd_types::SnapId(ulid::Ulid::from(u128::from(number)));
    let row = skyzfs_upload::publish::SnapRow {
        cut: skyzfs_local::inplace::CommittedCut {
            state: [1; 16],
            number,
            group: 7,
        },
        commit,
        snap,
        manifest_len: manifest.len() as u32,
        manifest_hash: *blake3::hash(manifest).as_bytes(),
    };
    let snapshot = Snapshot {
        name,
        snap,
        row: &row,
        manifest,
    };
    block_on(remote.publish(commit, &snapshot))
}

/// Through the remote the uploader makes: `manifest:N` falls after the
/// Nth publish's framed manifest landed and before its ref's
/// transaction; `publish:N` once the Nth publish's transaction has
/// landed, and not at one that was refused.
#[test]
fn the_uploaders_remote_falls_at_a_publishs_manifest_and_after_its_transaction() {
    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let mut make = maker(
        store.clone(),
        Arc::clone(&refs),
        "node-a".into(),
        remote.id,
        Some(KillAt::Manifest(2)),
        noted(&said),
        None,
    );
    let mut up = make(epoch);
    let made = commit(&mut up, 7, None).expect("the commit");
    taken(&said);
    publish(&mut up, made, "first", 1).expect("the first publish");
    let first = taken(&said);
    assert!(
        first.iter().all(|s| !s.starts_with("kill")),
        "no kill at the first: {first:?}"
    );
    publish(&mut up, made, "second", 2).expect("the second publish");
    let second = taken(&said);
    let kill = second
        .iter()
        .position(|s| s.starts_with("kill after the framed manifest PUT 2 landed"))
        .unwrap_or_else(|| panic!("no kill: {second:?}"));
    assert!(
        second[kill - 1].starts_with("put ") && second[kill - 1].contains("/snap/"),
        "{second:?}"
    );
    assert!(
        second[..kill]
            .iter()
            .any(|s| s.starts_with("put ") && s.contains("/skyzfs/manifest/")),
        "the ZFS manifest before it: {second:?}"
    );
    assert!(
        second[kill + 1..]
            .iter()
            .any(|s| s.starts_with("put_if") && s.contains("refs")),
        "the ref's transaction after it: {second:?}"
    );

    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let mut make = maker(
        store.clone(),
        Arc::clone(&refs),
        "node-a".into(),
        remote.id,
        Some(KillAt::Publish(2)),
        noted(&said),
        None,
    );
    let mut up = make(epoch);
    let made = commit(&mut up, 7, None).expect("the commit");
    publish(&mut up, made, "first", 1).expect("the first publish");
    taken(&said);
    assert_eq!(
        publish(&mut up, made, "first", 1),
        Err(RemoteError::Conflict),
        "a publish refused"
    );
    let refused = taken(&said);
    assert!(
        refused.iter().all(|s| !s.starts_with("kill")),
        "no kill at a refused publish: {refused:?}"
    );
    publish(&mut up, made, "second", 2).expect("the second publish");
    let second = taken(&said);
    let n = second.len();
    assert!(n >= 2, "{second:?}");
    assert_eq!(
        second[n - 1],
        "kill after the transaction of publish 2 (second) landed, before the uploader knew it"
    );
    assert!(
        second[n - 2].starts_with("put_if") && second[n - 2].contains("refs"),
        "the transaction landed first: {second:?}"
    );
    let made = block_on(refs.get(remote.id, "snap/second")).expect("read");
    assert!(made.is_some(), "the ref is in the refs");
}

/// The state of the cuts in the carry tests.
const STATE: [u8; 16] = [1; 16];

/// Cut `number` of group `group` of [`STATE`], opening as itself if
/// `itself`, as the store offers it.
fn offered(number: u64, group: u64, itself: bool) -> Uploadable {
    Uploadable {
        state: STATE,
        number,
        group,
        whole: true,
        itself,
        base: None,
        rewound: 0,
    }
}

/// A store that offers the cut it is given and reads nothing: what a
/// [`KillLocal`] needs to see a cut offered and its reading begun.
#[derive(Default)]
struct Offers {
    cut: Option<Uploadable>,
}

impl Store for Offers {
    fn uploadable(&mut self) -> Option<Uploadable> {
        self.cut
    }
    fn read(&mut self, _: u64, _: u64, _: &mut Buf) -> std::io::Result<CutSlice> {
        Err(std::io::Error::other("nothing read in these tests"))
    }
    fn begin_sync_copies(&mut self, _: u64) -> std::io::Result<()> {
        Ok(())
    }
    fn copies_synced(&mut self, _: bool) -> Option<std::io::Result<()>> {
        None
    }
    fn release(&mut self, _: u64) -> Released {
        Released::LetGo
    }
    fn name_committed(&mut self, _: u64, _: u64) -> bool {
        false
    }
    fn abandon_all(&mut self) -> Vec<u64> {
        Vec::new()
    }
    fn cuts(&mut self) -> CutsNow {
        CutsNow::default()
    }
}

/// The uploader offers `cut` and begins its reading (a read from block
/// 0), through `local`.
fn reads(local: &mut KillLocal<Offers>, cut: Uploadable) {
    local.inner_mut().cut = Some(cut);
    block_on(async {
        assert_eq!(local.uploadable().await, Some(cut));
        let (_, read) = local.read(cut.number, 0, Buf::zeroed(0)).await;
        assert!(read.is_err());
    });
}

/// A publish of `name` for a cut of [`STATE`] counted after `after`, of
/// group `group` or later.
fn request(name: &str, after: u64, group: u64) -> Request {
    Request {
        name: name.into(),
        attachment: [0xa1; 16],
        state: STATE,
        after,
        group,
        manifest: Vec::new(),
        expiry: EXPIRY_DEFAULT,
    }
}

/// `request` asked through `carry` as the uploader's inbox passes it
/// on, answered `answer` as the uploader would, and the post after it:
/// the asker is given that answer.
fn asked(carry: &Carry, request: Request, answer: Answer) {
    let (reply, mut answered) = tokio::sync::oneshot::channel();
    let passed = carry.ask(Control::Ask {
        request,
        rewound: 0,
        reply,
    });
    let Control::Ask { reply, .. } = passed else {
        panic!("an ask passed on as another request: {passed:?}");
    };
    assert!(
        answered.try_recv().is_err(),
        "no answer before the uploader's"
    );
    reply
        .send(answer.clone())
        .expect("the answer is waited for");
    carry.answered();
    assert_eq!(answered.try_recv(), Ok(answer), "the asker's answer");
}

/// `carry-put:N` falls once the first segment PUT of the first cut whose
/// reading began after publish N was accepted has landed, where that cut
/// carries it: not at a cut read before the publish, nor one of a group
/// before *G*, nor one that does not open as itself, nor at a PUT that
/// is not a segment's; once; and the requests of the channel reach the
/// uploader and their answers the asker. Mutants: publish N counted
/// from the requests asked rather than accepted, the cut chosen at its
/// offer rather than its reading, the reading of the cut under way
/// taken for the chosen, a cut that does not open as itself killed at.
#[test]
fn carry_put_falls_at_the_first_segment_of_the_first_cut_read_after_publish_n_that_carries_it() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarryPut(2)), Arc::clone(&die)).expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    let put = |key: &str| block_on(store.put(key, Bytes::from_static(b"x"))).expect("landed");

    asked(&carry, request("a", 0, 1), Answer::Accepted);
    asked(
        &carry,
        request("b", 0, 1),
        Answer::Refused(Refusal::Attachment),
    );
    // Read before publish 2 is accepted, and offered again after it but
    // not read anew: not chosen, though it would carry it.
    reads(&mut local, offered(6, 10, true));
    asked(&carry, request("c", 5, 10), Answer::Accepted);
    let _ = block_on(local.uploadable());
    put("vols/a/seg/1");
    // Read after it: of a group before G, then not opening as itself.
    reads(&mut local, offered(7, 9, true));
    put("vols/a/seg/2");
    reads(&mut local, offered(8, 10, false));
    put("vols/a/seg/3");
    // The one: a read further on in it is not a new reading.
    reads(&mut local, offered(9, 11, true));
    put("vols/a/node/1");
    let _ = block_on(local.read(9, 512, Buf::zeroed(0)));
    put("vols/a/seg/4");
    put("vols/a/seg/5");
    asked(&carry, request("d", 9, 11), Answer::Accepted);
    reads(&mut local, offered(10, 12, true));
    put("vols/a/seg/6");
    assert_eq!(
        taken(&said),
        [
            "put vols/a/seg/1",
            "put vols/a/seg/2",
            "put vols/a/seg/3",
            "put vols/a/node/1",
            "put vols/a/seg/4",
            "kill carry-put:2: after the first segment PUT of cut 9 (group 11), chosen to carry \
             publish 2, landed (vols/a/seg/4)",
            "put vols/a/seg/5",
            "put vols/a/seg/6",
        ]
    );
}

/// `carry-swap:N` falls once the commit of the first cut whose reading
/// began after publish N was accepted has landed, where that cut carries
/// it, and before the cut is let go: not at a cut read before the
/// publish, nor at one that does not open as itself (the next one is
/// chosen then), nor at a segment PUT. Mutants: the chosen cut not
/// checked to carry, not chosen anew after one that does not carry, the
/// commit of another cut taken for the chosen one's.
#[test]
fn carry_swap_falls_after_the_commit_of_the_first_cut_read_after_publish_n_that_carries_it() {
    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarrySwap(1)), Arc::clone(&die)).expect("a carry hook");
    let mut make = maker(
        store.clone(),
        Arc::clone(&refs),
        "node-a".into(),
        remote.id,
        Some(KillAt::CarrySwap(1)),
        die,
        Some(Arc::clone(&carry)),
    );
    let mut up = make(epoch);
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    let row = || {
        block_on(refs.meta_get(remote.id, skyzfs_upload::ROW_KEY))
            .expect("read")
            .expect("a row")
    };
    reads(&mut local, offered(7, 7, true));
    asked(&carry, request("t", 6, 7), Answer::Accepted);
    commit(&mut up, 7, None).expect("cut 7");
    reads(&mut local, offered(8, 8, false));
    commit(&mut up, 8, Some(&row())).expect("cut 8");
    let before = taken(&said);
    assert!(
        before.iter().all(|s| !s.starts_with("kill")),
        "no kill at a cut read before the publish nor at one that does not carry it: {before:?}"
    );
    assert!(
        before.iter().any(|s| s.contains("/seg/")),
        "segments were put: {before:?}"
    );
    reads(&mut local, offered(9, 9, true));
    // The commit of another cut than the one chosen (not seen in the
    // daemon, whose commit follows the reading of its cut).
    carry.committed(CommittedCut {
        state: STATE,
        number: 10,
        group: 10,
    });
    assert_eq!(taken(&said), Vec::<String>::new(), "no kill at another cut");
    commit(&mut up, 9, Some(&row())).expect("cut 9");
    let said_9 = taken(&said);
    let n = said_9.len();
    assert!(n >= 2, "{said_9:?}");
    assert_eq!(
        said_9[n - 1],
        "kill carry-swap:1: after the commit of cut 9 (group 9), which carries publish 1, \
         landed, before the cut was let go"
    );
    assert!(
        said_9[n - 2].starts_with("put_if") && said_9[n - 2].contains("refs"),
        "the swap landed first: {said_9:?}"
    );
    reads(&mut local, offered(10, 10, true));
    commit(&mut up, 10, Some(&row())).expect("cut 10");
    let after = taken(&said);
    assert!(
        after.iter().all(|s| !s.starts_with("kill")),
        "once: {after:?}"
    );
}

/// A carry hook only for `carry-put:N` and `carry-swap:N`; an ask whose
/// asker has gone is passed on so that the uploader finds it gone and
/// takes nothing, and the other requests of the channel pass unchanged.
#[test]
fn a_carry_hook_passes_the_channel_on_as_it_was() {
    let die = noted(&Said::default());
    for kill in [
        None,
        Some(KillAt::Put(1)),
        Some(KillAt::Swap(1)),
        Some(KillAt::Manifest(1)),
        Some(KillAt::Publish(1)),
    ] {
        assert!(Carry::new(kill, Arc::clone(&die)).is_none(), "{kill:?}");
    }
    let carry = Carry::new(Some(KillAt::CarrySwap(1)), die).expect("a carry hook");
    let (reply, answered) = tokio::sync::oneshot::channel();
    drop(answered);
    let passed = carry.ask(Control::Ask {
        request: request("t", 0, 1),
        rewound: 0,
        reply,
    });
    let Control::Ask { reply, .. } = passed else {
        panic!("{passed:?}");
    };
    assert!(
        reply.is_closed(),
        "the asker's going is seen by the uploader"
    );
    let (reply, mut answered) = tokio::sync::oneshot::channel();
    let passed = carry.ask(Control::Progress("t".into(), reply));
    let Control::Progress(name, reply) = passed else {
        panic!("{passed:?}");
    };
    assert_eq!(name, "t");
    reply.send(Progress::Pending).expect("sent");
    assert_eq!(answered.try_recv(), Ok(Progress::Pending));
}

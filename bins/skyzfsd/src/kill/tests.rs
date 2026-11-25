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

/// What a carry hook says, written down in `said` as `say ...`.
fn heard(said: &Said) -> Say {
    let said = Arc::clone(said);
    Arc::new(move |line: &str| said.lock().expect("the log").push(format!("say {line}")))
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
    let lines = Said::default();
    let die = noted(&said);
    let carry = Carry::new(Some(KillAt::CarryPut(2)), Arc::clone(&die), heard(&lines))
        .expect("a carry hook");
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
    assert_eq!(
        taken(&lines),
        [
            "say carry-put:2: publish 2 (c) accepted: armed for the first cut read from now on \
             of its state, counted after 5, of group 10 or later",
            "say carry-put:2: cut 8 (group 10) chosen to carry publish 2, but it does not open \
             as itself: the hook does not fire at it",
            "say carry-put:2: cut 8 (group 10) is no longer chosen: it ended without the hook \
             firing",
            "say carry-put:2: cut 9 (group 11) chosen to carry publish 2",
        ],
        "what the hook said of its course (S1 of the review of #150)"
    );
}

/// `carry-swap:N` falls once the commit of the first cut whose reading
/// began after publish N was accepted has landed, where that cut carries
/// it, and before the cut is let go: not at a cut read before the
/// publish, nor at one that does not open as itself (the next one is
/// chosen then), nor at a segment PUT. Mutants: the chosen cut not
/// checked to carry, not chosen anew after one that does not carry, the
/// commit of another cut taken for the chosen one's, the release of
/// another cut taken for the chosen one's.
#[test]
fn carry_swap_falls_after_the_commit_of_the_first_cut_read_after_publish_n_that_carries_it() {
    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let die = noted(&said);
    let carry = Carry::new(
        Some(KillAt::CarrySwap(1)),
        Arc::clone(&die),
        heard(&Said::default()),
    )
    .expect("a carry hook");
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
    // And another cut let go while 9 is chosen.
    block_on(local.release(8));
    assert_eq!(taken(&said), Vec::<String>::new(), "no kill at another cut");
    commit(&mut up, 9, Some(&row())).expect("cut 9");
    let said_9 = taken(&said);
    let n = said_9.len();
    assert!(n >= 2, "{said_9:?}");
    assert_eq!(
        said_9[n - 1],
        "kill carry-swap:1: after the commit of cut 9 (group 9), chosen to carry publish 1, \
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
        assert!(
            Carry::new(kill, Arc::clone(&die), heard(&Said::default())).is_none(),
            "{kill:?}"
        );
    }
    let carry =
        Carry::new(Some(KillAt::CarrySwap(1)), die, heard(&Said::default())).expect("a carry hook");
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

// The cut the uploader makes publish N on, against the cut the hook
// chooses (F1, F2 and Z of the review of #150): the uploader's own
// driver, through the remote `kill::maker` makes, over a store that holds
// whole cuts. These began as the review's demonstrations of the defects
// (`r150_show_*`), and now pin the fix.

/// A store that holds whole cuts of `blocks` blocks each, read one block
/// a slice, and lets a cut go at its release; a cut's blocks may all be
/// zeroes (`zero`), read as a run of zeroes and put as no segment.
struct Holds {
    held: Vec<(Uploadable, u64, bool)>,
}

impl Store for Holds {
    fn uploadable(&mut self) -> Option<Uploadable> {
        self.held.first().map(|(cut, _, _)| *cut)
    }
    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> std::io::Result<CutSlice> {
        let Some(&(_, blocks, zero)) = self.held.iter().find(|(c, _, _)| c.number == number) else {
            return Err(std::io::Error::other("gone"));
        };
        let next = (from + 1 < blocks).then_some(from + 1);
        if zero {
            return Ok(CutSlice {
                blocks: Vec::new(),
                zeroes: vec![(from, 1)],
                next,
                copied: false,
            });
        }
        buf.set_blocks(1)
            .map_err(|_| std::io::Error::other("the buffer"))?;
        buf.fill((number + from + 1) as u8);
        Ok(CutSlice {
            blocks: vec![from],
            zeroes: Vec::new(),
            next,
            copied: false,
        })
    }
    fn begin_sync_copies(&mut self, _: u64) -> std::io::Result<()> {
        Ok(())
    }
    fn copies_synced(&mut self, _: bool) -> Option<std::io::Result<()>> {
        Some(Ok(()))
    }
    fn release(&mut self, number: u64) -> Released {
        self.held.retain(|(c, _, _)| c.number != number);
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

type Rig = skyzfs_upload::Driver<
    KillLocal<Holds>,
    KillRemote<
        EngineRemote<
            KillStore<FaultStore<Log>>,
            Arc<skynbd_refs::ActingRefStore<Arc<ObjectRefStore<FaultStore<Log>>>>>,
        >,
    >,
>;

/// The uploader's driver over `held`, with the remote the daemon's
/// test-hooks build makes for `kill` (`kill::maker`), over a store in
/// memory that can fail a write after it has landed: the driver, the
/// hook, the faults, and the log, in which the hook's lines are written
/// down as `say ...` and its kill as `kill ...`.
fn rig(
    kill: KillAt,
    held: Vec<(Uploadable, u64, bool)>,
) -> (Rig, Arc<Carry>, FaultStore<Log>, Said) {
    let (store, said) = log();
    let (remote, _, epoch) = placed(&store);
    let faults = FaultStore::new(store);
    let refs = Arc::new(ObjectRefStore::new(faults.clone()));
    let die = noted(&said);
    let carry = Carry::new(Some(kill), Arc::clone(&die), heard(&said)).expect("a carry hook");
    let mut make = maker(
        faults.clone(),
        refs,
        "node-a".into(),
        remote.id,
        Some(kill),
        die,
        Some(Arc::clone(&carry)),
    );
    let driver = skyzfs_upload::Driver::with_slices(
        KillLocal::new(Holds { held }, Arc::clone(&carry)),
        make(epoch),
        1,
    )
    .with_publishing(skyzfs_upload::Publishing::new(
        Some([0xa1; 16]),
        Some(STATE),
    ));
    let _ = taken(&said);
    (driver, carry, faults, said)
}

/// Step until the hook kills or `n` steps: the steps, by their debug form.
fn steps(driver: &mut Rig, said: &Said, n: usize) -> Vec<String> {
    let mut seen = Vec::new();
    for _ in 0..n {
        let step = block_on(driver.step());
        seen.push(format!("{step:?}"));
        if said
            .lock()
            .expect("the log")
            .iter()
            .any(|s| s.starts_with("kill"))
        {
            break;
        }
    }
    seen
}

/// Accept publish `t` (C 6, G 7) in the driver's publishes and in the
/// hook, as the daemon's thread would.
fn accept(driver: &mut Rig, carry: &Carry) {
    assert_eq!(
        driver.publishing().ask(request("t", 6, 7)),
        Answer::Accepted
    );
    asked(carry, request("t", 6, 7), Answer::Accepted);
}

/// Whether one of `seen`'s steps made a publish on cut `number`.
fn published_on(seen: &[String], number: u64) -> bool {
    let number = format!("number: {number}");
    seen.iter()
        .any(|s| s.contains("Published {") && s.contains(&number))
}

/// The hook's lines and its kill in `log`.
fn hooks(log: &[String]) -> Vec<String> {
    log.iter()
        .filter(|s| s.starts_with("say ") || s.starts_with("kill "))
        .cloned()
        .collect()
}

const ARMED: &str = "publish 1 (t) accepted: armed for the first cut read from now on of its \
                     state, counted after 6, of group 7 or later";

/// F2: the chosen cut's commit lands and its call comes back failed (the
/// refs object's write takes effect, then an error). The driver finds the
/// row naming the cut and lets it go without the remote answering Ok:
/// `carry-swap` fires as the cut is let go, before the publish is made on
/// it. Mutant: no fire at the release (the hook then gives up at the
/// publish, with no kill).
#[test]
fn carry_swap_fires_as_the_chosen_cut_is_let_go_where_its_commits_call_failed() {
    let held = vec![
        (offered(7, 7, true), 1, false),
        (offered(8, 8, true), 1, false),
    ];
    let (mut driver, carry, faults, said) = rig(KillAt::CarrySwap(1), held);
    accept(&mut driver, &carry);
    // The lookup, the check of 7 and its read.
    let mut seen = steps(&mut driver, &said, 3);
    // Cut 7's commit: the swap lands, its answer is an error.
    block_on(faults.arm_after(FaultOp::Put, "/refs", 0));
    seen.extend(steps(&mut driver, &said, 1));
    block_on(faults.disarm());
    seen.extend(steps(&mut driver, &said, 12));
    assert!(
        seen.iter().any(|s| s.contains("injected fault")),
        "the commit's call failed: {seen:?}"
    );
    assert!(
        !published_on(&seen, 7),
        "killed before the publish: {seen:?}"
    );
    assert_eq!(
        hooks(&taken(&said)),
        [
            format!("say carry-swap:1: {ARMED}"),
            "say carry-swap:1: cut 7 (group 7) chosen to carry publish 1".into(),
            "kill carry-swap:1: after the commit of cut 7 (group 7), chosen to carry publish 1, \
             landed (its call failed; seen as the cut was let go), before the cut was let go"
                .into(),
        ]
    );
}

/// F1, case A, for each kill point: the carrier's reading began before
/// publish 1 was accepted, and the publish is made on it. The hook, which
/// chose the next cut, gives up when the publish is made, and kills
/// nowhere. Mutant: the check on the publish missing (the hook kills at
/// cut 8).
#[test]
fn a_publish_made_on_a_cut_read_before_its_acceptance_gives_the_hook_up() {
    for kill in [KillAt::CarrySwap(1), KillAt::CarryPut(1)] {
        let held = vec![
            (offered(7, 7, true), 2, false),
            (offered(8, 8, true), 1, false),
        ];
        let (mut driver, carry, _faults, said) = rig(kill, held);
        // The check of 7 and its first slice, then the acceptance.
        let mut seen = steps(&mut driver, &said, 2);
        accept(&mut driver, &carry);
        seen.extend(steps(&mut driver, &said, 14));
        assert!(published_on(&seen, 7), "{kill:?}: {seen:?}");
        let point = match kill {
            KillAt::CarryPut(_) => "carry-put:1",
            _ => "carry-swap:1",
        };
        let lines = hooks(&taken(&said));
        assert!(
            lines.iter().all(|s| !s.starts_with("kill")),
            "{kill:?}: no kill: {lines:?}"
        );
        assert!(
            lines.contains(&format!(
                "say {point}: publish 1 (t) was made on cut 7 (group 7), which the hook did not \
                 choose, before the hook fired; no kill"
            )),
            "{kill:?}: {lines:?}"
        );
    }
}

/// F1, case B: the carrier was committed before publish 1 was accepted;
/// the lookup gives it the publish at once, and the hook gives up there.
#[test]
fn a_publish_made_on_a_cut_committed_before_its_acceptance_gives_the_hook_up() {
    let held = vec![
        (offered(7, 7, true), 1, false),
        (offered(8, 8, true), 1, false),
    ];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarrySwap(1), held);
    let mut seen = steps(&mut driver, &said, 3);
    accept(&mut driver, &carry);
    seen.extend(steps(&mut driver, &said, 14));
    assert!(published_on(&seen, 7), "{seen:?}");
    let lines = hooks(&taken(&said));
    assert!(lines.iter().all(|s| !s.starts_with("kill")), "{lines:?}");
    assert_eq!(
        lines.last().map(String::as_str),
        Some(
            "say carry-swap:1: publish 1 (t) was made on cut 7 (group 7), which the hook did not \
             choose, before the hook fired; no kill"
        ),
        "{lines:?}"
    );
}

/// Z: the chosen cut puts no segment (its blocks are zeroes), so
/// `carry-put` does not fire at it; the publish is made on it, and the
/// hook gives up there rather than kill at the next cut.
#[test]
fn a_publish_made_on_the_chosen_cut_with_no_segment_gives_carry_put_up() {
    let held = vec![
        (offered(7, 7, true), 1, true),
        (offered(8, 8, true), 1, false),
    ];
    let (mut driver, carry, _faults, said) = rig(KillAt::CarryPut(1), held);
    accept(&mut driver, &carry);
    let seen = steps(&mut driver, &said, 16);
    assert!(published_on(&seen, 7), "{seen:?}");
    let lines = hooks(&taken(&said));
    assert!(lines.iter().all(|s| !s.starts_with("kill")), "{lines:?}");
    assert!(
        lines.contains(
            &"say carry-put:1: publish 1 (t) was made on cut 7 (group 7), the cut chosen, \
              before the hook fired; no kill"
                .to_string()
        ),
        "{lines:?}"
    );
}

/// The course the hook is for, for each kill point: the carrier is read
/// after the acceptance, and the hook fires at it before the publish is
/// made. This also shows that `kill::maker` gives the hook to the objects
/// and to the remote (M13 of the review of #150).
#[test]
fn the_carry_hooks_fire_at_the_carrier_read_after_the_acceptance() {
    for (kill, line) in [
        (
            KillAt::CarrySwap(1),
            "kill carry-swap:1: after the commit of cut 7 (group 7), chosen to carry publish 1, \
             landed, before the cut was let go",
        ),
        (
            KillAt::CarryPut(1),
            "kill carry-put:1: after the first segment PUT of cut 7 (group 7), chosen to carry \
             publish 1, landed",
        ),
    ] {
        let held = vec![
            (offered(7, 7, true), 1, false),
            (offered(8, 8, true), 1, false),
        ];
        let (mut driver, carry, _faults, said) = rig(kill, held);
        accept(&mut driver, &carry);
        let seen = steps(&mut driver, &said, 16);
        let lines = hooks(&taken(&said));
        assert!(
            lines.last().is_some_and(|s| s.starts_with(line)),
            "{kill:?}: {lines:?}"
        );
        assert!(
            !seen.iter().any(|s| s.contains("Published {")),
            "{kill:?}: killed before the publish's transaction: {seen:?}"
        );
    }
}

/// F1: publish N ends without being made (here at its deadline) before
/// the hook fires: the hook gives up with a line and kills nowhere; the
/// end of another publish changes nothing. Mutant: the post's ended
/// publishes not read.
#[test]
fn a_publish_n_that_ends_without_being_made_gives_the_hook_up() {
    let (store, said) = log();
    let (remote, refs, epoch) = placed(&store);
    let die = noted(&said);
    let carry =
        Carry::new(Some(KillAt::CarrySwap(1)), Arc::clone(&die), heard(&said)).expect("a hook");
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
    asked(&carry, request("t", 6, 7), Answer::Accepted);
    reads(&mut local, offered(7, 7, true));
    crate::upload::Around::posted(&carry, &[("u".into(), Progress::Expired)]);
    crate::upload::Around::posted(&carry, &[("t".into(), Progress::Expired)]);
    commit(&mut up, 7, None).expect("cut 7");
    assert_eq!(
        hooks(&taken(&said)),
        [
            format!("say carry-swap:1: {ARMED}"),
            "say carry-swap:1: cut 7 (group 7) chosen to carry publish 1".into(),
            "say carry-swap:1: publish 1 (t) ended (expired) before the hook fired; no kill".into(),
        ]
    );
}

/// S2 (open point 3): publish N is the Nth name accepted; a name asked
/// again and accepted again is not counted again; and another publish
/// made before the hook fires does not give it up. Mutants: every
/// acceptance counted (the hook arms on the second ask of "a"), any
/// publish made giving the hook up (it kills nowhere).
#[test]
fn a_name_accepted_again_is_not_counted_again() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry =
        Carry::new(Some(KillAt::CarryPut(2)), Arc::clone(&die), heard(&said)).expect("a hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    assert_eq!(carry.accepted(), 1);
    reads(&mut local, offered(6, 10, true));
    block_on(store.put("vols/a/seg/1", Bytes::from_static(b"x"))).expect("landed");
    assert_eq!(taken(&said), ["put vols/a/seg/1"], "not armed");
    asked(&carry, request("b", 6, 10), Answer::Accepted);
    assert_eq!(carry.accepted(), 2);
    // Publish 1 ("a") is made: not publish 2, so the hook goes on.
    carry.published(
        "a",
        CommittedCut {
            state: STATE,
            number: 6,
            group: 10,
        },
    );
    reads(&mut local, offered(7, 10, true));
    block_on(store.put("vols/a/seg/2", Bytes::from_static(b"x"))).expect("landed");
    assert_eq!(
        taken(&said),
        [
            "say carry-put:2: publish 2 (b) accepted: armed for the first cut read from now on \
             of its state, counted after 6, of group 10 or later",
            "say carry-put:2: cut 7 (group 10) chosen to carry publish 2",
            "put vols/a/seg/2",
            "kill carry-put:2: after the first segment PUT of cut 7 (group 10), chosen to carry \
             publish 2, landed (vols/a/seg/2)",
        ]
    );
}

/// C and G at their bounds: a cut numbered C, even of a group after G,
/// is not chosen, nor one counted after C of a group before G; the first
/// cut numbered C + 1 of group G is, and the hook fires there. Mutants
/// M3 (a cut numbered C chosen), M4 (a cut of group G not chosen) and M6
/// (C not checked) of the review of #150.
#[test]
fn a_cut_numbered_c_plus_1_of_group_g_is_chosen_and_none_before() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(
        Some(KillAt::CarryPut(1)),
        Arc::clone(&die),
        heard(&Said::default()),
    )
    .expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    // Cut 5 (= C) of group 12: not counted after C.
    reads(&mut local, offered(5, 12, true));
    block_on(store.put("vols/a/seg/1", Bytes::from_static(b"x"))).expect("landed");
    // Cut 6 (= C + 1) of group 9 (< G): not chosen.
    reads(&mut local, offered(6, 9, true));
    block_on(store.put("vols/a/seg/2", Bytes::from_static(b"x"))).expect("landed");
    // Cut 7 of group 10 (= G): chosen.
    reads(&mut local, offered(7, 10, true));
    block_on(store.put("vols/a/seg/3", Bytes::from_static(b"x"))).expect("landed");
    assert_eq!(
        taken(&said),
        [
            "put vols/a/seg/1",
            "put vols/a/seg/2",
            "put vols/a/seg/3",
            "kill carry-put:1: after the first segment PUT of cut 7 (group 10), chosen to carry \
             publish 1, landed (vols/a/seg/3)",
        ]
    );
}

/// The cut under way at the acceptance, read on from a later block after
/// it, is not chosen: only a reading from block 0 begins a cut's
/// reading. The next cut is chosen. Mutant M7 of the review of #150
/// (every read counted as a new reading).
#[test]
fn a_cut_under_way_read_on_after_the_acceptance_is_not_chosen() {
    let (inner, said) = log();
    let die = noted(&said);
    let carry = Carry::new(
        Some(KillAt::CarryPut(1)),
        Arc::clone(&die),
        heard(&Said::default()),
    )
    .expect("a carry hook");
    let store = KillStore::new(inner, None, Arc::clone(&die)).carrying(Some(Arc::clone(&carry)));
    let mut local = KillLocal::new(Offers::default(), Arc::clone(&carry));
    reads(&mut local, offered(6, 10, true));
    asked(&carry, request("a", 5, 10), Answer::Accepted);
    let _ = block_on(local.read(6, 512, Buf::zeroed(0)));
    block_on(store.put("vols/a/seg/1", Bytes::from_static(b"x"))).expect("landed");
    reads(&mut local, offered(7, 10, true));
    block_on(store.put("vols/a/seg/2", Bytes::from_static(b"x"))).expect("landed");
    assert_eq!(
        taken(&said),
        [
            "put vols/a/seg/1",
            "put vols/a/seg/2",
            "kill carry-put:1: after the first segment PUT of cut 7 (group 10), chosen to carry \
             publish 1, landed (vols/a/seg/2)",
        ]
    );
}

/// R2 and R1 of the review of #150, for each kill point: publish 1's
/// transaction is asked on cut 7 and its call fails, after it landed
/// (the refs object's write takes effect, then an error: R2) or before
/// (R1). The hook cannot tell the two apart and gives up, saying so: it
/// kills nowhere, though the publish waits its first second while cut 8
/// is read and committed. Once the wait is over the publish is made on
/// cut 7 (found there in R2). The post that follows ends publish 1, and
/// the hook says nothing more (G1). Mutant: `KillRemote::publish`
/// returns at the error without telling the hook (the hook kills at
/// cut 8).
#[test]
fn a_publish_n_whose_call_failed_gives_the_hook_up() {
    for kill in [KillAt::CarrySwap(1), KillAt::CarryPut(1)] {
        for landed in [true, false] {
            let held = vec![
                (offered(7, 7, true), 2, false),
                (offered(8, 8, true), 1, false),
            ];
            let (mut driver, carry, faults, said) = rig(kill, held);
            // The check of 7 and its first slice, the acceptance, then
            // the lookup, the last slice of 7 and its commit.
            let mut seen = steps(&mut driver, &said, 2);
            accept(&mut driver, &carry);
            seen.extend(steps(&mut driver, &said, 3));
            if landed {
                block_on(faults.arm_after(FaultOp::Put, "/refs", 0));
            } else {
                block_on(faults.arm(FaultOp::Put, "/refs", 0));
            }
            let failed = block_on(driver.step());
            block_on(faults.disarm());
            assert!(
                failed.is_err(),
                "{kill:?} {landed}: the call failed: {failed:?}"
            );
            let wait = driver
                .publish_failed(tokio::time::Instant::now())
                .expect("the publish waits");
            // Cut 8: checked, read and committed while the publish waits.
            seen.extend(steps(&mut driver, &said, 3));
            assert!(wait <= std::time::Duration::from_secs(30), "{wait:?}");
            std::thread::sleep(wait + std::time::Duration::from_millis(50));
            seen.extend(steps(&mut driver, &said, 3));
            assert!(published_on(&seen, 7), "{kill:?} {landed}: {seen:?}");
            assert_eq!(
                landed,
                seen.iter().any(|s| s.contains("found: true")),
                "{kill:?} {landed}: found made where it landed: {seen:?}"
            );
            crate::upload::Around::posted(&carry, &driver.publishing().take_ended());
            let point = match kill {
                KillAt::CarryPut(_) => "carry-put:1",
                _ => "carry-swap:1",
            };
            assert_eq!(
                hooks(&taken(&said)),
                [
                    format!("say {point}: {ARMED}"),
                    format!(
                        "say {point}: publish 1 (t) may have been made (its call failed) on cut \
                         7 (group 7), which the hook did not choose, before the hook fired; no \
                         kill"
                    ),
                ],
                "{kill:?} {landed}: {seen:?}"
            );
        }
    }
}

/// One step of the cut, or of the publishes, in the order the uploader's
/// turns give them (`Turns::next` in `skyzfs-upload`'s `uploader.rs`),
/// not a publish first as `Driver::step` does.
fn cut_step(driver: &mut Rig, seen: &mut Vec<String>) {
    let step = block_on(driver.step_cut());
    seen.push(format!("cut: {step:?}"));
}

fn publish_step(driver: &mut Rig, seen: &mut Vec<String>) {
    let step = block_on(driver.step_publish());
    seen.push(format!("publish: {step:?}"));
}

/// T of the review of #150, the window the module docs describe, for
/// each kill point and in both cases: publish 1 accepted while cut 7, its
/// carrier, uploads (case A) or after cut 7 committed (case B). The
/// lookup takes a turn; the publish is then due on cut 7, but the cut has
/// the next turns, as the uploader gives them where the lookup took
/// longer than the cut's turns: cut 8 is checked, read (chosen) and
/// committed before the publish's transaction. The hook kills at cut 8,
/// and the publish's next step would make it on cut 7. No fault is
/// needed. (After the restart, the publish asked again is made on cut 8,
/// the cut the kill line names: the module docs.)
#[test]
fn the_uploaders_turns_can_have_the_hook_kill_at_a_cut_after_the_carrier() {
    for kill in [KillAt::CarrySwap(1), KillAt::CarryPut(1)] {
        for case_b in [false, true] {
            let held = vec![
                (offered(7, 7, true), if case_b { 1 } else { 2 }, false),
                (offered(8, 8, true), 1, false),
            ];
            let (mut driver, carry, _faults, said) = rig(kill, held);
            let mut seen = Vec::new();
            cut_step(&mut driver, &mut seen); // Began 7
            cut_step(&mut driver, &mut seen); // Read 7, its first slice
            if case_b {
                cut_step(&mut driver, &mut seen); // Uploaded 7
            }
            accept(&mut driver, &carry);
            publish_step(&mut driver, &mut seen); // the lookup
            if !case_b {
                cut_step(&mut driver, &mut seen); // Read 7, its last slice
                cut_step(&mut driver, &mut seen); // Uploaded 7
            }
            let due = driver.publish_due();
            cut_step(&mut driver, &mut seen); // Began 8
            cut_step(&mut driver, &mut seen); // Read 8: chosen
            cut_step(&mut driver, &mut seen); // Uploaded 8
            let lines = hooks(&taken(&said));
            publish_step(&mut driver, &mut seen); // the publish's transaction
            assert!(due, "{kill:?} {case_b}: due when cut 8 was checked");
            assert!(
                lines
                    .last()
                    .is_some_and(|s| s.starts_with("kill") && s.contains("of cut 8 (group 8)")),
                "{kill:?} {case_b}: the kill fell at cut 8: {lines:?}"
            );
            assert!(
                published_on(&seen, 7),
                "{kill:?} {case_b}: publish 1 made on cut 7: {seen:?}"
            );
        }
    }
}

/// A store that holds whole cuts, as `Holds`, and writes its releases
/// down in the log, so that the log shows whether a kill came before the
/// cut was let go.
struct HoldsSaid {
    holds: Holds,
    said: Said,
}

impl Store for HoldsSaid {
    fn uploadable(&mut self) -> Option<Uploadable> {
        Store::uploadable(&mut self.holds)
    }
    fn read(&mut self, number: u64, from: u64, buf: &mut Buf) -> std::io::Result<CutSlice> {
        Store::read(&mut self.holds, number, from, buf)
    }
    fn begin_sync_copies(&mut self, number: u64) -> std::io::Result<()> {
        Store::begin_sync_copies(&mut self.holds, number)
    }
    fn copies_synced(&mut self, wait: bool) -> Option<std::io::Result<()>> {
        Store::copies_synced(&mut self.holds, wait)
    }
    fn release(&mut self, number: u64) -> Released {
        self.said
            .lock()
            .expect("the log")
            .push(format!("release {number}"));
        Store::release(&mut self.holds, number)
    }
    fn name_committed(&mut self, number: u64, group: u64) -> bool {
        Store::name_committed(&mut self.holds, number, group)
    }
    fn abandon_all(&mut self) -> Vec<u64> {
        Store::abandon_all(&mut self.holds)
    }
    fn cuts(&mut self) -> CutsNow {
        Store::cuts(&mut self.holds)
    }
}

/// F2, the order its line claims (G2 of the review of #150): where the
/// chosen cut's commit lands and its call fails, `carry-swap` fires as
/// the cut is let go, before it is. The test of the review, taken.
/// Mutant: the hook told after the inner release.
#[test]
fn carry_swap_at_the_release_fires_before_the_cut_is_let_go() {
    let (store, said) = log();
    let (remote, _, epoch) = placed(&store);
    let faults = FaultStore::new(store);
    let refs = Arc::new(ObjectRefStore::new(faults.clone()));
    let die = noted(&said);
    let kill = KillAt::CarrySwap(1);
    let carry = Carry::new(Some(kill), Arc::clone(&die), heard(&said)).expect("a carry hook");
    let mut make = maker(
        faults.clone(),
        refs,
        "node-a".into(),
        remote.id,
        Some(kill),
        die,
        Some(Arc::clone(&carry)),
    );
    let holds = HoldsSaid {
        holds: Holds {
            held: vec![
                (offered(7, 7, true), 1, false),
                (offered(8, 8, true), 1, false),
            ],
        },
        said: Arc::clone(&said),
    };
    let mut driver = skyzfs_upload::Driver::with_slices(
        KillLocal::new(holds, Arc::clone(&carry)),
        make(epoch),
        1,
    )
    .with_publishing(skyzfs_upload::Publishing::new(
        Some([0xa1; 16]),
        Some(STATE),
    ));
    let _ = taken(&said);
    assert_eq!(
        driver.publishing().ask(request("t", 6, 7)),
        Answer::Accepted
    );
    asked(&carry, request("t", 6, 7), Answer::Accepted);
    for _ in 0..3 {
        let _ = block_on(driver.step());
    }
    block_on(faults.arm_after(FaultOp::Put, "/refs", 0));
    let _ = block_on(driver.step());
    block_on(faults.disarm());
    let _ = block_on(driver.step());
    let log = taken(&said);
    let kill = log
        .iter()
        .position(|s| s.starts_with("kill "))
        .expect("a kill");
    let release = log
        .iter()
        .position(|s| s == "release 7")
        .expect("cut 7 let go");
    assert!(kill < release, "killed before the cut was let go: {log:?}");
}

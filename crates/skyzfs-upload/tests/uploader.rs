//! The uploader's loop (M2.4c): the lease, the backoff and the driver
//! together, against the in-place store on the simulated disk and the real
//! engine on a store in memory. Time is tokio's, paused: a test moves it.
//! The lease's clock is the refs object's own, moved by the test apart
//! from tokio's.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skynbd_refs::{ActingRefStore, LeaseOutcome, ObjectRefStore, RefStore, RefValue, RenewOutcome};
use skynbd_store::FaultOp;
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_local::inplace::{CommittedCut, InPlace};
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::lease::Keeper;
use skyzfs_upload::publish::{Found, Snapshot, EXPIRY_DEFAULT, EXPIRY_MAX};
use skyzfs_upload::remote::{Observed, Remote, RemoteError};
use skyzfs_upload::uploader::{Control, State, Uploader, AT_REST, IDLE};
use skyzfs_upload::{
    row, Answer, Doing, EngineRemote, LeaseState, Mismatch, Progress, Publishing, Refusal, Request,
};

use common::*;

const TTL: u64 = 30_000;
const HOLDER: &str = "node-a";

type LeaseRefs = Arc<ObjectRefStore<Store>>;
type Fenced = EngineRemote<Store, Arc<ActingRefStore<LeaseRefs>>>;

/// The refs object's clock, apart from tokio's.
#[derive(Clone)]
struct Clock(Arc<AtomicU64>);

impl Clock {
    fn new() -> Self {
        Self(Arc::new(AtomicU64::new(1_000_000)))
    }

    fn pass(&self, ms: u64) {
        self.0.fetch_add(ms, Ordering::SeqCst);
    }

    fn refs(&self, fx: &Fixture) -> LeaseRefs {
        self.refs_on(fx.store.clone())
    }

    fn refs_on(&self, store: Store) -> LeaseRefs {
        let now = Arc::clone(&self.0);
        Arc::new(
            ObjectRefStore::new(store).with_clock(Arc::new(move || now.load(Ordering::SeqCst))),
        )
    }
}

/// The remote the uploader makes at its first take: the engine, with
/// every commit fenced by `HOLDER` at `epoch`.
fn fenced(fx: &Fixture, clock: &Clock) -> impl FnMut(u64) -> Fenced {
    let (store, vol, refs) = (fx.store.clone(), fx.vol, clock.refs(fx));
    move |epoch| {
        let acting = Arc::new(ActingRefStore::new(Arc::clone(&refs), HOLDER, epoch));
        EngineRemote::new(store.clone(), acting, vol, cfg())
    }
}

fn keeper(fx: &Fixture, clock: &Clock) -> Keeper<LeaseRefs> {
    Keeper::new(clock.refs(fx), fx.vol, HOLDER, TTL)
}

/// Turn until the uploader is idle, moving tokio's time by each wait; at
/// most `limit` turns.
async fn until_idle<L, R, F, M>(up: &mut Uploader<L, R, F, M>, limit: usize) -> Vec<State>
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let mut states = Vec::new();
    for _ in 0..limit {
        let wait = up.turn().await;
        let state = up.shown().state;
        states.push(state.clone());
        if state == State::Idle {
            return states;
        }
        tokio::time::advance(wait).await;
    }
    panic!("not idle after {limit} turns: {states:?}");
}

/// A cut of group 7 held: block 10 written.
fn one_cut(fx: &mut Fixture) -> InPlace<SimDisk> {
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 7);
    store
}

#[tokio::test(start_paused = true)]
async fn no_lease_no_upload_and_the_cut_goes_up_once_the_lease_is_held() {
    let mut fx = Fixture::new(41).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);

    // The store is out of reach: no lease, and the driver is not made.
    for want in [1, 2, 4] {
        let wait = up.turn().await;
        assert!(
            matches!(&up.shown().state, State::NoLease(why) if why.contains("injected")),
            "{:?}",
            up.shown().state
        );
        assert!(up.driver().is_none(), "no driver before the lease");
        assert_eq!(wait, Duration::from_secs(want), "the take's backoff");
        tokio::time::advance(wait).await;
    }
    fx.store.disarm().await;
    assert_eq!(fx.uploaded().await.group, None, "nothing went up");

    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "once the lease is held").await;
    let shown = up.shown();
    assert!(matches!(shown.lease, LeaseState::Held { epoch: 1, .. }));
    assert_eq!(shown.figures.uploaded, 1);
}

#[tokio::test(start_paused = true)]
async fn another_holders_lease_stops_the_uploads_for_good() {
    let mut fx = Fixture::new(42).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let other = clock
        .refs(&fx)
        .lease_acquire(fx.vol, "node-b", TTL)
        .await
        .expect("the other's take");
    assert!(matches!(other, LeaseOutcome::Acquired(_)));
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let request = asked(state, "t", 0, 7);
    assert_eq!(up.publishing().ask(request.clone()), Answer::Accepted);

    let lost = State::LeaseLost {
        holder: "node-b".into(),
        epoch: 1,
    };
    assert_eq!(up.turn().await, AT_REST);
    assert_eq!(up.shown().state, lost);
    // A publish pending ends, and none is taken after (F4 of #122's
    // review).
    assert_eq!(
        up.publishing().progress("t"),
        Progress::Refused(Refusal::Stopped)
    );
    assert_eq!(
        up.publishing().ask(request),
        Answer::Refused(Refusal::Stopped)
    );
    // The other's lease expires: it is still not taken back.
    clock.pass(TTL * 3);
    for _ in 0..5 {
        tokio::time::advance(AT_REST).await;
        up.turn().await;
        assert_eq!(up.shown().state, lost);
    }
    assert!(up.driver().is_none());
    assert_eq!(fx.uploaded().await.group, None, "nothing went up");
    let row = clock.refs(&fx).lease_get(fx.vol).await.unwrap().unwrap();
    assert_eq!((row.holder.as_str(), row.epoch), ("node-b", 1));
}

#[tokio::test(start_paused = true)]
async fn a_commit_the_fence_refuses_loses_the_lease_and_lands_nothing() {
    let mut fx = Fixture::new(43).await;
    let mut store = one_cut(&mut fx);
    // The second cut, held before the upload begins.
    fx.write(&mut store, 20, 0x20);
    fx.group(&mut store, 8);
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    // The first cut goes up.
    for _ in 0..100 {
        up.turn().await;
        if up.shown().figures.uploaded == 1 {
            break;
        }
    }
    fx.assert_uploaded(7, "the first cut").await;
    // This holder's lease expires unrenewed (tokio's time does not move,
    // so no renewal comes) and another holder takes it.
    clock.pass(TTL + 1);
    let other = clock
        .refs(&fx)
        .lease_acquire(fx.vol, "node-b", TTL)
        .await
        .unwrap();
    assert!(matches!(other, LeaseOutcome::Acquired(l) if l.epoch == 2));

    let mut states = Vec::new();
    for _ in 0..100 {
        up.turn().await;
        states.push(up.shown().state);
        if matches!(states.last(), Some(State::LeaseLost { .. })) {
            break;
        }
    }
    assert_eq!(
        states.last(),
        Some(&State::LeaseLost {
            holder: "node-b".into(),
            epoch: 2
        }),
        "{states:?}"
    );
    assert!(
        states
            .iter()
            .any(|s| matches!(s, State::Working(Doing::Committing, Some(2)))),
        "the fence refused the commit itself: {states:?}"
    );
    fx.assert_uploaded(7, "head is still the first cut").await;
    assert_eq!(up.shown().figures.uploaded, 1);
    // Lost for good.
    tokio::time::advance(AT_REST).await;
    assert_eq!(up.turn().await, AT_REST);
    assert!(matches!(up.shown().state, State::LeaseLost { .. }));
}

/// A conflict from the read after a refused commit, which no remote
/// returns, is waited after as an error: the uploader never turns twice
/// in a row without a wait for a conflict.
#[tokio::test(start_paused = true)]
async fn a_conflict_right_after_a_refused_commit_is_waited_after() {
    let mut fx = Fixture::new(63).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let refuse = Arc::new(AtomicBool::new(true));
    let commits = Arc::new(AtomicU64::new(0));
    let make = {
        let (mut fenced, refuse, commits) = (fenced(&fx, &clock), refuse.clone(), commits.clone());
        move |epoch| Refusing {
            inner: fenced(epoch),
            refuse: refuse.clone(),
            observe_refused: true,
            commits: commits.clone(),
            sent: Arc::new(Mutex::new(Vec::new())),
        }
    };
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), make, 16);
    // Turn until the commit is refused, which waits nothing.
    let mut refused = None;
    for _ in 0..100 {
        let wait = up.turn().await;
        if commits.load(Ordering::SeqCst) == 1 {
            refused = Some((up.shown().state, wait));
            break;
        }
        tokio::time::advance(wait).await;
    }
    assert_eq!(
        refused,
        Some((State::Working(Doing::Committing, Some(1)), Duration::ZERO)),
        "the refusal"
    );
    // The read after it is refused too: an error, waited after.
    let wait = up.turn().await;
    assert!(
        matches!(&up.shown().state, State::Backoff { why, wait }
            if why.starts_with("the commit was refused") && *wait == SECOND),
        "{:?}",
        up.shown().state
    );
    assert_eq!(wait, SECOND, "the read's conflict waits");
    assert_eq!(commits.load(Ordering::SeqCst), 1, "no commit sent again");
}

/// A remote whose every `every`th PUT of sealed segments fails, the
/// segments staying in the engine for the next.
struct Flaky<R> {
    inner: R,
    every: u64,
    puts: Arc<AtomicU64>,
}

impl<R: Remote> Remote for Flaky<R> {
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        self.inner.observe().await
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        self.inner.write(offset, data)
    }

    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }

    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        let n = self.puts.fetch_add(1, Ordering::SeqCst) + 1;
        if n.is_multiple_of(self.every) {
            return Err(RemoteError::Store(format!("PUT {n} failed")));
        }
        self.inner.put_sealed().await
    }

    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> Result<CommitHash, RemoteError> {
        self.inner.commit(cut, expected).await
    }

    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        self.inner.snapshot(name).await
    }

    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError> {
        self.inner.publish(commit, snapshot).await
    }

    fn discard(&mut self) {
        self.inner.discard()
    }

    fn reset(&mut self) {
        self.inner.reset()
    }
}

/// An error's wait starts again at any step that succeeds (the lead's
/// decision on F3 of #117's review): with every 20th PUT failing, a cut
/// goes up waiting a second at each failure, and no longer.
#[tokio::test(start_paused = true)]
async fn a_put_that_fails_now_and_then_waits_a_second_each_time_and_the_cut_goes_up() {
    let mut fx = Fixture::new(62).await;
    let mut store = fx.start();
    for block in 10..50u8 {
        fx.write(&mut store, u64::from(block), block);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let puts = Arc::new(AtomicU64::new(0));
    let make = {
        let (mut fenced, puts) = (fenced(&fx, &clock), puts.clone());
        move |epoch| Flaky {
            inner: fenced(epoch),
            every: 20,
            puts: puts.clone(),
        }
    };
    // One block a slice: two PUTs a slice, so that a cut makes several
    // dozen.
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), make, 1);

    let began = tokio::time::Instant::now();
    let (mut waits, mut last) = (Vec::new(), None);
    for _ in 0..1000 {
        let wait = up.turn().await;
        let state = up.shown().state;
        if let State::Backoff { why, wait: w } = &state {
            assert!(why.contains("failed"), "{why}");
            if last.as_ref() != Some(&state) {
                waits.push(*w);
            }
        }
        if state == State::Idle && up.shown().figures.uploaded == 1 {
            break;
        }
        last = Some(state);
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
    fx.assert_uploaded(7, "with every 20th PUT failing").await;
    let puts = puts.load(Ordering::SeqCst);
    let failed = puts / 20;
    assert!(failed >= 3, "{puts} PUTs: too few to fail more than twice");
    assert_eq!(waits.len() as u64, failed, "one wait a failure: {waits:?}");
    assert!(waits.iter().all(|w| *w == SECOND), "{waits:?}");
    // Bounded: the waits of the failures, and the turns' own millisecond.
    let took = began.elapsed();
    assert!(
        took <= Duration::from_secs(failed + 1),
        "{took:?} for {failed} failed PUTs"
    );
}

/// A remote whose commits are refused with a conflict while `refuse` is
/// set, landing nothing: what a fencing store with no lease row did in
/// the review's S9.
struct Refusing<R> {
    inner: R,
    refuse: Arc<AtomicBool>,
    /// Whether the read of the row and head is refused too, with a
    /// conflict, once a commit has been sent and while `refuse` is set:
    /// what no remote does.
    observe_refused: bool,
    commits: Arc<AtomicU64>,
    /// When each commit was sent.
    sent: Arc<Mutex<Vec<tokio::time::Instant>>>,
}

impl<R: Remote> Remote for Refusing<R> {
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        if self.observe_refused
            && self.commits.load(Ordering::SeqCst) > 0
            && self.refuse.load(Ordering::SeqCst)
        {
            return Err(RemoteError::Conflict);
        }
        self.inner.observe().await
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        self.inner.write(offset, data)
    }

    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }

    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        self.inner.put_sealed().await
    }

    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> Result<CommitHash, RemoteError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        self.sent
            .lock()
            .expect("sent")
            .push(tokio::time::Instant::now());
        if self.refuse.load(Ordering::SeqCst) {
            return Err(RemoteError::Conflict);
        }
        self.inner.commit(cut, expected).await
    }

    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        self.inner.snapshot(name).await
    }

    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError> {
        self.inner.publish(commit, snapshot).await
    }

    fn discard(&mut self) {
        self.inner.discard()
    }

    fn reset(&mut self) {
        self.inner.reset()
    }
}

#[tokio::test(start_paused = true)]
async fn a_commit_refused_again_and_again_waits_longer_each_time_and_not_at_every_cycle() {
    let mut fx = Fixture::new(44).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let refuse = Arc::new(AtomicBool::new(true));
    let commits = Arc::new(AtomicU64::new(0));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let make = {
        let (mut fenced, refuse, commits, sent) = (
            fenced(&fx, &clock),
            refuse.clone(),
            commits.clone(),
            sent.clone(),
        );
        move |epoch| Refusing {
            inner: fenced(epoch),
            refuse: refuse.clone(),
            observe_refused: false,
            commits: commits.clone(),
            sent: sent.clone(),
        }
    };
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), make, 16);

    // Ten minutes of tokio's time, turning as the uploader asks.
    let mut waits = Vec::new();
    let began = tokio::time::Instant::now();
    while began.elapsed() < Duration::from_secs(600) {
        let wait = up.turn().await;
        if let State::Backoff { why, wait: w } = up.shown().state {
            let kind = if why.starts_with("commit of cut 1 refused: conflict") {
                "given up"
            } else if why.starts_with("the commit was refused") {
                "refused"
            } else {
                panic!("{why}")
            };
            if waits.last() != Some(&(kind, w)) {
                // The turn that announces a wait also asks for it: it
                // returns a wait, not a turn at once (G8).
                assert!(!wait.is_zero(), "{kind} {w:?} announced, none asked");
                waits.push((kind, w));
            }
        }
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
    let secs: Vec<u64> = waits.iter().map(|(_, w)| w.as_secs()).collect();
    // A commit refused is read again at once and given up: one wait a
    // cycle, at the give-up, doubling (the lead's decision on F3 of
    // #117's review). The refusal itself waits nothing.
    assert_eq!(&secs[..9], [1, 2, 4, 8, 16, 32, 64, 128, 256], "{waits:?}");
    assert!(
        waits.iter().all(|(kind, _)| *kind == "given up"),
        "{waits:?}"
    );
    let tries = commits.load(Ordering::SeqCst);
    assert!(
        tries <= 11,
        "{tries} commits in ten minutes: the whole cut went up at every cycle"
    );
    // Each wait is waited, not only announced (G8 of #117's review): from
    // one commit to the next, the whole wait of the give-up between them.
    let sent = sent.lock().expect("sent").clone();
    for (k, pair) in sent.windows(2).enumerate() {
        let given_up = waits[k].1;
        assert!(
            pair[1] - pair[0] >= given_up,
            "commit {k}: {:?} from the last, for a wait of {given_up:?}",
            pair[1] - pair[0]
        );
    }
    assert_eq!(fx.uploaded().await.group, None, "nothing landed");

    // The store accepts commits again: the next try lands, and the wait is
    // reset.
    refuse.store(false, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(300)).await;
    until_idle(&mut up, 200).await;
    fx.assert_uploaded(7, "once commits are accepted").await;
    refuse.store(true, Ordering::SeqCst);
    let mut store_wait = None;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 9);
    }
    for _ in 0..100 {
        up.turn().await;
        if let State::Backoff { wait, .. } = up.shown().state {
            store_wait = Some(wait);
            break;
        }
    }
    assert_eq!(
        store_wait,
        Some(Duration::from_secs(1)),
        "the wait starts again"
    );
}

#[tokio::test(start_paused = true)]
async fn a_store_out_of_reach_for_longer_than_the_lease_does_not_stop_the_uploads() {
    let mut fx = Fixture::new(45).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    up.turn().await;
    assert!(matches!(
        up.shown().lease,
        LeaseState::Held { epoch: 1, .. }
    ));
    // Every PUT and read of object storage fails: the steps fail, and the
    // renewals.
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut unsure = false;
    for _ in 0..40 {
        let wait = up.turn().await;
        unsure |= matches!(up.shown().lease, LeaseState::Unsure { epoch: 1, .. });
        clock.pass(wait.as_millis() as u64);
        tokio::time::advance(wait).await;
    }
    assert!(unsure, "a renewal failed: {:?}", up.shown().lease);
    assert!(
        matches!(up.shown().state, State::Backoff { .. }),
        "{:?}",
        up.shown().state
    );
    // Longer than the lease: the row has expired.
    clock.pass(TTL * 2);
    fx.store.disarm().await;
    until_idle(&mut up, 200).await;
    fx.assert_uploaded(7, "after the outage").await;
    assert!(
        matches!(up.shown().lease, LeaseState::Held { epoch: 1, .. }),
        "the same epoch: {:?}",
        up.shown().lease
    );
}

#[tokio::test(start_paused = true)]
async fn an_idle_uploader_asks_again_after_a_quarter_second() {
    let fx = Fixture::new(46).await;
    let store = fx.start();
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    assert_eq!(up.turn().await, IDLE);
    assert_eq!(up.shown().state, State::Idle);
    assert_eq!(up.shown().figures.uploaded, 0);
}

/// The lease is kept every third of its time, idle or not: each keep
/// moves its expiry to the refs clock's time plus the lease's time.
#[tokio::test(start_paused = true)]
async fn the_lease_is_kept_every_third_of_its_time() {
    let fx = Fixture::new(47).await;
    let store = fx.start();
    let clock = Clock::new();
    let start = clock.0.load(Ordering::SeqCst);
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    let held_until = |lease: LeaseState| match lease {
        LeaseState::Held { expires_at_ms, .. } => expires_at_ms,
        other => panic!("not held: {other:?}"),
    };
    let mut until = 0;
    let mut kept_at = Vec::new();
    let mut elapsed = 0;
    while elapsed <= 3 * TTL {
        let wait = up.turn().await;
        let now = held_until(up.shown().lease);
        if now != until {
            assert_eq!(now, start + elapsed + TTL, "kept at {elapsed} ms");
            kept_at.push(elapsed);
            until = now;
        }
        let ms = wait.as_millis() as u64;
        assert!(ms > 0, "an idle uploader waits");
        clock.pass(ms);
        elapsed += ms;
        tokio::time::advance(wait).await;
    }
    assert_eq!(
        kept_at,
        [
            0,
            TTL / 3,
            2 * TTL / 3,
            TTL,
            4 * TTL / 3,
            5 * TTL / 3,
            2 * TTL,
            7 * TTL / 3,
            8 * TTL / 3,
            3 * TTL
        ]
    );
}

/// A store whose answer to `uploadable` takes `delay` milliseconds of
/// tokio's time: a step that outlasts the lease's renewal.
struct Slow<L> {
    inner: L,
    delay: Arc<AtomicU64>,
}

impl<L: skyzfs_upload::Local> skyzfs_upload::Local for Slow<L> {
    async fn uploadable(&mut self) -> Option<skyzfs_upload::Uploadable> {
        let ms = self.delay.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.inner.uploadable().await
    }

    async fn read(
        &mut self,
        number: u64,
        from: u64,
        buf: skyzfs_blk::Buf,
    ) -> (
        skyzfs_blk::Buf,
        std::io::Result<skyzfs_local::inplace::CutSlice>,
    ) {
        self.inner.read(number, from, buf).await
    }

    async fn sync_copies(&mut self, number: u64) -> std::io::Result<()> {
        self.inner.sync_copies(number).await
    }

    async fn release(&mut self, number: u64) -> skyzfs_upload::Released {
        self.inner.release(number).await
    }

    async fn name_committed(&mut self, number: u64, group: u64) -> bool {
        self.inner.name_committed(number, group).await
    }

    async fn abandon_all(&mut self) -> Vec<u64> {
        self.inner.abandon_all().await
    }
}

/// The lease's expiry, as the status shows it.
fn expires<L, R, F, M>(up: &Uploader<L, R, F, M>) -> u64
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    match up.shown().lease {
        LeaseState::Held { expires_at_ms, .. } => expires_at_ms,
        other => panic!("the lease is not held: {other:?}"),
    }
}

/// The lease's way to the refs object, apart from the uploads': its own
/// store over the same objects, which can fail on its own, and a refs
/// clock that is tokio's time. Each take can be held for a while of
/// tokio's time first, the renewal can be told that the row is
/// gone, and the time at which each take reached the refs object is
/// recorded.
#[derive(Clone)]
struct Leasing {
    refs: LeaseRefs,
    /// The lease's own store: arm it to fail the lease's way alone.
    faults: Store,
    started: tokio::time::Instant,
    take_ms: Arc<AtomicU64>,
    gone: Arc<AtomicBool>,
    takes: Arc<Mutex<Vec<Duration>>>,
}

impl Leasing {
    fn apart(fx: &Fixture) -> Self {
        let faults = skynbd_store::FaultStore::new(fx.store.inner().clone());
        let started = tokio::time::Instant::now();
        let refs = Arc::new(
            ObjectRefStore::new(faults.clone()).with_clock(Arc::new(move || {
                1_000_000 + u64::try_from(started.elapsed().as_millis()).expect("ms")
            })),
        );
        Self {
            refs,
            faults,
            started,
            take_ms: Arc::new(AtomicU64::new(0)),
            gone: Arc::new(AtomicBool::new(false)),
            takes: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn keeper(&self, fx: &Fixture) -> Keeper<Leasing> {
        Keeper::new(self.clone(), fx.vol, HOLDER, TTL)
    }

    /// Tokio's time since the way was made.
    fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// When each take reached the refs object.
    fn takes(&self) -> Vec<Duration> {
        self.takes.lock().expect("takes").clone()
    }

    /// The row's expiry for a take that reached the refs object at `at`.
    fn expiry(at: Duration) -> u64 {
        1_000_000 + u64::try_from(at.as_millis()).expect("ms") + TTL
    }
}

#[async_trait::async_trait]
impl RefStore for Leasing {
    async fn get(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Option<RefValue>> {
        self.refs.get(vol, name).await
    }
    async fn compare_and_swap(
        &self,
        vol: VolumeId,
        name: &str,
        expected: Option<RefValue>,
        new_commit: Option<skynbd_types::CommitHash>,
    ) -> skynbd_refs::Result<skynbd_refs::CasOutcome> {
        self.refs
            .compare_and_swap(vol, name, expected, new_commit)
            .await
    }
    async fn list(&self, vol: VolumeId) -> skynbd_refs::Result<Vec<(String, RefValue)>> {
        self.refs.list(vol).await
    }
    fn watch(&self) -> tokio::sync::broadcast::Receiver<skynbd_refs::RefEvent> {
        self.refs.watch()
    }
    async fn transact(
        &self,
        vol: VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.refs.transact(vol, ops).await
    }
    async fn transact_as(
        &self,
        vol: VolumeId,
        ops: Vec<skynbd_refs::RefOp>,
        holder: &str,
        epoch: u64,
    ) -> skynbd_refs::Result<skynbd_refs::TxnOutcome> {
        self.refs.transact_as(vol, ops, holder, epoch).await
    }
    async fn pins(&self, vol: VolumeId, name: &str) -> skynbd_refs::Result<Vec<String>> {
        self.refs.pins(vol, name).await
    }
    async fn meta_get(&self, vol: VolumeId, key: &str) -> skynbd_refs::Result<Option<Vec<u8>>> {
        self.refs.meta_get(vol, key).await
    }
    async fn meta_list(
        &self,
        vol: VolumeId,
        prefix: &str,
    ) -> skynbd_refs::Result<Vec<(String, Vec<u8>)>> {
        self.refs.meta_list(vol, prefix).await
    }
    async fn lease_acquire(
        &self,
        vol: VolumeId,
        holder: &str,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<LeaseOutcome> {
        let ms = self.take_ms.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        self.takes.lock().expect("takes").push(self.elapsed());
        self.refs.lease_acquire(vol, holder, ttl_ms).await
    }
    /// A renewal is held as a take is, and recorded as one where it wrote
    /// the row. A row told gone is answered as a store with no row
    /// answers, and nothing is written.
    async fn lease_renew(
        &self,
        vol: VolumeId,
        holder: &str,
        epoch: u64,
        ttl_ms: u64,
    ) -> skynbd_refs::Result<RenewOutcome> {
        let ms = self.take_ms.load(Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(ms)).await;
        if self.gone.load(Ordering::SeqCst) {
            return Ok(RenewOutcome::NotHeld(None));
        }
        let at = self.elapsed();
        let outcome = self.refs.lease_renew(vol, holder, epoch, ttl_ms).await?;
        if matches!(outcome, RenewOutcome::Renewed(_)) {
            self.takes.lock().expect("takes").push(at);
        }
        Ok(outcome)
    }
    async fn lease_release(&self, vol: VolumeId, holder: &str) -> skynbd_refs::Result<()> {
        self.refs.lease_release(vol, holder).await
    }
    async fn lease_get(&self, vol: VolumeId) -> skynbd_refs::Result<Option<skynbd_refs::Lease>> {
        let row = self.refs.lease_get(vol).await?;
        Ok(row.filter(|_| !self.gone.load(Ordering::SeqCst)))
    }
}

const SECOND: Duration = Duration::from_secs(1);

/// The renewals came every third of the lease from the first take, none
/// late: the takes reached the refs object at 0, 10, 20 s and so on.
fn on_time(takes: &[Duration], case: &str) {
    let third = Duration::from_millis(TTL / 3);
    for (i, at) in takes.iter().enumerate() {
        let due = third * u32::try_from(i).expect("few takes");
        assert_eq!(*at, due, "{case}: take {i} of {takes:?}");
    }
}

/// How many takes fall due before `ended`: the first at 0, then one
/// every third of the lease.
fn due_before(ended: Duration) -> usize {
    let ms = u64::try_from(ended.as_millis()).expect("ms");
    usize::try_from(ms.div_ceil(TTL / 3)).expect("few")
}

/// A step that takes longer than two thirds of the lease, begun one
/// second after the take, does not delay the renewal: the renewal is
/// made beside it, every third of the lease (F4 of #117's review, its
/// scenario C1). The step's backoff is still counted from its failure.
/// At 1415db1 a step of 15 s put the renewal 16 s after the take.
#[tokio::test(start_paused = true)]
async fn a_step_longer_than_two_thirds_of_the_lease_does_not_delay_its_renewal() {
    for (seed, secs) in [(48, 5), (50, 15), (52, 19), (53, 21), (54, 25)] {
        let case = format!("a step of {secs} s");
        let mut fx = Fixture::new(seed).await;
        let delay = Arc::new(AtomicU64::new(0));
        let slow = Slow {
            inner: one_cut(&mut fx),
            delay: Arc::clone(&delay),
        };
        let clock = Clock::new();
        let way = Leasing::apart(&fx);
        // The uploads' way to object storage fails; the lease's does not.
        fx.store.arm(FaultOp::Any, "", 0).await;
        let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);

        let wait = up.turn().await;
        assert_eq!(wait, SECOND, "{case}: the first failure's backoff");
        tokio::time::advance(wait).await;

        // The store's answer to the check takes `secs`, then the check
        // fails on object storage.
        delay.store(secs * 1000, Ordering::SeqCst);
        let wait = up.turn().await;
        let ended = Duration::from_secs(1 + secs);
        assert_eq!(way.elapsed(), ended, "{case}: the turn ends with its step");
        assert!(
            matches!(&up.shown().state, State::Backoff { why, wait }
                if why.contains("injected") && *wait == 2 * SECOND),
            "{case}: {:?}",
            up.shown().state
        );
        on_time(&way.takes(), &case);
        // The take, and each renewal that fell due before the step's end
        // (one due at its very end is the next turn's, at once).
        assert_eq!(
            way.takes().len(),
            due_before(ended),
            "{case}: every renewal due during the step was made beside it: {:?}",
            way.takes()
        );
        let due = Duration::from_secs(ended.as_secs().div_ceil(10) * 10);
        assert_eq!(
            wait,
            (2 * SECOND).min(due - ended),
            "{case}: the backoff from the failure, or the renewal if sooner"
        );

        // Nothing steps before the backoff is over, counted from the
        // failure; then the cut goes up, and the renewals stay on time.
        delay.store(0, Ordering::SeqCst);
        fx.store.disarm().await;
        tokio::time::advance(wait).await;
        loop {
            let wait = up.turn().await;
            if !matches!(up.shown().state, State::Backoff { .. }) {
                break;
            }
            assert!(way.elapsed() < ended + 2 * SECOND, "{case}");
            tokio::time::advance(wait).await;
        }
        assert_eq!(way.elapsed(), ended + 2 * SECOND, "{case}: the next step");
        until_idle(&mut up, 100).await;
        fx.assert_uploaded(7, &case).await;
        while way.elapsed() < Duration::from_secs(60) {
            let wait = up.turn().await;
            tokio::time::advance(wait).await;
        }
        on_time(&way.takes(), &case);
        assert!(way.takes().len() >= 6, "{case}: {:?}", way.takes());
    }
}

/// An idle uploader's check of the store that takes 5 to 25 s, begun a
/// quarter second before the renewal is due, does not delay the renewal
/// (the review's scenario C2, where at 1415db1 a check of 21 s let the
/// row pass its expiry).
#[tokio::test(start_paused = true)]
async fn an_idle_check_longer_than_two_thirds_of_the_lease_does_not_delay_its_renewal() {
    for (seed, secs) in [(49, 5), (55, 15), (56, 19), (57, 21), (58, 25)] {
        let case = format!("a check of {secs} s");
        let fx = Fixture::new(seed).await;
        let delay = Arc::new(AtomicU64::new(0));
        let slow = Slow {
            inner: fx.start(),
            delay: Arc::clone(&delay),
        };
        let clock = Clock::new();
        let way = Leasing::apart(&fx);
        let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
        let begin = Duration::from_millis(TTL / 3) - IDLE;
        while way.elapsed() < begin {
            let wait = up.turn().await;
            assert_eq!((up.shown().state, wait), (State::Idle, IDLE), "{case}");
            tokio::time::advance(wait).await;
        }
        delay.store(secs * 1000, Ordering::SeqCst);
        let wait = up.turn().await;
        let ended = begin + Duration::from_secs(secs);
        assert_eq!(way.elapsed(), ended, "{case}");
        assert_eq!((up.shown().state, wait), (State::Idle, IDLE), "{case}");
        assert_eq!(
            way.takes().len(),
            due_before(ended),
            "{case}: {:?}",
            way.takes()
        );
        assert_eq!(
            expires(&up),
            Leasing::expiry(*way.takes().last().expect("a take")),
            "{case}: the last renewal holds"
        );
        delay.store(0, Ordering::SeqCst);
        while way.elapsed() < Duration::from_secs(70) {
            let wait = up.turn().await;
            tokio::time::advance(wait).await;
        }
        on_time(&way.takes(), &case);
    }
}

/// A step that ends while a renewal beside it is under way waits for the
/// renewal: neither is dropped half done. The step's backoff is counted
/// from the step's end, not the renewal's.
#[tokio::test(start_paused = true)]
async fn a_step_that_ends_during_a_renewal_waits_for_it_and_counts_its_wait_from_its_own_end() {
    let mut fx = Fixture::new(59).await;
    let delay = Arc::new(AtomicU64::new(0));
    let slow = Slow {
        inner: one_cut(&mut fx),
        delay: Arc::clone(&delay),
    };
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
    let wait = up.turn().await;
    assert_eq!(wait, SECOND);
    tokio::time::advance(wait).await;

    // A step from 1 s to 12 s; the renewal due at 10 s takes 3 s.
    delay.store(11_000, Ordering::SeqCst);
    way.take_ms.store(3_000, Ordering::SeqCst);
    let wait = up.turn().await;
    assert_eq!(way.elapsed(), Duration::from_secs(13), "the renewal's end");
    assert_eq!(
        way.takes(),
        [Duration::ZERO, Duration::from_secs(13)],
        "the renewal reached the refs object"
    );
    assert_eq!(expires(&up), Leasing::expiry(Duration::from_secs(13)));
    assert!(
        matches!(up.shown().state, State::Backoff { wait, .. } if wait == 2 * SECOND),
        "{:?}",
        up.shown().state
    );
    // The step failed at 12 s: its backoff of 2 s is over at 14 s.
    assert_eq!(wait, SECOND, "the backoff from the step's end");
}

/// The same for a step that finds nothing to do: the idle wait is counted
/// from the step's end, not from the end of the renewal it waited for.
#[tokio::test(start_paused = true)]
async fn an_idle_step_that_ends_during_a_renewal_counts_its_wait_from_its_own_end() {
    let fx = Fixture::new(61).await;
    let delay = Arc::new(AtomicU64::new(0));
    let slow = Slow {
        inner: fx.start(),
        delay: Arc::clone(&delay),
    };
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
    let wait = up.turn().await;
    assert_eq!(wait, IDLE);
    tokio::time::advance(wait).await;

    // An idle step from 0.25 s to 12 s; the renewal due at 10 s takes 3 s.
    delay.store(11_750, Ordering::SeqCst);
    way.take_ms.store(3_000, Ordering::SeqCst);
    let wait = up.turn().await;
    assert_eq!(way.elapsed(), Duration::from_secs(13), "the renewal's end");
    assert_eq!(up.shown().state, State::Idle);
    // Idle at 12 s: the next step was due at 12.25 s, already past.
    assert_eq!(wait, Duration::ZERO, "the idle wait from the step's end");
}

/// A turned uploader whose lease's way fails, and whose uploads' way
/// works, goes on uploading under the epoch last held (G1 of #117's
/// review): unsure is not lost.
#[tokio::test(start_paused = true)]
async fn the_uploads_go_on_while_the_lease_is_unsure() {
    let mut fx = Fixture::new(60).await;
    let store = fx.start();
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 16);
    assert_eq!(up.turn().await, IDLE);
    assert!(matches!(
        up.shown().lease,
        LeaseState::Held { epoch: 1, .. }
    ));

    way.faults.arm(FaultOp::Any, "", 0).await;
    let mut turns = 0;
    while !matches!(up.shown().lease, LeaseState::Unsure { .. }) {
        turns += 1;
        assert!(turns <= 100, "never unsure: {:?}", up.shown().lease);
        let wait = up.turn().await;
        tokio::time::advance(wait).await;
    }
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 10, 0x10);
        fx.group(store, 7);
    }
    let mut leases = Vec::new();
    for _ in 0..100 {
        let wait = up.turn().await;
        leases.push(up.shown().lease);
        if up.shown().state == State::Idle {
            break;
        }
        tokio::time::advance(wait).await;
    }
    assert!(
        leases
            .iter()
            .all(|l| matches!(l, LeaseState::Unsure { epoch: 1, .. })),
        "unsure throughout: {leases:?}"
    );
    fx.assert_uploaded(7, "while unsure").await;
    assert_eq!(up.shown().figures.uploaded, 1);
}

/// A renewal that finds no row at all loses the lease, naming nobody, and
/// does not take it again (G5 of #117's review): somebody removed the
/// row, and a take would make it anew at epoch 1 for a volume that
/// somebody else may now hold under a new row.
#[tokio::test(start_paused = true)]
async fn a_renewal_that_finds_the_row_gone_loses_the_lease_and_takes_nothing() {
    let fx = Fixture::new(61).await;
    let store = fx.start();
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 16);
    up.turn().await;
    way.gone.store(true, Ordering::SeqCst);
    for _ in 0..100 {
        let wait = up.turn().await;
        if matches!(up.shown().state, State::LeaseLost { .. }) {
            break;
        }
        tokio::time::advance(wait).await;
    }
    assert_eq!(
        up.shown().state,
        State::LeaseLost {
            holder: String::new(),
            epoch: 0
        }
    );
    assert_eq!(way.elapsed(), Duration::from_millis(TTL / 3));
    assert_eq!(way.takes(), [Duration::ZERO], "no take after the first");
}

/// Count the turns of `run` and keep what it publishes.
#[derive(Clone, Default)]
struct Published(Arc<Mutex<Vec<(Duration, skyzfs_upload::uploader::Shown)>>>);

impl Published {
    fn post(&self, at: Duration, shown: skyzfs_upload::uploader::Shown) {
        let mut all = self.0.lock().expect("published");
        assert!(all.len() < 10_000, "run turns without waiting");
        all.push((at, shown));
    }

    fn all(&self) -> Vec<(Duration, skyzfs_upload::uploader::Shown)> {
        self.0.lock().expect("published").clone()
    }
}

/// `run` publishes what the status shows after each turn, sleeps between
/// turns as each asks, and at the stop gives back the lease's keeper,
/// still held, for the caller to let go (G2 and F2 of #117's review).
#[tokio::test(start_paused = true)]
async fn run_publishes_each_turn_sleeps_between_them_and_gives_back_the_lease() {
    let mut fx = Fixture::new(62).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 16);
    let published = Published::default();
    let post = published.clone();
    let at = way.clone();
    let mut keeper = up
        .run(tokio::time::sleep(Duration::from_secs(10)), |shown| {
            post.post(at.elapsed(), shown)
        })
        .await;
    assert_eq!(
        way.elapsed(),
        Duration::from_secs(10),
        "stopped at the stop"
    );
    let all = published.all();
    let idle: Vec<Duration> = all
        .iter()
        .filter(|(_, shown)| shown.state == State::Idle)
        .map(|(at, _)| *at)
        .collect();
    // The cut goes up at once, then one idle check every quarter second.
    assert_eq!(idle.first(), Some(&Duration::ZERO), "{all:?}");
    assert_eq!(idle.len(), 40, "{idle:?}");
    for pair in idle.windows(2) {
        assert_eq!(pair[1] - pair[0], IDLE, "{idle:?}");
    }
    assert!(all.iter().any(|(_, s)| s.figures.uploaded == 1));
    fx.assert_uploaded(7, "under run").await;

    assert!(matches!(keeper.state(), LeaseState::Held { epoch: 1, .. }));
    keeper.release().await.expect("let go");
    let row = clock
        .refs(&fx)
        .lease_get(fx.vol)
        .await
        .expect("the row")
        .expect("a row");
    assert_eq!(
        (row.holder.as_str(), row.epoch, row.expires_at_ms),
        (HOLDER, 1, 0)
    );
}

/// `run` stops during a turn: a step that would take an hour is dropped
/// at the stop, with the uploader, and the keeper comes back held.
#[tokio::test(start_paused = true)]
async fn run_stops_during_a_turn() {
    let mut fx = Fixture::new(63).await;
    let delay = Arc::new(AtomicU64::new(3_600_000));
    let slow = Slow {
        inner: one_cut(&mut fx),
        delay,
    };
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
    let published = Published::default();
    let post = published.clone();
    let keeper = up
        .run(tokio::time::sleep(Duration::from_secs(5)), |shown| {
            post.post(Duration::ZERO, shown)
        })
        .await;
    assert_eq!(way.elapsed(), Duration::from_secs(5));
    assert!(published.all().is_empty(), "no turn ended");
    assert!(matches!(keeper.state(), LeaseState::Held { epoch: 1, .. }));
    assert_eq!(fx.uploaded().await.group, None);
}

/// A cut found committed by the check ends the wait (G7 of #117's
/// review): after two failures (waits of 1 and 2 s) the check finds cut
/// 2 committed by an instance that died before its reply, and the next
/// failure waits 1 s again.
#[tokio::test(start_paused = true)]
async fn a_cut_found_committed_ends_the_wait() {
    let mut fx = Fixture::new(64).await;
    let driver = committed_unheard(&mut fx).await;
    fx.died(driver);
    let store = fx.start();
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 64);
    for want in [1, 2] {
        let wait = up.turn().await;
        assert!(
            matches!(up.shown().state, State::Backoff { wait, .. } if wait == want * SECOND),
            "{:?}",
            up.shown().state
        );
        tokio::time::advance(wait).await;
    }
    fx.store.disarm().await;
    up.turn().await;
    assert_eq!(up.shown().figures.found_committed, 1);
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 9);
    }
    fx.store.arm(FaultOp::Any, "", 0).await;
    let wait = up.turn().await;
    assert!(
        matches!(up.shown().state, State::Backoff { wait, .. } if wait == SECOND),
        "the wait starts again: {:?}",
        up.shown().state
    );
    assert_eq!(wait, SECOND);
}

/// A cut committed and gone from the store before its release
/// ([`skyzfs_upload::Step::UploadedNotHeld`]) ends the wait too.
#[tokio::test(start_paused = true)]
async fn a_cut_committed_and_gone_before_its_release_ends_the_wait() {
    let mut fx = Fixture::new(65).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 64);
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    {
        let store = up.driver().expect("the driver").local();
        for block in 0..OVER {
            fx.write(store, 100 + block, 0x22);
        }
        fx.group(store, 8);
    }
    for _ in 0..100 {
        let wait = up.turn().await;
        if up.shown().state == State::Working(Doing::Committing, Some(2)) {
            break;
        }
        tokio::time::advance(wait).await;
    }
    assert_eq!(up.shown().state, State::Working(Doing::Committing, Some(2)));
    // The head swap lands and its reply is lost: a wait of 1 s.
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, skynbd_store::FaultWhen::After)
        .await;
    let wait = up.turn().await;
    assert!(
        matches!(up.shown().state, State::Backoff { wait, .. } if wait == SECOND),
        "{:?}",
        up.shown().state
    );
    fx.store.disarm().await;
    fx.assert_uploaded(8, "cut 2's swap landed").await;
    // The bound takes cut 2, and cut 3 takes its map.
    {
        let store = up.driver().expect("the driver").local();
        for block in 0..OVER {
            fx.write(store, 100 + block, 0x33);
        }
        fx.group(store, 9);
    }
    tokio::time::advance(wait).await;
    up.turn().await;
    assert_eq!(
        up.shown().figures.uploaded,
        2,
        "cut 2 counted uploaded, not held"
    );
    fx.store.arm(FaultOp::Any, "", 0).await;
    let wait = up.turn().await;
    assert!(
        matches!(up.shown().state, State::Backoff { wait, .. } if wait == SECOND),
        "the wait starts again: {:?}",
        up.shown().state
    );
    assert_eq!(wait, SECOND);
}

/// A first take that fails after 5 s is tried again 1 s after it failed,
/// not 1 s after it was sent (G9 of #117's review, the review's C4).
#[tokio::test(start_paused = true)]
async fn a_slow_first_take_that_fails_is_tried_again_a_second_after_it_failed() {
    let mut fx = Fixture::new(66).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    way.faults.arm(FaultOp::Any, "", 0).await;
    way.take_ms.store(5_000, Ordering::SeqCst);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 16);
    let wait = up.turn().await;
    assert_eq!(way.elapsed(), Duration::from_secs(5));
    assert!(matches!(up.shown().state, State::NoLease(_)));
    assert_eq!(wait, SECOND, "a second after the failure");
    way.faults.disarm().await;
    way.take_ms.store(0, Ordering::SeqCst);
    tokio::time::advance(wait).await;
    up.turn().await;
    assert_eq!(
        way.takes(),
        [Duration::from_secs(5), Duration::from_secs(6)]
    );
    assert!(matches!(
        up.shown().lease,
        LeaseState::Held { epoch: 1, .. }
    ));
}

/// A renewal that takes 4 s in the middle of a backoff is followed by
/// a wait of 6 s, what is left of the third of the lease, and not 10 s.
#[tokio::test(start_paused = true)]
async fn a_slow_renewal_during_a_backoff_is_followed_by_what_is_left_of_the_wait() {
    let mut fx = Fixture::new(67).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 16);
    // Failures at 0, 1, 3, 7 and 15 s: the last waits 16 s, to 31 s.
    loop {
        let wait = up.turn().await;
        if way.elapsed() + wait == Duration::from_secs(20) {
            tokio::time::advance(wait).await;
            break;
        }
        assert!(way.elapsed() < Duration::from_secs(20));
        tokio::time::advance(wait).await;
    }
    assert!(
        matches!(up.shown().state, State::Backoff { wait, .. } if wait == 16 * SECOND),
        "{:?}",
        up.shown().state
    );
    way.take_ms.store(4_000, Ordering::SeqCst);
    let wait = up.turn().await;
    assert_eq!(way.elapsed(), Duration::from_secs(24));
    assert_eq!(expires(&up), Leasing::expiry(Duration::from_secs(24)));
    assert_eq!(wait, Duration::from_secs(6), "to the next renewal, at 30 s");
}

/// A cut the store kept at its release (the intent map could not be
/// written) stops the uploader for good: it rests, and its driver is
/// not stepped again (P6 of #117's review).
#[tokio::test(start_paused = true)]
async fn a_cut_kept_at_its_release_stops_the_uploader_which_rests() {
    let mut fx = Fixture::new(68).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 16);
    fx.disk.fail_write_of("intent.map");
    for _ in 0..100 {
        let wait = up.turn().await;
        if up.shown().state == State::Stopped {
            assert_eq!(wait, AT_REST);
            break;
        }
        tokio::time::advance(wait).await;
    }
    assert_eq!(up.shown().state, State::Stopped);
    fx.assert_uploaded(7, "committed, then kept").await;
    for _ in 0..3 {
        tokio::time::advance(AT_REST).await;
        assert_eq!(up.turn().await, AT_REST);
        assert_eq!(up.shown().state, State::Stopped, "not stepped again");
    }
    assert_eq!(up.shown().figures.kept, 1);
}

/// Commits pass.
const PASS: u8 = 0;
/// Commits are refused with a conflict, and nothing lands.
const REFUSE: u8 = 1;
/// A commit lands, its answer is a conflict, and the read of the row and
/// head after it returns what was read before it: the commit is given up,
/// and the next check finds the cut committed.
const LAND_UNSEEN: u8 = 2;

/// A remote whose commits pass, are refused, or land unseen, as `mode`
/// says.
struct Refusals<R> {
    inner: R,
    mode: Arc<AtomicU8>,
    last: Option<Observed>,
    stale: Option<Observed>,
}

impl<R> Refusals<R> {
    fn make(
        mut make: impl FnMut(u64) -> R,
        mode: &Arc<AtomicU8>,
    ) -> impl FnMut(u64) -> Refusals<R> {
        let mode = Arc::clone(mode);
        move |epoch| Refusals {
            inner: make(epoch),
            mode: Arc::clone(&mode),
            last: None,
            stale: None,
        }
    }
}

impl<R: Remote> Remote for Refusals<R> {
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        if let Some(stale) = self.stale.take() {
            return Ok(stale);
        }
        let seen = self.inner.observe().await?;
        self.last = Some(seen.clone());
        Ok(seen)
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        self.inner.write(offset, data)
    }

    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }

    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        self.inner.put_sealed().await
    }

    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> Result<CommitHash, RemoteError> {
        match self.mode.load(Ordering::SeqCst) {
            REFUSE => Err(RemoteError::Conflict),
            LAND_UNSEEN => {
                self.inner.commit(cut, expected).await?;
                self.stale = self.last.clone();
                Err(RemoteError::Conflict)
            }
            _ => self.inner.commit(cut, expected).await,
        }
    }

    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        self.inner.snapshot(name).await
    }

    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError> {
        self.inner.publish(commit, snapshot).await
    }

    fn discard(&mut self) {
        self.inner.discard()
    }

    fn reset(&mut self) {
        self.inner.reset()
    }
}

/// Turn, moving tokio's time by each wait, until `n` more commits have
/// been given up: the wait each give-up announced.
async fn give_ups<L, R, F, M>(up: &mut Uploader<L, R, F, M>, n: usize) -> Vec<Duration>
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let (mut waits, mut last) = (Vec::new(), None);
    for _ in 0..1000 {
        let wait = up.turn().await;
        let state = up.shown().state;
        if let State::Backoff { why, wait: w } = &state {
            assert!(why.contains("refused: conflict"), "{why}");
            if last.as_ref() != Some(&state) {
                waits.push(*w);
            }
        }
        last = Some(state);
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
        if waits.len() == n {
            return waits;
        }
    }
    panic!("{n} give-ups not seen: {waits:?}");
}

/// A cut found committed at its check ends the wait of the refusals
/// before it, as a cut committed does: the next refused cycle waits a
/// second again (P2 of #117's review, which the two waits of F3 had left
/// to no test: the tests of a found cut reset the errors' wait).
#[tokio::test(start_paused = true)]
async fn a_cut_found_committed_ends_the_wait_of_the_refusals() {
    let mut fx = Fixture::new(69).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let mode = Arc::new(AtomicU8::new(REFUSE));
    let make = Refusals::make(fenced(&fx, &clock), &mode);
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), make, 16);
    assert_eq!(give_ups(&mut up, 2).await, [SECOND, 2 * SECOND]);
    mode.store(LAND_UNSEEN, Ordering::SeqCst);
    assert_eq!(give_ups(&mut up, 1).await, [4 * SECOND]);
    fx.assert_uploaded(7, "the commit given up had landed")
        .await;
    mode.store(REFUSE, Ordering::SeqCst);
    until_idle(&mut up, 100).await;
    assert_eq!(up.shown().figures.found_committed, 1);
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 9);
    }
    assert_eq!(
        give_ups(&mut up, 1).await,
        [SECOND],
        "the wait starts again"
    );
}

/// A cut committed and gone from the store before its release ends the
/// wait of the refusals before it (P3 of #117's review, as P2 above).
#[tokio::test(start_paused = true)]
async fn a_cut_committed_and_gone_ends_the_wait_of_the_refusals() {
    let mut fx = Fixture::new(70).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mode = Arc::new(AtomicU8::new(PASS));
    let make = Refusals::make(fenced(&fx, &clock), &mode);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), make, 64);
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    {
        let store = up.driver().expect("the driver").local();
        for block in 0..OVER {
            fx.write(store, 100 + block, 0x22);
        }
        fx.group(store, 8);
    }
    mode.store(REFUSE, Ordering::SeqCst);
    assert_eq!(give_ups(&mut up, 2).await, [SECOND, 2 * SECOND]);
    mode.store(PASS, Ordering::SeqCst);
    for _ in 0..100 {
        if up.shown().state == State::Working(Doing::Committing, Some(2)) {
            break;
        }
        let wait = up.turn().await;
        tokio::time::advance(wait).await;
    }
    assert_eq!(up.shown().state, State::Working(Doing::Committing, Some(2)));
    // The head swap lands and its reply is lost.
    fx.store
        .arm_when(FaultOp::Put, "refs", 0, skynbd_store::FaultWhen::After)
        .await;
    let wait = up.turn().await;
    fx.store.disarm().await;
    fx.assert_uploaded(8, "cut 2's swap landed").await;
    // The bound takes cut 2, and cut 3 takes its map.
    {
        let store = up.driver().expect("the driver").local();
        for block in 0..OVER {
            fx.write(store, 100 + block, 0x33);
        }
        fx.group(store, 9);
    }
    tokio::time::advance(wait).await;
    up.turn().await;
    assert_eq!(up.shown().figures.not_held, 1, "cut 2 committed, not held");
    mode.store(REFUSE, Ordering::SeqCst);
    assert_eq!(
        give_ups(&mut up, 1).await,
        [SECOND],
        "the wait starts again"
    );
}

// ------------------------------------------- publishes through the loop

/// The uploader's tests of a publish (M2.5-1), from the review of #122:
/// T2 (none went through `Uploader`), F3 (a publish that fails or is
/// refused must neither stop the cuts nor be sent again without a wait)
/// and F4 (a publish that cannot be made once the uploader stops is ended,
/// not left pending).
const ATTACHMENT: [u8; 16] = *b"the attachment..";

/// The request for `pool@name` in cut state `state`.
fn asked(state: [u8; 16], name: &str, after: u64, group: u64) -> Request {
    Request {
        name: name.into(),
        attachment: ATTACHMENT,
        state,
        after,
        group,
        manifest: format!("the ZFS manifest of pool@{name}").into_bytes(),
        expiry: EXPIRY_DEFAULT,
    }
}

/// `snap/<name>` and its row, as the refs object has them.
async fn snap(fx: &Fixture, name: &str) -> (Option<CommitHash>, Option<Vec<u8>>) {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    let found = refs
        .get(fx.vol, &format!("snap/{name}"))
        .await
        .expect("the ref")
        .map(|value| value.commit);
    let row = refs
        .meta_get(fx.vol, &format!("skyzfs/snap/{name}"))
        .await
        .expect("the row");
    (found, row)
}

async fn head_of(fx: &Fixture) -> CommitHash {
    let refs = ObjectRefStore::new(fx.store.inner().clone());
    refs.get(fx.vol, "head")
        .await
        .expect("head")
        .expect("a head")
        .commit
}

/// T2: a publish asked before the lease is held, and so before the
/// driver is made, is made once the lease is held and the cut committed.
#[tokio::test(start_paused = true)]
async fn a_publish_asked_before_the_lease_is_made_once_it_is_held() {
    let mut fx = Fixture::new(71).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    fx.store.arm(FaultOp::Any, "", 0).await;
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let wait = up.turn().await;
    assert!(matches!(up.shown().state, State::NoLease(_)));
    assert!(up.driver().is_none());
    assert_eq!(
        up.publishing().ask(asked(state, "t", 0, 7)),
        Answer::Accepted
    );
    assert_eq!(up.publishing().progress("t"), Progress::Pending);
    fx.store.disarm().await;
    tokio::time::advance(wait).await;
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    let head = head_of(&fx).await;
    assert_eq!(up.publishing().progress("t"), Progress::Done(head));
    let (found, row) = snap(&fx, "t").await;
    assert_eq!(found, Some(head));
    assert!(row.is_some());
}

/// T2 and F4: the publish's transaction is fenced by the lease's epoch.
/// After another holder has taken the lease, a publish by this uploader
/// lands no ref and no row and loses the lease, as a commit does; the
/// publish then ends refused (`Stopped`), and every request after it is
/// refused at once.
#[tokio::test(start_paused = true)]
async fn a_publish_after_another_took_the_lease_is_fenced_and_ends() {
    let mut fx = Fixture::new(72).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    clock.pass(TTL + 1);
    let other = clock
        .refs(&fx)
        .lease_acquire(fx.vol, "node-b", TTL)
        .await
        .expect("the other's take");
    assert!(matches!(other, LeaseOutcome::Acquired(l) if l.epoch == 2));
    let request = asked(state, "t", 0, 7);
    assert_eq!(up.publishing().ask(request.clone()), Answer::Accepted);
    fx.store.inner().record(true);
    let mut states = Vec::new();
    for _ in 0..20 {
        up.turn().await;
        states.push(up.shown().state);
        if matches!(states.last(), Some(State::LeaseLost { .. })) {
            break;
        }
    }
    fx.store.inner().record(false);
    assert_eq!(
        states.last(),
        Some(&State::LeaseLost {
            holder: "node-b".into(),
            epoch: 2
        }),
        "{states:?}"
    );
    let log = fx.store.inner().log();
    assert_eq!(
        log.iter().filter(|op| op.starts_with("put_if")).count(),
        0,
        "the fence refused the transaction before any write of the refs object: {log:?}"
    );
    assert_eq!(snap(&fx, "t").await, (None, None), "nothing landed");
    assert_eq!(
        up.publishing().progress("t"),
        Progress::Refused(Refusal::Stopped)
    );
    for _ in 0..3 {
        tokio::time::advance(AT_REST).await;
        assert_eq!(up.turn().await, AT_REST);
    }
    assert_eq!(
        up.publishing().ask(request),
        Answer::Refused(Refusal::Stopped)
    );
    assert_eq!(
        up.publishing().ask(asked(state, "u", 0, 7)),
        Answer::Refused(Refusal::Stopped)
    );
}

/// A remote that notes when each publish is sent, and its name. For a
/// name `refuse` holds for, the lookup finds no ref and the publish is
/// refused with a conflict, as a refs store that fences writes answers
/// while the volume has no lease row; otherwise both go to the engine.
/// While `cuts_fail` is set, every PUT of sealed segments fails, so that
/// a cut waits in its backoff; each PUT of them takes `cut_slow` first.
struct Publishes<R> {
    inner: R,
    refuse: fn(&str) -> bool,
    /// How long a refused publish takes to fail.
    slow: Duration,
    /// How long a publish that is made takes: its transaction.
    made_slow: Duration,
    /// How long each PUT of sealed segments takes: a slice's step makes
    /// two.
    cut_slow: Duration,
    cuts_fail: Arc<AtomicBool>,
    sent: Arc<Mutex<Vec<(String, tokio::time::Instant)>>>,
}

impl<R: Remote> Remote for Publishes<R> {
    async fn observe(&mut self) -> Result<Observed, RemoteError> {
        self.inner.observe().await
    }

    fn write(&mut self, offset: u64, data: &[u8]) -> Result<(), RemoteError> {
        self.inner.write(offset, data)
    }

    fn write_zeroes(&mut self, offset: u64, len: u64) -> Result<(), RemoteError> {
        self.inner.write_zeroes(offset, len)
    }

    async fn put_sealed(&mut self) -> Result<(), RemoteError> {
        if !self.cut_slow.is_zero() {
            tokio::time::sleep(self.cut_slow).await;
        }
        if self.cuts_fail.load(Ordering::SeqCst) {
            return Err(RemoteError::Store("the segments' PUT failed".into()));
        }
        self.inner.put_sealed().await
    }

    async fn commit(
        &mut self,
        cut: CommittedCut,
        expected: Option<Vec<u8>>,
    ) -> Result<CommitHash, RemoteError> {
        self.inner.commit(cut, expected).await
    }

    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        if (self.refuse)(name) {
            return Ok(None);
        }
        self.inner.snapshot(name).await
    }

    async fn publish(
        &mut self,
        commit: CommitHash,
        snapshot: &Snapshot<'_>,
    ) -> Result<(), RemoteError> {
        self.sent
            .lock()
            .expect("the times")
            .push((snapshot.name.to_string(), tokio::time::Instant::now()));
        if (self.refuse)(snapshot.name) {
            tokio::time::sleep(self.slow).await;
            return Err(RemoteError::Conflict);
        }
        if !self.made_slow.is_zero() {
            tokio::time::sleep(self.made_slow).await;
        }
        self.inner.publish(commit, snapshot).await
    }

    fn discard(&mut self) {
        self.inner.discard()
    }

    fn reset(&mut self) {
        self.inner.reset()
    }
}

/// Ten minutes of turns, the lease kept on its own clock, after a publish
/// that cannot be made was asked while cut 2 waits to go up: cut 2 goes
/// up, and the publish is sent again at waits that grow. The times it was
/// sent, and how the publish stands at the end. `expiry` is the request's.
async fn ten_minutes_of_a_publish_that_fails(
    seed: u64,
    refuse: bool,
    expiry: Duration,
) -> (Vec<Duration>, Progress) {
    let mut fx = Fixture::new(seed).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let mut make = fenced(&fx, &clock);
    let times = Arc::clone(&sent);
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(
        store,
        keeper(&fx, &clock),
        move |epoch| Publishes {
            inner: make(epoch),
            refuse: if refuse { every_name } else { no_name },
            slow: Duration::ZERO,
            made_slow: Duration::ZERO,
            cut_slow: Duration::ZERO,
            cuts_fail: Arc::new(AtomicBool::new(false)),
            sent: Arc::clone(&times),
        },
        16,
    );
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    if !refuse {
        // Every PUT of a ZFS manifest fails, and nothing else does: a
        // store that refuses that key (a policy on the prefix, say).
        fx.store.arm(FaultOp::Put, "skyzfs/manifest", 0).await;
    }
    let mut request = asked(state, "t", 0, 7);
    request.expiry = expiry;
    assert_eq!(up.publishing().ask(request), Answer::Accepted);
    {
        let local = up.driver().expect("the driver").local();
        fx.write(local, 20, 0x20);
        fx.group(local, 8);
    }
    let began = tokio::time::Instant::now();
    while began.elapsed() < Duration::from_secs(600) {
        let wait = up.turn().await;
        clock.pass(wait.as_millis() as u64);
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
    fx.assert_uploaded(8, "cut 2, while the publish fails")
        .await;
    assert_eq!(snap(&fx, "t").await, (None, None));
    let progress = up.publishing().progress("t");
    let sent = sent_of(&sent, "t", began);
    println!("publishes sent at {sent:?}; {progress:?}");
    (sent, progress)
}

/// Every publish is refused.
fn every_name(_: &str) -> bool {
    true
}

/// No publish is refused.
fn no_name(_: &str) -> bool {
    false
}

/// When the publishes of `name` were sent, from `began`.
fn sent_of(
    sent: &Mutex<Vec<(String, tokio::time::Instant)>>,
    name: &str,
    began: tokio::time::Instant,
) -> Vec<Duration> {
    sent.lock()
        .expect("the times")
        .iter()
        .filter(|(sent, _)| sent == name)
        .map(|(_, at)| at.duration_since(began))
        .collect()
}

/// The waits between the publishes `sent` grow from a second, and no
/// publish follows another at once.
fn waits_grow(sent: &[Duration], case: &str) {
    assert!(
        sent.len() >= 5,
        "{case}: the publish is tried again: {sent:?}"
    );
    assert!(
        sent.len() <= 12,
        "{case}: sent {} times in ten minutes: {sent:?}",
        sent.len()
    );
    let gaps: Vec<Duration> = sent.windows(2).map(|w| w[1] - w[0]).collect();
    assert!(gaps[0] >= Duration::from_secs(1), "{case}: {gaps:?}");
    assert!(
        gaps.windows(2).all(|w| w[1] >= w[0]),
        "{case}: the waits do not grow: {gaps:?}"
    );
}

/// F3: the PUT of a ZFS manifest fails every time. The cuts go on (design
/// §5.2: the cut gives, the recovery point does not wait on a snapshot's
/// name), and the publish is sent again at the publishes' own waits.
#[tokio::test(start_paused = true)]
async fn a_publish_that_cannot_be_made_does_not_stop_the_cuts() {
    let (sent, progress) = ten_minutes_of_a_publish_that_fails(73, false, EXPIRY_MAX).await;
    waits_grow(&sent, "a manifest's PUT fails");
    assert_eq!(progress, Progress::Pending);
}

/// F3, the conflict: a publish refused with a conflict, again and again,
/// is waited after, as a commit refused again and again is (G7 of #114's
/// review), and the cuts go on.
#[tokio::test(start_paused = true)]
async fn a_publish_refused_again_and_again_is_waited_after() {
    let (sent, progress) = ten_minutes_of_a_publish_that_fails(74, true, EXPIRY_MAX).await;
    waits_grow(&sent, "the publish is refused");
    assert_eq!(progress, Progress::Pending);
}

/// F4: a cut is committed and the store keeps it (the intent map's pages
/// cannot be written: the volume has failed). The driver stops and the
/// uploader steps it no more: the publish the cut carries ends refused
/// (`Stopped`), and no request after it is accepted.
#[tokio::test(start_paused = true)]
async fn a_publish_whose_cut_the_store_kept_ends_refused() {
    let mut fx = Fixture::new(75).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let request = asked(state, "t", 0, 7);
    assert_eq!(up.publishing().ask(request.clone()), Answer::Accepted);
    for _ in 0..100 {
        up.turn().await;
        if up.driver().is_some_and(|d| d.doing() == Doing::Committing) {
            break;
        }
    }
    fx.disk.fail_write_of("intent.map");
    for _ in 0..10 {
        let wait = up.turn().await;
        tokio::time::advance(wait).await;
    }
    fx.assert_uploaded(7, "cut 1 is committed").await;
    assert_eq!(up.shown().state, State::Stopped);
    assert_eq!(up.shown().figures.kept, 1);
    assert_eq!(
        up.publishing().progress("t"),
        Progress::Refused(Refusal::Stopped)
    );
    assert_eq!(snap(&fx, "t").await, (None, None));
    assert_eq!(
        up.publishing().ask(request),
        Answer::Refused(Refusal::Stopped)
    );
}

/// Q1 of #122's review, as the lead decided it: a publish that fails
/// past its deadline is looked up once more, found absent, and ends
/// expired, and its transaction is not sent after the deadline. A
/// manifest PUT that always fails, and a publish always refused, with an
/// expiry of a minute; the cuts go on either way.
#[tokio::test(start_paused = true)]
async fn a_publish_that_fails_past_its_deadline_ends_expired_and_is_not_sent_again() {
    for (seed, refuse) in [(76, false), (77, true)] {
        let (sent, progress) =
            ten_minutes_of_a_publish_that_fails(seed, refuse, Duration::from_secs(60)).await;
        assert_eq!(progress, Progress::Expired, "refused: {refuse}");
        assert!(
            sent.iter().all(|at| *at < Duration::from_secs(60)),
            "refused: {refuse}: sent after its deadline: {sent:?}"
        );
        assert!(sent.len() >= 5, "refused: {refuse}: {sent:?}");
    }
}

/// Q1 of #122's review, as the lead decided it: a publish that waits for
/// a cut past its deadline is dropped and counted while the uploader is
/// idle, and the cut that comes after does not carry it. A publish
/// withdrawn is dropped at once, and no cut carries it either.
#[tokio::test(start_paused = true)]
async fn a_publish_past_its_deadline_or_withdrawn_is_dropped_and_no_cut_carries_it() {
    let mut fx = Fixture::new(78).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let state = store.cut_state_id().expect("the cut state");
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    let mut late = asked(state, "late", 1, 8);
    late.expiry = Duration::from_secs(60);
    assert_eq!(up.publishing().ask(late), Answer::Accepted);
    assert_eq!(
        up.publishing().ask(asked(state, "withdrawn", 1, 8)),
        Answer::Accepted
    );
    // Looked up, absent: both wait for a cut after cut 1. Each lookup is
    // a turn of its own, after which the uploader turns again at once,
    // and the cut's check takes the turn between them: a publish and the
    // cut take turns while both are due (F1 of the review of #126), and
    // an idle cut is due again at once, its idle wait being the sleep
    // the uploader asks for. Then neither publish has a step to take,
    // and the idle uploader sleeps its idle wait.
    let mut waits = Vec::new();
    while waits.last().is_none_or(Duration::is_zero) {
        assert!(waits.len() < 10, "the uploader never sleeps: {waits:?}");
        waits.push(up.turn().await);
    }
    assert_eq!(up.shown().state, State::Idle);
    assert_eq!(
        waits,
        [Duration::ZERO, Duration::ZERO, Duration::ZERO, IDLE]
    );
    assert_eq!(up.publishing().progress("late"), Progress::Pending);
    assert_eq!(up.publishing().withdraw("withdrawn"), Progress::Withdrawn);
    let began = tokio::time::Instant::now();
    while began.elapsed() < Duration::from_secs(61) {
        let wait = up.turn().await;
        clock.pass(wait.as_millis() as u64);
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
    assert_eq!(up.publishing().progress("late"), Progress::Expired);
    let figures = up.publishing().figures().clone();
    assert_eq!((figures.expired, figures.withdrawn), (1, 1));
    assert_eq!(up.publishing().pending().0, 0);
    {
        let local = up.driver().expect("the driver").local();
        fx.write(local, 20, 0x20);
        fx.group(local, 8);
    }
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(8, "cut 2").await;
    for name in ["late", "withdrawn"] {
        assert_eq!(snap(&fx, name).await, (None, None), "{name}");
    }
    assert_eq!(up.publishing().figures().published, 0);
}

// ------------------------------------------- the daemon's channel (M2.5-2)

/// Ask `inbox`'s uploader `make(reply)` and wait for the answer.
async fn ask_of<T>(
    inbox: &tokio::sync::mpsc::Sender<Control>,
    make: impl FnOnce(tokio::sync::oneshot::Sender<T>) -> Control,
) -> T {
    let (reply, answer) = tokio::sync::oneshot::channel();
    inbox.send(make(reply)).await.expect("the uploader runs");
    answer.await.expect("an answer")
}

/// `run_with` answers the channel's requests between its turns: a
/// publish accepted, its progress until it is done, on cut 1, whose
/// commit is head. Each post of the status after a publish ended names
/// it once, as it ended. (The count of cuts is the store's, asked at the
/// desk: `tests/queued.rs`.)
#[tokio::test(start_paused = true)]
async fn run_with_answers_the_channels_requests_between_turns() {
    let mut fx = Fixture::new(81).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (inbox, requests) = tokio::sync::mpsc::channel(4);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let published = Published::default();
    let post = published.clone();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        move |shown| post.post(Duration::ZERO, shown),
    );
    let client = async move {
        let answer = ask_of(&inbox, |r| Control::Ask {
            request: asked(state, "t", 0, 7),
            rewound: 0,
            reply: r,
        })
        .await;
        assert_eq!(answer, Answer::Accepted);
        let done = loop {
            match ask_of(&inbox, |r| Control::Progress("t".into(), r)).await {
                Progress::Done(commit) => break commit,
                Progress::Pending => tokio::time::sleep(Duration::from_millis(100)).await,
                other => panic!("{other:?}"),
            }
        };
        let _ = stop.send(());
        done
    };
    let (_keeper, done) = tokio::join!(run, client);
    let head = head_of(&fx).await;
    assert_eq!(done, head);
    assert_eq!(snap(&fx, "t").await.0, Some(head));
    let all = published.all();
    let ended: Vec<(String, Progress)> = all
        .iter()
        .flat_map(|(_, shown)| shown.publishes.ended.clone())
        .collect();
    assert_eq!(ended, [("t".to_string(), Progress::Done(head))]);
    let last = &all.last().expect("posts").1.publishes;
    assert_eq!((last.pending, last.figures.published), (0, 1));
}

/// A publish accepted while the uploader sleeps between turns is taken
/// at once, not after the sleep: done in less than an idle wait.
/// Mutant: the accepted request does not end the sleep.
#[tokio::test(start_paused = true)]
async fn a_publish_accepted_between_turns_is_taken_at_once() {
    let mut fx = Fixture::new(87).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    let (inbox, requests) = tokio::sync::mpsc::channel(4);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        |_| {},
    );
    let client = async move {
        // Into the middle of an idle wait.
        tokio::time::sleep(IDLE / 2).await;
        let asked_at = tokio::time::Instant::now();
        let answer = ask_of(&inbox, |r| Control::Ask {
            request: asked(state, "t", 0, 7),
            rewound: 0,
            reply: r,
        })
        .await;
        assert_eq!(answer, Answer::Accepted);
        // Polled with a short sleep, not a yield: the clock is paused and
        // moves only while every task waits, and a mutant that leaves the
        // publish to the idle wait must reach it (and fail below) rather
        // than poll for ever.
        while asked_at.elapsed() < 2 * IDLE {
            match ask_of(&inbox, |r| Control::Progress("t".into(), r)).await {
                Progress::Done(_) => break,
                Progress::Pending => tokio::time::sleep(Duration::from_millis(1)).await,
                other => panic!("{other:?}"),
            }
        }
        let took = asked_at.elapsed();
        let _ = stop.send(());
        took
    };
    let (_keeper, took) = tokio::join!(run, client);
    assert!(
        took < IDLE / 2,
        "the publish waited for the sleep: {took:?}"
    );
}

/// A request whose asker stopped waiting before the uploader took it
/// (the channel's own limit) is not taken: the asker was told that no
/// answer came. A withdrawal is made all the same.
#[tokio::test(start_paused = true)]
async fn a_request_whose_asker_gave_up_is_not_taken() {
    let mut fx = Fixture::new(88).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (reply, answer) = tokio::sync::oneshot::channel();
    drop(answer);
    assert!(!up.control(Control::Ask {
        request: asked(state, "t", 0, 7),
        rewound: 0,
        reply,
    }));
    assert_eq!(up.publishing().progress("t"), Progress::Unknown);
    assert_eq!(up.publishing().figures().accepted, 0);

    assert_eq!(
        up.publishing().ask(asked(state, "u", 0, 100)),
        Answer::Accepted
    );
    let (reply, answer) = tokio::sync::oneshot::channel();
    drop(answer);
    assert!(!up.control(Control::Withdraw("u".into(), reply)));
    assert_eq!(up.publishing().progress("u"), Progress::Withdrawn);
}

/// A request is answered as the publishes stand when it comes: one whose
/// time is up reads expired at once, between two turns, not at the next
/// turn. Mutant: the publishes are not expired before an answer.
#[tokio::test(start_paused = true)]
async fn a_publish_whose_time_is_up_reads_expired_between_turns() {
    let mut fx = Fixture::new(84).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    let (inbox, requests) = tokio::sync::mpsc::channel(4);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        |_| {},
    );
    let client = async move {
        // No cut of group 100 comes: the publish waits until its time is
        // up, 1.1 s after it was accepted, between two idle turns.
        let mut request = asked(state, "t", 0, 100);
        request.expiry = Duration::from_millis(1100);
        let answer = ask_of(&inbox, |r| Control::Ask {
            request,
            rewound: 0,
            reply: r,
        })
        .await;
        assert_eq!(answer, Answer::Accepted);
        tokio::time::sleep(Duration::from_millis(1099)).await;
        let before = ask_of(&inbox, |r| Control::Progress("t".into(), r)).await;
        tokio::time::sleep(Duration::from_millis(1)).await;
        let at = ask_of(&inbox, |r| Control::Progress("t".into(), r)).await;
        let _ = stop.send(());
        (before, at)
    };
    let (_keeper, (before, at)) = tokio::join!(run, client);
    assert_eq!(before, Progress::Pending);
    assert_eq!(at, Progress::Expired);
}

/// A request is judged against the cut state the uploader was given at
/// the start, which is the store's (a store's state changes only at a
/// start): one of that state is taken before any turn has checked a
/// cut, and one of another state is refused. No store call is made for
/// it (`control` does not wait; F1 and F5 of the review of #127).
///
/// Mutant: the state not given at the start (the first is refused
/// `State`).
#[tokio::test(start_paused = true)]
async fn a_request_is_judged_against_the_cut_state_given_at_the_start() {
    let mut fx = Fixture::new(86).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (reply, answer) = tokio::sync::oneshot::channel();
    assert!(up.control(Control::Ask {
        request: asked(state, "t", 0, 1),
        rewound: 0,
        reply,
    }));
    assert_eq!(answer.await.ok(), Some(Answer::Accepted));
    let mut other = state;
    other[0] ^= 1;
    let (reply, answer) = tokio::sync::oneshot::channel();
    assert!(!up.control(Control::Ask {
        request: asked(other, "u", 0, 1),
        rewound: 0,
        reply,
    }));
    assert_eq!(answer.await.ok(), Some(Answer::Refused(Refusal::State)));
    assert_eq!(up.publishing().progress("t"), Progress::Pending);
}

/// The store's count of cuts and rewind mark now, as the daemon's channel
/// reads them at the desk; it gives the mark with each `publish`
/// (`CutsNow::rewound`).
fn counted<R, F, M>(up: &mut Uploader<InPlace<SimDisk>, R, F, M>) -> (u64, u64)
where
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let store = up.driver().expect("the driver").local();
    let now = <InPlace<SimDisk> as skyzfs_upload::Store>::cuts(store);
    (now.count, now.rewound)
}

/// B3 of the review of #140, from the rewind on: `zpool import -T`
/// rewinds the pool below cut 2's group 51, and the first cut of the new
/// history, of group 40, is counted: cut 3, and the store's mark is 3.
/// `pool@t` is taken on the new history at group 40, and asked with
/// *C* = 1, read before cut 2, and with the store's mark, between two
/// turns of the uploader, so before its check of cut 3: the answer.
fn a_rewind_and_an_ask_with_an_older_count<R, F, M>(
    fx: &mut Fixture,
    up: &mut Uploader<InPlace<SimDisk>, R, F, M>,
    state: [u8; 16],
) -> Answer
where
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 12, 0x12);
        fx.group_anew(store, 40);
    }
    let (count, rewound) = counted(up);
    assert_eq!((count, rewound), (3, 3), "the new history's first cut");
    let (reply, mut answer) = tokio::sync::oneshot::channel();
    up.control(Control::Ask {
        request: asked(state, "t", 1, 40),
        rewound,
        reply,
    });
    answer.try_recv().expect("answered at once")
}

/// B3 of the review of #140 (work package M2.6a), a cut of the old
/// history committed: cut 1 (group 30) is committed, the agent reads
/// *C* = 1, and cut 2 (group 51) is committed. Then the rewind and the
/// ask ([`a_rewind_and_an_ask_with_an_older_count`]): with the store's
/// mark given with it, the request is refused `rewound` at once, though
/// the uploader's own mark is still 0. Asked again with the count read
/// anew, it is published on the next cut above the abandoned group.
///
/// Mutant: the mark given with the request not taken (the uploader's
/// own is 0 until its check of cut 3): the request is accepted, its
/// lookup finds `snap/t` absent and gives it cut 2, of the old history,
/// which has no `pool@t`, and it is published there before cut 3 is
/// checked.
#[tokio::test(start_paused = true)]
async fn a_publish_asked_after_a_rewind_with_an_older_count_is_refused_beside_a_committed_cut() {
    let mut fx = Fixture::new(88).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 30);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 11, 0x11);
        fx.group(store, 51);
    }
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(51, "cut 2").await;

    let answer = a_rewind_and_an_ask_with_an_older_count(&mut fx, &mut up, state);
    assert_eq!(answer, Answer::Refused(Refusal::Rewound));
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(40, "cut 3").await;
    assert_eq!(snap(&fx, "t").await.0, None, "no cut carries it");

    // Asked again with the count read anew (the agent checks `pool@t`
    // first, M2.6): the next cut above the abandoned group 51 carries it.
    // The cuts of groups 41 to 51 would each be marked, the cost of rule
    // (R) after `-T`.
    let (count, rewound) = counted(&mut up);
    let (reply, answer) = tokio::sync::oneshot::channel();
    up.control(Control::Ask {
        request: asked(state, "t", count, 40),
        rewound,
        reply,
    });
    assert_eq!(answer.await.ok(), Some(Answer::Accepted));
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 13, 0x13);
        fx.group(store, 52);
    }
    for _ in 0..100 {
        let wait = up.turn().await;
        if up.publishing().progress("t") != Progress::Pending {
            break;
        }
        tokio::time::advance(wait).await;
    }
    let head = head_of(&fx).await;
    assert_eq!(up.publishing().progress("t"), Progress::Done(head));
    fx.assert_uploaded(52, "cut 4").await;
}

/// B3 of the review of #140 (work package M2.6a), a cut of the old
/// history uploading: cut 1 (group 30) is committed, the agent reads
/// *C* = 1, and cut 2 (group 51) is read and synced, its commit the
/// uploader's next step. Then the rewind and the ask
/// ([`a_rewind_and_an_ask_with_an_older_count`]): with the store's mark
/// given with it, the request is refused `rewound` at once, and no cut
/// carries it.
///
/// Mutant: the mark given with the request not taken: the request is
/// accepted and waits, and cut 2's commit hands it cut 2, of the old
/// history, before the check of cut 3 gives the uploader the mark.
#[tokio::test(start_paused = true)]
async fn a_publish_asked_after_a_rewind_with_an_older_count_is_refused_while_a_cut_uploads() {
    let mut fx = Fixture::new(89).await;
    let mut store = fx.start();
    fx.write(&mut store, 10, 0x10);
    fx.group(&mut store, 30);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 100).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 11, 0x11);
        fx.group(store, 51);
    }
    until_doing(&mut up, Doing::Committing, 100).await;

    let answer = a_rewind_and_an_ask_with_an_older_count(&mut fx, &mut up, state);
    assert_eq!(answer, Answer::Refused(Refusal::Rewound));
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(40, "cut 3").await;
    assert_eq!(snap(&fx, "t").await.0, None, "no cut carries it");
    assert_eq!(
        up.publishing().progress("t"),
        Progress::Unknown,
        "refused at the ask: never pending"
    );
}

/// The channel's requests, each answered between two turns, do not hold
/// back the lease's renewal, while each of the store's checks takes 21 s
/// (two thirds of the lease and more): no two takes of the lease are
/// more than a third of it apart, the first at 0 (F1 of the review of
/// #127, its scenario: a request 0.25 s before a renewal is due, and a
/// store answer of 21 s). Each request is answered, after the check
/// under way at most.
///
/// Mutant: a store call made for a request between two turns, outside
/// the lease's keeping (the code at 1595d6f: the store's state read at a
/// publish's ask): the row lapses.
#[tokio::test(start_paused = true)]
async fn the_channels_requests_do_not_hold_back_the_leases_renewal() {
    let fx = Fixture::new(95).await;
    let store = fx.start();
    let state = store.cut_state_id().expect("the cut state");
    let delay = Arc::new(AtomicU64::new(21_000));
    let slow = Slow {
        inner: store,
        delay: Arc::clone(&delay),
    };
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(slow, way.keeper(&fx), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (inbox, requests) = tokio::sync::mpsc::channel(4);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = up.run_with(
        async move {
            let _ = stopped.await;
        },
        requests,
        |_| {},
    );
    let begun = tokio::time::Instant::now();
    let third = Duration::from_millis(TTL / 3);
    let client = async move {
        let mut answered = Vec::new();
        for i in 1..=6u32 {
            // A quarter second before the i-th renewal is due.
            tokio::time::sleep_until(begun + third * i - IDLE).await;
            let name = format!("t{i}");
            let answer = match i % 3 {
                1 => format!(
                    "{:?}",
                    ask_of(&inbox, |r| Control::Ask {
                        request: asked(state, &name, 0, 100),
                        rewound: 0,
                        reply: r,
                    })
                    .await
                ),
                2 => format!(
                    "{:?}",
                    ask_of(&inbox, |r| Control::Progress(name.clone(), r)).await
                ),
                _ => format!(
                    "{:?}",
                    ask_of(&inbox, |r| Control::Withdraw(name.clone(), r)).await
                ),
            };
            answered.push(answer);
        }
        tokio::time::sleep_until(begun + Duration::from_secs(80)).await;
        let _ = stop.send(());
        answered
    };
    let (_keeper, answered) = tokio::join!(run, client);
    assert_eq!(
        answered,
        ["Accepted", "Unknown", "Unknown", "Accepted", "Unknown", "Unknown"],
        "each request is answered"
    );
    let takes = way.takes();
    on_time(&takes, "requests 0.25 s before each renewal");
    assert!(takes.len() >= 8, "{takes:?}");
}

/// An uploader of `store` whose remote notes the publishes in `sent`,
/// refuses those of the names `refuse` holds for, each after `slow`, and
/// fails the cuts' segment PUTs while `cuts_fail` is set. Slices of 16
/// blocks, and PUTs that take no time.
fn noting(
    fx: &Fixture,
    clock: &Clock,
    store: InPlace<SimDisk>,
    (refuse, slow): (fn(&str) -> bool, Duration),
    cuts_fail: &Arc<AtomicBool>,
    sent: &Arc<Mutex<Vec<(String, tokio::time::Instant)>>>,
) -> Uploader<InPlace<SimDisk>, Publishes<Fenced>, LeaseRefs, impl FnMut(u64) -> Publishes<Fenced>>
{
    let shape = Shape {
        refuse,
        slow,
        made_slow: Duration::ZERO,
        cut_slow: Duration::ZERO,
        slices: 16,
    };
    noting_shaped(fx, clock, store, shape, cuts_fail, sent)
}

/// How the remote of [`noting_shaped`] answers, and the uploader's
/// slices.
#[derive(Clone, Copy)]
struct Shape {
    /// The names whose publishes it refuses.
    refuse: fn(&str) -> bool,
    /// How long a refused publish takes to fail.
    slow: Duration,
    /// How long a publish that is made takes.
    made_slow: Duration,
    /// How long each PUT of a cut's sealed segments takes.
    cut_slow: Duration,
    /// The blocks of a slice.
    slices: usize,
}

/// As [`noting`], of the `shape` given.
fn noting_shaped(
    fx: &Fixture,
    clock: &Clock,
    store: InPlace<SimDisk>,
    shape: Shape,
    cuts_fail: &Arc<AtomicBool>,
    sent: &Arc<Mutex<Vec<(String, tokio::time::Instant)>>>,
) -> Uploader<InPlace<SimDisk>, Publishes<Fenced>, LeaseRefs, impl FnMut(u64) -> Publishes<Fenced>>
{
    let state = store.cut_state_id().expect("the cut state");
    let mut make = fenced(fx, clock);
    let (cuts_fail, sent) = (Arc::clone(cuts_fail), Arc::clone(sent));
    let mut up = Uploader::with_slices(
        store,
        keeper(fx, clock),
        move |epoch| Publishes {
            inner: make(epoch),
            refuse: shape.refuse,
            slow: shape.slow,
            made_slow: shape.made_slow,
            cut_slow: shape.cut_slow,
            cuts_fail: Arc::clone(&cuts_fail),
            sent: Arc::clone(&sent),
        },
        shape.slices,
    );
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    up
}

/// One turn of `up`, the lease kept on its own clock, and the time
/// moved on by the wait it asks for (a millisecond at least).
async fn one_turn<L, R, F, M>(up: &mut Uploader<L, R, F, M>, clock: &Clock)
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let wait = up.turn().await;
    clock.pass(wait.as_millis() as u64);
    tokio::time::advance(wait.max(Duration::from_millis(1))).await;
}

/// Turn `up` for `time`, the lease kept on its own clock.
async fn turns_for<L, R, F, M>(up: &mut Uploader<L, R, F, M>, clock: &Clock, time: Duration)
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let began = tokio::time::Instant::now();
    while began.elapsed() < time {
        let wait = up.turn().await;
        clock.pass(wait.as_millis() as u64);
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
}

/// D2 of the review of #122 (its u6 and u7): a publish that cannot be
/// made, its transaction refused every time, holds up no publish asked
/// after it. `b`, asked a second after `a`, is published at once, while
/// `a` is sent again at waits of its own that grow. With an expiry of
/// two minutes, `b` is published long before its deadline and `a` ends
/// expired.
#[tokio::test(start_paused = true)]
async fn a_publish_that_cannot_be_made_holds_up_no_other() {
    for (seed, expiry) in [(80, EXPIRY_MAX), (81, Duration::from_secs(120))] {
        let case = format!("expiry {expiry:?}");
        let mut fx = Fixture::new(seed).await;
        let store = one_cut(&mut fx);
        let state = store.cut_state_id().expect("the cut state");
        let clock = Clock::new();
        let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
        let refuse_a: fn(&str) -> bool = |name| name == "a";
        let mut up = noting(
            &fx,
            &clock,
            store,
            (refuse_a, Duration::ZERO),
            &cuts_fail,
            &sent,
        );
        until_idle(&mut up, 100).await;
        fx.assert_uploaded(7, "cut 1").await;
        let began = tokio::time::Instant::now();
        for name in ["a", "b"] {
            let mut request = asked(state, name, 0, 7);
            request.expiry = expiry;
            assert_eq!(up.publishing().ask(request), Answer::Accepted, "{case}");
            turns_for(&mut up, &clock, Duration::from_secs(1)).await;
        }
        turns_for(&mut up, &clock, Duration::from_secs(600)).await;
        let (a, b) = (sent_of(&sent, "a", began), sent_of(&sent, "b", began));
        println!("{case}: a sent at {a:?}, b at {b:?}");
        assert_eq!(b.len(), 1, "{case}: b sent once: {b:?}");
        assert!(
            b[0] < Duration::from_secs(2),
            "{case}: b waited for a: {b:?}"
        );
        assert_eq!(snap(&fx, "b").await.0, Some(head_of(&fx).await), "{case}");
        assert_eq!(
            up.publishing().progress("b"),
            Progress::Done(head_of(&fx).await)
        );
        waits_grow(&a, &case);
        assert_eq!(snap(&fx, "a").await, (None, None), "{case}");
        let a_ends = if expiry == EXPIRY_MAX {
            Progress::Pending
        } else {
            Progress::Expired
        };
        assert_eq!(up.publishing().progress("a"), a_ends, "{case}");
    }
}

/// D2 of the review of #122 (its N07): a publish that has failed several
/// times and then ends leaves no wait behind it. `a` is refused four
/// times, waiting a second, then 2, then 4, and is withdrawn; `c`, asked
/// after and refused too, is sent again a second after its first
/// refusal, not after the wait that `a` would have had next.
#[tokio::test(start_paused = true)]
async fn a_publish_that_ends_leaves_no_wait_to_the_next() {
    let mut fx = Fixture::new(82).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let mut up = noting(
        &fx,
        &clock,
        store,
        (every_name, Duration::ZERO),
        &cuts_fail,
        &sent,
    );
    until_idle(&mut up, 100).await;
    let began = tokio::time::Instant::now();
    assert_eq!(
        up.publishing().ask(asked(state, "a", 0, 7)),
        Answer::Accepted
    );
    while sent_of(&sent, "a", began).len() < 4 {
        assert!(began.elapsed() < Duration::from_secs(60), "a is not sent");
        turns_for(&mut up, &clock, Duration::from_millis(100)).await;
    }
    // Pending until its lookup, after its wait.
    assert_eq!(up.publishing().withdraw("a"), Progress::Pending);
    turns_for(&mut up, &clock, Duration::from_secs(20)).await;
    assert_eq!(up.publishing().progress("a"), Progress::Withdrawn);
    let asked_c = tokio::time::Instant::now();
    assert_eq!(
        up.publishing().ask(asked(state, "c", 0, 7)),
        Answer::Accepted
    );
    turns_for(&mut up, &clock, Duration::from_secs(5)).await;
    let c = sent_of(&sent, "c", asked_c);
    println!("a sent at {:?}, c at {c:?}", sent_of(&sent, "a", began));
    assert!(c.len() >= 2, "c is sent again within 5 s: {c:?}");
    let gap = c[1] - c[0];
    assert!(
        gap >= Duration::from_secs(1) && gap < Duration::from_millis(1500),
        "c's first wait is a second: {c:?}"
    );
}

/// D2 of the review of #122 (its N09): a publish's own wait wakes the
/// uploader while the cut waits longer. Cut 2's segment PUTs fail until
/// its backoff is past a minute; a publish then refused is sent again a
/// second after, not at the cut's next step or the lease's renewal.
#[tokio::test(start_paused = true)]
async fn a_publishs_wait_wakes_the_uploader_while_the_cut_waits_longer() {
    let mut fx = Fixture::new(83).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let mut up = noting(
        &fx,
        &clock,
        store,
        (every_name, Duration::ZERO),
        &cuts_fail,
        &sent,
    );
    until_idle(&mut up, 100).await;
    cuts_fail.store(true, Ordering::SeqCst);
    {
        let local = up.driver().expect("the driver").local();
        fx.write(local, 20, 0x20);
        fx.group(local, 8);
    }
    turns_for(&mut up, &clock, Duration::from_secs(200)).await;
    let State::Backoff { wait, .. } = up.shown().state else {
        panic!("cut 2 is not in its backoff: {:?}", up.shown().state);
    };
    assert!(wait >= Duration::from_secs(64), "the cut waits {wait:?}");
    let began = tokio::time::Instant::now();
    assert_eq!(
        up.publishing().ask(asked(state, "t", 0, 7)),
        Answer::Accepted
    );
    turns_for(&mut up, &clock, Duration::from_secs(5)).await;
    let t = sent_of(&sent, "t", began);
    println!("the cut waits {wait:?}; t sent at {t:?}");
    assert!(t.len() >= 2, "t is sent again within 5 s: {t:?}");
    let gap = t[1] - t[0];
    assert!(
        gap >= Duration::from_secs(1) && gap < Duration::from_millis(1500),
        "t's first wait is a second: {t:?}"
    );
}

/// F1 of the review of #126 (its v1): publishes that fail slowly hold the
/// cut back for one publish step at most. N publishes, carried by cut 1,
/// each fail after T (a store that lets their transaction time out), and
/// cut 2, of one slice, is written: it goes up within T and a second,
/// while they go on being sent. With a wait of each publish's own and a
/// publish's step always taken before the cut's, 10 publishes failing
/// after 20 s kept cut 2 back for over half an hour: there was always
/// another publish due. The bound is one step (F6 of the review's next
/// look); a long cut is [`publishes_that_fail_slowly_slow_a_long_cut_by_one_step_at_most`].
#[tokio::test(start_paused = true)]
async fn publishes_that_fail_slowly_hold_the_cut_back_for_one_step_at_most() {
    let cases = [
        (89, 10, Duration::from_secs(20)),
        (90, 20, Duration::from_secs(20)),
        (91, 64, Duration::from_secs(5)),
    ];
    for (seed, n, slow) in cases {
        let case = format!("{n} publishes failing after {slow:?}");
        let mut fx = Fixture::new(seed).await;
        let store = one_cut(&mut fx);
        let state = store.cut_state_id().expect("the cut state");
        let clock = Clock::new();
        let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
        let mut up = noting(&fx, &clock, store, (every_name, slow), &cuts_fail, &sent);
        until_idle(&mut up, 100).await;
        fx.assert_uploaded(7, "cut 1").await;
        for i in 0..n {
            let request = asked(state, &format!("p{i}"), 0, 7);
            assert_eq!(up.publishing().ask(request), Answer::Accepted, "{case}");
        }
        {
            let local = up.driver().expect("the driver").local();
            fx.write(local, 20, 0x20);
            fx.group(local, 8);
        }
        let began = tokio::time::Instant::now();
        let bound = slow + Duration::from_secs(1);
        while up.shown().figures.uploaded < 2 {
            assert!(
                began.elapsed() <= bound,
                "{case}: cut 2 is not up after {:?}",
                began.elapsed()
            );
            one_turn(&mut up, &clock).await;
        }
        let up_at = began.elapsed();
        let sends = sent.lock().expect("the times").len();
        println!("{case}: cut 2 up at {up_at:?}, after {sends} publishes sent");
        fx.assert_uploaded(8, &case).await;
        assert!(sends >= 1, "{case}: the publishes are sent meanwhile");
        assert_eq!(up.publishing().pending().0, n as usize, "{case}");
    }
}

/// F4 of the review of #126 (its w6): publishes that fail slowly slow a
/// long cut by one publish step at most, not by one for each of its
/// steps. Ten publishes, carried by cut 1, each fail after 20 s; cut 2 is
/// 124 blocks in slices of one, about 130 steps. It goes up within one
/// publish step and a second: the cut's steps take no time here, so
/// after a publish's step of 20 s the cut has the next 20 s, which is the
/// whole cut. With turns counted in steps the cut took one slice for each
/// failed publish step, and went up only when the publishes expired, ten
/// minutes after they were asked. Once the cut is idle, the publishes go
/// on being sent.
#[tokio::test(start_paused = true)]
async fn publishes_that_fail_slowly_slow_a_long_cut_by_one_step_at_most() {
    let (n, slow) = (10, Duration::from_secs(20));
    let mut fx = Fixture::new(93).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let shape = Shape {
        refuse: every_name,
        slow,
        made_slow: Duration::ZERO,
        cut_slow: Duration::ZERO,
        slices: 1,
    };
    let mut up = noting_shaped(&fx, &clock, store, shape, &cuts_fail, &sent);
    // Cut 1, whole, in slices of one block.
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1").await;
    for i in 0..n {
        let request = asked(state, &format!("p{i}"), 0, 7);
        assert_eq!(up.publishing().ask(request), Answer::Accepted);
    }
    {
        let local = up.driver().expect("the driver").local();
        for block in 20..144 {
            fx.write(local, block, 0x20);
        }
        fx.group(local, 8);
    }
    let began = tokio::time::Instant::now();
    let bound = slow + Duration::from_secs(1);
    let mut turns = 0;
    while up.shown().figures.uploaded < 2 {
        assert!(
            began.elapsed() <= bound,
            "cut 2 is not up after {:?}",
            began.elapsed()
        );
        one_turn(&mut up, &clock).await;
        turns += 1;
    }
    let up_at = began.elapsed();
    let sends = sent.lock().expect("the times").len();
    println!("cut 2 up at {up_at:?}, after {turns} turns and {sends} publishes sent");
    assert!(turns > 124, "a step for each slice: {turns} turns");
    fx.assert_uploaded(8, "cut 2").await;
    let idle_at = tokio::time::Instant::now();
    turns_for(&mut up, &clock, Duration::from_secs(120)).await;
    let after = sent
        .lock()
        .expect("the times")
        .iter()
        .filter(|(_, at)| *at >= idle_at)
        .count();
    println!("{after} publishes sent in the two minutes after");
    // One every 20 s, each sent once the last has failed: six at most.
    assert!(
        after >= 5,
        "the publishes go on being sent once the cut is idle: {after}"
    );
    assert_eq!(up.publishing().pending().0, n);
}

/// F7 of the review of #126 (its x4): one publish whose steps fail
/// slowly slows a long cut to half its pace at most, whatever its waits.
/// Its step fails after 120 s, and its wait after each failure is 1 s,
/// then 2 s, 4 s and so on; cut 2 is 124 blocks in slices of one, each of
/// its PUTs taking a second. Cut 2 must go up within twice the time it
/// takes with no publish, and one publish step. When the count of the
/// time started again at each of the publish's waits, the cut had only
/// the waits until one grew longer than a step: 1216 s, against 616 s
/// with the count kept and 256 s with no publish.
#[tokio::test(start_paused = true)]
async fn one_publish_that_fails_slowly_slows_a_long_cut_to_half_its_pace_at_most() {
    let slow = Duration::from_secs(120);
    let alone = a_long_cut_beside(0, slow).await;
    let beside = a_long_cut_beside(1, slow).await;
    println!("cut 2 up in {alone:?} alone and in {beside:?} beside one publish");
    assert!(
        beside <= 2 * alone + slow,
        "cut 2 up in {beside:?} beside one publish, {alone:?} alone"
    );
}

/// How long cut 2, 124 blocks in slices of one with PUTs of a second
/// each, takes to go up beside `n` publishes, carried by cut 1 and with
/// the longest expiry, whose steps each fail after `slow`.
async fn a_long_cut_beside(n: usize, slow: Duration) -> Duration {
    let mut fx = Fixture::new(95).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let shape = Shape {
        refuse: every_name,
        slow,
        made_slow: Duration::ZERO,
        cut_slow: Duration::from_secs(1),
        slices: 1,
    };
    let mut up = noting_shaped(&fx, &clock, store, shape, &cuts_fail, &sent);
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1").await;
    for i in 0..n {
        let request = Request {
            expiry: EXPIRY_MAX,
            ..asked(state, &format!("p{i}"), 0, 7)
        };
        assert_eq!(up.publishing().ask(request), Answer::Accepted);
    }
    {
        let local = up.driver().expect("the driver").local();
        for block in 20..144 {
            fx.write(local, block, 0x20);
        }
        fx.group(local, 8);
    }
    let began = tokio::time::Instant::now();
    while up.shown().figures.uploaded < 2 {
        assert!(
            began.elapsed() <= EXPIRY_MAX,
            "{n} publishes: cut 2 is not up after {:?}",
            began.elapsed()
        );
        one_turn(&mut up, &clock).await;
    }
    fx.assert_uploaded(8, "cut 2").await;
    assert_eq!(up.publishing().pending().0, n, "the publishes pend still");
    began.elapsed()
}

/// F5 of the review of #126 (its w4): slow steps of the cut slow the
/// publishes by one step of the cut at most, not by one for each publish
/// step. Cut 2, 40 blocks in slices of one, goes up while each of its
/// steps takes 10 s (two PUTs of 5 s); 64 publishes carried by cut 1 are
/// asked once it is under way. All are made within one of the cut's steps
/// and a second, before cut 2 is up: the publishes' steps take no time
/// here, so after a step of the cut's they have the next 10 s. With turns
/// counted in steps, a step of the cut's came between a publish's lookup
/// and its transaction, and between two publishes.
#[tokio::test(start_paused = true)]
async fn slow_steps_of_the_cut_slow_the_publishes_by_one_step_at_most() {
    let (n, put) = (64, Duration::from_secs(5));
    let mut fx = Fixture::new(94).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let shape = Shape {
        refuse: no_name,
        slow: Duration::ZERO,
        made_slow: Duration::ZERO,
        cut_slow: put,
        slices: 1,
    };
    let mut up = noting_shaped(&fx, &clock, store, shape, &cuts_fail, &sent);
    // Cut 1, whole, in slices of one block.
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1").await;
    {
        let local = up.driver().expect("the driver").local();
        for block in 20..60 {
            fx.write(local, block, 0x20);
        }
        fx.group(local, 8);
    }
    for _ in 0..4 {
        one_turn(&mut up, &clock).await;
    }
    assert!(
        up.driver().expect("the driver").under_way().is_some(),
        "cut 2 is under way"
    );
    for i in 0..n {
        let request = asked(state, &format!("p{i}"), 0, 7);
        assert_eq!(up.publishing().ask(request), Answer::Accepted);
    }
    let began = tokio::time::Instant::now();
    let bound = 2 * put + Duration::from_secs(1);
    while up.publishing().figures().published < n {
        assert!(
            began.elapsed() <= bound,
            "{} of {n} made after {:?}",
            up.publishing().figures().published,
            began.elapsed()
        );
        one_turn(&mut up, &clock).await;
    }
    println!("{n} publishes made in {:?}", began.elapsed());
    assert_eq!(up.shown().figures.uploaded, 1, "cut 2 is still going up");
    for _ in 0..200 {
        if up.shown().figures.uploaded == 2 {
            break;
        }
        one_turn(&mut up, &clock).await;
    }
    fx.assert_uploaded(8, "cut 2").await;
    let figures = up.publishing().figures();
    assert_eq!((figures.published, figures.expired), (n, 0));
}

/// The mirror of F7, y3 of the last look at #126: steps of the cut that
/// fail slowly take from the publishes no more than one step of the
/// cut's. Cut 2, 40 blocks in slices of one, is under way when its PUTs
/// start to fail, each after 60 s, the cut waiting 1 s, 2 s, 4 s and on
/// after each. Ten publishes carried by cut 1, whose transactions take
/// 10 s each, are asked then. All are made within twice their steps'
/// time and one step of the cut's, 260 s: what the publishes are owed of
/// the shared time outlives the cut's waits. With the count started
/// again at each failure of the cut's step, the publishes had one step
/// for each of the cut's, 460 s.
#[tokio::test(start_paused = true)]
async fn cut_steps_that_fail_slowly_slow_the_publishes_by_one_step_at_most() {
    let (n, put, made) = (10, Duration::from_secs(60), Duration::from_secs(10));
    let mut fx = Fixture::new(96).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let shape = Shape {
        refuse: no_name,
        slow: Duration::ZERO,
        made_slow: made,
        cut_slow: put,
        slices: 1,
    };
    let mut up = noting_shaped(&fx, &clock, store, shape, &cuts_fail, &sent);
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1").await;
    {
        let local = up.driver().expect("the driver").local();
        for block in 20..60 {
            fx.write(local, block, 0x20);
        }
        fx.group(local, 8);
    }
    for _ in 0..4 {
        one_turn(&mut up, &clock).await;
    }
    assert!(
        up.driver().expect("the driver").under_way().is_some(),
        "cut 2 is under way"
    );
    cuts_fail.store(true, Ordering::SeqCst);
    for i in 0..n {
        let request = asked(state, &format!("p{i}"), 0, 7);
        assert_eq!(up.publishing().ask(request), Answer::Accepted);
    }
    let began = tokio::time::Instant::now();
    let bound = 2 * made * n as u32 + put;
    while up.publishing().figures().published < n {
        assert!(
            began.elapsed() <= bound,
            "{} of {n} made after {:?}",
            up.publishing().figures().published,
            began.elapsed()
        );
        one_turn(&mut up, &clock).await;
    }
    println!("{n} publishes made in {:?}", began.elapsed());
    assert_eq!(up.shown().figures.uploaded, 1, "cut 2's PUTs fail");
    let figures = up.publishing().figures();
    assert_eq!((figures.published, figures.expired), (n, 0));
}

/// F2 of the review of #126 (its v5): a failed step's wait goes to the
/// publish that took the step, not to the oldest that pends. `w`, asked
/// first, waits for a cut after cut 1; `y`, asked after and refused
/// every time, is sent again at waits of its own that grow. Were the
/// wait given to `w`, which has no step to take, `y` would be sent at
/// every turn.
#[tokio::test(start_paused = true)]
async fn a_failed_steps_wait_goes_to_the_publish_that_took_it() {
    let mut fx = Fixture::new(92).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let (cuts_fail, sent) = (Arc::new(AtomicBool::new(false)), Arc::default());
    let refuse_y: fn(&str) -> bool = |name| name == "y";
    let mut up = noting(
        &fx,
        &clock,
        store,
        (refuse_y, Duration::ZERO),
        &cuts_fail,
        &sent,
    );
    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "cut 1").await;
    let began = tokio::time::Instant::now();
    assert_eq!(
        up.publishing().ask(asked(state, "w", 1, 8)),
        Answer::Accepted
    );
    assert_eq!(
        up.publishing().ask(asked(state, "y", 0, 7)),
        Answer::Accepted
    );
    turns_for(&mut up, &clock, Duration::from_secs(60)).await;
    let y = sent_of(&sent, "y", began);
    println!("y sent at {y:?}");
    waits_grow(
        &y,
        "a younger publish refused while an older waits for a cut",
    );
    assert!(sent_of(&sent, "w", began).is_empty());
    assert_eq!(up.publishing().progress("w"), Progress::Pending);
    assert_eq!(up.publishing().progress("y"), Progress::Pending);
}

/// Turn until the driver reads a cut, moving tokio's time by each wait;
/// at most `limit` turns.
async fn until_reading<L, R, F, M>(up: &mut Uploader<L, R, F, M>, limit: usize)
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    until_doing(up, Doing::Reading, limit).await;
}

/// Turn until the driver is about to take `doing` on a cut, moving
/// tokio's time by each wait; at most `limit` turns.
async fn until_doing<L, R, F, M>(up: &mut Uploader<L, R, F, M>, doing: Doing, limit: usize)
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let mut states = Vec::new();
    for _ in 0..limit {
        let wait = up.turn().await;
        let state = up.shown().state;
        if matches!(&state, State::Working(d, Some(_)) if *d == doing) {
            return;
        }
        states.push(state);
        tokio::time::advance(wait).await;
    }
    panic!(
        "not {doing:?} after {limit} turns: {states:?}, {:?}",
        up.driver().map(|d| d.figures().clone())
    );
}

/// Overwrite more blocks of the cut being read than the bound has room
/// for copies of, so that the store abandons it, and hold its heir at
/// group `group`.
fn over_the_bound<R, F, M>(
    fx: &mut Fixture,
    up: &mut Uploader<InPlace<SimDisk>, R, F, M>,
    byte: u8,
    group: u64,
) where
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let store = up.driver().expect("the driver").local();
    for block in 0..OVER {
        fx.write(store, 100 + block, byte);
    }
    fx.group(store, group);
}

/// An uploader of the in-place store, its remote fenced.
type Turned = Uploader<InPlace<SimDisk>, Fenced, LeaseRefs, Box<dyn FnMut(u64) -> Fenced>>;

/// Turn until a whole cut is lost, then until the next cut's step: the
/// wait the loss began, how long it was until the next cut's step, and
/// the turns between (the lease's, which go on). Each sleep the uploader
/// asks for meanwhile, the one of the turn that lost the cut first, must
/// leave the lease's renewal on time: a third of the lease at most.
async fn a_whole_cut_lost<L, R, F, M>(up: &mut Uploader<L, R, F, M>) -> (Duration, Duration, usize)
where
    L: skyzfs_upload::Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    let mut lost = None;
    for _ in 0..10 {
        let wait = up.turn().await;
        if let State::WholeLost { wait: began, why } = up.shown().state {
            assert!(why.contains("went before its commit"), "{why}");
            lost = Some((began, wait));
            break;
        }
        tokio::time::advance(wait).await;
    }
    let (wait_began, mut sleep) =
        lost.unwrap_or_else(|| panic!("no whole cut lost: {:?}", up.shown()));
    let lost_at = tokio::time::Instant::now();
    let mut turns = 0;
    loop {
        assert!(
            sleep <= Duration::from_millis(TTL / 3),
            "a sleep of {sleep:?} in a wait of {wait_began:?}, after {turns} turns"
        );
        tokio::time::advance(sleep).await;
        sleep = up.turn().await;
        if !matches!(up.shown().state, State::WholeLost { .. }) {
            return (wait_began, lost_at.elapsed(), turns);
        }
        turns += 1;
        assert!(turns < 1000, "the wait does not end");
    }
}

/// A whole cut that goes before its commit is followed by a wait before
/// the next cut's step: a minute, doubling at each whole cut lost in a
/// row, to fifteen. The lease is kept meanwhile, and no cut's step is
/// taken. A cut committed ends the waits: the next whole cut lost waits
/// a minute again (M2.4f).
#[tokio::test(start_paused = true)]
async fn a_whole_cut_lost_waits_a_minute_doubling_to_fifteen_until_a_cut_is_committed() {
    let mut fx = Fixture::new(101).await;
    let mut store = fx.start();
    // The first cut holds the whole volume, with more blocks to read
    // than a slice: it is read over several turns.
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    let mut waits = Vec::new();
    for (i, group) in (8..15).enumerate() {
        until_reading(&mut up, 100).await;
        over_the_bound(&mut fx, &mut up, group as u8, group);
        let (wait, waited, turns) = a_whole_cut_lost(&mut up).await;
        assert_eq!(
            waited, wait,
            "attempt {i}: the next cut's step comes at the wait's end"
        );
        // The lease is renewed every third of its time meanwhile.
        assert!(
            turns as u64 >= wait.as_secs() / (TTL / 3000) - 1,
            "attempt {i}: {turns} turns in {wait:?}"
        );
        waits.push(wait.as_secs());
    }
    assert_eq!(waits, [60, 120, 240, 480, 900, 900, 900]);
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!(
        (figures.lost, figures.lost_whole, figures.uploaded),
        (7, 7, 0)
    );
    assert_eq!(up.shown().figures.lost_whole, 7);
    assert_eq!(fx.uploaded().await.group, None, "nothing was committed");

    // The load drops: the heir goes up whole, and the waits end.
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(14, "the heir of the last whole cut lost")
        .await;
    assert_eq!(up.driver().expect("the driver").figures().uploaded_whole, 1);

    // Another writer names another cut of head's group in the row:
    // the next cut's check finds the mismatch, and the cut after it
    // holds the whole volume, which head allows. Lost, it waits a minute
    // again, and not the fifteen of the last wait.
    fx.put_row_on_head(*b"another state...", 1, 14).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 20);
    }
    until_idle(&mut up, 1000).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!(
        (figures.mismatches, figures.checked),
        (1, skyzfs_upload::Checked::Mismatched(Mismatch::NotTheBase))
    );
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    let (wait, waited, _) = a_whole_cut_lost(&mut up).await;
    assert_eq!(
        (wait, waited),
        (Duration::from_secs(60), wait),
        "the waits start again"
    );
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded_whole), (8, 1));
    // The load drops again: the heir goes up whole onto head.
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(22, "the heir of the whole cut lost after the commit")
        .await;
}

/// A delta cut that goes before its commit does not wait: the next
/// cut's step comes at once, and its heir goes up onto the same base
/// (M2.4f's wait is for whole cuts only).
#[tokio::test(start_paused = true)]
async fn a_delta_cut_lost_does_not_wait() {
    let mut fx = Fixture::new(102).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    // Cut 2 holds more blocks than the bound, so that overwriting them
    // all abandons it.
    {
        let store = up.driver().expect("the driver").local();
        for block in 0..OVER {
            fx.write(store, 100 + block, 0x22);
        }
        fx.group(store, 8);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x33, 9);
    let mut lost = None;
    for _ in 0..10 {
        let wait = up.turn().await;
        if up.driver().expect("the driver").figures().lost == 1 {
            lost = Some((wait, up.shown().state));
            break;
        }
        tokio::time::advance(wait).await;
    }
    let (wait, state) = lost.expect("cut 2 lost");
    assert_eq!(wait, Duration::ZERO, "no wait after a delta cut lost");
    assert!(
        matches!(state, State::Working(Doing::Checking, None)),
        "{state:?}"
    );
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(9, "the heir, as a delta").await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost, figures.lost_whole), (1, 0));
    assert_eq!((figures.uploaded, figures.uploaded_whole), (2, 1));
}

// ------------------------------- the review of #129: T1 to T5 of M2.4f

/// Move tokio's time and the refs object's clock together.
async fn both_pass(clock: &Clock, by: Duration) {
    tokio::time::advance(by).await;
    clock.pass(u64::try_from(by.as_millis()).expect("ms"));
}

/// Turn until the uploader's state is one that `want` holds for, both
/// clocks moving by each wait; at most `limit` turns. The wait the turn
/// that reached it asked for.
async fn until_state(
    up: &mut Turned,
    clock: &Clock,
    limit: usize,
    want: impl Fn(&State) -> bool,
) -> Duration {
    let mut states = Vec::new();
    for _ in 0..limit {
        let wait = up.turn().await;
        let state = up.shown().state;
        if want(&state) {
            return wait;
        }
        states.push(state);
        both_pass(clock, wait).await;
    }
    panic!("not reached after {limit} turns: {states:?}");
}

/// T2 of the review of #129 (its R1): through whole waits up to the
/// longest, the lease is kept by the refs object's clock. Whenever the
/// uploader wakes during a wait, the row as last kept is still ahead of
/// that clock: checked after each sleep and before the turn, which would
/// take a lapsed lease again. [The test of the waits][waits] moves
/// tokio's time alone, so a lease that lapsed would not show there.
///
/// [waits]: a_whole_cut_lost_waits_a_minute_doubling_to_fifteen_until_a_cut_is_committed
#[tokio::test(start_paused = true)]
async fn the_leases_row_never_lapses_in_the_whole_waits() {
    let mut fx = Fixture::new(201).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    let mut waits = Vec::new();
    for group in 8..14u64 {
        until_state(&mut up, &clock, 100, |s| {
            matches!(s, State::Working(Doing::Reading, Some(_)))
        })
        .await;
        over_the_bound(&mut fx, &mut up, group as u8, group);
        let mut sleep = until_state(&mut up, &clock, 20, |s| {
            matches!(s, State::WholeLost { .. })
        })
        .await;
        let State::WholeLost { wait, .. } = up.shown().state else {
            panic!("{:?}", up.shown().state)
        };
        waits.push(wait.as_secs());
        let mut turns = 0;
        loop {
            both_pass(&clock, sleep).await;
            let now = clock.0.load(Ordering::SeqCst);
            match up.shown().lease {
                LeaseState::Held { expires_at_ms, .. } => assert!(
                    expires_at_ms > now,
                    "wait {wait:?}, turn {turns}: the row expired at {expires_at_ms}, now {now}"
                ),
                other => panic!("wait {wait:?}, turn {turns}: {other:?}"),
            }
            sleep = up.turn().await;
            if !matches!(up.shown().state, State::WholeLost { .. }) {
                break;
            }
            turns += 1;
            assert!(turns < 1000, "the wait does not end");
        }
    }
    assert_eq!(waits, [60, 120, 240, 480, 900, 900]);
}

/// T1 of the review of #129 (its R2): a publish asked during a whole
/// wait, carried by a cut committed before it, is made during the wait,
/// not at its end: the publishes go on meanwhile.
#[tokio::test(start_paused = true)]
async fn a_publish_is_made_during_a_whole_wait() {
    let mut fx = Fixture::new(202).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_state(&mut up, &clock, 1000, |s| *s == State::Idle).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    // Another writer names another state's cut of head's group: the next
    // cut's check finds the mismatch, and the cut after it is whole.
    fx.put_row_on_head(*b"another state...", 1, 7).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 20);
    }
    until_state(&mut up, &clock, 1000, |s| *s == State::Idle).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    until_state(&mut up, &clock, 100, |s| {
        matches!(s, State::Working(Doing::Reading, Some(_)))
    })
    .await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    let mut sleep = until_state(&mut up, &clock, 20, |s| {
        matches!(s, State::WholeLost { .. })
    })
    .await;
    let lost_at = tokio::time::Instant::now();
    let State::WholeLost { wait, .. } = up.shown().state else {
        panic!("{:?}", up.shown().state)
    };
    assert_eq!(wait, Duration::from_secs(60));
    // The agent asks for a publish carried by cut 1 (after 0, group 7).
    let (reply, answer) = tokio::sync::oneshot::channel();
    if up.control(Control::Ask {
        request: asked(state, "p", 0, 7),
        rewound: 0,
        reply,
    }) {
        sleep = Duration::ZERO;
    }
    assert_eq!(answer.await.expect("an answer"), Answer::Accepted);
    let mut turns = 0;
    loop {
        both_pass(&clock, sleep).await;
        sleep = up.turn().await;
        if let Progress::Done(_) = up.publishing().progress("p") {
            break;
        }
        assert!(
            matches!(up.shown().state, State::WholeLost { .. }),
            "the wait ended ({:?} after the loss, {turns} turns) before the publish was made: {:?}",
            lost_at.elapsed(),
            up.publishing().progress("p")
        );
        turns += 1;
        assert!(turns < 1000, "the publish is not made");
    }
    assert!(
        lost_at.elapsed() < wait,
        "made {:?} after the loss",
        lost_at.elapsed()
    );
    assert!(matches!(up.shown().state, State::WholeLost { .. }));
    let (found, _) = snap(&fx, "p").await;
    assert!(found.is_some(), "snap/p");
}

/// T3 of the review of #129 (its R3): a whole cut whose copies fail to
/// sync is lost before its commit and is followed by the whole wait (the
/// test of the waits loses a whole cut at a read).
#[tokio::test(start_paused = true)]
async fn a_whole_cut_lost_at_its_sync_waits() {
    let mut fx = Fixture::new(203).await;
    let mut store = fx.start();
    fx.write(&mut store, 200, 0x20);
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    until_doing(&mut up, Doing::Reading, 100).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 200, 0x21);
        fx.group(store, 8);
    }
    until_doing(&mut up, Doing::Syncing, 200).await;
    fx.disk.fail_sync_of("copies.img");
    let sleep = up.turn().await;
    let state = up.shown().state;
    let State::WholeLost { wait, why } = state else {
        panic!("{state:?}, sleep {sleep:?}")
    };
    assert!(why.contains("went before its commit"), "{why}");
    assert_eq!(wait, Duration::from_secs(60));
    assert_eq!(up.driver().expect("the driver").figures().lost_whole, 1);
}

/// T3 of the review of #129 (its R4): a whole cut abandoned at the bound
/// after its last slice and before its commit is lost at the commit and
/// is followed by the whole wait.
#[tokio::test(start_paused = true)]
async fn a_whole_cut_gone_at_its_commit_waits() {
    let mut fx = Fixture::new(204).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 64);
    until_doing(&mut up, Doing::Committing, 100).await;
    over_the_bound(&mut fx, &mut up, 0x09, 8);
    let sleep = up.turn().await;
    let state = up.shown().state;
    let State::WholeLost { wait, why } = state else {
        panic!("{state:?}, sleep {sleep:?}")
    };
    assert!(
        why.contains("went before its commit: the store no longer held it at its commit"),
        "{why}"
    );
    assert_eq!(wait, Duration::from_secs(60));
    assert_eq!(fx.uploaded().await.group, None, "nothing was committed");
}

/// T4 of the review of #129: a cut found committed, or committed and no
/// longer held, ends the whole waits as a cut committed does. Two whole
/// cuts are lost (waits of 1 and 2 min), and the heir, whole, reaches its
/// commit. Then, as `gone` says:
///
/// - its commit lands, the answer is a conflict and the row and head
///   read after it are those from before: the commit is given up, and
///   the next check finds the cut committed and still held
///   ([`skyzfs_upload::Step::FoundCommitted`]);
/// - its head swap lands and the reply is lost, and the bound takes it
///   before the commit is tried again, which finds it committed and gone
///   from the store ([`skyzfs_upload::Step::UploadedNotHeld`]).
///
/// No other cut is committed after it, and the next whole cut is lost:
/// the cut that took the heir's map when it was gone, or, when the heir
/// was let go, the cut after one that another writer's row makes a
/// mismatch. Lost, it waits a minute, not the four of a third whole cut
/// lost in a row.
async fn a_cut_committed_unheard_ends_the_whole_waits(seed: u64, gone: bool) {
    let mut fx = Fixture::new(seed).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    // The lease apart, so that a fault armed on the store's refs fails
    // the head swap alone.
    let way = Leasing::apart(&fx);
    let mode = Arc::new(AtomicU8::new(PASS));
    let make = Refusals::make(fenced(&fx, &clock), &mode);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), make, 8);
    let mut waits = Vec::new();
    for group in 8..10 {
        until_reading(&mut up, 100).await;
        over_the_bound(&mut fx, &mut up, group as u8, group);
        waits.push(a_whole_cut_lost(&mut up).await.0.as_secs());
    }
    assert_eq!(waits, [60, 120]);
    until_doing(&mut up, Doing::Committing, 100).await;
    if gone {
        fx.store
            .arm_when(FaultOp::Put, "refs", 0, skynbd_store::FaultWhen::After)
            .await;
        let wait = up.turn().await;
        assert!(
            matches!(up.shown().state, State::Backoff { wait, .. } if wait == SECOND),
            "{:?}",
            up.shown().state
        );
        fx.store.disarm().await;
        fx.assert_uploaded(9, "the heir's swap landed").await;
        over_the_bound(&mut fx, &mut up, 0x0a, 10);
        tokio::time::advance(wait).await;
        up.turn().await;
    } else {
        mode.store(LAND_UNSEEN, Ordering::SeqCst);
        assert_eq!(give_ups(&mut up, 1).await, [SECOND]);
        fx.assert_uploaded(9, "the heir's commit given up had landed")
            .await;
        mode.store(PASS, Ordering::SeqCst);
        up.turn().await;
    }
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!(
        (
            figures.found_committed,
            figures.not_held,
            figures.lost_whole
        ),
        (u64::from(!gone), u64::from(gone), 2)
    );
    let uploaded = figures.uploaded;
    if !gone {
        // The heir was let go, and the next cut would be a delta onto
        // it: another writer's row makes its check a mismatch, and the
        // cut after it is whole.
        fx.put_row_on_head(*b"another state...", 1, 9).await;
        {
            let store = up.driver().expect("the driver").local();
            fx.write(store, 30, 0x30);
            fx.group(store, 20);
        }
        until_idle(&mut up, 1000).await;
        assert_eq!(up.driver().expect("the driver").figures().mismatches, 1);
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    // Gone, the heir left its map to the cut after it, which is whole.
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    let (wait, waited, _) = a_whole_cut_lost(&mut up).await;
    assert_eq!(
        (wait, waited),
        (Duration::from_secs(60), wait),
        "the waits start again"
    );
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!(
        (figures.lost_whole, figures.uploaded),
        (3, uploaded),
        "no cut committed since the heir"
    );
}

#[tokio::test(start_paused = true)]
async fn a_cut_found_committed_ends_the_whole_waits() {
    a_cut_committed_unheard_ends_the_whole_waits(205, false).await;
}

#[tokio::test(start_paused = true)]
async fn a_cut_committed_and_gone_ends_the_whole_waits() {
    a_cut_committed_unheard_ends_the_whole_waits(206, true).await;
}

// T5 of the review of #129 (its S1 to S4): only a cut committed ends the
// whole waits. Each of these tests puts one other step between two whole
// cuts lost, with no cut committed between, and the second still waits
// two minutes, not the one minute of waits begun again. The tests of T4
// isolate their step only as long as no such step ends the waits too.

/// The first whole wait.
const MINUTE: Duration = Duration::from_secs(60);

/// T5, S1: a step that fails on object storage (the uploader's error
/// wait) between two whole cuts lost does not end the waits. Transient
/// errors are common under load, and would otherwise hold the waits at a
/// minute.
#[tokio::test(start_paused = true)]
async fn an_error_between_two_whole_cuts_lost_does_not_end_the_waits() {
    let mut fx = Fixture::new(211).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    // The lease apart, so that the fault fails the cut's step alone.
    let way = Leasing::apart(&fx);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), fenced(&fx, &clock), 8);
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x08, 8);
    assert_eq!(a_whole_cut_lost(&mut up).await.0, MINUTE);
    // The heir's next step fails on object storage.
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut seen = None;
    for _ in 0..50 {
        let wait = up.turn().await;
        if let State::Backoff { .. } = up.shown().state {
            seen = Some(wait);
            break;
        }
        tokio::time::advance(wait).await;
    }
    fx.store.disarm().await;
    let wait = seen.unwrap_or_else(|| panic!("no error: {:?}", up.shown()));
    tokio::time::advance(wait).await;
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x09, 9);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, 0));
    assert_eq!(wait, 2 * MINUTE, "an error ended the whole waits");
}

/// T5, S2: a mismatch at the check of a whole cut, and the idle steps
/// after it, between two whole cuts lost, do not end the waits.
#[tokio::test(start_paused = true)]
async fn a_mismatch_between_two_whole_cuts_lost_does_not_end_the_waits() {
    let mut fx = Fixture::new(212).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    const OTHER: [u8; 16] = *b"another state...";
    // Another writer's row: the next (delta) cut is a mismatch, and the
    // cut after it is whole.
    fx.put_row_on_head(OTHER, 1, 7).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 20);
    }
    until_idle(&mut up, 1000).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    // During the wait the row names a commit that is not head's: the
    // heir, whole, is a mismatch at its check.
    let elsewhere = CommittedCut {
        state: OTHER,
        number: 1,
        group: 8,
    };
    fx.put_row(row::encode(&elsewhere, &[0xab; 32])).await;
    let before = up.driver().expect("the driver").figures().clone();
    assert_eq!(a_whole_cut_lost(&mut up).await.0, MINUTE);
    // Every held cut abandoned, the uploader idles until the next.
    until_idle(&mut up, 100).await;
    let after = up.driver().expect("the driver").figures().clone();
    assert_eq!(after.mismatches, before.mismatches + 1, "the heir's check");
    // The row as head's again: the next whole cut goes on, and is lost.
    fx.put_row_on_head(OTHER, 1, 7).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 32, 0x32);
        fx.group(store, 23);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x41, 24);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, before.uploaded));
    assert_eq!(wait, 2 * MINUTE, "a mismatch ended the whole waits");
}

/// T5, S3: a commit refused, read again and given up, between two whole
/// cuts lost, does not end the waits.
#[tokio::test(start_paused = true)]
async fn a_give_up_between_two_whole_cuts_lost_does_not_end_the_waits() {
    let mut fx = Fixture::new(213).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let clock = Clock::new();
    let way = Leasing::apart(&fx);
    let mode = Arc::new(AtomicU8::new(PASS));
    let make = Refusals::make(fenced(&fx, &clock), &mode);
    let mut up = Uploader::with_slices(store, way.keeper(&fx), make, 8);
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x08, 8);
    assert_eq!(a_whole_cut_lost(&mut up).await.0, MINUTE);
    until_doing(&mut up, Doing::Committing, 100).await;
    mode.store(REFUSE, Ordering::SeqCst);
    assert_eq!(give_ups(&mut up, 1).await, [SECOND]);
    mode.store(PASS, Ordering::SeqCst);
    // The heir is checked anew and read again: lost this time.
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x09, 9);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, 0));
    assert_eq!(wait, 2 * MINUTE, "a give-up ended the whole waits");
}

/// T5, S4: a publish made during a whole wait, as in
/// [`a_publish_is_made_during_a_whole_wait`], does not end the waits: the
/// next whole cut lost waits two minutes. An agent that publishes on a
/// schedule would otherwise hold the waits at a minute.
#[tokio::test(start_paused = true)]
async fn a_publish_between_two_whole_cuts_lost_does_not_end_the_waits() {
    let mut fx = Fixture::new(214).await;
    let mut store = fx.start();
    for block in 0..OVER {
        fx.write(&mut store, 100 + block, 0x07);
    }
    fx.group(&mut store, 7);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let make: Box<dyn FnMut(u64) -> Fenced> = Box::new(fenced(&fx, &clock));
    let mut up: Turned = Uploader::with_slices(store, keeper(&fx, &clock), make, 8);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    until_idle(&mut up, 1000).await;
    fx.assert_uploaded(7, "cut 1, whole").await;
    fx.put_row_on_head(*b"another state...", 1, 7).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 30, 0x30);
        fx.group(store, 20);
    }
    until_idle(&mut up, 1000).await;
    {
        let store = up.driver().expect("the driver").local();
        fx.write(store, 31, 0x31);
        fx.group(store, 21);
    }
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x40, 22);
    let mut sleep = until_state(&mut up, &clock, 20, |s| {
        matches!(s, State::WholeLost { .. })
    })
    .await;
    let (reply, answer) = tokio::sync::oneshot::channel();
    if up.control(Control::Ask {
        request: asked(state, "p", 0, 7),
        rewound: 0,
        reply,
    }) {
        sleep = Duration::ZERO;
    }
    assert_eq!(answer.await.expect("an answer"), Answer::Accepted);
    let mut turns = 0;
    while matches!(up.shown().state, State::WholeLost { .. }) {
        both_pass(&clock, sleep).await;
        sleep = up.turn().await;
        turns += 1;
        assert!(turns < 1000, "the wait does not end");
    }
    assert!(
        matches!(up.publishing().progress("p"), Progress::Done(_)),
        "{:?}",
        up.publishing().progress("p")
    );
    let uploaded = up.driver().expect("the driver").figures().uploaded;
    until_reading(&mut up, 100).await;
    over_the_bound(&mut fx, &mut up, 0x41, 23);
    let (wait, _, _) = a_whole_cut_lost(&mut up).await;
    let figures = up.driver().expect("the driver").figures().clone();
    assert_eq!((figures.lost_whole, figures.uploaded), (2, uploaded));
    assert_eq!(wait, 2 * MINUTE, "a publish ended the whole waits");
}

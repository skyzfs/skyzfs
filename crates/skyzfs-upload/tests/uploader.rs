//! The uploader's loop (M2.4c): the lease, the backoff and the driver
//! together, against the in-place store on the simulated disk and the real
//! engine on a store in memory. Time is tokio's, paused: a test moves it.
//! The lease's clock is the refs object's own, moved by the test apart
//! from tokio's.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use skynbd_refs::{ActingRefStore, LeaseOutcome, ObjectRefStore, RefStore, RefValue};
use skynbd_store::FaultOp;
use skynbd_types::{CommitHash, VolumeId};
use skyzfs_local::inplace::InPlace;
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::lease::Keeper;
use skyzfs_upload::publish::{Found, Snapshot, EXPIRY_DEFAULT, EXPIRY_MAX};
use skyzfs_upload::remote::{Observed, Remote, RemoteError};
use skyzfs_upload::uploader::{Control, State, Uploader, AT_REST, IDLE};
use skyzfs_upload::{
    Answer, Doing, EngineRemote, LeaseState, Progress, Publishing, Refusal, Request,
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
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<CommitHash, RemoteError> {
        self.inner.commit(group, expected, row).await
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
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<CommitHash, RemoteError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        self.sent
            .lock()
            .expect("sent")
            .push(tokio::time::Instant::now());
        if self.refuse.load(Ordering::SeqCst) {
            return Err(RemoteError::Conflict);
        }
        self.inner.commit(group, expected, row).await
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

    async fn cuts(&mut self) -> skyzfs_upload::CutsNow {
        self.inner.cuts().await
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
/// tokio's time first, the renewal's read can be told that the row is
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
    while !matches!(up.shown().lease, LeaseState::Unsure { .. }) {
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
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<CommitHash, RemoteError> {
        match self.mode.load(Ordering::SeqCst) {
            REFUSE => Err(RemoteError::Conflict),
            LAND_UNSEEN => {
                self.inner.commit(group, expected, row).await?;
                self.stale = self.last.clone();
                Err(RemoteError::Conflict)
            }
            _ => self.inner.commit(group, expected, row).await,
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

/// A remote that notes when each publish is sent. Where `refuse`, the
/// lookup finds no ref and the publish is refused with a conflict, as a
/// refs store that fences writes answers while the volume has no lease
/// row; otherwise both go to the engine.
struct Publishes<R> {
    inner: R,
    refuse: bool,
    sent: Arc<Mutex<Vec<tokio::time::Instant>>>,
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
        self.inner.put_sealed().await
    }

    async fn commit(
        &mut self,
        group: u64,
        expected: Option<Vec<u8>>,
        row: Vec<u8>,
    ) -> Result<CommitHash, RemoteError> {
        self.inner.commit(group, expected, row).await
    }

    async fn snapshot(&mut self, name: &str) -> Result<Option<Found>, RemoteError> {
        if self.refuse {
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
            .push(tokio::time::Instant::now());
        if self.refuse {
            return Err(RemoteError::Conflict);
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
            refuse,
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
    let sent: Vec<Duration> = sent
        .lock()
        .expect("the times")
        .iter()
        .map(|at| at.duration_since(began))
        .collect();
    println!("publishes sent at {sent:?}; {progress:?}");
    (sent, progress)
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
    // Looked up, absent: both wait for a cut after cut 1.
    until_idle(&mut up, 100).await;
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

/// `run_with` answers the channel's requests between its turns: the
/// count of cuts as the store has it, a publish accepted, its progress
/// until it is done, on cut 1, whose commit is head. Each post of the
/// status after a publish ended names it once, as it ended.
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
        let now = ask_of(&inbox, Control::Cuts).await;
        assert_eq!((now.state, now.count), (Some(state), 1), "{now:?}");
        let newest = now.newest.expect("cut 1");
        assert_eq!((newest.number, newest.group), (1, 7));
        let answer = ask_of(&inbox, |r| Control::Ask(asked(state, "t", 0, 7), r)).await;
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
    let mut fx = Fixture::new(82).await;
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
        let answer = ask_of(&inbox, |r| Control::Ask(asked(state, "t", 0, 7), r)).await;
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
    let mut fx = Fixture::new(83).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), Some(state));
    let (reply, answer) = tokio::sync::oneshot::channel();
    drop(answer);
    assert!(
        !up.control(Control::Ask(asked(state, "t", 0, 7), reply))
            .await
    );
    assert_eq!(up.publishing().progress("t"), Progress::Unknown);
    assert_eq!(up.publishing().figures().accepted, 0);

    assert_eq!(
        up.publishing().ask(asked(state, "u", 0, 100)),
        Answer::Accepted
    );
    let (reply, answer) = tokio::sync::oneshot::channel();
    drop(answer);
    assert!(!up.control(Control::Withdraw("u".into(), reply)).await);
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
        let answer = ask_of(&inbox, |r| Control::Ask(request, r)).await;
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

/// A request is judged against the store's cut state as it is when it
/// is asked: one whose state was read after the store's first cut is
/// taken before any turn has checked that cut, and one of another state
/// is refused all the same.
///
/// Mutant: the state left to the driver's check (the first is refused
/// `State`).
#[tokio::test(start_paused = true)]
async fn a_request_is_judged_against_the_stores_cut_state_when_it_is_asked() {
    let mut fx = Fixture::new(86).await;
    let store = one_cut(&mut fx);
    let state = store.cut_state_id().expect("the cut state");
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    *up.publishing() = Publishing::new(Some(ATTACHMENT), None);
    let (reply, answer) = tokio::sync::oneshot::channel();
    assert!(
        up.control(Control::Ask(asked(state, "t", 0, 1), reply))
            .await
    );
    assert_eq!(answer.await.ok(), Some(Answer::Accepted));
    let mut other = state;
    other[0] ^= 1;
    let (reply, answer) = tokio::sync::oneshot::channel();
    assert!(
        !up.control(Control::Ask(asked(other, "u", 0, 1), reply))
            .await
    );
    assert_eq!(answer.await.ok(), Some(Answer::Refused(Refusal::State)));
    assert_eq!(up.publishing().progress("t"), Progress::Pending);
}

/// The count of cuts a request reads is the store's when it is asked: a
/// cut counted after the uploader's last turn is in it.
#[tokio::test(start_paused = true)]
async fn the_count_of_cuts_is_the_stores_when_it_is_asked() {
    let mut fx = Fixture::new(85).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);
    until_idle(&mut up, 100).await;
    let local = up.driver().expect("the driver").local();
    fx.write(local, 12, 0x12);
    fx.group(local, 8);
    let (reply, answer) = tokio::sync::oneshot::channel();
    assert!(!up.control(Control::Cuts(reply)).await);
    let now = answer.await.expect("the count");
    assert_eq!(now.count, 2);
    assert_eq!(now.newest.map(|cut| cut.group), Some(8));
}

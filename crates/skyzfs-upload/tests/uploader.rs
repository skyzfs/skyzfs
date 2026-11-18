//! The uploader's loop (M2.4c): the lease, the backoff and the driver
//! together, against the in-place store on the simulated disk and the real
//! engine on a store in memory. Time is tokio's, paused: a test moves it.
//! The lease's clock is the refs object's own, moved by the test apart
//! from tokio's.

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use skynbd_refs::{ActingRefStore, LeaseOutcome, ObjectRefStore, RefStore};
use skynbd_store::FaultOp;
use skyzfs_local::inplace::InPlace;
use skyzfs_local::sim::SimDisk;
use skyzfs_upload::lease::Keeper;
use skyzfs_upload::remote::{Observed, Remote, RemoteError};
use skyzfs_upload::uploader::{State, Uploader, AT_REST, IDLE};
use skyzfs_upload::{Doing, EngineRemote, LeaseState};

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
    let mut up = Uploader::with_slices(store, keeper(&fx, &clock), fenced(&fx, &clock), 16);

    let lost = State::LeaseLost {
        holder: "node-b".into(),
        epoch: 1,
    };
    assert_eq!(up.turn().await, AT_REST);
    assert_eq!(up.shown().state, lost);
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

/// A remote whose commits are refused with a conflict while `refuse` is
/// set, landing nothing: what a fencing store with no lease row did in
/// the review's S9.
struct Refusing<R> {
    inner: R,
    refuse: Arc<AtomicBool>,
    commits: Arc<AtomicU64>,
}

impl<R: Remote> Remote for Refusing<R> {
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
    ) -> Result<(), RemoteError> {
        self.commits.fetch_add(1, Ordering::SeqCst);
        if self.refuse.load(Ordering::SeqCst) {
            return Err(RemoteError::Conflict);
        }
        self.inner.commit(group, expected, row).await
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
    let make = {
        let (mut fenced, refuse, commits) = (fenced(&fx, &clock), refuse.clone(), commits.clone());
        move |epoch| Refusing {
            inner: fenced(epoch),
            refuse: refuse.clone(),
            commits: commits.clone(),
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
                waits.push((kind, w));
            }
        }
        tokio::time::advance(wait.max(Duration::from_millis(1))).await;
    }
    let secs: Vec<u64> = waits.iter().map(|(_, w)| w.as_secs()).collect();
    let kinds: Vec<&str> = waits.iter().map(|(kind, _)| *kind).collect();
    // A commit refused (an error), then the same commit given up after
    // the row and head were read again, each doubling the wait.
    assert_eq!(&secs[..8], [1, 2, 4, 8, 16, 32, 64, 128], "{waits:?}");
    assert_eq!(
        &kinds[..4],
        ["refused", "given up", "refused", "given up"],
        "{waits:?}"
    );
    let tries = commits.load(Ordering::SeqCst);
    assert!(
        tries <= 12,
        "{tries} commits in ten minutes: the whole cut went up at every cycle"
    );
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

#[tokio::test(start_paused = true)]
async fn after_a_step_that_outlasts_the_renewal_the_lease_is_kept_at_once_and_the_backoff_counts_from_the_failure(
) {
    let mut fx = Fixture::new(48).await;
    let store = one_cut(&mut fx);
    let clock = Clock::new();
    let delay = Arc::new(AtomicU64::new(0));
    let slow = Slow {
        inner: store,
        delay: Arc::clone(&delay),
    };
    // The uploads' way to object storage fails; the lease's, a way of
    // its own to the same objects, does not.
    let apart = clock.refs_on(skynbd_store::FaultStore::new(fx.store.inner().clone()));
    let lease = Keeper::new(apart, fx.vol, HOLDER, TTL);
    fx.store.arm(FaultOp::Any, "", 0).await;
    let mut up = Uploader::with_slices(slow, lease, fenced(&fx, &clock), 16);

    let wait = up.turn().await;
    assert_eq!(wait, Duration::from_secs(1), "the first failure's backoff");
    assert_eq!(expires(&up), 1_000_000 + TTL, "the lease taken");
    tokio::time::advance(wait).await;

    // The next check takes twenty seconds, past the renewal due ten
    // seconds after the take, and fails.
    delay.store(20_000, Ordering::SeqCst);
    clock.pass(21_000);
    let wait = up.turn().await;
    assert!(
        matches!(&up.shown().state, State::Backoff { why, wait }
            if why.contains("injected") && *wait == Duration::from_secs(2)),
        "{:?}",
        up.shown().state
    );
    assert_eq!(wait, Duration::ZERO, "the renewal is overdue: at once");

    // The renewal comes first; the step waits out the backoff counted
    // from the failure, not from the turn's start.
    delay.store(0, Ordering::SeqCst);
    fx.store.disarm().await;
    let wait = up.turn().await;
    assert_eq!(expires(&up), 1_000_000 + 21_000 + TTL, "the lease kept");
    assert!(
        matches!(up.shown().state, State::Backoff { .. }),
        "no step before the backoff is over: {:?}",
        up.shown().state
    );
    assert_eq!(
        wait,
        Duration::from_secs(2),
        "the backoff, from the failure"
    );
    tokio::time::advance(wait).await;

    until_idle(&mut up, 100).await;
    fx.assert_uploaded(7, "after the backoff").await;
}

#[tokio::test(start_paused = true)]
async fn after_an_idle_check_that_outlasts_the_renewal_the_lease_is_kept_at_once() {
    let fx = Fixture::new(49).await;
    let clock = Clock::new();
    let delay = Arc::new(AtomicU64::new(0));
    let slow = Slow {
        inner: fx.start(),
        delay: Arc::clone(&delay),
    };
    let mut up = Uploader::with_slices(slow, keeper(&fx, &clock), fenced(&fx, &clock), 16);

    let wait = up.turn().await;
    assert_eq!((up.shown().state, wait), (State::Idle, IDLE));
    tokio::time::advance(wait).await;

    // A check that finds nothing, after twenty seconds: the renewal,
    // due ten seconds after the take, is overdue.
    delay.store(20_000, Ordering::SeqCst);
    clock.pass(20_250);
    let wait = up.turn().await;
    assert_eq!((up.shown().state, wait), (State::Idle, Duration::ZERO));

    delay.store(0, Ordering::SeqCst);
    let wait = up.turn().await;
    assert_eq!(expires(&up), 1_000_000 + 20_250 + TTL, "the lease kept");
    assert_eq!((up.shown().state, wait), (State::Idle, IDLE));
}

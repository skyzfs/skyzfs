//! The uploader's loop (M2.4c): the driver, the volume's lease and the
//! backoff, turned by the daemon's uploader thread.
//!
//! Each [`Uploader::turn`] does one bounded piece of work and says how
//! long to wait before the next:
//!
//! 1. **The lease**, when its time comes ([`crate::lease`]). No lease, no
//!    upload: until the first take succeeds the driver is not even made,
//!    and the store abandons its cuts at the bound as it does with no
//!    uploader. A first take that fails is tried again after the backoff.
//!    A renewal comes every third of the lease's time; one that fails on
//!    the store leaves the uploads going under the epoch last held. A
//!    lease lost stops the uploads for the rest of the instance, and not
//!    the serving. A renewal that falls due while a step is under way is
//!    made beside the step, which goes on meanwhile (F4 of #117's
//!    review): a step longer than two thirds of the lease, a slow PUT
//!    or a slow turn of the queue, cannot let the row lapse. Neither the
//!    step nor the renewal is dropped half done.
//! 2. **One step of the driver** ([`Driver::step`]), made at the first
//!    take with a remote fenced by its epoch: a renewal keeps the epoch or
//!    loses the lease, so the driver is never made again. Two waits, each
//!    one second doubling to five minutes ([`crate::backoff`]); the lead's
//!    decision on F3 of #117's review:
//!    - **A step that fails on object storage** (a PUT, a read) waits,
//!      and the driver makes the same step again after it: nothing of the
//!      cut is lost. That wait is reset by any step that succeeds, so a
//!      store that fails now and then costs a second for each failure.
//!    - **A commit refused** (a conflict) is followed at once by a step
//!      that reads the row and head again, and that step gives the cut up
//!      (or finds that it landed). The wait comes at the give-up, once a
//!      cycle, and is reset only when a cut is committed or found
//!      committed: a store that refuses every commit does not have the
//!      whole cut uploaded again at every cycle (G7 of #114's review).
//!
//!    A commit refused by the fence (`RemoteError::Fenced`) loses the
//!    lease. A cut the store kept at its release (`Step::Kept`) stops the
//!    driver for good.
//! 3. With no cut to upload, the next turn comes after [`IDLE`].
//!
//! [`Uploader::run`] turns it until it is told to stop, and takes it by
//! value: a turn dropped at the stop can leave the driver half way
//! through a step (`Driver::step` is not made to be dropped at an
//! await), so nothing may turn it again (F2 of #117's review). It gives
//! back the lease's keeper, for the caller to let go.

use std::time::Duration;

use tokio::time::Instant;

use skynbd_refs::RefStore;

use crate::backoff::Backoff;
use crate::driver::{Doing, Driver, Figures, Step};
use crate::lease::{Keeper, LeaseState};
use crate::local::Local;
use crate::remote::{Remote, RemoteError};

/// How long an idle uploader waits before it asks the store again for a
/// cut: a round trip to the queue's thread four times a second.
pub const IDLE: Duration = Duration::from_millis(250);

/// The wait of an uploader that will do nothing more: its lease is lost
/// or its driver stopped. It still turns, for the status.
pub const AT_REST: Duration = Duration::from_secs(60);

/// What the uploader is doing, for the status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// No lease is held yet, and why. Nothing is uploaded.
    NoLease(String),
    /// The lease is lost for good: the row names `holder` at `epoch`.
    /// Nothing more is uploaded by this instance.
    LeaseLost {
        /// The holder.
        holder: String,
        /// Its epoch.
        epoch: u64,
    },
    /// No cut to upload.
    Idle,
    /// The driver at work, and the cut under way.
    Working(Doing, Option<u64>),
    /// Waiting after a step that failed or a commit given up: why, and
    /// how long the wait was when it began.
    Backoff {
        /// The error, or the commit given up.
        why: String,
        /// The wait.
        wait: Duration,
    },
    /// The store kept a committed cut ([`Step::Kept`]): nothing more is
    /// uploaded by this instance.
    Stopped,
}

/// The uploader as the status shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shown {
    /// What it is doing.
    pub state: State,
    /// The lease.
    pub lease: LeaseState,
    /// The driver's figures: zero until the driver is made.
    pub figures: Figures,
}

/// The driver, the lease and the backoff. `M` makes the remote, fenced by
/// the epoch it is given, at the first take of the lease.
pub struct Uploader<L, R, F, M> {
    lease: Keeper<F>,
    make: M,
    local: Option<L>,
    driver: Option<Driver<L, R>>,
    slice_blocks: usize,
    /// After a step that failed on object storage.
    errors: Backoff,
    /// After a commit given up.
    refused: Backoff,
    /// A commit was refused, and the step after it, which reads the row
    /// and head again, has not ended yet.
    rereading: bool,
    /// For the first take of the lease.
    lease_backoff: Backoff,
    state: State,
    /// When the lease is next kept.
    lease_at: Instant,
    /// When the driver may next step.
    step_at: Instant,
}

impl<L, R, F, M> Uploader<L, R, F, M>
where
    L: Local,
    R: Remote,
    F: RefStore,
    M: FnMut(u64) -> R,
{
    /// An uploader of the store `local` under `lease`, with slices of
    /// [`crate::SLICE_BLOCKS`].
    pub fn new(local: L, lease: Keeper<F>, make: M) -> Self {
        Self::with_slices(local, lease, make, crate::SLICE_BLOCKS)
    }

    /// As [`Uploader::new`], with slices of `blocks` blocks.
    pub fn with_slices(local: L, lease: Keeper<F>, make: M, blocks: usize) -> Self {
        let now = Instant::now();
        Self {
            lease,
            make,
            local: Some(local),
            driver: None,
            slice_blocks: blocks,
            errors: Backoff::new(),
            refused: Backoff::new(),
            rereading: false,
            lease_backoff: Backoff::new(),
            state: State::NoLease("not tried yet".into()),
            lease_at: now,
            step_at: now,
        }
    }

    /// The uploader as the status shows it.
    pub fn shown(&self) -> Shown {
        Shown {
            state: self.state.clone(),
            lease: self.lease.state().clone(),
            figures: self
                .driver
                .as_ref()
                .map(|d| d.figures().clone())
                .unwrap_or_default(),
        }
    }

    /// The lease.
    pub fn lease(&mut self) -> &mut Keeper<F> {
        &mut self.lease
    }

    /// The driver, once made.
    pub fn driver(&mut self) -> Option<&mut Driver<L, R>> {
        self.driver.as_mut()
    }

    /// One bounded piece of work; how long to wait before the next turn.
    ///
    /// The lease is kept first when its renewal is due, and beside the
    /// step when it falls due during the step. The clock is read again
    /// after each call that waits on the store or object storage: a wait
    /// is counted from when that call came back, so a backoff starts at
    /// the failure and a slow take's wait at the take's end.
    pub async fn turn(&mut self) -> Duration {
        let mut now = Instant::now();
        if now >= self.lease_at {
            self.lease_at = Self::keep_lease(&mut self.lease, &mut self.lease_backoff, now).await;
            now = Instant::now();
        }
        let epoch = match self.lease.state() {
            LeaseState::Lost { holder, epoch } => {
                self.state = State::LeaseLost {
                    holder: holder.clone(),
                    epoch: *epoch,
                };
                return AT_REST;
            }
            LeaseState::None(why) => {
                self.state = State::NoLease(why.clone());
                return self.lease_at.saturating_duration_since(now);
            }
            LeaseState::Held { epoch, .. } | LeaseState::Unsure { epoch, .. } => *epoch,
        };
        if self.driver.is_none() {
            if let Some(local) = self.local.take() {
                let remote = (self.make)(epoch);
                self.driver = Some(Driver::with_slices(local, remote, self.slice_blocks));
            }
        }
        let Some(driver) = self.driver.as_mut() else {
            return AT_REST;
        };
        if driver.doing() == Doing::Stopped {
            self.state = State::Stopped;
            return AT_REST;
        }
        if now < self.step_at {
            return self.next_wait(now, self.step_at);
        }
        let (step, ended) = Self::step_keeping(
            driver,
            &mut self.lease,
            &mut self.lease_backoff,
            &mut self.lease_at,
        )
        .await;
        let now = Instant::now();
        let (doing, under_way) = (driver.doing(), driver.under_way());
        if step.is_ok() {
            self.errors.succeeded();
        }
        let rereading = std::mem::take(&mut self.rereading);
        match step {
            Ok(Step::Idle) => {
                self.state = State::Idle;
                self.next_wait(now, ended + IDLE)
            }
            Ok(Step::Kept { .. }) => {
                self.state = State::Stopped;
                AT_REST
            }
            Ok(Step::GaveUp { number }) => {
                let wait = self.refused.failed();
                self.wait_after(
                    wait,
                    ended,
                    now,
                    format!("commit of cut {number} refused: conflict"),
                )
            }
            Ok(step) => {
                // Only a cut committed, or found committed, ends the wait
                // of a refusal: a read that goes on after a refused commit
                // is not a success, or the whole cut would go up again at
                // every cycle.
                if matches!(
                    step,
                    Step::Uploaded { .. }
                        | Step::UploadedNotHeld { .. }
                        | Step::FoundCommitted { .. }
                ) {
                    self.refused.succeeded();
                }
                self.state = State::Working(doing, under_way);
                Duration::ZERO
            }
            Err(RemoteError::Fenced { holder, epoch }) => {
                self.lease.fenced(holder.clone(), epoch);
                self.state = State::LeaseLost { holder, epoch };
                AT_REST
            }
            // A commit refused: the next step reads the row and head
            // again at once, and the wait comes at its give-up. Only at
            // the commit, where the driver stays to read them, and only
            // once in a row: a conflict anywhere else, or from that read
            // (which does not return one), is an error like any other, so
            // that no conflict makes the uploader turn twice without a
            // wait.
            Err(RemoteError::Conflict) if doing == Doing::Committing && !rereading => {
                self.rereading = true;
                self.state = State::Working(doing, under_way);
                Duration::ZERO
            }
            Err(e) => {
                let wait = self.errors.failed();
                self.wait_after(wait, ended, now, e.to_string())
            }
        }
    }

    /// One step of the driver, and when it came back, with the lease
    /// kept beside it each time its renewal falls due while the step goes
    /// on. The step is polled throughout, and neither it nor a renewal is
    /// dropped half done: a step that ends during a renewal waits for the
    /// renewal to end, and its wait is still counted from its own end.
    ///
    /// A renewal that loses the lease does not stop the step: the step
    /// cannot be dropped, and a commit it makes is refused by the fence,
    /// which is what decides. The next turn finds the lease lost.
    async fn step_keeping(
        driver: &mut Driver<L, R>,
        lease: &mut Keeper<F>,
        lease_backoff: &mut Backoff,
        lease_at: &mut Instant,
    ) -> (Result<Step, RemoteError>, Instant) {
        let mut step = std::pin::pin!(driver.step());
        loop {
            tokio::select! {
                biased;
                step = &mut step => return (step, Instant::now()),
                () = tokio::time::sleep_until(*lease_at) => {}
            }
            let mut keep = std::pin::pin!(Self::keep_lease(lease, lease_backoff, Instant::now()));
            tokio::select! {
                biased;
                at = &mut keep => *lease_at = at,
                step = &mut step => {
                    let ended = Instant::now();
                    *lease_at = keep.await;
                    return (step, ended);
                }
            }
        }
    }

    /// Take or renew the lease; when to keep it next. `before` is when
    /// the take was sent. The next renewal is counted from then: the
    /// store set the row's expiry at some time after it, so the renewal
    /// comes a third of the lease after that at the latest. A failed
    /// take's backoff and the rest after a lost lease are counted from
    /// when the take came back.
    async fn keep_lease(
        lease: &mut Keeper<F>,
        lease_backoff: &mut Backoff,
        before: Instant,
    ) -> Instant {
        let renew = Duration::from_millis((lease.ttl_ms() / 3).max(1));
        let kept = lease.keep().await;
        let after = Instant::now();
        match kept {
            LeaseState::None(_) => after + lease_backoff.failed(),
            LeaseState::Held { .. } | LeaseState::Unsure { .. } => {
                lease_backoff.succeeded();
                before + renew
            }
            LeaseState::Lost { .. } => after + AT_REST,
        }
    }

    /// A step failed or a commit was given up, at `ended`: wait `wait`,
    /// from then. `now` is when the turn ends.
    fn wait_after(
        &mut self,
        wait: Duration,
        ended: Instant,
        now: Instant,
        why: String,
    ) -> Duration {
        self.step_at = ended + wait;
        self.state = State::Backoff { why, wait };
        self.next_wait(now, self.step_at)
    }

    /// The wait until `until`, or until the lease is next kept if that
    /// comes first.
    fn next_wait(&self, now: Instant, until: Instant) -> Duration {
        until.min(self.lease_at).saturating_duration_since(now)
    }

    /// Turn until `stop` resolves, giving `publish` what the status shows
    /// after each turn and sleeping between turns as each turn asks; then
    /// the lease's keeper, for the caller to let go or keep. A turn under
    /// way at the stop is dropped, and the uploader with it: nothing can
    /// turn a driver left half way through a step.
    pub async fn run(
        mut self,
        stop: impl std::future::Future<Output = ()>,
        mut publish: impl FnMut(Shown),
    ) -> Keeper<F> {
        let mut stop = std::pin::pin!(stop);
        loop {
            // The stop first: once it has come, no other turn begins.
            let wait = tokio::select! {
                biased;
                () = &mut stop => break,
                wait = self.turn() => wait,
            };
            publish(self.shown());
            tokio::select! {
                biased;
                () = &mut stop => break,
                () = tokio::time::sleep(wait) => {}
            }
        }
        self.lease
    }
}

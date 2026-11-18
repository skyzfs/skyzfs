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
//!    the serving.
//! 2. **One step of the driver** ([`Driver::step`]), made at the first
//!    take with a remote fenced by its epoch: a renewal keeps the epoch or
//!    loses the lease, so the driver is never made again. A step that
//!    fails on object storage, and a commit given up (a conflict, or a row
//!    or head that moved), put the uploader in the backoff
//!    ([`crate::backoff`]): one second, doubling to five minutes, reset
//!    when a cut is committed or found committed. A step that only goes on
//!    (a check, a slice read) does not reset it: the reading of "reset on
//!    a success" that keeps the rule's point. So a store that refuses every
//!    commit does not have the whole cut uploaded again at every cycle
//!    (the lead's decision on G7 of #114's review). A commit refused by
//!    the fence (`RemoteError::Fenced`) loses the lease. A cut the store
//!    kept at its release (`Step::Kept`) stops the driver for good.
//! 3. With no cut to upload, the next turn comes after [`IDLE`].

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
    /// For the driver's steps.
    backoff: Backoff,
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
            backoff: Backoff::new(),
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
    pub async fn turn(&mut self) -> Duration {
        let now = Instant::now();
        if now >= self.lease_at {
            self.keep_lease(now).await;
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
        let step = driver.step().await;
        let (doing, under_way) = (driver.doing(), driver.under_way());
        match step {
            Ok(Step::Idle) => {
                self.state = State::Idle;
                self.next_wait(now, now + IDLE)
            }
            Ok(Step::Kept { .. }) => {
                self.state = State::Stopped;
                AT_REST
            }
            Ok(Step::GaveUp { number }) => {
                self.wait_after(now, format!("commit of cut {number} refused: conflict"))
            }
            Ok(step) => {
                // Only a cut committed, or found committed, ends the wait:
                // a read that goes on after a refused commit is not a
                // success, or the whole cut would go up again at every
                // cycle.
                if matches!(
                    step,
                    Step::Uploaded { .. }
                        | Step::UploadedNotHeld { .. }
                        | Step::FoundCommitted { .. }
                ) {
                    self.backoff.succeeded();
                }
                self.state = State::Working(doing, under_way);
                Duration::ZERO
            }
            Err(RemoteError::Fenced { holder, epoch }) => {
                self.lease.fenced(holder.clone(), epoch);
                self.state = State::LeaseLost { holder, epoch };
                AT_REST
            }
            Err(e) => self.wait_after(now, e.to_string()),
        }
    }

    /// Take or renew the lease, and say when to keep it next.
    async fn keep_lease(&mut self, now: Instant) {
        let renew = Duration::from_millis((self.lease.ttl_ms() / 3).max(1));
        self.lease_at = match self.lease.keep().await {
            LeaseState::None(_) => now + self.lease_backoff.failed(),
            LeaseState::Held { .. } | LeaseState::Unsure { .. } => {
                self.lease_backoff.succeeded();
                now + renew
            }
            LeaseState::Lost { .. } => now + AT_REST,
        };
    }

    /// A step failed or a commit was given up: wait.
    fn wait_after(&mut self, now: Instant, why: String) -> Duration {
        let wait = self.backoff.failed();
        self.step_at = now + wait;
        self.state = State::Backoff { why, wait };
        self.next_wait(now, self.step_at)
    }

    /// The wait until `until`, or until the lease is next kept if that
    /// comes first.
    fn next_wait(&self, now: Instant, until: Instant) -> Duration {
        until.min(self.lease_at).saturating_duration_since(now)
    }

    /// Turn until `stop` resolves, giving `publish` what the status shows
    /// after each turn.
    pub async fn run(
        &mut self,
        stop: impl std::future::Future<Output = ()>,
        mut publish: impl FnMut(Shown),
    ) {
        let stop = std::pin::pin!(stop);
        let mut stop = stop;
        loop {
            let wait = tokio::select! {
                _ = &mut stop => return,
                wait = self.turn() => wait,
            };
            publish(self.shown());
            tokio::select! {
                _ = &mut stop => return,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }
}

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
//!
//!    **A publish** (M2.5) takes its step ([`Driver::step_publish`])
//!    when it has one and the cut's step is not due. While both are due
//!    they share the time: the side that has had less of it since both
//!    became due takes the next step, the publishes on a tie when the
//!    cut stepped last, and the count starts again from nothing whenever
//!    either side has nothing to do (an idle cut's step is that). So
//!    while both have work, each has half the time, give or take one step
//!    of the other's: publishes whose steps fail slowly, however many,
//!    slow the cut to half its pace and no more, and slow steps of the
//!    cut slow the publishes the same way. Neither paces the other a step
//!    at a time (F1 and F4 of the review of #126). A publish has a wait
//!    of its own (F3 of #122's review): a publish whose step fails takes
//!    no step for one second, doubling to five minutes at each failure
//!    of it
//!    ([`crate::publish::Publishing::wait_after`]). The wait is that
//!    publish's alone (D2 of the review of #122): the other publishes
//!    step meanwhile, oldest first, and a publish that ends, however it
//!    ends, takes its wait with it. Its lookup does not reset it, since
//!    the lookup can succeed while the transaction cannot be made. The
//!    cut's steps go on meanwhile, and their wait is not touched by a
//!    publish's: publishes that cannot be made hold up no other publish,
//!    and slow the uploads to half their pace at most. A publish refused
//!    by the fence loses the lease, as a commit does.
//! 3. With no cut to upload, the next turn comes after [`IDLE`].
//!
//! An uploader that will upload nothing more (its lease lost, or its
//! driver stopped) publishes nothing more either: its publishes pending
//! end refused, and so does every request after
//! ([`crate::publish::Publishing::stop`], F4 of #122's review). Each
//! turn first ends the publishes whose time is up
//! ([`crate::publish::Publishing::expire`]), whatever the turn does
//! next: before the lease is held, and while the uploader is idle.
//!
//! **The daemon's channel** (M2.5-2) reaches the uploader through
//! [`Uploader::run_with`]: a request ([`Control`]) is answered between
//! two turns, never in the middle of one, so it waits at most for the
//! step under way. The answer reads the publishes as they stand when it
//! is given: those whose time is up are ended first. A publish accepted
//! ends the sleep, and the next turn begins at once, so that a publish
//! asked of an idle uploader is looked up then and not after the sleep.
//! A request whose asker has stopped waiting is not taken: the asker was
//! told that no answer came, and would not hear of it. A withdrawal is
//! made all the same, since it ends a publish and makes none.
//!
//! An answer calls neither the store nor object storage
//! ([`Uploader::control`] does not wait), so a request between two turns
//! holds back neither the lease's renewal nor the next step (F1 of the
//! review of #127): the lease is kept as #117's F4 has it. A publish is
//! judged against the cut state given at the start
//! ([`Publishing::set_state`]), which is the store's: a store's state
//! changes only at a start. The count of cuts is not the uploader's to
//! answer: the channel asks it of the store at the desk
//! (`queued::Desk::cuts_now`), beside the uploader's calls.
//!
//! [`Uploader::run`] turns it until it is told to stop, and takes it by
//! value: a turn dropped at the stop can leave the driver half way
//! through a step (`Driver::step` is not made to be dropped at an
//! await), so nothing may turn it again (F2 of #117's review). It gives
//! back the lease's keeper, for the caller to let go.

use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;

use skynbd_refs::RefStore;

use crate::backoff::Backoff;
use crate::driver::{Doing, Driver, Figures, Step};
use crate::lease::{Keeper, LeaseState};
use crate::local::Local;
use crate::publish::{Answer, Progress, PublishFigures, Publishing, Request};
use crate::remote::{Remote, RemoteError};

/// How long an idle uploader waits before it asks the store again for a
/// cut: a round trip to the queue's thread four times a second.
pub const IDLE: Duration = Duration::from_millis(250);

/// The wait of an uploader that will do nothing more: its lease is lost
/// or its driver stopped. It still turns, for the status.
pub const AT_REST: Duration = Duration::from_secs(60);

/// Whose step comes next, a publish's or the cut's ([module docs](self)):
/// while both are due, the side that has had less time since both became
/// due; on a tie, the side that did not step last.
#[derive(Debug, Clone, Copy, Default)]
struct Turns {
    /// The time the publishes' steps have taken since both became due.
    publish: Duration,
    /// The time the cut's steps have taken since both became due.
    cut: Duration,
    /// The last step chosen was a publish's.
    published_last: bool,
    /// Both were due when the last step was chosen, so its time counts.
    shared: bool,
}

impl Turns {
    /// Whether the next step is a publish's, given which are due: `None`
    /// when neither is. While only one is due, the count starts again.
    fn next(&mut self, publish_due: bool, cut_due: bool) -> Option<bool> {
        self.shared = publish_due && cut_due;
        let publish = if self.shared {
            match self.publish.cmp(&self.cut) {
                std::cmp::Ordering::Less => true,
                std::cmp::Ordering::Greater => false,
                std::cmp::Ordering::Equal => !self.published_last,
            }
        } else {
            self.restart();
            if !publish_due && !cut_due {
                return None;
            }
            publish_due
        };
        self.published_last = publish;
        Some(publish)
    }

    /// The step [`Turns::next`] chose took `time`.
    fn took(&mut self, time: Duration) {
        if !self.shared {
            return;
        }
        let side = if self.published_last {
            &mut self.publish
        } else {
            &mut self.cut
        };
        *side = side.saturating_add(time);
    }

    /// One side has nothing to do (an idle cut): the count starts again.
    fn restart(&mut self) {
        self.publish = Duration::ZERO;
        self.cut = Duration::ZERO;
    }
}

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
    /// The publishes (M2.5-2).
    pub publishes: PublishesShown,
}

/// The publishes, as the status shows them (M2.5-2).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PublishesShown {
    /// The publishes pending.
    pub pending: usize,
    /// The oldest pending: its name, and how long it has pended.
    pub oldest: Option<(String, Duration)>,
    /// The figures since the uploader began.
    pub figures: PublishFigures,
    /// The publishes that ended since the last post of
    /// [`Uploader::run_with`], in the order they ended, each with how it
    /// ended ([`Publishing::take_ended`]). Empty in what
    /// [`Uploader::shown`] gives.
    pub ended: Vec<(String, Progress)>,
}

/// A request of the daemon's channel (M2.5-2), answered by
/// [`Uploader::control`] between two turns of [`Uploader::run_with`].
#[derive(Debug)]
pub enum Control {
    /// Ask for a publish ([`Publishing::ask`]).
    Ask(Request, oneshot::Sender<Answer>),
    /// How the publish of a name stands ([`Publishing::progress`]).
    Progress(String, oneshot::Sender<Progress>),
    /// Withdraw the publish of a name ([`Publishing::withdraw`]).
    Withdraw(String, oneshot::Sender<Progress>),
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
    /// When the driver may next take a cut's step.
    step_at: Instant,
    /// Whose step comes next, a publish's or the cut's.
    turns: Turns,
    /// The publishes asked before the driver is made, which it then takes
    /// ([`Uploader::publishing`]).
    publishing: Option<Publishing>,
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
            turns: Turns::default(),
            publishing: Some(Publishing::default()),
        }
    }

    /// The publishes (M2.5): the driver's once it is made, and before
    /// that, those it takes when it is. A publish asked while the lease is
    /// not held yet waits as accepted; one asked of an uploader that will
    /// upload nothing more is refused ([module docs](self)).
    pub fn publishing(&mut self) -> &mut Publishing {
        match self.driver {
            Some(ref mut driver) => driver.publishing(),
            None => self.publishing.get_or_insert_with(Publishing::default),
        }
    }

    /// The uploader as the status shows it.
    pub fn shown(&self) -> Shown {
        let publishes = match (&self.driver, &self.publishing) {
            (Some(driver), _) => Some(driver.publishes()),
            (None, publishing) => publishing.as_ref(),
        };
        let publishes = publishes
            .map(|publishing| {
                let (pending, oldest) = publishing.pending();
                PublishesShown {
                    pending,
                    oldest: oldest.map(|(name, age)| (name.to_string(), age)),
                    figures: publishing.figures().clone(),
                    ended: Vec::new(),
                }
            })
            .unwrap_or_default();
        Shown {
            state: self.state.clone(),
            lease: self.lease.state().clone(),
            figures: self
                .driver
                .as_ref()
                .map(|d| d.figures().clone())
                .unwrap_or_default(),
            publishes,
        }
    }

    /// Answer a request of the daemon's channel ([module docs](self)), and
    /// say whether the next turn should begin at once: a publish was
    /// accepted. It waits on nothing: no call of the store and none of
    /// object storage.
    pub fn control(&mut self, control: Control) -> bool {
        // The publishes as they stand now: those whose time is up end
        // before anything is answered.
        self.publishing().expire(Instant::now());
        match control {
            Control::Ask(request, reply) => {
                if reply.is_closed() {
                    return false;
                }
                let answer = self.publishing().ask(request);
                let accepted = answer == Answer::Accepted;
                let _ = reply.send(answer);
                accepted
            }
            Control::Progress(name, reply) => {
                let _ = reply.send(self.publishing().progress(&name));
                false
            }
            Control::Withdraw(name, reply) => {
                let _ = reply.send(self.publishing().withdraw(&name));
                false
            }
        }
    }

    /// What the status shows, with the publishes ended since the last
    /// post, for `publish`.
    fn post(&mut self, publish: &mut impl FnMut(Shown)) {
        let mut shown = self.shown();
        shown.publishes.ended = self.publishing().take_ended();
        publish(shown);
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
        // Publishes whose time is up end now, whatever the uploader does
        // next, so that `publish?` says so as soon as it is so.
        self.publishing().expire(now);
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
                self.publishing().stop();
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
                let publishing = self.publishing.take().unwrap_or_default();
                self.driver = Some(
                    Driver::with_slices(local, remote, self.slice_blocks)
                        .with_publishing(publishing),
                );
            }
        }
        let Some(driver) = self.driver.as_mut() else {
            return AT_REST;
        };
        if driver.doing() == Doing::Stopped {
            self.state = State::Stopped;
            driver.publishing().stop();
            return AT_REST;
        }
        // A publish and the cut share the time while both are due:
        // several publishes whose steps fail slowly, each with its own
        // wait, would otherwise always have one due (F1 and F4 of the
        // review of #126).
        let Some(publish) = self.turns.next(driver.publish_due(), now >= self.step_at) else {
            return self.next_wait(now, self.step_at);
        };
        let began = now;
        let (step, ended) = Self::step_keeping(
            driver,
            publish,
            &mut self.lease,
            &mut self.lease_backoff,
            &mut self.lease_at,
        )
        .await;
        let now = Instant::now();
        self.turns.took(ended.saturating_duration_since(began));
        if publish {
            return self.after_publish(step, ended);
        }
        let (doing, under_way) = (driver.doing(), driver.under_way());
        if step.is_ok() {
            self.errors.succeeded();
        }
        let rereading = std::mem::take(&mut self.rereading);
        match step {
            Ok(Step::Idle) => {
                self.turns.restart();
                self.state = State::Idle;
                self.next_wait(now, ended + IDLE)
            }
            Ok(Step::Kept { .. }) => {
                self.state = State::Stopped;
                driver.publishing().stop();
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
            Err(RemoteError::Fenced { holder, epoch }) => self.fenced(holder, epoch),
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

    /// After a publish's step that ended at `ended`: where it failed, the
    /// wait of that publish alone; nothing else waits on it ([module
    /// docs](self)). A publish that ends takes its wait with it.
    fn after_publish(&mut self, step: Result<Step, RemoteError>, ended: Instant) -> Duration {
        match step {
            Ok(_) => {}
            Err(RemoteError::Fenced { holder, epoch }) => return self.fenced(holder, epoch),
            Err(_) => {
                if let Some(driver) = self.driver.as_mut() {
                    driver.publish_failed(ended);
                }
            }
        }
        Duration::ZERO
    }

    /// A transaction refused by the fence: `holder` holds the lease at
    /// `epoch`, and nothing more is uploaded or published.
    fn fenced(&mut self, holder: String, epoch: u64) -> Duration {
        self.lease.fenced(holder.clone(), epoch);
        self.state = State::LeaseLost { holder, epoch };
        self.publishing().stop();
        AT_REST
    }

    /// One step of the driver, a publish's where `publish`, else the
    /// cut's, and when it came back, with the lease kept beside it each
    /// time its renewal falls due while the step goes on. The step is
    /// polled throughout, and neither it nor a renewal is dropped half
    /// done: a step that ends during a renewal waits for the renewal to
    /// end, and its wait is still counted from its own end.
    ///
    /// A renewal that loses the lease does not stop the step: the step
    /// cannot be dropped, and a commit it makes is refused by the fence,
    /// which is what decides. The next turn finds the lease lost.
    async fn step_keeping(
        driver: &mut Driver<L, R>,
        publish: bool,
        lease: &mut Keeper<F>,
        lease_backoff: &mut Backoff,
        lease_at: &mut Instant,
    ) -> (Result<Step, RemoteError>, Instant) {
        let mut step = std::pin::pin!(async {
            if publish {
                driver.step_publish().await
            } else {
                driver.step_cut().await
            }
        });
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

    /// The wait until `until`, or until the lease is next kept, or a
    /// publish with a step to take may take it, if that comes first.
    fn next_wait(&self, now: Instant, until: Instant) -> Duration {
        let mut until = until.min(self.lease_at);
        if let Some(at) = self.driver.as_ref().and_then(Driver::publish_due_at) {
            until = until.min(at);
        }
        until.saturating_duration_since(now)
    }

    /// Turn until `stop` resolves, giving `publish` what the status shows
    /// after each turn and sleeping between turns as each turn asks; then
    /// the lease's keeper, for the caller to let go or keep. A turn under
    /// way at the stop is dropped, and the uploader with it: nothing can
    /// turn a driver left half way through a step. [`Uploader::run_with`]
    /// with no request.
    pub async fn run(
        self,
        stop: impl std::future::Future<Output = ()>,
        publish: impl FnMut(Shown),
    ) -> Keeper<F> {
        let (_, requests) = mpsc::channel(1);
        self.run_with(stop, requests, publish).await
    }

    /// As [`Uploader::run`], and answering each request from `requests`
    /// between two turns ([`Uploader::control`]), with what the status
    /// shows given to `publish` after each. A publish accepted begins
    /// the next turn at once; any other request leaves the sleep as it
    /// was. Once every sender of `requests` is dropped, it turns as
    /// [`Uploader::run`] does.
    pub async fn run_with(
        mut self,
        stop: impl std::future::Future<Output = ()>,
        mut requests: mpsc::Receiver<Control>,
        mut publish: impl FnMut(Shown),
    ) -> Keeper<F> {
        let mut stop = std::pin::pin!(stop);
        let mut open = true;
        'turns: loop {
            // The stop first: once it has come, no other turn begins.
            let wait = tokio::select! {
                biased;
                () = &mut stop => break,
                wait = self.turn() => wait,
            };
            self.post(&mut publish);
            let until = Instant::now() + wait;
            loop {
                tokio::select! {
                    biased;
                    () = &mut stop => break 'turns,
                    control = requests.recv(), if open => {
                        let Some(control) = control else {
                            open = false;
                            continue;
                        };
                        let now = self.control(control);
                        self.post(&mut publish);
                        if now {
                            continue 'turns;
                        }
                    }
                    () = tokio::time::sleep_until(until) => continue 'turns,
                }
            }
        }
        self.lease
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: Duration = Duration::from_secs(1);

    /// `steps` steps of `turns` while both are due, a publish's taking
    /// `publish` and the cut's `cut`: whose each was, `P` or `C`.
    fn run(turns: &mut Turns, steps: usize, publish: Duration, cut: Duration) -> String {
        (0..steps)
            .map(|_| {
                let p = turns.next(true, true).expect("both are due");
                turns.took(if p { publish } else { cut });
                if p {
                    'P'
                } else {
                    'C'
                }
            })
            .collect()
    }

    #[test]
    fn steps_of_the_same_time_take_turns() {
        assert_eq!(run(&mut Turns::default(), 6, S, S), "PCPCPC");
        let zero = Duration::ZERO;
        assert_eq!(run(&mut Turns::default(), 6, zero, zero), "PCPCPC");
    }

    /// F4 of the review of #126: after a publish's step of 20 s, the cut
    /// has the next 20 s, however many steps that is.
    #[test]
    fn a_slow_step_of_a_publish_gives_the_cut_as_long() {
        let mut turns = Turns::default();
        let cuts = "C".repeat(20);
        let expected = format!("P{cuts}P{cuts}P");
        assert_eq!(run(&mut turns, expected.len(), 20 * S, S), expected);
    }

    /// F5 of the review of #126: after a cut's step of 10 s, the
    /// publishes have the next 10 s.
    #[test]
    fn a_slow_step_of_the_cut_gives_the_publishes_as_long() {
        let mut turns = Turns::default();
        let expected = format!("PC{}C{}C", "P".repeat(9), "P".repeat(10));
        assert_eq!(run(&mut turns, expected.len(), S, 10 * S), expected);
    }

    /// Over a long run each side has half the time, give or take one step
    /// of the longer.
    #[test]
    fn each_side_has_half_the_time_give_or_take_a_step() {
        let mut turns = Turns::default();
        let (publish, cut) = (7 * S, 3 * S);
        let (mut p, mut c) = (Duration::ZERO, Duration::ZERO);
        for step in 0..1000 {
            if turns.next(true, true).expect("both are due") {
                turns.took(publish);
                p += publish;
            } else {
                turns.took(cut);
                c += cut;
            }
            let apart = p.abs_diff(c);
            assert!(apart <= publish, "step {step}: {p:?} and {c:?}");
        }
    }

    /// Time taken while one side had nothing to do is not owed to it,
    /// nor is time from before: the count starts again.
    #[test]
    fn the_count_starts_again_while_only_one_side_is_due() {
        // The publishes alone, the cut waiting in its backoff.
        let mut turns = Turns::default();
        assert_eq!(run(&mut turns, 1, 20 * S, S), "P");
        assert_eq!(turns.next(true, false), Some(true));
        turns.took(20 * S);
        assert_eq!(run(&mut turns, 4, S, S), "CPCP");
        // The cut alone, no publish due.
        let mut turns = Turns::default();
        assert_eq!(run(&mut turns, 2, S, 20 * S), "PC");
        assert_eq!(turns.next(false, true), Some(false));
        turns.took(20 * S);
        assert_eq!(run(&mut turns, 4, S, S), "PCPC");
        // Neither due.
        let mut turns = Turns::default();
        assert_eq!(run(&mut turns, 1, 20 * S, S), "P");
        assert_eq!(turns.next(false, false), None);
        assert_eq!(run(&mut turns, 4, S, S), "CPCP");
        // An idle cut's step.
        let mut turns = Turns::default();
        assert_eq!(run(&mut turns, 1, 20 * S, S), "P");
        turns.restart();
        assert_eq!(run(&mut turns, 4, S, S), "CPCP");
    }
}

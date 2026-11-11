//! The pump: it moves requests between one queue and one block target.
//! Generic over both, so that it is tested with a fake queue and no
//! kernel.
//!
//! One pump serves one queue on that queue's thread. Each turn it waits
//! for the queue, hands what was fetched to the target, takes what the
//! target has completed, and commits results to the queue.
//!
//! # The rules, and where each is kept
//!
//! - **Order (design §4.1).** Requests are submitted to the target in
//!   the order they were fetched, and a request is committed only after
//!   the target completed it. So an operation the kernel saw
//!   acknowledged before it issued another was completed by the target
//!   before the other was submitted, and precedes it in the target's
//!   order.
//! - **The bound on requests in flight.** The pump submits at most the
//!   target's queue depth and keeps the rest, in the order fetched. A
//!   request with FUA is in the target once at a time: its operation,
//!   then its flush.
//! - **Held requests.** A request the target has not completed is not
//!   committed, however long that takes, and nothing times it out here.
//! - **Rejected requests reach nothing** ([`translate`](crate::translate)):
//!   a refused request is committed with the plain I/O error and is
//!   never submitted.
//! - **FUA.** The flush is submitted once the operation was
//!   acknowledged by the target, and the request is committed when the
//!   flush has completed. If the operation fails there is no flush.
//! - **What was acknowledged (design §4.2).** Before a batch of
//!   completions is committed, the position of the newest acknowledged
//!   record is stored ([`Positions`]), once for the batch, and only if
//!   it is beyond what this pump stored last. If it cannot be stored,
//!   nothing of the batch is committed, reads in it included: the
//!   requests stay held, [`Counters::unstored`] says how many, and the
//!   store is tried again at the next turn. The pump has no timer of
//!   its own. The retry rides on the queue waking: for a request, for
//!   the target, or, with no traffic at all, for the idle time-out of
//!   the queue's own wait. In `libublk` 0.4.8 that is 20 seconds
//!   (`UBLK_QUEUE_IDLE_SECS` in its `io.rs`), and the kernel spike saw
//!   it: two wakes in 45 idle seconds, 20 s apart. So the longest wait
//!   for a retry with no traffic is 20 seconds.
//! - **An orderly stop** completes what it accepted: once the queue
//!   says stop, nothing more is accepted, what is in the target is
//!   waited for and committed, and the pump ends. Requests that were
//!   fetched and not yet submitted are left uncommitted, for the kernel
//!   to issue again to the next instance.
//! - **A stop does not call itself orderly while completions wait.**
//!   If the position cannot be stored when the target has drained, the
//!   stopping pump tries again, up to a stated number of times
//!   ([`Pump::stop_tries`]), with [`Queue::pause`] before each try. It
//!   does not go back to the queue's wait for that, so the retry of a
//!   stop does not ride the idle time-out: it is paced by the pause
//!   alone. If the position is stored, the completions are committed
//!   and the stop is [`Stopped::Orderly`]. If it never is, the pump
//!   ends with [`Stopped::Unstored`] and commits nothing: the requests
//!   stay with the kernel, which issues them again to the next
//!   instance.
//! - **For M3, not built:** a stopping pump whose target has not
//!   drained goes back to the queue's wait until it has. That is right
//!   while the store completes everything it accepted. Once a request
//!   can be held for as long as object storage is away, a stop must
//!   leave held requests to the next instance after a stated time, as
//!   it does for unstored completions now.
//! - **A failed volume holds no request** ([`Pump::fail`]). When the
//!   pump is told that the volume failed, it commits the plain I/O
//!   error for every request it holds, wherever it is: fetched and not
//!   submitted, in the target, behind its flush, or completed and
//!   waiting for a position that could not be stored. From then on it
//!   hands the target nothing, takes nothing from it, stores no
//!   position, and answers every request that still arrives with the
//!   same error, until the queue goes or a stop is asked
//!   ([`Stopped::Failed`]). The reason: the daemon deletes a failed
//!   volume's device, and the driver's delete of a device that is
//!   served waits for every request its server holds. A store whose
//!   disk has gone may take minutes over a request or never answer, and
//!   the record of what was acknowledged is on the same disk.
//!
//!   An I/O error is always a permitted answer: it promises nothing. A
//!   write answered so may be in the log and may be applied by the next
//!   open; a client that was told "failed" cannot rest on either. The
//!   one thing that must not happen is the reverse, a success for
//!   something that is not kept, and no success is committed after the
//!   failure, not even for a completion that had succeeded and only
//!   waited for its position.
//!
//!   What the target completes later is never taken from it. Its
//!   buffers (the data of the reads and writes it held) stay with the
//!   target until the process ends: the queue made a new buffer for
//!   each such tag when it was answered without one, and the old
//!   addresses are not given back to the allocator while the queue
//!   lives.
//! - **A target that is gone** (its worker thread has ended) ends the
//!   pump at once and nothing more is committed, not even what the
//!   target had completed. That is safe: with recovery by reissue the
//!   kernel issues every request that was not committed again to the
//!   next instance, and all five operations are idempotent (§4.1), so
//!   one the lost target had applied is simply applied again. Failing
//!   the requests instead would put errors on a pool for a defect of
//!   the daemon.

use std::collections::VecDeque;
use std::io;

use skyzfs_blk::{BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId, BLOCK_SIZE};

use crate::translate::{Action, Desc, Plan, Translator};

/// A request as a queue hands it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fetched {
    /// The queue's tag for the request: below [`Queue::depth`], and not
    /// reused until the request is committed.
    pub tag: u16,
    /// What is asked.
    pub desc: Desc,
}

/// Why [`Queue::wait`] returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wake {
    /// Requests were fetched, or the target has something, or nothing:
    /// a wake-up may be spurious.
    Work,
    /// The daemon is asked to stop in an orderly way.
    Stop,
}

/// The queue has gone: the device was deleted or is being taken away.
/// Nothing can be fetched or committed any more.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueDown;

/// One queue of a device, as the pump needs it.
pub trait Queue {
    /// The number of tags.
    fn depth(&self) -> u16;

    /// Block until requests arrive, the target's waker fires or a stop
    /// is asked, and append what was fetched. It may return with
    /// nothing.
    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown>;

    /// The buffer of the request with `tag`, `blocks` blocks long. For a
    /// write it holds the data. Called once for a read or a write that
    /// is submitted.
    ///
    /// `None` if the queue has no buffer that holds this request's
    /// data: the request is longer than the queue's buffers, or the
    /// tag's buffer is not in its place. The pump then fails the
    /// request (`EIO`) and hands the target nothing: a buffer made up
    /// in its place would give a write zeros for its data.
    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf>;

    /// Complete the request with `tag`: `result` is the byte count or a
    /// negated error. `buf` is the buffer [`Queue::take_buf`] gave, back
    /// again; for a read that succeeded it holds the data to deliver.
    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>);

    /// Wait a short, stated time: between the tries of a stopping pump
    /// to store the acknowledged position. Not the queue's wait: it
    /// fetches nothing and returns after its time whatever happens.
    fn pause(&mut self);

    /// Hand the driver every commit made so far, without waiting for
    /// anything. A queue may keep a commit until its next
    /// [`Queue::wait`]; the pump calls this once when it ends for a
    /// stop, after its last commit, because no wait follows that one.
    /// A request whose commit never reached the driver is issued again
    /// to the next instance: safe, but not what an orderly stop says
    /// it did. `Err` if they could not be handed over.
    fn flush(&mut self) -> Result<(), QueueDown>;
}

/// What the pump needs of a target beyond the contract.
pub trait Positions: BlockTarget {
    /// The log position at which the newest record starts among the
    /// mutations whose completions [`BlockTarget::poll`] or
    /// [`BlockTarget::wait`] have returned so far. `None` before the
    /// first.
    fn acknowledged(&self) -> Option<u64>;

    /// Whether the target can no longer complete anything: the thread
    /// that executes its requests has ended.
    fn gone(&self) -> bool;
}

/// Where the position of the newest acknowledged record is kept (design
/// §4.2, "what was acknowledged").
pub trait AckRecord {
    /// Store `position` unless a higher one is stored already. An error
    /// means it is not stored.
    fn advance(&mut self, position: u64) -> io::Result<()>;
}

/// Why the pump ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// A stop was asked, and everything the target had accepted is
    /// completed and committed, and the commits were handed to the
    /// driver ([`Queue::flush`]).
    Orderly,
    /// A stop was asked, the target has drained, and the acknowledged
    /// position could not be stored in the stated number of tries.
    /// `left` completions were not committed.
    Unstored {
        /// How many completions are left for the next instance.
        left: u64,
    },
    /// The queue went away. Completions that waited are not committed:
    /// there is no queue to commit them to.
    QueueDown,
    /// The target can complete nothing more. Nothing was committed
    /// after that was seen, not even completions that waited.
    TargetGone,
    /// A stop was asked of a pump whose volume had failed
    /// ([`Pump::fail`]). Every request it held or was handed since was
    /// answered with the plain I/O error, and the commits were handed
    /// to the driver if the queue took them.
    Failed,
    /// The target broke the contract in a way the pump cannot go on
    /// from: it refused an id that is not in flight, or completed one
    /// that is not. Completions that waited are not committed.
    ///
    /// In every ending but [`Stopped::Orderly`], what was not committed
    /// stays with the kernel, which issues it again to the next
    /// instance; all five operations are idempotent.
    TargetFault,
}

/// What the pump has done since it started.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counters {
    /// Reads committed, whatever the result.
    pub reads: u64,
    /// Writes committed.
    pub writes: u64,
    /// Flushes committed. The flush behind a request with FUA is not
    /// counted: the kernel issued none.
    pub flushes: u64,
    /// Discards committed.
    pub discards: u64,
    /// Write-zeroes committed.
    pub write_zeroes: u64,
    /// Requests refused by the translation.
    pub refused: u64,
    /// Requests fetched and not committed yet.
    pub in_flight: u64,
    /// Completions that are waiting because the acknowledged position
    /// could not be stored.
    pub unstored: u64,
}

/// Defects, for the tests that show each rule is tested.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Mutants(pub(crate) u32);

impl Mutants {
    /// A mutation is committed when it is submitted, before the target
    /// completed it.
    pub(crate) const ACK_AT_SUBMIT: u32 = 1 << 0;
    /// A flush is committed without the target's flush.
    pub(crate) const FLUSH_NOT_SUBMITTED: u32 = 1 << 1;
    /// A request with FUA is committed when its operation completed.
    pub(crate) const FUA_NOT_WAITED: u32 = 1 << 2;
    /// A refused request is submitted all the same, as a discard of the
    /// first block.
    pub(crate) const REFUSED_REACHES_TARGET: u32 = 1 << 3;
    /// The acknowledged position is stored after the completions are
    /// committed.
    pub(crate) const STORE_AFTER_COMMIT: u32 = 1 << 4;
    /// Completions are committed although the position was not stored.
    pub(crate) const COMMIT_UNSTORED: u32 = 1 << 5;
    /// More than the target's depth is submitted.
    pub(crate) const UNBOUNDED: u32 = 1 << 6;
    /// A stop ends the pump at once.
    pub(crate) const STOP_AT_ONCE: u32 = 1 << 7;
    /// A stop is orderly although completions wait for the position.
    pub(crate) const ORDERLY_UNSTORED: u32 = 1 << 8;
    /// Requests are submitted newest first.
    pub(crate) const NEWEST_FIRST: u32 = 1 << 9;
    /// A target that is gone is not noticed.
    pub(crate) const GONE_UNNOTICED: u32 = 1 << 10;
    /// A target that is gone is looked for only after submitting.
    pub(crate) const GONE_AFTER_SUBMIT: u32 = 1 << 11;
    /// A tag that is still in flight is taken again.
    pub(crate) const TAG_REUSED: u32 = 1 << 12;
    /// A completion nobody waits for is ignored.
    pub(crate) const STRAY_IGNORED: u32 = 1 << 13;
    /// A refusal with room is a fault, also when the target went in
    /// that moment.
    pub(crate) const REFUSAL_IS_FAULT: u32 = 1 << 14;
    /// A stop ends with its last commits still in the queue.
    pub(crate) const STOP_UNSENT: u32 = 1 << 15;
    /// A request the queue has no buffer for is handed to the target
    /// with a buffer of zeros.
    pub(crate) const UNBUFFERED_SUBMITTED: u32 = 1 << 16;
    /// Told that the volume failed, the pump answers nothing and goes
    /// on as before.
    pub(crate) const FAILED_HOLDS: u32 = 1 << 17;
    /// Told that the volume failed, the pump answers what it holds and
    /// then serves on: it submits to the target and takes its
    /// completions.
    pub(crate) const FAILED_SERVES: u32 = 1 << 18;
    /// At the failure, a completion that waited for its position is
    /// committed with its own result.
    pub(crate) const FAILED_UNSTORED_AS_IS: u32 = 1 << 19;
    /// At the failure, a write that waits for its flush to be
    /// submitted is not answered.
    pub(crate) const FAILED_SKIPS_FLUSH_WAITERS: u32 = 1 << 20;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Where a request is.
enum Slot {
    /// No request has this tag.
    Idle,
    /// Its operation is in the target.
    Running(Plan),
    /// Its operation succeeded and its flush is waiting for room.
    WantsFlush(Plan, Option<Buf>),
    /// Its flush is in the target.
    Flushing(Plan, Option<Buf>),
    /// It is finished and waits to be committed.
    Finished,
}

/// The id of a request's flush after its operation.
const FLUSH_ID: u64 = 1 << 32;

/// The pump of one queue. See the [module documentation](self).
pub struct Pump<Q: Queue, T: Positions, A: AckRecord> {
    queue: Q,
    target: T,
    ack: A,
    translator: Translator,
    slots: Vec<Slot>,
    /// Fetched and not yet submitted, oldest first.
    backlog: VecDeque<Fetched>,
    /// Tags whose flush waits for room, oldest first.
    wants_flush: VecDeque<u16>,
    /// Finished and not yet committed: tag, result, buffer.
    finished: Vec<(u16, i32, Option<Buf>)>,
    /// Requests in the target now.
    in_target: usize,
    /// The position this pump stored last.
    stored: Option<u64>,
    stopping: bool,
    /// The volume failed: see [`Pump::fail`].
    failed: bool,
    /// Requests answered with the plain I/O error because of that.
    failed_answers: u64,
    stop_tries: u32,
    counters: Counters,
    mutants: Mutants,
}

impl<Q: Queue, T: Positions, A: AckRecord> Pump<Q, T, A> {
    /// A pump between `queue` and `target`, which stores what it
    /// acknowledges in `ack`.
    pub fn new(queue: Q, target: T, ack: A) -> Self {
        let translator = Translator::new(&target.geometry());
        let slots = (0..queue.depth()).map(|_| Slot::Idle).collect();
        Self {
            queue,
            target,
            ack,
            translator,
            slots,
            backlog: VecDeque::new(),
            wants_flush: VecDeque::new(),
            finished: Vec::new(),
            in_target: 0,
            stored: None,
            stopping: false,
            failed: false,
            failed_answers: 0,
            stop_tries: Self::STOP_TRIES,
            counters: Counters::default(),
            mutants: Mutants::default(),
        }
    }

    /// How often a stopping pump tries again to store the position,
    /// unless told otherwise: with a pause of 100 ms, two seconds.
    pub const STOP_TRIES: u32 = 20;

    /// Set how often a stopping pump tries again to store the
    /// acknowledged position before it gives up, with
    /// [`Queue::pause`] before each try.
    pub fn stop_tries(mut self, tries: u32) -> Self {
        self.stop_tries = tries;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_mutants(mut self, mutants: u32) -> Self {
        self.mutants = Mutants(mutants);
        self
    }

    /// What the pump has done so far.
    pub fn counters(&self) -> Counters {
        self.counters
    }

    /// The queue.
    pub fn queue(&mut self) -> &mut Q {
        &mut self.queue
    }

    /// The target.
    pub fn target(&mut self) -> &mut T {
        &mut self.target
    }

    /// End the pump and give its target back: for a caller that must
    /// keep the target alive (the store holds the volume's lock) beyond
    /// the thread the pump ran on. The queue and the record are
    /// dropped.
    pub fn into_target(self) -> T {
        self.target
    }

    /// The record of what was acknowledged.
    pub fn ack(&mut self) -> &mut A {
        &mut self.ack
    }

    /// The volume failed: answer every request the pump holds with the
    /// plain I/O error, and from now on every request that arrives.
    /// Returns how many were answered by this call: 0 if the pump had
    /// been told before. See the [module documentation](self).
    ///
    /// The commits are made, not sent: a queue may keep a commit until
    /// its next [`Queue::wait`], so the caller goes on calling
    /// [`Pump::turn`], whose wait sends them. It is not sent here
    /// because [`Queue::flush`] is for the end of a queue: the kernel's
    /// drops what it fetches meanwhile.
    pub fn fail(&mut self) -> u64 {
        if self.failed || self.mutants.has(Mutants::FAILED_HOLDS) {
            return 0;
        }
        self.failed = true;
        let before = self.failed_answers;
        let error = -crate::translate::EIO;
        // What is finished and waits for its position first: the pump
        // has these buffers and gives them back.
        let as_is = self.mutants.has(Mutants::FAILED_UNSTORED_AS_IS);
        for (tag, result, buf) in std::mem::take(&mut self.finished) {
            let result = if as_is { result } else { error };
            self.slots[usize::from(tag)] = Slot::Idle;
            self.queue.commit(tag, result, buf);
            self.failed_answers += 1;
        }
        let skips = self.mutants.has(Mutants::FAILED_SKIPS_FLUSH_WAITERS);
        for tag in 0..self.slots.len() {
            if skips && matches!(self.slots[tag], Slot::WantsFlush(..)) {
                continue;
            }
            let buf = match std::mem::replace(&mut self.slots[tag], Slot::Idle) {
                Slot::Idle | Slot::Finished => continue,
                // The buffer is in the target and stays there.
                Slot::Running(_) => None,
                Slot::WantsFlush(_, buf) | Slot::Flushing(_, buf) => buf,
            };
            self.queue.commit(tag as u16, error, buf);
            self.failed_answers += 1;
        }
        for fetched in std::mem::take(&mut self.backlog) {
            self.queue.commit(fetched.tag, error, None);
            self.failed_answers += 1;
        }
        self.wants_flush.clear();
        self.in_target = 0;
        self.counters.in_flight = 0;
        self.counters.unstored = 0;
        self.failed_answers - before
    }

    /// Whether the pump was told that the volume failed.
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    /// How many requests were answered with the plain I/O error because
    /// the volume had failed: those held at the failure and those that
    /// arrived after it.
    pub fn failed_answers(&self) -> u64 {
        self.failed_answers
    }

    /// A turn of a pump whose volume failed: what arrives is answered
    /// with the plain I/O error, and the target is not touched.
    fn failed_turn(&mut self) -> Option<Stopped> {
        let mut fetched = Vec::new();
        let woken = match self.queue.wait(&mut fetched) {
            Err(QueueDown) => return Some(Stopped::QueueDown),
            Ok(woken) => woken,
        };
        for Fetched { tag, .. } in fetched {
            self.queue.commit(tag, -crate::translate::EIO, None);
            self.failed_answers += 1;
        }
        if woken == Wake::Stop {
            // No wait follows that would send the commits.
            let _ = self.queue.flush();
            return Some(Stopped::Failed);
        }
        None
    }

    /// Serve until the pump ends.
    pub fn run(&mut self) -> Stopped {
        loop {
            if let Some(stopped) = self.turn() {
                return stopped;
            }
        }
    }

    /// One turn: wait for the queue, move what can move. `Some` when
    /// the pump has ended.
    pub fn turn(&mut self) -> Option<Stopped> {
        if self.failed && !self.mutants.has(Mutants::FAILED_SERVES) {
            return self.failed_turn();
        }
        let mut fetched = Vec::new();
        match self.queue.wait(&mut fetched) {
            Err(QueueDown) => return Some(Stopped::QueueDown),
            Ok(Wake::Stop) => self.stopping = true,
            Ok(Wake::Work) => {}
        }
        if self.stopping && self.mutants.has(Mutants::STOP_AT_ONCE) {
            return Some(Stopped::Orderly);
        }
        if !self.stopping {
            self.counters.in_flight += fetched.len() as u64;
            self.backlog.extend(fetched);
        }
        // Completions make room, and room lets more be submitted.
        loop {
            // Before anything is submitted: a target whose worker has
            // ended refuses requests, and that refusal is not a broken
            // contract.
            if self.is_gone() && !self.mutants.has(Mutants::GONE_AFTER_SUBMIT) {
                return Some(Stopped::TargetGone);
            }
            if let Err(stopped) = self.submit_what_fits() {
                return Some(stopped);
            }
            if self.is_gone() {
                return Some(Stopped::TargetGone);
            }
            let mut done = Vec::new();
            if self.target.poll(&mut done) == 0 && done.is_empty() {
                break;
            }
            for completion in done {
                if let Err(stopped) = self.completed(completion) {
                    return Some(stopped);
                }
            }
        }
        self.commit_finished();
        let drained = self.in_target == 0 && self.wants_flush.is_empty();
        if !(self.stopping && drained) {
            return None;
        }
        if self.mutants.has(Mutants::ORDERLY_UNSTORED) {
            return Some(Stopped::Orderly);
        }
        // The target has drained. Completions that wait for the
        // position are tried again here, paced by the pause and not by
        // the queue's wait.
        for _ in 0..self.stop_tries {
            if self.finished.is_empty() {
                break;
            }
            self.queue.pause();
            self.commit_finished();
        }
        // The last commits of a stop have no wait behind them that
        // would send them: the queue hands them over now. A stop whose
        // commits could not be handed over completed nothing in the
        // kernel's eyes and is not orderly.
        if !self.mutants.has(Mutants::STOP_UNSENT) && self.queue.flush().is_err() {
            return Some(Stopped::QueueDown);
        }
        if self.finished.is_empty() {
            Some(Stopped::Orderly)
        } else {
            Some(Stopped::Unstored {
                left: self.finished.len() as u64,
            })
        }
    }

    fn is_gone(&self) -> bool {
        self.target.gone() && !self.mutants.has(Mutants::GONE_UNNOTICED)
    }

    /// The target refused a request it had room for: because its worker
    /// ended in that moment, or against the contract.
    fn refused(&self) -> Stopped {
        let fault = self.mutants.has(Mutants::GONE_AFTER_SUBMIT)
            || self.mutants.has(Mutants::REFUSAL_IS_FAULT);
        if self.is_gone() && !fault {
            Stopped::TargetGone
        } else {
            Stopped::TargetFault
        }
    }

    /// How many requests the target takes at once.
    fn room(&self) -> usize {
        if self.mutants.has(Mutants::UNBOUNDED) {
            return usize::MAX;
        }
        (self.target.geometry().queue_depth() as usize).saturating_sub(self.in_target)
    }

    /// Submit flushes that wait, then the backlog, oldest first, as far
    /// as the target has room. A stop submits no new request, but the
    /// flush of a request whose operation was accepted belongs to it.
    fn submit_what_fits(&mut self) -> Result<(), Stopped> {
        while self.room() > 0 {
            let Some(&tag) = self.wants_flush.front() else {
                break;
            };
            let request = Request {
                id: RequestId(u64::from(tag) | FLUSH_ID),
                op: Op::Flush,
            };
            match self.target.submit(request) {
                Ok(()) => {}
                Err(e) if e.kind == skyzfs_blk::SubmitErrorKind::Busy => return Ok(()),
                Err(_) => return Err(self.refused()),
            }
            self.wants_flush.pop_front();
            self.in_target += 1;
            let slot = &mut self.slots[usize::from(tag)];
            if let Slot::WantsFlush(plan, buf) = std::mem::replace(slot, Slot::Idle) {
                *slot = Slot::Flushing(plan, buf);
            }
        }
        while !self.stopping && self.room() > 0 {
            let next = if self.mutants.has(Mutants::NEWEST_FIRST) {
                self.backlog.pop_back()
            } else {
                self.backlog.pop_front()
            };
            let Some(fetched) = next else {
                break;
            };
            if !self.submit(fetched)? {
                self.backlog.push_front(fetched);
                break;
            }
        }
        Ok(())
    }

    /// Translate and submit one request. `false` if the target is full.
    fn submit(&mut self, fetched: Fetched) -> Result<bool, Stopped> {
        let Fetched { tag, desc } = fetched;
        let Some(slot) = self.slots.get(usize::from(tag)) else {
            // A tag the queue does not have: nothing can be committed
            // for it.
            return Err(Stopped::QueueDown);
        };
        if !matches!(slot, Slot::Idle) && !self.mutants.has(Mutants::TAG_REUSED) {
            // The queue reused a tag that is not committed.
            return Err(Stopped::QueueDown);
        }
        let plan = match self.translator.translate(desc) {
            Ok(plan) => plan,
            Err(_) => {
                if self.mutants.has(Mutants::REFUSED_REACHES_TARGET) {
                    let _ = self.target.submit(Request {
                        id: RequestId(u64::from(tag)),
                        op: Op::Discard {
                            offset: 0,
                            len: BLOCK_SIZE,
                        },
                    });
                    self.in_target += 1;
                    self.slots[usize::from(tag)] = Slot::Running(Plan {
                        action: Action::Discard {
                            offset: 0,
                            len: BLOCK_SIZE,
                        },
                        then_flush: false,
                    });
                    return Ok(true);
                }
                self.counters.refused += 1;
                self.slots[usize::from(tag)] = Slot::Finished;
                self.finished
                    .push((tag, self.translator.refused(desc), None));
                return Ok(true);
            }
        };
        if matches!(plan.action, Action::Flush) && self.mutants.has(Mutants::FLUSH_NOT_SUBMITTED) {
            self.finish(tag, &plan, &Ok(()), None);
            return Ok(true);
        }
        let blocks = |len: u64| (len / BLOCK_SIZE) as usize;
        // The buffer is taken only once the target has room: a request
        // that waits in the backlog holds none.
        if self.room() == 0 {
            return Ok(false);
        }
        let buf = match plan.action {
            Action::Read { len, .. } | Action::Write { len, .. } => {
                match self.queue.take_buf(tag, blocks(len)) {
                    Some(buf) => Some(buf),
                    None if self.mutants.has(Mutants::UNBUFFERED_SUBMITTED) => {
                        Some(Buf::zeroed(blocks(len)))
                    }
                    None => {
                        // The queue has no buffer with this request's
                        // data: it fails, and the target sees nothing.
                        self.finish(tag, &plan, &Err(BlkError::Failed), None);
                        return Ok(true);
                    }
                }
            }
            _ => None,
        };
        let op = match (plan.action, buf) {
            (Action::Read { offset, .. }, Some(buf)) => Op::Read { offset, buf },
            (Action::Write { offset, .. }, Some(data)) => Op::Write { offset, data },
            // Not reached: a read or a write has its buffer here.
            (Action::Read { .. } | Action::Write { .. }, None) => return Ok(true),
            (Action::Flush, _) => Op::Flush,
            (Action::Discard { offset, len }, _) => Op::Discard { offset, len },
            (Action::WriteZeroes { offset, len }, _) => Op::WriteZeroes { offset, len },
        };
        let request = Request {
            id: RequestId(u64::from(tag)),
            op,
        };
        if self.target.submit(request).is_err() {
            // There was room, and the tag is not in flight: the target
            // is gone, or it is not keeping the contract. The buffer
            // went with the request.
            return Err(self.refused());
        }
        self.in_target += 1;
        self.slots[usize::from(tag)] = Slot::Running(plan);
        let mutation = !matches!(plan.action, Action::Read { .. } | Action::Flush);
        if mutation && self.mutants.has(Mutants::ACK_AT_SUBMIT) {
            self.queue
                .commit(tag, self.translator.result(&plan, &Ok(())), None);
        }
        Ok(true)
    }

    /// The target completed something.
    fn completed(&mut self, completion: Completion) -> Result<(), Stopped> {
        let Completion { id, result, buf } = completion;
        let tag = (id.0 & (FLUSH_ID - 1)) as u16;
        let is_flush = id.0 & FLUSH_ID != 0;
        let Some(slot) = self.slots.get_mut(usize::from(tag)) else {
            return Err(Stopped::TargetFault);
        };
        self.in_target = self.in_target.saturating_sub(1);
        match std::mem::replace(slot, Slot::Idle) {
            Slot::Running(plan) if !is_flush => {
                let waits = plan.then_flush && !self.mutants.has(Mutants::FUA_NOT_WAITED);
                if result.is_ok() && waits {
                    *slot = Slot::WantsFlush(plan, buf);
                    self.wants_flush.push_back(tag);
                } else {
                    self.finish(tag, &plan, &result, buf);
                }
                Ok(())
            }
            Slot::Flushing(plan, kept) if is_flush => {
                self.finish(tag, &plan, &result, kept);
                Ok(())
            }
            other if self.mutants.has(Mutants::STRAY_IGNORED) => {
                self.slots[usize::from(tag)] = other;
                Ok(())
            }
            _ => Err(Stopped::TargetFault),
        }
    }

    /// A request is finished: it waits to be committed.
    fn finish(&mut self, tag: u16, plan: &Plan, outcome: &Result<(), BlkError>, buf: Option<Buf>) {
        match plan.action {
            Action::Read { .. } => self.counters.reads += 1,
            Action::Write { .. } => self.counters.writes += 1,
            Action::Flush => self.counters.flushes += 1,
            Action::Discard { .. } => self.counters.discards += 1,
            Action::WriteZeroes { .. } => self.counters.write_zeroes += 1,
        }
        self.slots[usize::from(tag)] = Slot::Finished;
        let mutation = !matches!(plan.action, Action::Read { .. } | Action::Flush);
        if mutation && self.mutants.has(Mutants::ACK_AT_SUBMIT) {
            // Committed already.
            self.slots[usize::from(tag)] = Slot::Idle;
            self.counters.in_flight = self.counters.in_flight.saturating_sub(1);
            drop(buf);
            return;
        }
        self.finished
            .push((tag, self.translator.result(plan, outcome), buf));
    }

    /// Store the acknowledged position, then commit what is finished.
    /// If the position cannot be stored, nothing is committed.
    fn commit_finished(&mut self) {
        if self.finished.is_empty() {
            return;
        }
        let after = self.mutants.has(Mutants::STORE_AFTER_COMMIT);
        if !after && self.store_position().is_err() && !self.mutants.has(Mutants::COMMIT_UNSTORED) {
            self.counters.unstored = self.finished.len() as u64;
            return;
        }
        self.counters.unstored = 0;
        for (tag, result, buf) in self.finished.drain(..) {
            self.slots[usize::from(tag)] = Slot::Idle;
            self.counters.in_flight = self.counters.in_flight.saturating_sub(1);
            self.queue.commit(tag, result, buf);
        }
        if after {
            let _ = self.store_position();
        }
    }

    fn store_position(&mut self) -> io::Result<()> {
        let Some(position) = self.target.acknowledged() else {
            return Ok(());
        };
        if self.stored.is_some_and(|stored| position <= stored) {
            return Ok(());
        }
        self.ack.advance(position)?;
        self.stored = Some(position);
        Ok(())
    }
}

impl Positions for skyzfs_local::store::Served {
    fn acknowledged(&self) -> Option<u64> {
        skyzfs_local::store::Served::acknowledged(self).map(|position| position.0)
    }

    fn gone(&self) -> bool {
        skyzfs_local::store::Served::gone(self)
    }
}

#[cfg(test)]
mod tests;

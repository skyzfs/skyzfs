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
//! - **An orderly stop** completes what it accepted: once the queue
//!   says stop, nothing more is accepted, what is in the target is
//!   waited for and committed, and the pump ends. Requests that were
//!   fetched and not yet submitted are left uncommitted, for the kernel
//!   to issue again to the next instance.
//! - **For M3, not built:** a stopping pump whose target has not
//!   drained goes back to the queue's wait until it has. That is right
//!   while the store completes everything it accepted. Once a request
//!   can be held for as long as object storage is away, a stop must
//!   leave held requests to the next instance after a stated time.
//! - **A failed volume holds no request** ([`Pump::fail`]). When the
//!   pump is told that the volume failed, it commits the plain I/O
//!   error for every request it holds, wherever it is: fetched and not
//!   submitted, in the target, behind its flush, or completed and not
//!   yet committed. From then on it hands the target nothing, takes
//!   nothing from it, and answers every request that still arrives with the
//!   same error, until the queue goes or a stop is asked
//!   ([`Stopped::Failed`]). The reason: the daemon deletes a failed
//!   volume's device, and the driver's delete of a device that is
//!   served waits for every request its server holds. A store whose
//!   disk has gone may take minutes over a request or never answer.
//!
//!   An I/O error is always a permitted answer: it promises nothing. A
//!   write answered so may be on the disk or not; a client that was
//!   told "failed" cannot rest on either. The one thing that must not
//!   happen is the reverse, a success for something that is not kept,
//!   and no success is committed after the failure, not even for a
//!   completion that had succeeded and only waited for its commit.
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
//! - **A commit the driver refuses** ends the pump for a take-over
//!   ([`Stopped::CommitRefused`], M1.3ac part 3). The refused commit
//!   took no effect: the request it answered, if any, is still the
//!   server's in the driver's eyes, and the kernel hands it to the next
//!   server, which is handed exactly that request again and completes
//!   it (the control rig, on a read, a write and a flush). So nothing
//!   more is answered on the queue, nothing of the wait that brought
//!   the refusal is taken, and the volume is not failed: the refusal
//!   says nothing against the store. A refused fetch and any other
//!   command refused are counted and change nothing else. A pump whose
//!   volume failed only counts a refused commit too: the device is
//!   being deleted, and the delete answers the refused request with an
//!   error at once.
//! - **Open: a refused fetch** (M1.3ah). A fetch is sent once per tag,
//!   at the queue's start. Whether the driver then serves the queue
//!   with that tag missing, or the device's start and the end of a
//!   recovery wait for every tag's fetch (a reading of `ublk_drv`, not
//!   checked), so that the start never ends, is not known. A kernel
//!   step comes first (M1.T36), and what the daemon does then.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use skyzfs_local::store::Spent;

use skyzfs_blk::{BlkError, BlockTarget, Buf, Completion, Op, Request, RequestId, BLOCK_SIZE};

use crate::translate::{Action, Desc, Plan, Translator};

/// A request as a queue hands it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fetched {
    /// The queue's tag for the request: below [`Queue::depth`], and not
    /// reused until the request is committed. A queue that breaks this
    /// ends the pump at the hand-over ([`Stopped::BrokenRule`]), so
    /// that a tag is never committed twice for one request.
    pub tag: u16,
    /// What is asked.
    pub desc: Desc,
}

/// A command of a queue that the driver ended with a result other than
/// OK: it fetched no request. [`Queue::wait`] hands these over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unfetched {
    /// The tag the command was for.
    pub tag: u16,
    /// Which command.
    pub command: Command,
    /// The driver's result: [`ABORTED`] when the queue is being
    /// stopped, a negated error number when the driver refused
    /// the command, or another code it does not use with this queue.
    pub result: i32,
}

/// A command a queue sends the driver for a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// The tag's first fetch, at the queue's start.
    Fetch,
    /// The tag's answer, which also fetches its next request.
    Commit,
    /// Another command, by the low byte of its code.
    Other(u8),
}

/// The driver's result for a command of a queue that is being stopped
/// (`UBLK_IO_RES_ABORT`, `-ENODEV`): at a detach, a deleted device, a
/// failed volume. Every tag's last command ends so.
pub const ABORTED: i32 = -19;

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
    ///
    /// A command that the driver ended with a result other than OK
    /// fetched nothing: it is appended to `unfetched`, whether the wait
    /// then returns `Ok` or `Err`. The pump counts them
    /// ([`Counters::commands_aborted`], [`Counters::commands_failed`]),
    /// and ends on a refused commit (below). An abort is the queue being
    /// stopped. Any other result is a command that took no effect: one
    /// the driver refused when it was submitted, or one io_uring could
    /// not hand to the driver at all (a bad descriptor or address). A
    /// refused commit completed nothing, and its tag is then in one of
    /// two states, which the result does not tell apart (`-EINVAL`
    /// comes of either kind on every release the daemon runs on):
    ///
    /// - **The tag held no request in the server's hands**: a second
    ///   commit of one tag, or a commit of a tag that was never given a
    ///   request. The commit was for nothing. A command that waits on
    ///   the tag still waits, and requests come on it as before.
    /// - **The commit was malformed**: no buffer for a read, the wrong
    ///   thread, the wrong code, a tag out of range. The driver still
    ///   counts the request as the server's, unanswered, and no command
    ///   waits on the tag, so nothing comes on it again until the queue
    ///   stops.
    ///
    /// A count of 0 does not show that no commit was sent twice: a
    /// second commit that arrives after the tag's next request was
    /// handed over is taken, and answers that request with the stale
    /// result. A refused commit ends the pump
    /// ([`Stopped::CommitRefused`]); a refused fetch or other command is
    /// only counted.
    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown>;

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
pub trait PumpTarget: BlockTarget {
    /// Whether the target can no longer complete anything: the thread
    /// that executes its requests has ended.
    fn gone(&self) -> bool;

    /// When the target had the completion of request `id` ready, for a
    /// completion the last poll returned, if the target says: the pump
    /// then counts a completion's wait from there and not from when it
    /// took the completion.
    fn ready_at(&self, _id: RequestId) -> Option<Instant> {
        None
    }
}

/// Why the pump ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// A stop was asked, and everything the target had accepted is
    /// completed and committed, and the commits were handed to the
    /// driver ([`Queue::flush`]).
    Orderly,
    /// The queue went away. Completions that waited are not committed:
    /// there is no queue to commit them to.
    QueueDown,
    /// The driver broke its rule that a tag is handed over again only
    /// after its commit: it handed over `tag`, which the queue does not
    /// have or whose request is still out. Nothing of that wait was
    /// submitted or committed, and nothing more is answered on this
    /// queue: an answer could complete the wrong request. Completions
    /// that waited are not committed. The breach is the driver's, not
    /// the store's: the volume is not failed, and the device is left
    /// for the next server as after a kill (M1.3ac).
    BrokenRule {
        /// The tag handed over against the rule.
        tag: u16,
    },
    /// The driver refused a commit: the first of the wait that brought
    /// the refusal, if it brought several (M1.3ac part 3), also while the
    /// pump drains for a stop: that stop is not orderly, and an orderly
    /// end would say that nothing was left unanswered. The commit
    /// took no effect, so the request it answered, if any, is still the
    /// server's in the driver's eyes and goes to the next server.
    /// Nothing of that wait was taken, nothing more is answered on this
    /// queue and the refused tag is not committed again: completions
    /// that waited are not committed. The volume is not failed, and the
    /// device is left for the next server as after a kill. In the turn
    /// of a pump whose volume failed ([`Pump::fail`]) a refused commit
    /// is only counted, and a wait that ends with the queue down ends
    /// the pump with [`Stopped::QueueDown`], whatever it brought.
    CommitRefused {
        /// The tag of the refused commit.
        tag: u16,
        /// The driver's result negated: the error number it refused the
        /// commit with (`EINVAL`, 22, on every release the daemon runs
        /// on, whatever the cause).
        errno: i32,
    },
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
    /// Always 0 since M1.3t-b, which removed the record of what was
    /// acknowledged: completions that waited because the position could
    /// not be stored. Kept so that the status file keeps its field, and
    /// what reads it reads on, until the file's next version.
    pub unstored: u64,
    /// Commands the driver ended with its abort ([`ABORTED`]): the queue
    /// is being stopped. Each tag's last command ends so.
    pub commands_aborted: u64,
    /// Commands that ended with any other result than OK: a commit or a
    /// fetch the driver refused, or one io_uring could not hand to the
    /// driver (see [`Queue::wait`]). None of them took effect.
    pub commands_failed: u64,
    /// The result of the last command counted in
    /// [`Counters::commands_failed`]; 0 before the first.
    pub last_command_failure: i32,
}

/// Where a request's time goes in the pump, before the target has it
/// and after the target is done with it. Measurement only. One read
/// of the monotonic clock per turn that fetched something, per
/// request submitted, per request finished and per batch committed;
/// the times are kept by tag, in a table as long as the queue is deep.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PumpTiming {
    /// From the queue's wait returning with a request to the target
    /// having it: the translation, a first write's copy, and the time
    /// in the backlog while the target had no room. What the request
    /// waited in the kernel before the pump's thread came back to the
    /// queue is before this and in no number.
    pub to_submit: Spent,
    /// Reads: from the request being finished to its commit to the
    /// queue. Finished is when the target had the completion ready, if
    /// the target says so (the store does: the end of its call), else
    /// when the pump took the completion or refused the request. So it
    /// holds the time until the pump's thread came to take the
    /// completion.
    pub read_to_commit: Spent,
    /// The same for every other request.
    pub other_to_commit: Spent,
    /// Reads: the first part of [`PumpTiming::read_to_commit`] alone,
    /// from the target having the completion ready to the pump taking
    /// it, which is the hand-over from the target's thread to the
    /// pump's. Counted at the commit, so the count is that of
    /// `read_to_commit`, and the rest of that time is what the commit
    /// waited for after the pump had the completion: the other
    /// completions of the batch. Zero
    /// for a request the target never had, and where the target does
    /// not say when a completion was ready. A write that is followed
    /// by a flush of the pump's own counts the flush's completion.
    pub read_to_taken: Spent,
    /// The same for every other request.
    pub other_to_taken: Spent,
    /// Always zero since M1.3t-b: the storing of the acknowledged
    /// position. Kept, as [`Counters::unstored`] is, for the status
    /// file's fields.
    pub store: Spent,
    /// Always 0 since M1.3t-b: the completions committed behind those
    /// stores.
    pub behind_store: u64,
}

/// What the pump notes for a tag in flight.
#[derive(Debug, Clone, Copy)]
struct Stamp {
    /// When the queue's wait returned with it.
    fetched: Instant,
    /// When it was finished.
    finished: Instant,
    /// From the target having its completion ready to the pump taking
    /// it.
    taken: Duration,
    /// Whether it is a read.
    read: bool,
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
    /// More than the target's depth is submitted.
    pub(crate) const UNBOUNDED: u32 = 1 << 6;
    /// A stop ends the pump at once.
    pub(crate) const STOP_AT_ONCE: u32 = 1 << 7;
    /// Requests are submitted newest first.
    pub(crate) const NEWEST_FIRST: u32 = 1 << 9;
    /// A target that is gone is not noticed.
    pub(crate) const GONE_UNNOTICED: u32 = 1 << 10;
    /// A target that is gone is looked for only after submitting.
    pub(crate) const GONE_AFTER_SUBMIT: u32 = 1 << 11;
    /// A tag that is still in flight is taken again: no guard at all.
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
    /// At the failure, a write that waits for its flush to be
    /// submitted is not answered.
    pub(crate) const FAILED_SKIPS_FLUSH_WAITERS: u32 = 1 << 20;
    /// The guard against a tag handed over again looks at the tag's
    /// slot when its request is submitted, not when it is handed over
    /// (as before M1.3ac): a request on a tag still out that waits in
    /// the backlog until the tag's first request is committed gets
    /// past it.
    pub(crate) const GUARD_AT_SUBMIT: u32 = 1 << 21;
    /// A commit the driver refused is only counted and the pump goes
    /// on, as before M1.3ac part 3.
    pub(crate) const COMMIT_REFUSAL_IGNORED: u32 = 1 << 22;

    fn has(self, flag: u32) -> bool {
        self.0 & flag != 0
    }
}

/// Where a request is.
enum Slot {
    /// No request has this tag: the driver may hand it over.
    Idle,
    /// Its request was handed over and waits in the backlog.
    Waiting,
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
pub struct Pump<Q: Queue, T: PumpTarget> {
    queue: Q,
    target: T,
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
    stopping: bool,
    /// The volume failed: see [`Pump::fail`].
    failed: bool,
    /// Requests answered with the plain I/O error because of that.
    failed_answers: u64,
    counters: Counters,
    timing: PumpTiming,
    /// By tag.
    stamps: Vec<Stamp>,
    mutants: Mutants,
}

impl<Q: Queue, T: PumpTarget> Pump<Q, T> {
    /// A pump between `queue` and `target`.
    pub fn new(queue: Q, target: T) -> Self {
        let translator = Translator::new(&target.geometry());
        let slots = (0..queue.depth()).map(|_| Slot::Idle).collect();
        let now = Instant::now();
        let stamp = Stamp {
            fetched: now,
            finished: now,
            taken: Duration::ZERO,
            read: false,
        };
        let stamps = (0..queue.depth()).map(|_| stamp).collect();
        Self {
            queue,
            target,
            translator,
            slots,
            backlog: VecDeque::new(),
            wants_flush: VecDeque::new(),
            finished: Vec::new(),
            in_target: 0,
            stopping: false,
            failed: false,
            failed_answers: 0,
            counters: Counters::default(),
            timing: PumpTiming::default(),
            stamps,
            mutants: Mutants::default(),
        }
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

    /// Where the requests' time went in the pump: see [`PumpTiming`].
    pub fn timing(&self) -> PumpTiming {
        self.timing
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
    /// the thread the pump ran on. The queue is dropped.
    pub fn into_target(self) -> T {
        self.target
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
        // What is finished and not yet committed first: the pump has
        // these buffers and gives them back.
        for (tag, _, buf) in std::mem::take(&mut self.finished) {
            self.slots[usize::from(tag)] = Slot::Idle;
            self.queue.commit(tag, error, buf);
            self.failed_answers += 1;
        }
        let skips = self.mutants.has(Mutants::FAILED_SKIPS_FLUSH_WAITERS);
        for tag in 0..self.slots.len() {
            if skips && matches!(self.slots[tag], Slot::WantsFlush(..)) {
                continue;
            }
            let buf = match std::mem::replace(&mut self.slots[tag], Slot::Idle) {
                // A waiting request is in the backlog and is answered
                // from there below. One that `submit_what_fits` took from
                // the backlog and the target refused is in neither: the
                // pump ended then (`TargetGone`, `TargetFault`), and the
                // request stays with the driver, uncommitted, as it did
                // before its slot could be `Waiting`.
                Slot::Idle | Slot::Waiting | Slot::Finished => continue,
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

    /// The pump's end for the first commit of `unfetched` that the
    /// driver refused, if one is there ([`Stopped::CommitRefused`]). An
    /// abort is the queue being stopped, not a refusal.
    fn refused_commit(&self, unfetched: &[Unfetched]) -> Option<Stopped> {
        if self.mutants.has(Mutants::COMMIT_REFUSAL_IGNORED) {
            return None;
        }
        unfetched
            .iter()
            .find(|command| command.command == Command::Commit && command.result != ABORTED)
            .map(|command| Stopped::CommitRefused {
                tag: command.tag,
                errno: command.result.saturating_neg(),
            })
    }

    /// Count the commands the driver ended otherwise than OK.
    fn count_unfetched(&mut self, unfetched: &[Unfetched]) {
        for command in unfetched {
            if command.result == ABORTED {
                self.counters.commands_aborted += 1;
            } else {
                self.counters.commands_failed += 1;
                self.counters.last_command_failure = command.result;
            }
        }
    }

    /// A turn of a pump whose volume failed: what arrives is answered
    /// with the plain I/O error, and the target is not touched. A
    /// refused commit is only counted here, and does not end the pump
    /// as it does in [`Pump::turn`]: the device is being deleted, the
    /// delete answers the refused request with an error at once (the
    /// control rig), and what else arrives is still answered so that
    /// the delete waits for nothing.
    fn failed_turn(&mut self) -> Option<Stopped> {
        let mut fetched = Vec::new();
        let mut unfetched = Vec::new();
        let waited = self.queue.wait(&mut fetched, &mut unfetched);
        self.count_unfetched(&unfetched);
        let woken = match waited {
            Err(QueueDown) => return Some(Stopped::QueueDown),
            Ok(woken) => woken,
        };
        if let Err(stopped) = self.take_over(&fetched) {
            return Some(stopped);
        }
        for Fetched { tag, .. } in fetched {
            if let Some(slot) = self.slots.get_mut(usize::from(tag)) {
                *slot = Slot::Idle;
            }
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
        let mut unfetched = Vec::new();
        let waited = self.queue.wait(&mut fetched, &mut unfetched);
        self.count_unfetched(&unfetched);
        if waited == Err(QueueDown) {
            return Some(Stopped::QueueDown);
        }
        // Before anything of this wait is taken: what it handed over is
        // the next server's.
        if let Some(refused) = self.refused_commit(&unfetched) {
            return Some(refused);
        }
        if waited == Ok(Wake::Stop) {
            self.stopping = true;
        }
        if self.stopping && self.mutants.has(Mutants::STOP_AT_ONCE) {
            return Some(Stopped::Orderly);
        }
        if !self.stopping {
            if let Err(stopped) = self.take_over(&fetched) {
                return Some(stopped);
            }
            self.counters.in_flight += fetched.len() as u64;
            if !fetched.is_empty() {
                let now = Instant::now();
                for request in &fetched {
                    if let Some(stamp) = self.stamps.get_mut(usize::from(request.tag)) {
                        stamp.fetched = now;
                    }
                }
            }
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
        // The last commits of a stop have no wait behind them that
        // would send them: the queue hands them over now. A stop whose
        // commits could not be handed over completed nothing in the
        // kernel's eyes and is not orderly.
        if !self.mutants.has(Mutants::STOP_UNSENT) && self.queue.flush().is_err() {
            return Some(Stopped::QueueDown);
        }
        Some(Stopped::Orderly)
    }

    /// Take what the queue handed over at a wait: each tag must be one
    /// the queue has and have no request out, or the driver broke its
    /// rule that a tag is handed over again only after its commit, and
    /// the pump ends ([`Stopped::BrokenRule`]) before anything of this wait is
    /// submitted or committed. Checked here, at the hand-over, and not
    /// when the request is submitted: a request that waits in the
    /// backlog until its tag's first request was committed would get
    /// past a later check, and its tag be committed twice.
    fn take_over(&mut self, fetched: &[Fetched]) -> Result<(), Stopped> {
        let unguarded = self.mutants.has(Mutants::TAG_REUSED);
        let at_submit = self.mutants.has(Mutants::GUARD_AT_SUBMIT);
        for request in fetched {
            let tag = request.tag;
            let Some(slot) = self.slots.get_mut(usize::from(tag)) else {
                // A tag the queue does not have: nothing can be
                // committed for it.
                return Err(Stopped::BrokenRule { tag });
            };
            match slot {
                Slot::Idle if !at_submit => *slot = Slot::Waiting,
                Slot::Idle => {}
                _ if unguarded || at_submit => {}
                _ => return Err(Stopped::BrokenRule { tag }),
            }
        }
        Ok(())
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
            // Not reached: `take_over` let no such tag in.
            return Err(Stopped::BrokenRule { tag });
        };
        if self.mutants.has(Mutants::GUARD_AT_SUBMIT) && !matches!(slot, Slot::Idle) {
            return Err(Stopped::BrokenRule { tag });
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
                self.stamps[usize::from(tag)] = Stamp {
                    finished: Instant::now(),
                    taken: Duration::ZERO,
                    read: false,
                    ..self.stamps[usize::from(tag)]
                };
                self.finished
                    .push((tag, self.translator.refused(desc), None));
                return Ok(true);
            }
        };
        if matches!(plan.action, Action::Flush) && self.mutants.has(Mutants::FLUSH_NOT_SUBMITTED) {
            self.finish(tag, &plan, &Ok(()), None, None);
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
                        self.finish(tag, &plan, &Err(BlkError::Failed), None, None);
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
        self.timing
            .to_submit
            .add(self.stamps[usize::from(tag)].fetched.elapsed());
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
        // When the target had this completion ready. It goes with this
        // completion alone: a request that finishes without one (it
        // is refused, or needs nothing of the target) has none.
        let ready = self.target.ready_at(id);
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
                    self.finish(tag, &plan, &result, buf, ready);
                }
                Ok(())
            }
            Slot::Flushing(plan, kept) if is_flush => {
                self.finish(tag, &plan, &result, kept, ready);
                Ok(())
            }
            other if self.mutants.has(Mutants::STRAY_IGNORED) => {
                self.slots[usize::from(tag)] = other;
                Ok(())
            }
            _ => Err(Stopped::TargetFault),
        }
    }

    /// A request is finished: it waits to be committed. `ready` is
    /// when the target had the completion ready that finishes it, and
    /// `None` for a request that is finished without one.
    fn finish(
        &mut self,
        tag: u16,
        plan: &Plan,
        outcome: &Result<(), BlkError>,
        buf: Option<Buf>,
        ready: Option<Instant>,
    ) {
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
        let now = Instant::now();
        let stamp = &mut self.stamps[usize::from(tag)];
        stamp.finished = ready.unwrap_or(now);
        stamp.taken = now.saturating_duration_since(stamp.finished);
        stamp.read = matches!(plan.action, Action::Read { .. });
        self.finished
            .push((tag, self.translator.result(plan, outcome), buf));
    }

    /// Commit what is finished.
    fn commit_finished(&mut self) {
        if self.finished.is_empty() {
            return;
        }
        let now = Instant::now();
        for (tag, result, buf) in self.finished.drain(..) {
            self.slots[usize::from(tag)] = Slot::Idle;
            self.counters.in_flight = self.counters.in_flight.saturating_sub(1);
            let stamp = self.stamps[usize::from(tag)];
            let waited = now.saturating_duration_since(stamp.finished);
            if stamp.read {
                self.timing.read_to_commit.add(waited);
                self.timing.read_to_taken.add(stamp.taken);
            } else {
                self.timing.other_to_commit.add(waited);
                self.timing.other_to_taken.add(stamp.taken);
            }
            self.queue.commit(tag, result, buf);
        }
    }
}

/// The in-place store: what the kernel issues again after a lost
/// process is applied again, which rule R9 allows. It has no thread of
/// its own that could end, and it keeps no time of when a completion
/// was ready.
impl<D: skyzfs_local::io::Dir> PumpTarget for skyzfs_local::inplace::InPlace<D> {
    fn gone(&self) -> bool {
        false
    }

    fn ready_at(&self, _id: RequestId) -> Option<Instant> {
        None
    }
}

#[cfg(test)]
mod tests;

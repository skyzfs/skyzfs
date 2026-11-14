//! The pump's rules, each with the defect that shows the test would
//! notice. The queue and the target are fakes that write what happens
//! to them into one list, so that a test can ask in which order things
//! happened.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::time::Duration;

use skyzfs_blk::{Geometry, SubmitError, SubmitErrorKind};

use super::*;
use crate::translate::{flag, op, EIO};

type Outcome = Result<(), String>;

/// What happened, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    /// The target was handed a request: its id and its operation.
    Submitted(u64, &'static str),
    /// The test let the target complete a request.
    Completed(u64),
    /// The queue was given a result for a tag.
    Committed(u16, i32),
    /// The queue handed the driver the commits it held: at a wait or
    /// at a flush.
    Sent,
}

type Events = Rc<RefCell<Vec<Event>>>;

enum Step {
    Fetch(Vec<Fetched>),
    /// Commands the driver ended with a result other than OK, and
    /// nothing fetched.
    Unfetched(Vec<Unfetched>),
    /// The same, and then the queue is down.
    UnfetchedDown(Vec<Unfetched>),
    Nothing,
    Stop(Vec<Fetched>),
    Down,
}

thread_local! {
    /// How many times a fake queue on this thread saw the pump break
    /// the driver's rule (see [`FakeQueue::commit`]).
    static RULE_BROKEN: Cell<u64> = const { Cell::new(0) };
}

struct FakeQueue {
    depth: u16,
    script: VecDeque<Step>,
    events: Events,
    /// The buffers committed back, by tag.
    returned: HashMap<u16, Buf>,
    taken: u32,
    /// The queue cannot hand its commits over at a flush.
    flush_fails: bool,
    /// The tag the queue has no buffer for.
    no_buf: Option<u16>,
    /// The tags whose request was handed over and not yet committed,
    /// as the driver keeps them.
    out: HashSet<u16>,
    /// The commits that broke the driver's rule.
    broken: Vec<String>,
}

impl FakeQueue {
    /// Hand over `fetched` as the driver does: each tag is out from now
    /// until its commit. A script that hands over a tag that is still
    /// out breaks the driver's own rule, on purpose.
    fn hand_over(&mut self, fetched: &[Fetched]) {
        for request in fetched {
            self.out.insert(request.tag);
        }
    }
}

impl Queue for FakeQueue {
    fn depth(&self) -> u16 {
        self.depth
    }

    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        self.events.borrow_mut().push(Event::Sent);
        match self.script.pop_front() {
            Some(Step::Fetch(more)) => {
                self.hand_over(&more);
                fetched.extend(more);
                Ok(Wake::Work)
            }
            Some(Step::Unfetched(more)) => {
                unfetched.extend(more);
                Ok(Wake::Work)
            }
            Some(Step::Nothing) | None => Ok(Wake::Work),
            Some(Step::Stop(more)) => {
                self.hand_over(&more);
                fetched.extend(more);
                Ok(Wake::Stop)
            }
            Some(Step::Down) => Err(QueueDown),
            Some(Step::UnfetchedDown(more)) => {
                unfetched.extend(more);
                Err(QueueDown)
            }
        }
    }

    /// A write's data names its tag.
    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        if self.no_buf == Some(tag) {
            return None;
        }
        self.taken += 1;
        Some(Buf::filled(blocks, 0x40 + tag as u8))
    }

    /// The driver's rule: a commit names a tag whose request was handed
    /// over and is not yet committed. Any other commit is recorded as
    /// broken: a real driver refuses it, or, if the tag's next request
    /// was handed over meanwhile, takes it as that request's answer.
    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        if !self.out.remove(&tag) {
            self.broken
                .push(format!("tag {tag} committed with no request out"));
            RULE_BROKEN.with(|n| n.set(n.get() + 1));
        }
        self.events.borrow_mut().push(Event::Committed(tag, result));
        if let Some(buf) = buf {
            self.returned.insert(tag, buf);
        }
    }

    fn flush(&mut self) -> Result<(), QueueDown> {
        self.events.borrow_mut().push(Event::Sent);
        if self.flush_fails {
            return Err(QueueDown);
        }
        Ok(())
    }
}

struct FakeTarget {
    geometry: Geometry,
    events: Events,
    pending: Vec<Request>,
    ready: Vec<Completion>,
    gone: bool,
    /// Its worker ends in the moment of the next submit.
    goes_at_submit: bool,
    /// The worker ends when its completions are taken: after the last
    /// one it handed over.
    goes_at_poll: bool,
    /// The most requests it ever held.
    most: usize,
    /// When each request was let complete: when its completion was
    /// ready.
    stamps: HashMap<u64, Instant>,
}

impl FakeTarget {
    /// Let the request `id` complete.
    fn complete(&mut self, id: u64, result: Result<(), BlkError>) {
        let at = self
            .pending
            .iter()
            .position(|r| r.id.0 == id)
            .unwrap_or_else(|| panic!("request {id} is not in the target"));
        let request = self.pending.remove(at);
        self.events.borrow_mut().push(Event::Completed(id));
        self.stamps.insert(id, Instant::now());
        let buf = match request.op {
            Op::Read { mut buf, .. } => {
                buf.fill(0x77);
                Some(buf)
            }
            Op::Write { data, .. } => Some(data),
            _ => None,
        };
        let completion = Completion {
            id: request.id,
            result,
            buf,
        };
        self.ready.push(completion);
    }

    fn holds(&self, id: u64) -> bool {
        self.pending.iter().any(|r| r.id.0 == id)
    }
}

impl BlockTarget for FakeTarget {
    fn geometry(&self) -> Geometry {
        self.geometry
    }

    fn submit(&mut self, request: Request) -> Result<(), SubmitError> {
        let held = self.pending.len() + self.ready.len();
        self.most = self.most.max(held + 1);
        if self.goes_at_submit {
            self.gone = true;
        }
        // As the served store does: once its worker has ended it
        // refuses everything as busy.
        if self.gone || held >= self.geometry.queue_depth() as usize {
            return Err(SubmitError {
                kind: SubmitErrorKind::Busy,
                request,
            });
        }
        self.events
            .borrow_mut()
            .push(Event::Submitted(request.id.0, request.op.name()));
        self.pending.push(request);
        Ok(())
    }

    fn poll(&mut self, out: &mut Vec<Completion>) -> usize {
        let n = self.ready.len();
        if self.goes_at_poll && n > 0 {
            self.gone = true;
        }
        out.append(&mut self.ready);
        n
    }

    fn wait(&mut self, out: &mut Vec<Completion>, _: Duration) -> usize {
        self.poll(out)
    }
}

impl PumpTarget for FakeTarget {
    fn gone(&self) -> bool {
        self.gone
    }

    fn ready_at(&self, id: RequestId) -> Option<Instant> {
        self.stamps.get(&id.0).copied()
    }
}

/// Every rig holds the driver's rule: a rig without a defect planted
/// fails its test, when it is dropped, if its pump committed a tag with
/// no request out. A rig with one is judged by [`caught`].
struct Rig {
    pump: Pump<FakeQueue, FakeTarget>,
    mutants: u32,
    events: Events,
}

impl Rig {
    /// A pump over a queue of `queue_depth` tags and a target of 8 MiB
    /// that takes `target_depth` requests.
    fn new(mutants: u32, queue_depth: u16, target_depth: u32) -> Self {
        let events: Events = Rc::default();
        let queue = FakeQueue {
            depth: queue_depth,
            script: VecDeque::new(),
            events: events.clone(),
            returned: HashMap::new(),
            taken: 0,
            flush_fails: false,
            no_buf: None,
            out: HashSet::new(),
            broken: Vec::new(),
        };
        let target = FakeTarget {
            geometry: Geometry::new(8, target_depth, false).unwrap(),
            events: events.clone(),
            pending: Vec::new(),
            ready: Vec::new(),
            gone: false,
            goes_at_submit: false,
            goes_at_poll: false,
            most: 0,
            stamps: HashMap::new(),
        };
        Rig {
            pump: Pump::new(queue, target).with_mutants(mutants),
            mutants,
            events,
        }
    }

    /// Whether the pump kept the driver's rule so far.
    fn driver_rule(&mut self) -> Outcome {
        match self.pump.queue().broken.as_slice() {
            [] => Ok(()),
            broken => Err(format!("the driver's rule was broken: {broken:?}")),
        }
    }

    /// One turn in which the queue hands over `fetched`.
    fn fetch(&mut self, fetched: &[Fetched]) -> Option<Stopped> {
        self.pump
            .queue()
            .script
            .push_back(Step::Fetch(fetched.to_vec()));
        self.pump.turn()
    }

    /// One turn in which the queue has nothing.
    fn turn(&mut self) -> Option<Stopped> {
        self.pump.queue().script.push_back(Step::Nothing);
        self.pump.turn()
    }

    fn complete(&mut self, id: u64) {
        self.pump.target().complete(id, Ok(()));
    }

    fn events(&self) -> Vec<Event> {
        self.events.borrow().clone()
    }

    fn index(&self, event: &Event) -> Result<usize, String> {
        self.events()
            .iter()
            .position(|e| e == event)
            .ok_or_else(|| format!("{event:?} did not happen: {:?}", self.events()))
    }

    /// `first` happened, and before `then`.
    fn before(&self, first: &Event, then: &Event) -> Outcome {
        if self.index(first)? < self.index(then)? {
            Ok(())
        } else {
            Err(format!(
                "{first:?} is not before {then:?}: {:?}",
                self.events()
            ))
        }
    }

    fn committed(&self) -> Vec<(u16, i32)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Committed(tag, result) => Some((tag, result)),
                _ => None,
            })
            .collect()
    }

    fn submitted(&self) -> Vec<(u64, &'static str)> {
        self.events()
            .into_iter()
            .filter_map(|e| match e {
                Event::Submitted(id, name) => Some((id, name)),
                _ => None,
            })
            .collect()
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        if self.mutants == 0 && !std::thread::panicking() {
            if let Err(why) = self.driver_rule() {
                panic!("{why}");
            }
        }
    }
}

fn request(tag: u16, op: u8, flags: u32, block: u64, blocks: u32) -> Fetched {
    Fetched {
        tag,
        desc: Desc {
            op_flags: u32::from(op) | flags,
            nr_sectors: blocks * 8,
            start_sector: block * 8,
        },
    }
}

fn write(tag: u16, block: u64) -> Fetched {
    request(tag, op::WRITE, 0, block, 1)
}

fn expect<T: PartialEq + std::fmt::Debug>(got: T, want: T, what: &str) -> Outcome {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what}: expected {want:?}, got {got:?}"))
    }
}

fn holds(scenario: fn(u32) -> Outcome) {
    if let Err(why) = scenario(0) {
        panic!("{why}");
    }
}

/// The scenario holds for the pump as it is and fails with the defect,
/// or the defect breaks the driver's rule in it.
fn caught(scenario: fn(u32) -> Outcome, mutant: u32) {
    holds(scenario);
    let before = RULE_BROKEN.with(Cell::get);
    let failed = scenario(mutant).is_err();
    let broke = RULE_BROKEN.with(Cell::get) > before;
    assert!(failed || broke, "the defect {mutant:#x} was not noticed");
}

const BLOCK: i32 = 4096;

/// Order (§4.1): a request is committed to the kernel only after the
/// target completed it, so an operation acknowledged before another was
/// fetched was completed by the target before the other was submitted
/// and precedes it in the target's order. Mutant: a mutation is
/// committed when it is submitted.
#[test]
fn an_operation_acknowledged_before_another_was_fetched_precedes_it() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 4, 4);
        rig.fetch(&[write(0, 1)]);
        expect(
            rig.committed(),
            vec![],
            "committed while the target holds it",
        )?;
        rig.complete(0);
        rig.turn();
        expect(rig.committed(), vec![(0, BLOCK)], "committed")?;
        // The kernel issues the next request only now.
        rig.fetch(&[write(1, 1)]);
        rig.before(&Event::Completed(0), &Event::Committed(0, BLOCK))?;
        rig.before(&Event::Committed(0, BLOCK), &Event::Submitted(1, "write"))
    }
    caught(scenario, Mutants::ACK_AT_SUBMIT);
}

/// Order: requests fetched together are submitted in the order the
/// queue gave them, also those that had to wait for room. Mutant: the
/// newest is submitted first.
#[test]
fn requests_are_submitted_in_the_order_fetched() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 2);
        rig.fetch(&[write(3, 0), write(1, 1), write(2, 2), write(0, 3)]);
        rig.fetch(&[write(5, 4)]);
        for id in [3, 1, 2, 0, 5] {
            if rig.pump.target().holds(id) {
                rig.complete(id);
            }
            rig.turn();
            rig.turn();
        }
        for id in [0, 5] {
            if rig.pump.target().holds(id) {
                rig.complete(id);
                rig.turn();
            }
        }
        let order: Vec<u64> = rig.submitted().into_iter().map(|(id, _)| id).collect();
        expect(order, vec![3, 1, 2, 0, 5], "the order of submission")
    }
    caught(scenario, Mutants::NEWEST_FIRST);
}

/// The bound on requests in flight: the target is never handed more
/// than its depth, the rest wait in the order fetched and are submitted
/// as room is made, and every request is committed in the end. Mutant:
/// everything is submitted at once.
#[test]
fn the_target_is_never_handed_more_than_its_depth() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 2);
        let all: Vec<Fetched> = (0..6).map(|tag| write(tag, u64::from(tag))).collect();
        if let Some(stopped) = rig.fetch(&all) {
            return Err(format!("the pump ended: {stopped:?}"));
        }
        expect(rig.submitted().len(), 2, "submitted at first")?;
        expect(rig.pump.counters().in_flight, 6, "in flight")?;
        // A request that waits holds no buffer.
        expect(rig.pump.queue().taken, 2, "buffers taken")?;
        for id in 0..6 {
            rig.complete(id);
            if let Some(stopped) = rig.turn() {
                return Err(format!("the pump ended: {stopped:?}"));
            }
            let want = (id as usize + 3).min(6);
            expect(rig.submitted().len(), want, "submitted as room was made")?;
        }
        expect(rig.pump.target().most <= 2, true, "at most the depth")?;
        expect(rig.committed().len(), 6, "all committed")?;
        expect(rig.pump.counters().in_flight, 0, "in flight at the end")
    }
    caught(scenario, Mutants::UNBOUNDED);
}

/// Held requests: a request the target has not completed is neither
/// completed nor lost, however many turns pass; when the target
/// completes it, it is committed, a read with the target's data in the
/// queue's buffer.
#[test]
fn a_request_the_target_holds_is_neither_completed_nor_lost() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 4, 4);
        rig.fetch(&[
            request(2, op::READ, 0, 5, 3),
            request(1, op::FLUSH, 0, 0, 0),
        ]);
        for _ in 0..1000 {
            if let Some(stopped) = rig.turn() {
                return Err(format!("the pump ended: {stopped:?}"));
            }
        }
        expect(rig.committed(), vec![], "committed while held")?;
        expect(rig.pump.counters().in_flight, 2, "in flight")?;
        rig.complete(2);
        rig.turn();
        expect(rig.committed(), vec![(2, 3 * BLOCK)], "the read")?;
        let data = rig.pump.queue().returned.remove(&2);
        expect(
            data.map(|b| (b.len(), b.iter().all(|x| *x == 0x77))),
            Some((3 * 4096, true)),
            "the read's data",
        )?;
        rig.complete(1);
        rig.turn();
        expect(rig.committed(), vec![(2, 3 * BLOCK), (1, 0)], "the flush")
    }
    holds(scenario);
}

/// Rejected requests reach nothing (§4.1): a misaligned write, a
/// discard beyond the end and an operation outside the five are
/// committed with the plain I/O error, and the target is handed
/// nothing. Mutant: a refused request is submitted all the same.
#[test]
fn a_rejected_request_reaches_nothing() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 4, 4);
        let misaligned = Fetched {
            tag: 0,
            desc: Desc {
                op_flags: u32::from(op::WRITE),
                nr_sectors: 8,
                start_sector: 1,
            },
        };
        let beyond = request(1, op::DISCARD, 0, 2048, 1);
        let other = request(2, op::WRITE_SAME, 0, 0, 1);
        rig.fetch(&[misaligned, beyond, other]);
        rig.turn();
        expect(rig.submitted(), vec![], "handed to the target")?;
        expect(
            rig.committed(),
            vec![(0, -EIO), (1, -EIO), (2, -EIO)],
            "committed",
        )?;
        expect(rig.pump.counters().refused, 3, "counted as refused")?;
        expect(rig.pump.queue().taken, 0, "buffers taken")
    }
    caught(scenario, Mutants::REFUSED_REACHES_TARGET);
}

/// A flush is the target's flush (§4.1): it is submitted as a flush and
/// committed only when the target has completed it. Mutant: a flush is
/// committed without the target's.
#[test]
fn a_flush_is_committed_when_the_target_has_flushed() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 4, 4);
        rig.fetch(&[request(3, op::FLUSH, 0, 0, 0)]);
        rig.turn();
        expect(rig.submitted(), vec![(3, "flush")], "handed to the target")?;
        expect(
            rig.committed(),
            vec![],
            "committed before the target flushed",
        )?;
        rig.complete(3);
        rig.turn();
        expect(rig.committed(), vec![(3, 0)], "committed")?;
        expect(rig.pump.counters().flushes, 1, "counted")
    }
    caught(scenario, Mutants::FLUSH_NOT_SUBMITTED);
}

/// Forced unit access (§4.1): a write with FUA is a write followed by a
/// flush, submitted once the write was acknowledged by the target, and
/// the request is committed when the flush has completed, with the
/// write's length and its buffer. If the write fails there is no flush;
/// if the flush fails the request fails. Mutant: the request is
/// committed when its write completed.
#[test]
fn a_write_with_fua_is_acknowledged_when_its_flush_is() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 4, 4);
        rig.fetch(&[request(1, op::WRITE, flag::FUA, 7, 2)]);
        expect(rig.submitted(), vec![(1, "write")], "at first")?;
        rig.complete(1);
        rig.turn();
        expect(rig.committed(), vec![], "committed before the flush")?;
        let flush = 1 | FLUSH_ID;
        expect(
            rig.submitted(),
            vec![(1, "write"), (flush, "flush")],
            "the flush after the write was acknowledged",
        )?;
        rig.before(&Event::Completed(1), &Event::Submitted(flush, "flush"))?;
        rig.complete(flush);
        rig.turn();
        expect(rig.committed(), vec![(1, 2 * BLOCK)], "committed")?;
        expect(
            rig.pump.queue().returned.contains_key(&1),
            true,
            "the buffer came back",
        )?;
        let counters = rig.pump.counters();
        expect(
            (counters.writes, counters.flushes),
            (1, 0),
            "counted as one write",
        )?;
        // The write fails: no flush.
        rig.fetch(&[request(2, op::WRITE_ZEROES, flag::FUA, 0, 1)]);
        rig.pump.target().complete(2, Err(BlkError::Failed));
        rig.turn();
        expect(
            rig.submitted().len(),
            3,
            "no flush after a failed operation",
        )?;
        // The flush fails: the request fails.
        rig.fetch(&[request(3, op::DISCARD, flag::FUA, 0, 1)]);
        rig.complete(3);
        rig.turn();
        rig.pump
            .target()
            .complete(3 | FLUSH_ID, Err(BlkError::Failed));
        rig.turn();
        expect(
            rig.committed(),
            vec![(1, 2 * BLOCK), (2, -EIO), (3, -EIO)],
            "in the end",
        )
    }
    caught(scenario, Mutants::FUA_NOT_WAITED);
}

/// An orderly stop completes what it accepted: once the queue says
/// stop, nothing more is handed to the target, not even what arrived
/// with the stop or was waiting for room; what the target holds is
/// waited for, the flush of a request with FUA included, and committed;
/// then the pump ends. Mutant: a stop ends the pump at once.
#[test]
fn an_orderly_stop_completes_what_was_accepted() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 2);
        rig.fetch(&[
            write(0, 0),
            request(1, op::WRITE, flag::FUA, 1, 1),
            write(2, 2),
        ]);
        rig.pump
            .queue()
            .script
            .push_back(Step::Stop(vec![write(3, 3)]));
        expect(
            rig.pump.turn(),
            None,
            "the stop, with requests in the target",
        )?;
        rig.complete(0);
        rig.complete(1);
        expect(rig.turn(), None, "the flush is still to come")?;
        rig.complete(1 | FLUSH_ID);
        expect(rig.turn(), Some(Stopped::Orderly), "drained")?;
        expect(
            rig.committed(),
            vec![(0, BLOCK), (1, BLOCK)],
            "what was accepted is committed",
        )?;
        let ids: Vec<u64> = rig.submitted().into_iter().map(|(id, _)| id).collect();
        expect(ids, vec![0, 1, 1 | FLUSH_ID], "nothing new after the stop")
    }
    caught(scenario, Mutants::STOP_AT_ONCE);
}

/// A read or a write the queue has no buffer for fails with `EIO` and
/// never reaches the target: a buffer made up in its place would give
/// a write zeros for its data. The requests beside it are served.
/// Mutant: it is handed to the target with a buffer of zeros.
#[test]
fn a_request_the_queue_has_no_buffer_for_fails_and_does_not_reach_the_target() {
    fn scenario(mutants: u32) -> Outcome {
        for unbuffered in [write(1, 1), request(1, op::READ, 0, 1, 1)] {
            let mut rig = Rig::new(mutants, 8, 4);
            rig.pump.queue().no_buf = Some(1);
            rig.fetch(&[write(0, 0), unbuffered, write(2, 2)]);
            let ids: Vec<u64> = rig.submitted().into_iter().map(|(id, _)| id).collect();
            expect(ids, vec![0, 2], "only the requests with a buffer")?;
            rig.complete(0);
            rig.complete(2);
            rig.turn();
            let mut committed = rig.committed();
            committed.sort();
            expect(
                committed,
                vec![(0, BLOCK), (1, -EIO), (2, BLOCK)],
                "the one without a buffer failed",
            )?;
        }
        Ok(())
    }
    caught(scenario, Mutants::UNBUFFERED_SUBMITTED);
}

/// The last commits of a stop reach the driver: a queue keeps a commit
/// until its next wait, and after the last commit of a stop there is
/// none, so the pump has the queue hand them over before it ends. And
/// a stop whose commits could not be handed over is not called
/// orderly. Mutant: the pump ends with its last commits in the queue.
#[test]
fn the_last_commits_of_a_stop_are_handed_to_the_driver() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 2);
        rig.fetch(&[write(0, 0), write(1, 1)]);
        rig.pump.queue().script.push_back(Step::Stop(vec![]));
        expect(
            rig.pump.turn(),
            None,
            "the stop, with requests in the target",
        )?;
        rig.complete(0);
        rig.complete(1);
        expect(rig.turn(), Some(Stopped::Orderly), "drained")?;
        let events = rig.events.borrow().clone();
        let last_commit = events
            .iter()
            .rposition(|e| matches!(e, Event::Committed(..)))
            .ok_or("nothing was committed")?;
        if !events[last_commit..].contains(&Event::Sent) {
            return Err(format!(
                "the pump ended with commits still in the queue: {events:?}"
            ));
        }

        // The same stop on a queue that cannot hand them over.
        let mut rig = Rig::new(mutants, 8, 2);
        rig.pump.queue().flush_fails = true;
        rig.fetch(&[write(0, 0)]);
        rig.pump.queue().script.push_back(Step::Stop(vec![]));
        expect(rig.pump.turn(), None, "the stop")?;
        rig.complete(0);
        expect(
            rig.turn(),
            Some(Stopped::QueueDown),
            "a stop whose commits were not handed over",
        )
    }
    caught(scenario, Mutants::STOP_UNSENT);
}

/// The pump's own times. A write and a read complete together and lie
/// ready 20 ms before the pump takes them: that is each one's time to
/// the taking, and its time to the commit holds it. Nothing is stored
/// before a commit any more, so the times of the store that was there
/// until M1.3t-b stay zero. (Until then this test held a store of 30 ms
/// between the taking and the commit.)
#[test]
fn a_completion_counts_the_time_it_lay_ready_before_the_pump_took_it() {
    const LAY: std::time::Duration = std::time::Duration::from_millis(20);
    let lay = LAY.as_nanos() as u64;
    let mut rig = Rig::new(0, 8, 8);
    rig.fetch(&[write(0, 0), request(1, op::READ, 0, 0, 1)]);
    rig.complete(0);
    rig.complete(1);
    std::thread::sleep(LAY);
    rig.turn();
    assert_eq!(rig.committed().len(), 2);
    let timing = rig.pump.timing();
    assert_eq!(timing.to_submit.count, 2, "{timing:?}");
    assert_eq!(timing.store.count, 0, "{timing:?}");
    assert_eq!(timing.behind_store, 0, "{timing:?}");
    assert_eq!(timing.read_to_commit.count, 1, "{timing:?}");
    assert_eq!(timing.other_to_commit.count, 1, "{timing:?}");
    for (taken, commit) in [
        (timing.read_to_taken, timing.read_to_commit),
        (timing.other_to_taken, timing.other_to_commit),
    ] {
        assert_eq!(taken.count, 1, "{timing:?}");
        assert!(taken.sum_ns >= lay, "{timing:?}");
        assert!(commit.sum_ns >= taken.sum_ns, "{timing:?}");
    }
}

/// A completion's ready time goes with that completion alone. A write
/// with FUA completes and is not finished by it (its flush follows);
/// 20 ms later a read the queue has no buffer for fails without
/// reaching the target. That read was never ready in the target: its
/// time to the taking is zero, and not the time since the write's
/// completion.
#[test]
fn a_request_finished_without_a_completion_has_no_other_requests_ready_time() {
    let mut rig = Rig::new(0, 8, 8);
    rig.fetch(&[request(1, op::WRITE, flag::FUA, 1, 1)]);
    rig.complete(1);
    rig.turn();
    assert_eq!(rig.committed().len(), 0, "the write waits for its flush");
    std::thread::sleep(std::time::Duration::from_millis(20));
    rig.pump.queue().no_buf = Some(2);
    let began = Instant::now();
    rig.fetch(&[request(2, op::READ, 0, 0, 1)]);
    rig.turn();
    let within = began.elapsed().as_nanos() as u64;
    assert_eq!(rig.committed(), vec![(2, -EIO)]);
    let timing = rig.pump.timing();
    assert_eq!(timing.read_to_commit.count, 1, "{timing:?}");
    assert_eq!(timing.read_to_taken.count, 1, "{timing:?}");
    assert_eq!(timing.read_to_taken.sum_ns, 0, "{timing:?}");
    // Finished and committed inside these two turns of the test.
    assert!(timing.read_to_commit.sum_ns <= within, "{timing:?}");
}

/// A target that is gone ends the pump, and nothing is committed after
/// that, not even what it had completed: the kernel issues the requests
/// again to the next instance. Mutant: the pump does not notice.
#[test]
fn a_target_that_is_gone_ends_the_pump_and_commits_nothing() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.fetch(&[write(0, 0), write(1, 1)]);
        rig.complete(0);
        rig.pump.target().gone = true;
        expect(rig.turn(), Some(Stopped::TargetGone), "the turn")?;
        expect(
            rig.committed(),
            vec![],
            "committed after the target was gone",
        )
    }
    caught(scenario, Mutants::GONE_UNNOTICED);
}

/// A target that is gone when a request is waiting is still a target
/// that is gone, not one that broke the contract: the pump looks
/// before it submits. Mutant: it looks only afterwards, and takes the
/// refusal for a fault.
#[test]
fn a_target_that_is_gone_is_gone_also_when_a_request_waits() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.pump.target().gone = true;
        expect(
            rig.fetch(&[write(0, 0)]),
            Some(Stopped::TargetGone),
            "a turn that fetched a request",
        )?;
        expect(rig.submitted(), vec![], "nothing reached the target")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(scenario, Mutants::GONE_AFTER_SUBMIT);
}

/// The window between the pump's look and its submit: the worker ends
/// in that moment, and the refusal is that of a target that is gone.
/// Mutant: a refusal with room is always a fault.
#[test]
fn a_target_that_goes_in_the_moment_of_a_submit_is_gone() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.pump.target().goes_at_submit = true;
        expect(
            rig.fetch(&[write(0, 0)]),
            Some(Stopped::TargetGone),
            "the worker ended as the request was submitted",
        )?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(scenario, Mutants::REFUSAL_IS_FAULT);
}

/// Two guards against the other side. A queue that hands over a tag
/// that is still in flight is not served further: nothing can be
/// committed for a tag twice. And a completion nobody waits for means
/// the target is not keeping the contract. Mutants: each guard gone.
#[test]
fn a_tag_in_flight_is_not_taken_again_and_a_stray_completion_is_a_fault() {
    fn reused(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        expect(rig.fetch(&[write(0, 0)]), None, "the first")?;
        expect(
            rig.fetch(&[write(0, 5)]),
            Some(Stopped::BrokenRule { tag: 0 }),
            "the same tag again",
        )?;
        expect(rig.submitted().len(), 1, "the second reached nothing")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(reused, Mutants::TAG_REUSED);

    fn stray(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        expect(rig.fetch(&[write(0, 0)]), None, "a request")?;
        let nobodys = Completion {
            id: RequestId(5),
            result: Ok(()),
            buf: None,
        };
        rig.pump.target().ready.push(nobodys);
        expect(rig.turn(), Some(Stopped::TargetFault), "a stray completion")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(stray, Mutants::STRAY_IGNORED);
}

/// One commit per request handed over (M1.3ac): a tag the driver hands
/// over again while its request is still out ends the pump at the
/// hand-over, before anything of that wait is submitted or committed.
/// Here the second request on tag 0 would wait in the backlog behind
/// tag 1's until tag 0's first request was committed; a guard that
/// looked only when a request is submitted lets it through then, and
/// tag 0 is committed twice, which the fake queue's driver rule
/// records. Defects: the guard at the submit (as before M1.3ac), and
/// no guard.
#[test]
fn a_tag_handed_over_again_while_its_request_is_out_is_not_taken() {
    fn rehanded(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 1);
        expect(rig.fetch(&[write(0, 0)]), None, "tag 0")?;
        expect(rig.fetch(&[write(1, 1)]), None, "tag 1, which waits")?;
        // The driver breaks its rule: tag 0 again, while its first
        // request is in the target.
        let stopped = rig.fetch(&[write(0, 5)]);
        if stopped.is_none() {
            // A pump that took it: let everything end.
            for id in [0, 1, 0] {
                rig.complete(id);
                rig.turn();
            }
        }
        rig.driver_rule()?;
        expect(
            stopped,
            Some(Stopped::BrokenRule { tag: 0 }),
            "tag 0 handed over again",
        )?;
        expect(rig.submitted().len(), 1, "only tag 0's first request")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(rehanded, Mutants::GUARD_AT_SUBMIT);
    caught(rehanded, Mutants::TAG_REUSED);
    // What notices the guard at the submit is the driver's rule.
    let before = RULE_BROKEN.with(Cell::get);
    let _ = rehanded(Mutants::GUARD_AT_SUBMIT);
    assert!(
        RULE_BROKEN.with(Cell::get) > before,
        "the double commit was not recorded"
    );

    /// A tag twice in one wait.
    fn twice(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        let stopped = rig.fetch(&[write(2, 0), write(2, 8)]);
        expect(stopped, Some(Stopped::BrokenRule { tag: 2 }), "tag 2 twice")?;
        expect(rig.submitted(), vec![], "nothing reached the target")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(twice, Mutants::TAG_REUSED);

    /// A tag twice in one wait after the failure: answered once would
    /// still be twice.
    fn failed(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        expect(rig.pump.fail(), 0, "nothing was held")?;
        let stopped = rig.fetch(&[write(3, 0), write(3, 8)]);
        rig.driver_rule()?;
        expect(stopped, Some(Stopped::BrokenRule { tag: 3 }), "tag 3 twice")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(failed, Mutants::TAG_REUSED);
}

/// A tag the queue does not have breaks the driver's rule as a tag
/// still out does: the pump ends at the hand-over with the tag named,
/// and nothing of that wait reaches the target or is committed, not
/// even the request on a good tag beside it. Distinct from a queue
/// that went away (below), which is no breach.
#[test]
fn a_tag_the_queue_does_not_have_ends_the_pump_with_the_rule_broken() {
    let mut rig = Rig::new(0, 4, 4);
    assert_eq!(
        rig.fetch(&[write(1, 0), write(4, 8)]),
        Some(Stopped::BrokenRule { tag: 4 })
    );
    assert!(rig.submitted().is_empty());
    assert!(rig.committed().is_empty());
}

/// When the queue goes away the pump ends and says so.
#[test]
fn a_queue_that_is_down_ends_the_pump() {
    let mut rig = Rig::new(0, 8, 8);
    rig.fetch(&[write(0, 0)]);
    rig.pump.queue().script.push_back(Step::Down);
    assert_eq!(rig.pump.run(), Stopped::QueueDown);
    assert!(rig.committed().is_empty());
}

/// A write hands the target the queue's buffer with the request's data,
/// at the request's offset and length, and the buffer goes back to the
/// queue when the request is committed, also when it failed. The
/// counters name each operation once.
#[test]
fn a_write_carries_the_queues_buffer_there_and_back() {
    let mut rig = Rig::new(0, 8, 8);
    rig.fetch(&[
        request(5, op::WRITE, 0, 9, 4),
        request(6, op::WRITE, 0, 1, 1),
        request(7, op::WRITE_ZEROES, flag::NOUNMAP, 0, 2048),
    ]);
    let seen: Vec<_> = rig
        .pump
        .target()
        .pending
        .iter()
        .map(|r| (r.id.0, r.op.range(), r.op.name()))
        .collect();
    assert_eq!(
        seen,
        [
            (5, Some((9 * 4096, 4 * 4096)), "write"),
            (6, Some((4096, 4096)), "write"),
            (7, Some((0, 8 << 20)), "write_zeroes"),
        ]
    );
    let Op::Write { data, .. } = &rig.pump.target().pending[0].op else {
        panic!("not a write");
    };
    assert!(data.iter().all(|b| *b == 0x45));
    rig.complete(5);
    rig.pump.target().complete(6, Err(BlkError::ReadOnly));
    rig.complete(7);
    rig.turn();
    assert_eq!(rig.committed(), [(5, 4 * BLOCK), (6, -EIO), (7, 0)]);
    let returned = &rig.pump.queue().returned;
    assert_eq!(returned[&5].len(), 4 * 4096);
    assert_eq!(returned[&6].len(), 4096);
    let counters = rig.pump.counters();
    assert_eq!(
        (counters.writes, counters.write_zeroes, counters.in_flight),
        (2, 1, 0)
    );
}

/// A pump with four requests held in four of the five ways a request
/// can be held: tag 0 a write in the target, tag 1 a write with FUA
/// whose flush is in the target, tag 3 a read in the target, tag 4 a
/// flush that waits for room. The target answers none of what it still
/// has. The fifth way, a write with FUA whose flush is not submitted
/// yet, lasts beyond a turn only when the pump ends in it:
/// `a_failed_volume_answers_a_write_whose_flush_was_never_submitted`.
/// (Until M1.3t-b there was a sixth, a completion that waited for its
/// position to be stored, here tag 2.)
fn held_in_every_way(mutants: u32) -> Result<Rig, String> {
    let mut rig = Rig::new(mutants, 8, 3);
    rig.fetch(&[
        write(0, 0),
        request(1, op::WRITE, flag::FUA, 1, 1),
        request(3, op::READ, 0, 3, 1),
        request(4, op::FLUSH, 0, 0, 0),
    ]);
    rig.complete(1);
    rig.turn();
    expect(rig.committed(), vec![], "committed before the failure")?;
    expect(rig.pump.counters().in_flight, 4, "held before the failure")?;
    expect(
        rig.submitted(),
        vec![
            (0, "write"),
            (1, "write"),
            (3, "read"),
            (1 | FLUSH_ID, "flush"),
        ],
        "in the target before the failure",
    )?;
    Ok(rig)
}

/// The sixth way a request is held: a write with FUA that the target
/// completed and whose flush is not submitted yet. A turn submits that
/// flush as soon as the write completes, so the state outlasts a turn
/// only when the turn ends the pump in between: here the target's
/// worker ends right after it handed the write's completion over. The
/// daemon tells the pump of the failure after such a turn too, and the
/// write is answered like the rest, its buffer given back. Mutant: the
/// failure skips a write that waits for its flush.
#[test]
fn a_failed_volume_answers_a_write_whose_flush_was_never_submitted() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 3);
        rig.fetch(&[request(1, op::WRITE, flag::FUA, 1, 1), write(0, 0)]);
        rig.complete(1);
        rig.pump.target().goes_at_poll = true;
        expect(rig.turn(), Some(Stopped::TargetGone), "the turn")?;
        expect(
            rig.submitted(),
            vec![(1, "write"), (0, "write")],
            "in the target: no flush",
        )?;
        expect(rig.committed(), vec![], "committed before the failure")?;
        expect(rig.pump.fail(), 2, "answered at the failure")?;
        let mut committed = rig.committed();
        committed.sort_unstable();
        expect(
            committed,
            vec![(0, -EIO), (1, -EIO)],
            "committed at the failure",
        )?;
        let back: Vec<u16> = rig.pump.queue().returned.keys().copied().collect();
        expect(back, vec![1], "the buffer of the write that waited")?;
        expect(rig.pump.counters().in_flight, 0, "held after the failure")
    }
    caught(scenario, Mutants::FAILED_SKIPS_FLUSH_WAITERS);
}

/// A failed volume holds no request (M1.3d): when the pump is told
/// that the volume failed, every request it holds is committed with
/// the plain I/O error, once, wherever it was: in the target and never
/// answered, behind its flush, or not yet submitted. The target is not
/// waited for: it answers nothing in this test, ever. Mutant: the
/// failure answers nothing.
#[test]
fn a_failed_volume_answers_every_request_it_holds() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = held_in_every_way(mutants)?;
        expect(rig.pump.fail(), 4, "answered at the failure")?;
        let mut committed = rig.committed();
        committed.sort_unstable();
        expect(
            committed,
            [0, 1, 3, 4].into_iter().map(|tag| (tag, -EIO)).collect(),
            "committed at the failure",
        )?;
        expect(rig.pump.counters().in_flight, 0, "held after the failure")?;
        // The buffers the pump had go back to the queue with the
        // commit; those in the target stay there.
        let back: Vec<u16> = rig.pump.queue().returned.keys().copied().collect();
        expect(back, vec![1], "buffers given back")?;
        // The target still answers nothing: nothing more happens, and
        // the pump does not end.
        for _ in 0..100 {
            if let Some(stopped) = rig.turn() {
                return Err(format!("the pump ended: {stopped:?}"));
            }
        }
        expect(rig.committed().len(), 4, "commits in all")?;
        expect(rig.pump.fail(), 0, "a second failure")?;
        expect(rig.pump.failed_answers(), 4, "answered because it failed")?;
        expect(rig.committed().len(), 4, "commits after a second failure")
    }
    caught(scenario, Mutants::FAILED_HOLDS);
}

/// After the failure the target is handed nothing more and nothing is
/// taken from it: a new request is answered with the plain I/O error
/// at once, also on a tag that was held at the failure, and what the
/// target completes late is not committed (its tag has been answered
/// and may be another request's by now).
/// Mutant: the pump answers what it held and then serves on.
#[test]
fn a_failed_volume_hands_the_target_nothing_and_takes_nothing_from_it() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = held_in_every_way(mutants)?;
        rig.pump.fail();
        let before = rig.submitted().len();
        if let Some(stopped) = rig.fetch(&[write(0, 7), request(5, op::FLUSH, 0, 0, 0)]) {
            return Err(format!("the pump ended: {stopped:?}"));
        }
        expect(
            rig.committed()[4..].to_vec(),
            vec![(0, -EIO), (5, -EIO)],
            "new requests after the failure",
        )?;
        expect(rig.submitted().len(), before, "handed to the target")?;
        // The write that held tag 0 at the failure completes now.
        rig.complete(0);
        rig.complete(3);
        for _ in 0..10 {
            if let Some(stopped) = rig.turn() {
                return Err(format!("the pump ended: {stopped:?}"));
            }
        }
        expect(rig.committed().len(), 6, "commits after a late completion")?;
        expect(rig.pump.failed_answers(), 6, "answered because it failed")?;
        // A target whose worker has ended is no reason to end either:
        // the failed pump does not look at it.
        rig.pump.target().gone = true;
        expect(rig.turn(), None, "with the target gone")
    }
    caught(scenario, Mutants::FAILED_SERVES);
}

/// A failed pump ends when the queue goes (the delete) or at a stop.
/// At a stop it answers what arrived with it and hands its commits to
/// the driver: no wait follows that would.
#[test]
fn a_failed_pump_ends_with_the_queue_or_at_a_stop() {
    let mut rig = held_in_every_way(0).expect("the setup");
    rig.pump.fail();
    rig.pump
        .queue()
        .script
        .push_back(Step::Stop(vec![write(6, 1)]));
    assert_eq!(rig.pump.turn(), Some(Stopped::Failed));
    let events = rig.events();
    assert_eq!(
        &events[events.len() - 2..],
        [Event::Committed(6, -EIO), Event::Sent]
    );

    let mut rig = held_in_every_way(0).expect("the setup");
    rig.pump.fail();
    rig.pump.queue().script.push_back(Step::Down);
    assert_eq!(rig.pump.turn(), Some(Stopped::QueueDown));

    // With nothing held there is nothing to answer, and the pump goes
    // on answering what arrives.
    let mut rig = Rig::new(0, 4, 4);
    assert_eq!(rig.pump.fail(), 0);
    assert!(rig.pump.is_failed());
    assert_eq!(rig.fetch(&[request(2, op::READ, 0, 0, 1)]), None);
    assert_eq!(rig.committed(), [(2, -EIO)]);
    assert_eq!(rig.pump.queue().taken, 0, "no buffer is taken");
}

fn unfetched(tag: u16, command: Command, result: i32) -> Unfetched {
    Unfetched {
        tag,
        command,
        result,
    }
}

/// Commands the driver ended with a result other than OK fetched
/// nothing, and the pump counts them: aborts apart from the others,
/// and the result of the last of those. Nothing else follows from
/// them: no commit, no request, no change to what is in flight. (Until
/// M1.3aa the queue dropped them, and nothing showed them.)
#[test]
fn commands_the_driver_ended_otherwise_than_ok_are_counted() {
    let mut rig = Rig::new(0, 4, 4);
    assert_eq!(rig.fetch(&[request(2, op::READ, 0, 0, 1)]), None);
    let before = rig.committed();
    rig.pump.queue().script.push_back(Step::Unfetched(vec![
        unfetched(1, Command::Commit, -22),
        unfetched(3, Command::Fetch, ABORTED),
        unfetched(0, Command::Commit, -16),
        unfetched(3, Command::Other(0x23), 1),
        unfetched(0, Command::Commit, ABORTED),
    ]));
    assert_eq!(rig.pump.turn(), None);
    let c = rig.pump.counters();
    assert_eq!(c.commands_aborted, 2, "aborted");
    assert_eq!(c.commands_failed, 3, "failed");
    assert_eq!(c.last_command_failure, 1, "the last failure");
    assert_eq!(c.in_flight, 1, "in flight");
    assert_eq!(rig.committed(), before, "nothing committed for them");
    // A pump whose volume failed counts them too.
    let mut rig = Rig::new(0, 4, 4);
    rig.pump.fail();
    rig.pump
        .queue()
        .script
        .push_back(Step::Unfetched(vec![unfetched(1, Command::Commit, -5)]));
    assert_eq!(rig.pump.turn(), None);
    let c = rig.pump.counters();
    assert_eq!((c.commands_failed, c.last_command_failure), (1, -5));
    assert!(rig.committed().is_empty());
}

/// A wait that ends with the queue down still hands on what the driver
/// ended before: the pump counts it before it stops, in its normal turn
/// and in the turn of a failed volume.
#[test]
fn commands_ended_by_a_wait_that_ends_down_are_counted() {
    for failed in [false, true] {
        let mut rig = Rig::new(0, 4, 4);
        if failed {
            rig.pump.fail();
        }
        rig.pump.queue().script.push_back(Step::UnfetchedDown(vec![
            unfetched(0, Command::Commit, ABORTED),
            unfetched(1, Command::Commit, -22),
        ]));
        assert_eq!(rig.pump.turn(), Some(Stopped::QueueDown), "failed {failed}");
        let c = rig.pump.counters();
        assert_eq!(
            (
                c.commands_aborted,
                c.commands_failed,
                c.last_command_failure
            ),
            (1, 1, -22),
            "failed {failed}"
        );
    }
}

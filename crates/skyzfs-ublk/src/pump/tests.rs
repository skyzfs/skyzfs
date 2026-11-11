//! The pump's rules, each with the defect that shows the test would
//! notice. The queue, the target and the record of what was
//! acknowledged are fakes that write what happens to them into one
//! list, so that a test can ask in which order things happened.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
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
    /// A position was stored.
    Stored(u64),
    /// The queue was given a result for a tag.
    Committed(u16, i32),
}

type Events = Rc<RefCell<Vec<Event>>>;

enum Step {
    Fetch(Vec<Fetched>),
    Nothing,
    Stop(Vec<Fetched>),
    Down,
}

struct FakeQueue {
    depth: u16,
    script: VecDeque<Step>,
    events: Events,
    /// The buffers committed back, by tag.
    returned: HashMap<u16, Buf>,
    taken: u32,
    /// Pauses so far.
    pauses: u32,
    /// The record can be written again after this many pauses.
    heals: Option<(u32, Rc<Cell<bool>>)>,
}

impl Queue for FakeQueue {
    fn depth(&self) -> u16 {
        self.depth
    }

    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
        match self.script.pop_front() {
            Some(Step::Fetch(more)) => {
                fetched.extend(more);
                Ok(Wake::Work)
            }
            Some(Step::Nothing) | None => Ok(Wake::Work),
            Some(Step::Stop(more)) => {
                fetched.extend(more);
                Ok(Wake::Stop)
            }
            Some(Step::Down) => Err(QueueDown),
        }
    }

    /// A write's data names its tag.
    fn take_buf(&mut self, tag: u16, blocks: usize) -> Buf {
        self.taken += 1;
        Buf::filled(blocks, 0x40 + tag as u8)
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        self.events.borrow_mut().push(Event::Committed(tag, result));
        if let Some(buf) = buf {
            self.returned.insert(tag, buf);
        }
    }

    fn pause(&mut self) {
        self.pauses += 1;
        if let Some((after, fails)) = &self.heals {
            if self.pauses >= *after {
                fails.set(false);
            }
        }
    }
}

struct FakeTarget {
    geometry: Geometry,
    events: Events,
    pending: Vec<Request>,
    ready: Vec<(Completion, Option<u64>)>,
    acknowledged: Option<u64>,
    gone: bool,
    /// The most requests it ever held.
    most: usize,
}

impl FakeTarget {
    /// Let the request `id` complete. `position` is where its record
    /// starts, for a mutation that succeeded.
    fn complete(&mut self, id: u64, result: Result<(), BlkError>, position: Option<u64>) {
        let at = self
            .pending
            .iter()
            .position(|r| r.id.0 == id)
            .unwrap_or_else(|| panic!("request {id} is not in the target"));
        let request = self.pending.remove(at);
        self.events.borrow_mut().push(Event::Completed(id));
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
        self.ready.push((completion, position));
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
        for (completion, position) in self.ready.drain(..) {
            if let Some(position) = position {
                self.acknowledged = self.acknowledged.max(Some(position));
            }
            out.push(completion);
        }
        n
    }

    fn wait(&mut self, out: &mut Vec<Completion>, _: Duration) -> usize {
        self.poll(out)
    }
}

impl Positions for FakeTarget {
    fn acknowledged(&self) -> Option<u64> {
        self.acknowledged
    }

    fn gone(&self) -> bool {
        self.gone
    }
}

struct FakeAck {
    events: Events,
    fail: Rc<Cell<bool>>,
}

impl AckRecord for FakeAck {
    fn advance(&mut self, position: u64) -> io::Result<()> {
        if self.fail.get() {
            return Err(io::Error::other("the record cannot be written"));
        }
        self.events.borrow_mut().push(Event::Stored(position));
        Ok(())
    }
}

struct Rig {
    pump: Pump<FakeQueue, FakeTarget, FakeAck>,
    events: Events,
    ack_fails: Rc<Cell<bool>>,
}

impl Rig {
    /// A pump over a queue of `queue_depth` tags and a target of 8 MiB
    /// that takes `target_depth` requests.
    fn new(mutants: u32, queue_depth: u16, target_depth: u32) -> Self {
        let events: Events = Rc::default();
        let ack_fails = Rc::new(Cell::new(false));
        let queue = FakeQueue {
            depth: queue_depth,
            script: VecDeque::new(),
            events: events.clone(),
            returned: HashMap::new(),
            taken: 0,
            pauses: 0,
            heals: None,
        };
        let target = FakeTarget {
            geometry: Geometry::new(8, target_depth, false).unwrap(),
            events: events.clone(),
            pending: Vec::new(),
            ready: Vec::new(),
            acknowledged: None,
            gone: false,
            most: 0,
        };
        let ack = FakeAck {
            events: events.clone(),
            fail: ack_fails.clone(),
        };
        Rig {
            pump: Pump::new(queue, target, ack).with_mutants(mutants),
            events,
            ack_fails,
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

    fn complete(&mut self, id: u64, position: Option<u64>) {
        self.pump.target().complete(id, Ok(()), position);
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

/// The scenario holds for the pump as it is and fails with the defect.
fn caught(scenario: fn(u32) -> Outcome, mutant: u32) {
    holds(scenario);
    assert!(
        scenario(mutant).is_err(),
        "the defect {mutant:#x} was not noticed"
    );
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
        rig.complete(0, Some(64));
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
                rig.complete(id, Some(id * 100));
            }
            rig.turn();
            rig.turn();
        }
        for id in [0, 5] {
            if rig.pump.target().holds(id) {
                rig.complete(id, Some(1000));
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
            rig.complete(id, Some(id * 100 + 100));
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
        rig.complete(2, None);
        rig.turn();
        expect(rig.committed(), vec![(2, 3 * BLOCK)], "the read")?;
        let data = rig.pump.queue().returned.remove(&2);
        expect(
            data.map(|b| (b.len(), b.iter().all(|x| *x == 0x77))),
            Some((3 * 4096, true)),
            "the read's data",
        )?;
        rig.complete(1, None);
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
        rig.complete(3, None);
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
        rig.complete(1, Some(300));
        rig.turn();
        expect(rig.committed(), vec![], "committed before the flush")?;
        let flush = 1 | FLUSH_ID;
        expect(
            rig.submitted(),
            vec![(1, "write"), (flush, "flush")],
            "the flush after the write was acknowledged",
        )?;
        rig.before(&Event::Completed(1), &Event::Submitted(flush, "flush"))?;
        rig.complete(flush, None);
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
        rig.pump.target().complete(2, Err(BlkError::Failed), None);
        rig.turn();
        expect(
            rig.submitted().len(),
            3,
            "no flush after a failed operation",
        )?;
        // The flush fails: the request fails.
        rig.fetch(&[request(3, op::DISCARD, flag::FUA, 0, 1)]);
        rig.complete(3, Some(400));
        rig.turn();
        rig.pump
            .target()
            .complete(3 | FLUSH_ID, Err(BlkError::Failed), None);
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
        rig.complete(0, Some(100));
        rig.complete(1, Some(200));
        expect(rig.turn(), None, "the flush is still to come")?;
        rig.complete(1 | FLUSH_ID, None);
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

/// A stop while the position cannot be stored, and then can: the pump
/// tries again by itself, paced by the queue's pause and with no
/// further wait on the queue, stores the position, commits everything
/// and only then is the stop orderly. Mutant: the stop is called
/// orderly with completions waiting.
#[test]
fn a_stop_tries_again_to_store_the_position_and_is_orderly_only_when_all_is_committed() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.fetch(&[write(0, 0), request(1, op::READ, 0, 0, 1)]);
        rig.ack_fails.set(true);
        rig.pump.queue().heals = Some((3, rig.ack_fails.clone()));
        rig.complete(0, Some(4096));
        rig.complete(1, None);
        rig.pump.queue().script.push_back(Step::Stop(vec![]));
        // One turn: the script holds nothing more, so a pump that went
        // back to the queue's wait would not get there.
        expect(rig.pump.turn(), Some(Stopped::Orderly), "the stop")?;
        expect(rig.pump.queue().pauses, 3, "a pause before each try")?;
        rig.before(&Event::Stored(4096), &Event::Committed(0, BLOCK))?;
        expect(rig.committed().len(), 2, "everything is committed")?;
        expect(rig.pump.counters().unstored, 0, "none waiting")?;
        expect(rig.pump.counters().in_flight, 0, "none in flight")
    }
    caught(scenario, Mutants::ORDERLY_UNSTORED);
}

/// A stop while the position can never be stored ends, after the
/// stated number of tries, with a result of its own. Nothing is
/// committed, so nothing is lost: the kernel has every request still
/// and issues it again to the next instance.
#[test]
fn a_stop_that_can_never_store_the_position_ends_and_commits_nothing() {
    let mut rig = Rig::new(0, 8, 8);
    rig.pump.stop_tries = 7;
    rig.fetch(&[write(0, 0), write(1, 1), request(2, op::READ, 0, 0, 1)]);
    rig.ack_fails.set(true);
    rig.complete(0, Some(4096));
    rig.complete(1, Some(8192));
    rig.complete(2, None);
    rig.pump.queue().script.push_back(Step::Stop(vec![]));
    assert_eq!(rig.pump.turn(), Some(Stopped::Unstored { left: 3 }));
    assert_eq!(rig.pump.queue().pauses, 7, "the stated number of tries");
    assert!(rig.committed().is_empty(), "nothing is committed");
    assert!(
        !rig.events().iter().any(|e| matches!(e, Event::Stored(_))),
        "nothing is stored"
    );
    assert_eq!(rig.pump.counters().unstored, 3);
    assert_eq!(rig.pump.counters().in_flight, 3, "left for a reissue");

    // The default: twenty tries.
    let mut rig = Rig::new(0, 8, 8);
    rig.fetch(&[write(0, 0)]);
    rig.ack_fails.set(true);
    rig.complete(0, Some(4096));
    rig.pump.queue().script.push_back(Step::Stop(vec![]));
    assert_eq!(rig.pump.turn(), Some(Stopped::Unstored { left: 1 }));
    assert_eq!(rig.pump.queue().pauses, 20);
}

/// The other endings leave completions that waited uncommitted.
#[test]
fn every_ending_but_the_orderly_one_commits_nothing_that_waited() {
    for ending in [Stopped::QueueDown, Stopped::TargetGone] {
        let mut rig = Rig::new(0, 8, 8);
        rig.fetch(&[write(0, 0), write(1, 1)]);
        rig.ack_fails.set(true);
        rig.complete(0, Some(4096));
        assert_eq!(rig.turn(), None);
        assert_eq!(rig.pump.counters().unstored, 1);
        // The record would take the position now: it is not asked.
        rig.ack_fails.set(false);
        match ending {
            Stopped::QueueDown => rig.pump.queue().script.push_back(Step::Down),
            _ => rig.pump.target().gone = true,
        }
        assert_eq!(rig.pump.turn(), Some(ending));
        assert!(rig.committed().is_empty(), "{ending:?}");
    }
}

/// What was acknowledged (§4.2): the position of the newest
/// acknowledged record is stored before the completions it covers are
/// committed, once for a batch; a batch that acknowledges no mutation
/// stores nothing; and the stored position never moves back. Mutant:
/// the position is stored after the completions are committed.
#[test]
fn the_acknowledged_position_is_stored_before_the_completions_are_committed() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.fetch(&[write(0, 0), write(1, 1), request(2, op::READ, 0, 0, 1)]);
        rig.complete(0, Some(4096));
        rig.complete(1, Some(8192));
        rig.turn();
        rig.before(&Event::Stored(8192), &Event::Committed(0, BLOCK))?;
        rig.before(&Event::Stored(8192), &Event::Committed(1, BLOCK))?;
        let stored = |rig: &Rig| -> Vec<u64> {
            rig.events()
                .into_iter()
                .filter_map(|e| match e {
                    Event::Stored(p) => Some(p),
                    _ => None,
                })
                .collect()
        };
        expect(stored(&rig), vec![8192], "one store for the batch")?;
        // A read alone: nothing new to store.
        rig.complete(2, None);
        rig.turn();
        expect(stored(&rig), vec![8192], "after a read")?;
        expect(rig.committed().len(), 3, "the read is committed")?;
        // A discard whose record lies further on.
        rig.fetch(&[request(3, op::DISCARD, 0, 0, 1)]);
        rig.complete(3, Some(12288));
        rig.turn();
        rig.before(&Event::Stored(12288), &Event::Committed(3, 0))?;
        // A target that reported a lower position would not move it back.
        rig.pump.target().acknowledged = Some(100);
        rig.fetch(&[write(4, 0)]);
        rig.complete(4, None);
        rig.turn();
        expect(stored(&rig), vec![8192, 12288], "never back")
    }
    caught(scenario, Mutants::STORE_AFTER_COMMIT);
}

/// What was acknowledged (§4.2): if the position cannot be stored, the
/// completions are not committed. The requests are held, and committed
/// once the store succeeds at a later turn. Mutant: they are committed
/// although the position was not stored.
#[test]
fn a_position_that_cannot_be_stored_holds_the_completions() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.ack_fails.set(true);
        rig.fetch(&[write(0, 0), request(1, op::READ, 0, 0, 1)]);
        rig.complete(0, Some(4096));
        rig.complete(1, None);
        for _ in 0..5 {
            if let Some(stopped) = rig.turn() {
                return Err(format!("the pump ended: {stopped:?}"));
            }
        }
        expect(rig.committed(), vec![], "committed although not stored")?;
        expect(rig.pump.counters().unstored, 2, "counted as waiting")?;
        expect(rig.pump.counters().in_flight, 2, "still in flight")?;
        rig.ack_fails.set(false);
        rig.turn();
        rig.before(&Event::Stored(4096), &Event::Committed(0, BLOCK))?;
        expect(rig.committed().len(), 2, "committed once stored")?;
        expect(rig.pump.counters().unstored, 0, "none waiting")
    }
    caught(scenario, Mutants::COMMIT_UNSTORED);
}

/// A target that is gone ends the pump, and nothing is committed after
/// that, not even what it had completed: the kernel issues the requests
/// again to the next instance. Mutant: the pump does not notice.
#[test]
fn a_target_that_is_gone_ends_the_pump_and_commits_nothing() {
    fn scenario(mutants: u32) -> Outcome {
        let mut rig = Rig::new(mutants, 8, 8);
        rig.fetch(&[write(0, 0), write(1, 1)]);
        rig.complete(0, Some(4096));
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
            Some(Stopped::QueueDown),
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
        rig.pump.target().ready.push((nobodys, None));
        expect(rig.turn(), Some(Stopped::TargetFault), "a stray completion")?;
        expect(rig.committed(), vec![], "nothing is committed")
    }
    caught(stray, Mutants::STRAY_IGNORED);
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
    rig.complete(5, Some(64));
    rig.pump.target().complete(6, Err(BlkError::ReadOnly), None);
    rig.complete(7, Some(128));
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

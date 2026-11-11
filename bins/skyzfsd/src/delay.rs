//! The test build's delays between a request's completion and its
//! commit, as a queue around the queue: `serve --commit-delay-ms N`
//! for every request, and `serve --hold-one-request` for one.
//!
//! # Every request, by N ms
//!
//! For every request, the result is given to the kernel N ms after the
//! pump committed it. Nothing else waits: the store has completed the
//! request and the queue goes on fetching, so that many requests are
//! applied and uncommitted at once. A `kill -9` then finds requests
//! the daemon has applied and not answered: the case of "applied a
//! second time, as a new operation in the order" (design §4.1).
//!
//! The acknowledged position waits with the result: it is stored when
//! the result is given to the kernel, not when the pump commits. The
//! pump is given a record ([`LateAck`]) that only remembers the
//! position, and the queue stores it in the real record just before it
//! gives a result on. Otherwise the record would already cover what a
//! kill leaves unanswered, and the next start could not count it
//! (`recovered_unacknowledged`): the first run of the rig's kill test
//! with the delay counted nothing for that reason. If the position
//! cannot be stored the result stays back and is tried again at the
//! next wait, as the pump does without a delay.
//!
//! # One request, for seconds
//!
//! The first request that is fetched and is of the kind asked for (a
//! read or a write that covers a given byte, or a flush) has its
//! result kept back for the time asked for, minutes if need be. Every
//! other request goes on. To the kernel that is a request the server
//! holds: it must wait and must not fail it. (The store has completed
//! the request; what is held is the answer.)
//!
//! # Common to both
//!
//! Once a stop is asked nothing is delayed any more, and what was
//! waiting is committed at once: an orderly stop still answers all it
//! accepted.
//!
//! In a default build neither is ever set (the command line refuses
//! the options) and every commit passes straight through.

use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use skyzfs_blk::Buf;
use std::sync::Mutex;

use skyzfs_ublk::pump::{AckRecord, Fetched, Queue, QueueDown, Wake};
use skyzfs_ublk::translate::{op, Desc, SECTOR};

use crate::cli::{Held, Hold};

/// Whether `desc` is the request `what` waits for.
fn is_held(what: Held, desc: &Desc) -> bool {
    let covers = |byte: u64| {
        let start = desc.start_sector.saturating_mul(SECTOR);
        let end = start.saturating_add(u64::from(desc.nr_sectors).saturating_mul(SECTOR));
        (start..end).contains(&byte)
    };
    match what {
        Held::Read(byte) => desc.op() == op::READ && covers(byte),
        Held::Write(byte) => desc.op() == op::WRITE && covers(byte),
        Held::Flush => desc.op() == op::FLUSH,
    }
}

/// What the queue and the pump's record share: the newest position the
/// pump has asked to store, and whether positions go straight to the
/// real record (no delay, or a stop was asked).
#[derive(Debug)]
struct Late {
    newest: Option<u64>,
    direct: bool,
}

fn late(shared: &Mutex<Late>) -> std::sync::MutexGuard<'_, Late> {
    // Two plain values: whole whoever panicked while holding it.
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The record of what was acknowledged as the pump of a [`Delayed`]
/// queue sees it. Without a delay it is the real record. With one it
/// remembers the position, and the queue stores it when it gives the
/// result on.
pub struct LateAck<A: AckRecord> {
    real: A,
    late: Arc<Mutex<Late>>,
}

impl<A: AckRecord> AckRecord for LateAck<A> {
    fn advance(&mut self, position: u64) -> std::io::Result<()> {
        let mut late = late(&self.late);
        if late.direct {
            return self.real.advance(position);
        }
        late.newest = late.newest.max(Some(position));
        Ok(())
    }
}

/// How often the end of a stop tries again to give on what waits
/// behind a position that could not be stored: the pump's own figure.
const STOP_TRIES: u32 = 20;

/// A commit that waits: when it is due, the position to store before
/// it is given on, and the commit's tag, result and buffer.
type Waiting = (Instant, Option<u64>, u16, i32, Option<Buf>);

/// A queue whose commits reach the queue inside after a delay.
pub struct Delayed<Q: Queue, A: AckRecord> {
    inner: Q,
    /// The real record, for the positions that waited.
    ack: A,
    late: Arc<Mutex<Late>>,
    delay: Option<Duration>,
    /// The one request to hold, until it has been fetched.
    hold: Option<Hold>,
    /// The tag of the request that is held, and for how long, from its
    /// fetch to its commit by the pump.
    held: Option<(u16, Duration)>,
    /// Committed by the pump and not yet given on, in the order the
    /// pump committed them.
    /// With each, the position to store before it is given on.
    waiting: Vec<Waiting>,
    /// Tells the timer when the queue's wait has to be woken.
    timer: Option<mpsc::Sender<Instant>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stopping: bool,
}

impl<Q: Queue, A: AckRecord + Clone> Delayed<Q, A> {
    /// `inner`, with every commit delayed by `delay` if there is one,
    /// and one request held as `hold` says if there is one. `wake`
    /// wakes `inner`'s wait from another thread. `ack` is the real
    /// record of what was acknowledged; the pump gets the record that
    /// is returned.
    pub fn new(
        inner: Q,
        ack: A,
        delay: Option<Duration>,
        hold: Option<Hold>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (Self, LateAck<A>) {
        let late = Arc::new(Mutex::new(Late {
            newest: None,
            direct: delay.is_none(),
        }));
        let pumps = LateAck {
            real: ack.clone(),
            late: late.clone(),
        };
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let timer = delay.map(|_| {
            let (dues, due) = mpsc::channel::<Instant>();
            let wake = wake.clone();
            // The delays are all the same, so the times arrive in
            // order: one thread sleeps to each in turn. It ends when
            // the queue is dropped.
            std::thread::spawn(move || {
                for at in due {
                    std::thread::sleep(at.saturating_duration_since(Instant::now()));
                    wake();
                }
            });
            dues
        });
        let queue = Self {
            inner,
            ack,
            late,
            delay,
            hold,
            held: None,
            waiting: Vec::new(),
            timer,
            wake,
            stopping: false,
        };
        (queue, pumps)
    }

    /// Give on what is due, or everything.
    fn release(&mut self, all: bool) {
        let now = Instant::now();
        let mut kept = Vec::new();
        // Once a position could not be stored, nothing after it is
        // given on in this round.
        let mut unstored = false;
        for (due, position, tag, result, buf) in std::mem::take(&mut self.waiting) {
            if !unstored && (all || due <= now) {
                if let Some(position) = position {
                    if self.ack.advance(position).is_err() {
                        unstored = true;
                        kept.push((due, Some(position), tag, result, buf));
                        continue;
                    }
                }
                self.inner.commit(tag, result, buf);
            } else {
                kept.push((due, position, tag, result, buf));
            }
        }
        self.waiting = kept;
    }

    /// How many commits are waiting.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }
}

impl<Q: Queue, A: AckRecord + Clone> Queue for Delayed<Q, A> {
    fn depth(&self) -> u16 {
        self.inner.depth()
    }

    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
        // Before the wait, so that the queue's wait sends them.
        self.release(self.stopping);
        let before = fetched.len();
        let woken = self.inner.wait(fetched)?;
        if let Some(hold) = self.hold {
            let new = fetched.get(before..).unwrap_or_default();
            if let Some(first) = new.iter().find(|f| is_held(hold.what, &f.desc)) {
                self.held = Some((first.tag, Duration::from_secs(hold.seconds)));
                self.hold = None;
            }
        }
        if woken == Wake::Stop {
            self.stopping = true;
            // From here the pump's positions go straight to the record.
            late(&self.late).direct = true;
        }
        self.release(self.stopping);
        Ok(woken)
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        self.inner.take_buf(tag, blocks)
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        if self.stopping {
            return self.inner.commit(tag, result, buf);
        }
        // The position the pump asked to store with this commit, if it
        // was kept back.
        let position = {
            let late = late(&self.late);
            late.newest.filter(|_| !late.direct)
        };
        if let Some((_, long)) = self.held.filter(|(held, _)| *held == tag) {
            self.held = None;
            self.waiting
                .push((Instant::now() + long, position, tag, result, buf));
            // One thread for the one held request: it wakes the
            // queue's wait when the time is over.
            let wake = self.wake.clone();
            std::thread::spawn(move || {
                std::thread::sleep(long);
                wake();
            });
            return;
        }
        match (self.delay, &self.timer) {
            (Some(delay), Some(timer)) => {
                let due = Instant::now() + delay;
                self.waiting.push((due, position, tag, result, buf));
                let _ = timer.send(due);
            }
            _ => self.inner.commit(tag, result, buf),
        }
    }

    /// The end of a stop. What still waits here waits behind a
    /// position that could not be stored when the stop arrived: it is
    /// tried again [`STOP_TRIES`] times, paced by the queue's pause, as
    /// the pump does for its own completions. If something is left
    /// then, the stop is not orderly: the answer is `Err`, the pump
    /// ends with [`Stopped::QueueDown`](skyzfs_ublk::pump::Stopped) and
    /// the daemon with status 1, and the requests left are issued
    /// again to the next instance.
    fn flush(&mut self) -> Result<(), QueueDown> {
        self.release(true);
        for _ in 0..STOP_TRIES {
            if self.waiting.is_empty() {
                break;
            }
            self.inner.pause();
            self.release(true);
        }
        let sent = self.inner.flush();
        if self.waiting.is_empty() {
            sent
        } else {
            Err(QueueDown)
        }
    }

    fn pause(&mut self) {
        self.inner.pause();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;

    /// A queue that notes its commits and answers its waits from a
    /// script.
    /// A record that notes what it was asked to store, and can refuse.
    #[derive(Default, Clone)]
    struct Stored(Arc<Mutex<(Vec<u64>, bool)>>);

    impl Stored {
        fn positions(&self) -> Vec<u64> {
            self.0.lock().expect("lock").0.clone()
        }
        fn refuse(&self, refuse: bool) {
            self.0.lock().expect("lock").1 = refuse;
        }
    }

    impl AckRecord for Stored {
        fn advance(&mut self, position: u64) -> std::io::Result<()> {
            let mut stored = self.0.lock().expect("lock");
            if stored.1 {
                return Err(std::io::Error::other("refused"));
            }
            stored.0.push(position);
            Ok(())
        }
    }

    #[derive(Default)]
    struct Noting {
        committed: Vec<u16>,
        flushes: u32,
        pauses: u32,
        /// The record takes positions again after this many pauses.
        heals: Option<(u32, Stored)>,
        wakes: VecDeque<Wake>,
        fetches: VecDeque<Fetched>,
    }

    impl Queue for Noting {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
            fetched.extend(self.fetches.drain(..));
            Ok(self.wakes.pop_front().unwrap_or(Wake::Work))
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<Buf> {
            Some(Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, _result: i32, _buf: Option<Buf>) {
            self.committed.push(tag);
        }
        fn flush(&mut self) -> Result<(), QueueDown> {
            self.flushes += 1;
            Ok(())
        }
        fn pause(&mut self) {
            self.pauses += 1;
            if let Some((after, record)) = &self.heals {
                if self.pauses >= *after {
                    record.refuse(false);
                }
            }
        }
    }

    /// Without a delay every commit passes straight through, and no
    /// timer exists.
    #[test]
    fn without_a_delay_a_commit_passes_through() {
        let (mut queue, _ack) =
            Delayed::new(Noting::default(), Stored::default(), None, None, || {});
        queue.commit(1, 0, None);
        queue.commit(2, 0, Some(Buf::zeroed(1)));
        assert_eq!(queue.inner.committed, [1, 2]);
        assert_eq!(queue.waiting(), 0);
        assert!(queue.timer.is_none());
    }

    /// With a delay a commit waits its time, in order, the queue's
    /// wait is woken when it is due, and a wait gives on what is due.
    #[test]
    fn a_delayed_commit_is_given_on_when_it_is_due_and_the_wait_is_woken() {
        let woken = Arc::new(AtomicU32::new(0));
        let delay = Duration::from_millis(40);
        let (mut queue, _ack) =
            Delayed::new(Noting::default(), Stored::default(), Some(delay), None, {
                let woken = woken.clone();
                move || {
                    woken.fetch_add(1, Ordering::SeqCst);
                }
            });
        let began = Instant::now();
        queue.commit(1, 0, None);
        queue.commit(2, 0, None);
        let mut fetched = Vec::new();
        // Not due yet (unless this thread was held up for the delay).
        queue.wait(&mut fetched).expect("wait");
        if began.elapsed() < delay {
            assert!(queue.inner.committed.is_empty());
            assert_eq!(queue.waiting(), 2);
        }
        while woken.load(Ordering::SeqCst) < 2 {
            assert!(began.elapsed() < Duration::from_secs(10), "never woken");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(began.elapsed() >= delay);
        queue.wait(&mut fetched).expect("wait");
        assert_eq!(queue.inner.committed, [1, 2]);
        assert_eq!(queue.waiting(), 0);
    }

    /// A stop gives on everything that waits, and delays nothing after
    /// it: an orderly stop answers what it accepted.
    #[test]
    fn a_stop_gives_on_what_waits_and_delays_nothing_more() {
        let inner = Noting {
            wakes: VecDeque::from([Wake::Stop]),
            ..Noting::default()
        };
        let (mut queue, _ack) = Delayed::new(
            inner,
            Stored::default(),
            Some(Duration::from_secs(3600)),
            None,
            || {},
        );
        queue.commit(3, 0, None);
        assert_eq!(queue.waiting(), 1);
        let mut fetched = Vec::new();
        assert_eq!(queue.wait(&mut fetched), Ok(Wake::Stop));
        assert_eq!(queue.inner.committed, [3]);
        queue.commit(0, 0, None);
        assert_eq!(queue.inner.committed, [3, 0]);
        assert_eq!(queue.waiting(), 0);
    }

    /// A stop while the position of a delayed commit cannot be stored:
    /// the commit stays here, and the end of the stop tries again. If
    /// the record heals in that time everything is given on and the
    /// end is in order; if it never does, the end says so (the pump
    /// then does not call the stop orderly) and the commit was never
    /// given on, so its request is issued again.
    #[test]
    fn a_stop_with_an_unstored_delayed_commit_is_tried_again_and_is_not_orderly_if_it_stays() {
        for heals in [true, false] {
            let stored = Stored::default();
            let inner = Noting {
                wakes: VecDeque::from([Wake::Stop]),
                heals: heals.then(|| (3, stored.clone())),
                ..Noting::default()
            };
            let (mut queue, mut pumps) = Delayed::new(
                inner,
                stored.clone(),
                Some(Duration::from_secs(3600)),
                None,
                || {},
            );
            pumps.advance(100).expect("kept back");
            queue.commit(1, 0, None);
            stored.refuse(true);
            let mut fetched = Vec::new();
            assert_eq!(queue.wait(&mut fetched), Ok(Wake::Stop));
            assert_eq!(queue.waiting(), 1, "behind a position that is not stored");
            assert!(queue.inner.committed.is_empty());
            let ended = queue.flush();
            assert_eq!(queue.inner.flushes, 1, "the queue inside is flushed");
            if heals {
                assert_eq!(ended, Ok(()));
                assert_eq!(queue.inner.committed, [1]);
                assert_eq!(stored.positions(), [100]);
                assert_eq!(queue.inner.pauses, 3);
            } else {
                assert_eq!(ended, Err(QueueDown));
                assert!(queue.inner.committed.is_empty());
                assert!(stored.positions().is_empty());
                assert_eq!(queue.inner.pauses, STOP_TRIES);
            }
        }
    }

    /// With a delay the pump's position is stored in the real record
    /// when the result is given on, not when the pump commits: what a
    /// kill leaves unanswered is then beyond the record. A stop stores
    /// and gives on at once, and afterwards the pump's record is the
    /// real one.
    #[test]
    fn with_a_delay_the_position_is_stored_when_the_result_is_given_on() {
        let stored = Stored::default();
        let inner = Noting {
            wakes: VecDeque::from([Wake::Work, Wake::Stop]),
            ..Noting::default()
        };
        let (mut queue, mut ack) = Delayed::new(
            inner,
            stored.clone(),
            Some(Duration::from_secs(3600)),
            None,
            || {},
        );
        ack.advance(7).expect("remembered");
        queue.commit(1, 0, None);
        ack.advance(9).expect("remembered");
        queue.commit(2, 0, None);
        let mut fetched = Vec::new();
        assert_eq!(queue.wait(&mut fetched), Ok(Wake::Work));
        assert!(stored.positions().is_empty());
        assert!(queue.inner.committed.is_empty());
        assert_eq!(queue.wait(&mut fetched), Ok(Wake::Stop));
        assert_eq!(stored.positions(), [7, 9]);
        assert_eq!(queue.inner.committed, [1, 2]);
        ack.advance(11).expect("stored");
        assert_eq!(stored.positions(), [7, 9, 11]);
    }

    /// A position that cannot be stored keeps its result back, and
    /// every result after it, until it can.
    #[test]
    fn a_result_whose_position_cannot_be_stored_stays_back() {
        let stored = Stored::default();
        let inner = Noting {
            wakes: VecDeque::from([Wake::Stop]),
            ..Noting::default()
        };
        let (mut queue, mut ack) = Delayed::new(
            inner,
            stored.clone(),
            Some(Duration::from_secs(3600)),
            None,
            || {},
        );
        ack.advance(7).expect("remembered");
        queue.commit(1, 0, None);
        queue.commit(2, 0, None);
        stored.refuse(true);
        let mut fetched = Vec::new();
        assert_eq!(queue.wait(&mut fetched), Ok(Wake::Stop));
        assert!(queue.inner.committed.is_empty());
        assert_eq!(queue.waiting(), 2);
        stored.refuse(false);
        queue.wait(&mut fetched).expect("wait");
        assert_eq!(queue.inner.committed, [1, 2]);
        assert_eq!(stored.positions(), [7, 7]);
    }

    /// Without a delay the pump's record is the real one.
    #[test]
    fn without_a_delay_the_position_is_stored_at_once() {
        let stored = Stored::default();
        let (_queue, mut ack) = Delayed::new(Noting::default(), stored.clone(), None, None, || {});
        ack.advance(5).expect("stored");
        assert_eq!(stored.positions(), [5]);
    }

    fn request(tag: u16, op: u8, sector: u64, sectors: u32) -> Fetched {
        Fetched {
            tag,
            desc: Desc {
                op_flags: u32::from(op),
                nr_sectors: sectors,
                start_sector: sector,
            },
        }
    }

    /// Which request a hold waits for: the kind, and for a read or a
    /// write the byte, by the request's first and last byte.
    #[test]
    fn a_hold_names_a_read_or_a_write_that_covers_a_byte_or_a_flush() {
        let read = request(0, op::READ, 8, 8).desc; // bytes 4096..8192
        assert!(is_held(Held::Read(4096), &read));
        assert!(is_held(Held::Read(8191), &read));
        assert!(!is_held(Held::Read(8192), &read));
        assert!(!is_held(Held::Read(4095), &read));
        assert!(!is_held(Held::Write(4096), &read));
        assert!(!is_held(Held::Flush, &read));
        let write = request(0, op::WRITE, 8, 8).desc;
        assert!(is_held(Held::Write(5000), &write));
        assert!(!is_held(Held::Read(5000), &write));
        let flush = request(0, op::FLUSH, 0, 0).desc;
        assert!(is_held(Held::Flush, &flush));
        assert!(!is_held(Held::Read(0), &flush));
    }

    /// The first request of the kind is held for its time while the
    /// others pass; a second of the kind is not held; the wait is woken
    /// when the time is over.
    #[test]
    fn one_request_is_held_and_only_the_first_of_its_kind() {
        let woken = Arc::new(AtomicU32::new(0));
        let inner = Noting {
            fetches: VecDeque::from([
                request(0, op::WRITE, 8, 8),
                request(1, op::READ, 8, 8),
                request(2, op::READ, 8, 8),
            ]),
            ..Noting::default()
        };
        let hold = Hold {
            seconds: 1,
            what: Held::Read(4096),
        };
        let (mut queue, _ack) = Delayed::new(inner, Stored::default(), None, Some(hold), {
            let woken = woken.clone();
            move || {
                woken.fetch_add(1, Ordering::SeqCst);
            }
        });
        let mut fetched = Vec::new();
        queue.wait(&mut fetched).expect("wait");
        assert_eq!(fetched.len(), 3);
        let began = Instant::now();
        for tag in [0, 1, 2] {
            queue.commit(tag, 0, None);
        }
        // The write and the second read went on; the first read waits.
        assert_eq!(queue.inner.committed, [0, 2]);
        assert_eq!(queue.waiting(), 1);
        queue.wait(&mut fetched).expect("wait");
        if began.elapsed() < Duration::from_secs(1) {
            assert_eq!(queue.inner.committed, [0, 2]);
        }
        while woken.load(Ordering::SeqCst) == 0 {
            assert!(began.elapsed() < Duration::from_secs(20), "never woken");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(began.elapsed() >= Duration::from_secs(1));
        queue.wait(&mut fetched).expect("wait");
        assert_eq!(queue.inner.committed, [0, 2, 1]);
        // A later read of the same block is not held.
        queue.inner.fetches.push_back(request(3, op::READ, 8, 8));
        queue.wait(&mut fetched).expect("wait");
        queue.commit(3, 0, None);
        assert_eq!(queue.inner.committed, [0, 2, 1, 3]);
    }

    /// A stop answers the held request at once.
    #[test]
    fn a_stop_answers_the_held_request() {
        let inner = Noting {
            fetches: VecDeque::from([request(5, op::FLUSH, 0, 0)]),
            wakes: VecDeque::from([Wake::Work, Wake::Stop]),
            ..Noting::default()
        };
        let hold = Hold {
            seconds: 3600,
            what: Held::Flush,
        };
        let (mut queue, _ack) = Delayed::new(inner, Stored::default(), None, Some(hold), || {});
        let mut fetched = Vec::new();
        queue.wait(&mut fetched).expect("wait");
        queue.commit(5, 0, None);
        assert!(queue.inner.committed.is_empty());
        assert_eq!(queue.wait(&mut fetched), Ok(Wake::Stop));
        assert_eq!(queue.inner.committed, [5]);
    }
}

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

use skyzfs_ublk::pump::{Fetched, Queue, QueueDown, Unfetched, Wake};
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

/// A commit that waits: when it is due, and the commit's tag, result
/// and buffer.
type Waiting = (Instant, u16, i32, Option<Buf>);

/// A queue whose commits reach the queue inside after a delay.
pub struct Delayed<Q: Queue> {
    inner: Q,
    delay: Option<Duration>,
    /// The one request to hold, until it has been fetched.
    hold: Option<Hold>,
    /// The tag of the request that is held, and for how long, from its
    /// fetch to its commit by the pump.
    held: Option<(u16, Duration)>,
    /// Committed by the pump and not yet given on, in the order the
    /// pump committed them.
    waiting: Vec<Waiting>,
    /// Tells the timer when the queue's wait has to be woken.
    timer: Option<mpsc::Sender<Instant>>,
    wake: Arc<dyn Fn() + Send + Sync>,
    stopping: bool,
    /// The volume failed: see [`Delayed::fail`].
    failed: bool,
}

impl<Q: Queue> Delayed<Q> {
    /// `inner`, with every commit delayed by `delay` if there is one,
    /// and one request held as `hold` says if there is one. `wake`
    /// wakes `inner`'s wait from another thread.
    pub fn new(
        inner: Q,
        delay: Option<Duration>,
        hold: Option<Hold>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
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
        Self {
            inner,
            delay,
            hold,
            held: None,
            waiting: Vec::new(),
            timer,
            wake,
            stopping: false,
            failed: false,
        }
    }

    /// Give on what is due, or everything.
    fn release(&mut self, all: bool) {
        let now = Instant::now();
        let mut kept = Vec::new();
        for (due, tag, result, buf) in std::mem::take(&mut self.waiting) {
            if all || due <= now {
                self.inner.commit(tag, result, buf);
            } else {
                kept.push((due, tag, result, buf));
            }
        }
        self.waiting = kept;
    }

    /// The volume failed. Every commit that waits here is given on now
    /// with the plain I/O error in place of its result: the result may
    /// say "written", and nothing says so after the failure. From now on a commit passes
    /// straight through, the held request's too. Returns how many were
    /// given on. Call it before [`Pump::fail`](skyzfs_ublk::pump::Pump::fail):
    /// a failed volume holds no request, and what waits here is held.
    pub fn fail(&mut self) -> usize {
        self.failed = true;
        let waiting = std::mem::take(&mut self.waiting);
        let given = waiting.len();
        for (_, tag, _, buf) in waiting {
            self.inner.commit(tag, -skyzfs_ublk::translate::EIO, buf);
        }
        given
    }

    /// How many commits are waiting.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }
}

impl<Q: Queue> Queue for Delayed<Q> {
    fn depth(&self) -> u16 {
        self.inner.depth()
    }

    fn wait(
        &mut self,
        fetched: &mut Vec<Fetched>,
        unfetched: &mut Vec<Unfetched>,
    ) -> Result<Wake, QueueDown> {
        // Before the wait, so that the queue's wait sends them.
        self.release(self.stopping);
        let before = fetched.len();
        let woken = self.inner.wait(fetched, unfetched)?;
        if let Some(hold) = self.hold {
            let new = fetched.get(before..).unwrap_or_default();
            if let Some(first) = new.iter().find(|f| is_held(hold.what, &f.desc)) {
                self.held = Some((first.tag, Duration::from_secs(hold.seconds)));
                self.hold = None;
            }
        }
        if woken == Wake::Stop {
            self.stopping = true;
        }
        self.release(self.stopping);
        Ok(woken)
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Option<Buf> {
        self.inner.take_buf(tag, blocks)
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        if self.stopping || self.failed {
            return self.inner.commit(tag, result, buf);
        }
        if let Some((_, long)) = self.held.filter(|(held, _)| *held == tag) {
            self.held = None;
            self.waiting.push((Instant::now() + long, tag, result, buf));
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
                self.waiting.push((due, tag, result, buf));
                let _ = timer.send(due);
            }
            _ => self.inner.commit(tag, result, buf),
        }
    }

    /// The end of a stop: what still waits here is given on, and then
    /// sent.
    fn flush(&mut self) -> Result<(), QueueDown> {
        self.release(true);
        self.inner.flush()
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
    #[derive(Default)]
    struct Noting {
        committed: Vec<u16>,
        results: Vec<i32>,
        flushes: u32,
        wakes: VecDeque<Wake>,
        fetches: VecDeque<Fetched>,
        ends: VecDeque<Unfetched>,
    }

    impl Queue for Noting {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(
            &mut self,
            fetched: &mut Vec<Fetched>,
            unfetched: &mut Vec<Unfetched>,
        ) -> Result<Wake, QueueDown> {
            fetched.extend(self.fetches.drain(..));
            unfetched.extend(self.ends.drain(..));
            Ok(self.wakes.pop_front().unwrap_or(Wake::Work))
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Option<Buf> {
            Some(Buf::zeroed(blocks))
        }
        fn commit(&mut self, tag: u16, result: i32, _buf: Option<Buf>) {
            self.committed.push(tag);
            self.results.push(result);
        }
        fn flush(&mut self) -> Result<(), QueueDown> {
            self.flushes += 1;
            Ok(())
        }
    }

    /// The commands the queue underneath ended otherwise than OK pass
    /// through to the pump, with or without a delay.
    #[test]
    fn the_commands_ended_otherwise_than_ok_pass_through() {
        for delay in [None, Some(Duration::from_millis(40))] {
            let mut inner = Noting::default();
            let ended = Unfetched {
                tag: 2,
                command: skyzfs_ublk::pump::Command::Commit,
                result: -22,
            };
            inner.ends.push_back(ended);
            let mut queue = Delayed::new(inner, delay, None, || {});
            let mut unfetched = Vec::new();
            queue.wait(&mut Vec::new(), &mut unfetched).expect("wait");
            assert_eq!(unfetched, [ended], "{delay:?}");
        }
    }

    /// Without a delay every commit passes straight through, and no
    /// timer exists.
    #[test]
    fn without_a_delay_a_commit_passes_through() {
        let mut queue = Delayed::new(Noting::default(), None, None, || {});
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
        let mut queue = Delayed::new(Noting::default(), Some(delay), None, {
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
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        if began.elapsed() < delay {
            assert!(queue.inner.committed.is_empty());
            assert_eq!(queue.waiting(), 2);
        }
        while woken.load(Ordering::SeqCst) < 2 {
            assert!(began.elapsed() < Duration::from_secs(10), "never woken");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(began.elapsed() >= delay);
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
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
        let mut queue = Delayed::new(inner, Some(Duration::from_secs(3600)), None, || {});
        queue.commit(3, 0, None);
        assert_eq!(queue.waiting(), 1);
        let mut fetched = Vec::new();
        assert_eq!(queue.wait(&mut fetched, &mut Vec::new()), Ok(Wake::Stop));
        assert_eq!(queue.inner.committed, [3]);
        queue.commit(0, 0, None);
        assert_eq!(queue.inner.committed, [3, 0]);
        assert_eq!(queue.waiting(), 0);
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
        let mut queue = Delayed::new(inner, None, Some(hold), {
            let woken = woken.clone();
            move || {
                woken.fetch_add(1, Ordering::SeqCst);
            }
        });
        let mut fetched = Vec::new();
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        assert_eq!(fetched.len(), 3);
        let began = Instant::now();
        for tag in [0, 1, 2] {
            queue.commit(tag, 0, None);
        }
        // The write and the second read went on; the first read waits.
        assert_eq!(queue.inner.committed, [0, 2]);
        assert_eq!(queue.waiting(), 1);
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        if began.elapsed() < Duration::from_secs(1) {
            assert_eq!(queue.inner.committed, [0, 2]);
        }
        while woken.load(Ordering::SeqCst) == 0 {
            assert!(began.elapsed() < Duration::from_secs(20), "never woken");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(began.elapsed() >= Duration::from_secs(1));
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        assert_eq!(queue.inner.committed, [0, 2, 1]);
        // A later read of the same block is not held.
        queue.inner.fetches.push_back(request(3, op::READ, 8, 8));
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        queue.commit(3, 0, None);
        assert_eq!(queue.inner.committed, [0, 2, 1, 3]);
    }

    /// A failed volume holds no request: what waits here, delayed or
    /// held, is given on at the failure with the plain I/O error, and
    /// later commits pass straight through.
    #[test]
    fn a_failure_gives_on_what_waits_with_an_error() {
        let inner = Noting {
            fetches: VecDeque::from([
                request(0, op::WRITE, 8, 8),
                request(1, op::FLUSH, 0, 0),
                request(2, op::FLUSH, 0, 0),
            ]),
            ..Noting::default()
        };
        let hold = Hold {
            seconds: 3600,
            what: Held::Flush,
        };
        let delay = Some(Duration::from_secs(3600));
        let mut queue = Delayed::new(inner, delay, Some(hold), || {});
        let mut fetched = Vec::new();
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        queue.commit(0, 4096, Some(Buf::zeroed(1)));
        queue.commit(1, 0, None);
        assert_eq!(queue.waiting(), 2);
        assert!(queue.inner.committed.is_empty());

        assert_eq!(queue.fail(), 2);
        assert_eq!(queue.waiting(), 0);
        assert_eq!(queue.inner.committed, [0, 1]);
        assert_eq!(queue.inner.results, [-5, -5]);
        // What the pump commits from now on is not kept.
        queue.commit(2, -5, None);
        assert_eq!(queue.inner.committed, [0, 1, 2]);
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
        let mut queue = Delayed::new(inner, None, Some(hold), || {});
        let mut fetched = Vec::new();
        queue.wait(&mut fetched, &mut Vec::new()).expect("wait");
        queue.commit(5, 0, None);
        assert!(queue.inner.committed.is_empty());
        assert_eq!(queue.wait(&mut fetched, &mut Vec::new()), Ok(Wake::Stop));
        assert_eq!(queue.inner.committed, [5]);
    }
}

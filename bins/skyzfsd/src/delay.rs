//! The test build's delay between a request's completion and its
//! commit (`serve --commit-delay-ms N`), as a queue around the queue.
//!
//! For every request, the result is given to the kernel N ms after the
//! pump committed it. Nothing else waits: the store has completed the
//! request and the acknowledged position is stored before the pump
//! commits, and the queue goes on fetching, so that many requests are
//! applied and uncommitted at once. A `kill -9` then finds requests
//! the daemon has applied and not answered: the case of "applied a
//! second time, as a new operation in the order" (design §4.1).
//!
//! Once a stop is asked nothing is delayed any more, and what was
//! waiting is committed at once: an orderly stop still answers all it
//! accepted.
//!
//! In a default build the delay is never set (the command line refuses
//! the option) and every commit passes straight through.

use std::collections::VecDeque;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use skyzfs_blk::Buf;
use skyzfs_ublk::pump::{Fetched, Queue, QueueDown, Wake};

/// A queue whose commits reach the queue inside after a delay.
pub struct Delayed<Q: Queue> {
    inner: Q,
    delay: Option<Duration>,
    /// Committed by the pump and not yet given on, oldest first.
    waiting: VecDeque<(Instant, u16, i32, Option<Buf>)>,
    /// Tells the timer when the queue's wait has to be woken.
    timer: Option<mpsc::Sender<Instant>>,
    stopping: bool,
}

impl<Q: Queue> Delayed<Q> {
    /// `inner`, with every commit delayed by `delay` if there is one.
    /// `wake` wakes `inner`'s wait from another thread.
    pub fn new(inner: Q, delay: Option<Duration>, wake: impl Fn() + Send + 'static) -> Self {
        let timer = delay.map(|_| {
            let (dues, due) = mpsc::channel::<Instant>();
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
            waiting: VecDeque::new(),
            timer,
            stopping: false,
        }
    }

    /// Give on what is due, or everything.
    fn release(&mut self, all: bool) {
        let now = Instant::now();
        while let Some((due, ..)) = self.waiting.front() {
            if !all && *due > now {
                break;
            }
            if let Some((_, tag, result, buf)) = self.waiting.pop_front() {
                self.inner.commit(tag, result, buf);
            }
        }
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

    fn wait(&mut self, fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
        // Before the wait, so that the queue's wait sends them.
        self.release(self.stopping);
        let woken = self.inner.wait(fetched)?;
        if woken == Wake::Stop {
            self.stopping = true;
        }
        self.release(self.stopping);
        Ok(woken)
    }

    fn take_buf(&mut self, tag: u16, blocks: usize) -> Buf {
        self.inner.take_buf(tag, blocks)
    }

    fn commit(&mut self, tag: u16, result: i32, buf: Option<Buf>) {
        match (self.delay, &self.timer) {
            (Some(delay), Some(timer)) if !self.stopping => {
                let due = Instant::now() + delay;
                self.waiting.push_back((due, tag, result, buf));
                let _ = timer.send(due);
            }
            _ => self.inner.commit(tag, result, buf),
        }
    }

    fn pause(&mut self) {
        self.inner.pause();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use super::*;

    /// A queue that notes its commits and answers its waits from a
    /// script.
    #[derive(Default)]
    struct Noting {
        committed: Vec<u16>,
        wakes: VecDeque<Wake>,
    }

    impl Queue for Noting {
        fn depth(&self) -> u16 {
            4
        }
        fn wait(&mut self, _fetched: &mut Vec<Fetched>) -> Result<Wake, QueueDown> {
            Ok(self.wakes.pop_front().unwrap_or(Wake::Work))
        }
        fn take_buf(&mut self, _tag: u16, blocks: usize) -> Buf {
            Buf::zeroed(blocks)
        }
        fn commit(&mut self, tag: u16, _result: i32, _buf: Option<Buf>) {
            self.committed.push(tag);
        }
        fn pause(&mut self) {}
    }

    /// Without a delay every commit passes straight through, and no
    /// timer exists.
    #[test]
    fn without_a_delay_a_commit_passes_through() {
        let mut queue = Delayed::new(Noting::default(), None, || {});
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
        let mut queue = Delayed::new(Noting::default(), Some(delay), {
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
        let mut queue = Delayed::new(inner, Some(Duration::from_secs(3600)), || {});
        queue.commit(3, 0, None);
        assert_eq!(queue.waiting(), 1);
        let mut fetched = Vec::new();
        assert_eq!(queue.wait(&mut fetched), Ok(Wake::Stop));
        assert_eq!(queue.inner.committed, [3]);
        queue.commit(0, 0, None);
        assert_eq!(queue.inner.committed, [3, 0]);
        assert_eq!(queue.waiting(), 0);
    }
}

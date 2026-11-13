//! The in-place store's reads and writes on threads of their own
//! (design, decision D6). Each thread has a handle of its own on the
//! home image and does one request at a time, by direct I/O (through
//! the cache in some tests), so that as many requests reach the device
//! at once as there are threads. The store decides what may run beside
//! what ([`super::InPlace`]); a thread only executes.
//!
//! A thread that panics dies, and says so first: the pool counts the
//! death ([`Pool::died`]), and the job the thread held comes back to the
//! store with its buffer and an error. The store then fails the
//! volume and takes back the jobs no thread has begun
//! ([`Pool::take_back`]), so that nothing waits for a thread that is
//! gone. The release build unwinds on a panic (the workspace does not
//! set `panic = "abort"`); where a build aborts instead, the process
//! ends, and the kernel's device waits for the next server as after a
//! `kill -9`.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use skyzfs_blk::{Buf, RequestId};

use super::Mutants;
use crate::io::File;

/// A request handed to a thread.
pub(super) enum Job {
    Read {
        id: RequestId,
        offset: u64,
        buf: Buf,
    },
    Write {
        id: RequestId,
        offset: u64,
        data: Buf,
    },
}

impl Job {
    /// Its id, whether it writes, and its buffer.
    pub(super) fn into_parts(self) -> (RequestId, bool, Buf) {
        match self {
            Job::Read { id, buf, .. } => (id, false, buf),
            Job::Write { id, data, .. } => (id, true, data),
        }
    }
}

/// What a thread did with a [`Job`].
pub(super) struct Done {
    pub(super) id: RequestId,
    /// Whether the job was a write.
    pub(super) write: bool,
    pub(super) result: io::Result<()>,
    pub(super) buf: Buf,
}

#[derive(Default)]
struct State {
    jobs: VecDeque<Job>,
    done: Vec<Done>,
    ending: bool,
    /// Threads that died of a panic.
    died: u64,
    /// Threads waiting for a job.
    #[cfg(test)]
    waiting: usize,
    /// Wakes that found no job and no end.
    #[cfg(test)]
    futile: u64,
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// The threads wait here for a job.
    work: Condvar,
    /// The store waits here for a job's end ([`Pool::take`]).
    done: Condvar,
    mutants: Mutants,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The threads. Dropping the pool lets them do every job pushed so far,
/// then ends them.
pub(super) struct Pool {
    shared: Arc<Shared>,
    threads: Vec<JoinHandle<()>>,
}

impl Pool {
    /// One thread for each of `files`, handles on the home image. With
    /// `direct`, they read and write by direct I/O. `wake` is called
    /// after each job's end, as a sync's end calls it.
    pub(super) fn start<F: File + Send + 'static>(
        files: Vec<F>,
        direct: bool,
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        mutants: Mutants,
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared {
            mutants,
            ..Shared::default()
        });
        let mut pool = Self {
            shared: shared.clone(),
            threads: Vec::new(),
        };
        for (n, file) in files.into_iter().enumerate() {
            let shared = shared.clone();
            let wake = wake.clone();
            match spawn(n, move || work(file, direct, &shared, wake.as_deref())) {
                Ok(thread) => pool.threads.push(thread),
                Err(e) => {
                    if mutants.has(Mutants::POOL_LEAVES_STARTED) {
                        std::mem::forget(pool);
                    }
                    // The pool is dropped, which ends the threads
                    // already started.
                    return Err(e);
                }
            }
        }
        Ok(pool)
    }

    pub(super) fn push(&self, job: Job) {
        self.shared.lock().jobs.push_back(job);
        if self.shared.mutants.has(Mutants::POOL_WAKES_ALL) {
            self.shared.work.notify_all();
        } else {
            self.shared.work.notify_one();
        }
    }

    /// Take the jobs that ended; with a `timeout`, wait up to it for one
    /// if none has.
    pub(super) fn take(&self, out: &mut Vec<Done>, timeout: Duration) {
        let mut state = self.shared.lock();
        if state.done.is_empty() && !timeout.is_zero() {
            state = self
                .shared
                .done
                .wait_timeout_while(state, timeout, |s| s.done.is_empty())
                .map(|(guard, _)| guard)
                .unwrap_or_else(|poisoned| poisoned.into_inner().0);
        }
        out.append(&mut state.done);
    }

    /// How many threads have died of a panic so far.
    pub(super) fn died(&self) -> u64 {
        self.shared.lock().died
    }

    /// Take back the jobs no thread has begun.
    pub(super) fn take_back(&self) -> Vec<Job> {
        self.shared.lock().jobs.drain(..).collect()
    }
}

/// Start thread `n` of the pool, running `f`.
fn spawn(n: usize, f: impl FnOnce() + Send + 'static) -> io::Result<JoinHandle<()>> {
    #[cfg(test)]
    if tests::SPAWN_REFUSED_AT.with(std::cell::Cell::get) == Some(n) {
        return Err(io::Error::other("a test refused the thread"));
    }
    std::thread::Builder::new()
        .name(format!("skyzfs-io-{n}"))
        .spawn(f)
}

/// Wait on `shared` for a job or the end. The tests count the waiting
/// threads and the wakes that found nothing to do.
#[cfg_attr(not(test), allow(unused_mut))]
fn wait<'a>(shared: &'a Shared, mut state: MutexGuard<'a, State>) -> MutexGuard<'a, State> {
    #[cfg(test)]
    {
        state.waiting += 1;
    }
    let mut state = shared
        .work
        .wait(state)
        .unwrap_or_else(PoisonError::into_inner);
    #[cfg(test)]
    {
        state.waiting -= 1;
        if state.jobs.is_empty() && !state.ending {
            state.futile += 1;
        }
    }
    state
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.shared.lock().ending = true;
        self.shared.work.notify_all();
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}

/// A thread's life: on a panic, it reports the death and hands back
/// the job the thread held, with its buffer, and wakes the store.
struct Life<'a> {
    shared: &'a Shared,
    wake: Option<&'a (dyn Fn() + Send + Sync)>,
    /// The job being executed: its id, whether it writes, its buffer.
    job: Option<(RequestId, bool, Buf)>,
    /// The thread is in `wake`: a panic there is not followed by
    /// another call of it, which would abort the process.
    waking: bool,
}

impl Drop for Life<'_> {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            return;
        }
        let mut state = self.shared.lock();
        state.died += 1;
        if let Some((id, write, buf)) = self.job.take() {
            state.done.push(Done {
                id,
                write,
                result: Err(io::Error::other("the I/O thread panicked")),
                buf,
            });
        }
        drop(state);
        self.shared.done.notify_all();
        if let (Some(wake), false) = (self.wake, self.waking) {
            wake();
        }
    }
}

fn work<F: File>(
    mut file: F,
    direct: bool,
    shared: &Shared,
    wake: Option<&(dyn Fn() + Send + Sync)>,
) {
    let mut life = Life {
        shared,
        wake,
        job: None,
        waking: false,
    };
    let mutants = shared.mutants;
    let mut after_job = false;
    loop {
        let job = {
            let mut state = shared.lock();
            if after_job && mutants.has(Mutants::POOL_WAITS_FIRST) && !state.ending {
                state = wait(shared, state);
            }
            loop {
                if state.ending && mutants.has(Mutants::POOL_ENDS_QUEUED) {
                    return;
                }
                if let Some(job) = state.jobs.pop_front() {
                    break job;
                }
                if state.ending {
                    return;
                }
                if mutants.has(Mutants::POOL_WAKE_LOST) {
                    drop(state);
                    std::thread::sleep(Duration::from_millis(1));
                    state = shared.lock();
                }
                // The look above and this wait are under one hold of
                // the lock, so that no push falls between them.
                state = wait(shared, state);
            }
        };
        after_job = true;
        let offset = match &job {
            Job::Read { offset, .. } | Job::Write { offset, .. } => *offset,
        };
        // Held by `life` while it runs, so that a panic hands it back.
        let (_, write, buf) = life.job.insert(job.into_parts());
        let result = match (*write, direct) {
            (false, true) => file.read_direct_at(offset, buf),
            (false, false) => file.read_at(offset, buf),
            (true, true) => file.write_direct_at(offset, buf),
            (true, false) => file.write_at(offset, buf),
        };
        if let Some((id, write, buf)) = life.job.take() {
            shared.lock().done.push(Done {
                id,
                write,
                result,
                buf,
            });
        }
        shared.done.notify_all();
        if let Some(wake) = wake {
            life.waking = true;
            wake();
            life.waking = false;
        }
    }
}

#[cfg(test)]
mod tests;

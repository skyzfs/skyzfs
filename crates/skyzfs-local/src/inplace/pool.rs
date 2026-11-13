//! The in-place store's reads and writes on threads of their own (an
//! experiment: the direct I/O note of M1.3u). Each thread has a handle
//! of its own on the home image and does one request at a time,
//! through the cache or by direct I/O, so that as many requests reach
//! the device at once as there are threads. The store decides what may
//! run beside what ([`super::InPlace`]); a thread only executes.

use std::collections::VecDeque;
use std::io;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::Duration;

use skyzfs_blk::{Buf, RequestId};

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
}

#[derive(Default)]
struct Shared {
    state: Mutex<State>,
    /// The threads wait here for a job.
    work: Condvar,
    /// The store waits here for a job's end ([`Pool::take`]).
    done: Condvar,
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
    ) -> io::Result<Self> {
        let shared = Arc::new(Shared::default());
        let mut pool = Self {
            shared: shared.clone(),
            threads: Vec::new(),
        };
        for (n, file) in files.into_iter().enumerate() {
            let shared = shared.clone();
            let wake = wake.clone();
            // On an error the pool is dropped, which ends the threads
            // already started.
            let thread = std::thread::Builder::new()
                .name(format!("skyzfs-io-{n}"))
                .spawn(move || work(file, direct, &shared, wake.as_deref()))?;
            pool.threads.push(thread);
        }
        Ok(pool)
    }

    pub(super) fn push(&self, job: Job) {
        self.shared.lock().jobs.push_back(job);
        self.shared.work.notify_one();
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

fn work<F: File>(
    mut file: F,
    direct: bool,
    shared: &Shared,
    wake: Option<&(dyn Fn() + Send + Sync)>,
) {
    loop {
        let job = {
            let mut state = shared.lock();
            loop {
                if let Some(job) = state.jobs.pop_front() {
                    break job;
                }
                if state.ending {
                    return;
                }
                state = shared
                    .work
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        };
        let done = match job {
            Job::Read {
                id,
                offset,
                mut buf,
            } => {
                let result = if direct {
                    file.read_direct_at(offset, &mut buf)
                } else {
                    file.read_at(offset, &mut buf)
                };
                Done {
                    id,
                    write: false,
                    result,
                    buf,
                }
            }
            Job::Write { id, offset, data } => {
                let result = if direct {
                    file.write_direct_at(offset, &data)
                } else {
                    file.write_at(offset, &data)
                };
                Done {
                    id,
                    write: true,
                    result,
                    buf: data,
                }
            }
        };
        shared.lock().done.push(done);
        shared.done.notify_all();
        if let Some(wake) = wake {
            wake();
        }
    }
}

//! The I/O threads alone. Ported from the log store's tests of its
//! reader threads (`store::tests`, at a234314), when the log store was
//! removed (M1.3t): the in-place store's threads take their jobs from
//! one queue as those readers did, and are held to the same rules.

use std::cell::Cell;
use std::time::Instant;

use super::*;
use crate::io::Dir;
use crate::sim::{SimDisk, SimFile};

thread_local! {
    /// The thread that [`spawn`] refuses to start, on the calling
    /// thread.
    pub(super) static SPAWN_REFUSED_AT: Cell<Option<usize>> = const { Cell::new(None) };
}

/// The bound that ends a test that would otherwise hang.
const WAIT: Duration = Duration::from_secs(60);

/// `n` handles on a file of 64 blocks.
fn files(disk: &SimDisk, n: usize) -> Vec<SimFile> {
    let mut file = disk.create("image").unwrap();
    file.write_at(0, &[7u8; 64 * 4096]).unwrap();
    (0..n).map(|_| disk.open("image").unwrap()).collect()
}

/// A read of one block, numbered `n`.
fn read(n: u64) -> Job {
    Job::Read {
        id: RequestId(n),
        offset: (n % 64) * 4096,
        buf: Buf::filled(1, 0),
    }
}

/// Wait, for at most [`WAIT`], until `holds`.
fn until(holds: impl Fn() -> bool, what: &str) {
    let began = Instant::now();
    while !holds() {
        assert!(began.elapsed() < WAIT, "never: {what}");
        std::thread::yield_now();
    }
}

/// How many jobs are done once the pool, dropped with `mutants`, has
/// ended: two threads held in their jobs, thirty more queued, and the
/// drop under way before the held threads go on.
fn done_at_the_end(mutants: u32) -> usize {
    let disk = SimDisk::new(1);
    let pool = Pool::start(files(&disk, 2), true, None, Mutants(mutants)).unwrap();
    let shared = pool.shared.clone();
    disk.hold_direct_io(2);
    pool.push(read(0));
    pool.push(read(1));
    until(|| disk.held_waiters() == 2, "both threads are held");
    for n in 2..32 {
        pool.push(read(n));
    }
    let dropping = std::thread::spawn(move || drop(pool));
    until(|| shared.lock().ending, "the drop began");
    disk.release_direct_io();
    dropping.join().unwrap();
    let state = shared.lock();
    assert!(state.done.iter().all(|done| done.result.is_ok()));
    state.done.len()
}

/// At the end the threads do every job queued before they end: a store
/// that ends hands nothing back undone. The defect that ends the
/// threads first is noticed. (Stands in for
/// `store::tests::at_the_end_the_readers_take_what_is_queued_and_end`.)
#[test]
fn at_the_end_the_threads_do_what_is_queued_and_end() {
    assert_eq!(done_at_the_end(0), 32);
    assert_eq!(done_at_the_end(Mutants::POOL_ENDS_QUEUED), 2);
}

/// Whether a job pushed while both threads are held in theirs is done
/// within `timeout` of their release, with `mutants`.
fn taken_by_the_first_free(mutants: u32, timeout: Duration) -> bool {
    let disk = SimDisk::new(2);
    let pool = Pool::start(files(&disk, 2), true, None, Mutants(mutants)).unwrap();
    disk.hold_direct_io(2);
    pool.push(read(0));
    pool.push(read(1));
    until(|| disk.held_waiters() == 2, "both threads are held");
    // No thread waits: the push wakes nobody, and the job waits in the
    // queue for the first thread that is free.
    pool.push(read(2));
    disk.release_direct_io();
    let mut done = Vec::new();
    let began = Instant::now();
    while done.len() < 3 && began.elapsed() < timeout {
        pool.take(&mut done, Duration::from_millis(10));
    }
    done.len() == 3
}

/// A job pushed while every thread is busy is taken by the first that
/// is free, which looks at the queue before it waits. The defect of a
/// thread that waits to be woken first is noticed. (Stands in for
/// `store::tests::a_read_pushed_while_every_reader_is_busy_is_taken_by_the_first_free`.)
#[test]
fn a_job_pushed_while_every_thread_is_busy_is_taken_by_the_first_free() {
    assert!(taken_by_the_first_free(0, WAIT));
    assert!(
        !taken_by_the_first_free(Mutants::POOL_WAITS_FIRST, Duration::from_secs(1)),
        "the defect is not noticed"
    );
}

/// Whether each of `rounds` jobs, pushed one at a time to one thread,
/// is done within `timeout`, with `mutants`.
fn every_push_reaches_the_thread(mutants: u32, rounds: usize, timeout: Duration) -> bool {
    let disk = SimDisk::new(3);
    let pool = Pool::start(files(&disk, 1), false, None, Mutants(mutants)).unwrap();
    let mut done = Vec::new();
    for n in 0..rounds {
        pool.push(read(n as u64));
        let began = Instant::now();
        while done.len() <= n {
            if began.elapsed() > timeout {
                return false;
            }
            pool.take(&mut done, timeout);
        }
    }
    true
}

/// A thread looks at the queue and waits under one hold of the lock, so
/// that a job pushed in between is not left with the thread asleep.
/// Ten thousand pushes, each as the thread has just ended the job
/// before. The defect that lets go of the lock between the look and
/// the wait is noticed. (Stands in for
/// `store::tests::no_wake_is_lost_between_a_readers_look_and_its_wait`.)
#[test]
fn no_wake_is_lost_between_a_threads_look_and_its_wait() {
    assert!(every_push_reaches_the_thread(0, 10_000, WAIT));
    assert!(
        !every_push_reaches_the_thread(Mutants::POOL_WAKE_LOST, 1_000, Duration::from_millis(500)),
        "the defect is not noticed"
    );
}

/// How many wakes of four waiting threads found nothing to do, over
/// eight jobs pushed one at a time, with `mutants`.
///
/// A thread is counted as waiting under the lock, just before its wait
/// lets go of the lock and puts it to sleep. A job pushed in that gap
/// finds the thread not yet asleep, and the thread returns from its
/// wait as well as the one the push woke: a wake for nothing that the
/// pool did not ask for. So each push, the first one included, comes a
/// while after the last thread was counted, when all four are asleep.
fn futile_wakes(mutants: u32) -> u64 {
    let disk = SimDisk::new(4);
    let pool = Pool::start(files(&disk, 4), false, None, Mutants(mutants)).unwrap();
    let shared = pool.shared.clone();
    let mut done = Vec::new();
    for n in 0..8 {
        until(|| shared.lock().waiting == 4, "every thread waits");
        // Time for the threads counted to be asleep.
        std::thread::sleep(Duration::from_millis(20));
        pool.push(read(n as u64));
        while done.len() <= n {
            pool.take(&mut done, WAIT);
        }
        until(|| shared.lock().waiting == 4, "every thread waits again");
        // Time for a thread woken for nothing to find so.
        std::thread::sleep(Duration::from_millis(20));
    }
    let futile = shared.lock().futile;
    futile
}

/// A job wakes one waiting thread, not all of them: the others would
/// wake for nothing, on every request. The defect that wakes them all
/// is noticed. (Stands in for `store::tests::a_read_wakes_one_waiting_reader`.)
#[test]
fn a_job_wakes_one_waiting_thread() {
    assert_eq!(futile_wakes(0), 0);
    assert!(
        futile_wakes(Mutants::POOL_WAKES_ALL) > 0,
        "the defect is not noticed"
    );
}

/// How many of the threads started before the third, which cannot be
/// started, are left once the start has failed, with `mutants`: each
/// holds a share of the waker until it ends.
fn left_after_a_refused_start(mutants: u32) -> usize {
    let disk = SimDisk::new(5);
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(|| {});
    SPAWN_REFUSED_AT.with(|at| at.set(Some(2)));
    let started = Pool::start(files(&disk, 3), false, Some(wake.clone()), Mutants(mutants));
    SPAWN_REFUSED_AT.with(|at| at.set(None));
    assert!(started.is_err());
    Arc::strong_count(&wake) - 1
}

/// A start that cannot start one of its threads ends those it started:
/// none is left waiting for a job that never comes. The defect that
/// leaves them is noticed. (Stands in for
/// `store::tests::a_thread_that_cannot_be_started_leaves_no_reader_waiting`.)
#[test]
fn a_thread_that_cannot_be_started_leaves_no_thread_waiting() {
    assert_eq!(left_after_a_refused_start(0), 0);
    assert_eq!(left_after_a_refused_start(Mutants::POOL_LEAVES_STARTED), 2);
}

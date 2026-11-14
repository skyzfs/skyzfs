# inplace::pool::tests::a_job_wakes_one_waiting_thread can fail without a defect

From M1.3t (#69), on zfs-topology. `futile_wakes(0) == 0` (pool/tests.rs:174)
failed in the first two runs of skyzfs-local's lib tests at #75's head
6fe1aba (one unplanted, one beside plant S2), minutes after the container
started. Afterwards: 0 failures in 74 runs at the head and 108 at the base
a7853c9 (rates.txt, rates2.txt), including two whole suites at once.

Why it can fail: `wait()` in pool.rs counts `waiting += 1` under the lock,
then Condvar::wait reads the futex value, unlocks, and only then sleeps.
The test pushes as soon as `waiting == 4`. A push in the gap: notify_one
bumps the futex and wakes one sleeping thread; the thread not yet asleep
finds the futex changed and returns at once. Two threads wake for one job,
one counts a futile wake. Most likely at the first push after
Pool::start (the later pushes follow a 20 ms sleep).

Reported to the lead as a decision; suggested: sleep before each push, or
bound the clean count (e.g. <= 2) against the plant's 24.

Note on method: worktrees sharing CARGO_TARGET_DIR (new build-dir layout)
build the test binary to one path, and cargo can call another worktree's
build fresh. Check the test count (`--list`) or use a target dir per
worktree.

# M1.3p (skyzfs/skyzfs#37): the readers' queue wakes one thread per read

`skyzfs-coding` at 00f4440, base `zfs-topology` 35e5c40. One commit,
`crates/skyzfs-local/src/store.rs` and `store/tests.rs`.

**Verdict: one finding blocks the merge (finding 1, a small fix). The queue
itself is right.**

## What I did

- Read the diff before any description (the description was not read).
- Ran the store's lib tests at 00f4440: 242 pass, 3 ignored, 113 s.
- Planted a failure of the second reader's start in `serve` and a test that
  counts the threads named `skyzfs-reader*` two seconds after `serve`
  returned its error. Base 35e5c40: none left, the test passes. 00f4440:
  `readers left: 0 -> 2`. Plants removed.
- Ran the four new tests in a loop for two minutes with twelve busy loops on
  four CPUs: 652 runs, no failure.

## What I did not do

- No daemon or kernel run; no benchmark. Whether a read now costs one wake
  on a runner is the testing session's to measure.
- No clippy or fmt run.

## The lead's questions

1. **Lost wakes.** None. `next` looks and waits under one hold of the lock;
   `push` queues under the lock and wakes after it. `close` sets the flag
   under the lock and wakes all; `look` and `next` give a queued read before
   they say closed, so reads queued at the close are taken.
2. **The idle account.** Unchanged in shape: the reader shows itself idle,
   calls `next`, and `reader_woke` closes the idle span. A wake for nothing
   stays inside `next` and counts as idle, which is right. The wait for the
   old queue's lock is gone with the lock.
3. **The mutants and the 100 ms.** `READ_WAKE_LOST` is caught by events
   alone. `READ_WAKES_ALL` rests on the 100 ms and on the count of five
   wakes; see finding 2. Neither can pass a wrong store in practice.
4. **What the old channel gave.** One thing is lost: finding 1. A panic in
   one reader behaves as before (it happens outside the queue's lock; the
   lock ignores poisoning; `gone` refuses the next submit).

## Findings

1. **Blocking. An early return from `serve` leaves the started readers
   waiting for ever.** `store.rs:4524-4526` (and the worker's start at
   `:4832`). The comment says "The readers already started end when the
   reads' sender is dropped on an early return." That was true of the
   channel. Dropping the `Arc<ReadQueue>` closes nothing, so each started
   reader stays in `next` with its handles on the store's files open, and
   its `Ending` never runs. Sequence: `serve` with two readers, the second
   thread (or the worker) cannot be started, `serve` returns the error;
   reader 0 never ends. Measured as above. Sure. Fix: close the queue on the
   way out (a guard that closes on drop covers both returns). I drop it when
   the early returns close the queue and a test fails without that.
2. **Not blocking. `a_read_wakes_one_waiting_reader` can in principle fail a
   right store, and its mutant can pass.** `tests.rs`, the test's 100 ms
   watch and `woke != 5`.
   - Fail a right store: the standard library's condition variable lets go
     of the lock before it enters the kernel's wait. A reader counted as
     waiting but not yet in the kernel returns at once when `push` wakes
     another, so one push wakes two, one for nothing. The window is a few
     instructions wide.
   - Mutant passes: if the three readers woken for nothing do not run within
     the 100 ms, the close finds them not yet counted, and the wakes sum to
     five.
   Neither happened in 652 loaded runs. From reading; low. I drop it as it
   stands; the note is for whoever sees the test fail once.
3. **Not blocking. With every reader gone, `push` still accepts.** `:5043`.
   The old `send` failed once all receivers were dropped. Now only `gone`
   stops a submit, and it is asked first, so I found no path that queues a
   read nobody takes for long. Read only.

## Suggestions

- `push` wakes even when no reader waits: one system call per read on the
  submitting thread while all readers are busy. The channel did none. A
  count of waiting readers kept outside `cfg(test)` would let `push` skip it.
- No test of the idle account is new; the existing timing tests cover the
  path (they passed).

## Second look, at cc4b1a0 (af7b09c and cc4b1a0 on top of 00f4440)

**Verdict: the code is right, both the fix and the new wake logic. One thing
still blocks, and it is small: the new test passes with the fix taken out.**

What I did: read both commits before the answers on the pull request (those
I did not read). Store lib tests: 243 pass, 3 ignored. Plants, each removed
after. Six tests of the queue and of several readers looped for two minutes
with twelve busy loops on four CPUs: 210 runs, no failure. No clippy or fmt.

Per finding:

1. **The fix: right. Its test: kept, blocking.**
   - `Starting` closes the queue and joins the started readers on both early
     returns and on a panic; `hand_over` is the last field of the `Served`,
     so nothing can return between. With my own count of threads named
     `skyzfs-reader*` after a failed `serve`: none left, for the second
     reader's start and for the worker's.
   - `a_thread_that_cannot_be_started_leaves_no_reader_waiting` passes when
     `Starting`'s drop does nothing (planted: `if self.handed || true`),
     which is the store as it was before the fix. `serve` returns its error
     at once and the store reopens while a reader still waits. The test
     fails only for a join without the close. My count of threads fails
     with the same plant (`readers left ... left: 1 right: 0`).
   - I drop it when a test fails with the guard taken out: a count of the
     readers that have ended, or of the threads, after `serve` returned.
2. Dropped. The comment says what can happen and why it is left.
3. Unchanged, not blocking.

The suggestion, now code (cc4b1a0): **no push can skip a wake that a reader
needed.**

- A reader is counted in `waiting` under the hold of the lock in which it
  found the queue empty, and counted out under the hold in which it looks
  again. So `waiting == 0` at a push means no reader is in a wait or on its
  way into one; each reader looks before it waits.
- A reader woken and not yet running is still counted, so a second push
  wakes again; that wake goes to another waiting reader or to nobody, and
  the first reader finds what is queued when it looks. Nobody waits beside
  a queued read.
- The close always wakes all. `look` followed by `next` looks again under
  the lock.
- Planted: wake only when more than one waits, and never counting a
  waiter. `no_wake_is_lost_between_a_readers_look_and_its_wait` fails for
  the first, it and `a_read_wakes_one_waiting_reader` for the second.

## Third look, at 3cf0a0b (one commit, tests only in effect)

**Verdict: can merge. Finding 1 is dropped; nothing blocks.**

- Each reader counts itself as ended when its thread's closure ends; the
  test reads the count after `serve` returned its error and wants it equal
  to the readers started (1 of 1 for the second reader's failed start, 2
  of 2 for the worker's). A reader that was refused never ran and is not
  counted, which is right.
- With my own plant (`if self.handed || true`) the test fails:
  `thread 1: 0 of 1 readers ended when the serve returned`. The named
  mutant `STARTED_READERS_LEFT` is the same change and is asserted to fail.
- Store lib tests: 243 pass, 3 ignored. No clippy or fmt. The answer on
  the pull request was not read.
- Left as they were, not blocking: finding 3 (with every reader gone a
  push still accepts), and the two accepted gaps in the timing test.

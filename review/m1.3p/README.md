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

# M1.3z (#57): two timing tests that raced with the store's threads

Reviewed at 8bf6af4 (two commits on 5ca6129), test code only. Comment:
https://github.com/skyzfs/skyzfs/pull/57#issuecomment-6852492012

Verdict: nothing found. Both fixes remove their race and weaken
nothing. Can merge once CI is green (coverage was still running).

## What the fixes rest on

1. **The idle look.** A look (`both`, `store.rs:4416`) closes a
   thread's wait for work only if the thread has shown itself in it. A
   reader that finds its first read already queued takes it without a
   wait, and has idle time 0 until it loops back and shows itself idle.
   The test now looks again until both threads show idle time (10 s at
   most). When both are above 0 at the first look, it is the old test.
2. **The barrier hold.** The barrier's wait is counted from the end of
   the step's save part (`barred`, `store.rs:1416`); the old hold began
   at the request. The test now waits for `shared.pin.waited()` to move
   (`at_a_barrier`) and then holds 200 ms, as the file's five other
   hold tests do.

## What is here

- `plants.py WORKTREE BASE [name...]`: seven plants in `store.rs`; for
  each it runs the test in its old form (the test file at BASE) and its
  new form. Delays (d1 50 ms in the reader's loop; d2, d3 20 and 150 ms
  before the step's save part) fail the old test and pass the new.
  Controls (c1 the reader's wait counted as "other"; c2 the home
  barrier not waited at; c3 the wait counted as the save part; c4 the
  wait not added to the barrier's sum) fail both.
- `under-load.sh WORKTREE BASE RUNS`: builds both forms and runs each
  test RUNS times with every CPU busy four times over, nothing planted.
  Here (4 CPUs, 40 runs): the old barrier test failed 4 times (196.0 ms
  against 200), the new 0; the idle test 0 and 0.

## Run here

- The seven plants and the load comparison, as above.
- `cargo fmt --check`, clippy on `skyzfs-local` plain and with
  `test-hooks`: clean. `cargo test -p skyzfs-local`: 294 passed, 0
  failed, 6 ignored, and the same with `test-hooks`.

## Traps met

- With the home barrier skipped, `at_a_barrier` still returns: the
  step's second barrier waits for the same pinned reader. The test
  fails all the same, on `barrier_home.max_ns`, not on the helper.
- Load alone does not bring out the idle race: it needs a reader thread
  that starts after its first read is queued.
- After `git fetch origin A B`, `FETCH_HEAD` is A: fetch a pull
  request's head on its own before checking it out.

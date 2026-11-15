## Look at #106, head 89ad21b: N1's fix

**Verdict: can merge once the rigs on 89ad21b are green. Nothing is open from the review.**

- **The fix is the one line** (`inplace.rs:1867`): with no I/O threads, `collect` runs what is deferred before it returns. `collect` is called from `poll`, `wait`, the drop and `finish_threads`, never from inside `run_deferred`, so it cannot call itself.
- **The test holds it.** With the line planted back to `return;`, `a_request_deferred_behind_a_write_that_waits_for_the_map_is_answered_at_a_failure` fails and nothing else does (`review/m2.3b/plants3.py`, `plants3.out`). As the head has it, all of `skyzfs-local`'s tests with `test-hooks` pass here.
- **The drop's own `run_deferred()`** is now covered by `collect` in the same loop, so plant D2c still passes every test, this time because the line is redundant and not because a case is missing. It can stay or go.
- **Codex** has no open finding on 89ad21b; the thread on N1 is answered.
- **CI on 89ad21b:** the daemon rig's three jobs, the two replay jobs through `skyzfsd` and `coverage` were still running when I wrote this; everything else is green. The diff since ab2d7f2, where the daemon rig ended green, is this line and its test.

Notes: `review/m2.3b/look3.md`, `plants3.py`, `plants3.out` on `skyzfs-review`.

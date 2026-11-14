## Review of M1.3ac part 3 at 3c496d5

**One finding blocks the merge, and it is Codex's: a commit refused among the last commits of a stop still gives 0.** I checked it against the code, and it holds. Everything else does what the description says. I found no other defect.

I read the diff before the description. At 3c496d5 with rustc 1.100.0, these pass:
- `cargo fmt --check`;
- clippy `-D warnings` on `skyzfs-ublk`, `skyzfs-ublk-sys` and `skyzfsd`, plain, with `test-hooks` and with `control-test-program`;
- the tests of `skyzfs-ublk` (142 in the lib) and of `skyzfsd`'s lib (63 with `test-hooks`, 51 without).

I planted eleven changes (table below). Two pass every test; both are suggestions below, not defects.

### Blocking: the last commits of a stop are sent by `flush()`, which throws their refusals away

**Where.** `Pump::turn`, the end of a stop (`pump.rs`, after `commit_finished()`), and `KernelQueue::flush` in `crates/skyzfs-ublk-sys/src/queue.rs`.

**What happens.**
- When a stop has drained, the pump hands its last commits on with `self.queue.flush()` and returns `Stopped::Orderly`.
- `KernelQueue::flush` reaps with the handler `|_, _, _| {}`. Its own doc says why: "A command that the driver ends otherwise than OK in it is not handed on either: this is the pump's last call."
- The driver refuses such a commit when it is submitted (the hook's module doc says so too). Whether that flush reaps the refusal or it is left on the ring, nothing reads it: no turn follows.

**The scenario.**
1. `serve --refuse-one-commit write:B`. A writer's request at B is in the store when `SIGTERM` comes.
2. The pump drains. B's write completes in the stop's last turn, and `commit_finished()` commits it. The hook sends that commit with no buffer.
3. `drained` is true, so `flush()` submits it. The driver refuses it, and the flush drops the refusal.
4. The pump returns `Orderly`, and `ended(n, true, Orderly)` gives 0.

So serve says that it stopped cleanly while B's request is unanswered. That is the case reading 4 rules out, and the stop's doc says the opposite: "also while the pump drains for a stop: that stop is not orderly". The new test's stop case does not see it, because there the refusal comes in a wait before the drain ends.

**Why it blocks.** Every stop that has requests in flight ends this way. A course of M1.T35 that refuses a request during an orderly stop would read 0 and judge the take-over after it as one that followed a clean stop. Reading 4 is a lead's ruling, and the code does not hold it for the last commits.

**One way to fix it.** Have `flush` hand on what it reaps, as `wait` does: for example `flush(&mut self, unfetched: &mut Vec<Unfetched>)`. Then `turn` checks `refused_commit` on that before it says `Orderly`, and returns `CommitRefused`. The fake queue's `flush` can report a refused commit, and a test like the new one's stop case, with the refusal at the flush, would hold it. Fetches that complete in the flush stay as they are: their requests go to the next server.

### What you asked me to judge

**1. The order of checks in `Pump::turn`. Right.**
- QueueDown comes first. P2 moves the refusal ahead of it, and `commands_ended_by_a_wait_that_ends_down_are_counted` fails.
- Then the refusal, before the stop is noted and before `take_over`. P7 lets a stopping pump ignore it, and the new test's stop case fails.
- The failed volume's turn does not look at refusals. P3 makes it end on one, and both counting tests fail.
- **Not held by a test: "before anything of that wait is taken".** P4 checks the refusal after `take_over` and after the wait's requests go into the backlog, just before the submit loop. Every test passes.
  - What differs: `in_flight` counts the request handed over with the refusal, which is the next server's.
  - Also, a tag handed over against the rule in the same wait would end the pump as `BrokenRule` (7) before the refusal is seen.
  - Your hand plant put the check after the submit, which the tests catch.
  - **Suggestion:** in the new pump test, check `counters().in_flight` after the refusal. Add a wait that brings a refused commit and a tag handed over twice, and expect `CommitRefused`.

**2. Readings 2 to 5.**
- **2, a failed volume only counts:** held by `commands_the_driver_ended_otherwise_than_ok_are_counted` (P3). serve then ends with 3: the main thread sees `flags.failed` and takes the failure's path, so the delete answers the refused request. That holds even when the refusal and the failure come in the same turn, since `answer_if_failed` still runs after the pump's last turn.
- **3, QueueDown wins:** held (P2).
- **4, draining for a stop gives 8:** held only for a refusal that a wait brings. Not for the last commits (the blocking finding).
- **5, the first of several refusals is named: not held by a test.** P1 names the last refused commit of the wait, and every test passes. The new test's wait has one refused commit; the other two entries are an abort and a refused fetch. **Suggestion:** add a second refused commit after tag 0's, say on tag 3 with `-16`, and expect tag 0 to be named.

**3. Do the tests pin "nothing answered after the refusal, the refused tag not again"? Yes, as far as a wait goes.**
- The pump test checks that nothing is committed after the refusal: not tag 1's completion that waited, and not tag 0 again. It also checks that tag 2 never reaches the target.
- The serve test checks the same through `pumped` and `answer_if_failed`, as serve runs them: no answer after the refusal, tag 2 never in the store, the volume not marked failed.
- P5 (an abort taken for a refusal), P6 (a refused fetch ending the pump) and P8 (the error number not negated) each fail a test.
- What they do not reach is the stop's last flush (above).

**4. `refuse.rs`: the hook's queue below the request log. Right.**
- **The pump sees the refusal.** The no-buffer commit goes out through the library's `complete_io_cmd_unified`, so its user data is the library's commit-and-fetch. `ended()` reads its refusal as `Command::Commit`, which is what `refused_commit` looks for.
- **The order of the wrappers.** The record is above `Refusing`, so the log writes the commit as any other `commit` line and its refusal as an `ended` line, as the module doc says. The delays and holds are above both.
- **The kept buffer.** It is never named to the driver: the refused commit names 0, and the tag's place is left empty. Keeping it costs nothing, and dropping it would be safe as well.
- **Only once.** P10 (every request of the kind refused) and P11 (the commit also sent with its buffer) each fail `the_request_named_has_its_commit_sent_once_with_no_buffer`.
- **With a failed volume,** `failed_turn` commits with `-EIO` and no buffer. If the hook's request arrives then, its commit also goes out with no buffer. The refusal is only counted, and the delete answers it, as the module doc says.

### Also checked

- **The next server cannot start while this one's store still writes.** After 8 the queue's thread ends and its ring closes, so the device can be recovered. But the store's threads may still be finishing writes that were in flight. The volume's lock (`_lock`) is held until `run` returns, so a next `serve` of the same volume cannot start before this process is gone. That is the same as for 7.
- **No other code matches on `Stopped`.** The only matches outside the pump are `serve::ended` and two `Orderly` asserts in `tests/pump_store.rs`, so nothing sends the new stop down a catch-all.
- **One queue.** If a later milestone serves several queues, one queue's pump ending with 8 (or 7) while the others serve would leave the refused request unanswered with the process alive. `--queues` takes only 1 in M1, so this is a note for later, not a finding.

### Planted changes

One at a time. After each I ran the tests of `skyzfs-ublk`'s pump and `skyzfsd`'s serve (with `test-hooks`), or `skyzfsd`'s lib for the hook.

| plant | result |
|---|---|
| P1: the last refused commit of a wait is named | **nothing fails** (reading 5) |
| P2: the refusal is checked before QueueDown | `commands_ended_by_a_wait_that_ends_down_are_counted` fails |
| P3: a failed volume's turn ends on a refused commit | both counting tests fail |
| P4: the refusal is checked after the wait's requests are taken, before the submit | **nothing fails** |
| P5: an abort is taken for a refusal | the new pump test and `commands_the_driver_ended_otherwise_than_ok_are_counted` fail |
| P6: a refused fetch ends the pump too | `commands_the_driver_ended_otherwise_than_ok_are_counted` fails |
| P7: a refusal is ignored while the pump drains for a stop | the new pump test fails |
| P8: the error number is not negated | the new pump test and the serve test fail |
| P9: a refusal during an asked stop gives 0 | the serve test fails |
| P10: the hook refuses every request of its kind | `the_request_named_has_its_commit_sent_once_with_no_buffer` fails |
| P11: the hook also sends the commit with its buffer | the same test fails |

### CI on 3c496d5, when I wrote this

- **Green:**
  - the control rig on all three images (43581985663);
  - the replay rig's daemon jobs and "write into a hole" jobs on both images (43581985640);
  - ci.yml's test, lint and unit-arm64 (43581985629);
  - coverage and e2e.
- **Still running:** the daemon rig on all three images (43581985630).

---
_Generated by [Claude Code](https://claude.ai/code)_

## Review of M1.T31 at f3404e7

**It can merge once its rigs read as they should. Nothing blocks.** The four things you asked me to judge hold. I have five suggestions. The first two are a few lines each:
- a replay dispatch that can give an old build `--store` twice;
- a start refused after a kill that still reads as the rig's error.

I read `09959c2...f3404e7` and checked it against the runs of #69's last head (75c32a7). I ran every test of `tests/kernel`, `tests/lib` and `tests/bench` at the head, and all pass. I also planted six defects in the judge and in g.4's rule (table below).

### What you asked me to judge

**1. Nothing removed checked anything on the daemon.** Right.
- **The twelve lines.** At 75c32a7 the in-place daemon wrote NA for eleven of them on 24.04, 26.04 and 26.04-arm. The twelfth, `ctl_f_starts_refused`, ran only when check f was asked for by name.
- **The code that goes ran only for the log store:**
  - check c's third pass and `c_low_steps.py`;
  - the pre-fills;
  - the log sampling and `replayed_bytes`;
  - `check_a_zeroes`, `check_b_steps` and `check_b_short`;
  - check f;
  - the stop test's `recovered_unacknowledged`.
- **The zeros.** The claim of `pin_write_zeroes_through_a_checkpoint` has a part that is not the log's: zeros over data read back as zeros, with the data around them, after a cut. The suite's case `write_zeroes_hides_the_base` holds that part on the daemon through check c.
- **The two lines that stay.** `pin_c_kill_with_applied_unacknowledged` and `ctl_delay_before_apply` are NA on the daemon and judged on the stand-in. Right.
- **The words are not quite right for four of the twelve.** The description and `test_the_log_stores_pins_are_gone` call every removed line "WAITING on the stand-in". `pin_unstored_zero_a/b/e` and `pin_serve_after_kill_accepts_log` have no stand-in verdict in the base's table, so on the stand-in they were judged PASS:
  - the three `unstored` lines read the stand-in's own `unstored 0`, which holds nothing;
  - `pin_serve_after_kill_accepts_log` counted re-serves that exited 4. That leads to suggestion 2.

**2. The rule for builds from before M1.3t.** Right, as a test of the source.
- Every commit whose `cli.rs` has the option writes it `[--store`: all 13 commits that touch `cli.rs` from 3dd9288 (M1.3r, which made the in-place store) to the base. Before 3dd9288 the file does not name `--store`: a build has only the log store and nothing to choose.
- cd0e763 has `[--store log|in-place]`.
- `$RUNNER_TEMP/daemon-src` is the worktree that the build step made in the same job, in all three workflows. The daemon rig sets `RIG_SERVE_ARGS` only for a `daemon_ref`.
- The stand-in takes `--store in-place` and refuses `log`. A leftover `RIG_STORE` ends the rig with 64.
- Suggestions 1 and 3 are about the edges of this rule.

**3. g.4 without the log allowance.** Right.
- At 75c32a7 the start refused on `data=journal` read "files of the volume new or rewritten: none; gone or rewritten: none" on all three images. So no allowance was in use.
- A volume made by this daemon has no `log.*` file to allow.
- `test_the_log_files_are_no_longer_allowed` fails when the allowance is put back (N3 below).

**4. The settle with no log keys.** Right in both places.
- **The benchmark.** An absent `checkpoint_step`, `unstored` or `log_live` counts as at rest only with `held` 0 and the write count still for a second. A status that cannot be read has no `held`, so it never counts as at rest, and it ends in the 600 s error as before.
- **The replay rig** takes a 0 or no key. Suggestion 4 is about what else "no key" now lets through.
- Neither change has a test (suggestion 5).

### Not blocking

1. **A replay or hole dispatch can give an old build `--store` twice.**
   - The workflow prepends `--store in-place` for a `daemon_ref` from before M1.3t without looking at `serve_args`. `replay-rig.sh` and `hole-rig.sh` both accept `--store in-place` in `RIG_SERVE_ARGS`.
   - Scenario: dispatch with `daemon_ref` cd0e763 and `serve_args` `--store in-place --io-threads 1`. That is the form the pull request jobs passed until now, and someone running a control will reach for it.
   - The old build gets `--store in-place --store in-place …` and refuses it: cd0e763's `cli.rs` has "given more than once". The first start then fails with a usage error, which `replay-rig.sh`'s own comment says "reads as a verdict".
   - The fix is to prepend only when `serve_args` names no `--store`, as the benchmark does.
2. **A start refused after a kill reads as the rig's error.**
   - In `kill_test`, a re-serve that does not come back sets `KT_ERR`. `kill_verdict` makes that ERROR, so `pin_kill_under_load` and the three pins after it read ERROR, and the job exits 2: "not a verdict on the server".
   - Scenario: a daemon refuses its own volume as failed (exit 4) after a kill -9 under load. That is the false alarm the removed pin was for, and it suspends a healthy pool.
   - `check_b_stop_under_load` already FAILs the same thing after an orderly stop ("THE SERVER DID NOT COME BACK").
   - On the in-place daemon this is as it was at the base, where the pin was NA. This pull request removes `KT_REFUSED`, the one count that named it.
   - Proposal: when the server ran and exited with a refusal of its own (3, 4 or 5), FAIL `kill_under_load` with that exit status, and keep ERROR for what the rig could not do (no pid, the kill failed). Whether that goes here or in a follow-up is the lead's call.
3. **The benchmark tests the help for `--store` anywhere; the rigs test `cli.rs` for `[--store`.**
   - Today both match every build that has the option.
   - After M1.3t-b, a help text that still names `--store` in prose (for example "--store was removed") would make the benchmark put `--store in-place` on every daemon row of a build that refuses it.
   - Testing the help for `[--store` would keep the benchmark in step with the rigs.
4. **The replay rig no longer notices a status it could not read.**
   - `STEPS` comes from `skyzfsd status … || true`. A status that timed out or failed now passes as "none: the daemon's store has no log". At the base it was the rig's error ("gave no checkpoint_steps").
   - Asking for a key that every status has (`writes`) before accepting an absent `checkpoint_steps` would keep the check.
5. **The two settle changes have no test.** `test_copy_limit.sh` stubs the daemon. A stubbed `skz_stat` that returns no log keys, and one with no status at all, would hold the benchmark's half.

### Planted defects

`tests/kernel/test_daemon_check.py` at the head, one plant at a time:

| plant | result |
|---|---|
| N1: NA expected of the two lines on the stand-in too | 26 tests fail |
| N2: a line that the table does not know is not an error | `test_a_pin_the_table_does_not_know_is_a_rig_error`, `test_the_log_stores_pins_are_gone` fail |
| N3: `log.super` and `log.N` allowed again in g.4 | `test_the_log_files_are_no_longer_allowed` fails |
| N4: `f` a check again | `test_check_f_went_with_the_log_store` fails |
| N5: `pin_c_kill_with_applied_unacknowledged` out of `NOT_ON_THE_DAEMON` | nothing fails; the rig would show it, NA where PASS is expected being the rig's error |
| N6: the suite judged on the daemon by the stand-in's counts | 12 tests fail |

### CI

- When I wrote this, the daemon rig (run 43555391150), the replay rig, coverage, ci, and the control dispatch with cd0e763 (43555427037) were still running.
- The kernel probe and e2e are green.
- I will read the tables and the control when they finish.

---
_Generated by [Claude Code](https://claude.ai/code)_

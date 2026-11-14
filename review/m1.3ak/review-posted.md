## Review of M1.3ak at 6bc687f

**Nothing blocks.** The tests close S4 of #87's review.
- **Every plant that breaks the shared limit is caught** by a time that the plant cannot meet on any machine.
- **A right implementation passed 30 of 30 runs with every CPU busy.**

It can merge once the daemon rig and coverage, still running, are green. The daemon is unchanged, so their verdict should be the base's.

There are three small points, none blocking:
- the six-order test hangs rather than failing when the bound is gone (section 4);
- one row of the plant table rests on a race (section 4);
- one plant survives, harmlessly (section 3).

I read the diff and the product code it drives before the description. At 6bc687f with rustc 1.100.0:
- `cargo fmt --all --check` passes.
- `cargo clippy --workspace --all-targets -D warnings` passes, plain and with `skyzfsd/test-hooks,skyzfs-local/test-hooks`.
- `cargo test -p skyzfsd` passes: 62 tests plain and 75 with `test-hooks`, the three new ones in both.
- The three new tests passed 30 of 30 runs alone. They passed 30 of 30 more with four busy loops on the container's four CPUs.

### 1. Your point (1): the 1.5-limit bound is exact for the plants, and fair

**Exact.**
- In a plant, the first command's half limit is a `sleep`, and the second's whole limit is a `recv_timeout` from a deadline taken after the first returned. Neither ends early.
- So the plant's time is at least 1.5 limits, and `took < limit * 3 / 2` fails.
- Measured:
  - a deadline per command took 1.501 s in the three-ways test;
  - a second deadline for the failure's way after the stop took 1.5008 s.

**Fair.**
- A right implementation's time is the deadline plus the wake-up of one thread. `took >= limit` cannot fail, since `began` is taken before `Bounded::new` sets the deadline.
- The 500 ms of margin held 30 of 30 runs with every CPU busy.

### 2. Your point (2): the three ways and the orders the give-up can send

**Every sequence of bounded commands `give_up_unanswered` can send.**

The commands come from:
- `failed_at_end`, which sends `note_failure`'s open and then `delete_failed`;
- `give_up_start`;
- `lifecycle::delete`, which reads the info only after a refused delete.

| path | commands, in order |
|---|---|
| a failure before the delete, at a start or a take-over | open, delete, [info], stop |
| at a start, no failure | delete, [info], stop |
| at a start, a failure after the stop | delete, [info], stop, open, delete, [info] |
| at a take-over, no failure | stop |
| at a take-over, a failure after the stop | stop, open, delete, [info] |

**Consecutive pairs.** The pairs that can be sent one after the other are:
- open → delete;
- delete → info;
- delete → open, across the stop;
- info → open, across the stop: a refused delete at a start, then a failure after the stop.

**What the tests drive:**
- **The three ways drive the first three pairs.**
- **The fourth pair is not driven through `give_up_unanswered`, and no take-over way is driven with times.** #87's `a_start_with_no_answer_is_given_up_in_order` holds the take-over orders without times.

**No realistic code gets through.**
- **The six-order test** sends every command as the unanswered one after a late one. So any command that waits its own limit is caught there: the info by "delete then info", the open by "delete then open", the delete by "open then delete".
- **Way 3** holds the stop in between: a second deadline after the stop is caught there.
- **Code that breaks only the info → open path** would have to treat a refused delete specially. I see no reason for code to do that.

**A fourth way is optional.** A refused delete answered late, then a failure after the stop and an open that never answers, would drive the fourth pair.

### 3. Your point (3): what survives

**U4: the deadline counted from the first command sent, not from `Bounded::new`. It survives, as expected, and it is harmless.**
- **Why the tests cannot see it.** In every test the first command follows `Bounded::new` at once, and the tests' `stop` returns at once.
- **Where it would differ.** Only at a take-over, where the first command comes after the stop. The open and the delete would then get a whole limit after the stop's wait, not what is left of it.
- **The bound still holds.** The 45 s sum checked in #87's test (`START_LIMIT + ANSWER_LOOK × ANSWER_LOOKS + GIVE_UP_LIMIT`) already counts the stop's 2 s on its own. So a give-up stays within 42 s either way. It only goes against `Bounded`'s doc ("a deadline, `GIVE_UP_LIMIT` after this was made").
- **If you want it held:** a take-over way whose `stop` takes half a limit, with an open that never answers, would catch it at 1.5 limits.

Everything else I planted was caught (table below).

### 4. Two points about the tests

**(a) The six-order test hangs, not fails, when `Bounded` waits with no limit (U8).**
- With `recv()` in place of `recv_timeout(left)`, the three-ways test and #87's two give-up tests fail, since `given_up_with` and `given_up` wait for their thread with a limit.
- `the_give_ups_commands_share_one_limit` joins its order threads with none. The test binary never ends, and in CI the job would run on to its timeout. I stopped it after 240 s.
- The bound is still caught, but by other tests, and the failure reads as a hang. Waiting for each order on a channel with `recv_timeout`, as `given_up_with` does, would make it a failure.
- Suggested, not needed for the merge.

**(b) The row "a command sent after the deadline: (2) and (3)" holds deterministically only for (2).**
- **(2)** catches it by the third command's answer, which must say "was not sent". That holds in every run.
- **(3)** catches it only when the command sent too late has logged before the test reads the log. Each command logs from its own thread, and `recv_timeout` with nothing left returns at once without waiting for it.
- **Measured:** with that plant, (3) failed in 4 of 10 runs and passed in 6. Its log in one of the failures was `["holds 3", "delete 3", "stopped", "info 3"]`, with the late command's entry after "stopped".
- **For a right implementation the log is deterministic,** since nothing is sent after the deadline and each answered command logs before it answers.
- So this concerns only the description: it could say "(2); (3) when the late thread has logged".

### Planted changes

One at a time in the worktree, each against `serve::tests` with `--features test-hooks` (19 tests, #87's included), then restored.

| plant | fails |
|---|---|
| U1a: each command waits the whole limit (`recv_timeout(self.limit)`), the not-sent check kept | (2) and (3), each on time: "the give-up took 1.501047947s" |
| U1b: a deadline per command (`let left = self.limit`, your S4) | (2), (3), (4), and #87's `a_delete_with_no_answer_does_not_hold_...`; the info is sent after the delete |
| U2: a second deadline for the failure's way after the stop | (3) on time ("a failure after the stop: ... 1.500750044s"), and #87's no-answer test on its log |
| U3: commands sent after the deadline | (2), and #87's no-answer test; (3) only in 4 of 10 runs (section 4b) |
| U4: the deadline counted from the first command sent | **nothing** (section 3) |
| U5: the open sent around `Bounded` | (2), (3), and #87's two give-up tests: "unbounded holds 3" |
| U6: the info sent around `Bounded` | (2), (3), (4), and #87's no-answer test: "unbounded info 3" |
| U7: a delete with no answer read as done | (2), (4) on its line, and #87's no-answer test |
| U8: the answer waited for with no limit (`recv()`) | (3), (4), and #87's no-answer test; (2) **hangs** (section 4a) |

Here (2) is `the_give_ups_commands_share_one_limit`, (3) is `a_start_given_up_ends_within_one_limit_whatever_its_commands_wait_for`, and (4) is `a_delete_with_no_answer_gives_9_and_a_line_that_names_it`.

Your four plants match mine:
- your S4 is U1b;
- your second `Bounded` is U2;
- your command after the deadline is U3, with section 4b's caveat;
- I did not plant your renamed delete, since (4) compares the whole line word for word.

### CI on 6bc687f, when I wrote this

- **Green:**
  - ci.yml's test, unit-arm64, lint, chart-lint and s3-backends;
  - `skyzfs-ci.yml`'s test, unit-arm64 and lint;
  - e2e-tier-a;
  - the replay rig's daemon jobs on both images, both kinds, and its no-kernel job;
  - the daemon rig's table job.
- **Running:**
  - the daemon rig on the daemon (24.04, 26.04, 26.04-arm);
  - coverage.

---
_Generated by [Claude Code](https://claude.ai/code)_

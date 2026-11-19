## Second next look at #126 at e6d5570 (M2.5-1b)

I checked the author's answer to F4 to F6 of my next look at `1f591fa`
([comment](https://github.com/skyzfs/skyzfs/pull/126#issuecomment-6995602757))
against the code. There is one new commit on `1f591fa`: `e6d5570`,
"Uploader: publishes and the cut share the time while both are due".
It changes two files:

- `uploader.rs`: a small `Turns` that counts the time, the module docs,
  and five unit tests of `Turns`.
- `tests/uploader.rs`: a slow cut in the test remote (`cut_slow`,
  `Shape`), two new tests, and a tighter bound in the test of F1.

I confirmed the head with the API. CI built the pull request merged
with `0d42566`. Since then `zfs-topology` has moved to `26723a8`, which
changes only the notes, and the pull request still merges cleanly
(`git merge-tree`).

**Verdict: nothing blocks on my standard. One finding, F7, is for the
lead, because it is the kind of thing the lead decided blocks in F4.**

- **Settled.** F4, F5 and F6 are settled. Counting the turns in time
  works:
  - w6 goes up in 20.13 s, as at the base, where it took 1300 s at
    `1f591fa`.
  - In w4 all 64 publishes are made, where 34 expired at `1f591fa`.
  - Nothing starves, and there is no busy loop.
  - Both new tests fail with the rule of `1f591fa` (plant K2, the
    author's T1).
- **F7. The count starts again while a publish only waits (medium in
  effect, low in likelihood; certain in the simulation: x1, x4).**
  - The module docs say the count starts again "whenever either side
    has nothing to do", and that publishes "however many, slow the cut
    to half its pace and no more".
  - The code starts the count again whenever only one side is due. A
    publish that waits out its own backoff is not due, so the cut's
    claim to the time of the publish's last step is forgotten.
  - So with one or two publishes whose steps fail slowly, the cut gets
    only the publish's waits (1 s, 2 s, 4 s and so on) until a wait
    grows longer than a step. With one publish failing after 120 s,
    cut 2 (256 s of steps) is up at 1216 s, against 616 s with the
    count kept.
  - This is not a regression. `1f591fa` and the base do the same or
    worse for one publish. It is a case the docs and the answer claim
    and the code does not meet.
  - The fix is one line (drop the restart while only one side is due;
    keep the one at an idle cut's step), plus the first three cases of
    one unit test. The plant of that fix (K1) changes nothing else in
    the suite. In my scenarios it changes only x1 and x4, which it puts
    right.
  - **If the lead holds F4's standard ("publishes never hold up the
    uploads"; half the cut's pace at most), F7 blocks.** I recommend
    the fix here, with a test of x4's shape. Otherwise the docs should
    say "while a publish stays due".
- **Nits.**
  - The ratio of the halves is pinned only by the unit tests of
    `Turns`, not by the tests through the uploader.
  - One plant of mine (K7) survives. It is equivalent in the tests'
    world.

Both nits are below, and neither needs action.

### What I did

- **Reading.** I read `e6d5570` against `1f591fa` and checked the
  answer's claims against it:
  - the count is kept from just before the step to its end;
  - a renewal of the lease before the step is not counted;
  - ties go to the side that did not step last.
- **Earlier scenarios.** I reran my 50 scenarios of #122 and #126 at
  the head (`r1` to `w6`).
- **New scenarios.** I added four (`x1` to `x4`) on the count:
  - `x1` and `x4`: how the time is shared with one to three publishes
    that fail slowly;
  - `x2`: a cut step of ten minutes;
  - `x3`: a busy loop.
- **Comparisons.** I ran `x1` to `x4` again at `1f591fa` and at the
  base `ef47c06`, and with the head's restart removed (K1).
- **Plants.** I reran 14 of my 21 earlier plants as they were. I
  remade the seven on the turns (T01 to T07) for the new rule, and
  made nine new ones on the count (K1 to K5 and K7 to K10; there is no
  K6).
- **Gates, CI and Codex**, at the head.

### (1) The bound, and the scenarios again

"Each side has half the time, give or take one step of the other's" is
the bound I meant, and the code reaches it while some publish is due
all the time. That holds in w6, in v1, and in x1 with three publishes.
Where publishes wait between their steps, see F7.

At `e6d5570`, with `1f591fa`, `ec02de6` and the base `ef47c06` for
comparison:

| Scenario | What it sets up | `e6d5570` | `1f591fa` | `ec02de6` | `ef47c06` |
|---|---|---|---|---|---|
| v1 | N publishes failing after T, then a cut of one slice | 20.004 s for N = 1, 5, 10 and 20 at 20 s; 5.004 s for 20 and 64 at 5 s | 20.005 s; 5.005 s | 720 s to not in 30 minutes | 20 s; 5 s |
| w6 | ten publishes failing after 20 s, cut 2 of 130 quick steps; expiry an hour, then ten minutes | 20.13 s and 20.13 s (two publish steps) | 1300 s; 600.2 s | 1920 s; 600.2 s | 20.1 s; 20.1 s |
| w4 | 64 publishes that can be made, cut 2 of 124 steps of 10 s | 64 made, none expired | 30 made, 34 expired | 64 made | 64 made |
| w2 | one publish, cut steps of 10 s | made after 10.0 s, `PCP` | 10.0 s, `PCP` | 2 ms, `PP` | 2 ms, `PP` |
| w3 | one publish with an expiry of 10 s, cut steps of 40 s | expired, `PCC` | expired | made | made |
| w1 | eight publishes that can be made, cut 2 of 62 steps of 4 s | made by 4.0 s, `PCPCPPPP...`; cut 2 at 256 s | 56 s, `PCPC...` | 16 ms, `PPPP...` | 16 ms |
| v2 | one publish refused at once, the cut idle, ten minutes | 2436 turns, at most 3 without a wait, 10 sends | the same | passes | passes |
| v5 | a younger publish that fails while an older one waits for a cut | its check passes; 4 turns without a wait (my bound of 3 was `ec02de6`'s pattern) | the same | passes | passes |

In w4, cut 2 is up at 1280 s in all four columns.

- **w1 now fails by design.** Its assertion was written for turns
  counted in steps. The simulation's publishes take no time, so after
  one cut step of 4 s they have 4 s of steps of no time, which is all
  sixteen of them. That is the rule.
- **w2 and w3 are the bound itself.** A publish's lookup and its
  transaction are two steps. A cut step that is due comes between
  them: the lookup takes some time, the cut has had none, so the cut
  goes. A publish whose expiry is shorter than one cut step can expire
  that way (w3). x2 shows the far end:
  - cut steps of ten minutes (two PUTs of five), and a publish asked
    meanwhile with an expiry of eleven minutes;
  - it is made after 600 s, `PCP`, the same at `1f591fa`;
  - with the default expiry of ten minutes it would expire.

  That needs a link far slower than any we plan for, and it is what
  "give or take one step of the other's" means. It is not a finding.
- **The rest are as before.** `n1` and `n2` fail as at #122 (F1's
  remainder there, outside this package). All the others pass.

### (2) The reset rule: F7

**What the code does.** `Turns::next` (`uploader.rs` 117-134) starts
the count again in the `else` branch, that is, whenever not both sides
are due. A publish is not due while it waits out its own backoff
(`publish_due` is false until `retry_at`), and the cut is not due
while it waits out its own. Time counts only while both are due
(`took`), so a wait adds nothing to either side. What the restart
does is wipe the lead one side has built up.

**What the docs and the answer say.**

- The module docs (`uploader.rs` 39-48) say the count starts again
  "whenever either side has nothing to do (an idle cut's step is
  that)".
- The answer says "a turn where only one is due".
- The unit test `the_count_starts_again_while_only_one_side_is_due`
  calls "the cut waiting in its backoff" nothing to do, and its
  comment says "nor is time from before".

But a publish in its backoff has something to do: it steps when its
wait is over. That is where the docs' "however many, ... no more"
fails.

**The case.** One publish that fails after T, and a cut under way whose
steps are quick:

1. The publish takes its step (T) and fails, then waits 1 s.
2. Only the cut is due, so the count starts again, and the cut steps
   for that one second.
3. Both are due on a tie, and the cut stepped last, so the publish
   goes: T again, then a wait of 2 s.

So the cut gets the publish's waits, 1 s, 2 s, 4 s and so on, against
T for each publish step. That lasts until a wait passes T, about
log2(T) publish steps, and only after that does the cut have half the
time. With two publishes, their steps sometimes overlap the other's
wait and sometimes do not. With three or more at 20 s, one is always
due and the count is never wiped.

**x1.** The setup:

- one to three publishes failing after 20 s, carried by cut 1, with an
  expiry of an hour;
- cut 2 of 124 slices of one block, each step two PUTs of 1 s: 256 s of
  steps.

The table gives when cut 2 is up and, in brackets, the publishes'
worst lead: their time minus the cut's, at any turn.

| Publishes | `e6d5570` | with K1 (the count kept) | `1f591fa` | `ef47c06` |
|---|---|---|---|---|
| 1 | 416 s (78 s) | 396 s (20 s) | 416 s (78 s) | 436 s (88 s) |
| 2 | 556 s (46 s) | 516 s (20 s) | 616 s (222 s) | 436 s (88 s) |
| 3 | 516 s (20 s) | 516 s (20 s) | 796 s (364 s) | 436 s (88 s) |

- **The head.** In the first 120 s the cut had 24 s and the one
  publish 100 s. With K1 each had 60 s.
- **The base.** It is quicker for two and three publishes because they
  shared one wait, which D2 undid on purpose. With waits of their own,
  half the time is the most they should take, and with K1 they take
  exactly half (256 s and 260 s).

**x4.** One publish with slower steps, and the same cut:

| One publish failing after | `e6d5570` | with K1 | `1f591fa` | `ef47c06` |
|---|---|---|---|---|
| 60 s | 736 s (lead 344 s) | 496 s (60 s) | 736 s (344 s) | 796 s (356 s) |
| 120 s | 1216 s (lead 818 s) | 616 s (120 s) | 1216 s (818 s) | 1336 s (832 s) |

At 120 s the cut had 24 s of the first ten minutes and the publish
600 s. With K1 it had 246 s, nearly the whole cut. At half pace, cut 2
should be up by about twice its 256 s of steps and one publish step:
572 s and 632 s. K1 is within that (496 s and 616 s); the head is not
(736 s and 1216 s).

**How likely, and how much.**

- **Likelihood.** It is F1's: publish steps that fail slowly while the
  cut's succeed. Here it is one or two publishes rather than many,
  which is if anything the more common case.
- **Effect.** The cut's pace falls far below half for about log2(T)
  publish steps, then recovers. With T of 20 s that costs 20 s (one
  publish step). With T of 120 s it costs ten minutes.
- **Not new.** The base and `1f591fa` do the same for one publish.
- **The mirror.** The same restart lets a cut whose steps fail slowly
  give the publishes only its own waits. I did not measure that.
  Publishes keep no pace, only an expiry, and quick publishes make
  much use of a 1 s wait, so it weighs less.

**A fix.** Remove `self.restart()` from the `else` branch of
`Turns::next`, and keep the restart at an idle cut's step. That is
plant K1:

- **Why it holds.** Time still counts only while both are due, so a
  wait is owed to no one. Across a wait, the lead carried over is at
  most one step, by the same argument as the unit test of 1000 steps:
  in a shared turn the side behind goes, so the gap never passes one
  step.
- **What changes.** With K1, x1's worst lead is one publish step
  (20 s) for one, two and three publishes, and x4's is one step too.
- **What does not change.** The whole suite passes except that one
  unit test. All my other scenarios give what they give at the head:
  v1, v2, w1 to w6, x2 and x3 to the millisecond.
- **The cost.** A lead of up to one step outlives a wait, until the
  cut is next idle. After x2's cut step of ten minutes, for example,
  the publishes keep ten minutes of credit. That is within "give or
  take one step of the other's".
- **A refinement, if the lead wants it.** Also start again when no
  publish has a step to take at all, now or later. That is what
  "nothing to do" means for the publishes.
- **The tests to change.**
  - `the_count_starts_again_while_only_one_side_is_due`: its first
    three cases would expect the lead from before the wait to be made
    up (`C` × 20 after a publish step of 20 s, `P` × 19 after a cut
    step of 20 s). The fourth, an idle cut's step, stays as it is.
  - A test of x4's shape would pin it: one publish failing after
    120 s, cut 2 of 124 slices with PUTs of 1 s, cut 2 up within twice
    its steps' time and one publish step (about 632 s). The head takes
    1216 s, and K1 616 s.

**What else I looked for, and did not find.**

- **Starvation.**
  - In a shared turn the side behind goes, ties alternate, and after a
    restart a tie goes to the side that did not step last. So a side
    waits at most one of the other's steps.
  - The plants that break this are each caught by 15 to 17 tests: T02
    (on a tie, the cut), T03 (the cut first whenever due) and K8 (on a
    tie, the same side again).
- **A busy loop.**
  - x3 has one publish refused at once and the cut idle, for ten
    minutes. It makes 2436 turns, at most 3 in a row without a wait,
    and 20 publish calls (10 sends), as v2 does and as at `1f591fa`.
    The base makes 2426 turns, at most 2.
  - The count never makes a turn return zero by itself: a zero comes
    only after a step, and each side's own wait is untouched.
- **Steps that take no time.** The counts stay equal, and ties
  alternate as at `1f591fa` (`steps_of_the_same_time_take_turns`).
- **The clock.**
  - `Instant` is monotonic, and `saturating_duration_since` counts a
    step that seems to end before it began as nothing.
  - The uploader's sleeps between turns are not counted.
- **A step that spans a very long time.** x2, above. The cut's lead of
  600 s goes as soon as no publish is due.

### (3) F6, the new tests and the plants

**F6 is settled.**
`publishes_that_fail_slowly_hold_the_cut_back_for_one_step_at_most` now
bounds cut 2 by one publish step and a second: 21 s, 21 s and 6 s. The
long cut has its own test.

**The two new tests.** I read them against w6 and w4, and they are my
scenarios:

- **`publishes_that_fail_slowly_slow_a_long_cut_by_one_step_at_most`**
  - ten publishes failing after 20 s, and cut 2 of 124 slices;
  - cut 2 must be up within 21 s, after more than 124 turns;
  - then, with the cut idle, at least five sends in two minutes.
- **`slow_steps_of_the_cut_slow_the_publishes_by_one_step_at_most`**
  - cut 2 of 40 slices, steps of 10 s;
  - 64 publishes, all made within 11 s and before cut 2 is up;
  - then 64 published and none expired.

K2, which counts the turns in steps (the rule of `1f591fa`, the
author's T1), fails both, as the author says, and three of the unit
tests as well. Each also fails with K4 (a step's time goes to the
other side). Of the tests through the uploader, only the long cut's
test catches K9 (no restart at an idle cut's step) and K3 (no restart
at all). An idle cut's step takes next to no time, so without the
restart the cut stays behind and takes every turn, and the publishes
do not step again.

Neither test can see F7:

- the long cut's test has ten publishes, so one is always due;
- its cut steps take no time, and so do w4's publishes.

**The five unit tests of `Turns`** pin what the tests through the
uploader cannot:

- the ratio of the halves. T04 (the publishes get eight times the
  cut's time), T06 (twice) and T07 (the cut twice) are each caught by
  the five unit tests and by no test through the uploader, whose slow
  side always faces a side with steps of no time;
- the tie: T02 and K10, the "off by one" of the comparison, are caught
  by 15 and by 4 tests;
- the restart: K1 and K5 are caught by
  `the_count_starts_again_while_only_one_side_is_due` alone.

`Turns` is pure, so its unit tests are the right place for the ratio.
That is the first nit, and it needs no action.

**Plants.** Each was made alone in the scratch worktree, `cargo test -p
skyzfs-upload` was run, and the file was restored. A plant that
survives is also run against my scenarios. The baseline is the head's
failing scenarios: n1, n2, v5, w1 and x1.

| Plant | What it does | Result at `e6d5570` |
|---|---|---|
| E09 | the newest publish with work is stepped first | caught: `each_publish_waits_alone_after_its_own_failures` |
| N07 | a failure's wait goes to every pending publish | caught: 2 tests |
| N09 | the uploader's sleep ignores when a publish may step | caught: 2 tests |
| D2g | a publish that waits for a cut has a time to step | caught: 2 tests |
| M01, M02 | the same request asked again leaves the withdrawal, or clears an expiry too | caught: `a_withdrawn_publish_asked_again_with_the_same_request_goes_on` |
| M03 | a lookup that finds the name absent resets the publish's wait | caught: 5 tests |
| M04 | no publish steps while the cut waits in its backoff (moved to the `turns.next` line) | caught: `a_publishs_wait_wakes_the_uploader_while_the_cut_waits_longer` |
| M05 | a failed step's wait goes to the first pending publish | caught: `a_failed_steps_wait_goes_to_the_publish_that_took_it` |
| L01 to L05 | a withdrawn or expired publish kept for a cut | caught: the lapse test; L01 to L03 also a test of `publish/tests.rs` |
| T01 | `shared` always false: a publish due always first (the rule at `ec02de6`) | caught: 8 tests, among them the tests of F1 and F4 |
| T02 | on a tie, the cut | caught: 15 tests |
| T03 | the cut first whenever it is due | caught: 17 tests |
| T04 | the publishes get eight times the cut's time | caught: the five unit tests only |
| T05 | an idle cut takes no turn | caught: the suite hangs, 17 failed before the limit of 300 s |
| T06 | the publishes get twice the cut's time | caught: the five unit tests only |
| T07 | the cut gets twice the publishes' time | caught: the five unit tests only |
| K1 | no restart while only one side is due (F7's fix) | caught: `the_count_starts_again_while_only_one_side_is_due` only |
| K2 | a restart at every turn: turns counted in steps (the author's T1) | caught: both new tests and 3 unit tests |
| K3 | no restart ever | caught: the long cut's test and the restart's unit test |
| K4 | a step's time goes to the other side | caught: 8 tests |
| K5 | a step's time counts while only one side is due | caught: the restart's unit test only |
| K7 | a step's time runs to the end of a renewal that ends after it | **survives**; scenarios as at the head |
| K8 | on a tie, the side that stepped last goes again | caught: 15 tests |
| K9 | no restart at an idle cut's step | caught: the long cut's test |
| K10 | on a tie, a publish | caught: 4 tests |

**Counts.** 29 of the 30 plants are caught.

- **The earlier set.** My last note said "All 22 caught", but it had 21
  plants: the 22nd was the first T04, whose count was shared between
  tests and which I dropped. All 21 are caught again here, the T
  plants in their new form.
  - At `1f591fa`, T04, T06 and T07 made the cut wait for eight, three
    and four publish steps in a row. With turns in time they become
    weights.
  - Their new form is caught by the unit tests only, for the reason
    above.

**K7 survives (the second nit).** It counts a step's time to the end
of a renewal that falls due during the step and ends after it.

- In the tests the lease keeper answers at once, so no renewal ends
  after its step, and K7 is equivalent there.
- The head's choice (the step's own end, `ended`) is the right one,
  since a slow renewal is neither side's time.
- The difference is one renewal at most. It needs no test.

### Gates

At `e6d5570`, with rustc 1.100.0 (2026-11-09), the current stable:

- `cargo fmt --all --check` is clean.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean in
  all three builds: plain, with `--features
  skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with `--features
  skyzfs-ublk-sys/control-test-program`.
- None of the tests failed:
  - `cargo test -p skyzfs-upload`: 137 passed (130 at `1f591fa`, plus
    two new tests and five unit tests);
  - `cargo test -p skyzfsd`: 106 passed;
  - `cargo test -p skyzfsd` with the test hooks: 142 passed.

### CI and Codex

- **CI on `e6d5570`.** There are 22 checks: 18 green and 4 skipped,
  none failed and none running. The green ones include:
  - `test` and `unit-arm64`;
  - lint and coverage;
  - `e2e-tier-a` and `s3-backends`;
  - the three jobs "record through skyzfsd and check";
  - the two k3s smoke jobs.

  CI's merge ref was built on `0d42566`, and `zfs-topology` has since
  changed only the notes.
- **Codex.** It reviewed `e6d5570` (code review and security review
  completed) and has no finding. Nothing is posted on the diff, and
  there are no reviews.
- **Not committed.** The scenarios and plants are not committed. They
  were run in a scratch worktree of the head, which I moved to
  `1f591fa` and to `ef47c06` for the comparisons of x1 to x4. The
  worktree was clean after every run.

## Third next look at #126 at 5b526ac (M2.5-1b)

This looks at the author's answer to F7 of my second next look at
`e6d5570`
([comment](https://github.com/skyzfs/skyzfs/pull/126#issuecomment-6997635129)),
checked against the code. There is one commit on `e6d5570`: `5b526ac`,
"Uploader: a wait keeps what a side is owed of the shared time".

- I confirmed the head with the API. The pull request is mergeable and
  GitHub calls its state clean.
- `git merge-tree` against `zfs-topology` at `d646117` finds no
  conflict.
- CI ran on the merge with `d646117`, the current tip. #128, which
  brought `zfs-topology` there, touches only `tests/bench`,
  `tests/lib`, the docs and the workflows.

**Verdict: can merge. Nothing blocks.**

- **F7 is fixed as I proposed (my plant K1).**
  - With one publish whose step fails after 120 s, cut 2 is up at
    616 s, where it took 1216 s at `e6d5570`.
  - With one to three publishes failing after 20 s, the publishes lead
    by one step at most.
  - Nothing else changes in my scenarios.
- **The count kept across a wait.** I looked for a new way for a side
  to starve, to hog, or to turn without a wait, and found none.
  - In a check of `Turns` over 1.1 million random turns, the two counts
    never differ by more than the longest step counted, and a run of
    one side's steps never exceeds two such steps.
  - The same fix also mends the mirror of F7 (cut steps that fail
    slowly).
- **The tests.**
  - Both changed tests are right and catch what they should.
  - My 29 plants that still apply, the author's 25 and three new ones
    of mine are all caught, except two:
    - K7, as before; it is equivalent in the tests' world.
    - K13, new: the count started again at a failure of the cut's
      step. No test of the suite pins the mirror. My scenario y3
      catches it. That is a nit, not a blocker.
- **Gates, CI and Codex.** The gates are clean. CI has 18 green and 4
  skipped, none failed. Codex has no finding.

### (1) What 5b526ac changes

`git diff e6d5570 5b526ac` touches two files, 93 lines in and 15 out.
Every hunk belongs to F7:

| File | Hunk | What it does |
|---|---|---|
| `uploader.rs` | module docs, the paragraph on publishes | Time counts only while both sides are due. A wait does not wipe what one side is owed. The count starts again only at an idle cut's step. |
| `uploader.rs` | doc of `Turns::next` | While only one side is due, its steps are not counted, and the count from before is kept. |
| `uploader.rs` | `Turns::next`, the `else` branch | the line `self.restart();` removed: the whole fix, my K1 |
| `uploader.rs` | doc of `Turns::restart` | "The cut has nothing to do (an idle cut's step)" |
| `uploader.rs` | the unit test | `the_count_starts_again_while_only_one_side_is_due` becomes `a_wait_keeps_the_count_and_an_idle_cuts_step_starts_it_again`; its first three cases change, the fourth stays |
| `tests/uploader.rs` | after the long cut's test | the new test `one_publish_that_fails_slowly_slows_a_long_cut_to_half_its_pace_at_most` and its helper `a_long_cut_beside` |

Nothing else changes: no remote, no other test and no other code.
Outside the unit tests, `restart()` is now called in one place only,
at `Ok(Step::Idle)` (line 376), and the docs say so.

### (2) The rule, and the scenarios again

`Turns::next` no longer starts the count again in its `else` branch,
which is what happens while only one side is due or neither is. Time
still counts only while both are due (`took` returns at once where
`shared` is false). The one place the count starts again is an idle
cut's step.

**The F7 scenarios.** Cut 2 is 124 slices, each step two PUTs of 1 s,
256 s of steps. Each cell gives when cut 2 is up, and in brackets the
worst lead of the publishes.

| Scenario | `5b526ac` | `e6d5570` |
|---|---|---|
| x1, one publish failing after 20 s | 396 s (20 s) | 416 s (78 s) |
| x1, two | 516 s (20 s) | 556 s (46 s) |
| x1, three | 516 s (20 s) | 516 s (20 s) |
| x4, one publish failing after 60 s | 496 s (60 s) | 736 s (344 s) |
| x4, one failing after 120 s | 616 s (120 s) | 1216 s (818 s) |

These are the numbers my plant K1 gave at `e6d5570`, to the
millisecond. x1 passes now: the publishes lead by at most one of their
steps. In x1's first 120 s the cut and the publishes have 60 s each.

**The other scenarios at 5b526ac** match `e6d5570` to the millisecond,
except where noted:

| Scenario | What it sets up | `5b526ac` |
|---|---|---|
| v1 | N publishes failing after T, then a cut of one slice | 20.004 s for N = 1, 5, 10 and 20 at 20 s; 5.004 s for 20 and 64 at 5 s |
| v2 | one publish refused at once, the cut idle, ten minutes | 2436 turns, at most 3 in a row without a wait, 10 sends |
| v5 | a younger publish that fails while an older one waits | its check passes; 4 turns in a row without a wait (my bound of 3 is `ec02de6`'s pattern, as before) |
| w1 | eight publishes that can be made, cut 2 of 62 steps of 4 s | made by 4.0 s; fails by design, as at `e6d5570` |
| w2 | one publish, cut steps of 10 s | made after 10.0 s, `PCP` |
| w3 | expiry 10 s, cut steps of 40 s | expired, `PCC` (the bound itself) |
| w4 | 64 publishes, cut 2 of 124 steps of 10 s | 64 made, none expired; cut 2 up at 1280 s |
| w5 | the lapse test's control | both published |
| w6 | ten publishes failing after 20 s, a cut of 130 quick steps | 20.13 s with either expiry |
| x2 | a cut step of ten minutes, a publish with an expiry of eleven | made after 600 s, `PCP` |
| x3 | one publish refused at once, the cut idle, ten minutes | 2436 turns, at most 3 in a row without a wait, 20 publish calls |

`n1` and `n2` still fail, as at #122. They are F1's remainder there,
outside this package.

**What keeping the count across a wait could break.** I looked for
four things.

**1. A lead larger than one step.** It cannot happen, by induction:

- The lead changes only in a counted step.
- In a counted step, the side behind (or tied, and not the last to
  step) goes and adds its step `t`.
- So the new lead is at most the larger of the old lead and `t`.

The lead therefore never passes the longest step counted since the
count last started, however many waits come between.

To check the reasoning, I added a test inside the module (`p1`; it
reaches the private `Turns`):

- 400 runs of 3000 turns each;
- each side due three times in four;
- steps of no time, of up to 10 s and of up to ten minutes;
- an idle cut's step one cut turn in 25.

What it found:

- 1,124,813 turns, 673,776 of them with both sides due.
- The choice is always the side due, or the side behind, or on a tie
  the side that did not step last.
- The lead reaches 1.000 of the longest step counted, and no more.
- A run of one side's counted steps, the other due all the while,
  reaches 1.976 of that step. The bound is two.

With the rule of `e6d5570` (plant aF7a) the same test gives 1.000 and
1.980.

**2. A lead from long ago (y1).** The setup:

1. Cut 2's steps take 300 s.
2. A publish that can be made is asked. One cut step comes between its
   lookup and its transaction, so the publishes are owed about 300 s.
3. The cut then takes two steps alone, 600 s.
4. Three publishes failing after 120 s are asked.

What happened:

- The cut, due all the while, waited 360 s at most between two of its
  steps: three lookups of no time and three transactions of 120 s. My
  bound is one cut step and one publish step, 420 s.
- With the rule of `e6d5570` it waits 360 s as well. There, the cut's
  next step of 300 s makes the same lead at once.

So a lead carried across a wait costs the cut at most what the stated
bound allows, "one step of the other's". The cost of keeping the count
is that the lead can come from a step long past: as long as the cut is
not idle, up to the longest cut step counted since then. On a volume
that never goes idle, that step can be an hour old. It is still one
step, and the author declined my refinement for that reason; I agree
it is not needed.

**3. Due again after a long idle (y2).** The setup:

- One or three publishes failing after 120 s pend while the cut is
  idle for half an hour, so their waits grow to five minutes;
- then cut 2 is written.

What happened:

- Cut 2 is up in 256.13 s alone, 376.13 s beside one publish and
  496.13 s beside three. The bound is 632 s.
- The publishes still pend afterwards (the expiry is an hour).
- The rule of `e6d5570` gives the same numbers: each idle cut's step
  starts the count again, so nothing is carried into the cut.

**4. The mirror of F7, cut steps that fail slowly (y3).** The setup:

- Cut 2 of 124 slices is under way when its PUTs start to fail, each
  after 60 s, the cut waiting 1 s, 2 s, 4 s and so on.
- Ten publishes that can be made, each transaction taking 10 s, are
  asked with the default expiry of ten minutes.

What happened:

- At `5b526ac` all ten are made after 220 s: the cut's steps 120 s,
  the publishes' 100 s. The bound is twice the publishes' time and one
  cut step, 260 s.
- With the rule of `e6d5570` (aF7a) it takes 460 s: the cut's steps
  360 s, the publishes' 100 s. None expired, so the effect there was
  mild.
- The fix mends the mirror too. In my last note I had only reasoned
  about this case, not run it.

**A wait or busy loop.** None:

- x3 and v2 make at most three turns in a row without a wait, as
  before.
- The count never makes a turn return no wait by itself: a turn
  returns zero only after a step, and each side's own wait is
  untouched.
- y2's publishes, waiting five minutes each, add no turns: the cut's
  idle turns come at its own pace (`IDLE`).

**Starvation and hogging.** A side due waits at most for the lead it
is behind plus one step of the other's. That is at most two steps of
the other's: p1's runs reach 1.976 of the longest step, and y1's cut
waited 360 s against 420 s. The plants that break the tie or the
comparison (T02, T03, K8, K10, aT5) are caught by 4 to 17 tests.

### (3) The changed and new tests, and the plants

**The unit test `a_wait_keeps_the_count_and_an_idle_cuts_step_starts_it_again`.**
I worked its four cases by hand:

- **Case 1.** A publish step of 20 s, then the publishes alone: the
  cut owes nothing for that, and is still owed 20 s. So it goes 20
  times, then the tie goes to the publish, then the cut: `C`×20, `P`,
  `C`.
- **Case 2.** A cut step of 20 s against a publish's 1 s, then the cut
  alone: `P`×19, then the tie to the cut, then `P`.
- **Case 3.** As case 1, with neither side due in between.
- **Case 4.** An idle cut's step: `CPCP`, unchanged.

Each expected string is what the rule gives. Planted:

- the rule of `e6d5570` (aF7a) fails it;
- so does each restart in one of the three cases alone (aF7b to aF7d);
- so does counting a step taken alone (K5), and no restart at all
  (K3).

**The new test `one_publish_that_fails_slowly_slows_a_long_cut_to_half_its_pace_at_most`.**

- **What it is.** It is my x4: one publish failing after 120 s, with
  the longest expiry, and cut 2 of 124 slices of one block with PUTs
  of 1 s. It measures the cut's time alone in the same test and bounds
  the time beside the publish by twice that and one publish step.
- **What it measures.** I ran it alone at the head: cut 2 up in
  252.128 s alone and 612.134 s beside, against a bound of 624.256 s.
  The author's figures agree.
- **What it catches.** It fails with:
  - the rule of `e6d5570` (aF7a);
  - a restart while the cut alone is due (aF7c);
  - a restart at a publish's failure (K11, a fix put in the wrong
    place);
  - turns counted in steps (K2, the author's T1);
  - the publishes given eight or two times the cut's time (T04, T06);
  - and with T01, K4, aT5, aT10 and aT11.
- **The margin.** It is 12 s, but the clock is paused, so the test is
  exact and does not flake.

The new test is the first test through the uploader that catches T04
and T06 (the ratio of the halves). Before, only the unit tests did:
the first nit of my last note is answered in passing.

**Plants.**

- **How they were run.** Each plant was made alone in the scratch
  worktree, `cargo test -p skyzfs-upload` was run, and the file was
  restored.
- **Survivors.** A plant that survives was run against my scenarios
  too.
- **The baseline.** The scenarios that fail at the head are n1, n2, v5
  and w1.
- **The author's plants.** Of the author's 25, I rebuilt from their
  descriptions the 12 that are not already mine. The other 13 are the
  same change as one of mine:
  - D1a is M01, D1b is M02, D2e is N09, D2g is D2g, F2a is M05;
  - T1 is K2, T2 is T01, T3 is T03, T4 is K8;
  - T7 is K9, T8 is K5, T9 is K4;
  - D2a is the same idea as N07 (one wait for all the publishes),
    not the same text.

| Plant | What it does | Result at `5b526ac` |
|---|---|---|
| E09, N07, N09, D2g, M01 to M05 | my plants of #122's D1 and D2, and F2 | all caught, by the same tests as at `e6d5570` |
| L01 to L05 | a withdrawn or expired publish kept for a cut | all caught: the lapse test, and L01 to L03 a test of `publish/tests.rs` too |
| T01 | a publish due always first (the rule at `ec02de6`) | caught: 9 tests, the new one among them |
| T02 | on a tie, the cut | caught: 15 tests |
| T03 | the cut first whenever it is due | caught: 17 tests |
| T04 | the publishes get eight times the cut's time | caught: the new test and the five unit tests |
| T05 | an idle cut takes no turn | caught: the suite hangs, 18 failed before the limit of 300 s |
| T06 | the publishes get twice the cut's time | caught: the new test and the five unit tests |
| T07 | the cut gets twice the publishes' time | caught: the five unit tests |
| K1 | no restart while only one side is due | does not apply: it is now the rule |
| K2 | a restart at every turn: turns in steps (the author's T1) | caught: 7 tests, among them the three through the uploader on F4, F5 and F7 |
| K3 | no restart ever | caught: the long cut's test and the changed unit test |
| K4 | a step's time goes to the other side (the author's T9) | caught: 9 tests |
| K5 | a step's time counts while only one side is due (the author's T8) | caught: the changed unit test |
| K7 | a step's time runs to the end of a renewal that ends after it | **survives**; scenarios as at the head; equivalent while the keeper answers at once, as before |
| K8 | on a tie, the side that stepped last (the author's T4) | caught: 15 tests |
| K9 | no restart at an idle cut's step (the author's T7) | caught: the long cut's test |
| K10 | on a tie, a publish | caught: 4 tests |
| aF7a | a restart while only one side is due (the rule at `e6d5570`) | caught: the new test and the changed unit test |
| aF7b | a restart while neither side is due | caught: the changed unit test |
| aF7c | a restart while the cut alone is due, a publish waiting | caught: the new test and the changed unit test |
| aF7d | a restart while the publishes alone are due, the cut waiting | caught: the changed unit test |
| aD2b | the oldest publish with work holds up the others while it waits | caught: 2 tests |
| aD2c | a publish's wait is not kept | caught: 7 tests |
| aD2d | the wait does not grow | caught: 5 tests |
| aD2f | the time a publish may step ignores its wait | caught: `each_publish_waits_alone_after_its_own_failures` |
| aD2h | a failed step gives its wait to no publish | caught: 6 tests |
| aT5 | the side that has had more time goes next | caught: 9 tests |
| aT10 | the uploader counts no time for a step | caught: the three tests through the uploader on F4, F5 and F7 |
| aT11 | the last step is never noted as a publish's | caught: 9 tests |
| K11 | a restart at a publish's failure (new) | caught: the new test and the long cut's test |
| K12 | the publishes may lead by a second (new; the off-by-one of the half) | caught: 6 tests |
| K13 | a restart at a failure of the cut's step (new; the mirror of K11) | **survives the suite**; my y3 fails with it (460 s against a bound of 260 s) |

**Counts.**

- **My earlier 30.** 29 still apply and 28 are caught; K7 survives as
  before.
- **The author's 25.** All are caught, and each by the tests the
  author names.
- **My three new ones.** K11 and K12 are caught; K13 survives.

**K13 (a nit).** The fix itself is in `Turns::next`, and the changed
unit test guards the cut's side of it (its second case, and aF7d). But
a restart put elsewhere is a different matter:

- At the cut's error arm in `turn`, which is K13, no test through the
  uploader sees it.
- On the publishes' side the same misplaced restart (K11) is caught by
  the new test.

A test of y3's shape would close that gap. I set it out here so the
author can choose:

- cut PUTs that fail after 60 s;
- ten publishes of 10 s each;
- all made within twice their steps' time and one cut step.

It does not block: the head is right, and the gap is in what would
catch a later change.

### Gates

At `5b526ac`, with rustc 1.100.0 (2026-11-09), the current stable:

- `cargo fmt --all --check` is clean.
- `cargo clippy --workspace --all-targets -- -D warnings` is clean in
  all three builds: plain, with `--features
  skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with `--features
  skyzfs-ublk-sys/control-test-program`.
- `cargo test -p skyzfs-upload`: 138 passed (137 at `e6d5570` and the
  new test; the unit test was replaced, not added).
- `cargo test -p skyzfsd`: 106 passed.
- `cargo test -p skyzfsd` with the test hooks: 142 passed.

None failed.

### CI, Codex and the merge

- **CI.** It ran on `5b526ac` with 22 checks: 18 green and 4 skipped,
  none failed and none running. The green ones include:
  - `test` and `unit-arm64`;
  - lint and coverage;
  - `e2e-tier-a` and `s3-backends`;
  - the three jobs "record through skyzfsd and check";
  - the two k3s smoke jobs.

  The jobs started after #128 had merged, so they tested the merge
  with the current `zfs-topology` (`d646117`).
- **Codex.** It reviewed `5b526ac` (code review and security review
  completed) and has no finding. Nothing is posted on the diff, and
  there are no reviews.
- **The merge.** The pull request is mergeable, its state is clean,
  and `git merge-tree` against `d646117` finds no conflict.

My scenarios and plants are not committed. They were run in a scratch
worktree of the head, which was clean after every run.

## Next look at #126 at 1f591fa (M2.5-1b)

The author's answer to F1 to F3 of my review at `ec02de6`
([comment](https://github.com/skyzfs/skyzfs/pull/126#issuecomment-6994105433)),
checked against the code. One commit on `ec02de6`: `1f591fa`, "Uploader:
a publish and the cut take turns while both are due". It changes
`uploader.rs` (the turns, 20 lines with the docs), one doc comment in
`publish.rs`, and `tests/uploader.rs` (two new tests, a slow refusal in
the test remote, and one expected sequence changed). I confirmed the
head with the API. The pull request merges cleanly.

**Verdict: a decision for the lead (F4); nothing else stands in the
way.** The turns are what I suggested, and they do what the module docs
say: a cut's step waits for one publish step at most, and a publish's
for one cut step at most. v1 now passes, and nothing starves either
way. But a bound per step is not a bound on the pace. While publishes
fail slowly, the cut takes one step per failed publish step, so a cut
of more than a few dozen slices is held back about as long as at
`ec02de6`: with the default expiry, until the publishes expire, ten
minutes, against 20 s at the base. The lead decided that publishes
never hold up the uploads, and for such a cut they still do. On my own
standard it would not block, any more than F1 did: it needs publish
steps that fail slowly while the cut's succeed. So: **it can merge if
the lead accepts a bound per step; if the lead holds to "never hold up
the uploads", F4 blocks.** I recommend fixing it here: it takes a few
lines more than the flag, and it fixes F5 as well. The hole is in the
fix I suggested, not only in its making. F2 and F3 are settled. The
changed sequence does not weaken the lapse test; it makes it stricter.

- **F4. The turns bound the cut's step, not its pace (medium; certain
  in the simulation, w6).** F1's remainder. Turns measured in time
  instead of in steps close it, and F5 with it.
- **F5. Slow cut steps now pace the publishes (low; w3, w4).** The
  mirror image, new in `1f591fa`: a publish waits one cut step between
  its lookup and its transaction, and a burst of publishes during an
  upload whose steps are slow can expire. Not blocking.
- **F6. The new test of F1 checks a looser bound than its name (nit;
  plants T06 and T07).**
- F1 to F3 of the review: F1 fixed for the case it named, F2 and F3
  fixed.

### What I did

- Read `1f591fa` against `ec02de6` and the answer.
- Reran my 44 scenarios of #122 and #126 (`r1` to `v5`) on the head,
  and added six for the turns (`w1` to `w6`), which I also ran at
  `ec02de6` and at the base `ef47c06`.
- Reran my nine plants of the review, with M04 moved to the new line,
  and made twelve new ones: seven on the turns (T01 to T07) and five
  that let a withdrawn or expired publish be carried by a cut (L01 to
  L05).
  I also ran the L and T plants against the changed test alone, to see
  which assertion each trips, and the T plants against the new test of
  F1 alone.
- Gates at the head, and CI and Codex.

### F1: the turns

`uploader.rs` 284-288: a turn takes a publish's step when one is due,
unless the last step was a publish's and the cut's step is due too
(`now >= step_at`). After a cut's step `published_last` is false, so a
publish due takes the next turn. So:

- **The cut is not starved by publishes.** A cut's step waits for one
  publish step at most (v1 below).
- **Publishes are not starved by the cut** (the other direction). w1:
  eight publishes that can be made, carried by cut 1, while cut 2 of
  62 slices goes up, each step 4 s. At the head the steps go
  `PCPCPCPCPCPCPCPCPCPCPCPCPCPCPCP` and the eight are made by 56 s,
  long before cut 2 (256 s). At `ec02de6` and the base they go first,
  `PPPPPPPPPPPPPPPP`, and are made at once.
- **While the cut waits in its backoff, publishes step back to back**
  (`now < step_at`), as before. M04 is still caught.
- **An idle cut takes its turn too.** An idle step sets no `step_at`,
  so it is due again at once: each publish step is followed by a cut
  step that asks the local store for a cut and finds none. That is the
  cost the author names. No busy loop: v2 (one publish refused for ten
  minutes, the cut idle) sees at most three turns in a row without a
  wait, as at `ec02de6`. v5 sees four where it saw three, since its
  younger publish now has an idle check after each of its steps. Each
  of those turns does work, and my bound of three in v5 was `ec02de6`'s
  pattern, not a property: v5's own check (the younger publish waits,
  six sends in a minute) passes.
- **The wait and the deadline.** `next_wait` is unchanged and still
  takes the first publish's due time; after a publish's step the turn
  returns zero, and the next turn picks by the rule above. A publish
  whose deadline passes while the cut takes its turn ends at the next
  turn's `expire`, and is not made (w3). That is right by the rules, and
  new: see F5.

v1 rerun (N publishes carried by cut 1, each refused after T seconds,
then cut 2 written; a cut of one slice):

| N publishes, failing after T | cut 2 up at `1f591fa` | at `ec02de6` | at the base `ef47c06` |
|---|---|---|---|
| 1, 20 s | 20.004 s | 20 s | 20 s |
| 5, 20 s | 20.005 s | 720 s | 20 s |
| 10, 20 s | 20.005 s | not in 30 minutes | 20 s |
| 20, 20 s | 20.005 s | not in 30 minutes | 20 s |
| 20, 5 s | 5.005 s | 820 s | 5 s |
| 64, 5 s | 5.005 s | not in 30 minutes | 5 s |

The author's table agrees (20.006 s and 5.006 s with their test's
remote).

**F4. The turns bound the cut's step, not its pace (medium; certain in
the simulation; w6).** The cut takes one step for each publish step,
and while N publishes each fail after T seconds, one is due nearly all
the time (that is what F1 was), so the cut moves one slice every T
seconds. v1 has a cut of one slice, which goes up after one publish
step. w6 has ten publishes failing after 20 s and a cut 2 of 124
slices of one block, 130 quick cut steps in all:

| Expiry of the ten publishes | cut 2 up at `1f591fa` | at `ec02de6` | at the base `ef47c06` |
|---|---|---|---|
| an hour (`EXPIRY_MAX`) | 1300 s (130 publish steps) | 1920 s | 20.1 s (2 publish steps) |
| ten minutes (the default) | 600.2 s, when they expire | 600.2 s | 20.1 s |

With the default expiry the head holds the cut back exactly as long as
`ec02de6`: at one slice per 20 s, a cut of more than about 30 slices
(7.5 MiB at 256 KiB a slice) cannot go up before the publishes expire.
At the base, one wait shared by all the publishes gave the cut every
step between two failed publish steps, so it went up in 20 s. The
likelihood is F1's: it needs publish steps that fail slowly while the
cut's succeed (my review, F1, *How likely*). It is low; but the module
docs say a publish that cannot be made "holds up neither the uploads
nor another publish" (`uploader.rs` 52-54), and the answer says F1 is
fixed "as the lead decided (publishes never hold up uploads)". For a
cut of more than a few dozen slices that is not so yet.

This one is mine. My review suggested taking turns and said that the
cut's progress would then be "bounded by T for each of its steps", and
I did not weigh what that means for a cut of many steps.

*A fix.* Take turns in time, not in steps: while both are due, the one
that has had less time since both became due goes next, and the count
starts again when either has nothing to do. Then in w6 the cut takes
20 s worth of quick steps after each failed publish step, which is the
whole cut; in w4 (F5) the publishes take a slow cut step's worth of
quick steps after it, which is all of them; and where both sides are
quick (w1) the turns alternate as now. The single-flag rule of the head
is the special case where every step takes the same time. w6 is its
test (cut 2 up within a few publish steps), and w4 the other side's.

**F5. Slow cut steps now pace the publishes (low; w3, w4).** The mirror
of F4, new in `1f591fa`. A publish's lookup and its transaction are two
steps, and when the cut's step is due, one cut step comes between them;
so does one between two publishes.

- w2: one publish carried by cut 1, asked while cut 2's steps take
  10 s (two PUTs of 5 s): made 10 s after it was asked, the steps
  `PCP`. At `ec02de6` and the base: 2 ms, `PP`. One cut step, as the
  docs say.
- w3: the same with cut steps of 40 s and a publish's expiry of 10 s:
  it ends expired, where `ec02de6` and the base make it at once.
- w4: 64 publishes with the default expiry, asked while cut 2 of 124
  steps of 10 s goes up: 30 are made and 34 expire. At `ec02de6` and
  the base all 64 are made at once, and cut 2 is up at the same time
  (1280 s) in all three.

It needs cut steps of seconds each. A real cut step reads 256 KiB and
PUTs only the segments sealed so far, so it is long only on a very slow
link or store, where the publishes may be slow as well. A step that
fails waits in the cut's backoff, and publishes then step back to
back. So it is unlikely, and it does not block; the fix of F4 fixes it.

### The changed sequence in the lapse test

`a_publish_past_its_deadline_or_withdrawn_is_dropped_and_no_cut_carries_it`
now expects the waits `[0, 0, 0, IDLE]` where it expected `[0, 0, IDLE]`:
the idle cut's check between the two lookups. Its guard of the lapses
is the rest of the test (`late` pending, `withdrawn` withdrawn, then
`late` expired, the figures 1 and 1, nothing pending, cut 2 up, no
`snap/` ref for either, nothing published), and that part is unchanged.
The waits only establish that both publishes were looked up and wait
for a cut when they lapse: the final `IDLE` comes only when no publish
has a step to take, since `next_wait` takes the first publish's due
time. That still holds.

To see that the test still fails where a lapsed publish is carried by
a cut or made, I planted five ways of it and ran the test alone:

| Plant | What it does | The test alone fails at |
|---|---|---|
| L01 | a lapsed publish that waits for a cut is not ended, so the next cut carries it | `withdraw` gives `Pending`, not `Withdrawn` (line 2081) |
| L02 | a lapsed publish that waits for a cut is ended and counted, and also kept, its lapse cleared, to be made on the next cut | the same (2081) |
| L03 | a publish that waits for a cut never expires | `late` is `Pending`, not `Expired` (2088) |
| L04 | the turn does not expire publishes, only a publish's own step does (Q1 of #122 undone) | the same (2088) |
| L05 | a publish that waits for a cut cannot be withdrawn | `Pending`, not `Withdrawn` (2081) |

Each also fails in the whole suite (L01 to L03 fail a unit test of
`publish.rs` as well). Each trips the test before cut 2 is written: a
lapsed publish kept to be carried is still pending, and the test asks
how each stands first. None of those assertions changed.

And the control, w5: the same two publishes, neither withdrawn nor
expiring, are both carried by cut 2 and made (two published), so the
test's last assertions do bite where a lapse is ignored. w5 sees the
same waits, `[0, 0, 0, IDLE]` at the head and `[0, 0, IDLE]` at
`ec02de6`.

The new expectation also pins the turns. Run alone against the
plants, the test fails on the waits with T01 (the turns undone:
`[0, 0, IDLE]`), T04, T05, T06, T07 and N09, and with "the uploader
never sleeps" with D2g, T02 and T03. So the change makes the test
stricter, not looser.

### F2 and F3

- **F2.** `a_failed_steps_wait_goes_to_the_publish_that_took_it` is v5's
  shape (`w` asked first waits for a cut, `y` asked after is refused
  every time; `y`'s sends must grow, `w` is never sent). M05 (the wait
  given to the first pending publish) now fails it. Settled.
- **F3.** "In a row" is gone from both doc comments (`publish.rs` 741,
  `uploader.rs` 46). Settled.

### The new test of F1

**F6. `publishes_that_fail_slowly_hold_the_cut_back_for_one_step_at_most`
checks a looser bound than its name (nit; T06, T07).** It asserts that
cut 2 is up within N × T (200 s, 400 s, 320 s), one round of the
publishes, while the head does it in one publish step (20 s, 20 s,
5 s). Plants T06 and T07 let the cut's step wait for three and for four
publish steps in a row instead of one, and the test passes with each.
The suite still catches both, but only through the exact waits of the
lapse test (an idle uploader), and my v1 and w1 fail with them. With
eight in a row (T04) the test fails. A bound of T and a little (one
publish step, as the name says) would catch them where they belong.
With F4's fix the test would want w6's shape anyway.

### Plants

Each made alone in the scratch worktree, `cargo test -p skyzfs-upload`
run, the file restored. A plant that survives is run against my
scenarios as well.

| Plant | What it does | Result at `1f591fa` |
|---|---|---|
| E09 | the newest publish with work is stepped first | caught: `each_publish_waits_alone_after_its_own_failures` |
| N07 | a failure's wait goes to every pending publish (one wait for all) | caught: 2 tests |
| N09 | the uploader's sleep ignores when a publish may step | caught: 2 tests |
| D2g | a publish that waits for a cut has a time to step | caught: 2 tests |
| M01 | the same request asked again leaves the withdrawal (the author's D1a) | caught: the unit test |
| M02 | the same request asked again clears an expiry too (the author's D1b) | caught: the unit test |
| M03 | a lookup that finds the name absent resets the publish's wait | caught: 5 tests |
| M04 | no publish steps while the cut waits in its backoff (moved to the new line) | caught: `a_publishs_wait_wakes_the_uploader_while_the_cut_waits_longer` |
| M05 | a failed step's wait goes to the first pending publish, not to the one stepped | caught: `a_failed_steps_wait_goes_to_the_publish_that_took_it` (F2) |
| T01 | the turn is never noted: a publish always first (the fix undone; the author's F1a) | caught: the lapse test and the test of F1 |
| T02 | once a publish has stepped, the cut first whenever it is due | caught: 10 tests |
| T03 | the cut first whenever it is due: publishes step only while it waits | caught: 10 tests |
| T04 | the cut's step waits for eight publish steps in a row | caught: the lapse test and the test of F1 |
| T05 | an idle cut takes no turn: publishes back to back while no cut is under way | caught: the lapse test and the test of F1 |
| T06 | the cut's step waits for three publish steps in a row | caught: the lapse test only (F6); v1 and w1 fail |
| T07 | the cut's step waits for four publish steps in a row | caught: the lapse test only (F6); v1 and w1 fail |
| L01 to L05 | a withdrawn or expired publish kept for a cut (the table above) | caught: the lapse test, and L01 to L03 a unit test too |

All 22 caught. T02 and T03 are the starvation the other way: with
either, publishes never step while the uploader is idle, and ten tests
fail. A first version of T04 kept its count in a `static` shared by the
tests that run in parallel, and survived the suite for that reason;
the table shows the count of each test's own. I did not rebuild the
author's other plants; their table says each is caught, and the plants of
mine that overlap them (N07 and D2a, N09 and D2e, D2g, M01 and D1a,
M02 and D1b, M05 and F2a, T01 and F1a or F1b, T03 and F1c) agree.


### Scenarios

At `1f591fa`, and at `ec02de6` and the base `ef47c06` for comparison.

| Scenario | What it sets up | `1f591fa` | `ec02de6` | `ef47c06` |
|---|---|---|---|---|
| r1 to r10, r3b, f1 to f3, q1 to q16, u1 to u7 | #122's and #126's, rerun | pass | pass | as in my review |
| n1, n2 | F1's remainder at #122 | fail, as at #122 | fail | fail |
| v1 | N publishes that fail slowly, then a cut of one slice | **passes** (F1) | fails | passes |
| v2, v3, v4 | no busy loop; an expired publish leaves no wait; D1 through the uploader | pass | pass | as in my review |
| v5 | a younger publish that fails while an older one waits for a cut | its check passes; 4 turns in a row without a wait, my bound 3 (above) | passes | passes |
| w1 | publishes that can be made while a long cut goes up | pass: `PCPC...` | fails: `PPPP...` | fails: `PPPP...` |
| w2 | one publish while the cut's steps are slow | passes: one cut step between | passes: none | passes: none |
| w3, w4 | slow cut steps and the publishes' expiry (shown, not judged) | expired; 30 made, 34 expired | made; 64 made | made; 64 made |
| w5 | the lapse test's control | passes | passes | passes |
| w6 | slow-failing publishes and a cut of 130 steps (shown) | 1300 s; 600 s | 1920 s; 600 s | 20 s; 20 s |

w1 failing at `ec02de6` and the base is the old rule (publishes first),
not a fault there. The scenarios and plants are not committed. They
were run in a scratch worktree of the head (moved to `ec02de6` for one
comparison, and a second at `ef47c06`), which was clean after every
run.

### Gates

At `1f591fa`, rustc 1.100.0 (2026-11-09), the current stable:
`cargo fmt --all --check` clean; `cargo clippy --workspace
--all-targets -- -D warnings` clean plain, with
`--features skyzfsd/test-hooks,skyzfs-local/test-hooks` and with
`--features skyzfs-ublk-sys/control-test-program`; `cargo test -p
skyzfs-upload` 130 passed, `cargo test -p skyzfsd` 106 passed, and with
the test hooks 142 passed, none failed.

### CI and Codex

CI on `1f591fa`: 22 checks, 18 green and 4 skipped, none failed, none
running. Among them are `test` and `unit-arm64` (the workspace's tests
on amd64 and arm64), lint, coverage, e2e, and the three jobs "record
through skyzfsd and check". Codex reviewed `1f591fa` (code review and
security review completed) and has no finding, and nothing is posted on
the diff. `zfs-topology` has moved since by #123 (the bench, the rigs,
`tests/kernel/`, two workflows and the notes), none of it in
`skyzfs-upload`, and the pull request still merges cleanly.

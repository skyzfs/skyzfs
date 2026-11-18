## Review of #126 at ec02de6 (M2.5-1b)

D1 and D2 of my next look at #122, as the lead decided them: a
withdrawal undone when the same request is asked again, and a wait of
each publish's own. Two commits on `ef47c06` (`zfs-topology`):
`145426f`, the change and its tests, and `ec02de6`, a test the
author's plants showed missing. I confirmed the head with the API.
`ef47c06..ec02de6` touches only `skyzfs-upload`: `driver.rs`,
`publish.rs` and its unit tests, `uploader.rs` and `tests/uploader.rs`.
`zfs-topology` has moved since, by one notes commit, and the pull
request merges cleanly.

**Verdict: nothing blocks the merge, but F1 should be fixed here.** D1
is right in all the cases I could set up, and so are D2's waits for one
publish, or for several that fail quickly. The tests are the ones I
meant. But D2 as built lets several publishes that fail slowly hold the
cut back. That is my first look's F3 again, under a narrower condition.
At the base, the same scenario holds the cut back for one publish step
and no more. The fix I suggested in my next look, a wait per publish
with nothing more, is what opened this hole, so the hole is mine as
much as the author's. A few lines close it (below).

- **F1. Several publishes that fail slowly hold the uploads back
  (medium; certain in the simulation, v1).** Not blocking in my
  judgement: it needs failures of the publishes alone that take
  seconds each. It is a regression of F3 against `zfs-topology`, and I
  would fix it in this package, since M2.5-2 builds on it.
- **F2. No test has the wait go to a publish that is not the oldest
  (low; a gap in the tests, v5).** Plant M05 survives the tests.
- **F3. "In a row" in two doc comments (nit).**

CI and Codex: see the end.

### What I did

- Read the description, the diff and the lead's decisions on D1 and
  D2 as the description gives them, against the code.
- Gates at `ec02de6` with rustc 1.100.0 (current stable):
  `cargo fmt --all --check` clean; `cargo clippy --workspace
  --all-targets -- -D warnings` clean plain, with
  `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`. Tests, 0 failed:
  `skyzfs-upload` 128 (five new); `skyzfsd` 106 plain and 142 with
  `test-hooks`. I did not run the whole workspace's tests here, for the
  container's disk; this package changes `skyzfs-upload` alone, and
  `skyzfsd` is the only crate that uses it.
- My 34 scenarios of #122 rerun on the new head (`scen2.rs`,
  `scen2_up.rs`, appended for a run to `tests/publish.rs` and
  `tests/uploader.rs`), and 10 new ones: q12 to q16 (D1's scope, on the
  driver) and v1 to v5 (D2, through the uploader). 41 of the 44 pass
  at the head. n1 and n2 fail as they did at #122 (F1's remainder,
  which this package leaves alone and is not about), and v1 fails (F1
  here). At the base `ef47c06` the same scenarios fail n1 and n2, and
  also q9, q13, u6, u7, v3 and v4, which are D1 and D2; v1 passes
  there.
- Nine plants, one at a time, each against
  `cargo test -p skyzfs-upload --no-fail-fast` with a limit of 300 s,
  and a survivor against the scenarios. They are my E09, N07 and N09
  of the next look rebuilt on the new code, the author's D2g (also at
  `145426f`), and five more (table below).

### D1: a withdrawal undone by asking again

`Publishing::answer` (`publish.rs` 598-615): the same request, pending,
clears a `Withdrawn` lapse and answers `Accepted`; an `Expired` lapse
stays. A lapse exists on a pending publish only while its failed
transaction waits for its lookup (`Recheck`), since every other lapsed
publish ends at once, so that is the only window D1 acts in.

| | Case | Result at `ec02de6` |
|---|---|---|
| q9 | withdrawn while its lookup is due, then the same request | `Accepted`, then published (`Done`): fixed |
| q12 | the same, the transaction having landed unheard | `Done`, nothing written |
| q13 | withdrawn, the deadline passes, then the same request | `Accepted`, then `Expired` at the lookup, no transaction, counted expired |
| q14 | withdrawn, the same request, withdrawn again | `Withdrawn` at the lookup, nothing written |
| q15 | withdrawn, then the name with another manifest | `Refused(Asked)` while the first waits; once that ends `Withdrawn`, the new request is accepted and published |
| q16 | past its deadline, then withdrawn, then the same request | `Expired` (counted expired, not withdrawn), nothing written |
| v4 | through the uploader: three failed manifest PUTs, withdrawn, the same request, the store works again | `Done` 4 s later, after its own wait |
| u2, u5, q8, q10 | fence, the store keeping a cut, a restart, a stop | as at #122: `Refused(Stopped)`; `Unknown` on a new uploader, which then finds `Done` |

The deadline is kept in two ways: the lapse `Expired` is not cleared,
and `expire` sets it again from the deadline before every publish step
(`driver.rs` 419-421). So the second alone would do. That is why the
author's D1b (my M02: clear any lapse) is equivalent through the driver.
The unit test catches it only because it calls `absent` with no
`expire` before it. Nothing to change: the rule is right, and it is
held twice.

### D2: a wait of each publish's own

`Pending::errors` and `retry_at`; `next` (`publish.rs` 719-725) skips a
publish whose wait has not passed; `due_at` (731-737) gives the first
time one may step, none for publishes that wait for a cut;
`wait_after` (748-754) doubles that publish's wait; the driver notes
which publish it stepped (`stepped`, `driver.rs` 423) and
`publish_failed` gives it the wait; the uploader's shared
`publish_at` is gone and `next_wait` wakes at `publish_due_at`
(`uploader.rs` 457-463).

Against what the coordinator asked me to judge:

- **One publish that cannot be made holds up no other.** u6 and u7 now
  pass: `b`, asked a second after `a`, is published at 1.004 s while
  `a` goes on being sent at 1, 3, 7, ... 511 s; with an expiry of two
  minutes, `a` ends expired and `b` is done.
- **The wait is reset with the publish's end, and only then.** v3: `a`
  ends expired, and `c`, asked after and failing too, waits 1 s, then
  2 s. A lookup that succeeds does not reset it (plant M03 is caught
  by four tests).
- **No busy loop, no wait for ever.** v2: one publish refused every
  time for ten minutes, the cut idle: never more than two turns in a
  row without a wait, and 10 sends. v5: an older publish waiting for a
  cut and a younger one refused every time: the younger's waits grow
  (sent at 0, 1, 3, 7, 15 and 31 s), at most two turns in a row without
  a wait. A
  wait is at most five minutes, and a lapsed publish in `Recheck` ends
  at its lookup after its wait.
- **F3 of my first look holds for one publish** (u3: cut 2 up at 4 ms;
  v1 with one publish failing after 20 s: cut 2 up after that one
  step) **and not for several that fail slowly**: F1.

**F1. Several publishes that fail slowly hold the uploads back
(medium; certain in the simulation; v1).** `uploader.rs` 273: a turn
takes a publish's step whenever any publish is due, and the cut's only
when none is. Since each publish now has its own wait, there is always
another one due while N publishes each take T seconds to fail, until
every publish's wait is longer than a whole round of them (N × T).
Waits stop growing at five minutes, so when N × T is over five minutes
the cut never takes a step while the publishes pend, which can be an
hour (`EXPIRY_MAX`). Below that, the cut waits until each publish has
failed about log2(N × T) times. v1 asks N publishes whose transaction fails after
T seconds (a store that times out on them), all carried by cut 1, and
then writes cut 2:

| N publishes, failing after T | cut 2 up at `ec02de6` | at the base `ef47c06` |
|---|---|---|
| 1, 20 s | 20 s | 20 s |
| 5, 20 s | 720 s | 20 s |
| 10, 20 s | not in 30 minutes (90 publishes sent) | 20 s |
| 20, 20 s | not in 30 minutes | 20 s |
| 20, 5 s | 820 s | 5 s |
| 64, 5 s | not in 30 minutes (360 publishes sent) | 5 s |

At the base, one wait shared by all the publishes gave the cut every
turn between two failed publish steps. Each publish now waits only for
its own failures.

*How likely.* It needs publish steps that fail slowly while the cut's
steps succeed. A refusal (`Conflict`) or an error status is fast, and
then N × T stays small: with 64 publishes failing after 100 ms, a
round is 6.4 s, and the waits pass that by the fourth failure. A store
that times out on everything stops the cut's steps as well. So it takes
a failure of the publishes alone that takes seconds, for example a
manifest PUT under `skyzfs/manifest/` that the store throttles or lets
time out. Several such publishes can pend at once: M2.6's agent asks
once a minute, and a failing publish pends until it expires (10
minutes by default). It is unlikely, but it is exactly the case F3 was
about, and the module docs now say that a publish that cannot be made
"holds up neither the uploads nor another publish" (`uploader.rs`
49-50). That is true of one publish and not of several.

*A fix.* When both are due, take turns: after a publish's step, the
next turn takes the cut's step if the cut's own wait has passed (a flag
in `Uploader`). A cut's step then waits for at most one publish step,
so the cut's progress is bounded by T for each of its steps, whatever
N is. An idle cut's step is a check that reads only the local store, so
taking turns costs nothing when there is no cut. v1 is its test.

**F2. No test has the wait go to a publish that is not the oldest (low;
a gap in the tests; v5).** Plant M05 gives a failed step's wait to the
first pending publish instead of to the one stepped
(`driver.rs` 423, 434-437), and every test passes. In each test the
publish that fails is the oldest one that pends. With M05, a younger
publish that fails while an older one waits for a cut never waits: v5
fails, the younger one sent at every turn. v5 catches it, and so would
a test of that shape in `tests/uploader.rs`.

**F3. "In a row" (nit).** `publish.rs` 741 and `uploader.rs` 42 say the
wait doubles "at each failure of it in a row". Nothing resets it short
of the publish's end, not even a lookup that succeeds (by design, and
the unit test says so). So it doubles at each failure of the publish,
in a row or not.

A note, not a finding: when the store fails for everything, each
pending publish now retries on its own clock, so up to 64 publishes and
the cut make failing requests in one wait instead of two. That is
bounded and harmless.

### The tests the author added, against what I meant

- u6 and u7 are `a_publish_that_cannot_be_made_holds_up_no_other` (u6
  with an expiry of an hour instead of the default, which is fine): what
  I meant. Plant N07 (one wait for all) is caught by it.
- N07 is `a_publish_that_ends_leaves_no_wait_to_the_next`. In the new
  design no shared wait is left to reset, and the test checks what that
  becomes: a publish asked after another ended starts from a second.
  What I meant.
- N09 is `a_publishs_wait_wakes_the_uploader_while_the_cut_waits_longer`:
  what I meant. Plants N09 and M04 are caught by it, and by it alone.
- E09 is now held by `each_publish_waits_alone_after_its_own_failures`,
  which checks the order of `next`.
- D2g: it survived at `145426f` (I ran it there: every test passes, and
  my v5 fails with it, since an idle uploader then turns without a
  wait). At `ec02de6` it is caught by two tests, the unit test and
  `a_publish_past_its_deadline_or_withdrawn_is_dropped_and_no_cut_carries_it`,
  as the description says.

### Plants

| Plant | What it does | Result at `ec02de6` |
|---|---|---|
| E09 | the newest publish with work is stepped first | caught: `each_publish_waits_alone_after_its_own_failures` |
| N07 | a failure's wait goes to every pending publish (one wait for all) | caught: 2 tests |
| N09 | the uploader's sleep ignores when a publish may step | caught: `a_publishs_wait_wakes_the_uploader_while_the_cut_waits_longer` |
| D2g | a publish waiting for a cut has a time to step | caught: 2 tests (survives at `145426f`; my v5 fails with it) |
| M01 | the same request asked again leaves the withdrawal (the author's D1a) | caught: the unit test |
| M02 | the same request asked again clears an expiry too (the author's D1b) | caught: the unit test (equivalent through the driver, above) |
| M03 | a lookup that finds the name absent resets the publish's wait | caught: 4 tests |
| M04 | no publish steps while the cut waits in its backoff | caught: `a_publishs_wait_wakes_the_uploader_while_the_cut_waits_longer` |
| M05 | a failed step's wait goes to the oldest pending publish, not to the one stepped | **survives**; my v5 fails with it (F2) |

I did not rebuild the author's other plants. Their table says each is
caught, and the five of mine that overlap them (N07 and D2a, N09 and
D2e, D2g, M01 and D1a, M02 and D1b) agree.

### Scenarios

At `ec02de6`, and at the base `ef47c06` for comparison.

| Scenario | What it sets up | `ec02de6` | `ef47c06` |
|---|---|---|---|
| r1 to r10, r3b, f1 to f3, q1 to q8, q10, q11, u1 to u5 | #122's, rerun | as at #122: pass | pass |
| n1, n2 | F1's remainder at #122 | fail, as at #122 | fail |
| q9 | withdrawn while its lookup is due, then the same request | passes | fails |
| q12 to q16 | D1's scope (table above) | pass | q13 fails |
| u6, u7 | one publish that cannot be made beside another | pass | fail |
| v1 | N publishes that fail slowly, then cut 2 | **fails** (F1) | passes |
| v2 | a failing publish, the cut idle: no busy loop | passes | passes |
| v3 | a publish that expired leaves no wait | passes | fails |
| v4 | D1 through the uploader | passes | fails |
| v5 | a younger publish that fails while an older one waits for a cut | passes | passes |

The scenarios and plants are not committed. They were run in a
scratch worktree of the head (moved to `145426f` and `ef47c06` for the
two comparisons), which was clean after every run.

### CI and Codex

CI on `ec02de6`: 22 checks, 18 green and 4 skipped, none failed. Among
them are `test` and `unit-arm64` (the workspace's tests on amd64 and
arm64), lint, coverage, e2e, and the three jobs "record through
skyzfsd and check" (on 26.04, on 26.04 with the upload, and on 26.04
arm64). Codex reviewed `ec02de6` and has no finding, and nothing is
posted on the diff.

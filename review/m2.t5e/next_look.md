# #145 (M2.T5e): next look at aabde27, 9bac3dd and b3bb916

## Which head each part was judged at

| Part | Judged at | Why there |
|---|---|---|
| The cut check's busy-export retry (`cut_check.py`, `test_cut_check.py`) | **aabde27** | Neither file changed after aabde27 (`git diff aabde27 b3bb916` leaves both alone). |
| F1, F2, G1 to G3 (the marks, `a1_premise`, `pace_faults`, the statuses, the `pidle` tests) | **9bac3dd** | The head when this look began. |
| The stray-group change (the coordinator's question) | **b3bb916** | The head moved there during this look: one commit that changes one line of `a1_premise`, its docstring and its tests. |
| #144's schedule | #144 at **1b5e5f9** | #144's current head. `assist_due` and the three constants are unchanged since 5faaabf. |

Line numbers are at the head named in each place. The plants, the replays
and the gates were run on clean worktrees of these commits. Their outputs
are in this directory.

## Verdict

**At 9bac3dd: cannot merge.** Two findings block:

- **B1.** The new premise cannot hold on a real pool, so passist ends 2
  on every kernel run.
- **B2.** A new test in `test_cut_record.sh` is flaky, and `ci.yml` runs
  it on every pull request.

**At b3bb916: still cannot merge, on B2 alone.**

- b3bb916 fixes B1, and opens no hole: a daemon that publishes below G
  is still caught, by the row's pin (see Part 3).
- `test_cut_record.sh` is unchanged at b3bb916, so B2 still stands.
- The publish rig's kernel run at b3bb916 (run 45622568792) passed.
  passist ended 0, with the same groups as at 9bac3dd: 16 at the
  acceptance and 23 at the end. The premise reads "assist 2 took it
  across G, and every group from G on that wrote was an assist's". The
  worst exit of the nine courses is 0.

**aabde27 (the retry) can merge as it is.** Nothing found against it
blocks. One suggestion (S3) and one test gap (T1) follow.

## Findings, most severe first

### B1. Blocks at 9bac3dd; fixed at b3bb916. Defect, certain.

**Where:** `tests/kernel/publish_check.py:750-751` at 9bac3dd.

```python
theirs = {x for r in rows for x in range(r['before'] + 1, r['after'] + 1)}
stray = sorted(written_groups(txgs, w_acc, top) - theirs)
```

The premise counts every group that wrote from the acceptance mark's W
to the end, and fails on any group that is not in an assist's range.

**Why it cannot hold.** The rig itself writes a group in that window:

- The agent's flow (`publish_agent.py:373-375`) makes the snapshot
  (createtxg 15) and then `zfs hold` (a sync task, group 16, committed
  in 7 ms).
- The hold's history record is written lazily, in the next open group,
  17. That group is born at 417.37 s and committed by the group timeout
  (otime 5.5 s).
- That commit falls after the acceptance mark (417.76 s, W 16) and
  before the first assist (427.79 s, `before` 17).

The same happens on every recording of passist: at cb91ad1, aabde27,
9bac3dd and b3bb916, a1's first assist starts from `before` 17. a2 shows it too: it
was accepted at W 31, and its first assist read 32.

**The failing scenario is every passist run on the kernel.** The
publish rig at 9bac3dd (run 45616155197, ubuntu-26.04) ended `worst
exit 2`:

> passist: premise_a1_needed_the_assist: ERROR: [rig] a1 asked with G
> 20: the newest group that wrote was 16 when it was accepted and 23 by
> its end (done), over 2 assists: groups no assist wrote: 17

The other eight courses passed.

**What it cost.** The course could give no verdict at all. The replay of
that recording at 9bac3dd (`next_replay.out`) gets 25 of 46 cases right:

- D1 and D11, the faults the course exists for (a daemon that never
  counts the assists' cuts), end 2, not 1.
- In all, 16 of the 21 D cases end 2. Only D2, D3, D4, D8a and D14 end
  as they should: there a1's end mark reads below G, or a1 was never
  accepted.

The fixtures did not see it: `test_publish_check_main.py:243` and
`test_publish_check.py:559-565` have no group between the acceptance and
the first assist. The author's answer of 12:42 UTC (that the D cases end
1) rested on those fixtures. The kernel run ended 2 at 12:51 UTC.

**What fixes it.** Either of two forms passes the same 41 of 46 cases;
the other five are S1's and S2's pace cases.

- Count strays only at or above G. b3bb916 does this.
- Count strays from the first assist's `before` on. This is plant N23b:
  every suite passed with it at 9bac3dd.

**At b3bb916:** fixed, with strays counted only at or above G
(`publish_check.py:755`). It
also settles the fixtures' gap: new tests put a group below G before
the first assist, in the unit test and through `main`.

### B2. Blocks. Defect (a flaky test), certain.

**Where:** `tests/kernel/test_cut_record.sh:543-566`, the same at
9bac3dd and b3bb916.

**What goes wrong.** `pidle_run`'s stand-in `sleep` moves bash's
`SECONDS` on by assignment (`SECONDS=$((SECONDS + $1))`, line 552). But
`SECONDS` reads as the value assigned **plus the whole seconds of wall
clock since the assignment**. So a second boundary that falls inside a
run adds one.

**How often** (bash 5.2.21):

- **Run alone:** the idle run takes about 20 ms and the grow run about
  45 ms (`pidle_time_b3bb.out`). So about 2% and 4.5% of runs cross a
  boundary, roughly one `test_cut_record.sh` run in fifteen.
- **Counted at b3bb916, load about 1.5,** 200 runs of each
  (`pidle_flake_b3bb.out`):
  - 6 idle runs answered another way: "idle after 12 s, 11 sleeps" (3)
    and "idle after 13 s, 12 sleeps" (3).
  - 7 grow runs answered "error after 30 s, 29 sleeps".
- **Counted at 9bac3dd, load about 4.5** (`pidle_flake_9bac_load.out`):
  6 idle runs and 16 grow runs of 200 answered another way.
- **Seen in the gates:**
  - The gate run at 9bac3dd failed once this way: "got 'error after 30
    s, 29 sleeps', want 'error after 30 s, 30 sleeps'" (`gates_py_9bac.log`).
  - In the earlier look, 2 of 8 full runs under load failed.
  - In #141's plants, re-run at 9bac3dd, it killed a no-op plant (N2)
    and one more (N21). Re-run alone, both survive.

**Who runs it:**

- `ci.yml:62`, on every pull request, whatever its base;
- `skyzfs-ci.yml:89`;
- `skyzfs-cut-daemon.yml:115`;
- `skyzfs-publish-rig.yml:94` (the unit job).

A flake there turns other work packages' CI red at random.

**A sketch, run on a copy** (`pidle_unset.sh`): `unset SECONDS` in
`pidle_run`'s subshell before `SECONDS=0`. In bash, an unset `SECONDS`
loses its special property, so only the stand-in sleep moves it.

- Result: 300 of 300 runs of each gave the wanted answer
  (`pidle_unset_b3bb.out`).
- It still catches plant S2 ("idle after 1 s, 1 sleeps").
- One cost: a `pidle` that never sleeps (plant S3) would then loop for
  ever, where today it ends after 12 s of wall clock. The subshell needs
  a bound of its own. Another way is to give `pidle` a clock the test
  replaces.

The choice is the author's. "Not weakened" means the exact answers
stay.

### S1. Suggestion, and a test gap. The pace premise has floors only.

**Where:** `publish_check.py:759-773` at 9bac3dd (`:763-777` at b3bb916).

**What passes that should not.** Each of these ends 0 at b3bb916, and
the sketch with ceilings ends it 2 (`next_replay_b3bb.out`):

- V5: a2 assisted in its burst and never again for 58 s.
- V6: spaced at 20 s.
- V7: the first assist 30 s after the acceptance.
- V8: a run with A of 5 s.

A is read from the mark and never compared with `ASSIST_AFTER`. Plant
N29 shows the tests rely on that: their A is 5 ns.

**Why it does not block:**

- The agent's own tests pin its schedule (plants A15 to A19 are KILLED).
- `test_cut_record.sh:533` pins the rig's call, which passes no
  `--assist-after-s`.
- No verdict rests on the pace: `obs_assist_pace` is INFO.

**The sketch** (`pace_faults_ceiling` in `next_replay.py`) gets 46 of 46:

- A equal to `ASSIST_AFTER`;
- the first assist within A + 3 s;
- spaced gaps within 1.5 A;
- for a publish that ended `pending`, its last assist within 1.5 A of
  the end.

**For the lead (L4):** did "the course judges its own schedule" mean
floors only?

### S2. Suggestion, low. The burst's bound is measured from start to start.

**Where:** `publish_check.py:768-769` at 9bac3dd (`:772-773` at b3bb916).

**How #144 paces the burst.** At 1b5e5f9 the engine records
`assisted_ms` after the sync returns (`engine.rs:701-703`). It pauses
`ASSIST_PAUSE` from there. The stand-in does the same: after t1 it waits
in `settled` (0.1 to 1 s), then `sleep(1)`.

**The failing scenario.** A start-to-start gap reaches A/2 = 5 s when a
set and its sync take about 3 to 4 s. Then a right pace fails, and the
course ends 2. Replay F2 (4.1 s each, gaps of 5.2 s) does this at
b3bb916.

**How likely.** The recorded set and sync took 15 to 36 ms, with burst
gaps of 1.12 s. So the risk on hosted runners is low.

**Sketch:** take the burst's pause from the previous t1
(`t0[k] - t1[k-1]`).

**Tolerance otherwise:**

- The first-assist floor holds by construction: the agent takes its
  `t0` after it writes the mark.
- The 0.95 A floor on spaced gaps allows 0.5 s of jitter between the
  `waited` read and the set. That jitter is a kstat read.
- I found no other way for a slow runner to fail a right pace. F1
  (gaps of 3.5 s) and F3 (11.8 s) end 0 at b3bb916.

### S3. Suggestion (aabde27). Count the export refusals in the summary.

**Where:** `cut_check.py:257`, `:595-598`.

A busy export that clears within the waits leaves its record only in the
job's log (`print`). It reaches the summary only through the error, when
it never clears. One INFO line with the count would show whether the
retry starts to fire. It has not fired yet: 0 refusals in all eight
kernel jobs at aabde27 and 9bac3dd.

### S4. Nit. The premise list in the module docstring is stale.

**Where:** `publish_check.py:176-179` at 9bac3dd, and `:179-183` at
b3bb916.

It still says "at a1's first assist the pool's newest group that wrote
was below the G asked". The premise is now:

- taken from the acceptance;
- with the stray rule;
- with the branch for a1 refused before acceptance;
- with a2's premise only for `pending after`.

The two pace premises are not in that list. b3bb916 updated the paragraph
at `:112-123`, but not this one.

### Test gaps (none blocks)

- **T1 (aabde27).** Nothing tests `kimport`'s use of the retry. Plants
  E11 (a failed export no longer the rig's error), E12 (the retry
  bypassed) and E13 (a busy export after every wait passed as exported)
  all SURVIVE.
- **T2 (9bac3dd, b3bb916).** Three of `a1_premise`'s boundaries are
  unpinned:
  - N13 (top from the end mark alone). On a kernel this is equivalent:
    the end mark is read after the assists.
  - N16 (`top < g` read as `<=`). No test has top equal to G.
  - N19 (a crossing with `<=` at `before`).
- **N40 is equivalent** (a stray at G itself not counted, at b3bb916).
  An assist that crosses G has G in its range. With no crossing, the
  premise ends before the stray check. The author says the same of
  F1l.
- **Still SURVIVED from the earlier look:** A1 and A4 (equivalent), S10
  and S13 (weighed at cb91ad1).

## Part 1: aabde27, the busy-export retry (judged at aabde27)

**The code:** `export_after_import`, `cut_check.py:239-261`, and its
caller in `Check.kimport`, `:594-598`.

**Does it tell a busy pool from a real failure?** Yes.

- It tries again only when the export failed and zpool's output (stderr
  merged into stdout by `run`) says `busy`. That covers "cannot export
  'x': pool is busy".
- Any other failure returns at once, with no wait
  (`test_another_failure_is_not_tried_again`; plants E4 and E5 KILLED).
- A failure that says busy for another reason (an unmount, a device) is
  also tried again. That does no harm: after the waits it is still the
  rig's error.

**Is it bounded?** Yes.

- At most five exports, with waits of 1, 2, 4 and 8 s
  (`replay_check.EXPORT_WAITS`, 15 s in all).
- Each export runs under `run`'s 120 s limit, and each `ps` under 30 s.
  The worst case is about 12 minutes, against 2 before. A busy refusal
  is immediate in practice.
- What CLAUDE.md says of an export on a suspended pool (it cannot be
  killed) held before aabde27 as well. The retry adds no new way to
  hang.

**Can it mask a pool the daemon left busy?** No.

- `kimport` exports only after the check's own successful import of a
  recorded image (`:594`). The node B check uses the same `Check`
  (`nodeb_check.py:395`).
- No daemon runs in either check's loop.
- A failed export was the rig's error (exit 2) before aabde27, and it
  still is. It was never a verdict on the server.
- So the retry can turn an exit 2 into a run that goes on and judges. It
  cannot turn an exit 1 into 0.
- What it can hide is a step of the check that leaves the pool busy for
  under 15 s. Those refusals are printed with the ZFS commands then
  running, but only to the log (S3).

**Does the test fail if the retry is removed or made unbounded?** Yes.

- Removed: E1 (one export, as before) is KILLED.
- Unbounded: E2 (a wait of 1 s for ever) is KILLED.
- E3, E6, E7, E8, E9 and E10 are KILLED too: one more wait, no wait, a
  fixed wait, no refusal recorded, no try after the last wait, and busy
  read as exported.
- All by `test_cut_check.py`.
- The gap is only the caller (T1).

**In CI:**

- The cut rig passed on all four images at aabde27 (run 45607098885) and
  at 9bac3dd (run 45616155245).
- None of the eight kernel jobs has a line "cut_check: zpool export".
  The retry never fired.
- The busy export it answers was seen once, on ubuntu-24.04 at cb91ad1
  (run 45600485861).

**#143's W plants** cover `cut_check.main`'s wiring. All 69 of #143's
plants were re-run at 9bac3dd (`p143_at_9bac.out`). Line for line, they
match their earlier results:

- W5 to W13, H1 to H4 and B1 to B7 KILLED;
- W14 to W21 and H5 SURVIVED, as at d3b75af.

## Part 2: 9bac3dd

### The replay of the kernel recording

**The script:** `next_replay.py WORKTREE ARTIFACT_DIR`. It replays the
publish rig's own passist recording at 9bac3dd (run 45616155197, the
first with `accepted.tsv`) through `publish_check.main`, with node B
stood in for.

**How each case is built:**

- A daemon fault changes only what such a daemon would change: its
  answers, its log lines, its row. It also changes what the agent would
  then have done, such as fewer assists, other marks, or fewer requests.
- A rig or kernel fault changes only what that fault would leave.

**The columns:**

- the head as it is;
- B1's sketch (form `first`, or `ge_g` with `B1_SKETCH=ge_g`);
- and, for the pace cases, the ceilings of S1 and S2.

| Run | Head | With B1's sketch | With B1's sketch and the ceilings |
|---|---|---|---|
| `next_replay.out` | 9bac3dd: 25 of 46 | 41 of 46 | 46 of 46 |
| `next_replay_ge_g.out` | 9bac3dd: 25 of 46 | 41 of 46 | 46 of 46 |
| `next_replay_b3bb.out` | **b3bb916: 41 of 46** | 41 of 46 | 46 of 46 |
| `next_replay_b3bb_ge_g.out` | b3bb916: 41 of 46 | 41 of 46 (the same as the head) | 46 of 46 |

**What each kind of case should end:**

- Daemon faults (D) should end 1.
- Faults of the rig or the kernel (R) should end 2.
- A wrong pace or a stray group (V) should end 2.
- A right but slow run (S1 and F) should end 0.
- I1 is not judged.

| Case | Should end | 9bac3dd | b3bb916 |
|---|---|---|---|
| P0, the run as recorded | 0 | 2 (B1) | 0 |
| D1 to D19 (daemon faults; D13 and D13b are F2's statuses and should end 0) | 1, or 0 for D13 and D13b | 2 in 16 of 21 (B1) | all right |
| R1 to R11 (the rig or the kernel) | 2 | all right | all right |
| V1, V1b, V4 (a1 carried by a group no assist wrote, at or above G: G1) | 2 | right | right |
| V2, V3 (an assist storm; assisted too early) | 2 | right | right |
| V5 to V8 (too slow, too late, the wrong A) | 2 | 2, but only through B1 | **0** (S1) |
| S1, F1, F3 (right, slow) | 0 | 2 (B1) | 0 |
| F2 (right, with a set and sync of 4.1 s) | 0 | 2 | **2** (S2) |
| I1 (a group below G between a1's assists, written by something else) | not judged | 2 | 0, where form `first` gives 2 |

**The faults the coordinator asked for:**

- **G1, a1 carried by a group no assist wrote.** V1 (assist 2 read 23
  before its set, and 22 carried a1), V1b (a group at or above G after
  the last assist) and V4 (group 20 before any assist): exit 2 at every
  head and in every column.
- **F2, a status that lacks `uploaded` or `upload_blocks`.** D13 (no
  `upload_blocks` after) and D13b (no `uploaded`, and the status before
  missing) give an INFO line naming what is lacking, with no verdict.
  They end 0 with B1 fixed; 2 at 9bac3dd only because of B1.
- **A daemon that answers `pending` for different lengths:**
  - D1 (a1 `pending after 300 s`) and D11 (no cut counted for the
    assists' groups) end 1.
  - D16 (a2 pending 20 s, then expired) ends 1.
  - V5 (a2 pending 58 s past its burst, with no assist) ends 0 (S1).
  - R1 and R3 (a1 pending, with assists that never reached G) end 2.
  - All with B1 fixed.
- **The schedule's tolerance:** see S2.

### The tables, and #144's schedule

**The tables are pinned both ways.** The earlier look's plants on
EXPECTED, PUBLISHED, ABSENT and COURSES are all KILLED again, at 9bac3dd
and at b3bb916 (K19 to K23).

**#144's schedule at 1b5e5f9** (`plan.rs` `assist_due` and constants),
against the stand-in (`publish_agent.py:89-90`, `:423-427`):

- **Burst and spacing.** #144 has `ASSIST_QUICK = 3` syncs
  `ASSIST_PAUSE` (1 s) apart, from `ASSIST_AFTER` (10 s) after
  acceptance, then one per `ASSIST_AFTER`. The stand-in's
  `ASSIST_BURST = 3` and `ASSIST_AFTER = 10` match.
  - The recording: first assists 10.02 s after acceptance; burst gaps
    1.12 s; then 10.14 s (`next_replay.out`, P0).
- **Two groups per assist.** The set's value is new at each assist: the
  engine's clock, or one past the last (`engine.rs:696`), and the
  agent's `max(wall ms, last + 1)` (`publish_agent.py:352`).
  - On the kernel every assist wrote two groups: a2's eight did, 16 in
    all.
  - `pin_assist_writes_a_group` asks for at least one.
  - Plant A9 (the same value at every assist) is KILLED.
- **What the stand-in does not model.** #144's branch for a carried
  publish (no sync before 2 × `ASSIST_AFTER` once a cut qualifies). This
  only changes what happens after a1 is carried. It touches no verdict.

### Plants at 9bac3dd

Each plant is applied alone; its suites run under 600 s each; the file
is restored in a `finally`; and the tree is clean after each. Every JSON
records `clean_after`.

**This look's** (`next_plants.py`; `next_plants.out`): 49 plants, 43
KILLED and 6 SURVIVED.

- E1 to E13: the retry (aabde27). E11 to E13 SURVIVE (T1).
- N1 to N8: the agent's marks. All KILLED.
- N10 to N37: `read_marks`, `a1_premise`, `pace_faults` and the passist
  block.
  - N13, N16 and N19 SURVIVE (T2).
  - N23 (strays at or above G, the form b3bb916 took) is KILLED at
    9bac3dd by the tests b3bb916 then changed.
  - N23b (strays from the first assist on; `next_plants_n23b.json`)
    SURVIVED. Every suite passes with it.

**The earlier look's** (`plants145.py`; `p145_at_9bac.out`): 64 plants,
56 KILLED, 4 SURVIVED and 4 NOT APPLIED.

- Against cb91ad1, A19, S2 and S3 are now KILLED (G3).
- K1 to K3 and K18 no longer apply: F1 and F2 rewrote their text, and
  N12 to N23 and N37 replace them.
- A1, A4, S10 and S13 SURVIVE, as before.

**Earlier pull requests' plants, re-run at 9bac3dd:**

- **#143's** (`review/m2.t5d/plants143.py`, all 69; `p143_at_9bac.out`):
  57 KILLED, 10 SURVIVED and 2 NOT APPLIED.
  - Line for line, as at d3b75af and cb91ad1.
  - V1 and S14 do not apply, as at cb91ad1. Re-aimed, they are KILLED
    as V1x and S14x.
- **#139's** (`review/m2.t7/next_plants_py.py`, 82; `p139_at_9bac.out`):
  65 KILLED, 8 SURVIVED and 9 NOT APPLIED. Line for line, as at cb91ad1.
- **#141's next look** (`review/m2.t7b/plants_next.py`;
  `p141_at_9bac.out`) and #143's re-aim (`reaim143_at_9bac.out`):
  91 plants: 73 KILLED, 11 SURVIVED and 7 NOT APPLIED.
  - Line for line as at cb91ad1, except N2 and N21, KILLED where they had
    SURVIVED. Both were killed by `test_cut_record.sh` alone.
  - **N2 is a no-op plant ("the harness itself"), so its kill is B2's
    flake caught in the act.**
  - Re-run alone twice, both SURVIVE (`p141_n2_n21_again.out`). So the
    true count is 71, 13 and 7, as at cb91ad1.
  - The re-aim: four KILLED and three NOT APPLIED, as at cb91ad1.

Several of these suites include `test_cut_record.sh`, so B2 can turn a
SURVIVED into a false KILLED. It did twice, in #141's N2 and N21 (above).
No other line moved against the earlier results.

## Part 3: b3bb916 (the coordinator's question)

**What changed.** b3bb916 counts as a stray only a group at or above G
(`publish_check.py:755`):

```python
stray = sorted(x for x in written_groups(txgs, w_acc, top) - theirs if x >= g)
```

It also updates the docstring at `:112-123` and replaces the tests of a
stray below G with tests of a stray at or above G, between assists and
after the last one. A new test says a group below G passes.

### Does it still catch a1 carried by a stray group (G1)?

Yes: V1, V1b and V4 end 2 (`next_replay_b3bb.out`).

- **Below G, a group cannot carry a1.** The rig asks with G =
  createtxg + 5, and the daemon is held to it: the row's cut must be of
  group G or later (`judge_row`, `:484`).
- **At or above G, every way to be carried is still seen:**
  - A group written between assists or after the last one is a stray.
    Plant N44 (only a stray at the top counted) is KILLED by the new
    unit case, with a stray at 56 between assists for G 55.
  - A group at G before any assist leaves no assist to cross G: "no
    assist took the pool across G".
  - G at the acceptance fails "G was written before any assist".

### Does it still catch D2, D3 and D6 to D8?

Yes. All end 1 at b3bb916:

- D2: a1 below G after assist 1.
- D3: a1 on the hold's history group 17, before any assist.
- D6: a2 after its first assist.
- D7: a2 expired.
- D8: a2 refused.

Every other D case ends as it should too: 1, and 0 for F2's D13 and
D13b.

### Does it open a hole, a daemon fault that publishes below G and that the stray check no longer sees?

No. The stray check never judged the daemon.

- **What it is.** A premise on the rig's own records: `accepted.tsv`,
  `assist.tsv` and the txgs kstat. It shows that nothing but an assist
  could have carried a1. It never reads which cut the daemon used.
- **Who catches a publish below G.** The pins on the row do, at both
  heads:
  - `pin_a1_row` (`judge_row`, `publish_check.py:484`: "the row's cut is
    of group N, below G");
  - `pin_a1_cut`, since the row's cut must be a cut line of the log.
- **The two cases that probe exactly this** both end 1 at b3bb916.
  Each has the premise `ok` and `pin_a1_row` FAILED:
  - D18: a1 published on cut 11 of group 19, below G, answered after the
    assists took the pool to 23.
  - D19: the same on cut 9 of group 17, the hold's history.
- **They are a test gap of 9bac3dd only:** there they ended 2 through B1.
- **What b3bb916 gives up against the form `first`.** Only a pool that
  writes below G between a1's assists (I1): b3bb916 ends 0, form `first`
  ends 2.
  - It is a weaker sign that the pool is idle. It is not a verdict:
    below G nothing can carry a1, and `pidle` before a1 still asks for
    an idle pool.
  - I see no reason to prefer either form. b3bb916's is the smaller
    premise that the verdict needs, and it matches the rule the daemon
    is held to.

### Plants at b3bb916

**This look's on `publish_check.py`** (`next_plants_b3bb.out`): N10 to
N44.

- N40 to N44 aim at b3bb916's line:
  - N41 (b3bb916 undone), N42 (only below G), N43 (the form `first`) and
    N44 (only at the top) are KILLED.
  - N40 (a stray at G itself not counted) SURVIVES, and is equivalent
    (T2).
- N13, N16 and N19 SURVIVE, as at 9bac3dd (T2).
- N23 and N23b do not apply: their text is 9bac3dd's.
- All the others are KILLED, as at 9bac3dd.

**The earlier look's K plants** (`p145_at_b3bb.out`), with V1x and L4y:
the same as at 9bac3dd.

The author's 40 of 41, with F1l equivalent, agrees with N40.

## Local gates

**At 9bac3dd** (`gates_cargo_9bac.log`, `gates_py_9bac.log`):

- `rustup update stable` (rustc 1.100.0), `cargo fmt --check`.
- `cargo clippy --all-targets -D warnings`, three ways: plain,
  `skyzfsd --features test-hooks`, and
  `skyzfs-ublk-sys --features control-test-program`. All clean.
- `cargo test`, the same three ways: 1775, 184 and 37 passed; 0 failed.
  - These ran without debuginfo and without the incremental cache,
    because the disk is small.
- Every `tests/kernel/test_*.py`, `tests/bench/test_*.py` and
  `tests/kernel/test_*.sh` passed, except two:
  - `test_cut_record.sh` failed once on B2.
  - `test_control_refused.sh`'s 32 failures came from a full disk during
    that run. Re-run alone, it passes (`control_refused.out`).
- shellcheck 0.11.0 over `git ls-files '*.sh'` is clean.

**At b3bb916:**

- It changes no Rust, Cargo or toolchain file, so the cargo gates of
  9bac3dd stand.
- Every `tests/kernel/test_*.py`, `tests/bench/test_*.py` and
  `tests/kernel/test_*.sh` passed (`gates_py_b3bb.log`). That includes
  `test_cut_record.sh`, which passed this time (B2 is a rate, not a
  certainty).
- shellcheck 0.11.0 is clean.

## CI (read only)

### At 9bac3dd

| Workflow | Run | State | What it means |
|---|---|---|---|
| ci | 45616155435 | success | Includes `test_cut_record.sh`, which passed this time (B2). |
| coverage | 45616155272 | success | |
| e2e | 45616155467 | success | |
| skyzfs-nodeb-rig | 45616155154 | success | |
| skyzfs-cut-daemon | 45616155389 | success | |
| skyzfs-cut-rig | 45616155245 | success | All four images, plus the reference. 0 export refusals. |
| **skyzfs-publish-rig** | **45616155197** | **failure** | The unit job passed. On ubuntu-26.04, passist ended 2 on B1, and the other eight courses passed. |
| skyzfs-kernel-probe | 45616155200 | cancelled | |
| skyzfs-daemon-rig | 45616155385 | cancelled | |
| skyzfs-replay-rig | 45616155182 | cancelled | |

### At b3bb916

| Workflow | Run | State |
|---|---|---|
| ci | 45622568309 | success |
| coverage | 45622568375 | success |
| e2e | 45622568591 | success |
| skyzfs-nodeb-rig | 45622568275 | success |
| skyzfs-cut-daemon | 45622568479 | success |
| **skyzfs-publish-rig** | **45622568792** | **success**: the unit job, and all nine courses exit 0 on ubuntu-26.04. passist's premise is `ok` on the kernel. |
| skyzfs-cut-rig | 45622568337 | cancelled |
| skyzfs-kernel-probe | 45622568631 | cancelled |
| skyzfs-daemon-rig | 45622568529 | cancelled |
| skyzfs-replay-rig | 45622568302 | cancelled |

The push runs of b3bb916 were skyzfs-ci (success) and the publish rig
(cancelled, superseded by the pull request's run).

### Were the cancellations right?

**Yes, at both heads.**

- **The probe** reads `ublk-probe.sh` and `ublk-flags.py`. Neither
  changed after aabde27, where the probe passed (45607099059).
- **The daemon rig's** files (`daemon-rig.sh`, `daemon_check.py`,
  `skyzfsd-ref.sh` and its tests) are not touched by this pull request.
  It passed at aabde27 (45607099099).
- **The replay rig** reads no file of this pull request. `cut_check.py`
  imports `replay_check`, not the other way round.
- **The cut rig at b3bb916:** `cut_check.py` and `cut-record.sh` are
  unchanged since aabde27. It passed at aabde27 and at 9bac3dd.
- **What 9bac3dd and b3bb916 touch:** only `publish_agent.py`,
  `publish_check.py`, `test_cut_record.sh` and three `test_publish_*.py`.
  The publish rig and ci run those, and they were not cancelled.

## For the lead

1. **B2: fix here, or merge with the flake?**
   - In my view it blocks. It is this pull request's own test, and
     `ci.yml` runs it on every pull request.
   - The fix is small: `unset SECONDS`, with a bound, or a clock that
     `pidle` reads and the test replaces.
2. **S1: what did L4 mean?** Did "the course judges its own schedule"
   mean floors only (as built), or the decided pace both ways?
3. **B1's two forms are both sound.** b3bb916 chose `>= G`. Form `first`
   would also have called a pool that writes below G between the
   assists not idle (I1). Neither changes a verdict.

## Files

**Scripts:**

- `next_replay.py`: the replay. Usage: `python3 -I next_replay.py
  WORKTREE ARTIFACT_DIR`, with `B1_SKETCH=ge_g` for the other form.
- `next_plants.py`: this look's plants. Usage: `python3 -I
  next_plants.py WORKTREE OUT.json [ID...]`.
- `pidle_flake.sh`, `pidle_time.sh` and `pidle_unset.sh`: B2's count,
  B2's timing and the sketch of B2's fix.

**Outputs:**

- `next_replay.out`, `next_replay_ge_g.out` (9bac3dd);
  `next_replay_b3bb.out`, `next_replay_b3bb_ge_g.out` (b3bb916).
- `next_plants.out` and `.json`, `next_plants_n23b.json` (9bac3dd);
  `next_plants_b3bb.out` and `.json` (b3bb916).
- `p145_at_9bac.out` and `.json`, `p145_at_b3bb.out` and `.json`:
  `plants145.py`.
- `p143_at_9bac.*`, `p139_at_9bac.*`, `p141_at_9bac.out`,
  `p141_n2_n21_again.out`, `reaim143_at_9bac.out`: earlier pull
  requests' plants.
- `pidle_flake_9bac_load.out`, `pidle_flake_b3bb.out`,
  `pidle_time_b3bb.out`, `pidle_unset_b3bb.out`: B2.
- `gates_cargo_9bac.log`, `gates_py_9bac.log`, `gates_py_b3bb.log`,
  `control_refused.out`: the gates.

**Kernel recordings:** the passist recordings come from the artifacts
of runs 45616155197 (9bac3dd), 45607098954 (aabde27) and 45600486106
(cb91ad1). They are not copied here.

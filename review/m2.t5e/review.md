# Review of #145 (M2.T5e): the assist on a real pool, at cb91ad1

Head reviewed: **cb91ad1** (`gh api repos/skyzfs/skyzfs/pulls/145`), into
`zfs-topology` at 7c9fa5a (the merge base is the base's tip). Two
commits:

- a32dc37: the kernel probe's assist lines;
- cb91ad1: the publish rig's course `passist`, and G3 and G4 of #143's
  review.

Only rig and test files change: no product code. The one workflow
change adds `passist` to the publish rig's default list of courses.

I formed my view from `git diff origin/zfs-topology...cb91ad1` before I
read the description. Then I checked it four ways:

- against the CI recordings of this head: the publish rig's passist
  (run 45600486106, ubuntu-26.04) and the kernel probe's (45600485899,
  24.04 and 26.04);
- by replaying the passist recording, changed one way at a time, through
  `publish_check.main` (`replay145.py`);
- against #144's engine as it now stands (head 28d1d64);
- with plants in the working tree.

File:line references are at cb91ad1 unless they name #144.

## Verdict

**Can merge once the cut rig is green on the head. No finding blocks,
but F1 should be fixed here, and whether it blocks is the lead's call
(L1).**

**The head has moved since.** At 11:43 UTC the author pushed aabde27
on top of cb91ad1. It changes only `cut_check.py` and its test: an
export refused as busy after a check's import is tried again, as the
replay rig does. That answers the cut rig's red job below (L3). I have
not reviewed aabde27; it needs its own look and its own CI. Nothing in
it touches what this note judges: the publish rig, the agent, the
probe.

- **Counting only groups with `nwritten` above 0 is right** in every
  place it is used: the agent, `pidle`, the probe and the judge. The
  caveats are below; none bites on the rigs' idle pools. In the passist
  recording, the groups that wrote are exactly the groups the daemon
  cut, from 5 to 71.
- **F1 (defect, medium, certain).** `premise_a1_needed_the_assist` and
  `premise_a2_assisted` do not rest on the kstat alone. Which assists
  exist depends on how long the daemon answered `pending`. Six daemon
  faults therefore end 2 (the rig's error) instead of 1. The course's own
  targets end 1 as they should, and the masked faults are judged with 1
  by other courses in the same run. A fix is sketched and replayed:
  23 of 24 cases right.
- **F2 (defect, low, certain).** A status without `uploaded` or
  `upload_blocks` makes the INFO pace line the rig's error.
- **The stand-in matches #144's engine at its head now** (f6ef194 is
  on 28d1d64), where no qualifying cut is seen. It differs once one is
  seen, which this recording never reached. The pace figures are INFO,
  with no tolerance: nothing about the pace can fail on the kernel.
- **The probe's pin** is sound. One observation contradicts #144's
  fake: a `zfs set` of the value the property holds **does** write a
  group, and each assist through the CLI writes **two** groups, not
  one (L2).
- **G3 and G4 of #143** are fixed. EXPECTED, PUBLISHED and ABSENT
  are pinned both ways for passist.
- **Plants.** 52 of my 59 are KILLED; the 7 survivors are 2
  equivalent mutants and 5 low test gaps. #139's, #141's and #143's
  plants re-run at this head give what they gave at d3b75af, but for
  the plants whose text this pull request changed, which are KILLED
  when aimed again.
- **Local gates pass**, and shellcheck 0.11.0 is clean.
- **CI.** One job is red at cb91ad1: the cut rig on ubuntu-24.04
  (45600485861), with the rig's own error ("cannot export 'skzcut': pool
  is busy") in a recording this pull request did not touch. aabde27 is
  the author's answer to it (L3).
  The daemon and replay rigs were cancelled; they run no file this
  pull request changes. Codex's review failed (usage limit) and posted
  no finding.

## Findings, most severe first

### F1. The passist premises rest on how long the daemon let the agent run (defect, medium; certain)

**Where.**

- `publish_check.py:975`: `assist.tsv` must exist.
- `:982-986`: `premise_a1_needed_the_assist` is
  `bool(mine) and mine[0]['before'] < a1['group'] <= max(r['after'] for r in mine)`.
- `:1000`: `premise_a2_assisted` is `len(a2) >= 2`.
- `publish_agent.py:386-405`: the agent assists only while the daemon
  answers `pending`, and returns at the first `done`, `expired` or
  refusal.

**The mechanism.** The number of rows in `assist.tsv`, and how far a1's
rows reach, is decided by when the daemon stopped answering `pending`.
A daemon that ends a request early leaves too few rows. The premise
then fails and the course ends 2, although the pins have a verdict
against the daemon.

The description says the premise "rests on the kstat alone, not on the
daemon's cut, so a daemon that does not count the assist's cut still
ends 1". That is true of that one fault (D11 below). It is not true of
the premise as a whole: its rows are the daemon's answer.

The rule this breaks is the one the notes record for M2.T7b (#141's
B2/D-b): "Premises read the request log, the agent's records or the
pool, never the daemon's own answers, so a real fault is a verdict
(exit 1) and not the rig's error (exit 2)."

**The replay.** `replay145.py` copies the run's passist recording, with
a stand-in node B built from its own results. It changes the recording
one way at a time and runs `publish_check.main`. The full output is in
`replay145.out`.

| Case | The daemon's fault | Pins that fail | Exit | Should be |
|---|---|---|---|---|
| P0 | none (the run as recorded) | none | 0 | 0 |
| D1 | a1 never carried, though assists wrote 22 and 23 (at or past G 20) | `pin_answer_a1`, `pin_a1` | 1 | 1 |
| **D2** | a1 published on cut 11 of group 19 (below G) right after assist 1 | `pin_a1_row` | **2** | 1 |
| **D3** | a1 published on group 17 before any assist | `pin_a1_row` | **2** | 1 |
| **D4** | G ignored: a1 on group 16 and a2 on group 31, before any assist | five pins | **2** | 1 |
| D5 | a2 published after its second assist | four pins | 1 | 1 |
| **D6** | a2 published after its first assist | four pins | **2** | 1 |
| **D7** | a2 expired after 5 s (900 asked) | `pin_answer_a2`, `-withdraw` | **2** | 1 |
| **D8** | a2 refused at once | `pin_answer_a2`, `-withdraw` | **2** | 1 |
| D9 | a2's withdrawal ignored | `pin_answer_a2-withdraw` | 1 | 1 |
| D10 | a2-look answers a snap | `pin_answer_a2-look` | 1 | 1 |
| D11 | no cut counted for the assists' groups (cuts of 18 to 67 dropped) | `pin_answer_a1`, `pin_a1` | 1 | 1 |
| D12 | the carrying cut opens only as a rewind | `pin_a1_cut` | 1 | 1 |
| D13 | the status after a2 lacks `upload_blocks` | none | **2** | 1 (F2) |
| R1 to R8 | the rig's faults: no assist; pool not idle; assists that write nothing; files missing, short or torn | (various) | 2 | 2 |
| **V1** | a1 carried by a group no assist wrote (G1) | none | **0** | 2 |
| V4 | a1 carried before any assist by a group something else wrote | `pin_a1_cut` | 2 | 2 |
| V2, V3 | an assist storm; assists from 1 s (G2) | none | 0 | not judged |

**What it costs.** The run stays red in every case: 2 fails the job as
1 does. What goes wrong is the label: the reader is told to suspect the
rig. The course's own targets end 1 (D1 and D11). The masked faults are
judged elsewhere in the same run:

- a G ignored by pplain's t4 (asked 20 groups up, with no premise of
  this kind);
- a far-ahead G refused, or expired early, most likely by prewindc's c1,
  which is asked the same way (`--group-plus 1000000 --expiry-s 900`).
  I did not replay that course with these faults.

That is why I do not hold it as blocking. By #141's line, B2 blocked
because the course's own target ended 2, and that is not the case
here.

**A fix, verified** (`premise_sketch.diff`, 92 lines):

1. The agent records the pool's newest group that wrote when the
   request was accepted, and again when its end was read, in
   `accepted.tsv`. Both are kstat reads, whatever the daemon answered.
2. premise_a1 becomes `w_acc < G and (crossed if w_end >= G else not pended)`:
   - the pool was below G when a1 was accepted;
   - if it reached G by the end, an assist of a1's took it across G;
   - if not, a1 must not be `pending` (it was not carryable, so a
     `done` is the daemon's fault and the pins judge it).
3. For a2: if its end is not `pending after`, `obs_assist_pace` is NA
   and the premise is skipped; `pin_answer_a2` has judged it.
4. `assist.tsv` is read with `need=False`.

**Result:** 23 of the 24 judged cases end as they should
(`replay145_sketch.out`). D2, D3, D4, D6, D7 and D8 end 1, and V1 ends 2.
Only D13 remains, which is F2.

- Seven of the pull request's tests pin today's premise or fixture shape
  and fail under the sketch. They would need `accepted.tsv`, and the
  fake's synced reads, in their fixtures:
  - in `test_publish_agent.py`: `test_the_assist_sets_a_new_value_then_syncs_and_writes_nothing`
    and `test_the_group_after_the_sync_is_read_until_it_stays_put`;
  - in `test_publish_check_main.py`: `Assist.test_a_carrying_group_no_assist_wrote_is_said`,
    `test_an_assist_that_wrote_no_group_ends_1` and
    `test_the_right_course_names_the_assist_that_wrote_the_carrying_group`,
    `Premises.test_the_watcher_s_verdict_ends_1`, and
    `Right.test_each_course_as_a_right_daemon_leaves_it_ends_0`.
- One edge the sketch leaves: a1 refused at its publish, before the
  loop, writes no line to `accepted.tsv`, and still ends 2. One more
  branch for a refused end closes it.

### F2. A status the pace cannot read is the rig's error, for an INFO line (defect, low; certain)

**Where.** `publish_check.py:715-718`, called at `:1001`.

**The scenario.** The daemon's status after a2 lacks `upload_blocks`
(D13). `assist_pace` raises RigError, and the course ends 2, although
every pin passed and the line it fails for is INFO. A daemon that drops
a counter from its status reads as the rig's fault.

**Suggestion.** Make the line NA, saying which key is missing, as the
check does elsewhere for figures it cannot read.

## Test gaps and suggestions (none blocks)

### G1. a1 carried by a group no assist wrote ends 0

**Where.** `obs_a1_cut` (`publish_check.py:996`) is INFO.

The course's comment says a1 "must be done on a cut of a group an
assist wrote" (`cut-record.sh`, above `rec_passist`). Nothing judges
that. V1 (the second assist read group 23 before its set, so group 22
was written by something else) ends 0.

On an idle pool (`pidle` waits for 12 s with no write) this is
unlikely. The sketch of F1 closes it (V1 ends 2). Without the sketch,
`obs_a1_cut` as a premise would do the same.

### G2. Neither the pace nor the timing of the assists is judged on the kernel

- **Pinned:** only by `test_three_assists_a_second_apart_then_one_every_assist_after`.
  My plants A15 to A18 (the burst's size, the spacing, no wait) are
  KILLED there.
- **Not pinned:** a recording whose rows do not follow the schedule
  ends 0:
  - V2: a2 assisted every second (35 rows);
  - V3: assists from 1 s after the acceptance.
- **Why it matters:** `obs_assist_pace` then gives figures of a pace
  that is not the decided one. The description calls them "figures of
  the decided pace".
- **Suggestion:** a premise on a2's rows, against the times already in
  the files (`t0` and `requests_span`):
  - the first row at least ASSIST_AFTER after the acceptance;
  - three rows about a second apart;
  - then at least ASSIST_AFTER apart.

### G3. Plants that survive

| Plant | What survives | Weight |
|---|---|---|
| A19 | `ASSIST_AFTER = 5` | The course passes `--assist` without `--assist-after-s`, so the default is what must equal #144's `plan::ASSIST_AFTER` (10 s). No test pins the default. Low. |
| S2, S3 | `pidle` idle after 1 s; `pidle` never waits | `test_cut_record.sh` does not pin `pidle`'s wait. On the kernel, a pool that is not idle at a1's first assist fails premise_a1 (R2 ends 2), so it is not vacuous. Low. |
| S10 | a2's two statuses swapped | The INFO line's upload figures go negative. Low. |
| S13 | `zfs_txg_history` not raised in `cut-record.sh` | The default of 100 still covers the course's ~70 groups; a short history is the rig's error (R6). Low. |
| A1 | `written_txg` without the state C | Equivalent: `spa_txg_history_fini_io` sets C before `nwritten` (`spa_stats.c`, `spa_txg_history_set`, then `spa_txg_history_set_io`), so a row with `nwritten` above 0 is always C. |
| A4 | `written_txg` skips the first group row too | Equivalent in practice: the rows run oldest first, and the newest group is never the first row. |

### N1. The description (nit)

- It says "#144's head does not have it yet". #144's head 28d1d64 now
  has the pace (f6ef194); see "The stand-in against #144" below.
- The CI section reads "(to follow)". The probe's and passist's figures
  (below) are not in it yet.
- Its sentence on premise_a1 (F1).

## The questions asked

### (1) Counting only groups with `nwritten` above 0

**Used in four places:**

- `publish_agent.written_txg` (`:310-326`);
- `pidle` (`cut-record.sh:1549`);
- the probe's `written_group` (`ublk-probe.sh:298`);
- the judge's `kstat_groups` (`publish_check.py:688-704`).

**Verdict: right in every place.**

**The kstat.**

- `/proc/spl/kstat/zfs/POOL/txgs` has one column header (procfs_list)
  and no raw kstat header. `rows[1:]` skips it, and the header is not
  numeric anyway.
- `nwritten` is the sixth column.
- `$3 == "C"` and `f[2] == 'C'` leave out groups still open, quiesced,
  waiting or syncing.

**When `nwritten` is filled.**

- `txg_sync_thread` (`txg.c:600-610`) runs `init_io`, then `spa_sync`,
  then `spa_txg_history_fini_io`, and only then sets `tx_synced_txg`.
- `fini_io` sets the state C, then, under a second hold of the lock,
  `nwritten`.
- **A command that waited** (`zfs set`, `zpool sync`) therefore sees C
  and `nwritten` together.
- **A reader with no command behind it** can, for microseconds, see C
  with `nwritten` 0. That understates the newest group that wrote:
  - in an `after` read, it gives a false FAIL (the conservative way);
  - in a `before` read on an idle pool, no written group is in flight.
  - `settled()`'s two reads 0.1 s apart cover the `after` side as well.

**What `nwritten` is.** The root vdev's write bytes over `spa_sync`.
It counts every top-level vdev, log devices included, and any write
that runs alongside the sync: the ZIL on a pool with no log device,
MMP, scrub or resilver repairs, `zpool initialize`.

- So **"`nwritten` above 0" means "wrote an uberblock" only on an idle
  pool with none of those**, as the rigs' pools are.
- **An empty group writes no uberblock** (`vdev_config_sync`, when
  `uberblock_update` says nothing changed and no config is dirty).
- **On a multihost pool every group writes one.** There, "idle" never
  comes, and `pidle` and `idle_pool` end in the rig's error, not a pass.
- **An uberblock with `nwritten` 0** cannot happen: the label writes are
  writes.

**Counter width.** The counter is a u64. Python's int has no limit, and
awk's doubles are exact to 2^53 bytes, far above any one group.

**Against the recordings.**

- **passist** (45600486106): the groups the kstat says wrote are exactly
  the groups of the daemon's cuts, from 5 to 71: 16 written groups, 16
  cuts and 19 empty groups over a2's assists.
- **The probe**, on both images: a bare `zpool sync` on the idle pool
  committed 3 groups and wrote none.

**Caveats** for the note, none of which bites here:

- the window above;
- the vdev-wide count;
- the birth column is SPL's `gethrtime`, not CLOCK_MONOTONIC. The pull
  request does not use it; the request log's clock is CLOCK_MONOTONIC
  (`record.rs:232`), as is the agent's `mono`;
- the probe's `zfs_txg_history` is set with `|| :`. If that write
  failed, the default of 100 still serves the pin; only the
  `obs_assist_pace` line's "of N listed" would be short.

### (2) Whether premise_a1 rests on the kstat alone

**No.** See F1 and the replay table.

**Vacuous passes:**

- **V1** (G1): a1 carried by a group no assist wrote ends 0.
- **V2 and V3** (G2): the pace and the timing are not judged.

**No other judged case ends 0 wrongly.**

### (3) The stand-in against #144's engine

**#144 has moved.**

- **At the start of this review**, #144's head was b5063b2. Its
  `assist_due` assisted once a second, with no limit, until the request
  ended.
- **#144 is now at 28d1d64.** Its f6ef194 ("Agent: three quick syncs a
  request, then one each ASSIST_AFTER") adds `ASSIST_QUICK = 3`.
  `plan.rs:433-457` at 28d1d64 now reads, with no qualifying cut:
  - from `ASSIST_AFTER` (10 s) after `asked_ms`;
  - `ASSIST_PAUSE` (1 s) after the last sync while fewer than 3 were
    run;
  - `ASSIST_AFTER` after the last from then on.

**The stand-in matches that** (`publish_agent.py:401-405`), and so does
the recording:

- a1's two assists at 10.0 and 11.2 s after its request;
- a2's three at about 1.1 s apart, then five about 10.1 s apart.

The value matches too: #144's engine uses `now_ms.max(last + 1)`
(`engine.rs:641` at 28d1d64), the stand-in `max(wall ms, last + 1)`.

**Where they differ:**

- **With a qualifying cut (`carried`).** #144 does not sync before twice
  ASSIST_AFTER after `asked_ms`, and then once per ASSIST_AFTER. The
  stand-in does not look at the daemon's cuts, and goes on at its burst
  and pace.
  - In this recording it makes no difference. a1 became carried by cut
    12 of group 22, which assist 2 itself wrote; before assist 2 the
    newest cut was of group 19, below G. Its `done` came at the next
    look, before a third assist was due.
  - On a slower uploader, the stand-in would make a third assist where
    #144 would not.
- **The count of syncs.** #144 counts them per request (`assists`), so
  the count starts again after the request is asked anew. The stand-in
  counts them per flow. passist asks nothing anew, so this is the same
  here.
- **ASSIST_AFTER is copied by hand** (plant A19 survives).

**Does an agent that follows #144 pass the course?** Yes. One that
followed b5063b2 (once a second) would also pass: a1 the same, a2 with
about 60 assists rather than 8, and `obs_assist_pace` saying so. Only
INFO lines describe the pace (G2).

**Are the pace figures judged with tolerances that can fail?** No.
`obs_assist_pace` and the probe's `obs_assist_pace` are INFO, with no
bound.

## The probe

**The pin** `zfs_set_writes_a_group_on_an_idle_pool` (`ublk-probe.sh:432-437`):

- **Its premise:** an idle pool, 12 s with no group that wrote
  (`idle_pool`, `:315`).
- **The command:** `zfs set skyzfs:assist=$(date +%s%3N)` (a fresh value
  on a fresh pool), then `zpool sync`.
- **The test:** both commands succeeded and the newest group that wrote
  moved (`w1 > w0`).
- **It fails** where the set and its history write nothing; a bare sync
  writes nothing.
- **It passes** on either the set's group or the history's. An assist
  made through libzfs with no history record would write one group,
  which the pin still sees.
- **A kstat it cannot read, or a pool never idle**, leaves the pin
  unrecorded. The final loop (`:635`) then FAILs it "not checked".
- **The verdict comes from `PINS_FAILED`.** `ublk-flags.py` only renders:
  it exits 0 on a results file with the pin line deleted.
- **No local suite** covers the probe's lines. I planted nothing there.

**What the kernel showed** (ZFS 2.4.1 module on both images; userland
2.4.1 on 26.04, 2.2.2 on 24.04):

| Line | ubuntu-24.04 | ubuntu-26.04 |
|---|---|---|
| bare `zpool sync`, idle | 0 groups written; 3 committed (89 to 92) | 0 written; 3 committed (104 to 107) |
| the pin | PASS: 86, 93 after the set, 94 after the sync | PASS: 100, 108, 109 |
| the same value again | 94, **97 after the set**, 98 after the sync | 109, **112**, 113 |
| 30 assists, one a second | 60 groups wrote (320,102 bytes a group), 60 empty; the set wrote in 30, the sync in 30 | 60 wrote (317,440 a group), 60 empty; 30 and 30 |

**What contradicts #144's fake** (`engine/fake.rs:1252-1274` at 28d1d64):

- **A set of the value the property holds writes a group** on both
  images. The fake models no group there. That is conservative, and
  the engine never sets the same value.
- **Each assist through the CLI writes two groups**: the set's own, and
  the history record that `zfs set` logs, which the following
  `zpool sync` writes. The fake says "the `zpool sync` after it writes
  nothing more".
- **#144's `ASSIST_QUICK` doc** says the spaced pace "costs about six
  groups a minute". Through the CLI it is about twelve.

**passist adds the daemon's side** (ubuntu-26.04, a2, 70 s):

- 8 assists in 53 s;
- 16 groups that wrote and 19 that wrote nothing;
- the daemon counted 16 cuts;
- it uploaded 19 cuts and 703 blocks: about 37 blocks a cut on this
  nearly empty pool.

## G3 and G4 of #143; the tables

- **G3.** An empty `watch.log` is the rig's error (`publish_check.py:799`).
  My K24 (G3 undone) and V1x (#143's V1 aimed again at the new text)
  are both KILLED by `test_publish_check_main.py`.
- **G4.** The test comment now says `-FX`.
- **EXPECTED, PUBLISHED, ABSENT and COURSES** for passist are pinned
  both ways:
  - `Tables` checks them by equality;
  - the fake course checks them through `main`;
  - K19 to K23 are all KILLED.
- **`PUBLISH_NAMED` and the workflow's default** are pinned by
  `test_cut_record.sh` (S12 and S14 KILLED).

## Plants

**This review's** (`plants145.py`, `plants145.out`; each alone, the
file restored in a finally, 600 s a suite, the tree clean after each):
59 plants, **52 KILLED and 7 SURVIVED** (G3 above).

| Group | KILLED / of |
|---|---|
| A: the agent's assist and `written_txg` | 18 / 21 (A1, A4 equivalent; A19) |
| K: premises, pin, `kstat_groups`, `read_assists`, the pace, the tables, G3 | 24 / 24 |
| S: `pidle`, `rec_passist`, the wiring | 10 / 14 (S2, S3, S10, S13) |

**Earlier plants re-run at cb91ad1:**

- **#139's** (`review/m2.t7/next_plants_py.py`, 82 plants): 65 KILLED,
  8 SURVIVED and 9 NOT APPLIED, line for line the same as at d3b75af
  (`review/m2.t5d/p139_at_head.out`).
- **#143's**, on the publish rig's files (`review/m2.t5d/plants143.py`,
  the 38 A, V, C and S plants; `p143_at_145.out`): 35 KILLED, 1
  SURVIVED (V6, as at d3b75af) and 2 NOT APPLIED. The two are the
  plants whose text this pull request changed: V1 (the watch block,
  which G3 lengthened) and S14 (the workflow's default list, which now
  ends in passist). Aimed again at the new text (V1x and S14x in
  `plants145.py`), both are KILLED. Every other plant has the result
  it had at d3b75af.
- **#141's next look** (`review/m2.t7b/plants_next.py`, 91 plants):
  RESULT141.
- **#132's** bench plants were not re-run: this pull request touches no
  bench file.

## Local gates at cb91ad1

All pass:

- every `tests/kernel/test_*.py` and `tests/bench/test_*.py`;
- every `tests/kernel/test_*.sh`;
- shellcheck 0.11.0 over `git ls-files '*.sh'` (CI's command) is clean.

## CI at cb91ad1 (read only)

| Workflow | Run | State |
|---|---|---|
| ci | 45600485948 | success |
| coverage | 45600486118 | success |
| e2e | 45600485982 | success |
| skyzfs-kernel-probe | 45600485899 | success (24.04, 26.04; the pin PASS on both) |
| skyzfs-publish-rig | 45600486106 | success: the unit job, and all nine courses exit 0 on ubuntu-26.04 |
| skyzfs-nodeb-rig | 45600486023 | success |
| skyzfs-cut-daemon | 45600485931 | success |
| **skyzfs-cut-rig** | **45600485861** | **ubuntu-24.04 failed** (job 139157785744); 24.04-arm, 26.04, 26.04-arm and the reference passed |
| skyzfs-daemon-rig | 45600486134 | cancelled about 30 s after it started |
| skyzfs-replay-rig | 45600486060 | cancelled about 30 s after it started |

The push runs (45600335xxx) were cancelled by the pull request's.

**The cut rig's failure.**

- **What failed:** the course `full`, in `cut_check.py:571-572`:
  "rig: ERROR: zpool export after the import: cannot export 'skzcut':
  pool is busy". The job then ended with "worst exit 2", which is the
  rig's own error, not a verdict.
- **Not this pull request's doing.** Its changes to `cut-record.sh` are:
  - `pidle` and `rec_passist`, which only passist calls;
  - the name passist in `PUBLISH_NAMED`;
  - a comment.

  `zfs_txg_history` 2000 was already in the base. `cut_check.py` is
  unchanged.
- **The same message has been seen before**, in the replay rig: #128's
  review (`review/m2.t5/review.md`) recorded it. #130 then made
  `replay_check.py` try a busy export again and record what held the
  pool. `cut_check.py` does neither: one busy export ends the course.
- **What to do:** the author has answered it with aabde27 (above). Its
  cut rig on ubuntu-24.04 shows whether the export now passes.

**The daemon and replay rigs.** They are cancelled with no newer run
of the pull request; their concurrency groups are per pull request. They
start because `tests/kernel/` changed, but they read none of the files
this pull request changes:

- the daemon rig: `daemon-rig.sh`, `daemon_check.py` and its tests;
- the replay rig: `replay*.py`, `replay-rig.sh`, `hole-rig.sh`.

**Codex.** The summary comment (11:01 UTC) says the review **failed**,
and the next comment says the Codex usage limit was reached. Codex
posted no finding and no thread.

## Decisions for the lead

- **L1. Does F1 block?** My view is no. The run is red either way, the
  course's own targets end 1, and the masked faults are judged with 1
  elsewhere in the same run. But it breaks the premise rule the notes
  record for M2.T7b, and the fix is small and verified. I would fix it
  in this pull request.
- **L2. The assist's cost on the kernel against #144's model** (for
  #144 and M2.6c):
  - Through the `zfs` CLI, an assist writes two groups: the set's, and
    its history record, which the sync writes.
  - A set of the value already held also writes a group.
  - So the spaced pace is about twelve groups and twelve cuts a minute,
    not six. On this nearly empty pool that is about 440 uploaded blocks
    a minute while a publish stalls.
  - Whether the real `Zfs::assist` should go through libzfs without a
    history record (one group), and whether #144's fake and its
    `ASSIST_QUICK` doc should say two groups, is for the lead and
    coding.
- **L3. The cut rig's "pool is busy" on 24.04.** This export failure
  has now been seen in `cut_check.py` too, which at cb91ad1, unlike
  `replay_check.py` since #130, neither tried again nor recorded what
  held the pool. The author's aabde27 adds both, inside this pull
  request. Should a fix to another rig ride in this work package, or go
  in its own? Either way, aabde27 needs a review of its own.
- **L4. Should the course judge its schedule (G2)?** It would make
  `obs_assist_pace`'s figures known to be at the decided pace. Today
  only the unit test pins it.

## Files

All in `review/m2.t5e/`:

- `replay145.py`, `replay145.out`, `replay145_sketch.out`: the replay of
  the passist recording through `publish_check.main`, at the head and
  with the sketch;
- `premise_sketch.diff`: the sketch of F1's fix (on cb91ad1);
- `plants145.py`, `plants145.out`, `plants145.json`: this review's
  plants;
- `p139_at_145.out`, `p143_at_145.out`, `p141_at_145.out`: #139's,
  #143's and #141's plants re-run at cb91ad1.

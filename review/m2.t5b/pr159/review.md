# Review of #159 (M2.T5 follow-ups; the CLI recorder's retained snapshots) at bca1a27 and b99f21a

Pull request: skyzfs/skyzfs#159 (`skyzfs-testing` into `zfs-topology`),
base 2489dda. **Head: b99f21a** (checked with `gh api .../pulls/159 --jq
.head.sha`). Diff read with `git diff origin/zfs-topology...origin/skyzfs-testing`.

Three commits:

- cb445a6 and bca1a27: the agent rig, the follow-ups G1, D5, G6, G7, G8
  and G9 of my look at #158 (`review/m2.t5/next_look.md`).
- b99f21a: the ZFS CLI recorder, #157's R1. It touches only
  `zfs-cli-record.sh`, `test_zfs_cli_record.sh` and
  `skyzfs-zfs-cli-record.yml`, so everything said here of bca1a27's rig
  and judge holds at b99f21a.

Line numbers are at b99f21a. This directory, `review/m2.t5b/`, already
holds the review of #132 (also named M2.T5b), so this one is in `pr159/`
beside it and does not replace it. The plant scripts, their outputs and
the show-case are in `pr159/`. Sets A to D reuse #158's scripts
unchanged (`../../m2.t5/pr158/next_plants_{a,b,c,d}.py`).

## Verdict

- **Not ready to merge.**
- **Every agent rig job but d is red on b99f21a, and only on false
  FAILED lines.** Two findings block:
  - **B1.** The new pin `pin_ID_stays_absent` (G7) fails right runs in
    a1, a2 and k, and did in a1 on bca1a27. The pin compares names, and
    the agent takes a name again within the same second when nothing was
    asked under it.
  - **B2.** G1's new K-sweep cases kill inside the publish proper, where
    `EXPECTED['sweep'] = 'published'` does not hold. Four right cases fail
    `pin_ID_outcome` in k.
- **Merge condition M1.** The publish rig has not run on either head,
  although its paths include `tests/kernel/cut-record.sh`, which this pull
  request changes. Its record job was cancelled both times, so the
  description's claim about the cancelled rigs does not hold for this
  one. The risk is low (M1). The lead decides whether it must run.
- **For the lead:** b99f21a is a second work package in this pull
  request, as 0604977 was in #158. B1 raises no question of design: the
  name the agent takes again was never asked of the daemon, and it never
  takes again a name it gave up (B1).
- **(a) D5 and G6, the recorder's half: done.** Verdicts go to
  `verdicts.tsv` and the case's errors to `rig-error.txt`, the hook judges
  the cases done so far, and the plants are killed. What is left is not
  blocking:
  - D7: node B's stages and the controls are never judged in the hook
    path.
  - D8: the judge's own RigError still ends the case loop, and the
    docstring says it does not.
  - D9: recover's exemption.
  - D10: the shared steps that still call `rig_error` on what the daemon
    did, and errexit, which skips the hook.
- **(b) G1's `new_points` is sound and bounded.** It covers look, release
  and unlink after the scenario's own step, and the k job took 25
  minutes. But for sweep it reaches into the publish proper, and the
  judge was not ready for that (B2).
- **(c) G8 and G9: every survivor they named that still applies is
  killed.** #158's older survivors in sets A and C that matter are still
  open (Test gaps). Newly killed since #158: 10 in set A, 8 in set B, 11 in
  set C (among them T10, T11, T12, T20, T21, T30 and T31) and 2 in set D.
  Removing the FIRST_RC entries loosens no verdict. The new code has
  survivors of its own (test gaps).
- **b99f21a: one should-fix, C1.** The batching cannot work on ZFS, and
  the fake `zfs` in the test says it does. The cost is then K calls, and
  K is not bounded. The N:K parsing is sound.

## Blocking

### B1. `pin_ID_stays_absent` fails right runs (`agent_check.py:716-725`)

**Certainty: high.** It is seen in CI on both heads, explained from the
artifacts, and reproduced by a show-case with the test's own fixtures.

**What the pin does.** `snaprow.txt` is node B's snap-row of T, read
right after the settle. Where it says `ref none`, the check reads snap/T
at node B again and FAILs if it now has a ref. It compares the name and
nothing else.

**Why a right run fails it.**
- The recorder runs the retry when node B has no snap/T after the settle
  (`agent-record.sh:365-371`). The retry is a new publish, and the header
  says it must publish (`agent-record.sh:22-25`).
- The agent names a snapshot `skz-<UTC second>`. It adds `-1`, `-2` and
  so on, taking the lowest suffix not in the pool.
- The settle can destroy the killed run's `@T`. Design §5.4 says: "it
  destroys @t recursively and takes the snapshot again under a new name".
  The settle then publishes that new snapshot as T-1 or T-2. Or the killed
  run never took `@T` at all (killed at intent:before).
- Either way the name T is free again. A retry in the same second takes
  a new snapshot under the name T and publishes it. Node B then has
  snap/T, and the pin fails.

**Evidence.** These are the agent rig's red jobs. Each job has no other
FAILED line. The createtxg comes from `published.tsv`.

| Run (head) | Job | Case | T | The settle printed | The retry printed | What T is at the check |
|---|---|---|---|---|---|---|
| 45772672638 (bca1a27) | a1 | A-intent-before-1 | skz-20261126T070731Z | nothing | snap/T | a new snapshot: the killed run never took one |
| 45774302182 (b99f21a) | a1 | A-intent-before-1 | skz-20261126T072331Z | nothing | snap/T on 6d1c144af192 | the same |
| 45774302182 (b99f21a) | a1 | A-manifest-before-1 | skz-20261126T072333Z-1 | snap/…072333Z-2, createtxg 31 | snap/T on 0b3a6c5b166f | createtxg 34: taken after the settle's -2, so not the killed run's |
| 45774302182 (b99f21a) | a2 | A-take-after-1 | skz-20261126T072113Z | snap/…072113Z-1, createtxg 28 | snap/T on c303aaa2c930 | createtxg 31: taken after the settle's -1 |
| 45774302182 (b99f21a) | k | K-sweep-intent-before-1 | skz-20261126T073501Z | nothing | snap/T on 4263c8398509 | a new snapshot: the killed run never took one |

- In each of these cases `pin_ID_criterion`, `pin_ID_settle`,
  `pin_ID_clean`, the row and node B's pins hold. The criterion holds
  because a retry did publish.
- On b99f21a the pin was judged and failed as follows: in a1, three
  times with two failures; in a2, twice with one; in k, 28 times with
  one.
- In a1's and a2's cases where it held (A-taken-before-1,
  A-manifest-after-1), the retry ran in a later second and took a new
  name. So whether a right run passes depends on the clock.
- The show-case (`pr159/showcases159.py`, output
  `showcases159.out`) builds the a1 case with `test_agent_check`'s
  fixtures at bca1a27. It ends exit 1, with `pin_x7_row` and
  `pin_x7_nodeb` ok.

**Fix (the author's choice).**
- Give the pin G7's scope: a T that was given up, that is, a name in the
  settle's or the first run's `abandoned` (the destroy and tooold cases
  among them). The agent never takes such a name again (below).
- Or judge identity, not the name. Make it INFO, not FAILED, when node
  B's ref for snap/T is the commit the retry printed for snap/T. Or
  compare `published.tsv`'s guid with the retry's snapshot.
- A probe (K24: skip the pin when the retry printed snap/T) survives
  every local test, so it breaks none. Whatever fix is taken needs a test
  of its own: no fixture has a retry that prints T (K21, below).

**The same root, outside this pull request.**
- In these cases `obs_ID_outcome` and `pin_ID_criterion` say "T
  published". The snapshot published as T is the retry's new one and not
  the killed run's (its createtxg is above the settle's).
- No verdict is wrong for that, since the criterion only needs "a retry
  publishes". But the line misnames which snapshot was published.

**Why the reuse is not a defect of the agent.**
- `Agent::fresh_name` (`crates/skyzfs-agent/src/engine.rs:1604-1619`)
  skips this second's names that are in the record, in its `abandoned`
  list, or renamed in this process. Its comment says that renamed names
  are not recorded.
- So a name that was given up is never taken again. What the retry
  takes again is one of two things:
  - a name that the settle renamed away, before anything was asked of
    the daemon (the take is redone at the manifest, §5.4);
  - a name the killed run never recorded (killed at intent:before).
- In both cases node B never had that name, so taking it again harms
  nothing. This also gives the cleanest fix: judge the pin only for an
  abandoned T, which the agent cannot reuse, and that is G7's own scope.

### B2. G1's new K-sweep cases fail right runs on `EXPECTED['sweep']` (`agent_check.py:202, 235-253, 340-351, 699-704`)

**Certainty: high.** It is seen in the k job on b99f21a (job
139754496336, run 45774302182, failure) and explained from its artifact.
At bca1a27 the k job was cancelled before it got there.

**The FAILED lines.** Four lines `pin_K-sweep-ID_outcome`, plus one B1
line (`pin_K-sweep-intent-before-1_stays_absent`). They are the job's only
FAILED lines.

| Case | T | The settle printed | The retry printed |
|---|---|---|---|
| K-sweep-take-after-1 | skz-20261126T073518Z | snap/…073518Z-1 | snap/…073520Z |
| K-sweep-manifest-before-1 | skz-20261126T073523Z | snap/…073524Z | snap/…073525Z |
| K-sweep-manifest-after-1 | skz-20261126T073529Z | snap/…073529Z-1 | snap/…073531Z |
| K-sweep-taken-before-1 | skz-20261126T073534Z | snap/…073535Z | snap/…073537Z |

Each says "T: absent, not published". In each, `pin_ID_criterion`,
`pin_ID_settle`, `pin_ID_first_publish` and `pin_ID_clean` hold.

**Why.**
- In the sweep scenario the case's first point (`sweep:before`) is one
  the base never reaches. So `new_points` returns the whole trace, capped
  (37 sweep cases in this job): kills at intent, take, manifest and taken,
  in the publish proper and not only in the sweep.
- A kill between the take and the manifest makes the settle destroy `@T`
  and take it again under a new name (§5.4). T is then absent, which is
  right.
- In the plain course the same cases have `EXPECTED['plain'] = None`, so
  their outcome is INFO. A2's A-take-after-1 on this head is the same
  sequence, with INFO.
- `EXPECTED['sweep'] = 'published'` was written for kills in the sweep
  itself. Applied to these cases, it fails right runs.

**Fix (the author's choice).** `expected()` (340-351) returns None for a
plain publish that was killed, and `EXPECTED[scen]` for every other
scenario. Make it judge a K case whose kill is in the publish proper as
the plain course does: the outcome INFO, the criterion pinned. Or keep `'published'` for kills at the sweep's own
points only. A fixture with a K-sweep case killed at take:after, whose
settle prints a renamed T, would pin it.

**The cost of G1 is fine.** The k job ran 102 cases in 16.5 minutes and
the check in 6.5 more, 25 minutes in all, against its 150-minute limit.

## Merge condition

### M1. The publish rig has not run on either head

- `skyzfs-publish-rig.yml:56` lists `tests/kernel/cut-record.sh` among
  its `pull_request` paths.
- This pull request changes `rig_error` in that file
  (`cut-record.sh:228-240`), and the publish rig runs its courses through
  it.
- Its record job, "publish through skyzfsd and check", was cancelled on
  both heads (runs 45772672358 and 45774302326). Its unit job, which runs
  `test_cut_record.sh`, passed.

**Risk: low.**
- With no hook, which is every course but the agent's, the new
  `rig_error` differs from the old only in that the message loses a
  trailing space (`printf '%s'` in place of a here-string).
- A green run never calls `rig_error` at all.
- The cut-daemon and node B rigs run `cut-record.sh` with no hook, and
  both are green on b99f21a. So is the cut rig on three images, with
  ubuntu-24.04-arm still running at my last read.

By the lead's rule in #158, a rig whose paths the diff touches runs, and
is green, on the final head. The publish rig is to be dispatched on the
final head, or the lead waives the rule.

The daemon, replay and kernel-probe rigs trigger on `tests/kernel/**`,
but none of their workflows, and none of the scripts those run, name a
changed file (`git grep` of the four changed scripts over `tests/` and
`.github/`). Cancelling them was right.

## Defects, not blocking

### D7. In the hook path node B's stages and the controls are never judged

Files: `agent-record.sh:603-611`, `agent_check.py:789`,
`nodeb_check.py:318-320`. Certainty: high, by reading.

- `agent_judged` runs the check from inside `rig_error`. That is before
  `agent_end`'s `export_pool` (`agent-record.sh:612-614`), so node A's
  pool is still imported and the daemon is serving.
- The check's first `nb.verify` (`agent_check.py:789`) calls
  `NodeB.snapshot_files`. That raises `RigError('a pool named skzcut is
  still imported')` (`nodeb_check.py:318-320`).
- So in the hook path the names loop and the controls (823-849) never
  run. A node B verdict on a snapshot published before the error
  (`pin_N_nodeb`) is not judged.
- The recording then ends 2 unless a case's own line FAILED.
- The description says that a verdict among the cases done ends the
  recording 1. That holds for the cases' own pins, not for node B's.

The store can also be frozen. `rig_error` can be called while it is
frozen (freeze in destroy, tooold, dhook and both), and `agent_judged`
does not thaw it. The snap-row of `stays_absent` (`agent_check.py:720-721`)
then fails, and its RigError ends the case loop (D8).

**Fix.** Before the check, have `agent_judged` thaw the store, stop the
writer and export the pool under limits, as `agent_end` does. Or have the
check skip node B's stages with an INFO line when `rig-error.tsv` is
there.

### D8. The judge's own RigError in a case still ends the case loop (G6's judge half)

Files: `agent_check.py:124-125`; raised at 533, 588, 597, 602, 657 and
721. Certainty: high.

- The recorder's errors in a case now go to `rig-error.txt`, and the
  check skips that case (604-608). That half is done.
- A RigError the check raises itself while it judges a case still leaves
  the loop. Examples: a missing file (`no {path}`, 533), an unreadable
  `verdicts.tsv`, a daemon hook it does not know, and node B's snap-row
  failing in the new `stays_absent` (721).
- Every later case then goes unjudged, and a FAILED in one of them is
  hidden behind exit 2.
- #158's show-case 6, rerun at bca1a27 (`next_showcases_bca1a27.out`):
  exit 2, and `pin_gone_clean` is not written. Once the missing file is
  there, the same course ends FAILED, exit 1.
- The docstring's "a FAILED line is never hidden behind an error of the
  rig" (124-125) is true only of lines written before the error.

**Fix.** Catch the RigError for each case, write `premise_ID_check`
ERROR, and go on, as the `rig-error.txt` path does. Or correct the
docstring.

### D9. recover's exemption hides a daemon's death behind a case's error

File: `agent-record.sh:291-295`. Certainty: high that the exemption is
wider than it needs to be; low that a run meets it.

- When the case has a `rig-error.txt`, recover writes a note in
  `recovered.txt` and no verdict `daemon_serving`.
- Case errors are written in `scen_destroy` (444), `scen_gone` (493,
  495) and `scen_dhook` (541). Only dhook's is about the daemon's end.
- In destroy and gone the rig never stops the daemon, so a daemon that
  ended there ended on its own, and that is a verdict.
- **Scenario.** A destroy or gone case meets a case error of its own
  (`zfs release` or `zfs destroy` fails), and the daemon also ended
  during the case. The daemon's death is then only a note, and the
  course ends 2 (`premise_ID_rig`) where it should end 1.
- A daemon that died before the rig's `zfs` commands would more likely
  make them hang than fail. They run with no limit (443, 494), against
  CLAUDE.md's rule for commands that touch a pool after a failure, so
  the job's own timeout would end it.

**Fix.** Keep the exemption for dhook alone. Or write the verdict
whenever the rig did not stop the daemon itself. Put `lim` on the
release and the destroy.

### D10. The rest of D5: steps that still end as the rig's error on what the daemon did

Certainty: high, by reading. I have seen none of these in a run.

- **`kill_serve`** (`cut-record.sh:925-929`), used by restart
  (`agent-record.sh:414`) and late (567), says "kill -9 failed: the daemon
  had ended". That is a daemon that died on its own before the rig's
  kill. Through the hook it is ERROR, exit 2, unless a case failed.
- **`serve_start`** (`cut-record.sh:375`): an instance that ends before
  it is ready. In `serve_again`, after a kill, that is a take-over that
  failed, which is a verdict on the daemon.
- **`end_daemon`** (`cut-record.sh:550-551`) at the course's end: the
  daemon does not end on SIGTERM, or ends non-zero. `reserve` now calls
  the same thing a verdict (`daemon_stopped`). Its message names
  `serve.log`, while the agent courses' current instance logs to
  `serve-K.log`.
- **errexit.** A command that fails under errexit in a course, with no
  `|| rig_error`, ends the recording through `cleanup`
  (`cut-record.sh:298-320`), which does not call `ON_RIG_ERROR`. The cases
  done are not judged, and the job ends with recordings.tsv's line "a
  command failed", exit 2.

## b99f21a: the CLI recorder's retained snapshots (#157's R1)

**Scope.** This is a second work package in this pull request. Whether it
stays here as its own commit, as 0604977 did in #158, is the lead's call.

### C1 (should fix). The batching cannot work on ZFS, and the test's fake `zfs` says it does

Files: `zfs-cli-record.sh:988-1027`, `test_zfs_cli_record.sh:157, 286-287,
747-751`.

**Certainty: high, from the OpenZFS source.** I could not run it here:
there is no ZFS in this container.
- `zfs_ioc_snapshot` refuses two snapshots of one file system in one
  call with EXDEV ("This must be the only snap of this fs").
- libzfs prints "multiple snapshots of same fs not allowed" for that.
- Apart from the refusal, `dsl_dataset_snapshot_check_impl` allows one
  snapshot of a dataset in a transaction group. So K snapshots of a
  dataset take K groups however they are batched.

**What happens.**
- `retain` gives `zfs snapshot -r` up to 10000/(N+1) names a call:
  `pool@r1 pool@r2 …`. With `-r`, every name covers the same N+1 file
  systems.
- For every N below 4999 (per ≥ 2), the first call is refused. about.txt
  then says "one name a call", and retain makes K calls of one name each.
- At N ≥ 4999, per is 1 from the start.
- So the batching is never used. The commit message's "as many names a
  call as make about 10000 snapshots" does not happen.
- **The test pins the opposite.**
  - The fake `zfs` accepts several names unless `FAKE_SNAP_ONE` is set
    (157).
  - "retain: all in one call" (747) and "scale 3:2: the snapshots before
    the take" (286) expect one call of several names.
  - "scale 3:2: said" (287) expects "2500 names a call", which a real run
    never writes.
- The refusal line says "of $per names" (2500), not the number of names
  the call had (3 at K=3). That is a nit.

**The cost.**
- K calls run in sequence. Each one is a sync task (one group), a
  process, and `lim`'s polling every 0.1 s (`tests/lib/lim.sh:50`). That
  is at least 0.1 s a call, and more on a runner.
- The bound (N+1)·K ≤ 200000 does not bound K. `1:100000` and the
  description's own example `10:10000` pass both the workflow and
  `scale_all`.
- `10:10000` is 10000 calls, at least 17 minutes. `1:100000` cannot end
  within the 80-minute budget.
- The budget check before each call (`scale_limit`) stops it, and
  about.txt says "(stopped: the budget was spent)". The outcome is a run
  with too few retained snapshots, or no scale records, not a silent one.
- The steps after `retain` share that budget, so retaining spends the
  time the measurement needs.

**Asks.**
- Drop the batching and say "one name a call".
- Bound K, or the share of the budget that `retain` may take.
- Make the fake `zfs` refuse two snapshots of one file system by
  default, as ZFS does, and pin about.txt's line on that.
- Correct the commit message's and the description's claim.
- The dispatch 45774416615 was still running when I last read. If it ran
  an N:K entry, its about.txt will show the fallback line.

### The N:K parsing is sound

Files: workflow lines 135-147; `scale_all` (`zfs-cli-record.sh:796-840`).

I ran the workflow's check and `scale_all`, both taken out of the head's
files, on these values (`pr159/nk_check.sh`, run from a checkout's root;
its output is `nk_check.out`):

- **Exit 0:** `''`, `10`, `10:1000`, `10:0`, `100000:1`, `1:100000`,
  `10:10000`, `4:40000`, `10:1000 10:1000`, `' 10:1 '`.
- **Exit 64:** `10:`, `:5`, `0`, `0:5`, `10:02`, `010`, `1:2:3`, `10::5`,
  `100000:2`, `100001`, `1:100001`, `1:999999`, `9999999`, `10:x`, `a`,
  `5 6 7 8`, `1:99999999999999999999`.
- A tab is refused by the character class. An empty input runs nothing.
- `scale_all` agrees:
  - It keys "seen" on N:K, so `10` and `10:0` are one entry ("10:0
    twice").
  - It refuses a fourth entry.
  - K=0 records under the plain label `scale-N`.
- The only concern is K's time (C1).

### Other points on `retain` (each low or a nit)

- **Odd bytes:** none possible. The pool is `skzqs`, and a name is `@r`
  followed by a decimal count. Nothing from the input reaches a name but
  digits.
- **An empty batch:** impossible. per ≥ 1 (995), and the inner loop runs
  while j ≤ K, with i ≤ K.
- **K not a multiple of the batch:** handled. "two names a call at 4999
  datasets" (748) gives 2 and then 1.
- **A failed first batch:** it is tried again one name a call from @r1.
  That is right, since a call is atomic: all or none.
- **A failed later batch:** retain stops and gives the reason in about.txt.
- **An edge case:** a first batch cut by `lim`'s limit (124) could still
  complete in the kernel. The retry of @r1 would then fail with EEXIST,
  and retain stops and says so.
- **`have`** (1022) is reported but never compared with (N+1)·K. When
  retain stopped short, the records are still labelled `-sK`. They could
  say how many there were, or skip the steps.
- **"made in N s"** includes the `zfs list`'s own time.
- **Cleanup.** The retained snapshots go with the pool (`lim 600 zpool
  destroy -f`, 1135), and leftovers are destroyed at the start (1184).
- **`take-cli-get-pairs`** (1106-1108) times the agent's pairs read, as
  the commit says.

### Tests

- `bash tests/kernel/test_zfs_cli_record.sh`: all 510 checks ok locally,
  in 68 s.
- The dispatch 45774411213's unit job failed on "window beside snapshot:
  txg's log began a margin before".
  - That check has a 0.2 s margin (`test_zfs_cli_record.sh:590`) and is
    already in the base (line 573 there), so the failure is not this
    commit's.
  - The push run's unit job passed on the same head.
  - It is a flake worth a line to the author.
- Plants in `retain` and `scale_all`, one at a time (restore, a 600 s
  timeout): see "Plants", set R.

## Test gaps

These are plants that survive. I give each its consequence, most serious
first.

- **K21** (`stays_absent` judged for every T) and **K22** (a failure of
  node B read as absent) survive.
  - Only the gone fixture has a `snaprow.txt`, and FakeNodeB never fails
    snap-row.
  - That is how B1 got through: no fixture has a retry that prints T.
- **No fixture has a sweep case killed inside the publish** whose settle
  renames T. That is how B2 got through; the first run of the full k job
  found it.
- **R09 and R10** (`retain` with no budget check, and with no time limit
  a call) survive.
  - The budget is what stops C1's K calls, and no test has a budget that
    runs out inside `retain`.
- **U13** (`acase` without `recover`) survives. Nothing tests that a case
  starts from a recovered state.
- **U11 and U12** (a stop that hangs is not killed, or is not a verdict,
  in `reserve`) survive. The path of a stop that hangs has no test.
- **U14 and U15** (destroy goes on after its error; its log is not
  moved), **U16** (the same in gone), **U18** (dhook writes no verdict
  `asked`), and **U19, U20 and U21** (late's verdicts, dhook's lost pid)
  survive.
  - Each scenario's error path is untested past `case_error` and
    `verdict` themselves.
- **Still open from #158.** Set A's 13 survivors and set C's 8 survive,
  as they did there.
  - #158's first review named several of them as gaps that matter: P43,
    P46, P48, P52, P60, P61, P67, Q06 and Q23 in the judge, and S15,
    S16, S17, S22 and S23 in the recorder.
  - G9 did not take them up, so they stay open, with the same weight as
    there.
  - N05, N07 and T09 are benign (#158's next look).
- More results are in the plant tables below.

## Nits

- **`agent_judged` names the wrong case.** CDIR is not cleared between
  cases, so a rig error after a case (in `agent_end`'s `writer_off`, for
  example) names the previous case as the one under way in
  `rig-error.tsv` (`agent-record.sh:605`).
- **`ON_RIG_ERROR` stays set.** It is cleared only when it is used, never
  at the end of a course.
- **"Did not reach asked in 60 s"** is a verdict in late and dhook but a
  case's error in restart and destroy.
- **`await_agent`'s bound is shorter than the agent's.** `await_agent`
  waits 300 s after the thaw (destroy, tooold, both), and the agent's own
  limit is `--limit-s 600`. An agent that is slow but within its limit
  gets `publish_ended` FAILED. That is unlikely, but it is a false FAILED
  if it happens.
- **`reserve` can write a second verdict.** It writes `daemon_stopped`
  for status 137 on SIGTERM. If a hook armed on that instance fires on
  the way down, that is a second verdict besides the `asked` the case
  already wrote, not a false one.
- **`new_points` depends on timing.** Its "last reach" depends on the
  base run. In the k job at bca1a27, K-destroy-look-before-2 and
  K-destroy-look-after-2 did not reach their point (first run 3). FIRST_RC
  judges them right, but they test nothing.
- **shellcheck 0.9.0** reports SC2218 at `test_agent_record.sh:141, 146`.
  CI's newer shellcheck passes (ci lint green on bca1a27 and b99f21a), so
  this is not a finding.

## The author's asks

### (a) D5 and G6

**Can a rig error hide a verdict?** Yes, in these ways:
- D7: node B's stages in the hook path.
- D8: the judge's own RigError in a case.
- D9: recover's exemption.
- D10: `kill_serve`, `serve_start`, `end_daemon`, and errexit.

The recorder's own paths now do what the description says:
- A verdict goes to `verdicts.tsv`, which becomes `pin_ID_PIN`.
- A case's error goes to `rig-error.txt`, which becomes `premise_ID_rig`,
  and the next cases are still judged.
- A `rig_error` in an agent course judges the cases done (1 on a
  FAILED, otherwise 2).
- Plants K10-K17, U01-U04, U06-U10 and U22-U29 are killed (tables
  below).

**Can a premise failure become a false FAILED?** Not through the premise
lines. But B1 (the new pin) and B2 (sweep's expectation, met by G1's new
cases) are false FAILEDs in right runs. A smaller one is in the nits:
`await_agent`'s 300 s.

**Can exit 1 and exit 2 come out in the wrong order?** No, inside a
recording:
- `any_failed` puts 1 over 2 in the judge.
- `rig_error` keeps 1 only when the hook returns 1, and makes anything
  else 2. A hook that returns 0 still makes 2, which is right, since a
  rig error happened.
- Each agent job runs one recording, so `cut-record.sh`'s max over
  recordings (where 2 would beat 1) does not apply there.

**Making the judge fail a right run:** B1 and B2, both seen in CI.
**Making the judge pass a run that should fail:** the hook path with a node B failure (D7), and a later
case behind the judge's own RigError (D8, show-case 6).

### (b) G1, `new_points` (`agent_check.py:235-253`)

**Sound.**
- It returns every point from the first one the base did not reach, so
  it covers look, release and unlink after the scenario's own step.
- In the k job at bca1a27, K-restart had 20 cases from count:before:1
  on, K-assist 18, and K-destroy 10 (withdraw, look 1 and 2, abandon,
  unlink).

**Bounded.**
- After the first new point it keeps at most four reaches for each
  (effect, side): reaches 1 to 3 and the last.
- The k job had run 52 cases in about 10 minutes when a push cancelled
  it, against 32 cases in 15 minutes before. Its job timeout is 150
  minutes.
- The k job on b99f21a ran 102 cases in 16.5 minutes (37 of them sweep,
  21 restart, 19 assist, 13 tooold, 9 destroy), and 25 minutes in all.
- **Coverage is sound, but the judge is not ready for the sweep cases.**
  For sweep, the first new point is the sweep's own first point, so the
  cases cover the whole publish. There `EXPECTED['sweep']` fails right
  runs (B2).

**Edge cases:**
- **The base reaches nothing:** everything, from the start, with the cap.
- **A point reached once, or fewer than three times:** all its reaches
  are kept.
- **A rare trace with no new point:** an empty output, exit 0, and no K
  cases.
- **An empty rare trace:** no error. Making it one (K08) survives and
  would be a change, not a fix.

**Killed:** S13a and K01-K07, K09. Making the first reach the only one,
starting from the beginning, the old set difference, no last reach,
`NEW_REACHES` 2 or 4, and taking the last reach over the new part only
are all caught.

### (c) G9: the plants again, one at a time

Method: restore, a 600 s timeout each, and the anchor checked to match
exactly once.

- **Set A** (`agent_check.py`) gives 13 killed and 13 survived.
  - Newly killed since #158: P02, P17, P18, P20, P58, Q17, R03, R06, R09
    and R11.
  - Survivors: P43, P46, P48, P52, P60, P61, P64, P65, P67, P73, P74, Q06
    and Q23. They are the same as at #158, and still open (Test gaps).
- **Set B** gives 31 killed and 2 survived (N05 and N07, benign at
  #158). N12, N13, N20 and N23 no longer have an anchor: the code they
  changed is gone.
  - Newly killed: N03, N04, N18, N34, N41, N62, N63 and N64.
  - N12a and N13a, adapted to the new `not_reached`, are killed.
- **Set C (shell):**
  - Killed: S14, T01-T07, and the seven survivors G8 named: T10, T11,
    T12, T20, T21, T30 and T31.
  - Survived: S03, S05, S15-S18, S22 and S23, as at #158.
  - S13, S20 and S21 have no anchor. Adapted, S13b and S20a are killed
    and S21a survives.
- **Set D** (`published()`): 9 of 10 killed. T09 (the gone line
  overwrites, keeping only the last) survives; the gone pin still fails
  on what is kept.
- **The FIRST_RC removal loosens no verdict.**
  - gone and both with no kill are now the premise error
    `premise_ID_killed` (KILLED_ONLY). That is stricter than a first-run
    pin against an entry that was never read.
  - unmarked never had a first-run pin. It keeps `pin_unmarked_refused`
    (first run 6).
  - The test pins `set(FIRST_RC) == set(EXPECTED) - KILLED_ONLY -
    {'unmarked'}`.
  - Plants K18 and K19, which put the old meanings back, are killed.
- **The author's claim** that "the 23 that still apply are killed at
  bca1a27" holds for every plant I redid.

## Plants

`KILLED` means that a test failed with the plant in place. Sets A to D
are in `redo_{a,b,c,d}.out`. The new ones are in `new_py.out`,
`new_sh.out` and `retain.out`, and their scripts are beside them.

### New plants in the judge (`new_py.py`, at bca1a27)

| Plant | | Result |
|---|---|---|
| K01-K07, K09, S13a | new_points' variants (above) | KILLED |
| K08 | an empty rare list is an error | SURVIVED (benign) |
| K10, K11 | rig-error.tsv gives no exit 2 / is ignored | KILLED |
| K12 | points-error as INFO | KILLED |
| K13, K14, K15 | verdicts only for listed cases / as premises / the pin name unchecked | KILLED |
| K16, K17 | a case's rig error silent / judged further | KILLED |
| K18, K19 | gone/both with no kill not an error / judged on FIRST_RC 0 | KILLED |
| K20 | stays_absent never fails | KILLED |
| K21 | stays_absent for every T | **SURVIVED** (gap; B1) |
| K22 | node B's failure read as absent | **SURVIVED** (gap) |
| K23 | stays_absent only without the snaprow file | KILLED |
| K24 | stays_absent skipped when the retry printed T (the fix probe) | SURVIVED (as hoped) |
| N12a, N13a | first_run and a reach not made | KILLED |

### New plants in the recorder (`new_sh.py`, at bca1a27)

Test command: `bash tests/kernel/test_agent_record.sh`, with
`test_cut_record.sh` added for V01-V05.

22 of 38 killed.

| Plant | | Result |
|---|---|---|
| S13b, S20a | rare's first three reaches in the shell again; `await_agent` with no verdict | KILLED |
| U01-U04 | `verdict` keeps tabs; `case_error` overwrites; `await_agent` writes no timeout status / keeps the pid | KILLED |
| U06-U09 | recover: the store not thawed; a verdict even after the rig's error; the daemon not served again; its status never read | KILLED |
| U10 | reserve: a non-zero stop not a verdict | KILLED |
| U17 | frozen_until_withdrawn: no verdict, the settle runs | KILLED |
| U22-U29 | agent_begin with no hook; agent_judged: the status lost, no case in its line, no `checked` line, CHECK=none ends 0; aplain/arare: no `points-error` line; agent_begin keeps the old lists | KILLED |
| V03, V04 | rig_error: the hook not called; no `rig error` line | KILLED |
| U11, U12 | reserve: a stop that hangs is not killed / not a verdict | **SURVIVED** (gap) |
| U13 | acase: recover not called | **SURVIVED** (gap) |
| U14, U15, U16 | destroy goes on after its error; destroy's and gone's log not moved | **SURVIVED** (gap) |
| U18, U19, U20, U21 | dhook: no verdict `asked`; late: no verdict `asked` (and goes on), no verdict `landed`; dhook: a lost pid not a case error | **SURVIVED** (gap) |
| U30 | serve_log names serve-K.log for the first instance too (in a message only) | SURVIVED (low) |
| U05 | recover sends no SIGCONT before SIGKILL (SIGKILL ends a stopped process anyway) | SURVIVED (equivalent) |
| V01, V05 | rig_error: the mapping of a hook status to 2 removed or widened. It is equivalent for statuses 0, 1 and 2 (rc starts at 2) and differs only for a check that dies with another status, which no test tries | SURVIVED (low) |
| V02 | rig_error: the hook not cleared before it runs (a guard against a hook that calls `rig_error` itself; `agent_judged` does not) | SURVIVED (low) |
| S21a | vrun: the run directory itself (old code, #158's S21) | SURVIVED (as at #158) |

### Set R: `retain` and `scale_all` (`retain.py`, at b99f21a)

Test command: `bash tests/kernel/test_zfs_cli_record.sh`.

15 of 27 killed. Every killed plant failed on a check of the code it
changed, and none failed on the timing flake. I ran two worktrees side by
side to finish sooner: `retain.py` up to R13, and `retain_b.py` for the
rest.

| Plant | | Result |
|---|---|---|
| R01 | per = 10000/N | KILLED |
| R03, R05 | no fallback (stop); the fallback skips the first batch | KILLED |
| R06, R07, R18 | one name too many a batch; the last name dropped (an endless loop: timeout); the same names every batch | KILLED |
| R11 | `zfs snapshot` without `-r` | KILLED |
| R15, R16, R17 | retain never runs; the label `-sK` always; retain after the steps | KILLED |
| A01, A02, A03, A05, A10 | `scale_all`: K=0 refused; K with a leading 0 accepted; the bound ×10; "seen" keyed by N alone; the bound N·K | KILLED |
| R09 | `retain` with no budget check | **SURVIVED** (gap: the one thing that bounds C1's cost is untested) |
| R10 | `zfs snapshot` with no time limit a call | **SURVIVED** (gap) |
| R08 | a failure of a later batch goes on | SURVIVED (gap, low) |
| R02 | no `per ≥ 1` guard: per is 0 at N ≥ 10000, and retain then stops at once | SURVIVED (gap, low: no test with K at N ≥ 10000) |
| R12, R13 | about.txt's line gives the first batch size / counts the root's snapshots only | SURVIVED (low: report only) |
| R14 | `retain` runs at K=0 too (one more line in about.txt) | SURVIVED (low) |
| R04 | the fallback on any batch, not only the first | SURVIVED (benign: on ZFS the first batch is refused anyway) |
| A04, A07, A08, A09 | `scale_all`: no length check of K (bash arithmetic can overflow); K taken as the last field (`1:2:3` runs as 1:3); an empty N; an empty K accepted | SURVIVED (low: the workflow refuses all four first; only a run by hand reaches them) |

R01, R03, R05, R06 and R18 are killed by checks that pin the batching.
Those checks change with C1's fix, so the fix needs checks of its own.

## CI on b99f21a (last read 07:57 UTC)

| Run | Workflow | Result |
|---|---|---|
| 45774295371 | skyzfs-ci (push) | success |
| 45774295392 | skyzfs-zfs-cli-record (push) | success |
| 45774302179 | ci | success (changes, lint, test, unit-arm64, chart-lint, s3-backends) |
| 45774302162 | e2e | success |
| 45774302073 | skyzfs-cut-daemon | success |
| 45774302129 | skyzfs-nodeb-rig | success |
| 45774302175 | skyzfs-cut-rig | 26.04, 24.04, 26.04-arm success; 24.04-arm still running |
| 45774302069 | coverage | success |
| 45774302182 | skyzfs-agent-rig | a1 **failure** (B1), a2 **failure** (B1), k **failure** (B2 ×4, B1 ×1), d success |
| 45774302326 | skyzfs-publish-rig | record job cancelled; unit success (M1) |
| 45774302058, 45774302210, 45774302484 | daemon rig, replay rig, kernel probe | cancelled (right: none of their files) |
| 45774411213 | zfs-cli-record (dispatch) | failure: the unit flake above; record skipped |
| 45774416615 | zfs-cli-record (dispatch) | unit and lua success; the record job on ubuntu-26.04 still running |

On bca1a27, agent rig run 45772672638 had a1 failure (B1), a2 and d
success, and k cancelled by the push of b99f21a after 12m28s. ci's lint
was green on bca1a27 (run 45772672430).

## Local runs (bca1a27; b99f21a for the CLI recorder)

| Command | Result |
|---|---|
| `python3 -I tests/kernel/test_agent_check.py` | 65 tests OK |
| `bash tests/kernel/test_agent_record.sh` | all ok, about 32 s |
| `bash tests/kernel/test_cut_record.sh` | all ok |
| `python3 -I` test_publish_check, test_publish_check_main, test_cut_check, test_nodeb_check, test_cut_daemon_check | OK |
| `bash tests/kernel/test_zfs_cli_record.sh` (b99f21a) | 510 ok, 68 s |
| shellcheck 0.9.0 `-S warning` on agent-record.sh, cut-record.sh, zfs-cli-record.sh | clean |
| shellcheck 0.9.0 `-S warning` on test_agent_record.sh, test_zfs_cli_record.sh | SC2218 (and SC2034 in the second), as in the base; one more SC2218 for the new `scale 3 2` call. CI's newer shellcheck is green on both heads (`lint`). |

The downloaded artifacts were read as data only. Python that read them
ran with `-I`.

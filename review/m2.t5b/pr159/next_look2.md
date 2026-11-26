# Second next look at #159 (M2.T5 follow-ups; the CLI recorder) at 11f02ef

Pull request: skyzfs/skyzfs#159 (`skyzfs-testing` into `zfs-topology`).
**Head: 11f02ef** (checked with `gh api .../pulls/159 --jq .head.sha` at
the start and again before writing this). My last look was at 1a40bfc
(`next_look.md`, comment 7187351598).

What is new since 1a40bfc:

- 2198025: D11, a line for a scenario that ran no kill case, the
  recorder's tests for R13, R14, A07, A08 and A09, and `lim` on
  retain's `zpool list`.
- aad248a: `zpool list -H -o name` records, and `count-snapshots` at
  each scale.
- 16cc099: `holds` at each scale.
- 11f02ef: the merge of `zfs-topology` at 3818056 (#160 and the lead's
  notes). The merge is clean. `tests/` and `.github/` are as at 16cc099,
  and every other path is as at 3818056.

Line numbers are at 11f02ef. The scripts and outputs of this look are in
this directory with the prefix `n2_` (how to run them is at the end).

## Verdict

- **Nothing blocks the merge. It can merge from review.**
- **Every fix claimed in the delta holds.** I checked each against the
  code, the head's runs and plants. 25 plants on the delta: 21 killed
  and 4 survive (gaps G1 and G2).
- **CI on the head is green where it ran.**
  - The agent rig's k is green now. `pin_destroy_first_run` is ok, and
    K-destroy ran 8 kill cases. M2 of my last look is settled by #160,
    merged in.
  - The daemon, probe and replay rigs were cancelled, which is right:
    none of their scripts reads a file this pull request changes.
- **Two things to do before the merge.** Neither is a change of code,
  and neither blocks:
  1. Correct the description's plants paragraph. It undercounts what
     still survives (D-1).
  2. Answer on the pull request the findings of my last look that have
     no answer yet (D-2).

## Findings, most severe first

### D-1. The description's account of the surviving plants is wrong (should fix; description only)

Where: the description, "How it was verified", "Plants" (lines 84 to 87
of the body).

The description says:

- on 1a40bfc, 21 plants survived;
- 2198025 kills five of them (R13, R14, A07 to A09);
- the 16 left are shell plants.

I ran every survivor of my last look again at 11f02ef, one at a time.
This is what still survives:

| set | still survives at 11f02ef | in the description |
|---|---|---|
| shell, agent rig (`n2_old_sh.out`) | 25 of 25: the 16 named, and S03, S05, S15, S16, S17, S18, S22, S23 and T09 | 16 |
| Python, the judge (`n2_old_py.out`) | 17: P43, P46, P48, P52, P61, P64, P65, P67, P73, P74, Q06, Q23, N05, N07, K21, K22 and the probe N05p | none |
| the recorder (`n2_old_rec_a.out`, `n2_old_rec_b.out`) | 8: A04a, C06, C09, C16, C22, C23, C24 and R10a | none |
| new in this delta (`n2_rec1.out`) | 4: L1, L2, P1, P2 (G1, G2) | none |

Notes on the table:

- `n2_old_sh.out` shows U19 and U20 as KILLED. Those two were G4's
  failure under load: run alone, both survive (`n2_u19.out`).
- R13 (C20), R14, A07, A08 and A09 are killed now, as the description
  says (Checks, 1).
- M02 no longer applies, because the code now drops `destroy:after`.
  The description's "D11's test kills M02" means that the test kills
  M02's inverse: my D11r, which adds `destroy:after` back to MOVED, is
  killed.

Certainty: high. Each run was alone in its worktree, with a 600 s limit.

Fix: say which sets were run. Give the survivors by set, or point to the
review's lists.

### D-2. The last look's open findings have no answer on the pull request (should do; process)

CLAUDE.md asks the author to answer every finding: fix it, or say why
not. Since comment 7187351598, the pull request has only Codex's
no-findings summary.

- **Fixed in code but not answered:** D11, G-e (the silent scenario) and
  N2 (`lim`).
- **Neither fixed nor answered:**
  - G-a: a retry that takes an abandoned T again passes. N05p still
    survives.
  - G-b: a double publish after a MOVED kill passes.
  - G-c: a take left behind after a MOVED kill is not looked for.
  - K21 and K22.
  - N1: the K-sweep fixture's shape.
  - N3: the records are named by the K asked, not the K reached.

"To the next rig package, with D7, D9 and D10" is a fine answer for any
of them. It needs to be on the pull request.

### G1. `lim` on retain's `zpool list` is pinned by no test (gap; low)

Where: `tests/kernel/zfs-cli-record.sh:1046` and `:1062`.

- Plants L1 and L2 drop the `lim` from either read, and both survive.
  The fake `zpool` cannot tell the difference.
- The recorder already pins its 32 GiB pool with a grep of its own text
  (`test_zfs_cli_record.sh:305`). The same kind of grep would pin these
  two.
- The risk is low: the scale pool is a fresh file-backed pool.

### G2. The `pools-*` records of `main` are pinned by no test (gap; low)

Where: `zfs-cli-record.sh:1325`, `1327` and `1331`.

- Plant P1 drops `pools-one`, and plant P2 makes `pools-three` name a
  pool. Both survive, because the tests never run `main`.
- `none-pools-named` is pinned (`test_zfs_cli_record.sh:257`), and its
  plant P3 is killed.
- The head's recording shows the three records right (Measurements):
  - `pools-none`: empty stdout, rc 0;
  - `pools-one`: `skzq`;
  - `pools-three`: `skzq`, `skzqm` and `skzqr`.

  They are in the fixture `recorded-ubuntu-26.04.json`.

### G3. The holds read runs with no load beside it (gap; low; for the lead)

- `take-cli-list` runs beside the writer, but `holds` does not
  (`zfs-cli-record.sh:1138-1141`, plain `rec`).
- The figure below is therefore the cost on an idle pool. Keep that in
  mind when `holds_per_snapshot` is set from it.

### G4. Some checks fail under load (observation; low certainty; older than this delta)

I ran three plant series at once on four cores. Some checks then failed
beside plants that do not touch what they check:

- `test_agent_record.sh`, the check "recover: the agent killed, the
  store thawed, the daemon served again" (with U19 and U20);
- `test_zfs_cli_record.sh`, the checks "no lua: count" (with A09) and
  the budget's (with H4 and H5).

Every affected verdict was checked again:

- U19 and U20, run alone, survive.
- A09, run alone, fails only the two checks it should fail.
- H4 and H5 failed their own checks first, so their kills stand.

CI runs each test alone, and it is green. A check bound to wall time
(`SCALE_END` two seconds ahead) can fail on a slow runner.

### Nits

- **N-a.** The `holds` line in `about.txt` gives the lines of holds,
  N+1. The cost per snapshot needs the `count-snapshots` line and the
  record's `secs`, and `secs` is only in the tarball: neither
  `about.txt` nor the job summary carries it. Putting `secs` on the
  `lines_line` of the timed reads would put the figure in the summary.
- **N-b.** The docstring at `agent_check.py:108-111` still says
  `premise_TRACE_points` is "a trace that gave no points". It now also
  covers a rare scenario whose trace adds none. The message itself says
  so plainly.
- **N-c.** `none-pools-named` records `zpool list -H -o name skznone`.
  The adapter does not run that form: #160 lists every pool and looks
  for its line. It is harmless, and the header says what it is.

## Checks

### 1. 2198025

**D11 is fixed** (`agent_check.py:353-363`):

- MOVED no longer has `('destroy', 'after')`, and the comment says why.
- The unit case (`test_agent_check.py:707-711`) pins three things:
  - `moved('destroy:before:1')`;
  - not `moved('destroy:after:1')`;
  - `expected('sweep', 'destroy:after:1', ...) == 'published'`.
- Plant D11r adds the member back. It is killed by
  `test_a_kill_between_the_take_and_taken`.
- My show-case SC5 (`nl_showcases.py`) is a kill at `destroy:after`
  where the settle publishes T itself. It now gives exit 0. In a plain
  case the outcome is left to the criterion (`obs_..._outcome` INFO), as
  for every other kill in plain.
- SC5b (`destroy:before`) still judges `absent` and `moved` ok.
- The merge's engine changes (`engine.rs`, `plan.rs`) touch only
  comments and `NoPool(_)`. The rule MOVED stands on is unchanged.

**A scenario that ran no kill case** (`agent-record.sh:623-630` and
`657-662`):

- `rec_arare` writes a line to `points-error.tsv` when `acheck points`
  succeeds with no output. That is `new_points` with a base, which
  `main_points` lets exit 0 (`agent_check.py:509-529`).
- The check turns each line into `premise_S_points`, ERROR
  (`agent_check.py:629-631`).
- `premise` sets exit 2 unless a FAILED line sets 1. So the line is the
  rig's error, not a verdict. When the cause is the agent, as on
  1a40bfc, the FAILED pin keeps the exit at 1.
- `rec_aplain`'s new branch can be reached only through the test's
  stub. With one argument, `main_points` already exits 2 on an empty
  result. The branch is defensive and harmless.
- Plants:
  - E1: no line for an empty scenario. Killed by "arare: a scenario
    that adds no point is said".
  - E2: no line for an empty plain trace. Killed by "aplain: a trace
    that gives no kill point is said".
  - E3: a line for every scenario with points. Killed by "arare: each
    point the check gives".
  - E4: the line written to another file. Killed.
  - E5: `points-error.tsv` read as INFO. Killed by
    `test_the_rig_s_error_in_the_course_and_in_a_trace`.
- On the head, no job wrote `points-error.tsv`. In k, every rare
  scenario added points:
  - K-assist 18;
  - K-restart 20;
  - K-sweep 36;
  - K-tooold 12;
  - K-destroy 8.

**The recorder's tests** (`test_zfs_cli_record.sh:303-304` and
`791-794`):

| plant | what it breaks | killed by |
|---|---|---|
| R13 (C20) | the count of retained snapshots without `-r` | "the count lists every dataset's snapshots" |
| R14 | `retain` runs at K 0 | "scale 3: none retained, none said" |
| A07 | K read as the last field | `10:5:3` runs as 10:3 |
| A08 | an empty N accepted | `:5` runs |
| A09 | an empty K accepted | `10:` runs as 10 |

- A09 was run again alone. It fails only "N:K entries run" and "an
  empty or a second count, said".
- The `lim` on retain's two reads holds, but no test pins it (G1).

### 2. aad248a and 16cc099: `count-snapshots`, `holds` and `zpool list`

**The commands are the adapter's own:**

- `count-snapshots` is `zfs list -r -H -o name -t snapshot P`, the same
  as `cli.rs:861` at the head.
- `holds` is `zfs holds -H -r P@t`, the same as `cli.rs:668`.
- `pools-*` is `zpool list -H -o name`, `confirm`'s read at
  `cli.rs:564`.

**The order is the agent's**, and the tests pin it:

- `count-snapshots` runs after the dataset count and before the take
  (`test_zfs_cli_record.sh:289`). That is the agent's count, before its
  own take.
- `holds` runs after `hold` and before `release` (`:287`), so it reads
  while the hold is on.
- The budget test now stops before `holds` (`:500-501`).

**Nothing can hang:**

- Every new read goes through `rec`, so it runs under `lim`:
  - the scale's reads within the scale's `LIMIT` (at most 1800 s, and
    never more than the budget left);
  - `pools-*` within `main`'s 120 s.
- `lines_line` reads only a file.
- The scale pool is a fresh file-backed pool. Even a suspended one could
  hold each read only up to its limit.

**The plants:**

- Killed:
  - H1 to H5: `holds` not run, run before `hold`, run after `release`,
    run without `-r`, or with no lines line;
  - S1 to S4: `count-snapshots` without `-r`, after the snapshot, with
    no lines line, or not run;
  - P3: `none-pools-named` with no pool.
- Survived: P1 and P2 (G2).

**What #161 needs.** #161's `holds_per_snapshot` is the cost of `holds
-r` for each snapshot counted, and it is provisional until this
recording gives a figure. The figure needs two values from each scale:

- the `holds` record's `secs`;
- the `count-snapshots` lines, which the agent's count sees: the
  retained snapshots, before the take.

Both are in the artifact, taken the way the agent reads them. Only their
placement is awkward (N-a).

### 3. CI on 11f02ef (read with `gh api`, and the artifacts downloaded and read)

| run | what | result |
|---|---|---|
| 45800708400 | agent rig: a1, a2, d, k | success; each course `checked exit 0` (see the list below) |
| 45800708268 | publish rig, ubuntu-26.04 | success: 9 courses exit 0; 139 ok, 21 INFO, 3 controls caught |
| 45800708401 | cut rig, 4 images | success: 10 courses exit 0 on each image |
| 45800708396 | cut-daemon, 3 jobs | success: 38 courses exit 0; 356 CAUGHT, 256 INFO, 399 PASS, 176 SAME |
| 45800708319 | node B, 26.04 and 26.04-arm | success: 2 courses exit 0 on each |
| 45800708231 | ci: changes, lint, test, unit-arm64, chart-lint, s3-backends | success |
| 45800708235 | coverage | success |
| 45800708387 | e2e | success |
| 45800697045 | skyzfs-ci (push) | gate only: the pull request is open, so `ci.yml` runs the jobs |
| 45800696947 | zfs-cli-record (push) | success: unit |
| 45800940382 | zfs-cli-record by dispatch | 26.04 success; 24.04 still running (does not gate) |
| 45801291679 | zfs-cli-record by dispatch, `1:20000` | queued (does not gate) |
| 45800708338, 45800708377, 45800708326 | daemon rig, replay rig, probe | cancelled. Right: none of their scripts reads `agent-record.sh`, `agent_check.py`, `zfs-cli-record.sh` or `cut-record.sh` |
| 45800697020 | agent rig (push) | cancelled. The pull request's run 45800708400 is the same build |

In the agent rig's run:

- The counts in each job's `results.tsv` match the description's table:

  | job | cases | ok | INFO | controls caught |
  |---|---|---|---|---|
  | a1 | 19 | 149 | 38 | 3 |
  | a2 | 19 | 148 | 38 | 3 |
  | d | 10 | 96 | 15 | 3 |
  | k | 102 | 1030 | 109 | 3 |

- The job times match it too.
- Across the four jobs there is no FAILED, ERROR, MISSED or DIFFERS line.
- In k:
  - `pin_destroy_first_run`: ok. The first run ended 3.
  - K-destroy ran 8 cases: withdraw, look, abandon and unlink, each
    before and after.
  - `obs_K-sweep-intent-before-1_name_reused` is INFO, and that case's
    pins are ok, `stays_absent` against the retry's commit among them.
- `obs_points` still lists `failed` and `destroy` under "none reached"
  in every job. D11's point is still not exercised by any job.

In the lint job's log (139842940081):

- `test_agent_check.py` ran 69 tests, OK.
- The new checks of the two shell tests pass.
- shellcheck passes.

### 4. The description against the head

It is right on these points:

- the commit table and the merge base 3818056;
- the four-job table and its times;
- the cases by scenario, and K-destroy's 8;
- the reuse of a name in k, and that its pins hold;
- why the three rigs were cancelled;
- the new records, and what each is for;
- D11, and the zero-case line.

It is wrong or out of date on these:

- The plants paragraph (D-1).
- "Recordings on 11f02ef ... pending". 45800940382's 26.04 job is done;
  its figures are below.
- It does not mention `lim` on retain's reads. That is only in
  2198025's message.

## Measurements (for the lead): dispatch 45800940382, Ubuntu 26.04, 11f02ef

The snapshot program was not run: the Lua chosen has no `snapshot.lua`.
Every record of the table below ended rc 0.

| entry | snapshots counted | `count-snapshots` | `holds -r`, while held | holds per snapshot | `tagged.lua` | list read |
|---|---|---|---|---|---|---|
| 10-s1000 | 11000 | 2.66 s | 3.93 s | 0.36 ms | 4.04 s | 4.34 s |
| 100-s1000 | 101000 | 25.69 s | 37.27 s | 0.37 ms | 39.04 s | 41.55 s |
| 10-s10000 (stopped at 5500 a dataset, 8.27 GB free) | 60500 | 14.77 s | 21.90 s | 0.36 ms | 22.30 s | 24.17 s |

- **`holds -r` costs about 0.36 to 0.37 ms for each snapshot counted.**
  - The figure is flat from 11000 to 101000 snapshots.
  - #161's provisional `holds_per_snapshot` is 200 ms, about 550 times
    that, before its cap of 35 minutes.
  - The read was on an idle pool (G3).
- **`count-snapshots` costs about 0.24 to 0.25 ms a snapshot.**
  - #160 gives it a read's limit, 60 s and 100 ms for each dataset: at
    101 datasets, 25.7 s against 70.1 s.
  - The margin is narrowest with few datasets and many snapshots. Room
    stopped 1:20000 at 25000 snapshots on 1a40bfc, which is about 6 s by
    this rate against 60.2 s.
- **The holds lines were N+1 at each scale** (11, 101 and 11). Each read
  saw every dataset's `@t` hold.
- **The space figures agree with 1a40bfc's run.** At 10-s10000 the
  tenths read 0.19, 0.53, 4.95, 12.16 and 21.09 GB allocated. The run
  stopped before @r5501 with 8.27 GB free.
- **The 24.04 job, and 45801291679 (`1:20000`), were not done** when I
  wrote this.

## How this was checked

**Local tests**, in a scratch worktree at 11f02ef, with Python run as
`python3 -I`. All pass:

- `test_agent_check.py`: 69 OK.
- `test_agent_record.sh`: all passed.
- `test_zfs_cli_record.sh`: 532 ok, exit 0, 79 s.
- `test_cut_record.sh`: all passed.
- `test_publish_check`, `test_publish_check_main`, `test_cut_check`,
  `test_nodeb_check`, `test_cut_daemon_check` and `test_zfs_cli_json`:
  OK.

**shellcheck.** My shellcheck is 0.9.0, and it reports only info-level
notes on the two changed scripts. Those notes are older than this
delta. CI's newer shellcheck is green.

**Plants**, with `plant2.py`, unchanged:

- Each plant replaces one anchor that occurs once and runs the test
  command with a 600 s limit. Then it restores the file.
- Each series ran in a worktree of its own.
- The specs:
  - `n2_agent.py`: D11r and E1 to E5.
  - `n2_rec1.py`: R13, R14, A07 to A09, L1, L2 and P1 to P3.
  - `n2_rec2.py`: H1 to H5 and S1 to S4.
  - `n2_gen.py` writes `n2_old_py.py` and `n2_old_sh.py`, the survivors
    of my last look.
  - The recorder's survivors not already in `n2_rec1.py` were split by
    hand into `n2_old_rec_a.py` and `n2_old_rec_b.py`.
  - `n2_u19.py` holds U19 and U20 again, alone. Both survived
    (`n2_u19.out`, G4).
- The outputs are the `.out` files of the same names.

**Artifacts read**, with zips downloaded into new empty directories:

- the agent rig's a1, a2, d and k;
- the publish rig;
- node B on both images;
- cut-daemon's three jobs;
- the cut rig's four images;
- the recorder's 26.04 tarball.

**To run** (from the checkout's root at 11f02ef):

- Plants: `python3 review/m2.t5b/pr159/plant2.py . review/m2.t5b/pr159/n2_agent.py`.
  The same command runs each spec.
- Show-cases: `python3 -I review/m2.t5b/pr159/nl_showcases.py .`

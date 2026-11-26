# Review of skyzfs/skyzfs#164: M2.T5 follow-ups, third round

- **Pull request:** skyzfs/skyzfs#164, `skyzfs-testing` into
  `zfs-topology`.
- **Final head:** 22009e987ab555324f3bde245fcd8369a5dda432, as the API
  gave it at the end of the review.
- **Commits:**
  - 59305e8: the sweep's rounds.
  - a138065: #163's lows.
  - 0f25686: merge of `zfs-topology` at fb39232.
  - 432bd0a: merge of `zfs-topology` at 517d055.
  - 22009e9: the stray snapshots renamed `skz-*`.
- **Order of reading.** I read the code at 0f25686 and formed my view
  before I read the description. The last two commits came during the
  review; section 1 covers them.

## Verdict

- **Nothing blocks the merge from review.** I found no defect in the
  code.
- **The findings:**
  - one measurement gap, for the lead's notes before they stand (R1);
  - five low gaps (G-a, G-b, R2, R3, R4);
  - nits, which include R04, the one plant of this round that survives
    where a test should have caught it.
- **22009e9 is right.** Each place that names the two stray snapshots
  was renamed: the rig, both test fixtures and the shell test. No
  other place in the rig, the judge or the agent reads them by a
  pattern (section 1). 432bd0a is a clean merge.
- **CI on 22009e9, when this was written:**
  - green: publish, cut-daemon, node B, e2e, skyzfs-ci;
  - cancelled, rightly: daemon, replay, probe;
  - running: ci, coverage, the cut rig, and the agent rig.
- **The agent rig is the one run that tests the rename on a real
  pool.** Its job k runs the `sweep` and `unmarked` cases, with
  `pin_sweep_swept` and `pin_unmarked_refused`. Its a1/a2/d/k results
  should be read before merging. On 0f25686 all of CI was green (section
  6). Codex's code review of 22009e9 completed with no findings.
- **R1 is for the lead, not a fix before merging.**
  - The idle control is fair as a control. But the figure it is paired
    with, the writer's longest write over the window, cannot see a read
    at these scales.
  - Take all 40 holds windows, and all 20 list windows at 1000, read
    and idle alike. In each, that longest write began 0.8 to 2.0 s
    before the window opened. It was blocked by the sync of the load's
    first group, which had closed 1.3 to 2.0 s before the window
    opened.
  - So most of the medians in about.txt compare the load's start-up
    with itself.
  - The note in 9e70246 rests on them:
    - "the list read adds nothing measurable";
    - "This closes the §5.2 question of the sweep's commands for the
      list";
    - the holds to be repeated "with more rounds".

    More rounds of this design would only measure the first group's
    size again.
  - What the records do support comes from the txg lines. Groups go on
    committing during the list reads. Every sync that overlapped a read
    ran at the idle rate, 213 to 215 MiB/s.

## Findings, most severe first

### R1. The sweep's rounds time the load's first sync, not the read (measurement gap; for the lead's notes; high certainty)

Where:

- `zfs-cli-record.sh:1018-1050`: `writing`, where t0 is the logs'
  start plus `ZCR_WRITER_MARGIN`, 2 s;
- read by `sweep_rounds` (`:1198-1243`) through `stall_of` (`:1179`).

How I measured, from run 45853804429's artifacts (both images):

- the window: each record's `window`;
- the longest write: from `writer.tsv`;
- the group: from `txg.tsv`. A group closed at its commit time minus
  its sync, wait and quiesce times.

**The start-up stall.**

- `writing` starts the O_DSYNC writer, the writer of new data and the
  probe. It waits for their logs, then 2 s, then opens the window.
- The writer of new data fills the first group at memory speed: 563
  to 574 MiB at scale 1000, and 495 to 1080 MiB at 10-s1000.
- That group closes 0.1 to 0.8 s after the start, and syncs for 2.6 s
  at 1000, or 2.3 to 5.0 s at 10-s1000.
- The O_DSYNC writer's write that began in that phase is blocked until
  the sync ends, 0.35 to 3.3 s after t0.

**Holds, 40 of 40 windows** (both images, both scales, read and idle):

- The window is 0.12 to 0.48 s long.
- The longest write began 0.8 to 2.0 s before t0.
- Its group closed 1.3 to 2.0 s before t0.
- No group committed in the window (`txgs=0`).
- The read cannot have changed that write's length except by slowing
  the sync, and the sync ran at 214 to 215 MiB/s on every side.

**List at 1000, 20 of 20 windows: the same.**

- The longest write began 1.7 to 1.9 s before t0.
- Its group, of 563 to 574 MiB, closed 1.7 to 1.9 s before t0.
- Groups go on committing in every list window, at both scales: two,
  or three in one window of each ten.

**List at 10-s1000, 15 of 20 windows** (read 4 and 4 of 5, idle 4 and
3 of 5):

- The longest write is the sync of a later group, of about 1.1 GiB.
- It began 3.7 to 4.0 s after t0, inside a 4.4 to 5.2 s window, and
  lasted 4.7 to 5.5 s.
- The other 5 windows have the start-up stall again.
- Read and idle alike: 213 to 215 MiB/s.

**The holds' "0.5 s above idle" at 10-s1000 is the first group's
size**, fixed before the read began:

| image | side | first group (MiB) | closed before t0 (s) | sync rate (MiB/s) |
|---|---|---|---|---|
| 26.04 | read | 905 to 1042 | 1.79 to 1.83 | 214 to 215 |
| 26.04 | idle | 495 to 967 | 1.29 to 1.95 | 215 |
| 24.04 | read | 823 to 1080 | 1.75 to 1.88 | 215 |
| 24.04 | idle | 870 to 1067 | 1.36 to 1.85 | 215 |

The longest write is the group's size over 215 MiB/s. The 26.04 idle
round of 2.26 s is the one with a 495 MiB group.

What this means:

- **The stall medians in about.txt cannot show a read effect smaller
  than the load's own first sync:** 2.6 s at 1000, and 2.3 to 5.5 s at
  10-s1000. They say nothing either way about the holds read.
- **The txg lines are evidence of another kind.**
  - Every group that synced across a read window did so at the idle
    rate.
  - At 1000, the holds read's window (0.43 to 0.48 s) lay inside a
    2.6 s sync. That sync was within about 10 ms of its length with no
    read:
    - on 26.04, 2.614 to 2.625 s against 2.613 to 2.614 s;
    - on 24.04, 2.65 to 2.67 s against 2.65 to 2.66 s.
  - The list reads did not stop groups from committing.
  - This supports "the sweep's reads do not hold or slow the pool's
    sync at these scales". I would write that conclusion from these
    lines, with that limit, rather than from the stall medians.
- **Older than this pull request.** Most short records made by
  `writing` have the same start-up stall in their `stall` line:
  `holds`, `take-cli-get`, `take-cli-members`, `sweep-holds` and others.

**Fix (follow-up, the author's choice):**

- Open the window only once the load is past its start:
  - after the probe has seen the load's first group commit, that is a
    line in `txg.tsv` with `written > 0` after `start`;
  - or with a margin longer than one sync of the dirty data limit at
    the disk's rate.
- For a read shorter than a sync cycle, run it back to back over a
  fixed window of several cycles (10 s, say), with the idle over the
  same window.
- Judge each window by the groups committed in it and their sync rate,
  next to the longest write.

### G-a. `agent_quiet`'s dhook exemption is wider than `recover`'s (gap; low; high certainty that the path exists)

Where: `agent-record.sh:686`
(`if [ -n "$CDIR" ] && [ "$SCEN" = dhook ]`).

- **The rule.** In a dhook case, an ended daemon seen after a rig error
  is only logged, whichever instance it was.
- **Failing scenario:**
  1. In a dhook case, the hooked daemon ends by its hook.
  2. `scen_dhook` writes `kill.tsv` and calls `serve_again`
     (`:569-574`), which starts an unhooked instance.
  3. `recover` sees that instance alive.
  4. The instance then dies by itself.
  5. A step of `asettle` fails as the rig's error: a `zfs list`,
     `holds` or `get`, or node B's `snaprow`. With no server behind the
     pool, such a failure is likely. `acase` still has `CDIR` set and
     `SCEN=dhook` there (`:419-430`).
  6. `agent_quiet` logs the end and gives no verdict.
- **Result.** The recording ends with exit 2 (the rig's error) where
  `daemon_serving` and exit 1 were due. It is never a pass.
- **Why copying `recover`'s rule would be wrong here.**
  - `recover` exempts only when `rig-error.txt` (the case's error) is
    not empty.
  - In a carry-* case, the hook fires and then the thaw fails as the
    rig's error, with no `rig-error.txt`.
  - `recover`'s rule would then give a false `daemon_serving` for a
    hooked daemon that ended as asked. The author's choice is right on
    that path.
- **Fix:**
  - Exempt only while the hooked instance is the one serving:
    `[ -n "$CDIR" ] && [ "$SCEN" = dhook ] && [ ! -e "$CDIR/kill.tsv" ]`.
  - Test it with `quiet_t` in a dhook case with `kill.tsv` present,
    which should give `daemon_serving`.

### G-b. #163's G2 is half done, and the description does not say so (gap; low; description)

Where: `cut-record.sh:599` (`end_daemon`) and `agent-record.sh:265`
(`reserve`).

- **Done.** The hazard in G2's last point is gone: `z` runs under
  `lim` (`cut-record.sh:258-260`).
- **Not done.** G2's own point is still open: a daemon that had already
  ended 0 by itself reads as a clean stop, through
  `kill -TERM "$SERVE_PID" 2>/dev/null || true`.
- **The description** says "G2. `z` ... runs under `lim`" and "G3 and
  G4 ... left as they are", and nothing of G2's main point.
- **Ask:** either #163's one-line fix, or a line under "Not done" that
  says why not. The window is small, as #163's review said.

### R2. The idle control is the step's first read time, not each round's (gap; low)

Where:

- `zfs-cli-record.sh:1218`: `secs` comes from
  `scale-LBL-sweep-KIND/secs`;
- `:1232` and `:1236`: `idle_for "$secs"`.

The rounds' reads took longer than the step's read they are paired
against:

| image | scale | read | read windows (s) | idle windows (s) |
|---|---|---|---|---|
| 26.04 | 1000 | holds | 0.448 to 0.452 | 0.340 |
| 24.04 | 1000 | list | 1.356 to 1.381 | 1.259 to 1.260 |
| 24.04 | 10-s1000 | list | 4.900 to 5.156 | 4.897 to 4.900 |

- `lim` polls every 0.1 s, so `secs` itself is coarse.
- A longer window has more chance to overlap a long write, so the bias
  is toward the read. Under R1 it does not matter.
- **Fixes:**
  - Pair each idle with its own round's read time (read first, then
    the idle as long as it took).
  - Or put each round's read seconds in the about line.

### R3. The step's status is not read, and the idle is not bounded by the budget (gap; low)

Where: `zfs-cli-record.sh:1218-1222` and `:1225-1236`.

- **A cut read still gets rounds.**
  - Rounds run on any `secs` that is a number, even when the step's
    read was cut at `LIMIT` (rc 124, `secs` equal to `LIMIT`).
  - Each round's read is then cut again, and its stall counts in the
    median.
  - The about line does not say so.
- **The idle can overrun the budget.**
  - `scale_limit` checks the budget before each round.
  - But `idle_for "$secs"` sleeps the full `secs` whatever is left.
- **Fixes:** skip rounds when the step's `rc` is not 0. Skip a round
  whose idle does not fit in what is left.

### R4. Five rounds by default is dear at the largest counts (observation; low)

- **The cost.** At 100-s1000 on 24.04, the sweep's list took about
  186 s (run 45800940382).
  - Five rounds of read plus idle, each with its margins, stops and
    settle, come to about 33 minutes of the 4800 s budget.
  - The next counts then read "the budget was spent".
- **Ask.** This is not wrong. The input's description could say so,
  or the default could depend on the step's time.

### Nits

- **The comment on the alternation** (`zfs-cli-record.sh:1195-1196`).
  - It says the alternation puts "the pool's own sync cadence ... on
    both alike".
  - But each `writing` starts its own load and settles it with
    `zpool sync`, so the pair shares no cadence.
  - The records show no order effect. On 26.04 at 10-s1000, the holds
    idles were:
    - 3.10, 3.84 and 4.50 s with the idle first;
    - 3.81 and 2.26 s with it second.
  - The alternation does no harm.
- **Code beside the rounds:**
  - `# scale N [K]: ...` (`:1170-1174`) now sits above `idle_for`,
    not `scale()`.
  - "N rounds" counts windows whose stall reads `?` (a `NOT COVERED`
    line).
  - With 5 rounds, 3 have the idle first and 2 the read first.
- **A leading zero in `ZCR_SWEEP_ROUNDS`.**
  - `ZCR_SWEEP_ROUNDS=08` passes the script's check (`:1200-1201`).
  - Then `i <= rounds` fails ("value too great for base"), so no round
    runs and the line says "0 rounds".
  - Only when the script is run by hand: the workflow rejects a
    leading zero.
- **R04 survives: the rounds' tests cannot tell the read's records
  from the control's.**
  - The test computes its expected line from the same four records.
  - On this machine all four stalls are equal, so taking the reads'
    stalls from the controls' records passes.
  - A test whose stub writer gives the read's window a long write
    would pin it.
- **quiet_hang cannot see the bound values**
  (`test_agent_record.sh:888-905`).
  - The `seq` stub hides the numbers in `$(seq 1 20)` and
    `$(seq 1 120)`, so a bound of a million tries passes (N17 and N17b
    survive).
  - It is fair for what #163 asked, no wait without a bound: an
    unconditional `wait` or a `while kill -0` loop fails it.
- **Older than this pull request.** In the `CUT_S3_UP` one-liners
  (`skyzfs-agent-rig.yml:228`, `skyzfs-cut-daemon.yml:224`,
  `skyzfs-publish-rig.yml:190`), a failed `docker start` falls through
  the `&&` chain to "did not answer 60 s after docker start".
- **A race in the message only.**
  - In `reserve`, if `serve_end 10` fails after the SIGKILL and the
    daemon dies just afterwards, `agent_quiet` says "with no kill of
    the rig's".
  - `daemon_stopped` was given already, so the verdict is right.
- **Cleanup does not detach a dead daemon's device.** With
  `SERVE_PID` now cleared in `agent_quiet`, `cleanup` skips the detach
  for a device whose daemon died. Harmless on a throwaway runner.
- **A stale comment, new at 22009e9.** `agent-record.sh:100` still
  says the sweep's stray snapshot is the rig's "(sweep-ID)"; it is now
  `skz-sweep-ID`.

## Checks

### 1. The two commits after 0f25686

- **432bd0a is a clean merge.**
  - Its tree is the one `git merge-tree --write-tree 0f25686 517d055`
    gives (b6f4aab).
  - Against 517d055 it differs by the same 10 files, +318 -24, as
    0f25686 did against its base.
  - What it brings from `zfs-topology` is #165 and the notes. No file
    of this pull request's diff is touched.
- **22009e9 renames five commands in the rig.**
  - `scen_sweep`: `zfs snapshot`, `hold`, `holds -H` of
    `@skz-sweep-ID`.
  - `scen_unmarked`: `snapshot`, `hold`, `holds -H`, `release` of
    `@skz-stray-ID`.
  - Two fixtures in `test_agent_check.py` (`STRAY`, and the sweep's
    changed `stray-holds.txt`) and the one expected line in
    `test_agent_record.sh`.
- **Nothing else names those snapshots or lists them by pattern.** I
  grepped `agent-record.sh`, `cut-record.sh`, `agent_check.py`, node B's
  check, every `tests/kernel` script and test, and the agent's crate.
  - The judge reads only `stray-holds.txt`, for the hold tag, and finds
    snapshots by exact name (`f'{POOL}@{t}'` in
    `snapshots-after.txt`, `agent_check.py:781`).
  - The agent calls a snapshot its own by the exact name in its record
    (`engine.rs:1155-1163`). Its names have the form
    `skz-YYYYMMDDTHHMMSSZ[-N]` (`names.rs`), which `skz-sweep-ID` and
    `skz-stray-ID` cannot take.
  - The sweep on `zfs-topology` has no name filter yet, so the commit
    message is right that the change holds on both sides of M2.6c-5f.
- **Tests at 22009e9:** `test_agent_record.sh` 132 ok, all passed;
  `test_agent_check.py` 80 OK.

### 2. The sweep's rounds (59305e8)

- **The idle control measures what it says.** It has the same load,
  margins, settle, stop and pool, and nothing but `sleep` in its
  window. Its length is the step's (R2). It is a fair control. Its
  limit is R1's.
- **Order.** Each record starts and settles its own load, so the order
  inside a pair can only matter through what the first leaves, and the
  `zpool sync` settle clears that. No order effect shows (nits).
- **Timing.** The window is stamped with `date +%s.%N`, and the writer
  and probe with `time.time()`: the same clock.
- **Bounds and suspended pools.**
  - Each read runs through `rec` under `lim "$LIMIT"`.
  - The writers and probe stop through `stopped`: 60 s, then SIGKILL
    without a wait.
  - The settle is `lim 120 zpool sync`, and `idle_for` is a `sleep`.
  - I found no unbounded wait, and no command substitution around a
    pool command.
  - A spent budget is said once, then `break`.
- **Against #162's limits** (`cli.rs:444-452`: a read gets 60 s, plus
  10 ms a snapshot for the list and for `holds -r`).
  - The rounds' list reads took 4.41 to 5.16 s for 11011 lines at
    10-s1000. That is 0.40 to 0.47 ms a line, a twentieth of
    `list_per_snapshot`, before the 60 s.
  - The holds batches of 1000 names took 0.12 to 0.48 s.
  - These figures come only from the records: about.txt has neither
    (R2).
- **`zfs_cli_json.py`** skips `scale-*`, so its output does not change.
- **The workflow's `sweep_rounds`** is checked (`*[!0-9]*|0?*|???*`,
  at most 20); empty means unset (5); and it is passed with
  `--preserve-env`.

### 3. #163's lows (a138065)

- **G5 (`agent-record.sh:677-691`).**
  - After `session_end`, `agent_quiet` finds an ended `SERVE_PID`,
    reads it with `serve_end 5` (its status, or `unknown`), and clears
    it.
  - It gives the verdict through `agent_daemon_wrong`, in the case's
    directory or `cases/course`.
  - Every kill of the rig's clears `SERVE_PID`: `akill_serve`,
    `scen_both`, `serve_end` in `reserve` and `end_daemon`, and
    `serve_start`'s failure path. So no kill of the rig's becomes this
    verdict.
  - It can neither hide nor fake a verdict, except G-a (hides one, as
    exit 2) and the message race in the nits.
  - It never turns a rig error into a verdict without cause. Only a pid
    that the rig had not cleared, and that no longer runs, is judged,
    and the export after is skipped, as B1's fix wants.
- **G2 (`cut-record.sh:258-260`).**
  - It gives the same status (124) and puts the output where it did.
    At the limit it sends SIGKILL instead of SIGTERM, and it does not
    wait for a child it cannot kill.
  - The test pins the text. G-b covers what is left of G2.
- **The thaw's message**, in all three workflows, is bounded by
  `SECONDS`: at most 60 s, plus a 2 s `curl` and a 0.5 s sleep.
  - On 0f25686, job k thawed 128 times through it and none failed.
  - The publish rig's `s3-up.log` is clean.
- **D-1.** `akill_serve`'s comment (`agent-record.sh:300-304`) is now
  accurate.
  - The kill fails on a reaped child, but also for other reasons; the
    status from `wait` (137) is what says it.
  - An older edge: an OOM SIGKILL of the daemon also reads 137, as the
    rig's.
- **G1's tests** each pin their path, and each of the nine plants is
  killed by the new test, not by a neighbour (section 5):
  - X06b: the `none` and `empty` loop;
  - X04 and X04b: `late_t 1 1`;
  - Y09 and Y10: `ed_t`, with a stand-in that ignores TERM and
    `serve_end` cut to 1 s;
  - Z08: `test_a_verdict_of_the_course_s_end`;
  - X25 and X26: `quiet_hang`;
  - X12: `asettle`'s settle two/fail.
- **quiet_hang is a fair pin of X25 and X26.**
  - The `kill` stub keeps the two `sleep 20` children alive through
    SIGKILL. With them alive, an unconditional `wait` blocks 20 s, and
    the test fails ("waited 20 s").
  - It passes for the right reason, the `kill -0 ... || wait` guard,
    and not because the children ended.
  - Its limit is the bounds' values (nits).

### 4. The description against the head

- **Accurate:**
  - the commits;
  - 132 checks and 80 tests (I reran both);
  - the rounds' table: I recomputed each median from about.txt of run
    45853804429;
  - the input;
  - G5, D-1, the thaw and the G1 list.
- **Not accurate:**
  - **"The merge of #162 brings `crates/skyzfs-agent/` changes, so the
    rigs that start on those run too."**
    - A `pull_request` path filter reads the pull request's own diff
      against the merge base. After the merge, that is these 10 files,
      and none is under `crates/`.
    - The rigs ran for `tests/kernel/**`.
  - **"as job k did in #163".** Job k hung in `cleanup`'s
    `timeout 120 zpool export -f`, behind `agent_quiet` (#163's B1), not
    in `z`. `z` could have hung the same way, so the fix stands.
  - **G2:** see G-b.
  - **"five rounds do not settle it"** (under "Not done"): see R1. More
    rounds of this design would not settle it either.
  - **The commit list** does not have 432bd0a and 22009e9 yet.

### 5. Plants

I applied each plant alone to a detached worktree at 0f25686, with a
600 s limit, one at a time, restoring the tree after each. The specs and
outputs are beside this file.

| set | spec | plants | killed | survivors |
|---|---|---|---|---|
| #163's courses and hooks, rerun | `p3_sh.py` | 71 | 63 | U05r, U05q, X10, X11, X12b, Y02, Y13 |
| #163's judge and node B, rerun | `p3_A.py` | 26 | 25 | Z04 |
| this round's code | `p5_new.py` | 26 | 23 | N17, N17b, R04 |

- **Not made:** X24 of `p3_sh.py`, whose anchor this round's block
  moved (the anchor occurs 0 times). I remade it as X24h in
  `p5_new.py`, and it is killed.
- **The G1 nine** (X04, X04b, X06b, X12, X25, X26, Y09, Y10 in
  `p3_sh.py`, and Z08 in `p3_A.py`) are all killed, each by the new
  test written for it. X21, a survivor at #163, is now killed as well:
  without the stop file, the 60 s wait lets `quiet_t`'s serving stand-in
  end, and G5 makes that a verdict.
- **The survivors are #163's, less those nine and X21.** Each was
  explained in #163's review:
  - U05r and U05q are equivalent: SIGKILL ends a stopped process.
  - X10 and X11 are constants.
  - X12b and Y02 were out of G1.
  - Y13 is a message's log.
- **This round's survivors:**
  - N17 and N17b, bounds of a million tries, are hidden by the `seq`
    stub (nits).
  - R04, the reads' stalls taken from the controls' records, is a
    gap of the rounds' test (nits).
- **Killed in `p5_new.py`, each by the test meant for it:**
  - on `agent_quiet`: the block removed; exempt in every case; dhook
    exempt outside a case; the status not read; a live daemon judged;
    `verdict` instead of `agent_daemon_wrong`; another pin;
    `recover`'s `rig-error.txt` rule; no log tail; the verdict moved
    before `session_end`; `serve_end 0`;
  - the two waits as `while` loops (N18, N18b);
  - `z` under `timeout` again, `|| true`, no limit;
  - an empty `rig-error.txt` exempt in `recover` (N23);
  - late not served again, late's agent left stopped (N24, N25);
  - `end_daemon` sending SIGTERM twice (N26);
  - an idle of no length (R01) and the medians swapped (R05).
- **How each kill was read.**
  - On this machine, the suite failed one block of 12 to 18 checks
    in some runs, with nothing planted. The block starts at `recover`'s
    loops and ends at `reserve`'s hung stop.
  - So I read every kill from its own FAIL lines (`plant4.py` prints
    all of them), and reran each plant whose only failures were in that
    block: U11, X10, X12b, Y13, S03 and U12. U11, S03 and U12 are then
    killed by their own test. X10, X12b and Y13 survive, as at #163.
  - The block is described under "How this was checked". It does not
    bear on this pull request.

### 6. CI

**On 0f25686, every run ended.** Its runs were all green except the
cancelled rigs:

- ci 45858301437 (lint, test, unit-arm64), coverage 45858301316, e2e
  45858301665.
- publish 45858301360: 9 recordings, exit 0.
- cut-daemon 45858301389: 26.04, 26.04-arm and 26.04-upload, the eight
  upload courses at exit 0.
- node B 45858301581: both images.
- cut rig 45858301400: four images.
- agent rig 45858301513:

  | job | pins ok | controls caught | FAILED | exit | thaws |
  |---|---|---|---|---|---|
  | a1 | 7 | 3 | none | 0 | |
  | a2 | 7 | 3 | none | 0 | |
  | d | 19 | 3 | none | 0 | |
  | k | 49 | 3 | none | 0 | 128, through the new wait |

  Job k recorded 104 case directories.

**Cancelled: daemon rig 45858301454, replay rig 45858301409, kernel
probe 45858301403.**

- Each triggered on `tests/kernel/**`.
- They run `daemon-rig.sh` and its sourced helpers, `skyzfsd-ref.sh`,
  `replay-rig.sh`, `replay_*`, the hole rig, `ublk-probe.sh` and
  `machine-facts.sh`. None of these is among the pull request's
  10 files, and the daemon's code is not in the diff.
- So cancelling them loses nothing, and CLAUDE.md's rules for when they
  must run do not apply. The same holds on 22009e9 (45872071602,
  45872071755, 45872071580).

**On 22009e9, at the time of writing:**

- **Green:** publish 45872071797, cut-daemon 45872071588, node B
  45872071582, e2e 45872071784, skyzfs-ci push 45872065093.
- **Running:** ci 45872071735, coverage 45872071739, cut rig
  45872071847, agent rig 45872071834.
- **The pins to read in job k:** `pin_sweep_swept`,
  `pin_unmarked_refused` and the `K-sweep-*` cases. They are the only
  ones the rename can move. On 0f25686 they were ok under the old
  names.
- **Codex:** no findings on 0f25686, and its code review of 22009e9
  completed with none.

## How this was checked

- **Worktrees.** I worked in detached worktrees at 0f25686: one for
  reading and the suites, one for plants. The first then moved to
  22009e9.
- **Suites at 0f25686:**
  - `test_agent_record.sh`: 132 ok, all passed.
  - `test_cut_record.sh`: all passed.
  - `test_zfs_cli_record.sh`: 648 ok, all passed, in 169 s.
  - `tests/lib/test_lim.sh`: all passed.
  - With `python3 -I`:
    - `test_agent_check.py`: 80 OK;
    - `test_dsync_writer.py`: 15;
    - `test_txg_probe.py`: 20;
    - `test_zfs_cli_json.py`: 7;
    - `test_cut_daemon_check.py`: 104;
    - `test_nodeb_check.py`: 29.
- **Suites at 22009e9:** the agent's two, as in section 1.
- **shellcheck** here is 0.9.0, older than CI's, and found nothing new.
- **The plants' tools.**
  - The plant runners are `plant3.py` (#163's) and `plant4.py` (the
    same, printing every FAIL line).
  - `p0_null.py` runs the suite through the runner with nothing
    planted.
- **The suite's failing block on this machine.**
  - The machine was shared with other sessions' builds, at load 3
    to 11.
  - Through the runner with nothing planted, `test_agent_record.sh`
    failed the block described in section 5 in 1 run of 3. Run
    directly, it failed in 1 of 4 and passed in 3 of 3 more.
  - In the failing direct run, a watch of `/tmp` showed the suite's own
    `mktemp -d` directory vanish 3 s after the start, and again 19 s
    in. Only that one of the 2437 `/tmp/tmp.*` entries vanished, and
    each time well before the suite's EXIT trap. A test recreated the
    directory in between with `mkdir -p`.
  - A subshell does not run the EXIT trap (checked). I did not find
    what removes it, in the suite or outside it.
  - The CI test job passed, so I note it only so that the method can be
    judged.
- **Records.** I downloaded the artifacts of run 45853804429 (both
  images) and of the agent rig on 0f25686. R1's figures come from each
  record's `window`, `writer.tsv` and `txg.tsv`.
- **GitHub.** I used `gh api` reads only, and posted nothing.

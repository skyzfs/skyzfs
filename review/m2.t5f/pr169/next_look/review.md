# Review of skyzfs/skyzfs#169: the next look, at af0c183

- **Pull request:** skyzfs/skyzfs#169, `skyzfs-testing` into
  `zfs-topology`, at base 2751e3b.
- **Head:** af0c183f2041717c108def88a60b757d7e19262f, as the API gave
  it at the start of this look and at the end.
- **The delta:** one commit on 54b666b, the head of the last review
  (`../review.md`, posted as comment 7202717469):
  - af0c183, "a round's read lasts its whole window; review lows".
  - Five files: `zfs-cli-record.sh`, `agent-record.sh`, `cut-record.sh`
    and the tests of the three.
- **The base moved** to 3fa5a8e (#166 and notes). It touches none of
  the files of this pull request.
- **Order of reading.** I read the delta, ran the demonstrations and
  the plants, and read the dispatch's records before I read the
  author's answer (comment 7203342998) and their figures (comment
  7204030861). Section 7 checks those two comments against what I found.

## Verdict

- **It can merge.** No defect blocks it.
  - **D1 is fixed where it was.** `reading_for` now waits out the rest
    of its window after its last run. The demonstration of the last
    review gives 10.02 s for both runs of 4.9 s and of 5.1 s in a
    window of 10 s. Before the fix it gave 9.82 and 5.11 s.
  - **Q3's fix is right on every path** (section 3).
  - **Every low is answered.** Each has a test, and all but two of the
    tests fail when the change is planted out (section 4). The two
    that survive are test gaps (section 5).
- **One finding on D1's subject, not blocking: D4** (section 2).
  - On the runners, every read window was still longer than its idle
    control, in 40 of 40 rounds:
    - 24.04: by 0.03 to 1.21 s, median 0.45 s;
    - 26.04: by 0.12 to 1.22 s, median 0.68 s.
  - The cause is not a run that went past the window, as the author's
    figures say. It is `rec` and `lim` around `reading_for`. The idle
    control runs without them. The records show it: `rec`'s own time
    equals the window to 0.01 s, and in the rounds with one run, no
    second run was started.
  - The about line states both windows truly. But its "groups
    committed" are counts, so their medians lean to the read side by up
    to about one group. That would hide a hold of that size.
  - The groups per second in the author's figures are right, and they
    are what the notes should quote.
- **The new dispatch** (run 45895208967, section 6):
  - Over each window's own length, no read changed the group rate by
    more than the rounds' own spread.
  - The earlier difference is not seen here: the list read at
    10-s1000 on 26.04, a ratio of 0.78. But this runner committed
    groups about twice as fast as the earlier one, so this run cannot
    confirm or refute that difference.
  - D2 is unchanged.
- **CI on af0c183 is green** for every run that drives the changed
  files.
  - The daemon rig, the replay rig and the kernel probe were cancelled
    by hand two minutes in. None of them drives a changed file
    (section 8).
  - Codex has no finding on af0c183.

## Decisions for the lead

1. **Counts or rates in the about line (D4).**
   - The line gives each round's groups committed, and their medians,
     over windows that are not equal. The read window is 0.5 to 0.7 s
     longer at the median and up to 1.2 s longer.
   - At 1000 datasets the load commits about 1.9 groups a second, so
     the difference is about one group in 19. Example, 26.04 at 1000,
     holds:
     - counts: read 21 against idle 19 (+10%);
     - per second: 1.974 against 1.898 (+4%).
   - Two ways to fix it:
     - give groups per second per round, and their median ratio. This
       is cheap, and it holds whatever the windows are;
     - or run the idle control through `rec` as well. That narrows the
       difference but does not remove it.
   - Either way, the notes on §5.2 should quote rates, as the author's
     comment does. Whether the fix goes into this pull request or a
     later one is your call.
2. **D2 (still yours).** The new figures do not change it.
   - Some windows still opened before the load's first group of 300
     MiB or more had committed:
     - 24.04: 7 of 56 (5 of them round windows), all at 10-s1000;
     - 26.04: 1 of 56;
     - the earlier run: 12 and 2.
   - The settle's comment now says so (`zfs-cli-record.sh:1078-1089`),
     and it is accurate against both runs.
3. **The earlier list difference at 10-s1000 on 26.04.**
   - Ratio 0.78 then, lower in 5 of 5 rounds. Now 1.06, lower in 2 of
     5.
   - This run's runner committed groups at 0.82 a second when idle,
     against 0.43 then. Its list read took 15.6 to 15.8 s a run (16.4 s
     in the step), against 11.8 s then. It is a different machine.
   - To settle it: more rounds on one runner, and the same runner
     twice if it can be had. One run per image cannot do it.
4. **The three cancelled runs on the pull request** (the daemon rig,
   the replay rig and the probe).
   - CLAUDE.md asks for CI green on the head. These three are not
     green, but none drives a file that this pull request changes
     (section 8).
   - Whether to rerun them before the merge is your call.

## 1. D1 as fixed

**The code.** `reading_for` (`zfs-cli-record.sh:1297-1314`):

- It takes its own t0, and starts runs while the last run's time
  predicts an end within SECS.
- Then it sleeps for what is left of SECS, by the wall clock.
- `idle_for` is `sleep SECS` (`:1286`).

**The demonstration of the last review** (`../d1_demo.sh`), run again
at af0c183 (`d1_demo_af0c183.out`). Both cases now last the window:

| case | 54b666b | af0c183 |
|---|---|---|
| runs of 4.9 s, window 10 s | runs=2, 9.821 s | runs=2, 10.020 s |
| runs of 5.1 s, window 10 s | runs=1, 5.111 s | runs=1, 10.024 s |

**Each side's window as `writing` measures it** (`d1_windows.sh`,
`d1_windows.out`):

- It uses writing's t0 and t1 around the command, as the window file
  has them.
- The reads go through `rec` and `lim`, as the rounds run them.

| case | window (s) | reads' line |
|---|---|---|
| `idle_for 10` | 10.01 | |
| runs of 5.1 s | 10.10 | runs=1 failed=0 |
| runs of 4.9 s | 10.16 | runs=2 failed=0 |
| runs of 0.02 s | 10.12 | runs=249 |
| runs that fail at once | 10.05 | runs=975 failed=975 |
| a run of 4 s, then one of 7 s | 11.21 | runs=2 |
| one run of 12 s | 12.13 | runs=1 |
| rec's limit of 3 s (forced) | 3.85 | rc=124 |

**Where the sides can still differ**, path by path:

- **A run that overshoots its prediction, or a run longer than the
  window.** The read side is longer: 11.21 and 12.13 s above. The about
  line shows it.
- **The settle.** It ends before the margin and before t0, on both
  sides, so it is outside both windows. Its length differs from window
  to window, but the windows do not include it.
- **Reads that fail.** They loop for the whole window, and the about
  line now gives `failed` per round.
- **A step read that ended with a status other than 0 or 124.** It gets
  no rounds, and the about line says so (`:1390-1397`). The plant P07
  is killed.
- **A round's read cut by rec's limit.** `sweep_rounds` refuses a limit
  under 2W (`:1419`), so this needs a run that hangs past 2W. Then the
  round still enters the medians:
  - its `runs` and `failed` read `?`;
  - its window is at least 2W, and the line shows that.

  A nit.
- **A clock step.** `reading_for` measures its wait by the wall clock,
  `idle_for` by `sleep`. A step of the wall clock during a window
  changes the read side by the step. The window file and the probe's
  mapping are on the wall clock too. Rare on a runner; a nit.
- **SIGTERM.** Nothing sends one to the reads. A cancelled job ends
  everything.
- **rec and lim themselves.** They are on the read side only. On a
  quiet machine they add 0.04 to 0.15 s (above). On the runners they
  added up to 1.2 s: D4, section 2.

**Does the about line state the true windows?** Yes.

- `window_of` reads writing's t0 and t1. The probe counts groups
  between the same two times.
- What the line does not state is how much of the read window had a
  read in it. After the last run, `reading_for` waits with no read.
  The share of the window with a read in the dispatch:
  - list rounds: 0.75 to 0.92 for one run a window, and at least 0.60
    to 0.80 for two to four;
  - holds rounds: at least 0.88.

  An effect of the list read is diluted by up to 40% before it reaches
  a per-window figure. A `busy=` (the sum of the runs) on
  `reading_for`'s line would let the line say so. Low.

**The tests.**

- The `reading_for` unit checks pin the wait (`test_zfs_cli_record.sh:1013`,
  `:1019`).
  - P01 removes the wait. It is killed there.
  - P02 waits half a second too long. It is killed there, and also in
    the scale-3 window check.
- The scale-3 window check (`:1049`) does not catch P01: the fake reads
  are fast and fill the window anyway.
- `rounds_t` stubs `writing` and writes the window files itself, so it
  pins how the line reads the windows (P03 and P04 are killed). It does
  not pin that they are equal.
- So the author's sentence "`rounds_t` and the real-pool rounds now
  require the read window to be at least the idle one" is not true:
  - nothing checks a real pool's windows;
  - the about line states them, and that is all.

## 2. D4: on the runners the read windows are still longer (low to medium, not blocking)

- **Where:** `zfs-cli-record.sh:1426-1430`. The read side is
  `writing "$r" ... rec "$r" reading_for "$w" ...`. The idle side is
  `writing "$c" ... idle_for "$w"`, with no `rec`.
- **The figures, run 45895208967.** The idle windows were 10.003 to
  10.008 s, and 17.008 s for 26.04's list at 10-s1000. Read minus idle
  in the same round:

  | image | rounds | least | median | most | more than 0.3 s |
  |---|---|---|---|---|---|
  | 24.04 | 20 | 0.03 | 0.45 | 1.21 | 11 |
  | 26.04 | 20 | 0.12 | 0.68 | 1.22 | 16 |

- **The time is in `rec` and `lim`, not in the runs:**
  - `rec`'s own `secs` matches the window to 0.01 s. Examples on
    24.04:
    - list at 10-s1000, r2: one run of 8.400 s, `secs` 11.139, window
      11.149. A second run (8.4 + 8.4 > 10) was not started, and
      `reading_for` waited the rest to 10 s.
    - holds at 10-s1000, r1: 514 runs, the longest 0.060 s, `secs`
      11.205, window 11.212.
  - In the holds rounds at 10-s1000 the longest run was at most 0.13
    s, so the last run's overshoot cannot exceed that. Their windows
    were still 0.06 to 1.22 s longer than the idle ones.
  - So in these two rounds about 1.1 to 1.2 s lies outside
    `reading_for`'s own span of 10 s but inside `rec`'s. What is there
    is process starts and file operations:
    - `lim`'s start and end: two `mktemp`, the command started in the
      background, a poll every 0.1 s with `sed` and `cut`, then `cat`
      and `unlink` of its output files;
    - the `date` that takes `reading_for`'s t0, and the `sleep` that
      waits out its rest.

    All of it runs on a runner whose disk is syncing groups of up to
    1.1 GiB. I did not split it further.
- **The author's figures say otherwise** (comment 7204030861): "a run is
  started when the run before it predicts it will end within the
  window, and then takes longer. At 10-s1000 a single list run took
  17.2 to 18.2 s, against 16.4 s in the step."
  - 17.2 to 18.2 s are the windows.
  - The runs themselves (`longest=` in each record's stdout) took
    15.63 to 15.82 s, one a round, so the rest of the 17 s was waited
    out.
- **Its effect.**
  - The author writes "A longer window can only raise the read side's
    longest write and its group count. So the overrun weighs against
    the read." That holds for the longest write.
  - For the groups it is the other way. §5.2 asks whether a read holds
    the pool's groups, and a hold lowers the count. A longer read
    window raises the count, so it hides a hold of about one group.
  - Per second, that bias is gone. 26.04, 1000:

    | read | counts, read / idle | per second, read / idle | per second, rounds lower |
    |---|---|---|---|
    | holds | 21 / 19 | 1.974 / 1.898 | 1 of 5 |
    | list | 19 / 20 | 1.876 / 1.998 | 5 of 5 |

- **Not blocking.** The line states the windows, so a reader can divide.
  The author's figures already do. The lead decides whether the line
  should do it too (decision 1).
- **Certainty:**
  - that the read windows are longer, and by how much: high (the
    records);
  - that the time is in `rec` and `lim`: high by elimination (`rec`'s
    `secs` against `reading_for`'s runs). I did not measure it on the
    runner.

## 3. Q3: `SERVE_PID` let go when SIGKILL does not end the daemon

**The code.** In both functions, when `serve_end 10` fails, the
function sets `SERVE_PID=""` and then calls `rig_error`:

- `reserve` at `agent-record.sh:275`;
- `end_daemon` at `cut-record.sh:613`.

**Path by path:**

- **The real rig, in an agent course.** `rig_error`
  (`cut-record.sh:242-253`) runs its hook, `agent_judged`, which runs
  `agent_quiet` (`agent-record.sh:668-713`). With `SERVE_PID` empty:
  - `agent_quiet` skips the verdict "had ended with no kill of the
    rig's". That verdict was the false second verdict of the last
    review's Q3, at `reserve`'s second call and after the cases.
  - It also skips the export and logs "not exported for the check: no
    daemon serves its device". The check then judges node B's stages
    with its own error, as for a failed export. Before the fix, the
    export could wait its whole 120 s on a device whose daemon would
    not end.
- **Then, on every course,** `rig_error` exits and the EXIT trap runs
  `cleanup` (`cut-record.sh:316-357`).
  - `cleanup` skips its SIGCONT, its SIGTERM, its `serve_end 30` and
    its `kill -9`. They would do nothing: the SIGKILL is already
    pending on the process. They would also cost 30 s.
  - The forced export under `lim 120` and the detach under `lim 60`
    still run.
- **A survivor is not hidden.** The process still exists, unreaped,
  with SIGKILL pending. The rig's error names it ("did not end 10 s
  after SIGKILL"), and the run's status is the check's or the rig's
  error, as before. The only check that looked at `SERVE_PID` after
  this point was `agent_quiet`'s, and what it found there was the
  false verdict.
- **The pid cannot be taken for another process.** An unreaped
  process keeps its pid, and the rig no longer holds it.

**The tests.**

- `reserve` (`test_agent_record.sh:241-249`):
  - Its stub `rig_error` records `SERVE_PID` as it was at the call, so
    the test pins the order.
  - Q01 (the pid not let go) and Q02 (let go after `rig_error`) are
    both killed.
- `end_daemon` (`ed_t ... unreaped`, `:785-791`, `:821-824`):
  - It prints the pid after `end_daemon` returns. Its `rig_error`
    returns, where the real one exits.
  - So it catches Q03 (the pid not let go), but not Q04 (let go
    after `rig_error`). In the real rig, Q04's `SERVE_PID=""` would
    never run, and that is the same as Q03.
  - A test gap: section 5.
- The author says "Tests for both: the pid is empty at the rig error."
  That is true of `reserve`'s test only.

## 4. The lows, against the plants

Each plant was applied alone to a worktree at af0c183. Then the suite
named ran under a 600 s limit, and the file was restored with
`git checkout`. `git diff --stat` was empty after each. The plants are
in `plants.py`; their output is in `plants_a.out`, `plants_b.out`,
`plants_q.out` and `plants_q2.out`.

| low | the change | plant | result |
|---|---|---|---|
| D1 | the wait removed | P01 | killed (`reading_for` checks) |
| D1 | 0.5 s past the window | P02 | killed |
| D1 | the line's windows swapped between the sides | P03 | killed |
| D1 | the idle window read from the read's record | P04 | killed |
| R3 | `failed` dropped from the line | P05 | killed |
| R3 | `failed` read from `runs=` | P06 | killed |
| R3 | a step read that ended 1 gets rounds | P07 | killed |
| D3 | NOT COVERED lines read again by `field_of` | P08 | PENDING_P08 |
| octal | `ZCR_SETTLE_S` read as octal in the budget | P09 | PENDING_P09 |
| octal | the budget always takes 30 s for the settle | P10 | killed |
| S01 | the settle after the margin | P11 | killed |
| S10 | the line's default for the groups 3 | P12 | killed |
| S12 | the line's default for the margin 5 | P13 | killed |
| S10 | the code's default for the groups 3, the line's 2 | P14 | killed |
| S12 | the code's default margin 3, the line's 2 | P15 | **survives** |
| S10 | the code's default limit 40, the line's 30 | P16 | killed |
| K04 | `@c` not let go before `counts-destroy` | P17 | PENDING_P17 |
| K04 | `release_quietly` lists but releases nothing | P18 | PENDING_P18 |
| Q3 | `reserve`: the pid not let go | Q01 | killed |
| Q3 | `reserve`: let go after `rig_error` | Q02 | killed |
| Q3 | `end_daemon`: the pid not let go | Q03 | killed |
| Q3 | `end_daemon`: let go after `rig_error` | Q04 | **survives** |

**The `median` question.** `median` still counts `>X` as X. The author
kept it on purpose:

- the line prints each value with its `>`;
- a lower bound is the best figure there is for that round.

I agree. That point is closed.

**The settle's comment** (`:1078-1089`) is accurate against both
dispatches:

- "about one window in ten still opens in it": 8 of 112 now, and 14
  of 112 before;
- "the large one can be the third to fifth to commit": as the replay
  in the last review found.

## 5. Test gaps and nits

**Test gaps:**

- **P15: `writing`'s own default margin is not pinned**
  (`zfs-cli-record.sh:1131`, and `:1401` in the budget).
  - The suite always sets `ZCR_WRITER_MARGIN`.
  - The settings line's default is pinned (P13 is killed). The code's
    default can drift from it without a failure.
- **Q04: the order in `end_daemon` is not pinned**
  (`test_agent_record.sh:821-824`).
  - A stub `rig_error` that records `SERVE_PID`, as `reserve`'s test
    has, would pin it.

**Nits:**

- **The reserve test leaves a `sleep 30` behind.**
  - Its stand-in is `bash -c 'trap "" TERM; sleep 30 & wait'`
    (`test_agent_record.sh:245`). The SIGKILL ends the `bash`, and the
    `sleep` lives on with the suite's stdout.
  - Through a pipe, the suite took 35.2 s against 27.2 s into a file.
  - `ed_t`'s stand-ins send their output to `/dev/null`; this one
    could do the same.
- **The comments.**
  - `sweep_rounds`' comment (`:1340-1354`) still says "Not run for a
    kind whose step's read has no time or was cut at its limit". It
    leaves out a step read that ended with another status.
  - That comment and the file's header (`:86-106`) give the about
    line's contents as the longest writes, the groups and the longest
    `zpool sync`. Neither mentions the reads' failures or the windows.
  - The workflow's input text says "an idle control of the same
    window" (`skyzfs-zfs-cli-record.yml:30`). The windows are not the
    same (D4).
- **The budget's fallback for the settle**
  (`zfs-cli-record.sh:1405-1407`).
  - A `ZCR_SETTLE_S` that is not a count is budgeted as 30 s, while
    `settle_txgs` says NOT SETTLED at once. Harmless: it budgets too
    much.
  - A count of five digits or more is also budgeted as 30 s, while
    `settle_txgs` waits that long. Only a mistaken setting would do
    this.
- **The reads' busy share is not in the line** (section 1).
- **`test_agent_record.sh` is flaky here under load.** It is not of
  this pull request.
  - The suite's temporary directory vanishes mid-run, and 16 to 23
    checks fail after that point.
  - It happens at 54b666b too (2 of 3 runs).
  - It happens most with two other suites running beside it. Alone, the
    suite passed (the Q04 runs and the final gate).
  - I did not find the cause:
    - A background child killed before its exec runs the parent's EXIT
      trap in bash 5.2 (`trap_race.sh`: 35 of 40 tries).
    - But guarding the suite's trap with its own pid did not stop the
      flake.
  - CI's runs of the suite pass.

## 6. The dispatch, run 45895208967 (af0c183)

**The inputs:**

- Ubuntu 24.04 and 26.04, each on its own runner;
- scale `1000 10:1000`, 5 rounds of 10 s;
- the same inputs as the earlier run 45884296395.

Both jobs are green. The tables are in `rounds_2404.txt` and
`rounds_2604.txt`, made by `rounds_check2.py` from the artifacts.

**Groups a second, read against idle, median of 5 rounds:**

| image | scale | read | read | idle | ratio by round | median ratio | rounds read lower |
|---|---|---|---|---|---|---|---|
| 24.04 | 1000 | list | 1.869 | 1.899 | 1.13 0.98 0.94 0.98 0.98 | 0.98 | 4 of 5 |
| 24.04 | 1000 | holds | 1.885 | 1.899 | 1.09 1.01 1.01 0.98 0.99 | 1.01 | 2 of 5 |
| 24.04 | 10-s1000 | list | 0.896 | 0.800 | 1.41 1.01 1.00 1.06 1.43 | 1.06 | 1 of 5 |
| 24.04 | 10-s1000 | holds | 0.895 | 0.800 | 1.15 1.40 1.03 0.79 1.28 | 1.15 | 1 of 5 |
| 26.04 | 1000 | list | 1.876 | 1.998 | 0.94 0.94 1.00 0.96 0.94 | 0.94 | 5 of 5 |
| 26.04 | 1000 | holds | 1.974 | 1.898 | 1.07 1.04 1.04 0.95 1.17 | 1.04 | 1 of 5 |
| 26.04 | 10-s1000 (W 17 s) | list | 0.814 | 0.823 | 0.69 1.11 1.07 1.06 0.96 | 1.06 | 2 of 5 |
| 26.04 | 10-s1000 | holds | 0.891 | 0.899 | 1.11 1.14 0.91 0.84 0.93 | 0.93 | 3 of 5 |

These figures match the author's table in comment 7204030861.

**What the figures can show:**

- **The resolution.**
  - One group is about 5% of a count at 1000 (17 to 24 a window) and 5
    to 14% at 10-s1000 (7 to 20 a window).
  - At 10-s1000 the idle rounds alone spread widely: 0.70 to 1.20 a
    second around a median of 0.80 (24.04, holds).
  - At 1000 the load's own rhythm is regular: idle 17 to 20 groups a
    window.
  - So these 5 rounds can show a consistent effect of about 20% or
    more at 10-s1000. At 1000 they can show one of about 5% at best.
- **What they show.**
  - No read lowered the group rate by 20% or more consistently.
  - At 1000 the list read is lower per second in 9 of the 10 rounds,
    on the two images together, by up to 6%. That is at the edge of the
    resolution, and the windows are unequal. It is a sign to keep, not
    a finding.
- **The longest write and the longest `zpool sync`.**
  - The writer's longest write is the same on both sides within 0.05
    s: medians of 2.9 s (24.04) and 2.75 s (26.04) at 10-s1000.
  - The longest `zpool sync` medians differ both ways.
  - As decision 2 of the last review said, neither figure can see a
    read below the pool's own syncs.
- **The earlier difference** (decision 3).
  - 26.04's list at 10-s1000 was 0.78, lower in 5 of 5 rounds; it is
    1.06 now.
  - This runner's idle rate was 0.82 a second against 0.43. The list
    read took 15.6 to 15.8 s a run against 11.8 s, and the window was
    17 s rather than 14 s.
  - One run on a different machine neither confirms nor refutes the
    earlier difference.
  - The earlier run's ratios, recomputed with this script, are in
    `old_rounds2_2404.txt` and `old_rounds2_2604.txt`.
- **The settle and the probe.**
  - Every one of the 56 settles per image ended `settled=`.
  - There was no NOT COVERED line, no line "rounds not run", and no
    `exit=` in either `about.txt`.
- **D2.** The counts are in decision 2. All of the late windows are at
  10-s1000:
  - 24.04: holds-idle-r2, holds-r4, holds-r5, list-idle-r3,
    list-idle-r5, `holds` and `take-cli`;
  - 26.04: holds-r5.

**Not checked by me:**

- the author's counts of windows whose longest write began before the
  window opened;
- the figures of the snapshot program's take.

## 7. The author's answer and figures, against the above

**Comment 7203342998:**

- **"the read side lasts as long as its idle control unless a single
  run goes past the window."**
  - True of `reading_for`'s own span.
  - Not true of the windows on the runners: D4.
- **"`rounds_t` and the real-pool rounds now require the read window to
  be at least the idle one."** Not true (section 1).
- **Q3, "Tests for both: the pid is empty at the rig error."** True of
  `reserve` only (section 3).
- **"All 10 were killed."** Their set and mine overlap:
  - mine also kill K04, S01, S10, S12, D3, R3, the octal settle, and
    Q3 removed in both functions;
  - two of mine survive (P15, Q04), and the author's set does not have
    them.
- **"shellcheck 0.11 clean."** I have 0.9.0. It finds no new code
  against 54b666b, only four more SC2317 notes (info) in
  `test_agent_record.sh`.

**Comment 7204030861:**

- **CI:** true (section 8).
- **The table:** true; it matches mine.
- **"The cause: a run ... then takes longer."** Not what the records
  show (D4).
- **"the overrun weighs against the read, not for it."**
  - True for the longest write.
  - Not true for the group count (D4).
- **"The differences left are about one group per window and go both
  ways."** True overall. The exception is the list at 1000, lower in 9
  of 10 rounds (section 6).

## 8. Gates and CI

**The local gates at af0c183** (`gates.sh`, `gates.out`):

| suite | result |
|---|---|
| `test_zfs_cli_record.sh` | 711 ok, 0 fail |
| `test_agent_record.sh` | 144 checks; passes alone. Flaky under load here, as at 54b666b (section 5) |
| `test_cut_record.sh` | all passed |
| `tests/lib/test_lim.sh` | all passed |
| `test_agent_check.py`, `test_txg_probe.py`, `test_dsync_writer.py`, `test_zfs_cli_json.py` | OK (80, 20, 15, 7 tests) |
| shellcheck 0.9.0 on the five changed files | nothing new against 54b666b but four SC2317 notes (info) |

**CI on af0c183**, read at the end of this look:

- **Green, on the pull request:**
  - ci (it runs `test_zfs_cli_record.sh` and `test_agent_record.sh`),
    coverage and e2e;
  - the agent rig: the check's logic, and a1, a2, d and k on 26.04;
  - the cut rig, cut-daemon, node B and publish.
- **Green, on the push:** skyzfs-ci.
- **Green, by dispatch:** the recorder, run 45895208967 (section 6).
- **Cancelled by hand two minutes in:** the daemon rig, the replay rig
  and the kernel probe on the pull request.
  - Their workflows run `daemon-rig.sh`, `replay-rig.sh`,
    `hole-rig.sh`, `ublk-probe.sh` and their checks.
  - None of these sources `zfs-cli-record.sh`, `agent-record.sh` or
    `cut-record.sh`.
- **Cancelled:** the push runs that duplicate the pull request's runs.
- **Codex:** no finding on af0c183.

## Files here

- **D1 and D4:**
  - `d1_demo_af0c183.out`: the last review's `../d1_demo.sh` at
    af0c183;
  - `d1_windows.sh`, `d1_windows.out`: each side's window through
    `writing`, `rec` and `lim`, path by path.
- **The plants:**
  - `plants.py`;
  - `plants_a.out` (P01 to P09), `plants_b.out` (P10 to P18),
    `plants_q.out` (Q01 to Q04, first runs; Q01 and Q02 there are mixed
    with the suite's flake);
  - `plants_q2.out`: Q01 and Q02 run again until a run had no flake,
    each with the failing line.
- **The dispatch:**
  - `rounds_check2.py`;
  - `rounds_2404.txt`, `rounds_2604.txt`: run 45895208967;
  - `old_rounds2_2404.txt`, `old_rounds2_2604.txt`: the earlier run
    45884296395, through the same script.
- **The rest:**
  - `trap_race.sh`, `trap_race.out`: the EXIT trap in a killed child
    (section 5);
  - `gates.sh`, `gates.out`: the local gates (section 8). The agent
    suite's failures in `gates.out` are the flake.

# Review of skyzfs/skyzfs#169: M2.T5 follow-ups, fourth round

- **Pull request:** skyzfs/skyzfs#169, `skyzfs-testing` into
  `zfs-topology` (at 2751e3b, its tip throughout the review).
- **Final head:** 54b666ba083844a464de15528dc76526f3749267, as the API
  gave it at the start of the delta's review and at the end.
- **Commits:**
  - 94ea1d7: #165's records (the counts after `holds -r`, odd tags).
  - 6d7c01d: G-a and G-b of #164's review.
  - 27f067d: #164's nits.
  - 55f4523: the settle before every window beside a writer; the
    sweep's rounds read back to back.
  - 54b666b: the settle's setting said in `about.txt`. It came during
    the review; section 6 covers it.
- **Order of reading.** I read the code at 55f4523 and the design's §5.2
  and formed my view before I read the description.

## Verdict

- **One defect blocks the merge: D1.** It is small to fix.
  - The read side of a round does not last the window. `reading_for`
    stops before a run that would pass the window, so the read side
    lasts up to one run less than the idle control, which is exactly
    the window.
  - The author's own dispatch on 55f4523 (run 45884296395) shows it.
    The list rounds' read windows were 8.6 to 9.2 s at 1000 on 24.04,
    against 10.0 s idle. At 10-s1000 on 26.04 they were 12.0 s against
    14.0 s.
  - "Groups committed" is a count over the window, so the about line
    reads as if the list read held the groups:
    - 24.04 at 1000: "read 17, idle 19". Per second it is 1.94 read
      against 1.90 idle.
    - 26.04 at 10-s1000: "read 4, idle 6". Per second it is 0.33
      against 0.43: a difference, but smaller than the count's.
  - That is the very question of design §5.2 that the rounds exist to
    answer. The description's "The idle control lasts the same window
    (R2)" is not true of the code. Codex found the same (P2, on
    `zfs-cli-record.sh:1304`).
- **A measurement gap, for the lead, not blocking: D2.**
  - Two groups since the probe began are not always past the load's
    first large group (300 MiB or more):
    - by replay on #164's run, 9 of 108 windows would open before that
      group commits;
    - in the dispatch, 14 of 112 windows did, 8 of them round windows,
      all on 24.04 at 10-s1000.
  - Past it, the load's syncs in steady state hold the O_DSYNC writer as
    long as the first did. In the dispatch at 10-s1000, each window's
    longest write was about 3 s on 24.04 and 4.4 to 5.8 s on 26.04, on
    both sides, whether or not the window opened after the first large
    group.
  - So the settle removes the start-up stall R1 was about. It does not
    make "longest write", or the longest `zpool sync`, able to see a
    read.
- **The author's three questions** (section 2):
  - **Q1, the settle and the windows.** They answer R1's start-up stall
    in most windows. Two groups are not always enough (D2). The rounds'
    windows are not equal (D1).
  - **Q2, G-a in `recover`.** Sound, and today a no-op. In a dhook case
    the only rig error is written before `kill.tsv`. It cannot fail a
    legitimate recovery or hide a fault.
  - **Q3, the race.** It is in `reserve`'s and `end_daemon`'s SIGKILL
    branch. "Only the message" holds for the exit status. But since
    G-a, at `reserve`'s second call, the race adds a second verdict
    whose text is false. A one-line fix is given.
- **Lows and nits:**
  - D3: `field_of` now reads figures from a NOT COVERED line into the
    medians, which #164's `stall_of` did not.
  - R3 is half done: a step read that failed other than at its limit
    still gets rounds, and the about line does not give `failed=`.
  - Test gaps: K04 and S01 survive (section 7).
  - Smaller nits in section 5.
- **54b666b's line is true of the code.**
  - Its test fails when the line's limit is wrong or when it is said
    once per entry (S11, S13).
  - It does not fail when the line's default for the groups or the
    margin drifts from the code's (S10, S12 survive), because the suite
    sets both. That is a nit.
- **#165's records are what #165 asked for**, on both pools, and the
  dispatch recorded them (section 9).
- **CI on 54b666b is green** for every job that ran to its end. The
  daemon rig, the replay rig and the probe were cancelled; they do not
  drive the changed scripts (section 9).

## Decisions for the lead

1. **What opens a window (D2).**
   - Two groups since the probe began leave some windows inside the
     load's first large sync: 9 of 108 by replay on #164's run, and 14
     of 112 in the dispatch.
   - Ways checked on those records:
     - **Wait for the first slow write of the writer of new data**
       (`fill.tsv`). It ended within 50 ms of the large group's commit
       in 105 of 108 records of #164's run and in 54 of 56 on 26.04 in
       the dispatch. But on 24.04 in the dispatch it did so in only 32
       of 56: that runner's disk took the first large groups without a
       slow fill write. So it is not a sure marker either.
     - **Wait for more groups.** With 3, 2 windows of 101 still opened
       during it, by replay; with 4, 1 of 85; with 5, none of 79. Each
       group can cost a sync of about 5 s at 10-s1000, against the
       settle's limit of 30 s.
     - **Wait for a group of at least a set share of
       `zfs_dirty_data_max`.**
   - Decision 2 makes this matter less. If a round compares rates and
     not the longest write, the start-up stall weighs only on the first
     seconds of a window.
2. **What a round compares.**
   - Each window's longest write is the load's own sync, as long in
     steady state as the first:
     - in #164's records, steady-state medians of 5.4 s at 10-s1000
       and 1.4 s at 1000;
     - in the dispatch, about 3 s (24.04) and 4.4 to 5.8 s (26.04) at
       10-s1000, on both sides.

     A read shows in that figure only by lengthening a sync. The
     longest `zpool sync` waits on the same syncs: 8.4 to 11.5 s on both
     sides at 10-s1000 on 26.04.
   - The figures that can show a read are groups per second over equal
     windows (D1), and each group's sync time per MiB written for the
     groups that overlapped a read. The second is R1's suggestion; the
     about line does not give it.
   - Per second, the dispatch shows one difference: the list read at
     10-s1000 on 26.04, 0.33 groups a second against 0.43 (a ratio of
     0.78), lower in each of the five rounds. Every other pair's ratio
     is between 0.96 and 1.08 (section 9). It is worth a second look before the notes say "the
     list read adds nothing measurable".
   - Whether the note on §5.2 rests on the about line's medians as they
     are, or on rates, is the lead's call.
3. **The settle on every record, or only on the rounds.** The author
   asks this. It is right on every record beside a writer: the take and
   read records had the same 2 s margin and so the same start-up stall.
   Records from #164 and earlier are therefore not comparable with
   these in their stall figures. The notes should say so where they
   quote take stalls. It cost each window 1 to 2 s on 24.04 and 1 to 7
   s on 26.04 in the dispatch. The jobs took 23 and 31 minutes, against
   17 for #164's.

## 1. What the pull request does, as I read it

- **#165's records** (`zfs-cli-record.sh:806-853`). After `holds-r`,
  the counts read that the agent makes next. Then `@c` taken and held
  with `-r`, its holds read, and the middle name of that list released
  and destroyed before the same counts read. Then odd tags (a tab, a
  newline) held, listed and released on `@t`.
- **G-b** (`agent-record.sh:264-278`, `cut-record.sh:603-611`). A
  SIGTERM that finds no process is a verdict, `daemon_stopped`, with
  the status `serve_end` reaps. Before, `|| true` let a daemon that
  had ended by itself pass as a clean stop.
- **G-a** (`agent-record.sh:318`, `:695`). dhook's exemption holds
  only until `kill.tsv` is written.
- **Nits.** `cleanup` detaches whenever the course went through the
  daemon and the volume is there, under `lim` (`cut-record.sh:342-350`).
  `quiet_hang` records `seq`'s bounds and a test pins them.
- **The settle** (`zfs-cli-record.sh:1078-1106`, called at `:1148`).
  `writing` waits, before its margin, until `ZCR_SETTLE_TXGS` (2) txg
  lines whose END is after the probe's start are in its log, for at
  most `ZCR_SETTLE_S` (30) s. It is polled every 0.1 s. The probe reads
  the kstat once a second, so a commit is seen up to about 1 s late.
- **The rounds** (`:1284-1417`). Each kind runs `reading_for W` beside
  the load, against `idle_for W`. W is the window, at least the step's
  read rounded up. Odd rounds run the idle first. The about line gives
  each side's longest write, `txgs=` and `sync_longest=`, with medians.

## 2. The author's three questions

### Q1. Do the settle and the new windows answer R1? Is two groups right?

**In most windows, yes; not in all (D2), and the windows are not
equal (D1).**

- **How the large group falls.** The probe runs `zpool sync` at its
  start, which commits the small groups open then. So the load's first
  large group (300 MiB or more) is often not the first or second to
  commit. In #164's 108 writer records it was:
  - the 2nd group to commit after the probe's start in 92 records;
  - the 3rd in 9, the 4th in 6, the 5th in 1.

  The settle's comment, "the group open when the load began commits
  first or second", is true of that group. It is not true of the
  large one.
- **Replayed.** `settle_sim.py` replays the settle on those records:
  the probe's reads of the kstat, the 0.1 s poll and the 2 s margin.
  With two groups, 9 of 108 windows would open 0.78 to 2.41 s before
  the large group committed:

  | image | record | large group | its sync | commits after t0 |
  |---|---|---|---|---|
  | 24.04 | scale-10-s1000-holds | 5th, 1056 MiB | 4.94 s | +2.39 s |
  | 24.04 | scale-10-s1000-sweep-holds-idle-r4 | 3rd, 898 MiB | 4.17 s | +1.79 s |
  | 24.04 | scale-10-s1000-sweep-holds-r1 | 3rd, 1079 MiB | 5.02 s | +2.25 s |
  | 24.04 | scale-10-s1000-take-cli-get | 3rd, 811 MiB | 3.77 s | +0.89 s |
  | 24.04 | scale-10-s1000-take-cli-list | 3rd, 988 MiB | 4.58 s | +2.41 s |
  | 24.04 | scale-1000-holds | 4th, 646 MiB | 3.42 s | +0.78 s |
  | 26.04 | scale-10-s1000-sweep-holds-idle-r3 | 3rd, 927 MiB | 4.30 s | +2.00 s |
  | 26.04 | scale-10-s1000-take-cli-list | 3rd, 1002 MiB | 4.65 s | +1.61 s |
  | 26.04 | scale-10-s1000-take-cli | 3rd, 925 MiB | 4.30 s | +1.74 s |

  Three of the 40 holds round windows are among them. Outputs:
  `sim_n2.txt` to `sim_n6.txt`.
- **Does the measurement separate the read's effect from the
  first-sync stall?**
  - Where the window opens after the large group, yes: that stall is
    out of the window.
  - But the O_DSYNC writer waits out every large group's sync, not only
    the first (`steady.py` on the same records, `steady.txt`):

    | scale | first large sync's stall | longest stall after it |
    |---|---|---|
    | 10-s1000 | median 4.14 s (54 records) | median 5.39 s, 2.83 to 5.93 s (27 records with 5 s or more after it) |
    | 1000 | median 2.63 s (54 records) | median 1.42 s, 1.18 to 4.30 s (49 records with 2.5 s or more after it) |

  - A 10 s window at 10-s1000 holds one or two such syncs on either
    side. So its longest write is the load's own sync whether a read
    runs or not.
  - The settle moves the comparison from the first sync to a later
    one. It does not make "longest write" able to see a read.
  - The groups-committed figure can see one, if the windows are equal
    (D1).
- **Is two groups right?** As a count, it is too few for about one
  window in eight to twelve: 9 of 108 by replay on #164's run, 14 of
  112 in the dispatch (D2). A signal from the load itself would be
  better, though the one I tried is not sure either (decision 1).

### Q2. G-a in `recover`: sound?

**Yes. Today it changes no outcome.**

- In a dhook case `rig-error.txt` is written only by `case_error`.
  `scen_dhook` calls it once, at `agent-record.sh:579`, and returns
  before `kill.tsv` is written at `:580` or `:584`. The other calls
  (`:478`, `:531`, `:533`) are in `scen_destroy` and `scen_gone`.
  So when `recover` runs in a dhook case with a rig error, `kill.tsv`
  is absent, and the added `[ ! -e "$CDIR/kill.tsv" ]` is true. The
  outcome is as before.
- It cannot fail a legitimate recovery. The only recovery it would
  turn into a verdict is one where `kill.tsv` exists. The instance then
  serving has no hook armed (`serve_again` at `:582`, or `reserve` at
  `:587`), so if it ended, nothing the rig did ended it.
- It cannot hide a fault: it only narrows an exemption.
- If a later change adds a `case_error` after `kill.tsv`, the narrowed
  rule gives the right verdict for the unhooked instance. The wide
  rule would have logged it away.
- Plant G01, the condition removed, is killed (section 7).

### Q3. "A race that only affects the message text": where, and is it true?

- **Where.** It was in my #164 review, about the SIGKILL branch of
  `reserve` (`agent-record.sh:270-273`):
  ```
  kill -9 "$SERVE_PID" 2>/dev/null || true
  serve_end 10 || rig_error "skyzfsd serve $SERVE_K did not end 10 s after SIGKILL"
  ```
  The same branch is in `end_daemon` (`cut-record.sh:608-611`).
- **The race.**
  - `serve_end 10` gives up, and `SERVE_PID` stays set.
  - `rig_error` runs `agent_judged` (`agent-record.sh:635`, `:654`),
    which runs `agent_quiet`. Before it reaches the daemon (`:691`),
    `agent_quiet` stops the agent and the writer, and ends the session.
  - If the daemon dies in that time, `agent_quiet` finds it ended and
    takes it for one that ended by itself.
- **What it says, by where it happens:**
  - **`reserve`'s first call** (`:551`, no `kill.tsv` yet). A log line,
    "had ended ... in a dhook case, after the rig's own error". That is
    harmless.
  - **`reserve`'s second call** (`:587`, after `kill.tsv` says
    `killed_rc none`). With G-a this is now `agent_daemon_wrong
    daemon_serving "... with no kill of the rig's ..."`: a second
    verdict, and its text is false, because the rig sent SIGTERM and
    SIGKILL. At 22009e9 the wide exemption made it a log line. My #164
    text was imprecise on this.
  - **`end_daemon`, after the cases** (`CDIR` empty). The same false
    verdict, in `cases/course`. This was so before #169 as well.
- **Is "only the message" true?** For the recording's status, yes:
  `daemon_stopped` was given first, so the status is 1 either way. For
  the pins, no: the check sees a second verdict, `daemon_serving`, that
  names a fault the daemon did not have.
- **Certainty:**
  - high that the path exists;
  - low that it happens: it needs a daemon that outlives SIGKILL by
    more than 10 s, for example one held on the device.
- **Fix.** In both SIGKILL branches, clear `SERVE_PID` before
  `rig_error`. The daemon was killed by the rig:
  ```
  serve_end 10 || { SERVE_PID=""; rig_error "... did not end 10 s after SIGKILL"; }
  ```
  Or keep a flag that `agent_quiet` reads. Not blocking.

## 3. The defect

### D1. The read side of a round is shorter than the idle side (blocks)

- **Where.** `zfs-cli-record.sh:1302`:
  ```
  awk ... 'BEGIN { exit !(now - t0 + l <= s) }' || break
  ```
  This is against `idle_for "$w"` at `:1404` and `:1408`. The header at
  `:1329` says "an idle control: the load alone for the same window".
- **What happens.**
  - `reading_for` starts a run only when the last run's time would
    still end within the window. So the read side lasts between
    `w - last` and `w`.
  - The idle side lasts `w`.
  - With runs near half the window, the read side lasts about one run.
- **Shown** (`d1_demo.sh` on the recorder at 54b666b, `d1_demo.out`):
  ```
  run of 4.9 s in a window of 10 s: runs=2 failed=0 longest=4.905, the window 9.821 s
  run of 5.1 s in a window of 10 s: runs=1 failed=0 longest=5.103, the window 5.111 s
  ```
- **Where it bites.**
  - "Groups committed" is a count, and a count scales with the window.
    The dispatch shows it (table below). On 24.04 at 1000 the list
    read's side committed 17 groups against 19. Per second, that is
    1.94 against 1.90: the whole difference is the window.
  - In the about line that reads as "the list read holds the groups",
    which is design §5.2's question, answered wrongly.
  - It is worst where a read takes just over half the window. In
    #164's run the step's list read at 10-s1000 on 24.04 took 4.89 to
    5.15 s, so W = 10 and half the read windows would have been about
    5 s against the idle's 10.
  - It is least where the reads are short. The holds reads (about
    0.02 s at 10-s1000, 0.7 to 0.9 s at 1000) left windows from 0.2 s
    short to 1.4 s long; a single slow run can also carry a window
    past W.
- **Fix,** either:
  - after the last run, sleep out the rest of the window, so the read
    side lasts at least `w`, as the idle does;
  - or give each side's window (the record's file `window` already has
    it) and its groups per second, and compare those.

  The first keeps the about line's figures comparable as they are.
- **The test.** `rounds_t` accepts a read window of 0.6 of a 1 s idle.
  A plant that makes the read side one run long (W02) is killed. The
  test does not pin the two sides equal, and with the fix it should.
- **Certainty:** high (the code, the demonstration and Codex's thread
  agree).
- **In the dispatch on 55f4523** (run 45884296395; `rounds_check.py`,
  `rounds_2404.txt`, `rounds_2604.txt`). The window is the record's
  file `window`, t1 - t0. Medians over the five rounds:

  | image | scale | kind | W | read window | idle window | groups read / idle | groups a second read / idle |
  |---|---|---|---|---|---|---|---|
  | 24.04 | 1000 | list | 10 | 8.79 s (8.60 to 9.17) | 10.00 s | 17 / 19 | 1.94 / 1.90 |
  | 24.04 | 10-s1000 | list | 10 | 9.20 s (8.63 to 10.39) | 10.01 s | 8 / 8 | 0.81 / 0.80 |
  | 26.04 | 1000 | list | 10 | 9.40 s (9.37 to 9.43) | 10.01 s | 9 / 10 | 0.96 / 1.00 |
  | 26.04 | 10-s1000 | list | 14 | 12.00 s (11.98 to 12.19) | 14.01 s | 4 / 6 | 0.33 / 0.43 |
  | 24.04 | 1000 | holds | 10 | 9.92 s | 10.00 s | 19 / 19 | 1.91 / 1.90 |
  | 24.04 | 10-s1000 | holds | 10 | 10.16 s | 10.00 s | 9 / 8 | 0.79 / 0.80 |
  | 26.04 | 1000 | holds | 10 | 10.01 s | 10.01 s | 11 / 10 | 1.08 / 1.00 |
  | 26.04 | 10-s1000 | holds | 10 | 10.19 s | 10.01 s | 4 / 4 | 0.40 / 0.40 |

  - The list reads ran 1, 3 or 4 times a window, so their windows
    ended up to one run short.
  - The holds reads ran 11 to 535 times, so theirs ended near W.
  - The about line's "groups committed" differ between the sides in
    the first and fourth rows mostly because of the window. The probe's
    `txgs=` equals the count of groups whose END falls in the window
    in every record.

## 4. Measurement gap and lows

### D2. Two groups are not always past the first large sync (lead)

See Q1 and decision 1. Not blocking: it is a matter of the
measurement's design, and it touches about one window in eight to
twelve.

- **In the dispatch.** For each record with a settle, the first
  group of 300 MiB or more that committed after the probe's start is
  compared with the window's t0 (`rounds_check.py`, the end of
  `rounds_2404.txt` and `rounds_2604.txt`):
  - **24.04: 12 of 56 windows opened 0.08 to 0.85 s before that group
    committed.** Eight are round windows at 10-s1000, four read and four
    idle. The other four are the holds, snapshot, sweep-list and
    take-cli-get records at 10-s1000. In each, the settle counted two
    small groups that the probe's first `zpool sync` committed, and
    settled within about 1 s. The writer's longest write then
    overlapped the window: about 3.0 s, ending at the large group's
    commit. For example, `scale-10-s1000-sweep-list-idle-r2` had groups
    of 84 and 264 MiB commit 3.0 and 2.3 s before t0, and one of
    1131 MiB, synced in 2.96 s, commit 0.67 s after it.
  - **26.04: 2 of 56**, the two snapshot records. No round window.
  - All 112 settles ended `settled=`; none hit the limit.
  - On 24.04 the later syncs were as long as the first (about 2.9 s), so
    the windows that opened during it read the same longest write as
    those that did not (decision 2).

### D3. `field_of` reads figures from NOT COVERED lines (low)

- **Where.** `zfs-cli-record.sh:1309-1315`. `field_of` takes `KEY=` from
  anywhere in the line. `stall_of` is now `field_of NAME stall
  longest`.
- **The regression.** In #164 it was
  `sed -n 's/^longest=\([^ ]*\) .*/\1/p'`, anchored at the line's
  start. So a writer line that begins "NOT COVERED: ...; longest=..."
  (`dsync_writer.py:125`) gave `?` and was left out of the median. Now
  its figure is taken. The same holds for `txgs=` and `sync_longest=`
  after the probe's "NOT COVERED: ...;" (`txg_probe.py:208`).
- **Effect.** A window that its writer or probe did not cover gives a
  lower bound, and that lower bound goes into the median as a figure.
- **Test gap.** No test has a NOT COVERED line in a round's record.
- **Fix.** Match `^KEY=` or `[ ;]KEY=` only on lines that do not begin
  "NOT COVERED", or keep `?` for them.
- **Certainty:** high that it is so; low effect, since NOT COVERED is
  rare.

### R3 half done (low)

- A kind whose step read was cut at its limit (rc 124) has no rounds
  (`:1374`). A step read that failed otherwise, for example
  `zfs holds` on a name gone, still gets rounds. Its runs fail fast,
  and their `failed=` is in each record's stdout but not in the about
  line (`:1415` gives `runs` only).
- A round whose reads all failed reads in the about line like one that
  read.
- **Fix.** Give `failed=` beside `runs=`, or skip a kind whose step's
  rc is not 0.

### The settle's limit with a leading zero (nit)

`need` at `:1385` takes `${ZCR_SETTLE_S:-30}` in `$((...))`, where `030`
is octal (24), and `08` or `09` is an arithmetic error. `settle_txgs`
reads it as `10#$limit`, in decimal. The workflow does not pass it, so
only a hand-run sees it.

## 5. Nits

- **`median` of a `>x`.** A `zpool sync` still open at the probe's stop
  reads `>X` and the median counts it as X (`:1320`). It is said in the
  comment, but `>X` is a lower bound. One sync reached 10.58 s in #164.
- **The counts read's order.** `X-holds-r-counts` names the snapshots in
  `holds -r` order. The agent sorts them (a `BTreeSet`) and reads in
  batches. Its parser does not depend on order, so "the agent's next
  read" is right in content, not word for word.
- **The settle's comment** (`:1084-1085`): "The group open when the load
  began commits first or second." True, but the group that matters is
  the large one, which comes later (Q1).
- **S01 survives.** Moving the settle after the margin passes every
  test. The description says "before its margin", and 54b666b's line
  does not say the margin follows the settle.
- **The header's cost.** "2 * (window + 40) s" is the most a round may
  take, with the settle at its limit. That is the right figure for the
  budget. A round's real cost is nearer 2 * (window + 10) s.

## 6. 54b666b: the settle's setting in `about.txt`

- **The line** (`zfs-cli-record.sh:1004`), said once by `scale_all`
  before its entries:
  "scale settings: each window beside a writer opens after
  ZCR_SETTLE_TXGS=N groups have committed since its probe began, waiting
  at most ZCR_SETTLE_S=S s (0 groups: no wait, the margin of
  ZCR_WRITER_MARGIN=M s alone)".
- **Is it true of the code?** Yes.
  - Its defaults are those of `settle_txgs` (2, 30) and `writing` (2).
  - Every call of `writing` is inside `scale` (`:1404-1550`), and
    `scale` is called only from `scale_all`, which `main` calls once
    (`:1757`). So "each window beside a writer" covers every window
    that has a settle.
  - With `ZCR_SETTLE_TXGS=0` the settle returns at once with
    `settle=off`, and the margin alone opens the window, as the line
    says.
- **Imprecise, nits:**
  - It does not say that the margin also follows the settle when there
    is one.
  - It is said even when every entry is refused and no window opens.
  - With a non-number in `ZCR_SETTLE_TXGS` it states that value as a
    count. Each record then says NOT SETTLED, so nothing is hidden.
- **Does the test fail if the line is wrong?** The test greps the whole
  line, with the suite's settings, and counts it once. Plants
  (`p6b.py`, one at a time):

  | plant | what is wrong | result |
  |---|---|---|
  | S09 | the line not said | killed |
  | S10 | the line's default for the groups 3 (the code's still 2) | **survives** |
  | S11 | the line's default for the limit 3 (the code's still 30) | killed |
  | S12 | the line's default for the margin 5 (the code's still 2) | **survives** |
  | S13 | the line said once per entry | killed |

  - S10 and S12 survive because the suite sets `ZCR_SETTLE_TXGS=0`
    and `ZCR_WRITER_MARGIN=0.3` for every test, so the line's own
    defaults are never read.
  - A drift between the line's defaults and the code's would go
    unseen. Fix: one `scale_all` call with both unset.
  - Low.

## 7. Plants

Each was applied alone to a copy of the tree at 54b666b and run
against the suite named in `p6.py` and `p6b.py`, with `plant4.py`.
Every copy was restored and `git diff --stat` was empty after each run.
They were written before I read the author's list.

- **The settle.**
  - Killed: S02 (not called), S03 (the kstat's history counted), S04
    (ticks counted as groups), S05 (a default of one group), S06 (the
    limit read as tenths), S07 (no `settled` file), S08 (no about
    line), S09 (no settings line).
  - **S01 survives:** the settle after the margin instead of before it.
    No test tells the order apart; the fake commits its groups at once.
- **The rounds.**
  - Killed: W01 (a run past the window), W02 (one run only), W03 (the
    idle as long as the step's read).
  - Killed: W04 (no rounding up), W05 (the window ignores the step),
    W06 (a cut read's rounds run), W07 (no budget check), W08 (the
    need without the settle), W09 (the limit against one window), W10
    (octal rounds), W11 (the groups read from the control), W12
    (`longest_sync` for `sync_longest`), W13 (the stalls read from the
    control), W14 (`>x` not read as a number).
- **#165's records.**
  - Killed: K01 (the first name gone, not the middle), K02 (names not
    made unique), K03 (destroyed without its release), K05 (a space for
    the tab), K06 (the newline's tag not released).
  - **K04 survives:** `@c` not let go before `counts-destroy`. On a pool
    the `destroy -r` then fails, because the snapshots are held, and
    leaves `@c` held on every dataset. The fake's `destroy` does not
    check holds, and no test pins `release_quietly`'s calls. Low.
- **G-a, G-b.** Killed: G01 (`recover` ignores `kill.tsv`), G02
  (`agent_quiet` ignores it), G03 (the test inverted), G04 (`reserve`
  back to `|| true`), G05 (no serve again after the verdict), G06 (the
  status not read), G07 (`end_daemon` back to `|| true`), G08 (no
  detach after its verdict).
- **The nits.** Killed: N01 (the detach only with a daemon), N02 (under
  `timeout`), N03 (no volume check), N04 (not only through the daemon),
  N05 (the agent's bound 2000), N06 (the writer's bound 1200).

Outputs: `p6_a.out`, `p6_b.out`, `p6_c.out` and `p6b.out`. Each kill
was read from its own FAIL lines.

## 8. The local gates, at 54b666b

| gate | result |
|---|---|
| `bash tests/kernel/test_zfs_cli_record.sh` | all passed, 702 checks (208 s) |
| `bash tests/kernel/test_agent_record.sh` | all passed, 142 checks |
| `bash tests/kernel/test_cut_record.sh` | all passed, 109 checks |
| `python3 -I tests/kernel/test_agent_check.py` | OK, 80 tests |
| `python3 -I tests/kernel/test_txg_probe.py` | OK, 20 tests |
| `python3 -I tests/kernel/test_dsync_writer.py` | OK, 15 tests |
| `python3 -I tests/kernel/test_zfs_cli_json.py` | OK, 7 tests |
| `bash tests/lib/test_lim.sh` | all passed |
| shellcheck 0.9.0 on the changed scripts | no code that the base does not have; only more SC2317 notes (info) |

## 9. CI and the dispatch

- **CI on 54b666b,** read at the end of the review:
  - green: ci (lint, test, unit-arm64, s3-backends, chart-lint),
    coverage, e2e, node B (both images), publish, cut-daemon (all three
    jobs), the cut rig (all four images), and the agent rig (a1, a2, d,
    k);
  - the agent rig's job d runs the dhook cases. In each the hook fired
    (`premise_d-*_daemon_killed: ok`), so each case went through
    `reserve`'s first call with a live daemon (the SIGTERM finds it),
    then `kill.tsv` and `serve_again`, and the narrowed `recover` and
    `agent_quiet` after it. Every pin passed;
  - cancelled: the daemon rig, the replay rig and the kernel probe,
    which do not drive the changed scripts, and the push run of
    `skyzfs-zfs-cli-record` (`unit`, `matrix.image`).
- **Codex:** one finding on 55f4523, P2, the same as D1. Nothing on
  54b666b when this was written.
- **The dispatch,** run 45884296395 on 55f4523, Ubuntu 24.04 and 26.04,
  scale `1000 10:1000`, 5 rounds of 10 s. Both jobs green: 23 and 31
  minutes.
  - **The rounds' about lines** are in D1's table, and the medians are
    in `rounds_2404.txt` and `rounds_2604.txt`.
  - **The settle** ended `settled=` in all 112 windows: medians of
    0.98 s on 24.04 and 4.0 s on 26.04 (most 2.0 and 7.0 s).
  - **No NOT COVERED line,** no probe error and no failed `zpool sync`
    on either image.
  - **#165's records are there on both pools.**
    - `X-holds-r-counts` ended 0, with each name once.
    - `X-holds-r-counts-gone` ended 1. It listed the names that were
      left and said "cannot open 'skzq/child@c': dataset does not
      exist" on stderr.
    - `holds -H` with the odd tags printed the tab inside the tag, so
      the line has four tab-separated fields, and split the newline's
      line in two.
    - Every hold and release ended 0.
  - **Not of this pull request, for the lead's notes:** the snapshot
    program (`snapshot.lua`) at 1000 file systems held one sync for
    36.5 s on 24.04 and 48.2 s on 26.04. The O_DSYNC writer beside it
    waited 54.0 s on 26.04 but no more than 0.92 s on 24.04, over 89345
    writes.

## 10. The description against the diff

Checked claim by claim. True unless said otherwise.

- **Commits and base.** True: four commits on 2751e3b, which is still
  `zfs-topology`'s tip. 54b666b is not listed; it came after.
- **#165's records.**
  - The counts read on the names `holds -r` gave, once each
    (`holds_names`), on both pools (`record_pool rig` and
    `record_pool mirror`, `:1717-1718`). This matches what #165's
    description asked of testing.
  - "With two names or more, the middle one released and destroyed":
    true (`:828`, `:831`).
  - "the agent's next read" is true of the command and the names, not
    of their order or batching (section 5).
- **G-b.** "A SIGTERM that finds no process is now the verdict
  `daemon_stopped`" is true in `reserve`, and in `end_daemon` in an
  agent course. In other courses `end_daemon`'s `daemon_wrong` makes it
  the rig's error (`cut-record.sh:238-241`), as the other
  `daemon_stopped` lines there already were. That is right, but not
  what the sentence says.
- **G-a.** True; judged in Q2.
- **The nits.** True:
  - the detach is under `lim`, with the daemon or not, and the volume
    is checked;
  - the bounds are pinned (N05, N06 killed);
  - the comment says `skz-sweep-ID`;
  - `scale`'s comment is above `scale()`, and the cadence sentence is
    gone;
  - the rounds count is read as decimal.
- **"a race that only affects the message text ... please point at the
  line".** Placed in Q3.
- **The settle.** True as described: before the margin, the file
  `settled`, the about line, every record beside a writer.
- **"A run is not started where it would end past the window."** True
  only as a prediction from the last run's time. **"The idle control
  lasts the same window (R2)"** is not true (D1). The header (`:1329`)
  and the workflow's `sweep_rounds` text ("an idle control of the same
  window") say the same.
- **R3.** True for a read cut at its limit and for the budget and the
  limit. A step read that failed otherwise still gets rounds (section
  4).
- **The about line.** True as described. `failed=` is not in it.
- **R4.** "about 2 × (window + 40) s": true as the most a round may
  take (section 5).
- **Verification.**
  - 701 checks at 55f4523: true. 702 at 54b666b.
  - The other suites pass (section 8).
  - "shellcheck 0.11 ... clean": I have 0.9.0. It finds no new code
    against the base, only more SC2317 notes, which are info.
- **Plants.**
  - Mine are a separate set, written before I read the author's list.
    They agree on what both cover.
  - "The writer bound's text occurs twice in the file, so that plant
    did not apply as written": with three more lines of context it
    applies once, and it is killed (N06).

## Files here

- `d1_demo.sh`, `d1_demo.out`: D1's demonstration on the recorder.
- `settle_sim.py`, `sim_n2.txt` to `sim_n6.txt`: the settle replayed on
  #164's records with 2 to 6 groups (Q1, D2).
- `steady.py`, `steady.txt`: the first large sync's stall and the
  longest after it, on #164's records.
- `fillsig.py`, `fillsig.txt` (#164's run), `fillsig_run2_2404.txt`,
  `fillsig_run2_2604.txt` (the dispatch): the fill writer's first slow
  write against the large group's commit (decision 1).
- `rounds_check.py`, `rounds_2404.txt`, `rounds_2604.txt`: the
  dispatch's rounds, windows and settles (D1, D2).
- `plant4.py`, `p6.py`, `p6b.py` and their outputs `p6_a.out`,
  `p6_b.out`, `p6_c.out`, `p6b.out`: the plants (sections 6 and 7).
  Temporary directories in the outputs read `TMPDIR`.

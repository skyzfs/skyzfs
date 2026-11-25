## Next look at #144 at 45648ab and 6b05991 (M2.6b2)

**The heads.** I began at `45648ab`, checked with `git fetch origin`
and the pull request's `head.sha`. During the look the head moved to
`6b05991`, one commit on `45648ab`. That commit changes the fake's
assist to write two groups, and the docs of `ASSIST_QUICK`,
`Zfs::assist` and one test, after #145's kernel probe. I extended the
look to it.

**Which head each part was judged at.**

| Part | Judged at | Re-run at 6b05991 |
|---|---|---|
| Items (1) to (6), N1 to N3, TG1 to TG4, S2 | `45648ab` | yes: the 35 plants of `45648ab`, the scratch tests and the candidate fix of N1. Every result is the same. |
| S1 | `45648ab` | settled by `6b05991` |
| Item (8), TG5, S3, plants G1 to G5 | `6b05991` | (judged there) |
| Gates | both | 67 passed at each |
| CI (7) | both | the runs of `45648ab` and of `6b05991` |

- `engine.rs`, `record.rs` and the planner's code are the same at both
  heads. `6b05991` changes `plan.rs` in a doc comment only.
- At `6b05991`, a line of `plan.rs` after 156 is five more than the
  number this note gives: N1's arm is at `plan.rs:571-583`, and
  `away()` at `plan.rs:623`.

**What changed since the first look (`b5063b2`).** Four commits on the
branch:

- `f6ef194`: the pace of the sync task (L4);
- `28d1d64`: F1, T1 to T3, and F2's docs;
- `45648ab`: the age bound (L2), and the record's `begun_ms`;
- `6b05991`: two groups per assist in the fake, and the docs that
  follow from #145's probe.

The "17 or so" of the brief counts the base. `zfs-topology` moved from
`59c4ad5` to `7c9fa5a` (#143 and its notes). That change is 14 files
under `tests/` and `docs/`, none under `crates/`, `bins/` or the Cargo
files. The branch has not merged it. It merges cleanly
(`git merge-tree`).

**How I worked.** I read the code first, then the author's answer
(issue comment 7161908568). Then I checked each claim with plants and
scratch tests.

**Verdict: blocked on N1, which is new and comes from the age bound.**

- F1 is fixed. The report is now at least once in every window of a
  stop.
- T1 to T3 are answered. Their plants are killed, except the two
  syncs of `Disk`, which need a kernel.
- The pace agrees with #145's stand-in.
- `6b05991` states #145's finding honestly where it counts: two groups
  for each assist, and about twelve groups and cuts a minute. The pace
  plants are still killed. Three older sentences in `zfs.rs` and the
  fake were left behind (S3). The comment that the values test "needs" the
  conservative model is not borne out (TG5). Neither blocks.
- N1: once a publish is too old, an answer of `no-answer` (and the
  other answers the docs call "a daemon away", and `refused asked`)
  leads to `look`. On `absent`, the planner releases the hold and gives
  up, before `held_until`, while the request may still pend.
  - In a scratch run of the engine, a person's `zfs destroy` follows.
    `snap/t` is then made on a cut that does not hold `pool@t`.
  - This contradicts the module's own docs and the lead's reading of
    "whatever the daemon says".
  - The fix is a few lines. I tried it in scratch, and all 67 tests of
    the PR still pass with it.

### Findings, most severe first

#### Defects

- **N1. A publish that is too old lets its hold go while its request may
  still land.** Blocking. The code path is certain. Reaching it on a real
  daemon is plausible, not observed.
  - **Where.** `crates/skyzfs-agent/src/plan.rs:566-578`, the too-old
    arm of `onward`.
    - It runs before `match answer`.
    - Only `Reply::Pending` is withdrawn. Every other answer goes to
      `look`, and on `absent` to `give_up`, which is `Release`, then
      `Abandon(TooOld)`.
    - The answers that keep the hold (`stopped`, `ended`, `no-answer`,
      `no-upload`, which go to `away()`) are matched at `plan.rs:618`.
      The too-old arm never reaches that line.
  - **What it contradicts.**
    - The module docs at `plan.rs:64-76`: "With the daemon away, a
      publish asked keeps its hold until `held_until` … and one held
      and not asked keeps it".
    - `plan.rs:101-105`, which defines a daemon away as "no answer, or
      `stopped`, `ended`, `no-answer`, `no-upload`".
    - The lead's reading, as the docs give it: "the bound overrides
      every answer of the daemon, but not its silence".
  - **Why `no-answer` can mean a request that pends.**
    - skyzfsd answers `progress` and `withdraw` with `no-answer` when
      its uploader did not answer within the limit, or when the
      uploader's inbox is full (`bins/skyzfsd/src/control/handler.rs:212-236`).
      Its own text says: "a publish may have been taken: ask again, or
      withdraw it".
    - The uploader answers requests only between two turns
      (`crates/skyzfs-upload/src/uploader.rs:726-770`). A long step, as
      on the slow uplink that motivated L2, gives `no-answer` while the
      request pends.
    - `cuts` (answered at the store's desk) and `look` (which reads
      object storage directly, `handler.rs:196`) still answer. So the
      agent sees the daemon as here.
  - **The planner's rows** (`review_n1_too_old_with_an_answer_of_a_daemon_away`
    in `next144plan.rs`; it passes at the head and prints each row).
    Each is a publish too old, with `look` absent and `held_until` still
    640 s away:

    | Phase and answer | Planned | The docs |
    |---|---|---|
    | asked, `no-answer` | `Release` | `Wait` |
    | asked, `stopped` | `Release` | `Wait` |
    | asked, `ended` | `Release` | `Wait` |
    | asked, `no-upload` | `Release` | `Wait` |
    | held, `no-answer` (to its `publish`) | `Release` | `Wait` |
    | asked, `refused asked` (another request of the name pends) | `Release` | `Withdraw` |
    | asked, `no-answer`, not yet looked | `Look` | `Wait` |

    Not too old, the same answers keep the hold, or withdraw.
  - **End to end** (`review_n1_a_too_old_publish_lets_its_hold_go_while_its_request_pends`
    in `next144.rs`; it passes at the head; output in
    `next144-scratch.out`). The setup:
    - an idle pool;
    - the upload of a cut counted before the request, in flight for
      100 s;
    - a daemon whose uploader answers `progress` and `withdraw` with
      `no-answer` once it has accepted the request;
    - a bound of 60 s.

    What happens:
    1. At 63 s the publish is given up as `TooOld` and its hold
       released. The daemon still says `Pending`, and `held_until` is
       9 minutes away.
    2. A person runs `zfs destroy -r pool@t`. No hold refuses it now.
    3. The upload in flight ends. The cut that waits is the newest,
       counted after the destroy. The skipped cuts' maps went to it, as
       skyzfsd's driver gives a skipped cut's map to a later one
       (`driver.rs` module docs). That cut carries the request.
    4. `snap/t` is made on cut 6, which does not hold `pool@t`. This is
       check 0 of the matrices.

    The same world with the default bound keeps the hold to
    `held_until`. The request is carried by cut 4, which holds
    `pool@t`, and the publish is found published at two hours.
  - **Why it matters.**
    - L2's own case (an uplink too slow, or an uploader stuck) is where
      `no-answer` is likely. It is also where cuts are abandoned at the
      copies' bound, so the release's own cut, which would carry the
      request with `pool@t`, may not be the one uploaded.
    - It breaks the invariant the hold exists for: a cut that carries a
      request that pends holds `pool@t`.
    - Without a destroy, a request that lands after the give-up leaves
      `snap/t` published for a publish recorded as `TooOld`.
    - The 60 s bound only scales the scenario down. At two hours it
      needs a request that pends with `no-answer` at the round where
      the bound passes.
  - **Fix (tried in scratch, the diff in `next144-scratch.out`).** In
    the too-old arm:
    - withdraw on `refused asked`, as on `pending`;
    - wait on `stopped`, `ended`, `no-answer` and `no-upload` while
      `held_until` has not passed (asked), or at all (held);
    - look, then give up, only after that.

    With this change all 67 tests of the PR pass, and the rows above
    read as the docs do.
    - Whether `stopped`, `ended` and `no-upload` (an uploader that can
      carry no request) may give up at once is decision L5. If so, the
      docs must say so.
    - The table `a_publish_older_than_its_bound_is_given_up_whatever_the_daemon_says`
      has no row for any of these answers (TG1).
  - **One claim of the docs to amend.** `plan.rs:72-73` says that while
    the daemon is away no publish can be begun. That holds where `cuts`
    does not answer. Where only the uploader is silent, `cuts` answers,
    and new publishes can be begun.

- **N2. A publish renamed at its program is aged on the wall clock.** Not
  blocking. Certain.
  - **Where.** `crates/skyzfs-agent/src/engine.rs:760-765`. A member's
    `@t` of the name exists, so the publish takes the next name. The
    engine's `memory` entry, which holds `begun`, stays under the old
    name. `age_now` (`engine.rs:574-581`) then finds no `begun` and
    reads the wall clock, though the publish was begun in this process.
    This is against the module docs (`engine.rs:47-48`) and the
    description.
  - **Scenario** (`review_n3_a_publish_renamed_at_its_program_is_aged_on_the_wall_clock`
    in `next144.rs`):
    - The wall clock steps three hours forward just after the intent,
      and a person's `tank/db@t` takes the name.
    - The renamed publish is given up as `TooOld`. Without the rename
      it is published.
  - The author's plant A15 (`begun_ms = 0`) is killed by
    `a_name_a_person_took_first_is_passed_over`, for the same reason.
  - The old entry is also never removed.
  - **Fix.** Move the memory entry to the new name at the rename.

- **N3. Too old, `done` that `look` does not confirm is given up.** Not
  blocking. Certain.
  - **Where.** `plan.rs:573-577`. Not too old, the same answer stops the
    publish for a person (`Stop::DoneNotSeen`). Too old, it is
    `Release`, then `Abandon(TooOld)`, and nothing says the daemon said
    `done` (`review_n4_too_old_done_not_seen_is_given_up` in
    `next144plan.rs`).
  - **Judgement.** Nothing is lost, since the daemon says the request
    landed. But a case kept for a person becomes a silent give-up.
    Decision L6.

#### Test gaps

- **TG1. No test of a publish too old whose answer is one of a daemon
  away, or `refused asked`.** The candidate fix of N1 passes all 67
  tests.
- **TG2. F1's de-duplication within a process is untested.**
  - These plants SURVIVED:
    - F1c, with no de-duplication;
    - F1d, de-duplicated by name alone. That plant would drop a
      `SnapshotGone` after a `NoProgress` report of the same publish.
  - The property F1 is about, at least once, is pinned: F1a and F1b
    are KILLED.
- **TG3. A3 SURVIVED.** The plant reads a wall clock stepped back as
  old (`abs_diff`).
  - The table's `!too_old(BEGUN, BEGUN - 1, AGE_MAX)` steps back by
    1 ms only.
  - `!too_old(BEGUN, BEGUN - ms(AGE_MAX), AGE_MAX)` would kill it.
  - The test's own doc lists this mutant.
- **TG4. F2's behaviour is documented, not pinned.** A take-over counts
  as a stall. The first look's `review_n2` (re-run here; it passes:
  given up after 0 s) could become the test.
- **The syncs of `Disk`.** T3f and T3g (the first look's V9 and V10,
  syncs that do nothing) SURVIVED, as expected. They need a kernel, and
  the author lists them for a rig in M2.6c.
- **TG5. The values test misses a value that is not above the last**
  (found at `6b05991`). The test and the rule came with `5f766d2`, so
  they were already in the first look's head. The first look planted
  only a fixed value (P15, killed).
  - **The test.** `each_sync_task_sets_a_value_of_its_own`
    (`engine/tests.rs:484-530`). Its doc says it kills "the sync task's
    value set again".
  - **The plant.** G4, `self.clock.now_ms().max(self.assists)` in
    place of `.max(self.assists.saturating_add(1))`
    (`engine.rs:674`), SURVIVED. Plant G3, a fixed value, is killed.
  - **Why it survives** (traced in `next144-scratch.out`).
    - After the clock steps back an hour, the second publish sets the
      first's value again, 366 times, and the fake writes no group.
    - When the clock passes the last value, about an hour later, a new
      value writes a group. The publish is published, before its eighth
      stall at about 80 minutes.
    - `world.assists` keeps only the values that wrote, so the test's
      check that the values differ sees none of the repeats.
  - **A check that kills it.** Assert that no set repeated a value, for
    example that no log line says "again". Tried in scratch: it passes
    at the head, and fails with G4 ("366 sets of a value held").
  - **The cost of the miss is small.** On ZFS 2.4.1 a set of a value
    the property holds writes groups anyway (`zfs.rs` says so since
    `6b05991`).
  - **It bears on the fake's comment.** The comment
    (`engine/fake.rs:1260-1262`, and the test's doc at
    `engine/tests.rs:484-486`) says the conservative model is "the
    conservative model, which a test of the agent's values needs".
    - With a fake that writes groups for a value it holds, as ZFS 2.4.1
      does, the test still kills G3 (plant G2) and G4 (plant G5),
      through its count of values.
    - With the conservative model, G4 survives.
    - So, as the test is written, it does not need that model. The
      check above would make it kill G4 under the conservative model.

#### Suggestions (not blocking)

- **S1. The docs of `ASSIST_QUICK` understate the groups**
  (`plan.rs:147-156` at `45648ab`). **Settled by `6b05991`.**
  - At `45648ab` they said "about six groups a minute", which is the
    number of syncs.
  - On ZFS 2.4.1, #145's probe, cited in `zfs.rs`, saw each sync write
    two groups, so about twelve a minute.
  - At `6b05991` they say twelve, and the overlong line is gone. See
    (8).
- **S2. After a restart the age is the wall clock's for the whole
  process.** A step of the wall clock during that process moves the
  bound. Anchoring once (the wall clock at the first round, then the
  monotonic clock) would make it as stable as for a publish begun in
  the process. `held_until` has the same property already.
- **S3. Three sentences left behind by `6b05991`** (all at `6b05991`).
  - **`zfs.rs:270-272`.** "The property is a change of the pool's own,
    so the group it is in writes, and the sync writes that group now."
    - #145's probe saw the newest group that wrote move at the set
      itself, before the sync. In the pin, 86 moved to 93 after the
      set, then to 94 after the sync.
    - So the set writes its own group, and the sync writes the next
      one, with the history record. The doc's later sentence and
      `ASSIST_QUICK`'s doc say that correctly.
  - **`zfs.rs:281-282` and `engine/fake.rs:1260-1262`.** The first says
    a set of the value already held "writes a group too". The second
    says ZFS writes "a group as for a new value".
    - The probe's row "the same value again" moved the newest group at
      the set (94 to 97) and again at the sync (97 to 98). That is two
      groups, as for a new value.
    - "Two groups, as for a new value" would say it.
  - **`engine/fake.rs:7`.** "Each change is a sync task that writes one
    group". The assist is now the exception, at two groups.

### What was asked to be judged

#### (1) F1's fix (`28d1d64`): at least once in every window. Sound.

- **The order.** The engine pushes the report (`engine.rs:709-714`),
  then commits `Done` (`engine.rs:715-717`). `commit` stops at
  `Point { Done, before }` before it saves.
- **A stop before the push.** The record is not yet `Done`. The next
  agent sees the snapshot still broken (a lost `pool@t` does not come
  back), looks, and plans `Done` with the report again.
- **A stop after the push and before the save.** The report is made.
  The next agent plans `Done` again and reports again, so twice in
  all.
- **A stop after the save.** The report is made once. The next agent
  releases and forgets.
- **A save that fails, not a stop.** The report is pushed once. The
  plan of `Done` comes again in the same process and is de-duplicated
  there.
- **Checked by tests.**
  - The first look's scratch `review_f1` now fails with
    `stopped: [(t, SnapshotGone)]`: the stop no longer loses the
    report.
  - Plant F1a (the order at `b5063b2`) is KILLED by
    `a_snapshot_gone_report_is_made_however_the_agent_stops_after_the_loss`.
  - Plant F1b (the report dropped, the first look's V22) is KILLED by
    seven tests. The matrices' check now requires the report of each
    publish whose snapshot the agent last read as gone at the `Done`
    point (`gone_at_done`).
- **For M2.6c.** The sink must deliver a report when it is pushed.

#### (2) F2's docs, and T1 to T3

- **F2.** The docs of `stalls` (`plan.rs:412-423`) now say what is
  built: a reboot or a rewind resets the stalls, and a take-over does
  not.
- **T1.** See (1).
- **T2.** `asking_anew_checks_the_snapshot_and_its_hold_before_the_count`
  KILLS T2a, T2b and T2c (the first look's V15 to V17). It pins D5's
  order of calls.
- **T3.** `disk_writes_renames_lists_and_removes_files_in_a_directory`
  KILLS these plants:
  - T3a, `write` that does not truncate (the first look's V11);
  - T3b, `unlink` that does nothing;
  - T3c, `rename` that does nothing;
  - T3d, `create_dir` that does not refuse a directory that is there;
  - T3e, `list` that leaves a name out.

  T3f and T3g survive (see Test gaps).

#### (3) The age bound (`45648ab`)

- **The monotonic clock: yes, for a publish begun in this process.**
  - `begin` remembers `(begun_ms, mono)`.
  - `age_now` is `begun_ms` plus the monotonic clock's run since then.
  - `SystemClock::mono_ms` is `Instant`, that is `CLOCK_MONOTONIC`.
  - A4 (the wall clock in-process), A5 (`now_for`'s time) and A6 (no
    memory) are all KILLED.
  - The exception is a publish renamed at its program (N2).
- **Across a restart or a reboot.**
  - **The wall clock is used.** A `begun_ms` from a previous boot reads
    right on a clock that is right.
  - **A step forward while the agent was down.** The publish is given
    up early. That costs only the publish, but it widens N1's window
    until N1 is fixed.
  - **A step back.** The age saturates at none, so the bound comes later
    by the size of the step. The stall bound still applies.
  - **A host that suspends.** `CLOCK_MONOTONIC` does not count the
    suspend, so the bound comes later. That is conservative.
- **The record.**
  - `VERSION` stays 2. `Publish.begun_ms` follows `stalls`,
    `Why::TooOld` is appended, and the golden bytes are regenerated.
  - Version 1 is refused by its number.
  - `b5063b2`'s golden version-2 record is refused by the head
    (`Body("Tried to parse invalid utf-8")`, `next144rec.rs`). That
    rests on how the bytes happen to parse, not on the version.
    Amending in place is safe only because no agent ever wrote a
    version-2 record. That was the lead's decision, and the docs say
    so.
- **Can a too-old give-up lose a cut that `look` would still find?**
  - The give-up needs `look` to answer `absent`.
  - The planner looks again after the release and before `Abandon`:
    `Release` clears the look, and the next round looks again.
  - Where the request has ended, nothing lands later.
  - Where it may still pend, it can land later. That is N1.
- **Plants A1 to A15: 14 KILLED, A3 SURVIVED (TG3).**

#### (4) The lead's reading with the daemon away

- **No answer at all** (`cuts` or `progress` not answered). There is no
  give-up.
  - `DaemonSeen::Away` returns `away()` at `plan.rs:528`, before the
    too-old arm.
  - A publish asked keeps its hold to `held_until`. A publish held keeps
    it for as long as the daemon is away.
  - A13 is KILLED.
  - Intent and Taken are given up while the daemon is away. Nothing was
    asked there, so this is safe.
- **An answer of `no-answer`, `stopped`, `ended` or `no-upload`.** The
  publish is given up after `look` answers `absent`. This contradicts
  the reading (N1).
- **`look` refused (`store`) or not answered.** There is no give-up:
  `snap` stays empty, and the planner looks again.
- **Too old and the stall bound.**
  - Stalls are counted only at `AskAnew`. The too-old arm comes before
    `AskAnew` and `Stalled`. So once a publish is too old, no stall is
    counted and no `NoProgress` report is made.
  - Eight stalls reached before two hours are given up as `NoProgress`
    first. A12 is KILLED.
- **A daemon away for longer than two hours.**
  - A publish asked releases its hold at `held_until`, then waits.
  - A publish held keeps its hold throughout.
  - No `AskAnew` runs, so no stall is counted and nothing is reported
    at three.
  - When the daemon answers (`expired`, or `unknown` after a take-over
    or a reboot), the agent looks. The publish is then done, or given up
    as `TooOld`.

#### (5) The pace (`f6ef194`)

- **What changed.** The engine counts the syncs of the request under
  way (`Memory.assists`). The count starts again at each acceptance
  and at a restart.
  - With no qualifying cut: from `ASSIST_AFTER` after the acceptance,
    three syncs 1 s apart, then one each `ASSIST_AFTER` (10, 11, 12,
    22, 32 … s).
  - With a qualifying cut: unchanged, once each `ASSIST_AFTER` from
    twice `ASSIST_AFTER`.
  - Q1 to Q4 are all KILLED.
- **Consistency with #145's stand-in.** `publish_agent.py`'s `flow` at
  `aabde27` runs `ASSIST_BURST` (3) assists a round apart from
  `ASSIST_AFTER`, then one once `ASSIST_AFTER` has passed since the
  last. That is the agent's schedule without a qualifying cut, per
  request.
  - The stand-in has no branch for a qualifying cut.
  - **a2** (asked with G far above, so no cut ever qualifies): its pace
    figures are the agent's.
  - **a1** (asked with G five above its snapshot's): the agent, asking
    with its own G, would see its first sync's group as a qualifying
    cut and slow to the `ASSIST_AFTER` pace. But a1 judges only that
    the assists' groups carry the publish, which does not depend on
    the pace.
  - So the two agree on what the course measures.

#### (6) The rest of the new commits (at `45648ab`)

- N2 and N3 above.
- The fake's additions (`calls`, `members_read`, `synced_at`) are for
  tests only.
- **The `zfs.rs` and fake docs cite #145's probe.**
  - `zfs.rs`'s figures (two groups a sync, about 310 KiB each) match
    that pull request's description.
  - The fake still modelled one group per assist ("the `zpool sync`
    after it writes nothing more").
  - `ASSIST_QUICK` said six groups a minute (S1).
  - `6b05991` changes both. See (8).

#### (7) CI

**On `45648ab`.** Read at 12:21 and 12:28 UTC.

| Run | Workflow | State |
|---|---|---|
| 45610413056 | skyzfs-ci (push: `gate` only, since a pull request is open) | success |
| 45610426271 | ci (`lint`, `test`, `unit-arm64`, `s3-backends`, `chart-lint`) | success |
| 45610426351 | e2e | success |
| 45610426313 | k3s-smoke | success |
| 45610426329 | skyzfs-control-rig | success |
| 45610426348 | skyzfs-publish-rig | success |
| 45610426437 | coverage | success |
| 45610426459 | skyzfs-replay-rig | cancelled by the push of `6b05991` |
| 45610426363 | skyzfs-daemon-rig | cancelled by the push of `6b05991` |
| 45610426302 | skyzfs-cut-daemon | cancelled by the push of `6b05991` |

Before they were cancelled, replay-rig had four jobs green, among them
"through skyzfsd, ubuntu-24.04", and cut-daemon had 26.04-arm and
26.04-upload green.

**On `6b05991`.** Read at 12:34 UTC.

| Run | Workflow | State |
|---|---|---|
| 45613056765 | skyzfs-ci (push) | success |
| 45613066269 | ci | success |
| 45613066292 | e2e | success |
| 45613066707 | k3s-smoke | success |
| 45613066320 | skyzfs-control-rig | success (24.04, 26.04, 26.04-arm) |
| 45613066328 | skyzfs-publish-rig | success |
| 45613066296 | skyzfs-cut-daemon | success (26.04, 26.04-arm, 26.04-upload) |
| 45613066390 | coverage | running |
| 45613066462 | skyzfs-daemon-rig | running (daemon and stand-in jobs) |
| 45613066345 | skyzfs-replay-rig | running; both "write into a hole, through skyzfsd" jobs green |

- **Nothing is red.**
- The rigs drive the daemon, which this pull request changes by a
  comment only. So their tables bear on the base, not on the agent.
- **Codex** has made no review: its usage limit, as at the first look.
  There are no review threads (no reviews, and no review comments).

#### (8) `6b05991`: the fake's second group, and the docs, against #145's note (L2 of `review/m2.t5e/review.md`)

**What #145's note says** (L2, and the probe's table):

- Through the `zfs` command, an assist writes two groups: the set's,
  and its history record, which the sync writes.
- A bare `zpool sync` on an idle pool writes none.
- A set of the value already held writes groups too. The newest group
  that wrote moved at the set (94 to 97) and at the sync (97 to 98).
- The spaced pace is therefore about twelve groups and twelve cuts a
  minute, not six. On that nearly empty pool, that is about 440
  uploaded blocks a minute.
- In passist, the daemon counted 16 cuts for 16 groups that wrote.
- Whether the assist should go through libzfs (one group) is left to
  the lead and coding.

**What `6b05991` states.**

- **`ASSIST_QUICK` (`plan.rs:147-161`). Honest.**
  - It says: two groups a sync, the set's and the history record's; a
    cut for each; two groups a second unspaced; about twelve groups
    and cuts a minute spaced; about 61 syncs and 122 groups a request;
    about 440 blocks a minute on a nearly empty pool.
  - I checked the figures. At the default expiry of 600 s, the syncs
    run at 10, 11 and 12 s, then 22 to 592 s every 10 s: 3 + 58 = 61
    syncs, so 122 groups, and 6 syncs or 12 groups a minute.
  - The 440 blocks and the cut for each group are #145's figures, as
    cited.
  - "The lead accepted that cost for the command's route" is the
    lead's to confirm, not mine.
- **`Zfs::assist` (`zfs.rs:270-285`). Honest, with two stale
  sentences** (S3):
  - The new sentences match the probe: two groups at about 310 KiB
    (320,102 and 317,440 bytes in the probe), and no group for a bare
    sync.
  - So does the conclusion: a new value at each call is not needed on
    2.4.1, and is kept.
  - The first sentence ("the sync writes that group now") predates the
    probe. It contradicts the probe, where the set's own group was
    written before the sync.
  - "Writes a group too" understates the probe's two groups for a
    value already held.
- **The history record.** The probe shows the second group at the
  `zpool sync`, and none at a bare sync. That the group holds the
  set's history record is #145's reading of why. The docs cite it, as
  the note does. I see nothing against it: the CLI logs a command's
  history after the command, and that record goes to a later group.
- **The fake (`engine/fake.rs:1252-1277`). Honest, and conservative
  where it departs.**
  - A new value writes two groups at the same instant: the set's, then
    "zpool sync: the set's history record". That is the probe's count.
    The two are at one time where the kernel's are a moment apart, and
    nothing in the agent reads that interval.
  - A plain sync writes none, as on 2.4.1.
  - A value already held writes none. ZFS 2.4.1 writes groups there.
    The comment says so, and calls this the conservative model.
  - Its claim that the values test needs that model is not borne out
    (TG5).
  - The header (`fake.rs:7`) still says each change writes one group
    (S3).
- **The test's doc** (`engine/tests.rs:484-486`). Changed to the same
  wording. TG5 applies to it.

**Are the pace plants still caught?** Yes.

- Q1 to Q4 are KILLED at `6b05991` by the same tests as at `45648ab`.
- The engine test of the pace, `a_publish_no_cut_carries_is_reported_then_given_up`,
  runs with `plain_sync` set, so the assist writes no group in either
  model. It counts the calls (`synced_at`), which the change does not
  touch.
- The planner's test is pure.

**Does any test read the second group?** No.

- Plant G1 (the history record's group removed, back to one group)
  SURVIVED.
- The two groups are fidelity to the kernel, for the cost the docs
  state. No property of the agent rests on them, and the count rests on
  #145's probe, not on this suite. That is right for a fake. I do not
  count it as a gap.

**Everything else at `6b05991`.**

- All 35 plants of `45648ab` give the same verdicts, with the same
  killing tests.
- The scratch tests give the same output, and N1's run is unchanged:
  given up at 63 s, `snap/t` on cut 6 without `pool@t`.
- The candidate fix of N1 applies cleanly and gives the same 71 and 4.
- N1 to N3 stand as written.

### Gates (toolchain 1.100.0 stable; `rustup update stable` changed nothing)

All passed at `45648ab`, and again at `6b05991`:

- `cargo fmt --all -- --check`;
- `cargo clippy -p skyzfs-agent --all-targets -- -D warnings`;
- `RUSTDOCFLAGS="-D warnings" cargo doc -p skyzfs-agent --no-deps`;
- `cargo test -p skyzfs-agent`: 67 passed.

The workspace's clippy, plain and on arm64, is green in `ci` on both
heads.

### Plants (one at a time, each file restored in `finally`, 600 s limit)

Script: `next144.py`. The suite was `cargo test -p skyzfs-agent --lib`
on a clean worktree of the head. Every plant applied, and none timed
out.

- **Output at `45648ab`:** `next144.out`, 35 plants (F*, T*, A*, Q*).
- **Output at `6b05991`:** `next144-6b05991.out`, the same 35, plus
  G1 to G5.

**At `45648ab`: 35 plants, 30 KILLED and 5 SURVIVED. At `6b05991`:
40 plants, 32 KILLED and 8 SURVIVED.** The 35 have the same verdicts at
both heads.

| Plant | Result | Reading |
|---|---|---|
| F1a the report after the save of `Done` | KILLED | (1) |
| F1b the report dropped (V22) | KILLED | (1) |
| F1c no de-duplication | SURVIVED | TG2 |
| F1d de-duplicated by name alone | SURVIVED | TG2 |
| T2a-T2c D5's re-checks | KILLED | (2) |
| T3a-T3e `Disk` write, unlink, rename, create_dir, list | KILLED | (2) |
| T3f `Disk::sync_dir` does nothing (V9) | SURVIVED | needs a kernel |
| T3g `Disk::sync_file` does nothing (V10) | SURVIVED | needs a kernel |
| A1, A2, A4-A15 the age bound | KILLED | (3), (4) |
| A3 a clock stepped back read as old | SURVIVED | TG3 |
| Q1-Q4 the pace | KILLED | (5), (8) |
| G1 the fake's assist back to one group (6b05991 only) | SURVIVED | (8): no test reads the second group |
| G2 G3 with a fake that writes groups for a value it holds | KILLED | TG5 |
| G3 the agent's value fixed | KILLED | TG5 (control) |
| G4 the agent's value not above the last (`max` without `+ 1`) | SURVIVED | TG5 |
| G5 G4 with G2's fake | KILLED | TG5 |

### Scratch tests (`next144-scratch.out`)

- **Where they go.** In a scratch worktree only:
  - `review144.rs` (the first look's) and `next144.rs` are appended to
    `engine/tests.rs`;
  - `next144plan.rs` to `plan/tests.rs`;
  - `next144rec.rs` to `record/tests.rs`.
- **At `45648ab`, and again at `6b05991`, with the same output:**
  - The first look's `review_f1` and `review_d1` now fail. F1 and L2
    are fixed.
  - `review_n2` still passes, as F2's docs now say.
  - The new ones pass, which shows N1 to N3 as built.
- **With the candidate fix of N1, at both heads:**
  - all 67 tests of the PR pass;
  - the N1 scratch tests fail, as they should once the behaviour
    follows the docs.
- **At `6b05991`:**
  - plant G4 traced in the values test;
  - the check of TG5, at the head and with G4.

### Decisions for the lead

- **L5. N1's fix: which answers keep the hold of a publish too old.**
  - Recommendation:
    - `no-answer` keeps the hold to `held_until` for a publish asked,
      and keeps it while the publish is only held;
    - `refused asked` is withdrawn first.
  - For `stopped`, `ended` and `no-upload`, either:
    - the same as `no-answer`, as the module docs say now; or
    - look and give up at once, with the docs changed. These are
      uploaders that can carry no request.
- **L6. N3: too old, `done` that `look` does not see.** Either:
  - `Stop::DoneNotSeen`, as when the publish is not too old (my
    recommendation); or
  - `TooOld`, with a report.

### Files here

- `next_look.md`: this note.
- `next144.py`: the plants.
- `next144.out`: their results at `45648ab`.
- `next144-6b05991.out`: their results at `6b05991`, with G1 to G5.
- `next144.rs`, `next144plan.rs`, `next144rec.rs`: the scratch tests.
- `next144-scratch.out`: their output at both heads, with the candidate
  fix of N1 at both heads, and TG5's trace and check at `6b05991`.

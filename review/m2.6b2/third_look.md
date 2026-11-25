## Third look at #144 at 1b5e5f9 (M2.6b2)

**The head.** `1b5e5f9`, checked with `git fetch origin` and the pull
request's `head.sha` at the start and again before this note was
written. It is two commits on `6b05991` (the last look's head), both in
`crates/skyzfs-agent` only:

- `5faaabf`: N1 as first built, N2, S2, S3, TG1 to TG5;
- `1b5e5f9`: N1 and N3 rebuilt to the lead's L5 and L6.

**Which head each part was judged at.** Everything in this note was
judged at `1b5e5f9`: items (1) to (6), the plants, the scratch tests,
the gates and CI. `5faaabf` was read only as part of the diff
`6b05991..1b5e5f9`; its rigs were cancelled by the push of `1b5e5f9`.

**The base.** `zfs-topology` is at `7c9fa5a`; the pull request's base
is `59c4ad5`. Between the two: #143 and its notes, 14 files under
`tests/`, `docs/` and `.github/workflows/skyzfs-publish-rig.yml`, none
under `crates/`, `bins/` or the Cargo files. The branch merges cleanly
(`git merge-tree`). The rigs on the head built the pull request on
`59c4ad5`.

**How I worked.** I read the code and the module docs first and built
the ended arm's table myself (item 1), then the scratch tests and the
plants. I read the author's answer (issue comment 7163712364) after
that, and the L5 and L6 text as the pull request's description gives
it (its section "What the daemon's answers do to a publish too old").

**Verdict: blocked on N4, which is new and comes from the rebuild of
N1.**

- N1 is fixed as L5 states it. No cell of the ended arm releases the
  hold or gives up while a request of this agent may pend, but for
  L5's own release at `held_until`.
- N3 is built as L6 states it.
- N2, S2, S3 and TG1 to TG5 are fixed, and the new tests kill the
  plants that break each.
- The author's E2 and E3 are equivalent mutants. I confirm the
  argument and give its basis below.
- **N4.** The rebuild sends every answer that is not named to "as a
  daemon away". Among those are the uploader's own ends of a request:
  `refused rewound`, `refused state` and `refused attachment`. skyzfsd
  keeps answering them for the name. A publish too old that meets one
  is then never given up, and if it is held, its hold is kept without
  end. Nobody is told. Not too old, the same answers end it in one
  round. At `6b05991` the ended arm looked and gave up on them. A
  decision for the lead (L7) unless the fix below is taken.

### Findings, most severe first

#### Defects

- **N4. A publish too old whose request the uploader ended (`refused
  rewound`, `refused state`, `refused attachment`) is never given up;
  held, it keeps its hold without end.** Blocking, unless the lead
  decides (L7) that these answers keep the hold. Certain.
  - **Where.** `crates/skyzfs-agent/src/plan.rs:621`, the catch-all
    `_ => away()` of the ended arm (`plan.rs:608-623`). `away` is at
    `plan.rs:552-555`. The planner's table pins it as "keep"
    (`plan/tests.rs:1497`).
  - **What the daemon answers.**
    - These three are refusals that the uploader itself gives to a
      request it has accepted:
      - `set_rewound` (`skyzfs-upload/src/publish.rs:579-588`);
      - `set_state` (`publish.rs:567-570`);
      - `set_attachment` (`publish.rs:556-559`).

      Each goes through `end_where` and `end`
      (`publish.rs:603-612` and `913-930`). The outcome is kept, and
      `progress` answers it for the name (`publish.rs:715-721`).
    - A withdraw of a name that is not pending answers the same
      (`publish.rs:743-749`).
    - So the daemon keeps that answer until 1024 later outcomes have
      pushed it out (`OUTCOMES_KEPT`), or until skyzfsd restarts.
    - No request of the agent pends after one of these answers. The
      uploader ended it, and the ended arm asks nothing.
  - **What the agent does.** In the ended arm, each of these answers
    is `Wait`. A publish asked has its hold released at `held_until`,
    then waits. One held keeps its hold. Nothing withdraws, so
    `Seen.withdrawn` is never set and the publish is never given up.
    - The entry stays under way and takes one of
      `UNDER_WAY_MAX` (8) places.
    - No outcome and no report is made.
  - **Not too old**, the same answers end in one round:
    - `refused rewound` asks anew, or gives up as `Rewound` where
      `pool@t` is gone (`plan.rs:655-656`);
    - `refused state` and `refused attachment` look, then ask anew
      (`plan.rs:654`).
  - **Against the docs.** The docs say a publish too old is given up
    "once no request of it can pend" (`plan.rs:58-60`). Here none can
    pend, and it is not given up.
    - The bullet "Any other answer, or none: as a daemon away"
      (`plan.rs:81-83`) does cover these answers, so code and docs
      agree on the case.
    - L5's own list does not. The pull request gives that list as
      `stopped`, `ended`, `no-answer`, `no-upload`, an answer not
      about a publish, or silence. These three are none of those: they
      are answers about the publish, and they say it has ended.
  - **A regression of `1b5e5f9`.** At `6b05991` the arm looked, then
    gave up, on any other answer. That was N1's fault for the away
    answers, and right for these three. Plant R1 (the arm at
    `6b05991`) is killed by the table, so the table now pins the hold.
  - **Scenario** (`third144rec.rs`, shown in `third144-scratch.out`).
    The fake's uploader uploads no cut; the bound is a minute. A
    `zpool import -F` comes just before the ended arm's first
    withdraw. The daemon's mark passes the request's count, and the
    request ends `refused rewound`.
    - **`pool@t` lost by the rewind.** After the settle limit (three
      hours of the fake's time) the publish is still under way at
      `Asked`, with no outcome. `progress` still answers `refused
      rewound`.
    - **`pool@t` kept.** The same.
    - **Control, not too old.** The same rewind gives it up as
      `Rewound` at once.
    - **Held, accepted and not recorded.** The first agent stops
      before `Asked`, and the next one is too old at its first round.
      Its three holds (one per member's `@t`) are still there after
      three hours.
  - **The same arm also waits on two other kinds of answer.** Both are
    certain in the planner's table; neither was built in the fake.
    - **`refused taken` where `look` finds no `snap/t`**
      (`plan.rs:615`). `refused taken` is also an end, from a lookup.
      Not too old, it looks, then asks anew. This is the less likely
      case: `snap/T` would have to vanish after the uploader saw it.
    - **The refusals that stop for a person when not too old**
      (`name`, `manifest`, `expiry`, `version`, `malformed`, `peer`,
      `store`; `plan.rs:660-666`). In the ended arm they are a silent
      wait. Of these, only `version`, `malformed` and `peer` can
      answer a `progress` or a `withdraw`. Where one does, a held
      publish keeps its hold without end and nobody is told. Not too
      old, it would stop with `Outcome::Stopped`.
  - **Candidate fix** (scratch only; the diff is in
    `third144-scratch.out`). In the ended arm, read `refused rewound`
    and `refused state` as a withdraw answered `withdrawn`: look, then
    give up. The reason is `Rewound` where `pool@t` is gone and the
    answer was `rewound`, as in `anew(true)`.
    - With it, the suite passes but for the table's 8 "keep" rows for
      those two codes.
    - The fake runs above end `Abandoned(Rewound)` at 60 s, and
      `Abandoned(TooOld)` at 120 s for the held case. The hold is
      released in both.
    - `refused attachment` is the same case and belongs in the fix. I
      did not run it.
    - For `refused taken` with no `snap/t`, and for the protocol
      refusals, `Stop::Refused` as when not too old would tell a person
      and keep the hold. That is in line with L6.

#### Notes (not blocking)

- **N5. After an agent restart, a predecessor's `publish` can still be
  at the store's desk when its successor withdraws and gives up.** Not
  blocking. Possible by reading; not shown, since the fake has no desk.
  - **Where.** skyzfsd's handler reads the store's count at the desk
    within the waiter's limit, and only then sends the ask to the
    uploader's inbox (`bins/skyzfsd/src/control/handler.rs:149-179`).
    `progress` and `withdraw` go to the inbox at once
    (`handler.rs:180-189`).
  - **Scenario.**
    1. An agent stops inside an ask (`Ask` or `AskAnew`) while the
       desk is slow.
    2. Its successor starts within the waiter's limit (30 s) and finds
       the publish too old.
    3. It withdraws (answered `unknown`), looks (`absent`), and gives
       up, releasing the hold.
    4. Only then does the old ask reach the inbox and get accepted. It
       may land on a cut taken after the hold was released.
  - **Not new.** The not-too-old path's give-ups after a restart rest
    on the same reading, for example `unknown` with the snapshot
    broken. The flag does not cause it: it is false after a restart,
    so the successor always withdraws once.
  - Whether skyzfsd drops a request whose peer has gone was not
    checked. If it does, the window closes. A line in the docs, or a
    case for M2.6c, would do.

#### Suggestions

- **S4. `absent` among the answers that withdraw first**
  (`plan.rs:75`, and the description).
  - `absent` is an answer to `look`. skyzfsd answers `progress` and
    `withdraw` only from the uploader's `Progress`, which has no such
    answer.
  - Not too old, it counts as "not an answer about a publish" and the
    agent waits (`plan.rs:670-674`). In the ended arm it withdraws.
  - That is harmless, since a withdraw is always safe. But the docs
    read as if `progress` could answer it. Say so, or put it with the
    answers not about a publish.
- **S5. "Only a person, or M2.6c's `recover`, ends that entry"
  holds within a process** (`plan.rs:71-72`, `plan.rs:328-329`).
  - The stop is the in-process set `stopped`
    (`engine.rs:313`, `1039-1042`).
  - After an agent restart, the publish is planned again. While the
    daemon still answers `done`, it stops again.
  - Once the daemon has forgotten (1024 outcomes, or a restart of
    skyzfsd), `progress` answers `unknown`. The ended arm then
    withdraws, looks, finds no `snap/t`, and gives the publish up as
    `TooOld`. The hold is released, with no report.
  - Say "in this process", or record the stop. The lead may want this
    under L6.
- **S6. A publish too old and held, whose daemon is away, keeps its
  hold without end and nobody is told.** That is L5's decision, and
  the same as when the publish is not too old. A report, once a held
  publish passes its bound while away, would tell a person. The engine
  already has reports (`NoProgress`).

#### Test gaps

- None new beyond N4. The table pins the "keep" rows for the three
  codes, so a fix must change those rows on purpose.
- T3f and T3g (the syncs of `Disk`) still need a kernel; they are on
  M2.6c's list.
- G1 (the fake's assist back to one group) still SURVIVES. As at
  `6b05991`, no property of the agent rests on the second group, and I
  do not count it as a gap.
- E2 and E3 survive and are equivalent (item 4).

### (1) N1 as rebuilt: the ended arm in full

`third144plan.rs` runs the planner over every state the arm reads:

- the reason: too old, or eight stalls;
- the phase: held or asked;
- the time: one ms before `held_until`, or at it;
- the pool: whole and held, whole and not held, or `pool@t` gone;
- the flag `Seen.withdrawn`: set or not;
- what `look` saw: nothing yet, absent, ours, ours at another commit,
  another's, or unreadable;
- the answer: 28 of them. These are every reply of the protocol, each
  of the 17 refusal codes, and none.

That is 8064 cells. `third144table.py` sums them up
(`third144-scratch.out`).

**Every cell that releases the hold or gives up** (796 cells) is in one
of three classes, and none is elsewhere:

| Class | Cells | Can a request of this agent pend? |
|---|---|---|
| A withdraw answered `withdrawn`, `unknown` or `expired` (the flag), then `look` | 216 | No: the withdraw's own answer says so, and it is the kept answer the look and the give-up rest on (`engine.rs:666-670`). |
| `refused taken`, and `look` finds another's or an unreadable `snap/t` (`NameTaken`) | 96 | No: `taken` is an end. |
| Asked, at `held_until`, on an answer the arm reads as away (`Release`) | 484 | Possibly. This is L5's own rule: the hold goes at `held_until`, and nothing is given up. |

**By answer:**

- **The away answers** (`stopped`, `ended`, `no-answer`, `no-upload`,
  and none). Held, they wait in every cell, so the hold is kept without
  end (S6). Asked, they release at `held_until`. This is as L5 says.
- **`pending`, `refused asked` and `absent`** withdraw in every cell.
  `absent` is S4.
- **`withdrawn`, `unknown` and `expired`** withdraw while the flag is
  unset. With the flag set they look, then give up, or are done where
  the snapshot is ours.
- **`done`** looks first. With ours at the same commit it is Done,
  with a `SnapshotGone` report where the snapshot is broken. Absent,
  another commit, another's or unreadable all stop with
  `Stop(DoneNotSeen)` in every state, too old or stalled. That is N3
  done as L6 says. The commit `done` names must be the one `look` sees,
  so the author's B4 is fixed too.
- **`refused rewound`, `refused state`, `refused attachment`**, and
  every other refusal not named, read as away. That is N4.
- **`refused taken` with no `snap/t`** reads as away. That is also N4.

**Is any cell releasing or giving up while a request may pend?** No,
but for L5's release at `held_until`.

**Is any cell holding forever where a person should be told?** Yes:

- the uploader's own ends (N4);
- the protocol refusals that stop for a person when not too old (N4);
- the away answers on a held publish. That is L5's decision; S6
  suggests a report.

**Is the arm the same for a publish stalled eight times?** Yes. The
table gives the same actions for both reasons, but the reason given
when the publish is given up.

### (2) `Seen.withdrawn`

**Where it lives.**

- In `Memory` (`engine.rs:274-291`), per process.
- Set when a withdraw is answered `withdrawn`, `unknown` or `expired`
  (`engine.rs:666-669`).
- Cleared when a request is accepted (`engine.rs:882`).
- Dropped with the entry when the publish leaves
  (`engine.rs:1046`).
- Moved with the entry at a rename (`engine.rs:790-791`).

**Across an agent restart.** The flag is lost, so the successor
withdraws once more before it gives up: one withdraw more, never a
give-up without one.

- `review3_flag_lost_at_a_restart_withdraws_again` stops the agent
  just after its withdraw is answered `withdrawn`.
- The next agent withdraws again (the count of withdraws goes from 1
  to 2), then gives up as `TooOld` with no hold left.

**The reset on acceptance is right.** A withdraw answered before a new
acceptance says nothing of the new request.

- Not resetting (E3) is equivalent in outcome only because the ended
  arm never asks (item 4).
- At the head the flag is not cleared on an ask answered `no-answer`.
  That is safe for the same reason: such an ask either was never sent
  to the uploader or is ahead of every later `progress` in its inbox.

**At least once, not silent loss.**

- The flag only lets the planner look; it never skips the look. Plant
  R7 (a give-up without the look) is killed, and so is R6 (a give-up
  without the withdraw's answer).
- So a `snap/t` that landed is always seen and recorded done.
- The give-up rests on the withdraw's own answer, kept as the next
  round's answer (`engine.rs:670`), with no `progress` read between.

**With F1's order.** In the ended arm, a look that finds ours with the
snapshot broken plans Done with `SnapshotGone`. The report is pushed
before the save (`engine.rs:727-737`).

- A stop between the two leaves a successor with no flag. It
  withdraws, and the answer is the same outcome. It looks, and plans
  the same Done with the report again.
- The report is still made at least once. The flag is read only before
  the look, so it cannot cost the report.

### (3) N2, S2, S3, TG1 to TG5: fixed, and pinned

| Finding | Fixed? | Plant (killed by) |
|---|---|---|
| N2 the memory left under the old name at the rename | yes: moved in `take` (`engine.rs:790-791`) | N2a: `a_name_a_person_took_first_is_passed_over` |
| S2 the age after a restart | yes: anchored once on the wall clock, then the clock that does not step | S2a (the wall clock at each round), S2b (anchored at `begun_ms` on the monotonic clock): `a_publish_s_age_is_read_on_the_clock_that_does_not_step_in_its_process` |
| S3 the three older sentences | yes: `zfs.rs` and the fake now say two groups for each set and sync, as #145's probe found | (docs) |
| TG1 a publish too old meeting each answer | yes: the table covers every reply and every refusal code in four states | R1 to R11: the table (R10, eight stalls left out of the arm, by the engine's stall tests) |
| TG2 F1's de-duplication | yes: `a_report_is_made_once_in_a_process_for_each_kind` | F1c, F1d |
| TG3 a clock stepped back read as old | yes: `!too_old(BEGUN, BEGUN - AGE_MAX)` and `!too_old(BEGUN, 0)` | A3 |
| TG4 F2's take-over counted as a stall | yes: `a_publish_whose_daemon_is_taken_over_at_each_request_is_given_up` | TG4a (an `unknown` after an acceptance asked again with the recorded count, no stall counted): that test, and two planner tests |
| TG5 the values test misses a value not above the last | yes: the test now fails on a log line of a set to the value held | G4, G5 |

### (4) E2 and E3: equivalent, confirmed

E2 remembers a withdraw's answer whatever it is. E3 does not clear the
flag when a request is accepted. Both survive the suite (`third144.out`),
as the author says.

**I could not build a scenario where they differ from the head in
outcome, in the fake or in skyzfsd.** The argument rests on three facts.

1. **A publish in the ended arm stays there in its process.**
   - The age runs on the clock that does not step from its anchor
     (`engine.rs:587-599`).
   - Stalls change only at an ask, and the arm never asks.
2. **skyzfsd answers a withdraw of a name not pending exactly as
   `progress`** (`publish.rs:743-749`).
3. **`progress` and `withdraw` go through one ordered inbox**
   (`handler.rs:180-189`).
   - An ask this agent made earlier is either ahead of them there, or
     was never sent: an ask answered `no-answer` at the desk is not
     sent.

**What follows.**

- **E2.** The flag is set by a withdraw answered, say, `pending` (a
  publish in `Recheck`). It can only matter at a later `progress` that
  reads `withdrawn`, `unknown` or `expired`. The head sends one more
  withdraw there, which reads the same name's outcome. Nothing of the
  name can enter the uploader in between, since the arm asks nothing.
- **E3.** The flag was left from before an acceptance. That acceptance
  came before the arm was entered. The `progress` that leads to the
  look reads the outcome of the request then accepted, and so does the
  head's extra withdraw.
- **A restart of skyzfsd between the two** gives `unknown` to both.

So they differ only by one round and one withdraw.

**The one thing that could enter the uploader between** is a request
from another process: N5's predecessor at the desk. The head's extra
round narrows that window by a round and does not close it. Neither
plant is a gap.

### (5) Plants and gates at 1b5e5f9

**Plants.** Script `third144.py`, run on a clean worktree of the head.
Each plant is applied alone, against `cargo test -p skyzfs-agent --lib`
with the look's scratch tests skipped. The file is restored in
`finally`, with 600 s per plant. Every plant applied, and none timed
out. Output: `third144.out` and `third144.json`.

**56 plants: 51 KILLED, 5 SURVIVED.**

| Plants | Result | Reading |
|---|---|---|
| F1a to F1d (F1, TG2) | KILLED | F1c and F1d SURVIVED at `6b05991`; TG2 is fixed |
| T2a to T2c (D5's re-checks) | KILLED | as before |
| T3a to T3e (`Disk`) | KILLED | as before |
| T3f, T3g (`Disk`'s syncs) | SURVIVED | need a kernel |
| A1, A2, A3, A4', A5, A6', A7, A8, A9, A12, A13, A14', A15 (the age bound) | KILLED | A3 SURVIVED at `6b05991`; TG3 is fixed. A4', A6' and A14' are re-worded for `5faaabf`'s anchor. A10 and A11 are re-made as R7 and R11 for the rebuilt arm. |
| R1 to R11 (the ended arm, L5 and L6) | KILLED | item 1 |
| E1, E4, E5 (the flag set: never, on `withdrawn` alone, without `unknown`) | KILLED | item 2 |
| E2, E3 (the author's) | SURVIVED | equivalent (item 4) |
| N2a, S2a, S2b, TG4a | KILLED | item 3 |
| Q1 to Q4 (the pace) | KILLED | as before |
| G1 (the fake's assist back to one group) | SURVIVED | as at `6b05991`, not a gap |
| G2 to G5 (the sync task's value) | KILLED | G4 SURVIVED at `6b05991`; TG5 is fixed |

**Gates.** Toolchain 1.100.0 stable; `rustup update stable` changed
nothing. All pass at `1b5e5f9`:

- `cargo fmt --all -- --check`;
- `cargo clippy -p skyzfs-agent --all-targets -- -D warnings`;
- `RUSTDOCFLAGS="-D warnings" cargo doc -p skyzfs-agent --no-deps`;
- `cargo test -p skyzfs-agent`: 69 passed.

**The features.** No crate but `skyzfs-agent` itself names it in its
`Cargo.toml`. So `skyzfsd --features test-hooks` and `skyzfs-ublk-sys
--features control-test-program` do not build the agent, and I did not
run them here. CI's `lint` and `unit-arm64`, which run them, are green
on the head.

**The scratch tests** (`third144rec.rs`, `third144plan.rs`) pass at the
head and print what N4 and item 2 rest on. Their output, the table's
summary, the candidate fix of N4 and the suite with that fix are in
`third144-scratch.out`.

### (6) CI on the head, and the diff since the last look

**Runs on `1b5e5f9`: all ten completed, all green.**

| Run | Conclusion | Jobs |
|---|---|---|
| ci | success | changes, lint, unit-arm64, test, chart-lint, s3-backends |
| coverage | success | coverage (badges skipped) |
| e2e | success | e2e-tier-a |
| k3s-smoke | success | install, UDS |
| skyzfs-ci (push) | success | gate only; the rest skipped, as a pull request is open |
| skyzfs-control-rig | success | the table and its judge; 24.04, 26.04, 26.04-arm |
| skyzfs-daemon-rig | success | daemon on 24.04, 26.04, 26.04-arm; stand-in on 24.04, 26.04 |
| skyzfs-replay-rig | success | both layouts; through skyzfsd and write into a hole, 24.04 and 26.04 |
| skyzfs-cut-daemon | success | record through skyzfsd and check, 26.04, 26.04-arm, 26.04-upload |
| skyzfs-publish-rig | success | publish through skyzfsd and check, 26.04 |

**What green means here.**

- The unit tests (69, as above) are the only judge of the agent.
- No rig drives `skyzfs-agent`; that is M2.6c. The rigs judge the
  kernel and daemon paths, which `6b05991..1b5e5f9` does not touch.
- I read the conclusions. The step summaries cannot be read through
  the API, so the rigs' tables are the lead's to read.

**On `5faaabf`.** The four rigs (control, daemon, replay, publish) were
cancelled by the push of `1b5e5f9`, as a newer push does. They give no
verdict, and are superseded by the runs above. Its other six runs are
green.

**The diff `6b05991..1b5e5f9`.** Six files, all in
`crates/skyzfs-agent`: `engine.rs`, `engine/fake.rs`,
`engine/tests.rs`, `plan.rs`, `plan/tests.rs`, `zfs.rs`.

- Nothing in it goes beyond what the answer lists.
- The lines removed from the tests are the rows and docs of the old
  table, "given up whatever the daemon says", and the old doc of the
  values test. They are replaced by stricter rows.
- No assertion was weakened.
- The rename of the table's test is in the description.

### Decisions for the lead

- **L7 (N4).** In the ended arm, what should the uploader's own ends
  of a request do?
  - The answers are `refused rewound`, `refused state` and `refused
    attachment`, and also `refused taken` with no `snap/t`.
  - The choice:
    - look, then give up (or be done), as after a withdraw answered
      `withdrawn`. This is what I propose.
    - or keep the hold, as the head does, until the daemon forgets the
      name.
  - And should the protocol refusals that stop for a person when not
    too old (`version`, `malformed`, `peer` above all) stop too old as
    well?
- **Optional.**
  - N5: a sentence in the docs, or a case for M2.6c.
  - S5: whether L6's stop should outlast a restart.
  - S6: a report for a publish too old whose hold is kept while the
    daemon is away.

### Files (beside this note, in `review/m2.6b2/`)

- `third144.py`: the 56 plants. Run it with `WORKTREE=<clean worktree
  of the head> OUT=<json> python3 -I third144.py`.
- `third144.out`, `third144.json`: their results at `1b5e5f9`.
- `third144rec.rs`: the engine's scratch tests (N4 and item 2),
  appended to `engine/tests.rs` in a scratch worktree.
- `third144plan.rs`: the ended arm in full, appended to
  `plan/tests.rs`. It prints rows and asserts nothing.
- `third144table.py`: sums up those rows.
- `third144-scratch.out`: the scratch tests' output at the head, the
  table's summary, the candidate fix of N4, and the suite with it.

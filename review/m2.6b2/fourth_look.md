## Fourth look at #144 at cf53e94 (M2.6b2)

**The head.** `cf53e94`, checked with `git fetch origin` and the pull
request's `head.sha` at the start and again before this note was
written. It is one commit on `1b5e5f9` (the third look's head), in
`crates/skyzfs-agent` only: `engine.rs`, `engine/tests.rs`, `plan.rs`,
`plan/tests.rs`.

**Which head each part was judged at.** Everything in this note was
judged at `cf53e94`: items (1) to (5), the table, the scratch tests,
the plants, the gates and CI. The daemon's code read for N5 and N6
(`bins/skyzfsd`, `crates/skyzfs-upload`, `crates/skyzfs-control`) is
the base's, as it stands in the head's tree; the pull request does not
change it.

**The base.** `zfs-topology` is still at `7c9fa5a`; the pull request's
base is `59c4ad5`. Between the two are #143's tests and docs, none under
`crates/`, `bins/` or the Cargo files. The branch merges cleanly
(`git merge-tree`).

**How I worked.** I read the code first and built the ended arm's table
again, now with `report` (item 1). I read skyzfsd's channel, handler and
uploader for N5 (item 2). Then came the scratch tests and the plants
(item 3). I read the author's answer (comment 7165113710) after that.
L7 is taken as the description gives it (its section "What the daemon's
answers do to a publish too old", and the commit table); I found no
separate comment of the lead's in the thread.

**Verdict: it can merge.** No finding blocks it.

- **N4 is fixed as L7 states it.**
  - No cell of the ended arm waits on an answer of L7's list. Each looks,
    then is done, gives the name up, or releases the hold and gives up.
  - No cell releases the hold or gives up while a request of this agent
    may pend, but for L5's own release at `held_until`.
  - `report` gives `Refused(code)` in exactly the cells that give up on
    an end, and in no cell of a publish not too old.
  - The third look's N4 runs (and one more, at `BelowHold`) now end,
    the hold released and the code reported once.
- **The report is at least once.** A stop at every point of the
  give-up, then a new agent, always leaves the report made in one of the
  two processes. It is not repeated from round to round.
- **N5's claim holds for skyzfsd as built.** It rests on five facts of
  the daemon's code, listed in item 2. The docs state the main one.
- **New, not blocking:**
  - N6, a question for the lead: `refused stopped` is also an end of the
    uploader's own, and a held publish too old waits on it until skyzfsd
    restarts;
  - TG6: the order of the new report before the save is not pinned (the
    plant W9 survives the suite; a scratch test kills it);
  - TG7 and TG8: two smaller gaps in the report's de-duplication and in
    a report next to a publish done.

### Findings, most severe first

#### Defects

None.

#### Questions for the lead

- **N6. `refused stopped` ends every request the uploader holds, as
  `rewound` does; the ended arm reads it as a daemon away.** Not
  blocking: L5 and the description put `stopped` with the away answers,
  and the code follows them. Certain as to what the code does.
  - **What the daemon does.**
    - `Publishing::stop` ends every publish pending refused `stopped`,
      through the same `end_where` and `end` as `rewound`, `state` and
      `attachment` (`crates/skyzfs-upload/src/publish.rs:589-595`,
      `603-612`). The outcome is kept, and `progress` and `withdraw`
      answer it for the name (`publish.rs:715-721`, `743-749`).
    - It is called when the lease is lost for good, when a transaction
      is fenced, and when the store kept a committed cut or stopped
      (`crates/skyzfs-upload/src/uploader.rs:466-473`, `493-497`,
      `530-533`, `611-616`), between two steps. After it the uploader
      takes no more steps, and every request after is refused `stopped`
      (`publish.rs:649-651`). Nothing of the request can land after it;
      a transaction that landed before is what `look` finds.
    - `cuts` is the store's, answered at the desk, not the uploader's
      (`bins/skyzfsd/src/control/handler.rs:140-148`). So the agent
      sees the daemon here, answering `refused stopped`, until skyzfsd
      restarts. Only then does `progress` answer `unknown`.
  - **What the agent does.** In the ended arm, `stopped` is `away()`
    (`crates/skyzfs-agent/src/plan.rs:702`; `ends` puts it with "the
    daemon's silence", `plan.rs:576-603`).
    - A publish asked releases its hold at `held_until`, then stays
      under way. It has no outcome and no report, and takes one of
      `UNDER_WAY_MAX` places.
    - A publish held and not asked (accepted and not recorded) keeps
      its hold until skyzfsd restarts.
    - Not too old it is the same (`plan.rs:740`), so a held publish
      reaches the age bound and goes on waiting.
  - **Shown.** `review4_stopped_waits_and_version_gives_up`
    (`fourth144rec.rs`): from the ended arm's first withdraw on, the
    daemon answers `refused stopped`. After three hours of the fake's
    time the publish is still under way, with no outcome and no report.
    The table (item 1) has `refused stopped` at `Wait` in every state of
    a publish held.
  - **The rationale in the code is not right for this code.** `ends`
    calls `stopped` "the daemon's silence". It is an answer about the
    publish, kept by the uploader, and it says the request ended. Of the
    four codes named there, only `stopped` is so:
    - `no-upload` comes from a handler fixed when the channel opens, so
      `cuts` is refused too, and the round sees a daemon away
      (`serve.rs:915-929`);
    - `ended` can also be the channel's own runtime failing, before the
      request reached the uploader (`handler.rs:115`), so it cannot tell;
    - `no-answer` cannot tell.
  - **The choice.** Either add `stopped` to `ends` (look, then give up
    and report), or keep L5's reading and correct the word "silence" in
    `ends`' doc. Treating it as an end is safe: a stopped uploader takes
    no more steps, and a new one starts with nothing pending. S6's
    report, when M2.6c makes it, would also tell a person of this case.

#### Notes (not blocking)

- **N5. Confirmed for skyzfsd as built; the author's claim holds.** The
  daemon's code bears it out. Five facts make it hold, and the docs
  (`plan.rs:109-116`) state the first:
  1. **One connection at a time.** One thread accepts, reads, asks the
     handler and writes, then accepts the next
     (`bins/skyzfsd/src/control/server.rs:261-318`). A successor's
     connection waits in the socket's backlog, which Linux accepts in
     order.
  2. **The ask is decided before the handler returns.** For a
     `publish`, the handler reads the desk within its limit. Past the
     limit it returns `no-answer` without sending
     (`handler.rs:156`, `243-258`). Otherwise it puts the ask in the
     uploader's inbox (`handler.rs:166-174`), then waits for the answer.
  3. **The uploader takes its inbox in order, one at a time**
     (`crates/skyzfs-upload/src/uploader.rs:731-768`, `control` at
     `401-429`). An ask whose answer is no longer awaited (the handler
     gave up) is skipped there (`uploader.rs:411`). One reached just
     before is taken. Either way it is decided before what follows it
     in the inbox.
  4. **`progress` and `withdraw` go to the same inbox**
     (`handler.rs:180-189`).
  5. **One daemon per volume** (the volume's lock, `server.rs:80-91`),
     and its pending requests do not outlive it (`Publishing` is in
     memory).
  - **A publish accepted from a connection that then closes.** The
    handler does not watch its peer. It goes on, the uploader accepts
    the ask, and the write of `accepted` fails
    (`control.rs:114-124`; `server.rs:310`). The request pends as any
    other and lands, expires or is withdrawn. A peer that closed before
    its request was whole sends nothing (the read fails before the
    handler). A request left in the backlog by a peer that gave up is
    still read and answered later, in its place in the order.
  - **The assumption.** The order holds only while skyzfsd answers one
    connection at a time, as the docs now say. Nothing pins that in
    skyzfsd's tests, and its module docs (`control.rs:21-22`) do not
    say the agent relies on it. One sentence there would help.
  - **A nit in the docs.** "hands an ask to the uploader's inbox (or
    drops it, past its limit)". Past the desk's limit the channel does
    not send it. Past the uploader's limit the ask is in the inbox, and
    the uploader skips it or takes it. The order holds either way.
  - **Not covered, and not new.** A conditional PUT that the killed
    process had already sent to object storage. That is M2.5's
    publish, not the channel's order.

#### Test gaps

- **TG6. Nothing pins the new report before the save.** Plant W9 moves
  the report in `Agent::round` (`engine.rs:522-530`) after `act`. The
  suite passes with it. A stop just after the give-up's save then loses
  the report in both processes.
  - My scratch test `review4_a_stop_at_each_point_of_the_give_up_reports_at_least_once`
    kills W9. It fails at the stop "after the save of `Abandon`", with
    no report in either process (`fourth144-scratch.out`, section 3).
  - The report of `done` has such a test
    (`a_snapshot_gone_report_is_made_however_the_agent_stops_after_the_loss`);
    the new report has none.
  - The order at the head is right (item 3), so this is a gap and not a
    defect. That scratch test, or one like it, would close it.
- **TG7. The de-duplication by name alone survives (W12).** The report
  is kept once per (name, report). Plant W12 keeps it once per name. It
  survives because no engine test has a publish reported `NoProgress`
  that is then given up on an end, eight stalls and a refusal. Under W12
  that publish would lose its `Refused` report. Small; F1d is the same
  plant for the report of `done`, and is killed.
- **TG8. A report next to a publish done survives (W16).** Plant W16
  reports `Refused(code)` where `look` finds the publish made and the
  plan is `Done`. It survives: the table's rows with the publish ours
  check the action and not the report. Small.

#### Suggestions

- **S7. Two sentences of the docs.**
  - The bullet in "What is this module's reading"
    (`plan.rs:163-167`) still says that `name`, `manifest`, `expiry`,
    `version`, `malformed` and `peer` stop the publish. That holds when
    the publish is not too old. Too old, L7 gives it up. A few words
    would say so.
  - `ends`' doc says `busy` is "one may, later". skyzfsd gives `busy`
    only to a `publish` that it did not accept (`publish.rs:690-692`).
    In the ended arm it can only be an ask's answer kept for a round,
    and the next round reads `progress`. Harmless as it stands.

### (1) The ended arm in full, with the report

`fourth144plan.rs` runs the planner and `report` over every state the
arm reads. That is the third look's grid:

- the reason: too old, or eight stalls;
- the phase: held or asked;
- the time: before or at `held_until`;
- the pool: whole and held, whole and not held, or `pool@t` gone;
- the flag;
- what `look` saw: six values;
- the answer: 28 of them, every reply and each of the 17 codes, and
  none.

Beside it is the same grid for a publish not too old, the control. That
makes 12096 cells. `fourth144table.py` sums them up
(`fourth144-scratch.out`, section 1).

**Every cell that releases the hold or gives up** (2040 cells, too old
or stalled) is in one of three classes, and none is elsewhere:

| Class | Cells | Can a request of this agent pend? |
|---|---|---|
| A. A withdraw answered `withdrawn`, `unknown` or `expired` (the flag), then `look` | 216 | No: the withdraw's own answer says so. As at `1b5e5f9`. |
| B. An end of L7's list, then `look` (absent, another's, unreadable) | 1584 | No. The uploader ended the request (`rewound`, `state`, `attachment`, `taken`; `publish.rs:556-588`, `876`), or the daemon did not take it (`name`, `manifest`, `expiry`; `version`, `malformed` and `peer` are refused for each connection, `cuts` included). `store` answers only `look`. An ask kept for a round and refused so never pended: skyzfsd refuses `state`, `attachment`, `name`, `manifest` and `expiry` before it accepts, and an ask anew comes only after the last request ended. |
| C. Asked, at `held_until`, on an answer the arm reads as away | 240 | Possibly. This is L5's own rule: the hold goes at `held_until`, and nothing is given up. It was 484 at `1b5e5f9`, when the ends were among these. |

**By answer:**

- **The eleven codes of `ends`.**
  - None waits.
  - With nothing looked at yet, each looks. `Look` is never planned
    where a sight is already there, so it cannot loop on a sight.
  - Ours is `Done` (`SnapshotGone` where the snapshot is broken).
  - Another's or unreadable is `NameTaken`, after the release.
  - Absent releases the hold, then gives up:
    - as `Rewound` for `rewound` with `pool@t` gone;
    - with the snapshot's own reason where it is broken;
    - otherwise as `TooOld` or `NoProgress`.
- **`refused taken`**, with someone else's `snap/t`: released, then
  `NameTaken`. With none: released, then `TooOld` or `NoProgress`
  (`Destroyed` with `pool@t` gone). Both are reported `Refused(taken)`.
- **The protocol refusals** (`name`, `manifest`, `expiry`, `version`,
  `malformed`, `peer`, `store`) are given up the same way, the hold
  released first, and reported.
  - In skyzfsd, `version`, `malformed` and `peer` cannot come to
    `progress` while `cuts` is answered: the greeting and the peer are
    checked for each connection (`control.rs:134-150`,
    `server.rs:294-303`). So the round sees a daemon away.
  - `review4_stopped_waits_and_version_gives_up` shows that the agent
    takes the code at its word. A daemon that held the request and
    answered `version` to `progress` would have it given up and its
    hold released. That is the "a daemon that cannot read this agent's
    requests accepted none of them" of `ends`' doc, and it holds for
    skyzfsd.
- **`pending`, `refused asked`, `absent`** withdraw in every cell (S4's
  docs now say why `absent` is among them).
- **`withdrawn`, `unknown`, `expired`** are unchanged from `1b5e5f9`.
- **`done`** is unchanged: Done only with ours at the commit it names,
  otherwise `Stop(DoneNotSeen)` (L6).
- **The away answers** wait in every cell of a publish held:
  - none, `no-answer`, `ended`, `no-upload` (L5's own);
  - `busy`, and the replies that are not about a publish (skyzfsd never
    gives these to `progress`);
  - `stopped`, which is N6.

**Is any cell still waiting where nothing can pend?** Only `stopped`
(N6). With skyzfsd, `no-upload` is only seen together with `cuts`
refused, and `ended` cannot tell.

**Is any cell giving up, or releasing the hold, while a request may
pend?** No, but for L5's release at `held_until`.

**The report.**
- In all 12096 cells, `report` is `Refused(code)` exactly where the
  action is `Release` or `Abandon` and the answer is an end of L7's list
  (1584 cells), and `None` everywhere else.
- It is `None` in all 4032 cells of a publish not too old.
- Phase `Taken` gives `None` (the PR's own assertion).

**Too old against eight stalls.** The same action in every cell, but
the reason given.

### (2) N5, from the daemon's code

Given above (N5). The author's three steps (the ask decided before the
next connection, the successor behind it, one inbox) are what the code
does. I add facts 3 and 5, which the order also needs: the uploader
takes its inbox in order, skipping an ask whose answer is no longer
awaited, and nothing pending survives a restart of skyzfsd.

### (3) The new engine test, the scratch tests and the plants

**The new engine test** is
`a_publish_too_old_whose_request_the_uploader_ended_is_given_up_and_reported`.
It is the third look's scenario:

- `zpool import -F` comes just before the ended arm's first withdraw, at
  `AfterHold` and at `BelowCreated`;
- it checks the outcome, the one report, no hold left, nothing
  published and nothing under way;
- it is killed by W1, W2, W3-Rewound, W10 and W19 below.

**The scratch tests** are in `fourth144rec.rs`, and their output is in
`fourth144-scratch.out`, section 2.

- **`review4_n4_runs_of_the_third_look_now_end`.**
  - The third look's runs at `BelowCreated`, `AfterHold`, and
    `BelowHold` (new) end as `Rewound`, `TooOld` and `TooOld`.
  - The held run (accepted and not recorded, the next agent too old)
    ends `TooOld`, its three holds released.
  - Each makes the one report `Refused(rewound)`.
- **`review4_a_stop_at_each_point_of_the_give_up_reports_at_least_once`.**
  - The first agent stops at each of its points after the rewind, in
    turn (8 for `BelowCreated`, 14 for `AfterHold`, the release
    included), and a new agent settles.
  - In every case the report is in one of the two processes, the
    publish is in the record's given-up list with the right reason, and
    no hold is left.
  - At the stops before the save of `Abandon`, both processes make it.
    That is at least once, not exactly once, as for every report.
- **`review4_stopped_waits_and_version_gives_up`.** N6, and item 1's
  protocol refusals.

**The report and F1's order.**
- The engine pushes the report before `act` (`engine.rs:522-530`), so
  before the release's and the give-up's saves.
- It is kept once per (name, report) in a process. A release that fails
  and is planned again at each round does not repeat it.
- It reaches a person only through `Agent::reports()`. No binary reads
  that yet: the sink is M2.6c's, as for `SnapshotGone` and `NoProgress`.
- Once the sink exists, at least once holds only if the sink emits a
  report when it is pushed, not at the end of `settle`. That is a line
  for M2.6c's list.

**Plants.** The script is `fourth144.py`, run on a clean worktree of
the head. Each plant is applied alone, against `cargo test -p
skyzfs-agent --lib`, and the file is restored in `finally`, with 600 s
per plant. Output: `fourth144.out` and `fourth144.json`.

**90 plants: 82 KILLED, 8 SURVIVED.** Every plant applied, none timed
out, and none failed to build.

| Plants | Result | Reading |
|---|---|---|
| The third look's F1a to F1d, T2a to T2c, T3a to T3e, A1 to A15 (as there), R1 to R11 (re-anchored on `cf53e94`'s arm), E1, E4, E5, N2a, S2a, S2b, TG4a, Q1 to Q4, G2 to G5 | KILLED | as at `1b5e5f9` |
| T3f, T3g (`Disk`'s syncs) | SURVIVED | need a kernel, on M2.6c's list |
| E2, E3 (the author's) | SURVIVED | equivalent, as the third look argued; L7 does not touch the flag |
| G1 (the fake's assist back to one group) | SURVIVED | as before, not a gap |
| W1 the catch-all restored (both new arms removed: N4 at `1b5e5f9`, and the protocol refusals) | KILLED | the new engine test and the table |
| W2 the `rewound` arm removed (`pool@t` lost given up as `Destroyed`) | KILLED | the engine test and the table |
| W3: `ends` without each of its eleven codes, one at a time (moved to the arm that is false, as the match must cover every code) | all 11 KILLED | the table; W3-Rewound by the engine test too (only `report` reads it for `rewound`) |
| W4: `ends` with `stopped`, `ended`, `no-upload`, `no-answer`, `busy`, `asked`, one at a time | all 6 KILLED | the table |
| W5 an end given up without a look; W6 `rewound` given up without a look | KILLED | the table |
| W7 an end given up as too old where the snapshot is broken | KILLED | the table |
| W8 `refused taken` with no `snap/t` read as away again | KILLED | the table |
| **W9 the report after the act, so after the save** | **SURVIVED** | **TG6**; my scratch stop test kills it (`fourth144-scratch.out`, section 3) |
| W10 the engine makes no report; W11 the report at each round (no de-duplication) | KILLED | the engine test |
| **W12 the report de-duplicated by name alone** | **SURVIVED** | **TG7** |
| W13 reported when not too old | KILLED | the crash matrices (a `Refused` report is an error there) |
| W14 reported at eight stalls only when too old; W15 reported at the look too; W17 reported for any code; W18 reported at any phase | KILLED | the table |
| **W16 reported where the publish is done** | **SURVIVED** | **TG8** |
| W19 reported at the release only, not at the give-up after it | KILLED | the engine test and the table |

W1 to W8 cover the plants the brief names: the catch-all restored,
`ends` missing a code, and the look skipped. W9 is the report after the
save. The author's K1 to K15 are the same kind of plant, and my W plants
re-make them in my own words:

- K1 is W2; K2 and K4 are among W3; K2b is W1; K3 is W8;
- K5 to K7 are W4-Busy, W4-Asked and W4-NoAnswer; K8 is W5;
- K9 and K14 are W10; K10 is W15; K11 is W13; K12 is W17; K13 is W18;
  K15 is W7.

Beyond K1 to K15: W4-Stopped, W4-Ended and W4-NoUpload, W6, W11, W14,
W19 (all killed), and W9, W12 and W16 (the three that survive).

### (4) Gates

Toolchain 1.100.0 stable; `rustup update stable` and `rustup check`
say it is current. All pass at `cf53e94`:

- `cargo fmt --all -- --check`;
- `cargo clippy -p skyzfs-agent --all-targets -- -D warnings`;
- `RUSTDOCFLAGS="-D warnings" cargo doc -p skyzfs-agent --no-deps`;
- `cargo test -p skyzfs-agent`: 70 passed (69 at `1b5e5f9`, and the new
  engine test).

No crate but `skyzfs-agent` itself names the agent, so the two
features do not build it. CI's `lint` and `unit-arm64`, which run them,
are green on the head.

### (5) CI on the head, and the diff since the last look

**Runs on `cf53e94`: all ten completed, all green**, read through the
API after the last job ended.

| Run | Conclusion | Jobs |
|---|---|---|
| ci | success | changes, lint, unit-arm64, test, chart-lint, s3-backends |
| coverage | success | |
| e2e | success | |
| k3s-smoke | success | |
| skyzfs-ci (push) | success | gate only; the rest skipped, as a pull request is open |
| skyzfs-control-rig | success | the table and its judge; 24.04, 26.04, 26.04-arm |
| skyzfs-daemon-rig | success | daemon on 24.04, 26.04, 26.04-arm; stand-in on 24.04, 26.04 |
| skyzfs-replay-rig | success | both layouts; through skyzfsd and write into a hole, 24.04 and 26.04 |
| skyzfs-cut-daemon | success | record through skyzfsd and check, 26.04, 26.04-arm, 26.04-upload |
| skyzfs-publish-rig | success | publish through skyzfsd and check, 26.04 |

**What green means here.**
- The unit tests are the only judge of the agent. No rig drives
  `skyzfs-agent` yet; that is M2.6c.
- The rigs judge the kernel and daemon paths, which
  `1b5e5f9..cf53e94` does not touch.
- I read the conclusions. The rigs' tables are the lead's to read.

**The diff `1b5e5f9..cf53e94`.** Four files, all in
`crates/skyzfs-agent`.

- **Nothing in it goes beyond the answer's list:**
  - `ends`, `report` and `Report::Refused`;
  - the ended arm's two new arms;
  - the report in `Agent::round`;
  - the docs for L7, N5, S4 and S5;
  - the table's kind "end" with its report check, and the explicit
    rows;
  - the new engine test.
- **No test was weakened.**
  - The table's rows for `refused taken` change from "look, then wait"
    to "look, then release". That is the behaviour L7 changes.
  - The kind "look" now holds `done` alone, its row unchanged.
  - The crash matrices' `check` now fails on a `Refused` report as on
    `NoProgress`. That is stricter.
  - The eight-stall rows for the away codes are as they were.

### Decisions for the lead

- **N6.** Should `refused stopped` be read as an end (look, then give up
  and report), as L7 reads `rewound`? Or should it stay with the away
  answers, as L5 lists it? In the second case the doc of `ends` should
  not call it silence.
- **Optional.**
  - TG6: the scratch stop test, or one like it, as the pull request's
    test of the new report's order.
  - S7: two sentences of the docs.

### Files (beside this note, in `review/m2.6b2/`)

- `fourth144.py`: the plants (the third look's, re-anchored, and W*).
  Run it with `WORKTREE=<clean worktree of the head> OUT=<json> python3
  -I fourth144.py [name prefixes]`.
- `fourth144.out`, `fourth144.json`: their results at `cf53e94`.
- `fourth144plan.rs`: the ended arm in full with `report`, appended to
  `plan/tests.rs`. It prints rows and asserts nothing.
- `fourth144table.py`: sums up those rows.
- `fourth144rec.rs`: the engine's scratch tests, appended to
  `engine/tests.rs` in a scratch worktree.
- `fourth144-scratch.out`: the table's summary, the scratch tests'
  output at the head, and the stop test against plant W9.

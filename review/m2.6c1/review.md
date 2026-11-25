## Look at #146 (M2.6c-1): head 4ff4549

**The heads.** This look began at `cd3a67c`. The head then moved to
`4ff4549`, one commit on `cd3a67c` ("Agent: refused stopped gives a
publish up whatever its age"), after the lead ruled that `refused
stopped` ends every publish, too old or not. The verdict is on
`4ff4549`, checked with `git fetch origin` and the pull request's
`head.sha` before this note was written. Base `8663cd7`, the tip of
`zfs-topology`, which has not moved. Each part below says the head it
was judged at:

| Part | Judged at |
|---|---|
| (1) The young arm's `stopped`, `report`'s widened condition, and no give-up while a request may pend | `4ff4549` |
| (2) The record at version 2 with `Why::Stopped` appended | `4ff4549`, and the base for an agent before the variant |
| (3) The new test, the author's P5 to P10, and plants of mine | `4ff4549` (the earlier plants rerun there) |
| (4) N6 in the ended arm, TG6 to TG8, S7 | `cd3a67c`; the grid rerun at `4ff4549` changes no cell of the ended arm, so it holds there |
| (5) Gates | both heads |
| (6) CI and the diff | `4ff4549` (and `cd3a67c`) |

The daemon's code read here (`crates/skyzfs-upload`, `bins/skyzfsd`) is
the base's; the pull request does not change it.

**Verdict: it cannot merge yet. N8 blocks it.**

- **N8 (defect, blocking).** A publish whose own `publish` is refused
  `stopped` (one begun after the uploader stopped, or asked anew after
  it) is not given up. It is held, asked, looked up and released again
  and again, with no tick between, until it is too old (2 h). The
  ruling's "given up as `Why::Stopped`" holds only where the request
  pended when the uploader stopped. The fake's own livelock check
  fires. It replaces N7 of the look at `cd3a67c` (one ask a second),
  and is worse.
- **Where the request pended at the stop, the change is right.** The
  table for `stopped` not too old is as the description says, the
  report is made exactly where the ruling says, and nothing is given up
  while a request may pend (item 1).
- **The record may stay at version 2** by its own docs and the lead's
  decision on #144: no agent has run version 2. An agent before the
  variant refuses such a record as corrupt, loudly; it does not misparse
  it, and nothing is lost silently (item 2). One nit (S11).
- **The new test covers the planner** (the author's P5 to P10 are
  killed), **but no engine test stops the uploader**, which is why N8
  passes (TG12). Smaller gaps: TG13, and TG9 to TG11 from `cd3a67c`.
- **Gates pass at both heads; CI is green on `4ff4549`.**

### Findings, most severe first

#### Defects

- **N8. A publish whose `publish` is refused `stopped` livelocks:
  hold, ask, look, release, with no tick between, until it is too old.
  Blocks the merge.** Certain in the fake given skyzfsd's stop (the
  model is below); reproduced at `4ff4549`.
  - **What skyzfsd does after the stop** (`Publishing::stop`,
    `crates/skyzfs-upload/src/publish.rs:592-595`). It ends every
    request that pends, `refused stopped`, kept for its name. Every
    `publish` after is refused `stopped` at once and not kept
    (`publish.rs:649-651`). `progress` answers `pending` for a request
    that pends, else the name's kept outcome, else `unknown`
    (`publish.rs:715-721`). `cuts`, `progress`, `withdraw` and `look`
    go on answering.
  - **What the agent does, not too old** (a publish begun after the
    stop):
    1. Held, it asks; `refused stopped` comes back, and the engine keeps
       it for one round (`engine.rs:893-895`).
    2. `stopped` plans `Look` (`plan.rs:760`); the answer is kept across
       the look (`engine.rs:627`). With `snap/t` absent it plans
       `Release` (`plan.rs:760`, `518-524`), and `Refused(stopped)` is
       reported (`plan.rs:632`). The release clears the kept answer
       (`engine.rs:627-631`).
    3. The next round reads `progress`: `unknown`, since that name never
       pended in this run of skyzfsd. Held and not asked, its hold gone,
       the publish is held again (`plan.rs:745-749` to `anew`,
       `687-689`), then asked (`plan.rs:746`), and step 2 comes again.
    4. Every act of that cycle moves (`Hold`, an ask answered, a look
       with a sight, `Release`), so `settle` never sleeps a tick
       (`engine.rs:471-473`).
    - A publish whose request expired just before the stop loops the
      same way, through `expired` (`plan.rs:744`) and `AskAnew`, with a
      count saved each time.
  - **Shown** (`rec146b.rs` with `scratch146.py`'s stopped uploader;
    output in `scratch146-4ff4549-eng.out`):
    - `review6_a_publish_begun_after_the_uploader_stopped`: the fake's
      check (`engine/fake.rs:852-862`: 2000 questions with no time
      passing) fires at 0 s, after 125 holds, 124 releases, 124 asks,
      124 looks and 250 record saves. Nothing given up.
    - `review6_a_publish_asked_anew_after_the_stop`: it fires at 600 s,
      just after the stop, after 69 holds and releases, 70 asks, 207
      looks and 209 saves.
    - Where the request pended at the stop, `progress` goes on
      answering `refused stopped`, and the publish is given up as
      `Stopped` within a round, reported once, one hold and one release
      (`review6_stopped_asked_is_given_up_at_once`,
      `review6_stopped_held_is_given_up_at_once`, and two publishes in
      `review6_two_publishes_ended_by_one_stop_are_each_given_up`).
  - **The cost in a real agent.** Time passes only in the calls
    themselves, so the cycle runs as fast as they go: `zfs hold` and
    `zfs release` (each a sync task of the pool), two record saves
    (each a write, fsync, rename and directory sync), a `publish`, and a
    `look`, which reads object storage in skyzfsd
    (`bins/skyzfsd/src/control/handler.rs:190`, `196-205`). It lasts
    until `AGE_MAX` (2 h) on the clock that does not step. Then the
    ended arm reads `unknown`, withdraws, looks and gives up as
    `TooOld`. That holds for every publish the program begins while
    the uploader is stopped, up to `UNDER_WAY_MAX` (8) at once. At
    `cd3a67c` the same case asked once a second and kept its hold (N7);
    this pull request turned the wait into a release and a hold again.
  - **Why the tests pass.** Each round's plan is what the module says.
    The loop runs between rounds, through the engine. No engine test
    stops the uploader: the fake cannot, and `check` says so
    (`engine/tests.rs:318-323`) (TG12).
  - **A candidate fix, for the author to judge.** Keep the answer and
    the sight across a `Release` as across a `Look`: at `engine.rs:627`,
    `if !matches!(action, Action::Look | Action::Release)`. Then the
    round after the release still reads `refused stopped` and `absent`,
    and plans `Abandon(Stopped)`. In a scratch worktree with that one
    line (`probe146.out`):
    - both loops above end at once, as `Abandoned(Stopped)`, with one
      hold, one release and one report;
    - the head's 73 tests pass, and so do mine but the two that pin the
      loop;
    - as plant Z1 it survives the suite (item 3): no test tells it from
      the head.
    - L5 holds: nothing is asked between the release and the next
      round, so the answer carried cannot hide a request. A process
      that stops between the two runs one more cycle.
  - Whatever the fix, the docs that say `stopped` gives a young publish
    up (`plan.rs:86-88`, the comment at `756-759`, the `ends` doc at
    `592-594`, and the description) are true today only where the
    request pended at the stop.

#### Test gaps

- **TG12 (with N8). No engine test of `stopped` for a publish not too
  old.** The new test is the planner's, round by round. The fake has
  no uploader that stops; `scratch146.py` adds one in nine lines, and
  `review6_*` in `rec146b.rs` are the cases (asked, held, begun after
  the stop, asked anew after it, two publishes). A test of N8's fix
  needs at least the case begun after the stop.
- PLACEHOLDER_GAPS

#### Suggestions

- **S9 (from `cd3a67c`, still at `plan.rs:590`). The `ends` doc says
  the stopped uploader "refuses every request after it".** skyzfsd
  refuses every `publish`; `progress` and `withdraw` go on answering,
  and `progress` answers `unknown` after a refused `publish`. N8 turns
  on that. Say "every `publish`".
- **S11. `record.rs:30-32` says "Version 2 (M2.6b2) added ...
  [`Why::Stopped`]".** M2.6c-1 added it, before any agent ran version 2.
  Say so: whoever later reads the history of version 2 should not look
  for `Stopped` in M2.6b2.
- **S8 (from `cd3a67c`, unchanged at `plan.rs:165-172`).** The S7
  sentence's "that holds for a publish not too old" follows "`done`
  that `look` does not confirm", which stops a publish too old too
  (L6). "these refusals" excludes it, but a reader may not see that.

### (1) `stopped` not too old, and `report`'s widened condition (at 4ff4549)

**The grid.** `plan146.rs` (unchanged since `cd3a67c`) runs the planner
and `report` over 12096 cells:

- too old, eight stalls, or neither;
- held or asked;
- one ms before `held_until`, or at it;
- the pool whole and held, whole and not held, or `pool@t` gone;
- the flag `Seen.withdrawn`;
- what `look` saw: nothing yet, absent, ours, ours with another
  commit, another's, unreadable;
- the answer (28: every reply, each of the 17 codes, and none).

`table146b.py` compares the rows at `4ff4549` with those at `cd3a67c`
(`table146b.out`; rows printed by `grid146-4ff4549.out`'s run).

**What changed.** 144 cells, all `refused stopped`, all not too old.
No cell too old or stalled, and no other answer.

**The table for `refused stopped`, not too old.** The same when held or
asked, before or at `held_until`, and with the flag either way (no cell
differs by those).

| `look` saw | Pool whole and held | Whole, not held | `pool@t` gone |
|---|---|---|---|
| nothing yet, or `look` refused (`store`, `no-answer`) | `Look` | `Look` | `Look` |
| absent | `Release`, reported | `Abandon(Stopped)`, reported | `Abandon(Destroyed)`, reported |
| ours | `Done` | `Done` | `Done` with `SnapshotGone` |
| another's, or unreadable | `Release`, reported | `Abandon(NameTaken)`, reported | `Abandon(NameTaken)`, reported |

- A refusal of `look` is not a sight (`snap_seen`, `plan.rs:389-404`).
  The engine keeps the answer across the look, and a look with no sight
  does not move (`engine.rs:627`, `684-700`), so the agent looks again a
  tick later. It never gives up without a look that answered.
- `Done` names the commit `look` saw, as in every other arm.
- Against `refused rewound` at the same cells: 104 of 144 differ.
  `rewound`, not too old, asks anew; `stopped` gives up. As the ruling
  says.

**Releases and give-ups not too old, by answer.** The counts at the two
heads are the same for every answer but `refused stopped` (12 cells at
`cd3a67c`, the release at `held_until`; 72 at `4ff4549`).

**`report`.** Its condition (`plan.rs:632`) is now `under_way && (ended
|| *code == Code::Stopped) && gives_up && ends(*code)`. In all 12096
cells the report is `Refused(code)` exactly where the publish is
released or given up on an end too old or stalled (L7), or on `refused
stopped` at any age; `None` everywhere else. Not too old that is 72
cells, all `Refused(stopped)`. None beside a `Done`.

**No give-up while a request may still pend.** It holds, for three
reasons:

- In the planner, every young `stopped` release or give-up follows a
  look that answered (the table).
- In skyzfsd, `stopped` from `progress` or `withdraw` is the kept
  outcome of a request the stop ended; `progress` answers `pending`
  first if any request of the name pends (`publish.rs:715-721`).
  `stopped` from `publish` comes from an uploader that ended everything
  and accepts nothing more (`publish.rs:649-651`). A skyzfsd that
  restarts begins with nothing pending.
- The agent asks nothing between reading `stopped` and the release; it
  only looks. In N8's loop too, each ask is refused, so nothing pends
  at any of its releases. After a restart, the hold is placed before
  the next ask (`anew`).

**Shown in the engine** (`scratch146-4ff4549-eng.out`): asked, held,
two publishes and too old, each as the table says; N8 for the two
cases where the ask itself is refused.

**A note, not a finding.** If skyzfsd restarts between the release and
the give-up (`review6_a_restart_between_the_release_and_the_give_up`),
`progress` answers `unknown`. The publish is held again, asked anew and
published, with `Refused(stopped)` already reported. The report is
true (the request was refused, and the hold released). A person reads
it beside a publish that was made. The candidate fix of N8 does not
change this, since there the answer comes from `progress` and is not
kept.

### (2) The record stays version 2 with `Why::Stopped` appended (at 4ff4549)

**What the record's docs say** (`record.rs:22-37`):

- A record of another version is corrupt to this agent (no reader of
  other versions yet).
- "No agent has run version 2 either; from the first run of the
  program (M2.6c), any change to the record takes a new version and a
  test that decodes the version before it (the lead's decision on
  #144)."
- The golden bytes (`record/tests.rs:421-424`): "a field moved or
  added, or a variant inserted before another, changes these bytes
  without a test of reading back noticing: such a change is a new
  version".

**Has an agent run version 2?** No. No crate or program depends on
`skyzfs-agent`; only the workspace lists it. No rig drives the agent
yet. The program comes in M2.6c-3. So the change is before "the first
run of the program", and keeping version 2 follows the docs and the
lead's decision. It is the last time that holds.

**Appended, not inserted.** `Stopped` is the tenth and last variant of
`Why`; no other variant's index moves. The golden record gives every
variant an entry, so it gained one, and the bytes gained that entry
(the abandoned count 9 to 10, the body 487 to 493 bytes). Checked
(`recver146.rs`, run by `recver146.py`; `recver146.out`):

- **The base's version 2 bytes read at `4ff4549`** as `4ff4549`'s
  `golden()` less its `Stopped` entry. A record written before the
  change reads the same after it.
- **An agent before the variant** (the base, whose record code is
  `cd3a67c`'s) reads `4ff4549`'s golden bytes, and a record whose only
  abandoned entry is `Stopped`, as `Err(Body("Serde Deserialization
  Error"))`. The hash and the head pass, and postcard refuses variant
  index 9.

**What an old agent does with it.** It does not misparse it: a body
that does not decode is `Loaded::Corrupt` (`record.rs:517`). The agent
will not start (`Error::Corrupt`, `engine.rs:335`), so it publishes
nothing, and a save does not write over the record (`record.rs:22-28`,
`525`): a person judges. So a downgrade fails loudly and loses nothing silently. It
fails only once a `Stopped` entry is in the record; a record without
one still reads. Two costs, both acceptable while no agent has run:

- The person reads "corrupt body", not "a newer version": the version
  field is the same. A version 3 would read `Corrupt::Version(3)`.
- The entry stays among the 64 kept, and an old agent never writes, so
  it refuses until a person acts.

**Golden bytes and plants.** Y9 (`Stopped` inserted before `TooOld`)
and Y10 (the version 3) are both killed by the golden test (item 3).

### (3) The new test and the plants (at 4ff4549)

PLACEHOLDER_PLANTS

### (4) N6 in the ended arm, TG6 to TG8, S7 (judged at cd3a67c)

`4ff4549` changes neither the ended arm nor the engine. The grid's
ended-arm cells are the same at both heads (item 1), so what follows
holds at `4ff4549`.

**N6 is built as the lead decided** (`docs/skyzfs-impl-notes.md`,
"M2.6b2 as built": "`refused stopped` is an end like `rewound` (look,
give up, release the hold, report the code)").

- Against `cf53e94`, 288 cells changed, all `refused stopped`, all too
  old or stalled eight times (`table146.py`, `table146.out`).
- Too old or stalled, `stopped` acts as `rewound` in every cell but
  one class: `pool@t` gone and `look` absent gives `Abandon(Destroyed)`,
  not `Abandon(Rewound)`. That is right: `stopped` says nothing of a
  rewind (TG9).
- Every release or give-up of the ended arm is in one of the fourth
  look's classes:
  - A. A withdraw answered `withdrawn`, `unknown` or `expired`, then a
    look: 216 cells (216 at `cf53e94`).
  - B. An end of L7's list, `stopped` among them, then a look: 1728
    (1584).
  - C. Asked, at `held_until`, on an answer that cannot tell: 216
    (240).
- No cell waits on an end; `Look` is never planned where a sight is
  already there; `report` is `Refused(code)` in exactly the 1728 cells
  of class B.
- `ends()` agrees everywhere: the function (`plan.rs:600-616` at
  `4ff4549`), its two uses (`plan.rs:632`, `714`), its doc, the
  module's bullets, and the tests' copies. The engine has no list of
  its own.
- `stopped` is safely an end: the stop ends every request that pends,
  the uploader takes no step after (`uploader.rs:466-473`, `493-497`,
  `530-533`, `611-616`), and `look` reads object storage, not the
  uploader.

**TG6, TG7 and TG8 are closed** at `cd3a67c`: the new tests kill the
fourth look's W9, W12 and W16 and the author's P1 to P4. Their status
at `4ff4549` is in item 3.

**S7's two sentences are true** (`plan.rs:165-172` and the `ends` doc
on `busy`). skyzfsd's only `busy` is `Publishing::answer` at
`PENDING_MAX` (`publish.rs:690-692`); no outcome is `busy`, so
`progress` and `withdraw` never answer it. S8 and S9 are nits of
wording.

**N7 (at `cd3a67c`), now replaced by N8.** At `cd3a67c` a young
publish whose own ask was refused `stopped` waited as for a daemon
away, and asked again once a second until it was too old
(`rec146.rs`, `scratch146.out`). The lead's ruling answered the
question N7 put. N8 is what the ruling's code does in the same case.

### (5) Gates

Toolchain 1.100.0 stable. All pass, each in a clean worktree of the
head (`gates146.sh`):

- `cargo fmt --all -- --check`;
- `cargo clippy -p skyzfs-agent --all-targets -- -D warnings`;
- `cargo test -p skyzfs-agent`: 73 passed at `4ff4549`, 72 at
  `cd3a67c`;
- `RUSTDOCFLAGS="-D warnings" cargo doc -p skyzfs-agent --no-deps`;
- `cargo clippy --workspace --all-targets -- -D warnings`, plain, with
  `--features skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `--features skyzfs-ublk-sys/control-test-program`.

Logs: `gates146-4ff4549.log`, `gates146.log` (`cd3a67c`). I did not run
`cargo test --workspace` in the three modes: no crate but
`skyzfs-agent` names the agent, and CI's `test` and `unit-arm64` run
them (item 6).

### (6) CI on the head, and the diff (at 4ff4549)

**Runs on `4ff4549`: all five completed and green**, read through the
API after the last one ended.

| Run | Conclusion |
|---|---|
| ci (changes, lint, unit-arm64, test, chart-lint, s3-backends) | success |
| coverage | success |
| e2e | success |
| k3s-smoke | success |
| skyzfs-ci (push; gate only, the rest skipped with a pull request open) | success |

- The same five were green on `cd3a67c`.
- No rig ran: none of their paths covers `crates/skyzfs-agent`, and no
  rig drives the agent yet.
- Codex did not review either head. Its summary on `cd3a67c` says it
  hit the usage limit; nothing on `4ff4549`.

**The diff `cd3a67c..4ff4549`.** Five files, all in
`crates/skyzfs-agent`: `plan.rs` (+18 -13), `plan/tests.rs` (+91 -2),
`engine/tests.rs` (+6 -1), `record.rs` (+5 -1), `record/tests.rs`
(+6 -4).

- **Nothing in it goes beyond the description:**
  - the young arm's `stopped` line;
  - `report`'s condition;
  - three doc passages (the L7 bullet, the daemon away, `ends`), and
    `report`'s doc;
  - `Why::Stopped` with the record's doc;
  - the golden record and bytes, and `a_record_reads_back_as_written`;
  - `check` in the engine tests;
  - the new test.
- **No test was weakened.**
  - `stopped` leaves the away rows of
    `asked_with_the_daemon_away_the_hold_is_kept_until_held_until`.
    That is the behaviour the ruling changes; the new test has its rows
    with stronger checks.
  - `check`'s `Why::Stopped => false` says the fake never gives up so,
    which is true (TG12).
  - The golden bytes changed by one entry for one new variant, the
    case the golden test is for. The base's bytes still read (item 2).

### Decisions for the lead

None needed for the verdict: N8 is a defect against the ruling as the
description states it. The lead should know that until N8 is fixed the
ruling holds only for a request that pended at the stop, and that a
fix should come with an engine test of a stopped uploader (TG12).
Optional: TG9 to TG11 and TG13, S8, S9, S11.

### Files (beside this note, in `review/m2.6c1/`)

PLACEHOLDER_FILES

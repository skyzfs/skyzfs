# Fourth look at #147 (M2.6c-2) at 66b9044

The last look was at `ab8db11` (`third_look.md`). This look is at
**`66b9044`**, one commit on `ab8db11`, the pull request's head when it
began and when it ended. The base is still `8f0390f`.

I judged `git diff ab8db11...66b9044` (nine files) and the head's code
where it reaches: the planner, the engine, the adapter's `hold`,
`release` and `held`, and the fake world. The author's answer is
issue comment 7173999742.

## Verdict

**No finding blocks the merge.** TL-F1 is fixed, and the fix is safe:

- **The planner releases a part exactly where it releases a whole hold,
  and nowhere else.** An exhaustive search over 3,672,000 planner inputs
  shows it (N2 below). Everywhere else a part is planned as no hold.
  So the new releases add no release that a whole hold did not already
  have.
- **No release while a request pends, and no wrong publish.** I checked
  this on the fake world:
  - with a strict check at every agent release, across every matrix;
  - with the partial release at every point, with and without a person
    who destroys `pool@t` once it is unheld;
  - with the partial release followed by a stop of the agent at every
    later point (5,640 runs).

  None of these found a failure. A plant that does release while a
  request pends is found by the strict check.
- **The adapter's `Part` when `pool@t` goes between the reads is
  right.** It is the conservative answer, and its release works
  without `pool@t`.
- **The oracle's two new exceptions hide no wrong publish.** Check 0
  stands apart from both. In the matrix, the exceptions confirm only
  `TooOld` with a partial hold left at the give-up, and `Refused
  rewound` in ErrLow. They could be narrowed to exactly that (FL-S1).
- **11 of the author's 12 plants are killed. A12 survives**, as the
  author says, but **it is not equivalent**. One more case in an
  existing test kills it (FL-T1).
- **The deferrals are stated accurately.**

**Can it merge?** Yes, once the runs still going on `66b9044` end green.
At 21:01 UTC:

- **Green:**
  - `ci`, every job (`lint`, `test`, `unit-arm64`, `changes`,
    `chart-lint`, `s3-backends`);
  - `skyzfs-control-rig`, `skyzfs-nodeb-rig`, `skyzfs-cut-daemon`;
  - `skyzfs-publish-rig`, `skyzfs-replay-rig`;
  - `e2e`, `k3s-smoke`.

  `skyzfs-ci`'s push run only ran its gate, since the pull request is
  open.
- **Still running:** `coverage` and `skyzfs-daemon-rig` (three daemon
  jobs).

Codex has posted nothing on this head.

## Findings, most severe first

None is a defect that blocks. FL-F1 is a defect of the docs and the
fake, not of the adapter. FL-T1 and FL-T2 are gaps in the tests.

### FL-F1 (Low; certain on the code, ZFS from source). The trait, `Held::No` and the fake say `No` where `pool@t` does not exist; the adapter answers `Part` there

**What the docs and the fake say.**
- `crates/skyzfs-agent/src/zfs.rs:236`: "No member's `@t` has the tag,
  or `pool@t` does not exist".
- `zfs.rs:292`: "[`Held::No`] where `pool@t` does not exist".
- The fake follows them. It answers `No` at once when `tank@t` is
  absent (`engine/fake.rs:1292-1294`).

**What the adapter does.** `held()` (`zfs/cli.rs:598-626`) reads
`zfs holds -H -r pool@t`.
- With `-r`, `zfs holds` cuts the name at `@`. It walks the pool's
  file system and its descendants for snapshots named `t`, whether
  `pool@t` exists or not (OpenZFS 2.4.1 `zfs_main.c:6707-6719`, and
  `holds_callback` at `:6612-6621`).
- The recording `none-holds-r` shows the same: for a pool that does not
  exist, it fails on the pool's file system (`cannot open 'skznone'`),
  not on the snapshot.

So where a person has released the tag on `pool@t` and destroyed
`pool@t` alone, `tank/db@t` still has the tag. The adapter then answers
`Part`, with no member read (`cli.rs:611-612`).

**The adapter's answer is the useful one.**
- The planner sees `pool@t` gone: `Destroyed`. It releases first
  (`give_up`), because the hold is not `No`.
- `zfs release -r` also starts from the file system. It releases what
  has the tag and exits 0 (`libzfs_dataset.c:5092-5120`,
  `:5138-5161`).
- The doc's `No` would abandon at once and leave `tank/db@t` held until
  the sweep at the next start.

**The test gap.**
- No test has a part with `pool@t` gone. The plan rows build a gone
  snapshot with `hold: No` (`plan/tests.rs:170-175`).
- The fake cannot produce this case.
- Plant **M6** (a part with `pool@t` gone abandons without a release)
  survives.

**Reaching it** takes two deliberate acts of a person, a release and a
destroy, both without `-r`.

**Suggestion.**
- Make `No` mean "no member's `@t` has the tag".
- Drop the fake's early `No` for a missing root.
- Add a plan row: Taken, `pool@t` gone, `Part` gives `Release`.

### FL-T1 (Low; test gap, certain). A12 is not equivalent: the existing ask-anew test kills it with one more case

The author calls plant A12 equivalent at the fakes' points. A12 makes
`ask_anew`'s own read take a part as held: `!= Held::No` at
`engine.rs:937`.

That holds for the matrices: no point falls between the round's read
and `ask_anew`'s.

But `asking_anew_checks_the_snapshot_and_its_hold_before_the_count`
(`engine/tests.rs:1470`) calls `ask_anew` directly after a meddle. Its
meddles are `Released` (every tag) and `Replaced`. Neither tells a part
from all.

I added a scratch fourth meddle, the tag cleared on `tank/db@t` only
(`fl147_meddle.py`, `fl147_meddle.out`):

| Build | Result |
|---|---|
| Head | passes |
| A12 | **fails**: `left: Yes, right: No`, the request sent on a part |
| A12, the test as committed | passes |

**Suggestion:** add that meddle to the test.

### FL-T2 (Low; test gap, certain). The matrix cannot see a release while a request pends in a partial-release run

After `User::ReleaseChild`, nothing in the run destroys anything. So an
agent that released while a request pended would still publish on a
cut that holds `pool@t`, and every check would pass.

Plant **M5** (`pending` with a part plans `Release`) shows it:

- **The plan rows kill it** (`a_hold_on_some_members_is_released_where_the_publish_ends`).
- **The matrices do not.** In the plants run, the matrices passed.
- **The scratch strict check finds it** in 21 runs (Busy 4, Idle 4,
  Abandoned 6, ErrLow 7; `fl147_engine_M5.out`). It reads the fake
  daemon's own pending requests at each of the agent's releases.

A3 and A5 are likewise killed only by the plan rows.

**Suggestion.** Make the matrix able to see it, by either:

- a person who destroys `pool@t -r` as soon as nothing is held, armed
  once the hold is whole (the destroyer the world already has); or
- the strict check, which needs a reader of the fake daemon's pending
  requests (`fl147_fake_add.rs`).

### Nits

- `zfs/cli.rs:25`: the module docs still say "[`Zfs::held`]: `zfs holds
  -H pool@t`". Since `62df306` it is `zfs holds -H -r pool@t` and the
  members program.
- `zfs/cli/recordings.rs:545`: the comment says "false". The value is
  now `Held::No`.

## Suggestions (none blocks)

- **FL-S1. Narrow the oracle's two exceptions to what the matrix
  shows.** `engine/tests.rs:319` and `:354`.
  - **What the matrix shows.** Every `Refused` report in a
    partial-release run is `Refused(Rewound)`, in ErrLow, of a publish
    given up `TooOld` (3 runs). Every `TooOld` outcome has the hold a
    part at the agent's last release.
  - **A stricter oracle passes** in all 144 runs of
    `fl147_partial_release_tabulated` and its destroyer variant.
    - Its rules: `TooOld` only where the hold at the agent's last
      release was a part; a `Refused` report only `rewound`, in ErrLow,
      of a publish given up `TooOld`.
  - **The narrowing to take:** `Report::Refused(Code::Rewound)` for
    that publish, with its outcome `Abandoned(TooOld)`. That can be
    done without instrumenting the releases.
- **FL-S2. A person is not told why a partial hold was given up.**
  - The hold's refusal names the members that lack the tag (the
    adapter's error). The engine drops it (`engine.rs:647-649`).
  - So the person sees `TooOld`, and in ErrLow a `Refused(rewound)`
    report. `rewound` is true, but it is not the cause.
  - This goes with TL-F3's "where the words go", which is M2.6c-3's.

## Notes for the lead

### FL-N1. A correction to the third look's TL-F2, and a narrow race behind the new doc line

**The correction.** The third look said that a `zfs program` killed at
its limit "runs on in syncing context and takes what it takes". In
OpenZFS 2.4.1 it does not:

- A channel program is a `dsl_sync_task_sig` (`zcp.c:1159-1160`).
- The signal makes `zcp_eval_sig` set `zri_canceled` (`zcp.c:973-985`).
- The instruction hook then ends the program at its next check, every
  100 instructions (`zcp.c:780-788`, `:111`).
- The process waits on in the kernel until the group is synced.

So a take killed at 120 s takes what it took up to the kill (the root
first) and no more. The scale run's 964.7 s is not reached under the
agent.

The author's line (`cli.rs:74-80`) says the sync task "may go on in
the pool". That is right, for the rest of that group.

**The race.** The line goes on: "the engine's next round finds `pool@t`
and gives the publish up as `ManifestLost`". That holds where the
group has synced before the next round reads the members. The agent
returns at once (`host.rs`), and the next round comes a tick later.
If the group has not synced yet:

1. The members read sees no `pool@t`, and the round plans `Take`.
2. The new program's check fails on `pool@t` with `EEXIST`.
3. The engine reads that as someone else's snapshot (`ZfsError::Exists`,
   `engine.rs:800-811`), and the publish takes the next name.
4. The first take is left with no record, no hold and no outcome.

The same shape remains with M2.6c-2b's `zfs snapshot -r`. Its
`lzc_snapshot` waits with no signal callback, so a killed take
finishes whole in its group. A take that passes the limit (a slow or
suspended pool) could then leave a whole set behind under the old name.

- **Certainty:** source reading and timing. Not run.
- **Reaching it:** a take that passes `Limits::change`.
- **For M2.6c-2b:** decide how `Exists` at the take is told from the
  agent's own late take. Two ways:
  - read the `createtxg`, or a property the take sets, of the `@t`
    found;
  - or treat `Exists` after a timed-out take of the same name as
    `ManifestLost`.

### The cost of no repair

A part at Held or Asked goes round as follows:

1. `pending` gives `Withdraw`.
2. The daemon then answers `withdrawn`. The publish is looked up, and
   `look` finds it absent.
3. The publish is held again, and the hold fails.
4. The next round starts again from the look.

This goes on each tick until `AGE_MAX`, 2 h. Each round also saves the
record (`Holding`) and runs the hold's failure reads, the members
program and `holds -r`. That is about a look per two seconds, and a
record save per round.

This was so at `ab8db11` too. It is the price of the ruling, noted, not
a finding.

## The points asked

### 1. The oracle's two new exceptions

**What they confirm.** Both are keyed on the publish's name and on the
world's log line `partly released t`. The fake writes that line only
when the person's release removed a tag (`fake.rs:908-920`).

| | Busy | Idle | Abandoned | ErrLow |
|---|---|---|---|---|
| runs (one per point) | 34 | 34 | 36 | 40 |
| partly released | 12 | 12 | 14 | 18 |
| given up `TooOld` | 7 | 7 | 9 | 13 |
| published | 27 | 27 | 27 | 27 |
| `Refused` reports | 0 | 0 | 0 | 3, all `Rewound` |
| asked on a part (see below) | 0 | 0 | 0 | 2 |
| releases while a request pends | 0 | 0 | 0 | 0 |

The figures are the same with the destroyer (`fl147_engine_head.out`).

**Can they hide a wrong publish? No.**

- **A publish on a cut without the snapshot.** Check 0 runs before
  them and apart from them (`engine/tests.rs:267-288`). It checks
  every `snap/t` against the cut's snapshots. The `TooOld` exception
  applies only to a name with no `snap/t` (`:298-300`). The other
  exception applies only to reports.
- **A hold released while a request pends.**
  - **The exceptions are not what would hide it.** In a
    partial-release run nothing is destroyed afterwards, so the
    committed checks cannot see it in any case (FL-T2).
  - **The scratch checks find none at the head:**
    - the strict check, in every matrix through `run_under` (all 33 committed
      engine tests pass with it);
    - the tabulated runs, with the destroyer and without;
    - 5,640 runs of a partial release followed by a stop and a
      restart.
- **"Asked on a part" in ErrLow (2 runs).**
  - The person's release falls between `ask_anew`'s read and the
    request: the points of the count's commit.
  - The next round sees `pending` on a part and withdraws.
  - This race of a person after a check is the same as for a whole
    release, and it is not the agent's to close.

**Are they as narrow as claimed?** "For that publish only" is true.
The code is wider than the runs need. It allows any code, and `TooOld`
without a part at the give-up. The runs never use that width (FL-S1).

**The `Refused` report itself.**

1. In ErrLow, the person's release writes the group that the daemon
   counts as the cut with the stale group.
2. The daemon refuses `rewound`.
3. The publish cannot be held again, and at `AGE_MAX` `look` finds it
   absent.
4. It is given up `TooOld`, and `report()` reports the code. That is
   what L7 defines.

It is accurate, but it does not name the cause (FL-S2).

### 2. The adapter's `Part` when `pool@t` goes between the reads

It is right.

**Why the snapshot went.** At the first read `pool@t` had the tag, so
it could not be destroyed. For it to be gone at the members read, a
person must have released and destroyed it. Then either:

- **(a) Some members' `@t` keep the tag.** `Part` is the true answer,
  and `zfs release -r` frees them without `pool@t` (FL-F1).
- **(b) Nothing is left.** `Part` costs one release, which fails on
  ENOENT and reads as released (`cli.rs:575-578`), and one round.

**Why not the other answers.** `No` would leave (a)'s tags, and `All`
would be false. Whatever the planner does with `Part` is at least as
cautious as with `All`:

- at Taken, `Hold`, which fails, and the next round sees `pool@t`
  gone;
- `pending` is withdrawn.

**It is pinned.** Plant A10 (`No` there) is killed by the adapter's
test.

The same `Part` also comes without any race, where `pool@t` is gone
before the first read. That is FL-F1.

### 3. Release on anything but `No`

**The search (N2).** `fl147_plan_search.rs` plans every input with
each of the three holds:

- the five phases, with three stall counts;
- the snapshot whole, replaced or gone, with the premise and
  `load_guid` either way;
- the daemon away, or here with three counts and 28 answers (every
  reply, and every refusal code);
- the six sights of `snap/t`;
- five times around `held_until`, two assist times, `too_old` and
  `withdrawn`.

That is 3,672,000 inputs (`fl147_plan_search.out`).

**What holds over all of them:**

- **P1.** `plan(Part)` is `Release` wherever `plan(All)` is `Release`,
  and is `plan(No)` everywhere else. So a part is released exactly
  where a whole hold is, and is otherwise no hold.
- **P2.** `plan(Part)` is never `Held`, `Ask`, `AskAnew` or `Assist`.
- **P3.** `plan(Part)` is `Release` wherever `plan(No)` gives up or
  forgets. The one exception is the intent with no `pool@t`, where the
  hold is not read. No hold of the agent's can be there.
- **P4.** The report is the same as for the hold whose action it took.

`Part` differs from `All` in 70,120 inputs, and none breaks these.

**The search finds what it is meant to find**
(`fl147_search_controls.out`). It flags A2, A3, A5, M1 and M5.

**So the safety of every release on a part reduces to the release on a
whole hold**, which earlier looks examined. The engine checks above
confirm that on the fake world.

**Fails to release:**

- only in the fake and the docs, for `pool@t` gone (FL-F1);
- and at the intent with no `pool@t`, which cannot carry the agent's
  hold.

The end check "no snapshot carries the agent's hold" passed in every
run above.

**The ten plan rows** agree with P1, and with the rows of a whole hold
and of none for the same phase and sight:

| Row | Part | All | No |
|---|---|---|---|
| taken | `Hold` | `Held` | `Hold` |
| taken, too old | `Release` | `Release` | `Abandon` |
| taken, replaced | `Release` | `Release` | `Abandon` |
| the intent with a snapshot | `Release` | `Release` | `Abandon` |
| held, `unknown` | `Hold` | `Ask` | `Hold` |
| asked, `pending` | `Withdraw` | `Wait` or `Assist` | `Withdraw` |
| away, at `held_until` | `Release` | `Release` | `Wait` |
| away, before it | `Wait` | `Wait` | `Wait` |
| done, here | `Release` | `Release` | `Forget` |
| done, away | `Release` | `Release` | `Forget` |

**Rows that could be added.** A part with `pool@t` gone (FL-F1) is the
one that would kill a surviving plant (M6).

### 4. The deferrals

Each claim checked against the head and the design:

| Claim | At `66b9044` |
|---|---|
| TL-F3 to M2.6c-2b | "was not taken" is unchanged (`cli.rs:319-320`), as is the wording of a destroy past its limit. The design at `e6788b0` keeps a destroy (a `createtxg` that differs), where "may not have been destroyed" would go. Accurate |
| TL-S2 to M2.6c-2b | `see()` and `ask_anew` still read the members twice where the root is tagged. Accurate. A partial hold adds the hold's failure reads each round (the cost of no repair, above) |
| TL-S3 to M2.6c-2b | `partly_taken` is unchanged; it goes with the partial take. Accurate |
| TL-T2 to M2.6c-2b | `rig-holds-r-partial` and `rig-holds-r-after-partial` are not in `cli/recorded/`. Accurate |
| No repair of a partial hold | No such code. Accurate |
| `zfs hold -r` holds all or none | Right. `dsl_dataset_user_hold_check` returns at the first error other than `ENOENT`, and the sync task fails as a whole (`dsl_userhold.c:136-146`) |
| TL-F2's doc line | Accurate on "may go on". "The next round finds `pool@t`" holds unless the group is slower to sync than the next round's members read (FL-N1) |

## TL items

| Item | At `66b9044` |
|---|---|
| TL-F1 | Fixed. A1-A11 are killed, and so are M1-M5. The planner's releases on a part are exactly those of a whole hold (N2). The engine checks find no release while a request pends. Gaps: FL-T1, FL-T2 |
| TL-F2 | Doc line, as ruled. A correction and a race note: FL-N1 |
| TL-F3, TL-S2, TL-S3, TL-T2 | Deferred to M2.6c-2b by the lead's ruling, as stated |
| TL-S1 | No special rule, as ruled |
| TL-T1 | Fixed. The fake's `hold` is all or none (A8 killed). The fake's `held` on the root alone is killed (A7, F1e) |

## Gates, plants, searches

### Gates at `66b9044`

From a clean scratch checkout, with rustc 1.100.0 (`rustup update
stable`: unchanged), a target directory of my own and `debug=0`
(`fl147_gates.sh`, `fl147_gates.log`):

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -D warnings` | clean |
| `cargo test -p skyzfs-agent -p skyzfs-control` | 125 + 16 passed |
| `cargo doc -D warnings` for the two crates | clean |

The workspace's clippy and tests on amd64 are from `ci`'s `lint` and
`test` on the head (green).

### Plants (`fl147_plants.py`, `fl147_plants.out`)

**Method.**
- Each plant is applied alone, as text, to the scratch checkout.
- `cargo test -p skyzfs-agent --lib` runs with a 600 s limit.
- The file is restored in a `finally`. The checkout was clean after.

**Notes on the output.**
- A7 was first planted with a type error. The output keeps that line;
  A7 was run again alone, and the second line is the result.
- "the matrix" below is `a_persons_action_at_any_point_is_met`.

| Plant | What | Result |
|---|---|---|
| A1 | `give_up` releases only a whole hold | KILLED: the engine test, the matrix, the plan rows, the rule (R) test |
| A2 | `Done` releases only a whole hold | KILLED: the matrix, the plan rows, the rule (R) test |
| A3 | away past `held_until` releases only a whole hold | KILLED: the plan rows |
| A4 | taken: a part as held | KILLED: the plan rows |
| A5 | `pending`: a part as held | KILLED: the plan rows |
| A6 | `anew`: a part as held | KILLED: the plan rows |
| A7 | F1e, the fake's `held` on the root's tag alone | KILLED: the engine test |
| A8 | the fake's `hold` adds the tag where it is missing | KILLED: the engine test |
| A9 | the adapter: root untagged read as `No` | KILLED: the adapter's test |
| A10 | the adapter: `pool@t` gone between the reads read as `No` | KILLED: the adapter's test |
| A11 | the adapter: a part read as `All` | KILLED: the adapter's test |
| A12 | `ask_anew`'s read: a part as held | **SURVIVED**. Not equivalent: killed by a fourth meddle (FL-T1) |
| M1 | held, `unknown`: a part as held (`Ask`) | KILLED: the plan rows |
| M2 | the adapter: nothing tagged read as `Part` | KILLED: the adapter's test, the recordings |
| M3 | the fake's `held`: a part read as `No` | KILLED: the engine test, the matrix, the rule (R) test |
| M4 | taken: `Held` recorded on a part | KILLED: the plan rows |
| M5 | `pending` with a part: `Release` | KILLED: the plan rows only (FL-T2) |
| M6 | a part with `pool@t` gone: abandon, no release | **SURVIVED** (FL-F1) |
| M7 | the fake's `hold` refuses a whole hold too | SURVIVED. Equivalent in outcome: `Hold` is planned only on a hold that is not whole, and no person adds the tag |

### Searches and scratch tests

These were applied to the scratch checkout, run, and taken out in a
`finally`:

- `fl147_plan_search.rs`: N2, the planner, exhaustively (point 3).
- `fl147_search_controls.py`: N2 at the head and with A2, A3, A5, M1
  and M5.
- `fl147_engine.py`, with `fl147_engine_add.rs` and
  `fl147_fake_add.rs`:
  - the strict check in `run_under`, so in every matrix;
  - `fl147_partial_release_tabulated`: ReleaseChild, DestroyChild,
    Destroy and Retake at every point, with and without the destroyer;
  - `fl147_partial_release_then_a_stop`: a partial release at `k1`,
    then a stop at each later `k2` and a recovery.

  Results: `fl147_engine_head.out` at the head, `fl147_engine_M5.out`
  as a control.
- `fl147_meddle.py`: FL-T1.

| Search | Runs | Failures at the head | Control |
|---|---|---|---|
| N2, the planner | 3,672,000 × 3 holds | 0 | A2, A3, A5, M1, M5 each flagged |
| The strict check in every matrix | the 33 committed engine tests | 0 | M5: two engine tests fail on it |
| Partial release, tabulated (and DestroyChild, Destroy, Retake) | 144 each, × 2 | 0 (oracle, stricter oracle, strict) | M5: 21 releases while pending, per variant |
| Partial release, then a stop | 5,640 | 0 | — |

## CI on `66b9044`

Read at 21:01 UTC with `gh api .../actions/runs?head_sha=…`:

| Run | Result |
|---|---|
| `ci` | success, every job (`unit-arm64` at 21:00) |
| `coverage` | in progress |
| `skyzfs-daemon-rig` | in progress (ubuntu-24.04, 26.04, 26.04-arm) |
| `skyzfs-control-rig`, `skyzfs-nodeb-rig`, `skyzfs-cut-daemon`, `skyzfs-publish-rig`, `skyzfs-replay-rig`, `e2e`, `k3s-smoke` | success |
| `skyzfs-ci` (push) | gate only; the rest skipped, since a pull request is open |

## Decisions for the lead

- **FL-N1, for M2.6c-2b.** How the agent tells an `Exists` at the take
  from its own take that passed the limit and landed late.
- **FL-F1.** Fix the docs and the fake here, or with M2.6c-2b. It is
  small. The adapter is already right.
- **FL-S1, FL-T1, FL-T2.** Narrow the oracle, add the fourth meddle,
  and arm a destroyer in the partial-release runs: here, or in
  M2.6c-2b. None blocks.

## What this look does not cover

- Nothing ran on ZFS. The following rest on OpenZFS 2.4.1's source and
  on the recordings, not on a run:
  - what `zfs holds -r`, `zfs release -r` and `zfs hold -r` do without
    `pool@t` or with a part;
  - how a channel program is canceled.
- The workspace's tests were not run here. `ci`'s `test` ran them green
  on the head.
- The daemon rig and `coverage` had not finished.

## Files (beside this note)

| File | What it holds |
|---|---|
| `fl147_gates.sh`, `fl147_gates.log` | The gates at the head |
| `fl147_plants.py`, `fl147_plants.out` | The plants and their results |
| `fl147_plan_search.rs`, `fl147_plan_search.out` | N2 |
| `fl147_search_controls.py`, `fl147_search_controls.out` | N2's controls |
| `fl147_engine.py`, `fl147_engine_add.rs`, `fl147_fake_add.rs` | The strict check and the engine searches |
| `fl147_engine_head.out`, `fl147_engine_M5.out` | Their results at the head and with M5 |
| `fl147_meddle.py`, `fl147_meddle.out` | FL-T1 |

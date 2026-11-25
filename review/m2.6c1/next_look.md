## Next look at #146 (M2.6c-1): head 0213a6c

**The heads.** Judged at `0213a6c`, checked with `git fetch origin` and
the pull request's `head.sha` before this look began and again before
this note was written. It is `26e83c5` ("Agent: a publish refused
stopped to its own ask is given up at once", on `4ff4549`, the head of
the last look) merged with the tip of `zfs-topology`, `a4e004e`, which is
the base. The merge adds nothing of its own: `git diff a4e004e..0213a6c`
is byte for byte `git diff 8663cd7..26e83c5`, and touches
`crates/skyzfs-agent` only. The base's own changes since `8663cd7`
(notes, testing's rig files) are not judged here.

| Part | Judged at |
|---|---|
| (1) N8's fix, its cases, L5, at-least-once, a stale answer | `0213a6c` (the engine's code is `26e83c5`'s) |
| (2) The livelock search | `0213a6c`, with `4ff4549` and the base `a4e004e` as controls |
| (3) TG12, TG13, TG9 to TG11, S8, S9, S11, and the plants | `0213a6c` |
| (4) Gates, the base's doc failure | `0213a6c`; the doc gate also at `a4e004e` |
| (5) CI and the diff | `0213a6c` (diff from `4ff4549`) |

The daemon's code read here (`crates/skyzfs-upload`) is the base's; the
pull request does not change it.

**Verdict: it can merge. N8 is fixed and nothing found blocks.**

- **N8 is fixed as proposed** (`engine.rs:637`: the answer and the sight
  are kept across a `Release` as across a `Look`). Every livelock
  scenario of the last look now ends at once as `Abandoned(Stopped)`,
  with one hold, one release and one report per publish, and nothing
  asked between the release and the give-up (item 1).
- **It is right in every case I could construct.** It changes only what
  the round after a release plans from, and that round plans what the
  release's round planned, on the same answer, sight and a fresh pool
  (item 1). L5 holds, and the report is still made at least once across
  a stop at every point (a stop matrix over four cases, 198 stop
  points).
- **A kept answer can outlive a restart of skyzfsd**, but only within
  the cycle look, release, give-up (no tick), or across a `look` that
  keeps failing, as before this pull request. A publish is then given up
  as `Stopped` though the restarted uploader would take it: what the
  ruling accepts ("the next publish covers a restart"). An absence of the
  daemon drops the kept answer (a `Wait`), so a restart seen as an
  absence is picked up. Not a defect; the comment that says the answer
  "cannot have changed" overstates it (S12).
- **No other loop with no tick that skyzfsd's answers can drive**
  (item 2): a scripted search over 14460 daemon scripts finds 692 at the
  head, all present at `4ff4549` and on the base, and each needs an
  answer skyzfsd does not give. None is new; 338 of `4ff4549`'s are gone.
- **TG12, TG13, TG9 to TG11 are closed; S8, S9, S11 are done** (item
  3). The new engine tests fail with the fix reverted and with every
  other plant of the kept answer that changes behaviour; one plant that
  only costs a look survives (TG14, minor).
- **Gates pass; CI is green on the head** (17 check runs: 13 success, 4
  skipped by design); the test counts are the answer's. The workspace
  doc gate fails at the head exactly as on the base: the answer's
  `queue.rs:373` is one of 33 errors in eight crates, none in this pull
  request's files (item 4).

### Findings, most severe first

#### Defects

None.

#### Test gaps

- **TG14 (minor). The sight kept across a release is not pinned.** Plant
  N8s keeps the answer across a release but not what `look` saw: the
  round after the release then looks again before it gives up, one more
  `look` per give-up and nothing else. It survives the suite
  (`plants146c.out`): `given_up_at_the_stop` counts holds, asks and
  releases, not looks. The module docs (`engine.rs:33-41`) say the sight
  is kept. Counting looks in `with_the_uploader_stopped` (one each, two
  in the case asked anew) would pin it. Not blocking.

#### Suggestions

- **S12 (wording). "The daemon cannot have changed its answer"
  overstates it** (`engine.rs:630-636`, and the description's N8
  paragraph). Nothing is asked between the release and the next round,
  so no request can pend that the kept answer does not tell of (L5):
  that is what holds. The answer itself can change unasked: skyzfsd
  restarts (`unknown` after), or the outcome leaves its view. For
  `stopped` that changes nothing the ruling cares about (item 1, "A
  stale answer"); say that, rather than that it cannot change. Two
  nearby nits:
  - `progress` of a name whose `publish` was refused at the door does not
    always answer `unknown` (`plan.rs:598-599`, `engine.rs:634-635`): it
    answers the name's last kept outcome if there is one (`expired` in
    the case asked anew after the stop; `publish.rs:715-721`), else
    `unknown`. The fake's doc (`engine/fake.rs:564`, "a name that never
    pended") is exact.
  - `plan.rs:599` is a 100-column line in a paragraph wrapped at 72.
  - The description's N6 paragraph still says the stopped uploader
    "refuses every request after", which S9 corrected in the code.
- **S13 (optional, not for this pull request). The engine has no guard
  against a no-tick loop driven by a daemon that answers what skyzfsd
  does not** (item 2: a `publish` asked anew refused `rewound`,
  `state`, `attachment` or `asked` every time, or `progress` answering
  `refused asked`). Each cycle saves the record (`Count`) with no tick,
  and the base has the same. `settle`'s rule ("each act moves") rests on
  the daemon's consistency. A cheap guard (a tick where a round asks a
  publish already asked at the same instant) would bound it. For
  M2.6c-3, when the program meets real daemons, if at all.

### (1) N8's fix (at 0213a6c)

**How it keeps the answer.** `act` clears the publish's kept answer and
sight before every action but `Look` and, now, `Release`
(`engine.rs:637`). `see` uses a kept answer in place of `progress`
(`engine.rs:557`, `565`). So the round after a release plans from the
answer and the sight the release was planned from, with the pool read
afresh (not held now). Nothing else changed in the engine; the planner's
code is unchanged (only docs).

**The livelock scenarios of the last look, rerun** (`rec146c.rs`,
`review7_n8_scenarios_end_at_once`; output in `scen146.out`). Each ends
`Abandoned(Stopped)`, reported once under its name, no hold left,
nothing asked between the release and the give-up, within a second of
the stop:

| Case | Holds | Asks | Looks | Releases | Saves |
|---|---|---|---|---|---|
| begun after the stop | 1 | 1 | 1 | 1 | 4 |
| two begun after the stop | 2 | 2 | 2 | 2 | 8 |
| held, the ask refused | 1 | 1 | 1 | 1 | 4 |
| asked anew after the stop (expired just before it) | 1 | 2 | 2 | 1 | 6 |
| asked, the request pended at the stop | 1 | 1 | 1 | 1 | 5 |
| two asked, ended by one stop | 2 | 2 | 2 | 2 | 10 |

At `4ff4549` the first, third and fourth livelocked (the last look).

**Is it right in every case? Where `Release` is planned, and what the
round after it now does:**

- **A give-up with the hold there** (`give_up`, `plan.rs:523`): the
  round after plans the same `give_up` on the same answer and sight, the
  pool now not held: `Abandon`. Before, it read `progress` again and
  looked again. Every answer that leads to a give-up says nothing pends
  (an end, `withdrawn`/`unknown`/`expired` after a withdraw so
  answered, or the snapshot broken), and nothing is asked between, so a
  fresh `progress` could only repeat it or answer `unknown` (skyzfsd
  restarted or forgot). With `unknown` the old code gave up too, but
  for one case: a young publish, pool whole, refused `stopped`, which it
  held and asked again. That case is the ruling's.
- **The daemon away at `held_until`** (`away()`, `plan.rs:651`): with
  the daemon away the round after is away too and plans `Wait`, which
  clears. With an answer kept from a withdraw (`no-answer`, `ended`,
  `no-upload`), the round after plans `Wait` on it (not held), which
  clears, and `progress` is read a tick later. One tick later than
  before, nothing else.
- **Done** (`plan.rs:447`): `Done` cleared the memory before; nothing is
  kept.
- **A release that fails** (`engine.rs:733`, `Moved::No`): the kept
  answer outlives the tick, and the release is planned again on it.
  Before, `progress` was read again. Only a pool whose release keeps
  failing while `held` reads true sees the difference.

**L5.** No request is sent between a release and the next round: the
release sends none, and the next round's `see` asks `cuts` only, since
the answer is kept. Counted in every scenario above, in the restart and
absence cases below, and in the stop matrix: zero. A give-up on a kept
`stopped` from the door is safe on its own too: skyzfsd answers
`stopped` at the door before anything else (`publish.rs:649-651`) only
after `stop` ended every request that pended, and accepts none after;
a restarted skyzfsd begins with none.

**At least once** (`review7_n8_stop_matrix`). For four cases (begun after
the stop; held, the ask refused; asked anew after the stop; asked, the
request pended), the agent is stopped at each of its points from the
manifest saved on (18, 18, 142 and 20 points), and a second agent with
nothing in memory settles. Every run ends with the publish given up as
`Stopped` in the record, `refused stopped` reported in one process or
both (both where the stop fell between the report and the give-up's
save), and no hold left. The second agent runs one more hold, ask, look
and release, as the module docs say.

**A stale answer: skyzfsd restarts** (`review7_a_restart_while_the_answer_is_kept`,
`review7_away_while_the_answer_is_kept`,
`review7_pended_then_restart_after_the_release`):

| skyzfsd restarts (a take-over: its uploader not stopped) | Ends |
|---|---|
| after the refused ask, before the look | `Abandoned(Stopped)`, at once |
| after the look, before the release | `Abandoned(Stopped)`, at once |
| after the release, before the give-up | `Abandoned(Stopped)`, at once (at `4ff4549`: held and asked again, and made) |
| away a minute after the refused ask, the look or the release, not restarted | `Abandoned(Stopped)` when it answers again |
| the same, restarted while away | `Published`, the next ask accepted (after the release: with `Refused(stopped)` already reported) |
| the request pended at the stop; restarted after the release | `Published`, `Refused(stopped)` reported beside it (the last look's note, unchanged) |

- **What invalidates the kept answer:** any action but `Look` and
  `Release` (a `Wait`, a hold, an ask, a withdraw, a sync, a give-up, a
  stop for a person); the publish leaving the record; a restart of the
  agent. A daemon away plans `Wait`, but for a publish asked and still
  held at `held_until`, which it releases (the answer kept) and then
  waits; so an absence drops it within a round or two.
- **What does not:** time passing in a `look` that is not answered or a
  release that fails (each `Moved::No`), and a round in which the pool
  is not seen (no act). A restart of skyzfsd is not noticed by itself:
  `cuts` is read, and its attachment and state are not compared.
- **Does the agent ask again after a restart?** Where the restart falls
  in the cycle look, release, give-up (no tick, a few calls long): no,
  the publish is given up as `Stopped` though the new uploader would
  take it. Where a `look` keeps failing (`refused store`) the window
  widens to as long as that lasts; that was so at `4ff4549` too (the
  answer was kept across a look since M2.6b2). Where the restart shows
  as an absence: yes, the publish is asked again and made. This is the
  ruling's trade ("waiting would only hold `pool@t`... the next publish
  covers a restart"), not a defect.

### (2) The livelock search (at 0213a6c; controls 4ff4549 and a4e004e)

`rec146s.rs` runs the engine on the fake's pool, clock and `cuts`, with
a scripted daemon for `publish`, `progress`, `withdraw` and `look` that
answers as skyzfsd's `Publishing` does (`publish.rs:640-760`): an
identical request that pends is accepted again, another refused `asked`;
otherwise the script's first answer to the name's first ask and its
later answer to each after; `accepted` makes the request pend and
forgets the name's outcome; a refusal at the door is not kept; the
request ends as the script's fate at the next `progress` or `withdraw`;
`progress` answers `pending`, else the kept outcome, else `unknown`;
`withdraw` ends a request that pends as `withdrawn`; `look` answers the
script's sight (absent, ours, another's, unreadable, `refused store`,
no answer).

Scripts: first answer and later answer each of `accepted`, no answer,
and the 17 refusals; fate each of `pending`, `done`, `expired`,
`withdrawn`, `unknown` and the 17 refusals (only where an ask is
accepted); six sights; young (90 s of the world) or with a 20 s age
bound (120 s); one publish, and two for the scripts whose answers are
`accepted` or `stopped`. 14460 runs a head. A livelock is 300 points of
the engine with no time passing (the fake's own check is caught too).
Run by `search146.py`; summary `search146.out` (`search146_sum.py`).

| Head | Livelocks |
|---|---|
| base `a4e004e` | 938 |
| `4ff4549` | 1030 (N8's among them: the search finds it) |
| `0213a6c` | 692 |

- **At the head and not at `4ff4549` or the base: none.** 338 of
  `4ff4549`'s are gone at the head, 158 of them with `stopped` as an
  answer to the ask.
- **Each of the 692 left needs an answer skyzfsd does not give**
  (`search146.out` lists them by class, with a trail each):
  - `progress` or `withdraw` answering `refused asked` (an answer of the
    door only), 280 alone and more with the others: withdraw, withdraw...;
  - every ask anew refused `asked` with nothing pending (122),
    `rewound` (122), `state`, `attachment` or `taken` (32 each): ask,
    look, count, ask... An ask anew carries a count read from `cuts` just
    before; the same skyzfsd does not refuse it so for good, and the
    door never answers `taken`.
  - With the scripts limited to what skyzfsd answers, the head has
    none. That is S13.
- **The search's limits.** One daemon whose answers do not change with
  time, one or two publishes, no restart of either side, no failure of
  `zfs`. The restarts and stops are in item 1.

### (3) TG12, TG13, TG9 to TG11, S8, S9, S11, and the plants (at 0213a6c)

**TG12 is closed.** The fake has an uploader that stops as skyzfsd's
does (`World::stop_uploader`, `engine/fake.rs:566`; the refusal at the
door first, `engine/fake.rs:1091`, as `publish.rs:649`): every request
that pends ends `refused stopped`, kept; every `publish` after is
refused at once and not kept; `progress` and `withdraw` go on; a
take-over or reboot starts a daemon not stopped, and an export and
import leaves it stopped (as skyzfsd, which an import does not
restart). Two engine tests
(`a_stopped_uploader_gives_each_publish_up_at_once`,
`a_publish_asked_anew_after_the_uploader_stopped_is_given_up`,
`engine/tests.rs:2098`, `2136`) cover asked, held, begun after the stop,
two asked, two begun after, and asked anew. Each checks the outcome, one
report per name, the exact holds, asks and releases, no hold left, and
the end within 10 s of the stop. With the fix reverted both hit the
fake's livelock check.

**TG13, TG9, TG11 are closed**: seven rows in
`refused_stopped_gives_a_publish_up_whatever_its_age`
(`plan/tests.rs:588-633`), each also checking `report`. **TG10** (X12b) is
killed by the two-publish cases.

**S8, S9, S11 are done** as the answer says (`plan.rs:169-177`,
`592-601`, `record.rs:30-34`). S12 has two nits of S9's sentence.

**The plants** (`plants146c.py`, `plants146c.out`): each applied alone in
a clean worktree of `0213a6c`, then `cargo test -p skyzfs-agent --lib`,
then the file restored. The 118 plants of `plants146b.py` (read from that
file, less Z1, which is now the head) and this look's nine of the fix.

| Plants | Run | Killed | Survived |
|---|---|---|---|
| `plants146b.py`'s, less Z1 | 118 | 111 | 7: T3f, T3g, R3, E2, E3, G1, X9b |
| this look's, of the fix (N8*) | 9 | 6 | 3: N8s, N8g, N8h |

- **Against `4ff4549`** (`plants146b.out`): every one of the 118 has the
  same verdict but N6e, X12b, X16b, Y2 and Y3, survivors there and
  killed now, which is TG9, TG10, TG11 and TG13 closed as the answer
  says (N6e, X16b, Y2 and Y3 by the new rows of
  `refused_stopped_gives_a_publish_up_whatever_its_age`; X12b by
  `a_stopped_uploader_gives_each_publish_up_at_once`, two publishes).
  The seven left are the last look's known survivors: T3f, T3g (the
  `Disk` syncs), E2, E3, G1 (earlier looks), R3 (dead code) and X9b (the
  model's limit).
- **The `stopped` reason not reported or not given:** W3-Stopped, N6f,
  P6, P8, Y5 and W10 are killed, each by the new engine tests, the new
  rows, or both.
- **This look's nine:**

| Plant | Verdict |
|---|---|
| N8r the fix reverted (the answer and the sight cleared at a release, as at `4ff4549`) | killed: both new engine tests (the fake's livelock check) |
| N8a the sight kept across a release, the answer not | killed: both new engine tests |
| N8s the answer kept across a release, the sight not (one more look) | **survived** (TG14) |
| N8d the answer dropped at the look instead (kept across a release only) | killed: both new engine tests |
| N8e the kept answer read once (taken by `see`) | killed: both new engine tests |
| N8f the answer and the sight never cleared | killed: seven engine tests (the restart matrices among them) |
| N8g the answer and the sight kept across a hold too | survived: not a defect (below) |
| N8l the look's result not kept at all | killed: 29 tests |
| N8h kept across a release only after `stopped` (a narrower fix) | survived: an alternative fix (below) |

**Do the tests cover the fix?** Yes, where it changes behaviour: every
plant that makes the agent ask again after a release, or drop the
answer before the give-up, or not report, is killed by the new engine
tests. N8s (one more look) survives (TG14). N8g and N8h are not
defects: N8g keeps the answer across a hold too, and the action after a
hold clears it; N8h keeps it across a release only after `stopped`,
which gives the same plans in every test (the other answers end the
same with one more `progress` or `look`).

### (4) Gates (at 0213a6c) and the base's doc failure

`rustup check`: stable 1.100.0 (2026-11-09), up to date. Run by
`gates146c.sh` in a clean worktree of `0213a6c`, with a shared target
directory and debug info off (`CARGO_PROFILE_DEV_DEBUG=0`,
`CARGO_PROFILE_TEST_DEBUG=0`; the container's disk was nearly full);
summed in `gates146c.log`.

| Gate | At `0213a6c` |
|---|---|
| `cargo fmt --all -- --check` | pass |
| `cargo clippy -p skyzfs-agent --all-targets -- -D warnings` | pass |
| `cargo test -p skyzfs-agent` | 75 passed |
| `cargo doc -p skyzfs-agent --no-deps`, `-D warnings` | pass |
| `cargo clippy --workspace --all-targets -- -D warnings`: plain, `test-hooks`, `control-test-program` | pass, pass, pass |
| `cargo test --workspace`, plain | 1816 passed, 0 failed, 16 ignored |
| the same with `skyzfsd/test-hooks,skyzfs-local/test-hooks` | 1858 passed, 0 failed, 16 ignored |
| the same with `skyzfs-ublk-sys/control-test-program` | 1840 passed, 0 failed, 18 ignored |
| `cargo doc --workspace --no-deps`, `-D warnings` | fails, as on the base (below) |

The counts are the answer's (1816, 1858, 1840).

**The base's doc failure: true, and not the only one.** At `a4e004e`
the workspace doc gate, run as `gates146c.sh` runs it, stops at
`crates/skyzfs-ublk-sys/src/queue.rs:373` (the public `start` links to
the private `own_control_ring`), as the answer says. But cargo stops at
the first crate that fails, and which one that is depends on the order
of the build: at the head the same command stopped at `skyzfs-local`
first (`inplace/intent.rs:141`). With `--keep-going` the base and the
head fail alike: 33 errors in eight crates (`skyzfs-ublk-sys`,
`skyzfs-local`, `skyzfsd`, `skynbd-refs`, `skynbd-engine`,
`skynbd-agent`, `skynbd-agent-cli`, `skynbdctl`), the same locations at
both heads, all in files this pull request does not touch;
`skyzfs-agent` documents cleanly. No workflow runs `cargo doc`, which is
how they went unseen. Not this pull request's to fix.

### (5) CI on the head, and the diff (at 0213a6c)

**CI on `0213a6c`: green**, read through the API after the last run
ended. Five runs, 17 check runs: 13 success and 4 skipped.

| Run | Conclusion |
|---|---|
| ci (changes, lint, test, unit-arm64, s3-backends, chart-lint) | success |
| coverage | success |
| e2e | success |
| k3s-smoke | success |
| skyzfs-ci (push; gate only, the rest skipped with a pull request open) | success |

- The skipped four: `skyzfs-ci`'s lint, test and unit-arm64 (a pull
  request is open), and coverage's badge step. The answer's "17 of 17"
  is all 17 check runs green, four of them skipped by design.
- No rig ran: the pull request's diff from its base touches only
  `crates/skyzfs-agent`, which no rig's paths cover. The merge brought
  testing's rig files, but as the base's, not as this pull request's.
- Codex did not review the head (its last summary, on `cd3a67c`, hit the
  usage limit).

**The diff `4ff4549..26e83c5`** (six files, all in
`crates/skyzfs-agent`): `engine.rs` (+15 -5), `engine/fake.rs` (+19),
`engine/tests.rs` (+169 -1), `plan.rs` (+19 -10), `plan/tests.rs` (+47),
`record.rs` (+3 -2).

- **Nothing beyond the answer's list:** the one line of the fix and its
  comment, the module docs of `engine.rs` and `plan.rs`, the `ends` doc
  and the `stopped` arm's comment (S9, N8), S8's sentence, S11's
  sentence, the fake's stopped uploader, the two engine tests and their
  helpers, `check`'s comment, and the seven rows.
- **No test was weakened:** the test files only gain lines, but for one
  comment line in `check` (`engine/tests.rs:318-319`), which now names
  the test where the uploader stops.

### Decisions for the lead

None needed for this pull request. S12 and TG14 are small and can
follow in M2.6c-2 or later; S13 is a question for M2.6c-3, if at all.

For information: the workspace doc gate (`cargo doc --workspace
--no-deps` with `-D warnings`) fails on `zfs-topology` in eight crates
(item 4), and no workflow runs it. If it is meant to be a gate, it needs
a package of its own and a CI job.

### Files (beside this note, in `review/m2.6c1/`)

All at `0213a6c` unless named:

- `rec146c.rs`: the scratch engine tests `review7_*` of item 1 (N8's
  scenarios, restarts, absences, the stop matrix); `scen146.out`, their
  run.
- `rec146s.rs`: the livelock search; `search146.py` appends a file to
  `engine/tests.rs` in a scratch worktree and runs it in release mode;
  `search146_sum.py` and `search146.out`: the search at the head,
  `4ff4549` and `a4e004e`, summed up.
- `plants146c.py`, `plants146c.out`: the plants (item 3).
- `gates146c.sh`, `gates146c.log`: the gates (item 4).

Paths in the outputs are shortened to `<wt>` (the scratch worktree).

## Look at #146 at cd3a67c (M2.6c-1)

**The head.** `cd3a67c`, checked with `git fetch origin` and the pull
request's `head.sha` at the start and again before this note was
written. It is one commit on `8663cd7`, the tip of `zfs-topology`, in
`crates/skyzfs-agent` only: `plan.rs`, `plan/tests.rs`,
`engine/tests.rs`. Everything in this note was judged at `cd3a67c`. The
daemon's code read for item 1 (`crates/skyzfs-upload`, `bins/skyzfsd`)
is the base's, as it stands in the head's tree; the pull request does
not change it.

**How I worked.** I read the diff, the module docs and the lead's
decision first, rebuilt the ended arm's table with the fourth look's
grid (item 1), ran the engine with an uploader that stops as skyzfsd's
does (item 1), then the plants (item 2). I read the description after
that.

**The lead's decision.** It is in `docs/skyzfs-impl-notes.md`, the
entry "M2.6b2 as built" (commit `8663cd7`): "Decided at the end (N6):
`refused stopped` is an end like `rewound` (look, give up, release the
hold, report the code); it goes into M2.6c with the fix of the doc of
`ends`." The review record's row for #144 says the same. There is no
comment of the lead's in #144's or #146's thread.

**Verdict: it can merge.** No finding blocks it.

- **N6 is built as the lead decided.** In the ended arm `stopped` now
  does what `rewound` does in every cell but one, where the difference
  is right (`pool@t` gone gives `Destroyed`, not `Rewound`). It is
  reported exactly where the publish is given up on it. Only `stopped`
  cells changed (288 of 12096).
- **The conservative reading is consistent** with the decision's text,
  L5 and L7, and the docs now say it. But it does not wait in two cases
  that skyzfsd produces. **N7** (new, not blocking, older than this pull
  request): a publish not too old whose own `publish` is refused
  `stopped` asks again every second until it is too old, saving the
  record and reading object storage each time in one of the two cases.
  A decision for the lead, since it is the open question the author
  raises.
- **TG6, TG7 and TG8 are closed.** The new tests kill the fourth look's
  W9, W12 and W16, and the author's P1 to P4. Two small new gaps: TG9
  and TG10.
- **S7's two sentences are true.** Two nits: S8 and S9.
- **Gates pass**; CI is green on every run that finished (item 5).

### Findings, most severe first

#### Defects

None in the diff.

#### Questions for the lead

- **N7. The conservative reading waits only where the request pended
  when the uploader stopped. A publish whose own `publish` is refused
  `stopped` asks again every second until it is too old.** Not blocking:
  the code that does it is unchanged since `cf53e94` (the same counts
  there), and nothing is lost or released early. Certain, in the fake
  given skyzfsd's stop.
  - **What skyzfsd does after `Publishing::stop`.** Each `publish` is
    refused `stopped` at once, before the name's last outcome is
    removed (`crates/skyzfs-upload/src/publish.rs:649-651`, `696`). It
    is not kept. `progress` answers the name's kept outcome, or
    `unknown` for a name that never pended (`publish.rs:715-721`). So
    `progress` answers `refused stopped` only for a request that pended
    at the stop. The `ends` doc's "refuses every request after it"
    holds for `publish` only (S9).
  - **What the agent does, not too old.** It keeps an ask's answer for
    one round (`engine.rs:893-895`), plans `Wait` on `stopped`
    (`plan.rs:756`), and clears the answer at that act
    (`engine.rs:627-631`). The next round reads `progress`:
    - `unknown`, for a publish held and never accepted: asked again
      (`plan.rs:742`);
    - `expired` or `withdrawn`, the last request's outcome: looked up,
      then asked anew (`plan.rs:740`), a count saved in the record
      first (`engine.rs:942-948`).
  - **Shown** (`rec146.rs`, with `scratch146.py`'s stop added to the
    fake; output in `scratch146.out`).
    - *Begun after the stop* (`review5_a_publish_begun_after_the_uploader_stopped`):
      600 `publish` requests in 600 s, then given up `TooOld` at the
      bound. No report.
    - *Asked anew after the stop* (`review5_a_publish_asked_anew_after_the_stop`,
      the request expired just before the stop): every second, one
      `publish`, one record save (each a write, fsync, rename and
      directory sync on `Disk`) and one `look` (an object-storage read
      in skyzfsd, `handler.rs:190`). That is 6600 of each until the
      bound at 7200 s, then `TooOld`. No report.
    - The same two scratch tests at `cf53e94` give the same counts
      (`scratch146_cf53e94.out`).
    - *Where the request pended at the stop*, the reading holds:
      asked, one request in an hour, the hold released at
      `held_until`, given up at the bound and reported `Refused(stopped)`
      (`review5_stopped_asked_waits_until_the_bound`). Held and not
      recorded: the hold kept until the bound, no request, then given up
      and reported (`review5_stopped_held_keeps_its_hold_until_the_bound`).
  - **The docs say it waits** (`plan.rs:86-89`, `144-145`, the comment
    at `752-755`, and the description). That is true of the last two
    cases only.
  - **The cost.** It begins when the uploader stops for good: the lease
    lost, a transaction fenced, or the volume failed. Each publish under
    way (up to `UNDER_WAY_MAX`, 8), and each the program begins
    meanwhile (`cuts` is still answered), loops for up to `AGE_MAX`
    (2 h), holding its snapshot. Then it is given up as `TooOld` with
    no `Refused(stopped)` report, since the ended arm reads `unknown` or
    `expired` there.
  - **The choice.**
    1. Not too old, `stopped` is an end as well: look, then give up and
       report. The loop goes. A young publish is lost while skyzfsd is
       stopped, but no request of it pends, and the next skyzfsd starts
       with none.
    2. Keep waiting, but make the engine remember an ask's `refused
       stopped`, and not ask again until `cuts` shows another daemon or
       the publish is too old.
    3. Keep the code; correct the docs and accept the cost.
  - Whichever is chosen, the fake has no stopped uploader. My scratch
    adds one in nine lines (`scratch146.py`), and a test of it would pin
    the choice.

#### Test gaps

- **TG9. `stopped` with `pool@t` gone given up as `Rewound` survives
  (N6e).** No row of the table has `stopped` with the snapshot broken.
  The planner gives `Destroyed` there (item 1), which is right, but
  nothing pins it. Small: it is the reason a person reads.
- **TG10. The report de-duplicated by code alone survives (X12b).** It
  matters for `stopped` above all: one stop ends every request that
  pends, so several publishes are given up on the same code in one
  process. Under X12b only the first is reported. My scratch
  `review5_two_publishes_ended_by_one_stop_are_each_reported` kills it.
  Small.
- **Not a gap: X9b survives.** It makes the report after `act` whatever
  `act` returns. In the engine tests a stop is an early return of
  `Err(Stopped)`, so code after `act` that does not use `?` still runs
  "before" the stop. A killed process would not run it. Every stop
  matrix here (F1's test too) shares that model. The new test pins the
  order against code that returns at the stop (W9, X9c, X9e), which is
  what the model can see.

#### Suggestions

- **S8. The new S7 sentence (`plan.rs:170-173`) also takes in `done`.**
  "That holds for a publish not too old" follows a sentence that ends
  with "`done` that `look` does not confirm". Too old, that `done` still
  stops for a person (L6). "these refusals" does exclude it, but a
  reader may not see that. Say "these refusals (not `done`, which stops
  a publish too old too, L6)". Also "looked up and given up" could be
  "looked up and, unless found, given up".
- **S9. The `ends` doc (`plan.rs:589-593`): "refuses every request
  after it".** skyzfsd refuses every `publish` after the stop.
  `progress` and `withdraw` go on answering, which is what N7 turns on.
  Say "every `publish`".

### (1) N6: the ended arm with `stopped`, and the conservative reading

**The grid.** `plan146.rs` is the fourth look's `fourth144plan.rs`
unchanged but for its name. It runs the planner and `report` over every
state the ended arm reads, and the same states for a publish not too
old (the control):

- too old, eight stalls, or neither;
- held or asked;
- one ms before `held_until`, or at it;
- the pool whole and held, whole and not held, or `pool@t` gone;
- the flag `Seen.withdrawn`;
- what `look` saw (six values);
- the answer (28: every reply, each of the 17 codes, and none).

That makes 12096 cells. `table146.py` compares them cell by cell with
the rows at `cf53e94` and sums them up (`table146.out`).

**What changed.** 288 cells, all `refused stopped`, all too old or
stalled eight times. Not one cell of a publish not too old, and no
other answer. Each was `Wait` (or `Release` at `held_until`) and is now
`Look`, `Done`, `Release` or `Abandon`.

**`stopped` against `rewound`, too old or stalled.** The same action in
every cell but one class: `pool@t` gone and `look` absent, which gives
`Abandon(Destroyed)` for `stopped` and `Abandon(Rewound)` for `rewound`.
That is right: `Rewound` says a rewind lost `pool@t`, and `refused
stopped` says nothing of a rewind (`plan.rs:706-710`). No test pins it
(TG9).

| Too old or eight stalls; `look` saw | Pool whole and held | Whole, not held | `pool@t` gone |
|---|---|---|---|
| nothing yet | `Look` | `Look` | `Look` |
| absent | `Release`, reported | `Abandon(TooOld or NoProgress)`, reported | `Abandon(Destroyed)`, reported (`rewound`: `Rewound`) |
| ours | `Done` | `Done` | `Done` with `SnapshotGone` |
| another's, unreadable | `Release`, reported | `Abandon(NameTaken)`, reported | `Abandon(NameTaken)`, reported |

The same in each phase (held or asked), time and flag.

**`stopped` not too old** (the conservative reading). `Wait` in every
cell, but `Release` where the publish is asked, whole and held, at
`held_until`. That is the same as `ended`, `no-answer` and `no-upload`
(the rows at `plan/tests.rs:503-523`). `rewound` there asks anew
(`AskAnew`, `Hold`, or a look first and a give-up where the snapshot is
broken).

**The ended arm as a whole at the head.** Every cell that releases the
hold or gives up is in one of the fourth look's three classes:

| Class | Cells | At `cf53e94` |
|---|---|---|
| A. A withdraw answered `withdrawn`, `unknown` or `expired`, then `look` | 216 | 216 |
| B. An end of L7's list, `stopped` now among them, then `look` (absent, another's, unreadable) | 1728 | 1584 |
| C. Asked, at `held_until`, on an answer that cannot tell (L5) | 216 | 240 |

- No cell waits on an end. The wait cells are none, `accepted`, `cuts`,
  `busy`, `ended`, `no-answer`, `no-upload`, `snap` and `unreadable`.
- `Look` is never planned where a sight is already there.
- `report` is `Refused(code)` in exactly the 1728 cells of class B, and
  `None` in every other cell, every cell not too old included. `stopped`
  has 144 of them, as many as `rewound`.

**Is `stopped` an end safely?** Yes, as the fourth look found.
`Publishing::stop` ends every request that pends
(`crates/skyzfs-upload/src/publish.rs:592-595`), and the uploader takes
no step after (`uploader.rs:466-473`, `493-497`, `530-533`, `611-616`).
`look` goes to object storage and not to the uploader
(`bins/skyzfsd/src/control/handler.rs:190`, `196-205`), so it is
answered while the uploader is stopped. A transaction that landed
before the stop is what `look` finds.

**`ends()` agrees everywhere.**
- The function (`plan.rs:599-615`): twelve codes true, five false; the
  match covers all 17.
- Its two uses, the ended arm (`plan.rs:710`) and `report`
  (`plan.rs:630`). The engine has no list of codes of its own.
- Its doc (`plan.rs:582-598`), the module's L7 bullet
  (`plan.rs:77-89`) and its "daemon away" bullet (`plan.rs:144-148`).
- The tests' two copies (`plan/tests.rs:1495-1510`, `1719-1723`).

**The doc no longer calls `stopped` silence.** Silence is `ended`,
`no-answer` and `no-upload` (`plan.rs:595`). The module calls `stopped`
"a daemon away" only for a publish not too old, and the arm's comment
says why (`plan.rs:752-755`). Those two sentences, and `plan.rs:86-89`,
are true where the request pended at the stop, and not where an ask is
refused `stopped` (N7).

**The conservative reading against the decision.**
- The decision's text names what the ended arm does, "look, give up,
  release the hold, report the code". It says nothing of a publish not
  too old. Not too old, `rewound` is not given up either: it is asked
  anew. So the reading does not contradict it.
- It does not contradict L5: a held publish keeps its hold on `stopped`
  while young. No request pends there, so the hold is kept longer than
  needed, but never let go while one may pend. Since this pull request
  that is bounded by `AGE_MAX`; at `cf53e94` it lasted until skyzfsd
  restarted.
- L7 is the ended arm's, and is built.
- Shown in the engine, where the request pended at the stop:
  - `review5_stopped_asked_waits_until_the_bound`: asked, then stopped.
    One request in all; the hold released at `held_until`; given up as
    `TooOld` at the bound, with `Refused(stopped)`.
  - `review5_stopped_held_keeps_its_hold_until_the_bound`: accepted and
    not recorded, then stopped. The hold kept at 30 minutes, no request;
    given up at the bound with the report.
  - `review5_stopped_too_old_is_given_up_and_reported`: the fourth
    look's `review4_stopped_waits...` run, which waited for good at
    `cf53e94`, now ends at 60 s: `TooOld`, one report, no hold.
- Where an ask is refused `stopped`, it does not wait: N7.

### (2) The new tests and the plants

PLACEHOLDER_PLANTS

### (3) S7

Both sentences are true at the head.

- **`plan.rs:166-173`.** Not too old, `name`, `manifest`, `expiry`,
  `version`, `malformed`, `peer` and `store` stop the publish
  (`plan.rs:757-763`). Too old or stalled eight times they are ends
  (`plan.rs:710`): looked up, then done, given up as `NameTaken`, or
  released and given up with the report (item 1). S8 is a nit of
  wording.
- **`plan.rs:595-598`.** skyzfsd's only `busy` is `Publishing::answer`
  at `PENDING_MAX` (`publish.rs:690-692`). No outcome is `busy`, so
  `progress` and `withdraw` never answer it. A full inbox is `no-answer`
  (`handler.rs:212-217`). The ended arm never asks; an ask's `busy` kept
  for a round (`engine.rs:893-895`) waits there (`plan.rs:714`), and the
  next round reads `progress`.

### (4) Gates

Toolchain 1.100.0 stable; `rustup update stable` changed nothing. All
pass at `cd3a67c`, in a clean worktree (`gates146.sh`, log
`gates146.log`):

- `cargo fmt --all -- --check`;
- `cargo clippy -p skyzfs-agent --all-targets -- -D warnings`;
- `cargo test -p skyzfs-agent`: 72 passed (70 at `cf53e94`, and the
  two new engine tests);
- `RUSTDOCFLAGS="-D warnings" cargo doc -p skyzfs-agent --no-deps`;
- `cargo clippy --workspace --all-targets -- -D warnings`, plain, with
  `--features skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `--features skyzfs-ublk-sys/control-test-program`.

I did not run `cargo test --workspace` in the three modes. No crate
but `skyzfs-agent` names the agent, so the diff cannot change another
crate's tests; CI's `test` and `unit-arm64` run them (item 5).

### (5) CI on the head, and the diff

**Runs on `cd3a67c`: all five completed, all green**, read through the
API after the last job ended.

| Run | Conclusion | Jobs |
|---|---|---|
| ci | success | changes, lint, unit-arm64, test, chart-lint, s3-backends |
| coverage | success | |
| e2e | success | |
| k3s-smoke | success | |
| skyzfs-ci (push) | success | gate only; the rest skipped, as a pull request is open |

- No rig ran. None of their paths covers `crates/skyzfs-agent`, and the
  diff touches nothing else. No rig drives the agent yet (M2.6c).
- `lint` and `unit-arm64` run clippy and the tests in the three modes,
  on amd64 and arm64.
- Codex did not review: its summary on the pull request says the review
  failed (the usage limit), and it posted no finding.

**The diff `8663cd7..cd3a67c`.** Three files, all in
`crates/skyzfs-agent`: `plan.rs` (+32 -16), `plan/tests.rs` (+51 -2),
`engine/tests.rs` (+166).

- **Nothing in it goes beyond the description's list:**
  - `ends` with `stopped`, and its doc;
  - the arm's comment on `stopped` not too old;
  - the three module-doc sentences (L7, the daemon away, S7);
  - the table's kind "end" with `stopped`, three explicit rows, the
    report checked on the explicit rows (TG8), and the stall block;
  - the helper `rewound_when_too_old` and the two engine tests.
- **No test was weakened.**
  - `stopped`'s rows in the too-old table change from "keep" to "end",
    and the stall block's `stopped` from `Wait` to `Release` with the
    report. That is the behaviour N6 changes, and the new rows also
    check the report.
  - The rows of a publish not too old (`plan/tests.rs:503-523`) are
    unchanged and still have `stopped` with the away codes.
  - No assertion was removed elsewhere. The other two changed lines
    bind `publish` to a name so that `report` can be called with it.
- The branch is on the tip of `zfs-topology` (`8663cd7`), which has not
  moved.

### Decisions for the lead

- **N7.** For a publish not too old, should `refused stopped` be an end
  as well (look, give up, report), or a wait that does not ask again?
  The code as it stands asks every second in two cases. The author's
  open question is the same choice.
- **Optional.** TG9 and TG10 (a row and a two-publish test); S8 and S9
  (two sentences).

### Files (beside this note, in `review/m2.6c1/`)

PLACEHOLDER_FILES

# Sixth look at #153 (M2.6c-2b): 291746c

This note judges head **291746ccbab47759fb0f11d9ccd3f6e0c127746f** of
`skyzfs-coding-m2.6c-2b`: two commits on 0a19e1c.

- **de57162**: D2's fix by option (a), the count before the destroy; the
  tests T9 and T10; the nits.
- **291746c**: the merge of `zfs-topology` at 6e1cc2a, which brings in
  #149 (merged at 53799be).

I formed a view from the code and from design §6.2 and the notes for
M2.6 first, and then read the author's answer (issue comment
7180458351). The earlier notes are `review.md` (65d72ee) and
`next_look.md` (3d40045, 0a19e1c) beside this one. The scratch files
of this look carry the suffix `153f`.

## Verdict

**Can merge at 291746c.** I found no defect, and nothing here blocks.
CI on 291746c has ended green. I read the daemon rig's tables (run
45740135508) and every line is as expected (see CI below). Codex
completed with no findings.

- **D2 is fixed.** The searches below found no run where a take that
  fails its check is taken more than 3 times, and no run that left a
  snapshot, hold, ref, manifest or entry under way behind. They found no
  publish after a `NotWhole` report and no wrong outcome.
- **The claims in the doc and the answer hold.** There are at most 3
  takes whose checks fail, and a stop after the last destroy gives up
  with no take. Scenario 2 is bounded only by `AGE_MAX`, and the doc
  says so. One sentence is exact only for a take that is not whole
  (N10).
- **The merge with #149 keeps the record's bytes.** #149's `GOLDEN`
  decodes at this head and encodes back to the same bytes. I found no
  break in order or encoding.
- **Reporting before the destroy is sound.**
- **T10 is done**, and the nits are answered.

Found here, none of it blocking. I would add T12, T11 and T8 in this
PR, since they are small, but that is the author's and the lead's call:

- **T12 (low):** no test pins that the give-up is told at least once.
  Three plants survive.
- **T11 (low, from the merge):** the program's "told apart" test leaves
  out `Why::NotWhole` and `Report::NotWhole`. Three plants on the CLI's
  word and line survive.
- **T8 (low, open from the last look):** no test checks the names in
  the report of a take that is not as before, or of more than one
  dataset. Three plants survive.
- **T13 (low, from #149):** no test checks that `Effect::ALL` names
  every effect.
- **Nits N10 to N13:** doc wording, and the CLI's line.

## (1) D2 across restarts

### The code

- **The count before the destroy.**
  - `failed` (`engine.rs:1307-1328`) commits the count at
    `Effect::Failed` (`:1313`).
  - Only after that does it either retake (`retake` → `rename`) or report
    and destroy the take.
  - `rename` (`:1355-1357`) now commits only the next name.
- **The give-up with no take.** `take` (`:1137-1145`) gives the publish
  up as `Why::NotWhole`, with `Report::NotWhole(vec![])`, when
  `attempts >= CHECKS_FAILED_MAX`. It does this before the pause, the
  count, the Intent commit and the take.
  - `attempts` at Intent counts nothing else.
  - A publish is created at Intent only once (`:704-706`), and no path
    goes back to Intent.
  - In later phases `attempts` goes on counting the requests (`:1468`).
    Only the program's state line reads it there, so no bound depends on
    it.
- **A not-whole take is always counted.** `adopt` sends `NotWhole` to
  `failed` before it looks for a reading. Only a whole take with no
  reading in memory goes to the uncounted `retake(name, false)`.

### The claims, checked

| Claim | Holds? | Shown by |
|---|---|---|
| The takes whose checks fail are at most 3, wherever each process stops | yes | SX1 at depths 3 and 4, SX3; Q1, Q3, Q8 and Q17 killed by T9 |
| A stop after the last destroy has the next process give up with no take | yes | Q2, Q5 and Q6 killed by T9; SX1 |
| A stop between the count and the destroy gives up early, never with a take more | yes, for a take that is not whole (see N10 for one that is not as before) | SX1 Never: 64 of 7 577 runs gave up after a single take |
| Scenario 2 is bounded only by `AGE_MAX`, and the doc says so | yes (`engine.rs:1297-1302`) | SX2 NotAsBefore with the same stop: 120 to 240 takes in about 240 processes, ended `TooOld` at about 2 h |

### The searches

The harness is `scratch153f.rs` (with `scratch153f_patch.py`). It runs
the real `Agent` over the engine's fakes, one process after another.
Each process stops at a point the schedule chooses, and the next process
starts after `DOWN`.

**The cases:**
- **Never:** every take misses a dataset.
- **Held(k):** as Never, and a person's hold lands on the k-th take, so
  that its destroy is refused.
- **NotAsBefore:** every take is whole, but a person changes a property
  between the reading and the take.
- **FirstWhole:** the first take is whole and as before; every later
  take misses a dataset. A first take that a restart leaves with no
  reading is retaken uncounted.

There is also a knob that takes the daemon away for a window.

**What the judge checks:**
- every run has exactly one end, published or given up, read from the
  processes or else from the record and the refs;
- the takes are at most 3 plus the uncounted retakes;
- a take that is never whole is never retaken uncounted;
- every report is `NotWhole`, and no publish follows one;
- nothing is left behind (snapshots, holds, refs, manifests, entries
  under way). A `TooOld` give-up may keep its take, as the plan says.

| Search | Runs | Problems |
|---|---|---|
| SX1, every schedule of stops, depth 3: Never / Held(1) / Held(2) / Held(3) / NotAsBefore / FirstWhole | 7 577 / 689 / 3 213 / 7 881 / 11 545 / 9 005 | 0 |
| SX1, depth 4: Never / Held(1) / Held(2) / Held(3) / NotAsBefore / FirstWhole | 63 953 / 3 857 / 23 885 / 69 737 / 143 225 / 99 777 | 0 |
| SX3, random stops and daemon-away windows, per case | 60 000 × 5 | 0 |
| SX2, every process stops at the same point or after the same take | 168 + 80 per case | only the expected endless runs (below) |

- **Never, under every schedule:**
  - at most 3 takes;
  - no uncounted retake;
  - every run ends `NotWhole`. Where the process that committed the
    end stopped before it said so, the judge reads the end from the
    record; the harness's tally shows these as "none".
- **NotAsBefore:** at most 6 takes at depth 3 and 7 at depth 4. That is
  always 3 plus the uncounted retakes, and each of those needs a stop
  between that take and its adoption.
- **SX2's only "problems" are the runs where no process can make
  progress.** Every process stops before the give-up's commit
  (`Abandon`, before), or, for FirstWhole, before the look. These end
  only when the stops end. This is the property the doc names: a
  process that stops at one point every time.

### Does T9 cover scenarios 1 and 2?

- **Scenario 1: yes.**
  `a_take_never_whole_is_given_up_whatever_point_each_process_stops_at`
  (`engine/tests.rs:2452`) kills the old order (Q1). It also kills a
  count made only on a retake (Q3), no give-up without a take (Q2), that
  give-up recorded as `TooOld` (Q5), and the bound off by one (Q6).
- **Scenario 2: not in T9.** Its world is never whole. That is right,
  since the PR does not claim a bound there beyond `AGE_MAX`.
- **T9's stop points.** It stops at `Failed`, `Destroy` and `Intent`
  only.
  - In the `Intent` case, every process after the first stops at
    `take`'s own Intent commit, before any take. Only process 0 stops at
    the rename.
  - It never stops at the give-up's `Abandon` or `Unlink` (T12).

## (2) The merge with #149

| What | Judged |
|---|---|
| `Why` order: `…, Stopped, Person, NotWhole` (`record.rs:310-315`) | `Person` keeps #149's index 10, and `NotWhole` is 11. Q12 (the two swapped) is killed by `version_2s_bytes_are_these`. |
| `GOLDEN` regenerated | It differs from #149's only in the frame's length (0x1fb to 0x202), the count of entries given up (0b to 0c), the new entry `w11` (`037731310bf307`) and the hash. |
| A record written by #149's agent | `record_compat153f.rs.txt`, a scratch test: #149's `GOLDEN` (from 6e1cc2a) decodes at this head to `golden()` less its last entry, and that record encodes to #149's bytes again. It passes (`record_compat153f.out`). An agent of #149 reads a record holding `NotWhole` as Corrupt: a downgrade only. |
| `Effect::ALL` (`engine.rs:230-259`) | 28 effects; `take`, `failed` and `destroy` stand where #149's `program` stood. No rig or workflow names an agent effect: the only `--kill-at` in `tests/` is skyzfsd's `--kill-at-upload`. Q18 (Failed left out of `ALL`) survives: T13. |
| `begin` counts before #149's sweep (`engine.rs:680`) | Q10 (no count) and Q11 (the count after the sweep) are killed by `each_round_counts_the_datasets_before_its_calls`. |
| #149's two tests skip the count's reads | Neither is weakened. `a_stop_for_a_person_outlasts_a_restart_until_recovered` leaves out `count` and `tagged`, the round's own reads that #149's fake did not record, beside `props`, which it already left out. The program test's fake `zfs` answers `zfs list` without logging it. #149's fake already did not log `zfs program`, so the reads were never in "no zfs call before the refusal". |
| `rename` now links the old name to the next in `renamed` (`:1358`) | Needed for #149's `name_now`. Q9 (the link left out) is killed by `a_publish_made_under_its_next_name_exits_by_its_outcome`. |
| `tell` passes `&made.1` to the sink | Same report, same once-per-process rule. |
| `Report` | Not serialized, so its order has no bytes to keep. |

**A note for the lead, not a finding.** `record.rs:39-42` says "No
agent has run version 2 either; from the first run of the program
(M2.6c), any change to the record takes a new version". The program is
now on `zfs-topology`. Appending `NotWhole` at version 2 is right on
your reading that no agent has run outside CI. The next change to the
record, after this PR, takes version 3 and a decode test of version 2.

## (3) The report before the destroy ("told at least once")

**It is sound.** The report is made after the count is committed
(`:1313`) and before the destroy (`:1317-1320`) and the give-up's commit
(`abandon`, `:1648`). At each stop point:

| Stop | What follows |
|---|---|
| Before the report | The next process finds the count at 3. A take that is not whole is counted again and reported with its names by `failed`; one that is not as before is destroyed uncounted, and `take` gives up and reports with no names. |
| After the report, before the destroy | Told once. The next process tells it again and gives up. |
| After the destroy | Told once. The next process gives up from `take` and tells it again, with no names. |
| After the give-up's commit | Told already. |

- **Why the alternative is worse.** Reporting after the give-up's commit
  would lose the report, and with it the names, on a stop between the
  commit and the report. The outcome would still be read from the
  record.
- **The cost:** a second report, or one with no names. That is the right
  side to err on.
- **My earlier nit** (a `NotWhole` report for a publish that was not
  given up) **no longer happens.** `attempts >= 3` sticks across
  processes, so a publish that was reported always ends:
  - `NotWhole`;
  - `ManifestLost`, where the destroy fails (documented, the report
    kept);
  - or `TooOld`, where the next process starts after `AGE_MAX`. If the
    take is still there, that give-up keeps it, as the plan does for any
    publish at Intent.

  No search run published after a `NotWhole` report.
- **Where the answer is not exact.** "The next process count[s] the same
  take again … with a second report" holds for a take that is not
  whole. It does not hold for one that is whole but not as before (N10).

## (4) T10 and the nits of the last look

- **T10: done.** `the_limits_grow_with_the_datasets_counted` now kills
  C2, C3 and C9, the full read's limit given to the plain members
  program, to `tagged` and to `zpool status`.
  - C12 (`zpool get` given the full read's limit) survives. It was not
    in T10, and I do not ask for it.
- **The cost doc** now adds each round's reads, the hold, the release
  and the sweep: some 4 to 5 more minutes at 10 000 datasets. This
  matches my figures.
- **The partial-hold test** is renamed
  `a_hold_on_a_part_of_the_take_as_recorded`.
- **The cap's figure** is corrected in the answer. The commit message
  cannot change on a branch under review, and I agree.
- **N6** is in the module doc of `zfs/cli.rs` as a known cost, with
  #155's measurement.
- **N7.** The CLI names at most 20 datasets and then "and K more"
  (`program.rs:556-577`). No test checks this (T11).
- **N8** goes to M2.6c-5.
- **N9.** The "cannot loop" doc is replaced by "Across restarts".
- **T6** (a not-whole take found after a restart is counted): covered.
  Q8 is killed by T9.
- **T7** (a stop at every point of the last check): covered in part.
  T9 does not stop at `Abandon` or `Unlink`, and Q7 survives (T12).
- **T8** (the names in a report): still open. The test the answer
  cites names one dataset, of a take that is never whole; N1, N2 and
  N3 survive (below).

## Test gaps

### T12 (low): "told at least once" is not pinned

- **Where:**
  - `engine.rs:1142`, the report of the give-up with no take;
  - `engine.rs:1317`, the report before the destroy;
  - T9's effects (`engine/tests.rs:2456`).
- **Scenario:**
  - **Q4 survives.** The report at `:1142` is removed. Take a take that
    is whole but not as before, whose third check is counted (`:1313`),
    and a process that stops before the report. The next process has no
    reading. It destroys the take uncounted, renames it, and gives the
    publish up from `take`. Its `NotWhole([])` is then the only report
    the person gets. In 873 of the 11 545 runs of SX1 NotAsBefore at
    depth 3, every report was `NotWhole([])` (832 with one, 40 with
    two, 1 with three). With Q4 the publish is given up and never told.
  - **Q16 survives.** The same report is pushed but not given to the
    sink, so the CLI prints nothing.
  - **Q7 survives.** The report at `:1317` is moved after `abandon`.
    With a stop at `Abandon` (after) or `Unlink`, nobody is told and the
    names are lost. T9 never stops there.
- **Fix:** add `Effect::Abandon` and `Effect::Unlink` to T9's effects.
  Then either give `never_whole` a world that is not as before, or add a
  unit test: a publish at Intent with `attempts = 3` and no snapshot
  gives up with `NotWhole([])`, told to the sink.
- **Sure:** high that the plants survive; low that it matters soon,
  since the code is right.

### T11 (low): the CLI's word and line for `NotWhole` are untested

- **Where:** `program/tests.rs:349-361` and `:376-381`.
  - `each_outcome_report_and_exit_is_told_apart` lists every `Why` up to
    `Person`, and every `Report` up to `HeldAway`. The merge added
    `Why::NotWhole` (`why_word`, `program.rs:519`) and
    `Report::NotWhole` (`not_whole`, `:560-577`), but not to the test.
  - No other test calls `not_whole`.
- **Scenario:**
  - Q13: `why_word(NotWhole)` is made `"person"`. The outcome line of a
    publish given up after three takes that were not whole then reads
    "abandoned skz-t person", the same as a person's give-up.
  - Q14: the 20-name cap is removed.
  - Q15: the line after a restart says "datasets: " and nothing more.

  All three survive.
- **Fix:** add `Why::NotWhole` to the list, and `Report::NotWhole` with
  0, 1 and 21 names to the reports.
- **Sure:** high.

### T8 (low, open from the last look): the names in a report

- **Where:**
  - `differing` (`engine.rs:493-505`), the names of a take that is not
    as before;
  - `adopt`, which passes those names (`:1230`);
  - `zfs::taken` (`zfs.rs:287`), the names of a take that is not whole.
- **Scenario:** the answer cites
  `a_take_never_whole_is_given_up_at_the_third_failed_check`, which
  checks `NotWhole(["skzq/vol"])`: one dataset, of a take that is never
  whole. Three plants survive:
  - N1: the report of a take not as before names nothing;
  - N2: `differing` leaves out the datasets read before and missing
    from the take;
  - N3: `taken` names only the first dataset that breaks a take.

  With N1, a publish given up because a dataset was renamed under each
  of its three takes tells the person nothing about which one.
- **Fix:** a test where the third take is not as before, by a rename
  and by a dataset destroyed, that checks the names. Have two datasets
  skipped in the never-whole world.
- **Sure:** high.

### T13 (low, from #149): `Effect::ALL` is not checked against the variants

- **Where:** `engine.rs:230`.
- **Scenario:** Q18 removes `Effect::Failed` from `ALL`. Every test
  passes, and `--kill-at failed:after` would then be refused as an
  unknown effect. Testing's rigs could not stop at the new count.
- **Fix:** in `each_effect_is_named_by_a_word_of_its_own`
  (`engine/tests.rs:3080`), an exhaustive `match` that maps every
  variant to its index in `ALL`, checked for each variant.
- **Sure:** high. It was there in #149; it is not new in this PR.

## Nits

- **N10.** Two places say "count the same take again", and that is exact
  only for a take that is not whole:
  - the doc "Across restarts" (`engine.rs:1289-1291`): "A stop between
    the record and the destroy has the next process find the same take
    and count it again";
  - the answer's "with a second report".

  A take that is whole but not as before is destroyed **uncounted** by
  the next process, which has no reading. That process then takes again,
  or, at the last check, gives up from `take` with no names, under the
  next name. The bound is the same either way; only the wording is off.
  Also, "one check early" is one per such stop: 64 runs of SX1 Never
  at depth 3 gave up after a single take, each after two such stops.
- **N11.** `Report::NotWhole`'s doc (`plan.rs:454-455`) and the comment
  at `engine.rs:1134-1136` give one way the list ends up empty: "the
  last take was destroyed before a stop". It is also empty in the case
  of N10, where the take was still there at the stop and its count was
  recorded.
- **N12.** The CLI's line (`program.rs:561`) says "not one whole take of
  the pool" for a take that was whole but not as read before it too.
  "Not one whole take of the pool, or not as read before" would match
  the `Why`'s doc.
- **N13.** The names are joined by a space (`program.rs:570`). A ZFS
  dataset name may hold a space (`valid_char` in OpenZFS's
  `zfs_namecheck.c`), so the line can be read two ways. Low.

## Plants

`plants153f.py`, each plant applied alone to a copy of the head. Then
`cargo test -p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks --no-fail-fast` ran, with a limit of 600 s.
Outputs: `plants153f.out` and `plants153f_n.out`.

| Plant | Result | By |
|---|---|---|
| Q1 the count after the destroy (D2's order) | KILLED | T9 |
| Q2 no give-up without a take | KILLED | T9 |
| Q3 the count only on a retake | KILLED | T9 |
| Q4 no report at the give-up without a take | SURVIVED | T12 |
| Q5 that give-up recorded as `TooOld` | KILLED | T9 |
| Q6 that give-up one check late (`>`) | KILLED | T9 |
| Q7 the last check's report after the give-up's commit | SURVIVED | T12 |
| Q8 a not-whole take with no reading not counted | KILLED | T9 |
| Q9 the rename leaves no link in `renamed` | KILLED | `a_publish_made_under_its_next_name_exits_by_its_outcome` |
| Q10 `begin` does not count | KILLED | `each_round_counts_the_datasets_before_its_calls` |
| Q11 `begin` counts after its sweep | KILLED | the same |
| Q12 `NotWhole` ordered before `Person` | KILLED | `version_2s_bytes_are_these` |
| Q13 the CLI's word for `NotWhole` is `person` | SURVIVED | T11 |
| Q14 the 20-name cap removed | SURVIVED | T11 |
| Q15 the line after a restart names an empty list | SURVIVED | T11 |
| Q16 the give-up without a take not told to the sink | SURVIVED | T12 |
| Q17 the count saved at the points of `Intent` | KILLED | T9 |
| Q18 `Failed` left out of `Effect::ALL` | SURVIVED | T13 |
| N1 the report of a take not as before names nothing | SURVIVED | T8 |
| N2 `differing` leaves out what was read before and is missing | SURVIVED | T8 |
| N3 `taken` names only the first dataset that breaks the take | SURVIVED | T8 |
| C2 the plain members program given the full read's limit | KILLED | `the_limits_grow_with_the_datasets_counted` |
| C3 `tagged` given the full read's limit | KILLED | the same |
| C9 `zpool status` given the full read's limit | KILLED | the same |
| C12 `zpool get` given the full read's limit | SURVIVED | not asked for |

## Gates

rustc 1.100.0 (2026-11-09), the current stable (`rustup update stable`
changed nothing). The gates were run in a scratch work tree of 291746c
with its own target directory, by `gates153f.sh`; `gates153f.out` is
the summary of its log.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings`: plain / `skyzfsd/test-hooks,skyzfs-local/test-hooks` / `skyzfs-ublk-sys/control-test-program` / `skyzfs-agent-cli/test-hooks` | clean, each |
| `cargo test --workspace --no-fail-fast`, plain | 1934 passed, 0 failed, 16 ignored |
| the same with `skyzfsd/test-hooks,skyzfs-local/test-hooks` | 1992 passed, 0 failed, 16 ignored |
| the same with `skyzfs-ublk-sys/control-test-program` | 1958 passed, 0 failed, 18 ignored |
| the same with `skyzfs-agent-cli/test-hooks` | 1934 passed, 0 failed, 16 ignored |
| `RUSTDOCFLAGS='-D warnings' cargo doc --workspace --no-deps`: plain and with each of the three feature sets | clean, each |

These are the author's counts.

## CI on 291746c

All 37 check runs ended: 32 passed and 5 were skipped. None failed.

- **Runs:**
  - ci 45740135284 (test, lint, unit-arm64, s3-backends, chart-lint);
  - coverage 45740135394;
  - e2e 45740135295;
  - k3s-smoke 45740135258;
  - skyzfs-ci 45740128232 (push).
- **Rigs:**
  - the cut rig 45740135211;
  - the publish rig 45740135373;
  - the replay rig 45740135322, whose jobs "through skyzfsd" passed and
    whose reference jobs were skipped, as a pull request that touches
    only the daemon's paths has them;
  - the daemon rig 45740135508.
- **Skipped:** "Publish badges", the replay rig's reference jobs, and
  `skyzfs-ci`'s lint, test and unit-arm64, which do not run where a
  pull request is open.
- **Why the rigs ran.** Against its base, this PR touches
  `bins/skyzfsd/src/kill.rs`: docs only, from 9717321, as noted in
  `next_look.md`. It touches nothing under `tests/`.
- **The daemon rig's tables.** I read the three summaries from the
  artifacts `daemon-rig-ubuntu-24.04-daemon`,
  `daemon-rig-ubuntu-26.04-daemon` and
  `daemon-rig-ubuntu-26.04-arm-daemon`. Each says "Every pin and every
  control is as the table `EXPECTED` in `tests/kernel/daemon_check.py`
  has it." Each table has 87 lines, and none differs from its expected
  value:
  - 54 PASS;
  - 5 FAIL, all of them controls (`ctl_torn_overlapping_writes`,
    `ctl_kill_without_reissue`, `ctl_volume_that_does_not_fail`,
    `ctl_hold_too_short` and `ctl_g_old_content_after_good_sync`);
  - 1 WAITING, `pin_failed_volume_while_a_request_is_held`, as CLAUDE.md
    says;
  - 2 NA, for the in-place store;
  - 13 WEAKER;
  - 12 NOT_EXERCISED.

  The store was in-place on Ubuntu 24.04 and 26.04, amd64 and arm64.
- **Codex** completed its code review and its security review on
  291746c, with no findings.

## Files beside this note

- `scratch153f.rs`, `scratch153f_patch.py`: the search harness and the
  script that copies it into a scratch work tree. These were not added
  to the product.
- `search153f.out` holds SX0 to SX3, with `SX1_DEPTH=3` and
  `SX3_RUNS=60000`. `search153f_d4.out` holds SX1 with `SX1_DEPTH=4`.
  Both ran in release:
  `cargo test -p skyzfs-agent --release --lib review_scratch -- --nocapture --test-threads=1`.
- `plants153f.py`, `plants153f.out`, `plants153f_n.out`: the plants.
- `record_compat153f.rs.txt`, `record_compat153f.out`: the decode test
  of #149's `GOLDEN`, appended to `record/tests.rs` in a scratch work
  tree.
- `gates153f.sh`, `gates153f.out`: the gates.

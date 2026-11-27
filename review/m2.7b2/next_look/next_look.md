# Next look at skyzfs/skyzfs#179 (M2.7b-2), from e6530f1 to 78c3997

| head | what it adds |
|---|---|
| `c195ea4` | retention yields between its steps (M2's first half); L2, L5; tests |
| `8653845` | the lead's rulings on the review: early leave of the give-ups (M1), the step's own count of children, `ZfsError::NotRun`, `PUBLISHED_MAX` 1128, docs |
| `cdf6cb4` | tests for what the author's plants of the two commits above found untested |
| `78c3997bfab4550213c9420fd3bef1ce679fc779` | docs only: `set_keep`'s comment (the lead's ruling on L4) |

**The verdict is on 78c3997.** `git diff cdf6cb4 78c3997` is four
added lines of doc comment on `Agent::set_keep`
(`engine/retain.rs:236-241`) and nothing else, so what I ran at
cdf6cb4's code holds; every run below was at 78c3997 all the same.

- **The base.** `7cf6da6`. `zfs-topology` is now at `6a8a3f6`: #180
  (the agent rig's failed-while-serving course, `tests/kernel/` and
  the workflow only) and a notes commit merged since. `git merge-tree`
  of the head with `origin/zfs-topology` is clean.
- **What I read first.** My own view of `c195ea4..78c3997` against my
  review of e6530f1 (`../review.md`), before the answer
  (issuecomment-7216853058); then the answer's claims one by one.
- **Paths and lines.** Under `crates/skyzfs-agent/src/` unless another
  crate is named; lines at 78c3997.

## Verdict on 78c3997

**Ready to merge in my reading once CI is green: no finding blocks.**
CI at this sha is green but for the agent rig's part `a2`, which
failed at installing its packages (the rig's own error, exit 2; below)
and needs a re-run. M1 and M2 are fixed as the lead ruled, `NotRun`
holds everywhere I looked, and the answer's plant table is honest:
every plant it says is caught is caught when I plant it alone, and the
one it says survives (`G16'`) survives. Codex has not reviewed past
`adc391a`; its one finding there (P1) is answered and fixed in
`c195ea4`.

What I would still take, none blocking alone:

- **T1** (test gap): `ZfsCli::destroy_one`, the only destroy
  retention runs, has no test; three plants of it survive. A scratch
  test is ready and kills all three. I would take it before the merge.
- **N2** (low defect, for the author): `destroy_one` reads ZFS's
  "could not find any snapshots to destroy" as a failed destroy, so a
  member gone since the read sends its name to the names stuck.
  `destroy_r` already reads that line as destroyed.

And two for the lead:

1. **N1, low (a question on the ruling on M1).** The early leave is
   the last arm of the round's eight steps, and a retire refused
   `stopped` takes a step. With eight or more names marked to be
   retired when the uploader stops, no give-up leaves early: the
   begins stop at the 257th round of the stop, as before the ruling,
   and resume after the uploader is back only once the marked names
   are fewer than eight. Bounded (about `(published - keep) / 8`
   rounds after the uploader is back, at most about 140), so not M1's
   day, but the module docs' "does not stop the publishes after it is
   back" (`engine/retain.rs:95-96`) does not hold then.
2. **N3, information (the ruling on M2 already covers it).** The bound
   the answer gives for the step after a failure (30 s, the test's
   35 s) is that of a retire in flight. A member's destroy in flight
   is not cut either, and on a pool whose device was deleted it runs
   to its limit, `Limits::change`: at least 120 s.

## Findings, most severe first

### N1. A backlog of names marked to be retired starves the early leave (low; question for the lead)

`engine/retain.rs:285-315`. `retain_steps` spends the round's
`STEPS_MAX` (8) on the names published that are marked first, in
record order, and only then calls `retain_given_up` with what is left.
`retire_ref` takes a step before its ask (`steps += 1`, line 305), and
a refusal other than `no-answer` does not end the round's retires
(lines 373-382; the lead's ruling on L2 names `no-answer` and no
answer only). skyzfsd refuses every retire `stopped` while its
uploader is stopped (`bins/skyzfsd/src/control.rs:138`,
`RetireRefused::Stopped`), the same condition that gives up every
publish in M1's scenario. So where eight or more names are marked when
the uploader stops, every round of the stop spends its eight steps on
retires refused `stopped`, and `retain_given_up` meets
`*steps >= STEPS_MAX` (line 605) for every name, early or past its
keep.

- **Scenario.** `--keep` is lowered by eight or more (marks up to
  `RETIRING_MAX`, 64, at once, and more as they leave), or the
  uploader stops while a backlog of marked names is being worked off.
  The uploader then stays stopped for more than 256 rounds (4.3 h at
  `--every-s 60`). From about the 257th round every begin is refused
  for room, as before the ruling. Once the uploader is back, the
  retires succeed, eight a round, and the give-ups get a step only in
  the round where fewer than eight marked names are left; the begins
  resume the round after. Up to about `(PUBLISHED_MAX - 1) / 8`, 141
  rounds.
- **Shown** by a scratch test (`scratch-program-tests.rs`, planted in
  a copy of the head only): `twelve()` names published, `--keep 1`,
  the publishes refused `stopped` and the retires refused `stopped` as
  skyzfsd does, for 5 h, then both answered. Result in
  `scratch-tests.out`: while stopped, 44 of 300 rounds began nothing,
  every round from the 257th (index 256) on, with 256 names given up;
  after the uploader was back, the first round that began was the
  third (eleven names marked: eight retired in the first round, three
  in the second with the early leave after them).
  `hours_of_give_ups_leave_room_and_the_publishes_resume_at_once`
  passes because its world has no name published, so nothing is
  marked, and its fake answers the retires whatever the publishes get.
- **Certainty.** High on the code path; the scratch test shows it.
  How often a backlog meets a stop of hours in production I cannot say.
- **Not blocking.** It is bounded, and the ruling's letter ("a few
  hours of give-ups do not stop the publishes for a day") holds. A
  small change would close it: let the early leave take its steps
  before the names marked (`Record::abandoned_over()` is one name a
  round in the steady state), or give it a step of its own. The lead
  decides whether it is in this pull request.

### N2. `ZfsCli::destroy_one` takes a member already gone for a failed destroy (low)

`zfs/cli.rs:1523-1551`. A member destroyed between `destroy_members`'
read of the members (`engine/retain.rs:503`) and its `zfs destroy
pool/ds@t` makes ZFS answer `could not find any snapshots to destroy;
check snapshot names.` and exit 1 (OpenZFS 2.4.1 `zfs_do_destroy`:
the `@` branch opens the dataset, `gather_snapshots` drops `ENOENT`,
and an empty list gives that line). `failure()` (line 1752) maps only
`dataset does not exist` to `NoSnapshot`, so this is `Other`,
`Destroyed::Failed`, and the name is told `not-retired` and moved to
the names stuck (`engine/retain.rs:418` for a name published, 628 for
one given up). `destroy_r` already reads the same line as destroyed
(line 1086, with its doc at 1062).

- **Scenario.** A destroy killed at its limit in an earlier round
  (`NotRun`: "the ioctl may finish in its own time", line 1535) whose
  sync task finishes after this round's read; or a person's destroy
  by hand in the same window. The name is stuck, and its next turn
  in the retries (at most eight a round, in turn) reads the members
  again, finds none of its GUIDs, and retires it.
- **Effect.** A wrong `not-retired` line for a person and a detour
  through the names stuck; no snapshot is lost or kept by it. In the
  first scenario, it is the case decision 4 meant to keep out of the
  names stuck.
- **Certainty.** High on the path from the code and OpenZFS's source;
  I have no recording of ZFS's line for the non-recursive form, the
  rigs see the recursive one. The scratch test
  `review179n_a_member_already_gone_is_destroyed`
  (`scratch-cli-tests.rs`) fails at the head: it gets
  `Err(Other("zfs destroy tank/db@skz-1: exit 1: could not find any
  snapshots to destroy; check snapshot names."))` where it expects
  `Ok(())` (`scratch-tests.out`).
- New in this pull request (`39ef4eb`), not raised in my first review.

### N3. The step after a failure waits for a destroy in flight to its limit (information; ruled)

The answer's bound for M2 is "its 30 s answer limit, plus the note's
read and the reopen" of a retire in flight; the test asserts 35 s with
a fake whose ask costs 30 s. Two notes for the record the lead keeps:

- The client's limit for an ask is `ASK_LIMIT`, 40 s
  (`crates/skyzfs-control/src/lib.rs:275`): 30 s where the daemon
  answers `no-answer`, up to 40 s where it does not answer at all.
- A member's destroy in flight is not cut (`program.rs:286-287` says
  "a step of the engine under way is not cut"). It is a sync task, and
  the notes (`docs/skyzfs-impl-notes.md`, the ublk transport, "A pool
  that only writes does not suspend") measured that on a pool whose
  device was deleted the syncing transaction group never finished.
  So the destroy runs to `Limits::change`, 120 s plus 100 ms a dataset
  (at most 20 min, `zfs/cli.rs:669-683`), is killed, and the reopen
  follows. With a log device, and where zed does not suspend the pool
  first, synchronous writes are acknowledged meanwhile (same
  section). The step's own count (`host.rs`) keeps the reopen from
  being refused after it, which was M2's other half.

The lead's ruling on M2 says "a destroy with a time limit", so this is
what was ruled; I note the figure because the description and the
answer give only the 30 s one. If it matters later: the host's wait
for a retention destroy could read the note each second and kill it
(`NotRun`, so the name is tried again; nothing is lost).

## Test gaps

Survivors of my plants of the new code, most useful first. None
blocks alone; T1 I would take before the merge.

- **T1. `ZfsCli::destroy_one` has no test** (`zfs/cli.rs:1523-1551`).
  It is the only destroy retention runs, and no test calls it: the
  engine's tests use the fake `Zfs`. Three plants survive the agent's
  suite: a destroy not run rewrapped as `Other` (`B-cli-one-other`,
  which undoes decision 4 at the only place it matters for retention:
  a destroy killed at its limit would stick the name), `dataset does
  not exist` taken as an error (`B-cli-one-gone`), and the guard on
  the name loosened from `||` to `&&` (`B-cli-one-guard`, so
  `other/db@skz-1` would be destroyed). The scratch test
  `review179n_the_destroy_of_one_member` (`scratch-cli-tests.rs`)
  passes at the head and fails with each of the three planted
  (`scratch-tests.out`); it can be taken as it is. `destroy_r` has the
  equivalent tests (`B-cli-exec-other` is caught by
  `the_destroy_is_zfs_destroy_r`).
- **T2. A name at `Retire::Members` after a silent retire**
  (`B-silent-all`: `_ if self.retention.silent` for
  `Retire::Ref if ...`, `engine/retain.rs:302`). With it, a retire not
  answered also stops the members' destroys of names whose ref is
  already retired, which ask nothing of the daemon. No test has such a
  name behind a retire not answered in the same round.
- **T3. The stuck names' turn after a destroy not run** (`B-nr-turn`:
  `NotRun` leaves `stuck_last` where it was, line 450). With it, names
  whose destroy the host keeps killing at the limit hold the window of
  eight, and the names after them wait. No test has a `NotRun` in the
  retries of the names stuck.
- **T4. A give-up past its keep among the oldest over room**
  (`B-early-nofilter`: the filter at `engine/retain.rs:564-569`
  removed). With it, such a name is looked up as an early one and,
  with `snap/T` there, its local snapshot is kept instead of
  destroyed. No test has one.

Survivors I read as equivalent where it matters, no test asked:

- `B-halt-noreset` (the round does not reset `halted`) and
  `B-halt-stuck-goes-on` (a halt in the retries moves the turn on): a
  halt comes only from an end asked, after which `run` ends, or from a
  lost note, after which rounds touch nothing but the step. They differ
  only where a lost note goes away while the same process runs on.
- `B-halt-program` (`run` asks `go` once more before retention): an
  extra ask; only the marks, a save, are skipped.
- `G16'`: the command line's suite has no test that `--keep` reaches
  `schedule`, as the answer says; the library half is caught
  (`G16-lib`, three tests).

## Nits

- The description still has a placeholder: "**PLANTFIX** (`LASTSHA`)".
- `engine/retain.rs:6-8` says the description lists the rulings ?1 to
  ?4; it names ?3 and ?4 inline and not ?1 or ?2. The answer says this
  nit was done.
- `program.rs:286` is a doc line of 123 columns.
- `program.rs:280-284` lists where the engine asks whether to go on
  (a sync after a rewind import, a begin after syncs, each pass of the
  settle); retention's asks (`retain_while`, before each step, each
  retry of a name stuck and each call to ZFS) are not in that list.
- `Report::NoGuids` (`program.rs:977-979`) says the name "is forgotten
  after --abandoned-keep-s"; since the early leave it may be forgotten
  sooner, when the give-ups alone are short of room.
- `engine/retain.rs:95-96` ("does not stop the publishes after it is
  back") is true only without a backlog of marked names (N1).

## What the answer claims, checked

### M1 (give-ups filling ABANDONED_MAX)

Fixed as ruled. `Record::abandoned_over` (`record.rs:591-597`) counts
the give-ups over the room only where the publishes under way and the
names published have theirs, and `retain_given_up`
(`engine/retain.rs:552-570`) takes that many of the oldest by `at_ms`,
leaving out those already past their keep (they leave anyway), so
exactly the N oldest go. Each early one is looked up
(`look: early || ...`, line 583): `snap/T` there keeps the local
snapshot and leaves with `forgotten` (the text says "given up", not "by
a person", line 654); absent, its members are destroyed by GUID;
unanswered, it stays and the round's looks stop (`silent`). `Left::
Nothing` and `Left::Unknown` leave as past their keep. A plant of each
part of this is caught (below). The residual is N1.

### M2 (retention between the note and the step)

Fixed as ruled. `retain_while` (line 267) resets `silent` and `halted`
each round; `halt` (line 278) asks `go` and the note and latches. It is
asked before each step of the names published (line 303) and given up
(line 607), before each name stuck retried (inside `destroy_members`),
before each members read (line 500) and each member destroy (line 522).
`run` calls it after the begin and before the settle
(`program.rs:1670-1680`); a halt returns `Ok` and the next round reads
the note first. `Host::run_step` counts the step's children apart
(`host.rs:71`, `run_counted(&STEP_STUCK_NOW, ...)` at 167), and the
reopen goes through it (`zfs/cli.rs:1674-1682`). The residual is N3.

### `ZfsError::NotRun`

`exec` (`zfs/cli.rs:885-894`) maps every error of `host.run`, a
timeout, the cap's `WouldBlock` and a spawn that failed, to `NotRun`;
`destroy_one` and `destroy_r` rewrap it with "may not have been
destroyed". `not_run_or_failed` (`engine/retain.rs:729`) makes it
`Destroyed::NotRun`, which leaves a published name and a given-up name
where they are (lines 419 and 629) and moves the stuck names' turn on
(line 450). So a refusal by the host never sticks a name. One edge,
not a finding: a command that ZFS answered `no such pool` whose
`zpool list` the host then does not run is `Other` by `confirm`'s
documented conservative reading, so a failed destroy; ZFS answered
there.

### L1 to L5 and the decisions

- L1: in `d68471f`, as I said.
- L2: `silent` is set by `no-answer` or no answer to a retire
  (lines 373-397) or a look (lines 659-675), and skips the round's
  remaining retires and looks but not the members' destroys of names
  whose ref is already retired. As ruled.
- L3: documented (`engine/retain.rs`, "A name stopped as
  `Retire::Other`").
- L4: ruled to stay; `set_keep`'s doc says so (`78c3997`).
- L5: `engine.rs:1688-1700`, a take found destroyed is
  `Left::Nothing`.
- Decision 1 and 2: M1 and M2 above. Decision 3: L3. Decision 4:
  `NotRun` above. Decision 5: `PUBLISHED_SLACK` 64, `PUBLISHED_MAX`
  1128 (`record.rs:112-119`); `--keep` keeps its bound of 1000.
  Decisions 6 and 7: documented (`engine/retain.rs` module docs on v2
  names; `engine.rs:2214-2227` on `Left::Nothing`).

### The answer's plant table

Re-run one at a time, each in a clean worktree of the head, restored
after its run (`plants.json`, `runplants.sh`, results in
`plants.out`):

| plants | result here | the answer says |
|---|---|---|
| R15, R16, R16b, R22, E3, E4, E4b, E8 | caught, by the tests the answer names | caught as they were |
| R12', R21', G11' (re-anchored) | caught: R12' by `a_members_read_that_fails_keeps_the_name` and the two `NotRun` tests; R21' by `a_given_up_name_is_told_before_it_leaves`; G11' by five of `program`'s retention tests | caught |
| U1, U2 (`skyzfs-upload --test retire`) | caught by `the_driver_counts_each_retire_by_its_answer` | caught |
| G16-lib (`schedule` drops `set_keep`), and my G16-lib-ab (drops `set_abandoned_keep`) | caught, the first by three tests, the second by `run_takes_its_abandoned_keep` | "caught by three of the library's tests" |
| G16' (`main.rs` passes `Kept::default()`; the CLI's suite) | survives | survives |
| M5, Q8, Q9 | caught by `the_turn_goes_by_name_holds_across_a_restart_and_clears_one_name` | same |
| M7 | caught by `a_name_stuck_keeps_all_its_members` and two more | same |

So the table is honest, re-anchored rows included. My own plants of
the code since e6530f1 (38, `B-*` in `plants.json`): 28 caught, one
refused by the compiler (`B-slack0`: the const assertion
`PUBLISHED_SLACK >= 64` in `record/tests.rs:446`), nine survive (test
gaps above). In all, 58 plants: 47 caught, 1 refused by the compiler,
10 survive. The baselines passed first (the agent's library 356,
`skyzfs-upload`'s `retire` 11, the command line's suite 7); each
plant was restored after its run, and the tree was clean at the end.

## Gates

At 78c3997, rustc 1.100.0 (current stable after `rustup update
stable`), the three system packages installed, `gates.sh`, results in
`gates.out`:

| features | clippy `-D warnings` | `cargo test --workspace` | `cargo doc`, `-D warnings` |
|---|---|---|---|
| none | clean | 2146 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean | 2205 passed, 0 failed, 16 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | clean | 2146 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | clean | 2170 passed, 0 failed, 18 ignored | clean |

`cargo fmt --all -- --check` is clean. The agent's two crates
(`-p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks`) passed five more times, 363 of 363 each.
The figures are the description's.

## CI at 78c3997

`gh api repos/skyzfs/skyzfs/commits/78c3997/check-runs` and the runs at
that sha, read from their summaries and artifacts, not their colour:

| workflow | result |
|---|---|
| `ci` (lint, test, unit-arm64, s3-backends, chart-lint), `coverage`, `e2e`, `k3s-smoke`, `skyzfs-ci` (gate) | success |
| control rig (3 images) | every line as `EXPECTED` |
| cut rig on the daemon (26.04, 26.04-arm, 26.04-upload) | every course recorded and checked, exit 0 |
| publish rig (26.04) | 11 courses checked, exit 0, `pretire` and `pkillret` among them |
| replay rig (both layouts, and through skyzfsd on 24.04 and 26.04) | every control as `EXPECTED`; through skyzfsd, the pull request's budget (120 of 2079 and 121 of 2123 prefixes), so design §9's criterion is not shown by this run, as the summary says |
| daemon rig (daemon on 24.04, 26.04, 26.04-arm; stand-in on 24.04, 26.04) | "Every pin and every control is as the table `EXPECTED` has it"; `pin_failed_volume_while_a_request_is_held` WAITING, as expected |
| agent rig | **failure, the rig's own error, not a verdict:** part `a2` (aplain's second shard) failed at installing its packages, exit 2: `packages.microsoft.com` answered 403 to the runner image's apt source in three tries. `tests/lib/apt-fallback.sh` replaces only Ubuntu mirrors whose names do not resolve, so it did not apply. The other parts (`a1`, `d`, `k`, `r`, `f`, `fz`) recorded and checked with exit 0. Job `a2` needs a re-run before CI is green. |

One point for the lead's record, not against this pull request: no
kernel rig drives the agent's retention at this head. Each agent rig
part lists `retiring`, `retire`, `retired`, `look-given-up`,
`destroy-member` and `retain` among its points not reached (`a2` is
`aplain`'s other shard, so it would not either), and the publish rig's
retire courses drive the daemon's `retire` from the rig's own client.
Retention's evidence is the agent's unit and program tests and the
plants above.

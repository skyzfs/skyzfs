# Review of skyzfs/skyzfs#170 (M2.7d) at bb03454

Head checked with `gh api repos/skyzfs/skyzfs/pulls/170 --jq .head.sha`
at the start and at the end: `bb034546b34be39e665cd869bb6a8de220b61e85`
both times. Two commits on the base fe68a59: 4936585 (the change) and
bb03454 (a merge of `zfs-topology` at fe68a59, no Rust).
`zfs-topology` has since moved to 47c11b0, which changes one line of
`docs/skyzfs-design.md` (the lead's rewording of §5.2 for this
package); the pull request merges onto it cleanly.

I read CLAUDE.md, design §5.2, §5.4 and §9 (M2), the implementation
notes on the agent, and the review notes of M2.7a, M2.6c-5e and
M2.6c-5f before the description. Then `git diff
origin/zfs-topology...pr170`, every changed line against the code
around it, the plants, the gates, a signal driver on the binary, and
last the description, claim by claim.

Files beside this one:

- `plants.py`, with `plants-author.out` (the author's seventeen,
  re-made, and the review's own, against the head's tests) and
  `plants-both.out` (the plants that survived, against the head's
  tests plus the review's);
- `scratch-tests.rs`, the review's tests (not for merging as they
  are), with `scratch-tests-head.out` (the two that fail at the head);
- `gates.sh` with `gates.out`;
- `live.sh` with `live.out`.

The scripts take the scratch directory from `$SCRATCH`; the outputs
print it as `$SCRATCH`.

## Verdict

**It can merge from review's side. Nothing blocks.** The code does
what the description and the design say, the seventeen plants are all
caught, the gates are clean on all four feature sets, and the binary
ends promptly at SIGTERM (n1 of #167 is closed: one call after the
signal, against three at 28782cd).

Two low defects, neither a loss and neither in the crash matrix:

- **L1**: an end asked for during the round's last sync (or during the
  first read of `cuts`) still lets the round begin a publish, with its
  count, sweep and reads and a save of its intent, before the settle
  asks `go`. The docs say a signal ends `run` once its step is done.
  A one-line fix.
- **L2**: a round whose syncs fail after some ran says `synced=0`.

Then test gaps (mutants of the new code that the suite does not
catch: thirteen of the review's nineteen, among them two that break
the bound, and the error arm of the syncs in `run`, which no test
reaches), and doc nits, one of them a
description claim that is not so (the ragged break in `engine.rs` is
not fixed).

Four points for the lead (end of this file), none blocking; the first
is a mismatch between the design's new words ("at most 128 times per
process") and the code (128 without a cut that opens as itself).

CI on bb03454: see "CI" below. Codex did not review this head: its
summary comment says the review failed (usage limit). There are no
Codex findings to answer.

## The change against the code

### `Agent::sync_past` (`engine.rs:773-811`)

The loop reads `cuts`; an unread daemon or another reply is `Unseen`.
A newest cut that opens as itself, or none, is `Clear` and sets the
process's count (`past_synced`) to 0. Otherwise, in this order: `NoCut`
where a sync ran in this call and the count did not rise since the
last sync (`count_before` is the count read just before the last
sync); `Bound` where `past_synced` has reached 128; `Told` where `go`
says no; the premise on the first sync of the call; then one assist,
counted toward the bound before it runs.

Crash matrix: nothing in it touches the record's directory. A kill
anywhere leaves at most groups written and a value of `skyzfs:assist`;
the next process starts the count at 0 (the bound is per process). An
engine restart keeps `past_synced` (`engine.rs:648-659` does not clear
it) and clears `assists`, whose next value is the clock's ms anyway.
No path is new to the crash matrix.

Each stop, judged:

| Stop | Right? | Note |
|---|---|---|
| `Clear` on `Some((_, true))` | yes | Resets the bound. |
| `Clear` on `None` (no cut held) | yes | Nothing can be behind. It also resets the bound; the docs name only "a cut that opens as itself" (d2). |
| `NoCut` | yes | Judged from the last sync, not the first (a plant of the first survives: T1). It cannot fire before the first sync. A daemon restart between two syncs that lowers the count (a new state) also stops the round; the next round goes on. Harmless. |
| `Bound` | yes, as a per-process bound | Checked after the cut is read and before `go`. 128 is enough: `-T` loads an uberblock in the ring, so at most 127 abandoned groups are above it (ashift 9: 128 slots of 1 KiB; ashift 12: 32), each assist writes at least one group, and the `Clear` check runs before the bound check, so the 128th read can still clear. A failed assist counts (T1). |
| `Told` | yes | Asked before each sync and after the bound. |
| Premise | yes | Checked once per call, before the first sync: a pool the agent refuses is never written. The live driver shows it: with the daemon answering `opens=rewind` and `zpool get` refused, no `zfs set` or `zpool sync` ran (`live.out`, cases f and g). |

### `run` (`program.rs:1017-1150`)

- Each round calls `sync_past` (unless a restart is still owed), and
  begins only on `Clear` or `Unseen` (`program.rs:1082`). `Behind`
  prints one line on standard error and the round is still `ok`
  (`program.rs:1057-1066`). An error is handled as a begin's error,
  with the restart at once where `restarts` says so
  (`program.rs:1067-1079`); a refusal of the premise does not restart.
- The settle runs in every case but an owed restart, so a round behind
  still settles what is under way (T1: a plant that skips it survives
  the suite).
- The round line gains ` synced=<k>` at its end. Nothing in the rigs
  reads the round line: `tests/kernel/agent_check.py` reads `snap/` and
  `abandoned` lines only, and `agent-record.sh` runs `publish`,
  `settle`, `recover` and `record`, never `run`. So no rig parses the
  new field, and no rig runs `sync_past` (below, "Rigs").
- Exit codes: unchanged. `sync_past`'s errors are `Zfs`, `Daemon`
  (through the premise) or `Premise`, none of which `ends_run`.

### D2 / n1: `go` at the top of a pass (`engine.rs:878-921`)

`go` is now asked before the count and the sweep. Nothing needed is
skipped by it: `settle`, `publish` and `recover` pass `|| true`; in
`run`, a pass not run leaves its publishes in the record for the next
start, whose first round counts and sweeps. `unsettled()` returns `Ok`
where nothing is under way, as the old code did by its empty check.

Driven for real (`live.sh`, `live.out`: the debug binary at bb03454,
fake `zfs` and `zpool` on `PATH` that answer "refused" after a sleep,
and for cases f and g a fake control socket that answers every `cuts`
with a newest cut of group 40 and `opens=rewind`):

| Case | Signal | Exit | Signal to exit | Calls begun after the signal |
|---|---|---|---|---|
| a | TERM while waiting for a tick | 0 | 0.08 s | 0 |
| b | INT while waiting | 0 | 0.08 s | 0 |
| c | TERM 1 s into a 3 s call (the begin's count) | 0 | 5.0 s | 1 (the begin's own `zpool get`) |
| d | TERM, then TERM 0.5 s later, in a call | 1 | 0.004 s after the second | 0 |
| e | TERM, then INT, in a call | 1 | 0.004 s after the second | 0 |
| f | none until 4 s; the newest cut behind | 0 | 0.05 s | 0 |
| g | TERM 1 s into the premise's 3 s `zpool get`, the newest cut behind | 0 | 2.05 s (the call's end) | 0 |

At 28782cd case c ran three calls after the signal and ended 11.1 s
after it (`review/m2.7a/next_look/live.out`); now one, the begin's
own. In f and g the round read `cuts` first, found the cut behind,
read the premise, which the fake refused, and ran no `zfs set` and no
`zpool sync`: a pool whose premise cannot be read is not synced. The
round was `failed` with `begun=- ... synced=0`.

### The doc fixes asked for

| Item | Where | Judged |
|---|---|---|
| `Error::Io` doc (#167 n4) | `engine.rs:385-386` | Right: every `Io` is the record, a manifest, the listing or the marker. |
| restarts' timing (#167 n5) | `program.rs:921-926` | Right for begin and settle, but it misses the new third case: an error of the syncs restarts at once too (d3). |
| failed reading (#167 n6) | `program.rs:94-100` | Right. |
| D1's cost | `program.rs:103-107` | Present and right. |
| 100-column line (#167 n7) | `engine.rs` settle docs | Gone. No added line in the diff is over 100 columns. |
| "nor hide one" (#168) | `cli.rs:1076-1084`, `1222-1228` | Right now: never hides, forges only under the accepted race. |
| 103-column line (#168) | `cli.rs:541-542` | Fixed. |
| ragged break (#168) | `engine.rs:982-983` | **Not fixed** ("publish. One agent runs per pool, and" / "within it ..."), though the description says it is (d5). |

## Findings, most severe first

### L1 (low, defect against the docs): an end during the last sync still begins a publish

`program.rs:1049-1082`. `sync_past` asks `go` only before a sync. Where
the end comes during the sync after which the newest cut opens, the
next `cuts` read returns `Clear`, and the round goes straight to
`agent.begin()` (`program.rs:1082`), which asks no `go`: it counts,
sweeps, reads the pool's properties and the premise, enrols, reads
`cuts`, finds a name and saves the intent. Only then does
`settle_while` ask `go` and end the round.

The review's test `rp1_an_end_during_the_last_sync_begins_no_publish`
(`scratch-tests.rs`; rewind gap 5, three syncs, `go` says no from the
fifth question on) fails at the head:

```
calls after the end was asked: ["count", "props", "tagged", "props", "cuts", "members"]
  left: "skz-20261125T010000Z"
 right: "-"
```

The same holds where the end comes during `sync_past`'s first read of
`cuts` (every round, rewind or not) or during the premise's reads
before the first sync where the pool is behind: there, the first
assist runs too (`go` was asked before the premise).

Cost: the end waits for a whole begin (each call within its own limit,
one more daemon read among them) and leaves one more publish under
way in the record for the next start. Nothing is lost and no path is
new to the crash matrix, since a begin can always be cut by a kill.
But the module docs (`program.rs:147-153`) say a signal ends `run`
"once its step is done (a begin, or a pass over the publishes under
way), before the count and the sweep that would open the next", and
`schedule`'s docs (`program.rs:1010-1013`) list where `go` is asked;
neither holds for a sync followed by a begin. Before this package the
window was the gap between the loop's `go` and the begin's first call;
the syncs and the read of `cuts` widen it.

Fix: `if !restart && clear && go() {` at `program.rs:1082` (a round
that is told to end begins nothing; its settle then ends at its own
`go`), and the docs say that a sync is a step. I tried it: RP1 passes,
and so does every agent test but one, the author's
`an_end_during_a_begin_makes_no_read_after_it`, whose `go` says yes at
the round's start only (`asked == 1`, `program/tests.rs:1454-1457`):
with the fix the begin is never reached. Given one more yes
(`asked <= 2`, the end then coming while the begin works, as its doc
says) it passes, and it still fails under A14. Certain (shown by a
test at the head).

### L2 (low, defect): a round whose syncs fail after some ran says `synced=0`

`program.rs:1067-1079`. On `Err` from `sync_past` the syncs it had run
are lost: `Error` carries no count, and `synced` stays 0. A round whose
second assist timed out ran one sync, and its line says `synced=0`.
The review's `rp2_a_round_whose_sync_failed_says_the_syncs_it_ran`
fails at the head:

```
round 1 failed took_ms=0 late_ms=0 skipped=0 begun=- under_way=0 synced=0
skyzfs-agent: round 1: zfs: zpool sync: timed out
```

The module docs (`program.rs:84-85`) say `synced` is "the syncs the
round ran". Only tooling reads it, and no rig does yet, so it is low.
Fix: keep the call's count in the engine (for example a getter of the
last call's syncs), or return it beside the error. Certain.

### T1 (test gaps): mutants of the new code the suite does not catch

The author's seventeen are all caught (below). Of the review's
nineteen (R1-R17, R19, R20), thirteen survive the head's suite. Each
is a change of behaviour that a later edit could make with every test
green:

| Plant | What survives | Killed by (review's test) |
|---|---|---|
| R1 | The bound reset only by a cut that opens, not by "no cut held" (`engine.rs:782-785`). | RV4 |
| R3 | `Unseen` begins nothing: a daemon unread at the syncs would skip the begin and its error line. | RP6 |
| R5 | `NoCut` judged against the call's first count instead of the last sync's. | RV5 |
| R6 | A failed assist not counted toward the bound: a pool whose sync always fails would be synced once a round for ever. | RV2 |
| R7 | The premise checked only once per process (while `past_synced` is 0). | RV3 |
| R8 | An error of the syncs never restarts the engine. | RP3 |
| R9 | An error of the syncs still begins a publish. | RP4 |
| R10 | A failed reading after an error of the syncs still settles (the restart's rule broken). | RP5 |
| R11 | `NoCut` resets the bound count: where the daemon counts no cut, one sync a round for ever. | RV1 |
| R12 | A refusal of the premise at the syncs restarts the engine. | RP8 |
| R16 | The bound checked after `go`. | none: near-equivalent (only which reason is printed where both hold, and one more `go`) |
| R19 | `settle_while` at an end returns `Ok` even with publishes under way. | RV6 (equivalent within `run`, which treats both alike; it breaks the method's documented result) |
| R20 | A round still behind settles nothing. | RP7 |

The review's scratch tests (`scratch-tests.rs`: RV1-RV6 in
`engine::tests`, RP3-RP8 in `program::tests`) pass at the head and
kill each of these but R16 (`plants-both.out`). The `Err` arm of the
syncs in `run` (`program.rs:1067-1079`) is reached by no test of the
head at all (R8, R9, R10, R12). R6 and R11 matter most: each breaks
the bound. The author may take the review's tests as they are or write
their own.

### d1-d7 (nits): docs and the description

- **d1** `engine.rs:81`: "(the daemon's `opens=no`)". The control
  protocol says `opens=rewind` (`crates/skyzfs-control/src/lib.rs:45`,
  `:57-60`, `:710`).
- **d2** `program.rs:129-132`: "until they see a cut that opens as
  itself (a change of the pool's own, or a person's `zpool sync`, makes
  one)". A bare `zpool sync` on an idle pool writes no group (the
  description's own open question 6, and design §5.2 now). And a
  daemon that holds no cut also starts the count again
  (`engine.rs:782-785`); `past_synced`'s doc says so ("since it last
  saw no cut behind"), the module docs do not.
- **d3** `program.rs:921-926` (`restarts`) and `program.rs:1002-1009`
  (`schedule`): the restart comes "at once after a begin's error" and
  "at the next round after a settle's error"; an error of the syncs
  restarts at once as well (`program.rs:1067-1079`). And
  `program.rs:1010-1013`, where `go` is asked: now also before each
  sync.
- **d4** `program.rs:80-82`: `begun=-` lists "its begin failed, or the
  round's syncs left the daemon's newest cut still behind". A round
  whose syncs failed (an error, `failed`) begins none either and is in
  neither case.
- **d5** (description) "The 103-column line in `cli.rs` and the ragged
  break in `engine.rs` are fixed." The first is; the second is not
  (`engine.rs:982-983`). Cosmetic.
- **d6** (description) P15 is said to be caught by
  `the_holds_under_a_snapshot_are_checked_as_the_sweeps_are`. True for
  the advice dropped from the missing-snapshot error
  (`ODD_HOLDS` at `cli.rs:1130-1133`; my A15b); the advice in
  `outnumbered`'s message (`cli.rs:541-542`; my A15) is caught by
  `holds_that_do_not_add_up_are_an_error_that_names_the_snapshot`
  instead. Both caught.
- **d7** (description) P14, "`go` asked after the count (n1 undone)",
  is said to be caught by `an_end_comes_between…` and
  `an_end_during_a_begin…`. In both forms I made (A14: `go` after the
  count and the sweep; A14b: where it was before the change, after the
  count, the sweep and the empty check) only
  `an_end_during_a_begin_makes_no_read_after_it` fails. Caught either
  way; the table's first name may be from another form of the plant.

## The plants (`plants.py`)

Each plant alone, against `cargo test -p skyzfs-agent -p
skyzfs-agent-cli --tests` (the library's tests, the binary's and its
integration tests), restored after each, 600 s each. The baseline
passes: 243 + 0 + 7.

**"17 of 17 plants are caught": confirmed.** I re-made the seventeen
from the table's one-line descriptions (A1-A17); every one is caught,
by the tests the table names or a subset of them
(`plants-author.out`). A14 and A15 differ from the author's forms
(d6, d7); A14b and A15b are the forms closest to theirs, and both are
caught.

The review's nineteen (R1-R17, R19, R20; R18 dropped): R2, R4, R13,
R14, R15 and R17 are caught by the head's suite; the other thirteen
survive it (T1 above), and the review's tests kill all of them but
R16 (`plants-both.out`).

In the run with the review's tests (`plants-both.out`), `rp1` and
`rp2` fail at the head by design (L1, L2) and are not counted as a
catch; since they fail in the library's test binary, cargo stops there
and the binary's tests do not run in that mode. Every review test is
in the library.

| Plant | Caught by the head's suite (`plants-author.out`) | Review's tests that catch it (`plants-both.out`) |
|---|---|---|
| A1 no syncs | `a_sync_that_makes_no_cut_ends_the_syncs`, `after_a_rewind_import_the_agent_syncs_until_a_cut_opens_as_itself`, `an_end_stops_the_syncs`, `the_syncs_stop_at_their_bound_until_a_cut_opens`, `a_round_after_a_rewind_import_syncs_past_it_before_it_begins`, `a_round_still_behind_begins_no_publish` | - |
| A2 one sync only | `after_a_rewind_import_the_agent_syncs_until_a_cut_opens_as_itself`, `an_end_stops_the_syncs`, `the_syncs_stop_at_their_bound_until_a_cut_opens`, `a_round_after_a_rewind_import_syncs_past_it_before_it_begins`, `a_round_still_behind_begins_no_publish` | - |
| A3 a sync while the newest cut opens | `after_a_rewind_import_the_agent_syncs_until_a_cut_opens_as_itself`, `the_syncs_stop_at_their_bound_until_a_cut_opens`, `with_no_cut_behind_nothing_is_synced`, `a_round_after_a_rewind_import_syncs_past_it_before_it_begins`, `an_end_during_a_begin_makes_no_read_after_it` | - |
| A4 no bound | `the_syncs_stop_at_their_bound_until_a_cut_opens`, `a_round_still_behind_begins_no_publish` | - |
| A5 the bound per call | `the_syncs_stop_at_their_bound_until_a_cut_opens`, `a_round_still_behind_begins_no_publish` | - |
| A6 the count not reset by a cut that opens | `the_syncs_stop_at_their_bound_until_a_cut_opens` | - |
| A7 the count reset by a restart | `the_syncs_stop_at_their_bound_until_a_cut_opens` | - |
| A8 no NoCut check | `a_sync_that_makes_no_cut_ends_the_syncs` | - |
| A9 go not asked in the syncs | `an_end_stops_the_syncs` | - |
| A10 the premise not checked | `a_pool_refused_its_premise_is_not_synced` | - |
| A11 a begin while behind | `a_round_still_behind_begins_no_publish` | - |
| A12 no sync before the begin | `a_round_still_behind_begins_no_publish`, `a_round_after_a_rewind_import_syncs_past_it_before_it_begins` | - |
| A13 synced left out of the line | `a_round_line_says_what_the_round_did`, `a_round_after_a_rewind_import_syncs_past_it_before_it_begins`, `a_round_still_behind_begins_no_publish` | - |
| A14 go asked after the count and sweep (n1 undone) | `an_end_during_a_begin_makes_no_read_after_it` | - |
| A14b go asked where it was before the change (after the count, the sweep and the empty check) | `an_end_during_a_begin_makes_no_read_after_it` | `rv6_an_end_before_a_pass_names_what_is_under_way` |
| A15 N3a the advice dropped | `holds_that_do_not_add_up_are_an_error_that_names_the_snapshot` | - |
| A15b N3a the advice dropped from the missing snapshot error | `the_holds_under_a_snapshot_are_checked_as_the_sweeps_are` | the head's, as before |
| A16 A9e the fake filter takes skz- anywhere | `the_sweep_leaves_a_hold_of_the_tag_on_a_name_not_the_agents` | - |
| A17 a round still behind marked failed | `a_round_still_behind_begins_no_publish` | - |
| R1 the count reset only by a cut that opens, not by no cut | **survives** | `rv4_no_cut_held_starts_the_count_again` |
| R2 no cut held taken as behind | `with_no_cut_behind_nothing_is_synced` | - |
| R3 the daemon unread: no begin | **survives** | `rp6_the_daemon_unread_still_begins` |
| R4 NoCut only where the count fell | `a_sync_that_makes_no_cut_ends_the_syncs` | - |
| R5 NoCut against the first count, not the last | **survives** | `rv5_no_cut_is_judged_from_the_last_sync` |
| R6 a failed sync not counted toward the bound | **survives** | `rv2_a_failed_sync_counts_toward_the_bound` |
| R7 the premise checked only while the bound count is 0 (once per process) | **survives** | `rv3_the_premise_is_checked_at_each_call` |
| R8 the syncs error does not restart | **survives** | `rp5_a_failed_reading_after_the_syncs_error_settles_nothing`, `rp3_an_error_of_the_syncs_reads_the_record_again` |
| R9 the syncs error still begins | **survives** | `rp4_a_round_whose_syncs_were_refused_begins_nothing` |
| R10 a failed restart after the syncs error still settles | **survives** | `rp5_a_failed_reading_after_the_syncs_error_settles_nothing` |
| R11 NoCut resets the bound count | **survives** | `rv1_no_cut_at_each_call_still_meets_the_bound` |
| R12 the syncs error restarts even on a refusal | **survives** | `rp8_a_refusal_at_the_syncs_reads_the_record_once` |
| R13 synced of a round left at 0 where behind | `a_round_still_behind_begins_no_publish` | - |
| R14 the stderr line of a round behind not printed | `a_round_still_behind_begins_no_publish` | - |
| R15 go asked only before the first pass of a settle | `an_end_comes_between_two_rounds_of_the_engine` | - |
| R16 the bound checked after go (order) | **survives** | **survives** |
| R17 the sync value not kept for the planner | `after_a_rewind_import_the_agent_syncs_until_a_cut_opens_as_itself`, `the_syncs_stop_at_their_bound_until_a_cut_opens`, `a_round_after_a_rewind_import_syncs_past_it_before_it_begins`, `a_round_still_behind_begins_no_publish` | - |
| R19 the unsettled names dropped at an end (Ok always) | **survives** | `rv6_an_end_before_a_pass_names_what_is_under_way` |
| R20 a round whose syncs did not clear settles nothing | **survives** | `rp7_a_round_still_behind_still_settles` |

## Gates (`gates.sh`, `gates.out`)

rustc 1.100.0 (`rustup update stable` first), in a scratch worktree at
bb03454, the target cleaned between feature sets.

GATES_TABLE

`cargo fmt --all -- --check`: clean. The agent crates' tests twenty
times: 0 failed runs (250 tests a run). The host tests
(`cargo test -p skyzfs-agent --lib host::`) twenty times: 0 failed runs.

## CI

On bb03454, 42 check runs succeeded and 1 was skipped ("Publish
badges", which runs on `main` only); none failed. The workflow runs:
`ci` 45907325720 (lint, test, unit-arm64, s3-backends, chart-lint),
`coverage` 45907325779, `e2e` 45907325825, `k3s-smoke` 45907325821,
`skyzfs-ci` 45907308902 (the push), the agent rig 45907325673 (a1, a2,
d, k on ubuntu-26.04), the publish rig 45907325822, the daemon rig
45907325700 (on `skyzfsd`, three images), the control rig 45907325687,
the cut daemon 45907325685 and node B 45907325690. I read the
conclusions, not each rig's summary table: none of the rigs runs the
code this package changes (below), so their tables say nothing about
it.

Codex: its summary comment on the pull request says the review of
bb03454 failed (a usage limit). There are no Codex findings.

### Rigs

The pull request changes `crates/skyzfs-agent/` and `Cargo.lock`.
`Cargo.lock` triggers the daemon rig and the control rig, which do not
run the agent. The agent rig ("the agent through skyzfsd", courses a1,
a2, d, k) and the publish rig run on it, but they run `publish`,
`settle`, `recover` and `record`, never `run`, and no rig makes a
`-T` rewind import under the Rust agent. So `Agent::sync_past`, the
round's new line and its new stderr line run only in the unit tests
(`test`, `unit-arm64`, `coverage`). The description says so ("Not
shown on a real kernel"). On a real pool, what is unshown is whether
the daemon has counted the sync's cut by the time `zpool sync`
returns (else every round stops at `NoCut` after one sync: the
description's open question 4), and whether 2 groups per assist holds.

## The description, claim by claim

- Commits, base and the merge's content: as stated (the merge adds
  `tests/kernel/`, a workflow and the notes only).
- `sync_past`'s returns, stops, premise, bound and its derivation from
  `skyzfs-uberblock` (a dev-dependency, pinned by
  `the_bound_is_the_most_slots_a_ring_holds`): as stated.
- "The count of syncs toward the bound is kept across
  `Agent::restart` and starts again only when a cut opens as itself":
  also when the daemon holds no cut (d2).
- "`sync_past` saves nothing in the record and has no crash points":
  true.
- `run`: as stated, but "An error from `sync_past` ... fails the round"
  loses its syncs from the line (L2).
- D1, D2/n1, n4-n7: as stated, except the restarts' docs miss the
  syncs' error (d3).
- #168: N3a and A9e as stated; the ragged break is not fixed (d5).
- Tests: the seven `past` tests and the three `program` tests do what
  the description says; each fails under the plants it names.
- Gates: plain `cargo test --workspace` 2003 / 0 / 16, as stated;
  GATES_CLAIM
- "17 of 17": confirmed (above).

## Decisions for the lead

None blocks the merge.

1. **The design's words and the bound.** Design §5.2 at 47c11b0 says
   the agent syncs "at most 128 times per process". The code syncs at
   most 128 times per process *without seeing a cut that opens as
   itself, or no cut* (`engine.rs:782-785`): a process that sees a cut
   open and then meets a second rewind import syncs up to 128 more.
   This is the author's open question 3, which the lead accepted, so
   the design's words are what should change ("at most 128 times in a
   process without a cut that opens as itself"). The same sentence
   says "a group at a time" and then "about two groups" an assist.
2. **A per-process bound and a supervisor.** The bound is not saved.
   A `run` that ends with 6 (a corrupt record, another volume's, a
   hold with no marker) after its first round, under a supervisor that
   starts it again, syncs up to 128 times at each start, before the
   sweep that ends it, while the newest cut stays behind. That needs a
   pool whose cuts never open as themselves although the daemon counts
   them, so it is unlikely; the lead may accept it as is.
3. **`ok` while behind.** A round stopped at `Bound` is `ok`, every
   round, for as long as the process lives; only standard error says
   why no publish begins. Monitoring that keys on `failed` does not see
   a pool that publishes nothing. The lead accepted "the round is still
   `ok`"; the question is only whether `Bound`, which says that
   "something else keeps the newest cut from opening", should be
   `failed`. Note also that the new line has the prefix of the error
   lines (`skyzfs-agent: round <n>: `), so a reader that takes every
   such line for an error of the round sees one in a round marked
   `ok`.
4. **No time bound on the syncs.** A round may run up to 128 assists
   before its begin, each within its own limit; the round then overruns
   its tick and later rounds are counted as skipped. That is the
   scheduler's rule ("no overlap"), so it is consistent; it is named
   here only so that the choice is seen.

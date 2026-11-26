# Next look at skyzfs/skyzfs#167 (M2.7a) at 28782cd

Head checked with `gh api repos/skyzfs/skyzfs/pulls/167 --jq .head.sha`
at the start and at the end: `28782cddb8423a5a05c9d34a60266e0075e32c8d`,
one commit on 16f3b3b ("agent run: tests of the restart's paths; docs
after review"). The base is still 9e70246.

I read `git diff 16f3b3b 28782cd` and checked each changed doc line
against the code, ran the plants and the gates, and drove the binary
with signals, before I read the author's answer (comment 7203013747)
and the description. The lead's rulings are not in
`docs/skyzfs-impl-notes.md` on `zfs-topology` (no line on M2.7a there
yet); I took them as the author's answer and the description quote
them.

Files beside this one: `plants-28782cd.py` with its outputs
(`plants-28782cd-author.out`, `plants-28782cd-author-a5.out`,
`plants-28782cd-both.out`),
`gates-28782cd.sh` with `gates-28782cd.out`, `live.sh` with `live.out`.

## Verdict

**It can merge from review's side.** Nothing blocks, and CI is green on
28782cd. G1 is closed: the four tests are ported verbatim, each fails
under its plant, and the new test of another volume's record fails
under its own. The lead's D1 to D4 are applied as the answer states
them. The commit changes one thing outside tests and docs, the text of
`Error::Io`, and that change is right.

What is left is docs and wording, all low:

- n1: the new signal docs say a signal ends `run` "once its step is
  done". The count and the sweep that open the next pass run too,
  before `go` is asked. The binary shows it: 2 of the 3 calls after a
  SIGTERM came after the begin had ended.
- n2: the description's "What was done" still says the restart comes
  at the next round after any error. Since 16f3b3b, after a begin's
  error it comes at once.
- n3: the answer's reason for leaving the recount alone is not right,
  though leaving it is fine.
- Four small doc nits (n4 to n7).

CI on 28782cd is green: 41 succeeded and 4 were skipped. Codex found
nothing at 28782cd.

## What 28782cd changes outside tests

`git diff 16f3b3b 28782cd` touches `engine.rs`, `program.rs` and
`program/tests.rs`. Outside the tests there is one change of behaviour:

| Where | Change | Judged |
|---|---|---|
| `engine.rs:340` | `Error::Io` prints "the record's directory: {e}", not "a manifest's file: {e}" | Right. Every `Io` comes from the `Keep`: `load`, `manifest_names`, `write_manifest`, `read_manifest`, `remove_manifest`, `marked` (`engine.rs:862`, `:1317`, `:1461`, `:1732`, `:1821`, `:1829-1831`). Each is a file in, or a listing of, the record's directory. Nothing read the old text: `git grep` finds no rig, script or test that matched it. Two of the new tests check the new one. |

Everything else outside the tests is doc comments: the module docs of
`program.rs` (lines 34-40, 77-115) and the reflow of `settle`'s docs
(`engine.rs:737-743`). `schedule`, `restarts`, `ends_run` and
`settle_while` are byte for byte as at 16f3b3b.

## G1: the ported tests

**Not weaker than mine.** `program/tests.rs:1761-2035`, module
`restarts`. Against `scratch-tests.patch`, run through rustfmt:

- `Flaky`, `flaky`, `sched_any`, `forget_fails` and `settle_save_fails`
  are mine, line for line. The one change is that `spoil` became an
  enum, `Spoil`, with a `Volume` case.
- RC, RF, RI and RJ have the same bodies and asserts. The only change
  is the error text "the record's directory: " in RC and RJ, which
  follows the `Io` change.
- The author did not take RA, RB, RD, RE, RG or RH. D0 asked for none
  of them.

**Plants at 28782cd** (`plants-28782cd.py`). Each plant was applied
alone to the head, run against `cargo test -p skyzfs-agent --lib` with
a 600 s limit, and restored. The baseline passes 228 of 228.

| Plant | Killed by (the head's suite) |
|---|---|
| N3: a failed restart after a begin's error lets the round settle | `a_restart_that_fails_leaves_the_round_alone` (RC) |
| N4: a failed restart at a round's start goes on with the round | `a_failed_restart_at_a_round_start_leaves_the_round_alone` (RJ) |
| N5: a settle's error does not restart | `a_settle_whose_save_failed_restarts_at_the_next_round` (RI), and RJ |
| P2: only `Unmarked` ends `run` | the corrupt test (RF), the other-volume test |
| P2c: `Corrupt` does not end `run` | the corrupt test (RF) |
| P2v: `Volume` does not end `run` | the other-volume test |
| P8: `loaded` takes another volume's record as its own (`engine.rs:1823-1825` removed) | the other-volume test, alone |
| N1: 16f3b3b's fix out (the begin's restart after the settle) | `a_begin_whose_save_failed_…`, RC, RF, the other-volume test |
| N2: the limit asked before the first pass | `a_round_settles_once_however_long_the_count_took` |
| N6: every begin's error restarts at once, refusals too | `a_begin_refused_for_eight_under_way_still_settles` |
| A5 (the author's): `restarts` true for every error | `a_begin_refused_for_eight_under_way_still_settles` (`plants-28782cd-author-a5.out`) |
| N7: a begin's error that restarts skips the settle | survives, as before: it is the other safe design (F1 of the first look) |

**With my originals beside them** (mode `both`: my scratch tests added,
with their two error texts moved to the new words). Each of N3, N4,
N5, P2, P2v and P2c kills a ported test exactly when it kills the
original it came from. The baseline then fails only RE, which is F5's
demonstration and was expected to fail until M2.7b.

**The new test, `a_record_of_another_volume_at_a_restart_ends_run`.**

- The failing save replaces the record with `Record::new(OTHER)`
  encoded. The restart's `loaded` then returns `Error::Volume`
  (`engine.rs:1823-1825`).
- It asserts exit 6, exactly one round line, `round 1 failed`, and
  "round 1: the record is of volume eeee…" on standard error. That
  pins both the path (a restart within the round) and the variant.
- It is sound and not vacuous: P2v and P8 each fail it alone.
- It also closes an older gap. P8, the volume check in `loaded`
  removed, fails nothing else in the agent's library. Before this
  commit, nothing tested that check; the only other test of `Volume`
  is the exit-code map (`program/tests.rs:537`).

**Fragility.** The tests count loads (`fail_load` 2 is the second load,
the first being `Agent::new`'s). They are not vacuous if the count
shifts, though. Each asserts the round and the text of the error it
expects ("round 2: the record's directory: the record could not be
read"), so a shift fails loudly. They run on the fake clock and take
under a second. The host tests ran 20 times with no failure.

**What is still untested** (unchanged from the first look, not for this
pull request):

- P3, "`forget` drops the stops of publishes under way", and P4, "the
  `Forget` step keeps the publish's memory", survive the head's suite.
  My RG and RH catch them. Both concern the memory staying bounded,
  which the description claims.
- The author's R2 and F3 survive, as the description says.
- N7 survives (above).

## The lead's decisions, against the code

| Ruling as quoted | Code at 28782cd | Docs |
|---|---|---|
| D0: take RI, RJ, RC, RF | Taken, plus one (above) | The description's plant table names them (A4', RJ, RC, RF, RV) |
| D1: the restart rule stays wide | `restarts` is `!matches!(e, Full \| Premise(_))` (`program.rs:884-886`), unchanged | The module docs say "any but those two refusals". The cost is not said: while the daemon is down, every tick restarts, walks the pool's holds again (`swept` cleared) and tells every stop and stall report again. That is open for the lead, since the ruling as quoted says nothing about docs. |
| D2: no third word; `failed` covers the refusals | `Round.failed` is "any error pushed", `Full` and `Premise` included (`program.rs:996-997`, `:1029`) | Said in the `failed` bullet (`program.rs:80-82`) |
| D3: the `abandoned` window closes in M2.7b with record v3 | Unchanged, as ruled | Both ways in are named (`program.rs:96-100`). The 64-give-up bound on the record's fallback (`ABANDONED_KEPT`) is not. |
| D4: `--device` for the life of `run` is M2.7c | Unchanged, as ruled | Not said in `run`'s docs. Optional until M2.7c. |

## Docs, against the code

**The round line** (`program.rs:63-90`):

- **True:**
  - `ok`: no error was returned, and nothing is said about movement.
  - `failed` includes the two refusals.
  - The reload comes at once after a begin's error and at the next
    round after a settle's error.
  - A reading that fails is tried again at the next round.
  - `begun=-` can follow a `snap/` line for the very publish whose
    begin failed.
- **n6:** a round whose reading failed begins nothing as well as
  settling nothing. `begun` is then `-` with no begin at all, and
  `under_way` is the process's count from before. The docs say only
  "settles nothing", and that `begun` is `-` "where its begin failed".

**S1** (`program.rs:92-100`):

- True. The `abandoned` window now names the failed save as well as
  the kill (my F5).
- "At least once" is stated for a run that is killed. It does not
  claim more, so the power cut is left unsaid rather than overclaimed.

**Exit codes** (`program.rs:109-114`):

- True. `main.rs`'s `agent()` maps an error of `wired` by
  `exit_of_error`, so a corrupt record, or another volume's, at the
  start exits 6.
- The record unread exits 1, and the directory held exits 8.

**Signals** (`program.rs:102-108`). **n1, below.**

**The `--limit-s` sentence** (`program.rs:34-37`): true.

## Driven for real (`live.sh`, `live.out`)

The debug binary at 28782cd ran as `run`, with a fake `zfs` and `zpool`
that fail after a sleep, and no daemon. Each case logs its calls with
their times relative to the first signal.

| Case | Exit | From the last signal to the exit | Calls started after the first signal |
|---|---|---|---|
| SIGTERM while it waits (`--every-s 5`) | 0 | 0.05 s | 0 |
| SIGINT while it waits | 0 | 0.06 s | 0 |
| SIGTERM in a call of 3 s (`--every-s 60`) | 0 | 11.1 s | 3 |
| SIGTERM, then SIGTERM 0.5 s later | 1 | 0.005 s | 0 |
| SIGTERM, then SIGINT 0.5 s later | 1 | 0.001 s | 0 |

The same as at 16f3b3b. In the third case the calls were these. The
begin opens with the count and the sweep (`engine.rs:701-702`), and so
does each pass of the settle (`:765-766`):

| Time from SIGTERM | Call | Whose |
|---|---|---|
| -1.00 s | `zfs list -r -H -o name -t filesystem,volume tank` | begin: the count (in flight at the signal) |
| +2.03 s | `zpool get … guid,load_guid,… tank` (fails) | begin: the sweep's read. The begin ends here with its error. |
| +5.05 s | `zfs list …` | the settle's first pass: the count |
| +8.08 s | `zpool get …` (fails) | the settle's first pass: the sweep. The round fails here, and the exit follows. |

## Findings, most severe first

None blocks the merge. There is no defect in the code. The findings are
about docs, the description and the answer.

### n1 (low, docs): the signal docs say less than the code does

- **Where.** `program.rs:104-108`, "A signal that comes while the engine
  works ends `run` only once its step is done (a begin, or a pass over
  the publishes under way): a ZFS call is not cut, and the step's other
  calls run first". The answer and the description say the same.
- **What the code does.** `go` is asked at the top of each round, in
  `settle_while` only after the pass's count and sweep
  (`engine.rs:765-782`), and in the wait.
  - After a begin, the settle's count and sweep run before `go` is
    asked.
  - After a pass in which nothing moved, so do `TICK`'s sleep and the
    next pass's count and sweep (`:793`, then `:765-766`).
  - The sweep includes its walk of the pool's holds after a restart
    has cleared `swept`, and its releases of stale holds.
- **Seen.** Above: 6 s of the 11 s after the signal were the settle's
  two reads, after the begin had ended. With the real limits (60 s for
  a read) that is two more reads' limits, two minutes. More when the
  sweep walks the holds (`swept` cleared by a restart).
- **Why it is only low.** A SIGKILL at any of those points is in the
  crash matrix, so a stop timeout that runs out is safe.
- **Fix, either of:**
  - (a) say it: "…and the count and the sweep that open the next pass
    run before it ends";
  - (b) ask `go` before the count in `settle_while`, or between the
    begin and the settle in `schedule`.
- **On the answer's reason.** It says that cutting a step part-way would
  add a path the crash matrix does not have. That fits cutting a ZFS
  call. It does not fit (b): stopping before the count is stopping
  between two steps, where a kill leaves the same state.
- **Certainty.** Certain: read, and seen with the binary.

### n2 (low, description): "the next round first calls `Agent::restart`"

- **Where.** The description's "What was done", "Failed rounds", first
  bullet: after any error but the two refusals, "the next round first
  calls `Agent::restart`".
- **What is true.** Since 16f3b3b that holds after a settle's error
  only. After a begin's error the restart is at once, before the
  settle. The section "For review" and the module docs say it right.
- **Also there.** "A refused begin does not keep the round from
  settling" is true except where the reload after it fails.
- **Certainty.** Certain.

### n3 (nit, the answer's reason): the recount at a pass's end

- **What the answer says.** The recount (my N9) is left, and "that pass
  is the 'at least one pass' of Codex's fix".
- **It is not.** The count and sweep that are wasted open a pass after
  the first one. `passes` is at least 1 by then, and the limit then
  ends the round before any publish moves (`engine.rs:765-782`).
  Checking the limit before `TICK`'s sleep would leave Codex's
  guarantee as it is.
- **Leaving it is fine.** The cost is one count and one sweep's read a
  round, which the sweep's cache keeps to a `zpool get` and a `zfs
  list` while `swept` holds. The answer only says why wrongly.
- **Certainty.** Certain, from the code.

### n4 (nit): the variant's doc still says "A manifest's file failed"

`engine.rs:313`. The text now names the record's directory, and the
variant also covers the record's own load, the listing and the marker.

### n5 (nit): `restarts`'s docs say "after a round that failed"

`program.rs:876-883`. Since 16f3b3b, the restart after a begin's error
is within the round, before its settle. The rest of the doc ("so that
no path is new to the crash matrix") now holds: since 16f3b3b no engine
call follows a failed save before a reload.

### n6 (nit): a round whose reading failed begins nothing

Above, under "The round line".

### n7 (nit, cosmetic): the reflow left a line of 100 characters

`engine.rs:741` is 100 characters in a paragraph wrapped at 72. It is
within rustfmt's width, so the first look's point is met.

## The first look's nits, one by one

| First look | At 28782cd |
|---|---|
| N1: the description's list of what `restart` drops | Fixed. Description line 32 now matches `Agent::restart` (`engine.rs:570-581`): the device, the memory, the sweep done, the stops, the reported, the renames, the assists, and the kill point's publish. It keeps the settings, the points, the sinks, the outcomes and the reports. |
| N2: "a manifest's file" for an unreadable record | Fixed by the new text (above), and right. The variant's doc is stale (n4). |
| N3: SIGTERM waits for the round's remaining reads | Behaviour kept, as the author chose. The docs added for it say less than the code does (n1). |
| N4: `run`'s exit 6 at the start | Fixed, and true |
| N5: `signal-hook` declared in the bin | Not raised in the answer. It follows the file it is in; still a nit. |
| N6: the module docs' `failed` bullet | Fixed. There is one gap left (n6). |
| N7: "at most `--limit-s`" | Fixed |
| N7: `engine.rs:739`, 103 characters | Fixed (n7, cosmetic) |
| N8: eedd83e's `<=` | Unchanged and sound. The host tests ran 20 times with no failure. |
| N9: the recount at a pass's end | Not fixed. The decision is fine, the reason is not (n3). |

## Gates (`gates-28782cd.sh`, `gates-28782cd.out`)

Run on rustc 1.100.0 (current stable after `rustup update stable`),
over the whole workspace. The target was cleaned between feature sets.

`cargo fmt --all -- --check`: clean.

| Features | `clippy --workspace --all-targets -D warnings` | `test --workspace` | `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps` |
|---|---|---|---|
| none | clean | 1984 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean | 2042 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | clean | 2008 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | clean | 1984 passed, 0 failed, 16 ignored | clean |

These equal the answer's figures. Each is 16f3b3b's plus the five new
tests.

`cargo doc -p skyzfs-agent -p skyzfs-agent-cli --no-deps` with
`-D warnings` is clean plain and with `skyzfs-agent-cli/test-hooks`.
With the other two feature sets, cargo refuses that command line ("none
of the selected packages contains these features"). That is my script's
error, not a gate: the workspace's doc with each of them, above,
covers the agent's crates.

The host tests (`cargo test -p skyzfs-agent --lib host::`) ran twenty
times on the plain build: no failed run.

## CI

CI on 28782cd (`gh api …/commits/28782cd…/check-runs`): 41 succeeded
and 4 were skipped. That includes the daemon rig and the stand-in on
their images, the agent through skyzfsd, the cut and node B rigs, and
coverage. It is the same shape as 16f3b3b's end: 41 succeeded, 4
skipped.

Codex's summary for 28782cd reads `"has_any_findings":false` for its
code review. Its two threads from 99e00ae were answered at 16f3b3b.

## Decisions for the lead

- **On D1, the docs.** Should `run`'s docs say what the wide rule costs
  while the daemon is down? At each failed tick, the walk of the pool's
  holds is made again, and every `stopped` line and stall report is
  told again. It is one sentence. The ruling as quoted does not say.
- **On n1, the signal docs.** Reword them, (a), or ask `go` before the
  settle's count, (b). (b) is a one-line change of behaviour that
  shortens a stop by up to two reads. It adds no new point to the crash
  matrix.

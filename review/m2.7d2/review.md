# Review of skyzfs/skyzfs#172 (M2.7d-2) at edf9a17

Head `edf9a1701a3caeca95fead8c181e2a553127e48a`, checked with `gh api
repos/skyzfs/skyzfs/pulls/172 --jq .head.sha` at the start and at the
end: it did not move. Two commits on `zfs-topology` at 6868430 (its
tip): e16a8f9 (L2, T1 with the review's tests adapted, d1-d5, d8) and
edf9a17 (the lead's ruling 3, `bound=0|1`; the docs of rulings 2, 4
and 5). Five files, all in `crates/skyzfs-agent/`.

I read CLAUDE.md, design §5.2 and the notes' M2.7d entry, then the code
(`Agent::sync_past`, `Agent::last_synced`, `schedule`) before the
description, then the diff line by line, the plants, the review's new
tests, the gates, PR #171's reader of the round line, and last the
description and the author's answer on #170 (comment 7206666325),
claim by claim.

Files beside this one:

- `plants3.py` with `plants3-head.out` (the plants against the head's
  tests) and `plants3-both.out` (the survivors against the head's tests
  plus the review's);
- `scratch-tests.rs`, the review's tests (not for merging as they are),
  with `scratch-tests-head.out` (the two that fail at the head);
- `fix-L3.diff`, the fix tried for L3;
- `pr171-reader.out`, PR #171's reader of the round line at three of
  its heads;
- `gates.sh` with `gates.out`.

The scripts take the scratch directory from `$SCRATCH`.

## Verdict

**It can merge from review's side; nothing blocks.** L2 is fixed on
the path it named (a sync that fails), `bound=` is set for
`Stuck::Bound` and for nothing else, the review's T1 survivors are all
killed by the adapted tests (R16 aside, as agreed), the author's
fifteen mutants are all caught, and the gates are clean.

One low defect, which I recommend fixing in this pull request since
this package exists to make these two claims true and the fix is four
lines of code (the lead decides):

- **L3**: where the daemon's `cuts` cannot be read after syncs that
  returned (`Past::Unseen`), the round line says `synced=0`, and an end
  asked during the last sync still lets the begin run. It is L2 and L1
  of #170 again, on the one arm of `schedule` that the fixes missed.

Then test gaps (four mutants of the new logic survive; the review's
tests kill them) and doc nits.

Codex reviewed edf9a17 with no findings. CI: see below.

## The code against the questions

### (a) `Agent::last_synced`, the count after a failed sync

`engine.rs:796-839`. `last_synced` is set to 0 at the top of every call
of `sync_past` (`:798`) and to the call's `synced` after each sync that
returned (`:836`); `Agent::new` sets it to 0, `Agent::restart` leaves it
alone. `schedule` reads it only in the `Err` arm of the same call
(`program.rs:1112`), before the restart. So:

| Path | `synced=` on the line | Right? |
|---|---|---|
| A sync fails after k returned | k (`a_round_whose_sync_failed_counts_the_syncs_before_it`: 2) | yes (L2 fixed) |
| The first sync fails | 0 | yes |
| The premise's reads fail, or the premise is refused | 0: the premise is checked only while `synced == 0` (`:822`), so nothing has returned | yes |
| A failure before any sync in a round after a round whose syncs failed after k | 0 (the review's S5) | yes: the reset is at the call's start |
| A round whose engine restart is owed and fails | 0: `sync_past` is not called and `last_synced` not read | yes |
| `Clear`, `Behind` | the `Past`'s own `synced`, equal to `last_synced` | yes |
| **`Unseen` after k syncs** | **0**: the `Unseen` arm (`program.rs:1097`) does not read it | **no (L3)** |

It can never report a count from a previous round: every read follows
a call of the same round, and every call resets it first. A mutant that
resets it only where nothing is behind (F4) survives the head's suite
(S5 kills it).

### (b) `bound=`

`program.rs:1104`: `bound = why == Stuck::Bound`, in the `Behind` arm
only; `bound` is a local of the round, false at its start (`:1077`).
So it is 1 exactly where `sync_past` returned `Behind { why: Bound }`:

- `NoCut`, `Told`, a failed sync, a refused premise, `Unseen`, a
  `Clear`, and a round whose restart is owed: 0. Only `NoCut` is pinned
  by a test (`a_round_whose_sync_made_no_cut_is_not_at_the_bound`);
  `Told` and a failed sync are not (mutants F1, F2 survive; the review's
  S2 and S3 kill them).
- A round after the one that met the bound, which syncs nothing: 1
  (`past_synced` stays at 128, `Bound` is checked before `go`). Pinned
  by `a_round_still_behind_begins_no_publish` (round 2, `synced=0
  bound=1`). Each round says it again until a cut opens as itself or
  the daemon holds no cut; a mutant that keeps the flag across rounds
  (F5) survives (the review's S4 kills it).
- It cannot be 1 in a round that is not behind at its syncs: only the
  `Behind` arm sets it, and that arm begins nothing.
- Precedence: `NoCut` is judged before the bound (`engine.rs:813-818`),
  so the round whose 128th sync made no cut says `bound=0` (its
  stderr line says "the daemon counted no cut") and the next round says
  `bound=1`. Consistent with "`Stuck::Bound`", one round late against
  the module docs' "had met their bound" (n1).

**Tooling.** `bound=` is appended after `synced=`; the other fields and
their order are unchanged (F6, the two swapped, is caught by the format
test). Nothing on `zfs-topology` reads the round line:
`tests/kernel/agent_check.py` reads `snap/`, `abandoned` and other
outcome lines, and `agent-record.sh` never runs `run`. PR #171 (the
agent rig's `arun` course) does read it: at its earlier head 7ab6c65
its `ROUND_LINE` took `synced=K` only as the line's last field under
`fullmatch`, so a line ending ` bound=0` was a `RigError` (exit 2); its
head now, 4ccc4b4 ("read the round line's fields after under_way by
name"), reads fields after `under_way` by name and takes `bound=0` and
`bound=1`, and so does its head at the end of this review, 3b9dc1c
(`pr171-reader.out`). So the order of merging does not matter as long
as #171 merges at 4ccc4b4 or later. #171 reads `bound` and judges
nothing on it.

### (c) "What the bound does not cover" (`program.rs:151-160`)

- "It is the process's, and not saved: a supervisor that starts `run`
  again ... syncs up to SYNC_PAST_MAX times at each start while the
  newest cut stays behind." True: `past_synced` starts at 0 in
  `Agent::new` and is in no file.
- "The tick does not limit a round's syncs: a round runs them to their
  end, and may run past its tick by up to that many syncs. Rounds still
  never overlap." True: `sync_past` has no time check; the scheduler
  counts the ticks it overran as `skipped` and the next round's
  `late_ms`.
- "A sync that fails is not counted ...: where every sync fails, only
  the tick limits them, one a round, each round `failed` with the
  error." True: the `?` on the assist ends the call (`engine.rs:834`),
  the count rises only after it returned.
- `ok` while stopped at the bound (the `bound` bullet, `:89-93`): the
  bound does not fail a round, true. But "Such a round is `ok`" is not
  so where its settle fails (the settle runs in a round behind, R20):
  `round n failed ... bound=1` can be printed (n1).

### (d) T1 and the plants

Each plant alone against `cargo test -p skyzfs-agent -p
skyzfs-agent-cli --tests --no-fail-fast` (the library's tests, the
binary's and its integration tests), restored after each, 600 s each.
The baseline passes: 259 + 0 + 7.

| Plant | Caught by the head's suite (`plants3-head.out`) |
|---|---|
| R1 the count reset only by a cut that opens, not by no cut | `past::no_cut_held_starts_the_count_again` |
| R3 the daemon unread: no begin | `a_round_with_the_daemon_unread_still_begins` |
| R5 NoCut against the first count | `past::no_cut_is_judged_from_the_last_sync` |
| R7 the premise once per process | `past::the_premise_is_checked_at_each_call` |
| R8 the syncs' error does not restart | `restarts::an_error_of_the_syncs_reads_the_record_again`, `restarts::a_failed_reading_after_an_error_of_the_syncs_settles_nothing` |
| R9 the syncs' error still begins | `a_round_whose_syncs_were_refused_begins_nothing`, `a_round_whose_sync_failed_counts_the_syncs_before_it` |
| R10 a failed reading after the syncs' error still settles | `restarts::a_failed_reading_after_an_error_of_the_syncs_settles_nothing` |
| R11 NoCut resets the bound count | `past::a_sync_that_makes_no_cut_still_counts_toward_the_bound` |
| R12 the syncs' error restarts even on a refusal | `restarts::a_refusal_at_the_syncs_reads_the_record_once` |
| R16 the bound checked after `go` | **survives** (near-equivalent, as agreed on #170) |
| R19 `settle_while` at an end returns `Ok` | `an_end_before_a_pass_names_what_is_under_way` |
| R20 a round still behind settles nothing | `a_round_still_behind_still_settles` |
| L2a the count lost on the error | `a_round_whose_sync_failed_counts_the_syncs_before_it` |
| L2b `last_synced` not reset at a call's start | `past::the_syncs_before_a_failure_are_told` |
| B1 `bound` never set | `a_round_still_behind_begins_no_publish` |
| B2 `bound` set for every cause | `a_round_whose_sync_made_no_cut_is_not_at_the_bound` |
| F1 `bound` also for an end (`Told`) | **survives** |
| F2 `bound=1` on an error of the syncs | **survives** |
| F3 `last_synced` counts the sync that failed | `past::the_syncs_before_a_failure_are_told`, `a_round_whose_sync_failed_counts_the_syncs_before_it` |
| F4 `last_synced` reset where nothing is behind, not at the call's start | **survives** |
| F5 `bound` kept across rounds | **survives** |
| F6 `bound=` before `synced=` | `a_round_line_says_what_the_round_did` |
| F7 `last_synced` lags one sync | `past::the_syncs_before_a_failure_are_told`, `a_round_whose_sync_failed_counts_the_syncs_before_it` |
| F8 `NoCut` judged after the bound | `past::a_sync_that_makes_no_cut_still_counts_toward_the_bound` |
| F9 `last_synced` cleared by a restart and read after it | `a_round_whose_sync_failed_counts_the_syncs_before_it` |
| F10 `bound=1` only in a round that synced nothing | `a_round_still_behind_begins_no_publish` |

- **The review's T1 survivors** (R1, R3, R5, R7-R12, R19, R20): all
  killed by the adapted tests, each by the test the description names.
  R8-R12 (the `Err` arm of the syncs in `run`, which no test reached at
  02af425) and R11 (`NoCut` resetting the bound) included. R16 still
  survives, as agreed.
- **"15 of 15 caught"**: confirmed (R1, R3, R5, R7-R12, R19, R20, L2a,
  L2b, B1, B2, as I read the description's one-line forms).
- **The review's new plants**: F1, F2, F4 and F5 survive the head's
  suite; the review's S2, S3, S5 and S4 kill them one each
  (`plants3-both.out`; S1 and S1b fail at the head by design and are
  not counted).

### (e) The docs, claim by claim

| Item | Where | Judged |
|---|---|---|
| d1 `opens=rewind` | `engine.rs:81` | Right (`skyzfs-control/src/lib.rs:45`, `:710`). |
| d2 what makes a cut open; no cut held resets | `program.rs:137-142`, `engine.rs:598-601` | Right. |
| d3 restarts' timing names the syncs | `program.rs:953-959` (`restarts`), `:1037-1047` (`schedule`) | Right. The module docs' `failed` bullet (`:100-104`) still names only a begin's and a settle's error (n4); the paragraph at `:143-146` covers the syncs. |
| d3 where `go` is asked | `program.rs:1048-1050`, `:176-183` | "before a begin that follows syncs": not on the `Unseen` path (L3). |
| d4 `begun=-` | `program.rs:80-84` | Complete: a failed begin; syncs failed, behind, or ended; a failed reading. |
| d5 ragged breaks | `engine.rs:1010-1011`, `schedule`'s docs | Fixed; but the change adds a 123-column line at `engine.rs:599` and leaves `program.rs:182` (85 columns), `:956` and `:1028` ragged (n6). |
| d8 the syncs' bounds | `engine.rs:86-95` | Right except "Only there can the syncs go on without end" (n2). |
| `synced` | `program.rs:86-88` | "the syncs the round ran, and that returned": not on the `Unseen` path (L3). |
| `bound` | `program.rs:89-93` | Loose in three words (n1). |
| An end during the syncs | `program.rs:146-149` | "begins no publish": not where `cuts` is unread after the last sync (L3). |
| What the bound does not cover | `program.rs:151-160` | True, each bullet (above). |
| `last_synced` | `engine.rs:602-604`, `:784-789` | The field's doc is right; the getter's "what `run`'s round line says" is not on the `Unseen` path (L3). |
| `Round::bound` | `program.rs:990-992` | Right. |

## Findings, most severe first

### L3 (low, defect): the daemon unread after syncs, the line says `synced=0` and an end still begins

`program.rs:1097`: `Ok(Past::Unseen) => clear = true,`. `sync_past`
returns `Unseen` wherever a read of `cuts` fails or is answered with
something else (`engine.rs:801-804`), also after syncs that returned:
`Past::Unseen` carries no count (its doc, `engine.rs:151-154`, says it
"syncs nothing"). The arm then

1. leaves `synced` at 0, though `last_synced()` holds the syncs that
   returned: the L2 that this package fixes, on the one arm the fix
   missed; and
2. begins without asking `go`, where the `Clear` arm asks it when
   syncs ran (`program.rs:1095`, 02af425's fix of L1): an end asked
   during the last sync still lets the begin run, with its count,
   sweep (which may release holds), the premise's reads and its `cuts`
   read; if the daemon answers by then, it saves an intent for the next
   start.

Scenario: the daemon restarts, or its socket times out, between two
syncs after a `-T` import. The review's tests (fake world, rewind gap
10; the daemon away from the third `go` on, which comes before the
second sync) fail at the head (`scratch-tests-head.out`):

```
round 1 failed took_ms=0 late_ms=0 skipped=0 begun=- under_way=0 synced=0 bound=0
skyzfs-agent: round 1: the daemon: the daemon is away
```

with two syncs landed (`synced_at.len() == 2`), and, with the end
asked during the second sync (`s1b`):

```
calls after the end was asked: ["cuts", "count", "props", "tagged", "props", "cuts"]
```

The docs that say otherwise: `program.rs:86-88` (`synced`), `:146-149`
("An end asked during the syncs begins no publish"), `:179-182` and
`:1048-1050` (`go` "before a begin that follows syncs"), and the
getter's doc at `engine.rs:784-789`.

Cost: a wrong count on a line only tooling reads, and one begin after
an end, which leaves at most an intent for the next start; nothing is
lost and no path is new to the crash matrix (a kill can always cut a
begin). It needs the daemon unread right after a sync. Low.

Fix (`fix-L3.diff`): in that arm, `synced = agent.last_synced(); clear
= synced == 0 || go();`. A round that synced nothing begins as before,
so the lead's decision that an unread daemon lets the begin report its
own error holds (R3's test still passes). Tried: with it, the head's
suite and the review's six tests pass (265 + 0 + 7). `Past::Unseen`'s
doc would then say "syncs no more". Certain (shown by two tests).

### T2 (test gaps): four mutants of the new logic survive

| Plant | What survives | Killed by (review's test) |
|---|---|---|
| F1 | `bound=1` for a round stopped by an end (`Told`) | `s2_a_round_stopped_by_an_end_is_not_at_the_bound` |
| F2 | `bound=1` for a round whose sync failed | `s3_a_round_whose_sync_failed_is_not_at_the_bound` |
| F4 | `last_synced` reset only where nothing is behind: a round whose premise is refused, after a round whose syncs failed after 2, says `synced=2` | `s5_a_refusal_after_a_failed_sync_says_synced_0` |
| F5 | `bound` kept across rounds: once at the bound, every later round says 1, after a cut opens too | `s4_a_round_after_the_bound_that_finds_nothing_behind_says_bound_0` |

The description says a round stopped "for any other cause reads 0: a
sync that made no cut, an end, or a failed sync"; only the first is
pinned. The author may take the review's tests as they are or write
their own.

### n1-n6 (nits): docs

- **n1** `program.rs:89-93` (`bound`): "Such a round is `ok`": not where
  its settle fails, which it runs (`round n failed ... bound=1`); "and
  so is every round after it until a cut opens as itself": also until
  the daemon holds no cut, and not a round that fails otherwise; "none
  of them publishes": none of them *begins* a publish; "had met their
  bound": the round whose 128th sync made no cut says `NoCut` and
  `bound=0`, the next one `bound=1`. Suggested: "1 where the round's
  syncs stopped at their bound (`Stuck::Bound`) ... the bound fails no
  round".
- **n2** `engine.rs:94-95`: "Only there can the syncs go on without
  end": not so. A daemon that holds no cut now and then (a state begun
  anew, as at a start that trusts no state) starts the count again each
  time, so a daemon that restarts over and over while the pool stays
  behind gets up to 128 syncs from each instance; and `program.rs:152-154`
  names the other case, a supervisor that starts `run` again.
- **n3** `engine.rs:151-154` (`Past::Unseen`, "syncs nothing") and
  `engine.rs:784-789` (the getter, "what `run`'s round line says"):
  with L3.
- **n4** `program.rs:100-104`: the `failed` bullet lists the restart
  "at once after a begin's error, before the round settles; at the next
  round after a settle's error"; an error of the syncs restarts at once
  too (d3's residue; said at `:143-146`).
- **n5** The two windows left from L1 (an end during the first `cuts`
  read with nothing behind lets the begin's count and sweep run; an end
  during the premise's reads lets one sync run): the author's reasons
  are sound and I accept leaving them. But the signal docs
  (`program.rs:176-183`) and the answer on #170 ("the step under way,
  and nothing after it") read as if no step starts after an end; one
  clause naming the two windows would make the docs exact.
- **n6** Cosmetic: `engine.rs:599` is 123 columns (new; the description
  says the ragged breaks are reflowed); `program.rs:182` (85 columns),
  `:956` and `:1028` break raggedly.

## Decisions for the lead

None blocks.

1. **L3**: fix it in this pull request (recommended: four lines, the
   package's own claims) or accept it as a nit for a later package.
2. **Design §5.2** says the agent stops "after 128 syncs without a cut
   that opens as itself". Since 02af425 the count is of syncs that
   returned, and a failed sync is limited only by the tick (the notes'
   M2.7d entry says so). The design's sentence could say "128 syncs
   that returned".
3. **The two windows of L1** (n5): accept as left, with or without a
   clause in the docs.

## The description and the answer, claim by claim

- Commits, base, files: as stated. "Nothing here changes what a round
  does, apart from" the count after a failed sync and `bound=`: true.
- L2: fixed for a sync that fails; not for `Unseen` after syncs (L3).
- Ruling 3: as stated, but "a round stopped for any other cause reads
  0" is pinned for `NoCut` only (T2), and "Such a round is `ok`, and so
  is every round after it until a cut opens" is loose (n1).
- Rulings 2, 4, 5: written, and true (above).
- T1 table: each test kills its R (above).
- "15 of 15 caught": confirmed.
- Docs d1-d5, d8: as judged above; d5 adds one new ragged line (n6),
  d8 one sentence that is too strong (n2).
- "This changes the line the agent rig reads; testing is told": #171's
  head 4ccc4b4 reads it (above).
- Gates: see below.

## Gates (`gates.sh`, `gates.out`)

rustc 1.100.0 (`rustup update stable` first: unchanged), in a scratch
worktree at edf9a17, the target cleaned between feature sets.

| Gate | Plain | `skyzfsd/test-hooks,skyzfs-local/test-hooks` | `skyzfs-ublk-sys/control-test-program` | `skyzfs-agent-cli/test-hooks` |
|---|---|---|---|---|
| `cargo clippy --workspace --all-targets -- -D warnings` | clean | clean | clean | clean |
| `cargo test --workspace` (passed / failed / ignored) | 2019 / 0 / 16 | 2077 / 0 / 16 | 2043 / 0 / 18 | 2019 / 0 / 16 |
| `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps` | clean | clean | clean | clean |

`cargo fmt --all -- --check`: clean. The agent crates' tests
(`cargo test -p skyzfs-agent -p skyzfs-agent-cli`) twenty times: 0
failed runs, 266 tests a run. The numbers are the description's.

## CI

On edf9a17, read when this was written: 19 check runs succeeded, 4
skipped (`lint`, `test`, `unit-arm64` of the push run of `skyzfs-ci`,
which leaves a branch with an open pull request to `ci`; "Publish
badges", which runs on `main` only), none failed, none running. The
workflow runs: `ci` 45918170847 (lint, test, unit-arm64, s3-backends,
chart-lint), `coverage` 45918170818, `e2e` 45918170938, `k3s-smoke`
45918170783, `skyzfs-ci` 45918152211 (the push), and the agent rig
45918170794 (a1, a2, d, k on ubuntu-26.04, and the check's logic). I
read the conclusions, not the agent rig's summary table: it does not
run the code this package changes (below).

Codex: its summary comment says the code review and the security review
of edf9a17 completed, with no findings.

### Rigs

The pull request changes `crates/skyzfs-agent/` only. That triggers the
agent rig (courses a1, a2, d, k on ubuntu-26.04), which runs `publish`,
`settle`, `recover` and `record` but never `run`; no rig on
`zfs-topology` runs `run` or makes a `-T` import under the Rust agent.
So `sync_past`, `last_synced` and the round line run only in the unit
tests (`test`, `unit-arm64`, `coverage`). PR #171's `arun` course is the
first rig to run `run`, with a rewind import; it is not merged.

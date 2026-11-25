# Third look at #147 (M2.6c-2) at ab8db11

The last look was at `0f709ae` (`next_look.md`). This look is at
**`ab8db11`**, the pull request's head when it began and when it ended.
Three commits are new:

- `62df306`: `held()` reads every member's `@t` (NL-F1);
- `cd97b14`: a merge of `zfs-topology` at `8f0390f` (#146). It is
  clean: `git show --remerge-diff` shows nothing, and the base is still
  `8f0390f`;
- `ab8db11`: the partial take is destroyed, and NL-F2, NL-F3, NL-S1,
  NL-S4 and NL-T3.

I judged the diff `0f709ae...ab8db11`, the PR's own two commits, and the
head's code where they reach it (the planner and the engine).

## Verdict

**No finding blocks the merge.** The new partial-take code is sound:

- It cannot destroy a snapshot or a hold that the program did not
  take, except through the window between the program's return and the
  destroy. `destroy_taken` already has that window, and it was accepted
  at the last look.
- The `EEXIST` exclusion is right, and conservative.
- A failed destroy is reported as failed.
- The new tests kill every plant that is not equivalent (below).

NL-F1 is fixed: a partial hold is no longer published. The fix has one
side effect, TL-F1: a partial hold is now never released. It needs a
person's partial release first, it errs on the safe side, and the
sweep clears it at the next start. So it is Low, but it should be fixed
here or next. The answer's "the hold released first" is not what the
code does.

**Can it merge?** Yes, once every run on `ab8db11` is green. At
19:33 UTC these were done and green:

- `skyzfs-ci` (push), which runs fmt, clippy and the workspace's tests;
- `e2e`;
- `k3s-smoke`;
- `skyzfs-nodeb-rig` (green by 19:35).

These were queued or running:

- `ci`, `coverage`;
- `skyzfs-control-rig`, `skyzfs-publish-rig`, `skyzfs-cut-daemon`;
- `skyzfs-daemon-rig`, `skyzfs-replay-rig`.

The last look's other conditions are met:

- On `0f709ae`, `skyzfs-daemon-rig` (45665358670) and
  `skyzfs-replay-rig` (45665358640) ended green.
- The recorder's ubuntu-24.04 job (run 45665823917) is green. Its
  `*-snapshot-odd*` records give the same `local` maps as 26.04 (GUIDs
  and groups aside), with the same hex values.
- No `.lua` file and no recording has changed since `0f709ae`.

## Findings, most severe first

### TL-F1 (Low, certain; a side effect of NL-F1's fix). A partial hold is never released

`held()` now means "every member's `@t` has the tag". The planner uses
the same answer for its release decisions:

- `plan.rs:447`: `Phase::Done { .. } if pool.held => Release`, else
  `Forget`;
- `plan.rs:523-528`: `give_up` releases only `if pool.held`;
- `plan.rs:651`: `away` releases only `if ... && pool.held`.

So where some members have the tag and others do not, every path that
ends a publish skips the release, and the tags that are there remain.

**Scratch engine tests** (`tl147_engine.py`, `tl147_engine.out`) run on
the fake world at both heads. In one variant, the fake's `hold`
refuses as ZFS does: `zfs hold -r` fails when any member has the tag,
and the adapter then reports the members without it.

| Where the tag is removed, and from what | `0f709ae` | `ab8db11` |
|---|---|---|
| At `Done`, from the members (the root keeps it) | Published; no hold left | Published; **`tank@t` still held** |
| Just after the hold, from the members (ZFS-like hold) | Published with an incomplete hold (NL-F1) | Abandoned `TooOld`; **`tank@t` still held** |
| At `Done`, from the root (the members keep it) | Published; members' `@t` still held | the same |
| Just after the hold, from the root (ZFS-like hold) | Abandoned `TooOld`; members' `@t` still held | the same |

The first two rows are new at this head. The last two are older.

- **The sweep** (`engine.rs:495-512`) releases a tag no publish names.
  It runs once per `load_guid` per process, so it clears these only at
  the next agent start or the next import.
- **The answer to the next look** says that a partial hold is "given up
  at `AGE_MAX` (`Why::TooOld`, the hold released first)". The first
  part is right. The second is not: `pool.held` is false, so
  `give_up` abandons at once.
- **Why the tests do not see it.** The fake's `hold` (`fake.rs:1212`)
  adds the tag where it is missing and answers `Ok`. ZFS refuses and
  holds nothing. The crash matrix's people (`USERS`, `fake.rs:180`)
  never release a tag on part of a snapshot.
- **Why Low.** It needs a person to release the tag on some members, or
  a member's `@t` made after the hold. A hold left behind loses
  nothing, and stops only a `zfs destroy` until the next start.
- **Suggestion.** Decide releases on "some member's `@t` has the tag".
  `held()` could answer none, part or all. Or the planner could release
  wherever the record's `held` is set, since `release()` is already
  idempotent: "no such tag" with no member keeping it is `Ok`.
  - The test: a partial release in `USERS`, with the fake's `hold`
    refusing as ZFS does.

### TL-F2 (Low, certain; a claim and the docs, not the code). The scale run's own case is not one the agent would destroy

`snapshot()` destroys a partial take only when it reads the program's
output. The program runs within `limits.change`, 120 s (`cli.rs:198`).
At 10 000 file systems, the scale run's `snapshot.lua` took 964.7 s
(`scale-10000-snapshot/secs` in run 45666280714's artifact).

Under the agent, `zfs program` would be killed at 120 s (`host.rs`,
`TimedOut`), and `program_output`'s `?` returns before any destroy.

The channel program runs on in syncing context and takes what it
takes. At the next round, `pool@t` is there, so the outcome is
`ManifestLost`, with the snapshots kept, as before this head.

So the test row "the scale run's own output" covers a path the agent
reaches only when the program ends inside its limit. At 1000 file
systems it took 98.6 s, which is inside.

The module docs (`cli.rs:74-77`) name "an instruction or memory limit"
as the failures the program cannot report. The time limit belongs in
that list. This is for the lead, beside the provisional limits the
author has already raised.

### TL-F3 (Low; wording). Two of the destroy's outcomes can read wrong, and no outcome reaches a person yet

- **A contradiction.** After a partial take, `could not find any
  snapshots to destroy` reads "`…after 3 were taken; tank@t was not
  taken`". `pool@t` was taken, so it can only have been destroyed by
  someone else in between. "was not found" fits better
  (`cli.rs:316-318`).
- **A timeout.** A destroy killed at its limit reads "could not be
  destroyed", but in the kernel the destroy goes on and may finish.
  "may not have been destroyed" is closer. This is the same in
  `destroy_taken`.
- **Where the words go.** The engine drops the take's error
  (`engine.rs:813`, `Err(_) => return Ok(Moved::No)`). So "what became
  of it" reaches no one at this head. This is for M2.6c-3, which runs
  the agent as a command.

## Suggestions (none blocks)

- **TL-S1. A destroyed partial take is tried again every round, for
  up to 2 h.**
  - After the destroy, the next round sees `Intent` with no `pool@t`,
    and plans `Take` again with the same name (`plan.rs:420`). It goes
    on until `AGE_MAX`, 2 h (`plan.rs:233`).
  - A failure that repeats (a `snapshot_limit`, `EDQUOT`) takes and
    destroys again each round.
  - Before this head, a partial take was given up at the next round
    (`ManifestLost`).
  - A failure before any take already retried this way, so this is
    consistent. It does cost more now: real snapshots are taken and
    destroyed each round.
  - The lead's call: accept it, or give the publish up after a
    destroyed partial take.
  - `FakeZfs::snapshot` never fails part way, so the engine's tests do
    not cover this.
- **TL-S2. The members are read twice each round.** `see()` calls
  `members()` and then `held()` (`engine.rs:552-553`), and `ask_anew`
  does the same (`:936-937`). `held()` runs the members program again
  wherever the root has the tag.
  - At 10 000 file systems, that program took 24.3 s.
  - `zfs holds -H -r` was not timed at scale.
  - Passing the members already read into `held()` would save one read
    a round.
- **TL-S3. A failure with no errno is destroyed.** `partly_taken`
  (`cli.rs:967-977`) destroys where `errno` is missing or not a number,
  since `None != Some(EEXIST)`. The program always gives one, so only a
  changed program could reach this. The conservative reading would keep
  the snapshots when the errno is unknown.

## Test gaps (not blocking)

- **TL-T1. The fake's `held` change is not pinned.** Plant F1e puts
  the fake's `held` back to the root's tag alone, and it survives.
  `62df306`'s message says the test checks a member left untagged.
  That is true of the adapter's test, not of the fake. TL-F1's test
  would pin both.
- **TL-T2. NL-T6's fixture exists.** Testing's scale run
  (45666280714) recorded `rig-holds-r-partial` (the root tagged only)
  and `rig-holds-r-after-partial`. They are not yet in `cli/recorded/`.

## The points asked

### 1. `partly_taken`, `destroy_partial` and the `EEXIST` exclusion

Can it destroy a snapshot or a hold it did not create?

- **`taken` counts from the root.** `walk(pool)` puts the pool first,
  and `taken = i - 1` (`snapshot.lua:77-81`). So `taken > 0` means the
  program took `pool@t`.
- **The checks come first, in the same sync task.** Every member's
  `zfs.check.snapshot` runs before any take (`snapshot.lua:20-25`).
  So when the program returns, every `@t` under the pool falls in one
  of three groups:
  - the members already taken, which are the program's own;
  - the member that failed with an errno other than `EEXIST`, so its
    `@t` was not there;
  - the members after it, which were checked absent.
- **`EEXIST` after a take has begun cannot come from this program.**
  The check would have returned first, and the names are distinct.
  `dsl_dataset_snapshot_check` gives `EAGAIN`, not `EEXIST`, for a
  second snapshot of a dataset in one group. Leaving `EEXIST` out keeps
  those snapshots, and the next round gives `ManifestLost`. That is
  the conservative reading, and it is right.
- **Holds.** `zfs destroy` removes no hold. A held snapshot fails the
  destroy (`EBUSY`).
  - A destroy of many snapshots is all or nothing. In OpenZFS 2.4.1,
    `dsl_destroy_snapshots_nvl` runs `zfs.check.destroy` on every
    snapshot and returns the errors before any `zfs.sync.destroy`.
  - So a hold placed in the window gives "could not be destroyed", and
    nothing is destroyed.
- **The window.** It runs from the program's return to `zfs destroy`,
  one process start. A snapshot named `t` that someone makes under the
  pool in that window would be destroyed too.
  - Another agent cannot do it. The state directory is locked
    (`fs.rs:103`). A second agent with the same `t` would also find
    `pool@t` at its check and take nothing.
  - So only a person, or a `zfs receive` of a snapshot of exactly that
    name, could.
  - `destroy_taken` has the same window, and it was accepted at the
    last look.
- **Names.** `valid_name(t)` keeps `%` and `,` away from
  `zfs destroy`. The agent's names pass it by construction
  (`names/tests.rs:31`).

### 2. A destroy that fails or races

These rest on `cli.rs:313-325`:

| What `zfs destroy -r` gives | What the error says |
|---|---|
| exit 0 | "destroyed" (all of it: the destroy is atomic) |
| "could not find any snapshots to destroy" (`zfs_main.c:1762-1766` in 2.4.1) | "was not taken" (TL-F3) |
| "dataset does not exist" | "was not found" |
| any other failure | "could not be destroyed: `zfs destroy -r …: exit N: message`" |
| killed at its limit | "could not be destroyed: …" (may be wrong, TL-F3) |

The engine acts on what it sees at the next round, not on these words:

- the snapshots destroyed: `Take` again (TL-S1);
- not destroyed: `ManifestLost`, the snapshots kept.

The words reach no one yet (TL-F3).

### 3. Would the new tests fail if the code were broken?

See the plants below. Each one was applied alone to a scratch
checkout, with a `finally` that restores the file and a 600 s limit.
Every plant that is not equivalent was killed, except F1e (TL-T1).

### 4. `62df306` alone, and `ab8db11`'s fix to the recordings

**`62df306` alone fails two tests.** At `62df306`,
`a_pool_that_does_not_exist_reads_as_recorded` and
`holds_read_as_recorded` fail (`tl147_62df306.log`, 112 passed, 2
failed). The fake host has no record for `zfs holds -H -r`.

**`ab8db11` fixes them properly:**

- `held` is fed `*-holds-r` and `*-members` (and `none-holds-r`), the
  records of the commands it now runs.
- Every assertion is kept: `Ok(true)` with the tag, `Ok(false)` with
  another tag, and `Ok(false)` for a pool that does not exist.
- Checks are added of the commands run. The root untagged reads no
  member (`read[..1]`), and `none-holds-r` is the one command run.

**Nothing is weakened.** The `*-holds` records are no longer read,
because `held` no longer runs `zfs holds` without `-r`. In `tests.rs`,
every removed line comes back as an equal or stronger assertion. The
root-tagged-only row changes from `Ok(true)` to `Ok(false)`, which is
the fix itself. "dataset does not exist" now reads "was not found"
(NL-S1).

## NL items

| Item | At `ab8db11` |
|---|---|
| NL-F1 | Fixed (plants F1a to F1d killed; recordings updated). Side effect: TL-F1 |
| NL-F2 | Docs (`host.rs:21-28`) state both limits, as found (h1, h2). The PR body says so too |
| NL-F3 | Pinned. N1 (F3a), F3b and F3c are killed. The Lua pin is a text check on the `string.find` line; NL-T1's Lua job is still the full guard |
| NL-S1 | Done: the name guard (S1a and S1b killed), and "was not found" (S1c killed) |
| NL-S2 | Left to the lead (M2.7), as answered |
| NL-S4 | Done (S4 killed) |
| NL-S5 | The body names run 45665823917 and the group kill's limits |
| NL-S6 | Moved to #151 (M2.D1) |
| NL-T1, NL-T2, NL-T4 | Proposed to testing. Not in this pull request |
| NL-T3 | Pinned (T3n, the review's N12, killed) |
| NL-T6 | Asked of testing. The fixture exists (TL-T2) |
| NL-D1, NL-D2 | Documented (`host.rs:37-40`, `client.rs:16-20`), as ruled |

## Gates, plants, CI

### Gates at `ab8db11`

These are in `tl147_gates.sh` and `tl147_gates.log`. They ran from a
scratch checkout, on rustc 1.100.0 (`rustup check`: up to date), as
root:

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -D warnings` | clean |
| `cargo test -p skyzfs-agent -p skyzfs-control` | 123 + 16 passed |
| `cargo doc -D warnings` for the two crates | clean |

The workspace's clippy and tests are taken from `skyzfs-ci`'s push run
on the head (green).

### Plants

The plants are in `tl147_plants.py`, with results in `tl147_plants.out`.
Each was run alone with `cargo test -p skyzfs-agent --lib`.

| Plant | What it plants | Result |
|---|---|---|
| T1 | the partial-take arm left out | KILLED |
| T2 | `EEXIST` not excluded | KILLED |
| T3 | `taken` 0 counted | KILLED |
| T4 | no `taken` counted | KILLED |
| T5 | `failed` not required | SURVIVED: equivalent. The first arm takes every error with no `failed` |
| T6 | the destroy's outcome left out of the error | KILLED |
| T7 | "destroyed" whatever the destroy gave | KILLED |
| T8 | `errno` read from the wrong key | KILLED |
| T9 | the destroy without `-r` | KILLED |
| T10 | `destroy_taken`'s wording for a partial take | KILLED |
| T11 | the two arms swapped | SURVIVED: equivalent. The arms are disjoint on `failed` |
| S1a | the name guard left out | KILLED |
| S1b | the guard on the pool, not on `t` | KILLED |
| S1c | "dataset does not exist" read as "not taken" | KILLED |
| S1d | "could not find any snapshots" read as "not found" | KILLED |
| S4 | the prefix not taken off | KILLED (first planted as a deletion, which did not build; run again as `why.as_str()`) |
| F1a | `held`: the root's tag alone | KILLED |
| F1b | `held`: the root not checked first | KILLED |
| F1c | `held`: any member instead of every | KILLED |
| F1d | `held`: members gone read as held | KILLED |
| F1e | the fake's `held`: the root's tag alone | **SURVIVED** (TL-T1) |
| F3a | 0x7f printable (N1) | KILLED |
| F3b | 0x20 not printable | KILLED |
| F3c | the Lua pattern widened to DEL | KILLED |
| T3n | `says` on the first `": "` (N12) | KILLED |

### CI on `ab8db11`

Read with `gh api .../actions/runs?head_sha=...` at 19:33 UTC:

| Run | Result |
|---|---|
| skyzfs-ci (push) | success |
| e2e | success |
| k3s-smoke | success |
| coverage | in progress |
| skyzfs-control-rig | in progress |
| skyzfs-nodeb-rig | success (at 19:35) |
| skyzfs-publish-rig | in progress |
| ci (`unit-arm64` included) | queued |
| skyzfs-cut-daemon | queued |
| skyzfs-daemon-rig | queued |
| skyzfs-replay-rig | queued; in progress at 19:35 |

Codex has posted nothing on this head. Its last word on the pull
request is that it has reached its usage limit.

## Decisions for the lead

- **TL-F1.** How a partial hold is ended. Release it wherever any
  member's `@t` has the tag? This is small enough for this pull
  request.
- **TL-F2.** At the current limit (120 s), a partial take is destroyed
  only when the program ends within it. The scale run's case would be
  kept, as before. This goes with the limits the author already left to
  you.
- **TL-S1.** Retry a destroyed partial take until `AGE_MAX`, or give
  the publish up?

## What this look does not cover

- Nothing ran on ZFS. The atomicity of `zfs destroy` and the
  check-then-take order rest on OpenZFS 2.4.1's source
  (`dsl_destroy.c:629-656`, `zcp_synctask.c:298-343`) and on the
  program's text.
- The workspace's tests were not run here. `skyzfs-ci` ran them green
  on the head.
- The rigs on `ab8db11` had not finished.

## Files (beside this note)

| File | What it holds |
|---|---|
| `tl147_engine.py`, `tl147_engine.out` | The scratch engine tests of a partial hold (TL-F1), at both heads |
| `tl147_plants.py`, `tl147_plants.out` | The plants and their results |
| `tl147_gates.sh`, `tl147_gates.log` | The gates at the head |
| `tl147_62df306.log` | The two recordings tests failing at `62df306` alone |

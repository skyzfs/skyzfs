# Review of skyzfs/skyzfs#173 (M2.7c: the agent and a failed volume) at 29f5538

Head `29f5538fab63aede594935f4294a9fee0964b781`. I checked it with `gh api
repos/skyzfs/skyzfs/pulls/173 --jq .head.sha` at the start and at the
end, and it did not move. The pull request was stacked on #172, whose
head was `edf9a17` at the start and at the end. #172 merged into
`zfs-topology` during the review (01:59 UTC, merge commit 0f518da), so
#173 now targets `zfs-topology` and GitHub reports it mergeable. This
review judges only `git diff edf9a17..29f5538`. That range is five commits: 5ca82d8,
8466e15, 657dbec, 48222a2 and 29f5538. #172's changes belong to
another reviewer.

I formed the view below before reading the description, from:
- design §4.2 ("A failed volume gives up its attachment");
- design §10's row "A pool that only writes does not notice";
- the M2.7 row of the notes' work packages;
- the daemon's lifecycle docs (`skyzfs-ublk/src/lifecycle.rs`).

Then I checked the author's claims one by one ("What was checked",
below).

Paths such as `program.rs`, `engine/lost.rs` and `daemon.rs` are under
`crates/skyzfs-agent/src/` unless another crate is named.

## Verdict

**Not yet.** One finding blocks in my reading.

- **F1 blocks.** The step on a lost volume comes at the next tick of
  `run`. With the default `--every-s 60` that is up to a minute after
  the failure. Design §10 says the agent suspends the pool "within a
  second or two", and the M2.7 criterion says "within seconds of the
  failure". If the lead rules that the rig is to show the criterion
  with a short `--every-s`, F1 becomes a follow-up.
- **F2 needs the lead's ruling before the merge.** `run` exits 9
  whatever the step showed, and the ruling fixes what a service
  manager is told.
- **Everything else is small:**
  - one low defect (F3);
  - two other decisions (F4, F5);
  - test gaps: no test reaches the error paths (7 surviving plants),
    and A-P5, R-L1 and R-C2 survive too;
  - nits.

The moved code is equivalent and every test of it is still there. The
gates I ran are clean, and the test counts match the description's.
With `skyzfs-agent-cli/test-hooks`, clippy is clean locally; that
set's tests and docs were still running when this note was written,
and CI's tests with that feature are green ("Gates"). CI is green on every job at 29f5538.

## Defects, most severe first

### F1. The step waits for the next tick, so the pool can stay ONLINE for up to a minute (blocks; certain)

**Where.**
- `program.rs:1241` reads the note only at the start of a round.
- Between rounds, `run` sleeps until the next tick and checks only `go`
  (`program.rs:1409-1416`).
- Inside a round, a publish under way is settled until the next tick
  (`program.rs:1364-1367`), and a daemon that stops answering does not
  end that settle (R1 below).

So a volume that fails just after a round's note read is acted on at
the next tick: up to `--every-s` later (60 s by default), half that on
average.

**Scenario.** Scratch test R1 fails at the head:
1. `run --every-s 60` with a publish under way.
2. The daemon fails at +5 s, and its note is there from then on.
3. Round 1 runs to +60 s (`round 1 ok took_ms=60000`).
4. Round 2 reads the note and reopens at +60 s, **55 s after the
   failure**.
5. The pool suspends 1.05 s after that (the fake uses the figure from
   check f).

**Why it matters.**
- Design §10 (`docs/skyzfs-design.md:555`) says what happens to a pool
  that only writes on a deleted device: it "goes on acknowledging
  synchronous writes that no transaction group can carry. A node lost
  in that state loses all of it, beyond the bound D2 states". The
  mitigation it names is "The agent makes the pool read, which
  suspends it within a second or two".
- The M2.7 row of the notes (`docs/skyzfs-impl-notes.md:1085`) asks
  that "the pool is suspended within seconds of the failure".
- The module's own docs (`engine/lost.rs:20-22`) say "so that it
  suspends within seconds".
- With this code, all of that holds only where `--every-s` is a few
  seconds. The daemon rig's planned course (Q6) would show it only if
  it set such a value, and then it would not show the default.

**Fix (small).**
- Check for the note every `WAKE` while waiting for the tick, and in
  the `go` passed to the settle and the syncs. The check is a `stat`
  of a file in `/run`.
- When the note is there, start the next round at once.
- R1 is a test for it.

### F2. `run` ends with 9 whatever the step showed, and 9 means "do not restart" (decision for the lead; the behaviour is certain)

**Where.**
- `program.rs:1252-1290` returns `exit::LOST` on every path of a lost
  note:
  - when the step itself failed (`step=failed`);
  - when the reopen was refused, or killed at its limit, and the pool
    still reads ONLINE;
  - when the look ended at `LOOK_MAX` with the pool ONLINE
    (`engine/lost.rs:183`).
- The docs tell a service manager not to start `run` again on 9
  (`program.rs:193-196` and `:359-366`).

So nothing tries again in this boot, and the pool goes on only
writing: the hazard the step exists for. In every case but the first,
the round line says `ok`.

**Scenarios.** These are scratch tests. All pass at the head, so they
record what it does.
- **R2: the pool does not read SUSPENDED within the look.**
  - Output: `lost tank ... step=reopened reopen=ok state=ONLINE
    looked_ms=10000`, then `round 1 ok`, then exit 9.
  - A `run` started again on the same note reopens again
    (`reopens == 2`). That is the useful behaviour.
  - It contradicts the docs, which say "a restart on the same note
    ends with 9 again, with no second reopen" (`program.rs:362-364`).
    The module docs at `:194-195` assume the pool was found suspended.
- **R3: the reopen is refused (status 1) and the pool stays ONLINE.**
  - Output: exit 9 with `round 1 ok`.
  - On a real node, a reopen is refused without being run where the
    host already has four stuck children (`STUCK_MAX`).
  - It can also be killed at its limit, for example where it waits on
    ZFS's namespace lock behind a command stuck on a suspended pool
    (read from how ZFS locks, not seen here).
- **R4, R5: the pool's state cannot be read, before or after the
  reopen.**
  - Output: `step=failed`, then `round 1 failed`, then exit 9.

**A related window.**
- The daemon writes its note first. It deletes the device up to about
  2 s later (the order in the lifecycle docs). If it is killed in
  between, the delete never comes, and the device is left waiting for
  a server.
- A round that reads the note in that window reopens a device that
  still exists. Where the device is waiting, the reopen can hang until
  it is killed at 10 s, and the pool reads ONLINE. That is the R3 case.
- I did not see this on a kernel; it is read from the code and the
  lifecycle docs.

**Suggestion.**
- End with 9 only where the step left the pool SUSPENDED or not
  imported: `Step::Suspended`, `Step::NotImported`, or `Reopened` with
  the state SUSPENDED or none.
- Otherwise:
  - fail the round;
  - say so;
  - take the step again at the next round (or sooner, with F1's
    check), for as long as the note is there.
- Exit 9's docs then become true as written.

### F3. A look that fails after the reopen hides the reopen (low; certain)

- `engine/lost.rs:180`: when a look after the reopen fails, `lose`
  returns `Err`. The reopen's own result and its error text are lost.
- The line then reads `step=failed reopen=- state=- looked_ms=-`,
  although `zpool reopen` ran (R4: `reopens == 1`). An operator
  reading the line cannot tell that the pool was made to read.
- Suggestion: a `Reopened` step that carries the look's error, or
  `reopen=ok|failed` taken from the reopen itself.

### F4. A note that says "unknown" ends `run` for good, though a later start may settle it to "no" (decision; the behaviour is certain)

**What the code does.** `engine/lost.rs:159-160` takes
`Held::Unknown` as lost.

**When the daemon writes "unknown".**
- The daemon writes "unknown" when the kernel cannot tell whether a
  client holds the device.
- ZFS opens a vdev exclusively, and the kernel can tell that
  (`skyzfs-ublk/src/lifecycle.rs:110-119`). So "unknown" with the
  pool imported on the device arises only where the driver could not
  be asked.

**How it can be settled.**
- The next daemon start settles "unknown" by what the driver says
  about the recorded number ("free, or another's, not held") and
  writes the answer into the note.
- An acknowledging start then serves the volume afresh.

**Scenario.**
1. The pool is exported for maintenance while the daemon serves.
2. The volume fails, and the note says "unknown".
3. `run`'s next round takes the step, finds the pool not imported
   (`step=not-imported`) and ends with 9.
4. The operator settles the note and serves the volume again.
5. No agent runs, and the docs say none is to be started before a
   reboot.

The same holds for any `step=not-imported` with a note that says
"unknown".

**Suggestion.**
- "Unknown" with the pool not imported: say so and let the round go
  on, as for held = No.
- "Unknown" with the pool imported: take the step.

The lead ruled on a torn note (Q1). Treating "unknown" as lost is the
author's own reading ("Reading the note" in the description).

### F5. The agent's 9 and 10 are also the daemon's 9 and 10, with other meanings (nit; certain)

- `bins/skyzfsd/src/cli.rs:430-461`: the daemon's 9 is NotStarted
  ("Retry the serve once"), and its 10 is LeaseHeld.
- The agent's 9 means "do not restart before a reboot", and its 10
  means "restart at once, on the device as it is now".
- The docs say the codes are distinct. Still, a unit or a rig that
  copies one binary's restart rules to the other gets 9 the wrong way.
- No unit in the tree uses either code yet, so other numbers (11, 12)
  are cheap now.

## Test gaps (plants that survive)

71 plants. 57 are killed and 14 survive. None timed out, and every
plant built. The full list is under "Plants" below.

- **A-P5: a begin before the step.** It survives, although the test
  `run_on_a_lost_volume_makes_the_pool_read_and_ends_with_9` lists it
  as a mutant.
  - The test's "nothing begun" is `world.programs.is_empty()`. The fake
    fills `programs` at the take, which only the settle makes, so a
    begin that only records its intent is not seen.
  - Checked: under the plant the round line reads `under_way=1`.
    Adding `assert_eq!(field(lines[1], "under_way"), "0")` to the test
    kills the plant, and the test still passes at the head.
- **The error paths are untested.** Seven plants survive there:
  - **A-C6b and R-D1: a read error taken as "none".**
    - A-C6b is a read error of the pool's kstat, taken as "not
      imported". R-D1 is a read error of the failure note, other than
      NotFound, taken as "no note".
    - The fake host's `read` can only answer NotFound
      (`host/fake.rs:94-99`), so no test can show either. A-C6b is a
      mutant that `the_pool_state_is_the_kstat_word` lists ("an error
      as no file").
    - So the description's "the command runner (7 of 7)" is not what I
      found. Of my 9 renderings of the mutants its tests list (A-C1 to
      A-C8), 8 are caught and A-C6b is not.
  - **R-L5, R-L4 and R-P1: the pool's state cannot be read.**
    - R-L5 is a state that cannot be read before the reopen, taken as
      "not imported".
    - R-L4 is the same after the reopen.
    - R-P1 is the step's error dropped by `run` (the round says `ok`,
      and nothing is on standard error).
  - **R-P2 and R-P3: the note cannot be read.**
    - R-P2 is `run` ignoring the error.
    - R-P3 is `publish` and `settle` letting the error through.
  - These are the paths of F2 and F3. The tests for them are cheap:
    - A wrapper of a few lines around the fake's `Zfs` gives a state
      that fails. The scratch tests R4 and R5 do it, and would kill
      R-L4, R-L5 and R-P1.
    - A wrapper around the fake daemon whose `failure` fails does the
      same for the note. The scratch test R6 does it, and would kill
      R-P2; the same wrapper under `publish` kills R-P3.
    - A `read` of the fake host that can be told to fail (EACCES, EIO)
      closes A-C6b and R-D1.
- **R-L1: a pool gone during the look is not taken as the end of the
  look.** It survives, and the description says so itself ("Not
  covered").
- **R-C2: a reopen ended by a signal (no exit status) is read as
  done.** It survives. The line then says `reopen=ok`. Low: the look,
  not the reopen, is the signal. A-C4 (a reopen past its limit) is
  caught.
- **Survivors that are fine:**
  - **A-E2 (N3)** is the author's own survivor; see the nits.
  - **R-L2: `LOOK_MAX` is not pinned.** The test reads the constant.
    That is right while the lead's limit is provisional. `LOOK_EVERY`
    (R-L3) and `Limits::reopen` (R-C3) are pinned.
  - **R-F1 (a longer note accepted by the length check) is
    equivalent.** The padding comparison `bytes[56..] == [0; 8]` also
    fails on any longer file.
  - **R-F7: `parse_boot_id` takes more than 32 hex digits.** This is in
    the moved code and was already a gap before it moved. The kernel
    always gives a uuid.

## Nits

- **The description says the daemon tests read "real files".**
  `daemon/tests.rs:74-89` puts the note and the boot id in the fake
  host's map. The test is sound; the description's words are not.
- **N3 is not quite equivalent.**
  - `restart` also runs inside a round, after an error of a begin or
    of a sync (M2.7d).
  - Where a `cuts` read earlier in that round set `moved`, N3 clears it
    at that restart.
  - The end with 10 then comes one round later, at the next read.
  - Harmless; "equivalent" is a little strong.
- **A note that cannot be read** (EACCES, or a directory in its place):
  - `run` records the error and goes on to begin a publish in the same
    round (R6; `program.rs:1291`).
  - `publish` refuses with 1 in the same case.
  - It would be consistent for a round to begin nothing when it cannot
    tell whether its volume failed.
  - Low: the daemon is likely gone anyway.
- **The docs of exit 9 at `program.rs:359-366`** say "`run` made the
  pool read". With `step=not-imported` it did not. Fold this into
  F2's docs.
- **`zpool reopen` without `-n`.** Under the author's first reading (a
  wrong `--pool`), the reopen of a healthy pool also reassesses its
  leaves, and a leaf with a dirty time log starts a resilver. Low, and
  from my reading of OpenZFS, not run here. It is only a note for the
  docs that say a wrong pool "is made to read once, which suspends
  nothing" (`engine/lost.rs:36-41`).
- **For testing.** The agent ignores the daemon's test hook
  `--boot-id-file`: it compares with the kernel's boot id. A course
  that runs the agent beside a daemon started with that hook would
  read no note. Only `replay_daemon.py` uses the hook today.

## Decisions for the lead

1. **F1.** Is "within seconds of the failure" a property of `run` at
   its default schedule (then F1 blocks), or is the rig to show it
   with a short `--every-s` (then F1 is a follow-up)?
2. **F2.** Should `run` end with 9 where the step did not show the
   pool suspended, or fail the round and try again?
3. **F4.** Is a note that says "unknown", with the pool not imported, a
   lost volume?
4. **F5.** Keep 9 and 10, or move the agent's codes to numbers the
   daemon does not use?
5. **The author's readings under "Not done".** I checked each against
   the code.
   - **The step acts on `--pool` with no premise check.**
     - The code does what the description says (`engine/lost.rs:36-41`).
     - A wrong `--pool` reopens a healthy pool and leaves the lost one
       ONLINE, while the line says `lost` and `run` ends with 9.
     - With F2's suggestion, the line would at least show
       `state=ONLINE` and the round would fail.
     - I agree with the reading, given F2.
   - **`publish` and `settle` refuse held = No with 1.** As described
     (`program.rs:989-998`). Agreed: the volume can be served again.
   - **`recover` and `record` do not read the note.** As described
     (R7). Agreed: neither needs a live daemon for what it does.
   - **held = No in `run`: a line each round, and the round goes on**
     (it then fails at the daemon). Agreed. It also chains well with
     exit 10: an acknowledging start gives a new attachment, and `run`
     ends with 10 to be started on the new device.
   - **The look is not cut short by a signal.** It lasts at most 10 s,
     and `run` ends after it. Agreed.
   - **The 10 s limits are provisional.** Agreed. `LOOK_MAX` is not
     pinned (R-L2), which is right for now. The reopen's limit is
     pinned (R-C3 is caught).

## What was checked

### The author's claims, against the diff

| Claim | Found |
|---|---|
| The note's code moves to `skyzfs-control`, and nothing else changes for the daemon | Yes: see "The moved code" below |
| The note is read beside the socket, only for this boot; a torn note is read as such without the boot | Yes (`daemon.rs:66-84`). A torn note is not checked against the boot, which is how the daemon's start counts it |
| Three outcomes; torn counts as held (Q1) | Yes (`engine/lost.rs:151-161`) |
| The step: the kstat word; not imported or SUSPENDED means nothing is done; then the reopen within 10 s and a look every 250 ms for at most 10 s | Yes (`engine/lost.rs:170-191`, `zfs/cli.rs:1638-1676`). The pool name is checked before the path is built |
| The reopen's error goes to standard error and changes nothing (Q3) | Yes (`program.rs:1259-1268`) |
| On a lost note, `run` prints `lost`, then the round line (nothing begun, nothing synced), and ends with 9 | Yes. But 9 comes whatever the step showed (F2), and only at the next tick (F1) |
| "A restart on the same note finds the pool suspended, makes no second reopen" (Q4) | Only where the first step did suspend it (R2: a second reopen). The code docs state it unconditionally (F2) |
| held = No touches no pool, and says so each round (Q2) | Yes (`program.rs:1243-1251`; R-P5 shows the words are tested) |
| D4: the attachment at the first `cuts` and the device at the first premise check; exit 10 after the round line; a take-over is not a move | Yes (`engine.rs:1854-1890`, `program.rs:1399-1406`). An error that ends `run` wins over 10 |
| `publish` and `settle` refuse: lost with 9, held = No with 1 | Yes. A note that cannot be read is refused with 1 too |
| The tests listed | All present, with the counts given: daemon 2, cli 2, lost 9, failed_volume 8, failure 7 (5 new, 2 moved) |
| The daemon tests use "real files" | No: they use the fake host's map (nit) |
| Mutants caught: the note's read 3 of 3, the command runner 7 of 7, the program 6, the engine's step and watch 8; N3 equivalent | My renderings of the listed mutants: the note's read 3 of 3; the command runner 8 of 9 (A-C6b survives); the program 13 of 14 (A-P5 survives); the engine's step and watch 17 of 18 (N3, A-E2, survives); the words 1 of 1 (A-F1). N3 is not quite equivalent (nit) |
| The gate table | Every row I ran matches (below) |

### The moved code

- `crates/skyzfs-control/src/failure.rs` holds what `skyzfs-ublk`'s
  `records.rs` and `lifecycle.rs` held:
  - `Reason`, `Held`, `Failure`, `StoredFailure` and the code table;
  - `encode_failure` and `decode_failure`, now public;
  - `read_failure` and `read_if_there`;
  - `parse_boot_id`, `BOOT_ID` and `boot_id`.
- The bodies are the same as before. The only additions are
  `Reason::word` and `Held::word`.
- `skyzfs-ublk` re-exports what it used:
  - `lifecycle.rs`: `pub use ... {Failure, Held, Reason}`;
  - `records.rs`: `encode_failure` and the read functions.
- The daemon's writes and removals did not change:
  - `failure_path` (`volume_run_dir/failed`);
  - `remove_failure` at an acknowledging start;
  - `note_failure` writing Unknown.
- **Tests.**
  - `the_boot_id_is_the_kernels_uuid` and
    `the_failure_note_has_the_documented_layout` moved verbatim from
    `skyzfs-ublk/src/records/tests.rs`.
  - The lists of test names before and after (edf9a17 and 29f5538)
    differ only by those two moves and the 26 new tests (2160 to
    2186). No test is dropped.
  - `skyzfs-ublk`'s integration test `failure_note` still passes (it
    runs with every R-F plant).
- **On a kernel.** The daemon rig on 29f5538, on all three images,
  passes every failed-volume pin (exit 3, a held volume refused with
  4, an unheld one served when acknowledged, the pool suspended at a
  failed start and at a failed sync). The one WAITING pin is the
  request-held one, as its table expects. So the moved note code works
  end to end from the daemon's side.

### Gates

`gates.sh`, in a scratch worktree at 29f5538, after `rustup update
stable` (rustc 1.100.0, 2026-11-09). The target directory is cleaned
between feature sets. Output: `gates.out`.

| Gate | Plain | `skyzfsd/test-hooks,skyzfs-local/test-hooks` | `skyzfs-ublk-sys/control-test-program` | `skyzfs-agent-cli/test-hooks` |
|---|---|---|---|---|
| `cargo fmt --all -- --check` | clean | | | |
| clippy `--workspace --all-targets -D warnings` | clean | clean | clean | clean |
| `cargo test --workspace` (passed / failed / ignored) | 2045 / 0 / 16 | 2103 / 0 / 16 | 2069 / 0 / 18 | running locally at the end (CI's `test` and `unit-arm64` ran `cargo test -p skyzfs-agent-cli --features test-hooks`: green) |
| `RUSTDOCFLAGS=-D warnings cargo doc --workspace --no-deps` | clean | clean | clean | running locally at the end (CI does not build docs with this feature) |
| `skyzfs-agent`, `skyzfs-agent-cli`, `skyzfs-control` tests 20 times | 0 failed runs of 20 (the last: 311 passed) | | | |

### Plants

`plants.py`, in a scratch worktree at 29f5538.
- **Method.** One plant at a time, with `cargo test -p skyzfs-agent -p
  skyzfs-control -p skyzfs-ublk --lib`, plus `skyzfs-ublk`'s
  `failure_note` test where `failure.rs` is planted. 600 s per plant.
  The worktree is restored with `git checkout` after each plant.
- **Two kinds.** A-.. re-make the author's mutants from the "Mutants:"
  lines in the new tests' docs (the description gives only counts).
  R-.. are mine.
- **Result.** BASE passes 464 tests. Output: `plants.out`.

| Plant | Result |
|---|---|
| A-D1 the note read elsewhere | killed: `a_note_whose_boot_cannot_be_told_is_an_error` and 1 more |
| A-D2 the boot not compared | killed: `the_failure_note_is_read_beside_the_socket_in_its_boot` |
| A-D3 a torn note taken for none | killed: `the_failure_note_is_read_beside_the_socket_in_its_boot` |
| A-C1 the reopen within a change limit | killed: `the_reopen_is_zpool_reopen_within_its_limit` |
| A-C2 the reopen error confirmed with zpool list | killed: `the_reopen_is_zpool_reopen_within_its_limit` |
| A-C3 a failed reopen read as done | killed: `the_reopen_is_zpool_reopen_within_its_limit` |
| A-C4 a reopen past its limit read as done | killed: `the_reopen_is_zpool_reopen_within_its_limit` |
| A-C5 the wrong kstat path | killed: `the_pool_state_is_the_kstat_word` |
| A-C6a no file read as an error | killed: `the_pool_state_is_the_kstat_word` |
| A-C6b an error read as no file | **survives** |
| A-C7 the word not trimmed | killed: `the_pool_state_is_the_kstat_word` |
| A-C8 a name that leaves the directory read | killed: `the_pool_state_is_the_kstat_word` |
| A-L1 Unknown taken as not held | killed: `the_note_says_what_became_of_the_volume` and 1 more |
| A-L2 a torn note taken as none | killed: `the_note_says_what_became_of_the_volume` and 2 more |
| A-L3 a torn note taken as not held | killed: `the_note_says_what_became_of_the_volume` and 2 more |
| A-L4 No taken as lost | killed: `the_note_says_what_became_of_the_volume` and 2 more |
| A-L5 no reopen | killed: `a_refused_reopen_is_still_looked_after` and 5 more |
| A-L6 the reopen taken for the signal (no look after a reopen that returned) | killed: `a_reopen_then_a_look_until_the_pool_suspends` and 4 more |
| A-L7 the look not repeated | killed: `a_refused_reopen_is_still_looked_after` and 5 more |
| A-L8 the look not waited for between reads (half the wait) | killed: `a_reopen_then_a_look_until_the_pool_suspends` and 1 more |
| A-L9 the look bounded at twice its limit | killed: `the_look_ends_at_its_limit` |
| A-L10 the step ended at the reopen error | killed: `a_refused_reopen_is_still_looked_after` |
| A-L11a a pool not imported reopened | killed: `no_reopen_of_a_pool_not_imported_or_suspended` and 1 more |
| A-L11b a suspended pool reopened | killed: `no_reopen_of_a_pool_not_imported_or_suspended` and 2 more |
| A-E1 the attachment not compared | killed: `a_new_attachment_is_a_move_and_a_take_over_is_not` and 1 more |
| A-E2 N3 | **survives** |
| A-E2b N3b | **survives** |
| A-E3 the move forgotten at a later read that matches the first | killed: `a_new_attachment_is_a_move_and_a_take_over_is_not` |
| A-E4 the device not compared | killed: `a_new_device_number_is_a_move` |
| A-E5 the old 8-bit split | killed: `a_device_number_splits_as_glibc_splits_it` and 1 more |
| A-P1 run makes no reopen | killed: `a_torn_note_is_a_lost_volume_named_torn` and 2 more |
| A-P2 run ends lost with 1 | killed: `a_pool_not_imported_is_not_reopened` and 3 more |
| A-P3 the lost round line left out | killed: `run_on_a_lost_volume_makes_the_pool_read_and_ends_with_9` |
| A-P4 the lost line after the round line | killed: `a_torn_note_is_a_lost_volume_named_torn` and 2 more |
| A-P5 a begin before the step | **survives** |
| A-P6 Unheld | **survives** |
| A-P7 Unheld | **survives** |
| A-P8 no refusal of the one-shots | killed: `publish_and_settle_refuse_a_failed_volume` |
| A-P9 one code for both refusals | killed: `publish_and_settle_refuse_a_failed_volume` |
| A-P10 a refusal that reopens | killed: `publish_and_settle_refuse_a_failed_volume` |
| A-P11 no end on a move | killed: `run_ends_with_10_when_the_attachment_changes` |
| A-P12 an end on a move before the round line | killed: `run_ends_with_10_when_the_attachment_changes` |
| A-P13 an end at a take-over (any second cuts a move) | killed: `a_new_attachment_is_a_move_and_a_take_over_is_not` and 20 more |
| A-P14 a lost line field misnamed | killed: `a_pool_not_imported_is_not_reopened` and 2 more |
| A-F1 two reasons with one word | killed: `every_reason_has_its_own_word` |
| R-L1 a pool gone during the look not taken as the end | **survives** |
| R-L2 LOOK_MAX 60 s | **survives** |
| R-L3 LOOK_EVERY 100 ms | killed: `a_reopen_then_a_look_until_the_pool_suspends` and 1 more |
| R-L4 an error of a look swallowed (read as not imported) | **survives** |
| R-L5 the state before the reopen | **survives** |
| R-P1 run | **survives** |
| R-P2 run | **survives** |
| R-P3 one-shots | **survives** |
| R-P4 run | **survives** |
| R-P5 run | **survives** |
| R-P6 run | **survives** |
| R-D1 the note | **survives** |
| R-D2 the note | **survives** |
| R-D3 the note | **survives** |
| R-C1 a state of two words taken | killed: `the_pool_state_is_the_kstat_word` |
| R-C2 a reopen ended by a signal read as done | **survives** |
| R-C3 Limits::reopen 60 s | killed: `the_reopen_is_zpool_reopen_within_its_limit` |
| R-F1 decode | **survives** |
| R-F2 decode | **survives** |
| R-F3 decode | **survives** |
| R-F4 decode | **survives** |
| R-F5 encode | **survives** |
| R-F6 decode | **survives** |
| R-F7 parse_boot_id | **survives** |
| R-F8 read_failure | **survives** |
| R-F9 Held word unknown as no | killed: `run_started_again_on_the_same_note_ends_with_9_and_no_reopen` and 1 more |

### Scratch tests

`scratch-tests.rs` is a module appended to
`crates/skyzfs-agent/src/program/tests.rs` in a scratch worktree, and
was not committed anywhere. Output at 29f5538:
`scratch-tests-head.out`.
- R1 fails at the head: F1.
- R1b, and R2 to R7, pass. They record the behaviour that F1, F2 and
  F3 and the nits describe.

### CI at 29f5538

Every check run completed with success; the rest were skipped by
design (the `skyzfs-ci` duplicates of jobs `ci.yml` runs on a pull
request, and the replay rig's reference jobs, which this diff does not
trigger). The rigs that drive the changed files:

- **The daemon rig** (`crates/skyzfs-control/`, `crates/skyzfs-ublk/`):
  - jobs: ubuntu-24.04, 26.04 and 26.04-arm, on the daemon;
  - the failed-volume pins are as above; the judge passed, and no
    line differs from its table.
- **The control rig** (`crates/skyzfs-ublk/`): all three images
  passed.
- **The replay rig, through skyzfsd** (`crates/skyzfs-control/`,
  `crates/skyzfs-ublk/`): 24.04 and 26.04, and "write into a hole" on
  both, passed.
- **The agent rig.** It drives `publish` and `settle` through skyzfsd,
  with kills that take over (jobs a1, a2, d, k), and all passed.
  - It never runs `run`, a failure note, the lost step or exits 9 and
    10. Its `FIRST_RC` has neither code.
  - So nothing on a kernel shows this package's new behaviour. That is
    Q6, testing's course.
- **Other runs.**
  - the publish rig and the cut rig (`skyzfs-cut-daemon`), through
    skyzfsd;
  - `ci.yml`: lint, test, unit-arm64, s3-backends, chart-lint;
  - coverage, e2e and k3s-smoke.

  All passed.

Codex posted its review summary at 29f5538 with no findings, and no
review comment is open.

**After the merge of #171.** `zfs-topology` now also holds #171
(testing's run course for the agent rig, job R: `skyzfs-agent run`
under kills, signals, a held record, a bad record and a rewind import).
#173's CI was built before that merge, so no run of the course on
#173's code exists. From reading the course, its daemon restarts are
take-overs (same attachment) and it leaves no failure note, so neither
exit 10 nor exit 9 should arise there. That is read, not run: the agent
rig should be dispatched on `zfs-topology` after this pull request
merges (or on the pull request once it is updated to the new base).

## Files here

- `review.md`: this note.
- `gates.sh` and `gates.out`: the gates.
- `plants.py` and `plants.out`: the plants. `SCRATCH` names a scratch
  directory that holds the worktree.
- `scratch-tests.rs` and `scratch-tests-head.out`: the scratch tests,
  with their output at the head.

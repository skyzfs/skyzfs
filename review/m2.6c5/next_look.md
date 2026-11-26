# Next look at skyzfs/skyzfs#157 (M2.6c-5) at 4edc1b3

Head checked with `gh api repos/skyzfs/skyzfs/pulls/157 --jq .head.sha`:
`4edc1b3cb4f8c5148f17b6b7040ff3626d3bda89`, branch `skyzfs-coding-m2.6c-5`.
New since my review of e35f9e9 (`review.md`): `8bf35c2`, `17d057c`,
`3541ba1`, `d718e26`, `4edc1b3`, 14 files. I read that diff, the
author's answer (issuecomment-7182860222), both recordings the new tests
use, and libzfs's source for where `-s local` comes from.

## Verdict

**Nothing blocks the merge.** B1 is fixed, and the fix is pinned three
ways:

- on recorded zfs output from both images;
- through the real adapter inside the engine;
- by a unit case.

T1 to T4, N1, N2, N4 and N5 are done as claimed, each checked by a
plant. Codex's review of 4edc1b3 completed with no findings.

What is left is low:

- one test gap (T5): a filter wider than the two names is not pinned;
- one nit carried over (N3, documentation only);
- two new nits.

R1 is with the lead and I did not judge it again. The daemon rig and
`coverage` were still running when I read CI (below).

## B1: fixed

- **The fix.** `COUNTERS` and `carried` (`crates/skyzfs-agent/src/zfs/cli.rs:1208-1214`)
  leave `filesystem_count` and `snapshot_count` out in two places:
  - the final loop (`:766`);
  - the reads alone (`:758-760`).

  The name match is exact, the limits stay, and the agent's own property
  is still left out.
- **Is the named list complete?** Yes, against both the recordings and
  libzfs.
  - **The recordings.** I listed every property with source `local` in
    every `zfs get` of both artifacts (#156's run 45738224324 and #158's
    run 45751625028, both images). They are:
    - `compression`, `mountpoint`, `refreservation`, `volsize`;
    - `snapshot_limit`, `filesystem_limit`;
    - the two counters;
    - user properties.

    Read-only properties such as `used`, `written`, `refcompressratio`,
    `logicalused` and `compressratio` never appear.
  - **Before and after a take.** I compared `*-odd-get-local` (before)
    with `*-odd-get-received` (after `zfs snapshot -r` and a receive),
    on both pools, both images and both runs. Every line that differs is
    one of three kinds:
    - `snapshot_count` (15 lines, each one higher);
    - the received dataset's own lines;
    - its parent's `filesystem_count`.

    No other local value changes at a take (`take_diff.py`).
  - **libzfs.** OpenZFS zfs-2.4.1, `lib/libzfs/libzfs_dataset.c:2254-2270`,
    `get_numeric_property` gives a source of "not default, must be
    local" to exactly ten properties:
    - `canmount`, `volsize`, `quota`, `refquota`, `reservation`,
      `refreservation`;
    - `filesystem_limit`, `snapshot_limit`;
    - `filesystem_count`, `snapshot_count`.

    Only the last two are read-only, and the other eight are settings a
    person makes. Every other read-only number takes the `default:`
    branch. Its source is what the kernel's nvlist gives, which is none
    or default, so `-s local` does not list it (`get_source`,
    `:2376-2399`).

    So no other derived property can be listed as local and change at a
    take. The comment at `cli.rs:1201-1207` cites the right function.

## Ask 1: the recordings tests

**Both tests use recorded zfs output.** I compared each fixture with the
run's artifact, downloaded to a new scratch directory and read as data
(`cmpfix.py`):

- `recorded/cli-read-ubuntu-{24.04,26.04}.json` against run 45751625028:
  32 of 32 records match on each image. The command, rc, stderr and
  stdout match byte for byte, `stdout_hex` included.
- `recorded/read-ubuntu-{24.04,26.04}.json` against run 45738224324:
  32 of 32 match on each image. stdout matches byte for byte (by
  `stdout_hex` where the bytes are not UTF-8).

**`the_three_reads_as_recorded` (`recordings.rs:1166`).** Its fake
answers only the recorded commands' words (`read_recorded`, `:1120`).

- The list, the pairs pass and the values pass are therefore the
  adapter's exact commands, as zfs printed them. A word that differed
  would fail the read.
- The values read alone are recorded for the 11 odd values and for the
  gone child. The other values read alone (for example `skyzfs:note` of
  `child`) are made from their line of the values pass. That is sound
  for single-line values, and the test says so.
- It checks the derivation:
  `pairs_of(read-odd-get-values) == read-odd-get-names`.

**`the_manifests_read_as_recorded` (`:984`).**

- The list and the values pass are recorded, and the pairs pass is
  derived.
- Plants D1 and D2 on `pairs_of` show what checks the derivation:
  - D1 (no lines skipped) is killed by both tests;
  - D2 (one line too many skipped) is killed only by the second test.

  So the second test is what checks the first test's derivation, as the
  answer says.

**Would they fail without the fix?** Yes. I ran each plant alone, then
the agent's tests (`-p skyzfs-agent -p skyzfs-agent-cli`, features
`skyzfs-agent-cli/test-hooks`, 600 s limit), then restored the file
(`plants157n.py`, `plants157n.out`).

| Plant | Result | Of the tests asked about, failed |
|---|---|---|
| C1 `snapshot_count` not in `COUNTERS` | KILLED | both recordings tests, the engine test, the unit case |
| C2 `filesystem_count` not in `COUNTERS` | KILLED | the same four |
| C3 `COUNTERS` empty (B1 as at e35f9e9) | KILLED | the same four |
| C4 a counter read alone where its dataset is read again | KILLED | `the_manifests_read_as_recorded`, the unit case |
| C5 the counters read, read alone and kept | KILLED | the same four as C1 |
| C7 the limits left out too (`_limit`) | KILLED | both recordings tests, the unit case |
| C8 `volsize` and `refreservation` left out | KILLED | `the_three_reads_as_recorded` and 2 more |
| C9 the agent's own property kept | KILLED | `the_three_reads_as_recorded` and 1 more |
| **C6** every property whose name ends `_count` left out | **SURVIVED** | T5 |
| **C6b** every property whose name ends `:snapshot_count` left out | **SURVIVED** | T5 |

## Ask 2: the engine test through the adapter

`counts_that_rise_with_a_take_do_not_fail_its_check`
(`engine/tests.rs:4479`, wrapper `ThroughCli` at `:4298`).

- **It goes through the adapter.** `ThroughCli::datasets` first runs the
  world's own read, for its calls and faults. It then renders the world
  as the three outputs and reads them with
  `zfs::cli::ZfsCli::new(..).datasets`. The members the engine compares
  come from that read, not from the world's `members`.
  - The render matches ZFS's counting: `filesystem_count` counts the
    file systems and volumes below a dataset; `snapshot_count` counts
    the snapshots of the dataset and of those below.
  - The render puts both counters on every dataset, and
    `snapshot_limit` on the root.
- **It fails without the fix.**
  - C1, C2, C3 and C5 each kill it.
  - C2 (only `filesystem_count` kept) is caught by the case where a
    dataset is created after the take. The publish is then made on the
    second take, and the test expects one.
  - C4 and C9 do not reach it. That is expected: the render has no
    value with a newline, and the world's assist does not land between
    the reads.

## Ask 3: the macro for `Effect`: pinned by construction

`effects!` (`engine.rs:165-188`) makes the enum, `ALL` (its length is
the list's) and `word` from one list. An effect cannot exist outside
`ALL`.

| Plant | Result |
|---|---|
| Q19a an effect appended to the list, the test's match not extended | KILLED (build: E0004, non-exhaustive match in `every_effect_is_in_the_list_of_all`) |
| Q19b the same with the test's match extended, plus `ALL.contains(&Probe)` and `from_word("PROBE")` asserted | SURVIVED, as expected: the appended effect is in `ALL` and is accepted by its word |
| Q20 two effects with one word | KILLED (`each_effect_is_named_by_a_word_of_its_own`) |
| Q21 two effects swapped in the list | KILLED (`every_effect_is_in_the_list_of_all`) |

T3 is closed. My plant Q19 from the review can no longer be written.

## Ask 4: `4edc1b3` (`bins/skyzfsd/src/kill.rs`, `kill/tests.rs`)

- **Docs only.** Every line the commit adds or removes is a `//!` or
  `///` comment. The `cfg(feature = "test-hooks")` code is unchanged,
  and no other file under `bins/` or `crates/skyzfs-ublk*` changed in
  these five commits.
- **It now states the ruling as the notes record it** (impl notes,
  M2.6c-4, "Ruling, revised on #158"):
  - after a take-over the row may move to the first cut the last
    accepted request accepts: count above its *C*, group at or above its
    *G*, opening as itself (§5.2, Q2 of M2.5);
  - a difference from the kill line's cut is information;
  - a finding against the daemon is either a row older than the kill
    line's cut, or a `snap/T` that node B does not verify.

  It agrees with `skyzfs_upload::publish`'s module docs.
- **The rig's judge** (testing's `tests/kernel/agent_check.py` at
  c580c72, `:690-740`) pins three things:
  - `pin_*_carried_by_the_last_request`: the row is a cut the last
    accepted request accepts;
  - `pin_*_no_older_than_the_killed_cut`, under `carry-swap` only;
  - node B.

  The docs and the judge agree in effect. See N6 for two small
  differences in wording.

## Ask 5: T2, T4, N1 to N5, and a property a person sets

- **T2:** R9 (the order compared by dataset only) is KILLED by
  `a_value_with_a_newline_is_read_again_alone`, through its check of the
  reads alone, by the new case "one dataset's pairs read in another
  order" (`cli/tests.rs:723`).
- **T4:**
  - V1 (`VALUE_MOST` = 8192) and V2 (8190) are each KILLED by both
    recordings tests.
  - `assert_eq!(VALUE_MOST, 8191)` is at the head of the first test.
  - The recorded 8191-byte value is read whole.
- **N1:** done (`cli.rs:207-217`).
- **N2:** done. The reworded paragraph (`cli.rs:126-136`) says:
  - the rule is exact only where nothing changes between the gets;
  - one change can give a value nobody set;
  - that needs write access to the property forged;
  - the check sees it where the two readings differ.
- **N4:** done. `#[cfg(unix)]` is on `volume_run_dir`, `socket_path` and
  their test. Nothing else in `skyzfs-control` uses `std::os::unix`.
  I did not build for a non-unix target.
- **N5:** done. NoPool applies only to a line that is exactly
  `cannot open 'POOL': dataset does not exist` (`cli.rs:407-415`).

  | Plant | Result | Killed by |
  |---|---|---|
  | P1 any `dataset does not exist` (e35f9e9) | KILLED | `datasets_that_cannot_be_read_are_an_error` |
  | P2 a name that begins with the pool's | KILLED | the same |
  | P3 never NoPool for that message | KILLED | the same, and `the_three_reads_as_recorded` (its recorded `read-none-list`) |
- **N3:** half done, as before. See below.
- **A property a person sets is still compared.** I ran a scratch engine
  test through the PR's `ThroughCli` (`scratch157n.rs.txt`): a person
  sets a local property on `pool/db` between the read before each take
  and the take.
  - Each of these properties gives the publish up as `NotWhole` after
    three takes:
    - `com.example:snapshot_count`;
    - `x:filesystem_count`;
    - `snapshot_limit`;
    - `filesystem_limit`.
  - The control row, where nothing is set, is published on one take.
  - The test fails under C6 (`_count`), because the user property is
    then dropped and the publish is made, and under C7 (`_limit`). So
    the test is valid, and the head compares these properties. The
    PR's own tests do not pin it (T5).

## Not blocking

### T5 (low): no test pins that only the two counters are left out (C6 survives)

- **Where:** `cli.rs:1212-1214`. The code is right: the match is exact.
  No test shows a user property named like a counter kept.
- **Scenario:** a later change widens the filter to
  `property.ends_with("_count")`, for example to future-proof it. Every
  test still passes.
  1. A person's `com.example:snapshot_count` is then dropped from the
     manifest, so a restore loses it.
  2. A change to it between the reads is no longer seen by the check.
- **Fix:** add, in `the_counters_libzfs_calls_local_are_left_out`, a
  user property `a:snapshot_count` kept with its value. Or add the
  engine case of `scratch157n.rs.txt`.
- **Sure:** high on the plant. Whether it matters is a judgement.

### N3 (carried over, docs): the first way of an empty `NotWhole` list still says "destroyed before a stop"

- **Where:**
  - `plan.rs:454-460` ("where the last take was destroyed before a stop
    and the give-up made by the next process");
  - the comment at `engine.rs:1097-1101`, which says the same.
- **The course that does not fit (N10's):**
  1. The take is whole but not as before.
  2. The process stops between the failed check's record and the
     destroy.
  3. The next process destroys the take and gives up from `take` with
     no names.

  The take is destroyed after the stop, by the next process.
- **What changed:** the answer added the second way to `differing`'s
  doc, which is right, but left the first as it was.
- **Suggest:** "where the give-up was decided before a stop (its take
  destroyed before the stop or by the next process) and made by the
  next process".

### N6 (new, docs): kill.rs and the rig's judge differ in two words

- **Where:** `kill.rs:90-100`.
- **"The first cut after the last accepted request".** The judge accepts
  any cut that request accepts, and reports the first only as INFO
  (`obs_*_carried_by`). The upload docs (Q2 of M2.5) allow a later cut
  where the first is abandoned. kill.rs cites both, so this is wording.
- **"A row that names a cut older than the kill line's" is a finding
  under either kill point.** The judge pins it under `carry-swap` only.
  Under `carry-put` it follows from the judge's
  `pin_*_carried_by_the_last_request`:
  - the *C* read after the take-over already counts the killed cut;
  - so a cut counted after that *C* is newer than the killed cut.
- **Suggest:** "the first such cut it commits" in kill.rs. A line in the
  judge's docstring could say why the older-cut pin is redundant under
  `carry-put`.

### N7 (new, test): the first recordings test feeds the values pass as lossy text

- **Where:** `recordings.rs:1025` and the values read alone gives
  `record.stdout`, in which bytes that are not UTF-8 are replaced.
- **Effect:** `odd-raw` and `child`'s `skyzfs:raw` reach the adapter as
  U+FFFD, not as the bytes ZFS printed.
- The second test gives the exact bytes (`given`, `stdout_bytes`), so
  nothing goes unchecked. Using `stdout_bytes` in the first test too
  would make it "as recorded" throughout.

## Gates (rustc and clippy 1.100.0, current stable)

- `cargo fmt --all --check`: clean.
- `cargo clippy --all-targets -- -D warnings`: exit 0 on each of four
  builds:
  - the workspace, plain;
  - `skyzfsd --features test-hooks`;
  - `skyzfs-ublk-sys --features control-test-program`;
  - `skyzfs-agent-cli --features test-hooks`.
- Tests, 0 failed in every run:
  - the workspace: 1949 passed, 16 ignored;
  - `skyzfsd` with test-hooks: 200 passed;
  - `skyzfs-ublk-sys` with control-test-program: 37 passed, 11 ignored;
  - `skyzfs-agent-cli` with test-hooks: 7 passed.
- `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps`: exit 0 on each of
  four builds:
  - the workspace, plain;
  - the workspace with `skyzfsd/test-hooks`;
  - the workspace with `skyzfs-agent-cli/test-hooks`;
  - `skyzfs-ublk-sys` with `control-test-program`.

## CI at 4edc1b3 (read at about 05:10 UTC)

CI_PLACEHOLDER

## Files

| File | What it is |
|---|---|
| `plants157n.py`, `plants157n.out` | the plants of this look and their results |
| `scratch157n.rs.txt` | the scratch engine test (appended to `engine/tests.rs` for one run, then removed) |
| `cmpfix.py`, `props.py`, `take_diff.py` | the fixture comparison with the artifacts, the local properties listed, and the before/after-take diff |

None of these files is product code. The worktree was clean after every
plant and after the scratch test.

# Review of skyzfs/skyzfs#157 (M2.6c-5) at e35f9e9

Head checked with `gh api repos/skyzfs/skyzfs/pulls/157 --jq .head.sha`:
`e35f9e964e937372924d75a37883331903a68d25`, branch `skyzfs-coding-m2.6c-5`,
base `zfs-topology` at 77da00c (the merge base). Reviewed
`git diff origin/zfs-topology...origin/skyzfs-coding-m2.6c-5`, 19 files.
I read the code and design §5.4 and the notes of #155 and #156 before
the description.

## Verdict

**One finding blocks the merge: B1.** On any pool where a file-system or
snapshot limit was ever set, `zfs get -s local` lists `snapshot_count`
and `filesystem_count` as local. Each take raises `snapshot_count`, so
the reading before the take and the take's members never agree, and
every publish of that pool is given up as `not-whole`. This is
Codex's P1 on `cli.rs:748`, and the recordings of #156 show it on both
images. The base's channel program did not carry these counts, so this
is a regression.

The rest is low: two test gaps that plants expose, one unmeasured
scaling point, and nits. The read's rule itself is sound when nothing
changes between the two gets. The socket fix is right. The items carried
from #153 are answered: T8, T11 and T12 are pinned by plants, and so is
T13 for an effect removed from `ALL`. Two items are open in part: T13
does not catch an effect appended to the enum and left out of `ALL` (T3),
and N11 is half done (N3).

## Blocking

### B1 (high, blocks): the counts are read as local, and the take changes one, so every check fails

- **Where:**
  - `crates/skyzfs-agent/src/zfs/cli.rs:740-750` keeps every property
    of the get but `skyzfs:assist`.
  - `crates/skyzfs-agent/src/engine.rs:482-487` (`same`,
    `dataset.local == member.local`) compares them, called from
    `adopt` at `:1239-1241`.
- **Evidence (recorded):** testing's odd-values run 45738224324 (#156),
  both images (24.04 and 26.04) and both pools (rig and mirror).
  - `*-odd-get-local` is the agent's own command with the value column,
    `zfs get -r -H -p -t filesystem,volume -s local -o
    name,property,value all P`. It lists `filesystem_count 0` and
    `snapshot_count` on `P/child`, where `snapshot_limit` had been set
    and lifted. It lists them also on every one of the 14 `P/odd-*`
    file systems, which were created afterwards with no limit of their
    own. Once the pool's `filesystem_limits` feature is active, each
    dataset created later carries both counts.
  - After `zfs snapshot -r P@g`, `*-odd-get-received` lists, with source
    `local`, `P/child snapshot_count` going from 3 to 4 and every
    `P/odd-*` going from 0 to 1. Same on both images and both pools.
  - The base's full program, recorded in the same run (`*-odd-full`),
    carried none of them: `P/child` local = `skyzfs:note`, `skyzfs:raw`,
    `skyzfs:utf8`, `snapshot_limit`.
- **Scenario:** a pool where someone once set `snapshot_limit` (or
  `filesystem_limit`) on any dataset, and a dataset created since.
  1. `take` reads `snapshot_count = N` on that dataset.
  2. `zfs snapshot -r pool@t` runs, and the after-read in `adopt` gives
     `N+1`.
  3. `as_before` is false, so `failed` destroys the take and takes
     again under the next name.
  4. The same happens to the second and third takes. The publish is
     given up as `Why::NotWhole`, its report names the counted
     datasets, and so it goes for every publish of that pool.

  The engine's half of this is the PR's own
  `a_take_not_as_before_names_the_datasets_that_differ`
  (`engine/tests.rs:2547`): a property that changes as each take lands
  gives the publish up after three takes. The read's half is the
  scratch test `review_b1_snapshot_count_differs_across_the_take`
  (`scratch157.rs.txt`): `ZfsCli::datasets` over the two states gives
  `local` maps that differ in `snapshot_count` alone.
- **Also `filesystem_count`:** it changes on every ancestor with counts
  when a dataset is created or destroyed beneath it. In the base, a
  dataset created after the take was harmless: it has no `@t` and is no
  member. Now its parent's count fails the check.
- **The description's deferral does not cover this.** "M3: the restore
  skips read-only properties" is about setting them at a restore. This
  failure happens at the publish, now.
- **Fix (suggested):** leave both counts out at the read, as the base
  did. They are read-only and never "set locally" in the sense of
  design §5.4. Alternatively, leave out every read-only property; that
  is a question for the lead.
  - Test it on the recorded lines: `odd-get-local` before and
    `odd-get-received` after (or the after-take get with `-s local`, if
    testing records it), checked through `datasets` and `taken`.
  - Add an engine test where a count rises at each take and the publish
    is made.
- **Sure:** high. The recordings show the change on both images and
  pools, and the comparison is plain in the code.

## Not blocking

### T1 (medium): the reads are tested on hand-written output only, and recordings exist

- **Where:** `cli.rs:198-202` says the read's output "is not recorded
  yet". The description says fixtures are queued with testing.
- **What exists:** #156's run 45738224324 recorded these on both
  images, two pools each:
  - `odd-get-local` (the third read's command);
  - `odd-list` (the first read's command);
  - `odd-value-K` (a value read alone, for 14 kinds of value);
  - `odd-get-received`, after a take.

  Fed to `parse_listed`, `match_values` and `value`, these lines would
  have shown B1.
- **Suggest:** bring the recordings test in with B1's fix and not later.
  Check that only the datasets whose value holds a newline (`odd-nl`,
  `odd-forge`) are read alone, and that each `odd-value-K` is read
  exactly.

### R1 (low, unmeasured): the list reads every snapshot in the pool, and its limit grows with datasets only

- **Where:**
  - `cli.rs:1185-1197` lists with `-t filesystem,volume,snapshot`, so
    one line for every snapshot of every dataset.
  - The limit is `full_read_limit` (`cli.rs:371-375`), scaled by
    `count` (`cli.rs:690-709`), which counts file systems and volumes
    only.
- **Scenario:** a pool whose datasets keep many snapshots: a person's
  schedule, or the agent's own `@skz-*` once M4's retention keeps them.
  The list's work is datasets × snapshots, and its limit stays
  60 s + 200 ms × datasets.
  - A list past its limit is `Unreadable` at every round.
  - The publish then ends `TooOld`.
  - The base's members program asked `zfs.exists(ds@t)` once per
    dataset.
- **I did not find** a recording of the list with many snapshots per
  dataset. Suggest a recorder line at, say, 1000 datasets × 100
  snapshots. Then either scale the list's limit by the lines, or note
  the bound.
- **Sure:** medium on the mechanism. The size of the effect is unknown.

### T2 (low): the order of properties within one dataset is not pinned (plant R9 survives)

- **Where:** `cli.rs:1408-1413`.
- **Plant R9:** compare only the dataset's name at the first
  difference. Every test passes.
  - The case table's "pairs read in another order" swaps two datasets,
    not two properties of one dataset.
  - The oracle changes nothing between the gets, and with nothing
    changed the order loop never acts (R6 and R7 are killed by the case
    table only).
- **Scenario under R9:**
  1. The pairs are `(d, q1)`, `(d, q2)`.
  2. Between the gets `q1` is inherited, and `q2`'s value is
     `v\nd\tq1\tEVIL`.
  3. The lines are `d q2 v` and then `d q1 EVIL`.

  The head reads `d` again alone; R9 takes `q1 = EVIL` and a cut `q2`.
- **Fix:** add that case to `a_value_with_a_newline_is_read_again_alone`.

### T3 (low, T13 in part): an effect appended to the enum and left out of `ALL` passes (plant Q19 survives)

- **Where:** `engine/tests.rs:3322`, `every_effect_is_in_the_list_of_all`.
- **Plant Q19:**
  - `Effect::Probe` added after `Recover`, with its word in `word()`;
  - `Effect::Probe => 28` added to the test's match;
  - `ALL` left as it is.

  Every test passes, and `--kill-at probe:...` would be refused.
- **Why:** the test checks each element of `ALL` against its place, and
  `ALL.len() == place(Recover) + 1`. A variant placed after `Recover`
  breaks neither.
- **Q18 is killed** (`Failed` removed from `ALL`).
- **Fix:** list the variants once in the test through a small macro.
  The macro generates the exhaustive `match` (so the list must be
  complete to compile) and an array, and the array is compared with
  `ALL`.

### T4 (low): `VALUE_MOST` is not pinned to 8191 (plant R22 survives)

- **Plant R22:** `VALUE_MOST = 8192`. Every test passes, because the
  tests measure from the constant.
- **Fix:** add `assert_eq!(VALUE_MOST, 8191)`, citing the recording:
  8191 bytes was read exactly, and 8192 cannot be set.

### Nits

- **N1.** `cli.rs:198-202`, "its output is not recorded yet", is stale
  (T1). Say it is recorded (#156) and not yet a fixture.
- **N2.** `cli.rs:123-127` says "The same check finds it", meaning the
  before and after readings then differ. They differ only if the change
  is not made again, at the same point, during the read after.
  - With one change between the two gets, a read can hold a value
    nobody set, not just an old one. An example:
    1. `tank/a p:x` is `\ntank/a b\tp:y\tforged`.
    2. `tank/a b p:y` is inherited between the gets.
    3. The lines then name exactly the pairs, in order, so the read
       takes `tank/a p:x = ""` and `tank/a b p:y = forged`.
  - My search found 1087 such values in 200 000 random worlds with one
    change (`review_one_change_between_the_gets`).
  - This is inside the design's "changed and changed back" carve-out.
    It needs the right to change the pair whose line is forged, and the
    manifest then gives nobody more than that right allows. Word the
    doc so; no code change.
- **N3 (N11 half done).** `plan.rs:454-460` gives two ways the
  `NotWhole` list is empty. The first, "destroyed before a stop and the
  give-up made by the next process", still does not describe N10's
  case:
  1. A take is whole but not as before.
  2. The process stops between the failed check's record and the
     destroy.
  3. The next process destroys the take uncounted and gives up from
     `take` with no names, all in one process.

  The T12 test's doc describes this case; the type's doc does not.
- **N4.** `skyzfs-control` builds on any OS today: its client and
  `rustix` are gated to Linux. The new `volume_run_dir` uses
  `std::os::unix::fs::MetadataExt` without a gate
  (`crates/skyzfs-control/src/lib.rs:190-194`), so the crate no longer
  builds for a target that is not unix. Nothing builds such a target
  today. `#[cfg(unix)]` on the two functions keeps the crate as it was.
- **N5.** `cli.rs:391`: for the three reads that name the pool's root
  (`pool_named`), `dataset does not exist` maps to `NoPool` whatever
  dataset the message names. If `zfs get -r` says that of a child
  destroyed during its run and exits non-zero, the report reads "no
  such pool". Both callers in the engine treat `NoPool` and `Other`
  alike, so only the report's words are wrong. I am not sure zfs exits
  non-zero there.

## The read (ask 1)

### The rule is sound when nothing changes between the gets

A newline in a value only adds lines. Each added line is one of three
kinds, and all three are suspect:

- it does not split into three fields;
- it names a pair that was not listed;
- it names a listed pair, whose own line is also printed, so the count
  is 2.

The nearest line before a run of added lines that splits is the head
line of the value that went on, so that value's dataset is read again
alone. Names hold neither a tab nor a newline, so the pairs pass cannot
be forged. When nothing changes, the order loop never acts. It and the
"named by no line" rule matter only when something changes between the
gets.

### Search, end to end through `ZfsCli::datasets`

A simulated `zfs` prints the list, both gets and each value read alone
from a world of values. The scratch tests are in `scratch157.rs.txt` and
their output in `scratch157.out`. The values are made of these atoms:

- a newline, a tab, `-`, CR, a space, `\n` as two characters, 0xff,
  0x7f;
- dataset names, one of them with a space;
- `\tp:x\t`;
- forged lines that name listed pairs and unlisted ones;
- lengths 4095, 4096, 8191, and 8190 followed by a newline.

With nothing changed between the reads:

- **exhaustive:** 1 000 810 worlds (two pairs with up to two atoms each,
  three pairs with up to one), 732 350 of them with reads alone;
- **random:** 200 000 worlds of up to 4 datasets × 4 properties with up
  to 6 atoms each.

Every result is exactly the world, in both searches.

With one change between the gets, the read is not exact, as N2 says.

### Plants on the read

`plants157.py` applies each plant alone, then runs the agent's tests
(`-p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks`) with a 600 s limit, then restores the
file. The results are in `plants157.out`. "Oracle" says whether
`the_values_matched_are_the_values_set` was among the failures.

| Plant | Result | Oracle | Killed by (others) |
|---|---|---|---|
| R1 a pair named twice not suspect | KILLED | yes | newline cases |
| R2 a pair not listed not suspect | KILLED | yes | newline cases |
| R3 a line that does not split not suspect | KILLED | yes | 3 more |
| R4 no walk back to the line before | KILLED | yes | 3 more |
| R5 walk back to the line just before only | SURVIVED | | equivalent: a line that does not split is suspect itself, and its own walk back finds the head |
| R6 a pair named by no line not read again | KILLED | no | newline cases |
| R7 the order not compared | KILLED | no | newline cases |
| R8, R8b at a difference only one side's dataset | KILLED | no | newline cases |
| R9 the order compared by dataset only | **SURVIVED** | | T2 |
| R10 a line longer than a value not suspect | KILLED | no | `a_value_that_cannot_be_read_alone_fails_the_read` |
| R11 a line split at every tab | KILLED | yes | tabs test |
| R12 the value made UTF-8 | KILLED | yes | tabs test |
| R13 every empty line at the end dropped | KILLED | yes | |
| R14 the empty line after the last newline kept | KILLED | yes | 5 more |
| R15 values of datasets read again taken too | KILLED | yes | |
| R16 a value alone: every final newline taken off | KILLED | no | newline cases |
| R17 a value alone: no newline required | KILLED | no | cannot-be-read test |
| R18 a value alone: not bounded | KILLED | no | the same |
| R19 a value alone: bounded one short | KILLED | no | the same |
| R20 the assist property kept | KILLED | no | members test |
| R21 `-s local,received` | KILLED | no | 7 tests |
| R22 `VALUE_MOST` = 8192 | **SURVIVED** | | T4 |
| R23 a line of exactly 8191 bytes suspect (conservative) | SURVIVED | | harmless: one read alone more |

The oracle kills every plant that breaks the rule when nothing changes.
The plants that only matter under a change (R6 to R9) are left to the
case table.

## The items from #153 (ask 2)

| Item | Plant | Result | By |
|---|---|---|---|
| T12 | Q4 no report at the give-up with no take | KILLED | `a_give_up_is_told_at_least_once_whatever_the_stop`, first at the single stop `Failed`-after, not as before |
| T12 | Q7 the last check's report after the give-up's record | KILLED | the same, first at `Abandon`-after, a take not whole |
| T12 | Q7b the report of the give-up with no take after its record | KILLED | the same, first at **`[Failed`-after, `Abandon`-after]**; with the double-stop cases taken out of the test it PASSES (`t12cases157.out`) |
| T12 | Q16 that report kept from the sink | KILLED | the same, single stop |
| T11 | Q13 the word `person` | KILLED | `a_not_whole_report_names_its_datasets`, `each_outcome_report_and_exit_is_told_apart` |
| T11 | Q14 no cap; Q15 `datasets: ` and nothing | KILLED | `a_not_whole_report_names_its_datasets` |
| T13 | Q18 `Failed` left out of `ALL` | KILLED | `every_effect_is_in_the_list_of_all` |
| T13 | Q19 a new effect appended and left out of `ALL` | **SURVIVED** | T3 |
| T8 | N1 nothing named for a take not as before | KILLED | `a_take_not_as_before_names_the_datasets_that_differ` |
| T8 | N2 datasets with no member not named | KILLED | the same |
| T8 | N3 only the first breaking dataset named | KILLED | `a_take_left_out_by_two_datasets_names_both` |
| N13 | names joined by a space | KILLED | `a_not_whole_report_names_its_datasets` |
| (new) | U1, U2 no `Unreadable` report before or after the take; U3 its error not carried; U4 the publish given up on it | KILLED | `a_read_that_fails_is_reported_and_read_again` |
| (new) | U5 the report line keeps the error's line breaks | KILLED | `each_outcome_report_and_exit_is_told_apart` |

The double-stop cases of T12 are what pin Q7b, as the author says.
Without them, the test passes under Q7b; with them, it fails first at
`[Failed`-after, `Abandon`-after]. The control (the head with the
double-stop cases taken out) passes. N10 and N12 are done. N11 is half
done (N3 above).

## The socket (ask 3): right

- **Behaviour is equal.** `volume_run_dir` moved verbatim: `metadata`
  follows a link, and the name is `v-{dev:x}-{ino:x}`.
- **Every caller in product code goes through it:**
  - the daemon's `records::Files::new` (`records.rs:423`), the `ask`
    commands (`bins/skyzfsd/src/commands.rs:719`) and `serve`'s channel
    (`serve.rs:1555`), all through the re-export;
  - the agent's `main` (`bins/skyzfs-agent/src/main.rs:115`), through
    `socket_path`. `publish`, `settle` and `recover` all pass through
    `settle()`, and nothing else in the agent builds a `DaemonSocket`.
- **No other product code builds the path.** The rigs keep their own
  copies, in the same hex form, unchanged by this PR:
  - `tests/kernel/daemon-rig.sh:1590`;
  - `tests/kernel/skyzfsd-ref.sh:114`;
  - `tests/kernel/cut_hold.py:98`, pinned by `test_cut_hold.py:125`.
- **`--volume` is right and `--device` would not be.** The daemon names
  the directory from the volume's directory.
- **Plants:**
  - S2 (no link followed), S3 (decimal) and S4 (another name than
    `SOCKET`) are KILLED by
    `the_socket_is_in_the_volumes_own_run_directory`,
    `the_daemon_and_the_agent_name_one_socket` and the records test.
  - S1 (the agent's `main` back to `RUN/control`) SURVIVES, as the
    description says. Testing's agent rig is the check.
- See N4 for the unix-only gate.

## Gates

Run on rustc and clippy 1.100.0, the current stable.

- `cargo fmt --check`: clean.
- `cargo clippy --all-targets -D warnings`: exit 0 on each of four
  builds:
  - the workspace, plain;
  - `skyzfsd --features test-hooks`;
  - `skyzfs-ublk-sys --features control-test-program`;
  - `skyzfs-agent-cli --features test-hooks`.
- Tests, 0 failed in every run:
  - the workspace: 1945 passed, 16 ignored;
  - `skyzfsd` with test-hooks: 200 passed;
  - `skyzfs-ublk-sys` with control-test-program: 37 passed, 11
    ignored;
  - `skyzfs-agent-cli` with test-hooks: 7 passed.
- `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps`: exit 0 on each of
  four builds:
  - the workspace, plain;
  - the workspace with `skyzfsd/test-hooks`;
  - the workspace with `skyzfs-agent-cli/test-hooks`;
  - `skyzfs-ublk-sys` with `control-test-program`.

## CI at e35f9e9

- **Green:**
  - `skyzfs-ci`, `ci`, `coverage`, `e2e`, `k3s-smoke`;
  - the node B rig, the publish rig, and the cut rig on the daemon.
- **The control rig** (run 45749135299) is green on 24.04, 26.04 and
  26.04-arm: 76 lines on each image, and "Every line is as the table
  has it".
- **The replay rig** (run 45749135251) ran its daemon jobs at the
  pull-request budget, on both images:
  - 24.04: 124 of 2205 prefixes checked;
  - 26.04: 122 of 2151 prefixes checked;
  - every checked prefix passed, and every fsync mark was checked;
  - the hole jobs caught every control.

  That budget does not show design §9's criterion, and does not claim
  to.
- **The daemon rig** (run 45749135120, `skyzfsd` with its test
  hooks, in-place store, checks a b c d e g) is green on 24.04, 26.04
  and 26.04-arm. I read each image's `summary.md`. All three say "Every
  pin and every control is as the table `EXPECTED` in
  `tests/kernel/daemon_check.py` has it", and in each of the 87 rows the
  result is the one expected. Each image has the same counts:
  - 54 PASS;
  - 5 FAIL, all of them controls expected to fail;
  - 13 WEAKER and 12 NOT_EXERCISED, as expected;
  - 2 NA, the counts of the in-place store;
  - 1 WAITING, `pin_failed_volume_while_a_request_is_held`.

  This PR does not touch the daemon's I/O path; the rig ran because
  `crates/skyzfs-ublk/` changed (the re-export).
- **Codex:** one P1, B1 above (discussion r4703575851). I checked it
  against the code and the recordings, and it holds. It is broader than
  Codex says: the counts are on every dataset created after a limit was
  first set anywhere in the pool, not only on a dataset under a limit.

## Files

| File | What it is |
|---|---|
| `plants157.py`, `plants157.out` | the plants and their results |
| `t12cases157.py`, `t12cases157.out` | which case of T12's test kills Q4, Q7, Q7b and Q16 |
| `scratch157.rs.txt`, `scratch157.out` | the search and B1's read |

The scratch module is appended to `zfs/cli/tests.rs` for a run and then
removed. None of these files is product code.

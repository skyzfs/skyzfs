# Review of skyzfs/skyzfs#160 (M2.6c-5b) at 9354462

Head checked with `gh api repos/skyzfs/skyzfs/pulls/160 --jq .head.sha`:
`9354462c639c2c731a7678d5586af757ba4d3322`, branch `skyzfs-coding`, base
`zfs-topology` at 2489dda (the merge base). The pull request has two
commits:

- `214c69c`: the manifest's list limit counts the snapshots;
- `9354462`: a missing pool only where `zpool list` agrees.

It changes 11 files: `skyzfs-agent`, and the docs of
`bins/skyzfsd/src/kill.rs`. Before the description I read:

- the diff;
- design §5.4;
- the open R1 in `docs/skyzfs-impl-notes.md` (M2.6c-5 as built);
- my notes on #157 (`review/m2.6c5/`);
- the recorder's snapshot-scale run 45781769557;
- the agent rig's artifact at this head;
- OpenZFS 2.4.1's `zfs holds`, `zfs list`, `zfs program` and libzfs
  error paths.

## Verdict

**Nothing blocks the merge.**

- **Both fixes do what they say.** The tests pin them: 36 of my 38
  plants are killed, and the two that survive are low (V16: the text
  that reaches the operator; N7b: explained below).
- **The author's 22 plants agree with mine.** My versions of them
  (L1 to L8, V1 to V10, T5a, T5b, N7 and N7b) give the same result: 21
  are killed, and N7b survives for the reason given.
- **The agent rig's `destroy` case ends 3 at this head.** Run
  45792609629, job k: `publish.rc` 3, `abandoned … destroyed`.
- **One process too.** A scratch test that runs the race in one agent
  process (the rig's shape) settles with `Abandoned(Destroyed)` (S1).
  Under the old rule it fails with the rig's `NoPool` error (S1V1).
- **The gates are clean** on all four feature sets.
- **CI:** every run on the head has ended green, the daemon rig
  included. Its table reads as expected on all three images.

Two findings need an answer. Neither blocks, in my view:

- **R1-reach (medium, for the lead).** The list's limit now counts the
  snapshots. But two other reads in every settle also walk every
  snapshot in the pool, and both keep the datasets' scale:
  - the round's `zfs holds -r`, in each round;
  - the sweep's `tagged.lua`, in each process.

  By the recorder's figures, a round's reach on a pool with many
  snapshots barely moves. On 11 datasets `tagged.lua` takes it from
  about 157 000 to about 168 000 snapshots, and `holds -r` very likely
  not at all (inferred: it is not recorded at scale). On 101 datasets it
  does not move. Whether R1 is closed is the lead's call.
- **NAMES (low).** `confirm` matches the pool against every word of the
  command. So a pool named like a command word (`get` above all) turns
  a missing pool into `Vanished`, and the run ends with exit 1.
  Codex's P2 on `cli.rs:579` is the same finding. I checked it against
  the code, and it is right.

The rest is test fidelity and nits. #157's T5, N3, N6 and N7 are done as
claimed.

## Findings, most severe first

### R1-reach (medium, not blocking, for the lead): two other reads walk every snapshot on the datasets' scale

- **What the PR does, and it is right as built.** The manifest's list
  now takes `read + full_per_dataset × (datasets + snapshots)`, at most
  `full_most` (`crates/skyzfs-agent/src/zfs/cli.rs:427-441`).
  - The snapshot pass runs after the dataset count, within a read's
    limit (`:861-866`).
  - A failed pass leaves the list on the gets' limit, which is the
    limit the list had before (`:438`).
  - The sum saturates and is capped at 35 minutes.
  - L1 to L12 below are all killed.

  The list costs about 0.4 ms a snapshot (below), so 200 ms a snapshot
  is about 500 times what it needs.
- **What it leaves.** Two more reads in every settle walk every snapshot
  in the pool. Both keep `read_limit`: 60 s plus 100 ms per dataset,
  with no term for snapshots.
  - **`zfs holds -H -r pool@t`.** `held()` runs it in each round's
    `see()` (`cli.rs:668`, `engine.rs:836`). With `-r`, `zfs` cuts its
    argument at the `@` and walks the whole pool with full handles
    (`ZFS_ITER_RECURSE`, not simple mode). Only then does it filter by
    the snapshot's name (OpenZFS 2.4.1 `cmd/zfs/zfs_main.c:6998-7036`;
    `holds_callback` at `:6920`). This read is not recorded at scale.
  - **`tagged.lua`.** The sweep runs it once per process
    (`cli.rs:1029-1033`, `engine.rs:779`), and so does `begin`
    (`:644`).
- **The figures.** Testing's recorder, run 45781769557 (Ubuntu 26.04,
  one run, no write load), in seconds:

  | Datasets | Snapshots | count | members | `tagged.lua` | the list |
  |---:|---:|---:|---:|---:|---:|
  | 11 | 11 000 | 0.12 | 0.12 | 4.03 | 4.39 |
  | 11 | 60 500 | 0.12 | 0.12 | 22.05 | 23.92 |
  | 101 | 101 000 | 0.12 | 0.12 | 38.64 | 41.19 |

  The list costs about 0.40 ms a snapshot, and `tagged.lua` about 0.37
  ms a snapshot. `hold -r`, `release -r` and `members` stay at 0.12 s:
  they do not walk the snapshots.
- **Scenario: 11 datasets** (10 and the root), with many snapshots kept,
  for example by an hourly snapshot tool:
  - **At about 157 000 snapshots**, the base's list ran past its 62.2 s
    limit. At the head, the list is well inside its limit.
  - **At about 168 000**, `tagged.lua` runs past its own limit of
    61.1 s. Each settle's sweep then fails, with exit 1, in every
    process.
  - **`holds -r`** walks the same handles as the list, which is not
    simple mode either (`-o name,guid,type,createtxg`; `type` is not a
    fast property, `cmd/zfs/zfs_iter.c:244-262`). So it very likely
    fails near the base's list, about 155 000 to 160 000 snapshots.
    This is inferred, not measured.
- **Scenario: 101 datasets.** `tagged.lua` fails first, at about
  183 000 snapshots against its 70.1 s. The base's list failed later, at
  about 197 000 against its 80.2 s. So here the change to the list
  moves the round's reach not at all.
- **Certainty.** The code shows for certain that the two reads keep the
  datasets' scale while they walk every snapshot. The figures come from
  one run on one runner. `holds -r`'s cost comes from the source.
- **What the lead decides.** The open item in the implementation notes
  says "follow-up (a) counts snapshots in the scale". This PR counts
  them for the list alone, and its description says so ("Everything
  else is unchanged"). The two ways on:
  - give `held()`'s `holds -r` and `tagged`'s program a per-snapshot
    term too (`per_dataset` per snapshot would do, by the figures);
  - or rule that R1 meant the list alone, and say in the module's docs
    up to what snapshot count a round is expected to work.

  Neither is a regression: the base failed at about the same count.
- **The snapshot pass's own cost is not measured.** The count runs at
  the start of every round, so each round now walks every snapshot once
  more. It is the cheap kind: `-o name` alone is simple mode
  (`cmd/zfs/zfs_main.c:4044-4046`). The section "What a publish costs"
  (`cli.rs:260-265`) does not mention it (nit A).

### NAMES (low, not blocking): `confirm` matches the pool against every word of the command (Codex's P2, `cli.rs:579`)

- **Where.** `crates/skyzfs-agent/src/zfs/cli.rs:576-579` checks
  `args.iter().any(|arg| names(arg, pool))` for each listed pool.
  `names` (`:1076-1079`) is right for a word that is a dataset name.
  But the words also include:
  - the subcommand (`get`, `list`, `status`, `holds`, `program`);
  - option values (`name`, `property,value`);
  - the hold tag.
- **Scenario.** The agent's pool `tank` has been exported (moved to
  another node), and the node has an imported pool named `get`. ZFS
  allows that name: only names that begin with `mirror`, `raidz`,
  `draid` or `spare`, the name `log`, and names like `c0` are reserved
  (`lib/libzfs/libzfs_pool.c:1222-1226`).
  1. A settle runs `props()`. `zpool get … tank` says
     `cannot open 'tank': no such pool`.
  2. `zpool list -H -o name` lists `get`, and `names("get", "get")` is
     true.
  3. The result is `Vanished`, not `NoPool`.
  4. The sweep (`engine.rs:772-775`) and `see()` (`:824-827`) return it
     as an error, and the process ends with exit 1. The base waited for
     the pool here.

  In practice the word that matters is `get`. `props()`
  (`cli.rs:790-796`) is what both the sweep and `see()` read first, and
  its other words cannot be pool names, which begin with a letter and
  have no comma. `status` (the premise's `zpool status`) matters only
  where the pool leaves between `props()` and the premise.
- **What is right.** These all give the right verdict, and plants V4,
  V11, V12 and V19 check them:
  - a pool whose name begins another's (`tank` and `tank2`);
  - a `/` or `@` after the pool's name;
  - spaces in a word;
  - words that begin with `-`.
- **Fix.** Each caller knows the pool it means. Pass that pool to
  `failed` and `confirm`, and look only for it in `zpool list`'s answer.
  Checking only the last word is not enough: the programs' commands end
  with an argument of the program (the tag, or `t`), and V13 (last word
  only) is killed. A test with a pool
  named `get` listed would pin the fix.
- **Certainty.** Certain from the code. The scenario needs an unusual
  pool name, and the cost is an exit 1 that the next run tries again.
  The author answers Codex's thread anyway, so fixing it here costs
  little.

### Test gaps and test fidelity (low)

- **TF1: the second `zfs holds -r` answer is not what `-r` prints.**
  - **Where.** Both tests answer the read made again with exit 1 and
    `cannot open 'POOL@t': dataset does not exist`:
    - the engine test, `engine/tests.rs:4573-4577`;
    - `a_read_whose_snapshot_vanished_is_read_again`,
      `zfs/cli/tests.rs:1878-1883`.
  - **What zfs prints.** That answer is what plain `zfs holds` prints
    (recorded as `rig-holds-absent` in run 45781769557). With `-r`,
    `zfs` cuts the argument at `@` and walks the pool, which exists. An
    absent `@t` then gives exit 0, no output, and `no datasets
    available` on stderr (`zfs_main.c:7017-7048`). This is not
    recorded.
  - **Why it does not matter here.** Both answers lead to `Held::No`.
    F1 and F2 swap the real answer into the two tests, and both still
    pass.
  - **The ask.** Testing could record `zfs holds -H -r P@absent` on a
    pool that exists.
- **TF2: the program shape in `VANISHED`** (`zfs/cli/tests.rs:1854`,
  `Channel program execution failed: no such pool`) is not a message a
  vanish can give.
  - The agent runs its programs with `-n`, in open context, holding
    the pool's configuration lock for the whole run (`dsl_pool_hold`,
    `module/zfs/zcp.c:1014`). A destroy is a sync task, which takes that
    lock as writer, so nothing is destroyed under a program.
  - `zfs program` words ENOENT as `dataset does not exist`
    (`zfs_main.c:8618`, by `zfs_standard_error`).

  So the retries in `members` and `tagged` are defensive: harmless, but
  never reached on a real system. The docs' "and `no such pool` too in
  some commands" (`cli.rs:555`) names no command that does that (nit B).
- **TF3: the text that reaches the operator is not pinned (V16
  survives).**
  - **The plant.** V16 shows `Vanished` as a fixed `a dataset vanished`,
    without the command or its message, and survives.
  - **What the tests check.** They match the variant's inner string,
    not its `Display`. For `NoPool` the words are pinned: V18 is killed.
  - **Why it matters.** A read that goes on vanishing ends with exit 1,
    and what the operator then reads comes from this `Display`.
  - **The ask.** An `assert` on `to_string()` in
    `a_read_that_goes_on_vanishing_is_an_error_that_names_it` would pin
    it.
- **TF4: the engine test starts a second agent between the ask and the
  destroy**, while the rig's failing case was one process. Scratch test
  S1 runs the same plant in one agent: its point hook arms the destroy
  at `asked:after` and does not stop the agent.
  - **At the head.** Busy, Idle and Abandoned settle with
    `Abandoned(Destroyed)`. ErrLow settles with `Abandoned(Rewound)`,
    as the description says.
  - **Under V1 (S1V1).** Busy fails with
    `Zfs(NoPool("zfs holds -H -r tank@skz-…"))`, which is the rig's
    failure.

  So the engine test's restart hides nothing. Adding S1 would be
  optional.

### Nits

- **A.** `cli.rs:260-265` ("What a publish costs") leaves the snapshot
  pass out of each round's cost.
- **B.** `cli.rs:555`, "and `no such pool` too in some commands", names
  no such command (TF2).
- **C (description).** It says "a `zpool list` that fails gives an
  error, not NoPool, so the engine tries again at its next round". In
  fact an `Other` from `props()` or `see()` ends `settle`
  (`engine.rs:692`, `:708`), and the process exits 1. It is the next run
  that tries again. The code's own docs (`cli.rs:54-55`) are accurate.
- **D (description).** Under N7 it says "These four records per image
  now hold `stdout_hex`". They held it before. The diff removes the
  lossy `stdout` line, and the loader now reads `stdout_hex` alone. The
  result is the same.
- **E (not this PR's).** Two host timing tests failed on the first run
  of three plants (L2, F2, S1), with `--test-threads=8` on 4 CPUs:
  `host::tests::a_program_past_its_limit_goes_with_what_it_started` and
  `an_output_held_open_past_the_end_is_waited_for_only_to_the_limit`.
  Their limit is 300 ms. They passed in the three reruns and in five
  runs at the head. Under load they flake, and the plants' table counts
  only the plants' own tests.

## Questions the brief asked

- **R1's soundness and failure modes.** These hold:
  - The count's failure leaves the bases, and no snapshot pass runs
    then (L12 is killed).
  - A failed pass is `None`, and the list falls back to the gets' limit
    (L6 and L7 are killed).
  - Snapshots taken after the count (the take adds one per dataset)
    are covered with a margin of about 500 times.
  - Huge counts saturate and cap at `full_most` (the test runs
    1 000 000).
  - The units are milliseconds in `Duration`, and the same function
    `grown` serves both limits.

  The engine's and the adapter's docs agree with the code. The open
  points are R1-reach and nit A.
- **The `names` rule.** These words all give the right verdict:
  - odd bytes (the answer is read lossily, and only a pool's name is
    matched);
  - prefix pools;
  - a dataset named like another pool (`tank/get`: the rest after
    `get` does not begin with `/` or `@`, and `tank/get` does not begin
    with `get`);
  - `pool@snap`;
  - `-` words;
  - spaces.

  Only the command words are wrong (NAMES).
- **The conservative readings.**
  - A failed `zpool list` is `Other`, never `NoPool` (V5a and V5b are
    killed).
  - A read that keeps vanishing ends as that error after three reads
    (V3 and V15 are killed).
  - No loop and no hang: `again` is bounded, `confirm` calls `exec` and
    not `run` (so it does not recurse), and `zpool list` runs within
    `read_limit`.
  - The cost of one `held()` that vanishes three times is at most three
    `holds -r` limits and three `zpool list` limits.
  - A real missing pool is never hidden, except by NAMES.
- **Vanished through every match.** Only `NoPool` is special-cased
  (`engine.rs:773`, `:826`, `:831`). `Vanished` takes the generic error
  path everywhere:
  - in `see()` and the sweep, `Error::Zfs`, so exit 1;
  - a hold's or release's failure in `act` is "not moved", as before.

  `hold()` and `release()` call `tagged_snapshots` without `again`. A
  vanish there is "not moved" in `act`, and an error in the sweep's
  release. That needs a race with a person who releases and destroys a
  stale hold by hand: acceptable.
- **The exit for the rig's case.** It is 3:
  1. `held()` reads again and gets `Held::No`;
  2. the planner withdraws (Pending, and the hold not all);
  3. the next round sees `@t` destroyed and gives up
     `Abandoned(Destroyed)`, which exits 3.

  This holds both in the engine test and in S1 (one process). The rig
  at this head agrees: case `destroy`, `withdraw` then `abandon`,
  `publish.rc` 3. The artifact does not show whether the race itself
  was hit in that run.

## #157's items

- **T5: done.** `a_persons_properties_named_like_the_counters_are_kept`
  keeps `com.example:snapshot_count`, `x:filesystem_count` and
  `x:y_count`. T5a (`*_count`), T5b (contains a counter) and T5c
  (`*:snapshot_count`) are all killed by it.
- **N3: done.** Both empty-list courses are now in `Report::NotWhole`'s
  docs (`plan.rs:451-466`) and in `take`'s comment
  (`engine.rs:1097-1104`). They match `adopt`: with no reading it
  retakes without a count and destroys.
- **N6: docs only.** Every changed line in `bins/skyzfsd/src/kill.rs`
  is a `//!` line.
- **N7: done.** The values pass is fed as exact bytes (N7 is killed by
  `the_manifests_read_as_recorded`). The test asserts `a\xff\xfeb\xc3`
  for `odd-raw` and for `child`'s `skyzfs:raw`.
- **N7b.** It survives, as the description says. The reason holds: N7c
  adds an assert that `zfs get … skyzfs:v POOL/odd-raw` is never run,
  and it passes. So no value with odd bytes is read alone in the
  recordings test, and N7b changes only answers to commands that are
  not run.

## Plants

The plant runner is `plants160.py` (this folder). Each plant is applied
alone to a scratch copy of the head as exact text replacements. Then
`cargo test -p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks` runs, with a 600 s limit, and the files are
restored. The raw lines are in `plants160.out`, followed by the rerun of
L2, F2 and S1 (nit E). The worktree was clean after both runs.

| Id | Plant | Result | Killed by |
|---|---|---|---|
| L1 | the list on the gets' limit | killed | `the_lists_limit_grows_with_the_snapshots_counted` |
| L2 | snapshots counted but not added | killed | the same (rerun: that test alone) |
| L3 | the list capped at `most` | killed | both limit tests |
| L4 | the gets on the list's limit | killed | `the_lists_limit_grows…` |
| L5 | the snapshot pass within the count's limit | killed | three tests |
| L6 | a failed snapshot count fails the count | killed | `the_count_as_recorded`, `the_lists_limit_grows…` |
| L7 | a failed snapshot count gives the base | killed | `the_lists_limit_grows…` |
| L8 | snapshots at `per_dataset` | killed | `the_lists_limit_grows…` |
| L9 | the larger of datasets and snapshots, not the sum | killed | `the_lists_limit_grows…` |
| L10 | `count` returns datasets and snapshots | killed | `the_lists_limit_grows…` |
| L11 | the snapshot pass within the base read limit | killed | three tests |
| L12 | the snapshot pass after a failed dataset count | killed | `the_count_as_recorded`, `the_limits_grow_with_the_datasets_counted` |
| V1 | no `zpool list` (the base) | killed | the engine test and five adapter tests |
| V2 | a vanished read not made again | killed | the engine test and two adapter tests |
| V3 | one read more than `VANISHED_READS` | killed | `a_read_that_goes_on_vanishing…` |
| V4 | any word that begins with the pool's name | killed | `no_such_pool_is_a_missing_pool_only_where_zpool_list_agrees` |
| V5a | a failed `zpool list` read as no pool listed | killed | the same |
| V5b | a `zpool list` that does not run read as none | killed | the same |
| V6 | `members` not read again | killed | `a_read_whose_snapshot_vanished_is_read_again` |
| V7 | `held` not read again | killed | the engine test and two adapter tests |
| V8 | the exact-name path not confirmed | killed | `datasets_that_cannot_be_read_are_an_error` |
| V9 | `tagged` not read again | killed | `a_read_whose_snapshot_vanished…` |
| V10 | a listed pool read as `NoPool` | killed | five tests |
| V11 | `pool@t` not the pool's | killed | the engine test and others |
| V12 | `pool/x` not the pool's | killed | `no_such_pool_is_a_missing_pool…` |
| V13 | only the last word checked | killed | two adapter tests |
| V14 | `again` on any error | killed | two tests |
| V15 | `again` bounded at 100 | killed | `a_read_that_goes_on_vanishing…` |
| V16 | `Vanished` shown without its command and message | **survives** | (TF3) |
| V17 | `Vanished` without the command's message | killed | two adapter tests |
| V18 | `NoPool` without the command's words | killed | `no_such_pool_is_a_missing_pool…` |
| V19 | only the pool itself matched | killed | the engine test and others |
| T5a | the counter filter widened to `*_count` | killed | `a_persons_properties_named_like_the_counters_are_kept` |
| T5b | widened to names that contain a counter | killed | the same |
| T5c | widened to `*:snapshot_count` | killed | the same |
| N7 | the values pass as lossy text | killed | `the_manifests_read_as_recorded` |
| N7b | every value read alone as lossy text | **survives** | (explained above) |
| S1V1 | S1 under V1 | killed | S1 and six others |

Checks, not plants (each expected to pass):

| Id | Check | Result |
|---|---|---|
| N7c | `odd-raw` is never read alone | passes |
| F1 | the engine test with `-r`'s real answer | passes |
| F2 | the adapter test with `-r`'s real answer | passes (rerun) |
| S1 | the rig's shape in one process (`scratch160.rs.txt`) | passes (rerun) |

## Gates (rustc and clippy 1.100.0, current stable)

The gates ran on a scratch worktree at the head with its own target
directory.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| the same for `-p skyzfsd --features test-hooks` | clean |
| the same for `-p skyzfs-ublk-sys --features control-test-program` | clean |
| the same for `-p skyzfs-agent-cli --features test-hooks` | clean |
| `cargo test --workspace` | 1955 passed, 0 failed, 16 ignored |
| `cargo test -p skyzfsd --features test-hooks` | 200 passed |
| `cargo test -p skyzfs-ublk-sys --features control-test-program` | 37 passed, 11 ignored |
| `cargo test -p skyzfs-agent-cli --features test-hooks` | 7 passed |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps`: workspace, `skyzfsd/test-hooks`, `skyzfs-agent-cli/test-hooks`, `skyzfs-ublk-sys` with `control-test-program` | clean, all four |

## CI at 9354462 (read at 10:14 UTC, every run ended)

| Run | Workflow | Result |
|---|---|---|
| 45792585803 | skyzfs-ci (push): gate, lint, unit-arm64 | success |
| 45792609886 | ci: test, lint, unit-arm64, chart-lint, s3-backends | success |
| 45792609751 | coverage | success |
| 45792609794 | e2e | success |
| 45792609918 | k3s-smoke | success |
| 45792609629 | agent rig: a1, a2, d, k (26.04) | success; `destroy` ends 3 |
| 45792609685 | publish rig (26.04) | success |
| 45792609625 | cut rig through skyzfsd: 26.04, 26.04-arm, upload | success |
| 45792609774 | replay rig: through skyzfsd and write into a hole, 24.04 and 26.04 | success (reference jobs skipped, as the paths say) |
| 45792609775 | daemon rig: 24.04, 26.04, 26.04-arm | success; on each image 55 PASS rows, five controls FAIL as they must, two NA (a control and a pin the in-place store cannot take), the held-request pin WAITING |

- **The agent rig at this head** uses `zfs-topology`'s rig, which does
  not yet have testing's pin on `destroy`. The case itself shows the
  exit, though: `publish.rc` 3.
- **The daemon rig and the replay rig's daemon jobs** run here only
  because of `kill.rs`'s docs.
- **Codex** reviewed 9354462 with one finding, the P2 above (NAMES).

## Files

- `review.md`: this note.
- `plants160.py`: the plant runner.
  Usage: `python3 -I plants160.py WORKTREE scratch160.rs.txt [IDS]`.
- `plants160.out`: one JSON line per plant, then the rerun.
- `scratch160.rs.txt`: scratch test S1, appended to `engine/tests.rs`
  for one run and then removed.

# Review of skyzfs/skyzfs#161 (M2.6c-5c) at 3ea9be9

Head checked with `gh api repos/skyzfs/skyzfs/pulls/161 --jq .head.sha`:
`3ea9be923a5916652c6e6aa5d0714dbe2fc0427a`, branch `skyzfs-coding`, base
`zfs-topology` at 3818056 (the merge base, and the base's tip when I
read it). Two commits, `crates/skyzfs-agent` only:

- `b7c5256`: NAMES, the command's own pool passed to `confirm`;
- `3ea9be9`: R1-reach (`walk_limit`), the fidelity items and the nits
  of my review of #160.

Before the description I read the diff, design §5.2, the module docs
and every caller in `cli.rs` and `engine.rs`, the implementation notes
on #155 and #160, and OpenZFS 2.4.1's `zfs holds`, `zfs list` and
`zcp.c`.

## Verdict

**Nothing blocks the merge.**

- **NAMES is fixed.** Every caller passes the pool its command acts
  on, and `confirm` compares it with each line of `zpool list`
  exactly. The old rule (N1) and every prefix rule (N2, N3) are
  killed.
- **R1-reach is built as the ruling says.** The arithmetic saturates
  and the 35-minute cap holds (W1 to W16, apart from W11 and W12).
- **The `Vanished` message is pinned whole.** M1 to M4 are killed.
- **My plants.** 46 in all, each applied alone and restored, with a
  600 s limit. 35 are killed. The 11 that survive are low:
  - W11 and W12: the default of `holds_per_snapshot` is not pinned
    (TG1);
  - N14 to N19, N21 and N22: callers whose pool reaches no test (TG2);
  - N9: no change a caller can see.
- **The author's 14 plants.** I ran my own version of each. All 14 are
  killed. Two are killed by other tests than the description says
  (nit D).
- **The gates are clean.** CI ended green on every run at this head,
  the agent rig included. Its `destroy` case ends 3.

One finding goes to the lead (S1): `tagged.lua`'s new limit lets the
sweep stall the pool's writes for longer. Then a question of reach for
testing (R-COUNT), two test gaps and nits.

## Findings, most severe first

### S1 (medium, not blocking, for the lead): a longer `tagged.lua` can stall the pool's writes for longer

- **Where.** `crates/skyzfs-agent/src/zfs/cli.rs:1088-1091`.
  `tagged.lua` now runs within `walk_limit(full_per_dataset)`: a
  read's limit plus 200 ms for each snapshot counted, at most 35
  minutes. Before, it ran within a read's limit: 60 s plus 100 ms a
  dataset, at most 20 minutes.
- **Why that is more than a time limit.** `tagged.lua` is a read-only
  channel program (`zfs program -n`).
  - Such a program runs in open context. It holds the pool
    (`dsl_pool_hold`) for its whole run (`zcp.c:1014`, OpenZFS 2.4.1).
  - #155 measured another read-only program, `members.lua` in full
    mode, at 1001 datasets (run 45729174446; implementation notes,
    "The full read stalls the pool's buffered writes"). It kept the
    pool's group sync from completing for its whole 31 s run. The writer
    of new data made no write in that time.
  - That is why the manifest's read moved from a program to `zfs list`
    and `zfs get`.
  - The limit is what ends such a stall. The `Host` kills the command
    at its limit, and an open-context program stops on a signal
    (`zcp.c:784`, `!ri->zri_sync && issig()`).
  - `zfs holds -r` and the list are commands, not programs. They do not
    stall the sync: 707 groups were written during the get, in run
    45733395008.
- **Scenario.** The pool has 11 datasets and 500 000 snapshots. The
  sweep runs once in each agent process, so once in each `publish`.
  - **At 0.37 ms a snapshot** (testing's recorder, run 45781769557, no
    write load, one run), `tagged.lua` takes about 185 s.
  - **The base** killed it at 61.1 s. The sweep failed, and every run
    ended with exit 1.
  - **At this head** it runs to its end. If it stalls the sync as
    `members.lua` did, buffered writes stop for about 3 minutes in each
    `publish`.
  - **The cap is 35 minutes,** at about 5.7 million snapshots. In
    practice the long limit holds only while the snapshot count
    succeeds within a read's limit (R-COUNT). Past that, `tagged.lua`
    is back on a read's limit. So the count's reach bounds the stall.
    If the count costs 0.1 ms a snapshot (not measured), that is about
    225 s on 11 datasets and about 10 minutes on 1001.
  - Where `tagged.lua` ended within the old limit, nothing changes: the
    stall was its run time before as well (about 3.7 s at 10 000
    snapshots, 37 s at 100 000). With 11 datasets the change starts at
    about 165 000 snapshots, and with 1001 datasets at about 430 000.
- **Design.** §5.2 says "No freeze, no pause. Applications keep writing
  throughout." The pools this concerns are beyond the design's scale
  ("tens to hundreds of datasets"). Testing's space figures also show
  that many snapshots per dataset are costly.
- **Certainty.**
  - **Certain from the code and the source:** the program, its new
    limit, and that a kill cancels it.
  - **Inferred:** that `tagged.lua` stalls the sync. #155 measured
    `members.lua` in full, which runs through the same open-context
    path. Nobody has measured `tagged.lua` with the stall probe, and
    the mechanism was never checked in the source (the notes say so).
- **What the lead decides.** The ruling (b) trades an exit 1 for a
  stall in this regime. I suggested it on #160 myself, without weighing
  #155. Three ways:
  1. Keep (b), and say in the module docs that the sweep stalls the
     pool's buffered writes for `tagged.lua`'s whole run, up to 35
     minutes.
  2. Keep `tagged.lua` on a read's limit: the sweep fails rather than
     stalls. `holds -r` keeps its new limit, since it does not hold the
     sync.
  3. Move the sweep off the program. One way:
     - `zfs list -H -p -r -t snapshot -o name,userrefs POOL`, a command
       that walks the snapshots without holding the sync;
     - then `zfs holds -H` of the snapshots with a hold.

     That is a package of its own.

  Testing could run the stall probe beside `tagged.lua` at R1's
  scales, with its `holds -r` recording.

### R-COUNT (low, for the lead and testing): the round's reach now rests on the snapshot count, which is not measured

- **The dependency.** `walk_limit` grows only where the snapshot count
  succeeded (`cli.rs:472-481`). That count is `zfs list -r -H -o name
  -t snapshot POOL`, and it runs within a read's limit (`cli.rs:918-
  925`). That is 61.1 s on 11 datasets, and it does not grow with the
  snapshots.
- **What happens past it.** Past the count's own reach, `snapshots` is
  `None`. Both walks then fall back to a read's limit and fail as they
  did before this PR.
- **What the description claims.** "35 minutes covers about 5.7
  million snapshots for `tagged.lua`". That is the cap's reach, not
  the round's.
- **How cheap the count is.** `-o name` alone is simple mode
  (`zfs_main.c:4044-4046`). In simple mode the kernel skips the
  dataset's hold and its statistics for each snapshot. So it costs
  less per snapshot than `holds -r` or the list, which build full
  handles. How much less is not known:
  - at 0.1 ms a snapshot, the round reaches about 600 000 snapshots on
    11 datasets;
  - at 0.37 ms, it reaches about 165 000, which is where the base
    already failed.
- **The ask.** Testing's `holds -r` recording could also time the
  snapshot count at R1's scales. The module docs could then say up to
  what count a round works.
- **Certainty.** Certain from the code. The cost is inferred from the
  source.

### Test gaps (low)

- **TG1: the default of `holds_per_snapshot` is not pinned.** W11 (20
  ms) and W12 (zero) survive.
  - **What W12 does.** It takes the snapshots out of `holds -r`'s limit
    in production, which undoes half of R1-reach, and no test fails.
  - **Why the tests miss it.**
    - `the_limits_grow_with_the_datasets_counted` pins every other
      field of `Limits::default()` (`zfs/cli/tests.rs:1640-1647`).
    - `the_lists_limit_grows_with_the_snapshots_counted` computes its
      expected value from the default itself (`:1836`).
    - The new test sets 30 ms (`:1866`).
  - **Fix.** Add `holds_per_snapshot` to the asserted defaults.
- **TG2: the pool given to `confirm` is pinned at some callers only.**
  - **What the code does.** By reading, every caller passes the pool its
    command acts on. I checked every call site of `failed`, `confirm`,
    `run`, `zfs`, `zpool` and `zfs_bytes`. Certain.
  - **What the tests pin.** `props`, the programs, `holds -r`, a value
    read alone and the manifest's reads (N7, N8, N11, N12, N20 killed).
  - **What survives a wrong pool:** `zfs hold`, `release`, `destroy`,
    the take, the count, `zpool status` in `layout` and in `props`'
    fallback, and `assist`'s `zpool sync` (N14 to N19, N21, N22).
  - **Which of these matter.** Only the `zpool` ones are recorded
    saying `no such pool` of a missing pool, and so reach `confirm`.
    For `layout` (the premise) and `assist`, a pool named `status` or
    `sync` is the NAMES case.
  - **Fix.** A `layout` case with `status` listed in
    `only_the_commands_own_pool_is_looked_for` would pin the one the
    premise depends on.
- **N9 survives, with no change a caller can see.** It makes every read
  through `zfs_bytes` look for `cannot open 'POOL'`, a value read alone
  included.
  - That changes only a value read alone of the root dataset whose pool
    went between the gets and the value.
  - There it gives `NoPool` or `Vanished` instead of `Other`.
  - `datasets()` reports any error as `Unreadable` (`engine.rs:1125-
    1130`, `:1181-1186`), so the engine acts the same way.
  - Nothing to do.

### Nits

- **A: stale words.** Each of these still describes the rule before
  this PR:
  - `zfs/cli.rs:918` ("for the list's limit alone") and
    `zfs.rs:386-388` ("for the manifest's list alone"). The snapshot
    count now sets the walks' limits too.
  - `zfs.rs:343-344`: `NoPool` "where `zpool list` does not list a pool
    they name". It is now the pool the command acts on.
  - `zfs/cli/tests.rs:1787-1788`: "and every other command keep the
    datasets' scale". `holds -r` no longer does.
  - `zfs/cli/tests.rs:2010-2015`: "lists no pool the command names".
  - `engine.rs:638-640`: the sweep's read "grows with the datasets",
    and now with the snapshots as well.
- **B: the reason given for the programs' retry is wrong.**
  - **Where:** `zfs/cli.rs:606-607` and `zfs/cli/tests.rs:1915` say "a
    program runs in one sync task".
  - **What happens instead.** The agent's programs run with `-n`, in
    open context (`zcp.c:1014`), not in a sync task.
  - **Why the conclusion holds anyway.** The program holds the pool's
    config lock as a reader for its whole run. A destroy is a sync task,
    which takes that lock as writer (`dsl_sync_task_sync` in
    `dsl_synctask.c`). So a destroy cannot land inside a program.
- **C: one cost is stated without a measurement.**
  - **Where:** `zfs/cli.rs:274-278` gives "about 0.4 ms a snapshot each"
    for the list, `holds -r` and `tagged.lua`.
  - **What is measured.** The list (0.40 ms) and `tagged.lua` (0.37 ms)
    were measured. `holds -r` and the snapshot count are not.
- **D (description): two plants are attributed to the wrong test.**
  - **W3** (`holds -r` looked for by its snapshot's name). It is killed,
    but not by `only_the_commands_own_pool_is_looked_for`, whose `held`
    case lists neither `tank` nor `tank@skz-1`, so both readings give
    `NoPool`. My N8, the same plant, is killed by three other tests:
    - `a_read_whose_snapshot_vanished_is_read_again`;
    - `a_read_that_goes_on_vanishing_is_an_error_that_names_it`;
    - the engine's `a_destroy_between_two_reads_is_no_missing_pool`.
  - **W6** (a listed line that begins with the pool). It is killed only
    by `no_such_pool_is_a_missing_pool_only_where_zpool_list_agrees`,
    through `tank2` (my N2). The new test lists no line that begins
    with `tank`.
- **E: the cap's floor depends on the defaults.** `walk_limit` never
  undercuts a read's limit only because `most` (20 min) is below
  `full_most` (35 min).
  - `Limits`' fields are public, but production builds only
    `Limits::default()` (`bins/skyzfs-agent/src/main.rs:132`).
  - A `.max(self.read_limit())`, or a line in the docs, would make it
    hold for any `Limits`. Very low.
- **F: the second answer of `holds -r` is not recorded yet.** It is now
  exit 0, no line and `no datasets available`. That is right by the
  source (`zfs_main.c:7036-7048`, 2.4.1), but not recorded yet. #160's
  TF1 ask to testing stands.

## The questions the brief asked

### 1. `walk_limit`

- **The arithmetic.** It is `grown(read_limit(), per, full_most, n)`
  (`cli.rs:472-481`, `:391-397`):
  - `n` above `u32::MAX`, or an overflowing `per × n`, gives `most`
    (here `full_most`);
  - the sum saturates;
  - `.min(full_most)` caps it.

  So it cannot exceed 35 minutes (W7 is killed). With the defaults it
  never goes below a read's limit, which is at most 20 minutes (nit E).
  The new test runs 1 000 000 snapshots.
- **Every walking command uses its limit.** The commands that walk
  every snapshot are these four (OpenZFS 2.4.1):
  - `zfs holds -r`: `ZFS_ITER_RECURSE` with snapshots,
    `zfs_main.c:6998-7036`, filtered by name only in `holds_callback`;
  - `tagged.lua`: `zfs.list.snapshots` of every dataset;
  - the manifest's list, which has its own `list_limit`;
  - the snapshot count itself, on a read's limit by necessity
    (R-COUNT).

  `holds -r` has one call site, `tagged_snapshots`, used by `held`,
  `hold` and `release`. `tagged` is the other.

  These do not walk the snapshots:
  - `members.lua` (`zfs.exists` lookups);
  - `zfs destroy -r`, `hold -r` and `release -r`, which look up each
    dataset's `@t` by name, and `snapshot -r`, which goes by dataset.
    In testing's recorder, `hold -r` and `release -r` stayed at 0.12 s
    at 101 000 snapshots.
- **The source of the snapshot count.**
  - **What it counts.** `zfs list -r -H -o name -t snapshot POOL`
    counts every snapshot of every file system and volume in the pool.
    That is the set both walks visit.
  - **When it runs.** At the start of each round and before `begin`'s
    sweep (`engine.rs:643`, `:691`). Every call of `tagged`, `held`,
    `hold` and `release` comes after it in the same process
    (`engine.rs:779`, `:788`, `:836`, `:949`, `:1033`, `:1505`).
  - **What it misses.** The take's own `@t` (one per dataset) come
    after the count. The 200 ms allowance, about 500 times the
    measured cost, covers them.
- **A limit too small or too large.**
  - **Too small.** The command is killed and gives `Other` (module
    docs). From `held`, that error ends `settle` with exit 1
    (`engine.rs:836`). From the sweep's `tagged`, the same
    (`engine.rs:779`). The next run tries again. This is as the module
    docs say. The design itself says nothing about command limits.
  - **Too large.** It costs only time, with one exception, S1. With the
    defaults the cap is reached at about 10 000 snapshots, so past that
    every walk gets 35 minutes.
  - **It limits itself.** On a pool whose commands hang (a suspended
    pool), the count also fails or times out first. The scale then
    falls to the bases or to `snapshots: None`, and the walks are back
    on a read's limit.

### 2. NAMES

- **The fix.** `confirm` compares the given pool with each line of
  `zpool list -H -o name`, exactly (`cli.rs:628-635`):
  - a pool that begins another's is not it (`tank` and `tank2`, either
    way: N2 and N3 killed);
  - a pool named like a word of the command is not it (`get`, `status`,
    `holds`, `skz-1`, the tag);
  - names with spaces, which ZFS allows, compare whole.
- **`pool_of`** is the name up to its first `/` or `@`
  (`cli.rs:1135-1137`). It is used at two sites only:
  - a value read alone, whose name comes from `zfs get -r … POOL`;
  - `holds -r`'s `POOL@t`.

  Both give the agent's pool exactly. Neither ever sees a flag, a path,
  an `-o` value or a bookmark. A bookmark's `#` is not split, but no
  caller passes one, and a pool's name cannot hold `#`. Every other
  call site passes `pool`, the engine's own.
- **No caller can pass the wrong pool.** That is certain by reading,
  and every caller passes it explicitly. The tests pin some callers
  only (TG2).

### 3. The `Vanished` message

- **What it pins.** `a_read_that_goes_on_vanishing_is_an_error_that_names_it`
  compares the whole `Display`:
  - the command's words;
  - its trimmed message;
  - the closing words "(`zpool list` lists its pool)".
- **The plants.** M1 (no reason), M2 (a fixed text), M3 (no command)
  and M4 (untrimmed) are all killed. #160's V16 is closed.
- **What the operator reads.** The engine prefixes `zfs: `
  (`engine.rs:333`). After three reads, the message is the last read's.

### 4. Plants

`plants161.py` (this folder) applies each plant alone to a scratch
worktree at the head, as exact text replacements. It then runs `cargo
test -p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks -- --test-threads=4` with a 600 s limit,
and restores the file in a `finally`. The worktree was clean after
both runs. The raw lines are in `plants161.out`. Without a plant, the
same command at the head passed: 201 and 7 tests.

| Id | Plant | Result | Killed by |
|---|---|---|---|
| W1 | `holds -r` on a read's limit (the base) | killed | both snapshot-limit tests |
| W2 | `tagged.lua` on a read's limit (the base) | killed | `the_holds_and_the_tags_grow…` |
| W3 | `holds -r` on `full_per_dataset` | killed | the same |
| W4 | `tagged.lua` on `holds_per_snapshot` | killed | the same |
| W5 | the walk capped at `most` | killed | both snapshot-limit tests |
| W6 | the walk's base `l.read`, not scaled | killed | three limit tests |
| W7 | the walk not capped | killed | both snapshot-limit tests |
| W8 | a failed snapshot count gives the gets' limit | killed | `the_holds_and_the_tags_grow…` |
| W9 | the walk counts datasets and snapshots | killed | three limit tests |
| W10 | no count yet (`Base`): the walk on `full_most` | killed | `the_limits_grow_with_the_datasets_counted` |
| W11 | default `holds_per_snapshot` 20 ms | **survives** | (TG1) |
| W12 | default `holds_per_snapshot` zero | **survives** | (TG1) |
| W13 | the walk's base the change limit | killed | three limit tests |
| W14 | the walk's base the gets' limit | killed | three limit tests |
| W15 | the per-snapshot term halved | killed | both snapshot-limit tests |
| W16 | the snapshot count never kept | killed | both snapshot-limit tests |
| N1 | `confirm`: the pool or any word (the old rule, widened) | killed | `only_the_commands_own_pool_is_looked_for` |
| N2 | `confirm`: a line that begins with the pool | killed | `no_such_pool_is_a_missing_pool…` |
| N3 | `confirm`: a line the pool begins with | killed | the same |
| N4 | `pool_of` cuts at `/` only | killed | four tests, the engine's among them |
| N5 | `pool_of` cuts at `@` only | killed | two tests |
| N6 | `pool_of` is the name | killed | five tests |
| N7 | a value read alone passes its dataset's name | killed | `only_the_commands_own_pool…` |
| N8 | `holds -r` passes its snapshot | killed | the two vanish tests and the engine's (nit D) |
| N9 | `zfs_bytes`: every read names the pool | **survives** | (no change a caller can see) |
| N10 | `zfs_bytes`: no read names the pool | killed | `datasets_that_cannot_be_read_are_an_error` |
| N11 | `props` passes `get` | killed | `only_the_commands_own_pool…` |
| N12 | a program passes its last argument | killed | three tests |
| N13 | `confirm` on the last word | killed | five tests |
| N14 | `hold` passes its snapshot | **survives** | (TG2) |
| N15 | `release` passes its snapshot | **survives** | (TG2) |
| N16 | `destroy` passes its snapshot | **survives** | (TG2) |
| N17 | the take passes its snapshot | **survives** | (TG2) |
| N18 | the count passes `list` | **survives** | (TG2; the engine ignores the count's error) |
| N19 | `layout`'s `zpool status` passes `status` | **survives** | (TG2) |
| N20 | the manifest's reads pass `list` to `confirm` | killed | `datasets_that_cannot_be_read_are_an_error` |
| N21 | `props`' fallback `zpool status` passes `status` | **survives** | (TG2) |
| N22 | `assist`'s `zpool sync` passes `sync` | **survives** | (TG2) |
| A4 | `pool_of` takes the last part (author's W4) | killed | five tests |
| A6 | the walk's base grown by `per_dataset` to `full_most` (author's R6) | killed | `the_limits_grow_with_the_datasets_counted` |
| A8a | `holds -r`'s snapshots at `per_dataset` (author's R8) | killed | both snapshot-limit tests |
| A8b | `tagged.lua`'s snapshots at `per_dataset` (author's R8) | killed | `the_holds_and_the_tags_grow…` |
| M1 | `Vanished` without its reason | killed | `a_read_that_goes_on_vanishing…` |
| M2 | `Vanished` as a fixed text | killed | three tests |
| M3 | `Vanished` without the command | killed | three tests |
| M4 | `Vanished` untrimmed | killed | `a_read_that_goes_on_vanishing…` |

**The author's 14, in my versions:**

| Author's plant | My version | Result |
|---|---|---|
| W1 | N1 | killed |
| W2 | N7 | killed |
| W3 | N8 | killed, by other tests (nit D) |
| W4 | A4 | killed |
| W5 | N4 | killed |
| W6 | N2 | killed, by another test (nit D) |
| R1 | W1 | killed |
| R2 | W2 | killed |
| R3 | W3 | killed |
| R4 | W4 | killed |
| R5 | W5 | killed |
| R6 | A6 | killed |
| R7 | W8 | killed |
| R8 | A8a and A8b | killed |

**What #160's NAMES finding asked:** N1 shows that
`only_the_commands_own_pool_is_looked_for` fails on the old rule.

## Gates (rustc and clippy 1.100.0, current stable after `rustup update stable`)

These ran on a scratch worktree at the head.

The workspace tests and the docs ran with `CARGO_INCREMENTAL=0` and
no debug info (`CARGO_PROFILE_DEV_DEBUG=0`). The container's disk could
not hold a build with debug info. The code and the tests were the same.

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| the same with `--features skyzfsd/test-hooks` | clean |
| the same with `--features skyzfs-ublk-sys/control-test-program` | clean |
| the same with `--features skyzfs-agent-cli/test-hooks` | clean |
| `cargo test --workspace` | 1957 passed, 0 failed, 16 ignored |
| `cargo test -p skyzfsd --features test-hooks` | 200 passed |
| `cargo test -p skyzfs-ublk-sys --features control-test-program` | 37 passed, 11 ignored |
| `cargo test -p skyzfs-agent -p skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks` | 208 passed |
| `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps --workspace`: plain, and with each of the three features | clean, all four |

## CI at 3ea9be9 (every run ended)

| Run | Workflow | Result |
|---|---|---|
| 45806588699 | skyzfs-ci (push): gate | success (lint, test and unit-arm64 skipped there, run by `ci`) |
| 45806599155 | ci: test, lint, unit-arm64, chart-lint, s3-backends | success |
| 45806599207 | coverage | success |
| 45806599357 | e2e | success |
| 45806599219 | k3s-smoke | success |
| 45806599223 | agent rig: a1, a2, d, k (26.04) | success; `destroy` ends 3 (`cases.tsv` in the k artifact) |

- **Rigs not run:** the daemon, replay, control and cut rigs do not run
  here. Their paths are not touched; only `crates/skyzfs-agent`
  changed.
- **Codex** reviewed 3ea9be9 and found nothing.
- **#160's thread:** the author's answer to Codex's P2 is posted there.

## Files

- `review.md`: this note.
- `plants161.py`: the plant runner.
  Usage: `python3 -I plants161.py WORKTREE [IDS]`.
- `plants161.out`: one JSON line per plant, from both runs.

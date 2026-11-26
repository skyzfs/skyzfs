# Review of skyzfs/skyzfs#162 (M2.6c-5d) at 6e420dc

Head checked with `gh api repos/skyzfs/skyzfs/pulls/162 --jq .head.sha`:
`6e420dcc5b72b948b22d2979074ee59e938c0752`, branch `skyzfs-coding`, base
`zfs-topology` at 1d14ee9 (the merge base, and the base's tip when I
read it). Two commits, `crates/skyzfs-agent` and `bins/skyzfs-agent`
only:

- `16be309`: the sweep reads holds with `zfs list` and `zfs holds -H`;
  `tagged.lua` deleted; 10 ms a snapshot (`holds_per_snapshot`, new
  `list_per_snapshot`); TG1, TG2, nits A to F of my review of #161;
- `6e420dc`: a comment on the holds line the sweep skips.

Before the description I read the diff, design §5.2 and §7, the module
docs and every caller of the changed functions in `cli.rs` and
`engine.rs`, the implementation notes on #155, #159 and #161, the
recorder's records of run 45800940382 (both images, downloaded), and
OpenZFS's `zfs_do_holds` (2.2.2 and 2.4.1, the same but for a printf
cast), `zfs_for_each`, `zfs_open`, `zfs_do_list_ioctl`,
`zfs_standard_error_fmt`, `zfs_get_holds`, `zfs_ioc_snapshot_list_next`
and `dsl_userhold.c` (2.4.1).

## Verdict

**Nothing blocks the merge.** One defect should be fixed in this pull
request, since it is one line in the code under review (D1); one
question goes to the lead with figures (R-COUNT); the rest is low.

- **S1 is resolved.** The sweep runs no channel program. `zfs list`
  and `zfs holds` hold the pool only for each ioctl, so the sync is
  not held up for the read's run. The old semantics are kept: every
  snapshot of every dataset, the exact tag, the name after `@`, a set.
- **The partial `zfs holds -H` fake is faithful to the source** of
  2.2.2 and 2.4.1, and the code does not depend on its stdout anyway.
- **The limits are as ruled**, saturate, and are capped. Every command
  that walks the snapshots has its walk limit except the snapshot count
  itself, which is R-COUNT.
- **TG1 and TG2 are answered.** PLANTS-SUMMARY
- **The stand-in `zfs` of `program.rs` still drives the sweep's kill
  point honestly**, through both new commands. AGENT-RIG
- **Gates.** GATES-SUMMARY

## Findings, most severe first

### D1 (defect, medium-low; fix in this PR): another holder's tag that is not UTF-8 stops every agent process

- **Where.** `crates/skyzfs-agent/src/zfs/cli.rs:834`. Each of the
  sweep's `zfs holds -H` goes through `self.zfs`, that is `run`, which
  fails the whole output where it is not UTF-8 (`cli.rs:606-612`).
- **Why it can happen.** A hold's tag may hold any byte but NUL. The
  kernel checks only its length (`dsl_userhold.c:61-65`, 2.4.1), and
  `zfs hold` only refuses a leading `.` (`zfs_main.c:6775`).
  `print_holds` writes the tag with `%s` (2.2.2 and 2.4.1).
- **Scenario.** Someone runs `zfs hold $'caf\xe9' tank/db@backup`: a
  Latin-1 tag, by root or by a user given `hold` with `zfs allow`, on
  any snapshot of the pool.
  1. The sweep's list gives `tank/db@backup` with `userrefs` 1.
  2. Its `zfs holds -H` prints the byte 0xe9. The read is
     `Other("zfs holds -H ...: its output is not UTF-8")`.
  3. `sweep()` returns it (`engine.rs:779`). `begin` (`engine.rs:644`)
     and every `settle` round (`engine.rs:692`) end with it.
  4. No `publish` and no `settle` gets past its start until that tag
     is released.
- **Before.** `tagged.lua` compared the tags in the kernel and
  returned only snapshot names, so no other holder's tag reached the
  agent. `held()`'s `zfs holds -H -r pool@t` has been as strict since
  #147, but it reads only the holds of the agent's own `@t`. The sweep
  now reads the holds of every held snapshot in the pool.
- **Certainty.** Certain from the code, and shown twice at the head.
  - **A scratch unit test,** not committed. The list gives `tank@a`
    and `tank/db@backup`, and `zfs holds -H` gives the agent's hold on
    `tank@a` and `caf\xe9` on the other. `tagged()` returned
    `Err(Other("zfs holds -H tank@a tank/db@backup: its output is not
    UTF-8"))`. The right answer is `{"a"}`.
  - **The program.** `skyzfs-agent settle` (plain build) ran on a
    stand-in `zpool` and `zfs`, as in `program.rs`, in a marked
    directory, with one such tag. It printed `skyzfs-agent: zfs: zfs
    holds -H tank/db@backup: its output is not UTF-8` and exited 1.
- **Fix.** Read the holds as bytes (`zfs_bytes` and
  `String::from_utf8_lossy`, as the list does). The agent's tag and
  the snapshots' names are ASCII, so a replaced byte cannot make
  another tag read as the agent's. A test with a byte above 0x7f in
  another holder's tag.

### R-COUNT (medium, for the lead; the description raises it too): on Ubuntu 24.04 the snapshot count ends the round's reach at about 50 000 snapshots

- **Where.** `cli.rs:1040-1044`. The snapshot count runs within a
  read's limit on the datasets counted, 60 s + 100 ms a dataset. It
  does not grow with the snapshots, which it is there to count.
- **The figures**, from the records of run 45800940382 (one run on
  each image, idle pool; I read the `secs` files):

  | Image, pool | snapshot count | its limit | `holds -r` | manifest's list |
  |---|---|---|---|---|
  | 24.04, 10:1000 (11 000) | 13.3 s | 61.1 s | 18.9 s | 20.3 s |
  | 24.04, 100:1000 (101 000) | **126.2 s** | **70.1 s** | 174.2 s | 185.9 s |
  | 26.04, 10:1000 (11 000) | 2.7 s | 61.1 s | 3.9 s | 4.3 s |
  | 26.04, 10:10000 (60 500) | 14.8 s | 61.1 s | 21.9 s | 24.2 s |
  | 26.04, 100:1000 (101 000) | 25.7 s | 70.1 s | 37.3 s | 41.6 s |

- **What follows.** At 1.25 ms a snapshot, the count fails past about
  48 000 + 80 × D snapshots on 24.04: 48 900 with 11 datasets, 56 100
  with 101. Past that, `snapshots` is `None`, and every walk is back
  on a read's limit or the gets' limit (61 to 80 s). Each walk then
  takes longer than that: `holds -r` passes 61.1 s at about 35 500
  snapshots. So on 24.04 every round fails from about 50 000 snapshots.
  On 26.04 (0.25 ms) the same happens at about 240 000 to 280 000.
- **Against the description.** "At 10 ms, the 35-minute cap is
  reached at about 200 000 snapshots" is the cap's reach. The round's
  reach on 24.04 is about a quarter of that.
- **The new test claims more than holds.**
  `the_holds_limit_covers_its_recording` (`cli/tests.rs:2105`) checks
  the 24.04 row (101, 101 000) with a count the fake answers at once.
  The same recording's count at that scale took 126.2 s against its
  70.1 s limit. So on that pool `holds -r` never gets the limit the
  test checks. The test could also hold the count's limit against the
  count recorded at each row. It fails today, and that is the finding.
- **Not a regression.** The base has the same count and the same
  fallbacks.
- **Options for the lead.**
  1. Give the snapshot count `full_most`. It is `-o name`, simple mode,
     and does not hold the sync. N2's case (a pool that holds every
     command) mostly ends at the dataset count's 60 s first, since the
     snapshot count runs only after the dataset count answered.
  2. Where the snapshot count is killed at its limit after the dataset
     count answered, take the walks to their cap: a pool that answers
     but cannot list its snapshots in a minute has many.
  3. Grow the count's limit from the last count. A fresh process on a
     large 24.04 pool would still fail its first count.

  My preference is 1: one line, plus the test's extra check.
- **Certainty.** Certain from the code and the recorded seconds. One
  run per image, idle pool.

### F1 (low): another holder's tag that holds a newline can add a line to the sweep's read

- **Where.** `cli.rs:844-858`. Each line of `zfs holds -H` is split at
  tabs. A line is kept where its second field is the agent's tag,
  whatever its first field.
- **Scenario.** A tag `x\ntank@zz\tskyzfs-publish` on any held
  snapshot prints a line `tank@zz<TAB>skyzfs-publish<TAB>...`. The
  sweep then reads `zz` as held by the agent.
- **Effects, by reading the code.**
  - In a marked directory, the agent asks `zfs release -r
    skyzfs-publish tank@zz`. That is `NoSnapshot`, or `no such tag`
    and then an empty `holds -r`: both `Ok` (`cli.rs:1149-1160`). A
    point `sweep before zz` is traced.
  - In a directory with no marker, exit 6 naming `zz`
    (`engine.rs:782-784`).
  - Nothing is released that the agent does not own.
- **Before.** `tagged.lua` matched tags exactly in the kernel and
  could not be forged.
- **Fix.** Keep a line only where its first field is one of the names
  that command asked for. One set lookup, and it narrows a forged line
  to a real snapshot. Lines split from a multi-line tag then drop out
  as well.
- **Already in the base, for later.** `tagged_snapshots`
  (`cli.rs:773-781`, `zfs holds -H -r pool@t`, since #147) parses the
  same way. A forged line in a tag on one member's `@t` could make
  `held()` read `Held::All` for a member that lacks the hold, or make
  `hold()` take a partial hold for whole. That needs a holder with
  `hold` on the agent's own snapshots. Low, not this PR's.
- **Certainty.** Certain from the code and the source. A scratch unit
  test shows the read: holds output `tank@a<TAB>x` then
  `tank@zz<TAB>skyzfs-publish<TAB>...` gave `Ok({"zz"})`. The
  engine's effects above are by reading only.

### TG-A (low): the sweep's `zfs holds -H` in libzfs's second shape of a vanished snapshot is not tested

- **What is tested.** A snapshot gone before its `zfs holds`: `cannot
  open 'X': dataset does not exist` (`zfs_open`). That is
  `NoSnapshot`, then `Vanished`, then the read is made again.
- **What is not.** A snapshot destroyed between the command's
  `zfs_open` and its `zfs_get_holds`. That says `cannot get holds for
  'X': no such pool or dataset` (`libzfs_dataset.c:5355-5356`, 2.4.1),
  the shape the agent rig recorded for `holds -r`. It reaches
  `confirm` with the pool passed at `cli.rs:834`. By reading, that pool
  is right and the read is made again.
- **The plants.** Each of these survives:
  - R1: that command's pool to `confirm` replaced by its first
    snapshot's name, which turns the shape into `NoPool`;
  - R2: `Vanished` from it taken as no hold, which skips the chunk;
  - R3: `Vanished` from it made an error at once.

  R2 is the one that matters: a chunk skipped is a stale hold kept
  until the next process.
- **Fix.** One more case in
  `the_sweep_reads_again_what_vanished_under_it` with that message and
  a `zpool list` that lists `tank`, and one in
  `only_the_commands_own_pool_is_looked_for` with the snapshot's name
  listed.

### TG-B (low): three more survivors

- **R4: a tag that the agent's begins with, kept as the agent's**
  (`tag.starts_with(held_by)`, so `skyzfs` or an empty field). It
  survives. The test has `skyzfs-publish-not` (S3) but no tag that is
  a prefix of the agent's. Add a hold `skyzfs` to
  `the_sweep_reads_the_holds_of_the_snapshots_that_have_one`.
- **R5: a holds line with fewer than two fields made an error instead
  of skipped.** It survives. `6e420dc` documents the skip as a choice,
  but no test pins it. With F1's fix such lines drop out anyway.
- **R7: only the second `zpool status` (`-g`) of `layout` confirmed on
  a pool named `status`.** It survives. The new case fails the first
  call, so the second is never reached. It matters only where the pool
  goes between the two calls. A case where the first call passes and
  the second says `no such pool` would pin it.

### G-REC (low, for testing; Codex's P2 on the PR): the recorder needs `tagged.lua`

- **Where.** `skyzfs-zfs-cli-record.yml` (`for f in members tagged`)
  and `tests/kernel/zfs-cli-record.sh` (`main()` exits 64 without
  `tagged.lua`).
- **What happens.** Both fail for any `lua_ref` after this merge. The
  default `lua_ref` (0f709ae) still works. The description names the
  workflow line but not the script's check, which Codex names.
- **Not this PR's files.** Testing's to change.
- **What to record with it.**
  - `zfs list -H -p -r -t snapshot -o name,userrefs` and `zfs holds
    -H` of several snapshots, one gone, on both images: that is the
    fake below.
  - The list with `userrefs` at R1's scales: its cost is inferred.
  - A tag with a byte above 0x7f, and one with a newline (D1, F1).
- **Who answers.** The author should still answer Codex's thread on
  the pull request.

### Nits

- **N-a: a dataset gone during the list is not an error.** The doc of
  `tagged_once` (`cli.rs:790-793`) says "a dataset gone during either
  is Vanished". The test feeds `cannot iterate filesystems: no such
  pool or dataset` to the list (`cli/tests.rs:1684-1693`). libzfs does
  not say that of a vanished dataset:
  - `zfs_do_list_ioctl` ends the walk on `ENOENT`
    (`libzfs_iter.c:85-96`);
  - `zfs_ioc_snapshot_list_next` turns `ENOENT` into `ESRCH`
    (`zfs_ioctl.c:2417`);
  - `ENOENT` through `zfs_standard_error` says `dataset does not
    exist`, not `no such pool or dataset` (`libzfs_util.c:455-458`).

  The list exits 0 without that dataset's snapshots, which is right
  for the sweep. Say so, and call the case defensive.
- **N-b: one figure is from another run.** `list_per_snapshot`'s doc
  (`cli.rs:381-384`) gives 26.04's list as "41.2 s ... run
  45800940382, on an idle pool". In that run it took 41.6 s. 41.2 s is
  run 45781769557's, beside the writers. The rate, 0.41 ms, is right
  either way.
- **N-c: the per-snapshot cost grows with the snapshots per dataset.**
  #159 measured on 26.04, under writers: 0.40 ms a line up to 5500 a
  dataset, and 1.06 ms at 12 500. The "about five times the slowest"
  is at 1000 a dataset on an idle pool. At 12 500 a dataset on 24.04
  the margin may be about two. The docs could say so, or testing
  records one 24.04 point at a high count per dataset.
- **N-d: argument length.** `HOLDS_AT_ONCE`'s doc (`cli.rs:314-317`)
  says "far below the kernel's limit". Linux's limit is a quarter of
  the stack's rlimit, so a unit with `LimitSTACK=1M` allows 256 KiB,
  which is 1000 names of the longest kind. Real names are far shorter.
  Say "with the default stack limit".
- **N-e: the skip's reason.** The comment at `cli.rs:846-849` says the
  skip "keeps the hold it might name". Only another holder's
  multi-line tag makes a line without two fields: the agent's own
  lines are always whole. So the skip never keeps an agent's hold,
  and that is the better reason to give.

## What I checked, by the four questions

### (1) `tagged_once` (`cli.rs:796-861`)

- **No stall.** `zfs list -r -t snapshot` takes the pool for one
  `SNAPSHOT_LIST_NEXT` ioctl at a time (`dmu_objset_hold` and its
  release, `zfs_ioctl.c:2415-2468`). `zfs holds` opens each snapshot
  and reads its holds with one `dsl_pool_hold` each
  (`dsl_userhold.c:665-670`). Neither is held for the command's run,
  unlike a channel program. #155/#156 measured the same kind of
  command not holding the sync (707 groups during the get at 10 000
  datasets). Nobody has run the stall probe beside this read, but the
  mechanism is the per-ioctl hold. S1 is resolved.
- **The same snapshots as `tagged.lua`.** `zfs list -r -t snapshot
  POOL` lists the snapshots of the pool and of every file system and
  volume under it, as the program's walk did. Hidden datasets are left
  out by both.
- **The same holds.** `userrefs` is `ds_userrefs`
  (`dsl_dataset.c:2854`), which every hold counts, temporary ones
  included (`dsl_userhold.c:176`). So a snapshot with `userrefs` 0 has
  no hold, and skipping it is exact.
- **The same tag match.** `held_by == tag`, exact: a tag that begins
  with the agent's, or that the agent's begins with, is not kept (S3
  and R4 below). The name kept is the part after the first `@`, and a
  snapshot's name has one `@`.
- **Order and duplicates.** The names asked follow the list's order.
  The result is a `BTreeSet` of `t`, so `@t` on several datasets is
  one entry, as the program's table was.
- **What is no longer one read.** `tagged.lua` read everything under
  one hold of the pool; this is two reads.
  - A hold placed after the list is missed. Only an agent places the
    agent's tag, and one agent holds the pool's lock.
  - A `zfs rename` or `zfs promote` during the list can hide a
    snapshot from one sweep. Its hold is then kept until the next
    process. That is conservative.
  - A snapshot gone between the reads is read again (below).
- **When something vanishes.** By reading and against the source:
  - **Snapshot gone before its `zfs holds`:** `cannot open 'X':
    dataset does not exist` (`zfs_open`, `libzfs_dataset.c:714-736`;
    `libzfs_util.c:455-458`). That is `NoSnapshot`, then `Vanished`,
    and the whole read again.
  - **Snapshot gone between open and read:** `cannot get holds for
    'X': no such pool or dataset`. That goes through `confirm`, with
    the pool listed, to `Vanished`. Not tested (TG-A).
  - **Pool exported between the reads:** the holds say `dataset does
    not exist` (`Vanished`); then the list again says `cannot open
    'tank': dataset does not exist`, which `confirm` makes `NoPool`.
    One extra read.
  - **Dataset gone during the list:** no error, its walk just ends
    (N-a).
- **The partial `zfs holds -H` fake** (`cli/tests.rs`, `gone`) is what
  the source does.
  - `zfs_do_holds` (2.2.2 and 2.4.1) runs `zfs_for_each` once per
    argument and gathers every snapshot it opened into one list.
  - An argument it cannot open sets `errors` and the loop goes on
    (`zfs_iter.c:516-527`).
  - It then prints the list it gathered, and says `no datasets
    available` only where that list is empty. It returns `errors`, so
    the exit code is 1.
  - Certainty: high from the source; not recorded.
  - The code takes nothing from the stdout of a command that did not
    exit 0, so the exact partial output does not matter. If a version
    exited 0 instead, the gone snapshot would just be missing, which
    is also right: it has no hold left to release.
- **Chunks.** `held.chunks(1000)` gives no command for 0 names, one
  for 1, 999 and 1000, and two (1000 and 1) for 1001, by `chunks`'
  definition. The test pins 2500 as 1000, 1000 and 500, and 0 names as
  no command. S7 (1001) is killed.
- **Argument length.** 1000 names at the longest are 256 KiB, against
  the default 2 MiB limit (N-d).
- **Odd names.** A name may hold spaces, `:`, `.`, `-` and `_`, but
  no tab or newline.
  - Names go to the command as separate arguments, with no shell.
  - `rsplit_once('\t')` on the list is exact.
  - A name always begins with the pool's, which begins with a letter,
    so none is read as an option.
- **Odd tags.** Another holder's tag can hold any byte but NUL. That
  is D1 (not UTF-8) and F1 (a newline).

### (2) The limits

- **The arithmetic.** In seconds, with D datasets and S snapshots
  counted:
  - the manifest's list (`list_limit`, `cli.rs:485-499`):
    `min(min(60 + 0.2·D, 2100) + 0.01·S, 2100)`;
  - `holds -r` and the sweep's list (`walk_limit`, `cli.rs:511-520`):
    `min(min(60 + 0.1·D, 1200) + 0.01·S, 2100)`;
  - each of the sweep's `zfs holds -H`: `min(60 + 0.1·D, 1200)`.
- **Saturation.** `grown` takes `most` where S does not fit in a
  `u32` or the product overflows, then adds with `saturating_add` and
  caps. The cap is reached at about 204 000 snapshots with 10
  datasets, as the tests pin (203 700 under it, 203 900 at it).
- **Every command that walks the snapshots** has a walk limit:
  - `holds -r` through `tagged_snapshots`, in `held`, `hold` and
    `release`;
  - the sweep's list;
  - the manifest's list.

  The gets, `hold -r`, `release -r`, `destroy -r` and `snapshot -r`
  walk the file systems only. The one walk without a walk limit is the
  snapshot count itself: R-COUNT.
- **The figures behind 10 ms**, per snapshot on an idle pool (run
  45800940382):

  | Image | `holds -r` | manifest's list | `tagged.lua` | count |
  |---|---|---|---|---|
  | 24.04 | 1.72 ms | 1.84 ms | 1.96 ms | 1.25 ms |
  | 26.04 | 0.37 ms | 0.41 ms | 0.39 ms | 0.25 ms |

  - 10 ms is 5.4 times the slowest list and 5.8 times the slowest
    `holds -r`, plus the read's base. That justifies them on that
    run.
  - The margin under writers at many snapshots a dataset is thinner
    (N-c).
  - The sweep's list with `userrefs` is not measured. It needs each
    snapshot's stats, as the manifest's list and `tagged.lua` do, and
    both of those came in under 2 ms.

### (3) TG1 and TG2

- **TG1.** `the_holds_limit_covers_its_recording` asserts 10 ms for
  `holds_per_snapshot`, and `the_lists_limit_grows_...` asserts it for
  `list_per_snapshot`. L3 (5 ms) and L4 (4 ms) are killed.
- **TG2.** Each new case pins the pool passed to `confirm`:
  - the layout's `zpool status` (T1);
  - the `props` fallback's `zpool status` (T2);
  - the assist's `zpool sync` (T3).

  Each is killed alone by `only_the_commands_own_pool_is_looked_for`.
  The layout's second call (`-g`) is not pinned (R7, TG-B).
- **The `zfs` commands left out.** The description gives the reason:
  hold, release, destroy, the take and the count say `dataset does not
  exist` of a missing pool, which is not `no such pool`. I agree: none
  of them reaches `confirm` with a recorded message.

### (4) The stand-in `zfs` in `program.rs`

- **What it answers now.** `*name,userrefs*` gives `tank@skz-old`
  with one hold, and `holds*` gives that snapshot's hold of
  `skyzfs-publish`. Every other command but `list` is logged to
  `zfs.calls`.
- **The same points are reached.**
  - Unmarked: exit 6, naming `skz-old`.
  - Marked with the kill: `sweep before skz-old`, then SIGKILL with
    no `release`.
  - Without the kill: `sweep before`, the release, `sweep after`.
- **None is skipped silently.** The point is reached only where
  `tagged()` returned `skz-old`, which needs both new commands to be
  run and parsed. The case arms check the list before `list*`, so the
  count does not take the list's answer.
- **The rig's judge.** `agent_check.py` judges the sweep's points by
  name (`sweep:before:N`) and is unchanged. AGENT-RIG-DETAIL

## Plants

`review/m2.6c5d/plants162.py`, on a scratch worktree at the head.
Each plant is applied alone as exact text (each site found once), then
`cargo test -p skyzfs-agent -p skyzfs-agent-cli --features
skyzfs-agent-cli/test-hooks` runs, with a 600 s limit, and the file is
restored after. The baseline passed (204 + 7). The worktree was clean
at the end. 31 plants.

**The author's 21, my versions.** All are killed, by the tests the
description names. L5 I made two ways, the inner and the outer cap
(L5a, L5b), and both are killed.

| Plant | Killed by (failed tests) |
|---|---|
| S1 `refs > 1` | the three sweep tests |
| S2 every snapshot's holds read | `…_that_have_one` |
| S3 `starts_with(tag)` | `…_that_have_one` |
| S4 the dataset's part kept | `…_that_have_one`, `…_vanished_under_it` |
| S5 the list on a read's limit | `…_that_have_one`, `the_holds_and_the_tags_grow_…`, `the_holds_limit_covers_its_recording` |
| S6 holds on the list's limit | `…_that_have_one` |
| S7 1001 a command | `…_a_thousand_snapshots_at_a_time` |
| S8 gone taken as no hold | `…_vanished_under_it` |
| S9 gone an error at once | `…_vanished_under_it` |
| S10 the list's missing pool not confirmed | `only_the_commands_own_pool_is_looked_for`, `…_vanished_under_it` |
| S11 no `@` taken for a snapshot | `…_that_have_one` |
| S12 no reading again | `…_vanished_under_it` |
| T1 `zpool status` on `status` | `only_the_commands_own_pool_is_looked_for` |
| T2 the fallback's `zpool status` on `status` | `only_the_commands_own_pool_is_looked_for` |
| T3 `zpool sync` on `sync` | `only_the_commands_own_pool_is_looked_for` |
| L1 the list's snapshots at 200 ms | `the_lists_limit_grows_…` |
| L2 the sweep's list on `holds_per_snapshot` | `the_holds_and_the_tags_grow_…` |
| L3 `list_per_snapshot` 5 ms | three limit tests |
| L4 `holds_per_snapshot` 4 ms | `the_holds_limit_covers_its_recording` |
| L5a the list's datasets part capped at `most` | `the_limits_grow_with_the_datasets_counted` |
| L5b the list capped at `most` | `the_lists_limit_grows_…`, `the_limits_grow_with_the_datasets_counted` |
| L6 the list's snapshots not added | `the_lists_limit_grows_…` |

**Mine.**

| Plant | Result |
|---|---|
| R1 the sweep's holds confirmed on a snapshot's name | **survives** (TG-A) |
| R2 its `Vanished` taken as no hold | **survives** (TG-A) |
| R3 its `Vanished` an error at once | **survives** (TG-A) |
| R4 a tag the agent's begins with kept | **survives** (TG-B) |
| R5 a short holds line an error | **survives** (TG-B) |
| R6 the sweep's list on the manifest list's limit | killed (three tests) |
| R7 only `zpool status -g` on `status` | **survives** (TG-B) |
| R8 the snapshot count on `full_most` | killed: a fix of R-COUNT shows in three tests, as it should |
| R9 `rsplit_once('@')` for the name | survives; equivalent, a name has one `@` |

## Gates

GATES

# Review of skyzfs/skyzfs#162 (M2.6c-5d) at b83ac36

Heads checked with `gh api repos/skyzfs/skyzfs/pulls/162 --jq .head.sha`.
The first look was at `6e420dc`. This note is the second look, at
`b83ac36bef0fd046411251e8a2fe45459b09a6c4`, branch `skyzfs-coding`,
base `zfs-topology` at 1d14ee9 (the merge base, and the base's tip when
I read it). Three commits, `crates/skyzfs-agent` and `bins/skyzfs-agent`
only:

- `16be309`: the sweep reads holds with `zfs list` and `zfs holds -H`;
  `tagged.lua` deleted; 10 ms a snapshot (`holds_per_snapshot`, new
  `list_per_snapshot`); TG1, TG2, nits A to F of my review of #161;
- `6e420dc`: a comment on the holds line the sweep skips;
- `b83ac36`: the lead's ruling on R-COUNT. The snapshot count runs
  within `Limits::full_most`; the three walks' errors name its failure
  (`walked`); one new test; three tests expect the count's new limit.

Line numbers are at `b83ac36`. `engine.rs` is the same at both heads.

Before the description I read the diff, design §5.2 and §7, the module
docs and every caller of the changed functions in `cli.rs` and
`engine.rs`, `host.rs`'s runner, the implementation notes on #155, #159
and #161, the recorder's records of run 45800940382 (both images,
downloaded), and OpenZFS's `zfs_do_holds` (2.2.2 and 2.4.1, the same
but for a printf cast), `zfs_for_each`, `zfs_open`,
`zfs_do_list_ioctl`, `zfs_standard_error_fmt`, `zfs_get_holds`,
`zfs_ioc_snapshot_list_next` and `dsl_userhold.c` (2.4.1).

## Verdict

**Nothing blocks the merge.** One defect should be fixed in this pull
request, since it is one line in the code under review (D1, unchanged
at `b83ac36`). The rest is low.

- **S1 is resolved.** The sweep runs no channel program. `zfs list`
  and `zfs holds` hold the pool only for each ioctl, so the sync is
  not held up for the read's run. The old semantics are kept: every
  snapshot of every dataset, the exact tag, the name after `@`, a set.
- **The partial `zfs holds -H` fake is faithful to the source** of
  2.2.2 and 2.4.1, and the code does not depend on its stdout anyway.
- **R-COUNT is resolved as the lead ruled.**
  - The snapshot count runs within 35 minutes, whatever the datasets.
    The runner returns at that limit even when the child cannot be
    killed, and the count does not hold the pool's sync.
  - `walked` names the count's failure in the `Other` errors of the
    three walks, and nowhere else. Every count clears the failure, so
    none reaches a later walk, and the walk's own error comes first.
  - The new test fails when the count goes back to a read's limit, by
    what its host does and not only by the limit it names (Q6).
  - Two survivors, both low (TG-C).
- **The limits are as ruled**, saturate, and are capped.
- **TG1 and TG2 are answered.** My versions of the author's 28 plants
  (21 at `6e420dc`, C1 to C7 at `b83ac36`) are all killed, by the
  tests the description names. Of mine, eight survive: R1 to R5 and R7
  at the first look, Q3 and Q4 at the second (TG-A, TG-B, TG-C). All
  are gaps in tests, not in the code.
- **The stand-in `zfs` of `program.rs` still drives the sweep's kill
  point honestly**, through both new commands. On a real pool, the
  agent rig's sweep course (job k, Ubuntu 26.04, ZFS 2.4.1) passed at
  `b83ac36`: a stray hold released, a kill at either sweep point, and
  exit 6 in an unmarked directory with nothing released.
- **Gates and CI.** fmt, clippy, the tests and the docs are clean on
  all four feature sets, with the description's counts. Every CI
  job on `b83ac36` succeeded.

## Findings, most severe first

### D1 (defect, medium-low; fix in this PR): another holder's tag that is not UTF-8 stops every agent process

- **Where.** `crates/skyzfs-agent/src/zfs/cli.rs:859`. Each of the
  sweep's `zfs holds -H` goes through `self.zfs`, that is `run`, which
  fails the whole output where it is not UTF-8 (`cli.rs:628-635`).
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
- **Certainty.** Certain from the code, and shown twice at `6e420dc`.
  `b83ac36` does not touch these lines.
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

### R-COUNT (resolved at `b83ac36`): the snapshot count no longer ends the round's reach

- **What it was, at `6e420dc`.** The snapshot count ran within a
  read's limit on the datasets counted (60 s + 100 ms a dataset). On
  Ubuntu 24.04 it took 1.25 ms a snapshot, so it failed past about
  48 000 + 80 × D snapshots. Every walk then fell back to 61 to 80 s
  and ran out of time: every round failed from about 50 000 snapshots.
  The figures, from the records of run 45800940382 (one run on each
  image, idle pool):

  | Image, pool | snapshot count | its old limit | `holds -r` | manifest's list |
  |---|---|---|---|---|
  | 24.04, 10:1000 (11 000) | 13.3 s | 61.1 s | 18.9 s | 20.3 s |
  | 24.04, 100:1000 (101 000) | **126.2 s** | **70.1 s** | 174.2 s | 185.9 s |
  | 26.04, 10:1000 (11 000) | 2.7 s | 61.1 s | 3.9 s | 4.3 s |
  | 26.04, 10:10000 (60 500) | 14.8 s | 61.1 s | 21.9 s | 24.2 s |
  | 26.04, 100:1000 (101 000) | 25.7 s | 70.1 s | 37.3 s | 41.6 s |

- **The ruling, as built.** The count runs within `Limits::full_most`
  (`cli.rs:1068`). Where it still fails, the walks keep the limits
  they had and their errors name the count's failure. That was my
  option 1, plus the error the lead asked for. Checked below,
  "Second look".
- **Reach now.** The count's 35 minutes is about 1.7 million
  snapshots at 1.25 ms (24.04, idle). The walks' limits stop growing
  at about 200 000 snapshots, at 35 minutes, and at the recorded rates
  that still covers the slowest walk (the list, 1.84 ms) up to about
  1.1 million. So the count is no longer the first to fail at any
  size; on that run's figures the list is.

### F1 (low): another holder's tag that holds a newline can add a line to the sweep's read

- **Where.** `cli.rs:869-883`. Each line of `zfs holds -H` is split at
  tabs. A line is kept where its second field is the agent's tag,
  whatever its first field.
- **Scenario.** A tag `x\ntank@zz\tskyzfs-publish` on any held
  snapshot prints a line `tank@zz<TAB>skyzfs-publish<TAB>...`. The
  sweep then reads `zz` as held by the agent.
- **Effects, by reading the code.**
  - In a marked directory, the agent asks `zfs release -r
    skyzfs-publish tank@zz`. That is `NoSnapshot`, or `no such tag`
    and then an empty `holds -r`: both `Ok` (`cli.rs:1174-1192`). A
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
  (`cli.rs:784-806`, `zfs holds -H -r pool@t`, since #147) parses the
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
  `confirm` with the pool passed at `cli.rs:859`. By reading, that pool
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

### TG-C (low, new at `b83ac36`): two properties of `walked` that no test pins

- **Q3: a walk's `Vanished` or `NoPool` made `Other` while a count
  failure is kept** (`walked`'s first arm matching any error). It
  survives: the new test feeds the walks only `Other`. Were it so,
  `again` would no longer read a vanished snapshot again in the sweep
  or the manifest's list, and a missing pool would read as another
  error.
- **Q4: the failure cleared only by a count whose snapshot count
  succeeds**, not after every dataset count. It survives: no test
  fails a dataset count after a failed snapshot count. Were it so, a
  round whose dataset count failed would run its walks on the base
  limits and name a snapshot count that is no longer the reason.
- **The code is right on both today** (`cli.rs:464-472`,
  `cli.rs:1051`); only the pins are missing.
- **Fix.** Two more steps in
  `a_count_past_the_datasets_limit_is_not_cut_off`, after the failed
  count: a walk answered by a vanished snapshot, or by a `zpool list`
  without the pool, whose kind is checked unchanged; and a count whose
  dataset count fails, then a walk whose error has no note.

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
- **Who answers.** Codex's state for `b83ac36` still lists it as
  unresolved. The author should answer its thread on the pull
  request.

### Nits

- **N-a: a dataset gone during the list is not an error.** The doc of
  `tagged_once` (`cli.rs:813-816`) says "a dataset gone during either
  is Vanished". The test feeds `cannot iterate filesystems: no such
  pool or dataset` to the list (`cli/tests.rs:1684-1695`). libzfs does
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
  (`cli.rs:384-389`) gives 26.04's list as "41.2 s" and the run as
  45800940382, on an idle pool. In that run it took 41.6 s. 41.2 s is
  run 45781769557's, beside the writers, as the module docs
  (`cli.rs:294-295`) say. The rate, 0.41 ms, is right either way.
- **N-c: the per-snapshot cost grows with the snapshots per dataset.**
  #159 measured on 26.04, under writers: 0.40 ms a line up to 5500 a
  dataset, and 1.06 ms at 12 500. The "about five times the slowest"
  is at 1000 a dataset on an idle pool. At 12 500 a dataset on 24.04
  the margin may be about two. The docs could say so, or testing
  records one 24.04 point at a high count per dataset.
- **N-d: argument length.** `HOLDS_AT_ONCE`'s doc (`cli.rs:318-321`)
  says "far below the kernel's limit". Linux's limit is a quarter of
  the stack's rlimit, so a unit with `LimitSTACK=1M` allows 256 KiB,
  which is 1000 names of the longest kind. Real names are far shorter.
  Say "with the default stack limit".
- **N-e: the skip's reason.** The comment at `cli.rs:871-874` says the
  skip "keeps the hold it might name". Only another holder's
  multi-line tag makes a line without two fields: the agent's own
  lines are always whole. So the skip never keeps an agent's hold,
  and that is the better reason to give.

## Second look: `b83ac36`, the lead's ruling on R-COUNT

### (1) The snapshot count within `Limits::full_most`

- **The code.** `count` (`cli.rs:1037-1080`) runs the dataset count
  within `Limits::count` (60 s), then `zfs list -r -H -o name -t
  snapshot POOL` within `self.limits.full_most`. That is the
  configured cap itself, not grown and not depending on the count it
  makes. 35 minutes by default.
- **The cap holds.** `System::run` (`host.rs:165-188`) returns
  `TimedOut` at the limit. It kills the child's process group and does
  not wait for it, so a child stuck on a suspended pool does not hold
  the agent past 35 minutes. The `Slow` host of the test returns the
  same `TimedOut`.
- **The pool is not stalled.** The count lists names only (libzfs's
  simple walk), one `SNAPSHOT_LIST_NEXT` ioctl at a time, each with
  its own short hold of the pool (`zfs_ioctl.c:2415-2468`). Nothing
  holds the configuration lock for the count's run, so the sync goes
  on beside it.
- **How long a round can wait.** Before the sweep, a round can now
  wait up to 60 s for the dataset count and 35 minutes for the
  snapshot count. The long wait needs a pool that answered the
  dataset count, so the case of #153's N2 (a pool on which every
  command hangs) still ends at the dataset count's 60 s. The
  description says this under "Not done", and the ruling chose it.
  - One consequence, low: `settle(limit)` checks its end between
    rounds, so a round's count can now overrun it by up to 35
    minutes. The walks could already overrun it by as much, so this is
    not new in kind.
- **Names and output.** A snapshot's name is ASCII, so the count's
  output is always UTF-8. At 1.7 million snapshots its output is about
  70 MB, held once.

### (2) `walked`

- **Where it is applied.** Exactly the three walks whose limit grows
  with the snapshots counted: `holds -r` in `tagged_snapshots`
  (`cli.rs:794`; so `held`, `hold` and `release`), the sweep's list
  (`cli.rs:832-834`), and the manifest's list (`cli.rs:1098-1100`).
  Those are the only callers of `walk_limit` and `list_limit`. The
  sweep's per-chunk `zfs holds -H` and the manifest's gets are not on
  a snapshot limit and do not append. That is right.
- **What it passes through.** Only `Other` is changed. `NoSnapshot`
  is matched before `walked` in `tagged_snapshots`, and `Vanished`
  and `NoPool` are returned as they are, so `again` still reads a
  vanished snapshot again and a missing pool is still `NoPool`. Not
  pinned by a test (Q3, TG-C).
- **No stale failure.** `uncounted` is set only by `count`, which is
  also the only writer of `scale`. It is cleared after every dataset
  count, before its result is read. So:
  - after a count that counted both, it is `None`;
  - after a count whose snapshot count failed, it holds that failure,
    and the walks are on the limits it left;
  - after a count whose dataset count failed, it is `None`, and the
    scale is `Base`.

  The engine counts at the start of every round, before the sweep
  (`engine.rs:643`, `engine.rs:691`). Between two counts the scale
  does not change, so the note is true of every walk it is added to.
  Commands run without a count first (a fresh `ZfsCli`) have neither.
  The clearing on a failed dataset count is not pinned (Q4, TG-C).
- **No masking.** The walk's own error comes first and is kept whole;
  the count's failure is added after it in parentheses. The test
  checks that the walk's own words stay (Q5 is killed).
- **Outside the ruling, for the record.** A failed dataset count
  leaves every limit at its base, and the walks' errors then do not
  say so. That was the lead's ruling on N2 of #153 (the round's own
  calls say what is wrong). The dataset count does not walk the
  snapshots and has 60 s, so I have no case where it times out on a
  pool that answers. No change asked.

### (3) `a_count_past_the_datasets_limit_is_not_cut_off` and its `Slow` host

- **The host.** `Slow` times a command out when the limit given is
  shorter than the time the test gives it. It records the call and
  returns `TimedOut`, as `System::run` does at its limit. Otherwise
  the `Fake` answers. Equal is not cut off, and the test uses 126.2 s
  against 35 minutes, then 35 minutes and 1 ms.
- **What it checks.** The count is given `full_most`. The three walks
  then get their grown limits, worked out from the recorded figures.
  When the count is cut off, the dataset count stands, the walks keep
  a read's limit (the list the gets'), and each error holds the walk's
  own words, the note and the count's command. A count that succeeds
  again clears it.
- **Would it fail if the count used the datasets' limit again?** Yes,
  twice over. C1 (a read's limit, 70.1 s) is killed by it and by the
  three tests that pin the count's limit. Q6 is C1 with the test's two
  checks of the count's limit taken out of a scratch copy. That test
  still kills it: the count is cut off at 70.1 s, and the walks then
  get a read's limit and not their grown one. Q2 (the gets' limit,
  80.2 s) and C2 (the count's 60 s) are killed too.

## What I checked at the first look, by the four questions

### (1) `tagged_once` (`cli.rs:808-886`)

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
  - the manifest's list (`list_limit`, `cli.rs:508-523`):
    `min(min(60 + 0.2·D, 2100) + 0.01·S, 2100)`;
  - `holds -r` and the sweep's list (`walk_limit`, `cli.rs:534-543`):
    `min(min(60 + 0.1·D, 1200) + 0.01·S, 2100)`;
  - each of the sweep's `zfs holds -H`: `min(60 + 0.1·D, 1200)`;
  - the snapshot count, since `b83ac36`: 2100.
- **Saturation.** `grown` takes `most` where S does not fit in a
  `u32` or the product overflows, then adds with `saturating_add` and
  caps. The cap is reached at about 204 000 snapshots with 10
  datasets, as the tests pin (203 700 under it, 203 900 at it).
- **Every command that walks the snapshots** has a limit that covers
  it:
  - `holds -r` through `tagged_snapshots`, in `held`, `hold` and
    `release`;
  - the sweep's list;
  - the manifest's list;
  - the snapshot count, at the cap (since `b83ac36`).

  The gets, `hold -r`, `release -r`, `destroy -r` and `snapshot -r`
  walk the file systems only.
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

- **TG1.** `the_holds_limit_covers_its_recording`
  (`cli/tests.rs:2240`) asserts 10 ms for `holds_per_snapshot`, and
  `the_lists_limit_grows_...` asserts it for `list_per_snapshot`. L3
  (5 ms) and L4 (4 ms) are killed.
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
  name (`sweep:before:N`) and is unchanged.
- **On a real pool.** Run 45827334221 at `b83ac36`, job k (Ubuntu
  26.04, ZFS 2.4.1, through `skyzfsd`), the course `sweep`: the stray
  hold that no publish names is released (`pin_sweep_swept`); killed
  at `sweep:before:1` and at `sweep:after:1`, the retry publishes and
  leaves no hold of the tag; in a fresh directory with such a hold,
  exit 6 with nothing released (`pin_unmarked_refused`). `sweep` is
  among the points reached. So the list with `userrefs` and `zfs holds
  -H` of the held snapshots parse as the fakes say on 2.4.1. The rig
  runs on 26.04 only, and with one stray hold: it does not show a
  chunk of 1000, a snapshot gone between the reads, or 24.04's 2.2.

## Plants

Each plant is applied alone as exact text (each site found once) on a
scratch worktree at the head, then `cargo test -p skyzfs-agent -p
skyzfs-agent-cli --features skyzfs-agent-cli/test-hooks` runs, with a
600 s limit, and the file is restored after. The worktree was clean at
the end of each run.

### First look, at `6e420dc` (`plants162.py`, 31 plants)

The baseline passed (204 + 7).

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
| R8 the snapshot count on `full_most` | killed at `6e420dc`: the fix of R-COUNT showed in three tests, as it should. It is `b83ac36`. |
| R9 `rsplit_once('@')` for the name | survives; equivalent, a name has one `@` |

The code these plants touch is the same at `b83ac36`, except the
count (R8) and the sites `walked` now wraps.

### Second look, at `b83ac36` (`plants162b.py`, 13 plants)

The baseline passed (205 + 7). 13 plants: my versions of the author's
C1 to C7, and six of mine. All but two are killed.

| Plant | Result |
|---|---|
| C1 the snapshot count on a read's limit (as before) | killed: `a_count_past_…`, `a_value_read_alone_takes_a_reads_limit`, `the_limits_grow_with_the_datasets_counted`, `the_lists_limit_grows_…` |
| C2 the count on its own 60 s | killed: the same four |
| C3 `holds -r`'s error not naming the count | killed: `a_count_past_…` |
| C4 the sweep's list's error not naming it | killed: `a_count_past_…` |
| C5 the manifest's list's error not naming it | killed: `a_count_past_…` |
| C6 the failure never cleared | killed: `a_count_past_…` |
| C7 the failure kept without its reason | killed: `a_count_past_…` |
| Q1 the count on the other reads' cap (20 min) | killed: the same four as C1 |
| Q2 the count on the gets' limit (80.2 s at 101 datasets) | killed: the same four as C1 |
| Q3 a walk's `Vanished` or `NoPool` made `Other` | **survives** (TG-C) |
| Q4 the failure cleared only when the snapshots are counted | **survives** (TG-C) |
| Q5 the walk's own error dropped, the count's alone said | killed: `a_count_past_…` |
| Q6 C1 with the test's two checks of the count's limit taken out | killed: the same four as C1 |

The author's table lists the same tests for C1 to C7.

## Gates

At `b83ac36`, on a scratch worktree, rustc 1.100.0 after `rustup
update stable`, with `CARGO_INCREMENTAL=0` and no debug info. `cargo
fmt --all -- --check` is clean. For each feature set: `cargo clippy
--workspace --all-targets -- -D warnings`, `cargo test --workspace`,
and `cargo doc --workspace --no-deps` with `RUSTDOCFLAGS=-D warnings`.

| Feature set | clippy | tests | doc |
|---|---|---|---|
| plain | clean | 1961 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean | 2019 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | clean | 1985 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | clean | 1961 passed, 0 failed, 16 ignored | clean |

The counts are the description's. With `skyzfs-agent-cli/test-hooks`,
`the_agent_traces_a_point_then_dies_of_sigkill_there` ran and passed
(it takes the place of a test built without the feature, so the count
is the plain one).

At `6e420dc` I ran fmt, clippy, tests and doc plain (1960 passed) and
clippy with the daemon's hooks, all clean, before the new head came.

## CI

At `b83ac36`, read with `gh api .../commits/<head>/check-runs`: every
job succeeded. The CI suite's test, lint and unit-arm64, coverage,
e2e-tier-a, the k3s smokes, s3-backends and chart-lint; the agent rig
(run 45827334221): a1, a2, d and k, all on Ubuntu 26.04, and "the
check's logic". `skyzfs-ci.yml`'s jobs are skipped, as on any pull
request. At `6e420dc` the same jobs succeeded but for the rig's d and
k, which the push of `b83ac36` cancelled.

Codex's review state for `b83ac36` has one unresolved finding, its P2
on `tagged.lua` (G-REC), and nothing new. Its thread had no reply when
I read it.

# Review of skyzfs/skyzfs#168 (M2.6c-5f) at 85c4708

Head checked with `gh api repos/skyzfs/skyzfs/pulls/168 --jq .head.sha`.
I began at `2233537a164f5d97ad40968430af672f51555e69`, branch
`skyzfs-coding-m2.6c-5f`: one commit on `zfs-topology` at 517d055, which
was both the merge base and the base's tip, so nothing already on
`zfs-topology` was in the diff. While I read it, the head moved to
`85c4708583499515e109bb867238fa815fdc27c6`: a merge of the tip 2751e3b,
which carries #164 and the rig's rename (22009e9). **This review is
against 85c4708.**

**The merge changes no code** (checked, not taken on trust):

- `git diff 2233537 85c4708` is byte for byte `git diff 517d055 2751e3b`,
  what `zfs-topology` brought: rig scripts, workflows and the notes.
- `git diff 2751e3b 85c4708` is byte for byte `git diff 517d055 2233537`,
  the pull request's own change.
- Nothing under `crates/`, `bins/`, `Cargo.toml`, `Cargo.lock` or
  `rust-toolchain.toml` differs between 2233537 and 85c4708.

So the gates, plants and scratch tests, run on a worktree of 2233537,
hold for 85c4708. Line numbers are the same at both.

Before the description I read:

- the diff, design §5.2, and §5.4 where the restore releases the tag;
- `names.rs`; in `engine.rs`, `fresh_name`, `rename`, `begin`, `settle`
  and `sweep`; `record.rs`'s `check`; the fake's `tagged`;
- my notes on #165 (`review/m2.6c5e/`) and the lead's notes on
  M2.6c-5e;
- #164's commit 22009e9, the rig's rename;
- OpenZFS 2.4.1: `dsl_dataset_rename_snapshot_check_impl`
  (`dsl_dataset.c`) and `zfs_release_one` (`libzfs_dataset.c`).

## Verdict

**One thing blocks, and it is a one-line fix (B1):** the commit adds
`crates/skyzfs-agent/src/zfs/cli.rs.orig`, a byte-for-byte copy of the
new `cli.rs`. It is 2214 of the diff's 2682 added lines. Remove it.

**Otherwise nothing blocks from review.**

- **The prefix (question 1).** It is read on `t` alone, with its dash,
  before the holds are read. Every name the agent gives begins with
  `skz-`, and has since the agent's first commit. The `-r` reads of
  `held`, `hold` and `release` need no filter: they read one `@t`, a
  name of the agent's. One way a hold of the agent's now goes unswept
  is new and not documented: a person renames a held snapshot of the
  agent's (L1, low, for the lead).
- **Read once, read again (question 2).** The split is right. Only
  `outnumbered` can change between reads; `hold_lines`' errors come
  from a tag and are read once. The bound is three reads in all.
- **Plants (question 3).** 51 plants, each alone, against both agent
  crates: 46 killed. My nine TG1 survivors from #165 all die now, by
  the tests the description names. Of the new ones, every plant on the
  filter's place and form dies (no dash, in the whole name, in the
  dataset, anywhere in `t`, after the holds read, dropped, `or`, a name
  with no `@`), and every plant on the split dies but one. Four
  survive, all test gaps (TG2): the prefix's case (A2), the fake's
  filter (A9), the no-newline error's single read (S2), and N3's
  message (N3m). M16 is equivalent, as on #165.
- **#165's other findings (question 4).** R1, N1, N2, N3 and N5 are
  answered. N4 names its new cause the wrong way round for `held`,
  `hold` and `release` (N4', a nit). A few docs are now stale (N6).
- **Gates.** fmt, clippy `-D warnings`, the tests and `cargo doc` with
  `-D warnings` are clean on all four feature sets, on rustc 1.100.0,
  the current stable. The counts match the description's.
- **CI at 85c4708.** All six runs are green, the agent rig's job k
  included: 1049 pins ok, none failed, worst exit 0, the 36 K-sweep
  cases ran (with kills at `sweep:before` and `sweep:after`), and
  `pin_sweep_swept`, `pin_sweep_clean`, `pin_unmarked_refused` and
  `pin_unmarked_outcome` are ok. All three controls were caught.
- **The rig's renamed strays.** Every stray hold the agent rig places
  now sits on an `@skz-…` name, so the narrowed sweep reads it. The
  rig's expectations hold, and job k shows it. What the rig does not
  show is the other side: a hold on a name without `skz-` left alone
  on a real pool (a gap for testing, not for this merge).
- **Codex.** Its one finding (P2, `cli.rs:1188`, the rig's fixtures
  outside the prefix) is settled by the merge of #164's rename and is
  answered on its thread. Its review of 85c4708 has no findings.

It can merge from review once B1 is gone and CI is green on that head.
The lead's condition (the rename on `zfs-topology`, the tip merged in,
the rig green on that head) is met at 85c4708.

## What is in the diff (the +2682)

`git diff --stat 2751e3b 85c4708` (the pull request's own change):

| File | Added | Removed |
|---|---|---|
| `zfs/cli.rs.orig` | 2214 | 0 |
| `zfs/cli/tests.rs` | 338 | 42 |
| `zfs/cli.rs` | 127 | 56 |
| `zfs.rs`, `engine/fake.rs` | 3 | 1 |

`cli.rs.orig` is the same blob as the new `cli.rs` (`c3b9396`), so it
is a leftover of a patch, not a change. Without it the change is
+468/−99, in the agent's crate only.

## Question 1: the prefix

### The names the agent gives

- **One source.** A publish gets its name from `fresh_name`
  (`engine.rs:1607`), which calls `names::name`
  (`skz-YYYYMMDDTHHMMSSZ`, then `-N` where taken). It is called by
  `begin` (`engine.rs:660`) and by `rename` after a retake
  (`engine.rs:1336`). Nothing else writes a name into the record, and
  the program takes no name from a person.
- **Since the start.** `PREFIX = "skz-"` came in with the agent's crate
  itself (ac097d9, M2.6b1), with the record. No older form exists.
- **Where the hold goes.** The agent holds `pool@t -r` with `t` from the
  record (`cli.rs:1493`). So every hold it places is on a snapshot
  whose part after `@` begins `skz-`. The sweep exists for one case: a
  rewind brings back such a hold after its publish left the record. A
  rewind brings back names as they were, so the prefix holds there too.
- **What the design says.** §5.2 names the snapshot `pool@t` and the
  ref `snap/t`, and gives no rule for `t`. The rule is the plan's (M2.6
  §6, the lead's Q6). Nothing in the design conflicts.

### Can a hold of the agent's sit on a name without `skz-`?

| Case | Result |
|---|---|
| A dataset-qualified name, `pool/a/b@skz-…` | `agents_name` splits at the first `@` and tests `t`. ZFS allows no `@` in a dataset's name, so that `@` is the snapshot's. Right (A3, A4, A10 killed) |
| A `t` with `@` in it | ZFS refuses it (`@` is not a component character). Not possible |
| A dataset named `skz-…`, `pool/skz-db@daily` | not the agent's. Right, and tested (A3, A4 killed) |
| Upper case, `pool@SKZ-1` | not the agent's; ZFS names are case-sensitive. Right; untested (A2 survives) |
| The pool imported under another name, a dataset renamed | `t` is unchanged. Swept |
| A snapshot of the agent's **renamed by a person while held** | **not swept any more (L1)** |
| The agent's tag put by hand on another snapshot | not swept. The lead's ruling accepted this |
| The rig's reference agent (`tests/kernel/publish_agent.py`, `pool@t1`) | the only other writer of the tag in the repository, on the publish rig's own pools. Not the product |

### Can a snapshot the agent did not take be read as the agent's?

Yes: any `@skz-…` that someone else made. That is harmless, and no
worse than the base, which read every held snapshot:

- With the agent's tag on it, put by hand, the sweep releases that tag
  where no publish names `t`. The base did that for every name.
- With another holder's tag that has a tab or a newline, the sweep
  stops until that hold is released. The base did that for every name;
  this is the cost the ruling keeps.
- `fresh_name` checks `members` before it takes a name, so a person's
  `pool@skz-…` is passed over, not taken over.

### Are the filters consistent?

- **The sweep** (`tagged_once`, `cli.rs:1186-1190`) filters the list
  before any `zfs holds`. So the holds of a snapshot not the agent's
  are never read. `asked` is the command's own names, all of them the
  agent's. A forged line on an `@skz-…` that names a snapshot not the
  agent's is a line not asked: an error, read once.
- **`held`, `hold` and `release`** read `zfs holds -H -r pool@t` with
  `t` from the record, and `under` (`cli.rs:1092-1100`) keeps the lines
  of that `@t` under the pool. They need no prefix: every line they
  take is of the agent's own `t`.
- **The sweep's release** is `release(pool, t)`, which runs `zfs
  release -r` and, on `no such tag`, the same `-r` check. Same `t`.
- **The fake** (`engine/fake.rs:1625`) filters `t` by the same prefix.
  No engine test depends on it (A9 survives).

So the sweep's reads and its releases agree, and the `-r` paths are
untouched. The doc comments' claim of conservatism is right: the prefix
reads more than the full form would, never fewer.

## Question 2: read once, read again

`Odd` now carries `again` (`cli.rs:421-433`), and `again_checked`
(`cli.rs:973-997`) reads again only `Odd { again: true }` and
`Vanished`, while `tries < VANISHED_READS`.

| Error | Where | `again` | Can it change between reads? | Verdict |
|---|---|---|---|---|
| output with no newline at its end | `hold_lines`, `cli.rs:466-471` | false | No. `print_holds` ends every hold with `\n`, and a command killed at its limit is an error before this. | right |
| a line not three fields, or of a snapshot not asked | `hold_lines`, `cli.rs:489-496` | false | No. Its bytes are a tag's, and a tag does not change while it is held. A race cannot make such a line: the sweep's `zfs holds -H NAMES` prints only the names given, and `holds -r` prints only the `@t` it walks, each under the pool. A snapshot renamed during the walk is printed under the name opened. | right |
| more lines than holds | `outnumbered`, `cli.rs:526-540` | true | Yes: the holds change between the two commands | right |
| a name gone, at the holds or at the count | `ZfsError::Vanished` | — | Yes | read again, as before |
| the count's parse (`userrefs`), `NoPool`, a timeout, anything else | `ZfsError::Other`, `NoPool` | — | not by reading again | returned at once, as every other read |

- **Bounded.** One counter for `Vanished` and `Odd { again: true }`,
  three reads in all, as before. A read past the bound returns its own
  error: `Vanished` as itself, an `Odd` as `Other` with "(read 3
  times)". S4 (four reads) and S6 (the counter stepped by two) die.
- **The message's count is right.** `tries` is the reads made, so an
  `Odd { again: false }` after a `Vanished` says "(read 2 times)". S5
  (off by one) dies.
- **What it saves.** A tag with a tab or a newline on an agent's
  snapshot now costs one walk, not three, before the error: in the
  sweep one `zfs list -r` of every snapshot, in `held` one `holds -r`.
- **What it does not save.** A forged whole line that names a snapshot
  asked is found only by `outnumbered`, so it is still read three
  times: the code cannot tell it from holds that change. That is right.

S1 (every `Odd` read again) and S3 (a line that is not a hold read
again) die. The one survivor here is a test gap, not a defect: S2 (TG2,
the no-newline case's single read is not pinned).

## Question 3: the plants

`plants168.py` in this directory, run as `python3 -I plants168.py
WORKTREE IDS` on a scratch worktree of 2233537. Each plant is applied
alone as exact text (each site found exactly once), then

```
cargo test -p skyzfs-agent -p skyzfs-agent-cli \
  --features skyzfs-agent-cli/test-hooks --no-fail-fast -- --test-threads=4
```

runs with a 600 s limit, and the file is restored in a `finally`. The
worktree was clean after all 51. Full output: `plants168.out`.

### #165's TG1, my survivors there

| Plant | What | Killed by |
|---|---|---|
| M3 | the `-r` count on a walk's limit | `the_holds_and_the_tags_grow_with_the_snapshots_counted` |
| M4b | the `-r` count not batched, none where none | `the_holds_check_reads_only_the_agents_tag_and_the_counts_answer` |
| M5 | the `-r` reads take a tag that the agent's begins | the same |
| M6, M7 | the checks of `hold` and `release` do not read again on `Vanished` | the same |
| M8 | the `-r` count's exit code not checked | the same |
| M9 | the sweep's `asked` is any held snapshot of the pool | `holds_that_do_not_add_up_are_an_error_that_names_the_snapshot` (its 1001-name case) |
| M10 | fewer lines than holds an error too | `the_holds_check_reads_only_the_agents_tag_and_the_counts_answer` |
| M14 | the count's other failures made `Vanished` | the same |

All nine die. M16 (the `-r` tag compared after a lossy UTF-8 decode)
survives, as on #165; it is equivalent: a lossy decode equals the ASCII
tag only where the bytes do.

### The rest of #165's plants

G1 to G15 (my versions of the author's #165 plants, G7 now "more lines
than holds not read again") and M1, M2, M4, M11, M12, M13, M15, M17:
all killed.

### New: the prefix

| Plant | What | Result |
|---|---|---|
| A1 | the prefix without its dash (`skz`) | killed: `another_holders_odd_tag_on_a_snapshot_not_the_agents_does_not_stop_the_sweep`, `the_sweep_reads_the_holds_of_the_snapshots_that_have_one` |
| A2 | the prefix in any case (`SKZ-1` read) | **survives** (TG2) |
| A3 | the prefix anywhere in the whole name, dataset too | killed: `another_holders_odd_tag_…` |
| A4 | `t`, or the dataset's last component, begins with it | killed: the same |
| A5 | the prefix anywhere in `t` | killed: the same |
| A6 | the filter after the holds read (every held snapshot read, the agent's kept) | killed: both tests of A1 |
| A7 | the filter dropped (the base) | killed: the same |
| A8 | the filter `or` the count | killed: the same |
| A9 | the fake's `tagged` without its filter | **survives** (TG2) |
| A10 | a name with no `@` taken as the agent's | killed: `another_holders_odd_tag_…` |

### New: read once, read again, and #165's messages

| Plant | What | Result |
|---|---|---|
| S1 | every `Odd` read again (N2 undone) | killed: `holds_that_do_not_add_up_…`, `the_holds_under_a_snapshot_are_checked_as_the_sweeps_are` |
| S2 | an output with no newline at its end read again | **survives** (TG2) |
| S3 | a line that is not a hold read again | killed: the two tests of S1 |
| S4 | four reads, not three | killed: four tests |
| S5 | the message's count of reads off by one | killed: the two tests of S1 |
| S6 | the counter stepped by two | killed: four tests |
| N3m | the count's missing-snapshot message without the forged cause | **survives** (TG2) |
| N4m | `outnumbered`'s message without the churn cause | killed: `holds_that_do_not_add_up_…` |

## Question 4: #165's other findings

| Finding | Claimed | At 85c4708 |
|---|---|---|
| R1, the residual race | the claim that a count on both sides would close it is withdrawn; the race stays as accepted | Right. The race is written in the lead's notes on M2.6c-5e and nowhere in the code, where two comments still say the check cannot be forged (N6) |
| N1, why a line is not skipped | the docs give the real reason | Fixed, and right: the module doc (`cli.rs:94-98`) and `hold_lines`' (`cli.rs:455-458`) both give the count, with a worked tag |
| N2, reading again | a line that is not a hold is read once | Fixed (question 2) |
| N3, a forged line naming a snapshot that never was | the message says so and gives the advice | Fixed (`cli.rs:1115-1124`). The message is not pinned by a test (N3m, TG2) |
| N4, the count's error | it names both causes | Done, but the churn cause is named the wrong way round for `held`, `hold` and `release` (N4') |
| N5, #165's description | stated in this description | The limit and "every publish on the pool" are right. The race's second case is right but for one word (N7) |
| TG1 | tests for the nine survivors | All nine die (question 3) |

## Findings, most severe first

### B1 (blocks; certain; a one-line fix): a copy of `cli.rs` is committed as `cli.rs.orig`

- **What.** `crates/skyzfs-agent/src/zfs/cli.rs.orig`, 2214 lines, the
  same blob as the new `cli.rs` (`git rev-parse
  85c4708:…/cli.rs.orig 85c4708:…/cli.rs` gives `c3b9396` twice). It is
  a patch tool's backup. Neither the commit message nor the description
  names it, and Codex did not flag it.
- **Harm.** None to the build: no module names it, and cargo, rustfmt
  and clippy do not read it. But it would land on `zfs-topology` as a
  second copy of the agent's CLI module. Every grep of the module finds
  it twice (mine did), and it goes stale at the next change to
  `cli.rs`.
- **Fix.** `git rm crates/skyzfs-agent/src/zfs/cli.rs.orig`, as a new
  commit.

### L1 (low; for the lead): a held snapshot of the agent's that a person renames keeps the agent's hold

- **ZFS allows it.** A held snapshot can be renamed:
  `dsl_dataset_rename_snapshot_check_impl` (2.4.1, `dsl_dataset.c:2970-2997`)
  checks the names, not the holds. Holds stop only a destroy.
- **Scenario.** A publish under way holds `tank@skz-X` and
  `tank/db@skz-X`. A person runs `zfs rename tank/db@skz-X
  tank/db@keep`, or `zfs rename -r tank@skz-X tank@keep`.
  - The publish ends with `zfs release -r skyzfs-publish tank@skz-X`.
    libzfs's `zfs_release_one` releases each `ds@skz-X` that exists and
    has the tag, so the command exits 0. After a rename of them all it
    says the snapshot does not exist, which `release` takes as released
    (`cli.rs:1536`).
  - `tank/db@keep` keeps `skyzfs-publish`.
- **At the base** the next sweep found `keep`, which no publish names,
  and released it. `zfs release -r skyzfs-publish tank@keep` collects
  `tank/db@keep` even though `tank@keep` does not exist:
  `zfs_release_one` walks the file systems whatever the root answers.
- **Now** `agents_name("tank/db@keep")` is false, and nothing of the
  agent's releases it. Scratch test `scratch_renamed_snapshot_not_swept`
  (`scratch_tests.rs`): the sweep runs its list, no `zfs holds`, and
  returns no names.
- **Cost.** The person cannot destroy `tank/db@keep` until they release
  `skyzfs-publish` from it. `zfs holds` shows it, and they acted by
  hand, like the case the ruling accepted. I would accept it.
- **Ask.** Say it where the trade is written:
  - `agents_name`'s doc (`cli.rs:406-414`) names only "a hold of the
    agent's tag on a snapshot of another name, made by hand". Add "or a
    snapshot of the agent's renamed by hand".
  - The engine's sweep doc (N6).
- **For M3.** Design §5.4: the restore "first releases, in the writable
  clone, every hold that carries the agent's publication tag".
  `Zfs::tagged` now reads only `@skz-…`. Built on `tagged`, a restore
  would leave a renamed snapshot's hold in the clone, and the rollback
  would fail on it. That is M3's to decide; this is a note so that
  `tagged` is not reused there unawares.

### N4' (nit; certain): the count's new cause is named the wrong way round for `held`, `hold` and `release`

- **Order of reads.**
  - The sweep reads the count first (its list) and the holds after.
    There, churn gives more lines than holds when a hold is *added*
    between.
  - `tagged_snapshots`, used by `held`, `hold` and `release`, reads
    `holds -r` first and the count after (`cli.rs:1073-1141`). There,
    more lines than holds come from a hold *released* between. A hold
    added between gives fewer lines, as the new test says
    (`cli/tests.rs:2324`: "Fewer lines than holds: holds added between
    the two reads").
- **The new texts say "added" for every path:**
  - the message (`cli.rs:529`);
  - `outnumbered`'s doc (`cli.rs:507-508`);
  - `Odd`'s doc (`cli.rs:427-428`);
  - `again_checked`'s doc (`cli.rs:966-967`);
  - the module doc (`cli.rs:90`), right after it describes the `-r`
    order.
- **Scratch test `scratch_n4_direction_in_held`.** `holds -r` shows a
  `.send-77-1` hold beside the agent's. The count, read after that hold
  was released, says 1. `held` gives `Err(Other("… holds were added
  between the two reads, again and again, … (read 3 times)"))`.
- **Why it matters a little.** `held` runs in every round of every
  publish, so it is the path a person is most likely to meet.
- **Fix.** "holds changed between the two reads (added after the
  sweep's list, or released after `holds -r`)". The lead's notes have
  the forged-line case the right way round.

### TG2 (low; certain): four plants survive

Each is a missing pin; the code is right in each case.

| Plant | Survives because | Were it so | A pin |
|---|---|---|---|
| A2, the prefix in any case | no fixture has an upper-case `SKZ-…` | the sweep would read and release on `pool@SKZ-…`, which the agent never names | an `@SKZ-1` with an odd tag in `another_holders_odd_tag_on_a_snapshot_not_the_agents_does_not_stop_the_sweep` (`cli/tests.rs:1898`) |
| A9, the fake's filter dropped | no engine test has a held snapshot whose `t` lacks the prefix | the fake would sweep what the real `tagged` leaves, and engine tests would pass on behaviour the product does not have | an engine test with the tag on `pool@keep` that the sweep leaves held |
| S2, the no-newline error read again | its case (`cli/tests.rs:2048-2056`) checks the words only, not "(read once)" nor the commands run | it would cost three walks, as before N2 | `ends_with("(read once)")` and the words, as the cases above it do |
| N3m, N3's message undone | the `-r` count's missing-snapshot case (`cli/tests.rs:2961-2980`) checks only that the message lacks "count failed" | the forged cause and its advice would be lost | `contains("names a snapshot that never was")` |

### N6 (nits): docs left stale

- **"after a read again"** (`cli.rs:1212`). The sweep's comment says a
  tag with a tab or a newline is an error "after a read again". A line
  that is not a hold is now read once.
- **"cannot forge the agent's hold, nor hide one"** (`cli.rs:1068-1070`,
  and `cli.rs:1212-1214`, "neither forges the agent's hold nor hides
  one").
  - With R1 accepted, a forged line can pass one read when a real hold
    on its snapshot changes between the two commands.
  - The author says the race "stays as documented", but the code does
    not document it. One sentence in the module's "holds' check" would:
    "unless a hold of that snapshot changes between the two commands
    (accepted, R1 of #165's review)".
- **The engine's sweep doc** (`engine.rs:739-747`). It says the sweep
  releases "each hold [`HOLD_TAG`] in the pool that no publish under way
  names", and that "a hold of the tag is the agent's alone". It now
  releases only those on `@skz-…`. `Zfs::tagged`'s doc says so; the
  engine's does not.
- **`names::PREFIX`** (`names.rs:10-11`). Its doc is "What every name of
  the agent begins with". The sweep now depends on it: a change of
  prefix would leave every hold on an older name unswept. A line saying
  so would guard it.
- **`Record::check`** (`record.rs:400-403`). It says "is not a name the
  agent gives" but checks only the protocol's rule. The sweep's
  narrowing now rests on every recorded name beginning with `skz-`,
  which `fresh_name` alone ensures. It holds today; a check there would
  make it a rule. Optional.
- **A long line.** `hold_lines`' doc (`cli.rs:458`) runs to 93
  columns; rustfmt does not wrap comments.

### N7 (nits): the description

- **"where that snapshot has no marker".** The marker is the agent
  directory's (`record::MARKER`), not a snapshot's. My #165 text was "in
  a directory with no marker, exit 6 names that `t`".
- **The +2682.** It is mostly B1. The real change is +468/−99.
- **The plants** ran against `cargo test -p skyzfs-agent --lib
  zfs::cli` only. They are all killed there. I ran mine against both
  agent crates (question 3).
- **A stale pointer.** "Not done" names `test_agent_record.sh:368` and
  `skzcut@sweep-w`; at 85c4708 that fixture is `skzcut@skz-sweep-w`, at
  line 393. The paragraph is about 2233537, so this is only a reading
  aid.

## Decisions for the lead

1. **L1.** Accept that a held snapshot of the agent's renamed by hand
   keeps its hold, as the ruling accepted a tag put by hand, and have
   it written in `agents_name`'s doc and the engine's sweep doc? I
   would. And a note for M3 that the restore's release (design §5.4,
   "every hold that carries the agent's publication tag") is wider
   than `Zfs::tagged` now is.
2. **A rig case for the other side of the narrowing (for testing).** No
   rig case puts the tag, or a tag with a tab or a newline, on a
   snapshot not named `skz-…` and pins that the sweep leaves it and is
   not stopped. The unit tests show it on the fake only. Not needed to
   merge.

## Gates

On a scratch worktree of 2233537 (the same code as 85c4708), rustc
1.100.0 (`rustup update stable` found nothing newer),
`CARGO_INCREMENTAL=0`, `CARGO_PROFILE_DEV_DEBUG=0`. `cargo fmt --all
--check`: clean.

| Feature set | clippy `--all-targets -D warnings` | `cargo test --workspace` | `cargo doc` (`RUSTDOCFLAGS=-D warnings`) |
|---|---|---|---|
| none | clean | 1968 passed, 0 failed, 16 ignored | clean |
| `skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean | 2026 passed, 0 failed, 16 ignored | clean |
| `skyzfs-ublk-sys/control-test-program` | clean | 1992 passed, 0 failed, 18 ignored | clean |
| `skyzfs-agent-cli/test-hooks` | clean | 1968 passed, 0 failed, 16 ignored | clean |

These match the description's table.

## CI at 85c4708

Read from the head's check runs and job k's log.

| Workflow | Run | Jobs |
|---|---|---|
| `ci` | 45882303937 | test, lint, unit-arm64, chart-lint, s3-backends: success |
| `coverage` | 45882303906 | success |
| `e2e` | 45882303997 | e2e-tier-a: success |
| `k3s-smoke` | 45882303988 | install and UDS smokes: success |
| `skyzfs-ci` (the push) | 45882295967 | gate: success; test, lint and unit-arm64 skipped, since `ci` runs them on the pull request |
| `skyzfs-agent-rig` | 45882303919 | the check's logic; a1, a2, d and **k** on ubuntu-26.04: success |

**Job k** (140124384174) ended "done: worst exit 0": 1049 pins ok, 113
INFO, none failed or in error, and its three controls caught
(`ctl_early` at the snapshot, `ctl_files` at the files, `ctl_manifest`).
The lines that rest on the narrowing:

- `pin_sweep_swept`: ok, "the publish released the hold that no
  publish named"; `pin_sweep_clean`: ok.
- `pin_unmarked_refused`: ok, "a fresh directory with a hold of the
  tag: exit 6, nothing released, no record written";
  `pin_unmarked_outcome`, `_settle`, `_criterion` and `_clean`: ok.
- The 36 K-sweep cases ran, among them kills at `sweep:before:1` and
  `sweep:after:1` (`premise_…_killed` ok, `pin_…_criterion` ok), and
  `obs_points` lists `sweep` as reached. No `premise_sweep_points`
  error.

Against #165's job k (run 45858921309: 1032 ok, 103 cases, worst exit
0), this run has the same cases and two more, `K-assist-assist-before-3`
and `-after-3`: the count of kill points in the assist case varies from
run to run.

### At 2233537, before the rename (for the record)

Job k (run 45874752872) ended "worst exit 1" with 576 ok in 68 cases.
Its only failures were `pin_sweep_swept`, `pin_sweep_clean`,
`pin_unmarked_refused`, `pin_unmarked_outcome` (FAILED) and
`premise_sweep_points` (ERROR, "no K-sweep case ran"). Every one rests
on the rig's stray names, then `@sweep-…` and `@stray-…`, which the
narrowed sweep does not read. Against 85c4708 the difference is the 36
K-sweep cases, those five lines, and two `K-destroy-look-*-2` cases (run
to run). No other pin was lost. Everything else on 2233537 was green.

## The rig's renamed strays against the narrowing (85c4708)

Every hold of `skyzfs-publish` that the agent rig places by itself, at
85c4708 (`git grep` of `agent-record.sh`, `agent_check.py` and their
tests):

| Where | Snapshot | Begins `skz-`? | What the rig expects | Holds with the narrowing? |
|---|---|---|---|---|
| `scen_sweep` (`agent-record.sh:515-518`) | `$POOL@skz-sweep-$id` | yes, whatever `$id` (the case directory's name) | the second publish sweeps it: `pin_<id>_swept`, and the K-sweep kills at `sweep:before/after` | yes. The sweep lists it, reads its holds, finds the tag, and no publish names `skz-sweep-…`, so it releases it and reaches the `Sweep` points. Job k shows it |
| `scen_unmarked` (`agent-record.sh:535-540`) | `$POOL@skz-stray-$id` | yes | a fresh directory with no marker: exit 6, nothing released, no record (`pin_<id>_refused`) | yes. The stray is read as stale and the directory has no marker, so `Error::Unmarked`. Job k shows it |
| the unit fixtures (`test_agent_record.sh:393`, `test_agent_check.py:66` and `:1080`) | `skzcut@skz-sweep-w`, `@skz-stray-u`, `@skz-sweep-sweep` | yes | the scripts' own words and the checker's verdicts | not the agent's code; consistent with the above |
| `test_agent_check.py:517` | `{POOL}@a1` | no | the checker's "clean" pin fails on a tag left at a case's end | not the sweep: the checker reads `holds.txt`, any name |
| the rig's cleanup after each case (`agent-record.sh:409-410`) | every snapshot holding the tag | any | the rig releases each with `zfs release` | not the sweep |
| `test_agent_record.sh:482` (a fake `zfs`) | `skzcut@t1` | no | the cleanup's own unit test | no agent runs |

No stray of the agent rig is named without `skz-` any more. What the
rig does not cover is decision 2 above.

## What was not checked

- Nothing on a real pool by me: no ZFS here. The real-pool evidence is
  job k on 85c4708, on ubuntu-26.04 only; the agent rig runs 24.04 by
  dispatch only, so the narrowing on ZFS 2.2 is not shown (the
  description says so too).
- A tag with a tab or a newline on a real pool (as on #165).
- The plants and scratch tests ran on 2233537, not on 85c4708; the code
  is the same (above).
- I did not re-read the parts of `cli.rs` that this change leaves as
  #165 had them, beyond what the plants touch.

## Files here

- `plants168.py`: the 51 plants, and how they ran.
- `plants168.out`: one JSON line per plant, and the worktree's state
  after.
- `scratch_tests.rs`: the two scratch tests (N4' and L1), appended to
  `crates/skyzfs-agent/src/zfs/cli/tests.rs` on a scratch worktree of
  2233537, run with `cargo test -p skyzfs-agent --lib scratch_ --
  --nocapture`, then removed. Their output:

```
SCRATCH n4 held: Err(Other("zfs holds -H -r tank@skz-1: `tank@skz-1` has 2 lines of holds and 1 holds (`userrefs`): holds were added between the two reads, again and again, or a line is forged by a tag with a newline, on the hold just before one of them (of tank@skz-1); read `zfs holds` of those snapshots and, where a tag holds a newline, release that hold by hand (read 3 times)"))
SCRATCH renamed: Ok({}) words ["zfs list -H -p -r -t snapshot -o name,userrefs tank"]
test result: ok. 2 passed; 0 failed
```

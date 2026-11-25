## Review of #140 at 83b1d9d (M2.6a)

The daemon's side of M2.6: rule (R) in the in-place store (`Cuts::rewound`)
and the refusal `rewound` in the uploader; the channel's verb `look`, with
its own client of object storage; the protocol moved to
`crates/skyzfs-control`; and the M2.5 publish tests that ask across a
restart or a rewind, changed to follow (R). The five commits are `e113977`,
`b5ee13b`, `aabbc45`, `bb318ed` and `4b6b20b`. They are followed by a merge
of `zfs-topology`'s tip `6e060e7`, which changes no file here but
`Cargo.lock`. I formed the findings below from the diff, the design and
OpenZFS 2.4.1's source before I read the description. Then I checked the
description against them.

**Verdict: not ready to merge. Three findings block.**

- **B1. CI is red on the head.** `ci` / lint and `skyzfs-cut-daemon` /
  "the check's logic" both fail `tests/kernel/test_cut_plan.py`,
  `test_every_crate_under_the_uploader`, for `crates/skyzfs-control`. The
  uploader and the daemon now build with that crate, and it is in none of
  the rig's lists. The description names this gap for the lead; the
  repository's own guard turns it into a red run.
- **B2 (F1). Rule (R) does not see `zpool import --rewind-to-checkpoint`.**
  - ZFS writes the checkpointed state under a group *above* every group
    before it, so no cut is marked.
  - A publish pending across that import is then carried by the first cut
    of the checkpointed state, which has no `pool@t`.
  - The store's module docs say a group can only go down or repeat across
    cuts by a rewind. That is not so.
  - Certainty: certain on the ZFS source, and shown on the publish
    fixture; not run on a real pool.
  - The design lets a pool have a checkpoint: §6.2 says the agent
    "reports" one and does not refuse it.
  - Which guard closes this is a decision for the lead (D1). The docs must
    say which before the merge.
- **B3 (F2). A stale mark can publish a request asked after a `-T` rewind
  on a cut of the abandoned history.**
  - The store has already marked the rewind. The uploader learns of the
    mark only at its next check of a cut, and that check waits for the cut
    being uploaded.
  - A request asked in that window, with a count from before the rewind,
    is accepted. It is then given an older cut of the old history that is
    committed or committing.
  - Two scratch tests show this.
  - The module docs and the description say such a request "ends
    `rewound` at the next check". It never does: it ends `done`.
  - Only the agent's rule (L) (M2.6b) would keep it from harm.
  - The text must change before the merge.
  - I recommend a small code fix: put the store's live mark on each
    `publish`, read at the desk, which the handler already asks for
    `cuts`. The lead decides whether (L) is to be the only guard instead
    (D2).

Everything else is as it should be:

- **The rule.** It is what the plan asked for, and the code does what its
  docs say.
  - On my table of fourteen group sequences, rule (R) gives the mark the
    plan wants on every row.
  - The rows: rising groups, `-F`, `-T`, a repeat, one group lower, two
    rewinds, a take-over before and after a rewind, a reboot to a new
    state, and the checkpoint case, which it does not mark (B2).
- **Formats.** The mark is in memory and recomputed at each start. No
  format or version changes.
- **The move.** `bb318ed` is a pure move, apart from the changes the
  description lists.
- **`look`.**
  - It classifies as its docs say. An error of either read is `store`, a
    timeout is `no-answer`, and neither is ever `absent`.
  - Its client is made by the same `connect`, from the same `Remote`
    (`begun.remote()`), as the uploader's client. Endpoint, credentials
    and region are therefore the same, and nothing is asked of the store
    when the client is made.
- **Planted defects.** 34 on the head: 29 were caught and 5 survived
  (table below). Four of the survivors are test gaps (R8a and L4 in N3,
  L8 in N2, W1 in T1). The fifth, M2b, is now unreachable in the daemon
  (N5).
- **CI and Codex.** Every CI job other than the two B1 failures is green,
  the daemon rig included. Codex reports no findings at the head.

### Blocking

#### B1. CI red: `skyzfs-control` is not in the cut rig's lists (certain)

`ci` run 45534918518, job `lint`, and `skyzfs-cut-daemon` run
45534918495, job "the check's logic (no kernel)", fail on the same test:

```
FAIL: test_every_crate_under_the_uploader [crates/skyzfs-control]
AssertionError: False is not true : crates/skyzfs-control
```

What the test does (`tests/kernel/test_cut_plan.py:112`):

- It walks the workspace crates that the uploader, the daemon and node B
  build with.
- It asks that each crate starts the upload job
  (`tests/kernel/cut_plan.py:33`, `UPLOAD_PATHS`).
- It asks that each crate starts the workflow
  (`.github/workflows/skyzfs-cut-daemon.yml:70`, `paths`).

Since `bb318ed`, `skyzfs-control` is a dependency of `skyzfs-upload` and
of `skyzfsd`, and it is in neither list.

The fix is two lines:

- `'crates/skyzfs-control/',` in `UPLOAD_PATHS`;
- `- crates/skyzfs-control/**` in the cut rig's `paths`.

Three other rigs filter on the daemon's crates and do not name
`skyzfs-control` either:

- `.github/workflows/skyzfs-daemon-rig.yml:79`;
- `.github/workflows/skyzfs-control-rig.yml:44`;
- the daemon jobs of `.github/workflows/skyzfs-replay-rig.yml:99` and
  `:161`.

So a change to the protocol alone starts none of them. Whoever owns the
rigs should add the crate there too; the description leaves that to the
lead. The merge needs CI green on the head, so this blocks whoever makes
the change.

#### B2 (F1). A rewind to a checkpoint marks nothing (certain on the ZFS source; not run on a pool)

`zpool import --rewind-to-checkpoint` runs `spa_ld_checkpoint_rewind`
(OpenZFS 2.4.1, `module/zfs/spa.c:5825-5915`):

```c
checkpoint.ub_txg = spa->spa_uberblock.ub_txg + 1;
checkpoint.ub_timestamp = gethrestime_sec();
spa->spa_uberblock = checkpoint;
...
error = vdev_config_sync(svd, svdcount, spa->spa_first_txg);
```

What this does:

- It writes the checkpointed uberblock, with the checkpoint's root block
  pointer, to the labels as group *last + 1* with the current time, so
  that ZFS chooses it.
- The groups after it count up from there.
- The cut this makes has a group above the cut before it, and it opens
  as itself at once.
- Rule (R) (`crates/skyzfs-local/src/inplace/cuts.rs:577-583`) therefore
  does not mark it.

Two texts are false for this rewind:

- the module docs' premise, "A group can only go down or repeat across
  cuts by a rewind or by a group that errs low" (`cuts.rs:91-93`);
- design §5.2's "after a rewind import the pool counts again from an
  earlier group" (line 345), on which (R) rests.

Failing scenario. There is one daemon and no restart, so the count at the
start does not help.

1. The pool has a checkpoint at group 30. Design §6.2 allows this: the
   agent "takes none and reports a pool that has one".
2. The agent reads *C*, takes `pool@t` at group 45, holds it and asks.
   The request is accepted. The cut of group 45 waits behind a slow
   upload.
3. The operator exports, then imports with `--rewind-to-checkpoint`. The
   checkpoint's uberblock is written as group 46, and its cut replaces
   the cut that waits (D7).
4. Cut 46 is counted after *C*, its group is at least 45, and it opens as
   itself, so it carries `t`. `snap/t` now names the checkpoint's state,
   which has no `pool@t` and no hold.

The scratch test `review_f1_checkpoint_rewind_marks_nothing`
(`review140.rs` here) plays this on the publish fixture: cut 1 uploading,
cut 2 of group 45 waiting, then `group_anew(46)` with block 101 put back.

- Result: the store's `(count, rewound)` is `(3, 0)`, and `snap/t` is
  published on group 46 (`scenarios.out`).
- The fixture cannot make a ZFS pool's state, so the test shows only the
  daemon's side: nothing marks the cut and nothing refuses the publish.

What could close it (the lead's decision, D1):

- **In the store.** Mark a cut whose root block pointer was born at or
  below the group of the cut before it.
  - `skyzfs_uberblock::State.root[10]` is the root's logical birth
    (`blk_birth_word[1]`), so the parser needs no change.
  - A normal sync writes a root born in its own group.
  - The first group after `-F` or `-T` has a root born in that group, so
    the group rule marks it already.
  - The checkpoint's root was born at the checkpoint's group, which is
    lower.
  - Still to confirm on the recorded pools, which hold the uberblocks:
    that every uberblock a normal sync writes has a root born in its own
    group.
- **In the agent.** Publish nothing while the pool has a checkpoint,
  checked *after* the snapshot is taken. A checkpoint taken after
  `pool@t` exists holds it, so a rewind to that checkpoint keeps it. This
  changes §6.2's row from "reports" to "refuses".
- **Both.**

Rule (L) (M2.6b) does not cover this: the publish was asked before the
import, so both reads of the load GUID agree.

#### B3 (F2). The stale mark: a request asked after a rewind, with a count from before it, is published on the abandoned history (certain in the fixture)

Where the uploader learns the mark:

- in `Driver::check` (`crates/skyzfs-upload/src/driver.rs:551`);
- at the daemon's start (`bins/skyzfsd/src/upload.rs:1094`).

Why it can lag:

- `check` runs when the stage is `Check`, that is after the cut being
  uploaded is committed.
- `Driver::step` takes a publish's step first whenever one is due
  (`driver.rs:412-418`).
- `Uploader::control` answers `ask` with no call to the store
  (`crates/skyzfs-upload/src/uploader.rs:389-400`).

So while a cut uploads, the uploader's mark can be one rewind behind the
store's, for as long as the upload takes. An upload that fails keeps it
behind longer, since an error from object storage leaves the stage as it
was, to be stepped again.

Failing scenario:

1. Cut 1 (group 30) is committed. The agent reads *C* = 1.
2. Cut 2 (group 51) is counted, and is committed or being uploaded.
3. The pool is exported and imported with `zpool import -T` to group 37.
   The new history's first cut (group 40, below 51) is counted, and the
   store's mark becomes 3.
4. `pool@t` is taken on the new history at group 40 and asked with
   *C* = 1, before the uploader's next check.
5. The uploader's mark is still 0, so the request is accepted. Cut 2
   then carries it (number 2 > 1, group 51 ≥ 40, opens as itself), in
   one of two ways:
   - cut 2 already committed: the lookup finds the name absent, and
     `absent()` gives the request the first known cut that carries it,
     cut 2 (`publish.rs:816-840`);
   - cut 2 still uploading: the request waits, and cut 2's commit hands
     it cut 2 (`committed`, `publish.rs:756-768`).

   Either way the request is `Ready` before the next check, and
   `set_rewound` does not end a `Ready` request.
6. `snap/t` is published on cut 2, of the abandoned history, which has
   no `pool@t`.

Two scratch tests show both paths:
`review_f2a_stale_mark_known_cut_of_the_old_history` and
`review_f2b_stale_mark_cut_being_uploaded`.

- The store says `(count, rewound) = (3, 3)`.
- The ask is `Accepted`.
- `snap/t` is on group 51. The steps are `PublishLooked`, then
  `Published { number: 2 }`, and only then cut 3's check.

Three texts claim otherwise:

- The module docs (`publish.rs:93-95`): "Between two checks the mark here
  can be behind the store's: a request asked then is accepted and ends
  `rewound` at the next check, before any cut after the rewind is
  committed." This is false: the request ends `done`.
- The description: "It is never carried by a cut of the new history,
  since that cut is checked first." True, but beside the point: the
  request is carried by a cut of the *old* history.
- The docs' reason why a publish given its cut before a rewind may go on,
  "as that cut was counted on the history it was asked on"
  (`publish.rs:91-93`). This does not hold for a request asked after the
  rewind.

What keeps it from harm today is only the agent.

- With rule (L), the pool's load GUID is read before *C* and after the
  snapshot. So a snapshot is on the same history as its *C*, and a cut of
  the old history after *C* then holds it.
- So this is defence in depth, not a hole the agent cannot close.
- But (R) exists to make this refusal on the daemon's side, and the store
  already knows the mark when the request arrives.

The recommended fix is small: give the store's mark with each `publish`.

- The handler already asks the desk for `cuts`
  (`bins/skyzfsd/src/control/handler.rs:221-236`, `Desk::cuts_now`,
  `crates/skyzfs-upload/src/queued.rs:254`).
- `CutsNow` could carry `rewound`. The handler could read it before the
  publish and send it in `Control::Ask`, and the uploader would call
  `set_rewound` before `ask`.
- In the plain driver (the tests), `step` could refresh the mark from the
  local store before a publish's step.

Whichever the lead chooses, the text at `publish.rs:91-95` and the
description's paragraph must change.

### Not blocking

**N1. "Asking twice is asking once" no longer holds across a restart.**

- What happens: a request done before a restart, asked again of the next
  daemon, is refused `rewound`. Asked again with a new count, it is
  refused `taken`, because its row records a cut before the new count.
- The agent has to `look` to learn that the snapshot is its own. Commit 5
  pins this (`asked_again_answers_done_and_a_new_uploader_moves_nothing`).
- The description calls it the cost of the count at the start. I agree
  it is safe, and the agent's plan already looks after a `taken`.
- If the lead wants to keep the old property: answer a request below the
  mark from its lookup first, giving `done` where its row records it and
  `rewound` only where the name is absent.

**N2. `look`'s second read is not tested on its own.**

- `find` reads the ref, then the row (`look.rs:86-99`). A failure of the
  row's read alone must be `store`, not "no row".
- The test arms the fault store at the first operation
  (`look/tests.rs:145`).
- So plant L8, which takes a row's read error as no row (and so answers
  `unreadable why=no-row` for a snapshot that is whole), survives.
- One test with the fault at the second read pins it.

**N3. Two pieces of the daemon's wiring have no unit test.**

- Not pinned: the mark at the start (plant R8a, in `serve.rs:1221`) and
  `look`'s volume (plant L4, `serve.rs:912`). Both survive. The
  description says the same.
- The rigs do not ask `look` yet.
- A daemon test would pin both: start a store with a count, ask on the
  channel a request below it, and `look` on a volume with a published
  snapshot.
- R8a matters only until the first check, but with B3 that can be long.

**N4. The status file's `Cuts` reads `rewound: 0`**
(`crates/skyzfs-ublk/src/status.rs:1278-1280`). A reader cannot tell that
from "no rewind". Nothing reads it today. An `Option`, or a type of the
status's own, would keep it from being read wrong later.

**N5. Commit 5 keeps each test's point, with one shift worth recording.**

Plants on the old and the new `tests/publish.rs` alone, so that lib unit
tests do not stop the run first:

| Plant | Base `6e060e7` | Head `83b1d9d` |
|---|---|---|
| C1 `itself` not required | KILLED: `a_cut_before_c_or_not_opening_as_itself_is_not_published` | KILLED: only the new `a_cut_not_opening_as_itself_in_a_new_cut_state_is_not_published` |
| C2 the row's commit not compared | KILLED: `a_ref_made_anew_beside_a_row_left_behind_is_not_ours` | KILLED: the same test (its same-uploader half) |
| C3 the manifest not compared | KILLED: `a_name_taken_is_refused_and_the_cuts_go_on` | KILLED: the same |
| C4 `>` C made `>=` | KILLED (3 tests) | KILLED (6 tests) |
| M2 the check's hand-off removed (and `not_offered` with it) | KILLED (4 tests, the crash matrix among them) | KILLED (3 tests, by `not_offered`) |
| M2b the check hands a cut found committed to no publish, `not_offered` still counted | KILLED: `a_failure_at_any_operation_leaves_no_ref_or_a_whole_one` | **SURVIVED**, also against every test of `skyzfs-upload` and `skyzfsd` |
| M7 a cut found committed offered on head though head moved (F1 of #122) | KILLED (4 tests) | KILLED (4 tests) |

What changed with M2b:

- On the base, the crash matrix's restart half was the one test in which
  a new uploader found cut 1 committed at its check and gave it to the
  publish.
- On the head, that path cannot carry a publish in the daemon. Every
  request the new uploader accepts has *C* at or above the count at the
  start, which is at or above the number of any cut it finds committed.
  So M2b is an equivalent mutant there, and its survival is not a test
  gap. This is my reading of the code, not a test.
- The description could say that the hand-off at the check now serves
  only the figure `not_offered`, and the driver used without the start
  mark.

The tests whose point was F1 of #122 (`a_cut_found_committed_*`,
`head_at_another_commit_*`): after a restart, the count read anew already
keeps the publish off cut 1. The author kept them meaningful by asserting
`not_offered`, and M7 is still caught by four of them.

**N6. Design §5.2's sentence**, "after a rewind import the pool counts
again from an earlier group", is not general (B2). It is the lead's to
correct.

**T1. The client's parse of `snap` is not pinned for `number` against
`group`.**

- Plant W1 swaps them in `Reply::parse` (`crates/skyzfs-control/src/lib.rs:626-627`)
  and survives `skyzfs-control`'s and `skyzfsd`'s tests.
- The agent will compare these two fields with its request, so a line
  test that parses a `snap` line with different values for the two
  pins it.

**T2. Scenarios the tests lack:**

- B2 and B3 (`review140.rs` here);
- a rewind while a cut uploads, with a publish asked after it;
- a checkpoint rewind.

### Decisions for the lead

- **D1 (B2).** Which guard closes the checkpoint rewind:
  - the store's root-birth rule;
  - the agent's refusal of a pool with a checkpoint, checked after the
    snapshot (design §6.2 then changes from "reports" to "refuses");
  - or both.

  Until then, the module docs of `cuts.rs` and `publish.rs` should say
  that (R) does not see it.
- **D2 (B3).** Either the daemon's refusal is exact at the ask (the live
  mark sent with each `publish`), or the agent's (L) is the guard for a
  request asked after a rewind with an old count. The docs follow the
  choice.
- **D3 (B1).** Who adds `skyzfs-control` to `cut_plan.py` and to the
  rigs' path filters: coding in this pull request, or testing. It blocks
  the merge either way.
- **The author's questions.**
  - `rewound` and `store` as new words in version 1 of the protocol: no
    agent exists yet, so no client is broken. I see no reason against.
  - Q1, the count at the start: its cost is N1, plus one more group for
    a publish pending across a restart. It is safe.

### What I did

- **Gates.** Run in a worktree of `83b1d9d`, rustc 1.100.0 (stable after
  `rustup update`):
  - `cargo fmt --all -- --check`: clean.
  - `cargo clippy --workspace --all-targets -- -D warnings`: clean. Also
    clean with `--features skyzfsd/test-hooks,skyzfs-local/test-hooks`
    and with `--features skyzfs-ublk-sys/control-test-program`.
  - Tests of the touched crates (`skyzfs-control`, `skyzfs-local`,
    `skyzfs-upload`, `skyzfsd`, `skyzfs-ublk`): 829 passed, 0 failed,
    1 ignored.
  - `skyzfsd` and `skyzfs-local` with their test hooks: 513 passed,
    0 failed, 1 ignored.
  - I did not run the whole workspace (disk), so I have not checked the
    description's workspace totals.
- **The move (`bb318ed`).**
  - I compared the multiset of lines removed from `bins/skyzfsd` with the
    lines added in `crates/skyzfs-control`, and read
    `git show --color-moved`.
  - The only lines that did not move are those the description lists:
    - `read_line` made public;
    - `connect_counted` visible to the crate's tests;
    - "module docs" changed to "crate docs";
    - the link on `valid_name`;
    - `EXPIRY_DEFAULT`'s doc;
    - `Code::of` and `Reply::of` turned into `code_of` and `reply_of`;
    - the crate's opening paragraph;
    - the client put under `cfg(target_os = "linux")`.
  - The constants have the same values, and `valid_name` is identical.
  - The same 35 test names exist before and after: 27 in `skyzfsd` and
    8 in `skyzfs-control`. The helpers are copied.
- **The scratch tests** are in `review140.rs` here; their output is in
  `scenarios.out`.
  - They ran in the head's worktree as
    `crates/skyzfs-upload/tests/review140.rs`, and I removed them after.
  - What they cover: F1 (B2), F2a and F2b (B3), and the table of rule
    (R).
  - The table's fourteen rows all give the mark I expected, the
    checkpoint row included (no mark, which is B2).
- **Plants.**
  - Each plant is a textual change in a worktree, made one at a time,
    with the file restored in `finally` and a 20-minute limit per suite
    (`plants140.py` here).
  - Suites: `l` = `cargo test -p skyzfs-local`; `u` =
    `-p skyzfs-upload`; `d` = `-p skyzfsd` with the test hooks; `c` =
    `-p skyzfs-control`; `p` = `-p skyzfs-upload --test publish`.
  - A suite stops at its first failing test binary, so a plant is listed
    with the tests of that binary that failed.

| # | Plant | Head |
|---|---|---|
| R1 | the driver gives no mark at the check | KILLED (u): `a_cut_before_c_or_not_opening_as_itself_is_not_published`, `a_publish_asked_before_a_rewind_by_f_is_not_carried_by_its_first_cut` |
| R2 | no refusal at the ask | KILLED (u): `a_request_asked_after_a_count_below_the_rewind_mark_is_refused` |
| R3 | the mark goes back | KILLED (u): the same |
| R4 | a publish given its cut is ended too | KILLED (u): `a_publish_given_its_cut_before_a_rewind_goes_on` |
| R5 | publishes ended at the mark itself | KILLED (u): 2 tests |
| R6 | a request refused at the mark itself | KILLED (u): 9 tests |
| R7 | the store offers no mark | KILLED (u): the two of R1 |
| R8a | the daemon's start reads no mark (`serve.rs`) | **SURVIVED** (d) (N3; the description says so) |
| R8b | the daemon's start gives the uploader no mark (`upload.rs`) | KILLED (d): `the_thread_answers_the_channel_for_its_attachment_and_state` |
| R9 | `REWIND_UNSEEN` by hand | KILLED (l): 4 tests, the recorded rewinds among them |
| R10 | `REWIND_START_FORGOTTEN` by hand | KILLED (l): `a_take_over_takes_the_count_it_starts_with_as_a_rewind` |
| M1 | an equal group is no rewind (`<` for `<=`, the `-F` case) | KILLED (l): 4 tests |
| M2 | the check's hand-off removed | KILLED (u): 3 tests |
| M2b | the check hands a found cut to no publish | SURVIVED (u, d): equivalent in the daemon (N5) |
| M3 | the mark checked before the same request pending | KILLED (u): `a_publish_given_its_cut_before_a_rewind_goes_on` |
| M4 | the mark at a start one below the count | KILLED (l): `a_take_over_takes_the_count_it_starts_with_as_a_rewind` |
| M5 | the mark one below the rewind's cut | KILLED (l): 4 tests |
| M7 | a found cut offered on head though head moved | KILLED (p): 4 tests |
| L1 | another commit's row read as ours | KILLED (u): 2 `look` tests |
| L2 | no row read as absent | KILLED (u): 2 `look` tests |
| L3 | an error answered as absent | KILLED (d): `a_look_that_object_storage_fails_reads_store` |
| L4 | another volume looked up (`serve.rs`) | **SURVIVED** (d) (N3) |
| L5 | number and group swapped in the answer | KILLED (d): 2 tests |
| L6 | version read as length | KILLED (d): `a_looks_answer_keeps_what_was_seen` |
| L7 | `look`'s name not checked | KILLED (d): `a_request_that_cannot_be_taken_is_refused_without_asking` |
| L8 | the row's read error taken as no row | **SURVIVED** (u, d) (N2) |
| L9 | no answer in time read as `store` | KILLED (d): `a_look_that_object_storage_does_not_answer_reads_no_answer` |
| L10 | the ref read but not the row | KILLED (u): 2 tests |
| W1 | the client parses `group` as `number` | **SURVIVED** (c, d) (T1) |
| W2 | `rewound` answered as `taken` | KILLED (d): `a_publishs_refusal_keeps_its_own_code` |
| C1-C4 | commit 5's (above, N5) | KILLED (u, p) |

  - The author's sample of eight plants for rule (R) is R1-R8b above; all
    are caught except R8a, as the description says. The author's sample
    for `look` is L1-L7; all are caught except L4, which the description
    also lists as not pinned.
- **CI at the head** (`83b1d9d`):
  - green: `skyzfs-ci`, `e2e`, `k3s-smoke`, `coverage`,
    `skyzfs-control-rig`, `skyzfs-nodeb-rig` and `skyzfs-replay-rig`;
  - the three kernel jobs of `skyzfs-cut-daemon` (ubuntu-26.04,
    ubuntu-26.04-arm, ubuntu-26.04-upload);
  - `skyzfs-daemon-rig` on ubuntu-24.04, 26.04 and 26.04-arm. In its log
    the pins read PASS, the held-request pin reads WAITING, and the `ctl_`
    lines are its controls. The job summary's table cannot be read
    through the REST API, so I read the log's lines instead;
  - `ci` / test, unit-arm64, s3-backends and chart-lint.
  - Red: `ci` / lint and `skyzfs-cut-daemon` / "the check's logic", both
    from B1.
- **Codex.** The summary comment at `83b1d9d` reports no findings, and
  there are no review threads.

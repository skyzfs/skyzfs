## Review of #137 at bec9a2f (M2.T6: the cut rig on rows of version 2 and 1, node B's codec against the uploader's)

**Verdict: not yet. One finding blocks the merge: B1.**

**B1: the pull request's upload job runs nothing.**
- The workflow's check of recording names takes letters only: `[[ $r =~ ^[a-z]+$ ]]` (`.github/workflows/skyzfs-cut-daemon.yml:217`).
- `uv1` and `uv1moved` have a digit. `cut_plan.PR_UPLOAD_RECORDINGS` now names them (`tests/kernel/cut_plan.py:32`).
- So the job "record through skyzfsd and check, ubuntu-26.04-upload" ends with `not a recording: uv1`, exit 64, before it records anything. This is run 45512357623, job 138863068225.
- **Neither new course has run on a kernel.** Nor have the six courses that ran before (life, umoved, urow, ukill, ulease, ulost). So this head's change to the content check is not shown on a kernel either: the eighth field of `uploaded.tsv`, `row_commits_off_head`, and `read_row`'s commit.
- **After a merge, every pull request that touches the upload paths would get a red upload job that records nothing.** The uploader would lose its kernel gate.
- The fix is one line: widen the pattern to `^[a-z][a-z0-9]*$`, or rename the two courses to letters only. Then the job must run, and its course lines must be read.
- **Certainty: high.** I saw it in the job's log.

**The rest holds up, as far as it can be judged without a kernel run:**
- **What the courses check.**
  - The two courses check what F1 and the review of #134 say should happen.
    - In uv1, a version 1 row with head's group the row's reads `row_v1`. That holds whether head stayed or moved within the group.
    - In uv1moved, a move to another group reads `head_moved`, and nothing else passes.
  - The safety half is pinned exactly: no delta goes up under a version 1 row.
    - uv1 wants 2 mismatches and 3 whole uploads, exactly.
    - uv1moved wants `uploaded` unchanged after the act, and 1 whole upload.
    - The content check covers node B's 0x5a block under the whole cut.
- **`move-head --row` is atomic.** The row op rides in the same `commit_with` transaction as head's swap. The only form the rig uses, `v1:`, writes exactly the bytes a daemon from before F1 writes.
- **G3 compares node B with the real `skyzfs_upload::row`, not with a copy.** All seven plants on `row.rs` are killed by the G3 test itself. Eight of the nine plants on node B's codec are killed by it. The ninth is killed by three other tests of node B.

**Not blocking:**
- N1: uv1 accepts `matched` at a step with no check that the whole cut was the reason. It cannot then tell `row_v1` from `head_moved`.
- N2: `move-head --row` with no guard exits 6 on a lost head swap.
- N3: a refusal message.
- D1: the description is stale.
- Test gaps G-a to G-e.

**Which findings block: B1 only.**

**How I worked.**
- I read `git diff 69837ba...bec9a2f` first: 11 files, +644 −75.
- Then I read:
  - the course functions around it in `cut-record.sh` (`other_act`, `before_other`, `guarded`, `nudge_until`);
  - `upload_lines`, `daemons_fetches` and `course_wants` in `cut_daemon_check.py`;
  - node B's `move-head`, `set-row` and its codec;
  - the uploader's `row.rs`, `check.rs` and the driver's check;
  - the engine's `commit_with`;
  - my notes on #134 (review/m2.f1/review.md and next_look.md).
- The description and the CI came after that.
- **Plants:**
  - The Rust plants ran in a clean worktree of bec9a2f, each alone, against `cargo test -p skyzfs-nodeb --no-fail-fast` (2400 s limit), and each was restored in a `finally`.
  - The Python plants ran the same way against the four touched test files of `tests/kernel` (900 s limit each), in four worktrees side by side.
  - Every worktree was clean afterwards.
  - One serial run of the Python plants was stopped by hand in the middle. Its planted file was restored with `git checkout` and the whole set was run again; the results below are from that second run.
- I posted nothing on GitHub.

**Files in this folder:**
- `review.md`, this note;
- `plants_rs.py`, `plants_rs.out`, `plants_rs.json`, `plants_rs2.out`, `plants_rs2.json`: the Rust plants (NB, RW, MH);
- `plants_py.py`, `plants_py.out`, `plants_py.json`: the Python plants (PY);
- `gates.out`: fmt, clippy, the tests;
- `upload_job_bec9a2f.txt`: the end of the failed upload job's log.

### Blocking

**B1. The workflow refuses the names `uv1` and `uv1moved`, so the upload job records nothing** (`.github/workflows/skyzfs-cut-daemon.yml:217`, with `tests/kernel/cut_plan.py:32` and `tests/kernel/cut-record.sh:168`).

**What happens.**
- The step "Record through skyzfsd and check" first checks every name it is given against `^[a-z]+$`.
- The plan gives the upload job `life umoved urow ukill ulease ulost uv1 uv1moved`.
- `uv1` fails the check, and the step ends with exit 64. `cut-record.sh` never starts.
- The job's summary step then prints an empty table: `cut-out/` holds only `setup-apt.log`, the one file the artifact has.
- This is in `upload_job_bec9a2f.txt`:
  ```
  ##[error]not a recording: uv1
  ##[error]Process completed with exit code 64.
  ```

**`cut-record.sh` itself takes the names.**
- It checks each name against `KNOWN` (`cut-record.sh:188`), which holds `UPLOAD_ONLY` (`:168`).
- So a dispatch with `recordings` empty would run both courses, since no name passes through the workflow's check. A dispatch that names either course fails as the pull request did.
- `skyzfs-cut-rig.yml:96` and `skyzfs-nodeb-rig.yml:119` have the same pattern. Their recordings have no digit today.

**What the failure costs:**
- The courses' first runs, which the description counts on ("The courses' first runs on a kernel are this pull request's upload job"), did not happen. Nothing the description says of uv1 and uv1moved on a kernel is shown, nor how long they take.
- The six courses that ran on #134 did not run either. This head changes code they all go through:
  - the watcher's key and its eighth field;
  - `read_uploaded`;
  - `read_row`'s `row_commit`;
  - `daemons_fetches`'s skip;
  - the new `row_commits_off_head` in `upload_content`.
- Once merged, every pull request that touches `UPLOAD_PATHS` (the uploader, `skyzfsd`, `skyzfs-local`, the manifests, the rig's upload files) gets this red job with no course in it.

**The colour is right, the content is not.** The job is red, so nothing passes silently. But it is red for a reason that says nothing about the daemon.

**Nothing local catches it.**
- `test_cut_plan` pins the string `PR_UPLOAD_RECORDINGS` (`test_cut_plan.py:108`), not that each name passes the workflow.
- `test_cut_record.sh` does not read the workflow.

**Fix, either of:**
- `[[ $r =~ ^[a-z][a-z0-9]*$ ]]` at `skyzfs-cut-daemon.yml:217`. The other two workflows may keep theirs or take the same.
- Names of letters only, for example `uold` and `uoldmoved`, everywhere the courses are named.

**A test would keep it from coming back.** In `test_cut_plan`, read the pattern from the workflow file and check every name in `PR_UPLOAD_RECORDINGS`, `cut-record.sh`'s lists and the dispatch input's description against it.

**Then run the upload job and read its course lines.** The questions below that only a kernel answers are under "What the first kernel run must show".

**Certainty: high** (seen in the job's log). **Blocks:** yes. Without it, this pull request adds two courses that cannot run, and takes away the six that could.

### The questions asked

#### (1) The courses uv1 and uv1moved

**What F1 says** (my notes on #134):
- `row_v1` only where a version 1 row names the cut's base and head's group is the row's;
- `head_moved` where the group moved;
- no delta ever goes up under a version 1 row.

**What the courses do** (`cut-record.sh:1126-1152`, `:1202-1221`; `cut_daemon_check.py:1847-1913`):

| Course, step | Node B's act | What the judge wants of the step | Safety pinned by |
|---|---|---|---|
| uv1, `v1` | `set-row --row v1:S:N:G --expect-row S:N:G`. Head is not touched. | Before the act, the daemon's row is of version 2, names head's commit, and is the course's state; others.tsv has `v1 S N G`. Once the mismatch is counted, `upload_checked` is `row_v1` or `matched`. | `upload_mismatches` exactly 2 and `uploaded_whole` exactly 3 at the end; the end row is a later cut of the state, of version 2, naming head's commit |
| uv1, `v1in` | `move-head --at 0 --group G --row v1:S:N:G --expect-row S:N:G`, in one transaction | the same | the same, and `upload_content`: the whole cut goes up onto node B's commit, whose block 0 is 0x5a |
| uv1moved | `move-head --at 0 --group G+1000 --row v1:S:N:G --expect-row S:N:G` | Before the act: the same as uv1. After it: `upload_checked` must be `head_moved`, nothing else. At the end: head is node B's, the row is node B's version 1 row over the same cut, and `upload_checked head_moved`. | `uploaded` at the end equals `uploaded` before the act, `uploaded_whole` 1, and `STOPPED_AT`: the last cut fetched is the one before the act |

**Against F1:**
- **`row_v1` where head's group is the row's.** Both steps of uv1 are that case:
  - `v1` is the upgrade's own case, where nothing moved;
  - `v1in` is the move inside the group, which a version 1 row cannot show.
  
  The judge wants `row_v1` at both, or `matched` (N1).
- **`head_moved` where the group moved:** uv1moved, and it is pinned strictly. Plant PY9 (also accept `row_v1`) is killed.
  - The daemon's own guard is pinned by the daemon's tests. A kernel run of uv1moved would add the kernel's word for it: #134's plant RVG would read `row_v1` there and fail the course.
- **No delta under a version 1 row.** This is pinned by the counts, not by the status word:
  - A daemon that let the delta up (#134's plant V1D) counts no mismatch at that step. uv1 then ends with fewer than 2 mismatches and 3 whole uploads.
  - In `v1in`, such a delta would also go up onto node B's 0x5a block, and `upload_content` would fail.
  - A daemon that refused the whole cut (V1W) would count more than 2 mismatches and fewer than 3 whole uploads.
  - In uv1moved, any upload after the act raises `uploaded` and fails.
  - Plants PY3 and PY4 (the counts as "at least 1") are killed.
- **Not in a course:** a version 1 row that names another cut, which must read `not_the_base` and not `row_v1`. The crate's tests and #134's plant QN cover it. No rig course does, and it is not asked for.

**Is accepting `matched` sound?**
- **For safety: yes.**
  - The status is one snapshot. `matched` with `upload_mismatches` at the step's count means a later check, of the cut after the mismatch, passed.
  - That cut can only be the whole cut. A delta held at the mismatch was abandoned, and the next cut is whole.
  - A wrong outcome there (a delta, or no whole upload) shows in the counts and the content.
- **For the word: no.** See N1. Where the read lands after the whole cut's check, the step passes whatever the mismatch's word was: `row_v1`, `head_moved` (the word at 75bed3f, before N3), `not_the_base`, anything.
  - In `v1in` that is the whole point of the step. The review of #134 asked for it precisely because the word is all that tells this case.
  - How likely is it? urow has the same shape: a mismatch, then a whole cut. In the three upload runs on #134 (4595f55, 75bed3f, 7989f5e), its two steps read the mismatch's own word all six times, `not_the_base`, and never `matched`. So the word will probably be seen. But the judge would pass a run where it was not, and the line's PASS would not say so.

**Could a course pass vacuously?**
- **The other writer's act:**
  - others.tsv gets `v1 S N G` only after node B ended with 0 and printed `row_commit none` (`v1_said`).
  - The judge wants that line for the very cut the daemon's row named before each act (`first['cut'] not in v1`). PY5 is killed.
  - What `v1_said` checks is node B's own print, which comes from `--row`'s argument and not from the store. A node B that printed it and wrote nothing would show in the counts.
- **The mismatch counted, and the whole cut checked:** both are pinned by the exact counts.
- **v1in's move happened, within the group:**
  - The group is the row's by construction: `act_v1in` passes `$3` twice.
  - The judge never looks at others.tsv's `head` line for uv1. So if `move-head --row` left head where it was, `v1in` would turn into a second `v1`, and uv1 would still pass.
  - Node B's unit test pins that head moves (`move_head_writes_the_row_in_the_same_commit`). This is gap G-d.
- **uv1moved:** the move is pinned (`end['head'] in heads`, PY10 killed), and so are the version 1 row at the end (PY11) and the strict word (PY9).
- **The look before each act** is `before_other`, unchanged. It waits for the watcher to fetch the daemon's cut under that head, so `STOPPED_AT` and the skip have a fetch to work on.

**The EXPECTED table** (`cut_daemon_check.py:239-252`):
- uv1 and uv1moved join `RECS` and `UNKNOWN`. So:
  - the six list controls and `early` are `caught`, pinned by `Expected.test_the_table`;
  - `mark-dropped` and `mark-kept` are `same`, since the courses have one instance;
  - `txg-alone`, `count-repeats` and `previous` accept either result, as for every upload course before its first run.
- On a kernel the judge fails a control that differs from its entry in either direction.
- The python tests pin only the first group:
  - PY24 (uv1 out of `RECS`) is killed.
  - PY21 (`mark-kept` caught on uv1) and PY22 (uv1 and uv1moved out of `UNKNOWN`) survive.
  - PY23 (`mark-dropped` caught on uv1moved) is killed, by uv1moved's end-to-end test.
  - uv1 has no end-to-end test, so its mark rules are pinned only on a kernel. The three rules accept either result for every upload course, so they are not pinned for any of them. That is not new. This is G-e.

**The first kernel run** (run 45512357623; the other runs on the head are under "CI"):
- **The upload job, which runs every course this pull request adds or changes, shows nothing.** It ended before the rig started (B1). There is no upload line, course line or control line on this head.
- **The two other jobs of the run, ubuntu-26.04 and ubuntu-26.04-arm, are green.**
  - They hold cuts and do not upload. Their lines (cuts, `held_content`, takeover, failed and the rest) do not go through the code this pull request changes.
  - They show only that the shared parts of the check still load and judge as before.

#### (2) Node B's `move-head --row`

**Atomic: yes.**
- `move_head_on` puts the row op in the ops of `Engine::commit_with`.
- The engine sends one `transact` holding head's `CasRef` (from the engine's head to the new commit) followed by the ops (`crates/skynbd-engine/src/lib.rs:1802-1830`; `commit_with` is `:1630`).
- The ops are:
  - with a guard, `MetaCas` from the bytes node B read to `--row`'s bytes;
  - without one, `MetaPut`.
- So head and the row move together, or neither does.

**The guard:**
1. Node B first reads the row (`expected_row`). A row that does not hold ends with 6 before anything is written.
2. Then the engine opens at head.
3. A daemon's commit between those reads moves the row, and the `MetaCas` conflicts (`OpConflict`, exit 6).
4. A commit after the open moves head, and the `CasRef` conflicts (`Conflict` under a guard, exit 6).

**What a failed write leaves.**
- On a conflict, head and the row are untouched. The segment, the nodes and the commit object already PUT are orphans, as after any failed commit; M4's garbage collection is meant for them.
- Any other error is exit 2, the rig's error.
- The engine's doc says a lost reply or an S3 retry of a landed conditional PUT can read as `Conflict` (`lib.rs:1622-1629`). Node B would then say exit 6 for an act that landed, and the rig would act again on the version 1 row it then reads. This is the same as for `set-row` and plain `move-head` since #119, not new here. It would leave two node B heads, and only the second goes into others.tsv.

**Forms.**
- **Taken:** `v1:`, `hex:` and `STATE:NUMBER:GROUP:COMMIT`.
- **Refused:** `STATE:NUMBER:GROUP`, which names the head being written, and `none` (N3 on the message). The test pins both refusals, and MH4 is killed.
- **Not pinned:** that `v1:` is taken by the command line. MH3, which refuses `v1:`, survives. On a kernel it would be a rig error (exit 2), not a verdict.

**Can it write a row the daemon's writer could not, and make a course meaningless?**
- **Yes, it can write such rows:**
  - `hex:` writes any bytes;
  - `STATE:NUMBER:GROUP:COMMIT` writes a version 2 row naming a commit that is not the one its swap installs, which the uploader never does.
- **The rig uses neither.** It uses only `v1:`, and those bytes are exactly a daemon's from before F1:
  - the version 1 layout has no free byte (2 + 16 + 8 + 8);
  - G3 shows the uploader decodes node B's version 1 row to the same cut and no commit;
  - the uploader's decoder of version 1 is the one from before F1.
- **The states the courses build are ones a real deployment can meet:**
  - `v1`: a version 1 row naming the daemon's cut, with head at the commit of that cut. That is the state after an upgrade.
  - `v1in`: a version 1 row naming the daemon's cut, with head at another commit of the row's group. That is n2 under a row from before F1: another writer, an `import -T`, or a rollback to a snapshot of the same group, before the first whole cut after the upgrade.
  - uv1moved: the same, with head in another group.
- Writing the row with the move, rather than in two steps, is only a way to keep the daemon from cutting between them. The state reached is the same.
- **So the courses are not made meaningless by node B.**
- One thing node B cannot do is write a version 2 row naming its own new commit, because it does not use `commit_naming`. No course needs it.

#### (3) G3: node B's codec against the uploader's

**It uses the real code.**
- `skyzfs-upload` and `skyzfs-local` are dev-dependencies of node B, both workspace path dependencies (`bins/skyzfs-nodeb/Cargo.toml`).
- The test calls `skyzfs_upload::row::encode`, `row::decode`, `row::ROW_LEN` and `row::ROW_V1_LEN` (`main.rs:923-989`).
- It runs in CI with the workspace's tests.

**What it compares:**
- **Version 2, both ways:** node B's reading of the uploader's row, field by field, commit included; node B's `encode_row` byte for byte against `row::encode`; and `row::decode` of those bytes.
- **Version 1:** the uploader reads node B's row as the same cut and no commit. Node B's reading of a version 1 row is checked only for "no commit" (`:970-971`; G-a).
- **The lengths:** both constants.
- **Acceptance:** 10 versions × 81 lengths (0 to 80) of fixed bytes. One accepts exactly what the other accepts.
  - The fields of accepted bytes are not compared there. Given the field comparisons above and the fixed layout, I do not count that as a gap.

**Plants:**

| Plant | Where | What is planted | Result |
|---|---|---|---|
| NB1 | node B | version 1 read at any length from 34 | KILLED by G3 (and `the_cut_row_is_read_as_the_uploader_writes_it`) |
| NB2 | node B | the number and the group read at each other's offsets | KILLED by G3 and 3 others |
| NB3 | node B | the commit read from bytes 2..34 | KILLED by G3 and 2 others |
| NB4b | node B | the version written big-endian | KILLED by G3 and 4 others |
| NB5 | node B | version 2 read by its low byte alone | KILLED by G3 alone |
| NB6 | node B | the number and the group swapped for version 1 only | KILLED, **not by G3**: by `an_expected_row_holds_for_its_cut_in_either_version`, `the_cut_row_is_read_as_the_uploader_writes_it`, `the_guard_holds_the_row_as_read_in_either_version` (G-a) |
| NB7 | node B | every row labelled version 2 | KILLED by G3 and 4 others |
| NB8 | node B | version 2 is 67 bytes | KILLED by G3 and 3 others |
| NB9 | node B | version 3 also read as version 2 | KILLED by G3 and 1 other |
| RW1 | `row.rs` | a row longer than its version says is read | KILLED by G3 alone |
| RW2 | `row.rs` | the group written before the number | KILLED by G3 alone |
| RW3 | `row.rs` | `ROW_VERSION` 3 | KILLED by G3 alone |
| RW4 | `row.rs` | the commit read from bytes 33..65 | KILLED by G3 alone |
| RW5 | `row.rs` | the version read big-endian | KILLED by G3 alone |
| RW6 | `row.rs` | a version 1 row read with a commit of zeros | KILLED by G3 alone |
| RW7 | `row.rs` | any version from 2 read as version 2 | KILLED by G3 alone |

- "By G3 alone" means G3 is the only test of node B that fails. The uploader's own tests would catch the `row.rs` plants too, and I did not run them, since what was asked is whether G3 holds the two together.
- The plant listed first as NB4 was not applied: its text occurs twice in `main.rs`. NB4b is the same plant with a unique anchor.

**move-head --row plants:**

| Plant | What is planted | Result |
|---|---|---|
| MH1 | under a guard, `--row` dropped (the row kept) | KILLED: `move_head_writes_the_row_in_the_same_commit` |
| MH2 | (my first attempt at the non-atomic plant; it also dropped the guard of `Some(None)`) | KILLED by `the_other_writer_acts_on_the_row_it_read`, for that side effect only; replaced by MH2b |
| MH2b | head committed under the guard alone, the row put in a second transaction after (not atomic) | **SURVIVED** (G-c) |
| MH3 | the command refuses `--row v1:` | **SURVIVED** (G-c; on a kernel, a rig error) |
| MH4 | the command takes `--row STATE:NUMBER:GROUP` | KILLED: `move_head_writes_the_row_in_the_same_commit` |
| MH5 | with no guard, no row written | KILLED: the same |
| MH6 | the guard dropped (`MetaPut` whatever the row) | KILLED: 3 tests |
| MH7 | `guarded` true only for a `MetaCas` (N2's fix) | SURVIVED: neither behaviour is pinned (N2) |

**The judge, the watcher and the plan** (`plants_py.py`; each plant against `test_cut_daemon_main`, `test_cut_daemon_check`, `test_cut_watch` and `test_cut_plan`). 26 of 28 are killed:

| Plant | What is planted | Result |
|---|---|---|
| PY1 | uv1: a step that reads head_moved passes | KILLED: `test_each_courses_files` |
| PY2 | uv1: matched no longer accepted at a step | KILLED: `test_each_courses_files` |
| PY3 | uv1: upload_mismatches at least 1, not exactly 2 | KILLED: `test_each_courses_files` |
| PY4 | uv1: uploaded_whole at least 1, not exactly 3 | KILLED: `test_each_courses_files` |
| PY5 | uv1: no check that others.tsv has the step's row of version 1 | KILLED: `test_each_courses_files` |
| PY6 | uv1: no check that the end row names head's commit | KILLED: `test_each_courses_files` |
| PY7 | uv1: no check that the course stays in one state | KILLED: `test_each_courses_files` |
| PY8 | uv1: no check of the row before each act (version 2 naming head) | KILLED: `test_each_courses_files` |
| PY9 | uv1moved: row_v1 accepted once the mismatch was counted | KILLED: `test_each_courses_files` |
| PY10 | uv1moved: no check that head is the other writer's at the end | KILLED: `test_each_courses_files` |
| PY11 | uv1moved: no check of the row at the end | KILLED: `test_each_courses_files` |
| PY12 | uv1moved: the end status wants row_v1 | KILLED: `test_a_course_that_stops_the_uploads`, `test_each_courses_files` |
| PY13 | row_commits_off_head finds nothing | KILLED: `test_a_rows_commit_against_head`, `test_what_an_upload_gets_wrong` |
| PY14 | row_commits_off_head does not leave out another writer's head or row | KILLED: `test_a_rows_commit_against_head` |
| PY15 | daemons_fetches skips a cut in v1 whatever the fetch's commit | KILLED: `test_a_course_that_stops_the_uploads`, `test_a_row_of_version_1_over_the_daemons_cut` |
| PY16 | daemons_fetches does not skip the row of version 1 | KILLED: `test_a_row_of_version_1_over_the_daemons_cut` |
| PY17 | read_uploaded drops the commit | KILLED: `test_a_course_that_stops_the_uploads`, `test_a_rows_commit_against_head`, `test_what_an_upload_gets_wrong` |
| PY18 | read_row drops node B's row_commit | KILLED: `test_a_course_that_stops_the_uploads`, `test_each_courses_files`, `test_the_rows_commit_node_b_prints` |
| PY19 | STOPPED_AT without uv1moved | KILLED: `test_a_course_that_stops_the_uploads` |
| PY20 | uv1's second step reads the first step's status | KILLED: `test_each_courses_files` |
| PY21 | EXPECTED: mark-kept caught on uv1 (a single instance) | SURVIVED |
| PY22 | EXPECTED: uv1 and uv1moved out of UNKNOWN (the three rules pinned same) | SURVIVED |
| PY23 | EXPECTED: mark-dropped caught on uv1moved | KILLED: `test_a_course_that_stops_the_uploads` |
| PY24 | RECS without uv1 (no row of EXPECTED) | KILLED: `test_the_table` |
| PY25 | the watcher's key leaves out the row's commit | KILLED: `test_the_rows_commit_is_written_with_its_cut` |
| PY26 | the watcher writes none for every commit | KILLED: `test_the_rows_commit_is_written_with_its_cut` |
| PY27 | parse_row drops node B's row_commit | KILLED: `test_rows`, `test_the_rows_commit_is_written_with_its_cut` |
| PY28 | the plan runs uv1moved no more on a pull request | KILLED: `test_a_pull_request_that_touches_the_uploader` |


### Non-blocking defects

**N1. uv1 accepts `matched` at a step without checking that the whole cut was the reason** (`tests/kernel/cut_daemon_check.py:1878-1880`).
- **Why it accepts it.** The status read once the mismatch is counted (`nudge_until … upload_mismatches N`) can come after the next cut's check, which is the whole cut. That check sets `upload_checked matched`.
- **The cost.** Whenever that happens, the step passes whatever the mismatch's word was. The course then cannot tell 7989f5e from 75bed3f (N3 reverted), or from a daemon that says `not_the_base` there.
  - For `v1in` that is the only thing the step shows beyond `v1`.
  - The description's open question says the course "cannot tell `row_v1` from a check that skipped the mismatch's word but still counted the mismatch". That understates it: when it reads `matched`, it cannot tell `row_v1` from `head_moved` either.
- **Safety is not weakened** (see (1)).
- **Likelihood: low**, by urow's six reads on #134.
- **Suggestions, any of:**
  - When a step reads `matched`, say in the course line that the step's word was not seen (a note of its own, or ERROR for that step), so that a PASS of uv1 says whether both words were seen.
  - Read the status after each nudge's sync in a tight loop, every 100 ms, before writing more, so that the mismatch's own status is read before the next cut's check. `upload_rule` cannot stand in for this: it is written when an upload ends (`driver.rs:775`), so at a `matched` read it may still say `delta`.
  - A product-side counter of mismatches by reason would remove the race. That is a question for the lead, not this package.
- **Certainty: high** on the code. **Likelihood: low.**

**N2. `move-head --row` with no `--expect-row` exits 6, `row moved`, when it loses head's swap** (`bins/skyzfs-nodeb/src/main.rs:697`, `:707`).
- `guarded = ops.is_some()`. Before this pull request, ops existed only with a guard. Now an unguarded `--row` makes a `MetaPut`.
- So a lost swap of head reads as "the expected row did not hold", against node B's documented exit 6 (`main.rs:86`).
- The rig always passes a guard, so no course is affected.
- Fix: `let guarded = expect.is_some()`, computed before `row_op` consumes it.
- Neither behaviour is pinned: MH7 survives.
- **Certainty: high. Impact: none on the rig today.**

**N3. A nit in the refusal text** (`main.rs:611`). `move-head --row none` is refused "which do not name the head being written". `none` names no head; it is refused because it would delete the row. Say "only v1:, hex: and STATE:NUMBER:GROUP:COMMIT (bytes known before the commit; not none)", or similar.

**D1. The description is stale or does not match the code in five places:**
- "CI: to be added."
- "Courses in the pull request's upload job: `uv1` and `uv1moved` run in it". They cannot, by B1.
- "The courses' first runs on a kernel are this pull request's upload job". That job ran nothing.
- The open question on `matched` understates what is lost (N1).
- The plants table says "every one is caught". I did not rerun the author's plants as they were written. Mine on the same lines (the skip, the end commit, the first row's version, `row_commit`, the state check, uv1moved's kind, end head, end row and `STOPPED_AT`, node B's high byte) are all killed, so I have no reason to doubt it.

### Gaps in the tests

- **G-a.** G3 checks node B's reading of a version 1 row only for "no commit" (`main.rs:970-971`). Comparing the fields too would make G3 kill NB6 on its own.
  ```rust
  assert_eq!((read.state, read.number, read.group), (state, number, group))
  ```
- **G-b.** Nothing ties the workflow's name pattern to the names (B1).
- **G-c.** Nothing pins that `move-head --row` writes head and the row in one transaction (MH2b survives). A refs wrapper that counts `transact` calls, or fails the second one, would pin it. Nothing pins that the command takes `v1:` (MH3 survives).
- **G-d.** uv1's judge never uses others.tsv's `head` line.
  - A `v1in` whose head did not move would pass as a second `v1`.
  - Wanting one node B head recorded, and the fetch of it skipped, would pin that the move happened.
  - The shell acts (`act_v1`, `act_v1in`, `act_v1out`, `v1_move`, `v1_said`) have no case in `test_cut_record.sh`. Swapping the two groups, or dropping `--row`, would show only on a kernel. It would show there as FAIL of both courses, so a kernel run would catch it, once B1 is fixed.
- **G-e.** uv1 has no end-to-end test through `upload_lines`, as uv1moved and umoved have (`test_a_course_that_stops_the_uploads`).
  - Its `course_wants` and its skip in `daemons_fetches` are tested apart.
  - Its EXPECTED entries for the mark rules are pinned only on a kernel (PY21 survives).
  - An end-to-end case would also put `row_commits_off_head` and the version 1 skip through one recording with both of node B's acts.

### What the first kernel run must show (after B1's fix)

For the lead and the author, when the upload job runs:
- **uv1:**
  - the notes `v1: upload_checked 'row_v1'` and `v1in: upload_checked 'row_v1'`. Any `matched` means that step's word was not seen (N1).
  - `upload_status` with `upload_mismatches 2`, `uploaded_whole 3`, `upload_checked matched`.
  - `upload_content` PASS, with at least one fetch of another writer's skipped (node B's head in `v1in`, and the version 1 row over the daemon's cut in `v1`).
- **uv1moved:** `upload_course` PASS (`head_moved` after the act), `upload_status` with `uploaded` unchanged and `uploaded_whole 1`, and `upload_content` PASS ending at the cut before the act.
- **The six earlier courses:** as on #134. Above all, `upload_content` PASS with `row_commits_off_head` now part of it.
- **The controls:** as EXPECTED, with `mark-dropped` and `mark-kept` `SAME` on both new courses.

### CI and Codex

**Codex** completed its code review and its security review of bec9a2f and posted no findings. There are no review comments on the code.

**CI on bec9a2f** (pull request runs; the push runs were cancelled by the pull request's):
- **Red:** skyzfs-cut-daemon's upload job (B1).
- **Green, as of 00:20 UTC on 2026-11-25:**
  - skyzfs-cut-daemon's "which jobs", "the check's logic (no kernel)", ubuntu-26.04 and ubuntu-26.04-arm (held cuts, no upload);
  - `ci`: lint, test, unit-arm64, s3-backends, chart-lint;
  - coverage;
  - skyzfs-cut-rig: the reference, ubuntu-24.04, 26.04 and 26.04-arm;
  - skyzfs-replay-rig: all eight jobs, the jobs through skyzfsd included;
  - skyzfs-nodeb-rig: both images and its logic. This rig uses node B's `put` and `get`, not `move-head --row`;
  - skyzfs-control-rig, skyzfs-kernel-probe;
  - skyzfs-daemon-rig's ubuntu-26.04 and ubuntu-26.04-arm (daemon), "which server", and the table and its judge;
  - e2e, k3s-smoke.
- **Still running then:**
  - skyzfs-cut-rig's ubuntu-24.04-arm;
  - skyzfs-daemon-rig's ubuntu-24.04 (daemon) and its two stand-in jobs.
  
  None of them runs the upload courses, so none bears on B1 or on the new courses.

### Gates (here, at bec9a2f, rustc 1.100.0, the current stable)

- `cargo fmt --all --check`: clean.
- `cargo clippy -p skyzfs-nodeb --all-targets -- -D warnings`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings`: clean.
- `cargo test -p skyzfs-nodeb`: 16 passed, the description's count.
- **The python tests that CI runs, each with `python3 -I`, all OK:**
  - `test_cut_daemon_main` 41;
  - `test_cut_daemon_check` 104;
  - `test_cut_watch` 13;
  - `test_cut_plan` 11;
  - `test_cut_hold` 18;
  - `test_cut_check` 29;
  - `test_cut_ref` 41;
  - `test_nodeb_check` 28;
  - the six of `tests/bench`.
- `bash tests/kernel/test_cut_record.sh`: all passed.
- **shellcheck:**
  - `git ls-files '*.sh' | xargs shellcheck -S warning`: clean.
  - Without `-S warning`, shellcheck 0.9.0 here gives the same three kinds of notes at base and head. The only kind that grows is SC2317, "command appears to be unreachable", from 783 to 821 notes. That is the new functions, which are called by name.

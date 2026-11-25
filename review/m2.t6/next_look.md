## Next look at #137 (M2.T6) at 1d130ea, after the review at bec9a2f

**Verdict at 1d130ea: can merge once the CI runs still going are green. Nothing blocks.**

**B1 is fixed, and a kernel run shows it.**
- 6d15da8 widens the check of recording names to `^[a-z][a-z0-9]*$`. It does so in all three workflows that pass names to `cut-record.sh`. No other check of a name's form exists (see (1)).
- The new test `test_cut_plan.test_every_recording_passes_the_workflows_check_of_names` fails when any of the three workflows goes back to the old pattern (plants WF1 to WF3). It also fails on four other wrong patterns (WF4 to WF7).
- The upload job of skyzfs-cut-daemon run 45519738951 (job 138888996001) has run. It ran all eight courses: the six from before, `uv1` and `uv1moved`. Each ended with exit 0, and each control came out as `EXPECTED` says.
  - In `uv1`, both steps read their own word, `row_v1`. The race N1 describes did not happen in this run.
  - In `uv1moved`, the step read `head_moved`, and nothing went up after it.
  - Every precondition the courses rest on happened (see (2)). No line passed vacuously.
- The job built `33941a7`, which merges 1d130ea into `b9def12`, today's tip of `zfs-topology` (after #136).

**The other findings of the review:**
- **N2, N3, G-a, G-c, G-d and G-e are done as asked.** Every plant of mine on them is killed.
- **N1 is answered by a note, which is sound.** A step that reads `matched` now says in the course line that its own word was not seen.
- **D1:** the description is updated. It still says "CI at 1d130ea: running; links to be added" (L3).

**Plants:**
- Rust: 29 of 30 killed. The survivor is MH7, at the call site (L1).
- Python and the workflows: 40 of 41 killed. The survivor is PY22, which survives by design: the three list rules are set from a course's first kernel run, as for every upload course.
- Every earlier survivor except those two is now killed: MH2b, MH3, PY21. NB6 is now killed by G3 itself.

**Gates at 1d130ea, rustc 1.100.0 (current stable): all clean or passing.**
- fmt and the four clippy runs;
- `cargo test -p skyzfs-nodeb`: 19 passed;
- the python tests CI lists;
- the three bash tests;
- shellcheck `-S warning`.

**Codex** completed its code review and its security review of 1d130ea, with no findings.

**CI on 1d130ea, at 00:58 UTC:**
- **Green:** ci (lint, test, unit-arm64, s3-backends, chart-lint), coverage, e2e, k3s-smoke, skyzfs-kernel-probe and skyzfs-control-rig. Also skyzfs-nodeb-rig (both images, run on this pull request) and all of skyzfs-cut-daemon, the upload job included.
- **Still running then:**
  - skyzfs-daemon-rig: five jobs;
  - skyzfs-replay-rig: its two reference layouts (the jobs through skyzfsd are green);
  - skyzfs-cut-rig: its four kernel jobs.
- None of the jobs still running runs the upload courses. The lead should see them green before the merge.

**Findings open at 1d130ea, most severe first.** None blocks.

| Id | Severity | Where | What | Certainty |
|---|---|---|---|---|
| L1 | low | `bins/skyzfs-nodeb/src/main.rs:699` | `guarded = expect.is_some()` is not pinned through `move_head_on`. MH7, which puts back the old behaviour at the call site, survives. `commit_outcome` itself is pinned. | high |
| L2 | nit | `bins/skyzfs-nodeb/src/main.rs:1267-1271` | The doc comment of `move_head_writes_the_row_in_the_same_commit` now sits above the new test `move_head_takes_the_row_forms_known_before_the_commit`. The test it describes has none. | high |
| L3 | nit | the description | "CI at 1d130ea: running; links to be added". The upload job's course lines are not in it yet; the author said they would add them. | high |
| L4 | nit | `tests/kernel/cut_daemon_check.py:1882` | The note says "this step's own word (row_v1) was not seen". The word not seen could have been anything, which is the point of N1. "(row_v1)" reads as if it were known. | high |

**L1 in more detail.**
- **Scenario.** Someone turns `guarded` back into "an op exists". An unguarded `move-head --row` whose swap of head is lost would then exit 6, `row moved`, again (N2). The rig always passes a guard, so no course would change.
- **The author's reason does not hold.** They say a test needs a hook inside the engine. I think it does not:
  - `commit_with` PUTs the commit's objects through the store before it swaps the ref (`crates/skynbd-engine/src/lib.rs`, the doc of `commit_naming`).
  - A test store that wraps the store given to `move_head_on` could move head on its first PUT of a commit object, through a second `ObjectRefStore` over the same inner store. The engine's `CasRef` then loses, and `commit_with` returns `Conflict`.
  - `skynbd-refs/src/cadence.rs:458` has an interposer of this kind.
  - I have not written this test, so its feasibility is my reading, not a run.
- **Blocks:** no.

### (1) Is the pattern fixed in every place a name is checked?

**Yes.**
- `git grep` at 1d130ea finds the recording check in three workflows, and all three now take `^[a-z][a-z0-9]*$`:
  - `.github/workflows/skyzfs-cut-daemon.yml:217`;
  - `skyzfs-cut-rig.yml:96`;
  - `skyzfs-nodeb-rig.yml:119`.
- **No other place checks a name's form.**
  - `cut-record.sh:188` checks each name against its lists (`KNOWN`). It has no pattern, and its lists hold `uv1` and `uv1moved` (`:168`).
  - `cut_plan.py` passes a dispatch's names through as given, and the pull request's as `PR_UPLOAD_RECORDINGS`.
  - The judges never check a recording name.
- **The other `^[A-Za-z0-9...]` checks in the workflows are for other inputs:** a ublksrv ref, a mirror URL, `serve_args`, a bench label, a daemon label. None takes a recording name.
- **The new pattern is no looser than the old one in what matters.** It still refuses capitals, a leading digit, `;`, `/`, spaces and a leading `-`. Each of these is in the test's list.

**The G-b test** (`tests/kernel/test_cut_plan.py:203`):
- It reads the one `[[ $r =~ ... ]]` of each workflow, and fails if there is not exactly one.
- It runs the pattern in bash, unquoted as in the workflow, over every name in `ALL`, `HELD_ONLY`, `UPLOAD_ONLY`, `UPLOAD_NAMED` and `PR_UPLOAD_RECORDINGS`.
- It also wants seven bad names refused.

**Plants on the workflows**, each alone, against the same four test files as the Python plants:

| Plant | What is planted | Result |
|---|---|---|
| WF1 | skyzfs-cut-daemon.yml back to `^[a-z]+$` | KILLED: the G-b test (subtests `uv1` and `uv1moved`) |
| WF2 | skyzfs-cut-rig.yml back to `^[a-z]+$` | KILLED: the same |
| WF3 | skyzfs-nodeb-rig.yml back to `^[a-z]+$` | KILLED: the same |
| WF4 | skyzfs-cut-daemon.yml `^[a-z0-9]+$` (a leading digit taken) | KILLED: the G-b test (`1life`) |
| WF5 | skyzfs-cut-daemon.yml without the end anchor | KILLED: the G-b test (`life;x`, `life/`, `a b`) |
| WF6 | skyzfs-cut-daemon.yml `^[A-Za-z][A-Za-z0-9]*$` | KILLED: the G-b test (`Uv1`) |
| WF7 | skyzfs-cut-daemon.yml with the check of names removed | KILLED: the G-b test (no pattern found) |

**The kernel run agrees.** The upload job's log shows the new check passing `life umoved urow ukill ulease ulost uv1 uv1moved`. skyzfs-nodeb-rig's two kernel jobs, which ran on this pull request, are green with `life slog`.

### (2) The upload job's first kernel run (run 45519738951, job 138888996001)

The job built `33941a7`, which merges 1d130ea into `b9def12`. The step "Record through skyzfsd and check" took 86 s (00:52:09 to 00:53:35 UTC). `recordings.tsv` has all eight courses recorded and checked with exit 0. The lines and the course files are in `upload_job_1d130ea.txt`.

**uv1.** Each line, and what it shows:
- **`upload_course PASS`:** `v1: upload_checked 'row_v1' once the mismatch was counted; v1in: upload_checked 'row_v1' once the mismatch was counted`.
  - Both steps read their own word. Neither read `matched`.
  - So this run shows F1's word for both cases: the upgrade, where nothing moved, and a move inside the row's group.
  - The second case is the one the review of #134 asked for, and the one that only the word tells.
- **The steps' status, before and after:**
  - `v1-status.txt`: mismatches 0. `v1-after.txt`: mismatches 1, `row_v1`. `v1-whole.txt`: whole 2.
  - `v1in-status.txt`: mismatches 1. `v1in-after.txt`: mismatches 2, `row_v1`. `v1in-whole.txt`: whole 3.
  - So each mismatch came from its own act, and each was followed by a whole cut.
- **`upload_status PASS`:** `uploaded_whole 3, upload_mismatches 2, upload_abandoned_at_mismatch 2, upload_checked matched`, `uploaded 12`, `upload_errors 0`, `upload_cuts_lost 0`.
  - These are the exact counts that pin "no delta under a version 1 row".
  - `upload_balance PASS`: 22 cuts = 12 committed + 8 replaced + 2 abandoned.
- **`upload_content PASS`:** "node B fetched cuts 8, 12, 14, 22 at head and 2 of another writer's", over 24803 blocks. The two of another writer's, from `uploaded.tsv`:
  - cut 8 under the daemon's head `8f5d…` with no commit: the version 1 row over the daemon's cut, skipped by the new rule in `daemons_fetches`;
  - cut 14 under node B's head `6b95…`, 6411 blocks, "1 changed": node B's 0x5a block 0, skipped as another writer's head.
  - Cut 22, at the end, is the daemon's again, over the whole cut that went up onto node B's commit. Its content matches the reference, so node B's block 0 did not survive under it. A delta onto node B's commit would have kept it, unless the daemon wrote block 0 again. Block 0 is the first 4 KiB of ZFS's first label, which, as I read ZFS, it does not write after creating the pool. The unit test G-e makes the same argument.
  - The other fetches name the head's own commit in the eighth field. So the watcher's new field and `row_commits_off_head` ran on a kernel and found nothing off head.

**uv1's preconditions all happened:**
- Before each act, the daemon's row was of version 2 and named head's commit:
  - `v1-row.txt`: cut 8, group 14, `row_commit` = head `8f5d…`;
  - `v1in-row.txt`: cut 14, group 33, `row_commit` = head `5bc7…`.
- Node B ended with 0 both times and said `row_commit none`. Its `set-row` and `move-head` wrote `v1:…:8:14` and `v1:…:14:33`.
- The guard's control (group 0) ended with 6 at both steps (`guard-controls.txt`). There is no `acts-again.txt`, so no act was repeated.
- `others.tsv` names one node B head, `6b95…`, which is not the daemon's head before `v1in`. This is the new G-d check, and it passed on a kernel.
- **The move stayed inside the row's group.** The status said `row_v1` after `v1in`, and with F1's daemon `row_v1` is given only where head's group is the row's. uv1moved shows the other case, `head_moved`, with the same command and the group plus 1000.
- **The end:** the row names cut 18, group 48, and `row_commit` = head `50a1…`. It is of version 2 and names its own commit.

**uv1moved.** Each line, and what it shows:
- **Before the act** (`v1moved-row.txt`): cut 8, group 14, `row_commit` = head `a8f5…`. Status: mismatches 0, uploaded 4, whole 1.
- **Node B** moved head to `e71a…` in group 1014, with the row `v1:…:8:14`, and said `row_commit none`. The guard's control ended with 6.
- **After** (`v1moved-after.txt`): mismatches 1, `upload_checked head_moved`.
- **`upload_status PASS`:** `uploaded 4`, the same as before the act, with `uploaded_whole 1`, `upload_mismatches 3` and `upload_checked head_moved`.
  - The two further mismatches are the daemon's later cuts, checked against node B's head and refused each time.
  - Nothing went up after the act.
- **`upload_course PASS`:** at the end, head is node B's `e71a…`, and the row is node B's version 1 row over cut 8 (`row_commit none`).
- **`upload_content PASS`:** "node B fetched cuts 8 at head and 1 of another writer's". The one is node B's head, skipped. Cut 8, the last cut fetched, is the one before the act.

**The six earlier courses.** All have `upload_content` PASS, and every course but life has `upload_course` PASS:
- **life:** cut 26 at head.
- **umoved:** 1 of another writer's.
- **urow:** `not_the_base` at both steps, as on #134.
- **ukill:** cut 136.
- **ulease:** `lease_epoch '2'` after the other holder let go.
- **ulost:** cut 10.

**Across all eight courses:**
- Every result line is PASS, INFO, CAUGHT or SAME. There is no FAIL, ERROR or NA.
- Every course has 9 CAUGHT (the six list controls, `early` and the two content controls) and 5 SAME (`txg-alone`, `count-repeats`, `previous`, `mark-dropped`, `mark-kept`).
- On `uv1` and `uv1moved`, the three list rules read SAME where `EXPECTED` allows either result. This run is what their entries get set from (PY22).

**What a kernel run cannot show here.**
- The only `upload_checked` read that could be `matched` in uv1 read `row_v1` both times. So the note N1 added was not exercised on a kernel; its unit test pins it.
- This is one run on one image. How often the race in N1 happens on a runner is not known from it.

### (3) N1: uv1 and `matched`

**Sound.** At `cut_daemon_check.py:1878-1883`, `matched` is still accepted, and the course line now says so whenever it is read. That is what the review asked for.
- Safety still rests on the exact counts and the content, not on the word.
- The unit test wants exactly one such note, on the right step.
- **Plants:**
  - PY32 (no note) is KILLED.
  - PY33 (the note for every kind) is KILLED.
  - PY2 (`matched` refused) is still KILLED: the course must still accept it.
- The author left the tight status loop out. Their reason, that it narrows the race but does not close it, is right, and a count by reason is a question for the lead. In this run both steps' own words were read anyway.

### (4) N2, N3 and node B's tests (G-a, G-c)

**The fixes:**
- **N2:** the guard is read from `--expect-row` before `row_op` takes it (`main.rs:699`). The outcome is decided in `commit_outcome` (`:717`), and its test (`:1295`) pins each case: success with and without a guard, `OpConflict`, and `Conflict` with and without a guard.
- **N3:** the refusal (`main.rs:640-644`) names both reasons, and the test checks the words for `none`.
- **G-a:** G3 compares state, number, group and commit for version 1 (`main.rs:990-993`).
- **G-c:**
  - `move_head_takes_the_row_forms_known_before_the_commit` (`:1273`) pins the five forms.
  - `move_head_writes_head_and_the_row_in_one_transaction` (`:1311`) pins atomicity.
    - I checked the second against `FaultStore` (`crates/skynbd-store/src/fault.rs:89-116`). `arm(Put, refs_key, 1)` lets one `put_if` of the refs object through and fails every one after it, so a second transaction fails.
    - The test then reads head and the row through a fresh refs store over the inner store, so no cache hides the result.

**Plants.** Run in a worktree of 1d130ea, each alone, against `cargo test -p skyzfs-nodeb --no-fail-fast`, each restored in a `finally`; the worktree was clean afterwards:

| Plant | What is planted | Result |
|---|---|---|
| MH2b | head committed under the guard alone, the row put in a second transaction after | KILLED: `move_head_writes_head_and_the_row_in_one_transaction` |
| MH2c (new) | the row swapped (guarded) in a first transaction, head committed alone after | KILLED: the same, and `move_head_writes_the_row_in_the_same_commit` |
| MH3 | `--row v1:` refused | KILLED: the forms test and the one-transaction test |
| MH3h (new) | `--row hex:` refused | KILLED: the forms test |
| MH3c (new) | `--row STATE:NUMBER:GROUP:COMMIT` refused | KILLED: the forms test |
| MH4 | `--row STATE:NUMBER:GROUP` taken | KILLED: the forms test and the same-commit test |
| MH4n (new) | `--row none` taken | KILLED: the same |
| MH7 | call site: `guarded = expect.is_some() \|\| row.is_some()` (the behaviour before N2) | **SURVIVED** (L1) |
| MH7b (new) | `commit_outcome`: a lost swap of head is `None` whatever the guard | KILLED: `a_lost_swap_of_head_is_the_row_moved_only_under_a_guard` |
| MH7c (new) | `commit_outcome`: a lost swap of head is an error even under a guard | KILLED: the same |
| MH8 (new) | the refusal no longer says why `none` is refused | KILLED: the forms test |
| MH1, MH5, MH6 | as at bec9a2f | KILLED |
| NB1 to NB9 (with NB4b) | node B's codec, as at bec9a2f | all KILLED, each by G3 among others. **NB6 is now killed by G3** (G-a). |
| RW1 to RW7 | the uploader's `row.rs`, as at bec9a2f | all KILLED by G3 alone |

### (5) The judge: G-d, G-e, PY21 and PY22

**G-d** (`cut_daemon_check.py:1884`):
- `v1in` now wants exactly one node B head in `others.tsv`, and not the daemon's head before the act.
- **The head line is good evidence of a move.** `v1_move` writes it from node B's own print after exit 0, and node B prints the commit that `commit_with` installed.
- **Plants:**
  - PY29 (check removed) is KILLED.
  - PY30 (two heads taken) is KILLED.
  - PY31 (the daemon's own head taken) is KILLED.
  - PY34 (the check also at `v1`) is KILLED: a missing head would then count twice.
- **Still a gap:** the shell acts have no case in `test_cut_record.sh`. The kernel run shows them right.

**G-e** (`test_cut_daemon_main.py:1379`, `test_the_course_of_rows_of_version_1`):
- It takes uv1 through `upload_lines` with both of node B's fetches.
- With the whole cut writing block 0 back, the course passes, with "2 of another writer's". Without it, `upload_content` fails.
- It kills:
  - PY21 (`mark-kept` caught on uv1), alone;
  - PY15 and PY16 (the version 1 skip);
  - PY17 and PY18 (the commit dropped by `read_uploaded` and `read_row`).

**PY22 survives, and the author's reason holds.**
- `UNKNOWN` lets `txg-alone`, `count-repeats` and `previous` read either SAME or CAUGHT on an upload course until its first kernel run. Every upload course was treated this way.
- What a synthetic recording gives for these rules says nothing about the kernel.
- This run gave SAME for all three on both new courses. Pinning them is the author's stated next step.

**Python plants, all 41 at 1d130ea.** Each was run alone against `test_cut_daemon_main`, `test_cut_daemon_check`, `test_cut_watch` and `test_cut_plan`, in four worktrees side by side; all were clean afterwards. 40 are killed; PY22 survives. Each plant's failing tests are in `plants_py_1d130ea.out`.

### Gates (at 1d130ea, here)

- `cargo fmt --all --check`: clean.
- **clippy `-D warnings`, all clean:**
  - `-p skyzfs-nodeb --all-targets`, run again after touching `main.rs`;
  - `--workspace --all-targets`;
  - `-p skyzfsd --features test-hooks`;
  - `-p skyzfs-ublk-sys --features control-test-program`.
  - Only `main.rs` changed in Rust since bec9a2f, where I ran the workspace from cold.
- `cargo test -p skyzfs-nodeb`: 19 passed, the description's count.
- **The python tests in `ci.yml`, with `python3 -I`, all OK:**
  - `test_cut_daemon_main` 42;
  - `test_cut_daemon_check` 104;
  - `test_cut_plan` 12;
  - `test_cut_watch` 13;
  - the rest as at bec9a2f.
- **Bash:** `test_cut_record.sh`, `test_zfs_stuck.sh` and `test_copy_limit.sh` all passed.
- `git ls-files '*.sh' | xargs shellcheck -S warning`: clean.

### Files added to this folder

- `next_look.md`: this note.
- `plants_rs_1d130ea.py`, `plants_rs_1d130ea.out`, `plants_rs_1d130ea.json`: the Rust plants at 1d130ea.
- `plants_py_1d130ea.py`, `plants_py_1d130ea.out`, `plants_py_1d130ea.json`: the Python and workflow plants at 1d130ea.
- `gates_1d130ea.out`: the gates.
- `upload_job_1d130ea.txt`: the upload job's result lines for all eight courses, with uv1's and uv1moved's course files.

I posted nothing on GitHub.

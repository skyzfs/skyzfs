# Review of #121: the upload job's paths, and the last points of #119

Pull request: https://github.com/skyzfs/skyzfs/pull/121, `skyzfs-testing` into `zfs-topology` (240628a). I was asked to look at b51d3fe; the head moved to **75f3227** while I read (one more commit: `rust-toolchain.toml` starts the upload job too), and everything below is at 75f3227. I read the diff before the description.

**Verdict: not yet, for one small finding (B1). The rest can merge as it is.**

B1 blocks by the rule I was given, that every crate under the uploader, the daemon and node B starts both the workflow and the upload job. The pull request does what the lead's decision on F4 listed, and adds `skynbd-types` and the toolchain file. But that list was mine (F4 in the next look at #119), and I did not make it from the dependency graph: it left out four crates. Merging as it is makes nothing worse than the base; the fix is about ten lines.

CI at 75f3227 when I looked: 22 checks green, 19 still running, none failed. Codex reviewed 75f3227 and has no finding. The pull request's own cut-daemon run [44313127063](https://github.com/skyzfs/skyzfs/actions/runs/44313127063) is done and green: its plan gave three jobs, and the upload job's six courses all ended `checked exit 0`. The head's tests pass here (`test_cut_plan.py` 8, `test_cut_watch.py` 12, `test_cut_daemon_main.py` 38, `test_cut_daemon_check.py` 99, `test_cut_hold.py` 18, `test_cut_record.sh`).

## (a) The path list against the dependency graph

`closure.py` reads `cargo metadata --locked --all-features` at the head and follows normal and build dependencies from `skyzfs-upload`, `skyzfsd` and `skyzfs-nodeb` to the workspace's crates; `closure.out` is its result. Fifteen workspace crates are in the closure.

**B1 (blocks). Four crates of the closure do not start the upload job, and two of them start no job of this rig at all.**

| Crate | In the closure of | Workflow `paths` | `UPLOAD_PATHS` | What rests on it |
|---|---|---|---|---|
| `crates/skynbd-tree/` | all three, through `skynbd-engine` | no | no | the engine's extent map and index (skynbd-engine `lib.rs:39`, `index.rs:160`): every block the uploader writes and node B reads goes through it |
| `crates/skyzfs-mapped/` | `skyzfs-upload` and `skyzfsd`, through `skyzfs-local` | no | no | the region the cut state lives in across a daemon's death (skyzfs-local `inplace/state.rs:379-382`): what `ukill`'s take-over and the held cuts' `takeover` rest on |
| `crates/skyzfs-uberblock/` | `skyzfs-upload` (through `skyzfs-local`) and `skyzfsd` (directly) | yes | no | which writes make a cut, and a cut's group, which is the row's group |
| `crates/skyzfs-ublk-sys/` | `skyzfsd` (directly, and through `skyzfs-local`) | yes | no | the binding the daemon serves through |

- Scenario: a pull request that changes only `crates/skynbd-tree/src/lib.rs` (say, how an insert splits a range) starts neither the held cuts' jobs nor the upload job. One that changes only `crates/skyzfs-mapped/` likewise. One that changes only `crates/skyzfs-uberblock/` or `crates/skyzfs-ublk-sys/` starts the two held cuts' jobs and no upload course.
- The tests pin the last two as they are: `test_a_pull_request_that_touches_no_upload_path` uses `crates/skyzfs-uberblock/src/lib.rs` (test_cut_plan.py:22) and `test_paths_that_only_look_alike` uses `crates/skyzfs-ublk-sys/src/lib.rs` (line 41). Nobody decided to leave them out: G7's list and my F4 simply did not name them.
- Fix: the four directories in `UPLOAD_PATHS` (cut_plan.py:34-58), `crates/skynbd-tree/**` and `crates/skyzfs-mapped/**` in the workflow's `paths` (lines 62-83), and the two test lines moved to the positive list. If the lead means `skyzfs-uberblock` and `skyzfs-ublk-sys` to stay with the held cuts' jobs alone, that is a decision to write next to the list; the two crates that start nothing are not a matter of cost.
- Certainty: certain, from `cargo metadata` and the two lists. I checked that each of the four is used in code that is built, not only named in a manifest.

For the lead, outside this pull request: no workflow in `.github/workflows` names `crates/skyzfs-mapped/` or `crates/skynbd-tree/` in its `paths`. A pull request that changes only one of them runs `ci.yml` and no kernel rig.

**The rest of the list is complete.**

- Every other crate of the closure starts both (11 of 15). Nothing is listed that no root is built from, and every `UPLOAD_PATHS` entry is also under the workflow's `paths` (`tests/kernel/cut*` covers `cut_plan.py` and `cut_summary.py`).
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` and `tests/lib/` are in both. There is no `.cargo/` directory, and no crate includes a file from outside its own directory.
- Renames: the plan's diff has `--no-renames` (workflow line 127), so a file moved out of a listed directory still shows under its old name.

**N1 (not blocking, for the lead). Four files the upload job's judge runs are not in the plan.** `cut_daemon_check.py` imports `cut_check.py` and `cut_ref.py`, and `cut_check.py` imports `replay_check.py` and `logwrites.py`. A pull request that changes only one of the first three starts the held cuts' jobs, which run the same judge, and no upload course. `tests/kernel/logwrites.py` is not in this workflow's `paths` at all, though the cut rig's and node B's workflows name it.

## (b) The guard's control on group 0

Safe for both commands.

- **It cannot meet a real row.** Node B takes `STATE:NUMBER:0` as any other row: the group is parsed as a number and written as eight bytes, with nothing special about 0 (main.rs:368-390). The guard compares the row's bytes whole, in the refs transaction. A daemon's row has its cut's group, and no cut has group 0: an uberblock write counts only with a group above zero (skyzfs-uberblock `lib.rs:371-373`). The other writer's rows keep the look's group (`act_row1`, `act_row2`). Unlike the control of #119 (number one past, same group), a second cut of the look's group between the look and the control cannot make it true.
- **It leaves nothing a judge reads.** `set-row` is one `MetaCas` and nothing else (main.rs:535-539): refused, nothing was written. `move-head` writes its block's objects before the transaction, so a refused control leaves objects that head never names, as the control of #119 did; `others.tsv` is not appended, and the real act writes `moved.txt` or `set-row.txt` afterwards.
- **Exit 6 comes from the guard.** For `set-row` a refused `MetaCas` is `TxnOutcome::Conflict`, which is "not set" and 6. For `move-head` it is `OpConflict`, or `Conflict` where the daemon moved head meanwhile; the second is 6 only with `--expect-row` (main.rs:506-508). A node B that ignored the row's group would act on the control and the rig would say "the guard is not wired", exit 2, which is then true.
- **On the kernel** (run 44313127063, the upload job): `guard-controls.txt` has `moved ...:8:0` in `umoved`, and `row1 ...:8:0` and `row2 ...:14:0` in `urow`, against looks at `8:14`, `8:14` and `14:33`. All three were refused with 6, or the course would have ended as the rig's error. `others.tsv` has one line per act, and both courses ended `checked exit 0`. No `looks-again.txt` and no `acts-again.txt` in the run.

The `sleep 1` before the second look on a stale status (cut-record.sh:1046) is inside the same five tries: at most five seconds more, then the rig's error as before.

## (c) Plants

`plants4.py`, results in `plants4.out`: 72 lines, 63 killed, 7 survive, 2 not applied (P17c and P17d of #119, whose line 4aac932 rewrote; P17e and P17f take their place and are killed). The checkout was clean afterwards.

| Where | Plants | Killed | Survive |
|---|---|---|---|
| The plan's plants of #119's next look (P01 to P24), with P17e and P17f for the two not applied | 30 | 30 | |
| The plan without one path, each of the 21 (12 of them new) | 21 | 21 | |
| The plan's path rule (P50 to P55) | 5 | 3 | 2 (P51, P52) |
| `before_other`'s status check and its wait | 6 | 5 | 1 (R70) |
| `other_act`'s control | 8 | 4 | 4 (R63, R64, R66, R67) |

- **R52 and R65 are killed**, as the description says: R52 by the new case "a status ahead of its row", R65 by counting the stand-in's calls.
- **R66 and R67 survive: no test holds that the control is on group 0.** With the control put back on the number one past and the row's group (R66), or on group 1 (R67), `test_cut_record.sh` passes: the stand-in act takes any row that is not the look's as a control, and the expected message has `:0` only because the script's message does. A stand-in that records the control's row would hold it.
- **R64 survives now** and was killed at 4aac932: the test no longer reads `guard-controls.txt`, so nothing local holds that the file is written. It is what I read from the run above. Counting both the calls and the file's lines would hold both.
- **R70** (the wait removed) survives by construction: the test stubs `sleep`.
- **R63** as at 4aac932: the stand-in act ends with 0 or 6 only.
- **P51 and P52** are the rule's own slack and harmless: a listed directory's name deeper in a path (`x/tests/lib/a`), and a listed file taken as a directory.

## Files

Under `review/m2.t3-4/` on `skyzfs-review`: `review.md` (this), `closure.py` and `closure.out`, `plants4.py` and `plants4.out`. `plants4.py` uses the harness under `review/m2.t3-3/`.

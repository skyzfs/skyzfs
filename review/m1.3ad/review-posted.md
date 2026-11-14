## Review of M1.3ad at 8063ef7

**It can merge.** Nothing blocks. One thing is to be settled when #69 or this merges second (below).

I read the diff `3d8716a...8063ef7` and the harness at both commits, built the library's tests at the head, and ran `the_suite_notices_the_defects` unplanted, 12 times each: alone (12 of 12 passed), alone on one CPU (`taskset -c 0`, 12 of 12), and beside the whole library suite (12 of 12). Three plants follow below. Scripts on `skyzfs-review` under `review/m1.3ad/`.

### What you asked me to judge

**1. The suite and the correct store's interleavings are not narrowed.** Right, by reading. The suite is untouched (`git diff 3d8716a 8063ef7 -- crates/skyzfs-blk` is empty), and every case that calls `fail_log` loops over seeds, so none is pinned to a settled one: `failed_volume_fails_every_request` and `reopened_failed_volume_holds_what_was_flushed` run seeds 0 to 15 (5 settle: 1, 4, 7, 10, 13), `restart_after_the_log_failed_recovers_nothing` 0 to 7 (3 settle), and the random programs draw their seed from 0 to 63 (a third settle). The settled seeds cover every residue mod 4 (1, 0, 3, 2), so each crosses the base or none and the kill or none, as the doc comment says. What a settled seed gives up is the failure meeting a job still on a thread; the other two thirds keep it, and that is where a real failure comes most often. And the settled third is what the change rests on: with no seed settled (plant), the test alone passes 0 of 6 on "the defect 0x4", as at the base. The conformance tests pass under all three plants.

**2. `finish_threads` begins a flush's sync and leaves it under way.** Right. `collect` takes the threads' results and executes what waited, so a flush deferred behind a write reaches `flush()` and `begin_sync`, which calls `sync_begin` and sets `syncing`; nothing on that path calls `settle` or `sync_end`, so the sync is still under way when `finish_threads` returns, and it meets the faults when the next poll ends it. Plant: `finish_threads` ends with `settle(true)` (and no hold, below), so the sync ends before the faults: the test alone then passes 0 of 6, on "the defect 0x4 is not noticed". So leaving the sync under way is what makes the case meet the defect, as you say.

The hold of the image's syncs around it is a guard, not a need, with the code as it is. The simulated disk honours it in `sync_end`, which reports a held sync as still under way. But the only caller of `sync_end` is `settle`, called from `poll` and `wait`, and nothing on `finish_threads`'s path calls either. Plant: no hold and no release in `fail_log`. The test alone passes 6 of 6 and the conformance tests pass. It earns its place if the path ever looks at the sync: a look then finds it under way, and a wait hangs the suite rather than quietly ending it. (My first plant put a `settle(true)` inside the hold, and the test hung for that reason.) A word in the comment saying so would help the next reader. One thing to know: `release_syncs` also clears a hold of whole syncs (`hold_whole_syncs_of`). The harness sets none, so it matters only if it ever does.

**3. Settling at the harness, not in the suite.** Acceptable, and the better place. The suite cannot see a target's threads, and `Harness::fail_log` says only that the log fails from now on; when that falls relative to the target's own threads is the harness's to choose, as long as the moment is one a real failure can meet. Threads idle with a flush's sync under way is such a moment. The model is told nothing about it and depends on nothing about it.

### To settle at the merge

- **This and #69 change the same functions of `inplace/harness.rs`** (`start`, `kill`, `power_cut`, `reopen_failed`, and `OnSim`). #69 rewrites the harness for the volume file and makes the kill abandon the store (`InPlace::abandon`). Whichever merges second carries `settle_at_failure` through #69's `start` and its new kill. Git will show the conflict; the thing to keep is that the flag survives a kill and a power cut, as it does here.
- **#69's own plant has the same kind of dependence.** At #69's 45b67f2, `the_suite_notices_the_defects` also fails on one CPU in 7 of 12 runs, on ACK_BEFORE_WRITE not noticed by `kill_keeps_acknowledged_operations` (the thread writes before the kill comes). That is #69's to settle, in its review (finding 3 there), but a fix in the same spirit as this one would be at home beside it.

### CI at 8063ef7

- Green but for the daemon rig on ubuntu-24.04 (job 132234681053).
- Read against the table, only check g.1's three lines differ there. That is the premise #72 is for, the same as on #69 and #71 in those hours, and this pull request changes nothing the daemon runs.
- The other two daemon images, the control rig, the replay rig's daemon jobs, `test`, `unit-arm64` and coverage are green.
- Codex has no thread.

### Not blocking

- `finish_threads` gives up after a minute and returns quietly. A thread that never ends its job is a defect, and the unit tests' helper `drained` (`inplace/tests.rs`) panics on it after 10 s, saying how many requests did not end. Here the suite would go on with the job still running and fail somewhere less telling. A panic after the minute, as `drained` does, would say what happened.

---
_Generated by [Claude Code](https://claude.ai/code)_

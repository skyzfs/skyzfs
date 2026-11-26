# Fourth look at #149 (M2.6c-3, the program `skyzfs-agent`) at 5259bf5

- **Head judged:** `5259bf5f0085de020cb22738648a2470d8ac481c`, read from the pull request before and after the work; it did not move. **Base:** `zfs-topology` at `f8e5e0c` (the pull request's base sha).
- **Previous look:** `09e3227` (`third_look.md` here, comment 7176857279 on the pull request).
- **The author's answer:** https://github.com/skyzfs/skyzfs/pull/149#issuecomment-7177671991
- **Diff reviewed:** the author's changes since `09e3227`, without what the merge of `zfs-topology` brought.
  - `755dad2`: N6's fix and its test, and suggestion 1 (the text of `Error::Unmarked`). Two files: `crates/skyzfs-agent/src/engine.rs` (+7/-3) and `crates/skyzfs-agent/src/engine/tests.rs` (+167).
  - `5259bf5`: the merge of `zfs-topology` at `f8e5e0c` (#150 and #152 merged there).

## The merge 5259bf5

It brings `zfs-topology` and changes nothing of the package.
- `git show --remerge-diff 5259bf5` is empty, and `git merge-tree --write-tree 755dad2 f8e5e0c` gives `5259bf5`'s tree (`9d02890`): no conflict and no hand edit.
- `zfs-topology` changed none of the package's files between `a5e883a` and `f8e5e0c` (`crates/skyzfs-agent`, `bins/skyzfs-agent`, `crates/skyzfs-control`, `Cargo.toml`, `Cargo.lock`).
- The pull request's own diff (`git diff origin/zfs-topology...5259bf5`) is still the package alone: 16 files, +4227/-151. Against `09e3227` it differs only by `755dad2`.

## Verdict at 5259bf5

**Not ready to merge yet, for one new finding: C1, Codex's P2 on this head, which I confirm.** Everything the author changed since `09e3227` is right.

- **N6 is fixed**, with my fix F2, at the asking commit (`engine.rs:1173-1175`). The new test is my N6 course with its 8 rows. Planting the clearing out fails exactly the 4 `moved` rows, as the author says. Three more plants that move the clearing are also killed.
- **Suggestion 1 is done.** `Error::Unmarked` now says `zfs release -r skyzfs-publish <pool>@<name>` (`engine.rs:371`).
  - The recordings the author cites do show `zfs release -r` exiting 0 on a partial hold, on both images, with no tag left after it.
  - But the recorded hold is on the child alone, not on the root alone as the answer says (nit 2; details under "Suggestion 1" below; O1 is the same mix-up in #153's test). The conclusion stands, and for the harder case.
- **Codex's two threads from `af50814`** are answered on their threads and resolved. Each answer matches the code at this head, and the plants behind each are killed here.
- **The gates pass**, the third look's searches pass again at this head, and the CI runs on this head are green (see "CI on the head").
- **C1 (new, from Codex on this head, unanswered):** the asking record's deadline is stamped before the asking save, and the request is sent after it. If that save, or a stop of the process, takes longer than `HOLD_MARGIN` (40 s), and the answer is then lost, the hold is released while the request pends.
  - It comes from L1 of this pull request. It is the same class as N5, and N5 was ruled blocking.
  - The fix is a few lines (fix probe F3, verified).
  - **Whether it blocks is the lead's call; by N5's precedent it would.** Either way, the author must answer the Codex thread before saying "ready to merge".

## Defects

### C1 (should fix before merge; Codex's P2 on 5259bf5, confirmed): a slow asking save eats the hold's margin

**Codex's thread:** https://github.com/skyzfs/skyzfs/pull/149#discussion_r4702172281 (`engine.rs:1156`, "Revalidate the recorded deadline before sending the publish").

**Where.** In `Agent::ask` (`crates/skyzfs-agent/src/engine.rs`):
- `:1152-1153` reads `sent` (the wall clock) and `sent_mono` (the clock that does not step).
- `:1154-1157` computes `asking_until = held_until(sent + ASK_LIMIT, expiry)`, which is `sent + 40 s + expiry + 40 s`.
- `:1158` saves the record `asking`. `Store::save` writes a file, syncs it, renames it over the record and syncs the directory (`record.rs:746-751`), with no deadline.
- `:1174` anchors the hold on `(sent, sent_mono)`, which is N5's fix.
- `:1177` sends the request.

**The budget.** The budget assumes the send happens at `sent`. A request sent at `sent + d` is accepted by `sent + d + ASK_LIMIT`.
- skyzfsd's uploader drops an ask whose handler stopped waiting (`skyzfs-upload/src/uploader.rs:411`), so acceptance is bounded by the channel's wait from the send.
- The request then expires by `sent + d + ASK_LIMIT + expiry`.
- `now_for` releases at `held_until` on the clock that does not step, measured from `sent_mono`.
- So for `d > HOLD_MARGIN` (40 s), the hold ends while the request can still pend. For a smaller `d`, the margin left for what it was set for shrinks by `d`.

**When it is reached.** Only with an ask whose answer never comes back to the agent. That is one of:
- an `Err` (a reset);
- `refused no-answer`, which keeps the record asked;
- the agent killed after the send and before `asked` is recorded.

Then, with the daemon away or silent until `held_until`, either of these releases the hold:
- the agent's own flow (`onward`, `away()`);
- a person's give-up (`given_up`, silent).

A person may then destroy `pool@t`.

**Regression from L1.** At `af50814`, before the asking record, `held_until` was read after the answer, so a slow save could not shorten it. N5's fix anchored the hold on `sent_mono` and inherited the stamp's place.

**Reproduced on the fakes** by scratch probe D3 (`fourth149_scratch.rs`; output in `fourth149_scratch.out`).
- **Set-up:**
  - The world's clock is advanced by `d` at the point after the asking save, which stands for a slow save.
  - N5's daemon `LostAnswer` loses the answer (`Err` or `no-answer`) and goes away for 900 s.
  - A destroyer destroys `pool@t` as soon as it is not held.
  - The strict check: at each release, does the fake daemon's own state have the request pending?
- **Runs:** for each `d`, 216 runs: Idle and Busy, `Err` and `no-answer`, the agent's own flow and a same-process give-up, and 27 upload lags.

| Save takes `d` | Released while pending | Wrong publishes |
|---|---|---|
| 0, 30, 60, 79 s | 0 of 216 | 0 |
| 81, 120, 300 s | 64 of 216 (8 lags in each of the 8 cases) | 0 |

- **The threshold.** The fake accepts at the send, so its threshold is `ASK_LIMIT + HOLD_MARGIN` = 80 s. With `d = 81` the release comes 599 s after the send, and the request expires at 600 s. Against skyzfsd, which can accept up to `ASK_LIMIT` after the send, the threshold is 40 s.
- **No wrong publish in this sweep, by the fake's limits.** The fake uploads every cut with the same lag. In this window, a cut that carries the request is always chosen before the destroy. A wrong publish needs an uploader that is slow on the cut in flight and quick on the cut after the destroy. skyzfsd's uploader can be that, since a step can wait on object storage.
- **What a wrong publish needs follows from the precedent.** N5 showed that a release while the request pends is followed, on the fakes, by `snap/t` on a cut without `pool@t`. The pull request's own tests use the same strict check as their criterion (the N6 test asserts it).

**Fix (verified): fix probe F3.**
1. Budget a slack `S` into the asking deadline: `asking_until = held_until(sent + ASK_LIMIT + S, expiry)`.
2. After the asking save, if the clock that does not step has run more than `S` since `sent_mono`, send nothing this round (`Ok(Moved::No)`).
3. The record stays asked with nothing out. The next round reads `progress` `unknown` and asks anew, with a fresh stamp.

Results with `S` = 10 s:
- D3 gives 0 releases while pending at every `d`.
- The whole library suite passes (158 tests).

Things for the author to weigh:
- A save that is always slow then never sends. The publish stalls and is given up by the planner's own bounds.
- Each skipped round may count as one stall.
- Re-stamping and committing `asking` again is the other shape of the same fix.

A test of the form of D3 would pin the fix: a slow asking save, the answer lost, the daemon away; no release while the request pends.

**Certainty**
- Certain in the code, and reproduced on the fakes with the strict check.
- In production it needs all of these:
  - the asking save (two syncs) or a stop of the process between the clock's reading and the send taking over 40 s;
  - the answer lost, or the agent killed before `asked`;
  - the daemon away or silent until `held_until`;
  - a person destroying `pool@t` after the release;
  - an uploader that carries the request on a cut after the destroy.
- This is narrow, like N5.

## The author's changes

### N6's fix (`engine.rs:1170-1175`)

**This is F2.** At the asking commit, `memory.withdrawn` is cleared together with setting `memory.accepted`.
- The comment says why: a new request may be out from there, and a withdraw answered for an earlier request does not count for it.
- The clearing comes after the save and before the send:
  - a save that fails returns before it, with nothing sent;
  - every path that sends a `publish` goes through `Agent::ask` (`ask_anew` calls it, `engine.rs:1281`).

**Where `withdrawn` is read** (`plan.rs:500`, `:520`, `:823`), each reading is now of the request under way.
- The clearing makes `given_up` more conservative: it withdraws or waits.
- In the age bound's arm it can only turn `look` into `Withdraw`. A withdraw answered `withdrawn` sets the flag again, so the next round looks. No new loop.

**The clearing at acceptance** (`engine.rs:1198`) is now redundant. Plant E1 (removing it) SURVIVES. It is harmless and can stay.

**N6 interacts with Codex's P1.** P1's fix in `given_up` turns silence off once `withdrawn` is set. N6 was a way for a stale flag to turn it off for a later request, so this fix closes a hole in P1's fix too.

### N6's test (`engine/tests.rs:2939-3105`)

`a_withdraw_answered_does_not_count_for_the_request_asked_anew` is my scratch N6, taken verbatim and formatted:
- `LoseNth` (`:2942`), `n6_run` (`:2984`) and the test (`:3092`);
- 8 rows: Idle and Busy, `Err` and `no-answer`, `moved` false and true.

**It passes at the head.** The plants:

| Plant | What it does | Verdict |
|---|---|---|
| N6a (the author's mutant) | the clearing removed | KILLED, failing exactly the 4 `moved` rows (re-run alone, rows listed in `plants149d.out`) |
| N6b | cleared only on `Err` | KILLED |
| N6c | cleared only on an answer | KILLED |
| N6d | both clearings removed | KILLED |

**Its course is reached** (scratch probe D1).
- In all 8 rows the answer is lost.
- The `moved` rows send two publishes and have a withdraw of the first answered before the loss. The others send one.
- After the give-up, each row asks `progress` (`pending`), withdraws (`withdrawn`) and looks (`absent`).
- Each ends settled, `Abandoned(Person)`, with no hold left.

**Widened** (scratch probe D2), the `moved` course gives 36 runs, 0 bad:
- the first withdraw's answer as `withdrawn`, `unknown` or `expired`;
- the give-up 0, 30 or 90 s after the loss.

The test asserts none of the course itself (see T1).

### Suggestion 1 (`engine.rs:366-375`)

The text now ends `release those holds by hand (zfs release -r skyzfs-publish <pool>@<name>)`. The names the message lists are the publishes' `t`. `HOLD_TAG` is `skyzfs-publish` (`zfs.rs:39`). No test pins the text, which is fine for an error's words.

**On the partial hold, I checked the recordings** (runs 45703752703 and 45703760219, artifacts `zfs-cli-ubuntu-26.04` and `zfs-cli-ubuntu-24.04`, recorder at `f47d864`).
- On both images and both pools (`rig`, `mirror`), `zfs release -r skyzfs-publish <pool>@t` exits 0 with nothing on standard error.
- `zfs holds -H -r` then prints nothing.
- The recorded case is a hold on `<pool>/child@t` alone:
  - `hold-child` comes after `release_quietly`, which releases every member's hold unrecorded (`tests/kernel/zfs-cli-record.sh:419-424, 448-455`);
  - so the root lacks the tag.
- So the remedy works where the root lacks the tag, which is the case one might expect to fail.
- The case "a hold on the root alone, then `release -r`" is not recorded. The `hold -r` on a root-alone hold fails with `tag already exists` and adds nothing, and the hold is then released unrecorded.
- By libzfs's `zfs_release` as I read it, the root-alone case exits 0 too. It collects the members that have the tag and fails with `no such tag` only when none has it, as the recorded `release-2` does. That is my reading of OpenZFS, not a recording.

## Codex's threads

Read with `gh api`, read-only, including the review threads' route.

| Thread | State | Answer | Checked against the code at `5259bf5` |
|---|---|---|---|
| P1, "Keep the hold when recovery receives an inconclusive answer" (`plan.rs`) | resolved | "Fixed in 2d806e8 (the review's B1)" | Right. `given_up` (`plan.rs:495-533`) reads `ended`, `no-answer` and `no-upload` as silence until a withdraw of its own is answered, and waits until `held_until` for a publish asked. Plants B1a and B1f are KILLED at this head. |
| P2, "Track the publish across snapshot-name collision retries" (`program.rs`) | resolved, outdated | "Fixed in 2d806e8 (the review's D2)" | Right. `Agent::name_now` (`engine.rs:570`) follows the renames, and `execute` looks the outcome up by it. Plant D2a is KILLED at this head by `a_publish_made_under_its_next_name_exits_by_its_outcome`. |
| **New:** P2, "Revalidate the recorded deadline before sending the publish" (`engine.rs:1156`) | **open, no answer** (posted 23:39 UTC on `5259bf5`) | none yet | C1 above: valid in substance. |

Codex's security review of `5259bf5` completed with no findings.

## Test gaps and suggestions (none blocks)

- **T1. The N6 test does not assert its own course.**
  - It checks only "no release while pending" and "no wrong publish". It does not check:
    - that the answer was lost;
    - that the `moved` rows sent two publishes after a withdraw was answered;
    - that no hold is left at the end (the N5 test does check this).
  - D1 shows all of it holds at this head.
  - Asserting it would stop the 4 `moved` rows from quietly becoming copies of the control if the agent's withdraw on a moved pool changed.
- **Nit 1.** The doc of `Seen::withdrawn` (`plan.rs:325-329`) says "since a request of it was last accepted". It is now cleared when a request is asked (the asking commit), so "asked" is the word.
- **Nit 2.** In the answer, "a hold on the root alone" describes the recorded state wrongly (see suggestion 1 above). The conclusion is right.

## Observations

**O1, for #153 (not this pull request).**
- `a_hold_on_the_root_alone_as_recorded` (`crates/skyzfs-agent/src/zfs/cli/recordings.rs:764` on `skyzfs-coding-m2.6c-2b`) feeds `rig-release-r-partial` and `rig-holds-r-after-partial` as the release of the root-alone hold it set up.
- In the recorder, those two records follow `release_quietly` and `hold-child`: a hold on the child alone. The test's doc, "`zfs release -r` then releases it", joins two states that were not one.
- It passes because the adapter reads the same commands. Worth a word to #153's author and reviewer.

## Verification

### Gates (`gates149d.log`, made by `gates149d.sh`)

Run on a clean worktree of `5259bf5`. The toolchain was rustc 1.100.0, current stable after `rustup update stable`. The target directory was my own, with `debug=0`.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy -p skyzfs-agent -p skyzfs-agent-cli -p skyzfs-control --all-targets -D warnings`, plain and with `--features skyzfs-agent-cli/test-hooks` | pass |
| Tests, plain and with hooks | pass: 158 library (157 + the new one), 7 binary, 16 `skyzfs-control` |
| `cargo doc --no-deps` of the three crates with `RUSTDOCFLAGS=-D warnings`, plain and with `skyzfs-agent-cli/test-hooks` | pass |

- The workspace-wide gates were not run here.
- The author reports them clean, with each feature.
- At `5259bf5`, CI's `lint` job runs the workspace's fmt, clippy and `cargo doc -D warnings`: plain, with `skyzfsd/test-hooks` and with `skyzfs-ublk-sys/control-test-program`. It does not run them with `skyzfs-agent-cli/test-hooks`; that is L3/T1, still open.

### Plants (`plants149d.py`, `plants149d.out`)

Each plant was applied alone as text to a clean worktree, with a 600 s limit and the files restored in `finally`. The worktree was clean afterwards. The suite is `cargo test -p skyzfs-agent --lib`.

| Plant | Verdict | Caught by |
|---|---|---|
| N6a-d (the clearing removed or moved) | KILLED, all 4 | `a_withdraw_answered_does_not_count_for_the_request_asked_anew` |
| E1 (the clearing at acceptance removed) | SURVIVED, as expected | (equivalent) |
| N5a (N5's anchor removed, on the new text) | KILLED | `an_ask_whose_answer_is_lost_keeps_its_hold_on_the_clock_that_does_not_step` |
| B1a, B1f (behind Codex's P1) | KILLED | `plan::tests::a_person_gives_up_in_the_order_of_every_give_up`; B1a also by `a_person_s_give_up_keeps_the_hold_while_the_uploader_is_silent` |
| D2a (behind Codex's P2) | KILLED | `program::tests::a_publish_made_under_its_next_name_exits_by_its_outcome` |
| F3 (fix probe for C1) | SURVIVED, as intended: the fix breaks no test | |

### Scratch tests

**`fourth149_scratch.rs` (D1, D2, D3), output in `fourth149_scratch.out`.** D3 is there at the head and with F3.

**The third look's `third149_scratch.rs`, re-run at this head** (release build; `fourth149_search.out`):

| Test | Result |
|---|---|
| N1a, exhaustive to depth 4, with a partial hold crossed in | 114,552 runs, 0 wrong publishes, 0 other violations |
| N1b, 50 seeds a case | 88,800 runs, 0 wrong, 0 other |
| N5 | no wrong publish in any of its 24 rows (re-run alone; the section in the log's middle is cut at 30 lines) |
| N6 | all 8 rows pass (they failed at `09e3227`) |
| N2c | 0 unsafe rows; `Part` and `All` give the same give-up in every row |

The run counts are the same as at `09e3227`.

## CI on the head

Read with `gh api`, read-only, at 00:06 UTC on 2026-11-26, the head still `5259bf5`. **All ten runs on `5259bf5` completed with success.**

- **Green with no further reading:** `ci`, `coverage`, `e2e`, `k3s-smoke`, `skyzfs-ci` (on the push), `skyzfs-control-rig`, `skyzfs-nodeb-rig` and `skyzfs-publish-rig`.
- **`ci`** (`lint`, `test`, `unit-arm64`): fmt, workspace clippy, the workspace tests and the features' clippy and tests are green on amd64 and arm64. So is the new `cargo doc` step in `lint` (plain, `skyzfsd/test-hooks`, `skyzfs-ublk-sys/control-test-program`).
- **`skyzfs-cut-daemon`:** green on `ubuntu-26.04`, `ubuntu-26.04-arm` and `ubuntu-26.04-upload`. On the upload image every course reads `upload_course: PASS`, `uv1` and `uv1moved` among them; the end-row race was fixed in #152, now merged.
- **`skyzfs-daemon-rig`** (on `skyzfsd`, in-place store): on `ubuntu-24.04`, `ubuntu-26.04` and `ubuntu-26.04-arm`, the judge's lines read the same as at `09e3227`:
  - 53 pins `PASS`;
  - 1 `WAITING`: `pin_failed_volume_while_a_request_is_held`, not built, as the guide says;
  - 1 pin `NA` and 1 control `NA`, both named by the in-place store's expected table;
  - the five controls that must fail, fail.
  - The jobs are green, so the judge exited 0. They built the head merged with `f8e5e0c`, which is still the tip of `zfs-topology`.
- **`skyzfs-replay-rig`** is not triggered by this pull request (see the third look).
- **None of these drives the program** (the publish rig's agent is still the Python stand-in), and none builds `skyzfs-agent-cli` with `test-hooks` (L3/T1). A green CI says nothing about C1.
- **Codex:** its code review of `5259bf5` completed with one finding, C1, which is unresolved. Its security review completed with no findings.

## What blocks the merge

1. **C1**, if the lead rules as for N5. I recommend fixing it here with F3 or its re-stamping variant, plus a test of the form of D3. In any case, the author must answer Codex's new thread on `5259bf5` before "ready to merge".
2. Nothing else. N6, suggestion 1 and the two old Codex threads are settled. T1 and the nits do not block.

## Decisions for the lead

1. **C1:** does it block, as N5 did? It is a regression from L1 of this pull request, narrower in its trigger than N5's (a save or stop over 40 s, against a wall-clock step), and the fix is a few lines. I recommend fixing it before the merge.
2. **Still open, as ruled:** L3/T1 (`skyzfs-agent-cli/test-hooks` in CI's clippy, tests and doc) and the follow-up for S1, S8 and the rest of T2.

## Files (in `review/m2.6c3/`)

- `fourth_look.md`: this note.
- `fourth149_scratch.rs`: D1, D2 and D3. It is included as `#[path = "review_scratch_d.rs"] mod review_scratch_d;` from `engine/tests.rs` in a scratch worktree, and never committed to the product.
- `fourth149_scratch.out`: D1, D2, and D3 at the head and with F3.
- `fourth149_search.out`: the third look's scratch re-run at this head.
- `plants149d.py` and `plants149d.out`.
- `gates149d.sh` and `gates149d.log`.

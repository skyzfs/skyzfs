# Third look at #149 (M2.6c-3, the program `skyzfs-agent`) at 6d1841a, and its merge 09e3227

- **Head now:** `09e322790c8bd2ca2078a9635bc9ad3f6a92000c`. It merges `zfs-topology` at `a5e883a` (with #147 and #151 merged there) into `6d1841a`. **Base:** `zfs-topology`; the pull request's base sha is now `a5e883a`.
- **Head of the review proper:** `6d1841a46bde7be423fa37f5f0fcd25a23840fe1`. The base was then `8f0390f`.
- **Which head each part was judged at:**
  - The code, the three questions, N6, the scratch tests, the searches and the plants were judged at `6d1841a`. They hold at `09e3227` because the package's files are byte for byte the same there (next section).
  - The merge `09e3227`, the gates and CI were judged at `09e3227`.
- **Previous look:** `0ad7944` (`next_look.md` here, posted on the pull request as comment 7173804184). N5 blocked by the lead's ruling, plus T-a, T-b and nits.
- **The author's answer:** https://github.com/skyzfs/skyzfs/pull/149#issuecomment-7175161277
- **Diff reviewed:** `git diff 0ad7944...6d1841a`, in three parts:
  - `9d1e80a`: the PR's own change. N5's fix and test, T-a, T-b, nits 2-6.
  - `03793ac`: the merge of #147's head `66b9044`, read with `git show --remerge-diff`.
  - `6d1841a`: docs only.

## The new head 09e3227: the merge of zfs-topology

The merge brings zfs-topology and changes nothing of the package.
- **Parents:** `6d1841a` and `a5e883a`, the tip of `origin/zfs-topology`. #147 is merged there as `f927674` (its head `66b9044`, which `03793ac` had merged) and #151 as `d8f7f7c`.
- **No conflict and no hand edit:**
  - `git show --remerge-diff 09e3227` is empty.
  - `git merge-tree --write-tree 6d1841a a5e883a` exits 0 with tree `75ee4d3`, which is `09e3227`'s tree.
- **What the merge brings** (`git diff 6d1841a 09e3227`): 25 files, +2036/-32, all of them zfs-topology's. These are the workflows, the zfs-cli record, the txg probe, the dsync writer, `skynbd-*`, `skyzfsd`, `skyzfs-local` and the docs. None is in `skyzfs-agent`, `skyzfs-agent-cli` or `skyzfs-control`. Those three crates depend only on each other.
- **The pull request's own diff against zfs-topology** (`git diff a5e883a 09e3227`) is this package alone: 16 files, +4056/-151.
  - The files are `Cargo.toml`, `Cargo.lock`, `bins/skyzfs-agent/` and `crates/skyzfs-agent/`.
  - The patch is identical to the one `6d1841a` carried over `66b9044` for those 16 files.
  - Every one of the 17 files the branch changed since `66b9044` has the same blob at `6d1841a` and `09e3227`.
- **One file left the diff:** `crates/skyzfs-ublk-sys/src/queue.rs`.
  - The branch had a doc-link fix there, from `af50814`. zfs-topology made the same change in `cd4d0b5` ("Docs: cargo doc without warnings"), and both sides have the same blob.
  - So it drops out of the diff. As a result, the replay rig is no longer triggered on this pull request.

## Verdict at 09e3227

**Can merge at `09e3227`.** CI is green there (read at 22:49 UTC), and no finding blocks. The code was judged at `6d1841a` and is unchanged at `09e3227`.

- **N5 is fixed**, exactly as verified at the last look (fix probe F1).
  - Its test is the review's N5, and it kills the plants the author names.
  - T-a and T-b are pinned.
  - The nits are answered.
- **The merge of #147's head is resolved correctly.** The matrices' oracle takes both sides, and each side's exception is load-bearing.
  - In a person's give-up, `Held::Part` is planned exactly as `Held::All`, both by the exhaustive table and by the searches. So B1's rule holds with a partial hold.
  - Reporting `HeldAway` for `Part` is right.
- **One new finding, N6.** It is a defect of the engine's contract, but the program cannot reach it.
  - A withdraw answered in the agent's own flow leaves `memory.withdrawn` set across an ask anew. If that ask's answer is lost and a person gives the publish up in the same process, the hold is released while the request pends. That gives a wrong publish on the fakes.
  - The program never runs a give-up in a process that has settled: `recover` runs first, in a fresh process.
  - The fix is one line, at the same place as N5's.
  - I would not block on it; the lead decides.
- **CI:** all ten runs on `09e3227` are green. This includes the cut-daemon run, with the course `uv1` that failed at `0ad7944` (see "CI on the head").

## Defects

### N6 (should fix; not reachable through the program): a withdraw answered for one request counts for the next

**Where**
- `crates/skyzfs-agent/src/engine.rs:938` sets `memory.withdrawn` when a withdraw is answered `withdrawn`, `unknown` or `expired`. Only `accepted` clears it again (`engine.rs:1194`).
- `crates/skyzfs-agent/src/plan.rs:500` and `:520`: `given_up` reads it as "a withdraw of this agent's own has been answered" for the request under way.
- `engine.rs:1171` is the asking commit, where `memory.accepted` is now set, but `withdrawn` is not cleared.

**Scenario.** On the fakes, scratch test N6 (`third149_scratch.rs`, output in `third149_scratch.out`):
1. The first request is accepted and pends (a lag of 60 s).
2. The pool is exported and imported (`-T`, one group back, after the hold), which gives a new `load_guid`.
3. The agent's own flow withdraws the request (`onward`: `pending` with `moved`). The withdraw is answered `withdrawn`, so `memory.withdrawn` becomes true.
4. The agent looks, then asks anew. The daemon accepts the second request, and the answer is lost (`Err`, or `refused no-answer`). `memory.withdrawn` stays true.
5. A person gives the publish up in the same process (`Agent::recover`, then `settle`, as the N5 test does).
6. `given_up` sees `withdrawn`:
   - with `pending`, it skips the withdraw;
   - with a kept `no-answer`, `silent` is false, so it does not wait.
7. Either way it looks (`absent`) and releases the hold 2 s after the loss, while request 2 pends. The person destroys `pool@t`, and `snap/t` is made on a cut without it.

**Results**

| `moved` | Modes and lost answers | Released while pending | Wrong publish |
|---|---|---|---|
| no (the control: N5's case at skew 0) | Idle and Busy, `Err` and `no-answer` | 0 of 4 | 0 of 4 |
| yes | Idle and Busy, `Err` and `no-answer` | 4 of 4 | 4 of 4 |

**Fix (verified): fix probe F2.** At the asking commit, clear `withdrawn` together with setting `accepted`. A new request may be out from then on, and no withdraw has been answered for it.
- With F2, N6 passes in all 8 rows.
- The whole library suite passes (157 tests), and plant F2 SURVIVED.

**Reach.** `program::execute` (`crates/skyzfs-agent/src/program.rs:608-626`) runs one task and then `settle`: `begin`, `recover` or nothing. `recover` runs on a fresh `Agent` with empty memory, before any round, so a person's give-up never follows the agent's own rounds in one process.

Inside the engine, stale `withdrawn` is read in two places, and neither is reached:
- In `given_up`, after a give-up's own withdraw, no ask follows.
- In the age bound's branch (`onward`, `ended`), a `pending` request is always withdrawn, whatever `withdrawn` says.

So the program cannot reach this. It is a hole in the engine's contract as its own tests state it: `an_ask_whose_answer_is_lost_keeps_its_hold_on_the_clock_that_does_not_step` exercises the same-process give-up.

**Certainty:** certain on the fakes.

**Recommendation:** fix with F2, plus a row of the shape of N6 in the N5 test (the request withdrawn and asked anew before the loss). Not blocking.

## Suggestions and nits

1. **The `Error::Unmarked` text** (`engine.rs:370-371`) gives `zfs release -r skyzfs-publish <snapshot>`, while the message lists names `t`.
   - `<pool>@<name>` would say what to type.
   - With #147's partial holds, the remedy may fail on a member that has no tag, depending on how ZFS treats `-r` there. That is #147's ground; I have not checked it.
2. Nit 1 of the last look (the stall test's doc) is left as it was, as I suggested; N3 is unchanged at this head.
3. **Codex's two threads have no answer from the author** (a process item, read at `09e3227`). Both findings were made at `af50814`, and both are fixed in `2d806e8`:
   - P1, "Keep the hold when recovery receives an inconclusive answer" (`plan.rs`), is the review's B1. Plants B1a-h are killed.
   - P2, "Track the publish across snapshot-name collision retries" (`program.rs`), is the review's D2. It is pinned by `a_publish_made_under_its_next_name_exits_by_its_outcome`, and plants D2a-d are killed.
   - Codex's code review of `09e3227` completed with no findings, and its state says no unresolved findings. Its security review completed and posted nothing.
   - The guide asks the author to answer Codex on its thread. One line on each thread, pointing at `2d806e8`, would close this.

## The three questions

### (1) N5's fix and its test

**The fix is F1 as verified at the last look.** The only difference is that both clocks are read together, before the asking commit (`engine.rs:1152-1153`), and the pair is stored right after it (`engine.rs:1171`). That is equivalent, and better: `sent` and `sent_mono` are one instant.
- `now_for` (`engine.rs:849`) then reads the clock that does not step for any publish asked in this process.
- `accepted` overwrites the pair. A refusal that says nothing pends puts the record back to `Held`, where `now_for` does not read it.

**The test** `an_ask_whose_answer_is_lost_keeps_its_hold_on_the_clock_that_does_not_step` (`engine/tests.rs:2878`) is N5, with its 24 combinations:
- three modes;
- `Err` and `no-answer`;
- the agent's own flow and a same-process give-up;
- skew 0 and +2 h.

It also checks that no hold is left.

**Re-run at this head**

| Check | Result |
|---|---|
| The review's N5 | passes, 24 of 24 |
| N1a, exhaustive to depth 4, now crossed with a partial hold | 114,552 runs: 0 wrong publishes, 0 releases while the request pends, 0 other violations |
| N1b, 50 seeds a case, now crossed with a partial hold | 88,800 runs: 0 wrong publishes, 0 releases while the request pends, 0 other violations |
| N2, the planner | 0 unsafe rows; the same 289 rows differ from the age bound as before |

"Given up then made (whole)" occurs only with the daemon unable to answer through `held_until`, now documented on `Why::Person`.

**Plants** (each alone, on a clean worktree; table below):
- N5a (the fix removed) and N5b (the pair stored with an `asked_ms` that does not match) are KILLED by the new test.
- L1j is KILLED by the new test.
- L1b is KILLED by `the_record_says_asked_before_the_request_is_sent`.
- These were the last look's survivors. None survives now.

### (2) The merge of #147's `66b9044` (`03793ac`)

**Textually**
- `git merge-tree` of the two parents conflicts in three files, as the author says.
- `git show --remerge-diff 03793ac` shows the conflicts resolved by taking both sides, plus four changes outside the conflict markers:
  - `plan::report` reads `hold != Held::No` (needed to build, since `pool.held` is gone);
  - #147's new engine test takes the hook's second argument (needed to build);
  - the comment "No person gives a publish up in the matrices";
  - one new row in `a_publish_too_old_held_for_a_daemon_away_is_reported` (`partly()` reported).
- Nothing else changed.
- `6d1841a` touches only docs, in `engine.rs` and `record.rs`.

**The oracle** (`engine/tests.rs:312-362`) takes both sides:
- `Why::TooOld` is confirmed only where the log has "partly released t" (#147).
- `Why::Person` is never confirmed (this PR).
- `Report::Refused` is accepted only for a partly released publish (#147).
- `Report::HeldAway` is an error (this PR).

Each of #147's exceptions is load-bearing: plants O1 (the refusal's exception removed) and O2 (`TooOld` never confirmed) are both KILLED by the matrices.

**`Held` against this PR's give-up and stop paths**
- **N2c**, exhaustive over phase × answer × sight × withdrawn × time × too_old × hold (No, Part, All); 13,440 rows:
  - A person's give-up gives the same action for `Part` and `All` in every row (0 differ).
  - No row releases while a request may land, before `held_until`, with no withdraw of the agent's own answered.
  - With `Held::No` the give-up abandons instead (no hold to release).
  - The age bound differs between `Part` and `All` in 720 rows: #147's hold again and withdraw, by design.
- **The searches** above were crossed with a person's release of `pool/db@t` before the give-up (`User::ReleaseChild`): 0 wrong publishes and 0 releases while pending, with a partial hold as with a whole one.
- **B1's rule** gates on the phase, the answer and `withdrawn`, never on the hold. The hold decides only between `Release` and `Abandon` in `give_up` (`plan.rs:607-614`), and anything but `No` is released. This includes the person's give-up from `Intent`/`Taken` and a done publish.
- **The person's stop** (`Person::Stopped`, `Action::Stop`) does not read the hold.
- **Plants on `Held`'s readings** (H1-H4: `== All` in place of `!= No` in the report, `give_up`, a done publish and `away`) are each KILLED, so the merged tests pin every place where `Part` must count as a hold.

### (3) `plan::report`: `HeldAway` for `hold != Held::No`

Right.
- `HeldAway` says that a publish too old waits on a daemon that does not answer while the agent keeps a hold.
- With `Part`, the plan waits exactly as with `All`. `onward`'s `away()` releases for anything but `No` at `held_until`, and waits before it. N2c finds the reports for `Part` and `All` equal in every row.
- A partial hold still stops a person destroying those members' snapshots, so it is worth reporting.
- The new test row pins it, and plant H1 (`== All`) is KILLED.

## Verification

### Gates (`gates149t.log` at `6d1841a`, `gates149m.log` at `09e3227`)

Run with rustc 1.100.0 (stable after `rustup update stable`), with my own target directory and `debug=0`. The results are the same at both heads:

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy -p skyzfs-agent -p skyzfs-agent-cli -p skyzfs-control --all-targets -D warnings`, plain and with `--features skyzfs-agent-cli/test-hooks` | pass |
| Tests, plain and with hooks: 157 library, 7 binary, 16 `skyzfs-control` | pass |
| `cargo doc` with `RUSTDOCFLAGS=-D warnings` | pass |

### Scratch tests (`third149_scratch.rs`; outputs `third149_search.out`, `third149_scratch.out`)

These are the last look's tests, adapted to `Held`:
- N1a and N1b, with a partial hold crossed in, and the strict check (no release while the request pends at the daemon);
- N2, N2b, N3, N4 and N4b, unchanged;
- N5;
- new: N2c (the planner with the hold) and N6 (above).

N3 still fails by design: it asserts the stall window has no slack, and the 12 known unobservable stops remain. No change was asked there.

### Plants (`plants149t.py`, `plants149t.out`)

Each plant was applied alone as text to a clean worktree of the head, with a 600 s limit, and the files restored in `finally`. The worktree was clean afterwards.

| Plants | Verdict | Caught by |
|---|---|---|
| B1a-h | KILLED, all 8 | `plan::tests::a_person_gives_up_in_the_order_of_every_give_up`; B1a and B1b also by `engine::tests::a_person_s_give_up_keeps_the_hold_while_the_uploader_is_silent` |
| L1a-i | KILLED, all 9 | `the_record_says_asked_before_the_request_is_sent`; L1a, L1c, L1e and L1g also by the new N5 test |
| **L1b** (survived the last look) | **KILLED** | `the_record_says_asked_before_the_request_is_sent` (T-b) |
| **L1j** (survived the last look) | **KILLED** | `an_ask_whose_answer_is_lost_keeps_its_hold_on_the_clock_that_does_not_step` (T-a) |
| ST1, ST2 | KILLED | `a_stop_at_any_point_of_a_stalling_run_counts_each_stall_once` |
| D2a-d | KILLED | `program::tests::a_publish_made_under_its_next_name_exits_by_its_outcome` |
| L4a-g | KILLED | the marker's tests, the crash matrices, the mistyped-directory tests, the hooks binary test |
| S2a, S3a, S4a, S6a | KILLED | the command-line tests |
| N5a (the fix removed), N5b (`asked_ms` not matching) | KILLED | the new N5 test |
| H1 (report `== All`) | KILLED | `plan::tests::a_publish_too_old_held_for_a_daemon_away_is_reported` (the merge's new row) |
| H2 (`give_up` `== All`) | KILLED | `a_hold_a_person_partly_released_is_given_up_and_released`, `a_hold_on_some_members_is_released_where_the_publish_ends`, the matrices |
| H3 (done `== All`), H4 (`away` `== All`) | KILLED | `plan::tests::a_hold_on_some_members_is_released_where_the_publish_ends`; H3 also by the matrices |
| O1, O2 (each of #147's oracle exceptions removed) | KILLED | the matrices (`a_persons_action_at_any_point_is_met`, `the_matrices_need_the_rule_r_of_the_daemon`) |
| F2 (fix probe for N6) | SURVIVED, as intended | the fix breaks no test |

No plant survives. The `host::` timing tests did not fail in any run, since the plants ran with nothing else on the machine.

## CI on the head

Read with `gh api`, read-only, at 22:49 UTC. All ten runs on `09e3227` completed with success.
- **Green with no further reading:** `ci`, `coverage`, `e2e`, `k3s-smoke`, `skyzfs-ci` (on the push), `skyzfs-control-rig`, `skyzfs-nodeb-rig` and `skyzfs-publish-rig`.
- **`skyzfs-cut-daemon`:** green on `ubuntu-26.04`, `ubuntu-26.04-arm` and `ubuntu-26.04-upload`.
  - On the upload image, the courses `uv1` and `uv1moved` pass every line (`upload_course: PASS`). Each control is `CAUGHT`, or `SAME` where that is expected.
  - At `0ad7944`, `uv1` failed on this job. The testing session put that down to a race in the rig. This is one green run: it agrees with that account but does not show the race gone.
- **`skyzfs-daemon-rig`** (on `skyzfsd`, in-place store): on `ubuntu-24.04`, `ubuntu-26.04` and `ubuntu-26.04-arm`, the judge's table reads:
  - 53 pins `PASS`;
  - 1 `WAITING`: `pin_failed_volume_while_a_request_is_held`, not built, as the guide says;
  - 1 `NA`, which the in-place store's expected table names;
  - the five controls that must fail, fail.
  - The judge exited 0. The run built the head merged with `a5e883a`, which is still the tip of zfs-topology.
- **`skyzfs-replay-rig`** is not triggered at this head, since the pull request no longer touches `skyzfs-ublk-sys` (see above). It was green at `0ad7944`.
- **The runs on `6d1841a`:** the push of `09e3227` cancelled the long runs still going there (cut-daemon and the daemon rig). They are superseded.
- **Codex:** both reviews of `09e3227` have completed and posted nothing new.
  - The code review reports no findings.
  - The security review's summary field for findings is empty.

## What blocks the merge

Nothing in the code. CI is green on `09e3227`.

Before the author says "ready to merge", Codex's two threads from `af50814` need an answer (suggestion 3).

N6 is for the lead to decide. I recommend fixing it here, but I do not block on it.

## Decisions for the lead

1. N6: fix in this pull request (one line, F2, plus a test row), or as a follow-up? It is not reachable through the program. I recommend fixing it here, since it sits next to N5's fix and the engine's tests treat a same-process give-up as covered. I would not block on it.
2. Still open, as ruled: L3/T1 (the third feature in CI) and the follow-up for S1, S8 and the rest of T2.

## Files (in `review/m2.6c3/`)

- `third_look.md`: this note.
- `third149_scratch.rs`: the scratch tests, included as `#[path = "review_scratch.rs"] mod review_scratch;` from `engine/tests.rs` in a scratch worktree.
- `third149_search.out`: N1a and N1b.
- `third149_scratch.out`: N2, N2b, N2c, N3, N4, N4b, N5 and N6, and N6 with F2.
- `plants149t.py` and `plants149t.out`.
- `gates149t.log` (at `6d1841a`) and `gates149m.log` (at `09e3227`), both made by `gates149n.sh`.

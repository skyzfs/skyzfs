## Review of #138 at 029a48e (F1b: L1, L2, N-a and N-b of #134's review, and T1 of #136's)

**Verdict at 029a48e, the final head: can merge. Nothing blocks.**

- **L1 is the test I meant.** It is the no-row scenario of my notes on #134 (`next_variant.rs` there): cut 1 with no row, another writer's commit `Z` of group 5 and of no group, with the same seeds.
  - It passes at the head: 900 of 900 runs, idle and beside 8 and 16 busy loops on 4 CPUs.
  - It fails with B1H planted: 300 of 300 runs, plus the suite and 5 repeats at each of `fd8f685` and `029a48e`. B1's older test passes with B1H, so this test is the only one that catches it.
  - **It passes with B1's original line planted, and it should.** B1 is wrong only where a row names a commit. With no row, `judge` never reads `head`, so no test without a row can see B1. B1's own test catches it, and with both lines planted both tests fail. My notes said the same of my variant on #134.
  - It is deterministic: there are no timers, and the swap comes at a fixed call.
- **L2 and N-b hold against the code.** I checked every claim of the new text (tables under (2) and (3)).
- **N-a is right.** The two doc lines are back on their test, and the touched test files leave no test without a doc line that had one before.
- **T1 is safe and catches what it says.**
  - The head connected at try 36 in all 301 runs with the probe, idle and beside 8 and 16 busy loops, and passed 100 of 100 more without it. The floor is 20.
  - To fall below 20, the 19 pauses before try 20 (713 ms) would have to take 1.5 s: about 0.8 s of lateness in the connecting thread. The delay check that was already there fails on a stall of 0.2 s near the room.
  - The caps of 500, 400 and 200 ms and the uncapped pause are caught 60 of 60 times each. The cap of 100 ms and CF survive 60 of 60, as the test's doc says.
- **The merge `2930b39` is mechanical.** Its change is #136's, byte for byte. It touches two places near this work:
  - `status.rs`, at another row (offset 976);
  - the upload tests' fixture, which now has `intent_header_sync: true`.
  
  So I ran L1's plants again at `029a48e`. Every verdict is the same as at `fd8f685`.
- **Gates at 029a48e:** fmt and the three clippy runs are clean. The tests give 1700, 1742 and 1724 passed, 0 failed. That is the description's counts at `fd8f685` plus #136's three tests.
- **CI on 029a48e:** every run is green (table under (6)). The daemon rig's table is the same as on #134's head.
- **Codex** completed its code review and its security review of `029a48e` with no findings.

**Findings, most severe first. None blocks.**

| Id | Severity | Where | What | Certainty |
|---|---|---|---|---|
| D3 | the description | "How it was verified", "CI" | The gates shown are those at `fd8f685` ("will be replaced"). At `029a48e` the counts are 1700, 1742 and 1724: three more than shown, from #136's tests, so four more than at `7989f5e` and not one. The CI line still says "to be added", and the runs on `029a48e` have ended green. | high |
| n1 | nit, a test's doc | `bins/skyzfsd/src/control/tests.rs:461-462` | "at 50 ms there are about 36 before the connection": `tries` counts the try that connects too, so 35 fail before it. The 16 and the 22 are counted the same way, so the comparison with 20 holds. Optional. | high |

Nothing else found. The description's "Not done" names only some of #134's points as settled. N4 and P1 were answered on #134, and G3 was passed to testing, so "Nothing" is still right.

**How I worked.**
- I read `git diff 69837ba...fd8f685` first, then my notes on #134 (`review/m2.f1/review.md`, `next_look.md`), and the description last.
- The head moved to `029a48e` while I worked. I then read `git diff fd8f685 029a48e`: the merge, and T1, which is `a1618ce` from #136's branch, byte for byte.
- Plants: each plant was put in alone, textually, with each site found exactly once. Each ran under a subprocess time limit and was restored in a `finally`, in a worktree of its own, and the worktree was clean after every script.
- Repeat runs used the built test binaries, so that a run times the test and not cargo.
- The gates ran in another clean worktree, after `rustup update stable` (rustc 1.100.0, 2026-11-09, unchanged, the current stable).
- Paths: `remote.rs`, `check.rs` and `driver.rs` are in `crates/skyzfs-upload/src/`, and `tests/driver.rs` and `tests/common/mod.rs` are in `crates/skyzfs-upload/`, unless a path says otherwise. Line numbers are at `029a48e`.

**Files in this folder:**
- `plants.py`, with `plants_fd8f685.{out,json}` and `plants_029a48e.{out,json}`: L1's plants at both heads.
- `l1_build.py`, `l1_repeat.py`, `l1_repeat.{out,json}`: L1's repeat runs, at the head and with B1H planted.
- `t1_build.py`, `t1_build.{out,json}`: T1's plants, one binary each, with a probe line that prints the tries and the delay.
- `t1_repeat.py`, `t1_repeat.{out,json}`: T1's repeat runs.
- `t1_unprobed.out`: the head's own test binary, without the probe, beside 16 busy loops.
- `gates.sh`, `gates.out`: the gates at `029a48e`. `gates_fd8f685_partial.out`: the gates at `fd8f685`, stopped when the head moved.
- `upload_job_029a48e.txt`: the result lines of the cut rig's upload job on `029a48e`.

### (1) L1: the race at cut 1 with no row

**The test.** It is `head_swapped_before_the_first_cut_is_judged_by_the_group_the_engine_found` (`tests/driver.rs:1275-1334`).
- It is my variant of B1's test (scenario S1 without a row) with the same seeds, 93 and 94, and the same two cases: `Z` of group 5, and `Z` with no group.
- What it adds:
  - in the group 5 case, block 0 is still `Z`'s `0xCD`;
  - in the no-group case, the exact steps `[Began 1 Whole, Uploaded 1 Whole]`, and `assert_uploaded(7)` (head holds the volume, the row names head's commit, fsck passes).
- My `before.row.is_none()` is gone. The fixture is new, so there is no row, and the group 5 case asserts that the row is still absent afterwards.
- The answer on #134 said "B1's test gains your no-row case". It is a separate test beside B1's, using the same `Swapping` refs. That is equivalent.

**It reproduces the race.**
- `Swapping::get` (`tests/driver.rs:1118-1133`) runs the other writer's commit after the inner read of `head` returns, and before handing the read back.
- `EngineRemote::new` reads nothing. `observe` reads the row with `meta_get` and then head with `get`: that read is the one that triggers the swap. `Engine::open` then reads head again and sits on `Z`.
- Proof that the swap falls between the two reads and not before both: B1H fails the test. If both reads saw `Z`, B1H's first-read group would be `Z`'s too, and the test would pass.

**Plants** (`plants.py`; the whole crate's suite, then each race test 5 more times). The results were the same at `fd8f685` and at `029a48e`. Unplanted, the suite gave 176 passed and 0 failed at each.

| Plant | What is planted | Suite | New test | B1's test |
|---|---|---|---|---|
| B1H | `head_group: engine.commit_group(&head.commit).await?` in the last `Observed` (`remote.rs:235`): the group of head as first read | **KILLED**: 1, the new test only | fails, 5 of 5 more: `[Began 1 Whole, Uploaded 1 Whole]` where `[Mismatch HeadMoved, abandoned [1]]` was due (group 5) | passes 5 of 5 |
| B1 | `head: head.commit` (`remote.rs:237`), B1's original line | **KILLED**: 1, B1's test only | passes 5 of 5 (by design: no row names a commit) | fails 5 of 5: cut 2 goes up as a delta onto `Z` |
| B1B1H | both lines | **KILLED**: 2 | fails 5 of 5 | fails 5 of 5 |
| B1RH | control: both lines, and the check observes again where not `current` (a correct fix of another shape) | SURVIVED (as expected) | passes | passes |
| NR | with no row a whole cut goes up whatever head's group (`check.rs:117`) | **KILLED**: 3, the new test among them | fails 5 of 5 | passes |
| NC | stricter: with no row the check refuses wherever the engine was opened on another head than `observe`'s first read | **KILLED**: 1, the new test only (its no-group case) | fails 5 of 5: `[Mismatch HeadMoved]` where the cut was to go up | passes |

- B1RH surviving shows that the test checks the outcome, not a line.
- NC shows that the no-group case is not vacuous: it pins that a race onto a commit with no group still lets the cut go up whole onto it, as the rule says.

**Repeat runs at 029a48e** (`l1_repeat.out`; 4 CPUs):

| Binary | Idle | Beside 8 busy loops | Beside 16 busy loops |
|---|---|---|---|
| head, the new test | 300 of 300 pass | 300 of 300 pass | 300 of 300 pass |
| B1H, the new test | 100 of 100 fail | 100 of 100 fail | 100 of 100 fail |
| head, the whole driver binary (25 tests in parallel) | | | 30 of 30 runs pass 25 of 25 |

There are no timers in the test, and the swap is injected at a fixed call, so the result cannot depend on scheduling. No run differed from the others.

### (2) L2: the doc of `Observed::head_group` (`remote.rs:81-88`)

| Claim | The code | True? |
|---|---|---|
| "The group of the commit in `head`" | Branches A (at head) and C (opened again) give `engine.head_group()` (`:235`), which is `head_commit.group` (`skynbd-engine/src/lib.rs:856-858`). That is the commit the engine sits on, and `head` is `engine.head_commit_hash()` (`:237`). `Engine::open` loads both from one read of head, and a commit and a rollback set them together (`lib.rs:1835-1836`, `:1079-1080`). Branch B gives `commit_group(&head.commit)` with `head: head.commit` (`:209-214`). | yes |
| "`None` for a commit with none" | `Commit::group` is `None` for a commit made with no group | yes |
| "the engine's own where it is open (at head, or opened again)" | `:235` covers both branches. It reads the commit object's group, not `next_group`, so a group set for a commit that failed cannot leak into it | yes |
| "so that the group judged is that of the commit judged" | `judge(&cut, &row, seen.head, seen.head_group)` (`driver.rs:554`); in every branch the group is that of the commit in `head` | yes |
| "Where the engine is left behind head (the branch with writes), it is read from that commit's object, not taken from the engine" | Branch B is the engine that exists, is not at head by hash, and is not clean (`:208-216`). A clean engine behind head is opened again instead (`:217`). So "left behind head" happens only with writes, and the group is read from head's commit through the engine's resolver | yes |
| "The check reads it only under a row of version 1, which names no commit, or no row" | `check.rs:116-122`: `head_group` is read for `Row::Absent` and for `Named` with `commit: None`. `Unreadable` and `Committed` return first. The driver reads `head_group` nowhere else (`git grep` finds only `driver.rs:554`) | yes |

The doc now matches what my L2 asked: "the group of the commit in `head`", and "the branch with writes" in place of "not `current`". It also uses the same words as the doc of `head` just below it ("where the engine is open (it was at head, or was opened again)").

### (3) N-b and N-a

**N-b** (`crates/skyzfs-ublk/src/status.rs:156`). The new text of code 6 is: "… names the cut's base with head's group, but no commit: the first cut of part of the volume after an upgrade, or one after head moved within the row's group, which a row of version 1 cannot show."
- `judge` gives `RowV1` only after `NotTheBase` (`check.rs:132`), so the row names the cut's base. It needs `named.commit.is_none() && head_is_the_rows` (`check.rs:135`), which for a version 1 row is `head_group == Some(named.cut.group)` (`:120`).
- Under such a row, head is either the commit the base went up as (the upgrade, nothing moved) or another commit of the row's group (head moved within the group). Those two are the cases the text names. Nothing else reaches code 6.
- "which a row of version 1 cannot show" is right: the row names no commit, so the two cases cannot be told apart. `Mismatch::RowV1`'s doc says the same ("head is not known to be the commit the base went up as", `check.rs:43-45`).
- The row is still one line of the doc's table, with no `|` in the new text. Only that line changed.
- The description says `check.rs` "already said so". It says it in substance, not in these words. That does not matter.

**N-a** (`crates/skyzfs-ublk/src/status/tests.rs`).
- `:1111-1113` now opens `each_finding_of_the_check_keeps_its_code_and_its_word` with its own three lines.
- `:1147-1148` are back above `the_uploaders_post_is_kept_by_the_stores_and_written_on_its_own` (`:1150`). That is where they stood at `75bed3f`.
- I counted tests without a `///` line at both the base and the head:
  - `status/tests.rs`: 8 at the base, 7 at the head. The one fixed is this test. The other seven were already like that at the base, and this pull request does not touch them.
  - `tests/driver.rs`: none at either.

**Nothing unrelated** is in `fd8f685`. Its four files are the four the description names. `remote.rs` changes only a doc comment. T1 changes one test's doc and adds one assertion.

### (4) T1: the floor on the connect test's tries

**What it changes** (`bins/skyzfsd/src/control/tests.rs:454-490`, under `#[cfg(target_os = "linux")]`):
- the doc of `a_connect_is_made_soon_after_the_backlog_has_room`;
- `assert!(tries >= 20, …)` after the delay check.

`029a48e`'s diff is `a1618ce`'s, byte for byte.

**The pause** (`bins/skyzfsd/src/control.rs:721-751`): 1 ms, doubling to `CONNECT_PAUSE`, 50 ms. Each try counts, the one that connects included.

**When each try falls.** With no lateness, try *n* falls at 63 + 50(*n* − 7) ms. Room is made at 1500 ms, so try 35 falls at 1463 ms and try 36 at 1513 ms. The head connects at try 36.

**Is 20 safe?**
- With no lateness, try 20 falls at 713 ms.
- For the head to stop below 20 tries, the 19 pauses before try 20 would have to take 1500 ms instead of 713. That is about 0.8 s of lateness in the connecting thread, in its first 0.7 s.
- The room thread's own lateness only adds tries.
- The delay check that was already there fails on any stall over 0.2 s that covers the room. So the floor adds a way to fail only for a stall about four times as long, and only early on.
- Measured (`t1_repeat.out`, `t1_unprobed.out`; 4 CPUs):

| Binary | Idle | Beside 8 busy loops | Beside 16 busy loops |
|---|---|---|---|
| head, with the probe | 100 of 100 pass; tries 36 in all; delay 15.5 / 16.1 / 19.1 ms | 100 of 100; tries 36 in all; 7.3 / 12.1 / 24.2 ms | 100 of 100; tries 36 in all; 13.6 / 27.8 / 47.2 ms |
| head, its own binary (no probe) | | | 100 of 100 pass; and the whole `skyzfsd` library (142 tests in parallel) 5 of 5 |

(Delays are min / median / max.)

- The head was at try 36 in every one of its 300 repeat runs and in its build run, 301 in all. I never saw 35, which my look at #136 had also seen.
- The largest delay was 47 ms, a quarter of the 200 ms bound. A floor of 20 is 16 tries below what the head does.

**Does it catch the caps?** (`t1_build.out`, `t1_repeat.out`; each plant 20 runs per condition.)

| Plant (`control.rs`) | Tries in every run | Delay (min to max over the three loads) | Idle / 8 / 16 busy loops | Caught by |
|---|---|---|---|---|
| CE: `pause = pause * 2`, no cap | 12 | 540 to 580 ms | 20 / 20 / 20 fail | both checks |
| CE5: cap 500 ms | 12 | 3.9 to 36 ms | 20 / 20 / 20 fail | **the floor only** |
| cap 400 ms | 13 | 204 to 232 ms | 20 / 20 / 20 fail | both (the delay by 4 to 32 ms) |
| cap 200 ms | 16 | 148 to 188 ms | 20 / 20 / 20 fail | **the floor only** |
| cap 100 ms | 22 | 23 to 72 ms | 20 / 20 / 20 pass | survives, as the doc says |
| CF: first pause 50 ms | 31 or 32 | 0.1 to 48 ms | 20 / 20 / 20 pass | survives, as the doc says |

- Without the floor, CE5 and the cap of 200 ms pass. Under 16 loops the cap of 200 ms came within 12 ms of the delay bound, so the delay check alone would catch it only now and then.
- With no lateness, the floor catches any cap of 125 ms or more: a cap of 125 ms connects at try 19, and one of 124 ms at try 20. Lateness only lowers the count.
- **"Not caught here or above"** holds. With the cap of 100 ms and with CF, all 16 tests of the `socket` module pass, the test above (`a_full_backlog_is_tried_again_after_pauses_that_grow`, 2 to 12 tries in 300 ms) among them.
  - With no lateness, a cap of 100 ms makes 10 tries in 300 ms, and CF makes 7.

**The doc's figures match the code and the runs.**
- CE5 "tries at 1511 ms": 1 + 2 + … + 256 = 511, then 1011, then 1511.
- 36 at 50 ms, 16 at 200 ms, 22 at 100 ms: measured exactly.
- "costs a connect 100 ms at most" and "50 ms" for CF: those are the longest pauses.
- n1 is only that "36 before the connection" counts the try that connects.

The description's T1 table matches mine: 12 tries and 12.35 ms for CE5, 16 and 157.97 ms for the cap of 200 ms, 548.5 ms at try 12 for CE, 22 tries for the cap of 100 ms.

### (5) The merge `2930b39` and the description

**The merge.**
- `git diff 2930b39^1 2930b39` and `git diff 69837ba b9def12` differ only in one `index` line, for `status.rs`, whose base differs. So the merge brings exactly #136.
- #136's files that this pull request also touches, or uses:
  - **`crates/skyzfs-ublk/src/status.rs`**: the doc of offset 976 (`:162`), another row of the table than N-b's (`:156`). Both changes are in the head.
  - **`crates/skyzfs-upload/tests/common/mod.rs:155-157`**: `Fixture::start` now opens the store with `intent_header_sync: true`. L1's test uses this fixture. It passes at `029a48e`, and every L1 plant gives the same verdict as at `fd8f685` (`plants_029a48e.out`).
  - **`bins/skyzfsd/src/control/tests.rs`**: #136's connect test, which T1 then changes. That is expected.
- Its other files are `cli.rs` and its tests, `skyzfs-local`'s `fs.rs` and `inplace/tests/cuts.rs`, and a reflow in `skynbd-refs`. Nothing of this pull request's.

**The description against the diff.**
- The three commits, the L1, L2, N-b and N-a bullets, the T1 bullet and its table, and "B1's earlier test passes with the plant" all match what I found.
- The gates and the CI line are stale (D3).
- The title starts with the work package's id.

### (6) Gates, CI and Codex at 029a48e

**Gates** (`gates.out`; rustc 1.100.0, the current stable):

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| the same with `--features skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean |
| the same with `--features skyzfs-ublk-sys/control-test-program` | clean |
| `cargo test --workspace` | 1700 passed, 0 failed, 16 ignored |
| the same with the test hooks | 1742 passed, 0 failed, 16 ignored |
| the same with the control test program | 1724 passed, 0 failed, 18 ignored |

These are the description's counts at `fd8f685` plus the three tests #136 added: the connect test, `the_drop_of_a_file_waits_for_the_sync_its_helper_is_doing`, and `a_release_on_a_failed_store_is_named_by_the_acknowledging_start_after_a_power_cut`. At `fd8f685` fmt and the three clippy runs were clean too (`gates_fd8f685_partial.out`).

**CI on 029a48e.**
- `029a48e` already contains `zfs-topology` at `b9def12`, its tip. So the pull request's runs built the head itself.
- On `fd8f685`, `ci`, `coverage`, `e2e`, `k3s-smoke` and `skyzfs-ci` ended green. The four rigs were cancelled by the push of `029a48e`.

| Workflow | Run | Jobs |
|---|---|---|
| ci | [45513894815](https://github.com/skyzfs/skyzfs/actions/runs/45513894815) | changes, test, lint, s3-backends, chart-lint, unit-arm64: success |
| coverage | [45513894939](https://github.com/skyzfs/skyzfs/actions/runs/45513894939) | changes, coverage: success (badges skipped) |
| e2e | [45513894923](https://github.com/skyzfs/skyzfs/actions/runs/45513894923) | changes, e2e-tier-a: success |
| k3s-smoke | [45513894903](https://github.com/skyzfs/skyzfs/actions/runs/45513894903) | UDS smoke, install smoke: success |
| skyzfs-ci | [45513884851](https://github.com/skyzfs/skyzfs/actions/runs/45513884851) | the push: gate success; the rest skipped, as the pull request's own runs cover them |
| skyzfs-control-rig | [45513894770](https://github.com/skyzfs/skyzfs/actions/runs/45513894770) | the table and its judge; ubuntu-24.04, ubuntu-26.04, ubuntu-26.04-arm: success |
| skyzfs-cut-daemon | [45513895028](https://github.com/skyzfs/skyzfs/actions/runs/45513895028) | which jobs; the check's logic; record through skyzfsd and check on ubuntu-26.04, ubuntu-26.04-arm and ubuntu-26.04-upload: success |
| skyzfs-replay-rig | [45513894804](https://github.com/skyzfs/skyzfs/actions/runs/45513894804) | which server; log reader and planning; through skyzfsd, and write into a hole, on ubuntu-24.04 and ubuntu-26.04: success; the reference's replay jobs skipped |
| skyzfs-daemon-rig | [45513894817](https://github.com/skyzfs/skyzfs/actions/runs/45513894817) | which server; the table and its judge; ubuntu-24.04, ubuntu-26.04 and ubuntu-26.04-arm on the daemon: success |

**What the rigs' tables say, not only their colour.**
- **The cut rig's upload job** (`upload_job_029a48e.txt`):
  - all six courses (life, umoved, urow, ukill, ulease, ulost) PASS `upload_content`, `upload_status`, `upload_balance` and `upload_reads`;
  - the five other than life PASS `upload_course`;
  - umoved shows `head_moved` and urow `not_the_base`;
  - worst exit 0, and no FAIL line.
- **The daemon rig:** on each of the three images, the step that runs the judge (`daemon_check.py`) passed, so the judge's exit was 0 and every line is as its table `EXPECTED` says. The job logs give the same lines on all three as on #134's head:
  - 53 pins PASS;
  - one NA, `pin_c_kill_with_applied_unacknowledged` (the in-place store keeps no log);
  - one WAITING, `pin_failed_volume_while_a_request_is_held`;
  - five controls caught (FAIL), and one control NA, `ctl_delay_before_apply`.

**Codex.** Its summary comment records two reviews of `029a48e`:
- the code review, completed with `has_any_findings: false`;
- the security review, completed.

There are no review comments and no reviews on the pull request.

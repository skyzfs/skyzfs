## Next look at #134 (F1) at 75bed3f

**Verdict: can merge. Nothing blocks.**
- B1 is fixed, and its test pins the fix.
  - `8e6544d` is the line proposed in the review: the last `Observed` gives `head: engine.head_commit_hash()` (`remote.rs:235`).
  - The new test is S1. It fails in 5 of 5 runs with the old line put back, and passes in 10 of 10 runs without it.
- Two low findings, L1 (a gap in the tests) and L2 (a line of documentation). Neither blocks.
- One note on the description, D1. It does not block.
- `current` and `head` are right in every case I could construct (table under (2)).
- The merge `75bed3f` is mechanical. Nothing in it touches a file of this pull request. The gates pass at the head.
- Every CI run on 75bed3f is green, and the rigs' tables agree with their colour (under (6)).
- Codex finished its code review and its security review of 75bed3f, and posted no findings.

**How I formed the view.**
- I read `git diff 4595f55 8e6544d` (the fix) and checked the merge on its own (under (5)).
- I read the author's answer (https://github.com/skyzfs/skyzfs/pull/134#issuecomment-7145881930) only after that, and it agrees with what I found.
- The plants and the variant ran in a clean worktree at 75bed3f. Each plant was restored afterwards, and the worktree was clean at the end.
- The gates ran in a clean checkout of 75bed3f.
- Paths: `remote.rs`, `driver.rs`, `check.rs` and `lib.rs` are in `crates/skyzfs-upload/src/`, and `tests/…` is `crates/skyzfs-upload/tests/`, unless a path says otherwise.

**Files in this folder from this look:**
- `next_plants.py`, `next_plants.out`, `next_plants.json`: the plants and the repeat runs.
- `next_variant.rs`, `next_variant.sh`, `next_variant.out`: L1's variant test.
- `next_baseline.out`: `cargo test -p skyzfs-upload -p skynbd-engine` at 75bed3f, unplanted.
- `next_gates.out`, `next_gate_counts.py`: the gates, and the script that counts the tests by binary.
- `next_upload_job_75bed3f.txt`: the cut rig's upload courses on 75bed3f.

### Findings, most severe first

#### L1 (low; a gap in the tests; does not block): B1's test pins `head`, not where `head_group` comes from

**The code at the head is right.** `remote.rs:233` takes `head_group` from the engine. So the group the check judges is the group of the commit the engine sits on, the same commit `head` now names (`remote.rs:235`).

**But no test holds it there.**
- Plant B1H puts the group of head as first read there: `head_group: engine.commit_group(&head.commit).await?`. `head` stays the engine's.
- The whole crate's suite survives B1H (`cargo test -p skyzfs-upload`: 0 failed).
- B1's test cannot see it. Its three cases all have a row of version 2, and `judge` reads `head_group` only under a row of version 1 or no row (`check.rs:105-110`).

**Scenario: B1's race where there is no row (or a row of version 1) and the cut is whole.**
- Cut 1 of a new volume, so there is no row. Another writer commits `Z`, of group 5, onto head between `observe`'s read and `Engine::open`'s read.
- With B1H, the check takes head's group as `None`, from the first read. Rule 3 then lets the whole cut go up, and cut 1 goes up onto `Z`, moving head off the other writer's commit.
- At the head, the check sees `Z`'s group 5 and says `HeadMoved`, and head stays `Z`.

**I wrote that case as a test** (`next_variant.rs`: the race at cut 1 with no row, with `Z` of group 5 and with no group; `next_variant.sh` appends it to `tests/driver.rs` in the worktree and takes it out again). Three runs each:

| Tree | Result |
|---|---|
| 75bed3f | passes, 3 of 3 |
| B1H planted | fails, 3 of 3: `[Began Whole, Uploaded Whole]` where `[Mismatch HeadMoved]` was due, so cut 1 goes up onto `Z`, which has a group |
| B1 planted (the old line) | passes, 3 of 3: B1 needs a row that names a commit, which this case lacks |

**Why it is worth a case.**
- The code was never in B1H's state: the base already took `head_group` from the reopened engine. The risk is a later change.
- The dirty branch reads `engine.commit_group(&head.commit)` (`remote.rs:207`). A change that made the two branches alike in that direction would bring B1's class back for rows of version 1 and for no row, and nothing would fail.
- Suggestion: add a no-row case (or a version 1 case with a whole cut) to `head_swapped_while_the_engine_is_opened_again_is_judged_as_the_engine_found_it`. `Swapping` already does what is needed.

**Certainty:** high that B1H survives the suite and that the variant kills it. The code is right as it stands.

#### L2 (low; documentation; does not block): `Observed::head_group` still equates "not `current`" with "left behind head"

- `remote.rs:81-85` says `head_group` is "the group of head's commit as read now", and that where the engine is left behind head ("not `current`") it is read from head's commit object, not taken from the engine.
- Since `8e6544d`, `current` is also false for an engine opened again whose second read of head differed from the first. `remote.rs:90-91` now says so.
- In that case `head_group` is the engine's (`remote.rs:233`): the group of the commit in `head` (`Z`), not of head as first read.
- The value is right; only the doc is off. The doc could say "the group of the commit in `head`", and "(the branch with writes)" in place of "(not `current`)".
- **Certainty:** high.

#### D1 (the description; does not block)

- The CI section lists the runs on 4595f55 and says the runs on 75bed3f are still going.
- The description says the author will add those runs when they end. They have ended (under (6)), so the section can now link them and drop "still going".
- Nothing else in the description is out of date (under (4)).

#### Nothing else found

`8e6544d` changes no product code besides the one line above and documentation:
- `remote.rs`: the docs of `current` and `head`, and a comment;
- `lib.rs`: rules 2 and 3.

### (1) B1's test

**It is S1.** `head_swapped_while_the_engine_is_opened_again_is_judged_as_the_engine_found_it` (`tests/driver.rs:1232`) has the same shape and checks the same outcome as the review's S1:
- cut 1 whole, a restart, then cut 2 as a delta onto cut 1;
- another writer's `Z`, with block 0 full of `0xCD`, of no group, of the row's group 7, and of group 99;
- the outcome S1's table gave "with the fix": exactly `[Mismatch HeadMoved, abandoned [2]]`, head the other writer's, the row as cut 1 left it, block 0 `0xCD`.

**The swap falls between the two reads.**
- `Swapping::get` (`tests/driver.rs:1117-1132`) runs the other writer after the inner read of `head` returns and before handing it back.
- The first read of `head` after `after_the_next_read` is `observe`'s:
  - `EngineRemote::new` reads nothing (`remote.rs:171-181`);
  - `observe` reads the row with `meta_get`, then head with `get` (`remote.rs:197-202`);
  - the reopen (`remote.rs:217`) then reads `Z`.
- The other writer goes through refs of its own, over the store's inner side. The pending swap is taken before it runs, so it cannot recurse.
- The B1 plant confirms the placement: if the swap fell before the first read, both reads would see `Z`, and the old line would pass.

**Plants and repeats** (`next_plants.py`; each plant alone; `cargo test --no-fail-fast -p skyzfs-upload`; then the named test again and again):

| Plant | What is planted | Suite | Repeat runs of B1's test |
|---|---|---|---|
| unplanted | (none) | (baseline: 273 passed, 0 failed, with `-p skynbd-engine`) | 10 of 10 pass |
| B1 | the old line, `head: head.commit` (`remote.rs:235`) | **KILLED**: 1, B1's test | 5 of 5 fail |
| B1R | control, a fix of another shape: the old line, plus the check observes again when `!current` | SURVIVED (expected: a correct fix) | 3 of 3 pass |
| B1H | `head_group` from the first read, `head` the engine's (L1) | **SURVIVED** | (not run) |

- B1R surviving shows that the test checks the outcome, not the line.
- The other plants (CT, CD, PSa, PSb, PSc, V1W) are under (2) and (3). All of them are in `next_plants.out`.
- The test is deterministic: there are no timers, and the swap is injected at a fixed call. No run differed from the others.

### (2) `current` and `head` now that `head` is the engine's

**Where they are read.**
- `driver.rs:549-561`, the check:
  - `judge(&cut, &row, seen.head, seen.head_group)`;
  - on `Committed`, `head_of(&named, &seen)`;
  - `expected: seen.row` (`:578`).
- `driver.rs:721-733`, the commit's retry:
  - where the row names the cut, `head_of`;
  - otherwise `GaveUp` on `conflict || seen.row != upload.expected || !seen.current`.
- `head_of` itself is at `driver.rs:809-811`.
- `current` is read only at `driver.rs:730`.
- The daemon's `KillRemote` (`bins/skyzfsd/src/kill.rs:217-219`) only passes `observe` through.

**Which branch of `observe` each caller can reach.**
- **At the check, the engine is always clean**, so `observe` takes branch A (at head, by hash) or C (opened again):
  - writes go in only after an `Upload` verdict;
  - `lost` discards them (`driver.rs:792`);
  - the retry resets (`:726`, `:731`);
  - an upload that lands commits them.
- **In the retry, the engine holds the cut's writes**, so `observe` takes A or B (left behind, with writes). `is_clean` is "the overlay is empty" (`skynbd-engine/src/lib.rs:1164`), and every cut writes blocks.
  - Branch C cannot be reached in the retry.
  - That is the same at the base.

**The cases.** H1 is `observe`'s read of head, H2 the read inside `Engine::open`, C1 the base cut's commit, CN the commit this cut's swap installs, and `Z` another writer's commit.

| Where | Case | Branch | `head` | `current` | Outcome | Right? | Pinned by |
|---|---|---|---|---|---|---|---|
| check | engine at head; nothing moves | A | H1, the engine's | true (unread) | judged on the head the swap expects | yes | the suite |
| check | engine missing, or clean and behind head; nothing moves | C | H2 = H1 | true (unread) | judged on the head the swap expects | yes | the suite |
| check | swap between H1 and H2 (B1) | C | `Z` (H2) | false (unread) | v2 row names C1 ≠ `Z`: a delta or a whole cut is `HeadMoved`; a cut found committed hands nothing on | yes | B1's test; plant B1 killed |
| check | the same race, no row or a v1 row, whole cut | C | `Z` | false (unread) | judged on `Z`'s group (`remote.rs:233`): `HeadMoved` if it has one | yes | **nothing (L1)**; the variant |
| check | swap after head was read (after H1 in branch A, after H2 in C), before the commit | A or C | the engine's (H1 or H2) | true | the swap's CAS expects the engine's head: `Conflict`, the retry gives up, and the next check (reopened) is `HeadMoved` | yes | `a_row_moved_before_the_commit_gives_the_commit_up` (the same path, through the row's guard); no test moves head there |
| check | head moved away and back onto C1 with a new generation (P1), engine kept at C1 | A (by hash) | C1 | true | judged as a delta onto C1; the CAS conflicts on the generation; the retry gives up; the next check reopens and lands | yes, one round trip more | not in the suite; the first review's S3 showed it (P1, unchanged) |
| retry | the swap landed, the reply was lost, nothing since | B | CN (H1) | false | the row names the cut and CN = head: uploaded, the publish carried | yes | found by the row: `a_failed_commit_is_made_again_or_found_landed_by_the_row`; the publish carried: S2, S5, the crash matrix (PSa, PSb, PSc killed) |
| retry | the swap landed, then head moved to `Z` | B | `Z` | false | uploaded, nothing handed on, `not_offered` | yes | n1's tests |
| retry | the swap landed, head moved away and back onto CN | B | CN | false | uploaded, the publish carried on CN | yes | S5 |
| retry | the swap did not land, nothing moved | A | C1 | true | not conflict, row as expected, current: commit again | yes | `a_failed_commit_is_made_again_or_found_landed_by_the_row` |
| retry | the swap did not land, head moved to `Z` | B | `Z` | false | `GaveUp`, and the next check is `HeadMoved` | yes | `a_failed_commit_on_a_head_or_row_another_writer_moved_is_given_up`; CD killed |
| retry | `Conflict` | A or B | any | any | `GaveUp` | yes | the suite |
| retry | engine clean (branch C) | — | — | — | not reachable (see above) | — | — |

**Conclusions.**
- The fix changes only the check's branch C, and there `current` is not read.
- In the retry, `head` is what it was at the base: branch A's engine head is H1, and branch B is unchanged.
- `current` is right everywhere it is read.
- Plant CT (`current: true` in the last `Observed`) survives the suite. That is an equivalent mutant, not a gap: in the retry that `Observed` comes only from branch A, where `current` is true anyway.

### (3) S2 and S5 as tests

- `the_upgrades_whole_cut_after_a_lost_reply_carries_its_publish` (`tests/publish.rs:1051`) is S2. It checks:
  - `HeadMoved` under the version 1 row;
  - after the lost reply, that the row's commit is head (`:1085-1089`);
  - `Uploaded { 3, Whole, BLOCKS }`;
  - `snap/t` on cut 3's commit, `Done`, and `not_offered` 0.
- `a_cut_found_committed_on_head_moved_and_moved_back_carries_its_publish` (`:1114`) is S5. Head is moved back with a CAS rather than `Engine::rollback`. To the uploader the two are the same: head holds cut 1's hash at a new generation.
- Unplanted, each passes 5 of 5.

**They fail when the behaviour breaks:**

| Plant | What breaks | Killed by |
|---|---|---|
| PSa | the retry hands on no commit for a cut the row names | S2, S5, `a_failure_at_any_operation_leaves_no_ref_or_a_whole_one` |
| PSb | the branch with writes reports the engine's commit as head (`remote.rs:212`) | the same three |
| PSc | the retry hands on head only where `current` | the same three |
| V1W | no whole cut under a version 1 row (the upgrade stops) | S2, `after_a_row_of_version_1_…`, `check::tests::a_whole_cut_is_uploaded_onto_the_rows_commit_whatever_the_row_names` |

S2 and S5 are two of the three tests that catch PSa, PSb and PSc. Before them, only the crash matrix did.

### (4) N1 and N5

**N1.** The crate's rules (`lib.rs:13-21`) now match `judge` (`check.rs:94-125`) rule by rule:
- Rule 1: the row names the cut, so it is `Committed`.
- Rule 2: a delta goes up only where the row names the base and `named.commit == Some(head)`. A version 1 row names no commit, so it vouches for no delta.
- Rule 3: a whole cut goes up where head is the commit the row names; under a version 1 row, by group; with no row, where head has no group.
- Rule 4: anything else is a mismatch.

No other text in the crate's sources still says head is matched by group, apart from version 1's case (`check.rs:85-89`, `:103-104`).

**N5.** The description now has:
- every commit of the pull request (`git log 5807457..75bed3f` gives the same five);
- B1, its test and its plant row;
- EN's 77, with both crates;
- the gates at 75bed3f;
- node B's bullets.

I checked the node B forms it names against `bins/skyzfs-nodeb/src/main.rs:14-71`: `row_commit`, `--expect-row` for either version, and `set-row` with `STATE:NUMBER:GROUP`, `…:COMMIT` and `v1:…`. "Nine plants of its own, every one caught" matches the first review's table (21 plants, 12 of them the author's). The only part out of date is D1.

### (5) The merge

**It is mechanical.**
- The merge base of `8e6544d` and `5807457` is `e155dbf`.
- `git diff 75bed3f^1 75bed3f` is `git diff e155dbf 5807457`: 24 files, +1089 −147.
- No file is in both that list and the pull request's own 15.

**What it brings near this work:**
- **Upload crate:** `crates/skyzfs-upload/src/lease.rs`, five lines of documentation on `LeaseState::Lost`.
- **Node B:** nothing in `bins/skyzfs-nodeb`. `tests/kernel/nodeb_check.py` changes one line: `pick_withheld` takes the new argument from #132's `cut_check.py`.
- **Rig:** `tests/kernel/cut_check.py` changes the withheld control (#132). It does not parse the row.
- **`skynbd-refs`:** the lease renewal's docs and tests (#133). Nothing in `get`, `compare_and_swap`, `transact` or `meta_*` changes.
- **The rest:** `skyzfs-local`'s in-place store (the intent header sync on by default, #133), `skyzfsd`'s flag for it, the bench, and the docs.

**Gates at 75bed3f** (rustc 1.100.0, 2026-11-09, the current stable; `next_gates.out`):

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| clippy, `-p skyzfsd -p skyzfs-local --features skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean |
| clippy, `-p skyzfs-ublk-sys --features control-test-program` | clean |
| `cargo test --workspace` | 1695 passed, 0 failed, 16 ignored |
| `cargo test --workspace --features skyzfsd/test-hooks,skyzfs-local/test-hooks` | 1737 passed, 0 failed, 16 ignored |
| `cargo test -p skyzfs-ublk-sys --features control-test-program` | 37 passed, 0 failed, 11 ignored; with the plain run's other binaries, 1719 passed and 18 ignored |

- These are the description's counts, all three.
- The test hooks add 39 tests, all in `skyzfsd`'s unit tests. Over the whole workspace they add 42: those 39, and 3 unit tests of `skyzfs-ublk-sys` that the features reach through feature unification.

### (6) CI and Codex on 75bed3f

Every run on 75bed3f ended green.

| Workflow | Run | Jobs |
|---|---|---|
| ci | [45498067655](https://github.com/skyzfs/skyzfs/actions/runs/45498067655) | changes, test, s3-backends, lint, chart-lint, unit-arm64: success |
| coverage | [45498067527](https://github.com/skyzfs/skyzfs/actions/runs/45498067527) | changes, coverage: success |
| e2e | [45498067715](https://github.com/skyzfs/skyzfs/actions/runs/45498067715) | changes, e2e-tier-a: success |
| k3s-smoke | [45498067590](https://github.com/skyzfs/skyzfs/actions/runs/45498067590) | UDS smoke, install smoke: success |
| skyzfs-ci | [45498058301](https://github.com/skyzfs/skyzfs/actions/runs/45498058301) | the push to `skyzfs-coding-f1`: gate success; the rest skipped, as the pull request's own runs cover them |
| skyzfs-nodeb-rig | [45498067509](https://github.com/skyzfs/skyzfs/actions/runs/45498067509) | node B's logic; record and check on node B, ubuntu-26.04 and ubuntu-26.04-arm: success |
| skyzfs-cut-daemon | [45498067633](https://github.com/skyzfs/skyzfs/actions/runs/45498067633) | the check's logic; record through skyzfsd and check on ubuntu-26.04, ubuntu-26.04-arm and ubuntu-26.04-upload: success |
| skyzfs-replay-rig | [45498067423](https://github.com/skyzfs/skyzfs/actions/runs/45498067423) | through skyzfsd and write into a hole, ubuntu-24.04 and ubuntu-26.04: success; the reference's replay jobs skipped (this pull request does not touch `tests/kernel/`, `tests/lib/` or the workflow) |
| skyzfs-daemon-rig | [45498067504](https://github.com/skyzfs/skyzfs/actions/runs/45498067504) | the table and its judge; ubuntu-24.04, ubuntu-26.04 and ubuntu-26.04-arm on the daemon: success |

**What the rigs' tables say**, not only their colour:
- **The daemon rig:** on each of the three images, the judge's step ended with rc 0, so every line is as its table `EXPECTED` says:
  - 53 pins PASS;
  - one pin NA, `pin_c_kill_with_applied_unacknowledged`: the in-place store keeps no log;
  - one pin WAITING, `pin_failed_volume_while_a_request_is_held`, as before this pull request;
  - five controls caught (FAIL), and one control NA.
- **The cut rig's upload job** (`next_upload_job_75bed3f.txt`): all six courses (life, umoved, urow, ukill, ulease, ulost) PASS `upload_content`, `upload_status`, `upload_balance` and `upload_reads`. The five courses other than life also PASS `upload_course`. Node B reads the daemon's version 2 rows.

**Codex**, read after every run had ended:
- Its summary comment records the code review of 75bed3f as completed with `has_any_findings: false`.
- It records the security review as completed, with no findings recorded.
- There are no review comments and no reviews on the pull request. GitHub reports it mergeable (`clean`).

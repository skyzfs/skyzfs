## Next look at #134 (F1): the final head 7989f5e (N3), after 75bed3f

**Verdict at 7989f5e, the final head: can merge. Nothing blocks.**
- `7989f5e` is `75bed3f` plus one commit, N3: `Mismatch::RowV1`, shown as `upload_checked row_v1`, code 6.
- **The guard in `judge` is right.**
  - `row_v1` is given only where a version 1 row names the cut's base and head's group is the row's. `head_moved` stays where the group moved, and `not_the_base` where the row names another cut. The case table is under (N3.1).
  - `row_v1` never lets a delta up, and the driver treats it exactly as `head_moved`.
  - The word can stand where head moved, but only for a move inside the row's group. A version 1 row cannot show such a move at all: at `75bed3f` the same case read `head_moved`, and so did the plain upgrade, where nothing had moved.
  - No wrong history gets through, and none is left uncaught (N3.2).
- **Code 6 with no new status version is justified.**
  - The field's contract since it was added reads any unknown value as "not said".
  - A status file written at `7989f5e` reads as `upload_checked unsaid` at `75bed3f`; I ran this across the two versions.
  - A new test pins the code number, the decoding and the status line, and two plants on the code are caught by it.
  - The readers in `tests/` and node B are not affected. Details under (N3.3).
- **Three new points, none blocking:**
  - N-a, a doc comment left on the wrong test;
  - N-b, a gloss in the status doc of code 6;
  - D2, the description still says the runs on `7989f5e` are to be added.
- **The earlier points:**
  - L1 and L2 (low, from the look at `75bed3f`) stand as they were: `7989f5e` does not touch `remote.rs`.
  - D1 is resolved: the description now links the runs on `75bed3f`.
- **Plants:** all nine caught (under (N3.4)): the four of the author's I ran again, and five of my own.
- **Gates at 7989f5e:** fmt and the three clippy runs are clean. The tests give 1696, 1738 and 1720 passed, 0 failed, the description's counts.
- **CI:** every run on 7989f5e is green, and the rigs' tables agree with their colour. Their merge base is `zfs-topology` at `1699116` (#135, bench files only); the pull request merges into it with no conflict.
- **Codex:** it completed its code review and its security review of 7989f5e and posted no findings.

**Findings open at 7989f5e, most severe first.** None blocks.

| Id | Severity | Where | What | Certainty | Since |
|---|---|---|---|---|---|
| L1 | low, a gap in the tests | `crates/skyzfs-upload/src/remote.rs:233` | no test pins that `head_group` is the engine's; plant B1H survives (details below, under the look at 75bed3f) | high | 75bed3f |
| L2 | low, documentation | `crates/skyzfs-upload/src/remote.rs:81-85` | `head_group`'s doc equates "not `current`" with "left behind head" | high | 75bed3f |
| N-b | nit, documentation | `crates/skyzfs-ublk/src/status.rs:156` | code 6 is glossed as the first cut after an upgrade only; it is also the code after a move inside the row's group (N3.2) | high | 7989f5e |
| N-a | nit, a doc comment | `crates/skyzfs-ublk/src/status/tests.rs:1111-1112` | two doc lines of the test at `:1150` now open the new test's doc (N3.3) | high | 7989f5e |
| D2 | the description | the CI section | the runs on 7989f5e are "to be added"; they have ended, green (N3.5) | high | 7989f5e |

**How I formed the view of 7989f5e.**
- I read `git diff 75bed3f 7989f5e`: 8 files, +75 −12.
- I checked the guard against the cases, the driver's handling of a mismatch, the status file's code, and every reader of `upload_checked`.
- I read the author's N3 answer (https://github.com/skyzfs/skyzfs/pull/134#issuecomment-7146595946) after that, and it agrees.
- The plants and the cross-version run ran in a clean worktree, each plant restored afterwards. The gates ran in a clean checkout of 7989f5e.
- Paths: `check.rs`, `driver.rs` and `lib.rs` are in `crates/skyzfs-upload/src/`, and `status.rs` and `status/tests.rs` are in `crates/skyzfs-ublk/src/`, unless a path says otherwise.

**Files in this folder from this look at 7989f5e:**
- `next_n3_plants.py`, `next_n3_plants.out`, `next_n3_plants.json`: the plants. RV1, RVG, RVD and RVC are the author's (their ids as in the description); QN, QX, QW, QA and QU are mine.
- `next_n3_plant_QA.txt`: how QA's run ended.
- `next_n3_baseline.out`: the unplanted runs the plants are compared with.
- `next_n3_xv_write.rs`, `next_n3_xv_read.rs`, `next_n3_xv.sh`, `next_n3_xv.out`: the cross-version run of code 6.
- `next_n3_gates.out`: the gates and the counts by test binary.
- `next_n3_upload_job_7989f5e.txt`: the cut rig's upload courses on 7989f5e.

### N3 at 7989f5e

**What the commit changes:**
- `check.rs`:
  - `Mismatch::RowV1` and its doc (`:41-50`);
  - its words, "the row names no commit" (`:62`);
  - the guard (`:133-137`);
  - `judge`'s doc (`:95-104`).
- `lib.rs:14-18`: rule 2 names `row_v1`.
- The daemon shows it as `CheckedShown::RowV1` (`bins/skyzfsd/src/upload.rs:978`, in an exhaustive match, `:972-980`), with a row in its test (`bins/skyzfsd/src/upload/tests.rs:1122`).
- `status.rs`:
  - `RowV1 = 6, "row_v1"` (`:513-515`);
  - the doc of offset 568 (`:156`);
  - `VERSION` stays 19 (`:229`).
- `status/tests.rs:1111-1147`: a new test of the codes.
- Two tests now expect `RowV1`: `after_a_row_of_version_1_the_first_commit_is_whole_and_writes_version_2` (`tests/driver.rs:406`, `:431`) and S2 (`tests/publish.rs:1068`).

#### (N3.1) The guard, case by case

For a cut of part of the volume, `judge` (`check.rs:105-141`) tries, in order:

| Step | Verdict | Where |
|---|---|---|
| 1 | `Committed` | `:111` |
| 2 | `NoRow` | `:131` |
| 3 | `NotTheBase` | `:132` |
| 4 | `RowV1` | `:135` |
| 5 | `HeadMoved` | `:138` |
| 6 | `Delta` | `:139` |

- Under a version 1 row, "head is the row's" means only that head's group is the row's (`:120`).
- The guard needs `named.commit.is_none()`, which no version 2 row has, so rows of version 2 and the absence of a row are judged as at `75bed3f`.
- `RowV1` is the second arm a version 1 row can reach. The other is `:138`, which sends every other version 1 row to `HeadMoved`, since `None != Some(head)` always holds. So no arm gives a version 1 row a delta.

**Cases under a version 1 row** (the row names cut 4 of `STATE`, group 40):

| The row names | Head | Before F1 | At 75bed3f | At 7989f5e | Pinned by |
|---|---|---|---|---|---|
| the base | the commit the base went up as (the upgrade; nothing moved) | delta up | `HeadMoved` | **`RowV1`** | `check.rs:302`; `tests/driver.rs:431`; S2 (`tests/publish.rs:1068`) |
| the base | another commit of group 40 (a rollback onto a snapshot of the same group, another writer, `import -T`) | delta up, onto a history the row does not name (n2) | `HeadMoved` | **`RowV1`** | `check.rs:303` |
| the base | a commit of group 41 | `HeadMoved` | `HeadMoved` | `HeadMoved` | `check.rs:304` |
| the base | a commit of no group | `HeadMoved` | `HeadMoved` | `HeadMoved` | `check.rs:305` |
| another cut (`OLD`) | head at group 40 | `NotTheBase` | `NotTheBase` | `NotTheBase` | `check.rs:306` |
| this cut | any | `Committed` | `Committed` | `Committed` | `check.rs:234` |
| any (a whole cut) | group 40 | whole up | whole up | whole up | `check.rs:340` |
| any (a whole cut) | group 41, or none | `HeadMoved` | `HeadMoved` | `HeadMoved` | `check.rs:367-368` |

- The two bold rows are the only change.
- Every row of the table is in the crate's tests. The plants under (N3.4) show that the tests fail when the guard is widened (RVG, QX), moved (QN), dropped (RV1), or extended to a whole cut (QW).

#### (N3.2) Can `row_v1` hide a real head move, or let a wrong history through?

**A wrong history: no.**
- `RowV1` is a `Mismatch`. The driver handles every mismatch alike (`driver.rs:566-571`): it abandons every held cut, counts the mismatch and the cuts, and goes back to the check.
- So `RowV1` and `HeadMoved` differ only in the reason kept in the figures, and so in the status word and code.
- No delta goes up under a version 1 row (above).
- What comes next is unchanged from `75bed3f`: the next cut is whole, it goes up where head's group is the row's, and its commit writes a row of version 2. That is the residual risk of version 1 as the lead decided (2026-11-19), and N3 does not widen it.
- Plant QA shows the tests depend on that sameness. With the held cuts kept at `RowV1`, the upgrade's tests never end (the check refuses the same delta again and again).

**A hidden move: only one a version 1 row cannot show.**
- The status says `row_v1` and not `head_moved` in one case where head did move: head moved to another commit of the row's own group (the second bold row).
- No status word can be right there. The row names no commit, so the check cannot tell that move from the upgrade where nothing moved.
- At `75bed3f` both cases read `head_moved`. At `7989f5e` both read `row_v1`. That says what is known, "the row names no commit", and `Mismatch::RowV1`'s doc says head "is not known to be the commit the base went up as" (`check.rs:43-45`).
- A move to another group, or to no group, still reads `head_moved` (RVG is caught).
- The case lasts only until the first whole cut after the upgrade. From then on the rows are of version 2, they name the commit, and `head_moved` is exact again.

**The narrower reading.** The description leaves it to the lead to confirm that `row_v1` is given only where the row names the base and head's group is the row's.
- I find that reading the sound one. `not_the_base` and `head_moved` keep their meaning wherever they are known to hold.
- `row_v1` for every version 1 row would show `row_v1` where head is known to have moved to another group, or where the row is known to name another cut.
- The action is the same either way, so this is a choice of words for the lead, not a correctness question.

**Nit N-b (documentation; does not block).**
- The status doc of code 6 (`status.rs:156`) states the condition exactly: a version 1 row that names the cut's base, with head's group.
- It then glosses the code as "the first cut of part of the volume after an upgrade". It is also the code after a move inside the row's group, while the row is still of version 1.
- `check.rs:41-50` is exact. The status doc could add "or head moved within the row's group, which a row of version 1 cannot show".
- **Certainty:** high; the impact is small.

#### (N3.3) Code 6 with no new status version

**The bytes.**
- Offset 568 is 8 bytes, as before.
- Codes 0 to 5 keep their numbers and words.
- The only change is a new value, 6.
- `VERSION` is 19, as at `75bed3f` (`status.rs:229`).

**Older readers see it safely.**
- The field's documented contract, at `75bed3f` as now, ends: "Any other value is read as 'not said'".
- The `coded!` macro implements that (`status.rs:409-447`): `of` maps any unknown code to `Unsaid` (`_ => Self::Unsaid`), and `said()` gives "unsaid".
- The test at `status/tests.rs:1076-1089`, present at both heads, writes 99 at offsets 424, 448, 568 and 576, and reads `Unsaid` at each.
- **The cross-version run** (`next_n3_xv.sh`, `next_n3_xv.out`):
  - `7989f5e`'s `encode` writes a status file whose last check was `RowV1`. Bytes 568..576 are 6.
  - `75bed3f`'s `decode` and `report` read it: it decodes, `upload_checked` is `unsaid`, and the rest of the uploader's lines are as written (`upload committing cut 42`).
  - Both halves pass.
- An older daemon's `status` therefore prints `upload_checked unsaid`. It does not refuse the file, and it does not print another reason.

**The readers.** The only readers of `upload_checked` outside the status module:
- `tests/kernel/cut_daemon_check.py`:
  - `:1788`: umoved expects `head_moved`. That course moves head under a version 2 row, so the case is unchanged.
  - `:1799` and `:1843` expect `matched`.
  - `:1803` and `:2038` only print it.
- `tests/kernel/test_cut_daemon_main.py`: its own fixtures.
- `tests/kernel/cut-record.sh:1084` and `:1099`: comments.
- `tests/bench/skz_time.py` reads only the `time_*` keys and `status_version`.
- `bins/skyzfs-nodeb` has no reader of the status.
- No course of the rigs puts a version 1 row (`git grep` finds no `v1:` row in `tests/kernel`), so no course reaches code 6 today.
- The cut rig's upload job on 7989f5e is green: all six courses PASS their upload lines, and umoved shows `head_moved` (`next_n3_upload_job_7989f5e.txt`).
- In the product code, the daemon's match from `Checked` to `CheckedShown` (`bins/skyzfsd/src/upload.rs:972-980`) is exhaustive, so a new reason cannot be left unmapped.

**The rule.** CONTRIBUTING.md:31-32: "changing serialized bytes requires a version bump and a cross-version decode test."
- No status an older daemon could write changes in any byte. The new value falls within what the field's contract already defines ("any other value").
- A new version would make older readers treat the whole file as a newer version, to say one word they would print as "unsaid" anyway.
- The precedent is `08cfe8d`: `zeroes_applied` gained its code 4 in the same way, and its doc says so.
- The doc of offset 568 records the exception: "(from F1, with no new version: a reader of the nineteenth version from before it reads 'not said')".
- **I find the exception justified.**
  - The cross-version decode test the rule asks for exists in two halves. The old reader's half is the unknown-code test (`:1076-1089`). The new writer's half is the new test, and my cross-version run joins them.
  - Suggestion for the lead: write this exception into design §5.2 or CONTRIBUTING.md, so that the next one rests on the rule and not on two precedents. That is a rule for a new value in a field whose contract reads unknown values as "not said".

**The test that pins the code.** `each_finding_of_the_check_keeps_its_code_and_its_word` (`status/tests.rs:1117-1147`) checks each code from 0 to 6:
- that bytes 568..576 are the code;
- that it decodes to the same value;
- that `said()` is the word;
- that `report`'s `upload_checked` line is the word.

Plants RVC (code 7) and QU (code 5, `unreadable`'s) are each caught by it alone. The daemon's mapping is pinned by `the_status_shows_the_uploader_as_it_is` (RVD is caught).

**Nit N-a (a doc comment; does not block).**
- `status/tests.rs:1111-1112` are the two doc lines of `the_uploaders_post_is_kept_by_the_stores_and_written_on_its_own`. At `75bed3f` they stood over that test.
- The new test was inserted between them and their test. They now open the new test's doc (`:1111-1116`), and the old test (`:1150`) has none.
- Move the two lines back above `:1149`.
- **Certainty:** high.

#### (N3.4) Plants at 7989f5e

Each plant was applied alone, then `cargo test --no-fail-fast` was run on the crate named, then the plant was restored (`next_n3_plants.py`).
- Unplanted: `-p skyzfs-upload` gives 175 passed. The libraries of `skyzfs-ublk` and `skyzfs-upload` give 214 passed (`next_n3_baseline.out`).
- `skyzfsd`'s library passes in the plain gate.

| Plant | Whose | What is planted | Run | Result |
|---|---|---|---|---|
| RV1 | author's | the upgrade's first delta is `HeadMoved` again | `-p skyzfs-upload` | **KILLED**: 3 (the check's table, `after_a_row_of_version_1_…`, S2) |
| RVG | author's | `RowV1` for every version 1 row naming the base, whatever head's group | `-p skyzfs-upload` | **KILLED**: 1, `a_cut_of_part_of_the_volume_against_another_row_is_a_mismatch` |
| QN | mine | the guard before `NotTheBase`: a version 1 row naming another cut reads `row_v1` | `-p skyzfs-upload` | **KILLED**: 1, the same test |
| QX | mine | `RowV1` also for a version 2 row where head is another commit of the row's group (would hide n2's move) | `-p skyzfs-upload` | **KILLED**: 3, the check's table, `head_at_another_commit_of_the_cuts_group_takes_no_publish_and_no_delta`, B1's test |
| QW | mine | a whole cut under a version 1 row, with head at another group, reads `RowV1` and not `HeadMoved` | `-p skyzfs-upload` | **KILLED**: 1, `a_whole_cut_onto_a_head_that_is_not_the_rows_is_a_mismatch` |
| QA | mine | the driver keeps the held cuts at `RowV1` | `-p skyzfs-upload` | **KILLED** (the tests never end) |
| RVD | author's | the daemon shows `RowV1` as `head_moved` | `-p skyzfsd --lib` | **KILLED**: 1, `upload::tests::the_status_shows_the_uploader_as_it_is` |
| RVC | author's | `row_v1` is written as code 7 | `-p skyzfs-ublk --lib` | **KILLED**: 1, `status::tests::each_finding_of_the_check_keeps_its_code_and_its_word` |
| QU | mine | `row_v1` is written as code 5, `unreadable`'s (a reader could not tell them apart) | `-p skyzfs-ublk --lib` | **KILLED**: 1, the same test |

- QA in detail: the driver's and the publish tests' binaries never ended. The kernel killed the first when it ran out of memory, and I stopped the second after about six minutes (`next_n3_plant_QA.txt`). Unplanted, each ends in under a second. The check refuses the same delta for ever, so the plant is caught, though as a hang and not as a named failure.
- The description's RVW (code 6 printed as another word) I did not run. The `said()` and `upload_checked` lines of the same test cover it.

#### (N3.5) Gates, CI and Codex at 7989f5e

**Gates at 7989f5e** (rustc 1.100.0, 2026-11-09, the current stable; `next_n3_gates.out`):

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| clippy, `-p skyzfsd -p skyzfs-local --features skyzfsd/test-hooks,skyzfs-local/test-hooks` | clean |
| clippy, `-p skyzfs-ublk-sys --features control-test-program` | clean |
| `cargo test --workspace` | 1696 passed, 0 failed, 16 ignored |
| `cargo test --workspace --features skyzfsd/test-hooks,skyzfs-local/test-hooks` | 1738 passed, 0 failed, 16 ignored |
| `cargo test -p skyzfs-ublk-sys --features control-test-program` | 37 passed, 0 failed, 11 ignored; with the plain run's other binaries, 1720 passed and 18 ignored |

These are the description's three counts. Each is one more than at `75bed3f`, the new status test.

**CI on 7989f5e.** Every run ended green. The pull request's runs built `7989f5e` merged into `zfs-topology` at `1699116`:
- `1699116` is #135, which changes four files under `tests/bench/` and nothing of this pull request's.
- `git merge-tree` merges the head into today's `zfs-topology` with no conflict.

| Workflow | Run | Jobs |
|---|---|---|
| ci | [45503748777](https://github.com/skyzfs/skyzfs/actions/runs/45503748777) | changes, test, s3-backends, lint, chart-lint, unit-arm64: success |
| coverage | [45503748892](https://github.com/skyzfs/skyzfs/actions/runs/45503748892) | changes, coverage: success (badges skipped) |
| e2e | [45503748907](https://github.com/skyzfs/skyzfs/actions/runs/45503748907) | changes, e2e-tier-a: success |
| k3s-smoke | [45503748898](https://github.com/skyzfs/skyzfs/actions/runs/45503748898) | UDS smoke, install smoke: success |
| skyzfs-ci | [45503738957](https://github.com/skyzfs/skyzfs/actions/runs/45503738957) | the push: gate success; the rest skipped, as the pull request's own runs cover them |
| skyzfs-control-rig | [45503748788](https://github.com/skyzfs/skyzfs/actions/runs/45503748788) | the table and its judge; ubuntu-24.04, ubuntu-26.04, ubuntu-26.04-arm: success (N3 touches `crates/skyzfs-ublk/`) |
| skyzfs-nodeb-rig | [45503748661](https://github.com/skyzfs/skyzfs/actions/runs/45503748661) | node B's logic; record and check on node B, ubuntu-26.04 and ubuntu-26.04-arm: success |
| skyzfs-cut-daemon | [45503748672](https://github.com/skyzfs/skyzfs/actions/runs/45503748672) | the check's logic; record through skyzfsd and check on ubuntu-26.04, ubuntu-26.04-arm and ubuntu-26.04-upload: success |
| skyzfs-replay-rig | [45503748825](https://github.com/skyzfs/skyzfs/actions/runs/45503748825) | through skyzfsd and write into a hole, ubuntu-24.04 and ubuntu-26.04: success; the reference's replay jobs skipped |
| skyzfs-daemon-rig | [45503749083](https://github.com/skyzfs/skyzfs/actions/runs/45503749083) | the table and its judge; ubuntu-24.04, ubuntu-26.04 and ubuntu-26.04-arm on the daemon: success |

**What the rigs' tables say, not only their colour.**
- **The daemon rig:** on each of the three images the judge's step ended with rc 0, with the same lines as at `75bed3f`:
  - 53 pins PASS;
  - one NA, `pin_c_kill_with_applied_unacknowledged` (the in-place store keeps no log);
  - one WAITING, `pin_failed_volume_while_a_request_is_held`;
  - five controls caught (FAIL), and one control NA, `ctl_delay_before_apply`.
- **The cut rig's upload job:** all six courses PASS `upload_content`, `upload_status`, `upload_balance` and `upload_reads`. The five other than life also PASS `upload_course`. The job has no FAIL line.

**Codex.** I read its summary comment after every run had ended. It records:
- the code review of 7989f5e: completed, with `has_any_findings: false`;
- the security review: completed, with no findings.

There are no review comments and no reviews on the pull request. GitHub reports it mergeable (`clean`).

**D2 (the description; does not block).**
- The CI section still says "The runs on `7989f5e` (N3) are to be added when they end."
- They have ended, all green, so the section can link them (the table above).
- The rest of the description matches the head: the commits, N3's bullets, the plant rows, the gates and the narrower reading left to the lead.

**For the testing session** (not for this pull request): the description lists a follow-up course on a version 1 row left from before the upgrade. Its first delta should expect `row_v1`:
- also after head moves to another commit of the row's group;
- and `head_moved` after a move to another group.

That would be the first rig course to reach code 6.

---

## The look at 75bed3f, as it was written (D1 since resolved)

**Verdict at 75bed3f: can merge. Nothing blocks.**
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

#### D1 (the description; does not block; resolved at 7989f5e: the description now links every run on 75bed3f)

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

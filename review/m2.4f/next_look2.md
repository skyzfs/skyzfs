## Second next look at #129 at ee90899 (M2.4f)

The head moved from `ef8b227` to `ee90899` by two commits:

- `528ce5c` merges `zfs-topology` at `35f260d`, where #127 had merged
  as `ebf7bd6`;
- `ee90899` takes T5 as tests (my S1 to S4, under names of the file's
  kind), N5, N6, and the status table's magic.

The author's answer is issue comment 6999449565. I checked it against
the code. The head did not move while I looked.

**Verdict: can merge. Nothing blocks.**

- T5 is settled: S1 to S4 are in the package as I wrote them, and
  they kill the wrong resets that survived at `ef8b227`.
- The product diff since `ef8b227` is texts only.
- The merge brings no product code, and leaves one merge base.
- Every workflow at the head has finished green, and its tables read
  as expected.
- Codex has no open finding. Its one P2 is #127's code, and the
  author has put it in F5.

One thing for the lead after the merge: #130 merged after these runs
began, so dispatch the cut rig on `zfs-topology` (§4).

### 1. S1 to S4 as lifted, and the plants at `ee90899`

**S1 to S4 stand as my text intended.**

- I extracted the four tests from `ee90899` and from my
  `scenarios_next.rs`, removed the comments, and renamed
  `REVIEW_M24F_MINUTE` to `MINUTE`. Then I compared them token by token.
- All four are identical: `an_error_…` (S1), `a_mismatch_…` (S2),
  `a_give_up_…` (S3) and `a_publish_between_two_whole_cuts_lost_does_not_end_the_waits`
  (S4).
- `MINUTE` is defined once, as 60 s. Each test is
  `#[tokio::test(start_paused = true)]`.
- The only other change to the test file is the section's heading
  comment, which now says T1 to T5. The docs of the four tests say
  what each does and why, and they are right.

**The plants, made again by me at `ee90899`.** I ran
`plants2.py --out=plants3.out` with nothing appended. It ran
plants2.py's 27 plants, and then, through `first_look()`, the first
look's other 12 from `plants.py`, as written there. Each plant was
made alone, the tests of its crate were run, and the files were
restored. The worktree was clean at the end.

**38 of the 39 plants are KILLED. X4 survives**, and it is equivalent:
between two whole cuts lost, no delta cut exists without a commit, and
a commit ends the waits anyway.

| Plants | Result at `ee90899` |
|---|---|
| V1, V2, V3, U4 (the steps that end the waits) | KILLED: V1 by the found test alone, V2 by the gone test alone, V3 by the doubling test, U4 by both T4 tests |
| X1, X5, C1 | KILLED by S2 (`a_mismatch_between_…`) |
| X2, X7, C2 | KILLED by S3 (`a_give_up_between_…`) |
| X3, C3 | KILLED by S1 (`an_error_between_…`) |
| X8 | KILLED by S4 (`a_publish_between_…`) |
| X6 | KILLED (both T4 tests, S3, and five more) |
| X4 | SURVIVED (equivalent, as above) |
| U1 to U10 (U6 with N1's text) | all KILLED. U2 by S4 and R2; U3 by R1 alone; U5 by R3 alone; U6 by R4 alone |
| the author's W1 to W8 | all KILLED |
| Z06, Z07, G12, G5, G8, G11, G13 (the slices, from the first look) | all KILLED in `skyzfs-local` |

The author's own run of the same 15 plants, at `7692558`, agrees with
mine. That commit differs from the head only in `cargo fmt` putting
one of S2's assertions on one line.

### 2. The product diff since `ef8b227` is texts only

`git diff ef8b227 ee90899` over `crates` and `bins`, with every test
file left out, changes no line that is not a `//!` or `///` comment.
The three hunks:

- **N5.** The doc of `WholeLost` in `uploader.rs`, and the status
  table's row for state 10, now read "under a load that fills the
  bound on copies before one is committed, say". That covers R4's
  case.
- **N6.** The version history's line in `status.rs` is reflowed to the
  width of the lines around it.
- **The magic.** The status table's row at offset 0 now reads "magic
  `SKYZST19`". That matches the code:
  - `const MAGIC: &[u8; 8] = b"SKYZST19"` and `const LEN: usize =
    1416` (`status.rs:226`, `:229`);
  - the encode writes the last field at `bytes[1408..1416]`, which is
    the table's last row, `| 1408 | 8 | upload_cuts_lost_whole |`;
  - the tests check it: `status/tests.rs:154-155` asserts that an
    encoded file is 1416 bytes and begins `SKYZST19`, `:431` lists
    `(b"SKYZST19", 1416)` among the versions decoded, and `:2262`
    lists `("19", b"SKYZST19")`.

At the head, `cargo test -p skyzfs-ublk --lib -- status` passes, 44
tests.

### 3. `528ce5c`: the merge of `zfs-topology`

- **It is clean, and it is the plain merge.** `git merge-tree
  --write-tree ef8b227 35f260d` gives tree `b8de132…`, the tree of
  `528ce5c` exactly. So no hand edit was made during the merge.
- **It brings no product code.** `git diff ef8b227 528ce5c` touches
  only `docs/skyzfs-impl-notes.md`, `tests/bench/*` (`bracket.py`,
  `minio.sh`, `test_bracket.py`, `test_copy_limit.sh`,
  `test_embedded_python.py`, `topology-bench.sh`) and
  `tests/lib/test_skz_store.sh`. Nothing under `crates/` or `bins/`
  changes.
- **The package's diff is the same across the merge.** `git diff
  3a3985b ef8b227` and `git diff 35f260d 528ce5c`, over `crates` and
  `bins`, are identical (compared without index and hunk-header
  lines).
- **One merge base is left.** `git merge-base --all 35f260d ee90899`
  gives `35f260d` alone. GitHub now reads the pull request as
  `mergeable: true`, and the `pull_request` workflows ran.
- **Nothing of #127's code changed under this PR.**
  - `git diff 35f260d ee90899` touches only the package's 9 files:
    `upload.rs`, `upload/tests.rs`, `state/tests.rs`, `status.rs`,
    `status/tests.rs`, `backoff.rs`, `driver.rs`, `uploader.rs` and
    `tests/uploader.rs`.
  - Over `bins/skyzfsd/src/control.rs` and `control/` that diff is
    empty.
  - #127 as merged equals #127 as reviewed: `git diff 3a3985b
    ebf7bd6 -- crates bins` is empty.

### 4. CI and Codex at `ee90899`

With one merge base left, the `pull_request` workflows ran at
`ee90899`. **All of them have finished, with success. None was still
running when I looked.** I read each table, not its colour.

| Workflow (run) | What I read |
|---|---|
| ci (44456630587) | changes, lint, test, unit-arm64, chart-lint and s3-backends: all success |
| coverage (44456630650) | success |
| e2e (44456630645) | success |
| k3s-smoke (44456630555) | both jobs success |
| skyzfs-ci, the push (44456621146) | success |
| control rig (44456630962) | 24.04, 26.04, 26.04 arm and the judge: success |
| cut rig through skyzfsd (44456630542) | 26.04, 26.04 arm and 26.04 with uploads: success, details below |
| replay rig (44456630968) | the jobs through skyzfsd and the hole jobs: success, details below |
| daemon rig (44456630695) | 24.04, 26.04 and 26.04 arm: success, details below |

**The cut rig through skyzfsd.** I rendered each job's records with
the head's `cut_summary.py`. No line reads FAIL, and every control is
as expected; some of them read "same or caught".

- 26.04: 157 PASS, 141 CAUGHT, 69 SAME, 107 INFO.
- 26.04 arm: 157 PASS, 143 CAUGHT, 67 SAME, 107 INFO.
- The upload courses (life, umoved, urow, ukill, ulease, ulost): 59
  PASS, 54 CAUGHT, 30 SAME, 30 INFO. `upload_status` and
  `upload_balance` PASS in each course.

The courses `bound` and `boundsparse`:

| Course | Image | Cuts, all PASS | `cuts_abandoned` |
|---|---|---|---|
| `bound` | 26.04 | 98 | PASS: one cut abandoned at the bound, its copies 0 of 268435456 bytes. With `--zero-fill` and no holes, at least one must be. `held_content`: 2 of the 98 cuts written and released, 262144 blocks compared, 0 discarded |
| `boundsparse` | 26.04 | 97 | PASS: none abandoned. The image's holes were dropped, 8192 chunks of 64 KiB, and the copies stayed under the bound, so none must be |
| `bound` | 26.04 arm | 97 | PASS: 1 abandoned |
| `boundsparse` | 26.04 arm | 98 | PASS: 0 abandoned |

**#130 merged at 04:47 as `dbb1d11`, after these runs began at
04:11.** It changes only `tests/kernel`:

- `cut-record.sh` records `status-bound-held.txt`;
- `cut_daemon_check.py` adds the judgement `cut_copied_held`, and a
  premise to `bound`.

So the `bound` lines above come from the judge as it was before #130.
That judge is the one the rig ran at this pull request's base.

- Merging `ee90899` into `dbb1d11` is clean, and their one merge base
  is `35f260d`.
- The pull request still reads `mergeable: true`, `clean`.
- After the merge, the lead should dispatch the cut rig on
  `zfs-topology` to see the new lines on this package's daemon. CLAUDE.md
  gives the same rule for the daemon rig.

**The replay rig.** It ran on the pull request's budget, so it does
not show design §9's criterion, and its summaries say so: "not shown
by this run".

| Job | Prefixes checked | Exit | Controls |
|---|---|---|---|
| through skyzfsd, 24.04 | 123 of 2223 | 0 | stuck and withheld as expected |
| through skyzfsd, 26.04 | 120 of 2072 | 0 | stuck and withheld as expected |

- The hole jobs on 24.04 and 26.04 read "Every checked prefix as
  design §4.1 allows".
- The reference layouts were skipped, as they are on a pull request
  that does not touch `tests/kernel/`, `tests/lib/` or the workflow.

**The daemon rig.** Each image's summary reads "Every pin and every
control is as the table `EXPECTED` in `tests/kernel/daemon_check.py`
has it".

- All three images give the same counts: 54 PASS, 5 FAIL (the
  controls, as expected), 2 NA, 1 WAITING, 13 WEAKER and 12
  NOT_EXERCISED.
- The WAITING is `pin_failed_volume_while_a_request_is_held`, as
  before. The WEAKER and NOT_EXERCISED rows are the conformance suite's.
- Each row's expected and got columns are the same as in the 26.04
  table at `b75a128` (run 44439241578), which I read for the first
  look. Only the measured values differ.

**A correction to my first look.** `review.md` says that 26.04 table
had 55 PASS. It has 54 PASS, the same as now. I miscounted: the verdict
is the same and nothing else in that review changes.

**Codex at `ee90899`.** Its summary reads code review "Completed" at
`ee90899`, with `has_unresolved_findings: false`, and there is no new
review comment at this head. The one thread, the P2 on
`control.rs:662`, is answered and resolved:

- The author's reply says it is valid, says it is #127's code (`ask`,
  merged as `ebf7bd6`) that showed here only because #129 carried
  #127, and names the fix for the next package (F5): a non-blocking
  connect, retried on `EAGAIN` until the limit, then made blocking
  with its timeouts, with a test of a full backlog.
- The thread is resolved, and now outdated, since `control.rs` is no
  longer in this pull request's diff.
- The author also noted it on #127 (issue comment 6999043414).

This is right: nothing in #129 changes for it. The lead should see
that F5 carries it.

### Gates I ran at `ee90899`

All runs used rustc 1.100.0, the current stable.

- `cargo fmt --all --check`: clean.
- `cargo test -p skyzfs-upload`: every binary passed, 57 in
  `tests/uploader.rs`, the four of T5 among them.
- `cargo test -p skyzfs-local --lib`: 309 passed, 1 ignored.
- `cargo test -p skyzfs-ublk --lib -- status`: 44 passed.
- `cargo test -p skyzfsd`: 137 passed in the daemon's tests, and 3
  more.

I did not run clippy locally. CI's lint jobs ran it.

### Files

- `plants2.py`: the plant script. It now takes `--out=NAME` and
  `--with=FILE`, makes C1 as one edit, and runs the first look's
  other plants through `first_look()`.
- `plants3.out`: its results at `ee90899`.

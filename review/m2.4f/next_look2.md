## Second next look at #129 at ee90899 (M2.4f)

The head moved from `ef8b227` to `ee90899` by two commits:

- `528ce5c` merges `zfs-topology` at `35f260d`, where #127 had merged
  as `ebf7bd6`;
- `ee90899` takes T5 as tests (my S1 to S4, under names of the file's
  kind), N5, N6, and the status table's magic.

The author's answer is issue comment 6999449565. I checked it against
the code. The head did not move while I looked.

**Verdict: VERDICT_PLACEHOLDER**

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

CI_PLACEHOLDER

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

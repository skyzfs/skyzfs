## Next look at #129 at ef8b227 (M2.4f)

The head moved from `b75a128` to `ef8b227` by three commits:

- `b4c261a` merges #127's head `3a3985b`, which carries #126's final
  head `5b526ac` (F7);
- `3d9de24` takes N1 to N4;
- `ef8b227` adds tests only: my R1 to R4, lifted, and two new tests
  for T4.

The author's answer is issue comment 6998625261. I checked it against
the code. The head did not move while I looked.

**Verdict: can merge, as to the code. Nothing in it blocks.** The
earlier "can merge" stands at `ef8b227`:

- every hunk of `b75a128..ef8b227` is accounted for;
- N1 to N4 are fixed as asked;
- the lifted R1 to R4 are my scenarios, and each still catches its
  plant;
- the T4 tests catch the two plants they were written for;
- every plant of the first look that I made again here (U1 to U10, and
  the author's W1, W4 and W5) is caught.

Two things still stand before the merge. Neither is a defect of the
package:

- **No pull-request CI ran at `ef8b227`.** GitHub reads #129, and #127,
  as conflicting with `zfs-topology` (`mergeable_state: dirty`), so
  none of the `pull_request` workflows started: not ci, coverage, e2e,
  k3s-smoke, nor the daemon, replay, control or cut-daemon rig. Only
  `skyzfs-ci`'s push run started; its gate saw the conflict and runs
  the gates in their place. The rigs last ran at `b75a128`, green.
  The cause is below, under "Mergeability", with the remedy.
- **#127 merges first**, as the description says. This package's
  range is `3a3985b..ef8b227`.

What I would still add, none of it blocking:

- **T5, a test gap.** No test says that a step other than a commit
  does not end the whole waits. Six plants of a wrong reset survive
  every test of the package: by a mismatch, a give-up, a refused
  commit's re-read, an error, an idle step, or a publish's step.
  Because of that gap, the T4 tests isolate their step only as long
  as nothing else resets. Three compound plants show it: in each, the
  step does not end the waits and a neighbour wrongly does, and all
  three pass the T4 tests. My scenarios S1 to S4
  (`scenarios_next.rs`) catch all six wrong resets and all three
  compounds. They are written to be lifted as they are.
- **Two optional nits, N5 and N6**, on N2's and N3's new texts.

### 1. The hunks of `b75a128..ef8b227`

| Commit | What | Accounted for |
|---|---|---|
| `b4c261a` | merge of `3a3985b` | It adds exactly #127's `d65ffb9..3a3985b`. `git diff b75a128 b4c261a` equals `git diff d65ffb9 3a3985b`, and `git diff 3a3985b b4c261a` equals `git diff d65ffb9 b75a128` (both compared without index and hunk-header lines): the package's own changes are the same on both sides of the merge. `d65ffb9..3a3985b` is `c9fe419`, which is #127's own (a closed desk's count reads that the store is not served: `control/handler.rs`, `control/tests.rs`, `tests/queued.rs`), plus the merge of `5b526ac`. `git diff c9fe419 3a3985b` equals `git diff e6d5570 5b526ac`, which is F7 alone (`uploader.rs` and its tests). Nothing else comes with it. |
| `3d9de24` | N1 to N4 | Five files, +18/−15, nothing else. See below. |
| `ef8b227` | tests | `crates/skyzfs-upload/tests/uploader.rs` only, +380/−4. The −4 are `until_reading`, which now calls `until_doing`, and the two helpers made generic (`over_the_bound`, `a_whole_cut_lost`). No product code. |

**N1 to N4, as fixed:**

- **N1.** `driver.rs:739` now reads `the store no longer held it at its
  commit`, so the line is "whole cut N went before its commit: the
  store no longer held it at its commit". Nothing repeats. The lifted
  R4 checks that line, and my U6, remade with N1's text, applies and
  is caught.
- **N2.** The state's line, its row in the status table, and the doc
  of `WholeLost` in `status.rs` and `uploader.rs` now say "no whole
  cut can be uploaded now". The doc names two causes: a load that
  fills the bound, and a failed sync.
- **N3.** The version history says that an earlier reader refuses the
  file by its magic, and that a reader of `status`'s text meets
  `whole_lost`. This is true: `decode` takes a file only when its
  length and magic are exactly one version's, and `SKYZST19` is no
  earlier version's.
- **N4.** The line reads "next whole cut after a wait of N s". The
  tests of the daemon and of the status check the new line.
- **No reader of the old texts.** No rig, bench script or judge parses
  either old text ("cannot be seeded", "next whole cut in"). I
  searched `tests/`, `.github/` and `docs/` at the head and on
  `origin/skyzfs-testing`.

### 2. The T4 tests, and the plants

`plants2.py` (results in `plants2.out`) makes each plant alone in a
worktree of `ef8b227`, runs the tests of its crate, and restores the
files. The worktree was clean after every run. Nothing is appended for
the first run, since R1 to R4 are now the author's own tests. With
`--with=scenarios_next.rs` it appends my S1 to S4.

**The steps that end the waits, one at a time:**

| Plant | The change | Result at `ef8b227` |
|---|---|---|
| V1 | `FoundCommitted` does not end the whole waits (refusals still reset): the author's W14 | KILLED by `a_cut_found_committed_ends_the_whole_waits` alone |
| V2 | `UploadedNotHeld` does not end them: the author's W15 | KILLED by `a_cut_committed_and_gone_ends_the_whole_waits` alone |
| V3 | `Uploaded` does not end them (the two others still do) | KILLED by `a_whole_cut_lost_waits_a_minute_doubling_to_fifteen_until_a_cut_is_committed` |
| U4 | only `Uploaded` ends them | KILLED by both T4 tests (it survived everything at `b75a128`) |

**The T4 tests, read against the code:**

- **Each test makes the step it names.** It checks
  `(found_committed, not_held, lost_whole)` right after the step:
  (1, 0, 2) in the found case, (0, 1, 2) in the gone case. Then it
  checks that `uploaded` has not moved by the third loss. So no
  `Uploaded` comes between, and the third loss waits 60 s, with
  `waited == wait`.
- **The found case.** `LAND_UNSEEN` lands the commit and answers a
  conflict. The re-read returns the stale row, so the commit is given
  up, and the next check finds the cut committed. After that, another
  writer's row makes the next delta cut a mismatch, and the cut after
  it is whole.
- **The gone case.** The head swap lands, then fails (`FaultWhen::After`
  on `refs`, with the lease kept apart). The bound takes the cut, and
  the retry finds it committed and gone.
- **Both paths are real at this head.** The `figures` assertions would
  fail if the step were another.

**The tests isolate the step from `Uploaded`, but not from the other
steps between.** Several steps come between the second loss and the
third:

- in the found case: a refused commit's re-read, a give-up, the step
  itself, idle steps, a mismatch, and the next cut's own steps;
- in the gone case: an error, the step itself, and the next cut's
  steps.

The cut's own Ok steps are covered: U1, and X6 below, are caught. The
others are covered by no test:

| Plant | Which step wrongly ends the whole waits | Result, the author's tests | With S1 to S4 |
|---|---|---|---|
| X1 | a `Mismatch` | SURVIVED | KILLED by S2 |
| X2 | a `GaveUp` | SURVIVED | KILLED by S3 |
| X3 | a step that fails with an error (the `Err(e)` arm) | SURVIVED | KILLED by S1 |
| X5 | an `Idle` step | SURVIVED | KILLED by S2 |
| X7 | the re-read after a refused commit (the `Conflict` arm) | SURVIVED | KILLED by S3 |
| X8 | a publish's step that succeeds (`after_publish`) | SURVIVED | KILLED by S4 |
| X4 | a delta cut lost | SURVIVED | SURVIVED. Equivalent: between two whole cuts lost no delta cut exists without a commit, which ends the waits anyway |
| X6 | a cut begun (a part of U1) | KILLED (both T4 tests, the doubling test, R1) | (not rerun) |
| C1 | V1 with X1: the mismatch after the found cut stands in for it | SURVIVED | KILLED by S2 |
| C2 | V1 with X2: the give-up before the found cut stands in for it | SURVIVED | KILLED by S3 |
| C3 | V2 with X3: the failed head swap before the gone cut stands in for it | SURVIVED | KILLED by S1 |

The code at the head is right: only the three committed steps reach
`whole_lost.succeeded()` (`uploader.rs:545-553`). T5 matters because
the package exists to bound the waste.

- **X3 is the one to fear most.** An object store's transient errors
  are common under load. With X3, each error between two lost whole
  cuts would bring the next wait back to a minute. The waits would
  never double, and the 22.7 GiB of the benchmark's `zf` row would
  return in part.
- **X8 is next.** An agent that publishes on a schedule would hold the
  waits at a minute.

S1 to S4 each put the step between two whole cuts lost and check that
the second waits 120 s, with no cut committed between. All four pass
at the head:

- S1: an error on object storage, with the lease kept apart;
- S2: a mismatch at the whole heir's check (the row names a group
  that is not head's), then idle until the next cut;
- S3: a refused commit, re-read and given up (`Refusals`, `REFUSE`);
- S4: a publish made during the wait, as in R2, carried on to the next
  loss.

**My U1 to U10, at `ef8b227`:** all ten are KILLED. U6 uses N1's text.

| Plant | Caught by |
|---|---|
| U1 | both T4 tests, the doubling test, R1 |
| U2 | `a_publish_is_made_during_a_whole_wait` (R2) alone |
| U3 | `the_leases_row_never_lapses_in_the_whole_waits` (R1) alone |
| U4 | both T4 tests |
| U5 | `a_whole_cut_lost_at_its_sync_waits` (R3) alone |
| U6 | `a_whole_cut_gone_at_its_commit_waits` (R4) alone |
| U7 | both T4 tests, R1, R2, the doubling test |
| U8 | the doubling test, the backoff's unit test, R1 |
| U9 | the two status tests of the eighteenth version |
| U10 | `the_status_shows_the_uploader_as_it_is` |

**The author's W1, W4 and W5, made again:** all KILLED. W5 is now also
caught by the T4 tests, through `a_whole_cut_lost`'s bound on each
sleep.

**R1 to R4 as lifted are the same scenarios.** I extracted the four
tests and the two helpers from `ef8b227` and from my
`scenarios_upload.rs`, removed the comments, mapped the names
(`review_m24f_advance` to `both_pass`, `review_m24f_until` to
`until_state`, `review_m24f_until_doing` to `until_doing`), and
compared them token by token.

- The helpers are identical.
- R1 and R2 differ only in a panic's message (`unreachable!()` became
  `panic!("{:?}", ...)`, and `turns < 1000` gained one).
- R3 and R4 differ in names (`began` became `wait`, `wait` became
  `sleep`). R3 adds a check of the line. R4's check of the line now
  follows N1.
- The generic `until_doing` matches as mine did. It only keeps the
  states for its panic.
- The seeds (201 to 204), the slices (8, 8, 8, 64), the writes, the
  groups and every other assertion are the same.

### 3. R2 with F7 merged

I ran an instrumented copy of `a_publish_is_made_during_a_whole_wait`.
It is the same scenario and seed, printing at each turn. I ran it at
`ef8b227`, and with the same text at `b75a128`, which is before F7.
The figures are the same at both heads:

| | `b75a128` (no F7) | `ef8b227` (F7) |
|---|---|---|
| the wait that began | 60 s | 60 s |
| the losing turn's sleep | 10 s (the lease's renewal, a third of 30 s) | 10 s |
| the request accepted, and the uploader woken | yes, at once | yes, at once |
| turns to make the publish | 2 (the first leaves it pending, the second makes it), both 0 s after the loss | the same |
| the state when it was made | `WholeLost`, lease held, row expiring at 1,030,000 ms on the refs clock | the same |
| the wait's end | 60 s after the loss, 7 more turns (the renewals every 10 s), then `Working(Reading, Some(4))` | the same |

F7 changes nothing here, as expected. During the wait the cut is not
due, so `Turns::next(true, false)` gives the publishes their turns
uncounted. F7 only keeps the count from before the wait instead of
clearing it. Nothing was owed when this wait began, so the two heads
agree.

### 4. CI, Codex and mergeability at `ef8b227`

**CI.** Only `skyzfs-ci`'s push run 44450398884 started. Its jobs at
the time of writing:

- gate: success. It found #129 conflicting and so ran the gates.
- lint: success. It covers shellcheck, `cargo fmt --check`, and clippy
  plain, with the control test program, and with the test hooks.
- test: success (the workspace's tests, plain and with each feature).
- unit-arm64: still running at my last look.

No `pull_request` workflow has a run at `ef8b227`. At `b75a128` all
eight ran green, and at #127's `3a3985b`, before #126 merged, they ran
green as well.

**Mergeability.** GitHub said `mergeable: false`,
`mergeable_state: dirty` for #129, and the same for #127, from #126's
merge until after the push of `ef8b227`. At my last look both read
`unknown`: GitHub was computing it again, and no `pull_request` run
had started. Its test
merge refs are stale: `refs/pull/129/merge` is still `d646117` with
`b75a128`. Locally:

- **A plain merge is clean.** `git merge-tree --write-tree` of
  `ef8b227` into `zfs-topology` at `cc141b1` exits 0, as does
  `3a3985b` into it.
- **The clean merge changes no product code.** Against `ef8b227`, the
  merged tree differs only in `docs/skyzfs-impl-notes.md`,
  `tests/bench/*` and `tests/lib/test_skz_store.sh`, all from
  `zfs-topology`.
- **The history is a criss-cross.** #126 merged into `zfs-topology`
  as `cc141b1`, while #127, and #129 through it, carry `5b526ac` by a
  merge of their own. So `git merge-base --all cc141b1 ef8b227` gives
  two bases, `5b526ac` and `0d42566`.
- **Each base alone conflicts.** A merge with `5b526ac` alone
  conflicts in twelve files. One with `0d42566` alone conflicts in
  `uploader.rs` and its tests. Only the recursive merge of the two
  bases (git's default) is clean.

That fits GitHub's "dirty", and its not starting the `pull_request`
workflows. **The remedy is for the author and the lead to decide:**
merge `origin/zfs-topology` into #127's branch and then into this one,
or into this one after #127 merges. The merge is clean, and it is no
force-push. It makes `cc141b1` an ancestor and leaves a single merge
base. The `pull_request` workflows should then run, and their rigs'
tables should be read before the merge, as the guide asks.

**Codex at `ef8b227`.** One finding, P2, on
`bins/skyzfsd/src/control.rs:662`: `ask` calls the blocking
`UnixStream::connect` before it sets its timeouts. So a client can
wait forever on a daemon that has stopped accepting connections while
its backlog is full.

- **The code is #127's** (`898a3b5c`, M2.5-2), unchanged since. Codex
  meets it here only because #129's diff against `zfs-topology` still
  carries #127. It is not this package's.
- **It is valid as stated.** On Linux, a blocking connect to a
  listener with a full backlog waits for room with the socket's send
  timeout, which is unset before the connect. So it can wait without
  end on a stopped or wedged daemon.
- **In practice it is narrow.** While the channel's thread lives, each
  connection is bounded (`REQUEST_LIMIT`, `ANSWER_LIMIT`), and the
  backlog drains.
- **It is for #127's author and review to answer.** It should be
  answered on #129's thread too, with a pointer to #127.

Codex raised nothing on the package's own code. The first look's
Codex pass at `b75a128` had no findings.

### 5. Optional nits

- **N5.** The new text of the status table and of the `WholeLost` doc
  reads "under a load that fills the bound on copies before one is
  read, say". It offers this only as an example. R4's case, the bound
  taking the cut after its read and before its commit, is "before one
  is committed".
- **N6.** N3's rewrite left `status.rs:26` at 95 columns between lines
  of about 70. It is a comment, so rustfmt does not reflow it.
- **Not this package's.** The table in `status.rs` still says "magic
  `SKYZST15`" at offset 0, and has since the sixteenth version (it is
  so on `zfs-topology` too). The lead may want it fixed when the
  status next changes.

### Gates I ran at `ef8b227`

All runs used rustc 1.100.0, the current stable.

- `cargo fmt --all --check`: clean.
- `cargo test -p skyzfs-upload`: every test binary passed, 53 in
  `tests/uploader.rs`, the six new among them.
- `cargo test -p skyzfsd`: 137 passed in the daemon's tests, and 3
  more.
- `cargo test -p skyzfs-ublk --lib -- status`: 44 passed.
- My S1 to S4, appended: 4 passed.

I did not run clippy locally. CI's lint job ran it in all three
configurations, green.

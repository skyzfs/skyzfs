## Second next look at #127 at 3a3985b (M2.5-2)

The answer to my next look at d65ffb9 is the issue comment of 19 November
(6997941415). Two commits are new:
- c9fe419 is M2.5-2's own. It takes N3, N4 and the nit on a closed
  desk's `cuts`.
- 3a3985b merges #126's final head 5b526ac, which is F7 of #126.

**Verdict: "can merge" stands at 3a3985b.** Nothing blocks.
- **The diff is accounted for, hunk by hunk.** The merge adds #126's
  e6d5570..5b526ac and nothing else.
- **N3, N4 and the nit are fixed.** Each has a test that fails when the
  fix is planted away.
- **The reasons for leaving N5 and N6 hold.** I add a cheap test that
  would catch N5; see (2).
- **My scenarios pass on the new head,** with the figures of d65ffb9.
  - The F1 scenarios: the lease's renewals stay on time with bursts of
    requests, and with `cuts` asked at a slow desk.
  - The socket's: the trickling client, the silent client, and `close()`
    with a full backlog.
- **Plants:** 49 at the head. 47 are caught; N5 and N6 survive, as the
  answer says they would.
- **CI** is green on every workflow at 3a3985b.
- **Codex** finished its code review and its security review at
  3a3985b with no findings.

Three small points, none blocking:
- a test wait with no limit, which hangs when a wake is planted away;
- a test of `gave_up` that would catch N5;
- the protocol's documentation of `ended` and `no-answer`, which names
  only the uploader.

### What I did

- **The diff:** read d65ffb9..3a3985b. I compared the merge with git's
  own merge of its parents, and its patch with #126's.
- **Gates** at the head, with rustc 1.100.0, on `skyzfsd`,
  `skyzfs-upload`, `skyzfs-ublk` and `skyzfs-local`:
  - `cargo fmt --all --check`: clean.
  - `cargo clippy --all-targets -- -D warnings`: clean, plain and with
    `skyzfsd/test-hooks,skyzfs-local/test-hooks`.
  - Tests with test hooks, 0 failed:
    - `skyzfsd`: 176, and 3 in `closed_output`;
    - `skyzfs-upload`: 153 (one more than before: #126's F7 test);
    - `skyzfs-ublk`: 160;
    - `skyzfs-local`: 323, 1 ignored.
  - Tests plain: `skyzfsd` 137 and 3, 0 failed.
- **The scenarios of the next look,** unchanged
  (`scenarios_control_2.rs`, `scenarios_up_2.rs`,
  `scenarios_queued_2.rs`). I appended them to the test files at the
  head for one run, then removed them. The output is in
  `scenarios_3.out`.
- **Plants** (`plants_3.py`, results in `plants_3.out`). These are the 46
  of the next look, run again because the merge changes `uploader.rs`.
  - A4 is rewritten for the new refusal text.
  - Three are new: N9, N10 and N11.
  - Each plant ran alone against the pull request's own tests, and its
    file was restored afterwards.

The worktree of the head was clean after every run.

### (1) The diff d65ffb9..3a3985b

**c9fe419** touches three files.
- **`bins/skyzfsd/src/control/handler.rs`.** `count` refuses a closed
  desk's `None` as `ended` "the store is no longer served". Before, it
  said "the uploader has ended". The other `ended` refusals keep "the
  uploader has ended": a closed inbox, and a request dropped unanswered.
  That is what they mean.
- **`bins/skyzfsd/src/control/tests.rs`.**
  `a_closed_desk_reads_ended_for_cuts` now also checks that the text
  names the store.
- **`crates/skyzfs-upload/tests/queued.rs`.** Two tests are extended:
  - `a_closed_desk_answers_as_a_store_that_holds_nothing` leaves an ask
    of the count waiting at the close. The ask must end within 5 s with
    `None`. This is N3.
  - `the_count_of_cuts_is_read_through_the_desk_on_the_stores_thread`
    checks that `Desk::answered` is still 0 after a count is given. This
    is N4.

  Both test comments are updated.

No product code changes but the one refusal text.

**3a3985b** is exactly the automatic merge.
- `git merge-tree --write-tree c9fe419 5b526ac` gives tree 918b200, and
  3a3985b's tree is 918b200.
- Its diff from c9fe419 is #126's `git diff e6d5570 5b526ac`, hunk for
  hunk. Only the line numbers in the hunk headers differ, and the blob
  ids.
- That diff touches only two things:
  - **`Turns` in `uploader.rs`:** the module docs, `Turns::next` (which
    no longer calls `restart()` while only one side is due),
    `restart`'s doc, and the unit test
    `a_wait_keeps_the_count_and_an_idle_cuts_step_starts_it_again`;
  - **one test in `tests/uploader.rs`.**

  Nothing of M2.5-2's own is in it. `control`, `post`, `run_with` and
  the channel's tests are as they were at d65ffb9.

### (2) N3, N4, the nit, and N5 and N6

**Planted, each alone, at 3a3985b:**

| Plant | Result |
|---|---|
| N3: the close leaves the asks of the count waiting (`slot.counts.clear()` removed) | caught by `a_closed_desk_answers_as_a_store_that_holds_nothing` (survived at d65ffb9) |
| N11 (new): the close answers each waiting ask with a count of nothing | caught by the same test |
| N4: a count given is counted as a call | caught by `the_count_of_cuts_is_read_through_the_desk_on_the_stores_thread` (survived at d65ffb9) |
| N9 (new): the nit undone, a closed desk's `cuts` reads "the uploader has ended" | caught by `a_closed_desk_reads_ended_for_cuts` |
| A4 (rewritten): a closed desk's `None` answered as a count of nothing | caught by `a_closed_desk_reads_ended_for_cuts` |

**A wait with no limit (minor, not blocking).** N2 is the plant in which
an ask of the count does not wake the queue. At d65ffb9 only the
daemon's handler tests caught it. At 3a3985b the queue's tests catch it
too, but by hanging.
- The new line in `a_closed_desk_answers_as_a_store_that_holds_nothing`
  (`tests/queued.rs:294`) waits for the ask's wake with
  `woken.recv().await` and no limit.
- With N2 planted, that test runs until it is killed. I ran each test of
  `tests/queued.rs` alone with a limit of 30 s, and it was the only one
  that did not end. The plant harness counted it as caught when it
  killed the suite at 900 s.
- A `tokio::time::timeout` around that wait would turn the hang into a
  failure. 4342c7e and 81c7824 did the same for two other tests. The
  wait for the call's wake just above it (`:287`) is the same pattern,
  from before.

**N5 (every `accept` error retried at once).** The reason holds for the
pause itself. Making `accept` fail on the socket's own error in a test
would need a process-wide limit, such as `RLIMIT_NOFILE` for EMFILE, in
a process that runs other tests. But the choice is a pure function,
`gave_up` (`server.rs:322`). A test of a few lines would catch N5: true
for ECONNABORTED, EINTR, EAGAIN and EPROTO, and false for EMFILE and
ENFILE. I suggest it, but it does not block.

**N6 (any request begins the next turn at once).** The reason holds, and
since F7 it holds more firmly.
- `next_wait` sleeps until the earliest of three times: the cut's
  `step_at`, the lease's `lease_at`, and the publishes' `publish_due_at`.
  So while `run_with` sleeps, nothing is due.
- A request does not make anything due, except an accepted publish,
  which already begins a turn.
- So an extra turn calls `Turns::next(false, false)` and takes no step.
- Before F7, `next` also called `restart()` there, so an extra turn
  would have wiped the count of the shared time. With 5b526ac it no
  longer does. An extra turn now only expires the publishes whose time
  is up and posts the status again, as the next turn would anyway.

### (3) The scenarios on the new head

All 17 pass at 3a3985b (`scenarios_3.out`).

**F1's scenarios:**
- **U1:** bursts of 16 requests at 0.25 s, 1 ms and 5 ms before a
  renewal, while each check of a cut takes 21 s. All 48 are answered,
  and the lease is taken at exactly 0, 10, 20 … 180 s.
- **U3:** `cuts` is asked at a desk whose queue takes 21 s for each
  turn, 0.25 s before the first renewal, and a publish 0.25 s before the
  second. `cuts` is answered after 11.25 s and the publish is accepted.
  The lease is taken at exactly 0, 10 … 80 s.
- **A1, planted again:** a store call put back between two turns.
  `the_channels_requests_do_not_hold_back_the_leases_renewal` still
  catches it, with two other tests, against #126's new `Turns`.
- **N10, new:** F7 undone. #126's unit test catches it, so the merge
  brought F7's test with it.

**The socket's scenarios,** with the daemon's real limits:

| Scenario | d65ffb9 | 3a3985b |
|---|---|---|
| R3: the client behind a silent client is answered after | 10.10 s | 10.01 s |
| R4: the client behind a trickler is answered after; the trickler is cut off at | 9.70 s; 10.80 s | 9.70 s; 10.80 s |
| R5: `close()` with a full backlog (4097 connections) returns `false` after | 5.000 s | 5.000 s |
| R6: 1 MiB manifest in pieces over 4.8 s | accepted | accepted |
| R7: the client behind a late manifest is answered after (the late one is not taken) | 9.99 s | 9.90 s |
| R8, R9: a 12 s handler, and a client queued behind it | both answered | both answered |
| R1: 20 concurrent clients | 1.00 s | 1.00 s |
| R2: an over-long greeting or line; a name of 128 bytes and of 129 bytes; a tab in a name | as before | as before |
| R10: a wake of nothing | returns | returns |

**The desk's scenarios:**
- **D1:** ten asks are answered alike and are not counted as calls.
- **D2:** an ask ends at the close. This is now also the author's test.
- **D3:** a cut counted after the ask is in the count.
- **D4:** after 1000 asks that gave up, the next is answered in 48 µs.

**U2:** the cut state is named before the first cut.

### Plants

All 49 at 3a3985b, against the pull request's own tests (`d`:
`skyzfsd`'s library with test hooks; `u`: `skyzfs-upload`'s library
and tests; `b`: `skyzfs-ublk`'s library). The tests that catch each
are in `plants_3.out`.

| Plants | Result |
|---|---|
| C1 to C5, K1 to K4, R1, R2, U1, U2 (`d`); S1 to S3 (`b`) | all caught, as at d65ffb9 |
| A1 to A11, with A2/A2d and A3/A3d one plant each and A4 rewritten | all caught |
| M2, M3, M4, M6 to M12, M14 (first look) | all caught |
| N1, N7, N8 | caught |
| N2 | caught: by a hang of the queue's tests, see (2) |
| N3, N4 | caught, now by the pull request's tests |
| N5, N6 | survive, as the answer says; see (2) |
| N9, N10, N11 (new) | caught |

### Smaller points (none blocking)

- **The wait with no limit at `tests/queued.rs:294`,** see (2).
- **A test of `gave_up`'s classification would catch N5,** see (2).
- **The protocol's documentation names only the uploader.**
  - In `control.rs`, the module docs (lines 96 to 104) still say that
    `ended` means "its uploader has ended" and that `no-answer` means
    "the uploader did not answer … since it answers between two of its
    steps". The doc comments of `Code::Ended` and `Code::NoAnswer` say
    the same.
  - For `cuts`, both now come from the store's queue: "the store is no
    longer served", and "the store did not answer … its queue takes no
    turn".
  - The texts are right; a sentence in the docs would make them agree.
    The same was true at d65ffb9, and I missed it then.

### CI and Codex

Runs on 3a3985b, all green:
- `ci` (44445444540): `test`, `lint`, `unit-arm64`, `s3-backends` and
  `chart-lint`;
- `coverage`, `e2e`, `k3s-smoke`, and `skyzfs-ci` on the push.
- `skyzfs-control-rig`.
- `skyzfs-replay-rig` (44445444066): all eight jobs, among them both
  layouts, the jobs through skyzfsd on ubuntu-24.04 and ubuntu-26.04,
  and the writes into a hole.
- `skyzfs-daemon-rig` (44445444144):
  - **On the daemon** (ubuntu-24.04, ubuntu-26.04, ubuntu-26.04-arm):
    each table has 53 PASS, 2 NA and 1 WAITING. The NA lines are the
    in-place store's (`ctl_delay_before_apply`,
    `pin_c_kill_with_applied_unacknowledged`). The WAITING line is
    `pin_failed_volume_while_a_request_is_held`. The 5 FAIL lines are
    the negative controls.
  - Those verdict lines are identical to d65ffb9's run. My next look
    said "54 PASS" there and left out the two NA lines; I have
    corrected that line in `next_look.md`.
  - The stand-in jobs ran too and are green.
- `skyzfs-cut-daemon` (44445444193): green on ubuntu-26.04,
  ubuntu-26.04-arm and ubuntu-26.04-upload.
  - On 26.04 and 26.04-arm, `bound: cuts_abandoned` passes, with one cut
    abandoned on a `--zero-fill` volume.
  - `boundsparse` passes too: none abandoned, with the holes set dropped
    after 8192 chunks.
  - Each job ends "worst exit 0".

No rig drives the channel yet.

**Codex** finished its code review (02:37) and its security review
(02:40) at 3a3985b with no findings. There are no new review threads.

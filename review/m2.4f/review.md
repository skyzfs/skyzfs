## Review of #129 at b75a128 (M2.4f)

The uploader waits after a whole cut is lost (`4ad45ab`), and two
test-only additions from the review of #125, G3 and G4 (`b75a128`).
The package is `d65ffb9..b75a128`, stacked on #127 at `d65ffb9`. The
head did not move while I reviewed it.

**Verdict: can merge. Nothing blocks.** I found no defect in the product
code. Every way a whole cut can go before its commit inside an instance
takes the new wait. The wait cannot loop or grow without bound, and
renewing the lease and making publishes go on during it. Status v19 is
laid out, versioned and read as documented, and the rigs' readers
accept it. `b75a128` changes only tests, and its two tests catch the
plants they were written for (Z06 and Z07), quickly and with no
randomness.

What I would still add, none of it blocking:

- **Four test gaps (T1 to T4).** In each one a plant of mine survives
  the author's tests:
  - the publishes during the wait (T1);
  - the lease, judged by its row rather than by the sleeps (T2);
  - a whole cut lost at its sync or at its commit, rather than at a
    read (T3);
  - the reset by `UploadedNotHeld` or `FoundCommitted` (T4).
  My scenarios R1 to R4 catch the first three. They are written and
  can be lifted as they are.
- **Two questions for the lead (Q1 and Q2).** One is about the
  15-minute wait against the 10-minute expiry of a publish. The other
  is about the ways of leaving segments behind that this wait does not
  cover.
- **Four nits in the texts (N1 to N4).**

### What I did

- **Reading.** I read design §5.2 and §5.3 and the notes' entries on
  M2.4c, M2.4e and the seed under load, where the lead decided the
  backoff. Then I read the diff and the code around it before the
  description, and then the description against the code.
- **Gates at the head** with rustc 1.100.0, the current stable:
  - `cargo fmt --all --check` and `cargo clippy --workspace
    --all-targets -- -D warnings`, plain, with
    `skyzfsd/test-hooks,skyzfs-local/test-hooks` and with
    `skyzfs-ublk-sys/control-test-program`. The results are in
    "Gates" below.
  - The tests of the touched crates, 0 failed in each:
    - `skyzfs-upload`: 46 in `tests/uploader.rs` (2 new), with every
      other test file and the library, which has the new backoff
      test.
    - `skyzfs-local`: 309 in the library (1 ignored), G3 and G4 among
      them.
    - `skyzfsd`: 137, with the test of the daemon's status.
    - `skyzfs-ublk`: the status tests, as each status plant's run.
- **Plants.** 25 textual plants, each made alone in a worktree of the
  head (`plants.py`, `plants.out`):
  - 8 are the author's W1 to W8, made again from their description;
  - 10 are my own on the wait, the driver, the status and the daemon
    (U1 to U10);
  - 7 are on the slices: Z06 and Z07, made again from my
    `review/m2.4e/plants3.py`, and G5, G8, G11, G12 and G13.
  - Each plant ran the tests of its own crate, the uploader's with my
    four scenarios appended, under a limit of 900 s, and the file was
    restored after each run.
  - The worktree was clean after every run.
- **Scenarios.** Four of my own, R1 to R4 (`scenarios_upload.rs`,
  appended to `tests/uploader.rs` for a run). All pass at the head
  (`scenarios.out`).
- **CI and Codex** at the head, read once each. There is no kernel
  here.

### (1) The wait: every way a whole cut is lost

The upload leaves segments in object storage, in no commit, whenever it
drops a cut's writes after it PUT some. Here is each such way in the
code, and what waits after it.

| Way the cut goes | Where | Which wait | Tested by |
|---|---|---|---|
| Abandoned at the bound during its read: the next slice is refused | `driver.rs:658` → `Lost{rule}` | **whole wait**, 60 s doubling to 900 s | the author's test; U7 caught |
| Its read fails (the store's error) | same | **whole wait** | same path |
| Its copies' sync fails | `driver.rs:705` | **whole wait** | none of the author's (U5 survives); my R3 |
| Gone at the commit: abandoned after its last slice | `driver.rs:739` | **whole wait** | none of the author's (U6 survives); my R4 |
| A commit that failed, read again: the cut gone | `commit()` → `driver.rs:739` | **whole wait** | as above |
| Commit refused (conflict), or row or head moved: `GaveUp` | `driver.rs:727-731` | the refusal's wait, 1 s doubling to 5 min, reset only by a commit (G7 of #114) | earlier tests |
| A slice the engine refuses (`write_slice`) | `driver.rs:660-665` | the error's wait, 1 s, reset by any step | unreachable: the engine refuses only a write past its end, and `create` refuses a remote of another size (`upload.rs` `place`) |
| A PUT or observe fails | stays in its stage | the error's wait; the cut is not lost, and the step is made again | earlier tests |
| The commit fenced, or the lease lost | `uploader.rs:551`, `turn` | none: uploads stop for the rest of the instance | earlier tests |
| The daemon dies or stops mid-upload; the next instance uploads the cut again from block 0 | next start | none: a new instance starts with no wait | not applicable |
| Mismatch at the check | `check()` | none needed: nothing was PUT yet | earlier tests |

**Every way a whole cut goes before its commit inside an instance takes
the new wait.** All three `Lost` sites pass the cut's own rule, through
`lost(&upload, ..)` (`driver.rs:789-800`). A delta cut lost does not
wait (`a_delta_cut_lost_does_not_wait`; W2 caught).

**What resets the wait.** Only `Uploaded`, `UploadedNotHeld` and
`FoundCommitted` do (`uploader.rs:539-547`): a cut committed, as the
lead decided.
- `Began`, `Read` and `CopiesSynced` do not. U1 makes any step that
  succeeds reset it, and it is caught.
- A `Lost` does not.
- A `Mismatch` does not, and should not.
- The errors' wait is still reset by the `Ok(Lost)` step
  (`uploader.rs:497-499`). That is right: no error is pending then.

**No busy loop, starvation or unbounded growth.**
- The sleep is `next_wait`: the earliest of the wait's end, the lease's
  renewal and a publish that is due (`uploader.rs:532`, `679-685`).
- `due_at` gives no time for a publish that waits for a cut
  (`publish.rs:751-757`), so a pending publish with no work does not
  make the sleep zero.
- The doubling saturates at `WHOLE_LAST`.
- During the wait the uploader makes no call to the queue's thread: no
  `uploadable()`, no read. The store goes on abandoning the unread cut
  at the bound for free, as with no uploader.
- `step_at` is moved only by the cut's own steps, so no publish step
  can shorten or lengthen the wait.

### (2) The lease and the publishes during the wait

- **The lease.** The author's test bounds every sleep in the wait to a
  third of the lease, and counts the turns. W5 is caught. But the
  test never moves the refs object's clock, so a lease that lapsed
  would not show there. U3 wakes at every renewal's time and renews
  nothing, and it **survives the author's tests**. My R1 moves the
  refs clock with tokio's time. Before every wake-up it checks that
  the row as last kept has not expired. It catches U3 and W5 (T2).
- **The publishes.** No test of the package asks for a publish during
  the wait. U2 lets no publish step during a whole wait, while the
  lease is still kept, and it **survives the author's tests**. My R2
  covers it:
  1. a cut is committed;
  2. another writer's row makes a mismatch, and the next cut is whole;
  3. that whole cut is lost, and the 60 s wait begins;
  4. the agent asks for a publish that the committed cut carries;
  5. R2 checks that the publish is done, and `snap/p` exists, before
     the wait ends.
  R2 passes at the head and catches U2 (T1).
- **The turns of #126.** While the cut waits, `Turns::next(due, false)`
  starts the count again and `took` counts nothing, because the two
  sides do not share the time. When the wait ends, the cut and the
  publishes share the time from zero. That is benign. #126's final
  head, F7, keeps the count across a wait. It is still benign, since
  only shared time counts. The merge of that head is still to come
  here, so R2 should be run once more after it.
- **A publish accepted during the wait** begins the next turn at once
  (`run_with`), and the publish's lookup and transaction follow with
  zero sleeps. That is not a loop: each is one step, and the publish
  then waits for a cut.

### (3) Status v19

- **The layout.** v19 is 1416 bytes with magic `SKYZST19`. The count
  `upload_cuts_lost_whole` is at 1408. State 10, `whole_lost`, keeps
  its wait in `upload_wait_ms` at 440.
  - The writer: `status.rs:1169`.
  - The reader: `status.rs:1316` reads the count only when the version
    is 19 or later.
  - Version 18 (1408 bytes) and version 17 are still read without it,
    and the version tests cover 8 to 19.
  - U9 reads a v18 file with the count, past the end of the file, and
    it is caught.
  - The documentation table matches the code: the new row, state 10,
    and `upload_wait_ms` for states 8 and 10.
- **The daemon.** It passes the state, the wait and the count on
  (`upload.rs:947-951`, `970`). U10 drops the wait, and it is caught.
- **The consumers tolerate v19.**
  - `tests/bench/skz_time.py` reads only `time_*` keys, and v19 adds
    none.
  - `topology-bench.sh` prints `upload_state` as text and reads
    `upload_cuts_lost`. It does not yet read the new key, which is
    testing's to add.
  - `tests/kernel/cut_daemon_check.py` compares `upload_state` with
    named words. `whole_lost` matches none of them, which is right: an
    upload course that loses a cut is already a FAIL through
    `upload_cuts_lost`.
  - In `cut-record.sh`, `drain()` waits up to 300 s for `idle`. A
    course that loses two whole cuts in a row (60 + 120 s) could
    reach that limit. It is judged anyway.
  - `status_version` is compared as ≥ 17 (`cut_daemon_check.py:1515`).
  - The cut rig through skyzfsd with uploads is green on this head
    (below).

### (4) b75a128: G3 and G4, test-only

- **The commit is test-only.** It touches
  `crates/skyzfs-local/src/inplace/state/tests.rs` alone.
- **G3** (`a_run_of_zeros_over_an_empty_volume_reads_the_map_and_hole_set_once_over`).
  - At the head the test counts 28,702 loads against a bound of
    4 × 20,480 = 81,920.
  - With Z06 it counts 33,574,942 loads; with Z07, 8,415,262. G3
    fails on both, and no other test does. That matches the author's
    claim and my figures from #125.
  - The run takes 0.08 s, and 0.55 s with Z06.
  - My G12, a stretch of one word, is caught by G3 and by the older
    `the_slices_of_a_large_hole_read_the_hole_set_once_over`.
- **G4** (`random_slices_over_several_stretches_read_each_block_once_as_the_holes_say`).
  - It is seeded (xorshift over seeds 0 to 63), so it is the same
    every run, and it takes under 0.1 s. It cannot be flaky.
  - Its model of what the slices must read is independent of the
    code: a chunk is a hole only where one range holds all of the
    chunk inside the volume, and a written block is never zeros.
  - It catches G5, a written block inside a hole chunk read as zeros.
  - It catches G8, a partial last chunk never a hole.
  - It catches G11, a run cut a stretch from its start.
  - It catches G13, the author's E3 made again.
  - The floors on its own cases (runs that cross a stretch's edge, ≥ 20,
    and runs that end at one, ≥ 5) make sure the cases reach what the
    test is for.

### Findings

No defects. The test gaps come first, the most useful first; then the
questions; then the nits.

**T1. Nothing tests that publishes step during a whole wait** (certain;
U2 survives). `uploader.rs:479` lets them through because the cut is not
due. A change that held them back until the wait's end, with the lease
still kept, passes every test of the package. R2 catches it. The PR
description claims "the publishes step during the wait", and R2 is the
evidence for that. I suggest taking R2, or one like it.

**T2. The lease's renewal is judged by the sleeps alone** (certain; U3
survives). `tests/uploader.rs:2943` and `:2985` bound the sleeps and
count the turns, but the refs clock (`Clock`) never moves in that test.
So a wait that wakes and does not renew passes. R1 moves both clocks
together and checks the row's expiry before every wake-up. It catches
U3 and W5. I suggest taking R1, or adding `clock.pass` and the expiry
check to `a_whole_cut_lost`.

**T3. A whole cut lost at its sync, or gone at its commit, is not tested
through the uploader** (certain; U5 and U6 survive).
- The driver's tests cover these two `Lost` paths, but not the rule
  they carry or the wait that follows.
- The author's test loses whole cuts only at a read.
- R3 covers the sync: the sync of `copies.img` fails, the state is
  `WholeLost`, the wait is 60 s and `lost_whole` is 1.
- R4 covers the commit: a whole cut is abandoned after its last
  slice, and the state is `WholeLost` with 60 s.
- All three sites go through one `lost(&upload, ..)`, so the risk of
  a regression is low. The PR's "every way a whole cut is lost waits"
  is the claim these two scenarios pin.

**T4. Nothing tests that `UploadedNotHeld` or `FoundCommitted` end the
wait** (certain; U4 survives everything, mine included).
- U4 makes `Uploaded` alone end it.
- Inside an instance, `FoundCommitted` after a whole loss needs another
  writer to have committed the very cut, so it is near unreachable.
- `UploadedNotHeld` is reachable: a whole cut committed and abandoned
  at the bound before its release, which is what a seed under load
  could do.
- Low. A test would need the queue between the commit and the
  release, as `committed_then_taken` does for the driver.

**Q1 (lead). The longest wait, 15 min, is longer than a publish's
default expiry, 10 min (`publish.rs:108`).**
- A whole cut is not only a seed. It is also the first cut after a
  mismatch, and the first of a new state on a volume with commits.
- When such a cut is lost under load, the recovery point and every
  publish wait at most 15 minutes after the load drops, plus the
  upload itself.
- Publishes asked in the first minutes of a long wait expire before
  the next attempt begins. The agent asks once a minute, so the later
  ones land on the cut after the seed.
- This follows from the decision as made. I raise it so that it is
  chosen on purpose: lower `WHOLE_LAST`, or say so in the notes.

**Q2 (lead). Other ways of leaving segments that this wait does not
cover.**
- A whole cut given up at its commit takes the refusal's wait, 1 s to
  5 min (G7 of #114), not this one.
- A new instance starts with no wait. A daemon that restarts in a loop
  under load leaves one attempt's segments per start.
- `write_slice`'s path is unreachable today.
- All of this agrees with the decision's words ("an abandoned whole
  cut"). It belongs in the notes' list for M4's collection.

**N1. The reason is said twice for a cut gone at its commit.**
- `driver.rs:739` gives "it went before its commit", and
  `uploader.rs:529` puts "whole cut N went before its commit: " in
  front.
- The status then reads: "whole cut 2 went before its commit: it went
  before its commit: the volume cannot be seeded under this load,
  next whole cut in 60 s".

**N2. The status says "the volume cannot be seeded under this load"
for every whole cut lost** (`status.rs:738`; `uploader.rs:218-220`).
That includes a failed sync of the copies file, which is an I/O error
and not the load, and a whole cut after a mismatch on a volume seeded
long ago. Something like "no whole cut can be uploaded now" would hold
for all of them. The reason after it already says which case it is.

**N3. The version history says something that cannot happen**
(`status.rs:21-24`). It says: "A reader of an earlier version reads
that state as 'not said'". A reader of an earlier version refuses a
v19 file by its magic and never sees the state. Only the readers of
the text output, the rigs' judges, meet the new word `whole_lost`.

**N4. "next whole cut in N s" is the wait as it began, not the time
left** (as for state 8's "retry in N s", and as `upload_wait_ms` is
documented). Fourteen minutes into a 900 s wait the line still says
"in 900 s". It is consistent, so it needs no change. "after a wait of
N s" would read truer.

### Plants

These are the results from `plants.out`. W4 and W5 were run a second
time, after I made R1 check the row before each wake-up. Those are the
lines below.

| Plant | Change | Result |
|---|---|---|
| W1 | a landed commit does not end the wait | KILLED (author's) |
| W2 | a delta cut lost waits too | KILLED (author's `a_delta_cut_lost_does_not_wait`) |
| W3 | a whole cut lost takes the error's wait | KILLED (author's; R1 to R4) |
| W4 | no wait, the state kept | KILLED (author's; R2) |
| W5 | the losing turn sleeps out the wait | KILLED (author's; R1) |
| W6 | the longest wait is an hour | KILLED (author's, and the backoff's unit test; R1) |
| W7 | the first wait is a second | KILLED (author's; R1 to R4) |
| W8 | a whole cut lost is not counted | KILLED (author's; R3) |
| U1 | any step that succeeds ends the wait | KILLED (author's; R1) |
| U2 | no publish steps in a whole wait, the lease kept | **SURVIVES the author's**; R2 catches it (T1) |
| U3 | wakes at each renewal's time, renews nothing | **SURVIVES the author's**; R1 catches it (T2) |
| U4 | only `Uploaded` ends the wait | **SURVIVES** (T4) |
| U5 | a whole cut lost at its sync is taken for a delta | **SURVIVES the author's**; R3 catches it (T3) |
| U6 | a whole cut gone at its commit is taken for a delta | **SURVIVES the author's**; R4 catches it (T3) |
| U7 | a whole cut lost at a read is taken for a delta | KILLED (author's; R1, R2) |
| U8 | the whole wait capped at 5 min | KILLED (author's) |
| U9 | a v18 file read with the count | KILLED (author's status tests) |
| U10 | the daemon drops the whole wait | KILLED (`the_status_shows_the_uploader_as_it_is`) |
| Z06 | `hole_run_end(block, stop)` | KILLED by G3 alone |
| Z07 | `first_in(.., block, stretch)` | KILLED by G3 alone |
| G12 | a stretch of one word | KILLED (G3; `the_slices_of_a_large_hole_read_the_hole_set_once_over`) |
| G5 | a hole run not cut at the stop | KILLED (G4 and 15 others) |
| G8 | a partial last chunk never a hole | KILLED (G4 and 9 others) |
| G11 | a run cut a stretch from its start | KILLED (G4, G3, and one older test) |
| G13 | a hole run ends at a word of the set (E3) | KILLED (G4, G3, and one older test) |

Of the author's 19 plants I made W1 to W8, Z06 and Z07 again, and G13
is their E3. All 11 are caught, by the tests the description names.

### Gates

All at `b75a128` with rustc 1.100.0, the current stable:

- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean
  plain, with `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with
  `skyzfs-ublk-sys/control-test-program`.
- Tests, plain, 0 failed in each:
  - `skyzfs-upload`: every test file, `uploader.rs` 46.
  - `skyzfs-local`: 309 in the library, 1 ignored.
  - `skyzfsd`: 137.
  - `skyzfs-ublk`: 160 + 1 + 1.
- With the test hooks, 0 failed: `skyzfs-local` 309 and 16, and
  `skyzfsd` 176.

### CI and Codex at b75a128

All of it is green. 36 checks passed and 2 were skipped: the replay
rig's reference jobs, as on a pull request that touches only the
daemon's code.

- `ci` 44439241462, with `unit-arm64`.
- `skyzfs-ci` 44439119906, from the push.
- `coverage` 44439241613.
- `e2e` 44439241381.
- `k3s-smoke` 44439241707.
- The control rig 44439241356, on three images.
- The cut rig through skyzfsd, 44439241349. It ran on 26.04, on
  26.04 arm, and on 26.04 with uploads, which runs the upload
  courses.
- The replay rig 44439241361: the jobs through skyzfsd and the jobs
  that write into a hole.
- The daemon rig 44439241578, on 24.04, 26.04 and 26.04 arm.
  - I read the 26.04 job's own table, not its colour: 55 PASS,
    5 FAIL, 2 NA and 1 WAITING, and the job ended with exit 0.
  - The 5 FAIL are the controls in the table `EXPECTED`, and the
    WAITING is the pin with a request held.

**Codex** reviewed `b75a128` with no findings
(`has_any_findings: false`).

### Files

- `plants.py`: the plant script. `plants.out`: its results.
- `scenarios_upload.rs`: R1 to R4. `scenarios.out`: their run at the
  head.

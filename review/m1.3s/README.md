# M1.3s (skyzfs/skyzfs#46): the contract's power cut is the volatile-cache disk

`skyzfs-coding` at 4424460 into `zfs-topology` (merge base b93f495, one
commit). Posted as
https://github.com/skyzfs/skyzfs/pull/46#issuecomment-6842779626.

**Verdict: one finding blocks as the change is worded, and needs the lead's
ruling more than code. The model's rule matches §4.1 and the store passes the
suite with real cuts. Eight more findings, none blocking.**

Findings:

1. **Blocks.** A failed volume reopened without losing the kernel's cache does
   not keep what its start shows. With `reopen_failed` as the log store's
   harness has it (`disk.kill()`), `reopened_failed_volume_holds_what_was_flushed`
   fails ("block 4 is Torn"). Probe N: at 62 of 64 seeds a block the reopened
   volume showed is different after a completed flush and a power cut. A
   failed sync leaves sectors clean in the cache and never written; no later
   sync writes them. The change makes the harness cut the power before the
   reopen, and the documentation says R11 holds as for the log store. In the
   daemon the hole is narrow (a volume held at its failure is not served again
   in that boot).
2. At a start, the failure note waits for the marker: `assemble` fails, the
   store is dropped, `Drop` joins the marker's thread, and only then does
   `serve` write the note and delete the device.
3. The marker's thread is started at the moment of failure, on the queue's
   thread, under `mlockall(MCL_FUTURE)`: 2 MiB of stack at the moment memory is
   likely short, and the fallback writes inline.
4. The marker's test asserts the store's state: with `fail()` waiting for the
   thread before it returns, all 18 in-place tests pass.
5. After a cut the model holds a discarded block to one content (probe A:
   accepted before this change, refused now). §4.1 lets the reads differ.
   Removing the `Inconsistent` check fails no test.
6. `SECTOR = 256` in the model passes every test.
7. The harness's `power_cut` as a process loss passes every test.
8. `into_in_place` on a failing disk has no test (probe M: it works at each of
   its three syncs).
9. Smaller: `Failed` also for a helper's end; `serve.rs`'s module documentation
   and the help on exit 3; a model test that does not test the floor; the
   closure in `flushed` (probe B); the log store no longer held to a prefix by
   the suite.

What was run: `plants.sh` (m1 to m6 on the model, h1 to h4 on the harness, i1
to i4 on the marker), the probes in `probes-model.rs` (appended inside the
`tests` module of `model.rs`) and `probes-inplace.rs` (appended to
`inplace/tests.rs`).

| Plant | Result |
|---|---|
| m1 `SECTOR = 256` | all pass |
| m2 no floor in `versions` | 1 test fails |
| m3 what was read is not made durable | 3 fail |
| m4 a second, different read at the read-back accepted | all pass |
| m5 the floor exclusive | 1 fails |
| m6 whole blocks in place of sectors | 1 fails; the in-place suite fails |
| h1 the harness's cut loses nothing | all pass |
| h2 the reopen without a cut | the suite fails (finding 1) |
| h3 no cut points | the suite's test fails |
| h4 garbage in the cut | the suite fails |
| i1 `fail()` waits for the marker's thread | all pass |
| i2 `Drop` does not join | 4 fail |
| i3 the thread does not write | 3 fail |
| i4 the writer dropped before the thread | 3 fail |

Probes: J, the suite against `FLUSH_JOINS_ANY_SYNC`: 0 of 8 seeds on the two
harnesses whose syncs end at once, 3 of 8 on the one with late syncs, through
`prop_failures_match_model`. K, six other seeds with no defect: 18 runs pass.
L, which cases notice: a flush that syncs nothing fails the power-cut cases.

Not done: nothing on a kernel; clippy and the feature builds; the rig jobs of
the pull request had not finished.

## After the first look: the lead's rulings (2026-11-13)

- Finding 1: every start, after its sync, drops the kernel's cache of the
  volume's file, checks that it is gone, and serves only what the disk holds;
  it refuses the volume if it cannot (§4.1, §5.1 at `zfs-topology` 86d2441).
  The suite's reopen goes back to a lost process without a power cut, so the
  case pins it. Still blocks #46 until it is in.
- Finding 5: the model must not be stricter than §4.1 on a discarded block's
  reads; fixed if small, else owed.
- Finding 2: the note before the marker at a start, as while serving.

For the second look:

- How the cache is checked. The daemon runs under `mlockall(MCL_FUTURE)`: a
  check by mapping the image and `mincore` would read the range in and lock
  it. `cachestat` (Linux 6.5) needs no mapping.
- `POSIX_FADV_DONTNEED` is advisory and drops only clean pages nothing maps or
  locks; the check is the only evidence.
- Plants to run: a drop that does nothing (the reopen case must fail); a check
  that always passes (a test of the refusal must fail); `plants.sh` h2 is now
  the harness as it should be.

## Second look, at 2eb79ec (2026-11-13)

Posted: https://github.com/skyzfs/skyzfs/pull/46#issuecomment-6843979599

Verdict: one new finding blocks as §4.1 is worded (A); the first look's
findings are answered (9e goes with M1.3t).

Heads: 1280579 (the answer to the review; the plants and probes ran here) and
2eb79ec (finding 9d, `model.rs` only; read, tests run).

- **A (blocks).** On an overlay file system `cachestat` through the file's
  descriptor reads 0 whatever the cache holds: it counts the descriptor's own
  mapping, and an overlay file's pages live in the file below. The drop is
  passed down and works. So the start's check passes where the drop left every
  page. Run here on 6.18, overlay over ext4 (`cachestat.py DIR`,
  `cachestat2.py MERGED UPPER`): 0 through the overlay, 256 through the file
  below after a write of 1 MiB. Probe S (`probe-s.rs`, appended to
  `tests/inplace_real.rs`, with `PROBE_DIR` and `PROBE_BELOW`): another process
  maps the home image; on ext4 the start refuses ("4096 pages of it stay"), on
  the overlay it serves with 4096 pages of the file below cached before and
  after. Plant c6 (the real count answers 0): all 23 in-place tests and the 3
  real-file tests pass. Remedy offered: read a block through the cache first
  and require the count to see it.
- **B.** Inside the recovery window the discard's freedom is judged read by
  read against any world (Codex's thread; probe Q in `probe-q.rs`): block 0 as
  0, 1, 2 all accepted; after the window 1 then 2 is refused.
- **C.** tmpfs: 2 of 3 real-file tests and the count's own test fail (the start
  refuses, rightly). Overlay: the real-file tests pass, the count's test fails.
- **D.** Not pinned: the lock before the marker in `FailedStart::write` (f2),
  the second try of the drop (c4). The two marker tests wait without a bound.
  The refusal's message; the parked thread's 2 MiB stack under `mlockall`.

`plants2.sh` (run from a clean checkout of 1280579): c1 no drop: 5 in-place
tests fail. c2 check always passes: 1. c3 drop and check before the sync: the
in-place suite. c4 one try: passes. c5 real drop does nothing: 2 of 3
real-file tests. c6 real count answers 0: passes. f1 the failure waits for the
marker: 1 (by its 60 s bound). f2 lock before the marker: passes. f3 a failed
start writes its marker before it returns: 1. m1 `SECTOR = 256`: 1. m4 a
second answer never refused: 1. m7 no discard freedom after a cut: 1. The
first look's h1 (the harness's cut as a process loss) now fails
`the_suite_notices_the_defects`.

On finding 2: at a take-over of a volume served in place the log is empty, the
checkpoint returns at once (`store.rs:1242`), so the first sync that carries
the dead instance's unflushed writes is the in-place store's own, which is the
one that now hands the marker back.

For the lead, not in the review: the design says nothing at a start can show
that the file holds less than the kernel was told. `syncfs` on a newly opened
descriptor reports a writeback error on the file system that no `syncfs` has
reported yet (the superblock's error sequence, Linux 5.8), also one the dead
instance's `fdatasync` already took. From memory, not tried: no device here
can fail a write.

Not done: nothing on ublk; the cost of the drop at a take-over; clippy and the
feature builds; the rigs of both heads were still running.

## Third look, at fe54d96

Posted as https://github.com/skyzfs/skyzfs/pull/46#issuecomment-6844771666.
Read as `git diff 318e013 fe54d96` (318e013 is a clean merge of `zfs-topology`
at a54b78b): six files, adbd490 and fe54d96.

**Verdict: one new finding blocks (E). A fixed as to the overlay; B and D
dropped; C answered, one point left that does not block.**

- **E (blocks).** `count_sees_the_file` (`inplace.rs:490`, `:520`) reads block 0
  through the cache just before the drop. The kernel reads ahead (4 pages on
  6.18 ext4). Where block 0 is a hole the read returns at once while the pages
  after it are under I/O; `POSIX_FADV_DONTNEED` leaves them; the second try
  comes microseconds later. The start refuses: "2 pages of it stay in the cache
  after two drops". A pool's label at byte 0 (design §4.2) is a hole of 8 KiB,
  then data.
  - CI at this head: replay rig through `skyzfsd`, ubuntu-26.04 (kernel 7.0, XFS
    on a loop file), run 43368460688, job 131700180830: exit 1 with that line at
    the restart after `zpool create` and export (`serve.log` in the artifact).
  - `probe-r.rs` (appended to `tests/inplace_real.rs`; `PROBE_DIR`,
    `PROBE_STARTS`, `PROBE_BLOCK`): blocks 2 and 3 written: 731 of 1500 starts
    refused; blocks 1 and 2: 728; blocks 0 and 1: 0.
  - `race.py DIR N`: the same sequence in a script, five layouts, 3000 tries
    each: only "hole then data" leaves pages (500 after one drop, 24 after two).
  - The order was my own remedy at the second look, untried then.
  - Remedy tried in a scratch checkout: the look after the drop has counted 0
    (move the call into the `left == 0` branch). Probe R: 1500 of 1500 served
    for both failing layouts; 25 in-place tests and the real-file tests pass.
    `race2.py`: that order and `POSIX_FADV_RANDOM` before the read both give 0
    of 3000.
- **A.** `probe-o.rs` (one start of a new volume in `PROBE_DIR`): ext4 serves;
  an overlay refuses, "the file system's cache cannot be observed through this
  file"; tmpfs refuses with the same line (a hole read keeps no page there).
- **C.** With `TMPDIR` on an overlay or tmpfs the real-file tests and the
  count's test skip; the reason shows only with `--nocapture`, a plain
  `cargo test` prints "ok. 3 passed". Suggested: a skip is a failure where `CI`
  (or a variable the workflows set) says so.
- Plants (`plants3.sh`), all as the answer says: s1 the start does not look
  (2 tests fail), s2 the look accepts 0 (1), s3 one try (1), s4 three tries (1),
  s5 lock before marker (1), s6 the store's call counts 0 (2 of 3 real-file
  tests), s6b the binding counts 0 (nothing fails, three skips), s7 the model
  does not narrow the worlds (1).
- Tests at fe54d96: `skyzfs-blk` 61 + 11 + 29 + 24 + 4 + 7; `skyzfs-local` 268
  passed, 4 ignored, real files 3 + 3 + 12; `skyzfs-ublk-sys` 7; `skyzfs-ublk`
  142 + 1 + 1; `skyzfsd` 53 + 3.
- A trap met: a removal of `$d/*` in a loop is stopped by the harness's safety
  check; make new directories with `mktemp -d` instead.

## The fix for E, at 4e0d26d (checked ahead of the fourth look)

`git diff fe54d96 4e0d26d`: the look comes after the drop has counted 0; a new
real-file test, `every_start_serves_where_the_image_begins_with_a_hole` (blocks
2 and 3 flushed, 200 starts).

- Tests: `skyzfs-local` 268 passed, 4 ignored, real files 4 + 3 + 12; the new
  test 5 runs of 5.
- Probe R: 1500 of 1500 served for data from block 2 and from block 1.
- Plants: s1 to s4 as before. s9 (the order of fe54d96 exactly: the look
  before the drop, none after) fails the new test; the 25 in-place tests on the
  simulated disk pass, as they must (no read-ahead there).
- s8 (a look before the drop and the look after kept as well) fails nothing:
  the look after leaves blocks 0 to 3 in the cache, so the next start's early
  read starts no read-ahead. The test's pin rests on each start ending with an
  empty cache; dropping the image's cache from the test before each start would
  make it hold for any read before the drop. Told coding, not blocking.

## Fourth look, at 6f98d25

`git diff 4e0d26d 6f98d25`: the order sits behind a mutant
(`LOOK_BEFORE_DROP`); the simulated disk can hold a read's read-ahead under
I/O (`SimDisk::hold_read_ahead_of`) and `the_look_comes_after_the_drop` uses
it; a skip of a real-file test or of the count's test panics where `CI` is set
(non-empty and not "false"); the hole test drops the image's cache before each
start; the refusal no longer names the overlay as the cause.

- Tests: `skyzfs-blk` 61 + 11 + 29 + 24 + 4 + 7; `skyzfs-local` 269 passed, 4
  ignored, real files 4 + 3 + 12; `skyzfs-ublk-sys` 7; `skyzfsd` 53 + 3.
- The hole test: 20 runs of 20. Probe R: 1500 of 1500 for both layouts.
- The `CI` rule: with `CI=true` on ext4 all pass. `TMPDIR` on tmpfs: `CI`
  unset or "false", four "ok" (skipped); `CI=true`, the three real-file tests
  that touch files and the count's test fail, "not skipped where CI is set".
  The fourth real-file test is the kill test's child, which returns unless the
  kill test started it.
- Workflows that run `cargo test` (`ci.yml`, `coverage.yml`, `e2e.yml`,
  `skyzfs-ci.yml`, the control rig): none in a container, all on
  `ubuntu-latest`.
- Plants (`plants3.sh`, t1 to t5): t1 the look always before the drop, and t2 a
  look before the drop with the look after kept, each fail the hole test and
  the simulated disk's test; t3 the binding counts 0 with `CI=true` fails the
  count's test and three real-file tests; t4 is t2 with the hole test's own
  drop removed, and nothing fails (the test's drop is what catches t2); t5 the
  start does not look fails three in-place tests; s2, s3, s4 as before.
- What is left of E's cause, not blocking: a drop leaves pages under I/O and
  the two tries are microseconds apart, so a start whose drop falls within a
  disk read begun by another reader of the image refuses. A start takes about
  6 ms here (200 in 1.2 to 1.5 s), a read far less; 0 refusals in 7000 starts.
- CI at 6f98d25: the replay rig through `skyzfsd` on ubuntu-26.04 (run
  43372945268, the job that showed E) is green; its artifact has the start
  after `zpool create` served, 132 starts on pool images and no refusal. `test`,
  `lint`, the 24.04 replay job and the control rig are green. The daemon rig's
  two jobs were still running when the look was posted
  (https://github.com/skyzfs/skyzfs/pull/46#issuecomment-6845514957).
- After the look was posted: the daemon rig at 6f98d25 (run 43372945297) is
  green on both images; each artifact has 20 kills with writes in flight
  (`pin_kill_under_load`), 28 kills in the conformance suite, and no line with
  "stay in the cache", "could not empty the kernel" or "cannot be observed".
  `coverage` is green. The comment was edited to say so; lead and coding told.

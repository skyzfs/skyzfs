# M1.3x (#55): the in-place store's failure gets its own reason

Reviewed at 6355607, base `zfs-topology` 0726a81. Review comment:
https://github.com/skyzfs/skyzfs/pull/55#issuecomment-6849640783

Verdict: at first look nothing blocked; corrected an hour later to **one
finding blocks** (comment
https://github.com/skyzfs/skyzfs/pull/55#issuecomment-6849901500), on the
artifact of check g run against this build (run 43400705709): a failed
start of the in-place store still writes the log's reason (code 1), and
the refusal for the rest of the boot repeats it, while the serving case
is fixed. `StartFailed::Image` comes only from `assemble`'s own
`home.sync()`; everything the log store's part of the hand-over sees
keeps code 1, including where the log is empty. (My guess at the
mechanism then, a device flush failing under the log's sync, was wrong:
see the fourth look.)

## What is here

- `plants.py WORKTREE [name...]`: plants eleven defects, one at a time,
  runs the crate's tests for each and says which tests fail. Each file
  is put back after its plant. The three `serve.rs` call-site plants
  (s1, s2, s3) are caught by nothing; the other eight are caught.
- `removed-note.rs`: the reproducer for finding 2. Copy it to
  `crates/skyzfs-ublk/tests/` in a worktree at the pull request's head
  and run
  `cargo test -p skyzfs-ublk --test removed-note -- --nocapture`
  (rename the file to match the test name). It fails the volume while
  serving, rewrites the note's reason to an unknown code (8, which
  stands for what 7 is to an older daemon), starts, removes the note by
  hand and starts again, with and without `--acknowledge-failure`, for
  a client that holds the device and one that does not.

## What the findings rest on

1. **No test pins the daemon's three call sites** (`serve.rs:800`,
   `1051`, `1125`). `each_store_notes_its_own_failure` tests the two
   helper functions, not that `run` calls them. The three plants that
   write `Reason::LogFailed` at the call site leave 57 tests passing.
   The third plant is the state at the tip before this change: the
   defect the work package exists to fix.
2. **"Remove the note by hand" does not work.** The store's marker
   (`log.failed`) outlives the note, and the marked volume can be
   acknowledged only when no attachment record of this boot names a
   device (`lifecycle.rs:829`), which is never true after a failure in
   this boot. Removing the note only changes the refusal's words, and
   the start then writes a fresh note saying a client held the device.
   A reboot clears it; a same-boot remedy must name the marker too.
3. **Code 1's words reach a start that served nothing**: "the log
   failed while the volume was served" is given where the log store's
   checkpoint failed at an in-place start. Pre-existing, now
   deliberate.

## Run here (kernel 6.18, amd64, no ublk, device-mapper or ZFS)

- `cargo test -p skyzfs-ublk -p skyzfs-local -p skyzfsd`: 496 passed,
  0 failed, 6 ignored; `-p skyzfsd --features test-hooks`: 57 passed.
- `cargo fmt --check` clean; `cargo clippy --all-targets -D warnings`
  on the three crates clean.
- The eleven plants, and the reproducer above.

## Traps met

- Nothing of the daemon's `run` can be exercised without a kernel, so
  the call-site gap cannot be closed by a unit test: it belongs in the
  daemon rig's check g, which does not read the reason today.


## Third look, at 7808412

Comment: https://github.com/skyzfs/skyzfs/pull/55#issuecomment-6850175555

Findings 2 and 3 are fixed and verified by running:

- the reason now renders "the log store's log failed"; planting the old
  words back fails `status::tests::the_report_shows_a_failure_of_this_
  boot_and_what_it_needs`, and no rig matches either wording (grepped
  `tests/kernel` at #54's head).
- the corrected remedy paragraph matches the code. `removed-note.rs`
  now runs all of it: the note alone removed leaves the volume refused
  as marked (and writes a fresh note); the note and `log.failed`
  removed, it serves; after a reboot the marker alone is refused
  without the flag and acknowledged with it.

The blocking finding (a failed start still reports the log store) is
untouched at this head; coding's answer and 7808412 crossed with the
correction. The rewording makes the g.3 line "the volume failed at the
start of the in-place store (the log store's log failed)" — no longer
false on its face, but still the log store's name on an image failure.

Gates at 7808412: 496 passed over the three crates, 0 failed, 6
ignored; fmt and clippy clean.


## Fourth look, at 754a15a

Comment: https://github.com/skyzfs/skyzfs/pull/55#issuecomment-6851711010

Verdict: the blocking finding is fixed, nothing blocks, it can merge.
The rule now (the lead's ruling, my option 1): a failed hand-over is
the image's (code 7) when the log store's index is empty, read before
the hand-over, and the log store's (code 1) otherwise.

- `what-each-start-says.rs`: append to
  `crates/skyzfs-local/src/inplace/tests.rs` in a worktree and run
  `cargo test -p skyzfs-local --lib zz_review -- --nocapture`. For a
  log that never held an operation, one emptied by an earlier start in
  place, one with a write and one with a discard only, it fails each
  sync of a start in turn, and every sync of each file, and prints what
  the start reports.
- `plants-handover.py WORKTREE`: six plants in `into_in_place_with`.
  Always-Log is caught by the new test only, always-Image by the old
  one only, the inverted test by both, "start equals end" by the new
  test. Two are caught by nothing: a store that had failed before the
  hand-over saying Image (not reachable from the daemon), and the index
  read after the hand-over (no difference with one batch).

What it showed:

- **Every open seals the log with a record of its own** (`Log::seal`),
  so the log's start never equals its end at a hand-over, and the
  hand-over's checkpoint always takes a batch: a sync of `home.img` and
  the log's give-up (`log.super`), operations or none. That is why g.3
  failed in the log store's part with "0 operations" in the log, and
  why "the log holds no record" could not have been the test.
- The failing step is the checkpoint every time, never the log's flush
  (after an open the log has nothing to make durable; `log.000000` is
  not synced by a start at all).
- A discard never enters the index, so a log with discards and no write
  counts as nothing to apply and reports the image.
- With a write in the log, a start that fails on `home.img`'s sync
  still reports the log store: by the ruling, until M1.3t.

Kernel: run 43413318350 (testing's M1.T26 branch at 1bf6f47, daemon
built from 754a15a, ubuntu-24.04 only): g.3's start and refusal and
g.1's line and refusal all name the image. Gates here: 497 passed over
the three crates, 57 with the hooks, fmt and clippy clean.

## Traps met (fourth look)

- To see which step of a chained `and_then` fails, split it for one run
  and print; guessing from which syncs exist led me to a wrong
  mechanism two looks ago.
- A plant driver that filters tests (`--lib inplace`) can report
  "caught by nothing" for a plant another crate's tests would catch:
  run a surviving plant against all three crates before saying so.

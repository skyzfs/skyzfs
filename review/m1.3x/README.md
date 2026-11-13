# M1.3x (#55): the in-place store's failure gets its own reason

Reviewed at 6355607, base `zfs-topology` 0726a81. Review comment:
https://github.com/skyzfs/skyzfs/pull/55#issuecomment-6849640783

Verdict: nothing blocks. Three findings, none blocking.

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

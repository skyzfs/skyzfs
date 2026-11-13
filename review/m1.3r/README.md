# M1.3r: an in-place store (pull request 43)

Head 3dd9288 on `skyzfs-coding`, on `zfs-topology` 9b6439c. Eleven files,
2221 lines added: `crates/skyzfs-local/src/inplace.rs` with its harness and
tests, `tests/inplace_real.rs`, the entry `Store::into_in_place`, and in
`skyzfsd` the option `--store log|in-place` and a `Target` enum. The default
store is unchanged. The design at 9b6439c does not mention the package; its
criteria are in the pull request's description.

## First look: nothing blocks it as an experiment

Posted on the pull request. What stands behind it:

- **Built and run.** `cargo test -p skyzfs-local`: 258 passed and 3 ignored in
  the library, and 3, 3 and 12 passed in the three integration tests.
  `cargo test -p skyzfsd`: 56 passed, with and without `test-hooks`.
  `cargo fmt --check` clean; `cargo clippy -p skyzfs-local -p skyzfsd
  --all-targets -- -D warnings` clean, with and without the test hooks. No
  `unwrap`, `expect` or `panic!` in `inplace.rs` or `serve/target.rs`.
- **Seven plants of mine** in `inplace.rs` (`plants.sh` here, with `plant.py`),
  each against the 15 in-place tests of the library:
  - caught: a successful write not counted as unsynced (6 tests fail); waiting
    flushes completed without their own sync (3); a failed volume that still
    reads (3); a failure that leaves no marker (2); a failed write that does
    not fail the volume (3);
  - not caught: **a discard not counted as a change** (15 pass), and no sync
    at the start (15 pass; the description says so: the entry's checkpoint
    syncs the image as well).
- **The harness's `fail_log`.** With `halt_after(0)` replaced by
  `fail_write(0, true)` and `fail_sync(0, true)`, the suite fails two cases,
  `reopened_failed_volume_holds_what_was_flushed` and
  `prop_failures_match_model`, both with "block N is Torn" on a block the
  failed write was writing (minimal input `[FailLog, Write(0, 1)]`). So the
  halt is what lets the suite pass, the failure is the stated weaker promise
  and not lost flushed content, and the suite's report carries no mark of it.
- **The flush rule, traced by hand** (`flush`, `begin_sync`, `synced`,
  `settle`): a flush joins the sync under way only when nothing was executed
  since it began; otherwise it waits, and the next sync begins when this one
  ends; `waiting` is never non-empty with `unsynced` false; a failed write
  during a sync fails the flushes under way and leaves the helper's result
  unread, which is harmless on a failed volume. The helper sends its result
  before it wakes (`fs.rs:379-382`), and it syncs through a duplicate of the
  descriptor, which shares the open file and so its record of write-back
  errors.
- **The pump and `acknowledged() == None`** (`pump.rs:971-974`): no position
  is stored and the commit goes on. The emptied log keeps its position (floor
  equals tail), so a later start with the log store passes its check against
  the record. The pump polls the store after every submit
  (`pump.rs:640-662`), so requests completed inside `submit` are collected
  without a wake.
- **Not run by any test:** the daemon with the in-place store. `Target::InPlace`
  appears in `serve.rs` and `target.rs` only; the tests of `skyzfsd` parse the
  option and no rig or workflow passes `--store in-place`.
- **`InPlace::wait` ignores its timeout** (`inplace.rs:524-530`), against the
  trait's "or `timeout` passes". The pump calls `poll` only.
- **Wording.** Point 3 of the module documentation puts a sector that is
  neither old nor new down to the disk. A file system can do the same: ext4
  with `data=writeback`, which is how the CI runners' root is mounted, may
  show a block allocated since the last flush with bytes it never wrote.

Not done: the whole workspace's tests and `cargo doc` were not run; nothing was
run through ublk or ZFS; `tests/inplace_real.rs` was run, not read line by
line.

## Second look: d1641a4 (delta from 3dd9288, three files in `inplace`)

Read the delta before the author's answer, then the answer.

**Verdict: the five findings of the first look are answered and the lead's
change to criterion 1 is done. One new finding, which I missed the first time
and an automated review thread on the pull request raised: after a switch back
to the log store, a flush does not cover what the in-place store acknowledged.
Reproduced. In my view it is due before a merge; it does not touch the
measurements.**

- **Criterion 1 as the lead ruled it: done.** `fail_log` uses `fail_write(0,
  true)` and `fail_sync(0, true)` (the flag is "sticky"; a failed write always
  may leave any subset of its sectors). `run_suite` is `run_all` but for the
  two cases of `NOT_MET`, reported as not exercised with `NOT_MET_REASON`; the
  acceptance test asserts the reason word for word and that no other case is
  idle for it. `the_cases_not_met_fail_on_torn_blocks_and_nothing_else_does`
  runs `run_all` and asserts exactly those two fail, each with "is Torn".
  Plants: with the halt back in `fail_log`, that test fails; with only the
  write fault, it and the suite's mutant test fail.
- **Plants at this head** (`plants2.sh`, thirteen): caught: a write not
  counted as unsynced (6 tests), the discard not counted (2, the new test and
  the pin), waiting flushes without their sync (3), a failed volume that still
  reads (4), no marker (2), write errors ignored (4), a failed volume that
  still takes writes (1), `wait` that never waits (2), the halt again (1),
  writes-only fault (2). Not caught: no sync at the start (as before, see
  below), `wait` with a zero timeout waiting as before (nothing pins the new
  rule), `fail_log` with faults that strike once (no difference to the suite).
- **Why no test needs the start's own sync.** Traced the simulated disk's
  syncs with the store's sync taken out: every start still syncs `home.img`,
  because the entry's flush puts a record in the log and the checkpoint that
  applies it syncs the home image. So the start is safe twice over, and the
  comment in `assemble` is right.
- **New: back to the log store, a flush does not cover what the in-place
  store acknowledged.** `Store::open` syncs the home image only when it
  restores preimages; the log store's flush syncs the log only. Sequence
  (`repro-back-to-log.rs`, appended to `inplace/tests.rs`, run, taken out):
  in-place store, write block 7 (acknowledged, no flush); the process is lost;
  `Store::open`; the log store reads 7 as written, then acknowledges a write
  to block 9 and a flush; power cut that keeps nothing unsynced; open: block 9
  holds its write, block 7 holds zeros. The control (the second start in place
  again) keeps both. That is rule R7 broken on the default store, on a volume
  that was last served in place, until the log store's first checkpoint syncs
  the home image or the kernel writes the pages back by itself. The daemon
  has no check on a change of `--store` between starts and syncs nothing more.
  The module documentation says a volume "can go back and forth between the
  two"; `a_volume_goes_back_to_the_log_store` reads the block and stops there.
  Raised first by the automated review on the pull request
  (discussion_r4558227512), at the first commit; I did not see it in my first
  look.
- **The other automated thread** (the failure marker written on the queue's
  thread, discussion_r4558366408): `fail()` does call `log.mark_failed()` on
  the caller's thread. It is of a piece with the store's design (reads and
  writes of the home image block that thread too). Not traced further.
- **Small:** the pin test's comment says the two cases fail "not on anything a
  flush covered"; the test checks "is Torn" only, which the model says of any
  block that mixes its versions, covered or not. The weaker-promise test is
  what holds flushed content.

Verified: `cargo test -p skyzfs-local` 260 passed and 3 ignored in the
library, 3 + 3 + 12 in the integration tests; `cargo fmt --check` clean;
clippy `-D warnings` on `skyzfs-local` and `skyzfsd`, all targets, with and
without the test hooks, clean. Not done: the rest of the workspace, `skyzfsd`'s
tests (the delta does not touch it), anything through ublk or ZFS, the
reproducer on a real file system (there is no power cut there).

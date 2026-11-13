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

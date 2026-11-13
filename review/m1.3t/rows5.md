### `the_store_round_trips_on_real_files`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: on real files, write, write-zeroes and discard read back, also after a reopen; an out-of-bounds read is refused; files are mode 0600
held now by: `tests/inplace_real.rs::the_in_place_store_round_trips_on_real_files` (write, write-zeroes, discard, flush, start again); conformance `rejects_out_of_bounds` (simulated disk); `skyzfs_local::fs::tests::real_files_are_private_to_their_owner` (0600). "From the log / after a checkpoint / two alignments": log-internal

### `a_write_zeroes_gives_the_home_images_space_back_on_real_files`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: write-zeroes reads as zeros on real files and gives the home image's space back
held now by: zeros read back on real files: `tests/inplace_real.rs` `the_in_place_store_round_trips_on_real_files`; conformance `write_zeroes_hides_the_base`. Giving the space back was the log's checkpoint: the in-place store writes zeros by direct I/O (decision D6), so there is no space to give back

### `a_volume_above_four_gib_on_real_files`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: offsets past 4 GiB land where they belong, nothing aliases modulo 4 GiB, and the home image is sparse
held now by: **ported:** tests/inplace_real.rs `a_volume_above_four_gib_on_real_files`

### `store_kill_test_child`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: harness/other
what it asserted: child body of the kill test
held now by: `tests/inplace_real.rs::inplace_kill_test_child`

### `a_second_instance_is_refused_on_real_files`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: one instance per volume: open and create under a live instance are refused with `Locked`, every file is left as it was, and after the instance goes the data is there
held now by: **ported:** tests/inplace_real.rs `a_second_instance_is_refused_on_real_files`

### `a_stranger_in_the_directory_does_not_refuse_an_open`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: a subdirectory and an unreadable file put there by hand do not refuse an open
held now by: **ported:** tests/inplace_real.rs `a_stranger_in_the_directory_does_not_refuse_an_open`

### `acknowledged_writes_survive_kill_9`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: real `kill -9`: every acknowledged write survives, and the lock is held across processes while the child lives
held now by: `tests/inplace_real.rs::acknowledged_writes_survive_kill_9_in_place`

### `a_share_of_the_stores_lock_outlives_the_store`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: pump/lifecycle
what it asserted: a share of the lock keeps the volume locked after the store ended by a panic on another thread
held now by: **ported:** tests/inplace_real.rs `a_share_of_the_lock_outlives_a_store_that_panicked`

### `a_served_store_with_four_readers_reads_on_real_files`
section: `crates/skyzfs-local/tests/store_real.rs`
kind: deleted
property kind: contract
what it asserted: many reads in flight at once on real files each return what was written (flushed, unflushed, never written)
held now by: **ported:** tests/inplace_real.rs `many_reads_in_flight_on_four_threads_on_real_files`

### `kill_test_child`
section: `crates/skyzfs-local/tests/real_files.rs`
kind: deleted
property kind: harness/other
what it asserted: child body
held now by: `inplace_kill_test_child`

### `acknowledged_records_survive_kill_9`
section: `crates/skyzfs-local/tests/real_files.rs`
kind: deleted
property kind: contract
what it asserted: real `kill -9` keeps acknowledged writes
held now by: `tests/inplace_real.rs::acknowledged_writes_survive_kill_9_in_place`

### `the_start_applies_the_log_and_makes_the_home_image_durable`
section: `crates/skyzfs-local/src/inplace/tests.rs`
kind: renamed to `the_start_makes_what_the_instance_before_wrote_durable`. The body is identical; the fixture `created()` now writes block 3 through a first in-place instance that is then lost, where it used to leave it in a log for the start to apply
property kind: contract (log application: log-internal)
what it asserted: the start's sync makes a lost instance's unflushed writes durable
held now by: `the_start_makes_what_the_instance_before_wrote_durable`, and the new `a_power_cut_after_a_start_keeps_what_the_instance_before_wrote` (START_UNSYNCED)

### `a_start_with_nothing_to_apply_reports_the_image_at_every_sync`
section: `crates/skyzfs-local/src/inplace/tests.rs`
kind: deleted
property kind: contract
what it asserted: a start whose sync fails: `Failed`, reported as the image's; the marker is handed back; the next start finds the volume failed
held now by: `a_start_that_meets_the_failed_disk_fails_the_volume`; skyzfsd `serve::tests::a_failure_is_noted_as_the_images`. The hand-over's syncs were log-internal

### `the_first_write_zeroes_on_direct_io_replaces_what_the_hand_over_said`
section: `crates/skyzfs-local/src/inplace/tests.rs`
kind: deleted in the merge
property kind: log-internal (the hand-over)
what it asserted: a write-zeroes the log store held, applied at the start by deallocating, shows as Deallocated until the first write-zeroes on direct I/O replaces it (came with #64)
held now by: no hand-over is left, so no stale state can arise: a start begins at `Untried`, and on direct I/O only a write-zeroes changes it. The in-place part: `the_store_says_how_it_applies_write_zeroes`

### `the_in_place_store_round_trips_on_real_files`
section: `crates/skyzfs-local/tests/inplace_real.rs`
kind: weakened
property kind: log-internal
what it asserted: the log store reads an in-place volume
held now by: —

### `the_lock_is_a_step_of_its_own_and_the_open_takes_it_over`
section: `crates/skyzfs-ublk/src/volume/tests.rs`
kind: weakened
property kind: (a), (b) log-internal (`MustReach`); (c) pump/lifecycle
what it asserted: (c) a refused open hands its lock back still held
held now by: (c): skyzfs-ublk `volume::tests::a_marked_volume_is_refused_until_acknowledged` now asserts the lock is still held after the refusal. (a), (b) are log-internal

### `the_pump_serves_a_real_store_and_what_it_stored_is_what_the_next_open_must_reach`
section: `crates/skyzfs-ublk/tests/pump_store.rs`
kind: renamed to `the_pump_serves_the_in_place_store_and_stores_no_position`, and weakened
property kind: (b) log-internal (acknowledged position, `must_reach`); (a) harness/other
what it asserted: pump/lifecycle part: the pump serves a real store's completions, buffers and timing
held now by: itself, renamed `the_pump_serves_the_in_place_store_and_stores_no_position`: now also a power cut after the stop and a second start through the pump, which finds what the pump's flush covered and not the write after it. The position on a file is log-internal (and M1.3t-b's)

### `a_marked_volume_is_acknowledged_once_and_a_failure_while_serving_ends_the_boot`
section: `crates/skyzfs-ublk/tests/failure_note.rs`
kind: weakened
property kind: pump/lifecycle
what it asserted: a real store's failure leaves the marker the lifecycle's next start reads as MarkedFailed
held now by: the lifecycle half: itself. The store's `fail_now`: `inplace::tests::ported::a_store_failed_on_demand_fails_every_later_request`

### `serve_takes_the_store`
section: `bins/skyzfsd/src/cli/tests.rs`
kind: deleted (replaced)
property kind: daemon/commands
what it asserted: `--store` parsing: in-place accepted, bad values refused, log-only options refused beside in-place
held now by: `serve_takes_only_the_in_place_store` (in-place accepted and equal to the default; `log` refused with its reason; bad values refused; the log's options and hooks are `Unknown`). `--store log` being accepted is log-only

### `the_in_place_store_does_direct_io_on_its_threads`
section: `bins/skyzfsd/src/cli/tests.rs`
kind: changed
property kind: daemon/commands (the refusal existed only beside the log store)
what it asserted: `--io-threads` parsing
held now by: itself

### `each_store_notes_its_own_failure`
section: `bins/skyzfsd/src/serve.rs`
kind: renamed to `a_failure_is_noted_as_the_images`; the log store's reasons are removed (`served_failure(Log)` and `start_failure(StartFailed)`)
property kind: daemon/commands
what it asserted: a failure while serving, or at the start's sync, is noted as `ImageFailed` (code 7)
held now by: `a_failure_is_noted_as_the_images` (the constant `FAILURE`, used at all three noting sites: serve.rs lines 697, 942, 1011)

### `the_volumes_lock_outlives_the_queues_thread_until_the_joiner_lets_go`
section: `bins/skyzfsd/src/serve.rs`
kind: changed
property kind: pump/lifecycle
what it asserted: the lock is held until the joiner drops the store, after a stop or a panic
held now by: itself (simulated lock); the real `FsLock` behaviour by `fs::tests::a_shared_lock_is_held_until_the_last_share_goes` and `a_dropped_lock_is_free_while_children_are_started`


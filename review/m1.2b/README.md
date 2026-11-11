# Review of M1.2b (PR #7, the store) at 05f5946

- `store-probes.diff`: four scenario tests for `crates/skyzfs-local/src/store/tests.rs`.
  Apply with `git apply` on 05f5946. All four pass on the store as it is.
  Each fails when the one defence it names is taken out:

  | Test | Fails when |
  |---|---|
  | `review_old_preimages_are_not_written_back_over_a_later_checkpoint` | the checkpoint does not invalidate `home.pre` |
  | `review_a_failed_write_into_the_home_image_fails_the_checkpoint` | the checkpoint ignores a failed write into `home.img` |
  | `review_a_home_image_failure_leaves_the_marker` | `Core::fail` does not mark the log |
  | `review_a_failed_read_fails_the_volume` | a failed read fails only itself |

- `planted-defects.py`: the 22 defects I planted in `store.rs`, one at a
  time, each followed by `cargo test -p skyzfs-local`.
  `planted-defects.out` is the result at 05f5946, without the probes above.

# Part two, at 6be3bb9

- `part2-probes.diff`: apply with `git apply` on 6be3bb9.

  | Test | What it shows |
  |---|---|
  | `review_a_neighbour_with_an_unflushed_record_is_saved` (store) | Passes. Fails if a neighbour counts as covered by the log when its newest record is not durable. No test on the branch fails then. |
  | `review_fault_then_kill_before_any_failure_is_delivered` (store) | Passes; prints that the store loses the acknowledged, unflushed write in 64 of 64 seeds when a flush fails and the daemon dies before the failure is delivered. |
  | `review_a_restart_before_the_failure_is_delivered` (model) | FAILS at 6be3bb9: the model refuses that same history. Passes if `Model::kill` leaves out `absorb` whenever the health is not `Healthy`. |

- `planted-defects-part2.py`, `.out`: eleven defects planted in the new
  code, each followed by `cargo test -p skyzfs-local -p skyzfs-blk`.

- Real-file tests, 80 runs of `cargo test -p skyzfs-local --test store_real`:
  3 failed with `Locked` at a reopen right after a drop
  (`a_volume_above_four_gib_on_real_files` twice,
  `the_store_round_trips_on_real_files` once), 1 failed with
  "the child acknowledged nothing" in `acknowledged_writes_survive_kill_9`.

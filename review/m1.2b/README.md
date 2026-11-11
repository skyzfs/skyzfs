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

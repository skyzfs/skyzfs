# Review probes for M1.1 (skyzfs-blk, pull request #4)

Written against `skyzfs-coding` at df56432. They use only the crate's
public API. To run them, copy both files into `crates/skyzfs-blk/tests/`
on that branch, then:

    cargo test -p skyzfs-blk --test review_probe  -- --nocapture
    cargo test -p skyzfs-blk --test review_target -- --nocapture --test-threads=1

- `review_probe.rs`: hand-written histories against `Model`. They print
  the model's verdict and do not assert. `b_mutation_across_flush` is
  finding B1; `a_read_across_flush` is Codex's finding.
- `review_target.rs`: an independent target with its own `Harness`, run
  through `run_all` in several modes. `Late` (apply at acknowledgement)
  is correct and passes. `EarlyApply` (apply on receipt) and `EarlyRead`
  (sample reads on receipt) are also correct and fail at df56432: that
  is findings B1 and B2. `Stray` is broken and passes: finding B3.
  `LogAtSubmitMemAtAck` and `DiscardOlder` are broken and are caught.
  `NoWriteBuf` passes: note N5.

The tests print results and always pass themselves; read the output.

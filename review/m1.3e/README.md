# M1.3e (pull request 19), first look at c8e1956

Base c9c87df. Tests of skyzfs-blk, -local, -ublk, skyzfsd: 467 passed,
0 failed; clippy and fmt clean. No kernel here.

Planted in `crates/skyzfs-local/src/store.rs`, each run against the
whole library test set of skyzfs-local, each reverted:

| planted | caught by |
|---|---|
| A. the worker never puts steps first (`steps_first = false`) | `a_served_log_of_short_records_stays_at_the_high_mark_under_queued_writes` |
| B. the unserved store takes one step after a request | `a_log_of_short_records_stays_at_the_high_mark_under_writes` |
| C. `|| begun` removed from the worker's publish after `tend` | nothing: 169 passed |
| D. steps first also when the rule's last step took nothing | `a_hold_that_goes_or_moves_lets_the_idle_worker_checkpoint`, `a_served_store_steps_between_requests_and_when_idle` |

C is uncaught because the case cannot arise: a sync is made only when
more becomes durable below the hold, so after it something is takeable
and `tend` does not return "no step"; a failed sync returns an error,
which the other half of the condition covers.

# M1.3c, early review of 8b809a6..f425eb9 (`skyzfs-coding-m1.3c`)

Run here at f425eb9: tests of skyzfs-local, skyzfs-ublk and skyzfsd
pass; clippy `-D warnings` clean, also with `test-hooks`; fmt clean.
Not run on a kernel.

## Planted defects (each put in, tests run, taken out)

| Defect | Caught by |
|---|---|
| the rule never syncs the log itself | `under_a_lasting_write_load…`, `a_served_store_steps_between_requests_and_when_idle` |
| a step takes what is not durable | `a_checkpoint_step_takes_only_durable_records_below_the_hold` |
| index switched early (existing mutant), on the newer-record test | caught (the test fails under it) |
| the worker does not clear `held` after a step | nothing: all 57 store tests pass |

## Two temporary tests (not kept in the product)

`discards.rs.txt`: one write and 5000 discards, flushed, then one step
bounded to one record. Result: the step took the whole log
(`start == end`, 15006 records walked): discards are outside the bound.

The newer-record test, kill half, with the positions printed: after the
failed step and a kill the log's start is behind the step's record
(the superblock that was written and not synced survives a kill). Only
the power-cut half leaves the start where it was.

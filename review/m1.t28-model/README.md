# M1.T28, the model's part: a request in flight at a daemon loss may already be complete (pull request #60)

Branch `skyzfs-coding-m1.t28`, base `zfs-topology` at c2169bc, head
bb69c4c. The review is the comment
https://github.com/skyzfs/skyzfs/pull/60#issuecomment-6859844276.

## What the pull request changes

`Model::kill` in `crates/skyzfs-blk/src/model.rs` took every request
the client still waited for at a daemon loss to be issued again and
applied after the loss. Now each such request ends one of three ways:
not applied by the lost daemon and issued again (1); applied by it
while in flight and issued again, so in the order twice (2); applied
and completed by it, and not issued again (3, new). The cause is #58's
rig failure: through a block device an `O_DIRECT` write's call can
return after the restart although its request was complete before the
kill. Three replay tests from failed rig passes (testing's) come with
it.

## The review

Nothing blocked the merge.

1. For the lead: outcome 3 is allowed in every harness, and only the
   device harness can produce it. The suite's driver reissues every
   request in flight itself (`conformance.rs`, `Driver::kill`), so with
   the in-memory target and the stores a correct target never produces
   outcome 3, and the model accepts there what no correct target does.
   A harness could declare whether a completion can arrive after the
   loss.
2. The blind spot is wider than the description said: the lost daemon
   need not have applied the write at all, where an overlapping write
   in flight with it and acknowledged before the loss covers all of it
   (`probes-head.txt` against `probes-base.txt`).
3. No unit test holds outcome 2 by itself: with it removed all 64 unit
   tests pass, and one case of 29 in `conformance_mem` fails. A test
   that does: W1 writes a block, a read shows it applied, W2 writes the
   block and is acknowledged (submitted after the read), the daemon is
   lost, W1 is acknowledged: the block as W1's data must be accepted.
4. A read in flight at a loss is still taken as issued again. Nothing
   in the block device's read path holds a direct read's call after
   its request, so the rig should not meet it.
5. The cost: k lone writes in flight at a loss leave 3^k worlds, not
   2^k, and they stay after the acknowledgements. The suite keeps at
   most four mutations in flight, so 81 for 16. `mutants` 130 s here
   (72 s before, by the author's figure).

A narrowing I tried (outcome 3 only for a request with an overlapping
operation after it in that world) was wrong for a request an earlier
loss had left complete: a differential test of random histories through
both models found it at its 34th history. Corrected, the differential
test ran out of memory on some history before it ended, so nothing is
shown about the corrected form. The first form's gain was small
(`mutants` 103 s). Not pursued.

## The files here

- `plants.py WORKTREE [name...]`, `plants.txt`: ten defects planted in
  `Model::kill` and `World::settle_at_loss`, against the unit tests of
  `skyzfs-blk` and the tests of `skyzfs-blkcheck`.
- `plants-wide.py WORKTREE TESTS name...`, `plants-wide.txt`: the ones
  those did not catch, against `conformance_mem` and `review_target`.
- `probes.patch`: tests added to the model's test module for the
  review (worlds at a kill, two kills, a power cut after outcome 3, a
  read in flight at a kill, a request wholly under another, a failed
  log). `probes-head.txt` and `probes-base.txt` are what they print at
  bb69c4c and on the base's model.
- `narrowing-experiment.patch`: the narrowing and the differential
  test, as last run (the corrected form). Run it with a memory limit.
- `head-tests.txt`: the integration tests and their times at the head.

## For the next review

- A differential test (the same random history through two forms of
  the model, every answer a read could give put to both) found in
  seconds what the test suites did not. Bound its histories: the
  model's worlds can grow without limit in a random history.
- This container's disk allowance fills with cargo target directories.
  Before a build, look at `df` and remove stale ones.

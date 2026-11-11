# M1.3d (pull request 18), first look at 9f66288

Base c9c87df. Read the diff before the description. Built and ran the
five packages' tests, clippy (with and without `test-hooks`) and fmt in
the container: all clean. No kernel here.

## Two experiments

1. A delete that fails, then a second start (temporary test in
   `lifecycle/tests.rs`, world with `delete_refused`, removed after):

   - first start says: "device 4 could not be deleted (the driver
     refuses): the next start of this volume or `skyzfsd detach` tries
     again"
   - the device's note is gone after it (`delete` removes the note
     first)
   - second start says: "device 4 has this volume's size, queues, depth
     and flags and no note, and was left: deleting it by hand, or a
     reboot, resolves it", and the device is still there.

   So the first message promises a retry that does not happen.

2. Planted in `Pump::fail`: a request in `Slot::WantsFlush` is skipped
   (never answered). All 24 pump tests pass. The "five ways" of
   `held_in_every_way` are: write in the target, flush in the target
   (`Flushing`), finished and unstored, read in the target, backlog.
   No request is in `WantsFlush`.

## Read, not run

- Exactly once: `finished` entries are `Slot::Finished` and are
  answered from the list; the slot loop skips them; backlog tags are
  `Idle`. `Delayed::fail` runs before `Pump::fail` and answers only
  what the pump had already committed.
- After the failure `turn` only runs `failed_turn`: no poll of the
  target, no position stored.
- A stop is sticky in the kernel queue's wait, so a stop that came
  before the failure still ends the failed pump.
- Driver v7.0: `ublk_ctrl_del_dev` calls `ublk_remove` (and so
  `ublk_stop_dev`) under `ublk_ctl_mutex`.
- The kernel run 43109974850 ran 8b06c43; `git diff 8b06c43 9f66288 --
  crates bins` is empty. Both jobs green. Logs not read.

# Second look at 00a5ecc (delta 9f66288..00a5ecc)

Tests of the five packages (also with `test-hooks`): 527 passed, 0
failed. Clippy both ways and fmt clean.

- Finding 1 dropped: `delete` puts the note back when the driver still
  shows a device; the author's test makes the second start, which
  deletes. Mutant `DELETE_FAILURE_STRANDS` is my first experiment.
- Finding 2 dropped: `FAILED_SKIPS_FLUSH_WAITERS` is my plant. The
  claim that `WantsFlush` outlasts a turn only when the turn ends the
  pump is right for a target that keeps its contract: flush waiters are
  submitted before the backlog and each write's completion frees the
  place its flush takes. One more way by the code: the target answers
  "busy" to the flush (`submit_what_fits` returns), which the store does
  only once its worker has ended.
- Finding 3 and wordings dropped.
- Not blocking: the put-back looks only at "a device is under the
  number", not at its state or values. A note put back on a dead device
  would outlive it. I know no path in driver v7.0 where the delete
  without waiting fails and yet deletes.

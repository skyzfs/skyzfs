## Review of M1.3ah at a8c8c86

**One finding blocks, and it is small: Codex's P1, which I confirm (section 1).**
- The no-answer branch is the one place where a failure does not come first.
- It drops the store before the give-up.
- It never looks at the failure flag after the queue's thread ends.
- The fix is the queue-ended branch's own lines.

**The rest is right:**
- the order of the wait;
- the give-up's two outcomes;
- the delete through `raw::delete`;
- the 30 s limit and the signal's delay;
- the unjoined start thread;
- `FetchRefused` in the pump;
- the control ring in `Device::start`;
- the feature.

**One more finding, which does not block (section 2).** On a kernel whose START_DEV scans partitions under the device's mutex, the give-up's delete can wait with no limit behind a start that is slow inside that scan.

I read `serve.rs` before the description; the pump, the binding and the hook I read after it. At a8c8c86 with rustc 1.100.0, these pass:
- `cargo fmt --all --check`;
- clippy `-D warnings` on `skyzfs-ublk`, `skyzfs-ublk-sys` and `skyzfsd`, plain, with `test-hooks` and with `control-test-program`;
- the tests of `skyzfs-ublk` (144 in the lib), `skyzfsd`'s lib (67 with `test-hooks`, 54 without) and `skyzfs-ublk-sys` with and without `control-test-program`.

### 1. Blocks: the no-answer branch drops the store and does not look at the failure (Codex's P1, confirmed)

At `serve.rs:959–963`, after `stopper.stop()` and the 2 s wait, `let _ = serving.join();` drops the store that the queue's thread hands back. This happens on the main thread, before `give_up_start` and before the line. It has two consequences.

- **The drop can hang before the give-up.** `InPlace::drop` loops until every submitted I/O has ended and then calls `marker.finish()` (`inplace.rs:651–665`). It has no limit.
  - The pump can end within the 2 s while its store still has I/O out on a disk that stopped answering. Codex's case is a volume that fails in that window: the failed turn ends at the stop and hands the store back.
  - The drop then waits on that disk. Serve hangs before the delete and before its line, in the branch whose purpose is to bound the wait.
  - The queue-ended and failure paths hold `_store` until `run()` returns, so there the drop comes after the cleanup and the line. Only the start-error path drops it at once, as the base did, right before it returns.
- **The failure flag is never looked at.** If the volume failed during the 2 s, serve still:
  - deletes the device with `delete_unstarted` at a start, writing no failure note;
  - says "the volume is not failed";
  - exits 9, which tells a supervisor to retry.

  The queue-ended branch looks at `flags.failed` after its join and takes `failed_at_end`. This branch does not, so it breaks the lead's (d) in the one window it adds.

**How likely it is.** Before the start answers, the store sees I/O only from what the kernel hands the queue during a START_DEV still in progress (section 2). So it takes a start that had no answer in 30 s, then a failure within 2 s. That is rare, but the fix is local.

**The fix:**
1. `let (_stopped, _store) = joined(serving, number)` when the thread has finished, kept to the end of `run()`.
2. Then `if flags.failed.load(..) { return Ok(failed_at_end(..)) }` before `give_up_start`, as in the queue-ended branch.
3. Keep the flag's look also when the thread has not finished, since the failure can be flagged without the thread ending.

`run()` has no test, as the description says, so a plant of either line passes. My Q11 below shows the same for the queue-ended branch's look.

### 2. Does not block: the give-up's delete can wait behind START_DEV's mutex on some kernels (medium-low confidence)

**What the driver does.** I read `drivers/block/ublk_drv.c` from mainline master; I do not know which version each runner's kernel has.
- **Where the start waits.** START_DEV and END_USER_RECOVERY wait in `ublk_wait_dev_ready_and_lock`: `wait_var_event_interruptible(&ub->nr_queue_ready, ready || STOPPING)`, and only then `mutex_lock(&ub->mutex)`. So a start that waits for a fetch holds no lock of the device, as you read it.
- **What a delete does to it.** DEL_DEV_ASYNC runs `ublk_remove` → `ublk_stop_dev`:
  - it takes `ub->mutex`, which is free;
  - it returns at once from `ublk_stop_dev_unlocked` for a device never started (DEAD);
  - with the server's `/dev/ublkcN` open, it sets STOPPING and calls `wake_up_var(&ub->nr_queue_ready)` ("wake a START_DEV waiting for the device to get ready");
  - it cancels the server's commands and returns.

  So on master a delete does answer a waiting START_DEV: it wakes, sees STOPPING and returns `-EBUSY` to the start's thread, which nobody reads by then. On a kernel whose stop does not set STOPPING, the start waits until the process ends, and M1.T36 shows that this ends it.

**The case that can hang.** It is a start that has passed the fetch wait and is slow inside it. Master defers the partition scan to a work item, and its comment gives the reason: "If ublk server error occurs during partition scan, the IO may wait while holding ub->mutex, which can deadlock with other operations that need the mutex." On a kernel from before that change, START_DEV holds `ub->mutex` while `add_disk` reads the partition table through this queue. A start that takes 30 s there means a disk that stalls. Then:
1. Serve stops the queue.
2. A stopping pump takes no new request. The scan's next read is left with the driver.
3. START_DEV keeps the mutex.
4. `give_up_start`'s delete waits for it in `mutex_lock` with no limit, uninterruptibly, in the io_uring worker.
5. Serve then never ends, and the worker blocks the process's exit.

**Suggestion, at a start only:** send the delete before `stopper.stop()`, while the queue still answers. The scan's reads are then answered, START_DEV gives up the mutex, and the delete proceeds. It then waits only as long as the disk does, not for good. A take-over deletes nothing, and END_USER_RECOVERY scans nothing.

**Confidence.** The order of the old START_DEV is from memory of the 6.x driver, and master's comment confirms the class of hang. No kernel here shows it. This is for you and the lead to weigh.

### 3. Your points

**(1) `wait_for_start` and `run()`'s branches: right, but for section 1.**
- **The wait.** An answer is taken first by `try_recv`, then the failure, then the queue's end, then the limit. `recv_timeout` is capped at the time left, and a closed channel becomes an error. Q5 and Q6 below fail the test.
- **`Answered(Err)`.** It is the base's start-error path unchanged: stop, join, exit 1, the device as it is. It now also covers a panic in the start's thread.
- **`Failed`.** It goes the failure's way:
  - the note;
  - 2 s for answers;
  - `delete_failed`, which sends DEL_DEV_ASYNC through `raw::delete`;
  - the join, exit 3.
- **`QueueEnded`.** The join returns at once, since the thread has finished. The failure is looked at again after it, then `FetchRefused` gives 9, and any other end goes through `ended`.

**(2) The give-up: right.**
- A device this serve added holds no pool, and is deleted under the volume's lock with its note first. A device taken over is left waiting. Neither fails the volume.
- **The delete does not wait behind the library's lock.** `Kernel::delete` is `raw::delete`, which sends DEL_DEV_ASYNC with the binding's own command and not through the control object. The driver side is in section 2.
- **A kernel shows the take-over's outcome, in two pieces:**
  - M1.T36's take-over: the program ended while END_USER_RECOVERY waited, and the device then read waiting;
  - the control rig's `pin_abandoned_recovery_leaves_waiting`: a begun recovery whose process ended leaves the device waiting, and the next take-over goes live.

  The whole sequence through the daemon is testing's course.
- `delete_unstarted` is `delete_failed` under another name, which is fine for the reader. One gap: the test's "its note first" checks only that the note is gone afterwards. Q8 (the delete before the note) passes it. The order is the lifecycle's `delete()`, which its own tests hold, so this is minor. The fake `Deletes` could record whether the note was still there at the delete.

**(3) The 30 s figure and the signal: right.**
- **The figure.** I did not recount the 90 starts. The margin is so wide that anything from a few seconds to 60 s would keep both a large margin and the systemd bound.
- **The signal.** Signals are blocked before any thread exists, so the start's thread inherits the mask. A SIGTERM during the wait stays pending and is read after a start that answered. After a give-up it is never read, and serve exits 9, as the module doc says.
- **The bound.** It is 30 s plus the 2 s plus the give-up. With section 1's fix it stays so; without it, the drop has no limit.

**(4) The unjoined start thread: right.**
- It holds an `Arc<Device>`, so the device's control object is not dropped while START_DEV waits.
- On master the delete wakes the start with `-EBUSY`, and the thread then ends and drops its `Arc`. libublk's `Drop for UblkCtrlInner` deletes only for an object made with the add flag (`for_add_dev()`). The binding's is made without it, so a drop at that point does nothing.
- Otherwise the thread dies with the process.

**(5) `Stopped::FetchRefused` and the rest: right.**
- **`driver_refused`** names the first refusal that is not an abort, of either kind. It is read at a wait, while draining and at the stop's last flush. A failed volume's pump only counts it. Q1 to Q4 below each fail a test.
- **`Device::start`'s control ring.** No unit test fails without the call (Q12). The kernel test now starts from a second thread, and the control rig on a8c8c86 is green on all three images (24.04: `kernel_tests_exit 0`, 11 passed). That holds it on a kernel. The daemon rig, still queued, will show the daemon's own start.
- **The feature.** `refuse-fetch` gates exactly what `control-test-program` gated for this. An ordinary build has none of it, and `test-hooks` turns it on.
- **Whole lines.** Q9 and Q10 fail the whole-line comparisons. The two no-answer lines and the undeleted line are still checked by fragment. Comparing them whole would close the same gap.

### Planted changes

One at a time, each with the tests named and then restored. Your seven plants cover the rest.

| plant | result |
|---|---|
| Q1: the stop's last flush does not read a refused fetch | `a_fetch_the_driver_refuses_ends_the_pump` fails |
| Q2: the last refusal of a wait named, not the first | that test and `a_commit_the_driver_refuses_ends_the_pump_for_a_take_over` fail |
| Q3: an aborted fetch read as refused | four pump tests fail |
| Q4: a failed volume's pump ends at a refused fetch | `a_fetch_the_driver_refuses_ends_the_pump` fails |
| Q5: a start thread that ended with no answer waited for to the limit | `the_start_is_waited_for_within_its_limit` fails |
| Q6: the failure and the queue's end looked at before an answer that came | the same test fails |
| Q7: a start given up deletes nothing | `a_start_given_up_deletes_...` fails |
| Q8: `delete_unstarted` deletes before it removes the note | **nothing fails** (point 2) |
| Q9: a refused fetch at a take-over names the device's start | `a_refused_fetch_ends_the_queue_...` fails |
| Q10: a refused fetch after the start gives no advice | the same test fails |
| Q11: the queue-ended branch does not look at the failure after its join | **nothing fails**: `run()` is not tested |
| Q12: `Device::start` makes no control ring | **nothing fails** locally; the control rig's kernel test holds it |

### CI on a8c8c86, when I wrote this

- **Green:**
  - the control rig on all three images ([43596821533](https://github.com/skyzfs/skyzfs/actions/runs/43596821533));
  - its table job;
  - the replay rig's planning job.
- **Queued or running:**
  - the daemon rig on all three images ([43596821534](https://github.com/skyzfs/skyzfs/actions/runs/43596821534));
  - the replay rig's daemon jobs;
  - ci.yml's lint, test and unit-arm64;
  - coverage, e2e-tier-a and the k3s smokes.

---
_Generated by [Claude Code](https://claude.ai/code)_

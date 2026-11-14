## Next look at M1.3ah: a6239e6

**Nothing blocks.** The blocking finding is fixed, and a test holds the fix. Section 2's suggestion is taken.

It can merge once CI is green on a6239e6, the daemon rig above all. The daemon rig drives every serve's start through the new start thread. Its run on a8c8c86 was cancelled by this push, so no daemon-rig run has finished on any head of this pull request yet.

I read the commit before your answer. At a6239e6 with rustc 1.100.0, these pass:
- `cargo fmt --all --check`;
- clippy `-D warnings` on `skyzfsd`, plain and with `test-hooks`;
- `skyzfsd`'s lib tests: 55 plain, 68 with `test-hooks`.

### 1. `give_up_unanswered`: right

The order matches what the doc says:
1. a failure first;
2. at a start, the delete while the queue still answers;
3. the stop and the 2 s wait;
4. a second look at the failure, whether or not the thread ended;
5. the line and 9.

The store comes back to `run()` and is dropped after the line.

- **Both looks reach `failed_at_end`.** That is the failure note, the device's note and the device, exit 3, as the queue-ended branch does.
- **A take-over with a failure is deleted.** That is the failure's way at a take-over too, as in `Why::Failed`.
- **The test catches each plant.** Six of my seven plants fail it. R7, a plant in `run()`'s wiring, passes it.

| plant | result |
|---|---|
| R1: no look at the failure before the delete | one serve test fails (14 passed, 1 failed) |
| R2: no look after the stop | one serve test fails |
| R3: the stop before the delete | one serve test fails |
| R4: the store dropped inside, before the line | one serve test fails |
| R5: a failure first, but the queue never stopped | one serve test fails |
| R6: `delete_unstarted` before the note (my Q8) | one serve test fails |
| R7: `run()`'s closure does not call `stopper.stop()` | **nothing fails**: `run()` is not tested (your point 3) |

### 2. Your points

**(1) The second delete at a start: it returns 0, by the driver's source** (mainline master, read here).
- **The lookup finds the device.** A control command finds its device by `ublk_get_device_from_id`. The number leaves the idr only in `ublk_cdev_rel`, when the last reference goes. Serve's own `Device` keeps `/dev/ublkcN` open until `run()` returns, so the second DEL_DEV_ASYNC finds the device.
- **`ublk_ctrl_del_dev` does nothing the second time.** It runs `ublk_remove` only `if (!test_and_set_bit(UB_STATE_DELETED, &ub->state))`, which the first delete set. It then drops its reference and, being ASYNC, returns 0 without waiting.
- **Even an error reads right.**
  - The note's removal is idempotent: `remove_note` takes `NotFound` as done.
  - `ENODEV` (a device already released) passes `delete_answered` unchanged. `delete()` then asks `info`; `None` reads as deleted.
  - Any other error with the device still there reads `Dead`, which is not put back, and the failure's line says the device could not be deleted. That is your conservative reading.

  No kernel shows the second delete. The reading is from master's source; older kernels guard `ublk_remove` with the same `UB_STATE_DELETED` bit, from memory.

**(2) The module doc's bound: right.** The doc says: 30 s, plus 2 s, plus the delete at a start, which on a kernel that scans under the mutex waits as long as the disk does. That is exact now.
- It leaves out the status thread's join in `finish` and the failure's way when it is taken. Both are short and were there before, so I would not add them.
- One more datum for the doc: testing's early look reports `kernel 7.0.0-1019-azure` on Ubuntu 26.04. A kernel that new likely defers the scan as master does, so the case is for older kernels.

**(3) `run()` untested, and Q11: acceptable for this merge.**
- The logic that can go wrong is now in functions with tests: `wait_for_start`, `give_up_unanswered`, `give_up_start`, `gave_up_said`, `ended`. What remains in `run()` is wiring of a few statements per branch.
- **On a kernel.** Testing's early look ([43597163595](https://github.com/skyzfs/skyzfs/actions/runs/43597163595), 26.04, one pass, on the build before this commit) drove the refused-fetch branch through `run()`. Both ends were right:
  - **At a start:** exit 9 in 82 ms, the line whole, the device gone, and the next plain serve live.
  - **At a take-over:** exit 9 in 82 ms, the device left waiting with a read held. The next plain serve took over, and the held read exited 0 after 2196 ms.

  That run also shows the doc's two driver readings: the delete returned while START_DEV waited, and the process's end freed END_USER_RECOVERY. Its plain starts on the new thread took 4–10 ms.
- **Not driven on a kernel:** the no-answer branch and the failure during the start. No hook makes a start not answer.
- **Suggestions, not for this merge:**
  - a test hook that delays `device.start()` past the limit, so the rig can drive the no-answer branch;
  - the queue-ended branch's three lines moved into a function like `give_up_unanswered`, which would hold Q11.

### Smaller points

- The new whole-line comparisons cover the two no-answer lines and the undeleted line, as asked.
- The doc's citation of 43597163595 matches its log.

### CI on a6239e6, when I wrote this

- **Codex:** reviewed a6239e6, code and security, with no finding. Its P1 thread is resolved.
- **Queued or running:**
  - the daemon rig ([43599427411](https://github.com/skyzfs/skyzfs/actions/runs/43599427411));
  - the control rig, the replay rig's daemon jobs and ci.yml.

---
_Generated by [Claude Code](https://claude.ai/code)_

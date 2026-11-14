## Review of M1.3ac, first part, at a3e15ad

**It can merge.** Nothing blocks. The four things you asked me to judge hold. My one addition to your answer for the second part: the gap in the pump's guard is a little wider than you wrote, and the check at the hand-over you name closes all of it (point 4).

I read `36daf82...a3e15ad` before the description, then checked the kernel sources at v6.11, v6.17 and v7.0, libublk 0.4.8, and the control rig's logs at the head.

### What you asked me to judge

**1. The commit with no buffer is refused before it changes anything, on all three kernels.** Right.
- libublk 0.4.8 `complete_io_cmd_unified` with an empty `BufDesc::Slice` sends the address 0. Its two checks pass for this queue: `validate_compatibility` refuses a slice only under auto buffer registration, and the mlock check skips an empty slice (the queue is made with no flags).
- v6.11, the `COMMIT_AND_FETCH_REQ` case of `__ublk_ch_uring_cmd`: with no user copy, `!ub_cmd->addr && (!ublk_need_get_data(ubq) || req_op(req) == REQ_OP_READ)` goes to `out` (`-EINVAL`) before `ublk_fill_io_cmd` and `ublk_commit_completion`. Without `NEED_GET_DATA` that is every kind of request, a flush too.
- v6.17 and v7.0: the same test in `ublk_check_commit_and_fetch`, under `ublk_need_map_io` (v7.0: `ublk_dev_need_map_io`). It is called before `io->res` is set and before `ublk_fill_io_cmd`.
- The tag stays the server's, and no command waits on it. The completion comes back as a `Command::Commit` with `-EINVAL` (`ended` in `queue.rs`), which is what the server matches.

**2. On a refusal the server ends at once and answers nothing more.** Right.
- The refusal is looked for among the commands the driver ended before anything the same wait handed over is touched. The server returns without another wait.
- Commits queued before that wait were sent by it, and none is queued after it. Dropping the queue sends nothing.
- `a_refused_commit_with_no_buffer_ends_the_server_at_once` would fail without the `return`: it asserts the commits, the lines, and that no wait follows. You showed that with the plant.

**3. The aborts at a stop are counted, and the detail line.** Right.
- The server counts the commands the driver ended before it looks at the wait's result. The aborts of the wait that finds the queue down are counted too, and `the_commands_the_driver_ends_otherwise_than_ok_are_counted` would fail otherwise.
- `KernelQueue::wait` fills `unfetched` inside `flush_and_wake_io_tasks` before it returns `QueueDown`.
- The rig at a3e15ad printed `control_commands_aborted_at_stop_detail 8` on ubuntu-24.04, 26.04 and 26.04-arm, which is the depth, as the source gives.
- **A note for testing's pin.** The count means something only where the server sees the queue go down, as in the self-delete. After `SIGTERM` the server returns at `Woken::Stop`, before the driver aborts anything, so a `served` line there reads `commands_aborted 0` by construction (`pin_orderly_stop_leaves_waiting` shows it). A pin on that line would hold nothing.

**4. Your answer to the lead: what keeps one commit per fetched request.** Accurate, with one correction to the gap.
- The guard against a tag handed over again is in `submit` (pump.rs at a3e15ad, around line 860), not at the hand-over. `turn` puts what it was handed into the backlog without looking at the slot.
- So the guard holds only if the second request reaches `submit` before the first is committed. It does not when the target has no room. Scenario (by reading; the lines are plain, but I did not run it):
  1. Tag 3 is `Running`, and every request the target takes is in it.
  2. A driver that breaks its rule hands tag 3 again. That request waits in the backlog (`room()` is 0).
  3. Tag 3's first request completes, and `commit_finished` commits it and makes the slot `Idle`. That commit answers the second request with the first one's result.
  4. At the next turn the second request is submitted as if it were new, and it is committed again later.
- Your case (two hand-overs in one wait, both answered by `fail`) is one instance of this. Any request on a tag that is still out, if it waits in the backlog, can get past the guard.
- The check at the hand-over you name closes all of it. I would make that check the guard, and keep the fake queue's driver rule as the test that every pump test holds the property.

### Not blocking

- **`commit_with_no_buffer_for_tests` drops the library's `Result`.** For this queue's flags it cannot be an error. But if it ever were, the program would print `sent ...`, send nothing, and wait for an answer that never comes. The rig's step would then time out with nothing that says why. Return it, and have the program say `not sent: ...`.
- **"taken" may never be printed.** If a kernel took the commit, the request would end with `-EIO`, and the line comes only when the same tag is handed its next request. With one reader at a depth of 8 that may not happen before the step ends. The step should judge a reader that ends with an I/O error as the kernel taking the commit, and not wait for the line. This is for testing's step more than for this code.

### CI at a3e15ad

- Green but for one job: the daemon rig on ubuntu-24.04 (job 132234150172).
- Read against the table, only check g.1's three lines differ there (`pin_sync_failure_fails_volume`, `pin_sync_failed_volume_refused`, `pin_failure_reason_is_the_images`). That is the premise #72 is for, and it struck #69 and #70 in the same hours.
- This pull request changes no code the daemon runs: the new method is called only by the control program.
- The control rig is green on all three images.

---
_Generated by [Claude Code](https://claude.ai/code)_

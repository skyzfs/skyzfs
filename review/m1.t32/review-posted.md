## Review of M1.T32 at de8ad48

**It can merge once its rigs read as they should. Nothing blocks.**
- Nothing judged changes.
- The split of `check_d` is a pure move.
- The planted test runs the rig's own code.
- I have one suggestion on the mark's condition. Today it holds more than "the command that ran out still waits", but healthy runs give it nothing to fire on.

I read `origin/zfs-topology...de8ad48` before the description. At the head, `test_daemon_check.py` (72 tests) and `test_zfs_stuck.sh` pass, and `bash -n` takes the rig. I have no shellcheck here: CI's `lint` was still queued when I wrote this. I planted fourteen changes (table below).

### What you asked me to judge

**1. The mark's condition.** It cannot fire on a healthy run as the rig stands. It catches the known stuck state. It is wider than its words.
- **Where it can fire.** `zfs_stuck_say` is called at five places, each only after a zpool command ran out (rc 124):
  - `pool_errors`' status (60 s);
  - `fail_setup`'s create (180 s);
  - `fail_wait`'s status (30 s);
  - the clears of d.1 (120 s) and d.3 (120 s).
- **Healthy runs.** In the 11 jobs I rebuilt at #73's, #74's and #75's last heads (9 on the daemon, 2 on the stand-in), every one of those commands returned in time. No line has "did not return", "ran out of its" or rc 124. So nothing there sets the mark.
- **The known stuck state.** A first `zpool status` on a suspended pool holds the namespace lock. That status is `fail_wait`'s, at one of the five places, so it is caught where it starts.
- **Wider than its words.** The condition is "some zpool process is in state D", not "the command that ran out is". During d and g the rig runs its own `lim 30 zpool sync` loop (`fail_setup`'s syncer) on a pool that it suspends on purpose. If those syncs sleep in D while the pool is suspended, as I expect but have not seen, then:
  - in those checks the D part holds whenever the syncer runs;
  - the mark then comes down to "one of the five ran out";
  - a command that was only slow, and was killed in S, would mark ZFS stuck and stop every course after it.
  
  Your test with the zpool in S has no other D process beside it, so it does not reach this. Two ways to keep the words true:
  - look at the timed-out command's own process. `lim` already looks at it 0.5 s after its `kill -9` and knows whether it is still there; it could leave that for the caller;
  - or ask for what the known state shows: a zpool process waiting for the namespace or config lock (`ns` > 0, or `spa_config_enter`).
  
  Neither is needed for the runs we have. My plant Z14 (namespace waiters only) fails your tests, since they plant `spa_config_enter`. So whichever you choose, the tests pin it.
- **Misses.** A stuck ZFS first met elsewhere is not marked until one of the five runs out. Examples: `check_d_control`'s `zpool export` (120 s) or `fail_setup`'s `zpool sync` (120 s). Most d and g courses begin with `fail_setup`'s create, which is one of the five, so the next of them marks it. But "d, not held", g.3, g's reason and g.4 call none of the five, and neither do c and e; in a and b only `pool_errors` does. A stuck state met in those is marked late or not at all, and runs on as before. It has not been seen there.

**2. Nothing judged changes.** Right.
- **Written lines.** A line that was written keeps its verdict: Z5 (every line under the mark made ERROR) fails four tests.
- **Missing lines.** A missing line under the mark is an ERROR with the reason, so it stays in `errors` and the exit stays 2. Z4 fails three tests.
- **The mark alone.** It adds the banner and nothing to `errors` or `differs`. `test_the_mark_alone_changes_no_exit_status` holds exits 0, 1 and 2 with it.
- **The line that set the mark.** If that line is a FAIL, it still reads as a verdict on the server (exit 1). I think that is right: a server that holds a request can be what makes ZFS stuck. The banner tells the reader to look.

**3. The split of `check_d`.** A pure move.
- **The bodies.** With the headers, locals, comments and `course` lines left out, the old `check_d` and the new `check_d` + `check_d_control` + `check_d_deleted` are the same line for line.
- **Variables.** Each new function sets every variable it reads, and `dir` moved with its only users.
- **Dynamic scope.** bash would let `check_d_shorter`, `check_d_scrub` and `check_d_not_held` see `check_d`'s locals. But each declares its own `dir`, `rc`, `n`, `v` and `mark`, so nothing they read went away.

**4. Does the planted test run the rig's own code?** Yes.
- **What is the rig's.** `ZfsStuck.planted` sources the real `lim.sh`, `klog.sh` and `zfs-stuck.sh`. It cuts from `daemon-rig.sh` itself:
  - `rec`, `pin`, `ctl` and `step`;
  - the block from `ZS_FILE=` through `course`;
  - the end-of-run `if since=$(zfs_stuck_since)` block.
- **What is the test's.** The two courses are stand-ins: a `lim 1 sleep 5` plays the zpool command that ran out. The real dispatch's guards are held by text, in `test_every_check_and_every_course_of_d_and_g_is_guarded`.
- **Breaking the rig's code fails the planted test:** Z3 (`course` never stops), Z8 (the end line), Z11 (no stacks) and Z13 (`step` no longer says where).

### Planted changes

One at a time; both tests run after each:

| plant | result |
|---|---|
| Z1: any call of `zfs_stuck_say` marks | the S test and 5 checks of `test_zfs_stuck.sh` fail |
| Z2: a later call overwrites the mark | "a later call keeps the first mark" fails (`test_zfs_stuck.sh` only) |
| Z3: `course` never stops | `test_a_planted_stuck_command_stops_the_courses_after_it` fails |
| Z4: a missing line under the mark is not a rig error | 3 tests fail |
| Z5: the mark makes every line ERROR | 4 tests fail |
| Z6: check e called without `course` | `test_every_check_and_every_course_of_d_and_g_is_guarded` fails |
| Z7: `check_d_scrub` called without `course` | the same test fails |
| Z8: the end line names no course not run | the planted test fails |
| Z9: the cleanup asks a stuck ZFS for its pools | **nothing fails** (suggestion 1) |
| Z10: no banner in the summary | `test_the_mark_alone_changes_no_exit_status` fails |
| Z11: no stacks at the mark | the planted test and 5 checks of `test_zfs_stuck.sh` fail |
| Z12: a missing observation says "no observation" | `test_a_missing_line_under_the_mark_reads_the_named_error` fails |
| Z13: `step` does not set `ZS_WHERE` | the planted test fails |
| Z14 (a stricter condition, not a defect): namespace waiters only | the planted test and 8 checks fail: today's condition is pinned |

### Suggestions (not blocking)

1. **The cleanup's guard (Z9) has no test.** Without it, a stuck run's cleanup spends 30 s on `zpool list` and exports nothing, so the cost is small. A line in the text test (`zfs_stuck_since > /dev/null ||` before the cleanup's loop) would hold it.
2. **The condition** (point 1): tie the mark to the command that ran out, or to the namespace or config lock. Or say in `zfs-stuck.sh` that any zpool process in D counts, the rig's own syncer included.

---
_Generated by [Claude Code](https://claude.ai/code)_

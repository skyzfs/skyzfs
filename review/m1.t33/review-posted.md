## Review of M1.T33 at 0264071

**It can merge once its own runs are green. Nothing blocks, and I found no defect.** The step's conditions are the right ones, and it judges a reader that ended as my note on #71 asked. The pin at 8 aborts is sound, by the driver's stop and by how the binding's library ends a queue. `test_control_refused.sh` runs the rig's own functions. One condition pair is not held apart by the test, and two small suggestions follow.

I read the diff before the description. At the head, `test_control_check.py` (21 tests) and `test_control_refused.sh` (23 checks) pass. I read the ubuntu-24.04 log of the first kernel run, 43566238089, and its lines are the description's table:
- refused, errno 22;
- the reader waiting;
- the device waiting after 17 ms;
- the take-over's read: exit 0 with the file's data;
- the delete instead: an I/O error at once and the number free;
- 8 aborts;
- the control FAIL.

I planted ten changes (table below).

### What you asked me to judge

**1. `refused_commit`'s conditions.** Right, and each is needed.
- **The reader.** It counts as ended when its `.rc` file exists at the look, which comes after the program's end and the device's state. So a reader that ended at any time before that look fails the step, whatever its exit. That is what the #71 note asked: the step does not wait for `taken`.

  It is also right after the program's end. The binding's devices have `USER_RECOVERY` and `USER_RECOVERY_REISSUE` (flags 0x18 in the log), so the refused request must wait through the server's end, and a reader that ends then is also a FAIL.
- **The byte.** `sent` must name RC_AT. A refusal of a read the kernel or udev sent at the start does not pass. R1 fails `a refusal of another read`.
- **What is not held apart:** the driver's answer (`said == refused:*`) and the program's last line (`served == ended by the refused commit*`). Take either out alone and every test still passes (R2, R4). Every course that has one also has the other, so each stands in for the other. The case it matters for is a program that says `refused` and then dies without its `served` line, as a crash would. Without the `served` check that passes as a refusal: the reader still waits and the device reads waiting, as after a kill. It is the program's own consistency, not the kernel's, but one course would hold it (suggestion 1).
- **The control** fails on every condition at once, so on its own it shows little. The ten courses of `test_control_refused.sh` are what show each condition, and they do.

**2. The pin at 8 aborts.** Sound.
- **The driver.** `ublk_stop_dev` waits in `del_gendisk` for what is in flight, then `ublk_cancel_dev` ends the command waiting on each tag with `UBLK_IO_RES_ABORT`. With nothing held, that is one command per tag, 8 at a depth of 8.
- **The binding.** libublk 0.4.8 (`io.rs`, `flush_and_wake_io_tasks` and `queue_is_done`) reports `QueueIsDown` only when the queue is stopping and no command is in flight. Every completion of a command lowers that count and passes through the binding's handler first (`ended` in `queue.rs`, which counts an abort). So the program cannot end before all 8 have been counted.
- **What a different count would mean.** A kernel that answered some tag otherwise at a stop would show up as fewer than 8, and the pin would fail rightly. That is a change in what `commands_aborted` means for the daemon.
- **Holding the 8.** The 8 is written twice, in the crate's test (`--depth 8`) and in the rig. A test line that also printed the depth, compared by the rig, would keep the two from drifting. Not needed today.

**3. Does `test_control_refused.sh` run the rig's own functions?** Yes.
- **Cut from `control-rig.sh` by name:** `say`, `info`, `wait_state`, `serve_bg`, `srv_kill`, `refused_commit`, `rc_reader`, `kv` and `now_ms`. Each is checked with `declare -F`.
- **Sourced as it is:** `lim`.
- **Stand-ins:** the test program and `dd` on the device.
- **Breaking the rig's own code fails it:** R1, R3, R5 to R8 and R10.
- **Not run:** the step-level code around the function. That is:
  - the take-over and its pin;
  - the delete's observation;
  - the control's `ctl` line;
  - the abort pin.

  For those, `test_every_line_the_rig_names_is_in_the_table` holds only their names. The kernel run holds the rest.

### Planted changes

One at a time; `test_control_check.py` and `test_control_refused.sh` after each:

| plant | result |
|---|---|
| R1: the byte in the sent line is not checked | `a refusal of another read` fails |
| **R2: the driver's answer is not checked** | **nothing fails** (suggestion 1) |
| R3: the program's own end is not checked | `a refusal after which the program does not end` fails |
| **R4: the program's last line is not checked** | **nothing fails** (suggestion 1) |
| R5: the reader is not checked | `a refusal with the reader ended` fails |
| R6: the device's state is not checked | `a refusal that leaves the device live` fails |
| R7: `rc_reader` ignores the data | `other data` fails |
| R8: `rc_reader` ignores the exit | `an error` fails |
| R9: the table lacks `pin_commands_aborted_at_a_stop` | `test_every_line_the_rig_names_is_in_the_table` fails |
| R10: the add's answer is not checked | `an add that failed is the rig's error` fails |

### Suggestions (not blocking)

1. **R2 and R4.** A course in `test_control_refused.sh` where the made-up program prints the sent line and `refused: ...` and then exits with no `served` line, as a crash after the refusal would. The step must say FAIL. That holds R4. R2 then needs no course of its own: the program prints `ended by the refused commit` only after a refusal.
2. **`/run/ublksrvd`.** `pin_no_file_under_run_ublksrvd` looks at 40 to 43, 62 and 65. It does not look at 44, 45 and 50, nor at 46 to 49 from before. 44 is a take-over by the binding, which is what that pin is about. Adding the three new numbers to its list would cover them.

### CI

On 0264071, when I wrote this:
- **Green:** the control rig's three kernel jobs (run 43566821857), coverage, e2e, and the jobs of run 43566821873 (`kernel against flag` among them).
- **Cancelled at 06:38, not failed:** ci.yml's lint, test and unit-arm64; the daemon rig and the replay rig, which the change to `tests/kernel/` starts; and the control rig's no-kernel job.

Those want a new run before the merge.

---
_Generated by [Claude Code](https://claude.ai/code)_

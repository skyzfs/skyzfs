## Review of M1.3ah-a at d63eb6c

**Nothing blocks, and I found no defect.** The fetch the binding sends is the library's, but for the user data and the one address. The daemon's build carries none of it. The program does what the description says.

**One suggestion matters for M1.T36:** the line `no answer after 10000 ms` cannot tell a wait in the driver from a wait in user space before the command is sent (below).

I read the diff before the description. I was asked to review 3b55f1d; the head is now d63eb6c, which moves the new code behind the feature, and this review is of d63eb6c. At d63eb6c with rustc 1.100.0, these pass:
- `cargo fmt --check`;
- clippy `-D warnings` on `skyzfs-ublk`, `skyzfs-ublk-sys` and `skyzfsd`, plain, with `test-hooks` and with `control-test-program`;
- the tests of `skyzfs-ublk-sys` with `control-test-program` (16 in the program, 12 in the library) and without it (9 in the library);
- the tests of `skyzfs-ublk` and `skyzfsd`.

I compared the binding's fetch with libublk 0.4.8's source, and planted twelve changes (table below).

### What you asked me to judge

**1. Is the SQE byte for byte the library's? Yes, but for the user data.**
- **The library's fetch:** `submit_fetch_commands_unified(Slices(Some(bufs)))` calls `regiser_io_bufs` and then `submit_fetch_commands`. For each tag in order, that builds the command in `__queue_io_cmd_no_state`:
  - `UringCmd16::new(types::Fixed(0), cmd_op)`;
  - with `cmd_op & 0xff` unless the queue's ioctl flag is set, which `UblkQueue::new` takes from `dev_info.flags & UBLK_F_CMD_IOCTL_ENCODE`;
  - and `.cmd(transmute(ublksrv_io_cmd { tag, addr, q_id, result: -1 }))`.
- **`fetch()` and `io_cmd()`** build the same: the same opcode, fixed file 0, the code from the same flag, and the same 16 bytes. The layout is asserted at compile time, and the test compares the bytes with the struct's `transmute`.
- **`ublk_submit_sqe_sync`** pushes to the submission queue as the library's own push does, and submits only when it is full. So the fetches reach the driver at the queue's first wait, as on the usual path, and the `sent on tag 0` line comes just before.
- **What the library's path does besides:**
  - It adds each command to its count (below).
  - It skips a command once the queue is stopping, which cannot be so at the start.
  - It refuses with `EPERM` when locking a buffer failed. The binding sets no library device flag, so the library never locks a buffer, and that check is always false.
- **Held by tests:** the payload's result and queue (A2, A3), the user data's code (A4) and the refused tag's address (A6).
- **Not held by a test:** the ioctl encoding of the code (A1, the code never encoded, passes every test). It reads the same flag as the library, so it holds by reading. Only a kernel would show it.

**2. The `OWN_FETCH` marks, the library's count and `ended()`. Right.**
- **Bit 63** is the library's `UblkUringData::Target`. `flush_and_wake_io_tasks`, which the queue's `wait` calls, hands every completion to the binding's handler. But it counts only those without bit 63 (`is_io_command`) against the commands it sent. So the binding's own fetches reach `ended()` and never lower the library's `u32` count. A7 (bit 62 alone) fails the test that asks for bit 63.
- **Bit 62** is the library's `UblkUringData::NonAsync`, a mark for its async path. Nothing in 0.4.8's synchronous path sets it, and `build_user_data` shifts its target data inside 32 bits. So no user data of the library has both bits. The wake's poll (`(1 << 63) | 1`) is still read as none.
- **The count without the fetches.** The library says the queue is down when it is stopping and its count is 0, and only an aborted command it counted marks it stopping. So the two consequences in the function's doc are right: a queue that made no commit is never reported down, and aborts of fetches never handed a request may come after it was.
- **The daemon's build.** At d63eb6c, without the feature, `ended()` is the base's line for line. `Buffers::new`, `KernelQueue::new` and `made` are the base's steps split out, unchanged: the same buffers, the same lock check, the library's fetches, then the wake's poll armed. A build of the whole workspace with `skyzfs-ublk-sys/control-test-program` compiles the branch into the daemon as well, as the gates do. That is harmless: nothing sets the marks.
- **A remark.** `OWN_FETCH` is the library's `Target | NonAsync`. If the binding ever uses the library's async path, the marks would mean something to the library. A line in the doc would say so.

**3. The start from a thread under a 10 s limit, and the end on the signal. Right, with one suggestion.**
- `start_aside` sends `device.start()` from its own thread, waits 10 s for it, says what it returned or `no answer after 10000 ms`, says a late answer when it comes, and returns only on `SIGTERM` or `SIGINT`.
- After the signal the program stops the queue's thread, whose armed wake ends its wait, and joins it. It sends no control command that could wait behind the start. A start still waiting is not joined and ends with the process.
- **Suggestion: say where the start waits.** `Device::start` first waits, in user space and with no limit, for the queue's thread to register its buffers. Only then does it send END_USER_RECOVERY; the library's `start_dev` does the same before START_DEV. A5 drops the registration from `first_fetches`, and every test passes. On a kernel the start would then wait in `wait_for_buffer_registration`, and the program would print `no answer after 10000 ms` exactly as for a wait in the driver.
  - The code as written does register: every tag's buffer, the refused tag's too, since only its fetch names 0. So this is not a defect.
  - But M1.3ah will rest on that one line. The line could show which wait it is. For example, at the limit the program could print the start thread's `/proc/self/task/TID/wchan`: a futex wait there is the binding, anything else is the kernel. Or it could time the registration wait and the command apart.

**4. Readings 1 to 5. All sound.**
- **1 (the value must agree with the step)** is held by B5, and **3 (the exclusions)** by B4.
- **2 (tag 0)** is printed in the `sent` line, so a later change of tag shows in the output.
- **4 (no `live` line)** and **5 (10 s)** are fine: the `start` or `end-recovery` line carries what `live` would.

### A remark on `taken`

If the driver takes the fetch with no buffer and tag 0's first request is a write, `take_buf` copies the data from the library's buffer of tag 0, since the tag is not yet the binding's. The driver was told address 0 for that fetch, not that buffer. So the write would put bytes that are not the client's into the file.
- `taken` is already the surprise the step looks for, so this changes no verdict.
- But the program says the request "is answered as any other". The doc could say that a write's data is not the client's after `taken`, so that the rig does not judge the data then.

### Planted changes

One at a time, each with the tests of `skyzfs-ublk-sys` with `control-test-program`.

| plant | result |
|---|---|
| A1: the fetch's code never ioctl-encoded | **nothing fails**: it reads the same flag as the library; only a kernel shows it |
| A2: the payload's result 0, not -1 | `a_fetch_names_its_queue_tag_and_address_as_the_driver_reads_them` fails |
| A3: the payload names queue 1 | the same test fails |
| A4: the user data names a commit | `a_first_fetch_of_the_bindings_own_is_read_as_a_fetch` fails |
| A5: the buffers are not registered before the fetches | **nothing fails**: on a kernel the start would wait in user space (suggestion 3) |
| A6: the refused tag names its buffer | `the_refused_tag_names_no_buffer_and_the_others_their_own` fails |
| A7: `OWN_FETCH` is bit 62 alone | `a_first_fetch_of_the_bindings_own_is_read_as_a_fetch` fails |
| B1: the `sent on tag 0` line is not said | both new server tests fail |
| B2: a refused fetch ends the server | `a_refused_first_fetch_is_said_and_the_server_goes_on` fails |
| B3: any tag's refused fetch is said | `a_first_fetch_with_no_buffer_that_is_taken_or_aborted_is_said` fails |
| B4: the option is taken with `--delete-after-ms` | `a_server_can_be_told_to_refuse_one_fetch` fails |
| B5: the value is not checked against the step | the same test fails |

### CI on d63eb6c, when I wrote this

- **Green:** the control rig on ubuntu-24.04 and 26.04 (43583095631), and the daemon rig's table job.
- **Still running or queued:** the control rig on 26.04-arm, the daemon rig on all three images, the replay rig's daemon jobs, ci.yml, coverage and e2e.

---
_Generated by [Claude Code](https://claude.ai/code)_

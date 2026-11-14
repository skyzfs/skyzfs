## Next look at M1.3ah-a: 88524f8

**Nothing blocks.** It can merge once CI is green on 88524f8 (the daemon rig and the replay rig's daemon jobs were still running when I wrote this). The new entry point is right, and so is its test. A kernel has now shown the `io_uring_enter` reading. I found no defect, and two small test gaps (C8 and C9 below).

**First, my miss.** My first look judged `start_aside` right without knowing that libublk keeps a control ring per thread. Testing's kernel run found the panic that I should have found by reading `ctrl.rs`.

At 88524f8 with rustc 1.100.0, these pass:
- `cargo fmt --check`;
- clippy `-D warnings` on `skyzfs-ublk`, `skyzfs-ublk-sys` and `skyzfsd`, plain, with `test-hooks` and with `control-test-program`;
- the tests of `skyzfs-ublk` (141 in the lib) and `skyzfsd`'s lib (59 with `test-hooks`, 50 without);
- the tests of `skyzfs-ublk-sys` with `control-test-program` (13 in the library, 17 in the program) and without it (9).

This container allows io_uring, so the new ring test ran here and passed.

### 1. The entry point and its test: right

- **The ring is the library's own.** `UblkCtrl`'s constructor calls `init_ctrl_task_ring_default(16)`, which builds `IoUring::<squeue::Entry128>::builder().build(16)` with no other setup flag. `own_control_ring` builds the same ring through the same public `ublk_init_ctrl_task_ring`, and keeps a ring the thread already has.
- **Commands on that ring.** The library's synchronous command path pushes onto the calling thread's ring, calls `submit_and_wait(1)`, and reads the completion by a token. The token is the control object's own count, so a second ring in the process changes nothing there.
- **The take-over.** END_USER_RECOVERY goes out on the binding's own ring (`raw::end_recovery`), as you say. Making a control ring there is harmless.
- **The library's lock, which I checked because the start may never return.** `start_dev` holds the control object's write lock (`get_inner_mut`) for the whole of START_DEV. While START_DEV waits in the driver, any other use of that object would wait behind it, in a futex. The program makes no such use:
  - `start_aside` moves the device into the start's thread;
  - the stop goes through the stopper and the wake;
  - the binding uses `control` only in `depth`, `serve` (before the start) and `start`, and the program no longer holds the device;
  - `--delete-after-ms` is refused with this option (B4 in my first look).
  
  So nothing waits behind the start. This is not a finding, but it is worth a line in `start_aside`'s doc if the program grows.
- **The test holds the helper.**
  - C1 (the helper makes nothing) fails it.
  - C2 (the helper always makes a new ring) fails it too. The new ring is built before the old one is dropped, so its descriptor differs.
- **Not held by a test: C3**, the program's thread calling `device.start()` as before. Only a kernel shows that, and testing's run did (next section).

### 2. The `io_uring_enter` reading: right, and a kernel shows it

- **Your correction is right.** Neither the library nor the binding sends a control command by `ioctl`.
  - The library's path is push, then `submit_and_wait(1)`, so a thread whose command is with the kernel is in `io_uring_enter`.
  - `wait_for_buffer_registration` is a condition variable, so it waits in `futex`.
  - My "anything else is the kernel" was loose. The line as written is better.
- **Testing's M1.T36 try on 88524f8** ([run 43585528398](https://github.com/skyzfs/skyzfs/actions/runs/43585528398), job `ubuntu-26.04`) reads, at a start and at a take-over:
  - first `sent on tag 0`, then `refused: errno 22`;
  - then `no answer after 10000 ms; its thread waits in io_uring_enter (wchan io_cqring_wait)`;
  - and still no answer 10 s later.
  
  So the start no longer panics, its command reached the kernel, and it waits there. When I wrote this, its 24.04 and arm64 jobs were still running.
- **What `io_cqring_wait` says.** It is io_uring's wait for completions, not a ublk function. So the start's own thread is not the one that waits in the driver. I believe ublk sends a non-blocking issue of a control command back with `-EAGAIN`, so io_uring hands it to one of the process's workers, and the driver's wait is there. That is from memory of the driver, not from source I could read here. The line does not claim more than this, as your answer says.
- **Suggestion (optional): name the driver's wait itself.** At the limit, also print the `wchan` of the process's io_uring workers: the threads in `/proc/self/task` whose `comm` is `iou-wrk-<the start thread's id>`. Their `wchan` should name the ublk function that waits (for START_DEV, I expect `ublk_ctrl_start_dev`). Then M1.3ah's line would say "the driver waits" in the kernel's own words rather than by inference.
- **A test gap (minor).** `a_threads_wait_is_named_by_its_system_call` checks only that `waits_in` contains `(wchan `.
  - C8 (`waits_in` reads `stat` instead of `syscall`) passes every test.
  - A thread that reads its own `syscall` file is in `read`, so `waits_in(own_tid)` starts with `syscall {libc::SYS_read} ` (`syscall 0` on amd64, and `wchan 0`; I checked `/proc/self/syscall` in this container).
  - Asserting that prefix would hold C8.
  - C7 (the thread's id never sent, so the line says `unknown`) also passes every test. Only a kernel shows that one, and testing's run shows the id is sent.

### 3. `taken` answered with an I/O error: right

- **What the commit sends.** `commit(0, -EIO, None)` puts nothing back, and names tag 0's own buffer, which `Buffers::new` made for every tag, tag 0 included. So tag 0's next fetch has an address, and later requests on it are served as any other. The result is negative, so the driver ends the request with the error and copies nothing.
- **Held:** C5 (the old answer) and C6 (not counted as failed) each fail the test.
- **Not held: C9**, `refusing_fetch` not cleared after `taken`. Every later request on tag 0 is then answered with an I/O error and said `taken` again, and every test passes. The test hands tag 0 only one request. A second request on tag 0 after `taken`, expected to be served from the file and not said, would hold it.
- **A note for the rig, not for the code (medium confidence, from memory of `ublk_drv.c`).** On a driver that takes a fetch with no address, I expect that a write on tag 0 is never handed to the server at all:
  - The driver copies a write's data to the fetch's address before it hands the request over.
  - When nothing could be copied, it requeues the request and tries again ("Nothing mapped, retry until we succeed").
  - A read is handed over and gets the I/O error.
  
  So a course that writes first would see the start return `ok` (all fetches arrived, so the start completes), and then a write that hangs with no `taken` line. The start's `ok` already shows the fetch was taken, so the verdict is the same. But the course should drive tag 0 with a read, or not wait for `taken` after a write. On 26.04 the fetch was refused, so this stays hypothetical there.

### Planted changes

One at a time, each with the tests of `skyzfs-ublk-sys` with `control-test-program`.

| plant | result |
|---|---|
| C1: `own_control_ring` makes nothing | `a_thread_that_did_not_open_the_device_makes_its_own_control_ring` fails |
| C2: `own_control_ring` always makes a new ring | the same test fails |
| C3: the program's start thread calls `device.start()` | **nothing fails**: only a kernel shows it, and testing's run did |
| C4: `futex` and `io_uring_enter` named the other way round | `a_threads_wait_is_named_by_its_system_call` fails |
| C5: `taken` answered as any other (the old code) | `a_first_fetch_with_no_buffer_that_is_taken_or_aborted_is_said` fails |
| C6: `taken` not counted as failed | the same test fails |
| C7: the start thread's id never sent | **nothing fails**: only a kernel shows it |
| C8: `waits_in` reads `stat`, not `syscall` | **nothing fails** (section 2) |
| C9: after `taken` the tag is still watched | **nothing fails** (section 3) |

### Smaller points

- **The conflict with #83 stands.** Both change the control program's `server()` loop, so whichever merges second has to put the refused fetch's reading beside #83's `note_ended`.
- **Codex** reviewed 88524f8's code with no finding. Its security review was still running when I wrote this.

### CI on 88524f8, when I wrote this

- **Green:**
  - the control rig on all three images ([43585427162](https://github.com/skyzfs/skyzfs/actions/runs/43585427162));
  - the daemon rig's table job;
  - ci.yml's lint, chart-lint and s3-backends;
  - e2e-tier-a.
- **Still running:**
  - the daemon rig on all three images ([43585427209](https://github.com/skyzfs/skyzfs/actions/runs/43585427209));
  - the replay rig's daemon jobs ([43585427212](https://github.com/skyzfs/skyzfs/actions/runs/43585427212));
  - ci.yml's test and unit-arm64;
  - coverage and the k3s smokes.

---
_Generated by [Claude Code](https://claude.ai/code)_

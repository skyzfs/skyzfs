## Next look at M1.3ac part 3: 343dd01

**Nothing blocks now.** It can merge once CI is green on 343dd01, the daemon rig included. The blocking finding is fixed, and both suggestions are taken. Each of the plants the earlier tests missed now fails a test. I found nothing new that blocks.

At 343dd01 with rustc 1.100.0, these pass:
- `cargo fmt --check`;
- clippy `-D warnings`, plain, with `test-hooks` and with `control-test-program`;
- the tests of `skyzfs-ublk` (143 in the lib, and its integration tests), of `skyzfsd`'s lib (63 with `test-hooks`, 51 without) and of `skyzfs-ublk-sys` with `control-test-program` (14 in the program, 9 in the library).

### The fix

- **The end of a stop, in this order: QueueDown, then CommitRefused, then Orderly. That order is right.**
  - The flush's ends are counted first, whatever follows.
  - A flush that failed ends with `QueueDown`. The device is then going, and 8's line would say that it waits, which is reading 3's reason at a wait.
  - Otherwise the first commit refused in the flush ends the pump, and only then is the stop orderly.
  - `STOP_UNSENT` still returns `Orderly` with no flush, as before.
- **The failed volume's turn** counts the flush's ends and still ends `Failed` (reading 2).
- **`KernelQueue::flush`** reads each completion with `ended()`, as `wait` does. A fetch that completes there (`Ok(tag)`) is still dropped: its request is the next server's.
- **The wrappers.**
  - `Delayed::flush` releases every commit it holds or delays before the inner flush. So a held or delayed commit that is refused there is seen too.
  - `Recorded` writes the flush's `ended` lines after its `sent` line.
  - `Refusing` passes everything on.
- **The control program** counts and says the flush's ends through `note_ended`, as at a wait.

### Planted changes

One at a time, with the tests of `skyzfs-ublk`'s pump and `skyzfsd`'s serve, or of the crate the plant is in.

| plant | result |
|---|---|
| P1 (from the first look): the last refused commit of a wait is named | `a_commit_the_driver_refuses_ends_the_pump_for_a_take_over` fails (it passed at 3c496d5) |
| P4 (from the first look): the refusal checked after the wait's requests are taken | the same test fails (it passed at 3c496d5) |
| N1: at a stop's end the refusal is checked before QueueDown | **nothing fails** |
| N2: what a failed flush reaps is not counted | **nothing fails** |
| N3: a failed volume's last flush is not counted | **nothing fails** |
| N4: `KernelQueue::flush` hands nothing on | **nothing fails**: only a kernel shows it, as for `wait`'s handler |
| N5: the request log writes no `ended` line at a flush | `a_command_the_driver_ended_otherwise_than_ok_has_its_line` fails |
| N6: the control program does not read its last flush | `a_commit_with_no_buffer_refused_at_the_last_flush_is_said` fails |

**Suggestion, for N1 to N3.** The order you asked me to judge is right, but no test holds it. Two more cases in the new stop test would:
- a flush that fails and brings a refused commit: expect `QueueDown`, with the refusal counted (N1, N2);
- a failed volume whose last flush brings one: expect `Failed`, with it counted (N3).

### Your reading: the refusal is posted before the flush's submit returns

- **The library's side, which I checked in libublk 0.4.8.**
  - `flush_and_wake_io_tasks(handler, 0)` goes to `__wait_ios(0)`, which submits with no wait (`submit_with_args(0, …)`).
  - It then takes `ring.completion().len()` and hands each of those completions to the handler.
  - The queue's ring has no SQPOLL.

  So whatever io_uring has posted by the time the submit returns, this flush reaps.
- **The kernel's side.** I agree with your reading from memory of `io_uring/uring_cmd.c` and the submit path, but I could not read the source from here.
  - ublk's `uring_cmd` handler refuses such a commit inline, in the submitting task.
  - io_uring completes an inline failure in its submit state, which it flushes before `io_uring_enter` returns.
  - The ring's `COOP_TASKRUN` concerns task work, not those completions.
- **A kernel can show it, with the two hooks on one request:** `serve --hold-one-request 300:write:B --refuse-one-commit write:B`, a writer at B, and `SIGTERM` during the hold.
  - The pump has handed the commit to `Delayed`, so it drains at once and calls the flush.
  - `Delayed::flush` releases the held commit (`release(true)`). `Refusing` sends it with no buffer, and the flush submits it.
  - If the reading holds, serve ends with 8, and the request log has the `ended` line just after the flush's `sent`. If serve ends with 0, the reading is wrong.

  That course would fit beside M1.T35's.

### Smaller points

- **A broken link.** Your reply on Codex's thread points to `pull/83#issuecomment`, with no comment id.
- **#84 will conflict in the control program, not only in `queue.rs`.** This head moves the counting of `server()`'s loop into `note_ended`. #84 adds its refused fetch's lines to that same loop. #84's description names only `queue.rs` as shared. Whichever merges second has to put the refused fetch's reading beside `note_ended`.

### CI on 343dd01, when I wrote this

- **Green:**
  - the control rig on all three images (43584619559);
  - the replay rig's "write into a hole" on 26.04;
  - ci.yml's lint, e2e, chart-lint and s3-backends.
- **Still running:** the daemon rig on all three images (43584619311), the replay rig's daemon jobs, ci.yml's test and unit-arm64, and coverage.
- **Codex** reviewed 343dd01 with no finding, and its thread is resolved.

---
_Generated by [Claude Code](https://claude.ai/code)_

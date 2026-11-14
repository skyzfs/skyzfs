## Review of M1.T35 at 707cab3

**Nothing blocks, and I found no defect.** The four courses check the right things, the kill test reads 8 as it reads 7, and the replay rig's limit is right. Three suggestions:
- A test should hold the client's exit after the take-over (R1 below). On the daemon it is the only evidence that the take-over completed a write or a flush, and no test fails without it.
- The flush course's PASS does not show that the refused flush was the client's. I found nothing that flushes first, and a small change would make it certain.
- Two loops are limited per try, not in total.

This time I read the description before the code, because it came with the branch's name. At 707cab3:
- `bash tests/kernel/test_daemon_refused.sh` passes (73 s here);
- `python3 tests/kernel/test_daemon_check.py` passes its 86 tests.

shellcheck is not installed in my container; ci.yml's lint is green on 707cab3.

### 1. `rc_course`'s PASS: the right conjunction

Each part is needed:
- **exit 8 with its line.** I checked the words against `commit_refused_said` in `serve.rs`. `ended()` is serve's last call, so the line is the log's last.
- **QUIESCED.** The device waits for a server.
- **The client still waits 2 s after.** The refused request was not answered.
- **The client exits 0 after the take-over.** The request was handed again and answered.
- **The block read back through the device, the next stop's 0 and the home image.** The data and the clean end.

**What shows the take-over's completion of a write or a flush is the client's exit, not the data.**
- In the daemon, a write is in the store before its answer is committed. So by the time the driver refuses the commit, the first server has already put the block in the home image.
- So the read-back and the home image would read `same` whether the take-over carried out the reissued write or only answered it.
- For a write and a flush, `[ "$crc" = 0 ]` is therefore what carries "the take-over completes the request". For the read, the client's data shows it too, since the first server's data never reached the client.
- The made-up `lost` scenario FAILs on the data only because the made-up server writes the home image at its stop. On the daemon the only way to catch such a take-over is the client's exit.

**Suggestion, for R1 and R2 (table below).** Two more made-up scenarios would hold the client's exit and the last stop's exit:
- a take-over that answers the reissued request with an error (the made-up `dd` exits 1);
- a take-over whose stop ends with another status.

### Limits: every wait has one, but two loops' limits are per try

- **The QUIESCED loop** is 80 tries of `ublk_state` (`lim 20` each) and 0.25 s. That is 20 s when `ublk list` answers at once, and up to 27 minutes if every try reaches its limit.
- **The held loop** is 120 tries of `vol_stat` (`vol_status` is `lim 30`) and 0.25 s. That is 30 s normally, and up to an hour.
- The job's own limit ends either one, so nothing waits forever. But the description's "20 s" would not hold in the one case where it matters, a device or a daemon that does not answer.
- **Suggestion:** a deadline by the clock (`now_ms`) for both loops.

The other waits are bounded as the description says:
- serve's end, 60 s;
- the client: `lim 240` inside it, and `job_end 120` after the take-over;
- the reads, 30 s each;
- the last stop, 30 s;
- `serve_start`, 120 s;
- `vol_detach`, 90 s;
- `rc_cleanup`'s own limits.

### 2. The flush course: almost surely the client's flush, but the PASS does not show it

**What I found that could flush the fresh device first: nothing.**
- The course writes to the raw device of a new volume, with no pool or file system on it.
- The device has a volatile write cache (`UBLK_ATTR_VOLATILE_CACHE` in `queue.rs`), so the client's fsync reaches the daemon as a flush. `blkdev_fsync` sends one whatever is dirty.
- udev's probes after the add only read. Closing a block device writes back its page cache but sends no flush.

**But the verdict would be the same if something else's flush were refused first.**
- The client's write would then wait on the quiesced device from its start.
- The take-over would answer the other flush, and the client's write and fsync would follow, giving PASS.
- The line would show the client waiting from its start, but nothing reads that.

**Suggestion: split the client in two.**
1. First the write, as its own `dd ... oflag=direct` under a short limit, which must return 0 while serve lives. That rules out an earlier flush that was refused.
2. Then the client is the fsync alone: `"$DD" if=/dev/null of="$BDEV" conv=notrunc,fsync status=none`. GNU dd with nothing to copy still calls `fsync` on its output; strace here shows `fsync(1) = 0` for `dd if=/dev/null of=FILE conv=notrunc,fsync`.

Then the refused flush is the client's by construction, but for the instant between the two.

### 3. The stop course: right, and the negative control differs by the exit alone

**How the daemon takes it.**
- Both hooks name the one write, so `Delayed` holds that write's commit, and `status` says `held` before the SIGTERM.
- At the SIGTERM, the pump has nothing in flight but the held commit, so it drains at once.
- Its last flush releases the held commit (`Delayed::flush` calls `release(true)`), and `Refusing` sends it with no buffer.
- Since 343dd01, the flush's refusal ends the pump with `CommitRefused`, and serve ends with 8 and the line.

**The negative control.** At 3c496d5, `Delayed::flush` already released held commits (I read it there). So that build drains, sends the refused commit, drops the refusal, and ends 0, rather than ending by a timeout. The pin then FAILs with "serve ended with exit 0", as `test_a_stop_that_ends_0_differs` expects.

In its table, the rest of the course should go as on the daemon: QUIESCED, the client waiting, the take-over completing. Then the control shows exactly the one difference.

### 4. `test_daemon_refused.sh`'s made-up server: matches #83

The made-up server does three things:
- **At a refusal:** the device QUIESCED, `skyzfsd: the kernel refused serve's answer on tag 0 of device 9 (...)`, and exit 8. The words are `commit_refused_said`'s.
- **At a stop with the refused request held:** 8 (343dd01). `stop0` is the control.
- **At any other stop:** 0.

One difference does not matter to the rig: it writes the home image at the stop, where the daemon writes as it goes (section 1).

### 5. The replay rig's limit: right

- `needs.plan.outputs.budget` is the plan job's output (`pull-request` or `full`).
- `timeout-minutes` may read `matrix` and `needs`.
- `startsWith(matrix.image, 'ubuntu-24.04')` takes both 24.04 images.
- `cond && 180 || 120` gives 180 or 120.

This pull request's replay jobs started, so the expression parses. Only the next full dispatch on 24.04 takes the 180 branch.

### The kill test's 8

Both places read 8 as they read 7, and the new `KillRefusal` tests cover the pin, the control and the loop.

### Planted changes

One at a time in `daemon-rig.sh`, each with `bash tests/kernel/test_daemon_refused.sh`.

| plant | result |
|---|---|
| R1: the client's exit after the take-over not checked (`[ "$crc" = 0 ]` removed) | **nothing fails** (section 1) |
| R2: the next stop's exit not checked (`[ "$stop" = 0 ]` removed) | **nothing fails** |

Your ten plants cover the rest of the conjunction.

### CI on 707cab3, when I wrote this

- **Green:**
  - the daemon rig's table job (which runs the new test);
  - ci.yml's lint, chart-lint and s3-backends;
  - e2e-tier-a;
  - the replay rig's "write into a hole" on both images;
  - the kernel probe.
- **Still running:**
  - the daemon rig on the daemon and the stand-in ([43592866016](https://github.com/skyzfs/skyzfs/actions/runs/43592866016));
  - the negative control's dispatch ([43592894494](https://github.com/skyzfs/skyzfs/actions/runs/43592894494));
  - the replay rig's daemon jobs;
  - ci.yml's test and unit-arm64;
  - coverage.

The four pins' first run on a kernel is in 43592866016, so I have not seen them PASS yet.

---
_Generated by [Claude Code](https://claude.ai/code)_

## Review of M1.T37 at 7b2b083

**One finding blocks, and it is small (section 1).** A daemon that hangs with the hook set is judged as the rig's error, not as a FAIL. That hang is what the two courses exist to catch.

The rest is right:
- **Your point (a): no wrong daemon passes.** Every check of `rf9_start` and `rf9_take` fails a test when left out.
- **Your point (b):** the cleanup is bounded.
- **Your point (c):** the made-up server follows #87 closely enough that its FAIL scenarios mean something.
- **The kill test** reads a 9 as it reads 7 and 8.

I read the diff before the description. At 7b2b083:
- `bash tests/kernel/test_daemon_fetch.sh`: all passed, 39 checks, 2 min 16 s.
- `python3 tests/kernel/test_daemon_check.py`: 90 tests, OK.
- shellcheck is not installed here; ci.yml's lint is green on this head.

### 1. Blocks: a serve that hangs with the hook set is the rig's ERROR, not a FAIL

**Where.** When `serve --refuse-one-fetch` neither ends nor becomes ready within `serve_start`'s 120 s, both courses return 2:
- at a start, `daemon-rig.sh:2081-2085`: "serve neither ended nor became ready";
- at a take-over, `daemon-rig.sh:2149-2153`: "the take-over neither ended nor became ready".

**Why that is the case these courses exist for.** The description says the two courses show what #87 rests on:
- **A delete answers a START_DEV that waits.** #87's refused-fetch branch sends that delete with no limit (my look at 8387402, section 1). If the delete does not answer, serve never ends.
- **The process's end answers an END_USER_RECOVERY that waits.** If it does not, the process does not finish exiting.

Either failure is the hang above, and the pin then reads ERROR with "none: neither ready nor gone after 120 s". CLAUDE.md tells a reader that the rig's error is "not a verdict on the server". So the one outcome that would show #87's premise false on a kernel reads as the rig at fault, and invites a rerun.

**Shown.** In a copy of `test_daemon_fetch.sh`, I added a made-up server that, with the hook set, neither gives up nor becomes ready. Both courses return 2:

```
FAIL   HANG at a start: the verdict: got '2', want '1'
FAIL   HANG at a take-over: the verdict: got '2', want '1'
```

**Inconsistent with the rig's own courses.** `rc_course` (M1.T35) reads "serve did not end in 60 s" as a FAIL (`daemon-rig.sh:2229-2231`). 120 s with no end is outside what #87 says serve does:
- it gives a refused first fetch up as soon as the queue ends, trusting the driver to answer its delete;
- it gives up a start with no answer within 42 s in all.

**The fix:**
1. Return 1 in both places, with a line that says what was expected. For example: "serve neither ended nor became ready in 120 s; #87 gives a refused first fetch up at once".
2. Add the hang scenario to `test_daemon_fetch.sh` with want 1.
3. In the take-over's branch, wait for the killed serve (`serve_wait 30`) before `SERVE_PID=""` (`:2151-2152`). Otherwise `rf9_cleanup`'s serve can race a dying server for the device.

### 2. Your point (a): can `rf9_start` or `rf9_take` pass on a wrong daemon? No

**Every check is held.** With each check left out, one at a time, some scenario of `test_daemon_fetch.sh` fails (table below). The four gaps your first round found are closed: P3, P4, P11 and P9 each fail one of your added scenarios.

**The held I/O is judged both ways.**
- **Still held after the 9.** It is sampled after the wait for QUIESCED. `releases` holds this (P7).
- **Completed by the next take-over with exit 0.** `ioerr` holds this (P9).
- **Its pool after.** `poolbad` holds the pool ONLINE with no vdev error (P10).

**Limits, none of which lets a wrong daemon pass by itself:**
- **The PASS rests on the daemon's word that the kernel refused the fetch.** The rig sees no evidence of its own. A daemon that gave up at once, without sending the fetch, would pass.
  - #87's unit tests hold that branch.
  - The control rig's `pin_refused_fetch_sent_and_refused` (M1.T36) holds the driver's side.
  - So this is acceptable.
- **`rf9_start` checks that the number the 9's line names is free** (`:2089-2092`). A daemon whose line named another number than the device it added would pass that check while its device stayed. Either of two cheap checks would close it, as a suggestion:
  - The daemon names its device before the start, in the hook's own line: "--refuse-one-fetch start: tag 0's first fetch on queue 0 of device N is sent with no buffer" (`bins/skyzfsd/src/refuse.rs:141-144`). Require that line, and the same N in the 9's line.
  - Or compare `ublk list` before the serve and after the 9.
- **`"${st:-}" != failed` accepts an empty status.** The next serve covers it, since a failed volume's serve is refused with 4. Fine.

### 3. Your point (b): the cleanup of a take-over that went wrong: right, and bounded

- **"No server" reads right.** `serve_start` and `serve_wait` clear `SERVE_PID` when the process has ended. So after a 9, or after a kill and its wait, the test is correct.
- **A serve first, when the device is QUIESCED with none,** is what lets the held I/O complete and `kill_cleanup`'s export work.
- **If that serve fails too, the cleanup still ends:**
  1. `job_end` kills the writer after 60 s;
  2. the export of an ONLINE pool with I/O held waits up to its `lim 120`;
  3. the detach deletes the device.

  That is the same as the other courses.
- **It reads `bdev`, `dir`, `writer` and `name` from `rf9_take`'s locals** by bash's dynamic scope. That works; a comment saying so would help the next reader.
- **Not tested.** With the cleanup's serve removed (P13), `test_daemon_fetch.sh` still passes. A scenario where only the next plain serve fails would show that the cleanup's serve ends the held writer, with no 60 s kill. Not needed for the merge.
- **One race,** in the ERROR branch: section 1, item 3.

### 4. Your point (c): the made-up server against #87's daemon: close enough

**What matches:**
- **Which start the hook takes.** It is the daemon's rule (`refuse.rs:136-139`): `start` when not taken over, `recovery` when taken over. A start of the other kind serves plainly, as the daemon does.
- **Its two lines** are #87's `gave_up_said` (`serve.rs:1625-1665`) word for word, and match #87's own test strings.
- **The device follows the driver:**
  - deleted at a start, left waiting at a take-over;
  - a killed server leaves it QUIESCED;
  - a `zpool sync` waits until a live server serves it.

**Differences that change no verdict:**
- it prints no hook line;
- its 9 comes at once;
- it marks the device deleted before it exits. On a kernel the number is freed at the process's end, which the rig's 5 s covers.

**Wrong daemons it does not play:**
- the hang of section 1;
- a 9 by the other road, "had no answer within 30 s". The words check would FAIL it, so a scenario for it is optional.

### 5. The kill test's 9: right

- **Both places that read 7 and 8 before a kill now read 9.** Leaving 9 out of both (P14) fails `test_the_kill_loop_reads_a_9_from_a_server_that_ended_before_the_kill`.
- **`kt_refusal` names the 9.** With no case for 9 (P15), three tests fail.
- **The cut at 600 characters** keeps the whole line. The line of 9 is about 360.

### Planted changes

One at a time, each in its own copy of `tests/`, then thrown away. In P1 to P12, each check was replaced by `true`.

| plant | test | result |
|---|---|---|
| P1: start, the number free | `test_daemon_fetch.sh` | "a device that is not deleted" fails |
| P2: start, the volume's status | same | "a volume that reads failed" fails |
| P3: start, "is deleted" | same | "the take-over's words at a start" fails |
| P4: start, the next serve's read and stop | same | "a next serve that does not start" fails |
| P5: start, the supervisor's words | same | "a line without what a supervisor should do" fails |
| P6: start, exit 9 | same | "a start given up with 0" fails |
| P7: take-over, the I/O still held | same | "a server that answers the held I/O before it gives up" fails |
| P8: take-over, QUIESCED | same | "a take-over that deletes the device" fails |
| P9: take-over, the held I/O's exit | same | "held I/O that fails after the take-over" fails |
| P10: take-over, vdev errors | same | "vdev errors" fails |
| P11: take-over, "left waiting" | same | "the start's words at a take-over" fails |
| P12: take-over, the volume's status | same | "a volume that reads failed" fails |
| P13: the cleanup's serve removed | same | **nothing fails** (section 3) |
| P14: the kill loop does not read 9 | `test_daemon_check.py` | 1 test fails |
| P15: `kt_refusal` has no case for 9 | same | 3 tests fail |
| hang: a made-up server that neither gives up nor becomes ready | `test_daemon_fetch.sh`, copy | both courses return 2 (section 1) |

### CI on 7b2b083, when I wrote this

- **Green:**
  - the daemon rig's no-kernel job, which runs `test_daemon_check.py` and `test_daemon_fetch.sh`;
  - the kernel probe on both images ([43609145648](https://github.com/skyzfs/skyzfs/actions/runs/43609145648));
  - ci.yml's lint, chart-lint and s3-backends;
  - e2e-tier-a;
  - the replay rig's "write into a hole" jobs and its daemon job on 26.04.
- **Running:**
  - the daemon rig on the daemon (24.04, 26.04, 26.04-arm) and on the stand-in ([43609145702](https://github.com/skyzfs/skyzfs/actions/runs/43609145702));
  - the replay rig's other jobs;
  - ci.yml's test and unit-arm64;
  - coverage.

I will read the two new lines on each image when the daemon rig ends.

---
_Generated by [Claude Code](https://claude.ai/code)_

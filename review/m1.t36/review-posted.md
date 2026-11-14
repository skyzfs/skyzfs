## Review of M1.T36 at e33eff9

**Nothing blocks.**
- The limits are right.
- Observing and not judging is what the lead asked for.
- The made-up program's words match #84's program.

**One defect in what a line records, low severity.** It is Codex's finding, and I confirm it (section 3):
- The fetch's answer is read once, early.
- A `taken` or `aborted` that comes later is left off the line.
- None of the three kernels gets there, so the table in the description is right.

I read the code before the description. At e33eff9:
- `bash tests/kernel/test_control_fetch.sh` passes (27 ok).
- shellcheck is not installed in my container; ci.yml's lint is green on this head.

### 1. Every wait has a limit (your point 1): right

In `refused_fetch`:
- **The start's line:** 200 × 0.1 s, and the loop leaves early when the program has ended.
- **The late look:** 100 × 0.1 s.
- **The read:** `lim 15`.
- **The end.** A program that has ended by itself is waited for, not signalled. bash keeps a reaped background job's status for `wait`, so `$?` is its exit; the `panics` scenario holds that with 101. Otherwise:
  - SIGTERM, 10 s;
  - then kill -9, 10 s;
  - then the stacks, `lim 5` per thread, and the program is left behind with no `wait`.
- **`rf_delete`:** `say` (`lim 60`), `wait_state` 10 s and `free_for_add` 5 s.
- **The take-over's preparation:** `serve_bg`, `srv_end 30` and `wait_state 5`.

### 2. Only the control is judged (your point 2): right for what the lead asked

- The lead asked for observations, with only the control judged. That is what this does.
- The "(PASS)" in front of an observation is the same test as the control's verdict. Since it judges nothing, that is fine.
- **Suggestion, for the lead.** If the program stopped sending the bad fetch, the observation would read "(FAIL) ... 'nothing'" and CI would stay green.
  - "sent on tag 0" is the binding's own doing and does not depend on the kernel. So a pin on that line alone would judge the binding and not the driver.
  - This is not needed for the merge.

### 3. The made-up program against #84 (your point 3), and Codex's finding

**The words match.** I checked each one against `say` in `skyzfs-ublk-control.rs`:
- `sent on tag 0`, `refused: ...`, `taken`, `aborted`;
- `returned after N ms: ok|errno`, `no answer after N ms ...`;
- the `served` line.

**Two answers come at the wrong time.** The made-up program says `taken` and `aborted` right after `sent`. The real program says them later:
- `taken` only when tag 0 is handed a request (`skyzfs-ublk-control.rs:630`);
- `aborted` only when the queue's commands end at the stop (`:724`).

So `test_control_fetch.sh` cannot show the gap Codex found.

**Codex's finding: confirmed.** `said` is read once, right after the start's line (`control-rig.sh:902`), and not read again. The line then reads "then 'nothing'" in two cases:
- **A driver that takes the bad fetch.** The start returns ok, and `taken` comes only when the read hands tag 0 a request. It may never come: one request does not reliably land on tag 0. In this run's refused-commit courses, a single request landed on tags 0, 3, 4, 5 and 6.
- **A driver that holds the fetch until the stop.** `aborted` comes at the SIGTERM, after the line was made.

The raw lines still go into `steps.log`. But the observation line is what the notes will quote.

**Suggested fix.**
- Read `rf_line refuse-fetch 2` again after the program's end, and add it to the line when it differs.
- In the made-up program, say `taken` at the read and `aborted` at the SIGTERM, so a test holds the fix.

### 4. Smaller points

- **The `served` line is cut at 120 characters.** That drops `commands_aborted` and `commands_failed`, which would count an aborted or refused fetch at the end. On this run it reads "... first_write_copies 0 commands". A cut at 200 keeps them.
- **The three step blocks are not run by the test.** The take-over's preparation and the control's block are covered only by a kernel run, and this head's run shows them working.

### Planted changes

One at a time in `control-rig.sh`, each with `bash tests/kernel/test_control_fetch.sh`. Your eight cover the rest.

| plant | result |
|---|---|
| F5: the verdict does not look at "sent on tag 0" | **nothing fails**. No scenario says `refused` without `sent`, and the real program always says `sent` first. Harmless |
| F7: the late look runs its full 10 s after an answer | **nothing fails**. It costs time only; the line is the same |

### The kernel, on this head

[The control rig on e33eff9](https://github.com/skyzfs/skyzfs/actions/runs/43596458451) is green on all three images. I read each image's three lines, and they agree with the table in the description:
- **At a start:** the fetch is refused with errno 22. START_DEV has no answer in 10 s or 10 s later; its worker waits in `ublk_wait_dev_ready_and_lock`. The device reads dead with no block node. SIGTERM ends the program, a delete returns in 3–5 ms, and the number is free.
- **At a take-over:** the same for END_USER_RECOVERY. There are two workers, the main thread's idle one (`io_wq_worker`) and the start thread's in `ublk_wait_dev_ready_and_lock`, as expected in #84. The read gets no answer in 15 s. SIGTERM ends the program, a delete returns in 21–27 ms, and the number is free.
- **The control:** FAIL, as expected. A plain serve goes live, and its read returns in 9–25 ms.

**For M1.3ah (#87).** Every delete here came after SIGTERM had ended the program. So these steps do not show a delete answering a START_DEV that is still waiting.

### CI on e33eff9, when I wrote this

- **Green:**
  - the control rig on all three images;
  - the kernel probe;
  - ci.yml's lint, chart-lint and s3-backends;
  - both table jobs;
  - the replay rig's "write into a hole" on both images.
- **Still running:**
  - the daemon rig on the daemon and the stand-in;
  - the replay rig's daemon jobs;
  - ci.yml's test and unit-arm64;
  - coverage and e2e-tier-a.

---
_Generated by [Claude Code](https://claude.ai/code)_

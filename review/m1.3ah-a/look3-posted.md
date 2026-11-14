## Next look at M1.3ah-a: b35f72f

**Nothing blocks.** It can merge once CI is green on b35f72f; the daemon rig, the replay rig's daemon jobs and ci.yml's tests were still running when I wrote this. C8 and C9 are held. The merge with #83 reads both refusals as each pull request meant. The workers line is right, but it reads differently from what its doc and your answer expect, and a test does not hold its reading of `/proc` (below). Neither is a defect.

At b35f72f with rustc 1.100.0, these pass:
- `cargo fmt --check`;
- clippy `-D warnings` on `skyzfs-ublk`, `skyzfs-ublk-sys` and `skyzfsd`, plain, with `test-hooks` and with `control-test-program`;
- the tests of `skyzfs-ublk` (143 in the lib) and `skyzfsd`'s lib (63 with `test-hooks`, 51 without);
- the tests of `skyzfs-ublk-sys` with `control-test-program` (13 in the library, 19 in the program) and without it (9).

### 1. C8 and C9 (fea5954): held

- **C8** (`waits_in` reads `stat`) now fails `a_threads_wait_is_named_by_its_system_call`.
- **C9** (the tag still watched after `taken`) now fails `a_first_fetch_with_no_buffer_that_is_taken_or_aborted_is_said`.
- The second request on tag 0 is served from the file, `(0, 4096, true)`, and is not said, as I asked.

### 2. The merge with #83 (98f6743): right

- `note_ended` takes both tags, at a wait and at the stop's last flush:
  - the refused commit returns `true` and ends the server, as in #83;
  - the refused fetch is said and ends nothing, as here.
- `KernelQueue::flush` is #83's, and reads each completion with `ended()`. With the feature, `ended()` strips this pull request's marks, so the binding's own fetch reaped at a flush is read as a fetch.
- Plants on the merged reading:

  | plant | result |
  |---|---|
  | D5: a refused fetch ends the server | `a_refused_first_fetch_is_said_and_the_server_goes_on` fails |
  | D6: the last flush does not read the refused commit | `a_commit_with_no_buffer_refused_at_the_last_flush_is_said` fails |
  | D7: a wait does not read the refused commit | three tests fail |
  | D8: a wait does not read the refused fetch | both refused-fetch tests fail |
  | D4: the last flush does not read the refused fetch | **nothing fails** |

- **D4 is not a gap worth a test.** The fetches go out at the first wait, and the driver refuses one when it is submitted. So its refusal is reaped by that wait, before any stop's flush. Reading it at the flush too keeps the two calls alike, and that is fine.

### 3. The workers line (a8e5e18): right, with two notes

- **What it does.** It lists every thread of the process whose `comm` starts with `iou-wrk-`, with its `wchan`. The parsing is held: D1 (every `iou-` thread taken, so `iou-sqp-` too) fails `the_io_uring_workers_are_named_with_their_wait`.
- **Note 1: expect two workers, not one.** Your answer says "The program runs one start, so there is nothing to sort out." But a worker stays after its request ends.
  - I checked this in this container with a small program. A read on an empty pipe, sent with `IOSQE_ASYNC`, made the thread `iou-wrk-<tid>` with `wchan io_wq_worker` at once. That thread was still there, with the same `wchan`, 7 s after the read had completed.
  - Testing's runs show that the start's control command waits in `io_cqring_wait`, not in ublk. So the driver's control commands run in a worker. The main thread's earlier commands (ADD_DEV and the others it sends when it opens the device, and BEGIN_USER_RECOVERY at a take-over) will likely have left an idle worker `iou-wrk-<main thread's id> (wchan io_wq_worker)`.
  - So the line will likely read something like `io_uring workers: iou-wrk-M (wchan io_wq_worker), iou-wrk-S (wchan <the driver's function>)`.
  - That is still readable: an idle worker waits in `io_wq_worker`. But two cheap changes would make it plain:
    - print the start thread's id in the line (`its thread S waits in ...`), so that its worker can be matched;
    - say in the module doc that an idle worker reads `io_wq_worker`.
  
  This is medium confidence, since the program on a kernel is the test, and testing's rerun on this head will show it.
- **Note 2: the reading of `/proc` is not held.** `the_io_uring_workers_are_named_with_their_wait` checks `workers_named` on a made-up list, and `workers_wait()` only for not being empty. D2 (`workers_wait` reads `status` instead of `comm`) and D3 (`workers_wait` always says `none`) pass every test.
  - A test can make a real worker, as my check did. In a thread, push a read on an empty pipe with `squeue::Flags::ASYNC` on an `IoUring`, submit, and expect `workers_wait()` to contain `iou-wrk-<that thread's id> (wchan io_wq_worker)`.
  - It needs io_uring and no driver, like your ring test. In my check the worker was there as soon as the submit returned.
  - The test harness's other threads may add workers, so check with `contains`.

### Plants, all at b35f72f

One at a time, each with the tests of `skyzfs-ublk-sys` with `control-test-program`. D4 to D8 are in the table in section 2.

| plant | result |
|---|---|
| C7: the start thread's id never sent | **nothing fails**: only a kernel shows it, and testing's runs show the id sent |
| C8: `waits_in` reads `stat`, not `syscall` | `a_threads_wait_is_named_by_its_system_call` fails |
| C9: after `taken` the tag is still watched | `a_first_fetch_with_no_buffer_that_is_taken_or_aborted_is_said` fails |
| D1: every `iou-` thread taken for a worker | `the_io_uring_workers_are_named_with_their_wait` fails |
| D2: `workers_wait` reads `status`, not `comm` | **nothing fails** (note 2) |
| D3: `workers_wait` always says `none` | **nothing fails** (note 2) |

### Testing's run on 88524f8, all three images

[Run 43585528398](https://github.com/skyzfs/skyzfs/actions/runs/43585528398) has finished. On ubuntu-24.04, 26.04 and 26.04-arm alike, at a start and at a take-over, it reads: the fetch `refused: errno 22`, then `no answer after 10000 ms; its thread waits in io_uring_enter (wchan io_cqring_wait)`. No run of this head's workers line was there yet when I wrote this.

### The library's lock (your point 5)

Noted. I have not read M1.3ah's code; I will check its delete when it comes for review.

### CI on b35f72f, when I wrote this

- **Green:**
  - the control rig on all three images ([43590456720](https://github.com/skyzfs/skyzfs/actions/runs/43590456720));
  - the replay rig's "write into a hole" on 26.04;
  - both table jobs;
  - ci.yml's lint, chart-lint and s3-backends;
  - e2e-tier-a.
- **Still running:**
  - the daemon rig on all three images ([43590456770](https://github.com/skyzfs/skyzfs/actions/runs/43590456770));
  - the replay rig's daemon jobs ([43590456757](https://github.com/skyzfs/skyzfs/actions/runs/43590456757));
  - ci.yml's test and unit-arm64;
  - coverage and the k3s smokes.
- **Codex** reviewed b35f72f, code and security, with no finding.

---
_Generated by [Claude Code](https://claude.ai/code)_

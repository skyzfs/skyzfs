## Second look at a7c20ab and 75c32a7

**It can merge. Nothing blocks any more.** Findings 1 and 2 are settled: each planted breach now fails the test you name. Finding 3's fix holds on one CPU. The merge of #70 carries `settle_at_failure` where it belongs. 75c32a7 settles Codex's open thread.

### The blocking findings, planted again at a7c20ab

My own plants, each alone, `cargo test -p skyzfs-local --lib` (and `skyzfs-ublk` and `skyzfsd` with its hooks for S1):

| plant | at ffde7bf | at a7c20ab |
|---|---|---|
| S1: each I/O thread wakes the owner for its first result only | nothing fails | `ported::the_waker_is_called_for_what_the_threads_make_ready` fails |
| S2: `block <= 2 * unit` | nothing fails | `ported::create_and_open_hold_the_system_to_the_write_unit` fails |
| S4: `fail()` tells the observer without taking it | nothing fails | `ported::the_failure_is_told_before_the_marker` fails |
| P7b (finding 5): the refusal for a home image of another size writes the marker | nothing fails | `the_open_refuses_a_volume_that_does_not_hold_together` fails |
| P24 (finding 7, new): a volume file that names another block size is taken | (no check) | `volume::tests::a_damaged_or_unknown_file_is_refused_by_reason` fails |

The new tests ask the right things:
- **The waker test** runs four writes on one I/O thread as well as on eight. That is what makes a thread that wakes the owner once fail it, since on eight threads four writes can each be some thread's first.
- **The write-unit test** asks for each refusal's fields, at the create and at the open, for blocks of 2×, 4× and 256× the unit and for pages of 2× and 4×.
- **The failure test**'s new way, two writes failing together, does reach `fail()` twice, and every way is followed by a request that fails.

### Finding 3: the early-acknowledged write held back

Right, by reading, and on the scheduler. At a7c20ab, unplanted, `the_suite_notices_the_defects` passed 8 of 8 runs alone, 8 of 8 on one CPU (`taskset -c 0`), and 8 of 8 runs of the whole library suite. At 45b67f2 it failed 7 of 12 on one CPU, each time on `ACK_BEFORE_WRITE` not noticed by `kill_keeps_acknowledged_operations`.
- The hold acts only under the flag: `held` is set only in the `ACK_BEFORE_WRITE` arm of `on_thread`, and `release_early` returns at once on an empty list.
- A held write keeps its key in `busy` with its range, so a later request that overlaps it waits for it as before.
- Every `submit`, `poll` and `wait` hands over the writes whose acknowledgement has been taken, and the wait's loop does too, where nothing is ready. So nothing waits on a held write that no call would release.
- The drop and `finish_threads` hand over all of them on each turn of their loops. A failure drops them and takes their keys out of `busy` and `acked_early`, so neither loop waits on a write no thread will begin. `abandon` clears them, which is what makes the kill meet the write unbegun.

### The merge of #70 (9fdf3d1)

Right. `settle_at_failure` goes through the create's second start (the judged target), the abandoning kill, the power cut and the reopen. The create's first, plain start never settles, which is right, since it is not judged. `finish_threads` is #70's head, panic included, and also hands over held writes on each turn, as the drop does. #70 merging first should leave nothing to resolve. It also settles finding 4: the test passes alone now (8 of 8 above), where at 45b67f2 it failed 12 of 12 on SYNC_RETRIED.

### 75c32a7 (Codex's open thread)

Right. I had checked the thread against a7c20ab, and it held: `serve.rs:676` read and checked the record unconditionally, `ack::read` refuses a malformed one, and nothing wrote a failure note. Now a volume that holds to no position gets an empty record, unread (`AckFile::reset`), and the in-place daemon never writes positions into it (`Positions for InPlace` gives none).

**Not blocking:** `a_damaged_record_of_what_was_acknowledged_does_not_stop_an_in_place_start` tests `open_ack` itself. The call site passes `volume.reaches_positions()`, and a `true` there would pass every test. One line in `run`, and M1.3t-b removes the record anyway, so I leave it at a word.

### Also settled

- 6 (`format.rs` in the table), 8 (the message names another directory or removing the files) and 9 (the stale words) are as I asked.
- The check of the home image's own file system against the unit is still untested. It was so at the base, and a test needs a block size per file in the simulated disk. I would not hold this pull request for it; whether it goes here or later is the lead's call.

### CI at 75c32a7

- **Every job is green.**
- **The daemon rig (run 43549273948).**
  - The three tables are the same, line for line: on 24.04, 26.04 and 26.04-arm.
  - They are also the same as a7c20ab's tables on 26.04 and 26.04-arm. a7c20ab's 24.04 job was cancelled by the push.
  - Pins and controls: 47 PASS, 13 NA, 1 WAITING (`pin_failed_volume_while_a_request_is_held`), and the five controls FAIL. The suite's cases: 1 PASS, 13 WEAKER, 12 NOT_EXERCISED.
  - No line differs from `EXPECTED`, and there is no ERROR. The NA lines are the ones the table names for the in-place store.
  - I read each job's `results.tsv` from its log and ran `daemon_check.py` on it here. The step summary itself is not in the log.
- **The replay rig through skyzfsd on 26.04.** It checked every mark (27 fsync, 9 sync), 80 of the 81 flush boundaries and the whole log, and deep-checked the first and last checked prefix of each transaction group. Both controls were caught as expected. Exit 0.
- **Codex** has posted nothing after its thread at a7c20ab.

---
_Generated by [Claude Code](https://claude.ai/code)_

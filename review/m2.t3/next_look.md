## Next look at #99 at 6e0ecd8

**Verdict: the review says it can merge.** Every finding is answered in the code, and I found nothing new that blocks.

One test gap is left, not blocking: no test has a control that differs from its table (below).

### What I checked

- **The tests pass here:** `test_cut_daemon_check` 24, `test_cut_daemon_main` 8, `test_cut_ref` 32, `test_cut_check` 26.
- **The 40 plants of the first look again** (`review/m2.t3/plants_next.out` on `skyzfs-review`):
  - 30 are caught;
  - 6 no longer apply, their code having changed;
  - 4 pass: the three guards your answer names as not done, and a missed control that does not fail the check.
- **17 new plants in the code the answer changed** (`plants_next2.py`, `plants_next2.out`): the status, the requests the check cannot place, the three new lists, the rule controls and the table. 13 are caught, 4 pass (below).
- **The rig on this head** (run 43680211986, both images). In both jobs' logs, each of the ten recordings has `cuts`, `status`, `sessions` and `import_syncs_before_return` PASS, and every control is as the table says:
  - `txg-alone` is caught on rewind and rewindf in both;
  - `count-repeats` on multihost in both, and on stray on arm;
  - `previous` on multihost on arm.

### By finding

- **S1.** `last` and `extra` fail by the count alone and `late` by the instant alone, and with either comparison planted out a test fails. The rule controls and their table are as I suggested.
- **S2.** Taken in. Each plant in `main` that sets the exit code is caught now, but the one below.
- **S3.** The values decide, and the time only words the reason. The latest time is right: the read's end less `age_ms`, plus the clock's 10 ms.
- **S4.** A failed `cuts` beside a `fua_flush` barrier or a write-zeroes over a label is the rig's error with exit 2, and a test holds both.
- **S5.** Both clauses have a test: with either planted out, a test fails.

### What is left (does not block)

**A control that differs from its table has no test.** (`cut_daemon_check.py`, the loop over `CONTROLS` in `main`.)

- Two plants pass every test: a control that differs does not fail the check, and a row with two allowed verdicts allows any.
- The code is right. I ran the check on the fixture `life` under the name `rewind`, whose row expects `txg-alone` caught (`review/m2.t3/table_differs.py`). It ends with 1, and the line reads `ctl_txg-alone: SAME ... (expected: caught; DIFFERS)`.
- That run as a test would hold "in either direction".

The other two plants that pass change no verdict on a right daemon:

- a `cuts` line that passes is made the rig's error where the log holds a write-zeroes over a label (a false alarm only, and no recording has one);
- a recording with no row of the table is the rig's error either way.

### CI on the head

- The daemon's cut rig is green on both images.
- The dm-log-writes cut rig (43680212024) was still running when I looked.
- The replay rig's and the daemon rig's jobs are cancelled on this head. Your comment on the pull request says why. Whether a cancelled job counts as green is the lead's to say.

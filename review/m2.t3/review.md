## Review of #99 (M2.T3, first stage: the cut rig on the daemon) at 2c67c42

**Verdict: nothing here blocks the merge.** The check reads the daemon's log in the order the daemon counts in, and I found no sequence ZFS issues in which it would pass a wrong count or fail a right one.

What remains, none of it blocking:

- **S1 (your question 3):** the four controls do not exercise two of the things `cuts` compares, the count and the instant.
- **S2:** `main` has no test. `review/m2.t3/test_main.py` on `skyzfs-review` has seven that pass at the head.
- **S3 (your question 2):** the status's window allows 1 ms, and the status's clock ticks every 10 ms. It holds on the runners for a reason the code does not state.
- **S4:** Codex's two findings are right by the code, and no recording reaches either.
- **S5:** two clauses of the receipt rule in `cut_ref.cuts` have no test.

On 2c67c42 the rig passed on ubuntu-26.04 and ubuntu-26.04-arm. In both jobs' logs, each of the ten recordings has `cuts`, `status`, `sessions` and `import_syncs_before_return` PASS and the four controls CAUGHT. The dm-log-writes cut rig was still running when I wrote this.

### How I checked

- **The code first:** `cut_daemon_check.py`, the change to `cut_ref.cuts`, `cut-record.sh` and the workflow, against design §5.2, `record.rs`, the pump's turn and the store's `accept`.
- **The tests pass here:** `test_cut_daemon_check` 18, `test_cut_ref` 32, `test_cut_check` 26.
- **Planted changes.** 40, one at a time, in the check and in the receipt rule (`review/m2.t3/plants.py`, `plants.out`). The tests caught 21. Of the 19 that passed:
  - ten are noticed by `test_main.py` and one by `test_receipt.py` (`plants_main.out`);
  - the other eight are in S2, S5 and the small remarks.
- **The fixtures through the daemon check's path, by each rule of the reference** (`rules.py`, `rules.out`).

### Your three questions

**1. `records_of`.** Right for what ZFS issues.

- **A write at `applied`, a flush's receipt at `recv`.** The `recv` lines of a wait are written when the wait returns. The pump then submits every fetched request before it polls the store (`pump.rs`, `turn`), and the store's `submit` collects nothing from its threads. So no `applied` line can fall between a flush's `recv` line and the store's own receipt of it, and the two are the same point in the order. This rests on the store taking everything a turn fetched, which holds while its depth is the device's (both 64 as the rig runs it).
- **The receipt takes the mark, the barrier counts.** As `cuts.rs` does, the repeat of a carried mark included.
- **Discards ignored.** Right: the store's discard changes no content.
- **FUA and write-zeroes.** See S4.
- **The marks.** Both clocks are `CLOCK_MONOTONIC`. A group synced between the kstat's read and the end mark's stamp is left out by the kstat's own range (`session_verdicts`), so the tens of milliseconds a python process takes to start do not matter.

Questions 2 and 3 are S3 and S1.

### Findings

**S1. The controls do not exercise the count or the instant.** (`cut_daemon_check.py:215`, `compare`, and line 247, `mutants`. Confidence: high, run here.)

- `compare` holds each cut's instant, group and answer, and then the two lists' length. With the length not compared, and separately with the instant not compared, every unit test passes, and on the fixture `life` all four controls still read `caught`.
  - `drop` removes the middle cut. The lists then differ cut by cut from there on, so the length is never what catches it. A daemon that misses its last cut, or counts one more after it, is seen by the length alone.
  - `early` moves each cut's instant. At the earlier instant the reference also gives another answer to `itself`, and that is what catches it.
- Three more lists would exercise both: the last cut dropped, a cut added, and one cut at the barrier after its own with its group and answer kept.
- **The rules.** The wrong counts the repeat rule exists for are a repeat counted and a repeat keyed by its group alone. `cut_ref.cuts` has both rules, and the dm-log-writes check carries them as controls with a table by recording. Through the daemon check's path, on the fixtures:

  | Rule | Differs from the design's on |
  |---|---|
  | `count-repeats` | multihost (108 cuts for 93), stray (30 for 17) |
  | `txg-alone` | multihost (91 for 93), rewind (31 for 32), rewindf (28 for 29) |
  | `previous` | multihost |

  With those as controls, each of those recordings shows that the comparison would see that kind of daemon.

**S2. `main` has no test.** (`cut_daemon_check.py:311`. Confidence: high, run here.)

- Eight plants in `main` pass every test:
  - a failed `cuts`, `status`, `sessions` or `cut_numbers` line that does not fail the check;
  - a missed control that does not;
  - the rig's own error ending with 1;
  - a second instance read as the first;
  - the `early` control computed by the design's rule.
- On a runner these branches run only when something is wrong, so a green run does not exercise them.
- `test_main.py` writes a recording's directory from the fixture `life` (the log, the marks, a status and its times, each session's kstat from the reference's own cuts) and runs the check as `cut-record.sh` does. Its seven tests pass at the head:
  - a right daemon ends with 0 and four controls caught;
  - a cut not counted (a middle one, the last one), a cut counted at the flush after its own, a status one cut behind, a kstat with a group the daemon did not cut, and wrong cut numbers each end with 1;
  - a second instance ends with 2.
- It notices ten of the nineteen plants that passed, the two of S1 among them. It does not notice a missed control that does not fail the check: no control is missed at the head.

**S3. The status's window allows 1 ms, and the status's clock ticks every 10 ms.** (`cut_daemon_check.py:273`, `status_window`; `skyzfs-ublk/src/status.rs:572`. Confidence: high on the arithmetic; I have not seen it fail.)

- `age_ms` is `/proc/uptime` now less `/proc/uptime` when the file was written. `/proc/uptime` has two decimals, so each is a multiple of 10 ms and the true age lies within 10 ms either side of `age_ms`.
- `status_window` takes 1 ms off, and `SLACK_NS` is 2 ms. So `hi` can lie up to 10 ms before the time the status was written.
- **A right status that could fail:** written within about 8 ms after the last `cut` line, it reads as "written before the daemon's last cut line" if `after` is stamped right at the read.
- **Why it does not on the runners:** `after` is stamped by a python process that starts after `status` has exited, which takes longer than 10 ms. `hi` is late by more than the clock's error. Nothing in the code says the margin rests on that.
- **A stale status always fails,** by the window or else by its values, since its count is short. So the window never changes a verdict for the better: it words a failure, and it can turn a right status into a FAIL.
- Either compare the values first and use the window only to say why they differ, or allow the clock's 10 ms. `lo` is computed and never read.

**S4. Codex's two findings are right, and no recording reaches them.** (`cut_daemon_check.py:157` and `:189`. Confidence: high from the code. I had reached both before I read them.)

- **`fua_flush`.** The store receives the added flush, and takes the mark, when the pump submits it after the write completed. Its barrier waits for the writes in flight. An uberblock applied between the two is not that flush's in the daemon, and is in the check.
- **Write-zeroes.** One that overlaps a write in flight waits behind it, and the log has no line for when it was applied.
- ZFS issues no write with FUA and never zeroes a label, and all twenty recordings on the head pass.
- Either could fail a right daemon and neither could pass a wrong one. Short of a new line from the daemon, the check could make them its own error: on a `cuts` mismatch in a log that holds a `fua_flush` barrier or a write-zeroes over a ring, exit 2 and say which.

**S5. Two clauses of the receipt rule have no test.** (`cut_ref.py:180` to `:217`. Confidence: high, run here.)

- **An uberblock that repeats a carried mark marks nothing.** Without the clause every test passes. `review/m2.t3/test_receipt.py` passes at the head and fails without it: a stray flush received after the first copy of a group's uberblock was applied, with its barrier after the second. Without the clause the group is cut twice.
- **The mark made since the receipt outlives the barrier** (`mark = kept`). Without it every test passes too. In the one test that has such a mark, the next copy of the same uberblock makes it again. A flush whose barrier comes after the last copy would show it.

### Small remarks

- **Your open question, the cut lines after `session-2-end`.** `export_pool` writes the end mark before it exports (`cut-record.sh:158`), and the export syncs a few more groups. Those cut lines are the export's, not a late log.
- **Guards with no test:** a cut line after another flush's barrier, a label line of another length, a barrier of a flush never received, a flush received twice, a mark stamped at a line's own time, and the slack itself.
- **The marks' places** are tested only by the order of two names. `test_main.py` notices marks that all come after the last line.

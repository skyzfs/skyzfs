## Review of #111 at 8210332 (M2.T3 stage 2b)

**Verdict: it can merge. Nothing I found blocks it.** The reference's `instance` record is the design's rule and the daemon's, the takeover recording does what its comment says, and the three runs I could read (two images of the pull request's run, one of the push's) give the same verdicts when I run the judge on their artifacts here. There is one defect, D1: `lost_at_kill` excuses a case that a kill cannot produce, and the two cases a kill can produce are judged against a right daemon (exit 1). It needs a kill inside a window some microseconds wide, about one run in several thousand, so I would not hold the merge for it; but the description's sentence about it should not go into the notes as it stands. The rest is gaps in the tests: 39 of my 62 plants are caught, and the survivors are listed below with what each one means.

I formed this from the diff before I read the description. Codex has no findings on this head.

### The five questions

**(1) The `instance` record and the stored mark. It matches the design, `take_up_cut_state` and the freeze.**

- **Set when a mark is made** (`cut_ref.py:255`), as `record_cuts` stores the mark when the store has applied the write (`inplace.rs:2727`).
- **Cleared when a cut of its state freezes** (`cut_ref.py:206`), as `freeze` clears the mark only if it equals the cut's state (`state.rs`, the end of `freeze`). A flush received with group 5's mark that reaches its barrier after group 6's uberblock leaves 6's mark stored: the reference does that, and I ran it (`scenarios.out`, line 1: cuts of 5 and of 6 across a take-over).
- **Kept at a take-over, dropped on a created device** (`cut_ref.py:215-223`), as `take_up_cut_state` does (`inplace.rs:2329`). The flushes received and not at their barrier carry nothing on (`carried = {}`), which is right: their cuts were never counted, and the state still holds the last mark made, whether one of those flushes had taken it or not.
- **The newest cut is kept in both cases**, so a repeat of its uberblock on a created device marks nothing (`scenarios.out`, line 2). The state keeps `newest` too.
- **Both controls are caught in every run I read**: `mark-dropped` at the cut after course (b)'s kill, `mark-kept` at the cut after course (c)'s import.

Two parts of this have no test: see R7 and R10 under the gaps.

**(2) `records_of` at an instance boundary. Right for what the recordings hold.**

A `barrier` line is written at the barrier, before anything behind the flush runs, so a flush with no barrier line had not let anything past it, and the kernel hands it and what it had not answered to the next instance. Dropping `received`, `pending`, `data` and `zeroing` there (`cut_daemon_check.py:322-335`) is that. Three limits, none of them met by today's recordings:

- **The test does not hold the clears** (plants C2, C3, C4 survive): `test_what_the_first_instance_did_not_finish` gives the second instance the same tags as the first, so the dictionaries are overwritten whether they were cleared or not. With other tags in the second instance, which is what the kernel may do, the test would hold them.
- **A write in flight at the kill that reached the disk is placed only where the next instance applies it.** A cut counted by the next instance before that line then reads the block as the killed instance left it, and the judge wants the older content: `held_content` FAIL for a right daemon (`scenarios.out`, line 3, a hand-built log). It needs a flush that reaches its barrier before a write handed over with it. In the takeover recording ZFS does not do that: it waits for a group's writes before it flushes, and nothing there writes synchronously. A recording with `fsync` under a kill could. I am sure of the judge's answer and only reasoning about ZFS.
- **A write-zeroes placed at its receipt and not answered at the kill stays placed.** No recording has one before a kill.

**(3) `lost_at_kill`. No: see D1.** It is the rig's error for a log a kill does not produce, and not for the logs a kill does produce.

**(4) `noflush_kstats`. Acceptable, with what it costs said plainly.**

The groups left out come from the reference (`cut_daemon_check.py:776`), `cuts` holds the daemon to the reference, and both controls must be caught, so a daemon that cut one of them or missed the one it should cut fails `cuts`. What is given up: for the groups between the marks, `sessions` no longer checks the reference against ZFS's own account. That is the right trade for groups ZFS committed with no flush, which have no cut by definition. Only groups whose uberblock write lies strictly between the two marks are left out, and the recording puts `noflush-end` after the next start, so the export's groups in course (c) are inside. Each run left out three groups (398, 402 and 411 in the push's), all of course (c): its sync's and the export's. Course (b)'s one group is cut after the take-over by the kept mark, so it stays in `sessions` (`noflush_groups.out`).

The span's bounds are held by no test (N2, N3 survive), nor is main's use of the function (N5).

**(5) The three courses. Right as far as I can read them; I cannot run them here.**

- **(a)** The holder leaves, a writer writes and syncs, `kill -9` after 3 s, serve 2 takes the device over, the writer must end with exit 0 across it. In the three runs 31, 155 and 109 cuts were counted in the 3 s before the kill, so the kill does fall under load.
- **(b)** A sync with `zfs_nocacheflush` leaves a mark waiting on an idle pool, `kill -9`, serve 3 takes over, flushing is turned on again. The first flush cuts the kept mark: that is what catches `mark-dropped`.
- **(c)** A mark waiting again, export (its groups have no flush either), SIGTERM with exit 0 required, `detach`, serve 4 creates the device, import. The first cut after it is of the import's group and not of the mark: that is what catches `mark-kept`.
- `instances.tsv` must read added, taken over, taken over, added, or the recording is the rig's error (`cut-record.sh:760`). The cleanup sets `zfs_nocacheflush` back.
- `status-start-K.txt` is this instance's status and not a stale file of the one before: `serve` prints `ready` only after its own first status write was tried (`serve.rs`, `first_write_tried`).

What the courses show about held cuts depends on chance more than the judge admits: see G1.

### D1 (defect; does not block). `lost_at_kill` is on the wrong side of the kill

`cut_daemon_check.py:704-720`, used at `:986-989`; pinned by `test_a_cut_counted_and_its_line_lost_at_the_kill` (`test_cut_daemon_check.py:949`, with `split_log(lose_line=True)` at `:877`).

**What it assumes.** That the daemon can count a cut in its state and be killed before the cut's line is written, so the next instance counts on from K+1 and the log has no line K.

**What the store does.** At a flush's barrier it runs `self.cuts.barrier(id); self.record_cuts();` (`inplace.rs:1713-1714`). The first call writes the `barrier` line, then counts in the rule and writes the `cut` line (`cuts.rs:467`, `:527-548`). Only the second call stores the count, the newest cut and the cleared mark in the mapped state (`freeze`). Each log line is one `write` at the event. So the lines are on the file before the state knows of the cut, and there are two windows, both the other way round:

| | Killed | The log | The next instance | The judge today |
|---|---|---|---|---|
| A | (cannot happen) | barrier, no cut line; next cut is K+1 | | exit 2, "killed between counting cut K and writing its line" |
| B | after the `barrier` line, before the `cut` line | barrier, no cut line | still has the mark, gets the flush again, cuts it as K | exit 1: `cuts` FAIL, "cut K: the daemon's is group G at record 734, the reference's group G at record 731" (the fixture's numbers) |
| C | after the `cut` line, before the state's stores | cut line K | the same: a second cut line K | exit 1: `cuts`, `cut_numbers` and `instance_2` FAIL |

In B and C the daemon is right by design §5.2: the cut was not counted, the mark is left to the next instance, the flush was not answered and is cut when it comes again. The reference disagrees because it clears `stored` at the old instance's barrier record.

**How I know.**
- `kill_between.out`: two tests added to the store's own (`kill_between.rs`, run by `kill_between.py`, which restores the file). After the barrier line, and after the cut line, a new instance on the same state starts at the count before the cut with the mark kept, and handed the flush again it counts the same number and group.
- `kill_windows.out`: the test fixture cut at cut 13 into two instances and run through `main` as `takeover`, in the three shapes of the table. The exits and lines above are from it.
- `window_width.out`: from the three runs' logs, the `barrier` line to the `cut` line is 4.4 to 4.5 µs at the median, and with the cuts counted in the 3 s before the kill that is one kill in about 20,700, 4,200 and 6,000. C's window is not in the log; it is stores to memory, plus removing files when an older cut gives its map up, so it is of the same order or somewhat more.

I am sure of the order and of the judge's answers. The rate is an estimate from three runs.

**What it costs.** About once in some thousands of runs the takeover recording will fail with a verdict against a daemon that did nothing wrong, with a message that points at the daemon. And the exit-2 path is kept for a log that no kill gives: if it ever fires, the cause is something else (a log line that failed to be written, or a daemon that counts before it says so), and the message names the wrong one. Neither weakens a green run.

**A fix.** At an instance boundary, a `barrier` line with no `cut` line for its flush and no store line after it means the cut was not counted (B): the reference should treat that flush as it treats one with no barrier. A `cut` line K followed in the next instance by another line K, or by a `status-start` count of K-1, means the same (C): the first line is not a cut. Shape A's message should then not speak of a kill; whether it stays the rig's error is the author's to weigh. The description's sentence ("a cut counted and killed before its line was written ... the next instance's numbers say so") should be put right with it, since the notes are written from it.

**The tests around it.** Plants K2 to K6 survive: the next instance's number is not looked at, the extra cut may be at any barrier, any number of cuts may be missing, and main may skip `lost_at_kill` or ignore its answer. Only "the function returns a message for shape A" is held (K1). The description says this path is shown by unit tests only; they show less than that.

### G1 (gap). Nothing requires that the takeover recording carried a held cut across a start

`held_lines` judges the cuts written, whichever instance counted them, and `cut_hold.py`'s leave (`:239`) hands on whatever the holder has in hand at that moment. If it has nothing, or a cut none of whose blocks is overwritten before the next holder writes it, the recording is green and shows nothing about copies surviving a start.

What the three runs do show (`held_across.out`, `copies_lost_at_start.out`):

| Run | Cuts written across a start | Blocks of them read from a copy an earlier instance made |
|---|---|---|
| pull request, 26.04 | 7, 44, 49 | 20, 0, 0 |
| pull request, 26.04-arm | 9, 174, 180 | 69, 0, 0 |
| push, 26.04 | 8, 131, 136 | 28, 0, 0 |

So course (a) shows it, and a daemon that lost its copies at a start is caught there (`held_content` FAIL, exit 1, on all three). Courses (b) and (c) show it for no block: the pool is idle, or writes too little, between the leave and the next holder, so no block of the held cut is written again.

**Suggestions.** A judged line for `takeover` with a floor, as `edges` has for its write-zeroes: at least one cut written by a later instance's holder than the instance that counted it, with at least one block from an earlier instance's copy. And in (c), enough groups synced with the holder gone that the held cut's uberblock slots come round again (33 or more), so that the created device's start is shown with copies too. The second is my reading of where the blocks would come from, not something I ran.

Beside it, plant H4 survives: `main` of the holder could drain its cuts on a leave instead of ending, and no test would say so. The recording would show it only through this same missing line.

### Gaps in the tests (none blocks)

Plants are in `plants.py`, their results in `plants.out`: 39 KILLED, 23 SURVIVED of 62. Two of the kills are by a hang that the script's limit ends (H1, H6), which is a fair kill. The survivors not named above:

| Plant | What survives | A test that would hold it |
|---|---|---|
| R7 | any cut clears the stored mark, whatever its state | line 1 of `scenarios.out` as a case of `Instances` |
| R10 | a created device forgets the newest cut | line 2 of `scenarios.out` |
| C9 | an instance that starts before the last line of the one before is no error | a log with two pids interleaved |
| I3, I8 | a later instance's `cuts` line may say `begun` | `test_instance_lines` with such a line in instance 2 |
| I7, N5 | `main` drops what `instance_lines` or `noflush_kstats` returns | a main test with a failing instance line; one with a no-flush group |
| I10 | several instances with no holder are no error | a main test |
| I11 | a held stage on a log with no hashes is judged | a main test |
| N2, N3 | every group the reference does not cut is left out, in a span or not; a span with no end reaches the end | a group outside the marks that is not cut; a `noflush-begin` alone |
| B4, B5 | `skz_release_line` keeps the last line's blanks; the snapshot does not call it | small, the bench's |

I12 is not a gap: the plant is equivalent on any log the other lines accept.

### CI on this head

- Cut rig on the daemon: green on ubuntu-26.04 and ubuntu-26.04-arm (run 44156993103) and on the push (44154951477). I ran the judge here on all 39 recordings of those three artifacts: exit 0 and the same lines as the runs (`rejudge.out`).
- Cut rig on dm-log-writes (four images; run 44156993188), node B's rig, lint, test, unit-arm64, e2e-tier-a, coverage: green.
- The daemon rig, the replay rig, the control rig and the kernel probe were cancelled on this head by the author, who says so on the pull request. I checked their reason: none of those four runs a file this pull request changes. Whether a cancelled run counts for the merge rule is the lead's to say.

### What I could only read

- `cut-record.sh`'s takeover course and the workflow change: no kernel, ublk or ZFS here. I read them against the three runs' artifacts.
- shellcheck is not installed here; CI's lint ran it.
- The width of window C in D1.
- That ZFS never sends a flush ahead of a write still in flight (question 2, second limit).

### Evidence in this directory

`plants.py`, `plants.out`; `kill_between.rs`, `kill_between.py`, `kill_between.out`; `kill_windows.py`, `kill_windows.out`; `window_width.py`, `window_width.out`; `held_across.py`, `held_across.out`; `copies_lost_at_start.py`, `copies_lost_at_start.out`; `rejudge.py`, `rejudge.out`; `noflush_groups.py`, `noflush_groups.out`; `scenarios.py`, `scenarios.out`. The scripts that read artifacts take the artifact directories as arguments; the artifacts are not kept here.

Review of #116 at 49c5f62 (M2.T3 2e)

**Verdict: at 49c5f62 it cannot merge, and one finding blocks (B1). The head moved to 92256f8 while I was writing; that one commit answers B1, and I found nothing else that blocks.** At 92256f8 it can merge from my side once the reference cut rig's run on that head (44237459703, still running when I looked) is green and Codex's P2 has an answer. Two defects in the check (D1, D2) do not block, because no course of the rig reaches them; they should be fixed or recorded as known.

B1 first, because part of it is mine. The cut rig on the daemon was red on 49c5f62 on both images: `takeover` exit 2, `held_across_start: ERROR` (run 44233560555). The cause is S1's wait, and S1's number came from my review of #115: I wrote "a loop that ends 40 groups after the holder left", where the ring needs 31 cuts, which are 93 groups under this writer. The author implemented what I wrote. They found the cause themselves before this review reached them and pushed 92256f8. I had the same reading independently, from the recordings (ring.out), so the two agree.

Everything below was done on 49c5f62 unless it says 92256f8. The check (`cut_daemon_check.py`) and its tests are the same at both heads; 92256f8 changes 13 lines of `cut-record.sh` only.

## The four questions

**1. R1 (window C at whatever group): it judges as much as before, and two right daemons that failed before now pass.** I built seven logs around a kill on a cut's flush (windows.py, on the fixture `life`) and judged them at the base and at the head:

| Log | The daemon | Base 5edf461 | Head |
|---|---|---|---|
| Ca: not stored, taken over, cuts K at the same flush | right | passes | passes |
| Cb: not stored, taken over, cuts nothing at the flush handed again, K at the next group | wrong | FAIL (cut_numbers) | FAIL (cuts, against the reference) |
| Cc: not stored, the next start created the device, K at the next group | right | FAIL | passes |
| Cd: stored, then K+1 | right | passes | passes |
| Ce: stored, and K counted again at the next group after a take-over | wrong | FAIL | FAIL |
| Cg: as Ca, the next instance killed after the flush's receipt, a third created the device | right | FAIL | passes |
| Ch: as Cg, the third took the device over and cut nothing at the flush | wrong | FAIL | FAIL |

So every wrong log I could build still fails; the line that fails moved from `cut_numbers` to `cuts`, and the reference now says where the cut should have fallen, which is a better message. I found no right daemon that fails now and passed before.

One log passes now that failed before, and it cannot be told apart: a daemon that stored cut K and then lost it at a start that created the device leaves Cc's log line for line. The flush was never answered, so nothing was promised, and the same was already true at the base for a take-over within the group. I do not count it as a loss. The ten plants on R1's lines are all killed, among them the restored same-group condition (by `test_a_cut_line_that_is_not_cut_again`).

**2. Window M: "applied again before any cut line" is close to the right test but not it. It fails a right daemon in one order of events that passed at the base (D1 below). It does not block: the order needs a flush under way at the kill, which `takeover`'s load does not produce.**

What the daemon does (crates/skyzfs-local/src/inplace.rs and inplace/cuts.rs): a write's completion writes the `applied` line, stores the mark, and answers. A flush takes the mark out of the state at its receipt (cuts.rs:430) and counts the cut at its barrier. A flush waits for every write on the threads (`must_wait`, inplace.rs:1836-1860), and a request received behind a waiting one waits too (`accept`, 1864-1876). So the right test is: the mark was stored exactly when a flush that the next instance received before the write's second `applied` line cuts at its barrier. The head's test agrees with that only when the flush is received before the write is received again. Results:

| Log (W the marking write, F another flush under way at the kill) | The daemon | Base | Head | With prototype.diff |
|---|---|---|---|---|
| Ma: mark not stored; F handed first, no cut; W applied again | right | FAIL | passes | passes |
| Mb: mark stored; F handed first, cuts K | right | passes | passes | passes |
| Mc: mark stored; W handed first, F received while W is on the threads; F cuts K after W's second `applied` line | right | passes | **FAIL** | passes |
| Md: as Mc, mark not stored | right | FAIL | passes | passes |
| Me: the state holds an older group's mark (two groups with no flush between); F cuts the older group | right | FAIL | FAIL | FAIL |
| Mf: as Me with no F | right | passes | passes | passes |
| Mg: as Ma, the next instance killed before it applied W again | right | FAIL | FAIL | FAIL |
| Mh: the next start created the device | right | passes | passes | passes |

The head is better than the base (two false FAILs gone, one new). A wrong daemon that passes: one that stores the mark and drops it at the take-over, killed inside this window, leaves Ma's log (Mw); the status has no line for the mark, so nothing in the recording tells them apart. Course (b) of `takeover` holds that defect outside the window, so I count it as a limit and not a finding.

**3. S2 at created starts too: yes, that is right.** A start that creates the device takes the state up and drops only the mark (`take_up_cut_state`, inplace.rs:2380); the held cuts stay (`settle`, state.rs). All twelve take-over recordings I hold (six runs, both images) have `cuts_held 1` at the created fourth start, and 2 and then 1 or 2 at the two take-overs before it. Two things to know. No test holds it: with S2 limited to take-overs every test still passes (plant S2e). And a newest cut that the daemon abandons at its bound would rightly leave 0; `takeover` does not reach the bound, and S2 does not read `cuts_abandoned`, so a course that did would get a false FAIL.

**4. G3 on `held_lines`: the claim is true with the test helper as it is, but the reason given holds for `failed` only, and the test leaves one link open.** For `failed`, a floor's zero does break `held_content`, as the description says (floors_main-49c5f62.out). For `takeover` the floor can fail alone: what keeps the fixture from exit 2 is `ctl_mark-kept` reading DIFFERS (exit 1), because the fixture has no created start with a mark waiting. The open link: `main` passes `held_lines`' ERROR flag on at cut_daemon_check.py:1585 and 1594 and turns it into exit 2 at 1660, and dropping the flag at either call survives every test (plants G3e, G3f). `test_a_failed_cuts_with_what_the_check_cannot_place` holds the exit through main's own `cuts` ERROR and not through the floor's. The red run on 49c5f62 happens to show the whole chain on a real recording (exit 2 with only the floor's ERROR), so I am confident the code is right today; a test would keep it so. It needs either main run in the test's process with `held_lines` replaced, or a fixture with a created start and a mark waiting.

## Findings

### Blocks the merge

**B1. S1's wait counts the wrong thing, and the head's own run fails on it. Certain; closed at 92256f8 as far as a run and a reading show.**
tests/kernel/cut-record.sh:763-789 at 49c5f62 kills the first instance once 40 groups are committed after the holder left. The floor `held_across_start` needs the held cut's slot of the uberblock ring written again before the kill. The slot is the group modulo 31, and under this writer only every third group writes an uberblock (`zpool sync` waits for three groups; the eight recordings I ran through ring.py all show cuts three groups apart). So the slot comes round 93 groups after the held cut, not 31, and 40 groups are about 13 cuts.

- Run 44233560555 on 49c5f62: both images `takeover checked exit 2`, the log says "the kill at group 91, 75 after the holder left" (26.04) and "group 73, 57 after" (arm). The held cut is group 17; its slot is first written again by group 110, which came after the second start; `cut_copied_bytes 0` at that start (ring.out).
- The comment at cut-record.sh:777 ("written again 31 groups after it") had the same error.
- The push runs at 076c1f4, before S1's commit, were green, so the description's "to follow" for the head's run did not come true at 49c5f62.

At 92256f8 the wait counts groups with `nwritten > 0` in the kstat after the holder left and wants 40 (cut-record.sh:784-790). I checked it three ways (s1_fix.out):

- Read against the kstat copies of six recordings: the held cut's slot is rewritten by the 32nd written group after the leave in each, so 40 leaves eight spare.
- Run 44237460823 on 92256f8: both images green, 14 of 14 recordings exit 0 on each; the kill came 54 and 45 cuts after the leave, the slot was rewritten before the second start, `cut_copied_bytes 16384`, and `held_across_start` is INFO with 4 blocks on each image.
- The history the count reads is long enough: the script sets `zfs_txg_history` to 2000 (cut-record.sh:245).

One thing remains, as a suggestion and not a blocker. The count is right while the written groups are evenly spaced, at whatever step, since 31 is prime. One written group out of step between the 11th and the 31st after the held cut moves the slot's next write past the 40th: with steps of three and a single step of four after the 11th, the next group in slot 17 is the 42nd written. The result would be the rig's error (exit 2), not a wrong verdict, and I see nothing in this writer that breaks the step (each file is one `dd` and one `zpool sync`, far below the dirty-data limit, and the 5 s timer does not fire at fifty syncs a second). A wait on the slot itself (a written group whose number equals the first written group's after the leave modulo 31), or on `cut_copied_bytes` growing, would not depend on the step at all.

### Defects that do not block

**D1. Window M takes out an `applied` line whose mark was stored, when the write is handed to the next instance before the flush. Certain as a reading of the check and the store; no course reaches it.**
tests/kernel/cut_daemon_check.py:996-1013 (`reapplied`) and 1075-1079. Scenario Mc: the first instance applies the group's first uberblock write W, stores the mark, and is killed before the answer, with another flush F under way. The kernel hands both to the next instance, W first. F is received while W is on the threads and takes the stored mark at its receipt; it waits for W, so its `cut K` line comes after W's second `applied` line. `reapplied` sees the second `applied` line before any cut line, returns true, and the first `applied` line is taken out. The reference then places the mark at the second application, after F's receipt, and wants no cut at F: "cut 13: the daemon's is group 26 (itself) at record 730, the reference's at record 735". Exit 1 on a right daemon. At the base this log passed.

Two more right logs fail at both the base and the head, so they are not regressions: Me (the state held an older mark; the cut at F is of the older group, and telling it needs the cut's group compared with the write's) and Mg (only the very next instance is searched, where R1 now searches every later one).

All of them need a flush under way at the kill together with a group's first uberblock write. ZFS sends its own flush only after the uberblock writes are answered, so F must be a ZIL flush, and `takeover`'s load has no `fsync`. That is also why the window the description aims at (Ma) is itself not reached by the course.

prototype.diff is one way to do it: remember the flushes received before the second `applied` line, and keep the first line only when one of them cuts. It passes Ma to Md and all 207 unit tests; it does not fix Me or Mg.

**D2. S2 compares numbers of two different counts. Certain; Codex's P2 at cut_daemon_check.py:1223 says the same and is right. No course reaches it.**
cut_daemon_check.py:1218-1225. `counted` holds the check's running cut numbers (after `epochs` has carried them across an acknowledged failure), and `gone` holds the holder's release numbers, which are the state's own and begin at 1 again after such a failure. `gone` is also not limited to the state's own lifetime. Scenario (s2_new_state.py, the unit test's own three-instance log): the second start acknowledges a failure, the new state counts cuts 1 and 2, the holder releases both, the third start takes the state up with `cuts_held 0`. The check compares 15 with 2 and says "cut 15 was counted before it and not released": FAIL on a right daemon. `failed` has two starts and `takeover` has no acknowledged failure, so no course has such a start today. prototype.diff compares the state's own numbers and reads only the releases since the state began; with it the right logs pass and the wrong one still fails.

### Gaps and suggestions

- **G3's link through main is untested** (plants G3e, G3f; question 4).
- **S2 at a created start is untested** (plant S2e; question 3).
- **`releases()` has thin tests** (cut_daemon_check.py:1149-1162). Four plants on it survive: a refused release counted as answered (S2h), a line that is not a time and a text taken as the rig's error or as the end of the reading (two plants), and the answer matched to the release asked first whatever tag it carries (S2m). S2 now rests on this function.
- **S2 and an abandoned cut** (question 3): worth a line in the function's comment, or reading `cuts_abandoned`.
- **The description's reason for G3** is right for `failed` and not for `takeover` (question 4). A sentence would do.
- **S1's loop and the `rewindf` dd have no shell test**; `test_cut_record.sh` passes at both heads and does not touch them. Only a rig run shows them, which is how B1 got through.
- Survivors I count as equivalent: M1 (an `applied` line is only ever a write's), R2d, S2g and S2l (the detail text and a filter that D2's fix would replace anyway).

## My earlier items

- **N1** (the log of `test_a_group_synced_with_no_flush`): closed. The test now removes all seven flush lines between the marks and ends the span before the next group's uberblock writes, which is the consistent log I asked for. Plants N5 and O14 to O17 are killed.
- **G2** closed (H8 killed). **G3** closed at `held_lines` (H10, A6 and five new plants killed), with the gap above. **G4** closed (U4, U10, U11, U14, U15, re-planted in the new text where it changed). **G5** closed (H7, L7, T5).
- **R1** closed (question 1). **R2** closed (three plants killed, one equivalent).
- **S1**: implemented as I wrote it, which was wrong (B1); right at 92256f8. **S2**: implemented, with D2. **F2**: implemented, with D1.
- **The 24.04 `rewindf` fix**, read only: the zeroing now uses GNU dd from `tests/lib/gnu-dd.sh` in the device's logical blocks (`blockdev --getss`), with `seek` and `count` in those blocks; a root block of 4 KiB is one block at 4096. On both heads' runs `rewindf` is exit 0 on 26.04 and 26.04-arm. For 24.04 I have only the conclusion of the author's dispatch at 076c1f4 (44230431783, green on four images); I did not read its recordings.

## No earlier test weakened

I ran every older plant list on the head and compared the results with the ones recorded on the heads they were written for (compare.out).

| List | Plants | Killed | Survived | Not applied |
|---|---|---|---|---|
| 2b | 62 | 42 | 1 | 19 |
| 2b-next | 20 | 16 | 0 | 4 |
| 2c | 162 | 120 | 7 | 35 |
| again | 21 | 21 | 0 | 0 |
| head2 | 59 | 40 | 3 | 16 |
| look | 31 | 29 | 1 | 1 |
| new for this head (plants.py) | 69 | 58 | 11 | 0 |

- No plant went from killed to survived. Six went from survived to killed (U11, U14, H8, H10, A6, T5).
- Twenty plants no longer apply because their text changed. I planted them again in the new text (the "again" entries of plants.py, and R1's plants where the old text was the same-group condition); all are killed except the two on `releases()` named above.
- Twelve older survivors survive still; all were reported before.
- The PAGE plants (eight sizes from 1 byte to 64 KiB) are all killed.
- The test lines the diff removes are R1's old expectation, on purpose, and the `cuts_held` additions change expected details only.

## The description's claims

- Re-judging: I judged 56 older recordings with the head's check (two runs of #115's and #113's, both images). All exit 0, verdicts as the runs. `held_across_start` shows 4 blocks and `carried_across_failure` 1775 and 1792 on #113's last run, as the description says.
- The plants table: every plant it names is killed here too.
- The runs at 076c1f4 are green as it says.
- "CI on the head to follow": red at 49c5f62 (B1), green at 92256f8.

## CI and Codex, read at 04:40 UTC

- 92256f8: the cut rig on the daemon (44237460823), `ci`, `e2e` and the push run of `skyzfs-ci` are green. The reference cut rig (44237459703) and `coverage` were still running.
- The author cancelled the daemon rig, the replay rig, the kernel probe and node B's rig on both heads and says so on the pull request. The four files the pull request changes are the cut rig's own, and none of those rigs reads them, apart from node B's use of `cut-record.sh`, whose push run on 49c5f62 was green. Whether cancelled runs are acceptable for the merge is the lead's call; I only note that they are cancelled and not green.
- Codex: one finding, P2 at cut_daemon_check.py:1223, which is D2. It is right and has no answer yet.

## What I could only read, not run

- Nothing with a kernel: the rig itself, S1's loop, the `rewindf` dd, and ZFS's behaviour (three groups a sync, which groups write an uberblock). For those I have the recordings of two runs per head and the code.
- 24.04: no recording read at either head.
- The daemon's order of events in D1 and question 3 is from reading `inplace.rs`, `cuts.rs` and `state.rs` and from the order of lines in real request logs; I did not run the daemon.
- The logs in windows.py and s2_new_state.py are built by hand on a fixture. Each says in its header which daemon would leave it and why.

## Evidence, in review/m2.t3-2e/

- `windows.py` with `windows-49c5f62.out`, `windows-5edf461.out`, `windows-prototype.out`: questions 1 and 2, D1.
- `s2_new_state.py` with its two outputs: D2.
- `prototype.diff`: one way to fix D1 (in part) and D2; not a patch to apply as it is.
- `ring.py`, `ring.out`, `s1_fix.out`: B1 and the reading of 92256f8.
- `floor_main.py`, `floor_failed.py`, `floors_main-49c5f62.out`: question 4.
- `plants.py` with `plants-new-a/-b/-c-49c5f62.out`; the older lists' results as `plants-*-49c5f62.out`; `compare.py`, `compare.out`.
- `rejudge-49c5f62.out`, `rejudge-113-49c5f62.out`: the older recordings under the head's check.

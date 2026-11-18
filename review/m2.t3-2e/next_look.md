Next look at #116 at 8e20e51

**Verdict: can merge from my side. Nothing blocks.** D1 and D2 are fixed and I could not break either fix. The link through `main` (question 4), S2 at a created start and `releases()` now have tests that kill the plants that survived before. No earlier test is weaker. What is left is four small notes, none of them a defect in the check.

The condition outside my reading is CI on the head: see the last section.

## The two questions

**(a) `reapplied`'s early-flush set: yes, it is the rule I meant, and the tag used again is handled correctly.**

The rule (tests/kernel/cut_daemon_check.py:996-1031): a flush that the next instance received before the write's second `applied` line is "early". The first cut line decides. If an early flush made it, the mark was stored and the first `applied` line is kept. If a flush received later made it, or there is no cut line, the line is taken out, provided the write was applied again.

That matches the store. A flush takes the mark at its receipt and cuts at its barrier, the `applied` line is written at the moment the mark is set, and both lines are written by the main loop, so their order in the log is the true order.

The tag: my prototype left a tag in the set for good. The head discards a tag at any later receipt that is not an early flush (lines 1022-1025). That is right and better than mine. A tag is free only once its request is answered, and a flush's `cut` line comes before its answer, so by the time a tag is received again its earlier flush can no longer cut.

What I ran (windows.py, windows2.py; outputs `windows-8e20e51.out`, `windows2-8e20e51.out`):

| Log | The daemon | 49c5f62 | 8e20e51 |
|---|---|---|---|
| Ma: mark not stored; the flush handed first, no cut | right | passes | passes |
| Mb: mark stored; the flush handed first, cuts | right | passes | passes |
| Mc: mark stored; the write handed first, a ZIL flush received while it is on the threads, cuts after the second `applied` line | right | FAIL | **passes** |
| Md: as Mc, mark not stored | right | passes | passes |
| Mt: as Ma, and the group's own flush, received later, carries the early flush's tag | right | not run | passes |
| Mr: mark stored; two flushes handed first, the first cuts | right | not run | passes |
| Ms: as Mr, but the second flush cuts and not the first | wrong | not run | FAIL |
| Mx: as Ma, and the group's own flush cuts nothing (the second mark lost) | wrong | not run | FAIL |
| My: as Mb, and the group's own flush cuts the same group again | wrong | not run | FAIL |
| Mz: as Mc, and the group's own flush cuts the same group again | wrong | not run | FAIL |
| Me, Mg (an older mark in the state; the next instance killed too) | right | FAIL | FAIL, recorded in the description |

Window C is unchanged: the seven logs of the review judge as at 49c5f62.

**(b) The in-process `main` is a sound link. It runs the same function to the same exit, and I found no way for it to pass where the subprocess would not, other than the one named below.**

- **Same code path.** `main` reads `sys.argv`, writes `results.tsv`, and ends with `sys.exit(rc)` (cut_daemon_check.py:1534-1691); the `__main__` block only calls it. The helper patches `sys.argv` and `held_lines`, and takes the exit code from the `SystemExit`. The module keeps no state between calls.
- **Same result on every recording the tests build.** I ran the whole of `test_cut_daemon_main.Main` with each recording judged twice, by the subprocess and in the test's process with `held_lines` replaced by itself. 52 recordings (8 exit 0, 36 exit 1, 8 exit 2): no exit and no line differs (inproc.py, `inproc-8e20e51.out`).
- **Can it hide a bug in `main`?** One way exists, and the test closes it. `main` turns any exception into exit 2, so a replacement that raised would end 2 exactly as a set flag does (shown in inproc.py, part 2). The test's case with the flag clear wants exit 0, which a raising replacement cannot give. Both cases are needed, and both are there.
- **What it does not show:** that the program starts at all in a fresh interpreter with `-I`. Every other test of `main` still runs the subprocess, so that is covered.
- **Plants:** the helper not replacing `held_lines`, the helper returning a fixed 2 or a fixed 0, `main` setting the flag whatever `held_lines` says (on each call), `main` taking the flag from the first call only, and an ERROR flag ending 1: all seven killed. G3c to G3f of the review are killed too.

## The claims, each checked

- **D1 fixed:** yes (question a). Fourteen plants on the new rule: eleven killed, three survive and are equivalent on any log a daemon can write (below).
- **D2 and Codex's P2 fixed:** yes. `counted` is the state's own numbers and `gone` only the releases since the state began (cut_daemon_check.py:1240-1250). My scenario of the review now passes (`s2_new_state-8e20e51.out`), and six more cases judge as they should (`s2_more-8e20e51.out`): a release answered after the start, a refused release, only the old state's releases, and no release at all each FAIL with `cuts_held 0` and pass with 1. The plants for the running numbers, for no `since`, and for a release after the start are killed. Codex's thread has the author's answer; I cannot see over REST whether it is resolved. Codex has posted no review of 8e20e51.
- **G3's link through `main`:** yes (question b).
- **S2 at a created start:** yes. S2e, S2f and S2j, planted again in the new text, are killed by `test_instance_lines`.
- **`releases()` tests:** yes. The four plants that survived at 49c5f62 are killed by `test_the_releases_answered`.
- **The comment in `cut-record.sh`:** now right (lines 775-778: 31 written groups, at an even step, 31 prime).
- **Re-judging:** the 28 recordings of run 44237460823 give the same lines here as in the run, every one exit 0; `carried_across_failure` is 1775 and 1795 (`rejudge-8e20e51.out`). I also re-judged the 28 of the red run at 49c5f62: the same as that run, `takeover` exit 2 on both images.
- **The tests:** 209 pass here (41, 26, 18, 98, 26), and `test_cut_record.sh` passes.

## Notes, none blocking

1. **Two plants on D2's `since` survive** (cut_daemon_check.py:1246). Taking the releases since the start before this one, or since the second start, passes every test. The tests have no log where the state has lived through two starts since it began, or began at a third start. The code is right as written; a four-instance case would hold it.
2. **The new test's case "after the holder's verdict" holds a state the real function cannot produce.** On that call `held_lines` always returns `(1, False)` at once (line 1433), so the flag `main` reads there (line 1611) can only be set by a replacement. Harmless, and it guards a later change to `held_lines`; I mention it so nobody reads that case as evidence about a real recording.
3. **One wrong daemon is not told from a right one in window M:** a daemon that had not stored the mark and lets a flush take the mark at its barrier instead of its receipt leaves Mc's log line for line. It failed at 49c5f62 only because Mc itself did. This is a limit of the log, like Cf in window C.
4. **The description's F2 row** still has the old sentence ("applies the same write again ... before any cut line") before the new one. A wording matter.

Survivors I count as equivalent: D1i (the `not again` test at a cut line is implied by the tag test), D1j (a later flush cannot cut before an earlier one that waits), D1k (a cut line's tag is always a flush's), D2g (a release at the very nanosecond of a start), and M1, R2d and S2l as in the review.

## Plants on 8e20e51

| List | Plants | Killed | Survived | Not applied |
|---|---|---|---|---|
PLANTS_TABLE

- **No plant went from killed to survived.** Six went from survived to killed: S2h, G3e, G3f and the three on `releases()`.
- Ten of the review's 69 no longer apply because the commit changed their text (five in `reapplied`, five in S2). Each is planted again in the new text in plants2.py and killed.
- The older survivors are the same twelve as before.
- The diff of the tests since 92256f8 only adds: 86 lines in `test_cut_daemon_check.py`, and in `test_cut_daemon_main.py` the new test and the helper's `held` parameter. The helper's subprocess path gives the same exit and lines as before.

## CI and Codex

CI_SECTION

## What I could not verify

- Nothing with a kernel. The commit changes test code and one comment, so the rig runs on 92256f8 stand for the scripts; I read their recordings and did not run them.
- Whether Codex's thread is marked resolved (REST does not say).
- The daemon's behaviour in the window-M logs is from reading `inplace.rs` and `cuts.rs`, as in the review; I built the logs by hand and did not run the daemon.
- 24.04: I read no recording. The reference cut rig on 92256f8 is reported green on four images; I read its conclusion only.

## Evidence, in review/m2.t3-2e/

- `windows.py`, `windows2.py` with `windows-8e20e51.out`, `windows2-8e20e51.out`: question (a).
- `inproc.py`, `inproc-8e20e51.out`: question (b).
- `s2_new_state.py`, `s2_more.py` with their `-8e20e51.out`: D2.
- `plants2.py`, `plants2-8e20e51.out`: the new plants. `plants-new-8e20e51.out`: the review's 69. `plants-*-8e20e51.out`: the older lists. `compare2.py`, `compare2.out`: every list against its results at 49c5f62.
- `rejudge-8e20e51.out`: 56 recordings under this head's check.

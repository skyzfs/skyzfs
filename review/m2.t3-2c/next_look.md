## Next look at #113 at e88773b (the four commits after 98024d5)

**Verdict: it can merge. Nothing I found blocks it.** D1 and D2 are fixed, and both fixes are held by tests that fail when the old behaviour comes back. The count from 1 is pinned in both directions. G4's control fired both lines. The rule for a request log line cut short by a kill is sound: a kill does leave what the rule assumes, the daemon writes its log from one thread so the torn line is its last, and dropping it gives the log of a kill a moment earlier, so no wrong daemon passes through it. One thing is left before "ready to merge", by the workflow and not by a finding of mine: Codex's second finding, on `ae36c2f`, has no answer on its thread yet. It is R1 below, which I confirm and do not hold the merge for. The runs the author left on `e88773b` are all green; four rigs were cancelled there, and what each last showed is under "CI and Codex" for the lead.

There are three defects, all small, none blocking, and all on the safe side or older than this pull request:

- **T1.** The new rule takes a torn line only if what is left of it ends in a digit 0 to 9. A label line that stops on a hex digit a to f, or any line that stops after a tab, stays the rig's error. About one tear in nine by the recorded log. One character mends it.
- **T2.** A line cut inside its first column reads, with the start line that follows, as a start line with a wrong time, and the judge gives exit 1 against a right daemon. Older than `e88773b` and left as it was by it. About one tear in twenty-five.
- **R1.** A holder that ends with its verdict before its first written cut is still the rig's error in `takeover` and `failed`, and in `edges` where the recording waits for it to pause. This is Codex's P2 on `ae36c2f`.

I formed this from `git diff 98024d5..ae36c2f` and then `ae36c2f..e88773b`, the store and the kernel's behaviour as I could measure it here, before I read the author's answers and notes. Results below say which head they are from. `cut-record.sh`, the holder and the reference are the same file on `ae36c2f` and `e88773b` (`git diff --stat ae36c2f e88773b`: the check and its test only), and every scenario I ran on both heads gave the same lines on both.

### The items asked

**D1 (`e805d10`). Fixed, at all eighteen places, and the shell test fails if the bare wait comes back.**

- The four `holder_leave` and the fourteen `end_daemon` (one per recording) both end in `holder_ended` (`cut-record.sh:333-342`, called at `:325` and `:368`). I took the functions from the script as they are and ran them under `set -euo pipefail` with a stand-in holder (`next_holder_ends.sh`; `next_holder_ends-ae36c2f.out` and `-e88773b.out`, the same): a holder that ended with 1 and a VERDICT line goes on to the status read and the check from both; 1 without a VERDICT line and 2 are the rig's error with the holder's last lines in the message.
- Planted in `cut-record.sh` on `e88773b` (`next_plants-e88773b.out`): the bare wait back in `end_daemon` (W1), inside `holder_ended` (W2), after the kill in `end_daemon` (W7), `holder_leave` taking the status itself (W8), 1 without a VERDICT line taken as a verdict (W3), a verdict made the rig's error (W5), the holder's own 2 let through (W6): all KILLED by `test_cut_record.sh`. One survivor, W4 (a VERDICT line counts whatever the status): the holder never writes VERDICT and ends otherwise, so it is a gap of no weight.
- What is left of D1's kind is R1.

**The holder as a program (K6, K7, K8). Held now.** `test_the_program_ends_with_1_at_a_verdict` (`test_cut_hold.py:271`) kills all three on both heads; `test_the_program_ends_with_2_at_the_rigs_error` (`:285`) is its counterpart. K3, K5 and K9 still survive, as the author says (2d).

**D2 (`ae36c2f`). Fixed: my S9 is a verdict now, and a right daemon is not failed.**

- S9 (the acknowledged start took the state up, counts on, and its holder first writes cut 9 of the first state) gave exit 2 at 98024d5. On `ae36c2f` and on `e88773b` it gives exit 1 on both images, with `cut_numbers`, `status`, `cut_state`, `instance_2` and `held_content` FAIL, the last one naming it: "the holder wrote cut 9 in state 2, which has no cut line in that state and has one in the state before" (`next_scenarios-ae36c2f.out`, `next_scenarios-e88773b.out`).
- A right daemon: S0 (as recorded) exit 0, and the head's judge on the 28 recordings of run 44176849709 and the 14 of run 44183715455 gives exit 0 on every one, the run's lines plus the new `obs_torn_at_kill` INFO line (`next_rejudge-e88773b.out`; on `ae36c2f` the 28, the run's lines in the new order, `next_rejudge-ae36c2f.out`). This is the author's "42 recordings" claim, which I confirm.
- The branch (`cut_daemon_check.py:757-760`, `:1166-1173`) is narrow in the right way: only a written cut with no line in its own state and a line of that number in an earlier state. A written cut with no line in any state is still the rig's error. Plants X1 to X4 and X6 KILLED (the branch gone, widened to any state, ignored by `held_lines`, FAIL with rc 0, taken where the state has the line too). X5 SURVIVED: nothing pins that the instances' lines come before the held cuts (`:1291`). The order changes which lines a rig's error leaves behind, not a verdict.
- One limit, a remark: where the new state already has a cut of the same number, the write is taken for the new state's cut and fails by content, not by this line. Still a FAIL.

**The count from 1 (`f5afb60`). Pinned in both directions.**

- A state that counts on fails: S4 (counts on 13..20, the status says 20) and S5 (counts on, the status says 8) are exit 1 with `cut_numbers` FAIL "state 2's first cut line (request log line 4124) carries the number '13', not 1: a new state counts from 1 again"; at 98024d5 S4 passed. S7 (begins at 5) and S9 fail on the same line.
- A state that restarts at 1 passes: S0, and the 42 recordings.
- Planted one rule at a time in `numbers_verdict` (`cut_daemon_check.py:579-606`): may count on as before the commit (M1), must count on (M2), the state's own refusal gone (M3), the count not begun again (M4), a first line that is not 1 passes (M5), a new state not seen (M6): all KILLED. The status's count held to the whole log (Q1): KILLED. Q2 (the first new state, not the last) and Q3 (the last state only where its first line is 1) SURVIVE; Q2 needs a recording with two begun states, which none has, and Q3 changes no verdict because `cut_numbers` fails first.
- Still unpinned, as at 98024d5 and on the author's list for 2d: the lines of a new state after its first (plant N3 on this head, N5 before).
- The decision is in the description and in the function's comment. It is not in `docs/skyzfs-design.md` on `zfs-topology` yet; that is the lead's file.

**G4's control. Confirmed from the run.** Run 44185480330 (dispatch on `skyzfs-testing-2c` at `ae36c2f`, `daemon_ref` bb70c2dc, check g, ubuntu-26.04), job 134400862289: `pin_data_journal_refused` FAIL, "wrong: the refused start changed a cuts.state; the serve after took up a state (taken_up) though the refused start wrote a new start token". The other eight pins of check g PASS (`pin_start_serves_what_the_disk_holds`, `pin_failed_start_fails_volume`, `pin_failed_start_suspends_pool`, `pin_failed_start_volume_refused`, `pin_sync_failure_suspends_pool`, `pin_sync_failure_fails_volume`, `pin_sync_failed_volume_refused`, `pin_failure_reason_is_the_images`). The run ends with 1, "results differ from the table EXPECTED", as a negative control must. G4 is closed.

**The red run on `f5afb60`. The claim is right.** In the artifact of run 44183715455 (ubuntu-26.04), `takeover/requests.tsv` has one line that does not parse, 43866: pid 9937's `label 25 write 1047960 8` line stops after 8012 bytes and pid 10976's start line follows in the same line. The line began 180 bytes into a page of the file and stops at byte 13389824, which is 3269 times 4096 (`torn_line.out`). Pid 9937 is the instance the rig kills in course (a), under a writer that syncs the pool all the time, so the daemon is writing label lines of 8 KB one after another: 1054 lines of the log are longer than a page and hold four fifths of its bytes.

- On `ae36c2f` a line cut short was the rig's error wherever it stood, last of a killed instance or in the middle (`next_torn-ae36c2f.out`). That was right for the middle and wrong for the last line: with the torn line taken out by hand the recording is exit 0 with 250 cuts and every instance PASS.
- On `e88773b` the recording as it was recorded is exit 0, `obs_torn_at_kill` says "1 lines ... line 43866: pid 9937's ... line after 8012 bytes, then pid 10976's start", and every other line is the same as with the torn line taken out by hand (`next_torn-e88773b.out`). A line cut in the middle of an instance's log is still exit 2, and so is one before the start of an instance that created the device ("did not take the device over from a killed daemon").

### The rule of `e88773b` against what a kill leaves

A torn line below is a line that a kill cut short; a `cut` line is the daemon's line of a cut. (My outputs call the torn line "the cut line".)

**The writer.** The daemon keeps no buffer of its own: `line` formats one line and hands it to `write_all` on a file opened for appending (`record.rs:179`, `:255-266`), so each line is one `write` of its whole length, some 8 KB for a label line. What can cut it short is the kernel alone.

**What a kill leaves (measured here, `tear_at_kill.py`, `tear_at_kill.out`).** A process appends lines of 8235 and of 40 bytes, each one `write` on an `O_APPEND` descriptor, and is killed with SIGKILL; local kernel 6.18, ext4, 4 KiB pages.

- One writing thread, 300 kills: the last line was cut short 102 times, every time at a multiple of 4096 of the file, and never did a torn line stand before another line.
- Three writing threads, 300 kills: 45 torn last lines, and 16 files where a torn line is followed by one to four whole lines; these 16 stop at a multiple of 4096 too. In all 16 the lines after the torn one are of threads created after the thread whose write was cut short. My reading: the kill reaches the threads one after another, and a thread it has not reached yet goes on writing.

So a `write` of a line is not all-or-nothing under a kill: it can stop at any page boundary of the file that the line crosses. Any line can be torn where it crosses a boundary, a 40-byte `cut` or `barrier` line too. This is what the kernel's buffered write does as I know it (it looks for a fatal signal between pages); I could not read the kernel's source from here, the measurement is what I have.

**Can a torn line be anywhere but last? Not in this daemon, and the reason is worth writing down.** Every line of the request log is written on the queue's thread: the queue's record and the store's watcher (`record.rs:255-266`, `:464`; `serve.rs:946-949`, `:987`; the comment at `record.rs:301-306` says "both on the queue's thread"). One writing thread is the first case above: the torn line is the last thing the instance wrote, and the next writer of the file is the next instance's start. So "the last line of an instance, the next instance's start following it unbroken" is exactly what a kill leaves. If a second thread ever writes lines to this log, the second case applies, a torn line followed by the same instance's lines becomes a kill's ordinary outcome, and the rule's "the same instance goes on" (a tested error) turns it into a red run. I read that the daemon has one writer; I did not run the daemon.

**Can the lost line hide something, so that a wrong daemon passes? No.** A line is written by the same thread that does the work, and writing it changes nothing in the store. A log whose last line is dropped is therefore the log of a kill a moment earlier, before that line was written, which the judge has to stand anyway. Held against the recording (`next_torn-e88773b.out`): with the first instance's last whole line taken out as well, with the second killed instance's last line (`sent`) left without its newline and so dropped, and with the label line cut one page earlier, the judge gives exit 0 and no line is FAIL or ERROR. A daemon cannot choose where the kill falls, so the one line lost is not evidence it can make disappear. What the rule accepts is bounded on every side that matters: the torn line must begin with a time and the last instance's pid, the start must end the line, be of a pid not yet started, lie in time between the torn line and the line after it, and (in `main`) be of an instance that `instances.tsv` says took the device over. Only `takeover` kills the daemon (`cut-record.sh:721-725`), twice, and both instances after a kill are taken-over ones, so the condition fits the rig.

Two things follow from "a kill a moment earlier" that the rule does not make worse but that should be known:

- **A `cut` line that is torn.** If the kill falls in the write of a `cut` line, the judge now sees the barrier without the cut. That is window B of the stage 2b review's D1 (`review/m2.t3-2b/review.md`), which is still open: the reference cuts at the old instance's barrier, a right daemon cuts the flush again in the next instance, and `cuts` FAILs. `next_torn_cut-e88773b.out`, on the fixture of that review: "C cut" is exit 1 with `cuts` FAIL "cut 13: the daemon's is group 26 (itself) at record 734, the reference's group 26 (itself) at record 731", against a daemon that is right by design §5.2. Before `e88773b` this log was exit 2. A `barrier` line cut short is judged like a kill before the barrier, which is right ("B cut" has the lines of "B before"). The cure is 2b's D1, not this rule. It needs the kill inside one 40-byte write that crosses a page boundary.
- **`record.rs` says the opposite of what happened.** Its module comment (`record.rs:14-18`) says each line is one `write`, "a `kill -9` loses nothing that was written, and two instances never interleave inside a line". A line is one `write` call, but a kill can stop that call at a page boundary, and then the next instance's start line does stand inside the killed instance's line. The author reported it to the lead. A start that finds the file not ending in a newline could write one first: then a torn line stands on a line of its own and no start line is ever glued to it, which would also close T2. That is the test build's code and the lead's call.

### T1 (defect; does not block). A torn line is taken only if what is left of it ends in a digit

`cut_daemon_check.py:264-266`:

```python
        digits = m.group(1)
        if len(digits) <= len(nxt):
            continue
```

`digits` is the run of digits before the start's pid: the end of what is left of the torn line, then the start's time. Where what is left ends in something that is no digit, `digits` is the start's time alone, as long as the next line's time, and the candidate is skipped. So the tear is not taken and the recording ends as before the commit, exit 2 "not eight to ten columns".

- **Scenario** (`next_torn-e88773b.out`, the red run's log with one byte changed): the label line's last byte before the start line made `a` or `f`, both hex digits a label line is made of: exit 2. What is left ending in a tab (the seven columns, no bytes yet): exit 2. A `barrier` line cut before its last column: exit 2.
- **How often.** For every page boundary inside a line of the two killed instances of that recording, I looked at the byte before it (`torn_odds.py`, `torn_odds.out`): of 3438 boundaries, 2858 (83.1%) would be taken, 388 (11.3%) would not because of this comparison (most of them `applied` lines, whose hashes are hex, and `commit` and `recv` lines), 141 (4.1%) are T2, and 51 (1.5%) stop inside the pid and are the rig's error. Each boundary counts once there; how likely the kill is at one or another I do not know.
- **Direction.** Safe: the run is red with the rig's error, as it was before the commit. It is the flake the commit set out to end, left in about one case in nine.
- **Mend.** `<` in place of `<=`. With that one change in a copy of the judge outside the checkout, the three scenarios above and the torn `barrier` line are exit 0 with the line counted, and the rest of the fifteen are as before (`next_torn-e88773b-less-than.out`). The pull request's tests pass either way (plant L6 SURVIVED), so a test with what is left ending in a letter is the other half.
- **Certainty.** High: run on the head, and the mend run.

### T2 (defect; does not block; older than `e88773b`). A line cut inside its time reads as a start line with another time

Where what is left of the torn line is only digits of its first column, it and the start line make one line of eight columns whose first column is digits: `parse_log` takes it as the next instance's start line with a time some digits too long (`cut_daemon_check.py:303-314`; `torn_line` is not asked, the line parses).

- **Scenario** (`next_torn-e88773b.out`, "only four digits of the line's time are left"): exit 1, `sessions` FAIL "session 1: the cuts give 221 groups, the kstat 242 committed" and `import_syncs_before_return` FAIL "session 2: 0 cuts complete before the command returned". The daemon and the recording are the right ones; only the four digits differ. The same on `ae36c2f` (`next_torn-ae36c2f.out`), so the commit did not bring it; it did not close it either, and the description's sentence "A line cut short anywhere else stays the rig's error" is not true of this case.
- **How often.** 141 of 3438 boundaries (4.1%) by `torn_odds.out`: the short lines are many and their time is a third of them.
- **Direction.** The wrong one: a verdict against a right daemon, from the rig's own log.
- **Mend.** A start line's time later than the time of the line after it is not a start line as written; `torn_line` already holds a start to that bound. Or the newline at the start in the test build, above.
- **Certainty.** High for the scenario; that a kill produces it is as likely as the count says, no run has shown one.

### R1 (defect; does not block). A holder's verdict before its first written cut is the rig's error

This is Codex's P2 on `ae36c2f` (its thread on `cut-record.sh:804`, not answered when I read it), and what I had found before I read it. `holder_wrote_one` (`cut-record.sh:309-313`, used at `:732` in `takeover` and `:804` in `failed`) waits 300 s for `held.tsv` and does not look whether the holder is alive; `hold_idle` (`:658-673`, used in `edges`) waits 120 s for the holder to say it paused.

- **Scenario** (`next_holder_ends-e88773b.out`, part 2): the holder has ended with 1 and "VERDICT cut 1 is still being uploaded after its write was refused" before it wrote one cut. `holder_wrote_one`: `rig_error: the holder wrote no cut in 300 s: 1 VERDICT cut 1 is still being uploaded ...`, exit 2. `hold_idle`: `rig_error: the holder did not say it had paused in 120 s`, exit 2.
- **Why it does not block.** The run is red, the refusal is in the message, and the same daemon gets its verdict from `end_daemon` in the other recordings that hold cuts. It is the first cut of three recordings, after a wait of minutes.
- **Mend.** In both waits, a holder that is no longer alive goes to `holder_ended`; what the recording does after a verdict there (it has no holder for the steps that follow) needs a decision, the simplest being to stop recording and go to the check.
- **Certainty.** High: the script's own functions, run.

### Gaps and suggestions (none blocks)

- **The program is not tested with a torn line.** `test_cut_daemon_check.py:902-949` tests `parse_log` and `torn_detail`. `main` not passing the list to `parse_log` (L12), `main` not holding the torn line to the take-over (L13), and `main` not writing `obs_torn_at_kill` (L14) all SURVIVE. The recorded log shows `main` right today (`next_torn-e88773b.out`: as recorded exit 0 with the line; before an instance that created the device exit 2). One case in `test_cut_daemon_main.py` with a split log and a torn last line would hold it.
- **Other survivors in the new rule.** L2 (the start may be of a pid already started) is the same error by another path, "a second start line". L3 (the start need not end the line) and L5 (the start may be later than the line after it): no test has such a line.
- **A tighter condition is to hand.** Every torn line here ended at a multiple of 4096 of the file (163 of 163 in `tear_at_kill.out`, and the red run's). `parse_log` knows each line's offset. Holding a torn line to that would tell a kill's tear from anything else far better than the digits do. The hosted arm64 images have 4 KiB pages too.
- **G1, G2, G3** of the first review stand as they were and are on the author's list for stage 2d, as the lead set. G1 on this head: a third instance that takes the new state up after its cut 2 is still `instance_3` FAIL "cuts '2' at the start, with 14 counted before it" (`next_third_instance-e88773b.out`), and with the count from 1 pinned that is no longer a matter of which rule the daemon follows. No recording has a third instance after an acknowledged failure.

### Plants

`next_plants.py` on `e88773b`, one change at a time, against the four Python test files and `test_cut_record.sh`; the file is restored after each and both worktrees were clean afterwards. 106 plants: **81 KILLED, 19 SURVIVED, 6 NOT APPLIED** (`next_plants-e88773b.out`). On `ae36c2f` the run was stopped when the head moved; the 17 it had done (K1 to K9, M1 to M6, Q1, Q2) gave the same results as on `e88773b` (`next_plants-ae36c2f.out`).

| Plants | What they change | KILLED | SURVIVED | NOT APPLIED |
|---|---|---|---|---|
| The 68 of the first review | the checker, the reference, the holder at 98024d5 | 54 | 8: K3, K5, K9, N3, N5, S3, F7, A7 | 6: N1, N2, N4, H8, S1, S2 |
| M1 to M6 | `numbers_verdict`, the count from 1 (`f5afb60`) | 6 | | |
| Q1 to Q3 | the status's count (`f5afb60`) | 1 | 2: Q2, Q3 | |
| W1 to W8 | `cut-record.sh`, the holder's end (`e805d10`) | 7 | 1: W4 | |
| X1 to X6 | an old state's cut written after the failure (`ae36c2f`) | 5 | 1: X5 | |
| L1 to L15 | a line cut short by a kill (`e88773b`) | 8 | 7: L2, L3, L5, L6, L12, L13, L14 | |

Against the first review's thirteen survivors: E4, K6, K7 and K8 are KILLED now (E4 by `test_two_instances`, the three by the holder's program test); N4, S2 and the plants N1, N2, H8, S1 no longer apply, their text went with the count from 1 and with D2's fix; K3, K5, K9, N5, S3, F7 and A7 survive as before, and the author has K3, K5, N5, S3 and F7 on the list for 2d. N3 was killed before and survives now because it changed meaning: with the first line of a new state checked on its own, it only lets the later lines of a new state carry anything, which is N5.

The new survivors, each said where it belongs above: Q2, Q3 and X5 change no verdict on anything the rig records; W4 cannot happen; L6 is T1; L12, L13 and L14 are the missing program test; L2, L3 and L5 are bounds no test reaches.

### CI and Codex

Read only, by the head's SHA.

On `e88773b`:

- `ci` 44186543021, `coverage` 44186542979, `e2e` 44186543038 and the push's `skyzfs-ci` 44186532829: success.
- The cut rig on the daemon, 44186543251: success, the logic job and the recordings on ubuntu-26.04 and ubuntu-26.04-arm. Each of the 28 recordings has its `obs_torn_at_kill` line and each says 0 lines: no kill cut a line in this run, so the new rule's only run on a real tear is the red run's artifact judged again.
- The cut rig, 44186543070: success, the reference job and the recordings on ubuntu-24.04, ubuntu-26.04 and both arm images.
- Cancelled by the author, as their note says: the daemon rig 44186543325, the replay rig 44186543012, the kernel probe 44186542923, node B's rig 44186543135, and the push's copies of the two cut rigs.

Whether the cancelled rigs matter is the lead's to read; what I can say of each:

- The daemon rig runs `daemon-rig.sh` and `daemon_check.py`, which this pull request changes (g.4) and which no commit after 98024d5 touches. Its last full run on this pull request is 44176849721 on 98024d5, green, plus the control above. It was cancelled on `f5afb60`, `ae36c2f` and `e88773b`.
- Node B's rig runs `cut-record.sh` with its own check. It was green on `ae36c2f` (44185412095), whose `cut-record.sh` is the head's.
- The replay rig and the kernel probe were cancelled on every head of this pull request, 98024d5 included. They start on the path `tests/kernel/` and run none of the files it changes.
- So the note's "(green on `ae36c2f`)" is true of node B's rig only; the description's line on the later heads has it right.

On `ae36c2f`: `e2e`, the push's `skyzfs-ci`, the cut rig on the daemon (44185411905, both images) and node B's rig green; `ci`, `coverage` and the cut rig cancelled by the push of `e88773b`. On `f5afb60`: `ci`, `coverage`, `e2e` green, the cut rig on the daemon red on ubuntu-26.04 for the torn line (44183715455) and green on arm.

Codex: its review of `e88773b` is completed and brought no new finding. Two inline findings in all. The P2 on `cut_hold.py:316` (D1) has the author's answer, "fixed in e805d10", which I confirm. The P2 on `cut-record.sh:804` from its review of `ae36c2f` (R1 here) has no answer yet; the status comment says there are unresolved findings. I cannot read whether a thread is resolved (no GraphQL here).

### What I could only read

- No kernel, ZFS or ublk here: nothing of `cut-record.sh` ran as a recording. Its functions around the holder ran with stand-ins.
- shellcheck is not installed here; `ci` runs it and is green on the head.
- That the daemon writes its request log from one thread: read in `record.rs` and `serve.rs`, not run.
- The kernel's reason for where a write stops: measured on the local kernel and file system, not read in its source, and not measured on the runners' kernels. The red run's own line agrees with the measurement.
- G4's control: its job log, not its artifact.
- The two daemon instances that a torn `cut` line would leave ("C cut") are the fixture of the stage 2b review, not a recording.

### Evidence added to this directory

Outputs carry the head they are from in their name.

- `next_plants.py`, `next_plants-ae36c2f.out` (a part: K, M and Q1, Q2, then the head moved), `next_plants-e88773b.out` (all).
- `scenarios.py` again: `next_scenarios-ae36c2f.out`, `next_scenarios-e88773b.out`. `third_instance.py`: `next_third_instance-*.out`. `rejudge.py` (it now says which lines differ): `next_rejudge-ae36c2f.out` (28 recordings), `next_rejudge-e88773b.out` (42).
- `next_holder_ends.sh`, `next_holder_ends-*.out`: the holder's ends and the two waits.
- `torn_line.py`, `torn_line.out`: the red run's torn line, its bytes and offsets.
- `next_torn.py`: fifteen logs made from the red run's recording. `next_torn-ae36c2f.out`, `next_torn-e88773b.out`, and `next_torn-e88773b-less-than.out` (the judge with T1's one character changed, in a copy).
- `next_torn_cut.py`, `next_torn_cut-e88773b.out`: a `cut` line and a `barrier` line cut short, on the stage 2b fixture.
- `tear_at_kill.py`, `tear_at_kill.out`: what SIGKILL leaves of a file appended to line by line, with one writing thread and with three.
- `torn_odds.py`, `torn_odds.out`: what each page boundary of the recorded log would leave.

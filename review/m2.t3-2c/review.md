## Review of #113 at 98024d5 (M2.T3 stage 2c)

**Verdict: it can merge once D1 is settled. Nothing I found lets a wrong daemon pass, and nothing fails a right daemon on the recordings the rig makes today.** The check across an acknowledged failure does what it intends: a first cut after the failure that lacks a changed region, or one changed block, is a `held_content` FAIL; either counting rule passes and anything else fails; the status is held to the count that goes with the rule. The daemon is on the side of the design in each place I compared. There are two defects, both of the same kind: a verdict on the daemon that comes out as the rig's own error (exit 2). D1 is Codex's finding, which I confirm by running the script's own functions: a holder that ends with its new verdict reaches the check only where it was asked to leave, not where a recording ends, so R2 holds for four of the eighteen places a holder ends. I would settle it here (two lines) because it is what R2 says the pull request does. D2 does not block.

I formed this from the diff, the design and the store before I read the description. I ran the head's judge on both artifacts of the pull request's run (28 recordings: the same lines and exit 0 on every one, `rejudge.out`), built ten hand-made logs from the recording `failed`, and planted 68 changes in the checker, the reference and the holder.

### The three questions

**(1) The check across an acknowledged failure. Yes, a first cut that misses a region is caught, and both counting rules pass as intended.**

What the recording holds (`scenarios.out`, both images alike): the first state counted 12 cuts and its holder wrote two of them; the holder left, `load 1` and the export wrote on, two cuts were held at the failure; the new state counted 8 and its holder wrote four, its cut 1 among them. That first cut has 6180 blocks (6177 on arm): the six regions the intent map named, whole, and some thirty blocks outside them. 1775 of its blocks (1792) differ from the image the first holder left, which is what a cut that lacked them would get wrong.

- **A missed region is caught.** With the first cut lacking region 4 (S1), lacking one changed block (S2), or holding only what its own instance wrote, which is the store's mutant "the regions of an untrusted state not carried" (S3): exit 1, `held_content` FAIL at "cut 13 (group 33)", 866, 1 and 1731 blocks. This works because `held_content` compares the whole image with the reference at every written cut, across both states, and the image is the one the first holder left. The first cut of a new state is always written: it is the cut being uploaded from the moment it is made, and the holder is started before the import.
- **Keying by state is right for what the recording can hold.** `held_by_epoch` (`cut_daemon_check.py:663`) takes a written cut's state from the instance that had started when the write was asked for. The first holder has left before the failure, so no write of the first state is asked for after the second start. The one case it gets wrong is a cut of the first state written by the second holder, which only a daemon with the defect this stage pins can produce: see D2.
- **Either counting rule passes, and nothing else.** As recorded (S0): "state 2 counts from 1 again", status `cuts 8`. Renumbered to count on, status `cuts 20` (S4): exit 0, "state 2 counts on". Counting on with the status at 8 (S5), from 1 with the status at 20 (S6), and a state that begins at 5 (S7): exit 1 each, on `status` or `cut_numbers`.
- **What the daemon does.** `take_up_cut_state` (`inplace.rs:2273`) begins a state whenever it does not trust the one it finds, with `CutState::begin_with`, which zeroes the region, and then restores the count from it: 0. So every begun state counts from 1, after an acknowledged failure and after a reboot or a start token that does not match alike. Counting on would need the count kept somewhere outside the state. The intent map stores its base's number and never compares it with a later cut's (`inplace/intent.rs`), so nothing in the store today depends on the numbers being unique across states. Where it will matter is M2.5, if a publish or an upload names a cut by its number: that is the lead's to decide, and taking either until then is the right thing for the check to do. The line says which, so the day it changes shows.
- **"Its first cut is whole"** (the lead's pin list for this stage) is, as built and as the notes say under "M2.3 as built", the whole volume narrowed to the regions of an intent map that is taken up, and a start that acknowledges a failure takes the map up. `instance_2` pins exactly that (`cut_state begun`, `intent_state taken_up`; 6 of 128 regions named), and the first cut's map is those six regions. It also pins, without saying so, that the start refused for want of the acknowledgement writes no start token (`inplace/start.rs:546`, `MarkedFailed` is returned before the token is read or written): if it wrote one, the map would not name the token of the start before and could not be taken up.

**(2) R2. Yes: `read block B of cut N:` is the right boundary, and it is the only refusal that is the daemon's own.**

The refusals of `write` in `cut_commands.rs` are: no cut N is held; its flush has not completed; `PATH: e` and `BLOCKS: e` (the rig's files); the cut is no longer held after a turn of the queue; `PATH: write block B: e` (the rig's image); the map grew; and `read block B of cut N: e` (`cut_commands.rs:414`), which is `read_cut`'s error (`inplace.rs:2799`): the image or the copies file did not give the daemon a block it holds. Only the last is the daemon failing at what it holds cuts for. The match is anchored at the start of the reply (`cut_hold.py:88`), so a path that contains the words cannot pass for it, and it is a verdict only if the cut is still the one being uploaded afterwards (`must_be_gone`, `cut_hold.py:247`): a cut that went under the read stays the holder's "gone". Two limits:

- D1: the verdict reaches the check only through `holder_leave`.
- Once M2.4's uploader abandons a cut whose read failed, the cut is gone after the refusal and this is no verdict any more. That is a later stage's question; I note it so that it is not found by surprise.

**(3) g.4 with `--hold-cuts`. Yes, the rule is the design's.**

"A state is trusted only if it names the generation of the start before this one" (§5.2). On this head the refused start writes a new token in the open (`inplace/start.rs:552-578`, in `open_unlocked`), is refused at `check_direct_io` (`inplace.rs:1147`), and has not reached `take_up_cut_state`: `cuts.state` still names the token of the serve before. The serve after finds the new token, the state does not name it, and it must begin. The intent map agrees twice over: its token is the old one (`Distrust::Token`), and it has no base, no cut having been committed (`Distrust::NoBase`), so the new state's first cut is the whole volume. Where a daemon left the token as it was, taking the state up is right too: the same line shows that the refused start changed nothing in the volume's directory but the status file, and no `cuts.state`, so no instance the state never saw wrote to the volume; and a map without a base is begun whatever the token, which costs a whole first cut and loses nothing. Beginning is always safe, so allowing both there is right.

I ran the rule's own text on nine cases by hand (`g4_rule.sh`, `g4_rule.out`): token rewritten or removed with `taken_up` is named as wrong, `begun` passes; token left passes with either; `off` or no `cut_state` is named. Two remarks, neither a defect: the serve after's `intent_state` is not looked at (the store's own tests hold the token rule for the map); and see G4 for the negative control.

### What "Not done" lists

None of the five is a hole that makes a claim of the pull request untrue. Two claims are narrower than they read, for other reasons: the R2 claim (D1) and "also fails a daemon that took the state up" (D2). On the list itself:

- **The count not pinned**: right to leave open, see (1).
- **The failure on an idle, exported pool**: a fair limit. With it, "its first cut is whole" is shown for changes that were complete and flushed before the failure. A failure with writes in flight is where the map's "no write begins before its bit is durable" does its work, and that is shown by the store's power-cut tests, not by this rig.
- **The negative control of g.4's new part: the reason given is wrong, and the control can be run.** See G4.
- **One run each**: the table says "same or caught" for the three rules, which is honest for one run. Both images gave `same`.

### D1 (defect; settle before the merge). A holder's verdict at the end of a recording is the rig's error

`cut-record.sh:357`, in `end_daemon`: `wait "$HOLD_PID" 2>/dev/null; HOLD_PID=""`. This is Codex's P2 on `cut_hold.py:316`. It is right.

**What happens.** The holder ends with 1 after writing its VERDICT line (`cut_hold.py:313`). `holder_leave` takes that (`cut-record.sh:327-329`). `end_daemon` does not: the recording runs under `set -e`, the bare `wait` returns 1, and the recording's subshell ends there, before the status is read and before the check runs. The loop at the bottom (`:865`) then writes "rig error: a command failed (status 1)" and counts exit 2. No `held_reads` line is written.

**Shown** with the two functions' own text, taken from the script and run in a `set -euo pipefail` subshell with a holder that ends with 1 (`end_daemon_wait.sh`, `end_daemon_wait.out`): "end_daemon, the holder ended with 1: the recording's subshell ended with 1, before any check"; through `holder_leave` the same holder gives 0 and the recording goes on.

**How much of R2 it takes.** A holder ends in `holder_leave` four times in a run (three in `takeover`, one in `failed`) and in `end_daemon` fourteen times, once for every recording. The drain, where most cuts are written, is in `end_daemon`. So the description's "the holder writes a VERDICT line and ends with 1, and the check makes it a FAIL line" is true for the smaller part. The description does say that the take-over recording passes such a holder on; it does not say that the other ends do not. The job is red either way, so no daemon passes that should not; what is lost is what R2 was for, the difference between a verdict and the rig's error.

**Certainty:** high for the path (run), high that no test holds it (there is no test of `cut-record.sh`'s functions; the unit tests hold the holder's step and the check's line, and the holder's exit has no test either, G5).

**To settle it:** in `end_daemon`, take the holder's status as `holder_leave` does (`wait ... || rc=$?`, 1 with a VERDICT line goes on to the check, anything else but 0 is the rig's error), or narrow the claim in the description and say when it follows. Codex's thread wants its answer in either case.

### D2 (defect; does not block). The defect this stage pins ends as the rig's error

`cut_daemon_check.py:680-683`, reached from `held_lines` before `instance_lines` runs.

**The scenario.** A daemon whose acknowledged start takes the state up (the store's mutant `ACKNOWLEDGED_TRUSTED`). `failure_line` demands a cut held at the failure, so the state it takes up holds one, numbered in the first state, and the new holder writes it first. The check has put that write in state 2 by its time, finds no cut line with that number there, and raises: exit 2, `rig_error ERROR the holder wrote cut 9 of state 2, which has no cut line in the request log`. `cut_state` FAIL is written before it; `instance_2` and `held_content` are not reached (S9, on both artifacts).

**Why the unit test does not show it.** `test_an_acknowledged_failure` (`test_cut_daemon_main.py:243`) gives the taken-up start a log that counts from 1 and holds nothing (S8 is that shape: exit 1, `cut_state` and `instance_2` FAIL). A daemon cannot leave that: a state taken up counts on and still holds its cut.

**What it costs.** The run is red and the table has the `cut_state` FAIL line, so nothing passes. But the exit says "the rig's error" for the one defect the recording was made to find, and the line that would say it plainly (`instance_2`) is missing. Certainty: high for the judge (run); that a real daemon with the defect leaves this log I read from the store, I did not run one.

**A way:** a written cut with no line in its state and a line with that number in the state before is a verdict ("a cut of the state before the acknowledged failure was still held"), not the rig's error; and let `instance_lines` run before the held cuts are keyed.

### Gaps and suggestions (none blocks)

- **G1. A third instance after the acknowledged one would fail a right daemon.** `cut_daemon_check.py:864`: for an instance that takes a state up, `before` counts every cut line before it in the whole log. After a state that counts from 1 the daemon's `cuts` at the start is the new state's count. A hand-made third instance that takes the new state up after its cut 2 and says `cuts 2` (`third_instance.out`): `instance_3 FAIL ... cuts '2' at the start, with 14 counted before it`. No recording has this today (`failed` has two instances). It will be met the day a kill is put after the acknowledged start, which 2d may well do. The same holds for `since` in `main` (`:1193`) when the last state has no cut line.
- **G2. No floor under what the first cut carries across the failure.** That 1775 blocks changed between the first holder's image and the new state's first cut is what makes `held_content` a test of "the first cut is whole". It follows from the recording's order (the holder leaves, then `load 1`), not from anything judged. A line that gives the figure and fails at 0 would keep a later change of the recording from emptying the check without notice.
- **G3. The acknowledged start's status is not asked what it holds.** `cuts_held 0` at the acknowledged start would say directly that nothing of the old state is held; today that is seen only through D2's path.
- **G4. The negative control of g.4's new part can be run.** The description says the daemon from before #106's fix is from before the start token too. It is not: `zfs-topology` at bb70c2d, the parent of #106's merge, has the start token (6128f23 is its ancestor; `volume.start` in `volume.rs`), has `--hold-cuts`, and runs `take_up_cut_state` before `set_up_io`, where the refusal then was (`inplace.rs:972-976` there). Its refused start takes the state up and stores the new token in it, so as far as I can read, a dispatch with `daemon_ref` at bb70c2dc283fbf6a59f7a336bb324eb59e735222 should give both new lines: "the refused start changed a cuts.state" and "the serve after took up a state (taken_up) though the refused start wrote a new start token". I could not run it. It is one dispatch, check g alone.
- **G5. Tests that do not hold what the code does** (the plants that survive, below):
  - *The holder's end at a verdict has no test.* K6, K7 and K8 survive: `main` may end with 2, write ERROR in place of VERDICT, or not catch the verdict at all, and `test_cut_hold.py` passes. `test_a_read_of_the_held_cut_refused` holds `step`; `test_a_held_cut_the_daemon_could_not_read` holds the check on a log written by hand. Between them are `main`'s exit and line and the script's handling (D1), and neither has a test. A test that runs the holder as a program against the fake that refuses a read, and looks at its exit and its log, would hold the first.
  - *A refused read of a cut that then went is not held either way* (K5), nor that the reason must stand at the start of the reply in `step` (K3: the test asks the pattern, not the step).
  - *`cut_numbers` in a new state.* A new state that begins at some other number (N4) and one whose later lines are wrong (N5) pass every unit test. The head does refuse the first (scenario S7's line), so this is the test's gap, not the code's.
  - *The status's count.* The unit tests do not tell which count goes with which rule (plant S2) or which side of the new state's start is counted (plant S3). The head is right on both by the scenarios S4, S5 and S6, and plant S3 would turn the recording itself red (`survivors.out`).
  - *`main` and the failure line's verdict* (F7): a `failure` FAIL with exit 0 passes the tests of `main`.
  - E4 and A7 change nothing that today's recordings or my scenarios can show (`survivors.out`); E4 only words `cut_numbers` differently on `takeover`.

### Plants

68 plants, one at a time, in a second worktree at the head, each against the four unit test files of the changed code, with the file restored after each (`plants.py`, `plants.out`): **55 KILLED, 13 SURVIVED, none NOT APPLIED.** The four files pass on the head as it is (141 tests). Both worktrees are clean afterwards (`git status --short`). The plants S1 to S3 are those of the status's count, not the scenarios S0 to S9.

| Where | Plants | Survive |
|---|---|---|
| `epochs` | 5 | E4 |
| `numbers_verdict` | 7 | N4, N5 |
| `held_by_epoch` and its use | 9 | none |
| the status's count (`since`) | 3 | S2, S3 |
| `instance_lines`, `cut_state` | 9 | none |
| `failure_line` and its use | 8 | F7 |
| `held_reads` | 4 | none |
| instances.tsv, the instance record, `lost_at_kill` | 7 | A7 |
| the table `EXPECTED` | 3 | none |
| `cut_ref.py`, the begun instance | 4 | none |
| `cut_hold.py` | 9 | K3, K5, K6, K7, K8, K9 |

The seven survivors in the judge I then planted again and held against what the rig itself has: the judge on `failed` and `takeover` as recorded, and the ten scenarios (`survivors.py`, `survivors.out`). Plant S3 fails the recording as recorded; plant S2 turns the scenarios S4 and S5 round; the other five change no exit. K9 is of no weight (the daemon names the cut it was asked for). What the survivors mean is under G5.

No plant in `cut-record.sh` or `daemon-rig.sh`: nothing here runs them, so a plant there has nothing to fail. For those I ran the functions and the rule as text (D1, question 3).

### CI and Codex on 98024d5

- **Everything that ran on the head is green** (read when the last run had ended, forty minutes after the push): `ci` (lint with shellcheck, test, unit-arm64, s3-backends, chart-lint), `coverage`, `e2e`, node B's rig, and the three rigs below.
- **The cut rig on the daemon** (run 44176849709): green on ubuntu-26.04 and ubuntu-26.04-arm, 14 recordings each. `failed` on both: `failure` PASS with two cuts held, exit 3, then 4; "state 2 counts from 1 again"; `instance_2` begun and taken up, 6 of 128 regions; `held_content` PASS; status `cuts 8`; the three rules' controls `same`. I ran the head's judge on both artifacts here: the run's lines, exit 0, on all 28 recordings.
- **The daemon rig** (run 44176849721): green, the daemon on ubuntu-24.04, 26.04 and 26.04-arm and the stand-in on 24.04 and 26.04. From the jobs' logs: 55 pins in each, none FAIL or ERROR; one WAITING with the daemon (the known one) and 25 with the stand-in. `pin_data_journal_refused` is PASS on all three images with the daemon, in the same words: "served ... with --hold-cuts, cut_state begun; on data=journal: serve exit 1 ... Nothing was served and the volume has not failed; files of the volume new or rewritten: volume.start; gone or rewritten: volume.start; cuts.state as it was; on data=ordered again: served ... with --hold-cuts, cut_state begun". That is the first arm of the new rule (the token rewritten, the state begun again); the other arm (the token left) is met by no daemon there is. With the stand-in the pin is WAITING, as before.
- **The cut rig** (run 44176849667): green on its four images and the reference job. It does not record `failed`, which is of held cuts.
- **Cancelled by the author** (their comment on the pull request): the replay rig and the kernel probe, and the push's `skyzfs-ci`. Their reason holds: neither workflow, nor a script it runs, names a file this pull request changes (I looked for each changed file's name under `tests/kernel/` and the workflows), and `ci` ran what `skyzfs-ci` would have.
- **Codex:** one finding, P2, the one that is D1 here; its thread has no answer yet. Its security review has ended and lists no finding.

### What I could only read

- `cut-record.sh`'s `rec_failed` and `daemon-rig.sh`'s g.4: no kernel, ublk or ZFS here. I read them against the run's artifacts and logs, ran `end_daemon` and `holder_leave` as functions with a stand-in holder, and ran g.4's rule as text. shellcheck is not installed here; CI's lint job ran it.
- The store and the daemon's commands (`inplace.rs`, `inplace/state.rs`, `inplace/start.rs`, `inplace/intent.rs`, `cut_commands.rs`): read, not built or run for this review; the pull request does not change it.
- G4's expectation for the daemon at bb70c2d: read from that commit's code.

### Evidence

In `review/m2.t3-2c/` on `skyzfs-review`: `rejudge.py`, `rejudge.out` (the head's judge on the 28 recordings of run 44176849709); `scenarios.py`, `scenarios.out` (S0 to S9 on both images); `third_instance.py`, `third_instance.out` (G1); `end_daemon_wait.sh`, `end_daemon_wait.out` (D1); `g4_rule.sh`, `g4_rule.out` (question 3); `plants.py`, `plants.out`; `survivors.py`, `survivors.out`.

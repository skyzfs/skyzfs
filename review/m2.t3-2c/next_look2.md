## Second next look at #113: ab34565 and 3fe4133 (the two commits after e88773b)

The head of this look is `3fe4133`. Where a result is from `ab34565` it says so; nothing I had on `ab34565` changed on `3fe4133` (the files are named by head).

**Verdict: it can merge. Nothing I found blocks it.**

- **R1 is closed at every place.** A holder that ends with its verdict now reaches the check from all five places a recording can meet it, `hold_idle` in `edges` included, and the check gives exit 1 with `held_reads` FAIL.
- **T1 and T2 are closed** by the new rule, which asks where a kill can cut a write instead of what the cut leaves. On a page of the file, every shape of what is left is taken, a tear inside the time and inside the pid too. Off a page nothing is taken, and the line that used to read as a start with a wrong time (T2) is now the rig's error.
- **The page bound holds wherever I could measure it**: 1800 more kills on six mounts here, 505 torn lines, every one ending at a multiple of 4096 of the file.
- **No wrong daemon passes** through either commit, and no right one is failed: the 42 recordings I have give exit 0 with the same lines on `3fe4133`, and a torn line is judged exactly as a kill at the end of the line before it, in twenty shapes on the recorded `takeover`.
- **L12 to L14 are held now**, and so is everything else I planted in the two commits but the survivors listed below, none of which changes a verdict.

What is left is small and none of it blocks: one new defect on the safe side that needs a tear in one of a few bytes of a 13 MB log (T3: a torn line that ends in 8192 decimal digits or more, with a page on the first of them, is the rig's error), two remarks on the rule's edges, and the survivors.

I formed this from `git diff e88773b ab34565` and `git diff ab34565 3fe4133` before I read the author's notes on either.

### 1. The torn-line rule of 3fe4133

**What it is** (`cut_daemon_check.py:239-296`, `:313-365`). Every line of the log is now looked at, not only one that does not parse. Inside a line, at each multiple of 4096 of the file, the rest of the line must be a whole start line (no leading zero in its time or pid) of a pid not yet started, in time between the last instance's last whole line and the line after. What stands before it must begin as a line does: a time; or a time, a tab and the beginning of the last instance's pid; or the time, that pid and more. A whole time must lie between the same two lines. Two new errors beside it: a pid that writes after its torn line, and a start line later than the line after it.

**Is the bound of 4096 safe where the rig writes its log?**

- *Where the log lives.* `skyzfsd serve --request-log "$RDIR/requests.tsv"` (`cut-record.sh:275`), `RDIR=$OUT/NAME` (`:408`), and the workflow gives `$PWD/cut-out` as OUT (`skyzfs-cut-daemon.yml:125`): the checkout on the runner's own disk. The one tear recorded on a runner (run 44183715455, ubuntu-26.04) ends at byte 13389824, which is 3269 times 4096. The jobs do not print the file system or the page size of that directory, so for the hosted images I have that one tear and CLAUDE.md's "4 KiB pages only", not a reading of my own. A line in the job's log with `stat -f -c %T` of OUT and `getconf PAGESIZE` would name both the next time a run is red on this (a suggestion).
- *Measured here* (`next2_tear_fs.sh`, `next2_tear_fs.out`; `tear_at_kill.py` with one writing thread, 300 kills on each; kernel 6.18, 4 KiB pages):

| The file is on | Torn last lines of 300 | Where they end, modulo 4096 |
|---|---|---|
| ext4, the container's own | 94 | 0, all |
| tmpfs (`/dev/shm`) | 88 | 0, all |
| tmpfs mounted with `huge=always` | 0 | none torn |
| overlay on ext4 | 90 | 0, all |
| ext4 with 1 KiB blocks, on a loop device | 134 | 0, all |
| ext4 with `data=journal`, on a loop device | 99 | 0, all |

  With the 163 of the e88773b look that is 668 torn lines and not one off a page. The tmpfs with huge pages tore none: a write that stays inside one large folio is copied in one piece, so there a tear is rarer and falls on the folio's end, still a multiple of the page. I had no `mkfs.xfs` or `mkfs.btrfs` here, so xfs and btrfs are not measured.
- *A larger page.* A kernel with 16 KiB or 64 KiB pages cuts at multiples of those, which are multiples of 4096; the rule then also looks at places such a kernel cannot cut, which costs nothing.
- *Another writer first, or larger writes.* The offset is that of the file from its first byte, whoever wrote the bytes before: each instance appends to the same file and the judge reads the same file. A daemon that wrote several lines in one `write` would be cut on a page all the same. The judge counts bytes from the decoded text (`:328`), which is right for a log of plain ASCII with no carriage return; the 42 recorded logs have no byte above 0x7f, no carriage return and no control byte but tab and newline.
- *What can still be cut off a page.* I know of one way: a copy that stops short inside a page because the writer's own buffer had to be paged in, with the kill arriving before the retry. The daemon has just formatted the line, so this needs memory pressure at that instant. Such a line is not taken and the recording is the rig's error, exit 2: the safe side. A file system that does not write through the page cache would be another; the rig uses none.

**Is any shape a kill can leave still exit 2, or read wrongly?**

- *On a page, every shape is taken* (`next2_torn_rule.py`, `next2_torn_rule-3fe4133.out`, part 1): one digit of the time, four, all but the last, the whole time with and without its tab, one digit of the pid, the whole pid with and without its tab, inside the event's name, a tab before the label, a hex digit a to f, 3916, 8012 and 16204 bytes of a label line, a `barrier` and a `cut` line whole but for the newline. Two kills with a torn line each in one log are both taken (part 4).
- *One byte off the page, none is taken* (part 2): each is the rig's error. Four digits of the time, off the page, is "a start line later than the line after it" where it was exit 1 with `sessions` and `import_syncs_before_return` FAIL at e88773b. That is T2, closed.
- *On the recorded `takeover`* (`next2_torn_real.py`, `next2_torn_real-3fe4133.out`). I moved the first kill back to earlier pages of the file: what is left of the line the page falls in, then the next instance's start. Beside each I judged the control, the log ended at the end of the line before, which is a kill between two writes with no torn line. Twenty shapes, the first page found for each in the 400 pages before the recorded tear: in all twenty the torn log and its control give the same exit and the same lines, but for `obs_torn_at_kill`. (Moved back by 95 lines and more, both give exit 1, because the later instances were recorded after writes the shortened log no longer shows; that is my shortening, not the rule.) This is the direct form of "a lost line is a kill a moment earlier".
- *My fifteen logs of the e88773b look* (`next_torn-3fe4133.out`): those whose tear stayed on the page are exit 0 as before (as recorded, the last hex digit changed to a or f, cut a page earlier). Those I had cut at a place of my choosing (after a tab, in the time, in the pid, a `cut` or `barrier` line) are exit 2 now, as tears no kill leaves. T1's shapes are in the two lists above, on a page.
- *A whole start line that a page crosses is not taken for a torn one* (part 3), with one exception that the rig does not reach, a remark below.

**T3, a defect, not blocking, on the safe side.** `torn_line` turns the digits before the first tab of "the rest" into a number at every page inside a line (`cut_daemon_check.py:289`). Scenario: a page of the file falls on a decimal digit other than 0 in a label line, everything after it in the line is decimal digits, and the kill tears the line two pages or more further on. At that earlier page the rest is those 8192 digits or more and the start line, a "start line" with a time of over 8200 digits, and Python refuses the conversion: `ValueError: Exceeds the limit (4300 digits)`, which `main` reports as the rig's error, exit 2, where the line should have been taken (`next2_torn_rule-3fe4133.out`, part 7: 4000 such digits are taken, 9000 are not; one page of them, 4096 and the time's 12, stays under the limit). Certainty: high, run.

Weight: small. Only `label` lines are longer than a page. Their long runs are the digit 0, and a page inside a run of zeros is stopped by the rule's "no leading zero" before any conversion: the line torn on the runner ends in a 1 and 7609 zeros and is taken. What it takes is a page exactly on the last digits other than 0 before such a run. In the recorded `takeover` log a kill tears a line at 3251 of the first instance's 3269 pages, and at none of them is an earlier page of the line such a place (`next2_torn_digits.py`, `next2_torn_digits-3fe4133.out`); the log has 28 runs of 8192 digits and more after a digit other than 0, so a page has to fall on one of a few dozen bytes of 13 MB, and the kill inside that line. A bound on the digits of a start's time (a time has 12 or 13) before the conversion would close it.

**Two remarks on the rule.**

- *A whole start line can be read as torn if two instances are 100 s apart.* Where a page falls after the first digit of a whole start line's time, the rest is a start line with a shorter time, and it is taken if that time is not earlier than the last instance's last line: last line at 95 s, start at 196 s, read as a start at 96 s with "1 byte lost" (part 3, its second list, where the page falls one byte into the start line; the same at 950 s and 1960 s, and not where the shorter time would be earlier than the last line's). It needs at least 100 s between one instance's last line and the next one's start while the clock is below 1000 s, 1000 s above that, and the page exactly there. The rig leaves under a second before the two instances that take a device over, and a line taken before any other instance is the rig's error (`torn_detail`), so no recording of this rig can be read wrongly this way.
- *The bounds on time rest on the log being in the order of its times.* The torn line's time must not be earlier than the line before it. In the 42 recorded logs, 675510 pairs of neighbouring lines, no line carries an earlier time than the line before it.

The rule's premise is as at e88773b and is now written into the function: one writing thread. A pid that writes after its torn line is the rig's error with that reason (`:346-348`).

**A torn `cut` line** is still window B of the stage 2b review's D1 (exit 1 against a right daemon). The author has it on the list for 2d; the new rule neither mends nor worsens it.

### 2. R1: closed at every place

| Place | Called from | A verdict there at e88773b | ab34565 | 3fe4133 |
|---|---|---|---|---|
| the stop, `end_daemon` (`cut-record.sh:373`, through `holder_ended` at `:385`) | `end`, every recording that holds cuts | the check, exit 1 | the same | the same |
| asked to leave, `holder_leave` (`:324`, `holder_ended` at `:333`) | `takeover` (`:752`, `:778`, `:789`), `failed` (`:824`) | the check, exit 1 | the same | the same |
| the wait for the first cut, `holder_wrote_one` (`:313`) | `takeover` (`:751`), `failed` (`:823`) | the rig's error after 300 s | **the check, exit 1** | the same, through `holder_gone` |
| the wait for the pause, `hold_idle` (`:676`) | `edges` (`:710`) | the rig's error after 120 s | the rig's error after 120 s | **the check, exit 1** |
| no wait: `rec_bound`'s pause (`:660`), `hold_go` (`:693`), `cleanup` (`:214`) | | nothing to take | | |

The `:732` and `:804` of the question are `:751` and `:823` on this head.

I took the functions from the script as they are and ran them the way a recording does: the recording's function called plainly under `set -euo pipefail`, the wait in it as the script at hand calls it, then `holder_leave` and `end_daemon` (`next2_holder_ends.sh`; `next2_holder_ends-3fe4133.out`, `-ab34565.out`, `-e88773b.out`).

| The holder | e88773b | ab34565 | 3fe4133 |
|---|---|---|---|
| a verdict before the first cut | exit 2 | pool exported, on to the check | the same |
| ended with 0, or with 1 and no VERDICT line, or with 2, before the first cut | exit 2 | exit 2, saying which | the same |
| wrote a cut, then a verdict | on to the check | the same | the same |
| wrote a cut and stays | goes on, leaves when asked | the same | the same |
| a verdict while `edges` waits for the pause | exit 2 | exit 2 | pool exported, on to the check |
| ended with 0, 1 and no VERDICT line, or 2 while `edges` waits | exit 2 | exit 2 | exit 2, saying which |

Three things I checked because they are easy to get wrong:

- Both waits are now called on the left of `||`, where bash ignores errexit in everything they run (the script's own comment at the recording loop says so). Nothing in `holder_wrote_one`, `hold_idle`, `holder_gone` or `holder_ended` relies on errexit: every way out is a `return` or `rig_error`'s own `exit 2`, and `z` ends in `rig_error` itself. The right side, `{ export_pool; return 0; }`, runs with errexit as the rest of the recording does.
- A holder that wrote its cut and gave its verdict in the same second is not lost between the two tests of a turn: whichever the wait sees first, the verdict reaches the check.
- After an early end `HOLD_PID` is empty, so `end_daemon` skips its part for the holder and does not wait 720 s for one that is gone; what stays in the work directory is removed by the next recording's `begin_daemon`.

A nit: the comment that belongs to `holder_ended` now stands above `holder_gone` (`cut-record.sh:335-350`), and `holder_ended` has none of its own.

### 3. The check after a verdict: no wrong daemon passes, no right one fails

Since `ab34565`, `main` asks first whether `holder.log` has a VERDICT line (`cut_daemon_check.py:1327`). If it has, only `held_lines` runs of the held cuts' lines; if not, the lines run in e88773b's order. `3fe4133` does not touch this.

- **No wrong daemon passes.** The test in `main` is the one `held_lines` makes (`:1165`), and `held_lines` then puts `held_reads` FAIL and returns 1 before it reads any file of the holder's (`:1166-1169`). A VERDICT line therefore ends in exit 1 at least, or in exit 2 where the holder's log has an ERROR line as well (`:1159-1161`). Before the commit it ended in 1 or 2 too, never in 0.
- **No right daemon fails.** With no VERDICT line the lines and their order are those of e88773b. The head's check on the 42 recordings gives exit 0 on each (`next_rejudge-3fe4133.out`, identical to the files of ab34565 and e88773b), and S0 to S9 and the third instance give what they gave (`next_scenarios-3fe4133.out`, `next_third_instance-3fe4133.out`, identical too). A right daemon writes no VERDICT: the holder writes one only where a write was refused with "read block B of cut N" and the cut is still the one being uploaded (`cut_hold.py:219-251`). I confirm the author's "42 recordings" for `3fe4133`.
- **The early end on recorded data** (`next2_verdict.py`; `next2_verdict-3fe4133.out`, identical to `-ab34565.out`; `-e88773b.out`). Made from `failed` of run 44176849709 on both images: the first instance's part of the request log, its line of `instances.tsv`, the first session's marks and groups, the status read after the export, the holder's log with the verdict alone, none of its other files and no `failure.tsv`.

| Recording | e88773b | ab34565 and 3fe4133 |
|---|---|---|
| early, judged as `failed` | exit 2, "no failure.tsv: the holder did not get as far" | exit 1, `held_reads` FAIL |
| early, judged as `takeover` | exit 1, `held_reads` FAIL | exit 1, `held_reads` FAIL |
| early, judged as `edges` | exit 1, `held_reads` FAIL | exit 1, `held_reads` FAIL |
| early, with an ERROR line before the verdict | exit 2 | exit 2, "the holder: ERROR ..." |
| early, with no verdict in the log (the script does not leave this) | exit 2 | exit 2, "no failure.tsv" |
| whole, a verdict as the holder's last line | exit 1 | exit 1 |
| whole, a verdict and a wrong start of the second instance | exit 1, `instance_2` and `held_reads` FAIL | exit 1, `held_reads` FAIL alone |

  So the change in the check is what `failed` needed; `takeover` and `edges` ended in exit 1 at an early end already, once the script got them that far.

Two remarks on what the summary of such a run shows. Neither changes a verdict.

- *After a verdict the instances' lines and the failure line are not written, also where the recording is whole* (the last two rows). A daemon with a second defect in a start has only the first named in that run. The instances' lines do not need a whole recording, only the failure line does: with the instances' lines judged after a verdict as well, every test passes (plant Z4).
- *An early `takeover` also has `ctl_mark-dropped` and `ctl_mark-kept` SAME, "expected: caught; DIFFERS".* It ended before any group was synced with no flush, so the two rules give the design's cuts. Two red control lines beside the verdict that follow from where the recording ended.

### 4. Plants

One at a time, the file restored after each, against the four Python test files and `test_cut_record.sh`.

**On `ab34565`** (`next2_plants-ab34565.out`): 19 plants in what the commit changed, 16 KILLED, 3 SURVIVED.

- The wait and the two recordings (Y1 to Y10): all KILLED. The wait that does not look whether the holder has ended (Y1, the script as it was) is killed here by my runner's limit of 300 s on a test file, because the old wait takes 300 s a case. Run to its end against that plant, `test_cut_record.sh` fails four lines and ends with 1: "a verdict before the first cut ends the recording for the check", "a holder that ended with 0 before a cut is the rig's error", "a holder that ended with its error before a cut is the rig's error", and the count of the calls of `holder_ended`.
- The check (Z1 to Z8, X5b): Z1, Z2, Z3, Z5, Z6 and Z8 KILLED; Z4, Z7 and X5b SURVIVED, below.

**On `3fe4133`** (`next2_plants-3fe4133.out`): the plants of the e88773b look in the code that the two commits touch (I, W, X and L: the acknowledged start, the holder's status, the held cuts' lines, the torn line) and those of `ab34565` again, and 27 new ones in the rule (P1 to P19) and the waits (U1 to U8). I did not run the other 71 plants of the e88773b look again: the two commits change four places of the check (the rule, two places of `parse_log`, the order in `main`) and the two waits of the script, remove no test and change one line of the shell test (the count of the calls of `holder_ended`). 81 plants: 59 KILLED, 8 SURVIVED, 14 NOT APPLIED.

- **L12, L13 and L14 are KILLED**, each by `test_a_line_torn_at_the_take_over` (`test_cut_daemon_main.py:354`): `main` without the list for torn lines, `main` not holding a torn line to the take-over, `main` saying nothing of it. L8 to L11 are killed as before.
- **The rule** (P1 to P19): 16 KILLED. The start at any byte and not at a page (P1), a page of 8192 (P2), pages counted from the line's beginning (P4), an offset that counts no newlines (P5), a leading zero (P6), any whole pid or any digits of a cut pid before the start (P8, P9), each of the four bounds on time (P10, P12, P13, P14), the writer after its torn line (P15), the start later than the line after it (P16), only a line that does not parse looked at (P17), the last instance's start instead of its last line as the bound (P18), what is left without its whole time (P19). P3, P7 and P11 SURVIVED, below.
- **The waits** (U1 to U8): all KILLED. `hold_idle` and `holder_wrote_one` without the look at the holder (U1, U7) are killed by my runner's limit again. Run to its end against U1 (the old `hold_idle`), `test_cut_record.sh` fails four lines and ends with 1: "a verdict while the holder is asked to pause ends the recording for the check", "a holder that ended with 0 while asked to pause is the rig's error" and "a holder that ended with its error while asked to pause is the rig's error", each with "the holder did not say it had paused in 120 s", and "no wait for the holder without a look at whether it lives". The author's four lines; three of them carry the message they quote, the fourth is the count.
- **NOT APPLIED, 14**, each because the text it changes is gone and another plant stands for it: L1 to L7 are in the rule of e88773b, which the new rule replaced (P1 to P19); Y1 to Y5 and Y10 are in the wait as `ab34565` had it, now in `holder_gone` (U5 to U8, and W2 and W5 for the status); X5 is X5b.

The survivors:

| Plant | Why it survives | What it means |
|---|---|---|
| P3, a page is 2048 bytes | No test has a start at an odd multiple of 2048 that must not be taken. | A gap in the tests, not in the rule: with 2048 the rule would take a tear that no kernel with 4 KiB pages leaves. A case of `test_a_line_torn_anywhere_on_a_page` with the start moved by 2048 bytes would hold it. A suggestion. |
| P7, the start may be of a pid already started | Equivalent: `parse_log` gives "a second start line of pid" for it, the rig's error either way. | Nothing. The author names it as equivalent, and I agree. |
| P11, the start may be later than the line after it | Equivalent: `parse_log`'s own test (`:351`) gives the rig's error either way. | Nothing. The same. |
| L15, the line after the start need not begin with a time | Equivalent in the exit: without the test the conversion of that line's first column fails, the rig's error either way. | Nothing. |
| W4, a VERDICT line is a verdict whatever the holder's status | As at e88773b. | As there: the holder writes the line and ends with 1 in one place. |
| Z4, the instances' lines judged after a verdict as well | No test asks that they are not written. | The first remark of part 3. |
| Z7, a verdict judged first in `failed` only | `takeover` gave exit 1 at an early end before the commit too (the table of part 3); the test's `takeover` half asks for the exit and `held_reads`, which both orders give. | The order matters to `failed` alone; nothing is lost. |
| X5b, the held cuts judged before the instances' lines where there is no verdict | No test pins the order of the lines. | As X5 at e88773b: the order changes what a summary shows first, not an exit. |

All three worktrees are at `3fe4133` with nothing changed after the runs.

### 5. CI and Codex

**On `3fe4133`**, every run has ended. Green: `ci` (44198227959, `lint` and `unit-arm64` in it), `coverage` (44198227947), `e2e` (44198227827), the push's `skyzfs-ci` (44198217339), the cut rig on the daemon (44198228019, the check's logic and two images), the cut rig (44198228271, the reference with no kernel and four images) and node B's rig (44198227956). I read the cut rig on the daemon's two jobs, ubuntu-26.04 and ubuntu-26.04-arm: `takeover`, `failed` and `edges` are whole on both, `held_reads` PASS, no line cut short at a kill in this run, worst exit 0. So the new waits and the new rule have run on a kernel in their ordinary course; the early end and a torn line have not run there on this head.

Cancelled by the author on this head, as on the heads before it: the daemon rig (44198227931), the replay rig (44198227944), the kernel probe (44198227990), and the push's duplicates of three rigs. The two commits change `cut-record.sh`, `cut_daemon_check.py` and their three test files, which those three workflows do not run; what each of them last showed on this pull request is in `next_look.md`, and nothing since changes it. Whether that is enough for "CI is green on the head" is the lead's to read.

**On `ab34565`**: green `ci` (44194942962), `coverage` (44194942806), `e2e` (44194942793), `skyzfs-ci` (44194934440), the cut rig on the daemon (44194942764) and node B's rig (44194942861); the cut rig (44194942885) was cancelled by the push of `3fe4133`.

**Codex** has reviewed neither `ab34565` nor `3fe4133`. Its only sign since is a thumbs-up on the pull request at 23:23 UTC on 17 November, five minutes after the push of `3fe4133`, which is what it leaves when it has nothing to say. Both of its findings (the stop's bare wait, and R1) have the author's answer on their threads; whether the threads are marked resolved cannot be read over REST. The author's answers to T1, T2, R1 and L12 to L14 are in the comment of 23:20 UTC. I checked each claim there against what I had already run: the page rule, the equivalent survivors (P7, P11), the 42 recordings and the four failing lines of the shell test agree with mine. T3, the 100 s remark and P3 are not in it.

### 6. What I could only read

- No kernel module of the rigs, no ZFS and no `skyzfsd` ran here. An early end at a holder's verdict has not run on a kernel anywhere and cannot with what the tree has: it takes a daemon that refuses to read a held cut, and the test build has no hook for that (`cut_commands.rs:414` is the only place the refusal is made). What I ran are the script's functions with stand-in holders and the check on recordings cut down by hand.
- Two steps after an early end rest on the daemon and are read, not run: `zpool export` with the holder gone and cuts held (a pool does go on with a paused holder, which `bound` shows on every green run), and `end_daemon`'s "serve ended with 0 on SIGTERM", which a right daemon gives with cuts held (course (c) of `takeover`) and a daemon that has just refused a read may not. If it does not, the recording is the rig's error after all; the same is true of the path e805d10 opened.
- The page bound: measured on this container's kernel (6.18, amd64, 4 KiB pages) and file systems, and seen once on a hosted runner. Not measured: xfs, btrfs, an arm64 kernel, a kernel with larger pages. Why a kill cuts between pages is my reading of how a buffered write behaves, as at e88773b; I could not read the kernel's source from here.
- That the daemon writes its request log from one thread: read at e88773b (`record.rs`, `serve.rs`), not run; neither commit touches the daemon.
- shellcheck is not installed here; CI's run of it on `3fe4133` is green (`ci`, `skyzfs-ci`).

### Evidence

All in `review/m2.t3-2c/` on `skyzfs-review`. New for this look:

- the torn-line rule: `next2_tear_fs.sh`, `next2_tear_fs.out`; `next2_torn_rule.py`, `next2_torn_rule-3fe4133.out`; `next2_torn_real.py`, `next2_torn_real-3fe4133.out`; `next_torn-3fe4133.out`;
- the holder's end: `next2_holder_ends.sh` with its three outputs; `next2_verdict.py` with its three outputs;
- T3's weight: `next2_torn_digits.py`, `next2_torn_digits-3fe4133.out`;
- plants: `next2_plants.py`, `next2_plants-ab34565.out`, `next2_plants-3fe4133.out`; the shell test run to its end against the old waits, `next2_shell_old_wait-ab34565.out` and `next2_shell_old_hold_idle-3fe4133.out`;
- reruns: `next_scenarios-`, `next_third_instance-` and `next_rejudge-` with `ab34565.out` and `3fe4133.out`, and `next_torn-ab34565.out`.

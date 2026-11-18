## Third next look at #113: 73869d1 (the one commit after 3fe4133)

**Verdict: it holds. It can merge, and nothing I found blocks it.**

The commit does three things: it bounds the digits of a torn line's times before any conversion (T3), puts `holder_ended`'s comment back above it, and logs the file system and the page size of the request logs. I formed this from `git diff 3fe4133 73869d1` before I read the author's note.

- **T3 is closed.** The log that ended in `ValueError` at 3fe4133 is taken at 73869d1, in the small logs and on the recorded `takeover`.
- **The skip cannot drop a real tear.** Where the torn line's time is whole it only says early what the bounds on time said already; where the time is cut it is a new bound that every real tear meets. All 18 shapes are taken with times of 9, 12, 13 and 20 digits and across the change from 12 to 13.
- **It lets no wrong one through.** It only removes candidates, and after it at most one page of a line can still give one.
- **Nothing else moved.** The 42 recordings give exit 0 with the same lines; the twenty moved kills on the recorded `takeover` are judged as their controls; the scenarios of the earlier looks give what they gave.

Of ten plants in the new bound seven are KILLED and three SURVIVED (B3, B7, B8), and one plant of the 3fe4133 look that was killed survives now (P6, the start's leading zero); each is a gap in the tests, not in the rule. Three remarks at the rule's edge, all about lines that no daemon writes and no kill leaves, are in part 1.

### 1. Is the skip right?

The change (`cut_daemon_check.py:276-279`, `:292-293`): no torn line is taken where the next line's time has more than 20 digits; and a candidate is skipped where the start's time, or the torn line's own time without leading zeros, has more digits than the next line's time.

**Can it skip a real tear?** No, for three reasons that I checked one by one.

- *The start's time.* The start line's time has no leading zero (`START_LINE`), so more digits than the next line's time means a later time, and `before <= ns <= int(nxt)` (`:295`) refused that already. The skip says it before the conversion and changes no result.
- *The torn line's time where it is whole* (a tab follows). The rule held it to `before..ns` already (`:297`), and `ns` to the next line's time: again only earlier.
- *The torn line's time where it is cut* (digits and nothing else are left). This is new: 3fe4133 put no bound on those digits. What a kill leaves there is the beginning of a time of the same clock. The daemon writes `CLOCK_MONOTONIC` in nanoseconds with no leading zero (`record.rs:203-209`), one clock for every instance on the machine, so the torn line's time is not later than the next line's and its beginning is not longer.

*Clocks that differ* would be a log with instances of two boots or two machines. The rule already asked for times in order (`before <= ns`) at 3fe4133, so such a log was the rig's error then and is now; the rig records each log on one machine in one boot. In the 42 recorded logs no line is earlier than the one before it (the 3fe4133 look).

*Leading zeros*: the daemon writes none. The rule counts the torn line's time without them, which keeps what 3fe4133 did with such a line (a remark below).

Run (`next3_bound.py`, `next3_bound-73869d1.out`; the same on 3fe4133 in `next3_bound-3fe4133.out`):

| Times | Shapes taken at 73869d1 |
|---|---|
| 12 digits, the recorded ones | 18 of 18 |
| 13 digits, after 1000 s | 18 of 18 |
| the torn line at 999.95 s (12 digits), the start at 1000.1 s (13) | 18 of 18 |
| the start at 999.99 s (12 digits), the next line at 1000.0 s (13) | 18 of 18 |
| 9 digits, the first second after boot | 18 of 18 |
| 20 digits | 18 of 18 |

The 18 shapes of the 3fe4133 look are taken on a page and refused one byte off it as before (`next2_torn_rule-73869d1.out`, which differs from the file of 3fe4133 in the two lines of T3 alone).

**Can it let a wrong one through?** No. The skip is a `continue`: it takes nothing itself. With the next line's time at 20 digits at most, a candidate that passes has a first column of 20 digits at most, and two pages are 4096 bytes apart, so one page of a line at most can pass: skipping an earlier page cannot make the rule choose between two. What it newly refuses is what no kill leaves, and it ends as the rig's error: 13 digits and no tab before a start where the next line's time has 12 was taken at 3fe4133 and is "a start line later than the line after it" now; 30 digits the same (part 2 of the output).

**T3 itself** (part 4 and part 5).

- Small logs: 8192 digits of 1 or of 9, 12288 of 7, 262144 of 1, and a 1 on the page followed by 8191 zeros all ended in `ValueError` at 3fe4133 and are taken at 73869d1 with the whole line lost. 4096 digits were taken at both.
- On the recorded `takeover`: I moved the kill into the 229422-byte label line of line 844 and put a 1 on the page two pages before the tear, at two places. At 3fe4133 the check stops with the `ValueError` before any line. At 73869d1 the torn log is judged line for line as its control and `obs_torn_at_kill` names the line. (Both end as the rig's error, because a log shortened by 2500 pages no longer has the cuts the holder wrote; that is my shortening.)
- The author's new case in `test_a_line_torn_anywhere_on_a_page` is this scenario with 9000 digits, and it holds the fix (plants B1, B2, B10).

**Three remarks, none of weight.** All three are about lines no daemon writes and no kill leaves.

- *Where the next line's time has more than 20 digits, a line torn in its time is read as a start with a longer time, and nothing says so* (part 2: with 21 digits the second instance "starts at 2639264075526204" and no line is lost; 3fe4133 took the tear). That is T2's shape, behind a time of 3000 years of uptime: `mono_ns` is seconds times 10^9, 12 or 13 digits on a runner. A second kill cannot make such a first column either: the line after a torn line's start begins some 45 bytes after a page, so its time is not on one (part 3: the rig's error at both heads). Refusing a first column of more than 20 digits in `parse_log` itself would close it; a suggestion.
- *A line whose own first column has more than 4300 digits still ends in `ValueError`* (`parse_log`'s own conversion, `:349`): exit 2, as it should be for such a line, with Python's words instead of the rig's.
- *The torn line's time may begin with zeros* (`TORN_HEAD` takes any digits; "0" and eleven digits before a start on a page is taken, at 3fe4133 as well). The `lstrip('0')` is there for a test: `test_a_line_torn_at_the_take_over` fills the torn line up to its page with zeros before its time (`test_cut_daemon_main.py:367-376`, and its comment says so), and counted with them that time is longer than the next line's (plant B4 is killed by this test alone). So the rule is looser than the daemon's format by what one fixture needs. A real time never begins with 0; a fixture that put the filling into the line before would let `TORN_HEAD` ask for `[1-9]`. A suggestion.

### 2. The comment and the logging line (read, and the line run by hand)

- `holder_ended`'s comment stands above `holder_ended` again and `holder_gone`'s above `holder_gone` (`cut-record.sh:338-353`). Comments only; `bash -n` is clean.
- The line (`cut-record.sh:147-149`): `[ "$SERVER" != daemon ] || log "request logs on $(stat -f -c %T "$OUT"), page size $(getconf PAGESIZE)"`. It stands after `mkdir -p "$OUT"` (`:122`) and after `SERVER` is checked, so OUT exists. Run under `set -euo pipefail` here it prints `request logs on ext2/ext3, page size 4096` for the checkout and `tmpfs` for `/dev/shm`; with a directory that does not exist `stat` complains, the line is printed with an empty name and the script goes on, since a failing substitution in an argument does not end a script. With the reference server it prints nothing. `ext2/ext3` is `stat`'s one name for ext2, ext3 and ext4.
- On the runners: both jobs of the cut rig on the daemon print it, `request logs on ext2/ext3, page size 4096` on ubuntu-26.04 and on ubuntu-26.04-arm (run 44207022432). That is the reading I did not have at 3fe4133: the hosted images write the request logs on a file system of the ext family (ext4 by every sign, `stat` does not tell them apart) with 4 KiB pages, amd64 and arm64: the first row of my table of mounts.

### 3. Plants

`next3_plants.py`, `next3_plants-73869d1.out`: one at a time, the file restored after each, against the four Python test files and the shell test.

| Plant | Result |
|---|---|
| B1, no bound at all (the rule of 3fe4133) | KILLED by `test_a_line_torn_anywhere_on_a_page` |
| B2, the start's time not bounded | KILLED by the same |
| B3, the torn line's own time not bounded | SURVIVED |
| B4, the torn line's time counted with its leading zeros | KILLED by `test_a_line_torn_at_the_take_over` |
| B5, a start as long as the next line's time is skipped (`>` to `>=`) | KILLED by 4 |
| B6, a torn line's time as long as the next line's is skipped | KILLED by 4 |
| B7, no bound of 20 on the next line's time | SURVIVED |
| B8, the bound of 20 is 12 | SURVIVED |
| B9, the bound of 20 is 11 | KILLED by 3 |
| B10, a candidate too long ends the search (`continue` to `return None`) | KILLED by `test_a_line_torn_anywhere_on_a_page` |

The survivors:

- **B3.** Where the time is whole the bound is implied by the bounds on time, so there it is equivalent. Where the time is cut it is the one new refusal of the commit (13 digits and no tab before a start, the next line's time 12 digits), and no test has it. A case in the list of logs that stay the rig's error would hold it.
- **B7.** No test has a next line whose time has more than 20 digits. Without the bound of 20 the rule does what 3fe4133 did there, which is the better of the two (the first remark of part 1); above 4300 digits both end in `ValueError`. The bound of 20 buys nothing that I can find.
- **B8.** No torn-line test has a time of 13 digits: with a bound of 12 every test passes, and every recording after 1000 s of uptime with a torn line would be the rig's error. The rule is right there (part 1's table); the tests would not notice if it stopped being. The recorded logs end between 70 s and 391 s, 11 and 12 digits. One torn line with times of 13 digits in `test_a_line_torn_anywhere_on_a_page` would hold it. A suggestion.

**The plants of the earlier looks in the rule, again on this head** (L1 to L15 and P1 to P19, in the same file): 22 KILLED, 4 SURVIVED, 8 NOT APPLIED (L1 to L7 as at 3fe4133, and L15, whose line the commit changed). L8 to L14 are killed as before, L12 to L14 by `test_a_line_torn_at_the_take_over`. P3, P7 and P11 survive as at 3fe4133. One result changed:

- **P6, the start line may carry a leading zero, was KILLED at 3fe4133 and SURVIVES now.** The case that held it is "the start not on a page" in `test_a_torn_line_not_followed_by_a_start`: a start one byte after a page with a 0 before it. With a leading zero allowed, that is a start with one digit more, and the new bound refuses it for its length before the rule about the zero is asked. So the commit took the teeth out of that case for this rule. It still matters in one place: where the next line's time has a digit more than the start's (the start at 999.99 s, the next line at 1000.0 s), the planted rule takes that log, "lost 148 bytes", and the head's rule refuses it, as it should (`next3_p6.py`, `next3_p6-73869d1.out`). The rule is right; the test no longer holds this half of it. The same case with a next line of 13 digits would. A suggestion.

I did not run the plants in the script again: the commit moves a comment there and adds the one line of part 2.

### 4. CI and Codex

**On `73869d1`**, every run has ended. Green: `ci` (44207022782), `coverage` (44207022390), `e2e` (44207022154), the push's `skyzfs-ci` (44207012281), the cut rig on the daemon (44207022432: the check's logic, ubuntu-26.04 and ubuntu-26.04-arm), the cut rig (44207022167) and node B's rig (44207022341). In the two jobs on the daemon I read: the new line, as in part 2; `takeover`, `failed` and `edges` whole with `held_reads` PASS; no line cut short at a kill in this run; no FAIL, ERROR or DIFFERS line; "worst exit 0" on both.

Cancelled by the author, as on the heads before: the daemon rig (44207022602), the replay rig (44207022793), the kernel probe (44207022423) and the push's duplicates of three rigs. The commit changes `cut-record.sh`, `cut_daemon_check.py` and `test_cut_daemon_check.py`, which those three workflows do not run. As at 3fe4133, whether that is enough for "CI is green on the head" is the lead's to read.

**Codex** has not reviewed this head. Its thumbs-up on the pull request is now of 00:23 UTC on 18 November, three minutes after the push, which is what it leaves when it has nothing to say. No new thread; its two findings have the author's answers; whether the threads are marked resolved cannot be read over REST.

**The author's note** (the comment of 00:21 UTC) agrees with what I ran: the scenario with 9000 digits ends in `ValueError` at 3fe4133 and is taken at 73869d1, and the three `takeover` recordings give exit 0. It does not speak of the bound of 20 or of the cut time's new bound; neither changes a verdict.

### 5. What I could only read

- As before: no kernel, no ZFS and no `skyzfsd` ran here; a torn line and an early end have not run on a kernel on this head.
- That the times of one log come from one clock in one boot: read (`record.rs`, the rig's script) and seen in the recordings, not forced.
- shellcheck is not installed here; CI's is the reading of the new line.

### Evidence

In `review/m2.t3-2c/` on `skyzfs-review`: `next3_bound.py` with `next3_bound-73869d1.out` and `next3_bound-3fe4133.out`; `next3_plants.py`, `next3_plants-73869d1.out`; `next3_p6.py`, `next3_p6-73869d1.out`; the reruns `next2_torn_rule-73869d1.out`, `next2_torn_real-73869d1.out`, `next_rejudge-73869d1.out`, `next_torn-73869d1.out`, `next_scenarios-73869d1.out`, `next_third_instance-73869d1.out`, `next2_verdict-73869d1.out` (all but the first the same as the files of 3fe4133; the older file of the moved kills has one line of my own at its end).

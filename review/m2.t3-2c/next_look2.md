## Second next look at #113: ab34565 (one commit after e88773b)

**Verdict: it can merge. Nothing I found blocks it.** The commit does what its note says for the two places it names. A holder's verdict before its first written cut now ends `takeover` and `failed` there, the pool is exported, and the check gives exit 1 with `held_reads` FAIL. No wrong daemon passes through the check's new order, and no right one is failed by it. The new tests fail when the fix is taken away, in the script and in the check.

R1 had three places and two are closed. The third is as it was: `hold_idle`, where `edges` waits for the holder to pause, still turns a holder's verdict into the rig's error (R1c below). It was not in Codex's finding, the author's note does not claim it, and I do not hold the merge for it, for the reason I gave at e88773b: the run is red either way, and the same daemon gives its verdict in the other recordings of the job. T1 and T2 are unchanged.

I formed this from `git diff e88773b ab34565` (four files: `cut-record.sh`, `cut_daemon_check.py` and a test for each) before I read the author's note.

### Is R1 closed at every place? At two of three

Where a recording meets a holder that has ended, at `ab34565`:

| Place | Called from | A verdict there at e88773b | At ab34565 |
|---|---|---|---|
| the stop, `end_daemon` (`cut-record.sh:369`, through `holder_ended` at `:381`) | `end`, every recording that holds cuts | on to the check, exit 1 | the same |
| asked to leave, `holder_leave` (`:329`, `holder_ended` at `:338`) | `takeover` three times (`:746`, `:772`, `:783`), `failed` once (`:818`) | on to the check, exit 1 | the same |
| the wait for the first cut, `holder_wrote_one` (`:313`) | `takeover` (`:745`), `failed` (`:817`) | the rig's error after 300 s, exit 2 | **closed**: the recording ends there, the check gives exit 1 |
| the wait for the pause, `hold_idle` (`:671`) | `edges` (`:704`) | the rig's error after 120 s, exit 2 | **unchanged (R1c)** |
| no wait: `rec_bound`'s pause (`:656`), `hold_go` (`:687`), `cleanup` (`:214`, with `set +e`) | | nothing to take | the same |

The lines `:732` and `:804` of the question are `:745` and `:817` on this head.

**The wait for the first cut.** `holder_wrote_one` now looks whether the holder is alive on each turn. One that has ended goes through `holder_ended`, so its own error (2, or 1 with no VERDICT line) is the rig's error with its last lines; 0 before a cut is the rig's error too; a verdict returns 1, and both recordings have `holder_wrote_one || { export_pool; return 0; }`. I took the functions from the script as they are and ran them the way a recording does, the recording's function called plainly under `set -euo pipefail`, then `end_daemon` (`next2_holder_ends.sh`, `next2_holder_ends-ab34565.out`; the same script against e88773b's functions in `next2_holder_ends-e88773b.out`):

| The holder | e88773b | ab34565 |
|---|---|---|
| a verdict before the first cut | exit 2 | the pool exported, on to `end_daemon`, its status read and the check |
| ended with 0 before the first cut | exit 2 | exit 2, "the holder ended with 0 before it wrote a cut" |
| ended with 1 and no VERDICT line | exit 2 | exit 2 |
| ended with 2, its own error | exit 2 | exit 2, with the ERROR line in the message |
| wrote a cut, then a verdict | on to the check (through `holder_leave`) | the same |
| wrote a cut and stays | goes on, leaves when asked | the same |

Three things I checked in the function because they are easy to get wrong:

- It is called on the left of `||`, where bash ignores errexit in everything it runs (the script's own comment at the recording loop says so). Nothing in it or in `holder_ended` relies on errexit: every way out is a `return` or `rig_error`'s own `exit 2`. The right side, `{ export_pool; return 0; }`, runs with errexit as the rest of the recording does.
- A holder that wrote its cut and gave a verdict in the same second is not lost between the two tests of a turn: whichever the wait sees first, the verdict reaches the check, through `holder_leave` or through the early end.
- After the early end `HOLD_PID` is empty, so `end_daemon` skips its part for the holder and does not wait 720 s for one that is gone. What it then leaves in the work directory (`held.img`, `hold.stop`) is removed by the next recording's `begin_daemon` before its holder starts.

**R1c: the wait for the pause, a defect, not blocking.** `hold_idle` (`cut-record.sh:671-686`) is byte for byte what it was. Scenario: in `edges` the daemon refuses to read a block of a cut while the holder writes what it holds during `hold_idle`'s first pause. The holder writes its VERDICT and ends with 1; `hold_idle` waits 600 times 0.2 s for `hold.pause.held`, which a dead holder never writes, and says "the holder did not say it had paused in 120 s": the recording is the rig's error, exit 2, and the check never runs on it. Certainty: high, run with a stand-in holder (`next2_holder_ends-ab34565.out`, part 2). The same shape as the fix mends it: look whether the holder is alive inside the wait, take an ended one through `holder_ended`, and end the recording at a verdict. A verdict after `hold_idle` returned (while the cut is held across the write-zeroes, or after `hold_go`) is taken at the stop and reaches the check, so the hole is the first seconds of `edges`.

### The check's new order: no wrong daemon passes, no right one fails

`main` now asks first whether `holder.log` has a VERDICT line (`cut_daemon_check.py:1293`). If it has, only `held_lines` runs of the held cuts' lines; if not, the lines run in e88773b's order.

- **No wrong daemon passes.** The test in `main` is the same as the one `held_lines` makes (`:1131`), and `held_lines` then puts `held_reads` FAIL and returns 1 before it reads any file of the holder's (`:1132-1135`). So a VERDICT line always ends in exit 1 at least, or in exit 2 where the holder's log has an ERROR line as well (that test comes first, `:1125-1127`). Before the commit a VERDICT line also ended in 1 or 2, never in 0. Plant Z5 (the verdict judged first does not count) is KILLED.
- **No right daemon fails.** With no VERDICT line the lines and their order are those of e88773b. The head's check on the 42 recordings I have gives exit 0 on each, with the same lines as at e88773b (`next_rejudge-ab34565.out` is identical to `next_rejudge-e88773b.out`), and S0 to S9, the third instance and the fifteen torn logs give what they gave (`next_scenarios-ab34565.out`, `next_third_instance-ab34565.out`, `next_torn-ab34565.out`, each identical to its e88773b file). A right daemon writes no VERDICT: the holder writes one only when a write was refused with "read block B of cut N" and the cut is still the one being uploaded (`cut_hold.py:227-251`).
- **The early end on recorded data** (`next2_verdict.py`, `next2_verdict-ab34565.out`, `next2_verdict-e88773b.out`; made from `failed` of run 44176849709 on both images: the first instance's part of the request log, its line of `instances.tsv`, the first session's marks and groups, the status read after the export, the holder's log with the verdict alone and none of its other files, no `failure.tsv`):

| Recording | e88773b | ab34565 |
|---|---|---|
| early, judged as `failed` | exit 2, "no failure.tsv: the holder did not get as far" | exit 1, `held_reads` FAIL |
| early, judged as `takeover` | exit 1, `held_reads` FAIL | exit 1, `held_reads` FAIL |
| early with an ERROR line before the verdict | exit 2 | exit 2, "the holder: ERROR ..." |
| early with no verdict in the log (the script does not leave this) | exit 2 | exit 2, "no failure.tsv" |
| whole, a verdict as the holder's last line | exit 1 | exit 1 |
| whole, a verdict and a wrong start of the second instance | exit 1, `instance_2` and `held_reads` FAIL | exit 1, `held_reads` FAIL alone |

So the change in the check is what `failed` needed; `takeover` ended in exit 1 at an early end already, once the script got it that far.

Two remarks on what the summary of such a run shows. Neither changes a verdict.

- **After a verdict the instances' lines and the failure line are not written, also where the recording is whole** (the last two rows). A daemon with a second defect in a start has only the first named in that run. The instances' lines do not need a whole recording (at e88773b the early `takeover` has its `instance_1` line); only the failure line does. PLANT_Z4
- **An early `takeover` also has `ctl_mark-dropped` and `ctl_mark-kept` SAME, "expected: caught; DIFFERS".** It ended before any group was synced with no flush, so the two rules give the design's cuts. Two red control lines beside the verdict that follow from where the recording ended, not from the check or the daemon. The same in e88773b's order.

### Do the new tests hold the fix? Yes

One plant at a time on `ab34565`, the file restored after each, against the four Python test files and `test_cut_record.sh` (`next2_plants.py`, `next2_plants-ab34565.out`).

PLANTS_TABLE

### T1 and T2: unchanged

`torn_line` is not in the diff, and the fifteen torn logs give the exit codes of e88773b (`next_torn-ab34565.out`): a fragment that ends in a hex digit a to f or after a tab is still exit 2 (T1, `cut_daemon_check.py:265`, `<=` where `<` mends it), and a line cut inside its time column is still exit 1 against a right daemon (T2). Both stand as written in `next_look.md`, not blocking.

### CI and Codex on ab34565

CI_TEXT

### What I could only read

- No kernel, no ZFS and no `skyzfsd` ran here. The early end has not run on a kernel anywhere, and cannot with what the tree has: it takes a daemon that refuses to read a held cut, and the daemon's test build has no hook for that (`cut_commands.rs:414` is the only place the refusal is made). What I ran are the script's functions with stand-in holders and the check on recordings cut down by hand.
- Two steps after the early end rest on the daemon and are read, not run: `zpool export` with a holder gone and cuts held (the pool does go on with a paused holder, which `bound` shows on every green run), and `end_daemon`'s "serve ended with 0 on SIGTERM", which a right daemon gives with cuts held (course (c) of `takeover`) and a daemon that has just refused a read may not. If it does not, the recording is the rig's error after all; the same was true of the path e805d10 opened.
- shellcheck is not installed here; CI's run of it on this head is green (the `ci` workflow).

### Evidence

All in `review/m2.t3-2c/` on `skyzfs-review`. New for this look: `next2_holder_ends.sh` with `next2_holder_ends-ab34565.out` and `-e88773b.out`; `next2_verdict.py` with `next2_verdict-ab34565.out` and `-e88773b.out`; `next2_plants.py` with `next2_plants-ab34565.out`; and the reruns `next_scenarios-ab34565.out`, `next_third_instance-ab34565.out`, `next_rejudge-ab34565.out`, `next_torn-ab34565.out`.

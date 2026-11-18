# #119, the last look at 4aac932

Pull request: https://github.com/skyzfs/skyzfs/pull/119. Head 4aac932, one commit on cfe8146 (the head of `next_look.md`). I read the commit's diff before the answer (issuecomment-6979162793).

**Verdict: it can merge. No blocking finding.** What is left is three small points on tests and one optional pause, below.

CI on 4aac932: 49 checks green, 4 skipped, none failed. Codex reviewed the head and has no finding. The head's Python and shell tests pass here (`test_cut_watch.py` 12, `test_cut_daemon_main.py` 38, `test_cut_daemon_check.py` 99, `test_cut_hold.py` 18, `test_cut_plan.py` 8, `test_cut_record.sh`).

## F1: the status against the row (`before_other`, cut-record.sh:1031-1063)

The check is right as written: "lower than", not "not equal".

- **Why lower.** The status is read first and the row after it. A status that says idle, nothing held, cut group `cg` says that cut was let go. Either it was uploaded, and then the row read afterwards names its group or a later one, or it was abandoned (a mismatch, a lost lease), and then `cg` is above the row's group for good. So a status older than its row is exactly `cg` below the row's group, and `cg` above it is a state the judge has to see, not a stale look.
- **A status ahead of its row passes at once and does not loop.** This is `urow`'s second act when the whole upload after `row1` did not come (`nudge_until ... || true`, line 1088), and any look after an abandoned cut. With "not equal" those would be looked at five times and end as the rig's error, where the judge should give the verdict. I ran `test_cut_record.sh`'s stand-in with the status at group 41 and the row at 40 (the test's `cg=39` changed to `cg=41` in a copy): the head takes the look at once ("fetched st 1 40 after 0 looks again"); with `-ne` in a copy of the script the same look is taken five times and ends with exit 2.
- **I found no case where `cg` stays below the row's group in a right run.** `before_other` runs in the first instance only, after `load`, in all five places (lines 1072, 1086, 1090, 1240, 1304). The other writer's rows keep head's group (`act_row1`, `act_row2`) or leave the row (`act_move`). A status with no `cut_group`, or one that is no number, skips the check.
- **The bound.** The `continue` is inside `for _k in 1 2 3 4 5` (line 1033), so stale looks and moved rows share five tries, then `rig_error` (line 1062), exit 2. A status that is never rewritten gives five looks and the rig's error: no hang, and no verdict against the daemon.
- **Small, optional: no pause before `continue`.** Each look is a sync, a drain and node B's read. With the pool exported (`ulost`, `ufailed`) there is no sync, and five looks could pass in a few hundred milliseconds. The daemon rewrites the status within 100 ms, so five should do; `sleep 1` before the `continue` at line 1044 would make it independent of how fast the looks are. Where it fell short the result is exit 2, not a wrong verdict.
- **What the check cannot see**, as in `next_look.md`: a stale status whose cut's commit has not landed when the row is read (the row then equals the status). In `umoved` and `urow` the guarded act ends with 6 and the look is taken again. In `ulost` it is G3's case (a commit in flight when the lease runs out), which the description states as not done. Not new.

On the kernel (run 44301210684, the upload job): no `looks-again.txt` and no `acts-again.txt`; the four looks have the status's cut group equal to the row's (14/14, 14/14, 33/33, 27/27), with drains of 1 to 4 s.

## F2: the guard's control (`other_act`, cut-record.sh:1104-1123)

Sound. Before each act the same act is made on the row with the number one past (line 1113); anything but 6 is the rig's error (line 1114). So an act that does not pass `--expect-row`, or a node B that ignores it, now fails every run of `umoved` and `urow`: R41, R42 and node B's N28, N30, N31, which no local test catches, are caught on the kernel.

- The control changes nothing a judge reads: refused, `act_move` and `other_row` return before `others.tsv` is written, and the real act writes `moved.txt` or `set-row.txt` over the control's. For `move-head` a refused control leaves the objects of a commit that head never names.
- Run 44301210684: three controls, all refused with 6 (`guard-controls.txt`: `moved ...:9:14`, `row1 ...:9:14`, `row2 ...:15:33`); `umoved` and `urow` checked with exit 0.
- **Small.** The state in the row is the volume's and does not change from cut to cut, so the control's row `state:n+1:g` is the daemon's next row if the daemon commits, between the look and the control, a second cut of the same group. Then the control acts and the rig says "the guard is not wired" (exit 2, the safe side). Two cuts of one group need a second root for one group, which the reference's recordings show only after a rewind import (`cut_check.py`, the table's line for the rule `txg-alone`); these courses do none. A control with group 0, which no cut has, could not meet a real row. Not blocking.
- The control is one more call of node B between the look and the act, so the window G2 closed is a little wider; it is still closed at the store and bounded by five acts.

## F4, F5, F6

- **F4.** `--no-renames` is in the workflow's diff (line 127). A dispatch whose `images` is not a non-empty JSON list of names ends with one line and exit 1 (cut_plan.py:60-65, 87). Plants P30 to P34 are all killed by `test_a_dispatch_with_no_images`. The path list is with the lead.
- **F5.** `test_cut_watch.py`'s class `Main` runs the watcher as a command: W18, W19, W20 and W38 are killed.
- **F6.** W02a, J49 and J50 are killed.
- **F3** is in the description.

## Plants at 4aac932

`plants3.py`, results in `plants3.out`: 34 plants, 22 killed, 12 survive, all applied. The checkout was clean afterwards.

| Where | Plants | Killed | Survive |
|---|---|---|---|
| Survivors of cfe8146 the answer names, and their neighbours | 16 | 7 (W02a, W18, W19, W20, W38, J49, J50) | 9 (W04, W05, W22, J42, R27, R28, R30; R41 and R42 are kernel only) |
| `before_other`'s status check (R51 to R57) | 7 | 6 | 1 (R52) |
| `other_act`'s control (R60 to R65) | 6 | 4 | 2 (R63, R65) |
| The plan's `images` (P30 to P34) | 5 | 5 | |

The three new survivors, none blocking:

- **R52** (`-lt` made `-ne`): no case in `test_cut_record.sh` has a status ahead of its row. One call of `looked` with the stand-in status at 41 would hold the direction of the comparison, which is the point of the check.
- **R65** (the control made before the first act only): the test counts the lines of `guard-controls.txt`, not the calls of its stand-in act.
- **R63** (the control may end with anything but 0): the stand-in act ends with 0 or 6 only. On the kernel `guarded` already makes any other exit the rig's error.

Still open as the answer says, and not blocking: N08, J42, `ulease`'s `release-row.txt`, G1, G3, the rest of G6. Node B's source is unchanged since cfe8146, so its plants were not run again.

## Files

Under `review/m2.t3-3/` on `skyzfs-review`: `next_look2.md` (this), `plants3.py`, `plants3.out`. The earlier looks' files are beside them.

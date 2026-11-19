# Fourth look at #130 at a3b37ba: M1 to M4 and the nit

This looks at pull request skyzfs/skyzfs#130 at head `a3b37ba`, into
`zfs-topology`. The merge base is still `d646117`, and GitHub reports the
pull request mergeable. The only commit since the third look
(`next_look2.md`, at `9710a24`) is `a3b37ba`: "one zdb signal rule on the
stopped path; busy exports kept". The author's answer is issue comment
6999392149; I checked each claim in it against the code and by running
it.

Scenarios are in `next_scenarios3.py`, with output in
`next_scenarios3.out`. Plants are in `next_plants3.py`, with output in
`next_plants3.out`.

## Verdict

**No blocking finding: it can merge** once the rigs still running on
`a3b37ba` are green.

- The replay rig is green: both layouts and every job through skyzfsd.
- Five jobs of the daemon rig and the cut rig were still running when I
  wrote this. Section (7) lists them.

- M1, M2, M3, the N3 count of M4 and the nit are fixed, each with a test
  that kills the old code planted back.
- Every hunk of `9710a24..a3b37ba` is one of those fixes or its test.
- Three plants still survive. Each is a test gap with no effect on
  behaviour, so none blocks:
  - L04: `export_forced` in the results on a normal end;
  - L06 as I wrote it;
  - L14: where the `Rig` is kept.
- One correction about L06. The author is right about the plant that my
  words in `next_look2.md` described. The plant I actually wrote is a
  different one, and it can change the summary (see (5)).

## (1) The diff, hunk by hunk

`git diff 9710a24 a3b37ba` touches four files, with 91 lines added and
12 removed.

| File | Hunk | For |
|---|---|---|
| `replay_check.py` | The comment at `ZDB_ABORT` now says each rig keeps its own rule for its errors: in the reference jobs exit 2 with a pool left imported, else 1; in the daemon jobs exit 2 whenever nothing stands against the daemon, in `verdict` and on the stopped path | the nit (comment only) |
| `replay_daemon.py` | `tally`: `== rc.ZDB_ABORT` in place of the copied text | M3 |
| | `Checker.deep`: `self.rig.at = p['n']` as its first line | M2 |
| | `classify`: `== rc.ZDB_ABORT` | M3 |
| | `check()`'s stopped path: `not rc.zdb_rig_code(zcc)`; `export_busy` and `export_forced` from `st['rig']` into the results; `rc.rig_lines(busy)` into the summary | M1, M2 |
| | `_check`: `st['rig'] = ck.rig`, right after the `Checker` is made | M2 |
| `test_replay.py` | The N3 test's forced exports now exit 0, 1, 0, giving "exported 2 and left 1" | M4 (G08) |
| `test_replay_daemon.py` | `test_zdb_signals_in_the_daemon_judge`: signals 6 and 9 through `classify` and `tally`, and no copy of the text left in the module | M3 |
| | `test_a_deep_check_names_its_own_prefix`: the real `Checker.deep` with `rig.at` on the next prefix | M2 |
| | `test_busy_exports_are_kept_when_the_rig_stops`: a busy export that went to `-f`, then a stop in the withheld control; checks the results, the line, and that the summary does not say a pool was left | M2, M4 |
| | `test_busy_exports_reach_the_results_and_the_summary`: also checks that the summary does not say a pool was left | M4 |
| | `test_a_timed_out_zdb_at_the_end_in_a_stopped_run_is_the_rigs`: signals -9, -11 and -7 give 2 on the stopped path, and -6 gives 1 | M1 |

The diff contains nothing else. The reference rig's logic, the
workflows and the shell scripts did not change.

## (2) M1: zdb -bcc on the stopped path

Y1 ends zdb -bcc with each code and then stops the run on the stuck
control's `RigError`. It compares that with the same code on a normal
end.

| zdb -bcc rc | normal end | stopped path | the stopped summary |
|---|---|---|---|
| 0 | 2 | 2 | the rig's error only |
| 1 | 1 | 1 | "zdb -e -bcc ... failed ... a verdict against the daemon" |
| 124, 127 | 2 | 2 | the rig's error only |
| -6 | 1 | 1 | a verdict against the daemon |
| -7, -9, -11, -15 | 2 | 2 | the rig's error only |

The two paths now agree at every code, as the lead decided: an abort
counts against the daemon, and a kill or crash is the rig's.

On a normal end, a 0 gives exit 2 here because the scenario also hands
`verdict` the stuck control's rig error.

The plants of the stopped path are all killed:

- K12r: `RIG_RC` back;
- K13: any nonzero exit against the daemon;
- K14: never against the daemon;
- K15: SIGABRT counted as the rig's;
- K16: zdb not counted in the exit;
- K17: only SIGKILL counted as the rig's.

## (3) M2: the deep check's prefix, and the record on a stopped run

**The deep check's prefix.** Y2 runs the main loop of the fixture with
the real `Checker.deep`, with the Rig's scrub replaced by one busy
export. All seven deep checks name their own prefix: (0, 0), (13, 13),
(14, 14), (24, 24), (25, 25), (35, 35), (36, 36). At `9710a24` it was 3
of 7 wrong. The test calls the real `Checker.deep` directly. L07r (the
line removed) and L08 (the line after the scrub) are killed.

**The record on a stopped run.**

- **Y3: a stop in a control.** A busy export in each deep check, one of
  them forced, then the stuck control stops the run. The result is exit
  2, with `export_busy` and `export_forced` in the results and the "Busy
  exports" line in the summary. The summary does not say a pool was left
  imported. (The line counts one export across 7 refused attempts only
  because the scenario calls `held` without `export`.)
- **Y3c: a stop in the main pass.** At prefix 18, a cleanup left
  something behind (`abort`). The result is exit 2, with the record and
  the line kept.
- **Y3b: no `Checker` was made.** The run stopped before one existed.
  The result is exit 2 with no record and no error, through the
  `if rig is not None` guard.

Plants:

- L09 (no `Rig` in `st`), L10 (no record), L11 (no line), L12 (forced
  dropped) and L13 (the stopped summary told exit 2) are killed.
- **L14 survives.** It keeps the `Rig` only once the main pass has
  ended. The new test stops in a control, after the main pass, so the
  placement of `st['rig'] = ck.rig` is pinned by no test. The code is
  right: Y3c shows a stop in the main pass keeping the record. A test
  that stops in the main pass would pin it. This does not block.

## (4) M3: one `ZDB_ABORT`

`classify` and `tally` now use `rc.ZDB_ABORT`. Y4 runs every failure
that the every-copy check can write through both:

| Failure | `classify` | `tally` |
|---|---|---|
| signal 6 | failure | catch (`copies`) |
| signal 9, 11 or 7 | rig error | rig error |
| `rig: zdb timed out` | rig error | rig error |
| `copies: zdb` | failure | catch (`copies`) |

This is the reference rig's rule. K10d to K10h are all killed:

- the abort left out of `classify`, and left out of `tally`;
- the text copied back, which the test's look at the module's source
  catches;
- every signal counted, in `classify` and in `tally`.

## (5) M4, and the claim about L06

- **G08** (the failed `-f` counted as exported): killed by the new
  0, 1, 0 case.
- **L04** (`export_forced` written as `[]` on a normal end): **still
  survives.** The author's answer says L04 is caught. It is caught on
  the stopped path, where L12 is killed, and when the key is left out of
  the normal end's results. But the normal-end test has no forced
  export, so writing `[]` in place of the list passes. One forced export
  in `test_busy_exports_reach_the_results_and_the_summary` would pin it.
  This does not block.
- **L06.** The answer says that "as planted it cannot change anything",
  because the daemon's results key the exit as `exit`. That holds for
  passing the whole results. Y5 checks it:
  `rc.rig_lines(res)` on a normal end with exit 2 prints no exit-2
  line. But I described L06 in `next_look2.md` as "passes its whole
  results". The plant itself maps the key:

  ```python
  out.extend(rc.rig_lines(dict(res, exit_code=res.get('exit'))))
  ```

  That form does fire. Y5 runs `test_a_rig_stage_in_the_main_pass_is_the_rigs_error`'s
  case, a normal end with exit 2, with it planted. The summary then says
  a pool was left imported. It survives, because no test of a normal
  end with exit 2 reads the summary. The stopped-path form (my L13, the
  author's L06b) is caught.

  The plant is contrived, my words misdescribed it, and the hazard the
  author describes is covered. I raise no finding. One `assertNotIn` in
  that test would close it.

## (6) Plants

`next_plants3.py` ran 140 plants on a clean worktree at `a3b37ba`. Each
plant in either judge ran both judges' tests. The worktree was clean
after the run.

The set is the third look's plants, with these changes:

- The plants whose code `a3b37ba` changed are turned round:
  - K12 and L07 had applied the fixes. As K12r and L07r they now put the
    old code back.
  - K10, K10b and K10c become K10d to K10h, on the new text.
  - K13 and K14 are on the new text.
- New plants: K15 to K17 and L08 to L14.

| | Plants | Killed | Survived |
|---|---|---|---|
| Carried over unchanged | 121 | 117 | B06, E09, L04, L06 |
| New or turned round | 19 | 18 | L14 |
| All | 140 | 135 | 5 |

None was left unapplied.

- B06 and E09 survive as in every look since the second. I reported them
  as not blocking.
- C01 to C06, G06 to G10, H05 to H07, K01 to K09 and K11 stay killed.

## (7) Local gates, CI at a3b37ba, Codex

**Locally**, on a clean worktree:

- `test_replay.py`: 53 tests, OK.
- `test_replay_daemon.py`: 54 tests, OK.
- `test_replay_hole.py`: 24 tests, OK.
- Every `test_*.py` under `tests/kernel`, `tests/lib` and `tests/bench`
  passes, and so does every `test_*.sh`.
- No shell file changed.

**CI at `a3b37ba`**, as of 04:42 UTC on 2026-11-19, about 34 minutes
after the push.

Green:

- `skyzfs-ci` (the push), `ci` (with `unit-arm64`), `coverage` and
  `e2e`;
- `skyzfs-nodeb-rig`;
- `skyzfs-kernel-probe`: all three jobs;
- `skyzfs-cut-daemon`: 26.04, 26.04-arm and 26.04-upload;
- `skyzfs-cut-rig`: the reference, 26.04 and 26.04-arm;
- `skyzfs-daemon-rig`: the daemon on 26.04 and on 26.04-arm;
- `skyzfs-replay-rig` (run 44456214186): every job. I read its
  summaries:

| Job | Exit | zdb -bcc | Controls | Busy exports |
|---|---|---|---|---|
| product layout (`replay-rig-slog`) | 0; every checked prefix imported and verified | 0 | data, drop, reorder as expected; intent not built, as expected | 6 (prefixes 58, 103, 168, 859, 3474, 3481), each exported when tried again after 1 s; none forced |
| whole-pool layout (`replay-rig-whole`) | 0; every checked prefix imported and verified | 0 | data, drop, intent, reorder as expected | 3 (prefixes 238, 676, 1070), each exported after 1 s; none forced |
| through skyzfsd, 24.04 | 0; 123 of 2218 prefixes (the pull-request budget), every checked prefix passed | 0 | stuck, withheld as expected; the rest not built, as expected | none |
| through skyzfsd, 26.04 | 0; 120 of 2101 prefixes, every checked prefix passed | 0 | | 1, at prefix 1518 (its own check, not a deep one), exported after 1 s; none forced |
| write into a hole, 24.04 and 26.04 | success | | | |

Still running at 04:42 UTC:

- `skyzfs-daemon-rig`: the daemon on 24.04, and the stand-in on 24.04
  and 26.04;
- `skyzfs-cut-rig`: 24.04 and 24.04-arm.

Neither rig drives code that this commit changed:

- The daemon rig runs on any change under `tests/kernel/`.
- The cut rig has not changed since the first look.

All five were green at `9710a24`.

**Codex** shows a code review and a security review, both completed on
`a3b37ba`, with no findings and no unresolved ones.

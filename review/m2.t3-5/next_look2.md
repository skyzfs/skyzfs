# Third look at #130 at 9710a24: N1, N3, the lead's rule on zdb's signals, S7

This is pull request skyzfs/skyzfs#130 at head `9710a24`, into
`zfs-topology`. The merge base is still `d646117`.

The base has moved since then. `zfs-topology` is at `cc141b1`, seven
commits ahead: #126, the uploader. None of those commits is under
`tests/kernel/`, and GitHub reports the pull request as mergeable.

New since the second look (`next_look.md`, at `09860a0`):

| Commit | What it does |
|---|---|
| `242a591` | N1 (the cap is gone), N3 (the wording) and S7 (the daemon jobs record busy exports) |
| `40f7b0d` | Pins zdb's signals in `exit_code` |
| `0c4b691` | Explains why any SIGABRT counts |
| `9710a24` | One rule for zdb's signals, `zdb_rig_code`, in both rigs |

All of this is test code under `tests/kernel/`. I read the code first,
then checked each claim in the author's answer (issue comment
6998599929) against the code and by running it.

Scenarios: `next_scenarios2.py` (output in `next_scenarios2.out`).
Plants: `next_plants2.py` (output in `next_plants2.out`).

## Verdict

**No blocking finding. The pull request can merge.** Every rig at
`9710a24` is green, the product and whole-pool layouts included (see
(8)).

- N1 and N3 are fixed. The lead's rule on zdb's signals is built as
  decided, at every site but one.
- S7 records busy exports and summarises them. On the real kernel, both
  daemon jobs at this head show the line.
- I recommend one fix in this pull request, M1: one line and one test.
  Without it, the daemon jobs judge zdb's signals by the old rule on one
  path, which contradicts the lead's "both rigs, main pass and end".
- M2 to M4 do not block. They can go in this pull request or in N2's
  follow-up.

| | What | Certainty | Blocks |
|---|---|---|---|
| N1 | Fixed. Every busy export waits through `EXPORT_WAITS`, however many went to `-f` before it | driven (Z4); plants H05 to H07 killed; seen in CI | settled |
| N3 | Fixed. The line says how many `-f` exported and how many it left imported | driven (Z4) | settled. One test gap (M4) |
| N4 | Built as the lead decided. Signal 6 counts against the image everywhere. Signals 7, 9, 11 and 15 are the rig's in the main pass and at the end, in both rigs. No job moves to or from a pass | driven over every code (Z1, Z2) | settled, except M1 |
| S7 | Done. The record reaches the daemon jobs' results and summary, with the prefix set before each image is made | driven; seen in CI | settled, except M2 |
| M1 | `replay_daemon.check()`'s path for a run that stops on the rig's error still judges zdb -bcc by `RIG_RC`. A zdb killed by signal 9 or 11 there reads "a verdict against the daemon" (exit 1), where the normal end says it is the rig's (exit 2) | driven (Z3) | no, but fix it in this PR |
| M2 | S7's prefix is wrong for a deep check of the previous group's last prefix: 3 of 7 records name the next prefix. A run that stops on the rig's error keeps no record of busy exports at all | driven (Z5, Z6) | no |
| M3 | The daemon judge's own rule for signal 6 has no test: in the main pass (`classify`) and in its controls (`tally`). It also repeats `ZDB_ABORT`'s text instead of using it | plants K10b and K10c survive | no |
| M4 | Three small test gaps: the N3 count, `export_forced` in the daemon jobs, and the daemon summary's exit-2 line | plants G08, L04 and L06 survive | no |

N2 has moved to a follow-up, as the lead agreed. I did not look at it
again.

## (1) `zdb_rig_code`: every exit code, both rigs, main pass and end

```python
def zdb_rig_code(rc):
    return rc is not None and (rc in RIG_RC or (rc < 0 and rc != -6))
```

Its users:

- `replay_check.exit_code`, at the end;
- `replay_daemon.verdict`, twice: what stands against the daemon, and
  the rig's line.

The main pass did not change. `every_copy_verdict` already wrote
`stage('zdb', rc)` for 124 and 127, `rig: zdb died with signal N` for a
negative code, and `copies: zdb` (a judging stage) for a positive
nonzero code. Both judges then count signal 6 alone against the image.

Z1 runs both judges at `09860a0` ("old") and at `9710a24` ("new"), each
in its own subprocess, over every code asked about and a few more. An
arrow marks a change. "ref, left" means the reference judge with a pool
left imported and nothing else against the image. "ref, no left" means
the reference judge with no pool left. "daemon" means the daemon judge
with nothing else against.

| rc | rig code | end: ref, left | end: ref, no left | end: daemon | main pass: failure written | main: ref, left | main: daemon |
|---|---|---|---|---|---|---|---|
| 0 | no | 2 | 0 | 0 | none | | |
| 1, 2 | no | 1 | 1 | 1 | `copies: zdb` | 1 | failure |
| 124 | yes | 2 | 1 | 2 | `rig: zdb timed out` | 2 | rig |
| 127 | yes | 2 | 1 | 2 | `rig: zdb could not start` | 2 | rig |
| 134, 135, 137, 139 | no | 1 | 1 | 1 | `copies: zdb` | 1 | failure |
| -6 | no | 1 | 1 | 1 | `rig: zdb died with signal 6` | 1 | failure |
| -7, -9, -11, -15 | yes | **1->2** | 1 | **1->2** | `rig: zdb died with signal N` | 2 | rig |

`zdb_rig_code(None)` is False. The daemon judge gives
`(2, ['zdb -bcc did not run on the final image'])`, unchanged.

Z2 shows how `run()` returns a signal, using `subprocess.run`:

- A command killed by signal N comes back as -N.
- 134, 137, 139 and 135 come only from a shell or another wrapper that
  passes on its child's death.

Every zdb call in both rigs is `run(['zdb', ...])`, with no wrapper:

- `replay_check.py`: 1382, 1599, 1643 and 1654;
- `replay_daemon.py`: 643 and 942.

So 134 to 139 do not occur today. If zdb ever ran under `sudo`,
`timeout` or `lim`, a SIGKILL would read 137, and that would count
against the image. Treating `rc > 128` as `-(rc - 128)` would cover it,
but this is optional.

**Can an image's fault become a rig stage?** Yes, by design. A zdb that
crashes with SIGSEGV or SIGBUS while reading a damaged pool reads as the
rig's error. The lead chose this, and the comment at `ZDB_ABORT` says
so. The job still fails: exit 2 with a pool left, else 1, and always 2
in the daemon jobs. It is never 0, so nothing passes silently.

**Can a rig's fault become a verdict on the image?** Only on the daemon
judge's stopped path (M1, below), and through a wrapper, which no call
uses.

**The end-pass zdb -bcc killed by a signal: did its exit change, and is
that intended?** It changed, as the lead decided:

- With signal 6 it keeps exit 1 in both rigs.
- With any other signal it now gives 2:
  - in the daemon jobs, whenever nothing else stands against the daemon;
  - in the reference jobs, only with a pool left imported. With no pool
    left it stays 1. `job_verdict` still lists "zdb -bcc on the final
    image: exit code -9" among the reasons, and that is true as fact.

The two rigs differ because each keeps its own rule for the rig's
errors, as they did for 124 and 127 before this pull request:

- The reference judge gives exit 2 only with a pool left.
- The daemon judge gives exit 2 for any rig error when nothing stands
  against the daemon.

The comment at `ZDB_ABORT` gives the reference rule ("exit 2 with a pool
left imported, else 1"). The comment in `replay_daemon.verdict` points
to it, and so does the author's answer ("in both rigs … 1 without"). For
the daemon jobs it is 2 with or without a pool. This is a wording nit
that can go with M1.

**Did a job that used to fail now pass, or the reverse?** No. Every move
is from 1 to 2, and both fail the job:

- The daemon jobs' step maps any exit other than 0 or 1 to `exit 2`
  ("the rig itself failed").
- The reference step passes the rig's exit through.

Nothing moves to or from 0, and the main pass is unchanged at every
code.

## (2) The daemon jobs' verdict, and M1

`verdict()` is right at every code (Z1). Its rig line now reads "did not
finish (exit code -9: killed by signal 9)". `test_exit_codes` pins this:
plants K06 to K09 are killed.

**M1.** `check()` has a second judge for a run that stops on the rig's
own error, for example the stuck control. It did not change. At
`replay_daemon.py:747`:

```python
zdb_against = zcc not in (0, None) and zcc not in rc.RIG_RC
```

Z3 makes zdb -bcc end with each code, then raises the stuck control's
`RigError`:

| zdb -bcc rc | stopped path | the same rc on a normal end |
|---|---|---|
| -9 | exit 1, "a verdict against the daemon" | 2 |
| -11 | exit 1, "a verdict against the daemon" | 2 |
| -6 | exit 1 | 1 |
| 124 | exit 2 | 2 |
| 0 | exit 2 | 2 |

So the same zdb result counts against the daemon or not depending on
whether a later step stopped the rig. This is the case the coordinator
asked about: a rig's fault becoming a verdict on the image. The job
fails either way, so this is a matter of labelling, not a false pass.

The fix:

```python
zdb_against = zcc not in (0, None) and not rc.zdb_rig_code(zcc)
```

Add a case for -9 to `test_a_timed_out_zdb_at_the_end_in_a_stopped_run_is_the_rigs`.
Plant K12, which applies this fix, survives: no test holds the stopped
path to either rule. K13 and K14 show that the path's other two
behaviours are pinned.

## (3) The summary line (N3)

Z4, with the stand-in `zpool` (the output is cut off in
`next_scenarios2.out`):

- "Busy exports: 3 (11 refused attempts), 2 of them still busy after
  every wait and sent to -f, which exported 2 and left 0 imported (-f
  exit codes 0, 0)."
- "Busy exports: 1 (5 refused attempts), 1 of them still busy after
  every wait and sent to -f, which exported 0 and left 1 imported (-f
  exit codes 1)."

These are right.

Plants G07, G09 and G10 are killed. **G08 survives** (it counts the
failed `-f` runs as exported). The test's two forced exports have exit
codes 0 and 1, so counting either way gives "exported 1 and left 1". A
case of 0, 0 or 0, 0, 1 would catch it (M4).

## (4) N1: every busy export waits

Z4 runs two exports that stay busy through every wait (each then
exported by `-f`), then a third that is busy until the first wait.

The third waits one second and exports. All three exit codes are 0, and
no pool is left imported. With the cap of the second look, the third
would have gone to `-f` at once, been refused, and left the pool
imported.

Plants:

- H05 (the cap, put back) is killed.
- H06 (only the first export waits) is killed.
- H07 (a refusal on the last try is not recorded) is killed.

The comment at `EXPORT_WAITS` now matches OpenZFS 2.2.2's
`spa_export_common` as I read it in the second look.

On the kernel at this head, each daemon job had one busy export, and it
cleared on the first one-second wait:

- 24.04: export 49, at prefix 1607;
- 26.04: export 7, at prefix 4.

Both summaries say "each exported when tried again". Neither had a
mounted file system or a ZFS command running.

## (5) S7: the daemon jobs' record, and M2

`Checker.point` sets `self.rig.at = n` before `daemon.make`. `_check`
writes `export_busy` and `export_forced` to the results, and `summary()`
adds `rig_lines` for those two keys. Both daemon jobs at the head show
it (above).

In both CI cases, the prefix recorded is the prefix whose own check met
the busy export. Neither was a deep check run after the next prefix was
made.

Plants L01, L02, L03 and L05 are killed. The test pins the prefix by
source order, not by driving it: it checks that `self.rig.at = n` comes
before `self.daemon.make(` in the source of `point`.

**M2.** Two problems:

- **The deep check's prefix.** When a transaction group changes, the
  main loop runs `ck.deep(prev)` after `ck.point(n)`, so `rig.at` still
  names n. Z5 records a busy export in every deep check of the fixture.
  It got (prefix deep-checked, prefix recorded) = (0, 0), (13, 14),
  (14, 14), (24, 25), (25, 25), (35, 36), (36, 36): 3 of 7 name the
  next prefix. The fix is `self.rig.at = p['n']` at the top of
  `Checker.deep`. Plant L07, which applies it, survives, so the fix
  needs a test.
- **The stopped path.** `check()`'s except path builds its results from
  `st['res']`. `_check` adds `export_busy` only after `verdict()`, after
  every step that can stop the run. Z6 records a busy export in each
  deep check, then stops on the stuck control. The result is exit 2,
  with no `export_busy` in the results and no "Busy exports" line. A run
  that stops on the rig's own error is the one where the record matters
  most. The fix is to keep the `Rig` (or its two lists) in `st`, and to
  write them in both paths.

## (6) M3: the daemon judge's copy of `ZDB_ABORT`

`replay_daemon.py` checks `r['failed'] == 'rig: zdb died with signal 6'`
in two places:

- `classify`, at line 719: a failure in the main pass;
- `tally`, at line 270: a catch in a control.

Both are text copies of `replay_check.ZDB_ABORT`. Neither has a test:

- Plant K10b (`classify` without it) survives.
- Plant K10c (`tally` without it) survives.

Both lines are older than this pull request (`b8a8d44`). But the
author's answer says that planting "-6 as the rig's" is caught in both
rigs. In the daemon jobs that is true at the end only. If
`ZDB_ABORT`'s text ever changed, the daemon jobs would quietly count
signal 6 as the rig's.

The fix is `rc.ZDB_ABORT` at both sites, with a test that puts
`rig: zdb died with signal 6` and `... signal 9` through `classify` and
`tally`. It does not block.

## (7) Plants

`next_plants2.py` ran 128 plants, each against `test_replay.py` and
`test_replay_daemon.py`, on a clean worktree at `9710a24`. The worktree
was clean after the run.

| Group | Plants | Killed | Survived | Not applied |
|---|---|---|---|---|
| The second look's, on the new text | 98 | 96 | B06, E09 | |
| New: H05 to H07, G07 to G10, K01 to K14 with K10b and K10c, L01 to L07 | 30 | 22 | G08, K10b, K10c, K12, L04, L06, L07 | K10 |

About the second look's plants:

- X07 was replaced by X07b and X07c in the second look.
- G05 and H01 to H04 were dropped, because the code they changed is
  gone.
- B06 and E09 survive, as they did in the second look, where I reported
  them as not blocking.
- **C02 and C06 are now killed**, as the author says.

About the new plants:

- K10 was not applied because its text occurs in two places. It was redone
  as K10b and K10c.
- K12 and L07 are the fixes for M1 and M2. They survive as expected,
  which shows that no test holds the current behaviour there.
- G08, K10b and K10c are M4 and M3.
- L04 (`export_forced` left out of the daemon's results) survives,
  because the test's run has no forced export (M4).
- L06 (the daemon summary passes its whole results, so a daemon exit 2
  for any reason would print the "pool left imported" line) survives,
  because no daemon test with exit 2 checks the summary for that line.

## (8) Local gates and CI at 9710a24

Locally, on a clean worktree:

- `test_replay.py`: 53 tests, OK.
- `test_replay_daemon.py`: 51 tests, OK.
- `test_replay_hole.py`: 24 tests, OK.
- Every `test_*.py` under `tests/kernel`, `tests/lib` and `tests/bench`
  passes, and so does every `test_*.sh`.
- This round changed no shell file.

CI at `9710a24`: every run is green.

- `ci` (with `unit-arm64`), `coverage`, `e2e`, `skyzfs-ci` (the push).
- `skyzfs-kernel-probe` and `skyzfs-nodeb-rig`.
- `skyzfs-cut-rig`: four images and the reference.
- `skyzfs-cut-daemon`: 26.04, 26.04-arm and 26.04-upload.
- `skyzfs-daemon-rig`: the daemon on 24.04, 26.04 and 26.04-arm, and
  the stand-in on 24.04 and 26.04.
- `skyzfs-replay-rig` (run 44450250278): every job.

The replay rig's summaries:

| Job | Exit | zdb -bcc | Controls | Busy exports |
|---|---|---|---|---|
| product layout (`replay-rig-slog`) | 0; every checked prefix imported and verified (110 of 110 flush boundaries, 3076 of 3076 near an uberblock write) | 0 | data, drop, reorder: as expected; intent: not built, as expected | 2 (prefixes 56 and 1274), each exported when tried again after 1 s; none forced |
| whole-pool layout (`replay-rig-whole`) | 0; every checked prefix imported and verified | 0 | data, drop, intent, reorder: as expected | 4 (prefixes 37, 189, 1401, 1478), each exported after 1 s; none forced |
| through skyzfsd, 24.04 | 0; 126 of 2272 prefixes (the pull-request budget); every checked prefix passed | 0 | | 1 (prefix 1607), exported after 1 s |
| through skyzfsd, 26.04 | 0; 124 of 2227 prefixes; every checked prefix passed | 0 | | 1 (prefix 4), exported after 1 s |
| write into a hole, 24.04 and 26.04 | success | | | |

So busy exports are common on the real kernel: one to four a job, and
every one cleared on the first wait. That fits N1's reading: what holds
the pool is short-lived, and the waits clear it.

The daemon jobs' "Every record prefix" line says that the pull-request
budget does not show design §9's criterion. That is as intended.

## (9) Codex

Codex's summary comment shows a code review and a security review,
both completed on `9710a24`. It reports no findings and no unresolved
finding.

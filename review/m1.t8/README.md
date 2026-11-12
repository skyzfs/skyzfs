# M1.T8 (skyzfs/skyzfs#30): the daemon rig's checkpoint pins

Reviewed: `origin/zfs-topology` 4b1868d ... `origin/skyzfs-testing` 06dce94.

## What was done

- Read the diff of daemon-rig.sh, daemon_check.py and its test,
  blkcheck-helper.sh, lim.sh and its test, the workflows, and the
  small changes to topology-bench.sh and replay-rig.sh.
- Ran test_lim.sh (passes; 2 of its cases fail on the old lim.sh) and
  test_daemon_check.py (36 pass).
- Ran the rig's functions takeover_pin, log_bound_pin, unflushed_said
  and unflushed_obs alone with made-up inputs.
- Read the daemon's rule of the marks, its status lines and the step
  pause hook for what the pins rest on.
- Not done: no kernel run; no artifact read (the description's numbers
  are the author's, of 52a1be5); the pull request's own rig run on
  06dce94 was still running; shellcheck is not installed here; check
  f's courses and check a's two new pins read once.

## Findings

1. `pin_c_suite_with_low_marks` can pass with no step during any call
   of the suite. The pre-fill of 130 MiB crosses the high mark itself,
   inside `create`. The rule then steps down to the low mark (64 MiB
   at 8 MiB a step: eight steps) whether or not a request comes. A
   case writes far less than the 64 MiB it would need to reach the
   mark again, and a server started after a kill is below it. The
   guard is "one step seen in the pass". The description's counts are
   495 and 549 steps on 61 volumes: eight to nine a volume, which is
   the pre-fill's own drain. How much of the drain is still going at
   a case's first calls the rig does not say.
2. A status reading of a dead server has no `pid` line, so the
   sample's columns move one to the left and its log_live is dropped
   (the steps count is read as log_live). Run alone: 30 such readings
   over the bound and one good one give PASS. Little is lost (the
   live readings and replayed_bytes still see a lasting excess).
3. The take-over bound is loose: with a first median of 1035 ms the
   last five may come to 3070 ms. A take-over that triples passes.
   The log's bound and replayed_bytes are what guard the cause.
4. Check b: no judged line became an observation. Two observations
   (the 300 s courses) are now made by dispatch only, and the
   controls' looks cover 20 s where they covered up to 300 s.
5. `obs_c_checked_reads` says "not recorded" without a rig error, and
   counts only each volume's last server.
6. The description is of 52a1be5; the head is 06dce94.

## Agreed with

- The log bound's arithmetic (136,335,360) and its "no step is the
  rig's error": the rule steps only from the high mark.
- The ten kills: each sent only with `PATH.reached` there, both files
  removed before the next serve, fewer than ten is ERROR.
- lim.sh under errexit and pipefail; the `sudo cp`.
- The judge's tables and tests: the four pins are judged on the
  daemon, WAITING there is a rig error, f is never a default.

## Second look (head 5db8604)

- 1 dropped. The pre-fill now stops within one 4 KiB write below the
  mark with no step taken, and each volume is classed from the trace
  and the helper's step counts. By `tend` and the worker's loop the
  rule is asked in the same turn as the request that crossed the mark,
  so a step comes before the next request. A wrong `inside` needs a
  first adding call that does not cross (a discard) and the crossing
  write as the case's last call: narrow. Every other doubt (a status
  100 ms old, a failed volume with no status) leans to `no_step`,
  which is the rig's error and not a pass. Not yet run on a kernel.
- 2 (Codex's): the writer's own status is returned; killed at the
  limit is told apart.
- 3 dropped: a dead server's reading keeps its columns (my made-up
  case now gives FAIL); the take-over bound is 1.5 times plus 500 ms;
  the checked reads' text says what it counts.
- test_daemon_check.py 40 pass, test_lim.sh passes.

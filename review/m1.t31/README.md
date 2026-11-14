# M1.T31 (the rigs and the benchmark without the log store), pull request 73

Head f3404e7 on 09959c2. Review posted on 2026-11-14 (issue comment
6871609829): nothing blocks; it can merge once its rigs read as they
should. `review-posted.md`.

- The twelve removed lines: eleven were NA on the in-place daemon at
  75c32a7 on all three images (`../m1.3t/rig-75c32a7/`); the twelfth
  (`ctl_f_starts_refused`) only ran when check f was named. On the
  stand-in, `pin_unstored_zero_a/b/e` and
  `pin_serve_after_kill_accepts_log` were PASS (no stand-in verdict in
  the base's table), not WAITING as the description says.
- The rule for builds from before M1.3t: all 13 commits that touch
  `bins/skyzfsd/src/cli.rs` from 3dd9288 (M1.3r) to the base write
  `[--store`; before 3dd9288 the file has no `--store`. A repeated
  option is refused ("given more than once", cd0e763's cli.rs), so a
  replay or hole dispatch with an old `daemon_ref` and `--store
  in-place` in `serve_args` breaks the first start.
- The kill test: a re-serve that does not come back is `KT_ERR` and
  ERROR (exit 2), while the orderly-stop test FAILs the same thing.
  Unchanged for the in-place daemon since the base; the PR removes
  `KT_REFUSED`. Put to the lead.
- `plants73.py` (six plants in the judge and g.4's rule): five fail a
  test; N5 (a name out of `NOT_ON_THE_DAEMON`) fails none, but the rig
  would write NA where PASS is expected, a rig error.
- All tests of tests/kernel, tests/lib and tests/bench pass at the head.
  shellcheck is not installed here.
- To do: the daemon rig (43555391150), the replay rig and the control
  dispatch with cd0e763 (43555427037) when they finish.

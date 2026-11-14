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

## The control with cd0e763 (dispatch 43555427037, job 132327252123)

Served with `--store in-place` ("the in-place store: the volume has failed"
in its log), checks g, ubuntu-24.04. Exactly the five expected lines
differ (pin_start_serves_what_the_disk_holds, pin_failed_start_fails_volume,
pin_failed_start_suspends_pool, pin_failure_reason_is_the_images,
pin_data_journal_refused); ctl_g_old_content_after_good_sync FAILs as
expected; the other g pins PASS; the judge exits 1. As a negative control
should.

## 11aa321 (testing's answers to the five suggestions)

All five answered in code. plants73b.py / plants73b.txt: ten plants in
kt_refusal, kill_verdict and skz_settle; nine caught. K5 (KT_REFUSED not
reset between kill tests) is not caught; the reset sits beside KT_ERR's on
one line, which no test holds either. Not asked for.

ci.yml `lint` red at 11aa321 (job 132331920595): shellcheck SC2329 on
test_copy_limit.sh:164-166, the directive names SC2317 only. Told testing.

## Second look posted (issuecomment-6872298388)

Can merge once ci.yml is green on ff93a91 (the SC2329 directive, nothing
else). Daemon rig at 11aa321 (run 43556886617), tables rebuilt from each
job's results.tsv and judged by this head's daemon_check.py: exit 0 on all
five. Daemon (24.04, 26.04, 26.04-arm): 47 PASS, 5 FAIL (controls), 2 NA,
1 WAITING; cases 1/13/12. Stand-in (24.04, 26.04): 30 PASS, 5 FAIL, 20
WAITING; cases 1/15/10. Each daemon table = 75c32a7's less the 11 NA lines
and obs_c_low_steps. rig-11aa321/ has the tables; table_nostore.py is
table.py for a judge without store_of.

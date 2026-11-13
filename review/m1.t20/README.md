# M1.T20 (skyzfs/skyzfs#45): the daemon rig drives `--store in-place`

`skyzfs-testing` at c9a3752 into `zfs-topology` (merge base b93f495, one
commit). Posted as
https://github.com/skyzfs/skyzfs/pull/45#issuecomment-6842412195.

**Verdict: nothing in the code blocks. Two findings, neither blocking. The
verdict to merge waits for the pull request's own daemon jobs (run
43353904493), which had not finished: the first run of the daemon rig on the
in-place store.**

Findings:

1. With the in-place store `check_b_delay` returns before the kill test with
   `--commit-delay-ms`, so the run is dropped and not only the count
   (`recovered_unacknowledged`, which this store never reports). What it
   exercises (applied, killed before the acknowledgement, issued again) is this
   store's rule too. Check c keeps its pass with the delay.
2. Three kept pins cannot fail on this store: `pin_unstored_zero_a` (the pump
   stores no position, `unstored` is always 0), `pin_serve_after_kill_accepts_log`
   (no check of what was acknowledged), and one clause of
   `pin_orderly_stop_under_load` (`recovered_unacknowledged` 0).

Checked:

- The rig's NA lines and the judge's `NOT_FOR_IN_PLACE`: the same eleven
  names, each under `in_place` (a regex over `daemon-rig.sh` against the
  table).
- Plants in `daemon_check.py`: NA accepted anywhere, two tests fail; the table
  applied to the log store too, nine fail; one name taken out of the table,
  all 48 pass (nothing without a kernel compares the table with the rig).
- Every `serve` of the rig passes `SERVE_ARGS`, the stand-in's included, which
  is why the stand-in takes `--store`.
- The benchmark's probe `skyzfsd help | grep -q -- --store` under `pipefail`:
  the help is one write of 6692 bytes, so the writer is never cut off; 400
  tries and 200 with a late reader, always found.
- The daemon refuses with in-place: `--log-high`, `--log-low`,
  `--writeback-kib`, `--home-writeback-kib`, `--readers`, `--step-pause-*`,
  `--refuse-holes` (`cli.rs:781-791`). `--fail-log-after`, `--commit-delay-ms`,
  `--hold-one-request` and `--recover-short` are taken. `fail_now` changes no
  block.

Not done: the rig and the benchmark were not run; the rig's 2,650 lines were
read only at `kill_test`, `check_b`, `check_b_delay`, `check_b_stop_under_load`
and `pin_unstored`.

## Second look: the run is in

Posted as https://github.com/skyzfs/skyzfs/pull/45#issuecomment-6842914384.

**Verdict: can merge at c9a3752.** Run 43353904493 is green in all six jobs.

- Both daemon jobs: `store in-place`; `c_declared` PASS 1, WEAKER 13,
  NOT_EXERCISED 12, as `C_DECLARED['daemon in-place']` says.
- NA on ten lines, each in `NOT_FOR_IN_PLACE`; the eleventh name
  (`ctl_f_starts_refused`) is check f's, which a pull request does not run.
- Take-overs on ubuntu-24.04: 20, from 263 to 267 ms from launch to the ready
  line, medians 264 and 265 ms. One run, one runner.
- The rig's step: about 25 minutes per image (the stand-in's about 36).
- `pin_failed_unheld_volume_serves_when_acknowledged` passes: the daemon is
  held to serving a failed volume again in the boot of a failure nobody held
  (bears on finding 1 of the M1.3s review).

Findings 1 and 2 kept, not blocking. Not read: the 26.04 job's check a and b
lines, the stand-in jobs' logs. The dispatch with `store=log` promised before
the merge is not in this run.

## Third look, at befb626 (2026-11-13)

Posted: https://github.com/skyzfs/skyzfs/pull/45#issuecomment-6843824113

Verdict: can merge; nothing new.

Runs 43359257961 (pull request, in place) and 43359260246 (dispatch,
`store=log`), green in every job. From the four daemon logs, both images:
in place 34 PASS, 13 NA, 4 control FAIL, the NA lines being the table's 14
names less `ctl_f_starts_refused`; with the log 44 PASS, 0 NA, 7 control FAIL.
`pin_kill_under_load_applied_unacknowledged` PASS in both;
`pin_unstored_zero_a/b/e` and `pin_serve_after_kill_accepts_log` NA in place
and PASS with the log; `pin_c_delay_cases` PASS, all 26 cases; `c_declared`
PASS 1, WEAKER 13, NOT_EXERCISED 12; the judge found no difference.
Take-over medians with the log: 24.04 1279 ms then 1535 ms, 26.04 802 ms then
803 ms; in place 520 ms and 282 ms.

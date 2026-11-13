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

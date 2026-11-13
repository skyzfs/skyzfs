# M1.T20a (skyzfs/skyzfs#44): the replay rig's serve options, the store always named

`skyzfs-testing-m1.t18` at d4d5b10 into `zfs-topology` (merge base b93f495).
Posted as https://github.com/skyzfs/skyzfs/pull/44#issuecomment-6842332625.

**Verdict: can merge once the jobs still running are green. No defect. Three
notes, none blocking.**

- The filter and the parsing, run on their own: `filter.sh` (28 strings; the
  round trip through `printf %q`; fifteen option strings).
- The options reach the recording's serve and every start of the checker
  (`replay-rig.sh:175`, `:476`; `replay_daemon.py:431`).
- The merge 782cf4d: one conflict, in the header comment, both sentences kept
  (`git show --remerge-diff 782cf4d`).
- This pull request's run 43353068322 is at d4d5b10 and drove
  `--store in-place`: the 24.04 daemon job exit 0, steps "none", `withheld` and
  `stuck` caught as expected.
- Notes: on the pull-request budget no prefix between two flushes is checked
  (321 in the log, 0 checked), and those are this store's harder states; the
  summary's "recovered logs of 0 to 0 MiB" line; no pull-request job drives the
  log store through this rig while it is still the default (the lead's ruling).

Verified: `test_replay_daemon.py` and `test_replay.py`, 99 tests. Not done:
shellcheck (not installed here), the rig itself, the 26.04 job's log.

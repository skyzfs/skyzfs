# M1.3t step a (pull request 65): the rigs and the benchmark name the in-place store

Reviewed at bb31008 and again at b0e2879; both posted. Nothing blocks.

- Every `skyzfsd serve` under `tests/` and `.github/` was read: in CI
  none is left to the daemon's default. `blkcheck-helper.sh` and
  `replay_daemon.py` name no store themselves and serve with what their
  caller gives.
- Found at bb31008: a log option with no store named got
  `--store in-place` beside it, which the daemon refuses (run with a
  built daemon: exit 2, "--log-high: not with --store in-place"). The
  benchmark's own example for `TB_SKYZFSD_ARGS` was such a row.
- Fixed at b0e2879: the replay rig and the benchmark refuse such
  options with a message before the start. The list is the daemon's
  nine (`cli.rs`, `refused`). `storecheck.sh` is the replay rig's
  handling alone, run on seven inputs (`storecheck.txt`).
- The lead: dropping `log` from the daemon rig's `store` input is the
  plan; CLAUDE.md's two lines are changed at the merge.

The rigs on the head were not read.

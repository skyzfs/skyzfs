# M1.T32 (#78): the daemon rig stops once ZFS is stuck

- `review-posted.md`: the review posted on #78 at head de8ad48
  (https://github.com/skyzfs/skyzfs/pull/78#issuecomment-6873056157).
  Verdict: can merge once its rigs read as they should; nothing blocks.
- `plants78.py`: the fourteen plants (Z1-Z14), each applied alone to a
  checkout of de8ad48, then `tests/kernel/test_daemon_check.py` and
  `tests/kernel/test_zfs_stuck.sh` run.
- `plants78.txt`: what each plant made fail.

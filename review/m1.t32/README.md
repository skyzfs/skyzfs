# M1.T32 (#78): the daemon rig stops once ZFS is stuck

- `review-posted.md`: the review posted on #78 at head de8ad48
  (https://github.com/skyzfs/skyzfs/pull/78#issuecomment-6873056157).
  Verdict: can merge once its rigs read as they should; nothing blocks.
- `plants78.py`: the fourteen plants (Z1-Z14), each applied alone to a
  checkout of de8ad48, then `tests/kernel/test_daemon_check.py` and
  `tests/kernel/test_zfs_stuck.sh` run.
- `plants78.txt`: what each plant made fail.
- `look2-posted.md`: the second look at 508d2c1
  (https://github.com/skyzfs/skyzfs/pull/78#issuecomment-6873240537):
  both suggestions taken; nothing blocks.
- `plants78b.py`, `plants78b.txt`: Z1-Z19 at 508d2c1 (Z1, Z2, Z14
  adapted; Z15-Z19 on the new condition). Only Z19 passes.

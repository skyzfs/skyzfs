# M1.T33 (#80): the control rig's refused-commit step and the abort pin

- `review-posted.md`: the review posted on #80 at head 0264071
  (https://github.com/skyzfs/skyzfs/pull/80#issuecomment-6873388080).
  Verdict: can merge once its own runs are green; nothing blocks.
- `plants80.py`, `plants80.txt`: ten plants (R1-R10) on refused_commit,
  rc_reader and the table; R2 and R4 pass every test.
- The pin at 8 aborts rests on libublk 0.4.8's `queue_is_done`
  (stopping and no command in flight) and `flush_and_wake_io_tasks`,
  read in the cargo registry's copy of the crate.
- Amended verdict
  (https://github.com/skyzfs/skyzfs/pull/80#issuecomment-6873394800):
  the lead asked for a write course and a flush course before the merge;
  not ready until they are in and looked at.

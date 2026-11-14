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
- Second look at f3f7d77 (7aaae9c, 0486339, f3f7d77):
  `look2-posted.md`
  (https://github.com/skyzfs/skyzfs/pull/80#issuecomment-6873777468).
  Verdict: blocked. `pin_refused_flush_completes_after_take_over` FAILs
  on all three images (control rig 43569917318): the control program's
  `answer()` computes `start_sector * 512` before matching on the kind
  and overflows for a flush (`skyzfs-ublk-control.rs:370:18` in the
  artifact's control.err). Outside #80's diff; the lead decides who
  fixes it. The write branch, the delete branch and f3f7d77's naming
  are right.
- `plants80b.py`, `plants80b.txt`: R2, R4, R1b and W1-W7 at 0486339
  (W5, a flush client without fsync, passes every test: only a kernel
  shows it).
- `plants80c.py`, `plants80c.txt`: T1 and T2 on f3f7d77's check of a
  take-over that ended by itself; both caught.
- `repro80.py`, `repro80.txt`: a test through the program's Scripted
  queue that hands a flush with start sector u64::MAX. It panics at
  370:18 as on the kernel, and passes with `saturating_mul`. The script
  restores the file it changes.
- Next look at 39a3625 (a5796cc, then a merge of zfs-topology with
  #81): https://github.com/skyzfs/skyzfs/pull/80#issuecomment-6874795702.
  The blocker is settled on 26.04 and 26.04-arm (control rig
  43576630830): `pin_refused_flush_completes_after_take_over` PASS,
  fsync 0 after 233 and 231 ms, the take-over handed `flushes 1`; the
  write take-over `writes 1`. ubuntu-24.04 was queued. `plants80d.py`,
  `plants80d.txt`: A1-A6 on a5796cc's changes, all caught.

# M1.3aa (pull request 64), head fcec1e4 on 43caee4

Review posted on the pull request. Nothing blocks.

- `kernel-reading.md`: the driver at v6.11, v6.17 and v7.0 against the
  description's table. "A refused commit completed nothing" holds on
  all three for this queue. "No command is outstanding on that tag
  afterwards" does not hold for a commit refused because the tag held no
  request: the earlier command still waits.
- `plants64.py`, `plants64.txt`: sixteen defects planted one at a time
  against the tests of skyzfs-ublk, skyzfs-ublk-sys and skyzfsd (with
  its test hooks). Fifteen are caught. Not caught: the kernel queue's
  wait dropping what `ended()` gives. Nothing without a kernel builds a
  `KernelQueue`, and the kernel tests pass a list they do not read.
- `plant-stale-state.txt`: the plant of pull request 58's fourth look
  (the state set only when nothing was said before) now fails
  `the_first_write_zeroes_on_direct_io_replaces_what_the_hand_over_said`
  (5d865d1); the other 45 in-place tests pass.
- A built daemon refuses nothing new here; the status file's other
  readers (`skz_time.py`, the rigs) read by key or by `status_version`
  and need no change.

CI when read (20:29 UTC): the control rig green on three images, the
daemon rig queued, lint and tests running, no failure.

## Second look, head d68f422 (comments only)

Posted: the text on `Queue::wait` is right for both kinds of refused
commit on v6.11, v6.17 and v7.0; can merge once CI is green.

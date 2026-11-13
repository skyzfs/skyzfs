# M1.T26 (#56): check g pins the failure's reason; four rig fixes

Reviewed at 602f837, base `zfs-topology` aa16288 (#55 merged). Comment:
https://github.com/skyzfs/skyzfs/pull/56#issuecomment-6852014534

Verdict: two findings block, both in how the new pin is wired into the
rig. The pin's reading, `serve_rc`, the fifo case and the log-store
observation are sound.

## The blocking findings

1. **`G_PINS` (`daemon-rig.sh:2617`) lacks the new pin.** The stand-in
   branch, the log-store branch and the no-file-system branch loop over
   that list, so none writes `pin_failure_reason_is_the_images`, and
   the judge exits 2 with "no result". Check g is in the default
   checks and every rig pull request runs the stand-in. The new unit
   test reads `OBSERVED` only, so 55 tests pass.
2. **A missing line is an ERROR of the pin, and the judge exits 2 on
   any ERROR.** A line is missing when the daemon did what check g is
   for (the start served; the serve did not fail). On the files of the
   negative control (run 43397129438, daemon cd0e763) the pin says
   ERROR, and the judge on that run's results goes from exit 1 with
   three lines that differ (base) to exit 2, "the rig failed". With a
   missing line counted as a FAIL it would be exit 1 with four lines.

## What is here

- `reason-harness.sh RIG_TREE reason DIR`: cuts `check_g_reason` out
  of the rig at RIG_TREE and runs it on an artifact's files.
  `reason-harness.sh RIG_TREE branch reference|log|nofs`: runs the
  rig's block that writes check g's lines without running it, then the
  judge on them with `--checks g`.
- For finding 2's table: copy the negative control's `results.tsv`,
  append the pin's line (and `obs_g_reads_while_failing`, which that
  run is too old to have) and run the judge at the head and at the
  base (`git show aa16288:tests/kernel/daemon_check.py`).

## Run here

- The pin on three real artifacts: 43413318350 (754a15a) PASS;
  43400705709 (#55 at 6355607) FAIL 2 of 4; 43397129438 ERROR.
- `serve_rc`: 652 ms for a status 0.6 s late, 8 ms present, 5381 ms
  never. No `cat` of the file is left; `serve()` empties it per start.
- `test_zfs_stuck.sh`: 8 ok in 7 s. With `lim 5` taken off the stack
  read in a copy, it fails after 31 s with exit 1 and leaves no reader;
  the base's test hangs on the same copy.
- `test_daemon_check.py`: 55 pass; with the reads observation taken out
  of the log-store branch the new test fails.

## Notes

- g.1 reaches the serving loop's call site in `serve.rs` (its line has
  "N requests the daemon held were answered", printed only there); the
  site for a failure that arrives with the queue's end (`serve.rs:1125`)
  is reached by neither g.1 nor g.3.
- The old waiter's status can land after the next `serve()` emptied the
  file (reading only): a stale 137 for the new serve.

## Traps met

- An old run's `results.tsv` judged by a newer judge has errors of its
  own (observations added since): add them before comparing exits, or
  the comparison shows nothing.
- `rec` in the rig also prints to stdout; a harness that evals it shows
  every line twice.

# M1.T14a: a limit on recording, a stuck-daemon control (pull request 42)

Head 19553d8 on `skyzfs-testing`, on `zfs-topology` 51f5972. Five files, test
code only: `replay-rig.sh`, `replay_daemon.py`, `replay_check.py`, two test
files. It follows the non-blocking notes of the review of pull request 39.

## First look: nothing blocks; the rig run decides

Posted on the pull request. What stands behind it:

- 160 tests pass as root; the 99 of `test_replay*.py` pass as a non-root user;
  `bash -n` passes. `review/m1.t14/repro.py` ends 1, 1, 2 once its third case
  is told that the rig's stop-after-ready hook is not an option of the daemon
  (my script failed every hooked start; that was its fault, not the head's).
- Plants in the Python side. Caught: no guard in `stuck_control`; `stuck`
  expected by `start`; no stuck control built; a writable mount that timed out
  judged again; the stage names not registered in `check()`. Not caught by a
  unit test: the rig's hook passed on to the daemon's arguments; SIGSTOP not
  sent. The rig run would show both (exit 2 and exit 1).
- `shell-try.sh` (here) runs `pool_step` and the fill step by themselves, with
  the real `lim` and stand-in commands:
  - `pool_step` through the daemon: success goes on; a failure and a command
    that does not end are a verdict (exit 1), the second after its limit;
    **a command that cannot be started (127) and a failure of `lim` itself
    (125) are a verdict too**;
  - on the reference server a failure keeps its status and there is no limit;
  - the fill: fits, goes on; does not fit, exit 2; `zfs get` gives no number,
    exit 2; out of space, exit 2; an I/O error, exit 1; does not end, exit 1.
    A `zfs get` that fails ends the script by errexit, which the cleanup
    reports as the rig's.
- `stuck_guard.py` (here), on the pull request's own harness:
  - a stuck start whose read fails at once with an I/O error gives exit 0 and
    the row "caught by reading the device": the control reads as expected
    though the limit, the kill and the reap never ran;
  - a stuck start whose reader is not reaped stops the run with exit 2 (the
    description says "with a verdict"; exit 2 is the right one).
- Control points are not among the points the stopped path counts
  (`st['outside']` takes calibration and end points only), so a control's
  planted failure cannot become a verdict when the run stops after it.
- The cleanup: every step that can sleep in the kernel is under `lim`. The
  export is tried before the daemon is killed, so after a stuck daemon it
  uses its 120 s and is left behind; no later step of the workflow touches the
  pool (Summary and upload only). Worst case after a verdict about 11 minutes.
- Mine to correct: I wrote on pull request 39 that nothing in the record
  phase had a time limit. The writer's end had one (300 s, then a verdict).

Not done: the rig was not run. Run 43324152732 at this head was under way; it
is the first run of `stuck` on a real device.

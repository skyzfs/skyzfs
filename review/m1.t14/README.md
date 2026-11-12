# M1.T14: the power-cut replay rig through skyzfsd (pull request 39)

Head 8348a73 on `skyzfs-testing`, merge-base 25685ac with `zfs-topology`.
Four files, test code only: `tests/kernel/replay_daemon.py`,
`tests/kernel/test_replay_daemon.py`, `tests/kernel/replay-rig.sh`,
`.github/workflows/skyzfs-replay-rig.yml`.

## First look: not ready, two findings block

Posted on the pull request. What stands behind it:

- `python3 tests/kernel/test_replay_daemon.py`: 20 pass. `test_replay.py`: 48 pass.
- `repro.py` (here): three cases on the pull request's own `Pass` harness.
  Run `python3 review/m1.t14/repro.py tests/kernel` from a checkout of the head.
  All three fail at 8348a73, which is the finding:
  1. the end image made for `zdb -bcc` fails at the daemon's start: exit 0;
  2. a prefix fails `fsync: files`, a later prefix raises `RigError`: exit 2;
  3. every hooked start fails at `start` with no start line: the control is "caught", exit 0.
- `replay_daemon.py --log <missing>` exits 1 with a traceback and no summary.
- `withheld_same.py` (here): on the unit tests' log, every image the `withheld`
  control checks is byte for byte the true image at the fsync mark before it.
- The boot id file: `os.path.exists(run_dir/../name.boot)` is False once the
  run directory is removed, so the file stays in `/run`.
- 20 plants in `replay_daemon.py`, one at a time; 14 fail a test, 6 do not:
  every failed main-pass point counted as a verdict; each of the three
  coverage errors switched off (required prefix not checked, passed without a
  txg, deep check missing); the "withheld could not be built" error switched
  off; no deep check at the last held point.
- The pull request's own run 43292136417 at this head: both daemon jobs
  green. ubuntu-24.04: 552 records, 110 flushes, 143 starts, 19 txgs, 38 deep
  checks, 0 of 443 prefixes between flushes, `withheld` caught at 468 by the
  fsync check (one write of 272 KiB held back, f.12 missing).

Not done: the rig itself was not run here (no kernel features). The order
of the lower log is argued from dm-log-writes' documentation and the code.

For the lead: M1's criterion says "every record prefix" and "the first and
the last prefix of every transaction group". The pull-request budget checks
no prefix between two flushes, and the one full run so far checked 17% and
26% of them.

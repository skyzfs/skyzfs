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

## Second look, at 2051d51: no blocking finding left

The four rig files are as at 7911228; 2051d51 adds one line of text in
`tests/kernel/control_check.py` for another package (M1.3q), which matches
the kernel test behind it (`the_driver_can_reissue` in
`crates/skyzfs-ublk-sys/tests/kernel.rs`).

- `test_replay_daemon.py`: 37 pass. `test_replay.py`, `test_control_check.py`: pass.
- `repro.py` here, all three cases, now end as they should: exit 1, exit 1, exit 2.
  `replay_daemon.py --log <missing>` exits 2 with a summary.
- 23 plants in the new `replay_daemon.py`; 15 fail a test. Not caught:
  - a required prefix not checked is no error (the author's stated gap);
  - `serve()`: a device left after a failed stage always raises (no test drives `serve()`);
  - with every prefix asked, optional prefixes still give way to time (the
    tested coverage error would then fire);
  - the criterion line says "shown" although the run fails;
  - the criterion line ignores the deep-check ends;
  - `dd` that cannot write the image is a device read again;
  - the writable mount is not judged again;
  - `withheld` expected by any stage again;
  - a stopped run drops the failures found outside the main pass.
- The lead's rule for item 7 (every prefix on the full budget, N of N and the
  deep ends in the summary, "not shown" otherwise) is what `criterion_line`
  and `--every-prefix` do, read against the code.
- No run of the rig has finished since 8348a73: the runs at 0c99998 and
  7911228 were cancelled by the next push, and the run at 2051d51
  (43296346205) was queued when I looked. So the reshaped `make()` and
  `serve()` have not run on a kernel in front of me.

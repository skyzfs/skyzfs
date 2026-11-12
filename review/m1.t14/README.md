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

## Third look, at 31b06ad: nothing open but the run

- `test_replay_daemon.py`: 44 pass. `test_replay.py`: pass.
- The eight plants that passed the tests at the second look each fail one now
  (the `serve()` branch, time on the full budget, both conditions of the
  criterion line, `dd` writing the image, the writable mount, `withheld` by
  any stage, a stopped run dropping outside failures). One new plant passes:
  the stopped run's "criterion not shown" sentence removed.
- `writer-try.sh` (here) runs the rig's `writer()` with its tools replaced:
  `bash review/m1.t14/writer-try.sh <file with the function>`. Status 0 when
  all steps work, 90 when the write on the pool fails, 91 when the mark fails
  or the scratch file cannot be written.
- The answer on the writable mount says the read-only mount "takes the same
  course". It does not: `light_imported` returns `stage('mount', rc)`,
  `deep_imported` returns `'mount (writable)'` for any exit code. Left for a
  change of its own in `replay_check.py`, which I accept.
- Run 43296922847 at this head was still queued when I looked.

## Fourth look, at 8e69d35: the delta is right; one older finding

Two commits on top of 31b06ad.

- **The rig run at 31b06ad is in** (run 43296922847). Both "through skyzfsd"
  jobs exit 0. Their summaries carry the criterion line as ruled: "Every record
  prefix: 145 of 552 checked" (24.04; 125 of 547 on 26.04), deep checks at the
  first and last checked prefix of each group, and "Design §9's replay
  criterion: not shown by this run". The withheld control is caught and the
  table is as expected. The run's one red job is the unit tests, which could
  not write `/run` as a non-root user.
- **5afd80a** (`RUN_PARENT`): `test_replay_daemon.py` as a non-root user gives
  2 errors at 31b06ad and 45 passes at 8e69d35. The default is still `/run`.
- **8e69d35**: 154 tests pass. Three plants, each caught: the old condition
  for the read (`not in rc.RIG_RC`); a timed-out `zdb -bcc` counted against
  the daemon again; a timed-out `zdb -bcc` not reported at all. `repro.py`
  still ends 1, 1, 2.
- **Does a 300 s timeout of the read belong to the daemon?** When it comes
  back, yes. `tread.py` (here) on both jobs' `replay-results.json`: 270 reads
  of the 1 GiB device took 0.26 to 0.55 s, so 300 s is about 550 times the
  slowest. And the daemon's device is made with user recovery and without
  fail-I/O (`crates/skyzfs-ublk-sys/src/lib.rs:104`), so when the daemon is
  gone the kernel queues reads and does not fail them (mainline v7.0,
  `drivers/block/ublk_drv.c:1177-1181, 2506-2508`): a crash after `ready`
  shows as a read with no answer, not as an error.
- **But a read with no answer most likely never comes back as a timeout.**
  Read, not run (no ublk here):
  - `dd iflag=direct` waits for its read uninterruptibly
    (`block/fops.c:98, 271-274`), so a kill does not end it while the read is
    queued;
  - `run()` uses `subprocess.run(timeout=)`, which kills the child and then
    waits for it without a limit (CPython 3.13, `subprocess.run`).
  So the rig would stop inside `run()` until the job's own limit (45 or 120
  minutes), with no summary and no verdict. The 124 can come back only when dd
  is between two reads, which is a device that answers slowly. The new test
  stubs `run()` to return 124, so it shows the label and not this. This is
  older than the delta: it was so at 31b06ad and I did not see it then.

## Fifth look, at 3b77bda: the fix is right as built; a rig run is owed

One commit on top of 8e69d35: the read of the device runs under
`read_bounded()`. At the timeout it kills the daemon and deletes the device,
then kills dd and waits `REAP_S` (60 s) for it; a dd still there sets
`info['abort']`, which stops the run and keeps the verdict. The daemon's own
end after SIGKILL is bounded as well.

- 156 tests pass as root, 47 of `test_replay_daemon.py` as a non-root user.
  `repro.py` still ends 1, 1, 2.
- Three plants, each caught: no `unstick()` at the timeout; no abort when the
  reader is left; waiting for the reader before unsticking (the old
  behaviour).
- `read_bounded_try.py` (here) runs it with real processes: a reader that
  ends, one that cannot start, one that hangs and dies when killed, and one
  whose pipe outlives the kill. Each comes back within the timeout plus
  `REAP_S`, with exit 0, 127, 124 ("ended once the server was killed") and 124
  ("not reaped").
- The kernel side, read at mainline v7.0: stopping a device that queues I/O
  for recovery sets `force_abort`, unquiesces the queue and kicks the requeue
  list (`ublk_force_abort_dev`, `drivers/block/ublk_drv.c:2855-2871`, called
  from `ublk_stop_dev_unlocked` at 2897-2899), and `ublk_queue_rq` then fails
  queued and new requests with an I/O error (2070-2073). So deleting the
  device after the daemon is killed ends dd's read.
- Not shown anywhere: the hang path on a real device. Nothing makes a daemon
  stop answering after `ready`, so no rig run reaches it.
- Every read now goes through `read_bounded()`, so this commit is on the path
  a passing run takes. The last finished rig run is at 31b06ad; run
  43307626765 at 3b77bda was under way when this was written.

# M1.T6 (skyzfs/skyzfs#13) at 80516b3: review notes

Nothing here ran on a kernel.

- **The look at /run/ublksrvd does not see the file libublk writes.**
  `libublk` 0.4.8, `src/ctrl.rs`, `get_json_path`:
  `format!("{}/{:04}.json", UblkCtrl::run_dir(), self.dev_id)`, so device 0
  has `/run/ublksrvd/0000.json`. `tests/kernel/ublksrvd-look.sh` looks for
  `N.*`, `N` and `*_N.*`. With the files `0000.json`, `0.pid`,
  `ublk_0.json` and `0010.json` in a directory, `ublksrvd_look 0 0 FILE`
  lists `0.pid` and `ublk_0.json` only.
- **What `klog_bad` counts**, one line each through the function as the
  rig has it: counted, `I/O error, dev ublkb0, ...` and `Buffer I/O error
  on dev ublkb0`; not counted, `timeout error, dev ublkb0, ...`,
  `critical space allocation error, dev ublkb0, ...`, `device offline
  error, dev ublkb0, ...`, `duration limit exceeded error, dev ublkb0,
  ...`, and ZFS's "has encountered an uncorrectable I/O failure and has
  been suspended". The first words are the kernel's names for a request's
  status (`blk_errors` in block/blk-core.c): -ETIMEDOUT is "timeout",
  -ENOSPC "critical space allocation", -ENODEV "device offline", -ETIME
  "duration limit exceeded". Also not counted: the line the rig itself
  writes when its marker is gone from the kernel log.
- `judge-inputs.py` (run from `tests/kernel`): the judge answers each new
  line altered or missing with a difference or a rig error.
- `overlap-tear.py` on a plain file with direct I/O here: 10,096 rounds in
  3 s, none torn.

## Second look, at 4a46b6a

- `stamp-slack.py`: how far the two stamps `overlap-tear.py` takes stand
  outside the system call between them, with two threads released by a
  barrier as there, around a call that does nothing. In this container,
  6,000 calls: median 10 µs, 90th percentile 40 µs, 99th 106 µs. A round
  is counted "together" when the later begin stamp is before the earlier
  end stamp, so two requests that missed each other at the device by less
  than this are counted as in flight together.
- `ublksrvd_look` with `0000.json`, `0008.json`, `08.pid`,
  `ublk_0008.json`, `0010.json` in the directory: each is listed under
  its own number and no other.

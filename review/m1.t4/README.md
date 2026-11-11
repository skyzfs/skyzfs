# M1.T4 (skyzfs/skyzfs#11) at beb53a3: review notes

- `judge-holes.py`: inputs for which `daemon_check.py` exits 0 although no
  check ran (`--checks "abcde"`, which the workflow's pattern accepts, and
  `" "`), or a pin or control the table does not know differs, or a pin
  was written FAIL and then PASS. All six print 0.
- The "killed server still LIVE" figure: in run 42988261841 (24.04) 19 of
  20 rounds of check b end "1 polls, then LIVE -1". The driver reports pid
  -1 once the server's task is gone (`ublk_ctrl_get_dev_info`, v7.0) while
  the state is still LIVE, and the watcher stops when the text is no longer
  "LIVE <pid>". So the device was still LIVE at the one look, and the
  figure is the length of one `ublk list`, not how long the device stayed
  LIVE.
- Three plants in `bins/skyzfs-blkcheck/src/device.rs`: carried results
  ignored (noticed), handed out before the resubmission (noticed), an I/O
  error after the failure not reported as the failure (not noticed by the
  unit tests; the kernel run's failed-volume cases would notice).

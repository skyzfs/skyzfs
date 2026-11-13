# M1.T25: the daemon rig says what holds ZFS (#53)

Review at 4e611b5 (base `zfs-topology` 0726a81), one file:
`tests/kernel/daemon-rig.sh`, 38 lines added and 4 removed.
Posted: https://github.com/skyzfs/skyzfs/pull/53#issuecomment-6848856263
Verdict: nothing blocks; four small findings.

## What the change is

`zfs_stuck_say [mark]`: the zpool processes in D state with the ZFS
function each waits in (from `/proc/PID/stack`, under `lim 5`), the states
of the txg_sync threads, and since the mark the hung-task reports. Called
where `zpool status` ran out in `fail_wait`, at the clear's FAIL in d.1,
and at a failed create in `fail_setup`.

## What ran here

- `say.sh RIG_DIR DMESG_FILE [mark]` cuts the function out of the rig and
  runs it with the rig's own `lim` and `klog_since`; `dmesg` is a stand-in
  that prints the file.
- The arm64 job's artifact (run 43389048554, artifact 14022382365):
  from d.1's mark `skz-rig-mark-4728-1794558052684667506` the function
  gives 148 reports, txg_sync running in 24, zpool:405492 x96,
  txg_sync:403031 x24. With the log cut at line 1329 (d.1's end): 28, 6,
  x12, x6. A mark that is not in the log: no kernel half, nothing said.
- The 19 stuck zpool tasks of that log, the function's pattern on each
  stack: 1 `spa_config_enter` (405492), 2 `txg_wait_synced`, 16
  `spa_namespace_enter` (12 in `zfs_ioc_pool_stats`, 4 in
  `zfs_ioc_pool_create`). All 19 have pid equal to tgid.
- Stand-ins: `vf.c` built as `zpool` gives a process in D state (the
  parent of a vfork); `named.py NAME sleep|run SECS` gives a process with
  any comm. Two in D: listed with `?`; in S: not listed; txg_sync `SR`.
- `stub-cat.sh` as `cat` first in PATH: `STUB=hang` makes a stack read
  never return (11.6 s for two processes, `?` for both); `STUB=<file>`
  feeds a stack (the three real ones give the three names).
- The ubuntu-24.04 job of the same run (kernel 7.0 too): 6 reports, no
  owner named, txg_sync shown twice in state D.

## Findings (none blocking)

1. A lost mark is not said (`daemon-rig.sh:331`).
2. The failed create does not say its status or that it ran out
   (`:1991-1992`); the job's create logs are empty, so "failed ()".
3. "txg_sync running in N" has no count of how often it was shown (`:333`).
4. Nothing in CI runs the function.

Suggestions: count by function when the list is long; d.3's clear and
`pool_errors` stay bare.

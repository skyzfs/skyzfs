# M1.T23: check g, a disk that fails under the in-place store (#54)

Review at 8833a95 (base `zfs-topology` 0726a81): `daemon-rig.sh` (+251),
`failed_write.py` (new), `daemon_check.py`, its test, the workflow.
Posted: https://github.com/skyzfs/skyzfs/pull/54#issuecomment-6849224409
Verdict: one finding blocks (a one-line fix).

## Findings

1. Blocks. `daemon-rig.sh:2789`: `pin_is NAME DETAIL [ A ] && [ B ]` gives
   `pin_is` only `[ A ]`. With status 3 and no failure line the pin is
   "PASS: ... no line of a failure in its log" (the line run alone).
2. Not blocking. g.1's table is dm-error over the whole device: reads fail
   too. The in-place store fails only the request for a failed read
   (`inplace.rs:687-693`). A read that misses the cache before the first
   failed sync would reach the application. Not seen in three runs.
3. Not code. The description's reason for the negative control's third
   line: at cd0e763 a failed start returns its error and deletes nothing
   (`serve.rs:760-761` there); "device 0 was deleted" is the next serve's.

Suggestions: a buffered read just before g.2's start; §4.1's refusal is
not in check g; record `daemon_ref` in the results; the serve logs are on
the failing file system.

## Why g.2 and g.3 hold

- g.2: the rig's `fdatasync` takes the file's error, the new descriptor's
  `syncfs` the file system's; the start's sync has nothing to report, and
  the drop of the cache is all that keeps 0x22 from the client. The
  premise (EIO, cache 0x22, disk 0x11) is checked per run; without it the
  pin is ERROR. The control pins the device-to-file offset.
- g.3: the table errors eight sectors under one synced block; a dirty page
  over it; the start's `fdatasync` reports the error whether it or the
  flusher wrote the page (an unseen error is reported once to a descriptor
  opened later). Kernel log of the 26.04 run: one ext4 write error at block
  39168 = sector 313344.
- The swap: `suspend --nolockfs`, `load`, `resume`. A freeze on the way
  back would write through the error table.

## What ran here

- `g2-branches.sh RIG`: the verdict branches of g.2 with stand-ins, twelve
  cases.
- `judge-plants.txt`: ten plants in the judge, each caught.
- `g_table`'s text with a stand-in for `dmsetup` (start, middle, end).
- `failed_write.py` on a local ext4 file; `extent` against `filefrag`.

## Read from artifacts

- Run 43397063281 (26.04 and 24.04, daemon at 0726a81): seven PASS, the
  control FAIL.
- Run 43397129438 (daemon cd0e763): three lines differ, as described.
- For the lead: g.3's vdev shows 278K to 362K write errors when suspended,
  and `zpool events -H` gives 580K to 750K `ereport.fs.zfs.data` lines in
  30 s; check d's pools (run 43389048554, 24.04) show hundreds.

## Owed

- arm64: no run of check g when the review was posted. Run 43398226980
  (the pull request's own) was in its rig step. A check-in is scheduled.

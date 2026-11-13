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


## Second look, at 8a3fb89

Comment: https://github.com/skyzfs/skyzfs/pull/54#issuecomment-6850030963

The blocking finding (the `pin_is` line at 2789) is fixed: the pin is an
`if` over the status and the line. Finding 2 (g.1's reads) is taken with
`FS_READS=observed` and a new observation; finding 3 (the description's
reason) is corrected. All four suggestions are taken or ruled on.

New, not blocking: the watcher that timestamps the daemon's failure line
is killed when `fail_wait` returns, i.e. when the pool suspends, so a
line that arrives later is reported as "not seen in 300 s" while the pin
may still find it. Pre-existing and small: `rerr` counts every non-empty
line of the reader's log, not only the failure markers.

`g2-branches.sh` is updated for this head (a `g_dir` stand-in and the
new `fw read`): fifteen cases, all landing where they should, including
three for the added premise (the cache no longer reads the write before
the start, the same in the control, and a failed read).

arm64 still owed: run 43398226980 was cancelled at 09:35:26 by the
concurrency group when 8a3fb89 was pushed, so its arm64 artifact ends in
check c. The run at this head is 43402554528.


## Check g on arm64 (run 43402554528, at 8a3fb89)

Comment: https://github.com/skyzfs/skyzfs/pull/54#issuecomment-6850159107

The first run of check g on arm64 (ubuntu-26.04-arm, kernel
7.0.0-1019-azure): all seven pins PASS, the control FAILs, as on
ubuntu-24.04 in the same run. The errseq observation is the same on
both (a new descriptor's fdatasync ok, its syncfs EIO), and
`obs_g_reads_while_failing` has 0 reads failed to the reader on either.

Check d beside check g, the same two jobs (vdev read / write errors when
suspended; `ereport.fs.zfs.data` in the 30 s window), one job each:

| | arm64 | amd64 |
|---|---|---|
| d.2 | 227 / 137, 243 | 17 / 134, 299 |
| d.3 | 197 / 134, 210 | 196 / 136, 251 |
| g.3 | 512 / 580K, 1,504,977 | 238 / 321K, 595,016 |
| g.1 | 3 / 4, 1,545,437 | 71 / 4, 569,645 |

The daemon in that run is the checkout's (without #55), so its reason
lines still say "the log failed while the volume was served".

Trap: a run's artifact that ends early with "The rig failed. This is
not a verdict on the server" may be a run cancelled by a newer push
(the concurrency group): look at the jobs' conclusions before reading
anything into it. Run 43398226980 was that.

# M1.T16: benchmark rows with barriers on the work disk (pull request 41)

Head d84a776 on `skyzfs-testing-m1.t16`, merge-base fb787cf with
`zfs-topology`. One file, `.github/workflows/topology-bench.yml`, 44 lines
added: a dispatch input `barriers`, a step that remounts the file system
holding `TB_WORK` with barriers before the first row and stops the job if that
did not take, and a step at the end that checks `nobarrier` is not back.

## First look

The review is posted on the pull request. What stands behind it:

- **The stop rule, run.** `stop-rule.sh` (here) takes the two steps' scripts
  out of the workflow and runs them as GitHub's bash would, with `findmnt`,
  `mount` and `sudo` replaced by stubs. Twelve cases: the job goes on only when
  the remount exits 0 and `nobarrier` is not an option afterwards; it stops on a
  failed mount, on `nobarrier` anywhere in the list, and on a failed `findmnt`.
  `review/m1.t16/stop-rule.sh <checkout of the head>`.
- **The string the rule tests is the one ext4 prints.** `_ext4_show_options`
  prints an option's token when it differs from the default, so barriers off
  shows as `nobarrier` and never as `barrier=0` (mainline v7.0,
  `fs/ext4/super.c`, table entries at 1901-1902).
- **Both jobs' logs** (runs 43292513180 on 24.04 and 43292520972 on 26.04)
  print the same options before, the same without `nobarrier` after (mount
  exit 0), and the same again at the end. Kernels 7.0.0-1017-azure and
  7.0.0-1019-azure. The work directory is on `/` on both.
- **Per row**, from the row JSONs of both artifacts, with `flushes.py` (here):
  24.04 work disk 0.37-0.38 flushes per transaction at 0.09-0.10 ms; 26.04 work
  disk 0 flushes in every row, while its log device (a loop device over
  `slog.img` on the work disk, direct I/O) counted 0.39-0.40 per transaction.
  `python3 -I review/m1.t16/flushes.py <artifact directory>`.
- **Why 0 on 26.04: the description's reading holds by elimination.** Mainline
  v7.0:
  - the loop driver turns a flush into `vfs_fsync` of its file
    (`drivers/block/loop.c:277`);
  - an ext4 fsync on a journal with `JBD2_BARRIER` ends in a flush to the
    device, from the commit or from `blkdev_issue_flush`
    (`fs/ext4/fsync.c:123-125, 178-179`); a remount sets the flag
    (`ext4_init_journal_params`, `fs/ext4/super.c:5856-5869`, called at 6661);
  - a flush that is sent is counted on the whole disk
    (`block/blk-flush.c:124-133, 220`), which is what `work_disk()` reads;
  - the one place a flush is completed without being sent or counted is
    `submit_bio_noacct`, for a device without a write cache
    (`block/blk-core.c:817-828`).
  So about 160,000 fsyncs per row reached a barrier-mounted ext4 and `sda`
  counted none. `sda`'s count is 0 since boot; the nvme disk of a `nobarrier`
  job (run 43285245364) had 22 before its first row.
- **Still unread.** No log line and no file of either run states `sda`'s
  `queue/write_cache`. M1.T15's `write_cache=write_back` was read on
  `nvme0n1p1` only: both of its runs were on the 24.04 image.
- **Nothing in an artifact marks a job as run with barriers.** The row JSONs
  and `summary.md` carry no mount option; the step summary is on the run's page
  only.

Not done: no job was run. Ubuntu's kernel tree was not read, mainline at the
tag v7.0 was.

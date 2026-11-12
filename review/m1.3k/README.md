# M1.3k (skyzfs/skyzfs#31): writeback starts early

Reviewed: 449a4c4 ... `origin/skyzfs-coding` f67d1d1.

## What was done

- Read writeback.rs (both), and the diff of log.rs, store.rs, fs.rs,
  io.rs, sim.rs, volume.rs, serve.rs, cli.rs and status.rs (code).
- fmt, clippy -D warnings clean. Tests: skyzfs-local 219 pass (3
  ignored), skyzfs-ublk 125, skyzfsd 48, skyzfs-ublk-sys pass;
  test_skz_time.py 27 pass.
- Kernel source read (v6.12: fs/sync.c, mm/filemap.c,
  mm/page-writeback.c) for what the call returns.
- Four plants in the tracker.
- Not done: no kernel run; skz_time.py's diff and the status tests not
  read line by line; the description not read.

## Findings

1. The hint can fail a healthy volume on ENOMEM. With the write flag
   alone the kernel writes in mode WB_SYNC_NONE, and `do_writepages`
   tries again after -ENOMEM only in mode WB_SYNC_ALL:
   `if (ret != -ENOMEM || wbc->sync_mode != WB_SYNC_ALL) break;`.
   The value goes back to the caller unchanged. So a file system
   whose writepages is short of memory makes the hint fail where the
   `fdatasync` that follows would have tried again and passed. The
   store takes any error but "unsupported" for a failed write and
   marks the log.
2. `start_writeback(file, offset, 0)` is passed on, and the kernel
   reads a length of 0 as "to the end of the file". The tracker never
   asks for it; the function does not refuse it.
3. Test gap: with no hand-over at a change of segment file, all 219
   tests pass. No test rolls a log with the hint on.
4. Where the cost lands: the first use of a segment file hands 64 MiB
   of zeros over inside a write's call, and the call may wait for the
   device's queue. Counted; for the benchmark to judge.
5. EINVAL is taken as "unsupported". For a regular file the kernel
   gives EINVAL only for bad arguments, so a wrong offset would switch
   the hint off quietly. The status says "off".

## Agreed with

- A hint that returns Ok changes counters only. With the unit 0 the
  tracker adds to a sum and returns; the daemon does not even give the
  call. Plant "unit 0 still hands": caught, 3 tests.
- The kernel's write-only path takes no writeback error (no errseq
  check), so the next fdatasync still reports one.
- The range is the span of finished writes of one file; a sync resets
  it; less than a unit waits for the sync by design.
- Plants "hint error ignored" and "unsupported not switched off":
  caught, one test each.
- The unsafe call: no pointer, offset and length within i64 and their
  sum too, the descriptor borrowed for the call.
- Status version 7: 1584 + 32 fields = 1840 bytes, 205 fields; 5, 6
  and 7 read; `status_version` printed.
- The three barrier tests wait for the barrier's own count.

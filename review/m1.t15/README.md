# M1.T15: the daemon's system calls and CPU profile (pull request 40)

Head eff4a8c on `skyzfs-testing-m1.t15` (it was 19b0634 when the review was
asked for), merge-base 25685ac with `zfs-topology`. Eleven files, benchmark
code only.

## First look: not ready, one finding blocks

Posted on the pull request. What stands behind it:

- `topology-bench.sh summarize <artifact of run 43285245364>` at four commits:
  12d56d5 renders 352 lines; a246283, 19b0634 and eff4a8c exit 1 with a
  SyntaxError and render nothing. The cause is two unescaped apostrophes in a
  single-quoted Python string (`topology-bench.sh:592`).
- `recount.py` and `inside.py` (here) on the raw samples of that run
  (`5/zfs-skyzfsd-tpcb.perfscript.gz` in its artifact `topology-bench-sequence`):
  `python3 -I review/m1.t15/recount.py tests/bench <perfscript.gz>`.
  The recount gives the description's table exactly (pump 1679 samples, 77% in
  the kernel: wake and switch 42, io_uring 19, ZFS completion 16, ublk driver
  14, copy in ublk 4). Inside the kinds:
  - `copy_folio_from_iter_atomic` is a leaf 6 times in the store and is not in
    `COPY`; with it the store's copies are 26%, not 24%;
  - 26 pump samples are inside `ublk_copy_user_pages` with a leaf that is not a
    copy routine (`check_heap_object`, `__check_object_size`); with them the
    pump's copy path is 6.4% of its kernel samples, not 4.4%;
  - 183 of the pump's 210 `ZFS completion` samples are at the interrupt restore
    in `taskq_dispatch_ent`, which is the wake of a ZFS thread;
  - 5 samples in all have block work under `schedule()` (a softirq).
- `syscalls.py` at the head on the real `perf trace -s` output of run
  43278672091 gives the description's per-transaction figures.
- Plants: 9 in `bracket.py`, `perfcat.py`, `syscalls.py`; 6 fail a test. Not
  caught: page cache ranked before wake; a copy judged by any frame; counting
  strace's unfinished lines (they do not match the line pattern anyway).

Not done: no benchmark was run. The background jobs in `topology-bench.sh`
were read, not run (they need the daemon and root).

For the lead: the work disk's ext4 is mounted `nobarrier` on the runner
(`disk-sync.txt` in the artifact confirms it), which the author raises as a
decision.

## Second look, at 3cde27c: no finding left that blocks; it can merge

3cde27c is e4590cc plus a merge of `zfs-topology`.

- The five bench test files pass, the new `test_embedded_python.py` among them.
- `topology-bench.sh summarize` on the artifact of run 43285245364 renders 352
  lines at the head.
- `recount.py` with the head's rules: the pump's copy in ublk is 83 samples
  (6%), the store's copies 98 (26%), the readers' 29 (5%); the rest as before.
- 8 plants; 7 fail a test: the apostrophe back, a syntax error in the row
  builder, the setting's mark ignored by `bracket.py`, `copy_folio_` not a
  copy, a sample under the ublk copy not a copy, page cache ranked before
  wake, a copy judged by any frame. Not caught: the row builder never writes
  `slowed_by` (no test runs the row builder).
- The shell expression that sets `TB_ROW_SLOWED`, evaluated for the five
  combinations of daemon up, TB_SYSCALLS and TB_PERF_RECORD: marks exactly the
  rows whose daemon is traced or recorded.

Not done: no benchmark has run at this head, so the changed lines of the
background jobs and the row's mark have not run on a runner.

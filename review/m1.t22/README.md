# M1.T22 (pull request 49): benchmark tooling for reclaim, slow calls, reclaim in the profile

Head reviewed: 7108a9b (`skyzfs-testing-m1.t22`; the runs are on 40cb2f3, and the
merge after it changes no benchmark file). Review posted:
https://github.com/skyzfs/skyzfs/pull/49#issuecomment-6843491992

Verdict: can merge, nothing blocks.

## What was checked, and how

The head's `perfcat.py` and `syscalls.py` were run over the artifacts of runs
43357382135 and 43357385356 (rows 4 `ip-1g-perf` and 5 `ip-1g-sys`), each
archive unpacked in its own directory, the scripts here kept elsewhere and run
with `python3 -I`:

- `win.py DIR`: the window differences of the cgroup's counters from the two
  snapshots against the row's JSON. Exact in both runs.
- `trace.py DIR BENCH`: the real trace through `PERF_EVENT`, `perf_slow` and
  `perf`; slow lines against the summary's maxima; lines the regex leaves out.
- `cat.py SCRIPT.gz BENCH`, `more.py`, `both.py`: the profile's kinds, the
  'page reclaim' samples without a reclaim entry point in the chain, what
  'memory group' samples would be without the kind, the throttle's samples.

## Findings (none blocks)

1. `lru_gen_` in `RECLAIM` (perfcat.py:54): 75 of 357 and 5 of 91 'page
   reclaim' samples have no reclaim in the chain: refault handling under
   `workingset_refault` (64, 5) and `lru_gen_add_folio` under `lru_add` (11, 0).
   Without it the queue thread's share goes 7.9% -> 6.3% in the second run; no
   test fails; nothing is lost (all 368 others have an entry point).
2. `DIRTY_THROTTLE` (perfcat.py:62) matches `balance_dirty_pages_ratelimited*`,
   the check every write makes: all 8 samples of 'write-back from a write' are
   that; none has `balance_dirty_pages` itself. `^balance_dirty_pages$` passes
   the tests. Not known: whether it is a symbol of its own on the runners'
   kernel (it is on 6.18 here).
3. perf's `[continued]` lines are calls whose entry perf missed, timed from the
   thread's previous entry (first run, trace lines 451-452). The parser leaves
   them out, rightly, by accident. `events` is 2 x calls less 264 on the queue
   thread of the second run: the same thing.
4. Smaller: `grep -q 'events'` can now be met by an event line; awk's printing
   of products beyond 2^31 and the reader's silent drop; `count_memcg`; the slow
   share of waiting calls; arm64 names in `COPY` and the `__x64_sys_*` rules
   (outside the change, for M1.T21).

## Question 2, the evidence

- 54,813 calls in the summary against 1,477 event lines; 189,123 against 1,399.
- Against the daemon's counters over the window: pread64 4,114/s vs 4,829
  reads, pwrite64 450/s vs 511 writes, fdatasync 192/s vs 214 flushes (second
  run); 1,398 vs 1,471 and 103 vs 137 (first).
- Slow lines exist exactly where the summary's maximum is >= 1 ms (34 rows).
- perf's source was read through a summary at v6.8, v6.14, v6.17, not verbatim:
  `thread__update_stats` before `trace__filter_duration` in `trace__sys_exit`.
  Also from that reading: `lru_gen_add_folio` is `static inline` in
  `mm_inline.h` at all three tags, yet the runner's kernel has it as a symbol
  (the profile shows it); `count_memcg_events` and `mod_memcg_state` are the
  real functions from v6.17.

## Plants (against test_perfcat and test_syscalls)

Caught: 'memory group' before 'page reclaim'; write-back without the write
call; slow as "more than" the mark; slow calls not gathered by name.
Not caught (the remedies): `lru_gen_` out; the throttle by its own name only.
A trap met on the way: two plants of equal length within one second leave a
stale `__pycache__`; remove it between plants.

## Not done

Nothing on a kernel; no profile without a limit; none where the throttle
fired; the strace path only by its unit test; arm64 not run.

## Second look, at bd8feef

Posted as https://github.com/skyzfs/skyzfs/pull/49#issuecomment-6844044932.
**Can merge.** Findings 1 to 4 all dropped:

- `lru_gen_` is out of `RECLAIM`; the throttle is
  `^(balance_dirty_pages|__balance_dirty_pages)$`; `count_memcg` is in `MEMCG`.
- The docstring and a fixture line cover perf's `[continued]` form.
- `grep -q ' events, '` decides that perf wrote a summary; the counters are
  printed with `%.0f`; a sentence under the table says what the slow share
  means for calls that wait.

Re-run over the two runs' artifacts: page reclaim 85 of 1,406 (6.0%) and 281
of 4,495 (6.3%) on the queue thread; every such sample has a reclaim entry
point in its chain (86 and 282 over all threads); write-back from a write 0.
Three plants, each caught. Lint, test and coverage were queued when I posted.

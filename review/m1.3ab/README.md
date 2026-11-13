# M1.3ab: the watch test's hole (pull request #61)

Reviewed at 532d612 against `zfs-topology` at c2169bc. The review is
https://github.com/skyzfs/skyzfs/pull/61#issuecomment-6860158305.
Verdict: nothing blocks the merge.

## What was found

- The failure of ci run 43463408420 ("a page four times larger did not
  reach the hole") is reproduced in kind. The test maps eight pages and
  takes the first address aligned to four pages as the hole. When the
  mapping's base is already aligned, the hole is the mapping's first
  page and joins the free space below it, and a larger mapping of
  another thread can end on it.
- What maps and unmaps in the unit-test binary (`strace-unit-tests.txt`):
  a four-page alternate signal stack per thread (mapped at the thread's
  start, unmapped at its exit), 128 MiB arenas that are trimmed at once,
  1 MiB buffers, 2 MiB stacks. Nothing maps a single page, which is why
  the author's stress run (threads mapping single pages) found nothing.
- The new rule (count an attempt only when the hole is empty before and
  after) catches the planted defect every time and does not pass it by
  luck (`plants61.txt`).
- A gap from before this change: a rounding to half the page given
  passes, because the question is asked about the second page. Asking
  about the fourth page would tell them apart (reasoned, not run).

## Files

- `probe.patch`: the probe as a diff against 532d612. An ignored test
  `probe_stray_pages_in_the_hole` that runs the test's attempt in a
  loop beside threads that start and end threads. `PROBE_SECS`,
  `PROBE_THREADS`, `PROBE_HOLE` = `edge`, `inner` or `edge-aligned`
  (unset: the hole as the test takes it).
- `probe-edge-aligned.txt`: the hole aligned and at the mapping's first
  page, 8 runs: 1.72 million attempts, the old rule's failure 4,280
  times, the new rule's longest run without a pass 8.
- `probe-as-the-test.txt`: the hole as the test takes it, 12 runs:
  2.49 million attempts, the hole at the edge twice, filled both times.
- `probe-edge-inner.txt`: `edge` and `inner`. Only the fill counts are
  valid there: the hole is not aligned to four pages in these modes, so
  the "old rule" column means nothing. `inner` (a page of the test's
  own below the hole): no fill in 1.22 million attempts.
- `plants61.txt`: the head and two planted defects, each 200 runs of
  the test alone, 200 of the whole binary, and one run under churn.
- `strace-unit-tests.txt`: the mmap and munmap calls of one run of the
  unit-test binary.

## For next time

- A stress that does not reproduce a failure says little until it makes
  the kind of mapping the real binary makes. Trace the binary first.

## Second look, e2e5ca1

Both suggestions taken: `aligned` strictly above `base`, and the fourth
page asked about. It can merge from the review's side once CI is green.

- `plants61-e2e5ca1.txt`: the head and three planted roundings (the
  system's page, half the page given, twice the page given), each 200
  runs alone, 200 of the whole binary and three runs under churn. The
  first two plants fail every time; nothing fills the hole at the head
  (2.86 million attempts). A rounding to twice the page given passes:
  the test claims only "less than the page given".
- `churn2.py`: inserts the churn probe used for those runs into
  `watch.rs` at e2e5ca1.

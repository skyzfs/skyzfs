# M1.3w: two timing tests take their own time after the look (#52)

Review at aa3cc87 (base `zfs-topology` daa02df), one file:
`crates/skyzfs-local/src/store/tests.rs`, 7 lines added and 2 removed.
Posted: https://github.com/skyzfs/skyzfs/pull/52#issuecomment-6847330008
Verdict: no findings; it can merge once `test` and `unit-arm64` are green.

## Why the new order holds

- A thread's time starts at `Spans::new()` inside `serve_with`
  (`store.rs:4532-4533`), after the test's `began`.
- A look reads the clock with both locks held (`store.rs:4419`). An idle
  thread's wait is counted up to that reading; a busy thread's sum ends at
  its last closed span, read before it showed the sum.
- The test now reads its clock after `timing()` returned, so the thread's
  sum is at most the test's time.
- The steps' parts are sums since the open; no step runs before `began`
  (the marks are set after the writes and the flush).

## What ran here (kernel 6.18, amd64, 4 cores)

- `cargo test -p skyzfs-local --lib`: 275 passed, 4 ignored.
- `variant.py FILE MODE` rewrites the two spots (old, old5, new5, slack-old,
  slack-new); `rep.sh BIN N [load]` runs the two tests N times.
  - old5 (old order, 5 ms between): both fail, 5 of 5, at 5508 (+2 lines)
    and at the steps' bound.
  - new5: both pass, 5 of 5.
  - slack (test's time minus the thread's sum, 60 runs): reader 1.6 to
    8.4 µs old (median 4.4), 3.4 to 18 µs new (median 6.4); worker median
    36 µs old, 37 µs new.
  - under 8 busy loops: old order 1 failure in 150 (the flush test, line
    4133, the worker's bound); new order 0 in 150.
- `plants.sh PLANT_PY p1 p2 p3 p4` (defects in `store.rs`): p1 and p2 fail
  5508, p3 fails 4133, p4 fails 4116.

## Not done

- The workspace totals; the failed log on #50 (the panic does not print
  the test's total).
- Suggestion made: print `total` in the three assertions.

A trap met: a wait loop around a background run, with a sleep inside
Python, is a way round the rule against polling and is refused. A
background run reports its own end.

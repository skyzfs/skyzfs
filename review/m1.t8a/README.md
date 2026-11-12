# M1.T8a (skyzfs/skyzfs#26): the pipeline's timing test on the paused clock

Reviewed: `origin/zfs-topology` 542ea13 ... 6e50760 (one test, one
dev-dependency feature).

Verdict: not a weakening of what the test was for. Nothing blocks.

- Ran the file's 16 tests: pass. The timing test three times: 1.542 s
  and 72 ms each time, factor 21.4; about 30 ms of wall clock.
- The publish path (`cycle.rs`) has no spawn, no spawn_blocking and no
  timer of its own; the probe is in-process and its only waits are
  tokio sleeps. So the paused clock counts the probe's sleeps and
  nothing else.
- Planted a cap on the effective depth in `cycle.rs`:
  cap 1: 1.032 s, fails; cap 2: 520 ms (factor 2.97), fails;
  cap 4: 264 ms (5.8), passes; cap 8: 136 ms (11.3), passes.
- Lost against the old test: anything per op that is not a wait on the
  runtime's clock. That is CPU, which the description says, and also
  blocking work in the task (the state store's file writes, a
  std sleep), which it does not say.
- The lower bound (one RTT per batch) is right and is the guard
  against the clock not being the simulated one.
- Suggestion: the clock is exact now, so the factor can be 12 and
  catch a pipeline that runs at half its depth; at 3 only a depth of
  1 or 2 fails.

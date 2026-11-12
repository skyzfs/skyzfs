# M1.T10 (skyzfs/skyzfs#28): the benchmark reads the daemon's times

Reviewed: 58e1b54 ... db864d3 (tests/bench/skz_time.py, its tests, 30
lines of topology-bench.sh, two workflow lines).

## What was done

- Read the code. Every key the script reads is in the daemon's key
  list (153 keys; 14 `_max_ns` keys are not used).
- The 9 unit tests pass. 16 arithmetic plants, one at a time: 10
  caught, 6 not (per-step parts in the wrong unit, the walk's share
  over seven, the gate in the wrong unit, the own sync left out of
  "all parts", the flush mean over the reads' count, the reads' total
  in the wrong unit).
- The summaries of all eight runs saved from the combined-table jobs'
  logs and compared with the description: all 16 ratios, the six
  read-life rows, the four wait-by-cause rows, both step breakdowns,
  the four per-step rows with steps per second and reads per step,
  flush calls, busy shares, the gate, the bounded rows, postgres's
  means, the pump's store.
- Not done: artifacts (JSON) not opened; the re-rendered summaries of
  the first four runs not seen, only their logs.

## Findings

1. Not blocking the merge; to correct before the design cites it. The
   per-step table gives the rule's own sync as "ms per step" (19 to 22
   on the faster rows). There are 18 to 23 such syncs a window against
   328 to 368 steps: each takes 320 to 375 ms (21 to 24 ms on the
   slower rows). The description's "a sync of the log costs 65 to 87
   ms" is the flush's call; the own sync of the same file is four to
   five times that, and it is what the longest read and write waits
   were behind (400 to 880 ms). The time shares are right; the cost of
   one event is not shown.
2. Not blocking. Six columns are pinned by no test, the per-step table
   among them (plants above).
3. Not blocking. A row whose two readings are of different pids, or
   in which a sum went down, is flagged in a line above the tables and
   then printed in every table like the others.
4. Description against the logs, small:
   - the worker identity differs by -2.9 to +140.9 ms in the logs, not
     -73 to +57 (the script's own text says -3 to +141);
   - the readings are 122.9 to 123.2 s apart in the four runs that
     print it, not 122.7 to 123.0;
   - one row's store of the position is 0.033 ms, outside "0.012 to
     0.014" (43166016571 daemon-A).

## Agreed with

- "The gate was never engaged in sixteen rows": count 0 and 0.0 ms in
  all sixteen. It is also what the code gives at this load: a step
  takes up to 8 MiB and a request adds at most 1 MiB, so the one step
  after the request that reached the mark brings the log under it
  before the next turn.

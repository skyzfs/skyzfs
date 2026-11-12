# M1.T12 (skyzfs/skyzfs#36): a daemon row judged against the model rows around it

`skyzfs-testing` at 1b8887d, base de83464 (merge-base with `zfs-topology`). One look.

**Verdict: can merge. No blocking finding.**

## What I did

- Read the whole diff before any description (the description was not read).
- Ran `tests/bench/test_bracket.py`: 8 pass.
- Planted ten faults in `bracket.py`, one at a time; the tests failed for each:
  machine not compared; workload not compared; a model row without tps
  accepted; a model row with snapshots accepted; `>` to `>=` at the width
  limit; the limit 2% wider; origin delay judged unless both rows have one; the
  farthest model row before instead of the nearest; a model row that does not
  say its delay judged; the 0.90 threshold moved.
- Ran `summarize` on made-up rows of one sequence (a model row that did not
  measure, a wide bracket, a model row with an origin delay, no model row
  after, a second machine with the same row numbers). Each row got the
  verdict or the reason the rule gives; the unmeasured model row was passed
  over and the other machine's rows were not used.
- Ran `work_disk` and `diskstats_line` under `set -euo pipefail`, and
  `disk-sync.py`, here (a whole disk, `vda`), with a directory that does not
  exist as well: nothing printed, nothing failed.

## What I did not do

- No benchmark run: no kernel features here. The cgroup, the pressure files
  and a partition (`nvme0n1p1`, `sda1`) were read, not run.
- shellcheck is not installed here.

## The lead's questions

1. **The rule.** A row cannot be bracketed by another machine's model row
   (`same_place`), by one that did not measure (`o.get('tps')`), or by itself
   (only `zfs-slog` rows with a strictly smaller or larger row number). The
   width is `|a - b| / ((a + b) / 2)` and a row is refused when it is over
   0.10: 1050 and 950 are judged, 1051 and 950 are not, as the test says. A
   nearest model row with an origin delay gives NO VERDICT; it is not passed
   over for a farther one.
2. **The cgroup.** No `memory.high` or cpu limit is written. `memory.max` is
   rewritten on every run of the script (`max` without TB_MEM_LIMIT), so a
   limit from an earlier row does not stay. Creation fails the script under
   `set -e`, as the daemon's cgroup already does; it is not skipped silently.
   See finding 1 for what can change.
3. **The samplers.** A missing file leaves the key out and the table prints
   `n/a` or `-`. The disk is found by device number through sysfs, not by
   name: a partition has a `partition` file and its parent directory is the
   disk, for nvme and sd alike.
4. **The workflow.** Only the input's description and a refusal of an empty
   ref. The push trigger is unchanged and ignores `zfs-topology` and
   `skyzfs-*`.

## Findings (none blocking)

1. **Postgres in a top-level cgroup can change how the CPU is shared.**
   `topology-bench.sh:893-896`, `:1100-1105`. Before, without TB_MEM_LIMIT,
   postgres stayed in the runner's cgroup with pgbench and the harness. Now it
   is a top-level group. Where the cpu controller is on for top-level groups,
   a busy machine shares the CPU between groups by weight, not between
   threads. Both the model rows and the daemon rows of a job get the same
   treatment, so a bracketed ratio stays fair within a job. Rows from before
   this change and after it are not known to be comparable. Not measured;
   medium-low confidence that the effect is visible. I drop it when the
   description or the design says so, or one job shows the model row's tps
   unchanged.
2. **A model row with other settings is taken as a bracket.** `bracket.py:44-46`.
   Only machine, workload, snapshots and tps are checked. A `zfs-slog` entry
   with another client count, duration or TB_MEM_LIMIT brackets a daemon row
   and gives a verdict. The old rule had the same gap. Sure of the behaviour.
   I drop it when `clients` (the row carries it) is compared, or the rule is
   written down as the sequence author's duty.
3. **A cgroup with no postgres in it reads as 0, not as "not there".**
   `:1102-1104` ignores a failed move into the cgroup; the pressure and CPU
   columns then show 0.0. The log line gives the count of processes. Low.
4. **Pressure files that exist but cannot be read fail the row.**
   `:1871-1881`: with PSI off at boot the file passes `-r` and the read fails,
   and `snapshot` runs under `set -e`. Ubuntu runners have PSI on. Low.

## Suggestions

- The memory table still says "The cgroup is postgres under TB_MEM_LIMIT"
  (`:549`); it now prints for rows without a limit.
- No test covers the 0.90 threshold exactly (`>=`), or `summarize`'s table
  itself.

## The added commit b4881ca (read, not run)

Head moved to b4881ca: one commit in `topology-bench.sh`, for a finding of
Codex. Every `skyzfs*` name and `postgres` is now kept in each per-name
metric, and a missing one prints "not recorded". The verdict stands.

- It does what it says: a kept name seen at the first look stays at 0, and
  the cut at twenty no longer applies to kept names.
- One corner left, low: the sampler prints a name only when its value is not
  0 (`if v: print(kind, k, v)`). A daemon thread whose counter is 0 at both
  looks (say an idle thread with no user ticks) prints "not recorded" where 0
  is true. I drop it when the sampler prints kept names at 0 too.

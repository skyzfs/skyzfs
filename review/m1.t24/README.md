# M1.T24 (#51): the benchmark's copy onto the daemon under a limit

## Review, at 50820e3

Posted: https://github.com/skyzfs/skyzfs/pull/51#issuecomment-6846588190
Verdict: nothing blocks. Diff: `git diff 3af3370 50820e3`, five files (the
benchmark script, its workflow, a new test, one line in two lint jobs).

Findings:

1. `rm -rf "$SKZ_DIR" "$SKZ_RUN"` in `skz_stop` (`topology-bench.sh:1596`) and
   at the next row's start (`:1819`) has no limit. By reading: `unlink` takes
   the victim's inode lock, which a buffered write holds; a daemon stuck in such
   a write that does not die on SIGKILL holds it. Not produced.
2. Rows after a hang run beside what it left (the cgroup's `rmdir` and the
   leftover's detach fail silently) and their output says nothing. The lead's
   choice: end the sequence after a step that ran out, or record at each row's
   start what was found.
3. The sync half of `skz_copy` is not tested (plants a3, a4 pass); the test has
   no stand-in for `sync` though its header says so, and runs the machine's
   real `sync` five times, once under 1 s. `t24-busy.sh`: 5 of 5 runs passed
   here while another process wrote 300 MiB files in a loop.
4. The call sites (`|| return 1`, `exit "$rrc"`: plants a6, a11 pass), the
   workflow's loop and the evidence step on a cancel have never run with a
   failure. A sequence with `TB_SKYZFSD_COPY_LIMIT_S=1` would show them.
5. The log line's count of tasks in D state is one too high: `grep -c ' D '`
   matches the file's own header. `t24-ev.sh` (the function by hand, the sysrq
   write sent to a file): "1 threads, 1 in D state" with nothing in D state;
   every file written; 1.4 s with 113 tasks.

Smaller: `lim` sees a step end at the next 0.1 s (20 ms reads 112 ms, 120 ms
reads 217 ms), so `sync ms` has a floor; the create's stdout ("created ...")
is dropped and the test's stand-in says it on stderr; "Evidence on failure" now
runs after the later rows; the repeat summary does not name the create's
arguments.

What ran: the test, 30 checks at d1c542a and 32 at 50820e3; `plants.sh` (a1,
a2, a5, a12 caught; a3, a4, a6, a9, a11 pass). CI's `lint` and `test` green at
50820e3. Nothing on ublk; shellcheck is not installed here.

On `skz_hang_evidence` and limits: what runs without `lim` is procfs reads of
`stat`, `comm`, `wchan`, pressure, `meminfo`, `vmstat`, writes to the row's
output, `kill -0` and the write to `sysrq-trigger`; stacks, sysfs queue
attributes, debugfs, `ublk list`, cgroup files, `dmesg` and the status are
under `lim`.

## Second look, at 991cd58

Posted: https://github.com/skyzfs/skyzfs/pull/51#issuecomment-6846985876
Verdict: it can merge; all five findings dropped. `git diff 50820e3 991cd58`.

- 1: both removals under `lim 120` (by reading; the test's `skz_stop` is a
  stand-in).
- 2, as the lead ruled: the workflow ends the sequence when the row's output
  has `skyzfsd-hang/` (`$TB_OUT/$i`, which is the row's `OUT`), and passes
  `TB_EARLIER_FAILED` to rows after a failed one; the label says "after failed
  row N" and the JSON has `earlier_failed`.
- 3: a `sync` stand-in and two new cases; plants a3 and a4 now fail.
- 4: run 43384421833 at 991cd58 (sequence badcreate, model, limit1, not-run),
  read from its artifacts: row 1 no result, row 2 labelled
  `zfs-slog [model; after failed row 1]` with `earlier_failed [1]`, row 3 with
  `skyzfsd-hang/` (daemon alive, five threads, ublkb0's attributes), row 4
  absent. Run 43384124782 at be79787: "Run benchmark" cancelled, "Evidence on
  failure" ran.
- 5: the count takes the `== ` lines; the old pattern fails the new check.

What ran: `test_copy_limit.sh` 44 checks, `test_embedded_python.py` 4 tests.
Plants: a3, a4, the D count with the header, the create's stdout dropped, the
JSON without the failed rows: all caught. The label line removed: both tests
pass. `label-lines.sh` holds the script's label lines; by hand they give
nothing for no failed row, `model; after failed row 3`, `model; after failed
rows 1,3`, and exit 64 for `x` or `1,3`.

Left, none blocking: the label's text is in no test; the step summary for the
row the sequence ends on says "the rows after it ran" and then "the sequence
ended there" (`topology-bench.yml:411-413`); `bracket.py` does not look at
`earlier_failed`, so a labelled row is still judged.

A trap met: the host of a run's whole-log download is not allowed from this
container; job logs come through the job-log tool's tail only.

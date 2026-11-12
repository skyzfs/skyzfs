# M1.T9 (skyzfs/skyzfs#24): tpcb on the daemon against the benchmark model

Reviewed: `origin/zfs-topology` 06b6dff ... `origin/skyzfs-testing` 07bd760
(tests/bench/topology-bench.sh, .github/workflows/topology-bench.yml).

## What was done

- Read the diff before the description; read the daemon's device
  parameters and status cadence (crates/skyzfs-ublk-sys/src/queue.rs,
  bins/skyzfsd/src/serve.rs).
- Rendered the summary from two made-up rows laid out as a matrix run's
  artifacts (one directory per job).
- Figures of all nine measuring runs taken from the `sequence` and
  `combined table` job logs and compared with the description.
- Not done: no benchmark run of my own; JSON artifacts not opened; the
  cgroup arm not run on a kernel by me.

## Findings

1. Blocking. A verdict across two machines. The workflow's matrix mode
   puts each topology on its own runner and the `combined table` job
   runs `summarize all` over every artifact. With `topologies:
   "zfs-slog zfs-skyzfsd"` and no sequence the table prints a ratio and
   "at least 0.90"/"BELOW 0.90" between two runners. Shown with made-up
   rows: zfs-slog 3000 in one job directory, zfs-skyzfsd 5000 in
   another gives "1.667 | at least 0.90". Rows carry nothing that
   identifies the machine.
2. Not blocking. "times at the limit" is memory.events `max` at the end
   of the window, counted since the cgroup was made: it includes the
   copy of the image and the warm-up. The description's "in a window"
   is not what the figure is. The bound itself is shown by the other
   figures (cgroup file 855-859 MiB, host cache 1.45-1.50 GiB).
3. Not blocking. Description: "3 to 5" samples with a step under way on
   the slower runners; the logs have 5, 7, 3, 5, 12, 10, 4, 23.

## Agreed with

- The verdict "not met". All 26 ratios in the description match the
  logs. Both arms give the daemon a cache under the pool that the model
  has not, so the comparison errs in the daemon's favour.
- "Not the ublk hop": zfs-ublk / zfs-loop is 0.964, 0.966, 0.971,
  1.004, 0.945.
- The daemon is in its cgroup from its first instruction (the shell
  joins, then execs); the logs print the pid's cgroup and its limit.
- Status sampling does not load the daemon: a thread of its own writes
  the status file every 100 ms whether anyone reads it.

## For M1.3h

Device reads per second in the window (status, after minus before):
about 2000 on a slower runner, about 3100 on a faster one, all through
one worker. Samples with a step under way: 4-6% slower, 18% faster.
p999 130-180 ms and longest transaction 760-900 ms in daemon rows
against 13-24 ms and 100-280 ms in the model on the faster runners,
with 15-28 checkpoint syncs per window. The figures do not separate
waiting behind a step's write, behind a step's sync, and behind other
requests on the one worker; the timing should report those three apart.

## Second look: 2e3e936

Finding 1 dropped. Each row carries the boot id; a ratio is formed only
between rows of the same one. Rendered six made-up directories: two
machines, one machine, no field, empty field, a model row without the
field, two machines with a model each. A verdict only where the ids are
equal and not empty. The JSON of the nine measured runs has no such
field and now renders NO VERDICT; their ratios stand in the logs.
Left: a virtual machine resumed from a memory snapshot would share its
boot id with its siblings; GitHub's runners boot, so this is for other
hosts only.

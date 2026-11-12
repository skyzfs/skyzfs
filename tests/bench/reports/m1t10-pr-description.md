## Work package

**M1.T10: where a request's time goes in the daemon, per benchmark row.** Commits `7ccc80f`, `3b544dc`, `26e4395`, `5d6f42b`, `db864d3`, `d8f2abe` on `skyzfs-testing-m1.t10`, off `zfs-topology` at `58e1b54`. Test code only: `tests/bench/skz_time.py`, `tests/bench/test_skz_time.py`, `tests/bench/topology-bench.sh`, and one line in each of `ci.yml` and `skyzfs-ci.yml`.

M1.T9 found that the daemon's tpcb row is not within 10% of the benchmark model (`zfs-slog`) on every machine (`docs/skyzfs-design.md` §9). Since status version 5 the daemon counts where each request's time goes (153 `time_*` keys). This package reads those keys before and after the measured window of every daemon row and prints the difference as tables, so that the next product change is chosen from a measurement. It judges nothing: the benchmark's verdict is unchanged.

**What the ten runs say, in short.** Every figure below is from one run; rows of a run shared one machine; no figure is averaged across runs.

- The runners came in two kinds, told apart by what a sync of the daemon's files costs, with nothing in between: five runs where the checkpoint step's sync of the home image takes 3.6 to 4.0 ms a step (**fast sync**), five where it takes 37 to 46 ms (**slow sync**). The slow-sync machines are the ones with the higher model throughput in four of five runs.
- On fast-sync machines the daemon is at 0.93 to 0.95 of the model unbounded (0.91, 0.92 in a 1 GiB cgroup). On slow-sync machines it is at 0.85 to 0.93 (0.82, 0.86 bounded), except run 43166016571 (1.02, 1.07), whose model row was slow.
- A read spends most of its time in the daemon **waiting for the one worker**, not being served: 183 to 199 µs of 259 to 280 on fast-sync machines, 564 to 755 µs of 603 to 818 on slow-sync ones, while the call itself takes 15 to 38 µs. The worker is idle 83 to 85% (fast sync) and 69 to 73% (slow sync) of the time: reads wait behind long single activities, not behind load.
- On slow-sync machines half of a read's wait is behind the step's home sync (47 to 52%) and a fifth to a quarter behind the rule's own sync (17 to 28%); the client's flush adds 7 to 12%.
- The gate (the limit on a step while requests wait) did not engage in any of the twenty rows. At this load it cannot, by construction, so the 0 is what the code gives and says nothing about the gate's worth.

The lead's decision from these numbers (reads on a thread of their own, M1.3j) is recorded in the design, not here.

## What was done

- **`tests/bench/skz_time.py`** (no kernel, no daemon needed): `delta()` takes two status readings and the times they were read and gives the window's difference, keeping apart the keys that are a state at the end (a maximum, a cause) from the ones that count; `render()` prints the tables. A reading that cannot be trusted (a key missing, a counter that went down, a daemon restarted) is kept and its row is labelled "(DOUBTFUL READING)" in every table.
- **The tables**, per daemon row: the window; a request's life for reads, writes and flushes (submit, wait for the worker, the call, ready to taken, taken to committed), for reads with totals beside what postgres counted for its relation reads and beside the model row's mean; the wait by cause in fourteen activities and in six groups; flushes beside reads; the worker's time by activity with its busy share; the checkpoint step in its six parts with steps per second, reads per step, the walk's share, the gate; the rule's own sync per sync, in a table of its own; the longest single one of each part SINCE THE DAEMON STARTED (not of the window); the pump's store; the longest wait of each kind with its cause; the identities.
- **`topology-bench.sh`**: the status snapshot carries the time it was read; the row's JSON carries the difference under `skyzfsd.time`; `summarize` renders it. A summary can be re-rendered from a run's `topology-bench-sequence` artifact with `bash tests/bench/topology-bench.sh summarize DIR`.
- **`tests/bench/test_skz_time.py`**, 19 tests, in `ci.yml` and `skyzfs-ci.yml`. It reads the key list from `crates/skyzfs-ublk/src/status.rs` (and expects 153, so a new key fails here until the tables know it), and checks every printed figure of the step, own-sync, flush, read-total, longest and store tables against a reading made by hand. After review: seven planted errors (a unit under the wrong header, the walk's share over seven parts, the own sync divided by steps, the mean flush call over the reads' count, the daemon's read total in the wrong unit, the gate's unit, the doubtful mark removed) each fail a test.

## How it was verified

`python3 tests/bench/test_skz_time.py`: 19 tests pass. `git ls-files '*.sh' | xargs -r shellcheck`: clean but for one SC2002 in `daemon-rig.sh` that the older local shellcheck reports and CI's does not. CI on the pull request.

**The runs.** Settings as in M1.T9: pgbench tpcb-like, scale 100, 8 clients, 30 s warm-up and 120 s measured, postgres in a 1 GiB cgroup, ARC 768 MiB, pool of 24 GiB, origin delay 0, `workloads: tpcb`. Sequences: `model, daemon-A, daemon-B` (the same daemon settings twice) and `model, bounded, unbounded` (`TB_SKYZFSD_MEM=1G` on the bounded row). The daemon is a release build of `zfs-topology` at `58e1b54`. Eight runs were made at `7ccc80f` to `5d6f42b` and two (43173407710, 43173412352) at `db864d3`; **all tables here were re-rendered from the runs' artifacts** with the script below over the JSON, and the two newest runs' summaries also with `d8f2abe`'s `summarize`. No benchmark was run at `d8f2abe` itself: it changes how the tables are printed, not what is recorded. One run per figure throughout.

| run | image | model tps | daemon rows: tps (ratio) | home sync, ms per step | own sync, ms each (count) | flush call, ms | kind by sync |
|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | 26.04 | 5291 | bounded 4361 (0.824), unbounded 4662 (0.881) | 39.1, 38.0 | 363 (21), 343 (23) | 65.2, 70.3 | slow sync |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | 24.04 | 4874 | daemon-A 4194 (0.860), daemon-B 4121 (0.846) | 37.3, 39.1 | 329 (20), 329 (21) | 81.8, 78.6 | slow sync |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | 24.04 | 4628 | bounded 3958 (0.855), unbounded 4156 (0.898) | 37.6, 37.9 | 322 (18), 375 (19) | 87.1, 81.5 | slow sync |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | 26.04 | 4000 | daemon-A 3580 (0.895), daemon-B 3704 (0.926) | 39.7, 37.2 | 300 (16), 319 (17) | 97.7, 86.3 | slow sync |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | 26.04 | 3649 | daemon-A 3427 (0.939), daemon-B 3414 (0.936) | 3.9, 3.8 | 24 (15), 23 (14) | 6.4, 7.2 | fast sync |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | 26.04 | 3584 | bounded 3296 (0.920), unbounded 3381 (0.943) | 3.6, 4.0 | 21 (15), 21 (15) | 6.2, 7.0 | fast sync |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | 24.04 | 3457 | daemon-A 3250 (0.940), daemon-B 3208 (0.928) | 3.8, 3.9 | 21 (13), 21 (17) | 7.6, 5.7 | fast sync |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | 24.04 | 3398 | daemon-A 3206 (0.943), daemon-B 3177 (0.935) | 3.7, 3.6 | 21 (15), 23 (13) | 6.6, 6.9 | fast sync |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | 24.04 | 3311 | bounded 3006 (0.908), unbounded 3161 (0.954) | 4.0, 3.8 | 21 (13), 21 (14) | 7.7, 7.1 | fast sync |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | 26.04 | 3124 | daemon-A 3197 (1.023), daemon-B 3331 (1.066) | 42.0, 46.4 | 318 (12), 326 (15) | 92.7, 84.5 | slow sync |

Home sync is the step's sync of the home image, per step. Own sync is the sync the rule takes for itself (`checkpoint_syncs`), per sync. The flush call is a client's flush as the worker serves it. **Which sync is which:** on slow-sync machines a client's flush takes 65 to 98 ms and the rule's own sync 300 to 375 ms; on fast-sync machines 5.7 to 7.7 ms and 21 to 24 ms.

**What tells the kinds apart.** The daemon's home sync per step and its flush call, each with a factor of ten between the kinds and the same in a run's second and third row. The benchmark's own `8k sync alone` test does not (below): it is one small write and a sync, and what is slow on those machines is a sync after much was written.

### A read

| run | row | reads | submit | wait | call | ready→taken | taken→committed | sum | in all, s | postgres: reads | µs each | model row's | s in all | daemon's share |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 349445 | 3.2 | 682.3 | 136.9 | 22.1 | 0.4 | 844.8 | 295.2 | 712076 | 478.8 | 173.5 | 340.9 | 86.6% |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 372438 | 5.2 | 564.0 | 14.9 | 18.9 | 0.3 | 603.3 | 224.7 | 762312 | 369.3 | 173.5 | 281.5 | 79.8% |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 336701 | 5.9 | 579.9 | 19.0 | 21.0 | 0.4 | 626.2 | 210.8 | 689963 | 396.7 | 183.8 | 273.7 | 77.0% |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 327651 | 5.6 | 624.3 | 23.2 | 21.1 | 0.4 | 674.6 | 221.0 | 676695 | 417.5 | 183.8 | 282.5 | 78.2% |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 310570 | 3.6 | 714.5 | 143.0 | 24.3 | 0.4 | 885.7 | 275.1 | 650739 | 504.9 | 196.9 | 328.6 | 83.7% |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 332910 | 5.4 | 590.1 | 20.6 | 20.1 | 0.4 | 636.7 | 212.0 | 682719 | 404.6 | 196.9 | 276.2 | 76.7% |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 276747 | 7.4 | 669.5 | 30.7 | 24.6 | 0.4 | 732.6 | 202.7 | 583234 | 433.5 | 189.3 | 252.8 | 80.2% |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 292852 | 7.4 | 621.3 | 22.9 | 24.3 | 0.4 | 676.4 | 198.1 | 604126 | 411.9 | 189.3 | 248.8 | 79.6% |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 265011 | 10.0 | 189.1 | 31.8 | 34.0 | 0.5 | 265.4 | 70.3 | 558313 | 241.1 | 177.0 | 134.6 | 52.2% |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 267912 | 9.7 | 183.3 | 31.1 | 34.4 | 0.6 | 259.0 | 69.4 | 556572 | 241.4 | 177.0 | 134.3 | 51.7% |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 254011 | 7.0 | 243.4 | 150.7 | 38.1 | 0.7 | 439.8 | 111.7 | 537150 | 319.9 | 180.1 | 171.8 | 65.0% |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 264780 | 9.7 | 182.9 | 31.5 | 34.3 | 0.6 | 259.0 | 68.6 | 551012 | 243.2 | 180.1 | 134.0 | 51.2% |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 250204 | 8.9 | 191.5 | 35.5 | 34.8 | 0.6 | 271.4 | 67.9 | 531171 | 254.5 | 190.3 | 135.2 | 50.2% |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 249546 | 9.1 | 192.2 | 36.9 | 34.0 | 0.7 | 272.9 | 68.1 | 525599 | 260.1 | 190.3 | 136.7 | 49.8% |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 247385 | 8.9 | 199.1 | 37.4 | 33.7 | 0.6 | 279.7 | 69.2 | 524311 | 259.6 | 194.6 | 136.1 | 50.8% |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 248103 | 8.9 | 190.9 | 38.4 | 34.3 | 0.6 | 273.1 | 67.8 | 520363 | 260.5 | 194.6 | 135.5 | 50.0% |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 230847 | 6.2 | 280.4 | 170.8 | 37.7 | 0.6 | 495.8 | 114.4 | 491985 | 355.6 | 199.2 | 174.9 | 65.4% |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 245610 | 9.0 | 199.0 | 38.3 | 33.5 | 0.7 | 280.4 | 68.9 | 517070 | 260.3 | 199.2 | 134.6 | 51.2% |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 244170 | 7.0 | 695.8 | 29.0 | 24.7 | 0.4 | 756.8 | 184.8 | 521505 | 441.6 | 248.9 | 230.3 | 80.2% |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 262886 | 7.1 | 755.0 | 30.2 | 25.3 | 0.4 | 818.0 | 215.0 | 542218 | 486.9 | 248.9 | 264.0 | 81.5% |

`sum` is the five parts; "in all" is sum × reads. Postgres counts about twice the daemon's reads (its 8 kB blocks against the pool's requests), so the two are set side by side as totals, not per read: the daemon accounts for 50 to 52% of postgres's relation-read time on fast-sync machines and 77 to 82% on slow-sync ones (unbounded rows). The rest is above the daemon: ZFS, the kernel's block layer, the driver.

| run | row | wait | idle | step parts + own sync | of which home sync | of which own sync | flush | write+discard+zeroes | read | other |
|---|---|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 682.3 | 10.2 (1.5%) | 509.5 (74.7%) | 292.3 (42.8%) | 156.2 (22.9%) | 43.9 (6.4%) | 2.5 (0.4%) | 111.2 (16.3%) | 5.2 (0.8%) |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 564.0 | 17.6 (3.1%) | 492.8 (87.4%) | 281.6 (49.9%) | 157.4 (27.9%) | 40.1 (7.1%) | 2.5 (0.4%) | 8.8 (1.6%) | 2.3 (0.4%) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 579.9 | 20.0 (3.4%) | 492.1 (84.9%) | 287.5 (49.6%) | 148.6 (25.6%) | 54.2 (9.3%) | 2.4 (0.4%) | 9.0 (1.5%) | 2.3 (0.4%) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 624.3 | 19.4 (3.1%) | 508.2 (81.4%) | 296.1 (47.4%) | 155.3 (24.9%) | 72.2 (11.6%) | 2.8 (0.4%) | 19.2 (3.1%) | 2.5 (0.4%) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 714.5 | 12.3 (1.7%) | 519.8 (72.8%) | 307.0 (43.0%) | 145.3 (20.3%) | 73.2 (10.2%) | 3.3 (0.5%) | 100.8 (14.1%) | 5.0 (0.7%) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 590.1 | 20.2 (3.4%) | 493.5 (83.6%) | 293.0 (49.6%) | 144.5 (24.5%) | 57.2 (9.7%) | 2.8 (0.5%) | 13.8 (2.3%) | 2.6 (0.4%) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 669.5 | 20.1 (3.0%) | 540.3 (80.7%) | 327.0 (48.8%) | 136.1 (20.3%) | 75.0 (11.2%) | 2.6 (0.4%) | 28.9 (4.3%) | 2.6 (0.4%) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 621.3 | 20.2 (3.2%) | 513.3 (82.6%) | 303.5 (48.9%) | 137.7 (22.2%) | 66.6 (10.7%) | 3.4 (0.5%) | 15.5 (2.5%) | 2.5 (0.4%) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 189.1 | 29.8 (15.8%) | 135.9 (71.8%) | 35.0 (18.5%) | 10.3 (5.5%) | 5.8 (3.1%) | 4.3 (2.3%) | 10.1 (5.4%) | 3.1 (1.7%) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 183.3 | 28.0 (15.3%) | 130.6 (71.3%) | 32.7 (17.8%) | 9.5 (5.2%) | 6.6 (3.6%) | 3.8 (2.1%) | 10.7 (5.8%) | 3.6 (1.9%) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 243.4 | 22.7 (9.3%) | 142.6 (58.6%) | 31.9 (13.1%) | 9.8 (4.0%) | 5.5 (2.3%) | 4.3 (1.8%) | 62.6 (25.7%) | 5.7 (2.4%) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 182.9 | 29.4 (16.1%) | 130.3 (71.3%) | 32.9 (18.0%) | 9.4 (5.1%) | 6.1 (3.4%) | 3.9 (2.1%) | 10.1 (5.5%) | 3.0 (1.6%) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 191.5 | 30.4 (15.9%) | 129.3 (67.5%) | 32.9 (17.2%) | 8.6 (4.5%) | 7.7 (4.0%) | 8.3 (4.3%) | 12.4 (6.5%) | 3.5 (1.8%) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 192.2 | 29.9 (15.6%) | 134.4 (69.9%) | 34.3 (17.8%) | 11.3 (5.9%) | 4.4 (2.3%) | 6.6 (3.4%) | 13.1 (6.8%) | 3.9 (2.0%) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 199.1 | 29.8 (15.0%) | 138.4 (69.5%) | 33.0 (16.6%) | 9.5 (4.8%) | 5.8 (2.9%) | 7.8 (3.9%) | 13.1 (6.6%) | 4.2 (2.1%) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 190.9 | 30.7 (16.1%) | 126.6 (66.3%) | 31.3 (16.4%) | 9.0 (4.7%) | 7.3 (3.8%) | 7.5 (3.9%) | 14.5 (7.6%) | 4.2 (2.2%) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 280.4 | 21.6 (7.7%) | 152.2 (54.3%) | 35.8 (12.8%) | 9.4 (3.4%) | 8.6 (3.1%) | 8.1 (2.9%) | 83.7 (29.9%) | 6.2 (2.2%) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 199.0 | 29.7 (14.9%) | 135.5 (68.1%) | 33.9 (17.0%) | 9.6 (4.8%) | 6.3 (3.2%) | 7.6 (3.8%) | 16.4 (8.2%) | 3.4 (1.7%) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 695.8 | 20.5 (2.9%) | 558.9 (80.3%) | 357.6 (51.4%) | 119.9 (17.2%) | 85.4 (12.3%) | 5.9 (0.8%) | 22.7 (3.3%) | 2.5 (0.4%) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 755.0 | 20.7 (2.7%) | 624.7 (82.7%) | 388.9 (51.5%) | 144.1 (19.1%) | 82.9 (11.0%) | 3.7 (0.5%) | 20.6 (2.7%) | 2.4 (0.3%) |

Where a read's time goes, unbounded rows:

| | fast sync | slow sync |
|---|---|---|
| in the daemon in all, µs | 259 to 280 | 603 to 818 |
| waiting for the worker | 183 to 199 | 564 to 755 |
| the call | 31 to 38 | 15 to 31 |
| ready → taken by the pump | 33.5 to 34.8 | 19 to 25 |
| of the wait: behind the six step parts and the own sync | 66 to 72% | 80 to 87% |
| … of which the home sync | 16 to 18.5% | 47 to 52% |
| … of which the own sync | 4.5 to 5.9% | 17 to 28% |
| behind a flush | 2.3 to 4.0% | 7 to 12% |
| behind writes | 2.1 to 4.3% | 0.4 to 0.8% |
| behind other reads | 5.4 to 8.2% | 1.5 to 4.3% |
| the worker idle when it arrived (the wake-up) | 15 to 16% | 2.7 to 3.4% |

In a 1 GiB cgroup the call itself is 137 to 171 µs (the home image's pages are no longer in memory), and waiting behind other reads becomes 14 to 30% of the wait.

### A write and a flush

| run | row | writes | submit | wait | call | ready→taken | taken→committed | sum |
|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 26883 | 3.2 | 2203.4 | 64.4 | 71.1 | 7.8 | 2349.9 |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 27166 | 5.2 | 2133.2 | 70.4 | 73.4 | 9.9 | 2292.1 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 25680 | 5.9 | 2130.8 | 67.1 | 93.3 | 10.5 | 2307.6 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 25106 | 5.6 | 2119.5 | 72.4 | 85.6 | 11.4 | 2294.5 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 24916 | 3.6 | 2041.9 | 71.3 | 76.9 | 28.0 | 2221.7 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 26117 | 5.4 | 2224.0 | 71.2 | 88.0 | 8.6 | 2397.2 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 22270 | 7.4 | 2132.0 | 83.1 | 73.7 | 13.9 | 2310.0 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 22755 | 7.4 | 2309.1 | 90.8 | 87.5 | 12.1 | 2506.8 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 21779 | 10.0 | 724.3 | 103.6 | 98.3 | 12.2 | 948.4 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 21439 | 9.7 | 750.8 | 109.1 | 108.2 | 12.9 | 990.7 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 20933 | 7.0 | 787.5 | 102.2 | 114.0 | 13.7 | 1024.3 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 22382 | 9.7 | 681.9 | 101.7 | 107.6 | 16.2 | 917.0 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 20854 | 8.9 | 724.6 | 201.2 | 121.0 | 12.9 | 1068.6 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 20265 | 9.1 | 810.9 | 188.5 | 124.2 | 13.1 | 1145.8 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 20501 | 8.9 | 828.5 | 193.3 | 120.9 | 12.2 | 1163.8 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 20212 | 8.9 | 825.8 | 195.1 | 130.3 | 13.1 | 1173.2 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 20504 | 6.2 | 894.8 | 181.6 | 108.0 | 14.3 | 1204.8 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 19976 | 9.0 | 802.9 | 198.3 | 113.7 | 13.6 | 1137.5 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 22032 | 7.0 | 2249.9 | 94.9 | 76.5 | 30.0 | 2458.3 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 22869 | 7.1 | 2381.0 | 93.3 | 79.8 | 13.3 | 2574.5 |

| run | row | wait | idle | step parts + own sync | of which home sync | of which own sync | flush | write+discard+zeroes | read | other |
|---|---|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 2203.4 | 67.4 (3.1%) | 1998.9 (90.7%) | 1078.8 (49.0%) | 685.2 (31.1%) | 0.0 (0.0%) | 26.8 (1.2%) | 95.6 (4.3%) | 14.9 (0.7%) |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 2133.2 | 90.7 (4.3%) | 1988.8 (93.2%) | 1095.2 (51.3%) | 673.5 (31.6%) | 0.0 (0.0%) | 28.7 (1.3%) | 10.2 (0.5%) | 14.7 (0.7%) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 2130.8 | 125.5 (5.9%) | 1955.0 (91.7%) | 1097.7 (51.5%) | 623.7 (29.3%) | 0.0 (0.0%) | 26.2 (1.2%) | 10.2 (0.5%) | 13.9 (0.7%) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 2119.5 | 114.4 (5.4%) | 1943.3 (91.7%) | 1142.4 (53.9%) | 571.9 (27.0%) | 0.0 (0.0%) | 26.2 (1.2%) | 21.1 (1.0%) | 14.5 (0.7%) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 2041.9 | 85.3 (4.2%) | 1788.0 (87.6%) | 1061.0 (52.0%) | 482.6 (23.6%) | 0.0 (0.0%) | 28.5 (1.4%) | 124.1 (6.1%) | 16.0 (0.8%) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 2224.0 | 124.4 (5.6%) | 2037.3 (91.6%) | 1111.0 (50.0%) | 692.6 (31.1%) | 0.0 (0.0%) | 28.5 (1.3%) | 19.0 (0.9%) | 14.7 (0.7%) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 2132.0 | 98.9 (4.6%) | 1963.9 (92.1%) | 1227.5 (57.6%) | 440.5 (20.7%) | 0.0 (0.0%) | 27.4 (1.3%) | 27.5 (1.3%) | 14.2 (0.7%) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 2309.1 | 85.5 (3.7%) | 2119.8 (91.8%) | 1200.3 (52.0%) | 622.1 (26.9%) | 0.0 (0.0%) | 57.6 (2.5%) | 27.2 (1.2%) | 18.9 (0.8%) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 724.3 | 133.8 (18.5%) | 503.5 (69.5%) | 123.4 (17.0%) | 42.1 (5.8%) | 0.0 (0.0%) | 44.5 (6.1%) | 18.3 (2.5%) | 24.1 (3.3%) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 750.8 | 122.1 (16.3%) | 504.3 (67.2%) | 120.1 (16.0%) | 39.9 (5.3%) | 0.0 (0.0%) | 63.5 (8.5%) | 28.6 (3.8%) | 32.3 (4.3%) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 787.5 | 87.2 (11.1%) | 515.7 (65.5%) | 111.5 (14.2%) | 37.1 (4.7%) | 0.0 (0.0%) | 49.4 (6.3%) | 106.7 (13.6%) | 28.5 (3.6%) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 681.9 | 109.5 (16.1%) | 483.6 (70.9%) | 116.7 (17.1%) | 37.9 (5.6%) | 0.0 (0.0%) | 47.7 (7.0%) | 19.5 (2.9%) | 21.5 (3.2%) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 724.6 | 123.1 (17.0%) | 457.1 (63.1%) | 113.1 (15.6%) | 26.4 (3.6%) | 0.0 (0.0%) | 91.5 (12.6%) | 28.4 (3.9%) | 24.5 (3.4%) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 810.9 | 141.8 (17.5%) | 509.3 (62.8%) | 124.6 (15.4%) | 43.0 (5.3%) | 0.0 (0.0%) | 91.7 (11.3%) | 37.7 (4.7%) | 30.4 (3.8%) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 828.5 | 129.4 (15.6%) | 511.9 (61.8%) | 115.9 (14.0%) | 34.1 (4.1%) | 0.0 (0.0%) | 117.0 (14.1%) | 37.2 (4.5%) | 33.0 (4.0%) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 825.8 | 138.7 (16.8%) | 504.3 (61.1%) | 119.9 (14.5%) | 38.2 (4.6%) | 0.0 (0.0%) | 111.3 (13.5%) | 38.1 (4.6%) | 33.4 (4.0%) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 894.8 | 79.2 (8.8%) | 539.8 (60.3%) | 123.4 (13.8%) | 35.9 (4.0%) | 0.0 (0.0%) | 92.3 (10.3%) | 152.1 (17.0%) | 31.4 (3.5%) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 802.9 | 127.5 (15.9%) | 502.3 (62.6%) | 121.4 (15.1%) | 35.8 (4.5%) | 0.0 (0.0%) | 106.3 (13.2%) | 40.2 (5.0%) | 26.6 (3.3%) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 2249.9 | 91.9 (4.1%) | 2074.4 (92.2%) | 1254.8 (55.8%) | 533.5 (23.7%) | 0.0 (0.0%) | 52.3 (2.3%) | 18.5 (0.8%) | 12.8 (0.6%) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 2381.0 | 96.6 (4.1%) | 2199.6 (92.4%) | 1334.1 (56.0%) | 554.4 (23.3%) | 0.0 (0.0%) | 50.2 (2.1%) | 20.9 (0.9%) | 13.7 (0.6%) |

A write waits 0.68 to 0.89 ms on fast-sync machines and 2.0 to 2.4 ms on slow-sync ones, there half of it behind the home sync and a fifth to a third behind the own sync. Writes wait longer than reads of the same row: they come in bursts at a transaction group's sync, when steps are being taken.

| run | row | flushes | submit | wait | call | ready→taken | taken→committed | sum |
|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 48 | 3.2 | 118.0 | 65191.5 | 71.1 | 7.8 | 65391.6 |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 48 | 5.2 | 14.6 | 70305.6 | 73.4 | 9.9 | 70408.7 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 48 | 5.9 | 22.1 | 81844.8 | 93.3 | 10.5 | 81976.7 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 48 | 5.6 | 17.2 | 78645.4 | 85.6 | 11.4 | 78765.1 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 48 | 3.6 | 62.6 | 87107.4 | 76.9 | 28.0 | 87278.4 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 48 | 5.4 | 13.0 | 81536.8 | 88.0 | 8.6 | 81651.8 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 48 | 7.4 | 21.4 | 97702.3 | 73.7 | 13.9 | 97818.7 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 48 | 7.4 | 27.7 | 86345.3 | 87.5 | 12.1 | 86480.0 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 48 | 10.0 | 46.9 | 6396.0 | 98.3 | 12.2 | 6563.3 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 46 | 9.7 | 37.7 | 7217.7 | 108.2 | 12.9 | 7386.2 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 46 | 7.0 | 67.3 | 6247.2 | 114.0 | 13.7 | 6449.1 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 48 | 9.7 | 41.8 | 6981.6 | 107.6 | 16.2 | 7156.9 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 46 | 8.9 | 37.9 | 7603.1 | 121.0 | 12.9 | 7783.8 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 46 | 9.1 | 53.2 | 5684.5 | 124.2 | 13.1 | 5884.1 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 46 | 8.9 | 44.2 | 6622.0 | 120.9 | 12.2 | 6808.2 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 46 | 8.9 | 82.6 | 6929.6 | 130.3 | 13.1 | 7164.5 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 46 | 6.2 | 96.8 | 7714.7 | 108.0 | 14.3 | 7940.0 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 46 | 9.0 | 48.4 | 7110.1 | 113.7 | 13.6 | 7294.8 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 48 | 7.0 | 38.0 | 92707.9 | 76.5 | 30.0 | 92859.4 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 48 | 7.1 | 23.8 | 84482.4 | 79.8 | 13.3 | 84606.3 |

| run | row | wait | idle | step parts + own sync | of which home sync | of which own sync | flush | write+discard+zeroes | read | other |
|---|---|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 118.0 | 14.1 (11.9%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 102.3 (86.6%) | 1.7 (1.4%) |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 14.6 | 14.3 (98.1%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.3 (1.9%) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 22.1 | 14.9 (67.2%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 5.8 (26.1%) | 1.5 (6.7%) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 17.2 | 15.3 (89.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.8 (4.6%) | 1.1 (6.4%) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 62.6 | 16.1 (25.7%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 44.4 (70.9%) | 2.1 (3.4%) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 13.0 | 11.2 (86.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 1.4 (10.9%) | 0.4 (3.1%) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 21.4 | 19.3 (89.8%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.6 (2.7%) | 1.6 (7.5%) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 27.7 | 22.7 (82.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 3.7 (13.2%) | 1.3 (4.8%) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 46.9 | 26.3 (56.1%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 18.5 (39.6%) | 2.0 (4.3%) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 37.7 | 26.6 (70.7%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 9.5 (25.3%) | 1.5 (4.0%) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 67.3 | 17.7 (26.3%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 45.8 (68.0%) | 3.9 (5.7%) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 41.8 | 33.1 (79.1%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 6.2 (14.7%) | 2.6 (6.1%) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 37.9 | 28.2 (74.4%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 8.5 (22.4%) | 1.2 (3.2%) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 53.2 | 38.6 (72.5%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 13.0 (24.4%) | 1.6 (3.1%) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 44.2 | 33.2 (75.2%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 9.1 (20.6%) | 1.8 (4.2%) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 82.6 | 24.9 (30.1%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 55.4 (67.0%) | 2.4 (2.9%) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 96.8 | 21.0 (21.7%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 72.2 (74.6%) | 3.5 (3.6%) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 48.4 | 42.9 (88.7%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 4.5 (9.3%) | 1.0 (2.1%) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 38.0 | 31.3 (82.5%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 5.9 (15.6%) | 0.7 (1.9%) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 23.8 | 15.3 (64.6%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 0.0 (0.0%) | 7.5 (31.6%) | 0.9 (3.9%) |

A flush hardly waits (13 to 118 µs); its cost is the call. The ready→taken and taken→committed columns of writes and flushes are one figure for all requests that are not reads: the daemon counts those together.

### The worker and the steps

| run | row | busy | read | write | flush | step walk | step apply | step home sync | step log start | own sync | other |
|---|---|---|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 64.2% | 38.8% | 1.4% | 2.5% | 0.5% | 1.4% | 11.1% | 0.5% | 6.2% | 1.6% |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 30.5% | 4.5% | 1.6% | 2.7% | 0.5% | 1.2% | 11.3% | 0.5% | 6.4% | 1.7% |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 29.5% | 5.2% | 1.4% | 3.2% | 0.5% | 1.2% | 10.4% | 0.5% | 5.3% | 1.7% |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 31.2% | 6.2% | 1.5% | 3.1% | 0.5% | 1.2% | 10.9% | 0.6% | 5.6% | 1.7% |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 59.7% | 36.1% | 1.4% | 3.4% | 0.5% | 1.4% | 10.0% | 0.5% | 4.7% | 1.6% |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 30.5% | 5.6% | 1.5% | 3.2% | 0.5% | 1.2% | 10.6% | 0.5% | 5.8% | 1.6% |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 30.3% | 6.9% | 1.5% | 3.8% | 0.6% | 1.1% | 10.1% | 0.7% | 3.9% | 1.6% |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 28.4% | 5.5% | 1.7% | 3.4% | 0.6% | 1.1% | 9.4% | 0.6% | 4.4% | 1.7% |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 14.9% | 6.8% | 1.8% | 0.2% | 0.7% | 1.4% | 1.0% | 0.6% | 0.3% | 2.1% |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 14.9% | 6.8% | 1.9% | 0.3% | 0.7% | 1.3% | 0.9% | 0.6% | 0.3% | 2.1% |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 39.2% | 31.1% | 1.7% | 0.2% | 0.7% | 1.6% | 0.9% | 0.6% | 0.3% | 2.2% |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 14.9% | 6.8% | 1.9% | 0.3% | 0.7% | 1.4% | 1.0% | 0.6% | 0.3% | 2.0% |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 16.5% | 7.2% | 3.4% | 0.3% | 0.7% | 1.3% | 0.9% | 0.5% | 0.2% | 1.9% |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 16.6% | 7.5% | 3.1% | 0.2% | 0.7% | 1.3% | 0.9% | 0.6% | 0.3% | 2.0% |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 16.8% | 7.5% | 3.2% | 0.2% | 0.7% | 1.3% | 0.9% | 0.6% | 0.3% | 1.9% |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 16.7% | 7.7% | 3.2% | 0.3% | 0.7% | 1.2% | 0.8% | 0.5% | 0.2% | 2.0% |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 41.4% | 32.1% | 3.0% | 0.3% | 0.7% | 1.5% | 0.9% | 0.6% | 0.2% | 2.1% |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 16.7% | 7.7% | 3.2% | 0.3% | 0.7% | 1.2% | 0.9% | 0.6% | 0.2% | 1.9% |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 27.4% | 5.8% | 1.7% | 3.6% | 0.5% | 1.0% | 9.6% | 0.7% | 3.1% | 1.4% |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 30.5% | 6.5% | 1.7% | 3.3% | 0.6% | 1.2% | 10.9% | 0.9% | 4.0% | 1.5% |

| run | row | steps | per second | reads per step | walk | save | apply | home sync | log start | end | six parts | walk share | gate count |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | 351 | 2.85 | 996 | 1.87 | 0.00 | 4.93 | 39.13 | 1.80 | 0.02 | 47.8 | 3.9% | 0 |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | 368 | 2.99 | 1012 | 1.84 | 0.00 | 4.11 | 37.97 | 1.78 | 0.02 | 45.7 | 4.0% | 0 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | 344 | 2.79 | 979 | 1.90 | 0.00 | 4.29 | 37.27 | 1.76 | 0.03 | 45.3 | 4.2% | 0 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | 344 | 2.79 | 952 | 1.90 | 0.00 | 4.12 | 39.13 | 1.98 | 0.03 | 47.2 | 4.0% | 0 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | 328 | 2.67 | 947 | 1.79 | 0.00 | 5.15 | 37.64 | 1.84 | 0.03 | 46.4 | 3.9% | 0 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | 344 | 2.79 | 968 | 1.89 | 0.00 | 4.28 | 37.88 | 1.75 | 0.03 | 45.8 | 4.1% | 0 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | 312 | 2.53 | 887 | 2.35 | 0.00 | 4.52 | 39.68 | 2.84 | 0.02 | 49.4 | 4.7% | 0 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | 312 | 2.53 | 939 | 2.40 | 0.00 | 4.53 | 37.20 | 2.40 | 0.02 | 46.5 | 5.2% | 0 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | 304 | 2.47 | 872 | 2.90 | 0.00 | 5.49 | 3.94 | 2.37 | 0.02 | 14.7 | 19.7% | 0 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | 304 | 2.47 | 881 | 2.92 | 0.00 | 5.42 | 3.76 | 2.37 | 0.02 | 14.5 | 20.1% | 0 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | 291 | 2.37 | 873 | 2.92 | 0.00 | 6.59 | 3.60 | 2.43 | 0.02 | 15.6 | 18.7% | 0 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | 296 | 2.41 | 895 | 2.97 | 0.00 | 5.73 | 3.98 | 2.50 | 0.02 | 15.2 | 19.5% | 0 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | 288 | 2.34 | 869 | 2.98 | 0.00 | 5.39 | 3.81 | 2.29 | 0.02 | 14.5 | 20.6% | 0 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | 288 | 2.35 | 866 | 2.89 | 0.00 | 5.40 | 3.94 | 2.47 | 0.02 | 14.7 | 19.7% | 0 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | 288 | 2.35 | 859 | 3.05 | 0.00 | 5.68 | 3.68 | 2.71 | 0.02 | 15.1 | 20.1% | 0 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | 283 | 2.30 | 877 | 2.87 | 0.00 | 5.35 | 3.65 | 2.37 | 0.02 | 14.3 | 20.2% | 0 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | 272 | 2.21 | 849 | 3.33 | 0.00 | 6.94 | 4.01 | 2.53 | 0.02 | 16.8 | 19.8% | 0 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | 280 | 2.28 | 877 | 2.93 | 0.00 | 5.45 | 3.84 | 2.54 | 0.02 | 14.8 | 19.8% | 0 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | 280 | 2.28 | 872 | 2.37 | 0.00 | 4.58 | 42.00 | 2.89 | 0.02 | 51.9 | 4.6% | 0 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | 288 | 2.35 | 913 | 2.38 | 0.00 | 4.93 | 46.38 | 3.80 | 0.02 | 57.5 | 4.1% | 0 |

Step parts are ms per step. The walk is a fifth of the six parts on fast-sync machines and a twentieth on slow-sync ones. The gate count is 0 in all twenty rows (see above).

### Limits of the numbers

| run | row | worker identity, ms | ages, ms | apart, s | store ms each | completions per store | read wait max since start, ms (cause, ms) |
|---|---|---|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | bounded | +54.8 | 80, 30 | 123.2 | 0.007 | 1.29 | 975 (own_sync, 472) |
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | unbounded | +76.0 | 80, 10 | 123.2 | 0.009 | 1.38 | 963 (step_home_sync, 511) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-A | +2.2 | 110, 120 | 123.1 | 0.009 | 1.38 | 757 (own_sync, 401) |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | daemon-B | +2.9 | 100, 130 | 123.2 | 0.010 | 1.37 | 939 (own_sync, 503) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | bounded | +9.8 | 120, 40 | 123.0 | 0.020 | 1.30 | 803 (own_sync, 429) |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | unbounded | +6.0 | 130, 110 | 123.1 | 0.008 | 1.40 | 845 (step_home_sync, 398) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-A | +4.2 | 30, 40 | 123.2 | 0.012 | 1.31 | 867 (step_home_sync, 452) |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | daemon-B | +8.5 | 20, 30 | 123.1 | 0.011 | 1.38 | 802 (own_sync, 439) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-A | -0.7 | 0, 80 | 123.0 | 0.012 | 1.38 | 189 (step_home_sync, 78) |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | daemon-B | +101.8 | 100, 50 | 122.9 | 0.012 | 1.39 | 188 (step_home_sync, 79) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | bounded | +76.6 | 30, 10 | 122.9 | 0.012 | 1.30 | 185 (step_home_sync, 65) |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | unbounded | +9.4 | 40, 50 | 123.0 | 0.014 | 1.41 | 173 (step_home_sync, 63) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-A | +97.0 | 80, 50 | 123.0 | 0.012 | 1.36 | 185 (step_home_sync, 70) |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | daemon-B | -6.6 | 140, 90 | 122.8 | 0.012 | 1.36 | 181 (step_home_sync, 69) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-A | +5.9 | 130, 60 | 122.7 | 0.012 | 1.36 | 169 (step_home_sync, 56) |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | daemon-B | +140.9 | 120, 10 | 122.8 | 0.013 | 1.36 | 179 (step_home_sync, 69) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | bounded | +91.9 | 130, 70 | 122.8 | 0.013 | 1.25 | 161 (step_apply, 50) |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | unbounded | -2.9 | 120, 50 | 122.7 | 0.013 | 1.36 | 182 (step_home_sync, 72) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-A | +3.8 | 70, 30 | 123.0 | 0.033 | 1.36 | 943 (own_sync, 514) |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | daemon-B | +1.3 | 70, 20 | 122.8 | 0.013 | 1.37 | 1114 (step_home_sync, 719) |

- **The worker's identity** (its fourteen activities should sum to the time between the readings) holds to about 0.1%: −6.6 to +140.9 ms of 123 s raw, −73 to +79 ms after the two readings' ages are taken off. The worker publishes the sums of closed spans only, so the span open at a reading is missing from it, and the status file is up to its age old; the ages do not account for it row by row. The wait-by-cause identities are exact in all twenty rows and every kind.
- The two readings are 122.7 to 123.2 s apart, for a window of 120 s: the tables' rates are over the readings, not over pgbench's window.
- "Longest" figures are since the daemon started serving and include the copy of the pool's image before the benchmark.
- The pump's store takes 0.007 to 0.020 ms each, 0.033 in the first daemon row of 43166016571.
- Two kinds of machine from ten runs; a machine in between has not been seen, and nothing here says what the kinds are (the disk, the host, a neighbour).

### txg sync, and the 8 kB sync test

| run | image | row 1 | row 2 | row 3 |
|---|---|---|---|---|
| [43169906456](https://github.com/skyzfs/skyzfs/actions/runs/43169906456) | 26.04 | model: txg 1364, 8k 213 | bounded: txg 1545, 8k 203 | unbounded: txg 1549, 8k 196 |
| [43169894572](https://github.com/skyzfs/skyzfs/actions/runs/43169894572) | 24.04 | model: txg 1287, 8k 219 | daemon-A: txg 1554, 8k 235 | daemon-B: txg 1585, 8k 227 |
| [43169903002](https://github.com/skyzfs/skyzfs/actions/runs/43169903002) | 24.04 | model: txg 1299, 8k 222 | bounded: txg 1558, 8k 220 | unbounded: txg 1599, 8k 232 |
| [43173412352](https://github.com/skyzfs/skyzfs/actions/runs/43173412352) | 26.04 | model: txg 1281, 8k 209 | daemon-A: txg 1381, 8k 211 | daemon-B: txg 1334, 8k 197 |
| [43169898732](https://github.com/skyzfs/skyzfs/actions/runs/43169898732) | 26.04 | model: txg 526, 8k 252 | daemon-A: txg 601, 8k 259 | daemon-B: txg 613, 8k 251 |
| [43166026162](https://github.com/skyzfs/skyzfs/actions/runs/43166026162) | 26.04 | model: txg 497, 8k 250 | bounded: txg 636, 8k 264 | unbounded: txg 602, 8k 254 |
| [43173407710](https://github.com/skyzfs/skyzfs/actions/runs/43173407710) | 24.04 | model: txg 582, 8k 269 | daemon-A: txg 697, 8k 268 | daemon-B: txg 704, 8k 277 |
| [43166011944](https://github.com/skyzfs/skyzfs/actions/runs/43166011944) | 24.04 | model: txg 549, 8k 285 | daemon-A: txg 690, 8k 277 | daemon-B: txg 692, 8k 275 |
| [43166021441](https://github.com/skyzfs/skyzfs/actions/runs/43166021441) | 24.04 | model: txg 568, 8k 281 | bounded: txg 720, 8k 288 | unbounded: txg 695, 8k 274 |
| [43166016571](https://github.com/skyzfs/skyzfs/actions/runs/43166016571) | 26.04 | model: txg 1687, 8k 214 | daemon-A: txg 1278, 8k 269 | daemon-B: txg 1393, 8k 535 |

The 8 kB fdatasync on the WAL file system is 196 to 235 µs on slow-sync machines and 250 to 288 µs on fast-sync ones, with the daemon rows of 43166016571 (269, 535) across the line: slightly the wrong way round, and not a discriminator.

The mean txg sync, with M1.T9's eight saved runs (model row / daemon rows, ms; M1.T9's sequences ran a daemon row FIRST, M1.T10's the model row first):

| package | run | model tps | model row | daemon rows |
|---|---|---|---|---|
| M1.T9 | 43143907710 | 6036 | 832 | 1609, 1501 |
| M1.T9 | 43143923434 | 5571 | 1514 | 1561, 1447 |
| M1.T9 | 43148434483 | 5088 | 590 | 1502, 1496, 1400 |
| M1.T9 | 43148439019 | 4917 | 593 | 1507, 1479, 1394 |
| M1.T9 | 43143912986 | 3466 | 534 | 705, 705 |
| M1.T9 | 43144256861 | 3686 | 552 | 700, 695, 666 |
| M1.T9 | 43143917908 | 3548 | 1127 | 1251, 1328 |
| M1.T9 | 43148429064 | 2963 | 1194 | 1365, 1435, 1348 |

What these support, and no more:

1. On the machines where the daemon's sync is slow (known from its own keys in M1.T10; by throughput only in M1.T9, which had no such keys) the daemon rows' txg sync is 1.28 to 1.61 s in every row of both packages; on M1.T10's fast-sync machines it is 0.60 to 0.72 s.
2. The model row's txg sync on those machines has been seen from 0.59 to 1.69 s. In M1.T10's ten runs it happens to separate the kinds (1281 to 1687 ms against 497 to 582); in M1.T9's it does not (832, 590, 593 on the faster runners). It does not mark the machine.
3. So "a sync after much was written is slow on those machines" holds **for the daemon's files; for the model's row sometimes**.

Not proven: that the first row on a fresh runner is the slow one. It fits the model rows (slow when first in M1.T10's slow-sync runs, slow when not first in one of M1.T9's four) and not the daemon rows, which are slow in every position. A sequence with the model row first and last on one machine would show it.

The tables above come from `python3 -I` over the unpacked artifacts with a script kept outside the repository (it reads the same `skyzfsd.time` object the summary renders); the figures agree with the rendered summaries where both print them (own sync and flush call of 43169894572 and 43169898732 checked by hand).

## Not done, and open questions

- No verdict and no threshold on any of these numbers; the summary prints them.
- No run of the daemon with reads off the worker (M1.3j): the same sequences, as `model, daemon, daemon, model` so that the model row is both first and last, are to be run on that branch when it has green gates, until two land on a slow-sync machine.
- The kind of machine is not printed per row. It can be (daemon rows by home sync per step), if wanted.
- What the remaining 20 to 50% of postgres's read time above the daemon is made of is not measured here.
- The bounded rows are two per kind of machine; the unbounded ones eight per kind.

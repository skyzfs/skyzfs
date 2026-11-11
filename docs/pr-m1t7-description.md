## Work package

**M1.T7: the control rig, the daemon rig on the daemon, and rigs that say when the fault is their own.** Branch `skyzfs-testing-m1.t7` into `zfs-topology`, commits `c9c87df..HEAD` (31 commits, 20 files, about 2,250 lines added).

Tests, rigs and workflows only: no product code, and `docs/skyzfs-design.md` is not touched. It serves design §4.2 ("The states of a device", "Library", the daemon's command line and what a pool does when its device goes away).

**For a reviewer, what to read first.** Three things carry the judgment and deserve a close read:

1. `tests/kernel/control_check.py` (310 lines): the table `EXPECTED` of the new control rig. Each row is a claim about the ublk driver or the binding. Then skim `tests/kernel/control-rig.sh` (717 lines) for how each row is measured.
2. In `tests/kernel/daemon-rig.sh`: `refused_start_arm` and `check_b_short` (the rebuilt control and the observation after a refused start), and the three new pins `check_b_detach_held`, `check_b_stop_under_load`, `check_b_on_zfs`.
3. In `.github/workflows/skyzfs-daemon-rig.yml`: the job `plan` (which server a run drives).

Mechanical: `tests/lib/net-try.sh` and its uses (three tries for network steps), the second upload step, the extra clippy and test lines in `ci.yml` and `skyzfs-ci.yml`, the test files, and the canary's rule.

## What was done

**A new rig for the binding to the ublk driver** (`skyzfs-control-rig.yml`, `tests/kernel/control-rig.sh`, `control_check.py`, `control-holder.py`).

- It runs on a pull request that touches `crates/skyzfs-ublk-sys/`, `crates/skyzfs-ublk/` or the rig's own files, and by dispatch. Under three minutes per image (Ubuntu 24.04 and 26.04).
- It first runs the crate's own kernel tests, which are ignored by default (root and the driver) and which no workflow of the repository ran before. They are limited to ten minutes, their output is shown as it comes, their ten `control_*` verdicts are in the table, their detail lines are shown and not judged, and `pin_kernel_tests_end` fails when the test binary ends with anything but 0 or does not end.
- Then it takes the binding through the driver's states one step at a time with the crate's test program (`skyzfs-ublk-control`, feature `control-test-program`), on its own devices and beside devices of the ublk project's reference server: added and never started, live, waiting after a kill, taken over, deleted while a client holds it, deleted while its server holds a request, and what it says of a stranger's device.
- The job fails unless every line is as `EXPECTED` has it, in either direction; two controls must fail.
- "The number is free" means an add under it succeeds, not a lookup that finds nothing: the crate's own test saw an add answered "taken" right after such a lookup (1 of 39 on both images in run 43085579233).

**The daemon rig** (`skyzfs-daemon-rig.yml`, `daemon-rig.sh`, `daemon_check.py`).

- *Which server.* On a pull request the workflow drove the stand-in, also when the pull request changed the daemon, so a green rig said nothing about `skyzfsd`. A job `plan` now decides and says so in its summary: a pull request runs the daemon, and the stand-in as well when it changes `tests/kernel/` or the workflow; by dispatch the input `server` is `daemon` (default), `stand-in` or `both` (the input `daemon` is gone); a push of the workflow file on a testing branch runs the stand-in. Artifacts are named `daemon-rig-<image>-<server>`.
- *Paths.* A pull request that touches `crates/skyzfs-ublk-sys/` or `crates/skyzfs-local/` now starts the rig.
- *Three new pins for the daemon in check b* (WAITING on the stand-in, which cannot carry them): `pin_detach_of_held_says_6` (a stopped device that a process holds open: `detach` exits 6, the number stays taken, and an add under it succeeds within 5 s of the holder closing it); `pin_orderly_stop_under_load` (SIGTERM while a writer writes and syncs: exit 0, the next serve finds nothing applied and not acknowledged, no write fails, every file reads back); `pin_serve_on_zfs_refused` (volume directory, then run directory, on a ZFS dataset: an error that says so, no device added).
- *The short-recovery control, rebuilt* (`ctl_recovered_log_too_short`). The server is killed at rest (writer stopped, pool synced, writes unchanged for a second, nothing held, `unstored 0`), so the log's newest record is an acknowledged write and `serve --recover-short 1` must refuse; killed under load the line was a coin toss. It judges the daemon's answer and nothing else: exit 4, no ready line, the device live at no look afterwards. If the start serves instead, `recovered_unacknowledged 0` with `checkpoint_step none` before the kill is the finding (a short log let through); anything else is the rig's error "the control did not happen". It no longer judges whether the pool suspends: that is ZFS's reaction and is reported, see the finding below.
- *Two observations, made with the daemon only:* `obs_b_refused_start_under_writes` and `obs_b_refused_start_no_log_device` (the same course on a pool with and without a separate log device). After the refusal: the pool's and the device's state at every change for up to 300 s; what waited at the device; at 10, 60, 110 and 170 s and at the end the pool's transaction groups, a sample of its events with a count by class, `zpool status` with the device's error counters, and the kernel stacks of ZFS's threads; a second synced writer at 120 s, a read of a file on a dataset that caches no data at 180 s, a scrub at 240 s; and what an operator would see.
- *Lines that wait.* Four checkpoint pins and `pin_failed_volume_while_a_request_is_held` are in the table as WAITING, also on the daemon; the set `WAITS_FOR_THE_DAEMON` names them. See "Not in this package".
- *Lost artifacts.* The rig prints the last lines of every serve log, and the transaction groups and `txg_sync` stacks of each arm, into the job's log; both rigs try the artifact upload a second time when the first fails.
- Smaller: take-over times per kill (`obs_b_takeover_ms`); a stricter count of overlapping writes; the scrub observation of check d says whether the scrub was still running at the failure.

**Rigs that say when the fault is their own** (`tests/lib/net-try.sh`; the kernel probe, the replay rig, the daemon rig, the control rig): a step that needs the network is tried three times; if a name still does not resolve the result is the rig's error (exit 2, "not a verdict"), not a failed check.

**CI:** `ci.yml` and `skyzfs-ci.yml` also lint and test `skyzfs-ublk-sys` with `control-test-program`. The mirror canary reports and does not count two package groups that cannot install on a runner for reasons that are not the mirror's, each by a rule stated in the file.

## How it was verified

Every figure is from GitHub's runners. Each image job is its own machine, so the two images are never compared with each other beyond "they agree". The number of runs behind a figure is stated.

**The package's evidence on the daemon as it is now (it checkpoints since #17): the whole rig, all checks, at `42bc50b`** — [run 43113307143](https://github.com/skyzfs/skyzfs/actions/runs/43113307143), one run per image. Judge 0 on both, no line differs: 39 PASS, 13 WEAKER, 12 NOT_EXERCISED, 5 WAITING, 6 expected FAIL, 0 ERROR. Kernels 7.0.0-1017 (24.04) and 7.0.0-1019 (26.04), ZFS 2.4.1, zed not running.

- The three new pins pass on both images: `detach` exit 6 after 5.1 / 5.2 s, an add 120 / 137 ms after the holder closed; the orderly stop with 2575 then 3004 writes counted just before the signal (26.04: 1850 then 2790), exit 0, `recovered_unacknowledged 0`, 16 files read back; both ZFS refusals with exit 1 and no device added. Two earlier runs of check b agree ([43087975967](https://github.com/skyzfs/skyzfs/actions/runs/43087975967), [43107654994](https://github.com/skyzfs/skyzfs/actions/runs/43107654994)).
- The control: `serve --recover-short 1` exited 4 and served nothing on both images, with `checkpoint_step none` and `checkpoint_steps` 32 / 24 before the kill. It has refused in every at-rest job so far (12 of 12 over six runs).
- The kill test: writes in flight before 20 of 20 kills, no failed write, scrub clean. Take-overs by the rig's clock 527 to 1287 ms (24.04) and 545 to 1067 ms (26.04) with no growth: median of the first five and of the last five 1031 / 1033 ms and 799 / 1056 ms. By the daemon's own lines over 22 starts the log found was at most 134,320,128 bytes (128 MiB + 102 KiB) and recovery took at most 970 / 877 ms. Before #17 the same test took 4.1 s rising to 45.1 s over the 20 kills (run 43107654994, 26.04, one run).

**Cost per image on the daemon** (run 43113307143, rig step, which includes the build):

| | 24.04 | 26.04 | before #17 and this package (run [43086281904](https://github.com/skyzfs/skyzfs/actions/runs/43086281904)) |
|---|---|---|---|
| rig step | 34 min 2 s | 32 min 42 s | 36 min 44 s / 37 min 31 s |
| build and setup | 2:04 | 1:56 | 1:52 / 1:45 |
| a | 3:02 | 3:03 | 3:03 / 3:04 |
| b, main block | 9:18 | 9:22 | 18:13 / 18:44 |
| of it: 20 kills | 1:25 | 1:26 | 9:49 / 10:10 |
| of it: the device waiting 300 s | 5:17 | 5:17 | 5:18 / 5:19 |
| c | 2:53 | 2:57 | 2:53 / 3:05 |
| e | 5:08 | 5:07 | 5:08 / 5:07 |
| d | 3:22 | 3:29 | 3:18 / 3:26 |
| the control and its two arms | 8:07 | 6:40 | 2:11 / 2:13 (the control alone) |

So the daemon's checkpoints took about nine minutes out of check b and this package's two arms put about five back. On the stand-in the rig takes 32 to 35 minutes (below).

**The stand-in, all checks, at `087021f`** — [run 43100359705](https://github.com/skyzfs/skyzfs/actions/runs/43100359705): judge 0 on both images, 35 PASS, 15 WEAKER, 10 NOT_EXERCISED, 10 WAITING, 5 expected FAIL; rig step 32 min 26 s and 34 min 36 s. The job `plan` chose the stand-in for the push and the daemon for the dispatches.

**The control rig, at `087021f`** — [run 43100359589](https://github.com/skyzfs/skyzfs/actions/runs/43100359589), green on both images; its table in full at `454d12f`, [run 43085579233](https://github.com/skyzfs/skyzfs/actions/runs/43085579233): 43 PASS, 2 expected FAIL, 0 ERROR; the crate's kernel tests 10 of 10 in about 10 s; whole job 2 min 26 s and 2 min 40 s.

**Delete under a held request** (control rig, two runs per image, [43074927907](https://github.com/skyzfs/skyzfs/actions/runs/43074927907) and [43077529820](https://github.com/skyzfs/skyzfs/actions/runs/43077529820); the images agree within about 100 ms): with a read held 20 s, the delete returns after 18.0 s; meanwhile a lookup of that device takes 17.0 s, an add under another number 17.0 to 17.1 s, a delete of an idle device 17.0 s, and a lookup of a free number 22 to 142 ms. With the server killed 5.0 s after the delete, the delete returns about 15 ms after the kill and the held read fails with EIO. (In design §4.2 already.)

**A finding: a pool that only writes does not notice that its device is gone** (in design §4.2 at `c9c87df` already; these are the runs behind it). After the refused start the daemon deletes the waiting device, which the pool still holds: no block node, and the driver lists the number as DEAD. Then, the same in every arm of five runs of check b on the daemon ([43093721420](https://github.com/skyzfs/skyzfs/actions/runs/43093721420), [43100211375](https://github.com/skyzfs/skyzfs/actions/runs/43100211375), [43100887191](https://github.com/skyzfs/skyzfs/actions/runs/43100887191), [43107654994](https://github.com/skyzfs/skyzfs/actions/runs/43107654994), [43113307143](https://github.com/skyzfs/skyzfs/actions/runs/43113307143); one run per image each):

- The pool and the vdev read ONLINE while the vdev's WRITE error counter runs up by millions (in the last run 1.85M at 10 s, 9.73M at 170 s on 24.04; up to 43.0M at the end in an earlier one). The transaction group that began to sync a few seconds after the refusal never finishes: the list of transaction groups is identical at 10 s, 60 s and the end, and `txg_sync` stands in `spa_sync → dsl_pool_sync → zio_wait`.
- The pool posts only `ereport.fs.zfs.data` events (failed writes re-executed: `zio_err` ENXIO, flags IO_RETRY REEXECUTED), about 14,000 a second on 24.04 and 27,000 on 26.04 by their numbering in run 43100887191. No I/O, probe, state-change or removal event.
- With writes only the pool was never seen to suspend (up to 351 s). It suspends within one or two seconds of the first read that must reach the device: the read of the uncached file (begun at 192 / 180 s with a log device and 206 / 181 s without in the last run; SUSPENDED at 193 / 181 s and 207 / 182 s), or the scrub in the run that had no such read.
- A synced 4 MiB write started in that state returned 0 after 132 to 244 ms on a pool with a separate log device in 9 of 10 jobs, and hung on a pool without one in 6 of 6. The tenth job with a log device is the one where the first writer's acknowledged files had gone on from 165 to 213 (192 MB) with the device gone; there the second writer hung.
- What an operator sees meanwhile: `zpool status` says `state: ONLINE` with "One or more devices has experienced an error resulting in data corruption", the vdev ONLINE with its WRITE count, and the writer's file under "Permanent errors" from 10 s on.

**What bounds each step of the arms.** Every command that touches the pool after the refusal runs under `lim` (which gives up on a process a kill does not end). Two faults of mine on the way are worth knowing, because they show what such a pool does to a tool that reads from it. `zpool events` does not end while the pool posts events faster than they are read: an unbounded dump wrote up to 584 MB a file (run 43100887191), and a count over all events ran into its limit at every snapshot and made the probes late by 37 to 88 s (run 43107654994, where the 26.04 job's two arms took 15 min 49 s for that reason and because its daemon did not checkpoint yet; it ended green). Both reads are now cut by `head` (1500 lines, 20000 events) under a limit of 15 s. No other command in the arms reads a source the pool refills: the transaction groups and the stacks are single reads of `/proc`, `zpool status` and `zpool list` are single answers under `lim 20`, and the kernel log is read once at the end.

**Mirror canary's rule:** [run 43077319473](https://github.com/skyzfs/skyzfs/actions/runs/43077319473), green on both images with the two groups reported as "expected here, not counted".

**Locally:** `python3 tests/kernel/test_daemon_check.py` (33 tests), `python3 tests/kernel/test_control_check.py` (17), `bash tests/kernel/test_net_try.sh`, `shellcheck -x -S warning` over `tests/kernel/*.sh` and `tests/lib/*.sh`, every workflow parses as YAML.

## Not done, and open questions

**Known gaps in what is here.**

- After the whole-rig run at `42bc50b` one commit changed text only: the WAITING lines of the four checkpoint pins said "the daemon does not checkpoint yet (M1.3c)", which stopped being true when #17 merged. They now say that the rig's code for them is not built. No run has that wording; the table tests cover it.
- On 24.04 the count of events by class was "not read" in the last run (the files do not exist there; on 26.04 they do: `ereport.fs.zfs.data` and nothing else). I have not found why; the sample of events beside it is there on both.
- In the final form of the arms the scrub at 240 s never runs, because the read at 180 s suspends the pool first. What a scrub does alone is M1.T8's first arm.
- One thing in run 43100211375 is not explained: in the two courses where a scrub suspended the pool, the driver stopped listing the device at that moment; in every course where a read did, it stays listed as DEAD. Whether a scrub's suspension makes the pool let go of the device is not established.
- No rig line for a stranger's device under the number of a deleted, unheld device (`detach` must answer 0): the rig cannot place the stranger before the daemon's look reliably, and a unit test covers it. Decided with the lead.

### Not in this package (M1.T8)

A green daemon rig after this package is **not** a verdict on the daemon's checkpoints. Four pins are WAITING, on the daemon too, and say so in the summary:

| pin | what it will check | what it waits for |
|---|---|---|
| `pin_takeover_time_does_not_grow` | over the kill test the median of the last five take-overs is at most twice that of the first five plus 1 s | the rig's code (the figures are already recorded in `obs_b_takeover_ms`) |
| `pin_log_stays_below_high_mark` | under the kill test's load the log never holds more than the high mark plus one step plus one request | the rig's code: a status read before each kill |
| `pin_kills_inside_steps_lose_nothing` | kills that land inside a checkpoint step lose nothing | the rig's code, with the daemon's hook `--step-pause-until` |
| `pin_c_suite_with_low_marks` | the conformance suite with checkpoint steps between its operations | the lowest marks a volume can have, to be asked of the author |

`pin_failed_volume_while_a_request_is_held` is WAITING until M1.3d says what the failure path does while a request is held.

M1.T8, in the order the lead set:

1. **From the refused-start finding, five more observation arms:** a scrub alone 10 s after the refusal (how long until SUSPENDED, does the command return, does the pool let go of the device); `zpool export`, `reopen` and `clear` in the spinning state; the course with zed running; the same after a failure while serving (after M1.3d); and the unexplained release above.
2. **From #17 (checkpoints):** the kill test's status line (largest `checkpoint_steps`, `checkpoint_syncs`, `log_live` before the kills) with a rig ERROR when no step was taken under that load; the code for the four pins above; kills inside a step, also a step of discards, and a power cut with records the daemon synced and the client never flushed; perhaps the author's "checkpoint step on a kernel" as a check.
3. **From M1.3d:** the same-process delete in the control rig (`--delete-after-ms`), `pin_failed_volume_while_a_request_is_held`, and the bookkeeping of `pin_failed_unheld_volume_serves_when_acknowledged` once a refused start deletes the device.
4. **For M1.3e:** an observation of write-zeroes records at the log's start letting the log outgrow the high mark.

Also not started: a floor for the overlap counts from measured figures; `failed-volume-obs.sh` on `klog.sh`; the pool as the holder of a deleted device in check d. The observation's two arms are to shrink to one short course once M1.T8's arms have answered what the design asks.

### Draft for the guide (CLAUDE.md, "Easy to miss"), replacing the daemon-rig bullet and the sentence that no workflow runs the kernel tests

> - **The daemon rig.** `skyzfs-daemon-rig.yml` runs on a pull request that touches `tests/kernel/`, `bins/skyzfsd/`, `crates/skyzfs-ublk/`, `crates/skyzfs-ublk-sys/`, `crates/skyzfs-local/`, `crates/skyzfs-blk/` or `bins/skyzfs-blkcheck/`, and by dispatch. It drives a server through the kernel and ZFS on Ubuntu 24.04 and 26.04: parameters, overlapping writes, `kill -9` under load, a device that waits 300 s for a server, the conformance suite through the block device, a failed volume, a held request. A pull request runs it on `skyzfsd` (built with its test hooks), about thirty-four minutes per image; one that changes `tests/kernel/` or the workflow also runs it on the stand-in (`tests/kernel/skyzfsd-ref.sh`, about thirty-three minutes), which is what shows that the rig still catches what it caught. By dispatch the input `server` chooses `daemon` (the default), `stand-in` or `both`, and `checks` the checks (`b` alone is about eighteen minutes on the daemon). The job `which server` says in its summary what was chosen, and artifacts are named `daemon-rig-<image>-<server>`. A green run on the daemon is not a verdict on its checkpoints: four pins are WAITING until M1.T8 builds them, and the summary says so. The summary says for each line whether it rests on the kernel, on the server, or only shows that the check works. Exit 2 is an error of the rig and not a verdict on the server. Read the summary's table and not the colour. When check c fails, download the run's `c-trace.tsv.gz` and run `skyzfs-blkcheck --replay c-trace.tsv VOLUME`: it replays that volume's calls through the model with no kernel and prints the same violation, or shows that the rig's bookkeeping and not the server was at fault.
> - **The control rig.** `skyzfs-control-rig.yml` runs on a pull request that touches `crates/skyzfs-ublk-sys/`, `crates/skyzfs-ublk/` or the rig's own files, and by dispatch; under three minutes per image. It runs the binding's kernel tests (ignored tests; this is the workflow that runs them) and then takes the binding through the driver's states with its test program, beside devices of the reference server. It fails when a line differs from the table `EXPECTED` in `tests/kernel/control_check.py`, in either direction.
> - **A pool whose device was deleted goes on reading ONLINE while it only writes**, and posts tens of thousands of events a second. A tool that reads `zpool events` from it does not come to an end: cut such a read with `head` and a limit.

(The figure "about eighteen minutes" for check b alone on the daemon is the sum of the last run's sections for build, b and the control's arms; it has not been run alone on the checkpointing daemon.)

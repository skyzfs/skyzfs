## An independent look at the M1 exit round (8697790)

**The verdict holds, and it holds more simply than the notes put it.** I read the round in the notes and design §9, and then checked them against the runs they name. Every figure the verdict rests on is in those runs. Nothing I found changes it.

One finding is about the record, not the result: the round classes machines differently from the rule fixed before the data. The short `dm-0` cell comes from that change. Two smaller points are suggestions.

### What I checked

**The benchmark** (runs 43573424221 to 43573498706). I read all thirteen job summaries.
- Each run is green on 848ab28, with thirteen different boot ids. The kernels and sync rates are those in the notes' table.
- Every daemon row was built from 848ab28 with no serve arguments: B rows at 1G, U rows with no limit.
- Every tps in the notes' table is the summary's. Every ratio is the summary's own, to 0.001.
- Only six rows have no verdict, all for a wide bracket: four in job 7 and two in job 12. No row lost its verdict to an origin delay or a mismatch of settings.
- From the rows I recomputed each cell under the flush rule alone. The medians, the counts of rows and jobs, and the ranges are the notes'.
- Job 7's work-disk write times are as the notes say: 0.35 to 0.70 ms a write on the daemon rows, and 1.50 ms on the last model row.

**The daemon rig** (run 43573413024). I read all four artifacts. On each image:
- 47 pins PASS, 5 controls FAIL, 2 NA and 1 WAITING. The summary says every line is as `EXPECTED` has it.
- The suite's cases: 1 PASS, 13 WEAKER, 12 NOT_EXERCISED.
- `zfs-stuck.tsv` is empty, and the daemon was built from 848ab28.
- `pin_kill_under_load`: writes were in flight before 20 of 20 kills. No application write failed, the vdev counted 0 errors, and the scrub was clean.
- `pin_device_waits_for_a_server`: QUIESCED at 30 of 30 looks in 300 s.

**The replay** (run 43573410351). I read both summaries.
- Every record prefix was checked: 2690 of 2690 on `ubuntu-26.04` and 2619 of 2619 on `ubuntu-26.04-arm`.
- 30 and 29 transaction groups were deep-checked, at 60 and 58 prefixes.
- The controls are as expected, and each run ended with exit 0. Both say "Design §9's replay criterion: shown by this run."

### 1. The round classes machines by work disk; the rule fixed before the data classes them by sync rate

**The rule.** At 848ab28 the notes state it in "The rule the criterion is stated by from here on": the criterion is met "for each kind of machine (slow-sync and fast-sync)". They add that "the rule was fixed before any row it will judge was measured". Further on they say: "the device's name does not tell the kind of machine, the measured rate does." arm64 was added as one more kind (M1.T21).

**The round.** It classes amd64 machines by work disk: nvme, sda and `dm-0`. That splits off `dm-0` as a fourth kind with one job, which is too few to count. Design §9 now says "A fourth kind, a `dm-0` work disk, drew one job … so the exit does not claim it."

**"Decided before the round" has no earlier record.** The notes say "Decided before the round: a short cell is named and not claimed." I found that sentence in no commit before 8697790, which was written two hours after the build (848ab28). The classing by work disk is also new in 8697790. M1.T29 grouped its thread-count table by device, but it did not judge the criterion that way.

**By the rule as it was fixed, every cell is met and none is short.** This uses the same rows and the flush rule alone. Fast-sync means a buffered sync over 1000 MiB/s; the rates are 2759 to 3226 MiB/s against 178 to 207, so any threshold between them gives the same split.

| kind (pre-round rule) | jobs | unbounded | bounded to 1 GiB |
|---|---|---|---|
| fast-sync amd64 | 2 (`dm-0`), 4, 5 | 0.956 (9 rows / 3 jobs) | 0.956 (9 / 3) |
| slow-sync amd64 | 1, 3, 6, 7, 8, 9, 10 | 0.994 (19 / 7) | 1.006 (19 / 7) |
| arm64 | 11, 12, 13 | 0.937 (8 / 3) | 0.938 (8 / 3) |

- The `dm-0` rows (0.916 to 0.932) fall in the fast-sync cell, which they do not pull below 0.90.
- With the write rule as well: if it leaves out the same rows (all of jobs 7 and 10), slow-sync becomes the notes' nvme cell, 1.001 and 1.012 over 15 rows from 5 jobs. I did not recompute the write rule's own medians under the other classing.

**Why it matters.** The exit does not depend on it: under either classing the rule is met. But design §9 now claims the exit for three kinds and names a fourth as not shown. The rule written before the data had no fourth kind, and by that rule nothing is left unclaimed. A reader who compares §9 with "the rule the criterion is stated by" will find two classings and no reason for the change.

**Suggestion.**
- Judge the round by the rule as fixed: fast-sync, slow-sync and arm64, with the table above.
- Keep the work-disk table as a finer view beside it.
- Drop "decided before the round", or point to where it was decided.

If the classing is meant to change from here on, the change belongs in the rule's own paragraph, with its date, as a rule for the next round.

### 2. The replay ran on the two 26.04 images only (suggestion)

- The workflow's default images for the daemon jobs are `ubuntu-26.04` and `ubuntu-24.04`. The round dispatched `ubuntu-26.04` and `ubuntu-26.04-arm`.
- That covers both architectures (goal 8). The verdict line says "on both 26.04 images", so the claim itself is exact.
- But the first line of the notes' M1 section says the exit was "judged by the M1 exit round on Ubuntu 24.04 and 26.04, amd64 and arm64". In this round the benchmark ran on 24.04 images only and the replay on 26.04 only.
- 24.04's ZFS tools are 2.2.2 with the 2.4.1 module (the daemon rig's version line). So its `zdb` and `zpool import`, which the replay's checks use, are not the ones 26.04 has.
- One full dispatch on `ubuntu-24.04` when runners are free would close this. Otherwise the first line could say which run covers which image.

### 3. The replay table reads "129 of 130" flush boundaries (suggestion, for the rig's summary)

- The notes say 130 of 130, and 128 of 128 on arm64. The summary's table has `a flush boundary (sampled) | 130 | 129 | 0`, and `128 | 127` on arm64.
- **It is not a gap.** The "checked" column counts each prefix once, under the first kind it belongs to: its rows sum to 2690, while "in the log" sums to 2764. One flush boundary is also a mark or the whole log.
- A reader who sees 129 of 130 beside "every record prefix" stops there, as I did. The table could say that the kinds overlap and a prefix is counted under the first, or count each kind in full.

### Not findings

- **Job 7.** Its U 0.588 and B 0.842 count under the flush rule alone, and the nvme cells still hold at 0.992 and 1.011. The notes say so.
- **The one counted row below 0.90.** Job 12's B 0.877 sits in a cell whose median is 0.938.
- **The kill test's criterion.** No vdev error and a clean scrub under `kill -9` are shown on all four images. Each image's control, `ctl_kill_without_reissue`, counts 9 or 10 write errors on the vdev as it should, so the pin would see a server that lost requests.

---
_Generated by [Claude Code](https://claude.ai/code)_

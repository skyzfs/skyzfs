# Next look at #153 (M2.6c-2b) at 3d40045

This note judges head **3d4004552e5a0e0a89469c0c91d9e20da669ca7b** of
`skyzfs-coding-m2.6c-2b`. That is one commit on 9717321, touching 9 files
in `crates/skyzfs-agent` (`git diff 9717321 origin/skyzfs-coding-m2.6c-2b`).
I formed a view from the code first, then read the author's answer (issue
comment 7178505979). The review at 65d72ee is `review.md` beside this note.

## Verdict

**Can merge at 3d40045 once the lead has confirmed the 35-minute cap.**

- **D1 is fixed within a process.** A take that is never whole is now taken
  three times, 10 s apart, then given up as `NotWhole`, with a report and
  the last take destroyed.
- **No search found anything wrong.** None of my searches found:
  - a wrong publish or a wrong manifest;
  - a leaked hold, manifest or under-way entry;
  - a take left with no give-up.
- **D2 (low, below):** across restarts the bound on failed checks can be
  lost, so the loop is bounded only by `AGE_MAX`. It needs a stop of the
  agent at one point in every process, and before this PR the loop needed
  no stop at all. It is the lead's call; I do not hold the merge for it.

What blocks the merge: **the cap of 35 minutes waits on the lead.** The
author asked the lead to confirm it. The arithmetic is under (d).

**CI on 3d40045.** Every check is green:

- ci: lint, test, unit-arm64, s3-backends, chart-lint;
- coverage, e2e, k3s-smoke, skyzfs-ci;
- the publish rig and cut-daemon;
- the replay rig's jobs "through skyzfsd" (24.04 and 26.04) and write-into-a-hole;
- the daemon rig, run 45727016749, on the daemon: ubuntu-24.04, ubuntu-26.04
  and ubuntu-26.04-arm.
  - The run ended with exit 0. Each job's table has 55 pins that pass and
    no pin that fails.
  - The `ctl_` lines fail, as their controls should.
  - The pin `pin_failed_volume_while_a_request_is_held` is WAITING, as it
    has been.
  - Two lines read NA, for the log, as the in-place store expects.
  - The rig runs on this PR only because 9717321 touches
    `bins/skyzfsd/src/kill.rs`, a doc-only change. Nothing in this PR
    reaches the daemon.

Codex completed on 3d40045 with no findings.

## Defects

### D2 (low): across restarts, the count of failed checks can be lost, and the loop is then bounded only by AGE_MAX

- **Where.**
  - The count of a failed check is committed in `rename`'s Intent commit
    (`crates/skyzfs-agent/src/engine.rs:1062-1069`, the increment at
    `:1067`).
  - That commit comes after the destroy in `retake` (`:1048-1050`).
  - A take adopted where this process has no reading from before it is
    retaken uncounted (`:945-948`). The reading lives only in the process's
    memory (`:886`).
  - The doc at `:1007-1008` says "it cannot loop, since a process reads the
    pool before each take it asks". That is true within one process only.
- **Scenario 1: the count lost.** The pool has a dataset that
  `zfs snapshot -r` skips, for example an interrupted `zfs receive -s`, so
  every take reads as not whole.
  1. The process takes, adopts, finds the take not whole, and decides on a
     counted retake (`failed` → `retake`).
  2. It destroys the take.
  3. It stops before `rename` commits the count.
  4. The next process plans at Intent with `attempts` unchanged. `pool@t`
     is gone, so it takes again.

  Repeat the stop in every process and `attempts` never reaches 3.
- **Scenario 2: every retake uncounted.** The take is whole, but its members
  are not as read before. Each process stops between its take and the
  take's adoption. The next process adopts with no reading and retakes
  uncounted.
- **Shown in the fake.** Search SL in `scratch153b.rs` stops each process at
  its own k-th point, up to 400 processes.
  - Scenario 1 (stop at `Destroy` after, or at `Intent` before): 239–240
    takes in 241 processes. Each run was given up as `TooOld` at about
    7200 s of the world's clock.
  - Scenario 2: 120 takes in 242 processes, also ended `TooOld`.
- **Against the base.** Before this PR the same take looped with no restarts
  and no pause, also until `AGE_MAX`. So this is not a regression. It is a
  gap in the new bound: the bound holds per process, and the code's doc
  says it cannot loop.
- **Certainty.**
  - That the paths exist: high. Shown, and read in the code.
  - That they happen in practice: low. Every process must stop at the same
    narrow point. A run cut at a fixed time does this only when the time
    always lands inside the same step. At 10 001 datasets one step is about
    32 minutes (under (d)), so a supervisor that kills runs at a fixed
    interval near that could do it.
  - `--limit-s` in #149 cannot do it: it ends the process only between
    rounds, and the take and its adoption are in one round.
- **Options, for the lead.**
  - (a) Commit the count before the destroy, in a commit of its own in
    `failed`. A stop between that commit and the destroy then counts one
    take twice, which errs toward giving up early.
  - (b) Count every take in the Intent commit that already comes before it
    (`take`, `:876`), and clear the count only where a check passes. Takes
    that were never judged would then count too.
  - (c) Accept it, and reword the doc at `:1007-1008` to say that the bound
    holds per process and that across restarts the bound is `AGE_MAX`.

  I would take (a), or (c) if the lead prefers no more change here.

## The points asked

### (a) "A restart's retake is not counted": restarts searched at every point

- **Within one process the bound holds.** SD1 crosses a take that is never
  whole with a stop at every point, a person's action at every point, the
  destroy's three answers and a late take: 4258 runs. No run took more than
  5 times (takes per run: 0 to 3 in 2736 runs, 4 in 1472, 5 in 50). The
  takes beyond 3 are ones the design does not count as failed checks: a
  late take of the same name landing, a person's destroy of the take, a
  rename after `EEXIST`, or a restart before an adoption.
  - **Every SD1 run ended published or given up for a reason it names.**
    - 276 published, none wrong by the strict oracle.
    - `NotWhole`: 3306, of them 30 read from the record (the process
      stopped after the commit).
    - `ManifestLost`: 330, of them 60 read from the record.
    - `Premise`: 256.
  - A take that is not whole is counted whether or not this process holds
    a reading (`:942`).
  - A take that is whole but not as before is counted only with a reading.
- **Across processes, see D2.**
- **A nit from the same reading:**
  1. A process makes the `NotWhole` report at the third failed check of a
     "not as before" take (`:1022-1023`, made before the destroy, as it
     should be).
  2. It stops before the destroy (`:1024`), so the take is still in the
     pool.
  3. The next process has no reading, so it retakes uncounted under a new
     name.

  The report then names a publish that was not given up. Under #149 it is
  printed on standard error before the agent acts, so a person may see
  `not-whole skz-X` and later `published skz-Y` for one publish. This is
  harmless; it could be documented next to the conservative reading.

### (b) `Why::NotWhole` appended at record version 2

- **It agrees with the rule in force.** The module doc on `zfs-topology`
  (`record.rs:35-38`) says: "No agent has run version 2 either; from the
  first run of the program (M2.6c), any change to the record takes a new
  version" (the lead's decision on #144). The program is #149, which has
  not merged. `Stopped` was appended the same way (S11 of #146's review).
  The CONTRIBUTING rule ("a version bump and a cross-version decode test")
  applies from that first run.
- **The bytes, checked.** `record_compat153b.rs.txt` is a scratch test,
  added only to the scratch copy's `record/tests.rs`.
  - It decodes the previous head's `GOLDEN` at this head.
  - The decoded record equals the one built without `NotWhole`.
  - It re-encodes to the same bytes. The test passed.
  - The index of `NotWhole` is 10, after `Stopped`. Plant R1, which inserts
    it before `Stopped`, is killed by `version_2s_bytes_are_these`.
- **Downgrade.** An older agent reading a record that holds `NotWhole`
  loads it `Corrupt`, publishes nothing and reports. That is the record's
  stated behaviour for what it cannot read. No agent has run.
- **The conflict with #149.** #149 appends `Why::Person` at the same index
  and adds `Publish.person`.
  - Whichever merges second must order the variants and regenerate
    `GOLDEN`. The author says so in the answer.
  - The lead should know that the order decides the bytes of v2 for good,
    since #149's first run freezes them.

### (c) Holds, takes and manifests at the pause and the give-up: S1-S3 rerun

- **SD1, 4258 runs, and SL, 34–36 runs per case.** These were judged by
  `judge_d1`, which:
  - reads the give-ups from the record as well as the outcomes;
  - checks that a `NotWhole` give-up has its report and that a `NotWhole`
    give-up kept no take;
  - checks holds, under-way entries and manifests;
  - then runs the strict GUID oracle with the `NotWhole` reports removed.

  None of these searches found:
  - a leaked hold, manifest file, or under-way entry;
  - a `NotWhole` give-up whose take survived;
  - a `NotWhole` give-up with no report.

  Every flag it raised is explained:
  - **136 runs: `ManifestLost` while the world shows the take gone.** All
    of them are destroy mode 2, a destroy that went through but returned an
    error. This is the documented case on `retake`.
  - **16 runs: `TooOld` with the take kept.** The plan gives up at Intent
    with a snapshot and does not destroy it (`plan.rs:458`, from before
    this PR). The take is attributable by its name.
  - **4 runs: "neither published nor given up".** These are an artifact of
    the harness. A person destroyed the first name, the publish took the
    next name, and that name was given up `NotWhole`. The judge looked for
    the first name.
- **S1 (15 162 runs), S2 (4868) and S3 (7680).** The results are the same as
  at 65d72ee: no wrong publish, and every flag explained as in `review.md`.
- **The probes PA, PM and PB.**
  - PA: 32 runs, none wrong.
  - PM: strict oracle 0 wrong.
  - PB (the hot loop of D1): 3 takes and 3 destroys in 20 s of the world's
    clock, where at 65d72ee it made 399 takes at one instant.
- **The pause.** `take` returns `Moved::No` until `retake_after` (`:868-870`),
  so `settle` sleeps a `TICK`. The deadline moves with the publish to the
  next name (`:1074-1078`). The least gap between two takes in a process,
  with no person and no late take, was 10 000 ms in SD1 and SL.

### (d) The limits, against run 45720225770 and the cap

The figures below are from the run's artifacts: Ubuntu 26.04, kernel
7.0.0-1020-azure, ZFS 2.4.1, Lua from 65d72ee. The full read is
`members.lua … full`. The read limit is 60 s + 180 ms × datasets counted
(`zfs/cli.rs:235`, `:290-293`).

| Datasets | Full read | Per dataset | Read limit | Margin |
|---|---|---|---|---|
| 101 | 8.663 s | 85.8 ms | 78.18 s | 9.0× |
| 1 001 | 88.181 s | 88.1 ms | 240.18 s | 2.72× |
| 10 001 | 911.828 s | 91.2 ms | 1860.18 s | 2.04× |

- **The author's arithmetic is right.** 2 × 911.8 = 1823.7 s, and the limit
  is 1860.2 s.
- **The other calls at 10 001 datasets.**
  - The count took 16.4 s, against its fixed 60 s.
  - `tagged.lua` took 55.5 s and the members read 21.5 s, both reads.
  - The take took 46.3 s (snapshot-cli 37.2 s), the destroy 49.0 s and the
    hold 33.8 s, against the change limit of 1120.1 s.
- **The cap (`:236`), 35 min = 2100 s.**
  - The read limit reaches the cap at 11 334 datasets, the change limit at
    19 800.
  - The 2× margin on the full read holds up to about 11.3k datasets, and
    falls after that.
  - At about 91 ms per dataset (it grows slowly: 85.8, 88.1, 91.2), the
    full read passes 2100 s at about 22–23k datasets. Above that no publish
    can be made.
  - With the old cap of 20 minutes the same limit was about 13k, and the
    margin at 10k was 1.3×.

  The figure is the lead's to confirm.
- **Caveats for the lead.**
  - The margin rests on one unloaded run on one runner. There is no
    measurement at scale on 24.04: run 45720134548 had no scale step.
  - **A failed count gives the base limits** (`:588`), so the reads that
    follow have 60 s.
    - The sweep reads `tagged` at a process's first round and after each
      import. At 10k datasets `tagged` (55.5 s) barely fits the 60 s.
      Above about 11k datasets it would not.
    - An error of the sweep fails the whole `settle` call
      (`self.sweep()?`), so that run of the agent ends.
    - This is as the lead ruled: the round's own calls say what is wrong,
      and the next run counts again. The lead should know where it bites.
  - **Time.** At 10k datasets one failed check costs about 32 minutes:
    - the count, 16 s;
    - two full reads, 912 s each;
    - the take, 46 s;
    - the destroy, 49 s.

    Three failed checks therefore take about 97 minutes, inside
    `AGE_MAX` (2 h) with about 20 minutes to spare.
  - A read that hangs now holds a round for up to 31 min at 10k (35 at the
    cap). The 2-hour age bound allows about four such waits.

### (e) The new tests against P3, P5, P7 and the D1 loop

Each plant was applied alone as text to a scratch copy of this head, then
`cargo test -p skyzfs-agent --lib` was run with a 600 s limit, and the copy
was restored in a `finally` (`plants153b.py`, `plants153b.out`). The work
tree was clean afterwards.

- **T1, all three killed by the intended tests:**
  - P3 by `a_dataset_renamed_after_the_take_has_it_taken_again`;
  - P5 by `a_dataset_destroyed_after_the_take_has_it_taken_again`;
  - P7 by `a_late_take_landing_at_any_call_publishes_its_truth`.
- **P2, P6 and P8 survive, as before.** I accept the author's argument for
  T2 (P2, the GUID compare): the name, type, property and count checks
  decide every case the GUID would. P6 and P8 were suggestions in
  `review.md`. They are not repeated as gaps here.
- **D1, all of the author's eleven reproduced as killed** (D1a–D1k). Plants
  killed besides:
  - D1o (the count read from `stalls`);
  - R1 (the variant order), by `version_2s_bytes_are_these`;
  - L1–L7 (the limits: a failed count keeping the last limits or giving the
    cap, a read scaled as a change and the reverse, the cap at 20 min, a
    read at 100 or 179 ms a dataset), by
    `the_limits_grow_with_the_datasets_counted`.
- **Plants that survive the committed tests:**
  - **D1l:** a not-whole take adopted with no reading goes uncounted. No
    test adopts a not-whole take after a restart.
    `a_take_retaken_after_a_restart_is_not_counted_as_failed` has its
    first take whole. My searches did not tell it apart either, since its
    loops look like D2's. Test gap.
  - **D1p:** the report made after the give-up's commit, so that a stop
    between them loses it. SD1 and SL kill it ("NOT REPORTED": 30 runs in
    SD1, 9 in each SL case; `plants153b_search.py`,
    `plants153b_search.out`). Test gap.
  - **D1m, D1n and D1q:** the names in a report. D1m gives no names for a
    take not as before. D1n leaves out the datasets read before and missing
    from the take. D1q names only the first dataset that breaks the take.
    Only one test checks names: `NotWhole(["skzq/vol"])`, one skipped
    dataset. Test gap, minor.
  - **D1r and D1s:** variants of the pause, which cost only time. These are
    not gaps.

## Test gaps (suggestions, not blocking)

- **T6.** A not-whole take found after a restart, with no reading, must be
  counted. This kills D1l, for example: stop before the adoption of a
  take that is never whole, and expect the give-up after three takes.
- **T7.** A stop at every point of the third failed check must still give
  exactly one `NotWhole` report and one give-up. This kills D1p. The
  existing restart harness can be reused.
- **T8.** The names in a report. Cover two skipped datasets, and a take not
  as before both because a dataset was renamed and because one was created
  before the take. This kills D1m, D1n and D1q.
- **T9.** If D2 is fixed by option (a), a test that stops at `Destroy` after
  in every process and expects the give-up within three counted takes.

## Notes and suggestions

- **N6: the lock.** The full read runs as a no-sync channel program.
  - In OpenZFS 2.4.1, `zcp_eval_open` → `dsl_pool_hold` →
    `dsl_pool_config_enter` holds `dp_config_rwlock` as reader for the
    whole program: 911.8 s at 10k.
  - `dsl_sync_task_sync` takes the same lock as writer. So a sync task that
    arrives during the read (a person's `zfs snapshot`, `zfs set`, or
    `zfs destroy`) holds up that txg's sync until the read ends. While the
    writer waits, the rrwlock also blocks new readers.
  - This rests on reading the source; I have not measured it.
  - It is not a regression: at the base the full read was a syncing
    program and stalled the txg for its whole run (843–960 s),
    unconditionally.
  - Suggest that testing measure it: a `zfs set` during a full read at 10k,
    timing the txg and a writer's latency.
- **N7: the size of a report.** `Report::NotWhole` carries every name that
  breaks the take (`zfs.rs:287-313`, `engine.rs:350`). On a pool where a
  whole subtree is skipped that could be thousands of names, printed on
  standard error under #149. Suggestion: cap the names printed (say 20,
  and "and N more").
- **N8: cost.** `as_before` and `differing` (`engine.rs:327`, `:350`) are
  quadratic in the datasets: each member searches the reading. At 10k
  datasets that is about 10^8 compares per adoption, small next to a
  912 s read, but a map by GUID would make it linear. Suggestion only.
- **N9: the doc.** Reword `:1007-1008` whatever is decided on D2.

## What this review did

- **The order of the review.** I read the diff, formed a view, ran the
  searches and plants, and only then read the author's answer.
- **Gates at 3d40045** (`gates153b.out`), with rustc 1.100.0, current
  stable (`rustup update stable` made no change), in a target directory of
  its own with `debug=0`:
  - `cargo fmt --all -- --check`: clean.
  - `cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -- -D warnings`:
    clean. `skyzfs-agent` has no features on this branch.
  - `cargo test -p skyzfs-agent -p skyzfs-control`: 142 and 16 passed.
  - `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` for both crates: clean.
  - The doc change of 9717321 in `bins/skyzfsd/src/kill.rs` is covered by
    CI's lint job, which builds the docs with `skyzfsd/test-hooks`. It is
    green.
- **The searches.**
  - `scratch153b_patch.py` patches a scratch copy's fake.
    `scratch153b.rs` holds the searches: S1-S3 and the probes PA, PB, PM
    of `review.md`, plus SD1, SL and SN (above).
  - The outputs are `search153b.out` (PB, PA, PM, S1-S3) and
    `search153b_d1.out` (SD1, SL and SN, with the judge that reads give-ups
    from the record). Lines are cut at 400 characters.
- **The plants.** `plants153b.py` and `plants153b.out` cover the committed
  tests; `plants153b_search.py` and `plants153b_search.out` cover D1l and
  D1p against the searches.
- **The record.** `record_compat153b.rs.txt` holds the decode of the
  previous head's bytes.
- **The limits.** Read from run 45720225770's artifact (`zfs-cli`
  recordings).

Nothing here changes product code, tests or workflows, and nothing was
posted to GitHub.

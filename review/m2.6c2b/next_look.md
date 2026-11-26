# Next look at #153 (M2.6c-2b): 3d40045, then 0a19e1c

This note judges two heads of `skyzfs-coding-m2.6c-2b`, and says which
head each part was judged at:

- **0a19e1ca6e97710249f6ecb8ab4cc28a4d38b91f** (the section "At 0a19e1c",
  first): two commits on 3d40045 that touch only `zfs/cli.rs`, its tests
  and two new fixtures (`git diff 3d40045 0a19e1c`);
- **3d4004552e5a0e0a89469c0c91d9e20da669ca7b** (the section "At 3d40045",
  after it): one commit on 9717321, 9 files in `crates/skyzfs-agent`. For
  it I formed a view from the code first, then read the author's answer
  (issue comment 7178505979).

The review at 65d72ee is `review.md` beside this note.

## At 0a19e1c

### Verdict at 0a19e1c

**Can merge at 0a19e1c once CI on 0a19e1c is green.**

- The rigs were still running when this was written (below).
- The lead has confirmed the 35-minute cap, so that blocker of 3d40045
  is gone. I take this from 0a19e1c's commit message and the request for
  this look; I have not seen the ruling itself.
- The two commits have no defect.
- The fixtures are the recordings they cite, record for record.
- The new recordings tests read the fixtures through the real parsers.
  Every parser plant they could kill, they kill.
- Every call passes the right kind of limit.
- D2 (low) and the rest of the 3d40045 section still stand: the engine,
  plan, record and `zfs.rs` are unchanged.

Found here, none of it blocking:

- **Test gap T10.** No test pins the limit kind of the members program
  (plain), of `tagged` or of `zpool status`. Plants C2, C3 and C9 give
  each the full read's limit and survive.
- **Nit.** "What a publish costs" leaves out each round's own reads.
- **Nit.** The commit message puts the cap at 10 300 datasets; the cap is
  reached at 10 200.
- **Nit.** The hold test's name still says "root alone".

### (1) The limits at 0a19e1c, against run 45720225770

The code (`zfs/cli.rs:248-255`, `:292-323`):

- **The members program in full** (`Zfs::datasets`): 60 s + 200 ms ×
  datasets counted, at most 35 min (`full_read_limit`).
- **Every other read:** 60 s + 100 ms × datasets, at most 20 min.
- **A change:** 120 s + 100 ms × datasets, at most 20 min.
- **The count:** 60 s, fixed. A failed count gives the bases.

The full read, from the artifact's `scale-N-take-cli-full`:

| Datasets | Full read | Limit | Margin |
|---|---|---|---|
| 101 | 8.663 s | 80.2 s | 9.3× |
| 1 001 | 88.181 s | 260.2 s | 2.95× |
| 10 001 | 911.828 s | 2060.2 s | 2.26× |

- **The cap.** The full read's limit reaches 2100 s at **10 200** datasets
  (60 + 2040).
  - The test's row at 10 300 (`tests.rs:1191`) is past the cap and
    right. The commit message's "reaching its cap at 10 300" is loose.
  - The 2× margin holds, under the cap, up to about 11.5k datasets (at
    91.2 ms a dataset).
  - The full read itself passes 2100 s at about 22–23k datasets. Above
    that no publish can be made: the same ceiling as at 3d40045, since
    the cap is the same.
- **The other calls at 10 001 datasets.** A read's limit there is 1060.1 s
  and a change's 1120.1 s. The read limit is reached at 11 400 datasets,
  the change limit at 10 800.

  | Call | Kind | Took | Margin |
  |---|---|---|---|
  | members (plain) | read | 21.493 s | 49× |
  | `tagged.lua` | read | 55.464 s | 19× |
  | the take (`zfs snapshot -r`) | change | 46.316 s | 24× |
  | the destroy | change | 49.033 s | 23× |
  | the hold | change | 33.809 s | 33× |
  | the release | change | 33.574 s | 33× |
  | the count | its own 60 s | 16.366 s | 3.7× |

  - #152's 391 s for `zfs snapshot -r` under a heavy write load on a slow
    runner fits the change limit 2.9 times.
  - `zfs holds -r` has no record at scale (0.127 s at 3 datasets). It is a
    read under the 100 ms rule.
  - The count passes its 60 s at about 36k datasets, which is already past
    the full read's ceiling.
- **After a failed count** the full read has 60 s, which is too short above
  about 650 datasets (88 s at 1 001). That round cannot read the pool in
  full; the next round counts again. This is as the lead ruled.
- **Every call passes the right kind of limit** (read at the head; plants
  C1–C11 under (2)):
  - `datasets` (`:629-640`) passes `full_read_limit`.
  - `members` (plain, `:645-646`) and `tagged` (`:749-750`) pass
    `read_limit`. They are the other two channel programs; all three are
    read-only (`-n`).
  - `zfs holds -r` (`:429`), `zpool status` (`:479`) and `zpool get`
    (`:560`, `:568`) pass `read_limit`.
  - `zfs snapshot -r` (`:619`), `destroy -r` (`:404`), `hold` (`:654`),
    `release` (`:686`), and the assist's `zfs set` and `zpool sync`
    (`:763-764`) pass `change_limit`.
  - The count (`:607`) passes `limits.count`.
  - `program_output` no longer chooses the limit from `read_only`; each
    caller passes its own.
- **The docs' figures** (`zfs/cli.rs:160-181`) are the artifact's:
  - the full read 8.7, 88.2 and 911.8 s;
  - the plain members program 0.34, 2.2 and 21.5 s;
  - the count 0.23, 1.7 and 16.4 s;
  - `zfs snapshot -r` as the adapter runs it 2.2, 4.4 and 46.3 s
    (`take-cli`).
- **"What a publish costs" (`:178-181`): its sums follow.**
  - At 101 datasets: 2 × 8.663 + 2.159 = 19.5 s, "about 20 s".
  - At 10 001: 2 × 911.8 + 46.3 = 1870 s = 31.2 min, "about 30 minutes".
  - It leaves out what each round reads besides: the count (16.4 s at
    10k), the members program (21.5 s) and `holds -r` (not measured at
    scale). It also leaves out the hold (33.8 s), the release (33.6 s)
    and the sweep's `tagged` once a process (55.5 s).
  - With the four or more rounds of a publish, that is about 4–5 minutes
    more at 10k.
  - **Suggestion:** say "at least", or add the rounds. The design's scale
    is tens to hundreds of datasets, so this is a nit.

### (2) The fixtures and the new recordings tests

- **Which runs** (`gh api`). Both are dispatches of
  `skyzfs-zfs-cli-record.yml` on `skyzfs-testing` at 6d886de:
  - 24.04: run 45720134548, artifact `zfs-cli-ubuntu-24.04`;
  - 26.04: run 45720225770, artifact `zfs-cli-ubuntu-26.04`.

  These are the runs each fixture's `source` names. The zips' SHA-256
  matched the digests the API gives. The later recorder run 45729174446
  (e9cc020, 26.04 only) is not the source of either fixture.
- **The fixtures are the recordings** (`cmpfix153c.py`,
  `fixtures153c.out`).
  - Each holds 16 of the artifact's 169 records: the count, the members
    program in full, `version` and `nocacheflush`, on the rig's pool, the
    mirror's and the pool that does not exist.
  - Each record is equal, field for field, to the record in the
    artifact's own `recorded-<image>.json`, and to the raw files `cmd`,
    `rc`, `stdout` and `stderr` of the record's directory.
  - No record of those kinds is left out.
  - The file's text is byte-identical to the artifact's JSON cut to those
    16 records, in the artifact's own layout (indent 1, keys sorted). The
    one change is the `source` line, which gains "; only the records …
    are kept here".
  - So the file as a whole is not an artifact file, by design, but every
    byte of every record is.
  - The head's `members.lua` is byte-identical to the `members.lua` in
    both artifacts. The Lua has not changed since 65d72ee.
- **The tests read them through the real code.**
  - `the_count_as_recorded`, `the_full_read_as_recorded` and
    `the_full_recordings_are_of_the_versions_the_docs_name`
    (`recordings.rs:838-1031`) give each record to `ZfsCli` on the fake
    host under the record's words, and check that the adapter ran
    exactly those words.
  - They call `count` and `datasets`, which run `program` → `readable` →
    `parse_datasets` → `local_value` / `parse_guid`, and judge the result
    with `taken()`.
  - A record made under `env LC_ALL=C.UTF-8` is given under the words
    without the prefix, since the adapter runs in `C`. The test checks
    that the same bytes come out.
- **Plants, each alone in a scratch copy of 0a19e1c** (600 s limit,
  restored in a `finally`; `plants153c.py`, `plants153c.out`; work tree
  clean after):
  - **The parsers.** The new recordings tests kill F1–F9:
    - F1: a negative GUID read as its absolute value;
    - F2: an `@t`'s group read from the dataset's;
    - F3: hex taken as text;
    - F4: local properties dropped;
    - F5: the count leaving out the root;
    - F6: a failed count read as 0;
    - F7: a volume read as a file system;
    - F8: an `@t`'s GUID read from the dataset's;
    - F9: `no such pool` not read as no pool.

    F10 (the agent's own property kept) and F11 (a snapshot read in part
    taken as none) are killed only by older unit tests. The recordings
    hold neither case, so that is expected.
  - **The fixtures.** Five plants, each rewriting the file in its own
    layout, are all killed by the new tests:
    - X1: a snapshot GUID of the 26.04 rig changed by one;
    - X2: the 24.04 child's `@t` of another group;
    - X3: one byte of the raw value;
    - X4: `skzq/vol` dropped from the 26.04 count;
    - X5: the 24.04 mirror's root given an `@p`.
  - **The limits.** Killed:
    - C1, the full read given a read's limit, by `the_full_read_as_recorded`
      and `the_limits_grow_with_the_datasets_counted`;
    - C4, C5, C6, C7 and C8 (`holds -r` given a change's limit; the full
      read on a change's base, capped as a read, or capped at 36 min; every
      call capped at 20 min), by the limits test, and C5 also by the
      recordings;
    - C10 (the destroy given a read's limit), by `the_destroy_is_zfs_destroy_r`;
    - C11 (`zpool sync` given a read's limit), by
      `the_assist_sets_the_property_then_syncs`.
  - **They survive: C2, C3 and C9.** These give the full read's limit to
    the members program (plain), to `tagged` and to `zpool status`. The
    limits test checks only the take, `holds -r` and the full read. The
    code is right, so this is **test gap T10**. Suggestion: add `members`
    and `tagged` to the limits test's calls, so that the full read's
    limit cannot spread to the other programs.

### (3) The partial-hold test at 0a19e1c

- **The doc of `a_hold_on_the_root_alone_as_recorded`
  (`recordings.rs:791-798`) now says what the records are.** I checked it
  against the recorder (`tests/kernel/zfs-cli-record.sh` at 6d886de,
  lines 505-512) and the 26.04 records.
  - `rig-hold-root`, `rig-hold-r-partial` and `rig-holds-r-partial` are
    made with the root's `@t` alone held. `hold -r` exits 1 with "tag
    already exists", and `holds -r` lists the root alone.
  - `release_quietly` then releases it, with no record.
  - `rig-hold-child`, `rig-release-r-partial` and `rig-holds-r-after-partial`
    are made with the child's `@t` alone held. `release -r` of the pool's
    `@t` exits 0, and nothing is held after.
  - The test uses the root-alone records for the refusal and for
    `Held::Part`, and the child-alone records for the release and
    `Held::No`. That is what the doc now says. The point I raised before
    is answered by the doc.
- **The release of a hold on the root alone has no record**, as the doc
  says. From libzfs (`zfs_release` / `zfs_release_one`), `release -r`
  gathers every snapshot of the name that has the tag. It fails only
  when none has it, so a root-alone release should exit 0 like the
  child-alone one. That is a source reading, not a recording.
- **Nit:** the test's name still says "root alone". It covers both
  halves; a name such as `a_hold_on_some_members_as_recorded` would match
  its doc.

### (4) Gates and CI at 0a19e1c

- **Gates** (`gates153c.out`), with rustc 1.100.0, current stable
  (`rustup update stable` changed nothing), in a target directory of its
  own with `debug=0`:
  - `cargo fmt --all -- --check`: clean.
  - `cargo clippy -p skyzfs-agent -p skyzfs-control --all-targets -- -D warnings`:
    clean. The agent has no features.
  - `cargo test -p skyzfs-agent -p skyzfs-control`: 145 and 16 passed.
    That is 3 more in the agent than at 3d40045: the three new recordings
    tests.
  - `RUSTDOCFLAGS='-D warnings' cargo doc --no-deps` for both crates:
    clean.
- **CI on 0a19e1c**, as of 01:10 UTC: 29 checks passed, 3 were skipped,
  none failed, and 7 were still running. The replay rig's product-layout
  reference job passed at about 01:05.
  - **Passed:** ci (lint, test, unit-arm64, s3-backends, chart-lint),
    e2e, k3s-smoke, skyzfs-ci, the publish rig, cut-daemon, and the
    replay rig's jobs through skyzfsd and write-into-a-hole.
  - **Still running:**
    - coverage;
    - the replay rig's whole-pool reference job;
    - the daemon rig (run 45731580101) on the daemon (24.04, 26.04,
      26.04-arm) and on the stand-in (24.04, 26.04).
  - **Why the stand-in and the reference replay jobs run this time.** The
    workflow's "which server" diffs `BASE...HEAD`. #154, merged into
    `zfs-topology` since, changed `tests/kernel/zfs-cli-record.sh` and
    its test, so the stand-in and the reference jobs run as well. That is
    my reading; this PR changes nothing under `tests/`.
  - Codex: completed on 0a19e1c with no findings.
  - Read the daemon rig's tables, not the colour, when it ends.

## At 3d40045

Everything from here on was judged at **3d40045**. 0a19e1c changes only
`zfs/cli.rs`, its tests and fixtures, so D2, (a), (b), (c), (e), T6–T9
and N6–N9 stand at 0a19e1c as written. (d) is replaced by (1) above;
the cap it waited on is confirmed.

### Verdict at 3d40045

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

### Defects

#### D2 (low): across restarts, the count of failed checks can be lost, and the loop is then bounded only by AGE_MAX

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

### The points asked

#### (a) "A restart's retake is not counted": restarts searched at every point

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

#### (b) `Why::NotWhole` appended at record version 2

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

#### (c) Holds, takes and manifests at the pause and the give-up: S1-S3 rerun

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

#### (d) The limits, against run 45720225770 and the cap (replaced at 0a19e1c by (1) above)

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

#### (e) The new tests against P3, P5, P7 and the D1 loop

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

### Test gaps (suggestions, not blocking)

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

### Notes and suggestions

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

### What this review did

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

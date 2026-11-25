## Review of #144 at b5063b2 (M2.6b2)

The agent's engine for M2.6, in `crates/skyzfs-agent`:

- the engine (`engine.rs`), its fake world (`engine/fake.rs`) and the
  crash matrices (`engine/tests.rs`);
- the planner's changes (`plan.rs`):
  - a look before every give-up;
  - a sync task that writes a group;
  - a bound on stalls (report at 3, give up at 8);
- the record on a file-system trait (`fs.rs`, `fs/model.rs`), with
  power cuts, failed calls and a lock (`record/crash.rs`), and SZAG
  version 2;
- the `zfs` seam (`zfs.rs`);
- one comment in `skyzfs-local` (`inplace/cuts.rs`).

**Commits.** `bece5ed`, `4cb95b3`, `5f766d2`, `7755b73` and `ca8cac7`,
then the merge `b5063b2` of `zfs-topology` at `59c4ad5`.

**The base has moved since.** `zfs-topology` is now at `7c9fa5a` (#143
and its notes). That change touches rigs, bench scripts and notes only,
nothing under `crates/` or `bins/`. The PR merges onto it cleanly.

**How I worked.** I formed the findings from the diff and design §5.2
and §6.2. I then checked them against the description, which I had seen
in passing.

**Verdict: blocked on F1, a fix of two statements plus a test.** The
report `SnapshotGone`, which the lead's D1 asked for, is made at most
once, not at least once.

- Nothing else blocks.
- The ordering of `Action::Stalled` is sound.
- The model of the file system catches every planted fault in the
  write path.
- The lock is sound.
- The gates pass.
- CI is green on everything that has finished. Two runs are still
  going (see CI, below).
- If the lead would rather move F1 to M2.6c, that is decision L1. I
  recommend fixing it here.

### Findings, most severe first

#### Defects

- **F1. The `SnapshotGone` report is made after the record says
  `Done`, so a stop between the two loses it for good.** Blocking.
  Certain; the window is one save.
  - **Where.** `crates/skyzfs-agent/src/engine.rs:659-670`
    (`Action::Done`). `self.commit(Effect::Done, ..)` comes first, and
    `self.reports.push((name, report))` second.
  - **Scenario** (scratch test
    `review_f1_a_snapshot_gone_report_is_lost_to_a_stop_after_the_done_commit`
    in `review144.rs`; it passes at the head):
    1. A busy pool. The daemon dies once its cut has landed, and the
       pool is imported with `-T` below `pool@t`'s group.
    2. With no stop, the agent records `Done` and reports
       `[(t, SnapshotGone)]`. `snap/t` is published, and `pool@t` is
       gone.
    3. With a stop at `Point { effect: Done, after: true }`, the
       first agent has saved `Done` and made no report.
    4. The agent that takes over plans from `Phase::Done`: release,
       then `Forget`. It never reports.
    5. The reports of the two agents together are empty. `snap/t`
       stays published, possibly through a gap in rule (R), and nothing
       says so.
  - **Why it matters.** This is the case D1 wanted a person to hear
    about: a publish that may lack `pool@t`.
    - After `Forget`, the record keeps no trace of it.
    - No sink of M2.6c can make the report at least once with this
      order.
    - The description lists "a report is made at least once" among
      its readings. That holds for `NoProgress` (below), not here.
  - **Fix.** Push the report before the commit. The planner plans
    `Done { report: Some(..) }` again until `Done` is saved, so a stop
    before the save reports again: at least once, possibly twice.
    - I tried this in scratch. The stop then keeps the report, and
      every test of the PR still passes. Only my scratch test, which
      pins the defect, fails.
    - Recording the report in `Phase::Done` would also work, but it
      is a format change.
  - **Test gap.** No test of the PR requires this report (T1), so
    F1's fix needs its own test.

- **F2. The docs of `stalls` say that a daemon that started anew is
  not taken for no progress. A take-over is.** Not blocking. Certain.
  - **Where.** `crates/skyzfs-agent/src/plan.rs:351-358`. `same_place`
    (`plan.rs:371-373`) compares attachment, state and the number of
    cuts.
  - **What happens.** A daemon taken over keeps the attachment and the
    state. It answers `unknown` for the request it lost, with the count
    where it was asked, and that is a stall.
  - **Scenario** (scratch test
    `review_n2_take_overs_with_no_cut_count_as_stalls`; it passes at the
    head):
    - an idle pool, with the sync task as built;
    - a take-over at each `accepted`;
    - result: given up as `NoProgress` after 0 s of fake time, not
      the ~80 minutes the description gives for eight expiries.
  - **Judgement.** The behaviour is defensible: a daemon that crashes
    eight times in a row without a cut is no progress, and giving up
    loses no data. The doc is wrong, though, and the time bound is not
    what the description implies. This is decision L3.

#### Test gaps

- **T1. No engine test requires a `SnapshotGone` report where one is
  due.**
  - `check()` (`engine/tests.rs:272-288`) checks each report that was
    made. It does not check that a due one was made.
  - Plant V22 (the engine drops the report of `Done`) SURVIVED the
    whole suite.
  - Fix: a test like `review_f1` with and without the stop, or a rule
    in `check()`: a published `t` whose snapshot did not survive, done
    in this run, has the report.

- **T2. `ask_anew`'s re-checks (the lead's D5) are untested.**
  - Where: `engine.rs:832-877`.
  - These plants SURVIVED:
    - V15: the count read before the hold and the members are checked;
    - V16: no members check;
    - V17: no hold check.
  - Why they survive: the planner has just checked the same things
    from `Seen`, and nothing in the fake changes the pool between
    `see()` and `ask_anew`.
  - The window is small, but the re-checks are D5's whole point.
  - Fix: a hook in the fake's `members`/`held`/`props` that destroys a
    member's `@t` or releases the hold mid-step.
  - Under V15, the order "verify after the count" is probably
    equivalent for the members, since a recreated `@t` has another
    GUID. That is not obvious for the hold, so pin it.

- **T3. `Disk`, the program's file system, is outside the
  matrices.**
  - These plants SURVIVED:
    - V9: `sync_dir` does nothing;
    - V10: `sync_file` does nothing;
    - V11: `write` does not truncate.
  - I traced the real path by hand under strace (scratch test
    `review144b.rs`). For a new store, two saves, a manifest and its
    removal, and a second open, the sequence is right:
    - `mkdir` then `fsync` of the parent;
    - the lock file opened with `O_CLOEXEC`, then
      `flock(LOCK_EX|LOCK_NB)`;
    - each save: the temporary file opened with `O_TRUNC` and written,
      reopened, `fsync`, `rename`, `fsync` of the directory;
    - `mkdir manifests` with its parent synced; the manifest written
      the same way;
    - `unlink`, then `fsync` of `manifests`;
    - the second open: `EEXIST`, `fsync` of the parent, `flock` gives
      `EAGAIN`, and the store says `Held`.
  - V11 would leave a longer old temporary file's tail behind the new
    bytes. The checksum would then refuse the record, and the agent
    would not start. A unit test that writes short over long and reads
    it back kills V11. V9 and V10 need a kernel (dm-flakey or
    dm-log-writes) and belong to a rig, not here.

#### Suggestions (not blocking)

- **S1. The sync task's error is dropped silently**
  (`engine.rs:645`, `let _ = synced;`).
  - With an error that persists (a permission, a property refused),
    every sync fails, and the request waits to its expiry with nothing
    said.
  - Suggest logging it, or counting it into the `NoProgress` report.

- **S2. `Disk::sync_file` reopens by path** (`fs.rs:69-72`).
  - Since Linux 4.16, an unseen write-back error is reported to a later
    opener. It is not reported if the inode was evicted between the
    close in `write` and the reopen.
  - Syncing on the writing descriptor (a `write_synced` in `Fs`) closes
    that gap and saves an open.
  - Low risk.

- **S3. The model's limits, for the record.**
  - The description states them. None of them hides a defect in the
    present path:
    - **Torn files.** A tear is a prefix only. Any shape is refused by
      the checksum.
    - **Directory sync retried.** A failed directory sync followed by
      a good one keeps the entry in the model. Here that is harmless:
      a step asked again writes and renames again.
  - **X4's equivalence.**
    - It holds under the model.
    - It holds on file systems whose metadata reaches the disk in order
      (ext4 with its journal, XFS).
    - It does not hold where the entries of one directory can persist
      out of order. During the `Taken` save, the record's rename in
      `<store>` and the `mkdir manifests` are both unsynced. Such a
      file system could keep the rename and lose the `mkdir`.
    - Keeping the sync, as the author does, is right. The claim needs
      that condition stated.

- **S4. Notes for M2.6c.**
  - Reports live in memory and are read after `settle` returns. For
    at least once across a real crash, the sink must deliver a report
    when it is pushed. That depends on F1's order.
  - `skyzfs:assist` is a user property set on the pool's root, so it
    enters the root member's `local` in every manifest
    (`manifest.rs:25-27`). Exclude `skyzfs:` properties, or say why it
    is harmless.
  - `zfs set` and `zpool sync` on a suspended pool may never return and
    cannot be killed. The real `Zfs` needs bounds, as `lim` gives the
    rigs.
  - Each sync task adds a line to the pool's history. The history is a
    ring of fixed size, so this only pushes older lines out, but at one
    a second during a stall it pushes many.
  - The repo has no kernel evidence that `zfs set` of a user property
    writes a group on an idle pool. My reading of OpenZFS: `zfs set`
    runs a sync task that dirties the MOS and logs to the history, so
    the group writes an uberblock. M2.T5 is to show it on a kernel, as
    the description says.

### What was asked to be judged

#### (1) The ordering of `Action::Stalled`, the release, crashes, restarts, the sync task, 3 and 8, persistence

Sound.

- **Where a stall is counted.**
  - `stalled` (`plan.rs:459-462`) is true only for a request that was
    asked, ended with the same attachment and state, and has the same
    count.
  - `anew` returns `Stalled` where that end would make the eighth
    stall (`plan.rs:472-473`). It does so after a broken snapshot is
    given up for its own reason, and before `Hold` and `AskAnew`.
- **What `Stalled` saves.** It saves `stalls = 8` and `Phase::Held`
  (`engine.rs:591-603`) and asks nothing.
  - From then on, `publish.stalls >= STALLS_ABANDONED`
    (`plan.rs:480-485`) comes before everything else:
    1. a pending request is withdrawn;
    2. `snap/t` is looked up;
    3. the publish is given up as `NoProgress`, or for its break, with
       the hold released first.
  - The release's own group counts as a cut, but the decision is
    already saved, so it cannot undo it.
- **Crashes and restarts.**
  - A stop before the `Stalled` save plans the same again.
  - A stop after it, or after the release and before `Released` is
    saved, still gives up.
  - At `Held`, `onward` runs with no request asked, so a stall cannot
    be counted twice.
  - V12 (leave the phase at `Asked`) SURVIVED. It is equivalent: both
    phases reach the arm at `plan.rs:480` first.
- **The bound.** Eight requests that stall give the publish up. The
  report comes at the third (`stalls_report`, `plan.rs:378-380`).
  - The plants for an off-by-one were all KILLED:
    - P10 (report from 4);
    - P11 (give up early);
    - V13 (the eighth not counted);
    - V19 (the bound never applied);
    - V21 (`>=` for `==` on the count).
- **What persists.** `stalls` is in the record (SZAG v2, with golden
  bytes in `version_2s_bytes_are_these`). The set of reported names,
  `assisted_ms` and the last sync value live in memory.
- **What resets the stalls.** A count with a cut since, or another
  attachment or state, resets them to 0. The hold's own group counts
  as a cut. Both are conservative: a reset delays the bound, it never
  gives up early.
- **The sync task.** The value is `max(now_ms, last + 1)`. P15 was
  KILLED. The stall tests use `plain_sync`; with the real sync task, a
  stall needs the daemon not to count the group it writes, which is
  the case the bound is for.

#### (2) Is the model of the file system strong enough?

Yes, for the write path as built.

- **What the matrices cover.**
  - A power cut after every call, under six kinds of cut: the bytes
    lost, kept or torn, times the entries lost or kept.
  - Each call failing once, then the step asked again.
  - Together these cover every prefix of operations under ordered
    metadata.
- **My plants on the write path were all KILLED:**
  - V1: rename before the file's sync;
  - V2: the file sync's error ignored;
  - V3: a failed `fsync` retried once, Linux's error lost on the retry;
  - V4: the record directory's sync error ignored;
  - V5: `remove_manifest`'s sync error ignored;
  - V6: `make_dir`'s parent sync error ignored;
  - V7: `write_manifest` without its directory's sync;
  - V8: `write_manifest` without its file's sync.
- **The author's plants R4 to R8, X1 to X3 and M1 to M2 were KILLED.**
- **Limits and the gap of `Disk`:** see S3 and T3.

#### (3) The at-least-once report

- **`NoProgress`: holds in effect.**
  - `ask_anew` saves the count (with `stalls`) before it reports
    (`engine.rs:852-870`). A stop between the two loses that report,
    and the new process asks again from `Held` without reporting.
  - It reports at the next `AskAnew`, with `stalls + 1`. Otherwise:
    - the publish lands, so the report is moot; or
    - the next end is the eighth stall, and the give-up is saved in
      `abandoned`.
  - So a person always gets the report, the publish, or a saved give-up.
  - Reports after a restart differ in `n`, so a receiver should
    de-duplicate by name.
- **`SnapshotGone`: does not hold** (F1).

#### The rest of the brief

- **N1, the sync task.**
  - It is documented (`zfs.rs:270-282`), with `ASSIST_PROPERTY`.
  - The premise refuses read-only, checkpoint, multihost and
    `nocacheflush` pools (`zfs.rs:158`), so `zfs set` is not tried on a
    read-only pool.
  - The error is dropped (S1). Kernel evidence and the history: S4.
- **D1, look before giving up.** Every give-up in `onward` from an
  ended request goes through `look`: `unknown`, `refused rewound`, and
  the arm at `stalls >= 8`. `Intent` and `Taken` were never asked, so
  they need no look. P1 to P4 were KILLED.
  - I also stopped an agent after the snapshot's program and before
    `Taken` was saved. The next agent gives the publish up as
    `ManifestLost` and records it in `abandoned`. Nothing is left
    unrecorded.
- **D5, the order in `ask_anew`.** As decided:
  1. `load_guid`;
  2. members and hold;
  3. the count;
  4. `load_guid` again, which must be the same;
  5. save;
  6. report;
  7. ask.

  P18 was KILLED. The re-checks are untested (T2).
- **The lock (S6).**
  - `flock` on `<dir>/lock` (`fs.rs:96-108`), with `O_CLOEXEC`, so a
    `zfs` child does not inherit it.
  - It is released when the process exits, so no lock can be left
    stale and a reused PID does not matter.
  - A second store in the same process is refused, because `flock` is
    per open file description.
  - `make_dir` and its parent's sync run before the lock
    (`record.rs:474-487`), which is harmless.
  - L1 and L2 were KILLED.
- **Engine robustness.** `commit` keeps the old record in memory when
  the save fails, and the next round plans from it. Replanning is
  idempotent, so this is sound.
- **CONTRIBUTING.**
  - fmt and clippy are clean (below).
  - No `unwrap`, `expect` or `panic!` outside tests. The one
    `unreachable!` is in the model, which is built for tests only
    (`fs/model.rs:194`).
  - Version 2 is a new version, with golden bytes, and refuses another
    version's header by name (`a_head_that_is_not_version_2s_is_named`).
- **The comment in `inplace/cuts.rs` is right.**
  - A checkpoint taken while the agent is down comes after `pool@t`,
    or after the `Intent` of a publish not yet taken.
  - In the first case it holds `pool@t`.
  - In the second, the premise check at `Taken` sees it.

### Gates (toolchain 1.100.0 stable; `rustup update stable` changed nothing)

All exited 0 at `b5063b2`:

- `cargo fmt --all -- --check`;
- `cargo clippy --workspace --all-targets -- -D warnings`, three
  times:
  - plain;
  - with `skyzfsd/test-hooks,skyzfs-local/test-hooks`;
  - with `skyzfs-ublk-sys/control-test-program`;
- `cargo test -p skyzfs-agent`: 61 passed;
- `cargo doc -p skyzfs-agent --no-deps`: no warnings;
- `cargo test -p skyzfs-local`: 323 passed and 1 ignored, plus 11
  passed. It is touched by a comment only.

### Plants (one at a time, each file restored in `finally`, 600 s limit)

Script: `plants144.py`. Output: `plants144.out`. The suite was
`cargo test -p skyzfs-agent --lib -- --skip review`. Every plant
applied; none timed out.

- **The author's 35 (P1-P22, R4-R8, X1-X4, L1-L2, M1-M2): 34 KILLED,
  X4 SURVIVED.**
  - This agrees with the description.
  - The pull request does not carry the edits, so I rebuilt them from
    their names. They may differ in detail from the author's.
  - X4 (the `manifests` directory made without its parent's sync) is
    equivalent under the model and on file systems whose metadata
    reaches the disk in order. It is not equivalent everywhere (S3).
- **Mine (V1-V22): 14 KILLED, 8 SURVIVED.**

| Plant | Result | Reading |
|---|---|---|
| V1-V8 (the write path) | KILLED | see (2) |
| V9 `Disk::sync_dir` does nothing | SURVIVED | T3 |
| V10 `Disk::sync_file` does nothing | SURVIVED | T3 |
| V11 `Disk::write` without truncate | SURVIVED | T3 |
| V12 `Stalled` leaves the phase at `Asked` | SURVIVED | equivalent, see (1) |
| V13 the eighth stall not counted | KILLED | |
| V14 a stall by the count alone | KILLED | |
| V15 count before the D5 checks | SURVIVED | T2 |
| V16 no members check in `ask_anew` | SURVIVED | T2 |
| V17 no hold check in `ask_anew` | SURVIVED | T2 |
| V18 a broken publish given up as `NoProgress` at the eighth | KILLED | |
| V19 the bound never applied | KILLED | |
| V20 `SnapshotGone` where only the premise is refused | KILLED | |
| V21 `>=` for `==` on the count in `stalled` | KILLED | |
| V22 the engine drops the report of `Done` | SURVIVED | T1, F1 |

### CI on `b5063b2` (read at 10:33 UTC)

Ten runs. Eight have finished, all success. Two are still running.

| Run | Workflow | State |
|---|---|---|
| 45591877335 | skyzfs-ci (push) | success |
| 45592601996 | ci (includes `unit-arm64`) | success |
| 45592602050 | e2e | success |
| 45592602028 | k3s-smoke | success |
| 45592601942 | skyzfs-control-rig (24.04, 26.04, 26.04-arm) | success |
| 45592602017 | skyzfs-replay-rig (through `skyzfsd`, both images, in both of its job kinds) | success |
| 45592602247 | skyzfs-publish-rig (26.04) | success |
| 45592602080 | skyzfs-cut-daemon (26.04, 26.04-arm, 26.04-upload) | success |
| 45592601933 | skyzfs-daemon-rig | still running: the three daemon jobs |
| 45592602065 | coverage | still running |

- **The daemon rig.** Its table must be read once it ends. The jobs
  drove the PR merged with `59c4ad5`. The PR touches `skyzfs-local` by
  a comment only, so the daemon is unchanged.
- **Codex** made no review. Its comment on the pull request (10:11)
  says it reached its usage limit. There are no review threads.

### Decisions for the lead

- **L1. Where F1 is fixed.** Recommendation: here. It is two
  statements and a test, and the order is the engine's contract,
  which M2.6c's sink builds on.
- **L2. A publish whose cuts are counted but never uploaded has no
  bound.**
  - Examples: an uplink too slow for the copies' bound, or an uploader
    that is stuck.
  - Each request expires with a cut counted, so no stall is counted.
  - Scratch test `review_d1_a_publish_whose_cuts_never_upload_is_never_bounded`
    (8 hours of fake time):
    - 48 requests;
    - 0 stalls;
    - 2,831 sync tasks;
    - no report and no outcome;
    - the hold kept.
  - After `UNDER_WAY_MAX` (8) such publishes, `begin` returns `Full`,
    and nothing names the publishes that are stuck.
  - This follows the letter of "no cut taken". Options:
    - a separate count of expiries with a cut counted, which reports
      but does not give up;
    - an age bound on a publish that reports.
- **L3. A take-over as a stall (F2).** Options:
  - accept it, since a daemon that crashes eight times with no cut is
    no progress, and fix the doc and the time bound the description
    gives;
  - reset the stalls on a new daemon instance, which needs an
    instance id in `cuts`.

  I recommend accepting it and fixing the doc.
- **L4. The pace of the sync task during a stall** (the description's
  open question).
  - With a qualifying cut, the pace is already once per `ASSIST_AFTER`
    (about 2,880 in 8 hours in L2's run).
  - The 1/s pace applies only while no qualifying cut is counted, that
    is, while the daemon does not count the groups the sync task
    writes.
  - Suggestion: once one sync has run since the request's count and
    the count has not moved, space the next ones by `ASSIST_AFTER`, or
    double the gap up to the expiry. This keeps the first, useful sync
    on an idle pool. It ends a group a second for up to 80 minutes
    where the daemon is not cutting.

### Files here

- `review.md`: this note.
- `plants144.py`: the plants; `plants144.out`: their results.
- `review144.rs`: the scratch scenarios F1, D1 (L2) and N2 (F2),
  appended to `engine/tests.rs` in a scratch worktree only. Each passes
  at the head.
- `review144b.rs`: the scratch trace of `Disk` under strace (T3).

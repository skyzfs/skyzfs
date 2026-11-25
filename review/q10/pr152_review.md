# Review of #152 (M2.6 Q10b): the recorder after #148's third look

Review session, read-only on GitHub. Nothing here was posted.

## Heads

| Part | Judged at | How |
|---|---|---|
| The recorder (`zfs-cli-record.sh`), the writer (`dsync_writer.py`), the probe (`txg_probe.py`), the converter (`zfs_cli_json.py`), the cut rig's `end_row` (`cut-record.sh`) and their tests | f47d864, and again at 889cfd5 | `tests/` is identical at the two heads. Local tests, shellcheck and the plants ran on an archive of 889cfd5. |
| Recorder runs 45703752703 (26.04, scale `100 1000 10000`, no `scale_lua_max`) and 45703760219 (24.04, no scale) | f47d864 | Each artifact's `source` line names f47d864. |
| Cut-daemon run 45703959363 (the upload courses with `end_row`) | f47d864 | |
| The merge of `zfs-topology` (0f69377) | 889cfd5 | remerge-diff |
| The `cargo doc` step (ci.yml, skyzfs-ci.yml) | 889cfd5 | builds of an archive of 889cfd5, rustc 1.100.0 (current stable) |
| CI | 889cfd5 | read-only through `gh api` |

## Verdict at 889cfd5

Can merge once finding 1 is fixed (two lines in each of the two workflows),
or as it is if the lead takes finding 1 as a follow-up. Nothing in the
recorder, the writer, the probe, the converter or `end_row` blocks it.
Findings 2-4 are small and can ride along or follow. Finding 5 corrects
the description: run 45703752703 did not exercise the new path it was
dispatched for.

## Findings, most severe first

### Defects

**1. The `cargo doc` step documents the plain build only** (medium;
certain by plant; 889cfd5).

- **Where:** `.github/workflows/ci.yml:105` and
  `.github/workflows/skyzfs-ci.yml:134`: `cargo doc --workspace --no-deps`
  with `RUSTDOCFLAGS=-D warnings`, once.
- **What it misses:** code behind a feature that is off is not compiled,
  so rustdoc never sees its docs. Off in the plain build:
  - `skyzfsd --features test-hooks`: the modules `cut_commands`, `kill`,
    `record` and `refuse` (`bins/skyzfsd/src/lib.rs`, all
    `cfg(all(target_os = "linux", feature = "test-hooks"))`), the
    test-hooks code in `serve.rs`, `skyzfs-local/test-hooks` and
    `skyzfs-ublk-sys/refuse-fetch` (both turned on by skyzfsd's
    `test-hooks`);
  - `skyzfs-ublk-sys --features control-test-program`: the bin
    `skyzfs-ublk-control` (`required-features`).
- **Shown:**
  - a broken intra-doc link planted in `bins/skyzfsd/src/kill.rs`:
    the plain build exits 0, and `--features skyzfsd/test-hooks`
    exits 101 ("unresolved link");
  - a broken link planted in `src/bin/skyzfs-ublk-control.rs`: the
    plain build exits 0, and
    `-p skyzfs-ublk-sys --features control-test-program` exits 101.
- **Why it matters now:** #150 (open) adds about ten intra-doc links,
  all in `kill.rs`, the test-hooks-only module. The step as written
  would not check one of them.
- **Against what was asked:**
  - #151 asked for `-D warnings` docs "in the three builds above";
  - CLAUDE.md says to run clippy and the tests "with each [feature] as
    well as plain". The lint job already has the three clippy lines.
- **Fix:** two more lines beside the step, in both workflows:
  - `cargo doc --workspace --no-deps --features skyzfsd/test-hooks`;
  - `cargo doc -p skyzfs-ublk-sys --no-deps --features control-test-program`.

  Each passes at 889cfd5 (27 s and 10 s on a warm local target). The
  first also passes with #150 merged onto a5e883a. In CI the plain step
  took 36 s.
- **Not a gap: the architecture.** Testing's claim holds: no documented
  item is under a `target_arch` cfg. The only `target_arch` in the
  workspace is a `cfg!` expression at `crates/skyzfs-local/src/fs.rs:47`.
  No workspace crate runs bindgen: the generated bindings are in a
  dependency, which `--no-deps` leaves out. So amd64 alone is enough
  for the architecture.
  - What amd64 Linux does not document is non-Linux code: the
    re-export `other::Mapped` in `skyzfs-mapped` and `skyzfs-blkcheck`'s
    non-Linux `main`. arm64 would not cover it either, and neither has
    a link today.
  - Also not documented: the dev-dependency feature `testutil` (the
    host-command mock).

**2. The converter drops each record's `note`** (low-medium; certain;
f47d864).

- **Where:** `tests/kernel/zfs_cli_json.py:37-48` builds each record from
  `cmd`, `rc`, `stdout`, `stderr` (and `*_hex`) and never reads the
  record's `note` file.
- **What is lost:** the committed fixtures (bd37422, still on
  `zfs-topology` a5e883a) carry `note` on 8 records per image. Three of
  them say a record did not catch the state its name claims:
  - 26.04: twice "not caught: no output of 100 matched /resilver in
    progress/; this is the last";
  - 24.04: once the same for "scrub in progress".

  Run 45703752703 wrote `note` in 28 records; all of them caught at
  try 1 or 2.
- **Failing scenario:** a later run in which a catch fails. Its fixture
  has a `*-resilver-status` record taken with no resilver, and nothing
  in the fixture says so. `recordings.rs` reads no `note`, so its
  scrub-and-resilver test would assert on the wrong state.
- **Round trip:** converting the records behind the committed fixtures
  gives the same text apart from these notes.
- **Fix:** carry `note` through when the file is there (one line), or
  refuse a record whose note says "not caught".

**3. `stall` calls a window covered when the writer's last partial
second reaches past its own end** (low; certain by probe; f47d864;
Codex P2 at `dsync_writer.py:108`, verified).

- **Where:** `dsync_writer.py:102`
  (`last = max(last or 0.0, sec + 1.0)`) and `:118`.
- **Shown:**
  - a synthetic log (start 90, `sec 100`, end 100.2) against the window
    [100.1, 100.8] reads as covered;
  - a real writer, SIGTERMed 0.2 s into a 0.8 s window, reads as
    covered with exit 0.
- **In the rig:** the writer is stopped 2 s after t1, so this needs a
  stray early SIGTERM. A SIGKILL leaves no end line, and `stopped` then
  shows the kill.
- **Fix:** when the log has an `end` line, it bounds `last`.

**4. `partial` is broader than #147's rule, and `about.txt` says
"destroyed" before the destroy has run** (low; certain by reading;
f47d864).

- **Where:** `zfs-cli-record.sh:354` (`partial`), `:361-371`
  (`cli_snapshot`).
- **What happens:** the destroy of `@t` runs on any `"failed"` in the
  program's output:
  - a failure in the check phase, which took nothing and gives no
    `taken`;
  - EEXIST, where the `@t` is not the program's.

  #147's rule destroys only when `taken` > 0 and errno ≠ EEXIST. The
  author's test "partial, no count" pins the broader rule. The line
  "…destroyed, and zfs snapshot -r takes @t" is written before
  `rec "$name-destroy-partial"`, and that record's rc is not checked.
- **Reach:** none in the recorder's fresh pools (no `@t` before the
  take). It matters only if the rule is copied into the agent. It
  should not be, since the agent's rule is #147's.
- **The other way round:** a program ended by its instruction or memory
  limit after some takes prints its error on stderr and no `"failed"`
  on stdout. It keeps the old rule. If the completed takes stay (read
  from the code, not tried), the root has `@t`, so there is no CLI
  take, and the records after it run on the partial set. The members
  line added here now shows the count, so it is visible. This is not
  reached at the counts the workflow offers: the 10000 program
  finished within `-t 100000000`.
- **Fix (optional):** say "destroying" before the record, or write the
  line after it with its rc. Key the destroy on `taken` > 0 if the
  author wants #147's rule.

### Not a defect, but the description is wrong

**5. Run 45703752703 did not take the partial path** (certain; f47d864).

- **What the description says:** the run was dispatched "so that the
  program's partial take at 10000 goes through the new path".
- **What the run did:**
  - `scale-10000-snapshot` returned all 10001 members, rc 0, in 960 s;
  - `about.txt` has `scale: scale-10000-members: 10001 members`;
  - there is no `scale-10000-snapshot-cli-destroy-partial` record.
- **The earlier run:** 45681972805 (77151b2) stopped at ENOSPC after
  4563 snapshots. The scale pool is 16 GiB at both heads. Why the
  earlier run ran out of space is not established here.
- **So:** the new path rests on the unit tests alone. Plants A1-A8, A11,
  A12 and A14 are killed by them. A13, the same path in `record_pool`,
  is not pinned (below).

### Suggestions

- **`end_row`** (`cut-record.sh:1054-1067`): a daemon that never comes
  to rest gives ten disagreeing reads and `rig_error` (exit 2), not a
  verdict. That is defensible, since the courses' verdicts need a
  settled row, but a summary line that names it would help.
  `end-drain.txt` is written and not read (`drain.txt` is).
- **`cli-limit`:** root is not held to `snapshot_limit`. Both images
  record rc 0, and `@q` is taken on the child under `snapshot_limit=0`.
  So no refusal of `zfs snapshot -r` other than EEXIST and a missing
  pool is recorded. The description says so; M2.6c-2b should not
  expect a limit refusal from root.
- **`none-snapshot-cli` exits 2**, not 1, with "cannot open 'skznone':
  dataset does not exist" and the usage text. The agent's mapping to
  `NoPool` in M2.6c-2b must accept rc 2.
- **The description's "belongs to #147's branch"** is out of date: #147
  has merged (f927674).

## The four questions

1. **NOT COVERED** (`dsync_writer.py` `stall`, `txg_probe.py`
   `report`):
   - Every R3 probe that was a false pass or a false fail at 7a813d9 now
     reads NOT COVERED, and `stopped` adds a nonzero exit status (table
     below).
   - One false pass is left: finding 3.
   - In run 45703752703 no line reads NOT COVERED. Every take's six
     lines are covered, `settle_rc=0`, and `sync_failed=0`, `errors=0`.
2. **`end_row`:**
   - **A torn read cannot pass.** Node B's three reads come from one
     SREF object per volume, and MinIO is strongly consistent, so reads
     are monotone. The head line carries its generation, which rises on
     every CAS. Two equal reads in a row therefore bracket a constant
     head. The row changes only with the head: the daemon writes it
     with each head swap, and nothing else writes at the end.
     Excluding the lease's expiry is right, since `read_row` uses only
     its holder and epoch.
   - **The drain has a limit:** 300 tries of `status` (each under a
     30 s timeout) a second apart. It never fails.
   - **Ten disagreeing reads give `rig_error`**, exit 2.
   - **Cut-daemon run 45703959363 (f47d864):** all five upload courses
     drained in 0-1 s, agreed at the second read and PASS.
3. **`cli-limit` and `-r`:**
   - `cli-limit` records what it says: on both images the take is not
     refused (rc 0).
   - Every take in both indexes has `-r` except the two
     `*-exists-child-pre`, which exist to make a child's `@p` first.
   - Plants B6-B8 (no `-r`) are killed.
   - B1 (another `--expect`), B4 and B5 (the cleanup) survive.
4. **R1 and the converter:**
   - **The destroy touches `@t` only, `-r`, on the take's own pool.**
     It is broader than #147's rule (finding 4) but unreachable here.
     A program killed by `lim` (rc 124) still finishes in syncing
     context. It prints nothing, so it keeps the old rule, which is
     right then (no txg race).
   - **The converter round-trips** #147's fixtures except `note`
     (finding 2).
   - **On the runs:** `recorded-ubuntu-26.04.json` (run 45703752703)
     and `recorded-ubuntu-24.04.json` (run 45703760219) each equal a
     local conversion of the same records (160 records each, no
     difference).

## What a refresh of the agent's fixture needs

This is not #152's work, but the description calls the refresh "a copy
from a named run".

`recorded-ubuntu-26.04.json` from run 45703752703 was copied over the
fixture on a5e883a, and `cargo test -p skyzfs-agent --lib
zfs::cli::recordings` was run. 7 of 10 tests fail:

- `TAG` in `recordings.rs` is `"skyzfs"`; the recorder now uses
  `skyzfs-publish` (holds, `none-tagged`);
- the pool, vdev and snapshot GUIDs written into `guids()` and the
  members' expectations;
- `the_recorded_failure_of_the_snapshot_program_is_an_error` expects
  the old program's `pcall` failure, and the program now succeeds.

The test's own doc comment says that last one "waits for the next
recording". So the refresh is M2.6c-2b's rewrite of those tests, not a
copy.

## R3 probes at the head (f47d864 code)

| Case | Line now | Exit / `stopped` |
|---|---|---|
| writer SIGKILLed before the window | NOT COVERED | the kill added |
| writer whose writes fail at once | NOT COVERED | 1 added |
| writer whose writes fail inside the window (EFBIG, `ulimit -f`) | NOT COVERED | `exit=1` added |
| writer started 1.5 s into the window | NOT COVERED | |
| healthy writer | covered | 0 |
| probe with no kstat | NOT COVERED; "last read of the kstat none"; `sync_failed=2 sync_longest=- errors=7` | |
| probe with the kstat's header only | NOT COVERED; `errors=0` | |
| writer SIGTERMed 0.2 s into a 0.8 s window (Codex) | covered (false pass, finding 3) | 0 |

## Plants at 889cfd5

Plants were made one at a time on an archive of 889cfd5 and restored
after each, with a 600 s limit per test. The harness is
`plants_pr152.py`; its outputs are `plants-pr152-889cfd5.out` (A-D) and
`plants-pr152-889cfd5-b.out` (D-G). Every base test passes, including
the cut test once the workflows are in the archive.

69 plants: 47 killed, 22 survived.

**Killed (47):** A1-A8, A11, A12, A14, B2, B3, B6-B9, C3, C4, C7, C10,
C11, D1-D6, E1, E2, E4-E10, F1-F3, F7, F8, G1, G2, G8-G10. A2 (#147's
rule in `partial`) is killed by the author's "partial, no count".

**Survived (22):**

| Plant | What | Read |
|---|---|---|
| A9 | the list in `cli_snapshot` without `lim` | gap: not pinned |
| A10 | the count from the last match | equivalent (one match) |
| A13 | `record_pool` back on the old rule | gap: R1 in `record_pool` not pinned (A14 pins `scale`) |
| B1 | another `--expect` for cli-exists-child | gap |
| B4, B5 | cli-limit's cleanup dropped | gap |
| C1, C2 | R4: the writer's or probe's stdout back on the script's | gap |
| C5, C6 | the settle's `lim`, its rc | gap |
| C8, C9 | the stall and probe lines on the window t0 t0 | gap: the fake take is instantaneous, so W1/T2 are still unpinned |
| D7 | `started >= t0` | boundary, equivalent in practice |
| E3 | a failed sync read as a time | gap: the test's failed syncs are shorter than the good one |
| F4-F6 | the fixture's sorting, escapes, indent | formatting; a diff-stability gap |
| G3-G5 | `end_row`: the lease, all but the head, the generation not compared | gap: the test's torn read differs only in the head line |
| G6, G7 | `end_row`: three reads, no sleep | gap |

## Local runs (889cfd5)

- `test_zfs_cli_record.sh`: 194 ok.
- `test_dsync_writer.py`: 14 ok.
- `test_txg_probe.py`: 20 ok.
- `test_zfs_cli_json.py`: 6 ok.
- `test_cut_record.sh`: 108 ok.
- shellcheck 0.9.0: nothing new. The warnings are on lines that predate
  the PR (SC2317, SC2218, SC2015, SC2002). CI's shellcheck step passed.

**`cargo doc` with `RUSTDOCFLAGS=-D warnings`, rustc 1.100.0:**

| Build | rc |
|---|---|
| plain | 0 |
| `--features skyzfsd/test-hooks,skyzfs-local/test-hooks` | 0 |
| `--features skyzfsd/test-hooks` | 0 |
| `-p skyzfs-ublk-sys --features control-test-program` | 0 |
| #150 merged onto a5e883a, plain | 0 |
| #150 merged onto a5e883a, test-hooks | 0 |
| planted broken link in `kill.rs`: plain / test-hooks | 0 / 101 |
| planted broken link in the control program: plain / control-test-program | 0 / 101 |

**The merge 0f69377** is clean: its remerge-diff is empty, and the pull
request's diff against a5e883a is the same 13 test and workflow files.

## CI at 889cfd5

| Workflow | Result |
|---|---|
| ci | lint success, with `cargo doc` passing in 36 s after the three clippy lines; unit-arm64, s3-backends and chart-lint success; `test` still running when read |
| skyzfs-ci (push) | gate success; lint, unit-arm64 and test skipped (pull request open), as designed |
| coverage | success |
| nodeb-rig | success |
| publish-rig | success |
| cut-daemon | success |
| cut-rig, daemon-rig, e2e | running when read |
| replay-rig, kernel-probe | queued when read |

Codex has one finding (P2, `dsync_writer.py:108`; finding 3), with no
answer yet.

## Recorder run 45703752703 (26.04, f47d864): figures for the lead

Each take ran beside an O_DSYNC writer, a writer of new data (twice
`zfs_dirty_data_max`) and the txg probe. "Hold" is the probe's
`longest_sync`.

| Count | Program: secs | Program: hold | Program: O_DSYNC longest | CLI `-r`: secs | CLI `-r`: hold | CLI `-r`: O_DSYNC longest |
|---|---|---|---|---|---|---|
| 100 | 12.5 | 11.6 | 0.9 | 0.9 | 1.3 | 1.3 |
| 1000 | 96.8 | 95.2 | 96.6 | 5.2 | 1.3 | 1.3 |
| 10000 | 960.2 | 951.6 | 951.5 | 391.3 | 12.3 | 15.1 |

- The program holds one sync for its whole run, about 0.095 s per
  dataset. From 1000 up, the writers stall for all of it.
- `zfs snapshot -r` spends its time outside syncing context: 391 s in
  all at 10000, with a 12.3 s hold.
- Every take's lines are covered, every `settle_rc` is 0 and every
  `before_sync_longest` has a value except at the 1000 program (`-`,
  one group before), so the baseline concern raised in the third look
  does not show.
- The failure records:
  - `cli-exists-child` and `cli-exists-root`: rc 1, "dataset already
    exists" and "no snapshots were created";
  - `cli-exists-child-after`: no `@p` on the root;
  - `cli-limit`: rc 0;
  - `none-snapshot-cli`: rc 2, with the usage text.

## Recorder run 45703760219 (24.04, f47d864)

Success. It recorded `zfs-2.2.2` userland over a `zfs-kmod-2.4.1`
module, as the committed fixture's versions do, with 160 records and no
scale.

- **The failure records match 26.04:**
  - `cli-exists-child` and `cli-exists-root`: rc 1, "dataset already
    exists" and "no snapshots were created";
  - `cli-limit`: rc 0, with `@q` on the child under `snapshot_limit=0`;
  - `none-snapshot-cli`: rc 2, with the usage text.
- **Takes:** every take has `-r` except the two `*-exists-child-pre`.
- **Notes:** all 28 notes are "seen" or "caught at try 1 or 2".
- **Exit codes:** across the 160 records, the two images differ only at
  `none-get` (2 on 24.04, 1 on 26.04). That predates this pull request.
- **The fixture:** `recorded-ubuntu-24.04.json` equals a local
  conversion of the same records.

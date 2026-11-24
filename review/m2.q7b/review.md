## Review of #136 at f2fcc02 (Q7b: the answers to the non-blocking findings of #133's review)

**Verdict: can merge. Nothing blocks.**

The only change that ships is the help text; the rest is tests and comments. Each answer does what my review of #133 asked:

- **s1b (N1)** holds the purpose of F-A's sync. It fails with FA, and also with a plant that nothing else in `skyzfs-local` catches. It passed 500 of 500 runs, idle and beside busy loops.
- **The new connect test (N3)** catches the uncapped pause with more than 300 ms to spare. It never came near its bound in 90 timed runs under load: the worst was 27.8 ms against 200 ms.
- **The real-file test** catches the join removed in 150 of 150 runs and passed 250 of 250.
- **The help and its test (N5, Q9)** kill Q9.
- **The uploader's fixture (N2)** runs the default, and all 167 of its tests pass.

What I have is non-blocking:

- the description's plant table says more than the plants show, and its CI line is stale (D1);
- the connect test holds an uncapped pause but not a cap of a few hundred ms (T1);
- two notes on what the simulator and the real-file test do and do not show (T2, T3).

The head did not move while I worked: `head.sha` f2fcc02, base 1699116. I read the diff against 1699116 and my review of #133 before the description.

### Blocking findings

None.

### What I was asked to judge

#### 1. s1b's single poll as a stand-in for the wider simulator fix

The test is `a_release_on_a_failed_store_is_named_by_the_acknowledging_start_after_a_power_cut` (`crates/skyzfs-local/src/inplace/tests/cuts.rs:5386`). It is my s1b, with two more checks: one sync of the map was begun (`cuts.rs:5395`), and the start took the map up. The poll is `settle_map`, a single `poll` that must hand out nothing.

- **It fails with FA, but at the count, not the outcome.** With FA planted (no sync begun on a failed store), s1b fails at `cuts.rs:5395` with 0 syncs against 1, before the drop and the power cut. The older F-A test fails at the same kind of assertion (`cuts.rs:5364`). So FA alone does not show that the end-to-end check works (D1).
- **The end-to-end check works.** Two plants of mine pass the count and fail the check that the start names cut 1 (`cuts.rs:5408`):
  - **FAc**: a release on a failed store clears the bits, writes the header and begins its sync, but does not name the cut. Across the whole `skyzfs-local` library, only s1b fails. That is a real loss of what F-A keeps the sync for, and no test in `skyzfs-local` held it before this pull request.
  - **FAp**: a poll of a failed store does not collect the end of the map's sync. s1b fails. See the next point.
- **The poll is honest.** The test's comment and the description both say why it is there. It moves the drop to after the sync's end, which is the order the daemon gives when the `fdatasync` ends first (the helper wakes the pump).
  - The order the simulator cannot show is the drop before the sync ends. There it loses the header, where the real helper keeps it.
  - That error is on the safe side. A test can fail where a real disk would keep the header, but no defect that loses the header on a real disk can pass because of it.
  - The opposite order is held by two links:
    - the store's drop drops the map's file after the marker and the lock (my #133 review, item 4);
    - the file's drop joins its helper. The new real-file test pins that, and my scenario (a) below holds it for the store's own sequence.
- **What the poll costs.** FAp is the one place where s1b's verdict rests on the simulator's model rather than the product. On a real disk the join would still make that header durable. FAp is not harmless, though: a second release on that failed store would set `header_due`, and no sync would ever begin for it. I would leave it caught (T2).
- **Deterministic.** 200 of 200 runs passed idle, 200 of 200 beside 8 busy loops and 100 of 100 beside 16, on 4 CPUs (`timing.out`).
  - The sync's end is bookkeeping on the caller's thread. With a sync delay of 0 (`created(205, 0)`), the first look ends it, and no random draw is made.
  - The store's 8 I/O threads serve only the cut's write and flush, which complete before the release.
- **The wider fix** (the simulated file's drop completes a pending sync that is not held) is not made, and the description says so. With s1b and the real-file test both in, I do not think it is needed now.

#### 2. The 200 ms bound of `a_connect_is_made_soon_after_the_backlog_has_room`

The test is at `bins/skyzfsd/src/control/tests.rs:462`, with its bound at line 479. To time it, I ran the test's own steps 30 times in a loop that prints how late each connect came after the room was made, and at which try (`scenario_room.rs`; `room_*.out`). Each plant got its own binary.

| Build | Load | Runs | Late after the room (ms) | Try that connected |
|---|---|---|---|---|
| head | idle | 30 | 15.7 to 17.8 | 36 |
| head | 8 busy loops | 30 | 5.5 to 19.9 (median 10.3) | 36 |
| head | 16 busy loops (load average 15.5) | 30 | 2.2 to 27.8 (median 11.9) | 35 to 36 |
| CE, uncapped | idle, 8 and 16 loops | 30 | 535.9 to 572.1 | 12 |
| CE8, capped at 400 ms | idle | 10 | 211.8 to 212.5 | 13 |
| CE5, capped at 500 ms | idle | 10 | 11.7 to 12.9 | 12 |
| CF, first pause 50 ms | idle | 10 | 2.3 to 4.2 | 31 |

- **The test itself:**
  - Alone, it passed 20 of 20 beside 8 busy loops.
  - The whole `control::tests::socket::` module passed 10 of 10 idle, 20 of 20 beside 8 loops and 20 of 20 beside 16.
  - The whole `skyzfsd` library passed 3 of 3 beside 8 loops and 3 of 3 beside 16.
  - It passed in CI on amd64 and arm64.
- **The bound is right; keep 200 ms.** The head's worst case under 16 busy loops on 4 CPUs was 27.8 ms, so the bound has about 7 times that in hand. CE is caught by at least 336 ms in every run.
- **What it does not hold (T1).** A pause that grows to a cap between about 200 ms and 1 s is caught or not depending on where the tries fall against the room at 1.5 s:
  - a cap of 500 ms has a try at 1511 ms, so it survives;
  - a cap of 400 ms is caught by 12 ms only.
- **Suggestion:** also assert `tries >= 20` once connected.
  - The head connects at try 35 or 36 under every load I tried. CE, CE5 and CE8 connect at 12 or 13.
  - The bound catches any cap above about 125 ms, wherever the room falls.
  - CF (31 tries) still survives, which is harmless.
  - This is optional: the defect I raised in N3 was the uncapped pause, and that is caught.

#### 3. N2, N5 and Q9

- **N2.** The fixture builds the store with `intent_header_sync: true` (`crates/skyzfs-upload/tests/common/mod.rs:155`). It is fixed, not a choice or a second run. All 167 of the uploader's tests pass at the head.
  - The uploader's tests kill the daemon or reboot the host, but never cut the power, so the flag changes no outcome they assert. What the flip buys is that they now drive the default path: a header sync begun on each release, and writes into cleared regions waiting behind it.
  - The setting with the sync off, for an uploading volume, is reached only with `--hold-cuts --no-intent-header-sync`. No uploader test runs it now. The store's own tests run both settings. This is not a defect.
- **N5.** The help (`bins/skyzfsd/src/cli.rs:272-275`) now says that a held cut is let go once it is uploaded, or by the test hooks. It says that `--hold-cuts` is off unless given, and that a volume that uploads holds its cuts without it. All of this matches `hold_cuts_of` and the upload's paragraph below it. The line that was too long is reflowed.
- **Q9.** The new loop (`bins/skyzfsd/src/cli/tests.rs:455-462`) removes `--no-intent-header-sync` and asserts that `--intent-header-sync` is named nowhere else. My Q9 is now killed: "the help names the option that is gone".
  - It covers `USAGE` and `HELP`.
  - The module doc of `cli.rs` (line 8) is outside its reach, and that is fine: my Q9u survives there, and it is only a comment.
- **N4(b).** The status row at offset 976 (`crates/skyzfs-ublk/src/status.rs:162`) now says that the field reads 0 where no cuts are held. That matches `intent_shown` (`bins/skyzfsd/src/serve.rs:605-610`). #133's own description was corrected for N4(a) and N4(b) before it merged.
- **The nit** (`crates/skynbd-refs/src/lib.rs:283-286`) is reflowed.

#### 4. The real-file test that a file's drop waits for its helper's sync

The test is `the_drop_of_a_file_waits_for_the_sync_its_helper_is_doing` (`crates/skyzfs-local/src/fs.rs:1230`).

- **It fails when the join is removed.** With J planted (`SyncHelper::drop` drops the thread's handle instead of joining it, `fs.rs:412`), it failed in 50 of 50 runs idle, 50 of 50 beside 8 busy loops and 50 of 50 beside 16, with "the file's drop returned before its helper's sync ended".
- **It is deterministic at the head.** It passed 100 of 100 idle, 100 of 100 beside 8 loops and 50 of 50 beside 16.
  - The join is the point both threads meet, so load can only make it slower.
  - With J planted, a pass would need the test's thread to stall 200 ms between the drop and its check.
- **What it holds exactly (T3).** The drop comes after the helper's `fdatasync` has returned, while the wake runs, so the test holds the join through the job under way. The wake's 200 ms stands in for a slow sync, as its comment says.
  - The store's own sequence is different: a sync is begun and the store dropped at once, while the job may still be queued.
  - The same join covers that case, because `std::sync::mpsc` hands a queued job to the helper before it reports the channel closed.
  - My scenario (a) begins a sync and drops the file at once; scenario (b) queues a second sync behind a slow one (`scenario_fs.rs`). Both passed 200 of 200, idle and beside 8 loops. With J planted, (a) failed 40 of 40 (`fs_scenario.out`).
  - So this is a note, not a gap. Case (a) could be added as a second case, at the author's choice.
- **My J2** (the helper's results let go before the join) survives this test. That is expected: the job under way has already sent its result when the drop comes. J2 is not a defect of the sync either, because the `fdatasync` is still made.

### Non-blocking

**D1. The description's plant table and CI line (stale or imprecise; sure).**

- **The FA row** says "caught: the s1b test finds no committed cut". With FA, s1b fails at its count of syncs (`cuts.rs:5395`, 0 against 1) and never reaches the power cut. The committed-cut check (`cuts.rs:5408`) is reached by FAc and FAp, not by FA.
- **The CE row** quotes "connected 549ms after the room was made". The test prints "connected 547.934163ms after the backlog had room, at try 12".
- **"CI: to be added when the runs on f2fcc02 end"** is stale: every run on f2fcc02 has ended green (below).

**T1. The connect test does not hold a cap of a few hundred ms (test gap; sure; low).** See item 2: CE5 survives and CE8 is caught by 12 ms. The suggested `tries >= 20` closes it. Optional.

**T2. s1b's verdict on FAp rests on the simulator (note; sure).**

- s1b fails a change whose first header would still be durable on a real disk. This is the conservative direction, and the test's comment explains it.
- If someone changes how a failed store's poll collects syncs on purpose, s1b will need the wider simulator fix rather than a weaker assertion. No action now.

**T3. The real-file test holds the join through a job under way, not a job still queued (note; sure; very low).** See item 4. The same join covers both cases, and scenario (a) shows it.

### Plants (`plants.py`, `plants.out`)

Each plant was put in alone, in a clean worktree of f2fcc02, and taken out again; the worktree was clean after the run. The panic's place is kept, to show which assertion caught each plant.

| Plant | What | Result |
|---|---|---|
| FA (author) | a release on a failed store begins no header sync | KILLED: s1b at its count (`cuts.rs:5395`), and `a_release_on_a_failed_store_begins_a_sync_the_drop_does_not_wait_for` (`cuts.rs:5364`) |
| J (author) | the drop of a file does not join its helper | KILLED: `the_drop_of_a_file_waits_for_the_sync_its_helper_is_doing`; 150 of 150 runs (item 4) |
| CE (author) | the pause keeps doubling past 50 ms | KILLED: "connected 547.934163ms after the backlog had room, at try 12" |
| Q9 (author, mine) | the help's paragraph names `--intent-header-sync` | KILLED: "the help names the option that is gone" (`cli/tests.rs:459`) |
| CF (author) | the pause starts at 50 ms | SURVIVED, as the test's comment says; costs 50 ms at most |
| FAc | a release on a failed store does not name the cut; the bits are cleared and the header is written and synced as before | KILLED by s1b only (whole `skyzfs-local` library), at `cuts.rs:5408` |
| FAp | a poll of a failed store does not collect the end of the map's sync | KILLED by s1b only (whole library), at `cuts.rs:5408` (T2) |
| J2 | the helper's results are let go before the join | SURVIVED: expected, see item 4 |
| CE5 | the pause is capped at 500 ms | SURVIVED (T1): a try comes 12 ms after the room |
| CE8 | the pause is capped at 400 ms | KILLED by 12 ms: 211.8 ms |
| Q9u | the module doc of `cli.rs` names `--intent-header-sync` | SURVIVED: a comment, outside what the test reads |

### Codex

Code review and security review are both completed on f2fcc02 with no findings. There are no review threads and no comments of Codex's to answer.

### CI on f2fcc02

Every run is green. The test job's log shows the four new or changed tests ran and passed, on amd64 and on unit-arm64.

| Run | Id |
|---|---|
| ci (changes, lint, chart-lint, s3-backends, test, unit-arm64) | 45506628479 |
| skyzfs-ci (gate, lint, test, unit-arm64) | 45506559356 |
| coverage | 45506628690 |
| e2e | 45506628588 |
| k3s-smoke | 45506628433 |
| the daemon rig (24.04, 26.04, 26.04-arm on the daemon) | 45506628396 |
| the replay rig | 45506628413 |
| the cut rig | 45506628482 |
| node B | 45506628407 |
| the control rig | 45506628627 |

I did not read the rigs' tables. Nothing they drive changed, apart from the help text.

### Gates run locally (`gates.sh`, `gates.out`)

After `rustup update stable` (rustc 1.100.0, clippy 0.1.100):

- **fmt:** clean.
- **clippy `-D warnings`:** clean on the workspace, on `skyzfsd --features test-hooks` and on `skyzfs-ublk-sys --features control-test-program`.
- **Tests at the head:**
  - **Plain** (`t_plain.out`): 844 passed, 0 failed, 1 ignored. That is `skyzfs-local` 329, `skyzfs-ublk` 162, `skyzfs-upload` 167, `skyzfsd` 145 and `skynbd-refs` 41.
  - **`skyzfsd --features test-hooks`:** 184 passed (`t_hooks.out`).
  - **`skyzfs-local --features test-hooks`:** 329 passed, 1 ignored (`t_local_hooks.out`).
  - **`skyzfs-ublk-sys --features control-test-program`:** 37 passed; the 11 ignored are the kernel tests (`t_ublksys.out`).

### Files here

- `review.md`: this note.
- `plants.py`, `plants.out`.
- `gates.sh`, `gates.out`, `t_plain.out`, `t_hooks.out`, `t_local_hooks.out`, `t_ublksys.out`.
- `bins.sh`: builds the timing runs' binaries, at the head and with plants.
- `timing.sh`, `timing.out`.
- `scenario_room.rs`, `room_*.out`.
- `scenario_fs.rs`, `fs_scenario.out`.
- `fail_fsJ_sample.out`: one of J's failures.

The scenarios are not for merging: each was inserted for a build and taken out again. Paths in the outputs are shortened to `<scratch>`.

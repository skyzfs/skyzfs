Next look at #119, head cfe8146 (M2.T3 stage 3: the cut rig on the daemon's uploads)

**Verdict: it can merge. No finding blocks it.** D1, D2, D3, D4, G2, G4 and G5 are closed in the code, and the condition of the first review is met: courses (a) to (d) have run on a kernel at this head, on the merged #118, in the pull request's own upload job, and every line passes. One new finding (F1) is a small window in which the rig can still fail a right daemon in `umoved` and `ulost`; I have not seen it happen and the fix is one line, so I would not hold the merge for it. The rest are gaps and suggestions.

CI on cfe8146 when I looked (11:57 UTC): 34 jobs green, none failed, 10 kernel jobs still running (the cut rig of M2.T1 on four images, the replay rig's product layout, the daemon rig's five). The cancelled and skipped jobs are the push's run, which the pull request's run replaced. Codex reviewed cfe8146 and has no finding; its one finding at 32d8d67 (P2, cut_watch.py:140, my D2) is answered on its thread and fixed. "Can merge" assumes the ten running jobs end green.

Not said elsewhere: the description still gives the range as `be306ed..6b1be16` and does not mention the plan job (cfe8146).

## What I did

I read the diff `32d8d67..cfe8146` (without the merge of `zfs-topology`) and the code it rests on (`Engine::commit_with`, the refs store's `transact`, the uploader's commit in `crates/skyzfs-upload/src/remote.rs`, the status publisher in `crates/skyzfs-ublk/src/status.rs`) before the answers and the description. Then:

- The head's tests: `test_cut_watch.py` 8, `test_cut_daemon_main.py` 38, `test_cut_daemon_check.py` 99, `test_cut_hold.py` 18, `test_cut_plan.py` 7, `test_cut_record.sh`, `cargo test --locked -p skyzfs-nodeb` 12. All pass.
- 129 plants, one at a time (`plants2.py`, `plants2.out`): the 44 that survived at 32d8d67, and 85 new ones in what changed. 85 killed, 40 survive, 4 no longer apply (their code was rewritten; new plants take their place). The first review's 41 watcher, recorder and node B plants ran once more as they were (`plants-first-at-cfe8146.out`): nothing that was killed survives now, and R03 (`before_other` does not wait for the watcher), kernel only then, is killed by `test_cut_record.sh`. The judge's earlier kills were not run again; the tests lost no line between the two heads.
- Scenarios (`scenarios2.py`, `scenarios2.out`): the watcher as a command beside a stand-in node B, and the head's judge on recordings of run 44280164949 changed as each question asks.
- The head's judge on the recordings of runs 44280164949 and 44287481926 (`rejudge2.out`): every recording exits 0 with every upload line PASS, as in the runs. `urow` of 44280164949 ended with the rig's error there and has nothing to judge.
- The pull request's own run on cfe8146, [44295178165](https://github.com/skyzfs/skyzfs/actions/runs/44295178165), its plan and the artifact of its upload job.

Nothing here ran on a kernel. The artifacts were read as data.

## The questions

### `before_other`'s second look (6b1be16) and D1

The second look does what it says. After the look it reads the row again every five seconds while it waits for the watcher; where head or the row moved, it syncs, drains and looks again, up to five times (cut-record.sh:1027-1052). `test_cut_record.sh` holds this: plants R23 to R26 and R31 are killed. Three survive because the stand-in never varies what they change: R27 and R28 (the match on `uploaded.tsv` without the head, or without the number and group) and R30 (a cut the watcher never fetched in 120 s taken as fetched).

D1 is closed: `rec_ufailed` waits for node B with `before_other failed` before the failure (cut-record.sh:1287), so the old state's last cut is in the files. No local test reaches that line (R40 survives by construction); the kernel run 44280164949 shows it (`failed-row.txt`, and cut 9 fetched before the failure).

What the second look does not cover is F1 below: it protects the row against a later cut, not the status against an earlier one.

### Node B's `--expect-row` (G2): is a race left between the check and the act?

**None at the store.** The check and the act are one transaction on the volume's one refs object:

- `move-head`: `Engine::commit_with` puts the swap of head and node B's `MetaCas` on the row in one `transact` (skynbd-engine lib.rs:1764-1789). A daemon's commit between node B's open and its commit moves head, which reads as `Conflict`; one before the open moves the row, which reads as `OpConflict`. Both become exit 6 (main.rs:507-508).
- `set-row`: one `MetaCas` from the row read to the new row (main.rs:525-556).
- The daemon's side: its commit is the swap of head with a `MetaCas` on the row as it observed it (skyzfs-upload remote.rs:206-221). A `set-row` that lands while an upload is under way makes that commit fail, and the daemon reads again and finds the mismatch, which is the course.

On the kernel the guard has fired: in the pull request's own run, `urow`'s second act found the row moved (`acts-again.txt` has `row2`), node B ended with 6, the rig looked again and acted on cut 14 of group 33. The run passes with exactly two mismatches. So exit 6 through the command line, `guarded` and `other_act`'s loop are shown once.

Plants: N20, N22 to N27 and N29 are killed by `the_other_writer_acts_on_the_row_it_read`; R20 by `test_cut_record.sh`. Five survive, and they are the wiring between the tested function and the tested loop:

- N28 (a row that moved ends with 0), N30 and N31 (the commands give the functions no expected row), R41 (`guarded` takes 6 as done), R42 (the acts are not guarded). A run shows these only when the daemon happens to cut between the look and the act.
- N21 (`Conflict` under a guarded commit is an error, not "moved"): the test cannot move head between the open and the commit.

Suggestion: a control in the course. Before the real act, call the same guarded command once with a row that is not the one read (the number plus one) and require exit 6 and an unchanged row. It is deterministic and kills N28, N30, N31, R41 and R42 on every run.

### `upload_raw` at 2 MiB (D4): can `ufailed` now tell a first cut that leaves out the map's regions?

**Yes.** The 16 blocks are written to the device at block 512 after the old state's last upload and before the failure, in the 512 KiB to 4 MiB reserve that ZFS does not write. Only the intent map's first region names them. A new state that follows the map counts that region's blocks as changed (state.rs:871-875: the ranges go into the open map by block), so its first cut holds them; one that leaves the map out uploads only what was written since, and node B's image keeps zeros there.

Scenario B on the recording of 44280164949:

| Node B's image | `upload_content` | `upload_raw` | Exit |
|---|---|---|---|
| as recorded | PASS | PASS | 0 |
| never holds the 16 blocks | FAIL, "cut 17: 16 blocks differ", block 512 named | FAIL, "0 of the 16" | 1 |
| holds them at the old state's last cut already | FAIL at cut 9 | PASS | 1 |

The third row answers my own doubt: a daemon that made a cut of the raw write and uploaded it under the old state would make `upload_raw` pass for the wrong reason, and the content line catches it, since the reference has no cut there. In the recording the blocks first appear at the new state's fetch, and the second start's log says the map was taken up naming 2 of 128 regions.

Two small things:

- The answer on the pull request describes the FAIL and ERROR branches the other way round. The code is the right one (cut_daemon_check.py:1963-1976): blocks missing and the content line failed is FAIL; blocks missing and the content line passed is ERROR, because then the reference lacks them too.
- The FAIL branch has no test: J49 (it says PASS) survives. `test_the_blocks_written_before_a_failure` reaches PASS, ERROR and no line. J50 survives too (`raw_held` reading the first fetch alone): the test's image holds two blocks either way.

### `ulease`'s new want (cd849a6)

Right, and held by the tests. Rows before `release-ns.txt` must all have the head of node B's first look and no cut in the row (cut_daemon_check.py:1766-1776). Plants J58 to J62 are killed, among them the want from before cd849a6 (head `none`), which a right daemon fails because `create` puts a commit at head. Scenario D: a head that moves, or a row that names a cut, at the second look gives `upload_course` FAIL.

Its power on the kernel is thin, which is worth knowing: in 44280164949 node B read 12 rows in all and 2 before the release, the last one 0.13 s before it and 0.16 s after the first instance's last request. The first instance lives for well under a second. An upload without the lease moves head for good, so one look after the daemon's end is enough, but whether there is one rests on the watcher's half second. Suggestion: the rig reads the row itself after `detach` and before the release (`nbrow release-row.txt`), and the judge holds that file to the same want.

### D2: a `get` that always fails, and a test of the watcher's main

The behaviour is fixed. Failures are counted per command, and the last look must settle or it is the rig's error (cut_watch.py:132-146, 193-200). Scenario A runs `cut_watch.py` as a command beside a stand-in whose `row` works: `get` always ending with 2 gives exit 2 after three tries, with the stop there or not; `get` ending with 4 gives exit 1 with a VERDICT line. Plants W30 to W36 are killed, and so is W11.

There is still no test of `main` itself: W18 (the pause ignored), W19 and W20 (the exit after a verdict and after an error) and W38 (`step` for `finish` at the stop) survive. The tests call `Watcher.finish()` directly. A test that runs the command as Scenario A does would take ten lines.

### D3: the balance's start look

Closed. The balance starts from `balance-start.txt`, taken with a drain once nothing moves, in the three courses that start the daemon again; a later instance without one is the rig's error, and a start look that is not idle with nothing held is an ERROR line (cut_daemon_check.py:1946-1960). Plants J40, J41, J43 to J46 and the earlier J19j are killed. J42 survives (at rest with nothing held whatever `upload_state` says): no test has a look that holds nothing and is not idle.

What it costs is F3 below.

### The plan of the jobs (cfe8146)

**It selects the upload job when a listed path changes and never drops a record job.** All 30 plants in `cut_plan.py` are killed: each of the nine paths removed, a prefix taken for a file or a directory, the upload job in place of the others, on arm64, also holding cuts, without `ulost`, a dispatch's inputs each ignored, a push changed. The outputs for a dispatch, a push and a pull request are what the workflow's expressions gave before. On this pull request's own 13 changed paths the plan gives three jobs, and run 44295178165 ran those three.

The upload job ran `life`, `umoved`, `urow`, `ukill`, `ulease` and `ulost` in 17.5 minutes with the builds; all exit 0. It is the first kernel run of `umoved` and `ulost` with 6b1be16's `before_other`.

What the list leaves out is F4, for the lead.

## Findings at this head, most severe first

None blocks the merge.

**F1. The look's status can be one upload older than its row, and then `umoved` and `ulost` fail a right daemon.** cut-record.sh:1031-1032, cut_daemon_check.py:1689-1691 and 1780-1782.

`before_other` takes the status with `drain` and then the row with `nbrow`. The status is a file the daemon rewrites at most once in 100 ms (`STATUS_INTERVAL_MS`, serve.rs:227; `Pace::due`), so a read just after `zpool sync` or `zpool export` returns can still show the state before the last cut: idle, nothing held, `uploaded` N-1. `drain` ends at the first read that says so. If that cut's upload lands before `nbrow`, the row names cut N. The watcher fetches N, the look returns, the guarded act holds (the row is as read), and at the end the status says `uploaded` N against the look's N-1.

- Wrong result: `upload_status FAIL uploaded 'N', not N-1`, exit 1, a verdict against a daemon that did nothing wrong. Scenario E shows the judge's side on both courses. In `ulost`, where nothing guards what follows the look, an older look can also end as "head and the row are ... at the end".
- The second look does not help: the row is read after the status, and a row the watcher has fetched returns at once.
- Certainty: the mechanism is certain from the code. I have not seen it: in the nine looks of the three runs I have (44280164949, 44287481926, 44295178165), the status's `cut_group` equals the row's group every time, and eight of the nine drains took one to three seconds, so their first read saw the cut held. How often it happens I cannot say without a kernel; I expect rarely.
- Why it matters now: cfe8146 puts `umoved` and `ulost` on pull requests that change the uploader, where a false exit 1 lands on someone else's work.
- Fix, either: wait before the first status read as `end_daemon` does for the same reason (cut-record.sh:497-500, `sleep 1`: "a second after the export it holds the last cut"), or after `nbrow` require the status's `cut_group` to be the row's group and look again where it is not.

**F2. The guard's wiring is shown only when the race happens** (N28, N30, N31, R41, R42 above). A gap, with the control suggested there.

**F3. `ukill`'s balance no longer covers the take-over.** In 44280164949 the second instance counted 111 cuts (208 to 319) and the balance covers the last 4, from the look at rest after the writer ended. The cut found committed (rule 1) is let go before that look, so `upload_found_committed` changes by 0 inside the balance, and a daemon that miscounts at the take-over passes it. The want `upload_found_committed 1` and `cuts_held 0` at the end remain. This is the price of D3's fix and I think it is the right trade; the description's figure "`ukill` 4 = 2 + 2 + 0 + 0" should say that it is the tail. A whole-instance balance needs a start look the daemon posts in one piece, which is the daemon's to give, not the rig's. For the record, in that run the whole instance does balance: 111 cuts and 2 held at the start, 36 let go and 77 replaced.

**F4. Paths the plan does not count (for the lead, who set the list).** cut_plan.py:30-40. A pull request that changes only these starts no upload course:

- `crates/skyzfs-blk/`, a dependency of `skyzfs-upload`;
- `Cargo.lock` and `Cargo.toml` (a new version of the object store's client);
- `tests/lib/` (`cut-record.sh` sources `gnu-dd.sh`), `tests/kernel/cut_plan.py` itself, `cut_summary.py`;
- `crates/skynbd-engine/`, `skynbd-refs/`, `skynbd-store/`, `skynbd-format/`: the commit, the row's transaction and the S3 client the uploader runs on. These are not in the workflow's `paths` either, so they start no job of this rig at all.

All but the last group already start the workflow. Two smaller things in the same place:

- `git diff --name-only "$BASE...$HEAD"` (workflow line 127) follows renames, so a file moved out of a listed directory shows under its new name only. `--no-renames` lists both.
- A dispatch with `images` emptied ends the plan with a Python traceback (`json.loads('')`, cut_plan.py:58); before, the expression fell back to the two default images. A message naming the input would do.

**F5. No test of the watcher's `main`** (W18, W19, W20, W38; under D2 above).

**F6. Small test gaps.**

- W02a still survives. `test_head_or_the_row_alone_moved`'s case "the row alone" uses `row_text(2, 11)`, which also changes the generation printed after head (test_cut_watch.py:52-53), so both move.
- N08 survives: `the_other_writer_acts_on_the_row_it_read` moves head with a group and never reads the group of the commit.
- J49, J50, J42 as said above.

## The first review's items

| Item | State at cfe8146 |
|---|---|
| D1 `ufailed` without the old state's last cut | Closed (`before_other failed`); kernel only |
| D2 a failing `get` never the rig's error | Closed in behaviour; `main` untested (F5) |
| D3 the balance's start look | Closed; `ukill` covers the tail only (F3) |
| D4 `ufailed` cannot tell the map's regions | Closed (`upload_raw` and the content line) |
| G1 no floor on what the fetches show | Not done, stated. `ukill` in the pull request's run: 3 of 504 cuts fetched |
| G2 the window before the other writer's act | Closed at the store; shown once on the kernel; F1 and F2 remain |
| G3 `ulost` and a commit in flight | Not done, stated |
| G4 `ulease` and `rows.tsv` | Closed; two rows in the run |
| G5 the holder the judge wants | Closed: J54 to J57 killed |
| G6 tests | `test_cut_watch.py` and `test_cut_plan.py` run in `ci.yml` and `skyzfs-ci.yml`. Now killed: W02b, W11, W17b, J19j, J20d, J20e, J21, J21d, J21e, J22d, J22e, J26, J26c, J26d, J26g, J26i. Still surviving: W02a, W04, W05, W18, W19, W20, W22, J04, J07, J08, J16b, J17h, J17j, J17k, J17l, J19d, J24e, J27, J28, N03, N07, N08, N10, N11 |
| G7 the upload stage on pull requests | Done by the plan (the lead's decision); F4 |

## Plants at cfe8146

| Where | Plants | Killed | Survive | No longer apply |
|---|---|---|---|---|
| Survivors of 32d8d67, run again | 44 | 16 | 24 | 4 (W12, W17, J17m, N09) |
| The watcher (W30 to W38) | 7 | 6 | 1 (W38) | |
| The judge (J40 to J62) | 22 | 19 | 3 (J42, J49, J50) | |
| The recorder, reached by `test_cut_record.sh` | 9 | 6 | 3 (R27, R28, R30) | |
| The recorder, kernel only (R40 to R44) | 5 | | 5, by construction | |
| Node B (N20 to N31) | 12 | 8 | 4 (N21, N28, N30, N31) | |
| The plan (P01 to P24) | 30 | 30 | | |
| All | 129 | 85 | 40 | 4 |

## Files

Under `review/m2.t3-3/` on `skyzfs-review`: `next_look.md` (this), `plants2.py` and `plants2.out`, `plants-first-at-cfe8146.out`, `scenarios2.py` and `scenarios2.out`, `rejudge2.out` (made with `rejudge.py`). The first review's files are beside them.

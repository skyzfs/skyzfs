# Third look at #148 (M2.6 Q10): the recorder of the adapter's ZFS commands

Review session, read-only on GitHub. Nothing here was posted.

## Heads judged

| Part | Head | Diff read |
|---|---|---|
| The answer to the next look (N1-N6, T1-T3, S9-S14), the four questions, the recordings 45679911640 (24.04) and 45681972805 (26.04, scale 100 1000 10000) | 77151b2 | `dd75e16...77151b2` |
| The transaction-group probe, the writer of new data, `ZCR_SCALE_LUA_MAX`, `stop_bg`; the recording 45691238816 (26.04, scale 100 1000 10000, `scale_lua_max` 1000) | 7a813d9 | `77151b2...7a813d9` |
| The verdict | 7a813d9 | |

Base: `zfs-topology` at a4e004e (the merge base). `zfs-topology` is now at
e6788b0, the lead's design change: the published snapshot is taken by
`zfs snapshot -r` and the manifest is read afterwards.

**77151b2's content is unchanged at 7a813d9.** Checked on the diff:

- the tests remove no lines (`test_zfs_cli_record.sh` +47, `test_dsync_writer.py` +31);
- `zfs-cli-record.sh` removes 27 lines, all in the header's scale paragraph, `writing()`, `scale`'s `local LIMIT`, and the two lines that call `writing` for the takes;
- `dsync_writer.py` removes 4 lines: `write`'s signature and its flags. Its defaults are unchanged.

Every survivor at 77151b2 that the new head did not touch survives at
7a813d9 too (plant tables below).

Line numbers are given per head: "77:N" is 77151b2 and "7a:N" is 7a813d9.

## Verdict at 7a813d9

**Can merge once the description covers 7a813d9.** CI on 7a813d9 is all
green, the daemon rig and the replay rig included. No code defect stops the
recorder from doing its main job, which is to record the adapter's outputs as
fixtures. N1 is fixed. The tag and the shape of `destroy -r` agree with #147
at ab8db11.

What blocks:

1. **The description is at 77151b2.** It says "The commits are
   bd37422..77151b2". It does not cover the probe, the writer of new data,
   `scale_lua_max` or run 45691238816. When it is brought up to date it
   should:
   - say outright that the times at 10000 after the program are on 4563
     snapshots (R1). Today that rests on the words "which stayed. At that
     `@t`";
   - say what R2 means for the new lines.

R1's code fix is a few lines and is recommended in this pull request. With
`scale_lua_max` empty, the default, a dispatch at 10000 takes that path
again. The lead may take it as a follow-up instead (decision 1 below).
Nothing else blocks.

## Findings, most severe first

### Defects

**R1. After a partial take by the program, the times at 10000 are on 4563 snapshots, and nothing says so** (medium; certain; both heads).

- **Where:** 77:554 and 7a:646 (`scale`); the same test at 77:367 and 7a:373 (`record_pool`).
- **The fault:** `lim 120 zfs list -H -o name "$p@t" || rec ... zfs snapshot -r "$p@t"` asks only whether the root has `@t`. The header (7a:51-52) says snapshot-cli runs "only if the program did not take @t". When the program stops part way, the root's `@t` exists, so snapshot-cli is skipped. `members`, `hold`, `tagged`, `release` and `destroy` then run on what was taken.
- **Seen in run 45681972805 (77151b2):**
  - `scale-10000-snapshot` returns `{"return":{"taken":"4563","failed":"skzqs/d183@t","errno":"28"}}` after 843.03 s;
  - `scale-10000-members` lists 4563 members, against 101 and 1001 at the smaller counts;
  - there is no `scale-10000-snapshot-cli` record, and nothing in `about.txt`.
- **So** hold 27.3 s, tagged 35.4 s, release 27.8 s and destroy 34.7 s are times on 4563 snapshots.
- **Run 45666280714 (dd75e16)** is the same: 4563 taken. The description says "4563 snapshots were taken, which stayed. At that `@t`: members 24.3 s, hold -r 36.0 s, tagged 45.6 s, release -r 36.6 s". That is right, but only by inference, and the recording itself says nothing. The times at 10000 in the 77151b2 run are not in the description.
- **At 7a813d9** `scale_lua_max` is empty by default (the workflow, 7a:22-24). A dispatch with 10000 and no `scale_lua_max` takes this path again. Run 45691238816 avoided it only because it set 1000.
- **Fix:** decide on the program's own answer, not on the root. When its return has `failed`, run `destroy -r @t` and then snapshot-cli. Or at least write the member count beside each time in `about.txt`.

**R2. The writer of new data runs beside every take, so the stall and txg lines of a short take show the writer's own cost and not the take's** (medium for what the lines are for; certain from the run; 7a813d9).

- **Where:** `writing()` 7a:518-572, `fill_span` 7a:577-595.
- **What the fill does:** 1 MiB buffered writes over 2 × `zfs_dirty_data_max` (here 3.29 GB) run through the whole window.
- **What run 45691238816 shows:**

  | | the take's secs | O_DSYNC writer: longest / writes / before | new data: longest / MiB / before | txg line |
  |---|---|---|---|---|
  | 100, program | 4.390 | 2.475 / 5820 / 1.708 | 2.730 / 320 / 1.711 | txgs 2, longest_sync 2.801, gap 2.801, sync_longest 6.796 |
  | 100, `zfs snapshot -r` | 2.695 | 2.436 / 2 / 0.770 | 2.430 / 256 / 0.779 | txgs 2, longest_sync 2.485, gap 2.485, sync_longest >6.226 |
  | 1000, program | 28.125 | 1.388 / 133807 / 0.600 | **27.355** / 300 / 0.618 | txgs 2, **longest_sync 27.432**, gap 27.432, sync_longest 29.679 |
  | 1000, `zfs snapshot -r` | 2.471 | 1.604 / 6 / 0.788 | 1.595 / 349 / 0.751 | txgs 2, longest_sync 1.638, gap 1.638, sync_longest 4.526 |
  | 10000, `zfs snapshot -r` (program skipped) | 18.146 | 6.834 / 245 / 0.063 | 6.834 / 2209 / 0.058 | txgs 10, longest_sync 5.770, gap 6.896, sync_longest 10.897 |

- **During the 1000 program's 27.4 s hold**, the O_DSYNC writer went on: 133807 writes, the longest 1.388 s. New data was held for 27.355 s. That is the caveat in 77151b2's description, now measured: a take that holds the sync stops new data but not ZIL writes.

- **Reading the groups myself** (`probes-third/txgread.py`):
  - The clock check passed in every log: every group's end falls between the read before it and the read that saw it. So the offset from `CLOCK_MONOTONIC_RAW` is right.
  - Group 386, in the 1000 program, synced for 27.432 s with ndirty 313 MiB and nwritten 1415 MiB. That is a real hold by the program, and the new-data writer was held 27.355 s with it.
  - Every other group carries 250-480 MiB of the fill and syncs in 1.2-2.8 s. Each `zpool sync` took 4.5-6.8 s.
  - At 10000, `zfs snapshot -r`'s group 3565 had s 5.770, q 1.126, ndirty 163 MiB and nwritten 431 MiB.
- **So only the 1000 program's hold stands out.** The 100 lines and every `zfs snapshot -r` line are mostly the fill's own sync time. The O_DSYNC writer's baseline (`before`) went from 0.001-0.2 s at 77151b2 to 0.6-1.7 s here, because its writes now meet the dirty-data throttle too.
- **The reader cannot tell which part is the take's:**
  - the txg line has no baseline (nothing like `before`) and no ndirty of its longest group;
  - the takes' secs are under that load, so they cannot be compared with 77151b2's (a different runner as well);
  - when the fill stops, the dirty data left behind is synced while the next records run.
- **Suggestion:**
  - a baseline in the txg line: the longest sync and `zpool sync` in the margin before t0;
  - the longest group's ndirty and nwritten in the line;
  - a bounded `zpool sync` after the fill stops, before the next record;
  - and either a take with the probe alone as the control, or the fill beside the program's take only.

  This is decision 2 for the lead.

**R3. Neither line says whether the writer or the probe covered the window** (low to medium; certain by probe; writer at both heads, probe at 7a813d9).

- **Where:** `stall` in `dsync_writer.py` (77:66-94, 7a:75-103) and `report` in `txg_probe.py` (7a:124-179) take any log that has a start. `writing()` (77:477-510, 7a:518-572) drops the processes' exit status: after `wait`, `stop_bg` returns 0 whatever it was. Their `.err` files are moved but not mentioned.
- **Probes** (`probes-third/writer_probe.sh`, `txg_sim.py` and `txg_fail_probe.sh`, outputs beside them):

  | Case | Line | Exit |
  |---|---|---|
  | writer SIGKILLed before the window | `longest=<0.01 writes=0 before=0.002` | 0 |
  | writer whose writes fail (ENOSPC) | `longest=<0.01 writes=0 before=0.000` | 0 |
  | writer that started 1.5 s into the window | not flagged | |
  | probe SIGKILLed before the window | `txgs=0 longest_sync=- longest_gap=- syncs=0 sync_longest=-` | 0 |
  | every `zpool sync` exits 1 | `syncs=2 sync_longest=0.051` | |
  | the kstat missing, or its header alone (`zfs_txg_history` 0) | `txgs=0 longest_sync=- longest_gap=>6.015` | 0 |

  - In the `zpool sync` case the log's rc column is ignored, so a failed sync reads as a fast one.
  - In the kstat case the line reads like a gap the length of the whole run.
- **So** a writer that died reads as a quiet window: a false pass on the very question the line answers. A probe that read nothing reads as a hold: a false fail.
- **Not in the recorded runs.** In run 45691238816 every log begins at least 2 s before its take and ends at least 2 s after it: the spans are secs + 4.0-7.4 s. There are no `error` lines, and every sync's rc is 0.
- **Fix:**
  - `stall` and `report` say "did not cover the window" unless start ≤ t0 and the end or stop (or the last sec or tick) is ≥ t1;
  - `report` counts error lines and syncs with rc ≠ 0;
  - `writing()` adds a nonzero exit status to the line.

**R4. The processes beside a take hold the script's stdout** (low; certain; both heads).

- **Where:** the writer at 77:485. At 7a813d9 the writer, the writer of new data and the probe, at 7a:526, 7a:529 and 7a:532.
- **Shown by** `probes-third/stdout_probe.sh`: the writer's fd 1 is the same pipe as the script's stdout.
- `lim.sh`'s own rule is that a child left behind must not hold the caller's pipe. `stop_bg` leaves a process behind after `kill -9` when it does not end in 60 s, and it then holds the job step's pipe.
- It needs a write stuck for more than 60 s, which is unlikely on a healthy pool. The `zpool sync` children of the probe already use DEVNULL.
- **Fix:** `> /dev/null` on the three.

**R5. The header's claim about the index order is false for `mirror-rebuild`** (low, doc; certain; both heads).

- **Where:** 77:29-31 and 7a:29-31 say that "a record run in the background (mirror-online, mirror-rebuild) comes after the records that watched it". The answer's N6 repeats it.
- **Seen in** 45679911640, 45681972805 and 45691238816: `index.tsv` has `mirror-rebuild` at line 123, before `mirror-rebuild-status` and `-g` at 124-125.
- **Why:** the status try waited behind `attach -s` and ended after it. The index is in the order records ended, which is the first half of the sentence.
- **Fix:** drop the second half, or have `during` write the background's index line last.

**R6. The budget's wording promises more than the code does** (low, doc; certain; both heads).

- **The words:** the workflow's `scale` input says "within 80 minutes in all", and the answer says "Once the budget is spent nothing starts".
- **What is outside the budget:**
  - `zpool create` (`lim 180`), and `zfs set` on the assist, which has no limit at all (7a:614);
  - two `lim 120 zfs list`;
  - `writing`'s margins and stops (≤ 60 s each, three at 7a813d9);
  - `lim 600 zpool destroy`.
- **So** the scale phase can end about 15-18 minutes after the 80. It still fits the job's 120 minutes: all of run 45691238816 took 9 minutes.
- **Fix:** say "starts nothing after 80 minutes".

**R7. `writes=0` for a window shorter than a second** (low; certain; both heads).

- **Where:** `stall` counts only writes in the window's whole seconds (77:85, 7a:94).
- **Seen:** the 100 `zfs snapshot -r` in run 45681972805 (0.56 s) reads `writes=0`, though the writer was doing thousands of writes a second.
- **Fix:** show `writes=-` when the window holds no whole second.

### Test gaps (plants that survived)

All plants are textual, one at a time, on a copy, with the unit tests run under
600 s.

- **The one place the test pins less than at dd75e16.**
  - **L1:** the call to `adapter_env` in `main` (77:604, 7a:696) can be deleted.
  - **L2:** it can be moved after the version, nocacheflush and locale records.

  The locale rule's test calls `adapter_env` itself, so it does not see `main`. At dd75e16 the export in `main` was pinned by a grep. Fix: pin the call, before the first `rec`.
- **V22a: N3's fix is unpinned.** `during` could go back to a bare `wait` (77:589, 7a:681), which would wait for every background child, including what `lim` gave up on. Fix: a test that calls `during` with an unrelated background child alive.
- **Unpinned bounds:**
  - V19c: the cap `ZCR_SCALE_LIMIT`;
  - B1: the default budget (48000 passes);
  - B2: the create loop under `lim`;
  - B3: `-u` on the creates.
- **N1d, N1g:**
  - `destroy-held-hold` could put its hold on the root instead of `child@o2`. The point of that record is that a child's hold stops the whole `destroy -r`.
  - The release after it could be dropped.

  Fix: pin that record's words and the release.
- **G1 (7a813d9; at 77151b2 too, but not planted there):** `get-checkpoint` (7a:417) without `-p` passes. The rules do not cover `zpool get`'s `-p` (the first site is 7a:338; it was not planted).
- **The wiring in `writing()`:**
  - W1: the stall's window empty;
  - W4: no margins;
  - T2 and T3 (7a813d9): the probe's or the fill's window empty.

  The test checks that the lines exist, not what window they were made on. Fix: a fake log with one slow write inside the window and one outside, and check the line.
- **The writer's details:**
  - P4: `>` for `>=`;
  - P5: the last second's line;
  - P10: any second that overlaps counts;
  - P12: 8 KiB writes.

  P2 (O_DSYNC) and P9 (the 64 MiB wrap), survivors at 77151b2, are killed at 7a813d9 by its new writer tests.
- **The probe's gaps that matter** (7a813d9):
  - **Q7:** `report` counting only the `zpool sync` runs that began in the window passes. A sync that began in the margin and was held across the take is exactly the case a hold makes, and it would drop out of the line. `test_a_sync_across_the_window` tests groups, not syncs.
  - **Q9:** the probe not logging a sync still open at its end passes. In run 45691238816 four of the five windows ended with one open, and without that line `sync_longest` loses its `>`.
  - **W3 (in `stop_bg` here):** SIGKILL in place of SIGTERM passes. The writer then leaves no end line and the probe no stop line.
  - **T11:** `stop_bg` saying a process stopped when it did not passes. The 60 s path is not tested.

  Fix: a `report` test with a sync that begins before t0 and ends after t1; a probe test that kills the fake `zpool` late; and a fake process that ignores SIGTERM, with a shorter wait in the test.
- **Smaller ones** (7a813d9):
  - T5: the fill span's 4 GiB cap;
  - F2: the fill's 1 MiB block;
  - T10: `stop_bg` reaping its process;
  - Q14: the last read after the stop.
- **Equivalent or practically so** (7a813d9):
  - **T15** (no `local FILL_SPAN`): `fill_span` resets it on every call, and with no writer's directory `writing` ignores it.
  - **Q5** (the start always an anchor): it adds only a pair of negative length.
  - **Q2** (`CLOCK_MONOTONIC` for `CLOCK_MONOTONIC_RAW`): the two clocks differ by milliseconds on a freshly booted machine. The code is right, because `gethrtime()` is the raw clock. A test would need a fake clock.

### Suggestions (not defects)

- **The probe leaves a `zpool sync` running**, by design: "open" was logged in four of the five windows. The next `lim 600 zpool destroy -f` (7a:657) could meet it and fail with EBUSY. Its result is not checked or said, so the next count would only say "zpool create failed". It did not happen in run 45691238816, where all three pools were made. Uncertain.
- **`fill_span` sets `zfs_txg_history` to 100 and does not put it back.** It is harmless on a hosted runner.
- **#147's `cli.rs:280` says `destroy` is "not recorded on ZFS"**, which is out of date now. `zfs holds -H` without `-r` is recorded but no longer used by #147; that is harmless.

## The four questions

1. **Do the rules-based pins fail when a recorded behaviour changes, and do they pin less than before?** (77151b2)
   - **What they catch:** a change of flag, tag, locale, `-g` partner or `destroy`'s shape, at every site the test runs. That is V2-V8, V17, N1a and N1e.
   - **The other checks:**
     - the site counts catch V9 (a status without its `-g` partner) and V30 (the rebuild's pattern);
     - the `--expect` checks catch V29 and N1c.
   - **Kept from dd75e16:** every per-record pin. "Pin the -g partners" is subsumed by a stronger rule.
   - **Less than before** in one place: `main`'s call to `adapter_env` (L1, L2).
   - **Not checked by the rules:** a command's other words (pool and snapshot names, a program's trailing arguments), and `zpool get`'s `-p` (G1).
2. **`during` (N3).**
   - Correct: it waits for the background record's pid alone, and the record's limit is its own (`ZCR_CHANGE_LIMIT`, 900 s).
   - V22b and V22c are caught.
   - The pid wait itself is unpinned (V22a).
3. **The scale budget and bounds (N2).**
   - The workflow and the script agree. `probes-third/wfcheck.sh` checks the workflow's validation:
     - accepted: '', "1000 10000", "5 100000", "100 1000 10000", "5 5 5" and " 7 ";
     - refused: four counts, 100001, a 20-digit count, 012, 0, "1;x" and a newline.
   - The script's bounds are caught when changed (B4-B11).
   - At 7a813d9, `ZCR_SCALE_LUA_MAX` is checked in both places. "0" means the program runs at no count.
   - The overrun past the budget fits the job (R6).
   - Unpinned: the default budget, `ZCR_SCALE_LIMIT`, and the create loop's `lim` (B1, V19c, B2).
   - The fallback after a partial take is wrong (R1).
4. **`dsync_writer.py` and its stall line.**
   - Sound while the writer runs through the window.
   - **False pass** when the writer died, started late, or its writes failed (R3), and as `writes=0` under a second (R7).
   - **No false fail seen.**
   - **No hang without a limit:**
     - `stop_bg` gives each process 60 s and then a `kill -9` that is not waited for;
     - the process writes to files;
     - `stall` and `report` read a file;
     - the probe polls its `zpool sync` and never waits for it.
   - It holds the stdout pipe (R4).
   - **At 7a813d9:** O_DSYNC writes go through the ZIL. With the fill beside it, the writer now meets the dirty-data throttle, so its line mixes the two (R2).

**N1 and the tag** (77151b2, unchanged at 7a813d9).

- `zfs destroy -r` is recorded on `@o0`, on `@absent` (with `--expect`), on `@o2` with a hold on a child, and on `none`. The default `lua_ref` is 0f709ae.
- **24.04 (45679911640):**
  - `@absent`: rc 1, "could not find any snapshots to destroy; check snapshot names.";
  - held: rc 1, "dataset is busy";
  - `none`: rc 1, "cannot open 'skznone': dataset does not exist".
- **26.04 (45681972805):** held: rc 1, "it's being held. Run 'zfs holds -r …'".
- On both images every member's `@o2` is left: the destroy is atomic.
- **#147 at ab8db11 agrees:**
  - `HOLD_TAG` is `skyzfs-publish`, and so is the recorder's tag;
  - `destroy` runs `["destroy","-r","POOL@T"]`;
  - `destroyed()` reads exit 0, "could not find any snapshots to destroy" or a failure, which matches what was recorded;
  - `LC_ALL=C` is set in `host.rs`;
  - the Lua is unchanged since 0f709ae.

**The rebuild (N4).**

- **26.04:** caught at try 2, "resilver (mirror-0) in progress" at "2G resilvered, 100.00% done". The try took 10.307 s against the attach's 10.545 s.
- **24.04:** not caught; only the scrub after it was seen.
- `zinject-rebuild.txt` lists the handlers on `m0` and `m1`.
- The header's words now match.

## The probe at 7a813d9: soundness

- **The clock.**
  - A group's end is `(birth + o + q + w + s) / 1e9` plus the offset `time.time() − CLOCK_MONOTONIC_RAW`.
  - `birth` is SPL's `gethrtime()`, which is the raw monotonic clock. The kstat's times are differences of the state times, so the sum is the time it was synced.
  - The clock check above confirms it on the run.
  - A channel program and `zfs snapshot -r` run in syncing context, so a hold shows in `s`, and `longest_sync` is the right measure. `txg_sim.py`'s simulated 3 s hold reads `longest_sync=3.000 longest_gap=3.001 sync_longest=3.059`.
- **Coverage.**
  - With `zfs_txg_history` 100 and a read every second, a group is missed only if more than 100 commit within a second.
  - A missing or empty history reads as a hold (R3).
- **Hangs.**
  - On SIGTERM the probe ends within 50 ms.
  - It never waits for its `zpool sync`, which may be in an uninterruptible wait.
  - `stop_bg` bounds the whole at 60 s.
  - A `zpool sync` left behind holds DEVNULL, not the script's pipes.
- **Where the line is uncertain:** with the fill on, the longest group and the longest `zpool sync` are mostly the fill's (R2).

## Plants at 77151b2 (`plants_third.sh`, output `plants-third-77151b2.out`)

Base: shell 112 of 112 and python 9 tests pass. Result: 42 KILLED, 18
SURVIVED.

| Plant | Result | What it changes |
|---|---|---|
| V2 V3 | KILLED | the locale: `LC_ALL=C.UTF-8` after the pinned line; `rec` with C.UTF-8 |
| V4 V5 V17 | KILLED | `-n` dropped (tagged-held, none's members, scale's members) |
| V6 V7 V8 | KILLED | `-r` dropped from hold-2 and release-2; the old tag |
| V9 V30 V30b | KILLED | a status without its `-g`; the rebuild's catch on "in progress"; the pattern reverted |
| U7 | KILLED | `catch` keeps an earlier try's rc |
| V19a V19b | KILLED | the limit line dropped; scale's LIMIT not local |
| **V19c** | SURVIVED | each command gets all that is left, not `ZCR_SCALE_LIMIT` |
| **V22a** | SURVIVED | `during` with a bare `wait` |
| V22b V22c | KILLED | `during` without a wait; the background record under LIMIT |
| V26 K1 | KILLED | `keep`: argv not NUL-separated; secs dropped |
| V29 N1c | KILLED | `--expect` dropped (hold-2, destroy-absent) |
| N1a N1b N1e N1f | KILLED | `destroy-held` without `-r`; none's destroy dropped; another tag; `-d` added |
| **N1d N1g** | SURVIVED | the hold on the root; the hold on `@o2` not released |
| **L1 L2** | SURVIVED | `main` without `adapter_env`; called late |
| **B1 B2 B3** | SURVIVED | budget default 48000; the create loop without `lim`; no `-u` |
| B4-B11 | KILLED | four counts, no length bound, 1000000, a count twice, nothing left not refused, no fallback snapshot, no `df`, no budget check at the start |
| **W1 W3 W4** | SURVIVED | stall's window `t0 t0`; SIGKILL (no end line); no margins |
| W5-W8 | KILLED | no 60 s to stop; no `about.txt` line; no writer beside take-cli; the writer's file in OUT |
| P1 P3 P6 P7 P8 P11 | KILLED | writer and stall details |
| **P2 P4 P5 P9 P10 P12** | SURVIVED | O_DSYNC dropped; `>` for `>=`; the last second; no 64 MiB wrap; any overlapping second; 8 KiB writes |

The author's 15 survivors from the last look are all killed, in the author's
forms.

## Plants at 7a813d9 (`plants_third_7a.sh`, output `plants-third-7a813d9.out`)

Base: shell 142 checks, `test_dsync_writer.py` 10 and `test_txg_probe.py`
14 tests pass.

Result: 64 KILLED, 29 SURVIVED, 93 plants. The 60 plants of 77151b2 were
adapted where the head moved:

- W3 is on `stop_bg`;
- W7 is on take-cli with `"$p"`;
- P2 is on `flags=os.O_DSYNC):`;
- P6 is on line 97;
- P9 is on `(n % span) * block`;
- V19b is on `local LIMIT FILL_SPAN=""`.

| Plant | Result | What it changes |
|---|---|---|
| V2-V9, V17, U7, V19a, V19b, V22b, V22c, V26, V29, V30, V30b, N1a, N1b, N1c, N1e, N1f, B4-B11, W5-W8, K1, P1, P3, P6, P7, P8, P11 | KILLED | as at 77151b2 |
| **P2 P9** | KILLED (survived at 77151b2) | O_DSYNC dropped; no 64 MiB wrap: caught by the new writer tests |
| **V19c V22a N1d N1g L1 L2 B1 B2 B3 W1 W3 W4 P4 P5 P10 P12** | SURVIVED | as at 77151b2 |
| **G1** | SURVIVED | `get-checkpoint` without `-p` |
| T1 | KILLED | the probe reads another kstat |
| **T2 T3** | SURVIVED | the probe's, the fill's window `t0 t0` |
| T4 | KILLED | the span is the limit, not twice it |
| **T5** | SURVIVED | no 4 GiB cap |
| T6 T7 T8 T9 | KILLED | history not turned on; `-ge` for `-gt`; `ZCR_SCALE_LUA_MAX` not checked; not capped |
| **T10 T11** | SURVIVED | `stop_bg` does not reap; says stopped when not |
| T12 T13 T14 | KILLED | no writer of new data; `about.txt`'s txg line dropped; the probe not stopped |
| **T15** | SURVIVED | no `local FILL_SPAN` (equivalent) |
| F1 F3 | KILLED | fill with O_DSYNC; its span taken as blocks |
| **F2** | SURVIVED | fill in 4 KiB writes |
| Q1 Q3 Q4 Q6 Q8 Q10 Q11 Q12 Q13 | KILLED | a group's end without o, q, w; w and s swapped; longest_sync only of groups that ended in the window; an anchor's gap not open; no `zpool sync`; a group logged at every read; groups not committed logged; the stop not an anchor; txgs counts every group |
| **Q2** | SURVIVED | `CLOCK_MONOTONIC` (practically equivalent) |
| **Q5** | SURVIVED | the start always an anchor (equivalent) |
| **Q7 Q9 Q14** | SURVIVED | only syncs that began in the window; an open sync not logged; no read after the stop |

## CI and recordings (read-only)

| Workflow | 77151b2 | 7a813d9 |
|---|---|---|
| ci (lint runs the shell test, `test_dsync_writer.py` and, at 7a813d9, `test_txg_probe.py`; shellcheck) | 45679909655 success | 45691245703 success |
| coverage | 45679909467 success | 45691245589 success |
| e2e | 45679909542 success | 45691245475 success |
| skyzfs-kernel-probe | 45679909527 success | 45691245659 success |
| skyzfs-daemon-rig (run because `tests/kernel/` changed; this pull request has no daemon code) | 45679909642 success | 45691245781 success (seven jobs: daemon on 24.04, 26.04 and 26.04-arm, and the stand-in) |
| skyzfs-replay-rig | 45679909594 success | 45691245600 success (eight jobs, both kinds) |
| skyzfs-ci (push) | 45679895498 success | 45691221912 success |
| recordings | 45679911640 (24.04), 45681972805 (26.04, scale) success | 45691238816 (26.04, scale, `scale_lua_max` 1000) success |

- No Codex comment or review since bd37422.
- The description was not updated after 77151b2.
- Local: ShellCheck 0.9.0 here reports version-only warnings (SC2218, SC2015, SC2317). CI's shellcheck step is green at both heads.

## For the lead

1. **R1: fix now, or a follow-up?** Either it is fixed in this pull request
   (recommended, a few lines), or it is a follow-up and the description
   labels the 10000 times of dd75e16 and 77151b2 as on 4563 snapshots. Either
   way, the hold, tagged, release and destroy times at 10000 in the notes
   should carry their count.
2. **What should the take's lines answer, now that the take is `zfs snapshot -r` (e6788b0)?**
   - **Option A:** whether a take holds new-data writes. That needs the fill and the probe, with a control.
   - **Option B:** whether it delays synchronous writes. That needs the O_DSYNC writer alone.

   Running both together, as 7a813d9 does, blurs the second (R2). The one
   clear result so far is that the 1000 program held a group's sync for
   27.4 s, and new data with it (run 45691238816).
3. **`zfs snapshot -r`'s failures are not recorded**, since e6788b0 makes it
   the take: a child's snapshot that exists, a pool that does not. Today the
   command is recorded only where it succeeds (scale) and in `record_pool`'s
   fallback, which does not run when the program works. Decide whether Q10
   records them now or when #147 adopts the command.
4. **Is the 4563 limit of the program a function of pool size?** That is
   unknown (16 GiB pool, ENOSPC, the same count at two heads). It no longer
   matters with e6788b0.
5. **#147's `recorded/*.json` come from bd37422 with the old tag**, and
   should be refreshed from a run at this head.

## Files beside this note

- `plants_third.sh`, `plants-third-77151b2.out`: the plants at 77151b2.
- `plants_third_7a.sh`, `plants-third-7a813d9.out`: the plants at 7a813d9.
- `probes-third/`: `writer_probe.sh` (R3), `txg_sim.py` (the probe on a simulated hold, and dead), `txg_fail_probe.sh` (R3), `stdout_probe.sh` (R4), `wfcheck.sh` (the workflow's validation), `txgread.py` (an independent reading of a take's `txg.tsv`), and their outputs.

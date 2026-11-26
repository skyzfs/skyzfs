# Review of #128 (M2.T5): upload rows judged only where they uploaded, the seed from the load, the watcher's border

Pull request: skyzfs/skyzfs#128 (`skyzfs-testing` into `zfs-topology`).
**Head reviewed: 0325c40** (one commit on `65dcfd6`). I read the head with
`gh api` when I began and again at the end. The base has moved since:
`zfs-topology` is at 3280276, which adds notes only and no file of this
pull request.

The pull request changes only test code:

- `tests/bench/bracket.py`, with `test_bracket.py`;
- `tests/bench/topology-bench.sh`, with `test_copy_limit.sh` and
  `test_embedded_python.py`;
- `tests/bench/minio.sh`;
- `tests/lib/test_skz_store.sh`.

I read the diff and formed a view from the code before I read the
description. I then checked the description against the code and
against run 44412222348's artifacts and logs.

## Verdict

**No finding blocks the merge.** The review's side is ready: the
behaviour claimed under (1) to (4) holds, and so do the tests behind
it. What remains:

- **CI is green but for one job, red for a reason outside this pull
  request.** The replay rig's "replay, product layout" job had a pool
  export that failed as busy, which is the rig's own error state. That
  job should be rerun. Every other run on the head has ended green,
  among them the daemon rig on five jobs and the cut rig on four. See
  "CI and Codex at 0325c40".
- **Codex's one finding (P2, `bracket.py:94`) needs an answer on its
  thread.** I checked it. It does not arise from rows the benchmark
  writes, because `snapshot` always writes `skz read_at_ns`. Hardening
  is optional. See "Codex's P2".
- **Low defects and test gaps.** None of them changed a figure of run
  44412222348:
  - D1 to D6 below;
  - eleven surviving plants. Four of them (M10 to M13) leave the freeze's
    own condition and its flag untested.
- **Figures in the description, and the notes copied from it.** These
  need a small correction (R1, R2) and one clarification (R3), for the
  lead.

## The four questions

### (1) `bracket.py` `not_uploading`: right

At `bracket.py:85-108`, a row that says it uploads (`skyzfsd.upload`) has
no verdict when any of these holds, checked in this order:

1. `pid_same` is not `True`;
2. the lease is not `held` or `unsure` at both ends, or the two epochs
   differ;
3. `uploaded` is not a number at both ends;
4. `uploaded` did not grow.

This is the lead's decision (c) on G4, as recorded in the notes' review
row for #123.

The idle reason is computed before `enclose` (`:147`). It comes before
"no zfs-slog row ... before it" (`:153`). It also comes before
"other settings", the origin delay and "bracket too wide" (`:157`).
When the row is enclosed, the ratio and width are kept.

**My differential test** (`diff_not_uploading.py`, appendix). It
compares the code with an oracle written from the specification:

- **The table.** A cartesian table of 225,792 pairs of readings: upload
  present or not; `pid_same` True, False, None or missing; the lease
  held, unsure, none, lost, `not_said` or missing at each end; the epoch
  `1`, `2`, empty or missing at each end; `uploaded` `3`, `27`, `0`, `2`,
  empty, `x` or missing at each end.
  - The only disagreements (392) are D1: an epoch unreadable at both
    ends.
  - For every row that is denied, the verdict starts with "NO VERDICT:
    the row says it uploads, but". This holds both enclosed (with a
    bracket 26% wide, to show that the reason comes first) and not
    enclosed (with an origin delay on the one model row). Ratio and width
    are kept when enclosed. 0 order problems.
  - Every row that is judged reads "at least 0.90" at 950 against 1000.
- **Named edges, all as specified except D1 and two type cases:**
  - an epoch change from 1 to 2 while held: denied;
  - `unsure` at the start, at the end or at both ends under one epoch:
    judged;
  - `unsure` with the epoch changed: denied;
  - a commit at the window's end not yet counted (3 and 3): denied as
    "no upload committed";
  - the same commit counted (3 and 4): judged;
  - a commit counted before the window's start (4 and 4): denied;
  - the counter going down: denied;
  - the epoch missing at one end: denied;
  - `uploaded` missing at both ends: "cannot be read";
  - no readings, or readings `None`: denied on the lease;
  - `pid_same` missing or `1`: denied on the pid;
  - no `skyzfsd` part, or an `upload` that is `{}`: not asked.

  The two type cases are `uploaded` given as an int or `None`. Each
  raises `AttributeError`, which is unreachable: the parser stores every
  `skz` value as a string (`topology-bench.sh:2862`).

**A window end equal to the commit time** is decided by the reading
alone: the row is judged exactly when the end reading counted the
commit. That is what "committed in its window" can mean with two
readings.

### (2) `TB_SKZ_UPLOAD_FROM_LOAD`: right, with untested edges (T1)

**The order.**

- The freeze comes after `skz_create` and before `skz_start`
  (`topology-bench.sh:2193-2201`), so the volume is placed in object
  storage before MinIO freezes.
- The daemon then starts with no lease. In the run, the serve took 10 s
  (23:09:44 to 23:09:55): `LEASE_LIMIT`, `upload.rs:99`.
- `run_workload` writes the mark, then calls `skz_minio_resume`, then
  starts the first warm-up (`:2756-2759`). So in a row from the load,
  the commit cannot come before the mark.

**The refusals** (`:1055-1062`), each tested and each killed by a plant
(M6 to M8):

- a value other than 1;
- the option without `TB_SKZ_UPLOAD=1`;
- a wait other than 0, including the default 900.

A wait spelled `00` is also refused: it is compared as a string. That is
harmless.

**Failure paths.**

- **Where MinIO is let go:**
  - by `teardown_topology` (`:1365`), before the export and before
    `skz_stop`, so that the daemon's lease release finds MinIO
    answering;
  - by `minio.sh down` (`minio.sh:63`), which the workflow runs after
    every upload row whatever its exit (`topology-bench.yml:442-451`).
- **What runs the teardown:** `trap teardown_all EXIT`, with `set -e`.
  I checked that bash runs the EXIT trap on SIGTERM, SIGINT and SIGHUP
  (rc 143, 0 and 129, trap ran each time).
- **So MinIO is let go:**
  - on an error between the freeze and the mark;
  - on an exit with a failure (`return 1` in the build, `exit 64` in
    `skz_start`);
  - on those signals.
- **Left frozen until the workflow's `down`:**
  - with `TB_KEEP` set (on purpose: the stack is kept);
  - on SIGKILL.
- **A failed unpause at the mark** keeps the flag. It is tried again at
  the next `run_workload` and at the teardown (plant M5 is killed). The
  row then has no upload in its window, and `not_uploading` gives NO
  VERDICT. The two parts of this package work together here.

### (3) N1, the border between rounds: right, one residue (D2)

Each round takes `rs` before the status read and `now` after it
(`:1949` and `:1952`). A round whose `uploaded` is empty does not move `prev`
(`:1982`). The commit therefore lies in `(prev, now]`. The labels are:

- "under the workload" when `prev ≥ began`;
- "as the workload began (within now−prev s)" when the mark lies inside
  the interval;
- "before the workload began" otherwise.

Nine plants on the border (N1a to N1i) are all killed:

- an unread round moving the lower end;
- the lower end taken at the end of the read before;
- the old rule (`now ≥ began`);
- the middle label dropped;
- `-gt`;
- "within" counted from the mark;
- the lower end not set at the start;
- the range's low end taken from `now`;
- `rs` taken after the read.

Run 44412222348's `fl` row: the mark and the unpause at 23:10:42, the
commit seen at 23:10:58. It reads "11 to 16 s after its warm-up began",
consistent with the job's log.

### (4) N2, `skz_seed_watch_stop`: right

`skz_seed_watch_stop` (`:1333-1339`) writes the NONE line only while the
watcher lives. It always kills and clears the watcher.
`test_copy_limit.sh:290-311` (`seed_stop` at `:293`) runs it against:

- a live `sleep 60`: one line, "42 or 43 s", the process gone, the pid
  cleared;
- a watcher that has ended: nothing written;
- none: nothing written.

The teardown's call is pinned. So is the `rm -f "$SKZ_SEED_BEGAN"`
before the watcher starts (old F5, now killed as W5 and W6). F9 (W8)
still survives: see T2.

## Defects (none blocks the merge)

### D1. An epoch unreadable at both ends counts as one epoch (low)

- **Where:** `bracket.py:99-100`.
- **What happens:** a missing `lease_epoch` becomes the string
  `'not read'`, and `'not read' == 'not read'`. A row held at both ends
  with no epoch at either, and with `uploaded` growing, is judged. The
  docstring and the lead's (c) say "under one epoch".
- **Scenario:** before `{lease: held, uploaded: 3}`, after
  `{lease: held, uploaded: 27}`, `pid_same` True. Result: "at least 0.90"
  at 950 against 1000. An empty epoch at both ends does the same.
- **Certainty:** the behaviour is certain. It is not reachable from a
  real status: `status.rs:1775-1777` prints `lease` and `lease_epoch`
  together, and the row parser keeps no empty value.
- **Fix:** ask that both epochs are digits.

### D2. "Under the workload" at the mark's own second (low)

- **Where:** `topology-bench.sh:1964` uses `prev -ge began`, on times
  truncated to the second.
- **What happens:** with `prev == began`, the read before can have
  started up to a second before the mark. A commit in that second gets
  "under the workload, 0 to N s". The test pins the choice:
  `test_copy_limit.sh:244`, `BEGAN=80`, and plant N1e (`-gt`) is killed.
- **When it matters:** only without `TB_SKZ_UPLOAD_FROM_LOAD`. With it,
  the commit comes after the unpause, which follows the mark. The "0 to"
  printed tells a reader the commit was at the border.
- **Fix:** `-gt`, with `prev == began` going to "as the workload
  began". Or leave it and say so in the comment.

### D3. The option's comment says pgbench's init writes under the first cut (low, text)

- **Where:** `topology-bench.sh:285-287`.
- **What is wrong:** `pgbench -i` runs once, in `prepare_origin`
  (`:1519`), on the ext4 origin, before the daemon exists.
- **What does write under the first cut before MinIO answers:**
  - the image's copy onto the device (`skz_copy`);
  - the import;
  - postgres's start;
  - `pg_test_fsync`.

### D4. The flag is set after the freeze (nit)

- **Where:** `:2195-2196`.
- **What can happen:** a `docker pause` that froze MinIO but returned
  non-zero, or a signal between the two lines, leaves MinIO frozen. The
  teardown does not know, and only the workflow's `down` lets it go.
- **Fix:** setting `SKZ_MINIO_PAUSED=1` before the call costs nothing,
  since the teardown's unpause is `|| true`.

### D5. A comment in the wrong place (nit)

- **Where:** `:1313-1315`.
- **What is wrong:** "The seed's watcher at the teardown. A seed that
  has not committed ..." sits above `skz_minio`. It belongs to
  `skz_seed_watch_stop` (`:1333`).

### D6. Two wordings (nit)

- **The pid reason.** When a reading's pid could not be read, the
  reason says "its daemon was not the same at the window's two ends
  (pid 7 and not read)". It is not known to differ. "Was not shown to
  be the same" would be exact.
- **The summary's paragraph** (`:913-915`) lists three of the four
  reasons. It omits "its uploads cannot be read".

## Test gaps (surviving plants; none blocks)

### T1. The freeze's own condition and flag are untested (low to medium)

The tests are text checks of order. Four plants survive, and each would
show only in a bench run:

| plant | what a bench run would show |
|---|---|
| M10: `SKZ_MINIO_PAUSED=1` dropped | MinIO frozen for the whole row; `skz_minio_resume` does nothing and the teardown does not unpause it. The row has no upload and NO VERDICT; the workflow's `down` lets MinIO go. |
| M11: the freeze on `$SKZ_UPLOAD` | every upload row frozen; a row with a wait over 0 then waits its 900 s for nothing |
| M12: the freeze's condition false | a row from the load is not frozen and runs as `sp` |
| M13: `upload_from_load 1` not written | the row's serve arguments do not say it was frozen |

The fix is a check that the block at `:2193` is guarded by
`SKZ_UPLOAD_FROM_LOAD` and sets the flag after the pause. Better, run
that block with `skz_minio` stubbed, as `minio_resume` already does for
the resume.

The description's plant "MinIO never let go" is caught in the form the
author planted it. M10 is another way to get the same result, and it
survives.

### T2. Smaller gaps

- **W8 (old F9):** `SKZ_SEED_INFO` not set at the build, so the NONE line
  goes to `/dev/null`. The function's test sets the variable itself. My
  N2 suggestion on #123 had the same blind spot.
- **W9:** `SKZ_SEED_T0` not set, so NONE counts from 0. Same cause.
- **W10 (old F4):** the mark rewritten at every warm-up. Harmless, as
  before.
- **M4:** the teardown's unpause moved after `skz_stop`. Harmless: the
  lease release waits `RELEASE_LIMIT` (5 s) and the stop allows 60 s.
- **M16 and M17:** `minio.sh pause|unpause` doing something else, and
  `down` not unpausing. There is no docker in the tests, and run
  44412222348 shows the freeze working: `fl` committed 16 s after the
  unpause, `sp` 15 s after its import.
- **B13:** the summary paragraph's new sentence. It is text.

## Codex's P2 (`bracket.py:94`): not reachable from the bench; answer it on the thread

Codex says that when the window-end status read fails, the row has no
`skyzfsd` part, because the part is built only under
`if skz[base + '.after']:` (`:2907`). The row is then judged as one
that does not upload.

**Codex's case reproduces as stated.** I took an `.after` with no
`skz` line and enclosing model rows, and got "at least 0.90"
(`codex_p2.py`).

**The benchmark cannot write such an `.after` for a daemon row.**
`snapshot` writes `skz read_at_ns ...` before the status read whenever
`SKZ_UP` is set (`:2725`), so `skz[after]` is never empty for a daemon
row. With the `.after` a failed status read really leaves (that line
alone), the part is built and the row reads: "NO VERDICT: the row says
it uploads, but its daemon was not the same at the window's two ends
(pid 7 and not read)".

**What to do.** The author should answer Codex on its thread with that.
An optional hardening would build `k['upload']` from `.skzinfo` even
when the end reading is empty.

## The description's figures, against run 44412222348 (R1 to R3, for the lead)

**What I checked.** The run is on head 0325c40, dispatched on
`skyzfs-testing`, with one job and one machine (boot 815979b0). I
checked against its artifacts (`topology-bench-sequence`,
`topology-bench-summary`) and the job's log. These agree with the
description:

- every tps, bracketed ratio, width and verdict;
- the model rows;
- the first-upload lines;
- uploads in the window and whole uploads: 24/23, 25/23, 0/0, and 0
  whole in every window;
- cuts abandoned and lost: 19/19 and 2/2, 33 by the end;
- `cut_holes` held (0) and dropped (524288);
- 8127449 zero blocks;
- MinIO 22702 MiB with 2835 objects, 7767 MiB and 8243 MiB;
- free disk 81502 MiB, and 95822 to 96298 MiB;
- the uploader's user CPU: 11.6 to 11.7 s, 4.7 to 4.8 s, under 1 s;
- 35.3 against 19.7 to 19.9 KiB per transaction;
- txg sync 1414 ms against 452 to 480 ms;
- the bound, 268435456.

What needs correcting:

- **R1. "128 to 161 MiB at the windows' ends" is in MB, not MiB.**
  - At the windows' ends, `cut_copied_bytes` is 161210368 after tpcb and
    128192512 after select. That is 153.7 and 122.3 MiB.
  - At their starts it was 15.8 and 129.3 MiB.
  - The notes at 3280276 copied the figure as written.
- **R2. The notes' "a seed that starts under a real load lands in about
  52 s"** counts from the import.
  - MinIO was frozen from 23:09:44 to 23:10:42. The import was at
    23:10:06, so 36 of the 52 s were spent frozen.
  - The seed itself took 11 to 16 s from MinIO's answer. The upper bound
    includes any wait the uploader was in when MinIO came back.
- **R3. `fl`'s windows hold no part of the seed.**
  - `uploaded_whole` was already 1 at the tpcb window's start. The seed
    ended within the 30 s warm-up.
  - So `fl`'s ratios (0.832 and 0.855) are figures from after the seed,
    like `sp`'s (0.842 and 0.821).
  - The notes say "the windows after it are as in the other sparse
    rows", which is right. The description should say it too, so that
    nobody reads 0.832 as what a seed costs under load.
  - The option as built shows that a sparse seed lands under load. It
    does not show its cost in tps. A row that measured that would let
    MinIO go at the window's start, after the warm-up, or run with a
    warm-up of 0. That is a suggestion for a later package.

## CI and Codex at 0325c40

I read the runs at 23:56 UTC, and again at 00:22 UTC when the last one
had ended. The head was still 0325c40.

| run | state |
|---|---|
| push: skyzfs-ci 44411672082 | success |
| push: control rig 44411672312 | success |
| pull request: ci 44421456230 | success: lint (the bench's tests and shellcheck), test, unit-arm64, chart-lint, s3-backends |
| coverage, e2e, kernel probe, nodeb rig, cut-daemon, control rig (44421455946, three images) | success |
| **replay rig 44421456349** | **one job failed**: "replay, product layout (local log device)", job 135215364067. "Replay, whole-pool layout" and the four daemon jobs ("through skyzfsd" and "write into a hole", each on both images) passed. |
| daemon rig 44421460750 | success, five jobs. The judge exited 0 in each. On the daemon (ubuntu-24.04, 26.04 and 26.04-arm), its table has 55 PASS, 1 WAITING (`pin_failed_volume_while_a_request_is_held`, "not built") and 2 NA, as `EXPECTED` has them. On the stand-in (24.04 and 26.04) it has 32 PASS. Every FAIL line is a negative control (`ctl_*`). These are my counts of the table's lines in the job logs. |
| cut rig 44421456076 | success, the four "record and check" jobs and the reference |

**The failed replay job.**

- **What happened:** at prefix 2486, `zpool export` answered
  "cannot export 'skzreplay': pool is busy". Every later check then
  failed with "rig: a pool named skzreplay is still imported", and the
  main pass stopped after 25 failures.
- **What failed with it:**
  - the coverage: 19 of 100 flush boundaries were not checked;
  - the three block controls, which read "the rig failed" and DIFFER
    from `EXPECTED`;
  - the job, with rc 1.
- **Whose error it is:** every failure is labelled `rig:`. The rig's own
  unit test pins that a pool left imported is "a rig error, not a
  verdict" (`test_replay.py:747`). The scrub and every-copy checks up to
  that point were all clean, and `zdb -bcc` on the final image exited 0.
- **Why this pull request is not the cause:** the job records the
  reference's pool, not the daemon's. No rig reads a file of this pull
  request. It runs only because `tests/lib/` changed, and the change
  there is `test_skz_store.sh`.
- **The notes:** I found no earlier "pool is busy" in them.
- **What to do:** rerun that job. If it recurs, it is a flake of the
  rig's export for the lead to record.

**Codex** posted one review on 0325c40 with one finding, the P2 above,
at 23:34 UTC. It is not yet answered.

## Local gates at 0325c40

All run on a worktree of the head:

| gate | result |
|---|---|
| shellcheck 0.11.0, every tracked `*.sh` | clean |
| `test_bracket.py` | 19 OK |
| `test_embedded_python.py` | 7 OK |
| `test_skz_time.py`, `test_cut_release.py`, `test_syscalls.py`, `test_perfcat.py` | OK |
| `test_copy_limit.sh` | 92 ok, all passed |
| `tests/lib/test_skz_store.sh`, as root | 104 ok, all passed |

## Plants at 0325c40

`plants128.py` (appendix) applies each plant alone to a worktree of the
head and runs the tests that cover the file:

- for `topology-bench.sh`: `test_copy_limit.sh`,
  `test_embedded_python.py` and `test_skz_store.sh`;
- for `bracket.py`: `test_bracket.py` and `test_embedded_python.py`;
- for `minio.sh`: `test_skz_store.sh`.

Each test runs under a 600 s limit. The file is restored in a `finally`,
and both worktrees were clean at the end.

The result: 51 plants, 40 killed, 11 survived, none not applied.

| plant | result |
|---|---|
| B1 an epoch change taken as held | KILLED (bracket) |
| B2 unsure taken as not held | KILLED (bracket) |
| B3 zero uploads taken as an upload | KILLED (bracket) |
| B4 a pid change ignored | KILLED (bracket) |
| B5 `pid_same` unknown taken as the same | KILLED (bracket) |
| B6 unreadable counters taken as zero | KILLED (bracket) |
| B7 the row's reason after the enclosure's | KILLED (bracket) |
| B8 an enclosed row that did not upload judged | KILLED (bracket) |
| B9 its ratio not kept when it did not upload | KILLED (bracket) |
| B10 a row that does not upload asked | KILLED (bracket) |
| B11 the lease asked at the end only | KILLED (bracket) |
| B12 the lease refused only when not held at both ends | KILLED (bracket) |
| B13 the summary paragraph's new sentence changed | SURVIVED |
| M1 MinIO let go before the mark | KILLED (copy_limit) |
| M2 MinIO let go after the first warm-up | KILLED (copy_limit) |
| M3 no unpause at the teardown | KILLED (copy_limit) |
| M4 the teardown's unpause after the daemon's stop | SURVIVED (harmless) |
| M5 the flag cleared when the unpause fails | KILLED (copy_limit) |
| M6 a wait other than 0 let through | KILLED (skz_store) |
| M7 the option without uploads let through | KILLED (skz_store) |
| M8 `TB_SKZ_UPLOAD_FROM_LOAD=yes` let through | KILLED (skz_store) |
| M9 MinIO never frozen (the pause line removed) | KILLED (copy_limit) |
| M10 the freeze not recorded (never let go until the teardown) | SURVIVED (T1) |
| M11 every upload row frozen | SURVIVED (T1) |
| M12 the freeze's condition false | SURVIVED (T1) |
| M13 the row's record does not say it was frozen | SURVIVED (T1) |
| M14 the resume logs but does not unpause | KILLED (copy_limit) |
| M15 the resume unpauses a MinIO that was not frozen | KILLED (copy_limit) |
| M16 `minio.sh pause` does something else | SURVIVED (T2) |
| M17 `minio.sh down` does not unpause | SURVIVED (T2) |
| N1a an unread round moves the lower end | KILLED (copy_limit) |
| N1b the lower end at the read's end, not its start | KILLED (copy_limit) |
| N1c under when the read that saw it was after the mark (the old rule) | KILLED (copy_limit) |
| N1d no "as the workload began" | KILLED (copy_limit) |
| N1e under only when strictly after the mark | KILLED (copy_limit; pins D2) |
| N1f "within" counted from the mark | KILLED (copy_limit) |
| N1g the lower end not set at the start | KILLED (copy_limit) |
| N1h the range's low end from the read that saw it | KILLED (copy_limit) |
| N1i the read's start taken after the read | KILLED (copy_limit) |
| W1 the NONE line not written | KILLED (copy_limit) |
| W2 NONE written for a watcher that has ended | KILLED (copy_limit) |
| W3 the watcher not stopped | KILLED (copy_limit) |
| W4 the watcher's pid not cleared | KILLED (copy_limit) |
| W5 the last row's mark not removed (old F5) | KILLED (copy_limit) |
| W6 the last row's mark removed after the watcher starts | KILLED (copy_limit) |
| W7 the teardown does not call it | KILLED (copy_limit) |
| W8 the NONE line goes nowhere (old F9) | SURVIVED (T2) |
| W9 the import's time not set | SURVIVED (T2) |
| W10 the mark rewritten at every warm-up (old F4) | SURVIVED (harmless) |
| F1 the mark written after the first warm-up | KILLED (copy_limit) |
| F6 the watcher's first line not written | KILLED (copy_limit) |

## Appendix: the scripts

Each is run with `python3 -I` from a directory of its own, with TREE a
checkout of the head. `codex_p2.py` runs the row's here-document on
files it writes itself.

`diff_not_uploading.py TREE`:

```python
"""diff_not_uploading.py TREE: bracket.py's not_uploading and bracketed against an oracle
written from the specification of review's G4 (c) on #123, over a cartesian table of
status readings at a window's two ends, then named edge scenarios. Prints the
disagreements by class and exits 1 when there is any."""
import itertools
import sys
from collections import Counter

sys.path.insert(0, sys.argv[1] + '/tests/bench')
import bracket as bk  # noqa: E402

MISSING = object()
HELD = ('held', 'unsure')


def oracle(k):
    """-> None (it uploaded, judged) or a class: pid, lease, unread, none."""
    if not (k or {}).get('upload'):
        return None
    b, a = k.get('before') or {}, k.get('after') or {}
    if k.get('pid_same') is not True:
        return 'pid'
    if b.get('lease') not in HELD or a.get('lease') not in HELD:
        return 'lease'
    eb, ea = b.get('lease_epoch'), a.get('lease_epoch')
    # "under one epoch": both epochs read, a number, and the same.
    if not (isinstance(eb, str) and eb.isdigit() and isinstance(ea, str) and ea.isdigit() and int(eb) == int(ea)):
        return 'lease'
    ub, ua = b.get('uploaded'), a.get('uploaded')
    if not (isinstance(ub, str) and ub.isdigit() and isinstance(ua, str) and ua.isdigit()):
        return 'unread'
    if int(ua) <= int(ub):
        return 'none'
    return None


def klass(text):
    if text is None:
        return None
    for prefix, c in (("its daemon was not the same", 'pid'), ('it did not hold the lease', 'lease'),
                      ('its uploads cannot be read', 'unread'), ('no upload committed in its window', 'none')):
        if text.startswith(prefix):
            return c
    return 'OTHER:' + text


def status(lease, epoch, up, pid='4242'):
    s = {'pid': pid}
    for key, v in (('lease', lease), ('lease_epoch', epoch), ('uploaded', up)):
        if v is not MISSING:
            s[key] = v
    return s


def row(seq, topo, tps, **kw):
    r = {'_seq': seq, 'topology': topo, 'tps': tps, 'machine': 'm', 'origin_delay_ms': 0, 'workload': 'tpcb'}
    r.update(kw)
    return r


UP = {'endpoint': 'http://127.0.0.1:9000', 'bucket': 'b', 'volume': 'v'}
LEASES = ['held', 'unsure', 'none', 'lost', 'not_said', MISSING]
EPOCHS = ['1', '2', '', MISSING]
UPS = ['3', '27', '0', '2', '', 'x', MISSING]
PIDS = [True, False, None, MISSING]

dis = Counter()
examples = {}
n = 0
order_bad = []
for upload, ps, lb, la, eb, ea, ub, ua in itertools.product(
        [UP, MISSING], PIDS, LEASES, LEASES, EPOCHS, EPOCHS, UPS, UPS):
    k = {'before': status(lb, eb, ub), 'after': status(la, ea, ua)}
    if upload is not MISSING:
        k['upload'] = upload
    if ps is not MISSING:
        k['pid_same'] = ps
    n += 1
    try:
        got = klass(bk.not_uploading({'skyzfsd': k}))
    except Exception as e:  # noqa: BLE001
        got = f'RAISED {type(e).__name__}'
    want = oracle(k)
    if got != want:
        key = (want, got)
        dis[key] += 1
        examples.setdefault(key, k)
    # The verdict: the row's own reason first, enclosed or not; figures kept when enclosed.
    if want is not None and got == want:
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300)]
        b = bk.bracketed(rows[1], rows)  # also a bracket too wide (26%)
        if not (b['verdict'].startswith('NO VERDICT: the row says it uploads, but ') and b['ratio'] and b['width']):
            order_bad.append(('enclosed', k, b))
        rows2 = [row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1300, origin_delay_ms=10)]
        b2 = bk.bracketed(rows2[0], rows2)
        if not (b2['verdict'].startswith('NO VERDICT: the row says it uploads, but ') and b2['ratio'] is None):
            order_bad.append(('not enclosed', k, b2))
    if want is None and got is None and upload is not MISSING:
        rows = [row(1, 'zfs-slog', 1000), row(2, 'zfs-skyzfsd', 950, skyzfsd=k), row(3, 'zfs-slog', 1000)]
        if bk.bracketed(rows[1], rows)['verdict'] != 'at least 0.90':
            order_bad.append(('judged row not judged', k, None))

print(f'{n} scenarios; disagreements: {sum(dis.values())}')
for (want, got), c in sorted(dis.items(), key=lambda x: -x[1]):
    print(f'  oracle {want!s:7} code {got!s:7} x{c}  e.g. before={examples[(want, got)]["before"]} '
          f'after={examples[(want, got)]["after"]} pid_same={examples[(want, got)].get("pid_same", "MISSING")}')
print(f'verdict-order problems: {len(order_bad)}')
for o in order_bad[:5]:
    print('  ', o)

# Named edges.
base = lambda **kw: {'upload': UP, 'pid_same': True, **kw}
EDGES = [
    ('epoch change 1 -> 2, held both', base(before=status('held', '1', '3'), after=status('held', '2', '27')), 'lease'),
    ('unsure at the start only', base(before=status('unsure', '1', '3'), after=status('held', '1', '27')), None),
    ('unsure at the end only', base(before=status('held', '1', '3'), after=status('unsure', '1', '27')), None),
    ('unsure at both ends', base(before=status('unsure', '1', '3'), after=status('unsure', '1', '27')), None),
    ('unsure, epoch changed', base(before=status('unsure', '1', '3'), after=status('unsure', '2', '27')), 'lease'),
    ('commit at the window end, not yet counted', base(before=status('held', '1', '3'), after=status('held', '1', '3')), 'none'),
    ('commit at the window end, counted', base(before=status('held', '1', '3'), after=status('held', '1', '4')), None),
    ('commit at the window start, counted before', base(before=status('held', '1', '4'), after=status('held', '1', '4')), 'none'),
    ('counter went down (a new daemon, same pid?)', base(before=status('held', '1', '5'), after=status('held', '1', '2')), 'none'),
    ('epoch missing at both ends', base(before=status('held', MISSING, '3'), after=status('held', MISSING, '27')), 'lease'),
    ('epoch empty at both ends', base(before=status('held', '', '3'), after=status('held', '', '27')), 'lease'),
    ('epoch missing at one end', base(before=status('held', '1', '3'), after=status('held', MISSING, '27')), 'lease'),
    ('uploaded missing at both ends', base(before=status('held', '1', MISSING), after=status('held', '1', MISSING)), 'unread'),
    ('no readings at all', base(before={}, after={}), 'lease'),
    ('readings None', {'upload': UP, 'pid_same': True, 'before': None, 'after': None}, 'lease'),
    ('pid_same missing', {'upload': UP, 'before': status('held', '1', '3'), 'after': status('held', '1', '27')}, 'pid'),
    ('pid_same truthy but not True (1)', base(pid_same=1, before=status('held', '1', '3'), after=status('held', '1', '27')), 'pid'),
    ('no skyzfsd part', None, None),
    ('upload {} (falsy)', {'upload': {}, 'pid_same': True, 'before': {}, 'after': {}}, None),
    ('uploaded as an int (JSON number)', base(before={'pid': '1', 'lease': 'held', 'lease_epoch': '1', 'uploaded': 3},
                                              after={'pid': '1', 'lease': 'held', 'lease_epoch': '1', 'uploaded': 27}), 'unread'),
    ('uploaded None', base(before=status('held', '1', None), after=status('held', '1', '27')), 'unread'),
]
bad = 0
for name, k, want in EDGES:
    r = {} if k is None else {'skyzfsd': k}
    try:
        got = klass(bk.not_uploading(r))
    except Exception as e:  # noqa: BLE001
        got = f'RAISED {type(e).__name__}: {e}'
    flag = 'ok ' if got == want else 'DIFF'
    bad += got != want
    print(f'{flag} {name}: expected {want}, got {got}')
sys.exit(1 if dis or order_bad or bad else 0)
```

`codex_p2.py TREE`:

```python
"""codex_p2.py TREE: Codex's P2 on #128 (an upload row whose window-end status was not read).
Runs the row here-document of TREE's topology-bench.sh on (a) an .after with no skz line at
all, Codex's case, and (b) an .after as snapshot() writes it when the status read fails: the
`skz read_at_ns` line alone. Each row is judged by TREE's bracket.py between two model rows."""
import json
import os
import re
import subprocess
import sys
import tempfile

tree = sys.argv[1]
HERE = os.path.join(tree, 'tests/bench')
sys.path.insert(0, HERE)
import bracket as bk  # noqa: E402

SCRIPT = os.path.join(HERE, 'topology-bench.sh')
HEREDOC = re.compile(r"<<\s*'PY'[^\n]*\n(.*?)\n\s*PY\n", re.S)
devs = 'dev pool 0 0 0 0 0 0 0 0\ndev origin 0 0 0 0 0 0 0 0\n'
before = devs + 'skz read_at_ns 1000\nskz pid 7\nskz lease held\nskz lease_epoch 1\nskz uploaded 3\n'
afters = {'no skz line (Codex)': devs, 'read_at_ns alone (a failed status read)': devs + 'skz read_at_ns 2000\n'}
with open(SCRIPT) as f:
    doc = next(x for x in HEREDOC.findall(f.read()) if 'sys.argv[12]' in x)
for name, after in afters.items():
    with tempfile.TemporaryDirectory() as d:
        open(os.path.join(d, 'row.py'), 'w').write(doc)
        base = os.path.join(d, 'r')
        for suf, text in (('.before', before), ('.after', after), ('.pgbench', 'tps = 950 (without initial connection time)\n')):
            open(base + suf, 'w').write(text)
        open(os.path.join(d, 'zfs-skyzfsd.skzinfo'), 'w').write(
            'serve_args \nrelease_every 0\nupload http://127.0.0.1:9000 b v\nbuild abc\n')
        p = subprocess.run(['python3', '-I', os.path.join(d, 'row.py'), base, 'zfs-skyzfsd', 'tpcb', '60', 'none', '0',
                            base + '.nolog', base + '.nofsync', 'x', '8', '0', HERE], capture_output=True, text=True, timeout=60)
        if p.returncode:
            print(name, 'row failed', p.stderr[-500:])
            continue
        r = json.loads(p.stdout)
    r.update({'_seq': 2, 'machine': 'm', 'origin_delay_ms': 0})
    m1 = {'_seq': 1, 'topology': 'zfs-slog', 'tps': 1000, 'machine': 'm', 'origin_delay_ms': 0, 'workload': r['workload']}
    m2 = dict(m1, _seq=3)
    rows = [m1, r, m2]
    for o in rows:
        o.pop('duration_s', None); o.pop('clients', None); o.pop('mem_limit', None)
    print(f"{name}: skyzfsd part {'present' if 'skyzfsd' in r else 'ABSENT'}; verdict: {bk.bracketed(r, rows)['verdict']}")
```

`plants128.py TREE [FIRST [LAST]]` (run here as 0 to 26 and 26 to the end, on two worktrees of the head):

```python
"""plants128.py TREE [FIRST [LAST]]: the plants of the review of #128, one at a time in TREE
(a worktree at the head). A plant of the bench script is followed by test_copy_limit.sh,
test_embedded_python.py and tests/lib/test_skz_store.sh; of bracket.py by test_bracket.py and
test_embedded_python.py; of minio.sh by test_skz_store.sh. KILLED: a test failed. SURVIVED:
none did. NOT APPLIED: the text was not found exactly once. The file is restored after each
plant (try/finally), every test runs under a time limit, and the tree is checked clean at the end."""
import subprocess
import sys

tree = sys.argv[1]
SH, BK, MS = 'tests/bench/topology-bench.sh', 'tests/bench/bracket.py', 'tests/bench/minio.sh'
T_COPY = ('copy_limit', ['bash', 'tests/bench/test_copy_limit.sh'])
T_EMB = ('embedded', ['python3', 'tests/bench/test_embedded_python.py'])
T_STORE = ('skz_store', ['bash', 'tests/lib/test_skz_store.sh'])
T_BRK = ('bracket', ['python3', 'tests/bench/test_bracket.py'])
TESTS = {SH: [T_COPY, T_EMB, T_STORE], BK: [T_BRK, T_EMB], MS: [T_STORE]}


def run(cmd):
    try:
        return subprocess.run(cmd, cwd=tree, capture_output=True, text=True, timeout=600).returncode
    except subprocess.TimeoutExpired:
        return 'TIMEOUT'


MARK = '    if [ -n "$SKZ_SEED_WATCH" ] && [ ! -e "$SKZ_SEED_BEGAN" ]; then date +%s > "$SKZ_SEED_BEGAN"; fi\n'
RESUME = '    skz_minio_resume\n'
WARM = ('    log "$tag: warm-up ${WARMUP}s"\n'
        '    as_pg "\'$PGBENCH\' -h \'$SOCK\' -T $WARMUP -c $CLIENTS -j $JOBS -b $builtin bench" >/dev/null 2>&1\n')
UNDER = 'if [[ $began =~ ^[0-9]+$ ]] && [ "$prev" -ge "$began" ]; then'
ASBEGAN = ('                elif [[ $began =~ ^[0-9]+$ ]] && [ "$now" -ge "$began" ]; then\n'
           '                    how=" from the import, as the workload began (within $(( now - prev )) s of its warm-up)"\n')
TEARDOWN_UNPAUSE = '    if [ -n "$SKZ_MINIO_PAUSED" ]; then skz_minio unpause || true; SKZ_MINIO_PAUSED=""; fi\n'
WATCH = ('        rm -f "$SKZ_SEED_BEGAN"\n'
         '        SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"\n'
         '        ( SKZ_UPLOAD_WAIT_S=3600; skz_first_upload watch ) &\n'
         '        SKZ_SEED_WATCH=$!\n')
P = [
    # (1) bracket.py: not_uploading and the verdict's order
    ('B1 an epoch change taken as held', BK, ' or lease[0][1] != lease[1][1]', ''),
    ('B2 unsure taken as not held', BK, "LEASE_HELD = ('held', 'unsure')", "LEASE_HELD = ('held',)"),
    ('B3 zero uploads taken as an upload', BK, 'if int(a) - int(b) <= 0:', 'if int(a) - int(b) < 0:'),
    ('B4 a pid change ignored', BK, "if k.get('pid_same') is not True:", 'if False:'),
    ('B5 pid_same unknown (None) taken as the same', BK, "if k.get('pid_same') is not True:",
     "if k.get('pid_same') is False:"),
    ('B6 unreadable counters taken as zero', BK, "a, b = sa.get('uploaded', ''), sb.get('uploaded', '')",
     "a, b = sa.get('uploaded') or '0', sb.get('uploaded') or '0'"),
    ('B7 the row\'s reason after the enclosure\'s', BK,
     "out['verdict'] = idle or f'NO VERDICT: no {model}", "out['verdict'] = f'NO VERDICT: no {model}"),
    ('B8 an enclosed row that did not upload judged', BK,
     "    if idle:\n        out['verdict'] = idle\n        return out\n", ''),
    ('B9 its ratio not kept when it did not upload', BK,
     "    before, after = enclose(row, rows, model)\n",
     "    if idle:\n        out['verdict'] = idle\n        return out\n    before, after = enclose(row, rows, model)\n"),
    ('B10 a row that does not upload asked', BK, "    if not k.get('upload'):\n        return None\n", ''),
    ('B11 the lease asked at the end only', BK, 'for s in (sb, sa)]', 'for s in (sa, sa)]'),
    ('B12 the lease refused only when not held at both ends', BK,
     'if any(state not in LEASE_HELD', 'if all(state not in LEASE_HELD'),
    ('B13 the summary paragraph says nothing of upload rows', SH,
     "the machine moved by more than is judged while it ran. A row that says it uploads (TB_SKZ_UPLOAD) keeps its '",
     "the machine moved by more than is judged while it ran. A row that says it uploads (TB_SKZ_UPLOAD) has its '"),
    # (2) TB_SKZ_UPLOAD_FROM_LOAD
    ('M1 MinIO let go before the mark', SH, MARK + RESUME, RESUME + MARK),
    ('M2 MinIO let go after the first warm-up', SH, RESUME + WARM, WARM + RESUME),
    ('M3 no unpause at the teardown', SH, TEARDOWN_UNPAUSE, ''),
    ('M4 the teardown\'s unpause after the daemon\'s stop', SH, TEARDOWN_UNPAUSE + '    # A trace',
     '    # A trace'),  # second half below
    ('M5 the flag cleared when the unpause fails', SH,
     '        log "WARNING: MinIO could not be let go as the workload began; the seed waits"\n',
     '        SKZ_MINIO_PAUSED=""; log "WARNING: MinIO could not be let go as the workload began; the seed waits"\n'),
    ('M6 a wait other than 0 let through', SH,
     '    [ "$SKZ_UPLOAD_WAIT_S" = 0 ] || { echo "TB_SKZ_UPLOAD_FROM_LOAD=1 needs TB_SKZ_UPLOAD_WAIT_S=0, not $SKZ_UPLOAD_WAIT_S" >&2; exit 64; }\n', ''),
    ('M7 the seed from the load without uploads let through', SH,
     '    [ "$SKZ_UPLOAD" = 1 ] || { echo "TB_SKZ_UPLOAD_FROM_LOAD=1 needs TB_SKZ_UPLOAD=1" >&2; exit 64; }\n', ''),
    ('M8 TB_SKZ_UPLOAD_FROM_LOAD=yes let through', SH,
     '    [ "$SKZ_UPLOAD_FROM_LOAD" = 1 ] || { echo "TB_SKZ_UPLOAD_FROM_LOAD is 1 or unset, not: $SKZ_UPLOAD_FROM_LOAD" >&2; exit 64; }\n', ''),
    ('M9 MinIO never frozen', SH,
     '                skz_minio pause || { echo "MinIO could not be frozen (TB_SKZ_UPLOAD_FROM_LOAD)" >&2; return 1; }\n', ''),
    ('M10 the freeze not recorded (MinIO never let go until the teardown)', SH,
     '                SKZ_MINIO_PAUSED=1\n', ''),
    ('M11 every upload row frozen (the freeze on TB_SKZ_UPLOAD)', SH,
     '            if [ -n "$SKZ_UPLOAD_FROM_LOAD" ]; then\n                # Frozen until',
     '            if [ -n "$SKZ_UPLOAD" ]; then\n                # Frozen until'),
    ('M12 never frozen (the freeze\'s condition false)', SH,
     '            if [ -n "$SKZ_UPLOAD_FROM_LOAD" ]; then\n                # Frozen until',
     '            if false; then\n                # Frozen until'),
    ('M13 the row\'s record does not say it was frozen', SH,
     '                echo "upload_from_load 1" >> "$OUT/$TOPO.skzinfo"\n', ''),
    ('M14 the resume logs but does not unpause', SH,
     '    if skz_minio unpause; then\n        SKZ_MINIO_PAUSED=""\n        log "skyzfsd: MinIO answers',
     '    if true; then\n        SKZ_MINIO_PAUSED=""\n        log "skyzfsd: MinIO answers'),
    ('M15 the resume unpauses a MinIO that was not frozen', SH,
     '    [ -n "$SKZ_MINIO_PAUSED" ] || return 0\n    if skz_minio unpause',
     '    if skz_minio unpause'),
    ('M16 minio.sh pause does something else (stop)', MS,
     '    pause | unpause) docker "$1" minio > /dev/null ;;', '    pause | unpause) docker stop minio > /dev/null ;;'),
    ('M17 minio.sh down does not unpause', MS, '    docker unpause minio > /dev/null 2>&1 || true\n', ''),
    # (3) N1: the border between the rounds
    ('N1a an unread round moves the lower end', SH, '        [ -z "$up" ] || prev=$rs\n', '        prev=$rs\n'),
    ('N1b the lower end at the read\'s end, not its start', SH, '        [ -z "$up" ] || prev=$rs\n',
     '        [ -z "$up" ] || prev=$now\n'),
    ('N1c under the workload when the read that saw it was after the mark (the old rule)', SH, UNDER,
     'if [[ $began =~ ^[0-9]+$ ]] && [ "$now" -ge "$began" ]; then'),
    ('N1d no "as the workload began": before', SH, ASBEGAN, ''),
    ('N1e under only when the read before was strictly after the mark', SH, UNDER,
     'if [[ $began =~ ^[0-9]+$ ]] && [ "$prev" -gt "$began" ]; then'),
    ('N1f within N counted from the mark', SH, '(within $(( now - prev )) s of its warm-up)',
     '(within $(( now - began )) s of its warm-up)'),
    ('N1g the lower end not set at the start', SH, '    t0=$(date +%s); prev=$t0\n', '    t0=$(date +%s); prev=0\n'),
    ('N1h the range\'s low end from the read that saw it', SH, '$(( prev - began )) to $(( now - began ))',
     '$(( now - began )) to $(( now - began ))'),
    ('N1i the read\'s start taken after the read', SH, '        rs=$(date +%s)\n        all=$(skz_status)\n',
     '        all=$(skz_status)\n        rs=$(date +%s)\n'),
    # (4) N2: skz_seed_watch_stop and the mark
    ('W1 the NONE line not written', SH,
     '        echo "first_upload NONE by the end of the rows ($(( $(date +%s) - SKZ_SEED_T0 )) s after the import)" >> "$SKZ_SEED_INFO"\n',
     '        :\n'),
    ('W2 NONE written for a watcher that has ended', SH, '    if kill -0 "$SKZ_SEED_WATCH" 2>/dev/null; then\n',
     '    if true; then\n'),
    ('W3 the watcher not stopped', SH, '    kill "$SKZ_SEED_WATCH" 2>/dev/null; SKZ_SEED_WATCH=""\n',
     '    SKZ_SEED_WATCH=""\n'),
    ('W4 the watcher\'s pid not cleared', SH, '    kill "$SKZ_SEED_WATCH" 2>/dev/null; SKZ_SEED_WATCH=""\n',
     '    kill "$SKZ_SEED_WATCH" 2>/dev/null\n'),
    ('W5 the last row\'s mark not removed (F5)', SH, WATCH, WATCH.replace('        rm -f "$SKZ_SEED_BEGAN"\n', '')),
    ('W6 the last row\'s mark removed after the watcher starts', SH, WATCH,
     WATCH.replace('        rm -f "$SKZ_SEED_BEGAN"\n', '') + '        rm -f "$SKZ_SEED_BEGAN"\n'),
    ('W7 the teardown does not call it', SH, '    skz_seed_watch_stop\n    if [ -n "$SKZ_MINIO_PAUSED" ]',
     '    if [ -n "$SKZ_MINIO_PAUSED" ]'),
    ('W8 the NONE line goes nowhere: no file set (F9)', SH,
     'SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"', 'SKZ_SEED_T0=$(date +%s)'),
    ('W9 the import\'s time not set: NONE counts from 0', SH,
     'SKZ_SEED_T0=$(date +%s); SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"', 'SKZ_SEED_INFO="$OUT/$TOPO.skzinfo"'),
    ('W10 the mark rewritten at every warm-up (F4)', SH, MARK,
     '    if [ -n "$SKZ_SEED_WATCH" ]; then date +%s > "$SKZ_SEED_BEGAN"; fi\n'),
    ('F1 the mark written after the first warm-up', SH, MARK + RESUME + WARM, RESUME + WARM + MARK),
    ('F6 the watcher\'s first line not written', SH,
     'echo "first_upload not committed by the window\'s end (watched from the import)" >> "$OUT/$TOPO.skzinfo"', ':'),
]
first = int(sys.argv[2]) if len(sys.argv) > 2 else 0
last = int(sys.argv[3]) if len(sys.argv) > 3 else len(P)
todo = P[first:last]
base = {k: run(c) for f in {p for _, p, _, _ in todo} for k, c in TESTS[f]}
assert all(v == 0 for v in base.values()), base
for name, path, old, new in todo:
    full = f'{tree}/{path}'
    text = open(full).read()
    if text.count(old) != 1:
        print(f'{name}: NOT APPLIED ({text.count(old)} matches)', flush=True)
        continue
    planted = text.replace(old, new)
    if name.startswith('M4 '):
        # The unpause moved to after the daemon's stop (and the ublk device's removal).
        anchor = '    for kv in $ZFS_PARAMS_SAVED; do\n'
        assert planted.count(anchor) == 1
        planted = planted.replace(anchor, TEARDOWN_UNPAUSE + anchor)
    try:
        open(full, 'w').write(planted)
        r = {k: run(c) for k, c in TESTS[path]}
    finally:
        open(full, 'w').write(text)
    died = [f'{k}({v})' if v == 'TIMEOUT' else k for k, v in r.items() if v != 0]
    print(f"{name}: {'KILLED by ' + ', '.join(died) if died else 'SURVIVED'}", flush=True)
st = subprocess.run(['git', 'status', '--porcelain'], cwd=tree, capture_output=True, text=True).stdout
print('tree clean' if not st.strip() else f'TREE NOT CLEAN:\n{st}', flush=True)
```

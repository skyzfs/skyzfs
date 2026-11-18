# Next look at #123: ab7fbce (the cut rig for M2.4e) and 72aaa06 (the answers)

Pull request: skyzfs/skyzfs#123 (`skyzfs-testing` into `zfs-topology`).
**Head reviewed: 72aaa06.** The first review (`review.md` here) was at
405b7ed. Since then:

- 0b43679: a watcher for the seed under the workload, and the v17 columns
  (zero blocks, holes);
- ab7fbce: `bound` on a `--zero-fill` volume, and the new course
  `boundsparse`;
- f80dc48: the answers to the first review;
- 72aaa06: a test of the wait ending on a status it cannot read.

I read `git diff 405b7ed 72aaa06`, the answers on the pull request
(issue comment 6989324037), and these runs:

- 44378510338: the cut rig by dispatch at ab7fbce with `daemon_ref` =
  #125's head 0f56669. Green on both images (ubuntu-26.04 and
  ubuntu-26.04-arm), all 15 recordings "checked, exit 0".
- 44371078092 (#125 at 0f56669, before ab7fbce) and 44340268684 (the
  base's daemon): `bound`'s status files, for comparison.
- 44372469282: the bench at 0b43679 on #125's daemon, with
  `TB_SKZ_UPLOAD_WAIT_S=0` rows (the sequence job, green).

The pull request's own cut rig runs at ab7fbce and f80dc48 were cancelled
by the newer pushes. CI and the rigs at 72aaa06 are read in the last
section.

Local gates at 72aaa06, all passing:

- `test_copy_limit.sh`;
- `test_embedded_python.py`, 7 tests;
- `tests/lib/test_skz_store.sh`;
- `test_cut_daemon_check.py`, 100 tests;
- `test_cut_record.sh`;
- shellcheck 0.11.0, clean on every tracked `*.sh`.

## Verdict

**Not yet.** F1, F2 and F3 are settled: I checked each fix against the
code, reran the loop simulation, the refusal probe and the plants (below).
G2, G3 and G5 to G8 are handled as the answers say. G4 is with the lead.

Two new findings block the merge. Both are of F2's kind, a line that says
what was not measured, and each has a fix of a few lines:

- **F4.** `boundsparse`'s "and nothing copied" is read when no cut can be
  held, so it cannot fail. While the first cut was held, the daemon
  must have copied about 0.75 to 1 MiB.
- **F5.** "after N s under the workload" was not true in any row of the
  only run that used it: in 44372469282 every seed committed before
  pgbench's warm-up began.

On the three questions about ab7fbce:

1. **The version gate.** Gating on `cut_holes` works today, because the key
   comes with status version 17. It should gate on the version itself
   (G11). What the gate guards is coarser than the line says (F4): only
   "no cut abandoned" can fail.
2. **`bound` on a zero-filled volume** tests what it tested before: a cut
   abandoned at the bound. Shown on #125's daemon in 44378510338.
3. **The v17 columns** are right for v15, v16 and v17 statuses and are
   pinned by the tests. The watcher has F5, and G10: a seed that has not
   landed by a window's end reads "not waited for".

## The first review's findings at 72aaa06

### F1: fixed

`topology-bench.yml:445` makes the row's directory before the `tee`. I
reran `loop_sim.sh` at 72aaa06 (`loop_sim-72a.out`):

- **A refused upload row (row 2 of four):** all four rows run. The
  summary has "Sequence 2 (up1) failed with 64 (its settings were
  refused ...)" and both MinIO lines, and the step ends 1, as a sequence
  with a failed row does.
- **Control, no refusal:** step 0, four rows, two MinIO lines.
- **Control, a refused row without uploads:** as before.

Codex's thread on this has its answer (comment 4619594584). The answers
say it is resolved; I could not see a thread's state through REST.

### F2: fixed

- **The column** is "abandoned (at a mismatch)": `cuts_abandoned`'s change,
  with `upload_abandoned_at_mismatch`'s change in brackets
  (`topology-bench.sh:714`).
- **The text** says the first counts every abandoned cut: at the bound, at
  a copy or read that failed, or at a mismatch.
- **The balance** now reads "uploaded + found committed − committed and
  gone − committed and kept + replaced + abandoned + the change in held",
  which is status.rs's balance term for term.
- **"held start → end"** (`:717`) lets the change in held be read from the
  table. 44358962767's "cuts 24, uploaded 25" now balances on the page.

One gap remains: the table has no columns for found committed, gone and
kept, so the balance cannot be read off it when those are not 0. They
were 0 in every window so far; a note is enough.

### F3: fixed, with one gap

`tests/lib/test_skz_store.sh:123-158` runs each refusal and checks exit 64
and the reason. Covered:

- no daemon;
- the daemon beside another topology;
- each of the four settings left empty;
- the releaser;
- copies repeated;
- `TB_SKZ_UPLOAD=yes` and `=0`;
- a wait of `abc`.

Plants R1 to R8 (each refusal disabled, a setting left out of the list,
the refusal exiting 0) are each killed by `skz_store` (`plants3.out`).
My probe (`refusals.sh` at f80dc48, `refusals-f80.out`) gives the same
lines as at 405b7ed, except that copies repeated are now refused with
their own reason.

**The gap: the last check is not shown to let a legitimate row through.**
"uploads with every setting: past the checks" runs `zfs-skyzfsd
no-such-topology`, and the loop's "unknown topology" ends it before the
check after the loop (`:1127`). That check is the one that refuses
anything but zfs-skyzfsd alone. So a check that refused every upload
row, the daemon alone included, passes every test (plant R9, SURVIVED).
On a runner it would refuse every upload row with "is for zfs-skyzfsd
only".

At 72aaa06 the check does let a legitimate row through (`refusals2.sh`):

- zfs-skyzfsd alone, with or without spaces around it, gets past every
  check and stops at the made-up machine's space check;
- "zfs-slog zfs-skyzfsd" and "zfs-skyzfsd zfs-skyzfsd" are refused;
- the mixed row without uploads is not refused.

Fix: one more case in `test_skz_store.sh`, with `TB_TOPOLOGIES=zfs-skyzfsd`
alone, that asserts the log does not say "is for zfs-skyzfsd only". It
does not block.

### G2, G3, G5, G6, G7, G8: handled

- **G2.** The intent map's table leaves out `intent_state off` (`:728`). The
  test's "nomap" case is a v16 status with the map off and zeros. Plant
  I9 (the filter dropped) is killed.
- **G3.** Most counters now differ at the two ends. Killed now: T1, T3
  and T4, I1 to I3, I8 (a header-sync count of 0 reads n/a, the "nohdr"
  case), T12 (held at the start) and T13 (no mark for two daemons). Four
  plants survive:
  - U1 (the keys not handed to the daemon) and U4 (no `upload` line in
    `.skzinfo`), as expected. A run shows both at once (the create
    fails, or the table is empty), so they need no test.
  - T2 (mismatches read at the end) and T11 (the bracket, abandoned at a
    mismatch, read at the end). In the fixture both counters go from 0
    to 1 (`test_embedded_python.py:66-72`), so each one's change equals
    its end value. The test's docstring says each counter differs at
    the two ends "so that a figure read at one end shows". That holds
    for the others but not for these two. A start of 1 for each (end 2
    and the rest unchanged) would kill both. Minor.
- **G5.**
  - The input's doc says the wait is per row and against the 120-minute
    limit.
  - Three rounds without a status end the wait, and the row says why
    (`:1882`).
  - The 72aaa06 test covers three rounds (idle and under the workload) and
    two (the wait goes on). Plants W7 (the count never ends the wait) and
    W8 (one round ends it) are both killed.
  - The rule fits the daemon: `uploaded` is in every status of version 15
    or later whether or not it uploads. So an empty value means the status
    was not read, not that the key is missing.
- **G6.** Both tables mark "(TWO DAEMONS in the window)" when the pid differs
  between the two reads (`:689-690`). A daemon that died, with its status
  unread at the end, is marked the same way. The comment says "died or
  changed", so that is fine.
- **G7.** An upload entry must have zfs-skyzfsd as its only topology
  (`read -ra` and `[ "${_topos[*]}" != zfs-skyzfsd ]`, `:1125-1128`).
  Copies repeated with uploads are refused (`:1117`). Both are tested.
- **G8.** `minio.sh down` deletes only a directory whose basename is
  `minio-data` (`minio.sh:56`, exit 64 otherwise). This is checked before
  docker or sudo is called, and is tested, including a trailing slash. The
  workflow's directory is `$RUNNER_TEMP/minio-data` (`topology-bench.yml:305`),
  so the real call passes. If the guard ever refused under the step's
  `bash -e`, the step would end. That is right for a misconfiguration.
  Not done, as the answer says: the restart before the hang check, and
  binding to 127.0.0.1. Both were suggestions.

## New at ab7fbce and 0b43679

### F4. boundsparse's "and nothing copied" cannot fail: the status is read after the holder let every cut go (high certainty; blocks)

`cut_daemon_check.py:1526` requires `cut_copied_bytes == '0'` in
`status.txt`. That status is read in `end_daemon`
(`cut-record.sh:482-505`) only after:

1. the holder was told to stop (`hold.stop`);
2. it wrote and released every cut still held;
3. it exited.

`cut_copied_bytes` is "the bytes the copies of held cuts take now"
(status.rs, offset 328). With no cut held it is 0 for any daemon. The
real runs show it:

| run | daemon | recording | abandoned | `cuts_held` | `cut_copied_bytes` |
|---|---|---|---|---|---|
| 44340268684 | base | `bound` | 1 | 0 | 0 |
| 44371078092 | #125 | `bound`, sparse | 0 (its FAIL) | 0 | 0 |
| 44378510338, both images | #125 | `bound`, zero-filled | 1 | 0 | 0 |
| 44378510338, both images | #125 | `boundsparse` | 0 | 0 | 0 |

In 44340268684 the judge's line reads "'1' abandoned at the bound, with
the copies '0' bytes". In 44378510338, `status-held.txt` (the holder's
stop, before its drain) also reads 0 copies in both recordings.

So the half of the line that says "and nothing copied" checks nothing.
The unit test's FAIL at `cut_copied_bytes='4096'`
(`test_cut_daemon_check.py:751`) pins a status that the rig cannot
produce. Plant K1 (the copies not asked) is killed, but only by that
fixture.

It is also not what a daemon that keeps holes does while the cut is
held. Consider the blocks written before the first cut's instant and
written again while it was held. I took the cut's time from the
request log's `cut` line and the release from the holder's `release 1`;
`hold_copies.py` counts the chunks and blocks:

- **44378510338, `boundsparse`, on #125's daemon:** 192 such blocks
  (786432 bytes) on amd64, 188 (770048 bytes) on arm64. 205 and 207
  blocks of the hold lie in a 64 KiB chunk written before the cut.
- **44371078092, `bound` sparse, on #125's daemon:** 252 such blocks
  (1032192 bytes).

They are the labels, uberblocks and pool metadata. Their content at the
instant is data, not a hole, so every daemon must copy them. A read
while the cut is held would show about 0.75 to 1 MiB of copies, not 0.

What the line does check is `cuts_abandoned == 0`: the copies stayed
under the bound, 256 MiB of the 320 MiB written. A daemon that keeps
the holes only in part passes. An example is one that copies half the
writes into holes, say with a hole set taken at the wrong granularity.
So "pinned, so that a change shows" holds only for the whole effect.

**Least fix, enough for the merge:** drop the copies criterion. Say what is
judged: "none abandoned: the copies stayed under the bound". Make the
test's status the one the rig reads (G12).

**Better fix:** add a status read in `rec_bound` just before
`rm -f "$WORK/hold.pause"` (`cut-record.sh:783`), with the first cut
still held, and judge the copies there:

- in `boundsparse`, well under the writes (for example under 16 MiB),
  not 0;
- in `bound`, at the bound, or a cut abandoned.

It blocks for the same reason F2 did. The line reports PASS for a
property it does not measure, and #125's verdict will quote it.

### F5. "after N s under the workload" was not under the workload in any row that used it (high certainty; blocks)

With `TB_SKZ_UPLOAD_WAIT_S=0` the watcher starts after the import and a
`zpool sync` (`topology-bench.sh:2154-2157`). It counts from there and
writes "first_upload after N s under the workload". Before pgbench runs,
the row still adds the log device, starts postgres and runs the fsync
test.

In 44372469282 (#125's daemon), each of the three rows with a wait of 0
committed its seed before its warm-up began. The job log's times:

| row | seed committed | warm-up began | the row says |
|---|---|---|---|
| on1 | 18:57:01 | 18:57:18 | "after 22 s under the workload (uploaded 2, ...)" |
| off1 | 19:03:25 | 19:03:41 | "after 22 s under the workload" |
| perf | 19:22:30 | 19:22:46 | "after 21 s under the workload" |

The idle row (wait 600) took 16 s. With M2.4e a mostly empty 32 GiB
volume seeds in about 20 s (`holes dropped (524288)`). So the seed now
lands during the setup, and this round did not put the seed under load
at all, though its rows say it did. The windows then measured uploads
of steady-state cuts, as the idle row did.

This is a measurement the lead will write into the notes ("the seed
under the workload"). The lead should hear it before reading 44372469282's
figures.

Fix:

- The watcher records the time of the commit.
- `run_workload` records when the warm-up began.
- The row says "before the workload (N s after the import)" or "under the
  workload (M s into the warm-up or window)".
- Or the label drops "under the workload" and says "while the row was
  set up and ran".

A test of the label against the two times.

To measure the seed under load at all, the seed has to start after
pgbench starts. That is a question of design for the lead, not a fix
here.

### G10. A seed that has not landed by a window's end reads "not waited for", and the watcher's NONE line cannot be written (high certainty; medium)

Each row reads `.skzinfo` when it is written, at its window's end
(`:2735`, `:2831-2835`). The teardown kills the watcher (`:1309`) and
writes nothing. This matters with the base's daemon. Under load its seed
never landed in any window of 44325181328: each whole cut was abandoned
at the bound.

- A row whose window ended before the seed landed shows the column's
  default, "not waited for" (`:719`). In a row with a wait of 0, that now
  means "the seed had not landed when this window ended".
- A seed that never lands while the topology runs leaves no line at all.
  The NONE line needs 3600 s, and a topology's rows take about six and
  a half minutes (44372469282: 18:56:15 to 19:02:39).
- The table's text (`:699`) still calls the column "the first upload
  before the windows".

Fix:

- The watcher writes a line when it starts, for example
  "first_upload not yet (watched since the import)". The row keeps the last
  value of a key (`:2835`), so a later line replaces it.
- The teardown writes "first_upload NONE by the end of the rows (N s)"
  before it kills the watcher.
- The text says what the column holds with a wait of 0.

Also minor:

- The watcher reads the status three times every 5 s during the windows,
  about ten processes per read through `lim`. Reading once per round
  would cut that to a third. The model rows have no such reader.
- Plants S2 (the watcher starts without the sync) and S5 (a watcher on
  every daemon row, uploads or not) survive. The first changes when cuts
  come, and the second only costs status reads, so neither needs a test.

### G11. Gate on the status version, and say why a FAIL failed (judgment; low)

Asked: is "`cut_holes` present" the right test of "the daemon keeps
holes"?

- **A daemon with the key and without the behaviour** does not pass as INFO.
  With the key, the line is judged. Say its hole set is empty
  (`cut_holes none`: the file system could not say where the holes are,
  or a regression that takes none). Then it copies every write, abandons
  at the bound and fails, which is right on the runners. On #125's
  daemon, `status-start-1.txt` reads `cut_holes held` with 8192 chunks for
  `boundsparse` (the whole 512 MiB) and `held` with 0 for `bound`. But
  the FAIL would say only "'1' abandoned". If the judge read
  `status-start-1.txt` it could say "the daemon took no holes on this
  volume". `bound` could check its premise the same way, 0 chunks at the
  start where the key exists. Otherwise a zero fill that did not happen
  shows only as "no cut abandoned".
- **A daemon without the key** is asked nothing. Today that is every daemon
  before M2.4e, rightly. But #125 merges after this pull request and is
  still in review. If a review there renames `cut_holes`, or prints it only
  while the set holds, `boundsparse` turns to INFO "before M2.4e"
  silently. Plant K4 shows the tests would not notice a narrower gate
  either. Fix: gate on `status_version >= 17` (it is in the judge's
  dict), and treat a missing `cut_holes` there as the rig's error.

### G12. The unit test's status is not the one the rig reads

`test_boundsparse_must_copy_no_hole` judges `cut_holes held` with 6000
chunks. The rig reads its status after the first cut was let go. That
status says `dropped` with the chunk count kept: in 44378510338,
`dropped` with 8192 chunks. The copies case (4096) cannot occur (F4). So a
judge that asked for `held`, INFO otherwise (plant K3), passes every test
and would turn every real run of #125 into INFO. Use the status the rig
reads (`dropped`, 8192, copies 0) and add a `none` case. The bench test's
"dropped (0)" is not what a daemon prints either, since a dropped set
keeps its count (44372469282: `dropped (524288)`). That one is harmless.

### Judgment: `bound` on a zero-filled volume still tests a cut abandoned at the bound

- **How the zero fill is written.** `--zero-fill` writes the image with zeros,
  1 MiB at a time, then syncs (`crates/skyzfs-local/src/inplace/start.rs`,
  `make`, and `store::write_zeros`). So the image is data to `SEEK_DATA`, not
  unwritten extents, and #125's `data_from` finds no hole. It is 512 MiB
  (`CUT_VDEV_MB`), quick within the create's 90 s.
- **On a daemon before M2.4e.** Such a daemon does not look at holes. It
  copies a block's old content at its first write under the held cut,
  whatever the content. The zero fill changes nothing for it, and the
  320 MiB written in the hold pass the 256 MiB bound as before
  (44340268684: 1 abandoned).
- **On #125's daemon.** The zero-filled image has no hole chunk
  (`status-start-1.txt`: `held`, 0 chunks), so every write under the first
  cut is copied, as before M2.4e. 44378510338, both images: `bound` PASS,
  "'1' abandoned at the bound"; `boundsparse` PASS with 0 abandoned.
- **No leak between courses.** Each recording runs in its own subshell
  (`cut-record.sh:1366-1372`), so `bound`'s `CREATE_ARGS` cannot reach
  `boundsparse`, which runs next. `create-args.txt` records it
  (`--zero-fill` and empty in 44378510338). Plants K5 to K8 are each
  killed.

### Asked by the lead: does this rig turn #125's failing run green, and still test an abandonment at the bound?

Yes. #125's run 44371078092 (at 0f56669) failed on both held images
with one line each:

```
bound: cuts_abandoned FAIL '0' abandoned at the bound
```

Every other recording checked exit 0. The volume was sparse, and
`status-start-1.txt` read `cut_holes held`, 8192 chunks: the whole
image was a hole. So M2.4e copied nothing for the 320 MiB written into
holes, and the bound was never reached. The premise of `rec_bound` no
longer held, as the commit message of ab7fbce says.

The dispatch 44378510338 runs this pull request's rig (ab7fbce) on the
same daemon (`daemon_ref` 0f56669). It is green on both images, 15 of 15
recordings exit 0:

- **`bound`, `--zero-fill`:** "'1' abandoned at the bound", PASS. The
  abandonment path is still driven on M2.4e's daemon. `status-start-1`
  reads `held`, 0 chunks: no holes, so the copies reach the bound.
- **`boundsparse`, sparse:** "'0' abandoned", PASS. The image's holes
  are `held`, 8192 chunks at the start. Its "and nothing copied" is F4.

The rig's files are the same at 72aaa06 as at ab7fbce: f80dc48 and
72aaa06 touch nothing under `tests/kernel/` nor the cut rig's workflow.
So the rerun of #125 on the merge should give the same, provided that:

- #125's head does not move in a way that changes `bound`;
- F4's fix does not change what passes. Its least form only removes a
  criterion that is always met.

The dispatch had no upload job (`ubuntu-26.04-upload`). That job does
not run `bound`, and passed in 44371078092.

### Judgment: the v17 columns

- **Zero blocks** is a change over the window (`d()`), n/a when either end
  lacks the key. So it is n/a for v16 and v15 daemons: `scenarios.py` at
  ab7fbce, every row v16, gives `n/a | n/a (n/a)`. It is counted per
  instance, like the uploader's other figures. The text says the blocks
  are not in "blocks", which is what status.rs says.
- **Holes** is a gauge at the end. `cut_hole_chunks` keeps its count after
  the set is dropped.
- **Pinned:** V1 (the end value), V2 (from the start) and V3 (chunks as a
  change) are each killed. In 44372469282 the zero blocks were 0 in
  every window. The seed had committed before each window (F5), so no
  window shows them.

## Plants

`plants2.py` at ab7fbce: the watcher, the v17 columns, the cut rig. Each
plant is followed by that part's tests: `test_copy_limit.sh` and
`test_embedded_python.py`, or `test_cut_daemon_check.py` and
`test_cut_record.sh`. The cut rig is the same at 72aaa06. Output in
`plants2.out`.

| plant | result | killed by |
|---|---|---|
| S1 the watcher is not started | KILLED | copy_limit |
| S2 the watcher starts without the sync | SURVIVED | - |
| S3 the teardown does not stop the watcher | KILLED | copy_limit |
| S4 the watcher's line does not say "under the workload" | KILLED | copy_limit |
| S5 the watcher on every daemon row, uploads or not | SURVIVED | - |
| S6 the watcher gives up after 60 s | KILLED | copy_limit |
| V1 zero blocks: the end value, not the change | KILLED | embedded |
| V2 holes: the start value | KILLED | embedded |
| V3 hole chunks: the change | KILLED | embedded |
| K1 boundsparse: the copies not asked | KILLED | cut_check |
| K2 boundsparse: the abandoned not asked | KILLED | cut_check |
| K3 boundsparse: INFO unless the set holds at the end | SURVIVED | - |
| K4 boundsparse: a v17 daemon that took no holes (none) is asked nothing | SURVIVED | - |
| K5 bound: the abandoned not asked | KILLED | cut_check |
| K6 bound on a sparse volume (no --zero-fill) | KILLED | cut_record |
| K7 boundsparse zero-filled too | KILLED | cut_record |
| K8 the create does not take the course's arguments | KILLED | cut_record |

`plants3.py` at 72aaa06: the first review's plants and the answers'.
Each is followed by `test_copy_limit.sh`, `test_embedded_python.py` and
`test_skz_store.sh`. Output in `plants3.out`. It ran in two halves on
two copies of the head (its second argument, the first plant to run).
The first copy also ran T11 and gave the same result.

| plant | result | killed by |
|---|---|---|
| R1 TB_SKZ_UPLOAD value not checked | KILLED | skz_store |
| R2 the four settings not checked | KILLED | skz_store |
| R3 TB_S3_SECRET left out of the check | KILLED | skz_store |
| R4 upload with the releaser let through | KILLED | skz_store |
| R5 upload beside or without the daemon let through | KILLED | skz_store |
| R6 that refusal exits 0 | KILLED | skz_store |
| R7 TB_SKZ_UPLOAD_WAIT_S not checked | KILLED | skz_store |
| R8 upload with copies repeated let through | KILLED | skz_store |
| R9 every upload row refused, the daemon alone too | SURVIVED | - |
| U1 the keys not handed to the daemon | SURVIVED | - |
| U4 no upload line in .skzinfo | SURVIVED | - |
| T1 abandoned: the end value | KILLED | embedded |
| T2 mismatches: the end value | SURVIVED | - |
| T3 errors: the end value | KILLED | embedded |
| T4 abandoned and mismatches swapped | KILLED | embedded |
| T11 abandoned at a mismatch: the end value | SURVIVED | - |
| T12 held at the start reads the end | KILLED | embedded |
| T13 no mark for two daemons | KILLED | embedded |
| I1 named at end: the start value | KILLED | embedded |
| I2 longest map sync: the start value | KILLED | embedded |
| I3 header sync said from the start | KILLED | embedded |
| I8 "ms each" divides by a count of 0 | KILLED | embedded |
| I9 rows with the map off kept | KILLED | embedded |
| W7 an unreadable status never ends the wait | KILLED | copy_limit |
| W8 one unreadable status ends the wait | KILLED | copy_limit |
| M1 minio.sh down deletes any directory | KILLED | skz_store |

## Scripts and outputs added

- **`hold_copies.py REQUESTS CUT_NS RELEASE_NS`:** from a request log, the
  blocks first written while a cut was held, and of them those in a chunk
  written before its instant (F4).
- **`refusals2.sh ROOT`:** rows whose topologies are all known, so that the
  check after the loop is reached (F3's gap). Output in `refusals2.out`.
- **`refusals-f80.out`:** `refusals.sh` at f80dc48.
- **`loop_sim-72a.out`:** `loop_sim.sh` at 72aaa06, with its two controls.
- **`plants2.py`, `plants2.out`, `plants3.py`, `plants3.out`.**

## Can #123 merge at its head?

**Head: 72aaa06 (72aaa064c2de), read again with `gh api` at 20:18 UTC.
Cannot merge yet.** Two blocking findings are open: F4 and F5. CI and
every rig are green on this head (below), and GitHub calls it
mergeable, so these two findings are all that stands in the way.

**Closed:**

- F1: a refused upload row ended the sequence.
- F2: the abandoned column and the balance.
- F3: no test of the refusals.

All three are fixed, and each fix is checked above.

**Open and blocking.** Each has a fix of a few lines, and both can go in
one commit:

- **F4.** `boundsparse`'s "and nothing copied" cannot fail. Least fix:
  drop that criterion, say "none abandoned: the copies stayed under the
  bound", and make the test's status `dropped`, copies 0.
- **F5.** "under the workload" was false in every row of 44372469282.
  Least fix: the label says when the seed committed relative to the
  warm-up, or it drops "under the workload". The lead should know this
  before reading that run's figures as the seed under load.

**Open, not blocking:**

- G10: the watcher's missing lines;
- G11: gate on `status_version`;
- G12: the unit test's fixture;
- F3's gap: the last check is not shown to pass a legitimate row (R9);
- T2 and T11: two counters start at 0 in the bench test's fixture, so
  a figure read at the end passes;
- the balance's terms with no column;
- G4, which is the lead's decision.

**For #125.** This pull request's `bound` on `--zero-fill` and
`boundsparse` turn #125's failing run green on #125's own daemon, and
`bound` still drives an abandonment at the bound: 44378510338, both
images, 15 of 15. F4's least fix changes no outcome there.

So if the lead chooses to take F4 as a follow-up, the rig's verdict on
#125 is sound as it is: only the wording of `boundsparse`'s line
overstates. F5 does not touch the cut rig.

**CI and the rigs at the head.** Every run on 72aaa06 finished green. I
read them at 20:17 UTC.

- **ci, e2e, coverage, and skyzfs-ci (the push):** success.
- **The cut rig through the daemon, 44381438074 (the base's daemon,
  since this pull request changes no daemon code):** "worst exit 0" in
  all three jobs. On both held images (ubuntu-26.04 and
  ubuntu-26.04-arm), `bound` reads PASS, "'1' abandoned at the bound".
  `boundsparse` reads INFO, "the daemon keeps no holes (no cut_holes in
  its status, before M2.4e)", which is right for this daemon.
- **The cut rig with the reference, 44381438096 (four images); the
  control rig; the nodeb rig; the kernel probe:** success.
- **The daemon rig, 44381438065:** five jobs, the daemon on 24.04, 26.04
  and 26.04-arm and the stand-in on 24.04 and 26.04. The judge
  (`daemon_check.py`) exits 0 in each, so every line matches `EXPECTED`.
  - Every line that reads FAIL is a negative control (`ctl_*`): torn
    overlapping writes, kill without reissue, a volume that does not
    fail, and the hold too short and old content after a good sync on
    the daemon, or the delay before apply on the stand-in.
  - On the daemon the one WAITING is
    `pin_failed_volume_while_a_request_is_held`, as CLAUDE.md says. The
    stand-in's WAITING lines are the pins that need test hooks or the
    daemon's store.
- **The replay rig, 44381438264:**
  - the reference's two layouts, and the daemon on both images: every
    control as in `EXPECTED`, and `zdb -bcc` exits 0;
  - the hole course: 0 violations on both images.

**Codex** posted one finding, at f91bb6e: a refused row ended the
sequence, which is F1. It is answered (4619594584). When I read the
pull request, Codex had not reviewed a later head.

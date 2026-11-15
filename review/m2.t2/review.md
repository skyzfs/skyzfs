## Review of #97 (M2.T2: object storage and node B on the runners), head 5de05aa

**Verdict: one finding blocks the merge (B1).** At this head the rig's own run fails on `slog`, the product's layout, on both images. Node B's `zdb` cannot walk a pool whose intent log lies on a log device it does not have, so for that layout neither the published cut nor any control gets a verdict. It also puts a question to the lead, since the design's own method (`zdb -cc`) meets the same wall.

The rest is sound: on `life` the published cut passes and all four controls are caught, on both images. I found no defect in which cut is published or in what node B is given. S1 to S8 are suggestions.

### How I checked

- **The code against the row.** I read the diff against the M2.T2 row and design §9, before the description.
- **The head's run**, [43666257621](https://github.com/skyzfs/skyzfs/actions/runs/43666257621): I read the logs of both `record and check on node B` jobs.
- **OpenZFS 2.4.1**, for why `zdb` dies and for what `zdb -cc` reports: `zio.c`, `zdb.c`, `dmu_traverse.c`, `vdev_mirror.c`.
- **The tests here:** `test_nodeb_check.py` (11) and `cargo test -p skyzfs-nodeb` (5) pass.
- **Planted changes**, one at a time (`review/m2.t2/` on `skyzfs-review`):
  - 29 in `nodeb_check.py`: the unit tests catch 19. Of the ten survivors, two cancel each other, one is S8, and seven are in code no test without a kernel reaches (S2 is the one that matters).
  - 11 in the tool: its tests catch 8. Of the three survivors one is S6; the other two change nothing a pool image shows.
- **Not seen:** the cut rig's four jobs on this head, which run the changed `load()`, were still running when I wrote this. Codex has no findings.

### B1 (blocks). On `slog`, node B's `zdb` aborts, and the rig ends in its own error

(`nodeb_check.py:197`. Confidence: high on the fact, from the run. The cause is from the source and the M2.T1 judge; I could not run it here.)

- **What the run shows**, on ubuntu-26.04 and ubuntu-26.04-arm alike:
  - `life`: the published cut passes and the four controls are caught;
  - `slog`: `rig_error: zdb died with signal 6 ... PANIC: Pool 'skzcut' has encountered an uncorrectable I/O failure`, after `nodeb_put`, so at node B and not at the source. Exit 2.
- **What that means for the criterion.** "The stand-in's cut passes on node B, and each control is caught" is shown for `life` alone. The description calls `slog` the product's layout. The description's line for this run still reads `(NODEB_RUN)`.
- **Why, from the source:**
  - `slog`'s writer keeps an intent log on the log device. `zdb`'s walk visits each dataset's intent log in a read-only open (`dmu_traverse.c:144`) and reads its first block.
  - That block is on a device that is not there. A read that fails with ENXIO on a loaded pool suspends the pool, whatever the caller allows (`zio.c:5705`).
  - `zdb` sets the failure mode to panic (`zdb.c:10117`), so it aborts.
- **Why the source does not show it.** `cut_check.Check.zdb` tries the data device alone and then falls back to a copy of the log device beside it (`cut_check.py:420`). So the same cut passes `ck.copies()` at the source and dies at node B, which has no such copy.
- **It is not specific to `-bbbbb`.** `-c` and `-cc` use the same walk. So design §9's "`zdb -cc` on the read-only import" cannot run either, on any published cut of a pool with synchronous writers and a local log device.
- **Ways out, for you and the lead to choose:**
  1. Let the rig's check, not node B, have the log device's copy beside the image for `zdb`, as M2.T1 does. The image under test is still the one read from the bucket, and the blocks judged are still the data device's. Node B's import and comparison stay as they are.
  2. A scrub of a writable throwaway copy imported with `-m`, the design's other method. My reading, not run: the import's own first group replaces the newest metadata, so a bad spare copy of a block it replaces is never scrubbed. On `life` the `lacks` control is exactly that (spare copies of the pool's root objset block), so this route could miss it.

### Suggestions

**S1. `EXPECTED` says only "caught", not at which stage, and `early` stops at the first failure.** (`nodeb_check.py:67`, `:328`.)

- In the run each control is caught at a different stage: `early` at the snapshot, `lacks` at the copies, `later` at the import, `hole` at the files. That is good coverage, and the table does not require it.
- Scenario: `early` fails at the import for some other reason. It still reads `CAUGHT (expected: caught)`, and it no longer shows what the criterion's sentence is for: a clean pool that lacks only the snapshot.
- Suggestion: name the stage in `EXPECTED`. For `early`, also run the every-copy check after the missing snapshot and require that it passes. Then the line says "imports, every copy passes, no snapshot", which is the criterion's own point.

**S2. Nothing shows that the comparison of content is live.** (`nodeb_check.py:248`.)

- A plant that never compares the digests passes the unit tests. On the runner all four controls would still be caught: `hole` by the read error, or by the every-copy check.
- Suggestion: a fifth line that costs one more comparison. Hold the published cut against another snapshot's list (`snap-s18.sha256` for `s24`) and require a failure at the files. Or factor the comparison out and test it without a kernel.

**S3. `later` was the fallback in both runs.** (`nodeb_check.py:343`.)

- It was an uberblock of a later group, caught when the import fails. That is a fair later write, and the line says which kind it was.
- The stronger kind, a later write over a block the cut reaches, has not run on any image, and the description expects that to be the usual case. So what `later` exercises in practice is the import alone.
- Suggestion: carry the kind into the verdict or the table, so a reader of the table does not take one for the other.

**S4. A detail of several lines breaks `results.tsv`.** (`nodeb_check.py:222`, `:272`.)

- `zpool import`'s message has newlines and tabs. `ctl_later` in both runs is written as six lines.
- `cut_summary.py` prints only lines of three fields. So the row loses its `(expected: caught)`, and would lose `DIFFERS`.
- `cut-record.sh`'s `rig_error` already flattens its message with `tr`; `put` could do the same.

**S5. `get` gives one exit code to a store that is down and to a ref whose content cannot be read.** (`main.rs:25`, `:330`.)

- Both are exit 2, which the rig reads as its own error.
- With the stand-in that is right. In M2.T3 a ref that is listed and whose objects are missing is a verdict against the uploader. A code of its own would let the rig say so.

**S6. No test tells `get --ref snap/t` from a read of the head.** (`main.rs`, `get_ref`.)

- A plant that opens `head` whatever ref is asked for passes the tool's tests. It would pass the whole rig too, since each stand-in volume's head equals its `snap/t`.
- M2.T3 depends on the difference, because the daemon's head moves on after the cut.
- A test: a second commit after the snapshot, then `get` of `snap/t` returns the first.

**S7. `multihost` by dispatch ends as a verdict that is not the cut's.** (`skyzfs-nodeb-rig.yml:27`.)

- The input's description lists it. The description of this pull request says it fails at the import today.
- It would read `nodeb_cut FAIL`, exit 1. Until node B imports a writable copy, refuse it by name or end it as the rig's own error with the reason.

**S8. Smaller notes.**

- **An uncaught exception exits 1.** `main` catches `RigError` alone (`nodeb_check.py:371`). A missing `snap-NAME.sha256` or a `ValueError` from the replay ends with Python's exit 1, which `cut-record.sh` records as a verdict, with no line in `results.tsv`.
- **A `zdb` abort in a control is a rig error here** (`:199`). The replay rig counts it as a catch, since the image is damaged on purpose.
- **`later_uberblock`'s condition on the record has no test.** The plant that drops it survives. It matters in the rewind recordings, where a later group's uberblock precedes the instant.
- **The docstring's "`zdb -cc`, which reads one copy".** It reads every copy and reports a block only when none passes (`zdb.c:6318`, `:6381`). The conclusion stands: the every-copy check is the stronger one.

### Your four questions

**1. Is node B a node that knows only the bucket? Yes, for `refs` and `get`.**

- **No local state.** `ObjectRefStore::new` holds the store, a clock and an in-process channel (`object_store.rs:203`). `Engine::open_ref` reads the superblock, the ref and the commit from the store (`skynbd-engine/src/lib.rs:720`). `EngineConfig` names no path.
- **No owner, so no lease.** Each command is a process of its own, and the only file it writes is `--out`.
- **What the rig gives node B beyond the bucket:** the volume's name, the ref's name (which it finds in the listing first), the pool's name, the ZFS snapshot's name, whether there is a log device, and the source's digests. The description says the snapshot's name is M2.6's to supply.
- **On the runner:** the import takes a read-only loop device with `-d`, and `zdb` a directory that holds that one file. The source's device is removed before the check runs.
- **One gap:** S6.

**2. Is the published cut the one that holds *t*? Yes.**

- `createtxg` is the group that first holds the snapshot. The first cut of the session at or after it holds *t*, and the cut before it does not.
- Staying inside the session is what makes this safe in the rewind recordings, where a later session's groups go back.
- Both cuts must be usable: not stale, and opening as themselves.
- The plants on `>=`, on each session bound, on either cut's usability and on the middle pick are all caught.
- The run agrees: `s24` taken in group 20, published by cut 10 of group 20, early cut 9 of group 17, which has `s6`, `s12` and `s18`.

**3. Do the four controls show what the criterion needs?**

- **On `life`, yes**, with the notes above:
  - `early` shows the criterion's point in the run, and the table does not require it (S1);
  - `lacks` is caught by the every-copy check alone: two or three copies of metadata blocks fail, and no block is left without a good copy. So `zdb -cc` would have passed this control;
  - `later` is the fallback (S3);
  - `hole` is caught at the files, by a read error.
- **On `slog`, nothing is shown yet** (B1).

**4. Is the every-copy check a fair stand-in? Yes where it runs, and stronger than the design's.**

- `zdb -cc` reports a block only when no copy passes; this check fails on any copy.
- It holds its list against `zdb`'s own count.
- It ends as the rig's error on what it cannot read: another checksum, ganged blocks, another device.
- **Its limits:**
  - it does not judge intent-log blocks;
  - it cannot run where `zdb` cannot walk the pool, which is B1.

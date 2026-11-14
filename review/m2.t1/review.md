## Review of #94 (M2.T1: a reference for cuts, and recorded pools), head 8a131ee

**Verdict: nothing here blocks the merge.** The reference applies design §5.2's rule as amended, the check holds the claim the work package names, and the two new control judges are sound for the two rules they judge. Five suggestions below, most useful first. The four-image cut rig run ([43651302466](https://github.com/skyzfs/skyzfs/actions/runs/43651302466)) was still recording when I wrote this. The merge should still wait for its table to read as `EXPECTED` on the three images that run for the first time. The description still has the placeholder `(CUT_RIG)` where that run goes.

### How I checked

- I read `cut_ref.py`, `cut_check.py`, `cut-record.sh`, the workflow, both test files and the fixtures' README against design §5.2 at the base. I also read OpenZFS 2.4.1 for:
  - the rewind policies: `spa.c`, `zpool_main.c`, `zdb.c`, `libzfs_pool.c`;
  - MMP: `mmp.c`.
- Both test files pass: 27 and 19 tests.
- I planted 30 changes, one at a time. The script is `review/m2.t1/plants.py` on `skyzfs-review`.
  - 25 changes were caught.
  - Every planted change to the repeat rule, the preflush or FUA barriers, group 0, the discard and overwrite handling in `opens`, the session filter, `committed_txgs`, the stale skip, `dropped_states`, `synced_before_ready` and the picks was caught.
  - The five that survived are covered in S3 and S5. A sixth survivor, `stale` flagging two consecutive equal states, is equivalent under the design rule.
- I recomputed `opens` from every fixture (`opens_check.py`). It equals `opens.tsv` at every cut.
- From run 43647779663's artifact (`coverage.py`), I measured which cuts the session check reaches.

### Suggestions (none blocks)

**S1. A `zpool import -T N` that passes does not show the pool came up at N.** (`cut_check.py:376-409`, used at `:445-456`; confidence high on the code path, not seen to happen.)

Why:
- `zpool import -T` asks for `ZPOOL_DO_REWIND | ZPOOL_EXTREME_REWIND` (`zpool_main.c:4442`).
- So `spa_import` loads in `SPA_LOAD_RECOVER` (`spa.c:7389`).
- If the load at N fails, `spa_load_best` retries older uberblocks down to `TXG_INITIAL` (`spa.c:6365-6378`).
- `kimport` counts any exit 0 as a pass.

Scenario:
1. A cut does not open as itself.
2. The kernel's load at N fails, for example on `spa_load_verify`'s metadata errors.
3. The kernel comes up at N−k instead and prints `Pool skzcut returned to its state as of <time>`.
4. The line still reads "at its own group (zdb -t, import -T)".

The volume's claim still holds, because `zdb -t N` is strict:
- without `-F`/`-X` it uses `ZPOOL_NEVER_REWIND` (`zdb.c:9451`, `9845-9847`);
- the judge requires it to select the cut's own uberblock;
- every copy is checked at N.

So only the import half of the line is weaker than it reads. Fix: compare the time in "returned to its state as of" with the cut's timestamp, and fail on a difference. That time is the loaded uberblock's `ZPOOL_CONFIG_LOAD_TIME`, printed at `libzfs_pool.c:1952`. A plain import, used for a cut that opens as itself, has no rewind, so this applies only to `-T`.

**S2. The order check does not reach the export's cuts, the create's first group or `-FX`'s group, and its line does not say so.** (`cut_check.py:177-204`; measured.)

In run 43647779663, 54 of 543 cuts are outside the check:
- the cuts after `session-K-end`, which is every export's: 2 a session, and 14 in `multihost` (cuts 80 to 93);
- group 4 of every create;
- group 43 of `rewindf`'s `-FX`, which falls below the kstat's window.

These cuts are still judged as volumes. But the reason the order check exists, "an import cannot tell a cut counted twice from one counted once", applies to them, and nothing checks it. Scenario: a rule that makes a second cut of one state only during an export would make a duplicate that no line shows, and `sessions` still reads `PASS`, "N sessions as committed".

Suggestion:
- give the line the count of cuts the kstat covered;
- for the rest, check at least that groups increase strictly, and that an export's last group is what `zdb -e -u` gives after the export.

**S3. `opens` differs from M2.1b's rule on a tie, and no unit test pins the order or the comparison of states.** (`cut_ref.py:232-235`.)

- **How `opens` picks.** `max(..., key=(txg, timestamp))` keeps the first slot in insertion order on a tie.
- **What ZFS does.** It replaces its best only on a strictly greater compare, and it reads the slots asynchronously, so its choice on a tie is not fixed.
- **What M2.1b says.** A tie with another state is "not itself".
- **The fixtures.** No fixture has such a tie at any cut. Two `multihost` cuts have several timestamps in the best group.

Two plants pass all 46 tests:
- taking the group alone as the key of the best (the rig would catch it only at run time, as a cut FAIL in `multihost`);
- comparing `itself` by the group alone.

Suggestions:
- make a tie with another state "not itself", as M2.1b's rule does;
- add two unit tests:
  - a multihost copy of the best group with a later timestamp, written to a new slot after the older one (the reported timestamp must be the later one);
  - a best of the cut's group with another root and a later timestamp (`itself` False);
- have `Fixtures` recompute `opens` and hold `ring_txg` and `itself` to `opens.tsv`. The fixture carries every ring write's data, and I checked that the recomputed values agree. This keeps M2.1b's expected values from drifting away from the reference unnoticed.

**S4. A stale cut outside `multihost` would pass unseen.**

- Stale cuts are left out of the sessions (`:183`, `:196`) and of `rule_same`.
- `obs_stale_cuts` is a count with no expectation.
- The design attributes stale writes to the multihost thread, and ZFS writes none without `multihost=on` (`mmp.c:796-797`).
- So a stale cut in `life` or `rewind` would mean something the design does not expect, and the rig would pass it.
- Run 43647779663 had 0 stale cuts everywhere but `multihost`, which had 2.

Suggestion: expect 0 outside `multihost`, in a table like `EXPECTED_OPENS`.

**S5. Smaller test gaps.**

- **`rule_same`.** Its key is `(txg, prefix)`. A plant that compares groups alone survives, because no test has the same groups at other instants. The key also leaves out the root. Key it on `(txg, rootbp, prefix)` and add the case.
- **`EXPECTED`.** `test_every_control_has_every_recording` accepts `missed` as an expected value, so a table that expects a control to be missed passes. A plant that adds `missed` to `later` survives. No control should ever expect `missed`: drop it from the allowed set.
- **`read_fixture`.** It ignores the version line. M2.1b reads the same files, so refuse any version other than `FIXTURE_VERSION`.

### The five questions

**(1) `cut_ref.cuts` against §5.2 as amended: it matches.**

- **The repeat rule.** A repeat is the pending mark or the newest cut. "A cut that a flush is carrying" needs no state here, because the barrier falls at the flush record itself.
- **REQ_PREFLUSH.** The barrier is at the record, before the record's own uberblocks.
- **FUA.** The barrier is after the write, as the pump serves it.
- **What the recordings exercise.** No recorded write carries FUA or REQ_PREFLUSH: every fixture has only `write`, `flush`, `discard` and `mark`. Those two branches are tested only synthetically.
- **A difference of instant.** The reference's instant is the flush's place in dm-log-writes' completion order. The daemon's barrier also holds writes received before the flush that completed after it was submitted. The rig checks the smaller instant. For ZFS 2.4.1 the difference is intent-log and multihost writes, which the design's second reason covers. M2.T3, with the daemon's own log, holds the daemon's instant.

**(2) The judge: it holds the claim**, with S1's caveat on the `-T` import.

For a cut that opens as itself:
- zdb's choice must equal the rings' best, by group and timestamp;
- every copy must pass;
- the plain read-only import must succeed. It cannot rewind, so it opens at that choice.

For a cut that does not open as itself:
- `zdb -t N` must choose the cut's own uberblock;
- every copy at N must pass.

The state it does open at is reported and not judged. That is right by §5.2 and by §10's row: the abandoned state may be damaged.

The writable import of a copy for `multihost` is right for 2.4.1. The claim's writes land on the copy, and the copies were already checked with zdb.

**(3) `dropped_states` and `rule_same`: sound for `txg-alone` and `count-repeats`.** Neither is a general comparison of rules.

- `dropped_states` sees only drops. It cannot see a rule that cuts the same state at an earlier instant; `early` covers that, through the volume.
- Extra cuts are left to the sessions, inside the kstat's window (S2).
- A cut that does not open as itself is exempt, and is left to the sessions; in `rewind` they catch it.
- The stale leniency in `rule_same` is right as long as stale cuts come only from multihost (S4).

**(4) `later` as `('not built', 'caught')`: not too loose.**

- `missed` is accepted nowhere.
- `full` and `autotrim` must catch it on every image, so every run shows the copies check catching a later write.
- The loosening admits only "not built". S5 asks for `missed` to be pinned out.

**(5) `read_fixture` and `Fixtures`: sound.**

- The reference, read back from each fixture, gives that fixture's cuts and uberblocks.
- Two gaps, above: the version check (S5) and the recomputed `opens` (S3).
- For M2.1b: `itself` in `opens.tsv` is the reference's reading. The rig held it to zdb by group and timestamp only. With no tie in the fixtures, that is the same as "zdb chose the cut's own state". M2.1b's tie rule and the reference's should agree (S3).

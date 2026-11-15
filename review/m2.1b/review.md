## Review of #95 (M2.1b: a cut says whether it opens as itself), head d665a95

**Verdict: nothing here blocks the merge.** I found no defect in the code. The tests leave two rules unpinned, and both are on the side that matters, a cut wrongly said to open as itself: S1 and S2 below. I would take those two in this pull request; three small tests for them are in `review/m2.1b/reproducers.patch` on `skyzfs-review`. The rest are suggestions. The daemon rig's three jobs on this head were still running when I wrote this; everything else on CI has passed.

### How I checked

- I read the diff against design §5.2 and the M2.1b row, and against OpenZFS 2.4.1: `vdev_uberblock_compare`, `vdev_uberblock_load_done` and `vdev_uberblock_load` (`vdev_label.c`), and the `MMP_*` macros (`uberblock_impl.h`).
- The tests of the four touched crates pass here (rustc 1.100.0), as do `test_skz_time.py` (39) and `test_cut_ref.py` (32).
- I planted 35 changes, one at a time, in the new code (`review/m2.1b/plants.py`, results in `plants.out`). The tests caught 28. Of the seven survivors:
  - five are S1 and S2;
  - one is S6;
  - one changes no output: reading `itself` from a version 11 file, which `status` never prints for that version.

### Findings (none blocks)

**S1. No test takes a cut that opens only with a rewind through the status file.** (`status.rs:363`, `:455`; confidence high, two plants survive.)

- Writing 1 for both answers passes every test. So does reading any value but 0 as `itself`.
- The tests print `rewind` only from a `Status` built in memory. The one cut that goes through `encode` and `decode` opens as itself.
- Scenario: with the first plant, `skyzfsd status` prints `cut_opens itself` for a cut that opens only with a rewind. That is the answer the agent's syncs and a publish will wait for.
- The test in the patch writes such a cut, expects 2 at offset 304, and reads it back as not itself. It fails with either plant.

**S2. The rule "while any position is unknown, no cut opens as itself" has no test.** (`skyzfs-uberblock/src/lib.rs:468`, `:380`; `inplace.rs:818`; confidence high, three plants survive.)

- The three plants that pass every test:
  - `View::chosen` without `is_known()`;
  - `View::unknown` covering the first ring alone;
  - `read_rings` taking a ring it could not read as read, with the zeros of its buffer.
- Why the tests miss it: wherever a view is unknown in them, it also holds no uberblock, and `chosen` then says no for that reason. No test fails a read at the start.
- Scenario: a start reads three rings and fails on the fourth. With the third plant that ring is taken to be empty, and a cut says it opens as itself whatever the ring holds. The description names this rule as one of the conservative readings.
- The two tests in the patch:
  - one on the view: three rings known and holding one state, the fourth unknown;
  - one on the store: it fails each of the start's reads in turn (`SimDisk::fail_read`) and expects exactly four starts whose first cut says `rewind`.

  Together they fail with each of the three plants.

**S3. A ring the start could not read leaves no trace.** (`inplace.rs:818`; a matter of observability.)

- The start goes on, which is right.
- From then on every cut says `rewind` until the ring's slots have all been written again. ZFS writes one slot of each label per group, so that takes up to as many groups as the ring has slots, 32 at ashift 12.
- Nothing says why: no line in the daemon's output, no status key. An agent syncing "until `cut_opens itself`" after a rewind import would see the same answer for both causes.
- Suggestion: say it once at the start, or count the rings not read in the status.

**S4. `recorded.rs` still describes the source it no longer reads.** (lines 12 to 15, and the doc of `the_rule_makes_the_reference_cuts_of_every_recorded_pool`.)

- Both say the flag is held to `check.tsv`. Since d665a95 it is held to `itself` in `opens.tsv`.
- The guard on `cut_N FAIL` lines (386 to 405) cannot fire:
  - `test_cut_ref.py`'s `Fixtures` refuses a fixture whose `check.tsv` holds a FAIL;
  - since #94 the judge does not fail a cut for opening at another group.
- It is harmless, but it reads as a check.

**S5. The pins in `the_recordings_notice_the_defects_of_a_repeat` depend on races that vary by run.** (line 502.)

- The pins hold here, and pinning them is right.
- `STALE_MOVES_MARK` is noticed in `multihost` only because this recording has a stale multihost write. #94's description says some runs have none.
- So a refresh of the fixtures by testing can fail `cargo test` on this line with nothing wrong.
- Suggestion: say so in the failure message and in the fixtures' README, so that whoever refreshes them knows to change the table.

**S6. Optional: no test that a request which ends without being applied changes nothing.** (`cuts.rs:370`.)

- A plant that applies the change at `ended` passes every test.
- It is reachable only on a read-only volume, where a refused uberblock write would then mark and a flush would count a cut.
- The rule is M2.1's ("counts once applied"); this package adds the view to what it would change.

### Your four questions

**1. `View::chosen`: it errs only toward "rewind", and comparing states is right.**

- **The order** matches `vdev_uberblock_compare`: the group, the timestamp, then the multihost sequence only with `MMP_MAGIC` and the valid bit.
- **Ties.** ZFS keeps a new best only on a strictly greater compare and reads the slots in no fixed order. So a tie between two states has no fixed answer, and "neither" is the safe one. It is also the rule the reference now has (#94).
- **What can make the view say "itself" wrongly.** An import's choice is the greatest uberblock among those that pass the label checksum. The view's set is a superset of that. So the view is wrong in that direction only when every copy of the cut's own state at the greatest rank fails its checksum while another state's valid uberblock ranks above the valid copies. I see no way to reach that without a rewind, a multihost copy and a torn write together. #96 closes it.
- **States, not uberblocks.** Two cases where the cut's own state is in view at a higher rank than the uberblock that marked it, and an import opens that state: a multihost copy, and the first group of a rewind by `-F`.
- **A newer state applied before the barrier.** `another_state_under_the_carried_group_marks` shows it: the cut says `rewind`, which is true of the volume at that instant.

**2. Where the start reads the rings: the right place.**

- **After the sync and after the cache is emptied and checked:** the reads see the disk.
- **Before `set_up_io`:** the store is not handed out until `assemble` returns, so no request precedes them.
- **Nothing later in the start writes the home image.**
- **A take-over** reads what the lost instance left, half-applied writes included. That is the volume as an import would find it, and the kernel's reissue then goes through `applied`.
- **A discard** is rightly left out of the view, since the store keeps the content. The reference clears the slot there. No fixture discards a ring (I checked all nine), so the two agree on the recordings.
- **The barrier.** `itself` is judged in `count`, at the barrier, where every write received before the flush is in the view and none received after has begun.

**3. Status version 12: the layout and the reading of version 11 are right.**

- **Version 12:** 312 bytes; the fields after the pump's times lie 32 bytes earlier than in version 11.
- **Version 11** is read from its own places. Four plants that leave one of its fields unshifted are caught.
- **`pump_taken_*`:** dropping them is sound. `InPlace` gives no `ready_at`, so the time was 0 by construction. In `tests/`, only `skz_time.py` reads those keys, and it handles version 12 since #94.
- **The rigs:** nothing in them pins the status version.
- **The one gap** is S1.

**4. d665a95: reading `opens.tsv` is right.**

- `itself` there is the reference's reading of the rings, which the rig held to zdb by group, timestamp and, since #94's last head, the root.
- The stale cuts 10 and 27 of `multihost` are `rewind` by both.
- The recorded tests run in 0.3 s here.
- S4 and S5 are the two notes on this commit.

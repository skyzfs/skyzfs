## Review of M2.1 at b89d82b

**Two findings block the merge:**
- CI's `lint` fails on both workflows (1).
- Codex's finding is right: one pool state can be cut twice (2).

**One gap in the tests, recommended here and not blocking (3).** The barrier's place for a flush that does not begin its own sync is held by no test.

**The rest holds.**
- **The parser** matches OpenZFS 2.4.1 line by line. Every defect I planted in it was caught.
- **Status version 11** lies where its table says.
- **The barrier's other half** is the defect I planted, and its test shows it.

I found no other way for a cut's group to err high within the design's premises. One premise case is in Q1.

I read the diff and the code it hooks into before the description. At b89d82b with rustc 1.100.0:
- **fmt** passes.
- **clippy** passes plain, with `skyzfsd/test-hooks,skyzfs-local/test-hooks`, and with `skyzfs-ublk-sys/control-test-program`.
- **The four crates this changes** (`skyzfs-uberblock`, `skyzfs-local`, `skyzfs-ublk`, `skyzfsd`) pass: 372 tests plain, 388 with the test hooks.
- **The whole workspace** ran this container's disk out twice, so for its other crates I rely on CI's `test` and `unit-arm64` jobs, green on this head.

### 1. Blocking: CI's `lint` is red on both workflows, from `tests/bench/test_skz_time.py`

**The failing runs:**
- [ci.yml](https://github.com/skyzfs/skyzfs/actions/runs/43636269781/job/132573660365)
- [skyzfs-ci.yml](https://github.com/skyzfs/skyzfs/actions/runs/43636210676/job/132573477633)

Both end with 7 failures and 9 errors.

**The cause.** `status_keys()` (`tests/bench/test_skz_time.py`) builds its readings from the `"time_*"` lines of `crates/skyzfs-ublk/src/status.rs`, and version 11 removes them all. I ran the script both ways:
- at this head it fails as in CI;
- with the base's `tests/bench` and `status.rs` it passes, 29 tests.

**The test is only the visible part.** `skz_time.delta` reads only the `time_*` keys. On a version 11 daemon the benchmark's time section therefore says "NO TIMES: … the status is older than version 5". That is wrong, and the pump's stages are gone from it.

**The pump's stages were shown before.** Through version 10, the in-place daemon showed them with real values: fetched to submitted, ready to taken, and taken to committed.
- The base decodes `timing` and `pump` for any file of version 5 or later (`fifth.then(...)`).
- So `status` printed every `time_*` key: the pump's with their values, the rest as zeros.
- The doc of `PUMP_KEYS` (status.rs:250-256) says "the in-place store never printed" them. That is not so for the pump's keys.

**What it needs.** `test_skz_time.py` must not depend on keys that the daemon no longer has, and `skz_time.py` has to read `pump_*`. Both files are under `tests/bench`. Which session changes them is the lead's call, but the pull request cannot merge red.

### 2. Blocking: a repeat applied between a flush's receipt and its barrier makes a second cut (Codex's finding, confirmed)

**Where.** A flush takes the mark when it is received (`cuts.rs:219`, `self.mark.take()`). `marks()` (`cuts.rs:271`) compares an applied uberblock with the mark and the newest cut only.

**What goes wrong.**
- A copy of the same uberblock may be applied after the flush was received and before the flush's barrier: it waits behind a write on the threads.
- It then finds no mark and an older newest cut, so it marks the same state again.
- The flush's barrier cuts that state. The next flush cuts it again.

**Reproduced.** `review_a_label_write_applied_after_a_flush_took_the_mark_is_a_repeat` in the patch below fails at this head with `(2, Some(5))` where it wants `(1, Some(5))`:
- label 0's write of group 5 is applied;
- label 1's write of the same uberblock is held on a thread;
- a flush is received;
- label 1's write is let go;
- another flush follows.

**ZFS sends exactly that order.**
- `vdev_uberblock_sync` (`vdev_label.c` 1791-1795) issues the four label writes under one parent zio. On the threads, one can be applied while another is still being written.
- A stray flush in between, such as the intent log's, is the case §5.2 names ("a stray flush between the uberblock copies").

**Effect.**
- The count goes up with no new pool state, against §5.2's "makes no new cut".
- A publish is not hurt: the second cut's group is below any later `createtxg`.
- But M2.T3's check of `cuts` against the groups would see it, and M2.2 would replace a waiting cut for nothing.

**Fix.** Treat a carried state as a mark for the repeat until its barrier: compare with `carrying.values()` too, or keep it in a field. The reproducer is the test.

### 3. Not blocking: no test holds the barrier's place for a flush that waits for a sync under way, or joins one

The code is right. The barrier is the first line of `flush()`, which runs when the flush is executed. But I planted three changes, and all 148 tests of `skyzfs-local` pass with each, plain and with `test-hooks`:

| plant | what changes |
|---|---|
| P1 | the barrier told when the flush's own sync begins (`begin_sync`) |
| P4 | the barrier told when its sync ends |
| P9 | no barrier for a flush that joins a sync under way |

**Why they pass.**
- The tests' syncs end at once.
- The one test that holds a flush back holds it behind a write, not behind a sync.

**Why it matters.** Under P1 and P4, a flush that waits behind a sync under way gets its barrier after writes received after it are applied. That is the wrong instant, the one M2.2 is to freeze a map at. Under P9 a joined flush counts no cut.

**ZFS reaches both orders:**
- an uberblock write applied while a sync begun before it runs;
- a stray flush received while the uberblock write is on a thread, which begins a sync once the write is applied, with ZFS's own flush joining it.

**Two tests in the patch below catch them:**
- `review_a_flush_behind_a_sync_under_way_has_its_barrier_when_executed` fails under P1 and P4;
- `review_a_flush_that_joins_a_sync_has_its_barrier_and_its_cut` fails under P1, P4 and P9.

Both pass at this head. The patch is on the review branch: [`review/m2.1/reproducers.patch`](https://github.com/skyzfs/skyzfs/blob/22ad5f3/review/m2.1/reproducers.patch), three tests appended to `crates/skyzfs-local/src/inplace/tests/cuts.rs`.

### Your five questions

**Q1. Can a group err high otherwise?**

Not that I can find, within the premise that the volume holds the whole pool. The import case needs a stale mark, and within one attachment a pool's life must end without the flush after its uberblock for one to exist.

What I checked in 2.4.1:

| path | what it writes | result |
|---|---|---|
| `vdev_config_sync` | uberblocks after the group's blocks and a flush; a flush after them | the cut is right |
| `mmp_update_uberblock` | called only after `vdev_config_sync` returns (`vdev_label.c:2141`), so a multihost write never carries a group above the last synced one | low only |
| `vdev_label_init` | the ring, with group 0 (`vdev_label.c:1218`) | no cut |
| `vdev_copy_uberblocks` | old uberblocks copied to labels 2 and 3 when a vdev is expanded (`vdev_label.c:1671`) | low, and only if a volume can grow; the store's labels are fixed at its start size |
| a checkpoint rewind | the checkpointed uberblock with the group above the newest (`spa.c:5857`), whose blocks the checkpoint kept | right |
| a rewind import | the new history's first group is `spa_last_ubsync_txg`, the abandoned best one, under `-F` (`spa.c:6342`, `4713`), and the group after the one it rewound to under `-T`, as testing recorded | right for the cut; the rule the lead added at 66cd30a covers which uberblock an import then chooses |

**The premise case.** A volume that joined a pool by `zpool attach` or `zpool replace`, a likely way to move an existing pool onto SkyZFS:
- it gets every uberblock from the moment it joins;
- the resilver has not yet copied the older blocks those uberblocks reach;
- so the recorded group is right, but the volume is not a complete pool at it.

The same holds for a pool with a second top-level data vdev. The rings cannot show either. That is for the design to state, or for a later package to read from the label's configuration; it is not this package's.

**Q2. The barrier's other half.**

It is the defect I planted, and wider: your mutant lets a flush pass waiting flushes too. The test notices it by the read that ends while the first write is held. In the clean run it also holds the write half, by `waiting == 3` and the order of `Seen`.

| plant | caught by |
|---|---|
| only writes pass a waiting flush (P8) | the new test, on the waiting count and the order |
| the release loop passes a blocked front and releases what is behind it (P2) | the new test and five whole-program tests |

**Q3. The hooks.**

**Every successful `Op::Write` has `Applied`.** It is called on the caller's thread and in `collect`. Removing the first is caught: P3 fails two tests in `tests/cuts.rs`.

Two changes have no `Applied` line, and neither matters for a cut:
- **a write-zeroes**, which is not `Op::Write`. It takes effect when executed, which is its place in the order received relative to a flush. Worth one line in the record's documentation, so that M2.T1's reference places it by receipt.
- **a write whose image write succeeded but which ends after the volume failed.** No barrier follows a failure.

**No flush is executed without its barrier.** `flush()` is the only way in, and a flush on a failed volume is completed before it. Section 3 is about the tests, not the code.

**Q4. The parser.**

Against `zfs-2.4.1`:

| what | where |
|---|---|
| the label layout | `vdev_impl.h` 478-482 and 535-540 |
| `VDEV_LABELS` | `vdev_impl.h` 562 |
| `MAX_UBERBLOCK_SHIFT` | `vdev_impl.h` 491 |
| `UBERBLOCK_MAGIC` and `UBERBLOCK_SHIFT` | `uberblock_impl.h` 46-47 |
| `vdev_label_offset` | `vdev_label.c` 163-170 |
| the slot written whole with zeros after the uberblock | `vdev_label.c` 1786-1795 |
| `uberblock_verify` | `uberblock.c` 33-42 |

All match the crate's reading, and so do the scan's two premises: no KiB boundary in a larger slot holds anything but zeros, and the label's checksum never starts at a whole KiB.

I planted seven defects. All were caught:
- the size not rounded to a label;
- a 4 KiB stride;
- little endian only;
- the ring at 112 KiB;
- the root read little endian always;
- the repeat by group alone;
- any group counted.

**Not checking the checksum** is the design's reading, and I agree it is enough for a pool on the whole volume. It would be cheap insurance, though: four checks a group, of `ZIO_CHECKSUM_LABEL`, SHA-256 with the vdev offset as the verifier, in the slot's last 40 bytes. With it, bytes that are not ZFS's could make no cut. Without it they can make one with any group, a higher one included: a pool on a partition, or another use of the volume. Optional.

**Q5. Status version 11.**

**The encoder and the decoder** agree with the table field by field:

| field | offset |
|---|---|
| failures | 112 |
| `first_write_copies` | 120 |
| `start_ms` | 128 |
| `zeroes_applied` | 136 |
| the pump | 144 to 248 |
| the queue | 248 |
| the commands | 296 to 312 |
| `cuts` | 320 |
| `cut_group` | 328 |
| total length | 336 |

**Version 10's constants** match its table: 120, 128, 168, 192, 1336, 1440, 1960 and 2008.

**`cuts` and `cut_group`** come from version 11 files only, and `cut_group` appears only once there is a cut. The 0x5a fill in the old versions' tests is a good guard.

The one problem is the keys' effect on the benchmark (section 1).

### CI on b89d82b

**Green:**
- the daemon rig on the daemon, on 24.04, 26.04 and 26.04-arm. The judge exited 0; every pin is PASS but the known WAITING one (a failure with a request held).
- the replay rig's jobs through the daemon;
- the control rig;
- ci.yml's `test` and `unit-arm64`, `skyzfs-ci.yml`'s `test` and `unit-arm64`;
- coverage;
- e2e;
- the k3s smokes.

**Red:** `lint`, on both workflows (section 1).

---
_Generated by [Claude Code](https://claude.ai/code)_

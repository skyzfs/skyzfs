## Next look at #103 at 008274f

**Verdict: the review says it can merge. Nothing blocks, and D1 and D2 are fixed.**

Two remarks are left (R1, R2). Neither lets a wrong daemon pass.

### The three things you asked about

1. **The per-cut scope of unplaced blocks (`held_content`, 008274f): right.**
   - A block is unplaced from its write-zeroes on, only for the cuts after it, and it is placed again by a write received after the write-zeroes. A wrong block that is placed is a verdict whatever else the log holds.
   - Planted wrong six ways (no block unplaced, any later write places it, no later write places it, every wrong block the rig's error, every wrong block a verdict, a placed wrong block not definite), each fails a test.
   - One small thing: a later write-zeroes over the same block, with no write pending on it, also places the block, and `live` keeps it. That errs towards ERROR and never towards PASS.
2. **D1's placement behind the last flush received (`records_of`'s `behind`): right.**
   - It matches the store: a request received while a flush waits runs behind that flush (`inplace.rs`, `accept`), and with two flushes waiting it runs behind the later one.
   - My reproducer now passes: the cut holds block 10 as it was, and the line reads PASS (`zeroes_behind_flush_008274f.out`).
   - Planted wrong four ways (placed at its receipt, behind the first flush, dropped at the barrier, placed before the barrier), each fails a test.
3. **`hold_idle` and the floor on a cut held across the write-zeroes: sound.**
   - The floor says what it should. On the old `edges` recording of run 43702505054, where no cut was held across the write-zeroes, the judge at this head ends with the rig's error and exit 2. On this head's runs it reads "held across the write-zeroes: cut 7 (32 blocks)" on both images ([run 43709423275](https://github.com/skyzfs/skyzfs/actions/runs/43709423275)).
   - The first write is random data, so the cut must read data where the home has zeros. A daemon that zeroes without copying would fail `held_content` there.
   - One window is left in `hold_idle`. If ZFS writes an uberblock by itself between the holder's pause with nothing held and the sync after the first write, that cut becomes the one being uploaded. The cut after the write then waits, and the next load's cut replaces it. The floor is not met, and the run ends as the rig's error. It names itself, so nothing wrong passes. If `edges` ever ends so, this is the likely cause.

### What I checked

- **The unit tests pass here on 008274f:** 57, 12 and 11.
- **Planted changes on this head** (`plants.py`, `plants2.py`, and 18 new ones in `plants3.py`; `plants_008274f.out`, `plants2_008274f.out`, `plants3.out`): 79 in all.
  - 64 fail a test. That includes every plant of S1, S2 and S3 that still applies.
  - 10 no longer apply, because the code they changed was rewritten; the new plants cover that code.
  - 5 pass. Four change nothing a run would see: a reply read by its first line, the cut that waits written when none is being uploaded, the command written in place, and `edges` with no write-zeroes (the floor raises for it now). The fifth: the first written cut is left out of the floor, and no test has a recording where only the first cut was held across the write-zeroes.
- **The judge at this head on the earlier runs' recordings** (`config`, `multihost`, `life`, `bound`): exit 0, as before.
- **Codex** has no unresolved finding on this head.
- **CI on 008274f:** 17 checks passed, none failed, 20 were cancelled. The cut rig on the daemon passed on 26.04 and 26.04-arm. The cancelled ones are the reference's recordings, node B, the daemon rig, the replay rig and "the table and its judge". Whether that counts as green is the lead's, as before.

The files are in `review/m2.t3-2a/` on `skyzfs-review`.

### Remarks (neither blocks)

**R1. `obs_copied_blocks` counts the blocks a write touched, not the blocks whose content changed.**

- A block rewritten with the content it had reads the same from the home as from a copy, so it shows no copy.
- On the same recordings, the judge's count against mine (which compares the hashes):

  | recording | first written cut | later cuts |
  |---|---|---|
  | config | 177 against 125 | 112 against 8 |
  | multihost | 1861 against 1809 | 25 against 25 |
  | life | 1803 against 1751 | 0 against 0 |
  | edges (run 43702505054) | 1808 against 1756 | 56 against 4 |

- The floor in `edges` is not affected: it counts blocks under the write-zeroes, and those held random data.
- The line is there to show when a recording stops reading from copies (S4). For that it should count a block only where the write's hash differs from the content at the cut's instant.

**R2. A write the daemon refuses for its own reason ends as the rig's error.** `must_be_gone` raises when the cut is still being uploaded after a refused write. That is right for the rig's image. It also covers a daemon that could not read a block of the held cut (`read block B of cut N`), which is the daemon's failure. The message carries the daemon's refusal, so a reader can tell; the exit code cannot.

### Answers to your notes

- **`refused_then_released.py` hangs against the new fake daemon.** That is my reproducer's fault: it waits for `ready` with no limit and gives the fake its old arguments. Your tests `test_a_write_refused_while_the_cut_is_held` and `test_a_release_refused_while_the_cut_is_held` replace it, and three plants in that code fail them.
- **A log with no hashes** left for the take-over stage: fine.

---
_Generated by [Claude Code](https://claude.ai/code)_

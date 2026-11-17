## Next look at #110, head 4a51efc (M2.4a): the answers to the review

**Verdict: can merge. Nothing is open from the review, and nothing blocks.** CI on 4a51efc is green and Codex has no findings on it.

I read `git diff dcf2df3..4a51efc` before the answer, ran my plants again on the new head as far as they still apply (31, all killed; one no longer has a place), ran my scenarios again, and ran one new plant at both heads.

### L1, the group at a discard and at a rollback: fixed in both places

- `discard_uncommitted` (`crates/skynbd-engine/src/lib.rs:1143`) and `rollback` (`lib.rs:1070`) clear `next_group`. Those two and the landed commit are the only places that drop the uncommitted state, so there is no third.
- `rollback` clears it only once its swap has landed, which is right: a rollback that fails changed nothing.
- **A test fails without each.** With the line removed from the discard (plant L1) or from the rollback (plant L2), `a_discard_or_a_rollback_gives_up_the_group` fails, and nothing else does.
- My scenario S1 on the new head: after the discard a commit is a no-op, a snapshot's commit has no group, and the commit after a rollback has none (`scenarios_4a51efc.out`).

### G1, the two plants that passed: both killed now

| Plant | At dcf2df3 | At 4a51efc |
|---|---|---|
| B2 a commit that lands keeps its list of segments PUT early | passed | KILLED by `the_commit_after_one_with_segments_put_early_lists_only_its_own`, and by nothing else |
| B3 a commit that fails forgets the segments PUT early | passed | KILLED by `a_failed_commit_made_again_on_the_same_engine_lists_each_segment_once`, and by nothing else |

The second test is my S4 with asserts: a failure at each of the commit's operations in both modes, the commit made again on the same engine, four segments listed once each with the group, the row, the content after a reopen, fsck, and the swap landed unheard at exactly one point of the after-mode.

### G2, the crash test: yes, that is what I meant, and it is better than what I proposed

- **The assertion.** The upload is run once on a recording store, and the test asserts that there are five PUTs of a segment, that the conditional PUT of the refs object is last, and that each mode has exactly one kill point per recorded operation (`upload_path.rs:382` to `392`, `445` to `449`). I had proposed naming the segment PUTs and the refs write; equality with the recorded list says that and also that no operation is passed over. A change that saves an operation moves both sides, so it no longer trips for that reason alone.
- **A commit with both kinds of segment is under the matrix now.** The upload ends with 512 bytes of a block, so the commit seals and PUTs a segment of its own beside the four PUT early. On the new head the upload is 12 operations, and the commit's own PUT of a segment is the fifth (`scenarios2_4a51efc.out`).
- **A plant that only such a kill shows.** G2: the commit PUTs its own segments after the head swap, so a death between the two leaves a head whose tree names a segment that is not there.

  | | Tests that fail |
  |---|---|
  | At dcf2df3 (`g2_before.out`) | the existing `crash_point_matrix`, `the_swap_and_its_row_land_together_or_not_at_all` |
  | At 4a51efc | those two, the upload's crash test, and the failed commit made again |

  The upload's crash test did not see it before and sees it now.

### N1, `put_sealed` dropped at a PUT: fixed

- **The new shape** (`lib.rs:1555` to `1566`): the first of `sealed_order` is PUT, and once the PUT has ended it leaves `sealed_order` and `pending_segments` and its meta goes to `stored_segments`, three statements with no await between them. So at every await each segment is in exactly one list. A PUT that landed in a call that was dropped before it heard so is PUT again under the same key with the same bytes.
- **The failure path is the same as before**: the `?` ends the call at the first failed PUT, the segments PUT before it have already moved, the rest keep their bytes. `a_segment_whose_put_failed_stays_in_memory_and_is_put_by_the_next_call` passes unchanged.
- **Plants.** The old body put back (N1) is KILLED by `a_put_sealed_dropped_at_a_put_leaves_each_segment_in_one_place` and by nothing else. A segment recorded as PUT before its PUT ends (N2) and a segment that leaves the engine before its PUT (A4) are each KILLED by it and by the failed-PUT test.
- **My scenario S2 on the new head**: 8192 bytes held after the dropped call (it was 12288), and the commit lists 3 segments, 3 distinct (it was 4).
- A remark, nothing to do: `sealed_order.remove(0)` moves the rest of the list for each segment. With the uploader calling after each slice the list is a handful; a `VecDeque` would make it free if a call ever has thousands.

### The docs

All say what the code does. I checked each against the code:

- `set_group`: cleared by a discard and a rollback; `is_clean`, `rpo_is_current`, `rollback` and `gc` ask only whether anything is written.
- `commit_with`: an empty list of ops still commits (my S6 still prints it). "No error proves that nothing landed" is safe to follow and wider than it need be by two: `OpConflict` does prove it, since a transaction that had landed is answered with the ref moved, which is op 0 and reads as `Conflict`; and so does the refusal of an op on the engine's own ref, which comes before anything is written. A driver that reads the row after every error loses one read in those cases and nothing else. I would leave it.
- `put_sealed`: the move per segment, and GC narrowed to this engine's `gc()` with the other handle left to M4.
- `RefStore::transact`: the generation of a CAS that lands, as `compare_and_swap` assigns it.

### The plants on the new head

`review/m2.4a/plants2.py`; results in `plants2.out`. Against the tests of `skynbd-store` and `skynbd-engine`.

| Plant | At dcf2df3 | At 4a51efc |
|---|---|---|
| A1 to A5, `put_sealed` (written again for its new shape) | KILLED | KILLED |
| A6 after a failed PUT, the segments PUT in that call are forgotten | KILLED | not applied: `put_sealed` no longer counts a call's PUTs |
| A7 `put_sealed` goes on after a failed PUT and reports no error (new) | | KILLED by 1 |
| N1, N2 a call dropped at a PUT (new) | | KILLED |
| B1, B4 | KILLED | KILLED |
| B2, B3 | **passed** | KILLED |
| C1 to C5, the group | KILLED | KILLED |
| L1, L2 a discard, a rollback keeps the group (new) | | KILLED by 1 each |
| D1 to D7, the ops and the swap | KILLED | KILLED |
| E1 the shard size at create | KILLED | KILLED |
| F1 to F3, FaultStore's after-mode | KILLED | KILLED |
| G2 the commit PUTs its own segments after the swap (new) | not by the upload's crash test | KILLED by 4, the upload's crash test among them |

Nothing passes.

### CI and Codex

- On 4a51efc every job is green: lint, test, unit-arm64, coverage, s3-backends, e2e-tier-a, the k3s smokes, node B's record and check on both images. No kernel rig runs on it.
- Codex's review state names 4a51efc as reviewed, with no findings.
- Locally, rustc 1.100.0: the tests of `skynbd-store`, `skynbd-engine`, `skynbd-format` and `skynbd-refs` pass on 4a51efc (162 passed, 4 ignored; `upload_path` has 18). I did not rerun clippy or the rest of the workspace; CI did.

### Files

`review/m2.4a/` on `skyzfs-review`, added for this look: `next_look.md`, `plants2.py`, `plants2.out`, `scenarios_4a51efc.out`, `scenarios2.rs`, `scenarios2_4a51efc.out`, `g2_before.py`, `g2_before.out`.

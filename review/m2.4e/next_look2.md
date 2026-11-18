# Last look at #125, at af7b524 (M2.4e)

Diff from da2b697: four commits. They change tests and the status
documentation only:

- `state/tests.rs`;
- `skyzfs-upload/tests/holes.rs` and `skyzfs-upload/tests/common/mod.rs`;
- `skyzfs-ublk/src/status.rs` (doc comments only).

`state.rs` and `driver.rs` are as they were at da2b697, so the code
verdict of `next_look.md` holds.

**Verdict: G1 and G2 are closed, and nothing I found blocks by itself.
It cannot merge yet:**

- the branch conflicts with `zfs-topology`, so no pull-request CI ran
  at af7b524;
- F1 waits on #123;
- Codex has a new P2 at af7b524 that has no answer yet. I checked and
  measured it: it is right (F4 below). By the measure I used for F2 it
  should be fixed, but it does not block by itself. Whether it goes in
  #125 or a follow-up is the lead's call.

## G1: the mismatch in the model (9a5c93d)

The new step is a real model of `abandon_all` and not a tautology:

- **Who is held.** `Rules::mismatch` lets go of every held cut through
  the model's own `abandon`, so the open map ends with the lowest `lo`.
  `check_rules` compares the state's held cuts with that after every
  step.
- **What a cut's map holds.** The model marks the open epoch at a
  mismatch in its own set `whole`. `model_map` counts a cut as whole
  when any epoch in `(lo, number]` is in that set. Before, the test was
  `lo < 1`, which a mismatch never met.
- **What a cut reads.** The content check reads each held cut through
  the state (`Read::Zero` is 0, `Image` is the disk) and compares it,
  block by block, with the volume saved at the cut's instant.
  - A written hole block that reads as zeros fails this check, and that
    is X01.
- **The hole set.** `check_holes` derives Held or Dropped from the held
  cuts' `lo`, and those are checked against the rules. A set dropped
  at the mismatch fails this, and that is X02.

The programs mix in a mismatch about one step in twelve, with holes and
without, at bounds 2 and 64:

- 800 programs, with more than 1000 mismatches asserted;
- 16 programs with a death at every store.

The fixed case writes blocks 1, 33 and 49 into holes before the
mismatch. It then checks that:

- they read as written;
- the rest of their chunks read as zeros;
- a write into a hole after the mismatch makes no copy;
- the release drops the set.

## G2: zeros over old bytes in every block (0301da8)

`move_head_everywhere` fills every block in object storage with a byte
of its own. The test pins the slices as `[8, 8, 0]` blocks, each with
runs, so the last slice is runs alone, which is the case of X13. It
checks that every block lost its old bytes.

## The plants, against af7b524's tests

They were rerun in full after the container restarted. The first run
had stopped with X01 still applied in my worktree; I restored it and
the worktree was clean before the rerun.

| Plant | | Result |
|---|---|---|
| X01 | the open map cleared before the open epoch is marked whole | KILLED (the fixed case and both random tests) |
| X02 | the set dropped at the mismatch | KILLED (the fixed case and the random programs) |
| X11 | the last block of each run of zeros not written | KILLED (the new uploader test) |
| X12 | only a slice's first run written | KILLED (same) |
| X13 | a slice with no block writes none of its runs | KILLED (same) |

At da2b697, X01 and X02 needed my scenarios, X11 and X12 needed U1, and
X13 survived everything. The pull request's own tests now catch all
five.

## 43ff930: `cut_hole_chunks`

The text is accurate against the code:

- `figures()` reads the count word at `holes_base + 1`, and 0 where no
  set was taken;
- the drop stores only the set's word, so the count stays at
  `cut_holes` 2;
- `serve.rs` reports `figures.hole_chunks`;
- a `--zero-fill` image gives a set with 0 chunks.

## F4 (Codex's P2 at af7b524): the next mapped block is looked for to the volume's end in every slice

**Should be fixed, and does not block by itself. Certainty: high**
(checked and measured).

`slice_from` (`state.rs:2472-2515`):

- it starts each call with `mapped = None`;
- at the slice's first hole block it calls
  `maps.blocks_from(.., block, 1)`;
- where the map holds nothing past that block, `blocks_from` loads
  every summary word to the end of the volume.

Take an image with data spread through it and few writes before the
first cut: each slice of 64 blocks then rereads the summary from its own
block to the end, so the cost is the slices times the volume.

My R7 and the pull request's load test both miss this. Each has one
hole with written blocks close together, so the next mapped block is
always near.

Scenario R9 (`state/tests.rs`, not committed):

- an image with one data chunk in each MiB and holes between;
- nothing written before the first cut;
- the cut read in slices of 64 blocks, as the uploader reads it;
- a release build on `MemRegion`, on a machine busy with other builds.

"Passes" is the loads divided by one pass over the open map and the
hole set.

| Volume | Slices | Loads | Passes | All slices |
|---|---|---|---|---|
| 1 GiB | 257 | 71641 | 16.5 | 1.0 ms |
| 4 GiB | 1025 | 384793 | 22.1 | 5.2 ms |
| 16 GiB | 4097 | 3111961 | 44.7 | 42 ms |
| 64 GiB | 16385 | 37613593 | 135 | 0.50 s, 31 µs a slice |

The loads grow twelve times for a volume four times larger, which is
close to quadratic.

At 1 TiB, extrapolated:

- about 1e10 loads in all;
- about two minutes of the queue thread over the upload;
- about 0.5 ms a slice, during which foreground requests wait.

It changes no data. It is the rest of F2, which is the same class: cost
on the queue thread.

A fix would bound each look by the other:

- walk the map's summary and the hole set together, a window at a time,
  and stop at the first block that ends the run;
- or carry the lookahead in the `Slice`.

A test in R9's shape with a bound on the loads, like
`the_slices_of_a_large_hole_read_the_hole_set_once_over`, would pin the
fix.

## CI and Codex at af7b524

- **`mergeable_state` is `dirty`.** #122 and #124 merged into
  `zfs-topology`, and a trial `git merge-tree` conflicts in three files:
  - `bins/skyzfsd/src/serve.rs`: imports;
  - `crates/skyzfs-ublk/src/status.rs`: #124's rewording of the 976 row
    beside the new rows;
  - `crates/skyzfs-upload/src/driver.rs`: #122's `head_of` beside the
    new `write_slice`.

  All three are textual. The base is still status version 16 with
  length 1008, so the new rows at 1008 to 1032 collide with nothing.
- **Pull-request workflows did not run on af7b524.** GitHub skips them
  while the branch conflicts with its base.
  - `skyzfs-ci` on the push is green: gate, lint, test and unit-arm64.
  - The merge with `zfs-topology` brings #122's uploader changes next
    to `write_slice`, so the rigs have to run on that merge before the
    lead merges. That includes the cut rig after #123, for F1.
- **At da2b697:** ci, e2e, coverage, k3s-smoke, the control rig and the
  replay rig are green. The cut rig failed, which is F1, as expected.
  The daemon rig was green on 26.04 amd64 and arm64, with 24.04 still
  running at the time of writing.
- **Codex** has one unresolved finding, F4 above. It came at 20:16,
  after coding's answer at 20:14. Its 0f56669 finding (F2) is answered.

## Before the merge

1. Merge `zfs-topology` into the branch: after #123, as coding plans.
2. Answer Codex's F4 thread, with a fix or a decision by the lead.
3. Get CI and the rigs green on that head, the cut rig among them.

## Review of #110 at dcf2df3 (M2.4a)

**Verdict: can merge. Nothing blocks.** I found one small defect (L1): the group outlives `discard_uncommitted` and `rollback`, so a commit can claim a cut's group it was not made for, which is the thing reading 2 sets out to prevent. No code calls `set_group` yet, so nothing goes wrong on `zfs-topology` with it; it should be settled before M2.4b's driver relies on the setter, and it is two lines and a test, so I would do it here. Two rules have no test (G1), and the crash test's commit never PUTs a segment of its own (G2). The head is right in both places.

I read the diff and the refs crate before the description. Then I ran 26 plants of my own against the tests of `skynbd-store` and `skynbd-engine` (24 killed, 2 pass), and seven scenarios the head's tests do not have (`review/m2.4a/scenarios.rs`, output in `scenarios.out`).

### The four questions

**(1) The readings. I agree with all four, with L1 as the exception to the second.**

- **The group is for the next commit only.** Right. A group that carried over would be read by `head_group()` on a head that is not that cut. A failed commit keeping it is right too: the commit made again is the same cut's. Plants C1 to C3 are killed. What the head does not do is drop the group when the writes it was set for are dropped (L1).
- **A group or ops make a commit with nothing written.** Right, and for the ops it is the only safe reading: a `commit_with` that returned the head and dropped the row would report a cut as recorded that was not. For a group alone it follows from the first reading, since a no-op would leave the group waiting for a commit it was not set for. Plants C4 and C5 are killed. Two consequences to write down: `commit_with(kind, vec![])` with nothing written also makes a commit (scenario S6), and `is_clean()`, `rpo_is_current()`, `gc()` and `rollback()` still ask only whether the overlay is empty, so with a group set they answer "clean" while `commit` is not a no-op.
- **`put_sealed` is a call and `write` stays synchronous; plain `commit` still swaps by `compare_and_swap`.** Both keep SkyNBD's paths as they were. The diff changes no existing test, and they pass.

**(2) The crash test. It kills at every store operation of its upload, before and after each, and the swap is one of them.**

- `FaultOp::Any` with an empty key matches all eight methods of the store, and each goes through `gated` (`fault.rs:148`, `168` to `207`). `k` runs until the upload finishes, so no operation is skipped by construction.
- The upload is 11 operations (S3): four PUTs of segments, the GET of the shard index, three PUTs of nodes, the PUT of the commit, then the GET and the conditional PUT of the refs object. So the refs object's write is the last kill point, and in the after-mode it is the one point where the upload is there unheard (`landed_unheard == 1`).
- What it asserts after each kill is strong: the whole volume compared with before and after, the row present exactly with the new content, the group with it, the upload made again, fsck.
- Plants: the swap and the ops in two transactions, in either order (D5, D6), the head swapped before the commit object is PUT (D7), an after-mode that fails before the store (plant F1) and one that returns the operation's own result (plant F2) are each KILLED by it.
- Two things it does not reach. Its commit PUTs no segment (G2). And every failure in it is a death: a commit that fails and is made again by the same engine, with segments PUT early, is in no test (G1).
- `points > 10` holds with one to spare (11), so a change that saves one operation fails the test for that reason alone. Asserting that the points include the four segment PUTs and the refs write would say more and trip less.

**(3) The generation after `transact`. Safe.**

- All four stores assign `expected.generation + 1` to a `CasRef` inside a transaction, as they do for `compare_and_swap`: `MemoryRefStore` (`skynbd-refs/src/lib.rs:490`), `ObjectRefStore` (`object_store.rs:144`), `RedbRefStore` (`redb_store.rs:127`); `CadencedRefStore` applies to a `MemoryRefStore`, and `ActingRefStore` passes through. The engine always gives `expected: Some(self.head)`, so `+ 1` is exact.
- If a store ever assigned something else, the engine's next commit would expect a value the ref does not have and get `Conflict`. It fails closed: no wrong head, an engine that must be reopened.
- Plant D1 (the old generation kept) is KILLED by `a_commit_with_ops_lands_them_with_the_head_swap`, on `MemoryRefStore`. S5 shows three commits with ops and one without following each other on `ObjectRefStore` and on redb.
- Suggestion: the trait states the rule only at `compare_and_swap` (`skynbd-refs/src/lib.rs:184`). One sentence at `transact` would make it the contract the engine's comment says it is.

**(4) `put_sealed`'s failure path. Right as built.**

- Stopping at the first failed PUT is the right choice: the segments keep their seal order, a store that is down is not asked again for each remaining segment, and what was PUT stays recorded. Plants A4 (the failed segment's bytes dropped) and A6 (the segments PUT in the failed call forgotten) are KILLED.
- A PUT that landed and whose reply was lost leaves no orphan: the segment keeps its bytes and is PUT again under the same key with the same bytes.
- Orphans come only from a death or a discard after a `put_sealed`. They are harmless to readers: nothing names them, and GC decides by what a ref's tree reaches (`maintenance.rs`, `closure`), not by a commit's list. They cost their size until M4's GC, as the description says.
- One thing the doc comment says more broadly than holds: "GC refuses to run while this engine has uncommitted writes". That is this engine's `gc()`. A GC through another handle on the volume would sweep segments PUT early and not yet committed (by reading: the sweep deletes every `seg/` key no ref's tree reaches), and the commit would then land on a tree that names them. Before this change that window was one `commit` call; now it is the whole upload. Nothing runs GC on a SkyZFS volume today, so this is for M4's row: GC on a volume with an upload in progress must hold the lease or wait.

### L1 (defect, small). The group outlives the writes it was set for

`set_group` (`skynbd-engine/src/lib.rs:1572`) says the group "is for that commit alone", "so that no commit claims a cut's group it was not made for". `discard_uncommitted` (`lib.rs:1133`) and `rollback` (`lib.rs:1034`) clear the overlay and the segments and leave `next_group`.

Scenario S1, on the head:

1. `set_group(Some(41))`, a write, `put_sealed`, then `discard_uncommitted()` (the cut is given up).
2. `is_clean()` is true.
3. `commit(Flush)`, or `snapshot_create`, with no group set for it: a new commit is made, its group is 41, `head_group()` is 41, and the volume holds the content from before the cut.

The same after `rollback`: the group set before it goes into the first commit after it.

Who meets it: a driver that sets the group when it begins a cut and discards on a failed read (the M2.4 row abandons the cut there), followed by anything that commits without setting a group, for example a publish that goes through `snapshot_create`. The row `skyzfs/cut` is the authority on what the head holds, so the harm is a commit whose `group` field is wrong, not a wrong upload. Certain: S1 prints it.

Fix: clear `next_group` in both, with a test. If the group is meant to survive a discard (the same cut begun again), then the doc has to say so and the driver has to clear it when it gives a cut up; I would not choose that.

### G1 (gap). Two rules about the list of segments PUT early have no test

Each plant passes every test of `skynbd-store` and `skynbd-engine`:

| Plant | What it breaks | What catches it |
|---|---|---|
| B2: a commit that lands keeps `stored_segments` (`lib.rs:1782` removed) | every later commit of that engine lists those segments again, and the list grows for the engine's life | S7: `put_sealed`, commit, a write, commit; the second commit lists 3 segments where it should list 1 |
| B3: a commit that fails forgets them (`mem::take` at `lib.rs:1621`) | the commit made again lists none of the segments PUT early, so they have no length and checksum on record | S4: a commit that fails at its first operation and is made again lists 2 segments where it should list 4 |

Neither is a wrong read: the tree names the segments either way. What is lost is the commit's record of its segments (design D9), which `put_sealed`'s doc promises ("the next commit lists it"). `gaps.out` has both plants against the scenarios. S7 is ten lines; S4 is the crash test's loop with the engine kept instead of dropped, and it also covers G2.

### G2 (gap). The crash test's commit PUTs no segment

`upload` (`upload_path.rs:218`) writes whole blocks only, and the seal threshold is one block, so every segment is PUT by `put_sealed` and the commit starts with none pending (S3: 0 PUTs of a segment by the commit). A commit that lists segments PUT early and PUTs others itself, killed at each operation, is not in the matrix. `sealed_segments_are_put_before_the_commit_and_leave_memory` has that commit once, without a kill.

S4 runs it: two segments PUT early, one sealed and not PUT, part of a block open, a failure at each of the commit's 9 operations in both modes, the commit made again on the same engine. It lists its four segments once each and reads right after a reopen. So the head is right; a part of a block at the end of `upload` would put it under the pull request's own matrix.

### N1 (suggestion). `put_sealed` dropped while a PUT is outstanding

`put_sealed` (`lib.rs:1540`) records each segment in `stored_segments` as it is PUT and removes them from `sealed_order` only after the loop. If its future is dropped at a PUT (a time limit around the call, a `select!`), the segments already PUT are in both lists.

S2: three sealed segments, the second PUT never answers, the call is dropped; then `put_sealed` again and a commit. The engine still holds all 12288 bytes after the first call although one segment is in the store, and the commit lists 4 segments, 3 of them distinct. The content reads right and fsck passes, so this is a duplicate entry and a second PUT of the same bytes, not a wrong cut.

`commit` is safe against this: it changes nothing in the engine until the swap has landed. Moving one segment at a time (PUT, then pop it from `sealed_order` and push its meta, with no await between) makes `put_sealed` the same. Worth doing before M2.4b puts a time limit around a call to the store.

### For M2.4b and M2.4c, nothing to do here

- **An error from `commit_with` does not say whether it landed.** S4, after-mode at the refs write: the commit again is a `Conflict`, and the head is the first attempt's commit. With `S3Store` there is a second way to the same place, by reading (`s3.rs:242`, the SDK's retries): a conditional PUT that landed and is sent again answers 412, `ObjectRefStore` reads again, and the engine gets `Conflict` for a commit that is the head. The driver must read the row after either. The engine could tell by itself: the ref then names this commit's own hash.
- **Which ref store.** The crash test's is `ObjectRefStore`, and that is the one the daemon should use. On `CadencedRefStore` a plain `commit`'s head swap stays in its cache (`cadence.rs:43`, `head` is deferred); a transaction with a meta row is exported at once. An upload must not take a deferred swap for "committed in object storage", since the intent map's bits are cleared on that.
- **What a commit costs in a fragmented shard.** `write_shard_nodes` PUTs every leaf of a touched shard again, one after the other (`lib.rs:1810`), changed or not. With 128 MiB shards in which every block is its own extent that is 128 leaves and an internal node per touched shard per cut. By reading only; M2.T4's figures will say whether it matters.

### My plants

`review/m2.4a/plants.py`; results in `plants.out`.

| Plant | Result |
|---|---|
| A1 `put_sealed` counts a segment as stored without a PUT | KILLED by 3 |
| A2 `put_sealed` does not record what it PUT | KILLED by 2 |
| A3 `put_sealed` keeps the bytes of what it PUT | KILLED by 2 |
| A4 `put_sealed` drops the bytes of the segment whose PUT failed | KILLED by 1 |
| A5 `put_sealed` seals and PUTs the open segment too | KILLED by 1 |
| A6 after a failed PUT, the segments PUT in that call are forgotten | KILLED by 1 |
| B1 the commit lists the segments PUT early after its own | KILLED by 1 |
| B2 a commit that lands keeps its list of segments PUT early | **passes** (G1) |
| B3 a commit that fails forgets the segments PUT early | **passes** (G1) |
| B4 a discard keeps the list of segments PUT early | KILLED by 1 |
| C1 the group goes into no commit | KILLED by 6 |
| C2 a commit that lands keeps the group for the next | KILLED by 1 |
| C3 a commit that fails loses the group | KILLED by 2 |
| C4 a group with nothing written makes no commit | KILLED by 1 |
| C5 ops with nothing written make no commit | KILLED by 1 |
| D1 the head held after a commit with ops keeps the old generation | KILLED by 1 |
| D2 the index of an op that did not hold counts the swap | KILLED by 1 |
| D3 with ops, a moved ref is reported as an op that did not hold | KILLED by 1 |
| D4 the ops may swap the ref the engine commits to | KILLED by 1 |
| D5 the swap, then the ops in a transaction of their own | KILLED by 3 (the crash test among them) |
| D6 the ops in a transaction of their own, then the swap | KILLED by 3 (the crash test among them) |
| D7 the head is swapped before the commit object is PUT | KILLED by 4 (the crash test and the existing `crash_point_matrix` among them) |
| E1 create takes any shard size | KILLED by 1 |
| F1 the after-mode fails the operation before the store | KILLED by 5 |
| F2 the after-mode returns the operation's own result | KILLED by 5 |
| F3 the after-mode does not poison | KILLED by 1 |

I did not rerun the description's seven plants; A2, A3, B4, C2, C4, C5, D5 and E1 are the same faults from my side.

### The shard size

- `MAX_SHARD_EXTENTS` is what `write_shard_nodes` enforces (256 leaves of 256 extents), and the claim in `EngineConfig::shard_bytes` holds: with every write a multiple of 4 KiB at a multiple of 4 KiB, a shard of 256 MiB has at most 65,536 extents. The engine's own unit is the 512-byte sector, so the claim rests on the daemon writing whole blocks.
- The check at create is right: a shard of zero divided by zero before (`lib.rs:545`).

### CI and Codex

- On dcf2df3 every job is green: lint, test, unit-arm64, coverage, s3-backends, e2e-tier-a, the k3s smokes, and node B's record and check on both images. No kernel rig runs on it, as the description says, since no daemon crate changed.
- Codex reviewed dcf2df3 and has no findings.
- Locally, rustc 1.100.0: the tests of `skynbd-store`, `skynbd-engine`, `skynbd-format` and `skynbd-refs` pass on the head. I did not rerun clippy or the workspace's other crates; CI's lint and test jobs did.

### Files

`review/m2.4a/` on `skyzfs-review`: `review.md`, `plants.py`, `plants.out`, `scenarios.rs`, `scenarios.py`, `scenarios.out`, `gaps.py`, `gaps.out`.

## Next look at #114 at ac9d128 (M2.4b-2)

**Verdict: it can merge. Nothing blocks.** Every finding of the review at 6c97063 is answered and I checked each answer against the code. The commit changes no behaviour. Ten of the plants that passed at 6c97063 now fail a test of the head, and each new test fails when its own condition is taken away. One correction, to the author's answer on G7 and to my own review: a conflict with the row and head read as before does not need head to move and move back. It is safe as built, and it is a matter for M2.4c, where the lease comes in (G7 below).

The review is `review.md` in this directory. This look is on `git diff 6c97063..ac9d128`: one commit, seven files, 369 lines added and 34 removed.

### What the commit changes

Only comments in the product's sources. I took every changed line of `crates/skyzfs-local/src/inplace.rs`, `crates/skyzfs-upload/src/driver.rs` and `crates/skyzfs-upload/src/local.rs` that is not a comment line: there is none. The rest is tests: `crates/skyzfs-upload/tests/common/mod.rs` (a helper `labels` split out of `group`), `tests/crash.rs`, `tests/driver.rs` and `crates/skyzfs-local/src/inplace/tests/cuts.rs`.

The head's tests pass here (rustc 1.100.0): plain, `skyzfs-local` 270 (1 ignored) and 11, `skyzfs-upload` 10, 3 and 22; with `--features skyzfs-local/test-hooks`, 270 and 15, then 10, 3 and 22. Output in `t2_plain.out` and `t2_hooks.out`. The description's count of 35 upload tests is right.

### The answers, finding by finding

**D1 (a naming whose write fails has changed something): corrected, and the new text is true.** `inplace.rs:3166` to `3179` now says false with nothing changed for the three refusals only, and that where the pages cannot be written the volume fails and the map in memory names the cut all the same. `driver.rs:104` to `113` says the same of `named`, and that the instance stops at the next cut's release if the map still cannot be written. That is what S2 shows.

**D2 (a named whole cut gives a whole heir): corrected.** `inplace.rs:3197` to `3200`, `local.rs:95` to `97` and `driver.rs:29` to `30`. That is what S3 shows.

**L1 (what `Intent.commits` counts): corrected.** `inplace.rs:1002` to `1007` now names each release, the first or one made again, and each naming. Plant L1 (a naming does not count) still passes every test and T1, as I expected: a naming clears no bit, so nothing can observe whether it counts. The comment says it clears none. Nothing to do.

**G1 (an engine with a cut's writes opened again at a moved head): a test, and it holds.** `tests/driver.rs:441`, `a_failed_commit_on_a_head_or_row_another_writer_moved_is_given_up`, in two cases: the other writer moves head, and the other writer moves the row. Plants U5, U7 and U26 fail the first case and U7b the second; all four passed at 6c97063. Its opposite, V1 (the other writer moves nothing), fails the test too, so it is not passing for want of the condition.

**G2 (a cut whose flush is under way): a test.** `tests/driver.rs:848`, `a_cut_whose_flush_is_under_way_is_not_uploaded`. Plant U9 (a cut is offered whatever its phase) now fails it. V2 (the image's sync is not held) fails it.

**G3 (the cut-state matrix did not fix its count): fixed.** `tests/crash.rs:165`, `stores_of_the_run`, counts the stores of a run that is not killed and asserts that no other step stores; the matrix then asserts `points == stores` and that the cut is held again for the row at exactly the three points around the two releases. M2 (the loop skips its first point) and M3 (it stops after three) now fail. A point skipped at either end or in the middle fails the count.

**G4 ("other failures still upload"): a test.** `tests/driver.rs:899`, `a_cut_held_when_the_volume_fails_at_a_write_of_the_image_is_still_uploaded`. Plant U32 now fails it. Two more: V3 (the image's writes do not fail) fails it, and U33 (a failed volume offers no cut at all) fails it and nothing else, so this is the one test that holds the half of F2 that says an upload goes on.

**G5 (the count of PUTs before the commit took in the commit's own): fixed.** `tests/crash.rs:143` to `159`: the run is stepped, and the count is of the segment PUTs made before the driver first says it is committing. U18+19 (no PUT of sealed segments before the commit at all) is now caught by two tests where it was caught by one. U18 alone and U19 alone pass, as the author says and as I expected: each call covers the other. One more of the family passes, V6 (no PUT after the last slice, so the last slice's segments wait for the commit): `early >= 4` still holds, since the earlier slices seal four. The author's description says what is not seen there. A remark, no more.

**G6 (a release made again must count): a test.** `inplace/tests/cuts.rs:3919`, `a_release_made_again_after_a_death_counts_for_a_write_that_waits`: T1, with two asserts more (the syncs, and that the cut was named once). L2 now fails it; so does L2b. V4 (the map's sync is not held under the waiting write) and V4b (the release is made again before the write waits) each fail it.

**G7 (a conflict with the row and head read as before): not done, and the reason given is not the whole of it.** See the next section.

**N1 to N3** are with the lead.

### G7: a conflict with nothing moved

The author's answer: such a conflict "means head moved and moved back between the failed commit and the read", there is no cheap way to make that on this store, and the condition is on the safe side. My review said the same ("I think the condition cannot be met unless head moves and moves back"). The last part is right and the first is not: I was wrong there.

There is a way with nothing moved at all. A refs store that fences every write under a holder and an epoch (`skynbd_refs::ActingRefStore`, `crates/skynbd-refs/src/acting.rs`) refuses a write to a volume that has no lease row with `LeaseLost` and an empty holder (`crates/skynbd-refs/src/lib.rs:103`). The engine maps exactly that to `EngineError::Conflict` (`crates/skynbd-engine/src/lib.rs:67`; a lease another holder has maps to `Fenced`, line 64), and the uploader maps it to `RemoteError::Conflict` (`crates/skyzfs-upload/src/remote.rs:47`). Head and the row have not moved, so the read after the failed commit finds them as before, and the branch at `driver.rs:497` is taken on its first operand alone.

S9 (added to `scenarios.rs`) makes it: a `Driver` on an `EngineRemote` whose refs store is an `ActingRefStore` over the fixture's store, on a volume with no lease row, one cut of 45 blocks, four runs.

| | run 1 | run 2 | run 3 | run 4 |
|---|---|---|---|---|
| As built: the run ends | `Err(Conflict)` | `Err(Conflict)` | `Err(Conflict)` | `Err(Conflict)` |
| segment PUTs in the run | 5 | 5 | 5 | 5 |
| segment bytes in the store after it | 180224 | 360448 | 540672 | 720896 |
| commits given up so far | 0 | 1 | 2 | 3 |
| With plant U6 (the commit is made again): segment PUTs | 5 | 1 | 1 | 1 |
| segment bytes in the store after it | 180224 | 180224 | 180224 | 180224 |

Head and the row are as before after every run, and the cut stays held: nothing wrong lands, in either variant. What the table shows is the cost as built. Each cycle gives the commit up, resets the engine, uploads the whole cut again under new segment names and is refused again: 180 KB of orphans per cycle for a cut of 45 blocks, without end while the lease row is absent. A lease that another node holds does not take this path: it is `Fenced`, not a conflict, and the commit is retried with nothing uploaded again.

What I make of it:

- **It does not block this pull request.** Nothing here passes a fencing refs store to the uploader; the wiring is M2.4c. As built it is safe.
- **It is a decision for M2.4c, so for the lead.** Whether a volume whose lease row is absent can be uploaded to at all, and what the uploader does then. As built it uploads the cut again each cycle. Two directions, neither of which I ran beyond the table: the engine's refusal for want of a lease row need not read as a conflict to the uploader; or `observe` could compare head's generation as well as its hash, and the driver make the commit again where nothing moved.
- **The author's case, head moved and moved back, is handled right as built.** `observe` compares the commit's hash (`remote.rs:158`), and the engine's swap expects the head it opened at, generation included. After a head that moved and came back to the same commit, the engine's expected head is stale, so a commit made again would be refused again each time; giving up and opening the engine again is what gets out. I reasoned this from the code and did not run it. It is the reason plant U6 is not simply the better behaviour.
- **A test is now cheap,** through `ActingRefStore`: S9 is offered for it, with whatever the lead decides as its assertion.
- **The description's sentence** ("it needs head moved and moved back", in "what is not done") should say there is a second way. Not blocking.

### The plants on ac9d128

`plants2.py` is `plants.py` with the new plants; `plants_ac9d128.out` has every result, `plants2_detail/` the scenario outputs. The 50 plants of the review ran again (52 runs), each text still applied, and 7 new plants and one rerun of U6 with S9: 60 runs, 53 killed, 7 pass.

| | at 6c97063 | at ac9d128 |
|---|---|---|
| U5, U5s (no reset where a commit is given up) | pass | KILLED by the G1 test |
| U7 (a failed commit made again though head moved) | pass | KILLED by the G1 test |
| U7b (the same, though the row moved) | pass | KILLED by the G1 test |
| U26 (`observe` opens an engine with writes again at a moved head) | pass | KILLED by the G1 test |
| U9 (a cut is offered whatever its phase) | pass | KILLED by the G2 test |
| U32 (a cut let go on a failed volume is reported as kept) | pass | KILLED by the G4 test |
| M2, M3 (the cut-state matrix skips points) | pass | KILLED by the matrix |
| L2 (a release counts only where it moves the base) | pass | KILLED by the G6 test |
| U18+19 (no PUT of sealed segments before the commit) | KILLED by 1 | KILLED by 2 |
| U6 (a conflict is made again where nothing moved) | pass | pass (G7) |
| U18 alone, U19 alone | pass | pass, as expected |
| L1 (a naming does not count as a commit of the map) | pass | pass, as expected |
| L3s (L3 against the crash tests alone) | pass | pass; L3 itself is KILLED by a store test, and S5 fails with it |
| the other 36 | KILLED | KILLED, each by the tests that killed it before, and nine of them by more |

New on this head:

| Plant | Result |
|---|---|
| V1. TEST: in the G1 test the other writer moves neither head nor the row | KILLED by that test |
| V2. TEST: in the G2 test the image's sync is not held | KILLED by that test |
| V3. TEST: in the G4 test the image's writes do not fail | KILLED by that test |
| V4. TEST: in the G6 test the map's sync is not held under the waiting write | KILLED by that test |
| V4b. TEST: in the G6 test the release is made again with no write waiting | KILLED by that test |
| U33. A failed volume offers no cut to the uploader | KILLED by the G4 test alone |
| V6. No PUT of sealed segments after the last slice | pass (see G5) |
| U6s. U6 with S9 | pass; S9's figures are in the table above |

The author's list of plants killed (U26, U5, U7, U7b, U9, U32, L2, M2, M3, U18+19) is confirmed, one by one. The checkout was clean after the runs.

### The scenarios

The nine of the review (S1 to S8 in `scenarios.rs`, T1 in `scenarios_store.rs`) pass on ac9d128 with the figures they had: S1 counts 35 operations on object storage and 70 and 21 kill points, with the cut held again at stores 0, 13 and 14; S5 runs 1200 random programs with none wrong. S9 is new and is described above. Outputs: `scenarios_ac9d128.out`, `scenarios_store_ac9d128.out`.

### The description

Updated for this head: the commit in the list, the criteria rows for G1, G2, G4 and G6, the test counts, the table of plants, and G7 and the PUT remark under what is not done. Two things left: the G7 sentence (above), and the CI links for ac9d128, which it says will follow.

### CI and Codex, read once at 03:57 UTC on 18 November

- **CI on ac9d128:** 39 check runs. Every one that had ended had succeeded, or was skipped by its own condition; among them lint, test, unit-arm64, coverage, e2e-tier-a, "through skyzfsd" on 24.04 and 26.04, and "ubuntu-26.04, daemon" and "ubuntu-26.04-arm, daemon". **One job was still running: "ubuntu-24.04, daemon".** The pull request's state read "unstable" for that reason. I read conclusions only, not the rigs' tables.
- **Codex:** its status comment names ac9d128 for the code review and the security review, both completed, no findings. There are no review comments and no reviews on the pull request.

### What I could not verify

- Nothing that needs a kernel runs here. I did not read the rigs' summaries, and one daemon-rig job had not ended.
- I did not run clippy or fmt on this head; CI's lint succeeded.
- S9 uses `ActingRefStore` as a stand-in for whatever refs store M2.4c passes to the uploader. What M2.4c will pass is not in this pull request, and whether a node can come to upload with no lease row is the lead's to say.
- Whether anything collects the segments an upload given up leaves behind: nothing in this pull request does (N3 of the review).
- The case of a head that moved and came back to the same commit is reasoned from the code, not run.

### Files

- `next_look.md`: this.
- `plants2.py`, `plants_ac9d128.out`, `plants2_detail/`: the plants on ac9d128.
- `scenarios.rs` (now with S9; the commit before this one has it as the review had it), `scenarios.py`, `scenarios_ac9d128.out`, `scenarios_store_ac9d128.out`.
- `t2_plain.out`, `t2_hooks.out`: the head's tests on ac9d128.
- `review.md` and the rest: the review at 6c97063, unchanged.

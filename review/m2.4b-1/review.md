## Review of #112 at 88e4edc (M2.4b-1)

**Verdict: can merge. Nothing blocks.** I found no defect in the code. What I have is one point for the lead and M2.4b-2 (P1: the bound can take the cut between its commit and its release, with no death, and the store then offers the uploader nothing but a mismatch), four gaps in the tests that my plants pass through and my scenarios close (G1 to G4), and a few suggestions (N1 to N4).

I read the diff, design §5.2 and the notes' M2.4 rows before the description. Then I ran 51 plants of my own against the tests of `skyzfs-local` with the test hooks (PENDING killed, PENDING pass), and ten scenarios the head's tests do not have (`scenarios_state.rs`, `scenarios_store.rs`, `scenarios_real.rs`, run by `scenarios.py`; output in `scenarios.out`). All ten pass on the head. The head's own tests pass here plain (261 and 11) and with the test hooks (261 and 14), rustc 1.100.0.

### The three questions

**(1) `abandon_all`: safe at every store, and the death test's count is right, a flush under way included.**

- **Every store.** Scenario S1 dies at each store of `abandon_all` (35 with a flush under way, 25 without) and checks more than the head's test does: every held cut, read as the region is at the death and again after the start has taken it up, reads each block as the volume was at that cut's instant, lacks no block that changed since the cut it is made against, and the cuts chain from the uploaded cut. The daemon then serves on (writes, a new cut) before the uploader checks again, and the second `abandon_all` ends as one with no death does. All right at every store.
- **Why the order is right.** Each cut goes through `abandon`, which hands its map on and never drops it, so no store leaves a cut that lacks a change. The whole mark is one store and comes last (`state.rs:1549`). A death before it leaves a state with no mark, in which the next cut holds what the abandoned cuts held; the uploader's next check finds the same mismatch (nothing it compares has changed) and marks it. That rests on M2.4b-2 checking before every upload, as the description says it will.
- **The whole cut.** With Q6 a whole cut is uploaded whatever the row names, so whether a cut reads as whole after a death is what the uploader's decision rests on. Scenario S2 starts with nothing uploaded (the oldest cut is whole) and dies at each of the 32 stores: the oldest cut held is whole at every one, and where none is left the next cut made is whole, with or without a second `abandon_all`. So the head has the property its comment claims ("a held cut that counted the whole volume does so until it is gone"). No test of the head holds it (G1).
- **The count with a flush under way.** Right. The start takes its sync for cut 4's flush, and cut 4 then replaces cut 3 if cut 3 still waits. That is the replacing of normal running, which is not counted there either. S1 shows the exact rule at every death: at the start, `counted + held == 3 − merged`, where `merged` is 1 exactly when the region at the death held cut 3 as waiting and cut 4 as flushing (7 of the 35 deaths), and after the second `abandon_all` every cut left is counted once more. So the total is 2 exactly at those 7 deaths and 3 at the other 28. The head's test accepts 2 or 3 at any death (`state/tests.rs:2023`), which is looser than what holds (N3), but the plants on the count are killed all the same (A5, A8, A9).
- **A flush under way at the store.** Scenario T1 holds the image's sync, calls `abandon_all_cuts` with cut 3 flushing and a write after it in flight, then lets the flush end, well or with a failure. Both cuts are counted, the flush that ends changes nothing, and the pool's next group makes a cut that is whole and reads the volume as at its instant under writes, in slices.

**(2) `release_cut`: the new order is right, and the kill-point test is enough for check (a).**

- **The order.** With the old order a death after `state.release` and before the map's pages leaves the cut gone and a map that names the cut before it: a mismatch, and a whole upload. With the new order (`inplace.rs:3119` to `3132`) a death leaves either the cut held as the one being uploaded, under the same state's name and with its group, which is what check (a) compares with the row; or the cut gone and the map naming it. Releasing twice is harmless: the second `intent_committed` names the same base and clears a subset.
- **The test.** `a_death_after_the_commit_leaves_the_cut_to_release_or_named_as_committed` (`tests/cuts.rs:4000`) asserts just that at each store, then releases again and checks the base and the next cut's `lo`. Plant R1 (the old order) is KILLED by it and by nothing else, so it is the one test that holds the order. R2 (a waiting cut named as committed), R3, R4 and R8 (the state's name in the header) are killed too.
- **What it does not reach, none of it a hole.** Its deaths are at the region's stores, and all of those come after the map's pages are written, so a death before or between the page writes is not in it. Such a death leaves the cut held and a base that is the old one or the new one, which is the state before `release_cut` or the first branch of the test. It runs on the caller's thread only; `write_intent` is synchronous in every mode, so the modes do not differ here.
- **One sentence of the comment is not true on one path.** "Never a cut gone that the map does not name" (`inplace.rs:3116`): when the pages cannot be written, the volume fails and `state.release` runs all the same (`inplace.rs:3167`, then `3129`). The next start then finds a mismatch and uploads the whole volume, which is safe. Returning before the release when the write failed would keep the cut for check (a). A suggestion, since the volume has failed either way.
- I agree with Q7 and with what the description says of it: the header is written without a sync, and a power cut that loses it costs one whole upload. See P1 for a second way into that state.

**(3) The runs of `read_cut_from`, and version 1 of the cut state: both right. The tests hold less of `follows` and of `blocks_from` than the code does.**

- **The runs.** `read_places` (`inplace.rs:2999`) fills the buffer in the order of the blocks and joins two neighbours only when `follows` says they are the same file and the next number (`inplace.rs:2198`). Scenarios T4, T6 and T7 give it the layouts the head's tests lack: two blocks apart whose copies are in adjacent slots (one read, rightly), adjacent blocks with their copies the other way round, adjacent blocks whose slots have a gap, a copy whose slot is the number after an image block's, and image blocks two apart. Every block of every slice, in slices of 1 to 64 blocks and in each I/O mode, is what the cut reads a block at a time, and the reads are as many as the runs.
- **The gaps.** Three plants in `follows` pass the head's tests: a copy taken to follow an image block (C1), a copy two slots on (C6), an image block two blocks on (C7). Each makes a slice hold another block's content. See G2.
- **`map_from` and `blocks_from`.** Right by reading and by scenario S3 (a volume of 12388 blocks, four words of summary; 245 slices from many blocks and limits, a whole cut, and a map in two parts after `abandon_uploading`, each equal to what `map()` lists). Every test volume of the head has 200 or 256 blocks, which is one word of summary, so the code for a summary word after the first is run by no test: plants C11 and C12 pass. See G3.
- **Version 1.** `version_in` (`state.rs:406`) changes only what the start says: a file of another length is made again by `Mapped::open` either way, and a file of version 1 with version 2's length is refused by `check()` with `Version(1)`. Scenario R1 writes thirteen headers on real files (version 1 two words shorter, of full length, a byte longer, cut to 16, 15 and 8 bytes, empty; version 2 shorter; versions 0, 3 and `u64::MAX`; another magic). In each the start begins a new state, the file has its full length again, and the start after takes the new state up. The start names the version where the file has the magic and 16 bytes, and says "the region was not there with the volume's length" otherwise. Nothing odd.
- **The intent map.** The version is read before the checksum (`intent.rs:296` to `306`); the checksum covers the new field, since it is the page's. V5 and V6: PENDING.

### P1 (for the lead and M2.4b-2; not a defect of this pull request). The bound can take the cut between its commit and its release

The description says of Q8 that a row of the same state with a higher number than the map's base is "our own later commit that the map does not name, which only such a power cut leaves now that a release writes the map first". There is a second way, with no death.

- **Scenario (T5, runs on the head).** Cut 1 is released, so the map names it. Cut 2 holds 72 blocks and is being uploaded. Its commit lands in object storage. Before `release_cut(2)` is called, the pool writes over those blocks, the copies pass the bound (64 blocks in the tests), and the bound abandons cut 2: its map goes to the open map or to the cut that waits. `release_cut(2)` now returns false (`inplace.rs:3120` to `3127`: the cut is not held), the map still names cut 1, and the next cut has `lo` 1. The row names cut 2. At the next check that is rule 4, a mismatch: every cut abandoned, the whole volume uploaded.
- **How wide the window is.** From the moment the head swap lands at the store until the queue's thread calls `release_cut`: the rest of the commit's round trip. The bound takes the cut being uploaded first, and only under a load that fills the bound during an upload, so it is rare. But it is a running daemon's path, not a power cut's, and its cost is the largest there is; a whole cut under that same load is also the cut most likely to meet the bound again.
- **Nothing is wrong in object storage or in the cut state.** The next cut's map holds everything that changed since cut 1, which is more than what changed since cut 2, so uploaded onto the row it would be right.
- **Two ways out, for the lead to choose.** Either the relaxation Q8 names (same state, a row newer than the base, and a cut whose `lo` is no newer than the row), or a call beside `release_cut` that names a committed cut in the map when the cut is no longer held. The second is sound by the map's own argument: `changed_after(2)` with cut 2 gone lists the open map and every newer cut, and cut 2's map went to one of them. It also keeps the check strict.
- My certainty: high for the scenario (it runs); the frequency is an estimate.

### G1 (gap). No test holds that the whole mark comes last

Plant A1 stores the mark first, before any cut is abandoned, and every test of the head passes. Scenario S2 fails with it at the second store: "cut 1 (lo 0) is not whole and is not made against None", that is, the cut being uploaded stops being whole while it is still held. With the uploader's check as Q6 has it I could not make that into a wrong upload (the cut is then judged by rule 2 and abandoned), so the gap is in the tests and not in what a daemon would do. But the order is one of the description's four readings and the comment gives a reason for it; a death test that starts from a whole cut, as S2 does, would hold it.

### G2 (gap). `follows` is held for one of its four cases

`state.rs` and `inplace.rs` are right; the tests would pass these three changes to `Place::follows` (`inplace.rs:2198`):

| Plant | The head's tests | Caught by |
|---|---|---|
| C1: a copy follows an image block (and the reverse) where the numbers follow | pass | T6: "block 1 of cut 2 in slices of 2 holds 0x48" |
| C6: a copy two slots on joins the run | pass | T4: PENDING |
| C7: an image block two blocks on joins the run | pass | T7: PENDING |

Each is a slice that holds another block's content, which an upload would commit. The layouts are small: blocks 0, 1 and 2 with copies of 0 and 2 in slots 0 and 2 (T6); copies made in an order that is not the blocks' (T4); a cut of blocks 10, 12, 14, 15 and 17 (T7).

### G3 (gap). `blocks_from` past the first word of summary is run by no test

`maps.rs:277` to `316`. Plants C11 (the summary read from its first word whatever block is asked) and C12 (the mask applied to the summary's first word and not to the word of the block asked) pass the head's tests, because no test volume has more than 4096 blocks. S3 fails with both ("cut 2 from block 4096, 1 blocks", "from block 8190"). With C11 a slice from a block past 4096 would start again at the map's beginning: an upload that never ends, or blocks sent twice. A real volume has thousands of summary words, so this is the path production runs. One test of `map_from` on a region of more than 4096 blocks closes it. C13: PENDING.

### G4 (gap, small). What the start says for a file of this version with another length, or without the magic

Plants V2 (the guard `version != state::VERSION` removed at `inplace.rs:2351`) and V3 (the version read whatever the magic, `state.rs:415`) pass. With V2 a file of version 2 whose length differs (another bound, a volume resized) would be reported as "of version 2, and this daemon reads version 2". R1: PENDING.

### N1 (suggestion). A slice into a buffer that is not aligned abandons the cut

`read_cut_from` takes `&mut [u8]` and checks its length (`inplace.rs:2957`) but not its address. With direct I/O a buffer off a block's alignment makes the read fail with `InvalidInput`, the store takes that for a read that failed, and the cut being uploaded is abandoned and counted (T3: held before `[(1, Uploading), (2, Waiting)]`, after `[(2, Uploading)]`, abandoned 1). `read_cut` with the same buffer fails and abandons nothing. A caller's mistake should cost an error and not the cut: refuse it with the length check, or take a `Buf`. I saw this on the simulated disk, which checks the address as the kernel does for direct I/O; I did not try it on a real file.

### N2 (suggestion). The death test could assert the exact count

`state/tests.rs:2023` accepts 2 or 3. What holds is: 2 exactly when the region at the death held cut 3 waiting and cut 4 flushing, 3 otherwise; and at the start `counted + held == 3 − merged`. The test has the region at the death (`CutState::view`), so it can say which.

### N3 (suggestion). The version-1 test on real files

`a_cut_state_of_version_1_is_refused_for_its_version` (`tests/inplace_real.rs:833`) is the only test that kills V1 (the header not read before the region is made again). It is built only with the test hooks, and it returns without a word where the file system has no direct I/O. It ran here. A line printed when it is skipped would show in CI's log whether it ran there.

### N4 (note). After a mismatch on an idle pool nothing is uploaded until the pool's next group

`abandon_all` marks the open epoch; the whole cut is made at the pool's next group. If the pool is idle, object storage stays at the row that did not match until a write comes. That is by design as I read it, and M2.4c's status will show it (mismatched, nothing held). For the lead to confirm that nothing should make a cut here.

### My plants

51 plants, each planted alone in a checkout of the head, against `cargo test -p skyzfs-local --features test-hooks`. PENDING table.

One run failed in a test the plant cannot reach: `no_wake_is_lost_between_a_threads_look_and_its_wait` (`inplace/pool/tests.rs:140`, not in this diff) failed once in some forty runs, under plant C2, on a machine with a load of about 20. It is a timing test; I report it and did not look further.

### CI and Codex

- CI on 88e4edc: 34 checks succeeded, 2 skipped (the badge job and the replay rig's reference jobs, which a pull request that touches only the daemon's code does not run). The daemon rig, the control rig and the replay rig's daemon jobs are among the green ones. I read the conclusions and not the rigs' tables.
- Codex reviewed 88e4edc and reports no finding; it left no inline comment.

### What I could not verify

- Anything through the kernel: the rigs run in CI only. What a failed sync of the copies file does on a real file system, and a direct read into an unaligned buffer on a real file (N1), I have from the simulated disk and from reading.
- The uploader's check itself: it is M2.4b-2. Where I say "the uploader then finds a mismatch" I rely on the description's rules.
- arm64, and the plain build under plants: the plants ran with the test hooks on amd64.

### Files

In `review/m2.4b-1/` on `skyzfs-review`: `plants.py` and `run_plants.py` with a result per plant in `plants/`; `scenarios_state.rs` (S1 to S3), `scenarios_store.rs` (T1, T3 to T7), `scenarios_real.rs` (R1) and `scenarios.py`, with `scenarios.out` (the head) and `gaps_<plant>.out` (the scenarios against each plant that passes the head's tests).

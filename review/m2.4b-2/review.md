## Review of #114 at 6c97063 (M2.4b-2)

**Verdict: can merge. Nothing blocks.** I found no defect in the code. Option (b) is sound, F2 is right as built, the count of commits must stay as it is, and the matrix has the 70 and the 21 kill points it says. What I have besides is two comments that say something the code does not do (D1, D2), seven gaps in the tests, where a plant of mine passes every test of the head (G1 to G7; with G1 the plant commits a cut that holds none of its blocks), and three notes for the lead (N1 to N3). None of them needs to hold the merge; G1 and G2 are the two I would close first, here or in M2.4c.

I read the diff against ba05d28, design §5.2 and the code of `intent_synced` before the description. Then:

- **The head's own tests** pass here, rustc 1.100.0: `skyzfs-local` 269 and 11 plain, 269 and 15 with the test hooks; `skyzfs-upload` 10, 3 and 19 both ways (`t_plain.out`, `t_hooks.out`).
- **50 plants** of my own, one at a time, in 52 runs, two of them being run a second time with a scenario (`plants.py`, `plants.out`): 37 runs killed, 15 pass. Of the 15, three are plants I expected to pass (L1, and U18 and U19, each of which the other covers), two are the second runs (U5s, L3s), and the other ten are the gaps below.
- **Nine scenarios** the head's tests do not have (`scenarios.rs` with eight, run by `scenarios.py`; `scenarios_store.rs` with one, in the store's own tests, run by `scenarios_store.py`). All nine pass on the head (`scenarios.out`, `scenarios_store.out`). Four of them (S6, S7, S8, T1) fail with a plant in place that passes every test of the head, which is what the gaps rest on.

### The four questions

**(1) Option (b) is sound. A named cut gives a superset in every interleaving I could build, and I could not construct a failing sequence.**

The argument in my words. The map has two readers, and each needs one thing.

- **The start that begins a new cut state** (a restart of the machine, an acknowledged failure) makes its first cut from the regions the map names, to go up onto the cut the map names as its base. It needs: every region with a block changed since the base has its bit.
- **The check** needs: the cut it sends as a delta holds every block changed since the cut the row names, and the row names the map's base.

Naming cut *N* changes the base to *N* and no bit. Before it, the bits covered everything changed since the old base *B*. *B* is older than *N*: the guard at `inplace.rs:3207` refuses a base of this state with a number at or above *N*, and a base of another state was committed before this state began. Everything changed since *N* changed since *B*, so the bits cover it. For the held cuts: an abandoned cut hands its map on (to the cut that waits, the cut whose flush is under way, or the open map) and its heir's `lo` is still the cut released before *N*, so the heir holds everything changed since then, which includes everything changed since *N*.

Each interleaving the question names, with what happens:

| Interleaving | What happens | Shown by |
|---|---|---|
| Writes after the cut's instant, before or after the naming | Their bits are set and durable before they are written; the naming clears none | the head's `a_committed_cut_the_bound_took_is_named_as_committed`; S5 |
| The bound takes the cut after its commit | Its map goes on; the naming moves the base; the heir is a delta onto the row | the head's two driver tests; S5 (1334 namings) |
| The cut taken was whole | The heir took a whole cut's map and is whole. Right, and no delta: see D2 | S3 |
| A release of a later cut | It clears against the later cut and moves the base forward. The naming never moves it back (L5 killed) | L5; S5 |
| A release of an earlier cut after the naming | Cannot be: the cut named was the oldest held | by reading |
| A death before the naming, same boot | The map names the cut before; the row is newer: `NotTheBase`, one whole upload | the head's death test |
| A death after it, same boot | The header is in the page cache; the heir goes up as a delta | the head's death test |
| A restart of the machine, the header on the disk | The new state's first cut is the map's regions, a delta onto the row | S4 b (S4 a is the same after a release); S5 |
| A restart of the machine, the header lost | The base is the cut before: `NotTheBase`, then whole | S4 c; S5 |
| The header kept and any of the map's other pages lost or kept | The naming wrote no other page. Older pages name more regions, never fewer | S5 (a power cut that decides sector by sector) |
| The map's write fails at the naming | The volume fails. The base in memory has moved all the same: see D1 | S2 |
| What the next check reads | The row is the base, head's group is the row's: `Delta`, and the cut holds everything since | S5: every upload compared with the volume at its cut's instant |

**S5 is my attempt at a failing sequence.** 1200 programs from seeds, half with the fixture's groups (all four labels, so every region of this small volume is named at every group) and half with a group written to one label only, so that the map narrows. Each program mixes writes, groups, uploads, a commit whose reply is lost, a cut taken by the bound after its commit (so a naming), a death, and a restart of the machine after a power cut that keeps or loses each sector on its own. After every upload, head is read back and compared with the volume as it was at that cut's instant. 4497 uploads, 1334 namings (506 of whole cuts), 477 restarts of the machine that took the map up (133 with fewer regions named than the volume has), 392 first cuts after such a restart sent as a delta: none wrong. With plant L3 in place (a naming that clears every bit) S5 fails at seed 40 with a block that differs, so it can see a wrong naming.

`intent.commits += 1` in the naming (`inplace.rs:3211`) is not needed, since the naming clears no bit, and does no harm: see question 3.

**(2) F2 as built is right. I found no loop, no upload skipped and no cut uploaded that must not be.**

- **`Kept` means one thing.** `release_cut` returns true with the cut still held only where `intent_committed` could not write the map's pages (`inplace.rs:3145`), which fails the volume. `Local::release` (`local.rs:129`) reads that as `Kept`, and the driver goes to `Stopped`. Stopping for good is right: only the cut being uploaded can be let go, the store would offer the same cut again, and the check would find it committed again. Plants: the driver that goes on after `Kept` (U17), a kept cut reported as let go (U31), and `release_cut` answering false there (L8, the old behaviour) are each killed.
- **Other failures still upload.** Scenario S8 fails the volume at a write of the image with cut 2 held: the driver uploads cut 2 as a delta, lets it go, and is at `Checking` again, with head reading as the volume at the cut's instant. That is design §5.2. No test of the head holds it: see G4.
- **`Gone` is handled right.** The driver names the cut on both paths (its own commit, and a check that found the row naming it; U13 and U13b killed). The cut is gone, so the next check gets its heir and nothing repeats. Where the naming answers false nothing loops either: the cut is not held, so it is not offered again.
- **Loops.** A mismatch abandons every held cut, so the next check has no cut until the pool makes one. A commit given up drops the engine and checks anew; it repeats only while something else keeps moving head. A commit that fails without a conflict is made again by the caller's next step, which is the caller's back-off (reading 7 of the description). `Stopped` answers `Idle`.
- **An upload skipped.** `Committed` needs the row to name the cut's state, number and group (U2 killed), and a state's name and a cut's number are not used twice.
- **A cut uploaded that must not be.** Not by the judge (U1, U3, U3b, U3c, U3d killed). Two rules of this kind rest on one line each that no test of the head holds: G1 and G2.
- F1's cost is as the description says, and is M2.5's: N2.

**(3) I agree: the count must not change, and a second clear must count. The alternative fails a healthy volume.**

`intent.commits` is not a figure for the status. It is the second half of the guard at `inplace.rs:2671`: a request that waits for the map, whose bit is not there when a sync that began after it ends, has lost the bit to the disk unless a commit's `clear` took it in between, and the count says whether one did. A release made again for the same cut calls `clear` again, and that call can take a waiting write's bit exactly as the first can.

- **The case is reachable on a healthy volume.** The daemon dies in a release after the map's pages and before the cut state's first store (the matrix's store 0). The next instance takes the state up with the cut held and the map naming it already. A write into a clean region waits for the map's sync. The check finds the row naming the cut and releases it again: the base does not move, and the `clear` takes the waiting write's bit.
- **Scenario T1 is that case** (`scenarios_store.rs`). On the head the write goes on once the map holds its bit, and the volume is healthy.
- **Plant L2 is the alternative** (count only where the base moves). Every test of the head passes with it, and T1 fails: the write is answered `Failed` and the volume has failed, with nothing wrong on the disk. L2b (a release does not count at all) is killed by the head's `a_write_whose_bit_a_clear_takes_while_it_waits_sets_it_again`. So the head's tests hold the count at a first release and not at one made again: G6.
- **The count at the naming** (L1 removes it; everything passes, T1 too). It is not needed, because a naming clears nothing. Its only effect: a bit that really was lost while a naming happened is set and synced once more instead of failing the volume at once. That is harmless. The field's comment (`inplace.rs:1002`, "how many commits have cleared bits") is no longer exact, since a naming counts and clears none.

**(4) The matrix covers what the M2.4 row asks. I count 70 and 21. The half on object storage fails if a point is skipped; the half on the cut state does not.**

- **Object storage: 35 operations, 70 kill points.** S1 records the run and counts: 7 PUTs of a segment, 10 of a tree node, 2 of a commit, 2 conditional PUTs of the refs object (the head swaps), 7 reads of the refs object, and 7 other reads. A kill at each, before it and after it: 35 and 35, and the run ends on its own at the 36th.
- **The kill points are real.** The fault store fails the operation and every one after it, so nothing more reaches the store, and the driver is dropped. Before: the inner store never sees the operation. After: it completes and the caller gets an error. The refs object is the same store, so the swaps and the reads of the refs are kill points too, and the test asserts that exactly the two swaps land unheard in the after mode and none in the before mode.
- **The cut state: 21 stores, 21 kill points.** S1 counts the stores of the run with no death through `MemRegion::stores()` (21) and the deaths the loop finds (21). A store is one word, and a death at it leaves it unmade, so before and after do not differ. The restart finds the cut held for the row to name at three of them (stores 0, 13 and 14), as the description says.
- **A point skipped.** Object storage: the test asserts `points == operations.len()` for each mode, and my plants in the test are killed (M1 starts at the second operation, M1b stops one early). The cut state: the test asserts `points >= 2` and one cut found held, so a loop that starts at the second store (M2) or stops after three (M3) passes: G3.
- **The order the matrix is there for.** Plant M4 lets the cut go before the map names it. It is killed by the cut-state matrix, the only one of the three crash tests to fail (I ran it against those; the store's own death test from #112 holds the order as well). So the matrix does see the order of the release.
- **What the matrix does not have, none of it asked for by the row:** deaths while the write path makes copies during an upload (the store's own death tests have those) and deaths inside a naming, which makes no store into the cut state (the death test has its two sides).

### The judge, orphans and the upload's lifetime

- **The judge.** I found no way a wrong upload passes it. A whole cut needs head's group to be the row's, or no row and no group; a delta needs the row to be the map's base and head's group to be the row's; a row that cannot be read is never overwritten; and the row's swap expects the row as the check read it, in the head swap's transaction. A commit that lands late, after its error, either makes the row name the cut (found at the next read of the row) or meets the head swap's condition. One remark on "head's group is the row's" for the lead: N1.
- **Orphans.** A cut lost before its commit and a commit given up leave the segments PUT early with no commit that names them, as the description says (reading 4). `lost` calls the engine's discard (U10, which leaves it out, is killed), and the discard also forgets the list of segments PUT early, so the heir's commit does not list them. Nothing in this pull request calls the engine's `gc`: N3.
- **The check runs on an engine with no writes** (reading 6). True by reading: every way back to the check is a commit that landed, a discard, or a reset.

### Defects

None found.

### Comments that say what the code does not do (not blocking)

**D1. A naming whose write of the map fails has changed something.** `inplace.rs:3162` says "False, with nothing changed, ... where the map's pages cannot be written", and `driver.rs:105` says that where the map does not name the cut "the next check finds a row the map does not name: a mismatch, and one whole upload".

- **What happens (S2).** `map.commit(named)` runs before the write, so the base in memory is cut 2 when the call answers false. The same instance then uploads cut 3 as a **delta** onto the row, with no mismatch. Its release cannot write the map either and the driver stops at `Kept`; or, where the file takes writes again, cut 3 is let go and the base is cut 3.
- **It is safe.** Head read back is the volume at cut 3's instant in S2. The base in memory is true (cut 2 is committed), the held cuts hold everything since, and the disk has the old header or the new one, both of which the table of question 1 covers. The volume has failed in any case.
- **Certainty:** certain, by scenario. A fix is a word in each comment.

**D2. A named whole cut gives a whole heir, not a delta.** `inplace.rs:3180` ("What it gives the uploader") and `local.rs:94` promise that the next cut goes up as a delta. Where the cut the bound took held the whole volume (the first of a state, the first after a mismatch), its heir took its map and is whole: S3 uploads 256 blocks twice. That is right, since `lo` has not moved. It is also the case most likely to meet the bound, a whole upload being the long one. Certain, by scenario. A sentence in the comment; whether the heir of a named whole cut could be narrowed is not for this pull request.

### Gaps in the tests (not blocking), most severe first

**G1. An engine that holds a cut's writes is opened again at a moved head, and the cut is committed empty.** `remote.rs:161` to `167`: `observe` leaves an engine with writes alone where head has moved, and says `current: false`.

- **Plant U26** removes the arm. Every test of the head passes.
- **Scenario S6:** cut 2's commit fails before anything lands (the PUT of the commit object), and another writer then moves head and leaves the row. On the head: `GaveUp`, then `Mismatch { HeadMoved }`, and head untouched.
- **With U26, S6 ends with** `Uploaded { number: 2, rule: Delta, blocks: 5 }`, the row naming cut 2, head's group 8, and head's content exactly what the other writer left: the engine was opened again clean, and the commit made again carried the group and the row and none of the cut's blocks. The next delta goes onto it. That is a cut silently lacking its blocks.
- Of my plants this is the one that passes every test and ends in a wrong cut in object storage. S6 is offered as the test.
- **Beside it,** three conditions at `driver.rs:491` and `492` pass too: no reset where a commit is given up (U5), the commit made again though head moved (U7) or though the row moved (U7b). With U7 and U7b the transaction's own conditions refuse the commit, so the cost is one commit that conflicts (S6 with U7: `Err(Conflict)`, then `GaveUp`). With U5 the engine keeps the cut's writes and its old head, the next check reads the old head's group and matches, and the run after ends in `Conflict` (by reading, so does every later one): the mismatch an operator should see is never said. S6 fails with each of U5, U7 and U26.

**G2. A cut whose flush is under way is kept from the upload by one line.** `local.rs:106` to `108`: `uploadable` gives the oldest held cut only once its phase is uploading. Design §5.2: a cut may leave the machine only once its flush has completed.

- **Plant U9** removes the check. Every test of the head passes.
- **Scenario S7** holds the image's sync with cut 1 flushing: on the head the step is `Idle` and nothing is checked; once the flush has ended the upload begins. With U9 the step is `Began { number: 1, rule: Whole }` while the flush is still held. The store reads any held cut, so nothing else stops it.
- The description leaves the failure paths to M2.4c. The rule itself is this crate's, and S7 is offered for it.

**G3. The cut-state matrix does not fix its count.** `tests/crash.rs:221` to `225` assert `points >= 2` and `landed_unheard >= 1`. M2 (the loop starts at the second store) and M3 (it stops after three) pass. The other half of the matrix counts the run's operations first and asserts equality; this half can do the same with `fx.region.stores()` before and after a run with no death (21), and can assert the three stores at which the cut is found held.

**G4. "Other failures still upload" has no test.** Plant U32 reports a cut let go on a failed volume as kept, so that the driver stops at any failure. Every test of the head passes; S8 fails. This is the half of F2 that design §5.2 asks for.

**G5. The matrix's "PUT before the commit" also counts the commit's own PUTs.** `tests/crash.rs:172` to `180` count the segment PUTs before the first PUT of a commit object, and the engine's commit PUTs its segments before that object. With both calls of `put_sealed` removed from the driver (U18+19) the matrix passes; only `a_failed_put_goes_on_from_the_next_slice` fails. Counting the segment PUTs when the driver reaches `Committing` would say what the message says. (U18 and U19 alone pass everything, each call covering the other; what the first call's comment gives, no pile-up while the store is out of reach, is not tested.)

**G6. A release made again must count: T1.** See question 3. Plant L2 passes the head's tests and fails a healthy volume in T1.

**G7. A conflict with the row and head read as before** (reading 3 of the description, `driver.rs:491`): plant U6 makes the commit again there, and everything passes. I have no scenario: I could not make a conflict with the row and head unmoved on this store, and I think the condition cannot be met unless head moves and moves back. Nothing to do unless a test is cheap.

### Notes for the lead

**N1. "Head's commit is the row's" compares the group alone** (`check.rs:89`), which is Q5 as decided. Design §5.2 says a group does not identify a pool state: after a rewind import a number already used is written again. Two commits of this uploader can then carry one group, and a head rolled back from the second to the first would pass the check with the row naming the second. It needs an operator's rollback between two such commits, so it is far away. If it should be closed, the row or the check would have to know head's commit and not only its group.

**N2. F1's cost stays for M2.5**, as the description says: an instance that kept a cut uploads no later cut. With D1 there is one more path into `Kept` (a failed naming, then the heir's release).

**N3. For M4's garbage collection:** an upload PUTs segments before its commit, slice by slice, so a collection by another handle must spare segments that no commit names yet while an upload is under way. Nothing calls `gc` in this pull request; the same note as on #110.

### The plants

`plants.py`, results in `plants.out`; for a plant that passes, the scenario run with it is in `plants_detail/`. Driver, check, row and matrix plants run against `cargo test -p skyzfs-upload`; store plants against `skyzfs-local` and `skyzfs-upload` with `--features skyzfs-local/test-hooks`.

| Plant | Result |
|---|---|
| U1 a whole cut uploaded whatever head's group | KILLED by 4 |
| U2 the row names the cut by state and number alone | KILLED by 1 |
| U3 a delta whatever cut the row names | KILLED by 3 |
| U3b Q8 relaxed | KILLED by 1, the death test |
| U3c a delta whatever head's group | KILLED by 3 |
| U3d a row that cannot be read taken as no row | KILLED by 2 |
| R1, R2 a row of another version, or too long, is read | KILLED by 1 each |
| U4 no reset where the row names the cut after a failed commit | KILLED by 1 |
| U5 no reset where a commit is given up | **passes**; S6 fails with it (G1) |
| U6 a commit that conflicted made again | **passes** (G7) |
| U7 a failed commit made again though head moved | **passes**; S6 fails with it (G1) |
| U7b the same though the row moved | **passes** (G1) |
| U8 only the last slice says whether a copy was read | KILLED by 1 |
| U10 a lost cut keeps its writes in the engine | KILLED by 1 |
| U12 a cut that went since its check is committed | KILLED by 2 |
| U13, U13b a cut committed and gone is not named | KILLED by 1, by 2 |
| U14 a run written as its first block is, zeros or data | KILLED by 14 |
| U15 the commit expects no row | KILLED by 9 |
| U17 the driver goes on after a kept cut | KILLED by 1 |
| U18, U19 one of the two calls of `put_sealed` removed | **pass**, as expected: each covers the other |
| U18+19 both removed | KILLED by 1, not by the matrix (G5) |
| U20 no sync of the copies before the commit | KILLED by 2 |
| U21 a failed sync of the copies commits all the same | KILLED by 1 |
| U24 a failed commit made again without reading the row and head | KILLED by 4 |
| U9 a cut offered whatever its phase | **passes**; S7 fails with it (G2) |
| U31 a kept cut reported as let go | KILLED by 1 |
| U32 a cut let go on a failed volume reported as kept | **passes**; S8 fails with it (G4) |
| U26 `observe` opens an engine with writes again at a moved head | **passes**; S6 fails with it (G1) |
| U27 the commit carries no group | KILLED by 18 |
| U28 the commit does not swap the row | KILLED by 19 |
| U30 `observe` keeps a clean engine though head moved | KILLED by 2 |
| M1, M1b the matrix on object storage skips a point | KILLED by 1 each |
| M2, M3 the matrix on the cut state skips points | **pass** (G3) |
| M4 release lets the cut go before the map names it | KILLED by the cut-state matrix |
| L1 a naming does not count as a commit | **passes**, as expected (question 3) |
| L2 a release counts only where the base moves | **passes**; T1 fails with it (G6) |
| L2b a release does not count | KILLED by 1; T1 fails with it too |
| L3 a naming clears every bit | KILLED by 1, the store's test; S5 fails with it |
| L4 a held cut can be named | KILLED by 1 |
| L5 a naming moves the base back | KILLED by 1 |
| L6, L6b the bounds on the cut's number | KILLED by 3, by 1 |
| L7 a naming whose write fails does not fail the volume | KILLED by 1 |
| L8 a release whose write fails answers false | KILLED by 4 |
| L9 a naming does not write the map's pages | KILLED by 3 |

U5s and L3s in `plants.out` are U5 and L3 run again with a scenario, the second against the upload crate's crash tests only.

### The scenarios

| | What it shows | On the head |
|---|---|---|
| S1 | The matrix counted: 35 operations, 70 and 21 kill points, the three stores | as the description says |
| S2 | A naming whose write of the map fails | D1; head read back is right |
| S3 | A whole cut taken by the bound after its commit | D2; right |
| S4 | A restart of the machine under the uploader: (a) after a release, (b) after a naming, (c) after a naming whose header the power cut loses | delta, delta, mismatch then whole; right each time |
| S5 | 1200 random programs with namings, deaths and power cuts; every upload compared | none wrong |
| S6 | A commit that fails, then head moved by another writer | given up, then a mismatch |
| S7 | A cut whose flush is under way | not offered |
| S8 | A volume failed at a write of the image | its cut is uploaded and let go |
| T1 | A release made again after a death, under a write that waits for the map | the write goes on, the volume is healthy |

### CI and Codex

- **CI on 6c97063 is green**: 37 jobs succeeded and 2 were skipped (the badge job, and the replay rig's job on the reference server, which a pull request that touches only the daemon's code does not run). That includes lint, test, unit-arm64, coverage, the control rig, the daemon rig on three images and the replay rig through skyzfsd on two. I did not read the rigs' tables; the lead does before the merge.
- **Codex** names 6c97063 as reviewed, with no findings and no comments.

### What I could not verify

- Nothing here ran on a kernel; the rigs in CI did.
- A real object store that lets a PUT land after its error: by reading only. The row's and head's conditions in the one transaction are what a late commit meets.
- G7's case, for want of a way to make it.
- clippy and fmt: not run again here; CI's lint is green on the head.
- The daemon's side (the thread, the round trips, the status) is M2.4c and not in this pull request.

### Files

`review/m2.4b-2/` on `skyzfs-review`: `review.md`, `plants.py`, `plants.out`, `plants_detail/` (the scenario's output under each plant that passes), `scenarios.rs`, `scenarios.py`, `scenarios.out`, `scenarios_store.rs`, `scenarios_store.py`, `scenarios_store.out`, `t_plain.out`, `t_hooks.out`.

## Review of #134 at 4595f55 (F1: the cut's row names its commit)

**Verdict: not yet. One finding blocks the merge: B1.**

**B1 is a defect, and a regression against e155dbf.**
- `observe` checks the row against head as it read it first, but the engine it reopens sits on head as read a second time.
- If another writer swaps head between those two reads, a delta goes up onto that writer's history, and head is moved off that writer's commit.
- The base caught this unless the other commit had the row's group.
- The fix is one line. I ran it: my scenario S1 passes, and so do all of the crate's tests.

**B2 is closed by 4595f55.** At acda3e7 the cut rig's upload job was red, because node B read only version 1 rows. 4595f55 changes only `bins/skyzfs-nodeb`, and with it the upload job is green: all six upload courses PASS, worst exit 0. Node B's reader agrees with the daemon's writer byte for byte (see "Node B at 4595f55").

Everything else holds up:
- **Reading a version 1 row, as the lead decided.** I found no path where a version 1 row lets a delta or a publish rest on the wrong history. The table is under (1).
- **`commit_naming`:** its order, its failures, and what a crash leaves in each window.
- **The row's format:** the version bump, the cross-version decode test, and golden bytes for version 2.
- **The two places that compared groups** both compare commits now.

All 21 plants are killed: the author's twelve and my nine. Nothing else blocks. Non-blocking items: a stale crate doc (N1), what a downgrade does (N2), the status word at the upgrade (N3), a GET with no reader (N4), stale text in the description (N5), and some test gaps.

**How I worked.**
- I read the diff `e155dbf...acda3e7` first. Then I read the design's §5.2 and §5.3, and the notes' entries on the upload stack: the row, the publish rule (M2.5), the lease (F5), and F1 of #122's review with its n1 and n2. The description came last.
- The upload change is the same at acda3e7 and 4595f55: `git diff acda3e7 4595f55` touches only `bins/skyzfs-nodeb/src/main.rs`.
- Everything below was run at acda3e7 unless it says otherwise. Node B's section was run at 4595f55.

**What I ran:**
- **Scenarios S1 to S5** (`review_f1.rs`). This is a test file I put beside the crate's tests in a worktree, and took out after.
  - At the head, S1 fails and S2 to S5 pass (`scenarios_head.out`).
  - S1 at e155dbf is in `scenarios_base_s1.out`. It came from a copy of S1 adapted to the base's test helpers, and that copy was not kept. S1 with B1's fix is in `fix_b1.out`.
- **Plants.** I ran 21 plants, each planted alone in a clean worktree (`plants_f1.py`, `plants.out`, `plants.json`; `baseline.out` is the unplanted run).
  - Each ran against `cargo test --no-fail-fast -p skyzfs-upload`, plus `-p skynbd-engine` for the engine's plants.
  - Each had a 2400 s limit, and the file was restored in a `finally`.
  - The worktree was clean afterwards.
- **A cross-check of node B's row codec against `skyzfs-upload::row`** (`nodeb_xcheck.rs`, `nodeb_xcheck.out`).
- **The gates** (under "Gates").
- **CI on both heads**, the logs of the cut rig's upload jobs, and Codex's comments.

### Blocking

**B1. The check judges head as first read, but a reopened engine commits on head as read again** (`crates/skyzfs-upload/src/remote.rs:188` to `225`, with `driver.rs:554`).

**How `observe` works today:**
- It reads the row (`:189`), then head (`:190` to `:194`).
- If the engine is missing, or is clean and not at that head, it drops the engine and calls `Engine::open` (`:209`).
- `Engine::open` reads head a second time (`crates/skynbd-engine/src/lib.rs:727` to `730`) and opens there.
- `observe` then returns `head: head.commit` (`:224`), the first read.
- `current` (`:223`) says whether the two reads agree, but the check never looks at `current`.

**What goes wrong:**
- `judge` compares the row's commit with `seen.head` (`check.rs:108`, `:122`).
- The driver then writes the cut into the engine. The engine's swap expects the engine's own head, which came from the second read.
- Suppose another writer swaps head from the row's commit `C1` to `Z` between the two reads:
  - the check finds `C1 == C1` and passes a delta;
  - the delta is written onto `Z`;
  - the swap lands. Head is `Z`, as the engine expects. The row's swap expects the row, which the other writer did not touch.
- Head then holds neither the volume at the cut nor `Z`. The uploader has moved head off another writer's commit, which Q5 forbids.
- A whole cut takes the same path. Its image is right, because it holds every block, but it too moves head off `Z`. I found this by reading the code; S1 runs only deltas.

**Scenario S1** (`review_f1_s1_…` in `review_f1.rs`):
- Cut 1 goes up whole. The uploader restarts. Cut 2 is a delta onto cut 1.
- A wrapper around the refs store swaps head to `Z` right after `observe`'s read of head returns. That is between the two reads.
- `Z` is another history: block 0 is filled with `0xCD`. I ran it with `Z`'s group set three ways:

| `Z`'s group | at acda3e7 (and 4595f55) | at e155dbf | with the fix below |
|---|---|---|---|
| none (an operator's write) | `Began Delta, Uploaded Delta`; head does not hold cut 2, and head is no longer `Z` | `Mismatch HeadMoved`; head stays `Z` | `Mismatch HeadMoved`; head stays `Z` |
| 7, the row's (n2) | the same as above | the same as at the head (n2 itself) | `Mismatch HeadMoved`; head stays `Z` |
| 99, another | the same as above | `Mismatch HeadMoved`; head stays `Z` | `Mismatch HeadMoved`; head stays `Z` |

**Why it is a regression:**
- At the base, `judge` read `head_group`, and `observe` took `head_group` from the reopened engine (`engine.head_group()`, `:222`). So the check and the swap looked at one head, and only a `Z` with the row's own group got through. That case was n2.
- At the head, the check looks at the first read and the swap at the second. Every `Z` gets through.

**When it can happen:**
- The engine is reopened at the first check after a start or a `reset`, and at every check where head has moved since the last commit.
- The other writer's swap must fall in the gap between two GETs of the same key, which is milliseconds wide.
- The lease fences commits, not reads. Q5's other writer, for example node B's `move-head` in the rig's `umoved`, is not fenced at all.
- So it is rare, but it is exactly the outcome F1 is meant to rule out.

**Fix:**
- In the last `Observed` (`remote.rs:224`), set `head: engine.head_commit_hash()`, the commit the engine will build on. Keep `current` as it is computed now.
- Then the check judges the head the swap will expect:
  - `Z` is not the row's commit, so the verdict is `HeadMoved`;
  - `head_of` hands on nothing, because the row names `C1`, not `Z`;
  - the commit's retry still gives up on `!current`.
- I ran this fix in a worktree (`fix_b1.out`, with the diff at its top). `cargo test -p skyzfs-upload` passes: unit 53, crash 5, driver 23, holes 5, publish 19, queued 9, uploader 58. S1 to S5 pass too, and S1 now gives `HeadMoved` for all three `Z`s (the last column above). That is stricter than the base, which let a `Z` of the row's group through.
- Another fix that works: have the check observe again whenever `current` is false.
- S1 should come in as a test with the fix. It needs a refs wrapper like `Racing` in `review_f1.rs`.

**Certainty: high.** S1 is deterministic, and the code path is two lines. **Likelihood: low**, because the window is narrow.

### B2, closed at 4595f55: the readers of the row

**At acda3e7** the job "record through skyzfsd and check, ubuntu-26.04-upload" failed: run 45485411707, job 138769835317, worst exit 2. The lines of each course are in `upload_job_acda3e7.txt`.
- life, ukill and ulease ended with `upload_content: FAIL: node B found no cut of the daemon's at head (0 of another writer's)`.
- umoved, urow and ulost ended in a rig error: `no cut in the row before …: … row unreadable 0200…`.
- The rows in that log were correct version 2 rows. For example umoved's, `0200 a3ff…a50f 0900…00 0e00…00 d5e362f9…`, is version 2, then the cut state, number 9, group 14, and the commit `d5e362f9…`, which is the head that the same line prints.
- Node B printed any row that was not 34 bytes of version 1 as `unreadable`. The notes had placed that reader in this package (the M2.5 entry: "a new row version read by `tests/kernel/` and `skyzfs-nodeb`").

**At 4595f55** the same job passes: run 45490762086, job 138788217687, worst exit 0 (`upload_job_4595f55.txt`).
- `upload_content` is PASS in all six courses:
  - life: 1 of 27 cuts fetched at head;
  - umoved: 1 of 8, and 1 of another writer's;
  - urow: 4 of 22, and 2 of another writer's;
  - ukill: 1 of 135;
  - ulease: 2 of 18;
  - ulost: 1 of 10.
- `upload_course` is PASS in umoved, urow, ukill, ulease and ulost. For urow it reads `row1` and `row2` both `not_the_base`.
- `upload_status` and `upload_balance` are PASS throughout.
- The rig's own parsers (`rowcut` in `cut-record.sh`, `read_row` in `cut_daemon_check.py`, and the watcher's `parse_row`) did not need to change. Node B prints the same four fields for both versions, and the new `row_commit` line is ignored by all three, since each matches on `row` exactly.

### Node B at 4595f55 (the reader of row versions 1 and 2)

**The format agrees with the daemon's.**
- Neither has a magic. Both put the version first, as a little-endian u16. Then come the cut state (16 bytes, at 2..18), the number and the group (little-endian u64 each, at 18..26 and 26..34), and in version 2 the commit (at 34..66).
- These are the same offsets as `row::encode` and `row::decode` (`crates/skyzfs-upload/src/row.rs:106` to `152`).
- Node B names the fields `state`, `number`, `group` and `commit` (`RowCut`). The daemon names them `Named { cut: CommittedCut { state, number, group }, commit }`.

**It reads exactly what the daemon reads.** `decode_row` takes 34 bytes that start `01 00` as version 1, and 66 bytes that start `02 00` as version 2. Anything else is `row unreadable HEX` followed by `row_commit none`. That is the daemon's acceptance set: version first, then that version's exact length.

**It refuses unknown versions.** Node B's own test covers version 3, version 2 without a commit, version 1 with one, and both versions cut short by a byte.

**I checked this against the daemon's code rather than by reading** (`nodeb_xcheck.rs`: node B's `hex`, `RowCut`, `decode_row`, `encode_row` and `row_line`, copied unchanged from lines 201 to 203 and 331 to 398 of `main.rs` at 4595f55, in a test beside `skyzfs-upload`'s, taken out after):
- 10 000 rows from the daemon's `row::encode` are read by node B as the same cut and commit. Node B's `encode_row` writes the same bytes back, and `row_line` prints the expected lines.
- 10 000 rows from node B's `encode_row`, half of each version, are read by the daemon's `row::decode` as the same cut and commit.
- 200 000 byte strings (lengths 0 to 79, biased towards versions 0 to 3 and lengths 34 and 66): node B reads one exactly where the daemon does, with the same fields. 34 133 were read and 165 867 refused, with no disagreement.

**Gates at 4595f55, here:**
- `cargo fmt --all --check` is clean.
- `cargo clippy -p skyzfs-nodeb --all-targets -- -D warnings` is clean.
- `cargo test -p skyzfs-nodeb`: 14 passed (`nodeb_gates.out`).

**Two notes, neither blocks:**
- **The codec is written twice, and nothing in CI holds the two copies together.** Node B is kept independent of the uploader on purpose. A dev-dependency on `skyzfs-upload` would keep it independent at run time, and a test like the first two above would then pin the agreement.
- **`set-row --row STATE:NUMBER:GROUP` reads head before the guard's read of the row, and outside the write's transaction.**
  - The daemon moves head and row together, so a commit by the daemon in between changes the row's cut, and the guard catches it with exit 6.
  - A writer that moves only head in between would leave the row naming a commit head no longer holds.
  - That only matters to a rig that runs two other writers at once. None does.

Another review of the same commit, posted on the pull request, reaches the same reading of the format and the guards.

### (1) The reading of a version 1 row

The code does what the lead decided:
- `judge` (`check.rs:94` to `124`) lets a whole cut under a version 1 row go up by group (`:109`).
- A delta under a version 1 row always gets `HeadMoved`, because `named.commit` is `None` and never equals `Some(head)` (`:122`).
- `head_of` (`driver.rs:809`) hands on nothing for a version 1 row.

The upgrade's whole cut writes version 2 (`remote.rs:253`). A version 1 row that names the very cut being uploaded can only come from a restart across the upgrade: a cut that a daemon from before F1 committed, and that the store still holds. The commit's retry after a lost reply always reads a version 2 row, because this daemon wrote it.

| Row | Head against the row | Lost reply / restart | Cut | What the code does | Right? | Shown by |
|---|---|---|---|---|---|---|
| v1, names the base | head's group is the row's (any history) | either | delta | `HeadMoved`; the held cuts are abandoned and the next cut is whole | yes: no delta rests on it | `after_a_row_of_version_1_…`; plants V1D, R9 killed |
| v1, names another cut | head's group is the row's | either | whole | `Upload Whole` onto head; the commit writes v2 | yes: the image is the volume | the same test; plant V1W killed |
| v1 | head is another history of the row's group (the description's residual risk) | — | whole, then a delta | the whole cut goes up onto it with the right image, and the next delta goes onto the v2 row the whole cut wrote | yes | S4: cut 2 `HeadMoved`; cut 3 whole and the volume; cut 4 a delta and the volume |
| v1 | head's group is not the row's | either | whole | `HeadMoved` | yes | check's unit tests; plant JW killed |
| v1, names this cut | any | restart across the upgrade | — | `Committed`, let go; `head_of` gives nothing, so `not_offered` and no publish | yes | `a_cut_found_committed_under_a_row_of_version_1_takes_no_publish`; plants H1, H2, HC killed |
| v1, then the upgrade's whole cut lands | head is its commit | lost reply | whole | the retry reads v2 naming the cut and head: uploaded, and the publish is carried on its commit | yes | S2: `snap/t` on cut 3's commit, `not_offered` 0 |
| v2, names the base | head is the row's commit | — | delta | `Upload Delta` | yes | the suite |
| v2, names the base | another commit of the same group (`import -T`, or a rollback to a snapshot) | lost reply or restart | delta | `HeadMoved`; head and row untouched | yes | n2: `head_at_another_commit_of_the_cuts_group_…`, 4 cases; plant JD killed |
| v2, names this cut | head moved on to another commit of its group | lost reply or restart | — | `Committed`, no publish, `not_offered` | yes | n1, the same test; plants H1, HC, HF, R1 killed |
| v2, names this cut | head moved away and rolled back onto the row's own commit | lost reply | — | `Committed`; the publish goes onto the cut's commit | yes | S5 |
| v2, names the base | head moved away and rolled back onto the row's commit | — | delta | one `Conflict` and `GaveUp` (P1), then `Upload Delta` onto it; the image is right | yes | S3 |
| v2, names the base | the row's commit at `observe`'s first read, another commit at `Engine::open`'s | — | delta or whole | `Upload` onto the other commit | **no** | **B1**, S1 |
| none | head's commit has no group | — | whole | `Upload Whole` | yes | the suite; plant R5 killed |
| none | head's commit has a group | — | whole | `HeadMoved` | yes | the same |

B1's row is the only wrong one, and B1 cannot happen with a version 1 row:
- A version 1 row is judged on `head_group`. In the reopen path `head_group` comes from the reopened engine, so the check and the swap agree.
- The race needs a row that names a commit.

### (2) `commit_naming`

**The order in `commit_inner`** (`crates/skynbd-engine/src/lib.rs:1667` to `1840`):
1. The segments are PUT, then the nodes, then the commit object. The commit's hash covers its parent, its group and `created_at_ms`.
2. `ops(commit_hash)` runs once (`:1803`). It is `FnOnce`, and each call of `Remote::commit` builds a new closure.
3. `own_ref_untouched` checks the ops (`:1804`).
4. One `transact` holds head's `CasRef`, which expects the engine's own head, and the caller's ops.

Plant EN builds the ops from head's hash instead of the new commit's. It is killed by 77 tests, so the row names the very commit the swap installs.

**Failures:**
- **Ops that swap the engine's own ref** are refused with `InvalidIo` after the PUTs.
  - Nothing is swapped, the engine keeps its writes and its group, and the objects already PUT are orphans. The doc says so (`:1638` to `1641`).
  - `commit_with` still refuses such ops up front (`:1631`), so a fixed set of ops PUTs nothing. Plant R6, which removes that up-front check, is killed by `ops_that_swap_the_engines_own_ref_are_refused`.
  - Plant R7, which drops the engine's writes on that refusal, is killed by `ops_made_from_the_hash_that_swap_the_engines_own_ref_are_refused`.
  - The uploader's ops are a single `MetaCas`, so only a caller's bug can leave these orphans.
- **A conflict on head** (`op_index: 0`) is `ref_moved`. **A conflict on the row** is `OpConflict { op_index: 0 }`, which the uploader reads as `Conflict`. Both are as before.

**Crash windows:**
- **Before the commit object is PUT, or after it and before the swap:** head and the row are as they were. The cut is still held and goes up again after the next start, judged on the old row. The objects PUT are orphans, the same as for any commit that fails at its swap; the notes leave those to M4's garbage collection.
- **The swap landed but the reply was lost:** the row is version 2 and names the cut and the commit, and head is that commit.
  - The retry's `observe` finds the cut named, and `head_of` gives head. S2 shows this for the upgrade's whole cut, and the suite's crash matrix for the rest.
  - A restart finds the same at its check: `Committed`, with the publish carried.
- **A second attempt after a lost reply that did not land** makes a new commit with a new hash, because `created_at_ms` changes. Its row names that new commit, and nothing reuses the first attempt's hash.

**The format:**
- The version comes first. The length is then checked against that version: 34 bytes for v1, 66 for v2 (`row.rs:117` to `152`).
- Any other version is `RowError::Version`. The check reads it as `Unreadable` and never overwrites the row.
- CONTRIBUTING's rule is met:
  - the bump to `ROW_VERSION = 2`;
  - a decode of version 1 bytes (the cross-version test, `a_row_of_version_1_names_its_cut_and_no_commit`);
  - the golden bytes of version 2 (`a_row_of_version_2_is_laid_out_as_documented`).
- Plant R3 (the commit read from bytes 2..34) is killed by 77 tests, and R4 (a version 2 row decoded without its commit) by 76.
- Old readers: node B is covered above, and a daemon from before F1 under N2.

### (3) Does anything still match head to the row by group?

**In the crate, no**, apart from the two places the lead decided:
- `check.rs:109`: a whole cut under a version 1 row.
- `check.rs:106`: no row, where head must have no group.

I searched for `group` and `head_group` in `crates/skyzfs-upload`, `bins/skyzfsd` (the upload thread, status, the control channel, kill), `publish.rs` and the lease:
- `Observed::head_group` is read only by `judge` (`driver.rs:554`).
- `publish.rs`'s `carried_by` compares the request's G with the cut's group. That is the publish rule (number above C, group at or above G), not a match of head.
- The status's `cut_group` and the control channel's groups are reports. `kill.rs` prints the group.

**Outside the crate, nothing decides from the row's group:**
- The cut rig's `before_other` compares the status's `cut_group` with the row's group, but only to decide when to look again.
- `cut_daemon_check.py` compares the group of a cut it fetched with the cut lines in the request log.
- The benchmark (`tests/bench/`) does not read the row.

### Non-blocking defects and suggestions

**N1. The crate's doc still describes the check by group** (`crates/skyzfs-upload/src/lib.rs:14` to `16`). It says "head's commit is the row's (its group is the row's)". `check::judge`'s doc and the driver's module doc are right. Certainty: high.

**N2. A daemon from before F1 cannot read a version 2 row, and its uploads stop for good.**
- The base's `judge` reads `Version(2)` as `Unreadable`, a mismatch at every check, and never overwrites the row (`git show e155dbf:crates/skyzfs-upload/src/check.rs`, line 82).
- So rolling the binary back after even one commit by this one abandons every cut, until somebody puts a version 1 row back by hand. Node B's `set-row --row v1:…` can do that.
- That is the right reading of a format the old daemon does not know. But neither the description nor the notes say it, and an operator should know it before upgrading.
- Certainty: high, from reading the base.

**N3. The upgrade shows as `head_moved`, and costs every volume a whole upload.**
- The first delta after the upgrade is a `HeadMoved` mismatch, and the status says `head_moved` (`bins/skyzfsd/src/upload.rs:977`). Head did not move: the row simply names no commit.
- A reason of its own, such as "the row names no commit", or a line in the notes, would spare an operator the question.
- The whole upload is what the lead decided. The notes record (2026-11-18) that a whole cut can fail to complete under a write load. I did not check whether M2.4e's hole set helps a whole cut that follows a mismatch, as opposed to one that follows a begin.
- Certainty: high on the status word.

**N4. The `commit_group` GET in `observe`'s dirty branch** (`remote.rs:199`) **no longer has a reader.**
- That branch is reached only from the commit's retry: every path to the check that has writes in the engine discards them or resets first.
- The retry reads `row`, `current` and `head`, not `head_group`.
- Since F1 of #122's review the GET served only that retry. Now it costs one GET per failed commit and can go.
- Certainty: medium-high, from reading the driver's paths.

**N5. The description is stale in four places.**
- "Commits, `e155dbf..acda3e7`" leaves out 4595f55.
- "Not done: Testing's readers of the row read version 1 only … `bins/skyzfs-nodeb`'s `row` prints any row that is not 34 bytes of version 1 as `unreadable`" is no longer true at 4595f55.
- "CI: to be added when the runs on `acda3e7` end" is a placeholder.
- In the plants table, EN's count is 1 because it was run with `-p skynbd-engine` alone. With `-p skyzfs-upload` as well it is 77 here.
- The rest agrees with the code, including "every one was caught" for the twelve plants and their other counts.

**N6 (a question, not a defect). `head_of` could hand on the row's commit whatever head is.**
- A version 2 row is written in one transaction with the swap that installs the commit it names. So the commit the row names is the cut's commit even after head has moved on, and a publish on it would be right.
- `head_of` is stricter: where head has moved, the publish waits for the next cut.
- My plant R8 does the looser hand-on. Three tests kill it, and they pin the stricter rule on purpose.
- Whether publishes should be carried on `named.commit` is the lead's question.

### Notes on code this pull request does not change

**P1. `at_head` compares the hash alone** (`remote.rs:195`).
- When head was moved away and rolled back onto the same commit, the generation has changed. The engine is kept anyway, and its swap conflicts on the generation.
- The driver then gives up once and checks again, and the second try is right. S3 shows `Began Delta, GaveUp, Began Delta, Uploaded Delta`, with one `Conflict`.
- The result is right and costs one round trip. Comparing the whole `RefValue` would save that round trip.

### Gaps in the tests

- **G1.** Nothing puts a swap between `observe`'s two reads of head (B1). S1 is that test.
- **G2.** Two cases are not in the suite: the upgrade's whole cut through a lost reply, with its publish carried (S2), and a found-committed cut whose head was moved away and rolled back onto its commit (S5). Both pass at the head.
- **G3.** Nothing in CI holds node B's row codec to the daemon's (see "Node B at 4595f55"). The cut rig shows the agreement only on the rows it happens to write.

### Plants

| Plant | What is planted | Result |
|---|---|---|
| JW (author) | a whole cut onto head where head's group is the row's, whatever commit the row names | KILLED: `a_whole_cut_onto_a_head_that_is_not_the_rows_is_a_mismatch` |
| JD (author) | a delta onto head where head's group is the row's (n2) | KILLED: 3, among them `head_at_another_commit_of_the_cuts_group_takes_no_publish_and_no_delta` |
| V1D (author) | a version 1 row vouches for a delta by its group | KILLED: `after_a_row_of_version_1_…`, `a_cut_of_part_of_the_volume_against_another_row_is_a_mismatch` |
| V1W (author) | under a version 1 row no whole cut goes up | KILLED: `after_a_row_of_version_1_…`, `a_whole_cut_is_uploaded_onto_the_rows_commit_whatever_the_row_names` |
| H1 (author) | a cut found committed is handed on with head where head's group is the cut's (n1) | KILLED: `a_cut_found_committed_under_a_row_of_version_1_takes_no_publish`, `head_at_another_commit_…` |
| H2 (author) | under a version 1 row, handed on with head by its group | KILLED: `a_cut_found_committed_under_a_row_of_version_1_takes_no_publish` |
| HC (author) | found committed at the check: handed on with head whatever the row names | KILLED: 3, among them `a_cut_found_committed_at_the_check_after_head_moved_carries_no_publish` |
| HF (author) | found committed after a failed commit: handed on with head whatever the row names | KILLED: `a_cut_found_committed_after_head_moved_carries_no_publish`, `head_at_another_commit_…` |
| RE (author) | the row names another commit than the one it lands with | KILLED: 76 |
| EN (author) | the engine builds the ops from head's hash, not the commit's | KILLED: 77 (with `-p skyzfs-upload`); `ops_made_from_the_commits_hash_name_the_commit_that_lands` among them |
| EO (author) | ops built from the hash may swap the engine's own ref | KILLED: `ops_made_from_the_hash_that_swap_the_engines_own_ref_are_refused` |
| RD (author) | a version 1 row is not read | KILLED: 5 |
| R1 | rule 1 (found committed) also asks head to be the row's commit | KILLED: 4, among them `a_row_naming_the_cut_being_uploaded_says_it_is_committed` |
| R2 | the row's `naming` finds the cut by its group alone | KILLED: `row::tests::a_row_naming_the_cut_is_found_by_the_cut` |
| R3 | decode reads the commit from bytes 2..34 | KILLED: 77 |
| R4 | a version 2 row decodes without its commit | KILLED: 76 |
| R5 | a whole cut with no row goes up whatever head's group | KILLED: `a_whole_cut_onto_a_head_that_is_not_the_rows_is_a_mismatch`, `no_row_under_a_cut_of_part_of_the_volume_is_a_mismatch` |
| R6 | `commit_with`'s up-front check of its own ref removed | KILLED: `ops_that_swap_the_engines_own_ref_are_refused` |
| R7 | refused ops (the own ref) drop the engine's writes | KILLED: `ops_made_from_the_hash_that_swap_the_engines_own_ref_are_refused` |
| R8 | the hand-on gives the row's commit whatever head is (N6) | KILLED: 3, the found-committed-after-head-moved tests and `head_at_another_commit_…` |
| R9 | a delta judged by `head_is_the_rows` (by group for a version 1 row) | KILLED: `after_a_row_of_version_1_…`, `a_cut_of_part_of_the_volume_against_another_row_is_a_mismatch` |

n1 and n2 hold: `head_at_another_commit_of_the_cuts_group_takes_no_publish_and_no_delta` fails under JD (n2), and under H1, HC, HF and R8 (n1). No plant is aimed at B1's race, because no test of the suite could kill one.

### CI and Codex

- **Codex:**
  - Its code review of acda3e7 and of 4595f55 completed with no findings, and its security review of each completed. There are no review comments on the code.
  - One other review comment on the pull request covers node B's commit; see the node B section.
- **At 4595f55** (as of 21:39 UTC on 2026-11-24):
  - Green:
    - the cut rig, all three jobs (the upload job included), "which jobs" and "the check's logic";
    - the node B rig on ubuntu-26.04 and ubuntu-26.04-arm, and its logic;
    - the replay rig's jobs through skyzfsd on ubuntu-24.04 and 26.04, the "write into a hole" jobs included;
    - e2e, k3s-smoke;
    - `ci`: lint, test, unit-arm64, s3-backends and chart-lint;
    - `skyzfs-ci`'s gate.
  - Still running: the daemon rig on ubuntu-24.04, ubuntu-26.04 and ubuntu-26.04-arm (its `which server` and its judge's own test are green), and coverage. The daemon rig's verdict on this head is for the next look.
- **At acda3e7:**
  - The cut rig's upload job was red (B2, now closed).
  - `ci` and the replay rig were green.
  - The daemon rig was cancelled by the push of 4595f55.

### Gates

In the main tree at acda3e7, on rustc 1.100.0 (2026-11-09), which `rustup check` reports as the current stable (`gates.out`):
- `cargo fmt --all --check`: clean.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfsd --features test-hooks --all-targets -- -D warnings`: clean.
- `cargo clippy -p skyzfs-ublk-sys --features control-test-program --all-targets -- -D warnings`: clean.
- `cargo test --workspace`: 1681 passed, 0 failed, 16 ignored, over 114 test binaries. This is the description's count.
- `cargo test -p skyzfsd -p skyzfs-local --features skyzfsd/test-hooks,skyzfs-local/test-hooks`: 506 passed, 0 failed, 1 ignored.
- `cargo test -p skyzfs-ublk-sys --features control-test-program`: 37 passed, 0 failed, 11 ignored. The ignored ones are the kernel tests.
- At 4595f55, node B's gates are listed in its section. The rest of the workspace is unchanged from acda3e7.

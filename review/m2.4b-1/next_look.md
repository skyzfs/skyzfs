## Next look at #112 at 4c887b5 (M2.4b-1)

**Verdict: can merge. Nothing blocks.** The answer commit `c162639` settles G1 to G4, N1 and N2 as I meant them. All nine plants that passed the tests at 88e4edc now fail a test of the pull request. I found no defect in the code. Two things are left, neither blocking:

- **F1.** Three comments say that a cut kept at a failed write of the map is there "for the next start's check". That holds only where the failure was not marked on the disk. It also has a cost for the instance that saw the failure, which is a point for the lead and M2.5. My own suggestion at the first look started this, and it was not thought through.
- **F2.** `release_cut` says true with the cut still held. That is right, but M2.4b-2's uploader can misread it, and the doc does not say what a caller should look at.

I formed my view from `git diff 88e4edc..4c887b5` before I read the answer. P1 is not in this pull request and I did not review it.

### What I ran

- **The head's tests** of `skyzfs-local`: plain 266 and 11, with the test hooks 266 and 15, none failed (`next/t_plain.out`, `next/t_hooks.out`). At 88e4edc they were 261 and 11, and 261 and 14: the six new tests.
- **My whole plant set** on 4c887b5, with the test hooks: the 51 plants of the first look and six new ones for the answer commit (X1 to X6). 56 are killed, one passes and is not a gap (X6, below). Table in `next/plants.out`, a result per plant in `next/plants/`.
- **My scenarios** on the head: S1 to S3, T1, T3 to T7 and R1 as before, T3 rewritten for the new buffer, and two new ones for the release (T8, T9). All twelve pass (`next/scenarios.out`).
- The timing test `no_wake_is_lost_between_a_threads_look_and_its_wait` did not fail in any of these runs, at loads of 16 to 21.

### (1) The tests for G1 to G4 hold my plants as I meant them

| Plant | At 88e4edc | At 4c887b5 |
|---|---|---|
| A1: the whole mark stored first | passes | KILLED by `a_death_while_a_mismatch_abandons_leaves_the_oldest_cut_whole` alone |
| C1: a copy follows an image block | passes | KILLED by `a_copy_never_follows_a_block_of_the_image` alone |
| C6: a copy two slots on joins the run | passes | KILLED by `copies_are_one_read_only_where_their_slots_follow` alone |
| C7: an image block two on joins the run | passes | KILLED by `blocks_of_the_image_are_one_read_only_where_their_numbers_follow` alone |
| C11, C12: the summary past its first word | pass | KILLED by `a_map_is_listed_from_any_block_over_several_words_of_summary` alone |
| C14: `map_from` lists a cut's first part only | passes | KILLED by `a_death_while_a_mismatch_abandons_the_held_cuts` alone |
| V2, V3: what the start says of a header | pass | KILLED by `the_start_names_the_version_of_a_cut_state_only_where_it_is_another` alone |

- **These are my scenarios as tests**, with the same layouts: S2 (G1), T4, T6 and T7 (G2), S3 (G3), R1 (G4). I read each against the scenario it comes from and found nothing left out that a plant of mine rests on.
- **C14** is held where I found it: the death test now reads the region as each death left it (`CutState::view`) and compares every held cut's `map_from`, a block at a time, with `map` (`state/tests.rs:2006` to `2017`).
- **Each of the nine is held by one test only.** That is enough, and it means each of those tests is the only thing between the code and that mistake.

### (2) `release_cut` when the map cannot be written

**The code is right, and returning true is right.** My reasons differ a little from the answer's.

- **What the head does** (`inplace.rs:3127` to `3131`, scenario T8). The map's write fails, the volume fails, the cut stays held as the one being uploaded, and `release_cut` says true. The map in memory already names the cut (`inplace.rs:3156`, before the write at `3167`); the map on the disk names the old base or, where the header page landed, the new one.
- **Why true is right.** The doc says "Whether it was that cut; false changes nothing". Here something has changed: the volume has failed, the map in memory names the cut, and a page of the map on the disk may name it too. The commit in object storage landed before the call. So false would be untrue by the doc's own words.
- **The power-cut test is a sound witness, not only a bookkeeper.** Plant X2 (false there) is KILLED by `a_failed_write_of_the_map_at_a_commit_fails_the_volume` and by `after_a_power_cut_the_next_cut_holds_every_block_changed_since_the_newest_commit`, as the answer says. That test's model takes true to mean "the map may name this cut from now on" (`tests/cuts.rs:3991`), and a disk that halts after the header page makes exactly that so. I would give that as the reason in the comment, and not that a test counts by the value.
- **Plants on this path:** X1 (the cut let go all the same, as at 88e4edc), X2 and X3 (the volume not failed) are all KILLED.

#### F1 (for the lead, and a comment to correct; not blocking). What the kept cut is for, and what it costs

**The comments.** Three places say the kept cut is found by the next start's check:

- `inplace.rs:3112` to `3115`: "a death between, or a failed write of the map's pages, leaves the cut held for the uploader's check to find in the row";
- `inplace.rs:3128`: "the cut stays held for the next start's check, which finds it in the row";
- `tests/cuts.rs:3569`: "the uploader's check at the next start finds it held".

**What the store does** (scenarios T8 and T9, both run on the head):

- **The failure is marked on the disk, which is the usual course (T8).** A start that does not acknowledge the failure is refused (`MarkedFailed`). A start that does acknowledge it begins a new cut state ("the start acknowledges a failure") and holds no cut. It takes the intent map up, and the map names the old base, cut 1. So the uploader's check finds no cut held. It finds a row newer than the map's base, which is a mismatch and a whole upload, as at 88e4edc.
- **The failure is not marked (T9).** I used the test mutant that leaves the marker out, to stand for a daemon that dies between the failed write and the marker, or a disk that takes no marker. The next start in the same boot serves, takes the state up, and holds cut 2 as the one being uploaded. `release_cut(2)` then writes the map and lets it go. This is the one course on which the comments are true and the change saves a whole upload.

So the sentences hold for a start that takes the cut state up and for no other. The rig merged since in #113 says the same of the marked course ("A held cut is lost with the instance that saw the failure", `tests/kernel/cut_daemon_check.py`, `failure_line`).

**What the kept cut costs.** Design §5.2 ("When a cut may leave the machine") has the instance that saw the failure still upload a cut whose flush had completed, and leaves to M2.5 whether the daemon stays up for it.

- **At 88e4edc** the failed write let cut 2 go. Cut 3 became the one being uploaded and the map in memory named cut 2 (`next/gaps_X1.out`, T8's first line). That instance could have uploaded cut 3 as a delta onto the row.
- **At the head** cut 2 stays the one being uploaded for as long as the map's file refuses. A second `release_cut(2)` says true again and changes nothing (T8, second case). Only the cut being uploaded can be let go, so cut 3 cannot leave the machine from that instance.
- **Where the file takes a write again**, the second call writes the map and lets the cut go, on a volume that has failed (T8, third case). I see no harm in that; the start after finds the map naming cut 2.

Neither course can be seen in a daemon as built: it ends with status 3 soon after the failure, and there is no uploader. And a map that cannot be written usually means a disk that an upload could not read from either.

**What I would do.** Keep the head's code. Correct the three comments so that they name the course on which they hold. Record for M2.5 that a failed write of the map stops the failed instance at that cut, so that the decision there is made knowing it. All of it can go into M2.4b-2, which touches this function's neighbourhood for P1 anyway.

**My part in this.** At the first look I wrote that returning before the release "would keep the cut for check (a)". I had not looked at what a start does after a marked failure, nor at §5.2's sentence. The author did what I suggested and took my reason over. Certainty: high for T8 and T9 (they run); the weighing against M2.5 is a judgement, and the lead's to make.

#### F2 (suggestion, for M2.4b-2). True does not say the cut is gone

**Can the uploader misread it? Yes.**

- **Scenario.** The commit lands. `release_cut(N)` says true. The uploader takes "the cut being uploaded" as its next cut: it is N again. Its check finds the row naming N, which is rule 1, and calls `release_cut(N)`: true again, nothing changed. If the uploader goes on after a failure, as §5.2 has it do one day, that is a loop with no end while the file refuses.
- **What the head's doc says.** Both halves are there: "Whether it was that cut" (`inplace.rs:3101`) and "the cut stays held" (`3110`). No sentence joins them, and none says what a caller looks at.
- **The daemon's command.** `release` answers `ok` in this case too (`bins/skyzfsd/src/cut_commands.rs:339`), and its doc says "lets cut N go ... Then `TOKEN ok`" (`:37`). The cut rig on the base's tip reads that `ok` as a commit (`first_commit`), which is the right reading, but not what the command's doc says.

**Suggestion.** Either a result of three values (let go; committed and still held, the volume has failed; not the cut being uploaded), or one sentence in the doc: after true the cut may still be held, and the caller looks at `is_failed()` before it takes the next cut. M2.4b-2's driver is the first real caller, so that is the place.

**A small thing on the same path.** Each call counts a commit (`intent.commits`, `inplace.rs:3161`), so a release that is tried twice counts two commits for one cut.

### (3) N1: the buffer

**Fixed, by the type.**

- `read_cut_from` takes `&mut Buf` (`inplace.rs:2952`). A `Buf` is whole blocks of 4 KiB in memory aligned to 4 KiB, so no caller can pass a length or an address that a direct read fails on.
- **The buffer of no block** is refused with `InvalidInput` at `inplace.rs:2954`, before the state is looked at and before any read. It abandons nothing and fails nothing.
- **Scenario T3** on the head, with direct I/O and through the cache: an empty `Buf` and a `Buf` lent at no block are both refused, the held cuts are as before (cut 1 being uploaded, cut 2 waiting), and the volume has not failed. A `Buf` of four blocks lent at two takes two blocks and no more.
- **Tests that fail without it.** Plant X4 (no refusal) and plant X5 (the refusal abandons the cut being uploaded) are both KILLED by `a_slice_of_a_cut_not_held_or_into_a_short_buffer_is_refused` alone (`tests/cuts.rs:1711`), which asserts the kind of the error and that the cut is still being uploaded.

### The other findings

- **N2, the exact count: done as I proposed.** `merges` is true exactly when the region at the death holds cut 3 as waiting and cut 4 as flushing; the test asserts `counted + held == cuts.len()` at the start and `counted == cuts.len()` after the second `abandon_all` (`state/tests.rs:2018` to `2043`). S1 still shows the same rule at every death (7 of 35 with a flush under way). A5, A8 and A9 are killed.
- **N3: my finding was wrong.** The skip is not silent. `on_disk` (`tests/inplace_real.rs:58`) writes "skipped" with the reason, and fails where `CI` is set. The author is right to change nothing.
- **N4:** confirmed as intended by the lead. Nothing for this pull request.
- **P1:** decided, for M2.4b-2. T5 still shows the window on this head, as expected.
- **X6, the one plant that passes.** It makes `intent_committed` say false where the store holds cuts and keeps no map. No store is like that: a start that cannot keep the map is refused (`inplace.rs:2416`). The branch cannot be reached, so this is not a gap.

### The merge of `zfs-topology`

- **What it brought (298947d).** #110 (`skynbd-engine`, `skynbd-store`, `skynbd-refs`, `skynbd-format`), #111 (the cut rig and the benchmark script), the notes, the design document and the guide: twenty files. Nothing in `crates/skyzfs-local`, `bins/skyzfsd` or the crates they depend on. The cut rig drives the daemon through its commands and status lines, which 88e4edc..4c887b5 does not change. I see no semantic conflict, and the cut daemon jobs ran green on the merged head.
- **The base has moved since.** #113 merged into `zfs-topology` at 01:02 UTC on 18 November, after this head's CI. It changes `tests/kernel/`, three workflows, the notes and the guide, and no Rust. The head merges with the new tip without conflict (`git merge-tree`). #113's new stage of the cut rig, a failed volume with cuts held and a start that acknowledges it, has not run on this pull request's daemon. Nothing in the pull request changes what a failure by signal does, so I expect no difference; the guide's rule to dispatch the rig on `zfs-topology` after the merge covers it.

### CI and Codex

- **CI on 4c887b5.** 31 check runs succeeded and 5 were skipped; none failed. The green ones include the daemon rig on three images, the control rig, the replay rig through `skyzfsd` and its hole jobs, the cut daemon jobs on 26.04 and 26.04-arm, `unit-arm64`, e2e and coverage. The skipped ones are the duplicates inside the gate's run, the badges and the replay rig's reference jobs, which this pull request does not trigger. I read conclusions, not the rigs' tables.
- **Codex.** Its summary comment names 4c887b5 and has no findings. No inline comment and no review.

### What I could not verify

- **The kernel.** The rigs run in CI only, and I did not read their tables.
- **fmt, clippy and the tests outside `skyzfs-local`.** I did not run them again; CI did.
- **The uploader.** It is M2.4b-2. F2's misreading and F1's cost are reasoned from the description's rules and design §5.2, not run.
- **A real death before the marker.** T9 uses the test mutant that leaves the marker out.
- **#113's rig against this daemon**, as said above.
- **arm64 here.** The plants ran with the test hooks on amd64.

### Files

In `review/m2.4b-1/next/` on `skyzfs-review`:

- `plants.py` and `run_plants.py`, with `plants.out` and a result per plant in `plants/`;
- `scenarios_state.rs` (S1 to S3), `scenarios_store.rs` (T1, T3 to T9), `scenarios_real.rs` (R1) and `scenarios.py`, with `scenarios.out` (the head) and `gaps_X1.out` (the scenarios with the release as 88e4edc had it);
- `t_plain.out` and `t_hooks.out`, the head's own tests.

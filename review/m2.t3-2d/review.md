Review of #115 at fe6970c (M2.T3 2d)

**Verdict: it can merge. No finding blocks.** I found no defect in what the pull request changes that gives a wrong verdict on a recording the rig makes today. What I found:

- one line whose words are wrong most of the times it will fire (F1);
- one sibling of D1 that the new rule does not cover and that today's courses cannot reach (F2);
- one test that the pull request weakened: a plant the base's tests killed now survives (G1);
- gaps in the tests of the two new floors (G2 to G5);
- two sentences of the description that the recordings contradict, and a description that still describes afd217a.

I formed this from `git diff origin/zfs-topology...pr115` (ba05d28 to afd217a), the store's code and the design before I read the description, and then from `git diff afd217a..fe6970c` when the head moved. Only test code changes: `cut_daemon_check.py`, five test files and `skz_release_line` in the benchmark's script. `cut_ref.py` itself is unchanged, though the description's first paragraph names it; only its test has a new case.

## The questions

### 1. D1: the window between the barrier and the state storing the cut

**The rule is right for every window the store has at a flush.** I checked it against the store in two ways.

- *By reading.* `InPlace::flush` (`crates/skyzfs-local/src/inplace.rs:1743-1744`) calls `cuts.barrier(id)`, which writes the `barrier` line, counts in the rule and writes the `cut` line, and then `record_cuts()`, whose `freeze` (`inplace/state.rs:1379`) stores the cut's record, then the count in one store (`at::COUNT`), then the newest cut and the cleared mark. The flush's `commit` line follows the sync.
- *By running the store.* For the time of one run I added a test (`kill_at_stores.rs`; nothing of it is in the product) that lets the store die at each of its stores after the `cut` line and starts the next instance on what is left. There are 42 stores before the flush's answer. A death at stores 0 to 21 leaves a state that counts the same cut again at the same group when the flush is handed to it again. A death at stores 22 to 41 leaves a state that has the cut and hears a barrier with no cut (`kill_at_stores.out`). No store gives a third outcome. This is the description's table: C before the count's store, "stored" after it.

**A stored cut is not mistaken for C by a right daemon.** Once the count is stored the next instance's first cut is K+1, and `unstored_at_kill` asks for K again at the same group (`cut_daemon_check.py:1022-1028`). A daemon that stored K and then cut K again would be read as C, but only where the flush had no `commit` line, so nothing was promised on it.

**What the check says of each shape** (`kill_windows.py`: the fixture `life` split at its 13th cut and run through the program as `takeover`; `kill_windows-fe6970c.out`):

| Shape | Result |
|---|---|
| A, the next instance counts on past a cut with no line | exit 2, "no kill leaves this" |
| B | `cuts` PASS, the line names the barrier |
| C | `cuts` PASS, the line names cut 13 |
| stored | `cuts` PASS, 0 flushes named |

**Wrong daemons I tried to pass through the new rule, and could not:**

- One that drops the mark at a take-over, under shape B: the reference keeps the mark and cuts at the next instance's barrier, so `cuts` FAILs as before.
- One that counts a cut before it writes its line: the next instance counts on from K+2, the barrier is kept (`skips`, `:1017`) and `cut_line_missing` ends the run as the rig's error. Plants U3 and U5 hold this.
- One that cuts K again at another group, or K+1 at the same group: not shape C (plants U6 and U7 killed).

**Right daemons that fail or are named wrongly: F1, F2 and R1 below.** None arises in a recording the rig makes today.

### 2. The two floors, and the rule of fe6970c

**`held_across_start` counts what the review of #111 counted.** The description says its count "differs" from mine and that my figures (20, 69 and 28) "counted copies another way". That is not so. On each of seven recordings the check's count and my method of #111 (`review/m2.t3-2b/copies_lost_at_start.py`: the blocks whose content at the next start is not the cut's) give the same number (`floors-fe6970c.out`):

| Recording | By `held_across_start` | By the #111 method |
|---|---|---|
| run 44217572301 (afd217a), ubuntu-26.04 | 17 | 17 |
| run 44217572301, ubuntu-26.04-arm | 4 | 4 |
| run 44222833248 (fe6970c), ubuntu-26.04 | 16 | 16 |
| run 44222833248, ubuntu-26.04-arm | 14 | 14 |
| #113's run, amd64 | 4 | 4 |
| #113's run, arm64 | 4 | 4 |
| #113's set of 14, amd64 | 282 | 282 |

The 20, 69 and 28 were three recordings at 8210332, before 1f71211 made the holder wait. At 1f71211 I counted 4 and 4 with the same script (`review/m2.t3-2b/next_copies_lost_at_start.out`). The recordings changed, not the count. The figures testing gave for #113's three recordings at fe6970c (4, 282 and 4) are what I get.

**The rule of fe6970c is right, and it is the rule my #111 count used.** A block now counts only where the last write before the next start left a known content that is not the cut's (`:788-799`).

- *Codex's case is closed.* A, B, A counted 1 at afd217a and counts 0 at fe6970c (`last_known.py`, both outputs). No block of the seven recordings is in that case, so no figure moved.
- *Can a right daemon fall under the floor because a block is wrongly "not known"? Not in `takeover`.* A block is not known after a write that covers it in part, a write-zeroes, or a write with no hashes. The seven `takeover` recordings have no write-zeroes, no discard and no write that is not a whole number of 4 KiB blocks (the pool has `ashift=12`, `cut-record.sh:186`), and a held stage with no hashes is the rig's error before the floor is asked. In all seven, 0 blocks are "not known" (`floors-fe6970c.out`).
- *Is "not known" the right reading of a write-zeroes?* Yes, as the conservative one. `records_of` places a write-zeroes at its receipt and not where the store ran it (`:498-510`), so the order of records around one does not say what the block holds. Calling the block not known can only lower the count. One edge goes the other way; see R2.
- *A discard is not looked at*, and need not be: it changes no content in the store (`:516-520`).
- *It errs on the low side.* "A, then B, then a write-zeroes" counts 0 though the block holds zeros and a copy is needed (`last_known-fe6970c.out`). That is the safe direction for a floor.

**Can a daemon that loses its copies at a start still pass? No.** I gave each counted block of the cut the content it had at the next start, as a daemon that took the map up without the record of its copies would write it (T1 in `floors.py`). All seven recordings end with `held_content` FAIL and exit 1, with 17, 4, 16, 14, 4, 4 and 282 blocks named. The verdict is `held_content`'s. The floor only says that the recording had something for it to judge.

**Can a right daemon fall under the floor for another reason? Only on a runner about three times slower than the slowest I have.** In three of the seven recordings the count is exactly four blocks, and in all seven those four are counted: the four copies of one slot of the uberblock ring, the slot the held cut's own uberblock was written to, overwritten when the ring came round 31 cuts later. In six recordings that was the first change to the cut's map, 0.38 to 0.83 s after the cut's instant. The writer runs for 3 s before the kill (`cut-record.sh:766`), and the first instance counted 95 to 213 more cuts in that time, where 31 are needed. Below about 10 groups a second the count can be zero, and the run then ends with exit 2. That is the right class (the rig did not show it), but the job would be red for speed. See S1.

**`carried_across_failure`.** 1784 and 1791 of 6180 blocks in the run on afd217a, 1784 and 1792 of 6177 in the run on fe6970c, and 1775, 1792 and 1793 on #113's three recordings judged here, as the description says. A right daemon reaches zero only if the load between the old holder's last write and the failure changed nothing, which the course does not allow. The nine wrong daemons of the review of #113 (`scenarios_failed.py`, S1 to S9, on both `failed` recordings of run 44217572301) all still end with exit 1. A daemon that carries nothing of the regions (S3) gets `carried_across_failure` INFO, "44 of its 102 blocks", and `held_content` FAIL with 1740 blocks: the floor does not excuse it.

**Is ERROR at zero right? Yes, with one remark.** `errored` turns exit 0 into exit 2 and leaves a verdict alone (`:1562`), so a floor cannot hide a FAIL. Zero by itself says nothing about the daemon's copies, so it is the rig's to say. The remark is S2: one defect of the daemon ends here as the rig's error and nowhere as a verdict.

### 3. `parse_log`'s bound of 20 digits

**It closes my remark in `review/m2.t3-2c/next_look3.md`.** I ran that review's script on this head (`next3_bound-fe6970c.out`):

| Case | At 73869d1 | At fe6970c |
|---|---|---|
| The next line's time has 20 digits | torn line taken | torn line taken |
| The next line's time has 21 digits | parsed as a start with a longer time, no line lost | the rig's error, "a time of 21 digits" |
| The next line's time has 5000 digits | Python's `ValueError` | the rig's error, "a time of 5000 digits" |
| Left of the torn line: "0" and eleven digits | taken | the rig's error (`TORN_HEAD` asks for `[1-9]`, `:247`) |

All 18 shapes of a real tear are still taken with times of 9, 12, 13 and 20 digits and across the change from 12 to 13. A u64 of nanoseconds has 20 digits at most, so the bound refuses nothing a daemon writes. Plants T1 to T4 (the bound dropped, set to 12, set to 19; `int()` on a long next line) are killed.

## Findings

Most severe first. None blocks the merge.

### Defects

**F1. `obs_unstored_at_kill` calls every barrier at the end of an instance "a flush the daemon was killed at, between the barrier and the state storing the cut". Most such barriers had no cut to store.** `cut_daemon_check.py:1018-1021` and `:1037-1038`.

- A flush that carries no cut also writes its `barrier` line and then syncs for milliseconds before its `commit`. A kill in that sync leaves the same last line as window B.
- Scenario N in `kill_windows.py`: the first instance ends on the barrier of the flush before the uberblock writes of group 26. No cut was at stake. The line says "1 flushes the daemon was killed at, between the barrier and the state storing the cut; line 2139: pid 4242's barrier with no cut line".
- How often: by the request logs of 14 killed instances in seven recordings (`window_share.out`), a kill at a random moment ends on a barrier line 0.4% to 1.1% of the time, and 96% to 99.9% of that time belongs to a flush with no cut. Window B itself is 0.001% to 0.018% of an instance's life.
- Effect on the verdict: none. Taking out the barrier of a flush that was never answered changes nothing for the reference.
- Effect on the reader: about one run in a hundred, per kill, will say the rig hit D1's window when it did not. The first such run would be reported to the lead as a first, wrongly.
- A change of words is enough: "a flush killed before its answer; if it carried a cut, the cut was not stored". The reference could say which, since it knows whether a mark was waiting at that barrier.
- Certainty: high. Run.

**F2. A kill between a marking write's `applied` line and the state storing the mark makes `cuts` FAIL for a right daemon, if the next instance receives another flush before the write is handed to it again.** This is D1's sibling for the mark. The pull request's rule covers the cut only.

- The store: the write path calls `cuts.applied(id)`, which writes the `applied` line, and then `record_cuts()`, which stores the mark (`inplace.rs:1617-1618`). A kill between leaves a log that says the uberblock write was applied and a state with no mark.
- I ran this on the store (`kill_at_stores.rs`, second test). After a death at the `applied` line the next instance, handed another flush first, hears a barrier and no cut. It cuts at the group's own flush after the write comes again (`kill_at_stores.out`, first line).
- The check: the reference marks at the first instance's `applied` line and keeps the mark over the take-over, so it cuts at that other flush. Scenario M1 in `kill_windows-fe6970c.out`: `cuts: FAIL: cut 13: the daemon's is group 26 (itself) at record 735, the reference's group 26 (itself) at record 729`, and `ctl_mark-dropped: MISSED`.
- With no other flush in between (scenario M0) the check passes.
- Not reachable in `takeover` today, by my reading: the writer is `dd` and `zpool sync`, ZFS sends its flush after its uberblock writes return, and nothing sends a flush of its own (no `fsync`, no log device). A course with synchronous writes across a kill could reach it. The window is microseconds wide.
- Certainty: high for what the store and the check do (both run); medium that no course reaches it (read, not run: no kernel here).

### Remarks on the rules' edges

**R1. Shape C is taken only where the very next instance cuts K again at the same group.** Two right daemons fall outside it (`kill_windows-fe6970c.out`, C2 and Cc):

- C2: the next instance is killed too before the flush reaches its barrier, and a third cuts K. `cuts`, `cut_numbers` and two instance lines FAIL. It needs two kills within milliseconds; `takeover`'s second instance lives for seconds.
- Cc: the next instance created the device. It takes the state up with K-1 cuts, drops the mark, and its first cut is K at a later group. `cut_numbers: FAIL: cut line 14 ... carries the number '13'`. No recording kills the daemon and then creates the device: `takeover`'s created start follows a SIGTERM.
- Whoever adds such a course will meet these. "The next cut line of the same state, in any later instance, carries K again" would take both.
- Certainty: high for the check's results (run). The daemon's side of Cc is by reading `take_up` and the reference's rule.

**R2. One order of a write and a write-zeroes is counted by `held_across_start` though the home holds the cut's content.** `cut_daemon_check.py:790-798`.

- The cut holds a block as zeros. A write B to it is received, then a write-zeroes over it, then B is applied. The store runs B first and the write-zeroes after it, so the block holds zeros again. `records_of` puts the write-zeroes at its receipt, before B's `applied` line, and marks the block `ambiguous` (`:498-510`). The new rule reads B as the last content and counts the block (`last_known-fe6970c.out`, last line: 1 counted).
- Reading a block in a write-zeroes' `ambiguous` set as not known would close it.
- No weight today: no `takeover` recording has a write-zeroes, and ZFS sends none (`edges` sends its own).
- Certainty: high for the function (run); the store's order is the check's own comment, which I did not run.

### Gaps in the tests

**G1. No test places a tear by a number of bytes any more: a plant that the base's tests killed now survives.** `PAGE = 8192` in `cut_daemon_check.py` (plant P2 of the review of #113) was KILLED at 73869d1 and is KILLED on the base ba05d28 by `test_a_line_torn_at_the_take_over` (`plants-base-ba05d28.out`). It SURVIVES at afd217a and at fe6970c, and so does `PAGE = 16384`.

- Why: the pull request rewrote that test's fixture as the helper `tearer` (`test_cut_daemon_main.py:184-209`), which computes its filling from `cdc.PAGE`, as `test_cut_daemon_check.py:922-923` does. The fixtures now move with the constant. The new test for P3 (2048) kills the smaller page, not a larger one.
- What a wrong `PAGE` of 8192 would do on a runner: half of the real tears fall on an odd 4 KiB page and would not be taken, and the run would end as the rig's error.
- One assertion with the tear at byte 4096 or 12288 of the file, written as a number, holds it.
- This is the one place where the pull request weakens a test. I do not think it was meant, and it does not change what the check does. Not blocking; it should be fixed here or named for the next package.

**G2. No test holds that a cut held across the start with no block changed is under the floor.** Plant H8 (the `if x[3]` at `:1387` removed, so that "0 blocks" meets the floor) survives. The test covers a holder that wrote every cut before the start, not a cut written across it with nothing changed, which is what the two later cuts of every recording are.

**G3. No test holds that either floor's ERROR reaches the exit.** Plants H10 and A6 (`errored = errored or not across` at `:1393` and `errored = errored or not ok` at `:1401` removed) survive. The fixture's exit is 1 for another reason (`ctl_mark-kept` DIFFERS), so the tests read the line and not the exit. With either line gone, a run that showed nothing would end with exit 0.

**G4. The new rule's handling of an acknowledged start is tested in the function and not through `main`.** Plant U14 (`main` calls `unstored_at_kill(instances)` without `acked`) survives. So do U4 (an acknowledged next instance may also "count past one more") and U11 (the count before the kill is not begun again at an acknowledged start). U10 and U15 (shape C whatever stands before the cut line) survive too: no test has a cut line at the end of an instance behind another flush's barrier.

**G5. Three smaller survivors in `held_across_start` and the torn line.**

- H7: a block outside the cut's map counts. No test writes such a block in the span.
- L7: a block once not known stays so, whatever is written after. No test has a whole write after a partial one.
- T5: `TORN_HEAD` takes leading zeros again. No test has a torn line that begins with 0.

H2, H3 and L8 also survive and are no gaps: with one start between a cut and its write, with the cut's own content left by the last write before its instant, and with a write-zeroes record that never carries hashes, each plant computes the same number.

### Suggestions

**S1. Give the take-over's writer a number of groups and not 3 s.** The floor needs the first instance to go 31 cuts past the held cut. The slowest run I have did that in 0.83 s. A loop that ends 40 groups after the holder left would not depend on the runner.

**S2. A daemon that lets go of what it held at a take-over ends as the rig's error.** T2 in `floors.py`: the cut held across the start is never written, and the next written cut has its blocks too. `held_content` PASS, `held_across_start` ERROR, exit 2, on all seven recordings. Nothing judges `cuts_held` at a take-over (`status-start-2.txt` says `cuts_held 2` in the recordings; G3 of #113 judges it only at an acknowledged start, `:1124`). Holding `cuts_held` at a taken-over start to at least 1, where the holder left with a cut held, would make it a verdict.

## The description's claims

**The description still describes afd217a.** Its commit list and its "How it was verified" do not name fe6970c, and the lead reads the description as the report.

| Claim | What I found |
|---|---|
| Local tests pass | Confirmed at fe6970c: `test_cut_daemon_check` 90 tests, `test_cut_daemon_main` 22, `test_cut_hold` 17, `test_cut_ref` 41, `test_cut_check` 26; `test_skz_store.sh` and `test_cut_record.sh` all passed. shellcheck is not installed here. |
| Each plant of the table KILLED | Confirmed for every one; see the next table. |
| 42 recordings of #113's runs exit 0 with the new check | Confirmed at fe6970c on the three sets of 14 I have from #113's runs (both images of one run, and a set whose `takeover` has the torn label line), with the description's figures (`rejudge-113-fe6970c.out`). |
| `obs_unstored_at_kill`: 0 in every recording | Confirmed on those 42 and on the 56 of this pull request's two runs. |
| "The review's figures counted copies another way" | Not so; see question 2. |
| The work is in "the reference (`cut_ref.py`)" | `cut_ref.py` is unchanged; `test_cut_ref.py` has one new test. |
| The author's three plants in fe6970c's rule (any write counts, a block not known counts, a write-zeroes ignored) each fail the test | Confirmed: my L1, L3 and L4, all KILLED by `test_held_across_start`. |
| The cancelled rigs run no changed file | Confirmed by reading the workflows. None of the daemon rig, the replay rig, the control rig, the kernel probe and node B's rig names `cut_daemon_check.py`, its tests, `test_cut_hold.py`, `test_cut_ref.py`, `test_skz_store.sh` or the benchmark's script. The cut rig on dm-log-writes runs `test_cut_ref.py`, which fe6970c does not change; it was green on afd217a. |

**The plants of the description's table, planted here one at a time.** "Re-planted" means the older review's text no longer applies and I planted the same fault in this head's text.

| Plants | Review | Result here | Killed by |
|---|---|---|---|
| C2, C3, C4 | #111 | KILLED | `test_nothing_of_the_first_instance_carries_over` |
| C9 | #111 | KILLED | `test_instances_do_not_interleave` |
| R7 | #111 | KILLED | `test_a_cut_clears_only_its_own_mark_from_the_state` |
| I10, I11 | #111 | KILLED | `test_what_cannot_be_judged` |
| N2, N3 | #111 | KILLED | `test_groups_synced_with_no_flush_and_not_cut` |
| K3, K4 | #111 | KILLED | `test_a_missing_cut_line_by_hand` |
| K5 | #111 | KILLED, re-planted | `test_a_kill_before_the_state_stored_the_cut` |
| K6 | #111 | KILLED | `test_a_kill_before_the_state_stored_the_cut` |
| B4 | #111 | KILLED, re-planted, and three plants in the new `sed` | `test_skz_store.sh` |
| B5 | #111 | KILLED | `test_skz_store.sh` |
| K3, K5 | #113 | KILLED | `test_a_read_refused_that_is_not_the_daemons_own`, `test_a_read_refused_of_a_cut_that_went` |
| N5 | #113 | KILLED | `test_every_line_of_a_new_state` |
| S3 | #113 | KILLED | `test_the_cuts_since_a_new_state` |
| F7 | #113 | KILLED, re-planted | `test_the_failure_and_later_states` |
| Q2, Q3 | #113 | KILLED | `test_the_failure_and_later_states` |
| X5 | #113 | KILLED (as X5b, the later text of the same plant) | see `plants-2c-next.out` |
| B3 | #113 | KILLED, re-planted | `test_a_torn_line_not_followed_by_a_start` |
| B7, B8 | #113 | KILLED | see `plants-2c-next.out` |
| P3, P6 | #113 | KILLED | see `plants-2c-next.out` |
| A7, K9 | #113 | SURVIVED, as the description says (not tested) | |
| R10, C1 | #111 | NOT APPLIED, as the description says (the text is gone) | |

## Plants

All one at a time, each file restored after it, every checkout clean afterwards (`plants.py`, `plants_compare.py`).

| List | Where it ran | Plants | KILLED | SURVIVED | NOT APPLIED |
|---|---|---|---|---|---|
| This review's, in what #115 changed (`plants-head2-a.out`, `-b.out`) | fe6970c | 59 | 45 | 14 | 0 |
| The same list in afd217a's text (`plants-new.out`) | afd217a | 51 | 41 | 10 | 0 |
| The review of #111's two lists (`plants-2b.out`, `plants-2b-next.out`) | afd217a | 82 | 49 | 13 | 20 |
| The review of #113's four lists (`plants-2c.out`, `-next.out`, `-rest.out`) | afd217a | 162 | 119 | 8 | 35 |
| Older plants re-planted in this head's text (`plants-again-fe6970c.out`) | fe6970c | 21 | 19 | 2 | 0 |
| P2 on the base (`plants-base-ba05d28.out`) | ba05d28 | 1 | 1 | 0 | 0 |

- **The older lists ran at afd217a and I did not run them again at fe6970c.** fe6970c changes only `held_across_start` and `test_held_across_start`. No plant of the older lists is in that function, and none was killed by that test.
- **Against the older reviews' results** (`plants_compare.out`): 33 plants that survived there are killed here, which are the description's table and a few more. One plant that was killed there survives here: P2, which is G1. The 13 of #111 and 7 of #113 that survived there and survive here are ones the description does not claim.
- **The 14 survivors of my own list at fe6970c** are U4, U10, U11, U14, U15 (G4); H8 (G2); H10 and A6 (G3); H7, L7 and T5 (G5); H2, H3 and L8 (the same number either way). The two of the re-planted list are P2 at 8192 and at 16384 (G1).
- **Of the 55 that did not apply:**
  - 16 change text that this pull request rewrote. I planted the same fault in this head's text (K2, K5, B3, B4 of #111; F6, F7, F8, I4, B1, B2, B3, B5, B6, B10, P16, P19 of #113), and all 16 are killed. They are in the re-planted list's row.
  - 21 did not apply at #113's last head either.
  - 16 of #111's had no text left before this pull request, and #113's C3 and B4 have none at this head (B4 is the `lstrip('0')` that is gone).

## CI and Codex

- **Codex.** One finding on afd217a (P2: a restored block counted). The author answered it on the thread and fixed it in fe6970c. Codex's review of fe6970c is complete with no findings. Its security review has none.
- **On fe6970c:** the cut rig on the daemon 44222833248 is green, 14 recordings exit 0 on each image, `held_across_start` 16 and 14 blocks, `carried_across_failure` 1784 and 1792 of 6177. `ci` 44222833323, `coverage` 44222833207, `e2e` 44222833275 and the push's `skyzfs-ci` 44222827169 are green. The cut rig on dm-log-writes and the other rigs were cancelled by the author on this head, with the reason on the pull request; I agree with it (see the description's claims).
- **On afd217a:** the cut rig on the daemon 44217572301 green (17 and 4 blocks; 1784 and 1791 of 6180), the cut rig on dm-log-writes 44217572240 green on all four images, `ci`, `coverage` and `e2e` green.
- **Judged here at fe6970c:** the 56 recordings of both runs end with exit 0 and the runs' own lines (`rejudge-fe6970c.out`).

## What I could only read

- **Nothing that needs a kernel ran here.** The recordings are CI's. I rejudged them; I did not make them.
- **The daemon under a real `kill -9` in these windows.** I ran the store's own code with a death at each store, in its unit-test harness, not `skyzfsd` under a signal. No recording has hit window B or C (0 in all 98 I have).
- **That no course reaches F2**, and the store's order of a write and a write-zeroes in R2: by reading.
- **The 16 KiB and 64 KiB page cases of the torn-line rule**, as before: the hosted images have 4 KiB pages.
- **shellcheck** on the two changed shell files: not installed here. CI's `ci` run was green on afd217a.

## Evidence

All in `review/m2.t3-2d/` on `skyzfs-review`.

| File | What it is |
|---|---|
| `kill_windows.py`, `kill_windows-fe6970c.out` | the shapes A, B, C, stored, N, M0, M1, C2, Cc through the check |
| `kill_at_stores.rs`, `.py`, `.out` | the store with a death at each store after the `cut` line, and after the `applied` line |
| `window_share.py`, `.out` | each window's share of a killed instance's life, 14 instances |
| `floors.py`, `floors-fe6970c.out` | the floors on seven recordings: the counts by both methods, where the blocks lie, the margin, T1 to T3 |
| `last_known.py`, `last_known-afd217a.out`, `-fe6970c.out` | Codex's case and the new rule on records made by hand |
| `scenarios_failed.py`, `scenarios_failed-fe6970c.out` | the wrong daemons of the review of #113 on `failed` |
| `rejudge-fe6970c.out`, `rejudge-113-fe6970c.out` | 56 recordings of this pull request's runs and 42 of #113's, judged here |
| `next3_bound-fe6970c.out`, `next3_p6-fe6970c.out` | the time bounds |
| `plants.py`, `plants-*.out`, `plants_compare.py`, `.out` | the plants |

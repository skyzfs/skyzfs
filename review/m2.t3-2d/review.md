Review of #115 at afd217a (M2.T3 2d)

**Verdict: it can merge. No finding blocks.** I found no defect in what the pull request changes that gives a wrong verdict on a recording the rig makes today. I found one line whose words are wrong most of the times it will fire (F1), one sibling of D1 that the new rule does not cover and that today's courses cannot reach (F2), and gaps in the tests of the two new floors (G1 to G3). The description has two sentences that the recordings contradict (under "The description's claims").

I formed this from `git diff origin/zfs-topology...pr115` (ba05d28 to afd217a), the store's code and the design before I read the description. Test code only changes, as the description says: `cut_daemon_check.py`, five test files and `skz_release_line` in the benchmark's script. `cut_ref.py` itself is not changed, though the description's first paragraph names it; only its test is.

## The three questions

### 1. D1: the window between the barrier and the state storing the cut

**The rule is right for every window the store has at a flush.** I checked it against the store in two ways.

- *By reading.* `InPlace::flush` (`crates/skyzfs-local/src/inplace.rs:1743-1744`) calls `cuts.barrier(id)`, which writes the `barrier` line, counts in the rule and writes the `cut` line, and then `record_cuts()`, whose `freeze` (`inplace/state.rs:1379`) stores the cut's record, then the count in one store (`at::COUNT`), then the newest cut and the cleared mark. The flush's `commit` line follows the sync.
- *By running the store.* I added a test for the time of the run (`kill_at_stores.rs`, not committed to the product) that lets the store die at each of its stores after the `cut` line and starts the next instance on what is left. Of 42 stores before the flush's answer, a death at stores 0 to 21 leaves a state that counts the same cut again at the same group when the flush is handed to it again, and a death at stores 22 to 41 leaves a state that has the cut and hears a barrier with no cut (`kill_at_stores.out`). No store gives a third outcome. This is the table in the description: C before the count's store, "stored" after it.

**A stored cut is not mistaken for C by a right daemon.** Once the count is stored the next instance's first cut is K+1, and `unstored_at_kill` asks for K again at the same group (`cut_daemon_check.py:1016-1022`). A daemon that stored K and cut K again would be read as C, but only where the flush had no `commit` line, so nothing was promised on it.

**What the check says of each shape** (`kill_windows.py`, on the fixture `life` split at its 13th cut and run through the program as `takeover`; `kill_windows.out`):

| Shape | Result on this head |
|---|---|
| A, the next instance counts on past a cut with no line | exit 2, "no kill leaves this" |
| B | `cuts` PASS, the line names the barrier |
| C | `cuts` PASS, the line names cut 13 |
| stored | `cuts` PASS, 0 flushes named |

**Wrong daemons I tried to pass through the new rule, and could not:**

- One that drops the mark at a take-over, under shape B: the reference keeps the mark and cuts at the next instance's barrier, so `cuts` FAILs as before.
- One that counts a cut before it writes its line: the next instance counts on from K+2, the barrier is kept (`skips`, `:1011`) and `cut_line_missing` ends the run as the rig's error. Plants U3 and U5 hold this.
- One that cuts K again at another group, or K+1 at the same group: not shape C (plants U6, U7 killed).

**Right daemons that fail, or are named wrongly: F1, F2 and R1 below.** None arises in a recording the rig makes today.

### 2. The two floors

**`held_across_start` counts what the review of #111 counted.** The description says its count "differs" from mine and that my figures (20, 69 and 28) "counted copies another way". That is not so. On each of five recordings the function's count and my method of #111 (`review/m2.t3-2b/copies_lost_at_start.py`: the blocks whose content at the next start is not the cut's) give the same number (`floors.out`):

| Recording | By `held_across_start` | By the #111 method |
|---|---|---|
| this run, ubuntu-26.04 | 17 | 17 |
| this run, ubuntu-26.04-arm | 4 | 4 |
| #113's run, amd64 | 4 | 4 |
| #113's run, arm64 | 4 | 4 |
| #113's set of 14, amd64 | 282 | 282 |

The 20, 69 and 28 were three recordings at 8210332, before 1f71211 made the holder wait; at 1f71211 I counted 4 and 4 with the same script (`review/m2.t3-2b/next_copies_lost_at_start.out`). The recordings changed, not the count.

**Can a daemon that loses its copies at a start still pass? No.** I gave each counted block of the cut the content it had at the next start, as a daemon that took the map up without the record of its copies would write it (T1 in `floors.py`). All five recordings end with `held_content` FAIL and exit 1, with 17, 4, 4, 4 and 282 blocks named. The verdict is `held_content`'s; the floor only says that the recording had something for it to judge.

**Can a right daemon fall under the floor? Only on a runner about three times slower than the slowest I have.** In four of the five recordings the count is exactly four blocks, and they are the four copies of one slot of the uberblock ring: the slot the held cut's own uberblock was written to, overwritten when the ring came round 32 groups later. That happened 0.38 to 0.83 s after the cut's instant; the writer runs for 3 s before the kill (`cut-record.sh:766`), and the first instance counted 95 to 222 more cuts in that time, where 32 are needed. Below about 11 groups a second the count is zero and the run ends with exit 2. That is the right class (the rig did not show it), but the job would be red for speed. See G4.

**`carried_across_failure`.** 1784 of 6180 blocks on amd64 and 1791 of 6180 on arm64 in this run; 1775, 1792 and 1793 on #113's three recordings judged with this head, as the description says. A right daemon reaches zero only if the load between the old holder's last write and the failure changed nothing, which the course does not allow. The nine wrong daemons of the review of #113 (`scenarios_failed.py`, S1 to S9, on both of this run's `failed` recordings) all still end with exit 1. A daemon that carries nothing of the regions (S3) gets `carried_across_failure` INFO, "44 of its 102 blocks", and `held_content` FAIL with 1740 blocks: the floor does not excuse it.

**Is ERROR at zero right?** Yes, with one remark. `errored` turns exit 0 into exit 2 and leaves a verdict alone (`:1556`), so a floor cannot hide a FAIL. Zero by itself says nothing about the daemon's copies, so it is the rig's to say. The remark is G5: one defect of the daemon ends here as the rig's error and nowhere as a verdict.

**Codex's finding on this function is right in the letter and has no weight today.** A block written with other content and then with the cut's content again before the next start is counted (`restored.py`: A, B, A gives 1). In the five recordings no counted block is in that case (`floors.out`, "held the cut's content again at the next start: 0"), and ZFS does not write old content back: it writes new blocks, and an uberblock slot gets a higher group each time. Counting by the last content before the start, as Codex proposes, is cheap and makes the line mean what it says. Not blocking.

### 3. `parse_log`'s bound of 20 digits

**It closes my remark in `review/m2.t3-2c/next_look3.md`.** I ran that review's script on this head (`next3_bound-afd217a.out`):

| Case | At 73869d1 | At afd217a |
|---|---|---|
| The next line's time has 20 digits | torn line taken | torn line taken |
| The next line's time has 21 digits | parsed as a start with a longer time, no line lost | the rig's error, "a time of 21 digits" |
| The next line's time has 5000 digits | Python's `ValueError` | the rig's error, "a time of 5000 digits" |
| Left of the torn line: "0" and eleven digits | taken | the rig's error (`TORN_HEAD` asks for `[1-9]`, `:247`) |

All 18 shapes of a real tear are still taken with times of 9, 12, 13 and 20 digits and across the change from 12 to 13. A u64 of nanoseconds has 20 digits at most, so the bound refuses nothing a daemon writes. Plants T1 to T4 (the bound dropped, set to 12, set to 19; `int()` on a long next line) are killed.

## Findings

Most severe first. None blocks the merge.

### Defects

**F1. `obs_unstored_at_kill` calls every barrier at the end of an instance "a flush the daemon was killed at, between the barrier and the state storing the cut". Most such barriers had no cut to store.** `cut_daemon_check.py:1012-1015` and `:1031-1032`.

- A flush that carries no cut also writes its `barrier` line and then syncs for milliseconds before its `commit`. A kill in that sync leaves the same last line as window B.
- Scenario N in `kill_windows.py`: the first instance ends on the barrier of the flush before the uberblock writes of group 26. No cut was at stake. The line says "1 flushes the daemon was killed at, between the barrier and the state storing the cut; line 2139: pid 4242's barrier with no cut line".
- How often: by the request logs of ten killed instances in five recordings (`window_share.out`), a kill at a random moment ends on a barrier line 0.4% to 1.1% of the time, and 96% to 99.9% of that time belongs to a flush with no cut. Window B itself is 0.002% to 0.018% of an instance's life.
- Effect: none on the verdict. Taking out the barrier of a flush that was never answered changes nothing for the reference. But the line will be read as "the rig hit D1's window" in about one run in a hundred per kill, and it did not. The first run that shows it would be reported to the lead as a first, wrongly.
- A fix in the words is enough: "a flush killed before its answer; if it carried a cut, the cut was not stored". The reference could say which: it knows whether a mark was waiting at that barrier.
- Certainty: high. Run.

**F2. A kill between a marking write's `applied` line and the state storing the mark makes `cuts` FAIL for a right daemon, if the next instance receives another flush before the write is handed to it again.** This is D1's sibling for the mark; the pull request's rule covers the cut only.

- The store: the write path calls `cuts.applied(id)`, which writes the `applied` line, and then `record_cuts()`, which stores the mark (`inplace.rs:1617-1618`). A kill between leaves a log that says the uberblock write was applied and a state with no mark.
- I ran this on the store (`kill_at_stores.rs`, second test): after a death at the `applied` line the next instance, handed another flush first, hears a barrier and no cut, and cuts at the group's own flush after the write comes again (`kill_at_stores.out`, first line).
- The check: the reference marks at the first instance's `applied` line and keeps the mark over the take-over, so it cuts at that other flush. Scenario M1 in `kill_windows.out`: `cuts: FAIL: cut 13: the daemon's is group 26 (itself) at record 735, the reference's group 26 (itself) at record 729`, and `ctl_mark-dropped: MISSED`.
- With no other flush in between (M0) the check passes.
- Not reachable in `takeover` today, by my reading: the writer is `dd` and `zpool sync`, ZFS sends the flush after its uberblock writes return, and nothing sends a flush of its own (no `fsync`, no log device). A course with synchronous writes across a kill could reach it. The window is microseconds wide.
- Certainty: high for what the store and the check do (both run); medium that no course reaches it (read, not run: no kernel here).

### Remarks on the rule's edges

**R1. Shape C is taken only where the very next instance cuts K again at the same group.** Two right daemons fall outside it (`kill_windows.out`, C2 and Cc):

- C2: the next instance is killed too before the flush reaches its barrier, and a third cuts K. `cuts`, `cut_numbers` and two instance lines FAIL. It needs two kills within milliseconds; `takeover`'s second instance lives for seconds.
- Cc: the next instance created the device. It takes the state up with K-1 cuts, drops the mark, and its first cut is K at a later group. `cut_numbers: FAIL: cut line 14 ... carries the number '13'`. No recording kills the daemon and then creates the device: `takeover`'s created start follows a SIGTERM.
- Whoever adds such a course will meet these. "The next cut line of the same state, in any later instance, carries K again" would take both.
- Certainty: high for the check's results (run); the daemon's side of Cc is by reading `take_up` and the reference's rule.

### Gaps in the tests

**G1. No test holds that a cut held across the start with no block changed is under the floor.** Plant H8 (the `if x[3]` at `:1381` removed, so that "0 blocks" meets the floor) survives. The test covers a holder that wrote every cut before the start, not a cut written across it with nothing changed, which is what cuts 152 and 157 of this run are.

**G2. No test holds that either floor's ERROR reaches the exit.** Plants H10 and A6 (`errored = errored or not across` at `:1387` and `errored = errored or not ok` at `:1395` removed) survive. The fixture's exit is 1 for another reason (`ctl_mark-kept` DIFFERS), so the tests read the line and not the exit. With either line gone a run that showed nothing would end with exit 0.

**G3. The new rule's handling of an acknowledged start is tested in the function and not through `main`.** Plant U14 (`main` calls `unstored_at_kill(instances)` without `acked`) survives, as do U4 (an acknowledged next instance may also "count past one more") and U11 (the count before the kill is not begun again at an acknowledged start). U10 and U15 (shape C whatever stands before the cut line) survive too: no test has a cut line at the end of an instance behind another flush's barrier.

### Suggestions

**G4. Give the take-over's writer a number of groups and not 3 s.** The floor needs the first instance to go 32 groups past the held cut (the uberblock ring). The slowest run I have did that in 0.83 s. A loop that ends 40 groups after the holder left would not depend on the runner.

**G5. A daemon that lets go of what it held at a take-over ends as the rig's error.** T2 in `floors.py`: the cut held across the start is never written and the next written cut has its blocks too. `held_content` PASS, `held_across_start` ERROR, exit 2, on all five recordings. Nothing judges `cuts_held` at a take-over (`status-start-2.txt` says `cuts_held 2` on both images of this run; G3 of #113 judges it only at an acknowledged start, `:1118`). Holding `cuts_held` at a taken-over start to at least 1 where the holder left with a cut held would make it a verdict.

## The description's claims

| Claim | What I found |
|---|---|
| Local tests pass on afd217a | Confirmed here: `test_cut_daemon_check` 90 tests, `test_cut_daemon_main` 22, `test_cut_hold` 17, `test_cut_ref` 41, `test_cut_check` 26; `test_skz_store.sh` and `test_cut_record.sh` all passed. shellcheck is not installed here. |
| Each plant of the table KILLED | See the next table. |
| 42 recordings of #113's runs exit 0 with the new check | Confirmed on the three sets of 14 I have from #113's runs (both images of one run, and a set whose `takeover` has the torn label line): 42 recordings, all exit 0 here, with the description's figures (4, 4, 282 and 1775, 1792, 1793; `rejudge-113.out`). The 28 recordings of this pull request's own run also exit 0 here with the run's lines (`rejudge.out`). |
| `obs_unstored_at_kill`: 0 in every recording | Confirmed on the 28 of this run and the five `takeover` recordings. |
| "The review's figures counted copies another way" | Not so; see question 2. |
| The work is in "the reference (`cut_ref.py`)" | `cut_ref.py` is unchanged; `test_cut_ref.py` has one new test. |
| The cancelled rigs run no changed file | Confirmed by reading the five workflows: none names `cut_daemon_check.py`, its tests, `test_cut_hold.py`, `test_cut_ref.py`, `test_skz_store.sh` or the benchmark's script. Node B's rig uses `cut-record.sh` and `cut_ref.py`, both unchanged. |

**The plants of the description's table, planted here one at a time on afd217a:**

PLANT_TABLE

## Plants

PLANT_SUMMARY

## CI and Codex on afd217a

CI_STATE

## What I could only read

- **Nothing that needs a kernel ran here.** The recordings are CI's; I rejudged them, I did not make them.
- **The daemon under a real `kill -9` in these windows.** I ran the store's own code with a death at each store, in its unit-test harness, not `skyzfsd` under a signal. No recording has hit window B or C (0 in all I have).
- **That no course reaches F2.** By reading ZFS's order of uberblock writes and flushes and the writer's commands.
- **The 16 KiB and 64 KiB page cases of the torn-line rule**, as before: the hosted images have 4 KiB pages.
- **shellcheck** on the two changed shell files: not installed here; CI's `ci` run is green on the head.

## Evidence

All in `review/m2.t3-2d/` on `skyzfs-review`.

| File | What it is |
|---|---|
| `kill_windows.py`, `.out` | the shapes A, B, C, stored, N, M0, M1, C2, Cc through the check |
| `kill_at_stores.rs`, `.py`, `.out` | the store with a death at each store after the `cut` line, and after the `applied` line |
| `window_share.py`, `.out` | each window's share of a killed instance's life, ten instances |
| `floors.py`, `.out` | the floors on five recordings: the counts by both methods, where the blocks lie, the margin, T1 to T3 |
| `scenarios_failed.py`, `.out` | the wrong daemons of the review of #113 on this run's `failed` |
| `restored.py`, `.out` | Codex's case |
| `rejudge.out`, `rejudge-113.out` | the 28 recordings of run 44217572301 and 42 of #113's runs, judged here |
| `next3_bound-afd217a.out`, `next3_p6-afd217a.out` | the time bounds |
| `plants.py`, `plants-*.out` | the plants |

## Review of #107 at b6aaad8 (M2.2d)

**Verdict: can merge once the daemon rig's three jobs on b6aaad8 are green. I found no defect.** One gap in the tests (G1): the new path after a failed run has two rules that no test holds, and breaking either gives a wrong cut with every test passing. The head is right there. I would add the tests in this pull request, since the path is new here and both scenarios are written; I do not count it as blocking.

I read `state.rs` before the description, then ran 13 plants of my own against `skyzfs-local` with `test-hooks`, and two scenarios the head's tests do not have.

### The three questions

**(1) The order of events against M2.2c. Safe, and the same outcome.**

- **Bits after all the part's records.** Nothing between a record and its bit reads the open map: `wanted` and `needed` look only at the held cuts' own maps. A death between the two leaves records whose `hi` is the open epoch and no bits; the write has not begun, so every cut reads the image correctly, and the reissued write finds the records and copies nothing again. That is M2.2c's state for one block, now for up to 256.
- **The bound.** `copied()` is the slots taken (`capacity - free_records.len()`), so a planned copy counts from the moment its slot is popped, and the check `copied() + BLOCK_SIZE > bound()` sees the same figure M2.2c saw after recording. `unplan` asks the same question of a planned copy that `free_unneeded` asks of a record (`unneeded`, with the mutant's `all_of`), so the cuts abandoned and the copies that remain are those of M2.2c.
- **The `lo` of a copy planned before an abandon** is kept. I checked that it is still the right one: M2.2c had stored the record with that `lo` before the abandon too, and a range wider than the cuts still held is harmless.
- **What differs is only where a death can fall:** after the abandon and before any of the part's copies. Nothing is recorded and no bit is set, so it is the state before the change with a cut fewer.

Plants: bits before records (A1) is KILLED by three tests, among them the death at every store. An abandon that drops every planned copy (C1), held cuts not read again after it (C2) and the bound checked only for the part's first copy (C3) are each KILLED, C1 and C3 by the model test.

**(2) A failed run counts as its first block's failure. Acceptable, and I agree with the choice.**

- **What it loses against M2.2c.** A cut older than the one that reads the block truly at fault, if that older cut reads an earlier block of the same run. The usual case would be the cut being uploaded, lost with the one that waits. Its map goes to the open map, so nothing is lost but the recovery point.
- **It is bounded.** Each failed run lets at least one cut go (a planned block is one a held cut reads), and the rest is planned under the cuts still held. So a change sees at most three failed runs, not one per block of a bad run.
- **Short transfers are not a way in.** `write_direct_at` and `read_direct_at` are `write_all_at` and `read_exact_at` (`fs.rs:620`), so a run that is written short is an error, and its slots are not recorded.
- The retry block by block would keep more cuts on an error that is a later block's own. I would not build it for errors this rare.

**(3) The split for M2.2e. Shaped right.** `Plan` is plain data, `Plan::copy` touches only the copier, and `record_plan` takes the plan and the copy's result. Three things M2.2e has to settle, the first two of which the description names:

1. **The record step must ask again whether each copy is needed.** Between plan and record another write's plan can reach the bound, another write's copy can fail, or a cut can be released. Each frees records and cannot see this plan's copies. A copy recorded for a cut that is gone is not a wrong read (no later cut's number falls in its epochs), but it takes room under the bound until the next cut goes.
2. **A slot taken counts as copied.** That is right for the bound with several plans outstanding; only the status figure runs early.
3. **A freeze must not fall between plan and record, and the barrier can see to it.** With a stale `plan.epoch` the record's `hi` is the new cut's number. A new cut whose map holds the block (or that counts the whole volume) reads the block as it was at its instant, which is what this copy holds; it finds no record for it (`number < hi` fails) and reads the image after the overwrite. So the write has to count as in flight from its plan, not from its hand-over to a thread: then the cut's flush waits behind it as it does today, and the epoch cannot move. I would write that into M2.2e's row rather than let the record step repair it.

### G1 (gap). After a failed run, two rules have no test

`record_plan` (`state.rs:1831`), the branch for a failed run. Each plant below passes all of `skyzfs-local`'s tests:

| Plant | What it breaks | In my scenario |
|---|---|---|
| A2: the copies after the failed run are recorded too, though never written | on the replan they count as made by a dead instance and are skipped | scenario Y: cut 3 reads blocks 100 and 170 from slots that were never written |
| A3: the blocks before the failed run get no bits | the cut that stays reads the image for blocks it has copies of, and the next cut's map lacks them | scenario X: cut 2 reads block 20 as overwritten, and the next cut's map lacks blocks 20 to 29 |

The head's tests of a refused copy either have the failure at the change's first copy or end with no cut held, so neither rule shows. Both scenarios need a cut older than the one that goes:

- **X.** On `three_held`, `change(20, 21)` with the second write of copies failing. Runs: block 20 for cut 2, blocks 30 to 32 for cut 3, block 40 for cut 4. Cuts 3 and 4 go. Cut 2 stays, must read block 20 from its copy, and the next cut's map must hold every block of the change.
- **Y.** `change(30, 141)` with the second write failing. Runs: 30 to 32 for cut 3, 40 for cut 4, then 100 and 170 for cut 3. Cut 4 goes alone. Cut 3 stays and must read 100 and 170 from copies made when the rest is planned again.

As the head has it, both come out right (`review/m2.2d/failed_run_scenarios.py` and `.out`, which also has the two plants' output). The test in that script can be lifted with asserts in place of its prints.

### Notes, nothing to do here

- **A4: the failed block's own bit** (`state.rs:1857`, the `insert` before `Ok(block + 1)`) can be dropped without a test failing, and without a wrong result in either scenario. A cut that reads the block gives its map to the open map in `abandon_needing`, and that map holds the block or counts the whole volume. It is M2.2c's line kept, and harmless.
- **D1: the look for a record made by a dead instance** can be dropped without a test failing. It is the same on the base (`plants_base.out`), so the gap is M2.2's and not this package's. Without the look a reissued write copies again and the duplicate takes room; no cut reads wrongly. Worth a test now that the window is 256 blocks and not one, in any later package.
- **How long the runs are in steady state** depends on the order in which freed slots come back. A plan takes the slots on top of the free list and sorts only its own. Slots freed with a released cut come back in the order of their blocks, so a record's 32 copies made as one run tend to come back as one run, and the test shows the best case of that. With cuts released while others hold copies between them, a plan can get fewer consecutive slots. M2.T4's round with releases is where it shows: the daemon's `pwrite` sizes on `copies.img` (`TB_SYSCALLS=1`) say how many blocks a run carries. If runs come out short, reading the image once for a run of blocks and writing its slots in several calls would keep half the gain.

### My plants

`review/m2.2d/plants.py`; results in `plants_ab.out` and `plants_cd.out`.

| Plant | Result |
|---|---|
| A1 a part's bits set before its records | KILLED by 3 |
| A2 after a failed run, the copies not made are recorded too | **passes** (G1) |
| A3 after a failed run, the blocks before it get no bits | **passes** (G1) |
| A4 after a failed run, its first block gets no bit | passes; equivalent (note) |
| A5 after a failed run, the slots of the copies not made stay taken | KILLED by 4 |
| A6 after a failed run, the cuts that need its first block stay | KILLED by 10 |
| B1 a run takes consecutive blocks whatever their slots | KILLED by 2 (the model test) |
| B2 a run takes consecutive slots whatever their blocks | KILLED by 2 |
| C1 an abandon while planning drops every planned copy | KILLED by 2 (the model test) |
| C2 after an abandon while planning, the held cuts are not read again | KILLED by 1 |
| C3 the bound is looked at only for a part's first copy | KILLED by 2 |
| D1 a record a dead instance made is not looked for | passes, as on the base (note) |
| D2 a record's second word stored after its first | KILLED by 1 |

I did not rerun the description's six plants; mine cover R4 to R6 from other sides, and R1 to R3 are counts that the two counting tests assert outright.

### The row's criteria

- **The model test and the mutants pass unchanged:** yes. The diff of `state/tests.rs` changes the existing tests only in their copiers' signatures.
- **A test counts the I/Os of a write over a held cut:** yes, at both levels, and the counts are exact (`reads + 1`, `writes + 1`, and the runs as lists).
- **The order of the rules per block:** holds; A1 and D2 are killed, and a record before its copy is no longer possible by the code's shape (`Plan::copy` returns before `record_plan` runs).

### CI and Codex

- On b6aaad8 every job is green but the daemon rig's three, still running when I wrote this. The cut rig's record-and-check through `skyzfsd` and the replay jobs through the daemon are green, which is the only place this code ran on real files.
- Codex reviewed b6aaad8 and has no findings.

### Files

`review/m2.2d/` on `skyzfs-review`: `review.md`, `plants.py`, `plants_ab.out`, `plants_cd.out`, `plants_base.py`, `plants_base.out`, `failed_run_scenarios.py`, `failed_run_scenarios.out`.

## Review of #100 (M2.2b-1: the cut state in a mapped region, trusted by rule) at 59d09bf

**Verdict: D1 and D2 block the merge.** Each is a sequence after which a start takes up a state that the design says it must not trust. Codex has both. I had reached both from the code before I read its threads, and each has a test here that fails at the head.

- **D1 (blocks):** a start without `--hold-cuts` between two with it. The held cut then reads a block the middle instance wrote.
- **D2 (blocks):** a start that acknowledges a failure and is lost before its second phase.
- **D3 (does not block):** the same family in a short window: a failed sync of the copies, heard by a start that dies before it begins the new state.
- **S1 to S3** are test gaps. None blocks.

Beyond those I found nothing. The unsafe module is sound, the cuts that go when a copy fails are the right ones, and a created device keeps no mark.

CI is green at 59d09bf: 36 check runs, each success or skipped.

The files named below are in `review/m2.2b-1/` on `skyzfs-review`. `reproducers.patch` adds six tests: three for D1 to D3, which fail at the head, and three for S1, which pass at the head and fail under its plants.

### How I checked

- **The code first.** I read `skyzfs-mapped`, `state.rs`, `start.rs`, `inplace.rs` and `serve.rs` against design §5.2's rule on trust and M2.2's row, before the description and Codex's threads.
- **The tests pass here** (rustc 1.100.0): `skyzfs-mapped` 6 with the child ignored, and `skyzfs-local` with its test hooks 184 and 13.
- **Planted changes.** PLANTS_TOTAL, one at a time (`plants.py`; `plants_moa.out`, `plants_t.out`, `plants_a.out`). The tests caught PLANTS_CAUGHT. Of the PLANTS_PASSED that passed:
  - three are S1;
  - four are S3;
  - PLANTS_NOTHING change nothing that matters (listed at the end).

### Defects

**D1. A start that holds no cuts leaves the state as it was, and the next start that holds cuts takes it up.** (`bins/skyzfsd/src/serve.rs:706`; the rules at `inplace.rs:1956`. Blocks. Confidence: high. Run here at the store; the daemon tells the store no more than the test does.)

- The sequence:
  1. `serve --hold-cuts`. Cut 1 is held, and block 5 holds 0x55 at its instant. The daemon stops or is lost.
  2. `serve` with no flag, which is the default. It writes 0x66 to block 5. The store was given no region, so no copy is made and no bit is set. It stops.
  3. `serve --hold-cuts`, in the same boot. The volume's identity is the same and the copies sync, so every rule passes.
- The result (`holes.py`, `holes.out`): `TakenUp, held [1], cut 1 reads block 5 as 0x66`.
- The same follows from a start with another `--run-dir` in between, by the code: the region is in the run directory and the image is not.
- A fix: a start that holds no cuts makes the state unreadable, under the volume's lock, before it serves. The other run directory needs more: the state would have to name something in the volume's directory that every start changes.

**D2. A failure acknowledged by a start that does not reach its second phase.** (`start.rs:545`, where the open takes the marker away; `inplace.rs:1957`, where the start applies the rule. Blocks. Confidence: high, run here.)

- The open removes the marker durably, and the lifecycle then removes the failure note. The new state begins only in `Opened::start`, after the lifecycle has found and attached the device.
- The sequence:
  1. A cut is held, and the volume fails.
  2. `serve --hold-cuts --acknowledge-failure` opens the store. It is killed, or the lifecycle stops (the device cannot be added, say), before the start.
  3. The next `serve --hold-cuts` finds no marker and no note. It acknowledges nothing, and takes the state up.
- The result: `TakenUp, held [1]`.
- M2.2's row names "a state trusted after the volume failed". This is that state.
- A fix: an open that acknowledges clears the state's magic before the marker goes.

**D3. A failed sync of the copies, heard by a start that is lost before it begins the new state.** (`inplace.rs:1956` to `:1980`. Does not block: the window is short. Confidence: high on the code. The kernel's part is from how it reports writeback errors; I have not tried it on a real file.)

- The kernel reports a failed writeback to the first sync after it, as the comment says, and to no sync of a file opened later.
- The start that hears it first runs `CutState::restore`, which finishes what a death interrupted, and only then begins the new state. A start killed in between leaves the state as it was, and the next start's sync succeeds.
- The result, with the simulated disk's fault cleared after the lost start as the kernel would have it: `TakenUp, held [1]`.
- A fix: a word in the region, set before the copies are synced and cleared once the sync has succeeded. A start that finds it set does not trust the state. If the acknowledging open sets it too, that is D2's fix as well.

### Test gaps (none blocks)

**S1. A copy that cannot be made: the tests hold two cuts, and the model never refuses a copy.** (`state.rs:1518`, `abandon_needing`; the tests at `inplace/tests/cuts.rs:1191` and `:1255`. Confidence: high, run here.)

- In both tests the cut that reads the block is the newest held, so "and every newer one" and "newest first" are never exercised. The model test's copier never fails, so no death falls inside the rule either, which M2.2's row asks of the state's rules.
- Three plants pass every test:

  | Plant | What goes wrong | What notices it |
  |---|---|---|
  | Only the cut that reads the block goes | The newer cut stays without the gone cut's blocks: held 2 and 4, and cut 4's map is block 40 alone | `a_refused_copy_lets_go_the_cut_that_reads_it_and_every_newer_one` |
  | The cuts go oldest first | A death between two leaves that same state. With every cut gone, the next cut's `lo` is 3 and not 1 | `a_death_while_a_refused_copy_lets_cuts_go`, and `a_refused_copy_for_the_oldest_cut_leaves_the_next_cut_its_lo` |
  | The open map is not read again after the cuts went | A write over two blocks whose first copy is refused: the second block's bit goes into a map nothing names, and the next cut lacks block 33 | the first test |

- The three tests drive the cut state with three cuts held (`refused_copy.py`, `refused_copy.out`). The second has a death at each store of the refused change.

**S2. The two lines by which the daemon says a device was taken over have no test.** (`serve.rs:776`. Confidence: high, by reading: nothing here runs `serve`'s `run`.)

- At the store both wrong ways are caught. In `serve`, "always" keeps the mark on a created device, and "never" drops it at every take-over: a group whose uberblock write completed before a kill is then not cut by its flush.
- This is for the rig: a kill between a group's uberblock write and its flush, on a device taken over, must show that group's `cut` line.

**S3. Two promises of `skyzfs-mapped` have no test.** (`linux.rs:99` and `:143`. Confidence: high, run here.)

- **Allocated in full.** With `fallocate` taken out, and with only the first page allocated, every test passes. `another_length_is_resized_and_said` counts the file's blocks, but its region is smaller than a page and got its block from the store made before. A new region of several pages, counted right after the open, would hold it.
- **Locked.** With `mlock` taken out every test passes. `VmLck` in `/proc/self/status` after the open would hold it.

### Your four questions

1. **The unsafe module and the order a killed writer leaves.** Sound. I found no way to reach the mapping outside its length or after it is unmapped.
   - What a killed writer leaves does not rest on the memory order at the machine: a store that has been made is in the page when the process dies, and the stores of one thread are made in the order the compiler emitted them. What `Release` adds is that the compiler may not reorder or merge two of them, which it may with `Relaxed`. So the documentation's sentence holds.
   - It holds for one writing thread. `store` takes `&mut self`, which keeps it to one at a time within the process, and the volume's lock keeps a second process away.
   - `Mapped::open` resizes a file of another length when it opens it. That is safe here because the store opens it under the volume's lock. A reader beside the daemon must not open the region with this call.
2. **The trust rules.** D1 to D3. Otherwise as the design has them.
   - The identity rule is right, and needed: without it a volume made again in the same directory passes every other rule whenever the old state names no copy.
   - `acknowledged` is read from the option and not from the marker. That is right: the lifecycle sets the option for a failure note with no marker too.
   - One thing for M2.4, not for this pull request. A copy's page can be lost to a failed writeback within one instance as well, and then be read back from the disk. An upload that read copies would have to sync the copies file before it commits the cut.
3. **A created device keeps no mark.** Yes, at the store: dropping the mark at a take-over, and telling the store "taken over" whatever the caller says, are each caught. The daemon's own two lines are S2.
4. **`abandon_needing`.** The cuts it lets go are the right ones.
   - The cut that reads the block's content now is the newest whose map holds the block. A newer cut would read the same content as soon as it took that cut's map, so it cannot stay.
   - An older cut never reads that content: where its map holds the block, the write that put the block into the newer map made its copy, or let it go at that time.
   - With no map holding the block, only the cut that counts the whole volume as changed reads it. The whole volume is counted in epoch 1 alone, so that cut is the oldest held and every cut goes, as the code has it.
   - A flush's completion for a cut already let go changes nothing (`flushed` and `flush_failed` return).
   - What is missing is in the tests (S1).

### Plants that pass and change nothing that matters

PLANTS_LIST

### Small remarks

- The count of abandoned cuts is stored after the map is given on, as at the bound (my remark on #98), so a death between leaves a cut uncounted.
- `inplace::pool::tests::a_job_wakes_one_waiting_thread` failed once here while three builds ran at once. I did not keep its output, and left it out of the runs after that.

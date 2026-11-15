## Review of #98 (M2.2a: held cuts read as the volume at their instant) at e905c2e

**Verdict: nothing here blocks the merge.** I looked for a sequence, with or without a lost daemon, after which a held cut reads a wrong block or lacks one, and found none. The row's property test and its mutants are there, and each mutant is noticed.

What remains:

- **D1, a small defect in what the next instance tells the status.** It is in code no daemon runs yet. Fix it here or carry it to M2.2b, which adds the status.
- **S1 to S4 are test gaps.** In each, a planted change passes every test of the pull request. S1 is a test that does not do what it says. For the others, `review/m2.2a/reproducers.patch` on `skyzfs-review` adds five tests and two checks. They pass at e905c2e and each fails under its plant.

CI is green at e905c2e. Codex has no finding.

### How I checked

- **The code first.** I read `state.rs`, `state/maps.rs` and the wiring in `inplace.rs` and `cuts.rs` against design §5.2 and M2.2's row before the description and the tests.
- **The tests pass here** (rustc 1.100.0): `skyzfs-local` 178 and 11, one ignored.
- **A second death, inside the restart.** The model test has one death in a run and none in `restore`. I ran 68,390 runs over eight seeds, both bounds and the five fixed programs, 60,307 of them with a second death at a store of the restart (at most 32 stores). None failed (`second_death.py`).
- **What notices each of the thirteen mutants** (`second_death.out`, `without_rules.out`):
  - with the rules model switched off, the content of the held cuts still notices each of the five that the rules notice first;
  - a freeze that a death can split is noticed only by the restart's bookkeeping, as the description says.
- **Planted changes.** 53 outside the pull request's mutants, one at a time (`plants.py`, `plants2.py`). The tests caught 32. Of the 21 that passed:
  - seven change nothing (listed at the end);
  - two matter little;
  - twelve are S2 to S4.

### Defect (does not block)

**D1. Once the newest cut is let go, the next instance says it does not open as itself.** (`inplace.rs:1875`, `take_up_cut_state`. Confidence: high, run here.)

- The region keeps the newest cut's state beside the count, but not whether it opens as itself. The start reads that flag from the held cut whose number is the count.
- Scenario: cut 1 of group 7 opens as itself; it is uploaded and let go (or abandoned at the bound); the daemon is lost. The next instance reports count 1, newest group 7, `itself: false`, and `status` prints it as a rewind (`skyzfs-ublk/src/status.rs:363`). With the cut still held it says `true`.
- Nothing but the status reads the flag, and no daemon holds cuts in this pull request.
- A fix: one more word beside the newest cut's state.
- Reproducer: `newest_itself.py`, with its output in `newest_itself.out`.

### Test gaps (none blocks)

**S1. The conformance suite "while it holds cuts" holds no cut.** (`inplace/tests.rs:164`; `harness.rs:77`. Confidence: high, run here.)

- A cut needs an uberblock write that passes the label checksum and a flush after it. Nothing in the suite or its harness writes one.
- I made the store panic at one moment at a time and ran the suite (`suite_cuts.py`):

  | Moment | In the suite |
  |---|---|
  | a mark is recorded | never reached |
  | a cut is frozen | never reached |
  | a block is copied for a held cut | never reached |
  | a cut is abandoned at the bound | never reached |
  | a block's bit is set in the open map | reached |

- So the test shows that setting bits in the open map changes nothing a client sees. The description's "cuts are abandoned beneath the suite's cases" and the harness's "writes are copied for a held cut" do not hold.
- Each start also gets a region of its own (`harness.rs:91`), so no case takes up a state after its kill.
- To make it hold: the harness writes a sealed uberblock and a flush between a case's requests, and keeps one region across its kills. Or the test says what it shows and no more.

**S2. Through the store, a death is tried only inside the uberblock write and on the caller's thread, and a held cut's block is only ever written, one block at a time.** (`inplace/tests/cuts.rs:1004`, `lost_in_a_mark`, and line 885. Confidence: high, run here.)

Eight plants pass every test:

| Plant | What goes wrong | What notices it |
|---|---|---|
| The start keeps a mark that the newest cut took (`state.rs:1159`), or does not make the newest cut's state good | A death in the cut's flush after the count's store: the flush comes again and the group is cut twice (count 3) | `a_death_at_any_store_of_a_cuts_flush_cuts_the_group_once`: all 42 stores of the flush |
| The next instance is not given the count (`inplace.rs:1882`) | The status counts from 0 again | the same test |
| The next instance is not given the newest cut's state | ZFS writes labels 0 and 2 and flushes (the cut), the daemon is lost, ZFS writes labels 1 and 3 and flushes: the repeat is cut again | `a_repeat_of_the_newest_cut_marks_nothing_after_a_lost_daemon` |
| A write the I/O threads applied records its mark at the next request (`inplace.rs:1603` removed) | The row's "mark after completion", on the path the daemon serves with | The pull request's own death test on one I/O thread: `OTHER_IO[0]` for `ON_CALLER` passes at the head and fails under the plant (`mark_on_threads.py`) |
| A write-zeroes does not pass through the cut state (`inplace.rs:1273`) | The held cut reads zeros | `a_held_cut_reads_as_the_volume_at_its_instant_after_a_write_zeroes`, every I/O mode |
| Only a change's first block is noted | The cut reads the later content of the others | the same test, which zeroes two blocks |
| A copy that cannot be made does not fail the write | The cut reads the block's later content, and nothing says so | No test here. #100 changes this rule, so I will look for its test there |

The model test in `state/tests.rs` reaches the freeze's stores, but its world never sets a mark.

**S3. After a death the model takes over what the start left, unchecked.** (`state/tests.rs:471`, `World::adopt`. Confidence: high, run here.)

Three plants in `settle` pass every test:

| Plant | What goes wrong | What notices it |
|---|---|---|
| A cut whose flush was under way is left so (`state.rs:1138`) | The store no longer knows that flush, so the cut never waits and is never uploaded; the next cut takes its map | In `restart`: no held cut's flush is under way after a start |
| With no cut being uploaded, the one that waits is left waiting | Nothing is uploaded again: each later cut replaces the one that waits | In `restart`: where a cut is held, one is being uploaded |
| A cut recorded and not counted is left in its slot (`state.rs:1105`) | A death in a freeze after the record's first word. Before the flush comes again the cut being uploaded is let go, which frees a lower slot. The freeze done again takes that slot, and the count's store makes the stale record a held cut too (held 2, 3, 3) | `a_cut_recorded_and_not_counted_is_none` |

The third needs a step between the restart and the step done again, which `World::step` never has.

**S4. The order of a copy record's two words.** (`state.rs:1342`, `copy`. Confidence: high, run here.)

- `free_record` clears the block word alone, so a free slot keeps the epochs of its last use. `copy` stores the epochs before the block, which is right. With the two stores swapped, no test fails.
- The case: a slot is freed with epochs [2,4) while cut 3 is still held, its block not being in cut 3's map. It is taken again for block 1, which cut 3's map holds and of which cut 3 has its own copy in a lower slot. A death between the two words leaves a record of block 1 for [2,4). The death view and the next start list a block's records from the highest slot down, so cut 3 reads the newer copy.
- Test: `a_death_between_a_records_two_words`, a fixed program of 17 steps with a death at each of its 193 stores. Under the swap it fails at store 191: cut 3 reads block 1 as 11, and the volume held 8.

### Plants that pass and matter little

- **The look for a record a lost instance made** (`state.rs:1303`). Without it the change done again makes a second copy with the same content. It costs a slot, and so an abandon that comes one copy early after a death.
- **A cut clears the mark whatever it is** (`state.rs:946`). It matters only when a newer uberblock is applied between a flush's receipt and its barrier, which ZFS does not do.

### Plants that change nothing

- The open map takes the donor's `lo` whoever else is held: the word is stored again before it counts.
- `finish` without naming the open map twice: `set_parts` makes the same two stores in the same order.
- `flushed` replaces a waiting cut whatever its number: the cut that waits is always the older.
- `abandon`'s heir for an oldest cut that is not being uploaded: with more than one cut held, the oldest is always the one being uploaded.
- The start's merging of several waiting cuts: not reached.
- `freeze` does not clear the mark the cut took: the start clears it, and the next mark replaces it.
- `move_into` leaves the summary of the part moved from: a stale summary costs passes and no content.

### Your four questions

1. **"Clear every map nothing names."** I found no map the start would clear wrongly. At every store I traced, a map with bits that count is named:
   - the freeze is one store;
   - an heir names the donor's map before the donor goes;
   - a finished move leaves the smaller part clean;
   - the open map is renamed in the word of the count before its second part goes.

   The only map with bits that nothing names is the oldest cut's after it is let go. Both directions are tested: the start not clearing, and `let_go` clearing a named map, are each caught.
2. **Taking a map across a death.** Sound. The direction of the move cannot flip at the next start: the summary of the part moved from is cleared last, so its weight does not fall while words are still to move. The second-death runs cover a death inside the finishing.
3. **The view at a death.** It reads the region as it stands: `view` builds only the index of records and stores nothing. One thing a reader beside the daemon must know. Between the heir naming a donor's map and the donor going, the donor reads a block that did not change since its instant through the fallback in `read`, whose comment says "a defect lost it". There the image is the right answer and no defect, so M2.4's reader must not take that case for an error.
4. **The mark.** As described: `record_cuts` runs after `applied` and before `complete` in all three places, and the freeze is at the barrier in `flush`. A flush waits for every write on the threads, so no copy reads a block under a write. The gaps are in the tests (S2), not in the order.

### Small remarks

- `ABANDONED` is stored after the map is given on (`state.rs:1085`), so a death between leaves an abandoned cut uncounted in the status.
- `read_cut` and `cut_map` (test hooks) do not check that the block lies in the volume. M2.2b's commands should.

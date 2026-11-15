## Look at #100 at 4331a90: N1's fix

**Verdict: N1 is fixed, and the review says it can merge once CI is green on the head.** I found no defect. One small test gap is left, not blocking.

### What I checked

- **The change since 5fd84fb** is fe59e13 (N1's fix in `skyzfs-mapped`) and the merge of `zfs-topology` with #102.
- **The fix works at the store.** `resized_twice.py` now gives the two lengths through `Mapped::open` itself, and the next start shows `Begun("the region holds no cut state (no magic)")` and holds no cut (`resized_twice_fixed.out`). At 5fd84fb the same sequence left cut 1 taken up and reading later content.
- **The tests pass here** (rustc 1.100.0): `skyzfs-mapped`, 9 with 2 ignored.
- **Three plants in the fix** (`plants_next4.py`, `next4.out`):
  - a file of another length only cut or grown: `a_region_cut_short_and_grown_back_keeps_nothing` and `another_length_is_resized_and_said` fail;
  - the file emptied only where it is cut short: `another_length_is_resized_and_said` fails;
  - the file emptied only where it grows: every test passes (below).
- **CI on 4331a90** was still running when I posted: 17 checks passed, 17 running or queued, none failed.

### What still passes (does not block)

**No test reads a region that was only cut short.** With the file emptied only where it grows, a region cut short keeps its first words, and an open with that shorter length would then find it kept. The store refuses such a state by its shape (too few words for the records its header names), so nothing follows from it today. One more assertion in `a_region_cut_short_and_grown_back_keeps_nothing`, that the four words read 0 after the first reopen, would hold it.

### #101 at 359f952

Its only change since 449b20e is the merge of this head. My look at 449b20e stands: it can merge once #100 is in and CI is green.

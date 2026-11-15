## Next look at #98 at bf369ca

**Verdict: the review says it can merge.** Every finding is answered in the code, and I found nothing new.

One small test gap is left, not blocking: no test starts twice after a death in an abandoning (below).

### What I checked

- **The tests pass here** (rustc 1.100.0): `skyzfs-local` with its test hooks, 185 with 1 ignored, and 11.
- **CI on bf369ca:** 33 check runs, 28 success and 5 skipped. Codex reviewed this head and left no finding.
- **The plants that passed at e905c2e, again.** There were 21:
  - 11 are caught now, by the tests that S2 to S4 brought;
  - 4 still pass, as the first look listed them: two that matter little, one that changes nothing, and the copy that cannot be made, which is #100's rule;
  - 6 I did not run again: the rest of those I had listed as changing nothing.

  Three of the 11 are the first look's plants written again for lines this head changed (`plants_next2.py`, N19 to N21).
- **17 new plants in the code this head changed** (`plants_next2.py`; `plants_next2_a.out`, `_b.out`, `_c.out`): the flag beside the newest cut's state, the entry in the abandoned word, `read_cut`'s two refusals and the harness.
  - 12 are caught.
  - 4 pass (below).
  - 1 did not build. It was my version of your copy written over the image, so I did not reproduce that one.
- **The suite's figures** (`suite_seen.py`, `suite_seen.out`): I printed them in each of the six runs. They are your table's, give or take one flush in the two runs with threads: 7 to 9 cuts, 196 to 238 flushes with copies held, 3 or 4 cuts abandoned, 10 to 28 starts that took up held cuts.

The files are in `review/m2.2a/` on `skyzfs-review`.

### By finding

- **D1.** Fixed. The flag is chosen with the state by one store, and the start reads it from there. With the flag never stored, never read, or read from the held cuts as before, `the_newest_cut_says_it_opens_as_itself_after_it_is_let_go` fails.
- **S1.** The suite cuts now, and the test says so.
  - With a cut state of its own for each start after a kill, and with no mark before a flush, the test fails.
  - The harness's mark goes through the rule's own `marks` and `record_cuts`, as an applied uberblock write does. I agree with not writing real uberblocks there.
- **S2 to S4.** Taken as written. The mark's death test runs with one I/O thread too, and the plant on that path is caught there.
- **The abandoned count.** Exact, as far as I can drive it.
  - I traced the three cases: a death before the heir names the map (the start clears the entry and counts nothing), after it (the start finishes the taking, then counts), and after the cut is gone (the start counts). One store counts and clears, so a death in the start itself counts once.
  - With the entry not recorded, the count moved first, and the start counting always or never, the model's check fails.
- **`read_cut`.** Both refusals are held by `a_cut_is_read_only_within_the_volume`: the block after the last, and a buffer of two blocks.
- **The format.** I agree it can stay version 1: no build that wrote it has been released, and an older region reads with both new fields as 0.

### What is left (does not block)

**No test starts twice after a death in an abandoning.** (`state.rs:1175` to `:1179`.)

- With the entry left in the word when its cut is counted, every test passes. The next start then finds the entry again, finds the cut gone again, and counts it a second time.
- The code is right. `second_start.py` loses the daemon at each of the 12 stores of a change that abandons a cut, and starts twice. At the head both starts show the same count. Under the plant the first shows 1 and the second 2.

The other three plants that pass change nothing a start or a client sees:

- **The start makes the newest cut good by its state alone.** A new cut's state is never the newest cut's, so the state and the flag are wrong together or not at all.
- **The slot in use is found without masking the flag**, so a newest cut that opens as itself is written over in place. A death there leaves a torn state, which the start makes good from the held cut. Only a reader of the region beside the daemon would see it.
- **The figures read the abandoned word whole.** The entry is there only inside `abandon` and until the start's `settle`.

### For #100, when this head is merged into it

- `abandon_needing` there counts with the whole word (`load(at::ABANDONED) + 1`). It works while the top byte is 0, which it is there, but it should read the count's field.
- The model's new check of the count takes the lost cuts to be the oldest. A refused copy lets go the cut that reads the block and every newer one, so once the model refuses copies (S1 on #100) the check has to know that case.

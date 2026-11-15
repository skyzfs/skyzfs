## Review of #105 at ccd17dd (M2.3a)

**Verdict: not yet. One defect blocks (D1): two writes of a page while one sync runs leave a bit called durable that the disk may not hold.** Codex's P1 on `intent.rs:487` is the same finding, and it is right; I reproduced it.

The rest reads right to me: the format, the trust rules, the clear rule and `CutState::changed_after`. The test that is named for D1's rule does not test it (G1), and the model test cannot reach it (G2).

### D1 (blocks): a bit cleared and set again during one sync is made durable by that sync

- **Where.** `intent.rs:487`, `synced`: `durable |= word & written`. `written` is the page's latest write only. The module's own rule, and the description, say "every later write of the page".
- **Scenario.**
  1. A page is written with region 2's bit set (W1). A sync begins (S1); it covers W1.
  2. While S1 runs, a cut is committed and region 2 is let go. Its page is written with the bit clear (W2). A clear needs no sync, so nothing waits.
  3. A write into region 2 arrives. The bit is set and the page written again (W3).
  4. S1 ends. `written` is W3, so the map calls region 2 durable, and `covers` lets the write begin.
  5. The power is cut. S1 made only W1 certain. The disk keeps W2 and loses W3.
- **The wrong result.** The start takes the map up (the token and the base are fine), and region 2 is not named, though a block in it changed after the base. The next cut lacks that block.
- **Shown by a run** (`two_writes_one_sync.py`, `two_writes_one_sync.out`): after S1 ends `covers` is true for region 2, and the file with W2 kept and W3 lost reads `trust Ok, names [0, 1]`.
- **A fix that works here** (`fix_check.py`, `fix_check.out`): in `write_begin`, a page written while a sync runs takes from that sync's record the bits the write clears. With it my test passes and so do your ten. `written` is then not needed.
- **Why it blocks.** The store does not use the map yet, but these rules are what this pull request is. A clear's page is written with no sync, so nothing in the rules keeps it out of a sync that is running. Confidence: high, by a run.

### Your four questions

**(a) The durability bookkeeping.** Right except for D1.
- A page written with a bit cleared takes the bit's certainty at once, a failed sync gives nothing, and a new map is not durable before its first sync. Planted wrong, each fails a test.
- The case you name (one later write that clears the bit) is handled by the code. Your test for it does not show that (G1).

**(b) A start that acknowledges a failure takes the map up.** I agree, and found no case against it.
- I went through a failed write and a failed sync of the image, a failed write and a failed sync of the map, a daemon lost after the failure, and the failed instance committing a cut and clearing bits afterwards. In each, every region a write may have changed is named on the disk.
- It rests on three things that are M2.3b's to keep and to test:
  - a failed write or sync of the map fails the volume before the request it guards begins;
  - the start syncs the map's file before it relies on what it read, and does not take the map up if that sync fails;
  - the acknowledging start finds the token of the start before. That holds today because a start refused for the marker writes no token. A test that refuses first and acknowledges second would hold it.

**(c) Whether the model's power cut is strong enough.** For what a power cut does to the map's file, yes. What the model lacks is in its schedule, not in the cut.
- Each unsynced write kept, dropped or torn, in order, covers an older well-formed page beside a newer one. With the checksum made and checked over the first sector only, the model test fails, so torn writes past the first sector are noticed (`fix_check.out`, "sector").
- A sync is one step in the model (`model.rs:240` to `:246`): no write of the map ever falls between `sync_begin` and `synced`. That is why it cannot reach D1 (G2).
- The check ends when the start reads the map. Nothing runs on a map that was taken up: no `kept_by`, no second power cut. I would have M2.3b's store test go through two power cuts.
- On the row's words, see Q1 below.

**(d) `CutState::changed_after`.** Right.
- The open map and every newer held cut are what holds changes after the cut's instant, and it may be called after the release, as the model does.
- P1 and P2 fail the model test here too, and so does leaving out only the cut after the committed one.
- P3 is unreachable today, as you say: the mark is stored only when a state begins.
- A map's second part exists only while a map is being handed on, inside one call. So leaving it out changes nothing a caller sees.
- Small: an index of a map out of range is dropped, which narrows the answer. `None` would be the safe answer there. It needs a damaged state.

### What I checked

- **The tests pass here** (rustc 1.100.0): `skyzfs-local` with its test hooks, 215 with 1 ignored, and 13. The map's ten tests take 6.6 s.
- **Planted changes** (`plants.py`, `plants.out`): 26, one at a time, against the map's tests. 18 fail a test. Of the 8 that pass:
  - two matter (A1 and A4: G1 to G3 below);
  - two are small (G4);
  - one depends on the store (the writes a failed sync covered are covered by the next good sync: moot if the store fails the volume there);
  - three change nothing a caller sees.

  One plant of mine was wrong (the checksum checked over one sector but still made over the page, which fails everything). The right form is the "sector" run above.
- **The first commit** (`7671536`): every changed line is the type and nothing else, the 69 flags are distinct bits, and nothing narrows the bits to 64 on the way.
- **The store's discard** changes no content (`inplace.rs`, `Op::Discard`), so the reading on discards holds.
- **CI on ccd17dd** as I write: 31 checks passed, 3 still run, none failed.

The files are in `review/m2.3a/` on `skyzfs-review`.

### Test gaps

**G1. `a_bit_cleared_by_a_later_write_is_not_made_durable_by_an_earlier_sync` passes when the sync does make the bit durable** (`intent/tests.rs:207`).
- With `synced` changed to `durable |= word` (plant A1), all ten tests pass.
- The test asks `covers`, and `covers` is false because the bit is clear in `bits`, whatever `durable` says.
- Sharper: after the sync ends, set the region again and require that `covers` is still false and the page is to be written (`fix_check.py`, `review_a_bit_cleared_by_a_later_write_and_set_again`). That fails under A1 and passes at the head.

**G2. The model never writes the map during a sync.** Under A1 the model test passes, and D1 is out of its reach. A schedule in which a release, or a write's `set` and page write, can fall between `sync_begin` and `synced` would reach both.

**G3. Nothing holds "a sync covers only the writes issued before it began".** With `synced` also taking the writes issued since the sync began (plant A4), every test passes.

**G4. Small ones.** Each passes every test:
- `kept_by` does not ask for the header to be written. The test's map is new, so its header is written anyway. The cost of the defect would be an upload of the whole volume after the next power cut, and no block.
- The token compared by its first half. The tests' tokens differ in every byte.

### For M2.3b (none blocks this pull request)

- **`sync_begin` says it covers every write issued so far.** A sync covers a write that has ended. With the store's I/O threads a page can be issued and not yet written when the sync starts. Two writes of one page in flight together can also land in either order. The map has no call for a write's end, so the store has to keep both rules, and the documentation should state them.
- **`covers` and `CutState::change` must be one step, with no `clear` between.** A clear that lands after a write's `covers` and before its `change` lets go of a region the write is about to change. A write that waits for its bit has changed nothing yet, so a clear may take its bit while it waits: after `synced` the store has to ask `covers` again. The model does not ask again (`model.rs:258` to `:264`).
- **`Shape` is not checked.** The region must be a power of two of at least 64 blocks (a word of the cut state's maps must lie in one region), and 0 divides by zero.
- **The header is rewritten at every commit with no sync.** A header torn by a power cut costs an upload of the whole volume. It is a cost only; two header pages written in turn would remove it.

### For the lead

**Q1. What "the state file left as a power cut leaves it (a well-formed state from seconds earlier included)" means in M2.3's row.** The description reads it as the map's file, with a dropped write as the state from seconds earlier. I read it as the cut state's file, which can survive a power cut on a file system that keeps it, well-formed and stale. The boot rule refuses it, and the store has to carry the map's regions into the new state. The model here has no cut state after the power cut, so on my reading that clause, like the fourth mutant, is M2.3b's to show. It does not change this pull request.

---
_Generated by [Claude Code](https://claude.ai/code)_

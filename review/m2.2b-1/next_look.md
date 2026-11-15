## Next look at #100 at 07a9f2d

**Verdict: not yet. The daemon rig is red on this head, for one line, and the start token is the cause (B1).** It needs a decision from the lead more than a fix.

In the code, D1, D2 and D3 are fixed and I found nothing new that blocks. One question on the design is open (Q1). It is Codex's new P1 and a case of my own, and in my view it does not block this pull request.

### B1 (blocks): a refused start rewrites `volume.start`, and the daemon rig says that is wrong

- **Where.** `inplace/start.rs:557` to `:560` writes the token in the open. The refusal for a file system with no direct I/O comes later, in the start (`inplace.rs:1010`).
- **What fails.** `pin_data_journal_refused` (check g.4) in all three daemon jobs of [run 43687498505](https://github.com/skyzfs/skyzfs/actions/runs/43687498505): `FAIL: wrong: volume.start rewritten`. The pin asks that a start refused on `data=journal` changes no file of the volume but the status file. It is the only line that differs from the table, on 24.04, 26.04 and 26.04-arm. The two stand-in jobs pass.
- **What it means.** The refused start left a new token, so the next good start does not trust the state and begins a new one. That is safe. The cost is that the cuts held before the refused start are gone, and the first cut of a new state counts the whole volume.
- **The decision, for the lead.** Either is consistent with design §5.2 as I read it ("every start changes [the generation] durably before it serves or acknowledges anything"):
  1. **Keep the code and widen the pin** to allow `volume.start` to change at a refused start. This is what I would do. "Every open writes a token" is a rule with no exception to reason about, and the cost is the one you already name for a start lost between its open and its take-up.
  2. **Write the token later**, after the refusals that come before the start changes anything. An acknowledging open would still have to write it before the marker goes. The rule then rests on "nothing was changed yet", which is the kind of reasoning D1 and D2 got wrong.

  The pin is in `tests/kernel/`, so option 1 is a change to the rig and its table, here or by testing.

### What I checked

- **The tests pass here** (rustc 1.100.0): `skyzfs-local` with its test hooks, 203 with 1 ignored, and 13; `skyzfs-mapped`, 8 with 2 ignored.
- **CI on 07a9f2d:** 3 jobs failed (B1), the rest passed or were skipped.
- **The first look's plants, again** (`plants.py`, and `plants_next.py` for the six whose lines changed). Of the 17 that passed at 59d09bf:
  - 11 are caught now: A1, A2, A7 to A11, M2, M2b, M3 and T14;
  - 6 still pass as the first look listed them (M6, O1, O2, T1, T5, T11), and none of them matters much.

  The 19 that were caught are still caught. Your four rewritten plants (A6, A7, A10, T13) match mine in meaning, and mine are caught by the tests you name.
- **9 new plants in the token's code** (`plants_next.py`, `next_new.out`): 8 caught, 1 passes (below).

The files are in `review/m2.2b-1/` on `skyzfs-review`.

### The three things you asked about

- **The start token (`start.rs`, `volume.rs`).** It reads sound to me.
  - Every open writes one, held cuts or not, and the lifecycle opens once per start (twice only after a refusal it then acknowledges).
  - A marked volume that is not acknowledged is refused before any token is written.
  - The region is mapped before the token and the marker, so a start refused there leaves the volume as it was.
  - Planted away one at a time, each of these fails a test: an open with no cuts writes no token, the token is not synced, the open reads no token, a token file that exists is not rewritten, the token is written after the marker goes.
- **The read-only `view` before `take_up` (`inplace.rs`).** Right, and the order inside `take_up` is right: the token first, then the rest.
  - A death between the token's two words leaves a mixed token, and the next start begins a new state. That is a cost only.
  - With the token recorded after `settle`, or not at all, `a_start_lost_after_it_trusted_the_state_leaves_it_to_the_next` fails.
  - With a new state begun under the token the open found, eleven tests fail.
- **`an_acknowledging_open_lost_at_any_write_leaves_the_marker_or_a_new_token`.** It reads right, and the simulated disk's halt is the right tool for it. It is the only test that fails when the token is written after the marker goes.

### Q1 (for the lead; does not block here, in my view): what the next start can know about the instance before it

The token closes every case where an instance left a trace. Two cases are left where it left none, and in both the next start takes up a held cut whose copy may be lost.

- **Codex's P1** ([thread](https://github.com/skyzfs/skyzfs/pull/100#discussion_r4577402997), `inplace.rs:2041`). I think it is right about the kernel. The copies are buffered writes. If writeback of one fails after the daemon is lost, and the inode is evicted before the next start, the error is gone and the start's sync of the reopened file succeeds. The notes already record this kernel behaviour (footnote `wberr`), from M1.2.
- **A volume that failed with no marker and no failure note.** `failed_unmarked.py`: cut 1 is held, the disk stops taking writes, a write fails and the volume fails, the marker cannot be written, the daemon is lost, the disk comes back. The next start, told of nothing to acknowledge, shows `TakenUp, held [1]`. In the daemon the lifecycle's failure note covers this, unless the daemon is lost before it writes the note. This is older than this head.

Neither is this head's doing. The design itself says "the next instance's sync at its start counts as the cut's flush", and the code does that. No cut leaves the machine before M2.4, so I would settle it before M2.4 and not here.

One way that covers both: write the copies by direct I/O, as the image is written. A copy that cannot be written is then refused in the instance that makes it, which already lets its cuts go, and nothing of a copy is left in the page cache to lose. A checksum for each copy in the state would also do it, at a cost in the state's size.

### What still passes (does not block)

- **K8: the start takes the state up before it has judged the copies.** Every test passes. It matters in one sequence: the copies' sync fails at a start, and the daemon is lost before it begins a new state. Under the plant that start has already recorded its token, so the next start trusts the state. No test loses the daemon at each store of a start that heard the failed sync, as `a_start_lost_after_it_trusted_the_state_leaves_it_to_the_next` does for a start that trusted the state.
- **T1, T5 and T11**, as at the first look: the rule on a region newly made (a new file fails `view` anyway), a copies file that cannot be opened counted as missing, and the identity compared by its first word alone.
- **M6, O1 and O2**, as at the first look: the region's file readable by all, and a named region counted as kept at another length or whatever the open said.

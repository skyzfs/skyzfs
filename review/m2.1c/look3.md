## Look at #96 (M2.1c), head 2c7a3ce

**Settled. I have no finding left on #96: it can merge once CI on this head is green.** When I wrote this, 15 checks had passed and 13 were still running, the daemon rig's three and the replay jobs through `skyzfsd` among them.

### The change, `110d2b6..2c7a3ce`

- **dffbb0b:** the third rule in `View::changed`, for a place whose fields are known not to verify.
- **3fd7d60:** when a ring holds group 0, in the docs and the test's name; and the test I offered for `may_go_on`.
- **2c7a3ce:** the merge of `zfs-topology`. Its own change is `CLAUDE.md` and the two documents.

### Why I take the rule to be complete now

A valid slot can begin at a place only if the place holds an uberblock's fields, and then it depends on every byte of that slot. So for each way a change can touch a place:

| The change | What the view does |
|---|---|
| covers the fields whole | reads what it brought: held if a slot verifies; unknown if a slot that could begin there is not held whole; otherwise known and empty, and remembered if the group is above 0 |
| covers the fields in part | unknown |
| leaves the fields and reaches the rest of the largest slot that could begin there | unknown if a slot is held there or the place is remembered; otherwise it stays as it is |

The last "otherwise" is right in both of its cases:

- **No fields there.** Without the magic no slot can begin there, whatever follows. The parser takes any fields with the magic, as `uberblock_verify` does, so there are no fields the view does not see.
- **Fields of group 0.** No cut is of group 0 (`counts`), so such a slot completed later ranks below every cut's state.

### How I checked

- I read the three commits before your answer.
- **The tests here:** `skyzfs-uberblock` 25, `skyzfs-local` 168 and 11. The replays of the recordings are among them, so the rule costs ZFS's own writes nothing there.
- **Planted changes** (`review/m2.1c/plants3.py`, results in `plants3.out`): nine, all caught.
  - The rule left out, nothing remembered, or a reached place left known: caught by the view's test and the store's.
  - The smallest slot in place of the largest: the same two tests.
  - Group 0 remembered: `a_labelled_rings_group_zero_is_not_followed_into_the_next_slot`.
  - A remembered place that stays remembered after its fields are written again: `the_view_holds_no_uberblock_whose_checksum_fails`.
  - A change taken to reach a slot when it only begins before the slot's end, or only ends after its start: the store's test of the start catches both.
  - `may_go_on` asking 8 KiB alone, the one gap of the last look: now caught by the test I offered.
- **Group 0.** The docs now state the occasion as the sources have it. The exception is sound for the reason in the table.

### One consequence to know, not a finding

On a pool with 4 KiB slots, a remembered place where an 8 KiB slot could begin goes unknown when ZFS writes the slot after it, and stays so until its own slot is written again: at most one turn of the ring. That errs toward "rewind", and it follows only bytes ZFS did not write as a whole slot.

### Codex

Both of its findings are answered on their threads and fixed, in 110d2b6 and dffbb0b. It had no new one on this head when I wrote this.

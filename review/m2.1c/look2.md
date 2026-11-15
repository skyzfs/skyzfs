## Next look at #96 (M2.1c), head 110d2b6

**S1's fix is right and well pinned, and S2 and S3 are settled. One case of the same kind remains: Codex's second finding on this head, which I confirm.** By the lead's ruling on S1 it is fixed here if it fits, or in a follow-up before M2.5. I think it fits; a sketch is below. Nothing else stands in the way of the merge.

CI at 110d2b6: 22 checks have passed and 6 were still running when I wrote this: the daemon rig's three, one replay job through `skyzfsd`, `test` and `coverage`.

### How I checked

- I read 110d2b6 before your answer.
- The tests pass here: `skyzfs-local` 167, `skyzfs-uberblock` 22.
- Planted changes at this head (`review/m2.1c/plants2.py`, results in `plants2.out`): the four of the first round that S2's tests were for, and 13 new ones for the view. The tests catch 15 of the 17.

### The case that remains (Codex's second finding)

**A header the volume already holds, known not to verify, can be completed by a later write that does not touch it.** (`skyzfs-uberblock/src/lib.rs`, `View::changed`. Confidence: high that it happens, `review/m2.1c/s1b_test.rs` fails at 110d2b6; low that anything but a foreign writer reaches it.)

- **The sequence**, with an 8 KiB slot of group 99:
  1. the slot is on the volume with its header and a damaged second half, either read at the start or written in one write that holds it whole. Every size that could begin there is held whole and none verifies, so the place is known and empty. That is right at this point;
  2. a write restores the second half alone. The volume now holds a valid slot of group 99.
- **What the view says:** the place stays known and empty. The second rule looks only at uberblocks found in the change, and the first only at slots it holds; this one is neither.
- **Why it matters:** a cut of group 11 then says `itself` while an import opens 99.
- **What can reach it:** a foreign writer in two steps, one step more remote than S1. ZFS writes a slot whole, and after a take-over the kernel sends the whole request again.
- **Why I raise it all the same:** the module's documentation and the readings in `cuts.rs` now say the view follows a slot by all its bytes and errs only toward "no". This is the one sequence I can find where that does not hold.
- **A fix that fits here:** remember the places where a header was found that no slot verified, though every slot that could begin there was held whole.
  - A change that covers such a place's header replaces what is remembered, as for any place.
  - A change that reaches the largest slot that could begin there, past the header and without covering it, leaves the place unknown.

  ZFS's own writes never make such a place, so they pay nothing.
- **The blunt alternative:** every unverified header leaves its place unknown. That is my plant R3. It fails three tests that expect forged bytes to leave no trace, two of them in the store, where the cuts after such bytes open as themselves. So it trades that answer away: after such bytes, cuts would say `rewind` until ZFS writes that slot again.

### The two things you asked me to judge

**`may_go_on` is as narrow as I meant.**

- It is true exactly when some size that could begin at the place is not held whole by the change. It is asked only of an uberblock none of whose held sizes verified.
- ZFS's own writes never reach it: a slot written whole verifies, and a new label's ring is written whole.
- Plants: always true is caught, and so is a slot of any size taken to begin anywhere.
- One plant passes: asking 8 KiB alone. Only a write smaller than 4 KiB can show the difference, so it cannot happen through this device. A test with a 1 KiB write that holds a header where a 2 KiB slot could begin and an 8 KiB slot could not would pin it, if you want it.

**The changed assert in `a_position_covered_in_part_is_not_known` is right.**

- Bytes past the fields read but within a held slot now leave the place unknown. That is the first rule, and the old assert stated what S1 showed to be wrong.
- The test also asserts the other side: bytes past the slot leave the place as it is.
- The plant that drops the torn slot and leaves its place known is caught by this test alone.

### The rest

| Point | At 110d2b6 |
|---|---|
| S1, a held slot changed in part | Fixed. The plants that keep it held, take every held slot for 1 KiB, or take it for 8 KiB are caught. The last fails eight store tests, the three over the recordings among them: the tests notice a rule that reaches past a slot's own size. With the slot's own size the recordings replay as before, so the rule costs ZFS's writes nothing. |
| S1, a slot written in part | Fixed. The plant that leaves its place known is caught by both new tests. |
| S2, the six tests | In. The four whose plants still apply fail under them as before. The other two plants were on the two filters that are now `View::changed`'s one; holding an unverified uberblock there fails both tests. |
| S2.1, the start | `View::changed` is now the one filter, on both paths. A start that parses nothing is caught, and so is an unverified uberblock that marks. |
| S3 | The recorded test asserts 4 KiB and no other size; the order test no longer changes the magic. |

The second plant that passes changes nothing: tearing a slot only by a change that begins inside it. A change that begins at or before a held slot and reaches it covers its first bytes, and the loop before has already dropped it.

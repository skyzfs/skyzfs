# Design read: §5.2's rules for M2 (zfs-topology 744db83, and the repeat rule at 5d3ad81)

Asked by the lead on 2026-11-14. Sent to the lead only; nothing posted on GitHub.

## Findings, most severe first

### 1. At the bound, the abandoned cut's copies are freed, but the cut that takes its map needs some of them (changes the design: §5.2's bound rule and D7)

**The rule.** At the bound, the cut being uploaded (U) is abandoned: "its copies are freed, its map goes to the newest cut" (W).

**Why W needs some of U's copies.**
- W's own map covers only what changed after U's instant.
- A block in U's map that W's own map lacks was not written between the two instants, so W's content for it is U's content.
- If that block is overwritten after W's instant, the copy made is U's, because the block is only in U's map.
- Free it, and W reads the block from the home image, that is, from after W's instant.

**A concrete case.**
- At a 4 KiB block, the uberblock ring has 32 slots.
- The last upload was at group 100, U is at 140, and W is at 150.
- Group 151's uberblock write goes to slot 151 mod 32 = 23. Group 119 last wrote that slot, so it is in U's map and not in W's.
- The write therefore copies the slot for U.
- Copies are made during a sync, so the bound is most often reached between a cut's instant and the next cut.
- Suppose it is reached before group 151's cut. U goes, its copy of slot 23 is freed, and W starts its upload with slot 23 holding uberblock 151.
- W then imports at group 151, whose blocks it does not hold.
- If a publish was handed to W, the published snapshot is corrupt.

**Fix.**
- U's copies of blocks outside W's own map become W's copies; only U's copies of blocks in W's map are freed.
- The copies handed over were all made after W's instant, so they are at most one group's worth, and the bound still frees nearly all of them.
- Stated generally: a copy is freed only when no held cut needs it.

**Replacing the waiting cut is right as written.** Every copy of W holds a block that the newer cut's own interval rewrote.

**Plan, M2.2.**
- Add a mutant: "an abandoned cut's copies freed that the cut taking its map needs".
- The property test must reach the bound with a waiting cut, and with an overwrite between that cut's instant and the bound.

### 2. A volume failure leaves the cut state trusted (changes the design: §5.2's trust rule)

**The gap.**
- The mapped state is trusted "only in the boot that wrote it, and only if no sync of it has failed".
- A failed volume is reopened for a new client "as after a power cut" (§4.1), in the same boot. Its cut state is then trusted.

**Held cuts.**
- A block of a held cut that was overwritten after the cut's instant may now hold the new content (the volatile-cache disk allows it).
- Its copy was never flushed, so a power cut's rules allow it to be gone.
- An upload resumed on the reopened volume would put a wrong block into the recovery point. A failure is exactly when the recovery point gets used.

**The mark.**
- U(N) applied whole sets the mark.
- The volume then fails before a flush consumes it: U's completion fails with the volume (rule R11), or ZFS sends no flush after a failed uberblock write and suspends.
- On reopening, U may be gone. The pool imports at N−1, and the import's first flush makes a cut recorded as N whose volume is a pool at N−1. The recorded group errs high.
- Its content is whole, and it is counted during the import, before any agent can read C (see the answer below), so no publish can take it.

**Fix.** "Trusted only in the boot that wrote it, if no sync of it has failed, and if the volume has not failed since." A failure drops the state as a power cut does, and the intent map covers the rest.

**The same for the mark alone.** "The next instance gets the write again from the kernel" holds only while the attachment lasts. A device deleted with the write unacknowledged ends it too. Rule: a new attachment starts with no mark.

### 3. The waiting cut is replaced at the newer cut's instant, before its flush has completed (changes D7's text; low to medium)

- If that flush fails, the newer cut is abandoned and the volume has failed, but W, whose flush did complete, is already gone.
- The last good cut is worth most exactly then, when the local flash is failing.
- **Fix:** replace W only when the newer cut's flush completes.
- **Open, for the lead:** are held cuts whose flush completed uploaded after the volume fails, within the same instance? Their content is still readable there, and after a restart finding 2 drops them. I would say yes.

### 4. Rule 1's second half has no test in M1 (plan, and the design's claim "both rules are M1's and have tests of their own")

The second half is that nothing received after the flush begins before it.

**What I planted and ran.**
- **The narrow plant.** In `accept` (crates/skyzfs-local/src/inplace.rs:1281 at 744db83), a request that need not wait for the threads may pass flushes that are waiting. The order among the other requests is kept.
- **Result:** all 150 tests of `skyzfs-local` pass, plain and with `test-hooks`.
- **The broader plant**, letting a request pass anything that waits, is caught only through the overlap reordering it also causes: five whole-program tests fail.

**Why M1 never tested it.** For a flush's durability it never mattered. For a cut it is half of the instant.

**What the code does.** It keeps the rule: `accept` defers a request while anything waits, and `poll` releases from the front only.

**The complete-pool argument does not need it.** A request that passed the flush was sent by ZFS before its flush after the uberblocks completed, so it never overwrites what N reaches. But M2.T1's reference and M2.2's model define the instant by it.

**Fix.** A test and a mutant, in M2.1.

### 5. M2.1's mutant "a cut at a flush received before the uberblock write was applied" fails no single-instance test (plan)

- **Within one instance it is harmless.** The store makes such a flush wait behind the write, so its barrier comes after the write.
- **"Applied, not received" matters only across a lost daemon:**
  - the mark is set before the write is whole;
  - the mark is carried to the next instance;
  - the kernel reissues requests in any order, and the next instance gets the reissued flush before the reissued write;
  - the cut then lacks the uberblock.
- **The test must drive that order.**
- **The mark must be in the mapped state before the write's completion is handed to the transport.**

### 6. The publish's hand-over assumes the snapshot is still in the pool at the later cut (design text, §5.2's publish bullet; low)

- A `zfs destroy pool@t` between the two instants makes `snap/t` name a cut without the snapshot.
- With D7 the window is as long as an upload, not as long as a group.
- **Fix:** a hold on the snapshot (`zfs hold`) from its creation until the swap (M2.6).

### 7. A cut whose flush was in flight when the daemon died (plan detail, M2.2 and M2.4)

- That flush never completes.
- The next instance's start sync gives what the rule wants: the local pool not behind object storage.
- So say that such a cut counts as flushed after the start; otherwise it waits forever.

## The lead's questions

### (a) Is the volume at the barrier a complete pool at N under rules 1 and 2?

Yes, with finding 2 excepted. Cases checked:
- **A flush received while U is on a thread:** it waits behind U.
- **A stray flush between the uberblock copies:** the barrier holds every copy received before it, and one is enough.
- **A lost daemon before U is whole:** no mark is set, and the reissue makes it.
- **A lost daemon between applying U and acknowledging it, with the reissued flush first:** the cut holds the U that the lost instance applied whole.
- **A reissued U:** a repeat of the mark.
- **Multihost writes:** below.

### (b) Do replacing and abandoning ever leave a later cut wrong?

- **Replacing:** right as written.
- **Abandoning at the bound:** finding 1.
- **When replacing happens:** finding 3.
- **The publish's hand-over:** finding 6.

### (c) Is the store's barrier in the code what rule 1 says?

Yes.
- **The instant** is the flush's execution, reached only when nothing waits and no write is on the threads; nothing received after the flush is accepted before it.
- **A write-zeroes** runs on the caller's thread and is whole before the next request.
- **Joining a sync.** A flush joins a sync only if `unsynced` is false. U's completion sets it, so a cut's flush never joins a sync that began before U.
- **A discard does not wait** for an overlapping write on the threads (`must_wait` is false for it). So "requests that overlap take effect in the order received" is not true of discards.
  - Rule R6 allows either content, so the cut is right if the map is updated on the caller's thread.
  - The text should say "writes and write-zeroes".
- **"A flush received while it is still being written marks no cut"** is stricter than needed, and harmless.

### (d) The repeat rule at 5d3ad81: can the recorded group err high?

**Err low: holds.**
- A multihost write carrying N−1 that is received before ZFS's flush after N's uberblocks is applied before that flush's barrier. The newest cut is then still N−1, so it is caught.
- One received after that flush is not caught (the block layer does not order a write against a flush).
  - It makes a cut recorded as N−1 whose volume is a complete pool at N or later.
  - Its group errs low.
- Multihost writes carry the last synced uberblock, so they can never carry a group higher than the pool's.

**Err high: only for a cut made during an import.**
- **How a mark survives** into a later attachment in the same boot:
  - the volume fails (finding 2);
  - or the device is deleted with U unacknowledged and the pool then goes through a forced export and an import.
- **What the stale mark makes.** On the new attachment, the import's first flush consumes it. The cut is recorded with the old group, while its volume is the imported pool, whole.
- **Why no publish can take it.**
  - As I read OpenZFS, a writable import syncs a group before it returns, so that flush comes before any agent can read C.
  - A publish begun before does not outlive its attachment.
  - This rests on my reading of the import; M2.T1's recordings of an export, an import and a rewind import can show it.
- **Export.** I agree that export consumes the mark. `vdev_config_sync` ends the export's last group with the flush after the uberblocks, and the contract never fails a flush for a condition that can recover.

**For the plan.** The newest cut's group and root block pointer must be in the state that outlives the daemon, for the repeat rule after a restart. The state's list names the count of cuts and the mark, not those two.

## Evidence

- `skip.py` and `skip.txt`: the broader plant. Five whole-program tests fail, by overlap reordering.
- `skipflush.py` and `skipflush.txt`: the narrow plant. Nothing fails, plain or with `test-hooks`.

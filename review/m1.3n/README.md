# M1.3n (skyzfs/skyzfs#34): several readers, and the status thread's wakes

Reviewed: 07be854 ... 7894d66 (skyzfs-coding).

## What was done

- Read the diff of store.rs and status.rs (code) whole, and the
  publisher's loop around it.
- fmt, clippy -D warnings clean. skyzfs-local 238 pass (3 ignored),
  skyzfs-ublk 127, skyzfsd 49.
- Three plants.
- Not done: no kernel run; the tests' text read only as far as their
  comments; cli.rs and serve.rs by their tests; the description not
  read.

## Findings: nothing that blocks

1. Test gap in part 2. An at-once post of the log (a step's begin
   and end) must wake a publisher that is waiting out its interval.
   With that removed all 127 tests of skyzfs-ublk pass: the write
   then comes up to 100 ms late, and nothing notices. The code is
   right; the rig's kills inside steps rest on it.
2. Note. The lookup's lock is now read by several threads while the
   worker takes it for writing at every insert. It depends on the
   lock letting a waiting writer in before new readers.
3. Note. A reader keeps its handle on a segment file by the file's
   number. That is right while the log reuses its files in place, as
   it does; it was so with one reader.

## Agreed with

- The barriers need no change. A reader enters under the mutex the
  barrier looks under, and is counted in the generation of that
  moment. One that entered before the barrier began is waited for;
  one that enters after looks up after the index's drop (second
  barrier), or finds the batch's blocks still in the index (first).
  That is per reader and does not depend on their number.
- The queue: its lock is held only for the take, or by one idle
  reader across its wait; no read is executed and no panic can occur
  under it, and a poisoned lock is taken all the same. When the
  reads' sender goes, each reader in turn finds the queue closed.
- A mismatch from several readers: each hands its read to the
  worker, whose failing of the volume is the same the second time.
- Order: the worker inserts into the index before a write's
  completion can be taken, and any reader looks up after it took the
  read. The pump keys completions by tag.
- The time account: every reader's account is there from the start.
  Plant "only the first reader's account is shown": caught, by the
  assertion that the readers' moment moves three times the worker's.
- Part 2: every post and the publisher's decision are under one
  lock. A post that does not wake is written at the end of the
  interval the publisher is waiting out, as before. Plant "a post
  never wakes a publisher that waits for a change": caught, 2 tests.

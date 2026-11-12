# M1.3m (skyzfs/skyzfs#33): the home image's sync leaves the worker

Reviewed: e8746ae ... 19ca666 (skyzfs-coding), one commit.

## What was done

- Read the diff of store.rs (whole), io.rs, fs.rs, sim.rs, lib.rs,
  serve.rs and status.rs (code), and the unchanged second half of a
  batch (`finish`), `set_hold`, the public checkpoint calls and
  `Log::reclaim`.
- fmt, clippy -D warnings clean. skyzfs-local 231 pass (3 ignored),
  skyzfs-ublk 126, skyzfsd 48; test_skz_time.py passes.
- Five plants, each a full run of the store's tests.
- Not done: no kernel run; the description not read; the tests' text
  read only as far as their comments; status tests and skz_time.py by
  running them; the rigs are the lead's.

## Findings

1. A helper that cannot be started fails and marks a healthy volume.
   `sync_begin` starts the helper the first time a step hands over:
   a `dup` of the descriptor and a new thread. Either can fail for
   want of a resource (descriptors; a thread under a memory or a
   process limit, and the daemon locks its memory, so a new thread's
   stack is locked too). That error is not "the helper ended", so the
   step takes it for a failed write of the home image: the volume
   fails and the log is marked. Nothing was wrong with the disk, and
   the worker could have made the sync itself. It also comes late:
   at the first step, after 128 MiB of log, not at the start.
2. No test for "a volume that failed meanwhile does not take the step
   up": with that line removed all 231 tests pass. Today the log
   refuses to move its start once it has failed (`Log::reclaim`), so
   the line is a second guard.
3. The simulated disk: a sync that ends later sets the durable length
   to the length at its begin, even if a whole sync in between made a
   greater length durable. The store cannot reach it (the home image
   does not grow, and nothing syncs it in between).
4. The home image's hint of M1.3k is now off unless asked for, and
   `step_home_sync` means the hand-over where the helper syncs.
5. The store's tests take 123 s here, where they took 65 s.

## Agreed with

- Nothing writes or deallocates in the home image outside the first
  half of a batch, recovery and create. Nothing moves the log's start
  outside the second half. Every way into either waits for the report
  first: the rule's step, an explicit checkpoint, a step asked for,
  room in a full log, a change of the hold, the end, and the serve of
  a store that had handed over.
- The second half drops the index by position, so a block written
  again while the sync was out keeps its newer record.
- The preimage file is written only at a batch's begin.
- The error: the helper's descriptor is a duplicate, so it shares the
  worker's error state, and the helper's report is the only sync of
  the file in between. The other descriptors on the home image (the
  reader's, the direct ones) never sync.
- Stop while the helper syncs: the worker waits for the report, takes
  the step up and ends. A worker that panics drops the file, which
  waits for the helper and discards its answer; the start has not
  moved.
- The three conservative readings are safe: in each the start has not
  moved, so the next open applies the whole batch again and syncs it.
- The simulated disk's sync in two points: what it leaves after a cut
  is among what a real fdatasync on another thread can leave. With no
  delay, begin and end are adjacent, draw nothing from the seed, and
  count for a halt at the end.
- Plants caught: an explicit checkpoint that does not wait (2 tests),
  a change of the hold that does not (1), the end that does not (1),
  no wait at the high mark (1).
- Status version 8: 1856 + 13 fields = 1960 bytes, 220 fields.
